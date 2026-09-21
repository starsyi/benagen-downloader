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
//! `kernel::*` 那些函数是**阻塞**的（`CoreClient::call` 没有每请求超时，
//! `load_delivery` 最坏约 91.5 秒），而 `Session` 的锁是所有命令共用的
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
use shell_core::presentation::about_info::AboutInfo;
use shell_core::presentation::app_preferences::{DownloadDirChange, DownloadDirectory};
use shell_core::presentation::breadcrumb::Breadcrumb;
use shell_core::presentation::browser_row::{BrowserRow, BrowserSelection, SelectionSummary};
use shell_core::presentation::delivery_summary::DeliveryCodeEntry;
use shell_core::api::tree::DownloadAction;
use shell_core::presentation::download_targets::{DownloadTargets, EnqueueFeedback};
use shell_core::presentation::kernel_death::CallFailure;
use shell_core::presentation::transfer_row::{
    TransferGlobalSummary, TransferListEmpty, TransferReveal, TransferRow,
};
use shell_core::presentation::verify_summary::{ProgressSummary, VerifySummary};
use shell_core::protocol::{EngineState, LoadState, Settings, TaskAction};
use shell_core::storage::history::History;
use shell_core::storage::preferences::Preferences;

use crate::kernel::{self, TransferPoll};
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
        load,
        retry,
        tree,
        enqueue,
        transfers,
        verify,
        // 任务 8 补的十个（规格 §3.4 那张表里标着"无（新增）"的那些）+ 一个
        // ⚠️ 规格表里没有的 `preferences_check` —— 理由见它自己的文档。
        task_action,
        reveal,
        settings_get,
        settings_set,
        preferences_get,
        preferences_check,
        preferences_set,
        // ⚠️ 规格 §3.4 那张表里没有这一条（同 `preferences_check`）：它是**系统对话框**
        //    那一路，第二代与 macOS 都没有对应的"端点"（macOS 那边是视图里直接调
        //    `NSOpenPanel`，没有命令行）。理由写在它自己的文档上。
        pick_directory,
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
#[tauri::command]
pub fn state(app: tauri::State<'_, Shell>) -> Value {
    let view = app.session.view();
    // ⚠️ 第二个实参是 `view.handshake_reply` 的**那一份**（计划任务 7 的调用点逐字）：
    //    冗余是这个签名本来的样子，不是笔误 —— 见 `api::state::state` 的文档。
    api::state::state(&view, view.handshake_reply.as_deref())
}

/// 起一次 `load_delivery`（规格 §3.4 第二行，对齐第二代 `POST /api/load`）。
///
/// **异步**：结果落在 `session.load` 上，前端靠轮询 `state()` 取（D6）——
/// `load_delivery` 最坏约 91.5 秒，同步会把这条 `invoke` 按住那么久。
///
/// ⚠️ **长度闸在发之前**（第二代那两条判据一字未搬，只是换了调用点）：
///    超限的请求内核**不报错**，它只把客户端**静默堵死**（`core/src/main.rs` 的行长上限
///    与 `CoreClient` 的 FIFO 队列，见 `DeliveryCodeEntry::MAXIMUM_BYTES` 的文档）。
///    判据只有 `DeliveryCodeEntry` 那一份 —— 本层不重写一遍长度判断。
#[tauri::command]
pub fn load(app: tauri::State<'_, Shell>, code: String, base_url: Option<String>) -> Value {
    // ⚠️ 前端不传 `base_url` 时它是 `None`（"高级：自定义下载地址"那一格没填）⇒ 空串，
    //    与第二代 `body.get("base_url").and_then(as_str).unwrap_or("")` 逐字同义。
    let base_url = base_url.unwrap_or_default();
    if !DeliveryCodeEntry::is_sendable(&code) {
        app.session
            .set_load(LoadState::Failed(DeliveryCodeEntry::too_long_hint()));
        return api::envelope::ok(serde_json::json!({ "status": "rejected" }));
    }
    if !base_url.is_empty() && !DeliveryCodeEntry::is_sendable(&base_url) {
        app.session
            .set_load(LoadState::Failed(DeliveryCodeEntry::base_url_too_long_hint()));
        return api::envelope::ok(serde_json::json!({ "status": "rejected" }));
    }
    // ⚠️ 锁的纪律：`client()` 克隆出来就放锁，`spawn_load` 起后台线程跑那 91.5 秒。
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
#[tauri::command]
pub fn tree(
    app: tauri::State<'_, Shell>,
    path: Option<String>,
    selection: Option<Vec<String>>,
) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match path {
        Some(path) => match kernel::list_dir(&client, &path) {
            Ok(result) => {
                app.session.set_last_error(None);
                // ⚠️ 面包屑由壳拼（`list_dir` 的目录项没有 `path` 键），而且这里传的
                //    `path` 就是请求里那一条**清单原文**（约束 3：不 trim、不加工）。
                api::tree::level(
                    &Breadcrumb::new(&path),
                    &BrowserRow::rows(&result.entries, &path),
                )
            }
            // ⚠️ **读不到某一层仍然是 `ok` 信封**（那是界面上的一个状态，不是整屏错误）——
            //    判据在 `api::tree::level_failure` 的文档里。面包屑跟着**退到的那一层**走。
            Err(why) => api::tree::level_failure(&Breadcrumb::new(&why.path), &why),
        },
        None => match kernel::get_tree(&client) {
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
    // 🔴 "此刻能不能按"：判据**只有一处实现**（`DownloadTargets::blocked_reason`），
    //    入参是**真正要发出去的那一串**（与 `enqueue_with` 那条路同一个算式）。
    //    ⚠️ 别把 `selected` 直接喂进去：整批全选会被 `paths` 收敛成空数组，
    //    而那条路**必须**永远走得通（它是超大批次唯一的出路 —— 那句论证在
    //    `DownloadTargets::blocked_reason` 的文档里，入参那一条同理）。
    let outgoing = DownloadTargets::paths(&selected, &BrowserSelection::all_files(&tree.flat));
    // ⚠️ 底栏那颗按钮的文案与它旁边那句「明说」：**同一个勾选面**（`selected`，
    //    也就是下面 `SelectionSummary::of` 吃的那一个）—— 几格说的必须是同一件事。
    //    这些字符串都由 `DownloadTargets` 算（`presentation` 的既有纯函数，有单测），
    //    本层只做那一次三元判断（"一项都没勾" ⇒ 那句话在场）——
    //    它的出处是 macOS 侧 `SelectionBar.swift:33` 的 `hint: selection.isEmpty ? … : nil`
    //    （那是视图里的一步取值，不是一条能被单测钉住的判据）。
    let action = DownloadAction {
        button_title: DownloadTargets::button_title(&selected),
        empty_selection_hint: if selected.is_empty() {
            Some(DownloadTargets::empty_selection_hint().to_string())
        } else {
            None
        },
        blocked_reason: DownloadTargets::blocked_reason(&outgoing).map(str::to_string),
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
///    内核按 `take(8 MiB)` 读不到整行、于是回一条对不上任何请求 id 的协议告警），
///    今天仍然敞着、归真机验收清单 —— 那件事的**出处是 `client.rs` 的
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
#[tauri::command]
pub fn enqueue(app: tauri::State<'_, Shell>, paths: Vec<String>) -> Value {
    enqueue_with(&app.session, paths)
}

/// `enqueue` 的**命令体**（[`enqueue`] 的唯一实现，单测直接调它）。
///
/// 三步（规格 §5.3）：取数据（**会话快照**里那一批的全部文件）→ 判据（`kernel_paths`
/// 里那一次 `DownloadTargets::paths`）→ 交给内核。
fn enqueue_with(session: &Session, paths: Vec<String>) -> Value {
    let Some(client) = session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 锁的纪律：`client()` 与 `view()` 都是"读一次就放锁"，两次临界区都在这一行之内，
    //    之后的 `kernel::enqueue` 手里没有会话锁（见文件头那条）。
    let paths = kernel_paths(&session.view().load, &paths);
    // 🔴 **兜底闸**（任务 17 的 C-3 另一半，判据仍是同一个 `DownloadTargets::blocked_reason`）：
    //    超过安全预算的请求**一个字节都不发出去**，并且把理由当作这一发的回执交给用户。
    //    ⚠️ 它为什么还需要（界面那一格不是已经禁用了吗）：那一格是**异步**取回来的
    //    （勾选面一变、载荷要过一小会儿才回来），而用户在那一小段里**点得下去** ——
    //    没有这道闸，落到用户眼前的就是 `client.rs` 里那句"…补救：…分批发"
    //    （**界面根本没有"分批"这个动作**，等于教用户做一件他做不到的事）。
    //    ⚠️ 闸门在 `kernel::enqueue` **之外**：不发、不排队、不碰内核
    //    （同 macOS 侧 `AppModel.enqueue` 那道守卫的纪律：点一下下载不该换掉用户的内核）。
    if let Some(why) = DownloadTargets::blocked_reason(&paths) {
        // ⚠️ **不碰 `last_error`**：被挡下不是"上一次失败过去了"—— 清掉那一格会让常驻提示行
        //    上那条（可能是在说"内核没了"）凭空消失，而这一发根本没跟内核说过话
        //    （同 `transfers` 那条"`Absorbed` 不碰 `last_error`"的纪律）。
        // ⚠️ 那句话住在 `shell-core`（`DownloadTargets::blocked_reason`），本层**一个字都不写**
        //    （R-24：命令层里不许出现面向用户的字符串）——这里只把它交给既有的回执形状。
        return api::enqueue::enqueue(&EnqueueFeedback::blocked(why));
    }
    match kernel::enqueue(&client, &paths) {
        Ok(result) => {
            session.set_last_error(None);
            // ⚠️ `added` 与 `rejected` 怎么成句全在 `EnqueueFeedback::of` 里（约束 4：
            //    两边都要有落点）；本层只把它交给 `api::enqueue::enqueue` 装信封。
            api::enqueue::enqueue(&EnqueueFeedback::of(&result))
        }
        Err(why) => {
            note_failure(session, &why);
            api::failure(&why)
        }
    }
}

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

/// 传输列表（规格 §3.4 第六行，对齐第二代 `GET /api/transfers`）。
///
/// ⚠️ 前端按 **200 ms** 轮询它（`TransferListPoll::INTERVAL_NANOSECONDS`），
///    而闸门是 `state().allows_requests`（`EngineGate`，规格 §3.5）。
#[tauri::command]
pub fn transfers(app: tauri::State<'_, Shell>) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 空列表那句话要用到引擎态 ⇒ 先取一份快照（锁只在这一行里）。
    let engine = app.session.view().engine;
    // ⚠️ 先取出这一拍的结局再 `match`（`flips_engine_to_not_started` 要读它，
    //    而那两处**在同一臂里**：一臂拿走了 `poll` 就没法再问它了）。
    let poll = kernel::transfer_list(&client);
    match poll {
        TransferPoll::Snapshot(result) => {
            app.session.set_last_error(None);
            let rows: Vec<TransferRow> = result.items.iter().map(TransferRow::new).collect();
            api::transfers::transfers(
                &rows,
                Some(&TransferGlobalSummary::of(&result.global)),
                TransferListEmpty::of(&engine),
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
            if poll.flips_engine_to_not_started() {
                app.session.note_engine_not_started();
            }
            api::transfers::transfers(&[], None, TransferListEmpty::of(&engine))
        }
        // 🔴 **内核亲口说"这一批没了"**（`no_delivery`）：除了发空快照，还要**作废当前批次**
        //    （清掉 `load_request` —— 对齐 macOS 的 `resetToEmptyState()`）。
        //    ⚠️ 少了这一句，那个死码会在**下一次内核重启**（改下载目录 / 点「重试」）时被
        //    **重放**一遍，用户看到一次本不该出现的失败 —— 而 macOS 在那一刻是**空态**
        //    （本任务审查 I-1 抓的）。
        //    ⚠️ 这一档**不碰 `last_error`**：macOS 的 `absorb` 把它当 `.absorbed`
        //    （正常态、不显示那句话），我们照办 —— 界面上的表现就是**空态**，不是红横幅。
        TransferPoll::NoDelivery => {
            app.session.reset_to_empty_state();
            api::transfers::transfers(&[], None, TransferListEmpty::of(&engine))
        }
        TransferPoll::Failed(why) => {
            note_failure(&app.session, &why);
            api::failure(&why)
        }
    }
}

/// 校验结果（规格 §3.4 第七行，对齐第二代 `GET /api/verify`）。
///
/// ⚠️ 前端**只在校验页可见时**按 1 s 轮询它（规格 §3.5），否则一发都不发。
#[tauri::command]
pub fn verify(app: tauri::State<'_, Shell>) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::verify_status(&client) {
        Ok(status) => {
            app.session.set_last_error(None);
            // ⚠️ `VerifySummary::of` 收 `Option`，而"还没取过"（`None`）那一档**在这里
            //    就分掉了**：走到这一行的每一条路都刚拿到内核回的 `status` ⇒ 恒 `Some`。
            //    ⚠️ 那句"算不出摘要"的话**不在本文件里**：它是一句面向用户的失败文案，
            //    按纪律（规格 §3.2 在 Rust 侧的同一形态）归 `shell-core::api`。
            //    它今天的不可达性、以及**为什么宁可留一句也不许 panic** 都写在
            //    `api::verify::NO_VERIFY_SUMMARY` 的文档里 —— 别把它搬回这里。
            match VerifySummary::of(Some(&status)) {
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
#[tauri::command]
pub fn task_action(
    app: tauri::State<'_, Shell>,
    action: TaskAction,
    gid: Option<String>,
) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    match kernel::task_action(&client, action, gid) {
        Ok(()) => {
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
#[tauri::command]
pub fn settings_get(app: tauri::State<'_, Shell>) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    // ⚠️ 锁的纪律：`view()` 与 `client()` 都是"读一次就放锁"（`-k` 的候选集合要从
    //    这一格里取，而它是一份 `String` 的克隆 —— 与内核无关）。
    let handshake = app.session.view().handshake_reply;
    match kernel::get_settings(&client) {
        Ok(current) => {
            app.session.set_last_error(None);
            api::settings::payload(&current, handshake.as_deref())
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
///    （`SettingsForm::APPLY_NOTE` 说的就是这件事）。改**下载目录**是另一条命令
///    （`preferences_set`）—— 那一个**必须**重启内核，因为目录只在 argv 上。
///
/// ⚠️ **回执发的是"写完之后"那一份**（内核的 `set_settings` 回的是归一化之后的值，
///    例如 `-k` 的 `"21m"` 会被归一成 `"21M"`）—— 与 `settings_get` **同一个形状**，
///    前端只有一条渲染路径。⚠️ 让它回读一次（再发一条 `get_settings`）是多余的：
///    内核这条响应本身就是"现在生效的那一份"。
#[tauri::command]
pub fn settings_set(app: tauri::State<'_, Shell>, settings: Settings) -> Value {
    let Some(client) = app.session.client() else {
        return api::envelope::err(api::NO_KERNEL);
    };
    let handshake = app.session.view().handshake_reply;
    match kernel::set_settings(&client, &settings) {
        Ok(echoed) => {
            app.session.set_last_error(None);
            api::settings::payload(&echoed, handshake.as_deref())
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
pub fn preferences_get() -> Value {
    match load_preferences() {
        Ok(preferences) => api::preferences::current(&preferences),
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
#[tauri::command]
pub fn preferences_set(app: tauri::State<'_, Shell>, dir: String) -> Value {
    let path = match preferences_path() {
        Ok(path) => path,
        Err(why) => return api::envelope::err(&why),
    };
    let current = Preferences::load(&path);
    // ② 之前先归一化（`setting_download_dir` 走 `AppPreferences::normalized`：
    //    只去首尾空白）—— 于是"多打了两个空格"不会被当成一次改动。
    let updated = current.setting_download_dir(&dir);

    if updated.download_dir == current.download_dir {
        return api::preferences::change(&DownloadDirChange::Unchanged {
            dir: updated.download_dir,
        });
    }
    if let Err(cause) = updated.save(&path) {
        return api::preferences::change(&DownloadDirChange::Failed {
            message: api::preferences::save_failed(&cause.to_string()),
        });
    }
    match restart_kernel(&app.session) {
        Ok(()) => api::preferences::change(&DownloadDirChange::Changed {
            dir: updated.download_dir,
        }),
        // 内核起不来的**原因原文**照登（可能是"没找到内核…"，也可能是握手超时那句，
        // 也可能是"没在期限内落地"）—— 三种都是**完整的一句话**，壳不再加工。
        Err(why) => api::preferences::change(&DownloadDirChange::Failed { message: why }),
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
    let history = History::load(&path);
    let updated = api::history::apply(&history, &entry, current_unix_seconds());
    if let Err(cause) = updated.save(&path) {
        return api::envelope::err(&api::history::write_failed(&cause.to_string()));
    }
    api::history::payload(&updated)
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
fn restart_kernel(session: &Arc<Session>) -> Result<(), String> {
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
    wait_for_the_kernel(session)
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
fn wait_for_the_kernel(session: &Session) -> Result<(), String> {
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
                    return Err(api::preferences::restart_timed_out().to_string());
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
fn note_failure(session: &Session, why: &CallFailure) {
    match why {
        CallFailure::KernelGone { reason, .. } => {
            session.note_kernel_death(reason.clone());
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
            Ok(Some(
                r#"{"id":1,"ok":true,"result":{"added":[],"rejected":[]}}"#.to_string(),
            ))
        }

        fn close(&self) {}
    }

    /// 一个**装上了内核连接**的会话 + 那条通道的记账句柄。
    fn a_session_with(load: LoadState) -> (Session, Recording) {
        let channel = Recording::new();
        let session = Session::new();
        session.install_client(
            Arc::new(CoreClient::new(Box::new(channel.clone()))),
            r#"{"protocol":1}"#.to_string(),
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
    ///    🔴 **"内核回 `id == 0` ⇒ 那条 `invoke` 永远不 resolve"是另一件事，今天仍然敞着**
    ///    —— 它说的是闸的**另一侧**（内核按 `take(8 MiB)` 读不到整行、于是回一条对不上
    ///    任何请求 id 的协议告警），而 `CoreClient::call` **没有每请求超时** ⇒ 任何一条
    ///    走到那条路上的请求都会让对应的 `invoke` 永久挂住。本用例**钉不到它**（这个触发器
    ///    在写出之前就被拦了，够不着内核），它归**真机验收清单**：账见
    ///    `.superpowers/sdd/2026-09-20-windows-tauri-client/task-16-report.md` §7①。
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

    /// 🔴 **部分勾选一个超大批次：不许碰内核，而且要说得出话**（任务 17 的兜底闸）。
    ///
    /// 判别力（**两条断言各挡一种写法**）：
    ///   · 去掉 `enqueue_with` 里那道 `if let Some(why)` ⇒ 第一条红：20 万条路径的请求体
    ///     约 9.8 MB，`CoreClient::call` 在写出去之前把它拒了（`RequestTooLong`），
    ///     落到用户眼前的是 `client.rs` 那句"…补救：…分批发"——**界面没有"分批"这个动作**；
    ///   · 把 `EnqueueFeedback::blocked` 的 `summary` 换成空串 ⇒ 第二条红：用户点了下载，
    ///     界面一个字都不说（约束 4 明禁的静默失效）。
    #[test]
    fn a_partial_selection_of_a_huge_batch_never_reaches_the_kernel() {
        let huge = an_over_budget_set();
        // ⚠️ 先钉**夹具的判别力**：这一串**作为显式列表**发出去必须是超预算的
        //    （夹具搭小了的话，下面的断言会退化成"一条本来就合法的请求也合法"）。
        assert!(
            DownloadTargets::exceeds_request_budget(&huge),
            "夹具没搭准：这一串单独发出去必须真的超预算"
        );
        // 勾选面 = 前 199999 项（**差一项**：不构成"整批全选"）。
        let selection = huge[..huge.len() - 1].to_vec();
        // ⚠️ 先钉**这条夹具走的是哪一支**：差一项 ⇒ 判据走的是**显式列表**那一支，
        //    不是"整批全选 ⇒ 发 `[]`"那一支（后者永远不超预算，那是任务 16 那条触发器，
        //    两条不能混）。少一项都不行的判据由 `a_real_subset_goes_out_one_by_one` 钉着。
        assert_eq!(
            kernel_paths(&LoadState::Loaded(a_batch_of(&huge)), &selection).len(),
            selection.len(),
            "夹具没搭准：这一条必须走显式列表那一支（逐条发出去）"
        );
        let (session, channel) = a_session_with(LoadState::Loaded(a_batch_of(&huge)));

        let value = enqueue_with(&session, selection);

        assert_eq!(value["ok"], serde_json::json!(true), "这一发是一个**回执**，不是错误信封：{value}");
        assert_eq!(
            channel.written_len(),
            0,
            "被挡下的请求**一个字节都不许写给内核**（写出去只会换来报错或挂死）"
        );
        assert_eq!(
            value["data"]["summary"],
            serde_json::json!("选中的项太多，超过一次能发出的上限：请少选一些"),
            "用户必须看到**壳写的**那句理由（不是 client.rs 原文）：{value}"
        );
        assert_eq!(
            value["data"]["switches_to_transfers"],
            serde_json::json!(false),
            "一件都没加进去，不许切走（切走会把这句话吞掉）：{value}"
        );
    }

    /// 🔴 **界面那一格：超预算的部分勾选 ⇒ 那个动作是禁用的、并带着理由**（任务 17）。
    ///
    /// 判别力：把 `whole_tree` 里 `blocked_reason:` 那一行改成恒 `None` ⇒ 这一条红 ——
    /// 而真机上那是**界面永远不拦**：用户勾了大半个超大批次，按钮亮着，
    /// 点下去撞上一句他看不懂的错（界面从头到尾没告诉他门槛在哪）。
    ///
    /// ⚠️ 期望值是**手写的字面量**（不从 `DownloadTargets` 现取）：现取的话，
    ///    改了那句话两边会一起变，这条断言就恒真了。
    #[test]
    fn the_tree_payload_carries_the_reason_when_the_action_is_unavailable() {
        let huge = an_over_budget_set();
        let (session, _) = a_session_with(LoadState::Loaded(a_batch_of(&huge)));

        // 勾选面 = 差一项（真子集）⇒ 发出去的是显式列表 ⇒ 超预算 ⇒ 挡下。
        let value = whole_tree(&session, &a_tree_of(&huge, &huge), Some(&huge[..huge.len() - 1]));

        assert_eq!(
            value["data"]["action"]["blocked_reason"],
            serde_json::json!("选中的项太多，超过一次能发出的上限：请少选一些"),
            "超预算的勾选面必须把理由带回去（前端靠它禁用那个动作）：{value}"
        );
        // ⚠️ 同一组里那两格仍然按**同一个面**算（"按钮叫什么"与"能不能按"是两件事）。
        assert_eq!(value["data"]["action"]["button_title"], serde_json::json!("下载选中"), "{value}");
        assert_eq!(
            value["data"]["action"]["empty_selection_hint"],
            serde_json::Value::Null,
            "{value}"
        );
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
}
