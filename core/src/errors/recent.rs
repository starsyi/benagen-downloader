//! 同类错误 30 秒内只记一次（规格 W2）。
//!
//! **为什么**：现场一个慢盘能在 20 小时里刷出上千条同一类失败（0.2.7 的
//! `rpc_retry=737`）。界面不能跟着刷——客户需要的是"又发生了 N 次"，
//! 而不是 N 张卡片。
//!
//! 判据：
//! * 同一类，距上次**不超过** 30 000 ms ⇒ 只把 `count` 加一，**不再重新浮出**；
//! * 隔了 30 秒以上又出现 ⇒ 算新的一次（重新浮出，计数从 1 开始）；
//! * 不同类**永不合并**；表的长度有上限（过期即清），不会无限长。

use crate::errors::Class;

/// 合并窗口（毫秒）。挑 30 秒：比一次重试退避（5/15/45 秒起步）短，
/// 客户不会因为"退避中"而看不到同一条错误的持续状态。
pub const WINDOW_MS: u64 = 30_000;

/// 一次"被看见"的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// 新的一次（界面应当浮出它）。
    New,
    /// 窗口内重复（界面只更新计数）。
    Repeat { count: u32 },
}

/// 某一类错误最近一次被看见的账。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub class: Class,
    /// 第一次看见（窗口内那次）的时刻，ms。
    pub first_ms: u64,
    /// 最近一次看见的时刻，ms。
    pub last_ms: u64,
    /// 窗口内累计次数（含第一次）。
    pub count: u32,
}

impl Entry {
    /// 同一条错误被压成一句话时用的后缀，例如 `（已 3 次）`。
    pub fn repeats(&self) -> u32 {
        self.count.saturating_sub(1)
    }
}

/// 有界的小账本（最多 8 条：八类各一条）。
#[derive(Debug, Default)]
pub struct Recent {
    entries: Vec<Entry>,
}

impl Recent {
    pub fn new() -> Self {
        Recent { entries: Vec::new() }
    }

    /// 记一次。返回"这是新的一次"还是"窗口内的第 N 次"。
    pub fn observe(&mut self, class: Class, now_ms: u64) -> Seen {
        self.prune(now_ms);
        if let Some(e) = self.entries.iter_mut().find(|e| e.class == class) {
            if now_ms.saturating_sub(e.last_ms) <= WINDOW_MS {
                e.count = e.count.saturating_add(1);
                e.last_ms = now_ms;
                return Seen::Repeat { count: e.count };
            }
            // 隔得太久：当成新的一次
            e.first_ms = now_ms;
            e.last_ms = now_ms;
            e.count = 1;
            return Seen::New;
        }
        self.entries.push(Entry {
            class,
            first_ms: now_ms,
            last_ms: now_ms,
            count: 1,
        });
        Seen::New
    }

    /// 当前仍在窗口内的（按严重度降序，界面按这个顺序画）。
    pub fn snapshot(&self, now_ms: u64) -> Vec<Entry> {
        let mut out: Vec<Entry> = self
            .entries
            .iter()
            .copied()
            .filter(|e| now_ms.saturating_sub(e.last_ms) <= WINDOW_MS)
            .collect();
        out.sort_by(|a, b| b.class.severity().cmp(&a.class.severity()));
        out
    }

    /// 清掉已经过期的那几条（窗口是滑动窗口，不是永久账）。
    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|e| now_ms.saturating_sub(e.last_ms) <= WINDOW_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据（规格 W2）：同一个类在 30 秒内重复 ⇒ 只加计数、不再算新的一次。
    #[test]
    fn the_same_class_inside_the_window_is_only_counted() {
        let mut r = Recent::new();
        assert_eq!(r.observe(Class::DiskSlow, 1_000), Seen::New);
        assert_eq!(
            r.observe(Class::DiskSlow, 2_000),
            Seen::Repeat { count: 2 }
        );
        assert_eq!(
            r.observe(Class::DiskSlow, 30_999),
            Seen::Repeat { count: 3 }
        );
        let snap = r.snapshot(30_999);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].count, 3);
        assert_eq!(snap[0].repeats(), 2, "文案要说“已 2 次”，不是“共 3 次”");
    }

    /// 判据：静默超过窗口 ⇒ 算新的一次（计数归 1），否则客户会看到
    /// 一个永远不清零的“已 1000 次”，反而分不清这是不是刚发生。
    #[test]
    fn silence_longer_than_the_window_starts_a_new_occurrence() {
        let mut r = Recent::new();
        r.observe(Class::DiskSlow, 1_000);
        r.observe(Class::DiskSlow, 2_000);
        // 窗口是“距**上一次**”，不是“距第一次”：上一次在 2 000，故 32 001 以后才算新
        assert_eq!(r.observe(Class::DiskSlow, 32_000), Seen::Repeat { count: 3 }, "边界上仍算同一场");
        assert_eq!(r.observe(Class::DiskSlow, 62_001), Seen::New);
        assert_eq!(r.snapshot(62_001)[0].count, 1);
    }

    /// 判据：不同类永不合并——八类各占一格。
    #[test]
    fn different_classes_never_merge() {
        let mut r = Recent::new();
        r.observe(Class::DiskSlow, 1_000);
        r.observe(Class::DiskFull, 1_100);
        let snap = r.snapshot(1_100);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].class, Class::DiskFull, "盘满比盘慢更该置顶");
    }

    /// 判据：过期的条目要被清掉（否则表会随运行时长无限增长）。
    #[test]
    fn stale_entries_are_pruned() {
        let mut r = Recent::new();
        r.observe(Class::NetworkUnreachable, 1_000);
        assert_eq!(r.snapshot(31_000).len(), 1, "窗口边界内还算数");
        assert!(r.snapshot(31_001).is_empty(), "过了窗口就不该再出现");
    }
}
