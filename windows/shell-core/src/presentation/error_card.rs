//! 错误卡片：内核给的那一格分型（`class`）→ **一句人话 + 一个动作**（规格 W2）。
//!
//! ## 为什么这一层在壳里（而不是内核直接发三句话）
//!
//! 内核对"哪里坏了"最清楚（它手里有错误码、有旁证），但它**不知道界面此刻在说什么语言**
//! 之外的排版决定；而"同类错误 30 秒内只更新计数、多条并存按严重度置顶"是**界面行为**。
//! ⇒ 分工：内核给 `class`（线上词），壳给句子与次序。两边都不猜对方那一半。
//!
//! ## 两张表必须都在
//!
//! `title` / `explain` / `action` 三个函数都对 [`ClassKind`] 做 match —— 少写一条是
//! **编译错误**（与 `core/src/errors/class.rs` 同一条纪律）；英文表里有汉字会被
//! `tests::no_cjk_in_the_english_cards` 抓住。

use serde::Serialize;

/// 八类病灶（线上词见 [`ClassKind::wire`]）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClassKind {
    NetworkUnreachable,
    DnsFailed,
    ServerRefused,
    DiskSlow,
    DiskFull,
    EngineUnresponsive,
    EngineCrashed,
    VerifyMismatch,
    /// 清单里的文件名在 Windows 上建不出来（冒号/通配符/保留设备名）。
    NameInvalid,
}

impl ClassKind {
    pub const ALL: [ClassKind; 9] = [
        ClassKind::NetworkUnreachable,
        ClassKind::DnsFailed,
        ClassKind::ServerRefused,
        ClassKind::DiskSlow,
        ClassKind::DiskFull,
        ClassKind::EngineUnresponsive,
        ClassKind::EngineCrashed,
        ClassKind::VerifyMismatch,
        ClassKind::NameInvalid,
    ];

    /// 线上词（与 `core/src/errors/class.rs` 的 `Class::wire` 逐字相同）。
    pub fn wire(self) -> &'static str {
        match self {
            ClassKind::NetworkUnreachable => "network_unreachable",
            ClassKind::DnsFailed => "dns_failed",
            ClassKind::ServerRefused => "server_refused",
            ClassKind::DiskSlow => "disk_slow",
            ClassKind::DiskFull => "disk_full",
            ClassKind::EngineUnresponsive => "engine_unresponsive",
            ClassKind::EngineCrashed => "engine_crashed",
            ClassKind::VerifyMismatch => "verify_mismatch",
            ClassKind::NameInvalid => "name_invalid",
        }
    }

    /// 从线上词反解（不认识 ⇒ `None`：**不猜**，界面按"意料之外的错误"处置）。
    pub fn of_wire(wire: &str) -> Option<ClassKind> {
        ClassKind::ALL.into_iter().find(|k| k.wire() == wire)
    }

    /// 严重度（大的先显示；与内核那套数**同源同序**，规格 §2.2）。
    pub fn severity(self) -> u8 {
        match self {
            ClassKind::NetworkUnreachable => 60,
            ClassKind::DnsFailed => 59,
            ClassKind::ServerRefused => 50,
            ClassKind::DiskSlow => 40,
            ClassKind::DiskFull => 30,
            ClassKind::EngineUnresponsive => 20,
            ClassKind::EngineCrashed => 19,
            ClassKind::VerifyMismatch => 18,
            ClassKind::NameInvalid => 28,
        }
    }

    pub fn title(self, lang: crate::i18n::Lang) -> &'static str {
        use ClassKind::*;
        match (self, lang) {
            (NetworkUnreachable, crate::i18n::Lang::Zh) => "连不上服务器",
            (NetworkUnreachable, _) => "Cannot reach the server",
            (DnsFailed, crate::i18n::Lang::Zh) => "域名解析失败",
            (DnsFailed, _) => "Domain lookup failed",
            (ServerRefused, crate::i18n::Lang::Zh) => "服务器拒绝了这次请求",
            (ServerRefused, _) => "The server refused this request",
            (DiskSlow, crate::i18n::Lang::Zh) => "目标磁盘写得太慢",
            (DiskSlow, _) => "The target disk is too slow",
            (DiskFull, crate::i18n::Lang::Zh) => "目标磁盘空间不足或不可写",
            (DiskFull, _) => "The target disk is full or not writable",
            (EngineUnresponsive, crate::i18n::Lang::Zh) => "下载引擎没有响应",
            (EngineUnresponsive, _) => "The download engine stopped answering",
            (EngineCrashed, crate::i18n::Lang::Zh) => "下载引擎启动失败或已退出",
            (EngineCrashed, _) => "The download engine failed to start or exited",
            (NameInvalid, crate::i18n::Lang::Zh) => "有文件名在 Windows 上建不出来",
            (NameInvalid, _) => "Some file names cannot be created on Windows",
            (VerifyMismatch, crate::i18n::Lang::Zh) => "文件校验没有通过",
            (VerifyMismatch, _) => "File verification failed",
        }
    }

    pub fn explain(self, lang: crate::i18n::Lang) -> &'static str {
        use ClassKind::*;
        match (self, lang) {
            (NetworkUnreachable, crate::i18n::Lang::Zh) => "这次没连上服务器：网络断了、被防火墙拦了，或对端暂时不可用。",
            (NetworkUnreachable, _) => "This request did not reach the server: the network may be down, blocked, or the host is unavailable.",
            (DnsFailed, crate::i18n::Lang::Zh) => "域名没解析出地址：DNS 配错了，或这台机器的网络还没就绪。",
            (DnsFailed, _) => "The domain did not resolve: DNS may be misconfigured or the network is not ready yet.",
            (ServerRefused, crate::i18n::Lang::Zh) => "服务器回了一个拒绝：交付码可能不对、链接过期了，或那一侧限流。",
            (ServerRefused, _) => "The server refused: the code may be wrong, the link expired, or it is rate limiting.",
            (DiskSlow, crate::i18n::Lang::Zh) => "目标磁盘写得比下载慢：下载会跟着慢，长时间写不动还会看起来像卡住。",
            (DiskSlow, _) => "The target disk writes slower than the download: transfers slow down and may look stuck.",
            (DiskFull, crate::i18n::Lang::Zh) => "目标磁盘放不下这次的数据，或者没有写权限。",
            (DiskFull, _) => "The target disk cannot hold this data, or it is not writable.",
            (EngineUnresponsive, crate::i18n::Lang::Zh) => "下载引擎连续几次没有回应，自动重启的预算也用完了。",
            (EngineUnresponsive, _) => "The engine did not answer several times in a row and the automatic restart budget is used up.",
            (EngineCrashed, crate::i18n::Lang::Zh) => "下载引擎没能起来，或者起来之后退出了。",
            (EngineCrashed, _) => "The engine could not start, or it exited after starting.",
            (NameInvalid, crate::i18n::Lang::Zh) => "清单里有名字带了 Windows 不允许的字符（例如冒号），系统建不出这些文件——文件本身是好的。",
            (NameInvalid, _) => "The list has names with characters Windows does not allow (for example a colon), so these files cannot be created. The files themselves are fine.",
            (VerifyMismatch, crate::i18n::Lang::Zh) => "下载完的内容与清单里的校验值对不上。",
            (VerifyMismatch, _) => "The downloaded content does not match the checksum in the file list.",
        }
    }

    pub fn action(self, lang: crate::i18n::Lang) -> &'static str {
        use ClassKind::*;
        match (self, lang) {
            (NetworkUnreachable | DnsFailed, crate::i18n::Lang::Zh) => "重试；还不行就换一个网络。",
            (NetworkUnreachable | DnsFailed, _) => "Retry; if it persists, switch networks.",
            (ServerRefused, crate::i18n::Lang::Zh) => "核对交付码，稍后再重试。",
            (ServerRefused, _) => "Check the code and retry later.",
            (DiskSlow, crate::i18n::Lang::Zh) => "换到内置盘，或把「并行文件数」调到 1–2。",
            (DiskSlow, _) => "Use the internal disk, or set parallel files to 1-2.",
            (DiskFull, crate::i18n::Lang::Zh) => "换一个下载目录，或清理出空间。",
            (DiskFull, _) => "Pick another folder, or free up space.",
            (EngineUnresponsive | EngineCrashed, crate::i18n::Lang::Zh) => "重启客户端；仍然失败就导出诊断日志。",
            (EngineUnresponsive | EngineCrashed, _) => "Restart the app; if it still fails, export diagnostics.",
            (NameInvalid, crate::i18n::Lang::Zh) => "联系交付方改名，或改用 macOS / Linux 客户端。",
            (NameInvalid, _) => "Ask the sender to rename them, or use the macOS / Linux client.",
            (VerifyMismatch, crate::i18n::Lang::Zh) => "重新下载这个文件。",
            (VerifyMismatch, _) => "Download this file again.",
        }
    }
}

/// 一张要上屏的卡片（**句子已经按语言成好**，前端只摆位置）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ErrorCard {
    pub class: String,
    pub title: String,
    pub explain: String,
    pub action: String,
    pub severity: u8,
    /// 同类在 30 秒窗口里出现过几次（1 = 只出现一次）。
    pub count: u32,
}

impl ErrorCard {
    /// 一张卡。`count > 1` 时**标题里带上次数**（规格 W2 的"30 秒内合并"要让客户看得见，
    /// 否则他会以为只错了一次）。
    ///
    /// ⚠️ 次数那句话**在这里拼**（Rust），不在前端：JS 不许拼接面向用户的字符串（§3.2）。
    pub fn of(kind: ClassKind, lang: crate::i18n::Lang, count: u32) -> ErrorCard {
        let title = if count > 1 {
            match lang {
                crate::i18n::Lang::Zh => format!("{}（{} 次）", kind.title(lang), count),
                _ => format!("{} (x{})", kind.title(lang), count),
            }
        } else {
            kind.title(lang).to_string()
        };
        ErrorCard {
            class: kind.wire().to_string(),
            title,
            explain: kind.explain(lang).to_string(),
            action: kind.action(lang).to_string(),
            severity: kind.severity(),
            count,
        }
    }
}

/// 30 秒窗口内同类只更新计数、多条并存按严重度置顶（规格 W2）。
///
/// ⚠️ 窗口用的时间是**调用方给的毫秒**（与内核 `errors::Book` 同一条纪律：
///    好测、不带时钟依赖）。
#[derive(Default)]
pub struct CardBook {
    entries: Vec<(ClassKind, u32, u64)>,
}

/// 同类错误的合并窗口（规格 W2）。
pub const DEDUPE_WINDOW_MS: u64 = 30_000;

/// 同时上屏的卡片上限（再多就把客户淹了；严重度最高的那几张才是要看的）。
pub const MAX_CARDS: usize = 3;

impl CardBook {
    /// 记一次；返回**当前该显示的卡片**（按严重度降序、最多 [`MAX_CARDS`] 张）。
    pub fn observe(
        &mut self,
        kind: ClassKind,
        now_ms: u64,
        lang: crate::i18n::Lang,
    ) -> Vec<ErrorCard> {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|(k, _, at)| *k == kind && now_ms.saturating_sub(*at) <= DEDUPE_WINDOW_MS)
        {
            entry.1 += 1;
            entry.2 = now_ms;
        } else {
            self.entries.retain(|(k, _, at)| {
                *k != kind || now_ms.saturating_sub(*at) <= DEDUPE_WINDOW_MS
            });
            self.entries.push((kind, 1, now_ms));
        }
        self.cards(lang)
    }

    /// 当前该显示的卡片（不改变状态）。
    pub fn cards(&self, lang: crate::i18n::Lang) -> Vec<ErrorCard> {
        let mut out: Vec<ErrorCard> = self
            .entries
            .iter()
            .map(|(k, n, _)| ErrorCard::of(*k, lang, *n))
            .collect();
        out.sort_by(|a, b| b.severity.cmp(&a.severity));
        out.truncate(MAX_CARDS);
        out
    }

    /// 全部清掉（客户点了"重试"成功之后）。
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    fn ck(s: &str) -> ClassKind {
        ClassKind::of_wire(s).expect("线上词必须认得")
    }

    /// 八类 × 三种语言位（`System` 兜底英文）× 三句，一个都不许缺。
    #[test]
    fn every_class_has_three_sentences_in_both_languages() {
        for kind in ClassKind::ALL {
            for lang in [Lang::Zh, Lang::En] {
                assert!(!kind.title(lang).is_empty());
                assert!(!kind.explain(lang).is_empty());
                assert!(!kind.action(lang).is_empty());
            }
        }
    }

    /// 英文里不许有汉字（中文表与英文表真的分开了）。
    #[test]
    fn no_cjk_in_the_english_cards() {
        let has_cjk = |s: &str| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        for kind in ClassKind::ALL {
            for s in [
                kind.title(Lang::En),
                kind.explain(Lang::En),
                kind.action(Lang::En),
            ] {
                assert!(!has_cjk(s), "英文卡片里有汉字：{s}");
            }
            assert!(has_cjk(kind.title(Lang::Zh)), "中文卡片里必须有中文");
        }
    }

    /// 线上词逐字对齐内核（写错一个字母，全世界的卡片都会消失）。
    #[test]
    fn the_wire_words_are_exactly_the_spec() {
        assert_eq!(
            ClassKind::ALL.map(ClassKind::wire),
            [
                "network_unreachable",
                "dns_failed",
                "server_refused",
                "disk_slow",
                "disk_full",
                "engine_unresponsive",
                "engine_crashed",
                "verify_mismatch",
                "name_invalid",
            ]
        );
        assert_eq!(ClassKind::of_wire("nope"), None);
    }

    /// 严重度两两不同且网络类最高（规格 §2.2 的口径：最宽的故障置顶）。
    #[test]
    fn severity_orders_the_cards() {
        let mut seen: Vec<u8> = ClassKind::ALL.iter().map(|k| k.severity()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 9, "九类严重度必须两两不同");
        assert!(ck("network_unreachable").severity() > ck("verify_mismatch").severity());
    }

    /// 30 秒窗口内同类只加计数；超窗之后重新计数。
    ///
    /// 判别力：把 `DEDUPE_WINDOW_MS` 改成 0 ⇒ 第二条断言红。
    #[test]
    fn the_same_class_within_the_window_only_counts_up() {
        let mut book = CardBook::default();
        let first = book.observe(ClassKind::DiskSlow, 1_000, Lang::Zh);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].count, 1);
        let second = book.observe(ClassKind::DiskSlow, 2_000, Lang::Zh);
        assert_eq!(second.len(), 1, "30 秒内同类不许再冒一张");
        assert_eq!(second[0].count, 2, "窗口内只加计数");
        // 窗口从**最近一次**算起：2_000 之后满 30 秒才算超窗。
        let after = book.observe(ClassKind::DiskSlow, 40_000, Lang::Zh);
        assert_eq!(after[0].count, 1, "超窗之后重新计数");
    }

    /// 多条并存：按严重度降序，最多三张。
    #[test]
    fn cards_are_sorted_by_severity_and_capped() {
        let mut book = CardBook::default();
        for (i, wire) in ["verify_mismatch", "disk_slow", "network_unreachable", "disk_full", "dns_failed"]
            .iter()
            .enumerate()
        {
            book.observe(ck(wire), 1_000 + i as u64, Lang::En);
        }
        let cards = book.cards(Lang::En);
        assert_eq!(cards.len(), MAX_CARDS);
        assert_eq!(cards[0].class, "network_unreachable");
        assert_eq!(cards[1].class, "dns_failed");
        assert_eq!(cards[2].class, "disk_slow");
    }

    /// 清空之后一张都不剩（客户重试成功那一刻）。
    #[test]
    fn clearing_leaves_no_cards() {
        let mut book = CardBook::default();
        book.observe(ClassKind::DnsFailed, 1, Lang::Zh);
        book.clear();
        assert!(book.cards(Lang::Zh).is_empty());
    }
}
