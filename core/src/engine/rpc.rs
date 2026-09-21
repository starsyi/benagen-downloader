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

/// aria2 JSON-RPC 的最小客户端。
///
/// 它**可并发使用**：界面线程调 `add_uri` 的同时，进度轮询线程调 `list`/`global`。
/// Go 侧靠"结构体里不放可变字段"来保证这一点，注释里还点名了"计数器之类'看着无害'的字段
/// 在这里就是 data race"。Rust 侧的请求 ID **必须是**一个递增计数器
/// （协议 §5.1 要求 `id` 参与配对，见 [`RpcClient::call`]），
/// 于是它按 Rust 的规矩落在 `AtomicU64` 上——并发安全由类型系统兜住，而不是靠纪律。
pub struct RpcClient {
    url: String,
    secret: String,
    /// `ureq::Agent` 自带连接池，它就是 Go 侧 `http.Client` 的等价物：
    /// 轮询每 200 ms 一次，每次新建 agent 等于每次重来一遍 TCP 握手。
    agent: ureq::Agent,
    /// 下一个请求 ID。只用 `fetch_add`，没有"先读后写"。
    next_id: AtomicU64,
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
            agent: ureq::AgentBuilder::new().timeout(RPC_TIMEOUT).build(),
            next_id: AtomicU64::new(1),
        }
    }

    /// 发一次 RPC 并返回 `result` 的原始 JSON。
    ///
    /// **顺序是承重的**（与 Go 的 `call` 逐字一致）：先把 body 解析成 JSON，
    /// 再看 `error` 对象，**最后**才看 HTTP 状态码。
    /// aria2 用 400 表示"调用出错"且详情在 body 里，所以**不能**先按状态码失败——
    /// 那会把 `Invalid GID x` 与 `Unauthorized` 一起压成一句"HTTP 400"。
    fn call(&self, method: &str, params: &[serde_json::Value]) -> Result<serde_json::Value, String> {
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
        .map_err(|e| format!("RPC 请求无法序列化: {e}"))?;

        // ⚠️ 不能把 4xx/5xx 当失败先返回：aria2 用 400 表示"调用出错"，详情在 body 里。
        // `ureq` 把 4xx/5xx 作为 `Err(Status(_, resp))` 抛出，但响应本身还在手上，照读。
        let (status, text) = match self
            .agent
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_string(&body)
        {
            Ok(resp) => (resp.status(), resp.into_string().map_err(read_body_error)?),
            Err(ureq::Error::Status(code, resp)) => {
                (code, resp.into_string().map_err(read_body_error)?)
            }
            Err(ureq::Error::Transport(e)) => return Err(format!("RPC 请求失败: {e}")),
        };

        let parsed: RpcResponse = serde_json::from_str(&text)
            .map_err(|e| format!("RPC 响应无法解析（HTTP {status}）: {e}"))?;
        if let Some(err) = parsed.error {
            return Err(format!("aria2 报错: {} (code {})", err.message, err.code));
        }
        if status != 200 {
            return Err(format!("RPC 返回 HTTP {status}"));
        }
        Ok(parsed.result)
    }

    /// 探活与校验 secret。
    ///
    /// 统一走 `aria2.getGlobalStat`：它同样校验 token，探活与验密一次做完。
    /// 结果被丢弃——探活只关心"这一趟走不走得通"。
    pub fn ping(&self) -> Result<(), String> {
        self.call("aria2.getGlobalStat", &[]).map(|_| ())
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
    /// ——"一次快照失败即判定引擎断开"的实际代价是 **1 次** RPC 失败（最坏约 10 秒），
    /// 而不是 4 次 × 10 秒。先跑完会把最坏情况从 1 次拖成 3 次。
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
    /// ⚠️ GID 落在 **`params[1]`**：`params[0]` 恒为 `token:<secret>`（[`RpcClient::call`] 补的）。
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
    /// **1 次** RPC 失败（最坏约 10 秒，connection refused 约 0 秒），而不是 3 次 × 10 秒。
    /// 先跑完会把最坏情况拖成三倍，也就把"为它多等几轮"这个被否掉的做法又请了回来。
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
}
