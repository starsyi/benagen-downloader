//! disk_notice —— 「目标盘写得比下载慢」的常驻提示。
//!
//! ---------------------------------------------------------------------------
//! 🔴 为什么客户端要主动说这一句（这不是预防性设计，是一次现场）
//! ---------------------------------------------------------------------------
//!
//! 2026-10-20，Windows 客户端 0.2.7，客户把下载目录放在**移动硬盘**上：下载停在
//! 1.7 MB/s，并且反复报「下载引擎已断开」。带宽、CDN、交付源都没问题 —— 卡住的是盘：
//! aria2 的写盘在它**自己的主循环**里同步完成，盘一慢，RPC 应答与任务进度一起被拖慢，
//! 从外面看就像"网络慢"和"引擎死了"。
//!
//! ⇒ 壳在**真正开始下载时**量一次目标盘的写入速度（`shell-win` 的 `diskspeed`，≤2 s），
//!   偏低就把这句话摆到常驻提示里。**成句在这里**，前端只负责画（规格约束 10：
//!   显示值全部由 Rust 决定）。
//!
//! ⚠️ 判据只有一条、且只在这里：实测值 **低于** [`SLOW_WRITE_MBPS`] 才提示。
//!    `None`（没量 / 量不出来）**一律不提示** —— 宁可不说，也不许拿猜的数字吓人。

/// 低于这个写入速度（MB/s，十进制，与「1.7 MB/s」这种显示口径一致）就提示。
///
/// 取 20：USB 3.0 机械盘（80–120 MB/s）、SATA 盘、内置 SSD 都在它之上；
/// 而 USB 2.0 盘（约 30 MB/s 顺序写，随机写更差）、老 U 盘、快满的机械盘会落下来。
/// ⚠️ 这是个**经验阈值**，不是测出来的边界：它只决定"要不要提一句"，
///    不参与任何下载决策 —— 提错了只是多一句话，不会让下载变慢。
pub const SLOW_WRITE_MBPS: u64 = 20;

/// 一句常驻提示（`/api/state` 的 `disk` 那一格）。
///
/// ⚠️ 这里只存**实测的数字**、不存成句：句子的语言由 `/api/state` 那一刻的界面语言定
///    （设置里换语言要立刻生效，句子存下来就只能等下次量盘才换）。
#[derive(Clone, PartialEq, Debug)]
pub struct DiskNotice {
    /// 实测写入速度（整数 MB/s）。
    pub mbps: u64,
}

impl DiskNotice {
    /// 实测值 → 那一句。数字是**实测的整数 MB/s**，不取整成"很慢"这种模糊说法：
    /// 客户拿着它才能判断"是盘的问题还是网的问题"。
    pub fn text(lang: crate::i18n::Lang, mbps: u64) -> String {
        crate::i18n::tf(
            lang,
            crate::i18n::Key::DiskSlow,
            &[("mbps", &mbps.to_string())],
        )
    }

    /// 判据：`None` 或够快 ⇒ 不说话。
    pub fn of(write_mbps: Option<u64>) -> Option<DiskNotice> {
        let mbps = write_mbps?;
        if mbps >= SLOW_WRITE_MBPS {
            return None;
        }
        Some(DiskNotice { mbps })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没量 / 量不出来 ⇒ **一句话都不说**。
    /// 判别力：把 `let mbps = write_mbps?` 换成 `write_mbps.unwrap_or(0)` ⇒ 这里
    /// 会冒出一句"约 0 MB/s"的假警报 ⇒ 红。
    #[test]
    fn no_measurement_says_nothing() {
        assert_eq!(DiskNotice::of(None), None);
    }

    /// 够快 ⇒ 不说话（阈值是**含**的：正好 20 也不提）。
    /// 判别力：把判据改成 `>` ⇒ 20 那一档红。
    #[test]
    fn a_disk_at_or_above_the_threshold_is_silent() {
        assert_eq!(DiskNotice::of(Some(SLOW_WRITE_MBPS)), None);
        assert_eq!(DiskNotice::of(Some(500)), None);
    }

    /// 慢盘 ⇒ 说话，并且**把实测数写进去**（客户要拿它判断是盘还是网）。
    /// 判别力：句子里改成固定"很慢" ⇒ 第二个断言红。
    #[test]
    fn a_slow_disk_is_named_with_its_measured_speed() {
        let n = DiskNotice::of(Some(9)).expect("9 MB/s 必须提示");
        assert_eq!(n.mbps, 9, "实测数要原样留着（成句时再写进去）");
        let text = DiskNotice::text(crate::i18n::Lang::Zh, n.mbps);
        assert!(text.contains("9 MB/s"), "要把实测值说出来: {text}");
        assert!(
            text.contains("内置盘"),
            "要给出可执行的下一步（换盘 / 降并发），不能只说慢: {text}"
        );
        assert!(
            text.contains("并行文件数"),
            "界面上那颗旋钮的名字要与设置页一致: {text}"
        );
        // 判据（规格 I-2）：同一句英文版里不许留汉字，且数字照样在。
        let en = DiskNotice::text(crate::i18n::Lang::En, n.mbps);
        assert!(en.contains("9 MB/s"), "{en}");
        assert!(!en.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)), "{en}");
    }

    /// 阈值两侧各取一档，把边界钉死。
    #[test]
    fn the_threshold_boundary_is_pinned() {
        assert!(DiskNotice::of(Some(SLOW_WRITE_MBPS - 1)).is_some());
        assert!(DiskNotice::of(Some(SLOW_WRITE_MBPS)).is_none());
    }
}
