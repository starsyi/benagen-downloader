//! 传输页顶部的「暂停全部 / 一键开始」两颗按钮的可用性，以及换下载目录时
//! 「搬运已下载文件」那一行的进度话（规格 2026-10-12 §5.2 / §6）。
//!
//! 本文件**没有任何 OS 调用**，所以能在 macOS 上 `cargo test`（本仓库「能写出断言的代码
//! 一律搬出视图目标」的纪律）。可用性的判据**只在这里**——JS 不重算（与
//! `TransferGlobalSummary::can_clear_finished` 同一条纪律）：
//!
//! * `can_pause_all`：有任何一行还提供「暂停」（= 活动中的，或排着队没暂停的）；
//! * `can_resume_all`：有任何一行提供「继续」（= 已暂停的），**或**壳记着「刚换过目录、
//!   队列还没补回来」（`resume_pending`）——后一种情形引擎是空的，列表里一行都没有，
//!   只有壳知道还有事要做；
//! * 搬运**进行中**两颗都不可用：搬运和引擎同写一个文件是数据损坏级的风险，内核那边
//!   也会拒绝（`op_resume_all`）——界面上就不要给一个注定被拒的按钮。

use serde::Serialize;

use crate::presentation::format::ByteFormat;
use crate::presentation::transfer_row::TransferRow;
use crate::protocol::{MigrateStatus, TaskAction};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchControl {
    pub can_pause_all: bool,
    pub can_resume_all: bool,
    /// 搬运还在跑。
    pub migrate_running: bool,
    /// 搬运进行中的进度话；没在搬运时是 `None`。
    pub migrate_text: Option<String>,
    /// 搬运**结束后**客户需要知道的事（失败 / 没覆盖）；干净时是 `None`。
    pub migrate_note: Option<String>,
}

impl BatchControl {
    pub fn of(
        rows: &[TransferRow],
        resume_pending: bool,
        migrate: Option<&MigrateStatus>,
    ) -> Self {
        let running = migrate.is_some_and(|m| m.running);
        let offers = |a: TaskAction| rows.iter().any(|r| r.available_actions.contains(&a));
        Self {
            can_pause_all: !running && offers(TaskAction::Pause),
            can_resume_all: !running && (offers(TaskAction::Unpause) || resume_pending),
            migrate_running: running,
            migrate_text: migrate.filter(|m| m.running).map(running_text),
            migrate_note: migrate.filter(|m| m.finished && !m.running).and_then(finished_note),
        }
    }

    /// 壳这一侧记下的搬运失败（内核没有 `MigrateStatus` 可以说）。并进 `migrate_note`，
    /// 与已有的结束话用空格并排（规则同 [`finished_note`]）。
    pub fn with_migrate_failure(mut self, failure: Option<&MigrateFailure>) -> Self {
        let Some(failure) = failure else {
            return self;
        };
        let sentence = match failure {
            MigrateFailure::Refused(reason) => migrate_refused_note(reason),
            MigrateFailure::ProgressLost => PROGRESS_LOST_NOTE.to_string(),
        };
        self.migrate_note = Some(match self.migrate_note.take() {
            Some(note) => format!("{note} {sentence}"),
            None => sentence,
        });
        self
    }
}

/// 搬运没能完成的原因（壳这一侧）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateFailure {
    /// 没能开始，或中途被内核拒绝：`reason` 是内核原文或壳给的那一句原因。
    Refused(String),
    /// 取不到搬运进度（内核没了，或搬运随内核重启丢了）：旧文件可能没有搬完。
    ProgressLost,
}

/// 搬运没能完成的那句话（与 [`finished_note`] 的失败句同一个后果描述）。
///
/// `reason` 是内核原文或壳给的那一句原因；这里只拼句式，不加工它。
pub fn migrate_refused_note(reason: &str) -> String {
    format!("旧目录里已下载的文件没能搬到新目录，会从头下载（旧文件仍在原目录）。原因：{reason}")
}

/// 取不到搬运进度的那句话（与 [`migrate_refused_note`] 并列，两者都经 [`BatchControl::with_migrate_failure`] 出口）。
pub const PROGRESS_LOST_NOTE: &str = "没能取到搬运进度，旧文件可能没有搬完。";


fn running_text(m: &MigrateStatus) -> String {
    format!(
        "正在搬运已下载的文件：{}/{} 个（{} / {}）",
        m.files_done,
        m.files_total,
        ByteFormat::text(m.bytes_done as i64),
        ByteFormat::text(m.bytes_total as i64),
    )
}

/// 结束后要说的话。两类各一句，都有才并排；**干净 ⇒ `None`**（不说「搬运完成」这种
/// 客户不需要读的话——按钮能点了就是答案）。
///
/// ⚠️ 失败那句点明**后果**（会从头下载）与**文件还在哪**（旧目录）：客户拿到这句话唯一
/// 要做的决定就是「要不要自己去旧目录处理」，所以两样都得在话里。
fn finished_note(m: &MigrateStatus) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(first) = m.errors.first() {
        parts.push(format!(
            "有 {} 个文件没能搬到新目录，会从头下载（旧文件仍在原目录）。第一个：{}：{}",
            m.errors.len(),
            first.path,
            first.reason
        ));
    }
    if !m.skipped.is_empty() {
        parts.push(format!(
            "有 {} 个文件在新目录里已经存在，没有覆盖。",
            m.skipped.len()
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::RowColor;
    use crate::protocol::MigrateProblem;
    use std::collections::BTreeSet;

    fn row(actions: &[TaskAction]) -> TransferRow {
        TransferRow {
            gid: "g".to_string(),
            title: "t".to_string(),
            manifest_path: None,
            progress_text: String::new(),
            percent_text: String::new(),
            progress_fraction: 0.0,
            speed_text: String::new(),
            state_label: String::new(),
            state_color: RowColor::Blue,
            state_icon_name: String::new(),
            shows_paused_badge: false,
            error_text: None,
            available_actions: actions.iter().copied().collect::<BTreeSet<_>>(),
            counts_as_unfinished: true,
        }
    }

    fn status(running: bool, finished: bool) -> MigrateStatus {
        MigrateStatus {
            running,
            finished,
            files_done: 2,
            files_total: 5,
            bytes_done: 1024,
            bytes_total: 4096,
            ..Default::default()
        }
    }

    /// 暂停只看「有没有行提供暂停」，继续只看「有没有行提供继续」。
    /// 判别力：把两个判据对调 ⇒ 两条断言都红。
    #[test]
    fn availability_follows_what_the_rows_offer() {
        let active = [row(&[TaskAction::Pause, TaskAction::Remove])];
        let c = BatchControl::of(&active, false, None);
        assert!(c.can_pause_all && !c.can_resume_all, "{c:?}");

        let paused = [row(&[TaskAction::Unpause, TaskAction::Remove])];
        let c = BatchControl::of(&paused, false, None);
        assert!(!c.can_pause_all && c.can_resume_all, "{c:?}");

        let done = [row(&[TaskAction::Retry, TaskAction::Remove])];
        let c = BatchControl::of(&done, false, None);
        assert!(!c.can_pause_all && !c.can_resume_all, "已完成的行两个都不给: {c:?}");
    }

    /// 换完目录后引擎是空的（一行都没有），「一键开始」只能靠壳记着的 `resume_pending`。
    /// 判别力：去掉 `|| resume_pending` ⇒ 红。
    #[test]
    fn resume_is_offered_on_an_empty_list_when_a_hand_over_is_pending() {
        let c = BatchControl::of(&[], true, None);
        assert!(c.can_resume_all && !c.can_pause_all, "{c:?}");
        let c = BatchControl::of(&[], false, None);
        assert!(!c.can_resume_all, "{c:?}");
    }

    /// 搬运进行中两颗都灰掉，并给出进度话。
    #[test]
    fn nothing_is_offered_while_files_are_being_moved() {
        let rows = [row(&[TaskAction::Pause]), row(&[TaskAction::Unpause])];
        let m = status(true, false);
        let c = BatchControl::of(&rows, true, Some(&m));
        assert!(!c.can_pause_all && !c.can_resume_all && c.migrate_running, "{c:?}");
        let text = c.migrate_text.expect("进行中必须有进度话");
        assert!(text.contains("2/5"), "{text}");
        assert!(c.migrate_note.is_none());
    }

    #[test]
    fn a_clean_finish_says_nothing() {
        let m = status(false, true);
        let c = BatchControl::of(&[], true, Some(&m));
        assert!(c.migrate_text.is_none() && c.migrate_note.is_none(), "{c:?}");
        assert!(c.can_resume_all, "搬完了就能开始");
    }

    /// 失败要说清后果与文件在哪；跳过的另说一句。
    #[test]
    fn failures_and_skips_are_reported_with_consequence_and_location() {
        let mut m = status(false, true);
        m.errors = vec![MigrateProblem {
            path: "a/b.fq".to_string(),
            reason: "磁盘已满".to_string(),
        }];
        m.skipped = vec![MigrateProblem {
            path: "c.fq".to_string(),
            reason: "x".to_string(),
        }];
        let note = BatchControl::of(&[], false, Some(&m)).migrate_note.expect("应当有话说");
        assert!(note.contains("从头下载") && note.contains("旧文件仍在原目录"), "{note}");
        assert!(note.contains("a/b.fq") && note.contains("磁盘已满"), "{note}");
        assert!(note.contains("没有覆盖"), "{note}");
    }

    /// 壳这一侧的搬运失败（没能开始 / 中途断了）必须说出来，且与内核那一侧的失败句同一个后果。
    /// 判别力：`with_migrate_failure` 改成什么都不做 ⇒ 第一条红（那就是「静默失败」本身）。
    #[test]
    fn a_migrate_the_shell_could_not_run_is_said_out_loud() {
        let refused = MigrateFailure::Refused("旧目录已经不存在".to_string());
        let c = BatchControl::of(&[], true, None).with_migrate_failure(Some(&refused));
        let note = c.migrate_note.expect("搬运没能开始必须有话说");
        assert!(note.contains("从头下载") && note.contains("旧文件仍在原目录"), "{note}");
        assert!(note.ends_with("原因：旧目录已经不存在"), "{note}");

        let quiet = BatchControl::of(&[], true, None).with_migrate_failure(None);
        assert!(quiet.migrate_note.is_none(), "没有失败就不许凭空说话");
    }

    /// 取不到搬运进度：话是那一句原文；与结束话并排时用空格分开。
    /// 判别力：去掉 `ProgressLost` 那一支（或改了句子）⇒ 红。
    #[test]
    fn losing_the_progress_says_so_verbatim_and_sits_beside_the_finished_note() {
        let lost = MigrateFailure::ProgressLost;
        let alone = BatchControl::of(&[], true, None).with_migrate_failure(Some(&lost));
        assert_eq!(alone.migrate_note.as_deref(), Some("没能取到搬运进度，旧文件可能没有搬完。"));

        let mut m = status(false, true);
        m.errors = vec![MigrateProblem {
            path: "a.fq".to_string(),
            reason: "磁盘已满".to_string(),
        }];
        let both = BatchControl::of(&[], true, Some(&m)).with_migrate_failure(Some(&lost));
        let note = both.migrate_note.expect("两句都要在");
        assert!(note.contains("从头下载") && note.ends_with("没能取到搬运进度，旧文件可能没有搬完。"), "{note}");
    }

    #[test]
    fn serializes_with_stable_keys() {
        let v = serde_json::to_value(BatchControl::of(&[], false, None)).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "can_pause_all",
                "can_resume_all",
                "migrate_note",
                "migrate_running",
                "migrate_text"
            ]
        );
    }
}
