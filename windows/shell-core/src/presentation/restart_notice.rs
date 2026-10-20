//! 自动重启的三句话（规格 W3）：界面只说"现在在做什么"，不解释判据。
//!
//! 内核的 `restart_progress` 只给四个字段（`attempt` / `budget` / `state` /
//! `sinceMs` / `fails`）—— **判据在 `core/src/engine/restart.rs`**。这里只做一件事：
//! 把那台状态机此刻处在哪一格，翻成一句客户读得懂的话（中英各一份）。
//!
//! ## 为什么句子在壳里
//!
//! 内核不知道界面此刻用什么语言（它只知道"客户选的语言"是哪一个，但**排版与
//! 什么时候该说**是界面的事）。⇒ 内核给状态、壳给句子，两边都不猜对方那一半。

/// 此刻要不要在界面上说话；要说的话是哪一句。
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct RestartNotice {
    pub text: String,
}

impl RestartNotice {
    /// `state` 是内核的四个词：`idle` / `waiting` / `verifying` / `exhausted`。
    ///
    /// * `idle` ⇒ 什么都不说（`None`）—— **不许**因为"没有重启"就摆一条横幅；
    /// * `waiting` ⇒ 「正在重启下载引擎（第 n/N 次）…」；
    /// * `verifying` ⇒ 「引擎已重启，正在确认它能正常工作…」；
    /// * `exhausted` ⇒ 预算用尽了，手动重试是唯一出路（与八类错误里的
    ///   `engine_unresponsive` 那张卡**不重复**：那张卡说的是"引擎没响应"，
    ///   这一句说的是"自动重启已经放弃"）。
    pub fn of(
        lang: crate::i18n::Lang,
        state: &str,
        attempt: u32,
        budget: u32,
    ) -> Option<RestartNotice> {
        let text = match (state, lang) {
            ("waiting", crate::i18n::Lang::Zh) => {
                format!("正在重启下载引擎（第 {attempt}/{budget} 次）…")
            }
            ("waiting", _) => {
                format!("Restarting the download engine (attempt {attempt}/{budget})…")
            }
            ("verifying", crate::i18n::Lang::Zh) => {
                "引擎已重启，正在确认它能正常工作…".to_string()
            }
            ("verifying", _) => "The engine restarted - checking that it works…".to_string(),
            ("exhausted", crate::i18n::Lang::Zh) => {
                "自动重启没能恢复下载引擎，请手动点「重试」".to_string()
            }
            ("exhausted", _) => {
                "Automatic restarts could not recover the download engine - please click Retry"
                    .to_string()
            }
            _ => return None,
        };
        Some(RestartNotice { text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    /// `idle` 与任何不认识的词都**不说话**（不猜、不编一句出来）。
    #[test]
    fn the_idle_state_says_nothing_at_all() {
        assert_eq!(RestartNotice::of(Lang::Zh, "idle", 0, 3), None);
        assert_eq!(RestartNotice::of(Lang::Zh, "whatever", 1, 3), None);
    }

    /// 三种要说话的格子，中文逐字（规格 W3 的那一句）。
    #[test]
    fn the_three_speaking_states_keep_the_spec_wording() {
        let waiting = RestartNotice::of(Lang::Zh, "waiting", 1, 3).expect("要说");
        assert_eq!(waiting.text, "正在重启下载引擎（第 1/3 次）…");
        let verifying = RestartNotice::of(Lang::Zh, "verifying", 1, 3).expect("要说");
        assert_eq!(verifying.text, "引擎已重启，正在确认它能正常工作…");
        let exhausted = RestartNotice::of(Lang::Zh, "exhausted", 3, 3).expect("要说");
        assert!(exhausted.text.contains("手动"));
    }

    /// 英文一列没有汉字，且三句都在（漏一句会让英文界面在那一刻**什么都不显示**）。
    #[test]
    fn the_english_column_has_no_cjk_and_covers_all_three() {
        for state in ["waiting", "verifying", "exhausted"] {
            let n = RestartNotice::of(Lang::En, state, 2, 3).expect("英文也要说");
            assert!(
                !n.text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "英文里混进了汉字：{}",
                n.text
            );
        }
    }

    /// 第几次是**数出来的**，不是写死的（`attempt/budget` 要跟着内核走）。
    #[test]
    fn the_attempt_counter_follows_the_kernel() {
        let second = RestartNotice::of(Lang::En, "waiting", 2, 3).expect("要说");
        assert!(second.text.contains("2/3"), "{}", second.text);
    }
}
