//! 参数面板那一格的载荷（任务 8 新增：`settings_get` / `settings_set` 两个命令共用）。
//!
//! 上游：`macos/Sources/BenagenDownloader/Settings/SettingsView.swift`（视图层）+
//! `Presentation/SettingsForm.swift`（判据层）。**本文件一个字都不自己写文案**：
//! 七项标签与区间、`-k` 的枚举面、两句说明、保存条横幅、保存按钮的 help 全部取自
//! [`SettingsForm`] 与 `EngineGate`（那个模块的头注记着"为什么连视图层的三句也要搬"）。
//!
//! ## ⚠️ 为什么这一个载荷要发这么多格（而不是"七个值 + last_code"就够）
//!
//! 规格 §3.2：**JS 不拼接任何面向用户的字符串**。参数面板上有五处**只有 Rust 算得出来**
//! 的东西：
//!
//!   * 七项的**标签**（"并行文件数（-j）"…）与它们的**区间** —— 区间是内核 `validate()`
//!     的口径，标签在上游是视图层的字面量（任务 6 按 R-9 搬进了 `settings_form`）；
//!   * `-k` 的**候选集合**（来自内核的 `hello`）+ 当前值不在集合里时那句说明；
//!   * `0 = 不限速` 与"改动什么时候生效"两句（它们说的是内核语义，不是排版）；
//!   * 保存条横幅（有未保存的改动 / 与内核当前参数一致）；
//!   * 保存按钮的 help（三态）。
//!
//! 少了任何一格，前端**只能自己编**（明禁）或者**什么都不显示**（用户看不出那里有问题）
//! —— `api::state::engine_wire` 的文档记着同一件事的一次真实教训（引擎徽标整颗不渲染，
//! 而**没有任何东西变红**）。
//!
//! ## ⚠️ 横幅与 help 是**成对**发出去的（两种状态各一格）
//!
//! "有没有未保存的改动"是**前端手里那个编辑中的表单**与内核那份的比较结果
//! （用户敲了一半还没点保存时，壳体不知道），所以壳没法替它选一句 —— 但**句子本身**
//! 必须由壳给。⇒ 两种状态各发一格，前端只做**挑选**，不做**造句**。
//!
//! ## ⚠️ 第二个数据来源：`hello` 回执
//!
//! `-k` 的候选集合**只在握手的 `hello` 回执里**（内核的 `get_settings` 不回它）。
//! 而壳把回执**原文**存进了 `SessionView.handshake_reply`（那是"整条架构通了"的证据，
//! 探路 §6 第 3 条）⇒ 这里从它解析，**不另发一次 `hello` 请求**（那会在每次打开设置时
//! 多一条内核往返，而集合在连接的生命周期里不会变）。

use serde_json::{json, Value};

use crate::api::{envelope, to_wire};
use crate::presentation::settings_form::SettingsForm;
use crate::presentation::transfer_row::EngineGate;
use crate::protocol::{HelloResult, SettingsResult};

/// 参数面板的**整份载荷**。
///
/// ⚠️ `current` **两个方向都用**：`settings_get` 给的是内核手里那一份，`settings_set`
///    给的是**写完之后内核回执**那一份（`-k` 被归一化成规范串之后的那份）。
///    两者形状相同（都是 [`SettingsResult`]）是刻意的 —— 前端只需要一条渲染路径。
///
/// ⚠️ `last_code` 照样发出去（规格 §3.4 那张表点名"七项参数 + `last_code`"）：它是
///    "上次用的码"，界面上那一格读它。**它是运行状态、不是用户参数**（`settings_form`
///    的头注记着"为什么它不在那里"）。
pub fn payload(current: &SettingsResult, handshake_reply: Option<&str>) -> Value {
    let form = SettingsForm::new(
        &current.settings,
        min_split_size_choices(handshake_reply),
    );
    envelope::ok(json!({
        // 七个字段用 `Settings` 自己编码（字段名就是线上键名，见 `protocol.rs`）。
        "settings": to_wire(&current.settings),
        "last_code": current.last_code,
        "parameters": to_wire(&SettingsForm::parameters()),
        "min_split_size_options": to_wire(&form.min_split_size_options()),
        "min_split_size_note": form.min_split_size_note(),
        "notes": {
            "limit_mbps": SettingsForm::LIMIT_MBPS_NOTE,
            "apply": SettingsForm::APPLY_NOTE,
        },
        "banner": {
            "unsaved": SettingsForm::UNSAVED_BANNER,
            "clean": SettingsForm::CLEAN_BANNER,
        },
        // ⚠️ 三格都取**既有的**那一个函数（`save_help` 是三态判据，本文件不重写它）：
        //    前两格传 `engine_allows_actions = true` 是**故意的** —— 引擎不可用的那一档
        //    由前端按 `state().allows_requests` 现判（引擎态一秒一变，写死一份会过期），
        //    这里只把**那句话**交给它。第三格直接给 `EngineGate::UNAVAILABLE_HELP`
        //    （`save_help` 也是在那一档返回它，两处指的是同一句）。
        "save_help": {
            "unsaved": SettingsForm::save_help(true, true),
            "clean": SettingsForm::save_help(true, false),
            "engine_unavailable": EngineGate::UNAVAILABLE_HELP,
        },
    }))
}

/// 从 `hello` 回执的**原文**里取 `-k` 的候选集合。
///
/// ⚠️ **解不出来时回空集合，而不是失败**：空集合下
///    [`SettingsForm::min_split_size_options`] 会把当前值追加进去（那是它的既有一档），
///    界面照样显示得出"现在用的是哪个"，并给出那句说明。把"壳解不动回执"升成一次失败
///    会更糟：**整个参数面板都打不开**，而缺的只是 `-k` 那一格的候选项。
///    （回执的形状由 `protocol.rs` 的 `HelloResult` 钉着；解不动只可能是内核改了协议。）
pub fn min_split_size_choices(handshake_reply: Option<&str>) -> Vec<String> {
    handshake_reply
        .and_then(|raw| serde_json::from_str::<HelloResult>(raw).ok())
        .map(|hello| hello.min_split_size_choices)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Settings;

    /// 内核那一份的夹具（值取内核的 `default_settings()`，`core/src/settings.rs`）。
    fn a_receipt(min_split_size: &str, last_code: &str) -> SettingsResult {
        SettingsResult {
            settings: Settings {
                parallel: 8,
                connections: 16,
                splits: 16,
                min_split_size: min_split_size.to_string(),
                limit_mbps: 0,
                max_tries: 3,
                retry_wait: 1,
            },
            last_code: last_code.to_string(),
        }
    }

    /// `hello` 回执的**原文**（`Connected.handshake_reply` 存的就是这个形状：
    /// `call` 回的是 `result` 那一格，不是整条响应行）。
    fn a_handshake(choices: &[&str]) -> String {
        serde_json::json!({
            "protocol": 1,
            "min_split_size_choices": choices,
        })
        .to_string()
    }

    /// 七个值 + `last_code` 都发出去了（表里点名的那两样）。
    #[test]
    fn the_seven_values_and_the_last_code_are_both_wired() {
        let v = payload(&a_receipt("20M", "C24-8"), None);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["last_code"], json!("C24-8"));
        // ⚠️ **键名逐个点名**：`Settings` 的字段名就是线上键名，写成 camelCase
        //    在界面上是"某一格永远显示默认值"，而那不会报错。
        for (key, want) in [
            ("parallel", json!(8)),
            ("connections", json!(16)),
            ("splits", json!(16)),
            ("min_split_size", json!("20M")),
            ("limit_mbps", json!(0)),
            ("max_tries", json!(3)),
            ("retry_wait", json!(1)),
        ] {
            assert_eq!(v["data"]["settings"][key], want, "settings.{key} 不对：{v}");
        }
    }

    /// 🔴 **标签与区间必须由 Rust 发出去**（规格 §3.2：JS 不许自己造）——
    /// 少了它，前端只能把七行中文标签写死在 JS 里，而那正是这条纪律要防的第二个真相源。
    #[test]
    fn the_labels_and_ranges_come_from_the_settings_form() {
        let v = payload(&a_receipt("20M", ""), None);
        let parameters = v["data"]["parameters"]
            .as_array()
            .expect("parameters 必须是数组");
        assert_eq!(parameters.len(), 7, "七项，一个不多一个不少：{v}");
        assert_eq!(parameters[0]["label"], json!("并行文件数（-j）"));
        assert_eq!(parameters[3]["label"], json!("最小分片大小（-k）"));
        assert_eq!(parameters[0]["min"], json!(1));
        assert_eq!(parameters[0]["max"], json!(64));
        // `-k` 那一项**没有数值区间**（取值面来自内核的 hello）⇒ 两格都是 null。
        assert!(parameters[3]["min"].is_null(), "`-k` 不该有数值下界：{v}");
        assert!(parameters[3]["max"].is_null(), "`-k` 不该有数值上界：{v}");
    }

    /// `-k` 的候选集合来自**握手的回执原文**，且**当前值不在集合里时会追加进去**
    /// （判据在 `SettingsForm::min_split_size_options`，本文件不重写它）。
    #[test]
    fn the_choices_come_from_the_handshake_reply() {
        let reply = a_handshake(&["1M", "20M", "100M"]);
        let v = payload(&a_receipt("20M", ""), Some(&reply));
        assert_eq!(
            v["data"]["min_split_size_options"],
            json!(["1M", "20M", "100M"]),
            "候选集合必须来自内核的 hello（顺序也照它）：{v}"
        );
        assert!(
            v["data"]["min_split_size_note"].is_null(),
            "在集合里就没什么可说的：{v}"
        );

        // 当前值不在集合里 ⇒ 追加在末尾，并给出那句说明。
        let v = payload(&a_receipt("21M", ""), Some(&reply));
        assert_eq!(
            v["data"]["min_split_size_options"],
            json!(["1M", "20M", "100M", "21M"])
        );
        let note = v["data"]["min_split_size_note"]
            .as_str()
            .expect("不在集合里时必须有一句说明");
        assert!(note.contains("21M"), "说明里要点名那个值：{note}");
    }

    /// ⚠️ **回执解不出来（或还没连上内核）⇒ 空集合，不是失败**：整个参数面板照旧打得开。
    ///
    /// 判别力：把 `unwrap_or_default()` 改成 `expect`（或让它失败），这一条立刻红；
    /// 而真机上的表现是"打开设置就报错"—— 用户连改都没得改（明明只有 `-k` 那一格缺信息）。
    #[test]
    fn an_unreadable_handshake_reply_leaves_the_choices_empty_instead_of_failing() {
        for reply in [None, Some(""), Some("{not json"), Some(r#"{"protocol":1}"#)] {
            let v = payload(&a_receipt("21M", ""), reply);
            assert_eq!(v["ok"], json!(true), "回执读不出来不该让整个设置屏失败：{v}");
            assert_eq!(
                v["data"]["min_split_size_options"],
                json!(["21M"]),
                "空集合 ⇒ 只有当前值（`min_split_size_options` 的既有兜底）：{v}"
            );
        }
    }

    /// 🔴 **那五句文案全都由壳给**（前端只挑不造句）——逐字钉住，格名也钉住。
    ///
    /// 判别力：把某一格改成一句话（或把两格发成同一句），这里立刻红；
    /// 而真机上的表现是"保存条上那句横幅不见了"，而**没有任何东西会变红**。
    #[test]
    fn every_sentence_the_panel_needs_is_shipped_by_the_shell() {
        let v = payload(&a_receipt("20M", ""), None);
        let data = &v["data"];
        assert_eq!(data["notes"]["limit_mbps"], json!("0 = 不限速（不限制总下载速度）"));
        assert!(
            data["notes"]["apply"]
                .as_str()
                .unwrap_or_default()
                .contains("即时生效"),
            "生效时机那句是内核语义（改了会以为正在传的任务变了）：{v}"
        );
        // 横幅两格**必须不同**（同一个状态说两句一样的话 = 用户看不出有没有改动）。
        assert_eq!(data["banner"]["unsaved"], json!("有未保存的改动"));
        assert_eq!(data["banner"]["clean"], json!("与内核当前参数一致"));
        assert_ne!(data["banner"]["unsaved"], data["banner"]["clean"]);
        // save_help 三格：前两格取 `save_help`，第三格与 `EngineGate::UNAVAILABLE_HELP` 同源。
        assert_eq!(
            data["save_help"]["unsaved"],
            json!("把七项参数下发给内核（set_settings）")
        );
        assert_eq!(
            data["save_help"]["clean"],
            json!("与内核当前参数一致：没有要保存的改动")
        );
        assert_eq!(
            data["save_help"]["engine_unavailable"],
            json!(EngineGate::UNAVAILABLE_HELP)
        );
    }

    /// 七个值**两两不同**的那一份也要原样发出去（免得只看 `parallel` 一格就以为对了）。
    #[test]
    fn a_receipt_with_distinct_values_is_carried_verbatim() {
        let mut receipt = a_receipt("21M", "R-9");
        receipt.settings.parallel = 3;
        receipt.settings.connections = 4;
        receipt.settings.splits = 5;
        receipt.settings.limit_mbps = 100_000;
        receipt.settings.max_tries = 7;
        receipt.settings.retry_wait = 60;
        let v = payload(&receipt, None);
        assert_eq!(
            v["data"]["settings"],
            json!({
                "parallel": 3, "connections": 4, "splits": 5,
                "min_split_size": "21M", "limit_mbps": 100_000,
                "max_tries": 7, "retry_wait": 60,
            }),
            "七个值必须逐格对位（串格在这里就会露出来）：{v}"
        );
    }
}
