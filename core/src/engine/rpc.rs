//! aria2 的 JSON-RPC 客户端：内核与 aria2 之间**唯一**的通信通道。
//!
//! 两条实测得出的硬约束（与 Go 的 `rpc.go` 同源）：
//!  1. **每次调用都必须带 `token:<secret>`**——不带会返回 400 `Unauthorized`。
//!  2. aria2 把错误放在 JSON 体的 `error` 对象里并回 400。只报 HTTP 状态码会让排查
//!     无从下手（例如 `Invalid GID x` 与 `Unauthorized` 都是 400），必须把 message 带出来。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::legs::{parse_servers, Leg};
use super::status::{parse_num, RawTask};

/// 单次 RPC 的超时。对应 Go 的 `&http.Client{Timeout: 10 * time.Second}`。
/// **快档**（探活/写/控制/判活采样）：判活要快，放宽它等于放慢「断开」判定与恢复阶梯。
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// **慢档**超时（快照类读：`tell*` 与快照的 `getGlobalStat`）。
/// 现场证据（诊断日志 20261014-081646）：245 次传输层重试里 94 次重试成功、
/// 应答落在首试 10 秒超时后的 2~9 秒 ⇒ 30 秒档下这些全是首试干净成功。
/// ⚠️ 慢档**单次尝试、不重试**（规格 §3.2）：周期性读，下一拍就是重试。
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

/// **`tellActive` / `tellWaiting` / `tellStopped` 的 `keys` 白名单**（2026-10-08）。
///
/// 恰好是 [`RawTask`] 反序列化时会读的那些键 —— 一个不多、一个不少。
/// 判据是 `the_keys_whitelist_matches_what_raw_task_reads`：它把一个**每个字段都非默认**
/// 的 `RawTask` 序列化出来，断言白名单与那份 JSON 的键集合**逐字相同**
/// ⇒ 给 `RawTask` 加字段而忘了加到这里，那条立刻红（少了键的表现是
/// **静默落空成 `0` / `""`**，也就是本项目已经栽过一次的"没有下载进度"那个形状）。
///
/// ⚠️ **为什么要传它**：不传时 aria2 为每个任务返回**全部字段**，其中 `files[]`
///    （每个分片的 uri / 路径 / 已完成字节）是响应里的大头 —— 而内核一个字节都不读。
///    一份真机日志（677 个任务的批次）里，`tell*` 的中位耗时 **600–2500 ms**、
///    最大 9940 ms；连只回几个数字的 `getGlobalStat` 也中位 **912 ms**。
///    aria2 只有一条事件循环，那几次巨大的序列化把它占住 ⇒ 同循环里所有请求一起等
///    ⇒ 撞满 `RPC_TIMEOUT` ⇒ 内核判「引擎已断开」。**这一条是那件事的一半。**
const TASK_KEYS: [&str; 7] = [
    "gid",
    "status",
    "totalLength",
    "completedLength",
    "downloadSpeed",
    "connections",
    "errorMessage",
];

fn task_keys() -> serde_json::Value {
    serde_json::Value::Array(TASK_KEYS.iter().map(|k| serde_json::Value::from(*k)).collect())
}

/// `tellWaiting` 一次拉多少条（**队头**）。
///
/// ⚠️ **值保持 1000，与 Go 逐字相同**：`tellWaiting` 是队头在前（FIFO，实验证明），
///    截头无损；更要紧的是「已暂停」任务**住在 waiting 表里**——[`super::daemon::Daemon`]
///    的「一键开始」判据（`BatchControl`）要看得见它们，收小会把暂停中的任务藏起来。
const WAITING_RANGE: u64 = 1000;

/// `tellStopped` 取**尾窗**多少条（最近完成的）。
///
/// ⚠️ **最旧在前**（实验证明，内嵌 aria2c 1.37.0）：`offset=0` 是**最老**的完成项，
///    截头会显示陈旧完成、藏起刚完成的。所以取的是**尾窗**——
///    `offset = max(0, numStopped − STOPPED_WINDOW)`——才是「最近 300 条」。
///    计数仍全量走 `getGlobalStat`，不许从行数推导。
///
/// ⚠️ **300 是行为变更**（2026-10-14 起，规格 §4.3）：传输列表的「已完成/失败」行
///    最多显示最近 300 条；已登记进两份 README 的验收册。UI 分页是它的下半场（§5 留给下一轮）。
const STOPPED_WINDOW: u64 = 300;

/// 一次 RPC 的失败。
///
/// `transport` 为真表示**传输层**失败（连不上 / 读到超时 / 半路断开）——**只有这一类**
/// 值得换一条新连接再试一次；协议层失败（HTTP 状态码、`error` 对象、JSON 形状）
/// 重试只会得到同样的结果。
///
/// ⚠️ 抽这个类型是为了让"该不该重试"由**类型**回答，而不是靠比对错误字符串前缀——
/// 后者会在某次改文案时**静默失去判别力**。
struct RpcFailure {
    msg: String,
    transport: bool,
    /// 引擎**明确应答**并报了 `error` 对象。只有这一种失败能证明动作没有发生。
    explicit: bool,
}

impl RpcFailure {
    fn protocol(msg: String) -> Self {
        Self { msg, transport: false, explicit: false }
    }
    fn transport(msg: String) -> Self {
        Self { msg, transport: true, explicit: false }
    }
    fn rejected(msg: String) -> Self {
        Self { msg, transport: false, explicit: true }
    }
}

/// 一次**写**调用的结局。
///
/// ⚠️ **"引擎拒绝了"与"引擎没回话"必须分开**：前者能证明动作没有发生；后者**不能**——
/// 读超时（规格 F1 的形态）时，请求很可能已经被 aria2 执行了，我们只是没等到应答。
/// 所以只有 `error` 对象才算 `Rejected`；body 读不完、解析不了、状态码异常，一律是 `NoAnswer`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 引擎明确应答且接受了。
    Applied,
    /// 引擎明确应答并拒绝（如 "GID not found" / "not paused"）。动作确定没有发生。
    Rejected(String),
    /// 没拿到可靠的应答。动作可能已经发生，也可能没有。
    NoAnswer(String),
}

/// **可以安全重试**的读方法白名单（规格 §3 A1）。
///
/// 🔴 写方法**一个都不许进来**：`aria2.addUri` **不幂等**，而本仓用
/// `--auto-file-renaming=false` + `--allow-overwrite=true` ⇒ 盲重试会让同一个 `out`
/// 建出**两个 GID 写同一个落盘路径**。那是数据风险，不是性能问题。
///
/// 白名单而不是"排除法"：将来加了一个新的写方法却忘了排除，排除法会**静默重试**它；
/// 白名单只会**不重试**它——少自愈一次，但绝不会多写一次。代价不对称，所以选白名单。
fn is_retryable_read(method: &str) -> bool {
    matches!(
        method,
        "aria2.getGlobalStat" | "aria2.tellActive" | "aria2.tellWaiting" | "aria2.tellStopped"
    )
}

/// 详细档那一行（"每一次到 aria2 的往返"）的出口。
///
/// 🔴 **它做成可注入的一格，是为了让判据能在不碰文件系统、也不翻进程级静态的前提下
///    钉住 [`RpcClient::send`] 那一行** —— 那两条硬约束的理由与绕过方式都写在
///    `send` 上（`log_verbose` 写的是**平台默认的用户日志目录**；`diagnostics::init`
///    写的是**进程级全局**，单测里动它会让同进程里那几条假设 normal 档的既有用例随机红）。
///    生产路径**恒为** [`crate::diagnostics::log_verbose`]，只有本文件的 `mod tests`
///    会把它换成记账替身。
///
/// 同形的先例（"决策与调用分开"）：`spawn.rs` 的 `creation_flags_for`、
/// `platform.rs` 的 `env_var_name`、`pickdir.rs` 的 `pick_outcome`。
type VerboseSink = std::sync::Arc<dyn Fn(&str, &[(&str, String)]) + Send + Sync>;

/// aria2 JSON-RPC 的最小客户端。
///
/// 它**可并发使用**：界面线程调 `add_uri` 的同时，进度轮询线程调 `list`/`global`。
/// Go 侧靠"结构体里不放可变字段"来保证这一点，注释里还点名了"计数器之类'看着无害'的字段
/// 在这里就是 data race"。Rust 侧的请求 ID **必须是**一个递增计数器
/// （协议 §5.1 要求 `id` 参与配对，见 [`RpcClient::send`]），
/// 于是它按 Rust 的规矩落在 `AtomicU64` 上——并发安全由类型系统兜住，而不是靠纪律。
pub struct RpcClient {
    url: String,
    secret: String,
    /// `ureq::Agent` 自带连接池，它就是 Go 侧 `http.Client` 的等价物：
    /// 轮询每 200 ms 一次，每次新建 agent 等于每次重来一遍 TCP 握手。
    agent: ureq::Agent,
    /// **慢档**的 agent（快照类读：`tell*` 与快照的 `getGlobalStat`）。
    ///
    /// 与 [`Self::agent`] 的差别只有超时：`.timeout(SNAPSHOT_TIMEOUT)`（读响应放宽到 30 秒）
    /// 而 `.timeout_connect(RPC_TIMEOUT)`（**连接阶段仍要快**——放宽的只是读响应，
    /// 规格 §3 A4）。慢档调用走 [`Self::call_slow`]，**单次尝试、不重试**。
    slow_agent: ureq::Agent,
    /// 下一个请求 ID。只用 `fetch_add`，没有"先读后写"。
    next_id: AtomicU64,
    /// 详细档那一行的出口（见 [`VerboseSink`]）。生产恒为 `diagnostics::log_verbose`。
    verbose_sink: VerboseSink,
    /// 可选的计数器句柄。`None` = 不记账。
    /// ⚠️ 它不参与任何判据，只被读去写日志 —— 记账失败不许影响一次下载。
    counters: Option<std::sync::Arc<crate::diag_counters::Counters>>,
}

/// `getGlobalStat` 的结果。
///
/// ⚠️ 字段名是 Rust 侧的命名，**不是**线上形态（线上形态是字符串，见下文的 raw 结构）。
/// Go 侧特意声明它可被 JSON 序列化：后续若要把状态快照落盘或经 IPC 传给别的进程，
/// `json:"-"`（Rust 侧对应 `#[serde(skip)]`）会让全部数值静默丢失，
/// 而调用方拿到的是一份"看着正常"的空快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalStat {
    pub download_speed: i64,
    pub num_active: i64,
    pub num_waiting: i64,
    pub num_stopped: i64,
}

/// 请求体。
#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: String,
    method: &'a str,
    params: Vec<serde_json::Value>,
}

/// 响应体。`error` 与 `result` 二选一（aria2 把错误塞在 `error` 里并回 400）。
#[derive(Deserialize)]
struct RpcResponse {
    /// 缺 `result` 键时取 `Null`——与 Go 的 `json.RawMessage` 收到 nil 的后果一致：
    /// `ping` 无所谓，`global`/`add_uri`/`list` 会在各自的反序列化处报错。
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    error: Option<RpcErrorBody>,
}

#[derive(Deserialize)]
struct RpcErrorBody {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// `getGlobalStat` 的线上形态：**所有数值都是字符串**。
///
/// 字段名↔tag 的对应关系没有编译期保护：写错任一 tag，对应字段不报错、只会静默变成 0
/// （`atoi("") == 0`），所以 `rpc_global_parses_string_numbers` 给四个值各挑不同的数——
/// 任何一个映射错位都会被抓到。缺字段同样退化为 0（`#[serde(default)]`），与 Go 一致。
#[derive(Deserialize)]
struct GlobalStatRaw {
    #[serde(default, rename = "downloadSpeed")]
    download_speed: String,
    #[serde(default, rename = "numActive")]
    num_active: String,
    #[serde(default, rename = "numWaiting")]
    num_waiting: String,
    #[serde(default, rename = "numStopped")]
    num_stopped: String,
}

/// `getGlobalStat` 的解析：线上形态（四个字符串数字）→ [`GlobalStat`]。
///
/// [`RpcClient::global`] 与 [`RpcClient::global_slow`] 共用这一段——**两个入口、
/// 同一段解析**，避免两处实现漂移（`parse_num` 那次教训的同款形状）。
fn parse_global_stat(raw: serde_json::Value) -> Result<GlobalStat, String> {
    let r: GlobalStatRaw = serde_json::from_value(raw)
        .map_err(|e| format!("解析 getGlobalStat 失败: {e}"))?;
    Ok(GlobalStat {
        // 与 `status::RawTask` 共用同一个 `parse_num`（同一份 `fmt.Sscanf("%d")` 语义），
        // 不在这里重写一遍——两处实现漂移过一次就够受了。
        download_speed: parse_num(&r.download_speed),
        num_active: parse_num(&r.num_active),
        num_waiting: parse_num(&r.num_waiting),
        num_stopped: parse_num(&r.num_stopped),
    })
}

/// `getUris` 的 `result` → URI 列表（保持线上顺序，**不去重**）。
///
/// 形状不对返回 `Err` 而不是空表：空表在调用方眼里是「这个任务没有域名腿」（正常的跳过），
/// 而形状漂移是另一件事——两者必须可区分，否则将来 aria2 换个形状又会变成一次静默失效。
fn parse_uris(v: &serde_json::Value) -> Result<Vec<String>, String> {
    let Some(items) = v.as_array() else {
        return Err(format!("解析 getUris 返回失败: 期望数组，实际 {v}"));
    };
    let mut uris = Vec::with_capacity(items.len());
    for item in items {
        let Some(uri) = item.get("uri").and_then(serde_json::Value::as_str) else {
            return Err(format!("解析 getUris 返回失败: 元素缺 uri 字符串，实际 {item}"));
        };
        uris.push(uri.to_string());
    }
    Ok(uris)
}

/// `changeUri` 的返回值 `[removed, added]` → 两个计数。
///
/// 读不懂就按 `(0, 0)`：**动作已经发生**，不能因为读不懂计数就把一次成功的换腿判成失败
/// （调用方会把 `Err` 当"这一拍没换成"）。代价只是诊断少记一条，收益是不会谎报。
fn change_uri_counts(v: &serde_json::Value) -> (u64, u64) {
    let count_at = |i: usize| -> u64 {
        match v.as_array().and_then(|a| a.get(i)) {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            // 线上是数字（实测 `[1,0]`）；接受数字字符串只是防形状漂移，不是假设它是字符串。
            Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    };
    (count_at(0), count_at(1))
}

impl RpcClient {
    /// 构造客户端。`url` 形如 `http://127.0.0.1:6800/jsonrpc`。
    pub fn new(url: &str, secret: &str) -> Self {
        Self {
            url: url.to_string(),
            secret: secret.to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(RPC_TIMEOUT)
                // ⚠️ **必须显式写**：ureq 的 `timeout_connect` 默认 30 秒，而且
                //    在连接阶段**优先于**上面那条整体超时（vendor 源码 `stream.rs:352-356`）。
                //    少了这一行，`rpc.rs` 与 `daemon.rs` 里"最坏约 10 秒"的论证在**连接阶段**
                //    就是错的（最坏 30 秒）——`connect_phase_is_bounded_by_rpc_timeout`
                //    守着这一行。规格 §3 A4。
                .timeout_connect(RPC_TIMEOUT)
                .build(),
            slow_agent: ureq::AgentBuilder::new()
                // 读响应放宽到 30 秒；连接阶段仍按 `RPC_TIMEOUT`（见 [`Self::slow_agent`]）。
                .timeout(SNAPSHOT_TIMEOUT)
                .timeout_connect(RPC_TIMEOUT)
                .build(),
            next_id: AtomicU64::new(1),
            // 生产路径**恒为**它；`mod tests` 才会换成记账替身（见 [`VerboseSink`]）。
            verbose_sink: std::sync::Arc::new(crate::diagnostics::log_verbose),
            counters: None,
        }
    }

    /// 挂上计数器。消费自身再返回，接在构造链后面写。
    #[must_use]
    pub fn with_counters(mut self, c: std::sync::Arc<crate::diag_counters::Counters>) -> Self {
        self.counters = Some(c);
        self
    }

    /// 一次性 Agent —— **保证一条全新的 TCP 连接**。
    ///
    /// `ureq::Agent` 自带连接池，同一个 Agent 的请求会复用池里的连接（本结构体那个
    /// `agent` 字段就是）。池里的连接**可能已经坏了而我们不知道**（对端被冻结、
    /// 连接被中间设备掐掉、机器休眠过），于是"这次调用失败"未必意味着引擎有问题。
    /// **探活与传输层重试都必须绕开它**——它们要回答的是「**现在**通不通」。
    ///
    /// ⚠️ **两处 agent 构造的超时必须一致**（这里与 `RpcClient::new`）：只给其中一处加
    /// `timeout_connect` 等于没加——探活/重试走的是这一条，常规调用走的是池内那一条，
    /// 哪一条漏了，连接阶段就会退到 ureq 默认的 30 秒。规格 §3 A4。
    fn fresh_agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(RPC_TIMEOUT)
            .timeout_connect(RPC_TIMEOUT)
            .build()
    }

    /// 发一次 RPC（用调用方指定的 agent），把失败分成传输层与协议层。
    ///
    /// ⚠️ **本函数是"每一次到 aria2 的往返"的唯一收口**：详细档那一行记在**这里**，
    ///    于是 `call`（读/写）与 `ping`（探活）自动全覆盖 —— 在调用点各记一遍
    ///    会漏掉下一个人新加的第三条路。
    ///    真正的实现是 [`Self::send_inner`]（本函数只负责计时与记账）。
    ///
    /// ⚠️ **"记不记"由调用点决定，本函数无条件记**：闸门在
    ///    [`crate::diagnostics::log_verbose`]（只有详细档才真落盘）。所以判据可以
    ///    往 [`VerboseSink`] 注入一个替身来数行数，**不必**碰文件系统、也不必翻
    ///    进程级静态。
    fn send(
        &self,
        agent: &ureq::Agent,
        method: &str,
        params: &[serde_json::Value],
    ) -> Result<serde_json::Value, RpcFailure> {
        let started = std::time::Instant::now();
        let outcome = self.send_inner(agent, method, params);
        let took = started.elapsed();
        if let Some(c) = &self.counters {
            c.record_call(took, outcome.is_ok());
        }
        let mut fields = vec![
            ("method", method.to_string()),
            ("ms", took.as_millis().to_string()),
            ("ok", outcome.is_ok().to_string()),
        ];
        // ⚠️ **只有失败时才带 `why`**：成功时补一个空字段会让"这一行有几个字段"
        //    随结果变，而按空格切字段读它的下一个人会读到空值。
        //    `why` 就是 **aria2 自己说的那句话** —— 这一整档存在的理由。
        //    ⚠️ 它与 `rpc_retry` 那条**不是同一件事，但会重叠**：传输层失败触发重试时，
        //       第一次的 `msg` 会**同时**出现在这里（`why`）与那条的 `first` 上
        //       —— 同一个串、两行。两行都值得有：那条还带着重试的结果与耗时。
        //       （协议层失败**不重试** ⇒ 那种失败只有这一行。）
        if let Err(failure) = &outcome {
            fields.push(("why", failure.msg.clone()));
        }
        (self.verbose_sink)("aria2_call", &fields);
        outcome
    }

    /// 发一次 RPC，把失败分成传输层与协议层。
    ///
    /// **顺序是承重的**（与 Go 的 `call` 逐字一致）：先把 body 解析成 JSON，
    /// 再看 `error` 对象，**最后**才看 HTTP 状态码。
    /// aria2 用 400 表示"调用出错"且详情在 body 里，所以**不能**先按状态码失败——
    /// 那会把 `Invalid GID x` 与 `Unauthorized` 一起压成一句"HTTP 400"。
    ///
    /// ⚠️ 调用点一律走 [`Self::send`]（它在这里加计时与详细档那一行）；
    ///    本函数只做事、不记账。
    fn send_inner(
        &self,
        agent: &ureq::Agent,
        method: &str,
        params: &[serde_json::Value],
    ) -> Result<serde_json::Value, RpcFailure> {
        // 请求 ID 用**递增计数器**：协议 §5.1 要求 `id` 参与请求/响应配对。
        // Go 用常量 `"1"`，理由是"调用方可以并发、递增计数器只引入竞态，而 id 在这里没有消费者"
        // ——那个理由在 Rust 侧不成立：并发在这里由 `AtomicU64` 的 `fetch_add` 兜住
        // （读-改-写是一步，`fetch_add` 返回的是**本次**的值），没有竞态可引入。
        // 类型仍是 JSON 字符串，与 Go 的线上形态一致；变的是取值不再恒定。
        //
        // 仍然**不解析**响应里的 `id`（与 Go 相同）：本客户端是一来一回的同步调用，
        // 配对由调用栈和 TCP 本身保证。计数的意义在于——将来若真的并发/批量化，
        // 唯一的 `id` 已经在那里了，而常量 `"1"` 在那一刻会立刻出错。
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let mut all = Vec::with_capacity(params.len() + 1);
        // secret 永远排在第一位；aria2 要求如此
        all.push(serde_json::Value::String(format!("token:{}", self.secret)));
        all.extend_from_slice(params);

        let body = serde_json::to_string(&RpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params: all,
        })
        .map_err(|e| RpcFailure::protocol(format!("RPC 请求无法序列化: {e}")))?;

        // ⚠️ 不能把 4xx/5xx 当失败先返回：aria2 用 400 表示"调用出错"，详情在 body 里。
        // `ureq` 把 4xx/5xx 作为 `Err(Status(_, resp))` 抛出，但响应本身还在手上，照读。
        let (status, text) = match agent
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_string(&body)
        {
            Ok(resp) => (
                resp.status(),
                resp.into_string()
                    .map_err(|e| RpcFailure::protocol(read_body_error(e)))?,
            ),
            Err(ureq::Error::Status(code, resp)) => (
                code,
                resp.into_string()
                    .map_err(|e| RpcFailure::protocol(read_body_error(e)))?,
            ),
            Err(ureq::Error::Transport(e)) => {
                return Err(RpcFailure::transport(format!("RPC 请求失败: {e}")))
            }
        };

        let parsed: RpcResponse = serde_json::from_str(&text)
            .map_err(|e| RpcFailure::protocol(format!("RPC 响应无法解析（HTTP {status}）: {e}")))?;
        if let Some(err) = parsed.error {
            return Err(RpcFailure::rejected(format!(
                "aria2 报错: {} (code {})",
                err.message, err.code
            )));
        }
        if status != 200 {
            return Err(RpcFailure::protocol(format!("RPC 返回 HTTP {status}")));
        }
        Ok(parsed.result)
    }

    /// 发一次 RPC 并返回 `result` 的原始 JSON；**读方法在传输层失败后会换新连接再试一次**。
    ///
    /// 用本结构体那个**带连接池**的 `agent` 打第一拍——承重的解析顺序与错误分级都在
    /// [`RpcClient::send`] 里。第一拍失败且满足两个条件（**传输层**失败 + 是**读**方法）时，
    /// 用 [`RpcClient::fresh_agent`] 换一条**全新连接**重试，且**只重这一次**。
    ///
    /// 为什么要重试：池里那条连接**可能已经坏了而我们不知道**（对端被冻结、连接被中间设备
    /// 掐掉、机器休眠过）。不重试的后果是上游 `on_rpc_failure` 把一次**抖动**判成"引擎断开"
    /// 并闩死；重试的成本只是一次循环调用。
    ///
    /// 为什么只对**读**方法重试：见 [`is_retryable_read`]。
    fn call(&self, method: &str, params: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        match self.send(&self.agent, method, params) {
            Ok(v) => Ok(v),
            // 传输层失败 + 是读方法 ⇒ 池里那条连接**可能**就是坏的，换一条**全新连接**再试一次。
            // 成功就当作没发生过（这正是"静默自愈"的那一类）；失败也**不**再试第二次。
            Err(first) if first.transport && is_retryable_read(method) => {
                let started = std::time::Instant::now();
                let out = self.send(&Self::fresh_agent(), method, params);
                // 成败**都**落一条（规格 §3 A6）：重试成功说明"池里那条连接坏了"，
                // 重试失败才是"引擎真的不回话"——两者在客户机器上长得一模一样，
                // 只有这条日志能把它们分开。
                // 记的是方法名 / 阶段 / 耗时 / 结果 / 第一次的错误串：方法名是固定白名单里的
                // 字面量，参数（含 `token:<secret>`）一个都不进来。
                crate::diagnostics::log(
                    "rpc_retry",
                    &[
                        ("method", method.to_string()),
                        ("phase", "read".to_string()),
                        ("ok", out.is_ok().to_string()),
                        ("elapsed_ms", started.elapsed().as_millis().to_string()),
                        ("first", first.msg.clone()),
                    ],
                );
                out.map_err(|second| second.msg)
                // ⚠️ 重试失败时返回**重试那次**的错误，不是第一次的（规格 §3 A1）：
                //    第一次的失败发生在一条**可能已经坏掉的池内连接**上，它描述的未必是引擎的真实状态；
                //    重试走的是全新连接，它的失败才是"现在到底通不通"。例：第一次读超时、
                //    重试却 connection refused ⇒ 引擎真的没了——报第一次的"超时"会把客户引向错方向。
            }
            // ⚠️ 这一路返回的是**第一次**的失败（`first`）；重试那一臂为什么返回它自己的错误，理由写在上面。
            Err(first) => Err(first.msg),
        }
    }

    /// 慢档调用：**单次尝试**（规格 §3.2）。只有快照类读走它。
    ///
    /// 与 [`Self::call`] 的差别只有两处：用 [`Self::slow_agent`]（读响应 30 秒），
    /// 以及**不走** `is_retryable_read` 的传输层重试——慢档读是周期性的（200 ms 轮询），
    /// 下一拍就是天然的重试；重试会把最坏代价翻倍，而契约 §4.3 要的正是"一次快照失败
    /// 的最坏代价小"。
    fn call_slow(&self, method: &str, params: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        self.send(&self.slow_agent, method, params).map_err(|e| e.msg)
    }

    /// 探活与校验 secret。
    ///
    /// 统一走 `aria2.getGlobalStat`：它同样校验 token，探活与验密一次做完。
    /// 结果被丢弃——探活只关心"这一趟走不走得通"。
    ///
    /// ⚠️ **走一次性 Agent（全新连接）**：每一次探活都必须绕开连接池，理由见
    /// [`RpcClient::fresh_agent`]。这是 2026-10-05 修的（规格 §3 A2）——此前它复用池内连接，
    /// 于是"引擎卡住了"与"池里那条连接坏了"在判定表里**长得一模一样**。
    ///
    /// ⚠️ **失败才落诊断，成功不记**（规格 §3 A6）：探活在重连判定里是**每一步**都调的，
    /// 成功也记的话日志会在几分钟内被刷满，真正出事那一条反而被淹没。
    pub fn ping(&self) -> Result<(), String> {
        let started = std::time::Instant::now();
        let out = self.send(&Self::fresh_agent(), "aria2.getGlobalStat", &[]);
        if let Err(f) = &out {
            crate::diagnostics::log(
                "ping_failed",
                &[
                    ("phase", "read".to_string()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                    ("err", f.msg.clone()),
                ],
            );
        }
        out.map(|_| ()).map_err(|f| f.msg)
    }

    /// 读全局统计。aria2 把这些数当字符串传，这里统一转成整数。
    pub fn global(&self) -> Result<GlobalStat, String> {
        let raw = self.call("aria2.getGlobalStat", &[])?;
        parse_global_stat(raw)
    }

    /// 慢档读全局统计（快照路径，[`super::daemon::Daemon::snapshot`] 用它）。
    ///
    /// 与 [`Self::global`] **同一段 `getGlobalStat` 解析**（[`parse_global_stat`]）、
    /// 同一个线上形态，只是走 [`Self::call_slow`]（30 秒档、单次尝试）。
    /// `global()` 是**快档**，留给 [`super::daemon::Daemon::probe`] 与判活采样——
    /// `getGlobalStat` 有两个调用点、两种语义，档位按**调用面**分，不按方法名分
    /// （规格 §3.3）：把 `getGlobalStat` 一律放宽会让判活变慢，恰得其反。
    pub fn global_slow(&self) -> Result<GlobalStat, String> {
        let raw = self.call_slow("aria2.getGlobalStat", &[])?;
        parse_global_stat(raw)
    }

    /// 加一个下载任务，返回 GID。
    ///
    /// `dir`/`out` 的语义与 `-i` 输入文件完全一致（含非 ASCII 与空格）。
    ///
    /// `uris` 是**全量 URI 列表**（规格 §5.1）：域名 URI 打头，IP 备胎跟在后面。
    /// aria2 的 `--uri-selector=inorder` 保证先用第一个、坏了才轮到下一个。
    ///
    /// `extra` 是**逐任务**选项（`max-connection-per-server`、`split`、`min-split-size`、
    /// `max-tries`、`retry-wait`）。它们随任务一次性交付，而不是走全局默认：
    /// 要么在每次 `add_uri` 前先 `change_global_option`（与界面并发加任务是竞态），
    /// 要么等拿到 GID 再改选项（那时任务已经在按旧选项传了）。
    /// 设置面板那五项的值因此必须从这里落地。
    ///
    /// `extra` 里的同名键会被 `dir`/`out` **覆盖**：`dir`/`out` 是本客户端的落盘基准，
    /// 让 `extra` 改写它们等于允许调用方越过目标目录，代价不可逆。
    ///
    /// ⚠️ **参数形状是 `[token, uris, options]`，`options` 在索引 2**（契约 §2.4 的 `#8`）。
    /// 控制者曾把这个下标写成 1，照抄运行是三条断言全报空 map 的**假红**——
    /// 断言"合并语义坏了"，其实是测试自己没看到请求。改这里时先看那条请求体断言。
    pub fn add_uri(
        &self,
        uris: &[String],
        dir: &str,
        out: &str,
        extra: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let mut opts = serde_json::Map::new();
        for (k, v) in extra {
            opts.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        // 顺序是刻意的：extra 先铺，dir/out 后压——同名的键被覆盖。
        opts.insert("dir".to_string(), serde_json::Value::String(dir.to_string()));
        opts.insert("out".to_string(), serde_json::Value::String(out.to_string()));

        // `call` 会在前面补上 token，所以这里只给 `[uris, options]` —— 线上就是
        // `[token, uris, options]`，options 落在索引 2。
        let params = [
            serde_json::Value::Array(
                uris.iter().map(|u| serde_json::Value::String(u.clone())).collect(),
            ),
            serde_json::Value::Object(opts),
        ];
        let raw = self.call("aria2.addUri", &params)?;

        let gid = raw
            .as_str()
            .ok_or_else(|| format!("解析 addUri 返回失败: 期望字符串，实际 {raw}"))?;
        if gid.is_empty() {
            return Err("addUri 未返回 GID".to_string());
        }
        Ok(gid.to_string())
    }

    /// 只取**活跃**的任务（一条 `tellActive`）。
    ///
    /// 停滞采样只用它：`list()` 还要 `tellWaiting`（队头 1000 条）/ `tellStopped`（尾窗 300 条），
    /// 而采样一条都用不到。
    pub fn active(&self) -> Result<Vec<RawTask>, String> {
        let raw = self.call("aria2.tellActive", &[task_keys()])?;
        serde_json::from_value(raw).map_err(|e| format!("解析 aria2.tellActive 失败: {e}"))
    }

    /// 读三个列表方法（而不是按 GID 的 `tellStatus`）。
    ///
    /// 用列表方法的理由：按 GID 查会因 GID 失效而 400（`Invalid GID`），
    /// 而列表方法不需要我们自己维护 GID 的生命周期。
    ///
    /// ⚠️ **拼接顺序 `tellActive` → `tellWaiting` → `tellStopped` 是一条承重前提，
    /// 不是实现细节**（契约 §1.2）：正是"已停止的排在后面"这一点，
    /// 让"逐个 apply、后者覆盖前者"（last-write-wins）会**确定性地**选到有害的一侧
    /// ——失败任务盖住刚传完待校验的完成任务，触发路径（失败 → 重下）正是常规重试流。
    /// **改变它会让 `view::compose` 的排名选择失去依据**（那套"先按 `task_rank` 挑出
    /// 唯一胜出者、再应用一次"的做法，存在的理由就是这条顺序），而且**不会报错**。
    /// 若认为该顺序在 Rust 侧应当不同，先停下来报告。
    ///
    /// ⚠️ **`stopped_count` 是 `getGlobalStat.numStopped` 的全量计数**，用来算
    /// `tellStopped` 的尾窗偏移（`max(0, stopped_count − STOPPED_WINDOW)`）——
    /// 最旧在前（实验证明），截头会显示陈旧完成、藏起刚完成的，只有尾窗才是「最近」。
    /// **计数仍全量走 `getGlobalStat`**，调用方不许从行数推导。快照路径上它由
    /// [`super::daemon::Daemon::snapshot`] 先发 `global_slow()` 再传进来。
    ///
    /// ⚠️ **第一次出错就返回**：不要先跑完三个子请求再报错。契约 §4.3 的论证建立在这上面
    /// ——**真死引擎**时"一次快照失败即判定引擎断开"的失败上界是**慢档单次 30 秒**
    /// （`SNAPSHOT_TIMEOUT`，首错即返）。反事实是「三个子请求都跑完才报错」= 3×30 秒量级
    /// （旧账「3–6 次 × 30 秒」是**含重试**的算法，慢档不重试后不成立）。
    /// 慢应答**成功**路径 ≈ 3×`SNAPSHOT_TIMEOUT`（本方法的三个子请求；整份快照 4×，
    /// 见 `kernel::current_tasks`）。
    /// ⚠️ **慢档单次尝试、不重试**（规格 §3.2）：快照类读是周期性的
    /// （200 ms 轮询），下一拍就是天然的重试；重试会把最坏代价翻倍，而这里要的正是
    /// "一次快照失败的最坏代价小"。"连接坏了还是引擎不理人"的判别由 `on_rpc_failure`
    /// 的探活承担，不靠读重试。先跑完会把最坏情况拖成三倍，也就把"为它多等几轮"
    /// 这个被否掉的做法又请了回来。
    ///
    /// ⚠️ **"30 秒"是单次尝试的上界，且含连接阶段**：慢档 agent 显式设了
    /// `.timeout(SNAPSHOT_TIMEOUT)`（读响应）与 `.timeout_connect(RPC_TIMEOUT)`（连接阶段
    /// 仍要快——放宽的只是读响应，规格 §3 A4）。少了 `timeout_connect` 那一行，连接阶段
    /// 会退到 ureq 默认的 **30 秒**（`timeout_connect` 在连接阶段**优先于**整体超时）——
    /// `slow_connect_phase_is_bounded_by_rpc_timeout` 与
    /// `connect_phase_is_bounded_by_rpc_timeout` 分别守着慢档与快档这两处。
    ///
    /// ⚠️ **这里曾经踩过一个静默坑，值得记住形状**（修复轮 1 已修）：
    /// `list()` 是 `RawTask` 的第一个**反序列化**消费者——任务 5 的四条测试全部直接构造结构体，
    /// 于是「从 aria2 的 JSON 读进来」这条路一次都没被走过，而 `RawTask` 当时**缺 `camelCase`
    /// 重命名**。后果：`#[serde(default)]` 让 `totalLength`/`completedLength`/`downloadSpeed`/
    /// `errorMessage` 四个多词键静默落空，经 `to_task()` 变成 `0`/`""`，**不报错**，
    /// 表现正是本项目要修的那条用户反馈——"没有下载进度"（`gid`/`status`/`connections`
    /// 恰好同名，所以看着"大部分是对的"，这掩盖了它）。
    /// 现在由 `status::tests::raw_task_reads_aria2_wire_form` 钉住（那条测试**真的走 serde**）。
    /// 教训是测试的形状：**绕开真正会出错的那条路的测试，等于没测**。
    pub fn list(&self, stopped_count: u64) -> Result<Vec<RawTask>, String> {
        // 顺序逐字照 Go：active → waiting → stopped。见本方法的文档注释——
        // 这是契约 §1.2 的前提，不是随手排的。
        let mut out: Vec<RawTask> = Vec::new();
        // 🔴 **只问用得上的那七个键**（2026-10-08）。
        //
        // 不传 `keys` 时 aria2 会为**每个任务**返回**全部字段** —— 含 `files[]`
        // （每个分片的 uri / 路径 / 已完成字节）、`bittorrent`、`uris`、时间戳……
        // 而 `RawTask` 只读七个（见 [`TASK_KEYS`] 的文档）。
        //
        // ⚠️ **这不是"省点流量"**：一份真机诊断日志（677 个任务的批次）里，
        //    `tell*` 的中位耗时是 **600–2500 ms**、最大 9940 ms，而
        //    `getGlobalStat`（只回几个数字）也中位 **912 ms**、最大 9805 ms ——
        //    **aria2 只有一条事件循环**，那几次巨大的序列化把它占住，
        //    排在同一循环里的所有请求（包括那几次只有几个数字的）一起等
        //    ⇒ 撞满 10 秒的 `RPC_TIMEOUT` ⇒ 内核判「引擎已断开」。
        //    整场 41% 的调用失败、`transfer_list` 撞壳的 30 秒超时。
        let keys = task_keys();
        // 尾窗偏移：`max(0, numStopped − 300)`。不足一窗时是 0（从头取，全量也就这么多）。
        let stopped_offset = stopped_count.saturating_sub(STOPPED_WINDOW);
        for (method, range) in [
            ("aria2.tellActive", None),
            ("aria2.tellWaiting", Some((0u64, WAITING_RANGE))),
            ("aria2.tellStopped", Some((stopped_offset, STOPPED_WINDOW))),
        ] {
            // `tellActive` 只收一个可选的 keys 数组，不收 [offset, num]；另两个要
            // （`tellWaiting(offset, num, keys)` / `tellStopped(offset, num, keys)`）。
            // （Go 里就是 `if m != "aria2.tellActive"` 这一句的效果。）
            let params: Vec<serde_json::Value> = match range {
                Some((offset, num)) => vec![
                    serde_json::Value::from(offset),
                    serde_json::Value::from(num),
                    keys.clone(),
                ],
                None => vec![keys.clone()],
            };
            // 第一次出错就返回：不跑完剩下的子请求（契约 §4.3 的代价论证）。
            // 慢档：30 秒读响应、**单次尝试**（规格 §3.2）。
            let raw = self.call_slow(method, &params)?;
            let batch: Vec<RawTask> = serde_json::from_value(raw)
                .map_err(|e| format!("解析 {method} 失败: {e}"))?;
            out.extend(batch);
        }
        Ok(out)
    }

    /// **全量** `tellStopped`——低频动作专用，**不受 [`STOPPED_WINDOW`] 尾窗约束**。
    ///
    /// [`super::daemon::Daemon::clear_finished`] 用它取 forget 集合：尾窗是高频轮询的
    /// 省载荷优化，而清空是低频动作，**必须**把窗口外的老完成项也查出来（不然 `by_gid`
    /// 泄漏，`restore_batch` 会把泄漏路径当 present、「一键开始」静默不补失败文件）。
    ///
    /// `(offset, num) = (0, 1000)`：引擎 `keep-download-result` 的环本身就是 1000 条，
    /// 一次拉全——**不要**拿 [`Self::list`] 的窗口版充数，也**不要**用「by_gid − 存活集」
    /// 反推（`tellWaiting` 同样有 1000 上限，>1000 排队时那个反推会误杀存活 GID）。
    ///
    /// 慢档（与 [`Self::list`] 同档、单次尝试）：千条级 payload，且这是点按钮才走的路。
    pub fn stopped_full(&self) -> Result<Vec<RawTask>, String> {
        let params = [
            serde_json::Value::from(0u64),
            serde_json::Value::from(1000u64),
            task_keys(),
        ];
        let raw = self.call_slow("aria2.tellStopped", &params)?;
        serde_json::from_value(raw).map_err(|e| format!("解析 aria2.tellStopped 失败: {e}"))
    }

    /// 改全局选项（`-j` 并行数、限速），**即时生效**（契约 §2.5）。
    ///
    /// ⚠️ 早先这里写的是"只影响之后加入的任务"——**错的**，实测（aria2 1.37.0）两次：
    ///   - 下载进行中把 `max-overall-download-limit` 从 100K 改到 8M，
    ///     同一个正在传的任务从 ~90KB/s 直接涨到 ~3MB/s；
    ///   - `-j1` 排着两个 waiting 任务时把 `max-concurrent-downloads` 改成 3，
    ///     那两个**立刻**转成 active。
    ///
    /// 界面的设置面板据此可以"改参数不重启引擎"。
    ///
    /// ⚠️ 但它不是设置面板的总开关：`split`、`max-connection-per-server`、`max-tries`、
    /// `retry-wait` 是**逐任务**的量，正确交付方式是随 `add_uri` 一起传，
    /// 而不是"先改全局默认再加任务"——那要与界面并发加任务天然打架。
    pub fn change_global_option(&self, opts: &BTreeMap<String, String>) -> Result<(), String> {
        let obj = serde_json::Value::Object(
            opts.iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        );
        self.call("aria2.changeGlobalOption", &[obj]).map(|_| ())
    }

    /// 让 aria2 退出。
    ///
    /// ⚠️ 收到 `shutdown` 的 aria2 在父进程立刻退出时**不会自己走**（实测孤儿存活 > 48 秒）
    /// ——`Kill` 兜底是承重的，不是装饰（契约 §4.2）。
    pub fn shutdown(&self) -> Result<(), String> {
        self.call("aria2.shutdown", &[]).map(|_| ())
    }

    // ---------------------------------------------------------------------
    // 传输列表动作（任务 11 新增，**非移植**：Go 内核没有这几个封装）
    // ---------------------------------------------------------------------

    /// `aria2.pause` —— 暂停一个任务。
    ///
    /// 实测（内嵌的 aria2 1.37.0）：暂停活动任务成功，任务随即从 `tellActive` 移到
    /// `tellWaiting`，状态字面量变成 `paused`——[`super::status::raw_status_to_state`]
    /// 把 `paused` 与 `waiting` 同义处理（两者都归入"还没在传"）。
    ///
    /// ⚠️ GID 落在 **`params[1]`**：`params[0]` 恒为 `token:<secret>`（[`RpcClient::send`] 补的）。
    /// 形状写错的表现是 aria2 回 400 `The parameter at 0 is required but missing.`（实测）。
    pub fn pause(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.pause", &[serde_json::Value::String(gid.to_string())])
            .map(|_| ())
    }

    /// `aria2.unpause` —— 继续一个被暂停的任务（实测：任务回到 `tellActive`）。
    ///
    /// 参数形状同 [`RpcClient::pause`]。
    pub fn unpause(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.unpause", &[serde_json::Value::String(gid.to_string())])
            .map(|_| ())
    }

    /// 与 [`Self::pause`] 同一条线，但保留"拒绝 / 没回话"的区别（见 [`WriteOutcome`]）。
    pub fn pause_detailed(&self, gid: &str) -> WriteOutcome {
        self.write_outcome("aria2.pause", gid)
    }

    /// `aria2.forcePause` —— 单个任务的**强制**暂停：不等它收尾，直接掐连接。
    ///
    /// 档 2 用它而不用 [`Self::pause_detailed`]：优雅暂停要等数据收尾，连接多时可以拖很久，
    /// 而档 2 要的正是把卡住的连接丢掉。拒绝 / 没回话的区别照旧保留。
    pub fn force_pause_detailed(&self, gid: &str) -> WriteOutcome {
        self.write_outcome("aria2.forcePause", gid)
    }

    /// 与 [`Self::unpause`] 同一条线，但保留"拒绝 / 没回话"的区别（见 [`WriteOutcome`]）。
    pub fn unpause_detailed(&self, gid: &str) -> WriteOutcome {
        self.write_outcome("aria2.unpause", gid)
    }

    /// 写方法不重试（见 [`Self::call`]），所以这里只做一次 `send`，并按 `explicit` 分类。
    fn write_outcome(&self, method: &str, gid: &str) -> WriteOutcome {
        match self.send(&self.agent, method, &[serde_json::Value::String(gid.to_string())]) {
            Ok(_) => WriteOutcome::Applied,
            Err(f) if f.explicit => WriteOutcome::Rejected(f.msg),
            Err(f) => WriteOutcome::NoAnswer(f.msg),
        }
    }

    /// `aria2.forcePauseAll` —— **强制**暂停全部任务（传输页的「暂停全部」）。
    ///
    /// 为什么是 force 而不是 `aria2.pauseAll`：后者是**优雅**暂停——要等各任务把手上
    /// 的数据收尾，连接多时可以拖很久。这个动作的主场景是**磁盘快满了、要立刻停写**，
    /// 等不起。代价是对端连接被直接掐断；`-c` 恒开，续传不受影响。
    ///
    /// 无参数（线上形态就是 `[token]`）。写操作，**不在**重试白名单里（见 `call`）。
    pub fn force_pause_all(&self) -> Result<(), String> {
        self.call("aria2.forcePauseAll", &[]).map(|_| ())
    }

    /// `aria2.unpauseAll` —— 把所有被暂停的任务放回等待队列（传输页的「一键开始」）。
    ///
    /// 无参数。对「没有暂停任务」的引擎也成功（返回 `OK`），所以调用方不必先问。
    pub fn unpause_all(&self) -> Result<(), String> {
        self.call("aria2.unpauseAll", &[]).map(|_| ())
    }

    /// `aria2.tellStatus` 的**单键**查询：只问 `status` 一个键。
    ///
    /// [`super::daemon::Daemon::remove`] 用它回答「这个 GID 停了没有」——
    /// ⚠️ **为什么不用 `list()` 做判据**（0.2.7，规格 §4.2）：`tellStopped` 取尾窗之后，
    /// 列表里**可能看不到**老完成项，判据不能建立在窗口上；单条 `tellStatus` 不受窗口影响。
    ///
    /// ⚠️ **GID 失效（aria2 回 400，如 `GID … is not found`）转成 `Err(原文)`**：
    /// 调用方把 `Err` 按「没停」处理，后续 `aria2.remove` 的错误原文照登——
    /// 与「不在列表里就按没停」的既有语义逐字相同。传输层失败同样是 `Err`，同一处理。
    ///
    /// ⚠️ **GID 落在 `params[1]`**：`params[0]` 恒为 `token:<secret>`（[`RpcClient::send`] 补的）。
    /// 读方法、快档（控制路径上的单条小查，与 [`Self::remove`] 同档）；**不在**
    /// [`is_retryable_read`] 白名单里——单次尝试，判据失败就交回调用方按「没停」走。
    pub fn status(&self, gid: &str) -> Result<String, String> {
        let params = [
            serde_json::Value::String(gid.to_string()),
            serde_json::Value::Array(vec![serde_json::Value::from("status")]),
        ];
        let raw = self.call("aria2.tellStatus", &params)?;
        raw.get("status")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("解析 tellStatus 返回失败: 期望 status 字符串，实际 {raw}"))
    }

    /// `aria2.getServers` —— 读一条 GID 的「腿表」（每个源/镜像此刻的速度）。
    ///
    /// 返回的就是判据 [`super::legs`] 的产出：`Vec<Leg>`，`speed` 单位是字节/秒。
    /// **判决不在这一层**——这一层只把 RPC 的 `result` 交给 [`parse_servers`]。
    ///
    /// ⚠️ **`result` 是一层数组**（实测 aria2 1.37.0）：
    /// `[{"index":"1","servers":[{"uri":…,"downloadSpeed":"1501578"}]}]`，速度是**字符串**。
    /// 这两点都由 [`parse_servers`]（`core/src/engine/legs.rs`）负责，本方法不碰形状。
    ///
    /// ⚠️ **单次尝试、不进 [`is_retryable_read`] 白名单**：调用方是 10 秒一拍的换腿线程
    /// （`kernel::spawn_leg_trim`，在 `core/src/kernel.rs`）。
    /// 传输层抖动就等下一拍——**下一拍本来就是天然重试**。重试在这里的代价是负的：
    /// 每次重试多卡一个 [`RPC_TIMEOUT`]，线程这一拍就漂了，而它重读到的仍是同一份腿表。
    pub fn servers(&self, gid: &str) -> Result<Vec<Leg>, String> {
        let v = self.call("aria2.getServers", &[serde_json::json!(gid)])?;
        Ok(parse_servers(&v))
    }

    /// `aria2.getUris` —— 读一条 GID 的**完整 URI 列表**（任务登记过的全部源，不要求此刻有连接）。
    ///
    /// 为什么必须有它（换腿的域名腿来源）：`getServers` 只列**这一拍真有连接**的 URI，
    /// 域名腿某一拍没有连接就根本不出现 ⇒ 从腿表里找域名腿会**永远找不到**，
    /// 该任务的换腿**静默永久跳过**（无错误、无日志，表现只是"特性整条失效"）。
    /// `getUris` 列的是任务的全部 URI，与"此刻连没连上"无关。
    ///
    /// 实测形状（内嵌 aria2 1.37.0；探针：2 条 URI + `split=4` + `max-connection-per-server=4`）：
    /// `[{"uri":"http://…","status":"used"}, …]`，`status` 只见到 `used` / `waiting` 两种。
    /// ⚠️ **它按连接槽位列，同一 URI 会出现多次**（4 条连接时同一 URI 出现 2 次，
    /// 状态可能一 `used` 一 `waiting`）——所以调用方只能 `find` 第一条满足条件的，
    /// 不能假设"列表里 URI 唯一"，也不该拿它的长度当腿数。
    ///
    /// ⚠️ **单次尝试、不进 [`is_retryable_read`] 白名单**：调用方是 10 秒一拍的换腿线程
    /// （`kernel::spawn_leg_trim`，在 `core/src/kernel.rs`），下一拍就是天然重试——
    /// 理由与 [`Self::servers`] 逐字相同，不重复。
    pub fn get_uris(&self, gid: &str) -> Result<Vec<String>, String> {
        let v = self.call("aria2.getUris", &[serde_json::json!(gid)])?;
        parse_uris(&v)
    }

    /// `aria2.changeUri` —— 换腿：删掉 `del` 里的 URI、加上 `add` 里的 URI。
    ///
    /// `file_index` 单文件恒为 `1`（本产品的 `addUri` 只交一个 URI ⇒ 恒一条 file 条目）。
    /// `del`/`add` 是**字符串数组**，顺序原样进 RPC——顺序是调用方（`plan` 的判决）
    /// 给的，这一层不排序、不去重、不拼串。
    ///
    /// ⚠️ **写方法、绝不重试**：不在 [`is_retryable_read`] 白名单里，与
    /// [`RpcClient::remove`] 等写方法同口径（重试一个非幂等的位置参数，可能把同一条腿
    /// 删/加两遍）。
    ///
    /// ⚠️ **不返回 [`WriteOutcome`]，返回 `Result<(u64, u64), String>`**：`changeUri` 的
    /// NoAnswer（超时/连接断了，aria2 可能已经照做）在这里**没有分支价值**——
    /// 调用方 10 秒后会用 `getServers` 重读腿表对账，改没改以那一拍的实际腿表为准。
    /// 但返回值里的 `[removed, added]` **必须交给调用方**：
    /// 实测（aria2 1.37.0）`del` 里放引擎不认识的 URI 时**不报错、回 `[0, 0]`**，
    /// 所以「我方计划删了几条」不等于「引擎真的删了几条」——诊断计数只能按后者记，
    /// 否则唯一能回答「换腿生效没」的读数就是假的。
    /// 这也解释了为什么它没有 `_detailed` 变体（对比 [`RpcClient::pause_detailed`]：
    /// 那里的调用方没有后续对账，必须当场知道「到底改没改」）。
    pub fn change_uri(
        &self,
        gid: &str,
        file_index: u32,
        del: &[String],
        add: &[String],
    ) -> Result<(u64, u64), String> {
        let v = self.call(
            "aria2.changeUri",
            &[
                serde_json::json!(gid),
                serde_json::json!(file_index),
                serde_json::json!(del),
                serde_json::json!(add),
            ],
        )?;
        Ok(change_uri_counts(&v))
    }

    /// `aria2.remove` —— 移除一个**活动/等待中**的任务。
    ///
    /// **不删除已落盘的部分文件**（`-c` 恒开，重下会续传）。
    ///
    /// ⚠️ **它只对活动/等待中的任务有效**。对已完成/已失败的条目调用，实测
    /// （aria2 1.37.0）返回 400 `Active Download not found for GID#…`（code 1）——
    /// 那种条目要用 [`RpcClient::remove_download_result`]。
    /// **分派是 [`super::daemon::Daemon::remove`] 的责任**，不在这一层：
    /// 本方法就是一条直通的 RPC，不发散、不猜。
    pub fn remove(&self, gid: &str) -> Result<(), String> {
        self.call("aria2.remove", &[serde_json::Value::String(gid.to_string())])
            .map(|_| ())
    }

    /// `aria2.purgeDownloadResult` —— 清空已完成/失败/已移除的历史条目。
    ///
    /// ⚠️ 它是**一次性清空**，没有"选择性地清"这个说法：清什么由 aria2 决定。
    /// 调用方若需要知道"清掉了哪些 GID"，**必须在调用之前**自己取快照
    /// （`tellStopped` 在 purge 之后就是空的）——见 [`super::daemon::Daemon::clear_finished`]。
    ///
    /// 无参数（线上形态就是 `[token]`，实测）。
    pub fn purge_download_result(&self) -> Result<(), String> {
        self.call("aria2.purgeDownloadResult", &[]).map(|_| ())
    }

    /// `aria2.removeDownloadResult` —— 移除**单条**已完成/失败/已移除的历史条目。
    ///
    /// ⚠️ 与 [`RpcClient::remove`] 是**两个不同的方法**，作用域不相交：
    /// 对活动任务调用本方法，实测（aria2 1.37.0）返回 400
    /// `Could not remove download result of GID#…`（code 1）。
    pub fn remove_download_result(&self, gid: &str) -> Result<(), String> {
        self.call(
            "aria2.removeDownloadResult",
            &[serde_json::Value::String(gid.to_string())],
        )
        .map(|_| ())
    }
}

/// 读响应体失败时的一句话（超时、连接中断，或超过 `ureq` 的 10 MB `into_string` 上限）。
///
/// Go 侧没有这条分支（`json.NewDecoder(resp.Body)` 无上限）。RPC 的响应体是几 KB 的 JSON，
/// 它实际不可达——但**不能吞**：吞掉它会伪装成下文那句"响应无法解析"，把"网断了"
/// 报成"格式不对"，正是这个文件开头第 2 条硬约束要防的那类误导。
fn read_body_error(e: std::io::Error) -> String {
    format!("RPC 响应无法读取: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{RawResponse, StubHttp};
    use serde_json::{json, Value};

    /// 桩服务的 JSON-RPC 地址（对应 Go 的 `srv.URL`）。
    fn rpc_url(srv: &StubHttp) -> String {
        format!("{}/jsonrpc", srv.base())
    }

    /// 对应 Go 的 `newStubRPC`：起一个假 aria2 RPC 服务。
    ///
    /// Go 的版本把收到的请求体解成 `(method, params)` 再交给 `handler`，
    /// 并按 handler 的返回拼 JSON 响应体（错误形态回 400 + `error` 对象）。
    /// 这里逐字对应，只多一件事：`id` 原样回显——真 aria2 就是回显的，
    /// 而 Go 的桩写死 `"1"` 是因为它的客户端从不解析 `id`。
    fn new_stub_rpc<F>(handler: F) -> StubHttp
    where
        F: Fn(&str, &[Value]) -> Result<Value, (i64, String)> + Send + Sync + 'static,
    {
        StubHttp::start_with(move |req| {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            let method = body["method"].as_str().unwrap_or("").to_string();
            let params: Vec<Value> = body["params"].as_array().cloned().unwrap_or_default();
            let id = body["id"].clone();
            match handler(&method, &params) {
                Ok(result) => RawResponse::new(200)
                    .body(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()),
                Err((code, msg)) => RawResponse::new(400).body(
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": code, "message": msg},
                    })
                    .to_string(),
                ),
            }
        })
    }

    /// 从原始请求体里取出 `(method, params)`。
    fn method_and_params(raw_body: &str) -> (String, Vec<Value>) {
        let body: Value = serde_json::from_str(raw_body).expect("请求体必须是 JSON");
        (
            body["method"].as_str().unwrap_or("").to_string(),
            body["params"].as_array().cloned().unwrap_or_default(),
        )
    }

    /// 一条最简的 aria2 任务条目（键名照线上形态写）。
    fn entry(gid: &str, status: &str) -> Value {
        json!({
            "gid": gid,
            "status": status,
            "totalLength": "100",
            "completedLength": "0",
            "downloadSpeed": "0",
            "connections": "1",
            "errorMessage": "",
        })
    }

    /// 从原始请求体里取出 `addUri` 的线上形态：`(params 的长度, params[2] 若能当对象看)`。
    ///
    /// 对应 Go 侧那句 `nParams = len(req.Params); if m, ok := req.Params[2].(map[string]any); ok`。
    fn add_uri_options(raw_body: &str) -> (usize, Option<serde_json::Map<String, Value>>) {
        let body: Value = serde_json::from_str(raw_body).expect("请求体必须是 JSON");
        let params = body["params"].as_array().cloned().unwrap_or_default();
        let opts = params.get(2).and_then(|v| v.as_object()).cloned();
        (params.len(), opts)
    }

    /// 对应 Go `TestRPCPingSendsToken`。
    ///
    /// 名字只声明它实际测到的东西：本条只走 `ping` 一条路径。
    /// 别的方法是否带 token 由它们的调用点各自保证（它们都走同一个 `call`）。
    #[test]
    fn rpc_ping_sends_token() {
        let srv = new_stub_rpc(|_m, p| {
            // secret 必须是第一个参数
            if p.first().and_then(|v| v.as_str()) == Some("token:s3cr3t") {
                Ok(json!({}))
            } else {
                Err((1, "Unauthorized".to_string()))
            }
        });
        let c = RpcClient::new(&rpc_url(&srv), "s3cr3t");

        c.ping().expect("带 secret 的调用不应失败");
        assert!(!srv.requests().is_empty(), "没有发出任何 RPC 调用");
    }

    /// 一个**按连接计数**的 aria2 桩：每条连接都正常应答 `getGlobalStat`，
    /// 且**一条连接上能服务多个请求**（keep-alive）。
    ///
    /// 为什么要 keep-alive：本条测试的对照组断言"池内 agent 两次 `global()` 只开一条连接"——
    /// 桩若每条连接只回一次就关，那条断言永远不成立。
    ///
    /// 为什么不用既有的 [`StubHttp`]：它不对外暴露"accept 过几条连接"，而本条的判据正是这个数。
    /// 内部的收发语义与 `testutil::read_request` 同源：按 `Content-Length` 读齐请求体、
    /// 在一条连接上循环读下一个请求。
    ///
    /// 返回 `(JSON-RPC 地址, 已 accept 的连接条数)`。
    fn counting_keepalive_rpc_stub() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read as _, Write as _};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let conns = Arc::new(AtomicUsize::new(0));
        let conns_srv = Arc::clone(&conns);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                // 计数发生在 accept 之后、应答之前：客户端拿到响应的那一刻，
                // 这条连接必定已经计过数了（断言不会与 accept 抢跑）。
                conns_srv.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut acc: Vec<u8> = Vec::new();
                    let mut tmp = [0u8; 4096];
                    loop {
                        let n = s.read(&mut tmp).unwrap_or(0);
                        if n == 0 {
                            break; // 对端关闭
                        }
                        acc.extend_from_slice(&tmp[..n]);
                        // 一条连接上可能连着来多个请求，逐个回。
                        while let Some(pos) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&acc[..pos]).to_string();
                            let cl = head
                                .lines()
                                .filter_map(|l| l.split_once(':'))
                                .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if acc.len() < pos + 4 + cl {
                                break; // 请求体还没到齐
                            }
                            acc.drain(..pos + 4 + cl);
                            let body = r#"{"jsonrpc":"2.0","id":"1","result":{"downloadSpeed":"0","numActive":"0","numWaiting":"0","numStopped":"0"}}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            if s.write_all(resp.as_bytes()).is_err() {
                                return;
                            }
                            let _ = s.flush();
                        }
                    }
                });
            }
        });
        (format!("http://127.0.0.1:{port}/jsonrpc"), conns)
    }

    /// **探活必须走一条全新连接**（规格 §3 A2）。
    ///
    /// 判据是**连接条数**，不是耗时。⚠️ 这一点是实测改正的：原来那条测试靠"第二拍会不会超时"
    /// 来判，而实测发现 **ureq 在超时之后会丢弃那条连接**——于是第二拍本来就开新连接，
    /// 那条判据在**任何**实现下都绿（对池复用零判别力）。连接条数是直接量，且快。
    ///
    /// **对照组**（`global()` 两次只开一条）证明这个计数器**看得见复用**；
    /// **被测组**（`ping()` 两次开两条）证明 ping 没有复用。
    #[test]
    fn ping_never_reuses_a_pooled_connection() {
        use std::sync::atomic::Ordering;

        let (url, conns) = counting_keepalive_rpc_stub();
        let c = RpcClient::new(&url, "s3cr3t");

        // 对照组：`global()` 走**池内** agent ⇒ 两次只开一条连接（证明计数器看得见复用）。
        c.global().expect("对照组第一次 global");
        c.global().expect("对照组第二次 global");
        assert_eq!(
            conns.load(Ordering::SeqCst),
            1,
            "对照组失败：池内 agent 本该复用同一条连接——计数器可能没数准"
        );

        // 被测：`ping()` 两次 ⇒ **再开两条**（合计 3）。若 ping 复用池内连接，这里只会是 1。
        c.ping().expect("第一次 ping");
        c.ping().expect("第二次 ping");
        assert_eq!(
            conns.load(Ordering::SeqCst),
            3,
            "ping 复用了池内连接（只开到 {} 条）—— 探活必须每次走全新连接",
            conns.load(Ordering::SeqCst)
        );
    }

    /// 对应 Go `TestRPCGlobalParsesStringNumbers`。
    ///
    /// aria2 把数字都当字符串传，且线上形态的字段名↔tag 对应关系**没有编译期保护**：
    /// 写错任一 tag，对应字段不会报错，只会静默变成 0——所以四个值各给不同的数，
    /// 任何一个映射错位都会被抓到。
    #[test]
    fn rpc_global_parses_string_numbers() {
        let srv = new_stub_rpc(|m, _p| {
            if m != "aria2.getGlobalStat" {
                return Err((1, format!("意外的方法: {m}")));
            }
            Ok(json!({
                "downloadSpeed": "3",
                "numActive": "2",
                "numWaiting": "1",
                "numStopped": "7",
            }))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s3cr3t");

        let gs = c.global().expect("global() 不应失败");
        assert_eq!(gs.download_speed, 3, "downloadSpeed 映射错误");
        assert_eq!(gs.num_active, 2, "numActive 映射错误");
        assert_eq!(gs.num_waiting, 1, "numWaiting 映射错误");
        assert_eq!(gs.num_stopped, 7, "numStopped 映射错误");
    }

    /// 对应 Go `TestRPCSurfacesAria2ErrorMessage`。
    ///
    /// aria2 把错误放在 JSON 的 `error` 对象里并回 400——
    /// 只报"HTTP 400"会让排查无从下手，必须把 message 带出来。
    #[test]
    fn rpc_surfaces_aria2_error_message() {
        let srv = new_stub_rpc(|_m, _p| Err((1, "Invalid GID x".to_string())));
        let c = RpcClient::new(&rpc_url(&srv), "s3cr3t");

        let err = c.ping().expect_err("应当返回错误");
        assert!(
            err.contains("Invalid GID x"),
            "错误里应含 aria2 的 message，实际: {err:?}"
        );
    }

    /// 对应 Go `TestRPCUnauthorizedIsDistinguishable`。
    ///
    /// 与上一条是同一类断言的第二个样本：`Unauthorized` 与 `Invalid GID x` **都是 400**，
    /// 能区分它们的只有 body 里的 message——这正是"不能先按状态码失败"的理由。
    #[test]
    fn rpc_unauthorized_is_distinguishable() {
        let srv = new_stub_rpc(|_m, _p| Err((1, "Unauthorized".to_string())));
        let c = RpcClient::new(&rpc_url(&srv), "wrong");

        let err = c.ping().expect_err("401 应当可识别");
        assert!(
            err.contains("Unauthorized"),
            "401 应当可识别，实际: {err:?}"
        );
    }

    /// 对应 Go `TestAddURIMergesExtraOptionsAndDirOutWin`。
    ///
    /// `dir`/`out` 必须压过 `extra` 里的同名键；`extra` 为空时退化为只有 `dir`/`out`。
    /// **`options` 在 `params[2]`**：aria2 的线上形态固定是 `[token, uris, options]`。
    /// 写错下标会**永远拿到 null**，于是断言看着像"合并语义坏了"，其实是测试自己没看到请求
    /// ——所以这里同时断言形状（`params` 恰好 3 个、`params[2]` 是个对象）。
    ///
    /// 断言的是**原始请求体**（`SeenRequest::body`），不是任何中间结构：
    /// 短路在客户端内部的"合并语义"再对，线上形状错了也照样白搭。
    #[test]
    fn add_uri_merges_extra_options_and_dir_out_win() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("gid-1")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        // extra 里故意放入与 dir/out 同名的键，且值不同
        let extra: BTreeMap<String, String> = [
            ("split".to_string(), "8".to_string()),
            ("dir".to_string(), "别处".to_string()),
            ("out".to_string(), "别的名字".to_string()),
        ]
        .into_iter()
        .collect();
        c.add_uri(&["http://x/f".to_string()], "PFX/sub", "a.txt", &extra)
            .expect("add_uri 不应失败");

        let (n, opts) = add_uri_options(&srv.requests()[0].body);
        assert_eq!(
            n, 3,
            "addUri 的线上形态应为 [token, uris, options]，实际 params={n}"
        );
        let opts = opts.expect("params[2] 必须是选项对象（下标写错会永远拿到 null）");
        assert_eq!(opts["split"], "8", "extra 的选项应被合并，实际 {opts:?}");
        assert_eq!(opts["dir"], "PFX/sub", "dir 必须压过 extra 里的同名键");
        assert_eq!(opts["out"], "a.txt", "out 必须压过 extra 里的同名键");

        // extra 为空：退化为只有 dir/out
        c.add_uri(&["http://x/f".to_string()], "PFX/sub", "b.txt", &BTreeMap::new())
            .expect("extra 为空不应失败");
        let (n, opts) = add_uri_options(&srv.requests()[1].body);
        assert_eq!(
            n, 3,
            "addUri 的线上形态应为 [token, uris, options]，实际 params={n}"
        );
        let opts = opts.expect("params[2] 必须是选项对象（下标写错会永远拿到 null）");
        assert!(
            opts.len() == 2 && opts["dir"] == "PFX/sub" && opts["out"] == "b.txt",
            "extra 为空时选项应恰好只有 dir/out，实际 {opts:?}"
        );
    }

    /// 多 URI 必须原样进 `params[1]`（规格 §5.1 的 add 链路）。
    ///
    /// 判别力：只发 `uris[0]` 或把列表拼成一个字符串 ⇒ 红。
    /// `params` 形状仍是 `[uris, options]`（线上 `[token, uris, options]`），
    /// 变的只是 `uris` 从单元素变多元素。
    #[test]
    fn add_uri_sends_all_uris_in_params_1() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("gid-1")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        let uris = vec![
            "http://download.benagen.com/C/a.txt".to_string(),
            "http://1.2.3.4/C/a.txt".to_string(),
            "http://5.6.7.8/C/a.txt".to_string(),
        ];
        c.add_uri(&uris, ".", "a.txt", &BTreeMap::new())
            .expect("add_uri 不应失败");

        let body: Value = serde_json::from_str(&srv.requests()[0].body).expect("请求体是 JSON");
        let params = body["params"].as_array().expect("params 必须是数组");
        assert_eq!(params.len(), 3, "线上形态 [token, uris, options]");
        let sent = params[1].as_array().expect("params[1] 必须是 URI 数组");
        let sent: Vec<&str> = sent.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(sent, ["http://download.benagen.com/C/a.txt", "http://1.2.3.4/C/a.txt", "http://5.6.7.8/C/a.txt"],
            "params[1] 必须原样携带全部 URI（顺序不许动）");
    }

    /// 对应 Go `TestRPCRejectsGarbageBody`。
    #[test]
    fn rpc_rejects_garbage_body() {
        let srv = StubHttp::start(vec![RawResponse::new(200).body("这不是 JSON")]);
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert!(c.ping().is_err(), "非法 JSON 应当报错");
    }

    // ------------------------------------------------------------------------
    // **不计入 6 条移植**的 4 条（全局约束 6 的例外，显式记账）。
    //
    // 为什么会有这四条：任务 9 的变异检查里，有两个变异体在 122 条测试下**存活**，
    // 而它们是承重的——①`list` 的拼接顺序（契约 §1.2 的整个论证建立在"已停止的排在后面"上）
    // 与"第一次出错就返回"（§4.3 的代价论证）；②`change_global_option`/`shutdown` 的方法名。
    // Go 的 `rpc_test.go` 一条都不碰这三处，所以这是**逐条移植原样带过来的缺口**，
    // 不是 Rust 侧新引入的。控制者裁定补测（修复轮 1 的 ②③）。
    // ------------------------------------------------------------------------

    /// `list` 的拼接顺序：**active → waiting → stopped**。
    ///
    /// 契约 §1.2 的整个论证建立在"已停止的排在后面"上——正是这一点让
    /// "逐个 apply、后者覆盖前者"（last-write-wins）**确定性地**选到有害的一侧，
    /// `view::compose` 才必须"先按 `task_rank` 挑出唯一胜出者、再应用一次"。
    /// Ruling #73 把这条前提前移进 T9 当硬约束；本条是它的落点。
    ///
    /// 判别力：把拼接顺序换成任何别的排列 → 前两条断言必红。
    #[test]
    fn list_concatenates_active_then_waiting_then_stopped() {
        // 三段的夹具必须能区分开：gid 与 status 都不同，顺序错了才看得出来。
        let srv = new_stub_rpc(|m, _p| match m {
            "aria2.tellActive" => Ok(json!([entry("gid-active", "active")])),
            "aria2.tellWaiting" => Ok(json!([entry("gid-waiting", "waiting")])),
            "aria2.tellStopped" => Ok(json!([entry("gid-stopped", "error")])),
            other => Err((1, format!("意外的方法: {other}"))),
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        let gids: Vec<String> = c
            .list(0)
            .expect("list 不应失败")
            .iter()
            .map(|t| t.gid.clone())
            .collect();
        assert_eq!(
            gids,
            vec!["gid-active", "gid-waiting", "gid-stopped"],
            "拼接顺序必须是 active -> waiting -> stopped（契约 §1.2 的前提）"
        );

        // 只断 gid 不够：顺序对、但方法名或参数打错，照样"看着对"。
        // 参数个数：1 = 只有 token；3 = 多带 [offset, num]。
        let calls: Vec<(String, usize)> = srv
            .requests()
            .iter()
            .map(|r| {
                let (m, p) = method_and_params(&r.body);
                (m, p.len())
            })
            .collect();
        // ⚠️ **参数个数在 2026-10-08 各 +1**：末尾多了一个 **keys 白名单数组**。
        //    少了它的表现**不是报错**，而是每次快照都要 aria2 为**每个任务**返回
        //    **全部字段**（`files[]` 是响应里的大头）—— 一份真机日志（677 个任务的批次）里
        //    `tell*` 中位 600–2500 ms、最大 9940 ms，连只回几个数字的 `getGlobalStat`
        //    也中位 912 ms（aria2 只有一条事件循环，被那几次序列化占住了），
        //    最后一起撞满 `RPC_TIMEOUT` ⇒ 内核判「引擎已断开」。
        assert_eq!(
            calls,
            vec![
                ("aria2.tellActive".to_string(), 2),
                ("aria2.tellWaiting".to_string(), 4),
                ("aria2.tellStopped".to_string(), 4),
            ],
            "方法名与参数形状（只有 tellActive 不带 [offset, num]；三个都要带 keys）"
        );
        let (_, waiting) = method_and_params(&srv.requests()[1].body);
        assert_eq!(waiting[1], json!(0), "tellWaiting 的 offset 应是 0");
        assert_eq!(waiting[2], json!(1000), "tellWaiting 的 num 应是 1000");
        // **只数个数不够**：传一个空数组同样过上面那条，而那等于"不传 keys"、
        // 也就是每次快照都拉全字段。三个请求的 keys 内容都要**逐字**对上白名单。
        let mut want: Vec<String> = TASK_KEYS.iter().map(|k| (*k).to_string()).collect();
        want.sort();
        for r in srv.requests() {
            let (method, p) = method_and_params(&r.body);
            let keys = p
                .last()
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("{method} 的最后一个参数必须是 keys 数组"));
            let mut got: Vec<String> = keys
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            got.sort();
            assert_eq!(got, want, "{method} 的 keys 白名单不对（空数组 = 不传 = 拉全字段）");
        }
    }

    /// `list` 第一次出错就返回，**不**跑完剩下的子请求。
    ///
    /// 契约 §4.3 的代价论证直接建立在这上面：**真死引擎**时"一次快照失败即判定引擎断开"
    /// 的失败上界是**慢档单次 30 秒**（`SNAPSHOT_TIMEOUT`，connection refused 约 0 秒，
    /// 首错即返）；反事实是「跑完三个子请求才报错」= 3×30 秒量级
    /// （旧账「3–6 次 × 30 秒」含重试，慢档不重试后不成立）。
    /// ⚠️ **慢档单次尝试、不重试**（规格 §3.2）：快照类读是周期性的，下一拍就是重试。
    /// ⚠️ **"30 秒"是单次尝试的上界，且含连接阶段**：慢档 agent 显式设了
    /// `.timeout(SNAPSHOT_TIMEOUT)` 与 `.timeout_connect(RPC_TIMEOUT)`（放宽的只是读响应，
    /// 规格 §3 A4）——`slow_connect_phase_is_bounded_by_rpc_timeout` 守着这一点。
    /// 先跑完会把最坏情况拖成三倍，也就把"为它多等几轮"这个被否掉的做法又请了回来。
    ///
    /// ⚠️ 本条测试的桩回的是**协议层**错误（`error` 对象），不在重试之列，所以那两条
    /// "子请求数 = 1 / 2"的断言与 A1 的重试**无关**，读数不受影响。
    ///
    /// 判别力：把提前返回改成"跑完再报错"→ 两条断言都会红（请求数变成 3 / 3）；
    /// 只对第一个子请求提前返回、后两个的错误被吞掉 → 场景 B 必红。
    ///
    /// ⚠️ 桩故意按**收到的第几个请求**决定成败，而不是按方法名：这样本条与拼接顺序**无关**
    /// （顺序由上面那条测试单独钉住）。否则改顺序会让两条测试一起红，
    /// 让人分不清"顺序错了"还是"提前返回错了"。
    #[test]
    fn list_returns_on_first_error_without_querying_the_rest() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use std::sync::Arc;

        // 只有**第 `fail_at` 个子请求**（从 0 数）失败，其余成功。
        let stub_failing_at = |fail_at: usize| -> StubHttp {
            let n = Arc::new(AtomicUsize::new(0));
            let n_in = Arc::clone(&n);
            new_stub_rpc(move |_m, _p| {
                if n_in.fetch_add(1, AtomicOrdering::SeqCst) == fail_at {
                    Err((1, "boom".to_string()))
                } else {
                    Ok(json!([]))
                }
            })
        };

        // 场景 A：第一个子请求就失败 —— 一个后续请求都不该发
        let srv_a = stub_failing_at(0);
        let c_a = RpcClient::new(&rpc_url(&srv_a), "s");
        let err = c_a.list(0).expect_err("第一个子请求出错时 list 必须报错");
        assert!(err.contains("boom"), "aria2 的错误必须带出来: {err:?}");
        assert_eq!(
            srv_a.requests().len(),
            1,
            "第一次出错就必须返回，不得再发后续子请求"
        );

        // 场景 B：第二个子请求失败 —— 第三个不该发
        let srv_b = stub_failing_at(1);
        let c_b = RpcClient::new(&rpc_url(&srv_b), "s");
        assert!(c_b.list(0).is_err(), "第二个子请求出错时 list 必须报错");
        assert_eq!(
            srv_b.requests().len(),
            2,
            "第二次出错后不得再发第三个子请求"
        );
    }

    /// 🔴 **`tellStopped` 取尾窗**（规格 §4）：最旧在前（实验证明）⇒ 截头显示陈旧完成、
    /// 藏起刚完成的；`offset = max(0, numStopped−300)` 才是「最近 300 条」。
    ///
    /// 判别力：写死 offset=0 ⇒ 第一条红；窗口算反（加而不是减）⇒ 第二条红。
    ///
    /// ⚠️ **线上参数形状是 `[token, offset, num, keys]`**（[`RpcClient::send`] 恒在
    /// `params[0]` 补 token）——所以偏移落在**下标 1**、窗口落在**下标 2**，
    /// 与 `list_concatenates_active_then_waiting_then_stopped` 里 `waiting[1]`/`waiting[2]`
    /// 的取法逐字相同。
    #[test]
    fn tell_stopped_asks_for_the_recent_window_not_the_oldest() {
        let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<Value>)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_srv = seen.clone();
        let srv = new_stub_rpc(move |m, p| {
            seen_srv.lock().unwrap().push((m.to_string(), p.to_vec()));
            Ok(match m {
                "aria2.tellActive" | "aria2.tellWaiting" | "aria2.tellStopped" => json!([]),
                _ => json!({"downloadSpeed": "0", "numActive": "0", "numWaiting": "0", "numStopped": "0"}),
            })
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.list(1000).unwrap();
        let stopped = seen.lock().unwrap().iter()
            .find(|(m, _)| m == "aria2.tellStopped").cloned().expect("发过 tellStopped");
        assert_eq!(stopped.1[1], json!(700), "尾窗偏移要等于 numStopped−300：{:?}", stopped.1);
        assert_eq!(stopped.1[2], json!(300), "窗口大小是 STOPPED_WINDOW：{:?}", stopped.1);

        seen.lock().unwrap().clear();
        let c2 = RpcClient::new(&rpc_url(&srv), "s");
        c2.list(20).unwrap();
        let stopped2 = seen.lock().unwrap().iter()
            .find(|(m, _)| m == "aria2.tellStopped").cloned().expect("发过 tellStopped");
        assert_eq!(stopped2.1[1], json!(0), "不足一窗时 offset 必须是 0：{:?}", stopped2.1);
    }

    /// `tellWaiting` 保持队头 1000 条（值与今天逐字相同；已暂停任务住在 waiting 表里——
    /// `BatchControl` 的「一键开始」判据要看得见它们）。
    #[test]
    fn tell_waiting_keeps_the_queue_head_at_1000() {
        let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<Value>)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_srv = seen.clone();
        let srv = new_stub_rpc(move |m, p| {
            seen_srv.lock().unwrap().push((m.to_string(), p.to_vec()));
            Ok(match m {
                "aria2.tellActive" | "aria2.tellWaiting" | "aria2.tellStopped" => json!([]),
                _ => json!({"downloadSpeed": "0", "numActive": "0", "numWaiting": "0", "numStopped": "0"}),
            })
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.list(0).unwrap();
        let waiting = seen.lock().unwrap().iter()
            .find(|(m, _)| m == "aria2.tellWaiting").cloned().expect("发过 tellWaiting");
        assert_eq!(waiting.1[1], json!(0), "tellWaiting 的 offset 不许动：{:?}", waiting.1);
        assert_eq!(waiting.1[2], json!(1000), "tellWaiting 的 (0,1000) 不许动：{:?}", waiting.1);
    }

    /// `change_global_option` 打在**精确**的方法名上。
    ///
    /// 方法名写错是**静默失败**：aria2 回 `Method not found`，而调用方若不看错误就什么都不发生
    /// ——客户改了限速却没生效，界面上什么都不会说。Go 的 6 条测试完全没碰这个方法。
    ///
    /// 判别力：把方法名改成 `changeGlobalOption`（少了 `aria2.`）或任何拼写 → 第一条断言必红。
    #[test]
    fn change_global_option_sends_exact_method_name() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("OK")));
        let c = RpcClient::new(&rpc_url(&srv), "s");
        let mut opts = BTreeMap::new();
        opts.insert("max-concurrent-downloads".to_string(), "8".to_string());
        opts.insert("max-overall-download-limit".to_string(), "0".to_string());

        c.change_global_option(&opts)
            .expect("change_global_option 不应失败");

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.changeGlobalOption", "方法名必须逐字正确");
        assert_eq!(
            params.len(),
            2,
            "线上形态是 [token, options]——options 在 token 之后的**一个**参数里"
        );
        assert_eq!(
            params[1]["max-concurrent-downloads"], "8",
            "选项要整份传过去，实际 {params:?}"
        );
        assert_eq!(params[1]["max-overall-download-limit"], "0");
    }

    /// `shutdown` 打在**精确**的方法名上（理由同 `change_global_option`）。
    ///
    /// 判别力：方法名改一个字母 → 第一条断言必红。
    #[test]
    fn shutdown_sends_exact_method_name() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("OK")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.shutdown().expect("shutdown 不应失败");

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.shutdown", "方法名必须逐字正确");
        assert_eq!(params.len(), 1, "线上形态是 [token]");
    }

    // ------------------------------------------------------------------------
    // 换腿（读腿表 / 换腿）：`aria2.getServers` 与 `aria2.changeUri`。
    //
    // ⚠️ 这两条都是**周期性的后台调用**（换腿线程 `spawn_leg_trim` 10 秒一拍），
    // 不是界面上的点一下就走的动作；`servers` 因此与 `status()` 同口径——单次尝试、
    // 不重试（下一拍就是天然重试）。下面三条钉的全是**线上形状**：方法名逐字、
    // 参数位置与类型、以及写方法绝不重试。
    // ------------------------------------------------------------------------

    /// `servers` 打在**精确**的方法名上，并把一层数组的真实形状解析成腿表。
    ///
    /// 夹具用**真实 aria2 1.37.0 的形状**（一层数组、`downloadSpeed` 是字符串）：
    /// 换成两层数组或数字速率时，红的会是 `legs.rs` 的既有判据而不是这里——
    /// 所以夹具必须与线上一致，否则这条测试只是在重复 `legs.rs` 的错误假设。
    ///
    /// 判别力：方法名写错（`getServer`）⇒ 第一条断言红；gid 没落在 `params[1]`
    /// ⇒ 第二条红；`servers` 忘了接 `parse_servers` ⇒ 腿表断言红。
    #[test]
    fn servers_sends_exact_method_name_and_parses() {
        let srv = new_stub_rpc(|_m, _p| {
            Ok(json!([{
                "index": "1",
                "servers": [
                    {
                        "uri": "http://1.2.3.4/x",
                        "currentUri": "http://1.2.3.4/x",
                        "downloadSpeed": "1501578"
                    },
                    {
                        "uri": "http://5.6.7.8/x",
                        "currentUri": "http://5.6.7.8/x",
                        "downloadSpeed": "90000"
                    }
                ]
            }]))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        let legs = c.servers("gid-1").expect("servers 不应失败");

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.getServers", "方法名必须逐字正确");
        assert_eq!(
            params.len(),
            2,
            "线上形态是 [token, gid]，实际 {params:?}（多一个/少一个参数 aria2 都会报参数错）"
        );
        assert_eq!(params[1], "gid-1", "GID 必须落在 params[1]");
        assert_eq!(
            legs,
            vec![
                Leg {
                    uri: "http://1.2.3.4/x".to_string(),
                    speed: 1_501_578
                },
                Leg {
                    uri: "http://5.6.7.8/x".to_string(),
                    speed: 90_000
                },
            ],
            "返回的腿表必须与真实形状的应答逐字一致（两条腿的 uri 与 speed）"
        );
    }

    /// 引擎态错误（GID 已消失）必须**冒出来**，不许退化成空腿表。
    ///
    /// 桩回 400 + `{"code":1,"message":"GID gid-gone is not found"}`——这是 aria2 1.37.0
    /// 对失效 GID 的真实应答形状。断的是**错误原文被带出来**（GID 与 `not found` 都在）。
    ///
    /// ⚠️ 这条守的是**一个静默失效**，不是文案：有人若把 `servers` 改成
    /// `self.call(...).map(parse_servers).unwrap_or_default()`，编译照样过、类型照样对，
    /// 但引擎态错误会变成**空腿表** ⇒ 上层的判决以为「这条 GID 没有腿」⇒ 整条换腿特性
    /// **无声失效**（不报错、不记日志，表现只是「从不换腿」）。换腿是优化不是正确性，
    /// 所以这种退化不会被别的判据发现——只能在这一层钉住"错误必须冒出来"。
    ///
    /// 判别力：把 `servers` 改成 `unwrap_or_default()` ⇒ 本用例第一条断言红
    /// （`servers` 回 `Ok(空表)` 而不是 `Err`）。
    #[test]
    fn servers_surfaces_engine_error() {
        let srv = new_stub_rpc(|_m, _p| Err((1, "GID gid-gone is not found".to_string())));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        let e = c
            .servers("gid-gone")
            .expect_err("引擎态错误必须冒出来，不得静默退化成空腿表");

        assert!(
            e.contains("gid-gone"),
            "错误必须带着失效的 GID（否则无法定位是哪条腿表读砸了），实际: {e:?}"
        );
        assert!(
            e.contains("not found"),
            "错误必须带着 aria2 的 message 原文（`not found`），实际: {e:?}"
        );
    }

    /// `change_uri` 的参数形状：`[token, gid, fileIndex, delUris, addUris]`。
    ///
    /// 两处承重：
    ///  * `fileIndex` 必须是**数字**（线上是 `1`）。本产品单文件 `addUri` ⇒ 恒为 1，
    ///    但写成字符串 `"1"` 时 aria2 按整数解析位置参数，会回 400。
    ///    ⚠️ 本用例**故意传 `3`**（不是 1）：传 1 时，把 `json!(file_index)` 写成
    ///    写死的 `json!(1)` 也照样绿——那正是「参数被偷偷写死」的变异，必须能红。
    ///    daemon 层那条委托用例同样传 3，两层各自守得住。
    ///  * `del`/`add` 必须是**字符串数组**、且顺序逐字保留——顺序是调用方
    ///    （`plan` 的判决）给的，这里不能排序、去重，也不能拼成逗号串。
    #[test]
    fn change_uri_sends_exact_params() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!([1, 1])));
        let c = RpcClient::new(&rpc_url(&srv), "s");
        let del = vec![
            "http://1.2.3.4/x".to_string(),
            "http://1.2.3.5/y".to_string(),
        ];
        let add = vec![
            "http://9.9.9.9/x".to_string(),
            "http://8.8.8.8/y".to_string(),
        ];

        let counts = c
            .change_uri("gid-1", 3, &del, &add)
            .expect("change_uri 不应失败");

        assert_eq!(
            counts,
            (1, 1),
            "changeUri 的 [removed, added] 必须原样返回给调用方（诊断只能按引擎的实际计数记）"
        );

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.changeUri", "方法名必须逐字正确");
        assert_eq!(
            params.len(),
            5,
            "线上形态是 [token, gid, fileIndex, delUris, addUris]，实际 {params:?}"
        );
        assert_eq!(params[1], "gid-1", "GID 必须落在 params[1]");
        assert!(
            params[2].is_number(),
            "fileIndex 必须是 JSON 数字，不能是字符串：实际 {:?}",
            params[2]
        );
        assert_eq!(
            params[2],
            json!(3),
            "fileIndex 必须原样透传调用方给的 3——写死成 1 就是偷偷改参数"
        );
        assert_eq!(
            params[3],
            json!(["http://1.2.3.4/x", "http://1.2.3.5/y"]),
            "delUris 必须是字符串数组且顺序逐字保留"
        );
        assert_eq!(
            params[4],
            json!(["http://9.9.9.9/x", "http://8.8.8.8/y"]),
            "addUris 必须是字符串数组且顺序逐字保留"
        );
    }

    /// 🔴 **引擎回 `[0, 0]` 时必须如实返回 `(0, 0)`**，不许拿我方计划冒充结果。
    ///
    /// 实测（aria2 1.37.0）：`del` 里放引擎不认识的 URI 时**不报错**、回 `[0, 0]`。
    /// 旧实现只要 `Ok(())` 就按 `plan.del.len()` / `plan.add.len()` 记账，于是「一条没删」
    /// 被记成「删了」——唯一能回答「换腿生效没」的诊断读数直接失真。
    ///
    /// 判别力：把 `change_uri` 改成 `Ok((del.len() as u64, add.len() as u64))`
    /// （即回到「按我方计划记账」）⇒ 本条红（会得到 `(1, 1)` 而不是 `(0, 0)`）。
    #[test]
    fn change_uri_reports_the_engine_counts_not_our_plan() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!([0, 0])));
        let c = RpcClient::new(&rpc_url(&srv), "s");
        let del = vec!["http://192.0.2.1/nope.bin".to_string()];
        let add = vec!["http://9.9.9.9/x".to_string()];

        let counts = c
            .change_uri("gid-1", 1, &del, &add)
            .expect("引擎不认识这些 URI 也不是错误：它已经应答了");

        assert_eq!(
            counts,
            (0, 0),
            "引擎回 [0,0]（一条都没删/没加）时不许记成删了：诊断必须按引擎的实际结果记"
        );
    }

    /// 🔴 **`change_uri` 绝不重试**：写方法不在 [`is_retryable_read`] 白名单里。
    ///
    /// 与 `a_non_retryable_write_is_counted_exactly_once` 同构，但钉的是**本方法**：
    /// 把它接到某条会重试的路径上（或误加进白名单）⇒ 往返计数变 2 ⇒ 红。
    /// 重试的代价在换腿线程上尤其糟：10 秒一拍，重试把最坏等待翻倍成 2×10 秒。
    #[test]
    fn change_uri_is_not_retried_on_transport_failure() {
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client = RpcClient::new("http://127.0.0.1:1/jsonrpc", "s")
            .with_counters(std::sync::Arc::clone(&c));
        let del = vec!["http://1.2.3.4/x".to_string()];
        let add = vec!["http://9.9.9.9/x".to_string()];

        let e = client
            .change_uri("gid-1", 1, &del, &add)
            .expect_err("掐断的连接上 changeUri 必须失败");
        assert!(e.contains("RPC 请求失败"), "错误文案变了：{e}");
        let s = c.snapshot();
        assert_eq!(
            s.rpc_calls, 1,
            "change_uri 被重试了 —— 写方法不在 is_retryable_read 白名单里，只许发一次"
        );
        assert_eq!(s.rpc_failures, 1);
    }

    /// 🔴 **`servers` 是读方法，但故意不重试**：它也不在 [`is_retryable_read`] 白名单里。
    ///
    /// 与 `change_uri_is_not_retried_on_transport_failure` 同形，钉的是**反直觉的那一半**：
    /// 白名单里另外四个 `tell*`/`getGlobalStat` 都是读方法，很容易顺手把 `getServers`
    /// 也加进去——而它必须留在外面。理由是调用方的节奏：换腿线程 **10 秒一拍**，
    /// 传输层抖动时**下一拍本来就是天然重试**；在这里重试只会让这一拍多卡一个
    /// [`RPC_TIMEOUT`]，线程跟着漂，而重读到的仍是同一份腿表（甚至同一份过期的）。
    ///
    /// 判别力：把 `aria2.getServers` 加进 `is_retryable_read` ⇒ `rpc_calls` 变 2 ⇒ 本条红
    /// （审查者实测过：加进白名单后既有 6 条腿表用例全绿，即此前无人守这条承诺）。
    #[test]
    fn servers_is_not_retried_on_transport_failure() {
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client = RpcClient::new("http://127.0.0.1:1/jsonrpc", "s")
            .with_counters(std::sync::Arc::clone(&c));

        let e = client
            .servers("gid-1")
            .expect_err("掐断的连接上 getServers 必须失败");
        assert!(e.contains("RPC 请求失败"), "错误文案变了：{e}");
        let s = c.snapshot();
        assert_eq!(
            s.rpc_calls, 1,
            "getServers 被重试了 —— 读方法也不许重试：10 秒一拍的下一拍就是天然重试，\
             重试只多卡一个超时"
        );
        assert_eq!(s.rpc_failures, 1);
    }

    /// `get_uris` 打在**精确**的方法名上，并按实测形状取出 URI 列表。
    ///
    /// 夹具是真实抓包（aria2 1.37.0，`split=4` + `max-connection-per-server=4`）：
    /// `[{"uri":…,"status":"used"}, …]`，**同一 URI 按连接槽位出现多次**。
    /// 这里有意保留重复：它钉的是「解析器不许去重/不许按长度当腿数」——
    /// 去重会把 `waiting` 槽位里的域名腿丢掉，而调用方只需要 `find` 第一条。
    ///
    /// 判别力：方法名写错 ⇒ 第一条断言红；gid 没落在 `params[1]` ⇒ 第二条红；
    /// 解析时按 `status == "used"` 过滤 ⇒ URI 列表少了 `waiting` 那条 ⇒ 第三条红。
    #[test]
    fn get_uris_sends_exact_method_name_and_parses() {
        let srv = new_stub_rpc(|_m, _p| {
            Ok(json!([
                {"uri": "http://127.0.0.1:8733/big.bin", "status": "used"},
                {"uri": "http://localhost:8733/big.bin", "status": "used"},
                {"uri": "http://127.0.0.1:8733/big.bin", "status": "waiting"},
                {"uri": "http://localhost:8733/big.bin", "status": "waiting"}
            ]))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        let uris = c.get_uris("gid-1").expect("get_uris 不应失败");

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.getUris", "方法名必须逐字正确");
        assert_eq!(
            params.len(),
            2,
            "线上形态是 [token, gid]，实际 {params:?}（getUris 没有 fileIndex 参数）"
        );
        assert_eq!(params[1], "gid-1", "GID 必须落在 params[1]");
        assert_eq!(
            uris,
            vec![
                "http://127.0.0.1:8733/big.bin".to_string(),
                "http://localhost:8733/big.bin".to_string(),
                "http://127.0.0.1:8733/big.bin".to_string(),
                "http://localhost:8733/big.bin".to_string(),
            ],
            "URI 列表必须逐字保留线上顺序与重复（同 URI 按连接槽位出现多次）"
        );
    }

    /// 🔴 **`get_uris` 是读方法，但故意不重试**：它不在 [`is_retryable_read`] 白名单里。
    ///
    /// 与 `servers_is_not_retried_on_transport_failure` 同形、同一理由：调用方是 10 秒一拍的
    /// 换腿线程，下一拍就是天然重试；在这里重试只会让这一拍多卡一个 [`RPC_TIMEOUT`]。
    /// 它**故意不进** `retryable_reads_are_exactly_the_read_methods` 的两段列表：
    /// 那条用例守的是"读方法可重试 / 写方法不可重试"，而本例守的正是"读方法也不重试"。
    ///
    /// 判别力：把 `aria2.getUris` 加进 `is_retryable_read` ⇒ `rpc_calls` 变 2 ⇒ 本条红。
    #[test]
    fn get_uris_is_not_retried_on_transport_failure() {
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client = RpcClient::new("http://127.0.0.1:1/jsonrpc", "s")
            .with_counters(std::sync::Arc::clone(&c));

        let e = client
            .get_uris("gid-1")
            .expect_err("掐断的连接上 getUris 必须失败");
        assert!(e.contains("RPC 请求失败"), "错误文案变了：{e}");
        let s = c.snapshot();
        assert_eq!(
            s.rpc_calls, 1,
            "getUris 被重试了 —— 读方法也不许重试：10 秒一拍的下一拍就是天然重试，\
             重试只多卡一个超时"
        );
        assert_eq!(s.rpc_failures, 1);
    }

    // ------------------------------------------------------------------------
    // 传输列表动作（**任务 11 新增，非移植**——Go 内核没有 pause/unpause/remove 的封装，
    // 这一段没有任何可移植的测试源，从零设计）。
    //
    // 每个方法两条：① 正常路径（打在精确的方法名上、GID 落在 `params[1]`）；
    // ② 非法 GID（aria2 回 400 且把详情放在 body 的 `error` 里，错误必须**带出来**，
    //    不得被吞成 `Ok(())`）。
    //
    // ⚠️ 为什么"吞成 Ok"这一条必须有：这三个动作的调用方是界面。错误被吞掉之后，
    // 用户点"暂停/移除"什么都不会发生，**也没有任何提示**——正是规格 §9 那条
    // "任何一类失败都必须出现在界面上"要防的形状。
    //
    // 断言的是**原始请求体**（`SeenRequest::body`），不是任何中间结构：
    // 短路在客户端内部的"参数拼装"再对，线上形状错了也照样白搭。
    // ------------------------------------------------------------------------

    /// 带 GID 的那几个方法的线上形态：方法名逐字正确、`params` 恰好 `[token, gid]`。
    ///
    /// 下标是承重的：GID 必须落在 `params[1]`。少了它（`params.len() == 1`）aria2 实测回
    /// 400 `The parameter at 0 is required but missing.`；而把 GID 顶到 `params[0]`
    /// 会把 token 挤走，回的是 `Unauthorized`——两种都是 400，不看请求体分不出来。
    fn assert_gid_call(raw_body: &str, want_method: &str, want_gid: &str) {
        let (method, params) = method_and_params(raw_body);
        assert_eq!(method, want_method, "方法名必须逐字正确");
        assert_eq!(
            params.len(),
            2,
            "线上形态是 [token, gid]，实际 {params:?}（多一个/少一个参数 aria2 都会报参数错）"
        );
        assert_eq!(
            params[0], "token:s",
            "params[0] 必须仍是 token——GID 不得把它挤走"
        );
        assert_eq!(params[1], want_gid, "GID 必须落在 params[1]");
    }

    /// 从桩收到的 `params` 里**安全地**取出 GID（`params[1]`），取不到给空串。
    ///
    /// ⚠️ 为什么用 `get(1)` 而**不是** `p[1]`（修复轮 2 的 ④）：客户端少发参数时
    /// （例如变异体 M8 把 GID 漏了），`p[1]` 会在**桩线程**里 panic，诊断信息退化成
    /// 一句线程 panic；测试照样会红，但**没有人看得懂是为什么**。
    /// `get(1)` 让桩照常回一条 `GID  is not found`，红的是断言、并且说得清形状不对。
    fn gid_of(p: &[Value]) -> String {
        p.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    /// 非法 GID 的错误必须带出来。
    ///
    /// 实测（内嵌的 aria2 1.37.0）：对不存在的 GID，`pause`/`unpause`/`remove`/
    /// `removeDownloadResult` 都回 **400** + `error: {code: 1, message: "GID <gid> is not found"}`。
    /// ⚠️ 简报里把这个文案记成了 `Invalid GID`——**实测不是**，1.37.0 的实际文案是
    /// `GID <gid> is not found`（探针输出见任务 11 报告）。这里断的是**带出了 message**
    /// 这件事，不是那段具体文案。
    fn assert_error_surfaces(res: Result<(), String>, what: &str, want_substr: &str) {
        let err = match res {
            Ok(()) => panic!("{what} 对非法 GID 必须返回错误，不能吞成 Ok(())"),
            Err(e) => e,
        };
        assert!(
            err.contains(want_substr),
            "{what} 必须把 aria2 的 message 带出来（否则 400 与 Unauthorized 无法区分），实际: {err:?}"
        );
    }

    /// `aria2.pause` 打在精确的方法名上，GID 落在 `params[1]`。
    #[test]
    fn rpc_pause_sends_exact_method_name_and_gid() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("gid-1")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.pause("gid-1").expect("pause 不应失败");

        assert_gid_call(&srv.requests()[0].body, "aria2.pause", "gid-1");
    }

    /// `pause` 对非法 GID 必须报错，且错误里带着 aria2 的 message。
    #[test]
    fn rpc_pause_surfaces_invalid_gid() {
        let srv = new_stub_rpc(|_m, p| {
            Err((1, format!("GID {} is not found", gid_of(p))))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert_error_surfaces(c.pause("abc"), "pause", "GID abc is not found");
    }

    /// `aria2.unpause` 打在精确的方法名上，GID 落在 `params[1]`。
    #[test]
    fn rpc_unpause_sends_exact_method_name_and_gid() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("gid-1")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.unpause("gid-1").expect("unpause 不应失败");

        assert_gid_call(&srv.requests()[0].body, "aria2.unpause", "gid-1");
    }

    /// `unpause` 对非法 GID 必须报错。
    #[test]
    fn rpc_unpause_surfaces_invalid_gid() {
        let srv = new_stub_rpc(|_m, p| {
            Err((1, format!("GID {} is not found", gid_of(p))))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert_error_surfaces(c.unpause("abc"), "unpause", "GID abc is not found");
    }

    /// `forcePauseAll` / `unpauseAll` 打在精确的方法名上，且**没有**位置参数
    /// （`params` 里只有 `RpcClient` 补的 `token:<secret>`）。
    #[test]
    fn rpc_pause_all_and_unpause_all_send_exact_method_names_without_args() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("OK")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.force_pause_all().expect("force_pause_all 不应失败");
        c.unpause_all().expect("unpause_all 不应失败");

        let reqs = srv.requests();
        assert_eq!(reqs.len(), 2, "两个动作各发一次，实际 {}", reqs.len());
        for (req, want) in reqs.iter().zip(["aria2.forcePauseAll", "aria2.unpauseAll"]) {
            let v: serde_json::Value = serde_json::from_str(&req.body).expect("请求体应是 JSON");
            assert_eq!(v["method"], want, "方法名不对: {v}");
            assert_eq!(
                v["params"].as_array().map(Vec::len),
                Some(1),
                "只该有 token 一个参数: {v}"
            );
        }
    }

    /// 暂停全部是写操作：**失败不许被自动重试**（重试白名单只有四个读方法）。
    /// 判别力：把 `aria2.forcePauseAll` 加进白名单 ⇒ 桩会收到两次 ⇒ 本条红。
    #[test]
    fn rpc_pause_all_failure_is_not_retried() {
        let srv = new_stub_rpc(|_m, _p| Err((1, "boom".to_string())));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert!(c.force_pause_all().is_err());
        assert_eq!(srv.requests().len(), 1, "写操作失败不得重试");
    }

    /// `aria2.remove` 打在精确的方法名上，GID 落在 `params[1]`。
    ///
    /// 判据里**不含**"已完成的任务该走哪个方法"——那是 `Daemon::remove` 的分派责任，
    /// 这一层只是一条直通的 RPC（见 `RpcClient::remove` 的文档注释）。
    #[test]
    fn rpc_remove_sends_exact_method_name_and_gid() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("gid-1")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.remove("gid-1").expect("remove 不应失败");

        assert_gid_call(&srv.requests()[0].body, "aria2.remove", "gid-1");
    }

    /// `remove` 对非法 GID 必须报错（实测文案是 `Active Download not found` 那句的近亲）。
    #[test]
    fn rpc_remove_surfaces_invalid_gid() {
        let srv = new_stub_rpc(|_m, p| {
            Err((1, format!("GID {} is not found", gid_of(p))))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert_error_surfaces(c.remove("abc"), "remove", "GID abc is not found");
    }

    /// `aria2.purgeDownloadResult` 打在精确的方法名上，且**不带多余参数**
    /// （线上形态就是 `[token]`，实测）。
    #[test]
    fn rpc_purge_download_result_sends_exact_method_name() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("OK")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.purge_download_result().expect("purge 不应失败");

        let (method, params) = method_and_params(&srv.requests()[0].body);
        assert_eq!(method, "aria2.purgeDownloadResult", "方法名必须逐字正确");
        assert_eq!(params.len(), 1, "线上形态是 [token]，实际 {params:?}");
    }

    /// `purgeDownloadResult` 出错时必须报错，不能吞成 `Ok(())`。
    ///
    /// 这一条没有"非法 GID"可造（该方法不收 GID），所以造的是**任何一次 400**：
    /// 吞掉它意味着"清空已完成"在失败时静默通过，而 `clear_finished` 随后会照常
    /// `forget` 一批根本没被清掉的 GID——列表上还在、映射却没了。
    #[test]
    fn rpc_purge_download_result_surfaces_error() {
        let srv = new_stub_rpc(|_m, _p| Err((1, "Unauthorized".to_string())));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert_error_surfaces(
            c.purge_download_result(),
            "purgeDownloadResult",
            "Unauthorized",
        );
    }

    /// `aria2.removeDownloadResult` 打在精确的方法名上，GID 落在 `params[1]`。
    ///
    /// 与 `aria2.remove` **只差两个词**——正是这种"看着差不多"的方法名最容易写错，
    /// 而写错的后果是静默失败（aria2 回 `No such method`，吞掉错误就什么都不发生）。
    #[test]
    fn rpc_remove_download_result_sends_exact_method_name_and_gid() {
        let srv = new_stub_rpc(|_m, _p| Ok(json!("OK")));
        let c = RpcClient::new(&rpc_url(&srv), "s");

        c.remove_download_result("gid-1")
            .expect("removeDownloadResult 不应失败");

        assert_gid_call(
            &srv.requests()[0].body,
            "aria2.removeDownloadResult",
            "gid-1",
        );
    }

    /// `removeDownloadResult` 对非法 GID 必须报错。
    #[test]
    fn rpc_remove_download_result_surfaces_invalid_gid() {
        let srv = new_stub_rpc(|_m, p| {
            Err((1, format!("GID {} is not found", gid_of(p))))
        });
        let c = RpcClient::new(&rpc_url(&srv), "s");

        assert_error_surfaces(
            c.remove_download_result("abc"),
            "removeDownloadResult",
            "GID abc is not found",
        );
    }

    // ------------------------------------------------------------------------
    // 传输层重试（**任务 4 新增，非移植**——规格 §3 A1）。
    //
    // 一段话讲清这三条的分工：① 读方法在传输层失败后**必须**换新连接重试一次；
    // ② 写方法**一次都不许**重试；③ 白名单**逐字**钉住（它是 ① 与 ② 的判别边界）。
    // ------------------------------------------------------------------------

    /// **读方法在传输层失败后，必须换一条新连接重试一次**（规格 §3 A1）。
    ///
    /// ⚠️ **桩的规矩是这条测试的关键，别照更早那一版抄**：每条连接**只答第一个请求，之后装死**
    /// （不回话、也不关连接）。为什么必须先答一次——**ureq 在一次调用超时之后会丢弃那条连接**
    /// （Task 3 实测），所以"池里握着一条已经坏掉的连接"这个形状**必须先成功过一次**、
    /// 把连接放进池子，那条连接才归我们摆布。
    /// 更早那一版"第一条连接收下就掐断"的桩**触发不到重试路径**：池内 agent 手上一条连接都没有，
    /// 第一次 `global()` 就开了新连接、直接成功，一次重试也不会发生。
    ///
    /// ⚠️ **应答必须回显请求自己的 `id`**（真 aria2 就是回显的）：客户端会把 id 不符判成
    /// `Desync`。第一拍用 id=1，第二拍走新连接时 id 已经是 2——写死 `"id":"1"` 会让重试那一拍
    /// 撞出 `Desync` 而不是成功，测试会以一个**与本次改动无关**的理由红。
    #[test]
    fn a_transport_failure_on_a_read_is_retried_on_a_fresh_connection() {
        let conns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let conns_srv = std::sync::Arc::clone(&conns);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                conns_srv.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 65536];
                    let n = match std::io::Read::read(&mut s, &mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    // 从请求行里把 `id` 抠出来原样回显（不回显会被判成 Desync）。
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let id = req
                        .split("\"id\":\"")
                        .nth(1)
                        .and_then(|r| r.split('"').next())
                        .unwrap_or("1")
                        .to_string();
                    let body = format!(
                        r#"{{"jsonrpc":"2.0","id":"{id}","result":{{"downloadSpeed":"0","numActive":"0","numWaiting":"0","numStopped":"0"}}}}"#
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    {
                        use std::io::Write as _;
                        let _ = s.write_all(resp.as_bytes());
                        let _ = s.flush();
                    }
                    // ……之后**装死**：不回话、也不关连接（连接留在池里，客户端下一步会撞上它）。
                    std::thread::sleep(std::time::Duration::from_secs(30));
                });
            }
        });

        let c = RpcClient::new(&format!("http://127.0.0.1:{port}/jsonrpc"), "s3cr3t");

        // 第一拍：成功 ⇒ 这条连接进了池子（此刻起它归我们摆布）。
        c.global()
            .expect("第一拍必须成功——桩会应答每条连接的第一个请求");
        assert_eq!(
            conns.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "第一拍应当只开一条连接"
        );

        // 第二拍：复用池内那条**已装死的**连接 ⇒ 传输层失败 ⇒ 必须换新连接重试一次。
        // ⚠️ 这一拍会等满 `RPC_TIMEOUT`（10 秒）才重试，是预期的。
        c.global()
            .expect("读方法在传输层失败后必须换新连接重试一次");
        assert_eq!(
            conns.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "第二拍应当**再开一条**连接（恰好一次重试）——\
             只开 1 条 = 没重试；开 3 条 = 重试了不止一次"
        );
    }

    /// 🔴 **重试也失败时，返回的是「重试那一次」的错误，不是第一次的**（规格 §3 A1 标红那条）。
    ///
    /// 为什么这条承重：第一次的失败发生在一条**可能已经坏掉的池内连接**上，它描述的未必是
    /// 引擎的真实状态；重试走的是**全新连接**，它的失败才是"现在到底通不通"。
    /// 本用例造的就是规格里举的那个例子：**第一次读超时、重试却 connection refused**
    /// ⇒ 引擎真的没了——报第一次那句"超时"会把客户引向错的方向。
    ///
    /// 判别力：把 `call` 里的 `out.map_err(|second| second.msg)` 改成 `Err(first.msg)`
    /// ⇒ 本用例红（返回的是那句超时，里面没有 "refused"）。
    ///
    /// ⚠️ **要等满 `RPC_TIMEOUT`（10 秒），这是判据的一部分不是浪费**：第一次必须**真的
    ///    超时**，两次失败才是**两种不同的错误** —— 否则两条错误串长得一样，
    ///    "返回了哪一次"这件事就分不出来，用例会退化成恒真（下面那条下界断言守着它）。
    #[test]
    fn a_failed_retry_returns_the_retrys_error_not_the_first_ones() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            // 只收**一条**连接：服务它的第一个请求（把这条连接放进池子），然后**装死**
            // （不回话、也不关 —— 第一次调用因此会在它上面**超时**）。
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 65536];
                let n = match std::io::Read::read(&mut s, &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let id = req
                    .split("\"id\":\"")
                    .nth(1)
                    .and_then(|r| r.split('"').next())
                    .unwrap_or("1")
                    .to_string();
                let body = format!(
                    r#"{{"jsonrpc":"2.0","id":"{id}","result":{{"downloadSpeed":"0","numActive":"0","numWaiting":"0","numStopped":"0"}}}}"#
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                {
                    use std::io::Write as _;
                    let _ = s.write_all(resp.as_bytes());
                    let _ = s.flush();
                }
                // 🔴 **关掉监听套接字**（不是关这条连接）：重试那一次去开**新连接**时
                //    会立刻拿到 connection refused —— 快，而且与第一次的"超时"截然不同。
                drop(listener);
                // 这条**已经建立**的连接继续装死（第一次调用就在它上面超时）。
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });

        let c = RpcClient::new(&format!("http://127.0.0.1:{port}/jsonrpc"), "s3cr3t");

        // 第一拍：成功 ⇒ 这条连接进了池子（此刻起它归我们摆布）。
        c.global()
            .expect("第一拍必须成功——桩会应答每条连接的第一个请求");

        let started = std::time::Instant::now();
        let err = c
            .global()
            .expect_err("池内那条装死、监听又关了 ⇒ 两次都失败，必须报错");
        // 下界：**第一次真的等满了超时** ⇒ 两次失败确实是两种不同的错误
        //（没有这条，桩一旦退化成"立刻拒绝"，本用例就恒真了）。
        assert!(
            started.elapsed() >= RPC_TIMEOUT,
            "第一次没有等满 RPC_TIMEOUT（{:?}）：那说明它没走「池内那条装死的连接」这条支\
             ⇒ 本用例判不了「返回的是哪一次的错误」",
            started.elapsed()
        );
        // 判据本体：返回的是**重试那一次**的 connection refused，不是第一次的超时。
        assert!(
            err.to_lowercase().contains("refused"),
            "必须返回**重试那一次**的错误（connection refused）；\
             拿到的是第一次那句 ⇒ 客户会被引向错的方向：{err}"
        );
    }

    /// 🔴 **写方法绝不重试**（规格 §3 A1）。
    ///
    /// `addUri` **不幂等**：本仓用 `--auto-file-renaming=false` + `--allow-overwrite=true`，
    /// 盲重试会让同一个 `out` 建出**两个 GID 写同一个落盘路径**——那是数据风险，不是性能问题。
    ///
    /// 判别力：把 `is_retryable_read("aria2.addUri")` 改成 `true` ⇒ 本测试必红。
    #[test]
    fn a_transport_failure_on_a_write_is_never_retried() {
        let conns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let conns_srv = std::sync::Arc::clone(&conns);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                conns_srv.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // 每一条连接都收下就掐断：写方法不许因此得到第二次机会。
                drop(s);
            }
        });

        let c = RpcClient::new(&format!("http://127.0.0.1:{port}/jsonrpc"), "s3cr3t");
        let e = c
            .add_uri(&["http://127.0.0.1:1/x".to_string()], "/tmp", "x.bin", &BTreeMap::new())
            .expect_err("掐断的连接上 addUri 必须失败");
        assert!(e.contains("RPC 请求失败"), "错误文案变了：{e}");
        assert_eq!(
            conns.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "写方法被重试了 —— addUri 不幂等，盲重试会让两个 GID 写同一个落盘路径"
        );
    }

    /// 白名单**逐字**钉住：多一个进去、少一个出来都要红。
    #[test]
    fn retryable_reads_are_exactly_the_read_methods() {
        for m in [
            "aria2.getGlobalStat",
            "aria2.tellActive",
            "aria2.tellWaiting",
            "aria2.tellStopped",
        ] {
            assert!(is_retryable_read(m), "{m} 是读方法，必须可重试");
        }
        for m in [
            "aria2.addUri",
            "aria2.remove",
            "aria2.pause",
            "aria2.unpause",
            "aria2.purgeDownloadResult",
            "aria2.removeDownloadResult",
            "aria2.changeGlobalOption",
            "aria2.shutdown",
            // 换腿：写方法，与上面同口径（重试可能把同一条腿删/加两遍）。
            "aria2.changeUri",
        ] {
            assert!(!is_retryable_read(m), "{m} 是写方法，**绝不许**重试");
        }
        // ⚠️ `aria2.getServers` 与 `aria2.getUris` **两段都故意不进**：它们是读方法，但同样
        //    不许重试（10 秒一拍的下一拍就是天然重试）。别把它们补进上面那段读方法的循环里——
        //    它们各由 `servers_is_not_retried_on_transport_failure` /
        //    `get_uris_is_not_retried_on_transport_failure` 单独守。
    }

    // ------------------------------------------------------------------------
    // **详细档"每一次往返"那一行**（任务 1 的主判据；非移植 —— Go 内核没有这一档）。
    //
    // 为什么必须有它：`send` 里那一句记账是**整档新增行为的唯一收口**，而它此前
    // 一条判据都没有 —— 删掉那一句、把 `why` 变成无条件、把闸门反过来，**全套判据仍然全绿**。
    //
    // 🔴 **两条硬约束**（下面那条用例逐条绕开，机制见 [`VerboseSink`]）：
    //   ① **不许往真的用户日志目录里写**：`log_verbose` 走 `paths::diagnostics_log()`
    //      （平台默认的 `%APPDATA%` / `$HOME` 那一份）——在本机就是开发者自己那份真日志。
    //      用例往里写 = "跑一次测试"变成"往用户的日志里灌测试数据"。
    //   ② **不许翻进程级静态**：`diagnostics::init(Level::Verbose)` 写的是全局，而同进程里
    //      那几条既有用例假设的是 normal（1 MiB / 一代）⇒ 在单测里 `init(Verbose)` 会让
    //      它们**随线程调度随机红**，那种 flaky 比没有判据更坏。
    //
    // 注入 sink 把两条**同时**绕开：用例既不取那条路径（不碰 ①），也不读不写 `LEVEL`
    // （不碰 ②）。残余：`log_verbose` 里那道**闸门本身**（normal 档不记）仍没有判据 ——
    // 要判它就得动上面两样中的一样；它今天的证据是任务 1 报告里 Step 8 的 A/B 实测。
    // ------------------------------------------------------------------------

    /// `send` 落下来的每一行：`(事件名, 字段表)`。
    type Recorded = Vec<(String, Vec<(String, String)>)>;

    /// 一个**记账替身** sink：把每一行收进内存，**不碰文件系统、不碰全局**。
    ///
    /// 返回 `(注入用的 sink, 读记录用的句柄)`。
    fn recording_sink() -> (VerboseSink, std::sync::Arc<std::sync::Mutex<Recorded>>) {
        let got: std::sync::Arc<std::sync::Mutex<Recorded>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_got = std::sync::Arc::clone(&got);
        let sink: VerboseSink = std::sync::Arc::new(move |event, fields| {
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

    /// 🔴 **每一次到 aria2 的往返都落一行；失败那一行带着 aria2 自己的原文**（规格 §5.1）。
    ///
    /// 三拍成功 + 一拍失败 ⇒ **四行**，且：
    ///   · 成功那三条**不带 `why`**（补一个空字段会让"这一行有几个字段"随结果变）；
    ///   · 失败那一条的 `why` **就是 aria2 报错原文**（这一整档存在的理由）。
    ///
    /// 判别力（两条都实测过，读数见任务 1 报告"修复轮 1"）：
    ///   · 把 `send` 里那句记账删掉 ⇒ 记录 **0 行**，第一条断言红；
    ///   · 把 `why` 改成**无条件**追加 ⇒ 成功那三条多出 `why=`，最后那道"不许带 why"红。
    #[test]
    fn every_round_trip_is_logged_with_aria2s_own_words_on_failure() {
        let srv = new_stub_rpc(|m, _p| match m {
            "aria2.getGlobalStat" => Ok(json!({
                "downloadSpeed": "0",
                "numActive": "0",
                "numWaiting": "0",
                "numStopped": "0",
            })),
            // 写方法这一拍失败：aria2 的原文必须原样出现在 `why` 里
            "aria2.addUri" => Err((1, "Invalid GID x".to_string())),
            other => Err((1, format!("意外的方法: {other}"))),
        });
        let (sink, got) = recording_sink();
        let mut c = RpcClient::new(&rpc_url(&srv), "s");
        // 只换出口：不动 `diagnostics` 的级别静态，也不动那条日志路径
        c.verbose_sink = sink;

        for _ in 0..3 {
            c.global().expect("读方法这三拍都应当成功");
        }
        c.add_uri(&["http://x/f".to_string()], "PFX/sub", "a.txt", &BTreeMap::new())
            .expect_err("写方法这一拍必须失败");

        let lines = got
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            lines.len(),
            4,
            "每一次往返都要落一行（3 拍成功 + 1 拍失败 = 4 行），实际 {lines:?}"
        );
        for l in &lines {
            assert_eq!(l.0, "aria2_call", "事件名必须是 aria2_call：{l:?}");
            assert!(field_of(l, "ms").is_some(), "每一行都要带耗时：{l:?}");
            assert!(field_of(l, "ok").is_some(), "每一行都要带成败：{l:?}");
        }
        let methods: Vec<String> = lines.iter().filter_map(|l| field_of(l, "method")).collect();
        assert_eq!(
            methods.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "aria2.getGlobalStat",
                "aria2.getGlobalStat",
                "aria2.getGlobalStat",
                "aria2.addUri",
            ],
            "记的是**固定白名单里的方法名**（参数含 token，一个都不许进来）；\
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
        // 失败那一条：`ok=false`，且 `why` 是 aria2 自己说的那句话
        let last = &lines[3];
        assert_eq!(field_of(last, "ok").as_deref(), Some("false"), "{last:?}");
        let why = field_of(last, "why").expect("失败时必须带 `why`");
        assert!(
            why.contains("Invalid GID x"),
            "`why` 必须带 **aria2 的原文**（这一整档存在的理由），实际：{why:?}"
        );
    }

    /// 「接了就断」的假引擎：每条连接 accept 之后立刻松手（连接被对端关掉），
    /// 客户端读到传输层错误。返回 `(JSON-RPC 地址, 已 accept 的连接条数)`。
    ///
    /// 为什么不用既有的 [`StubHttp`] / [`new_stub_rpc`]：它们不对外暴露"accept 过几条连接"，
    /// 而下面两条判据数的正是这个数（慢档不许重试 / 快档必须重试一次）。
    /// 形状照 [`counting_keepalive_rpc_stub`]：同一个返回元组、同一处计数时机
    /// （accept 之后、应答之前——本桩没有应答，所以是 accept 之后立刻计数再松手）。
    fn drop_after_accept_stub() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let conns = Arc::new(AtomicUsize::new(0));
        let conns_srv = Arc::clone(&conns);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                // 接受即松手：连接被对端关掉，客户端读到传输层错误。
                if stream.is_ok() {
                    conns_srv.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        (format!("http://127.0.0.1:{port}/jsonrpc"), conns)
    }

    /// 🔴 **慢档单次尝试、不重试**（规格 §3.2）：一个「接了就断」的假引擎，
    /// 慢档读必须只连一次——重试会把最坏代价翻倍，而它们是周期性读（下一拍就是重试）。
    ///
    /// 判别力：给 `list()` 也挂上 `is_retryable_read` 的重试 ⇒ 连接计数变 2 ⇒ 红。
    #[test]
    fn slow_snapshot_reads_never_transport_retry() {
        let (url, accepted) = drop_after_accept_stub();
        let c = RpcClient::new(&url, "s");
        let e = c.list(0).expect_err("接了就断的引擎不该成功");
        assert!(e.contains("RPC 请求失败"), "{e}");
        let n = accepted.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(n, 1, "慢档不许传输层重试：实际连了 {n} 次");
    }

    /// 🔴 **`global_slow` 同样单次尝试、不重试**（终审 Important #2 的补钉）——
    /// 同款「接了就断」桩，只连一次。
    ///
    /// 判别力：把 `global_slow` 的 `call_slow` 换成 [`Self::call`]（`getGlobalStat`
    /// 在 [`is_retryable_read`] 白名单里 ⇒ 传输层失败换新连接再试一次）⇒ 连接计数变 2 ⇒ 红。
    /// 超时档那一半由 [`slow_reads_wait_out_the_snapshot_timeout_before_giving_up`]
    /// 的 `global_slow` 探针钉（≥25s 且 ≤35s）——两条互为补集。
    #[test]
    fn slow_global_stat_never_transport_retries() {
        let (url, accepted) = drop_after_accept_stub();
        let c = RpcClient::new(&url, "s");
        let e = c.global_slow().expect_err("接了就断的引擎不该成功");
        assert!(e.contains("RPC 请求失败"), "{e}");
        let n = accepted.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(n, 1, "慢档 global_slow 不许传输层重试：实际连了 {n} 次");
    }

    /// 🔴 **快档读仍然重试一次**（对照组：上一条的反面）——同款假引擎，`global()`（快档）
    /// 允许换新连接再试一次 ⇒ 连接计数 2。
    #[test]
    fn fast_reads_still_retry_once_on_transport_failure() {
        let (url, accepted) = drop_after_accept_stub();
        let c = RpcClient::new(&url, "s");
        let e = c.global().expect_err("接了就断的引擎不该成功");
        assert!(e.contains("RPC 请求失败"), "{e}");
        let n = accepted.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(n, 2, "快档读的传输层重试不能丢（判别连接死 vs 引擎死的既有机制）");
    }

    /// 慢档的连接阶段同样受 `RPC_TIMEOUT` 约束（规格 §3 A4）——
    /// `slow_agent` 放宽的只是**读响应**（`SNAPSHOT_TIMEOUT`），连接阶段仍要快。
    ///
    /// 做法与 [`connect_phase_is_bounded_by_rpc_timeout`] 同源：连黑洞地址、判耗时上界、
    /// **宽松上界 + 快败跳过**（那条的免责声明同样适用：这里守的是"那个数字被写进构造里了"，
    /// 不是"ureq 是否真的尊重这个值"）。
    ///
    /// 探的是 `global_slow()`（慢档 `getGlobalStat`）与 `list()`（三条 `tell*`）两个慢档入口
    /// ——它们共用 `slow_agent`，但只探其中一个的话，另一个改回快档/漏了 `timeout_connect`
    /// 不会有任何测试变红。
    #[test]
    fn slow_connect_phase_is_bounded_by_rpc_timeout() {
        let c = RpcClient::new("http://10.255.255.1:9/jsonrpc", "s");
        let probes: [(&str, fn(&RpcClient) -> Result<(), String>); 2] = [
            ("慢档 global_slow", |c| c.global_slow().map(|_| ())),
            ("慢档 list", |c| c.list(0).map(|_| ())),
        ];
        for (label, probe) in probes {
            let started = std::time::Instant::now();
            let e = probe(&c).expect_err("这个地址不该有人应答");
            let took = started.elapsed();

            if took < Duration::from_secs(2) {
                eprintln!(
                    "跳过：本机对 10.255.255.1:9 立刻失败（{label} {took:?}），\
                     走不到超时那条路，判据无判别力"
                );
                continue;
            }
            assert!(e.contains("RPC 请求失败"), "{label} 的错误文案变了：{e}");
            assert!(
                took <= RPC_TIMEOUT + Duration::from_secs(5),
                "{label} 的连接阶段耗时 {took:?} 超出了 RPC_TIMEOUT（{RPC_TIMEOUT:?}）——\
                 慢档放宽的只是读响应，连接阶段必须仍显式 .timeout_connect(RPC_TIMEOUT)"
            );
        }
    }

    /// **慢档 30 秒上界 + 快档 10 秒上界**（规格 §6 判据 1）—— 快慢档各自的耗时上界。
    ///
    /// 做法与 [`connect_phase_is_bounded_by_rpc_timeout`] 逐字同款：连黑洞地址、
    /// 判**耗时上界**、**宽松上界 + 快败跳过**。
    ///
    /// 三个探面与各自上界：
    ///   · `list()`（慢档 `tell*`）与 `global_slow()`（慢档 `getGlobalStat`）——
    ///     上界 `SNAPSHOT_TIMEOUT + 5 秒`；
    ///   · 对照组 `ping()`（快档探活）——上界 `RPC_TIMEOUT + 5 秒`。
    ///
    /// **为什么缺一不可**：只测慢档的话，「快档被误改成 30 秒」不会有任何测试变红。
    /// ⚠️ 这条**只钉上界**——黑洞地址死在连接阶段（被 `timeout_connect(RPC_TIMEOUT)` 钉住），
    /// 慢档读超时**值**对它不可观测（把 `SNAPSHOT_TIMEOUT` 改回 10s 本条照样绿，实测过）。
    /// 慢档读超时的值由 [`slow_reads_wait_out_the_snapshot_timeout_before_giving_up`]
    /// 的「接受但不回」桩钉——两条互为补集。
    ///
    /// ⚠️ **免责声明照 [`connect_phase_is_bounded_by_rpc_timeout`] 那条**：
    /// 这条测不出"ureq 是否真的尊重这个值"——守的是"**那个数字被写进构造里了**"。
    /// 这句必须留着：否则下一个人会以为这条测试验过超时行为。
    ///
    /// ⚠️ 对照组用 `ping()` 而**不是** `global()`：`global()` 走 [`Self::call`]，
    /// 传输层失败会**重试一次**（`is_retryable_read`），对黑洞地址就是 2 × 10 秒
    /// ——超出 `RPC_TIMEOUT + 5 秒` 的上界，那不是"快档超时变了"而是"重试在起作用"。
    /// `ping()` 与 [`Self::call_slow`] 一样是**单次** `send`，读数才可比。
    ///
    /// ⚠️ 在网络受限的机器上（立刻 connection refused）与同款一样**跳过**，
    /// 不伪装通过。
    #[test]
    fn slow_tier_is_bounded_by_snapshot_timeout_and_fast_by_rpc_timeout() {
        let c = RpcClient::new("http://10.255.255.1:9/jsonrpc", "s");
        // (标签, 探针, 上界)——慢档上界 `SNAPSHOT_TIMEOUT`、快档上界 `RPC_TIMEOUT`。
        let probes: [(&str, fn(&RpcClient) -> Result<(), String>, Duration); 3] = [
            ("慢档 list（tell*）", |c| c.list(0).map(|_| ()), SNAPSHOT_TIMEOUT),
            ("慢档 global_slow", |c| c.global_slow().map(|_| ()), SNAPSHOT_TIMEOUT),
            ("快档 ping（对照组）", |c| c.ping(), RPC_TIMEOUT),
        ];
        for (label, probe, bound) in probes {
            let started = std::time::Instant::now();
            let e = probe(&c).expect_err("这个地址不该有人应答");
            let took = started.elapsed();

            if took < Duration::from_secs(2) {
                eprintln!(
                    "跳过：本机对 10.255.255.1:9 立刻失败（{label} {took:?}），\
                     走不到超时那条路，判据无判别力"
                );
                continue;
            }
            assert!(e.contains("RPC 请求失败"), "{label} 的错误文案变了：{e}");
            assert!(
                took <= bound + Duration::from_secs(5),
                "{label} 的耗时 {took:?} 超出了上界 {bound:?}（+5 秒宽松量）——\
                 这条守的是「那个数字被写进构造里了」，不是「ureq 真的尊重这个值」\
                 （后者由 ureq 语义保证，见 connect_phase_is_bounded_by_rpc_timeout 的说明）"
            );
        }
    }

    /// 「接受但不回」的桩：accept 后持有连接、**永不回响应**——客户端只能靠**读超时**放弃。
    /// 返回 JSON-RPC 地址。每条连接一个线程持有它（不关、不写），确定性等到读超时。
    fn accept_but_never_respond_stub() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(s) = stream {
                    // 接受后持有、永不回响应——读超时由客户端那一侧触发。
                    std::thread::spawn(move || {
                        let _held = s;
                        std::thread::sleep(std::time::Duration::from_secs(300));
                    });
                }
            }
        });
        format!("http://127.0.0.1:{port}/jsonrpc")
    }

    /// 🔴 **慢档读超时的值真的在生效**（与 [`slow_tier_is_bounded_by_snapshot_timeout_and_fast_by_rpc_timeout`] 互为补集）。
    ///
    /// 「接受但不回」桩（[`accept_but_never_respond_stub`]）：accept 后持有连接、
    /// 永不回响应 ⇒ 客户端只能靠**读超时**放弃，且耗时确定性 ≈ 读超时值。
    ///   · `list()`（慢档）⇒ 必须等满 `SNAPSHOT_TIMEOUT` 才失败：
    ///     `took >= SNAPSHOT_TIMEOUT - 5s`（≥25s，防"被改回 10s"）且
    ///     `took <= SNAPSHOT_TIMEOUT + 5s`（上界照旧）。
    ///   · `global_slow()`（慢档）⇒ 同款两界（终审 Important #2 补钉：与 `list()` 共用
    ///     `slow_agent`，但只探一头的话另一头被改回快档不会红）。
    ///   · 对照组 `ping()`（快档）⇒ `took <= RPC_TIMEOUT + 5s` 且 `took < 20s`
    ///     （防"快档被放宽"——上界行为测已有，这里再钉下界方向的分野）。
    ///
    /// ⚠️ 这条守的是「**那个 30 秒真的在生效**」，不是「数字被写进构造里了」——
    /// 后者由 [`slow_tier_is_bounded_by_snapshot_timeout_and_fast_by_rpc_timeout`] 守。
    /// 两条互为补集：那条对读超时**值**不可观测（黑洞地址死在连接阶段，
    /// 被 `timeout_connect(RPC_TIMEOUT)` 钉住），这条能观测到。
    ///
    /// 确定性 30 秒（接受后持有、永不回响应），不需要快败跳过分支。
    #[test]
    fn slow_reads_wait_out_the_snapshot_timeout_before_giving_up() {
        let url = accept_but_never_respond_stub();
        let c = RpcClient::new(&url, "s");

        // 慢档 `list()`：必须等满读超时（30 秒）才放弃。
        let started = std::time::Instant::now();
        let e = c.list(0).expect_err("永不回响应的桩不该成功");
        let took = started.elapsed();
        assert!(e.contains("RPC 请求失败"), "慢档失败文案：{e}");
        assert!(
            took >= SNAPSHOT_TIMEOUT - Duration::from_secs(5),
            "慢档 `list()` 在 {took:?} 就放弃了——没等满 SNAPSHOT_TIMEOUT（{SNAPSHOT_TIMEOUT:?}）：\
             那说明 30 秒档没在生效（被改回 10s 的形状）"
        );
        assert!(
            took <= SNAPSHOT_TIMEOUT + Duration::from_secs(5),
            "慢档 `list()` 等了 {took:?} 才放弃，超出 SNAPSHOT_TIMEOUT（{SNAPSHOT_TIMEOUT:?}）上界"
        );

        // 慢档 `global_slow()`：同款口径（终审 Important #2 的补钉——它与 `list()` 共用
        // `slow_agent`，但只探 `list()` 的话，`global_slow` 被改回快档/`call` 不会有测试变红）。
        let started = std::time::Instant::now();
        let e = c.global_slow().expect_err("永不回响应的桩不该成功");
        let took = started.elapsed();
        assert!(e.contains("RPC 请求失败"), "慢档失败文案：{e}");
        assert!(
            took >= SNAPSHOT_TIMEOUT - Duration::from_secs(5),
            "慢档 `global_slow()` 在 {took:?} 就放弃了——没等满 SNAPSHOT_TIMEOUT（{SNAPSHOT_TIMEOUT:?}）：\
             那说明它不在 30 秒档上（被改回 `call`/10s 的形状）"
        );
        assert!(
            took <= SNAPSHOT_TIMEOUT + Duration::from_secs(5),
            "慢档 `global_slow()` 等了 {took:?} 才放弃，超出 SNAPSHOT_TIMEOUT（{SNAPSHOT_TIMEOUT:?}）上界"
        );

        // 对照组快档 `ping()`：同一根桩，必须明显更快放弃。
        let started = std::time::Instant::now();
        let e = c.ping().expect_err("永不回响应的桩不该成功");
        let took = started.elapsed();
        assert!(e.contains("RPC 请求失败"), "快档失败文案：{e}");
        assert!(
            took <= RPC_TIMEOUT + Duration::from_secs(5),
            "快档 `ping()` 等了 {took:?}，超出 RPC_TIMEOUT（{RPC_TIMEOUT:?}）上界——快档被放宽了？"
        );
        assert!(
            took < Duration::from_secs(20),
            "快档 `ping()` 等了 {took:?}（≥20s）——与慢档分不开，说明快档被放宽了"
        );
    }

    /// 连接阶段受 `RPC_TIMEOUT` 约束（规格 §3 A4）—— **用行为验，不用配置验**。
    ///
    /// 做法：连一个**不回 SYN 也不回 RST** 的地址（`10.255.255.1:9`，本机实测是黑洞），
    /// 判据是**耗时上界**。两个 agent 各探一次，**缺一不可**：
    /// 池内 agent 走写方法 `shutdown`（写方法**从不重试**，所以恰好是一次尝试），
    /// `fresh_agent` 走 `ping`。只探其中一个的话，把另一处的 `timeout_connect` 删掉
    /// **不会有任何测试变红**。
    ///
    /// ⚠️ **这条测不出"ureq 是否真的尊重这个值"**——那一条由 ureq 自己的语义保证
    /// （vendor 源码 `stream.rs:352-356`：连接阶段用 `timeout_connect`，且它**优先于**
    /// 整体超时）。这里守的是"**那个数字被写进构造里了**"。
    /// 这句免责声明必须留着：否则下一个人会以为这条测试验过超时行为。
    ///
    /// ⚠️ 这条在网络受限的机器（例如代理设置会立刻拒绝）上可能**不是**走超时路径，
    /// 而是立刻 `connection refused`——那种情况下这条测试**没有判别力**。
    /// 因此它用一个**宽松上界**（`RPC_TIMEOUT + 5 秒`）并且**在耗时明显小于超时时直接跳过**，
    /// 把"本机环境不支持这个判据"如实说出来，而不是伪装成通过。
    #[test]
    fn connect_phase_is_bounded_by_rpc_timeout() {
        let c = RpcClient::new("http://10.255.255.1:9/jsonrpc", "s");
        let probes: [(&str, fn(&RpcClient) -> Result<(), String>); 2] = [
            ("池内 agent（shutdown：写方法，恰好一次尝试）", |c| c.shutdown()),
            ("fresh_agent（ping）", |c| c.ping()),
        ];
        for (label, probe) in probes {
            let started = std::time::Instant::now();
            let e = probe(&c).expect_err("这个地址不该有人应答");
            let took = started.elapsed();

            if took < Duration::from_secs(2) {
                eprintln!(
                    "跳过：本机对 10.255.255.1:9 立刻失败（{label} {took:?}），\
                     走不到超时那条路，判据无判别力"
                );
                continue;
            }
            assert!(e.contains("RPC 请求失败"), "{label} 的错误文案变了：{e}");
            assert!(
                took <= RPC_TIMEOUT + Duration::from_secs(5),
                "{label} 的连接阶段耗时 {took:?} 超出了 RPC_TIMEOUT（{RPC_TIMEOUT:?}）——\
                 ureq 的 timeout_connect 默认 30 秒且优先于整体超时，\
                 这个 agent 必须显式 .timeout_connect(RPC_TIMEOUT)"
            );
        }
    }

    /// **`keys` 白名单必须与 `RawTask` 真正读的字段逐字相同**（2026-10-08）。
    ///
    /// 判别力（三个方向都会红）：
    ///   · 白名单**少一个**键 ⇒ 那个字段永远读不回来，**静默落空成 `0` / `""`** ——
    ///     本项目栽过一次的形状（`RawTask` 缺 `camelCase` 重命名 ⇒ 客户报"没有下载进度"）；
    ///   · 白名单**多一个** ⇒ 白问一次，它就不再等于"真正用得上的那组"；
    ///   · 给 `RawTask` **加了字段**而忘了加进白名单 ⇒ 等于少一个键。
    ///
    /// ⚠️ 为什么从**源码**里取字段名：`RawTask` 只 derive 了 `Deserialize`
    ///    （序列化不出来），而"它有哪些字段"这件事只有在源码里是权威的。
    ///    手法与 `commands.rs` 那条"会阻塞的命令不许开在主线程上"同源：够不着运行时的
    ///    不变量，就用源码本身当判据 —— 并且**明确写出它够不着什么**。
    #[test]
    fn the_keys_whitelist_matches_what_raw_task_reads() {
        let src = include_str!("status.rs");
        let start = src.find("pub struct RawTask {").expect("RawTask 不见了");
        let end = start + src[start..].find("\n}").expect("结构体没有收尾");
        let mut got: Vec<String> = src[start..end]
            .lines()
            .filter_map(|l| {
                let t = l.trim().strip_prefix("pub ")?;
                // ⚠️ **必须有冒号**：没有它就不是字段行。少了这一条会把
                //    `pub struct RawTask {` 也当成一个"字段"（第一版就是这么红的）。
                let (name, _) = t.split_once(':')?;
                let name = name.trim();
                (!name.is_empty()).then(|| camel(name))
            })
            .collect();
        let mut want: Vec<String> = TASK_KEYS.iter().map(|k| (*k).to_string()).collect();
        got.sort();
        want.sort();
        assert!(
            !got.is_empty(),
            "一个字段都没从 `status.rs` 的 RawTask 里取到 —— 这条判据的切法坏了，不是「通过」"
        );
        assert_eq!(
            got, want,
            "`keys` 白名单与 `RawTask` 的字段对不上：少了哪个键，那个字段就永远读不回来\
             （静默落空成 0/\"\"，不报错）"
        );
    }

    /// `snake_case` → `camelCase`（serde 的 `rename_all` 对这几个字段就是这个规则）。
    fn camel(s: &str) -> String {
        let mut out = String::new();
        let mut upper_next = false;
        for c in s.chars() {
            if c == '_' {
                upper_next = true;
            } else if upper_next {
                out.extend(c.to_uppercase());
                upper_next = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    /// 每一次往返都要落进计数器，**成功的也要记** —— "慢"本身就是信号（规格 §3.1 信号 A）。
    #[test]
    fn every_round_trip_is_recorded_in_the_counters() {
        let srv = StubHttp::start_with(|_| {
            // ⚠️ 这一睡是判据的一部分，不许删：`rpc_worst` 是毫秒精度（`record_call` 写
            //    `took.as_millis()`，`snapshot()` 用 `from_millis` 读回，`diag_counters.rs:106-109`/`:152`）。
            //    回环往返常常不到 1ms，正确的实现也会读回 0，断言会在快机器上假红。
            std::thread::sleep(Duration::from_millis(20));
            RawResponse::new(200).body(r#"{"jsonrpc":"2.0","id":"1","result":{}}"#.to_string())
        });
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client =
            RpcClient::new(&rpc_url(&srv), "s3cr3t").with_counters(std::sync::Arc::clone(&c));
        let _ = client.call("aria2.getGlobalStat", &[]);
        let s = c.snapshot();
        assert_eq!(s.rpc_calls, 1, "成功的那一次也要记");
        assert_eq!(s.rpc_failures, 0);
        // 钉住"记的是真实耗时"：若改记 `Duration::ZERO`，下面 `rpc_worst` 那一条会红。
        // 分桶那条抓不到这个变异（`bucket_of(Duration::ZERO)` 也落在第 0 桶），它守的是分桶映射本身。
        assert!(
            s.rpc_worst >= Duration::from_millis(20),
            "耗时必须真量过：垫了 20ms，读回的却只有 {:?}",
            s.rpc_worst
        );
        assert_eq!(
            s.rpc_buckets,
            [1, 0, 0, 0],
            "回环上一次成功往返必然落在 <1s 那一桶"
        );
    }

    /// 失败的也要记，且记在 `rpc_failures` 里。
    #[test]
    fn a_failed_round_trip_is_recorded_as_a_failure() {
        // 指向没人监听的端口：连接立刻被拒（不是超时），用例跑得快，也不依赖任何桩。
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client = RpcClient::new("http://127.0.0.1:1/jsonrpc", "s")
            .with_counters(std::sync::Arc::clone(&c));
        let _ = client.call("aria2.getGlobalStat", &[]);
        let s = c.snapshot();
        // 读方法在传输层失败后会换新连接重试一次，两次都经过 `send`，所以是 2 而不是 1。
        assert_eq!(s.rpc_calls, 2, "读方法失败会重试一次，两次往返都要记");
        assert_eq!(s.rpc_failures, 2);
    }

    /// ⚠️ 钉住"收口在 `send`"：`ping` 不走 `call`（直接调 `self.send`），
    /// 所以它也记上了，就证明记账点在 `send` 里而不在 `call` 的分支里。
    #[test]
    fn ping_is_counted_too_proving_the_chokepoint_is_send() {
        let srv = StubHttp::start_with(|_| {
            RawResponse::new(200).body(r#"{"jsonrpc":"2.0","id":"1","result":{}}"#.to_string())
        });
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client =
            RpcClient::new(&rpc_url(&srv), "s3cr3t").with_counters(std::sync::Arc::clone(&c));
        client.ping().expect("桩会正常应答");
        assert_eq!(c.snapshot().rpc_calls, 1, "ping 也经过 send ⇒ 也记账");
    }

    /// 写方法不在 `is_retryable_read` 白名单里，从不重试 ⇒ 恒记 1 次。
    /// 与上面那条失败用例解耦：白名单变了只会让那条红，不会让这条红。
    #[test]
    fn a_non_retryable_write_is_counted_exactly_once() {
        let c = std::sync::Arc::new(crate::diag_counters::Counters::default());
        let client = RpcClient::new("http://127.0.0.1:1/jsonrpc", "s")
            .with_counters(std::sync::Arc::clone(&c));
        let _ = client.call("aria2.addUri", &[]);
        let s = c.snapshot();
        assert_eq!(s.rpc_calls, 1, "写方法不重试 ⇒ 只有一次往返");
        assert_eq!(s.rpc_failures, 1);
    }
}
