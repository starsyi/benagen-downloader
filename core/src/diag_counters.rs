//! §6 的量化证据计数器（规格 §6.1）。
//!
//! **为什么挂在 `Kernel` 上而不是自愈线程的局部变量里**：自愈线程确实跨换引擎存活，
//! 但 RPC 的延迟是在**协议循环**里量的（`engine/rpc.rs`），那条路够不到线程的局部变量。
//! 而 `Kernel` 跨换引擎存活（换的只是它内部的 `daemon` 字段）⇒ 挂在它上面，两边都够得到。
//!
//! **为什么用原子量**：这些字段在**每一次 RPC** 上被写。抢内核那把 `Mutex` 会让
//! 一次统计污染整个协议循环 —— 而那正是本规格要修的那类毛病。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::engine::supervisor::Rung;

/// 延迟分桶：`<1s` / `1–5s` / `5–10s` / `≥10s`。**左闭右开**，四桶覆盖全区间。
pub fn bucket_of(took: Duration) -> usize {
    let s = took.as_secs();
    if s < 1 {
        0
    } else if s < 5 {
        1
    } else if s < 10 {
        2
    } else {
        3
    }
}

/// 一次读数的快照（写日志用）。**纯数据**，便于测试与打印。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub rpc_calls: u64,
    pub rpc_failures: u64,
    pub rpc_buckets: [u64; 4],
    pub rpc_worst: Duration,
    pub rung_reconnect: u64,
    pub rung_pause_all: u64,
    pub rung_swap: u64,
    pub engine_restarts: u64,
    pub task_stalls: u64,
    pub stall_total: Duration,
    /// 构造时带了 IP 备胎的任务数（`backup_uris > 0` 的次数）。
    pub multiip_tasks: u64,
    /// 这些任务身上的备胎 URI **总数**（域名 URI 不算）。
    pub multiip_backup_uris: u64,
    /// 换腿线程从下载中的任务上摘掉的腿**总数**（`plan.del.len()` 逐次累加）。
    pub leg_trim_del: u64,
    /// 换腿线程补上的 IP 腿**总数**（`plan.add.len()` 逐次累加）。
    pub leg_add: u64,
}

impl Snapshot {
    /// 键序**是契约**：读日志的人按这个顺序找字段，加字段只能**往后加**，不许插队、不许改名。
    /// 被 `a_snapshot_renders_every_field_the_spec_asks_for` 逐项钉住。
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("rpc_calls", self.rpc_calls.to_string()),
            ("rpc_failures", self.rpc_failures.to_string()),
            ("rpc_lt1s", self.rpc_buckets[0].to_string()),
            ("rpc_1_5s", self.rpc_buckets[1].to_string()),
            ("rpc_5_10s", self.rpc_buckets[2].to_string()),
            ("rpc_gt10s", self.rpc_buckets[3].to_string()),
            ("rpc_worst_ms", self.rpc_worst.as_millis().to_string()),
            ("rung_reconnect", self.rung_reconnect.to_string()),
            ("rung_pause_all", self.rung_pause_all.to_string()),
            ("rung_swap", self.rung_swap.to_string()),
            ("engine_restarts", self.engine_restarts.to_string()),
            ("task_stalls", self.task_stalls.to_string()),
            ("stall_total_ms", self.stall_total.as_millis().to_string()),
            ("multiip_tasks", self.multiip_tasks.to_string()),
            ("multiip_backup_uris", self.multiip_backup_uris.to_string()),
            ("leg_trim_del", self.leg_trim_del.to_string()),
            ("leg_add", self.leg_add.to_string()),
        ]
    }

    /// 写一条 `engine_counters`。`why` 说明这次快照是**什么触发的**
    /// （`periodic` / `rung_reconnect` / `rung_pause_all` / `rung_swap`），
    /// 好让读日志的人分得清"定时那一份"与"动作那一份"。
    /// 快照是逐字段独立读的：并发下 `rpc_calls` 与四桶之和可能瞬时不等，**读的人不能假设二者相等**。
    pub fn log(&self, why: &str) {
        self.log_with(why, &crate::diagnostics::log);
    }

    /// [`Self::log`] 的本体：出口由调用方给。
    ///
    /// ⚠️ 为什么要有这条缝：`crate::diagnostics::log` 直接落盘、没有可注入的出口，
    /// 不开口子的话"事件名对不对"与"`why` 在第几位"没有任何用例看得见 ——
    /// 而读日志的人正是靠它们找入口的。缝开在这里，生产路径一个字不变。
    fn log_with(&self, why: &str, sink: &dyn Fn(&str, &[(&str, String)])) {
        let mut fields = vec![("why", why.to_string())];
        fields.extend(self.fields());
        sink("engine_counters", &fields);
    }
}

#[derive(Debug, Default)]
pub struct Counters {
    rpc_calls: AtomicU64,
    rpc_failures: AtomicU64,
    rpc_buckets: [AtomicU64; 4],
    rpc_worst_ms: AtomicU64,
    rung_reconnect: AtomicU64,
    rung_pause_all: AtomicU64,
    rung_swap: AtomicU64,
    engine_restarts: AtomicU64,
    task_stalls: AtomicU64,
    stall_total_ms: AtomicU64,
    multiip_tasks: AtomicU64,
    multiip_backup_uris: AtomicU64,
    leg_trim_del: AtomicU64,
    leg_add: AtomicU64,
}

impl Counters {
    /// 记一次往返。`took` 是**真实耗时**（不是超时上限）。
    pub fn record_call(&self, took: Duration, ok: bool) {
        self.rpc_calls.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.rpc_failures.fetch_add(1, Ordering::Relaxed);
        }
        self.rpc_buckets[bucket_of(took)].fetch_add(1, Ordering::Relaxed);
        // 只许变大：用 fetch_max 而不是 load+store（后者在并发下会丢最大值）。
        self.rpc_worst_ms
            .fetch_max(took.as_millis() as u64, Ordering::Relaxed);
    }

    /// 记一次阶梯动作。**档 0 不记**（它是"什么都没发生"，记了只会稀释分布）。
    pub fn record_rung(&self, rung: Rung) {
        // ⚠️ 每个非空臂都要**用花括号包住并把 `fetch_add` 的结果丢掉** ——
        //    `fetch_add` 返回 `u64`，而 `Rung::Observe` 那一臂是 `()`；
        //    写成表达式臂会让各臂类型不一致，**编不过**。
        match rung {
            Rung::Observe => {}
            Rung::ReconnectOne => {
                self.rung_reconnect.fetch_add(1, Ordering::Relaxed);
            }
            Rung::PauseAllThenResumeAll => {
                self.rung_pause_all.fetch_add(1, Ordering::Relaxed);
            }
            Rung::ReplaceEngine => {
                self.rung_swap.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn record_engine_restart(&self) {
        self.engine_restarts.fetch_add(1, Ordering::Relaxed);
    }

    /// 记一次任务停滞：次数 +1、时长累加（"分布"由逐次 `stall` 事件给，这里只给总量）。
    pub fn record_stall(&self, how_long: Duration) {
        self.task_stalls.fetch_add(1, Ordering::Relaxed);
        self.stall_total_ms
            .fetch_add(how_long.as_millis() as u64, Ordering::Relaxed);
    }

    /// 记一次**多 IP 构造**（规格 §6.1 的诚实替代）：任务数按 `backup_uris > 0` 的次数
    /// 累加，备胎 URI 总数按量累加。
    ///
    /// ⚠️ **诚实边界（§6.1 / §6.4 实测）**：规格想记「每 URI 的成功 / 失败 / 被切换次数」，
    /// 但 aria2 的 `tellStatus` 全量 15 键**没有 URI 归属** —— RPC 取不到这三样。
    /// 所以这里**只记构造证据**（提交时带了几个备胎、多少个任务带备胎）；
    /// 「被切换次数」只能读 aria2 自己的日志，已登记为 README 的未验项。
    /// **不造假数据**：没有的量就不写进计数器。
    ///
    /// `backup_uris` 是**备胎**条数（域名 URI 不算）。只数数字，不记 URL / 路径 / 随机码
    /// （诊断脱敏，与本模块其它计数同一纪律）。
    pub fn record_multiip(&self, backup_uris: usize) {
        if backup_uris > 0 {
            self.multiip_tasks.fetch_add(1, Ordering::Relaxed);
        }
        self.multiip_backup_uris
            .fetch_add(backup_uris as u64, Ordering::Relaxed);
    }

    /// 记一次**换腿动作**（摘慢腿、补新 IP 腿）。只在 `changeUri` **成功**后记——
    /// 失败的那一拍不记，免得日志里出现"摘了 N 条"却实际一条没摘。
    ///
    /// 只数条数，不记 URI / 域名 / IP（诊断脱敏，与本模块其它计数同一纪律）。
    pub fn record_leg_trim(&self, del: usize, add: usize) {
        self.leg_trim_del
            .fetch_add(del as u64, Ordering::Relaxed);
        self.leg_add.fetch_add(add as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            rpc_calls: self.rpc_calls.load(Ordering::Relaxed),
            rpc_failures: self.rpc_failures.load(Ordering::Relaxed),
            rpc_buckets: [
                self.rpc_buckets[0].load(Ordering::Relaxed),
                self.rpc_buckets[1].load(Ordering::Relaxed),
                self.rpc_buckets[2].load(Ordering::Relaxed),
                self.rpc_buckets[3].load(Ordering::Relaxed),
            ],
            rpc_worst: Duration::from_millis(self.rpc_worst_ms.load(Ordering::Relaxed)),
            rung_reconnect: self.rung_reconnect.load(Ordering::Relaxed),
            rung_pause_all: self.rung_pause_all.load(Ordering::Relaxed),
            rung_swap: self.rung_swap.load(Ordering::Relaxed),
            engine_restarts: self.engine_restarts.load(Ordering::Relaxed),
            task_stalls: self.task_stalls.load(Ordering::Relaxed),
            stall_total: Duration::from_millis(self.stall_total_ms.load(Ordering::Relaxed)),
            multiip_tasks: self.multiip_tasks.load(Ordering::Relaxed),
            multiip_backup_uris: self.multiip_backup_uris.load(Ordering::Relaxed),
            leg_trim_del: self.leg_trim_del.load(Ordering::Relaxed),
            leg_add: self.leg_add.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::supervisor::Rung;
    use std::time::Duration;

    /// 分桶的边界是**左闭右开**，且四个桶覆盖全区间（不许有落不进去的耗时）。
    #[test]
    fn latency_buckets_have_spec_boundaries() {
        assert_eq!(bucket_of(Duration::from_millis(999)), 0);
        assert_eq!(bucket_of(Duration::from_secs(1)), 1);
        assert_eq!(bucket_of(Duration::from_secs(5)), 2);
        assert_eq!(bucket_of(Duration::from_secs(10)), 3);
        assert_eq!(bucket_of(Duration::from_secs(120)), 3);
    }

    #[test]
    fn calls_and_failures_are_counted_separately() {
        let c = Counters::default();
        c.record_call(Duration::from_millis(20), true);
        c.record_call(Duration::from_secs(11), false);
        let s = c.snapshot();
        assert_eq!(s.rpc_calls, 2);
        assert_eq!(s.rpc_failures, 1);
        assert_eq!(s.rpc_buckets, [1, 0, 0, 1]);
        assert_eq!(s.rpc_worst, Duration::from_secs(11));
    }

    /// 判别力：把 `worst` 的更新条件写成 `>=` 也行，但**删掉**它 ⇒ 红。
    #[test]
    fn the_worst_call_only_grows() {
        let c = Counters::default();
        c.record_call(Duration::from_secs(9), true);
        c.record_call(Duration::from_secs(2), true);
        assert_eq!(c.snapshot().rpc_worst, Duration::from_secs(9));
    }

    #[test]
    fn rung_triggers_are_counted_per_rung() {
        let c = Counters::default();

        // ⚠️ 档 0（Observe）是 no-op：**整份快照必须一字不变**。
        //    只断言"某个字段是 0"不够 —— 那证明不了"没有别的东西被动过"。
        //    判别力：把 `Rung::Observe => {}` 改成记到随便哪一格 ⇒ 红。
        let before = c.snapshot();
        c.record_rung(Rung::Observe);
        assert_eq!(c.snapshot(), before, "档 0 不许动任何一格");

        // ⚠️ **每一档都要真的调一次**。"没被增加过"证明不了"它映到这一格"。
        //    判别力：把 `Rung::PauseAllThenResumeAll` 的臂改成记到 `rung_swap` ⇒ 红。
        c.record_rung(Rung::ReconnectOne);
        c.record_rung(Rung::ReconnectOne);
        c.record_rung(Rung::PauseAllThenResumeAll);
        c.record_rung(Rung::ReplaceEngine);

        let s = c.snapshot();
        assert_eq!(s.rung_reconnect, 2);
        assert_eq!(s.rung_pause_all, 1);
        assert_eq!(s.rung_swap, 1);
    }

    #[test]
    fn a_snapshot_renders_every_field_the_spec_asks_for() {
        let c = Counters::default();
        c.record_call(Duration::from_secs(6), true);
        c.record_rung(Rung::PauseAllThenResumeAll);
        c.record_stall(Duration::from_secs(200));
        c.record_multiip(2);
        c.record_leg_trim(3, 2);
        // ⚠️ 逐项比对整份 (键, 值) —— 不是只比键序，也不是只抽查一个值。
        //    判别力：`rung_pause_all` 误读成 `self.rung_swap`、`rpc_worst_ms` 写成
        //    `as_secs()`（6 而不是 6000）、`stall_total_ms` 写成 `as_secs()`、
        //    `rpc_failures` 误读成 `self.rpc_calls`、`multiip_backup_uris` 误读成
        //    `multiip_tasks` ⇒ 五种错各自都会红。
        //    新键只许**往后加**（键序是契约，见 `fields` 的 doc）。
        assert_eq!(
            c.snapshot().fields(),
            vec![
                ("rpc_calls", "1".to_string()),
                ("rpc_failures", "0".to_string()),
                ("rpc_lt1s", "0".to_string()),
                ("rpc_1_5s", "0".to_string()),
                ("rpc_5_10s", "1".to_string()),
                ("rpc_gt10s", "0".to_string()),
                ("rpc_worst_ms", "6000".to_string()),
                ("rung_reconnect", "0".to_string()),
                ("rung_pause_all", "1".to_string()),
                ("rung_swap", "0".to_string()),
                ("engine_restarts", "0".to_string()),
                ("task_stalls", "1".to_string()),
                ("stall_total_ms", "200000".to_string()),
                ("multiip_tasks", "1".to_string()),
                ("multiip_backup_uris", "2".to_string()),
                ("leg_trim_del", "3".to_string()),
                ("leg_add", "2".to_string()),
            ]
        );
    }

    /// 多 IP 构造计数（规格 §6.1 的诚实替代，见 `record_multiip` 的边界注释）：
    /// 「带备胎的任务数」按 `backup_uris > 0` 的次数累加，「备胎 URI 总数」按量累加。
    /// 判别力：把任务数的门槛写成「无条件 +1」⇒ `record_multiip(0)` 那一笔会把它推到 3 ⇒ 红；
    /// 把总数读成任务数（或漏加）⇒ 5 对不上 ⇒ 红。
    #[test]
    fn record_multiip_counts_tasks_with_backups_and_backup_uri_totals() {
        let c = Counters::default();
        // 三次构造：2 个备胎、0 个备胎（域名-only，**不**计入任务数）、3 个备胎。
        c.record_multiip(2);
        c.record_multiip(0);
        c.record_multiip(3);
        let s = c.snapshot();
        assert_eq!(s.multiip_tasks, 2, "带备胎的任务数 = backup_uris>0 的次数");
        assert_eq!(s.multiip_backup_uris, 5, "备胎 URI 总数 = 2+0+3");
    }

    /// 换腿计数按条数累加（摘与补各一根轴）。
    /// 判别力：把 `del` 与 `add` 的参数位置写反 ⇒ 两值对调 ⇒ 红。
    #[test]
    fn record_leg_trim_accumulates_del_and_add_totals() {
        let c = Counters::default();
        c.record_leg_trim(1, 1);
        c.record_leg_trim(2, 0);
        let s = c.snapshot();
        assert_eq!(s.leg_trim_del, 3, "摘掉的腿总数 = 1+2");
        assert_eq!(s.leg_add, 1, "补上的腿总数 = 1+0");
    }

    #[test]
    fn logging_a_snapshot_reports_the_event_name_and_why_first() {
        let c = Counters::default();
        c.record_call(Duration::from_secs(6), true);

        let seen: std::cell::RefCell<Vec<(String, Vec<(String, String)>)>> =
            std::cell::RefCell::new(Vec::new());
        c.snapshot().log_with("periodic", &|event, fields| {
            seen.borrow_mut().push((
                event.to_string(),
                fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            ));
        });

        let got = seen.into_inner();
        assert_eq!(got.len(), 1, "只该写一条");
        assert_eq!(got[0].0, "engine_counters", "事件名是读日志的人的入口，不许改");
        assert_eq!(
            got[0].1[0],
            ("why".to_string(), "periodic".to_string()),
            "`why` 必须是**第一项**"
        );
        assert_eq!(got[0].1.len(), 18, "`why` + 17 个字段");
        let expect: Vec<(String, String)> = c
            .snapshot()
            .fields()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(&got[0].1[1..], &expect[..]);
    }
}
