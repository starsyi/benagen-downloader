//! `/api/state` 那一格的载荷（搬自 `shell-win/src/server/routes.rs:160` 的 `state_json`、
//! `:187` 的 `engine_wire`、`:204` 的 `load_wire`）。
//!
//! 前端第一屏就靠它（还没有内核时也要答得出来）—— 它是**轮询**的那一格
//! （规格 §3.5）：图标、横幅、进度、`/api/load` 的异步结果全落在这里。

use serde_json::Value;

use crate::api::{envelope, to_wire};
use crate::presentation::delivery_summary::DeliverySummary;
use crate::presentation::engine_status::EngineStatusPresentation;
use crate::presentation::transfer_row::{EngineBanner, EngineGate};
use crate::protocol::{EngineState, LoadState};
use crate::session_view::SessionView;

/// `/api/state` 的全部内容（信封已套好，见 `api/mod.rs` 文件头）。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:160` 的 `state_json`。
/// **对齐 macOS**：那几格就是 `AppModel` 的 `engine`（`AppModel.swift:62`）、
/// `loadState`（`:61`）、`lastError`（`:192`）—— 末两格（`banner` / `allows_requests`）
/// 由 `Presentation/EngineStatusPresentation.swift`（**已移植**：
/// `presentation/engine_status.rs` 与 `presentation/transfer_row.rs` 的
/// `EngineBanner` / `EngineGate`）算出。
/// ⚠️ `fallback_notice` 那一格 **macOS 侧没有对应物**：它是 Windows 壳的 W-2 披露
///    （"这次用的不是内嵌那一份内核"），由 `shell-win` 的 `main.rs` 写。
/// ⚠️ `handshake_reply` 同样**没有 macOS 对应物**：SwiftUI 那一代不需要把"整条架构通了"
///    的证据摆给用户看，web 这一代的诊断区需要（探路 §6 第 3 条）。
///
/// ⚠️ **第二个参数就是 `view.handshake_reply`**（计划任务 7 的调用点逐字：
///    `state(&view, view.handshake_reply.as_deref())`）—— 冗余是这个签名本来的样子，
///    不是笔误。这里**以参数为准**（`view.handshake_reply` 在函数体内不再被读）：
///    一个值只从一个地方取，免得两个来源将来各说各话。
pub fn state(view: &SessionView, handshake: Option<&str>) -> Value {
    envelope::ok(serde_json::json!({
        "engine": engine_wire(&view.engine),
        "load": load_wire(&view.load),
        "last_error": &view.last_error,
        "fallback_notice": &view.fallback_notice,
        "handshake_reply": handshake,
        // ⚠️ 顶部横幅是**成句** —— "哪个来源优先、要不要给「重试」、握手超时那句补充
        //    说明"三条都是判据（`EngineBanner::of` 里写着）。前端零业务逻辑
        //    （规格约束 10）⇒ 那句话只能在这一侧成形。`None` = 此刻没话说。
        "banner": to_wire(&EngineBanner::of(&view.engine, view.last_error.as_deref())),
        // ⚠️ 同上：传输列表那 200 ms 轮询的闸（规格 §6.3"引擎不可用一拍都不发"）
        //    走的是 `EngineGate` —— 让 JS 自己判就等于把那道判据抄进前端。
        "allows_requests": EngineGate::allows_requests(&view.engine),
    }))
}

/// `EngineState` → **线上形状**。
///
/// ⚠️ **为什么不直接 `serde_json::to_value(engine)`**：`EngineState` 定义在
///    `crate::protocol`，那里**没有**给它派生 `Serialize`（它是内部状态，
///    不是线上消息）。往 `protocol` 加派生会改一个 232 条测试覆盖着的类型的公开面，
///    而这里需要的只是**一个显式的映射**。
/// ⚠️ **手写而不是派生**的理由与 `envelope.rs` 同一条：这是**线上契约**，
///    派生会跟着字段改名一起漂，而契约变了前端只会以"某个字段是 undefined"收场（静默）。
///    改这里任何一个字符串，都要先改这条注释。
///
/// ## ⚠️ 本函数发的是**两件事**，别把它们当成一件
///
///   * `kind`（+ `unavailable` 那一档的 `reason`）是**"是哪个状态"**——线上**契约**，
///     手写在下面那个 `match` 里（改一个字符串要先改上面那段注释）；
///   * `text` / `icon` / `tooltip` 是**"这个状态长什么样"**——它们**一律取自**
///     `presentation/engine_status.rs`（13 条测试钉着它，逐字对位 macOS 的
///     `EngineStatusPresentation.swift`），**本函数一个字都不自己写**。
///
/// 🔴 **那三格是补上的，而且它们曾经真的缺着**（任务 9 的前端骨架交付时暴露的）：
///    前端那颗引擎徽标（工具栏末尾）要显示的正是 `EngineStatusPresentation::{text,
///    icon_name, tooltip}` 三样，而本函数当时只发 `{kind, reason}` ⇒ **徽标整颗不渲染**。
///    它**不会让任何东西变红**（空着不等于报错），所以缺口一直没被发现 ——
///    这正是规格 §3.2（"JS 不拼接任何面向用户的字符串"）要说的事：
///    漏发一格呈现值的后果不是"前端少显示一点"，而是**前端只能自己造一个**（明禁），
///    或者**什么都不显示**（用户看到一颗空徽标，与"没画"无法区分）。
///
/// ⚠️ **两条路都走不得**（任务 9 的报告里逐条记着）：
///    · 在 JS 里照抄一遍那个 `match` ⇒ 与 macOS 的文案从那一刻起各走各的；
///    · 拿 `banner.text` 顶上 ⇒ 值**不一样**：横幅给的是**内核原文本身**，
///      徽标给的是"引擎不可用：<原文>"，而握手超时那一档两边都有特例。
///      显示一个"看起来对、其实不是那一句"的字符串，比空着更坏。
///
/// ⚠️ **`Unavailable` 的两格一个字都没加工**：`text` 是**折行版**、`tooltip` 是**原文**
///    （含换行）—— 折行是排版的事，判据在 `EngineStatusPresentation::single_line`
///    里（约束 3 那唯一一处让步）。别在这里"顺手"再折一次或 un-fold 一次。
///
/// ⚠️ **握手超时那一档有它自己的话**：`text` 就是 `HANDSHAKE_TIMEOUT_MESSAGE` 本身
///    （**不套**「引擎不可用：」前缀 —— 它本身就是一句完整的话），`tooltip` 是壳写的
///    那条补充（"内核可能正卡在系统授权对话框上…"）。分档靠
///    `EngineStatusPresentation::is_handshake_timeout`（**逐字相等**，不是包含），
///    本函数不重写那份判断。
fn engine_wire(engine: &EngineState) -> Value {
    use crate::protocol::EngineState::*;
    // ⚠️ `kind` 是本函数手写的那一半（线上契约）；`reason` **只在 `Unavailable` 那一档
    //    才有这个键**（另外三档的线上形状里根本没有它 —— 多一个 `"reason": null`
    //    就是给前端多一条要读的分支）。
    let mut wire = serde_json::Map::new();
    wire.insert(
        "kind".to_string(),
        serde_json::json!(match engine {
            Connecting => "connecting",
            NotStarted => "not_started",
            Running => "running",
            Unavailable(_) => "unavailable",
        }),
    );
    if let Unavailable(reason) = engine {
        // ⚠️ `reason` 是**内核原文照登**（约束 3）：壳不加前缀、不折行 ——
        //    折行与判"是不是握手超时"都是**呈现层**的事（`EngineStatusPresentation`）。
        wire.insert("reason".to_string(), serde_json::json!(reason));
    }
    // ---- 呈现值那一半：**全部来自 `presentation/engine_status.rs`** -------------
    wire.insert(
        "text".to_string(),
        serde_json::json!(EngineStatusPresentation::text(engine)),
    );
    wire.insert(
        "icon".to_string(),
        serde_json::json!(EngineStatusPresentation::icon_name(engine)),
    );
    wire.insert(
        "tooltip".to_string(),
        serde_json::json!(EngineStatusPresentation::tooltip(engine)),
    );
    Value::Object(wire)
}

/// `LoadState` → **线上形状**（理由同上）。
///
/// ⚠️ `Loaded` 那一格发的是 `DeliverySummary`（**界面值**），不是内核的 `DeliveryInfo`
///    —— 第六版裁定（见 `api/mod.rs` 文件头）。`DeliverySummary` 里有 `code` / `files_text` /
///    `size_text` / `validity_text` / `expired_badge_text` 五格，已经是"给客户看的样子"。
fn load_wire(load: &LoadState) -> Value {
    match load {
        LoadState::Idle => serde_json::json!({ "kind": "idle" }),
        LoadState::Loading => serde_json::json!({ "kind": "loading" }),
        LoadState::Loaded(info) => serde_json::json!({
            "kind": "loaded",
            "summary": to_wire(&DeliverySummary::of(info)),
        }),
        LoadState::Failed(message) => serde_json::json!({ "kind": "failed", "message": message }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DeliveryInfo, TreeNode};

    fn a_view() -> SessionView {
        SessionView {
            engine: EngineState::Connecting,
            load: LoadState::Idle,
            last_error: None,
            fallback_notice: None,
            handshake_reply: None,
        }
    }

    fn an_info(code: &str) -> DeliveryInfo {
        DeliveryInfo {
            code: code.to_string(),
            page_url: format!("https://d.example/{code}/index.html"),
            base_url: "https://d.example".to_string(),
            created_at: "2026-09-18T09:12:00+08:00".to_string(),
            expires_at: "2026-10-18T09:12:00+08:00".to_string(),
            expired: false,
            total_files: 2,
            total_bytes: 2048,
            tree: TreeNode::Empty,
        }
    }

    /// 没有内核时 `/api/state` 仍要能答（前端第一屏就靠它）。
    #[test]
    fn a_session_without_a_kernel_still_answers() {
        let v = state(&a_view(), None);
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["data"]["engine"]["kind"], serde_json::json!("connecting"));
        assert_eq!(v["data"]["load"]["kind"], serde_json::json!("idle"));
        assert_eq!(v["data"]["handshake_reply"], serde_json::Value::Null);
    }

    /// ⚠️ **`Unavailable` 的 `reason` 是内核原文照登**（约束 3）：壳不加前缀、不折行。
    /// 这条抓的是"顺手在壳里把原文洗一遍"（折行、加「引擎不可用：」前缀都是呈现层的事）。
    #[test]
    fn an_unavailable_engine_reports_the_reason_verbatim() {
        let raw = "内核进程已退出\n第二行";
        let mut view = a_view();
        view.engine = EngineState::Unavailable(raw.to_string());
        let v = state(&view, None);
        assert_eq!(v["data"]["engine"]["kind"], serde_json::json!("unavailable"));
        assert_eq!(v["data"]["engine"]["reason"], serde_json::json!(raw));
    }

    /// 三个**非** `Unavailable` 的引擎态各有一条线（少一条就有一种改法不会变红）。
    #[test]
    fn the_other_three_engine_states_have_their_own_wire_kinds() {
        for (state_, want) in [
            (EngineState::Connecting, "connecting"),
            (EngineState::NotStarted, "not_started"),
            (EngineState::Running, "running"),
        ] {
            let mut view = a_view();
            view.engine = state_;
            assert_eq!(state(&view, None)["data"]["engine"]["kind"], serde_json::json!(want));
        }
    }

    /// `Loaded` 那一格发的是**界面值** `DeliverySummary`（五格），不是内核的 `DeliveryInfo`。
    #[test]
    fn a_loaded_batch_is_wired_as_the_presentation_summary() {
        let mut view = a_view();
        view.load = LoadState::Loaded(an_info("C24-8"));
        let v = state(&view, None);
        assert_eq!(v["data"]["load"]["kind"], serde_json::json!("loaded"));
        let summary = &v["data"]["load"]["summary"];
        assert_eq!(summary["code"], serde_json::json!("C24-8"));
        // ⚠️ 这五格是 `DeliverySummary` 的全部字段：少一格或改名 ⇒ 前端某一栏空白。
        for key in ["code", "files_text", "size_text", "validity_text", "expired_badge_text"] {
            assert!(summary.get(key).is_some(), "summary 缺了 {key} 这一格：{summary}");
        }
        // 内核的 `DeliveryInfo` 那些字段**不许**漏出来（约束 1：前端不知道内核协议的形状）。
        assert!(summary.get("page_url").is_none(), "内核字段漏给前端了：{summary}");
        assert!(summary.get("total_bytes").is_none(), "内核字段漏给前端了：{summary}");
    }

    /// 加载失败那一格是**内核原文逐字**（约束 3），且形状与成功那几格同一条口径。
    #[test]
    fn a_failed_load_carries_the_message_verbatim() {
        let raw = "内核无响应（等待超过 5 秒）\n第二行";
        let mut view = a_view();
        view.load = LoadState::Failed(raw.to_string());
        let v = state(&view, None);
        assert_eq!(v["data"]["load"]["kind"], serde_json::json!("failed"));
        assert_eq!(v["data"]["load"]["message"], serde_json::json!(raw));
    }

    /// 🔴 **引擎徽标那三格必须由 Rust 算好发出去**（规格 §3.2：JS 不拼接任何面向用户的字符串）。
    ///
    /// ⚠️ **这三格曾经真的缺着**（任务 9 的前端骨架交付时暴露的）：前端那颗徽标
    ///    读的是 `engine.text` / `engine.tooltip` / `engine.icon`，而 `engine_wire`
    ///    当时只发 `{kind, reason}` ⇒ **徽标整颗不渲染，且没有任何东西变红**。
    ///    这条用例就是那个缺口的网。
    ///
    /// 判别力（每一档都会红）：
    ///   * 少发任何一格 ⇒ `get(...)` 是 `None`（下面那条 `is_string` 断言）；
    ///   * 把某一格串了（`text` 那一格发成 `icon_name` 之类）⇒ 与字面量对不上；
    ///   * 徽标拿 `banner.text` 顶上 ⇒ 也红（横幅给的是**内核原文本身**，
    ///     徽标给的是"引擎不可用：<原文>"，两句不一样）。
    ///
    /// ⚠️ 期望值是**字面量**、不是再算一遍 `EngineStatusPresentation::*`：
    ///    调用那两个函数来算期望的话，把 `engine_wire` 里的 `text` 换成 `tooltip`
    ///    这类"串格"**不会有任何东西变红**（两边一起错）。
    ///    这四个字面量同时钉住"呈现层那 13 条测试钉的东西真的走到了线上"。
    #[test]
    fn the_engine_badge_cells_are_computed_here_not_by_the_frontend() {
        for (state_, text, icon) in [
            (EngineState::Connecting, "正在连接内核…", "questionmark.circle"),
            (EngineState::NotStarted, "引擎未启动", "circle.dashed"),
            (EngineState::Running, "运行中", "bolt.fill"),
            (
                EngineState::Unavailable("内核崩了".to_string()),
                "引擎不可用：内核崩了",
                "exclamationmark.triangle.fill",
            ),
        ] {
            let mut view = a_view();
            view.engine = state_;
            let engine = &state(&view, None)["data"]["engine"];
            assert_eq!(engine["text"], serde_json::json!(text), "{engine}");
            assert_eq!(engine["icon"], serde_json::json!(icon), "{engine}");
            // tooltip 那一格**恒在**（四个态都有话说）——少了它徽标就没有 `.title`。
            assert!(
                engine["tooltip"].is_string(),
                // ⚠️ 措辞按字体子集的覆盖判据挑过字（`test.sh` 第 0.5 步：
                //    这条消息里的每个非 ASCII 字都必须在 `windows/assets/ui-subset.otf` 里）。
                "tooltip 那一格没有发出来（会缺 .title）：{engine}"
            );
        }
    }

    /// ⚠️ **握手超时那一档在徽标上说的是它自己的那句话**（不套「引擎不可用：」前缀），
    ///    而 `reason` 那一格照旧是原文 —— 两个消费者各取各的，谁都不许改。
    ///
    /// 判别力：
    ///   * 把分档去掉（让 `text` 走"引擎不可用：<原文>"那一支）⇒ 第一条断言红；
    ///   * 把 `tooltip` 也换成折行版原文 ⇒ 第二条断言红（那正是"该去点允许"那句补充
    ///     消失的形态：用户看着"内核无响应"却不知道下一步做什么）。
    #[test]
    fn the_handshake_timeout_keeps_its_own_wording_on_the_badge() {
        use crate::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;

        let mut view = a_view();
        view.engine = EngineState::Unavailable(HANDSHAKE_TIMEOUT_MESSAGE.to_string());
        let engine = &state(&view, None)["data"]["engine"];

        assert_eq!(
            engine["text"],
            serde_json::json!(HANDSHAKE_TIMEOUT_MESSAGE),
            "握手超时的正文必须**就是那句话本身**（不套前缀）：{engine}"
        );
        assert_eq!(
            engine["tooltip"],
            serde_json::json!("内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。\
                               请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"),
            "握手超时的补充提示是壳写的那一条（可操作的那半句）：{engine}"
        );
        assert_eq!(
            engine["icon"],
            serde_json::json!("exclamationmark.triangle.fill")
        );
        // ⚠️ `reason` 那一格**一个字都没被改写**（约束 3）：它是判别式的输入，
        //    也是横幅那句"引擎不可用"的来源。两格各说各的。
        let reason = engine["reason"].as_str().expect("reason 那一格在");
        assert!(
            EngineStatusPresentation::is_handshake_timeout(reason),
            "`reason` 被加工过了 ⇒ 判别式（逐字相等）就认不出它了：{reason}"
        );
    }

    /// ⚠️ **`text` 是折行版、`tooltip` 是原文**（约束 3 在排版上的唯一让步）：
    ///    正文只有一行，提示放得下换行 ⇒ 一个字符都不许动。
    ///
    /// 判别力：把两格发成同一份（"顺手"让 tooltip 也用折行版、或让 text 直接用原文）
    /// 这一条立刻红 —— 而真机上的表现是"那行提示里出现了换行"（版面被撑开），
    /// 或者"正文被截断成半句话"。
    #[test]
    fn the_badge_text_folds_newlines_while_the_tooltip_keeps_them() {
        let raw = "找不到内核可执行文件（找过：\n/a\n/b）";
        let mut view = a_view();
        view.engine = EngineState::Unavailable(raw.to_string());
        let engine = &state(&view, None)["data"]["engine"];

        let text = engine["text"].as_str().expect("text 那一格在");
        let tooltip = engine["tooltip"].as_str().expect("tooltip 那一格在");
        assert!(!text.contains('\n'), "正文里不许有换行：{text:?}");
        assert_eq!(tooltip, raw, "提示里放得下换行 ⇒ 原文一个字符都不许动");
    }

    /// ⚠️ **`last_error` 与 `fallback_notice` 是两格**（W-2）：合成一格会让"退回了同目录那份内核"
    /// 这条披露被一次成功请求抹掉（`SessionView` 的文档记着那次实测）。
    #[test]
    fn the_transient_error_and_the_sticky_notice_stay_in_two_slots() {
        let mut view = a_view();
        view.last_error = Some("旧的瞬时错误".to_string());
        view.fallback_notice = Some("退回了同目录那份内核".to_string());
        let v = state(&view, None);
        assert_eq!(v["data"]["last_error"], serde_json::json!("旧的瞬时错误"));
        assert_eq!(v["data"]["fallback_notice"], serde_json::json!("退回了同目录那份内核"));
    }

    /// ⚠️ **`handshake_reply` 取的是参数那一份**（见函数文档）：它是诊断区唯一保留的自报项，
    /// "整条架构通了"就是靠这一行证明的（探路 §6 第 3 条）。
    #[test]
    fn the_handshake_reply_comes_from_the_argument() {
        let v = state(&a_view(), Some(r#"{"proto":1,"ok":true}"#));
        assert_eq!(v["data"]["handshake_reply"], serde_json::json!(r#"{"proto":1,"ok":true}"#));
    }

    /// 横幅与轮询闸也在这一格里（前端零业务逻辑，约束 10）。
    #[test]
    fn the_banner_and_the_poll_gate_are_computed_here_too() {
        let mut view = a_view();
        view.engine = EngineState::Unavailable("内核没了".to_string());
        let v = state(&view, None);
        // 引擎不可用时横幅**有话要说**（`EngineBanner::of` 的判据），且轮询要停。
        assert!(!v["data"]["banner"].is_null(), "引擎不可用时横幅不该是 null：{v}");
        assert_eq!(v["data"]["allows_requests"], serde_json::json!(false));
    }
}
