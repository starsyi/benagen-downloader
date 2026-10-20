//! 命令层（`invoke` 的全部面，规格 §3.4）。
//!
//! ## 每个命令都是**三步**（规格 §5.3 逐字）
//!
//! ```text
//! 取数据（Session 的快照 / 内核回执）→ 调 `shell_core::api::*` 的纯函数 → 返回 Value
//! ```
//!
//! **不再有 `Route` / `Resp` / `Req` 那套 HTTP 形状**：本代没有监听端口（规格 §3.3），
//! 命令名与参数由 Tauri 的 `invoke` 取代。信封（`{"ok":true,"data":…}` /
//! `{"ok":false,"error":{"message":…}}`）由 `shell-core` 的 `api` 层套好 ——
//! 本层**一次都不自己拼 JSON 信封**，也不自己写面向用户的字符串
//! （`NO_KERNEL` 那句话的家在 `shell_core::api`，见那里的文档）。
//!
//! ## ⚠️⚠️ 锁的纪律：**绝不在持锁时调 `kernel::*`**
//!
//! `session.rs` 的文件头写着这条，而**本文件是新的调用点，同一条纪律照旧**。
//! `kernel::*` 那些函数是**阻塞**的（`CoreClient::call` 的上界按方法名分两档：普通
//! **150 秒**、长调用家族 **600 秒** —— 两档都刻意取长。⚠️ `load_delivery` 的最坏值
//! **不是** 91.5 秒：那只是内核三段记账里的第一段，见 `client.rs` 的 `LONG_CALL_TIMEOUT`），
//! 而 `Session` 的锁是所有命令共用的
//! （`state()` 一秒一问，规格 §3.5）—— 持着锁等内核，等于让**整个前端的轮询**跟着停摆。
//!
//! 本文件的写法只有两种，两种都把锁放掉了：
//!   * `session.client()` 把 `Arc<CoreClient>` **克隆出来**（临界区只做一次 clone），
//!     之后的每一次 `kernel::*` 都在锁外；
//!   * 长调用（`load_delivery`）交给 `session::spawn_load` 起的**后台线程**，
//!     命令本身立刻返回（D6）。
//!
//! ⚠️ 每一条命令都跑在 **Tauri 的 `invoke` 线程**上（不是主线程）：它们**可以**阻塞一会儿，
//!    但**不许**把会话锁带走 —— 那正是上面那条纪律要保的东西。
//!    （裁决 Z 管的是"无上界的内核调用不许在主线程上"，这里的落点与第二代
//!    HTTP 的每连接线程是同一个：一条命令占一条线程。）
//!
//! ## ⚠️ 实参名与 JS 那一侧的写法
//!
//! Tauri 2 默认把命令实参按 **camelCase** 收（Rust 侧写 `base_url` ⇒ JS 侧发
//! `{ baseUrl }`）。规格 §3.4 那张表写的是 `load(code, base_url)` —— 两边说的是同一件事，
//! 只是各自的书写习惯。前端（`windows/web/js/invoke.js`）照着那个约定发即可。
//!
//! ⚠️ 于是**第二代那四档 400 由 Tauri 的形状取代了**（见 `enqueue` 命令的文档）：
//!    实参类型不对（缺 `paths`、`paths` 不是数组、数组里有非字符串）在**进本层之前**
//!    就被 Tauri 的反序列化挡下（JS 拿到一个 rejected promise），走不到内核。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use shell_core::api;
use shell_core::export::Header;
use shell_core::presentation::about_info::AboutInfo;
use shell_core::presentation::app_preferences::{
    DownloadDirChange, DownloadDirectory, VerboseLoggingChange,
};
use shell_core::presentation::breadcrumb::Breadcrumb;
use crate::diskspeed;
use shell_core::presentation::disk_notice::DiskNotice;
use shell_core::presentation::browser_row::{BrowserRow, BrowserSelection, SelectionSummary};
use shell_core::presentation::delivery_summary::DeliveryCodeEntry;
use shell_core::api::tree::DownloadAction;
use shell_core::presentation::download_targets::{DownloadTargets, EnqueueFeedback};
use shell_core::presentation::kernel_death::CallFailure;
use shell_core::presentation::transfer_row::{
    TransferGlobalSummary, TransferListEmpty, TransferReveal, TransferRow,
};
use shell_core::presentation::update_check::UpdateCheck;
use shell_core::presentation::verify_summary::{ProgressSummary, VerifySummary};
use shell_core::protocol::{EngineState, LoadState, Settings, TaskAction};
use shell_core::storage::history::History;
use shell_core::storage::preferences::Preferences;

use crate::batch;
use crate::kernel::{self, TransferPoll};
use crate::openurl;
use crate::pickdir;
use crate::reveal;
use crate::session::{self, Session};

/// 命令层的**应用状态**（Tauri 的 `manage` / `State<'_, Shell>`）。
///
/// ⚠️ **为什么要有这一个类型**：Tauri 的命令只能拿到"被 `manage` 进去的东西"，
///    而"这一屏要用到的全部共享状态"**就是那个 `Arc<Session>`**（内核连接、引擎态、
///    加载态、两格错误、握手回执）。直接 `manage(Arc::<Session>::clone(&…))` 也能跑，
///    但那样命令的签名就写不出"这是**壳的状态**"这层意思，而且任务 8 一加新命令
///    （单实例、设置、历史…）就会开始往元组里塞第二个、第三个东西 ——
///    那时改的是每一条命令的签名。留一个具名类型，加一格是**改一处**。
///
/// ⚠️ **它只有一个字段，而且 `pub`**：`main.rs`（bin 是**另一个 crate**）要用它。
///    本代**不**给它加锁：里面那个 `Arc<Session>` 自己带锁，而"锁的纪律"那条
///    （绝不在持锁时调 `kernel::*`）说的是 `Session` 内部那把锁，不是本类型。
pub struct Shell {
    pub session: Arc<Session>,
}

impl Shell {
    pub fn new(session: Arc<Session>) -> Shell {
        Shell { session }
    }
}

/// **`invoke` 命令表的唯一落点**（规格 §3.4 那张表的前七行）。
///
/// ⚠️ **为什么是函数而不是在 `main.rs` 里写 `generate_handler![…]`**：
///    `#[tauri::command]` 展开出来的那个包装宏（`__cmd__state` 之类）是
///    **定义在本模块里的 `macro_rules!`**，而跨 crate 的宏路径**解析不了**
///    （实测：`a::m::__cmd__foo!()` 在另一个 crate 里报 `E0433: could not find
///    __cmd__foo in m`）—— 而 bin 与 lib 正是两个 crate。
///    ⇒ 命令表**必须和命令住在同一个模块里**，bin 只拿到一个现成的 handler。
///    ⚠️ 这个形状对任务 8 也更顺：加命令时**只改本文件这一行**，
///    `main.rs` 一行都不用动（它压根不知道有哪些命令）。
///
/// ⚠️ 泛型参数 `R` 由调用方（`tauri::Builder::<Wry>`）推断 —— 本函数不指名任何运行时。
pub fn invoke_handler<R: tauri::Runtime>(
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        // 七个既有端点（任务 7，对齐第二代那七条路由）
        state,
        // 自动重启的进度（规格 W3）：前端轮询它来画「正在重启下载引擎（第 n/3 次）…」。
        restart_progress,
        // 双域名竞速的解析过程（规格 W4）：加载清单期间把四阶段逐行摆出来。
        delivery_progress,
        load,
        retry,
        tree,
        enqueue,
        transfers,
        verify,
        // 更新提示的**四条**（规格 §4；R15）。⚠️ 不在规格 §3.4 那张表里 —— 那一张是
        // "第二代既有端点"的账，而"更新检查"是这一代新加的整件事（内核侧的四条同理）。
        // ⚠️ 第四条（`update_check_now`，设置里那颗「立即检测」）**规格 §2 有、壳里一直没有**
        //    —— 内核早就把它备好了（`core/src/kernel.rs:3404` 的 `op_check_update_now`，
        //    不受 24 小时节流），缺的只是这一条命令与前端那颗按钮。
        update_status,
        update_set_enabled,
        update_check_now,
        // ⚠️ **窄语义**的一条（R15）：只放行官方发布页那一个前缀、只做"交给默认浏览器"
        //    这一件事。它不是通用的 `open_url`（理由写在它自己的文档里）。
        open_update_url,
        // 任务 8 补的十个（规格 §3.4 那张表里标着"无（新增）"的那些）+ 一个
        // ⚠️ 规格表里没有的 `preferences_check` —— 理由见它自己的文档。
        task_action,
        reveal,
        settings_get,
        settings_set,
        preferences_get,
        preferences_check,
        preferences_set,
        // ⚠️ 规格 §3.4 那张表里也没有这一条（同 `preferences_check` / `pick_directory`）：
        //    它是**详细诊断日志那个开关**（规格 §2.4），与 `preferences_set` 是同一件事的
        //    另一半（"改一项要重启内核的设置"）。理由写在它自己的文档上。
        verbose_logging_set,
        // ⚠️ 规格 §3.4 那张表里没有这一条（同 `preferences_check`）：它是**系统对话框**
        //    那一路，第二代与 macOS 都没有对应的"端点"（macOS 那边是视图里直接调
        //    `NSOpenPanel`，没有命令行）。理由写在它自己的文档上。
        pick_directory,
        // ⚠️ 规格 §3.4 那张表里也没有这一条（同 `pick_directory`）：它是**导出诊断日志**
        //    那一次动作（规格 §2.5），与 `verbose_logging_set` 是一件事的两半
        //    （开关把日志打开、导出把它取出来）。理由写在它自己的文档上。
        diagnostics_export,
        history_get,
        history_put,
        license,
        about,
    ]
}

/// 会话总览（规格 §3.4 第一行，对齐第二代 `GET /api/state`）。
///
/// ⚠️ **它必须在前端第一屏就答得出来**：还没有内核时也一样（那是这条命令的常态，
///    不是错误分支）—— 页面的引擎横幅、加载态、诊断区全靠它。
#[tauri::command(async)]
pub fn state(app: tauri::State<'_, Shell>) -> Value {
    let view = app.session.view();
    // ⚠️ 第二个实参是 `view.handshake_reply` 的**那一份**（计划任务 7 的调用点逐字）：
    //    冗余是这个签名本来的样子，不是笔误 —— 见 `api::state::state` 的文档。
    api::state::state(&view, view.handshake_reply.as_deref())
}

/// 起一次 `load_delivery`（规格 §3.4 第二行，对齐第二代 `POST /api/load`）。
///
/// **异步**：结果落在 `session.load` 上，前端靠轮询 `state()` 取（D6）——
/// `load_delivery` 走的是长调用那一档（上界 600 秒，见 `client.rs` 的 `LONG_CALL_TIMEOUT`），
/// 同步会把这条 `invoke` 按住那么久。
///
/// ⚠️ **长度闸在发之前**（第二代那两条判据一字未搬，只是换了调用点）：
///    超限的请求内核**不报错**，它只把客户端**静默堵死**（`core/src/main.rs` 的行长上限
///    与 `CoreClient` 的 FIFO 队列，见 `DeliveryCodeEntry::MAXIMUM_BYTES` 的文档）。
///    判据只有 `DeliveryCodeEntry` 那一份 —— 本层不重写一遍长度判断。
#[tauri::command(async)]
pub fn load(app: tauri::State<'_, Shell>, code: String, base_url: Option<String>) -> Value {
    app.session.reset_auto_restart();
    // 这两句"太长"的提示要按界面语言成句（规格 W1）。
    let lang = app.session.lang();
    // ⚠️ 前端不传 `base_url` 时它是 `None`（"高级：自定义下载地址"那一格没填）⇒ 空串，
    //    与第二代 `body.get("base_url").and_then(as_str).unwrap_or("")` 逐字同义。
    let base_url = base_url.unwrap_or_default();
    if !DeliveryCodeEntry::is_sendable(&code) {
        app.session
            .set_load(LoadState::Failed(DeliveryCodeEntry::too_long_hint(lang)));
        return api::envelope::ok(serde_json::json!({ "status": "rejected" }));
    }
    if !base_url.is_empty() && !DeliveryCodeEntry::is_sendable(&base_url) {
        app.session
            .set_load(LoadState::Failed(DeliveryCodeEntry::base_url_too_long_hint(lang)));
        return api::envelope::ok(serde_json::json!({ "status": "rejected" }));
    }
    // ⚠️ 锁的纪律：`client()` 克隆出来就放锁，`spawn_load` 起后台线程跑那条长调用。
    match app.session.client() {
        Some(client) => {
            app.session.set_load(LoadState::Loading);
            session::spawn_load(&app.session, client, code, base_url);
            api::envelope::ok(serde_json::json!({ "status": "loading" }))
        }
        None => {
            // ⚠️ 这句话的家在 `shell_core::api`（**命令层一个字都不自己写**，R-13）。
            app.session
                .set_load(LoadState::Failed(api::NO_KERNEL.to_string()));
            api::envelope::ok(serde_json::json!({ "status": "no_kernel" }))
        }
    }
}

/// 重新找一次内核并握手（规格 §3.4 第三行，对齐第二代 `POST /api/retry`）。
///
/// ⚠️ **这是唯一那条恢复路径**：人类伙伴的验收动作就是"把内核放到壳旁边、再点重试"。
///    它每次都真的问一遍注入的连接器（`session::spawn_connect` 的文档记着这条）。
#[tauri::command]
pub fn retry(app: tauri::State<'_, Shell>) -> Value {
    app.session.reset_auto_restart();
    session::spawn_connect(&app.session);
    api::envelope::ok(serde_json::json!({ "status": "connecting" }))
}

/// 文件页（规格 §3.4 第四行，对齐第二代 `GET /api/tree`）。
///
/// ⚠️ **两条分支发的是两件不同的东西**（不是笔误，判据在 `api::tree` 的文件头）：
///   * 给了 `path` ⇒ 那一层的行（`list_dir` ⇒ `BrowserRow`）；读不到就是 [`api::tree::level_failure`]；
///   * 没给 `path` ⇒ 整棵树的**总进度 + 勾选摘要**（`get_tree`）。
///
/// ## 🔴 `selection`：**前端当前那一刻的勾选面**（任务 17 加的，只有无 `path` 那一支用它）
///
/// 它回答的是"用户**现在**勾了哪些项"—— `SelectionSummary`（已选几项 / 合计多大）、
/// `DownloadAction`（按钮叫什么 / 那一句"明说" / **此刻能不能按**）全都按它算。
/// ⚠️ **少了它，那三格描述的是内核的 `default_selected`**（"这一批里所有还没下完的文件"），
///    而用户早就把它改过了 ⇒ 底栏与工具栏那颗按钮说的是一句**过期的话**
///    （前端每次勾选之后都会重问这一支，本来就是冲着"刷新那几格"来的）。
/// ⚠️ **首帧/播种那一拍不传它**（`None`）⇒ 落回 `default_selected`，也就是"勾选面**将要**
///    被播种成什么" —— 那正是那一刻唯一说得通的那个面（前端此刻手上还没有自己的面）。
///
/// ## ⚠️ 它**不改变**这条命令问内核的方式
///
/// 仍然是那一次 `get_tree`（`selection` 只参与**回执怎么算**，不参与请求怎么发）——
/// 于是它既不额外增加一次内核往返，也不给这一支添一条新的失败路径。
#[tauri::command(async)]
pub fn tree(
    app: tauri::State<'_, Shell>,
    path: Option<String>,
    selection: Option<Vec<String>>,
) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // 状态列那几个词要按界面语言成句（规格 W1）。
    let lang = app.session.lang();
    match path {
        Some(path) => match kernel::list_dir(lang, &client, &path) {
            Ok(result) => {
                app.session.set_last_error(None);
                // ⚠️ 面包屑由壳拼（`list_dir` 的目录项没有 `path` 键），而且这里传的
                //    `path` 就是请求里那一条**清单原文**（约束 3：不 trim、不加工）。
                api::tree::level(lang,
                    &Breadcrumb::new(&path),
                    &BrowserRow::rows(&result.entries, &path),
                )
            }
            // ⚠️ **读不到某一层仍然是 `ok` 信封**（那是界面上的一个状态，不是整屏错误）——
            //    判据在 `api::tree::level_failure` 的文档里。面包屑跟着**退到的那一层**走。
            Err(why) => api::tree::level_failure(&Breadcrumb::new(&why.path), &why),
        },
        None => match kernel::get_tree(lang, &client) {
            Ok(result) => {
                app.session.set_last_error(None);
                // ⚠️ 那一层 `path` 的分支**不看** `selection`（"这一层有哪些行"与勾选面无关）。
                whole_tree(&app.session, &result, selection.as_deref())
            }
            Err(why) => {
                note_failure(&app.session, &why);
                api::failure(&why)
            }
        },
    }
}

/// 整棵树那一支要发的界面值（**搬自**第二代 `routes.rs:306` 的 `tree_json` 的调用侧）。
///
/// ⚠️ **为什么这一段留在命令层而不是 `api::tree::whole` 里**：它要读 `SessionView`
///    （拿"当前生效的是哪一批"的那个码），而 `api` 层的每一个函数都只收**纯数据**
///    （规格 §5.3）。`ProgressSummary::of` 那三道闸与 `SelectionSummary` 的算法
///    一个字都没搬第二份 —— 这里只做"取数据"，判断在 `shell-core` 里。
///
/// ## 🔴 `selection`：这个回执是按**哪一个勾选面**算的（任务 17）
///
/// 给了就是"用户此刻勾的那些项"（前端每次改勾选之后都会拿它重问一次）；没给就是
/// "还没有自己的勾选面"（首帧 / 换批复位之后那一拍）⇒ 落回内核的 `default_selected`。
/// **三格（摘要 / 动作的文案 / 动作此刻能不能按）全部按同一个面算** —— 各用各的面，
/// 用户看到的就是一句自相矛盾的底栏，而它们都是字符串，**不会有任何东西变红**
/// （`api::tree::whole` 的文档里写着这条）。
///
/// ⚠️ **"能不能按"那一格怎么来的**（C-3 的另一半）：勾选面先过 [`kernel_paths`]
///    ——**与 `enqueue` 逐字同一处判据**（整批全选 ⇒ 空数组，而空数组永远不超预算），
///    再过 `DownloadTargets::blocked_reason`。于是"界面拦下的"与"命令层兜底拦下的"
///    说的是**同一件事**，不会出现"按钮亮着但一点就报错"那种两套判据的分叉。
///
/// ⚠️ **这里的全集取自 `tree.flat`**（`BrowserSelection::all_files`），而
///    [`kernel_paths`] 用的是**会话里那棵树**（`all_files_of`）：两条来源算的是
///    **同一个集合**（等价性由 `the_two_receipts_of_one_manifest_name_the_same_files`
///    钉着），选它们各自手边现成的那个是为了**哪一边都不必额外问内核一次**。
///    竞态下（换批的窗口里）两边可能不同 ⇒ 最坏的结果是"按钮亮着、点下去被兜底闸挡下
///    并说明理由"，不会静默。
fn whole_tree(
    session: &Session,
    tree: &shell_core::protocol::TreeResult,
    selection: Option<&[String]>,
) -> Value {
    let view = session.view();
    let lang = view.lang;
    // ⚠️ `tree_code == code` 在这里**是事实不是捷径**：这一棵树就是为"当前已加载的那一批"
    //    刚拉下来的，两个码本来就是同一个值。那道闸实际拦住的是 `code == None`
    //    （还没有生效批次 ⇒ 不显示进度），也就是它的本意：不把上一批的进度摆在这一批底下。
    let code = match &view.load {
        LoadState::Loaded(info) => Some(info.code.as_str()),
        LoadState::Idle | LoadState::Loading | LoadState::Failed(_) => None,
    };
    let progress = ProgressSummary::of(Some(tree), code, code);
    // ⚠️ 勾选面：前端报上来的那个（"用户此刻勾了什么"）；没有就是内核给的
    //    `default_selected`（"所有非 complete 的文件" = 勾选面**将要**被播种成什么）。
    //    大小索引取 `flat` —— **`flat` 只在这里被消费，不进回执**（裁定逐字）。
    let selected: BTreeSet<String> = match selection {
        Some(paths) => paths.iter().cloned().collect(),
        None => tree.default_selected.iter().cloned().collect(),
    };
    let sizes = SelectionSummary::size_index(&tree.flat);
    // 大批量不再拦截（2026-10-12）：超预算的路径串在 `enqueue_with` 里切块依次发送，
    // 所以这里不再需要算"真正要发出去的那一串"来判能不能按。
    // ⚠️ 底栏那颗按钮的文案与它旁边那句「明说」：**同一个勾选面**（`selected`，
    //    也就是下面 `SelectionSummary::of` 吃的那一个）—— 几格说的必须是同一件事。
    //    这些字符串都由 `DownloadTargets` 算（`presentation` 的既有纯函数，有单测），
    //    本层只做那一次三元判断（"一项都没勾" ⇒ 那句话在场）——
    //    它的出处是 macOS 侧 `SelectionBar.swift:33` 的 `hint: selection.isEmpty ? … : nil`
    //    （那是视图里的一步取值，不是一条能被单测钉住的判据）。
    let action = DownloadAction {
        button_title: DownloadTargets::button_title(lang, &selected),
        empty_selection_hint: if selected.is_empty() {
            Some(DownloadTargets::empty_selection_hint(lang).to_string())
        } else {
            None
        },
        blocked_reason: None, // 大批量改为分批入队，不再拦截（2026-10-12）
    };
    api::tree::whole(
        &Breadcrumb::new(""),
        progress.as_ref(),
        &SelectionSummary::of(&selected, &sizes),
        &tree.default_selected,
        &action,
    )
}

/// 加入下载任务（规格 §3.4 第五行，对齐第二代 `POST /api/enqueue`）。
///
/// ⚠️ **实参是 `Vec<String>`**（不是第二代那个"自己解 JSON 正文"的形状）：
///    四种"没读懂"（正文不是 JSON / 没有 `paths` 键 / `paths` 不是数组 /
///    数组里有非字符串）**在 Tauri 的反序列化那一步就被挡下**（JS 拿到 rejected promise），
///    根本走不到这里 ⇒ 第二代那四档 400 与它们要防的事（把打错的请求**静默升级成
///    "下载整批"**，因为内核把空的 `paths` 定义成"下全部待下载"）**没有消失，只是
///    防线前移到了类型上**。⚠️ 这条仍然是承重的：它保证走到 `kernel::enqueue` 的
///    `paths` 一定是**显式**的（空的也一样）。
///    ⚠️ 而"显式的空数组 = 整批全选"这条**约定**照旧成立（C-3）：`Vec::new()` 是合法的，
///    这里**不许**把它拦成"没给"（那会把一个合法的动作判成错误）。
///
/// ## 🔴 前端发来的这一串**不是**直接发给内核的那一串（任务 16）
///
/// 前端给的是**用户勾了哪些项**；发给内核的是 `kernel_paths` 按 C-3 那条判据
/// （`DownloadTargets::paths`）算出来的那一串 —— **勾选面覆盖整批时是空数组**
/// （内核的 `paths` 为空 = 全部**待下载**），于是请求体**恒定**、不随批次大小增长。
///
/// ⚠️ 少了这一步，"全选一个大批次"会把成千上万条路径塞进请求体：内核的行长上限是
///    8 MiB（`core/src/main.rs`），而 `CoreClient::call` 在**写出去之前**就按同一个数字
///    拒绝（`client.rs` 的 `MAX_REQUEST_LINE_BYTES`）⇒ 那一次下载**报错**（不是静默挂死）。
///    ⚠️ **"内核回 `id == 0` ⇒ 那条 `invoke` 永远不 resolve"是另一件事**（闸的**另一侧**：
///    内核按 `take(8 MiB)` 读不到整行、于是回一条对不上任何请求 id 的协议告警）——
///    ✅ **那条账已由 A5 关闭**（`client.rs` 的看门狗：一次调用超过它那一档的上界 ⇒ 判死 ⇒
///    走「内核没了」路径，横幅 + 「重试」）。⚠️ 口径与 `commands.rs` 里那条同题用例
///    （`a_huge_batch_does_not_blow_up_the_request` 的文档）**必须一致**，两处都写着这件事。
///    那件事的**出处是 `client.rs` 的
///    `MAX_REQUEST_LINE_BYTES` 文档与 `a_huge_batch_does_not_blow_up_the_request` 那条用例的文档**，
///    不是本函数下面那个 `kernel_paths`（它的文档只说"勾选面怎么变成发出去的那一串"）。
///    判据本身**只有一处实现**（`shell-core` 的 `DownloadTargets::paths`，有单测），
///    本层不重写一遍"什么叫覆盖整批"。
///
/// ## 🔴 另一半：**部分**勾选一个超大批次（任务 17）
///
/// 上面那一条只挡住"勾选面覆盖整批"。**少勾几项**时发出去的是显式列表，它同样可能
/// 超过那道 8 MiB 的硬闸 ⇒ 用户撞上一句**他看不懂的错**（`client.rs` 的
/// `RequestTooLong`，里面还写着"把 enqueue 分批发"——**界面里没有"分批"这个动作**）。
/// 处置分两层，**判据是同一个** `DownloadTargets::blocked_reason`：
///   * **界面那一层**：`tree()` 无 `path` 那一支回执里的 `action.blocked_reason`
///     —— 前端拿它把工具栏那颗按钮**禁用**、并把那句话摆在底栏（用户**点之前**就知道）；
///   * **本函数的兜底闸**：上面那道 `if let Some(why)` —— 界面那一格有刷新延迟，
///     窗口里点下来仍会走到这里 ⇒ **不发内核**、把同一句话当成回执交给用户。
///
/// ⚠️ **兜底闸为什么不能省**：它是唯一**不可能被绕过**的那一层（前端的状态是异步的，
///    而这里是"发出去之前的最后一次判断"）。少了它，窗口里那一点就会落到
///    `client.rs` 那句原文上 —— 那正是本任务要消灭的那句话。
///
/// ⚠️ **命令体在 `enqueue_with` 里**（去掉 Tauri 那一层之后的全部逻辑）：
///    `tauri::State<'_, Shell>` 只有 Tauri 运行时造得出来，判据若写在下面这个函数的
///    函数体里，任务 16 要补的两条用例（"勾选面 == 全集 ⇒ 交给内核的是空数组"、
///    "大批次不会把请求撑爆"）**一条都写不出来** —— `kernel.rs` / `session.rs` 里那些
///    既有用例都够不到 `State`。拆出来的那一层收同样的东西（`&Session` + 裸 `paths`）。
#[tauri::command(async)]
pub fn enqueue(app: tauri::State<'_, Shell>, paths: Vec<String>) -> Value {
    enqueue_with(&app.session, paths)
}

/// `enqueue` 的**命令体**（[`enqueue`] 的唯一实现，单测直接调它）。
///
/// 三步（规格 §5.3）：取数据（**会话快照**里那一批的全部文件）→ 判据（`kernel_paths`
/// 里那一次 `DownloadTargets::paths`）→ 交给内核。
fn enqueue_with(session: &Arc<Session>, paths: Vec<String>) -> Value {
    // 失败文案要按界面语言成句（规格 W1）。
    let lang = session.lang();
    let Some(client) = session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 锁的纪律：`client()` 与 `view()` 都是"读一次就放锁"，两次临界区都在这一行之内，
    //    之后的 `kernel::enqueue` 手里没有会话锁（见文件头那条）。
    let paths = kernel_paths(&session.view().load, &paths);
    // 🔴 大批量不再被拒（2026-10-12）：超出单条请求预算的路径串被切成若干块**依次**发给内核；
    //    内核对每块只同步提交前 100 个、其余后台陆续加入（`pending`），所以每一块都很快返回。
    //    · 单条就超预算的路径不进任何一块（它发出去也只会让内核不回话），逐条作为 rejected 交代；
    //    · 整批全选的 `paths` 为空：仍发一条空的（= 全部待下载）；
    //    · 第一块失败 ⇒ 照旧走失败信封（什么都没加进去）；
    //    · 后面的块失败 ⇒ 前面已经入队的**保留**，没发出去的合成一条汇总（不静默少交）。
    let mut merged = shell_core::protocol::EnqueueResult {
        added: Vec::new(),
        rejected: DownloadTargets::oversized(&paths)
            .into_iter()
            .map(|path| shell_core::protocol::RejectedTask {
                path,
                reason: OVERSIZED_PATH_REASON.to_string(),
            })
            .collect(),
        pending: 0,
    };
    let chunks = if paths.is_empty() {
        vec![Vec::new()]
    } else {
        DownloadTargets::chunks(&paths)
    };
    for (i, chunk) in chunks.iter().enumerate() {
        match kernel::enqueue(lang, &client, chunk) {
            Ok(mut r) => {
                merged.added.append(&mut r.added);
                merged.rejected.append(&mut r.rejected);
                merged.pending += r.pending;
            }
            Err(why) if i == 0 => {
                note_failure(session, &why);
                return api::failure(&why);
            }
            Err(why) => {
                // 中途失败：这一次动作的死活照常记（引擎徽标要翻、错误要上屏），没发出去的剩余
                // 合成**一条**汇总——逐条展开会把界面撑爆（十几万行）。内核死了的话，它此前答应
                // 后台陆续加入的那些也不会再加了，计入「未提交」，不许再说"正在后台加入"。
                note_failure(session, &why);
                let rest: usize = chunks[i..].iter().map(Vec::len).sum();
                let orphaned = if matches!(why, CallFailure::KernelGone { .. }) {
                    std::mem::take(&mut merged.pending)
                } else {
                    0
                };
                merged.rejected.push(shell_core::protocol::RejectedTask {
                    path: format!("未提交 {} 个", rest + orphaned),
                    reason: why.text().to_string(),
                });
                return api::enqueue::enqueue(&EnqueueFeedback::of(&merged));
            }
        }
    }
    session.set_last_error(None);
    // ⚠️ **全部块都成功 ⇒ 引擎在跑**（对齐 macOS 入队尾部的 `if allSent { engine =
    //    .running }`，`AppModel.swift:1822` —— 成功 ⇒ 内核在 `op_enqueue` 里已经
    //    `ensure_engine` 过了）。走到这里必然全成功：任何一块失败都在上面提前 `return`，
    //    那时 `note_failure` 刚写下的结论（`.unavailable` / `.notStarted`）才是事实，
    //    不许被这行覆盖。
    //    ⚠️ 少了这一行，入队之后徽标停在「引擎未启动」而列表已在下载（2026-10-14 现场）。
    session.set_engine(EngineState::Running);
    // 目标盘可能比网慢（现场 1.7 MB/s 是移动硬盘写的锅，不是 CDN）：量一次、粘滞进会话。
    // 量不出来就不提示 —— 提示是"额外的好意"，不许让它变成一次失败。
    if let Ok(preferences) = load_preferences() {
        note_disk_speed(session, Some(&preferences.download_dir), false);
    }
    api::enqueue::enqueue(&EnqueueFeedback::of(&merged))
}

/// 单条路径超过单次请求上限、没有发送时的那句话（与 macOS `AppModel` 逐字相同）。
const OVERSIZED_PATH_REASON: &str = "这一项路径太长，超过单次请求上限，没有发送";

/// 勾选面 → **交给内核的 `paths`**：C-3 那条判据（`DownloadTargets::paths`）在本 crate 的
/// **唯一生产调用点**（`kernel::enqueue` 只负责把它发出去，**不碰那个判据** ——
/// 那是它的文档里写着的承诺，这一行就是兑现）。
///
/// ## ⚠️ `all_paths` 从**已有的会话状态**取，不额外问内核一次
///
/// 判据要的第二个实参是"这一批的**全部文件路径**"。它在 `shell-core` 里有两个
/// 来源（`BrowserSelection::all_files_of` 的文档记着它们的等价性）：
///   * `get_tree` 的 `flat[]` —— **要再问内核一次**，而它会把整棵树重新传一遍；
///   * `load_delivery` 回执里那棵树 —— **已经在会话里**（`SessionView::load` 的
///     `LoadState::Loaded(info)`）。
/// 这里取后者。理由不只是省一次往返：
///   * 内核那两条出口本来就是同一份清单的两种形状（`view::build_tree(&m.files)`），
///     文件集合逐字相同 —— 所以省下的这一次**不改变答案**；
///   * **没有"这份缓存什么时候失效"这个问题**：`Loaded(info)` 本身就是"info 是当前
///     这一批"这句话，换批（`set_load`）与批次作废（`reset_to_empty_state`）都由它
///     一个人说了算，不存在第二份记账与它分叉（那正是本仓库对"壳自己缓存一份"的既有
///     顾虑，见 `get_settings` 的文档）。
///
/// ## ⚠️ 没有生效批次 ⇒ 空集 ⇒ **逐条发出去**（保守的那一边）
///
/// `Idle` / `Loading` / `Failed` 三档一律交一个**空集**给判据。这不是"忘了"：
/// `DownloadTargets::paths` 对 `all_paths` 为空**本来就有一条分支**
/// （"一批是空的时不算整批"，`an_empty_batch_is_not_everything` 钉着）⇒ 结果是
/// **显式列表**。
/// ⚠️ 于是"壳不知道这一批有什么"与"这一批真的一个文件都没有"落成同一个动作，而
///    两条都**不该**发 `[]`：那会被内核读成"下全部待下载"，替用户做了一个他没做过的选择。
///
/// ## ⚠️ 两个集合的类型是 `BTreeSet`（判据的签名要的）
///
/// 前端给的是一个**数组**（顺序即它勾选的顺序），而判据是**集合相等** —— 这里转一下
/// 是那条签名（`paths(selection: &BTreeSet<String>, all_paths: &BTreeSet<String>)`）
/// 要的，不是"顺手排个序"。顺序确定这件事在 `BTreeSet` 里是结构性的
/// （见 `download_targets.rs` 的模块头）。
fn kernel_paths(batch: &LoadState, selection: &[String]) -> Vec<String> {
    let all_paths = match batch {
        LoadState::Loaded(info) => BrowserSelection::all_files_of(&info.tree),
        LoadState::Idle | LoadState::Loading | LoadState::Failed(_) => BTreeSet::new(),
    };
    let selection: BTreeSet<String> = selection.iter().cloned().collect();
    DownloadTargets::paths(&selection, &all_paths)
}

/// 「暂停全部 / 一键开始」两颗按钮的状态（判据在 `BatchControl::of`，本层只喂它输入）。
fn batch_control(rows: &[TransferRow]) -> shell_core::presentation::batch_control::BatchControl {
    shell_core::presentation::batch_control::BatchControl::of(
        rows,
        batch::resume_pending(),
        batch::migrate().as_ref(),
    )
    .with_migrate_failure(batch::migrate_failure().as_ref())
}

/// 传输列表（规格 §3.4 第六行，对齐第二代 `GET /api/transfers`）。
///
/// ⚠️ 前端按 **200 ms** 轮询它（`TransferListPoll::INTERVAL_NANOSECONDS`），
///    而闸门是 `state().allows_requests`（`EngineGate`，规格 §3.5）。
#[tauri::command(async)]
pub fn transfers(app: tauri::State<'_, Shell>) -> Value {
    transfers_with(&app.session)
}

/// `transfers` 的**命令体**（[`transfers`] 的唯一实现，单测直接调它 —— Tauri 的
/// `State` 只有运行时造得出来，同 [`enqueue_with`] 的拆法）。
fn transfers_with(session: &Arc<Session>) -> Value {
    let Some(client) = session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 空列表那句话要用到引擎态 ⇒ 先取一份快照（锁只在这一行里）。
    let engine = session.view().engine;
    // 空态文案要按界面语言成句（规格 W1）：取一次，两处空快照共用。
    let lang = session.lang();
    // ⚠️ 两个翻格判据**先算出来再 `match`**：`Snapshot` 臂按值拿走了 `result` 之后
    //    `poll` 就动不了了。判据本身住在 `TransferPoll`（四档各有一条用例钉着）。
    let poll = kernel::transfer_list(lang, &client);
    let promote_running = poll.promotes_engine_to_running();
    let flip_not_started = poll.flips_engine_to_not_started();
    match poll {
        TransferPoll::Snapshot(result) => {
            session.set_last_error(None);
            // ⚠️ **成功 ⇒ 引擎在跑**（同 `task_action` 那一行；判据与四档表在
            //    `TransferPoll::promotes_engine_to_running`）。少了这一行，列表在
            //    下载、徽标停在「引擎未启动」（2026-10-14 现场的那条缺口）。
            if promote_running {
                session.set_engine(EngineState::Running);
            }
            let rows: Vec<TransferRow> = result.items.iter().map(|i| TransferRow::new(lang, i)).collect();
            api::transfers::transfers(
                &rows,
                Some(&TransferGlobalSummary::of(lang, &result.global)),
                TransferListEmpty::of(lang, &engine),
                &batch_control(&rows),
            )
        }
        // ⚠️ 内核亲口说的**正常态**（还没添加过任务 / 这一批没了）⇒ 发 `global: null` 的
        //    空快照，由前端呈现成**空态文案**，不是红横幅（判据在 `TransferPoll` 的文档里）。
        //    ⚠️ `global` 为 `null` 与"编一个 0 / 0 / 0"是两件事：那条内核回执里**根本没有**
        //    `global` 这个字段，替它填三个 0 就是替内核说了一句它没说的话。
        //    ⚠️ 这一档**不碰 `last_error`**（第二代逐字如此）：它不是一次失败。
        TransferPoll::Absorbed => {
            // ⚠️ **引擎那一格要跟着内核的话走**：判据（哪一档翻、哪一档不翻）在
            //    `TransferPoll::flips_engine_to_not_started` 里（四档各有一条用例），
            //    本行只负责把它落到会话上。
            //    ⚠️ 它**不影响**闸门：`EngineGate::allows_requests` 对 `NotStarted` 与
            //    `Running` 都给 `true`（那一格回答的是"能不能发请求"，而 `enqueue` 正是
            //    靠 `NotStarted` 放行才会去起引擎的）。
            if flip_not_started {
                session.note_engine_not_started();
            }
            api::transfers::transfers(&[], None, TransferListEmpty::of(lang, &engine), &batch_control(&[]))
        }
        // 🔴 **内核亲口说"这一批没了"**（`no_delivery`）：除了发空快照，还要**作废当前批次**
        //    （清掉 `load_request` —— 对齐 macOS 的 `resetToEmptyState()`）。
        //    ⚠️ 少了这一句，那个死码会在**下一次内核重启**（改下载目录 / 点「重试」）时被
        //    **重放**一遍，用户看到一次本不该出现的失败 —— 而 macOS 在那一刻是**空态**
        //    （本任务审查 I-1 抓的）。
        //    ⚠️ 这一档**不碰 `last_error`**：macOS 的 `absorb` 把它当 `.absorbed`
        //    （正常态、不显示那句话），我们照办 —— 界面上的表现就是**空态**，不是红横幅。
        TransferPoll::NoDelivery => {
            session.reset_to_empty_state();
            api::transfers::transfers(&[], None, TransferListEmpty::of(lang, &engine), &batch_control(&[]))
        }
        TransferPoll::Failed(why) => {
            note_failure(session, &why);
            api::failure(&why)
        }
    }
}

/// 校验结果（规格 §3.4 第七行，对齐第二代 `GET /api/verify`）。
///
/// ⚠️ 前端**只在校验页可见时**按 1 s 轮询它（规格 §3.5），否则一发都不发。
#[tauri::command(async)]
pub fn verify(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::verify_status(lang, &client) {
        Ok(status) => {
            app.session.set_last_error(None);
            // ⚠️ `VerifySummary::of` 收 `Option`，而"还没取过"（`None`）那一档**在这里
            //    就分掉了**：走到这一行的每一条路都刚拿到内核回的 `status` ⇒ 恒 `Some`。
            //    ⚠️ 那句"算不出摘要"的话**不在本文件里**：它是一句面向用户的失败文案，
            //    按纪律（规格 §3.2 在 Rust 侧的同一形态）归 `shell-core::api`。
            //    它今天的不可达性、以及**为什么宁可留一句也不许 panic** 都写在
            //    `api::verify::NO_VERIFY_SUMMARY` 的文档里 —— 别把它搬回这里。
            match VerifySummary::of(app.session.lang(), Some(&status)) {
                Some(summary) => api::verify::verify(&summary),
                None => api::envelope::err(api::verify::NO_VERIFY_SUMMARY),
            }
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

// ===========================================================================
// 更新提示的三条命令（规格 §4；内核侧的三条由 `core/src/kernel.rs` 给）
// ===========================================================================

/// 更新状态（规格 §4）。**只读**，壳按宽节拍（60 s）轮询它，把结果画成文件页顶部
/// 那条**独立的**提示条。
///
/// ## ⚠️ 这一条**不会给界面冒出错误**（那是内核那一侧的判据）
///
/// 内核的 `op_update_status` **任何取不到的情形都回成功**（`has_newer:false`）——
/// 网络抖动、Gitee 不可达、`update.json` 坏了，全部落到"没有新版"上，**不是错误**
/// （规格 §5：失败一律静默）。所以壳这一侧**唯一可能**的失败是"没有内核 / 传输断了"，
/// 而那件事由 `state()` 的节拍在别处说出来。
///
/// 🔴 **因此本命令失败时走 `api::failure`（失败信封）但 `note_failure` 一个字都不写**：
///    `note_failure` 对 `CallFailure::Text` 会写 `session.last_error`，而那一格**会上屏**
///    （常驻提示行）—— 那就等于把"更新检查"这条静默的后台动作变成一句客户看得见的报错，
///    正是规格 §5 明禁的形态。内核没了那件事由 `state()` / `transfers()` 的节拍去报，
///    这一条**只把原文交给那次调用的回执**（前端对它的失败**只记控制台、不上屏**）。
#[tauri::command(async)]
pub fn update_status(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::update_status(lang, &client) {
        Ok(kernel_value) => {
            // 成功 ⇒ 清掉上一次失败（那一条更新检查恢复了），与其余命令同一条非粘滞口径。
            app.session.set_last_error(None);
            api::update::update_status(&kernel_value)
        }
        // ⚠️ **不调 `note_failure`**（理由见上）：这一条失败**静默**。
        Err(why) => api::failure(&why),
    }
}

/// 「启动时检查更新」那个开关（规格 §4）。**写内核自己的开关**（`update.json`，
/// 不塞进 `settings.json`），回执是**写完之后**那一份状态（与 `settings_set` 同口径）。
///
/// **对齐 macOS**：`SettingsView` 那个 `Toggle("启动时检查更新")` 的绑定。
///
/// ⚠️ 它与 [`update_status`] 不同：**这一条是一次用户动作**（点了勾选框），
///    失败是"你刚才那一下没成"，该照登（同 `settings_set` 那一档）。⚠️ 但它**不重启内核**
///    （开关状态在内核的 `update.json` 里，内核下一次检查时自己读）—— 与
///    `verbose_logging_set` 那种"改了要重启"的开关**不是一回事**。
#[tauri::command(async)]
pub fn update_set_enabled(app: tauri::State<'_, Shell>, enabled: bool) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::update_set_enabled(lang, &client, enabled) {
        Ok(kernel_value) => {
            app.session.set_last_error(None);
            api::update::update_status(&kernel_value)
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 「立即检测」：设置里那颗手动检查的按钮（规格 §2）—— **一次用户动作**，回执有一句人话。
///
/// ## 🔴 它回答的正是 [`update_status`] 回答不了的那个问题
///
/// 内核的 `update_status` 在"这次没查成"与"查成了、确实没有新版"两种情形下**长得一样**
/// （`has_newer:false`：网络抖动、Gitee 不可达、发布页上最新那条是预发布，全部落在它上面）。
/// 对那条**后台**提示条这是对的（规格 §5：没人做动作，界面不该冒话），但对**客户点出来的**
/// 这一下就成了假话 —— 断网时那颗按钮只会说"已经是最新的"。⇒ 内核因此多了一格
/// `last_check_ok`（见 `core/src/kernel.rs` 的 `Kernel::update_last_check_ok`），
/// **判决与那五句话**住在 `shell_core::presentation::update_check`，本命令只把
/// `update_check_now` 回来的那一份交给它装帧（`api::update::checked`）。
///
/// ⚠️ **它自己不问 `enabled`**：开关关着时内核那一条**一道请求都不发**就直接返回
///    （既有行为，注释称"规格 §5"），`UpdateCheck::of` 把这一档如实报成
///    "刚才没有去查" + 补救 —— **不许**报成"已经是最新的"（那是同一句假话的另一个入口）。
///
/// ⚠️ **失败照登**（`note_failure`）：与 [`update_status`] 那条"静默的后台动作"**不同**，
///    这一条是用户点出来的 ⇒ 没有内核 / 传输断了必须让用户知道自己那一下没成
///    （同 [`update_set_enabled`] 那一档）。而内核**自己的**失败不进这里：
///    它落进 `last_check_ok:false`、由那句回执说（那是"没查成"，不是"命令没发出去"）。
/// 自动重启的进度（规格 W3）：**轮询**它来画「正在重启下载引擎（第 n/3 次）…」。
///
/// ⚠️ 取不到 ⇒ 回 `{"text": null}`（不是错误）：这是一条**装饰**命令，
///    拿不到就什么都不说，不把一次读失败升级成红条。
/// 双域名竞速的解析过程（规格 W4）：加载清单期间轮询它，把四阶段逐行摆出来。
///
/// ⚠️ 取不到 ⇒ 回 `{"lines": []}`（不是错误，同 `restart_progress` 那条）。
#[tauri::command(async)]
pub fn delivery_progress(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::ok(serde_json::json!({"lines": []}));
    };
    match kernel::delivery_progress(lang, &client) {
        Ok(v) => api::envelope::ok(serde_json::json!({
            "lines": v.get("lines").cloned().unwrap_or_else(|| serde_json::json!([])),
        })),
        Err(_) => api::envelope::ok(serde_json::json!({"lines": []})),
    }
}

#[tauri::command(async)]
pub fn restart_progress(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::ok(serde_json::json!({"text": null}));
    };
    match kernel::restart_progress(lang, &client) {
        Ok(v) => {
            let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("idle");
            let attempt = v.get("attempt").and_then(|n| n.as_u64()).unwrap_or(0) as u32;
            let budget = v.get("budget").and_then(|n| n.as_u64()).unwrap_or(0) as u32;
            let notice = shell_core::presentation::restart_notice::RestartNotice::of(
                lang, state, attempt, budget,
            );
            api::envelope::ok(match notice {
                Some(n) => serde_json::json!({"text": n.text}),
                None => serde_json::json!({"text": null}),
            })
        }
        Err(_) => api::envelope::ok(serde_json::json!({"text": null})),
    }
}

#[tauri::command(async)]
pub fn update_check_now(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::update_check_now(lang, &client) {
        Ok(kernel_value) => {
            app.session.set_last_error(None);
            let check = UpdateCheck::of(&kernel_value);
            api::update::checked(&check)
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 「去下载」：把内核给的那条更新链接交给**系统默认浏览器**（R15）。
///
/// ## ⚠️ 为什么是**这一条窄语义**的命令，而不是通用的 `open_url`
///
/// 一条"打开任意 URI"的壳命令等于把 webview 变成一台**启动器**（前端一旦被注入，
/// 就能让壳替它拉起任意协议/站点）。而本功能的安全属性恰恰是**"客户只可能被带到那一个
/// 官方仓库"**（规格 §1 原话）。⇒ 命令名写死成"更新"这一件事，
/// **白名单写死在 `shell_win::openurl`**，不接受任何"再放行一个域名"的参数。
///
/// ## 🔴 白名单在 **Rust 这一侧**判，前端**一个字都不判**
///
/// 前端是**可被注入的那一侧**（承诺它自己会判 = 没有承诺）—— 白名单是那段威胁模型里
/// 唯一还站得住的位置。判据 `crate::openurl::is_allowed` 是**纯函数**（宿主上可断言），
/// 而"不匹配 ⇒ 拒绝、一次系统调用都不发"由 `openurl::open` 保证。
///
/// ⚠️ **它不碰会话状态**（同 `reveal`：这一次动作**没有经过内核**，不知道该清谁的错、
///    也不该翻引擎那一格）。失败只走**回执那一格**（`is_failure` 由信封形状决定）。
/// ⚠️ 实参是**内核给的那条 URL 原文**（`update_status().url`），本层不拼、不改写。
#[tauri::command]
pub fn open_update_url(url: String) -> Value {
    match openurl::open(&url) {
        openurl::Outcome::Opened => api::update::opened(),
        // 不在白名单里 ⇒ 壳自己的锁拦下的（R15）：拒绝、什么都不打开。
        openurl::Outcome::Refused => api::update::refused(),
        // 在白名单里、但系统那一半没能打开（原样带上原因 + 补救）。
        openurl::Outcome::Failed(why) => api::update::open_failed(&why),
    }
}

// ===========================================================================
// 任务 8 补的命令（规格 §3.4 那张表的下半张）
//
// **十个**是那张表列出的（task_action / reveal / settings_get / settings_set /
// preferences_get / preferences_set / history_get / history_put / license / about），
// 外加**一个表里没有的** `preferences_check` —— 它的文档里写清了"为什么非有它不可"
// （`DownloadDirectory::check` 的时机与那三条后果的确认文案都需要一个落点）。
//
// ⚠️ 十个里有**两个是这一代补的真缺口**（规格 §3.4 的 ⚠️ 逐字）：
//    `task_action` 与 `reveal` 在第二代的路由表里**根本不存在** ——
//    `TransferRow.available_actions` 算得出动作、却没有任何端点去执行它们。
//    它们**不是既有能力的搬运**，所以下面每一个都写清了"对齐的是 macOS 的哪一段"。
//
// ⚠️ 每一个的成功载荷里那几格，**文案一律来自 `shell_core::api::*`**
//    （本文件里没有一个面向用户的字符串字面量）—— 这条纪律的由来见 `api::NO_KERNEL`
//    的文档；任务 7 的报告里也记着"就这一句差点破墙"那次。
// ===========================================================================

/// 传输列表上的一个动作：暂停 / 继续 / 重试 / 移除 / 清空已完成（规格 §3.4 第八行）。
///
/// **对齐 macOS**：`AppModel.taskAction(_:gid:)`（`AppModel.swift:1406`）。
///
/// ⚠️ **实参 `action` 是 `TaskAction` 类型**（不是 `String`）：五个动作是**线上字面量**
///    （内核按字面量匹配），而 Tauri 的反序列化会挡住"名字写错/多了个动作"这一类
///    —— 与 `enqueue` 的 `Vec<String>` 是同一条纪律（**防线前移到类型上**）。
///    ⚠️ 于是 `reveal` 这个**动作名**（内核明确不认它，见 `protocol.rs` 里那条注释）
///    在这里根本构造不出来：那是**壳的事**，它有自己的命令。
///
/// ⚠️ **动作成功 ⇒ 引擎在跑**（macOS 那一行 `engine = .running` 同一个理由）：
///    内核的 `op_task_action` 走的是 `require_engine_for_action`（它过了才回 `ok`）
///    ⇒ 这不是猜测，是内核的结论。少了这一行，引擎徽标会停在「引擎未启动」，
///    而**没有任何东西会变红**（图标与文案都是既有判据算的，只是输入那一格过期了）。
#[tauri::command(async)]
pub fn task_action(
    app: tauri::State<'_, Shell>,
    action: TaskAction,
    gid: Option<String>,
) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::task_action(lang, &client, action, gid) {
        Ok(()) => {
            // 一键开始成功 ⇒ 「刚换过目录、队列还没补回来」这件事办完了；搬运的结果话也不用再留。
            if action == TaskAction::ResumeAll {
                batch::clear_after_resume();
            }
            app.session.set_last_error(None);
            app.session.set_engine(EngineState::Running);
            // ⚠️ `status` 是**状态令牌**（与 `load` 的 `loading` / `no_kernel` 同一个口径）：
            //    不是显示文案，前端只拿它分支。**动作成功没有别的话可说** ——
            //    这一下改的是内核里的任务，界面下一拍（200 ms）的 `transfers()` 就会显示新状态。
            api::envelope::ok(serde_json::json!({ "status": "done" }))
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 在资源管理器中显示某个文件（规格 §3.4 第九行）。
///
/// **对齐 macOS**：`TransfersView.reveal(_:)`（`TransfersView.swift:212`）—— 那边走
/// `NSWorkspace.activateFileViewerSelecting`，这边走 `ShellExecuteW + explorer /select,`
/// （平台那一半在 `crate::reveal`，含"为什么它是平台专属的"）。
///
/// ⚠️ **实参是"清单相对路径"**（`TransferRow.manifest_path` 那一格，内核给的原文），
///    **不是**磁盘路径：怎么把它换算成落盘路径**只有一处实现**
///    （`shell_core::api::reveal::local_path` → `TransferReveal`），本文件**不拼路径**。
///    前端把 `transfers()` 里那一行的 `manifest_path` 原样发回来即可。
///
/// ⚠️ **它不碰会话状态**（不清 `last_error`、不翻引擎）：这一次动作**没有经过内核**
///    （规格 §5.2：这是壳的事），所以它既不是"一次成功的请求"、也不该把引擎徽标
///    弄成别的样子。失败**只走失败信封**，由界面决定摆在哪（macOS 摆在传输页那条
///    失败栏里；我们这一侧的前端还没做，任务 11/12 定）。
#[tauri::command]
pub fn reveal(path: Option<String>) -> Value {
    // ⚠️ 本命令**不收 `State<'_, Shell>`**：它一次都不碰 `Session`（上面那段文档说了
    //    为什么 —— 这件事不经过内核）。要数据就去偏好文件里读，要环境就问进程。
    //    收一个用不到的参数只会让下一个人以为"这里迟早要用它"。
    // 下载目录是偏好里那一份（**空串 = 未配置** ⇒ 落盘根是内核默认值）。
    let download_dir = match load_preferences() {
        Ok(preferences) => preferences.download_dir,
        // 读到偏好的**存放根**都取不到（`%APPDATA%` 缺失）⇒ 那句话说清了根因与补救。
        Err(why) => return api::reveal::failure(&why),
    };
    match reveal::reveal(
        path.as_deref(),
        &reveal::process_environment(),
        &download_dir,
    ) {
        // 已经交给资源管理器了（那一格里的绝对路径不必回给前端：界面上没有它的位置，
        // 而"把一条路径铺进常驻行"正是一次布局事故的根因）。
        shell_core::api::reveal::Outcome::Ready(_) => api::reveal::revealed(),
        // 这一行没有路径映射 ⇒ **不是失败**（界面那一项本来就该是禁用的）。
        shell_core::api::reveal::Outcome::NoManifestPath => api::reveal::no_path(),
        // 算不出文件在哪 ⇒ **要说出来**（W-2）：用户点了那一项、什么都没发生，
        // 唯一的解释就在这句话里。变量名按**本靶**取（Windows 是 USERPROFILE）。
        shell_core::api::reveal::Outcome::RootUnknown => {
            api::reveal::root_unknown(TransferReveal::home_variable())
        }
        shell_core::api::reveal::Outcome::Failed(why) => api::reveal::failure(&why),
    }
}

/// 参数面板打开时要显示的东西（规格 §3.4 第十行）。
///
/// **对齐 macOS**：`AppModel.performHandshake` 里那一条 `get_settings`
/// （`AppModel.swift:574`）+ `SettingsView` 的渲染。
///
/// ⚠️ **内核那一份是权威**：壳**不缓存一份自己的设置**（缓存 = 与内核那份各走各的，
///    手改过 `settings.json` 时两边会分叉，而**没有任何东西会变红**）。
///    `-k` 的候选集合是个例外 —— 它在**握手的回执**里（`get_settings` 不回它），
///    由 `api::settings::payload` 从 `SessionView.handshake_reply` 那一格解析。
#[tauri::command(async)]
pub fn settings_get(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 锁的纪律：`view()` 与 `client()` 都是"读一次就放锁"（`-k` 的候选集合要从
    //    这一格里取，而它是一份 `String` 的克隆 —— 与内核无关）。
    let handshake = app.session.view().handshake_reply;
    match kernel::get_settings(lang, &client) {
        Ok(current) => {
            app.session.set_last_error(None);
            api::settings::payload_for(&current, handshake.as_deref(), app.session.lang())
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 写七项参数（规格 §3.4 第十一行）。
///
/// **对齐 macOS**：`AppModel.applySettings(_:)`（`AppModel.swift:1442`）。
///
/// ⚠️ **它不重启内核**：七个字段里没有一项走 argv —— 内核的 `set_settings` 把
///    `-j` 与限速**即时**下发给正在跑的引擎，其余各项在**添加新任务**时生效
///    （`SettingsForm::apply_note(crate::i18n::Lang::Zh)` 说的就是这件事）。改**下载目录**是另一条命令
///    （`preferences_set`）—— 那一个**必须**重启内核，因为目录只在 argv 上。
///
/// ⚠️ **回执发的是"写完之后"那一份**（内核的 `set_settings` 回的是归一化之后的值，
///    例如 `-k` 的 `"21m"` 会被归一成 `"21M"`）—— 与 `settings_get` **同一个形状**，
///    前端只有一条渲染路径。⚠️ 让它回读一次（再发一条 `get_settings`）是多余的：
///    内核这条响应本身就是"现在生效的那一份"。
#[tauri::command(async)]
pub fn settings_set(app: tauri::State<'_, Shell>, settings: Settings) -> Value {
    let lang = app.session.lang();
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    let handshake = app.session.view().handshake_reply;
    match kernel::set_settings(lang, &client, &settings) {
        Ok(echoed) => {
            app.session.set_last_error(None);
            // 客户这次明确选过语言了（首启询问从此不再出现）。
            app.session.note_language_chosen(&echoed.settings.language);
            api::settings::payload_for(&echoed, handshake.as_deref(), app.session.lang())
        }
        Err(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 下载目录那一段的**只读部分**（规格 §3.4 第十二行）。
///
/// **对齐 macOS**：`AppModel.downloadDir` + `SettingsView` 的下载目录段。
#[tauri::command]
pub fn preferences_get(app: tauri::State<'_, Shell>) -> Value {
    let lang = app.session.lang();
    match load_preferences() {
        Ok(preferences) => api::preferences::current(lang, &preferences),
        // 存放根取不到（`%APPDATA%` 缺失）⇒ **大声失败**：那句话点名了缺的是哪个变量、
        // 并给出补救（W-2）。**不许**悄悄当成"未配置"（那会让用户以为设置丢了）。
        Err(why) => api::envelope::err(&why),
    }
}

/// **准备改目录**：这个位置能不能用 + 改了会怎样（规格 §3.4 那张表里**没有**这一条）。
///
/// ## ⚠️ 为什么必须多出这一条命令（偏离规格表的理由，逐条）
///
/// `presentation::app_preferences` 的 `DownloadDirectory::check` 的头注记着这条判据的
/// **时机**：目录不可用要**在确认对话框之前**就报出来。理由是量级 —— 等用户看完
/// "正在跑的任务会停"、点了确认、内核重启完才发现"这个文件夹不可写"，他已经为它
/// **停掉了一次正在跑的下载**。
///
/// 而"确认对话框"与"选目录"都在**前端**（规格 §6.2 的设置窗口），于是：
///   * 前端要拿到 `check(path)` 的结论 ⇒ 必须有一条命令（它要问文件系统，JS 问不了）；
///   * 前端要拿到"三条后果"那段 `confirmation(from:to:)` ⇒ 也必须有一条命令
///     （§3.2：JS **不许**自己拼接那句中文）。
///
/// ⇒ 两条需要合成**同一个时机的同一次调用**（"准备改到某个目录"），就是本条。
/// 规格 §3.4 只列了 `preferences_get()` / `preferences_set(dir)`，**没有这一条** ——
/// 这是**这一代补的第二个缺口**（第一个是 `enqueue`，见 R-13）：没有它，
/// `app_preferences` 那两句现成的文案**没有任何落点**（前端只能自己写一份中文，
/// 而那正是 §3.2 明禁的）。
///
/// ⚠️ 它**没有任何副作用**（不写盘、不碰内核）：`check` 会在候选目录里建一个探针文件
///    再立刻删掉（那是它的判据，见 `probe_writable`），除此之外什么都不改。
#[tauri::command]
pub fn preferences_check(path: String) -> Value {
    // 确认文案要说清"从哪改到哪" ⇒ 得先知道**现在**用的是哪个目录。
    let current = match load_preferences() {
        Ok(preferences) => preferences.download_dir,
        Err(why) => return api::envelope::err(&why),
    };
    api::preferences::plan(
        DownloadDirectory::check(&path),
        DownloadDirectory::confirmation(&current, &path),
    )
}

/// 弹一次系统「选择文件夹」，把用户点到的那一个回给前端。
///
/// **对齐 macOS**：`SettingsView.swift:230-247` 的 `choose()`（`NSOpenPanel` +
/// `canChooseDirectories` + `canCreateDirectories`）。真机来由（2026-09-20）：
/// 本面板原先只能把路径**粘**进输入框，用户要的是"点着选"。
///
/// `data` 是**那个路径**，或 **`null`**（= 用户取消）——**取消不是错误**：
/// 取消一个对话框什么都不该发生，把它报成错误会让常驻提示行无缘无故亮一条。
/// ⚠️ **这一条不许回空串**：前端拿到 `""` 会把它当成"恢复默认"
///    （`preferences_set` 的既有语义：空串 = 回到未配置）⇒ 用户取消一次、
///    却弹出一个"更改下载目录？"的确认框。那条判据在
///    [`shell_win::pickdir::pick_outcome`]（纯函数，有单测），本层只是把它交出去。
///
/// ⚠️ **它照样要套信封**（`invoke.js:call` 只认 `{ok,data}`：一个裸 `null` 会被
///    读成"命令层忘了套信封" ⇒ 抛出一句**空话**）—— 所以返回的是 `Value` 而不是
///    `Option<String>`。取消落在 `data: null` 上，前端读那一格。
///
/// ⚠️ **它不检查、也不写任何东西**（不碰偏好、不碰内核）：选出来的路径走的是与
///    手工输入**完全相同**的那条路（前端 `settings.js:askDirectory`：检查 → 确认 →
///    写偏好 → 重启内核）。在这里顺手判一下"这个目录能不能用"会让
///    "什么算合法"出现第二个答案。
///
/// ⚠️ 它是**唯一一条会弹出系统对话框的命令**（`reveal` 只是把资源管理器叫起来）——
///    它会**阻塞调用它的那一拍**（`SHBrowseForFolderW` 是模态的）。前端必须在
///    「选择…」那颗按钮自己身上等它，不许拿它去挡别的动作。
/// ⚠️ 它对**运行期 `R` 是泛型的**：Tauri 的 `WebviewWindow<R>` 是一个泛型类型，
///    而 `invoke_handler::<R>` 把 `R` 传进来（写死 `tauri::Wry` 的话这一条只在
///    真交付形态下编得过，宿主上的单测直接编不过）。
#[tauri::command]
pub fn pick_directory<R: tauri::Runtime>(window: tauri::WebviewWindow<R>) -> Value {
    api::envelope::ok(match pickdir::pick_folder(owner_hwnd(&window)) {
        Some(path) => Value::String(path),
        None => Value::Null,
    })
}

/// **导出诊断日志**（规格 §2.5）。选一个**目标文件夹**，在里面建一个带时间戳的子目录。
///
/// **对齐 macOS**：`SettingsView` 那一节「诊断」里的导出按钮（`NSOpenPanel` + 拷贝 +
/// 说明文件；字段表见规格 §2.6）。
///
/// ## 四步（规格 §2.5 逐字）
///
///   ① 选一个**目标文件夹**（复用 `pick_directory` 那条既有能力：同一个
///      `pickdir::pick_folder`，**同一个**系统对话框）；
///   ② 在里面建一个**带时间戳的子目录**（名字由 `shell_core::export::subdirectory_name`
///      算 —— 多次导出不互相覆盖）；
///   ③ 拷进去那几份日志（**存在的才拷**，不存在的不算失败）+ 一份 `说明.txt`；
///   ④ 回执 = 那个子目录的路径 + 规格 §2.7 那句提醒（**都在 `api::diagnostics` 里**）。
///
/// ## ⚠️ 取消选择**不是失败**（`pick_folder` 回 `None` ⇒ 回执说"没有导出"）
///
/// 与 `pick_directory` 那条**同一条口径**（取消一个对话框什么都不该发生，
/// 把它报成错误会让常驻提示行无缘无故亮一条）。差别只有一处：这里**要说一句**
/// ——用户点的是「导出」，界面一动不动会与"卡住了"分不开（约束 4）。
///
/// ## ⚠️ 它**不碰会话状态**（不清 `last_error`、不翻引擎）
///
/// 与 `reveal` 同一条：这一次动作**没有经过内核**（规格 §5.2 的那一类）——
/// 它既不改变引擎的状态，也不该让"上一次失败"过去。失败**只走回执那一格**
/// （`is_failure: true`，界面据此换图标与颜色）。
///
/// ## ⚠️ 它照样会**阻塞这一拍**（与 `pick_directory` 同一条）
///
/// `SHBrowseForFolderW` 是模态的，而紧接着的拷贝与逐文件哈希也是同步的
/// （日志上界 12 MiB ⇒ 最多几十毫秒）。前端必须在那颗按钮自己身上等它。
///
/// ⚠️ 它对**运行期 `R` 是泛型的**（同 `pick_directory`：`invoke_handler::<R>` 把 `R`
///    传进来，写死 `tauri::Wry` 的话宿主上的单测直接编不过）。
///
/// ⚠️ **它比 `pick_directory` 多收一个 `State<'_, Shell>`**（计划里那行签名只有一个
///    `window`）：说明文件里"内核自报的协议版本"那一格住在**握手的回执**上
///    （`SessionView.handshake_reply`），而那是会话状态 —— 不收它就取不到，只能编一个
///    壳自己的常量，而那正是 `protocol_version: 未连上` 那条判据明禁的事。
///    ⚠️ 锁的纪律照旧：`view()` 是"读一次就放锁"，之后的系统对话框与拷贝都在锁外。
#[tauri::command]
pub fn diagnostics_export<R: tauri::Runtime>(
    app: tauri::State<'_, Shell>,
    window: tauri::WebviewWindow<R>,
) -> Value {
    // ① 选目标文件夹。取消 ⇒ 一句"没有导出"（**不是**错误信封，见函数文档）。
    let Some(target) = pickdir::pick_folder(owner_hwnd(&window)) else {
        return api::diagnostics::export_cancelled();
    };
    // ②③④ 的**事实**在这一段里取齐：说明文件要写的那七格（规格 §2.6 那张表）。
    //    ⚠️ 这些是**取数据**（规格 §5.3 的第一步），成句与摆位置在 `shell_core` 里。
    //
    // 🔴 **读时钟的只有下面这一句**（2026-10-06 终审）：这个时刻有**两个**落点 ——
    //    说明文件那格 `exported_at`，以及文件夹名 `诊断日志-YYYYMMDD-HHMMSS`。
    //    两处各读一次时钟的话，跨过秒边界时**同一个导出里那两个时刻会差一秒**，
    //    而它看起来完全正常（两个都是"合理的时间"，谁也看不出它们对不上）。
    //    ⇒ 取一次，两处都用它（`export_from` 的 `at` 参数就是为这条存在的）。
    let at = std::time::SystemTime::now();
    let header = Header {
        // 与 `about()` 同一个来源（`option_env!` 是编译那一刻的环境，`build.rs` 的
        // `rerun-if-env-changed` 盯的是同一个名字 —— 两处同源）。
        app_version: AboutInfo::version(
            option_env!("BENAGEN_VERSION"),
            Some(env!("CARGO_PKG_VERSION")),
        ),
        // 内嵌内核的**身份**（编译期常量，任何时候都拿得到 —— 不需要连上内核）。
        core_sha256: crate::export::embedded_core_sha256(),
        // 内核**自报**的协议版本。⚠️ 从握手的 `hello` 回执里取：连不上 ⇒ `None`
        // ⇒ 说明文件如实写"未连上"（**不许**回落到壳自己那个 `PROTOCOL_VERSION`）。
        protocol_version: shell_core::export::protocol_version_from_handshake(
            app.session.view().handshake_reply.as_deref(),
        ),
        // **这一格是灵魂**：没有它，看日志的人无法判断"这份日志为什么只有这么几行"
        // （是没问题，还是级别没开）。取值与内核的 `--log-level` **逐字同两个**
        // （`diagnostics::parse_level` 认的就是这两个串）。
        // ⚠️ 读不到偏好 ⇒ `normal`：壳自己那一档在那种情况下**确实**是 Normal
        //    （`apply_shell_log_level` 对空偏好就是这么落的），不是编的。
        log_level: match load_preferences() {
            Ok(preferences) if preferences.verbose_logging => "verbose".to_string(),
            _ => "normal".to_string(),
        },
        // 系统与版本（`GetVersionExW`；宿主那一支只回平台名）—— 见 `crate::export::os_version`。
        os: crate::export::os_version(),
        arch: std::env::consts::ARCH.to_string(),
        // 导出时刻（UTC，与目录名用的是**同一个**时刻 —— 见上面 `at` 那一段）。
        exported_at: shell_core::export::utc_stamp(at),
        // 每份文件的字节数 / sha256 / 覆盖范围由导出那一侧**量出来**（见
        // `shell-win/src/export.rs`），这里一个字都不编。
        files: Vec::new(),
    };
    match crate::export::export_to(&target, &header, at) {
        Ok(folder) => api::diagnostics::export_done(&folder),
        // 那句话是 `shell_core::export` 里**按失败的那一步**拼好的
        // （`folder_failure_text` / `copy_failure_text` / `header_failure_text` 三档，
        // 各自点名自己的那个位置、带上系统原话与下一步 —— 修复轮 1 拆的，
        // 见 `each_failure_names_the_step_that_actually_failed`），
        // 本层**一个字都不加工**（同 `preferences_set` 失败那一支的"照登"）。
        Err(why) => api::diagnostics::export_failed(&why),
    }
}

/// 主窗口的 `HWND`（**没有就退回空指针**），宿主上恒空。
///
/// ⚠️ `h.0` 那个字段访问是**跨 crate 的一个稳定面**：Tauri 用的是 `windows` crate 的
///    `HWND`（`pub struct HWND(pub *mut c_void)`），而 `shell-win` 用的是 `windows-sys`
///    的同名类型（也是 `*mut c_void`）—— 两棵树在这里**是同一个裸指针**，
///    所以不需要把 `windows` crate 引进来，也不需要 `transmute`。
///
/// ⚠️ 取不到句柄（窗口还没建好 / 平台不支持）**不报错**，退回空指针：
///    非模态的选择框仍然能用（只是可能被主窗口盖住、也可能被开出第二个）——
///    这比"因为取不到句柄就干脆不给选"好，而"取不到"这件事在这里没有任何可读的
///    处置（用户能做的仍是再点一次）。
#[cfg(target_os = "windows")]
fn owner_hwnd<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> *mut std::ffi::c_void {
    window.hwnd().map(|h| h.0).unwrap_or(std::ptr::null_mut())
}

/// 宿主那一支：`hwnd()` 是 Windows 专有的，这里恒空（宿主用 `osascript`，见 `pickdir`）。
#[cfg(not(target_os = "windows"))]
fn owner_hwnd<R: tauri::Runtime>(_window: &tauri::WebviewWindow<R>) -> *mut std::ffi::c_void {
    std::ptr::null_mut()
}

/// 改下载目录（规格 §3.4 第十三行）。**这是唯一能让新目录生效的入口**。
///
/// **对齐 macOS**：`AppModel.changeDownloadDir(to:)`（`AppModel.swift:850` 附近）。
/// 顺序是**硬要求**，四步（与那一段逐条对齐）：
///
///   ① **同值 ⇒ 什么都不做**（`Unchanged`）：确认文案里那句"正在跑的任务会停"说的是
///      **真的改了**的时候。用户在面板里选中**同一个**目录是很可能发生的
///      （"我只是想确认一下现在用的是哪个"），不该为此停掉他正在下的东西。
///   ② **偏好先落盘**（失败 ⇒ 中止，内核一个都不许动）：先重启再落盘的话，
///      重启中途失败/崩溃就会留下"内核在新目录里跑、盘上还记着旧目录"的分裂 ——
///      下次启动**静默**换回去。写盘失败那句话（`api::preferences::save_failed`）
///      说明了根因，而**盘上那一份一个字都没被动过**。
///   ③ **重启内核**：下载目录**只在 argv 上**（内核的 `set_settings` **没有**这个键）
///      ⇒ 不重启就不生效。⚠️ 重启的细节与它的**有界等待**见 [`restart_kernel`]。
///   ④ 回执是 `DownloadDirChange` 那四格里的一个 —— **成句在 `api::preferences` 里**，
///      本文件一个字都不自己写。⚠️ 失败那一支**不撤销**已经落盘的偏好：
///      那是用户的选择，下次启动仍然按它起内核（与 macOS 同一条）。
#[tauri::command(async)]
pub fn preferences_set(app: tauri::State<'_, Shell>, dir: String) -> Value {
    let lang = app.session.lang();
    let path = match preferences_path() {
        Ok(path) => path,
        Err(why) => return api::envelope::err(&why),
    };
    let current = Preferences::load(&path);
    // ② 之前先归一化（`setting_download_dir` 走 `AppPreferences::normalized`：
    //    只去首尾空白）—— 于是"多打了两个空格"不会被当成一次改动。
    let updated = current.setting_download_dir(&dir);

    if updated.download_dir == current.download_dir {
        return api::preferences::change(lang, &DownloadDirChange::Unchanged {
            dir: updated.download_dir,
        });
    }
    // ①½ **先停手，再记旧目录**（规格 2026-10-12 §6）。尽力而为：问不到就当没有——
    //    换目录本身不能因为这两步失败而失败。
    //    · 强制暂停：磁盘快满正是换目录的主场景，要立刻停写；也不让旧引擎在被掐掉之前
    //      又往旧盘写几个 GB。暂停失败要说出来（错误上屏），不然界面以为停了、引擎其实还在写；
    //    · 旧目录：偏好里「未配置」时落盘根是内核的默认值，只有内核知道它是哪儿。
    let (paused, old_dir) = match app.session.client() {
        Some(client) => {
            let paused = match kernel::task_action(lang, &client, TaskAction::PauseAll, None) {
                Ok(()) => true,
                Err(why) => {
                    note_failure(&app.session, &why);
                    false
                }
            };
            (paused, kernel::download_dir(lang, &client).ok())
        }
        None => (false, None),
    };
    if let Err(cause) = updated.save(&path) {
        let cause = cause.to_string();
        let message = if paused {
            api::preferences::save_failed_after_pause(&cause)
        } else {
            api::preferences::save_failed(&cause)
        };
        return api::preferences::change(lang, &DownloadDirChange::Failed { message });
    }
    // 待搬的旧目录先记下：批次加载好、内核在线的那一刻交接（见 `Session::take_handover`）。
    // 这一次重启失败的话，它留给之后的重试。
    app.session.defer_handover(old_dir);
    // ⚠️ 这是**用户的显式动作** ⇒ 复位自动重启预算（对齐 macOS `AppModel.swift:1244`）：
    //    否则"自动重启已经用掉预算"会让改目录变成一件按下去没有回音的事。
    app.session.reset_auto_restart();
    match restart_kernel(&app.session, ChangedSetting::DownloadDir.restart_timeout_message()) {
        Ok(()) => {
            // 新盘要重新量一次（旧盘那句不许粘在新盘上）。
            note_disk_speed(&app.session, Some(&updated.download_dir), true);
            api::preferences::change(lang, &DownloadDirChange::Changed {
                dir: updated.download_dir,
            })
        }
        // 内核起不来的**原因原文**照登（可能是"没找到内核…"，也可能是握手超时那句，
        // 也可能是"没在期限内落地"）—— 三种都是**完整的一句话**，壳不再加工。
        Err(why) => api::preferences::change(lang, &DownloadDirChange::Failed { message: why }),
    }
}

/// **详细日志开关**（规格 §2.4）。**改了要重启内核**（它与 `--download-dir` 同一条
/// argv 通路 —— 内核的 `set_settings` 里没有这一格）。
///
/// ## 四步与 [`preferences_set`] 逐条同形
///
///   ① **同值 ⇒ 什么都不做**（`Unchanged`）：勾选框正常来说只在真的变了的时候发这一条，
///      但界面是异步的 —— 一条迟到的"和当前一样"不许把用户正在跑的任务停掉；
///   ② **偏好先落盘**（失败 ⇒ 中止，内核一个都不许动；那句话说明了根因，而盘上那一份
///      一个字都没被动过）；
///   ③ **重启内核**（下载目录那段同理：档位只在 argv 上）；
///   ④ 回执由 `api::preferences` 成句，本层一个字都不自己写。
///
/// ## 🔴 中间那一步是**本命令独有**的：壳自己这一格必须**当场**跟着变
///
/// 改内核的级别要重启内核，但**壳这个进程不重启** —— 它自己那份 `diag-shell.log`
/// 的档位是一个**进程级静态**（`shell_core::diagnostics::init`），只在启动时落过一次。
/// ⇒ 少了 [`apply_shell_log_level`] 这一步，用户会看到"内核的日志变详细了、
/// **壳那一份一行都不变**"，而**没有任何东西会变红**（两个文件都还在，
/// 只是其中一个没跟着走）—— 这正是这个开关最要紧的那一半：
/// 客户报"下载引擎已断开"时，壳当时在做什么**只有这一份里才有**。
///
/// ## ⚠️ 回执用的是 [`VerboseLoggingChange`]，不是 `DownloadDirChange`
///
/// 简报让这条复用 `preferences_set` 那一套**回执形状**（"对用户来说这是同一件事：
/// 改了一项要重启内核的设置"）—— 形状确实复用了（`api::preferences` 装的是
/// **同一组四格**，前端只有一条渲染路径）。但**那三句话没有复用**：
/// `DownloadDirChange` 的三句把主词写死成"下载目录"，拿它来报一次勾选框的改动
/// 会让用户在常驻回执上读到一句**假话**。理由写在那个类型的文档里。
#[tauri::command(async)]
pub fn verbose_logging_set(app: tauri::State<'_, Shell>, on: bool) -> Value {
    let path = match preferences_path() {
        Ok(path) => path,
        Err(why) => return api::envelope::err(&why),
    };
    let current = Preferences::load(&path);
    let updated = current.setting_verbose_logging(on);

    if updated.verbose_logging == current.verbose_logging {
        return api::preferences::verbose_change(app.session.lang(), &VerboseLoggingChange::Unchanged { on });
    }
    if let Err(cause) = updated.save(&path) {
        return api::preferences::verbose_change(app.session.lang(), &VerboseLoggingChange::Failed {
            message: api::preferences::verbose_save_failed(&cause.to_string()),
        });
    }
    // 🔴 **先落壳自己这一格，再重启内核**（顺序承重）：反过来的话，内核重启那一段
    //    （几十毫秒到几秒）里壳的级别还是旧的那一档，而那一刻正是最该记全的时候
    //    （内核正在被换掉）。它与重启成不成功**无关** —— 内核起不来也不该让壳的
    //    日志继续停在旧档上（回执会把"没改成"如实说出来）。
    apply_shell_log_level(&updated);
    // ⚠️ 这是**用户的显式动作** ⇒ 复位自动重启预算（同 [`preferences_set`]）。
    app.session.reset_auto_restart();
    match restart_kernel(
        &app.session,
        ChangedSetting::VerboseLogging.restart_timeout_message(),
    ) {
        Ok(()) => api::preferences::verbose_change(app.session.lang(), &VerboseLoggingChange::Changed { on }),
        // 内核起不来的**原因原文**照登（与 `preferences_set` 同一条）。
        Err(why) => api::preferences::verbose_change(app.session.lang(), &VerboseLoggingChange::Failed { message: why }),
    }
}

/// 批次历史（规格 §3.4 第十四行）：屏上从上到下的那些行。
///
/// **对齐 macOS**：`AppModel.loadHistoryIfNeeded()`（`AppModel.swift:771`）+
/// `BatchHistoryRow.rows(_:)`。
///
/// ⚠️ 读失败/文件坏了 ⇒ **空历史**（E-1，`History::load` 的判据），不是错误 ——
///    第一次运行、文件被删、文件坏了都是它。唯一会返回失败信封的是**存放根取不到**
///    （`%APPDATA%` 缺失），那件事要说出来。
#[tauri::command]
pub fn history_get() -> Value {
    let path = match history_path() {
        Ok(path) => path,
        Err(why) => return api::envelope::err(&why),
    };
    api::history::payload(&History::load(&path))
}

/// 写一条历史：**又用了一次** / **只改一句备注**（规格 §3.4 第十四行）。
///
/// **对齐 macOS**：`AppModel.recordLoadedBatch`（`:787`）与 `setHistoryNote`（`:799`）
/// —— 那边是两个方法，我们这边是**一条命令的两个变体**（`HistoryWrite`：
/// "又用了一次"与"只改备注"在磁盘上是两件事，一个布尔开关会让"前端忘了带这一格"
/// 变成一次静默的语义降级，见那个类型的文档）。
///
/// ⚠️ **时间戳由壳给**（`BatchHistory::timestamp`，手写的 ISO8601 `+08:00`）：
///    前端没有时钟、也不许自己拼时间串（§3.2）。"又用了一次"那一支记的是**此刻**；
///    "只改备注"那一支**根本不看时间**（写备注不是"又用了一次"—— 用户刚看着的那一行
///    会从他眼皮底下跳到列表最上面）。
///
/// ⚠️ 写盘失败 ⇒ **失败信封**（`api::history::write_failed` 那句话 + 系统原文）。
///    与 macOS 的一处**有意差异**：那边写盘失败**不抛**（它在内存里留住了改动，
///    由一条常驻提示行说"历史写盘失败"）；我们这一侧的命令是**无状态**的
///    （读盘 → 改 → 写盘），写不进去就**真的没记下来** ⇒ 吞掉它才是静默失效。
///
/// ⚠️ 成功时回的是**写完之后的那份历史**（与 `history_get` 同一个载荷形状）：
///    前端不必再发一条命令去刷新（少一次往返，也少一个"界面与盘上不一致"的窗口）。
#[tauri::command]
pub fn history_put(entry: shell_core::api::history::HistoryWrite) -> Value {
    let path = match history_path() {
        Ok(path) => path,
        Err(why) => return api::envelope::err(&why),
    };
    match write_history_entry(&path, &entry) {
        Ok(updated) => api::history::payload(&updated),
        Err(cause) => api::envelope::err(&api::history::write_failed(&cause)),
    }
}

/// 写一条历史的**唯一胶水**（判据在 `api::history::apply`，落盘在 `History::save`）。
///
/// ⚠️ 三个调用点（`history_put` / `record_loaded_batch_in` / `remembered_code_in`）
///    都从这里过 —— "怎么写一条历史"不许长出第二份。
fn write_history_entry(
    path: &std::path::Path,
    entry: &shell_core::api::history::HistoryWrite,
) -> Result<History, String> {
    let history = History::load(path);
    let updated = api::history::apply(&history, entry, current_unix_seconds());
    updated.save(path).map_err(|cause| cause.to_string())?;
    Ok(updated)
}

/// 一次**成功**的加载之后把它记进批次历史（对齐 macOS `AppModel.recordLoadedBatch`）。
///
/// ⚠️ **只在成功那一支调**（`spawn_load` 的 `Ok` 臂），与内核 `remember_last_code`
///    的时机一致：失败的加载写进去，历史里就会堆满打错的码。
/// ⚠️ 记的是**这次请求用的** `base_url`（不是内核回显的 `info.base_url`）——
///    回显那份在"用户没指定"时是内核的默认交付服务器，写进历史、下次再显式传回去，
///    会在**内核默认值变化时静默分叉**（E-5 的同一条纪律）。
/// ⚠️ **调用点没有判据**（同 `remember_for_redaction` 的诚实记账）：判它要写真的
///    `%APPDATA%` 历史文件，越过本仓对测试的底线 ⇒ 被钉住的是 `_in` 那一层，
///    "调用点有没有被走到"只有真机验收能回答。与 macOS 的一处**有意差异**：
///    那边写盘失败有常驻提示行（`historyWriteFailure`），我们这一侧的记录发生在
///    **后台线程**、没有可挂的回执 ⇒ 落 `diag-shell.log`（导出可见），不打断加载、不上屏。
pub fn record_loaded_batch(code: &str, base_url: &str) {
    let outcome = match history_path() {
        Ok(path) => record_loaded_batch_in(&path, code, base_url),
        Err(why) => Err(why),
    };
    if let Err(why) = outcome {
        shell_core::diagnostics::log("history_write_failed", &[("why", why)]);
    }
}

/// [`record_loaded_batch`] 的可测内核（路径是参数，测试用临时目录）。
fn record_loaded_batch_in(
    path: &std::path::Path,
    code: &str,
    base_url: &str,
) -> Result<(), String> {
    let entry = shell_core::api::history::HistoryWrite::Used {
        code: code.to_string(),
        base_url: base_url.to_string(),
        note: None,
    };
    write_history_entry(path, &entry).map(|_| ())
}

/// 自动加载「记住的交付码」用哪个码（对齐 macOS `AppModel.rememberedCode()`，E-4）。
///
/// **权威只有一个：**
/// - 壳的历史非空 ⇒ 用它最近使用的那条（`base_url` 一起带回去）；内核的 `last_code`
///   **一个字都不读** —— 要防的分裂是"界面显示 A（壳的历史）、内核却自动加载 B"。
/// - 壳的历史为空（第一次升级、用户删了文件）⇒ 读内核的 `last_code` 作为**一次性种子**，
///   写进历史，之后不再读它（"先落下再加载"——这样"之后壳不再读它"才是一条真的承诺）。
/// - 两个来源都没有 ⇒ `None`："没有任何码可加载 ⇒ 一个请求都不发"。
///
/// ⚠️ **调用点没有判据**（同 [`record_loaded_batch`]）：真历史在 `%APPDATA%`。
///    ⚠️ 内核那个值可能带结尾换行（macOS 逐字注明）：trim 之后再当历史的主键用 ——
///    带空白的主键会让"同一个码"看起来是两条。
pub fn remembered_code(kernel_last_code: &str) -> Option<(String, String)> {
    let path = history_path().ok()?;
    remembered_code_in(&path, kernel_last_code)
}

/// [`remembered_code`] 的可测内核（路径是参数，测试用临时目录）。
fn remembered_code_in(
    path: &std::path::Path,
    kernel_last_code: &str,
) -> Option<(String, String)> {
    let history = History::load(path);
    if let Some(newest) = history.most_recent() {
        let base_url = newest.base_url_or_nil().unwrap_or("").to_string();
        return Some((newest.code.clone(), base_url));
    }
    let seed = kernel_last_code.trim();
    if seed.is_empty() {
        return None;
    }
    // 播种（先落下再加载）：写失败不改结论 —— 那是内核记着的那个码（不是用户打错的），
    // 下一次启动还会再播一次；macOS 的 `writeHistory` 同样不抛。
    let entry = shell_core::api::history::HistoryWrite::Used {
        code: seed.to_string(),
        base_url: String::new(),
        note: None,
    };
    if let Err(why) = write_history_entry(path, &entry) {
        shell_core::diagnostics::log("history_write_failed", &[("why", why)]);
    }
    Some((seed.to_string(), String::new()))
}

/// 两份许可全文（规格 §3.4 第十五行）。
///
/// ⚠️ **第二代就内嵌了这两份全文，但没有任何读取者**（W-5 里"随附"那一半成立、
///    "能看到"那一半一直没做）—— 本条命令就是那另一半的落点（`licenses.rs` 的文件头
///    记着这一笔账）。载荷的映射在 `licenses::wire()`（那里的四格有两格是中文文案，
///    而命令层一个字都不许自己写）。
///
/// ⚠️ **它不碰会话、不发内核请求**：许可全文是**编译期**编进这个 exe 的常量。
#[tauri::command]
pub fn license() -> Value {
    api::envelope::ok(crate::licenses::wire())
}

/// 关于窗口（规格 §3.4 第十六行）：今天的全部内容就是**版本号**。
///
/// **对齐 macOS**：`AboutView` + `AboutInfo.version(in:)`（`Presentation/AboutInfo.swift`）。
/// ⚠️ 回落规则（短版本 → 构建号 → 兜底；**绝不返回空串**）在 `presentation/about_info`，
///    两个来源由**这里**在编译期取好（build script 设的环境变量只对它自己那个 crate
///    可见，所以 `shell-core` 拿不到它们 —— R-7 的裁决）。
///
/// ⚠️ `option_env!` 读的是**编译那一刻的环境**（`BENAGEN_VERSION=0.2.0 bash build_windows.sh`
///    那种用法照样生效），而 `build.rs` 的 `rerun-if-env-changed` 盯的是同一个名字
///    —— 两处同源。未设时回落 `CARGO_PKG_VERSION`（本 crate 的 `Cargo.toml` 版本）。
#[tauri::command]
pub fn about() -> Value {
    api::about::about(&AboutInfo::version(
        option_env!("BENAGEN_VERSION"),
        Some(env!("CARGO_PKG_VERSION")),
    ))
}

// ===========================================================================
// 上面的命令共用的几件东西（都在本文件里，因为它们都是"装配"而不是判据）
// ===========================================================================

/// **这一次改的是哪一格** —— "改了要重启内核的设置"有两个，各自的话不一样。
///
/// ## 🔴 它存在的理由：那张"哪一格 → 哪句话"的表以前是**按位置**传的
///
/// `restart_kernel(session, on_timeout)` 收一句 `&'static str`，两个调用点各传一个字面量。
/// 那种写法**判不了**：把两句对调，用户改了下载目录却读到"详细日志开关已经记下来了"
/// （而那个目录改动**确实**记下来了 —— 是一句假话），
/// 而**所有测试照旧全绿**（`api::preferences` 那条钉的是两个常量**本身**，
/// 不是"哪个调用方用哪一个"）。
///
/// ⇒ 把那个**选择**收进这张表：[`Self::restart_timeout_message`] 是个**纯函数**，
///    在 `mod tests` 里有判据；两个调用点只传自己的那一类（`ChangedSetting::DownloadDir` /
///    `ChangedSetting::VerboseLogging`），**不再传字符串**。
///
/// ⚠️ **残余（如实记账）**：把两个调用点上的 enum **写反**仍然没有自动判据 ——
///    那两条命令的签名要 `tauri::State<'_, Shell>`，测试造不出来
///    （与 `enqueue` 当初要拆出 `enqueue_with` 是同一道缝）。但那一次写反在代码上是
///    **一眼可见**的（枚举名与函数名在同一屏），而原来那种"两个字符串按位置传"
///    连读都读不出来。⇒ 记在这里，并挂到真机验收那两条旁边（见任务报告）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangedSetting {
    /// 下载目录（[`preferences_set`]）。
    DownloadDir,
    /// 详细日志开关（[`verbose_logging_set`]）。
    VerboseLogging,
}

impl ChangedSetting {
    /// **内核重启没在期限内落地**时该说的那句话（那一格里"已经记下来了"说的是**用户刚改的
    /// 那一格** —— 所以这句话只能按"改的是哪一格"选，不能由内核那一侧代劳）。
    ///
    /// ⚠️ 两个取值**必须**各回各的那一句（`api::preferences` 的两条同构常量）：
    ///    这一格的判据在 `mod tests` 的 `each_setting_gets_its_own_restart_timeout_message`。
    pub fn restart_timeout_message(self) -> &'static str {
        match self {
            ChangedSetting::DownloadDir => api::preferences::restart_timed_out(),
            ChangedSetting::VerboseLogging => api::preferences::verbose_restart_timed_out(),
        }
    }
}

/// 壳的**偏好文件**在哪。
///
/// ⚠️ **这是唯一一处把「存放根」与「偏好文件名」拼起来的地方**：存放根怎么算在
///    `shell_core::storage::dir()`（与内核 `paths.rs` 同源），文件名在
///    `storage::preferences::FILE_NAME`。拼第二遍就是第二个真相源。
fn preferences_path() -> Result<PathBuf, String> {
    Ok(shell_core::storage::dir()?.join(shell_core::storage::preferences::FILE_NAME))
}

/// 壳的**历史文件**在哪（同 [`preferences_path`]，另一个文件名）。
fn history_path() -> Result<PathBuf, String> {
    Ok(shell_core::storage::dir()?.join(shell_core::storage::history::FILE_NAME))
}

/// 读一份**当前生效**的偏好。
///
/// ⚠️ **它是 `pub` 的，因为 bin（`main.rs`）是另一个 crate**：起内核之前要按它决定
///    `--download-dir`（那是下载目录**唯一**能生效的通道）。⚠️ **每次起内核都重读一遍**
///    （不缓存）：`retry()` 的语义是"重新找一次内核"，而用户可能刚在设置里改过目录
///    —— 缓存会让那一次改动对第二次之后的重试失效。
///
/// ⚠️ 取不到存放根 ⇒ `Err`（那句话点名了缺哪个变量、怎么补救）。**不许**吞成"未配置"：
///    两者在磁盘上看起来一样（都是不传 `--download-dir`），但一个是我们不知道、一个是
///    用户没设 —— 而前者会让"用户的目录设置去哪了"变成一次无从归因的静默失效。
pub fn load_preferences() -> Result<Preferences, String> {
    Ok(Preferences::load(&preferences_path()?))
}

/// 量一次目标盘的写入速度，把"盘比网慢"那句摆进会话。
///
/// 判据（多慢算慢、那句话怎么说）**全在 `shell_core::presentation::disk_notice`**；
/// 这里只负责"什么时候量、量哪个目录"：
///   * **真的开始下载时**（`enqueue` 成功）—— 客户此刻正盯着速度，提示才有地方落；
///   * **换下载目录之后**（`fresh = true`）—— 新盘要重新量，旧结论不许粘在新盘上。
///
/// ⚠️ `cached` ⇒ 同一个目录一次运行只写一次盘（探针 8 MiB，写完即删）。
/// ⚠️ 量不出来（目录还没建 / 盘被拔掉 / 只读）⇒ **不提示**：宁可不说，也不许编数字。
fn note_disk_speed(session: &Arc<Session>, dir: Option<&str>, fresh: bool) {
    let Some(dir) = dir.map(str::trim).filter(|d| !d.is_empty()) else {
        return;
    };
    let path = std::path::Path::new(dir);
    let mbps = if fresh {
        diskspeed::fresh(path)
    } else {
        diskspeed::cached(path)
    };
    session.set_disk(DiskNotice::of(mbps));
}

/// **把偏好里那一格落到壳自己这个进程的日志档位上**（规格 §2.3 / §2.4）。
///
/// 🔴 **两处调用点共用这一份**（它们时机不同，缺一个就是一个方向不工作）：
///   * **启动时**（`main.rs`，起 Tauri 之前、读完偏好之后）—— 少了它，客户把开关
///     打开再重启应用，`diag-shell.log` **一行都不会出现**；
///   * **开关被改时**（[`verbose_logging_set`]）—— 改内核的级别要重启内核，但
///     **壳自己这个进程不重启**，所以它那一格必须当场跟着变。少了这一步的表现是
///     "打开开关之后内核的日志变详细了、**壳那一份一行都不变**"，而**没有任何东西
///     会变红**（两个文件都还在，只是其中一个没跟着走）。
///
/// ⚠️ **它是 `pub` 的，因为 bin（`main.rs`）是另一个 crate**（同 [`load_preferences`]）。
/// ⚠️ **没有"读不到偏好"这一支**：调用方拿到的是一份已经读出来的偏好
///    （读不到存放根时的处置在 [`load_preferences`] 那里 —— 起内核那条路把它当
///    "未配置"，而壳自己的日志档位跟着落到 `Normal`，那是安全的默认值）。
pub fn apply_shell_log_level(preferences: &Preferences) {
    shell_core::diagnostics::init(shell_level(preferences));
}

/// 偏好里那一格 ⇒ **壳自己**日志的档位。**纯函数**：不碰进程静态、不写文件、不读时钟。
///
/// 🔴 **为什么把它从 [`apply_shell_log_level`] 里拆出来**（2026-10-06 终审）：
///    原来那一整句就在 `init(...)` 的实参位置上，而 `init` 落的是**进程级静态** ——
///    要判它就得让测试去翻全局（同进程里别的用例会跟着随机红）或者往真日志目录里写。
///    于是**那一格一条判据都没有**：把 `if/else` 两支**对调**，全套判据照旧全绿。
///    真机上那是本功能**最坏的那个形状** —— 用户勾上「详细日志」，**内核**那一档
///    确实变详细了（那条路有判据：`core_arguments` 的拼装），而**壳自己那份日志
///    停在 normal**，于是 `diag-shell.log` **根本不会被创建**，客户回传的仍然
///    只有内核那一份。拆成纯函数之后，两个方向都钉得住。
///
/// ⚠️ **拆出来之后仍然没有判据的那一半**：`init(...)` 那个**调用点本身**
///    （"这一句有没有被走到"）—— 要判它就得动上面那两样中的一样。它与 macOS 的
///    `DiagnosticsLog.configure(...)` 是同一个缺口，只由真机验收那两条守着。
pub fn shell_level(preferences: &Preferences) -> shell_core::diagnostics::Level {
    if preferences.verbose_logging {
        shell_core::diagnostics::Level::Verbose
    } else {
        shell_core::diagnostics::Level::Normal
    }
}

/// 此刻的 Unix 秒。**本 crate 唯一一处读时钟**（时间戳怎么格式化在
/// `BatchHistory::timestamp` 里，那边是纯函数、不读时钟 —— 同一条分工）。
///
/// ⚠️ 时钟早于 1970 时回 `0`（`unwrap_or`）：那种机器上本来就没有正确的"此刻"，
///    而这一格**只是排序键**（`History` 的排序用的是解析出来的值，解析不出来就排到最末）
///    —— 它不会让任何东西崩，也不会让哪一条记录消失。
fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// **重启内核**：收掉旧连接 → 清掉加载态 → 重新找内核 + 握手 → **有界地等它落地**。
///
/// ## ⚠️ 为什么必须重启（而不是"跟内核说一声换个目录"）
///
/// 下载目录**只在 argv 上**（`--download-dir`）：内核的 `set_settings` **没有**这个键
/// （`presentation::app_preferences` 的头注记着这条），所以"换目录"这件事只能靠
/// **换一个内核进程**。
///
/// ## ⚠️ 四步各自的理由（顺序不能换）
///
///   ① **旧连接先取走**：`take_client()` 只做 `take`（**不在锁里析构**）——
///      `CoreClient::drop` 就是 `shutdown()`（最坏 3 + 2 秒），留在临界区里会把
///      `state()`、`tree()`、`load()` 一起挡住那么久（`session.rs` 的头注记着这条纪律）。
///   ② **旧连接在后台丢**：它要花几秒，而这条命令线程还要接着干活（起新内核）。
///      ⚠️ 起不了线程时那个闭包会被 `std` **就地**析构 ⇒ 旧连接在这条线程上被收掉
///      （阻塞几秒）。这比"漏掉收尾"好得多：漏掉它留下的是一个**孤儿内核进程**。
///   ③ **加载态与"当前批次"都不动**：新内核起来之后 `spawn_connect` 会**自动重放**
///      那一批（`load_delivery`，控制在 `spawn_load` 那条既有路径上）——
///      **与 macOS 的 `restartKernel` → `performLoadDelivery` 逐字同一条**：
///      用户回到文件页**还是原来那批**，而不是"换完目录批次没了"。
///      ⚠️ 重放用的是**请求时那个 `base_url`**（`Session::load_request` 那一格），
///      **不是**内核回显的那份 —— 后者在"用户没指定"时是内核的默认服务器，
///      拿它当用户的选择再发回去会在内核默认值变化时**静默分叉**（E-5 明禁）。
///      ⚠️ 重放失败**不静默**：它落在 `load` 那一格（空态页显示内核原文 + 「重试」），
///      与 `load()` 命令走同一套回执。
///      ⚠️ 还没有成功加载过任何一批时（首次启动那趟连接）**天然不重放**：
///      判据是 `Session::load_request` 的 `None`，这里不需要特判。
///   ④ **等它落地（有界）**：见 [`wait_for_the_kernel`]。不等就回"已重启"是一句
///      **我们不知道真假**的话（新内核可能根本起不来）。
///
/// ⚠️ 它**不写偏好**（那是调用方在第 ② 步之前做的）：本函数只负责"把内核换成一个
///    按**当前** argv 起的新进程"，而 argv 是连接器在起内核那一刻现读的（`main.rs`）。
///
/// ## ⚠️ `on_timeout`：**"等超时了该说什么"由调用方给**
///
/// 那一刻的结论分两种（见 [`wait_for_the_kernel`]）：内核自己说了原因（`Unavailable`，
/// 照登），或者**到点还停在 `Connecting`**（那句话由**调用方**给）。后者的原文里有一句
/// "**已经记下来了**"——而**记下来的是哪一格只有调用方知道**（下载目录 / 详细日志开关）。
/// 合成一句的结局是其中一半场合对着用户说错话（"下载目录已经记下来了"，
/// 而他改的是一个勾选框、一个目录都没动）。
///
/// ⚠️ 调用方**不许**直接传字符串字面量：那一句要经 [`ChangedSetting::restart_timeout_message`]
///    选出来（那张"哪一格 → 哪句话"的表有单测，见 [`ChangedSetting`]）。
fn restart_kernel(session: &Arc<Session>, on_timeout: &str) -> Result<(), String> {
    if let Some(retired) = session.take_client() {
        let _ = std::thread::Builder::new()
            .name("core-retire".to_string())
            .spawn(move || drop(retired));
    }
    // ⚠️ **这里没有"把加载态清掉"那一行**（任务 8 的第一版有，控制者裁决后删掉）：
    //    新内核一连上，`spawn_connect` 就会按 `Session::load_request` **重放**那一批，
    //    而重放自己要写 `Loading`/`Loaded`/`Failed`（与 `load()` 命令同一套回执）。
    //    在这里先清成 `Idle` 会让界面在"重启 → 重放"那一小段里闪一下空态页 ——
    //    而 macOS 的 `restartKernel` 从头到尾都没让那一批消失过。
    // ⚠️ 引擎那一格回到「正在连接内核…」：这一刻手上**没有**内核（旧的那个正在被收掉）。
    //    少了这一行，界面会在新内核起来之前一直显示"引擎未启动"（一个已经不成立的结论）。
    session.set_engine(EngineState::Connecting);
    // 与启动、与 `retry()` **同一条路**（不重写一遍连接逻辑）：连接器每次都真的重查
    // 一遍内核在哪（那是"把内核放到壳旁边再点重试"这条补救的唯一实现）。
    session::spawn_connect(session);
    wait_for_the_kernel(session, on_timeout)
}

/// 等这一次重启**落地**（有界）。判据是**引擎那一格** —— 连接线程会把它写成
/// `NotStarted`（成功）或 `Unavailable(原因)`（失败），两者都是**结论**。
///
/// ⚠️ **为什么用轮询而不是通道**：`session::spawn_connect` 起的那条线程**没有**回执
///    通道（它是"起了就走"的形状，`retry()` 与启动那两处都不等它）。为了这一次等待
///    给它加一个通道，就得同时改那两处的调用形状 —— 而**引擎那一格本来就写着答案**
///    （连接成功/失败是它的全部内容）。轮询同一个判据比多一条通道更少的活动部件。
///    ⚠️ macOS 的 `waitForTheRestartInFlightToFinish` 是同一个形状（每 10 ms 看一次）。
///
/// ⚠️ 等待期间**不阻塞前端**：`state()` 那一秒一问跑在别的命令线程上，引擎徽标会一路
///    显示"正在连接内核…"。这里等的只是**这一次调用**的结论。
///
/// ⚠️ `on_timeout` 由调用方给（理由见 [`restart_kernel`] 的同名参数）：本函数只看得到
///    **内核**的状态，而那句话里"已经记下来了"说的是**用户刚改的那一格**。
fn wait_for_the_kernel(session: &Session, on_timeout: &str) -> Result<(), String> {
    /// 两次查看之间的间隔（10 ms：够密 —— 正常重启也就几十毫秒；又不至于空转）。
    const POLL: Duration = Duration::from_millis(10);

    let started = Instant::now();
    loop {
        match session.view().engine {
            // 新内核起来了（`NotStarted` 是连接线程的结论）—— 或者已经跑起来了。
            EngineState::NotStarted | EngineState::Running => return Ok(()),
            // 起不来：那句话是**完整的一句人话**（"没找到内核：…" / 握手超时那句），
            // 原样交给调用方去当回执（`DownloadDirChange::Failed` 的契约是"照登"）。
            EngineState::Unavailable(why) => return Err(why),
            EngineState::Connecting => {
                if started.elapsed() >= api::preferences::RESTART_DEADLINE {
                    // 到点了还是"正在连接"：继续等下去**不会再有新结论**（每一段都有上界，
                    // 说明某一段的上界破了）⇒ 把"这一次没改成"如实说出来。
                    // ⚠️ 那句话**由调用方给**（它才知道记下来的是哪一格，见函数文档）。
                    return Err(on_timeout.to_string());
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

/// 一条失败的**副作用那一半**（纯的那一半是 `api::failure`）。
///
/// ⚠️ **两件事必须一起做**（上游 `absorbing`：吸收 + 重抛）：翻引擎那一格（内核真的没了）
///    / 落 `last_error`（其余错误），**且**把"你刚才那一下没成"交回前端。
///    少了副作用那半，横幅永远不翻面（用户以为内核还活着）；少了 `api::failure` 那半，
///    用户看不到这一次动作的结果。**两条路都要走**，所以本函数不做别的。
///
/// ⚠️ **判据只有 `KernelDeath` 那一份**（在 `shell-core` 里，纯函数、有单测）：
///    本函数只做"认出来并交回去"，**不在这里判**"什么算内核死亡"。
///    `note_kernel_death` 自己保证"至多一次"（先到的那句原因就是事实）。
fn note_failure(session: &Arc<Session>, why: &CallFailure) {
    match why {
        CallFailure::KernelGone { reason, .. } => {
            if session.note_kernel_death(reason.clone()) && session.claim_auto_restart() {
                // 第一次死亡、且这一轮还没自动重启过 ⇒ 给它一次。
                session::spawn_connect(session);
            }
        }
        // 🔴 **内核亲口说"这一批没了"** ⇒ **作废当前批次**（清掉 `load_request`，
        //    对齐 macOS 的 `resetToEmptyState()`）—— 否则那个死码会在下一次内核重启时
        //    被重放（审查 I-1）。
        //    ⚠️ **不写 `last_error`**：macOS 的 `absorb` 在这一档是 `.absorbed`
        //    （它是正常态，不是错误，那句话不上屏）。而这一次动作的返回值仍然是一个
        //    失败信封（`api::failure`）—— 那一条路今天就是这样，本任务不动它
        //    （报告里如实记了"absorb 与显示"这一处与 macOS 的差异）。
        CallFailure::NoDelivery(_) => session.reset_to_empty_state(),
        // 内核亲口说"引擎还没起来" ⇒ 引擎那一格跟着它翻（对齐 macOS 的同一条处置）。
        // ⚠️ 同样**不写 `last_error`**：那是正常态（macOS 的 `.absorbed`），不上屏。
        CallFailure::EngineNotStarted(_) => session.note_engine_not_started(),
        // ⚠️ 这一格是**非粘滞**的（下一个成功请求就清）：这里只写，不清。
        CallFailure::Text(message) => session.set_last_error(Some(message.clone())),
        // 内核给了分型（规格 W2）：文字照旧进 `last_error`，**另外**记一张卡片
        // （同类 30 秒内只加计数、按严重度置顶，句子由 `error_card` 按语言成句）。
        CallFailure::Classified { text, class } => {
            session.set_last_error(Some(text.clone()));
            session.note_error_class(class);
        }
    }
}

#[cfg(test)]
mod tests {
    //! 命令层的用例。**只覆盖 `enqueue` 与 `tree`（无 `path` 那一支）这两条**
    //! （任务 16 与任务 17 的落点）：其余命令的判据都在 `shell-core` 的纯函数里，
    //! 那一边有各自的用例。
    //!
    //! ⚠️ **这两条为什么必须住在命令层**：它们守的是**接线**本身 ——
    //!    "前端发来的勾选面**不是**直接发给内核的那一串"，以及"那个勾选面**此刻**
    //!    能不能发出去"。判据（`DownloadTargets::{paths, blocked_reason}`）早有单测，
    //!    缺的是"生产调用点真的调了它、喂进去的是**哪一份数据**"。
    //!
    //! ⚠️ **为什么这一条必须住在命令层**：本任务要守的是**接线**本身 ——
    //!    "前端发来的勾选面**不是**直接发给内核的那一串"。判据（`DownloadTargets::paths`）
    //!    早有单测，缺的是"生产调用点真的调了它、且喂进去的是**会话里那一批的全集**"。
    //!    所以下面每一条都断言**真正写出去的那一行**（`Recording` 通道记下来的），
    //!    而不是某个中间变量的形状 —— 中间变量的形状证明不了接线。
    //!
    //! ⚠️ **Tauri 那一层（`tauri::State<'_, Shell>`）在这里是够不到的**：
    //!    它只有 Tauri 运行时造得出来。所以命令体被拆成 [`super::enqueue_with`]
    //!    （收 `&Session` 与裸 `paths`），本模块直接调它 —— 与 `kernel.rs` /
    //!    `session.rs` 里那些用例是同一个形状。

    use super::*;
    use shell_core::api::history::HistoryWrite;
    use shell_core::client::{ClientError, CoreClient, LineChannel};
    use shell_core::protocol::{
        DeliveryInfo, FileNode, FileState, FlatEntry, Progress, TreeResult, TreeNode,
    };
    use std::sync::{Arc, Mutex};

    /// 一条**记账用**的内核通道：把写出去的每一行记下来，回一条固定的成功回执。
    ///
    /// ⚠️ 它喂的是**真 `CoreClient`**（`LineChannel` 是 `shell-core` 的公开 trait）——
    ///    于是断言的对象是**线上那一行的正文**，判据、编码、长度守卫全都真的过了一遍。
    #[derive(Clone)]
    struct Recording {
        written: Arc<Mutex<Vec<String>>>,
    }

    impl Recording {
        fn new() -> Recording {
            Recording {
                written: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// 最后一条写出去的请求（没有就是空串 —— 断言里那句 `{sent}` 会把它印出来）。
        fn last_request(&self) -> String {
            self.written.lock().unwrap().last().cloned().unwrap_or_default()
        }

        /// 全部写出去的请求（按顺序）。
        fn all_requests(&self) -> Vec<String> {
            self.written.lock().unwrap().clone()
        }

        /// 已经写给内核的请求**条数**（"一条都没有"是任务 17 那条兜底闸要钉的事）。
        fn written_len(&self) -> usize {
            self.written.lock().unwrap().len()
        }
    }

    impl LineChannel for Recording {
        fn write_line(&self, line: &str) -> Result<(), ClientError> {
            self.written.lock().unwrap().push(line.to_string());
            Ok(())
        }

        /// 一条成功的 `enqueue` 回执：**一件都没加、也没有拒绝**（`EnqueueFeedback::of`
        /// 的文档里那条 D-2 形状 —— 内核真会发它，不是死形状）。
        /// ⚠️ `id` 必须是这一条请求的 id（`CoreClient` 从 1 起编号，首条就是 1）：
        ///    对不上会被判成 `Desync`，那是"把一条无关的 result 当成自己的"。
        fn read_line(&self) -> Result<Option<String>, ClientError> {
            // id = 到目前为止写出去的请求条数（`CoreClient` 从 1 起编号，逐条递增）：
            // 大批量会被切成多块依次发送，每一块的回执都要对得上自己的 id。
            let id = self.written.lock().unwrap().len();
            Ok(Some(format!(
                r#"{{"id":{id},"ok":true,"result":{{"added":[],"rejected":[]}}}}"#
            )))
        }

        fn close(&self) {}
    }

    /// 一个**装上了内核连接**的会话 + 那条通道的记账句柄。
    fn a_session_with(load: LoadState) -> (Arc<Session>, Recording) {
        let channel = Recording::new();
        let session = Arc::new(Session::new());
        session.install_client(
            Arc::new(CoreClient::new(Box::new(channel.clone()))),
            r#"{"protocol":2}"#.to_string(),
        );
        session.set_load(load);
        (session, channel)
    }

    /// 一个文件叶节点（`FileNode` 的每个键都给 —— 那也是内核真发的那些键）。
    fn a_file(path: &str) -> TreeNode {
        TreeNode::File(FileNode {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            crc64: String::new(),
            size: 1,
            completed: 0,
            total: 1,
            speed: 0,
            state: FileState::Pending,
            err: String::new(),
            source_mtime: None,
        })
    }

    /// 一批装着这些文件的交付清单（`load_delivery` 那个回执）。
    ///
    /// ⚠️ 树**故意做成扁平的一层**：这里要验的是命令层怎么用那棵树（取不取得到全集），
    ///    树的递归形状由 `shell-core` 的 `the_two_receipts_of_one_manifest_name_the_same_files`
    ///    钉着（那边有嵌套目录、`×` 与空格）。
    fn a_batch_of(paths: &[String]) -> DeliveryInfo {
        let children: std::collections::BTreeMap<String, TreeNode> = paths
            .iter()
            .map(|path| (path.rsplit('/').next().unwrap_or(path).to_string(), a_file(path)))
            .collect();
        DeliveryInfo {
            code: "C24-8".to_string(),
            page_url: "https://d.example/C24-8/index.html".to_string(),
            base_url: "https://d.example".to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            expired: false,
            total_files: paths.len() as i64,
            total_bytes: paths.len() as i64,
            tree: TreeNode::Dir {
                name: String::new(),
                children,
            },
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// 一批三个文件的**生效中**批次。
    ///
    /// ⚠️ 它**不需要**知道交付码：判据（`DownloadTargets::paths`）只看两个**集合**，
    ///    而"这一批是不是当前这一批"由 `LoadState::Loaded(info)` 这个**变体本身**回答
    ///    （见 [`super::kernel_paths`] 的文档）—— 这里没有"拿码比一比"那一层，
    ///    于是也没有"码相同但清单换了"那种分叉。
    fn a_batch() -> LoadState {
        LoadState::Loaded(a_batch_of(&v(&["a.bin", "b.bin", "c.bin"])))
    }

    // -----------------------------------------------------------------------
    // 任务 3：「哪一格 → 哪句超时话」那张表
    // -----------------------------------------------------------------------

    /// 🔴 **每一格拿到的是**它自己**那句话**（把两个臂对调 ⇒ 这一条红）。
    ///
    /// 判别力：`ChangedSetting::restart_timeout_message` 的两个分支一互换 ⇒
    /// 用户改了**下载目录**、收到的却是"**详细日志开关**已经记下来了" ——
    /// 而那句话是一句假话（那个目录改动确实记下来了）。在此之前两个调用点
    /// **按位置**各传一个字符串字面量，这一条缝隙**一条判据都没有**。
    ///
    /// ⚠️ 断言分两半，**两半都要**：
    ///   · 前半（`assert_eq!` 对常量）钉"这一格用的就是那句现成的话"（有人另编一句 ⇒ 红）；
    ///   · 后半（`contains` / `!contains`）钉**归哪一格** —— 只对常量的话，
    ///     把两个臂对调会是**恒真**的（`f() == f()` 那种写法）。
    #[test]
    fn each_setting_gets_its_own_restart_timeout_message() {
        let dir = ChangedSetting::DownloadDir.restart_timeout_message();
        let verbose = ChangedSetting::VerboseLogging.restart_timeout_message();

        assert_eq!(dir, api::preferences::restart_timed_out(), "下载目录那一格用错句子了");
        assert_eq!(
            verbose,
            api::preferences::verbose_restart_timed_out(),
            "详细日志那一格用错句子了"
        );

        assert!(dir.contains("下载目录已经记下来了"), "{dir}");
        assert!(!dir.contains("详细日志"), "改目录不许提详细日志：{dir}");
        assert!(verbose.contains("详细日志开关已经记下来了"), "{verbose}");
        assert!(!verbose.contains("下载目录"), "改开关不许提下载目录：{verbose}");
        assert_ne!(dir, verbose, "两格的话不是同一句（同一句 = 其中一半场合在说假话）");
    }

    /// 🔴 **勾选面 == 全集 ⇒ 交给内核的 `paths` 是空数组**（C-3，任务 16 的全部理由）。
    ///
    /// 判别力（**突变实测过**：把 `enqueue_with` 里那一行 [`super::kernel_paths`] 换回
    /// `paths` 原样透传 ⇒ 这一条红）—— 而真机上它是**一次报错**：
    /// 几千条路径的 `enqueue` 请求体会被 `CoreClient::call` 的长度守卫在写出去之前拒掉
    /// （`MAX_REQUEST_LINE_BYTES`，8 MiB），用户看到一句"请求行 … 字节…"的失败。
    /// 批次一大，那条路就必然踩到。
    ///
    /// ⚠️ 空数组**不是**"什么都没发"：内核的 `paths` 为空 = **全部待下载**，
    ///    与"整批全选"是同一件事（这正是判据允许这么做的理由）。
    #[test]
    fn the_whole_batch_goes_out_as_an_empty_list() {
        let (session, channel) = a_session_with(a_batch());

        let value = enqueue_with(&session, v(&["a.bin", "b.bin", "c.bin"]));

        assert_eq!(value["ok"], serde_json::json!(true), "{value}");
        let sent = channel.last_request();
        assert!(
            sent.contains(r#""params":{"paths":[]}"#),
            "勾选面覆盖整批 ⇒ 交给内核的必须是空数组：{sent}"
        );
    }

    /// **勾选面是全集的一个真子集 ⇒ 逐条发出去**（不许"看起来差不多就发 `[]`"）。
    ///
    /// 判别力：把判据写成"勾选面够大 / 勾了就好"（或干脆恒发 `[]`）⇒ 这一条红 ——
    /// 而真机上那是**用户点了两项、却下了整批**（一个不会报错、只会多下东西的错）。
    #[test]
    fn a_real_subset_goes_out_one_by_one() {
        let (session, channel) = a_session_with(a_batch());

        enqueue_with(&session, v(&["a.bin", "b.bin"]));

        let sent = channel.last_request();
        assert!(
            sent.contains(r#""paths":["a.bin","b.bin"]"#),
            "少一项就必须逐条发（保守的那一边）：{sent}"
        );
    }

    /// ⚠️ **还没有生效批次 ⇒ 只能逐条发**（"壳不知道这一批有什么"不是"全选"）。
    ///
    /// 判别力：把三档收成一个"空集 ⇒ 发 `[]`"的写法 ⇒ 这一条红 —— 而真机上那是
    /// **一次替用户做的、他没做过的选择**（内核把空的 `paths` 读成"下全部待下载"）。
    /// 三档各走一条线（`Idle` / `Loading` / `Failed`）：少任何一条，把某一档错当成
    /// "全选"都还是绿的。
    #[test]
    fn without_a_loaded_batch_the_selection_goes_out_verbatim() {
        for load in [
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Failed("这一批没了".to_string()),
        ] {
            let (session, channel) = a_session_with(load);
            enqueue_with(&session, v(&["x.bin"]));
            let sent = channel.last_request();
            assert!(
                sent.contains(r#""paths":["x.bin"]"#),
                "没有生效批次时不许把勾选面读成「整批」：{sent}"
            );
        }
    }

    /// 🔴 **大批次不会把请求撑爆**（任务 16 存在的理由 —— 没有它这条判据还是没人守）。
    ///
    /// 这条与 [`the_whole_batch_goes_out_as_an_empty_list`] 的差别在**量级**，而量级正是
    /// 这件事的全部内容：它先证明**夹具是真的会把请求撑爆**（这一串路径单独发出去
    /// 约 9.8 MB，`>` 内核那 8 MiB 行上限的一半预算、也 `>` 壳写出去之前那道硬闸），
    /// 再证明**真的发出去的那一行只有几十字节**。
    /// 两条合起来才说明"整批全选发 `[]`"不是省一点点，而是把一条**发不出去的请求**
    /// 变成一条**恒定**的短请求（不随批次大小增长）。
    ///
    /// ⚠️ 判别力（**突变实测过**）：把 [`super::kernel_paths`] 换回原样透传 ⇒ 这一条红，
    ///    而且红在**请求根本没写出去**上 —— 20 万条路径的请求体约 9.8 MB，
    ///    `CoreClient::call` 的长度守卫（`MAX_REQUEST_LINE_BYTES`，8 MiB）**在写之前**
    ///    就把它拒了（`ClientError::RequestTooLong`），用户看到的是那一发 `enqueue` **报错**。
    ///    ⚠️ **这个触发器（超大批次）碰到的是这条可读的报错**（结局由**壳自己那道闸**
    ///    给），**不是挂死**：请求在写出去之前就被拒了，内核根本看不到这一行。
    ///
    ///    🔴 **"内核回 `id == 0` ⇒ 那条 `invoke` 永远不 resolve"曾经是另一件事、曾经敞着**
    ///    —— 它说的是闸的**另一侧**（内核按 `take(8 MiB)` 读不到整行、于是回一条对不上
    ///    任何请求 id 的协议告警），在 A5 之前 `CoreClient::call` 没有上界 ⇒ 任何一条
    ///    走到那条路上的请求都会让对应的 `invoke` 永久挂住。本用例**钉不到它**（这个触发器
    ///    在写出之前就被拦了，够不着内核），它归**真机验收清单**：账见
    ///    `.superpowers/sdd/2026-09-20-windows-tauri-client/task-16-report.md` §7①。
    ///    ✅ **那条账已由 A5 关闭**（`client.rs` 的看门狗：一次调用超过**它那一档**的上界
    ///    ⇒ 判死 ⇒
    ///    走「内核没了」路径，横幅 + 「重试」）。⚠️ 本用例仍然钉不到它（够不着内核），
    ///    所以这里记的是**判据的边界**，不是判据本身。
    ///    发 `[]` 之后请求体恒定，**报错**那条路也够不着了（本用例钉的就是这件事）。
    #[test]
    fn a_huge_batch_does_not_blow_up_the_request() {
        /// 条数**要够多**：路径约 49 字节（含 JSON 的引号与逗号），20 万条约 9.8 MB ——
        /// **超过壳写出去之前那道 8 MiB 硬闸**。少一个数量级的话，这条用例会退化成
        /// "一条本来就合法的请求也合法"（恒真）。
        const HUGE: usize = 200_000;

        let paths: Vec<String> = (0..HUGE)
            .map(|i| format!("dir/file-with-a-fairly-long-name-{i}.bin"))
            .collect();

        // ⚠️ 先钉**夹具的判别力**：这一串勾选面单独发出去必须是超预算的。
        //    夹具搭小了的话，下面那条"没有撑爆"的断言就不再证明任何事。
        assert!(
            DownloadTargets::exceeds_request_budget(&paths),
            "夹具没搭准：这一批的勾选面必须真的超预算，否则这条用例证明不了任何事"
        );

        let (session, channel) = a_session_with(LoadState::Loaded(a_batch_of(&paths)));
        let value = enqueue_with(&session, paths.clone());

        assert_eq!(value["ok"], serde_json::json!(true), "这一发必须成：{value}");
        let sent = channel.last_request();
        assert!(
            sent.contains(r#""params":{"paths":[]}"#),
            "整批全选 ⇒ 交给内核的必须是空数组（20 万条路径不许塞进请求体）：{} 字节",
            sent.len()
        );
        assert!(
            sent.len() < DownloadTargets::REQUEST_BUDGET_BYTES,
            "发出去的那一行只有几十字节，不该接近预算：{} 字节",
            sent.len()
        );
    }

    // -----------------------------------------------------------------------
    // 任务 17：**部分**勾选一个超大批次（C-3 的另一半）
    // -----------------------------------------------------------------------

    /// 一串**确实超预算**的路径（两条用例共用；量级理由见那条 20 万条的用例）。
    fn an_over_budget_set() -> Vec<String> {
        (0..200_000)
            .map(|i| format!("dir/file-with-a-fairly-long-name-{i}.bin"))
            .collect()
    }

    /// 一份 `get_tree` 回执（`whole_tree` 的入参）：`flat` 与 `default_selected` 由调用方给。
    ///
    /// ⚠️ `default_selected` **故意与 `flat` 不同**（只给前两项）：真内核那一格是
    ///    "所有非 complete 的文件"，本来就可能比 `flat` 少；而两条路各自取哪个集合，
    ///    正是下面那几条用例要分开看的。
    fn a_tree_of(paths: &[String], default_selected: &[String]) -> TreeResult {
        TreeResult {
            tree: a_batch_of(paths).tree,
            flat: paths
                .iter()
                .map(|path| FlatEntry {
                    path: path.clone(),
                    name: path.rsplit('/').next().unwrap_or(path).to_string(),
                    size: 1,
                    state: FileState::Pending,
                })
                .collect(),
            default_selected: default_selected.to_vec(),
            progress: Progress {
                total_bytes: 1,
                done_bytes: 0,
                speed: 0,
                percent: 0,
            },
        }
    }

    /// 🔴 **部分勾选一个超大批次：切块依次发给内核，一条不少**（2026-10-12 取代原来的"拦下来"）。
    ///
    /// 以前超预算就回「请少选一些」——客户选的是整个目录，文件数多是常态，不是错误，
    /// 而且那句话让他做一件他不想做的事。现在按预算切块、每块一条 `enqueue`。
    ///
    /// 判别力：改回"超预算就拦"⇒ 写出去的条数为 0 ⇒ 红；切块时丢一条 ⇒ 拼回去不等 ⇒ 红；
    /// 有任何一块超预算 ⇒ 红（那会把客户端永久堵死）。
    #[test]
    fn a_partial_selection_of_a_huge_batch_is_sent_in_chunks_without_losing_any() {
        let huge = an_over_budget_set();
        assert!(
            DownloadTargets::exceeds_request_budget(&huge),
            "夹具没搭准：这一串单独发出去必须真的超预算"
        );
        let selection = huge[..huge.len() - 1].to_vec();
        let (session, channel) = a_session_with(LoadState::Loaded(a_batch_of(&huge)));

        let value = enqueue_with(&session, selection.clone());

        assert_eq!(value["ok"], serde_json::json!(true), "{value}");
        let written = channel.all_requests();
        assert!(written.len() > 1, "必须被切成多块：{} 条", written.len());
        let mut sent: Vec<String> = Vec::new();
        for line in &written {
            assert!(
                line.len() <= DownloadTargets::REQUEST_BUDGET_BYTES + 1024,
                "每一块都必须在预算内：{} 字节",
                line.len()
            );
            let v: serde_json::Value = serde_json::from_str(line).expect("请求是 JSON");
            for p in v["params"]["paths"].as_array().expect("paths 数组") {
                sent.push(p.as_str().expect("字符串").to_string());
            }
        }
        // 勾选面是个有序集合，`kernel_paths` 按字节序发出（见 `DownloadTargets::paths`）——
        // 所以期望的是**排序后的**勾选面：块与块之间顺序不变、一条不少。
        let mut want = selection;
        want.sort();
        assert!(sent == want, "切块之后必须一条不少、顺序不变（发出 {} 条，期望 {} 条）", sent.len(), want.len());
    }

    /// 🔴 **单条超预算的路径不进请求体**，而是逐条交代为 rejected；其余照发、顺序不变。
    ///
    /// 判别力：去掉 `enqueue_with` 里 `oversized` 那段 ⇒ 超大路径被写进请求 ⇒ 红。
    #[test]
    fn an_oversized_path_is_never_sent_and_is_named_in_the_receipt() {
        let huge = "y".repeat(DownloadTargets::REQUEST_BUDGET_BYTES);
        let batch = vec!["a.bin".to_string(), "b.bin".to_string(), huge.clone(), "c.bin".to_string()];
        let (session, channel) = a_session_with(LoadState::Loaded(a_batch_of(&batch)));

        let value = enqueue_with(&session, vec!["a.bin".to_string(), "b.bin".to_string(), huge.clone()]);

        let sent = channel.last_request();
        assert!(sent.contains(r#""paths":["a.bin","b.bin"]"#), "其余路径照发：{}", sent.len());
        assert!(!sent.contains(&huge), "超预算的那条不许出现在请求里");
        let rejections = value["data"]["rejections"].as_array().expect("rejections 是数组");
        assert_eq!(rejections.len(), 1, "{value}");
        assert_eq!(rejections[0]["path"].as_str(), Some(huge.as_str()));
        assert_eq!(
            rejections[0]["reason"],
            serde_json::json!("这一项路径太长，超过单次请求上限，没有发送"),
            "{value}"
        );
    }

    /// 第 `fails_at` 条请求（从 1 起）失败：`dies` 时是内核管道结束，否则是内核的一条错误。
    /// 其余请求照常成功，且每一块都说"还有 1 个在后台陆续加入"。
    #[derive(Clone)]
    struct FailingAt {
        written: Arc<Mutex<Vec<String>>>,
        fails_at: usize,
        dies: bool,
    }

    impl LineChannel for FailingAt {
        fn write_line(&self, line: &str) -> Result<(), ClientError> {
            self.written.lock().unwrap().push(line.to_string());
            Ok(())
        }

        fn read_line(&self) -> Result<Option<String>, ClientError> {
            let id = self.written.lock().unwrap().len();
            if id == self.fails_at && self.dies {
                return Ok(None);
            }
            if id == self.fails_at {
                return Ok(Some(format!(
                    r#"{{"id":{id},"ok":false,"error":{{"code":"invalid_params","message":"磁盘已满"}}}}"#
                )));
            }
            Ok(Some(format!(
                r#"{{"id":{id},"ok":true,"result":{{"added":[],"rejected":[],"pending":1}}}}"#
            )))
        }

        fn close(&self) {}
    }

    /// 发出去的顺序是勾选面的 `BTreeSet` 顺序（见 `kernel_paths`），分块要按它算。
    fn in_send_order(paths: &[String]) -> Vec<String> {
        paths.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect()
    }

    fn a_session_failing_at_the_second_chunk(dies: bool, huge: &[String]) -> (Arc<Session>, FailingAt) {
        let channel = FailingAt {
            written: Arc::new(Mutex::new(Vec::new())),
            fails_at: 2,
            dies,
        };
        let session = Arc::new(Session::new());
        session.install_client(
            Arc::new(CoreClient::new(Box::new(channel.clone()))),
            r#"{"protocol":2}"#.to_string(),
        );
        session.set_load(LoadState::Loaded(a_batch_of(huge)));
        (session, channel)
    }

    /// 🔴 **块与块之间失败**：这一次动作的死活照记（错误上屏），没发出去的剩余**合成一条**汇总。
    ///
    /// 判别力（改动前的代码都会红）：
    ///   · 不调 `note_failure` 并 `set_last_error(None)` ⇒ 错误被抹掉，界面说"没有错误" ⇒ 红；
    ///   · 逐条展开剩余路径 ⇒ `rejections` 不是一条 ⇒ 红（那就是十几万行的界面卡死）；
    ///   · 失败之后还接着发剩下的块 ⇒ 写出去的条数不是 2 ⇒ 红。
    #[test]
    fn a_chunk_failing_midway_is_recorded_and_the_rest_is_one_line() {
        let huge = an_over_budget_set();
        let selection = huge[..huge.len() - 1].to_vec();
        let sorted = in_send_order(&selection);
        assert!(
            DownloadTargets::chunks(&sorted).len() >= 3,
            "夹具没搭准：要证明'第二块失败、第三块没发'，至少三块"
        );
        let (session, channel) = a_session_failing_at_the_second_chunk(false, &huge);

        let value = enqueue_with(&session, selection.clone());

        let sent = channel.written.lock().unwrap().clone();
        assert_eq!(sent.len(), 2, "第二块失败之后不许再发第三块");
        assert_eq!(value["ok"], serde_json::json!(true), "前面已入队的保留，这一发不算整体失败：{value}");
        let rejections = value["data"]["rejections"].as_array().expect("rejections 是数组");
        assert_eq!(rejections.len(), 1, "剩余路径必须合成一条汇总，不许逐条展开：{}", rejections.len());
        let first: serde_json::Value = serde_json::from_str(&sent[0]).expect("请求是 JSON");
        let done = first["params"]["paths"].as_array().expect("paths 数组").len();
        let rest = selection.len() - done;
        assert_eq!(rejections[0]["path"], serde_json::json!(format!("未提交 {rest} 个")));
        assert!(
            rejections[0]["reason"].as_str().unwrap_or_default().contains("磁盘已满"),
            "{value}"
        );
        assert!(
            session.view().last_error.unwrap_or_default().contains("磁盘已满"),
            "失败必须记进错误格（不许被后面的 set_last_error(None) 抹掉）"
        );
    }

    /// 🔴 **内核在块与块之间死掉**：引擎那一格要翻；此前答应"后台陆续加入"的个数已经没有人加了，
    /// 必须并进「未提交」，话里不许再说"正在后台继续加入"（那是一句假话）。
    ///
    /// 判别力：不清零 `pending`（或不把它并进未提交）⇒ 后两条红。
    #[test]
    fn a_kernel_dying_midway_does_not_promise_the_background_ones() {
        let huge = an_over_budget_set();
        let selection = huge[..huge.len() - 1].to_vec();
        let sorted = in_send_order(&selection);
        let (session, _channel) = a_session_failing_at_the_second_chunk(true, &huge);

        let value = enqueue_with(&session, selection.clone());

        assert!(
            matches!(session.view().engine, EngineState::Unavailable(_)),
            "内核死了，引擎那一格必须翻过来：{:?}",
            session.view().engine
        );
        let summary = value["data"]["summary"].as_str().unwrap_or_default();
        assert!(!summary.contains("后台"), "内核已经死了，不许说还在后台加入：{summary}");
        let rejections = value["data"]["rejections"].as_array().expect("rejections 是数组");
        assert_eq!(rejections.len(), 1, "{value}");
        // 第一块那 1 个后台待加入 + 第二块起的剩余，全部算作未提交。
        let first_done = DownloadTargets::chunks(&sorted)[0].len();
        let rest = selection.len() - first_done + 1;
        assert_eq!(rejections[0]["path"], serde_json::json!(format!("未提交 {rest} 个")), "{value}");
    }

    /// 界面那一格：超大的部分勾选**不再**被禁用（不带 `blocked_reason`）。
    /// 判据：恢复拦截 ⇒ 这里变成字符串 ⇒ 红。
    #[test]
    fn a_huge_partial_selection_is_never_reported_as_blocked() {
        let huge = an_over_budget_set();
        let (session, _) = a_session_with(LoadState::Loaded(a_batch_of(&huge)));

        let value = whole_tree(&session, &a_tree_of(&huge, &huge), Some(&huge[..huge.len() - 1]));

        assert_eq!(
            value["data"]["action"]["blocked_reason"],
            serde_json::Value::Null,
            "大批量改为分批入队，不再拦截：{value}"
        );
        assert_eq!(value["data"]["action"]["button_title"], serde_json::json!("下载选中"), "{value}");
    }

    /// ⚠️ **整批全选（哪怕是 20 万条）永远不被挡** —— 那是超大批次**唯一**的出路。
    ///
    /// 判别力：把 `whole_tree` 里的判据改成量**勾选面**（`&selected`）而不是量
    /// `DownloadTargets::paths(...)` 的返回值 ⇒ 这一条红 —— 而真机上那是
    /// "超大批次再也下不了"：整批全选本来会被收敛成空数组（请求体恒定、必然合法），
    /// 改成量勾选面之后它会被自己的一道闸挡在门外（C-3 的全部意义就是把它放进来）。
    #[test]
    fn the_whole_batch_is_never_blocked_however_big_it_is() {
        let huge = an_over_budget_set();
        let (session, _) = a_session_with(LoadState::Loaded(a_batch_of(&huge)));

        let value = whole_tree(&session, &a_tree_of(&huge, &huge), Some(&huge));

        assert_eq!(
            value["data"]["action"]["blocked_reason"],
            serde_json::Value::Null,
            "整批全选 ⇒ 发空数组 ⇒ 永远在预算之内（这一条不许被挡）：{value}"
        );
    }

    /// ⚠️ **没给勾选面（首帧/播种那一拍）⇒ 按内核的 `default_selected` 算**（今天的行为）。
    ///
    /// 判别力：把 `None` 那一支改成"空集合"⇒ 这一条红 —— 而真机上那是**首帧的底栏
    /// 说反话**：勾选面马上要被播种成 `default_selected`（两项），底栏却说"未勾选任何项"、
    /// 按钮写着「全部下载」。
    ///
    /// ⚠️ 同一个夹具同时钉住"给了面就按给的面算"：`default_selected` 与传进来的那个
    ///    面**故意不同**（两项 vs 一项），于是"用的是哪一个面"在摘要那一格上直接可读。
    #[test]
    fn without_a_reported_selection_the_tree_falls_back_to_the_kernels_default() {
        let paths = v(&["a.bin", "b.bin", "c.bin"]);
        let default = v(&["a.bin", "b.bin"]);
        let (session, _) = a_session_with(LoadState::Loaded(a_batch_of(&paths)));

        // 没给 ⇒ 内核的默认面（两项）。
        let seeded = whole_tree(&session, &a_tree_of(&paths, &default), None);
        assert_eq!(seeded["data"]["selection"]["count_text"], serde_json::json!("已选 2 项"), "{seeded}");

        // 给了 ⇒ 前端报上来的那个面（一项）。
        let reported = whole_tree(&session, &a_tree_of(&paths, &default), Some(&v(&["c.bin"])));
        assert_eq!(reported["data"]["selection"]["count_text"], serde_json::json!("已选 1 项"), "{reported}");
        // ⚠️ 而 `selected`（内核给的选择面）**照旧原样透传**：它管的是"勾选面该被播种成什么"，
        //    与"这一刻按哪个面渲染"是两件事（前端只在 `seedSelection` 那一拍读它）。
        assert_eq!(
            reported["data"]["selected"],
            serde_json::json!(default),
            "内核给的选择面不许因为前端报了一个面就被顶掉：{reported}"
        );
    }

    /// 🔴 **档位映射的两个方向都要钉住**（规格 §2.4）。
    ///
    /// 为什么必须有它：`apply_shell_log_level` 原来把 `if/else` 直接写在 `init(...)`
    /// 的实参位置上，而 `init` 落的是**进程级静态** ⇒ 那一格一条判据都没有：
    /// **把两支对调，全套判据照旧全绿**（拆出 [`super::shell_level`] 就是为了这一条）。
    /// 真机上那是本功能**最坏的那个形状**：用户勾上「详细日志」，**内核**那一档确实
    /// 变详细了（`core_arguments` 有判据），而**壳那份日志停在 normal** ⇒
    /// `diag-shell.log` **根本不会被创建**，客户回传的只有内核那一份 ——
    /// 而那正是"详细日志要能回答壳当时在做什么"这件事要防的。
    ///
    /// 判别力：把 `shell_level` 的两支对调 ⇒ 下面两条断言同时红。
    /// ⚠️ **它钉住的是映射本身**；`init(...)` 那个调用点有没有被走到仍然没有判据
    ///    （理由见 `shell_level` 的文档），那一半只有真机验收那两条能回答。
    #[test]
    fn the_verbose_checkbox_maps_onto_the_shells_own_log_level_both_ways() {
        use shell_core::diagnostics::Level;

        let off = Preferences::new("D:\\out");
        assert_eq!(super::shell_level(&off), Level::Normal, "关着就是普通档（缺省）");

        let on = off.setting_verbose_logging(true);
        assert_eq!(super::shell_level(&on), Level::Verbose, "勾上必须是详细档");

        // 反向：**认的不是"偏好里有没有这个字段"，是它的取值**。
        assert_eq!(
            super::shell_level(&on.setting_verbose_logging(false)),
            Level::Normal,
            "关回去要跟着落回普通档（只认一个方向的实现过不了这一条）"
        );
    }

    /// **会阻塞地问内核的命令，一条都不许开在主线程上**（2026-10-08）。
    ///
    /// 为什么要有这条：
    ///   Tauri 的**非 async** 命令在**收 IPC 的那条线程**上跑完才回 ——
    ///   `tauri::ipc::ResponseTag::block` 就是 `resolver.respond(...)`，
    ///   而 `Webview::on_message` 里**没有** spawn（都在 vendored 源码里读过）。
    ///   而 IPC 是走**主线程**的 ⇒ 只要那条命令阻塞，窗口就「未响应」。
    ///   客户在 0.2.1 上实测到的正是这个形状：**短暂未响应、过一会自己好** ——
    ///   那是"跑完了"；0.2.0 那条 `df` 是"跑不完"，所以只能强杀。
    ///
    /// 判据：**同步（未加 `(async)`）的命令体里不许出现内核调用**。
    ///   `#[tauri::command(async)]` 加在非 async 函数上，生成的是 `sync_threadpool`
    ///   （`tauri-macros/src/command/wrapper.rs`：`ExecutionContext::Async
    ///   if function.sig.asyncness.is_none()`）⇒ 函数体跑在**线程池**上、主线程空着，
    ///   而**函数签名一个字不用改**。
    ///   ⚠️ 真把它写成 `async fn` 是**另一回事**：带引用入参的 async 命令有硬约束
    ///   "must return a `Result`"（同一份宏源码里的 `compile_error!`），那会动到
    ///   21 条命令的返回类型与前端契约。
    ///
    /// 判别力：把任意一条的 `(async)` 摘掉 ⇒ 这条立刻红。
    #[test]
    fn every_command_that_talks_to_the_kernel_runs_off_the_main_thread() {
        let src = include_str!("commands.rs");

        // 逐条命令切：`#[tauri::command…]` 到下一条 `#[tauri::command` 之间就是它的体。
        //
        // ⚠️ **必须收紧到"顶格 + 紧跟换行 + `pub fn`"**：本文件里有好几处**在注释与
        //    断言字符串里提到这个名字**（写这条判据的那段文字自己就提到它），
        //    `include_str!` 会把它们一并扫进来 —— 第一版就是这么红的（27 条而不是 21 条）。
        let mut decls: Vec<(bool, &str)> = Vec::new();
        for (idx, _) in src.match_indices("#[tauri::command") {
            if idx != 0 && src.as_bytes()[idx - 1] != b'\n' {
                continue; // 不在行首 ⇒ 引用，不是声明
            }
            let head = &src[idx..];
            let after = match head.strip_prefix("#[tauri::command(async)]\n") {
                Some(rest) => (true, rest),
                None => match head.strip_prefix("#[tauri::command]\n") {
                    Some(rest) => (false, rest),
                    None => continue, // 带别的参数的形态在这里不存在；真加了要一并想
                },
            };
            let (is_async, after) = after;
            let Some(rest) = after.strip_prefix("pub fn ") else {
                continue;
            };
            let Some(paren) = rest.find('(') else { continue };
            decls.push((is_async, &rest[..paren]));
        }
        // ⚠️ **命令数写死：改名 / 拆分 / 新增都会让"扫到了几条"这件事本身变成判据**
        //   （同 `ExitCode::ALL` 那条"数组长度是显式的"的纪律）。
        //   本波次 +3：`update_status` / `update_set_enabled`（都问内核 ⇒ `(async)`）
        //   与 `open_update_url`（**不问内核**，只做一次系统调用 ⇒ 保持同步、跑在
        //   收 IPC 的那条线程上 —— 它不许阻塞，也不该为它开一条线程池线程）。
        //   2026-10-15 +1：`update_check_now`（问内核、且内核那一条里要发网络请求
        //   ⇒ `(async)`，理由见它自己的文档）。
        //   2026-10-21 +1：`restart_progress`（问内核、只读 ⇒ `(async)`；
        //   它是**轮询**用的装饰命令，拿不到就回 `text: null`，不升级成红条）。
        assert_eq!(
            decls.len(),
            27,
            "命令数变了（期望 26）：{decls:?} —— 新增命令时请一并想清楚它该不该开在主线程上"
        );

        let mut offenders = Vec::new();
        let mut scanned_sync = 0;
        for (is_async, name) in decls.iter() {
            if *is_async {
                continue;
            }
            scanned_sync += 1;
            let start = src.find(&format!("pub fn {name}(")).expect("刚切出来的名字");
            // ⚠️ 体的边界靠**花括号配平**，不靠"下一条命令在哪"。
            //    第一版用的是后者，结果是**最后那一条命令的体一路罩到文件尾**，
            //    把 `restart_kernel` 自己的定义与测试模块都算了进去（两轮红都是这么来的）。
            let Some(brace) = src[start..].find('{') else {
                continue;
            };
            let tail = &src[start + brace..];
            let mut depth = 0usize;
            let mut end = src.len();
            for (off, ch) in tail.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = start + brace + off + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &src[start..end];
            // ⚠️ **只查代码行**：注释里提到这些名字是常事（本仓的注释本来就爱点名），
            //    把它们算进去会让这条判据变成"谁在注释里提一句就红"——那种会被绕开。
            let code: String = body
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for marker in ["client()", "restart_client", "restart_kernel"] {
                if code.contains(marker) {
                    offenders.push(format!("{name} 里出现了 {marker}"));
                }
            }
        }
        // 空扫也算不过（万一上面的切法坏了，"一条都没扫到"会被当成通过）。
        assert!(
            scanned_sync > 0,
            "一条同步命令都没扫到 —— 这条判据的切法坏了，不是「通过」"
        );
        assert!(
            offenders.is_empty(),
            "这些命令是同步的、却会阻塞地问内核 ⇒ 它们跑在主线程上，窗口会「未响应」。\
             处置：把 `#[tauri::command]` 改成 `#[tauri::command(async)]`（签名不用动）。\n{offenders:#?}"
        );
    }

    // ------------------------------------------------------------------------
    // 引擎徽标：成功 ⇒ 「运行中」（2026-10-14 现场缺口的接线用例）
    // ------------------------------------------------------------------------

    /// 一条**固定回一句**的内核通道：回 `transfer_list` 形状的快照回执。
    ///
    /// 与 [`Recording`] 的区别只在回执的形状（`enqueue` 的回执解不出
    /// `TransferListResult`）。`{id}` 占位符换成该条请求的 id（同 `Recording` 的账法）。
    #[derive(Clone)]
    struct Saying {
        reply: String,
        written: Arc<Mutex<Vec<String>>>,
    }

    impl Saying {
        fn new(reply: &str) -> Saying {
            Saying {
                reply: reply.to_string(),
                written: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl LineChannel for Saying {
        fn write_line(&self, line: &str) -> Result<(), ClientError> {
            self.written.lock().unwrap().push(line.to_string());
            Ok(())
        }

        fn read_line(&self) -> Result<Option<String>, ClientError> {
            let id = self.written.lock().unwrap().len();
            Ok(Some(self.reply.replace("{id}", &id.to_string())))
        }

        fn close(&self) {}
    }

    /// 一条空的传输快照回执（`items: []` + 全零 `global` —— `kernel.rs` 同款夹具）。
    const AN_EMPTY_SNAPSHOT: &str = r#"{"id":{id},"ok":true,"result":{"items":[],"global":{"download_speed":0,"num_active":0,"num_waiting":0,"num_stopped":0}}}"#;

    /// 🔴 **成功的传输快照 ⇒ 引擎徽标翻「运行中」**（对齐 macOS `fetchTransfers`
    ///    的 `if engine != .running { engine = .running }`，`AppModel.swift:1642`）。
    ///
    /// 2026-10-14 现场（诊断日志 061520）就是这条缺口的形状：`task_action` 有这一行、
    /// 200 ms 轮询没有 ⇒ **列表明明在下载、徽标停在「引擎未启动」**，而且没有任何
    /// 东西变红（图标与文案都是既有判据算的，只是输入那一格过期了）。
    ///
    /// 判别力：去掉 `Snapshot` 臂那一行 ⇒ 本条红。
    #[test]
    fn a_transfer_snapshot_promotes_the_engine_badge_to_running() {
        let ch = Saying::new(AN_EMPTY_SNAPSHOT);
        let session = Arc::new(Session::new());
        session.install_client(
            Arc::new(CoreClient::new(Box::new(ch))),
            r#"{"protocol":2}"#.to_string(),
        );
        session.set_engine(EngineState::NotStarted);

        let _ = transfers_with(&session);

        assert_eq!(
            session.view().engine,
            EngineState::Running,
            "快照到达 ⇔ 引擎真的在跑 —— 徽标必须跟着翻"
        );
    }

    /// 🔴 **全部块入队成功 ⇒ 引擎徽标翻「运行中」**（对齐 macOS 入队尾部的
    ///    `if allSent { engine = .running }`，`AppModel.swift:1822` —— 成功 ⇒
    ///    内核在 `op_enqueue` 里已经 `ensure_engine` 过了）。
    ///
    /// 判别力：去掉 `enqueue_with` 成功尾部那一行 ⇒ 本条红。
    #[test]
    fn a_fully_sent_enqueue_promotes_the_engine_badge_to_running() {
        let paths = v(&["a.bin", "b.bin"]);
        let (session, _ch) = a_session_with(LoadState::Loaded(a_batch_of(&paths)));
        session.set_engine(EngineState::NotStarted);

        let value = enqueue_with(&session, paths);

        assert_eq!(value["ok"], serde_json::json!(true), "{value}");
        assert_eq!(
            session.view().engine,
            EngineState::Running,
            "全部块都发成功 ⇒ 引擎在跑（内核 ensure_engine 过）"
        );
    }

    /// **部分失败不得翻「运行中」**：那一格上刚被 `note_failure` 写下的失败结论
    /// （`.unavailable` / `.notStarted` / 错误原文）是这次动作的结论，不能被覆盖
    /// （与 macOS `if allSent` 的条件逐字同款）。
    #[test]
    fn a_partially_sent_enqueue_does_not_claim_the_engine_is_running() {
        let huge = an_over_budget_set();
        let selection = huge[..huge.len() - 1].to_vec();
        let (session, _channel) = a_session_failing_at_the_second_chunk(false, &huge);
        session.set_engine(EngineState::NotStarted);

        let _ = enqueue_with(&session, selection);

        assert_ne!(
            session.view().engine,
            EngineState::Running,
            "只有全部块成功才能断言引擎可用"
        );
    }

    // ------------------------------------------------------------------------
    // 批次历史：「记住交付码」（规格 §8.3 / 阶段 E-4，2026-10-14 现场缺口）
    // ------------------------------------------------------------------------

    /// 一个**临时的**历史文件路径（按 `export.rs` 的 `a_temp_dir` 同款账法）。
    /// 每条用例自己一个名字 ⇒ 并行跑也互不干扰。
    fn a_temp_history_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "benagen-history-{tag}-{}.json",
            std::process::id()
        ))
    }

    /// 🔴 **一次成功的加载记「又用了一次」，记的是请求里的那个 base_url**
    ///    （对齐 macOS `AppModel.recordLoadedBatch` —— 不是内核回显的 `info.base_url`：
    ///    回显那份在"用户没指定"时是内核的**默认**服务器，写进历史再显式发回去，
    ///    会在内核默认值变化时静默分叉，E-5 明禁）。
    #[test]
    fn a_successful_load_is_recorded_with_the_requested_base_url() {
        let path = a_temp_history_path("record");
        let _ = std::fs::remove_file(&path);

        record_loaded_batch_in(&path, "C24-8", "https://user.example").expect("写历史不该失败");

        let history = History::load(&path);
        let newest = history.most_recent().expect("历史里必须有刚记的那一条");
        assert_eq!(newest.code, "C24-8");
        assert_eq!(newest.base_url, "https://user.example");
        let _ = std::fs::remove_file(&path);
    }

    /// 🔴 **E-4：历史非空 ⇒ 自动加载最近使用的那一条**（内核 `last_code` 一个字都不读 ——
    ///    权威只有一个，"界面显示 A、内核却自动加载 B"是它要防的分裂）。
    #[test]
    fn remembered_code_prefers_the_histories_most_recent() {
        let path = a_temp_history_path("most-recent");
        let _ = std::fs::remove_file(&path);
        write_history_entry(
            &path,
            &HistoryWrite::Used {
                code: "旧-1".into(),
                base_url: "https://old.example".into(),
                note: None,
            },
        )
        .expect("写历史不该失败");
        write_history_entry(
            &path,
            &HistoryWrite::Used {
                code: "新-2".into(),
                base_url: "https://new.example".into(),
                note: None,
            },
        )
        .expect("写历史不该失败");

        let got = remembered_code_in(&path, "内核-说-别的").expect("该有码可加载");

        assert_eq!(
            got,
            ("新-2".to_string(), "https://new.example".to_string()),
            "历史非空 ⇒ 用最近一条（base_url 跟着回去），内核 last_code 不参与"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 🔴 **E-4：历史为空 ⇒ 内核 `last_code` 播种一次**（macOS `rememberedCode` 的
    ///    "先落下再加载"：种子落进历史之后，壳不再读内核那份做决策）。
    ///    ⚠️ 内核那个值可能带结尾换行（macOS 逐字注明）：trim 之后再当主键用。
    #[test]
    fn remembered_code_seeds_the_kernel_last_code_when_history_is_empty() {
        let path = a_temp_history_path("seed");
        let _ = std::fs::remove_file(&path);

        let got = remembered_code_in(&path, "C24-8\n").expect("播种之后该有码可加载");

        assert_eq!(
            got,
            ("C24-8".to_string(), String::new()),
            "trim 之后当主键；base_url 空 = 默认服务器"
        );
        let history = History::load(&path);
        let newest = history
            .most_recent()
            .expect("播种必须先落盘（之后壳不再读内核那份）");
        assert_eq!(newest.code, "C24-8");
        let _ = std::fs::remove_file(&path);
    }

    /// **什么都没有 ⇒ 一个请求都不发**（"没有任何码可加载"是它的正常态）；
    /// 也**不许**凭空造一个历史文件。
    #[test]
    fn remembered_code_is_silent_when_there_is_nothing_to_remember() {
        let path = a_temp_history_path("nothing");
        let _ = std::fs::remove_file(&path);

        assert_eq!(remembered_code_in(&path, "  \n"), None, "空白的 last_code 不是码");
        assert_eq!(remembered_code_in(&path, ""), None, "空串更不是");
        assert!(!path.exists(), "没有码可加载时不该造出历史文件");
    }
}
