//! kernel —— **编排与副作用**。业务逻辑一律在模块里，这里只接线。
//!
//! 与 Go 版 `main.go` 只接线的纪律同源：解析清单、扫描分类、四态合成、进度、
//! 参数校验、复校验全部在 `delivery`/`planner`/`view`/`settings`/`verify` 里实现并有单测，
//! 本文件负责把它们接到 stdio 的 JSON Lines 协议上（设计规格 §5、§5.2 的方法集）。
//!
//! # 三条贯穿本文件的性质
//!
//! 1. **stdout 是协议专用通道**（全局约束 1）：每行一条 JSON，除此之外不写 stdout。
//!    诊断一律 `eprintln!`（stderr）。本文件会起 aria2c 子进程，而 `engine::daemon`
//!    已经把它自己的 stdout 设成 `Stdio::null()`——三层里任何一层都不许漏到协议通道上。
//! 2. **内核不含任何界面概念**（全局约束 2）：方法集里没有 `reveal`，
//!    `task_action` 的取值只有 aria2 真的有的那几个动作。
//! 3. **校验在锁外跑**（契约 §5.2，承重）：见 [`spawn_verify_worker`] 的注释——
//!    `verify::check` 的调用点与 `Mutex` guard 的作用域**在代码上就是分开的**。
//! 4. **读路径上最贵的网络 I/O 也在锁外跑**（契约 §5.2 的同一句话，"任何长持有的锁"）：
//!    `load_delivery` 的 `fetch`（≤91.5 s）、`clear_engine_batch`（≤约 40 s）、
//!    以及一切 `plan`（对每个 crc64 为空的文件一次 HEAD，≤30 s × N）
//!    ——三段都拆成"锁外取数 / 锁内安装"，见 [`plan_off_lock`] 与 [`ensure_complete`]。
//!    `ensure_complete` 被 `list_dir`/`get_tree`/`enqueue`/`tree_json` 的路径调用，
//!    所以这不在 `load_delivery` 一条路上。
//!
//! # 为什么它在 lib 里（2026-09-21）
//!
//! 本文件原先就是 `main.rs` 的正文，任务 2 把它**整段搬**过来（逻辑一个字未改，
//! 只加 `pub`）：本 crate 现在有**两个入口** —— `benagen-core`（图形客户端的后端，
//! 走 stdio 行协议）与 `benagen-dl`（命令行工具），两者的编排语义必须**只有一份**。
//! 两个 bin 都只是它的壳：`main.rs` 只剩 stdio 协议循环。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::autoretry;
use crate::delivery::{Manifest, UreqTransport};
use crate::engine::daemon::{Daemon, DaemonOptions};
use crate::engine::legs::leg_is_ip_literal;
use crate::engine::rpc::WriteOutcome;
use crate::engine::status::{Task, TaskState};
use crate::engine::supervisor::{Ladder, Liveness, Progress, Rung, TaskProgress, Thresholds};
use crate::intent::{self, Intent};
use crate::legtrim;
use crate::migrate;
use crate::planner::{Kind, Options as PlanOptions, Planner};
use crate::protocol::{codes, TransferItem};
use crate::settings::Settings;
use crate::state::State as FileState;
use crate::view::FileState as ViewFileState;
use crate::{delivery, engine, kcodes, planner, protocol, settings, state, update, verify, view};

pub use crate::protocol::{ErrorBody, Request, Response};

// ---------------------------------------------------------------------------
// 读循环的两条决定（简报第 13 条）
// ---------------------------------------------------------------------------

/// **行长上限：8 MiB。**
///
/// `BufRead::read_line` 会**无上限地**增长缓冲区——一个坏掉或恶意的壳可以让内核吃掉
/// 任意内存。协议层不做限制是对的（在那里限制就意味着要么截断要么拒，而"拒"需要
/// 调用方先读完），所以这个决定归调用方，也就是这里。**本内核选择设上限**：
///
/// - 取 8 MiB 而不是更小：`read_request` 明确接受 1 MiB 的合法请求
///   （`read_request_handles_very_long_line` 钉着"不许有隐含行长上限"），
///   8 倍余量足够任何真实请求；协议里也没有任何一个方法需要接近这个量级的参数。
/// - 超限**报结构化错误**而不是静默截断：截断会把一条请求变成另一条请求。
/// - 报错之后**不排空**这一行的剩余字节：`Take` 只吃掉上限那么多，剩下的留在流里，
///   下一次 `read_until` 从断点继续。碎片会被 JSON 解析自然拒掉（再回一条
///   `bad_request`），直到真正的换行到达、协议重新对齐。**这条路的内存有界、时间有界、
///   且不需要一段"排空到换行为止"的循环**（那样反而给了恶意壳一个无界的忙等入口）。
const MAX_LINE_BYTES: u64 = 8 << 20;

/// 一次读的三种结局。
pub enum LineOutcome {
    /// 读到了一行（已剥掉行尾符）。
    Line(Vec<u8>),
    /// 流结束。
    Eof,
    /// 超过 [`MAX_LINE_BYTES`] 仍然没有换行。
    TooLong,
}

/// 读一行，**带上限**。见 [`MAX_LINE_BYTES`]。
pub fn read_line_capped<R: BufRead>(r: &mut R) -> std::io::Result<LineOutcome> {
    let mut buf: Vec<u8> = Vec::new();
    let n = r.by_ref().take(MAX_LINE_BYTES).read_until(b'\n', &mut buf)?;
    if n == 0 && buf.is_empty() {
        return Ok(LineOutcome::Eof);
    }
    let ended = buf.last() == Some(&b'\n');
    if !ended && buf.len() as u64 >= MAX_LINE_BYTES {
        return Ok(LineOutcome::TooLong);
    }
    if ended {
        buf.pop();
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(LineOutcome::Line(buf))
}

// ---------------------------------------------------------------------------
// 内核状态
// ---------------------------------------------------------------------------

/// 一次校验任务。
pub struct VerifyJob {
    dir: PathBuf,
    files: Vec<delivery::File>,
    /// `Some(第一轮结果)` 表示这是**复校验轮**：结果要与第一轮 `verify::merge`。
    first: Option<verify::CheckResult>,
}

pub struct Kernel {
    download_dir: PathBuf,
    settings_path: PathBuf,
    last_code_path: PathBuf,

    settings: Settings,
    /// **已解析**的界面语言（永远是 `Zh`/`En`，不会是 `System`）。
    ///
    /// 来源依次是：`hello.language` > `settings.language` > 系统 locale。
    /// 所有客户可见文案（错误、提示、进度）都查这一格，不各自去读环境变量。
    pub language: crate::i18n::Lang,
    /// 壳在 `hello` 里交回来的系统界面语言（`zh-Hans-CN` …）。
    ///
    /// 为什么留着它：设置页改「跟随系统」时也要按**系统语言**解析一次，而
    /// `set_settings` 的入参里没有这一格 —— 记下来，别退回去读环境变量
    /// （图形壳里那是空的，见 `protocol::HelloParams::system_locale`）。
    pub system_locale: Option<String>,
    /// 「客户/壳有没有明确选过界面语言」（规格 W1 的首启判据）。
    ///
    /// 判据是设置文件里**有没有 `language` 这个键**（见 `settings::load_from_with_flag`），
    /// 不是"值是不是 system"：主动选了「跟随系统」也算选过。
    pub language_chosen: bool,
    /// 双域名竞速的进程内缓存（规格 W4.2）：`code → 上次命中的那一侧`。
    pub race: crate::delivery_race::Cache,
    /// 最近一次解析交付码的过程（四句人话，逐条追加，规格 W4.3）。
    pub resolve_lines: Vec<String>,
    /// 解析状态：`idle` / `resolving` / `hit_domestic` / `hit_international` / `failed`。
    pub resolve_state: &'static str,
    /// 命中的那一侧（没有就是 `None`）。
    pub resolve_origin: Option<crate::delivery_race::Origin>,

    /// 自动重启账本（规格 W3）：第 2/3 层。探活失败的计数、退避、预算都在里面。
    pub restart: crate::engine::restart::Gate,

    /// 最近 30 秒内见过的客户可见错误（同类只记一次，规格 W2）。
    /// 壳通过 `error_recent` 读它——界面画的是"这一类又发生了 N 次"，
    /// 而不是被上千条同样的失败刷屏。
    pub errors: crate::errors::Recent,
    /// `None` = 还没有批次（或已被换码作废）。
    manifest: Option<Manifest>,
    /// 上次用过的交付码。**单独一个小文件**（简报第 6 条），不进 `settings`。
    last_code: String,

    /// 更新状态文件（`update.json`）的路径。**独立于 `settings.json`** —— 那些是
    /// 下载参数、面板上一行一项；这个是运行状态（与 `last_code` 同层）。
    update_path: PathBuf,
    /// 更新检查的运行状态（开关 / 上次查于 / 上次看到的版本）。
    update: update::State,
    /// **手动查那颗按钮唯一的反馈**：正在查时为真。壳在**另一条连接**上轮询
    /// `update_status` 时看得到它 —— 所以置真/置假必须发生在**锁内**（见 [`run_update_check`]）。
    update_checking: bool,

    /// **上一次检查到底查成没有**：`Some(true)` 查成了、`Some(false)` 没查成、
    /// `None` = **从没查过**（三档不许并成两档 —— "没查过"与"没查成"要说得出不一样）。
    ///
    /// 🔴 **为什么非要有这一格**：手动查那颗按钮要**当场**回答"有没有新版本"，
    ///    而"没查成"（断网 / 发布页挂了）与"查成了、确实没有新的"在
    ///    [`update::State::last_seen`] 上**长得一模一样**（两件事都不改它）——
    ///    只按 `has_newer` 判的话，断网时那颗按钮会对客户说**"已经是最新的"**，
    ///    那是**谎报**。规格 §5 那条"失败一律静默"说的是**界面上不许冒看不懂的报错**，
    ///    不是**替客户把没查成说成查成了**。
    ///
    /// ⚠️ **与 `update_checking` 同层：运行状态，不落盘**（`update.json` 里那三格是持久状态）。
    ///    这一格只在"刚点完那颗按钮"的那一次对话里有意义；上一次开机的检查结果与
    ///    **这一次**点按钮的结果没关系，落盘只会让壳读到一份过期结论
    ///    （而每一次点按钮都会自己产生一份新的：`update_check_now` 查完才回状态）。
    update_last_check_ok: Option<bool>,

    /// 已校验文件的记录（跨会话，落盘）。
    state: FileState,
    /// 六类校验结果的累积。
    verify: verify::CheckResult,
    /// `{ f.path | plan 结果的对应条目 kind == Kind::Skip }`（简报第 8 条）。
    /// **只由 [`complete_from`] 推出来**，三个调用点（`ensure_complete`、
    /// `op_load_delivery`、非严格的 `op_plan`）都走它——`compose`/`progress` 都用它，
    /// 别处不得自行推导（契约 §1.7 的教训就是同一条规则有多份实现并漂移出了真缺陷）。
    complete: BTreeSet<String>,
    /// `complete` 是否已过期（状态变了就置位，下次要用时才算）。
    complete_dirty: bool,

    /// 正在校验的路径（防止落地监听器重复派活）。
    verifying: BTreeSet<String>,
    /// 已经出过最终结论的路径（"不再重试"的依据）。
    verified: BTreeSet<String>,
    /// 已经自动重入队过一次的路径（简报第 9 条：**只重入队一次**）。
    retried: BTreeSet<String>,

    daemon: Option<Arc<Daemon>>,
    /// 引擎是否已被判定断开（设计规格 §9）。判定之后每次读都会试一次重连。
    engine_disconnected: bool,
    /// 引擎进程**自己**在动吗（写盘 / 烧 CPU，见 [`crate::procstat::is_busy`]）。
    ///
    /// 🔴 **这一格是"RPC 不回话"与"引擎死了"之间唯一的判别证据**：自愈线程每拍采一次
    ///    写在这里，`settle_rpc_failure` 在判定断开之前读它。没有它的时候，引擎一边
    ///    一秒写几十 MB、一边被判死，于是档 2 暂停全部、档 3 换引擎 + 重新入队，
    ///    75 分钟里反复 3 次直到放弃（现场 `diag-kernel.log`，0.2.7）。
    ///
    /// ⚠️ 非 Windows 平台上 `procstat::read` 恒失败 ⇒ 这一格恒为 `false` ⇒ 行为与
    ///    引入它之前**逐字相同**（保守的那一边：按"没在动"处理）。
    engine_busy: bool,
    /// §6 的量化证据。挂在 Kernel 上而不是线程局部：它跨换引擎存活（换的只是 `daemon` 字段），
    /// 而 RPC 的耗时是在协议循环里量的，够不到线程局部。用原子量，不占内核锁。
    counters: Arc<crate::diag_counters::Counters>,

    /// 「用户想下的文件集」（规格 2026-10-12 §3）：换引擎 / 重启内核后据此补回队列。
    intent: Intent,
    /// 上一次给引擎进程采的资源读数（做差用，规格 §4）。
    stall_prev: Option<crate::procstat::Counters>,
    /// 换目录时搬运旧文件的进度（规格 §6）。`Arc` 是因为后台线程要自己持有一份。
    migrate: Arc<Mutex<migrate::Status>>,
    /// 用户按了「暂停全部」且还没「一键开始」：自动重试必须**按住不动**——
    /// 否则被暂停期间失败的任务会被它悄悄重新拉起来，等于没暂停。
    /// 入队**不**解除它：暂停期间新加入的文件一律以暂停态加入（见 [`submit_off_lock`]）。
    hold_auto_retry: bool,
    /// 我们自己 `pause` 了、但 `unpause` 没还上的 gid（档 1）。
    ///
    /// 写方法从不重试，而暂停态属于 `Waiting`、被 `active_tasks` 滤掉 ⇒ 不补的话，那个任务会永远停在
    /// 「已暂停」，且客户看得见。由恢复线程在后续拍里补发（见 [`retry_pending_unpause`]）。
    pending_unpause: BTreeSet<String>,
    /// 还没提交给引擎的目标（大批量入队的"后台陆续加入"队列，见 [`spawn_feeder`]）。
    /// 提交完成后才出队，所以「下载全部」能据此排除正在飞的那一批；换码时整体登记为拒绝。
    feed_queue: std::collections::VecDeque<delivery::File>,
    /// 批次代数：每次换码 +1（见 [`Kernel::discard_feed_queue`]）。一批提交在飞时凭它发现自己已经作废。
    batch_epoch: u64,
    /// 后台加入过程中被拒绝的（路径、理由）。壳通过 `enqueue_status` 取走。
    feed_rejected: Vec<Value>,
    /// 后台已经成功加入的个数（同上，取走即清零）。
    feed_added: usize,
    /// 后台线程是否在跑。
    feeder_running: bool,
}

/// 起一次引擎要用的**全部输入**（[`Kernel::engine_start`] 的产物）。
///
/// 存在的理由只有一个：**把"取输入"与"起引擎"拆到锁的两侧**。起引擎（含等它就绪）
/// 是网络 I/O、`close` 一个卡死的引擎最坏十几秒 —— 那两段都不许持着内核锁跑
/// （本仓的既有纪律，见文件头第 4 条）。所以先把输入拷出来，再在锁外起。
struct EngineStart {
    download_dir: PathBuf,
    global: BTreeMap<String, String>,
    per_task: BTreeMap<String, String>,
    counters: Arc<crate::diag_counters::Counters>,
}

/// **锁外**：按 [`EngineStart`] 起一个引擎，并**冗余但承重**地下发一次全局选项。
///
/// 那次下发为什么必须在这里、且错误必须可见：`Daemon::start` 内部那一次是**看不见**的
/// （启动路径上的错误被吞掉），这里是整条启动路径上唯一的观测点（契约 §2.6）。
///
/// 引擎就绪留一行**诊断**到 stderr（绝不进协议通道）：引擎出问题时，这行给出的 RPC
/// 端点就是 `curl` 的入口。secret 不打印。
fn start_engine_off_lock(lang: crate::i18n::Lang, start: EngineStart) -> Result<Arc<Daemon>, ErrorBody> {
    let opts = DaemonOptions {
        download_dir: start.download_dir,
        global_opts: start.global.clone(),
        per_task: start.per_task,
        binary_path: None,
        force_port: None,
        counters: Some(start.counters),
    };
    let d = Daemon::start(opts)
        .map_err(|e| {
        ErrorBody::new(
            kcodes::ENGINE_START_FAILED,
            crate::i18n::text::tf(
                lang,
                crate::i18n::text::MsgKey::EngineStartFailed,
                &[("err", &e.to_string())],
            ),
        )
    })?;
    d.apply_global(&start.global).map_err(|e| {
        ErrorBody::new(
            kcodes::ENGINE_START_FAILED,
            format!("引擎已启动，但下发全局选项失败：{e}"),
        )
    })?;
    eprintln!("benagen-core: 下载引擎已就绪（RPC {}）", d.rpc_url());
    Ok(Arc::new(d))
}

impl Kernel {
    pub fn new(download_dir: PathBuf, settings_path: PathBuf) -> Kernel {
        let last_code_path = match settings_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join("last_code"),
            _ => PathBuf::from("last_code"),
        };
        let (settings, language_chosen) = settings::load_from_with_flag(&settings_path);
        // 语言在构造时就解析一次：自愈线程/命令行工具不经过握手也要说对话。
        let language = settings.language.resolve(&crate::i18n::system_locale());
        let last_code = std::fs::read_to_string(&last_code_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let update_path = match settings_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join("update.json"),
            _ => PathBuf::from("update.json"),
        };
        let update = update::load_state(&update_path);
        let state = FileState::load(&download_dir);
        let intent_path = match settings_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(intent::FILE_NAME),
            _ => PathBuf::from(intent::FILE_NAME),
        };
        let intent = Intent::load(&intent_path);
        Kernel {
            download_dir,
            settings_path,
            last_code_path,
            settings,
            language,
            language_chosen,
            // 握手之前不知道壳那边的系统语言（GUI 里 `LANG` 通常是空的）。
            system_locale: None,
            manifest: None,
            last_code,
            update_path,
            update,
            update_checking: false,
            // 从没查过 —— 与 `Some(false)`（查了没查成）不是一回事，见字段上的注释。
            update_last_check_ok: None,
            state,
            verify: verify::CheckResult::default(),
            complete: BTreeSet::new(),
            complete_dirty: true,
            verifying: BTreeSet::new(),
            verified: BTreeSet::new(),
            retried: BTreeSet::new(),
            daemon: None,
            engine_disconnected: false,
            engine_busy: false,
            race: crate::delivery_race::Cache::new(),
            resolve_lines: Vec::new(),
            resolve_state: "idle",
            resolve_origin: None,
            restart: crate::engine::restart::Gate::new(),
            errors: crate::errors::Recent::new(),
            counters: Arc::new(crate::diag_counters::Counters::default()),
            intent,
            stall_prev: None,
            migrate: Arc::new(Mutex::new(migrate::Status::default())),
            hold_auto_retry: false,
            pending_unpause: BTreeSet::new(),
            feed_queue: std::collections::VecDeque::new(),
            feed_rejected: Vec::new(),
            feed_added: 0,
            feeder_running: false,
            batch_epoch: 0,
        }
    }

    /// **锁内（短）**：绑状态，并取一份 [`plan_off_lock`] 的输入。
    ///
    /// ⚠️ `plan` 必须在**锁外**跑（见 [`plan_off_lock`] 与 [`ensure_complete`]）。
    /// 状态取**副本**而不是 `&mut self.state` 的代价是 O(文件数) 的内存复制
    /// （几万个条目也就毫秒级），换掉的是**分钟级的持锁网络 I/O**——量级差得很远。
    ///
    /// `plan` 内部那次 `bind`（纵深防御）绑的是这份副本，所以**真正的 `state`
    /// 必须在这里先绑好**——不然"换码即作废"就只剩副本那一次、不落到真身，
    /// 拿上一批记录跳过这一批文件的静默少交就回来了。
    fn plan_inputs(&mut self, m: &Manifest) -> (PathBuf, FileState) {
        self.state.bind(&m.code);
        (self.download_dir.clone(), self.state.clone())
    }

    /// **锁内（短）**：拷出起一次引擎要用的全部输入（[`EngineStart`]）。
    ///
    /// 纯拷贝、无 I/O —— 为的是让 [`start_engine_off_lock`] 能在锁外跑。
    /// ⚠️ 不要图省事把 `Daemon::start` 挪进这里：那会让协议循环停摆到引擎起来为止。
    fn engine_start(&self) -> EngineStart {
        let global = self.settings.global_options();
        EngineStart {
            download_dir: self.download_dir.clone(),
            per_task: self.settings.per_task_options(),
            global,
            counters: Arc::clone(&self.counters),
        }
    }

    /// 取（必要时启动）引擎。
    ///
    /// ⚠️ **契约 §2.6**：`Daemon::start` 内部那次全局选项下发是**看不见**的
    /// （启动路径上的错误被吞掉），所以这里**必须再调一次 `apply_global` 并让错误可见**
    /// ——这是整条启动路径上唯一的观测点。
    fn ensure_engine(&mut self) -> Result<Arc<Daemon>, ErrorBody> {
        if let Some(d) = self.daemon.clone() {
            if !self.engine_disconnected {
                return Ok(d);
            }
            // 已判定断开 → **每次要用引擎都试一次重连**（这就是「恢复路径」）
            let ping = d.ping();
            let ok = ping.is_ok();
            let cause = ping.err().unwrap_or_default();
            let verdict = reconnect_outcome(ok, self.engine_busy);
            // `why` 传**这条探活路径的成败短语**，不是探活的错误串：探活成功时那个串是空串，
            // 而"true → false"这一格**只**在探活成功时发生，传它会让日志以 `why=` 结尾。
            // （进到这一格 ⇔ 标志本来就是 `true`，所以失败那一支的值没变、
            // `set_engine_disconnected` 不落日志；失败本身由 `ping_failed` 记。）
            let why = match verdict {
                EngineVerdict::Alive => "ensure_engine_probe_ok",
                EngineVerdict::Busy => "ensure_engine_probe_busy",
                EngineVerdict::Dead => "ensure_engine_probe_failed",
            };
            self.set_engine_disconnected(verdict.is_dead(), why);
            // 忙 ⇒ 回"慢"而不是"断开"：调用方按瞬时故障处理，壳不点亮"重启内核"。
            if let Some(e) = verdict.error(self.language, "ping", &cause) {
                return Err(e);
            }
            return Ok(d);
        }
        let d = start_engine_off_lock(self.language, self.engine_start())?;
        self.daemon = Some(Arc::clone(&d));
        // 新引擎起来了 ⇒ 一定不是断开态。走同一个口子（而不是直接赋值）是为了让
        // "`engine_disconnected` 只在 `set_engine_disconnected` 里被写"这条不变式
        // **用 grep 就能核**——这里实际上从不改变值（进这一支时 `daemon` 还是 `None`，
        // 而 `None` 只出现在首次启动前，那时标志必为 `false`），所以也从不落日志。
        self.set_engine_disconnected(false, "engine_started");
        Ok(d)
    }

    /// 读路径上取引擎：已判定断开时**每次读都试一次重连**（设计规格 §9 的"恢复路径"）。
    fn read_engine(&mut self) -> Result<Arc<Daemon>, ErrorBody> {
        match self.daemon.clone() {
            // 已经起来过：复用 `ensure_engine` 的重连逻辑
            Some(_) => self.ensure_engine(),
            None => Err(ErrorBody::new(
                kcodes::ENGINE_NOT_STARTED,
                crate::i18n::text::t(self.language, crate::i18n::text::MsgKey::EngineNotStarted),
            )),
        }
    }

    /// 一次 RPC 失败之后的处置：**尝试重连一次**（重新 ping）。
    /// 仍失败 → 判定断开，后续响应带 `engine_disconnected`（设计规格 §9）。
    ///
    /// `op` 是**出错的调用名**（`"aria2.addUri"` / `"快照"` 之类），只用于 `engine_rpc_failed`
    /// 那条消息——它是客户在壳里唯一能看到的定位信息。
    ///
    /// ⚠️ 这里曾经写 `format!("…（探活：{cause}）")`，而进入那一支 ⇔ `ping_ok == true`
    /// ⇔ `ping.err()` 是 `None`——**`cause` 恒为空串**，客户看到的消息以 `（探活：）` 结尾。
    /// 已换成出错的调用名（那才是有信息量的东西）。
    fn on_rpc_failure(&mut self, d: &Arc<Daemon>, op: &str, err: String) -> ErrorBody {
        let ping_ok = d.ping().is_ok();
        self.settle_rpc_failure(ping_ok, op, err)
    }

    /// [`Kernel::on_rpc_failure`] 的判定段。探活结果由调用方给，好让探活本身可以在锁外做。
    fn settle_rpc_failure(&mut self, ping_ok: bool, op: &str, err: String) -> ErrorBody {
        let verdict = reconnect_outcome(ping_ok, self.engine_busy);
        // `why` 带上**出错的调用名**：那是客户在壳里唯一能看到的定位信息，
        // 只记 `err` 的话日志里会有一堆看不出发生在哪一步的失败。
        // （探活自己的错误已经由 `ping()` 以 `ping_failed` 单独记过一条，不在这里重复。）
        self.set_engine_disconnected(verdict.is_dead(), &format!("{op}: {err}"));
        if verdict == EngineVerdict::Busy {
            // 现场取证用：这条与 `ping_failed` 一起才能还原"是慢不是死"。
            crate::diagnostics::log("engine_rpc_slow_but_writing", &[("op", op.to_string())]);
        }
        // ping 通了（`Alive`）⇒ 只是**这一次**调用失败（瞬时故障），不判断开，让壳可以重试。
        verdict
            .error(self.language, op, &err)
            .unwrap_or_else(|| engine_rpc_failed_error(self.language, op, &err))
    }

    fn require_manifest(&self) -> Result<Manifest, ErrorBody> {
        self.manifest.clone().ok_or_else(|| {
            ErrorBody::new(
                kcodes::NO_DELIVERY,
                "还没有加载交付清单（先调 load_delivery）",
            )
        })
    }

    fn remember_last_code(&mut self, code: &str) {
        self.last_code = code.to_string();
        if let Some(dir) = self.last_code_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 写失败不致命（记不住上次的码只影响体验），但也别静默——记一行日志。
        if let Err(e) = std::fs::write(&self.last_code_path, code) {
            eprintln!("benagen-core: 记录上次交付码失败（不影响下载）: {e}");
        }
    }

    /// 落盘「想下的文件集」。失败只记一行：它只影响「自动补回队列」，不影响任何下载。
    fn save_intent(&self) {
        if let Err(e) = self.intent.save() {
            crate::diagnostics::log("intent_save_failed", &[("err", e.to_string())]);
        }
    }

    /// 落盘更新状态。**返回值丢弃**：落盘失败不该让命令失败（与"失败一律静默"同一条口径）。
    fn save_update_state(&self) {
        let _ = update::save_state(&self.update_path, &self.update);
    }
}

/// `engine_rpc_failed` 那条消息的**唯一构造点**（抽出来是为了有测试守护）。
///
/// 这是客户在壳里唯一能看到的定位信息，所以两样东西都必须在里面：
/// **出错的调用名**（`op`）与 RPC 自己的错误串（`err`）。
///
/// ⚠️ 这里曾经写成 `format!("…（探活：{cause}）")`，而这条分支 ⇔ `ping_ok == true`
/// ⇔ `ping.err()` 是 `None`——**`cause` 恒为空串**，客户看到的消息会以 `（探活：）`
/// 结尾。`engine_rpc_failed_message_carries_the_operation` 把"必须带调用名"
/// 钉住（换回那个空括号就会红）。
fn engine_rpc_failed_error(lang: crate::i18n::Lang, op: &str, err: &str) -> ErrorBody {
    ErrorBody::new(
        kcodes::ENGINE_RPC_FAILED,
        crate::i18n::text::tf(
            lang,
            crate::i18n::text::MsgKey::EngineRpcFailed,
            &[("op", op), ("err", err)],
        ),
    )
}

/// 「引擎在写盘、但回不上话」那条消息的**唯一构造点**。
///
/// 与 [`engine_rpc_failed_error`] 的区别是**探活结果**：这条对应"探活不通，但进程在动"。
/// 措辞上**不许**出现"已断开"：文件在盘上、任务没有丢，壳里那颗"重启内核"的按钮
/// 不该被它点亮（现场 0.2.7 的误报正是从这里开始的）。
fn engine_rpc_slow_error(lang: crate::i18n::Lang, op: &str, err: &str) -> ErrorBody {
    ErrorBody::new(
        kcodes::ENGINE_RPC_FAILED,
        crate::i18n::text::tf(
            lang,
            crate::i18n::text::MsgKey::EngineRpcSlow,
            &[("op", op), ("err", err)],
        ),
    )
}

/// **引擎断开之后的唯一判定点**（设计规格 §9 / 简报第 5 条）。
///
/// 返回 `(新的「已断开」标志, 调用方该不该继续用这个引擎)`：
///   - `ping_ok == true` → 引擎还活着，**清掉**「已断开」，调用方继续；
///   - `ping_ok == false` → 保持「已断开」，回一条带
///     [`kcodes::ENGINE_DISCONNECTED`] 的错误（消息里点明"已落盘的文件保留"，
///     因为设计规格 §9 要求客户能直接重试）。
///
/// ⚠️ **抽成纯函数是为了让这张判定表有测试守护。** 但它守的是**表**，不是**调用点**：
/// `d.ping()` 那个真实调用在 e2e 里造不出"进程死了又活过来"的场景（内嵌 aria2c 是
/// 内核自己起的，端口与 secret 都在进程里），所以"把 `d.ping()` 换成恒定 `Err`"
/// 这个变异体**仍然存活**。这一点在 task-13 报告里显式记账——**不要以为这条测试守住了它**。
fn reconnect_outcome(ping_ok: bool, engine_busy: bool) -> EngineVerdict {
    if ping_ok {
        EngineVerdict::Alive
    } else if engine_busy {
        EngineVerdict::Busy
    } else {
        EngineVerdict::Dead
    }
}

/// 一次 RPC 失败之后对引擎的三档判读（[`reconnect_outcome`] 的结果）。
///
/// ⚠️ **"忙"必须与"死"分开**：两者的 RPC 表现一模一样（都不回话），但健康状态相反。
///    混成一档的后果在 0.2.7 的现场日志里是完整的：判死 ⇒ 阶梯暂停全部 ⇒ 换引擎 ⇒
///    重新入队 234 个任务，而被关掉的那个引擎当时正在往盘上写 GB 级数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineVerdict {
    /// 探活通了：引擎活着，只是这一次调用失败（瞬时故障，壳可重试）。
    Alive,
    /// 探活不通，但进程还在写盘 / 烧 CPU ⇒ **它在干活，只是回不上话**。
    Busy,
    /// 探活不通，进程也没动 ⇒ 真的死了。
    Dead,
}

impl EngineVerdict {
    /// 只有"真的死了"才置「已断开」——「忙」不许点亮壳里那颗"重启内核"的按钮。
    fn is_dead(self) -> bool {
        matches!(self, Self::Dead)
    }

    /// 这个判读该怎么回错误（`Alive` 以外都要回）。
    ///
    /// ⚠️ **错误文案的唯一出口**：三处判定（读/写方法失败、`ensure_engine` 的重连、
    ///    动作路径）都走这里，免得同一个判读在三处写出三种措辞。
    fn error(self, lang: crate::i18n::Lang, op: &str, err: &str) -> Option<ErrorBody> {
        match self {
            Self::Alive => None,
            Self::Busy => Some(engine_rpc_slow_error(lang, op, err)),
            Self::Dead => Some(engine_disconnected_error(lang, err)),
        }
    }
}

/// 「真的断开」那条消息的唯一构造点（消息里点明"已落盘的文件保留"，设计规格 §9）。
///
/// 文案在 `i18n::text`（规格 W1）：`lang` 由内核当前语言给，**不在这里判语言**。
fn engine_disconnected_error(lang: crate::i18n::Lang, cause: &str) -> ErrorBody {
    ErrorBody::new(
        kcodes::ENGINE_DISCONNECTED,
        crate::i18n::text::tf(lang, crate::i18n::text::MsgKey::EngineDisconnected, &[("cause", cause)]),
    )
}

// ---------------------------------------------------------------------------
// 自愈线程：每秒判活，两个信号分开判（规格 §3.1），阶梯决定动作（规格 §3.2）
// ---------------------------------------------------------------------------

/// 一段持续健康之前，最多换几次引擎。
///
/// ⚠️ 没有这条就是无限重启循环：新起来的引擎若照样不应答，就会一直关一个、起一个，
/// 日志被刷屏、CPU 白烧，而问题一点没动。到顶之后只剩判活本身，并落一条
/// `engine_restart_gave_up` 把"自动手段用尽了"留在日志里。
const RECOVERY_MAX_RESTARTS: u32 = 3;

/// 停滞期间"是哪一个任务卡住了"的采样间隔。
///
/// 这是**我们自己的采样**开销，不是引擎的开销：每拍都取一次任务列表，会叠加 `tellActive` 的最坏耗时
/// （规格 F2/F3，单次可达约 10 秒），把判活节拍拖慢，恰好在最需要看清的时候。停滞阈值是 180 秒，
/// 5 秒一次仍有 36 个采样点，够用。
const TASK_SAMPLE_EVERY: Duration = Duration::from_secs(5);

/// 从一次采样的任务表里取出**应当正在动的那些**，形如 `(gid, completedLength)`。
///
/// ⚠️ `Daemon::active()` 只发 `tellActive`，表里本就只有活跃任务；这里仍滤一遍，让"应当正在动"的定义只有这一处。
///    不滤的话，传完、出错、被移除的任务（`completedLength` 永远不再动）会被永远判成停滞。
/// ⚠️ `Waiting` 也不收：它既含真排队，也含客户手动暂停的；卡在并发上限后面的不算"卡住"。
fn active_tasks(tasks: &[Task]) -> Vec<(String, u64)> {
    tasks
        .iter()
        .filter(|t| t.state == TaskState::Active)
        .map(|t| (t.gid.clone(), t.completed.max(0) as u64))
        .collect()
}

/// 该记哪些停滞：对照 `already`（已记过的 gid），只返回**新**达到阈值的 `(gid, 停滞时长)`。
/// 纯函数，好在宿主上真测。
fn stalls_to_record(
    stalled: &[(String, Duration)],
    already: &BTreeSet<String>,
) -> Vec<(String, Duration)> {
    stalled
        .iter()
        .filter(|(gid, _)| !already.contains(gid))
        .cloned()
        .collect()
}

/// 把这一拍新识别出的停滞记进计数器。返回本次新记的条数。
/// 纯决策在 [`stalls_to_record`]；这里只多做"记账"与维护 `already` 那一步。
/// 判别力：删掉本函数里的 `record_stall` ⇒ [`new_stalls_are_recorded_in_the_counters`] 红。
fn record_new_stalls(
    counters: &crate::diag_counters::Counters,
    stalled: &[(String, Duration)],
    already: &mut BTreeSet<String>,
) -> usize {
    let fresh = stalls_to_record(stalled, already);
    for (gid, how_long) in &fresh {
        counters.record_stall(*how_long);
        already.insert(gid.clone());
    }
    // 不再停滞的 gid 从 already 清掉，下次再停滞才重记。
    let current: BTreeSet<&str> = stalled.iter().map(|(g, _)| g.as_str()).collect();
    already.retain(|g| current.contains(g.as_str()));
    fresh.len()
}

/// 「停 → 起」一个 gid 的结局。
enum Relaunch {
    /// 没有要还的。
    Clean,
    /// 欠一次 unpause。
    Owed,
    /// 停与起都没回话：欠账，而且引擎答不了这个 gid 的 pause/unpause，同一批后面的 gid 大概率也一样。
    Silent,
}

/// 停掉再拉起一个 gid。`stop` 是调用方选定的停法的结果（档 1 用 `pause`，档 2 用 `forcePause`）。
///
/// pause 被引擎**明确拒绝** ⇒ 任务根本没被暂停，不欠账，也不发 unpause（会打到不该碰的任务上）。
/// pause **没回话** ⇒ 它可能已经生效了，我们赌不起：照样补 unpause。
/// unpause 被拒 ⇒ 已经是未暂停态，不欠账；没回话 ⇒ 欠账。
///
/// `held` 在发 unpause **之前**再读一次：客户可能在 `stop` 进行期间按下「暂停全部」（`stop` 没回话时要等满
/// `RPC_TIMEOUT`，约 10 秒）。读到就不发 unpause，也不记欠账——那一格要么被我们的停住、要么被客户的暂停全部停着，
/// 两种都是客户要的"停着"，之后「一键开始」自会拉起。
/// ⚠️ 窗口只是收窄，没有消除：残留在"重读"与"unpause 被引擎处理"之间（正常毫秒级；引擎慢时，
/// unpause 在途期间若客户按下暂停全部，仍可能被它盖过）。
fn relaunch(d: &Daemon, gid: &str, stop: WriteOutcome, held: &dyn Fn() -> bool) -> Relaunch {
    let stop_silent = matches!(stop, WriteOutcome::NoAnswer(_));
    match stop {
        WriteOutcome::Rejected(_) => Relaunch::Clean,
        WriteOutcome::Applied | WriteOutcome::NoAnswer(_) => {
            if held() {
                return Relaunch::Clean;
            }
            match d.unpause_detailed(gid) {
                WriteOutcome::NoAnswer(_) if stop_silent => Relaunch::Silent,
                WriteOutcome::NoAnswer(_) => Relaunch::Owed,
                WriteOutcome::Applied | WriteOutcome::Rejected(_) => Relaunch::Clean,
            }
        }
    }
}

/// 档 1 的动作：把**一个**任务停掉再拉起来，丢它的在途连接，从 `.aria2` 偏移续下。
///
/// 返回 `Some(gid)` = **欠一次 unpause**；`None` = 不欠账。
fn reconnect_one(d: &Daemon, gid: &str, held: &dyn Fn() -> bool) -> Option<String> {
    match relaunch(d, gid, d.pause_detailed(gid), held) {
        Relaunch::Clean => None,
        Relaunch::Owed | Relaunch::Silent => Some(gid.to_string()),
    }
}

/// 挑出停滞最久（且最久没被照顾过）的那个任务重连它。欠账的 gid 记进 `owed`。没有停滞任务时什么都不做。
///
/// `held`：客户按过「暂停全部」时一条 RPC 都不发（理由见 [`retry_pending_unpause`]）。
/// 它是闭包而非布尔：动作前读一次，`relaunch` 发 unpause 前再读一次。
///
/// 与档 2 的 [`pause_then_resume_each`] 同一道保护：动作前重新取一次 `tellActive`，
/// 候选 gid 不在活跃表里（已被客户单独暂停 / 完成 / 移除）⇒ 本拍不动它。
/// `active()` 取不到 ⇒ 本拍不动。没有这道保护的话，`pause`（对已暂停是空操作）→ `unpause`
/// 会把客户的单独暂停撤掉。
///
/// 调用方须在锁外调用（内部是 RPC）。
fn reconnect_most_stalled(
    d: &Daemon,
    progress: &mut TaskProgress,
    t: &Thresholds,
    now: std::time::Instant,
    owed: &mut BTreeSet<String>,
    held: &dyn Fn() -> bool,
) -> Option<String> {
    if held() {
        return None;
    }
    let gid = progress.most_stalled(now, t)?;
    // 与档 2 的 `pause_then_resume_each` 同一道保护（见上方注释）。
    let Ok(live) = d.active() else {
        return None;
    };
    let live: BTreeSet<String> = live.into_iter().map(|t| t.gid).collect();
    if !live.contains(&gid) {
        return None;
    }
    if let Some(debt) = reconnect_one(d, &gid, held) {
        owed.insert(debt);
    }
    progress.mark_attempted(&gid, now);
    Some(gid)
}

/// 补发欠着的 `unpause`：没回话的留到下一拍；引擎明确应答（接受或拒绝）的都移出，
/// 因为拒绝意味着"已经没有要还的了"（如任务已被移除或已经是未暂停态）。
///
/// 客户按过「暂停全部」时整段跳过：那时"暂停"正是客户要的，补发 unpause 等于违背客户意图。
fn retry_pending_unpause(d: &Daemon, pending: &mut BTreeSet<String>, hold: bool) {
    if hold {
        return;
    }
    pending.retain(|gid| matches!(d.unpause_detailed(gid), WriteOutcome::NoAnswer(_)));
}

fn persist_pending(kernel: &Arc<Mutex<Kernel>>, owed: &BTreeSet<String>) {
    lock(kernel).pending_unpause = owed.clone();
}

/// 重启预算能不能复位？纯函数，好在宿主上真测。
///
/// ⚠️ **两个条件缺一不可**：只要求"健康"，那么"引擎应答、下载全停"也算健康，
///    F6 现场那种形态就能靠它无限续命。`stalled` 为零，才说明下载真的在动。
fn restart_budget_is_refilled(healthy: Option<Duration>, stalled: Duration, t: &Thresholds) -> bool {
    healthy.is_some_and(|h| h >= t.pre_swap_wait) && stalled.is_zero()
}

/// 记一次档位的**决策**（判定它要这一拍去做）：计数，并落一条快照日志。
/// 档 1、档 2 接入动作之后仍是一决策一记录，语义不变。`rung_swap` 的计数在认输检查之后，认输的拍不计入。
fn note_rung(counters: &crate::diag_counters::Counters, rung: Rung) {
    counters.record_rung(rung);
    counters.snapshot().log(match rung {
        Rung::ReconnectOne => "rung_reconnect",
        Rung::PauseAllThenResumeAll => "rung_pause_all",
        Rung::ReplaceEngine => "rung_swap",
        Rung::Observe => unreachable!(),
    });
}

/// 档 2 的动作：把**卡住的那些** gid 逐个用 `forcePause` 停掉再拉起来，返回欠了 `unpause` 的 gid（与档 1 同一套语义）。
///
/// 🔴 用 `forcePause`，不用优雅的 `pause`：优雅暂停要等各任务收尾，连接多时可以拖很久，
///    而档 2 要的正是把卡住的连接丢掉。用 `pause` 的话，停启之后连接可能根本没断，整个动作空转。
/// 🔴 不用 `Daemon::pause_all` / `unpause_all`：`unpauseAll` 会把**客户单独暂停**的任务也解开，那是撤掉客户的显式动作。
///    （对已暂停的任务再发 `forcePause` 是空操作，所以"它会再停一次客户的任务"不是理由。）
/// `held`：客户按过「暂停全部」时一条 RPC 都不发。它是闭包而不是布尔：整批可能跑几十秒，
/// 客户可能在中途按下「暂停全部」，所以**每个 gid 动作之前都重读一次**（一次锁读，不是 RPC），读到就收手。
/// `answered`：本拍探活没拿到应答时整批不发。引擎不应答时那些调用必然失败，整批是纯浪费，
/// 而且会白占掉这一拍（探活、采样、档位判定都排在它后面）。跳过不是为了让档 3 提前：
/// 档 3 的时点以决策时刻起算（`Ladder::next` 的 `last_pause_all`），与动作成败无关。
///
/// 动作前重新取一次 `tellActive`，与名单取交集：名单来自上一次成功采样，客户可能在那之后单独暂停了其中某个
/// （暂停中的任务不在 `tellActive` 里）。取不到就整批不动，宁可这一拍不做。
/// ⚠️ **仍有窗口**：取快照与每个 gid 的动作之间，客户仍可能暂停它。窗口从采样节拍（5 秒）缩到
///    "一次 `tellActive` 往返，加上排在前面的那些 gid 的耗时"；慢引擎（逼近 10 秒超时）下，靠后的 gid 窗口可达几十秒。
///
/// 某个 gid 的停与起都没回话（[`Relaunch::Silent`]）⇒ 立即收手：引擎答不了这个 gid，后面的 gid
/// 只会同样各赔一次超时。剩下的 gid 没被动过、也不欠账，跳过无害。
///
/// ⚠️ 只吃 `&Daemon` ⇒ 结构上够不到 `Kernel::engine_disconnected`。
/// 本动作本身不置断开标志；任何路径在"发现引擎真的不通"时都可以置位
/// （例如档 3 之后新引擎的 `addUri` 失败且 ping 也不通，经 `settle_rpc_failure`），那与恢复动作无关。
/// 组装 `ladder_pause_each` 那一行的字段（顺序是契约：读日志的人按位找）。
/// 出口由调用方给 —— 生产路径传 `&crate::diagnostics::log`。
/// （先例：`diag_counters.rs` 的 `Snapshot::log_with`，为同一类困难开的同一道缝。）
///
/// 判读（配合 `n`、`held`、`answered` 读出"这一档为什么没动手"）：
///   · n=0, held=true  ⇒ 被客户的「暂停全部」挡住；
///   · n=0, held=false ⇒ 四种：`answered=false`（探活无应答，整批不发）、`active()` 失败、
///     名单为空、名单里的 gid 都不在 `tellActive` 里（客户单独暂停了它们，或已不在活动表）；
///   · n>0, held=true  ⇒ 中途被按下，剩下的 gid 没动。
/// `held` 是**最终**那次读数，不是入场快照。
fn log_pause_each(
    sink: &dyn Fn(&str, &[(&str, String)]),
    n: usize,
    held: bool,
    answered: bool,
    owed: usize,
) {
    sink(
        "ladder_pause_each",
        &[
            ("n", n.to_string()),
            ("held", held.to_string()),
            ("answered", answered.to_string()),
            ("owed", owed.to_string()),
        ],
    );
}

fn pause_then_resume_each(d: &Daemon, gids: &[String], held: &dyn Fn() -> bool, answered: bool) -> Vec<String> {
    let mut owed = Vec::new();
    let mut attempted = 0usize;
    let mut held_last = held();
    'batch: {
        if held_last {
            break 'batch;
        }
        if !answered {
            break 'batch;
        }
        let Ok(live) = d.active() else {
            break 'batch;
        };
        let live: BTreeSet<String> = live.into_iter().map(|t| t.gid).collect();
        for gid in gids.iter().filter(|g| live.contains(*g)) {
            held_last = held();
            if held_last {
                break 'batch;
            }
            attempted += 1;
            match relaunch(d, gid, d.force_pause_detailed(gid), held) {
                Relaunch::Clean => {}
                Relaunch::Owed => owed.push(gid.clone()),
                Relaunch::Silent => {
                    owed.push(gid.clone());
                    break 'batch;
                }
            }
        }
    }
    // `held_last` 是最终那次读数（不是入场快照）；四个字段怎么读出"为什么没动手"，见 [`log_pause_each`]。
    log_pause_each(
        &crate::diagnostics::log,
        attempted,
        held_last,
        answered,
        owed.len(),
    );
    owed
}

/// **引擎的后台自愈**（规格 §3）：每秒判一次引擎是否还应答、下载是否还在走，
/// 由 [`Ladder`] 决定这一拍要不要动手。
///
/// 两个信号互相独立（见 `engine::supervisor`）：
///   · 信号 A（应答性）：每拍一条 `getGlobalStat`，记下它的成败与耗时；
///   · 信号 B（进展）：同一条响应里的速率与活跃数。全局速率只用来决定重启预算能否复位。
///     任务级的停滞按自己的节拍采样（[`TASK_SAMPLE_EVERY`]），与全局速率解耦：
///     全局速率大于 0 时，卡住的那一个任务照样要被认出来。
///
/// 重启计数与认输标记只在**持续健康**且**下载在动**之后才复位（见 [`restart_budget_is_refilled`]）；
/// 单次探通不复位。
///
/// ⚠️ **不用 `Daemon::snapshot()` 判活**：它的 `tellActive` 正是现场卡住的那一条（规格 F2/F3）。
///    判活只用 `probe()`；任务级的 `active()` 只按 [`TASK_SAMPLE_EVERY`] 的节拍、且本拍探通时才调。
///
/// ⚠️ **`daemon` 句柄每拍从锁里现取**，不当作参数传进来：换引擎之后句柄会变，启动时的那份会过期。
///
/// ⚠️ **网络 I/O 在锁外**：锁内只取句柄与计数器，以及翻断开标志那一下（连同它的诊断）。
///    [`replace_engine`] 内部同样把"收旧引擎"与"起新引擎"两段都放在锁外。
pub fn spawn_recovery(kernel: Arc<Mutex<Kernel>>) {
    std::thread::Builder::new()
        .name("engine-recovery".to_string())
        .spawn(move || {
            let mut ladder = Ladder::new(Thresholds::default());
            let mut liveness = Liveness::default();
            let mut progress = Progress::default();
            let mut task_progress = TaskProgress::default();
            let mut restarts: u32 = 0;
            let mut gave_up = false;
            let mut stall_prev: Option<crate::procstat::Counters> = None;
            let mut last_counters = std::time::Instant::now();
            let mut last_task_sample = std::time::Instant::now();
            let mut recorded_stalls: BTreeSet<String> = BTreeSet::new();
            // 进程读数的上一拍（只为"忙/死"判别服务，与 `stall_prev` 是两份基线，互不干扰）。
            let mut busy_prev: Option<crate::procstat::Counters> = None;
            // 一段"没应答但仍在写"只记一条日志，别把 75 分钟的静默刷成几千行。
            let mut busy_logged = false;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let (d, counters, disconnected, mut owed, dir) = {
                    let k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                    let Some(d) = k.daemon.clone() else { continue };
                    (
                        d,
                        Arc::clone(&k.counters),
                        k.engine_disconnected,
                        k.pending_unpause.clone(),
                        k.download_dir.clone(),
                    )
                };
                let now = std::time::Instant::now();

                let started = std::time::Instant::now();
                let probe = d.probe();
                let answered = probe.is_ok();
                match probe {
                    Ok(stat) => {
                        liveness.record_ok(started.elapsed());
                        progress.observe(
                            now,
                            stat.download_speed.max(0) as u64,
                            stat.num_active.max(0) as u64,
                        );
                        if disconnected {
                            let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                            if k.engine_disconnected {
                                crate::diagnostics::log(
                                    "engine_recovered",
                                    &[("by", "recovery_probe".to_string())],
                                );
                                k.set_engine_disconnected(false, "recovery_probe");
                            }
                        }
                    }
                    Err(_) => liveness.record_fail(),
                }

                // 本拍没应答就不采样：那一拍 tellActive 多半也拿不到应答。取不到就保留上次的记录，
                // 记录自带时刻，一次抖动不该把一段真实的停滞抹掉。
                if answered && now.saturating_duration_since(last_task_sample) >= TASK_SAMPLE_EVERY {
                    last_task_sample = now;
                    if let Ok(tasks) = d.active() {
                        task_progress.observe(now, &active_tasks(&tasks));
                    }
                }
                let stalled_tasks = task_progress.stalled_count(now, &ladder.thresholds());
                let stalled = task_progress.stalled_with_durations(now, &ladder.thresholds());
                record_new_stalls(&counters, &stalled, &mut recorded_stalls);

                if now.saturating_duration_since(last_counters) >= Duration::from_secs(60) {
                    last_counters = now;
                    counters.snapshot().log("periodic");
                    // 轮转路径与启动的 `--log=` **同一个绝对形式**（`absolute_dir`）：
                    // 不经它的话，`download_dir` 是相对路径时（`paths::download_dir` 在 `$HOME`
                    // 缺失时回退 `benagen-downloads`）轮转会指到别处。解析失败就跳过这一拍
                    // （日志轮转不许打断下载）。⚠️ 这个调用点活在循环里，运行时钉不到
                    // （同本计划其他「运行时钉不到」）；推导一致性由 [`crate::engine::daemon::absolute_dir`]
                    // 的既有行为测试与 `start_on_port` 的同一调用保证。
                    if let Ok(abs) = crate::engine::daemon::absolute_dir(&dir) {
                        crate::engine::daemon::rotate_engine_log(
                            &abs.join(crate::engine::daemon::ENGINE_LOG_FILE),
                        );
                    }
                }

                if answered && !owed.is_empty() {
                    let owed_before = owed.clone();
                    // 拍首取的 hold 快照可能已过时（探活与 active() 最长约 20 秒，客户可能在此期间按下「暂停全部」），
                    // 所以补发前于锁内重读一次。
                    let hold = lock(&kernel).hold_auto_retry;
                    retry_pending_unpause(&d, &mut owed, hold);
                    if owed != owed_before {
                        persist_pending(&kernel, &owed);
                    }
                }

                let engine_moving = progress.stalled_for(now) < ladder.thresholds().stall;
                // 引擎进程**自己**的读数：RPC 不回话时，只有它能分出"忙"与"死"。
                // 非 Windows 上 `procstat::read` 恒失败 ⇒ `None` ⇒ `false` ⇒ 与旧口径相同。
                let engine_busy = crate::procstat::busy(d.pid(), &mut busy_prev).unwrap_or(false);
                lock(&kernel).engine_busy = engine_busy;
                if answered {
                    busy_logged = false;
                } else if engine_busy && !busy_logged {
                    busy_logged = true;
                    crate::diagnostics::log(
                        "engine_slow_but_writing",
                        &[("pid", d.pid().unwrap_or(0).to_string())],
                    );
                }
                let rung = ladder.next(now, &liveness, stalled_tasks, engine_moving, engine_busy);

                // ── 第 2/3 层（规格 W3）：自动重启只由账本决定 ──────────────────
                // 阶梯的 `ReplaceEngine` 不再是执行者：它只是"看起来没救了"的一个旁证。
                // 真正动手要过三关——探活连败 3 次、进程确实没动（第 1 层取证）、预算未用尽。
                let ms = now_ms();
                let verdict = {
                    let mut k = lock(&kernel);
                    k.restart.on_probe(ms, answered, engine_busy, engine_moving)
                };
                match verdict {
                    crate::engine::restart::Action::Restart { attempt } => {
                        crate::procstat::log_sample(d.pid(), &mut stall_prev, "before_restart");
                        crate::diagnostics::log(
                            "engine_restart",
                            &[
                                ("attempt", attempt.to_string()),
                                ("exited", d.process_exited().to_string()),
                                ("by", "auto".to_string()),
                            ],
                        );
                        let lang = lock(&kernel).language;
                        match replace_engine(lang, &kernel) {
                            Ok(_) => {
                                // 欠账的 gid 属于旧引擎，换过去之后补发只会永远报"找不到"。
                                owed.clear();
                                persist_pending(&kernel, &owed);
                                counters.record_engine_restart();
                                lock(&kernel).restart.restart_done(ms);
                                crate::diagnostics::log(
                                    "engine_restarted",
                                    &[("attempt", attempt.to_string())],
                                );
                            }
                            Err(e) => {
                                lock(&kernel).restart.restart_done(ms);
                                crate::diagnostics::log(
                                    "engine_restart_failed",
                                    &[("attempt", attempt.to_string()), ("err", e.message)],
                                );
                            }
                        }
                        continue;
                    }
                    crate::engine::restart::Action::Recovered { attempt } => {
                        let mut k = lock(&kernel);
                        k.set_engine_disconnected(false, "auto_restart_recovered");
                        crate::diagnostics::log(
                            "engine_auto_recovered",
                            &[("attempt", attempt.to_string())],
                        );
                    }
                    crate::engine::restart::Action::Exhausted => {
                        let mut k = lock(&kernel);
                        if !k.engine_disconnected {
                            note_auto_restart_gave_up(&mut k);
                        }
                    }
                    crate::engine::restart::Action::None => {}
                }

                if restart_budget_is_refilled(
                    ladder.healthy_for(now),
                    progress.stalled_for(now),
                    &ladder.thresholds(),
                ) {
                    lock(&kernel).restart.reset();
                }
                match rung {
                    Rung::Observe => {}
                    Rung::ReconnectOne => {
                        note_rung(&counters, rung);
                        // 在动作前一刻读，不用拍首快照（快照可能已经过时）。
                        // 循环里的这处调用点（以及档 2 的同型调用）运行时钉不到：没有可注入的桩去造"客户恰好在此刻按下暂停全部"，
                        // 复查的行为由 `relaunch` 的单测守着。
                        let hold = || lock(&kernel).hold_auto_retry;
                        if reconnect_most_stalled(&d, &mut task_progress, &ladder.thresholds(), now, &mut owed, &hold)
                            .is_some()
                        {
                            crate::diagnostics::log("ladder_reconnect", &[("did", "1".to_string())]);
                        }
                        persist_pending(&kernel, &owed);
                    }
                    Rung::PauseAllThenResumeAll => {
                        note_rung(&counters, rung);
                        // `unresponsive` 分支不要求 `!engine_moving`，所以客户按过「暂停全部」时这里必须挡。
                        // 名单只取卡住的：档 2 是来救它们的，健康任务不该被停一下。
                        // 恢复动作本身不置断开标志（见 `pause_then_resume_each`）。
                        let hold = || lock(&kernel).hold_auto_retry;
                        let gids = task_progress.stalled_gids(now, &ladder.thresholds());
                        owed.extend(pause_then_resume_each(&d, &gids, &hold, answered));
                        persist_pending(&kernel, &owed);
                    }
                    Rung::ReplaceEngine => {
                        // 执行权已经交给上面的 `restart` 账本（规格 W3）：这里**不做**任何事。
                        // 保留这个分支是为了让"阶梯看到了什么"仍有日志可循。
                        crate::diagnostics::log("ladder_replace_wanted", &[("by", "ladder".to_string())]);
                        if false {
                            if !gave_up {
                                gave_up = true;
                                crate::diagnostics::log(
                                    "engine_restart_gave_up",
                                    &[("restarts", restarts.to_string())],
                                );
                            }
                            continue;
                        }
                        restarts += 1;
                        note_rung(&counters, rung);
                        crate::procstat::log_sample(d.pid(), &mut stall_prev, "before_restart");
                        // `exited` 区分"进程没了"与"活着但不吭声"，两者要修的方向完全不同。
                        crate::diagnostics::log(
                            "engine_restart",
                            &[
                                ("attempt", restarts.to_string()),
                                ("exited", d.process_exited().to_string()),
                            ],
                        );
                        let lang = lock(&kernel).language;
                        match replace_engine(lang, &kernel) {
                            Ok(_) => {
                                // 欠账的 gid 属于旧引擎，换过去之后补发只会永远报"找不到"。
                                owed.clear();
                                persist_pending(&kernel, &owed);
                                counters.record_engine_restart();
                                crate::diagnostics::log(
                                    "engine_restarted",
                                    &[("attempt", restarts.to_string())],
                                );
                            }
                            Err(e) => crate::diagnostics::log(
                                "engine_restart_failed",
                                &[("attempt", restarts.to_string()), ("err", e.message)],
                            ),
                        }
                    }
                }
            }
        })
        .expect("起自愈线程失败");
}

/// **有新版本就记下来**（规格 §3）：启动即可能查一次，之后每 24 小时一次。
///
/// ⚠️ **网络 I/O 在锁外**，与 [`spawn_recovery`] 同一条纪律（见 [`run_update_check`]）。
/// ⚠️ **手动查（`update_check_now`）不受节流限制**，但也要落盘 —— 于是手动查完
///    24 小时内不会再自动查一次。这一条由 [`op_check_update_now`] 自己保证。
///
/// ⚠️ **tick 取 60 秒而非 1 秒**：节流粒度是 24 小时，60 秒与 1 秒在行为上不可分辨，
///    而 60 秒是 60× 少的唤醒。
pub fn spawn_update_check(kernel: Arc<Mutex<Kernel>>) {
    std::thread::Builder::new()
        .name("update-check".to_string())
        .spawn(move || {
            // 立刻跑第一拍（"启动时一次"）。判据仍然是 `should_check`，
            // 所以"刚查过又重启"不会被查第二次。
            loop {
                let due = {
                    let k = lock(&kernel);
                    update::should_check(k.update.enabled, k.update.last_checked_at, now_secs())
                };
                if due {
                    run_update_check(&kernel);
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .expect("起更新检查线程失败");
}

/// 真查一次并落盘。**锁外做网络**；只有"置标志"与"写结果"这两下进锁。
///
/// 两条路径共用它：后台线程（[`spawn_update_check`]）与手动查（[`op_check_update_now`]）。
///
/// ⚠️ **这里只做 I/O，判决全部在 [`record_check_result`]**：本机的测试跑不出网络
///    （也不该跑），所以"这一格怎么记"必须留在打得到的地方 —— 见那条函数的注释。
fn run_update_check(kernel: &Arc<Mutex<Kernel>>) {
    {
        let mut k = lock(kernel);
        if !k.update.enabled {
            return; // 开关关掉 ⇒ 不发起任何请求（规格 §5）
        }
        // 手动查那颗按钮唯一的反馈：进网络前置真，写完结果置假。
        // 壳在另一条连接上轮询 `update_status` 时看得到它。
        k.update_checking = true;
    }
    let latest = update::fetch_latest(&update::UreqFetcher::new());
    let mut k = lock(kernel);
    record_check_result(&mut k, latest, now_secs());
}

/// 把**一次检查的结果**记下来（锁内跑；与网络那一半分开是为了能在本机断言）。
///
/// ⚠️ `latest` 是 `None` 有两处来源 —— 网络/解析失败，或"发布页上最新那条是预发布"
///    （`fetch_latest` 的口径）—— 两处都表示**这次没拿到一个可比较的版本号**，
///    所以在 [`Kernel::update_last_check_ok`] 上**只能记"没查成"**。
///    壳那一侧的后果仅此一句"没查成"，不会替客户编出"已是最新"。
fn record_check_result(k: &mut Kernel, latest: Option<String>, now: i64) {
    k.update_checking = false;
    // 🔴 先记"查成没查成"，再谈版本号：这一格是手动查那颗按钮**唯一**能分辨
    //    "没查成"与"没有新版"的依据（见字段上的注释）。
    k.update_last_check_ok = Some(latest.is_some());
    k.update.last_checked_at = now;
    // ⚠️ **失败时不覆盖 `last_seen`**：一次网络抖动不该把"上次看到有新版本"抹掉，
    //    那会让提示在网络恢复前先消失一下。失败就保持原样。
    if let Some(v) = latest {
        k.update.last_seen = Some(v);
    }
    k.save_update_state();
}

// ---------------------------------------------------------------------------
// 失败任务的自动重试（规格 2026-10-12 §7；纯判据在 `autoretry.rs`）
// ---------------------------------------------------------------------------

/// 自动重试的节拍。5 秒足够：真正的间隔由 [`autoretry::backoff`] 决定（30 秒起步）。
const AUTO_RETRY_TICK: Duration = Duration::from_secs(5);

/// 跑一拍自动重试，返回这一拍重新入队了几个文件。
///
/// 只重试：**瞬时性**失败（`is_transient`）、**用户想下的**（在 `intent` 里）、
/// **没有被暂停全部按住**、**引擎没被判断开**、**没在搬运**。其余一律不碰。
///
/// 网络 I/O（快照与逐个 `remove`/`add`/`pause`）都在**锁外**。进锁只有三处：取输入、
/// add 之后复查「暂停全部」、`reopen_for_download`。
/// `scale` 把退避按比例缩放（生产 1.0；测试 0.0 让间隔归零）。
fn auto_retry_tick(
    kernel: &Arc<Mutex<Kernel>>,
    tracker: &mut autoretry::Tracker,
    now: std::time::Instant,
    scale: f64,
) -> usize {
    let (d, m, wanted, complete, counters) = {
        let k = lock(kernel);
        let migrating = k.migrate.lock().unwrap_or_else(PoisonError::into_inner).running;
        if k.hold_auto_retry || k.engine_disconnected || migrating {
            return 0;
        }
        let (Some(d), Some(m)) = (k.daemon.clone(), k.manifest.clone()) else {
            return 0;
        };
        let wanted: BTreeSet<String> = k.intent.paths().cloned().collect();
        (d, m, wanted, k.complete.clone(), Arc::clone(&k.counters))
    };
    let Ok(snap) = d.snapshot() else { return 0 };
    let failing: Vec<(String, String)> = snap
        .tasks
        .iter()
        .filter(|t| t.state == engine::status::TaskState::Error && autoretry::is_transient(&t.err))
        .filter_map(|t| snap.path_map.get(&t.gid).map(|p| (t.gid.clone(), p.clone())))
        .filter(|(_, p)| wanted.contains(p))
        .collect();
    // 记账只在路径不再需要时清（已移出引擎 / 已完成 / 已从 intent 删掉）。活跃、等待中的重试
    // 必须保留它的次数：否则源端不回数据时每次都要活满超时才报错，计数每拍清零，上限形同虚设。
    let live: BTreeSet<String> = snap
        .path_map
        .values()
        .filter(|p| wanted.contains(*p) && !complete.contains(*p))
        .cloned()
        .collect();
    tracker.retain_failing(&live);

    let mut requeued: Vec<String> = Vec::new();
    // 每个提交操作解析一次、整批复用（规格 §5.3）。**惰性**：`failing` 为空是常态，
    // 循环外无条件 `resolve_ips` 会让每拍（5 秒）都做一次 DNS——解析慢时 tick 自己被阻塞。
    // 只在真要重试时才解析。
    let mut ips: Option<Vec<std::net::IpAddr>> = None;
    for (gid, path) in &failing {
        if !tracker.should_retry(path, now, scale) {
            continue;
        }
        let Some(f) = m.files.iter().find(|f| &f.path == path) else { continue };
        let ips = ips.get_or_insert_with(|| resolve_ips(&m.base_url));
        let Some(pf) = plan_file(&m, f, ips.as_slice(), &counters) else { continue };
        // 先记一次再动手：哪怕下面失败，也按退避排下一次，不会每拍都敲。
        tracker.record_attempt(path, now, scale);
        if d.remove(gid).is_err() {
            continue;
        }
        let Ok(new_gid) = d.add(&pf.uris, &pf.dir, &pf.out, &pf.opts) else {
            continue;
        };
        // 开工时看到的「暂停全部」已经过时了（三段 RPC 之间用户随时可以按）：进锁复查，
        // 按住就立刻暂停这个新任务。暂停不成功则摘掉——宁可不重试，也不让它在按住期间写盘。
        let held = lock(kernel).hold_auto_retry;
        if held && d.pause(&new_gid).is_err() {
            let _ = d.remove(&new_gid);
            continue;
        }
        requeued.push(path.clone());
    }
    if !requeued.is_empty() {
        let mut k = lock(kernel);
        for p in &requeued {
            reopen_for_download(&mut k, p);
        }
        // 日志只记数量与最大次数，不记路径（诊断日志不落文件名）。
        let max_attempt = requeued.iter().map(|p| tracker.attempts(p)).max().unwrap_or(0);
        crate::diagnostics::log(
            "auto_retry",
            &[
                ("files", requeued.len().to_string()),
                ("max_attempt", max_attempt.to_string()),
            ],
        );
    }
    requeued.len()
}

/// 起自动重试线程（图形客户端的内核用；命令行工具自有一套收尾语义，不起）。
pub fn spawn_auto_retry(kernel: Arc<Mutex<Kernel>>) {
    thread::Builder::new()
        .name("auto-retry".to_string())
        .spawn(move || {
            let mut tracker = autoretry::Tracker::default();
            loop {
                thread::sleep(AUTO_RETRY_TICK);
                auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 1.0);
            }
        })
        .expect("起自动重试线程失败");
}

// ---------------------------------------------------------------------------
// 下载中换腿（规格 D5 / D8；纯判据在 `legtrim.rs`、腿表解析在 `engine/legs.rs`）
// ---------------------------------------------------------------------------

/// 拍间隔：10 s（规格 D5）。
///
/// 为什么不是更短：每拍对每个任务至少发一次 `getServers`，而 aria2 的腿速率是 1 s 粒度
/// 的滑动平均——拍子比测量窗口还短只会读到半成品，判据（连续 2 拍慢）也会被噪声触发。
/// 也不是更长：摘腿要等 5 拍（暖机 3 + 去抖 2），10 s 一拍才让「一次误判的代价」停在
/// 一分钟量级。
const LEG_TRIM_TICK: Duration = Duration::from_secs(10);

/// 每 6 拍（60 s）重新解析一次域名，补新 IP（规格 D8）。
///
/// 域名腿是唯一的解析入口：提交时解析出的 IP 集合会过期（镜像扩缩容、线路切换），
/// 而判据只能在**已有候选**里挑，所以候选必须周期刷新。为什么要 60 s：解析是网络 I/O，
/// 每拍都做等于给这条后台线程加一份无界的 DNS 负担；而按 DNS TTL 的量级，一分钟内
/// 新 IP 基本也该可见了。
const LEG_RESOLVE_EVERY: u64 = 6;

/// 解析一条**域名腿**的候选 IP。测试用 `TEST_LOOKUP` 注入假解析（单测不许碰网络），
/// 生产走真解析。
///
/// 为什么要这条缝而不是直接写死真解析：这段代码的**全部**价值就是「解析出的 IP 会被造成
/// 腿」，不注入就只能测到「什么都没发生」那一半。与 [`resolve_ips`] 同一份纪律。
///
/// 生产侧走 [`delivery::resolve_for_base`]（`system_lookup` 是私有的）：它顺带带上
/// scheme 门与 IP 字面量门——域名腿本身不可能是 IP 字面量，门形同兜底。
fn resolve_leg_ips(leg_uri: &str) -> Vec<std::net::IpAddr> {
    #[cfg(test)]
    {
        let lookup = TEST_LOOKUP.with(|t| *t.borrow());
        if let Some(lookup) = lookup {
            return delivery::resolve_for_base_with(lookup, leg_uri);
        }
    }
    delivery::resolve_for_base(leg_uri)
}

/// 起换腿线程（图形客户端的内核用；命令行工具只起 worker、没有这条协议循环，
/// 与 [`spawn_auto_retry`] 的接线口径一致，不起）。
///
/// 形状照 [`spawn_auto_retry`]：`loop { sleep; tick }`。拍号、判据状态、解析缓存都活在
/// 闭包里——它们是**这一次下载过程的历史**，进 `Kernel` 只会让每个 RPC 都跟着持锁读写。
pub fn spawn_leg_trim(kernel: Arc<Mutex<Kernel>>) {
    thread::Builder::new()
        .name("leg-trim".to_string())
        .spawn(move || {
            let mut beat: u64 = 0;
            let mut states: BTreeMap<String, legtrim::TrimState> = BTreeMap::new();
            let mut resolved: BTreeMap<String, (u64, Vec<std::net::IpAddr>)> = BTreeMap::new();
            loop {
                thread::sleep(LEG_TRIM_TICK);
                // 第 1 拍在启动 10 s 后：刚提交的任务还没有速率样本，早了只能读到 0。
                beat += 1;
                leg_trim_tick(&kernel, &mut states, &mut resolved, beat);
            }
        })
        .expect("起换腿线程失败");
}

/// 跑一拍换腿，返回这一拍真正提交成功的动作数。
///
/// 三条纪律：
/// 1. **RPC 全在锁外**：进锁只拷两个 `Arc` 句柄（引擎 + 计数器），拷完立刻出锁。
///    `getServers` / `getUris` / `changeUri` 都是几十毫秒量级的网络往返，持锁会让协议循环停摆。
/// 2. **任何失败都只是「这一拍不动作」**：引擎刚被换掉、任务刚收尾、`getUris` 读不到、
///    `changeUri` 被拒，一律不 panic、不重试、不写日志刷屏——下一拍的 `getServers` 是事实来源，会对账。
/// 3. `beat` 是**参数**：线程里 +1 递推，测试里直接给数，不碰真实时钟。
fn leg_trim_tick(
    kernel: &Arc<Mutex<Kernel>>,
    states: &mut BTreeMap<String, legtrim::TrimState>,
    resolved: &mut BTreeMap<String, (u64, Vec<std::net::IpAddr>)>,
    beat: u64,
) -> usize {
    // 锁内只做拷贝。
    let (d, counters) = {
        let k = lock(kernel);
        let Some(d) = k.daemon.clone() else {
            return 0;
        };
        (d, Arc::clone(&k.counters))
    };
    // `active()` 只发 `tellActive`，表里本就只有活跃任务；仍按状态滤一遍，让「应当正在
    // 下载」的定义只有 `active_tasks` 一处。`Waiting` 也收：它含真排队与手动暂停，
    // 而对这两个换腿都无害（`changeUri` 只是改 URI 列表，不碰进度）——
    // 真正的门是下面「必须已有 IP 腿」，暂停中的任务同样满足才说明它当初带了备胎。
    let Ok(tasks) = d.active() else {
        return 0;
    };
    let gids: Vec<String> = tasks
        .iter()
        .filter(|t| t.state == TaskState::Active || t.state == TaskState::Waiting)
        .map(|t| t.gid.clone())
        .collect();
    let mut actions = 0usize;
    for gid in &gids {
        // 引擎可能刚被换掉（GID 还在旧实例里）⇒ 这个任务这一拍跳过。
        let Ok(legs) = d.servers(gid) else {
            continue;
        };
        if legs.is_empty() {
            continue;
        }
        // 🔴 **门槛：必须已经有一条 IP 字面量腿。**
        // 只有域名腿意味着当初提交时没带 `Host` 头（`planner::plan_file` 只在真有 IP 备胎
        // 时才塞 `header`）。这种任务上加 IP 腿会把请求打到裸 IP 却不带 Host，对象存储按
        // Host 分桶，取不到文件。宁可不动——少摘一次，不制造一个 404。
        if !legs.iter().any(|leg| leg_is_ip_literal(&leg.uri)) {
            continue;
        }
        // 域名腿既是**解析入口**，也是 `Host` 头的来源（`host_header_for` 取的就是 base 的
        // authority）。一条都没有 ⇒ 不知道 Host 是什么，造出来的 IP 腿也用不了。
        //
        // 🔴 必须从 `getUris`（任务的**完整** URI 列表）里找，**不能从腿表里找**：
        // `getServers` 只列「这一拍真有连接」的 URI，域名腿这一拍没连接就根本不出现
        // ⇒ 从腿表里 find 会永远找不到 ⇒ 该任务**静默永久跳过**（无错误、无日志，
        // 表现只是这条特性整条失效）。`getServers` 列的是「用过的源」，不是「任务的源」。
        let Ok(uris) = d.get_uris(gid) else {
            // 引擎可能刚被换掉 / 任务刚收尾：这一拍跳过，下一拍再说。
            continue;
        };
        let Some(domain_leg) = uris.into_iter().find(|u| !leg_is_ip_literal(u)) else {
            // 任务本身没有域名腿（提交时只给了 IP 字面量）：没有 Host 来源，也无从解析。
            continue;
        };
        // 解析缓存的键是**来源**（`scheme://host[:port]`），不是完整文件 URI，理由见
        // [`origin_of`]：交付码与路径每个文件都不同，用文件 URI 当键等于每文件各解析一次，
        // 缓存还会跨批次只增不减。取不出来源 ⇒ 这个任务这一拍跳过（与"没有域名腿"同处理）。
        let Some(origin) = origin_of(&domain_leg) else {
            continue;
        };
        // 何时重新解析：缓存里没有这个来源，或者到了重解析那一拍。
        let need_resolve = match resolved.get(&origin) {
            None => true,
            // 同一拍里多个任务共用同一个来源时只解析一次：否则 60 s 那一拍会敲 N 次 DNS。
            Some((at, _)) => *at != beat && beat % LEG_RESOLVE_EVERY == 0,
        };
        if need_resolve {
            // 解析失败/空结果是「这一拍没有新候选」，不是错误：旧候选仍然可用。
            let ips = resolve_leg_ips(&domain_leg);
            // 上限只是**防呆**：域名腿的来源量级是个位数（同一次交付最多几条域名腿），
            // 正常永远够不到 64。真要撞上（例如将来支持大量不同 host 的 metalink），
            // 全清比 LRU 便宜且不会留下"以为有缓存其实已过期"的条目——只是一次白敲 DNS。
            if resolved.len() > 64 {
                resolved.clear();
            }
            resolved.insert(origin.clone(), (beat, ips));
        }
        let Some((_, ips)) = resolved.get(&origin) else {
            continue;
        };
        // 造候选腿：换主机、保留端口/路径。再过一道 IP 字面量门（防 helper 将来被改成
        // 返回域名），并跳过已经在腿表里的——重复 URI 白占一个位置，也让「补了哪些」
        // 在日志里看不出真动作。
        let mut candidates: Vec<String> = Vec::new();
        for ip in ips {
            let Some(uri) = delivery::uri_with_host(&domain_leg, ip) else {
                continue;
            };
            if !leg_is_ip_literal(&uri)
                || legs.iter().any(|leg| leg.uri == uri)
                || candidates.contains(&uri)
            {
                continue;
            }
            candidates.push(uri);
        }
        let Some(plan) = states
            .entry(gid.clone())
            .or_default()
            .plan(beat, &legs, &candidates)
        else {
            continue;
        };
        // 失败 ⇒ **丢弃这一拍**。`plan` 已经把摘掉的腿登记进冷却，所以下一拍不会立刻重来；
        // 而真正的事实来源是下一次 `getServers`——对不上账，判据自然什么也不做。
        //
        // 诊断按**引擎的实际计数**记（`[removed, added]`），不按 `plan.del/add` 的长度：
        // 实测 `changeUri` 对不认识的 URI 不报错、回 `[0, 0]`，"我方计划删了两条"不等于
        // "引擎真的删了两条"；按计划记会让唯一能回答"换腿生效没"的读数失真。
        if let Ok((removed, added)) = d.change_uri(gid, 1, &plan.del, &plan.add) {
            counters.record_leg_trim(removed as usize, added as usize);
            actions += 1;
        }
    }
    // 拍末清理：任务结束/被删就丢掉它的判据状态。这是本线程**唯一**的长期增长点。
    // 解析缓存不主动清理：键是来源（几条），量级可忽略；清了只会让下一拍白敲 DNS。
    // 上限兜底在插入处（`resolved.len() > 64` 就全清）。
    let live: BTreeSet<&str> = gids.iter().map(|gid| gid.as_str()).collect();
    states.retain(|gid, _| live.contains(gid.as_str()));
    actions
}

/// 从一条文件 URI 取出**来源**（`scheme://host[:port]`）——解析缓存的键。
///
/// 为什么不能用完整文件 URI 当键：交付码与路径每个文件都不同，1 万文件的批次会让缓存常驻
/// 1 万条、跨批次只增不减；而「同一拍多个任务共用一条域名腿只解析一次」也永远不成立
/// （每个文件 URI 都不同）⇒ 每 6 拍 × 活跃文件数次阻塞 DNS。来源的量级是个位数。
///
/// 只认 `http(s)://<authority>...`：取 `://` 之后、第一个 `/`、`?`、`#` 之前的那段。
/// 取不出来（非 http(s)、没有 authority、带 userinfo）⇒ `None`，调用方跳过这个任务：
/// 猜一个来源去解析比不解析更坏（会把别人的 IP 造成这条腿）。
fn origin_of(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    // `split` 至少产出一段，取不到就是空串——空 authority 在下面被挡掉。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// 现在（unix 秒）。时钟回拨由 `update::should_check` 那一支兜住。
/// 毫秒时钟（`errors::Recent` 的窗口用它）。单调性不重要：只用差值。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// **把一个卡死的引擎换掉**：关掉旧的、起一个新的、把句柄换过去。
///
/// 与 `ensure_engine` 的分工：那个只在**没有句柄**时启动；这个**明知有句柄也要换**
/// （句柄还在、端口还占着，但引擎不应答）。
///
/// 顺序与锁的边界都是承重的：
///   1. **锁内（短）**：拷出走启动要用的输入、把旧句柄摘走。纯拷贝，无 I/O。
///   2. **锁外**：`close()` 旧的。⚠️ 对一个**卡死的**引擎，`close` 最坏要
///      `RPC_TIMEOUT`(10 s) ＋ `CLOSE_FALLBACK`(5 s) —— 持着锁做它会让协议循环
///      整整停摆十几秒，那正是本仓反复否掉的那类写法。
///   3. **锁外**：起新的。起不来就把旧句柄**装回去** —— 宁可退回到"断开"
///      （自愈下一拍还会再试），也不能让内核手里没有句柄：`read_engine` 在 `None` 上
///      回的是 `ENGINE_NOT_STARTED`（「先 enqueue 才会起引擎」），那是另一句错话。
///
/// ⚠️ **先收旧的、再起新的**（不是反过来）。反过来的话两个 aria2 会同时活着，
///   而旧的只是"不应答"、不一定"没在下载"——同时写同一批文件是数据损坏级的风险。
pub fn replace_engine(lang: crate::i18n::Lang, kernel: &Arc<Mutex<Kernel>>) -> Result<Arc<Daemon>, ErrorBody> {
    let (start, old) = {
        let mut k = lock(kernel);
        let start = k.engine_start();
        let old = k.take_daemon();
        (start, old)
    };
    if let Some(o) = &old {
        // 关不掉也往下走：它本来就不应答复，`close` 内部的兜底会 kill 掉它。
        let _ = o.close();
    }
    match start_engine_off_lock(lang, start) {
        Ok(fresh) => {
            let mut k = lock(kernel);
            k.daemon = Some(Arc::clone(&fresh));
            k.set_engine_disconnected(false, "engine_restarted");
            drop(k);
            // 旧引擎里的队列已经没了：据 `intent` 补回（`-c` 续传）。
            restore_queue_after_replace(kernel);
            Ok(fresh)
        }
        Err(e) => {
            if let Some(o) = old {
                let mut k = lock(kernel);
                k.daemon = Some(o);
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// 校验工作线程：**check 在锁外，写入在锁内**（契约 §5.2，承重）
// ---------------------------------------------------------------------------

/// 派活给校验线程。
///
/// ⚠️ **这里的调用点与锁的作用域在代码上就是分开的**（简报点名要说明这一点）：
/// `verify::check` 在函数体内**第一件事**就被调用，而它上面**没有任何 `Mutex` guard**——
/// `kernel` 只被 clone 进闭包，锁是在 `check` **返回之后**才取的。
/// 一个把 `check` 挪进 `{ let mut k = kernel.lock()…; }` 里的实现，
/// 在 50 GB 批量下会让整个协议循环停住几分钟（每次 CRC64 是几分钟的磁盘 I/O），
/// 而**小夹具上完全看不出来**——`verify_runs_outside_the_lock` 是钉住它的那条测试。
pub fn spawn_verify_worker(kernel: Arc<Mutex<Kernel>>, rx: Receiver<VerifyJob>, tx: Sender<VerifyJob>) {
    thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            // ── 锁外：几分钟量级的 CRC64 磁盘 I/O ──────────────────────────────
            // 这一行**必须**留在任何 `kernel.lock()` 之前。它下面一行才是取锁。
            let (res, records) = verify::check(&job.dir, &job.files);
            // ── 锁内：只有短写入 ────────────────────────────────────────────
            let (retry_files, retry_first) = {
                let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);

                // 1) 待写记录（只有校验通过的才在 `records` 里，见 `verify::check`）
                for (path, entry) in records {
                    k.state.put(&path, entry);
                }
                if let Err(e) = k.state.save() {
                    eprintln!("benagen-core: 写状态文件失败（不影响本次校验结论）: {e}");
                }
                k.complete_dirty = true;

                match job.first {
                    // 复校验轮：两轮并起来**才是**最终归类（简报第 9 条）。
                    Some(first) => {
                        let merged = verify::merge(first, res);
                        commit(&mut k, merged);
                        for f in &job.files {
                            k.verifying.remove(&f.path);
                            k.verified.insert(f.path.clone());
                        }
                        (Vec::new(), None)
                    }
                    // 第一轮：不符的那些要**自动重新入队一次**
                    None => {
                        let want: Vec<String> = res
                            .failures()
                            .into_iter()
                            .filter(|p| !k.retried.contains(p))
                            .collect();
                        // 全部都能在本批里找到（`failures` 是 `res` 的子集，而 `res`
                        // 就是对 `job.files` 逐条判出来的）
                        let retry: Vec<delivery::File> = job
                            .files
                            .iter()
                            .filter(|f| want.contains(&f.path))
                            .cloned()
                            .collect();
                        for f in &retry {
                            k.retried.insert(f.path.clone());
                        }

                        // ⚠️ **第一轮结果必须无条件提交**，不能只在"没有要重试的"时候提交。
                        // `ok`/`unverifiable`/`unreadable` **都不在** `failures()` 里，
                        // 它们永远不会进复校验轮——只在 `retry.is_empty()` 那一支提交的话，
                        // 一个混合批次里这些路径**六类里一条都不出现**（违全局约束 4
                        // 「不得静默少交」），而且会**永久留在 `verifying`**，让落地监听器
                        // 之后再也不会复校验它们。真实交付每批必然混合，这不是边角场景。
                        //
                        // 提交前留一份给复校验轮 `merge`（`merge` 的第一参就是第一轮结果）。
                        let first = if retry.is_empty() { None } else { Some(res.clone()) };
                        commit(&mut k, res);

                        for f in &job.files {
                            // 只有**不在重试集里**的路径才算最终完成；重试中的那些要一直
                            // 留在 `verifying` 里，免得落地监听器在复校验之前又派一次活。
                            if !retry.iter().any(|r| r.path == f.path) {
                                k.verifying.remove(&f.path);
                                k.verified.insert(f.path.clone());
                            }
                        }
                        (retry, first)
                    }
                }
            };

            // ── 锁外：重新入队（要发 RPC，持锁做 HTTP 会让协议循环冻住）────────
            if !retry_files.is_empty() {
                let manifest = {
                    let k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                    k.manifest.clone()
                };
                if let Some(m) = manifest {
                    let (daemon, counters) = {
                        let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                        (k.read_engine().ok(), Arc::clone(&k.counters))
                    };
                    if let Some(d) = daemon {
                        // 每个提交操作解析一次、整批复用（规格 §5.3）。
                        let ips = resolve_ips(&m.base_url);
                        for f in &retry_files {
                            // 落盘路径 = manifest.path 原文（契约 §3.1）
                            let Some(pf) = plan_file(&m, f, &ips, &counters) else {
                                continue;
                            };
                            if let Err(e) = d.add(&pf.uris, &pf.dir, &pf.out, &pf.opts) {
                                eprintln!(
                                    "benagen-core: 校验不符后重新入队失败（{}）: {e}",
                                    f.path
                                );
                            }
                        }
                    }
                }
                // 复校验轮：**带上第一轮的结果**，让工作线程用 `verify::merge` 把两轮并起来
                // （简报第 9 条）。少了 `first`，`merge` 在生产路径上就没有调用者，
                // 而"复校验的结论才是最终结论"这条语义也就落空了。
                let _ = tx.send(VerifyJob {
                    dir: job.dir.clone(),
                    files: retry_files,
                    first: retry_first,
                });
            }
        }
    });
}

/// 把一个校验结果并入累积结果：**先把这个结果涉及的路径从六类里全部摘掉，再写入**。
///
/// ⚠️ 「先摘旧类」是**六类互斥的唯一机制**。同一个路径先后出现在两类里
/// （例如第一轮 `bad`、复校验之后 `ok`）会让报告自相矛盾——客户看到的是
/// "这个文件既不符、又通过"。`mixed_batch_classifies_every_file_exactly_once`
/// 的「各类两两不相交」断言就是钉住它的（把 `retain` 改成空操作，那条当场红）。
///
/// 摘除集合取自 **`res` 自己**而不是调用方另传的路径表：`verify::check` 六类互斥穷尽，
/// 所以 `res` 里出现的路径就是它分类过的全部路径。这样复校验轮传入的 `merge` 结果
/// （**含整批的 `ok`/`unverifiable`/`unreadable`**）也能安全重放——不会与第一轮
/// 已经提交的那一份重复。
fn commit(k: &mut Kernel, res: verify::CheckResult) {
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for cls in [
        &res.ok,
        &res.bad,
        &res.missing,
        &res.size_mismatch,
        &res.unverifiable,
        &res.unreadable,
    ] {
        touched.extend(cls.iter().cloned());
    }
    let mut acc = std::mem::take(&mut k.verify);
    for cls in [
        &mut acc.ok,
        &mut acc.bad,
        &mut acc.missing,
        &mut acc.size_mismatch,
        &mut acc.unverifiable,
        &mut acc.unreadable,
    ] {
        cls.retain(|p| !touched.contains(p));
    }
    acc.ok.extend(res.ok);
    acc.bad.extend(res.bad);
    acc.missing.extend(res.missing);
    acc.size_mismatch.extend(res.size_mismatch);
    acc.unverifiable.extend(res.unverifiable);
    acc.unreadable.extend(res.unreadable);
    k.verify = acc;
}

/// 落地监听器：文件下载完成即校验（设计规格 §8.2）。
///
/// 轮询间隔与壳的快照间隔（200 ms）同量级——这也是 Go 版已验证的形态。
pub fn spawn_landing_watcher(kernel: Arc<Mutex<Kernel>>, tx: Sender<VerifyJob>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(200));

        let (daemon, manifest, dir) = {
            let k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
            (k.daemon.clone(), k.manifest.clone(), k.download_dir.clone())
        };
        let (Some(d), Some(m)) = (daemon, manifest) else {
            continue;
        };
        if kernel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .engine_disconnected
        {
            continue;
        }
        // 快照**不持内核锁**：它是最坏 10 秒的 RPC（契约 §4.3）。
        let Ok(snap) = d.snapshot() else {
            continue;
        };

        let files: Vec<delivery::File> = {
            let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
            let mut picked: Vec<delivery::File> = Vec::new();
            // `snap.tasks` 里的**重复 GID 已经在 `Daemon::snapshot` 里折过**——
            // 那个不变式由生产者持有，消费者不必也不该再自己折一次
            // （曾经这里与 `transfer_list` 各折一次、另两个消费点不折：
            // 同一条规则多份实现，正是契约 §1.7 的教训）。
            for t in &snap.tasks {
                // 只有 aria2 明确说传完了才算"落地"
                let done = snap
                    .raw_status
                    .get(&t.gid)
                    .map(|s| s == "complete")
                    .unwrap_or(false);
                if !done {
                    continue;
                }
                let Some(path) = snap.path_map.get(&t.gid) else {
                    continue;
                };
                if k.verifying.contains(path) || k.verified.contains(path) {
                    continue;
                }
                let Some(f) = m.files.iter().find(|f| &f.path == path) else {
                    continue;
                };
                k.verifying.insert(path.clone());
                picked.push(f.clone());
            }
            picked
        };

        if !files.is_empty() {
            let _ = tx.send(VerifyJob {
                dir,
                files,
                first: None,
            });
        }
    });
}

// ---------------------------------------------------------------------------
// 分派
// ---------------------------------------------------------------------------

/// 取内核锁。**中毒恢复**：校验线程里一次 panic 不该让整个内核再也回不了话
/// （客户看到的是"点了没反应"，比一条结构化错误差得多）。
fn lock(kernel: &Arc<Mutex<Kernel>>) -> MutexGuard<'_, Kernel> {
    kernel.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 搬运是否在跑（与 `op_enqueue` / 后台入队 / 自愈补回共用一个判据）。
fn migrating(k: &Kernel) -> bool {
    k.migrate.lock().unwrap_or_else(PoisonError::into_inner).running
}

fn with_kernel<T>(
    kernel: &Arc<Mutex<Kernel>>,
    f: impl FnOnce(&mut Kernel) -> Result<T, ErrorBody>,
) -> Result<T, ErrorBody> {
    f(&mut lock(kernel))
}

// ---------------------------------------------------------------------------
// `plan` 的锁外执行（I-2 的核心）
// ---------------------------------------------------------------------------

/// **唯一**调 `Planner::plan` 的地方。**必须在锁外调用。**
///
/// ⚠️ **为什么不能持锁**（契约 §5.2「校验不得占住协议循环或任何长持有的锁」）：
/// `plan` 对清单里**每个 crc64 为空的文件**发一次 HEAD，每次上限
/// `delivery.rs` 的 `FETCH_TIMEOUT` = 30 秒（`planner.rs` 的补齐那一段）——N 个文件就是
/// **30 秒 × N** 的网络 I/O。服务端"回读失败"导致清单里 crc64 为空是**生产上真实存在**
/// 的形态（e2e 的 `Faults::no_crc` 夹具就是照它造的），N=10 就是 5 分钟以上。
/// 持着内核锁跑它，校验线程与落地监听器（都要取同一把锁）会一起停摆。
///
/// 取数（本函数）在锁外、装结果（`complete_from`）在锁内——中间那一段窗口里
/// `complete` 还是旧值，所以调用方必须先把 `complete_dirty` **认领成 `false`**
/// （见 [`ensure_complete`] 的三段式注释）。
///
/// `state` 是一份**副本**（[`Kernel::plan_inputs`] 在锁内取的），
/// `plan` 内部那次纵深防御的 `bind` 因此只作用于副本。
fn plan_off_lock(
    dir: PathBuf,
    mut state: FileState,
    m: &Manifest,
    strict: bool,
    fill_budget: usize,
) -> planner::Todo {
    let transport = UreqTransport::new();
    let mut p = Planner {
        dir,
        state: Some(&mut state),
        transport: &transport,
    };
    p.plan_with_budget(m, PlanOptions { strict }, fill_budget)
}

/// **一拍最多为多少个「crc64 为空」的文件发 HEAD。**
///
/// 这个数是"**一拍最长能挡多久**"的预算：HEAD 每次上限 30 秒，取 8 ⇒ 最坏 4 分钟、
/// 典型（每次几十毫秒）**不到一秒**。取更大的值不会让总时间变短（总量由文件数决定），
/// 只会让**单拍**更长 —— 而"单拍有多长"正是客户看到的东西：壳的命令是同步的，
/// 一拍挡住主线程多久，窗口就「未响应」多久。
///
/// ⚠️ 它只管**界面读路径**（`ensure_complete`）。`load_delivery` / `enqueue` 走
/// **不限预算**那一支：那两个是客户**主动发起**的动作，语义上就是"把这一批准备齐"
/// ——而且到那时后台那条补齐线程（[`spawn_crc_filler`]）通常已经补完了。
const CRC_FILL_BUDGET_PER_READ: usize = 8;

/// **后台补齐 crc64**：一小批一小批地把清单里空着的 crc64 从服务端补回来。
///
/// 为什么要有一条**后台**线程（而不是只靠界面那一拍的预算）：
///   · 界面读路径的预算只保证"不卡"，**不保证"会走完"** —— 那一拍只有用户动一下才发生；
///   · 补齐必须**在下载之前**完成（没补到的文件这一轮只能比大小，等于少一半校验），
///     所以它得在"加载完"到"用户点下载"之间的那几秒里自己往前走。
///
/// 客户提的「少数量、多批次、逐步加载」落在**这里**：每拍 8 个、跑完就睡。
/// ⚠️ 界面那一侧**不需要**分批 —— 实测 3000 行全选只要 6 ms（`check_big_screen.sh`）。
///
/// 停的条件：**清单里再没有空 crc64** 就退回慢节拍（1 秒一次只做一次检查，不跑 plan）。
/// 补不到的文件**每拍都会重试**（`adopt_filled_crc64` 只记成功的那些），
/// 所以一次网络抖动不会把它钉成永久"无法校验"。
pub fn spawn_crc_filler(kernel: Arc<Mutex<Kernel>>) {
    std::thread::Builder::new()
        .name("crc-filler".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            // 锁内看一眼：还有没有等着补的？（**只读**，立刻放锁）
            let pending = {
                let k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                k.manifest
                    .as_ref()
                    .is_some_and(|m| m.files.iter().any(|f| f.crc64.is_empty()))
            };
            if !pending {
                continue;
            }
            fill_crc_batch(&kernel);
        })
        .expect("起 crc 补齐线程失败");
}

/// 跑**一拍**补齐（锁外做网络 I/O，锁内取数与装结果）。
///
/// ⚠️ 与 [`ensure_complete`] 分开写、而不是复用它：那一条是**按需**触发的
/// （`complete_dirty` 门控），而这一条是**推进**用的 —— 它不等任何人的信号，
/// 只要还有空着的 crc64 就往前走。把两者并成一个函数，会让"谁在推动"这件事读不出来。
fn fill_crc_batch(kernel: &Arc<Mutex<Kernel>>) {
    let work = {
        let mut k = lock(kernel);
        match k.manifest.clone() {
            Some(m) => (k.plan_inputs(&m), m),
            None => return,
        }
    };
    let (dir, state) = work.0;
    let todo = plan_off_lock(dir, state, &work.1, false, CRC_FILL_BUDGET_PER_READ);
    let mut k = lock(kernel);
    adopt_filled_crc64(&mut k, &todo);
    // 这一拍补到了东西 ⇒ `complete` 就该重算（否则界面要等到下一次 `complete_dirty`
    // 才看得到"这几个已经能校验了"）。
    k.complete = complete_from(&todo);
    k.complete_dirty = false;
}

/// 若 `complete` 已过期，**在锁外**重算它。
///
/// ⚠️ **三段式**：① 锁内认领 → ② 锁外跑 `plan` → ③ 锁内装结果。
///
/// 为什么 `complete_dirty` 要在第 ① 段就**认领成 `false`**（而不是留到第 ③ 段再清）：
/// 中间那段窗口可能很长（N×30 秒）。若留着 `true`，窗口里发起的另一次
/// `ensure_complete` 会**重复**跑一遍全量 plan（用户的每一次点击都再等 N×30 秒）。
/// 认领走之后，窗口里校验线程提交结果时置上的 `true` 会被第 ③ 段**原样保留**——
/// 下一次读会重算，不会用一份陈旧的结果把"又过期了"静默吞掉。
///
/// 读循环是**同步**的，同一个内核里不可能有两次 `ensure_complete` 真正交错，
/// 所以这里不需要"先摘旧类再写入"那样的累积器。
///
/// ⚠️ **`complete_dirty == false` 时也不是什么都不做**（阶段 D 任务 A）：那个布尔量说的
/// 是"**状态文件**变了没有"，**不是**"**磁盘**变了没有"。磁盘被内核之外的力量改动
/// （客户在访达里删文件、换机器、手滑清空目录）没有任何东西会置位它——见
/// [`recheck_complete_on_disk`]。
fn ensure_complete(kernel: &Arc<Mutex<Kernel>>) {
    let work = {
        let mut k = lock(kernel);
        if !k.complete_dirty {
            recheck_complete_on_disk(&mut k);
            return;
        }
        k.complete_dirty = false; // 认领这次计算
        match k.manifest.clone() {
            Some(m) => (k.plan_inputs(&m), m),
            // 还没有批次：`complete` 只能是空集（对应契约 §1.7 的唯一推导点）
            None => {
                k.complete.clear();
                return;
            }
        }
    };
    let (dir, state) = work.0;
    // 🔴 **带预算**（见 `CRC_FILL_BUDGET_PER_READ`）：这一条挂在**界面读路径**上
    //（`list_dir` / `get_tree` / `enqueue` 都会走它），所以它**不许**为了一批文件的 HEAD
    // 把壳的主线程挡上几分钟。没补到的交给 `spawn_crc_filler` 下一拍接着补。
    let todo = plan_off_lock(dir, state, &work.1, false, CRC_FILL_BUDGET_PER_READ);
    let mut k = lock(kernel);
    // 🔴 **把这一轮从服务端补到的 crc64 写回清单**（2026-10-08）。
    //
    // 少了这一步，`plan` 补到的值**只活在那份克隆上**（`planner.rs` 的补齐那一段对
    // `f.clone()` 动的手）—— 清单本身一个字节没变 ⇒ 下一次 `complete_dirty` 再跑 plan 时，
    // **同一批文件又得重问一遍服务器**。
    //
    // ⚠️ **而 `complete_dirty` 在每一批校验结果写回之后都会被置起**（本文件上方那个
    //    校验工作线程的收尾）：一次大下载里它会反复变真 ⇒ 「每次勾选/全选 ⇒ `get_tree`
    //    ⇒ `ensure_complete` ⇒ 全量 plan ⇒ N 次 HEAD」会**反复发生**，代价 ∝ 文件数。
    //    壳的每一个命令都是**同步**的（Tauri 的 `ResponseTag::block` 就在收 IPC 的那条
    //    线程上回），而 IPC 走主线程 ⇒ 这一串的直接表现是**窗口「未响应」**。
    //    N=10 就已经是 5 分钟以上（`plan_off_lock` 的文档里写着这条账）。
    //
    // ⚠️ **只写成功的那些**：补不到的（`crc64` 仍为空）**故意不写**——那是一句"这次没问到"，
    //    换一批校验结果之后就是一次新的机会。把失败也缓存下来，等于把一次网络抖动
    //    **钉成永久"无法校验"**（那会让客户在盘上明明有数据的情况下被判成 `unverifiable`）。
    adopt_filled_crc64(&mut k, &todo);
    k.complete = complete_from(&todo);
}

/// 把本轮 `plan` 从服务端补到的 crc64 **写回清单** —— 下一次规划不再为同一批文件重发 HEAD。
///
/// 纯记账：不做判定、不碰 `complete`（那是 [`complete_from`] 的事）。
/// 复杂度 O(N log N)：先把补到的收成一张表，再扫一遍清单，**不是**对每个文件线性扫清单。
fn adopt_filled_crc64(k: &mut Kernel, todo: &planner::Todo) {
    let filled: std::collections::BTreeMap<&str, &str> = todo
        .items
        .iter()
        .filter(|i| !i.file.crc64.is_empty())
        .map(|i| (i.file.path.as_str(), i.file.crc64.as_str()))
        .collect();
    if filled.is_empty() {
        return; // 这一轮一个都没补到：什么都不用做（下次照常重试）
    }
    let Some(m) = k.manifest.as_mut() else {
        return;
    };
    for f in &mut m.files {
        if f.crc64.is_empty() {
            if let Some(c) = filled.get(f.path.as_str()) {
                f.crc64 = (*c).to_string();
            }
        }
    }
}

/// **廉价的磁盘核对**（阶段 D 任务 A）：把 `k.complete` 里"盘上**现在**已经不满足
/// 完整性"的路径摘掉。实现在 [`planner::recheck_complete`]，判据与 `plan` 共用同一条
/// 判定链（不可能漂移），且**只 stat 磁盘、一个网络请求都不发**（D-1）。
///
/// ⚠️ **为什么挂在这里（`ensure_complete` 里），而不是挂在消费 `complete` 的三处**：
/// `complete` 的**客户端会走到的**三个消费点（`list_dir`、`get_tree`、`enqueue`）**都**先调
/// `ensure_complete`。挂在它里面 ⇒ **结构上**不可能漏掉任何一处。挂在调用点就变成
/// "三份调用要记得同步"——那正是契约 §1.7 反复吃过的教训（同一条规则多份实现必然漂移），
/// 而这一处漏掉的代价是**静默少交**：诊断报告 §7 第 6 条（盘上一个字节都没有，
/// `get_tree` 却回 100% / 全部 `complete`）走的正是 `get_tree` 那条路。
///
/// ⚠️ **`complete_dirty` 的门控没有、也不许被这一步取代**：它省的是 `plan` 的钱
/// （按文件数 × 最多 30 秒的网络 I/O），这里只补一层只碰磁盘的核对。
///
/// ⚠️ **只在 `complete_dirty == false` 的分支里调**：`dirty == true` 时紧接着那次
/// `plan` 本来就会从磁盘重算整份 `complete`，先摘一遍是纯粹的重复劳动。
///
/// ⚠️ 这一步在**锁内**做 —— **Ruling D3：接受，不修**（别在这里"顺手优化成锁外"）。
/// 代价是 `complete` 里每个路径一次 `stat`（上万文件是几十毫秒
/// 量级；`plan_inputs` 在同一个位置已经在做 O(文件数) 的 `State` 复制，先例同一量级），
/// 换来的是不必处理"锁外核对 → 装回时 `complete` 又被校验线程改过"的竞态。
/// 调用频率上它是安全的：只有 `list_dir` / `get_tree` / `enqueue([])` 走到这里，
/// 而界面上 200 ms 一拍的轮询走的是 `transfer_list`——那条路**不**调 `ensure_complete`。
///
/// ⚠️ **阶段 E 已复核这一条**（Ruling D3 原文："下阶段若要做「可选下载目录」，这条要
/// 一起复核 —— 因为那正好会让用户有机会把目录指到网络盘"；**阶段 E 就是那个下阶段**，
/// 而壳现在真的可以让用户把下载根指到任何地方）。**复核结论：仍然接受，不修。**
/// 理由与阶段 D 一致：① 默认仍是本地路径；② 移出锁要三段式（锁内认领 → 锁外算 →
/// 锁内装），而那**正是** `ensure_complete` 已经在用的复杂机制 —— 为一个未观测到的场景
/// 再加一层，与 C14/C15/C17 的教训相反；③ 它的失效形态是**变慢**，不是**变错**。
///
/// **代价如实记下（现在是"用户一步操作就能到达"的场景了）**：下载根落在 NFS / SMB 上时，
/// `list_dir` / `get_tree` / `enqueue([])` **每一次**都会在持锁的情况下对 `complete` 里
/// 每个路径 stat 一遍，而网络盘上的 stat 是秒级 ⇒ 内核的同步读循环停摆（校验线程、
/// 落地监听器、壳的 200 ms 轮询全排在它后面）。
///
/// **已在阶段 E 补文档**（这是本条的处置方式，不是"还没人管"）：
/// 设置窗口那一段的脚注（`macos/.../Presentation/DownloadDirectory.swift` 的
/// `sectionNote`）与 `macos/README.md` 的已知限制一节都写着"请选本机磁盘"。
fn recheck_complete_on_disk(k: &mut Kernel) {
    // 还没有批次：`complete` 只能是空集（与 `ensure_complete` 的另一条分支同一条规则）
    let Some(m) = k.manifest.as_ref() else {
        k.complete.clear();
        return;
    };
    planner::recheck_complete(&k.download_dir, Some(&k.state), m, &mut k.complete);
}

/// 握手：版本协商 + 语言落定（规格 §2.1）。
///
/// 语言优先级：`hello.language`（壳说的）> 内核里存的设置（客户上次选的）
/// > 系统 locale。解析结果既写回内核，也随响应回给壳——**两端不各算一次**。
fn op_hello(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    let mut hello = protocol::hello(params)?;
    let requested = protocol::requested_lang(params);
    // 语言从哪来：壳交的系统语言 > 本进程环境变量（CLI / 测试）。
    // ⚠️ 只在"跟随系统"时才用得到它；客户选了 zh/en 时 `resolve` 会直接返回那个值。
    let reported = protocol::requested_system_locale(params);
    let locale = reported
        .clone()
        .unwrap_or_else(crate::i18n::system_locale);
    with_kernel(kernel, |k| {
        // 记下来：设置页改「跟随系统」时还要用它（`op_set_settings`）。
        k.system_locale = reported;
        let lang = requested.unwrap_or(k.settings.language).resolve(&locale);
        k.language = lang;
        hello.language = lang;
        hello.language_chosen = k.language_chosen;
        to_value(&hello)
    })
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ErrorBody> {
    serde_json::to_value(v).map_err(|e| ErrorBody::new(codes::INTERNAL, format!("序列化失败: {e}")))
}

/// 分派一条请求。返回 `(响应, 是否结束)`。
/// 自动重启的预算用尽：**此刻**才允许说"下载引擎没有响应"（规格 W3.3）。
///
/// 三件事一起做：置断开标志（界面横幅）、把这**一类**错误记进账本（`error_recent` 会带
/// 三句话给壳）、落一行 `engine_restart_gave_up`（现场按这一行数"第几次才放弃"）。
fn note_auto_restart_gave_up(k: &mut Kernel) {
    let attempt = k.restart.progress().attempt;
    // 先置旗（横幅），再补一句人话（cause 换成完整消息）

    k.errors.observe(
        crate::errors::Class::EngineUnresponsive,
        now_ms(),
    );
    let lang = k.language;
    crate::diagnostics::log(
        "engine_restart_gave_up",
        &[
            ("restarts", attempt.to_string()),
            ("by", "auto".to_string()),
        ],
    );
    // 让壳读到的是**同一句人话**（与 RPC 失败那条走同一个 `text::tf`）。
    let msg = crate::i18n::text::tf(
        lang,
        crate::i18n::text::MsgKey::EngineDisconnected,
        &[(
            "cause",
            crate::errors::Class::EngineUnresponsive.explain(lang),
        )],
    );
    k.set_engine_disconnected(true, &msg);
}

/// 给一条错误填上客户可见的类，并落一行 `error_class=` 日志（规格 §2.3）。
///
/// **分型只在这一处发生**：客户看到的每一类错误，都能从线上 `error.class` 读到，
/// 也能从日志按 `error_class=` 聚合。内部错误（`class_of` 返回 `None`）保持 `class`
/// 为空——界面会显示"意料之外的错误 + 复制诊断信息"，**不猜**是哪一步坏的。
fn classify_error(k: &mut Kernel, e: &mut ErrorBody, op: &str, now_ms: u64) -> Option<u32> {
    let class = crate::errors::class_of(&e.code, None);
    let mut count = None;
    if let Some(c) = class {
        e.class = Some(c.wire().to_string());
        match k.errors.observe(c, now_ms) {
            crate::errors::Seen::New => {}
            crate::errors::Seen::Repeat { count: n } => count = Some(n),
        }
    }
    crate::diagnostics::log(
        "error_class",
        &[
            (
                "class",
                class.map(|c| c.wire()).unwrap_or("internal").to_string(),
            ),
            ("code", e.code.clone()),
            ("op", op.to_string()),
            ("count", count.unwrap_or(1).to_string()),
        ],
    );
    count
}

pub fn dispatch(kernel: &Arc<Mutex<Kernel>>, req: &Request) -> (Response, bool) {
    let id = req.id;
    if req.method == "shutdown" {
        return (Response::ok(id, json!({})), true);
    }
    let r: Result<Value, ErrorBody> = match req.method.as_str() {
        // 握手要多做一件事：把语言解析结果**记进内核**（后续所有客户可见文案都查它），
        // 并把实际语言回给壳。故它不能像原来那样一句 `protocol::hello` 了事。
        "hello" => op_hello(kernel, &req.params),
        // ⚠️ 下面五个方法**刻意不用 `with_kernel` 包起来**：它们最贵的部分都是网络 I/O
        // （`fetch` 最坏 91.5 秒、`plan` 最坏 30 秒 × N、换码清理最坏约 40 秒），
        // 持着内核锁跑会让校验线程与落地监听器一起停摆分钟级（契约 §5.2）。
        // 它们各自把"锁外取数 / 锁内安装"分开，见各自的注释。
        "load_delivery" => op_load_delivery(kernel, &req.params),
        "list_dir" => op_list_dir(kernel, &req.params),
        "get_tree" => op_get_tree(kernel, &req.params),
        "selection_summary" => op_selection_summary(kernel, &req.params),
        "plan" => op_plan(kernel, &req.params),
        "enqueue" => op_enqueue(kernel, &req.params),
        "transfer_list" => with_kernel(kernel, op_transfer_list),
        // `resume_all` 要在锁外算 `complete`（内含 HEAD），所以不能裹 `with_kernel`；
        // 其余动作照旧在锁内。分流点就这一处。
        "task_action" => match req.params.get("action").and_then(Value::as_str) {
            Some("resume_all") => op_resume_all(kernel),
            _ => with_kernel(kernel, |k| op_task_action(k, &req.params)),
        },
        "verify_status" => with_kernel(kernel, op_verify_status),
        "get_settings" => with_kernel(kernel, op_get_settings),
        "set_settings" => with_kernel(kernel, |k| op_set_settings(k, &req.params)),
        "get_state" => with_kernel(kernel, op_get_state),
        // `migrate_start` 不裹 `with_kernel`：它要把 `kernel` 交给后台线程（搬完后让
        // `complete` 过期）。真正的搬运在线程里，这条请求本身立刻返回。
        "migrate_start" => op_migrate_start(kernel, &req.params),
        "migrate_status" => with_kernel(kernel, op_migrate_status),
        "get_download_dir" => with_kernel(kernel, op_get_download_dir),
        "enqueue_status" => with_kernel(kernel, op_enqueue_status),
        "update_status" => with_kernel(kernel, op_update_status),
        "error_recent" => with_kernel(kernel, op_error_recent),
        "restart_progress" => with_kernel(kernel, op_restart_progress),
        "delivery_progress" => with_kernel(kernel, op_delivery_progress),
        "update_set_enabled" => with_kernel(kernel, |k| op_update_set_enabled(k, &req.params)),
        // ⚠️ 这一条**刻意不裹 `with_kernel`**（与上面那五个同源）：它最贵的部分是
        //    网络 I/O（最坏 5 秒），持锁跑会让校验线程与 `transfer_list` 一起停摆。
        //    末参数是 `kernel` 本身，不是闭包。见 `op_check_update_now` 的注释。
        "update_check_now" => op_check_update_now(kernel),
        other => Err(ErrorBody::new(
            kcodes::UNKNOWN_METHOD,
            format!("内核不认识方法 {other:?}"),
        )),
    };
    match r {
        Ok(v) => (Response::ok(id, v), false),
        Err(mut e) => {
            let now = now_ms();
            {
                let mut k = lock(kernel);
                classify_error(&mut k, &mut e, &req.method, now);
            }
            (Response::err_with(id, e), false)
        }
    }
}

// ---------------------------------------------------------------------------
// 各方法的实现
// ---------------------------------------------------------------------------

/// 换交付码时把**引擎侧**的上一批任务与 GID 映射清干净。
///
/// ⚠️ **为什么必须做**：`Daemon` 内部那张 `by_gid`（gid → **上一批**的相对路径）
/// 是它自己维护的，`load_delivery` 换码时并不会碰它。而 `view::compose` /
/// `view::progress` 过滤任务的判据只有**一句**——"这个路径在不在**新**清单里"
/// （`if !out.contains_key(path) { continue; }`）——**不是**"这个任务属不属于这一批"。
/// 于是只要新批次里存在与上一批**同相对路径**的文件，上一批那个任务的状态
/// （含 `errorMessage`、已完成字节）就会**冒充**成新批次这个文件的：
/// 上一批失败过 → 新批次的树直接显示 `failed` 并带着上一批的错误文案；
/// 上一批还在传 → 显示 `downloading` 且进度里混进上一批的字节。
///
/// 最容易撞上的场景正是**"同一批交付换个码重新生成链接"**（路径完全重合）——
/// 而那**正是"记住上次交付码"这个功能存在的场景**。所以这不是边角，是常规流。
///
/// 做法：逐个 `remove`（`Daemon::remove` 会按任务状态分派到 `aria2.remove` /
/// `removeDownloadResult`，**两条路径走完都会 `forget` 掉映射**），再 `clear_finished`
/// 收尾 purge。选它而不是"关掉引擎重建"，是因为它只用现成的 `Daemon` API、
/// 不必付一次重启（重新探端口 + 等就绪）的代价，而且**不碰 `download_dir` 里
/// 已经落盘的文件**——客户换批次不该丢掉上一批下好的数据。
///
/// 单个任务移除失败只记日志、不中断：一次 RPC 失败不该让整个"换批次"失败，
/// 而且后面的 `clear_finished` 还会再兜一次（它对已停止的条目同样会 `forget`）。
fn clear_engine_batch(d: &Daemon) {
    let gids: Vec<String> = match d.snapshot() {
        Ok(s) => s.tasks.iter().map(|t| t.gid.clone()).collect(),
        Err(e) => {
            eprintln!("benagen-core: 换码时读不到旧任务列表（跳过引擎侧清理）: {e}");
            return;
        }
    };
    // ⚠️ **连续失败即短路**。每个 `d.remove(gid)` 自己还要先发一次 `client.status()`
    // （`tellStatus` 单查判停走，0.2.7）——也就是**正常路径上**每个任务约 2 次 RPC；
    // `RPC_TIMEOUT` 是 10 秒。
    //
    // ⚠️ **两个口径别混**（它们量级相同、成因完全不同，摆在一起容易被读成 600 秒）：
    //   - **正常路径**（引擎健康）：每次移除 2 次 RPC，但都秒回，30 个任务是毫秒级；
    //   - **黑洞路径**（aria2 不响应也不拒连）：**`d.remove` 里的 `status()` 会
    //     等满 10 秒超时**，`Err` 按「没停」处理后仍要发那次 `aria2.remove`（它同样
    //     等满 10 秒）——**每次失败 2 次 RPC、约 20 秒**（0.2.7 把判据从 `list()`
    //     换成 `tellStatus` 单查之后，判据那一查失败**不再**短路掉后面的 `remove`：
    //     「GID 失效 400 按没停处理」与传输层失败在 `Result<String,_>` 上不可分，
    //     不做字符串匹配）。连续 3 次失败即短路，所以黑洞路径的上界是
    //     **10s 快照 + 3×20s ≈ 70 秒**，与任务数**无关**——30 个任务和 3000 个任务都是这个数。
    //
    // 把"单次 RPC"这个已接受的模式放大成**无界的循环**是量变到质变——
    // 正常情况毫秒级无感，坏情况下是用户会来投诉的形状。所以给它一个上界：
    // **连续 3 次失败**就认为引擎已经不可用，剩下的交给后面的 `clear_finished` 兜。
    // 阈值取 3 而不是 1：孤立的单次失败（某个 GID 恰好不在了）不该让整轮清理放弃，
    // 而连续三次说明是系统性的。
    //
    // ⚠️ **本函数现在跑在 `kernel` 锁外**（I-2）：它是由 `op_load_delivery` 在
    // 锁外 ③ 那一段调的，所以那约 70 秒不再冻结校验线程与落地监听器。
    // 短路保护**照样保留**——读循环仍然是同步的，引擎卡死时客户点"加载交付码"
    // 仍要等这 70 秒才拿到响应。
    // 残余：`clear_finished` 对"已经被 aria2 自己 purge 掉的 GID"无能为力——
    // 那是既有瑕疵，不在本轮。
    const MAX_CONSECUTIVE_FAILURES: usize = 3;
    let mut consecutive = 0usize;
    for gid in &gids {
        match d.remove(gid) {
            Ok(()) => consecutive = 0,
            Err(e) => {
                consecutive += 1;
                eprintln!("benagen-core: 换码时移除旧任务 {gid} 失败（第 {consecutive} 次）: {e}");
                if consecutive >= MAX_CONSECUTIVE_FAILURES {
                    eprintln!(
                        "benagen-core: 连续 {consecutive} 次移除失败，判定引擎不可用，\
                         提前结束换码清理（剩余条目交给 clear_finished）"
                    );
                    break;
                }
            }
        }
    }
    if let Err(e) = d.clear_finished() {
        eprintln!("benagen-core: 换码时清空已完成条目失败（继续）: {e}");
    }
}

/// `load_delivery`：拉清单、解析、绑定状态、返回批次摘要与树。
///
/// `params.base_url` 可选：壳可能指向镜像或测试服务端（默认 `delivery::DEFAULT_BASE_URL`）。
///
/// ⚠️ **本方法刻意不走 `with_kernel`**（契约 §5.2）。它有三段网络 I/O，一段都不能持锁：
///   - `delivery::fetch`：15 秒 × 5 次 + 15 秒退避 = 最坏 **90 秒**（2026-10-12 调参；记账仍按 91.5 秒留余量）；
///   - `clear_engine_batch`：10 秒快照 + 3×10 秒短路上限 = 最坏约 **40 秒**；
///   - 全量 `plan`：每个 crc64 为空的文件一次 HEAD，最坏 **30 秒 × N**。
///
/// 于是拆成"锁外取数 / 锁内安装"两个阶段，锁内只剩短写入。
/// 读循环是**同步**的，所以不存在两次 `load_delivery` 交错的语义问题。
/// 拉清单：**显式基址**照旧单域名；没给就两个 CDN 一起查（规格 W4）。
///
/// 缓存命中先直连那一侧，失败再回退竞速——缓存过期或换了网络都不会卡住客户。
fn load_with_race(
    kernel: &Arc<Mutex<Kernel>>,
    code: &str,
    lang: crate::i18n::Lang,
    explicit_base: Option<&str>,
) -> Result<delivery::Manifest, (String, crate::errors::Cause)> {
    // 显式基址：客户/壳指定了就不要自作聪明
    if let Some(base) = explicit_base {
        let t = UreqTransport::new();
        let r = delivery::fetch(&t, base, code, lang).map_err(|e| {
            let cause = crate::delivery_race::cause_of(&e);
            (e, cause)
        });
        if let Ok(m) = &r {
            let lines = vec![format!("{}（{}）", crate::delivery_race::Stage::Hit(
                crate::delivery_race::Origin::from_base(base).unwrap_or(crate::delivery_race::Origin::Domestic),
            ).text(lang), base)];
            remember_resolution(kernel, None, "hit_domestic", lines);
            let _ = m;
        }
        return r;
    }

    let cached = lock(kernel).race.lookup(code, now_ms());
    if let Some(origin) = cached {
        let t = UreqTransport::new();
        remember_resolution(
            kernel,
            Some(origin),
            "resolving",
            vec![crate::delivery_race::Stage::Start.text(lang).to_string()],
        );
        match delivery::fetch(&t, origin.base(), code, lang) {
            Ok(m) => {
                remember_resolution(
                    kernel,
                    Some(origin),
                    if origin == crate::delivery_race::Origin::Domestic {
                        "hit_domestic"
                    } else {
                        "hit_international"
                    },
                    vec![crate::delivery_race::Stage::CachedHit(origin).text(lang).to_string()],
                );
                return Ok(m);
            }
            // 缓存不许成为新的单点：失败就回退竞速
            Err(_) => {}
        }
    }

    remember_resolution(
        kernel,
        None,
        "resolving",
        vec![crate::delivery_race::Stage::Start.text(lang).to_string()],
    );
    let code_owned = code.to_string();
    let raced = crate::delivery_race::race(
        move |origin| {
            let t = UreqTransport::new();
            delivery::fetch(&t, origin.base(), &code_owned, lang)
        },
        lang,
        crate::delivery_race::RACE_TIMEOUT,
    )?;
    lock(kernel).race.remember(code, raced.origin, now_ms());
    let state = if raced.origin == crate::delivery_race::Origin::Domestic {
        "hit_domestic"
    } else {
        "hit_international"
    };
    remember_resolution(kernel, Some(raced.origin), state, raced.lines);
    let mut m = raced.manifest;
    // 清单里的基址就是**命中那一侧**：下载 URL 与交付页链接都跟着它（规格 W4.1）。
    m.base_url = raced.origin.base().to_string();
    Ok(m)
}

/// 把"解析到哪一步了"落进内核（界面读 `delivery_progress`）。
fn remember_resolution(
    kernel: &Arc<Mutex<Kernel>>,
    origin: Option<crate::delivery_race::Origin>,
    state: &'static str,
    lines: Vec<String>,
) {
    let mut k = lock(kernel);
    k.resolve_state = state;
    k.resolve_origin = origin;
    for line in lines {
        crate::delivery_race::push_line(&mut k.resolve_lines, &line);
    }
    if state != "resolving" {
        // 一次解析结束：把开头那句换新的（下一批不该还挂着上一批的过程）
        let keep = k.resolve_lines.len().min(8);
        k.resolve_lines.truncate(keep);
    }
}

pub fn op_load_delivery(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    // ── 锁外 ①：参数解析（纯计算，连锁都不用取）──────────────────────────
    let raw = params
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| ErrorBody::invalid_params("load_delivery 需要 code 参数（链接或裸随机码）"))?;
    // 客户可能直接粘贴邮件里的链接，也可能只输入那串码
    let code = delivery::extract_code(raw).map_err(ErrorBody::invalid_params)?;
    let base = params
        .get("base_url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(delivery::DEFAULT_BASE_URL);
    // 「要不要整棵树」——**默认要**（macOS 侧仍在用）；Windows 壳改成只要
    // 一层的 `list_dir` 之后，传 `false` 就不再花 33 MB 的序列化与传输。
    // 十万文件的批次里，那一棵树是最大的一笔开销（实测 9.2 万文件 33,027,043 B）。
    let want_tree = params.get("tree").and_then(Value::as_bool).unwrap_or(true);

    // 语言：只为"给客户的那句话"取一格，取完立刻放锁（下面全是 I/O）
    let lang = lock(kernel).language;

    // ── 锁外 ②：拉清单（最坏 91.5 秒）────────────────────────────────
    let explicit = params
        .get("base_url")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let m = load_with_race(kernel, &code, lang, if explicit { Some(base) } else { None })
        .map_err(|(e, cause)| {
            let class = crate::errors::class_of("", Some(cause));
            let mut body = ErrorBody::new(kcodes::DELIVERY_FETCH_FAILED, e);
            body.class = class.map(|c| c.wire().to_string());
            body
        })?;

    // ── 锁内 ①（短）：判"码变了没有"，并取旧引擎的句柄 ──────────────────
    let (code_changed, old_daemon) = {
        let mut k = lock(kernel);
        // ⚠️ **换码即作废，引擎侧也要清干净**（裁定 10 改判为"要修"）。
        // 只在**码真的变了**的时候清：同一个码重复 load（壳刷新树）不该把正在下的任务删掉。
        let changed = k.manifest.as_ref().is_some_and(|old| old.code != m.code);
        if changed {
            // 规格 W3.3：换了新交付码 = 换了一件事做，把自动重启的预算还回去。
            k.restart.reset();
            // 在清引擎**之前**就作废后台队列与在飞的一批：清引擎要几十秒，期间飞着的
            // addUri 若不被代数挡住，就会把旧批次的文件落进引擎。
            k.discard_feed_queue();
        }
        (changed, k.daemon.clone())
    };

    // ── 锁外 ③：换码清理（最坏约 40 秒，见 `clear_engine_batch`）────────
    // 它只碰 `Daemon`（自己有锁），不需要内核锁——而它是三个网络 I/O 段里
    // 唯一一个此前仍留在锁内的。
    if code_changed {
        if let Some(d) = old_daemon {
            clear_engine_batch(&d);
        }
    }

    // ── 锁内 ②（短）：安装清单、绑状态、作废上一批的记录 ─────────────────
    let (dir, state) = {
        let mut k = lock(kernel);
        // 换码即作废：上一批的记录不得用来跳过这一批的文件（否则是静默少交）。
        // `planner` 内部也会再 `bind` 一次（纵深防御），但那只作用于**锁外那份副本**，
        // 所以这里这一下（落盘 + 落到真身）是承重的。
        k.state.bind(&m.code);
        if let Err(e) = k.state.save() {
            eprintln!("benagen-core: 写状态文件失败（不影响本批）: {e}");
        }
        // 「想下的文件集」同样随码绑定：同一个码（重启内核后重放批次）原样保留，
        // 换码即清空。落盘失败不致命，只影响「自动补回队列」。
        if k.intent.bind(&m.code) {
            k.save_intent();
        }
        k.manifest = Some(m.clone());
        k.verify = verify::CheckResult::default();
        k.verifying.clear();
        k.verified.clear();
        k.retried.clear();
        // 认领下面的全量 plan：窗口里若校验线程提交了结果，它置上的 `true` 会被保留，
        // 下一次读会重算（见 `ensure_complete` 的三段式注释）。
        k.complete_dirty = false;
        k.remember_last_code(&m.code);
        k.plan_inputs(&m)
    };

    // ── 锁外 ④：全量 plan（最坏 30 秒 × N）──────────────────────────
    // ⚠️ **不限预算**（`usize::MAX`）：加载是客户**主动发起**的动作，语义就是"把这一批
    //    准备齐"。这里的 N 次 HEAD 是**一次性的**，而且到这一刻后台那条补齐线程
    //    通常已经补完了（它从上一批加载完就开始跑）—— 补完了这里就是 0 次。
    let todo = plan_off_lock(dir, state, &m, false, usize::MAX);

    // ── 锁内 ③（短）：装 `complete` 并出树 ────────────────────────────
    let mut k = lock(kernel);
    // 🔴 **这一趟补到的 crc64 必须写回清单**（2026-10-08）：不写回去，它只活在 `plan`
    // 内部那份克隆上 —— 而 `complete_dirty` 会在**每一批校验结果写回之后**被置起
    // ⇒ 紧接着的每一次 `get_tree`/`list_dir`/`enqueue` 都会把**同一批文件的 HEAD
    // 全部重发一遍**，代价 ∝ 文件数（N=10 就已 5 分钟以上）。壳的命令是**同步**的
    // （Tauri 在收 IPC 的那条线程上跑完才回，IPC 走主线程）⇒ 客户看到的是窗口「未响应」。
    adopt_filled_crc64(&mut k, &todo);
    k.complete = complete_from(&todo);
    // ⚠️ 出树仍用 `m`（plan **之前**那份克隆）是**对的**：`tree_json` 只消费
    //    path / size / source_mtime，**一个字节的 `crc64` 都不读**（`view::build_tree`）。
    //    两者在这一维上等价 —— 所以不必为了"看起来一致"多克隆一份清单。
    // 只有真要树的时候才序列化它（`tree_json` 要 `&mut k`，并遍历整份清单）。
    let tree = if want_tree { Some(tree_json(&mut k, &m)) } else { None };

    let mut out = json!({
        "code": m.code,
        // 交付页地址（`Manifest::url()` 在移植时没带过来，而 Go 版界面用它提示客户：
        // `manifest.go:113`）。壳拿它就能把"去哪儿看交付页"显示成人话。
        "page_url": format!(
            "{}/{}/{}",
            m.base_url.trim_end_matches('/'),
            m.code,
            delivery::PAGE_NAME
        ),
        "base_url": m.base_url,
        "created_at": m.created_at,
        "expires_at": m.expires_at,
        // Go 版界面在 loadDelivery 里就是这么用的（`main.go:672`）：
        // 过期要让客户看见，而不是等下载一个文件一个文件地 404。
        "expired": m.expired(std::time::SystemTime::now()),
        "total_files": m.total_files,
        "total_bytes": m.total_bytes,
    });
    if let Some(tree) = tree {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("tree".into(), tree);
        }
    }
    Ok(out)
}

/// `selection_summary`：「勾选面」的规模，**不把路径列回给壳**。
///
/// 为什么要有它：十万文件的批次里，壳为了在底栏写「已选 N 个 · M GB」，过去要把整棵树
/// （连 `flat` 一起）拉回本地再逐条累加 —— 实测 9.2 万文件 61 MB，而它只需要三个数。
///
/// 选项集的**身份**仍归壳（勾了哪些 path 它自己知道）；只有「默认面 / 全选」这一档，
/// 壳把 `paths` 留空，由内核按 [`view::default_selected`] 现算 —— 这样
/// 「默认勾哪些」全仓只有一个定义，不会出现两边各算一份、结果不一样。
///
/// ⚠️ `paths` 为空时要先 `ensure_complete`（锁外），和 `op_enqueue` 同一条理由。
pub fn op_selection_summary(
    kernel: &Arc<Mutex<Kernel>>,
    params: &Value,
) -> Result<Value, ErrorBody> {
    let raw: Vec<String> = params
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if raw.is_empty() {
        ensure_complete(kernel);
    }

    let mut k = lock(kernel);
    let m = k.require_manifest()?;
    let (path_map, tasks) = current_tasks(&mut k);
    let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
    let default = view::default_selected(&states);

    let sizes: std::collections::BTreeMap<&str, i64> =
        m.files.iter().map(|f| (f.path.as_str(), f.size)).collect();
    let selected: std::collections::BTreeSet<String> = if raw.is_empty() {
        default.clone()
    } else {
        // 清单里不存在的路径一律忽略（壳可能拿着上一份批次的选择）。
        raw.into_iter().filter(|p| sizes.contains_key(p.as_str())).collect()
    };

    let mut bytes: i64 = 0;
    let mut pending_count: usize = 0;
    let mut pending_bytes: i64 = 0;
    for path in &selected {
        let Some(size) = sizes.get(path.as_str()) else { continue };
        bytes += *size;
        let is_done = states
            .get(path.as_str())
            .is_some_and(|st| st.state == crate::view::State::Complete);
        if !is_done {
            pending_count += 1;
            pending_bytes += *size;
        }
    }

    Ok(json!({
        "count": selected.len(),
        "bytes": bytes,
        "pending_count": pending_count,
        "pending_bytes": pending_bytes,
        "all_selected": selected == default,
    }))
}

/// 把 `view` 的树转成 JSON。节点的四态取自 [`view::compose`]。
///
/// ⚠️ **调用方必须保证 `k.complete` 是最新的**：本函数不再自己 `ensure_complete`
/// ——那一步会做 N 次上限 30 秒的 HEAD，必须在**锁外**跑（见 [`ensure_complete`]）。
/// 它只有一个调用点（`op_load_delivery` 的锁内 ③），那里刚好算完 `complete`。
fn tree_json(k: &mut Kernel, m: &Manifest) -> Value {
    let (path_map, tasks) = current_tasks(k);
    let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
    node_json(&view::build_tree(&m.files), &states)
}

fn node_json(root: &BTreeMap<String, view::Node>, states: &BTreeMap<String, ViewFileState>) -> Value {
    fn conv(
        level: &BTreeMap<String, view::Node>,
        states: &BTreeMap<String, ViewFileState>,
    ) -> Value {
        let mut out = Map::new();
        for (name, n) in level {
            out.insert(name.clone(), one(n, states));
        }
        Value::Object(out)
    }
    fn one(n: &view::Node, states: &BTreeMap<String, ViewFileState>) -> Value {
        match n.node_type {
            view::NodeType::Dir => json!({
                "type": "dir",
                "name": n.name,
                "children": conv(&n.children, states),
            }),
            view::NodeType::File => {
                let st = states.get(&n.path);
                json!({
                    "type": "file",
                    "name": n.name,
                    // 约束 3：manifest 原文，原样带出去
                    "path": n.path,
                    "size": n.size,
                    "crc64": n.crc64,
                    // 源文件时间：**始终发这个键**，无值时是空串（老清单里没有它）。
                    // 与 `crc64` 同一种做法——它是"可能为空"的字符串、不是可选键，
                    // 壳不必区分"键不存在"与"没有时间"，读到的永远是字符串。
                    // ⚠️ 这里读的是 `view::Node`（不是 `delivery::File`）：两处都得带，
                    // 只改 `delivery` 的话字段会**静默地**到不了壳，且不报任何错。
                    // 原样搬运，不解析不换算（约束 C-7）。
                    "source_mtime": n.source_mtime,
                    "state": state_name(st.map(|s| s.state)),
                    "completed": st.map(|s| s.completed).unwrap_or(0),
                    "total": st.map(|s| s.total).unwrap_or(n.size),
                    "speed": st.map(|s| s.speed).unwrap_or(0),
                    "err": st.map(|s| s.err.clone()).unwrap_or_default(),
                })
            }
        }
    }
    Value::Object(match root.is_empty() {
        true => Map::new(),
        false => {
            let mut m = Map::new();
            m.insert("type".into(), json!("dir"));
            m.insert("name".into(), json!(""));
            m.insert("children".into(), conv(root, states));
            m
        }
    })
}

fn state_name(s: Option<view::State>) -> &'static str {
    match s {
        Some(view::State::Pending) | None => "pending",
        Some(view::State::Downloading) => "downloading",
        Some(view::State::Complete) => "complete",
        Some(view::State::Failed) => "failed",
    }
}

/// 引擎的当前任务与 GID→路径映射。**引擎没起来或断开时返回空**——
/// 四态合成在"没有任务"时退化成"按状态文件说话"，那正是正确的结果。
///
/// ⚠️ **它是在锁内被调的**：四个调用点（`load_delivery`/`list_dir`/`get_tree`/`enqueue`）
/// 都是**持有内核 `Mutex` 的时候**执行。
/// （这里曾经写着"快照不持锁"——那句话是错的，会误导后来者。）
///
/// **这是 I-2 修复之后仍然留在锁内的最后一段网络 I/O**，所以把上界写准
/// （规格 §3.4 的两段口径，**别混读**）：
/// `snapshot()` = 1 条 `getGlobalStat` + 3 条 `tell*`（0.2.7 起 `getGlobalStat` 先行——
/// 尾窗偏移要它的 `numStopped`）。
///   · **真死引擎**（失败路径）：`list()`/`global_slow()` **第一次出错就返回**（契约 §4.3）
///     ⇒ 失败上界 = **单次慢档 30 秒**（`SNAPSHOT_TIMEOUT`，单次尝试、不重试；
///     connection refused 时约 0 秒）——不是 4×30 秒；
///   · **慢应答但最终成功**（成功路径）：四个方法都要等到**自己的**应答
///     ⇒ 上界 ≈ **4×`SNAPSHOT_TIMEOUT`**。**这段 I/O 在内核锁内，按它计**。
/// `load_delivery` 里那三段真正的长 I/O（`fetch` 91.5 秒、`plan` N×30 秒、
/// 换码清理约 70 秒）已经全部移到锁外，见各自的注释。
///
/// 之所以慢应答那一段现在不是 bug：读循环是**同步的**，一次只处理一条请求，
/// 所以不存在"另一个协议请求被这把锁挡住"这回事。代价是**校验线程与落地监听器**
/// 要排在这次快照后面。要让读路径也彻底不持锁，得把这四个 handler 的
/// "取快照"那一步也拆出来（它们还要拿 `engine_disconnected` 与重连逻辑，
/// 那是 [`Kernel::read_engine`] 的副作用）；**这条注释只是把事实说准，
/// 不是"这里没问题"的背书**。
fn current_tasks(k: &mut Kernel) -> (BTreeMap<String, String>, Vec<Task>) {
    let Ok(d) = k.read_engine() else {
        return (BTreeMap::new(), Vec::new());
    };
    match d.snapshot() {
        Ok(s) => (s.path_map, s.tasks),
        Err(_) => (BTreeMap::new(), Vec::new()),
    }
}

/// `list_dir`：某目录的**直接**子项。
///
/// ⚠️ 同 `load_delivery`：`ensure_complete` 必须在锁外调（它内含 N 次 30 秒上限的 HEAD）。
pub fn op_list_dir(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_matches('/')
        .to_string();

    ensure_complete(kernel); // 锁外（长）；幂等，只有 `complete_dirty` 时才真算

    let mut k = lock(kernel);
    let m = k.require_manifest()?;
    let (path_map, tasks) = current_tasks(&mut k);
    let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
    let root = view::build_tree(&m.files);

    let level = if path.is_empty() || path == "." {
        Some(&root)
    } else {
        let mut cur = &root;
        let mut found = true;
        for seg in path.split('/') {
            match cur.get(seg) {
                Some(n) if n.node_type == view::NodeType::Dir => cur = &n.children,
                _ => {
                    found = false;
                    break;
                }
            }
        }
        if found {
            // 借用问题：把结果就地转成 JSON，避免把 `cur` 带出作用域
            return Ok(json!({
                "path": path,
                "entries": entries_json(cur, &states),
            }));
        }
        None
    };
    match level {
        Some(l) => Ok(json!({"path": path, "entries": entries_json(l, &states)})),
        None => Err(ErrorBody::new(
            kcodes::PATH_NOT_FOUND,
            format!("清单里没有目录 {path:?}"),
        )),
    }
}

fn entries_json(
    level: &BTreeMap<String, view::Node>,
    states: &BTreeMap<String, ViewFileState>,
) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for n in level.values() {
        out.push(match n.node_type {
            view::NodeType::Dir => json!({
                "type": "dir",
                "name": n.name,
                "children_count": n.children.len(),
            }),
            view::NodeType::File => {
                let st = states.get(&n.path);
                json!({
                    "type": "file",
                    "name": n.name,
                    "path": n.path,
                    "size": n.size,
                    "crc64": n.crc64,
                    // 同 `node_json`：始终发这个键，无值时是空串。
                    // **这是另一条 JSON 出口**——两条路各读一次 `view::Node`，
                    // 漏掉任何一条，字段都到不了壳（而且不报错）。
                    "source_mtime": n.source_mtime,
                    "state": state_name(st.map(|s| s.state)),
                    "completed": st.map(|s| s.completed).unwrap_or(0),
                    "total": st.map(|s| s.total).unwrap_or(n.size),
                    "speed": st.map(|s| s.speed).unwrap_or(0),
                    "err": st.map(|s| s.err.clone()).unwrap_or_default(),
                })
            }
        });
    }
    Value::Array(out)
}

/// `get_tree`：整树 + 总进度 + 默认勾选面。
///
/// `flat` 用的是 [`view::flatten`]（树序，与界面上看到的顺序一致），
/// 让壳不必跨进程边界递归就能渲染大目录；`default_selected` 是给勾选框的默认值
/// （Go 版界面就是这两处的调用方）。
pub fn op_get_tree(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    // `summary_only`：只要**总进度与默认面的规模**，不要树、不要 flat、不要那串路径。
    // 十万文件的批次里，后三者加起来 61 MB（实测 9.2 万文件），而底栏只用这几个数。
    let summary_only = params
        .get("summary_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    ensure_complete(kernel); // 锁外（长）；幂等，只有 `complete_dirty` 时才真算

    let mut k = lock(kernel);
    let m = k.require_manifest()?;
    let (path_map, tasks) = current_tasks(&mut k);
    let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
    let prog = view::progress(&m.files, &k.complete, &path_map, &tasks);

    if summary_only {
        // 默认面的规模（口径与 `view::default_selected` 同一个定义）。
        let sizes: std::collections::BTreeMap<&str, i64> =
            m.files.iter().map(|f| (f.path.as_str(), f.size)).collect();
        let selected = view::default_selected(&states);
        let mut bytes: i64 = 0;
        for path in &selected {
            bytes += sizes.get(path.as_str()).copied().unwrap_or(0);
        }
        return Ok(json!({
            "progress": {
                "total_bytes": prog.total_bytes,
                "done_bytes": prog.done_bytes,
                "speed": prog.speed,
                "percent": prog.percent,
            },
            "default_count": selected.len(),
            "default_bytes": bytes,
            "total_files": m.files.len() as i64,
        }));
    }

    let root = view::build_tree(&m.files);

    let flat: Vec<Value> = view::flatten(&root)
        .into_iter()
        .filter(|n| n.node_type == view::NodeType::File)
        .map(|n| {
            let st = states.get(&n.path);
            json!({
                "path": n.path,
                "name": n.name,
                "size": n.size,
                "state": state_name(st.map(|s| s.state)),
            })
        })
        .collect();
    let selected: Vec<String> = view::default_selected(&states).into_iter().collect();

    Ok(json!({
        "tree": node_json(&root, &states),
        "flat": flat,
        "default_selected": selected,
        "progress": {
            "total_bytes": prog.total_bytes,
            "done_bytes": prog.done_bytes,
            "speed": prog.speed,
            "percent": prog.percent,
        },
    }))
}

/// `plan`：本地扫描。
///
/// ⚠️ 同 `load_delivery`：扫描本身（含逐个 HEAD）必须在**锁外**跑。
pub fn op_plan(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    let strict = params
        .get("strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // ── 锁内 ①（短）：取清单 + plan 的输入副本 ─────────────────────────
    let (dir, state, m) = {
        let mut k = lock(kernel);
        let m = k.require_manifest()?;
        if !strict {
            // 同 `ensure_complete` 的第 ① 段：认领这次计算，窗口里别人置上的
            // `true` 会被保留（见那里的三段式注释）。
            k.complete_dirty = false;
        }
        let (dir, state) = k.plan_inputs(&m);
        (dir, state, m)
    };

    // ── 锁外（长）：全量扫描 ────────────────────────────────────────
    // ⚠️ **不限预算**（同 `op_load_delivery`）：点「下载」是客户主动发起的，
    //    而 `unverifiable` 那张表是**给客户看的**（"这 N 个文件本轮只能比大小"）——
    //    带预算会让它多出一些"其实还能补上"的条目，那是一句不该说的话。
    let todo = plan_off_lock(dir, state, &m, strict, usize::MAX);

    // ── 锁内 ②（短）：非严格模式顺手刷新 `complete` ────────────────────
    let mut k = lock(kernel);
    if !strict {
        // 同一次扫描顺带把 `complete` 刷新掉——**走同一个推导式**（见 `complete_from`），
        // 不在这里再写一遍过滤条件。
        k.complete = complete_from(&todo);
    }
    Ok(json!({
        "strict": strict,
        "items": todo.items.iter().map(|i| json!({
            "path": i.file.path,
            "size": i.file.size,
            "crc64": i.file.crc64,
            "kind": match i.kind { Kind::Skip => "skip", Kind::Download => "download" },
        })).collect::<Vec<_>>(),
        "unverifiable": todo.unverifiable,
        "complete": k.complete.iter().cloned().collect::<Vec<_>>(),
    }))
}

/// 把请求里的路径展开成清单文件（单文件 / 多选 / **整个目录**）。
fn resolve_targets(m: &Manifest, paths: &[String]) -> Result<Vec<delivery::File>, ErrorBody> {
    let mut out: Vec<delivery::File> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for raw in paths {
        let p = raw.trim().trim_matches('/');
        let all = p.is_empty() || p == ".";
        let prefix = format!("{p}/");
        let mut hit = false;
        for f in &m.files {
            let matched = all || f.path == p || f.path.starts_with(&prefix);
            if matched {
                hit = true;
                if seen.insert(f.path.clone()) {
                    out.push(f.clone());
                }
            }
        }
        if !hit {
            return Err(ErrorBody::invalid_params(format!(
                "清单里没有 {raw:?}（既不是文件，也不是任何文件的目录前缀）"
            )));
        }
    }
    if out.is_empty() {
        return Err(ErrorBody::invalid_params("没有匹配到任何文件"));
    }
    Ok(out)
}

/// **开工前检查**（设计规格 §9）。
///
/// 这条在 Go 版是界面层做的；现在引擎归内核，检查也归内核——否则壳要为了这件事
/// 重复实现一遍目录与磁盘逻辑，两边还会漂移。失败即返回结构化错误、**不启动引擎**。
fn preflight(lang: crate::i18n::Lang, dir: &Path, need_bytes: i64) -> Result<(), ErrorBody> {
    use crate::i18n::text::{tf, MsgKey};
    let bad = |m: String| ErrorBody::new(kcodes::PREFLIGHT_FAILED, m);
    let yes = |key: MsgKey, args: &[(&str, &str)]| bad(tf(lang, key, args));
    std::fs::create_dir_all(dir).map_err(|e| {
        yes(
            MsgKey::PreflightDirUnusable,
            &[("dir", &dir.display().to_string()), ("err", &e.to_string())],
        )
    })?;
    let probe = dir.join(".benagen-write-probe");
    std::fs::write(&probe, b"").map_err(|e| {
        yes(
            MsgKey::PreflightNotWritable,
            &[("dir", &dir.display().to_string()), ("err", &e.to_string())],
        )
    })?;
    let _ = std::fs::remove_file(&probe);

    let free = free_bytes(dir).map_err(bad)?;
    let need = need_bytes.max(0) as u64;
    if free < need {
        return Err(yes(
            MsgKey::PreflightNoSpace,
            &[
                ("need", &need.to_string()),
                ("dir", &dir.display().to_string()),
                ("free", &free.to_string()),
            ],
        ));
    }
    Ok(())
}

/// 目标所在文件系统的**可用**字节数。
///
/// ⚠️ **两个平台各一条实现**：std 不提供 `statvfs` 那一类入口，而本 crate 不许新增依赖
///    （`Cargo.toml` 里没有 `libc`/`fs4`）⇒ "问文件系统还剩多少"没有可移植的写法。
///
/// 🔴 **2026-10-08 修的一个**Windows 上必现**的缺陷**：原先**只有** Unix 那一支
///    （起 `df`），而 **Windows 上没有 `df`** ⇒ 每一次 `enqueue` 的开工前检查都在这里
///    失败，客户看到的是「无法读取磁盘剩余空间（df 不可用）：**program not found**」，
///    **一点下载就报**。
///
///    ⚠️ 这条代码从 2026-09-16 就在（早于两条线的共同祖先），**不是**哪一次合并带进来的；
///    而两侧的自动化判据都碰不到它：`cargo test` 跑在 macOS/Linux 上，那里 `df` 恒在；
///    Windows 侧只做过交叉编译、从没真跑过。⇒ 它**只**能在真机上暴露。
///
/// Windows 那一支走的是与 `bcrypt` 同一套手法（`#[link]` + 裸 `extern "system"`），
/// **不引入 crate 依赖**；先例见 `engine::daemon::random_secret` 的 `BCryptGenRandom`。
#[cfg(unix)]
fn free_bytes(dir: &Path) -> Result<u64, String> {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(dir)
        .output()
        .map_err(|e| format!("无法读取磁盘剩余空间（df 不可用）：{e}"))?;
    if !out.status.success() {
        return Err("df 读取磁盘剩余空间失败".to_string());
    }
    available_bytes_from_df(&String::from_utf8_lossy(&out.stdout))
}

/// `df -k` 的输出 → 可用**字节**数。
///
/// 单独拆成一个纯函数，是因为**这一段是 Unix 那条路上唯一会算错的地方**
/// （`df` 要真的存在、真的跑得起来，只有真机上才有那个前提；而取哪一列、乘多少
/// 是本机就能钉住的）。
#[cfg(unix)]
fn available_bytes_from_df(text: &str) -> Result<u64, String> {
    let line = text
        .lines()
        .nth(1)
        .ok_or_else(|| "df 输出格式不符预期（没有数据行）".to_string())?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    // macOS/Linux 的 `df -k`：Filesystem 1024-blocks Used Available Capacity …
    if cols.len() < 4 {
        return Err(format!("df 输出格式不符预期：{line:?}"));
    }
    cols[3]
        .parse::<u64>()
        .map(|k| k.saturating_mul(1024))
        .map_err(|e| format!("解析 df 的可用空间失败（{line:?}）：{e}"))
}

/// Windows 那一支：`GetDiskFreeSpaceExW`（`kernel32`，系统导入库，**不新增依赖**）。
///
/// ⚠️ **本支在开发机（macOS）上证不了"取得到数"** —— 本机能证的只有"**编得过**"
///    （`cargo build --target x86_64-pc-windows-gnu`）。它的真行为只在 Windows 上验，
///    已列进 `windows/README.md` 的真机验收清单。**这是如实记账，不是"验过了"。**
///
/// ⚠️ **路径长度**：`GetDiskFreeSpaceExW` 走的是 `MAX_PATH` 那一套，除非给 `\\?\` 前缀。
///    这里**不加**前缀——下载目录那一条 `Path` 来自 argv/偏好，正常都在 `MAX_PATH` 内；
///    真遇到超长路径时下面那条 `last_os_error()` 会把原因原样说出来（不静默）。
#[cfg(windows)]
fn free_bytes(dir: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;

    // `#[link]` + 裸 `extern "system"`：与 `random_secret` 取 `BCryptGenRandom` 同一条手法。
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            dir_name: *const u16,
            free_to_caller: *mut u64,
            total_bytes: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    // 宽字符串、以 NUL 结尾。
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);

    let (mut to_caller, mut total, mut free_total) = (0u64, 0u64, 0u64);
    // SAFETY：`wide` 以 NUL 结尾、在本调用期间一直存活；三个出参都是本函数栈上的 `u64`，
    //         指针有效且可写。这是 `GetDiskFreeSpaceExW` 的契约。
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut to_caller,
            &mut total,
            &mut free_total,
        )
    };
    // `BOOL`：非 0 即成功。失败时**把系统给的原因原样带出来**（W-2：失败必须看得见）。
    if ok == 0 {
        return Err(format!(
            "无法读取磁盘剩余空间（GetDiskFreeSpaceExW 失败：{}）",
            std::io::Error::last_os_error()
        ));
    }
    // 用 `free_to_caller`（**调用方**可用的那一个）而不是 `total_free`：磁盘配额下
    // 两者会不同，而我们关心的正是"这个进程还能写多少"。
    Ok(to_caller)
}

/// `enqueue`：加任务（单文件 / 多选 / 整个目录）。
///
/// `params.paths` 省略或为空时下**全部待下载**的文件——"谁还需要下"由
/// [`view::pending_paths`] **一处判定**（Go 版界面里有 6 处问同一件事，
/// 因此那里刻意收敛成一个纯函数）。
///
/// ⚠️ 两条**不对称**的错误语义（约束 D-2），别把它们"统一"掉：
///   - `paths` 为空且算出的 `pending` 也为空 ⇒ **成功**回执（`added: []`）。
///     界面「全部下载」在整批都完整时走的就是这条——它不是错误。
///   - `paths` **非空**（客户点名了具体文件/目录）⇒ `resolve_targets` 的报错**全部原样保留**，
///     点名一个不存在的路径仍然回 `invalid_params`（"不存在的路径"必须有声音，D-7）。
/// 「这批文件名在 Windows 上建不出来」那一份错误体（纯函数，便于在非 Windows 上测）。
///
/// 为什么单独抽出来：拒绝分支本身挂在 `cfg!(windows)` 后面，在 macOS/Linux 上跑不到；
/// 而**措辞**（数了几个、点出哪个字符、给出哪一条示例、带哪个 class）是客户唯一看到的东西，
/// 必须有判别力用例钉住。
fn invalid_filename_error(lang: crate::i18n::Lang, scan: &crate::names::Scan) -> ErrorBody {
    let (_seg, reason) = scan.first.clone().expect("count>0 必有 first");
    let count = scan.count.to_string();
    let reason = reason.describe();
    let example = scan.examples.first().cloned().unwrap_or_default();
    let msg = crate::i18n::text::tf(
        lang,
        crate::i18n::text::MsgKey::InvalidFilename,
        &[
            ("count", count.as_str()),
            ("reason", reason.as_str()),
            ("example", example.as_str()),
        ],
    );
    let mut body = ErrorBody::new(kcodes::INVALID_FILENAME, msg);
    body.class = Some(crate::errors::Class::NameInvalid.wire().to_string());
    body
}

pub fn op_enqueue(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    // 纯参数解析（连锁都不用取）
    let raw: Vec<String> = params
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // 只有"下全部待下载"才需要最新的 `complete`；要了就在**锁外**算（同 `list_dir`）。
    if raw.is_empty() {
        ensure_complete(kernel);
    }

    // ── 锁内（短）：定目标、查磁盘、拿引擎。这里**不做** RPC（addUri 在锁外逐个做）──────
    let (m, daemon, epoch, targets) = {
        let mut k = lock(kernel);
        let m = k.require_manifest()?;
        let mig = k.migrate.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if mig.running {
            return Err(ErrorBody::invalid_params(format!(
                "正在搬运已下载的文件（{}/{}），搬完后再试",
                mig.files_done, mig.files_total
            )));
        }
        let targets = if raw.is_empty() {
            let (path_map, tasks) = current_tasks(&mut k);
            let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
            // 已经在后台队列里等提交的，不算「待下载」：否则重复点「下载全部」会把同一个路径再 addUri 一遍。
            let queued: BTreeSet<&str> = k.feed_queue.iter().map(|f| f.path.as_str()).collect();
            // 引擎里已有**非完成**任务的路径（活跃 / 等待 / 暂停 / 失败）不由「下载全部」接手：
            // 重复提交会为同一路径造出第二个 GID。暂停的交给「一键开始」，失败的交给自动重试或用户显式重试。
            // 已完成的仍保留：本地文件被删后要能重下。
            let owned: BTreeSet<&str> = tasks
                .iter()
                .filter(|t| t.state != engine::status::TaskState::Complete)
                .filter_map(|t| path_map.get(&t.gid).map(String::as_str))
                .collect();
            let pending: Vec<String> = view::pending_paths(&m.files, &states)
                .into_iter()
                .filter(|p| !queued.contains(p.as_str()) && !owned.contains(p.as_str()))
                .collect();
            // ⚠️ **"没有待下载的"不是错误**（约束 D-2 / 阶段 D 任务 A）。
            //
            // 壳在「勾选面 == 全部文件」时**故意**发 `{"paths": []}`（全局约束 C-3：不许把
            // 整批路径塞进请求，会撑爆 8 MiB 行上限），所以"整批都已经下好了、客户又点了一次
            // 下载"这条**常规**流必然走到这里、`pending` 必然为空。修复前它落到
            // `resolve_targets` 的 `out.is_empty()` 分支，回的是
            // `invalid_params: 没有匹配到任何文件`——一个**成功状态被报成了错误**。
            //
            // 回执形状逐字沿用下面那条成功回执（`added` / `rejected`），既有字段名与形状
            // 一个不动（D-3）。这里**必须早退**，不能"让空 targets 走完下面的流程"：
            // 那样会为一件事都不做启动 aria2c（`ensure_engine`）。
            //
            // 另一半**一个字都不许改**：`raw` 非空（客户点名了具体路径）时走下面那条
            // `resolve_targets(&m, &raw)`，点名一个不存在的路径仍然必须是 `invalid_params`
            // ——错误语义是不对称的，由 e2e 的
            // `deleted_local_file_is_redownloaded_by_download_all` 一起钉住。
            if pending.is_empty() {
                return Ok(json!({"added": [], "rejected": []}));
            }
            resolve_targets(&m, &pending)?
        } else {
            resolve_targets(&m, &raw)?
        };

        // ① 开工前检查：**先于**引擎启动。
        // 🔴 **分段计时**（详细档，2026-10-08）。一次 `enqueue` 在真机上花过 **47 / 70 / 79 秒**
        //    （壳日志），而日志里原先只看得到"总耗时"——**钱花在哪一段读不出来**。
        //    ⚠️ 只在 `verbose` 档写（`log_verbose`）：普通档多这几行是噪声。
        let need: i64 = targets.iter().map(|f| f.size).sum();
        let t0 = std::time::Instant::now();
        preflight(k.language, &k.download_dir, need)?;
        crate::diagnostics::log_verbose(
            "enqueue_stage",
            &[
                ("stage", "preflight".to_string()),
                ("n", targets.len().to_string()),
                ("ms", t0.elapsed().as_millis().to_string()),
            ],
        );

        // ①½ 名字能不能在这台机器上建出来（Windows）：
        // `:`、`*`、`?`、`"`、`<`、`>`、`|`、`\\` 与保留设备名会让 aria2 一律建不出文件，
        // 而它的失败在传输列表里只是"这一条没动"，客户看到的是"已完成 / 校验通过 / 少 40 个文件"。
        // **开工前拒绝**，比下完 100 GB 再说便宜得多；句子在 i18n 表里，示例逐字带出来。
        if cfg!(windows) {
            let scan = crate::names::scan(targets.iter().map(|f| f.path.as_str()));
            if !scan.is_clean() {
                return Err(invalid_filename_error(k.language, &scan));
            }
        }

        // ② 起引擎（仍在锁内：起引擎的网络 I/O 是既有的另一处，见 `ensure_engine`）。
        let t1 = std::time::Instant::now();
        let daemon = k.ensure_engine()?;
        crate::diagnostics::log_verbose(
            "enqueue_stage",
            &[
                ("stage", "ensure_engine".to_string()),
                ("ms", t1.elapsed().as_millis().to_string()),
            ],
        );
        (m, daemon, k.batch_epoch, targets)
    };

    // ③ 加任务：大批量只同步提交前 [`FIRST_BATCH`] 个，让客户立刻看到任务开始；其余交给后台线程。
    // 一口气提交成千上万个 `addUri` 既会让回执等上几分钟，引擎的响应也会随任务数逐级退化
    // （现场日志：1ms → 8.6s → 超时）。
    let t2 = std::time::Instant::now();
    let (now_targets, later) = split_for_feeding(&targets);
    let (added, mut rejected, stopped) = submit_off_lock(kernel, &daemon, &m, epoch, now_targets);
    let stopped_at = stopped.as_ref().map(|(at, _)| *at);
    let (pending, extra) = {
        let mut k = lock(kernel);
        hand_over(&mut k, epoch, now_targets, later, stopped)
    };
    rejected.extend(extra);
    ensure_feeder(kernel);
    crate::diagnostics::log_verbose(
        "enqueue_stage",
        &[
            ("stage", "add_uri".to_string()),
            ("n", targets.len().to_string()),
            ("ok", added.len().to_string()),
            // 停在哪儿（`n` = 没停）。第 63 个 `addUri` 撞上超时那次，
            // 这个读数就是"已经入队了多少"的直接证据。
            (
                "stopped_at",
                match stopped_at {
                    Some(at) => at.to_string(),
                    None => targets.len().to_string(),
                },
            ),
            ("ms", t2.elapsed().as_millis().to_string()),
        ],
    );
    // 没有任何文件被加入、也没有待提交的 ⇒ 错误。待提交的（后台还会加）不算失败。
    if added.is_empty() && pending == 0 && !rejected.is_empty() {
        return Err(ErrorBody::invalid_params(format!(
            "没有任何文件被加入下载：{rejected:?}"
        )));
    }
    Ok(json!({"added": added, "rejected": rejected, "pending": pending}))
}

/// 队列里有活而后台线程没在跑 ⇒ 起一个（调用方持有 `kernel`，不持锁）。
fn ensure_feeder(kernel: &Arc<Mutex<Kernel>>) {
    let mut k = lock(kernel);
    if !k.feeder_running && !k.feed_queue.is_empty() {
        k.feeder_running = true;
        spawn_feeder(Arc::clone(kernel));
    }
}

/// 同步提交的第一批大小。
const FIRST_BATCH: usize = 100;
/// 后台每批提交的大小（持锁时间要短：每批之间放锁，让协议循环和校验线程插得进来）。
const FEED_BATCH: usize = 20;

/// 把目标切成「现在同步提交」与「后台陆续提交」两段。
fn split_for_feeding(targets: &[delivery::File]) -> (&[delivery::File], &[delivery::File]) {
    targets.split_at(targets.len().min(FIRST_BATCH))
}

/// 后台陆续加入线程：每拍取 [`FEED_BATCH`] 个提交，批间放锁并小睡；队列空了就退出。
///
/// 出队发生在**提交完成之后**（不是一取出就出队）：「下载全部」要据此排除正在飞的那一批。
/// 队列只在换码时整体作废（见 [`Kernel::discard_feed_queue`]），等待期间绝不丢条目。
pub fn spawn_feeder(kernel: Arc<Mutex<Kernel>>) {
    thread::Builder::new()
        .name("enqueue-feeder".to_string())
        .spawn(move || loop {
            if !feed_once(&kernel) {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        })
        .expect("起后台入队线程失败");
}

/// 后台加入的一拍。返回 `false` ⇔ 队列已空、线程该退出（`feeder_running` 与它在同一把锁内复位）。
///
/// 返回 `true` 的等待情形：暂停全部按住、搬运中、引擎暂缺（自愈换引擎的窗口）。
fn feed_once(kernel: &Arc<Mutex<Kernel>>) -> bool {
    let (d, m, epoch, batch) = {
        let mut k = lock(kernel);
        if k.feed_queue.is_empty() {
            k.feeder_running = false;
            return false;
        }
        if k.hold_auto_retry || migrating(&k) {
            return true;
        }
        let (Some(d), Some(m)) = (k.daemon.clone(), k.manifest.clone()) else {
            return true;
        };
        let batch: Vec<delivery::File> = k.feed_queue.iter().take(FEED_BATCH).cloned().collect();
        (d, m, k.batch_epoch, batch)
    };

    let (added, rejected, stopped) = submit_off_lock(kernel, &d, &m, epoch, &batch);

    let mut k = lock(kernel);
    if k.batch_epoch != epoch {
        // 这一批在飞期间换了码：队列已经被整体登记为拒绝，这里不再记任何东西。
        return true;
    }
    k.feed_added += added.len();
    k.feed_rejected.extend(rejected);
    match stopped {
        None => {
            k.feed_queue.drain(..batch.len());
        }
        Some((at, reason)) => {
            // 引擎中途断开：本批余下的与队列里更后面的，一律登记为拒绝（不静默丢）。
            let all: Vec<delivery::File> = k.feed_queue.drain(..).collect();
            for f in &all[at..] {
                k.feed_rejected.push(json!({"path": f.path, "reason": reason}));
            }
        }
    }
    true
}

/// `enqueue_status`：后台陆续加入的进度。读取即取走已完成的增量（`added` / `rejected`），
/// `pending` 是还没提交的个数。
pub fn op_enqueue_status(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let added = std::mem::take(&mut k.feed_added);
    let rejected = std::mem::take(&mut k.feed_rejected);
    Ok(json!({"pending": k.feed_queue.len(), "added": added, "rejected": rejected}))
}

/// 换码时未提交的理由（[`Kernel::discard_feed_queue`] 与 [`submit_off_lock`] 共用这一个）。
const REPLACED_REASON: &str = "未提交：批次已换";

// 测试缝：可换的 lookup（仅 `#[cfg(test)]`）。生产恒 `delivery::resolve_for_base` 的
// `system_lookup`。先例：`diag_counters::Snapshot::log_with` 的出口注入——生产路径一个字不变。
// （用 `//` 而非 `///`：`thread_local!` 是宏，rustdoc 不给宏展开产物生成文档。）
#[cfg(test)]
thread_local! {
    static TEST_LOOKUP: std::cell::RefCell<Option<fn(&str, u16) -> Vec<std::net::IpAddr>>> =
        std::cell::RefCell::new(None);
}

/// 解析 base 的备胎 IP（规格 §5.3）。**每个提交操作解析一次、整批复用**。
fn resolve_ips(base_url: &str) -> Vec<std::net::IpAddr> {
    #[cfg(test)]
    {
        let lookup = TEST_LOOKUP.with(|t| *t.borrow());
        if let Some(lookup) = lookup {
            return delivery::resolve_for_base_with(lookup, base_url);
        }
    }
    delivery::resolve_for_base(base_url)
}

/// 建 PlannedFile 并在真有备胎时补 Host 头（规格 §5.3 规则 6——同 [`delivery::host_header_for`]）。
/// Host 头只在真有备胎时下发（域名-only 时 URI 自带正确 Host，多发反而错怪镜像）。
///
/// 真有备胎时在**构造点**记一笔多 IP 诊断（`record_multiip`）：只记构造证据——
/// 「每 URI 成功/失败/被切换次数」aria2 的 `tellStatus` 取不到（§6.4 实测），
/// 诚实边界写在 `diag_counters::record_multiip` 的注释里。
fn plan_file(
    m: &Manifest,
    f: &delivery::File,
    ips: &[std::net::IpAddr],
    counters: &crate::diag_counters::Counters,
) -> Option<engine::PlannedFile> {
    let mut pf = engine::new_planned_file(m, f, ips)?;
    if pf.uris.len() > 1 {
        // `uris[0]` 是域名 URI，其后才是 IP 备胎 ⇒ 备胎数 = len - 1。
        counters.record_multiip(pf.uris.len() - 1);
        if let Some(h) = delivery::host_header_for(&m.base_url) {
            pf.opts.insert("header".to_string(), h);
        }
    }
    Some(pf)
}

/// 把一批文件逐个 `addUri` 进引擎。**RPC 全部在锁外**；每成功一个，才短暂进锁记 intent、复位复校验闸门。
///
/// `epoch` 是这一批取目标时的批次代数。进锁时若已变（换了码），刚加的那个立刻摘掉并停下：
/// 它属于已经作废的批次，不得记进新批次的 intent。
///
/// 暂停全部按住期间加入的，立刻暂停；暂停不成功则摘掉并记为拒绝——不让它在按住期间悄悄写盘。
///
/// 返回 `(added, rejected, stopped)`。`stopped` 为 `Some((下标, 理由))` 时，**从该下标起的文件都未提交**，
/// 由调用方登记（队列里的由 [`feed_once`] 登记，同步的由 [`hand_over`] 登记）。
/// intent 在整批结束后统一落盘一次（逐个落盘是 O(N²) 的写放大）。
fn submit_off_lock(
    kernel: &Arc<Mutex<Kernel>>,
    d: &Arc<Daemon>,
    m: &Manifest,
    epoch: u64,
    targets: &[delivery::File],
) -> (Vec<Value>, Vec<Value>, Option<(usize, String)>) {
    let mut added: Vec<Value> = Vec::new();
    let mut rejected: Vec<Value> = Vec::new();
    let mut stopped: Option<(usize, String)> = None;
    let mut intent_changed = false;
    // 每个提交操作解析一次、整批复用（规格 §5.3）。
    let ips = resolve_ips(&m.base_url);
    // 多 IP 构造计数挂在 Kernel 的计数器上（跨换引擎存活）；这里借一次锁只取 Arc 克隆。
    let counters = Arc::clone(&lock(kernel).counters);
    for (i, f) in targets.iter().enumerate() {
        // 路径守卫（契约 §3.2）；`dir`/`out` 由 manifest.path 拆出且**不得规范化**（§3.1）
        let Some(pf) = plan_file(m, f, &ips, &counters) else {
            rejected.push(json!({"path": f.path, "reason": "路径不安全（越界/控制字符/空段）"}));
            continue;
        };
        let gid = match d.add(&pf.uris, &pf.dir, &pf.out, &pf.opts) {
            Ok(gid) => gid,
            Err(e) => {
                let body = rpc_failure_off_lock(kernel, d, "aria2.addUri", e.clone());
                if body.code == kcodes::ENGINE_DISCONNECTED {
                    stopped = Some((i, format!("未提交：{}", short_cause(&body.message))));
                    break;
                }
                rejected.push(json!({"path": f.path, "reason": e}));
                continue;
            }
        };
        let (current, held) = {
            let mut k = lock(kernel);
            if k.batch_epoch == epoch {
                // 重下必须重新过一遍校验（旧的结论已经不成立了）——见 `reopen_for_download`。
                reopen_for_download(&mut k, &f.path);
                intent_changed |= k.intent.add(&f.path);
                (true, k.hold_auto_retry)
            } else {
                (false, false)
            }
        };
        if !current {
            let _ = d.remove(&gid);
            stopped = Some((i, REPLACED_REASON.to_string()));
            break;
        }
        if held {
            if let Err(e) = d.pause(&gid) {
                let _ = d.remove(&gid);
                rejected.push(json!({"path": f.path, "reason": e}));
                continue;
            }
        }
        added.push(json!({"gid": gid, "path": f.path}));
    }
    if intent_changed {
        lock(kernel).save_intent();
    }
    (added, rejected, stopped)
}

/// 锁外版的 [`Kernel::on_rpc_failure`]：探活（网络 I/O）在锁外，只有判定那一下进锁。
fn rpc_failure_off_lock(
    kernel: &Arc<Mutex<Kernel>>,
    d: &Arc<Daemon>,
    op: &str,
    err: String,
) -> ErrorBody {
    let ping_ok = d.ping().is_ok();
    lock(kernel).settle_rpc_failure(ping_ok, op, err)
}

/// 同步提交一批之后的收尾（`enqueue` 与补回队列共用这一份）。
///
/// 一路顺利 ⇒ `later` 记 intent、进后台队列，返回待提交个数；
/// 否则（引擎断开 / 换码）⇒ 本批余下的与 `later` 全部登记为拒绝，返回的就是这些拒绝项。
/// 调用方持有 `k`（锁内）；入队之后由调用方在锁外 [`ensure_feeder`]。
fn hand_over(
    k: &mut Kernel,
    epoch: u64,
    now: &[delivery::File],
    later: &[delivery::File],
    stopped: Option<(usize, String)>,
) -> (usize, Vec<Value>) {
    let stop = stopped.or_else(|| {
        (k.batch_epoch != epoch).then(|| (now.len(), REPLACED_REASON.to_string()))
    });
    match stop {
        None => {
            if later.is_empty() {
                return (0, Vec::new());
            }
            for f in later {
                k.intent.add(&f.path);
            }
            k.save_intent();
            k.feed_queue.extend(later.iter().cloned());
            (later.len(), Vec::new())
        }
        Some((at, reason)) => {
            let rejected: Vec<Value> = now[at..]
                .iter()
                .chain(later)
                .map(|f| json!({"path": f.path, "reason": reason}))
                .collect();
            (0, rejected)
        }
    }
}

/// 把一条内核错误消息压成**一行短因果**，给"未提交"那类逐条理由用。
///
/// 为什么需要它：同一个原因会被挂到**几十条**拒绝理由上（现场那次是 90 条）。
/// 逐条重复整句「下载引擎已断开：RPC 请求失败：http://… Network Error: … os error 10060」
/// 会让界面变成一堵墙，而客户要的判断只有"为什么没下上"这一句。
///
/// ⚠️ **只截第一个冒号之前那一段，绝不改写内核的措辞**（约束 3：拒绝理由是内核原文，
///    前面那一句本来就是 `ENGINE_DISCONNECTED` 的固定前缀）。截不出东西时
///    **原样返回整串**——宁可长，不可空（空理由正是约束 4 明禁的静默失效）。
fn short_cause(message: &str) -> String {
    match message.split_once('：') {
        Some((head, _)) if !head.is_empty() => head.to_string(),
        _ => message.to_string(),
    }
}

/// 用户重新下载一条路径时，把复校验的三道闸门一起打开。
///
/// 三样都必须清，理由各不相同：
///   - `verified`：上一轮的结论（"这个文件已有最终归类"）已经不成立；
///   - `retried`：简报第 9 条"只自动重入队一次"的计数要重新开始；
///   - `verifying`：**漏掉它是最重的那一个**——万一上一轮复校验因为任何原因没走完，
///     这个路径会**永久**留在 `verifying` 里，落地监听器之后再也不会看它一眼，
///     客户重新下载了也不会被复校验（**静默少交**）。
///
/// ⚠️ **两个入口必须同向**：`op_enqueue`（把文件加进下载）与 `task_action` 的 `retry`
/// （重试一个任务）都是"客户重新下载"，曾经只有前者清了 `verifying`——
/// 同一条规则两份实现、只差一项，正是契约 §1.7 的教训。
/// 抽出这一个函数就是为了让它们**结构上不可能不一致**。
fn reopen_for_download(k: &mut Kernel, path: &str) {
    k.verified.remove(path);
    k.verifying.remove(path);
    k.retried.remove(path);
}

/// `complete` 的**唯一**推导式（简报第 8 条：只算一次，不得让别处自行推导）。
///
/// ⚠️ 它被三个调用点用：状态变了之后的 [`ensure_complete`]、`op_load_delivery`
/// 装批次时、以及非严格的 `op_plan` 返回时顺手刷新。**三处必须走这个函数**——
/// 第一版里 `op_plan` 自己又写了一遍 `kind == Kind::Skip` 的过滤，于是
/// "把另一份推导改反"这个变异体被这份正确实现掩盖了，测试全绿（变异实验 M9 实测）。
/// 这正是契约 §1.7 的教训：同一条规则有多份实现，就会漂移出真缺陷。
fn complete_from(todo: &planner::Todo) -> BTreeSet<String> {
    todo.items
        .iter()
        .filter(|i| i.kind == Kind::Skip)
        .map(|i| i.file.path.clone())
        .collect()
}

/// `transfer_list`：传输列表快照。
///
/// ⚠️ **简报第 11 条**：每一项的 `raw_status` 只从 `snapshot().raw_status.get(&task.gid)` 取，
/// **不得**从 `task.state` 反推——领域模型有意把 `paused` 与 `waiting` 映成同一个变体
/// （契约 §8 的 `#14`），反推就等于把那个区分又抹掉，而设计规格 §8.4 要求界面标注「已暂停」。
pub fn op_transfer_list(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let d = k.read_engine()?;
    let snap = match d.snapshot() {
        Ok(s) => s,
        Err(e) => return Err(k.on_rpc_failure(&d, "快照（getGlobalStat/tellActive/tellWaiting/tellStopped）", e)),
    };
    // `snap.tasks` 的重复 GID 已在 `Daemon::snapshot` 里折过（生产者持有该不变式），
    // 且与 `raw_status` 一一对应——这里直接用，不再自己折一次。
    let items: Vec<Value> = snap
        .tasks
        .iter()
        .map(|t| {
            // 取不到就传空串，让壳显式看到"不知道"（**不要**退回去反推）
            let raw = snap.raw_status.get(&t.gid).map(String::as_str).unwrap_or("");
            let path = snap.path_map.get(&t.gid).cloned();
            to_value(&TransferItem::new(t, raw, path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"items": items, "global": to_value(&snap.global)?}))
}

/// `task_action`：`pause` / `unpause` / `retry` / `remove` / `clear_finished`。
///
/// **没有 `reveal`**——"在访达中显示"是壳的事（设计规格 §5.2、全局约束 2）。
pub fn op_task_action(k: &mut Kernel, params: &Value) -> Result<Value, ErrorBody> {
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ErrorBody::invalid_params("task_action 需要 action 参数"))?;
    let gid = params.get("gid").and_then(Value::as_str).unwrap_or("");
    let need_gid = || {
        if gid.is_empty() {
            Err(ErrorBody::invalid_params(format!("动作 {action:?} 需要 gid")))
        } else {
            Ok(gid.to_string())
        }
    };

    // ⚠️ **先判动作名，再碰引擎**：一个不存在的动作（比如 `reveal`）不该因为
    // "引擎没起来"而报成 `engine_not_started`——那是两件不同的事，
    // 而且壳需要"这个动作不存在"这条明确结论才能把界面上的按钮去掉。
    if !matches!(
        action,
        "pause" | "unpause" | "retry" | "remove" | "clear_finished" | "pause_all" | "resume_all"
    ) {
        return Err(ErrorBody::invalid_params(format!(
            "不认识的动作 {action:?}（可用：pause / unpause / retry / remove / clear_finished / pause_all / resume_all）"
        )));
    }
    // `resume_all` 只能从 `handle` 的分流点进来（它需要 `Arc<Mutex<Kernel>>` 才能把
    // `complete` 放在锁外算）。走到这里说明有人绕过了分流——大声报错，不要静默当成别的。
    if action == "resume_all" {
        return Err(ErrorBody::invalid_params(
            "resume_all 必须经 handle 分流（内部错误：不应走到 op_task_action）",
        ));
    }

    // 暂停全部在「引擎根本没起来」时是成功的空操作（没有东西可停；也别为此启动 aria2）。
    if action == "pause_all" && k.daemon.is_none() {
        k.hold_auto_retry = true;
        return Ok(json!({"action": "pause_all"}));
    }

    let d = k.require_engine_for_action()?;
    let rpc = |r: Result<(), String>| -> Result<Value, ErrorBody> {
        match r {
            Ok(()) => Ok(json!({"action": action})),
            Err(e) => Err(ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e)),
        }
    };

    match action {
        "pause" => {
            let gid = need_gid()?;
            rpc(d.pause(&gid))
        }
        "unpause" => {
            let gid = need_gid()?;
            rpc(d.unpause(&gid))
        }
        "remove" => {
            let gid = need_gid()?;
            // 先取路径（remove 会 forget 掉映射）：用户亲手移除的任务不该在换引擎后被补回来。
            let rel = d.path_for(&gid);
            let r = rpc(d.remove(&gid));
            if r.is_ok() {
                if let Some(rel) = rel {
                    if k.intent.remove(&rel) {
                        k.save_intent();
                    }
                }
            }
            r
        }
        "clear_finished" => rpc(d.clear_finished()),
        "pause_all" => {
            let r = rpc(d.pause_all());
            if r.is_ok() {
                k.hold_auto_retry = true;
            }
            r
        }
        // ── 简报第 7 条：`retry` 是**编排层**的两步，不是 aria2 动作 ──────────
        "retry" => {
            let gid = need_gid()?;
            let m = k.require_manifest()?;
            // ⚠️ **必须在 remove 之前取路径**：remove 会 `forget` 掉这条映射，
            // 之后就再也认不出这个 GID 对应清单里的哪个文件了。
            let rel = d.path_for(&gid).ok_or_else(|| {
                ErrorBody::invalid_params(format!("GID {gid} 没有路径映射（可能已被移除）"))
            })?;
            let f = m
                .files
                .iter()
                .find(|f| f.path == rel)
                .cloned()
                .ok_or_else(|| {
                    ErrorBody::invalid_params(format!("清单里没有 {rel:?}（批次可能已换）"))
                })?;
            // 每个提交操作解析一次、整批复用（规格 §5.3）。
            // ⚠️ **R9 批次债**：本段（DNS + remove/add RPC）在**内核锁内**执行——
            //    DNS 无超时上界，慢解析会冻协议循环。随「锁内取输入、锁外做 RPC」
            //    （F13/R9）一并搬出锁外；本波不重构，只登记。
            let ips = resolve_ips(&m.base_url);
            let pf = plan_file(&m, &f, &ips, &k.counters).ok_or_else(|| {
                ErrorBody::invalid_params(format!("{} 的路径不安全，拒绝重试", f.path))
            })?;

            d.remove(&gid)
                .map_err(|e| ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e))?;
            // 用**当前的**逐任务选项重新 add（`Daemon::add` 内部取的就是当前那份）；
            // `-c` 恒开，所以这一下是从已下的部分续传。
            let new_gid = d
                .add(&pf.uris, &pf.dir, &pf.out, &pf.opts)
                .map_err(|e| ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e))?;
            // `retry` 同样是"客户重新下载"的入口，闸门要开得与 `enqueue` **一模一样**
            // ——包括 `verifying`（曾经这里漏了它，见 `reopen_for_download` 的注释）。
            reopen_for_download(k, &f.path);
            Ok(json!({"action": "retry", "gid": new_gid, "path": f.path}))
        }
        // 上面已经把所有合法动作列完，这一支不可达；留着是为了 match 的穷尽性。
        other => Err(ErrorBody::invalid_params(format!(
            "不认识的动作 {other:?}（可用：pause / unpause / retry / remove / clear_finished / pause_all / resume_all）"
        ))),
    }
}

/// `migrate_start`：把旧下载目录里已下载的文件（含断点控制文件）搬到当前下载目录。
///
/// 参数 `from`：旧目录的绝对路径（壳在改目录之前记下的那一个）。立刻返回，
/// 真正的搬运在后台线程里，进度走 `migrate_status`。
///
/// 要搬哪些：清单里**每一个**在旧目录有、新目录没有的文件——不只是 `intent` 里的。
/// 已经下完的文件也要搬，否则客户换盘之后它们会被当成「没下过」重新下载。
pub fn op_migrate_start(kernel: &Arc<Mutex<Kernel>>, params: &Value) -> Result<Value, ErrorBody> {
    let from = params
        .get("from")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ErrorBody::invalid_params("migrate_start 需要 from 参数（旧下载目录）"))?;
    let from = PathBuf::from(from);
    if !from.is_dir() {
        return Err(ErrorBody::invalid_params(format!(
            "旧下载目录不存在或不是目录：{}",
            from.display()
        )));
    }

    let (to, wanted, status) = {
        let k = lock(kernel);
        let m = k.require_manifest()?;
        // 同一个目录（含符号链接 / 大小写别名指向同处）⇒ 无事可做，且绝不能「自己搬自己」。
        let same = match (from.canonicalize(), k.download_dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => from == k.download_dir,
        };
        if same {
            return Err(ErrorBody::invalid_params("旧目录与当前下载目录是同一个，无需搬运"));
        }
        let mut st = k.migrate.lock().unwrap_or_else(PoisonError::into_inner);
        if st.running {
            return Err(ErrorBody::invalid_params("上一次搬运还没结束"));
        }
        *st = migrate::Status {
            running: true,
            ..Default::default()
        };
        drop(st);
        let wanted: Vec<String> = m.files.iter().map(|f| f.path.clone()).collect();
        (k.download_dir.clone(), wanted, Arc::clone(&k.migrate))
    };

    let reset = Arc::clone(&status);
    let kernel = Arc::clone(kernel);
    thread::Builder::new()
        .name("migrate".to_string())
        .spawn(move || {
            migrate::run(&from, &to, &wanted, false, &status);
            let s = status.lock().unwrap_or_else(PoisonError::into_inner).clone();
            crate::diagnostics::log(
                "migrate_done",
                &[
                    ("files", s.files_total.to_string()),
                    ("bytes", s.bytes_total.to_string()),
                    ("errors", s.errors.len().to_string()),
                    ("skipped", s.skipped.len().to_string()),
                ],
            );
            // 盘上变了：`complete` 要重算（搬来的完整文件现在应当被认出来）。
            lock(&kernel).complete_dirty = true;
        })
        .map_err(|e| {
            // 线程没起来 ⇒ 没有人会把 `running` 复位；不在这里复位，搬运门会永久挡住一键开始与自愈。
            reset.lock().unwrap_or_else(PoisonError::into_inner).running = false;
            ErrorBody::new(codes::INTERNAL, format!("起搬运线程失败：{e}"))
        })?;
    Ok(json!({"started": true}))
}

/// `get_download_dir`：内核此刻使用的下载目录（绝对路径原文）。
///
/// 为什么需要：偏好里「未配置」时落盘根是内核的默认值，**壳不知道它是哪儿**——
/// 而换目录之前壳必须记下旧目录，才能在新内核起来之后让它把旧文件搬过来。
pub fn op_get_download_dir(k: &mut Kernel) -> Result<Value, ErrorBody> {
    Ok(json!({"dir": k.download_dir.to_string_lossy()}))
}

/// `migrate_status`：搬运进度（从没搬过 ⇒ 全零、`finished: false`）。
pub fn op_migrate_status(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let s = k.migrate.lock().unwrap_or_else(PoisonError::into_inner).clone();
    to_value(&s)
}

/// 补回队列的结果（回执的 `added` / `rejected` 与 `enqueue` 同形）。
#[derive(Default)]
struct Restored {
    added: Vec<Value>,
    rejected: Vec<Value>,
}

/// 这批文件**还需要多少字节**：每个文件的大小减去盘上已有的长度（不为负）。
///
/// 为什么不像 `enqueue` 那样直接用全量大小：补回队列时半截文件往往已经在盘上
/// （刚从旧目录搬来），按全量算会把「只差一点就下完」的批次误拦成「磁盘不够」。
/// 稀疏文件的长度可能高估已下的量 ⇒ 这里**略微低估**需求，方向是放行；
/// 磁盘真不够时 aria2 写盘报错会如实出现在任务上。
fn remaining_bytes(dir: &Path, files: &[delivery::File]) -> i64 {
    files
        .iter()
        .map(|f| {
            let have = std::fs::metadata(dir.join(&f.path))
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            (f.size - have).max(0)
        })
        .sum()
}

/// 补回队列要提交的那一批（[`restore_batch`] 的产物）。
struct RestoreBatch {
    m: Manifest,
    epoch: u64,
    targets: Vec<delivery::File>,
}

/// **锁外取数**：`intent` 里「没完成、引擎里也没有、也不在后台队列里」的文件。没有要补的 ⇒ `None`。
///
/// `check_space` 为真时先做开工前检查（用户主动「一键开始」时要）。它**必须在解除暂停之前跑**：
/// 否则暂停的任务已经全部恢复写盘，磁盘不够的报错就来晚了。引擎被自愈换掉后的静默补回不要——
/// 那批文件当初已经过过一次检查。
fn restore_batch(
    kernel: &Arc<Mutex<Kernel>>,
    d: &Arc<Daemon>,
    check_space: bool,
) -> Result<Option<RestoreBatch>, ErrorBody> {
    let (m, epoch, dir, wanted, queued, lang) = {
        let k = lock(kernel);
        let lang = k.language;
        let Some(m) = k.manifest.clone() else {
            return Ok(None);
        };
        let wanted: BTreeSet<String> = k.intent.unfinished(&k.complete).into_iter().collect();
        if wanted.is_empty() {
            return Ok(None);
        }
        let queued: BTreeSet<String> = k.feed_queue.iter().map(|f| f.path.clone()).collect();
        (m, k.batch_epoch, k.download_dir.clone(), wanted, queued, lang)
    };
    let present: BTreeSet<String> = match d.snapshot() {
        Ok(s) => s.path_map.into_values().collect(),
        Err(e) => return Err(rpc_failure_off_lock(kernel, d, "快照（补回队列）", e)),
    };
    let targets: Vec<delivery::File> = m
        .files
        .iter()
        .filter(|f| {
            wanted.contains(&f.path) && !present.contains(&f.path) && !queued.contains(&f.path)
        })
        .cloned()
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }
    if check_space {
        preflight(lang, &dir, remaining_bytes(&dir, &targets))?;
    }
    Ok(Some(RestoreBatch { m, epoch, targets }))
}

/// **锁外提交**补回队列的那一批；超出一批的交给后台线程。返回回执（与 `enqueue` 同形）。
fn submit_restore(kernel: &Arc<Mutex<Kernel>>, d: &Arc<Daemon>, b: RestoreBatch) -> Restored {
    let (now, later) = split_for_feeding(&b.targets);
    let (added, mut rejected, stopped) = submit_off_lock(kernel, d, &b.m, b.epoch, now);
    let (queued, extra) = {
        let mut k = lock(kernel);
        hand_over(&mut k, b.epoch, now, later, stopped)
    };
    crate::diagnostics::log(
        "queue_restore",
        &[
            ("wanted", b.targets.len().to_string()),
            ("added", added.len().to_string()),
            ("rejected", (rejected.len() + extra.len()).to_string()),
            ("queued", queued.to_string()),
        ],
    );
    rejected.extend(extra);
    ensure_feeder(kernel);
    Restored { added, rejected }
}

/// 引擎被自愈换掉之后，把队列补回来（[`replace_engine`] 成功后调用）。
///
/// 搬运中不补：搬运与引擎不能同写一个文件（规格 §6）；搬完后由用户「一键开始」补回（`intent` 还在）。
/// 失败一律只记日志：补不回来的路径还在 `intent` 里，用户点「一键开始」时会再补一次。
pub fn restore_queue_after_replace(kernel: &Arc<Mutex<Kernel>>) {
    ensure_complete(kernel);
    let Some(d) = lock(kernel).daemon.clone() else {
        return;
    };
    if migrating(&lock(kernel)) {
        crate::diagnostics::log("queue_restore_skipped", &[("reason", "migrating".to_string())]);
        return;
    }
    match restore_batch(kernel, &d, false) {
        Ok(Some(b)) => {
            let _ = submit_restore(kernel, &d, b);
        }
        Ok(None) => {}
        Err(e) => crate::diagnostics::log("queue_restore_failed", &[("err", e.message)]),
    }
}

/// `task_action` 的 `resume_all`（「一键开始」）：先查磁盘，再解开引擎里被暂停的全部，最后把
/// `intent` 里引擎没有的补回来。
///
/// ⚠️ 不裹 `with_kernel`：`ensure_complete` 内含 HEAD 请求，必须在锁外跑。
/// 没有可做的事（引擎没起、`intent` 也没有未完成的）⇒ 成功的空操作，**不启动 aria2**
/// （与 `enqueue` 在无待下载时的口径一致）。
pub fn op_resume_all(kernel: &Arc<Mutex<Kernel>>) -> Result<Value, ErrorBody> {
    // 搬运期间不许开始：引擎与搬运同写一个文件是数据损坏级的风险（规格 §6）。
    {
        let k = lock(kernel);
        let m = k.migrate.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if m.running {
            return Err(ErrorBody::invalid_params(format!(
                "正在搬运已下载的文件（{}/{}），搬完后再点「一键开始」",
                m.files_done, m.files_total
            )));
        }
    }
    ensure_complete(kernel);
    let d = {
        let mut k = lock(kernel);
        if k.daemon.is_none() && k.intent.unfinished(&k.complete).is_empty() {
            k.hold_auto_retry = false;
            return Ok(json!({"action": "resume_all", "added": [], "rejected": []}));
        }
        if k.daemon.is_none() {
            k.ensure_engine()?
        } else {
            k.require_engine_for_action()?
        }
    };
    // 先查磁盘，再解开暂停：查不过就什么都不动（暂停的任务留在暂停态，按住也不放）。
    let batch = restore_batch(kernel, &d, true)?;
    lock(kernel).hold_auto_retry = false;
    d.unpause_all()
        .map_err(|e| ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e))?;
    let r = match batch {
        Some(b) => submit_restore(kernel, &d, b),
        None => Restored::default(),
    };
    Ok(json!({"action": "resume_all", "added": r.added, "rejected": r.rejected}))
}

impl Kernel {
    /// `task_action` 用的取引擎：**动作**要么成功要么明确报错，
    /// 不像读路径那样把"断开"也当成一种可接受的状态。判定同样走
    /// [`reconnect_outcome`]（一个判定点，不许有两份表）。
    fn require_engine_for_action(&mut self) -> Result<Arc<Daemon>, ErrorBody> {
        let d = self.read_engine()?;
        let ping = d.ping();
        let ok = ping.is_ok();
        let cause = ping.err().unwrap_or_default();
        let verdict = reconnect_outcome(ok, self.engine_busy);
        // 与 `on_rpc_failure` 同一条口子：这里是**动作**路径上的断开判定
        // （`on_rpc_failure` 管的是读/写方法），漏了它就等于"从动作路径闩死的断开"
        // 在日志里没有对应的一行。
        self.set_engine_disconnected(verdict.is_dead(), &format!("action: {cause}"));
        if let Some(e) = verdict.error(self.language, "ping", &cause) {
            return Err(e);
        }
        Ok(d)
    }

    /// 把命令行给的一次性覆盖写进**内存中的** [`Settings`]（`-j` / `-x` / `-s`）。
    ///
    /// ⚠️ **绝不调 [`op_set_settings`]**：那个会**写盘**（它就是图形界面「保存」按钮的
    /// 实现）。`-j/-x/-s` 只作用**这一次运行**，写盘就等于静默改掉客户的配置
    /// （设计规格 §9 / 全局约束 3）。本方法只动这三个字段，`settings.json` 一个字节都不变。
    pub fn apply_overrides_in_memory(
        &mut self,
        parallel: Option<i32>,
        connections: Option<i32>,
        splits: Option<i32>,
    ) {
        if let Some(v) = parallel {
            self.settings.parallel = v;
        }
        if let Some(v) = connections {
            self.settings.connections = v;
        }
        if let Some(v) = splits {
            self.settings.splits = v;
        }
    }

    /// 取走引擎句柄，收尾用。**只交出所有权、不关引擎**——关不关由调用方决定，
    /// 而且必须在**锁外** `close`（它最坏要等满 5 秒的兜底）。
    /// **`engine_disconnected` 的唯一写入点**（规格 §3 A6）。
    ///
    /// 只在**值发生变化**时落一条诊断：这个标志在重连路径上被反复置位，
    /// "每次都记"会把日志刷成同一行的复读机，而真正要回答的问题是
    /// "**从哪一刻起**引擎被认为没了、**因为什么**"。
    ///
    /// `why` 是给人看的因果串，四个调用点各传各的：
    ///   - `on_rpc_failure` 传 `调用名: 错误`；
    ///   - `require_engine_for_action` 在探活的错误串前加一个 `action: ` 前缀
    ///     （好把"动作路径闩死的"与"读路径闩死的"在日志里分开）；
    ///   - `ensure_engine` 传它**自己那条探活路径的成败短语**（`ensure_engine_probe_ok` /
    ///     `ensure_engine_probe_failed`）——**不是**探活的错误串：探活成功时那个串是空的，
    ///     而"true → false"这一格**只**在探活成功时发生，传它会让日志以 `why=` 结尾；
    ///   - [`spawn_recovery`] 传 `"recovery_probe"`。
    ///
    /// 值里的空格在 `diagnostics::format_line` 里换成 `_`。
    ///
    /// ⚠️ 这是**对外接口**：另外两条恢复路径都按这个签名调——`spawn_recovery` 的
    ///     后台探活，以及 `ensure_engine` 的读路径就地探活。签名与"只在变化时记"
    ///     都是定死的，别改成别的形状。
    fn set_engine_disconnected(&mut self, now: bool, why: &str) {
        if now != self.engine_disconnected {
            // `exited` 是**这个比特**（2026-10-09）：RPC 超时这一个症状对应两种完全不同的
            // 情形 —— 引擎**进程没了**（该有人把它拉起来）与**进程活着但不吭声**
            // （该查它为什么卡）。此前两者在日志里长得一模一样，0.2.3 那份 9.4 小时的
            // 记录里 33 次「已断开」没有一次能分开它们。
            // 在**判定发生的那一刻**记最准：晚一点记，那个进程可能已经变了。
            crate::diagnostics::log(
                "engine_disconnected_changed",
                &[
                    ("now", now.to_string()),
                    ("why", why.to_string()),
                    ("exited", self.engine_exited().to_string()),
                ],
            );
            // 判成断开的那一刻顺手采一次引擎进程的读数（CPU / IO / 句柄），与上一次做差：
            // 这是下一次导出里唯一能分开「空转 / 在写盘 / 被阻塞」的证据。仅 Windows 有读数。
            if now {
                let pid = self.daemon.as_ref().and_then(|d| d.pid());
                crate::procstat::log_sample(pid, &mut self.stall_prev, "disconnected");
            }
        }
        self.engine_disconnected = now;
    }

    /// 引擎子进程**是否已经退出**。**还没有引擎**（`None`）时报 `true`。
    ///
    /// `None` 报 `true` 的理由：`daemon` 为 `None` 时确实**没有**一个活着的引擎，
    /// 而这一格要回答的问题正是"还有没有引擎在"。报 `false` 会把"还没起来"
    /// 与"起来了但不应答"混成同一格 —— 那恰好是这次要分开的两件事。
    fn engine_exited(&self) -> bool {
        match &self.daemon {
            Some(d) => d.process_exited(),
            None => true,
        }
    }

    /// 换码：后台队列里还没提交的全部登记为拒绝，并把批次代数顶过去。正在飞的那一批
    /// （[`submit_off_lock`]）进锁时发现代数变了，会摘掉刚加的任务、不再记账。
    fn discard_feed_queue(&mut self) {
        for f in self.feed_queue.drain(..) {
            self.feed_rejected.push(json!({"path": f.path, "reason": REPLACED_REASON}));
        }
        self.batch_epoch += 1;
    }

    pub fn take_daemon(&mut self) -> Option<Arc<Daemon>> {
        self.daemon.take()
    }
}

/// `verify_status`：六类校验结果（互斥穷尽，不得静默少交）。
pub fn op_verify_status(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let r = &k.verify;
    Ok(json!({
        "ok": r.ok,
        "bad": r.bad,
        "missing": r.missing,
        "size_mismatch": r.size_mismatch,
        "unverifiable": r.unverifiable,
        "unreadable": r.unreadable,
        "all_good": r.all_good(),
    }))
}

/// `get_settings`：参数 + **上次交付码**。
///
/// 交付码是**单独返回**的，不在 `settings` 里面——它是"上次用过什么"的运行状态，
/// 不是参数面板上的那一项（简报第 6 条）。
pub fn op_get_settings(k: &mut Kernel) -> Result<Value, ErrorBody> {
    Ok(json!({
        "settings": to_value(&k.settings)?,
        "last_code": k.last_code,
    }))
}

/// `delivery_progress`：**只读**，壳在"解析交付码"那一步逐条显示这个数组。
///
/// 返回 `{state, origin, lines}`；`state` 是 `idle`/`resolving`/`hit_domestic`/
/// `hit_international`/`failed`。解析完成后壳把整块折叠成一行（规格 W4.3）。
pub fn op_delivery_progress(k: &mut Kernel) -> Result<Value, ErrorBody> {
    Ok(json!({
        "state": k.resolve_state,
        "origin": k.resolve_origin.map(|o| o.wire()),
        "lines": k.resolve_lines,
    }))
}

/// `restart_progress`：**只读**，给界面画"正在重启下载引擎（第 1/3 次）…"。
///
/// 返回 `{attempt, budget, state, sinceMs, fails}`；`state` 是四个词：
/// `idle` / `waiting`（等退避）/ `verifying`（等进度证明）/ `exhausted`（预算用尽）。
pub fn op_restart_progress(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let p = k.restart.progress();
    Ok(json!({
        "attempt": p.attempt,
        "budget": p.budget,
        "state": p.state,
        "sinceMs": p.since_ms,
        "fails": p.fails,
    }))
}

/// `error_recent`：**只读**，壳轮询它来画"持续出错的横幅"。
///
/// 返回最近 30 秒内见过的客户可见错误（同类只算一条，带次数），按严重度降序，
/// **三句话也一起给**——文案在 Rust（规格 I-1），壳不做翻译、不自己拼句子。
pub fn op_error_recent(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let lang = k.language;
    let now = now_ms();
    let entries: Vec<Value> = k
        .errors
        .snapshot(now)
        .into_iter()
        .map(|e| {
            let c = e.class;
            json!({
                "class": c.wire(),
                "severity": c.severity(),
                "count": e.count,
                "repeats": e.repeats(),
                "title": c.title(lang),
                "explain": c.explain(lang),
                "action": c.action(lang),
            })
        })
        .collect();
    Ok(json!({ "entries": entries }))
}

/// `update_status`：**只读**。壳轮询它，与 `transfer_list` 同一条路子。
///
/// ⚠️ **任何取不到的情形都回成功**（`has_newer:false`），不是错误 ——
///    否则壳那边会冒出一句客户看不懂的报错（规格 §6.6）。
pub fn op_update_status(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let current = env!("CARGO_PKG_VERSION");
    // ⚠️ **现算**，不存。（存的判定会过期；过期的表现是"提示一个已经装上的版本"。）
    let has_newer = k
        .update
        .last_seen
        .as_deref()
        .is_some_and(|latest| update::is_newer(latest, current));
    let url = k
        .update
        .last_seen
        .as_deref()
        .and_then(update::parse_version)
        .map(|v| update::download_url(v, &update::asset_name(update::platform(), update::arch(), v)));
    Ok(json!({
        "enabled": k.update.enabled,
        "checking": k.update_checking,
        "current": current,
        "latest": k.update.last_seen,
        "has_newer": has_newer,
        "url": url,
        "checked_at": k.update.last_checked_at,
        // ⚠️ 这一格**不在**上面的"取不到就回成功"那条口径里：`has_newer:false` 仍然是
        //    成功（界面上不许冒报错，规格 §5），但**"这次到底查成没有"必须说得出来**
        //    （手动查那颗按钮靠它分辨"没查成"与"没有新版"，见 `update_last_check_ok`）。
        "last_check_ok": k.update_last_check_ok,
    }))
}

/// `update_set_enabled`：设置里那个勾选框。
pub fn op_update_set_enabled(k: &mut Kernel, params: &Value) -> Result<Value, ErrorBody> {
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| ErrorBody::invalid_params("缺少 enabled（布尔）".to_string()))?;
    k.update.enabled = enabled;
    k.save_update_state();
    op_update_status(k)
}

/// `update_check_now`：手动触发一次。**不受节流限制**（规格 §2），但**也要落盘**
/// —— 于是手动查完 24 小时内不会再自动查一次。
///
/// ⚠️ **刻意不用 `with_kernel` 包**：它最贵的部分是网络 I/O（最坏 5 秒），
///    持着内核锁跑会让校验线程与 `transfer_list` 一起停摆 —— 与 `kernel.rs` 里
///    [`dispatch`] 上方那条既有纪律、以及 [`op_load_delivery`]/[`op_plan`]/[`op_enqueue`]
///    的"锁外取数 / 锁内安装"同源。
///    **dispatch 那一行因此写成 `"update_check_now" => op_check_update_now(kernel),`**
///    （末参数是 `kernel` 本身，不是 `with_kernel` 的闭包）。
///
/// ⚠️ 回给壳的是**查完之后**那份状态，里面带着 [`Kernel::update_last_check_ok`]：
///    设置里那颗按钮拿它分辨"没查成"与"已经是最新的"（见那只字段上的注释）。
pub fn op_check_update_now(kernel: &Arc<Mutex<Kernel>>) -> Result<Value, ErrorBody> {
    run_update_check(kernel);
    with_kernel(kernel, op_update_status)
}

/// `set_settings`：参数写入。
///
/// 三道**顺序不能换**：
///   1. `Settings::validate`（契约 §2.1 的唯一输入闸门）——失败**不调用也不写盘**；
///   2. `protocol::check_min_split_size`（契约 §2.3 的 protocol 层落点）——
///      把 `-k` 归一成规范串。**少了这一步，§2.3 点名的那个函数就没有调用者**；
///   3. 落盘，然后对**正在跑的**引擎即时生效（契约 §2.5）。
pub fn op_set_settings(k: &mut Kernel, params: &Value) -> Result<Value, ErrorBody> {
    let raw = params
        .get("settings")
        .ok_or_else(|| ErrorBody::invalid_params("set_settings 需要 settings 参数"))?;
    let mut s: Settings = serde_json::from_value(raw.clone())
        .map_err(|e| ErrorBody::invalid_params(format!("参数形状不对：{e}")))?;

    // ① 整份参数的闸门
    s.validate().map_err(ErrorBody::invalid_params)?;
    // ② 契约 §2.3：`-k` 的取值集合在 protocol 层限定，并归一成规范串下发
    s.min_split_size = protocol::check_min_split_size(&s.min_split_size)?;
    // ③ 到这一步才写盘（前面任何一步失败都不写）
    s.save_to(&k.settings_path)
        .map_err(|e| ErrorBody::new(codes::INTERNAL, format!("保存设置失败：{e}")))?;
    k.settings = s.clone();
    // ④ 这份文件从此**带 language 键**了 ⇒ 「首次启动要不要问语言」的判据也要跟着翻。
    //    🔴 漏了这一格就是真机那个死循环：首启窗选完语言 → 壳重问 `hello` → 内核还回答
    //    `language_chosen: false` → 窗子又弹回来，客户只能杀进程（文件已写，重开才不弹）。
    k.language_chosen = true;
    // ⑤ 语言也要跟着换：客户在设置里点「英文」应当立刻生效，而不是等下次启动
    //    （规格 §2.2）。这里是"设置页改语言"的唯一落点。
    //    解析用的系统语言取握手记下的那一份（`set_settings` 的入参里没有它）。
    let locale = k
        .system_locale
        .clone()
        .unwrap_or_else(crate::i18n::system_locale);
    k.language = s.language.resolve(&locale);

    // ⑤ 引擎在跑就即时生效（改参数不重启引擎）
    if let Some(d) = k.daemon.clone() {
        d.set_per_task(s.per_task_options());
        if let Err(e) = d.apply_global(&s.global_options()) {
            // 参数已经落盘了，但下发失败必须让壳看得见（不得静默失效）
            return Err(ErrorBody::new(
                kcodes::ENGINE_RPC_FAILED,
                format!("设置已保存，但下发到下载引擎失败：{e}"),
            ));
        }
    }
    Ok(json!({"settings": to_value(&s)?, "last_code": k.last_code}))
}

/// `get_state`：已校验文件记录（跨会话的那一份）。
pub fn op_get_state(k: &mut Kernel) -> Result<Value, ErrorBody> {
    let mut m = Map::new();
    for (path, e) in &k.state.files {
        m.insert(path.clone(), json!({"size": e.size, "mtime": e.mtime, "crc64": e.crc64}));
    }
    Ok(json!({
        "version": state::VERSION,
        "code": k.state.code,
        "files": Value::Object(m),
    }))
}

/// 写一行响应到 stdout。**这是整个内核唯一写 stdout 的地方。**
pub fn write_line(out: &mut impl Write, r: &Response) -> std::io::Result<()> {
    let line = match protocol::write_response(r) {
        Ok(l) => l,
        Err(e) => {
            // 不可达（见 `protocol::codes::INTERNAL`）：手搓一条最小错误行，
            // 绝不静默丢响应——壳会因为等不到配对而挂起。
            eprintln!("benagen-core: 响应序列化失败（不可达分支）: {}", e.message);
            format!(
                r#"{{"id":{},"ok":false,"error":{{"code":"internal","message":"响应序列化失败"}}}}"#,
                r.id
            )
        }
    };
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拒绝那一条错误体：数对了几个、点出了哪个字符、带上了哪条示例、class 是不是九类里那个。
    ///
    /// 判别力：把 `count` 换成常量、或丢掉 `example`、或 class 写成 `disk_full`，
    /// 这条都会红——它是客户唯一看到的措辞。
    #[test]
    fn the_invalid_filename_error_names_the_count_the_char_and_an_example() {
        let scan = crate::names::scan([
            "d/SW_vs_SG.gsea.go.GO:0002181.pdf",
            "d/ok.bin",
            "d/other*name.bin",
        ]);
        for lang in [crate::i18n::Lang::Zh, crate::i18n::Lang::En] {
            let body = invalid_filename_error(lang, &scan);
            assert_eq!(body.code, "invalid_filename");
            assert_eq!(body.class.as_deref(), Some("name_invalid"));
            assert!(body.message.contains('2'), "数出来的个数必须进句子: {}", body.message);
            assert!(
                body.message.contains("SW_vs_SG.gsea.go.GO:0002181.pdf"),
                "示例要逐字带出来: {}",
                body.message
            );
            assert!(body.message.contains('`'), "要点出具体字符: {}", body.message);
        }
    }


    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::OnceLock;
    use std::time::Instant;

    use crate::testutil::{RawResponse, StubHttp, TempDir};

    /// **延迟探针（I-2）：`load_delivery` 的"拉清单"与"全量 plan"都必须在锁外跑。**
    ///
    /// 契约 §5.2 的原话是「校验不得占住协议循环或**任何长持有的锁**」。
    /// `load_delivery` 里两段最贵的都是网络 I/O：
    ///   - `delivery::fetch`：15 秒 × 5 次 + 15 秒退避 = 最坏 **90 秒**（2026-10-12 调参；记账仍按 91.5 秒留余量）；
    ///   - `Planner::plan`：对清单里**每个 crc64 为空的文件**发一次 HEAD，每次上限 30 秒
    ///     = 最坏 **30 秒 × N**（服务端"回读失败"导致 crc64 为空是生产上真实存在的形态）。
    ///
    /// 持着内核锁跑它们，校验线程与落地监听器会一起停摆分钟级——客户看到的是整个应用卡死。
    ///
    /// **判别力（确定性，不靠时序）**：两个桩收到对应请求后**挂住不答**，
    /// 直到主线程放行。于是在"fetch / HEAD 确确实实在飞"的那一刻各探一次锁：
    ///   - 修复前（整段在 `with_kernel` 里）→ 锁被占着 → 对应断言红；
    ///   - 修复后（两段都提到锁外）→ 锁是自由的 → 绿。
    ///
    /// 两个探针**先收集结果、最后统一断言**，中途一定会放行两个桩：
    /// 这样即使红了也不会把线程留在阻塞里。
    /// 十万文件的批次里，「整棵树」是最大的一笔开销（实测 9.2 万文件 33 MB）。
    /// 壳只要一层的 `list_dir` 时，必须能说「别把树给我」。
    #[test]
    fn load_delivery_can_skip_the_tree() {
        const CODE: &str = "AbCdEfGhIjKlMnOpQrSt";
        let base: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let base_m = Arc::clone(&base);
        let srv = StubHttp::start_with(move |_req| {
            let b = base_m.get().cloned().unwrap_or_default();
            RawResponse::new(200).body(format!(
                r#"{{"code":"{CODE}","base_url":"{b}","files":[{{"path":"t/a.bin","size":6,"crc64":""}},{{"path":"t/b.bin","size":4,"crc64":""}}]}}"#
            ))
        });
        base.set(srv.base()).ok();

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            dir.join("s.json"),
        )));

        let full = op_load_delivery(&kernel, &json!({"code": CODE, "base_url": srv.base()}))
            .expect("默认要树");
        assert!(full.get("tree").is_some(), "默认必须仍然带树（macOS 在用）");

        let slim = op_load_delivery(
            &kernel,
            &json!({"code": CODE, "base_url": srv.base(), "tree": false}),
        )
        .expect("不要树也要成功");
        assert!(slim.get("tree").is_none(), "tree:false 不该带回树：{slim}");
        assert_eq!(slim["code"], json!(CODE), "概要不受影响");
    }

    /// 十万文件的批次里，底栏只要进度与默认面的规模 —— 不该为此拉回一棵树。
    #[test]
    fn get_tree_can_answer_without_the_tree() {
        const CODE: &str = "AbCdEfGhIjKlMnOpQrSt";
        let base: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let base_m = Arc::clone(&base);
        let srv = StubHttp::start_with(move |_req| {
            let b = base_m.get().cloned().unwrap_or_default();
            RawResponse::new(200).body(format!(
                r#"{{"code":"{CODE}","base_url":"{b}","files":[{{"path":"t/a.bin","size":6,"crc64":""}},{{"path":"t/b.bin","size":4,"crc64":""}}]}}"#
            ))
        });
        base.set(srv.base()).ok();

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            dir.join("s.json"),
        )));
        op_load_delivery(&kernel, &json!({"code": CODE, "base_url": srv.base()})).expect("装批次");

        let slim = op_get_tree(&kernel, &json!({"summary_only": true})).expect("只要概要");
        assert!(slim.get("tree").is_none(), "不该带树：{slim}");
        assert!(slim.get("flat").is_none(), "不该带 flat：{slim}");
        assert!(slim.get("default_selected").is_none(), "不该带路径串：{slim}");
        assert_eq!(slim["default_count"], json!(2));
        assert_eq!(slim["default_bytes"], json!(10));
        assert_eq!(slim["total_files"], json!(2));
        assert!(slim["progress"]["total_bytes"].as_i64().unwrap_or(0) > 0);

        // 默认仍要整棵树（macOS 与既有夹具都在用）
        let full = op_get_tree(&kernel, &json!({})).expect("默认整棵树");
        assert!(full.get("tree").is_some());
        assert!(full.get("flat").is_some());
    }

    /// 底栏那句「已选 N 个 · M GB」只需要三个数 —— 不该为此把十万条路径拉回来。
    #[test]
    fn selection_summary_counts_without_shipping_the_paths() {
        const CODE: &str = "AbCdEfGhIjKlMnOpQrSt";
        let base: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let base_m = Arc::clone(&base);
        let srv = StubHttp::start_with(move |_req| {
            let b = base_m.get().cloned().unwrap_or_default();
            RawResponse::new(200).body(format!(
                r#"{{"code":"{CODE}","base_url":"{b}","files":[{{"path":"t/a.bin","size":6,"crc64":""}},{{"path":"t/b.bin","size":4,"crc64":""}}]}}"#
            ))
        });
        base.set(srv.base()).ok();

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            dir.join("s.json"),
        )));
        op_load_delivery(&kernel, &json!({"code": CODE, "base_url": srv.base()})).expect("装批次");

        // 不传 paths ⇒ 默认面（所有未完成的文件），并且它就是「全部」
        let all = op_selection_summary(&kernel, &json!({})).expect("默认面");
        assert_eq!(all["count"], json!(2));
        assert_eq!(all["bytes"], json!(10));
        assert_eq!(all["pending_count"], json!(2));
        assert_eq!(all["all_selected"], json!(true));

        // 传了 paths ⇒ 只数这些；清单里没有的忽略（壳可能拿着上一批的选择）
        let some = op_selection_summary(&kernel, &json!({"paths": ["t/a.bin", "t/./nope"]}))
            .expect("子集");
        assert_eq!(some["count"], json!(1), "不存在的路径必须被丢掉");
        assert_eq!(some["bytes"], json!(6));
        assert_eq!(some["all_selected"], json!(false), "只选了一个，不是全选");
    }

    #[test]
    fn load_delivery_touches_the_network_without_holding_the_kernel_lock() {
        const CODE: &str = "AbCdEfGhIjKlMnOpQrSt";

        let release_manifest = Arc::new(AtomicBool::new(false));
        let release_head = Arc::new(AtomicBool::new(false));
        let (manifest_seen_tx, manifest_seen_rx) = mpsc::channel::<()>();
        let (head_seen_tx, head_seen_rx) = mpsc::channel::<()>();
        // 清单里的 `base_url` 必须指向**桩二**，而桩二的端口要等它起好才知道——
        // 用 `OnceLock` 让桩一的闭包在**收到请求时**（那时已经填好了）再读它。
        let files_base: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

        // 桩一：只服务清单请求，答之前先挂住（模拟"网络慢"）。
        let rel_m = Arc::clone(&release_manifest);
        let files_base_m = Arc::clone(&files_base);
        let srv_manifest = StubHttp::start_with(move |_req| {
            let _ = manifest_seen_tx.send(());
            while !rel_m.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            // 清单里的 base_url 指向**桩二**（HEAD 会打到那里去）。
            // 两个桩是两个端口、两条连接——`StubHttp` 每条连接串行服务，
            // 用两个桩才不会让第二条请求排在被挂住的第一条后面。
            let base_b = files_base_m.get().cloned().unwrap_or_default();
            RawResponse::new(200).body(format!(
                r#"{{"code":"{CODE}","base_url":"{base_b}","files":[{{"path":"t/a.bin","size":6,"crc64":""}}]}}"#
            ))
        });

        // 桩二：只服务那个 HEAD，答之前先挂住。
        let rel_h = Arc::clone(&release_head);
        let srv_files = StubHttp::start_with(move |_req| {
            let _ = head_seen_tx.send(());
            while !rel_h.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            RawResponse::new(200) // 没有 X-Tos-Hash-Crc64ecma → 记入 unverifiable
        });
        files_base
            .set(srv_files.base())
            .expect("桩二的地址只设一次");

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            dir.join("s.json"),
        )));
        let params = json!({"code": CODE, "base_url": srv_manifest.base()});

        let k2 = Arc::clone(&kernel);
        let params2 = params.clone();
        let worker = thread::spawn(move || {
            // ⚠️ 修复前这里是 `let mut k = lock(&k2); op_load_delivery(&mut k, &params2)`
            // ——整段跑在锁内，两个探针都会红（红过，见修复报告）。
            op_load_delivery(&k2, &params2)
        });

        // ── 探针一：fetch 在飞的时候，锁必须是自由的 ──────────────────────
        manifest_seen_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("桩没有收到清单请求（fetch 根本没发出去？）");
        let fetch_free = kernel_lock_is_free(&kernel);
        release_manifest.store(true, Ordering::SeqCst);

        // ── 探针二：plan 的 HEAD 在飞的时候，锁必须是自由的 ────────────────
        head_seen_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("桩没有收到 HEAD 请求（plan 根本没发出去？）");
        let plan_free = kernel_lock_is_free(&kernel);
        release_head.store(true, Ordering::SeqCst);

        let r = worker.join().expect("工作线程 panic");
        assert!(r.is_ok(), "load_delivery 应当成功: {:?}", r.err());

        assert!(
            fetch_free,
            "拉清单时仍持有内核锁：`fetch` 最坏 15s×5 + 退避 = 90 秒（记账 91.5 秒），\
             这期间校验线程与落地监听器全部停摆（契约 §5.2）。\
             把 `delivery::fetch` 提到 `with_kernel` 之外。"
        );
        assert!(
            plan_free,
            "全量 plan 时仍持有内核锁：它对每个 crc64 为空的文件发一次 HEAD，\
             每次上限 30 秒（最坏 30s × N）。把 `plan` 提到 `with_kernel` 之外。"
        );
    }

    /// 内核锁现在是不是自由的（中毒不算"被别人占着"：那正是 `with_kernel` 会恢复的情形）。
    fn kernel_lock_is_free(kernel: &Arc<Mutex<Kernel>>) -> bool {
        match kernel.try_lock() {
            Ok(_) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => true,
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }

    /// **部分重试**：只有一部分失败项进了复校验轮时，**未被重试的失败项必须保持
    /// 它第一轮的分类**留在报告里（既不得消失，也不得错位）。
    ///
    /// 这条钉的是 `verify::merge` 的调用点契约（M-5）。审查者核清的事实是：
    /// `merge` **根本不用 `first` 的三张失败表**——输出里的 `bad`/`missing`/
    /// `size_mismatch` **整体取 `retry` 的**。所以"少一个"不是被 `merge` 兜住的，
    /// 而是被**累积器**救下来的：
    ///   - `commit(&mut k, res)` 在发复校验 job **之前**就**无条件**执行过，
    ///     把第一轮结果写进了 `k.verify`；
    ///   - 随后的 `commit(&mut k, merged)` **只摘 `merged` 里出现过的路径**，
    ///     于是没被重试的那一项完好如初。
    ///
    /// **判别力**：把 `commit` 的"先摘旧类再写入"改成"整表替换"，或让 `merge` 的
    /// 失败表并上 `first` 的 → 第三条断言红。
    #[test]
    fn partial_retry_keeps_unretried_failures_in_their_first_round_class() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));

        // 第一轮：a 通过、b 不符、m 缺失。`b` 与 `m` 都是失败项。
        let first = verify::CheckResult {
            ok: vec!["a".into()],
            bad: vec!["b".into()],
            missing: vec!["m".into()],
            ..Default::default()
        };
        commit(&mut k, first.clone());
        assert_eq!(k.verify.missing, vec!["m".to_string()], "{:?}", k.verify);

        // 复校验轮只覆盖 `b`（`m` 因为 `retried` 已含它而不再重试），且 `b` 这次通过了。
        let retry = verify::CheckResult {
            ok: vec!["b".into()],
            ..Default::default()
        };
        let merged = verify::merge(first, retry);
        // `merge` 的输出里 `m` **确实不见了**——这正是它"不用 first 的失败表"的证据。
        // 少报能被挡住，靠的是下面 `commit` 的累积语义，不是 `merge`。
        assert!(
            merged.missing.is_empty(),
            "（前提）merge 的失败表整体取 retry 的，`m` 不在其中: {merged:?}"
        );

        commit(&mut k, merged);

        assert!(k.verify.bad.is_empty(), "b 已复校验通过，不该还在 bad: {:?}", k.verify);
        assert_eq!(
            k.verify.ok,
            vec!["a".to_string(), "b".to_string()],
            "a 与 b 都应当在 ok 里: {:?}",
            k.verify
        );
        assert_eq!(
            k.verify.missing,
            vec!["m".to_string()],
            "**未被重试的失败项必须保持第一轮归类**（它没进 merge 的输出，\
             全靠累积器保留）——少报是全局约束 4 明令禁止的: {:?}",
            k.verify
        );
    }

    /// **`reopen_for_download` 必须清掉全部三道闸门**（M-4）。
    ///
    /// 两个"客户重新下载"的入口（`op_enqueue` 与 `task_action` 的 `retry`）现在
    /// 都走这一个函数，所以它们**结构上不可能不一致**——而在这之前，`retry`
    /// 只清了 `verified`/`retried`、**漏了 `verifying`**：上一轮复校验若没走完，
    /// 那个路径会永久留在 `verifying` 里，落地监听器之后再也不会复校验它（静默少交）。
    ///
    /// **判别力**：从实现里删掉 `k.verifying.remove(path)` → 第二条断言红。
    #[test]
    fn reopen_for_download_clears_all_three_gates() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));
        for set in [&mut k.verified, &mut k.verifying, &mut k.retried] {
            set.insert("t/a.bin".to_string());
        }

        reopen_for_download(&mut k, "t/a.bin");

        assert!(k.verified.is_empty(), "verified 没被清: {:?}", k.verified);
        assert!(
            k.verifying.is_empty(),
            "verifying 没被清——那个路径会永久留在里面，落地监听器再也不复校验它: {:?}",
            k.verifying
        );
        assert!(k.retried.is_empty(), "retried 没被清: {:?}", k.retried);
    }

    /// 简报第 5 条的判定表（设计规格 §9 的「恢复路径」）。
    ///
    /// 守两件事：**ping 通了要清掉「已断开」**（否则引擎其实还活着、壳却永远收到
    /// 已断开，表现为"客户再点还是失败"——**那正是简报点名的 Go 缺陷**）；
    /// **ping 不通要保持断开并回 `engine_disconnected`**，且消息里必须告诉客户
    /// "已落盘的文件保留"，因为设计规格 §9 许诺的是"可以直接重试"。
    ///
    /// ⚠️ **这条测试守的是判定表，不是调用点。** `d.ping()` 那个真实调用在 e2e 里
    /// 造不出"进程死了又活过来"的场景（内嵌 aria2c 由内核自己起，端口与 secret
    /// 都只在内核进程里；`daemon.rs` 的 HTTP 桩构造器是它的私有测试助手）。
    /// 所以——**"把 `d.ping()` 换成恒定 `Err`"这个变异体仍然存活**。
    /// 这是本任务一处**显式记账的零守护**，不要以为这条测试把它守住了。
    #[test]
    fn reconnect_outcome_table() {
        assert_eq!(
            reconnect_outcome(true, false),
            EngineVerdict::Alive,
            "ping 通了：引擎还活着，只是这一次调用失败"
        );
        assert_eq!(
            reconnect_outcome(true, true),
            EngineVerdict::Alive,
            "ping 通了就是活着——忙不忙不改变这一档"
        );
        assert_eq!(
            reconnect_outcome(false, true),
            EngineVerdict::Busy,
            "ping 不通但进程在写盘：是忙，不是死"
        );
        assert_eq!(
            reconnect_outcome(false, false),
            EngineVerdict::Dead,
            "ping 不通、进程也没动：这才叫断开"
        );

        // 三档各自回什么错误（文案的唯一出口）。
        assert!(EngineVerdict::Alive.error(crate::i18n::Lang::Zh, "快照", "x").is_none());
        let busy = EngineVerdict::Busy
            .error(crate::i18n::Lang::Zh, "快照", "timed out")
            .expect("忙也要回错误：调用方不能拿一份空快照继续");
        assert_eq!(busy.code, kcodes::ENGINE_RPC_FAILED, "忙不许回「已断开」: {busy:?}");
        assert!(
            !busy.message.contains("已断开"),
            "「忙」的消息里出现「已断开」就是 0.2.7 那个误报: {}",
            busy.message
        );
        assert!(busy.message.contains("写盘"), "要说清楚它在干活: {}", busy.message);

        let dead = EngineVerdict::Dead
            .error(crate::i18n::Lang::Zh, "快照", "connection refused")
            .expect("死了必须回错误");
        assert_eq!(dead.code, kcodes::ENGINE_DISCONNECTED, "壳按这个码分支（设计规格 §9）");
        assert!(dead.message.contains("保留"), "消息必须告诉客户文件还在: {}", dead.message);
        assert!(
            dead.message.contains("connection refused"),
            "断开原因必须出现在消息里: {}",
            dead.message
        );
    }

    /// 一次很慢的往返只让阶梯要求档 2，不要求换引擎，也不是"断了"。
    ///
    /// 本用例只钉**档位**。"档 2 不置 `engine_disconnected`"这一条由**构造**保证：动作函数只吃 `&Daemon`，
    /// 结构上够不着 `Kernel` 的那个字段（动作函数在任务 10 才接入，届时这条注释仍然成立）。
    /// 运行时钉不到标志本身，别在这里假装钉到了。
    /// 判别力：把慢判据从 `Liveness::is_unresponsive` 里删掉 ⇒ 红（阶梯会停在 Observe）。
    #[test]
    fn slowness_alone_asks_for_pause_all_rather_than_a_swap() {
        let mut a = Liveness::default();
        a.record_ok(Duration::from_secs(31));
        let mut l = Ladder::default();
        let now = Instant::now();
        assert_eq!(l.next(now, &a, 0, true, false), Rung::PauseAllThenResumeAll);
    }

    fn mk_task(gid: &str, state: TaskState, completed: i64) -> Task {
        Task {
            gid: gid.to_string(),
            total: 0,
            completed,
            speed: 0,
            conns: 0,
            state,
            err: String::new(),
        }
    }

    /// 只有应当正在动的任务进入停滞判定：传完、出错、被移除的字节数永远不动，等待中的不算卡住。
    /// 判别力：删掉 `active_tasks` 里的 `filter` ⇒ 红。
    #[test]
    fn active_tasks_keeps_only_the_ones_that_should_be_moving() {
        let tasks = vec![
            mk_task("act", TaskState::Active, 10),
            mk_task("wait", TaskState::Waiting, 0),
            mk_task("done", TaskState::Complete, 500),
            mk_task("bad", TaskState::Error, 3),
            mk_task("gone", TaskState::Removed, 7),
        ];
        assert_eq!(active_tasks(&tasks), vec![("act".to_string(), 10)]);
    }

    /// 停滞起表时记一次 `record_stall(how_long)`，同一停滞不重复记。
    /// 判别力：删掉 `record_new_stalls` 里的 `record_stall` ⇒ 红。
    #[test]
    fn new_stalls_are_recorded_in_the_counters() {
        let c = crate::diag_counters::Counters::default();
        let mut already = BTreeSet::new();
        let d180 = Duration::from_secs(180);
        let d300 = Duration::from_secs(300);

        // 第一拍：两个新停滞
        let n = record_new_stalls(&c, &[("g1".to_string(), d180), ("g2".to_string(), d300)], &mut already);
        assert_eq!(n, 2);
        let s = c.snapshot();
        assert_eq!(s.task_stalls, 2);
        assert_eq!(s.stall_total, d180 + d300);

        // 第二拍：还是这两个 ⇒ 不重复记
        let n = record_new_stalls(&c, &[("g1".to_string(), d180), ("g2".to_string(), d300)], &mut already);
        assert_eq!(n, 0);
        assert_eq!(c.snapshot().task_stalls, 2);

        // g1 不再停滞（消失了）⇒ 清掉；g1 再停滞 ⇒ 重记
        let n = record_new_stalls(&c, &[("g2".to_string(), d300)], &mut already);
        assert_eq!(n, 0);
        let n = record_new_stalls(&c, &[("g1".to_string(), d180), ("g2".to_string(), d300)], &mut already);
        assert_eq!(n, 1);
        assert_eq!(c.snapshot().task_stalls, 3);
    }

    /// 重启预算的复位要同时满足"健康够久"与"下载在动"。
    /// 判别力：去掉 `&& stalled.is_zero()` ⇒ 红（最后一条断言）。
    #[test]
    fn restart_budget_needs_sustained_health_and_a_moving_download() {
        let t = Thresholds::default();
        let window = t.pre_swap_wait;
        assert!(restart_budget_is_refilled(Some(window), Duration::ZERO, &t), "健康够久、下载在动 ⇒ 复位");
        assert!(
            !restart_budget_is_refilled(Some(window - Duration::from_secs(1)), Duration::ZERO, &t),
            "健康不够久 ⇒ 不复位"
        );
        assert!(!restart_budget_is_refilled(None, Duration::ZERO, &t), "此刻不健康 ⇒ 不复位");
        assert!(
            !restart_budget_is_refilled(Some(window), Duration::from_secs(1), &t),
            "健康够久但下载全停 ⇒ 不许复位"
        );
    }

    const RPC_OK: &str = r#"{"jsonrpc":"2.0","id":"1","result":"OK"}"#;
    const RPC_ERR: &str = r#"{"jsonrpc":"2.0","id":"1","error":{"code":1,"message":"boom"}}"#;

    /// 档 1 只动**那一个**任务，且是 pause → unpause 各一次（顺序也钉住）。
    ///
    /// 判别力：把 `unpause` 那句删掉 ⇒ 任务被**永久暂停** ⇒ 本用例红（请求数与 `aria2.unpause` 存在性断言）。
    /// 断言用的是**桩收到的请求体**，不是我们的中间状态：只看中间状态的话，"到底发没发出去"验不到。
    #[test]
    fn reconnecting_one_task_pauses_then_unpauses_exactly_that_gid() {
        let srv = StubHttp::start_with(|_| {
            RawResponse::new(200).body(r#"{"jsonrpc":"2.0","id":"1","result":"OK"}"#.to_string())
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        reconnect_one(&d, "abc123", &|| false);

        let bodies: Vec<String> = srv.requests().into_iter().map(|r| r.body).collect();
        assert_eq!(bodies.len(), 2, "pause 与 unpause 各一次：{bodies:?}");
        let pause_at = bodies
            .iter()
            .position(|b| b.contains("\"aria2.pause\""))
            .expect("必须调过 aria2.pause");
        let unpause_at = bodies
            .iter()
            .position(|b| b.contains("\"aria2.unpause\""))
            .expect("必须调过 aria2.unpause");
        assert!(pause_at < unpause_at, "先暂停再恢复：{bodies:?}");
        assert!(
            bodies.iter().all(|b| b.contains("abc123")),
            "只许动那一个 gid，不许波及其它：{bodies:?}"
        );
    }

    /// 引擎**明确拒绝**了 pause：任务根本没被暂停 ⇒ 不欠账，也不许再发 unpause（那会打到一条不该被碰的任务上）。
    /// 判别力：把 `Rejected` 那一支改成欠账 ⇒ 第一条断言红；pause 被拒也去 unpause ⇒ 第二条断言红。
    #[test]
    fn a_rejected_pause_is_not_owed_and_sends_no_unpause() {
        let srv = StubHttp::start_with(|_| RawResponse::new(400).body(RPC_ERR));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        assert_eq!(reconnect_one(&d, "abc123", &|| false), None, "pause 被拒 ⇒ 没有欠账");
        assert_eq!(srv.requests().len(), 1, "pause 被拒后不许再发 unpause");
    }

    /// ⚠️ 超时 / 没回话 ≠ 没生效：读超时时请求很可能已经被 aria2 执行了，这条路上必须欠账。
    /// 用连接被拒的端点造出"没回话"（传输层失败，立即返回）。
    /// 判别力：把 `NoAnswer` 那一支改成 `None`（即上一轮那句错前提）⇒ 红。
    #[test]
    fn a_timed_out_pause_still_owes_an_unpause() {
        let d = crate::engine::daemon::stub_daemon("http://127.0.0.1:1", "s", BTreeMap::new());
        assert_eq!(reconnect_one(&d, "abc123", &|| false), Some("abc123".to_string()));
    }

    /// ⚠️ 收到了却读不懂（非 JSON）的应答，不算"明确拒绝"：请求可能已经被执行。
    /// 判别力：把 `explicit` 的判定放宽为"所有协议层失败都算拒绝" ⇒ 红。
    #[test]
    fn an_unparseable_pause_answer_still_owes_an_unpause() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body("不是 JSON"));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        assert_eq!(reconnect_one(&d, "abc123", &|| false), Some("abc123".to_string()));
    }

    /// 判别力：把 unpause 的 `Rejected` 也记成欠账 ⇒ 红。
    #[test]
    fn an_unpause_the_engine_refuses_leaves_nothing_owed() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.unpause\"") {
                RawResponse::new(400).body(RPC_ERR)
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        assert_eq!(reconnect_one(&d, "abc123", &|| false), None);
    }

    /// 造一个停滞满阈值的任务进展表（只有 `gid` 一个），返回 `(进展表, 阈值, 判定时刻)`。
    fn one_stalled_task(gid: &str) -> (TaskProgress, Thresholds, Instant) {
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        let one = [mk_task(gid, TaskState::Active, 100)];
        tp.observe(t0, &active_tasks(&one));
        let t1 = t0 + Duration::from_secs(200);
        tp.observe(t1, &active_tasks(&one));
        (tp, Thresholds::default(), t1)
    }

    /// 「暂停全部」按在我们的 stop 与 unpause 之间：停已经生效，此时 hold 为真 ⇒ 不发 unpause，也不欠账。
    /// 判别力：删掉 `relaunch` 里发 unpause 之前的那次重读 ⇒ 发出了一条 unpause ⇒ 红。
    #[test]
    fn a_hold_pressed_after_the_stop_sends_no_unpause_and_owes_nothing() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        assert!(matches!(
            relaunch(&d, "abc123", WriteOutcome::Applied, &|| true),
            Relaunch::Clean
        ));
        assert!(srv.requests().is_empty(), "客户已按暂停全部 ⇒ 不许撤掉它");
    }

    /// 停没回话（可能已经生效）且 hold 为真：同样不发 unpause，且不得落成 `Silent`（那会让档 2 收手并欠账）。
    /// 判别力：删掉重读 ⇒ 落成 Silent 并去打 unpause ⇒ 红。
    #[test]
    fn a_hold_after_an_unanswered_stop_is_clean_not_silent() {
        let d = crate::engine::daemon::stub_daemon("http://127.0.0.1:1", "s", BTreeMap::new());
        assert!(matches!(
            relaunch(&d, "abc123", WriteOutcome::NoAnswer("x".to_string()), &|| true),
            Relaunch::Clean
        ));
    }

    /// 对照：没有 hold 时 unpause 照发 —— 上面两条的"没发"才有意义。
    #[test]
    fn without_a_hold_the_unpause_still_goes_out() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        assert!(matches!(
            relaunch(&d, "abc123", WriteOutcome::Applied, &|| false),
            Relaunch::Clean
        ));
        assert_eq!(srv.requests().len(), 1, "没按 hold 就该照常 unpause");
    }

    /// 欠账经 `reconnect_most_stalled` 记进集合：只有"没回话"的那一次才记。
    /// 判别力：删掉 `reconnect_most_stalled` 里的 `owed.insert(...)` ⇒ 第一条断言红。
    /// `tellActive` 必须在场（动作前的活跃表复查），否则函数直接返回 None 走不到记账。
    #[test]
    fn reconnect_most_stalled_owes_only_the_unanswered_gid() {
        // 第一段：pause 与 unpause 都没回话 ⇒ 欠账。`tellActive` 正常应答（活跃表里有 g）。
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g"]))
            } else if req.body.contains("\"aria2.pause\"") || req.body.contains("\"aria2.unpause\"") {
                RawResponse::new(200).body("不是 JSON")
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let (mut tp, t, t1) = one_stalled_task("g");
        let mut owed = BTreeSet::new();
        assert_eq!(reconnect_most_stalled(&d, &mut tp, &t, t1, &mut owed, &|| false), Some("g".to_string()));
        assert_eq!(owed, BTreeSet::from(["g".to_string()]), "pause 与 unpause 都没回话 ⇒ 欠账");

        // 第二段：unpause 被引擎拒绝 ⇒ 不欠账。
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g"]))
            } else if req.body.contains("\"aria2.unpause\"") {
                RawResponse::new(400).body(RPC_ERR)
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let (mut tp, t, t1) = one_stalled_task("g");
        let mut owed = BTreeSet::new();
        reconnect_most_stalled(&d, &mut tp, &t, t1, &mut owed, &|| false);
        assert!(owed.is_empty(), "unpause 被引擎拒绝 ⇒ 不欠账");
    }

    /// 候选 gid 不在活跃表里（客户在采样后单独暂停了它）⇒ 一条 RPC 都不发。
    /// 判别力：去掉 `reconnect_most_stalled` 里的 `active()` 交集 ⇒ 发出 pause/unpause ⇒ 红。
    #[test]
    fn a_candidate_the_client_paused_since_sampling_is_not_touched() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                // 活跃表里只有 other，没有 g
                RawResponse::new(200).body(active_reply(&["other"]))
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let (mut tp, t, t1) = one_stalled_task("g");
        let mut owed = BTreeSet::new();
        assert_eq!(reconnect_most_stalled(&d, &mut tp, &t, t1, &mut owed, &|| false), None);
        assert!(owed.is_empty());
        assert!(
            srv.requests().iter().all(|r| {
                !r.body.contains("\"aria2.pause\"") && !r.body.contains("\"aria2.unpause\"")
            }),
            "候选已被客户单独暂停 ⇒ 一条 pause/unpause 都不发：{:?}",
            srv.requests().iter().map(|r| r.body.clone()).collect::<Vec<_>>()
        );
    }

    /// 没回话的 unpause 留到后面的拍里补发；补上了才移出。这里用两个端点模拟"先不通、后来通了"。
    /// 判别力：把 `retry_pending_unpause` 改成空操作 ⇒ 第二条断言红。
    #[test]
    fn an_unanswered_unpause_is_retried_on_a_later_tick() {
        let dead = crate::engine::daemon::stub_daemon("http://127.0.0.1:1", "s", BTreeMap::new());
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let live = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let mut pending = BTreeSet::from(["abc123".to_string()]);
        retry_pending_unpause(&dead, &mut pending, false);
        assert!(pending.contains("abc123"), "还没回话 ⇒ 留着");
        retry_pending_unpause(&live, &mut pending, false);
        assert!(pending.is_empty(), "补上了就移出");
        assert_eq!(srv.requests().len(), 1, "只在通的那一拍发出一条 unpause");
    }

    /// 引擎明确拒绝（"找不到 / 不在暂停态"）⇒ 已经没有要还的了，立即移出，不再一拍一拍地去问。
    /// 判别力：把 `retain` 改回"保留所有失败"（`NoAnswer(_) | Rejected(_)`）⇒ 红。
    #[test]
    fn a_rejected_unpause_clears_the_debt() {
        let srv = StubHttp::start_with(|_| RawResponse::new(400).body(RPC_ERR));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let mut pending = BTreeSet::from(["abc123".to_string()]);
        retry_pending_unpause(&d, &mut pending, false);
        assert!(pending.is_empty(), "被拒 ⇒ 移出");
    }

    /// 客户按过「暂停全部」时补偿整段跳过：暂停正是客户要的，补发 unpause 等于违背客户意图。
    /// 判别力：去掉 `hold` 的判断 ⇒ 集合被清空且多出一条 RPC ⇒ 红。
    #[test]
    fn a_held_pause_all_suppresses_the_compensation() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let mut pending = BTreeSet::from(["abc123".to_string()]);
        retry_pending_unpause(&d, &mut pending, true);
        assert_eq!(pending, BTreeSet::from(["abc123".to_string()]), "原样不动");
        assert!(srv.requests().is_empty(), "一条 RPC 都不发");
    }

    /// 桩里 `tellActive` 的应答：列出 `gids` 这几个正在传的任务。
    fn active_reply(gids: &[&str]) -> String {
        let list: Vec<serde_json::Value> = gids
            .iter()
            .map(|g| {
                json!({
                    "gid": g, "status": "active", "totalLength": "0", "completedLength": "0",
                    "downloadSpeed": "0", "connections": "1", "errorMessage": "",
                })
            })
            .collect();
        json!({"jsonrpc": "2.0", "id": "1", "result": list}).to_string()
    }

    /// 档 2 对每个卡住的 gid 都是「先 forcePause、后拉起」，逐个走，且从不发全局的 `forcePauseAll` / `unpauseAll`。
    /// 判别力：改成优雅的 `pause` ⇒ 红；改回全局 `pause_all`/`unpause_all` ⇒ 红；删掉某个 gid 的 `unpause` ⇒ 红。
    ///
    /// 「档 2 不置 `engine_disconnected`」**钉不到运行时**：动作只吃 `&Daemon`，结构上够不到
    /// `Kernel` 的字段（见 [`pause_then_resume_each`]）；而线程里调它的那一支活在不返回的
    /// `spawn_recovery` 循环中，任何用例都走不到。那一条由源码级判据与报告承担，这里不假装钉住。
    #[test]
    fn rung_two_pauses_then_resumes_each_gid_with_force_pause_never_the_global_pair() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["a", "b"]))
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        let debts = pause_then_resume_each(&d, &["a".to_string(), "b".to_string()], &|| false, true);

        assert!(debts.is_empty(), "两条都应答 ⇒ 不欠账");
        let seq: Vec<(String, String)> = srv
            .requests()
            .into_iter()
            .filter(|r| !r.body.contains("\"aria2.tellActive\""))
            .map(|r| {
                let v: serde_json::Value = serde_json::from_str(&r.body).expect("请求体是 JSON");
                let method = v["method"].as_str().unwrap_or_default().to_string();
                let gid = v["params"].as_array().and_then(|p| p.last()).and_then(|x| x.as_str());
                (method, gid.unwrap_or_default().to_string())
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("aria2.forcePause", "a"),
            ("aria2.unpause", "a"),
            ("aria2.forcePause", "b"),
            ("aria2.unpause", "b"),
        ]
        .iter()
        .map(|(m, g)| (m.to_string(), g.to_string()))
        .collect();
        assert_eq!(seq, want, "逐个 gid、先强制停后拉起，且不发全局动作");
    }

    /// 暂停生效、拉起没回话 ⇒ 欠账（这正是客户会被永久停住的那一种）。
    /// 判别力：把 `relaunch` 里 `NoAnswer` 的那条 unpause 改成不欠账 ⇒ 红。
    #[test]
    fn a_paused_gid_whose_resume_gets_no_answer_is_owed() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g"]))
            } else if req.body.contains("\"aria2.unpause\"") {
                RawResponse::new(200).body("不是 JSON")
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        assert_eq!(pause_then_resume_each(&d, &["g".to_string()], &|| false, true), vec!["g".to_string()]);
    }

    /// 动作前已经不在 `tellActive` 里的 gid（客户在名单采样之后单独暂停了它）一条 RPC 都不发，也不欠账。
    /// 判别力：去掉与 `tellActive` 的交集（对所有名单 gid 照做）⇒ `b` 被停启 ⇒ 红。
    #[test]
    fn a_gid_the_client_paused_since_sampling_is_not_touched() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["a"]))
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        let debts = pause_then_resume_each(&d, &["a".to_string(), "b".to_string()], &|| false, true);

        assert!(debts.is_empty());
        assert!(
            srv.requests().iter().all(|r| !r.body.contains("\"b\"")),
            "b 已被客户暂停，不许碰：{:?}",
            srv.requests().iter().map(|r| r.body.clone()).collect::<Vec<_>>()
        );
    }

    /// 某个 gid 的停与起都没回话 ⇒ 立即收手：后面的 gid 一个都不碰（它们只会同样各赔一次超时）。
    /// 判别力：删掉 `Relaunch::Silent` 分支里的 `break` ⇒ `g2` 被动了 ⇒ 红。
    #[test]
    fn a_silent_gid_stops_the_batch_at_once() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g1", "g2"]))
            } else if req.body.contains("\"g1\"") {
                RawResponse::new(200).body("不是 JSON")
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        let debts = pause_then_resume_each(&d, &["g1".to_string(), "g2".to_string()], &|| false, true);

        assert_eq!(debts, vec!["g1".to_string()], "g1 停与起都没回话 ⇒ 欠账");
        assert!(
            srv.requests().iter().all(|r| !r.body.contains("\"g2\"")),
            "g1 静默之后 g2 不许再动"
        );
    }

    /// 停成功、拉起没回话 ⇒ 只欠账，**不**收手：引擎答得了 pause，后面的 gid 照常处理。
    /// 判别力：把 `Relaunch::Owed` 也当成 `Silent` 收手 ⇒ `g2` 没被动 ⇒ 红。
    #[test]
    fn a_gid_that_stopped_but_could_not_resume_does_not_stop_the_batch() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g1", "g2"]))
            } else if req.body.contains("\"g1\"") && req.body.contains("\"aria2.unpause\"") {
                RawResponse::new(200).body("不是 JSON")
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        let debts = pause_then_resume_each(&d, &["g1".to_string(), "g2".to_string()], &|| false, true);

        assert_eq!(debts, vec!["g1".to_string()]);
        assert!(
            srv.requests().iter().any(|r| r.body.contains("\"g2\"") && r.body.contains("\"aria2.unpause\"")),
            "g1 只是欠账，g2 照常停启"
        );
    }

    /// 客户按过「暂停全部」时档 2 一条 RPC 都不发、也不欠账。
    /// 判别力：删掉 `pause_then_resume_each` 里的 `if held` ⇒ 发出了 RPC ⇒ 红。
    #[test]
    fn a_held_pause_all_sends_no_rpc_for_rung_two() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        assert!(pause_then_resume_each(&d, &["g".to_string()], &|| true, true).is_empty());
        assert!(srv.requests().is_empty(), "一条 RPC 都不发");
    }

    /// 本拍探活没拿到应答 ⇒ 档 2 的批量动作整批跳过，一条 RPC 都不发（它们注定失败，而且会白占掉这一拍）。
    /// 与 `hold` 是两道不同的门：这里守的是 `answered`，那两条守的是 `held`，失效方式不同，不合并。
    /// 判别力：删掉 `pause_then_resume_each` 里的 `if !answered` ⇒ 发出了 RPC ⇒ 红。
    #[test]
    fn a_rung_two_batch_is_skipped_when_the_probe_answered_nothing() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        assert!(pause_then_resume_each(&d, &["g".to_string()], &|| false, false).is_empty());
        assert!(srv.requests().is_empty(), "一条 RPC 都不发");
    }

    /// 整批跑到一半时客户按下「暂停全部」：g1 的停启做完（unpause 已经发出）之后 hold 才变真，之后的 gid 一个都不碰。
    /// 触发用"g1 的 unpause 已发出"而不是读取次数：`relaunch` 在发 unpause 前也会读一次 hold，
    /// 按次数触发会把"何时按下"悄悄改掉。
    /// 判别力：删掉逐 gid 前的 `if held()` ⇒ g2 被 forcePause ⇒ 红。
    #[test]
    fn a_hold_set_mid_batch_stops_the_rest() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["g1", "g2"]))
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let held = || {
            srv.requests()
                .iter()
                .any(|r| r.body.contains("\"g1\"") && r.body.contains("\"aria2.unpause\""))
        };

        let debts = pause_then_resume_each(&d, &["g1".to_string(), "g2".to_string()], &held, true);

        assert!(debts.is_empty(), "g1 正常停启，不欠账");
        assert!(
            srv.requests().iter().any(|r| r.body.contains("\"g1\"") && r.body.contains("\"aria2.unpause\"")),
            "g1 在按下之前已经处理完"
        );
        assert!(
            srv.requests().iter().all(|r| !r.body.contains("\"g2\"")),
            "按下之后 g2 不许再被动：{:?}",
            srv.requests().iter().map(|r| r.body.clone()).collect::<Vec<_>>()
        );
    }

    /// `ladder_pause_each` 那一行的事件名与字段顺序是**契约**（读日志的人按位找，
    /// 规格 §9.1 靠它对账"为什么这一档没动手"）。字段记反了没有任何东西会红，
    /// 所以这里捕获出口、按名按位钉住。
    /// 判别力：把 `("held", …)` 删掉或换名 ⇒ 红。
    #[test]
    fn logging_pause_each_reports_the_event_name_and_the_four_fields_in_order() {
        let seen: std::cell::RefCell<Vec<(String, Vec<(String, String)>)>> =
            std::cell::RefCell::new(Vec::new());
        log_pause_each(
            &|event, fields| {
                seen.borrow_mut().push((
                    event.to_string(),
                    fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
                ));
            },
            0,
            true,
            false,
            3,
        );

        let got = seen.into_inner();
        assert_eq!(got.len(), 1, "只该写一条");
        assert_eq!(got[0].0, "ladder_pause_each", "事件名是读日志的人的入口，不许改");
        let want: Vec<(String, String)> = vec![
            ("n".to_string(), "0".to_string()),
            ("held".to_string(), "true".to_string()),
            ("answered".to_string(), "false".to_string()),
            ("owed".to_string(), "3".to_string()),
        ];
        assert_eq!(got[0].1, want, "四个字段、按名按位，顺序是契约");
    }

    /// `tellActive` 失败 ⇒ 整批一条 pause/unpause 都不发，也不欠账（不许拿未经核实的名单下手）。
    /// 判别力：把失败分支改成"不做交集、照名单全做" ⇒ 发出了 pause ⇒ 红。
    #[test]
    fn a_failed_active_sends_no_pause_and_owes_nothing() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(RPC_ERR)
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());

        let debts = pause_then_resume_each(&d, &["g".to_string()], &|| false, true);

        assert!(debts.is_empty(), "不欠账");
        assert!(
            srv.requests().iter().all(|r| r.body.contains("\"aria2.tellActive\"")),
            "只许有 tellActive 那一条：{:?}",
            srv.requests().iter().map(|r| r.body.clone()).collect::<Vec<_>>()
        );
    }

    /// 客户按过「暂停全部」时档 1 一条 RPC 都不发、也不欠账（即使确有停滞任务）。
    /// 判别力：删掉 `reconnect_most_stalled` 里的 `if held` ⇒ 发出了 RPC ⇒ 红。
    #[test]
    fn a_held_pause_all_sends_no_rpc_for_rung_one() {
        let srv = StubHttp::start_with(|_| RawResponse::new(200).body(RPC_OK));
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let (mut tp, t, t1) = one_stalled_task("g");
        let mut owed = BTreeSet::new();

        assert_eq!(reconnect_most_stalled(&d, &mut tp, &t, t1, &mut owed, &|| true), None);
        assert!(srv.requests().is_empty(), "一条 RPC 都不发");
        assert!(owed.is_empty());
    }

    /// 重连之后轮换：两个都卡住时，第一次动的是停滞最久的那个，第二次必须轮到另一个。
    /// 判别力：删掉 `reconnect_most_stalled` 里的 `mark_attempted` ⇒ 第二次仍是 `a` ⇒ 红。
    /// `tellActive` 要在场（动作前的活跃表复查），桩里得回一个含两个 gid 的任务列表。
    #[test]
    fn a_reconnect_hands_the_next_turn_to_the_other_stuck_task() {
        let srv = StubHttp::start_with(|req| {
            if req.body.contains("\"aria2.tellActive\"") {
                RawResponse::new(200).body(active_reply(&["a", "b"]))
            } else {
                RawResponse::new(200).body(RPC_OK)
            }
        });
        let d = crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new());
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        let both = [mk_task("a", TaskState::Active, 100), mk_task("b", TaskState::Active, 100)];
        tp.observe(t0, &active_tasks(&both[..1]));
        tp.observe(t0 + Duration::from_secs(1), &active_tasks(&both));
        let t1 = t0 + Duration::from_secs(200);
        tp.observe(t1, &active_tasks(&both));
        let mut owed = BTreeSet::new();
        assert_eq!(reconnect_most_stalled(&d, &mut tp, &t, t1, &mut owed, &|| false), Some("a".to_string()));
        let t2 = t1 + Duration::from_secs(60);
        tp.observe(t2, &active_tasks(&both));
        assert_eq!(reconnect_most_stalled(&d, &mut tp, &t, t2, &mut owed, &|| false), Some("b".to_string()), "a 刚被动过，轮到 b");
    }

    /// 全局速率大于 0（另一个任务照跑）时，停滞的那一个仍要被认出来，阶梯据此要求档 1。
    #[test]
    fn a_stuck_task_is_named_while_the_others_run() {
        let t = Thresholds::default();
        let mut tp = TaskProgress::default();
        let t0 = Instant::now();
        tp.observe(
            t0,
            &active_tasks(&[mk_task("moving", TaskState::Active, 100), mk_task("stuck", TaskState::Active, 100)]),
        );
        let t1 = t0 + Duration::from_secs(181);
        tp.observe(
            t1,
            &active_tasks(&[mk_task("moving", TaskState::Active, 900), mk_task("stuck", TaskState::Active, 100)]),
        );
        assert_eq!(tp.stalled_count(t1, &t), 1);
        assert_eq!(tp.most_stalled(t1, &t), Some("stuck".to_string()));
        let mut l = Ladder::default();
        assert_eq!(l.next(t1, &Liveness::default(), tp.stalled_count(t1, &t), true, false), Rung::ReconnectOne);
    }

    /// **换上的是另一个引擎、且断开标志被清掉**（"自愈真的能自愈"的那一层）。
    ///
    /// 为什么需要它：阈值与判据的单元测试都是纯函数，绿不代表**真的有一个新进程起来并把句柄换过去**——那是 `replace_engine`
    /// 的活，只有起真引擎才验得到。这条用例真起两个 aria2。
    ///
    /// 判据（规格 W1 的"闭集"）：预检那三句也在表里——空间不够是最常见的
    /// 开工失败，英文客户不该看到一句中文。同一个条件、两种语言、同一个码。
    #[test]
    fn the_preflight_sentence_follows_the_kernels_language() {
        let dir = TempDir::new();
        let huge = i64::MAX; // 任何盘都装不下 ⇒ 必走"空间不足"那一支
        let zh = preflight(crate::i18n::Lang::Zh, dir.path(), huge).expect_err("应当判空间不足");
        let en = preflight(crate::i18n::Lang::En, dir.path(), huge).expect_err("应当判空间不足");
        assert_eq!(zh.code, kcodes::PREFLIGHT_FAILED);
        assert_eq!(en.code, zh.code, "码不跟语言变");
        assert!(zh.message.contains("磁盘剩余空间不足"), "{}", zh.message);
        assert!(
            !en.message.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "英文界面里蹦出了中文: {}",
            en.message
        );
        assert!(en.message.contains("Not enough free space"), "{}", en.message);
        // 两个数字都要落在句子里（客户据此判断换哪个盘）
        assert!(en.message.contains(&huge.to_string()), "{}", en.message);
    }

    /// 判据（规格 W4.3）：解析过程**逐条追加**给壳，且完成后不会无限增长。
    #[test]
    fn the_resolution_lines_are_appended_and_bounded() {
        let dir = TempDir::new();
        let k = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        {
            let mut kk = lock(&k);
            assert_eq!(op_delivery_progress(&mut kk).unwrap()["state"], json!("idle"));
        }
        remember_resolution(
            &k,
            Some(crate::delivery_race::Origin::International),
            "hit_international",
            vec![
                "正在解析交付码…（同时查国内与国际 CDN）".to_string(),
                "已从国际 CDN 找到".to_string(),
            ],
        );
        let v = op_delivery_progress(&mut lock(&k)).unwrap();
        assert_eq!(v["state"], json!("hit_international"));
        assert_eq!(v["origin"], json!("international"));
        assert_eq!(v["lines"].as_array().unwrap().len(), 2);
        // 连同一个域名命中两次，界面不该出现两行一样的句子
        remember_resolution(
            &k,
            Some(crate::delivery_race::Origin::International),
            "hit_international",
            vec!["已从国际 CDN 找到".to_string()],
        );
        let v = op_delivery_progress(&mut lock(&k)).unwrap();
        assert_eq!(v["lines"].as_array().unwrap().len(), 2, "连续重复不追加: {v}");
    }

    /// 判据（规格 W3.3）：预算用尽时**只在那时**说话——置断开、记这一类的错误、
    /// 并且第二次用尽只加计数（界面不会重复弹卡片）。
    #[test]
    fn giving_up_speaks_once_and_that_class_reaches_the_page() {
        let dir = TempDir::new();
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));
        k.language = crate::i18n::Lang::Zh;

        note_auto_restart_gave_up(&mut k);
        assert!(k.engine_disconnected, "用尽之后界面要能说“没有响应”");
        let v = op_error_recent(&mut k).unwrap();
        let e = &v["entries"][0];
        assert_eq!(e["class"], json!("engine_unresponsive"));
        assert!(e["explain"].as_str().unwrap().contains("自动重启"));

        note_auto_restart_gave_up(&mut k);
        let v = op_error_recent(&mut k).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1, "同类只占一条: {v}");
        assert_eq!(v["entries"][0]["count"], json!(2));

        let p = op_restart_progress(&mut k).unwrap();
        assert_eq!(p["budget"], json!(3));
        assert_eq!(p["state"], json!("idle"), "没在重启时不该说正在重启: {p}");
    }

    /// 判据（规格 W2 / I-1）：`error_recent` 给的是**内核语言的**三句话 + 次数，
    /// 而且同类只占一条（不会因为连错两次就出现两张卡片）。
    #[test]
    fn error_recent_counts_repeats_and_speaks_the_kernels_language() {
        let dir = TempDir::new();
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));
        k.language = crate::i18n::Lang::Zh;
        // 用**真实时钟**：`error_recent` 自己取 `now_ms()`，假的 1_000 会立刻过期
        let now = now_ms();
        for _ in 0..2 {
            let mut e = ErrorBody::new(kcodes::PREFLIGHT_FAILED, "磁盘剩余空间不足");
            classify_error(&mut k, &mut e, "enqueue", now);
            assert_eq!(e.class.as_deref(), Some("disk_full"));
        }
        let v = op_error_recent(&mut k).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "同类只占一条: {v}");
        assert_eq!(entries[0]["count"], json!(2));
        assert_eq!(entries[0]["repeats"], json!(1));
        assert_eq!(entries[0]["title"], json!("目标磁盘空间不足或不可写"));
        assert!(entries[0]["action"].as_str().unwrap().contains("换目录"));

        // 内部错误不进这张表（不面向客户）
        let mut e = ErrorBody::new(kcodes::UNKNOWN_METHOD, "内核不认识方法");
        classify_error(&mut k, &mut e, "nope", now);
        let v = op_error_recent(&mut k).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1, "内部错误不该出现: {v}");
    }

    /// 判据（规格 §2.3）：分类的**唯一入口**。有类的填线上字符串，
    /// 内部错误保持空 `class`——客户界面据此决定"给一句人话"还是"意料之外"。
    #[test]
    fn the_funnel_fills_the_class_and_leaves_internal_codes_empty() {
        let dir = TempDir::new();
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));

        let mut e = ErrorBody::new(kcodes::PREFLIGHT_FAILED, "磁盘剩余空间不足");
        classify_error(&mut k, &mut e, "enqueue", 1_000);
        assert_eq!(e.class.as_deref(), Some("disk_full"), "预检不过 = 盘的问题");

        let mut e = ErrorBody::new(kcodes::ENGINE_DISCONNECTED, "下载引擎已断开");
        classify_error(&mut k, &mut e, "transfer_list", 2_000);
        assert_eq!(e.class.as_deref(), Some("engine_unresponsive"));

        let mut e = ErrorBody::new(kcodes::UNKNOWN_METHOD, "内核不认识方法");
        classify_error(&mut k, &mut e, "nope", 3_000);
        assert_eq!(e.class, None, "内部错误不许被塞进某一类");
    }

    /// 判据（规格 W1）：同一件事的两种说法——中文界面不该看到英文，英文界面
    /// **一个字都不能是中文**。两条文案共用同一个 `code`，只有话不一样。
    #[test]
    fn engine_errors_speak_the_language_the_kernel_is_set_to() {
        let zh = engine_disconnected_error(crate::i18n::Lang::Zh, "RPC 超时");
        let en = engine_disconnected_error(crate::i18n::Lang::En, "RPC timed out");
        assert_eq!(zh.code, en.code, "码是判据、话是呈现，码不许跟着语言变");
        assert!(zh.message.contains("已断开") && zh.message.contains("RPC 超时"), "{}", zh.message);
        assert!(
            !en.message.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "英文界面里蹦出了中文: {}",
            en.message
        );
        assert!(en.message.contains("RPC timed out"), "{}", en.message);
        assert!(en.message.to_lowercase().contains("retry"), "要给客户一条出路: {}", en.message);

        // 「慢」那条同样：英文文案里不许出现"已断开"这种判死的话术
        let slow = engine_rpc_slow_error(crate::i18n::Lang::En, "tellActive", "timeout");
        assert!(!slow.message.to_lowercase().contains("disconnected"), "{}", slow.message);
    }

    /// 判据（规格 §2.2）：设置页改语言**立刻**生效——落盘、换内核里的语言、下一次
    /// 报错就换一种说法，不用重启客户端。
    #[test]
    fn changing_the_language_in_settings_takes_effect_at_once() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let path = dir.join("s.json");
        let mut k = Kernel::new(dir.join("dl"), path.clone());
        assert_eq!(k.language, crate::i18n::Lang::En, "默认跟随系统；空 locale 落在英文");

        let params = serde_json::json!({"settings": {
            "parallel": 5, "connections": 8, "splits": 8,
            "min_split_size": "20M", "limit_mbps": 0,
            "max_tries": 1, "retry_wait": 1,
            "language": "zh"
        }});
        op_set_settings(&mut k, &params).expect("保存设置失败");
        assert_eq!(k.language, crate::i18n::Lang::Zh, "内存里要立刻换");
        assert_eq!(
            settings::load_from(&path).language,
            crate::i18n::Lang::Zh,
            "落盘也要是新的（下次启动还认这个选择）"
        );
    }

    /// 判据（规格 §2.1）：握手把语言**落进内核**并**回给壳**，两端不各算一次。
    ///
    /// 三行各钉一件事：壳说了算 > 存过的设置 > 系统 locale；且 `"system"` 永远
    /// 不会被当结果回出去（壳拿它去查文案会落空）。
    #[test]

    /// 判据（规格 W1 首启）：`language_chosen` 只认**设置文件里有没有那一格**，
    /// 不认"值是不是 system"——主动选了「跟随系统」也算选过，不该再弹一次。
    #[test]
    fn the_first_run_flag_follows_the_file_not_the_value() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let settings_path = dir.join("s.json");
        let hello_flag = |kernel: &Arc<Mutex<Kernel>>| {
            let req = protocol::Request {
                id: 1,
                method: "hello".to_string(),
                params: serde_json::json!({"protocol": protocol::PROTOCOL_VERSION}),
            };
            let (resp, _) = dispatch(kernel, &req);
            let v = serde_json::to_value(&resp).unwrap();
            v["result"]["language_chosen"].as_bool().unwrap_or(false)
        };

        // ① 没有设置文件 = 首次运行
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            settings_path.clone(),
        )));
        assert!(!hello_flag(&kernel), "首次运行必须说「还没选过」");

        // ② 客户选了「跟随系统」——**键在文件里**，就算选过了
        let mut saved = settings::default_settings();
        saved.language = crate::i18n::Lang::System;
        saved.save_to(&settings_path).expect("写设置失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(
            dir.join("dl"),
            settings_path.clone(),
        )));
        assert!(hello_flag(&kernel), "选了跟随系统也是选过了");

        // ③ 手写的文件里没有这一格 ⇒ 还是"没选过"（旧版本留下的设置）
        std::fs::write(
            &settings_path,
            br#"{"parallel":5,"connections":8,"splits":8,"min_split_size":"20M","limit_mbps":0,"max_tries":1,"retry_wait":1}"#,
        )
        .expect("写设置失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), settings_path)));
        assert!(!hello_flag(&kernel), "缺这一格 ⇒ 还要问一次");
    }

    fn hello_settles_the_language_once_for_both_ends() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let settings_path = dir.join("s.json");
        // 客户上次选的是英文
        let mut saved = settings::default_settings();
        saved.language = crate::i18n::Lang::En;
        saved.save_to(&settings_path).expect("写设置失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), settings_path)));

        let hello_with = |lang: Option<&str>| {
            let mut params = serde_json::json!({"protocol": protocol::PROTOCOL_VERSION});
            if let Some(l) = lang {
                params["language"] = serde_json::json!(l);
            }
            let req = protocol::Request {
                id: 1,
                method: "hello".to_string(),
                params,
            };
            let (resp, _) = dispatch(&kernel, &req);
            let v = serde_json::to_value(&resp).unwrap();
            (
                v["result"]["language"].as_str().map(str::to_string),
                lock(&kernel).language,
            )
        };

        // ① 壳没说话 ⇒ 用存过的设置（英文）
        let (echoed, stored) = hello_with(None);
        assert_eq!(echoed.as_deref(), Some("en"));
        assert_eq!(stored, crate::i18n::Lang::En);

        // ② 壳说中文 ⇒ 压过设置，且内核里也换了（后续报错才跟着说中文）
        let (echoed, stored) = hello_with(Some("zh"));
        assert_eq!(echoed.as_deref(), Some("zh"));
        assert_eq!(stored, crate::i18n::Lang::Zh);

        // ③ `system` 是输入值，不是产出值
        let (echoed, stored) = hello_with(Some("system"));
        assert_ne!(echoed.as_deref(), Some("system"), "回给壳的必须是解析后的语言");
        assert_ne!(stored, crate::i18n::Lang::System);
    }

    /// 首启语言窗选完就**不许再弹**：这次真机事故（选什么都在弹，只能杀进程）的根因。
    ///
    /// 判据是「设置文件里有没有 `language` 键」，而 `set_settings` 落盘之后那份文件
    /// 一定带这一格 ⇒ 内核的 `language_chosen` 必须同时翻成 true。漏了它，壳重问 `hello`
    /// 拿到的还是 `false`，窗口就再弹回来 —— 这正是客户看到的样子。
    #[test]
    fn choosing_a_language_marks_the_first_run_as_done() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let settings_path = dir.join("s.json");
        // 全新安装：设置文件根本还不存在
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), settings_path.clone())));

        let hello_now = || {
            let req = protocol::Request {
                id: 1,
                method: "hello".to_string(),
                params: serde_json::json!({
                    "protocol": protocol::PROTOCOL_VERSION,
                    "system_locale": "zh-Hans-CN",
                }),
            };
            let (resp, _) = dispatch(&kernel, &req);
            let v = serde_json::to_value(&resp).unwrap();
            (
                v["result"]["language_chosen"].as_bool(),
                v["result"]["language"].as_str().map(str::to_string),
            )
        };

        // ① 没选过 ⇒ 壳据此弹首启窗
        assert_eq!(hello_now().0, Some(false), "全新安装必须先问一次语言");

        // ② 客户在首启窗里选了英文（整份设置回传，与设置页保存同一个形状）
        let mut s = settings::default_settings();
        s.language = crate::i18n::Lang::En;
        let req = protocol::Request {
            id: 2,
            method: "set_settings".to_string(),
            params: serde_json::json!({"settings": s}),
        };
        let (resp, _) = dispatch(&kernel, &req);
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "保存语言不该失败：{v}");

        // ③ 再握手 ⇒ 已经选过、语言是英文（首启窗不会再弹）
        let (chosen, lang) = hello_now();
        assert_eq!(chosen, Some(true), "选过之后 flag 必须翻，否则首启窗会一直弹");
        assert_eq!(lang.as_deref(), Some("en"));
    }

    /// 设置页把语言切回「跟随系统」时，解析要用**壳交过的系统语言**（握手记下的那份），
    /// 而不是环境变量：图形壳里 `LANG` 是空的，读它等于把中文系统又判成英文。
    #[test]
    fn switching_back_to_follow_system_uses_the_locale_the_shell_reported() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let settings_path = dir.join("s.json");
        let mut initial = settings::default_settings();
        initial.language = crate::i18n::Lang::En;
        initial.save_to(&settings_path).expect("写设置失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), settings_path)));

        // 壳说系统是中文
        let req = protocol::Request {
            id: 1,
            method: "hello".to_string(),
            params: serde_json::json!({
                "protocol": protocol::PROTOCOL_VERSION,
                "system_locale": "zh-Hans-CN",
            }),
        };
        let (resp, _) = dispatch(&kernel, &req);
        assert_eq!(
            serde_json::to_value(&resp).unwrap()["result"]["language"],
            serde_json::json!("en"),
            "客户显式选了英文，就先说英文"
        );

        // 客户把设置里的语言切回「跟随系统」
        let mut s = settings::default_settings();
        s.language = crate::i18n::Lang::System;
        let req = protocol::Request {
            id: 2,
            method: "set_settings".to_string(),
            params: serde_json::json!({"settings": s}),
        };
        let _ = dispatch(&kernel, &req);

        assert_eq!(
            lock(&kernel).language,
            crate::i18n::Lang::Zh,
            "「跟随系统」必须按壳交的 zh-Hans-CN 解析，不能读空的环境变量"
        );
    }

    /// 壳把**系统界面语言**交回来时，"跟随系统"必须照着它解析，而不是照着环境变量。
    ///
    /// 这条是 0.3.0 的真机反馈（客户在中文系统上首启拿到英文界面）钉下来的：
    /// 图形壳里 `LANG` 通常是空的，空的按英文兜底 ⇒ 中文系统的默认语言就错了。
    #[test]
    fn hello_uses_the_system_locale_the_shell_reports() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let settings_path = dir.join("s.json");
        // 客户从没选过语言 ⇒ 设置里是 `system`
        settings::default_settings()
            .save_to(&settings_path)
            .expect("写设置失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), settings_path)));

        let hello_with_locale = |locale: &str| {
            let params = serde_json::json!({
                "protocol": protocol::PROTOCOL_VERSION,
                "system_locale": locale,
            });
            let req = protocol::Request { id: 1, method: "hello".to_string(), params };
            let (resp, _) = dispatch(&kernel, &req);
            let v = serde_json::to_value(&resp).unwrap();
            v["result"]["language"].as_str().map(str::to_string)
        };

        // ① 中文系统 ⇒ 中文界面（环境变量在这台机器上是英文或空也不影响）
        assert_eq!(hello_with_locale("zh-Hans-CN").as_deref(), Some("zh"));
        // ② 英文系统 ⇒ 英文界面
        assert_eq!(hello_with_locale("en-US").as_deref(), Some("en"));
        // ③ 认不出的语言 ⇒ 英文（与 `detect` 同一条规矩）
        assert_eq!(hello_with_locale("fr-FR").as_deref(), Some("en"));
    }

    /// 三个断言各自钉一件事：
    ///   · 新引擎 `ping` 得通 ⇒ 换上来的是**活的**，不是又一份死句柄；
    ///   · `rpc_url` 与旧的不同 ⇒ 确实是**另一个**进程，不是把旧句柄原地还回来；
    ///   · `engine_disconnected` 被清 ⇒ 壳那边下一次读不会再收到「已断开」。
    #[test]
    fn replacing_a_wedged_engine_swaps_in_a_live_one() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));

        let old_url = {
            let mut k = lock(&kernel);
            k.ensure_engine()
                .expect("首次启动引擎失败")
                .rpc_url()
                .to_string()
        };
        // 装作自愈已经探了一整轮都没探通。
        {
            let mut k = lock(&kernel);
            k.set_engine_disconnected(true, "test");
        }

        let fresh = replace_engine(crate::i18n::Lang::Zh, &kernel).expect("换引擎失败");
        let ping_ok = fresh.ping().is_ok();
        let new_url = fresh.rpc_url().to_string();
        let (installed, disconnected) = {
            let k = lock(&kernel);
            (
                k.daemon.as_ref().map(|d| d.rpc_url().to_string()),
                k.engine_disconnected,
            )
        };
        // 先收尸再断言：断言失败 panic 时也不会把新引擎留成孤儿。
        let _ = fresh.close();

        assert!(ping_ok, "换上的新引擎应当能应答");
        assert_ne!(new_url, old_url, "换上去的必须是另一个引擎（端口不同）");
        assert_eq!(
            installed.as_deref(),
            Some(new_url.as_str()),
            "内核手里必须拿着新引擎，不是旧的那一份"
        );
        assert!(!disconnected, "换成功之后不该还是断开态");
    }

    // -----------------------------------------------------------------------
    // 「想下的文件集」/ 补回队列 / 暂停全部 / 一键开始（规格 2026-10-12）
    // -----------------------------------------------------------------------

    /// 一个只 `listen` 不 `accept` 的源：连得上、永远不回话 ⇒ 任务停在 Active。
    struct StuckHost(std::net::TcpListener);

    impl StuckHost {
        fn start() -> Self {
            Self(std::net::TcpListener::bind("127.0.0.1:0").expect("绑端口失败"))
        }
        fn base(&self) -> String {
            format!("http://127.0.0.1:{}", self.0.local_addr().expect("端口").port())
        }
    }

    /// 内核 + 一份两个文件的清单（`crc64` 非空 ⇒ `plan` 不发 HEAD）。
    fn kernel_with_two_files(dir: &TempDir, host: &StuckHost) -> Arc<Mutex<Kernel>> {
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        install_manifest(&kernel, host);
        kernel
    }

    fn install_manifest(kernel: &Arc<Mutex<Kernel>>, host: &StuckHost) {
        let mut k = lock(kernel);
        let m = delivery::parse(
            format!(
                r#"{{"code":"AbC","base_url":"{}","files":[
                    {{"path":"t/a.bin","size":6,"crc64":"91"}},
                    {{"path":"t/b.bin","size":6,"crc64":"92"}}]}}"#,
                host.base()
            )
            .as_bytes(),
        )
        .expect("清单应当解析成功");
        k.state.bind(&m.code);
        if k.intent.bind(&m.code) {
            k.save_intent();
        }
        k.manifest = Some(m);
    }

    /// 引擎里现在有哪些清单路径（含暂停的）。
    fn engine_paths(kernel: &Arc<Mutex<Kernel>>) -> BTreeSet<String> {
        let d = lock(kernel).daemon.clone().expect("引擎应当已经起来");
        d.snapshot().expect("快照").path_map.into_values().collect()
    }

    fn close_engine(kernel: &Arc<Mutex<Kernel>>) {
        if let Some(d) = lock(kernel).take_daemon() {
            let _ = d.close();
        }
    }

    fn paths(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 🔴 **引擎被换掉之后，队列必须自己回来**——这是「后台稳定持续运行」的承重判据。
    ///
    /// 判别力：删掉 `replace_engine` 里的 `restore_queue_after_replace(kernel)` ⇒ 新引擎是空的 ⇒ 红。
    #[test]
    fn replacing_the_engine_restores_the_queue() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);

        op_enqueue(&kernel, &json!({"paths": ["t/a.bin", "t/b.bin"]})).expect("入队");
        assert_eq!(engine_paths(&kernel), paths(&["t/a.bin", "t/b.bin"]));

        replace_engine(crate::i18n::Lang::Zh, &kernel).expect("换引擎");
        let after = engine_paths(&kernel);
        close_engine(&kernel);

        assert_eq!(after, paths(&["t/a.bin", "t/b.bin"]), "新引擎里应当补回了全部未完成的");
    }

    /// 用户亲手移除的任务，换引擎之后**不该**被补回来（否则「移除」就成了一句空话）。
    /// 判别力：去掉 `remove` 分支里的 `intent.remove` ⇒ 红。
    #[test]
    fn a_task_the_user_removed_is_not_restored() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);

        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin", "t/b.bin"]})).expect("入队");
        let gid_a = out["added"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["path"] == "t/a.bin")
            .map(|x| x["gid"].as_str().unwrap().to_string())
            .expect("a 应当在 added 里");
        with_kernel(&kernel, |k| {
            op_task_action(k, &json!({"action": "remove", "gid": gid_a}))
        })
        .expect("移除");

        replace_engine(crate::i18n::Lang::Zh, &kernel).expect("换引擎");
        let after = engine_paths(&kernel);
        close_engine(&kernel);

        assert_eq!(after, paths(&["t/b.bin"]), "被移除的 a 不该回来");
    }

    /// 「想下的文件集」落盘：重启内核（改下载目录就是这样）之后，一键开始能把队列补回来。
    /// 判别力：不落盘 / 重启后不读 ⇒ 第二个内核的 `intent` 为空 ⇒ 红。
    #[test]
    fn the_wanted_set_survives_a_kernel_restart_and_resume_all_restores_it() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let first = kernel_with_two_files(&dir, &host);
        op_enqueue(&first, &json!({"paths": ["t/a.bin", "t/b.bin"]})).expect("入队");
        close_engine(&first);
        drop(first);

        // 同一个设置目录、全新的内核（= 重启）；壳重放批次 = 装上同一份清单。
        std::fs::create_dir_all(dir.join("dl2")).unwrap();
        let second = Arc::new(Mutex::new(Kernel::new(dir.join("dl2"), dir.join("s.json"))));
        install_manifest(&second, &host);

        let out = op_resume_all(&second).expect("一键开始");
        let after = engine_paths(&second);
        close_engine(&second);

        assert_eq!(out["added"].as_array().map(Vec::len), Some(2), "{out}");
        assert_eq!(after, paths(&["t/a.bin", "t/b.bin"]));
    }

    /// 暂停全部 → 全部离开 active；一键开始 → 暂停的回来，**引擎里缺的**也补上。
    /// 判别力：`pause_all` 空转 ⇒ 第一条红；`resume_all` 不 unpause ⇒ 第二条红；不补缺 ⇒ 第三条红。
    #[test]
    fn pause_all_then_resume_all_unpauses_and_fills_the_gaps() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin", "t/b.bin"]})).expect("入队");
        let gid_b = out["added"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["path"] == "t/b.bin")
            .map(|x| x["gid"].as_str().unwrap().to_string())
            .unwrap();

        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("暂停全部");
        let d = lock(&kernel).daemon.clone().unwrap();
        let wait = |want_active: bool| {
            let t0 = std::time::Instant::now();
            loop {
                let snap = d.snapshot().expect("快照");
                let active = snap
                    .tasks
                    .iter()
                    .filter(|t| t.state == crate::engine::status::TaskState::Active)
                    .count();
                if (active > 0) == want_active {
                    return;
                }
                assert!(t0.elapsed() < Duration::from_secs(15), "等不到 active>0 == {want_active}");
                thread::sleep(Duration::from_millis(100));
            }
        };
        wait(false);

        // 造一个缺口：b 被绕过 intent 直接从引擎里弄掉（模拟换引擎后丢了）。
        d.remove(&gid_b).expect("移除 b");
        assert_eq!(engine_paths(&kernel), paths(&["t/a.bin"]));

        let out = op_resume_all(&kernel).expect("一键开始");
        wait(true);
        let after = engine_paths(&kernel);
        close_engine(&kernel);

        assert_eq!(after, paths(&["t/a.bin", "t/b.bin"]), "缺的 b 必须补上");
        assert_eq!(out["added"].as_array().map(Vec::len), Some(1), "只补缺的一个: {out}");
    }

    /// 没有任何事可做时，两个动作都是成功的空操作，且**不启动 aria2**。
    #[test]
    fn pause_all_and_resume_all_with_nothing_to_do_do_not_start_the_engine() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);

        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("空暂停");
        let out = op_resume_all(&kernel).expect("空开始");

        assert_eq!(out["added"].as_array().map(Vec::len), Some(0), "{out}");
        assert!(lock(&kernel).daemon.is_none(), "没有可做的事就不该起 aria2");
    }

    /// 完成的文件不该被补回队列（`complete` 里的从「想下的」里扣掉）。
    ///
    /// 直接调 `restore_batch` + `submit_restore`：`op_resume_all` 会先 `ensure_complete` 重算，
    /// 而重算认的是盘上的真文件 + 状态记录，在这里造那两样不如直接钉过滤这一步。
    /// 判别力：去掉 `unfinished` 里的过滤 ⇒ a 也被加进引擎 ⇒ 红。
    #[test]
    fn restoring_the_queue_skips_completed_files() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        {
            let mut k = lock(&kernel);
            k.intent.add("t/a.bin");
            k.intent.add("t/b.bin");
            k.complete.insert("t/a.bin".to_string());
        }
        let d = lock(&kernel).ensure_engine().expect("起引擎");
        let batch = restore_batch(&kernel, &d, false).expect("取数").expect("有要补的");
        let added = submit_restore(&kernel, &d, batch).added;
        let after = engine_paths(&kernel);
        close_engine(&kernel);

        assert_eq!(added.len(), 1, "只该补 b: {added:?}");
        assert_eq!(after, paths(&["t/b.bin"]));
    }

    /// 端到端（内核层）：搬运把旧目录里清单内的文件搬进当前目录，进度可轮询，之后 `complete` 过期。
    /// 判别力：不起线程 / 不设 `finished` ⇒ 轮询等不到 ⇒ 红；搬了清单外的文件 ⇒ 第二条断言红。
    #[test]
    fn migrate_start_moves_manifest_files_and_reports_progress() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let old = dir.join("old");
        std::fs::create_dir_all(old.join("t")).unwrap();
        std::fs::write(old.join("t/a.bin"), b"partial").unwrap();
        std::fs::write(old.join("t/a.bin.aria2"), b"ctl").unwrap();
        std::fs::write(old.join("t/not-in-manifest.bin"), b"x").unwrap();

        op_migrate_start(&kernel, &json!({"from": old.to_str().unwrap()})).expect("启动搬运");
        let t0 = std::time::Instant::now();
        let status = loop {
            let s = with_kernel(&kernel, op_migrate_status).expect("状态");
            if s["finished"] == json!(true) {
                break s;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "搬运迟迟不结束: {s}");
            thread::sleep(Duration::from_millis(20));
        };

        assert_eq!(status["files_total"], json!(1), "{status}");
        assert_eq!(status["errors"].as_array().map(Vec::len), Some(0), "{status}");
        assert_eq!(std::fs::read(dir.join("dl/t/a.bin")).unwrap(), b"partial");
        assert_eq!(std::fs::read(dir.join("dl/t/a.bin.aria2")).unwrap(), b"ctl");
        assert!(old.join("t/not-in-manifest.bin").exists(), "清单外的文件不许碰");
        assert!(lock(&kernel).complete_dirty, "搬完之后 complete 必须过期");
    }

    #[test]
    fn get_download_dir_reports_the_directory_the_kernel_uses() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let v = with_kernel(&kernel, op_get_download_dir).expect("应当成功");
        assert_eq!(v["dir"], json!(dir.join("dl").to_string_lossy()));
    }

    #[test]
    fn migrate_start_refuses_the_same_directory_and_a_missing_one() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);

        let same = op_migrate_start(&kernel, &json!({"from": dir.join("dl").to_str().unwrap()}))
            .expect_err("同一个目录必须拒绝");
        assert!(same.message.contains("同一个"), "{}", same.message);

        let missing = op_migrate_start(&kernel, &json!({"from": dir.join("nope").to_str().unwrap()}))
            .expect_err("不存在的目录必须拒绝");
        assert!(missing.message.contains("不存在"), "{}", missing.message);

        assert!(op_migrate_start(&kernel, &json!({})).is_err(), "缺 from 参数");
    }

    /// 搬运期间「一键开始」必须被拦下（引擎与搬运同写一个文件是数据损坏级风险）。
    /// 判别力：去掉 `op_resume_all` 开头那段检查 ⇒ 红。
    #[test]
    fn resume_all_is_refused_while_a_migration_is_running() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        {
            let k = lock(&kernel);
            let mut m = k.migrate.lock().unwrap();
            m.running = true;
            m.files_done = 2;
            m.files_total = 5;
        }
        let e = op_resume_all(&kernel).expect_err("搬运中必须拒绝");
        assert!(e.message.contains("2/5"), "{}", e.message);
        assert!(lock(&kernel).daemon.is_none(), "拒绝时不该启动引擎");
    }

    fn wait_for_error_state(kernel: &Arc<Mutex<Kernel>>) {
        let d = lock(kernel).daemon.clone().expect("引擎应当已起");
        let t0 = std::time::Instant::now();
        loop {
            let s = d.snapshot().expect("快照");
            if s.tasks.iter().any(|t| t.state == crate::engine::status::TaskState::Error) {
                return;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "等不到任务落到 Error");
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// 把清单换成只有 `t/a.bin`、且指向一个**没人监听**的地址（连过去立刻 Connection refused）。
    fn point_manifest_at_a_closed_port(kernel: &Arc<Mutex<Kernel>>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let m = delivery::parse(
            format!(
                r#"{{"code":"AbC","base_url":"http://127.0.0.1:{port}","files":[
                    {{"path":"t/a.bin","size":6,"crc64":"91"}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        lock(kernel).manifest = Some(m);
    }

    /// 🔴 服务器突然 `Connection refused`：任务落到 Error 之后，自动重试把它重新入队。
    /// 判别力：删掉 `auto_retry_tick` 里的 `d.add` ⇒ 返回 0、gid 不变 ⇒ 红。
    #[test]
    fn a_transiently_failed_task_is_requeued_automatically() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        point_manifest_at_a_closed_port(&kernel);
        op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        wait_for_error_state(&kernel);
        let d = lock(&kernel).daemon.clone().unwrap();
        let before: BTreeSet<String> = d.snapshot().unwrap().path_map.into_keys().collect();

        let mut tracker = autoretry::Tracker::default();
        let n = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 0.0);
        let after: BTreeSet<String> = d.snapshot().unwrap().path_map.into_keys().collect();
        close_engine(&kernel);

        assert_eq!(n, 1, "应当重新入队 1 个");
        assert_eq!(tracker.attempts("t/a.bin"), 1);
        assert_ne!(before, after, "旧 gid 被换成了新的");
    }

    /// 「暂停全部」按住期间，自动重试不得把失败的任务悄悄拉起来。
    /// 判别力：去掉 `hold_auto_retry` 的判断 ⇒ 返回 1 ⇒ 红。
    #[test]
    fn auto_retry_is_held_while_everything_is_paused() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        point_manifest_at_a_closed_port(&kernel);
        op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        wait_for_error_state(&kernel);
        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("暂停全部");

        let mut tracker = autoretry::Tracker::default();
        let held = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 0.0);
        lock(&kernel).hold_auto_retry = false; // 一键开始 ⇒ 放开
        let released = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 0.0);
        close_engine(&kernel);

        assert_eq!(held, 0, "暂停全部期间不得自动重试");
        assert_eq!(released, 1);
    }

    /// `failing` 为空是常态：`auto_retry_tick` 不许在没有要重试的文件时还发 DNS。
    /// 每 5 秒一拍的无界网络 I/O，解析慢时 tick 自己被阻塞。
    /// 判别力：把惰性解析改回循环外无条件 `resolve_ips` ⇒ lookup 被调 ⇒ 红。
    #[test]
    fn auto_retry_tick_does_not_resolve_dns_when_there_is_nothing_to_retry() {
        static LOOKUP_HIT: AtomicBool = AtomicBool::new(false);
        fn counting_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            LOOKUP_HIT.store(true, Ordering::SeqCst);
            vec!["1.2.3.4".parse().unwrap()]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(counting_lookup));
        // 桩引擎：tell* 回空表、getGlobalStat 回正常读数 ⇒ snapshot() 成功、任务表为空。
        // 不这样答的话 snapshot() 直接 Err，函数提前 return 0，根本走不到 DNS 行。
        let srv = StubHttp::start_with(|req| {
            let result = if req.body.contains("aria2.getGlobalStat") {
                json!({"downloadSpeed": "0", "numActive": "0", "numWaiting": "0", "numStopped": "0"})
            } else {
                json!([]) // tellActive / tellWaiting / tellStopped
            };
            RawResponse::new(200).body(json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string())
        });
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        {
            let mut k = lock(&kernel);
            let m = delivery::parse(
                format!(
                    r#"{{"code":"AbC","base_url":"http://download.benagen.com","files":[
                        {{"path":"t/a.bin","size":6,"crc64":"91"}}]}}"#
                )
                .as_bytes(),
            )
            .expect("清单应当解析成功");
            k.state.bind(&m.code);
            if k.intent.bind(&m.code) {
                k.save_intent();
            }
            k.manifest = Some(m);
            k.daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
                &srv.base(),
                "s",
                BTreeMap::new(),
            )));
        }
        // 不入队、不制造失败任务 ⇒ `failing` 为空。
        let mut tracker = autoretry::Tracker::default();
        let n = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 1.0);
        let hit = LOOKUP_HIT.load(Ordering::SeqCst);
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);
        assert_eq!(n, 0);
        assert!(
            !hit,
            "没有要重试的文件时不得发 DNS（每 5 秒一次的无界网络 I/O，慢解析会阻塞 tick 自己）"
        );
    }

    /// 🔴 大批量入队：同步只提交前 100 个并返回 `pending`，其余由后台线程陆续加入，最终一个不少。
    /// 判别力：去掉 `split_for_feeding`（全部同步）⇒ `pending` 为 0 ⇒ 红；
    /// 后台线程不推进 ⇒ 等不到全部入队 ⇒ 红。
    #[test]
    fn a_big_enqueue_returns_after_the_first_hundred_and_feeds_the_rest() {
        const N: usize = 260;
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let files: Vec<String> = (0..N)
            .map(|i| format!(r#"{{"path":"d/f{i}.bin","size":6,"crc64":"9{i}"}}"#))
            .collect();
        lock(&kernel).manifest = Some(
            delivery::parse(
                format!(
                    r#"{{"code":"AbC","base_url":"{}","files":[{}]}}"#,
                    host.base(),
                    files.join(",")
                )
                .as_bytes(),
            )
            .unwrap(),
        );

        let out = op_enqueue(&kernel, &json!({"paths": ["d"]})).expect("入队");
        assert_eq!(out["added"].as_array().map(Vec::len), Some(FIRST_BATCH), "{out}");
        assert_eq!(out["pending"], json!(N - FIRST_BATCH));

        let t0 = std::time::Instant::now();
        let mut added_later = 0usize;
        loop {
            let st = with_kernel(&kernel, op_enqueue_status).expect("状态");
            added_later += st["added"].as_u64().unwrap() as usize;
            if st["pending"] == json!(0) && FIRST_BATCH + added_later == N {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(60), "后台入队迟迟不完: {st}");
            thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(engine_paths(&kernel).len(), N);
        close_engine(&kernel);
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let t0 = std::time::Instant::now();
        while !ok() {
            assert!(t0.elapsed() < Duration::from_secs(15), "等不到：{what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn raw_status(d: &Daemon, gid: &str) -> Option<String> {
        d.snapshot().ok()?.raw_status.get(gid).cloned()
    }

    /// 清单里放 `n` 个文件（`d/f{i}.bin`），用于大批量入队的测试。
    fn big_manifest(host: &StuckHost, n: usize) -> delivery::Manifest {
        let files: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"path":"d/f{i}.bin","size":6,"crc64":"9{i}"}}"#))
            .collect();
        delivery::parse(
            format!(
                r#"{{"code":"AbC","base_url":"{}","files":[{}]}}"#,
                host.base(),
                files.join(",")
            )
            .as_bytes(),
        )
        .expect("清单应当解析成功")
    }

    /// 一个应答正常、但拒绝每一次 `addUri` 的引擎桩：探活成功，于是失败只归于"这一个文件"。
    ///
    /// 每个回包都带 `Connection: close`：桩一次只服务一条连接，而 `addUri` 走的池化连接若一直
    /// 开着，探活（`ping` 另开新连接）就会排在它后面超时——那会把"拒绝一个文件"误判成"引擎断开"。
    fn refusing_engine() -> StubHttp {
        StubHttp::start_with(|req| {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            let reply = if body["method"].as_str() == Some("aria2.addUri") {
                json!({"jsonrpc": "2.0", "id": "1", "error": {"code": 1, "message": "拒绝这个地址"}})
            } else {
                json!({"jsonrpc": "2.0", "id": "1", "result": {}})
            };
            RawResponse::new(200)
                .header("Connection", "close")
                .body(reply.to_string())
        })
    }

    /// 🔴 换码时，后台队列里还没提交的旧批次文件必须作废：不得被新批次的 `base_url` 拼出去下载。
    /// 判别力：删掉 `op_load_delivery` 里的 `discard_feed_queue` ⇒ 队列原样留着 ⇒ 红。
    #[test]
    fn a_code_change_discards_the_previous_batch_still_waiting_to_be_fed() {
        const CODE_B: &str = "BbBbBbBbBbBbBbBbBbBb";
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        lock(&kernel).ensure_engine().expect("起引擎");
        let old = delivery::parse(
            format!(
                r#"{{"code":"AbC","base_url":"{}","files":[
                    {{"path":"t/q1.bin","size":6,"crc64":"91"}},
                    {{"path":"t/q2.bin","size":6,"crc64":"92"}}]}}"#,
                host.base()
            )
            .as_bytes(),
        )
        .expect("清单应当解析成功")
        .files;
        {
            let mut k = lock(&kernel);
            k.hold_auto_retry = true;
            for f in old {
                k.intent.add(&f.path);
                k.feed_queue.push_back(f);
            }
        }
        ensure_feeder(&kernel);

        let srv = manifest_server(CODE_B, &host.base(), r#"{"path":"b/x.bin","size":6,"crc64":"81"}"#);
        let r = op_load_delivery(&kernel, &json!({"code": CODE_B, "base_url": srv.base()}));
        assert!(r.is_ok(), "换码应当成功: {:?}", r.err());
        let left = lock(&kernel).feed_queue.len();
        assert_eq!(left, 0, "换码后旧批次的待加入项必须清空，实际仍有 {left} 个");

        lock(&kernel).hold_auto_retry = false;
        wait_for("后台线程收尾", || {
            let k = lock(&kernel);
            k.feed_queue.is_empty() && !k.feeder_running
        });
        let after = engine_paths(&kernel);
        close_engine(&kernel);
        assert!(
            after.iter().all(|p| !p.starts_with("t/q")),
            "旧批次的文件不得被加入新批次的引擎: {after:?}"
        );
    }

    /// 清单请求的回包固定为 `code` + 一个文件，用来触发 `load_delivery`。
    fn manifest_server(code: &str, base: &str, file: &str) -> StubHttp {
        let body = format!(r#"{{"code":"{code}","base_url":"{base}","files":[{file}]}}"#);
        StubHttp::start_with(move |_req| RawResponse::new(200).body(body.clone()))
    }

    /// 🔴 `enqueue` 的 `addUri` 在锁外做：RPC 在飞时内核锁必须是自由的（契约 §5.2，本批第 2 条）。
    /// 判别力：把 `submit_off_lock` 里的 `d.add` 挪进 `lock(kernel)` 的作用域 ⇒ 探针红。
    #[test]
    fn enqueue_does_not_hold_the_kernel_lock_while_add_uri_is_in_flight() {
        let (seen_tx, seen_rx) = mpsc::channel::<()>();
        let release = Arc::new(AtomicBool::new(false));
        let release_h = Arc::clone(&release);
        let first = AtomicBool::new(true);
        let srv = StubHttp::start_with(move |req| {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            if body["method"].as_str() == Some("aria2.addUri") && first.swap(false, Ordering::SeqCst) {
                let _ = seen_tx.send(());
                while !release_h.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            RawResponse::new(200).body(json!({"jsonrpc": "2.0", "id": "1", "result": "gid-1"}).to_string())
        });

        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        lock(&kernel).daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::new(),
        )));

        let k2 = Arc::clone(&kernel);
        let worker = thread::spawn(move || op_enqueue(&k2, &json!({"paths": ["t/a.bin"]})));
        seen_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("addUri 没有发出去");
        let free_while_in_flight = kernel_lock_is_free(&kernel);
        release.store(true, Ordering::SeqCst);
        let out = worker.join().expect("工作线程 panic");

        assert!(
            free_while_in_flight,
            "addUri 进行中时仍持有内核锁：一批 100 个约 14 分钟会卡住窗口。把 `d.add` 留在锁外。"
        );
        assert!(out.is_ok(), "入队应当成功: {:?}", out.err());
    }

    /// 从 addUri 请求体取出 `(uris, options)`。下标承重：线上形态是 `[token, uris, options]`。
    fn add_uri_uris_and_opts(raw_body: &str) -> (Vec<String>, Option<serde_json::Map<String, Value>>) {
        let body: Value = serde_json::from_str(raw_body).expect("请求体必须是 JSON");
        let params = body["params"].as_array().cloned().unwrap_or_default();
        let uris: Vec<String> = params
            .get(1)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let opts = params.get(2).and_then(|v| v.as_object()).cloned();
        (uris, opts)
    }

    /// 建一份**域名 base_url** 的单文件内核 + 记录 addUri 的桩引擎。
    /// 不能用 `kernel_with_two_files`：它的 base 是 `127.0.0.1`（IP 字面量），
    /// `resolve_for_base` 规则 3 会直接返回空，测不到多 IP 链路。
    fn kernel_with_domain_base(dir: &TempDir, base_url: &str) -> (Arc<Mutex<Kernel>>, StubHttp) {
        let srv = StubHttp::start_with(|_req| {
            RawResponse::new(200).body(json!({"jsonrpc": "2.0", "id": "1", "result": "gid-1"}).to_string())
        });
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        {
            let mut k = lock(&kernel);
            let m = delivery::parse(
                format!(
                    r#"{{"code":"AbC","base_url":"{base_url}","files":[
                        {{"path":"t/a.bin","size":6,"crc64":"91"}}]}}"#
                )
                .as_bytes(),
            )
            .expect("清单应当解析成功");
            k.state.bind(&m.code);
            if k.intent.bind(&m.code) {
                k.save_intent();
            }
            k.manifest = Some(m);
            k.daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
                &srv.base(),
                "s",
                BTreeMap::new(),
            )));
        }
        (kernel, srv)
    }

    /// 多 IP 一次交付：stub resolver 给 2 个 IP ⇒ addUri 的 uris 是 3 个、域名第一，
    /// options 带 header=Host: <域名>。判别力：去掉 Host 注入 ⇒ options 无 header ⇒ 红。
    #[test]
    fn enqueue_sends_domain_first_and_ip_backups_with_host_header() {
        fn stub_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            vec!["1.2.3.4".parse().unwrap(), "5.6.7.8".parse().unwrap()]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(stub_lookup));
        let dir = TempDir::new();
        let (kernel, srv) = kernel_with_domain_base(&dir, "http://download.benagen.com");

        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        assert_eq!(out["added"].as_array().map(Vec::len), Some(1), "一个文件应当入队: {out}");

        let add_uri = srv
            .requests()
            .into_iter()
            .find(|r| {
                let b: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
                b["method"].as_str() == Some("aria2.addUri")
            })
            .expect("应当有 addUri 请求");
        let (uris, opts) = add_uri_uris_and_opts(&add_uri.body);
        assert_eq!(uris.len(), 3, "域名 + 2 个 IP 备胎 = 3 条 URI，实际: {uris:?}");
        assert!(uris[0].starts_with("http://download.benagen.com/"), "域名必须打头: {}", uris[0]);
        assert!(uris[1].starts_with("http://1.2.3.4/"), "备胎 1: {}", uris[1]);
        assert!(uris[2].starts_with("http://5.6.7.8/"), "备胎 2: {}", uris[2]);
        let opts = opts.expect("params[2] 必须是选项对象");
        assert_eq!(
            opts.get("header").and_then(|v| v.as_str()),
            Some("Host: download.benagen.com"),
            "有备胎时必须下发 Host 头: {opts:?}"
        );
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);
    }

    /// https 的批次只发域名 URI 且不带 header。判别力：scheme 门去掉 ⇒ uris 变 3 ⇒ 红。
    #[test]
    fn enqueue_https_sends_domain_uri_only() {
        fn stub_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            vec!["1.2.3.4".parse().unwrap(), "5.6.7.8".parse().unwrap()]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(stub_lookup));
        let dir = TempDir::new();
        let (kernel, srv) = kernel_with_domain_base(&dir, "https://download.benagen.com");

        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        assert_eq!(out["added"].as_array().map(Vec::len), Some(1), "一个文件应当入队: {out}");

        let add_uri = srv
            .requests()
            .into_iter()
            .find(|r| {
                let b: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
                b["method"].as_str() == Some("aria2.addUri")
            })
            .expect("应当有 addUri 请求");
        let (uris, opts) = add_uri_uris_and_opts(&add_uri.body);
        assert_eq!(uris.len(), 1, "https 只发域名 URI，实际: {uris:?}");
        assert!(uris[0].starts_with("https://download.benagen.com/"), "域名 URI: {}", uris[0]);
        let opts = opts.expect("params[2] 必须是选项对象");
        assert!(
            opts.get("header").is_none(),
            "https 不该下发 Host 头（机制整体关着）: {opts:?}"
        );
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);
    }

    /// 多 IP 构造要进诊断计数器（§6.1 的诚实替代——只记构造证据，「被切换次数」RPC 取不到，
    /// 边界见 `diag_counters::record_multiip`）。判别力：**去掉 `plan_file` 里的
    /// `record_multiip` 接线点 ⇒ 红**（计数器停在 0）。
    #[test]
    fn enqueue_counts_multiip_construction_in_diag_counters() {
        fn stub_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            vec!["1.2.3.4".parse().unwrap(), "5.6.7.8".parse().unwrap()]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(stub_lookup));
        let dir = TempDir::new();
        let (kernel, _srv) = kernel_with_domain_base(&dir, "http://download.benagen.com");

        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        assert_eq!(out["added"].as_array().map(Vec::len), Some(1), "一个文件应当入队: {out}");

        let s = lock(&kernel).counters.snapshot();
        assert_eq!(s.multiip_tasks, 1, "一个带备胎的任务 ⇒ 记一次");
        assert_eq!(s.multiip_backup_uris, 2, "2 个 IP 备胎 ⇒ 备胎 URI 总数 2");
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);
    }

    /// `origin_of` 取的是「来源」而不是文件 URI：`scheme://authority`，路径/交付码全丢掉。
    ///
    /// 这是解析缓存的键——用文件 URI 当键会让 1 万文件的批次常驻 1 万条，
    /// 且「同拍多任务共用一条域名腿只解析一次」永远不成立。
    ///
    /// 判别力：实现写成「原样返回 uri」⇒ 前两条红；只取 host 不拼 scheme ⇒ 第一条红；
    /// 把 `?`/`#` 也算进 authority ⇒ 第二条红（来源里混进 query 就解析不到域名了）。
    #[test]
    fn origin_of_keeps_scheme_and_authority() {
        assert_eq!(
            origin_of("http://download.benagen.com/AbC123/data/f.bin"),
            Some("http://download.benagen.com".to_string()),
            "普通 host：路径与交付码必须被丢掉，只留 scheme://host"
        );
        assert_eq!(
            origin_of("https://download.benagen.com:8443/AbC123/x.bin?token=1"),
            Some("https://download.benagen.com:8443".to_string()),
            "带端口：端口必须保留；query 不许混进来源"
        );
        assert_eq!(
            origin_of("http://download.benagen.com#frag"),
            Some("http://download.benagen.com".to_string()),
            "没有路径时 fragment 也不许混进来源"
        );
    }

    /// 取不出来源 ⇒ `None`（调用方跳过这个任务），绝不猜一个去解析。
    ///
    /// 判别力：把非 http 也放过去 ⇒ 第一条红；把空 authority 拼成 `http://` ⇒ 后两条红。
    #[test]
    fn origin_of_rejects_non_http_and_missing_authority() {
        for uri in [
            // 非 http(s)：换腿/解析都不支持这些 scheme。
            "ftp://download.benagen.com/x.bin",
            "file:///tmp/x.bin",
            // 没有 `://` 的裸串与残缺形态。
            "download.benagen.com/x.bin",
            "http:/x.bin",
            // 有 `://` 但没有 authority（空 host）。
            "http:///x.bin",
        ] {
            assert_eq!(
                origin_of(uri),
                None,
                "{uri} 取不出来源时必须回 None：猜一个来源去解析会把别人的 IP 造成这条腿"
            );
        }
    }

    /// 粘合：下载中连推 4 拍（暖机 3 拍 + 第 4 拍判决）⇒ 第 4 拍摘掉最慢的 IP 腿、
    /// 补上这次解析出的 IP 腿；前 3 拍一动不动。
    ///
    /// 判别力：删掉 `leg_trim_tick` 里的 `change_uri` ⇒ 一拍都没有 ⇒ 红；把 `del`/`add`
    /// 传反 ⇒ 两条断言对调 ⇒ 红；把判决提到暖机之前 ⇒ 前 3 拍就有动作 ⇒ 红；
    /// 去掉「必须已有 IP 腿」那道门 ⇒ 本用例仍绿（它本来就带 IP 腿），门由下一条用例钉住。
    #[test]
    fn leg_trim_tick_trims_the_slow_leg_and_adds_a_resolved_ip_on_the_fourth_beat() {
        const SLOW: &str = "http://10.9.0.1/C/f.bin";
        const NEW_IP: &str = "http://203.0.113.9/C/f.bin";

        // 桩解析：只回一个 IP，好让「补上的是哪条」可断言（真解析会碰网络，单测不许）。
        fn stub_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            vec!["203.0.113.9".parse().expect("合法 IP")]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(stub_lookup));

        let writes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&writes);
        // 桩引擎：tellActive 回一个活跃任务，getServers 回「2 慢 IP 腿 + 1 快 IP 腿 + 域名腿」，
        // changeUri 把位置参数记下来。
        //
        // 速率是按阈值反推的：4 条腿 [50 KB/s, 200 KB/s, 4 MB/s, 3 MB/s] 的中位数取较小
        // 中间值 = 200 KB/s ⇒ 阈值 = max(66 KB/s, 128 KiB/s) = 128 KiB/s。于是 200 KB/s 虽
        // 仍是全组第二慢，但高于下限 ⇒ **留着**；只有 50 KB/s 那条该摘。写成别的数就会
        // 变成「摘两条」或「一条都不摘」。
        let srv = StubHttp::start_with(move |req| {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            let result = match body["method"].as_str().unwrap_or_default() {
                "aria2.tellActive" => json!([{
                    "gid": "g1",
                    "status": "active",
                    "totalLength": "1000000",
                    "completedLength": "1000",
                    "downloadSpeed": "0",
                    "connections": "4",
                }]),
                "aria2.getServers" => json!([{
                    "index": "1",
                    "servers": [
                        {"uri": "http://10.9.0.1/C/f.bin", "downloadSpeed": "50000"},
                        {"uri": "http://10.9.0.2/C/f.bin", "downloadSpeed": "200000"},
                        {"uri": "http://10.9.0.3/C/f.bin", "downloadSpeed": "4000000"},
                        {"uri": "http://download.example.com/C/f.bin", "downloadSpeed": "3000000"},
                    ],
                }]),
                // 任务的完整 URI 列表（域名腿在这里，不在腿表里也能找到）。
                "aria2.getUris" => json!([
                    {"uri": "http://10.9.0.1/C/f.bin", "status": "used"},
                    {"uri": "http://10.9.0.2/C/f.bin", "status": "used"},
                    {"uri": "http://10.9.0.3/C/f.bin", "status": "used"},
                    {"uri": "http://download.example.com/C/f.bin", "status": "used"},
                ]),
                "aria2.changeUri" => {
                    seen.lock().expect("记录锁").push(body["params"].clone());
                    // 引擎实际删了 1 条、加了 1 条：诊断必须按这个数记。
                    json!([1, 1])
                }
                _ => json!({}),
            };
            RawResponse::new(200)
                .header("Connection", "close")
                .body(json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string())
        });

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        lock(&kernel).daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::new(),
        )));

        let mut states: BTreeMap<String, legtrim::TrimState> = BTreeMap::new();
        let mut resolved: BTreeMap<String, (u64, Vec<std::net::IpAddr>)> = BTreeMap::new();
        for beat in 1..=3 {
            let n = leg_trim_tick(&kernel, &mut states, &mut resolved, beat);
            assert_eq!(n, 0, "第 {beat} 拍还在暖机，不该动作");
        }
        assert!(
            writes.lock().expect("记录锁").is_empty(),
            "暖机期间不得发 changeUri"
        );

        let n = leg_trim_tick(&kernel, &mut states, &mut resolved, 4);
        let got = writes.lock().expect("记录锁").clone();
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);

        assert_eq!(n, 1, "第 4 拍应当提交一次换腿");
        assert_eq!(got.len(), 1, "总共只该发一次 changeUri，且在第 4 拍: {got:?}");
        // 线上参数是 `[token:<secret>, gid, fileIndex, del, add]`（令牌由 RPC 层插在最前），
        // 所以这里从 1 开始取。
        assert_eq!(got[0][1], json!("g1"), "换的是这个 gid");
        assert_eq!(got[0][2], json!(1), "单文件任务恒传 fileIndex=1");
        assert_eq!(
            got[0][3],
            json!([SLOW]),
            "只该摘掉最慢（50 KB/s）那条腿；域名腿与正常腿不动"
        );
        assert_eq!(got[0][4], json!([NEW_IP]), "应当补上新解析出的 IP 腿");

        let s = lock(&kernel).counters.snapshot();
        assert_eq!(s.leg_trim_del, 1, "摘掉的腿总数 = 1");
        assert_eq!(s.leg_add, 1, "补上的腿总数 = 1");
    }

    /// 门槛：**只有域名腿**的任务一拍也不该动——它提交时没带 `Host` 头，加 IP 腿只会 404。
    /// 判别力：删掉 `leg_trim_tick` 里那道「至少一条 IP 字面量腿」的门 ⇒ 第 4 拍会发
    /// `changeUri` 并敲一次 DNS ⇒ 两条断言都红。
    #[test]
    fn leg_trim_tick_leaves_a_domain_only_task_alone() {
        static LOOKED_UP: AtomicBool = AtomicBool::new(false);
        fn stub_lookup(_host: &str, _port: u16) -> Vec<std::net::IpAddr> {
            LOOKED_UP.store(true, Ordering::SeqCst);
            vec!["203.0.113.9".parse().expect("合法 IP")]
        }
        TEST_LOOKUP.with(|t| *t.borrow_mut() = Some(stub_lookup));

        let writes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&writes);
        let srv = StubHttp::start_with(move |req| {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            let result = match body["method"].as_str().unwrap_or_default() {
                "aria2.tellActive" => json!([{
                    "gid": "g1",
                    "status": "active",
                    "totalLength": "1000",
                    "completedLength": "0",
                    "downloadSpeed": "0",
                    "connections": "1",
                }]),
                // 只有域名腿（提交时没带备胎 ⇒ 没有 Host 头）。
                "aria2.getServers" => json!([{
                    "index": "1",
                    "servers": [
                        {"uri": "http://download.example.com/C/f.bin", "downloadSpeed": "1000"},
                    ],
                }]),
                "aria2.getUris" => json!([
                    {"uri": "http://download.example.com/C/f.bin", "status": "used"},
                ]),
                "aria2.changeUri" => {
                    seen.lock().expect("记录锁").push(body["params"].clone());
                    json!([1, 1])
                }
                _ => json!({}),
            };
            RawResponse::new(200)
                .header("Connection", "close")
                .body(json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string())
        });

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        lock(&kernel).daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::new(),
        )));

        let mut states: BTreeMap<String, legtrim::TrimState> = BTreeMap::new();
        let mut resolved: BTreeMap<String, (u64, Vec<std::net::IpAddr>)> = BTreeMap::new();
        let mut actions = 0;
        for beat in 1..=6 {
            actions += leg_trim_tick(&kernel, &mut states, &mut resolved, beat);
        }
        let looked_up = LOOKED_UP.load(Ordering::SeqCst);
        TEST_LOOKUP.with(|t| *t.borrow_mut() = None);

        assert_eq!(actions, 0, "只有域名腿的任务不该被换腿");
        assert!(
            writes.lock().expect("记录锁").is_empty(),
            "不该发 changeUri"
        );
        assert!(
            !looked_up,
            "门槛就该挡在解析之前：没有 IP 腿时连 DNS 都不该敲"
        );
    }

    /// 🔴 替换引擎的窗口期（引擎暂缺、未换码）里，后台队列必须等待，而不是被清空（本批第 3 条）。
    /// 判别力：把 `feed_once` 的"引擎暂缺"分支改回 `clear()` ⇒ 窗口里队列消失 ⇒ 红。
    #[test]
    fn the_feeder_waits_out_an_engine_swap_instead_of_dropping_the_queue() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        {
            let mut k = lock(&kernel);
            let files = k.manifest.as_ref().expect("清单").files.clone();
            for f in files {
                k.intent.add(&f.path);
                k.feed_queue.push_back(f);
            }
        }
        ensure_feeder(&kernel);
        thread::sleep(Duration::from_secs(1));
        {
            let k = lock(&kernel);
            assert_eq!(k.feed_queue.len(), 2, "引擎暂缺时队列不得消失");
            assert!(k.feed_rejected.is_empty(), "引擎暂缺时不得记任何拒绝: {:?}", k.feed_rejected);
            assert!(k.feeder_running, "线程应当还在等");
        }

        lock(&kernel).ensure_engine().expect("起引擎");
        wait_for("引擎就绪后两个都加入", || {
            engine_paths(&kernel) == paths(&["t/a.bin", "t/b.bin"])
        });
        close_engine(&kernel);
    }

    /// 🔴 前 100 个全被拒、后面的还在后台排队：回执**不是错误**，后面的仍会被加入（本批第 6 条）。
    /// 判别力：把 `op_enqueue` 的错误判断挪回后台入队之前，或去掉 `pending == 0` ⇒ 红。
    #[test]
    fn a_big_enqueue_whose_first_hundred_are_refused_is_not_reported_as_failed() {
        const N: usize = 160;
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let srv = refusing_engine();
        {
            let mut k = lock(&kernel);
            k.manifest = Some(big_manifest(&host, N));
            k.daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new())));
        }

        let out = op_enqueue(&kernel, &json!({"paths": ["d"]}))
            .expect("后面还有待加入的，就不是「没有任何文件被加入下载」");
        assert_eq!(out["pending"], json!(N - FIRST_BATCH), "{out}");
        assert_eq!(out["rejected"].as_array().map(Vec::len), Some(FIRST_BATCH), "{out}");

        wait_for("后台队列排空", || {
            let k = lock(&kernel);
            k.feed_queue.is_empty() && !k.feeder_running
        });
        let st = with_kernel(&kernel, op_enqueue_status).expect("状态");
        assert_eq!(st["rejected"].as_array().map(Vec::len), Some(N - FIRST_BATCH), "{st}");
    }

    /// 一个都没加上、也没有待加入的入队，仍然是错误（与上一条配对：错误判断只在"真的一个都没有"时触发）。
    #[test]
    fn an_enqueue_that_every_file_refuses_is_still_an_error() {
        const N: usize = 100;
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let srv = refusing_engine();
        {
            let mut k = lock(&kernel);
            k.manifest = Some(big_manifest(&host, N));
            k.daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(&srv.base(), "s", BTreeMap::new())));
        }

        let e = op_enqueue(&kernel, &json!({"paths": ["d"]})).expect_err("全被拒且无待加入必须是错误");
        assert!(e.message.contains("没有任何文件被加入下载"), "{}", e.message);
    }

    /// 🔴 重试任务**下载中**时，它的次数记账不得被清零（本批第 5 条）。
    /// 判别力：把 `retain_failing` 的参数改回"只留失败态" ⇒ 第二拍后次数为 0 ⇒ 红。
    #[test]
    fn a_retry_that_is_downloading_keeps_its_attempt_count() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        point_manifest_at_a_closed_port(&kernel);
        op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        wait_for_error_state(&kernel);
        // 重试时源要换成一个连得上、却永不回数据的：重试任务就会停在下载中。
        install_manifest(&kernel, &host);

        let mut tracker = autoretry::Tracker::default();
        let first = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 0.0);
        assert_eq!(first, 1, "第一拍应当重新入队");
        let d = lock(&kernel).daemon.clone().expect("引擎应当已起");
        wait_for("重试任务离开 error 态", || {
            let snap = d.snapshot().expect("快照");
            snap.tasks.iter().any(|t| {
                t.state != engine::status::TaskState::Error
                    && snap.path_map.get(&t.gid).is_some_and(|p| p == "t/a.bin")
            })
        });

        let second = auto_retry_tick(&kernel, &mut tracker, std::time::Instant::now(), 0.0);
        close_engine(&kernel);
        assert_eq!(second, 0, "下载中的任务不该再被重试");
        assert_eq!(tracker.attempts("t/a.bin"), 1, "下载中的任务，次数记账不得清零");
    }

    /// 🔴 「暂停全部」之后的入队不解除暂停（本批第 8 条）：新加入的也必须是暂停态。
    /// 判别力：把 `op_enqueue` 里的 `hold_auto_retry = false` 加回去 ⇒ 第一条断言红。
    #[test]
    fn enqueuing_while_everything_is_paused_keeps_the_pause() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("暂停全部");

        let out = op_enqueue(&kernel, &json!({"paths": ["t/b.bin"]})).expect("暂停期间入队");
        let held = lock(&kernel).hold_auto_retry;
        let d = lock(&kernel).daemon.clone().expect("引擎应当已起");
        let gid_b = out["added"][0]["gid"].as_str().expect("gid").to_string();
        wait_for("新加入的 b 是暂停态", || raw_status(&d, &gid_b).as_deref() == Some("paused"));
        close_engine(&kernel);
        assert!(held, "入队不得解除「暂停全部」");
    }

    /// 🔴 「下载全部」时，已经在后台队列里等着的文件不得再被提交一次（本批第 7 条）。
    /// 判别力：去掉 `op_enqueue` 里对 `feed_queue` 的过滤 ⇒ t/b.bin 被 addUri 进引擎 ⇒ 红。
    #[test]
    fn download_all_does_not_resubmit_what_is_already_waiting_to_be_fed() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        {
            let mut k = lock(&kernel);
            k.hold_auto_retry = true;
            let b = k
                .manifest
                .as_ref()
                .expect("清单")
                .files
                .iter()
                .find(|f| f.path == "t/b.bin")
                .cloned()
                .expect("b 在清单里");
            k.intent.add(&b.path);
            k.feed_queue.push_back(b);
        }

        op_enqueue(&kernel, &json!({"paths": []})).expect("下载全部");
        let added = engine_paths(&kernel);
        close_engine(&kernel);
        assert_eq!(added, paths(&["t/a.bin"]), "等待中的 t/b.bin 不得被再提交一次");
    }

    /// 🔴 「下载全部」不为引擎里已有的路径再造一个 GID（裁决：暂停交给「一键开始」）。
    /// 判别力：去掉 `owned` 过滤 ⇒ a 被再次加入，任务数变成 3 ⇒ 红。
    #[test]
    fn download_all_does_not_duplicate_a_task_the_engine_already_has() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let first = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        let gid_a = first["added"][0]["gid"].as_str().expect("gid").to_string();
        let d = lock(&kernel).daemon.clone().expect("引擎应当已起");

        let out = op_enqueue(&kernel, &json!({})).expect("下载全部");
        let added: Vec<&str> = out["added"]
            .as_array()
            .expect("added 是个数组")
            .iter()
            .map(|x| x["path"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(added, vec!["t/b.bin"], "只该补 b，a 已经在引擎里: {out}");
        assert_eq!(d.snapshot().expect("快照").path_map.len(), 2, "同一路径不得出现两个任务");

        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("暂停全部");
        let again = op_enqueue(&kernel, &json!({})).expect("暂停后下载全部");
        let snap = d.snapshot().expect("快照");
        close_engine(&kernel);
        assert_eq!(again["added"].as_array().map(Vec::len), Some(0), "暂停的任务不由「下载全部」恢复: {again}");
        assert_eq!(snap.path_map.len(), 2, "暂停期间不得多出任务");
        assert_eq!(snap.path_map.get(&gid_a).map(String::as_str), Some("t/a.bin"), "原任务保持原 GID");
    }

    /// 🔴 失败态的任务不由「下载全部」静默重提（瞬时的归自动重试，永久的归用户显式重试）。
    /// 判别力：去掉 `owned` 过滤 ⇒ 失败的 a 被再次加入 ⇒ 任务数变成 2 ⇒ 红。
    #[test]
    fn download_all_does_not_resubmit_a_failed_task() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        point_manifest_at_a_closed_port(&kernel);
        op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        wait_for_error_state(&kernel);
        let d = lock(&kernel).daemon.clone().expect("引擎应当已起");

        let out = op_enqueue(&kernel, &json!({})).expect("下载全部");
        let count = d.snapshot().expect("快照").path_map.len();
        close_engine(&kernel);
        assert_eq!(out["added"].as_array().map(Vec::len), Some(0), "失败的任务不得被静默重提: {out}");
        assert_eq!(count, 1, "同一路径不得出现第二个任务");
    }

    /// 判别力：把 `op_resume_all` 里的 `unpause_all` 挪到 `restore_batch` 之前 ⇒ 暂停的任务已被放开 ⇒ 红。
    #[test]
    fn resume_all_refused_for_space_leaves_the_paused_tasks_paused() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let out = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect("入队");
        let gid_a = out["added"][0]["gid"].as_str().expect("gid").to_string();
        with_kernel(&kernel, |k| op_task_action(k, &json!({"action": "pause_all"}))).expect("暂停全部");
        let d = lock(&kernel).daemon.clone().expect("引擎应当已起");
        wait_for("a 已暂停", || raw_status(&d, &gid_a).as_deref() == Some("paused"));

        // 清单里另一个文件大到盘上一定放不下，且它在意向集里 ⇒ 一键开始会因磁盘不足失败。
        let huge = delivery::parse(
            format!(
                r#"{{"code":"AbC","base_url":"{}","files":[
                    {{"path":"t/a.bin","size":6,"crc64":"91"}},
                    {{"path":"t/b.bin","size":1152921504606846976,"crc64":"92"}}]}}"#,
                host.base()
            )
            .as_bytes(),
        )
        .expect("清单应当解析成功");
        {
            let mut k = lock(&kernel);
            k.manifest = Some(huge);
            k.intent.add("t/b.bin");
        }
        let r = op_resume_all(&kernel);
        thread::sleep(Duration::from_millis(800));
        let still = raw_status(&d, &gid_a);
        close_engine(&kernel);
        assert!(r.is_err(), "磁盘放不下，一键开始必须报错");
        assert_eq!(still.as_deref(), Some("paused"), "查不过磁盘就不能先把暂停的放开");
    }

    /// 🔴 搬运在跑的时候，入队与自愈补回都不得碰引擎（本批第 11 条）。
    /// 判别力：删掉 `op_enqueue` 的搬运检查 ⇒ 第一条断言红；删掉 `restore_queue_after_replace` 的搬运检查 ⇒ 第二条断言红。
    #[test]
    fn a_running_migration_keeps_the_engine_untouched() {
        let dir = TempDir::new();
        let host = StuckHost::start();
        let kernel = kernel_with_two_files(&dir, &host);
        let mig = Arc::clone(&lock(&kernel).migrate);
        mig.lock().unwrap_or_else(PoisonError::into_inner).running = true;

        let e = op_enqueue(&kernel, &json!({"paths": ["t/a.bin"]})).expect_err("搬运中入队必须拒绝");
        assert!(e.message.contains("搬运"), "{}", e.message);
        assert!(lock(&kernel).daemon.is_none(), "拒绝之后不得已经起了引擎");

        lock(&kernel).ensure_engine().expect("起引擎");
        {
            let mut k = lock(&kernel);
            k.intent.add("t/a.bin");
            k.intent.add("t/b.bin");
        }
        restore_queue_after_replace(&kernel);
        let after = engine_paths(&kernel);
        close_engine(&kernel);
        assert!(after.is_empty(), "搬运中自愈补回不得往引擎里加任务: {after:?}");
    }

    /// 盘上已有的半截文件要从「还需要的空间」里扣掉，别把「只差一点」误拦成「磁盘不够」。
    #[test]
    fn remaining_bytes_subtracts_what_is_already_on_disk() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("t")).unwrap();
        std::fs::write(dir.join("t/a.bin"), vec![0u8; 4]).unwrap();
        let f = |p: &str, size: i64| delivery::File {
            path: p.to_string(),
            size,
            crc64: String::new(),
            source_mtime: String::new(),
        };
        let files = [f("t/a.bin", 10), f("t/none.bin", 7), f("t/a.bin", 3)];
        // 10-4 + 7-0 + max(3-4,0)=0
        assert_eq!(remaining_bytes(dir.path(), &files), 13);
    }

    /// **引擎中途不应答时，回执必须把每一件没入队的文件都交代清楚**（2026-10-09）。
    ///
    /// 现场那份 0.2.3 日志（9.4 小时）里最刺眼的一格：一次 `enqueue` 已经把 **62 个**
    /// 文件加进了队列（真机上那些确实在下），然后后面某一次 `addUri` 撞上 10 秒超时 ——
    /// 内核判引擎断开，`return Err` **把整份回执连同那 62 个一起扔掉**。
    /// 客户看到「下载引擎已断开」，完全不知道三分之二的批次已经入队。
    ///
    /// 两条判据各自钉一件事：
    ///   · 结果是 **`Ok`**（正常回执），不是 `Err`；
    ///   · **已经加上的仍然在 `added` 里**，而**每一个没入队的文件都在 `rejected` 里**
    ///     —— 一条不许少（约束 4：不得静默少交），且每条都要有理由。
    ///
    /// 为什么用桩引擎：只有**可控地**在"第 K 个文件"之后开始不应答，才能确定性地造出
    /// "部分成功"这一格；真 aria2 死不到那么刚好。
    ///
    /// ⚠️ 探活（`getGlobalStat`）必须**跟 `addUri` 一起挂**：只让 `addUri` 挂的话，
    ///    `on_rpc_failure` 一探就通，根本判不成断开，这一格走不到。
    #[test]
    fn a_mid_batch_disconnect_keeps_the_receipt_and_accounts_for_every_file() {
        const OK_ADDS: usize = 2;
        const N: usize = 5;

        let adds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adds_h = Arc::clone(&adds);
        let srv = StubHttp::start_with(move |req| {
            let body: serde_json::Value =
                serde_json::from_str(&req.body).unwrap_or(serde_json::Value::Null);
            let ok = body["method"].as_str() == Some("aria2.addUri")
                && adds_h.fetch_add(1, Ordering::SeqCst) < OK_ADDS;
            let payload = if ok {
                serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": "gid-x"})
            } else {
                serde_json::json!({"jsonrpc": "2.0", "id": "1",
                                   "error": {"code": 1, "message": "引擎不应答"}})
            };
            RawResponse::new(200).body(payload.to_string())
        });

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).expect("建下载目录失败");
        let kernel = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        let paths: Vec<String> = (0..N).map(|i| format!("t/f{i}.bin")).collect();
        let files: Vec<String> = (0..N)
            .map(|i| format!(r#"{{"path":"t/f{i}.bin","size":6,"crc64":"9{i}"}}"#))
            .collect();
        {
            let mut k = lock(&kernel);
            k.manifest = Some(
                delivery::parse(
                    format!(
                        r#"{{"code":"AbC","base_url":"http://127.0.0.1:1","files":[{}]}}"#,
                        files.join(",")
                    )
                    .as_bytes(),
                )
                .expect("清单应当解析成功"),
            );
            k.daemon = Some(Arc::new(crate::engine::daemon::stub_daemon(
                &srv.base(),
                "s",
                BTreeMap::new(),
            )));
        }

        let out = op_enqueue(&kernel, &json!({ "paths": paths }))
            .expect("引擎在批次中间断开这一格必须回**正常回执**，不是错误");

        let added = out["added"].as_array().expect("added 是个数组");
        let rejected = out["rejected"].as_array().expect("rejected 是个数组");
        assert_eq!(added.len(), OK_ADDS, "已经加上的必须留在回执里：{out}");
        assert_eq!(
            added.len() + rejected.len(),
            N,
            "每一个文件都要有落点（约束 4：不得静默少交）：{out}"
        );
        for r in rejected {
            let reason = r["reason"].as_str().unwrap_or("");
            assert!(!reason.is_empty(), "每条拒绝都必须有理由：{out}");
        }
        assert!(
            lock(&kernel).engine_disconnected,
            "判成断开这件事必须照旧落到全局状态上，否则壳连横幅都不会出"
        );
    }

    /// [`short_cause`] 的措辞表（纯函数）。
    ///
    /// 两个方向都钉：截得出东西时取**第一个冒号之前**那一段；截不出东西时**原样返回**
    /// —— 宁可长，不可空（空理由正是约束 4 明禁的静默失效）。
    #[test]
    fn short_cause_is_a_table() {
        assert_eq!(
            short_cause("下载引擎已断开：RPC 请求失败：http://127.0.0.1:1/jsonrpc: Network Error"),
            "下载引擎已断开"
        );
        // 没有冒号：原样。
        assert_eq!(short_cause("engine gone"), "engine gone");
        // 冒号在开头（前半段为空）：也必须原样，不能交出一个空理由。
        assert_eq!(short_cause("：leading colon"), "：leading colon");
    }

    /// `engine_rpc_failed` 的消息必须带**出错的调用名**。
    ///
    /// 守的是一处**用户可见**的文案回归：这一支曾经写成
    /// `format!("…（探活：{cause}）")`，而进入这一支 ⇔ `ping_ok == true`
    /// ⇔ `ping.err()` 是 `None`，于是 `cause` **恒为空串**，客户看到的消息以
    /// `（探活：）` 结尾。把调用名换回那个空括号，这条就红。
    ///
    /// （这条分支本身在 e2e 里不可达：要"ping 成功但某次调用失败"，
    /// 那是瞬时竞态，构造不出来。所以它只有这一层守护——**这一层是真的**。）
    #[test]
    fn engine_rpc_failed_message_carries_the_operation() {
        let e = engine_rpc_failed_error(crate::i18n::Lang::Zh, "aria2.addUri", "GID x is not found");
        assert_eq!(e.code, kcodes::ENGINE_RPC_FAILED, "{e:?}");
        assert!(
            e.message.contains("aria2.addUri"),
            "消息里必须有出错的调用名（否则客户无从定位）: {}",
            e.message
        );
        assert!(
            e.message.contains("GID x is not found"),
            "RPC 自己的错误串也要带出来: {}",
            e.message
        );
        assert!(
            !e.message.contains("探活：)"),
            "不得再出现空括号（那正是修复前的问题）: {}",
            e.message
        );
    }

    // -----------------------------------------------------------------------
    // 磁盘剩余空间（`preflight` 的那一步）
    // -----------------------------------------------------------------------

    /// `df -k` 的解析：**取第 4 列（Available），按 1024 换算成字节**。
    ///
    /// 判别力（两个方向都会红）：取错列（例如第 2 列的 1024-blocks）会得到另一个数；
    /// 忘了乘 1024 会差 1024 倍。
    #[cfg(unix)]
    #[test]
    fn df_available_column_is_read_in_kib_and_scaled_to_bytes() {
        let mac = "Filesystem   1024-blocks      Used Available Capacity iused      ifree %iused  Mounted on\n\
                   /dev/disk3s5  971350584  505729716 465620868    53%  3410860 4294967295    1%   /System/Volumes/Data\n";
        assert_eq!(
            available_bytes_from_df(mac).unwrap(),
            465_620_868u64 * 1024,
            "macOS 那一行取的是 Available（第 4 列），不是 Used、也不是 1024-blocks"
        );

        let linux = "Filesystem     1K-blocks     Used Available Use% Mounted on\n\
                     /dev/sda1       103080896 51540448  46308352  53% /\n";
        assert_eq!(
            available_bytes_from_df(linux).unwrap(),
            46_308_352u64 * 1024,
            "Linux 那一行同理：第 4 列 × 1024"
        );
    }

    /// 解析不出来的 `df` 输出**必须是一个错误**，不许当成 0、也不许编一个数出来。
    ///
    /// ⚠️ 这条承重，两个方向都有后果：返回 0 会让**每一次** `enqueue` 都被判成
    ///    "磁盘剩余空间不足"（下载彻底用不了）；而"编一个够大的数"会让这条检查
    ///    形同虚设。两种都必须红。
    #[cfg(unix)]
    #[test]
    fn a_malformed_df_output_is_an_error_not_a_number() {
        assert!(available_bytes_from_df("").is_err(), "空输出必须报错");
        assert!(
            available_bytes_from_df("only one line\n").is_err(),
            "没有数据行必须报错"
        );
        assert!(
            available_bytes_from_df("hdr\n/dev/sda1 100 200\n").is_err(),
            "列数不够必须报错"
        );
        assert!(
            available_bytes_from_df("hdr\n/dev/sda1 100 200 abc 53% /\n").is_err(),
            "可用那一列不是数字必须报错（不许当成 0）"
        );
    }

    /// **Windows 上不许依赖任何外部程序**（2026-10-08 那条客户报障的回归判据）。
    ///
    /// 报障原文是「无法读取磁盘剩余空间（df 不可用）：**program not found**」——
    /// 那台机器上没有 `df`，于是**每一次点下载**都在开工前检查这一步失败。
    ///
    /// ⚠️ **本机（macOS）跑不到这一条**，这是**如实记账**、不是"验过了"：
    ///    它只在真 Windows 的测试套件里执行。开发机上能证的只有"Windows 靶编得过"
    ///    （`cargo build --target x86_64-pc-windows-gnu`），以及下面那条源码级判据。
    #[cfg(windows)]
    #[test]
    fn free_bytes_works_without_any_external_program() {
        let dir = std::env::temp_dir();
        let n = free_bytes(&dir).expect("Windows 上必须能取到磁盘剩余空间（不许再依赖 df）");
        assert!(n > 0, "可用空间应当是正数，实际 {n}");
    }

    /// **源码级判据：`free_bytes` 必须有 Windows 那一支，且那一支不许起外部进程。**
    ///
    /// ⚠️ 为什么要有它：上一条（真行为）**只在 Windows 上跑**，而这条缺陷恰恰是
    ///    "在 macOS 上永远看不见"的那一类 —— 所以本机需要一条**够得着源码**的判据。
    ///    `include_str!` 的手法在本仓有先例（`windows/shell-core/src/diagnostics.rs`
    ///    用它钉住两份 logger 的共享常量）。
    ///
    /// 判别力：把 `#[cfg(windows)]` 从 `free_bytes` 上摘掉、或让 Windows 那一支
    /// 重新写上 `Command::new("df")`，这条立刻红。
    #[test]
    fn free_bytes_has_a_windows_arm_that_shells_out_to_nothing() {
        let src = include_str!("kernel.rs");

        // ① 两支都在：Unix 那支起 `df`，Windows 那支调系统 API。
        assert!(
            src.contains("#[cfg(unix)]\nfn free_bytes("),
            "Unix 那一支不见了（`df` 那条路）"
        );
        assert!(
            src.contains("#[cfg(windows)]\nfn free_bytes("),
            "Windows 那一支不见了 —— 那正是 2026-10-08 客户报的「df 不可用」"
        );
        assert!(
            src.contains("GetDiskFreeSpaceExW"),
            "Windows 那一支必须走 `GetDiskFreeSpaceExW`（不新增依赖的系统入口）"
        );

        // ② Windows 那一支里**不许**再出现起外部进程的写法。
        let win_arm = src
            .split("#[cfg(windows)]\nfn free_bytes(")
            .nth(1)
            .expect("Windows 那一支的正文");
        let win_body = &win_arm[..win_arm.find("\n}\n").map_or(win_arm.len(), |i| i + 2)];
        assert!(
            !win_body.contains("Command::new"),
            "Windows 那一支里又出现了起外部进程的写法：{}",
            win_body
        );
        assert!(
            !win_body.contains("\"df\""),
            "Windows 那一支里又出现了 `df`"
        );
    }

    // -----------------------------------------------------------------------
    // crc64 补值必须写回清单（2026-10-08）
    // -----------------------------------------------------------------------

    /// **`plan` 从服务端补到的 crc64 必须写回清单**，否则下一轮会把同一批文件的 HEAD
    /// 全部重发一遍。
    ///
    /// 为什么它值一条判据（客户报的是「文件多了就卡死/未响应」）：
    ///   `complete_dirty` 在**每一批校验结果写回之后**都会被置起 ⇒ 一次大下载里
    ///   「每次勾选 ⇒ `get_tree` ⇒ `ensure_complete` ⇒ 全量 plan ⇒ **每个空 crc64 文件一次
    ///   HEAD**」会**反复发生**，代价 ∝ 文件数。而壳的命令是**同步**的（Tauri 在收 IPC 的
    ///   那条线程上跑完才回，IPC 走主线程）⇒ 直接表现就是窗口「未响应」。
    ///
    /// 判别力（两个方向都会红）：
    ///   · 把 `adopt_filled_crc64` 的调用从 `ensure_complete` 里删掉 ⇒ 第一条红
    ///     （清单还是空的 ⇒ 下一轮重发 HEAD）；
    ///   · 让它把**空的**也写回去（把失败也缓存）⇒ 第二条红 —— 那等于把一次网络抖动
    ///     **钉成永久的「无法校验」**，客户盘上明明有数据却永远过不了校验。
    #[test]
    fn a_filled_crc64_is_written_back_into_the_manifest() {
        let dir = TempDir::new();
        let mut k = Kernel::new(dir.join("dl"), dir.join("settings.json"));
        k.manifest = Some(
            delivery::parse(
                br#"{"code":"AbC","base_url":"http://127.0.0.1:1","files":[
                     {"path":"t/a.bin","size":6,"crc64":""},
                     {"path":"t/b.bin","size":7,"crc64":""},
                     {"path":"t/c.bin","size":8,"crc64":"999"}]}"#,
            )
            .expect("清单应当解析成功"),
        );

        let f = |p: &str, s: i64, c: &str| delivery::File {
            path: p.to_string(),
            size: s,
            crc64: c.to_string(),
            source_mtime: String::new(),
        };
        // 这一轮：a 补到了、b 没补到（HEAD 失败）、c 本来就有。
        let todo = planner::Todo {
            items: vec![
                planner::Item {
                    file: f("t/a.bin", 6, "123"),
                    kind: planner::Kind::Download,
                },
                planner::Item {
                    file: f("t/b.bin", 7, ""),
                    kind: planner::Kind::Download,
                },
                planner::Item {
                    file: f("t/c.bin", 8, "999"),
                    kind: planner::Kind::Skip,
                },
            ],
            unverifiable: vec!["t/b.bin".to_string()],
        };
        adopt_filled_crc64(&mut k, &todo);

        let m = k.manifest.as_ref().expect("清单还在");
        assert_eq!(
            m.files[0].crc64, "123",
            "补到的 crc64 必须写回清单 —— 不写回去，下一轮 plan 会为同一个文件重发一次 HEAD"
        );
        assert_eq!(
            m.files[1].crc64, "",
            "**没补到的**不许缓存：那会把一次网络抖动钉成永久「无法校验」"
        );
        assert_eq!(m.files[2].crc64, "999", "本来就有的不许被覆盖");
    }

    // -----------------------------------------------------------------------
    // 任务 4：三条更新命令（接线）
    // -----------------------------------------------------------------------

    /// 取不到 / 没新版时，`update_status` 回的是**成功**，不是错误。
    ///
    /// ⚠️ 这一条守的是一件客户可见的事：接口挂掉时**不许**在界面上冒出一句
    ///    看不懂的报错。规格 §6.6 点名了它。
    #[test]
    fn update_status_is_success_even_when_there_is_nothing_to_report() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let k = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        let v = with_kernel(&k, op_update_status).expect("必须成功返回，不是错误");
        assert_eq!(v["has_newer"], serde_json::json!(false));
        assert_eq!(v["enabled"], serde_json::json!(true), "默认开");
        assert!(v["current"].as_str().is_some_and(|s| !s.is_empty()));
        // 「从没查过」是**第三档**，不是「没查成」：壳拿它区分"我还没查过"与"查了没成"。
        assert_eq!(
            v["last_check_ok"],
            serde_json::json!(null),
            "还没查过时这一格必须是 null —— 与 Some(false)（查了没查成）合并的话，\
             手动查那颗按钮第一次点之前就会先对客户说一句『没查成』"
        );
    }

    /// 🔴 **手动查那颗按钮的判据**：`last_check_ok` 说得出"没查成"与"没有新版"的区别。
    ///
    /// 这一条是**关键判据**：没有它，断网时那颗按钮只能看着 `has_newer:false` 说
    /// "已经是最新的"——那是谎报（见 `Kernel::update_last_check_ok` 的注释）。
    ///
    /// ⚠️ 判决那一半（[`record_check_result`]）**直接喂结果**，不起网络：本机的测试
    ///    跑不到发布页，也不该跑（网络判据在 `update.rs` 的 stub fetcher 那一档里）。
    #[test]
    fn a_failed_check_is_told_apart_from_one_that_found_nothing_new() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));

        // ① 先查成一次、且看到一个新的版本
        record_check_result(&mut k, Some("99.0.0".to_string()), 100);
        let v = op_update_status(&mut k).unwrap();
        assert_eq!(v["last_check_ok"], serde_json::json!(true), "查成了");
        assert_eq!(v["has_newer"], serde_json::json!(true), "99 比当前版本高");
        assert_eq!(v["checked_at"], serde_json::json!(100));

        // ② 这次没查成（断网）：`last_seen` **不许**被抹，但必须说出来是"没查成"
        record_check_result(&mut k, None, 200);
        let v = op_update_status(&mut k).unwrap();
        assert_eq!(
            v["last_check_ok"],
            serde_json::json!(false),
            "查失败必须记成 false：这一格是壳唯一能区分『没查成』与『没有新版』的地方"
        );
        assert_eq!(
            v["latest"],
            serde_json::json!("99.0.0"),
            "一次网络抖动不许把上次看到的版本抹掉（否则提示会先消失一下）"
        );
        assert_eq!(
            v["checked_at"],
            serde_json::json!(200),
            "失败也要推新时间：不然每 60 秒重查一次发布页"
        );

        // ③ 这次查成了、而且确实没有新版：`Some(true)` + `has_newer:false`
        record_check_result(&mut k, Some("0.0.1".to_string()), 300);
        let v = op_update_status(&mut k).unwrap();
        assert_eq!(v["last_check_ok"], serde_json::json!(true));
        assert_eq!(
            v["has_newer"],
            serde_json::json!(false),
            "这一格为假时『查成没查成』才是关键 —— 两种情形的 has_newer 都是 false"
        );
        assert_eq!(v["checked_at"], serde_json::json!(300));
    }

    /// `last_check_ok` 是**运行状态，不落盘**：换一个 Kernel 读同一个目录 ⇒ 回到 `None`。
    ///
    /// ⚠️ 这一条钉的是**取舍**，不是"顺手"：上一次开机的检查结果与**这一次**点按钮的结果
    ///    没关系，落盘只会让壳读到一份过期结论。
    #[test]
    fn the_check_result_is_not_persisted() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let mut k = Kernel::new(dir.join("dl"), dir.join("s.json"));
        record_check_result(&mut k, Some("0.0.1".to_string()), 100);
        assert_eq!(
            op_update_status(&mut k).unwrap()["last_check_ok"],
            serde_json::json!(true)
        );
        let mut k2 = Kernel::new(dir.join("dl"), dir.join("s.json"));
        assert_eq!(
            op_update_status(&mut k2).unwrap()["last_check_ok"],
            serde_json::json!(null),
            "这一格不许进 update.json"
        );
        // 而同一个目录里**该落盘的那三格**照样读得回来（对照组：不是"什么都没落盘"）
        assert_eq!(
            op_update_status(&mut k2).unwrap()["latest"],
            serde_json::json!("0.0.1"),
            "last_seen 是持久状态"
        );
    }

    /// 开关写下去之后读得回来，并且落到了盘上。
    #[test]
    fn the_switch_round_trips_and_persists() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let k = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        with_kernel(&k, |k| op_update_set_enabled(k, &json!({"enabled": false}))).unwrap();
        let v = with_kernel(&k, op_update_status).unwrap();
        assert_eq!(v["enabled"], serde_json::json!(false));
        // 换一个 Kernel 读同一个目录 ⇒ 读回来还是 false（真的落盘了）
        let k2 = Kernel::new(dir.join("dl"), dir.join("s.json"));
        assert!(!k2.update.enabled, "必须落盘，不然重启就回到默认");
    }

    /// `has_newer` 是**现算的**，不是存的 —— 存的判定会过期，
    /// 而"过期"在这里的表现是"提示一个已经装上的版本"。
    #[test]
    fn has_newer_is_computed_not_stored() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let k = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        {
            let mut kk = lock(&k);
            // 存一个**比当前版本更旧**的 last_seen
            kk.update.last_seen = Some("v0.0.1".to_string());
        }
        let v = with_kernel(&k, op_update_status).unwrap();
        assert_eq!(v["has_newer"], serde_json::json!(false), "存的是旧的就不能提示");
    }

    /// 🔴 **正向**：有新版本 ⇒ `has_newer:true`，**并且给出形状正确的下载链接**。
    ///
    /// ⚠️ 这一条守的是**这个功能存在的理由**那一支。上面三条只钉住"不提示"这一侧；
    ///    少了它，`url` 从响应里掉了、或域名/路径/文件名拼错了，**没有任何东西会红** ——
    ///    而壳会照着那个 `null`/空串画一个「去下载」，客户点下去什么也拿不到。
    #[test]
    fn a_newer_version_yields_the_download_url() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("dl")).unwrap();
        let k = Arc::new(Mutex::new(Kernel::new(dir.join("dl"), dir.join("s.json"))));
        {
            let mut kk = lock(&k);
            // 比当前版本**更高**的 last_seen（当前版本是 `CARGO_PKG_VERSION`）
            kk.update.last_seen = Some("v999.0.0".to_string());
        }
        let v = with_kernel(&k, op_update_status).unwrap();
        assert_eq!(v["has_newer"], serde_json::json!(true), "更高的版本必须提示");
        assert_eq!(v["latest"], serde_json::json!("v999.0.0"));

        let url = v["url"].as_str().expect("有新版本就必须给出链接（不是 null）");
        // 域名与路径**逐字**，版本段是**解析后重新拼回**的三个数（不是 `tag_name` 原文）。
        assert!(
            url.starts_with(
                "https://gitee.com/starsyi/benagen-downloader/releases/download/v999.0.0/"
            ),
            "域名与路径必须逐字正确、版本段必须是重新拼回的：{url}"
        );
        let file = url.rsplit('/').next().unwrap();
        assert_eq!(
            file,
            update::asset_name(update::platform(), update::arch(), (999, 0, 0)),
            "文件名必须按本平台/架构选、且版本段与 URL 同源（拼错会给客户一个装不上的包）：{url}"
        );
    }

    /// 🔴 **入队节奏钉住**（规格 §2/§5）：同步只提交前 100 个、后台每批 20 个、批间 200 ms。
    /// 0.2.5 现场（诊断日志 20261014-081646）`n=686 ms=464` 一次灌完正是**没有**这套节奏的
    /// 形态；它已在 0.2.6（`4d93d08a`）落地——这条防的是「调参不记账」的静默漂移。
    ///
    /// ⚠️ 200 ms 那一格用**源码即判据**钉（`include_str!` 手法同 `free_bytes_has_a_windows_arm_that_shells_out_to_nothing`）：
    /// 它是 `spawn_feeder` 里的一个字面量，不抽常量就没法用行为测便宜地够着。
    /// **这条够不着什么**：它钉的是「200 这个字面量在 `spawn_feeder` 里」，
    /// 不是「线程真的按 200 ms 节奏睡」——后者要真引擎计时（0.2.6 验收的人肉读数），
    /// 单测里的 sleep 耗时断言会把 200 ms 的容差抖成 flake，故不取。
    #[test]
    fn the_enqueue_pacing_constants_are_pinned() {
        assert_eq!(FIRST_BATCH, 100, "同步批次动过就得同步这条与规格 §2");
        assert_eq!(FEED_BATCH, 20, "后台每批个数动过就得同步这条与规格 §2");

        // 批间 200 ms：源码断言（见本条文档的「够不着什么」）。
        let src = include_str!("kernel.rs");
        let feeder = src
            .split("pub fn spawn_feeder(")
            .nth(1)
            .expect("`spawn_feeder` 不见了——批间节奏都没了落点");
        let body = &feeder[..feeder.find("\n}\n").map_or(feeder.len(), |i| i + 3)];
        assert!(
            body.contains("thread::sleep(Duration::from_millis(200))"),
            "`spawn_feeder` 的批间小睡不是 200 ms 了（规格 §2/§5 的第三格）——实际正文片段里找不着 `thread::sleep(Duration::from_millis(200))`"
        );
    }
}
