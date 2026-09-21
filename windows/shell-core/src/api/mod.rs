//! `api` —— **载荷拼装层**（规格 §5.3）。
//!
//! 这一层是第二代 `shell-win/src/server/routes.rs` 里那批"把内核数据变成
//! **给前端的 JSON**"的函数搬过来的落点。搬的时候只改一件事：**入参从 `&Session`
//! 换成纯数据**（`SessionView` / `Breadcrumb` / `TransferRow` …）—— 于是"前端收到的东西"
//! 整条契约都能在 **macOS 上单测**（规格 §4.3），而命令层退化成
//! 「取数据 → 调纯函数 → 返回」三步（规格 §5.3 逐字）。
//!
//! ## ⚠️ 本层每一个 `pub fn` 返回的都是**完整的响应信封**，不是裸载荷
//!
//! 判据是规格 §5.3 那句"命令层退化成**三步**"，以及计划任务 7 的调用点逐字：
//!
//! ```ignore
//! pub fn state(app: tauri::State<'_, Shell>) -> serde_json::Value {
//!     let view = app.session.view();
//!     shell_core::api::state::state(&view, view.handshake_reply.as_deref())
//! }
//! ```
//!
//! 那个函数**直接把本层的结果交回前端**，中间没有第四步去套信封 —— 所以信封必须在
//! 这里套好（`{"ok":true,"data":…}` / `{"ok":false,"error":{"message":…}}`）。
//! 拼装信封的地方**只有一个**：[`envelope`]。
//!
//! ## ⚠️ 第二代那些 HTTP 形状**不搬**（`Resp` / `Req` / 状态码）
//!
//! `routes.rs` 的失败回答带着 403 / 404 / 405 / 400 / 503 这些**状态码**。本代没有
//! HTTP 了（规格 §3.3 逐字："没有监听端口、token、心跳、看门狗、浏览器进程"），
//! 所以状态码没有落点，**失败只剩信封里那句原文**（约束 3：原文逐字、不加工、不截断）。
//! 这是有意的减法，不是漏搬：`Req` 的四个字段（方法/路径/查询串/token）在 Tauri 的
//! `invoke` 里由命令名与参数取代。
//!
//! ## ⚠️ 第二代文件头那张"具名例外"表跟着搬过来了（原文保留）
//!
//! 上游 `routes.rs` 的原则是"`presentation` 算好的界面值 + `to_wire`，**不手写 JSON 投影**"
//! —— 手写投影是**第二份形状知识**，它会与 `shell-core` 漂移，而漂移时不会有任何东西变红。
//! 那条原则有**四处具名例外**，它们随代码一起搬到这里，逐条如下：
//!
//! 1. [`state::engine_wire`] / [`state::load_wire`]：`EngineState` / `LoadState`
//!    **刻意没有** `Serialize` 派生 —— 它们是**状态枚举**、不是界面值（`protocol.rs`
//!    那两段文档写着理由）。所以这两个映射是**唯一**一处手写，
//!    判据是"**改这里任何一个字符串，都要先改那段注释**"。
//!    ⚠️ `load_wire` 的 `Loaded` 那一格**不**手写：里面装的是 `DeliverySummary`（界面值）。
//! 2. `DirLoadFailure`：**曾经**是手写摊开（三个字段逐字写进 `json!`），
//!    第七版修订给它补了 `#[derive(Serialize)]` ⇒ 现在走 `to_wire(&why)`。
//!    **它是 18 个界面值里最后一个拿到派生的**，从此没有"摊开一个界面值的字段"的写法。
//! 3. `"selected": tree.default_selected`（[`tree::whole`]）：**原样透传**的是
//!    **内核给的选择面**，不是壳算出来的界面值 —— 壳根本没算它，只是把它交给前端当
//!    勾选框的初始面。⚠️ 它与"把内核回执的形状漏给前端"不是一回事：
//!    这里透出去的**不是内核协议的一个结构体**，而是一个 `Vec<String>`（路径原文列表），
//!    且它**同时**是 [`crate::presentation::browser_row::SelectionSummary::of`] 的入参
//!    —— 那份摘要才是壳算的界面值。
//!
//! 4. [`tree::row_wire`]（**任务 10 的第 2 轮修复补登记的**）：那一行的线上形状是
//!    "`to_wire(row)` **再补两格、去掉一格**"，而这是**唯一一处**这样的写法。
//!    它必须是例外，理由是 R-35 那条纪律：**界面要显示的东西必须在载荷里** ——
//!    而 `RowStateStyle::label()` / `color()` 是**方法**、不在 `BrowserRow` 的派生形状里，
//!    所以 `state_label` / `state_color` 两格**没有别的地方可以长出来**。
//!    ⚠️ 它**不是**"另写一份形状"，也**不是**"重写一张映射"：
//!      · 那两格的值是**调 `presentation` 的方法取来**的（`RowStateStyle::{label,color}`），
//!        本层一个字都不重写；
//!      · 其余七格（`kind` / `name` / `path` / `children_count` / `size` /
//!        `detail_text` / `source_time_text` / `icon_name`）**全部**来自 `to_wire(row)`；
//!      · 去掉的那一格是 `state`（`Serialize` 派生出来的**变体名** `"Complete"`）——
//!        前端要看的是上面那两格，它**没有任何消费者**（R-35 的另一半：别发没人读的字段）。
//!    ⚠️ **本例外只此一处**：再出现"某一行要补几格"时，先问"那几格该不该由
//!    `presentation` 的值类型直接带上"（比如给那个类型补 `Serialize` 的真字段），
//!    而不是在本层再开一处拼接。
//!
//! 除这四处外，任何新的回执形状都必须是"`presentation` 算好的界面值 + [`to_wire`]"。
//!
//! ⚠️ **`verify::verify` 不是例外**（第 2 轮顺带核过）：它就是 `to_wire(summary)`，
//!    没有一行手写拼接。但**它有一个 R-35 缺口**（与本条第 4 款是同一类问题，
//!    不是同一个修法）：`VerifySummary` 的五个字段里**没有** `headline()` /
//!    `classified_text()` 这两句 —— 它们是**方法**，而任务 12 的校验页要显示它们。
//!    详见任务 10 报告 §8 存疑 14（**本任务没有动它**：那一段 wire 归任务 12）。
//!
//! ## ⚠️ 任务 8 新增的五个模块（`about` / `history` / `preferences` / `reveal` / `settings`）
//!
//! 规格 §3.4 那张表里有九个命令在第二代**根本没有对应端点**（它们是这一代补的缺口，
//! 不是既有能力的搬运：⚠️ `task_action` 与 `reveal` 尤其如此 —— `TransferRow.available_actions`
//! 算得出动作、却没有端点去执行）。那九个命令的载荷拼装与**壳自己写的那些话**
//! （偏好写盘失败 / 记录写盘失败 / 重启没落地 / 显示失败 / 定位不到）按同一条纪律
//! 落在本层：**命令层一个字都不许自己写**（`api::NO_KERNEL` 那一席的文档记着这条纪律的由来）。
//!
//! ⚠️ 那五个模块里的每一个都只在**这一侧**成句：它们的文案全部取自 `presentation::*`
//!    的既有判据（`settings_form` / `app_preferences` / `batch_history` / `about_info`），
//!    或者是一句**没有来源可登**的壳自己的话（磁盘写失败、系统调用失败、内核还没起来）
//!    —— 后者每条都带着"为什么这里可以出现一句中文"的论证。
//!
//! ## ⚠️ `enqueue` 是**补搬**的（R-13）
//!
//! 计划任务 2 的表里没有 `enqueue`（规格 §5.3 的散文点了它的名，两者对不上）——
//! 任务 2 按表办、把它留在第二代路由里。那份路由随第二代传输层一起删掉之后，
//! 它是唯一一处**没有家**的载荷拼装 ⇒ 本任务（任务 7，按控制者裁定 R-13）
//! 把它补成第六个纯函数 [`enqueue::enqueue`]，形状与其余五个逐字一致。
//! 它的模块头记着"第二代那个函数里哪一半没搬过来、为什么"。

pub mod about;
pub mod enqueue;
pub mod envelope;
pub mod history;
pub mod preferences;
pub mod reveal;
pub mod settings;
pub mod state;
pub mod transfers;
pub mod tree;
pub mod verify;

use serde_json::Value;

use crate::presentation::kernel_death::CallFailure;

/// 「还没有内核可问」那句话（**壳自己写的**：没有内核，就没有内核原文可登）。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:157` 的 `NO_KERNEL`（R-13 的第二半）。
/// 它当时是第二代路由文件里的一个私有常量，读者是 `/api/tree`、`/api/enqueue`、
/// `/api/transfers`、`/api/verify` 的"没有内核"那一档（503）与 `/api/load` 的
/// `no_kernel` 回执。**搬到这里的理由是一条纪律**：它是**面向用户的失败文案**，
/// 按规格 §3.2（"JS 不拼接任何面向用户的字符串"，而命令层同样不许自造）
/// 它必须与 [`failure`] 住在同一个家 —— 于是命令层拿到 `None` 时只写
/// `envelope::err(api::NO_KERNEL)`，**一个汉字都不自己写**。
///
/// ⚠️ **文案一个字都没改**（逐字搬）：它必须同时说清**根因**（还没有连上内核）
/// 与**下一步**（点设置里的「重试」），而末句"这条不是内核报的错——内核此刻还没起来"
/// 是给诊断用的：用户会把这句话当成内核的报错去搜，那句话先把它排除掉（W-2）。
/// 钉住这两半的是下面 `mod tests` 里那条 `the_no_kernel_message_says_the_cause_and_the_next_step`。
///
/// ⚠️ 措辞里的「设置」（`NO_KERNEL` 原文如此）与规格 §2.1 第 9 项那个**设置窗口**是同一个
///    落点；本代它的「重试」在设置里（macOS 侧同形）。改这句话之前先确认那一处还在。
pub const NO_KERNEL: &str = "还没有连上内核，请先点设置里的「重试」。\
                             这条不是内核报的错——内核此刻还没起来。";

/// **界面值 → JSON**：本层**唯一**一处 `to_value` 口径。
///
/// ⚠️ **为什么不是 `to_value(x).unwrap()`**：真失败了要回一条**明确的**"这里没有值"
///    （前端拿到 `null` 会显示占位符），而不是把一个正在服务的进程带走
///    （同 `shell-win/src/views/transfers.rs:694` 那条注释的理由）。
///    对界面值它**不可能**失败：它们全是 `String` / `bool` / 整数 / `f64` / 枚举 /
///    `Vec` / `Option`，`serde_json` 对非有限浮点也只会写成 `null`。
///
/// ⚠️ **它在第二代是个宏**（`routes.rs` 的 `to_wire!`），到这里才变回函数：
///    上游那个 crate 的 `[dependencies]` 里只有 `serde_json` 与 `shell-core`（`serde`
///    **不是**它的直接依赖，全局约束 8 只许加三个、早已用完）⇒ 泛型版本的
///    `T: serde::Serialize` 那个 trait 在那边**叫不出来**（`E0433`）。
///    本 crate 的 `serde` 是白名单里的直接依赖（`shell-core/Cargo.toml`）
///    ⇒ 这里用泛型函数即可，宏那种写法没有理由跟着搬。
///
/// ⚠️ **`?Sized` 是承重的**：两个载荷函数收的是 `&[BrowserRow]` / `&[TransferRow]`
///    （切片是**不定长**类型），少了它这两处直接编译不过（`E0277`）。
///
/// ⚠️ **它是私有的**：`api` 的子模块能通过 `use crate::api::to_wire;` 拿到它，
///    本 crate 的其余部分与 `shell-win` 拿不到 —— "唯一的 `to_value` 口径"这句话
///    得由可见性兜着，不然它只是一句注释。
fn to_wire<T: serde::Serialize + ?Sized>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// 一条失败的**统一落点**。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:441` 的 `note_failure`。
/// **对齐 macOS**：`AppModel.onKernelDeath(_:)`（`macos/Sources/BenagenCoreKit/AppModel.swift:1500`）
/// 的分派 + `AppModel.lastError`（同文件 `:192`）那条**非粘滞**落点 —— macOS 那边两件事
/// 写在同一段代码里，这里拆成"本函数（纯）"+"命令层的副作用"，见下面那两条 ⚠️。
///
/// ⚠️ **判据只有 [`CallFailure`] 那一份**（`shell-core`，纯函数、有单测）；
///    本函数只做"认出来并交回去"。
///
/// ⚠️ **本函数是纯的**：第二代那个版本还会顺手写 `session`（内核死亡要翻 `engine` 那一格、
///    其余错误落 `last_error`）。副作用**留在命令层**（那是它唯一能拿到 `Session` 的地方）：
///    本层只回答"这一次动作没成，前端该看到哪句话"。
///    两件事都要做才完整（上游 `absorbing`：吸收 + 重抛）—— 少了副作用那半就是一次静默失效，
///    少了本函数这半就是用户看不到"你刚才那一下没成"。**两条路都要走**。
///
/// ⚠️ 发出去的是 [`CallFailure::text`]（这一屏就地显示的那句话）而**不是**
///    `reason`（带壳那半句前缀的引擎版本）—— 两句话不重复：横幅说"引擎没了"，
///    这一句说"你刚才那一下没成"（第二代 `views/file_browser.rs` 的记账）。
///    原文**逐字、不加工、不截断**（约束 3）。
pub fn failure(why: &CallFailure) -> Value {
    envelope::err(why.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ **失败也走同一条信封**（前端只解一种形状）：`ok=false` + `error.message`，
    ///    且**不带 `data` 键**（`data` 恒在是成功那一半的契约）。
    #[test]
    fn a_failure_is_the_same_envelope_without_a_data_key() {
        let v = failure(&CallFailure::Text("内核原文".to_string()));
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["error"]["message"], serde_json::json!("内核原文"));
        assert!(v.get("data").is_none(), "失败的正文里不该有 data：{v}");
    }

    /// ⚠️ **`NO_KERNEL` 要同时说清根因与下一步**（W-2），而且**不是**内核的报错。
    ///
    /// 判别力（两个方向都会红）：把它改成一句笼统的"出错了"，`contains("内核")` 那一行立刻红；
    /// 把末句（"这条不是内核报的错"）删掉，`contains("不是内核报的错")` 那一行立刻红 ——
    /// 而**没有别的东西会红**（这句话只在"内核还没起来"那一刻出现在用户眼前）。
    ///
    /// ⚠️ 它与第二代那条 `calling_the_kernel_without_one_reports_a_real_reason`
    ///    （断言 `msg.contains("内核")`）是**同一条判据**：那条用例留在第二代的路由测试里、
    ///    已随那份文件删除；判据本身（"这句话没说清根因"）在这里复活，且比原来多钉一半。
    #[test]
    fn the_no_kernel_message_says_the_cause_and_the_next_step() {
        assert!(
            NO_KERNEL.contains("内核"),
            "这句话没说清根因（还没有连上内核）：{NO_KERNEL}"
        );
        assert!(
            NO_KERNEL.contains("重试"),
            "这句话没给出可执行的下一步（点「重试」）：{NO_KERNEL}"
        );
        assert!(
            NO_KERNEL.contains("不是内核报的错"),
            // ⚠️ 措辞按字体子集的覆盖判据挑过字（`test.sh` 第 0.5 步：这条消息里的每个
            //    非 ASCII 字都必须在 `windows/assets/ui-subset.otf` 里，否则判据变红）。
            "这句话没排除掉「这是内核报的错」那条误读（用户会拿去查）：{NO_KERNEL}"
        );
        // 它是**失败**那一支的原文：信封形状与其余失败一致（前端只有一条解包路径）。
        let v = envelope::err(NO_KERNEL);
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["error"]["message"], serde_json::json!(NO_KERNEL), "原文逐字、不加工");
    }

    /// ⚠️ **内核死亡那一档发的是 `text` 不是 `reason`**（见函数文档）：
    ///    `reason` 带壳那半句前缀（`KernelDeath::PREFIX`），是**横幅**那一格的话；
    ///    这一屏要显示的是「你刚才那一下没成」的原文。
    ///    判别力：把 `text` 换成 `reason`，这一条立刻红。
    #[test]
    fn a_kernel_death_reports_the_call_text_not_the_banner_reason() {
        let why = CallFailure::KernelGone {
            reason: "下载引擎已断开：内核进程已退出".to_string(),
            text: "内核进程已退出\n第二行".to_string(),
        };
        let v = failure(&why);
        assert_eq!(
            v["error"]["message"],
            serde_json::json!("内核进程已退出\n第二行"),
            "内核死亡那一档发的必须是 text（这一次动作没成的那句话），不是横幅那句"
        );
    }
}
