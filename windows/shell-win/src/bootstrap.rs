//! `bootstrap` —— **装配逻辑**：起内核之前那些"要不要起、起哪一个、握手有没有回话"的决定。
//!
//! ## ⚠️⚠️ 本模块是**从 git 历史里搬回来的**（R-15，本任务最要紧的一件事）
//!
//! 任务 1 把第二代传输层（本地 HTTP 服务 + 系统浏览器那一整套）整个删掉时，
//! **把当时的 `main.rs` 整个换掉了** —— 于是这几样**不属于传输层**的东西一起离开了
//! 工作区（任务 1 的报告 §4 ④ 如实记了这件事，控制者据此裁定 R-15）：
//!
//! | 搬回来的 | 它是什么 |
//! |---|---|
//! | `CORE_EXE_NAME` | 内核 exe 的文件名（Windows 上必须带 `.exe`） |
//! | [`locate_core_binary`] | 定位内核 exe 的三段顺序（env → 内嵌释放 → 同目录） |
//! | [`CoreLookup`] | 上面那个的返回类型（选定路径 + "退回同目录那份"的原因） |
//! | [`choose_core`] | 三路优先级的**纯函数**（宿主上可断言） |
//! | `same_dir_candidate` / `embedded_core` | ②③ 两条路的落点 |
//! | [`core_not_found_message`] | 三条都断了时那句**指名道姓**的话（含可执行的补救） |
//! | [`DEFAULT_HANDSHAKE_TIMEOUT`] | 握手超时（**唯一一份**，与那句文案配对） |
//! | `mod bounded_handshake`（含 `handshake` / `call_with_deadline`） | 握手的**有界等待** |
//! | [`connect`] / [`Connected`] | 起内核子进程 → `hello` → 交出连接 |
//! | [`spawn_core_job`] / `cannot_spawn_thread_message` | "后台线程 + 通道"那个形状与它的失败话术 |
//!
//! **原文出处**：`git show 6f49010:windows/shell-win/src/main.rs`（那批东西自包含在那一个
//! 文件里）。搬的时候**判据与文案一个字都没改**（8 条随它们一起走的用例在下面 `mod tests`
//! 与 `bounded_handshake::tests` 里，逐条按原文恢复）。改动只有三类，逐条记在这里：
//!
//!   1. **路径**：`spawn::` → `crate::spawn::`、`embed::` → `crate::embed::`、
//!      `crate::DEFAULT_HANDSHAKE_TIMEOUT` → `super::DEFAULT_HANDSHAKE_TIMEOUT`
//!      （那批东西原先住在 bin 的 crate 根，现在住在 lib 的一个模块里）；
//!   2. **可见性**：`main.rs` 要用到的那几个（`locate_core_binary` / `CoreLookup` 及其字段 /
//!      `connect` / `Connected` 及其字段 / `spawn_core_job`）从私有改成 `pub`
//!      —— 本模块在 lib 里，bin 与 lib 是**两个 crate**；
//!   3. **注释里那些已经不成立的路径引用**（`server/routes.rs` / `views/*.rs` /
//!      "界面线程" / "没有窗口了"）按今天的形状改写。**代码与用户可见文案一个字未动。**
//!
//! ⚠️ **那 12 条装配用例里只有 8 条回来了**（本任务恢复的正是这 8 条）：
//!    · 内核查找 4 条（`a_failed_release_falls_back_to_the_kernel_next_to_the_exe_and_says_so`
//!      / `a_failed_release_with_nothing_to_fall_back_to_reports_the_release_failure`
//!      / `the_lookup_order_is_env_then_embedded_then_next_to_the_exe`
//!      / `the_not_found_message_names_all_three_places_and_a_remedy`）；
//!    · `spawn_core_job` 1 条（`spawn_core_job_runs_the_closure_off_the_calling_thread_and_sends_the_value`）；
//!    · 握手 3 条（`the_handshake_timeout_and_its_wording_agree`
//!      / `a_silent_kernel_times_out_into_the_message_the_ui_shows`
//!      / `the_handshake_itself_uses_the_shared_timeout_constant`）。
//!    **另外 4 条是第二代的，不恢复**（它们测的是已经不存在的东西）：启动 URL 的两条
//!    （`startup_url` 随本地服务删除）与 `ShellExecuteW` 门限的两条（`browser.rs` 那条路
//!    规格 §1.2 废掉了）。它们留在 git 历史里，出处同上。
//!
//! ## ⚠️ 本模块与 `main.rs` 的分工
//!
//! `main.rs` 是**装配入口**（谁先谁后、失败时弹哪个框）；本模块只回答**决定**
//! （起哪一个内核、握手有没有回话）。这条分工是规格 §4.3 那条"能写出断言的代码一律搬出
//! 视图目标"的同形落地：`choose_core` 那样的纯函数在宿主上逐支可断言，而"真机上释放失败"
//! 要造出来得靠杀毒软件 —— 摆三种输入比造一台被杀软拦住的机器便宜得多。
//!
//! ## ⚠️ 内核调用**绝不在主线程上**（裁决 Z）
//!
//! `CoreClient::call` **没有每请求超时**（与 macOS 一致、刻意的：内核的 `load_delivery`
//! 最坏约 91.5 秒，给它套一个超时只会把"慢"判成"错"）。把这样一次调用放在**主线程**上的
//! 后果是**整个进程冻住**：窗口不再响应、`invoke` 也不再回话 —— 冻死是**沉默的**
//! （不报错、不留日志）。
//! ⇒ [`connect`] 与 [`spawn_core_job`] **都只许在后台线程上跑**（调用方负责把它放上去：
//! `main.rs` 的连接器由 `session::spawn_connect` 起的那条线程驱动）。
//!
//! ⚠️ 唯一的例外是 [`locate_core_binary`]：它**有副作用**（内嵌那一份的释放会真的写盘），
//!    但那是**有界**的（一次本地文件读写，不是内核调用那种可能永不返回的东西），
//!    而且它跑在**起 Tauri 之前**（那时还没有任何界面在看）。裁决 Z 管的是
//!    `CoreClient::call` 那类**无上界**的调用，这一处不是那一类。

// ⚠️ 这里**没有** `core_arguments`（任务 8 改）：argv 由调用方（`main.rs` 的连接器）
//    按当下的偏好算好交进来，本模块不猜"该传什么"。全 crate 里拼那一段 argv 的地方
//    只有两处、且是同一条路：`DownloadDirectory::core_arguments_for`（→ `client::core_arguments`）。
use shell_core::client::CoreClient;
use shell_core::presentation::error_text::error_text;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// 内核可执行文件名。Windows 上**必须带 `.exe`**（`CreateProcess` 对无扩展名文件的行为
/// 是推断、不是实测——规格 §7.2 第 2 条明确"不要赌"）。
const CORE_EXE_NAME: &str = "benagen-core.exe";

/// 握手超时（**唯一一份**；`AppModel.defaultHandshakeTimeout` 的对位物，控制者裁定 FF）。
///
/// ⚠️ **这个值与那句话是一对**：`HANDSHAKE_TIMEOUT_MESSAGE` 里写着「等待超过 5 秒」，
///    而那句文案同时是 `EngineStatusPresentation::is_handshake_timeout` 的**判别式**
///    （逐字相等）—— 两个数对不上，那句文案就是假话，而且改一个忘改另一个**不会有任何
///    东西变红**。钉住这对关系的是
///    `the_handshake_timeout_and_its_wording_agree`（下面 `mod tests`）。
///
/// ⚠️ 上游把这条耦合钉在**三处**（`AppModel.swift:215-216` 的"改一个就改另一个"、
///    `AppModelTimeoutTests.swift:329` 的 `defaultHandshakeTimeout == 5`、
///    `:337` 的"正文含 5 秒且与 defaultHandshakeTimeout 一致"）。本波次没有移植
///    `AppModel`，**它的家就定在这里**；文案的家在 `shell-core`（裁定 EE：不搬）。
///    两边都是 `const`，而 Rust 的 `const` 字符串**没法插值**另一个 `const`
///    ⇒ 这条配对只能靠**断言**守住，不能靠类型。这就是那条测试存在的全部理由。
///
/// ⚠️ 值取 **5 秒**：本阶段的要求是"报错与显示与功能与 macOS 完全一致"。
///
/// ⚠️ **R-15 搬回来时它从 bin 的 crate 根挪到了本模块**（原因见文件头）：名字与值一字未改，
///    改的只是它的路径。`main.rs` 起 Tauri 之前读的是它（通过连接器里那一发 `connect`）。
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 内核在哪。三条路，**顺序即优先级**：
///
///   ① **`BENAGEN_CORE` 环境变量** —— 排障时"临时换一个内核来试"（与 shell-core 的 e2e
///      同形）。⚠️ 它排在**内嵌那一份之前**，这是**有意的**（W-6，见下）；
///   ② **从壳自己里释放出来的那份**（`embed::extract_core()`）——
///      `%LOCALAPPDATA%\BenagenDownloader\cache\benagen-core-<sha12>.exe`。
///      客户机器上**只有**这条路会被走到（单文件分发，W-4）；
///   ③ **与壳同一个目录**下的 `benagen-core.exe` —— 人类伙伴手工验收时把两个 exe
///      放进同一个文件夹即可（探路包就是这么用的）。
///
/// ⚠️ **为什么 ① 排在 ② 前面**（简报里写的是"把内嵌那份排在第一位"，这是**有意偏离**，
///    理由如下）：释放这条路有一条**真实的**失败分支 —— 被杀毒软件拦掉
///    （规格 §7.2 要申报的风险、§14.3 的待定项）。把它排在第一位的话，那条分支一旦发生，
///    这台机器上**再没有任何出路**：① 永远轮不到。而 ① 是**排障用**的开关，
///    客户机器上本来就没有它（正常用户的环境里没这个变量）——
///    "交付形态默认走内嵌那一份"这件事没有变，变的是**还有没有一条逃生路**。
///    规格 §10 要求失败给出"可执行的补救"：`embedded_core` 的每一句失败话术里
///    写的补救**正是**这条路（"把内核放到程序旁边，或设 BENAGEN_CORE"），
///    它必须真的走得通。
///
/// **② 那一档失败时，退回 ③，并且把原因带出来**（W-2：不许静默降级）。
///
/// ⚠️⚠️ 这条是**审查抓出来的**（2026-09-19，重要 #2）：在此之前 ② 失败会**直接返回 `Err`**，
///    而三处失败话术写的补救恰恰是"把 benagen-core.exe 放到本程序所在目录…后点「重试」"
///    —— 用户照做、重试、得到**同一个错**（③ 永远轮不到）。那条补救是**假的**，
///    而它命中的正是规格 §7.2/§14.3 点名的**杀软拦截**那一条（唯一需要逃生路的分支）。
///    ⇒ 现在：② 失败**不再堵死后面那条路**，但**必须说出来**（`embedded_failure`
///    会被调用方放进 `Session::set_fallback_notice` —— **页面**顶部横幅区的**第二行**
///    就是它的落点）。
///    （阶段 A 修订：此处原文写的是 `ShellState::engine_fallback_notice` 与"主区"——
///     那个类型与那块界面都是 egui 时代的，已随阶段 A 删除；**代码一字未改**。）
///
/// ⚠️ **落点改过一次**（2026-09-19，集成树「关键 2」）：它本来写进 `last_error`，
///    而那一格是**非粘滞**的（上游 `lastError`：下一个成功请求就清）⇒
///    一次成功的 `verify_status` 或传输快照就会把这条 W-2 披露抹掉、且没有东西会把它写回来。
///    现在它住**自己那一格**（`engine_fallback_notice`，粘滞）。
///
/// 失败时返回 `Err(一句能直接显示的话)`，由调用方**大声报出来**
/// （不许静默地起一个没有内核的壳；阶段 A 修订：原文写的是"窗口"，现在没有窗口了）。
/// ⚠️ 都断了的时候报的是 **② 那条原因**（最具体），
/// 而不是一句笼统的"哪儿都找不到"。
///
/// ⚠️ 每次连接都会重查一遍（不是启动时查一次就固定）：人类伙伴的验收动作正是
///    "把内核放到壳旁边，然后点「重试」" —— 启动时定死的话，那条路就没有出口。
pub fn locate_core_binary() -> Result<CoreLookup, String> {
    // ⚠️ **副作用如实记账**：这一行会**真的执行释放**（首次写十几 MB、此后核一次哈希），
    //    它发生在**主线程**上。这是有界的（一次本地文件读写，不是内核调用那种可能
    //    永不返回的东西），而且只发生在**起 Tauri 之前**（那时窗口还没建起来、
    //    没有任何页面在看）。裁决 Z 管的是 `CoreClient::call` 那类**无上界**的调用；
    //    这里不是那一类。
    //    真要挪，得把它塞进后台线程再把"释放失败"那条回执接回来 —— 那会让 ①/②/③ 的
    //    次序在两条路上分叉。
    //    （阶段 A 修订：此处原文说的是"界面线程 / 窗口还没画出第一帧"——那是 egui 时代的
    //     说法；Tauri 那一代修订：界面回来了，但那一刻**窗口还没建起来**，
    //     这一点仍然成立。**代码一字未改**，改的只是这句说明。）
    let from_embedded = embedded_core();
    choose_core(
        std::env::var_os("BENAGEN_CORE")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        from_embedded,
        same_dir_candidate(),
    )
}

/// 一次内核查找的结局。
///
/// `Debug` 是给测试用的（`Result::expect_err` 要求 `T: Debug`）—— 生产路径不打印它。
///
/// ⚠️ **两个字段是 `pub`**（R-15 搬回来的唯一一处签名改动）：`main.rs` 要用它们
/// （`path` 交给连接器、`embedded_failure` 写进 `Session::set_fallback_notice`），
/// 而 bin 与 lib 是两个 crate。**字段名与含义一字未改。**
#[derive(Debug)]
pub struct CoreLookup {
    /// 最终选定要起的那个内核。
    pub path: PathBuf,
    /// **②（内嵌释放）那一档失败的原因** —— 只有"退回 ③"时才是 `Some`。
    ///
    /// ⚠️ 它**不是**给自己看的日志：调用方把它放进 `Session::set_fallback_notice`，
    ///    于是**页面**顶部横幅区会出现**第二行**（橙色 `?`，与引擎那条并列）。
    ///    没有它，"用了同目录那份内核"这件事在界面上**一个字都不会说**（W-2）。
    ///    ⚠️ 它**不**进 `last_error`（那一格是非粘滞的，会把它清掉）—— 见
    ///    `SessionView::fallback_notice` 的文档（`shell-core/src/session_view.rs`）。
    ///    （阶段 A 修订：原文两处写的是 `ShellState::engine_fallback_notice`、"主区"，
    ///     都是 egui 时代的说法；**代码一字未改**。）
    pub embedded_failure: Option<String>,
}

/// **内核查找的决策**（纯函数：三个"有没有"进、一个结论出）—— 宿主上可断言。
///
/// ⚠️ 做成纯函数是**刻意的形状**（与 `spawn.rs` 的 `creation_flags_for(os)`、
///    `platform.rs` 的 `env_var_name(platform)` 同源）：真机上"释放失败"要造出来得靠
///    杀毒软件，而这里可以把三种输入直接摆出来，逐支断言。
fn choose_core(
    from_env: Option<PathBuf>,
    from_embedded: Result<Option<PathBuf>, String>,
    same_dir_candidate: Option<PathBuf>,
) -> Result<CoreLookup, String> {
    // ① 环境变量：排障开关。
    if let Some(path) = from_env {
        return Ok(CoreLookup {
            path,
            embedded_failure: None,
        });
    }
    // ② 从壳自己里释放出来的那一份（客户机器上的默认路径）。
    let mut embedded_failure = None;
    match from_embedded {
        Ok(Some(path)) => {
            return Ok(CoreLookup {
                path,
                embedded_failure: None,
            })
        }
        // 宿主构建：**这条构建路径上没有**内嵌那一份（不是失败，见 `embedded_core`）。
        Ok(None) => {}
        // 释放失败（磁盘满 / 权限不足 / 被杀软拦）：**记下来，继续找 ③**。
        Err(why) => embedded_failure = Some(why),
    }
    // ③ 与壳同目录的那一份。
    if let Some(candidate) = &same_dir_candidate {
        if candidate.is_file() {
            return Ok(CoreLookup {
                path: candidate.clone(),
                embedded_failure,
            });
        }
    }
    // 三条都断了：报 **② 那条原因**（比"哪儿都找不到"具体得多 —— 它带着补救）。
    Err(embedded_failure
        .unwrap_or_else(|| core_not_found_message(same_dir_candidate.as_deref())))
}

/// ③ 的落点：与壳同一个目录下的 `benagen-core.exe`。
///
/// ⚠️ `current_exe()` 失败时返回 `None`（而不是 `Err`）：这条路**失败也要能往下走**
///    —— 它只是三条路里的一条，"问不到自己的路径"不该独占一个错误出口
///    （真的一条都没有时，`core_not_found_message` 会说清"找过哪儿"）。
fn same_dir_candidate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(CORE_EXE_NAME))
}

/// 内嵌内核那一支（**只有 Windows 靶有**）。
///
/// ⚠️ 宿主（macOS）上直接返回 `Ok(None)`：那种构建里**没有**内嵌那份字节
///    （`build.rs` 只在 Windows 靶产出它，见 `embed.rs` 文件头）。这不是"静默降级"——
///    宿主的产物不是交付形态；而**交付形态那一条没有开关**。
#[cfg(target_os = "windows")]
fn embedded_core() -> Result<Option<PathBuf>, String> {
    crate::embed::extract_core().map(Some)
}

#[cfg(not(target_os = "windows"))]
fn embedded_core() -> Result<Option<PathBuf>, String> {
    Ok(None)
}

/// 找不到内核时那句话。**壳自己写的**（没有内核，没有原文可登）。
///
/// 它是 `EngineState::Unavailable` 的载荷，所以会随 `state()` 命令原样发给页面
/// （`{"kind":"unavailable","reason":…}`，`shell-core/src/api/state.rs::engine_wire`）——
/// 页面把它画成顶部那条横幅（带「重试」，落点就是 `retry()` 那个命令）：
/// 约束 4 要的"失败必须出现在界面上"在这里就是这一处。
/// （阶段 A 修订：原文说的是"主区顶部那条横幅与空态页的失败分支"—— 那是 egui 那两个
/// 落点，已随阶段 A 删除；**代码一字未改**。Tauri 那一代修订：**页面**仍然在，
/// 只是那条横幅搬到 `windows/web/` 里去了，上面那两句里的 `/api/state` 与 `/api/retry`
/// 换成了今天的命令名 —— 那两处引用指向的文件已经不存在了，留着一个够不着的路径引用
/// 就是一次"以注释形式说假话"。）
///
/// ⚠️ 文案里点名了**第三个地方**（缓存目录）：走到这里说明内嵌那份没释放成功
///    （或者这台机器上的构建压根没有内嵌那份）。不说清"它本该在哪"，
///    用户就没有任何线索能对上那句话。
fn core_not_found_message(same_dir_candidate: Option<&Path>) -> String {
    let where_looked = same_dir_candidate
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "本程序所在目录".to_string());
    format!(
        "没找到内核：{CORE_EXE_NAME} 不在 {where_looked} 里，\
         %LOCALAPPDATA%\\BenagenDownloader\\cache\\ 里也没有释放成功的那一份，\
         环境变量 BENAGEN_CORE 也没有指向它。\n\
         补救：把 benagen-core.exe 放到本程序所在目录（或设 BENAGEN_CORE 指向它），\
         然后点「重试」。（放好之后点「重试」，那一份**一定查得到**。）"
    )
}

// ---------------------------------------------------------------------------
// 握手：有上界的一次内核调用
// ---------------------------------------------------------------------------

/// 握手与它的有界等待。
///
/// ## ⚠️ 这一段为什么是一个**独立的模块**：可见性是承重的（复审重要 #1 的残余项）
///
/// `call_with_deadline` 在本模块里是**私有**的（`fn`，不是 `pub(crate)`），而 Rust 的
/// 私有可见性**对父模块同样有效** —— 于是 `bootstrap.rs` 里**任何地方都调不到它**。
/// 唯一能发出 `hello` 的路就是 [`handshake`]，而它用的超时值只有一处。
///
/// 这堵的是一条**实测出来的真洞**：把 `connect` 里那句 `handshake(&client)` 换成
/// `call_with_deadline(&client, "hello", …, Duration::from_secs(3600))`（即"绕开常量内联一个
/// 别的值"），**原来的 170 条测试一条都不会红** —— 它们全都直接调 `handshake`，
/// `connect` 的正文完全在它们的射程之外。现在那种改法**根本编不过**（报告里贴了编译错误原文）。
///
/// ⚠️ 模块私有堵住的是"**调用** `call_with_deadline` 绕开它"；它堵不住"把它的正文抄一遍"。
///    后者不是漂移而是重写（任何测试都能被重写绕开），所以这里不追。
///
/// ⚠️ R-15 搬回来时这里改了**一处路径**：`crate::DEFAULT_HANDSHAKE_TIMEOUT` 现在是
///    `super::DEFAULT_HANDSHAKE_TIMEOUT`（它的家从 bin 的 crate 根挪到了本模块）。
///    代码、判据、文案都没动。
mod bounded_handshake {
    use super::DEFAULT_HANDSHAKE_TIMEOUT;
    use serde_json::json;
    use shell_core::client::CoreClient;
    use shell_core::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;
    use shell_core::presentation::error_text::error_text;
    use shell_core::protocol::PROTOCOL_VERSION;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    /// 握手（`hello`）。**上界就是 [`DEFAULT_HANDSHAKE_TIMEOUT`]，不接受别的值**。
    ///
    /// ⚠️ **这个函数刻意不带"超时值"参数**（形状上的选择，理由控制者裁定 FF）：一参数化，
    ///    "握手到底用了哪个值"就有了第二个答案，而**那个答案是测不出来的** ——
    ///    调用点改成任何数都不会有东西变红，正是 FF 要防的"改一个忘改另一个"。
    ///    把值钉死在函数体里（生产路径上它只出现这一次），钉住它的那条测试
    ///    （`the_handshake_itself_uses_the_shared_timeout_constant`）就能真的走到这条路上来。
    ///
    /// ⚠️ 它吃 `&Arc<CoreClient>`（不是 `&CoreClient`）：超时的逃生口要**换到另一条线程上**
    ///    去收尾，那里得有一份**活得比本次调用久**的引用（见 [`call_with_deadline`]）。
    pub(crate) fn handshake(client: &Arc<CoreClient>) -> Result<serde_json::Value, String> {
        call_with_deadline(
            client,
            "hello",
            json!({ "protocol": PROTOCOL_VERSION }),
            DEFAULT_HANDSHAKE_TIMEOUT,
        )
    }

    /// 有上界地等一次内核调用。**只在后台线程上跑**（它会阻塞）。
    ///
    /// ⚠️ **为什么需要它**：`CoreClient::call` 没有每请求超时（与 macOS 一致、刻意的，
    ///    理由见 `bootstrap.rs` 文件头），而**握手**是唯一一处壳自己定死了上界的地方。
    ///    做法：把 `call` 放到一条内层线程上，在外面 `recv_timeout` 等它。
    ///
    /// ⚠️ **耗时上界就是 `timeout`**（这条现在是真的，不是"差不多"）：
    ///    内层线程是 **detached** 的（不 join），超时的收尾也在**另一条线程**上 ——
    ///    两者都**不进**本次等待。之所以要这样：`ProcessChannel::close()` 在
    ///    "**kill 也收不掉**"时会**提前 return**（`client.rs:548-557`，那里如实记账了），
    ///    于是那次卡住的读永远不会以 EOF 收场。要是这里 `join` 它，**本函数就跟着无界**：
    ///    后台线程永不回话 ⇒ 界面永远停在「正在连接内核…」——那正是本项目最恨的
    ///    **沉默的挂住**（而且它与本文档上一版声称的"最坏 timeout + 5 秒"不符，复审点名）。
    ///    ⇒ 于是：**等**只等到超时值；**收尾**交给另一条线程，它卡住也不影响这个结论。
    ///
    /// ⚠️ **残余（如实记账）**：那条收尾线程在"内核既收不掉也杀不掉"的机器上会一直卡着，
    ///    并一直持着那个连接的最后一个引用（于是那个内核进程也不回收）。这是**极端**情形
    ///    （kill 失败），代价被限制在"泄漏一条线程 + 一个进程"，换的是"界面一定会有结论"。
    ///
    /// ⚠️ 超时时返回的那句话**就是** `HANDSHAKE_TIMEOUT_MESSAGE`（**逐字**）：
    ///    它同时是 `EngineStatusPresentation::is_handshake_timeout` 的判别式，
    ///    界面靠它决定"该去点授权框"还是"照登内核原文"。往里拼别的东西（比如实际的
    ///    超时秒数）会让判别式失配 —— 上游对这一条有专门的叮嘱（`AppModel.swift:234-236`）。
    fn call_with_deadline(
        client: &Arc<CoreClient>,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        // ⚠️ `method` 要**拥有**（detached 的线程要求 `'static`），所以在这里转成 `String`；
        //    多留一份 `label` 给下面那句错误话术用（`method` 已经被 move 进闭包）。
        let method = method.to_string();
        let label = method.clone();
        let (tx, rx) = mpsc::channel();
        let inner = Arc::clone(client);
        std::thread::Builder::new()
            .name(format!("core-{method}"))
            .spawn(move || {
                // 接收端可能已经走了（超时之后我们就返回了）：`send` 失败不是错误，丢掉即可。
                let _ = tx.send(inner.call(&method, params));
            })
            .map_err(|cause| super::cannot_spawn_thread_message(&cause))?;

        match rx.recv_timeout(timeout) {
            Ok(Ok(value)) => Ok(value),
            // 内核报的错按**唯一映射**（规格 §10）变成一句话：内核原文逐字。
            Ok(Err(error)) => Err(error_text(&error)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // **逃生口**：收掉这条通道。它一收，里面那条卡住的读以 EOF 收场。
                // 它**在另一条线程上**跑（理由见上面那段"耗时上界"）。
                let escape = Arc::clone(client);
                let _ = std::thread::Builder::new()
                    .name("core-shutdown".to_string())
                    .spawn(move || escape.shutdown());
                Err(HANDSHAKE_TIMEOUT_MESSAGE.to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "等内核回 `{label}` 的那条线程没送回任何东西就结束了（它 panic 了吗？）。\
                 补救：把这条原样发给我们。"
            )),
        }
    }

    #[cfg(test)]
    mod tests {
        //! 控制者裁定 FF 的那条配对（超时值 ↔ 那句话里的秒数）与"这条等待真的有上界"。
        //!
        //! ⚠️ 它们住在这里而不是 `bootstrap.rs` 的测试模块里：**只有这里看得见**
        //!    `call_with_deadline`（它对父模块是私有的，见模块头 —— 那是承重的可见性）。
        //!    在这个模块之外，能发 `hello` 的路只有 [`handshake`]。

        use super::*;
        use shell_core::client::LineChannel;
        use shell_core::presentation::engine_status::EngineStatusPresentation;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Instant;

        /// 一条"内核不说话了"的假通道：`read_line` 会一直阻塞，直到 `close()` 被调用。
        ///
        /// 真实现里那个"解阻塞"来自"子进程被杀 ⇒ 管道 EOF"（`ProcessChannel::close`），
        /// 这里用一面旗子对位。
        ///
        /// ⚠️ 它有一个**上界**（[`WAIT_CAP`]）：万一有人把超时那条路去掉（改回阻塞的 `recv`、
        ///    或者忘了收通道），这条用例**不会**永远挂着 —— 它会在上界之后收到 EOF、
        ///    拿到一句 `KernelGone`，于是断言变红。**红**，不是挂住：挂住会毒掉整个套件，
        ///    而"挂了"在 CI 上比"红了"难查得多。
        #[derive(Clone)]
        struct Silent {
            closes: Arc<AtomicUsize>,
        }

        /// 假通道自己的上界。它必须**远大于**被测的超时值（否则测的就不是超时了），
        /// 又**远小于**"一条用例永远不结束"（否则失败形态就是挂住）。
        const WAIT_CAP: Duration = Duration::from_secs(15);

        /// 断言"握手用的就是那个常量"时允许的余量。
        ///
        /// ⚠️ **上界必须锚在常量上**（复审重要 #1）：只钉下界的话，"握手实际等了 6 秒、
        ///    文案说 5 秒"——恰恰是 FF 存在要防的情形——不会有任何东西红。
        ///    实测本机这一条是 **5.01 s**（收尾已经不在本次等待里了），
        ///    500 ms 的余量足够抓 6 秒那种漂移，又不会被调度抖动误伤。
        const UPPER_SLACK: Duration = Duration::from_millis(500);

        impl Silent {
            fn new() -> Self {
                Silent {
                    closes: Arc::new(AtomicUsize::new(0)),
                }
            }
            fn closes(&self) -> usize {
                self.closes.load(Ordering::SeqCst)
            }
            /// 等逃生口真的被用上（它是**异步**的：收尾在另一条线程上，见
            /// `call_with_deadline` 的文档）。有上界，所以不会挂住。
            fn wait_for_close(&self, within: Duration) -> bool {
                let deadline = Instant::now() + within;
                while self.closes() == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                self.closes() >= 1
            }
        }

        impl LineChannel for Silent {
            fn write_line(&self, _line: &str) -> Result<(), shell_core::client::ClientError> {
                Ok(())
            }
            fn read_line(&self) -> Result<Option<String>, shell_core::client::ClientError> {
                let deadline = Instant::now() + WAIT_CAP;
                while self.closes() == 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) // EOF：收尾之后读到的就是这个
            }
            fn close(&self) {
                self.closes.fetch_add(1, Ordering::SeqCst);
            }
        }

        /// **FF 的落点（一）：文案里的秒数必须等于壳实际用的超时值。**
        ///
        /// 上游把这条耦合钉在三处（`AppModel.swift:215-216` 的"改一个就改另一个"、
        /// `AppModelTimeoutTests.swift:329` 的 `defaultHandshakeTimeout == 5`、
        /// `:337` 的"正文含 5 秒且与 `defaultHandshakeTimeout` 一致"）。
        /// 本波次没有移植 `AppModel`：超时值的家是 `DEFAULT_HANDSHAKE_TIMEOUT`（`bootstrap.rs`），
        /// 文案的家是 `shell-core` 的 `HANDSHAKE_TIMEOUT_MESSAGE`。
        /// 两边都是 `const`，而 Rust 的 `const` 字符串**没法插值**另一个 `const`
        /// ⇒ 这条配对只能靠断言守住 —— 就是这一条。
        ///
        /// 判别力（**两个方向都会红**，都实测过）：
        ///   * 把 `DEFAULT_HANDSHAKE_TIMEOUT` 改成 6 秒 ⇒ 本用例红；
        ///   * 把 `engine_status.rs` 里那句的「5 秒」改成「6 秒」⇒ 本用例红。
        /// "改一个忘改另一个"没有别的东西会红，这就是唯一的那道网。
        #[test]
        fn the_handshake_timeout_and_its_wording_agree() {
            assert_eq!(
                DEFAULT_HANDSHAKE_TIMEOUT,
                Duration::from_secs(5),
                "本阶段的要求是「与 macOS 完全一致」⇒ 5 秒（上游 `defaultHandshakeTimeout`）"
            );
            assert_eq!(
                HANDSHAKE_TIMEOUT_MESSAGE,
                format!(
                    "内核无响应（等待超过 {} 秒）",
                    DEFAULT_HANDSHAKE_TIMEOUT.as_secs()
                ),
                "这句文案里的秒数与壳的握手超时值对不上 —— 那句话就是假话"
            );
            // 顺带把"这句话同时是判别式"也钉住：超时时写进界面的就是它本身（逐字）。
            assert!(
                EngineStatusPresentation::is_handshake_timeout(HANDSHAKE_TIMEOUT_MESSAGE),
                "超时那句必须被判别式认出来（否则界面会把它当内核原文，丢掉该点什么的那句补充）"
            );
        }

        /// **FF 的落点（二）：超时**真的会发生**，而且它写进界面的就是那句话。**
        ///
        /// 上一条只钉"两个常量一致"；那一条**不能**证明调用路径真的走了这个值。
        /// 这一条补上那一半：拿一个**永不回话、但能被 `shutdown` 收掉**的假通道跑一遍，
        /// 断言三件事：
        ///   ① 它在**超时值附近**回来（不是永远等下去，也不是立刻返回）；
        ///   ② 回来的那句话**就是** `HANDSHAKE_TIMEOUT_MESSAGE`；
        ///   ③ 它**用了那条逃生口**（`close` 被调到）。
        ///
        /// ⚠️ 它给的是一个**很短的**超时值（80 ms）：这一条管的是**机制**，不是"生产用了哪个值"
        ///    —— 后者是下面那条（`the_handshake_itself_uses_the_shared_timeout_constant`）。
        #[test]
        fn a_silent_kernel_times_out_into_the_message_the_ui_shows() {
            const TIMEOUT: Duration = Duration::from_millis(80);
            let stub = Silent::new();
            let client = Arc::new(CoreClient::new(Box::new(stub.clone())));

            let started = Instant::now();
            let result = call_with_deadline(&client, "hello", json!({ "protocol": 1 }), TIMEOUT);
            let took = started.elapsed();

            // ② 界面上那句话（**逐字**，判别式认得它）。
            assert_eq!(
                result,
                Err(HANDSHAKE_TIMEOUT_MESSAGE.to_string()),
                "超时之后必须交回那句固定的文案（往里拼实际秒数会让判别式失配）"
            );
            // ① 它等的是那个上界，不是"永远"、也不是"立刻"。
            //    ⚠️ 两边都要钉：下界抓"用了更小的值"，**上界抓"用了更大的值"**。
            assert!(
                took >= TIMEOUT,
                "没等满超时值就回来了（{took:?} < {TIMEOUT:?}）：那就不是「有上界的等待」"
            );
            assert!(
                took < TIMEOUT + UPPER_SLACK,
                "等了 {took:?}（超时值 {TIMEOUT:?} + 余量 {UPPER_SLACK:?} 都打不住）\
                 ⇒ 这个等待的长度不是它自己的超时值决定的"
            );
            // ③ 逃生口真的被用了：不收掉通道，那条请求会永远卡在内核的串行队列里。
            assert!(
                stub.wait_for_close(Duration::from_secs(1)),
                "超时之后必须收掉这条通道 —— 那是 `call` 没有超时时唯一被认可的逃生口"
            );
        }

        /// **FF 的落点（三）：握手**自己**用的就是那个常量（不是"机制在、没人用"）。**
        ///
        /// ⚠️ 这是本套件里**最慢的一条**（≈ `DEFAULT_HANDSHAKE_TIMEOUT`，5 秒）：它必须让
        ///    **真实的超时值**到点，任何"把值换小一点"的做法都会让这条用例失去意义。
        ///    代价换来的是一句话有断言：**"握手走的是 `handshake`，而它用的就是
        ///    `DEFAULT_HANDSHAKE_TIMEOUT`"**。
        ///
        /// **两条边界都要**（复审重要 #1 的正面）：下界抓"改成更小的值"，
        /// **上界抓"改成更大的值"** —— 后者恰恰是 FF 存在要防的那一幕
        /// （"握手实际等了 6 秒、文案说 5 秒"），而只钉下界的版本对它**全绿**：
        /// 实测把 `handshake` 的实参改成 `Duration::from_secs(6)` 时，
        /// 加上这条上界之前 12 条用例一条都不红。
        ///
        /// 它同时钉住了另一半：超时之后通道**被收掉**（否则那条请求会永远卡在 FIFO 队列里，
        /// 而界面上没有任何东西会变）。
        #[test]
        fn the_handshake_itself_uses_the_shared_timeout_constant() {
            let stub = Silent::new();
            let client = Arc::new(CoreClient::new(Box::new(stub.clone())));

            let started = Instant::now();
            let result = handshake(&client);
            let took = started.elapsed();

            assert_eq!(
                result,
                Err(HANDSHAKE_TIMEOUT_MESSAGE.to_string()),
                "握手遇到一个不说话的核时时，交回界面的必须是那句话"
            );
            assert!(
                took >= DEFAULT_HANDSHAKE_TIMEOUT,
                "握手在 {took:?} 就回来了 —— 比它自己的超时值还短，说明它用的**不是**这个值"
            );
            assert!(
                took < DEFAULT_HANDSHAKE_TIMEOUT + UPPER_SLACK,
                "握手等了 {took:?}，超过 常量 + {UPPER_SLACK:?} ⇒ 它用的**不是**那个常量\
                 （这正是「握手等了 6 秒、文案说 5 秒」那一幕）"
            );
            assert!(
                stub.wait_for_close(Duration::from_secs(1)),
                "超时之后必须收掉这条连接（那条请求已经永久堵住了内核的串行队列）"
            );
        }
    }
}

/// 走一遍真连接：起内核子进程 → `hello`（**有上界的等待**）→ 拿回执。
///
/// ⚠️ **本函数只在后台线程上跑**（裁决 Z，见文件头）。成功时把连接**交出去**
///    （`Arc<CoreClient>`），由壳持有到进程退出 —— 后续页面（文件 / 传输列表 / 校验）
///    必须在**同一个内核**上问：换一个内核就是换掉正在跑的引擎。
///
/// 失败时那个连接在**本线程上**被丢掉（`CoreClient::drop` = `shutdown()`，最坏 5 秒）——
/// 这正是"不许在 UI 线程上丢连接"那条纪律的落点。
///
/// ⚠️ **`hello` 只能经 `bounded_handshake::handshake` 发**：见那个模块的头
///    （那里把"绕开那个常量"这条改法**变成了编译错误**）。
///
/// ## ⚠️ `arguments` 为什么是**参数**（任务 8 改的签名，R-15 之外的第四类改动）
///
/// 这一段最初（第二代）把 argv 写死成 `core_arguments(None, None)` —— 也就是
/// **永远不传 `--download-dir`**（当时壳还没有"下载目录"这个偏好）。
/// 任务 8 让"改下载目录"真的生效了，而下载目录**只在 argv 上**（协议里没有这个键）
/// ⇒ 起内核时必须把**当时**那一份偏好接成 argv。
///
/// ⚠️ **argv 由调用方给、而不是在这里读偏好**：本函数是"起一个内核并握上手"，
///    而"用哪个目录"是**比它更高一层**的决定（偏好文件在哪、取不到时怎么办，
///    都是装配层的事，与"怎么起内核"无关）。这条分工与 `spawn::spawn_core`
///    收 `&[String]` 是同一个形状。
///
/// ⚠️ **每次调用都要现算一遍 argv**（调用方负责）：`retry()` 的语义是"重新找一次内核"，
///    而用户可能刚在设置里改过目录 —— 把 argv 缓存下来会让那次改动对重试失效。
pub fn connect(exe: &Path, arguments: &[String]) -> Result<Connected, String> {
    let client = Arc::new(
        crate::spawn::spawn_core(exe, arguments).map_err(|e| error_text(&e))?,
    );
    let reply = bounded_handshake::handshake(&client)?;
    Ok(Connected {
        client,
        // 内核回执的**原文**（JSON 一行）。壳不改写内核的话（规格 §10）——
        // 它同时是"整条架构通了"的那条证据（探路就是这么验的，spike §6 第 3 条）。
        handshake_reply: reply.to_string(),
    })
}

/// 一次成功连接的产物。
///
/// ⚠️ **两个字段是 `pub`**（R-15 搬回来的唯一一处签名改动，理由同 [`CoreLookup`]）：
/// `main.rs` 里的连接器要把它们分别交给 `Session::install_client`。**含义一字未改。**
pub struct Connected {
    /// 活着的内核连接。
    pub client: Arc<CoreClient>,
    /// `hello` 回执的原文（JSON 一行，全 ASCII）。
    pub handshake_reply: String,
}

/// 起一条后台线程跑一次内核交互（"**后台线程 + 通道**"那一个形状）。
///
/// ⚠️ **它目前零调用者**（阶段 A 修订：原文写着"任务 18/19/20：你们的内核调用走这里"
///    并指向 `views/*.rs` 里现成的三处调用点 —— **那些视图与那些调用点都已随阶段 A
///    删除**。Tauri 那一代修订：线上真正跑内核的是**命令层**（`commands.rs` 的七个命令，
///    每个都在 Tauri 的 `invoke` 线程上）与 `session.rs` 的 `spawn_load` / `spawn_connect`，
///    没有一条经本函数）。
///    **代码一字未改**：本任务的简报点名要保留它（连同下面那条用例），保留的是**代码**，
///    不是"它还有人用"这个已经不成立的说明。它的去留是一次待裁定的决定，
///    别把它当成"有人在用"的接口：**加调用点之前先确认那道闸（无上界调用不许在主线程上）
///    在你这条路上是怎么被判的**。
///
/// ⚠️ **它保证的是什么**：闭包**跑在另一条线程上**，值从通道送回（裁决 Z 的形状：无上界的
///    内核调用绝不许跑在主线程上 —— 冻死是沉默的，不报错、不留日志）。
///    它**不**保证"谁来收那个值"：消费者那一侧的形状由调用方定（在本文件的历史形状里是
///    "每帧 `try_recv()` 一次"；在服务端那一侧更自然的写法是在当前请求线程上 `recv`）。
///
/// 起不了线程时返回一句**能直接显示**的话（W-2：不许静默降级成"那就同步跑吧"——
/// 同步跑正是不变量禁止的那件事）。
pub fn spawn_core_job<T, F>(name: &str, job: F, tx: Sender<T>) -> Result<(), String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let _ = tx.send(job());
        })
        .map(|_| ())
        .map_err(|cause| cannot_spawn_thread_message(&cause))
}

/// 「起不了后台线程」那句话。
///
/// ⚠️ **抽成纯函数是为了让它可断言**（本 crate 里唯一能写测试的那一类）：真的把
///    `Builder::spawn` 弄失败只有"资源耗尽"一条路，测不出来。生产路径与测试调的是
///    **同一个函数**，所以"这句话长什么样"只有一处。
/// ⚠️ 刻意**不写 `_ =>`/兜底措辞**：它必须同时说清**根因**（起不了线程）与**下一步**
///    （W-2：大声失败 + 可执行的补救），而不是一句"出错了"。
///
/// ⚠️ **原样保留的是代码，不是已经不成立的说明**：这句话的正文原先写着"内核调用绝不能
///    在**界面线程**上跑（那会让**窗口冻死**）" —— 阶段 A 之后壳里既没有"界面线程"这个
///    角色也没有"窗口"（界面搬进了系统浏览器），那半句说的是**假话**。改后这半句说的是
///    今天的形状：调用方是 Tauri 的 `invoke` 线程（`commands.rs` 的七个命令，即第二代
///    那条 HTTP 每连接线程的同位物）、连接线程（`session::spawn_connect`）与启动装配的
///    主线程，而 `CoreClient::call` **没有每请求超时**（最坏约 91.5 秒，见文件头）
///    ⇒ 它占住哪条线程，那一侧就一起停摆。
///    而且它反过来**更贴切了**：Tauri 那一代界面又回到了进程里 —— 一条 `invoke` 线程
///    被按住，前端那一拍就没有回话，而那是可以看得见的"卡住"。
///    **根因**（起不了线程）与**可执行的补救**（把这条发回来）两样都还在（W-2）。
fn cannot_spawn_thread_message(cause: &std::io::Error) -> String {
    format!(
        "起不了后台线程（{cause}）：这次内核调用没有线程可跑 —— 它绝不能在调用方的线程上跑\
         （`CoreClient::call` 没有每请求超时，一次最坏约 91.5 秒，占住哪条线程那一侧就一起停摆）。\n\
         补救：把这条原样发给我们。"
    )
}

#[cfg(test)]
mod tests {
    //! 本模块自己的测试：**内核查找那三条路 + `spawn_core_job` 那个形状**。
    //!
    //! ⚠️⚠️ **这一组是 R-15 从 git 历史里恢复的**（原文出处见文件头）：
    //!    `git show 6f49010:windows/shell-win/src/main.rs` 的 `mod tests` 里那一组
    //!    （`fake_core_file` + 五条）。它们断的全是**本模块**的函数，而任务 1 把
    //!    `main.rs` 整个换掉时，那些函数**连同它们的判据**一起离开了工作区。
    //!
    //! ⚠️ **恢复时改的只有三类**（**断言的判据一个字没动**，见文件头）：
    //!    ① 指向 `ShellState::…` / "主区横幅" 的说法改成阶段 A 之后的名字
    //!    （`Session::set_fallback_notice`、页面横幅）；
    //!    ② `spawn_core_job` 那条的文档里"任务 18/19/20 要用的那条路"改成了事实
    //!    （它现在仍然零调用者，读者换成了 Tauri 的命令层）；
    //!    ③ 消息文本里已经没有"界面线程"这个说法了（`cannot_spawn_thread_message` 的
    //!    正文从来没有过它，只有它的**文档**有过 —— 见那个函数自己的文档）。
    //!
    //! ⚠️ **没有恢复的 4 条**（它们是第二代的，测的东西已经不存在）：
    //!    `the_startup_url_is_the_one_the_spec_names` / `the_url_is_loopback_only`
    //!    （随本地 HTTP 服务一起删）、`the_shell_execute_success_threshold_is_greater_than_thirty_two`
    //!    / `the_host_never_claims_the_browser_was_launched`（随 `browser.rs` 一起删，
    //!    规格 §1.2 废掉了"把 URL 交给系统浏览器"那条路）。

    use super::*;
    // ⚠️ 只有这一条用例（`spawn_core_job_…`）要建通道：放这一层而不是文件顶，
    //    否则非测试构建会报一条 `unused_imports`（零告警口径）。
    use std::sync::mpsc;

    /// 造一个**真的存在**的假内核文件（只在这几条用例里用）。
    ///
    /// 返回值是那个文件的路径；**调用方负责删掉它** —— 下面每条用例都在**自己的最后一行**
    /// 显式 `std::fs::remove_file`（⚠️ Rust **没有** `defer`：这里不写，就没有任何东西会删它，
    /// 而且用例**失败时**也不会删 —— 那会往 `%TEMP%` 里漏一个文件，是**有意接受**的代价，
    /// 因为"删不掉"的失败噪音会把真正的断言失败淹掉）。
    fn fake_core_file(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "benagen-choose-core-{tag}-{}.exe",
            std::process::id()
        ));
        std::fs::write(&path, b"MZ fake").expect("写假内核");
        path
    }

    /// ⭐ **② 释放失败 ⇒ 退回 ③（同目录那份内核），而且"退回"这件事被说出来**。
    ///
    /// ⚠️ 这一条是**审查抓出来的洞**（2026-09-19，重要 #2）的回归网：在此之前 ② 失败会
    ///    **直接返回 `Err`**，于是三处失败话术里那句"把 benagen-core.exe 放到本程序所在
    ///    目录…后点「重试」"是**假的** —— 用户照做、重试、得到同一个错（③ 永远轮不到）。
    ///    命中它的正是规格 §7.2/§14.3 点名的**杀软拦截**（唯一需要逃生路的那一支）。
    ///
    /// 判别力（两半都钉住）：
    ///   * **① 退回确实发生**：返回的路径就是同目录那一份；把这条逻辑改回 `?` 直接返回
    ///     `Err`，这条用例立刻红；
    ///   * **② 不许静默**：`embedded_failure` 必须是 `Some` —— 若只是"悄悄退回"，
    ///     `main.rs` 就没有东西可写进 `Session::set_fallback_notice`，
    ///     页面上横幅区那第二行会消失（W-2 的静默降级）。
    #[test]
    fn a_failed_release_falls_back_to_the_kernel_next_to_the_exe_and_says_so() {
        let fallback = fake_core_file("fallback");
        let why = "释放内嵌内核失败：磁盘满。补救：清理空间后点「重试」".to_string();

        let lookup = choose_core(None, Err(why.clone()), Some(fallback.clone()))
            .expect("第二条路失败、但同目录那一份在场 ⇒ 必须成功落回它");
        assert_eq!(
            lookup.path, fallback,
            "退回的必须是**同目录那一份** —— 那正是失败话术里写的补救"
        );
        assert_eq!(
            lookup.embedded_failure.as_deref(),
            Some(why.as_str()),
            "退回同目录那份时**必须**把释放失败的原因带出来（它会被写进 \
             `Session::set_fallback_notice`，在页面顶部横幅区画成第二行）：\
             只退回不说，就是 W-2 禁止的静默降级"
        );

        let _ = std::fs::remove_file(&fallback);
    }

    /// ② 失败、③ 也不在场 ⇒ 报的是 **② 那条原因**（带着补救），不是一句笼统的"没找到"。
    ///
    /// 判别力：把 `Err` 那一支改成 `core_not_found_message(...)`，这一条立刻红 ——
    /// 而它会把用户引向"去找一个根本不存在的东西"，丢掉"磁盘满了/被杀软拦了"这条真原因。
    #[test]
    fn a_failed_release_with_nothing_to_fall_back_to_reports_the_release_failure() {
        let missing = std::env::temp_dir().join(format!(
            "benagen-choose-core-absent-{}.exe",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);

        let why = "释放内嵌内核失败：杀毒软件把这次写入拦下来了".to_string();
        let error = choose_core(None, Err(why.clone()), Some(missing))
            .expect_err("三条都断了就必须失败");
        assert_eq!(
            error, why,
            "都断了的时候，报的必须是**最具体的那条原因**（它带着补救），而不是'哪儿都找不到'"
        );
    }

    /// 三条路的**先后次序**：① 环境变量 → ② 内嵌释放 → ③ 同目录。
    #[test]
    fn the_lookup_order_is_env_then_embedded_then_next_to_the_exe() {
        let env = PathBuf::from("/nonexistent/env-kernel.exe");
        let embedded = PathBuf::from("/nonexistent/embedded-kernel.exe");
        let fallback = fake_core_file("order");

        // ① 在场 ⇒ 赢（哪怕 ② 也成功、③ 也在场）。
        let picked = choose_core(
            Some(env.clone()),
            Ok(Some(embedded.clone())),
            Some(fallback.clone()),
        )
        .expect("环境变量在场时必须成功");
        assert_eq!(picked.path, env);
        assert!(picked.embedded_failure.is_none(), "环境变量赢了就不算'退回'");

        // ① 不在 ⇒ ② 赢（③ 在场也不看）。
        let picked = choose_core(None, Ok(Some(embedded.clone())), Some(fallback.clone()))
            .expect("第二条在场时必须成功");
        assert_eq!(picked.path, embedded);
        assert!(picked.embedded_failure.is_none(), "第二条成功就没有失败可说");

        // ① ② 都不在（宿主：`Ok(None)`）⇒ ③。
        let picked = choose_core(None, Ok(None), Some(fallback.clone()))
            .expect("宿主构建上同目录那份是主路径");
        assert_eq!(picked.path, fallback);
        assert!(
            picked.embedded_failure.is_none(),
            "`Ok(None)` 是**这条构建路径上没有内嵌那一份**（宿主），不是失败 —— \
             它不该在界面上说一句"
        );

        let _ = std::fs::remove_file(&fallback);
    }

    /// **三条路都断了**时那句原因：说清"找过哪三处"，并给出可执行的补救（W-2 / 约束 4）。
    ///
    /// 判别力：这句话是**内嵌那一份落地之后**才变成主路径的（客户机器上只有内嵌那一份），
    /// 而它最容易退化成一句"没找到内核" —— 那样的话用户既不知道**找过哪儿**、
    /// 也不知道**该怎么办**。这条用例把三处与补救都钉住。
    #[test]
    fn the_not_found_message_names_all_three_places_and_a_remedy() {
        let candidate = PathBuf::from("C:/apps/Benagen/").join(CORE_EXE_NAME);
        let message = core_not_found_message(Some(&candidate));

        // ⚠️ 这条补救**必须真的走得通**（重要 #2 的教训）：放好内核之后点「重试」，
        //    ③ 这条路一定查得到 —— 那句话里的承诺与 `choose_core` 的行为是一对，
        //    上面那三条用例钉的就是另一半。
        assert!(
            message.contains("一定查得到"),
            "那句话要说明白：放好之后再点重试**真的会**查到它（不让用户白试一次）：{message}"
        );

        assert!(
            message.contains(CORE_EXE_NAME),
            "要点名那个文件名（用户得知道去找什么）：{message}"
        );
        assert!(
            message.contains("apps"),
            "要点名**找过的那一处**（本程序所在目录）：{message}"
        );
        assert!(
            message.contains("LOCALAPPDATA") && message.contains("cache"),
            "要点名内嵌那份本来会落在哪（内嵌那一份的主路径）：{message}"
        );
        assert!(
            message.contains("BENAGEN_CORE"),
            "要点名那个环境变量（排障时的口子）：{message}"
        );
        assert!(
            message.contains("补救") && message.contains("benagen-core.exe"),
            "W-2：必须给出**可执行的**下一步（把内核放到程序旁边）：{message}"
        );
        assert!(
            message.contains("重试"),
            "补救的最后一步是点「重试」（页面顶部那条横幅上就有）：{message}"
        );
    }

    /// `spawn_core_job` 必须**真的把闭包跑在另一条线程上**并把值送回来。
    ///
    /// ⚠️ 阶段 A 修订：它现在**零调用者**（读者曾是 egui 那套视图，已删）。
    ///    原文那句"任务 18/19/20 要用的那条路"**已经不成立**，保留的是**代码**、
    ///    不是那句话；这条判据断的是它自己的行为（"后台线程 + 通道"这个形状没有被写坏）。
    #[test]
    fn spawn_core_job_runs_the_closure_off_the_calling_thread_and_sends_the_value() {
        let (tx, rx) = mpsc::channel();
        let caller = std::thread::current().id();
        spawn_core_job(
            "test-job",
            move || std::thread::current().id() != caller,
            tx,
        )
        .expect("起线程不该失败");
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).expect("必须有回执"),
            "闭包必须跑在**另一条**线程上（裁决 Z：无上界的内核调用绝不许在主线程上跑）"
        );

        // 起不来时那句话必须是**能直接显示**的（W-2：不许静默降级成"那就同步跑吧"）。
        // ⚠️ 这里调的是生产路径用的**同一个函数**（真的把 `Builder::spawn` 弄失败只有
        //    "资源耗尽"一条路，测不出来）；判别力的边界如实记下：把生产路径里那个
        //    `.map_err(...)` 换成别的消息，这一条**不会**红 —— 它钉的是"这句话长什么样"。
        let message = cannot_spawn_thread_message(&std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "资源暂时不可用",
        ));
        assert!(
            message.contains("起不了后台线程") && message.contains("绝不能") && message.contains("补救"),
            "失败话术要说清根因与补救：{message}"
        );
    }
}
