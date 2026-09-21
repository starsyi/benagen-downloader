//! `shell-win` —— Windows 壳的**装配入口**（规格 §3.3、§7、§9.0）。
//!
//! ## 进程模型（规格 §3.3 的六步，本任务落地前五步）
//!
//! ```text
//! ⓪ single_instance::acquire()  已经有一份在跑 ⇒ 说一句话然后退出（规格 D-3）
//! ① BenagenDownloader.exe 启动
//! ② wv2::install()      把内嵌的 WebView2Loader.dll 释放出来并装载（规格 §4）
//! ③ bootstrap::locate_core_binary()  内核在哪三条路（env → 内嵌释放 → 同目录）
//! ④ session.set_connector(…)         装那个"重新找一次 + 握手"的连接器
//! ⑤ tauri::Builder      起 Tauri：建窗口、挂 webview、装 Session、注册全部命令、
//!                       再在起之前发一趟后台连接（`session::spawn_connect`）
//! ⑥ 关窗 ⇒ shutdown（**任务 8 落地**：显式 + 幂等，规格 §3.3 第 6 条）
//! ```
//!
//! ⚠️ **第 ⑥ 步是任务 8 落的**（任务 7 的版本在这里留了一句"这是任务 8 的事"）：
//!    判据是 `WindowEvent::CloseRequested` —— **一条显式事件**，不是第二代那套
//!    "页面心跳没了 ⇒ 猜窗口关掉了"的推断（规格 §1.2 把那一整套废掉了）。
//!    `shutdown()` 的两次调用点（关窗那一刻、`run()` 返回之后）都走同一个函数，
//!    而它是**幂等**的 —— 第二次什么都不会做（这正是"幂等"这条要求在结构上的落点）。
//!
//! ## 与第二代（阶段 A）的差别：**本文件删掉了整整一台装配机**
//!
//! 第二代这里是"内核 → 连接器 → 本地 HTTP 服务 → 连内核 → 找浏览器 → 心跳看门狗"
//! 六步装配。规格 §1.2 把那套废掉了：**Tauri 自己就是宿主**（前端由 `tauri://localhost`
//! 提供，只有本进程的 webview 能调 `invoke`）⇒ 没有监听端口、没有一次性 token、
//! 没有心跳、没有看门狗、没有浏览器进程、没有"关窗"这件事的推断。
//!
//! ## ⚠️ R-15：那批**不属于传输层**的东西回来了（本任务最要紧的一件事）
//!
//! 任务 1 把第二代传输层整个删掉时，**把当时的 `main.rs` 整个换掉了** —— 于是
//! `locate_core_binary` / `choose_core` / `CoreLookup` / `CORE_EXE_NAME` /
//! `core_not_found_message` / `same_dir_candidate` / `cannot_spawn_thread_message` /
//! `bounded_handshake`（含 `DEFAULT_HANDSHAKE_TIMEOUT`）连同 12 条装配用例一起离开了
//! 工作区。它们**不是传输层**（"内核在哪"与"握手有没有回话"与换哪一代 UI 无关），
//! 控制者据此裁定 R-15：**从 git 历史里原样搬回来**。
//!
//! ⇒ 它们的家现在是 **`shell_win::bootstrap`**（本文件不再是它们的家，因为 bin 里的
//!    私有函数**没法单测**，而那些函数里有一整套断言 —— 搬进 lib 正是规格 §4.3
//!    那条"能写出断言的代码一律搬出视图目标"）。搬回来的清单、原文出处、
//!    "哪 8 条用例回来了、哪 4 条没回来"都写在 `bootstrap.rs` 的文件头。
//!    本文件现在只做**装配**（谁先谁后、失败时弹哪个框）。
//!
//! ⚠️ **没回来的那 4 条是第二代的**：启动 URL 的两条（随本地 HTTP 服务删除）与
//!    `ShellExecuteW` 门限的两条（`browser.rs` 那条路，规格 §1.2 废掉了）。
//!
//! ## 四个 Windows 专有落点（第 ④ 条是任务 8 加的）
//!
//! | # | 落点 | 落在这份代码的哪里 | 本机能验到什么 |
//! |---|---|---|---|
//! | ① | 壳自己的控制台窗口 | 下面那个 `windows_subsystem` attribute | `file` 抽查产物是 GUI 子系统（`build_windows.sh` 第 3b 步） |
//! | ② | 内核子进程的控制台窗口 | `spawn.rs` 的 `CREATE_NO_WINDOW`（**本文件不直接调它**：命令由 `bootstrap::connect` 组） | 标志位**被传了**（`spawn.rs` 的测试）；窗口有没有消失只有真 Windows 能判 |
//! | ③ | 起不来时的最后一档 | `native_message_box`（`MessageBoxW`） | 宿主上走 stderr 那一支（只有真 Windows 能看到那个框） |
//! | ④ | **单实例** | `single_instance.rs` 的 `CreateMutexW`（**本文件不直接调它**：只调 `acquire()`），它失败时那句人话走 ③ 那个消息框 | 只有真 Windows 能判（规格 §9.1 那张表）；本机**一条断言都没有**，如实记账 |
//!
//! ⚠️ **③ 在本代比第二代更要紧**：第二代的失败还能借浏览器的页面说话，本代启动期的失败
//!    （WebView2Loader 装不上、Tauri 起不来）发生时窗口还不存在。
//!    交付形态又没有控制台（就是下面那个 attribute）⇒ `MessageBoxW` 是那一刻**唯一**
//!    还能跟人说话的通道（规格 §7 那张表最后一行）。
//!
//! ## ⚠️ 落点①只在 **release** 下生效
//!
//! `not(debug_assertions)` 才抑制控制台：debug 构建保留它，好让 panic 与 `eprintln!`
//! 有去处（开发时的可观测性 > 观感）。这也是**为什么本文件的失败话术要有消息框那一档**：
//! release 下 stderr 没有去处，只写 `eprintln!` 等于没写（W-2：失败必须看得见）。
//!
//! ## ⚠️ `tauri.conf.json` 里那几处的取值为什么是那样（那份文件不能写注释，记在这里）
//!
//!   * **`build.frontendDist` = `"../web"`**：它的**家**是 `windows/web/`（规格 §6），
//!     而这份配置住在 `shell-win/` 里 ⇒ 必须上跳一层。写成 `"web"` 会让 `tauri-build`
//!     去找 `shell-win/web/`（不存在 ⇒ 构建期就失败，那是好事：它是响亮的）。
//!   * **`bundle.active` = `false`**（W-4）：交付形态是 `windows/dist/` 下**一个** exe，
//!     由 `windows/scripts/build_windows.sh` 组装；tauri 自己的打包器（msi/nsis）**不参与**
//!     （它产出的东西不是单文件）。
//!   * **`app.security.csp` = `null`**：本代的前端全是内嵌资源、不发任何外部请求
//!     （规格 §3.3：没有监听端口、没有外部依赖）⇒ 现在没有可利用的面。
//!     ⚠️ **这是一条记账的取舍，不是"忘了配"**：将来若真要发外部请求，这一格必须重设。
//!   * **`app.withGlobalTauri` = `true`**（本任务加的）：前端是**没有构建步骤**的
//!     原生 HTML/JS（规格 §6.1：不引入 npm/vite），拿不到 ES 模块版的 `@tauri-apps/api`
//!     ⇒ 它唯一的后端接触面就是 `window.__TAURI__` 这个全局对象，而**那个对象默认不注入**。
//!     不开这一格的表现是：页面上 `window.__TAURI__` 是 `undefined`，
//!     `invoke` 一次都发不出去（**白屏，且没有任何东西会变红**）。
//!     前端那一侧的 `windows/web/js/invoke.js` 就是按这个全局对象写的
//!     （规格 §3.2：它是前端唯一的后端接触点）。
//!     ⚠️ 这也是本代**唯一**允许全局注入的一项；它不扩大本机能力面
//!     （能调 `invoke` 的仍然只有本进程 webview 里那一份页面）。

// ⚠️ **落点①：消掉壳自己的控制台窗口。**
//
// ⚠️⚠️ **必须带 `windows`**（探路里裸写成 `not(debug_assertions)`，宿主 release 构建会
//    因此对**非 Windows 目标**施加 `windows_subsystem` —— 那在 macOS 上是一个被忽略的
//    attribute，但它会让"这条只在 Windows 上意味着什么"变得读不出来）。
//
// 第二代还有第二条配套的 `CREATE_NO_WINDOW`（消掉**内核子进程**的控制台）——那条的
// 落点在 `spawn.rs` 里，调用链是 `bootstrap::connect` → `spawn::spawn_core`
// （**本文件不直接调它**，所以这一条在本代是"经由装配逻辑到达"的）。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use shell_core::client::CoreClient;
use shell_core::protocol::EngineState;
use shell_win::bootstrap;
use shell_win::commands::{self, Shell};
use shell_win::session::{self, Connector, Session};

/// 启动期失败时那条消息框的**标题**。
///
/// ⚠️ **窗口标题的家是 `tauri.conf.json` 的 `app.windows[0].title`**（那是窗口真正用的
///    那一份，规格 §7 那张表点名它要与 macOS 的 `CFBundleDisplayName` 逐字相同）。
///    这一份是**另一件事的**取值：那一刻窗口还没建起来（或者根本建不起来），
///    取不到配置里那一份，而 `MessageBoxW` 需要一个标题。
/// ⚠️ 两处必须**逐字相同**（否则用户看到两个名字），而**没有任何东西会因此变红** ——
///    钉住它的就是下面 `mod tests` 里那条 `the_message_box_title_matches_the_window_title`
///    （它把 `tauri.conf.json` 读进来比字面量）。改标题时两处一起改。
const WINDOW_TITLE: &str = "Benagen 数据下载工具";

/// 原生消息框（规格 §7 那张表的最后一行）。
///
/// ⚠️ **交付形态的子系统是 `windows`（没有控制台）**，而启动期失败时**也没有窗口**
///    ⇒ 这是唯一还能跟人说话的通道（W-2：失败必须看得见）。
#[cfg(target_os = "windows")]
fn native_message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let text = wide(text);
    let caption = wide(WINDOW_TITLE);
    // SAFETY：两个缓冲区都以 NUL 结尾、在本调用期间一直存活；这是 MessageBoxW 的契约。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// 宿主（macOS / Linux）那一支：打 stderr。
///
/// ⚠️ **交付靶不许走这一支**：那里没有控制台（落点①），而 `std` 的 `eprintln!` 在写
///    `stderr` 失败时是 **panic** —— "报错"这件事本身会把进程带走，用户看到的是
///    "双击之后什么都没发生"，比原本要报的那个错误还糟（W-2）。
///    交付靶那一档是上面那个 `MessageBoxW`，不是这一行。
///
/// ⚠️ 宿主构建**不是交付形态**（规格 §9.0：它是开发期的脚手架），所以这里"能看见"
///    这件事在宿主上是成立的（终端里有字）。
#[cfg(not(target_os = "windows"))]
fn native_message_box(text: &str) {
    eprintln!("{text}");
}

/// 关掉内核连接（**显式、幂等**，规格 §3.3 第 6 条）。
///
/// ⚠️ **两个调用点、同一个函数**：`WindowEvent::CloseRequested`（用户关窗那一刻）
///    与 `run()` 返回之后（兜底：窗口被别的途径关掉、或者事件没走到我们这里）。
///    第二次调用时 `take_client()` 回 `None`，本函数**什么都不做** —— 那就是"幂等"。
///
/// ⚠️ **它是同步的**（就在调用它的那条线程上把连接收掉），代价是最坏几秒 ——
///    而那正是要的：第二代的退出靠"心跳超时推断"，本代**必须**保证"关窗之后内核
///    不留孤儿"（内核靠 stdin EOF 自收尾，而那条管道的另一端就是我们）。
///    把它甩给后台线程的后果是**进程可能先退出**，于是那句 EOF 发不发得出去
///    变成一次竞态。⏱ 关窗那一刻用户已经决定不看了，等它几百毫秒是看不见的代价。
///
/// ⚠️ **绝不在持锁时丢连接**：`take_client()` 只做 `take`（临界区一次 `Option` 交换），
///    析构发生在**它之外**（`session.rs` 的 `take_client` / `install_client` 都记着
///    这条纪律的由来：`CoreClient::drop` 就是 `shutdown()`）。
fn shutdown(session: &Arc<Session>) {
    // ⚠️ **必须"取走"而不是克隆一份**：`CoreClient::drop` 是 `shutdown()`，而析构
    //    发生在**最后一个 `Arc` 被丢掉的那条线程**上 —— 克隆出来只会把它挪到别处、
    //    会话自己仍然持着它（`Session::take_client` 的文档写着这条）。
    drop(session.take_client());
}

/// 装配入口（规格 §3.3）：**单实例 → WebView2Loader → 会话 → 内核在哪 → 连接器 → Tauri**。
fn main() {
    // ---- ⓪ 单实例（规格 §2.2 / §7 那张表的第五行 / 待定项 D-3）---------------
    //
    // ⚠️ **必须排在所有事情之前**：第二份进程不该去释放一份 DLL、不该去建窗口、
    //    更不该去起第二个内核（那正是"单实例"要防的那件事）。
    //    ⚠️ 宿主上 `acquire()` 恒回 `First`（那一支是 no-op，理由写在
    //    `single_instance.rs` 的模块头）—— 所以这一段在本地开发时**不会**拦第二个实例。
    if shell_win::single_instance::acquire() == shell_win::single_instance::Verdict::AlreadyRunning
    {
        native_message_box(shell_win::single_instance::already_running_message());
        std::process::exit(0);
    }

    // ---- ① 装载 WebView2Loader（规格 §4.2：**必须在 Tauri 起来之前**）---------
    //
    // ⚠️ 顺序是承重的：这个 DLL 供的是 wry/windows 在**建窗口**时要调的那 5 个符号。
    //    等它调的时候才发现没装载上，就是一次**没有任何界面可说的崩溃**
    //    （交付形态没有控制台，窗口也还没建起来）。
    //    ⇒ 先做、先报，把失败变成一句人话。
    //
    // ⚠️ **宿主靶上这是一个 no-op**（`install()` 那一支恒返回 `Ok`）：宿主是 WKWebView，
    //    压根没有 WebView2 这回事（规格 §9.0）。**不是静默降级** —— 宿主的产物本来
    //    就不是交付形态，而交付形态那一条（`not(msvc)` 的 Windows 靶）没有开关。
    if let Err(why) = shell_win::wv2::install() {
        native_message_box(&format!("{WINDOW_TITLE} 起不来：\n\n{why}"));
        std::process::exit(1);
    }

    // ---- ③ 会话 + 内核在哪（R-15 搬回来的那三条路）---------------------------
    let session = Arc::new(Session::new());

    // ⚠️ 这里**不留下那个路径**：真正要起的那个内核由连接器在**每一次尝试**里重查一遍
    //    （`retry()` 的语义就是"重新找一次"——人类伙伴的验收动作是"把内核放到壳旁边、
    //    再点重试"）。这一遍只回答两件事：
    //      · 是不是"退回了同目录那一份"（W-2 披露，写进它自己那一格，见下）；
    //      · 这一刻**有没有**可起的进程 —— 没有就**当场**落到"不可用"，让页面第一眼
    //        就能说清为什么（而不是等一次后台线程的往返）。
    let core_found = match bootstrap::locate_core_binary() {
        Ok(lookup) => {
            // ⚠️ W-2：**"退回了同目录那份内核"这件事必须说出来**，而且要住在**它自己那一格**
            //    （粘滞），不许借 `last_error`（那一格的契约是"下一个成功就清"）。
            session.set_fallback_notice(lookup.embedded_failure.clone());
            true
        }
        Err(why) => {
            // 没有可起的进程 ⇒ 落到"不可用"，理由照登。壳**照起**（页面要能显示这句话）。
            session.set_engine(EngineState::Unavailable(why));
            false
        }
    };

    // ---- ④ 装连接器（**必须在起 Tauri 之前**）--------------------------------
    //
    // ⚠️ 它是 `retry()` 的**唯一**恢复路径：每次调用都真的重查一遍（不许把上一次的
    //    结果缓存下来 —— 那样"把内核放到壳旁边再点重试"这条补救就只灵一次）。
    // ⚠️ 重查会再走一遍 `locate_core_binary`（含释放那一支的副作用）：那是**幂等**的
    //    （哈希相符即复用），见它的文档。
    //
    // ⚠️ **为什么必须早于 `run()`**：`retry()` 取的就是这里装进去的那个连接器，
    //    装晚了的表现是"壳起来了、但重试永远说'连接器还没有装配'"—— 而重试是**唯一**
    //    的恢复路径（`session.rs` 里那条不可达分支的文档写着同一件事）。
    {
        let worker = Arc::clone(&session);
        let connector: Arc<Connector> =
            Arc::new(move || -> Result<(Arc<CoreClient>, String), String> {
                // ⚠️ **两句 `set_fallback_notice` 合起来**才把那一格维持在"当前的事实"上
                //    （它的"粘滞"说的是**没有自动清除** —— 一次成功的普通请求不会把它抹掉，
                //    不是"永远不许改"）：
                //      · 查到内核 ⇒ 写**这一次**查到的值（退回了 ③ 就有、没退就是 `None`）；
                //      · **整体查不到**（三条路都断了）⇒ 必须是 `None`：那一刻**没有**
                //        "退回了同目录那份内核"这回事，留着上一轮那一句就是一句已不成立的话。
                //    ⚠️ 少了下面那一支，这一格会**跨过重试一直粘着**：② 修好之后页面上还挂着
                //    "本机用的是手工放的那份内核" —— 与 W-2 要的"那一格的字必须是事实"相反。
                let lookup = match bootstrap::locate_core_binary() {
                    Ok(lookup) => lookup,
                    Err(why) => {
                        worker.set_fallback_notice(None);
                        return Err(why);
                    }
                };
                // 重查可能让这件事**变了**（比如杀软放行之后内嵌那份释放成功了）。
                worker.set_fallback_notice(lookup.embedded_failure.clone());
                // ⚠️ **argv 在起内核这一刻现算**（任务 8）：下载目录**只在 argv 上**
                //    （内核的 `set_settings` 没有这个键）⇒ 改了目录之后的重启能生效，
                //    靠的就是"每一次起内核都重读一遍偏好"。
                //    ⚠️ 取不到偏好（`%APPDATA%` 缺失）时不中止起内核：**当成未配置**
                //    （不传 `--download-dir`，内核用它的默认值）。这不是静默降级 ——
                //    "没有偏好"与"读不到偏好"在**内核那一侧**是同一件事（都不传那个 flag），
                //    而"读不到存放根"这件事本身会在设置窗口那条路上大声报出来
                //    （`preferences_get` 回失败信封）。
                let arguments = match commands::load_preferences() {
                    Ok(preferences) => shell_core::api::preferences::core_arguments(&preferences),
                    Err(_) => Vec::new(),
                };
                // ⚠️ 这一发是**无上界**的（`hello` 有 5 秒上界，但起子进程没有），
                //    而它跑在 `session::spawn_connect` 起的那条**后台线程**上（裁决 Z）。
                let connected = bootstrap::connect(&lookup.path, &arguments)?;
                Ok((connected.client, connected.handshake_reply))
            });
        session.set_connector(connector);
    }

    // ---- ⑤ 连内核（后台线程；页面靠轮询 `state()` 看结果）--------------------
    //
    // ⚠️ 一条可起的进程都没有时不发这一趟：那一刻引擎已经是 `Unavailable`，再让连接器
    //    重查一遍只会把同一句原因写第二遍（而页面上的「重试」随时能重来一遍）。
    if core_found {
        session::spawn_connect(&session);
    }

    // ---- ⑥ 起 Tauri ---------------------------------------------------------
    //
    // ⚠️ `manage(Shell)` 必须在 `run()` 之前（命令一被调到就要取它），而
    //    **命令表住在 `commands.rs` 里**：`#[tauri::command]` 展开出来的那个包装宏
    //    是定义在命令所在模块里的 `macro_rules!`，**跨 crate 的宏路径解析不了**
    //    （实测 `E0433`）—— 而 bin 与 lib 正是两个 crate。`commands::invoke_handler()`
    //    就是那个收口（理由写在那里）。
    //
    // ⚠️ `generate_context!` 是**编译期**读 `tauri.conf.json` 的（图标、窗口配置、
    //    前端资源的路径都在那一刻定下来）⇒ 那几样东西缺任何一样都是**构建期失败**，
    //    不是运行期的惊喜。
    // ⚠️ **关窗即退出**（规格 §3.3 第 6 条）——它落在 `on_window_event` 的这一段里。
    //
    // 判据是 `WindowEvent::CloseRequested`（**一条显式事件**）：用户在窗口上点 ×、
    // 或者按 Alt+F4，都会先发它。这里**同步**把内核连接收掉（[`shutdown`]），
    // 然后**不拦**这次关闭（没有 `signal_tx`、也没有 `api.prevent_close()`）：
    // 窗口照常关掉 ⇒ tauri 发现"一个窗口都不剩了" ⇒ `run()` 返回 ⇒ 进程退出。
    //
    // ⚠️ **这里没有、也不许有**：心跳、看门狗、超时推断（第二代那一整套的全部理由
    //    是"我们没有一个能被程序控制的窗口"，规格 §1.2）。这一行**就是**"关窗即退出"。
    //
    // ⚠️ **事件是每个窗口都发的**：本代只有一个窗口（规格 §2.2：不做多窗口），
    //    所以"关掉它"就是"关掉全部"；`shutdown` 幂等，将来真加了第二个窗口也不会重复收尾。
    let window_events = {
        let watcher = Arc::clone(&session);
        move |_window: &tauri::Window, event: &tauri::WindowEvent| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                shutdown(&watcher);
            }
        }
    };

    if let Err(why) = tauri::Builder::default()
        .manage(Shell::new(Arc::clone(&session)))
        .invoke_handler(commands::invoke_handler())
        .on_window_event(window_events)
        .run(tauri::generate_context!())
    {
        // ⚠️ 走到这里说明**窗口都没起来**（前端资源坏了、webview 初始化失败、…）。
        //    与 ① 同一档：那一刻没有界面、没有控制台 ⇒ 只剩原生消息框这一条路。
        native_message_box(&format!(
            "{WINDOW_TITLE} 起不来：\n\n{why}\n\n补救：把这条原样发给我们。"
        ));
        std::process::exit(1);
    }

    // ⚠️ **关窗之后走到这里**（Tauri 的默认行为：最后一个窗口关掉 ⇒ `run()` 返回）。
    //    这一段是**兜底**：正常路径上连接已经在 `CloseRequested` 那一刻收掉了
    //    （上面那个闭包），而 `shutdown` 是**幂等**的 —— 第二次调用时
    //    `take_client()` 回 `None`，本行什么都不做。
    //    ⚠️ 它在两条"事件没走到我们这里"的路上是**唯一**的收尾：窗口被程序化关掉
    //    （`Window::close` 也会发 CloseRequested，所以其实也走到了）、或者事件循环
    //    因为别的原因退出（例如 tauri 自己要求重启）。
    shutdown(&session);
}

#[cfg(test)]
mod tests {
    //! 本文件自己的测试：**只有"两处标题必须逐字相同"这一条**。
    //!
    //! ⚠️ 本文件是**装配**（规格 §4.3：视图/装配不单测），能被写出断言的逻辑一律住在
    //!    `shell-core` 或 `shell-win` 的 lib 里。下面这一条之所以破例留在这里，
    //!    是因为它要断言的那两处**都在本 crate 的装配层**（一个是这个文件的常量、
    //!    另一个是 `tauri.conf.json`）—— 搬到别处就得把配置文件的路径也搬过去。
    //!
    //! ⚠️ 第二代在本文件里有一整套判据（启动 URL 的形状、`ShellExecuteW` 的 `> 32` 门限、
    //!    内核查找的三路次序、握手的超时值 …）。**任务 7 按裁定 R-15 把它们搬回来了** ——
    //!    但搬进的是 [`shell_win::bootstrap`] 的 `mod tests`（那些函数的家现在在那里，
    //!    判据也就跟着它们走；**本文件不再持有它们**）。8 条回来了、4 条留在 git 历史里
    //!    （哪 4 条、为什么，见 `bootstrap.rs` 的文件头）。

    use super::WINDOW_TITLE;

    /// **窗口标题与消息框标题必须逐字相同。**
    ///
    /// 判别力（两个方向都会红）：改 `tauri.conf.json` 里的 `title`、或改本文件的
    /// `WINDOW_TITLE`，这一条立刻红 —— 而**没有别的东西会红**（窗口标题只有真开窗
    /// 才看得见，消息框标题只有真失败才看得见，两个都在本机的射程之外）。
    #[test]
    fn the_message_box_title_matches_the_window_title() {
        let raw = include_str!("../tauri.conf.json");
        let config: serde_json::Value =
            serde_json::from_str(raw).expect("tauri.conf.json 必须是合法 JSON");
        let title = config["app"]["windows"][0]["title"]
            .as_str()
            .expect("tauri.conf.json 里必须有 app.windows[0].title（窗口标题的家在那里）");
        assert_eq!(
            title, WINDOW_TITLE,
            "两处标题对不上：窗口上显示一个名字、启动失败的消息框上显示另一个"
        );
    }
}
