//! 批次历史那一格的载荷与判据（任务 8 新增：`history_get` / `history_put` 两个命令共用）。
//!
//! 上游：`macos/Sources/BenagenCoreKit/AppModel.swift` 的
//! `loadHistoryIfNeeded` / `recordLoadedBatch` / `setHistoryNote` / `writeHistory`
//! （四个方法）+ `Presentation/BatchHistoryRow.swift`（行的呈现）。
//!
//! ## ⚠️ 本文件是**两个历史类型之间唯一的一座桥**
//!
//! `shell-core` 里有两份历史：
//!
//! | 类型 | 哪来的 | 它是谁的 |
//! |---|---|---|
//! | [`History`]（`storage::history`） | 任务 3 | **磁盘上那一份**：`version` / 上限 50 / 原子写 / 读坏当空（E-1/E-2） |
//! | [`BatchHistory`]（`presentation::batch_history`） | 任务 4 | **判据那一份**：备注优先、去重、排序、`timestamp`（手写 ISO8601 `+08:00`） |
//!
//! 两者的**字段逐字相同**、不变量也相同（任务 3 与任务 4 各自对着 macOS 抄了一遍），
//! 但**它们是两个类型**，而"写一条历史"这件事需要**两边同时在场**（判据在呈现层、
//! 落盘在存储层）。⇒ 桥只在这里造一座（[`as_presentation`] / [`as_storage`] /
//! [`as_presentation_entry`] 三个纯字段搬运函数），**判据一条都不重写**：
//! `recording` 的"备注传 `None` 就保持原有那一句"、空码空操作、`setting_note` 的
//! "不在历史里就是空操作"，全部走 `BatchHistory` 那两个**有测试的**方法。
//!
//! ⚠️ **那座桥曾经有一个"代价"，现在不是了**（审查修订轮 + 控制者裁决）：
//!    `storage::history::History::put`（任务 3 交付的那个"记一条"的公开方法）
//!    一开始被留成了**没有生产调用者**的形状 —— 因为走它就意味着在命令层
//!    重写一遍"备注传 `None` 就保持原有那一句"（`recording` 已经实现过一份）。
//!    控制者的裁决是：**一个"有测试、没有生产调用者"的函数不许留着**
//!    （它正是"测试给人虚假信心"的形状：测试全绿，而线上一次都没跑过）。
//!    ⇒ 它**连同它的用例一起删掉了**（删 3 改 2，判据一条没丢 ——
//!    逐条接手关系记在 `storage/history.rs` 里那个墓碑注释上）。
//!    **本模块因此是"怎么写一条历史"的唯一入口**：桥只搬运，判据在 `BatchHistory`。

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::envelope;
use crate::presentation::batch_history::{BatchHistory, BatchHistoryEntry, BatchHistoryRow};
use crate::storage::history::{History, HistoryEntry};

// ---------------------------------------------------------------------------
// 一次「写历史」的请求
// ---------------------------------------------------------------------------

/// `history_put` 的入参：**两件事，两个变体**。
///
/// ⚠️ **为什么用一个带标签的枚举、而不是一个 `used: bool` 字段**：
///    "又用了一次"与"只改一句备注"在磁盘上是**两件不同的事** ——
///    后者**不许**碰 `last_used_at`（写一句备注不是"又用了一次"：用户刚看着的那一行
///    会从他眼皮底下跳到列表最上面）。一个布尔字段的默认值（`false`）会让
///    "前端忘了带这一格"变成一次**静默的语义降级**（本该更新的时间戳没更新），
///    而枚举在类型上就要求前端**明说**是哪一件事（缺 `kind` 或写错 = 反序列化当场失败）。
///
/// ⚠️ `base_url` / `note` 用 `#[serde(default)]`：它们是**可省**的（"默认服务器"与
///    "不改备注"各有一个明确的语义），而 `code` **没有**默认值 —— 它是主键，
///    缺了就该失败（一个没有码的历史条目在列表里是一个点了没反应的假条目）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryWrite {
    /// **又用了一次这个码**（时间戳记**此刻**）。
    Used {
        code: String,
        /// 空串 = 默认交付服务器（请求里不出现 `base_url` 那个键）。
        #[serde(default)]
        base_url: String,
        /// `None` = **保持这一条原有的备注**（"客户张三"不该因为又用了一次而消失）；
        /// `Some("")` = 显式清空。
        #[serde(default)]
        note: Option<String>,
    },
    /// **只改一句备注**（**不动**时间戳，也不动这个码在列表里的位置）。
    Note {
        code: String,
        /// 新的备注原文（空串 = 清空备注，**不是**删掉这一条）。
        note: String,
    },
}

// ---------------------------------------------------------------------------
// 载荷
// ---------------------------------------------------------------------------

/// `history_get` / `history_put` 的载荷：屏上从上到下的那些行。
///
/// ⚠️ **空历史 ⇒ 空数组**（不是 `null`、也不是一句"还没有记录"）：上游那一侧的判据是
///    "返回空 `Vec` 时调用方整段不渲染"（`BatchHistoryRow::rows` 的文档）—— 列表为空时
///    不显示这一段，而不是给一个空盒子。这句话的落点在前端，壳只负责把空数组发出去。
pub fn payload(history: &History) -> Value {
    envelope::ok(json!({ "rows": rows(history) }))
}

/// 历史 → 屏上那些行（顺序**原样沿用**：排序是历史自己的不变量，见 `BatchHistoryRow::rows`）。
pub fn rows(history: &History) -> Vec<BatchHistoryRow> {
    history
        .entries
        .iter()
        .map(|entry| BatchHistoryRow::of(&as_presentation_entry(entry)))
        .collect()
}

/// 把一次「写历史」的请求应用到内存里那一份上。**判据全在 `BatchHistory` 里**（见模块头）。
///
/// ⚠️ `at_unix_seconds` 由**调用方**给（这一层不读时钟，与 `BatchHistory` 同一条分工）：
///    `history_put(Used)` 传"此刻"，而 `Note` 那一支**根本不看它**。
pub fn apply(history: &History, write: &HistoryWrite, at_unix_seconds: i64) -> History {
    let current = as_presentation(history);
    let updated = match write {
        HistoryWrite::Used {
            code,
            base_url,
            note,
        } => current.recording(code, base_url, note.as_deref(), at_unix_seconds),
        HistoryWrite::Note { code, note } => current.setting_note(note, code),
    };
    as_storage(&updated)
}

/// 「**记录写盘失败**」那句话。
///
/// ⚠️⚠️ **逐字是 macOS 的原文**（`macos/Sources/BenagenCoreKit/AppModel.swift:839` 的
///    `historyWriteFailure = "历史记录写入失败：\(error)"`）—— 控制者裁决 **R-39**：
///    这是一句**用户看得见的常驻提示行文案**，而且是从 macOS 抄过来的，
///    不许因为「历」「史」两个字不在字体子集里就换词（那是**让字体决定文案**，
///    把因果倒过来了）。⇒ 正确的处置是**刷新字体子集**（单独一次改动，
///    见 `windows/assets/ui-subset.otf` 与 `make_font_subset.sh` 的 `SUBSET_SHA256`）。
///    ⚠️ 这一段是**给下一个想换词的人**看的：本仓库里"壳自己写的错误话术"在
///    `storage/mod.rs` 那一侧确实有换词的先例（那一段也如实记着理由），
///    但**凡是与 macOS 有对应句的文案，一律不适用那条**——先看这里再动手。
///
/// 🔴 为什么是壳自己写的：写盘失败是**磁盘**说的，不是内核说的 —— 这件事与内核
///    一点关系都没有（历史文件根本不经过它），所以没有"内核原文"可登。
///    而它**必须有落点**：一次"以为记下来了"的静默失败，用户要等到下次换码时
///    才发现历史里什么都没有（那些码是他手动敲进去的）。对齐 macOS 的
///    `historyWriteFailure`（那边落在一条常驻提示行上；我们这一侧落在失败信封里，
///    由界面决定摆在哪 —— 但**不许**吞掉它）。
pub fn write_failed(cause: &str) -> String {
    format!("历史记录写入失败：{cause}")
}

// ---------------------------------------------------------------------------
// 两个历史类型之间的桥（**唯一**一处）
// ---------------------------------------------------------------------------

/// 存储层那一条 → 呈现层那一条（字段逐字相同，这里是纯搬运）。
fn as_presentation_entry(entry: &HistoryEntry) -> BatchHistoryEntry {
    BatchHistoryEntry {
        code: entry.code.clone(),
        note: entry.note.clone(),
        base_url: entry.base_url.clone(),
        last_used_at: entry.last_used_at.clone(),
    }
}

/// 存储层那一份 → 呈现层那一份。
///
/// ⚠️ 走 `BatchHistory::new`（它自己会排序 / 去重 / 截到 50）——**没有重复判据**：
///    磁盘上那一份在**读进来**的时候已经过了一次同样的规范化（`History` 的
///    `Deserialize` 就是 `History::new`），所以这里是**幂等**的第二遍，结果不变。
fn as_presentation(history: &History) -> BatchHistory {
    BatchHistory::new(history.entries.iter().map(as_presentation_entry).collect())
}

/// 呈现层那一份 → 存储层那一份（反方向，同样是纯搬运）。
fn as_storage(history: &BatchHistory) -> History {
    History {
        entries: history
            .entries
            .iter()
            .map(|entry| HistoryEntry {
                code: entry.code.clone(),
                note: entry.note.clone(),
                base_url: entry.base_url.clone(),
                last_used_at: entry.last_used_at.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条历史（时间戳用**递增**的固定值，免得依赖时钟）。
    fn an_entry(code: &str, note: &str, at: &str) -> HistoryEntry {
        HistoryEntry {
            code: code.to_string(),
            note: note.to_string(),
            base_url: String::new(),
            last_used_at: at.to_string(),
        }
    }

    /// 一个固定的"此刻"（2026-09-18 09:12:00 +08:00 = 1789693920）。
    ///
    /// ⚠️ 它是**算出来的**、不是估的：`2026-09-18T09:12:00+08:00` 就是
    /// UTC 的 `2026-09-18T01:12:00Z`。写下它的时候顺手核了一遍 ——
    /// 猜一个数会让下面那条"时间戳要记成此刻"的断言以**另一种理由**变绿/变红。
    const NOW: i64 = 1_789_693_920;

    fn a_history() -> History {
        History::new(vec![
            an_entry("AAA", "客户张三", "2026-09-18T09:01:00+08:00"),
            an_entry("BBB", "", "2026-09-18T09:02:00+08:00"),
        ])
    }

    /// 行是**倒序**（最近用的在最前），且三样东西齐全（标题 / 时间 / base_url）。
    #[test]
    fn the_rows_keep_the_histories_own_order_and_carry_the_three_cells() {
        let rows = rows(&a_history());
        assert_eq!(
            rows.iter().map(|r| r.code.as_str()).collect::<Vec<_>>(),
            vec!["BBB", "AAA"],
            "顺序必须照那份记录的既有次序来（最近的在前）"
        );
        // 备注非空 ⇒ 标题是备注；空 ⇒ 回落到码（那是这一行的全部意义）。
        assert_eq!(rows[1].title, "客户张三");
        assert_eq!(rows[1].note, "客户张三");
        assert_eq!(rows[0].title, "BBB");
        assert!(rows[0].time_text.contains("2026-09-18"), "{}", rows[0].time_text);
        assert_eq!(rows[0].base_url, "");
    }

    /// 空历史 ⇒ **空数组**（不是 null、也不是一句文案）。
    #[test]
    fn an_empty_history_is_an_empty_array() {
        let v = payload(&History::empty());
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["rows"], json!([]));
    }

    /// ⭐ **`Used`：时间戳记此刻，且不传备注时保持原有的那一句**。
    ///
    /// 判别力：把 `note` 那一格当成"总是覆盖"，第二条断言立刻红 ——
    /// 而真机上的表现是"用户写下的「客户张三」在用了一次之后**消失了**"。
    #[test]
    fn a_use_stamps_the_time_and_keeps_the_existing_note() {
        let history = a_history();
        let updated = apply(
            &history,
            &HistoryWrite::Used {
                code: "AAA".to_string(),
                base_url: String::new(),
                note: None,
            },
            NOW,
        );
        let aaa = updated
            .entries
            .iter()
            .find(|e| e.code == "AAA")
            .expect("AAA 还在");
        assert_eq!(aaa.note, "客户张三", "不传备注 ⇒ 保持原有的那一条");
        assert_eq!(
            aaa.last_used_at, "2026-09-18T09:12:00+08:00",
            "时间戳要记成**此刻**（由调用方给的 Unix 秒格式化而来）"
        );
        assert_eq!(updated.entries.len(), 2, "同码再写是就地更新，条数不变");
        assert_eq!(
            updated.entries[0].code, "AAA",
            "刚用过的那一条要在最前面"
        );
    }

    /// `Used` 显式传空串 = **清空备注**（与"不传"是两件事）。
    #[test]
    fn an_explicit_empty_note_clears_it() {
        let updated = apply(
            &a_history(),
            &HistoryWrite::Used {
                code: "AAA".to_string(),
                base_url: "https://d.example".to_string(),
                note: Some(String::new()),
            },
            NOW,
        );
        let aaa = updated.entries.iter().find(|e| e.code == "AAA").unwrap();
        assert_eq!(aaa.note, "", "空串是显式清空");
        assert_eq!(aaa.base_url, "https://d.example", "base_url 要带上");
    }

    /// ⭐ **`Note`：只改备注，时间戳一格都不许动**（写一句备注不是"又用了一次"）。
    ///
    /// 判别力：把 `setting_note` 换成 `recording`（一个很自然的"顺手复用"），
    /// 这一条立刻红 —— 而真机上的表现是"用户刚看着的那一行从他眼皮底下跳到了最上面"。
    #[test]
    fn a_note_edit_does_not_touch_the_timestamp_nor_the_order() {
        let updated = apply(
            &a_history(),
            &HistoryWrite::Note {
                code: "AAA".to_string(),
                note: "客户张三".to_string(),
            },
            NOW,
        );
        let aaa = updated.entries.iter().find(|e| e.code == "AAA").unwrap();
        assert_eq!(aaa.note, "客户张三");
        assert_eq!(
            aaa.last_used_at, "2026-09-18T09:01:00+08:00",
            "改备注**不许**碰时间戳"
        );
        assert_eq!(
            updated.entries.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec!["BBB", "AAA"],
            "顺序一格都不许动（那一行不该从用户眼前跑掉）"
        );
    }

    /// ⚠️ **对一个不在历史里的码设备注是空操作**（判据在 `BatchHistory::setting_note`）。
    ///
    /// 判别力：改成"没有就造一条"，这里立刻红 —— 而真机上的表现是历史里多出一条
    /// **没有时间戳、点了没反应**的假条目（额头上写着"排在最末"）。
    #[test]
    fn a_note_for_an_unknown_code_is_a_no_op() {
        let history = a_history();
        let updated = apply(
            &history,
            &HistoryWrite::Note {
                code: "ZZZ".to_string(),
                note: "查无此码".to_string(),
            },
            NOW,
        );
        assert_eq!(updated, history, "不该凭空造出一条记录");
    }

    /// ⚠️ 空码的 `Used` 也是空操作（它不是一条交付批次）。
    #[test]
    fn a_use_with_an_empty_code_is_a_no_op() {
        let history = a_history();
        let updated = apply(
            &history,
            &HistoryWrite::Used {
                code: String::new(),
                base_url: String::new(),
                note: None,
            },
            NOW,
        );
        assert_eq!(updated, history);
    }

    /// ⚠️ **上限 50 条那条不变量在写这一侧照样成立**（它是存储层与呈现层共同的不变量）。
    #[test]
    fn the_fifty_entry_cap_survives_a_write() {
        let many: Vec<HistoryEntry> = (0..55)
            .map(|i| an_entry(&format!("E{i:02}"), "", "2026-09-18T09:00:00+08:00"))
            .collect();
        let history = History::new(many);
        assert_eq!(history.entries.len(), 50);
        let updated = apply(
            &history,
            &HistoryWrite::Used {
                code: "FRESH".to_string(),
                base_url: String::new(),
                note: None,
            },
            NOW,
        );
        assert_eq!(updated.entries.len(), 50, "写一条之后仍然是最多 50 条");
        assert_eq!(updated.entries[0].code, "FRESH");
    }

    /// 🔴 **写盘失败那句话点名了根因，并且把系统原文照登**。
    #[test]
    fn the_write_failure_names_the_cause_verbatim() {
        // ⚠️ 期望值是**字面量**（macOS 的原文逐字）：这句文案被 R-39 钉住过 ——
        //    它**不许**因为字体子集缺字而换词（换词是让字体决定文案）。
        assert_eq!(
            write_failed("Permission denied (os error 13)"),
            "历史记录写入失败：Permission denied (os error 13)",
            "这句话逐字来自 macOS 的 AppModel.swift:839（R-39）"
        );
        assert!(write_failed("x").contains("失败"), "要说清是写失败这一步：{}", write_failed("x"));
    }

    /// ⚠️ **桥是双向恒等的**（同一份历史过一遍两个方向、字段一格不差）。
    ///
    /// 判别力：任何一个字段在桥里被写串（比如 `note` 与 `base_url` 对调），
    /// 这一条立刻红 —— 而那条路在真机上表现为"备注里出现了网址"。
    #[test]
    fn the_bridge_round_trips_without_losing_a_field() {
        let history = a_history();
        assert_eq!(as_storage(&as_presentation(&history)), history);
    }

    /// 反序列化：`kind` 决定是哪一件事，缺 `code` 或 `kind` 写错**当场失败**（不是静默降级）。
    #[test]
    fn the_write_request_needs_an_explicit_kind_and_a_code() {
        let used: HistoryWrite = serde_json::from_value(json!({"kind": "used", "code": "C24-8"}))
            .expect("最小的一份 Used（base_url / note 都可省）");
        assert_eq!(
            used,
            HistoryWrite::Used {
                code: "C24-8".to_string(),
                base_url: String::new(),
                note: None
            }
        );

        let note: HistoryWrite =
            serde_json::from_value(json!({"kind": "note", "code": "C24-8", "note": "客户张三"}))
                .expect("Note 要带 note");
        assert_eq!(
            note,
            HistoryWrite::Note {
                code: "C24-8".to_string(),
                note: "客户张三".to_string()
            }
        );

        // 三档"没读懂"：没有 kind / kind 不认识 / 没有 code。
        for bad in [
            json!({"code": "C24-8"}),
            json!({"kind": "reveal", "code": "C24-8"}),
            json!({"kind": "used"}),
            json!({"kind": "note", "code": "C24-8"}),
        ] {
            assert!(
                serde_json::from_value::<HistoryWrite>(bad.clone()).is_err(),
                "这一份不该解得出来：{bad}"
            );
        }
    }
}
