//! protocol —— stdio 上的 JSON Lines 编解码、请求/响应/错误类型与握手（设计规格 §5）。
//!
//! **本模块是新增，没有移植源**：Go 客户端没有进程边界，内核是进程内调用的。
//! 因此这里的行为由本模块自己的测试钉住，不逐条对应 Go 测试——按全局约束 6 的纪律，
//! 每条新增测试在 `task-12-report.md` 里**单独记账**（写明它守什么）。
//!
//! 控制者为本模块裁定的三条硬要求：
//!   1. `ok:true` 的序列化结果里**不得出现** `error` 键，`ok:false` 里不得出现 `result`。
//!      只断言结构体是看不出 `skip_serializing_if` 漏了的——测试**断言序列化后的文本**。
//!   2. 解析失败时，响应的 id **能配对就配对**：用 [`RequestError::responder_id()`]，
//!      只有**连 id 都读不出来**的畸形输入才回保留值 [`UNCORRELATED_ID`]（= 0）。
//!      **请求侧同样不接受 id 0**：否则一条 `{"id":0,…}` 会被回一条 id=0 的**成功**响应，
//!      而壳按约定把它读成协议级错误。
//!   3. 超长行不得 panic（见 `read_request_handles_very_long_line`）。
//!
//! ⚠️ **stdout 是协议专用通道**（全局约束 1）：本模块只**返回**字符串，
//! 一行一条、不含换行符（换行由调用方补）。诊断一律走 stderr，不在这里。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::status::{Task, TaskState};
use crate::settings::parse_size_mb;

/// 协议版本。握手时协商。
pub const PROTOCOL_VERSION: u32 = 1;

/// 结构化错误码。**壳不得靠字符串匹配判断错误类型**（设计规格 §5.1）——
/// 它匹配的是这里的 `code` 常量值，不是 `message` 的措辞（措辞会变，码不会）。
///
/// 本模块只定义**协议层自己会返回**的码；各业务模块（engine/verify/…）的码
/// 由各自的响应构造点给出，届时一并列在这里以便壳侧有一份完整清单。
pub mod codes {
    /// 请求不是合法 JSON、不是 JSON 对象，或 `id`/`method` 的形状不对。
    pub const BAD_REQUEST: &str = "bad_request";
    /// 请求是合法的，但参数不在允许的取值集合内（含契约 §2.3 的 `-k`）。
    pub const INVALID_PARAMS: &str = "invalid_params";
    /// 握手的版本号与本内核不符。
    pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
    /// 内核自身无法把响应序列化出来。
    ///
    /// ⚠️ **这一支在实践中不可达**（契约 §4.4 的同类记账）：`Response` 的每个字段
    /// 都是 `Value`/`String`/整数/`bool`，`to_string` 不可能失败。
    /// 保留 `Result` 只是让调用方的 `?` 形状统一，**不要**为它编一条假测试。
    pub const INTERNAL: &str = "internal";
}

/// 一条请求。
///
/// `params` 缺省为 `Value::Null`（缺键与显式 `null` 等价）——`shutdown` 这类**真正无参**
/// 的方法就该只写 `{"id":1,"method":"shutdown"}`。
///
/// ⚠️ **`hello` 不是无参方法**：设计规格 §5.1 的原文是
/// `{"id":1,"method":"hello","params":{"protocol":1}}`，[`hello`] 也要求 `params`
/// 是对象、里面带 `protocol`（缺了回 `invalid_params`）。别照前半句去发一个
/// 不带 `params` 的 `hello`——那握不上手，而且要到端到端才暴露。
///
/// 未知的额外键**不**报错：协议会加方法、加字段，旧内核遇到新壳发来的多余键
/// 应当继续工作，而不是把整条请求判死。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// 错误体。`code` 是结构化的（见 [`codes`]），`message` 是给人看的一句话。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ErrorBody {
    /// 结构化错误码。**壳不得靠字符串匹配判断错误类型。**
    pub code: String,
    pub message: String,
}

impl ErrorBody {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        ErrorBody {
            code: code.to_string(),
            message: message.into(),
        }
    }

    /// 参数不在允许的取值集合内。
    pub fn invalid_params(message: impl Into<String>) -> Self {
        ErrorBody::new(codes::INVALID_PARAMS, message)
    }

    /// 请求本身不合法（畸形 JSON、形状不对、用了保留 id）。
    pub fn bad_request(message: impl Into<String>) -> Self {
        ErrorBody::new(codes::BAD_REQUEST, message)
    }
}

/// 一条响应。
///
/// ⚠️ **`result` 与 `error` 的互斥靠 `skip_serializing_if`，那是承重的**：
/// 少了它，`ok:true` 的响应里会多出一个 `"error":null`，壳的分支判断就要同时看
/// `ok` 和 `error` 两处；`ok:false` 里多出的 `"result":null` 同理。
/// 钉住它的是 `ok_response_omits_the_error_key` / `err_response_omits_the_result_key`
/// ——那两条**断言序列化后的文本**。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    /// 成功响应。`id` 由调用方原样回填请求的 `id`。
    pub fn ok(id: u64, result: Value) -> Self {
        Response {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// 失败响应。`id` 由调用方原样回填请求的 `id`。
    ///
    /// ⚠️ **解析失败时不要传常量 [`UNCORRELATED_ID`]**：先取
    /// [`RequestError::responder_id()`]——它会把"id 读得出来、其余字段坏了"的输入
    /// 回填成那个 id（`{"id":5}` → 5），只有连 id 都取不到时才给出保留值 0。
    /// 一律回 0 的后果是**壳那条 `id = 5` 的请求永远等不到配对响应**（挂起或超时）。
    /// 接线点见 `main.rs` 的读循环。
    pub fn err(id: u64, code: &str, message: &str) -> Self {
        Response {
            id,
            ok: false,
            result: None,
            error: Some(ErrorBody::new(code, message)),
        }
    }
}

/// 一条读不进来的请求。
///
/// ⚠️ **它带上 `id` 是有理由的，别退回成裸 `ErrorBody`**（修复轮 2 的 M-3）：
/// `{"id":5}`（缺 `method`）这类输入里，**`id` 明明读得出来**——若一律用保留值 0 回应，
/// 壳按约定把 `id == 0` 理解成"协议级错误、不是某个请求的结果"，于是**它那条 `id = 5`
/// 的请求永远等不到配对响应**（挂起或超时）。协议承诺的是"每条请求都能配对"，
/// 那就得在能配对的时候真的配上。
#[derive(Debug, Clone, PartialEq)]
pub struct RequestError {
    /// 从这一行里**抢救出来的**请求 id。
    ///
    /// `Some`：这一行是合法 JSON 对象且 `id` 是一个合法的 `u64`
    /// （`{"id":5}` 缺 `method`、`{"id":5,"method":2}` 之类）。
    /// `None`：连 `id` 都取不到（畸形 JSON、顶层不是对象、`id` 是字符串/负数/浮点/缺失）
    /// ——那时**只能**回保留值。
    pub id: Option<u64>,
    pub body: ErrorBody,
}

impl RequestError {
    fn with_id(id: Option<u64>, body: ErrorBody) -> Self {
        RequestError { id, body }
    }

    /// 没有可抢救的 id 时的构造（畸形 JSON / 非对象 / id 形状不对）。
    fn uncorrelated(body: ErrorBody) -> Self {
        RequestError { id: None, body }
    }

    /// 该回给壳的响应 id：**能配对就配对**，配不上才用保留值。
    pub fn responder_id(&self) -> u64 {
        self.id.unwrap_or(UNCORRELATED_ID)
    }
}

/// 解析失败时使用的保留 `id`。
///
/// 畸形 JSON **没有 `id` 可回**，而协议要求每条响应都能配对。用 0 作保留值：
/// 壳收到 `id == 0` 的响应时，知道这是**协议级错误**（不是某个请求的结果）。
/// 请求的 `id` 由壳从 1 起分配，因此 0 永远不会与真实请求冲突。
pub const UNCORRELATED_ID: u64 = 0;

/// 读一行并解析。空行（含只含空白与行尾符的行）返回 `Ok(None)`（跳过，不报错）。
///
/// 返回值是 `Err(e)` 时，调用方**必须**用 `e.responder_id()` 构造响应：
/// ```ignore
/// match read_request(line) {
///     Ok(Some(req)) => handle(req),
///     Ok(None) => {}                                   // 空行，什么都不做
///     Err(e) => write(Response::err(e.responder_id(), &e.body.code, &e.body.message)),
/// }
/// ```
///
/// ⚠️ **`id == 0` 的请求被当作 `bad_request` 拒掉**（见 [`UNCORRELATED_ID`]）：
/// 接受的后果不是"多配对一次"，而是**一条成功的响应被壳误读成协议级错误**。
/// 这是把约定变成不变量——保留值只有在对侧真的不会用到时才成立。
///
/// ⚠️ `Err` 的 `message` **不含原始行内容**：一条 1 MiB 的畸形行若把原文塞进
/// 错误消息，壳会收到一条 1 MiB 的 `message`。
pub fn read_request(line: &str) -> Result<Option<Request>, RequestError> {
    // 空行先判掉：对它调 `from_str` 会得到一条"expected value"错误，
    // 而那**不是**请求方的错——调用方按 `Ok(None)` 跳过即可。
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // ⚠️ 先要求顶层是 JSON **对象**，再交给 serde。
    // 少了这一句，`[1,"x"]` 会被**静默接受**成一条请求：serde 派生的结构体反序列化器
    // 同时实现了 `visit_seq`，于是 JSON 数组会按**位置**填进字段（id=1、method="x"、
    // params 缺省为 Null）。它不报错——只是协议边界上凭空多出一种"请求"的形状。
    // 钉住它的是 `read_request_rejects_malformed_input` 里的 `[1,"x"]` 一条：
    // 数组里的元素**类型正确**，所以只靠类型错拦不住（`[1,2,3]` 拦得住，那是巧合）。
    //
    // ⚠️ 下面两条 `map_err` 的 `message` 里**只带 serde 的位置描述，不带原文**：
    // 否则一条 1 MiB 的畸形行会变成一条 1 MiB 的 `message` 发给壳
    // （`read_request_handles_very_long_line` 钉住这条）。
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| RequestError::uncorrelated(ErrorBody::bad_request(format!("请求不是合法的 JSON: {e}"))))?;
    if !value.is_object() {
        return Err(RequestError::uncorrelated(ErrorBody::bad_request(
            "请求必须是一个 JSON 对象（形如 {\"id\":1,\"method\":\"hello\"}），实际不是对象",
        )));
    }
    // 到这里 `id` 通常已经读得出来了：`as_u64()` 对**字符串/负数/浮点/缺失**一律返回
    // `None`，正好覆盖"id 形状不对"的那几类——那些确实只能回保留值。
    // 其余的形状错（如缺 `method`）里 id 是好的，**必须**用它，否则壳那条请求永远等不到配对。
    let recoverable = value.get("id").and_then(Value::as_u64);
    let req: Request = serde_json::from_value(value).map_err(|e| {
        RequestError::with_id(
            recoverable,
            ErrorBody::bad_request(format!("请求字段的形状不对: {e}")),
        )
    })?;
    // 保留 id 不接受（见 UNCORRELATED_ID 的注释：接受的后果是"成功被误读成协议级错误"）。
    if req.id == UNCORRELATED_ID {
        return Err(RequestError::with_id(Some(req.id), ErrorBody::bad_request(format!(
            "id {UNCORRELATED_ID} 是保留值（协议级错误专用），请求的 id 必须从 1 起分配"
        ))));
    }
    Ok(Some(req))
}

/// 序列化成一行（**不含换行符**——由调用方补）。
///
/// `Err` 那一支在实践中不可达，见 [`codes::INTERNAL`]。
pub fn write_response(r: &Response) -> Result<String, ErrorBody> {
    // 这一支不可达（见 codes::INTERNAL）：`Response` 的每个字段都能序列化，
    // 且 `to_string` 写的是内存缓冲、不会有 io 错误。留着只是让调用方的 `?` 统一。
    serde_json::to_string(r).map_err(|e| ErrorBody::new(codes::INTERNAL, format!("响应序列化失败: {e}")))
}

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

/// `hello` 的入参：`{"protocol":1}`。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HelloParams {
    pub protocol: u32,
}

/// `hello` 的结果。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HelloResult {
    pub protocol: u32,
    /// `-k`（最小分片大小）的取值集合，契约 §2.3。
    ///
    /// **它必须在协议里传输**，不能只写成壳里的一份硬编码：壳是另一个语言、
    /// 另一个阶段的产物，它拿不到本 crate 的常量。把集合交给壳，
    /// 壳才建得出那个"让客户不可能输错"的枚举控件（§2.3 的原话）。
    pub min_split_size_choices: Vec<String>,
}

/// 握手：版本协商（设计规格 §5.1）。
///
/// 版本**相符**才成功；不符一律 [`codes::PROTOCOL_MISMATCH`]，**不做降级协商**：
/// 目前只有版本 1，降级协商是没有第二个版本时的空想，而它会掩盖"壳与内核不是一套"。
/// 消息里带上双方版本号，壳才能把它显示成一句能看懂的话（设计规格 §9）。
pub fn hello(params: &Value) -> Result<HelloResult, ErrorBody> {
    // ⚠️ 同 `read_request`：必须**先要求对象**。serde 派生的结构体反序列化器也实现了
    // `visit_seq`，于是 `params = [1]` 会被按位置读成 `protocol = 1` 而**静默成功**
    // ——`hello_negotiates_the_protocol_version` 里 `[1]` 那一条正是为它写的。
    if !params.is_object() {
        return Err(ErrorBody::invalid_params(format!(
            "hello 的参数必须是一个 JSON 对象（形如 {{\"protocol\":{PROTOCOL_VERSION}}}）"
        )));
    }
    let p: HelloParams = serde_json::from_value(params.clone()).map_err(|e| {
        ErrorBody::invalid_params(format!(
            "hello 的参数形状不对（应为 {{\"protocol\":{PROTOCOL_VERSION}}}）: {e}"
        ))
    })?;
    if p.protocol != PROTOCOL_VERSION {
        return Err(ErrorBody::new(
            codes::PROTOCOL_MISMATCH,
            format!(
                "协议版本不匹配：内核是 {PROTOCOL_VERSION}，请求方是 {}",
                p.protocol
            ),
        ));
    }
    Ok(HelloResult {
        protocol: PROTOCOL_VERSION,
        min_split_size_choices: min_split_size_choices(),
    })
}

// ---------------------------------------------------------------------------
// 契约 §2.3：`-k` 的取值集合（在协议层限定）
// ---------------------------------------------------------------------------

/// `-k` 的取值范围（MB），与 [`parse_size_mb`] 的范围逐字一致。
///
/// ⚠️ **这两个常数只用来生成给壳的枚举面，不参与校验**：校验直接调
/// [`parse_size_mb`]（边界只有那一份实现，见契约 §2.1——"不把边界抄第二遍"）。
/// `min_split_size_choices_agree_with_settings_validation` 把两边钉在一起：
/// 枚举面里的每个值都必须过 settings 层，两个界外邻居必须被两边同时拒。
/// **改动要同向。**
pub const MIN_SPLIT_SIZE_MIN_MB: i64 = 1;
pub const MIN_SPLIT_SIZE_MAX_MB: i64 = 100;

/// 给壳的枚举面：`["1M", "2M", …, "100M"]`。
pub fn min_split_size_choices() -> Vec<String> {
    (MIN_SPLIT_SIZE_MIN_MB..=MIN_SPLIT_SIZE_MAX_MB)
        .map(|n| format!("{n}M"))
        .collect()
}

/// 校验一个 `-k` 取值，返回**规范串**（`"{n}M"`）。
///
/// 契约 §2.3 点名这一项：`-k` 是**写错会让 aria2 直接启动失败**的那一项，
/// 设置面板的价值是让客户**不可能**输错。契约给的出路是"用 `enum` 或在 protocol 层
/// 就限定取值集合"——`settings` 层的模型必须与 Go 一致（`min_split_size: String`，
/// 改成 `enum` 会改变落盘 JSON 的形状），**所以这里就是那个 protocol 层**。
///
/// 边界**只调** [`parse_size_mb`]，不在这里另判一套：同一组边界抄两遍就是漂移的开始
/// （契约 §2.1）。因此大小写与首尾空白与 settings 层同口径（`" 20m "` → `"20M"`），
/// 范围也同口径（`1M`–`100M`）。
///
/// 这一条**不替代** `Settings::validate`（它还管另外六项）：壳写参数时两道都要过。
pub fn check_min_split_size(value: &str) -> Result<String, ErrorBody> {
    // 范围只有 parse_size_mb 那一份实现：这里只把它的字符串错误换成结构化错误码。
    let n = parse_size_mb(value).map_err(ErrorBody::invalid_params)?;
    Ok(format!("{n}M"))
}

// ---------------------------------------------------------------------------
// 契约 §8.4 / Ruling #88：传输列表要能让壳区分"已暂停"
// ---------------------------------------------------------------------------

/// 传输列表的一项——`transfer_list` 响应里的一行。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TransferItem {
    pub gid: String,
    pub total: i64,
    pub completed: i64,
    pub speed: i64,
    pub conns: i32,
    /// 领域状态：四态合成的输入。
    pub state: TaskState,
    /// **aria2 的原始状态串，逐字**（`RawTask::status`）。
    ///
    /// ⚠️ **它不能被删掉，也不能由 `state` 反推**：`TaskState` 把 aria2 的
    /// `paused` 与 `waiting` 映射成**同一个变体**（契约 §8 表的 `#14`，Go 侧的别名，
    /// 为表达意图而保留），而设计规格 §8.4 要求界面把暂停的任务**归入下载中并标注
    /// 「已暂停」**——域状态里这两者长得一模一样，**壳拿不到这个区分就渲染不出标注**。
    ///
    /// **为什么不给 `TaskState` 加 `Paused` 变体**：那会扰动一张承重的排名表
    /// （`task_rank`：活跃 3 > 失败 2 > 完成 1 > 移除 0）。**加字段是加法，加变体是改表。**
    pub raw_status: String,
    /// aria2 的 `errorMessage`，无错时为空串（与领域模型 `Task::err` 同形）。
    /// 名字用 `error_message` 而不是 `error`：后者已经是响应级错误对象的键。
    pub error_message: String,
    /// `manifest` 相对路径，来自 GID 映射；没有映射的任务是 `None`。
    /// 界面按行显示文件名要靠它（`gid` 只是句柄，不是给人看的）。
    pub path: Option<String>,
}

impl TransferItem {
    /// 由领域模型 + **原始状态串** + 路径造一项。
    ///
    /// `raw_status` 必须逐字来自 `RawTask::status`（`engine::status`），
    /// **不得**写成 `format!("{:?}", task.state)` 之类的反推——那正是本字段要消灭的东西。
    ///
    /// ⚠️ **调用方的取法只有一种**：`snapshot().raw_status.get(&task.gid)`——
    /// 那个 map 是 `gid → aria2 原始状态串`，由 `Daemon::snapshot` 在同一趟
    /// `list()` 里顺手带出（零额外 RPC）。取不到（`None`）说明那个 GID 不在本次
    /// 快照里，此时**不要**退回去反推：传空串，让壳显式看到"不知道"。
    ///
    /// （签名收 `&str` 而不是 `&Snapshot`，是为了让本模块只依赖 `engine::status`
    /// 而不依赖 `engine::daemon`（连带 `RpcClient`/`ureq`）——可读的依赖面值这个代价。）
    pub fn new(task: &Task, raw_status: &str, path: Option<String>) -> Self {
        TransferItem {
            gid: task.gid.clone(),
            total: task.total,
            completed: task.completed,
            speed: task.speed,
            conns: task.conns,
            state: task.state,
            raw_status: raw_status.to_string(),
            error_message: task.err.clone(),
            path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::status::RawTask;
    use crate::settings::default_settings;
    use serde_json::json;

    fn parse(line: &str) -> Request {
        read_request(line)
            .unwrap_or_else(|e| panic!("{line:?} 应能解析，却报 {}: {}", e.body.code, e.body.message))
            .expect("非空行不该返回 None")
    }

    /// 正常往返（设计规格 §5.1 的示例形状）。
    ///
    /// 守：`id`/`method`/`params` 三项原样读进来（含非 ASCII 的交付码），
    /// 且成功响应序列化后能**重新读回**、`id` 能与请求配对——这是协议的核心承诺。
    /// 顺带钉住 JSON Lines 的"一行一条"：文本里不得有换行符。
    #[test]
    fn request_round_trip_and_response_shape() {
        let req = parse(r#"{"id":7,"method":"list_dir","params":{"path":"C24-8_×_25WS024"}}"#);
        assert_eq!(req.id, 7, "id 没原样读进来");
        assert_eq!(req.method, "list_dir", "method 没原样读进来");
        assert_eq!(
            req.params["path"],
            Value::String("C24-8_×_25WS024".to_string()),
            "非 ASCII 的参数被改写/转义了"
        );

        let txt = write_response(&Response::ok(req.id, json!({"entries": ["a.txt"]})))
            .expect("序列化不该失败");
        assert!(
            !txt.contains('\n'),
            "JSON Lines 要求一行一条，序列化结果里出现了换行符: {txt:?}"
        );
        let back: Value = serde_json::from_str(&txt).expect("响应必须是一行合法 JSON");
        assert_eq!(back["id"], json!(7), "响应的 id 必须与请求配对: {txt}");
        assert_eq!(back["ok"], json!(true));
        assert_eq!(back["result"]["entries"], json!(["a.txt"]));
    }

    /// `params` 缺省。
    ///
    /// 守：`{"id":1,"method":"hello"}` 这种**没有 params 键**的请求必须能读，
    /// 且 `params` 落成 `Value::Null`（`hello` 这类无参方法就是这么发的）。
    /// 另外钉住"未知键不致命"：协议会加字段，旧内核遇到多余键要继续工作。
    #[test]
    fn request_params_is_optional() {
        let req = parse(r#"{"id":1,"method":"hello"}"#);
        assert_eq!(req.params, Value::Null, "缺 params 应落成 Null");
        assert_eq!(req.method, "hello");

        // 显式 null 与缺键等价
        assert_eq!(parse(r#"{"id":1,"method":"hello","params":null}"#).params, Value::Null);

        // 多余的键不得让整条请求失败
        let req = parse(r#"{"id":2,"method":"hello","params":{},"future_field":123}"#);
        assert_eq!(req.id, 2);
        assert_eq!(req.params, json!({}));
    }

    /// 非法 JSON 与形状不对的请求。
    ///
    /// 守：这十二种输入都必须返回**结构化错误码** `bad_request`（而不是 panic、
    /// 也不是 `Ok`）。其中两类是重点：
    ///   - `id` 的四种坏形状（字符串/浮点/负数/缺失）——它们是"响应配不上对"的来源；
    ///   - **JSON 数组**：serde 派生的结构体反序列化器也实现 `visit_seq`，
    ///     `[1,"x"]` 会被按**位置**填成 `id=1`、`method="x"`、`params` 缺省——
    ///     **静默通过**。`[1,2,3]` 只是碰巧因 `2` 不是字符串而报错，
    ///     **光靠它会把"数组能当请求用"这个洞放过去**（所以两条都在）。
    #[test]
    fn read_request_rejects_malformed_input() {
        let bad: &[(&str, &str)] = &[
            ("非 JSON", "not json"),
            ("截断的 JSON", r#"{"id":1,"method":"x""#),
            ("合法 JSON 但不是对象", "[1,2,3]"),
            ("数组按位置填得进字段（类型都对）", r#"[1,"x"]"#),
            ("JSON 标量", "42"),
            ("JSON 字符串", r#""hello""#),
            ("缺 id", r#"{"method":"x"}"#),
            ("缺 method", r#"{"id":1}"#),
            ("id 是字符串", r#"{"id":"1","method":"x"}"#),
            ("id 是浮点", r#"{"id":1.0,"method":"x"}"#),
            ("id 为负", r#"{"id":-1,"method":"x"}"#),
            ("method 是数字", r#"{"id":1,"method":2}"#),
        ];
        for (name, line) in bad {
            let e = read_request(line).expect_err(&format!("{name} 应被拒绝: {line:?}"));
            assert_eq!(
                e.body.code,
                codes::BAD_REQUEST,
                "{name} 的错误码不对: {e:?}"
            );
            assert!(!e.body.message.is_empty(), "{name} 的错误消息为空");
        }
    }

    /// **能配对的就必须配上**（修复轮 2 的 M-3）。
    ///
    /// 守：`{"id":5}`（缺 `method`）这类输入里 `id` **明明读得出来**，响应就必须用 5。
    /// 一律回保留值 0 的后果不是"少配对一次"，而是**壳那条 `id = 5` 的请求永远等不到
    /// 配对响应**——协议承诺的是"每条请求都能配对"。
    ///
    /// 同时钉住反方向：**连 id 都取不到**的输入（畸形 JSON、非对象、`id` 形状不对）
    /// 必须回保留值 0。两侧都测，免得把 `responder_id()` 写成恒回填或恒 0。
    ///
    /// 判别力：把 `RequestError.id` 写死成 `None`（`responder_id` 恒返回 0）→ 第一条红；
    /// 写死成"回填"（畸形 JSON 也硬塞一个 id）→ 第二条红。
    #[test]
    fn recoverable_id_is_echoed_back() {
        // ① id 读得出来 → 必须回填
        for (line, want) in [
            (r#"{"id":5}"#, 5u64),
            (r#"{"id":7,"method":2}"#, 7),
        ] {
            let e = read_request(line).expect_err("这些输入必须被拒绝");
            assert_eq!(
                e.responder_id(),
                want,
                "{line:?} 里的 id 读得出来，响应的 id 必须是 {want}（回 0 会让壳那条请求永远等不到配对）"
            );
            // 端到端形状：把它序列化成响应，id 必须是那个数
            let txt = write_response(&Response::err(e.responder_id(), &e.body.code, &e.body.message))
                .expect("序列化不该失败");
            assert!(
                txt.starts_with(&format!(r#"{{"id":{want},"#)),
                "响应文本里的 id 不是 {want}: {txt}"
            );
        }

        // ② id 取不到 → 必须回保留值 0
        for line in ["{", "not json", "[1,2]", r#"{"id":"5"}"#, r#"{"id":-1}"#, r#"{"id":1.0,"method":"x"}"#] {
            let e = read_request(line).expect_err("这些输入必须被拒绝");
            assert_eq!(
                e.id, None,
                "{line:?} 的 id 取不到，不该假装知道: {e:?}"
            );
            assert_eq!(e.responder_id(), UNCORRELATED_ID, "{line:?}");
        }
    }

    /// 空行。
    ///
    /// 守：空行与只含空白/行尾符的行都必须 `Ok(None)`（跳过，不报错），
    /// **不得**被当成畸形请求回一条错误——调用方一般已剥掉 `\n`，
    /// 但 `\r\n` 与手写测试会用带行尾符的输入。
    #[test]
    fn read_request_skips_blank_lines() {
        for line in ["", " ", "\t", "  \r", "\n", "\r\n", " \t \n"] {
            assert!(
                read_request(line)
                    .unwrap_or_else(|e| panic!("{line:?} 是空行，不该报错：{}", e.body.message))
                    .is_none(),
                "{line:?} 应被跳过（Ok(None)）"
            );
        }
    }

    /// 超长行。
    ///
    /// 守两件事：① 1 MiB 的**合法**请求必须解析成功且参数原样带出来（不许有隐含行长上限）；
    /// ② 1 MiB 的**垃圾**行必须报错而**不是 panic**，且错误消息里**不带原文**
    /// （否则壳会收到一条和输入一样大的 `message`）。
    /// ③ 深嵌套不得把栈打爆：`serde_json` 的递归上限会先返回 `Err`
    /// （若有人日后打开 `unbounded_depth`，这条会以栈溢出红掉）。
    #[test]
    fn read_request_handles_very_long_line() {
        let big = "x".repeat(1 << 20);
        let line = format!(r#"{{"id":9,"method":"list_dir","params":{{"path":"{big}"}}}}"#);
        let req = read_request(&line)
            .expect("1 MiB 的合法请求不该失败")
            .expect("非空行");
        assert_eq!(
            req.params["path"],
            Value::String(big),
            "超长参数被截断或改写了"
        );

        let junk = "y".repeat(1 << 20);
        let e = read_request(&junk).expect_err("1 MiB 的垃圾行应报错");
        assert_eq!(e.body.code, codes::BAD_REQUEST);
        assert!(
            e.body.message.len() < 512,
            "错误消息里带了整行原文（{} 字节）——壳会收到一条巨大的 message",
            e.body.message.len()
        );

        let brackets = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let deep = format!(r#"{{"id":1,"method":"m","params":{brackets}}}"#);
        assert!(
            read_request(&deep).is_err(),
            "10 万层嵌套必须被递归上限挡成 Err（放行了就是栈溢出风险）"
        );
    }

    /// `ok:true` 时 `error` 键不得出现。
    ///
    /// ⚠️ **断言的是文本**：只断言结构体的话，`skip_serializing_if` 漏掉也照样绿。
    /// 最后一条同时钉住键序（`id`/`ok`/`result`）与"不得有多余键"——
    /// 响应形状是壳按固定形状解析的契约面。
    #[test]
    fn ok_response_omits_the_error_key() {
        let txt = write_response(&Response::ok(7, json!({"entries": ["a.txt"]})))
            .expect("序列化不该失败");
        assert!(
            !txt.contains("\"error\""),
            "ok:true 的响应里出现了 error 键: {txt}"
        );
        assert!(txt.contains("\"ok\":true"), "{txt}");
        assert!(txt.contains("\"result\""), "ok:true 必须带 result: {txt}");
        assert_eq!(txt, r#"{"id":7,"ok":true,"result":{"entries":["a.txt"]}}"#);
    }

    /// `ok:false` 时 `result` 键不得出现，且**三个错误码的字面量**逐个钉住。
    ///
    /// 同上是**文本**断言，且断言的是**全等文本**——不是"结构体里 code 等于某个常量"
    /// （那种写法在改掉字面量时照样绿）。**壳的分支面就是这些字面量**：
    /// 契约 §5.1 明写「壳不得靠字符串匹配判断错误类型」，它匹配的正是 `code` 的值。
    /// 三个码里只钉一个是不一致的（修复轮 2 的 M-1）。
    ///
    /// 同时钉住 err 响应也是**一行**（M-7）：`to_string_pretty` 会让它变多行。
    #[test]
    fn err_response_omits_the_result_key() {
        let txt = write_response(&Response::err(
            3,
            codes::INVALID_PARAMS,
            "分片数必须在 1–16 之间",
        ))
        .expect("序列化不该失败");
        assert!(
            !txt.contains("\"result\""),
            "ok:false 的响应里出现了 result 键: {txt}"
        );
        assert!(txt.contains("\"ok\":false"), "{txt}");
        assert!(
            !txt.contains('\n'),
            "JSON Lines 要求一行一条，err 响应里出现了换行符: {txt:?}"
        );
        assert_eq!(
            txt,
            r#"{"id":3,"ok":false,"error":{"code":"invalid_params","message":"分片数必须在 1–16 之间"}}"#
        );

        // 另外两个码也必须**逐字**出现在响应文本里（壳按这三个字面量分支）
        let txt = write_response(&Response::err(
            4,
            codes::BAD_REQUEST,
            "请求不是合法的 JSON",
        ))
        .expect("序列化不该失败");
        assert_eq!(
            txt,
            r#"{"id":4,"ok":false,"error":{"code":"bad_request","message":"请求不是合法的 JSON"}}"#
        );
        let txt = write_response(&Response::err(
            5,
            codes::PROTOCOL_MISMATCH,
            "协议版本不匹配",
        ))
        .expect("序列化不该失败");
        assert_eq!(
            txt,
            r#"{"id":5,"ok":false,"error":{"code":"protocol_mismatch","message":"协议版本不匹配"}}"#
        );
    }

    /// 保留 id。
    ///
    /// 守三条：① 畸形行（**连 id 都取不到**）→ 调用方用 `UNCORRELATED_ID` 造出的响应，
    /// `id` 就是 0，壳据此认出"这是协议级错误"；② 响应文本里的 `code` 逐字是
    /// `bad_request`（不是常量比对——见 `err_response_omits_the_result_key` 的说明）；
    /// ③ **请求侧也不接受 `id:0`**——少了这条，一条 `{"id":0,"method":"shutdown"}`
    /// 会被回一条 `"ok":true,"id":0`，而壳按约定把它读成协议级错误：**成功被误读成失败**。
    #[test]
    fn uncorrelated_id_is_reserved_for_protocol_errors() {
        assert_eq!(UNCORRELATED_ID, 0, "保留 id 必须是 0（壳从 1 起分配）");

        let e = read_request("{").expect_err("畸形 JSON 应报错");
        assert_eq!(e.id, None, "畸形 JSON 里没有可抢救的 id: {e:?}");
        let txt =
            write_response(&Response::err(e.responder_id(), &e.body.code, &e.body.message))
                .expect("序列化不该失败");
        let back: Value = serde_json::from_str(&txt).expect("响应必须是合法 JSON");
        assert_eq!(back["id"], json!(0), "协议级错误必须用保留 id: {txt}");
        assert_eq!(back["ok"], json!(false));
        assert!(
            txt.contains(r#""code":"bad_request""#),
            "壳按这个字面量分支（契约 §5.1），实际文本: {txt}"
        );

        let e = read_request(r#"{"id":0,"method":"shutdown"}"#)
            .expect_err("保留 id 不得被请求使用");
        assert_eq!(e.body.code, codes::BAD_REQUEST, "{e:?}");
    }

    /// 传输列表要能让壳区分「已暂停」（契约 §8.4 / Ruling #88）。
    ///
    /// 守：`paused` 与 `waiting` 两个条目在**领域状态上完全相同**（都是 `Waiting`），
    /// 但序列化出来的文本必须不同——差别只在 `raw_status`。
    /// 壳拿这个字段才画得出「已暂停」的标注。
    ///
    /// ⚠️ 输入走 **`RawTask` 的线上形态**（aria2 `tellWaiting` 的真实形状），
    /// 不直接构造结构体：本项目反复栽在"测试绕过了会出错的那条路"上
    /// （`RawTask` 的 camelCase 就是这么漏掉四个字段的）。
    ///
    /// ⚠️ **夹具的选值标准是"能被区分"，不是"能跑通"**（修复轮 2 的 I-1）：
    /// 本测试的全部职责就是搬运字段，而初版夹具把 `downloadSpeed` 取 `"0"`、
    /// `errorMessage` 取 `""`、两个夹具共用同一个 `gid`、领域状态又都是 `Waiting`，
    /// 于是 `error_message`/`speed`/`gid`/`state` 四个搬运**各自被改坏都全绿**
    /// （断言与"恒空/恒 0"不可区分）。现在三个夹具的每个字段都可区分：
    ///   - `paused`：**非空**错误文案 + **非零**速度 + 自己的 gid；
    ///   - `waiting`：与 `paused` **领域状态相同**（这是 raw_status 存在的理由）；
    ///   - `complete`：**非 `Waiting`** 的领域状态（钉 `state` 的透传）。
    ///
    /// 判别力：`raw_status` 删掉/反推、`error_message` 恒空、`speed` 恒 0、
    /// `gid` 恒空、`state` 写死成 `Waiting` —— 五种改动各有一条断言红。
    #[test]
    fn transfer_item_carries_the_raw_aria2_status() {
        let raw_of = |gid: &str, status: &str, speed: &str, err: &str| -> RawTask {
            serde_json::from_str(&format!(
                r#"{{"gid":"{gid}","status":"{status}","totalLength":"65536",
                    "completedLength":"32768","downloadSpeed":"{speed}","connections":"1",
                    "errorMessage":"{err}","dir":"/tmp","errorCode":"0"}}"#
            ))
            .expect("aria2 的真实形态必须能读进来")
        };
        let paused = raw_of("g-paused", "paused", "1024", "连接超时");
        let queued = raw_of("g-queued", "waiting", "2048", "");
        let done = raw_of("g-done", "complete", "3072", "");

        let t_paused = paused.to_task();
        let t_queued = queued.to_task();
        assert_eq!(
            t_paused.state,
            TaskState::Waiting,
            "契约 §8 的 #14：aria2 的 paused 与 waiting 映到同一个变体"
        );
        assert_eq!(
            t_paused.state, t_queued.state,
            "两者的领域状态**必须**相同——差别只能由 raw_status 承载"
        );

        let item_paused =
            TransferItem::new(&t_paused, &paused.status, Some("a/b.txt".to_string()));
        let item_queued =
            TransferItem::new(&t_queued, &queued.status, None); // 没有路径映射的任务
        let item_done = TransferItem::new(&done.to_task(), &done.status, None);

        let txt = write_response(&Response::ok(1, serde_json::to_value(&item_paused).unwrap()))
            .expect("序列化不该失败");
        assert!(
            txt.contains(r#""raw_status":"paused""#),
            "壳要靠这个字段画「已暂停」，实际文本: {txt}"
        );
        assert!(
            txt.contains(r#""state":"waiting""#),
            "领域状态照旧是 waiting: {txt}"
        );
        assert_ne!(
            item_paused, item_queued,
            "暂停与排队必须可区分，否则界面渲染不出标注"
        );
        assert_eq!(
            item_queued.raw_status, "waiting",
            "raw_status 必须是逐字透传，不是从 state 反推"
        );

        // ---- 逐字段搬运：每个值都必须与"零值/默认值"可区分 --------------------
        // gid：三个夹具各不相同，写死/搬运错一个都看得见
        assert_eq!(item_paused.gid, "g-paused", "gid 没搬过来");
        assert_eq!(item_queued.gid, "g-queued", "gid 没搬过来或被两个夹具共用");
        // 数值（界面按行渲染进度）
        assert_eq!(item_paused.total, 65536, "total 没搬过来");
        assert_eq!(item_paused.completed, 32768, "completed 没搬过来");
        assert_eq!(item_paused.conns, 1, "conns 没搬过来");
        assert_eq!(item_paused.speed, 1024, "speed 没搬过来（夹具取的是非零值）");
        // 失败原因（设计规格 §9：单个文件失败要显示 aria2 的 errorMessage）
        assert_eq!(
            item_paused.error_message, "连接超时",
            "error_message 没搬过来（夹具取的是非空值，恒空会被逮住）"
        );
        assert_eq!(item_queued.error_message, "", "无错时是空串");
        // 领域状态必须是**透传**的：`complete` 那条能戳穿"写死成 Waiting"
        assert_eq!(
            item_done.state,
            TaskState::Complete,
            "state 没透传（写死成 Waiting？）"
        );
        assert_eq!(item_done.raw_status, "complete", "raw_status 没透传");
        // 路径：有映射与无映射两种都要能表达
        assert_eq!(item_paused.path.as_deref(), Some("a/b.txt"));
        assert_eq!(item_queued.path, None, "没有 GID 映射时应是 None");
    }

    /// 契约 §2.3：`-k` 的取值集合在协议层限定。
    ///
    /// 守：`1M`–`100M` 两端与中间值必须通过并归一成规范串；集合外的十二种写法
    /// （越界/换单位/自由文本/小数/负号/空串）必须返回**结构化错误码**
    /// `invalid_params`，而不是返回一个字符串错误让壳去猜。
    /// `" 20m "` 一条**故意钉住与 settings 层同口径**（同一份范围、同一套归一），
    /// 免得这里另立一套更严的边界而与 `validate` 漂开（契约 §2.1）。
    #[test]
    fn min_split_size_is_limited_to_the_contract_value_set() {
        assert_eq!(check_min_split_size("1M").expect("下限应通过"), "1M");
        assert_eq!(check_min_split_size("100M").expect("上限应通过"), "100M");
        assert_eq!(check_min_split_size("20M").expect("默认值应通过"), "20M");
        assert_eq!(
            check_min_split_size(" 20m ").expect("大小写/空白与 settings 层同口径"),
            "20M",
            "归一后的规范串才是要下发给 aria2 的形状"
        );

        let bad = [
            "0M", "101M", "500K", "200M", "abc", "", "20", "20MB", "M", "M20", "-1M", "1.5M",
        ];
        for v in bad {
            let e = check_min_split_size(v)
                .expect_err(&format!("{v:?} 不在 1M–100M 的取值集合内，应被拒绝"));
            assert_eq!(e.code, codes::INVALID_PARAMS, "{v:?} 的错误码不对: {e:?}");
            assert!(!e.message.is_empty(), "{v:?} 的错误消息为空");
        }
    }

    /// 枚举面与 settings 层的边界**互相钉住**（防漂移）。
    ///
    /// 守：给壳的那 100 个取值，每一个都必须同时过协议层与 `Settings::validate`；
    /// 两个界外邻居（`0M`/`101M`）必须被**两边同时**拒。
    /// 少了这条，`MIN_SPLIT_SIZE_*_MB` 与 `parse_size_mb` 的范围可以各走各的
    /// ——壳照枚举面选的值会被 settings 层拒，或者反过来。
    #[test]
    fn min_split_size_choices_agree_with_settings_validation() {
        let choices = min_split_size_choices();
        assert_eq!(choices.len(), 100, "枚举面就是 1M–100M 这 100 个值");
        assert_eq!(choices.first().map(String::as_str), Some("1M"));
        assert_eq!(choices.last().map(String::as_str), Some("100M"));

        for c in &choices {
            assert!(
                check_min_split_size(c).is_ok(),
                "{c} 在枚举面里，协议层却拒了它"
            );
            let mut s = default_settings();
            s.min_split_size = c.clone();
            assert!(
                s.validate().is_ok(),
                "{c} 在枚举面里，settings::validate 却拒了它"
            );
        }

        for bad in ["0M", "101M"] {
            assert!(check_min_split_size(bad).is_err(), "{bad} 应被协议层拒绝");
            let mut s = default_settings();
            s.min_split_size = bad.to_string();
            assert!(
                s.validate().is_err(),
                "{bad} 被协议层拒了，settings 层却接受——两份边界已经漂了"
            );
        }
    }

    /// 握手：版本协商。
    ///
    /// 守：版本相符 → 成功，且**结果里带着枚举面**（壳靠它建枚举控件，
    /// 否则 §2.3 的"不可能输错"在壳侧无从落地）；版本不符 → `protocol_mismatch`
    /// 且消息里能看出双方版本（设计规格 §9：失败要能显示成人话）；
    /// 参数形状不对（缺 `protocol`/类型不对/`params` 不是对象）→ `invalid_params`。
    #[test]
    fn hello_negotiates_the_protocol_version() {
        let ok = hello(&json!({"protocol": PROTOCOL_VERSION})).expect("版本相符应握手成功");
        assert_eq!(ok.protocol, PROTOCOL_VERSION);
        assert_eq!(
            ok.min_split_size_choices,
            min_split_size_choices(),
            "枚举面必须随握手交给壳"
        );

        let e = hello(&json!({"protocol": PROTOCOL_VERSION + 1})).expect_err("版本不符应拒绝");
        assert_eq!(e.code, codes::PROTOCOL_MISMATCH, "{e:?}");
        assert!(
            e.message.contains(&(PROTOCOL_VERSION + 1).to_string())
                && e.message.contains(&PROTOCOL_VERSION.to_string()),
            "消息里要能看出双方版本，实际: {}",
            e.message
        );

        // `[1]` 那一条是**形状**洞的守卫：serde 的结构体反序列化器也实现 `visit_seq`，
        // 少一次对象判定，`params = [1]` 就会被按位置读成 `protocol = 1` 而握手成功。
        for p in [
            json!({}),
            json!({"protocol": "1"}),
            Value::Null,
            json!(1),
            json!([1]),
            json!([1, 2]),
        ] {
            let e = hello(&p).expect_err(&format!("{p} 不是合法的 hello 参数"));
            assert_eq!(e.code, codes::INVALID_PARAMS, "{p}: {e:?}");
        }
    }
}
