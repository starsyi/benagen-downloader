//! **内核调用点** —— 从今天的 egui 视图里原样搬出来的（阶段 A 后端 4/12）**七个**，
//! 加上**任务 8 新补的两个**（`get_settings` / `set_settings`：参数面板那两个方向）。
//!
//! ⚠️ **任务 8 补的那两个与那七个不是同一批东西**（别把它们读成"也是搬过来的"）：
//!    那七个是**既有调用点**的搬家（第二代的路由早就调着同一批函数），而
//!    `get_settings` / `set_settings` 是**本代补的缺口** —— 第二代根本没有设置窗口
//!    （规格 §3.4 那张表里，参数面板那几行全部标着"**无（新增）**"）。
//!    ⇒ 上面的"原样搬"三个字**只适用于那七个**；这两个是新写的，判据照抄
//!    `protocol.rs` 的 `SettingsResult` 与内核的 `op_get_settings`/`op_set_settings`。
//!
//! ## 为什么要有这个模块
//!
//! 换 UI 之前，这七件事的函数**散在四个文件里**（`views/file_browser.rs`、
//! `views/transfers.rs`、`views/verify.rs` 与 `main.rs`），而那些文件这一批要整个删掉
//! ⇒ 调用点必须先搬出来。
//!
//! ⚠️ 搬的时候**只动了一件事**：**失败的形状**统一到 `shell-core` 已有的
//!    `CallFailure` / `DirLoadFailure`（唯一映射）。
//!    ⚠️ **收参一个字都没改** —— 八个来源**本来就都收 `&CoreClient`**，
//!    所以它们的签名与来源**逐字相同**。
//!    （真正收 `&mut ShellApp` 的是那些**视图函数** —— `view` / `poll` / `enter` /
//!    `pump` / `ask_for_tree` / `apply` / `start_enqueue` —— 它们是**调用方**，
//!    不是这几个调用点。别把两者搞混。）
//!    ⚠️⚠️ **初稿这里写的是"这六件事没有各自的函数、每个都收 `&ShellApp`，
//!    搬的时候把收参改成 `&CoreClient`" —— 那两句都是假的。** 出处是**一处错的
//!    行号引用**（差额表第 11 条）：简报把 `transfers.rs:576` 当成了调用点，
//!    而那是 `start_poll(&mut ShellApp)` 的文档注释行，真调用点在 `:610`。
//!    我照那个错行号**推错了整段动机**，而它与计划的其余部分自洽，所以没人起疑。
//!    ⇒ **教训：行号引用也是"编"的一种** —— 引用一个没核对过的位置，
//!    与编造一个数字同样会污染下游。
//!    **一个字都不许重写**——那些函数里的每一句措辞（尤其是"壳解不动内核回执"那几句）
//!    都是复审判过的，重写就是引入第二份措辞。
//!
//! ## ⚠️ 这些函数是**阻塞**的
//!
//! `CoreClient::call` 是同步的，且它内部有**单飞闸门**（`client.rs` 的 `call`，闸门是
//! 函数体第一句 `let _gate = lock(&self.gate);`）⇒
//! 多个线程同时调它是**安全**的（串行排队），但一次 90 秒的 `load_delivery`
//! 会比它后面的请求一起憋住。这与今天的 egui 版行为**相同**，不是本计划引入的退化
//! （全局约束 6）。⚠️ **绝不在持有会话锁的时候调它们**（见 `session.rs`）。

// ⚠️ **这里没有 `use …error_text::error_text;`**（计划正文的清单里有，本计划第五版删掉了）：
//    本模块**一次都没有直接调用**它 —— 内核原文一律经 `CallFailure::of` /
//    `DirLoadFailure::of` / `TransferActionFailure::message_of` 三处**既有**入口取得
//    （它们内部就是 `error_text`，那仍然是唯一映射）。留着它是一条
//    `unused_imports` 告警，而本仓库是**零告警**口径（全局约束 4）。
use shell_core::client::{ClientError, CoreClient};
use shell_core::presentation::breadcrumb::DirLoadFailure;
use shell_core::presentation::kernel_death::{is_no_delivery, CallFailure, KernelDeath};
use shell_core::presentation::settings_form::SettingsForm;
use shell_core::presentation::transfer_row::TransferActionFailure;
use shell_core::protocol::codes;
use shell_core::protocol::{
    DeliveryInfo, EnqueueResult, ListDirResult, Settings, SettingsResult, TaskAction,
    TransferListResult, TreeResult, VerifyStatus,
};

/// 一次 `load_delivery`。
///
/// 规则与上游一致：`base_url` **空串不带这个键**；内核报错走 `error_text`（唯一映射）。
pub fn load_delivery(
    client: &CoreClient,
    code: &str,
    base_url: &str,
) -> Result<DeliveryInfo, CallFailure> {
    let mut params = serde_json::json!({ "code": code });
    if !base_url.is_empty() {
        params["base_url"] = serde_json::json!(base_url);
    }
    let value = client.call("load_delivery", params).map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("load_delivery", "DeliveryInfo", &e)))
}

/// 取整棵树（`flat` / `default_selected` / `progress`）。
pub fn get_tree(client: &CoreClient) -> Result<TreeResult, CallFailure> {
    let value = client
        .call("get_tree", serde_json::json!({}))
        .map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("get_tree", "TreeResult", &e)))
}

/// 读一层目录。`path` 是清单**原文**（约束 3：不 trim、不折叠 `//`、不动非 ASCII）。
///
/// ⚠️ 失败形状是 `DirLoadFailure`（不是 `CallFailure`）：`path_not_found` 的**回退判据**
///    认的是**结构化错误码**，所以内核那一支要把 `ClientError` **原样**带回来
///    —— 转成字符串再拼回去就等于把那个码丢了（`file_browser.rs` 第一版栽过这个坑，
///    而用例还全绿）。
pub fn list_dir(client: &CoreClient, path: &str) -> Result<ListDirResult, DirLoadFailure> {
    let value = client
        .call("list_dir", serde_json::json!({ "path": path }))
        .map_err(|e| DirLoadFailure::of(&e, path))?;
    serde_json::from_value(value).map_err(|e| DirLoadFailure {
        message: shell_cannot_decode("list_dir", "ListDirResult", &e),
        path: path.to_string(),
        notice: None,
    })
}

/// 加下载任务。`paths` 由 `DownloadTargets::paths` 算出来 —— **本函数不碰那个判据**
/// （整批全选 ⇒ 空数组的规则只有一处实现）。
///
/// ⚠️ **那个调用方是 [`crate::commands::enqueue`]（经 `commands::kernel_paths`）** ——
///    任务 16 把它接上的。这句注释原先只写"由调用方算"，而**调用方从来没算过**：
///    C-3 那道保护在本代一度**没有任何一端在守**（判据只有 `portcheck.sh` 那个对等检查
///    工具在跑）。把"是谁"写在这里，是为了"本函数不碰"与"总得有人碰"之间不再出现一个
///    谁都不认领的缺口。
pub fn enqueue(client: &CoreClient, paths: &[String]) -> Result<EnqueueResult, CallFailure> {
    let value = client
        .call("enqueue", serde_json::json!({ "paths": paths }))
        .map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("enqueue", "EnqueueResult", &e)))
}

/// 一拍 `transfer_list` 的结局。
///
/// ⚠️ **三档不是"成功/失败"两档**：`Absorbed` 是内核**亲口说的正常态**
///    （`engine_not_started` / `no_delivery`），把它挂红横幅等于把正常说成故障；
///    而 `KernelGone` 要交回**共享判定**去翻引擎那一格（落点完全不同）。
// ⚠️⚠️ **一个 derive 都不派生** —— 这是本计划第三版改的，理由是**它派生不出来**：
//   · `PartialEq` / `Eq`：载荷里的 `TransferListResult`（以及 `TransferItem` / `GlobalStat`）
//     在 `protocol.rs` 里**没有 `Eq`**（它们是 `#[derive(Debug, Clone, PartialEq, Deserialize)]`）
//     ⇒ `Eq` 不可能派出来；
//   · `PartialEq` 还要 `CallFailure: PartialEq`，而 `CallFailure` **一个 derive 都没有**
//     （`kernel_death.rs:116` 上面 `grep -c derive` = 0）；
//   · 而给这两个类型加 derive 会**扩大** shell-core 的公开面 —— 本计划对它的裁定是
//     **"只加一个只读访问器 `text()`"**，加 derive 就超出去了。
//   ⇒ **改用 `matches!` 断言**（见下面那两条用例），一个 derive 都不需要。
pub enum TransferPoll {
    Snapshot(TransferListResult),
    /// 引擎还没起来（`engine_not_started`）—— **正常态**，批次可能好好的。
    Absorbed,
    /// **内核亲口说"这一批不存在或已过期"**（`no_delivery`）—— **正常态**，
    /// 但它与上面那一档**必须分开**：调用方要据此**作废当前批次**
    /// （`Session::reset_to_empty_state`，对齐 macOS 的 `resetToEmptyState()`）。
    ///
    /// ⚠️ 合并成一个"吸收掉"的档的后果（本任务审查 I-1 抓的）：批次作废之后
    ///    "当前批次"那一格还留着那个死码 ⇒ 下一次内核重启会**重放**它，
    ///    用户看到一次本不该出现的失败，而 macOS 在那一刻是**空态**。
    /// ⚠️ 两档的**显示**处置完全相同（都发一个空快照给前端）—— 区别只在**状态**。
    NoDelivery,
    Failed(CallFailure),
}

impl TransferPoll {
    /// 这一拍的结局**要不要顺带把引擎那一格翻成"未启动"**（`true` = 要）。
    ///
    /// **对齐 macOS**：`absorb` 的 `case .engineNotStarted?: engine = .notStarted;
    /// transfers = .empty`（`AppModel.swift:1634`）—— 那是**一条**处置，所有调用点都从那里过。
    ///
    /// ⚠️ **表只有一行**（其余三档都不动那一格）：
    ///
    /// | 结局 | 翻不翻 | 为什么 |
    /// |---|---|---|
    /// | [`TransferPoll::Absorbed`]（`engine_not_started`） | **翻** | 内核刚说"引擎没起来"，而徽标是用户照着判断"任务在跑"的那一格 |
    /// | [`TransferPoll::NoDelivery`]（`no_delivery`） | 不翻 | 那是"这一批没了"，与引擎在不在跑无关 |
    /// | [`TransferPoll::Snapshot`] / [`TransferPoll::Failed`] | 不翻 | 成功的快照自己带着状态；失败那一档走另一条路（`commands::note_failure`） |
    ///
    /// ⚠️ 它做成 `TransferPoll` 的**方法**（而不是写在命令层那个 `match` 里）：
    ///    判据要能被单测钉住（四档各断一条），命令层只留"翻一下"那一行。
    pub fn flips_engine_to_not_started(&self) -> bool {
        matches!(self, TransferPoll::Absorbed)
    }
}

/// 发一拍 `transfer_list`（`params` 是 `null`：内核的 `op_transfer_list` 不读参数）。
pub fn transfer_list(client: &CoreClient) -> TransferPoll {
    match client.call("transfer_list", serde_json::json!(null)) {
        Ok(value) => match serde_json::from_value::<TransferListResult>(value) {
            Ok(result) => TransferPoll::Snapshot(result),
            Err(e) => TransferPoll::Failed(CallFailure::shell(shell_cannot_decode(
                "transfer_list",
                "TransferListResult",
                &e,
            ))),
        },
        // ⚠️ 这两档**不是错误**（内核亲口说的正常态）。判据认的是结构化 code。
        //    ⚠️ 它们**不再合成一档**（任务 8 审查 I-1）：`no_delivery` 意味着
        //    **这一批没了**，调用方要据此作废当前批次（否则死码会被重放）。
        //    判据用 shell-core 的那**一个**函数（`is_no_delivery`），不在这里另写一份。
        Err(ref e @ ClientError::Kernel { ref code, .. })
            if *code == codes::ENGINE_NOT_STARTED || *code == codes::NO_DELIVERY =>
        {
            // ⚠️ 两档**都由 shell-core 的那**一个**函数判**（`is_no_delivery`）——
            //    不在本文件里再写一遍 `code == "no_delivery"`（那会让"什么算这一批没了"
            //    出现第二份判据，而两条路迟早会各说各话）。
            if is_no_delivery(&e) {
                TransferPoll::NoDelivery
            } else {
                TransferPoll::Absorbed
            }
        }
        // 内核真的死了那一档走共享判定，**不**当瞬时错误照登。
        Err(e) => match KernelDeath::reason_of(&e) {
            Some(reason) => TransferPoll::Failed(CallFailure::KernelGone {
                reason,
                text: TransferActionFailure::message_of(&e),
            }),
            None => TransferPoll::Failed(CallFailure::Text(TransferActionFailure::message_of(&e))),
        },
    }
}

/// 一条 `task_action`。
///
/// ⚠️ 动作名是**线上字面量**（`TaskAction` 的 `snake_case` 序列化）；没有 gid 时
///    **不带那个键**（内核用 `params.get("gid").unwrap_or("")` 取它，空串与"键不在"
///    在那边是同一件事）。
pub fn task_action(
    client: &CoreClient,
    action: TaskAction,
    gid: Option<String>,
) -> Result<(), CallFailure> {
    let mut params = serde_json::Map::new();
    let action_value = serde_json::to_value(action).map_err(|e| {
        CallFailure::shell(format!(
            "这个动作没能编码成内核认识的形状（{e}）：这是壳自己的序列化失败。\
             补救：把这条原样发给我们。"
        ))
    })?;
    params.insert("action".to_string(), action_value);
    if let Some(gid) = gid {
        if !gid.is_empty() {
            params.insert("gid".to_string(), serde_json::Value::String(gid));
        }
    }
    client
        .call("task_action", serde_json::Value::Object(params))
        .map(|_| ())
        .map_err(|e| CallFailure::of(&e))
}

/// 取内核手里那一份设置（`get_settings`，无参方法发 `params: null`）。
///
/// ⚠️ **它是"参数面板打开时看到什么"的唯一来源**：内核那一份是**权威**
///    （它才真正生效、也可能被手改过 `settings.json`），壳**不缓存一份自己的**
///    （缓存 = 与内核那份各走各的，而漂移时没有任何东西会红）。
///    与 macOS 的一处**有意差异**：那边握手时把设置一起读进内存（`performHandshake`），
///    我们这里每次按需问一遍 —— 我们的 `Session` 里**没有**设置这一格，
///    加一格就要回答"它什么时候失效"，而"每次问一遍"是一个不需要回答的问题。
pub fn get_settings(client: &CoreClient) -> Result<SettingsResult, CallFailure> {
    let value = client
        .call("get_settings", serde_json::Value::Null)
        .map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("get_settings", "SettingsResult", &e)))
}

/// 写七项参数（`set_settings`）。回执是**归一化之后**的那一份。
///
/// ⚠️ **`params` 的形状是 `{"settings": <七个键>}`**（外面套一层）—— 那不是装饰：
///    内核的 `op_set_settings` 读的就是 `params.get("settings")`，少了这一层它会回
///    `invalid_params`。⚠️ 而七个键**由 [`Settings`] 自己编码**（字段名就是线上键名），
///    本函数**不手拼**那七个键 —— 手拼一份就是给"线格式"造第二个真相源。
///
/// 🔴 **本波次的修订（R-38 的口径）：那一层信封现在由
///    [`SettingsForm::set_settings_params`] 给，本函数不再自己 `json!` 拼一次。**
///    在此之前那两个东西**各有一份实现**（`presentation/` 里那一份**零调用者**、
///    这里这一份在跑），而那正是账本 R-38 裁掉 `History::put` 的形状：
///    "同一个形状拼两遍 = 迟早有一份漏了这一层，而漏的那一份**永远不会红**"
///    （`presentation` 那边的用例测的是一个**没人跑**的版本）。
///    ⇒ 从"那条路径调的就是这一处信封"这个意义上，两份合成了一份。
///
/// ⚠️ **编码失败仍然回一句可读的人话**（**不许 panic**）—— 那句话现在住在
///    [`SettingsForm::encode_failure_text`]，措辞与 [`task_action`] 那条**同款**
///    （同样点名"壳自己的序列化失败"、同样给出"把这条原样发给我们"的补救）。
///    🔴 **为什么不就地 `format!` 一句、而要绕一次 `presentation`**（这不是洁癖）：
///    这条分支在真实的 `Settings` 上**走不到**（七个成员全是 `i32`/`i64`/`String`），
///    而"这一支不可达"**恰恰是错的时候最贵的那一类断言** —— 它不适用时的后果不是
///    一句错话，是**前端那条 `invoke` 永远不 resolve**（界面静静地挂住）。
///    ⇒ 那句人话必须有一个**能被用例走到的落点**：`presentation` 那边把编码做成
///    可替换的一步，用例拿一个注定失败的编码器走同一条链、断言回的是这句话
///    （`an_encoding_failure_comes_back_as_a_readable_sentence_not_a_panic`）。
///    ⇒ 于是"回哪句话"这件事**有判据压着**，而不是"我看它不可达"。
///    ⚠️ 编码失败**不回落**：回落成 `.null` / 空对象就是**静默**发一条少键请求，
///    内核回 `invalid_params`，而壳这边看起来一切正常（本项目最恨的形状）。
///    ⚠️ [`task_action`] 那条**保持原样**（就地 `format!`）：它编的是用户输入里的
///    动作，那条路真的会失败、也真的走得到，所以它不需要这个"可测落点"的绕行。
pub fn set_settings(client: &CoreClient, settings: &Settings) -> Result<SettingsResult, CallFailure> {
    // ⚠️ 编码失败 ⇒ 回一句**可读的人话**（与 `task_action` 那条同款措辞），**绝不 panic**：
    //    panic 会让前端那条 `invoke` 永远等不到回话（静默挂住）。
    let params = SettingsForm::set_settings_params(settings)
        .map_err(|e| CallFailure::shell(SettingsForm::encode_failure_text(&e)))?;
    let value = client
        .call("set_settings", params)
        .map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("set_settings", "SettingsResult", &e)))
}

/// 取一次 `verify_status`（无参方法发 `params: null`）。
pub fn verify_status(client: &CoreClient) -> Result<VerifyStatus, CallFailure> {
    let value = client
        .call("verify_status", serde_json::Value::Null)
        .map_err(|e| CallFailure::of(&e))?;
    serde_json::from_value(value)
        .map_err(|e| CallFailure::shell(shell_cannot_decode("verify_status", "VerifyStatus", &e)))
}

/// 「内核**回了**、而壳解不动它的形状」那句话。
///
/// ⚠️ **这不是内核报的错**（内核报的错原文逐字照登）。措辞与
///    `views/verify.rs::shell_cannot_decode` **逐字相同** —— 搬过来的时候不许改写。
/// ⚠️ 六个方法共用这一个函数：同一件事只有一套话（规格 §10「唯一映射」）。
// ⚠️⚠️ **这里没有那个泛型的 `decode` 帮手**（计划正文里有一个，本计划第五版去掉了）：
//    它写的是 `fn decode<T: serde::de::DeserializeOwned>(…)`，而 `shell-win` 的
//    `[dependencies]` 里**没有 `serde`**（只有 `serde_json` 与 `shell-core`；`serde` 只是
//    传递依赖，不进 extern prelude）⇒ 那个 trait 在这个 crate 里**叫不出来**，编不过
//    （`E0433`）。补救只有两条：给 `shell-win` 加一个 `serde` 依赖（那张直接依赖清单是
//    **受管的** —— 它现在是 `shell-core` / `serde_json` / `tauri` / `windows-sys`，
//    见 `shell-win/Cargo.toml`，加一项就是往那张表里加一项），或者**把解码就地写**。
//    （修订史：这句话里原先点名的三个依赖是第二代的 `tiny_http` / `getrandom` /
//     `windows-sys` —— 前两个随第二代传输层在 Tauri 那一代被删，`tauri` 是那一代加的。）
//    选了后者 —— 它同时更贴近来源：今天 `file_browser.rs` / `main.rs` / `verify.rs` 的
//    那几处本来就是**就地** `serde_json::from_value(...).map_err(...)` 的。
//    ⚠️ **"唯一映射"没有因此多出一份**：那句话仍然只有 [`shell_cannot_decode`] 一处。
fn shell_cannot_decode(method: &str, shape: &str, cause: &serde_json::Error) -> String {
    format!(
        "{method} 的 result 与 {shape} 的形状不符：{cause}。\
         这是壳解不动内核的回执（不是内核报的错）。补救：把这条原样发给我们。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use shell_core::client::{ClientError, CoreClient, LineChannel};
    use std::sync::{Arc, Mutex};

    /// 一条**脚本化的内核通道**：按顺序吐出预置的响应行，并记下收到的请求。
    ///
    /// ⚠️ `LineChannel` 是 `shell-core` 的**公开** trait（`client.rs:307`），
    ///    所以这里能在不碰 `shell-core` 一个字节的前提下拿到一个真 `CoreClient`。
    // ⚠️⚠️ **两处签名必须照 `LineChannel` 的原样**（这是本计划第四版改的，前两版都编不过）：
    //    · 两个方法返回的是 `Result<_, ClientError>`，**不是 `std::io::Result`**；
    //    · `read_line` 返回 `Result<Option<String>, ClientError>`，`None` 就是 EOF。
    //    ⚠️ 而且**没有 `impl LineChannel for Arc<T>`**（全 crate 只有 `ProcessChannel`
    //    （`client.rs:469`）、`shell-core` 自己的测试替身 `StubChannel`（`client.rs:925`）
    //    与 `main.rs` 那个 `Silent`）⇒ `Box::new(Arc::clone(&ch))` **编不过**。
    //    ⇒ **照仓库既有那个替身的形状**（`main.rs:448-499`）：替身自己
    //    `#[derive(Clone)]`、内部持 `Arc`，用 `Box::new(script.clone())` ——
    //    `Clone` 共享同一份状态，所以测试里那个宿主句柄照样读得到 `written`。
    #[derive(Clone)]
    struct Scripted {
        replies: Arc<Mutex<std::collections::VecDeque<String>>>,
        written: Arc<Mutex<Vec<String>>>,
    }

    impl Scripted {
        fn new(replies: &[&str]) -> Scripted {
            Scripted {
                replies: Arc::new(Mutex::new(replies.iter().map(|s| s.to_string()).collect())),
                written: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn last_request(&self) -> String {
            self.written.lock().unwrap().last().cloned().unwrap_or_default()
        }
    }

    impl LineChannel for Scripted {
        fn write_line(&self, line: &str) -> Result<(), ClientError> {
            self.written.lock().unwrap().push(line.to_string());
            Ok(())
        }
        fn read_line(&self) -> Result<Option<String>, ClientError> {
            Ok(self.replies.lock().unwrap().pop_front())
        }
        fn close(&self) {}
    }

    /// 造一条回执：`{"id":N,"ok":true,"result":…}`。
    fn ok_reply(id: u64, result: &str) -> String {
        format!(r#"{{"id":{id},"ok":true,"result":{result}}}"#)
    }

    /// 造一条内核报错：`{"id":N,"ok":false,"error":{"code":…,"message":…}}`。
    fn err_reply(id: u64, code: &str, message: &str) -> String {
        format!(r#"{{"id":{id},"ok":false,"error":{{"code":"{code}","message":"{message}"}}}}"#)
    }

    /// 断言一次调用成功，失败时把**内核原文**说出来。
    ///
    /// ⚠️ **这里不能用 `.unwrap()`**（本计划第五版改的）：`Result::unwrap` 要 `E: Debug`，
    ///    而 `CallFailure` **没有 `Debug`** —— 裁定是"本任务对 shell-core 的改动只有
    ///    `text()` 一处，不加任何 derive"（理由见 `TransferPoll` 上面那段）。
    ///    ⇒ 用那个访问器本身把话说出来，失败时照样看得见内核原文
    ///    （与下面测试 4 那条断言同一个理由）。
    ///    ⚠️ `list_dir` 的两条不用它：那条路的失败是 `DirLoadFailure`，它**有** `Debug`
    ///    （`breadcrumb.rs` 的 derive），`.unwrap()` 照用。
    fn expect_ok<T>(outcome: Result<T, CallFailure>, what: &str) -> T {
        match outcome {
            Ok(value) => value,
            Err(why) => panic!("{what} 不该失败：{}", why.text()),
        }
    }

    /// `load_delivery` 的 method 与参数形状（`base_url` 空串时**不带那个键**）。
    ///
    /// ⚠️ **回执必须是合法的 `DeliveryInfo`**（本计划第五版改的，第三版那个夹具少了五个键）：
    ///    `protocol.rs` 的 `DeliveryInfo` **没有任何 `serde(default)`**，每个字段都得给；
    ///    `tree` 的合法空树是 `{}`（⇒ `TreeNode::Empty`）。夹具**只是让 `.unwrap()` 不 panic**，
    ///    这条用例验的始终是**发出去的请求**（下面那三行断言）。
    #[test]
    fn load_delivery_omits_base_url_when_it_is_empty() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"code":"R.x","page_url":"","base_url":"","created_at":"","expires_at":"","expired":false,"total_files":0,"total_bytes":0,"tree":{}}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(load_delivery(&client, "R.x", ""), "load_delivery");
        let sent = ch.last_request();
        assert!(sent.contains(r#""method":"load_delivery""#));
        assert!(sent.contains(r#""code":"R.x""#));
        assert!(!sent.contains("base_url"), "空串时**不许**带这个键");
    }

    /// 有 `base_url` 时要带上。
    #[test]
    fn load_delivery_carries_base_url_when_it_is_given() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"code":"R.x","page_url":"","base_url":"http://u/","created_at":"","expires_at":"","expired":false,"total_files":0,"total_bytes":0,"tree":{}}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(load_delivery(&client, "R.x", "http://u/"), "load_delivery");
        assert!(ch.last_request().contains(r#""base_url":"http://u/""#));
    }

    /// 内核报错 ⇒ **原文照登**（约束 3），且走的是 `error_text` 那**唯一**一份映射。
    #[test]
    fn a_kernel_error_keeps_the_kernel_wording_verbatim() {
        let ch = Scripted::new(&[&err_reply(1, "no_delivery", "这一批不存在或已过期")]);
        let client = CoreClient::new(Box::new(ch));
        let why = load_delivery(&client, "R.x", "").unwrap_err();
        assert_eq!(why.text(), "这一批不存在或已过期");
    }

    /// ⚠️ **内核报 `engine_disconnected` ⇒ 必须被判成"内核没了"**，不是一句普通错误。
    ///    这条判据在 `shell-core` 的 `KernelDeath` 里（唯一一份），这里只是用它。
    #[test]
    fn the_kernel_disconnected_code_is_recognised_as_kernel_death() {
        let ch = Scripted::new(&[&err_reply(1, "engine_disconnected", "引擎已断开")]);
        let client = CoreClient::new(Box::new(ch));
        let why = load_delivery(&client, "R.x", "").unwrap_err();
        // ⚠️ **不写 `{why:?}`**：`CallFailure` 没有 `Debug`（原因见 `TransferPoll` 上面那段），
        //    而给它加 `Debug` 会超出"只加一个访问器"的裁定。
        //    这里用**那个访问器本身**（`text()`，本任务新增的唯一一处 shell-core 改动）
        //    把话说出来 —— 既不需要 derive，失败时也照样看得见内核原文。
        assert!(
            matches!(why, CallFailure::KernelGone { .. }),
            "没认成内核死亡：{}",
            why.text()
        );
    }

    /// `get_tree` 发的是**空对象**参数（与视图里那条一致）。
    ///
    /// ⚠️ **回执必须是合法的 `TreeResult`**（本计划第五版改的，第三版那个夹具缺 `tree`、
    ///    且 `progress` 四个键名全错）：`TreeResult` 要 `tree`（空树 `{}`），
    ///    `Progress` 的四个键是 `total_bytes` / `done_bytes` / `speed` / `percent`。
    ///    这条用例验的是**请求**（`params:{}`），夹具只是让 `.unwrap()` 不 panic。
    #[test]
    fn get_tree_sends_an_empty_object() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"tree":{},"flat":[],"default_selected":[],"progress":{"total_bytes":0,"done_bytes":0,"speed":0,"percent":0}}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(get_tree(&client), "get_tree");
        assert!(ch.last_request().contains(r#""params":{}"#));
    }

    /// `list_dir` 的路径是**清单原文**：不 trim、不折叠 `//`、不动非 ASCII。
    #[test]
    fn list_dir_passes_the_manifest_path_through_untouched() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"path":"a//b/中文","entries":[]}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        list_dir(&client, "a//b/中文").unwrap();
        assert!(ch.last_request().contains(r#""path":"a//b/中文""#), "路径被加工过了");
    }

    /// ⚠️ `path_not_found` ⇒ **退到上一层**（这条判据认的是**结构化错误码**，
    ///    不是 `message` 的措辞 —— 契约 §5.1）。
    #[test]
    fn a_path_not_found_falls_back_to_the_parent_level() {
        let ch = Scripted::new(&[&err_reply(1, "path_not_found", "没有这个目录")]);
        let client = CoreClient::new(Box::new(ch));
        let why = list_dir(&client, "01.RawData/sub").unwrap_err();
        assert_eq!(why.path, "01.RawData");
        assert!(why.did_fall_back());
    }

    /// 其余错误码 ⇒ **原地不动**（不许"什么都退一层"，那会把用户甩到根目录）。
    #[test]
    fn any_other_error_stays_where_it_is() {
        let ch = Scripted::new(&[&err_reply(1, "no_delivery", "这一批没了")]);
        let client = CoreClient::new(Box::new(ch));
        let why = list_dir(&client, "01.RawData").unwrap_err();
        assert_eq!(why.path, "01.RawData");
        assert!(!why.did_fall_back());
    }

    /// `enqueue` 发的是 `{"paths":[…]}`（整批全选 ⇒ 空数组，判据在 `DownloadTargets`）。
    #[test]
    fn enqueue_sends_the_paths_it_is_given() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"added":[],"rejected":[]}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(enqueue(&client, &["a".to_string()]), "enqueue");
        assert!(ch.last_request().contains(r#""paths":["a"]"#));
    }

    /// ⚠️ `transfer_list` 的那两档"内核亲口说的正常态"**都不是错误**（挂红横幅
    ///    就是把**正常**说成**故障**），但它们**各自成一档**（任务 8 审查 I-1）：
    ///
    /// | 码 | 档 | 调用方要多做的那一步 |
    /// |---|---|---|
    /// | `engine_not_started` | [`TransferPoll::Absorbed`] | 只发空快照 |
    /// | `no_delivery` | [`TransferPoll::NoDelivery`] | **作废当前批次**（清 `load_request`） |
    ///
    /// 判别力（两个方向都会红）：把两档**合并**（回到 `Absorbed`）⇒ 第二条断言红 ——
    /// 而真机上的表现是"批次作废之后，下一次内核重启把那个死码重放一遍"；
    /// 把两档**判反**（`engine_not_started` 也去作废批次）⇒ 第一条断言红 ——
    /// 那会在**每次添加任务之前**把一批好端端的批次清掉。
    #[test]
    fn the_two_normal_states_have_their_own_kinds() {
        let ch = Scripted::new(&[&err_reply(1, "engine_not_started", "正常态")]);
        let client = CoreClient::new(Box::new(ch));
        // ⚠️ `matches!` 而不是 `assert_eq!`：`TransferPoll` **一个 derive 都没有**
        //    （理由见它上面那段）。
        assert!(
            matches!(transfer_list(&client), TransferPoll::Absorbed),
            "`engine_not_started` 是「还没添加过任务」——**批次好好的**，不许作废它"
        );

        let ch = Scripted::new(&[&err_reply(1, "no_delivery", "这一批没了")]);
        let client = CoreClient::new(Box::new(ch));
        assert!(
            matches!(transfer_list(&client), TransferPoll::NoDelivery),
            "`no_delivery` 是「这一批没了」——它必须**自成一档**（调用方要作废当前批次）"
        );
    }

    /// 🔴 **四档各自要不要翻引擎那一格**（审查要的那条：`engine_not_started` 之后
    /// 引擎那一格**真的变了** —— 不是只断言"发了空快照"）。
    ///
    /// 判别力：给 `NoDelivery` 那一档也返回 `true`（看起来很"周全"：内核说这一批没了，
    /// 顺手把引擎也标成没起来）⇒ 第二条断言红 —— 而真机上的表现是
    /// **用户一换批次（或这一批过期）就看到徽标变成「引擎未启动」**，
    /// 可引擎明明还在跑（那一批只是没了）。
    #[test]
    fn only_the_engine_not_started_outcome_flips_the_engine_slot() {
        let ch = Scripted::new(&[&err_reply(1, "engine_not_started", "正常态")]);
        let client = CoreClient::new(Box::new(ch));
        assert!(
            transfer_list(&client).flips_engine_to_not_started(),
            "内核说「引擎没起来」⇒ 引擎那一格要跟着翻（否则它停在「运行中」，用户以为任务在跑）"
        );

        let ch = Scripted::new(&[&err_reply(1, "no_delivery", "这一批没了")]);
        let client = CoreClient::new(Box::new(ch));
        assert!(
            !transfer_list(&client).flips_engine_to_not_started(),
            "「这一批没了」与引擎在不在跑无关 —— 不许顺手把那一格翻成「未启动」"
        );

        // 成功那一档也不翻（快照自己带着引擎的真实状态）。
        let ch = Scripted::new(&[&ok_reply(
            1,
            r#"{"items":[],"global":{"download_speed":0,"num_active":0,"num_waiting":0,"num_stopped":0}}"#,
        )]);
        let client = CoreClient::new(Box::new(ch));
        assert!(!transfer_list(&client).flips_engine_to_not_started());

        // 失败那一档也不翻（那是另一条路：`note_failure`）。
        let ch = Scripted::new(&[&err_reply(1, "engine_rpc_failed", "一次可重试的报错")]);
        let client = CoreClient::new(Box::new(ch));
        assert!(
            !transfer_list(&client).flips_engine_to_not_started(),
            "失败那一档走 `note_failure`，不在这里翻"
        );
    }

    /// 内核真的没了 ⇒ `Failed(KernelGone)`（与 Absorbed 是**两件事、两个落点**）。
    #[test]
    fn a_dead_kernel_is_not_absorbed() {
        let ch = Scripted::new(&[&err_reply(1, "engine_disconnected", "引擎已断开")]);
        let client = CoreClient::new(Box::new(ch));
        assert!(matches!(
            transfer_list(&client),
            TransferPoll::Failed(CallFailure::KernelGone { .. })
        ));
    }

    /// `task_action` 的动作名是**线上字面量**（`snake_case`），且没有 gid 时**不带那个键**。
    ///
    /// ⚠️ **回执必须是内核真发的那个形状**（本计划第五版改的，第三版写的是 `"null"`）：
    ///    `"result":null` 在 `CoreClient::call` 那里是 `ClientError::MissingResult`
    ///    （`client.rs:788` 的 `resp.result.ok_or(…)`：`Option<Value>` 把 JSON `null` 解成
    ///    `None`）⇒ 这一发会被判成失败。内核的 `op_task_action` 成功时回的是
    ///    `json!({"action": action})`（`core/src/main.rs:1574`），照它写。
    #[test]
    fn task_action_sends_the_wire_name_and_omits_an_empty_gid() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"action":"clear_finished"}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(task_action(&client, TaskAction::ClearFinished, None), "task_action");
        let sent = ch.last_request();
        assert!(sent.contains(r#""action":"clear_finished""#), "动作名不是线上字面量：{sent}");
        assert!(!sent.contains("gid"), "没有 gid 时**不许**带这个键");
    }

    /// `verify_status` 发的是 `params: null`（无参方法），且回执解得出 `VerifyStatus`。
    ///
    /// ⚠️ **这条用例是交付后补的**：它曾经是 7 个 `pub fn` 里唯一没有用例的那个，
    ///    而它当时靠 `views/verify.rs` 的用例**间接**覆盖 —— 任务 10 删掉 `views/`
    ///    之后那份覆盖就消失了 ⇒ 它会变成一段谁都不碰的代码。
    ///
    /// ⚠️ 夹具是 `protocol.rs:1437-1440` 那条**同一个键集合**（六个桶 + `all_good`）——
    ///    键名写错就是解码失败，而"六个空桶 + `all_good:true`"正是内核自己的
    ///    全通过口径（`core/src/verify.rs`）。
    /// `get_settings` 发的是 `params: null`（无参方法），且回执解得出 `SettingsResult`。
    ///
    /// ⚠️ 夹具是**内核真发的那个形状**（`op_get_settings` 的 `json!`：两个顶层键、
    ///    七个内层键）—— 键名写错就是解码失败，而那条路在真机上的表现是
    ///    **参数面板整屏空白**（`settings` 解不出来）。
    #[test]
    fn get_settings_sends_null_params_and_decodes_the_receipt() {
        let ch = Scripted::new(&[&ok_reply(
            1,
            r#"{"settings":{"parallel":8,"connections":16,"splits":16,"min_split_size":"20M","limit_mbps":0,"max_tries":3,"retry_wait":1},"last_code":"C24-8"}"#,
        )]);
        let client = CoreClient::new(Box::new(ch.clone()));
        let result = expect_ok(get_settings(&client), "get_settings");
        assert_eq!(result.last_code, "C24-8");
        assert_eq!(result.settings.min_split_size, "20M");
        let sent = ch.last_request();
        assert!(sent.contains(r#""method":"get_settings""#), "method 不在请求里：{sent}");
        assert!(sent.contains(r#""params":null"#), "无参方法要发 `params:null`：{sent}");
    }

    /// ⚠️ `set_settings` 的 `params` **外面套一层 `settings`**（内核读的就是这一层）。
    ///
    /// 判别力：去掉那一层（把七个键摊在顶层），内核会回 `invalid_params` —— 而
    /// 请求本身看起来"也是七个键"，只是形状不对（一个只有真机才看得见的错）。
    /// 顺带钉住**七个键的名字**（手拼/改名都会在这里露出来）。
    #[test]
    fn set_settings_wraps_the_seven_keys_under_the_settings_key() {
        let ch = Scripted::new(&[&ok_reply(
            1,
            r#"{"settings":{"parallel":9,"connections":11,"splits":13,"min_split_size":"21M","limit_mbps":12345,"max_tries":7,"retry_wait":3},"last_code":""}"#,
        )]);
        let client = CoreClient::new(Box::new(ch.clone()));
        let sent_settings = Settings {
            parallel: 9,
            connections: 11,
            splits: 13,
            min_split_size: "21M".to_string(),
            limit_mbps: 12_345,
            max_tries: 7,
            retry_wait: 3,
        };
        let echoed = expect_ok(set_settings(&client, &sent_settings), "set_settings");
        // 回执里是**内核归一化之后**的那一份（本用例里恰好与发出去的一致）。
        assert_eq!(echoed.settings, sent_settings);
        assert_eq!(echoed.last_code, "");

        let sent = ch.last_request();
        assert!(sent.contains(r#""method":"set_settings""#), "method 不在请求里：{sent}");
        assert!(
            sent.contains(r#""params":{"settings":{"#),
            "params 必须把七个键套在 `settings` 底下：{sent}"
        );
        for key in [
            "parallel",
            "connections",
            "splits",
            "min_split_size",
            "limit_mbps",
            "max_tries",
            "retry_wait",
        ] {
            assert!(sent.contains(&format!(r#""{key}":"#)), "少了 {key} 这个键：{sent}");
        }
    }

    /// 内核拒绝一份参数时，**原文照登**（用户要看到的正是那句"当前 99"）。
    #[test]
    fn a_rejected_setting_keeps_the_kernel_wording_verbatim() {
        let ch = Scripted::new(&[&err_reply(1, "invalid_params", "并行文件数必须在 1–64 之间，当前 99")]);
        let client = CoreClient::new(Box::new(ch));
        let why = set_settings(
            &client,
            &Settings {
                parallel: 99,
                connections: 16,
                splits: 16,
                min_split_size: "20M".to_string(),
                limit_mbps: 0,
                max_tries: 3,
                retry_wait: 1,
            },
        )
        .err()
        .expect("内核报错了，这里不该是成功");
        assert_eq!(why.text(), "并行文件数必须在 1–64 之间，当前 99");
    }

    #[test]
    fn verify_status_sends_null_params() {
        let ch = Scripted::new(&[&ok_reply(1, r#"{"ok":[],"bad":[],"missing":[],"size_mismatch":[],"unverifiable":[],"unreadable":[],"all_good":true}"#)]);
        let client = CoreClient::new(Box::new(ch.clone()));
        expect_ok(verify_status(&client), "verify_status");
        let sent = ch.last_request();
        // ⚠️ **顺序**：先断言 method **在**，再断言 `params:null` —— 反过来的话，
        //    `last_request()` 返回空串时那条否定形式的断言会**假绿**。
        assert!(sent.contains(r#""method":"verify_status""#), "method 不在请求里：{sent}");
        assert!(sent.contains(r#""params":null"#), "无参方法要发 `params:null`：{sent}");
    }
}
