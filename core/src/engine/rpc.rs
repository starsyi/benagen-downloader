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

use super::status::{parse_num, RawTask};

/// 单次 RPC 的超时。对应 Go 的 `&http.Client{Timeout: 10 * time.Second}`。
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

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
}

impl RpcFailure {
    fn protocol(msg: String) -> Self {
        Self { msg, transport: false }
    }
    fn transport(msg: String) -> Self {
        Self { msg, transport: true }
    }
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
    /// 下一个请求 ID。只用 `fetch_add`，没有"先读后写"。
    next_id: AtomicU64,
    /// 详细档那一行的出口（见 [`VerboseSink`]）。生产恒为 `diagnostics::log_verbose`。
    verbose_sink: VerboseSink,
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
            next_id: AtomicU64::new(1),
            // 生产路径**恒为**它；`mod tests` 才会换成记账替身（见 [`VerboseSink`]）。
            verbose_sink: std::sync::Arc::new(crate::diagnostics::log_verbose),
        }
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
        let mut fields = vec![
            ("method", method.to_string()),
            ("ms", started.elapsed().as_millis().to_string()),
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
            return Err(RpcFailure::protocol(format!(
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

    /// 加一个下载任务，返回 GID。
    ///
    /// `dir`/`out` 的语义与 `-i` 输入文件完全一致（含非 ASCII 与空格）。
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
        url: &str,
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
            serde_json::Value::Array(vec![serde_json::Value::String(url.to_string())]),
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
    /// ⚠️ **第一次出错就返回**：不要先跑完三个子请求再报错。契约 §4.3 的论证建立在这上面
    /// ——"一次快照失败即判定引擎断开"的实际代价是 **1–2 次** RPC 尝试（最坏约 10 秒），
    /// 而不是 3–6 次 × 10 秒。⚠️ 次数**不再恒为 1**：读方法在**传输层**失败时会换一条新连接
    /// 重试一次（`is_retryable_read`），所以是 1–2；**写方法从不重试**，恒为 1。
    /// 先跑完会把最坏情况拖成三倍，也就把"为它多等几轮"这个被否掉的做法又请了回来。
    ///
    /// ⚠️ **"约 10 秒"是单次尝试的上界，且含连接阶段**：两个 Agent 都显式设了
    /// `timeout_connect(RPC_TIMEOUT)`（规格 §3 A4）。少了那一行，连接阶段会退到 ureq
    /// 默认的 **30 秒**（`timeout_connect` 在连接阶段**优先于**整体超时），这句"最坏约
    /// 10 秒"在连接阶段就不成立——`connect_phase_is_bounded_by_rpc_timeout` 守着这一点。
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
    pub fn list(&self) -> Result<Vec<RawTask>, String> {
        // 顺序逐字照 Go：active → waiting → stopped。见本方法的文档注释——
        // 这是契约 §1.2 的前提，不是随手排的。
        let mut out: Vec<RawTask> = Vec::new();
        for (method, with_range) in [
            ("aria2.tellActive", false),
            ("aria2.tellWaiting", true),
            ("aria2.tellStopped", true),
        ] {
            // `tellActive` 只收一个可选的 keys 数组，不收 [offset, num]；另两个要。
            // （Go 里就是 `if m != "aria2.tellActive"` 这一句的效果。）
            let params: Vec<serde_json::Value> = if with_range {
                vec![serde_json::Value::from(0), serde_json::Value::from(1000)]
            } else {
                Vec::new()
            };
            // 第一次出错就返回：不跑完剩下的子请求（契约 §4.3 的代价论证）。
            let raw = self.call(method, &params)?;
            let batch: Vec<RawTask> = serde_json::from_value(raw)
                .map_err(|e| format!("解析 {method} 失败: {e}"))?;
            out.extend(batch);
        }
        Ok(out)
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
        F: Fn(&str, &[Value]) -> Result<Value, (i64, String)> + Send + 'static,
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
        c.add_uri("http://x/f", "PFX/sub", "a.txt", &extra)
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
        c.add_uri("http://x/f", "PFX/sub", "b.txt", &BTreeMap::new())
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
            .list()
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
        assert_eq!(
            calls,
            vec![
                ("aria2.tellActive".to_string(), 1),
                ("aria2.tellWaiting".to_string(), 3),
                ("aria2.tellStopped".to_string(), 3),
            ],
            "方法名与参数形状（Go 里只有 tellActive 不带 [offset, num]）"
        );
        let (_, waiting) = method_and_params(&srv.requests()[1].body);
        assert_eq!(waiting[1], json!(0), "tellWaiting 的 offset 应是 0");
        assert_eq!(waiting[2], json!(1000), "tellWaiting 的 num 应是 1000");
    }

    /// `list` 第一次出错就返回，**不**跑完剩下的子请求。
    ///
    /// 契约 §4.3 的代价论证直接建立在这上面："一次快照失败即判定引擎断开"的实际代价是
    /// **1–2 次** RPC 尝试（最坏约 10 秒，connection refused 约 0 秒），而不是 3–6 次 × 10 秒。
    /// ⚠️ 次数**不再恒为 1**：读方法在**传输层**失败时会换一条新连接重试一次
    /// （`is_retryable_read`），所以是 1–2；**写方法从不重试**，恒为 1。
    /// ⚠️ **"约 10 秒"是单次尝试的上界，且含连接阶段**：两个 Agent 都显式设了
    /// `timeout_connect(RPC_TIMEOUT)`（规格 §3 A4）。少了那一行，连接阶段会退到 ureq
    /// 默认的 **30 秒**（`timeout_connect` 在连接阶段**优先于**整体超时），这句"最坏约
    /// 10 秒"在连接阶段就不成立——`connect_phase_is_bounded_by_rpc_timeout` 守着这一点。
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
        let err = c_a.list().expect_err("第一个子请求出错时 list 必须报错");
        assert!(err.contains("boom"), "aria2 的错误必须带出来: {err:?}");
        assert_eq!(
            srv_a.requests().len(),
            1,
            "第一次出错就必须返回，不得再发后续子请求"
        );

        // 场景 B：第二个子请求失败 —— 第三个不该发
        let srv_b = stub_failing_at(1);
        let c_b = RpcClient::new(&rpc_url(&srv_b), "s");
        assert!(c_b.list().is_err(), "第二个子请求出错时 list 必须报错");
        assert_eq!(
            srv_b.requests().len(),
            2,
            "第二次出错后不得再发第三个子请求"
        );
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
            .add_uri("http://127.0.0.1:1/x", "/tmp", "x.bin", &BTreeMap::new())
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
        ] {
            assert!(!is_retryable_read(m), "{m} 是写方法，**绝不许**重试");
        }
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
        c.add_uri("http://x/f", "PFX/sub", "a.txt", &BTreeMap::new())
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
}
