//! **会话状态** —— 与 UI 无关的那一份壳侧状态（阶段 A 后端 5/12）。
//!
//! ## ⚠️ 锁的纪律（本模块**唯一**容易写错的地方）
//!
//! 里面是 `Mutex<Inner>`，**临界区一律极短**（读几个字段、写几个字段）。
//! **绝不在持锁时调用 `kernel::*`** —— 那些函数会阻塞（`load_delivery` 最坏约 91.5 秒），
//! 持锁等着它们会让 `state()` 命令也一起卡住，而前端正是靠轮询 `state()` 显示进度的
//! （规格 §3.5：`state()` 一秒一问）。
//!
//! ⚠️ 本文件里那几处命令名（`state()` / `tree()` / `load()` / `retry()`）**随代际换过两次**：
//!    egui 时代它们直接是视图调用，第二代是本地 HTTP 的路由（`/api/…`），Tauri 那一代
//!    起才是 `invoke` 的命令（规格 §3.4 那张表）。**改的只是这些名字，代码一字未改。**
//!
//! 正确的用法是**先把 `Arc<CoreClient>` 克隆出来、放掉锁、再去调**：
//!
//! ```ignore
//! let Some(client) = session.client() else { /* 没有内核 */ };
//! let tree = kernel::get_tree(&client);      // 锁已经放掉了
//! ```

use std::sync::{Arc, Mutex};

use shell_core::client::CoreClient;
use shell_core::protocol::{EngineState, LoadState};

/// 一次读取拿到的那份状态快照（`state()` 命令就返回它）。
///
/// ⚠️ **R-15/计划任务 7 起，这个类型是 `shell-core` 那一份的再导出**：定义已经搬到
///    `shell_core::session_view`（它是**纯数据**，而 `api` 层的每一个载荷函数都吃它 ——
///    留在 `shell-win` 的话，"前端收到的东西"那条契约就没法在 macOS 上单测，
///    而那正是本代把 `api` 层搬过去的全部理由，规格 §5.3）。
///    这里**只留一个再导出**，让本文件的读者与 `Session::view()` 的签名照旧读得通：
///    本模块里曾经有一份**定义**，两份并存是任务 2 那一步的过渡态，任务 7 收口
///    （`session_view.rs` 的文件头当时就写着"删掉那一份是任务 7 的事"）。
///
/// ⚠️ 它是**值**不是引用：调用方拿到之后锁就放掉了（见上面那段锁的纪律）。
pub use shell_core::session_view::SessionView;

/// 「找内核 + 握手」这件事的**注入点**。
///
/// ⚠️ **为什么是注入而不是直接调**：`locate_core_binary` / `connect` 是**平台集成**
///    （要读 `current_exe`、要 `Command` 起子进程），住在 `main.rs`；
///    而 `Session` 与路由是**与平台无关**的那一层。
///    让 session 直接调它们，会把平台代码拖进 lib 的公开面，也让命令层没法在宿主上测。
///    ⇒ `main.rs` 在**起 Tauri 之前**把它装进来（任务 7）。
///    （第二代修订：此处原文说的是"起服务之前"，第二代的"服务"是那个本地 HTTP 服务，
///     规格 §1.2 把它废掉了 —— 现在要起起来的是 Tauri 本身。）
pub type Connector =
    dyn Fn() -> Result<(std::sync::Arc<CoreClient>, String), String> + Send + Sync + 'static;

struct Inner {
    client: Option<Arc<CoreClient>>,
    engine: EngineState,
    load: LoadState,
    last_error: Option<String>,
    fallback_notice: Option<String>,
    handshake_reply: Option<String>,
    /// 「找内核 + 握手」的注入点。`None` = 还没装（见 [`Session::connector`]）。
    connector: Option<Arc<Connector>>,
    /// **当前那一批是怎么加载进来的**：`(code, base_url)`，`base_url` 是**我们发出去的那一份**。
    ///
    /// ⚠️ **它存在的唯一理由是"内核重启之后要重放这一批"**（见 [`Session::remember_load_request`]）。
    /// ⚠️ 它与 `load` 那一格**同时**成立（都只在一次**成功**的 `load_delivery` 之后写）：
    ///    一条"`Loaded` 但这里没有"的状态在今天**构造不出来**，而那正是重启之后
    ///    "该不该重放"的判据能只看这一格的原因。
    load_request: Option<(String, String)>,
}

/// 会话状态。由 `Arc<Session>` 共享给所有命令线程与连接线程。
///
/// ⚠️ **这里曾经有一格 `heartbeat: Arc<Heartbeat>`**（以及它的 `heartbeat()` 取用点）：
///    第二代靠"页面还在不在发心跳"推出"浏览器窗口关掉了 ⇒ 退出应用"，
///    那台推断需要一格共享的心跳时间戳。**Tauri 那一代把整条推断删掉了**
///    （规格 §3.3：关窗是 `WindowEvent::CloseRequested`，一条显式事件）⇒
///    `heartbeat.rs` 与这一格一起删除。**留在这里是记账，不是"以后可能还要"**。
pub struct Session {
    inner: Mutex<Inner>,
}

impl Session {
    pub fn new() -> Session {
        Session {
            inner: Mutex::new(Inner {
                client: None,
                engine: EngineState::Connecting,
                load: LoadState::Idle,
                last_error: None,
                fallback_notice: None,
                handshake_reply: None,
                connector: None,
                load_request: None,
            }),
        }
    }

    /// 当前活着的内核连接。**克隆出来之后立刻放锁**（锁的纪律见模块头）。
    pub fn client(&self) -> Option<Arc<CoreClient>> {
        self.inner.lock().expect("会话锁中毒").client.clone()
    }

    /// 把连接**取走**（`main.rs` 的收尾那一步用）。取走之后 [`Session::client`] 返回 `None`。
    ///
    /// ⚠️ **为什么要"取走"而不是克隆一份**：`CoreClient::drop` 就是 `shutdown()`（最坏
    ///    3 + 2 秒），所以收尾时要把那条连接**交给一条后台线程**去丢 —— 而那是**所有权的
    ///    转移**，克隆一份是丢不掉的（`Arc` 的最后一个引用决定析构发生在哪条线程上）。
    ///    克隆出来只会把它挪到线程里、而会话自己仍然持着它。
    ///
    /// ⚠️ **它与 [`Session::install_client`] 的关系（那个坑不会在这里重演）**：本函数把
    ///    `Option` **交出去**，**丢的责任在调用方**（`main.rs` 的收尾把它交给一条后台线程）；
    ///    函数体内只是 `take()`，**没有任何东西在这里析构** —— 锁的持有时间与
    ///    `client()` 一样短。所以"旧连接在持锁时被丢掉"那个缺陷**不适用于这里**，
    ///    但也正因为如此，**调用方必须自己安排 drop 的地方**（别在这里加"顺手清一下"的代码）。
    pub fn take_client(&self) -> Option<Arc<CoreClient>> {
        self.inner.lock().expect("会话锁中毒").client.take()
    }

    pub fn view(&self) -> SessionView {
        let inner = self.inner.lock().expect("会话锁中毒");
        SessionView {
            engine: inner.engine.clone(),
            load: inner.load.clone(),
            last_error: inner.last_error.clone(),
            fallback_notice: inner.fallback_notice.clone(),
            handshake_reply: inner.handshake_reply.clone(),
        }
    }

    /// **内核死亡那一档的共享落点**（`shell-core` 的 `KernelDeath` 算出那句话，
    /// 这里只做"写下去、且至多一次"）。
    ///
    /// 返回值 = 这一次真的翻了面。
    pub fn note_kernel_death(&self, reason: String) -> bool {
        let mut inner = self.inner.lock().expect("会话锁中毒");
        if matches!(inner.engine, EngineState::Unavailable(_)) {
            return false;
        }
        inner.engine = EngineState::Unavailable(reason);
        true
    }

    pub fn set_engine(&self, engine: EngineState) {
        self.inner.lock().expect("会话锁中毒").engine = engine;
    }

    /// **内核亲口说"下载引擎还没起来"** ⇒ 引擎那一格翻成 [`EngineState::NotStarted`]。
    ///
    /// **对齐 macOS**：`AppModel.absorb` 的 `case .engineNotStarted?:
    /// engine = .notStarted; transfers = .empty`（`AppModel.swift:1634`）——
    /// 那是**一条**处置，所有调用点都从那里过。
    ///
    /// ## 🔴 为什么必须做（这不是"内部实现差异"，是用户看得见的一格）
    ///
    /// 引擎徽标是工具栏四项之一。内核刚说过"引擎没起来"，而壳那一格还停在
    /// `Running` ⇒ **徽标显示「运行中」，用户以为任务在跑**。
    ///
    /// ## ⚠️ 它会**覆盖** `Unavailable`（与 [`Self::note_kernel_death`] 的"至多一次"方向相反）
    ///
    /// 这不是矛盾，是两个方向的事实不同：
    ///   * `note_kernel_death` 防的是"两个原因互相盖"（先到的那句原因就是事实）；
    ///   * 这一条是**内核答了话**（`engine_not_started` 是内核自己的回执）⇒ 内核进程活着
    ///     ⇒ 那一格上挂着的"内核没了/引擎不可用"**已经不成立**了。
    ///
    /// ⚠️ 少了"覆盖"这一半，一次内核崩溃之后**徽标会永远停在「引擎不可用」** ——
    ///    重启内核、重新连上（`set_engine(NotStarted)` 由连接线程写）、再被这一条挡住，
    ///    用户再也看不到"引擎未启动"这个**正常**状态。macOS 那边是无条件赋值，照办。
    pub fn note_engine_not_started(&self) {
        self.set_engine(EngineState::NotStarted);
    }

    pub fn set_load(&self, load: LoadState) {
        self.inner.lock().expect("会话锁中毒").load = load;
    }

    /// ⚠️ **只在"这一次请求成了"时传 `None`** —— 那一格的契约是**非粘滞**的。
    pub fn set_last_error(&self, error: Option<String>) {
        self.inner.lock().expect("会话锁中毒").last_error = error;
    }

    /// W-2 披露。**只有内核查找那一处会写它**。
    pub fn set_fallback_notice(&self, notice: Option<String>) {
        self.inner.lock().expect("会话锁中毒").fallback_notice = notice;
    }

    /// 装连接器（**`main.rs` 在起 Tauri 之前调**，任务 7）。
    pub fn set_connector(&self, connector: std::sync::Arc<Connector>) {
        self.inner.lock().expect("会话锁中毒").connector = Some(connector);
    }

    /// **回空态**：界面必须退回到"还没加载"的样子，而且**当前批次作废**。
    ///
    /// ## 🔴 它是 macOS `AppModel.resetToEmptyState()` 的对位物（任务 8 审查 I-1）
    ///
    /// 触发点只有一处：**内核亲口说"这一批不存在或已过期"**（结构化码 `no_delivery`）。
    /// macOS 那条路（`absorb` 的 `case .noDelivery?: resetToEmptyState()`）会清掉
    /// `loadedCode` / `loadedBaseURL` / 树 / 传输 / 校验快照 —— 也就是"这一批从界面上消失"。
    ///
    /// ⚠️ **本代要清的是两格**（其余的没有缓存：树 / 传输 / 校验都是每次现问内核）：
    ///   * `load` → [`LoadState::Idle`]（侧边栏的批次摘要、文件页的进度都跟着走）；
    ///   * **`load_request` → `None`** ← 这才是要命的那一格：留着它，下一次内核重启
    ///     （改下载目录 / 点「重试」）会**重放那个死码**，用户看到一次本不该出现的失败
    ///     （而 macOS 在那一刻是**空态**）。
    ///
    /// ## 🔴 一处**与 macOS 的有意偏离（W-6）**：加载自己失败于 `no_delivery` 时
    ///
    /// **macOS 在这一档止于 `.idle`（回空态，一个字都不说）**：
    /// `resetToEmptyState()` 把 `loadState` 置成 `.idle`，而 `performLoadDelivery`
    /// 那条兜底守着 `if mode == .initial, case .loading = loadState`
    /// （`AppModel.swift:1177`）⇒ **那句 `.failed` 根本不会被写**。
    /// 这一条被 **`AppModelTests.swift:581-597` 的 `noDeliveryReturnsToTheEmptyState`
    /// 具名钉住**（`#expect(model.loadState == .idle, "回空态")`）。
    ///
    /// **我们落 [`LoadState::Failed`]（内核原文）** ⇒ 空态页把那句话显示出来 + 给「重试」。
    ///
    /// 🔴 **理由是**：`no_delivery` 恰恰是**用户最需要一句解释**的时刻（他输的码无效/过期了），
    ///    而 macOS 那一刻默默回空态。我们多说一句的是**内核的原文**（约束 3：不自造任何文案）
    ///    ⇒ 这处偏离**只多给了一句原文**，没有多给一句壳编的话。
    ///
    /// ⚠️⚠️ **别把它写成"照 macOS"**（初稿就是这么写的，被定向复审 N-1 推翻）：
    ///    [`spawn_load`] 那一支的顺序是"先 reset、再 `set_load(Failed)`" ——
    ///    **那是我们的行为，不是 macOS 的**。一句"两边一样"的错话会让下一个人
    ///    按"一样"去推理。
    ///
    /// ⚠️ 它**不碰 `last_error`**：那是"你刚才那一下没成"的另一格，与"这一批作废"无关
    ///    （macOS 在这一档是 `.absorbed` —— 连显示都不显示）。
    pub fn reset_to_empty_state(&self) {
        let mut inner = self.inner.lock().expect("会话锁中毒");
        inner.load = LoadState::Idle;
        inner.load_request = None;
    }

    /// 取连接器（`spawn_connect` 用）。
    pub fn connector(&self) -> Option<Arc<Connector>> {
        self.inner.lock().expect("会话锁中毒").connector.clone()
    }

    /// **记下"当前这一批是用什么请求加载进来的"**（`spawn_load` 成功那一支调它）。
    ///
    /// ## 🔴 为什么要记，而且记的必须是**我们发出去**的那一份
    ///
    /// 改下载目录会**重启内核**，而新内核手里**没有**任何清单 —— macOS 在同一个位置
    /// （`restartKernel` → `performLoadDelivery`）会**自动重放**当前那一批，
    /// 于是用户回到文件页**还是原来那批**。不重放的话客户一眼就能看出差别
    /// （他得重新输一次交付码），而本代的口径是"功能与 macOS 完全一致"。
    ///
    /// ⚠️ **`base_url` 记的是请求里那一份**（用户"高级"里手填的那个），
    ///    **不是**内核回显的 `info.base_url`：用户没指定时回显那份是内核的**默认**交付服务器，
    ///    把它当成"用户选的"再发回去，会在**内核默认值变化时静默分叉**（E-5 明禁）。
    ///    这与 `recordLoadedBatch`（历史那一路）记的是同一个来源。
    ///
    /// ⚠️ **只在成功那一支调**（对齐 macOS 的 `loadedCode`）：一次打错的码写进去，
    ///    重放时会把那个错码再发一遍给内核；而"失败"本来就该留在失败态上（有原文可看）。
    pub fn remember_load_request(&self, code: String, base_url: String) {
        self.inner.lock().expect("会话锁中毒").load_request = Some((code, base_url));
    }

    /// 当前那一批的加载请求；`None` = **还没有成功加载过任何一批**（首次启动就是这个）。
    ///
    /// ⚠️ 它是"内核重启之后要不要重放"的**唯一**判据（`spawn_connect` 成功那一支读它）：
    ///    首次连接时它是 `None` ⇒ **天然不重放**，不需要在那边再特判一次。
    pub fn load_request(&self) -> Option<(String, String)> {
        self.inner.lock().expect("会话锁中毒").load_request.clone()
    }

    /// 装上一条刚握完手的连接（`spawn_connect` 成功那一支）。
    ///
    /// ⚠️ **它是 `handshake_reply` 与 `client` 的唯一写入者**：内核回执的**原文**
    ///    只在连接成功的那一刻取得到（`hello` 的响应行），过后再问就没有了 ——
    ///    而"整条架构通了"这件事就是靠这一行证明的（探路 §6 第 3 条）。
    /// ⚠️ **引擎那一格不在这里翻**（`Connecting` → `NotStarted` 由调用方
    ///    [`spawn_connect`] 的那条线程做）：两件事的时机不同 ——
    ///    `install_client` 只陈述"手上有一条连接了"。
    ///
    /// ⚠️⚠️ **旧连接必须在锁外丢**（本计划第九版修订）：换连接时那一格里的旧 `Arc`
    ///    会在这里析构，而 `CoreClient::drop` 就是 `shutdown()`（**最坏 3 + 2 秒**）——
    ///    留在临界区里就等于把 `state()`、`tree()`、`load()` 一起挡住那么久，
    ///    而那与本文件头的锁纪律（"绝不在持锁时做会阻塞的事"）方向相反。
    ///    触发路径是**唯一那条恢复路径**：任意一次成功的 `retry()` 覆盖已有连接时。
    ///    ⇒ 先把旧值 `replace` 出来、**放掉锁**，再丢它。
    pub fn install_client(&self, client: Arc<CoreClient>, handshake_reply: String) {
        let old = {
            let mut inner = self.inner.lock().expect("会话锁中毒");
            let old = inner.client.replace(client);
            inner.handshake_reply = Some(handshake_reply);
            old
        };
        // ⚠️ 这一行**必须在花括号之外**：到这里锁已经放掉了（见上面那段）。
        //    它不是"顺手写的"—— 挪回花括号里就会把临界区变回"可能阻塞 5 秒"。
        drop(old);
    }
}

/// 起一条后台线程做一次 `load_delivery`，结果落回 `session.load`（D6）。
///
/// ⚠️ **锁的纪律**：本函数**先**把 `client` 克隆出来（调用方已经克隆过了），
///    线程里**不再碰** `session` 的锁去做内核调用 —— 只在拿到结果之后短暂地写一次。
pub fn spawn_load(session: &Arc<Session>, client: Arc<CoreClient>, code: String, base_url: String) {
    // ⚠️ **两份句柄**：一份给线程（`move` 进去），一份**留在本线程**给下面那条
    //    「起不了线程」的分支用。只克隆一份的话编译器会报 E0382 —— 而那一支
    //    正需要把话说出去（它会写 `session.load`，前端轮询 `state()` 才看得见）。
    let worker = Arc::clone(session);
    let spawned = std::thread::Builder::new()
        .name("load-delivery".to_string())
        .spawn(move || {
            match crate::kernel::load_delivery(&client, &code, &base_url) {
                Ok(info) => {
                    // ⚠️ **两件事是同一个事实的两面**（成功加载了哪一批）：先记下**请求**，
                    //    再落加载态。下次内核重启时要按这一格重放（见 `remember_load_request`）。
                    //    ⚠️ 顺序无所谓（读者是另一条线程、且它读的是"这一批在不在"），
                    //    但**必须一起写** —— 落了 `Loaded` 而没有记请求，重启之后就静默不重放了。
                    worker.remember_load_request(code, base_url);
                    worker.set_load(LoadState::Loaded(info));
                }
                Err(why) => match why {
                    shell_core::presentation::kernel_death::CallFailure::KernelGone {
                        reason, text,
                    } => {
                        worker.note_kernel_death(reason);
                        worker.set_load(LoadState::Failed(text));
                    }
                    shell_core::presentation::kernel_death::CallFailure::Text(t) => {
                        worker.set_load(LoadState::Failed(t));
                    }
                    // 🔴 内核亲口说"这一批没了"（`no_delivery`）⇒ **当前批次作废**
                    //    （那一步与 macOS 的 `resetToEmptyState()` 同义），**然后**再落失败原文。
                    //    ⚠️⚠️ **"再落失败原文"这一半是我们自己的，不是照 macOS**：
                    //    macOS 那条兜底守着 `case .loading`（`AppModel.swift:1177`），
                    //    而 `resetToEmptyState()` 已经把 `loadState` 置成 `.idle` ⇒
                    //    **macOS 止于 `.idle`、一个字都不说**（`AppModelTests.swift:581-597`
                    //    具名钉住）。我们选择把**内核的原文**说出来 ——
                    //    理由与这件事的完整记账见 `reset_to_empty_state` 的文档。
                    shell_core::presentation::kernel_death::CallFailure::NoDelivery(t) => {
                        worker.reset_to_empty_state();
                        worker.set_load(LoadState::Failed(t));
                    }
                    // 内核说"引擎还没起来"（例如引擎刚崩过一次）：引擎那一格跟着翻
                    // —— 加载仍然是**失败**（这一屏要说的话在 `t` 里），但徽标不该
                    // 还挂着「运行中」。
                    shell_core::presentation::kernel_death::CallFailure::EngineNotStarted(t) => {
                        worker.note_engine_not_started();
                        worker.set_load(LoadState::Failed(t));
                    }
                },
            }
        });
    if let Err(cause) = spawned {
        // ⚠️ 起不了线程是**响亮**的失败，不许静默降级成"那就同步跑吧"——
        //    同步跑正是不变量禁止的那件事（会把连接线程挂死 91 秒）。
        session.set_load(LoadState::Failed(format!(
            "起不了后台线程（{cause}）：内核调用绝不能在连接线程上同步跑。\n\
             补救：把这条原样发给我们。"
        )));
    }
}

/// 起一条后台线程做一次「**重新找一次**内核 + 握手」，成功就把连接装进 session。
///
/// ⚠️ **每次调用都真的问一次注入的连接器**（`retry()` 命令的语义是"重新找一次"）：
///    人类伙伴的验收动作是**把内核放到壳旁边、再点重试**——缓存连接器的返回值
///    会让那条路对第二次之后的每一次点击都失效，而它是**唯一**的恢复路径。
///
/// ⚠️ **这一档（连接器还没装）在真实流程里不可达**：任务 7 的 `main.rs` 一定
///    **先** `set_connector` 再起 Tauri。它存在只是为了让 `retry()` 有一条
///    **不 panic** 的退路 —— 壳自己写的那句话（这一档没有内核原文可登），
///    按 W-2 说清根因与下一步。
///
/// ## 🔴 连上之后**重放当前那一批**（任务 8 按控制者裁决补的）
///
/// 新内核手里**没有**任何清单（它是一个刚起的进程）。而这条路有三个调用点，
/// 三个都需要"用户看不见换内核这件事"：
///   * **改下载目录**（`preferences_set` ⇒ `restart_kernel`）：macOS 在同一个位置
///     （`restartKernel` → `performLoadDelivery`）会自动重放，用户回到文件页还是原来那批；
///   * **`retry()`**：内核崩过之后重连，那一批同样只活在内核里；
///   * 启动那一趟：`Session` 里**没有**批次 ⇒ 天然不重放（判据是 [`Session::load_request`]
///     的 `None`，不需要在这里特判"是不是第一次"）。
///
/// ⚠️ 重放走的是**与 `load()` 命令同一条路**（[`spawn_load`]）⇒ **失败也走同一套回执**：
///    `load` 落到 `LoadState::Failed(内核原文)`（空态页把它显示出来，还带「重试」），
///    内核真没了的时候还会翻引擎那一格。**不许静默吞掉** —— 吞掉的表现是
///    "换完目录批次没了"，而根因在别处（W-2）。
///
/// ⚠️ 重放是**异步**的（又起一条线程）：这条连接线程不该被那 91.5 秒的 `load_delivery`
///    按住 —— 它手里那份 `Arc` 还要供 `state()` 之类的读写用。
pub fn spawn_connect(session: &Arc<Session>) {
    let Some(connector) = session.connector() else {
        session.set_engine(EngineState::Unavailable(
            "连接器还没有装配 —— main 应当在起 Tauri 之前装好它。".to_string(),
        ));
        return;
    };
    // ⚠️ 两份句柄（理由同 `spawn_load`）：一份进线程，一份留给"起不了线程"那一支。
    let worker = Arc::clone(session);
    let spawned = std::thread::Builder::new()
        .name("core-connect".to_string())
        .spawn(move || match connector() {
            Ok((client, reply)) => {
                // ⚠️ `Arc::clone` 出来一份给重放用（`install_client` 要拿走所有权）。
                worker.install_client(Arc::clone(&client), reply);
                // ⚠️ 与 `main.rs` 的 `start_connect` 的 `Reply::Connected` 那一支逐条对齐：
                //    内核只在 `enqueue` 里起引擎，而这个内核进程是壳刚起的 ⇒
                //    握手完成后「引擎未启动」是**事实**，不是猜测。
                worker.set_engine(EngineState::NotStarted);
                // ⚠️ **重放放在最后**（引擎那一格先落）：`preferences_set` 那条命令
                //    等的就是"引擎变成 NotStarted/Unavailable"，它不该被一次重放拖住
                //    —— 重放的结论由 `load` 那一格说（前端在轮询里看得见）。
                if let Some((code, base_url)) = worker.load_request() {
                    worker.set_load(LoadState::Loading);
                    spawn_load(&worker, client, code, base_url);
                }
            }
            Err(why) => worker.set_engine(EngineState::Unavailable(why)),
        });
    if let Err(cause) = spawned {
        // 起不了线程是**响亮**的失败（W-2），不许静默降级成"那就同步跑吧"。
        session.set_engine(EngineState::Unavailable(format!(
            "起不了后台线程（{cause}）：内核调用绝不能在连接线程上同步跑。\n\
             补救：把这条原样发给我们。"
        )));
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shell_core::protocol::LoadState;

    #[test]
    fn a_fresh_session_is_connecting_with_nothing_loaded() {
        let s = std::sync::Arc::new(Session::new());
        let v = s.view();
        assert_eq!(v.engine, EngineState::Connecting);
        assert_eq!(v.load, LoadState::Idle);
        assert!(v.last_error.is_none());
        assert!(v.fallback_notice.is_none());
        // ⚠️ 这一项**在本用例里不可反驳**（这个会话刚建出来、没人给它装过连接）。
        //    ⚠️ **措辞第七版修订**：原文写的是"没有写入者，只有任务 9 的 `install_client`
        //    会写" —— 那句在批次 C 之后**过期了**：`install_client` 已经落在**本文件**里
        //    （`spawn_connect` 成功那一支就是它的调用点），写入者是有的、只是不在本条用例里。
        //    断言与它的位置（"刚建出来的会话手上没有连接"）一字未改。
        assert!(s.client().is_none());
    }

    /// 🔴 **内核说"引擎还没起来" ⇒ 引擎那一格翻成 `NotStarted`**（审查要的那条：
    /// 引擎那一格**真的变了**，不是只断言"发了空快照"）。
    ///
    /// 判别力（两个方向都会红）：
    ///   * 把这一档写成"什么都不做"（今天之前就是这样）⇒ 第一条断言红 ——
    ///     而真机上的表现是**徽标停在「运行中」，而内核刚说过引擎根本没起来**
    ///     （用户照着徽标判断任务在不在跑）；
    ///   * 给它加一个"已经是 `Unavailable` 就不动"的守卫（一个看起来很"稳"的写法）⇒
    ///     第二条断言红 —— 而真机上的表现是**内核崩过一次之后，徽标永远停在
    ///     「引擎不可用」**（重启、重连都翻不回来）。
    #[test]
    fn the_engine_not_started_branch_flips_the_engine_slot() {
        let s = Arc::new(Session::new());

        // ① 从"运行中"翻过来（最常见的路径：引擎崩了一次，内核回 engine_not_started）。
        s.set_engine(EngineState::Running);
        s.note_engine_not_started();
        assert_eq!(
            s.view().engine,
            EngineState::NotStarted,
            "内核说「引擎没起来」⇒ 引擎那一格要跟着它走"
        );

        // ② **会覆盖 `Unavailable`**（与 `note_kernel_death` 的"至多一次"方向相反）：
        //    内核**答了话** ⇒ 引擎活着 ⇒ 那句"内核没了"已经不成立。
        s.set_engine(EngineState::Unavailable("内核崩了".to_string()));
        s.note_engine_not_started();
        assert_eq!(
            s.view().engine,
            EngineState::NotStarted,
            "内核答了话却还被「引擎不可用」按着 ⇒ 那一格再也回不到正常态"
        );
    }

    /// ⚠️ **"至多一次"**：已经是 `Unavailable` 时不再覆盖 —— 先到的那句原因就是事实，
    ///    后到的只是一次附带观测，盖上它等于让横幅在两句话之间跳。
    #[test]
    fn kernel_death_is_recorded_at_most_once() {
        let s = std::sync::Arc::new(Session::new());
        assert!(s.note_kernel_death("第一句".to_string()), "第一次该翻面");
        assert!(!s.note_kernel_death("第二句".to_string()), "第二次不该再翻");
        assert_eq!(s.view().engine, EngineState::Unavailable("第一句".to_string()));
    }

    /// ⚠️ **`note_kernel_death` 不碰 `last_error`**：那一格的契约是**非粘滞**的
    ///    （"下一个成功请求就清"），而内核死亡时横幅本来就优先显示引擎那一句
    ///    （`EngineBanner::of` 里 `Unavailable` 优先于 `last_error`）⇒
    ///    在这里写或清都是多余的，而**写**进去的危害是"重连成功之后留下一句过期的话"。
    ///
    /// ⚠️⚠️ **本用例第二版改过断言**：初稿断言 `last_error.is_none()`，而实现
    ///    **根本不碰那一格**（只写 `engine`）⇒ **必然红**。函数名与上面这句 doc
    ///    说的都是"**不碰**"，只有断言说的是"清成 `None`"—— **三处打架**，
    ///    而前两处才是这个类型要的契约。
    ///    ⇒ 断言改成"**原样保留**"。（实现一行没动。）
    #[test]
    fn kernel_death_does_not_touch_the_non_sticky_error_slot() {
        let s = std::sync::Arc::new(Session::new());
        s.set_last_error(Some("旧的瞬时错误".to_string()));
        s.note_kernel_death("内核没了".to_string());
        assert_eq!(
            s.view().last_error.as_deref(),
            Some("旧的瞬时错误"),
            "内核死亡碰了那一格 ⇒ 它只该翻 engine，不该写、也不该清 last_error"
        );
    }

    /// ⚠️ **`fallback_notice` 是粘滞的**（W-2 披露）：它是另一格，不该被清空逻辑碰到。
    ///    这条抓的是"两格被合成一格"那个已修复过的缺陷（`main.rs` 的 `engine_fallback_notice`）。
    #[test]
    fn the_fallback_notice_survives_a_successful_call() {
        let s = std::sync::Arc::new(Session::new());
        s.set_fallback_notice(Some("退回了同目录那份内核".to_string()));
        s.set_last_error(Some("一次失败".to_string()));
        s.set_last_error(None); // 下一个成功请求清掉瞬时错误
        assert!(s.view().fallback_notice.is_some(), "粘滞披露被清掉了");
        assert!(s.view().last_error.is_none());
    }

    // ⚠️ 这里**故意没有**"`handshake_reply` 是 `None`"那条用例（初稿写过，被复审拿下）：
    //    ⚠️ **理由在批次 C 之后要分开说**（第七版修订）：
    //      · 写那个字段的人**现在有了**（`install_client`，本文件里）；
    //      · 但"刚建出来的会话里它是 `None`"这条断言**仍然不可能变红** ——
    //        它陈述的是 `Session::new()` 的初始化，而**没有任何输入能让它不成立**。
    //        ⇒ 那是本仓库明令禁止的形态（"不可能变红的断言"），所以它仍然不写。
    //      · 真正有牙的那条（"装上一条连接之后，回执原文就在那里了"）属于**本文件**、
    //        也该在这儿 —— 但它是**端到端**：要有连接，就得先有人把连接器装进去，
    //        而那只发生在 `main.rs`（任务 7 的装配）里 ⇒ 它得在真起一次壳的路上去验，
    //        不是本文件这种"喂一个替身"的用例能盖住的。
    //        （第二代修订：此处原文说的是"任务 9 的 `spawn_connect` 走通之后" ——
    //         `spawn_connect` 已经落在本文件里了，那句"留给那里"指的那个未来已经过去。）
    //    （本文件里没有"不可能变红的断言是禁止的"那句话——它在计划的散文里，别搜。）

    // -----------------------------------------------------------------------
    // 锁纪律：**换连接时那条旧的不能在临界区里丢**（本计划第九版修订）
    // -----------------------------------------------------------------------

    use shell_core::client::{ClientError, LineChannel};
    use std::time::{Duration, Instant};

    /// 一条 `close()` 会**慢**的假通道：它先报信"我开始关了"，再睡一会儿。
    ///
    /// ⚠️ 那个 `Sender` 就是这条用例的**同步点**：没有它，断言只能靠"猜另一个线程跑到哪了"
    ///    （那会抖）。有了它，"开始关了"与"锁被放掉了没有"之间就没有窗口。
    struct SlowClose {
        started: std::sync::mpsc::Sender<()>,
        how_long: Duration,
    }

    impl LineChannel for SlowClose {
        fn write_line(&self, _line: &str) -> Result<(), ClientError> {
            Ok(())
        }
        fn read_line(&self) -> Result<Option<String>, ClientError> {
            Ok(None) // EOF：这条用例根本不发请求
        }
        fn close(&self) {
            // ⚠️ **先报信、再睡**：睡的是"`shutdown()` 要花的那段时间"。
            let _ = self.started.send(());
            std::thread::sleep(self.how_long);
        }
    }

    /// ⚠️ **换连接时，旧连接的 `shutdown()` 绝不许把会话锁按住**。
    ///
    /// 触发路径是**唯一那条恢复路径**：一次成功的 `retry()` 覆盖已有连接时，
    /// 旧 `Arc<CoreClient>` 会在那一格里被替换掉 ⇒ 它在**替换的那条线程上**析构，
    /// 而 `CoreClient::drop` = `shutdown()`（最坏 3 + 2 秒）。若这次析构发生在临界区里，
    /// `state()`、`tree()`、`load()` 会被一起挡住那么久 —— 那与本文件头的锁纪律
    /// （"绝不在持锁时做会阻塞的事"）**方向相反**，而且**不会有任何东西变红**。
    ///
    /// 判别力（**突变实测过**：把 `install_client` 里那句 `drop(old)` 挪回花括号里
    /// —— 即改回 `inner.client = Some(client)` 那个写法 —— 这一条立刻红，
    /// 报的是 `view()` 被挡了约 300 ms；输出贴在任务 9 报告里）。
    #[test]
    fn swapping_the_client_never_holds_the_lock_while_the_old_one_shuts_down() {
        /// 旧连接"关"一次要花的时间。取得足够大：它要**盖过**调度抖动，
        /// 又要**远小于**"一条用例永远不结束"（失败形态是红，不是挂住）。
        const SLOW: Duration = Duration::from_millis(300);

        let s = Arc::new(Session::new());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        // 第一条连接：装它时那一格是空的 ⇒ 没有东西要丢（这一步本来就不慢）。
        s.install_client(
            Arc::new(CoreClient::new(Box::new(SlowClose {
                started: started_tx.clone(),
                how_long: SLOW,
            }))),
            "第一次".to_string(),
        );

        // 第二条连接在**另一条线程**上装：它会把第一条替换出来丢掉。
        let swapper = {
            let s = Arc::clone(&s);
            std::thread::spawn(move || {
                s.install_client(
                    Arc::new(CoreClient::new(Box::new(SlowClose {
                        started: started_tx,
                        how_long: SLOW,
                    }))),
                    "第二次".to_string(),
                );
            })
        };

        // 等到"旧连接已经开始关了"——从这一刻起，锁若还held着，就是被那次 shutdown 按住的。
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("换连接必须真的把旧连接丢掉（它的 close 没被调到）");

        let started = Instant::now();
        let view = s.view();
        let took = started.elapsed();

        assert!(
            took < SLOW / 2,
            "换连接时 view() 被挡了 {took:?}：旧连接的 shutdown 跑在临界区里了\
             （锁纪律：临界区只许读几个字段、写几个字段）"
        );
        // 顺带钉住"换完之后状态是对的"（否则上面那条可以靠"什么都不做"变绿）。
        assert_eq!(
            view.handshake_reply.as_deref(),
            Some("第二次"),
            "换连接必须真的换掉那一格（回执原文也要跟着更新）"
        );

        swapper.join().expect("换连接的那条线程不该 panic");
    }

    // -----------------------------------------------------------------------
    // 🔴 新内核一连上就**重放当前那一批**（任务 8 按控制者裁决补的）
    //
    // 背景：改下载目录会重启内核，而新内核手里没有任何清单 —— macOS 在同一个位置
    // （`restartKernel` → `performLoadDelivery`）会自动重放，用户回到文件页还是原来那批。
    // 下面三条钉住：**重放用哪一份 `base_url`** / **失败可见** / **首次连接不重放**。
    // -----------------------------------------------------------------------

    /// 一条**脚本化的内核通道**：按顺序吐预置的响应行，并记下收到的每一条请求。
    ///
    /// ⚠️ 与 `kernel.rs` 测试里那个同名替身是同形（那边验的是"发出去的请求长什么样"，
    ///    这边验的是"重放时发出去的请求长什么样"）。两处**没有合并**：它们住在各自的
    ///    `mod tests` 里（本 crate 的测试不许跨模块借夹具），而各自的夹具只需要
    ///    `LineChannel` 那三个方法 —— 合并的收益（省 20 行）小于"测试之间互相依赖"的代价。
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
        /// 到目前为止收到的请求行（按顺序）。
        fn requests(&self) -> Vec<String> {
            self.written.lock().unwrap().clone()
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

    /// 一条合法的 `DeliveryInfo` 回执（`protocol.rs` 那个类型**没有任何 `serde(default)`**，
    /// 九个键一个都不能少；`tree` 的合法空树是 `{}`）。
    fn delivery_info(code: &str, base_url: &str, total_files: i64) -> String {
        format!(
            r#"{{"code":"{code}","page_url":"","base_url":"{base_url}","created_at":"","expires_at":"","expired":false,"total_files":{total_files},"total_bytes":0,"tree":{{}}}}"#
        )
    }

    fn ok_reply(id: u64, result: &str) -> String {
        format!(r#"{{"id":{id},"ok":true,"result":{result}}}"#)
    }

    fn err_reply(id: u64, code: &str, message: &str) -> String {
        format!(r#"{{"id":{id},"ok":false,"error":{{"code":"{code}","message":"{message}"}}}}"#)
    }

    /// **有界地**等一件事成立（那些活跑在别的线程上）。返回它最终成立没有。
    ///
    /// ⚠️ 上界 5 秒：这几条用例里最长的一段是"起线程 + 走一遍假通道"（毫秒级）。
    ///    真到了 5 秒还没成立，说明那件事**不会**发生了 —— 返回 `false` 让断言去报，
    ///    而不是把测试挂死（挂死的测试在 CI 上是一条读不出根因的红）。
    fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        ready()
    }

    /// ⭐ **换了一个内核之后，壳会把当前那一批重放过去 —— 而且用的是"请求时"那个
    /// `base_url`，不是内核回显的那一份。**
    ///
    /// 判别力（三个方向都会红）：
    ///   * 不重放（删掉 `spawn_connect` 里那一段）⇒ 新内核上一条请求都没有 ⇒ 红；
    ///   * 重放时用了回显那份 ⇒ `!requests[0].contains(ECHOED)` 那一条红
    ///     （而真机上的后果是 E-5 说的静默分叉：用户没指定服务器，壳却"替他"指定了一个）；
    ///   * 记的是"回显"而不是"请求" ⇒ 第二条断言（`load_request()`）先红。
    #[test]
    fn a_new_kernel_replays_the_current_batch_with_the_requested_base_url() {
        /// 用户手填的那个（**请求**里带的就是它）。
        const REQUESTED: &str = "https://user.example";
        /// 内核回显的那份（用户没指定时它是内核的**默认**服务器）。
        const ECHOED: &str = "https://default.example";

        let session = Arc::new(Session::new());

        // ① 第一次加载：请求带 REQUESTED，内核回显 ECHOED（total_files = 0）。
        let first = Scripted::new(&[&ok_reply(1, &delivery_info("C24-8", ECHOED, 0))]);
        spawn_load(
            &session,
            Arc::new(CoreClient::new(Box::new(first.clone()))),
            "C24-8".to_string(),
            REQUESTED.to_string(),
        );
        assert!(
            wait_until(|| matches!(session.view().load, LoadState::Loaded(_))),
            "第一次加载没有落地"
        );
        assert_eq!(
            session.load_request(),
            Some(("C24-8".to_string(), REQUESTED.to_string())),
            "记下的必须是**请求时**那一份，不是内核回显的（E-5）"
        );

        // ② 换一个内核（这就是"改下载目录重启"那一步的实质）。
        //    ⚠️ 这一份的回执 total_files = 7：重放**真的跑过**才会有 7。
        let second = Scripted::new(&[&ok_reply(1, &delivery_info("C24-8", ECHOED, 7))]);
        let connector: Arc<Connector> = {
            let channel = second.clone();
            Arc::new(move || {
                Ok((
                    Arc::new(CoreClient::new(Box::new(channel.clone()))),
                    "{}".to_string(),
                ))
            })
        };
        session.set_connector(connector);
        spawn_connect(&session);

        assert!(
            wait_until(|| match &session.view().load {
                LoadState::Loaded(info) => info.total_files == 7,
                _ => false,
            }),
            "新内核上那一批没有被重放（load 停在 {:?}）",
            session.view().load
        );

        // ③ 重放发出去的请求（**逐字**）。
        let requests = second.requests();
        assert_eq!(requests.len(), 1, "新内核上该**只**有一条重放请求：{requests:?}");
        assert!(
            requests[0].contains(r#""method":"load_delivery""#),
            "重放发的必须就是 load_delivery：{}",
            requests[0]
        );
        assert!(requests[0].contains(r#""code":"C24-8""#), "{}", requests[0]);
        assert!(
            requests[0].contains(REQUESTED),
            "重放用的必须是**请求时**那个 base_url：{}",
            requests[0]
        );
        assert!(
            !requests[0].contains(ECHOED),
            "不许把内核回显的那份当成用户的选择（E-5）：{}",
            requests[0]
        );
    }

    /// 🔴 **重放失败会留下可见的回执**（`load` 那一格 —— 空态页显示内核原文 + 「重试」）。
    ///
    /// 判别力：把重放的失败吞掉（例如只 `let _ =`），这一条立刻红 ——
    /// 而真机上的表现是"换完下载目录，批次没了，界面上一个字都不说"
    /// （用户以为是他自己弄丢的，而根因在内核那边）。
    #[test]
    fn a_failed_replay_leaves_a_visible_failure_in_the_load_slot() {
        let session = Arc::new(Session::new());
        session.remember_load_request("C24-8".to_string(), "https://user.example".to_string());

        let channel = Scripted::new(&[&err_reply(1, "delivery_fetch_failed", "拉取交付清单失败：连接超时")]);
        let connector: Arc<Connector> = {
            let channel = channel.clone();
            Arc::new(move || {
                Ok((
                    Arc::new(CoreClient::new(Box::new(channel.clone()))),
                    "{}".to_string(),
                ))
            })
        };
        session.set_connector(connector);
        spawn_connect(&session);

        assert!(
            wait_until(|| matches!(session.view().load, LoadState::Failed(_))),
            "重放失败必须落在 load 那一格（不能静默）：{:?}",
            session.view().load
        );
        assert_eq!(
            session.view().load,
            LoadState::Failed("拉取交付清单失败：连接超时".to_string()),
            "内核的失败原文要**逐字**照登（约束 3）"
        );
    }

    /// 一个**假连接器**（每次都造一条新的脚本化通道）—— 与上面那两条重放用例共用同一套。
    fn a_connector(channel: Scripted) -> Arc<Connector> {
        Arc::new(move || {
            Ok((
                Arc::new(CoreClient::new(Box::new(channel.clone()))),
                "{}".to_string(),
            ))
        })
    }

    /// 🔴 **批次作废之后重启，不会重放那个死码**（任务 8 审查 I-1 的正面判据）。
    ///
    /// 判别力：把 `reset_to_empty_state` 里清 `load_request` 那一行删掉，这一条立刻红 ——
    /// 而真机上的表现是：内核说"这一批没了"、界面回了空态，用户改一次下载目录
    /// （或点一次「重试」）之后，壳**又把那个死码发了一遍**，用户看到一次
    /// **本不该出现的失败**（macOS 在那一刻是空态）。
    #[test]
    fn a_restart_after_the_batch_was_invalidated_does_not_replay_it() {
        let session = Arc::new(Session::new());
        session.remember_load_request("C24-8".to_string(), "https://user.example".to_string());

        // 内核亲口说"这一批没了" ⇒ 回空态（macOS 的 `resetToEmptyState()`）。
        session.reset_to_empty_state();
        assert_eq!(
            session.load_request(),
            None,
            "批次作废之后不许再记着那一批（否则它会被重放）"
        );
        assert_eq!(session.view().load, LoadState::Idle, "界面也要退回「还没加载」的样子");

        // 换一个内核（这就是"改下载目录重启"那一步的实质）：**一条请求都不该发**。
        let channel = Scripted::new(&[&ok_reply(1, &delivery_info("C24-8", "", 0))]);
        session.set_connector(a_connector(channel.clone()));
        spawn_connect(&session);
        assert!(
            wait_until(|| session.view().engine == EngineState::NotStarted),
            "连接该成功：{:?}",
            session.view().engine
        );
        assert!(
            channel.requests().is_empty(),
            "批次作废之后重启**不许重放**那个死码：{:?}",
            channel.requests()
        );
        assert_eq!(session.view().load, LoadState::Idle, "也不该有人去动加载态");
    }

    /// ⚠️ **失败的加载不许改写"当前批次"**（macOS 的 `loadedCode` 只在成功那一支赋值）。
    ///
    /// 判别力：把 `remember_load_request` 挪到"请求发出去之前"（或给失败支也记一笔），
    /// 这一条立刻红 —— 而真机上的表现是：用户打错一个码、加载失败之后，
    /// **下一次内核重启会去重放那个打错的码**（而 macOS 重放的是**上一次成功的那批**）。
    #[test]
    fn a_failed_load_keeps_the_previous_batch() {
        const REQUESTED: &str = "https://user.example";
        let session = Arc::new(Session::new());

        // ① 先成功加载一批。
        let first = Scripted::new(&[&ok_reply(1, &delivery_info("C24-8", "", 0))]);
        spawn_load(
            &session,
            Arc::new(CoreClient::new(Box::new(first))),
            "C24-8".to_string(),
            REQUESTED.to_string(),
        );
        assert!(wait_until(|| matches!(session.view().load, LoadState::Loaded(_))));

        // ② 再加载一个**别的**码，它失败了（**不是** `no_delivery`）。
        let second = Scripted::new(&[&err_reply(1, "delivery_fetch_failed", "拉取交付清单失败")]);
        spawn_load(
            &session,
            Arc::new(CoreClient::new(Box::new(second))),
            "打错的码".to_string(),
            "https://other.example".to_string(),
        );
        assert!(wait_until(|| matches!(session.view().load, LoadState::Failed(_))));

        // ③ **仍然记着**上一次成功的那一批（失败那一次一个字都没写进去）。
        assert_eq!(
            session.load_request(),
            Some(("C24-8".to_string(), REQUESTED.to_string())),
            "失败的加载改写了当前批次 ⇒ 下一次重启会去重放那个打错的码"
        );
    }

    /// 🔴 **加载**自己撞上 `no_delivery` 时：**先作废批次、再落失败原文**。
    ///
    /// ⚠️⚠️ **"落失败原文"这一半是我们与 macOS 的**有意偏离**（W-6），不是"照做"**：
    ///    macOS 在这一档**止于 `.idle`** —— `resetToEmptyState()` 之后那句 `.failed`
    ///    被 `case .loading` 的守卫挡住（`AppModel.swift:1177`），
    ///    而 `AppModelTests.swift:581-597` 具名钉着"回空态、不是加载失败"。
    ///    我们多说一句**内核原文**（约束 3：不自造文案），因为 `no_delivery` 正是用户
    ///    最需要解释的时刻。完整理由见 `Session::reset_to_empty_state` 的文档。
    ///
    /// 判别力（两个方向都会红）：只 reset 不落原文 ⇒ 用户什么提示都看不到
    /// （**那正是 macOS 的行为**，但我们不要它）；只落原文不 reset ⇒ 那个死码
    /// 下一次重启还会被重放（I-1）。
    #[test]
    fn a_load_that_hits_no_delivery_clears_the_batch_and_still_reports_the_text() {
        let session = Arc::new(Session::new());
        session.remember_load_request("C24-8".to_string(), "https://user.example".to_string());

        let channel = Scripted::new(&[&err_reply(1, "no_delivery", "这一批不存在或已过期")]);
        spawn_load(
            &session,
            Arc::new(CoreClient::new(Box::new(channel))),
            "C24-8".to_string(),
            "https://user.example".to_string(),
        );
        assert!(wait_until(|| matches!(session.view().load, LoadState::Failed(_))));

        assert_eq!(
            session.view().load,
            LoadState::Failed("这一批不存在或已过期".to_string()),
            "内核原文要逐字落进加载态（用户看得见）"
        );
        assert_eq!(
            session.load_request(),
            None,
            "这一批已经作废 ⇒ 不许再记着它（否则下一次重启会重放）"
        );
    }

    /// ⚠️ **首次连接一条请求都不发**（`Session` 里没有批次 ⇒ 天然不重放，不用特判）。
    ///
    /// 判别力：若有人在 `spawn_connect` 里写成"无条件重放一次"（比如拿一个空码去发），
    /// 这一条立刻红 —— 而真机上的表现是**每次启动都多一发注定失败的请求**，
    /// 而它的失败还会顶掉页面上的空态（用户什么都还没做，就看到一句内核的报错）。
    #[test]
    fn a_first_connection_never_replays_anything() {
        let session = Arc::new(Session::new());
        // 备一条回执在那儿：它**不该**被用到（用了就是重放了）。
        let channel = Scripted::new(&[&ok_reply(1, &delivery_info("C24-8", "", 0))]);
        let connector: Arc<Connector> = {
            let channel = channel.clone();
            Arc::new(move || {
                Ok((
                    Arc::new(CoreClient::new(Box::new(channel.clone()))),
                    "{}".to_string(),
                ))
            })
        };
        session.set_connector(connector);
        spawn_connect(&session);

        assert!(
            wait_until(|| session.view().engine == EngineState::NotStarted),
            "连接该成功：{:?}",
            session.view().engine
        );
        assert!(
            channel.requests().is_empty(),
            "首次连接一条请求都不该发：{:?}",
            channel.requests()
        );
        assert_eq!(session.view().load, LoadState::Idle, "也**不许**动加载态");
    }
}
