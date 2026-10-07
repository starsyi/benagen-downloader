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
//! ## ⚠️ 一次调用**有上界**（A5）：看门狗，不是读线程
//!
//! 上面第 2 条那个"读回配对的那一行"本来**没有上界**：`std::io` 在管道上**没有读超时**
//! （Windows 的匿名管道尤其没有），内核不回话，那个 `invoke` 就永久挂住 ——
//! 界面上的表现是"某一块一直转圈，点别的也没用"。
//!
//! 处置**不是**"把读搬进独立读线程 + `recv_timeout`"（那会是本模块唯一一处结构性改动，
//! 而读路径正是最不该在这一波里动的地方）。用的是一条**看门狗**：超时之后它调
//! `ProcessChannel::close()`，而 `close()` 会 kill 子进程 ⇒ 管道关掉 ⇒ **卡在
//! `read_line` 上的那一次立刻返回 EOF**。⇒ 读路径一行都不用碰。
//!
//! ⚠️ **超时之后这条通道判死**（[`CoreClient::call`] 从此快速失败，见那里的判断），
//! **不许**接着在这条管道上收发 —— 协议按请求号顺序配对，一条迟到的响应会被**下一条**
//! 请求当成自己的答案，**错配比报错更坏**。判死之后走的是既有的「内核没了」路径：
//! `KernelDeath::reason_of` 把 [`ClientError::CallTimedOut`] 与 [`ClientError::KernelGone`]
//! **同等对待**（界面上要做的动作是同一个：点「重试」）。
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
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
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
    /// 一次 [`CoreClient::call`] 在允许的时间内没等到配对的那条响应：**看门狗已经把这条
    /// 通道关掉、子进程也收了**（见 [`CoreClient::call_timeout_for`] 与 `spawn_watchdog`）。
    /// 这条通道**从此判死** —— 后续调用给 [`ClientError::Closed`]。
    ///
    /// ⚠️ 它与 [`ClientError::KernelGone`] **不是同一件事**：那是"内核进程真的没了"
    /// （管道结束，唯一可靠信号），这是"**我们**等够了"。**内核可能还活着**，
    /// 所以不许报成 `KernelGone` —— 报"内核进程已退出"是一句假话，
    /// 会把客户与支持一起引向错误的方向。
    /// 两者的**相同之处**只有一个：界面上要做的动作（点「重试」），
    /// 所以「内核没了」那条共享判定把它们同等对待（`presentation::kernel_death`）。
    ///
    /// ⚠️ `waited` 是**允许它等的上限**（这一次调用那一档的取值），不是"精确等了多久"：
    /// 实际耗时是它，加上看门狗一个节拍（[`WATCHDOG_TICK`]），加上收尾那条链的耗时。
    ///
    /// ## 🔴 这一档的**承诺有一个洞**，如实记在这里（别以为它不可能发生）
    ///
    /// 看门狗的超时**只会调 `ProcessChannel::close()`**，而 `close()` 在
    /// **kill 之后仍未回收**子进程时会**提前 return**（`client.rs` 的
    /// `ProcessChannel::close` 第 ② 步那段，那里记账了"可能变成孤儿进程"）。
    /// 那一刻**读端没有被放掉**（它走不到第 ④ 步）⇒ 卡在 `read_line` 上的那次调用
    /// **永远不会拿到 EOF** ⇒ 看门狗虽然已经落了 `timed_out`，那个 `invoke` **仍然挂住**。
    ///
    /// 也就是说：这一档的"有界"建立在**"kill 一定收得掉子进程"**之上。
    /// 真机上 `TerminateProcess` 几乎总能成功，所以这条路的可达性很低；
    /// 但它是**可达**的（同一个 `close()` 自己就写着"收不掉"那一支），
    /// 所以不许把 [`CoreClient::call`] 说成"保证有界"。
    CallTimedOut { method: String, waited: Duration },
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
    /// 这条通道已经收场，不再收新请求。两种来源，**都是终局**：
    /// [`CoreClient::shutdown`] 已调用，或者一次调用超时之后看门狗把通道关掉了
    /// （见 [`ClientError::CallTimedOut`]）。两种都不许再往这条管道上写一个字节。
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
            // ⚠️ 这句话里那句"请点「重试」"**是真的**：`CallTimedOut` 与 `KernelGone`
            //    在「内核没了」那条共享判定里同等对待（`presentation::kernel_death`），
            //    所以点「重试」那颗按钮真的会出现、也真的会重连。改这里之前先确认那一处 —
            //    本项目明令：补救只说界面上真的做得到的动作（`RequestTooLong` 那一句
            //    就是这么改过的）。
            ClientError::CallTimedOut { method, waited } => write!(
                f,
                "内核在 {waited:?} 内没有回应 {method}——这条通道已判死，请点「重试」"
            ),
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
            // ⚠️ 两种来源都要说出来（见变体的文档）：只写"shutdown 已调用"会在
            //    **超时判死**那条路上变成一句假话 —— 而客户会照着它去找一件没发生的事。
            ClientError::Closed => write!(
                f,
                "内核连接已关闭（已收尾，或者一次调用超时之后这条通道被判死了）"
            ),
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
        //    会让那次写**无限期**阻塞。收尾若在这里 `lock`，就会跟着卡死——而收尾是
        //    既有的**唯一**逃生口（A5 的看门狗那条路要等满**那一档**的上界 —— 150 或 600 秒，
        //    见 `CoreClient::call_timeout_for`；用户点「退出」等不了那么久），
        //    那样"最坏约 5 秒"就不真了。
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

/// 一次 [`CoreClient::call`] 在飞期间的时间戳守卫：**任何返回路径**（含 `?` 提前返回）
/// 都会把它清成 `None`（见 [`CoreClient::call`]）。
///
/// ⚠️ 用 `Drop` 而不是散在各处赋值，是因为"漏清一条路"的后果是**静默**的：
/// 看门狗会把一次**早就返回**的调用当成"还在等"，于是超时之后把一条**健康的**通道
/// 关掉 —— 而那只在那条路被走到（外加一次超时）时才发生。
/// `Drop` 由编译器保证每条出路都走，漏不掉。
struct InFlightGuard<'a> {
    slot: &'a Mutex<Option<InFlight>>,
}

/// 在飞中的那一次调用。
#[derive(Clone, Copy)]
struct InFlight {
    /// 这次调用是什么时候开始的。
    since: Instant,
    /// 这次调用的**有效上界**：[`CoreClient::call_timeout_for`] 按方法名查出来的，
    /// 或者测试用 [`CoreClient::with_call_timeout`] 注入的那个固定值。
    ///
    /// ⚠️ 它**随请求一起落在在飞格子里**（而不是让看门狗自己去查表）：
    /// 看门狗只认"这一次调用允许等多久"这一个事实，两处各算一次迟早会分叉。
    timeout: Duration,
}

impl<'a> InFlightGuard<'a> {
    /// 置上时间戳与该次的上界，交出守卫。守卫活多久，"在飞"就成立多久。
    fn arm(slot: &'a Mutex<Option<InFlight>>, timeout: Duration) -> InFlightGuard<'a> {
        *lock(slot) = Some(InFlight {
            since: Instant::now(),
            timeout,
        });
        InFlightGuard { slot }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        *lock(self.slot) = None;
    }
}

/// 看门狗**多久看一眼**（不是超时本身）。
///
/// 它是"多等一会儿"的界：一次调用实际被放行的时间最多是
/// 它那一档的上界 + 一个节拍 + 收尾那条链的耗时。取 50 ms 是因为它相对 150 秒的
/// 最短那一档可以忽略，而相对"判据的响应速度"又足够细。
const WATCHDOG_TICK: Duration = Duration::from_millis(50);

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
/// 诊断日志的出口。**生产恒为 [`crate::diagnostics::log_verbose`]**；测试注入一个记账替身。
///
/// ⚠️ 做成**可注入**而不是直接调那个函数，是为了让判据能在**不写文件、不翻全局**的前提下
///    钉住"每一次调用记一行、成功不带 `why`、失败带原文"。两个陷阱都是实测踩出来的
///    （内核那一侧的同位判据，任务 1 的任务审查 + 修复轮）：往真日志目录里写会污染
///    开发者自己那份日志（`log_verbose` 走 `storage::dir()`）；翻 `diagnostics::init`
///    的进程级静态会让同进程里假设 normal 的用例随机红。
///
/// 同形的先例（"决策与调用分开"）：`core/src/engine/rpc.rs` 的 `VerboseSink`
/// （那边是内核→aria2 的同一条判据）。
type VerboseSink = Arc<dyn Fn(&str, &[(&str, String)]) + Send + Sync>;

pub struct CoreClient {
    /// ⚠️ `Arc` 而不是 `Box`：看门狗线程要拿一份（它要在超时之后 `close()` 这条通道）。
    channel: Arc<dyn LineChannel>,
    inner: Mutex<ClientInner>,
    /// 单飞闸门。`call` 全程持有；`shutdown` 用 `try_lock` 有界地探它。
    gate: Mutex<()>,
    /// ⚠️ `Arc` 而不是裸的 `AtomicBool`：看门狗拿一份，用它判"通道已经没了，线程收工"。
    shutdown_started: Arc<AtomicBool>,
    /// **测试注入**的上界；`None` = 按方法名查表（生产路径）。
    ///
    /// ⚠️ 为什么是被 `Mutex` 包着的共享格子、而不是一个按值捕获的 `Duration`：
    ///    [`CoreClient::with_call_timeout`] 是在 `new` **之后**才调的，而看门狗那时候
    ///    已经起来了 —— 按值捕获的话，注入的时间**到不了**看门狗，测试就只能等满 150 秒
    ///    （而那看起来像"测试本来就慢"，不是"注入没生效"）。
    call_timeout: Arc<Mutex<Option<Duration>>>,
    /// 当前那次调用是什么时候开始的、它那一档的上界是多少（没人在飞时是 `None`）。
    /// `call` 里那个 [`InFlightGuard`] 维护它；看门狗读它。
    in_flight_since: Arc<Mutex<Option<InFlight>>>,
    /// 看门狗**已经判过一次超时**。⚠️ 它与 `in_flight_since` 是两个事实，别合并：
    /// 前者是"这条通道已经判死"（粘滞，`call` 与读的 EOF 分支都读它），
    /// 后者是"此刻有没有人在飞"（每次调用各自置、清）。
    timed_out: Arc<AtomicBool>,
    /// 详细档那一行的出口（见 [`VerboseSink`]）。生产恒为 `diagnostics::log_verbose`。
    verbose_sink: VerboseSink,
    /// **写进日志之前必须抹掉的字面串**（交付码、下载目录）—— 见
    /// [`CoreClient::set_redactions`] 与 `diagnostics::redact`。
    ///
    /// ⚠️ 它挂在**连接**上而不是挂在进程级静态上：翻静态会让同进程里别的用例
    ///    随线程调度随机红（那是 `VerboseSink` 那段文档记着的同一个坑）。
    ///    而挂在连接上还有一条**语义上**的好处：**换一条连接 = 换一份该抹的东西**
    ///    （新内核是壳带着新偏好起出来的，旧的那些串已经不在任何一条在飞的请求里）。
    redactions: Arc<Mutex<Vec<String>>>,
}

impl CoreClient {
    /// 接一条现成的通道（测试替身走这里）。
    ///
    /// ⚠️ 看门狗线程在这里就起来（随 client 一起活）。起不来的话**大声 panic**：
    /// 本函数没有错误通道（`Box<dyn LineChannel>` 交出去就收不回来了，改签名会动
    /// 所有调用点），而"没有看门狗也照跑"= 悄悄把 F7 那个永久挂住放回来 ——
    /// 本仓对静默降级的口径是"宁可大声失败"。线程起不来只可能是进程资源耗尽。
    pub fn new(channel: Box<dyn LineChannel>) -> Self {
        let client = CoreClient {
            channel: Arc::from(channel),
            inner: Mutex::new(ClientInner {
                next_id: 1,
                closed: false,
                alerts: ProtocolAlerts::default(),
            }),
            gate: Mutex::new(()),
            shutdown_started: Arc::new(AtomicBool::new(false)),
            call_timeout: Arc::new(Mutex::new(None)),
            in_flight_since: Arc::new(Mutex::new(None)),
            timed_out: Arc::new(AtomicBool::new(false)),
            // 生产路径**恒为**它；`mod tests` 才会换成记账替身（见 [`VerboseSink`]）。
            verbose_sink: Arc::new(crate::diagnostics::log_verbose),
            // 起手**没有可抹的串**：壳还没告诉过我们它此刻在处理哪个交付码、下到哪个目录。
            // ⚠️ 这不是"永远抹不掉"——`set_redactions` 的调用点在**发请求之前**
            //    （`shell-win/src/session.rs` 的 `remember_for_redaction`）。
            redactions: Arc::new(Mutex::new(Vec::new())),
        };
        client.spawn_watchdog();
        client
    }

    /// 看门狗：一次调用在飞超过 `call_timeout` 就**把通道关掉**。
    ///
    /// ⚠️ 顺序是承重的：**先落 `timed_out` 这个事实，再关通道**。卡住的那次读会在
    /// `close()` 之后返回 EOF，它要靠这个标志才知道"这不是内核退出，是我们等超时了"。
    /// 反过来写的话，`call` 会把它报成 `KernelGone`——**而内核可能活着**，
    /// 报"内核进程已退出"是一句假话，会把客户与支持一起引向错误的方向。
    ///
    /// ⚠️ 关通道走的是既有的 `ProcessChannel::close()`：它**会 kill 子进程**，而 kill
    /// 会关掉管道 ⇒ 卡在 `read_line` 上的那一次立刻返回 EOF。**这一条不是推断**：
    /// 本模块的 `the_watchdog_unblocks_a_real_blocking_read_on_a_real_pipe` 用一条
    /// 真子进程 + 真管道踩住了它（替身配合得起来不算数）。
    ///
    /// ⚠️ 超时之后**不再收发**（判死，见 [`CoreClient::call`] 里的判断）：协议按请求号
    /// 顺序配对，一条迟到的响应会被**下一条**请求当成自己的答案。
    fn spawn_watchdog(&self) {
        let in_flight = Arc::clone(&self.in_flight_since);
        let timed_out = Arc::clone(&self.timed_out);
        let shutdown_started = Arc::clone(&self.shutdown_started);
        let channel = Arc::clone(&self.channel);
        std::thread::Builder::new()
            .name("shell-call-watchdog".to_string())
            .spawn(move || loop {
                // 判据粒度，不是超时本身 —— 50 ms 的量级让"多等一会儿"有界。
                std::thread::sleep(WATCHDOG_TICK);
                if shutdown_started.load(Ordering::SeqCst) {
                    return; // 通道已经没了（`shutdown` / `Drop` 收的尾），线程收工
                }
                let current = *lock(&in_flight);
                let Some(current) = current else { continue };
                // ⚠️ 比的是**这一次调用自己那一档**的上界（随请求落在格子里），不是某个全局值。
                if current.since.elapsed() < current.timeout {
                    continue;
                }
                timed_out.store(true, Ordering::SeqCst);
                channel.close(); // kill 子进程 ⇒ 卡住的那次读立刻 EOF
                return;
            })
            .expect("起看门狗线程失败（没有它，内核不回话时 call 会永久挂住）");
    }

    /// **普通**调用的上界（生产路径；测试用 [`CoreClient::with_call_timeout`] 调小）。
    ///
    /// 只覆盖**不在** [`Self::LONG_CALL_METHODS`] 表里的方法。常见调用都在秒级
    /// （一次 aria2 RPC 的传输上限是内核侧的 `RPC_TIMEOUT` ＝ 10 秒，`core/src/engine/rpc.rs:17`），
    /// 150 秒是**一个数量级**以上的余量。
    ///
    /// ⚠️ **别拿它去衡量长调用**：`load_delivery` 这一家族的记账见
    /// [`Self::LONG_CALL_TIMEOUT`] —— 那个数**不是** 91.5 秒（91.5 只是它三段里的第一段）。
    /// 这一条在 A5 的第一版里写错过（把 91.5 当成了 `load_delivery` 的最坏值），
    /// 于是"网络慢 + 换码 + 清单大"的一次**合法**慢调用会被判死**并杀掉内核** ——
    /// 那正是规格 §3 A5 的取值段要防的那件事。两档就是这么来的。
    pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(150);

    /// **已知的长调用家族**的上界。
    ///
    /// 名单（[`Self::LONG_CALL_METHODS`]）取的是**内核侧不用 `with_kernel` 包起来的那几个
    /// RPC 方法**（`core/src/main.rs:882-887` 的分派表：`load_delivery` / `list_dir` /
    /// `get_tree` / `plan` / `enqueue`），它们的共同点是最贵的部分都是网络 I/O。
    ///
    /// ## 取值 600 秒的依据（逐条可核）
    ///
    /// 内核自己对 `load_delivery` 的记账在 `core/src/main.rs:993-998`，三段**顺序执行**：
    ///   * `delivery::fetch`：30 秒 × 3 次 + 1.5 秒退避 = 最坏 **91.5 秒**；
    ///   * `clear_engine_batch`：最坏约 **100 秒**；
    ///   * 全量 `plan`：每个 crc64 为空的文件一次 HEAD，最坏 **30 秒 × N**
    ///     （单次 30 秒：`core/src/delivery.rs:22` 的 `FETCH_TIMEOUT`）。
    ///
    /// 三段是**顺序**跑的（那一段注释写的就是这个），所以算第三段时前两段已经花掉了：
    /// `600 − 191.5 = 408.5` 秒，按 30 秒一次折算 ⇒ **N ≈ 13**。
    /// （不是 600 ÷ 30 ＝ 20 —— 那个算法把三段当成并行的了。）
    ///
    /// ⚠️ **余量就到这里为止，别把它读成"600 秒一定够"**：`plan` 是 **30 秒 × N、
    /// 内核里没有全局 deadline**（`core/src/planner.rs:186` 的记账）⇒ **N 很大时任何常数
    /// 都盖不住**。那时壳会按本档判死、`close()` ⇒ **杀掉内核**。这是 `plan` 缺全局
    /// deadline 的直接后果，是**既有问题**（本批不动它，也不假装本档把它解决了）。
    /// 一个"网络慢 + 换交付码 + 清单里几千个 crc64 为空"的批次会走到那里。
    ///
    /// ⚠️ 改这个数字前先读 [`ClientError::CallTimedOut`] 与 `spawn_watchdog`：
    /// 它同时是"判死"的阈值（超时之后这条通道不再收发）。
    pub const LONG_CALL_TIMEOUT: Duration = Duration::from_secs(600);

    /// 走 [`Self::LONG_CALL_TIMEOUT`] 的方法名。**表外的走 [`Self::DEFAULT_CALL_TIMEOUT`]。**
    ///
    /// ⚠️ **按方法名查表，不是按"调用的地方"**：`call` 只拿得到方法名，而"这次会不会慢"
    ///    完全由**内核那一条 RPC 的实现**决定（见 `LONG_CALL_TIMEOUT` 的记账）。
    ///    写在调用点会把同一个方法的两种预算散到各处。
    ///
    /// ⚠️ 名单与内核分派表**必须一起改**：内核多一个"长方法"而这里漏了，那个方法就会
    ///    被按 150 秒判死、**并杀掉内核**（比"没超时"更坏）。
    ///
    /// 🔴 **但"必须一起改"这件事今天没有任何自动判据守着**（终审点名，如实记账）：
    ///    `the_long_method_table_matches_the_hand_copied_list`
    ///    比的是**下面这份名单与测试里手抄的同一份名字**（本 crate 读不到内核源码），
    ///    **内核从来没被读过**。⇒ **内核新增第六个"锁外跑网络 I/O"的方法时，那条用例
    ///    不会红**。这是已知缺口，不是"已经守住了"。
    ///    ⚠️ 另外，那条用例**没有集合相等断言**（往表里塞一个两份手抄名单里都没有的名字，它不会红）。
    ///    （下一批要么 `include_str!("../../../core/src/main.rs")` 真把分派表读进来比，
    ///    要么想别的办法；本批刻意不做 —— 那是另一件事的规模。）
    ///
    /// ⚠️ `tree_json` **不在**这张表里，因为它**不是**一个 RPC 方法名：它是内核里的一个
    ///    私有函数（`core/src/main.rs:1097`），由 `op_list_dir` / `op_get_tree` 调用
    ///    （`core/src/main.rs:20` 那句"被 `list_dir`/`get_tree`/`enqueue`/`tree_json`
    ///    的路径调用"说的是**代码路径**）。写进来只会是一条**永远匹配不到**的假条目。
    /// ⚠️ `plan` **在**表里但**壳今天不发它**（`grep -rn '\.call("' windows/` 没有它）：
    ///    它是一个真方法、而且正是本档取值理由里的第三段（30 秒 × N）挂在的那个方法，
    ///    所以留着 —— 这是一条**保险**，不是一条活路径。
    pub const LONG_CALL_METHODS: &'static [&'static str] = &[
        "load_delivery",
        "list_dir",
        "get_tree",
        "plan",
        "enqueue",
    ];

    /// 这个方法该用哪一档上界。**唯一**的查表处。
    pub fn call_timeout_for(method: &str) -> Duration {
        if Self::LONG_CALL_METHODS.contains(&method) {
            Self::LONG_CALL_TIMEOUT
        } else {
            Self::DEFAULT_CALL_TIMEOUT
        }
    }

    /// 换一个超时值。**只给测试注入**：生产路径一律走 [`Self::call_timeout_for`] 的查表。
    ///
    /// ⚠️ 它必须存在：真实取值是 150 / 600 秒，而本任务的判据全都以"耗时"为判据 ——
    /// 逐条等 600 秒的测试不会有人跑，而**没人跑的判据与没有判据等价**（本仓的 W-2）。
    /// ⚠️ 注入的是**一个固定值，对所有方法生效**（查表被它盖过去）。
    /// ⚠️ 它写的是那个**共享格子**，所以调用的时机不影响生效 —— 见 `call_timeout` 字段的文档。
    pub fn with_call_timeout(self, t: Duration) -> Self {
        *lock(&self.call_timeout) = Some(t);
        self
    }

    /// 这一次调用该等多久：测试注入优先，否则按方法名查表。
    fn effective_call_timeout(&self, method: &str) -> Duration {
        match *lock(&self.call_timeout) {
            Some(injected) => injected,
            None => Self::call_timeout_for(method),
        }
    }

    /// 一条通道错误该报成什么：**判死只有一个出口**。
    ///
    /// 看门狗一旦判过超时，这条通道就**已经永久死了**（后续每次调用都是
    /// [`ClientError::Closed`]）—— 所以此刻无论拿到的是 EOF（[`ClientError::KernelGone`]）、
    /// 写失败还是读失败，对客户来说都是**同一件事**：走「内核没了」那条路（横幅 + 「重试」）。
    ///
    /// ⚠️ 少了这一层，"判死"就会被报成 [`ClientError::WriteFailed`] / [`ClientError::ReadFailed`]
    ///    —— 它们**不在** `KernelDeath::reason_of` 认的那一档里 ⇒ 落 `last_error` ⇒
    ///    **瞬时横幅、没有「重试」**，而通道其实再也不会好。
    ///    ⚠️ 写那一路是**可达**的：内核僵住（不再读 stdin）+ 请求体大于管道缓冲区
    ///    （大批 `enqueue` 很容易）⇒ 那次写卡住 ⇒ 看门狗 `close()` ⇒ 写拿到 EPIPE。
    fn death_or(&self, method: &str, timeout: Duration, error: ClientError) -> ClientError {
        if self.timed_out.load(Ordering::SeqCst) {
            ClientError::CallTimedOut {
                method: method.to_string(),
                waited: timeout,
            }
        } else {
            error
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
    /// 这条是给界面线程用的：一条跑满那一档（最长 600 秒）的 `load_delivery`
    /// 不该把"显示一条告警"也拖住。
    pub fn protocol_alerts(&self) -> ProtocolAlerts {
        lock(&self.inner).alerts.clone()
    }

    /// 同步发一条请求，返回它的 `result`（`ok:false` 变成 [`ClientError::Kernel`]）。
    ///
    /// 返回的是**原始 [`Value`]**，不在这里做任何强类型解码：要不要强类型是调用方的事
    /// （少一次"编解码往返"就少一次 `.integer`/`.number` 被抹平的机会）。
    ///
    /// ⚠️ **本函数是"壳→内核"的唯一收口**：详细档那一行（`kernel_call`）记在**这里**，
    ///    于是所有调用点自动全覆盖——散在调用点各记一遍会漏掉下一个人新加的那条路。
    ///    真正的实现是 `Self::call_inner`（本函数只负责计时与记账）。
    ///
    /// ⚠️ **"记不记"由调用点决定，本函数无条件记**：闸门在
    ///    [`crate::diagnostics::log_verbose`]（只有详细档才真落盘）。所以判据可以
    ///    往 `VerboseSink` 注入一个替身来数行数，**不必**碰文件系统、也不必翻
    ///    进程级静态。
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let started = Instant::now();
        let outcome = self.call_inner(method, params);
        let mut fields = vec![
            ("method", method.to_string()),
            ("ms", started.elapsed().as_millis().to_string()),
            ("ok", outcome.is_ok().to_string()),
        ];
        // ⚠️ **只有失败时才带 `why`**：成功时补一个空字段会让"这一行有几个字段"
        //    随结果变，而按空格切字段读它的下一个人会读到空值。
        //    `why` 走的是壳既有的那个只读访问器（`presentation::error_text`）——
        //    与界面上给用户看的那句**同一份**，不是这里另写一句。
        //    它与内核 `RpcClient::send` 那一行**同形**（`method`/`ms`/`ok`/失败时 `why`）
        //    —— 客户回传时两个文件要能对着读。
        //
        // 🔴 **落进这一行之前必须过 [`CoreClient::redact`]**（规格 §2.3 B）：`why` 是
        //    **内核原文**，而交付码是**我们自己**拼进内核文案里的
        //    （`core/src/delivery.rs` 那条 404 把 `…/{交付码}/manifest.json` 整条 URL
        //    送了进来，`core/src/main.rs` 的 `preflight` 那一档带着客户目录名）。
        //    ⇒ 这一处**只抹日志**：给用户看的那句话一个字都不动（界面上显示客户自己的
        //    交付码/目录是应当的，问题只出在"要离开这台机器的那一份"上）。
        if let Err(error) = &outcome {
            fields.push(("why", self.redact(&crate::presentation::error_text::error_text(error))));
        }
        (self.verbose_sink)("kernel_call", &fields);
        outcome
    }

    /// 告诉这条连接：**这几个字面串写进日志之前必须抹掉**（交付码、下载目录）。
    ///
    /// 🔴 **它存在的全部理由是隐私，而且这次是"按构造"那一档**（规格 §2.3 B）：
    ///    `why` 里出现交付码的字符串**是我们自己拼的**（不是第三方文案），
    ///    而壳**知道**那个码（就在它刚发出去的请求里）与那个目录（就在它自己存的偏好里）
    ///    ⇒ 能按构造避开，就必须避开。
    ///
    /// ⚠️ **推下来的是"此刻该抹的整份清单"，不是追加**：交付码会换（用户换了批次），
    ///    下载目录会换。追加式的接口会把上一个批次的码永久留在里面，
    ///    而那种残留**看不出来**（它只是偶尔多抹掉一段无害的文字）。
    /// ⚠️ **调用点是"发请求之前"**（`shell-win/src/session.rs` 的 `remember_for_redaction`，
    ///    在 `spawn_load` 与 `spawn_connect` 两处）—— 晚一步的表现是**那一次**的日志里
    ///    带着码，而那一次恰恰是最可能失败、最需要那份日志的一次。
    /// ⚠️ **它不是一个过滤器**：只做字面子串替换，做不到的事见 `diagnostics::redact` 的文档。
    pub fn set_redactions(&self, secrets: Vec<String>) {
        *lock(&self.redactions) = secrets;
    }

    /// 把 [`CoreClient::set_redactions`] 那一份清单套到一段文字上。
    fn redact(&self, value: &str) -> String {
        // ⚠️ 先把清单克隆出来再抹：临界区里只有一个 `clone`（`redact` 是纯计算，
        //    而它可能走过 200 个字符 —— 不需要占着这把锁做）。
        let secrets = lock(&self.redactions).clone();
        crate::diagnostics::redact(value, &secrets)
    }

    /// `call` 的本体：**只做事、不记账**（计时与详细档那一行在 [`Self::call`] 上）。
    fn call_inner(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        // 单飞：整条请求-响应循环都在闸门里。它同时保证了"同一个内核上不会有两条在飞的
        // 请求"，于是下面那个 `id` 配对**不可能**撞上别人的响应（撞上就是失步，要吵）。
        let _gate = lock(&self.gate);

        // 🔴 一次调用超时之后这条通道**已经判死**（看门狗关掉了它）：与 `shutdown` 之后
        //    走同一条**快速失败**的路 —— **绝不再往这条管道上写一个字节**。
        //    承重的理由：协议按请求号顺序配对，一条迟到的响应会被**下一条**请求当成
        //    自己的答案 —— **错配比报错更坏**。所以这里不是"再等一次超时"，是立刻失败。
        if self.timed_out.load(Ordering::SeqCst) {
            return Err(ClientError::Closed);
        }

        // 这一次调用允许等多久：**按方法名查表**（长调用家族另有上界，见
        // `LONG_CALL_TIMEOUT` 的记账），测试注入的值盖过查表。
        let timeout = self.effective_call_timeout(method);

        // 在飞计时：从这一句起，本函数**任何**一条出路（含下面那些 `?`）都会清掉它
        // —— `Drop` 保证，见 `InFlightGuard`。上界随请求一起落格（看门狗只认这一个事实）。
        //
        // 🔴 **这一句必须在上面那句 `let _gate = lock(&self.gate);` 之后**（设计不变量：
        //    **排队的时间不计入预算**）。
        //    挪到闸门**之前**的后果是承重的、而且**判据全绿**：一次排在内核里的请求
        //    （前面压着一条最长 600 秒的 `load_delivery`）会从**自己被写出去之前**就开始计时，
        //    于是它可能在**自己那一档的 150 秒**被判死 ⇒ `close()` ⇒ **杀掉一个健康的内核**
        //    —— 正是规格 §3 A5 的取值段要防的那件事。
        //    ⚠️ 下面这些用例**抓不到**这种挪动：套件里没有哪两条 `call` 是**重叠**的
        //    （几处 `thread::scope` 都是"调用 vs 逃生口"，不是两条调用），而 `InFlightGuard`
        //    的 `Drop` 语义与它被 arm 的位置无关 ⇒ 把这一行挪上去，全绿。
        //    ⇒ 所以这条不变量靠**这一行注释**与 `transfer_list` 那条排队路径的语义守着，
        //    改这里之前先读 `CoreClient` 上"同一时刻至多一条在飞的请求"那段。
        let _in_flight = InFlightGuard::arm(&self.in_flight_since, timeout);

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
        // 写失败也过 [`Self::death_or`]：卡在**写**上的那次调用会被看门狗判死
        // （内核僵住、连 stdin 都不读，而请求体大于管道缓冲区），那时写拿到的是 EPIPE
        // —— 把它当传输层错误报出去，客户看到的会是一句**没有「重试」**的提示，
        // 而通道其实已经永久死了。
        if let Err(e) = self.channel.write_line(&line) {
            return Err(self.death_or(method, timeout, e));
        }

        loop {
            // 读的三条结局都过 [`Self::death_or`]：**判死只有一个出口**。
            // （EOF 那一条的判据是看门狗有没有动过手，见 `spawn_watchdog` 那段"顺序是承重的"：
            //  超时 ⇒ 通道是**我们**关的、内核可能还活着；否则 ⇒ 内核进程真没了。
            //  报错要说对根因：把"我们等够了"说成"内核进程已退出"会把客户与支持引向错的方向。）
            let reply = match self.channel.read_line() {
                Ok(Some(reply)) => reply,
                Ok(None) => {
                    return Err(self.death_or(method, timeout, ClientError::KernelGone));
                }
                Err(e) => return Err(self.death_or(method, timeout, e)),
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
    /// ⚠️ **也不能无界地等闸门**：内核正在跑长请求（`load_delivery` 那一档的上界是
    /// [`Self::LONG_CALL_TIMEOUT`] ＝ 600 秒，见那里的记账）时，
    /// 用户点退出，那条请求正卡在 `read_line` 上持着闸门——而能让它松开的
    /// `channel.close()` 就在等闸门之后（A5 的看门狗也会关它，但要等满**那一档**的上界
    /// —— 150 或 600 秒，见 `call_timeout_for`；退出等不了那么久）：那就成了"点了没反应"
    /// （约束 4）。所以：
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
///
/// ## 🔴 `verbose_logging`：**开了才拼那一对，关了什么都不拼**
///
/// 关着的时候**不许**拼 `--log-level normal`：那一对与"这个 flag 根本不出现"在内核侧
/// 是**同一件事**（`parse_args` 的 `log_level.unwrap_or(Level::Normal)`），拼出来只是给
/// 将来的自己多一处会漂的地方（内核改了默认档，壳还在显式地要 normal）。
/// 这与 `--download-dir` 那条 E-5 是同一个道理（未配置就别替内核写死一个默认值）。
///
/// ⚠️ **它是一个显式参数、不给默认值**：默认值会让"调用点忘了把它接上"退化成
/// 静默的 normal —— 而"客户开了详细日志、导出的却是一份普通档的"正是这条功能
/// 最怕的那种"没有任何东西会变红"。少一个实参就编不过，那才是要的。
pub fn core_arguments(
    download_dir: Option<&Path>,
    settings_path: Option<&Path>,
    verbose_logging: bool,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(d) = download_dir {
        args.push("--download-dir".to_string());
        args.push(d.to_string_lossy().into_owned());
    }
    if let Some(s) = settings_path {
        args.push("--settings".to_string());
        args.push(s.to_string_lossy().into_owned());
    }
    // ⚠️ 排在最后：与 macOS 那侧 `coreArguments(settingsPath:downloadDir:verboseLogging:)`
    //    的拼接次序逐个一致（内核的 `parse_args` 认顺序无关，但两边"什么时候有哪几格"
    //    要对得上，不然并排读两份 argv 时像是壳少拼了一个）。
    if verbose_logging {
        args.push("--log-level".to_string());
        args.push("verbose".to_string());
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
    use std::sync::{Arc, Condvar, Mutex};
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

    /// **会真阻塞**的通道：`read_line` 一直等到 `close()` 才返回 EOF。
    ///
    /// ⚠️ 既有的 `StubChannel` 造不出这个场景：它脚本用完之后**立刻**返回 `Ok(None)`，
    /// 而"内核活着但不回话"恰恰是**什么都不返回**。
    /// 用 `Condvar` 而不是 sleep 轮询：轮询会让"等了多久"变成不确定的量，
    /// 而本任务的判据正是耗时。
    ///
    /// ⚠️ 状态挂在 `Arc` 后面、替身**可 `Clone`**（与 `StubChannel` 同形）：用例要
    /// **在把它交给 `CoreClient` 之后**还能查到 `close()` 被调过几次 ——
    /// 一个在移交之前抄下来的计数快照会永远是 0，那条断言就成了"从构造上不可能红"。
    #[derive(Clone)]
    struct SilentChannel {
        gate: Arc<(Mutex<bool>, Condvar)>,
        closes: Arc<AtomicUsize>,
        /// `true` ⇒ **连写都阻塞**：`write_line` 也等到 `close()` 才收场。
        ///
        /// 它造的是"内核僵住（连 stdin 都不读）+ 请求体大于管道缓冲区"那一路 ——
        /// 那时卡住的是**写**，而判死之后写拿到的是 EPIPE（见 `CoreClient::death_or`）。
        block_write: bool,
    }

    impl SilentChannel {
        fn new() -> Self {
            Self::with(Default::default())
        }
        /// 连**写**都阻塞的替身（见 `block_write` 那段）。
        fn with_blocking_write() -> Self {
            Self::with(true)
        }
        fn with(block_write: bool) -> Self {
            Self {
                gate: Arc::new((Mutex::new(false), Condvar::new())),
                closes: Arc::new(AtomicUsize::new(0)),
                block_write,
            }
        }
        fn closes(&self) -> usize {
            self.closes.load(Ordering::SeqCst)
        }
        /// 等到 `close()`（或者已经关过）为止。`read_line` 与 `write_line` 共用。
        fn wait_for_close(&self) {
            let (lock, cv) = &*self.gate;
            let mut closed = lock.lock().unwrap();
            while !*closed {
                closed = cv.wait(closed).unwrap();
            }
        }
    }

    impl LineChannel for SilentChannel {
        fn write_line(&self, _line: &str) -> Result<(), ClientError> {
            if !self.block_write {
                return Ok(()); // 写得进去——问题出在对方不回话
            }
            self.wait_for_close();
            // `close()` 之后 = 子进程被杀 ⇒ 这一写拿到 EPIPE（`ProcessChannel` 同形）。
            Err(ClientError::WriteFailed {
                cause: std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "内核僵住之后被杀，这一写拿到 EPIPE",
                ),
            })
        }
        fn read_line(&self) -> Result<Option<String>, ClientError> {
            self.wait_for_close();
            Ok(None) // `close()` 之后 = 管道断了
        }
        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
            let (lock, cv) = &*self.gate;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
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
    // 每请求上界（A5）：内核**活着但不回话**
    // ------------------------------------------------------------------

    /// 跑一次 `call` 并**带期限**地等它：超过 `limit` 就先把 `escape` 收掉（把卡住的那次读
    /// 放出来），然后 panic。
    ///
    /// ⚠️ 为什么需要它：`SilentChannel` 的读**永远不返回**，所以"看门狗没生效"的表现是
    /// **本用例挂住** —— 而挂住不是判据（它毒掉整个套件、还没有任何诊断，
    /// 与"这条判据从构造上不可能红"是同一类问题）。有了期限，同样的改法变成**红**。
    /// 这也是本仓既有的做法（`stderr_is_drained_so_the_child_never_blocks_on_write`）。
    fn call_within(
        client: &CoreClient,
        escape: &SilentChannel,
        limit: Duration,
    ) -> (Result<Value, ClientError>, Duration) {
        let started = Instant::now();
        std::thread::scope(|s| {
            let call = s.spawn(|| client.call("get_state", Value::Null));
            while !call.is_finished() {
                if started.elapsed() > limit {
                    escape.close(); // 把卡住的那次读放出来，否则它会把本用例挂在这里
                    let _ = call.join();
                    panic!("{limit:?} 内没返回：看门狗没生效，call 在管道上永久挂住了");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let took = started.elapsed();
            (call.join().expect("调用线程不该 panic"), took)
        })
    }

    /// **内核不回话时，`call` 必须有界地失败**（规格 §3 A5）。
    ///
    /// 判别力：把看门狗去掉、或让它不落 `timed_out`，这一条红（见上面 `call_within`
    /// 那段"挂住不是判据"）。
    #[test]
    fn call_fails_bounded_when_the_kernel_never_answers() {
        let channel = SilentChannel::new();
        // 注入一个**测试用**的超时：生产默认 150 秒，逐条等不起。
        let client = CoreClient::new(Box::new(channel.clone()))
            .with_call_timeout(Duration::from_millis(300));

        let (outcome, took) = call_within(&client, &channel, Duration::from_secs(5));
        let e = outcome.expect_err("不回话必须有界地失败，而不是永久挂住");

        match e {
            ClientError::CallTimedOut { ref method, .. } => assert_eq!(method, "get_state"),
            other => panic!("期望 CallTimedOut，得到 {other:?}"),
        }
        assert!(took < Duration::from_secs(5), "等了 {took:?}，超时没生效");
    }

    /// 🔴 **超时之后这条通道必须判死**（规格 §3 A5）——不是"接着用"。
    ///
    /// 判据是"后续调用**立刻**失败"：迟到的那条响应**绝不能**被下一条请求当成自己的答案。
    #[test]
    fn a_timed_out_channel_is_dead_for_good() {
        let channel = SilentChannel::new();
        let client = CoreClient::new(Box::new(channel.clone()))
            .with_call_timeout(Duration::from_millis(300));
        let (first, _) = call_within(&client, &channel, Duration::from_secs(5));
        assert!(matches!(first, Err(ClientError::CallTimedOut { .. })));

        // 看门狗必须真的把通道关掉（在 Windows 上这一步会 kill 子进程）。
        // ⚠️ 现查，**不抄快照**（见 `a_normal_call_is_untouched_by_the_watchdog` 里那段）。
        assert!(channel.closes() >= 1, "超时之后通道没有被关掉");

        // 后续调用**立刻**失败，绝不再等一次完整超时。
        let started = Instant::now();
        let e = client.call("get_state", Value::Null).expect_err("通道已判死");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "判死之后必须立刻失败，实际等了 {:?}",
            started.elapsed()
        );
        assert!(matches!(e, ClientError::Closed), "期望 Closed，得到 {e:?}");
    }

    /// 两档上界都必须**大于各自那一族最慢的合法调用**，否则会把"慢"判成"错"。
    ///
    /// 🔴 **本用例的模型在 A5 修复轮 3 改过一次，理由留在下面**：第一版拿
    /// **91.5 秒**当"最慢的合法调用"，而那个数只是 `load_delivery` **三段里的第一段**
    /// （`core/src/main.rs:993-998`：fetch 三段里的 fetch）。按那个模型，
    /// "网络慢 + 换交付码 + 清单大"的一次**合法**慢调用会被判死 —— 而判死的代价是
    /// `close()` ⇒ **杀掉内核**，比修复前的"一直等"更坏。所以模型必须**按段相加**。
    #[test]
    fn the_call_timeouts_are_above_the_slowest_legitimate_call_of_their_family() {
        // ── 长调用那一档：内核自己给的三段记账（`core/src/main.rs:993-998`）──────────
        // 前两段**有界**，而且**顺序执行**：
        const FETCH_WORST: Duration = Duration::from_millis(91_500); // 30 s × 3 + 1.5 s 退避
        const CLEAR_WORST: Duration = Duration::from_secs(100); // 开头快照 + 3×2×10 + 收尾快照
        assert!(
            CoreClient::LONG_CALL_TIMEOUT > FETCH_WORST + CLEAR_WORST,
            "长调用那一档 {:?} 不大于两段有界之和 {:?}——那会把「慢」判成「错」，\
             而判死的代价是**杀掉内核**",
            CoreClient::LONG_CALL_TIMEOUT,
            FETCH_WORST + CLEAR_WORST
        );
        // 第三段（全量 `plan`）是 **30 秒 × N、内核里没有全局 deadline**：任何常数都盖不住
        // N 很大时的它。三段**顺序**跑 ⇒ 留给第三段的预算是 600 − 191.5 = 408.5 秒，
        // 按 30 秒一次折算 ⇒ N ≈ 13（也就是下面这条断言的 30×13 = 390 秒）。
        // 把这个**下界**钉住，免得有人把 600 当成"随手取的大数"再调小。
        // ⚠️ 那个"盖不住"的缺口写在 `LONG_CALL_TIMEOUT` 的文档里（既有问题，本批不动）。
        assert!(
            CoreClient::LONG_CALL_TIMEOUT >= Duration::from_secs(30) * 13,
            "长调用那一档 {:?} 盖不到 N = 13 个 crc64 为空的文件（每个一次 30 秒 HEAD）",
            CoreClient::LONG_CALL_TIMEOUT
        );

        // ── 普通那一档：最贵的一条是单次 aria2 RPC（内核侧 `RPC_TIMEOUT` ＝ 10 秒，
        //    `core/src/engine/rpc.rs:17`；A1 之后读方法最坏再重试一次）⇒ ≈ 20 秒。
        //    150 秒给的是一个数量级以上的余量。
        const SLOWEST_ORDINARY_CALL: Duration = Duration::from_secs(20);
        assert!(
            CoreClient::DEFAULT_CALL_TIMEOUT > SLOWEST_ORDINARY_CALL,
            "默认超时 {:?} 不大于普通调用最慢的一条 {:?}——那会把「慢」判成「错」",
            CoreClient::DEFAULT_CALL_TIMEOUT,
            SLOWEST_ORDINARY_CALL
        );

        // 两档不许倒挂（倒挂的话"长调用"会拿到更短的上界）。
        assert!(
            CoreClient::LONG_CALL_TIMEOUT > CoreClient::DEFAULT_CALL_TIMEOUT,
            "两档倒挂了：长调用那一档必须更宽"
        );
    }

    /// 🔴 **查表本身要有牙**：长家族的方法走 `LONG_CALL_TIMEOUT`，表外的走
    /// `DEFAULT_CALL_TIMEOUT`；下面这份名单逐字列的是**应该**在表里的那些方法。
    ///
    /// ⚠️⚠️ **这条用例守的是什么、不守什么（终审点名，别读大了）**：
    ///    * **守**：就这三件 —— `call_timeout_for` 对**下面手抄的这五个长名字**都返回
    ///      `LONG_CALL_TIMEOUT`、对**另外五个短名字**都返回 `DEFAULT_CALL_TIMEOUT`、
    ///      且 `tree_json` **不在** `LONG_CALL_METHODS` 里。表被误改、这五个里的某一个
    ///      被挪出表、这五个短名字里的某一个被塞进表，都会红；
    ///    * **不守（集合相等）**：**没有"表与手抄名单集合相等"这条断言**，所以往表里塞一个
    ///      **两份手抄名单里都没有**的名字（例：`push("get_progress")`）**两条循环都不红**，
    ///      而那个方法此后会拿到 600 秒那一档。本条钉的只是"这十个名字各自归哪一档
    ///      ＋ `tree_json` 不在表里"；
    ///    * **也不守（两档倒挂）**：`LONG_CALL_TIMEOUT <= DEFAULT_CALL_TIMEOUT` 是
    ///      `the_call_timeouts_are_above_the_slowest_legitimate_call_of_their_family`
    ///      末尾那句断言的事，**不在本条**；
    ///    * **不守**：这份手抄名单**与内核分派表**是否一致。本 crate 读的是内核源码吗？
    ///      **不是** —— 内核从来没被读过，而这正是缺口：**内核新增第六个"锁外跑网络 I/O"
    ///      的方法时，这条用例不会红**，而那个方法会被按 150 秒判死、并杀掉内核。
    ///      ⇒ 这一条是**已知缺口**，已同步记账在 `LONG_CALL_METHODS` 的文档里；
    ///      真要守住得让本 crate 去读内核源码（`include_str!` 那条路，另一件事的规模）。
    ///
    /// 判别力（实测过，见报告里的变异读数）：挪出去、或把某一档调小 ⇒ 数值断言红
    /// （后者红在 `the_call_timeouts_are_above_the_slowest_legitimate_call_of_their_family`
    /// 的阈值断言上，不在本条）；**塞进来只有在那个名字同时出现在下面那份手抄名单里时
    /// 才会红** —— 名单外的新名字两条循环都不红（见上面「守什么/不守什么」）。
    #[test]
    fn the_long_method_table_matches_the_hand_copied_list() {
        // ⚠️ **这是手抄的一份副本**（不是从内核读来的）：内核那五个"锁外跑网络 I/O"的方法
        //    写在 `core/src/main.rs:882-887` 的分派表里，抄过来只是为了让"表被改坏"能红。
        //    两边**不会自动同步** —— 见上面那段"守什么、不守什么"。
        // ⚠️ `tree_json` **不在这里**：它是内核的私有函数（`core/src/main.rs:1097`），
        //    不是 RPC 方法名（表里写它 = 一条永远匹配不到的假条目）。
        for method in ["load_delivery", "list_dir", "get_tree", "plan", "enqueue"] {
            assert_eq!(
                CoreClient::call_timeout_for(method),
                CoreClient::LONG_CALL_TIMEOUT,
                "{method} 是内核里「锁外跑网络 I/O」的方法，必须走长调用那一档"
            );
        }
        for method in ["hello", "get_state", "transfer_list", "task_action", "verify_status"] {
            assert_eq!(
                CoreClient::call_timeout_for(method),
                CoreClient::DEFAULT_CALL_TIMEOUT,
                "{method} 不在长家族里，必须走默认那一档（别把表放大）"
            );
        }
        assert!(
            !CoreClient::LONG_CALL_METHODS.contains(&"tree_json"),
            "`tree_json` 不是 RPC 方法名（它是内核里的私有函数），写进表里是一条\
             永远匹配不到的假条目"
        );
    }

    /// 🔴 **判死只有一个出口**：卡在**写**上的那次调用，超时之后必须报 `CallTimedOut`，
    /// **不是** `WriteFailed`。
    ///
    /// 为什么这条承重：`WriteFailed` 不在 `KernelDeath::reason_of` 认的那一档里 ⇒
    /// 它落 `CallFailure::Text` ⇒ **瞬时横幅、没有「重试」**，而通道其实**已经永久死了**
    /// （后续每次调用都是 `Closed`，落同一格）——客户看到一句没有出路的提示，只能重启应用。
    ///
    /// 场景是**可达的**（不是构造出来的）：内核僵住（不再读 stdin）+ 请求体大于管道缓冲区
    /// —— 大批 `enqueue` 很容易撞上。那时卡住的是**写**，看门狗 `close()` 之后它拿到 EPIPE。
    ///
    /// 判别力：把 `call` 里写失败那一路的 `death_or` 换回 `?`（直接抛原错）⇒ 本条红，
    /// 报的是 `WriteFailed`（实测读数见报告）。
    #[test]
    fn a_call_stuck_in_the_write_is_reported_as_the_timeout_not_a_transport_error() {
        let channel = SilentChannel::with_blocking_write();
        let client = CoreClient::new(Box::new(channel.clone()))
            .with_call_timeout(Duration::from_millis(300));

        let (outcome, _) = call_within(&client, &channel, Duration::from_secs(5));
        match outcome.expect_err("写卡住之后必须有界地失败") {
            ClientError::CallTimedOut { ref method, .. } => assert_eq!(method, "get_state"),
            other => panic!(
                "期望 CallTimedOut（判死那一档，客户才会看到「重试」），得到 {other:?}"
            ),
        }
        // 看门狗必须真的关过通道（关掉 ⇒ 卡住的那次写才拿得到 EPIPE 而收场）。
        assert!(channel.closes() >= 1, "看门狗没有关通道");
    }

    /// 正常路径**不许**被看门狗碰到：调用已经返回之后，看门狗不许再关通道。
    ///
    /// 判别力：把 `call` 里那个在飞守卫去掉（或漏清一条返回路径），本用例红 ——
    /// 而真机上那是"一次正常的调用过去一个节拍之后，一条健康的通道被无缘无故关掉"。
    #[test]
    fn a_normal_call_is_untouched_by_the_watchdog() {
        let ch = StubChannel::new(vec![
            line(r#"{"id":1,"ok":true,"result":{"a":1}}"#),
            Reply::Eof,
        ]);
        let client = CoreClient::new(Box::new(ch.clone()))
            .with_call_timeout(Duration::from_millis(300));

        let v = client.call("get_state", Value::Null).expect("正常应答必须照常返回");
        assert_eq!(v["a"], 1);

        // 睡过注入的超时：这一次调用早就返回了，看门狗**不许**再动手。
        // ⚠️ `ch.closes()` 必须**在这一刻现查**（`ch` 是共享状态的一份 clone）。
        //    把它先抄成一个 `usize` 再比，那条断言就恒真 —— 成了一个
        //    **从构造上不可能红**的判据（实测：把守卫的清理整个删掉，它也照样绿）。
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(ch.closes(), 0, "调用早就返回了，看门狗却把通道关了");
    }

    /// 🔴 **看门狗放出来的必须是那次真卡住的读** —— 用**真子进程 + 真管道**验，
    /// 不是靠 `SilentChannel` 替身"说"它会 EOF。
    ///
    /// 为什么非有这一条：`SilentChannel` 的 `close()` 与 `read_line` 是**同一个替身**
    /// 里的一对函数，它们当然配合得起来；而生产上那条链是
    /// **`close()` ⇒ kill 子进程 ⇒ 管道写端关掉 ⇒ 卡在 `read_until` 上的那次返回 EOF**。
    /// 这四步里任何一步不成立（例如 `close()` 不再 kill、或读端被换成一个不会 EOF 的
    /// 东西），替身那两条用例**照样全绿**，而真机上那个 `invoke` 依然永久挂住。
    ///
    /// `sh -c 'exec sleep 30'`：收下 stdin/stdout 两根管道，但**既不读 stdin、也不写
    /// stdout** —— 它就是"内核活着但不回话"。它不会因为 stdin 的 EOF 退出（它不读
    /// stdin），所以这一次收尾走的是 `close()` 的**kill 那一步**，于是本用例的耗时
    /// 约等于注入的超时 + `close()` 里那 3 秒的"体面告别"窗口（`client.rs` 的
    /// `ProcessChannel::close` 第 ② 步，有上界，不是等待）。
    ///
    /// ⚠️ 那条链要是断了（看门狗不关通道、或 `close()` 不再 kill），这次读会**永远**卡在
    ///    真管道上 —— 所以下面用**带期限的 `recv_timeout` + 一条 detached 线程**，不等
    ///    `join`：同样的改法于是变成**红**（带一句指名根因的话），不是**挂住**。
    #[test]
    fn the_watchdog_unblocks_a_real_blocking_read_on_a_real_pipe() {
        let (prog, args) = sh("exec sleep 30");
        let channel = ProcessChannel::new(Path::new(&prog), &args, None).expect("必须能起 sh");
        let client = Arc::new(
            CoreClient::new(Box::new(channel)).with_call_timeout(Duration::from_millis(300)),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Arc::clone(&client);
        std::thread::spawn(move || {
            let _ = tx.send(worker.call("get_state", Value::Null));
        });

        let started = Instant::now();
        let outcome = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("15 秒内没返回：真管道上卡住的那次读没有被放出来（close ⇒ kill ⇒ EOF 这条链断了）");
        let took = started.elapsed();
        let e = outcome.expect_err("内核不答话时，真管道上那一次读也必须被放出来");

        assert!(
            matches!(e, ClientError::CallTimedOut { .. }),
            "期望 CallTimedOut，得到 {e:?}"
        );
        assert!(
            took >= Duration::from_millis(300),
            "还没到注入的超时就返回了（{took:?}）——那说明它压根没等到看门狗动手"
        );
        assert!(
            took < Duration::from_secs(15),
            "等了 {took:?}：卡住的那次读没有被放出来（close ⇒ kill ⇒ EOF 这条链断了）"
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
    ///
    /// ⚠️ 第三个实参是**详细日志**（任务 3 加的）。它**没有默认值** —— 这正是要的：
    ///    少传一个实参走的是编译错误，而不是一次静默的 normal 档（见 `core_arguments`
    ///    的文档）。下面每一行都显式写 `false` / `true`。
    #[test]
    fn core_arguments_match_the_kernel_flags() {
        assert_eq!(core_arguments(None, None, false), Vec::<String>::new());
        assert_eq!(
            core_arguments(Some(Path::new("/tmp/d")), None, false),
            vec!["--download-dir", "/tmp/d"]
        );
        assert_eq!(
            core_arguments(None, Some(Path::new("/tmp/s")), false),
            vec!["--settings", "/tmp/s"]
        );
        assert_eq!(
            core_arguments(Some(Path::new("/tmp/d")), Some(Path::new("/tmp/s")), false),
            vec!["--download-dir", "/tmp/d", "--settings", "/tmp/s"]
        );
        // 开着的时候：那一对**排在最后**（与 macOS 那侧的拼接次序逐个一致）。
        assert_eq!(
            core_arguments(Some(Path::new("/tmp/d")), Some(Path::new("/tmp/s")), true),
            vec![
                "--download-dir",
                "/tmp/d",
                "--settings",
                "/tmp/s",
                "--log-level",
                "verbose"
            ]
        );
        // 别的都在、只有它在：证明这三格**互不依赖**（不是"有目录才拼得出级别"）。
        assert_eq!(
            core_arguments(None, None, true),
            vec!["--log-level", "verbose"],
            "关的那一侧由 core_arguments 的文档钉着：关了**什么都不拼**（连 --log-level \
             normal 也不许拼）"
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
    /// `write` 里**并**持着 stdin 的锁**。收尾若在那里用 `lock` 就会跟着卡死——而收尾是
    /// 既有的**唯一**逃生口（A5 的看门狗那条路要等满**那一档**的上界 —— 150 或 600 秒），
    /// 它一卡，"最坏约 5 秒"就不真了。
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

    // ------------------------------------------------------------------
    // **详细档"每一次壳→内核的调用"那一行**（任务 2 的主判据）。
    //
    // 为什么必须有它：`call` 里那一句记账是**整档新增行为在壳侧的收口**，而同一个缺口
    // 在内核那侧已经撞过一次（任务 1 的审查：`aria2_call` 那一行——那个功能的主角——
    // 一条判据都没有，删掉记账 / 把 `why` 改成无条件 / 把闸门反过来，**全套判据仍然全绿**）。
    //
    // 🔴 **两条硬约束**（下面那条用例逐条绕开，机制见 [`VerboseSink`]）：
    //   ① **不许往真的用户日志目录里写**：`log_verbose` 走 `storage::dir()`
    //      （`%APPDATA%\BenagenDownloader\`，macOS 上是开发者自己那份 Application Support）
    //      ——用例往里写 = "跑一次测试"变成"往用户的日志里灌测试数据"。
    //   ② **不许翻进程级静态**：`diagnostics::init(Level::Verbose)` 写的是全局，而同进程里
    //      那几条假设 normal 的用例（含 `diagnostics` 自己的轮转用例）会**随线程调度随机红**，
    //      那种 flaky 比没有判据更坏。
    //
    // 注入 sink 把两条**同时**绕开：用例既不取那条路径（不碰 ①），也不读不写 `LEVEL`
    // （不碰 ②）。残余：`log_verbose` 里那道**闸门本身**（normal 档不记）仍没有判据 ——
    // 要判它就得动上面两样中的一样；它与内核那侧的同位缺口是同一个，如实记账。
    // ------------------------------------------------------------------

    /// `call` 落下来的每一行：`(事件名, 字段表)`。
    type Recorded = Vec<(String, Vec<(String, String)>)>;

    /// 一个**记账替身** sink：把每一行收进内存，**不碰文件系统、不碰全局**。
    ///
    /// 返回 `(注入用的 sink, 读记录用的句柄)`。
    fn recording_sink() -> (VerboseSink, Arc<Mutex<Recorded>>) {
        let got: Arc<Mutex<Recorded>> = Arc::new(Mutex::new(Vec::new()));
        let sink_got = Arc::clone(&got);
        let sink: VerboseSink = Arc::new(move |event, fields| {
            sink_got
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    event.to_string(),
                    fields
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect(),
                ));
        });
        (sink, got)
    }

    /// 取一条记录里某个字段的值（没有这个字段 ⇒ `None`）。
    fn field_of(line: &(String, Vec<(String, String)>), key: &str) -> Option<String> {
        line.1
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    /// 🔴 **每一次壳→内核的调用都落一行；失败那一行带着壳自己给用户看的那句话**。
    ///
    /// 三拍成功 + 一拍失败 ⇒ **四行**，且：
    ///   · 成功那三条**不带 `why`**（补一个空字段会让"这一行有几个字段"随结果变）；
    ///   · 失败那一条的 `why` **就是 `error_text` 给的那句话**（与界面上显示的同一份）。
    ///
    /// 判别力（两条都实测过，读数见任务 2 报告）：
    ///   · 把 `call` 里那句 `(self.verbose_sink)("kernel_call", &fields)` 删掉
    ///     ⇒ 记录 **0 行**，第一条断言红；
    ///   · 把 `why` 改成**无条件**追加 ⇒ 成功那三条多出 `why=`，最后那道"不许带 why"红。
    #[test]
    fn every_kernel_call_is_logged_with_the_shells_own_words_on_failure() {
        let stub = StubChannel::new(vec![
            line(r#"{"id":1,"ok":true,"result":{"protocol":1}}"#),
            line(r#"{"id":2,"ok":true,"result":{}}"#),
            line(r#"{"id":3,"ok":true,"result":{}}"#),
            line(
                r#"{"id":4,"ok":false,"error":{"code":"no_delivery","message":"这一批不存在或已过期"}}"#,
            ),
        ]);
        let (sink, got) = recording_sink();
        let mut client = CoreClient::new(Box::new(stub.clone()));
        // ⚠️ 只换出口：不加 setter（那会多出一条只在测试里用的公开面，且在本仓的
        //    零告警口径下还可能给非测试构建带出一条 `dead_code`）。
        client.verbose_sink = sink;

        client.call("hello", json!({"protocol": 1})).expect("第一拍成功");
        client.call("get_state", Value::Null).expect("第二拍成功");
        client.call("get_state", Value::Null).expect("第三拍成功");
        let failure = client
            .call("verify", Value::Null)
            .expect_err("第四拍必须失败（内核回 ok:false）");

        let lines = got
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            lines.len(),
            4,
            "每一次调用都要落一行（3 拍成功 + 1 拍失败 = 4 行），实际 {lines:?}"
        );
        for l in &lines {
            assert_eq!(l.0, "kernel_call", "事件名必须是 kernel_call：{l:?}");
            assert!(field_of(l, "ms").is_some(), "每一行都要带耗时：{l:?}");
            assert!(field_of(l, "ok").is_some(), "每一行都要带成败：{l:?}");
        }
        let methods: Vec<String> = lines.iter().filter_map(|l| field_of(l, "method")).collect();
        assert_eq!(
            methods.iter().map(String::as_str).collect::<Vec<_>>(),
            ["hello", "get_state", "get_state", "verify"],
            "记的是方法名（参数里可能有交付码之类，一律不许进来）；\
             顺序也要对得上——一行一次的对应关系本身是判据"
        );

        // 成功那三条：`ok=true`，且**没有 `why`**
        for l in &lines[..3] {
            assert_eq!(field_of(l, "ok").as_deref(), Some("true"), "{l:?}");
            assert!(
                field_of(l, "why").is_none(),
                "成功时**不许**带 `why` —— 补一个空字段会让这一行的字段数随结果变，\
                 而按空格切字段读它的下一个人会读到空值：{l:?}"
            );
        }
        // 失败那一条：`ok=false`，且 `why` 是壳给用户看的那句话（内核原文，逐字）
        let last = &lines[3];
        assert_eq!(field_of(last, "ok").as_deref(), Some("false"), "{last:?}");
        assert_eq!(
            field_of(last, "why").as_deref(),
            Some("这一批不存在或已过期"),
            "失败时 `why` 必须是壳既有那句人话（`presentation::error_text`），\
             不是这里另写的、也不是 `{{:?}}` 的调试形态：{last:?}"
        );
        assert_eq!(
            field_of(last, "why").as_deref(),
            Some(crate::presentation::error_text::error_text(&failure).as_str()),
            "`why` 与界面上给用户看的那句必须是**同一份**"
        );
    }

    /// 🔴 **交付码与下载目录**绝不许出现在**写下去的那一行**里（隐私，规格 §2.3 B）。
    ///
    /// 为什么必须有它：这一批把**内核**那条拉交付页的路收窄成了"只记分类"
    /// （`core/src/delivery.rs` 的 `outcome_category`），正是**因为 URL 里含交付码** ——
    /// 而同一个码从**另一条**路回来了：内核 404 那句文案是**我们自己**拼的
    /// （`清单不存在（404）——请确认交付码是否正确：{url}`），它经 RPC 错误体
    /// 流进 `why`，而 `why` 是**逐字**落的。`core/src/main.rs` 的 `preflight`
    /// 同样把客户目录名带进来。⇒ 壳在**它自己知道的那一刻**把这两个串推下来，
    /// 由 [`CoreClient::set_redactions`] 在落盘前抹掉。
    ///
    /// 判别力（两条都实测过，读数见终审修复报告）：
    ///   · 把 `call` 里那两个 `self.redact(...)` 换回 `…error_text(error)`（即不抹）⇒
    ///     本用例两条断言都红 —— 而真机上的表现是**交付码与客户目录名被回传给我们**；
    ///   · 把 `set_redactions` 的调用点从**发请求之前**挪到**失败之后**（`shell-win/src/
    ///     session.rs`）⇒ 那一次失败的日志里仍然带着码（`set_redactions` 的文档记着这条）。
    ///
    /// ⚠️ **它钉住的是"抹掉"这一步本身**，不是"壳有没有把那两个串推下来"：
    ///    推的那一处（`remember_for_redaction`）的调用点判据不在本 crate —— 如实记账。
    #[test]
    fn the_delivery_code_and_download_directory_never_reach_the_written_line() {
        // 形状按真的来：码是 20 位随机码里的那种、目录是客户机上那种带用户名的绝对路径。
        let code = "C24-8ZQ7K3M9P1W5X7T2";
        let dir = r"C:\Users\张明\Downloads\Benagen";
        // 内核 404 那句（`core/src/delivery.rs`）—— 码**是 URL 的一个路径段**。
        let not_found = format!(
            "清单不存在（404）——请确认交付码是否正确：\
             http://download.benagen.com/{code}/manifest.json"
        );
        // 内核 preflight 那句（`core/src/main.rs`）—— 客户目录名在里面。
        let preflight = format!("目标目录不可写（{dir}）：拒绝访问");
        // ⚠️ `line()` 收的是 `&'static str`（脚本通道的既有形状），而这两条脚本是
        //    现拼的 ⇒ 用 `Box::leak` 造那两份**只活这一次测试**的静态串
        //    （测完进程就结束，泄漏的是两个几十字节的 `String`，没有代价）。
        //    ⚠️ 正文**必须交给 `serde_json` 去转义**：客户目录名里有 `\`（Windows 路径），
        //    手写 `format!` 拼出来的那一行**不是合法 JSON**，内核那一侧会先把它拒掉
        //    —— 于是这条用例会以"内核发来的一行不是合法 JSON"红，而它想测的东西
        //    一个字节都没走到（第一版就是这么红的，记在这里）。
        let envelope = |id: u64, code: &str, message: &str| -> &'static str {
            Box::leak(
                json!({"id": id, "ok": false, "error": {"code": code, "message": message}})
                    .to_string()
                    .into_boxed_str(),
            )
        };
        let not_found_line = envelope(1, "delivery_fetch_failed", &not_found);
        let preflight_line = envelope(2, "preflight_failed", &preflight);
        let stub = StubChannel::new(vec![line(not_found_line), line(preflight_line)]);
        let (sink, got) = recording_sink();
        let mut client = CoreClient::new(Box::new(stub));
        client.verbose_sink = sink;
        client.set_redactions(vec![code.to_string(), dir.to_string()]);

        client.call("load_delivery", Value::Null).expect_err("第一拍必须失败");
        client.call("enqueue", Value::Null).expect_err("第二拍必须失败");

        let lines = got.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        assert_eq!(lines.len(), 2, "两次失败两次记账：{lines:?}");

        let first = field_of(&lines[0], "why").expect("失败那一行必须带 why");
        assert!(!first.contains(code), "交付码进了日志：{first}");
        // **诊断价值要留住**：抹掉的是那两个串，不是整句话 —— URL 那个**路径段**
        // 只剩 `[已隐去]`，而主机名、文件名、那句人话都还在（看日志的人仍然看得出
        // "问题出在拉清单这一路上"）。
        assert!(first.contains("清单不存在（404）"), "只抹那两个串，别把话抹没了：{first}");
        assert!(first.contains("download.benagen.com"), "别把整条 URL 抹掉：{first}");
        assert!(first.contains("manifest.json"), "别把整条 URL 抹掉：{first}");
        assert!(first.contains(crate::diagnostics::REDACTED), "要看得出来这里被抹过：{first}");

        let second = field_of(&lines[1], "why").expect("失败那一行必须带 why");
        assert!(!second.contains(dir), "客户目录名进了日志：{second}");
        assert!(!second.contains("张明"), "目录名的任一段都不该在：{second}");
        assert!(second.contains("目标目录不可写"), "只抹那两个串，别把话抹没了：{second}");
        assert!(second.contains("拒绝访问"), "内核原文的其余部分必须逐字留着：{second}");
    }

    /// ⚠️ **没被告知要抹什么时，`why` 逐字透传**（这道防线不是"顺手改文案"的地方）。
    ///
    /// 判别力：把 `redact` 改成"无条件把整段 `why` 抹成 [`crate::diagnostics::REDACTED`]"
    /// ⇒ 本用例红 —— 而真机上那是**最需要的那半句诊断信息没了**（`why` 存在的全部意义
    /// 就是"引擎为什么没回话"）。它同时钉住"默认清单是空的"这件事。
    #[test]
    fn without_secrets_the_why_is_verbatim() {
        let stub = StubChannel::new(vec![line(
            r#"{"id":1,"ok":false,"error":{"code":"no_delivery","message":"这一批不存在或已过期"}}"#,
        )]);
        let (sink, got) = recording_sink();
        let mut client = CoreClient::new(Box::new(stub));
        client.verbose_sink = sink;

        client.call("load_delivery", Value::Null).expect_err("这一拍必须失败");

        let lines = got.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        assert_eq!(
            field_of(&lines[0], "why").as_deref(),
            Some("这一批不存在或已过期"),
            "没有要抹的串时一个字都不许动：{lines:?}"
        );
    }
}
