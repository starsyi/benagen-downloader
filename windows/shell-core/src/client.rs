//! client —— 驱动内核子进程的那一层（规格 §5 第 2 条、"边界协议"）。
//!
//! 内核是 `benagen-core`（Rust），壳通过 stdio 上的 **JSON Lines** 协议驱动它：
//! 一行一条消息、`\n` 结尾。本模块只做四件事，**不含任何业务判断**（约束 1）：
//!
//!   1. 分配请求 `id`（从 1 起；0 是内核的保留值，见 [`crate::protocol::UNCORRELATED_ID`]）；
//!   2. 写一行请求、读回**配对的那一行**响应；
//!   3. 把 `id == 0` 的协议级错误留出来（它不是任何请求的结果）；
//!   4. 有界地收尾，别把子进程留成孤儿。
//!
//! 与 macOS 侧同形（`CoreClient.swift` + `LineChannel.swift`），三条**承重的**决定：
//!
//! * **stderr 必须主动排空**：接一根 `Pipe` 却不读它，子进程往 stderr 写满管道缓冲区
//!   （几十 KB）之后会**阻塞在 `write` 上**——它不再读 stdin、也不再写 stdout，
//!   整个请求-响应循环就此死锁，而症状是"壳卡住不动"，极难归因。
//!   所以有一条**专门的线程**把 stderr 读干，每行交给 `on_stderr_line`（默认转发到壳自己的
//!   stderr —— **不是** stdout：那是协议专用通道；也不是丢弃：丢的是排障信息）。
//! * **读行有上限**：内核侧对**请求**设了 `MAX_LINE_BYTES = 8 MiB`
//!   （`core/src/main.rs:113`），壳侧对**响应**也要有上界，否则一个坏掉的内核可以让壳
//!   无限吃内存。读到超限**给结构化错误**（[`ClientError::LineTooLong`]）并把这一行丢弃到
//!   行尾重新对齐——**绝不"断管道"**：断了之后所有请求都等不到配对，壳会永久卡死。
//! * **`id` 单调递增、响应按 `id` 配对**：不许把"下一行"当成"我那条"。
//!   内核**会**在回你那条之前先吐 `id == 0` 的协议级错误（超长请求就是这么回事）。
//!
//! ⚠️ **跨平台**：本模块只用 `std::process` / `std::thread` / `std::io`——没有一行平台 API。
//!    "shell-core 不含 OS 调用"（见 `lib.rs`）指的是不许出现 `winapi`/`eframe` 这类
//!    **平台专属**依赖；`std::process` 不是（规格 §5 第 2 条明确把"驱动内核子进程"放在壳里）。
//!    于是它照样能在 macOS 上 `cargo test`，而 `test.sh` 的依赖守卫也不必放宽。

use crate::protocol::{ErrorBody, Request, Response, UNCORRELATED_ID};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 两个上限
// ---------------------------------------------------------------------------

/// 壳**发出去**的一行请求的上限：**镜像内核的 `MAX_LINE_BYTES`**（`core/src/main.rs:113`）。
///
/// ⚠️ 这不是壳自己的美学选择，是一条**跨进程约定**：内核读请求时 `take(MAX_LINE_BYTES)`，
/// 超限就回一条 `id == 0` 的 `bad_request`——**它读不到你的 id**，于是壳"等自己那条响应"
/// 就是**永久挂死**。所以在写出去**之前**就拦住（[`ClientError::RequestTooLong`]），
/// 而且用**同一个数字**：壳拦下的正好是内核会拒的那些，不会多拦一个内核本来能收的请求。
///
/// ⚠️ **内核的额度里含换行符**——这是最容易差一个字节的地方：
/// 内核的 `read_line_capped` 先 `take(MAX_LINE_BYTES).read_until(b'\n')`（**`\n` 也在额度里**），
/// 再看 `buf.last()` 是不是 `\n`；末尾不是 `\n` 且 `buf.len() >= MAX_LINE_BYTES` 就判超限
/// （`core/src/main.rs:118-136`）。所以**正文恰好 `MAX` 字节**的一行（它后面还得跟一个 `\n`）
/// 在**内核那里就是超限**。判据因此是
/// `正文.len() >= MAX_REQUEST_LINE_BYTES`，等价于"允许的最大正文 = `MAX - 1`"。
/// （第一版的判据写的是 `>`，**正好放行了这个字节**——而那条路的尽头正是它要防的挂死；
/// 边界由 `a_request_of_exactly_the_cap_is_refused_because_the_newline_counts_too` /
/// `a_request_one_byte_under_the_cap_is_sent` 两条单元测试 +
/// e2e 的 `the_request_cap_boundary_matches_the_real_kernel` 三面踩住。）
pub const MAX_REQUEST_LINE_BYTES: usize = 8 << 20;

/// 壳**读进来**的一行响应的上限：**壳自己的自保护栏**，不是协议（更不是内核）规定的上限。
///
/// ⚠️ 它是**壳侧**的、**可以调大**的一次显式决定，跟上面那个跨进程约定不是一回事：
/// 内核自己**不设**响应上限（它没有任何一条响应需要接近这个量级，但也没有代码拦着），
/// 而最大的合法响应是 `load_delivery` 的整棵交付树——它随清单的文件数增长。
/// 取 8 倍于内核请求上限：既远高于任何真实响应，又能把一个发疯/被污染的内核
/// 从"无限吃内存"变成一条结构化错误。
///
/// ⚠️ 额度口径与内核相同（换行符也算在额度里）：**允许的最大正文 = `MAX - 1`**。
/// 改这个数字是一次**要被审查的决定**：调小会让大交付批次读不进来（响亮失败，
/// 不是静默），调大则护栏形同虚设。两个方向都不要顺手改。
pub const MAX_RESPONSE_LINE_BYTES: usize = 64 << 20;

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 驱动内核时可能出的事。**每一条都指名根因**（本项目对"根因不许说错"有纪律）。
///
/// ⚠️ 内核自己报的错走 [`ClientError::Kernel`]，`code` **按值**交给上层
/// （契约 §5.1：壳不得靠 `message` 的措辞判断错误类型）。其余变体是**传输层**的，
/// 它们出现的场合与内核无关（进程没了、管道断了、行超限、流对不上号）。
#[derive(Debug)]
pub enum ClientError {
    /// 起不了子进程（找不到可执行文件、权限、或排水线程起不来）。
    Spawn {
        path: PathBuf,
        cause: std::io::Error,
    },
    /// 壳要发出去的请求超过内核的请求上限——**本地拒绝，一个字节都不写出去**。
    RequestTooLong { bytes: usize, limit: usize },
    /// 请求序列化失败。⚠️ 实践中**不可达**（`params` 是 `Value`，永远序列化得出来），
    /// 但认得一个不会来的分支比漏认一个会来的分支便宜（同内核的 `codes::INTERNAL`）。
    RequestUnserializable { cause: serde_json::Error },
    /// 写请求失败（管道已断 / 通道已收尾）。
    WriteFailed { cause: std::io::Error },
    /// 读响应失败（读端已关、管道坏了）。**EOF 不走这里**——那是 [`ClientError::KernelGone`]。
    ReadFailed { cause: std::io::Error },
    /// 读到的一行不是合法 UTF-8。
    LineNotUtf8,
    /// 读到的一行超过 [`MAX_RESPONSE_LINE_BYTES`]：该行已被丢弃到行尾、协议已重新对齐，
    /// **通道仍然可用**。
    LineTooLong { limit: usize },
    /// 管道结束：内核进程没了（这条是"内核已死"的唯一可靠信号）。
    KernelGone,
    /// 内核发来的一行不是合法 JSON。`line_prefix` 截断到 200 字符——那行可能是几 MB 的垃圾，
    /// 而它会显示在界面上。
    MalformedResponse {
        line_prefix: String,
        cause: serde_json::Error,
    },
    /// 响应的 `id` 与在等的那条对不上（收发失步）。**响亮报错**：把一条无关的 `result`
    /// 当成自己的（界面显示另一个请求的数据）比吵一句糟得多。
    Desync { expected: u64, got: u64 },
    /// 内核回的结构化错误（`ok:false`）。`code` 见 [`crate::protocol::codes`]。
    Kernel { code: String, message: String },
    /// `ok:true` 却没有 `result`（内核的 `skip_serializing_if` 保证了这两个键互斥）。
    MissingResult { id: u64 },
    /// 已经收尾（`shutdown` 已调用），不再收新请求。
    Closed,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Spawn { path, cause } => write!(
                f,
                "无法建立内核子进程通道（{}）：{cause}。补救：核对内核产物是否存在、可执行。",
                path.display()
            ),
            // ⚠️ 补救那句话**只说界面上真的做得到的动作**（任务 17 改的）：原来写的是
            //    "把 enqueue 分批发"，而**界面里没有"分批"这个动作** —— 那是在教用户做一件
            //    他做不到的事（`DownloadTargets::blocked_reason` 那句话的措辞纪律同源）。
            //    ⚠️ `enqueue` 那条路今天会被那道**预算守卫**提前挡下（说的也是"少选一些"），
            //    这一句留着是给**别的**请求用的：它是这一层的通用自保，不是 enqueue 的文案。
            ClientError::RequestTooLong { bytes, limit } => write!(
                f,
                "请求行 {bytes} 字节，达到/超过内核读一行请求的上限 {limit} 字节\
                 （内核的额度里含行尾换行符，所以正文最多 {max_body} 字节）：\
                 内核读不到这种请求的 id、回执的 id 会是 0，壳会永远等不到配对——\
                 所以在写出去之前就拒绝（这不是内核报的错，是壳在自保）。\
                 补救：减少该请求携带的数据量（例如这一次少选一些，分几次下载）。",
                max_body = limit - 1
            ),
            ClientError::RequestUnserializable { cause } => {
                write!(f, "请求序列化失败：{cause}")
            }
            ClientError::WriteFailed { cause } => {
                write!(f, "向内核写入请求失败（管道已断）：{cause}")
            }
            ClientError::ReadFailed { cause } => {
                write!(f, "读取内核响应失败（管道已断）：{cause}")
            }
            ClientError::LineNotUtf8 => write!(f, "内核发来的一行不是合法 UTF-8（协议是 UTF-8 的 JSON Lines）"),
            ClientError::LineTooLong { limit } => write!(
                f,
                "内核发来的一行超过了**壳自己**设的读上限 {limit} 字节（这是壳的内存护栏、\
                 不是内核或协议规定的上限，必要时可以调大 MAX_RESPONSE_LINE_BYTES）；\
                 已把这一行丢弃到行尾并重新对齐，连接继续可用。\
                 这通常意味着内核发疯或流被污染。"
            ),
            ClientError::KernelGone => write!(f, "内核进程已退出（管道结束）"),
            ClientError::MalformedResponse { line_prefix, cause } => write!(
                f,
                "内核发来的一行不是合法 JSON：{cause}（行首：{line_prefix:?}）"
            ),
            ClientError::Desync { expected, got } => write!(
                f,
                "内核回的响应 id 是 {got}，而当前在等 {expected}（请求-响应失步）"
            ),
            ClientError::Kernel { code, message } => {
                write!(f, "内核报错 [{code}]：{message}")
            }
            ClientError::MissingResult { id } => {
                write!(f, "ok:true 的响应里没有 result（id {id}）")
            }
            ClientError::Closed => write!(f, "内核连接已关闭（shutdown 已调用）"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Spawn { cause, .. }
            | ClientError::WriteFailed { cause }
            | ClientError::ReadFailed { cause } => Some(cause),
            ClientError::RequestUnserializable { cause }
            | ClientError::MalformedResponse { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

/// 锁一把 `Mutex`，**忽略毒化**（`lock()` 的 `Err` 只在持锁线程 panic 过时出现，
/// 而那不该把它变成第二个 panic：内核侧对同一件事写的是
/// `unwrap_or_else(PoisonError::into_inner)`）。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 把一行（**不含换行**）的正文截到 200 字符给错误消息用。
///
/// 用 `chars()` 而不是字节切片：`&s[..200]` 会在多字节字符中间**panic**
/// （而那是一条错误消息，不是崩溃现场）。
fn prefix_200(line: &str) -> String {
    line.chars().take(200).collect()
}

// ---------------------------------------------------------------------------
// 带上限的行读取器（纯逻辑：任何 `BufRead` 都能喂，测试用 `Cursor` 即可）
// ---------------------------------------------------------------------------

/// 一次读的三种结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineOutcome {
    /// 读到了一行（已剥掉结尾的 `\n`）。
    Line(String),
    /// 流结束。
    Eof,
    /// 超过上限仍然没有换行。**该行的剩余部分已经被丢弃到行尾**，下一次读是对齐的。
    TooLong,
}

/// 丢弃超限行的剩余部分时，一次最多吃多少字节（内存有界）。
const DISCARD_CHUNK_BYTES: u64 = 64 * 1024;

/// 带上限的行读取器。语义与内核的 `read_line_capped`（`core/src/main.rs:113-142`）同形：
///
/// * 上限内读到换行 ⇒ [`LineOutcome::Line`]（剥掉 `\n`）；
/// * 超过上限仍无换行 ⇒ [`LineOutcome::TooLong`]，**并把剩余字节丢到行尾为止**
///   （内核那边**不排空**：它靠"碎片会被 JSON 解析自然拒掉"重新对齐。
///   壳这边不能这么干——壳的读者是唯一的，丢到行尾是唯一能让后续读对齐的做法，
///   而且它**有界**（每次最多 `DISCARD_CHUNK_BYTES`，读到换行或 EOF 就停）。
/// * 流结束 ⇒ [`LineOutcome::Eof`]。
///
/// ⚠️ 丢弃的**时间**上界由对端决定（它写多长就丢多长，读到换行或 EOF 才停），
/// 换来的是一条**能继续用**的通道。这是"壳是唯一读者"的必然代价：不排空就永远错位，
/// 而错位之后每一次读都会把半行当整行——那是**静默解出垃圾**，比多读一会儿糟得多。
///
/// ⚠️ **非法 UTF-8 报 `InvalidData` 而不是替换成 `U+FFFD`**：后者会把垃圾喂给 JSON 解析器，
/// 于是报错的地方离现场很远（"解出来一个奇怪的对象"而不是"这行根本不是 UTF-8"）。
pub struct CappedLineReader<R: BufRead> {
    inner: R,
    cap: usize,
}

impl<R: BufRead> CappedLineReader<R> {
    /// `cap` 必须 > 0（0 会让每一次读都立刻判超限）。
    pub fn new(inner: R, cap: usize) -> Self {
        assert!(cap > 0, "行长上限必须 > 0");
        Self { inner, cap }
    }

    pub fn read_line(&mut self) -> std::io::Result<LineOutcome> {
        let mut buf: Vec<u8> = Vec::new();
        // `take` 只吃上限那么多字节：缓冲区**有界**（内核侧的 `read_line_capped` 同形）。
        let mut limited = self.inner.by_ref().take(self.cap as u64);
        let n = limited.read_until(b'\n', &mut buf)?;
        drop(limited);
        if n == 0 && buf.is_empty() {
            return Ok(LineOutcome::Eof);
        }
        let ended = buf.last() == Some(&b'\n');
        if !ended && buf.len() >= self.cap {
            discard_to_newline(&mut self.inner)?;
            return Ok(LineOutcome::TooLong);
        }
        if ended {
            buf.pop();
        }
        let line = String::from_utf8(buf).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("这一行不是合法 UTF-8（非法字节出现在偏移 {}）", e.utf8_error().valid_up_to()),
            )
        })?;
        Ok(LineOutcome::Line(line))
    }
}

/// 把当前这一行的剩余字节读到换行（含）或 EOF。
fn discard_to_newline<R: BufRead>(r: &mut R) -> std::io::Result<()> {
    loop {
        let mut buf: Vec<u8> = Vec::new();
        let mut limited = r.by_ref().take(DISCARD_CHUNK_BYTES);
        let n = limited.read_until(b'\n', &mut buf)?;
        drop(limited);
        if n == 0 || buf.last() == Some(&b'\n') {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// 通道：一条写一行、读一行、关掉的接缝
// ---------------------------------------------------------------------------

/// 一条双向的行通道。
///
/// ⚠️ **这是测试接缝，不要给它加额外方法**：加了之后每个测试替身都得跟着实现一遍，
/// 而它们要证明的东西（配对、告警、EOF、收尾）与通道的实现细节无关。
/// （与 macOS `LineChannel.swift` 的协议逐条同形。）
///
/// `read_line()` 返回 `Ok(None)` 表示 **EOF**（对端关了写端 / 进程已退出）——
/// 它是"内核没了"的唯一可靠信号，必须与"读到一行空串"区分开。
///
/// 方法取 `&self`（而不是 `&mut self`）：通道要能被 `Arc` 共享、要能在一条请求
/// **正卡在 `read_line` 上**时被另一个线程 `close()` 掉——那正是"收尾不许被在飞请求堵死"
/// 的实现方式（见 [`CoreClient::shutdown`]）。
pub trait LineChannel: Send + Sync {
    /// 写一行。**行尾的 `\n` 由实现补上**（调用方给的是"一行的正文"）。
    fn write_line(&self, line: &str) -> Result<(), ClientError>;
    /// 读一行（不含结尾的 `\n`）。`Ok(None)` = EOF。
    fn read_line(&self) -> Result<Option<String>, ClientError>;
    /// 收尾。**幂等**，且**有上界**。
    fn close(&self);
}

/// 内核 stderr 的每一行。
pub type StderrSink = Box<dyn Fn(String) + Send>;

/// `std::process` 形态的行通道：stdin/stdout 是协议通道，stderr 是诊断通道。
pub struct ProcessChannel {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    /// `None` = 读端已经放掉了（[`ProcessChannel::close`] 之后）。
    stdout: Mutex<Option<CappedLineReader<BufReader<ChildStdout>>>>,
    stderr_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 排水线程是否已经收场。`JoinHandle::join` **没有超时**，所以收尾要的是这个旗子
    /// （见 [`ProcessChannel::close`] 的步骤 ③）。
    stderr_done: std::sync::Arc<AtomicBool>,
    closed: AtomicBool,
}

impl ProcessChannel {
    /// 起一个子进程并接上三条管道。
    ///
    /// - Parameter `on_stderr_line`：内核 stderr 的每一行。`None`（默认）时转发到**壳自己的
    ///   stderr**。这条排水线程**必须**存在（见模块头：不排空就是死锁），所以它起不来时
    ///   本函数**失败**而不是"没有排水线程也照跑"（W-2：不许静默降级）。
    pub fn new(
        executable: &Path,
        args: &[String],
        on_stderr_line: Option<StderrSink>,
    ) -> Result<Self, ClientError> {
        let mut cmd = Command::new(executable);
        cmd.args(args);
        Self::with_command(cmd, on_stderr_line)
    }

    /// 用一条**调用方组好的**命令起子进程。语义与 [`ProcessChannel::new`] 逐字相同
    /// （`new` 就是"组一条最朴素的命令"再调本函数），差别只在**命令从哪来**。
    ///
    /// ⚠️ **这个接缝的用途**：让调用方能在本函数 `spawn` **之前**定制那条 `Command`
    ///    ——例如套上**平台专属的进程创建参数**。这类定制为什么必须留在外面：
    ///    它在 `std` 里是 `std::os::*` 的**平台 API**，而本 crate **不许出现平台 API**
    ///    （见 `lib.rs`：本 crate 必须跨平台、纯逻辑；`test.sh` 的依赖守卫与这条
    ///    是同一件事的两面）。
    ///    ⇒ **组命令**那一步由调用方负责，本函数只负责
    ///    "起进程 + 接管三条管道 + stderr 排水线程"。
    ///    没有这个接缝，调用方就少一个"创建进程之前"的入口，只能绕开本模块自己起进程。
    ///
    /// ⚠️ **具体是哪个平台、哪些参数、为什么需要它们，一律不写在这里**：
    ///    那是平台知识，属于壳的可执行目标（`shell-win`）。本 crate 的散文里也不该出现
    ///    界面语义（`lib.rs` 的章程：不含任何界面概念）。
    ///
    /// ⚠️ **三根管道由本函数接管**：传进来的 `cmd` 上原先设过的 stdio 会被覆盖成
    ///    `piped`——那是**有意的**，管道的所有权与生命周期归 `ProcessChannel`，
    ///    由调用方设会让"谁负责哪根、谁负责关"说不清（本模块的收尾逻辑全建立在这三根上）。
    pub fn with_command(
        mut cmd: Command,
        on_stderr_line: Option<StderrSink>,
    ) -> Result<Self, ClientError> {
        // 出错时要报"哪个程序起不来"，而 `spawn` 会把 `cmd` 借走，
        // 所以先在手里留一份程序名（`Command` 只给得出 argv[0]）。
        let program = PathBuf::from(cmd.get_program());
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|cause| ClientError::Spawn {
            path: program.clone(),
            cause,
        })?;

        let stdin = child.stdin.take().expect("stdin 是 piped 的，take 一定拿得到");
        let stdout = child.stdout.take().expect("stdout 是 piped 的，take 一定拿得到");
        let stderr = child.stderr.take().expect("stderr 是 piped 的，take 一定拿得到");

        let sink = on_stderr_line.unwrap_or_else(|| Box::new(forward_stderr_to_our_stderr));
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let done_in_thread = std::sync::Arc::clone(&done);
        let drain = std::thread::Builder::new()
            .name("benagen-core-stderr".to_string())
            .spawn(move || {
                // ⚠️ 这一行**必须在 `spawn` 之后**才起（起失败时写端由上面关掉，
                //    否则这条线程会永远阻塞在一根没人写的管道上）。
                let mut reader = CappedLineReader::new(BufReader::new(stderr), MAX_RESPONSE_LINE_BYTES);
                loop {
                    match reader.read_line() {
                        Ok(LineOutcome::Line(l)) => sink(l),
                        // stderr 上超长的一行不该拖垮排水线程：**说一声**，继续排（否则
                        // "没人读"又回来了——那正是要防的死锁）。措辞里带够线索。
                        Ok(LineOutcome::TooLong) => sink(format!(
                            "[stderr 上有一行超过 {} MiB，已丢弃到行尾]",
                            MAX_RESPONSE_LINE_BYTES >> 20
                        )),
                        // EOF：写端没了，这是**正常**收场。
                        Ok(LineOutcome::Eof) => break,
                        // 读**失败**是另一回事（管道坏了、fd 被关掉）：两者都收场，
                        // 但失败的根因要**说出来**（W-2：不许静默降级；sink 就在手边，
                        // 说一句的成本是零）。正常收尾走的是上面那条 EOF 分支，不会吵。
                        Err(e) => {
                            sink(format!("[stderr 读取中断，后面的诊断可能会缺：{e}]"));
                            break;
                        }
                    }
                }
                done_in_thread.store(true, Ordering::SeqCst);
            });
        let drain = match drain {
            Ok(h) => h,
            Err(cause) => {
                // 没有排水线程 ⇒ 迟早死锁。**不降级**：把子进程收掉再报错。
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClientError::Spawn {
                    path: program,
                    cause: std::io::Error::new(
                        cause.kind(),
                        format!("stderr 排水线程起不来（没有它内核会被写满的管道堵死）：{cause}"),
                    ),
                });
            }
        };

        Ok(ProcessChannel {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            stdout: Mutex::new(Some(CappedLineReader::new(
                BufReader::new(stdout),
                MAX_RESPONSE_LINE_BYTES,
            ))),
            stderr_thread: Mutex::new(Some(drain)),
            stderr_done: done,
            closed: AtomicBool::new(false),
        })
    }

    /// 子进程还在跑吗？（收尾与测试用；`try_wait` 出错时按"不在了"回答——
    /// 真正的错误会在紧接着的读写上以结构化错误出现。）
    pub fn is_running(&self) -> bool {
        matches!(lock(&self.child).try_wait(), Ok(None))
    }

    /// 有界地等子进程退出。不用 `wait()`：它没有超时，内核卡住时会把壳一起卡住。
    fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match lock(&self.child).try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl LineChannel for ProcessChannel {
    fn write_line(&self, line: &str) -> Result<(), ClientError> {
        // 锁着整行的两次 write（正文 + 换行）：不许别的线程插进中间把行劈开。
        //
        // ⚠️ 写向一个**已经死掉**的子进程：Rust 的 std 运行时把 `SIGPIPE` 设成忽略，
        //    于是这里拿到 `EPIPE` 并变成一条 `WriteFailed`（不是"进程被信号杀掉"）。
        //    macOS 侧要在 `ProcessChannel.init` 里手动 `signal(SIGPIPE, SIG_IGN)`
        //    （Swift/Foundation 没有这层默认），Rust 有——所以这里**不需要** unsafe 的 `signal`。
        let mut guard = lock(&self.stdin);
        let Some(stdin) = guard.as_mut() else {
            return Err(ClientError::WriteFailed {
                cause: std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "通道已收尾（stdin 已关）",
                ),
            });
        };
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|cause| ClientError::WriteFailed { cause })
    }

    fn read_line(&self) -> Result<Option<String>, ClientError> {
        let mut guard = lock(&self.stdout);
        let Some(reader) = guard.as_mut() else {
            return Err(ClientError::ReadFailed {
                cause: std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "通道已收尾（读端已关）",
                ),
            });
        };
        match reader.read_line() {
            Ok(LineOutcome::Line(l)) => Ok(Some(l)),
            Ok(LineOutcome::Eof) => Ok(None),
            Ok(LineOutcome::TooLong) => Err(ClientError::LineTooLong {
                limit: MAX_RESPONSE_LINE_BYTES,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(ClientError::LineNotUtf8),
            Err(cause) => Err(ClientError::ReadFailed { cause }),
        }
    }

    /// 收尾，**幂等**，且有上界：
    ///
    /// ① 关 stdin 写端 → 内核读到 EOF 自行收尾退出（它自己会关掉 aria2、落盘）；
    /// ② 有界地等（3 秒）→ `kill()`（SIGKILL / TerminateProcess）→ 再等 2 秒；
    /// ③ 子进程确认没了之后，等排水线程结束（管道 EOF，它马上就返回）；
    /// ④ 放掉读端。
    ///
    /// ⚠️ **顺序不能变**：`close()` 会在"有线程正阻塞在 `read_line` 上"时被调用
    ///    （见 [`CoreClient::shutdown`] 的强制路径）。先让子进程死掉，那个卡住的读才会
    ///    以 EOF 收场、让出锁；**先**去拿读端的锁就是自己把自己的收尾堵死。
    /// ⚠️ 收不掉时**大声说**（这里没有返回值可用），不装作收掉了。
    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return; // 幂等
        }

        // ① 给内核一个"正常收尾"的机会（关掉 stdin 写端 ⇒ 内核读到 EOF ⇒ 自行收尾）。
        //
        //    ⚠️ **`try_lock`，不是 `lock`**：`write_line` 是**持着这把锁**做整行的写的
        //    （正文 + 换行，不许被劈开），而"子进程不再读 stdin + 请求大于管道缓冲区"
        //    会让那次写**无限期**阻塞。收尾若在这里 `lock`，就会跟着卡死——而收尾正是
        //    「`call` 没有超时」（裁决 Z）唯一被认可的逃生口，那样"最坏约 5 秒"就不真了。
        //    拿不到锁 ⇒ 跳过"体面告别"，直接走下面的 kill 分支：子进程一死，那次写拿到
        //    `EPIPE` 并以结构化错误收场，锁随之让出。**没有静默降级**——只是少了一句
        //    "请你体面退出"，而它本来就已经不读 stdin 了。
        match self.stdin.try_lock() {
            Ok(mut guard) => drop(guard.take()),
            Err(TryLockError::Poisoned(poisoned)) => drop(poisoned.into_inner().take()),
            Err(TryLockError::WouldBlock) => {}
        }

        // ② 有界等待 → kill → 再等
        if !self.wait_for_exit(Duration::from_secs(3)) {
            let _ = lock(&self.child).kill();
            if !self.wait_for_exit(Duration::from_secs(2)) {
                let pid = lock(&self.child).id();
                // W-2：不许静默降级。收不掉就说清楚，并给可执行的补救。
                let _ = writeln!(
                    std::io::stderr(),
                    "shell-core: 内核子进程（pid {pid}）在 kill 之后仍未回收，可能变成孤儿进程。\
                     补救：在任务管理器/`ps` 里结束它，并把这份日志连同复现步骤报给维护者。"
                );
                // 卡在 read 上的那一侧还持着读端的锁——这里**不能**去碰它。
                return;
            }
        }

        // ③ 子进程已经没了 ⇒ stderr 那根管道是 EOF，排水线程马上结束。
        //
        //    ⚠️ **有界地等，不用无界的 `join()`**：`JoinHandle::join` 没有超时，而这条线程
        //    可能正卡在 `sink` 里（默认 sink 是往壳自己的 stderr 写——万一那也是根没人读的
        //    管道，`write` 一样会阻塞）。那时 join 会把**收尾**堵死，症状又是"点了没反应"。
        //    macOS 侧对同一处用的是 `stderrGroup.wait(timeout: .now() + 2.0)`，同一个理由。
        //    （不能在排水线程里调 close()：这一步要等那条线程结束。）
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.stderr_done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let handle = lock(&self.stderr_thread).take();
        if self.stderr_done.load(Ordering::SeqCst) {
            if let Some(h) = handle {
                let _ = h.join();
            }
        } else if let Some(h) = handle {
            // W-2：不许静默降级——放它自流，但**说清楚**。
            drop(h);
            let _ = writeln!(
                std::io::stderr(),
                "shell-core: stderr 排水线程没在 2 秒内结束（它可能正卡在转发 stderr 的写操作上，\
                 例如壳自己的 stderr 是一根没人读的管道）；这里放它自流，不阻塞收尾。\
                 补救：确认壳的 stderr 有人读（或重定向到文件）。"
            );
        }

        // ④ 现在没有线程会阻塞在读上了，放掉读端（否则每次重启内核都漏两个 fd）。
        let mut stdout = lock(&self.stdout);
        drop(stdout.take());
        drop(stdout);
    }
}

impl Drop for ProcessChannel {
    fn drop(&mut self) {
        self.close();
    }
}

/// 默认的 stderr 去处：壳自己的 stderr，带前缀。
///
/// 用 `writeln!` 到一个**忽略返回值**的调用，而不是 `eprintln!`：后者在写失败时
/// **panic**（"failed printing to stderr"），那会把排水线程弄死——而排水线程一死，
/// 它要防的那个死锁就回来了。（macOS 侧选了 `fputs` 而不是会抛异常的
/// `FileHandle.write`，同一条理由。）
fn forward_stderr_to_our_stderr(line: String) {
    let _ = writeln!(std::io::stderr(), "[benagen-core] {line}");
}

// ---------------------------------------------------------------------------
// CoreClient：请求-响应循环
// ---------------------------------------------------------------------------

/// 告警表的上限。见 [`CoreClient::protocol_alerts`]。
pub const MAX_PROTOCOL_ALERTS: usize = 32;

/// `id == 0` 的协议级错误（超长行 / 畸形 JSON / 非法 UTF-8），**带上被丢掉的条数**。
///
/// ⚠️ 两个字段必须一起看：只读 `items` 的界面会把"内核吐了一万条"显示成"吐了 32 条"。
/// 做成一个结构体（而不是像 macOS 那样只返回数组 + 另一个计数方法）就是为了让
/// "丢了多少"**拿不掉**。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProtocolAlerts {
    /// 保留的告警（**最早**的那些，见 [`CoreClient::protocol_alerts`]）。
    pub items: Vec<ErrorBody>,
    /// 因为超过 [`MAX_PROTOCOL_ALERTS`] 而**没有留下来**的条数。
    pub dropped: u64,
}

struct ClientInner {
    /// 下一个请求 `id`。从 1 起（0 是内核的保留值），**只增不减**。
    next_id: u64,
    /// 收到过 `shutdown` 或强制收尾：不再收新请求。
    closed: bool,
    /// 见 [`CoreClient::protocol_alerts`]。
    alerts: ProtocolAlerts,
}

/// 驱动一个内核：发一条请求、等它那条响应、收尾。
///
/// **同一时刻至多一条在飞的请求**（约束 15）：[`CoreClient::call`] 全程持着一条内部的
/// 闸门锁，`id` 与通道都不会交错。**这个方法不需要"提高并发度"**——内核的信号量只有
/// 一条管道，两条请求同时发出去只会让响应配对失序。
///
/// 方法取 `&self`（不是 `&mut self`）：上层可以把它放进 `Arc` 共享，而
/// [`CoreClient::shutdown`] 能在一条请求**正卡在 `read_line` 上**时照样收尾
/// （见那里的两条路径）。这是 macOS `CoreClient` 那套队列 + 状态锁的等价物，
/// 只是把"串行队列"换成了"一把闸门锁"。
pub struct CoreClient {
    channel: Box<dyn LineChannel>,
    inner: Mutex<ClientInner>,
    /// 单飞闸门。`call` 全程持有；`shutdown` 用 `try_lock` 有界地探它。
    gate: Mutex<()>,
    shutdown_started: AtomicBool,
}

impl CoreClient {
    /// 接一条现成的通道（测试替身走这里）。
    pub fn new(channel: Box<dyn LineChannel>) -> Self {
        CoreClient {
            channel,
            inner: Mutex::new(ClientInner {
                next_id: 1,
                closed: false,
                alerts: ProtocolAlerts::default(),
            }),
            gate: Mutex::new(()),
            shutdown_started: AtomicBool::new(false),
        }
    }

    /// 起一个**真内核**子进程（`ProcessChannel`），用内核自己的默认值。
    ///
    /// ⚠️ 两个路径参数是**内核的输入**而不是协议消息，所以走 argv（结构化传参，不经 shell）
    /// ——见 [`core_arguments`]。生产上让内核用默认值即可；测试必须传一次性目录，
    /// 否则内核会读写**用户家目录**里的设置文件。
    pub fn spawn(executable: &Path, args: &[String]) -> Result<Self, ClientError> {
        Ok(CoreClient::new(Box::new(ProcessChannel::new(
            executable, args, None,
        )?)))
    }

    /// 内核回的**协议级**错误（`id == 0`），**有上界**。
    ///
    /// 这些行是**内核说了算**的：一个持续吐 `id == 0` 的坏内核（或一条被污染的流）能让这张表
    /// 无限增长——[`MAX_RESPONSE_LINE_BYTES`] 管住了**行的大小**，管不住**行的条数**。
    /// 所以这里也设一个界：留**最早**的 [`MAX_PROTOCOL_ALERTS`] 条（问题的起头比第一万条
    /// 重复更有诊断价值），之后只计数、不再存。
    ///
    /// ⚠️ 截断**必须可见**：返回值里带着 `dropped`（丢掉了多少条），所以
    /// "内核吐了一万条"不会显示成"吐了 32 条"（静默截断正是本项目最恨的形态）。
    ///
    /// 读它不经过单飞闸门（`gate`），所以**不会被一条在飞的慢请求挡住**——`inner` 只在
    /// 极短的临界区里被持有（取一个 id、推一条告警），不跨那次阻塞读。
    /// 这条是给界面线程用的：一条 90 秒的 `load_delivery` 不该把"显示一条告警"也拖住。
    pub fn protocol_alerts(&self) -> ProtocolAlerts {
        lock(&self.inner).alerts.clone()
    }

    /// 同步发一条请求，返回它的 `result`（`ok:false` 变成 [`ClientError::Kernel`]）。
    ///
    /// 返回的是**原始 [`Value`]**，不在这里做任何强类型解码：要不要强类型是调用方的事
    /// （少一次"编解码往返"就少一次 `.integer`/`.number` 被抹平的机会）。
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        // 单飞：整条请求-响应循环都在闸门里。它同时保证了"同一个内核上不会有两条在飞的
        // 请求"，于是下面那个 `id` 配对**不可能**撞上别人的响应（撞上就是失步，要吵）。
        let _gate = lock(&self.gate);

        let id = {
            let mut inner = lock(&self.inner);
            if inner.closed {
                return Err(ClientError::Closed);
            }
            let id = inner.next_id;
            inner.next_id += 1;
            id
        };

        let line = serde_json::to_string(&Request {
            id,
            method: method.to_string(),
            params,
        })
        .map_err(|cause| ClientError::RequestUnserializable { cause })?;

        // 写出去**之前**拦超长（见 MAX_REQUEST_LINE_BYTES 的文档：写出去就是永久挂死）。
        // ⚠️ 判据是 `>=` 而不是 `>`：内核的额度里**含换行符**（通道还会补一个 `\n`），
        //    所以正文恰好 MAX 字节的那一行在内核那里**已经是超限**。差这一个字节，
        //    换来的是"壳继续等一条永不到来的响应"——正是这个常量要拦的那件事。
        let bytes = line.len();
        if bytes >= MAX_REQUEST_LINE_BYTES {
            return Err(ClientError::RequestTooLong {
                bytes,
                limit: MAX_REQUEST_LINE_BYTES,
            });
        }
        self.channel.write_line(&line)?;

        loop {
            let Some(reply) = self.channel.read_line()? else {
                return Err(ClientError::KernelGone);
            };
            let resp: Response = serde_json::from_str(&reply).map_err(|cause| {
                ClientError::MalformedResponse {
                    line_prefix: prefix_200(&reply),
                    cause,
                }
            })?;

            // `id == 0` 是内核的保留值（连 id 都读不出来的畸形输入）。它不是任何请求的结果，
            // 所以既不能当结果、也不能当异常：留进告警，**接着等自己那条**。
            if resp.id == UNCORRELATED_ID {
                if let Some(e) = resp.error {
                    let mut inner = lock(&self.inner);
                    if inner.alerts.items.len() < MAX_PROTOCOL_ALERTS {
                        inner.alerts.items.push(e);
                    } else {
                        // 有界：超出的**计数**，不静默丢（见 `protocol_alerts`）。
                        inner.alerts.dropped += 1;
                    }
                }
                continue;
            }

            // 单飞语义下"别人的响应"不可能出现。真出现就是收发失步——把一条无关的 result
            // 当成自己的（界面显示另一个请求的数据）比吵一句糟得多。
            if resp.id != id {
                return Err(ClientError::Desync {
                    expected: id,
                    got: resp.id,
                });
            }

            if !resp.ok {
                // 缺 error 的 `ok:false` 现实中不该出现（内核的 `Response::err` 一定带 error）；
                // 真出现也**不 panic**：给一条空码的错误，让上层按"不认识的码"处理。
                let e = resp.error.unwrap_or(ErrorBody {
                    code: String::new(),
                    message: String::new(),
                });
                return Err(ClientError::Kernel {
                    code: e.code,
                    message: e.message,
                });
            }
            // ⚠️ "ok:false" 与"ok:true 却缺 result"两条守卫**只此一份**，不在这里复制到别处。
            return resp.result.ok_or(ClientError::MissingResult { id });
        }
    }

    /// 发 `shutdown` → 关通道。**幂等**，且**任何情况下都有上界**。
    ///
    /// ⚠️ **不等它的响应**：内核可能已经退出、管道已经断了，而 `read_line` 没有超时
    /// ——等下去会把"退出应用"变成"卡住不动"。内核收到 EOF 一样会收尾（关 aria2、落盘），
    /// 所以"没读到那条响应"不影响正确性。
    ///
    /// ⚠️ **也不能无界地等闸门**：内核正在跑长请求（`load_delivery` 最坏约 91.5 秒）时，
    /// 用户点退出，那条请求正卡在 `read_line` 上持着闸门——而唯一能让它松开的
    /// `channel.close()` 就在等闸门之后：那就成了"点了没反应"（约束 4）。所以：
    ///
    ///   1. 闸门**空着**（正常路径，毫秒级）：把 `shutdown` 请求发出去，再从容关通道；
    ///   2. 闸门**被占着**：走强制收尾——不等它，直接 `close()`。`ProcessChannel::close`
    ///      内部有界（关 stdin → 等 3 秒 → kill → 等 2 秒），子进程被回收后那条卡住的读
    ///      会以 EOF 收场，闸门随之让出。
    ///
    /// 最坏耗时约 5 秒（3 + 2），正常路径 < 10 毫秒。
    pub fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::SeqCst) {
            return;
        }

        // ① 正常路径：闸门空着。`try_lock` 拿不到就只有两种可能——被在飞请求占着（②），
        //    或者锁被毒化（某个线程持锁时 panic 过；那种情况走②同样安全：关 stdin
        //    之后内核读 EOF 也会自行退出，只是少了那句"体面地请你退出"）。
        if let Ok(_gate) = self.gate.try_lock() {
            let id = {
                let mut inner = lock(&self.inner);
                let id = inner.next_id;
                inner.next_id += 1;
                inner.closed = true;
                id
            };
            // 这句写失败**不算静默降级**：紧随其后的 `close()` 关掉 stdin，内核读到 EOF
            // 一样会收尾退出——没有"以为发了其实没发"的悬空状态（`closed` 已经置位）。
            let _ = self.channel.write_line(&format!(
                r#"{{"id":{id},"method":"shutdown","params":null}}"#
            ));
            self.channel.close();
            return;
        }

        // ② 强制路径：见上面那段说明。
        lock(&self.inner).closed = true;
        self.channel.close();
    }
}

impl Drop for CoreClient {
    fn drop(&mut self) {
        // 不留孤儿：`shutdown` 幂等且有上界，所以"上层忘了调"与"调过了"在这里同形。
        self.shutdown();
    }
}

/// 内核的 argv（**顺序与 `core/src/main.rs::parse_args` 一致**）。
///
/// 两个路径都是**内核的输入**而不是协议消息，所以走 argv（结构化传参，不经 shell）。
/// `None` 表示"让内核用默认值"（设置文件在用户家目录、下载目录是 `~/Downloads/Benagen`）。
///
/// ⚠️ 传了 `settings_path` 就**同时**决定了 `last_code` 落盘的目录
/// （`Kernel::new`：同一目录下的 `last_code`）——测试要的正是这个"不碰用户家目录"的效果。
pub fn core_arguments(download_dir: Option<&Path>, settings_path: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(d) = download_dir {
        args.push("--download-dir".to_string());
        args.push(d.to_string_lossy().into_owned());
    }
    if let Some(s) = settings_path {
        args.push("--settings".to_string());
        args.push(s.to_string_lossy().into_owned());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{codes, ErrorBody};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // ------------------------------------------------------------------
    // 测试替身：一条按脚本吐行的通道
    // ------------------------------------------------------------------
    //
    // 与 macOS 侧 `LineChannel` 协议同形（`LineChannel.swift`）：真机是子进程 + 三根管道，
    // 测试是一条脚本化的假通道。**接缝只有这一处**——加更多方法会让每个替身都得跟着实现，
    // 而它们要证明的东西（配对、告警、EOF、收尾）与通道的实现细节无关。

    enum Reply {
        Line(&'static str),
        Eof,
        /// 读侧超限（真实现里由 `CappedLineReader` 报出来）。
        TooLong,
        /// 读失败（管道断了之类）。
        ReadFailed,
    }

    struct StubInner {
        writes: Mutex<Vec<String>>,
        script: Mutex<VecDeque<Reply>>,
        closes: AtomicUsize,
    }

    #[derive(Clone)]
    struct StubChannel {
        inner: Arc<StubInner>,
    }

    impl StubChannel {
        fn new(script: Vec<Reply>) -> Self {
            Self {
                inner: Arc::new(StubInner {
                    writes: Mutex::new(Vec::new()),
                    script: Mutex::new(script.into()),
                    closes: AtomicUsize::new(0),
                }),
            }
        }
        /// 壳**实际写出去**的每一行（不含换行）。
        fn written(&self) -> Vec<String> {
            self.inner.writes.lock().unwrap().clone()
        }
        fn closes(&self) -> usize {
            self.inner.closes.load(Ordering::SeqCst)
        }
    }

    impl LineChannel for StubChannel {
        fn write_line(&self, line: &str) -> Result<(), ClientError> {
            self.inner.writes.lock().unwrap().push(line.to_string());
            Ok(())
        }
        fn read_line(&self) -> Result<Option<String>, ClientError> {
            match self.inner.script.lock().unwrap().pop_front() {
                Some(Reply::Line(s)) => Ok(Some(s.to_string())),
                // 脚本用完 = EOF：多数用例靠它收场
                Some(Reply::Eof) | None => Ok(None),
                Some(Reply::TooLong) => Err(ClientError::LineTooLong {
                    limit: MAX_RESPONSE_LINE_BYTES,
                }),
                Some(Reply::ReadFailed) => Err(ClientError::ReadFailed {
                    cause: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "管道断了"),
                }),
            }
        }
        fn close(&self) {
            self.inner.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn line(s: &'static str) -> Reply {
        Reply::Line(s)
    }

    /// 造一个 `params`，使 `{"id":1,"method":<method>,"params":<它>}` 序列化后**正好**
    /// `target` 字节（`id` 取 1：新客户端的第一个 id 就是 1）。
    ///
    /// 夹具**自己会断言**造出来的长度就是 `target`——边界用例的全部意义就是"正好踩在
    /// 那个字节上"，夹具偏一字节这条用例就白写了。
    fn params_for_line_len(target: usize, method: &str) -> Value {
        let line_len = |params: Value| {
            serde_json::to_string(&Request {
                id: 1,
                method: method.to_string(),
                params,
            })
            .expect("序列化不该失败")
            .len()
        };
        let base = line_len(json!({"pad": ""}));
        assert!(
            target > base,
            "目标长度装不下一个请求壳（{target} <= {base}）"
        );
        let params = json!({"pad": "x".repeat(target - base)});
        assert_eq!(line_len(params.clone()), target, "夹具自身没对上目标长度");
        params
    }

    // ------------------------------------------------------------------
    // 请求侧：编号 / 形状 / 超长拦截
    // ------------------------------------------------------------------

    /// 壳发出去的第一条请求必须就是设计规格 §5.1 的那一行，且 `id` 从 1 起、单调递增
    /// （0 是内核的保留值，见 `protocol::UNCORRELATED_ID`）。
    #[test]
    fn requests_are_numbered_from_one_and_monotonic() {
        let stub = StubChannel::new(vec![
            line(r#"{"id":1,"ok":true,"result":{"protocol":1}}"#),
            line(r#"{"id":2,"ok":true,"result":{}}"#),
        ]);
        let client = CoreClient::new(Box::new(stub.clone()));

        let v = client
            .call("hello", json!({"protocol": 1}))
            .expect("第一次握手必须成功");
        assert_eq!(v["protocol"], json!(1));
        client.call("get_tree", Value::Null).expect("第二次必须成功");

        let w = stub.written();
        assert_eq!(w.len(), 2, "两次调用写两行");
        assert_eq!(
            w[0], r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
            "第一条请求的形状是跨进程契约（设计规格 §5.1 的原文）"
        );
        assert_eq!(
            w[1], r#"{"id":2,"method":"get_tree","params":null}"#,
            "无参方法发 params:null（内核的 #[serde(default)] 接受它）"
        );
    }

    /// **按 id 配对，不假设顺序。**
    ///
    /// `id == 0` 是内核的协议级错误（超长行 / 畸形 JSON / 非法 UTF-8），它不是任何请求的结果
    /// ——而内核**会**在回我们那条之前先把它吐出来（一个超长请求就是这么回事）。把"下一行"
    /// 当成"我的答案"的壳会在这里拿到一条错误、或者永远等不到自己那条。
    #[test]
    fn protocol_level_errors_become_alerts_and_the_answer_is_still_paired_by_id() {
        let stub = StubChannel::new(vec![
            line(r#"{"id":0,"ok":false,"error":{"code":"bad_request","message":"请求行超过 8 MiB 上限"}}"#),
            line(r#"{"id":1,"ok":true,"result":{"protocol":1}}"#),
        ]);
        let client = CoreClient::new(Box::new(stub.clone()));

        let v = client
            .call("hello", json!({"protocol": 1}))
            .expect("id==0 的协议级错误不该打断这次请求");
        assert_eq!(v["protocol"], json!(1), "配到的必须是 id==1 那条");

        let alerts = client.protocol_alerts();
        assert_eq!(alerts.items.len(), 1, "协议级错误必须被留成告警");
        assert_eq!(alerts.dropped, 0, "一条都没丢");
        assert_eq!(
            alerts.items[0].code,
            codes::BAD_REQUEST,
            "告警的码按**值**取（契约 §5.1：壳不得靠 message 措辞判断错误类型）"
        );
        assert_eq!(
            alerts.items[0].message, "请求行超过 8 MiB 上限",
            "message 原文照登"
        );
    }

    /// **告警表必须有界**：行的大小有 `MAX_RESPONSE_LINE_BYTES` 管着，**行的条数没人管**
    /// ——一个持续吐 `id == 0` 的坏内核可以把它撑到无限大（同一类"无限吃内存"，换个维度）。
    /// 而且截断**不许静默**：丢掉的条数必须报出来。
    #[test]
    fn the_alert_list_is_bounded_and_the_dropped_count_is_reported() {
        const EXTRA: usize = 5;
        let mut script: Vec<Reply> = (0..MAX_PROTOCOL_ALERTS + EXTRA)
            .map(|_| line(r#"{"id":0,"ok":false,"error":{"code":"bad_request","message":"x"}}"#))
            .collect();
        script.push(line(r#"{"id":1,"ok":true,"result":{}}"#));
        let stub = StubChannel::new(script);
        let client = CoreClient::new(Box::new(stub.clone()));

        client
            .call("hello", json!({"protocol": 1}))
            .expect("告警再多也不影响配对");

        let alerts = client.protocol_alerts();
        assert_eq!(
            alerts.items.len(),
            MAX_PROTOCOL_ALERTS,
            "留下来的条数必须封顶（不然坏内核能把内存吃光）"
        );
        assert_eq!(
            alerts.dropped, EXTRA as u64,
            "丢掉的条数必须报出来——静默截断会把\"吐了一万条\"显示成\"吐了 32 条\""
        );
    }

    /// 失步必须**响亮**：单飞语义下不该出现别人的响应，真出现就是收发失步——
    /// 把一条无关的 result 当成自己的（界面显示另一个请求的数据）比吵一句糟得多。
    #[test]
    fn a_response_for_another_id_is_a_loud_desync_not_an_answer() {
        let stub = StubChannel::new(vec![line(r#"{"id":7,"ok":true,"result":{}}"#)]);
        let client = CoreClient::new(Box::new(stub.clone()));

        match client.call("hello", json!({"protocol": 1})).expect_err("失步必须报错") {
            ClientError::Desync { expected, got } => {
                assert_eq!(expected, 1, "壳在等的是自己刚发出去那条");
                assert_eq!(got, 7, "内核回的是另一条");
            }
            other => panic!("应为 Desync，实际 {other:?}"),
        }
    }

    /// EOF = 内核没了。必须是一条**结构化错误**：不是 panic、更不能是永久挂起。
    #[test]
    fn eof_becomes_a_structured_error() {
        let stub = StubChannel::new(vec![Reply::Eof]);
        let client = CoreClient::new(Box::new(stub.clone()));
        let err = client
            .call("hello", json!({"protocol": 1}))
            .expect_err("内核进程没了必须报错");
        assert!(matches!(err, ClientError::KernelGone), "实际 {err:?}");
    }

    /// `ok:false` 把内核的**结构化码**原样交给上层。
    #[test]
    fn ok_false_carries_the_kernel_code_by_value() {
        let stub = StubChannel::new(vec![line(
            r#"{"id":1,"ok":false,"error":{"code":"engine_not_started","message":"引擎还没起来"}}"#,
        )]);
        let client = CoreClient::new(Box::new(stub.clone()));
        match client
            .call("transfer_list", Value::Null)
            .expect_err("ok:false 必须报错")
        {
            ClientError::Kernel { code, message } => {
                assert_eq!(code, codes::ENGINE_NOT_STARTED, "壳按这个值分支");
                assert_eq!(message, "引擎还没起来", "message 原文照登，不加工");
            }
            other => panic!("应为 Kernel，实际 {other:?}"),
        }
    }

    /// `ok:true` 却没有 `result` 也是结构化错误（与 macOS `RawEnvelope.unwrapResult` 同一条守卫）。
    #[test]
    fn ok_true_without_result_is_a_structured_error() {
        let stub = StubChannel::new(vec![line(r#"{"id":1,"ok":true}"#)]);
        let client = CoreClient::new(Box::new(stub.clone()));
        match client.call("get_tree", Value::Null).expect_err("缺 result 必须报错") {
            ClientError::MissingResult { id } => assert_eq!(id, 1),
            other => panic!("应为 MissingResult，实际 {other:?}"),
        }
    }

    /// **读失败与 EOF 必须走不同的分支**（内核在 `read_line_capped` 上记过同一条账）：
    /// 把一次坏管道静默当成"内核没了"，会让人去查错的根因。
    #[test]
    fn a_read_failure_is_not_reported_as_eof() {
        let stub = StubChannel::new(vec![Reply::ReadFailed]);
        let client = CoreClient::new(Box::new(stub.clone()));
        match client.call("hello", json!({"protocol": 1})) {
            Err(ClientError::ReadFailed { .. }) => {}
            other => panic!("应为 ReadFailed，实际 {other:?}"),
        }
    }

    /// **壳不许把一个超过内核上限的请求发出去。**
    ///
    /// 内核在那种输入下回的是 `id == 0` 的坏请求错误（`read_line_capped` 读不到 id），
    /// 于是"等自己那条响应"就是**永久挂死**——这正是超长行这一整类坑的形态。
    /// 所以在写出去之前就拦下来，并把两个数字都给出来。
    #[test]
    fn an_over_long_request_is_refused_locally_and_never_written() {
        let stub = StubChannel::new(vec![]);
        let client = CoreClient::new(Box::new(stub.clone()));
        let pad = "x".repeat(MAX_REQUEST_LINE_BYTES);
        match client
            .call("hello", json!({"protocol": 1, "pad": pad}))
            .expect_err("超长请求必须被本地拒绝")
        {
            ClientError::RequestTooLong { bytes, limit } => {
                assert!(bytes > limit, "报出来的字节数必须真的超限（{bytes} vs {limit}）");
                assert_eq!(limit, MAX_REQUEST_LINE_BYTES);
            }
            other => panic!("应为 RequestTooLong，实际 {other:?}"),
        }
        assert!(
            stub.written().is_empty(),
            "被拒的请求**一个字节都不许写出去**（写出去就会换来一次永久挂死）"
        );
    }

    /// ⚠️ **那句补救只说界面上真的做得到的动作**（任务 17 改的：原文是"把 enqueue 分批发"，
    /// 而**界面里没有"分批"这个动作**——那是在教用户做一件他做不到的事）。
    ///
    /// 判别力：把补救那半句改回（或改成任何"壳会自动分批 / 壳会替你切"的说法）⇒ 这一条红。
    /// ⚠️ 与 macOS 的同一条纪律：那边也要求这句话**给出路**而不是光说"不行"
    /// （`AppModelTests.swift:1087`），只是本代的出路**只有**"少选一些"这一条
    /// （macOS 还能说「全选」—— 它那颗「全选」是**整批**，而本代那颗只选当前这一层）。
    #[test]
    fn the_over_long_request_text_points_at_an_action_the_user_can_actually_take() {
        let text = ClientError::RequestTooLong {
            bytes: 9 << 20,
            limit: MAX_REQUEST_LINE_BYTES,
        }
        .to_string();
        assert!(text.contains("少选一些"), "必须给出路，不是光说「不行」：{text}");
        assert!(
            !text.contains("分批发"),
            "「把 enqueue 分批发」是界面做不到的事（界面里没有「分批」这个动作）：{text}"
        );
    }

    /// **边界：正文恰好 `MAX_REQUEST_LINE_BYTES` 字节 ⇒ 必须被本地拒掉。**
    ///
    /// ⚠️ 这一条钉的是**一个字节**的差：内核的额度里**含换行符**
    /// （`read_line_capped` 先把 `MAX_LINE_BYTES` 个字节读进 `buf`、再看末尾是不是 `\n`，
    /// 见 `core/src/main.rs:118-136`），所以"正文 MAX 字节"在**内核那里就是超限**。
    /// 壳若放行，会换来一条 `id == 0` 的 `bad_request`（内核读不到 id），
    /// 而 `call` 会继续等自己那条**永不到来**的响应——就是这个常量存在的全部理由。
    #[test]
    fn a_request_of_exactly_the_cap_is_refused_because_the_newline_counts_too() {
        let stub = StubChannel::new(vec![]);
        let client = CoreClient::new(Box::new(stub.clone()));
        let params = params_for_line_len(MAX_REQUEST_LINE_BYTES, "hello");

        match client.call("hello", params) {
            Err(ClientError::RequestTooLong { bytes, limit }) => assert_eq!(
                bytes, limit,
                "正文 MAX 字节 + 换行 ⇒ 已经踩在内核的超限上，报出来的字节数就是 MAX"
            ),
            other => panic!(
                "应为 RequestTooLong（这一行在内核那里是超限的；放行 = 永久挂死），实际 {other:?}"
            ),
        }
        assert!(stub.written().is_empty(), "被拒的请求**一个字节都不许写出去**");
    }

    /// 边界的另一侧：正文 `MAX_REQUEST_LINE_BYTES - 1` 字节（+ 换行 = 正好 MAX）
    /// ⇒ **内核收得下**，壳必须照发。
    ///
    /// 少了这一条，"判据收紧"可以一路收紧到 0 而没有任何东西变红。
    #[test]
    fn a_request_one_byte_under_the_cap_is_sent() {
        let stub = StubChannel::new(vec![line(r#"{"id":1,"ok":true,"result":{}}"#)]);
        let client = CoreClient::new(Box::new(stub.clone()));
        let params = params_for_line_len(MAX_REQUEST_LINE_BYTES - 1, "hello");

        client
            .call("hello", params)
            .expect("比上限少一个字节的请求必须发得出去（内核收得下这一行）");
        let w = stub.written();
        assert_eq!(w.len(), 1, "必须真的写出去了一行");
        assert_eq!(
            w[0].len(),
            MAX_REQUEST_LINE_BYTES - 1,
            "写出去的正文就是 MAX-1 字节（通道自己补换行）"
        );
    }

    // ------------------------------------------------------------------
    // 读侧：超限 / 收尾
    // ------------------------------------------------------------------

    /// 读侧超限：**结构化错误，且管道继续可用**。
    ///
    /// "断管道"是错的处置：断了之后所有在飞/后续请求都拿不到配对，壳会永久卡死。
    /// 正确处置是丢掉这一行的剩余部分、重新对齐，然后把一条错误交给当前那条请求。
    #[test]
    fn an_over_long_response_is_a_structured_error_and_the_channel_survives() {
        let stub = StubChannel::new(vec![
            Reply::TooLong,
            line(r#"{"id":2,"ok":true,"result":{"version":3}}"#),
        ]);
        let client = CoreClient::new(Box::new(stub.clone()));

        let err = client.call("get_state", Value::Null).expect_err("超限必须报错");
        assert!(matches!(err, ClientError::LineTooLong { .. }), "实际 {err:?}");
        assert_eq!(stub.closes(), 0, "读侧超限**不许**关通道");
        let v = client.call("get_state", Value::Null).expect("下一条请求必须还能跑");
        assert_eq!(v["version"], json!(3));
    }

    /// 收尾：幂等；`shutdown` 请求只发一次（之后不再收新请求，且是**快速失败**）。
    #[test]
    fn shutdown_is_idempotent_and_later_calls_fail_fast() {
        let stub = StubChannel::new(vec![]);
        let client = CoreClient::new(Box::new(stub.clone()));

        client.shutdown();
        client.shutdown();

        let w = stub.written();
        assert_eq!(w.len(), 1, "shutdown 请求只许发一次");
        assert_eq!(w[0], r#"{"id":1,"method":"shutdown","params":null}"#);
        assert_eq!(stub.closes(), 1, "通道只关一次");

        let err = client
            .call("hello", json!({"protocol": 1}))
            .expect_err("收尾之后不再收请求");
        assert!(matches!(err, ClientError::Closed), "实际 {err:?}");
    }

    // ------------------------------------------------------------------
    // 纯逻辑：带上限的行读取器（不起进程就能全测）
    // ------------------------------------------------------------------

    #[test]
    fn capped_reader_reads_a_short_line_and_then_reports_eof() {
        let mut r = CappedLineReader::new(Cursor::new(b"{\"a\":1}\n".to_vec()), 64);
        assert_eq!(
            r.read_line().expect("读不该失败"),
            LineOutcome::Line("{\"a\":1}".to_string()),
            "行尾的 \\n 必须被剥掉"
        );
        assert_eq!(r.read_line().expect("读不该失败"), LineOutcome::Eof);
    }

    /// 超限行：报 `TooLong`，并**丢弃到行尾重新对齐**——否则下一次读会把半行当整行，
    /// 之后永远错位（那才是"壳静默解出垃圾"的来源）。
    #[test]
    fn capped_reader_reports_over_long_and_resyncs_on_the_next_line() {
        let mut bytes = vec![b'x'; 200];
        bytes.push(b'\n');
        bytes.extend_from_slice(b"{\"id\":2}\n");
        let mut r = CappedLineReader::new(Cursor::new(bytes), 16);

        assert_eq!(
            r.read_line().expect("读不该失败"),
            LineOutcome::TooLong,
            "超限行必须报 TooLong，不许截断成一行交出去"
        );
        assert_eq!(
            r.read_line().expect("读不该失败"),
            LineOutcome::Line("{\"id\":2}".to_string()),
            "丢弃到行尾之后必须重新对齐"
        );
        assert_eq!(r.read_line().expect("读不该失败"), LineOutcome::Eof);
    }

    /// 非法 UTF-8 **不许静默替换**（`from_utf8_lossy` 会把垃圾交给 JSON 解析器，
    /// 而那是"报错的地方离现场很远"）。在切行的地方就判掉，判据是 `InvalidData`。
    #[test]
    fn capped_reader_rejects_invalid_utf8_instead_of_mangling_it() {
        let mut r = CappedLineReader::new(Cursor::new(vec![0xff, 0xfe, b'\n']), 64);
        let err = r.read_line().expect_err("非法 UTF-8 必须报错");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// 两个上限：请求侧是**跨进程约定**（镜像内核 `core/src/main.rs:113`），
    /// 读侧是一次显式决定（内核自己不设响应上限，树可以很大）。
    #[test]
    fn the_line_limits_are_pinned() {
        assert_eq!(
            MAX_REQUEST_LINE_BYTES,
            8 << 20,
            "内核的 MAX_LINE_BYTES 是 8 MiB；壳发出去的行不得更大（内核回执不带 id ⇒ 挂死）"
        );
        assert_eq!(
            MAX_RESPONSE_LINE_BYTES,
            64 << 20,
            "读侧上限是一次显式决定：改这个数字前先重读它文档注释里的理由"
        );
        assert!(
            MAX_RESPONSE_LINE_BYTES > MAX_REQUEST_LINE_BYTES,
            "响应（整棵交付树）可以比请求大，两个上限不是同一个东西"
        );
    }

    /// argv 是**内核的输入**（不是协议消息），顺序与 `core/src/main.rs::parse_args` 一致。
    #[test]
    fn core_arguments_match_the_kernel_flags() {
        assert_eq!(core_arguments(None, None), Vec::<String>::new());
        assert_eq!(
            core_arguments(Some(Path::new("/tmp/d")), None),
            vec!["--download-dir", "/tmp/d"]
        );
        assert_eq!(
            core_arguments(None, Some(Path::new("/tmp/s"))),
            vec!["--settings", "/tmp/s"]
        );
        assert_eq!(
            core_arguments(Some(Path::new("/tmp/d")), Some(Path::new("/tmp/s"))),
            vec!["--download-dir", "/tmp/d", "--settings", "/tmp/s"]
        );
    }

    #[test]
    fn error_bodies_are_cloneable_for_the_alert_list() {
        // 告警列表要能交出去（`protocol_alerts()` 返回一份克隆），这条只是把它钉住：
        // 少了 `Clone`，那一处会以一条编译错误的形式发现——不如在这里钉住。
        let e = ErrorBody {
            code: codes::BAD_REQUEST.to_string(),
            message: "x".to_string(),
        };
        assert_eq!(e.clone().code, codes::BAD_REQUEST);
    }

    // ------------------------------------------------------------------
    // 真子进程：stderr 排空 / 写向死管道
    // ------------------------------------------------------------------
    //
    // 这两条用一个自愿灌 stderr 的 `sh` 子进程，而不是真内核：内核不会自愿写满 stderr
    // （它一句诊断都不一定打），而这里要证的恰恰是"写满之后会不会阻塞"。
    //
    // ⚠️ 用 `sh` 不是"跳过 Windows"：本项目的 Windows 测试入口是 Git Bash/MSYS2
    //    （`windows/scripts/test.sh` 自己就是 bash 脚本，头部注释写着这条假设），
    //    `sh` 在 PATH 里。真找不到时**响亮失败**（W-2），不静默跳过。

    fn sh(script: &str) -> (String, Vec<String>) {
        (
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
        )
    }

    /// **stderr 必须主动排空。** 没人读的管道写满之后子进程会**阻塞在 `write` 上**：
    /// 它不再读 stdin、也不再写 stdout，整个请求-响应循环死锁，症状是"壳卡住不动"。
    ///
    /// 判别力：把排水线程去掉，这个子进程会卡在第一万行左右（管道缓冲区约 64 KiB），
    /// 于是 stdout 那一行永远不来——下面那条**带超时**的读会失败。用例是"变红"，
    /// 不是"挂住"：超时后先 `close()` 把子进程收掉，卡住的那一读才会以 EOF 收场。
    #[test]
    fn stderr_is_drained_so_the_child_never_blocks_on_write() {
        const LINES: usize = 4000;
        let script = format!(
            "i=0; while [ $i -lt {LINES} ]; do \
               echo \"诊断行 $i 这一行有几十个字节，用来把管道的缓冲区填满\" >&2; \
               i=$((i+1)); done; \
             echo '{{\"id\":1,\"ok\":true,\"result\":{{}}}}'"
        );
        let (prog, args) = sh(&script);
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let channel = ProcessChannel::new(
            Path::new(&prog),
            &args,
            Some(Box::new(move |l: String| sink.lock().unwrap().push(l))),
        )
        .expect("必须能起 sh 子进程（起不来是环境问题，这里响亮失败而不是跳过）");

        let line = std::thread::scope(|s| {
            let reader = s.spawn(|| channel.read_line());
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if reader.is_finished() {
                    break reader.join().expect("读线程不该 panic");
                }
                if Instant::now() > deadline {
                    // 先把子进程收掉：否则 scope 结束时那个卡住的读会让本用例**挂住**
                    channel.close();
                    let _ = reader.join();
                    panic!(
                        "15 秒内没等到 stdout 的那一行：stderr 显然没有被排空（子进程阻塞在 write 上）"
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        assert_eq!(
            line.expect("读到一行").expect("读不该失败"),
            r#"{"id":1,"ok":true,"result":{}}"#,
            "stdout 是协议通道，内容必须逐字"
        );

        // 排水线程必须把每一行都交出去（它是唯一在读那根管道的人）
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.lock().unwrap().len() < LINES && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let got = seen.lock().unwrap().len();
        assert_eq!(got, LINES, "stderr 的每一行都必须被排水线程读走（实际读到 {got} 行）");
        assert!(
            seen.lock().unwrap()[0].contains("诊断行 0"),
            "转发的是原文（不加工）"
        );

        channel.close();
    }

    /// 内核发来的一行**不是合法 UTF-8**：必须是 `LineNotUtf8` 这条明确结论，
    /// 不许用 `from_utf8_lossy` 把它换成 U+FFFD 再交给 JSON 解析器
    /// （那样报出来的会是"这一行不是合法 JSON"，把根因指错地方）。
    #[test]
    fn a_non_utf8_response_line_is_reported_as_such() {
        // `printf` 是 sh 的内建，`\377\376` 是八进制转义：写出两个非法字节再补换行。
        let (prog, args) = sh(r"printf '\377\376\n'");
        let channel = ProcessChannel::new(Path::new(&prog), &args, None).expect("必须能起 sh");
        match channel.read_line() {
            Err(ClientError::LineNotUtf8) => {}
            other => panic!("应为 LineNotUtf8，实际 {other:?}"),
        }
        channel.close();
    }

    /// **收尾在任何情形下都有上界**——包括"有线程正卡在 `write_line` 里"这一种。
    ///
    /// 场景：子进程收下 stdin 的写端却从不读它，而请求大于管道缓冲区 ⇒ 那次写**永远卡在
    /// `write` 里**并**持着 stdin 的锁**。收尾若在那里用 `lock` 就会跟着卡死——而收尾正是
    /// 「`call` 没有超时」（裁决 Z）唯一被认可的逃生口，它一卡，"最坏约 5 秒"就不真了。
    ///
    /// 判别力：把 `close()` 里那句 `try_lock` 换回 `lock`，本用例会**挂住**（不是变红）——
    /// 所以下面每条断言都带时间上界，且写线程在收尾之后必须能收场。
    #[test]
    fn close_stays_bounded_even_when_a_writer_is_stuck_on_stdin() {
        // `exec sleep` 把 sh 自己替换成 sleep：**收下一个从不读 stdin 的子进程**，
        // 而且 kill 掉的就是它本身（不会留下一个孤儿 sleep 在后台数到 30）。
        let (prog, args) = sh("exec sleep 30");
        let channel = ProcessChannel::new(Path::new(&prog), &args, None).expect("必须能起 sh");

        std::thread::scope(|s| {
            let writer = s.spawn(|| {
                // 远大于管道缓冲区（macOS 上 64 KiB 级）的一行 ⇒ 这次写会卡住
                let _ = channel.write_line(&"x".repeat(4 * 1024 * 1024));
            });
            // 给它足够时间真的卡在 write 上（拿不到 stdin 的锁就是这里的结果）
            std::thread::sleep(Duration::from_millis(300));

            let started = Instant::now();
            channel.close();
            let took = started.elapsed();
            assert!(
                took < Duration::from_secs(10),
                "收尾必须有上界（子进程会被 kill），实际用了 {took:?}"
            );
            assert!(!channel.is_running(), "收尾之后子进程必须被回收");

            // 写完那一侧会以一条结构化错误收场（EPIPE），不会永远卡在锁里
            writer.join().expect("写线程不该 panic");
        });
    }

    /// 写向一个已经退出的子进程：必须是**结构化错误**——不是崩溃、不是静默。
    ///
    /// ⚠️ 这条同时钉住 Rust 的 SIGPIPE 处置：std 的运行时把 `SIGPIPE` 设成忽略，
    /// 于是这一写拿到 `EPIPE` 并变成 `WriteFailed`。若哪天它变回默认处置，
    /// **本用例进程会被信号直接杀掉**（平台会记一次崩溃），而不是得到一条错误。
    /// （macOS 侧要在 `ProcessChannel.init` 里手动 `signal(SIGPIPE, SIG_IGN)`，
    ///   Swift/Foundation 没有这层默认——Rust 有，所以这里**不需要** unsafe 的 `signal`。）
    ///
    /// ⚠️ **为什么判据是"有界重试之后终究是结构化错误"，不是"第一次写就必须失败"**
    /// （本用例曾经偶发红，负载下约 0.3%）：**"子进程已被回收"不等于"它的管道读端已经释放"** ——
    ///    macOS/xnu 在这两者之间有一个 **µs 级的窗口**。它一旦命中，`write` 会把那 8 个字节
    ///    写进一个马上要被拆掉的管道并**返回成功**（实测：同一 fd 上紧接着的 1 MiB 写在
    ///    875 ns–2.5 µs 之后才拿到 EPIPE）。
    ///
    ///    **它不在本模块里**（这一点是实测的，不是推断）：用**纯 `std::process`** 复刻同一时序、
    ///    完全不经过 `ProcessChannel`，同样负载下异常率一模一样（本机复现：24 忙循环 + 6 并发
    ///    进程，8/2400 = 0.33%）⇒ **动 `is_running()` 一点用都没有**（插桩实测：
    ///    4000 次迭代 / 110 次异常里 `try_wait` 的 `Err` 一次都没出现，每次都是"子进程确实已被回收"）。
    ///
    ///    ⇒ 所以要改的是**本用例的前提**（"已回收 ⇒ 写必然 EPIPE"不成立）。
    ///    两条路都验证过：
    ///      · ① **夹具**：让子进程**先关掉 stdin、隔一会儿再退出**
    ///        （`sh -c 'exec 0<&-; sleep 0.2; exit 0'`）⇒ 实测 **0/1800**；
    ///        ⚠️ 但**紧挨着**的写法（`exec 0<&-; exit 0`，两条命令相隔 µs）**只把窗口收窄、
    ///        没有关掉**（实测 2/2400，仍有残留）；
    ///      · ② **判据**（**本用例采用**）：对"写成功"做**有界重试**，断言它在期限内
    ///        **终究**变成结构化错误 —— 不依赖夹具的时序，也不给夹具加 `sleep`。
    ///        真正承重的契约本来就是"它终究会以结构化错误收场"（那 8 个字节写进一个没人读的
    ///        管道没有任何后果），而不是"第一次写就必须失败"。
    ///    判别力：若 `write_line` 变成**永远成功**（或永远返回别的错误），本用例在有界重试
    ///    之后仍然拿不到 `WriteFailed` ⇒ **红**（不是挂住）。
    #[test]
    fn writing_to_a_dead_child_is_a_structured_error_not_a_crash() {
        let (prog, args) = sh("exit 0");
        let channel = ProcessChannel::new(Path::new(&prog), &args, None).expect("必须能起 sh");

        let deadline = Instant::now() + Duration::from_secs(10);
        while channel.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!channel.is_running(), "子进程必须已经退出（前面只有一句 exit 0）");

        // ② 有界重试：撞进上面那个 µs 级窗口时第一次写会"成功"，紧接着的写必然 EPIPE。
        const RETRY_WINDOW: Duration = Duration::from_millis(100);
        let retry_deadline = Instant::now() + RETRY_WINDOW;
        let outcome = loop {
            match channel.write_line(r#"{"id":1}"#) {
                Err(e) => break Err(e),
                Ok(()) if Instant::now() >= retry_deadline => break Ok(()),
                Ok(()) => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        let err = outcome.expect_err(
            "写向死管道**最终**必须报错：有界重试 100 ms 之后每一次都返回成功 \
             ⇒ 这不是退出竞态，是真的不再报错了",
        );
        assert!(matches!(err, ClientError::WriteFailed { .. }), "实际 {err:?}");
        assert_eq!(channel.read_line().expect("读不该炸"), None, "进程没了就是 EOF");

        channel.close();
    }

    // ------------------------------------------------------------------
    // `with_command`：给调用方的接缝
    // ------------------------------------------------------------------
    //
    // 这个构造器是**为了给调用方一个"在 spawn 之前定制命令"的入口**才加的：
    // 命令由调用方组（平台专属的进程创建参数只能在那里出现），
    // 本 crate 只管起进程、接管三条管道。
    // 下面两条钉住"它就是 `new`，只是命令从外面来"，免得日后有人把它改成另一套语义。

    /// 交进来的命令**就是**被执行的命令，且三根管道仍由通道接管。
    ///
    /// 判别力：若 `with_command` 忘了覆盖 stdio，`read_line` 会立刻拿到 EOF（子进程的
    /// stdout 没接到管道上）——而不是下面那行文本。
    #[test]
    fn with_command_runs_the_command_it_was_handed_and_pipes_its_stdio() {
        let (prog, args) = sh("echo hello-from-with-command");
        let mut cmd = Command::new(&prog);
        cmd.args(&args);

        let channel = ProcessChannel::with_command(cmd, None).expect("必须能起 sh");
        assert_eq!(
            channel.read_line().expect("读不该炸").as_deref(),
            Some("hello-from-with-command")
        );
        channel.close();
    }

    /// 起不来时的**根因**要说对：报出的路径必须是调用方交进来的那个程序，
    /// 而不是一个空的/伪造的名字（本项目对"根因不许说错"有纪律）。
    #[test]
    fn with_command_reports_the_program_it_was_handed_when_spawn_fails() {
        let missing = PathBuf::from("benagen-definitely-not-a-real-program-9f3a");
        match ProcessChannel::with_command(Command::new(&missing), None) {
            Ok(_) => panic!("一个不存在的程序居然起得来？"),
            Err(ClientError::Spawn { path, .. }) => {
                assert_eq!(path, missing, "报出的路径必须是交进来的那个程序")
            }
            Err(other) => panic!("根因说错了：{other}"),
        }
    }
}
