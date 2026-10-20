//! 引擎管家 —— 判「引擎还活着吗」与「下载还在走吗」。**纯逻辑**：不碰 I/O，
//! 时间一律由调用方传入（否则测试要真等 180 秒，这套阈值就没法测）。
//!
//! 两个信号**互相独立**，这是本规格的核心（规格 §3.1）：
//!   · 信号 A｜引擎应答性 —— 每次 RPC 的成败与真实耗时；
//!   · 信号 B｜任务进展 —— 全局速率是不是一直在 0（**速率型**，理由见计划「开工前必读」第 7 条）。
//! 今天内核把它们混成了一个 `RPC_TIMEOUT`：那测的是"RPC 快不快"，不是"下载有没有在走"。
//! 现场证据（规格 F4）显示**出口网络自己就在劣化**，那时 RPC 慢而下载也可能没停 ——
//! 混在一起就必然误杀。

use std::time::{Duration, Instant};

/// 任务进展信号（信号 B）。**速率型**，不是字节型 —— 理由见本计划「开工前必读」第 7 条：
/// `getGlobalStat` 根本没有字节总量，只有 `downloadSpeed` 与 `numActive`。
///
/// 口径：**速率持续为 0、且有活跃任务**，才算停滞。
#[derive(Debug, Default, Clone)]
pub struct Progress {
    /// 速率一开始为 0 的那一刻；一有速率就清掉。
    idle_since: Option<Instant>,
}

impl Progress {
    /// 采一次。两个入参都来自 `getGlobalStat`：
    /// `speed` = `downloadSpeed`（字符串，调用方解析成 `u64`），`active` = `numActive`。
    ///
    /// ⚠️ **`active == 0` 一律不算停滞**：客户点「暂停全部」之后速率当然是 0，
    ///    那是**客户要的**，不是引擎卡了。不排除它，阶梯会在客户主动暂停时反复触发。
    /// ⚠️ 只用一个 `getGlobalStat`（不碰 `tellActive`）——后者正是现场卡住的那一条（F2/F3）。
    pub fn observe(&mut self, now: Instant, speed: u64, active: u64) {
        if speed > 0 || active == 0 {
            self.idle_since = None;
        } else if self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }

    /// 速率已经为 0 多久。**没在停滞**时返回 0（"未知"不许被当成"停滞"）。
    pub fn stalled_for(&self, now: Instant) -> Duration {
        match self.idle_since {
            Some(t) => now.saturating_duration_since(t),
            None => Duration::ZERO,
        }
    }
}

/// 每个任务的"上次移动时刻"。**按需**更新（见 `spawn_recovery`），不是每拍。
#[derive(Debug, Default, Clone)]
pub struct TaskProgress {
    /// gid -> (上次见到的 `completedLength`, 那个值是在哪一刻**第一次**见到的)
    seen: std::collections::BTreeMap<String, (u64, Instant)>,
    /// gid -> 上一次**对它动过手**的时刻。只影响"选谁"（轮换），不影响"停滞了几个"。
    attempted: std::collections::BTreeMap<String, Instant>,
}

impl TaskProgress {
    /// 用一次 `Daemon::snapshot()` 的结果更新。入参是**应当正在动的那些任务**的
    /// `(gid, completedLength)` 列表 —— 调用方已经按 `TaskState::Active` 滤过
    /// （`list()` 是 `active ++ waiting ++ stopped` 三段拼接，不滤的话"传完的任务"
    /// 会以不动的字节数**永远**算作停滞）。
    ///
    /// 返回 `()`，不是"停滞最久的 gid"：判"停滞"要用 `Thresholds`，阈值不该由这里定死。
    /// 要那个答案请调 [`Self::most_stalled`]。
    pub fn observe(&mut self, now: Instant, tasks: &[(String, u64)]) {
        for (gid, completed) in tasks {
            match self.seen.get_mut(gid) {
                Some((last, since)) => {
                    if *completed > *last {
                        *since = now;
                    }
                    *last = *completed;
                }
                // 第一次见到它 ⇒ 从"现在"起算，不能填更早的时刻，否则换引擎后新生的任务会一上来就算作停滞。
                None => {
                    self.seen.insert(gid.clone(), (*completed, now));
                }
            }
        }
        // 这一拍没看见的 gid 一律丢掉：不丢的话，旧 gid 会以旧时刻永远留在表里，新引擎刚起来就被判成停滞。
        let present: std::collections::BTreeSet<&str> =
            tasks.iter().map(|(gid, _)| gid.as_str()).collect();
        self.seen.retain(|gid, _| present.contains(gid.as_str()));
        self.attempted.retain(|gid, _| present.contains(gid.as_str()));
    }

    /// 停滞**达到** `t.stall` 的任务数（喂给 `Ladder::next` 的 `stalled_tasks`）。
    pub fn stalled_count(&self, now: Instant, t: &Thresholds) -> usize {
        self.seen
            .values()
            .filter(|(_, since)| now.saturating_duration_since(*since) >= t.stall)
            .count()
    }

    /// 停滞最久的那个 gid（档 1 用）的**选择**：在停滞的任务里，取最久没被照顾过的那个。
    /// 从没动过手的排最前（`None < Some`）；其次按"动手时刻最早"；再并列按"停滞最久"。结果确定。
    pub fn most_stalled(&self, now: Instant, t: &Thresholds) -> Option<String> {
        self.seen
            .iter()
            .filter(|(_, (_, since))| now.saturating_duration_since(*since) >= t.stall)
            .min_by_key(|(gid, (_, since))| (self.attempted.get(*gid).copied(), *since))
            .map(|(gid, _)| gid.clone())
    }

    /// 停滞达到 `t.stall` 的那些 gid（档 2 的动作名单）。
    ///
    /// ⚠️ 只取卡住的，不取全部已见的：档 2 是来救卡住的任务的，健康任务被停一下毫无理由。
    pub fn stalled_gids(&self, now: Instant, t: &Thresholds) -> Vec<String> {
        self.seen
            .iter()
            .filter(|(_, (_, since))| now.saturating_duration_since(*since) >= t.stall)
            .map(|(gid, _)| gid.clone())
            .collect()
    }

    /// 停滞达到 `t.stall` 的那些 `(gid, 停滞时长)`。给计数器的 `record_stall` 喂数据用。
    pub fn stalled_with_durations(&self, now: Instant, t: &Thresholds) -> Vec<(String, Duration)> {
        self.seen
            .iter()
            .filter_map(|(gid, (_, since))| {
                let how_long = now.saturating_duration_since(*since);
                (how_long >= t.stall).then(|| (gid.clone(), how_long))
            })
            .collect()
    }

    /// 记一次"已经对它动过手"：只改"选谁"的顺序，**不碰 `seen`**。
    ///
    /// ⚠️ 碰了 `seen` 就等于改了"它停了多久"，于是 `stalled_count` 会少算一个，
    ///    档 2 的闸门（`stalled_tasks >= 2`）会被档 1 的动作遮蔽掉。
    /// ⚠️ 未见过的 gid 也照记（只进 `attempted`，不产生停滞记录），`observe` 会在它消失时清掉。
    pub fn mark_attempted(&mut self, gid: &str, now: Instant) {
        self.attempted.insert(gid.to_string(), now);
    }
}

/// 引擎应答性信号（信号 A）。
///
/// ⚠️ **不是**「单次 RPC 超时就判死」。今天那条路（撞满 `RPC_TIMEOUT` 就换引擎）正是要换掉的：
/// 现场的证据（规格 F1）是**读阶段超时、连接阶段从没被拒** ⇒ 连接建得起来、只是对方不回，
/// 那时把引擎整个换掉既修不好网络，又毁掉一个无辜组件的队列。
#[derive(Debug, Default, Clone)]
pub struct Liveness {
    consecutive_failures: u32,
    /// **最近一次**往返的耗时。
    ///
    /// ⚠️ **不是"历史最大值"** —— 若记最大值，则**一次** 31 秒的调用会把引擎**永久**
    /// 钉成"不应答"（`Liveness` 在自愈线程里活得很久，重建不了）。
    /// 历史最大值在**证据侧**（`diag_counters::Counters` 的 `rpc_worst_ms` 字段，用 `fetch_max`）。两者职责不同，
    /// 别合并：这里回答"**此刻**它应答吗"，那里回答"**曾经**最慢多慢"。
    last_took: Option<Duration>,
}

impl Liveness {
    pub fn record_ok(&mut self, took: Duration) {
        self.consecutive_failures = 0;
        self.last_took = Some(took);
    }

    pub fn record_fail(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// 两条判据取"或"：连续失败到上限，**或**最近一次慢到上限（含等于）。
    pub fn is_unresponsive(&self, t: &Thresholds) -> bool {
        self.consecutive_failures >= t.rpc_failures
            || self.last_took.is_some_and(|d| d >= t.single_slow)
    }
}

/// 全部阈值。**改这里是一处改动**（规格 §3.5：全部可配、全部进诊断日志）。
///
/// ⚠️ **定义在本任务**（不是任务 3）：`Liveness::is_unresponsive` 的签名要用它，
///    而任务 2 先落地 —— 放在任务 3 会让任务 2 编译不过。
///
/// 初值一律**保守**：规格 R2 说得很清楚，定得太激进会误杀健康的慢连接，
/// 而误杀的代价是一条几 GB 的段被丢弃重来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// 速率持续为 0 多久算停滞。**初值 180 秒**（规格 §3.3：任务级偏保守）。
    pub stall: Duration,
    /// 连续多少次 RPC 失败算"不应答"。
    pub rpc_failures: u32,
    /// 单次往返达到多久算"不应答"（判据是 `>=`，恰好等于也算）。
    /// 初值 15 秒（≈ 1.5 × RPC_TIMEOUT）：规格原文是 30 秒，但 `RpcClient::call` 最坏约 20 秒，
    /// 30 秒在生产上不可达。
    pub single_slow: Duration,
    /// 做完档 2 之后还要再等多久才允许换引擎。
    pub pre_swap_wait: Duration,
    /// 档 1 之后的冷却期。
    pub cooldown: Duration,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            stall: Duration::from_secs(180),
            rpc_failures: 3,
            single_slow: Duration::from_secs(15),
            pre_swap_wait: Duration::from_secs(120),
            cooldown: Duration::from_secs(60),
        }
    }
}

/// 恢复阶梯的一档。**顺序即代价**：越往下越贵（规格 §3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// 档 0：继续观察。
    Observe,
    /// 档 1：单任务重连（丢该任务在途连接，从 `.aria2` 偏移续）。
    ReconnectOne,
    /// 档 2：暂停全部 → 一键开始（丢全部在途连接，**队列不丢**）。
    PauseAllThenResumeAll,
    /// 档 3：换引擎（**最后手段**）。
    ReplaceEngine,
}

/// 阶梯的状态机。持有"上一次做了什么、什么时候"。
#[derive(Debug, Default, Clone)]
pub struct Ladder {
    thresholds: Thresholds,
    last_reconnect: Option<Instant>,
    last_pause_all: Option<Instant>,
    last_swap: Option<Instant>,
    /// 引擎从此刻起持续健康；`None` 表示此刻不健康。只有持续健康够久才结束本轮故障。
    healthy_since: Option<Instant>,
}

impl Ladder {
    pub fn new(thresholds: Thresholds) -> Self {
        Self { thresholds, ..Self::default() }
    }

    pub fn thresholds(&self) -> Thresholds {
        self.thresholds
    }

    /// 已经连续健康了多久；`None` = 此刻不健康。
    ///
    /// ⚠️ 读的是 `next()` 里那一个 `healthy_since`，不另起计时器：「什么叫健康」只许有一个定义。
    pub fn healthy_for(&self, now: Instant) -> Option<Duration> {
        self.healthy_since.map(|since| now.saturating_duration_since(since))
    }

    /// 这一拍该做什么。**纯函数**：同样的输入给同样的输出（除了它自己记的时间戳）。
    ///
    /// `stalled_tasks` 由调用方按**每个任务**算好（见 [`TaskProgress::stalled_count`]）。
    ///
    /// `engine_moving`：全局还没有停滞满 `stall`，即"下载还在走"。它**只**用在"要不要暂停全部"那一支：
    /// 档 1（某一个任务停滞、别的照跑）只看 `stalled_tasks`；档 2（多任务同时停滞，等于全局都停了）
    /// 必须要求它为假。套到档 1 上，那个任务永远认不出来；从档 2 上删掉，100 个在跑的任务里卡 2 个
    /// 就会每 120 秒暂停一次全部。
    ///
    /// 五段的顺序是承重的，别重排 —— 每一段都在挡一种"每拍重复触发"：
    /// ① 刚换过引擎 ⇒ 先给它时间；② 不该动就别动；③ 档 2 做过之后只有两条出路
    /// （等满 → 换引擎，或应答恢复 → 重开一轮）；④ 档 1 的冷却；⑤ 才轮到决定。
    /// `engine_busy` = 引擎进程**自己**在动（写盘 / 烧 CPU，见 [`crate::procstat::is_busy`]）。
    ///
    /// ⚠️ 它是**唯一不经过 RPC 的活口证据**：RPC 探活失败时，`a.is_unresponsive()` 与
    ///    `engine_moving` 都只能看见"不回话"，分不出"忙得回不上话"与"真死了"。现场日志里
    ///    就是分不出这一点的代价 —— 引擎一边每秒写几 MB 到几十 MB、一边被判死，档 2 暂停全部、
    ///    档 3 关掉一个正在写盘的引擎并重新入队 234 个任务，75 分钟里换了 3 次直到放弃。
    pub fn next(
        &mut self,
        now: Instant,
        a: &Liveness,
        stalled_tasks: usize,
        engine_moving: bool,
        engine_busy: bool,
    ) -> Rung {
        let t = self.thresholds;

        // ① 刚换过引擎：等满 `pre_swap_wait` 再重新开始判断。
        //    ⚠️ 这一段是"不许无限换引擎"的实现：换完之后 `last_pause_all` 与 `last_swap`
        //       一起清掉，于是下一次要换必须**重新走一遍**档 2。
        if let Some(swapped) = self.last_swap {
            if now.saturating_duration_since(swapped) < t.pre_swap_wait {
                return Rung::Observe;
            }
            self.last_swap = None;
            self.last_pause_all = None;
        }

        // ⓪ 持续健康够久 ⇒ 清掉档 1/2 记录。健康期内不会再产生这两类动作（它们只在不健康的拍上发生，
        //    而那一拍会把 `healthy_since` 清掉），所以健康期内反复清是幂等的。
        //    `healthy_since` 本身**不在这里清**：它是本次健康期的起点，`healthy_for` 要读它。
        if let Some(since) = self.healthy_since {
            if now.saturating_duration_since(since) >= t.pre_swap_wait {
                self.last_pause_all = None;
                self.last_reconnect = None;
            }
        }

        let unresponsive = a.is_unresponsive(&t);
        // ② 引擎应答正常、也没有任务停滞 ⇒ 观察，并开始或继续计健康时长（不一拍就清，免得抖动引擎反复重下档 2）。
        if !unresponsive && stalled_tasks == 0 {
            self.healthy_since.get_or_insert(now);
            return Rung::Observe;
        }
        self.healthy_since = None;

        // ②½ **探活不通，但进程还在动 ⇒ 它在干活，只是回不上话。**
        //    这一支必须挡住档 2 与档 3（不只档 3）：暂停全部与换引擎都是**对引擎动手**，
        //    而"回不上话"在忙的时候是**正常现象**（写盘把协议循环堵住），不是故障证据。
        //    ⚠️ 只挡"不应答"这一支：`stalled_tasks >= 2 && !engine_moving` 那条说的是
        //       "应答正常但没人前进"（限速成涓流），那时的 `engine_busy` 恒为真（一直在写盘），
        //       拿它一并挡掉就等于把档 1 彻底废掉。
        if unresponsive && engine_busy {
            return Rung::Observe;
        }

        // ③ 档 2 已经做过：只有两种出路。
        if let Some(paused_at) = self.last_pause_all {
            if now.saturating_duration_since(paused_at) < t.pre_swap_wait {
                // 等待期内既不重复档 2、也不越过它去换引擎。
                return Rung::Observe;
            }
            if unresponsive {
                self.last_swap = Some(now);
                return Rung::ReplaceEngine;
            }
            // 应答回来了但任务仍停滞 ⇒ 清掉记录，本轮重新给一次档 1/2 的机会。
            self.last_pause_all = None;
        }

        // ④ 档 1 的冷却期（理由见规格 §3.2"冷却"）。
        if self
            .last_reconnect
            .is_some_and(|at| now.saturating_duration_since(at) < t.cooldown)
        {
            return Rung::Observe;
        }

        // ⑤ 决定这一拍做什么。
        //    引擎不应答时**不做档 1** —— 单任务重连本身也要走 RPC，引擎不回话时它必然失败。
        if unresponsive || (stalled_tasks >= 2 && !engine_moving) {
            self.last_pause_all = Some(now);
            return Rung::PauseAllThenResumeAll;
        }
        self.last_reconnect = Some(now);
        Rung::ReconnectOne
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_first_zero_speed_observation_starts_the_clock() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 3);
        assert_eq!(p.stalled_for(t0), Duration::ZERO, "刚归零的那一拍还不算停滞");
    }

    #[test]
    fn a_positive_speed_clears_the_clock() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 3);
        p.observe(t0 + Duration::from_secs(100), 4_096, 3);
        assert_eq!(p.stalled_for(t0 + Duration::from_secs(100)), Duration::ZERO);
    }

    #[test]
    fn a_sustained_zero_speed_accumulates_stall_time() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 3);
        p.observe(t0 + Duration::from_secs(120), 0, 3);
        assert_eq!(p.stalled_for(t0 + Duration::from_secs(120)), Duration::from_secs(120));
    }

    /// ⚠️ **最容易犯的那条**：客户点了「暂停全部」，速率必然是 0，但那是**客户要的**，
    /// 不是引擎卡了。判别力：删掉 `active == 0` 那一支 ⇒ 红。
    #[test]
    fn paused_tasks_are_not_a_stall() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 0);
        p.observe(t0 + Duration::from_secs(600), 0, 0);
        assert_eq!(p.stalled_for(t0 + Duration::from_secs(600)), Duration::ZERO);
    }

    /// 暂停期间攒下的"停滞"不能在恢复后的第一拍冒出来：活跃数一回来就从头算。
    #[test]
    fn resuming_after_a_pause_starts_from_scratch() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 0);
        p.observe(t0 + Duration::from_secs(300), 0, 0);
        p.observe(t0 + Duration::from_secs(301), 0, 2); // 恢复了，但速率还没上来
        assert_eq!(p.stalled_for(t0 + Duration::from_secs(301)), Duration::ZERO);
    }

    /// 暂停前计时器已经起表（卡速 100 秒），暂停后恢复：暂停必须清零，而不是冻结这段计时。
    /// 判别力：把暂停分支改成「只冻结、不清零」⇒ 此条红。
    #[test]
    fn a_pause_clears_a_stall_clock_started_before_it() {
        let t0 = Instant::now();
        let mut p = Progress::default();
        p.observe(t0, 0, 3);
        p.observe(t0 + Duration::from_secs(100), 0, 0);
        p.observe(t0 + Duration::from_secs(301), 0, 2);
        assert_eq!(p.stalled_for(t0 + Duration::from_secs(301)), Duration::ZERO);
    }

    #[test]
    fn successes_clear_the_failure_streak() {
        let mut l = Liveness::default();
        l.record_fail();
        l.record_fail();
        l.record_ok(Duration::from_millis(5));
        assert_eq!(l.consecutive_failures(), 0);
    }

    #[test]
    fn three_consecutive_failures_are_unresponsive() {
        let mut l = Liveness::default();
        let t = Thresholds::default();
        l.record_fail();
        l.record_fail();
        assert!(!l.is_unresponsive(&t), "两次还不够");
        l.record_fail();
        assert!(l.is_unresponsive(&t), "三次连续失败即为不应答");
    }

    #[test]
    fn one_very_slow_call_is_unresponsive_even_without_failures() {
        let mut l = Liveness::default();
        let t = Thresholds::default();
        l.record_ok(Duration::from_secs(31));
        assert!(l.is_unresponsive(&t), "一次超过 15 秒也算不应答");
    }

    /// ⚠️ **最近一次**决定此刻的判据，不是历史最大值。
    ///
    /// 判别力：把 `last_took` 改回"取最大值"（`Some(last.map_or(took, |p| p.max(took)))`）⇒
    /// 第二条断言红 —— 而那正是"一次慢调用把引擎永久钉成不应答"的形态。
    #[test]
    fn a_fast_call_after_a_slow_one_clears_the_alarm() {
        let mut l = Liveness::default();
        let t = Thresholds::default();
        l.record_ok(Duration::from_secs(31));
        assert!(l.is_unresponsive(&t), "慢调用当时算不应答");
        l.record_ok(Duration::from_millis(20));
        assert!(!l.is_unresponsive(&t), "下一次快调用要把它清掉 —— 它答得很快");
    }

    /// 15 秒不是规格原文的 30 秒，是订正值：`RpcClient::call` 最坏约 20 秒（两次 `send`，
    /// 每次 `RPC_TIMEOUT` = 10 秒），30 秒在生产上按构造就到不了，那半条判据会永远为假。
    /// 下一个人若想把它改回规格原文的 30 秒，先读这里。
    #[test]
    fn the_default_thresholds_are_the_shipped_values() {
        let t = Thresholds::default();
        assert_eq!(t.stall, Duration::from_secs(180));
        assert_eq!(t.rpc_failures, 3);
        assert_eq!(t.single_slow, Duration::from_secs(15));
        assert_eq!(t.pre_swap_wait, Duration::from_secs(120));
    }

    /// 健康拍与不健康拍交替时，健康期不许跨过不健康拍被攒起来：任何一拍 `healthy_for` 都到不了复位窗口。
    ///
    /// 注意：不健康拍用的是**同一个静态的三次失败**，不是真实的应答/失败序列。
    /// 咬住的突变见 ② 段的 `self.healthy_since = None;`（删掉它 ⇒ 红）。
    /// `a_flapping_engine_does_not_reissue_pause_all` 也咬这一行：「一拍健康不足以结束本轮故障」这条保证正依赖它。
    /// "每个健康拍把起点重置为 now"这一类突变它是绿的，由 `a_continuous_healthy_run_reaches_the_reset_window` 咬住。
    #[test]
    fn an_alternating_engine_never_builds_a_healthy_run() {
        let t = Thresholds::default();
        let mut l = Ladder::default();
        let now = Instant::now();
        let ok = Liveness::default();
        let mut bad = Liveness::default();
        bad.record_fail(); bad.record_fail(); bad.record_fail();
        for s in 0..300u64 {
            let at = now + Duration::from_secs(s);
            let a = if s % 2 == 0 { &ok } else { &bad };
            l.next(at, a, 0, true, false);
            assert!(
                l.healthy_for(at).map_or(true, |h| h < t.pre_swap_wait),
                "第 {s} 拍不许攒出一个完整的复位窗口"
            );
        }
    }

    /// 持续健康满复位窗口之后，`healthy_for` 必须读得到这个长度。
    /// 判别力：每个健康拍都把起点重置为 now，或 ⓪ 清掉起点 ⇒ 红。
    #[test]
    fn a_continuous_healthy_run_reaches_the_reset_window() {
        let t = Thresholds::default();
        let mut l = Ladder::default();
        let now = Instant::now();
        for s in 0..=121u64 {
            let at = now + Duration::from_secs(s);
            l.next(at, &Liveness::default(), 0, true, false);
        }
        let end = now + Duration::from_secs(121);
        assert!(
            l.healthy_for(end).is_some_and(|h| h >= t.pre_swap_wait),
            "持续健康 121 秒之后必须读得到不小于 pre_swap_wait 的时长，实际 {:?}",
            l.healthy_for(end)
        );
    }

    // ---- TaskProgress：信号 B 只知道"有东西卡住了"，这三条补上"是哪一个" ----
    //
    // 本模块不重抄 `Ladder` / `Liveness` 自己的用例（它们已由各自的测试钉过），
    // 这里只钉 `TaskProgress` 新增的表面。

    /// 一个任务在两次观察之间 `completedLength` 没动过 ⇒ 它停滞；停滞最久的也是它。
    #[test]
    fn a_task_that_stops_moving_is_stalled() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("g1".to_string(), 100)]);
        let t1 = t0 + Duration::from_secs(181);
        tp.observe(t1, &[("g1".to_string(), 100)]);
        assert_eq!(tp.stalled_count(t1, &t), 1);
        assert_eq!(tp.most_stalled(t1, &t), Some("g1".to_string()));
    }

    /// 档 2 的名单只取卡住的：健康（仍在涨）的任务即使也在表里，也不许进名单。
    /// 判别力：把 `stalled_gids` 的过滤去掉 ⇒ `moving` 进名单 ⇒ 红。
    #[test]
    fn stalled_gids_lists_only_the_stuck_ones() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("stuck".to_string(), 100), ("moving".to_string(), 100)]);
        let t1 = t0 + Duration::from_secs(181);
        tp.observe(t1, &[("stuck".to_string(), 100), ("moving".to_string(), 900)]);
        assert_eq!(tp.stalled_gids(t1, &t), vec!["stuck".to_string()]);
    }

    /// 同一次观察里涨过的任务**不算**停滞（哪怕它是更早开始的那个）。
    /// 判别力：漏掉 `completed` 的比较（"见过就算停滞"）⇒ 红。
    #[test]
    fn a_task_that_moved_is_not_stalled() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("g1".to_string(), 100), ("g2".to_string(), 50)]);
        let t1 = t0 + Duration::from_secs(181);
        tp.observe(t1, &[("g1".to_string(), 100), ("g2".to_string(), 900)]);
        assert_eq!(tp.stalled_count(t1, &t), 1, "只有 g1 停滞");
        assert_eq!(tp.most_stalled(t1, &t), Some("g1".to_string()));
    }

    /// ⚠️ **换引擎之后 gid 全变了** —— 一批从未见过的新 gid **不许**一上来就被判停滞
    /// （那会变成"新引擎刚起来就被档 1 打"）。判别力：不把看不见的旧 gid 清掉 ⇒ 红
    /// （旧的 `since` 还停在 `t0`，会凑出 2 个停滞）。
    #[test]
    fn fresh_gids_after_an_engine_swap_are_not_stalled() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("old1".to_string(), 100), ("old2".to_string(), 100)]);
        let t1 = t0 + Duration::from_secs(181);
        tp.observe(t1, &[("new1".to_string(), 0), ("new2".to_string(), 0)]);
        assert_eq!(tp.stalled_count(t1, &t), 0, "新 gid 从 t1 起算，还没停滞");
        assert_eq!(tp.most_stalled(t1, &t), None);
    }

    /// 档 1 重连之后必须轮换：两个都卡住，动过停滞最久的那个之后，下一次轮到另一个。
    /// 判别力：去掉 `mark_attempted` ⇒ 第二次仍是 `a` ⇒ 红。
    #[test]
    fn a_reconnected_task_yields_the_turn_to_the_other_stuck_one() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("a".to_string(), 100)]);
        tp.observe(t0 + Duration::from_secs(1), &[("a".to_string(), 100), ("b".to_string(), 100)]);
        let t1 = t0 + Duration::from_secs(200);
        tp.observe(t1, &[("a".to_string(), 100), ("b".to_string(), 100)]);
        assert_eq!(tp.most_stalled(t1, &t), Some("a".to_string()), "a 停滞得最久，先轮到它");
        tp.mark_attempted("a", t1);
        let t2 = t1 + Duration::from_secs(60);
        tp.observe(t2, &[("a".to_string(), 100), ("b".to_string(), 100)]);
        assert_eq!(tp.most_stalled(t2, &t), Some("b".to_string()), "动过 a 之后轮到 b，不许 a 霸占");
    }

    /// 轮换只影响"选谁"，不影响"数几个"：动过一个之后，停滞的任务数仍是 2（档 2 的闸门看的就是这个数）。
    /// 判别力：把 `mark_attempted` 改回"碰 `seen`" ⇒ 数变成 1 ⇒ 红。
    #[test]
    fn rotation_does_not_reduce_the_stalled_count() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(t0, &[("a".to_string(), 100), ("b".to_string(), 100)]);
        let t1 = t0 + Duration::from_secs(200);
        tp.observe(t1, &[("a".to_string(), 100), ("b".to_string(), 100)]);
        assert_eq!(tp.stalled_count(t1, &t), 2);
        tp.mark_attempted("a", t1);
        assert_eq!(tp.stalled_count(t1, &t), 2, "动过 a 不许让它不再算停滞");
        assert_eq!(tp.most_stalled(t1, &t), Some("b".to_string()), "轮到没被照顾过的 b");
    }

    /// 没见过的 gid 不许被"记一次"凭空造出一条停滞记录，否则它会凭伪造的时刻被判停滞。
    /// 判别力：把 `mark_attempted` 改成未见即 `insert` 进 `seen` ⇒ 红。
    #[test]
    fn marking_an_unseen_gid_does_not_invent_a_record() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.mark_attempted("ghost", t0);
        let later = t0 + Duration::from_secs(181);
        assert_eq!(tp.stalled_count(later, &t), 0);
        assert_eq!(tp.most_stalled(later, &t), None);
    }

    #[test]
    fn healthy_engine_stays_at_rung_zero() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let a = Liveness::default();
        assert_eq!(l.next(now, &a, 0, true, false), Rung::Observe);
    }

    #[test]
    fn one_stalled_task_asks_for_a_single_reconnect() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 1, false, false), Rung::ReconnectOne);
    }

    #[test]
    fn several_stalled_tasks_ask_for_pause_all() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 4, false, false), Rung::PauseAllThenResumeAll);
    }

    /// 两个任务卡住而其余仍在动：这是档 1 的情形（认出并重连那一个），**不许**暂停全部。
    ///
    /// 注意断言是 `ReconnectOne` 而不是 `Observe`：`Observe` 会让"卡住的任务永远没人管"，
    /// 那正是档 1 要修的洞。
    /// 判别力：把 ⑤ 段的 `!engine_moving` 去掉（即回到 `stalled_tasks >= 2`）⇒ 红。
    #[test]
    fn two_stuck_tasks_while_the_engine_is_still_moving_do_not_trigger_pause_all() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 2, true, false), Rung::ReconnectOne);
    }

    /// 两个任务卡住且全局也停了：这才是档 2 的前提。
    #[test]
    fn two_stuck_tasks_and_a_stopped_engine_do_trigger_pause_all() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 2, false, false), Rung::PauseAllThenResumeAll);
    }

    #[test]
    fn an_unresponsive_engine_asks_for_pause_all_even_without_stall() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();
        assert_eq!(l.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
    }

    /// 档 1 之后必须有冷却：② 的爬坡会新增连接，新连接起步慢，
    /// 两个机制不加冷却会互相打架（规格 §3.2 的"冷却"）。
    #[test]
    fn a_reconnect_is_followed_by_a_cooldown() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 1, false, false), Rung::ReconnectOne);
        // 冷却期内再问，什么都不做
        assert_eq!(l.next(now + Duration::from_secs(1), &Liveness::default(), 1, false, false), Rung::Observe);
    }

    /// 档 2 之后**必须等满** `pre_swap_wait` 才允许换引擎（规格 §3.2 档 3 的代价最大）。
    #[test]
    fn replacing_the_engine_only_after_the_pause_all_has_had_its_wait() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();
        assert_eq!(l.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
        // 刚做完档 2：还没到等待上限 ⇒ 不许换
        assert_eq!(l.next(now + Duration::from_secs(60), &a, 0, true, false), Rung::Observe);
        // 等满了且仍然不应答 ⇒ 才轮到换引擎
        assert_eq!(l.next(now + Duration::from_secs(121), &a, 0, true, false), Rung::ReplaceEngine);
    }

    /// 探活不通、但引擎进程**还在写盘** ⇒ 一步都不许动。
    ///
    /// 判别力：删掉 ②½ 那一支 ⇒ 两条都会红（第一条变 `PauseAllThenResumeAll`、
    /// 第二条变 `ReplaceEngine`）。这两个动作在"忙"的时候都是纯损失：
    /// 暂停全部会把正在写的任务全按住，换引擎会关掉一个正在写盘的进程。
    #[test]
    fn a_busy_engine_is_neither_paused_nor_replaced() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();
        assert_eq!(l.next(now, &a, 0, true, true), Rung::Observe, "它在写盘，不是死了");
        assert_eq!(
            l.next(now + Duration::from_secs(600), &a, 0, true, true),
            Rung::Observe,
            "十分钟都不回话也一样：只要进程还在写，就不该动它"
        );
    }

    /// 走过档 2 之后等满了、引擎仍不应答：**忙 ⇒ 不换；闲 ⇒ 换**。
    /// 这一条把 ②½ 与档 3 的先后说清楚：判据在档 2 的等待之后仍要再判一次。
    #[test]
    fn the_busy_verdict_is_rechecked_after_the_pause_wait() {
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();

        let mut busy = Ladder::default();
        assert_eq!(busy.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
        assert_eq!(
            busy.next(now + Duration::from_secs(121), &a, 0, true, true),
            Rung::Observe,
            "等满之后它已经在写盘了 ⇒ 不许换"
        );

        let mut idle = Ladder::default();
        assert_eq!(idle.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
        assert_eq!(
            idle.next(now + Duration::from_secs(121), &a, 0, true, false),
            Rung::ReplaceEngine,
            "同样的时刻、同样的不应答，只是它没在动 ⇒ 这时换引擎才对"
        );
    }

    /// 换引擎之后，若引擎**又**不应答，**不许**立刻再换一次（那是无限换引擎）。
    #[test]
    fn a_second_swap_requires_another_full_wait() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();
        l.next(now, &a, 0, true, false);                                   // 档 2
        assert_eq!(l.next(now + Duration::from_secs(121), &a, 0, true, false), Rung::ReplaceEngine);
        assert_eq!(l.next(now + Duration::from_secs(122), &a, 0, true, false), Rung::Observe,
                   "刚换过，必须重新走一遍等待");
        // 换引擎在 now+121，等满 pre_swap_wait(120 秒) 之后是 now+241。
        assert_eq!(l.next(now + Duration::from_secs(242), &a, 0, true, false), Rung::PauseAllThenResumeAll,
                   "换过之后必须重新走一遍档 2，不能直接再换一次");
    }

    /// 一次健康期结束后，新一轮故障必须从档 2 开始，不能沿用上一轮的档 2 记录直接换引擎。
    #[test]
    fn a_healthy_period_restarts_the_ladder_from_pause_all() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut a = Liveness::default();
        a.record_fail(); a.record_fail(); a.record_fail();
        assert_eq!(l.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
        assert_eq!(l.next(now + Duration::from_secs(10), &Liveness::default(), 0, true, false),
                   Rung::Observe);
        let later = now + Duration::from_secs(500);
        assert_eq!(l.next(later, &a, 0, true, false), Rung::PauseAllThenResumeAll,
                   "新一轮故障必须先走档 2");
    }

    /// ⚠️ 这一条是**写计划时自己走查出来的**：初版实现里"多个任务停滞 ⇒ 档 2"那条路
    /// **不记** `last_pause_all`，于是每一拍都会重下一次"暂停全部"。实现时照着 `next()`
    /// 里 ③ 那一段写就不会犯。
    #[test]
    fn pause_all_is_not_reissued_on_every_tick() {
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &Liveness::default(), 4, false, false), Rung::PauseAllThenResumeAll);
        assert_eq!(l.next(now + Duration::from_secs(5), &Liveness::default(), 4, false, false), Rung::Observe,
                   "档 2 刚做过，等待期内不许再做一次");
    }

    /// 抖动的引擎不许每几拍就重下一次档 2：一拍健康不足以结束本轮故障。
    #[test]
    fn a_flapping_engine_does_not_reissue_pause_all() {
        let mut l = Ladder::default();
        let now = Instant::now();
        let mut bad = Liveness::default();
        bad.record_fail(); bad.record_fail(); bad.record_fail();
        let ok = Liveness::default();

        assert_eq!(l.next(now, &bad, 0, true, false), Rung::PauseAllThenResumeAll);
        assert_eq!(l.next(now + Duration::from_secs(1), &ok, 0, true, false), Rung::Observe);
        assert_eq!(l.next(now + Duration::from_secs(2), &bad, 0, true, false), Rung::Observe,
                   "一拍健康不足以结束本轮故障；等待期内不许重下档 2");
        assert_eq!(l.next(now + Duration::from_secs(121), &bad, 0, true, false), Rung::ReplaceEngine,
                   "抖动不该让阶梯永远停在档 2");
    }
}
