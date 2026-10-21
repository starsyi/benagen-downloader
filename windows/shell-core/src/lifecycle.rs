//! lifecycle —— 关窗 / 托盘 / 防休眠 / 看护节拍的**纯判据**（无任何 OS 调用，能在 macOS 上测）。
//!
//! 背景（现场）：客户端必须一直在线，否则容易断开。原因有两条：关窗就是退出；
//! 机器睡眠 / 锁屏后，引擎与 CDN 的长连接被系统掐断。所以：
//!   * 关窗 ⇒ 收到托盘，下载继续（托盘装不上时才真退出，否则用户再也退不掉程序）；
//!   * 有未完成的下载时 ⇒ 阻止系统**进入睡眠**（不阻止关屏）；
//!   * 窗口藏起来时没有前端在轮询 ⇒ 壳自己按节拍问内核，让"内核 → 引擎"那条连接一直有人用。

use std::time::Duration;

use crate::protocol::TaskState;

/// 用户点关窗时该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseIntent {
    /// 窗口藏起来，下载继续。
    HideToTray,
    /// 真退出（收尾内核）。
    Exit,
}

/// 关窗的判决。**托盘不可用 ⇒ 必须退出**：否则窗口没了、任务栏没了、托盘也没有，
/// 用户再也关不掉这个程序。
pub fn close_intent(tray_available: bool) -> CloseIntent {
    if tray_available {
        CloseIntent::HideToTray
    } else {
        CloseIntent::Exit
    }
}

/// 要不要阻止系统睡眠：有任何未完成的下载就要。
///
/// `unfinished` 取自传输列表里"未落定"的行数（与侧栏徽标同一口径）。
/// 取不到数（内核没应答）时由调用方传 `None`：**保持上一次的决定**，
/// 不因为一次抖动就放开（放开 ⇒ 机器可能正好此刻睡下去）。
pub fn keep_awake(unfinished: Option<usize>, previous: bool) -> bool {
    match unfinished {
        Some(n) => n > 0,
        None => previous,
    }
}

/// 防休眠与托盘计数只数「还要下载」的：等待中 / 进行中，且**没有被暂停**。
///
/// 暂停的不算：暂停就是没有连接在传，机器睡下去不会掐断任何东西；用一个已经停下的任务把整台机器钉醒是白耗电。
/// 失败的那一档也不算——它不会自己好，留着只会让机器永远不睡。徽标口径另有判据
/// （`TransferRow::is_unfinished` 把失败算进去），两者不共用。
pub fn is_busy(state: TaskState, paused: bool) -> bool {
    !paused && matches!(state, TaskState::Waiting | TaskState::Active)
}

/// 看护线程一拍从内核取到的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// 取到了：这么多个在忙的任务。
    Count(usize),
    /// 这一拍没取到（可能只是抖动）。
    Miss,
    /// 内核已经没了：没有进行中的活，也不会再有。
    Gone,
}

/// 连续这么多拍取不到数，就放开防休眠。单次抖动不放开（见 [`keep_awake`]）。
pub const GIVE_UP_AFTER_MISSES: u32 = 3;

/// 看护线程的防休眠状态机：把一拍的读数翻成"要不要阻止睡眠"。
#[derive(Debug, Default)]
pub struct AwakeTracker {
    misses: u32,
    awake: bool,
}

impl AwakeTracker {
    /// 返回 `(要不要阻止睡眠, 在忙的任务数)`；后者为 `None` 表示"这一拍还没有可信的数"。
    pub fn observe(&mut self, reading: Reading) -> (bool, Option<usize>) {
        let unfinished = match reading {
            Reading::Count(n) => {
                self.misses = 0;
                Some(n)
            }
            Reading::Gone => {
                self.misses = 0;
                Some(0)
            }
            Reading::Miss => {
                self.misses = self.misses.saturating_add(1);
                (self.misses >= GIVE_UP_AFTER_MISSES).then_some(0)
            }
        };
        self.awake = keep_awake(unfinished, self.awake);
        (self.awake, unfinished)
    }
}

/// 看护线程的节拍：窗口藏着时前端不轮询，壳要自己勤快一点（让引擎连接一直有人用）；
/// 窗口可见时前端自己在轮询，壳放慢，避免无谓加倍。
pub fn watch_interval(window_visible: bool) -> Duration {
    if window_visible {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(2)
    }
}

/// 托盘悬停文案。有下载时带上数量，让用户不开窗口也知道它在干活。
///
/// ⚠️ 启动期就要用（握手之前，窗口可能还没建起来）⇒ 语言由调用方给：
///    `shell-win` 用 `locale::startup_lang()`，握手之后换成 `Session::lang()`。
pub fn tray_tip(lang: crate::i18n::Lang, unfinished: usize) -> String {
    if unfinished == 0 {
        crate::i18n::t(lang, crate::i18n::Key::UiAppTitle).to_string()
    } else {
        let n = unfinished.to_string();
        crate::i18n::tf(
            lang,
            crate::i18n::Key::StartupTrayTip,
            &[("app", crate::i18n::t(lang, crate::i18n::Key::UiAppTitle)), ("n", &n)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判别力：两个分支写反 ⇒ 托盘在时真退出（下载中断）/ 托盘不在时藏起来（退不掉）。
    #[test]
    fn closing_hides_only_when_the_tray_can_bring_it_back() {
        assert_eq!(close_intent(true), CloseIntent::HideToTray);
        assert_eq!(close_intent(false), CloseIntent::Exit);
    }

    #[test]
    fn keep_awake_follows_unfinished_work() {
        assert!(keep_awake(Some(3), false));
        assert!(!keep_awake(Some(0), true), "做完了就放开，不能一直占着");
    }

    /// 一次取不到数不许放开防休眠。判别力：把 None 分支改成 false ⇒ 红。
    #[test]
    fn a_missed_reading_keeps_the_previous_decision() {
        assert!(keep_awake(None, true));
        assert!(!keep_awake(None, false));
    }

    /// 判别力：把 `is_busy` 放宽成含 `Error` ⇒ 第二条红；去掉 `!paused` ⇒ 第三条红。
    #[test]
    fn only_unpaused_waiting_and_active_tasks_keep_the_machine_awake() {
        assert!(is_busy(TaskState::Waiting, false) && is_busy(TaskState::Active, false));
        assert!(!is_busy(TaskState::Error, false), "失败不会自己好，不许把机器一直留着");
        assert!(!is_busy(TaskState::Complete, false) && !is_busy(TaskState::Removed, false));
        assert!(!is_busy(TaskState::Active, true), "暂停的任务不许把机器钉醒");
    }

    /// 判别力：删掉 `Reading::Gone` 那一支（沿用"一次取不到就保持"）⇒ 第一条红 ——
    /// 这就是"内核断开后防休眠永久残留"那个缺陷本身。
    #[test]
    fn a_dead_kernel_releases_the_machine_at_once() {
        let mut t = AwakeTracker::default();
        assert_eq!(t.observe(Reading::Count(2)), (true, Some(2)));
        assert_eq!(t.observe(Reading::Gone), (false, Some(0)));
    }

    /// 判别力：把 `>=` 改成 `>` 或把 `GIVE_UP_AFTER_MISSES` 减一 ⇒ 对应断言红；
    /// 把计数不在成功读数时清零 ⇒ 第二段（交替取不到）红。
    #[test]
    fn misses_must_be_consecutive_to_release_and_one_is_a_blip() {
        let mut t = AwakeTracker::default();
        t.observe(Reading::Count(2));
        assert_eq!(t.observe(Reading::Miss), (true, None), "单次抖动不放开");
        assert_eq!(t.observe(Reading::Miss), (true, None));
        assert_eq!(t.observe(Reading::Miss), (false, Some(0)), "连续第三拍放开");

        let mut t = AwakeTracker::default();
        t.observe(Reading::Count(2));
        t.observe(Reading::Miss);
        t.observe(Reading::Miss);
        assert_eq!(t.observe(Reading::Count(1)), (true, Some(1)), "取到数要清零");
        assert_eq!(t.observe(Reading::Miss), (true, None), "清零之后重新计");
    }

    #[test]
    fn a_hidden_window_is_watched_more_often_than_a_visible_one() {
        assert!(watch_interval(false) < watch_interval(true));
    }

    #[test]
    fn the_tip_mentions_the_count_only_when_there_is_work() {
        assert!(!tray_tip(crate::i18n::Lang::Zh, 0).contains("进行中"));
        assert!(tray_tip(crate::i18n::Lang::Zh, 7).contains("7 个任务进行中"));
    }

    /// 启动期托盘文案随语言切换：中英不同、英文里没有汉字（握手前的语言来自系统）。
    #[test]
    fn the_tray_tip_speaks_the_startup_language() {
        use crate::i18n::Lang;
        let zh = tray_tip(Lang::Zh, 3);
        let en = tray_tip(Lang::En, 3);
        assert_ne!(zh, en, "中英两份托盘文案不能是同一句");
        assert!(!en.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "英文托盘文案里不许有汉字：{en}");
        assert_eq!(tray_tip(Lang::En, 0), "Benagen Data Downloader");
    }
}
