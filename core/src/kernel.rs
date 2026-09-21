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

use crate::delivery::{Manifest, UreqTransport};
use crate::engine::daemon::{Daemon, DaemonOptions};
use crate::engine::status::Task;
use crate::planner::{Kind, Options as PlanOptions, Planner};
use crate::protocol::{codes, TransferItem};
use crate::settings::Settings;
use crate::state::State as FileState;
use crate::view::FileState as ViewFileState;
use crate::{delivery, engine, kcodes, planner, protocol, settings, state, verify, view};

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
    /// `None` = 还没有批次（或已被换码作废）。
    manifest: Option<Manifest>,
    /// 上次用过的交付码。**单独一个小文件**（简报第 6 条），不进 `settings`。
    last_code: String,

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
}

impl Kernel {
    pub fn new(download_dir: PathBuf, settings_path: PathBuf) -> Kernel {
        let last_code_path = match settings_path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join("last_code"),
            _ => PathBuf::from("last_code"),
        };
        let settings = settings::load_from(&settings_path);
        let last_code = std::fs::read_to_string(&last_code_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let state = FileState::load(&download_dir);
        Kernel {
            download_dir,
            settings_path,
            last_code_path,
            settings,
            manifest: None,
            last_code,
            state,
            verify: verify::CheckResult::default(),
            complete: BTreeSet::new(),
            complete_dirty: true,
            verifying: BTreeSet::new(),
            verified: BTreeSet::new(),
            retried: BTreeSet::new(),
            daemon: None,
            engine_disconnected: false,
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
            let (disconnected, outcome) = reconnect_outcome(ok, &cause);
            self.engine_disconnected = disconnected;
            outcome?;
            return Ok(d);
        }
        let global = self.settings.global_options();
        let opts = DaemonOptions {
            download_dir: self.download_dir.clone(),
            global_opts: global.clone(),
            per_task: self.settings.per_task_options(),
            binary_path: None,
            force_port: None,
        };
        let d = Daemon::start(opts)
            .map_err(|e| ErrorBody::new(kcodes::ENGINE_START_FAILED, format!("启动下载引擎失败：{e}")))?;
        // 冗余但**承重**的一次下发：启动路径上那一次的错误没人看得到（契约 §2.6）。
        d.apply_global(&global).map_err(|e| {
            ErrorBody::new(
                kcodes::ENGINE_START_FAILED,
                format!("引擎已启动，但下发全局选项失败：{e}"),
            )
        })?;
        // 引擎就绪留一行**诊断**（stderr，绝不进协议通道）：引擎出问题时，
        // 这行给出的 RPC 端点就是 `curl` 的入口。secret 不打印。
        eprintln!("benagen-core: 下载引擎已就绪（RPC {}）", d.rpc_url());
        let d = Arc::new(d);
        self.daemon = Some(Arc::clone(&d));
        self.engine_disconnected = false;
        Ok(d)
    }

    /// 读路径上取引擎：已判定断开时**每次读都试一次重连**（设计规格 §9 的"恢复路径"）。
    fn read_engine(&mut self) -> Result<Arc<Daemon>, ErrorBody> {
        match self.daemon.clone() {
            // 已经起来过：复用 `ensure_engine` 的重连逻辑
            Some(_) => self.ensure_engine(),
            None => Err(ErrorBody::new(
                kcodes::ENGINE_NOT_STARTED,
                "下载引擎尚未启动（先 enqueue 才会起引擎）",
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
        let ping = d.ping();
        let ok = ping.is_ok();
        let (disconnected, outcome) = reconnect_outcome(ok, &err);
        self.engine_disconnected = disconnected;
        match outcome {
            // ping 不通 → 判定断开
            Err(b) => b,
            // ping 通了 → 只是**这一次**调用失败（瞬时故障），不判断开，让壳可以重试
            Ok(()) => engine_rpc_failed_error(op, &err),
        }
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
fn engine_rpc_failed_error(op: &str, err: &str) -> ErrorBody {
    ErrorBody::new(
        kcodes::ENGINE_RPC_FAILED,
        format!("下载引擎调用失败（重连探活正常，可重试）：{op}：{err}"),
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
fn reconnect_outcome(ping_ok: bool, cause: &str) -> (bool, Result<(), ErrorBody>) {
    if ping_ok {
        (false, Ok(()))
    } else {
        (
            true,
            Err(ErrorBody::new(
                kcodes::ENGINE_DISCONNECTED,
                format!(
                    "下载引擎已断开：{cause}；已下载的文件保留在目标目录，可直接重试"
                ),
            )),
        )
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
                    let daemon = {
                        let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
                        k.read_engine().ok()
                    };
                    if let Some(d) = daemon {
                        for f in &retry_files {
                            // 落盘路径 = manifest.path 原文（契约 §3.1）
                            let Some(pf) = engine::new_planned_file(&m, f) else {
                                continue;
                            };
                            if let Err(e) = d.add(&pf.url, &pf.dir, &pf.out) {
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
) -> planner::Todo {
    let transport = UreqTransport::new();
    let mut p = Planner {
        dir,
        state: Some(&mut state),
        transport: &transport,
    };
    p.plan(m, PlanOptions { strict })
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
    let todo = plan_off_lock(dir, state, &work.1, false);
    lock(kernel).complete = complete_from(&todo);
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

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ErrorBody> {
    serde_json::to_value(v).map_err(|e| ErrorBody::new(codes::INTERNAL, format!("序列化失败: {e}")))
}

/// 分派一条请求。返回 `(响应, 是否结束)`。
pub fn dispatch(kernel: &Arc<Mutex<Kernel>>, req: &Request) -> (Response, bool) {
    let id = req.id;
    if req.method == "shutdown" {
        return (Response::ok(id, json!({})), true);
    }
    let r: Result<Value, ErrorBody> = match req.method.as_str() {
        "hello" => protocol::hello(&req.params).and_then(|h| to_value(&h)),
        // ⚠️ 下面五个方法**刻意不用 `with_kernel` 包起来**：它们最贵的部分都是网络 I/O
        // （`fetch` 最坏 91.5 秒、`plan` 最坏 30 秒 × N、换码清理最坏约 40 秒），
        // 持着内核锁跑会让校验线程与落地监听器一起停摆分钟级（契约 §5.2）。
        // 它们各自把"锁外取数 / 锁内安装"分开，见各自的注释。
        "load_delivery" => op_load_delivery(kernel, &req.params),
        "list_dir" => op_list_dir(kernel, &req.params),
        "get_tree" => op_get_tree(kernel),
        "plan" => op_plan(kernel, &req.params),
        "enqueue" => op_enqueue(kernel, &req.params),
        "transfer_list" => with_kernel(kernel, op_transfer_list),
        "task_action" => with_kernel(kernel, |k| op_task_action(k, &req.params)),
        "verify_status" => with_kernel(kernel, op_verify_status),
        "get_settings" => with_kernel(kernel, op_get_settings),
        "set_settings" => with_kernel(kernel, |k| op_set_settings(k, &req.params)),
        "get_state" => with_kernel(kernel, op_get_state),
        other => Err(ErrorBody::new(
            kcodes::UNKNOWN_METHOD,
            format!("内核不认识方法 {other:?}"),
        )),
    };
    match r {
        Ok(v) => (Response::ok(id, v), false),
        Err(e) => (Response::err(id, &e.code, &e.message), false),
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
    // ⚠️ **连续失败即短路**。每个 `d.remove(gid)` 自己还要先发一次 `client.list()`
    // ——也就是**正常路径上**每个任务约 2 次 RPC；`RPC_TIMEOUT` 是 10 秒。
    //
    // ⚠️ **两个口径别混**（它们量级相同、成因完全不同，摆在一起容易被读成 600 秒）：
    //   - **正常路径**（引擎健康）：每次移除 2 次 RPC，但都秒回，30 个任务是毫秒级；
    //   - **黑洞路径**（aria2 不响应也不拒连）：**第一条 `d.remove` 里的 `list()` 就会
    //     等满 10 秒超时**，然后进 `Err` 分支、`consecutive` 加一——**每次失败只花 1 次
    //     RPC（就是那次 `list()`），不是 2 次**（失败在 `list()` 上，`remove` 根本没发出去）。
    //     连续 3 次失败即短路，所以黑洞路径的上界是 **10s 快照 + 3×10s ≈ 40 秒**，
    //     与任务数**无关**——30 个任务和 3000 个任务都是这个数。
    //
    // 把"单次 RPC"这个已接受的模式放大成**无界的循环**是量变到质变——
    // 正常情况毫秒级无感，坏情况下是用户会来投诉的形状。所以给它一个上界：
    // **连续 3 次失败**就认为引擎已经不可用，剩下的交给后面的 `clear_finished` 兜。
    // 阈值取 3 而不是 1：孤立的单次失败（某个 GID 恰好不在了）不该让整轮清理放弃，
    // 而连续三次说明是系统性的。
    //
    // ⚠️ **本函数现在跑在 `kernel` 锁外**（I-2）：它是由 `op_load_delivery` 在
    // 锁外 ③ 那一段调的，所以那约 40 秒不再冻结校验线程与落地监听器。
    // 短路保护**照样保留**——读循环仍然是同步的，引擎卡死时客户点"加载交付码"
    // 仍要等这 40 秒才拿到响应。
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
///   - `delivery::fetch`：30 秒 × 3 次 + 1.5 秒退避 = 最坏 **91.5 秒**；
///   - `clear_engine_batch`：10 秒快照 + 3×10 秒短路上限 = 最坏约 **40 秒**；
///   - 全量 `plan`：每个 crc64 为空的文件一次 HEAD，最坏 **30 秒 × N**。
///
/// 于是拆成"锁外取数 / 锁内安装"两个阶段，锁内只剩短写入。
/// 读循环是**同步**的，所以不存在两次 `load_delivery` 交错的语义问题。
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

    // ── 锁外 ②：拉清单（最坏 91.5 秒）────────────────────────────────
    let transport = UreqTransport::new();
    let m = delivery::fetch(&transport, base, &code)
        .map_err(|e| ErrorBody::new(kcodes::DELIVERY_FETCH_FAILED, e))?;

    // ── 锁内 ①（短）：判"码变了没有"，并取旧引擎的句柄 ──────────────────
    let (code_changed, old_daemon) = {
        let k = lock(kernel);
        // ⚠️ **换码即作废，引擎侧也要清干净**（裁定 10 改判为"要修"）。
        // 只在**码真的变了**的时候清：同一个码重复 load（壳刷新树）不该把正在下的任务删掉。
        (
            k.manifest.as_ref().is_some_and(|old| old.code != m.code),
            k.daemon.clone(),
        )
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
    let todo = plan_off_lock(dir, state, &m, false);

    // ── 锁内 ③（短）：装 `complete` 并出树 ────────────────────────────
    let mut k = lock(kernel);
    k.complete = complete_from(&todo);
    let tree = tree_json(&mut k, &m);

    Ok(json!({
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
        "tree": tree,
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
/// **这是 I-2 修复之后仍然留在锁内的最后一段网络 I/O**，所以把上界写准：
/// `snapshot()` = 3 条 `tell*` + 1 条 `getGlobalStat`，而 `list()` **第一次出错就返回**
/// （契约 §4.3），因此它的真实上界是**一次 RPC 超时 ≈ 10 秒**
/// （`RPC_TIMEOUT`；connection refused 时约 0 秒）——不是 4×10 秒。
/// `load_delivery` 里那三段真正的长 I/O（`fetch` 91.5 秒、`plan` N×30 秒、
/// 换码清理约 40 秒）已经全部移到锁外，见各自的注释。
///
/// 之所以 10 秒这一段现在不是 bug：读循环是**同步的**，一次只处理一条请求，
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
pub fn op_get_tree(kernel: &Arc<Mutex<Kernel>>) -> Result<Value, ErrorBody> {
    ensure_complete(kernel); // 锁外（长）；幂等，只有 `complete_dirty` 时才真算

    let mut k = lock(kernel);
    let m = k.require_manifest()?;
    let (path_map, tasks) = current_tasks(&mut k);
    let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
    let prog = view::progress(&m.files, &k.complete, &path_map, &tasks);
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
    let todo = plan_off_lock(dir, state, &m, strict);

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
fn preflight(dir: &Path, need_bytes: i64) -> Result<(), ErrorBody> {
    let bad = |m: String| ErrorBody::new(kcodes::PREFLIGHT_FAILED, m);
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("目标目录不可用（{}）：{e}", dir.display())))?;
    let probe = dir.join(".benagen-write-probe");
    std::fs::write(&probe, b"").map_err(|e| bad(format!("目标目录不可写（{}）：{e}", dir.display())))?;
    let _ = std::fs::remove_file(&probe);

    let free = free_bytes(dir).map_err(bad)?;
    let need = need_bytes.max(0) as u64;
    if free < need {
        return Err(bad(format!(
            "磁盘剩余空间不足：本次需要 {need} 字节，{} 只剩 {free} 字节",
            dir.display()
        )));
    }
    Ok(())
}

/// 目标所在文件系统的剩余字节数。
///
/// ⚠️ **没有用 crate**：`Cargo.toml` 里没有 `libc`/`fs4` 之类，而 std 不提供 statvfs。
/// 本项目的纪律是"不要新增依赖"，所以走 `df -k` 这个到处都在的 POSIX 工具。
/// 代价是一次进程创建——它只在 `enqueue` 的开工前检查里发生，不在热路径上。
fn free_bytes(dir: &Path) -> Result<u64, String> {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(dir)
        .output()
        .map_err(|e| format!("无法读取磁盘剩余空间（df 不可用）：{e}"))?;
    if !out.status.success() {
        return Err("df 读取磁盘剩余空间失败".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
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

    let mut k = lock(kernel);
    let m = k.require_manifest()?;
    let targets = if raw.is_empty() {
        let (path_map, tasks) = current_tasks(&mut k);
        let states = view::compose(&m.files, &k.complete, &path_map, &tasks);
        let pending = view::pending_paths(&m.files, &states);
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

    // ① 开工前检查：**先于**引擎启动
    let need: i64 = targets.iter().map(|f| f.size).sum();
    preflight(&k.download_dir, need)?;

    // ② 起引擎
    let daemon = k.ensure_engine()?;

    // ③ 加任务
    let mut added: Vec<Value> = Vec::new();
    let mut rejected: Vec<Value> = Vec::new();
    for f in &targets {
        // 路径守卫（契约 §3.2）；`dir`/`out` 由 manifest.path 拆出且**不得规范化**（§3.1）
        let Some(pf) = engine::new_planned_file(&m, f) else {
            rejected.push(json!({"path": f.path, "reason": "路径不安全（越界/控制字符/空段）"}));
            continue;
        };
        match daemon.add(&pf.url, &pf.dir, &pf.out) {
            Ok(gid) => {
                // 重下必须重新过一遍校验（旧的结论已经不成立了）——见 `reopen_for_download`。
                reopen_for_download(&mut k, &f.path);
                added.push(json!({"gid": gid, "path": f.path}));
            }
            Err(e) => {
                let body = k.on_rpc_failure(&daemon, "aria2.addUri", e.clone());
                if body.code == kcodes::ENGINE_DISCONNECTED {
                    return Err(body);
                }
                rejected.push(json!({"path": f.path, "reason": e}));
            }
        }
    }
    if added.is_empty() && !rejected.is_empty() {
        return Err(ErrorBody::invalid_params(format!(
            "没有任何文件被加入下载：{rejected:?}"
        )));
    }
    Ok(json!({"added": added, "rejected": rejected}))
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
        Err(e) => return Err(k.on_rpc_failure(&d, "快照（tellActive/tellWaiting/tellStopped）", e)),
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
        "pause" | "unpause" | "retry" | "remove" | "clear_finished"
    ) {
        return Err(ErrorBody::invalid_params(format!(
            "不认识的动作 {action:?}（可用：pause / unpause / retry / remove / clear_finished）"
        )));
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
            rpc(d.remove(&gid))
        }
        "clear_finished" => rpc(d.clear_finished()),
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
            let pf = engine::new_planned_file(&m, &f).ok_or_else(|| {
                ErrorBody::invalid_params(format!("{} 的路径不安全，拒绝重试", f.path))
            })?;

            d.remove(&gid)
                .map_err(|e| ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e))?;
            // 用**当前的**逐任务选项重新 add（`Daemon::add` 内部取的就是当前那份）；
            // `-c` 恒开，所以这一下是从已下的部分续传。
            let new_gid = d
                .add(&pf.url, &pf.dir, &pf.out)
                .map_err(|e| ErrorBody::new(kcodes::ENGINE_RPC_FAILED, e))?;
            // `retry` 同样是"客户重新下载"的入口，闸门要开得与 `enqueue` **一模一样**
            // ——包括 `verifying`（曾经这里漏了它，见 `reopen_for_download` 的注释）。
            reopen_for_download(k, &f.path);
            Ok(json!({"action": "retry", "gid": new_gid, "path": f.path}))
        }
        // 上面已经把所有合法动作列完，这一支不可达；留着是为了 match 的穷尽性。
        other => Err(ErrorBody::invalid_params(format!(
            "不认识的动作 {other:?}（可用：pause / unpause / retry / remove / clear_finished）"
        ))),
    }
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
        let (disconnected, outcome) = reconnect_outcome(ok, &cause);
        self.engine_disconnected = disconnected;
        outcome?;
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

    // ④ 引擎在跑就即时生效（改参数不重启引擎）
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

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::OnceLock;

    use crate::testutil::{RawResponse, StubHttp, TempDir};

    /// **延迟探针（I-2）：`load_delivery` 的"拉清单"与"全量 plan"都必须在锁外跑。**
    ///
    /// 契约 §5.2 的原话是「校验不得占住协议循环或**任何长持有的锁**」。
    /// `load_delivery` 里两段最贵的都是网络 I/O：
    ///   - `delivery::fetch`：30 秒 × 3 次 + 1.5 秒退避 = 最坏 **91.5 秒**；
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
            "拉清单时仍持有内核锁：`fetch` 最坏 30s×3 + 退避 = 91.5 秒，\
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
        let (flag, outcome) = reconnect_outcome(true, "");
        assert!(!flag, "ping 通了必须清掉「已断开」，否则引擎还活着壳却永远收到断开");
        assert!(outcome.is_ok(), "ping 通了调用方应当继续用这个引擎");

        let (flag, outcome) = reconnect_outcome(false, "connection refused");
        assert!(flag, "ping 不通必须置上「已断开」");
        let e = outcome.expect_err("ping 不通必须回错误，不能静默继续");
        assert_eq!(
            e.code,
            kcodes::ENGINE_DISCONNECTED,
            "壳按这个码分支（设计规格 §9）: {e:?}"
        );
        assert!(
            e.message.contains("保留"),
            "消息必须告诉客户文件还在、可以直接重试: {}",
            e.message
        );
        // 失败原因要带出来，否则壳只能显示一句"已断开"、无从排查
        assert!(
            e.message.contains("connection refused"),
            "断开原因必须出现在消息里: {}",
            e.message
        );
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
        let e = engine_rpc_failed_error("aria2.addUri", "GID x is not found");
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
}
