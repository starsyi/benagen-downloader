//! protocol —— 内核 stdio JSON Lines 协议的**壳侧手写镜像**（规格 §5）。
//!
//! ⚠️ **本文件是手写的，不是从 `core/src/protocol.rs` 生成的，也**不依赖** `core` 这个 crate。**
//!    理由（规格 §5 第 1 条）：依赖它会把"语言中立的进程边界"降级成"两个 crate 的
//!    编译期耦合"，日后内核换语言时这条边界就报错了。漂移由**任务 8 的 e2e**（对着真内核
//!    二进制跑握手）钉住——那是**故意的分工**，不要为了"保证一致"把 `core` 变成依赖。
//!
//! 字段真相有两处，按优先级：
//!
//!   1. `core/src/protocol.rs` —— 信封（`Request`/`Response`/`ErrorBody`）、
//!      `HelloParams`/`HelloResult`、`TransferItem`、`codes`。**逐字段照抄，名字一个不改。**
//!   2. 内核的**响应构造点**（信封之外的聚合类型不在 `protocol.rs` 里）：
//!      `core/src/main.rs` 的 `node_json`/`entries_json`/`op_load_delivery`/`op_enqueue`/
//!      `op_transfer_list`/`op_verify_status`/`op_get_settings`，`core/src/engine/rpc.rs` 的
//!      `GlobalStat`，`core/src/settings.rs` 的 `Settings`，`core/src/engine/status.rs` 的
//!      `TaskState`。每一条的出处都写在它自己的文档注释里。
//!
//! ⚠️ **线上键名一律 snake_case**（内核的字段名就是 snake_case，`json!` 里的键也是）。
//!    macOS 壳靠 `.convertFromSnakeCase` 消化它；Rust 侧字段名天然同形，**不需要任何
//!    `#[serde(rename_all)]`**——但 `TaskState`/`FileState` 这两个**变体名**是例外（见下）。
//!
//! ⚠️ **宽容的方向只有一个**：壳要容忍内核**新增字段**（内核一升级壳就解不动是灾难），
//!    反方向（壳发内核不认的字段）由内核负责（`core/src/protocol.rs` 的 `Request` 文档：
//!    "未知的额外键**不**报错"）。所以这里的结构体**一律不加** `deny_unknown_fields`。
//!    但**缺字段仍然是硬错误**：字段被改名/删除必须**响亮地**失败，那是漂移探测器。
//!    只有线上真的可能缺的键（`TreeNode` 的空根、`FileNode::source_mtime` 这类）才用
//!    `Option` + `#[serde(default)]`。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// 信封：请求 / 响应 / 错误
// ---------------------------------------------------------------------------

/// 协议版本。握手时协商（`hello`）。
///
/// 出处：`core/src/protocol.rs`。版本**相符**才握手成功，**不做降级协商**——
/// 目前只有版本 1，降级协商会掩盖"壳与内核不是一套"。
pub const PROTOCOL_VERSION: u32 = 1;

/// 解析失败时使用的保留 `id`（`core/src/protocol.rs`）。
///
/// 壳收到 `id == 0` 的响应时，知道这是**协议级错误**、不是某个请求的结果。
/// 壳发请求的 `id` 从 1 起分配，所以 0 永远不会与真实请求冲突。
pub const UNCORRELATED_ID: u64 = 0;

/// 结构化错误码（`core/src/protocol.rs` 的 `codes` ∪ `core/src/main.rs` 的 `kcodes`）。
///
/// ⚠️ **壳不得靠字符串匹配判断错误类型**（契约 §5.1）：它匹配的是这里的**值**，
/// 不是 `message` 的措辞——措辞会变，码不会。所以字面量是本模块最承重的东西，
/// 由 `all_thirteen_error_codes_are_mirrored_verbatim` 逐个钉住。
///
/// 内核那边这份清单**分住在两个模块**（`protocol::codes` 是协议层自己会返回的四个，
/// `kcodes` 是业务侧的九个），内核的注释明写"壳侧的完整码表 = `protocol::codes` ∪ `kcodes`"
/// ——壳这边合成**一份**，因为它只有一个消费面（界面上的分支）。
pub mod codes {
    /// 请求不是合法 JSON、不是 JSON 对象，或 `id`/`method` 的形状不对。
    pub const BAD_REQUEST: &str = "bad_request";
    /// 请求是合法的，但参数不在允许的取值集合内。
    pub const INVALID_PARAMS: &str = "invalid_params";
    /// 握手的版本号与本内核不符。
    pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
    /// 内核自身无法把响应序列化出来。
    ///
    /// ⚠️ 实践中**不可达**（`core/src/protocol.rs` 的记账）：每个字段都能序列化，
    /// 写的是内存缓冲、不会有 io 错误。壳照旧要认这个码——**认得一个不会来的码
    /// 比漏认一个会来的码便宜**。
    pub const INTERNAL: &str = "internal";
    /// 请求了一个内核不认识的 `method`。
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    /// 还没有 `load_delivery`，或上一批已经被换码作废。
    pub const NO_DELIVERY: &str = "no_delivery";
    /// 拉清单失败（网络、404、清单本身不合法）。
    pub const DELIVERY_FETCH_FAILED: &str = "delivery_fetch_failed";
    /// 开工前检查没过（目标目录不可写 / 磁盘不足）。**引擎没有被启动。**
    pub const PREFLIGHT_FAILED: &str = "preflight_failed";
    /// 引擎还没起来（`enqueue` 之外的读方法会走到这里）。
    pub const ENGINE_NOT_STARTED: &str = "engine_not_started";
    /// 引擎启动失败（端口、二进制、RPC 不可达）。
    pub const ENGINE_START_FAILED: &str = "engine_start_failed";
    /// 引擎已被判定断开，且**这次重连也没成功**。
    pub const ENGINE_DISCONNECTED: &str = "engine_disconnected";
    /// 一次 RPC 调用失败，但重连探活是好的（瞬时故障，界面可以重试）。
    pub const ENGINE_RPC_FAILED: &str = "engine_rpc_failed";
    /// `list_dir` 的路径不在清单里。
    pub const PATH_NOT_FOUND: &str = "path_not_found";

    /// **完整**码表：上面 13 个字面量，一条不多一条不少。
    ///
    /// 它的存在是为了让"13"这个数字**可断言**：少了这条，删掉一个码不会有任何东西变红，
    /// 而壳会静默地落进"不认识的码"那条兜底分支——正是本项目最怕的静默失效。
    pub const ALL: [&str; 13] = [
        BAD_REQUEST,
        INVALID_PARAMS,
        PROTOCOL_MISMATCH,
        INTERNAL,
        UNKNOWN_METHOD,
        NO_DELIVERY,
        DELIVERY_FETCH_FAILED,
        PREFLIGHT_FAILED,
        ENGINE_NOT_STARTED,
        ENGINE_START_FAILED,
        ENGINE_DISCONNECTED,
        ENGINE_RPC_FAILED,
        PATH_NOT_FOUND,
    ];
}

/// 一条请求（`core/src/protocol.rs` 的 `Request`）。
///
/// 壳**发**它，内核收它。`params` 缺省是 `Value::Null`——内核的 `#[serde(default)]`
/// 让"缺键"与"显式 `null`"等价（钉它的是内核的 `request_params_is_optional`），
/// 所以壳对无参方法发 `"params":null` 是安全的，不必特判。
///
/// ⚠️ **`hello` 不是无参方法**：设计规格 §5.1 的原文是
/// `{"id":1,"method":"hello","params":{"protocol":1}}`，`params` 必须是对象且带 `protocol`
/// （缺了内核回 `invalid_params`）。别照"无参方法"那半句去发一个不带 `params` 的 `hello`
/// ——那握不上手，而且要到端到端才暴露。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// 错误体（`core/src/protocol.rs` 的 `ErrorBody`）。`code` 是结构化的（见 [`codes`]），
/// `message` 是给人看的一句话（壳**原文照登**，不加工）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

/// 一条响应（`core/src/protocol.rs` 的 `Response`）。
///
/// ⚠️ 内核侧靠 `skip_serializing_if` 保证 `result` 与 `error` **互斥**；壳这一侧
/// 只要**容忍**两种形状即可——两个字段都是 `Option`，`ok` 是那个承重的判别位。
/// （壳只**读**响应，不写；要造桩响应直接写结构体字面量即可，所以这里不派生
/// `Serialize`，也就不需要那两个 `skip_serializing_if` 属性。）
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<ErrorBody>,
}

/// `hello` 的入参（`core/src/protocol.rs` 的 `HelloParams`）：`{"protocol":1}`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: u32,
}

/// `hello` 的结果（`core/src/protocol.rs` 的 `HelloResult`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    /// `-k`（最小分片大小）的取值集合，契约 §2.3。
    ///
    /// ⚠️ **它随握手下发，所以壳这边**不得**有一份硬编码的副本**——设计规格 §2.3 的原话是
    /// "把集合交给壳"。那句话的后半截讲的是**呈现侧怎么用它**（本 crate 不含界面概念，
    /// 章程见 `lib.rs` 头部），这里只陈述它**是什么**：**内核认可的最小分片大小取值集合**。
    /// 抄第二遍就是漂移的开始（改内核不改壳，客户会选到内核拒收的值）。
    pub min_split_size_choices: Vec<String>,
}

// ---------------------------------------------------------------------------
// 缺字段的语义：**显式决定，不许让它默认发生**（控制者裁决 U）
// ---------------------------------------------------------------------------
//
// 判据按"内核是不是**始终**发这个键"分两类。两类都要在**字段上**看得出理由，
// 不许靠"反正加上 `default` 就不会炸"的滑坡——那会把漂移探测器整个关掉。
//
//   ① **内核始终发的键 ⇒ 非可选，缺失即解码错误（响亮失败）。**
//      内核自己的注释就是判据：`core/src/main.rs:1012` 与 `:1149` 写着"**始终发这个键**，
//      无值时是空串"，`node_json` 其余字段也都走 `unwrap_or*` 或直接取值——
//      线上**一定**有这些键。缺了只可能是"内核改名/删了字段"，**那就是漂移**，
//      必须失败。静默取默认值的后果是界面少一行、进度恒零，而**没有任何东西变红**
//      （本项目最怕的形态）。
//      这一类的范围：`Response` 的 `id`/`ok`、`ErrorBody` 的两个、
//      `HelloParams`/`HelloResult` 的全部、`TaskState`/`FileState`（枚举，不适用）、
//      **`TransferItem` 9 个字段里除 `path` 之外的 8 个**、**`FileNode` 10 个字段里
//      除 `source_mtime` 之外的 9 个**（这两个例外都归 ② 类，见下）、
//      `DirEntry::Dir` 的两个、`DeliveryInfo` 的 9 个、`Settings` 的 7 个、
//      `GlobalStat` 的 4 个、`AddedTask`/`RejectedTask`/`EnqueueResult` 的全部、
//      `TaskAction`（枚举字面量）。**测试逐条钉住**，见下面各条用例。
//      ⚠️ 小把戏：枚举**没法**用"缺字段"的方式漂移，但**变体名**能——
//      所以 `TaskState`/`FileState`/`TaskAction` 的线上字面量由专门的用例逐个钉。
//
//   ② **内核承认"可能不在"的键 ⇒ 字段类型是 `Option<T>`。**
//      判据不是"加上它更安全"，而是**有据可查**：
//      - `FileNode::source_mtime`：任务 4b 才加上的字段。macOS 的
//        `Protocol.swift:310` 对**同一个字段**写的是 `String?`，理由写在它旁边
//        ——"已交付的老清单里没有这个键，而一个非可选 `String` 会让**整条载荷解码失败**"，
//        并有专门的用例（`anOldPayloadWithoutTheKeyStillDecodesAndReadsNil`，
//        任务 13 会移植）。**内核与 `Protocol.swift` 在这一处一致**，所以两个方向都不冲突。
//      - `TransferItem::path`：`Protocol.swift:416` 同样是 `String?`，
//        注释点名"**没有映射时内核发 `null` 而不是省略键**"——键在、值是 `null`，
//        所以这一处要的是"能解 `null`"，不是"能缺键"（`Option` 两种都覆盖）。
//      - `Response::{result,error}`：`Protocol.swift:29-30` 的 `result`/`error` 都是 `?`。
//        它们的"可能不在"来自内核的 `skip_serializing_if`（`ok:true` 不带 `error`、
//        `ok:false` 不带 `result`），是**互斥省略**，与"老载荷"无关。
//      ⚠️ 这一类**共 4 处**，一处不多一处不少。代价是"改名"在这几个字段上探测不到，
//      所以它**只**用在这个名单上；要往名单里加人，先在这里写出理由。
//
//      ⚠️ **审这份名单要 grep 的是字段类型 `Option<`，不是属性 `#[serde(default)]`。**
//      判据：`grep -nE '^\s+pub [a-z_]+: Option<'` —— **正好 4 行**，一处不多一处不少。
//      四处里只有 `FileNode::source_mtime` 真写了那个属性；另外三处（`TransferItem::path`、
//      `Response::result`/`error`）靠 serde 对"缺键的 `Option`"天然取 `None`——
//      **行为完全等价**，但按**属性**数从来对不上：全文的属性今天只有 2 处
//      （`FileNode::source_mtime` 与 `Request::params`），而 `Request::params` **不在**
//      这份名单上（它属 ① 类的另一形态，理由在它自己的文档注释里）；把另外三处补上属性
//      则变成 5 处。两个数都不是 4。
//      （本段第一版把 ② 类写成 "`Option<T>` + `#[serde(default)]`"，暗示"grep 那个属性
//      就能审出例外名单"——**那句话不成立**，复审时被点名。这里**改措辞**而不是给那三处
//      补属性，理由写在 task-8 报告里：`Option<` 是唯一与这份名单一一对应的判据，
//      而属性数不管补不补都不等于 4。）
//
// **逐字段核对 `Protocol.swift` 的 optional 用法（裁决 U 的要求）**：
// 整份 Swift 镜像里只有 **4 处 `?`**（`Protocol.swift:29`、`:30`、`:310`、`:416`），
// 与上面那份名单**逐处对上**——没有一处多、没有一处少。**未发现内核与
// `Protocol.swift` 在 optionality 上的分歧**（若有，按裁决 T 第 3 条内核赢，
// 并写成一条发现——这一轮没有出现）。
// **改这里的 optional 之前，先回头看那 4 行。**

// ---------------------------------------------------------------------------
// 领域 / 线上类型
// ---------------------------------------------------------------------------

/// 客户端视角的任务状态（`core/src/engine/status.rs` 的 `TaskState`）。
///
/// ⚠️ `rename_all = "lowercase"` 是**承重的**：这些字符串会进状态文件，
/// 是**与 Go 版共用的持久化格式**（内核的注释原话）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// 还没开始传。
    Waiting,
    /// 正在传。
    Active,
    /// 传完了。
    Complete,
    /// 失败。
    Error,
    /// 被移除。
    Removed,
}

/// `transfer_list` 的一项（`core/src/protocol.rs` 的 `TransferItem`）。
///
/// ⚠️ **两个字段形状容易写错，以 `core/src/protocol.rs` 为准**：
///   - `conns` 是 `i32`（不是 `i64`）；
///   - `error_message` 是**非可选 `String`**、无错时是**空串**（不是 `Option`）
///     ——名字用 `error_message` 而不是 `error`，因为后者已经是响应级错误对象的键。
///     `path` 才是那个 `Option<String>`（没有 GID 映射时内核发 `null`）。
///
/// ⚠️ `raw_status` **不能被删掉，也不能由 `state` 反推**：`paused` 与 `waiting` 映射成
/// **同一个领域变体**（契约 §8 表的 `#14`），而设计规格 §8.4 要求界面把暂停的任务
/// 标注「已暂停」——域状态里这两者长得一模一样，壳拿不到这个区分就给不出那个标注。
/// （⚠️ 顺手改一个词：那句原文里的动词属于**界面概念**，本 crate 不含（章程见 `lib.rs` 头部）。
/// 判据与语义一字未改。范围外观察，先前的审查轮已记过同一形态。）
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TransferItem {
    pub gid: String,
    pub total: i64,
    pub completed: i64,
    pub speed: i64,
    pub conns: i32,
    /// 领域状态：四态合成的输入。
    pub state: TaskState,
    /// **aria2 的原始状态串，逐字**（`RawTask::status`）。
    pub raw_status: String,
    /// aria2 的 `errorMessage`，无错时为空串。
    pub error_message: String,
    /// `manifest` 相对路径，来自 GID 映射；没有映射的任务是 `None`。
    pub path: Option<String>,
}

/// 每个文件的四态（内核 `core/src/view.rs` 的 `State`，线上名见 `state_name`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    /// 待下载。
    Pending,
    /// 下载中。
    Downloading,
    /// 已完整。
    Complete,
    /// 失败。
    Failed,
}

/// 树 / `list_dir` 里的**文件叶节点**（内核 `node_json()` 与 `entries_json()`）。
///
/// ⚠️ 内核有**两条 JSON 出口**写同一份数据（`node_json` 与 `entries_json`），
/// 漏掉任何一条，字段都会**静默地**到不了壳——所以这个结构体要同时钉住两侧，
/// `entries_and_tree_nodes_carry_the_type_tag` 就是这么写的。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FileNode {
    pub name: String,
    /// 清单里的**原文**相对路径（全局约束 3：不得规范化），含空格与非 ASCII。
    pub path: String,
    /// CRC64 的十进制字符串；清单回读失败时是空串。
    pub crc64: String,
    pub size: i64,
    pub completed: i64,
    pub total: i64,
    pub speed: i64,
    pub state: FileState,
    /// 失败原因，无错时是空串。
    pub err: String,
    /// 源文件修改时间（内核原文，ISO 8601 带时区）。
    ///
    /// ⚠️ **必须可选**：已交付的老清单里没有这个键，而非可选的 `String` 会让
    /// **整条载荷解码失败**（macOS 侧记着的那次教训）。缺键 → `None`；
    /// 键在而值是空串 → `Some("")`——**那是两种输入，不是一件事**：
    /// 空串是"内核说这一条没有时间"，`None` 是"这份清单比这个字段还老"。
    /// 两者在界面上的落点相同（都是 `—`），但不要在这里合并：
    /// 合并之后就再也分不出"解码时把键丢了"与"内核真的没给值"。
    #[serde(default)]
    pub source_mtime: Option<String>,
}

/// `list_dir` 的一项（`core/src/main.rs` 的 `entries_json`）。
///
/// ⚠️ 目录项**没有 `path` 键**，只有子项数（`children_count`）——目录的路径由壳
/// 按父路径拼回来（任务 13 的 `dirRowsGetTheirPathRebuiltByTheShell`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DirEntry {
    /// 子目录。
    Dir { name: String, children_count: i64 },
    /// 文件叶节点。
    File(FileNode),
}

// ---------------------------------------------------------------------------
// `list_dir` / `get_tree` 的回执（控制者裁定 S：响应信封一律住在 protocol.rs）
// ---------------------------------------------------------------------------

/// `list_dir` 的回执（`Protocol.swift:336` 的 `ListDirResult`）。
///
/// ⚠️ `path` **空串 = 根**（内核把根目录的 `path` 回成 `""`，不是 `"/"`）——
///    壳不得把它规范化成 `"/"`：那是另一个路径，内核会回 `path_not_found`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ListDirResult {
    pub path: String,
    pub entries: Vec<DirEntry>,
}

/// `get_tree` 的 `flat[]` 的一项（`Protocol.swift:343`）。
///
/// ⚠️ `flat` **只列文件**（目录不在里面）——"勾选面是不是覆盖了整批"
///    那条判据（`BrowserSelection::all_files`）靠的就是这个事实。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FlatEntry {
    /// 清单里的**原文**相对路径（约束 3：不规范化、不转义）。
    pub path: String,
    pub name: String,
    pub size: i64,
    pub state: FileState,
}

/// `get_tree` 的总进度（`Protocol.swift:351`）。
///
/// ⚠️ 四个键都是内核**始终**发出的（`main.rs:1199-1204` 无条件写入）⇒ 按裁定 U
///    一律**非可选**：缺一个就是解码失败（响亮），不是静默取 0。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Progress {
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub speed: i64,
    /// 内核已经算好的整数百分比（**向零截断**，不是四舍五入，契约 §1.6）。
    pub percent: i32,
}

/// `get_tree` 的回执（`Protocol.swift:359` 的 `TreeResult`）。
///
/// ⚠️ 四个键同样由内核无条件发出（`main.rs:1195-1205` 的 `json!` 字面量）⇒ 非可选。
///    `default_selected` 是"所有非 complete 的文件"（`view::default_selected`），
///    **不是**当前层的全部条目 —— 那是"点开就能直接点下载"的来源。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TreeResult {
    pub tree: TreeNode,
    /// 树序的扁平列表（只列文件，供壳免递归处理大目录）。
    pub flat: Vec<FlatEntry>,
    /// 给勾选框的默认选中面（待下载 / 失败的那些路径）。
    pub default_selected: Vec<String>,
    pub progress: Progress,
}

/// 交付清单的树（内核 `core/src/main.rs` 的 `node_json`）。
///
/// 判别键是 `"type"`（`"dir"`/`"file"`），**不是 `is_dir` 之类的布尔**。
///
/// ⚠️ **零文件时内核发的树是空对象 `{}`**，不是 `{"type":"dir",…}`
/// （`node_json` 末段：`root.is_empty()` → 空 `Map`）。所以 `Empty` 必须是一个分支，
/// 少了它，零文件的交付批次会让壳解码失败——macOS 侧是**真机抓取**才发现这一条的。
///
/// ⚠️ 非空的根是 `{"type":"dir","name":"","children":{…}}`：根节点**名为空串**，
/// 文件名在 `children` 的键上。树是递归结构，所以这个 `impl Deserialize` 是手写的
/// （`{}` → `Empty` 这一支没法用 `#[serde(tag = …)]` 派生出来）。
#[derive(Debug, Clone, PartialEq)]
pub enum TreeNode {
    /// 空树（清单零文件）。
    Empty,
    /// 目录：`name` 是这一层的名字，`children` 是"名字 → 节点"。
    Dir {
        name: String,
        children: BTreeMap<String, TreeNode>,
    },
    /// 文件叶节点。
    File(FileNode),
}

impl TreeNode {
    /// 由已经解析出来的 JSON 值构造。
    ///
    /// 错误消息里**不带原值**：一条几 MiB 的畸形树不该变成一条几 MiB 的错误消息
    /// （内核在 `read_request` 上记过同一条账）。
    fn from_json(v: Value) -> Result<Self, String> {
        let Value::Object(mut map) = v else {
            return Err("树节点必须是一个 JSON 对象".to_string());
        };
        if map.is_empty() {
            return Ok(TreeNode::Empty);
        }
        let kind = map
            .get("type")
            .and_then(Value::as_str)
            .ok_or("树节点缺少 \"type\" 键（应为 \"dir\"/\"file\"）")?
            .to_string();
        match kind.as_str() {
            "dir" => {
                let name = map
                    .remove("name")
                    .and_then(|n| match n {
                        Value::String(s) => Some(s),
                        _ => None,
                    })
                    .ok_or("目录节点缺少字符串 \"name\" 键")?;
                let Some(Value::Object(children)) = map.remove("children") else {
                    return Err("目录节点缺少对象 \"children\" 键".to_string());
                };
                let children: BTreeMap<String, TreeNode> = children
                    .into_iter()
                    .map(|(k, v)| TreeNode::from_json(v).map(|t| (k, t)))
                    .collect::<Result<_, _>>()?;
                Ok(TreeNode::Dir { name, children })
            }
            "file" => serde_json::from_value(Value::Object(map))
                .map(TreeNode::File)
                .map_err(|e| format!("文件节点的形状不对: {e}")),
            other => Err(format!(
                "未知的节点类型 {other:?}（内核只发 \"dir\"/\"file\"）"
            )),
        }
    }
}

impl<'de> Deserialize<'de> for TreeNode {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = Value::deserialize(d)?;
        TreeNode::from_json(v).map_err(serde::de::Error::custom)
    }
}

/// `load_delivery` 的回执（`core/src/main.rs` 的 `op_load_delivery`）。
///
/// ⚠️ `created_at` / `expires_at` 是 **ISO 8601 带时区的字符串，可能是空串**——
/// 不要当日期解。`expired` 让壳能在下载一个个 404 之前就把"已过期"显示出来。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeliveryInfo {
    /// 交付码。**可能是非 ASCII**（`C24-8_×_25WS024`），逐字原样。
    pub code: String,
    /// 交付页地址（`base_url/code/index.html`）——"去哪儿看交付页"就是它。
    pub page_url: String,
    pub base_url: String,
    pub created_at: String,
    pub expires_at: String,
    /// 已过期要让客户看见，而不是等下载一个文件一个文件地 404。
    pub expired: bool,
    pub total_files: i64,
    pub total_bytes: i64,
    pub tree: TreeNode,
}

/// 客户可调的七项（`core/src/settings.rs` 的 `Settings`）。
///
/// ⚠️ **双向**：`get_settings` 收进来、`set_settings` 发回去，键名必须逐字相同
/// ——否则"改了设置却静默无效"。字段名与 Go 的 `json:` 标签逐字一致
/// （`min_split_size`/`limit_mbps` 等），所以无需任何 `rename`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// -j，1–64。
    pub parallel: i32,
    /// -x，1–16。
    pub connections: i32,
    /// -s，1–16。
    pub splits: i32,
    /// -k，1M–100M。**是字符串**（`"20M"`），不是数字——写错会让 aria2 直接启动失败。
    pub min_split_size: String,
    /// 0 = 不限速。
    pub limit_mbps: i64,
    /// 1–100。
    pub max_tries: i32,
    /// 0–60 秒。
    pub retry_wait: i32,
}

/// `transfer_list` 的 `global`（`core/src/engine/rpc.rs` 的 `GlobalStat`）。
///
/// ⚠️ 内核那个结构体**没有** `rename_all`，所以线上就是 snake_case
/// （macOS 壳靠 `.convertFromSnakeCase` 消化它）；Rust 侧字段名天然同形。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GlobalStat {
    pub download_speed: i64,
    pub num_active: i64,
    pub num_waiting: i64,
    pub num_stopped: i64,
}

/// `transfer_list` 的回执（`core/src/main.rs` 的 `op_transfer_list`；
/// 上游 `Protocol.swift:426` 的 `TransferListResult`）。
///
/// ⚠️ 两个键都是内核**每次都发**的（`items` 可能是空数组），所以按裁决 U 它们非可选。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TransferListResult {
    pub items: Vec<TransferItem>,
    pub global: GlobalStat,
}

/// `enqueue` 成功加进去的一个任务（`core/src/main.rs` 的 `op_enqueue`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AddedTask {
    pub gid: String,
    pub path: String,
}

/// `enqueue` 被拒的一个文件（`core/src/main.rs` 的 `op_enqueue`）。
///
/// `reason` 是**内核原文**（路径守卫生效、或 aria2 的报错），
/// 界面把它照登给客户——这是"哪些文件没进去、为什么"的唯一来源。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RejectedTask {
    pub path: String,
    pub reason: String,
}

/// `enqueue` 的回执（`core/src/main.rs` 的 `op_enqueue`）。
///
/// ⚠️ 内核在 `added` 为空**且** `rejected` 非空时改回一条 `invalid_params` 错误、
/// 不返回本结构体——所以壳不能假设"有回执就一定有成功的项"。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EnqueueResult {
    pub added: Vec<AddedTask>,
    pub rejected: Vec<RejectedTask>,
}

/// `task_action` 的 `action` 参数（`core/src/main.rs` 的 `op_task_action`）。
///
/// ⚠️ 这五个是**线上值**：壳把它们填进 `params.action` 发给内核，而内核用**字面量**匹配
/// （`"pause" | "unpause" | "retry" | "remove" | "clear_finished"`），
/// 不认识的会回 `invalid_params` 并列出可用的那几个。
/// 出处：`macos/Sources/BenagenCoreKit/Protocol.swift:168-170` 与内核的匹配臂，两边逐字一致。
///
/// ⚠️ **没有 `reveal`**：设计规格 §5.2 明确"在资源管理器/访达中显示"是**壳**的事，
/// 不进协议。内核那边还有个次序细节：它**先判动作名、再碰引擎**，
/// 所以发一个 `reveal` 得到的结论是"这个动作不存在"，而不是"引擎没起来"——
/// 壳要靠这条明确结论决定**某个动作名在壳这边还留不留**——它陈述的是**内核不认识这个名字**
/// 这条协议事实（本 crate 不含界面概念，章程见 `lib.rs` 头部）。
/// **不认识的动作在壳里就要拦住，别发出去换一条错误。**
///
/// `Ord` 不是装饰：任务 15 的 `TransferItemRow.available_actions` 是 `BTreeSet<TaskAction>`
/// （`task-15-brief.md:33`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    /// 暂停。
    Pause,
    /// 继续。
    Unpause,
    /// 重试（失败的任务）。
    Retry,
    /// 移除。
    Remove,
    /// 清空已完成。
    ClearFinished,
}

// ---------------------------------------------------------------------------
// 壳自己持有的三个枚举（**无线上形态**，也不会被内核看到）
//
// ⚠️ **这个数是数出来的，不是估的**：`LoadState` / `EngineState` / `VerifyClass`。
//    日后加第四个，请把这行的数一起改 —— 少算一个，下一个读到这里的人就会以为
//    "壳侧状态只有那么几个"，而漏掉的那个恰好是他在找的。
// ---------------------------------------------------------------------------

/// 交付清单的**加载状态**（上游 `AppModel.LoadState`，`AppModel.swift:30-36`）。
///
/// ⚠️ **它为什么在这里**（裁决 T 的记账，任务 11）：上游的第二个
///    `DeliverySummary.of(_ state: AppModel.LoadState)` 收的正是这个类型，而
///    `AppModel` 本波次还没有移植过来 —— 任务 11 的简报 API 块里**根本没有它**
///    （只写了收 `DeliveryInfo` 的那一个 `of`），可 `summaryIsAbsentUntilAManifestIsLoaded`
///    这条测试必须用 `.idle` / `.loading` / `.failed(...)` / `.loaded(...)` 四个变体。
///    按**源是权威**取源里的成员（四个，一个不多一个不少），落到 `protocol.rs`：
///    与它的同族 [`EngineState`] 同处一地（**控制者裁决 C**："只定义一份，不另立"）。
///    `AppModel.swift` 真正移植过来的那一天，这里就是它的家，不必再搬一次。
///
/// ⚠️ 变体名按 Rust 惯例写成 PascalCase，其余（载荷类型与顺序）逐字照源。
/// `failed` 装的是**失败原文**：内核怎么说就怎么留，壳不加工（约束 3）。
#[derive(Debug, Clone, PartialEq)]
pub enum LoadState {
    /// 还没加载过任何清单。
    Idle,
    /// 正在加载（清单还没到）。
    Loading,
    /// 加载成功：拿到内核的清单。
    Loaded(DeliveryInfo),
    /// 加载失败，附带原文 —— **内核怎么说就怎么写**，壳不加工。
    Failed(String),
}

/// 引擎四态（控制者裁决 C：**定义在这里**，不另立模块）。
///
/// `presentation::engine_status`（任务 12）以 `use crate::protocol::EngineState` 消费它
/// ——**两处各定义一份是必须避免的**（同 [`crate::protocol::VerifyClass`] 的理由）。
///
/// ⚠️ `Unavailable` 里的字符串是**内核原文照登**（约束 3：壳不加工）。
/// 折行、加前缀、判"是不是握手超时"都是**呈现层**的事，不在这里做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// 还没握上手（壳正在启动内核）。
    ///
    /// ⚠️ **变体名与上游不是逐字相同**：macOS 侧 `AppModel.EngineState` 的第一个变体叫
    /// `unknown`（`macos/Sources/BenagenCoreKit/AppModel.swift:51`）。这里是**同义改名**
    /// ——`unknown` 在 Rust 侧读起来像"未知/不认识"，而它实际的含义是"**还在连接**"，
    /// 即上游 `EngineStatusPresentation.swift:62-63` 把 `.unknown` 映成的那句
    /// 「正在连接内核…」（本波次的任务 12 在 `engine_status.rs` 里逐字照抄了同一个字面量）。
    /// 两处讲的是同一个状态，**不是分歧**；改的是名字，语义一个字节都没动。
    Connecting,
    /// 内核在，但下载引擎还没起来（`engine_not_started`）。
    NotStarted,
    /// 引擎在跑。
    Running,
    /// 不可用，附带**内核给的原因**（或壳探测到的原因，例如握手超时）。
    Unavailable(String),
}

/// 校验的六个归类（行为契约 §8；macOS 侧 `VerifyClass`）。
///
/// ⚠️ **本枚举只有变体 + [`VerifyClass::wire_key`]，别的方法一个也没有**：
/// `is_failure`/`label`/`color`/`icon_name`/`note` 是**呈现层**的事（要用
/// `presentation::RowColor`），由任务 14 在 `presentation/verify_summary.rs` 里 `impl`
/// ——同一个 crate，可以直接给这里定义的类型写固有 `impl`。
/// `wire_key()` 是唯一的例外，因为它是**协议层**的东西（变体 ↔ 线上键），不是呈现。
/// **变体只定义一份**，理由同 [`EngineState`]（控制者裁决 C）。
///
/// ⚠️ 它与内核的 `core/src/verify.rs::CheckResult` **不是**同一个东西：那是六个路径数组，
/// 这是"一条路径被归成哪一类"。六个分支必须都在，否则界面会把
/// `unverifiable`/`unreadable` 静默并进别的归类（"不得静默少交"）。
///
/// ⚠️ **它对应 `verify_status` 结果的六个键，而这六个键的名字与本枚举的变体名不一样**
/// （macOS 侧记着这一条，`Protocol.swift:156` 的原话是"**是键名，不是值**"）：
///
/// | 本枚举 | `verify_status` 结果里的键（`core/src/main.rs:1601` 的 `op_verify_status`） |
/// | --- | --- |
/// | `Passed` | `ok` |
/// | `Mismatched` | `bad` |
/// | `Missing` | `missing` |
/// | `SizeMismatch` | `size_mismatch` |
/// | `Unverifiable` | `unverifiable` |
/// | `Unreadable` | `unreadable` |
///
/// 变体名走的是任务 14 的呈现面（`Passed`/`Mismatched`，不是 `Ok`/`Bad`——`Ok` 会与
/// `Result` 的 `Ok` 撞），键名走协议。**本枚举刻意不派生 serde**：
/// 线上跑的是那六个路径数组（[`VerifyStatus`] 信封，已在本模块；任务 14 加进来的），
/// 而 `VerifyClass` 是壳自己的归类。那个信封**按上表的键名去解**，
/// **不要**按变体名去解 `"passed"`——那会解出六个空桶，而呈现层会报"全部通过"。
///
/// ⚠️ 上表**不是**注释，是 [`VerifyClass::wire_key`] 的规格：那张表与它的实现
/// 由 `verify_class_wire_keys_match_the_verify_status_receipt` 逐条钉住
/// （控制者裁决 V：注释不是守卫）。**改映射就改方法**，别只改这张表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyClass {
    /// 校验通过。
    Passed,
    /// CRC64 不符。
    Mismatched,
    /// 本地不存在。
    Missing,
    /// 大小不符。
    SizeMismatch,
    /// 清单与 HEAD 都没给出 crc64（**不是失败**，重下也没有可比对象）。
    Unverifiable,
    /// 能 stat 到但读不了（权限/IO 错误）。
    Unreadable,
}

impl VerifyClass {
    /// 本归类在 `verify_status` 回执里的**线上键名**（变体 → 键）。
    ///
    /// 判据：`core/src/main.rs:1601-1611` 的 `op_verify_status` 那个 `json!` 的六个键，
    /// 一条不多一条不少（外加一个不属于本枚举的 `all_good`）。
    ///
    /// ⚠️ **为什么这层翻译必须是代码**（控制者裁决 V）：`Passed`→`ok`、`Mismatched`→`bad`
    /// 这两条**连"变体名 snake_case 化"都对不上**。日后写 `VerifyStatus` 信封的人若按
    /// 变体名去解 `"passed"`，会得到空桶，而界面会报**"全部通过"**——一次数据全坏的交付
    /// 被静默报成齐全。一句"小心"的注释拦不住这件事（注释**不是**守卫），
    /// 可断言的方法 + 六条映射的测试才拦得住。
    ///
    /// ⚠️ **别把它换成 `#[serde(rename_all = "snake_case")]`**：六条里有四条恰好与变体名的
    /// snake_case 同形（`missing`/`size_mismatch`/`unverifiable`/`unreadable`），
    /// **同形纯属巧合**，那个属性会把 `ok`/`bad` 两条写错、而另外四条照样绿。
    ///
    /// 判据面：[`VerifyStatus`] 信封（本模块）那六个字段名，与
    /// `presentation::verify_summary` 的测试夹具里**用字面量**写的那六个键，
    /// 必须是同一组字符串（夹具走解码这条路，而对不上就是解码失败 —— 响亮，不是空桶）。
    /// 呈现层（`presentation::verify_summary`）消费的是**变体**，不是键名——
    /// 所以本方法不抢它的 API 面。
    pub fn wire_key(self) -> &'static str {
        match self {
            VerifyClass::Passed => "ok",
            VerifyClass::Mismatched => "bad",
            VerifyClass::Missing => "missing",
            VerifyClass::SizeMismatch => "size_mismatch",
            VerifyClass::Unverifiable => "unverifiable",
            VerifyClass::Unreadable => "unreadable",
        }
    }
}

/// **`VerifyClass` 的线上名字只有一处：[`VerifyClass::wire_key`]。**
///
/// ⚠️ **手写而不是 `#[derive(Serialize)]`**（这是 2026-09-19 第六版裁定里
///    "界面值类型上的 `Serialize`"那一条的**唯一一处例外**，理由如下）：
///    本枚举的线上键名与变体名**对不上**——`Passed`→`ok`、`Mismatched`→`bad`，
///    而另外四条恰好同形（`missing`/`size_mismatch`/`unverifiable`/`unreadable`）。
///    `#[derive(Serialize)]` 会写出 `"Passed"`/`"Mismatched"`，`rename_all = "snake_case"`
///    会写出 `"passed"`/`"mismatched"` —— **两种都是第二份身份**，
///    而它们与 [`VerifyClass::wire_key`] 漂移时**不会有任何东西变红**
///    （`presentation::verify_summary` 的 `VerifyClassRow` 已经在用 `wire_key`
///    当身份，见它的 `id()`）。
///    ⇒ 这个 impl 只做一件事：**把那一处转出去**。改名字仍然只改 `wire_key` 一处。
///
/// ⚠️ **它为什么在这一层**：`Serialize` 是外部 trait、`VerifyClass` 是本 crate 的类型
///    ⇒ impl 只能写在本 crate 里。而界面值那批 `Serialize` 之所以落在
///    `presentation/*.rs`，是因为**类型本身**在那里；本枚举的家在 `protocol.rs`
///    （控制者裁决 C：四态/归类定义在协议镜像这一侧），所以它跟着家在哪儿。
impl serde::Serialize for VerifyClass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire_key())
    }
}

/// `verify_status` 的回执（`core/src/main.rs` 的 `op_verify_status`；
/// 上游 `Protocol.swift:432` 的 `VerifyStatus`）。六个分桶都是**路径数组**。
///
/// ⚠️ **六个字段名就是内核线上的六个键**（`ok` / `bad` / `missing` / `size_mismatch` /
///    `unverifiable` / `unreadable`），**不是** [`VerifyClass`] 的变体名 ——
///    `Protocol.swift:156` 那句"**是键名，不是值**"说的就是这件事，
///    而"变体 → 键名"的翻译在本 crate 里**只有一份**：[`VerifyClass::wire_key`]。
///
/// ⚠️ **为什么字段名必须逐字取线上键**（控制者裁决 V）：按变体名去解 `"passed"`
///    会得到**六个空桶**，而 `presentation::verify_summary` 会把六个空桶读成
///    「全部通过」—— 一次数据全坏的交付被静默报成齐全。派生 `Deserialize` 在这里
///    帮了个大忙：**六个字段一个都不能缺**，键名写错（把 `ok` 写成 `passed`）
///    是一次**响亮的解码失败**，不是六个空桶。
///    钉住它的有两条：`presentation::verify_summary` 的测试夹具**用字面量**写那六个键
///    （**凡经 `VerifyStatus` 夹具的那些测试**——20 条里**13 条**走这条路 ⇒ 键名写错就是
///    那 13 条一起红；另外 7 条分别走 `TreeFx`／只解 `TransferItem`／不用夹具，
///    它们本来就碰不到这个信封），以及本模块的
///    `verify_class_wire_keys_match_the_verify_status_receipt`（钉住 `wire_key()` 那六条映射）。
///    两条合起来才把"信封的键 = 那张表"变成事实。
///    ⚠️ **订正**：这句原先写的是"那 20 条测试全走解码这条路"，**比事实强**。
///    逐条点算的实际分解是 **13 / 3 / 1 / 3**（合计 20）：
///      · **13** 条经 `VerifyStatus` 信封（1–7、10–15）—— 键名写错就是这 13 条一起红
///        （变异实测 `157 passed; 13 failed`，与 13 吻合）；
///      · **3** 条走 `TreeFx`（17–19）；
///      · **1** 条只解 `TransferItem`（16）；
///      · **3** 条不用任何夹具（8、9、20）。
///    —— 与本任务顺手替 `task-11-report.md:110` 订正的是**同一个病**。
///    （⚠️ 本条第一版把后三档写成"3 + 1 + **2**" —— **加不到 7**，是一处算术不闭合，
///     复审点名后按点算结果改成 3 + 1 + 3。）
///
/// ⚠️ 呈现层（`presentation::verify_summary`）消费的是**变体**（`Passed`/`Mismatched`/…），
///    经 [`VerifyStatus::paths`] 那一步 —— 那是"变体 → 桶"，与键名无关。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VerifyStatus {
    /// `ok` 桶（[`VerifyClass::Passed`]）。
    pub ok: Vec<String>,
    /// `bad` 桶（[`VerifyClass::Mismatched`]）。
    pub bad: Vec<String>,
    /// `missing` 桶（[`VerifyClass::Missing`]）。
    pub missing: Vec<String>,
    /// `size_mismatch` 桶（[`VerifyClass::SizeMismatch`]）。
    pub size_mismatch: Vec<String>,
    /// `unverifiable` 桶（[`VerifyClass::Unverifiable`]）。
    pub unverifiable: Vec<String>,
    /// `unreadable` 桶（[`VerifyClass::Unreadable`]）。
    pub unreadable: Vec<String>,
    /// 内核的判定，**原值**：`CheckResult::all_good()` 的口径 ——
    /// **不含 `unverifiable`**（`core/src/verify.rs`）。
    pub all_good: bool,
}

/// `get_settings` / `set_settings` 的**回执**（`core/src/main.rs` 的
/// `op_get_settings` / `op_set_settings`）。
///
/// ```json
/// {"settings": { …七项… }, "last_code": "C24-8"}
/// ```
///
/// ⚠️ **两个键都是内核每次都发的**（两个 op 都写死 `json!({"settings":…,"last_code":…})`，
///    `last_code` 无值时是**空串**、不是缺键）⇒ 按裁决 U 它们**非可选**：缺了只可能是
///    内核改名/删字段，那就是一次必须响亮的漂移，不许静默取默认值。
///    钉住它的是下面 `the_settings_receipt_carries_both_keys`。
///
/// ⚠️ **它两个方向都用**（这也是它只定义一份的理由）：
///    `get_settings` 读它、`set_settings` 也回它（后者回的是**归一化之后**的那一份 ——
///    `-k` 会被内核 `check_min_split_size` 归一成规范串），所以"设置窗口保存完显示什么"
///    与"打开时显示什么"是同一个形状，不必各定义一份。
///
/// ⚠️ **`settings` 用的是 [`Settings`] 本身**（不是另抄一份七个 `i32`/`String`）：
///    那个类型的字段名**就是**线上键名，抄第二遍就是第二个真相源。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SettingsResult {
    /// 内核手里**当前生效**的那一份（`set_settings` 的响应里是归一化之后的那一份）。
    pub settings: Settings,
    /// 内核记着的上一次成功加载的交付码。**空串 = 还没有过**（它不是缺键）。
    pub last_code: String,
}

impl VerifyStatus {
    /// 某一类的路径，**内核原文逐字**（全局约束 3：不取最后一段、不规范化、不转义）。
    ///
    /// 判据是"**变体 → 桶**"这一层。键名那一层在**解码时**就结清了
    /// （字段名就是线上键），所以这里看不到一个线上键字符串。
    pub fn paths(&self, class: VerifyClass) -> &[String] {
        match class {
            VerifyClass::Passed => &self.ok,
            VerifyClass::Mismatched => &self.bad,
            VerifyClass::Missing => &self.missing,
            VerifyClass::SizeMismatch => &self.size_mismatch,
            VerifyClass::Unverifiable => &self.unverifiable,
            VerifyClass::Unreadable => &self.unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // 信封（brief 点名的两条在这里）
    // -----------------------------------------------------------------------

    /// 任务 7 简报点名的第一条。
    ///
    /// 守：内核的 err 响应解出来必须 `ok == false`、`result == None`、`error` 带得出
    /// `code` 与 `message`。**断言的是 `code` 的字面量**——契约 §5.1 明写
    /// 「壳不得靠字符串匹配判断错误类型」，壳分支靠的正是这个值。
    /// 反方向一起测：`ok:true` 必须解出 `result` 且没有 `error`，免得把两个分支写反。
    #[test]
    fn response_with_error_decodes_into_ok_false() {
        let line = r#"{"id":3,"ok":false,"error":{"code":"invalid_params","message":"分片数必须在 1–16 之间"}}"#;
        let r: Response = serde_json::from_str(line).expect("内核的 err 响应必须能解");
        assert_eq!(r.id, 3, "id 没读进来");
        assert!(!r.ok, "ok:false 没读进来");
        assert_eq!(r.result, None, "err 响应里不该有 result");
        let e = r.error.expect("err 响应必须带 error");
        assert_eq!(e.code, codes::INVALID_PARAMS, "错误码没读进来");
        assert_eq!(e.code, "invalid_params", "壳按这个字面量分支（契约 §5.1）");
        assert_eq!(e.message, "分片数必须在 1–16 之间", "message 不逐字");

        let line = r#"{"id":7,"ok":true,"result":{"entries":["a.txt"]}}"#;
        let r: Response = serde_json::from_str(line).expect("内核的 ok 响应必须能解");
        assert!(r.ok, "ok:true 没读进来");
        assert_eq!(r.error, None, "ok 响应里不该有 error");
        assert_eq!(
            r.result.expect("ok 响应必须带 result")["entries"],
            json!(["a.txt"]),
            "result 没原样读进来"
        );
    }

    /// 任务 7 简报点名的第二条。**方向很重要**：
    /// **壳要容忍内核新增字段**（否则内核一升级壳就解不动）；反方向由内核负责。
    ///
    /// 守：信封、以及每一个聚合类型（`TransferItem`/`DeliveryInfo`/`TreeNode`/`DirEntry`/
    /// `Settings`/`GlobalStat`/`EnqueueResult`）上多出来的未知键都**不得**让解码失败，
    /// 且已知字段必须原样活下来。判别力：给任意一个结构体加上
    /// `#[serde(deny_unknown_fields)]`，这一条立刻红。
    #[test]
    fn unknown_fields_are_tolerated_so_the_kernel_can_add_fields() {
        let line = r#"{"id":1,"ok":true,"trace_id":"t-1","result":{"protocol":1,"min_split_size_choices":["1M"],"future_field":123}}"#;
        let r: Response = serde_json::from_str(line).expect("信封上多一个键不该让整条响应解不动");
        assert_eq!(r.id, 1);
        let h: HelloResult = serde_json::from_value(r.result.expect("带 result")).expect("多余键不该致命");
        assert_eq!(h.protocol, PROTOCOL_VERSION);
        assert_eq!(h.min_split_size_choices, vec!["1M".to_string()]);

        // 传输列表项：最常见的"内核加字段"落点
        let item: TransferItem = serde_json::from_str(
            r#"{"gid":"g1","total":65536,"completed":32768,"speed":1024,"conns":1,
                "state":"active","raw_status":"active","error_message":"","path":null,
                "eta_seconds":12,"piece_length":16384}"#,
        )
        .expect("TransferItem 上多一个键不该致命");
        assert_eq!(item.gid, "g1");

        // 树节点：两条出口（`node_json` / `entries_json`）都可能被加字段
        let node: TreeNode = serde_json::from_str(
            r#"{"type":"dir","name":"sub","children":{},"future":true}"#,
        )
        .expect("TreeNode 上多一个键不该致命");
        match node {
            TreeNode::Dir { name, children } => {
                assert_eq!(name, "sub");
                assert!(children.is_empty());
            }
            other => panic!("应解成目录，实际 {other:?}"),
        }
        let entry: DirEntry = serde_json::from_str(
            r#"{"type":"dir","name":"sub","children_count":3,"future":true}"#,
        )
        .expect("DirEntry 上多一个键不该致命");
        assert!(matches!(entry, DirEntry::Dir { .. }));

        let s: Settings = serde_json::from_str(
            r#"{"parallel":8,"connections":16,"splits":16,"min_split_size":"20M",
                "limit_mbps":0,"max_tries":3,"retry_wait":1,"future_field":"x"}"#,
        )
        .expect("Settings 上多一个键不该致命");
        assert_eq!(s.parallel, 8);

        let g: GlobalStat = serde_json::from_str(
            r#"{"download_speed":1,"num_active":2,"num_waiting":3,"num_stopped":4,"future":5}"#,
        )
        .expect("GlobalStat 上多一个键不该致命");
        assert_eq!(g.num_active, 2);
    }

    /// 协议常量。
    ///
    /// 守：`PROTOCOL_VERSION` 与 `UNCORRELATED_ID` 的字面量（它们是**跨进程约定**，
    /// 不是内部常量）+ 壳发请求的形状（设计规格 §5.1 的示例）。
    /// JSON Lines 要求一行一条，所以文本里不得有换行符。
    ///
    /// ⚠️ `params` 是 `Value::Null` 时这里发的是 `"params":null`——内核**显式接受**
    /// 这种写法（`core/src/protocol.rs`：缺键与显式 `null` 等价，
    /// 钉它的是 `request_params_is_optional`）。所以壳不必为无参方法特判。
    #[test]
    fn shell_emits_the_requests_in_the_documented_shape() {
        assert_eq!(PROTOCOL_VERSION, 1, "协议版本是跨进程约定，不是内部常量");
        assert_eq!(UNCORRELATED_ID, 0, "保留 id 必须是 0（壳从 1 起分配）");

        let line = serde_json::to_string(&Request {
            id: 1,
            method: "hello".to_string(),
            params: json!({"protocol": PROTOCOL_VERSION}),
        })
        .expect("序列化不该失败");
        assert_eq!(
            line, r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
            "壳发出去的握手请求必须就是设计规格 §5.1 的那一行"
        );
        assert!(!line.contains('\n'), "JSON Lines：一行一条，实际 {line:?}");

        let line = serde_json::to_string(&Request {
            id: 2,
            method: "shutdown".to_string(),
            params: Value::Null,
        })
        .expect("序列化不该失败");
        assert_eq!(
            line, r#"{"id":2,"method":"shutdown","params":null}"#,
            "无参方法发 params:null（内核的 `#[serde(default)]` 接受它）"
        );
    }

    /// 13 个错误码。
    ///
    /// 守：**每一个码的字面量**逐字钉住（壳按这些值分支，措辞会变、码不会），
    /// 且清单里**恰好 13 条**、两两不重。
    /// 少了这条，删/改一个码不会有任何东西变红——而壳会静默地落进 `_ =>` 分支。
    ///
    /// 出处：`bad_request`/`invalid_params`/`protocol_mismatch`/`internal` 在
    /// `core/src/protocol.rs` 的 `codes`；其余九个在 `core/src/main.rs` 的 `kcodes`
    /// （内核那边分两个模块，壳这边合成一份**完整清单**——内核自己的注释就是这么记的：
    /// "壳侧的完整码表 = `protocol::codes` ∪ `kcodes`"）。
    #[test]
    fn all_thirteen_error_codes_are_mirrored_verbatim() {
        let expected: [(&str, &str); 13] = [
            (codes::BAD_REQUEST, "bad_request"),
            (codes::INVALID_PARAMS, "invalid_params"),
            (codes::PROTOCOL_MISMATCH, "protocol_mismatch"),
            (codes::INTERNAL, "internal"),
            (codes::UNKNOWN_METHOD, "unknown_method"),
            (codes::NO_DELIVERY, "no_delivery"),
            (codes::DELIVERY_FETCH_FAILED, "delivery_fetch_failed"),
            (codes::PREFLIGHT_FAILED, "preflight_failed"),
            (codes::ENGINE_NOT_STARTED, "engine_not_started"),
            (codes::ENGINE_START_FAILED, "engine_start_failed"),
            (codes::ENGINE_DISCONNECTED, "engine_disconnected"),
            (codes::ENGINE_RPC_FAILED, "engine_rpc_failed"),
            (codes::PATH_NOT_FOUND, "path_not_found"),
        ];
        for (actual, want) in expected {
            assert_eq!(actual, want, "错误码字面量被改了");
        }
        assert_eq!(codes::ALL.len(), 13, "完整码表就是这 13 条");
        let mut sorted: Vec<&str> = codes::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 13, "码表里有重复项");
    }

    // -----------------------------------------------------------------------
    // 领域 / 线上类型：逐字段搬运
    // -----------------------------------------------------------------------

    /// `TransferItem` —— 简报里"形状"写错过一次的那个类型。
    ///
    /// 守（这两条是**故意设的陷阱**，形状以 `core/src/protocol.rs` 为准）：
    ///   - `conns` 是 **`i32`**（不是 `i64`）；
    ///   - `error_message` 是**非可选 `String`**，无错时是**空串**（不是 `Option`）；
    ///   - `path` 才是那个 `Option<String>`（没有 GID 映射时内核发 `null`）；
    ///   - `raw_status` 逐字透传（aria2 的原文），`state` 是领域四态。
    ///
    /// 夹具取的是**真实线上形态**（`core/src/main.rs` 的 `op_transfer_list` +
    /// `core/src/protocol.rs` 的 `transfer_item_carries_the_raw_aria2_status`）：
    /// `paused` 与 `waiting` 的领域状态相同、只有 `raw_status` 不同。
    #[test]
    fn transfer_item_mirrors_the_kernel_field_names_exactly() {
        let paused: TransferItem = serde_json::from_str(
            r#"{"gid":"g-paused","total":65536,"completed":32768,"speed":1024,"conns":1,
                "state":"waiting","raw_status":"paused","error_message":"连接超时","path":"a/b.txt"}"#,
        )
        .expect("内核发的 transfer_list 项必须能解");
        let queued: TransferItem = serde_json::from_str(
            r#"{"gid":"g-queued","total":1,"completed":0,"speed":0,"conns":2,
                "state":"waiting","raw_status":"waiting","error_message":"","path":null}"#,
        )
        .expect("内核发的 transfer_list 项必须能解");

        // 类型断言（编译期就是证据）：写法照 `core/src/protocol.rs` 逐字
        let conns: i32 = paused.conns;
        let err: &str = &paused.error_message;
        assert_eq!(conns, 1);
        assert_eq!(err, "连接超时");
        assert_eq!(queued.error_message, "", "无错时是空串，不是 None");

        assert_eq!(paused.state, TaskState::Waiting);
        assert_eq!(
            queued.state, paused.state,
            "契约 §8 的 #14：paused 与 waiting 映到同一个领域状态"
        );
        assert_eq!(paused.raw_status, "paused", "raw_status 必须逐字透传");
        assert_eq!(queued.raw_status, "waiting");
        assert_ne!(paused, queued, "暂停与排队必须可区分（界面要标注「已暂停」）");
        assert_eq!(paused.path.as_deref(), Some("a/b.txt"));
        assert_eq!(queued.path, None, "没有 GID 映射时内核发 null");
        assert_eq!(paused.total, 65536);
        assert_eq!(paused.completed, 32768);
        assert_eq!(paused.speed, 1024);
    }

    /// `TaskState` / `FileState` —— 两组**变体名字面量**。
    ///
    /// 守：它们进状态文件、是**与 Go 版共用的持久化格式**（`core/src/engine/status.rs`
    /// 的 `rename_all = "lowercase"`），以及内核 UI 四态的线上名
    /// （`core/src/main.rs` 的 `state_name`）。写错一个字母，界面上的状态会**静默**变成
    /// 另一个（或解码直接失败）。
    #[test]
    fn task_and_file_states_pin_the_wire_literals() {
        for (v, want) in [
            (TaskState::Waiting, "waiting"),
            (TaskState::Active, "active"),
            (TaskState::Complete, "complete"),
            (TaskState::Error, "error"),
            (TaskState::Removed, "removed"),
        ] {
            assert_eq!(
                serde_json::to_value(v).expect("序列化不该失败"),
                json!(want),
                "TaskState 的线上字面量错了"
            );
        }
        for (v, want) in [
            (FileState::Pending, "pending"),
            (FileState::Downloading, "downloading"),
            (FileState::Complete, "complete"),
            (FileState::Failed, "failed"),
        ] {
            assert_eq!(
                serde_json::to_value(v).expect("序列化不该失败"),
                json!(want),
                "FileState 的线上字面量错了"
            );
        }
    }

    /// `TaskAction` —— `task_action` 的五个线上字面量（控制者裁决 S）。
    ///
    /// ⚠️ **这条测试原先坐在「壳自己持有的三个枚举（无线上形态）」那个块里，是错位**
    ///    （任务 14 顺手归位，只搬位置、一个字都没改）：`TaskAction` **带
    ///    `Serialize`/`Deserialize`**，它的五个字面量是要**发到内核**的线上值
    ///    （`op_task_action` 用字面量匹配），与那个块标题（"无线上形态"）正相反。
    ///    它该与 [`TaskState`]/[`FileState`] 那两条**同为线上字面量**的测试坐在一起。
    ///
    /// 守：五个字面量逐字（壳把它们当 `params.action` 发出去，内核用**字面量**匹配，
    /// 不认识的会回 `invalid_params`），且 `reveal` **不是**其中之一
    /// ——设计规格 §5.2 明确"在资源管理器中显示"是**壳**的事、不进协议。
    ///
    /// 顺带钉住 `Ord`：任务 15 用的是 `BTreeSet<TaskAction>`（`task-15-brief.md:33`）。
    /// 少了 `Ord`，那边会以一条编译错误的形式发现——不如在这里钉住。
    #[test]
    fn task_action_pins_the_five_wire_literals() {
        for (v, want) in [
            (TaskAction::Pause, "pause"),
            (TaskAction::Unpause, "unpause"),
            (TaskAction::Retry, "retry"),
            (TaskAction::Remove, "remove"),
            (TaskAction::ClearFinished, "clear_finished"),
        ] {
            assert_eq!(
                serde_json::to_value(v).expect("序列化不该失败"),
                json!(want),
                "TaskAction 的线上字面量错了（内核按字面量匹配 action）"
            );
        }
        assert!(
            serde_json::from_str::<TaskAction>(r#""reveal""#).is_err(),
            "reveal 不在协议里（设计规格 §5.2：它是壳的事）"
        );
        let set: std::collections::BTreeSet<TaskAction> =
            [TaskAction::Pause, TaskAction::Retry].into_iter().collect();
        assert_eq!(set.len(), 2);
    }

    /// `HelloResult` —— 握手回执。
    ///
    /// 守：`protocol` 与 `min_split_size_choices` 两个键名（任务 8 的 e2e 会对着真内核
    /// 钉它；这一条是不用起进程的第一道网）。
    /// ⚠️ 壳**不得**自己硬编码那份取值集合——设计规格 §2.3 的原话是"把集合交给壳"
    /// （同 `min_split_size_choices` 那条：后半截是呈现侧的事，本 crate 不含界面概念）。
    /// 所以本模块**只有**这个解码函数，
    /// **没有**那 100 个值的常量表（有了它就等于把集合抄了第二遍）。
    #[test]
    fn hello_result_carries_the_choices_from_the_kernel() {
        let h: HelloResult = serde_json::from_str(
            r#"{"protocol":1,"min_split_size_choices":["1M","2M","100M"]}"#,
        )
        .expect("握手回执必须能解");
        assert_eq!(h.protocol, PROTOCOL_VERSION);
        assert_eq!(
            h.min_split_size_choices,
            vec!["1M".to_string(), "2M".to_string(), "100M".to_string()]
        );

        let p: HelloParams = serde_json::from_str(r#"{"protocol":1}"#).expect("握手参数必须能解");
        assert_eq!(p.protocol, PROTOCOL_VERSION);
    }

    /// `DirEntry`（`list_dir`）与 `TreeNode`（`load_delivery` / `get_tree`）。
    ///
    /// 守三件事：
    ///   1. 判别键是 `"type"`（`"dir"`/`"file"`），**不是** `is_dir` 之类的布尔
    ///      （`core/src/main.rs` 的 `node_json`/`entries_json`）；
    ///   2. 目录项**没有 `path`**、只有 `children_count`；
    ///   3. 文件叶节点共用一个形状（`FileNode`），**两条 JSON 出口的键一模一样**
    ///      ——内核那边同一份数据写了两遍（`node_json` 与 `entries_json`），
    ///      漂移的代价是"字段静默到不了壳"，所以这里两侧都钉。
    ///
    /// ⚠️ **清单零文件时内核发的树是空对象 `{}`**，不是 `{"type":"dir",…}`
    ///    （`core/src/main.rs` 的 `node_json` 末段：`root.is_empty()` → `Map::new()`）。
    ///    少了 `TreeNode::Empty` 这一支，零文件的交付批次会让壳解码失败——
    ///    macOS 侧是**真机抓取**才发现这一条的（`Protocol.swift` 的注释）。
    #[test]
    fn entries_and_tree_nodes_carry_the_type_tag() {
        let dir_entry: DirEntry = serde_json::from_str(
            r#"{"type":"dir","name":"sub","children_count":3}"#,
        )
        .expect("目录项必须能解");
        match &dir_entry {
            DirEntry::Dir { name, children_count } => {
                assert_eq!(name, "sub");
                assert_eq!(*children_count, 3);
            }
            other => panic!("应解成目录项，实际 {other:?}"),
        }

        // 文件项：`source_mtime` 在、值为空串（内核两条出口都**始终发这个键**）
        let file_entry: DirEntry = serde_json::from_str(
            r#"{"type":"file","name":"a.txt","path":"sub/a.txt","size":26,"crc64":"123",
                "source_mtime":"","state":"downloading","completed":10,"total":26,
                "speed":5,"err":""}"#,
        )
        .expect("文件项必须能解");
        let f = match &file_entry {
            DirEntry::File(f) => f,
            other => panic!("应解成文件项，实际 {other:?}"),
        };
        assert_eq!(f.path, "sub/a.txt", "路径是清单原文，不得规范化");
        assert_eq!(f.state, FileState::Downloading);
        assert_eq!(f.total, 26);
        assert_eq!(f.completed, 10);
        assert_eq!(f.speed, 5);
        assert_eq!(f.err, "");
        assert_eq!(
            f.source_mtime.as_deref(),
            Some(""),
            "内核始终发这个键；键在而值为空串 = 内核说「这一条没有时间」"
        );

        // 老清单里没有 source_mtime 这个键 → nil（与上面的空串**不是一件事**）
        let old: DirEntry = serde_json::from_str(
            r#"{"type":"file","name":"a.txt","path":"a.txt","size":1,"crc64":"",
                "state":"pending","completed":0,"total":1,"speed":0,"err":""}"#,
        )
        .expect("老载荷（缺 source_mtime 键）必须还能解");
        let f = match old {
            DirEntry::File(f) => f,
            _ => unreachable!(),
        };
        assert_eq!(f.source_mtime, None, "缺键是 None，不是空串");

        // 树：非空的根是 `{"type":"dir","name":"","children":{…}}`
        let tree: TreeNode = serde_json::from_str(
            r#"{"type":"dir","name":"","children":{
                "a.txt":{"type":"file","name":"a.txt","path":"a.txt","size":26,"crc64":"123",
                         "source_mtime":"","state":"pending","completed":0,"total":26,
                         "speed":0,"err":""},
                "sub":{"type":"dir","name":"sub","children":{}}}}"#,
        )
        .expect("整树必须能解");
        match &tree {
            TreeNode::Dir { name, children } => {
                assert_eq!(name, "", "非空树的根是 name 为空的目录节点");
                assert_eq!(children.len(), 2);
                assert!(matches!(children.get("sub"), Some(TreeNode::Dir { .. })));
                assert!(matches!(children.get("a.txt"), Some(TreeNode::File(_))));
            }
            other => panic!("应解成目录，实际 {other:?}"),
        }

        // 零文件：空对象 → `Empty`
        let empty: TreeNode = serde_json::from_str("{}").expect("空树必须能解");
        assert_eq!(empty, TreeNode::Empty, "零文件时内核发的是 {{}}");

        // ⚠️ 另一半同样承重：**改名必须响亮地失败**（漂移探测器的意义就在这里）。
        // 放行未知判别值的后果是界面静默少掉一个节点/一条状态，而不是报错。
        assert!(
            serde_json::from_str::<DirEntry>(r#"{"type":"directory","name":"sub","children_count":1}"#).is_err(),
            "未知的条目类型必须被拒（内核只会发 dir/file）"
        );
        assert!(
            serde_json::from_str::<DirEntry>(
                r#"{"type":"file","name":"a","path":"a","size":1,"crc64":"",
                    "source_mtime":"","state":"finished","completed":0,"total":1,
                    "speed":0,"err":""}"#
            )
            .is_err(),
            "未知的 state 字面量必须被拒（内核只会发 pending/downloading/complete/failed）"
        );
        assert!(
            serde_json::from_str::<TreeNode>(r#"{"type":"dir","children":{}}"#).is_err(),
            "目录节点缺 name 必须被拒"
        );
    }

    /// `DeliveryInfo` —— `load_delivery` 的回执。
    ///
    /// 守：`op_load_delivery`（`core/src/main.rs`）那个 `json!` 里的**每一个键**
    /// ——`page_url`/`base_url`/`created_at`/`expires_at`/`expired`/`total_files`/`total_bytes`/`tree`
    /// 全部 snake_case。少一个键就是硬失败（那是漂移探测器，见文件头）。
    /// `expired` 让壳能在下载一个个 404 之前就把过期显示出来。
    #[test]
    fn delivery_info_mirrors_the_load_delivery_receipt() {
        let d: DeliveryInfo = serde_json::from_str(
            r#"{"code":"C24-8_×_25WS024","page_url":"http://d.example/C24-8/index.html",
                "base_url":"http://d.example","created_at":"2026-09-01T10:00:00+08:00",
                "expires_at":"2026-10-14T16:13:34+08:00","expired":false,
                "total_files":2,"total_bytes":26,"tree":{}}"#,
        )
        .expect("load_delivery 的回执必须能解");
        assert_eq!(d.code, "C24-8_×_25WS024", "非 ASCII 的交付码不得被改写");
        assert_eq!(d.page_url, "http://d.example/C24-8/index.html");
        assert_eq!(d.base_url, "http://d.example");
        assert_eq!(d.created_at, "2026-09-01T10:00:00+08:00");
        assert_eq!(d.expires_at, "2026-10-14T16:13:34+08:00");
        assert!(!d.expired);
        assert_eq!(d.total_files, 2);
        assert_eq!(d.total_bytes, 26);
        assert_eq!(d.tree, TreeNode::Empty);

        // 缺一个键就必须红——那是"内核把字段改名了"的信号，不许静默吞掉
        assert!(
            serde_json::from_str::<DeliveryInfo>(
                r#"{"code":"c","base_url":"b","created_at":"","expires_at":"",
                    "expired":false,"total_files":0,"total_bytes":0,"tree":{}}"#
            )
            .is_err(),
            "缺 page_url 必须解码失败（缺字段是漂移信号，不该静默）"
        );
    }

    /// `Settings` —— 客户可调的七项（`core/src/settings.rs`）。
    ///
    /// 守：七个键名与类型逐字（`min_split_size`/`limit_mbps` 这类多词键最容易改名），
    /// 且 `min_split_size` 是**字符串**（`"20M"`，不是数字）——它是 `-k`，
    /// 写错会让 aria2 直接启动失败（契约 §2.3）。
    #[test]
    fn settings_mirrors_the_seven_adjustable_fields() {
        let s: Settings = serde_json::from_str(
            r#"{"parallel":8,"connections":16,"splits":16,"min_split_size":"20M",
                "limit_mbps":0,"max_tries":3,"retry_wait":1}"#,
        )
        .expect("设置必须能解");
        assert_eq!(s.parallel, 8);
        assert_eq!(s.connections, 16);
        assert_eq!(s.splits, 16);
        assert_eq!(s.min_split_size, "20M");
        assert_eq!(s.limit_mbps, 0);
        assert_eq!(s.max_tries, 3);
        assert_eq!(s.retry_wait, 1);

        // ⚠️ `Settings` 是**双向**的：`set_settings` 要把壳改过的值发回内核。
        // 发出去的键名必须与收进来的逐字相同——否则改设置会静默无效。
        let back = serde_json::to_value(&s).expect("序列化不该失败");
        assert_eq!(
            back,
            json!({"parallel":8,"connections":16,"splits":16,"min_split_size":"20M",
                   "limit_mbps":0,"max_tries":3,"retry_wait":1})
        );
    }

    /// `GlobalStat` 与 `EnqueueResult`。
    ///
    /// 守：`GlobalStat` 是 snake_case（`core/src/engine/rpc.rs` 的 `GlobalStat`
    /// **没有** `rename_all`，macOS 靠 `.convertFromSnakeCase` 消化——所以线上就是
    /// snake_case，Rust 侧天然同形）；`EnqueueResult` 的 `added`/`rejected` 两个数组
    /// 与各自的键（`op_enqueue`）。`rejected` 是"哪些文件没进去、为什么"的**唯一**来源，
    /// 界面靠它显示失败原因。
    #[test]
    fn global_stat_and_enqueue_result_mirror_the_kernel() {
        let g: GlobalStat = serde_json::from_str(
            r#"{"download_speed":1024,"num_active":1,"num_waiting":2,"num_stopped":3}"#,
        )
        .expect("getGlobalStat 的结果必须能解");
        assert_eq!(g.download_speed, 1024);
        assert_eq!(g.num_active, 1);
        assert_eq!(g.num_waiting, 2);
        assert_eq!(g.num_stopped, 3);

        let e: EnqueueResult = serde_json::from_str(
            r#"{"added":[{"gid":"g1","path":"a.txt"}],
                "rejected":[{"path":"b.txt","reason":"路径不安全（越界/控制字符/空段）"}]}"#,
        )
        .expect("enqueue 的结果必须能解");
        assert_eq!(e.added.len(), 1);
        assert_eq!(e.added[0].gid, "g1");
        assert_eq!(e.added[0].path, "a.txt");
        assert_eq!(e.rejected.len(), 1);
        assert_eq!(e.rejected[0].path, "b.txt");
        assert_eq!(e.rejected[0].reason, "路径不安全（越界/控制字符/空段）");
    }

    // -----------------------------------------------------------------------
    // 壳自己持有的三个枚举（**无线上形态**）
    // -----------------------------------------------------------------------

    /// `EngineState` —— 引擎四态（**控制者裁决 C**：定义在 `protocol.rs`，
    /// `presentation::engine_status`（任务 12）`use crate::protocol::EngineState` 消费它。
    /// 两处各定义一份是必须避免的）。
    ///
    /// 守：四个分支两两可区分，且 `Unavailable` **逐字**带着内核给的原因
    /// （约束 3「壳不加工」；折行是**呈现层**的事，不是这里的）。
    #[test]
    fn engine_state_carries_the_kernel_reason_verbatim() {
        let states = [
            EngineState::Connecting,
            EngineState::NotStarted,
            EngineState::Running,
            EngineState::Unavailable("内核崩了\n第二行".to_string()),
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                assert_eq!(i == j, a == b, "四个分支必须两两可区分（{a:?} vs {b:?}）");
            }
        }
        match &states[3] {
            EngineState::Unavailable(reason) => assert_eq!(
                reason, "内核崩了\n第二行",
                "内核原文必须逐字保留（含换行；折行是呈现层的事）"
            ),
            other => panic!("应解成 Unavailable，实际 {other:?}"),
        }
    }

    /// `VerifyClass` —— 校验的六个归类。
    ///
    /// ⚠️ **本枚举只有变体，没有方法**：`label`/`color`/`icon_name`/`note`/`is_failure`
    /// 是**呈现层**的事（要用 `presentation::RowColor`），由任务 14 在
    /// `presentation/verify_summary.rs` 里 `impl`（同 crate，可以直接给这里定义的
    /// 类型写固有 impl）。**变体只定义一份**——理由同裁决 C。
    ///
    /// 守：六个归类两两可区分。它存在的意义是"不得静默少交"：`unverifiable` 与
    /// `unreadable` 必须与"通过"分开，否则界面会把一次没验成的交付报成齐全。
    #[test]
    fn verify_class_has_six_distinct_classes() {
        let all = [
            VerifyClass::Passed,
            VerifyClass::Mismatched,
            VerifyClass::Missing,
            VerifyClass::SizeMismatch,
            VerifyClass::Unverifiable,
            VerifyClass::Unreadable,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b, "六个归类必须两两可区分（{a:?} vs {b:?}）");
            }
        }
    }

    /// `VerifyClass::wire_key()` —— **变体名 → 线上键名**的六条映射（控制者裁决 V）。
    ///
    /// 守的是本模块最险的一处：`verify_status` 的六个键是
    /// `ok`/`bad`/`missing`/`size_mismatch`/`unverifiable`/`unreadable`
    /// （`core/src/main.rs:1601-1611` 的 `op_verify_status`），而本枚举的变体名是
    /// `Passed`/`Mismatched`/…——**不是同一个字符串**。日后写 `VerifyStatus` 信封的人
    /// 若按变体名去解 `"passed"`，会得到**六个空桶**，而界面会报**"全部通过"**
    /// ——静默地报成功，本项目最恨的形态。钉住它的就是这一条。
    ///
    /// ⚠️ 这条测试**不能**替掉任务 8 的 e2e：e2e 拿真内核的 `verify_status` 回执
    /// 核对这六个键**真的在回执里**（副本抄错的探测器），本条只钉壳自己这张表。
    #[test]
    fn verify_class_wire_keys_match_the_verify_status_receipt() {
        let expected: [(VerifyClass, &str); 6] = [
            (VerifyClass::Passed, "ok"),
            (VerifyClass::Mismatched, "bad"),
            (VerifyClass::Missing, "missing"),
            (VerifyClass::SizeMismatch, "size_mismatch"),
            (VerifyClass::Unverifiable, "unverifiable"),
            (VerifyClass::Unreadable, "unreadable"),
        ];

        // ① 六条映射逐条钉住（判据 = 内核 `op_verify_status` 的 json! 键名）
        for (class, key) in expected {
            assert_eq!(class.wire_key(), key, "wire_key() 与内核的键名不符");
        }

        // ② 六个键两两不重：两个归类落到同一个桶 = 静默少交一个归类
        let mut keys: Vec<&str> = expected.iter().map(|(_, k)| *k).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 6, "六个键里有重复");

        // ③ **两套命名确实不同**——六条里有两条（`Passed`→`ok`、`Mismatched`→`bad`）
        //    连"变体名 snake_case 化之后"都对不上。这两条正是"按变体名解会解出空桶"
        //    的落点，所以单独点出来：它们是这条映射存在的**全部理由**，不是装饰。
        //    （另外四条恰好与变体名的 snake_case 同形——**同形纯属巧合**，别据此
        //     把 wire_key() 换成 `#[serde(rename_all = "snake_case")]`：那两条会错。）
        assert_ne!(
            VerifyClass::Passed.wire_key(),
            "passed",
            "线上键是 ok —— 按变体名解 \"passed\" 会解出空桶"
        );
        assert_ne!(
            VerifyClass::Mismatched.wire_key(),
            "mismatched",
            "线上键是 bad —— 这正是「校验失败的文件一条都没进界面」的那个坑"
        );

        // ④ 把六条绑到 `op_verify_status` 回执的**键集合**上：
        //    「六个都在回执里」+「回执里除 all_good 外恰好这六个」两个方向都查。
        let receipt = json!({
            "ok": [], "bad": [], "missing": [], "size_mismatch": [],
            "unverifiable": [], "unreadable": [], "all_good": true
        });
        let obj = receipt.as_object().expect("回执一定是对象");
        for (class, key) in expected {
            assert!(obj.contains_key(key), "回执里没有 {key}（{class:?} 的线上键）");
        }
        let mut extra: Vec<&String> = obj.keys().filter(|k| k.as_str() != "all_good").collect();
        extra.retain(|k| !expected.iter().any(|(_, want)| want == &k.as_str()));
        assert!(extra.is_empty(), "回执里有不在映射表里的键：{extra:?}");
    }

    // -----------------------------------------------------------------------
    // `get_settings` / `set_settings` 的回执（任务 8）
    // -----------------------------------------------------------------------

    /// 回执的**两个键**与**七个内层键**逐字（`op_get_settings` 的 `json!` 原样）。
    ///
    /// 判别力：把 `last_code` 改成 `lastCode`、或把内层那一份换成 camelCase
    /// （`minSplitSize`/`limitMbps`），这一条立刻红 —— 而真机上的表现是
    /// **参数面板整屏空白**（`settings` 解不出来）或者 `-k` 那一格是空的。
    #[test]
    fn the_settings_receipt_carries_both_keys() {
        let receipt = json!({
            "settings": {
                "parallel": 8, "connections": 16, "splits": 16,
                "min_split_size": "20M", "limit_mbps": 0,
                "max_tries": 3, "retry_wait": 1
            },
            "last_code": "C24-8"
        });
        let decoded: SettingsResult =
            serde_json::from_value(receipt.clone()).expect("这份回执必须解得出 SettingsResult");
        assert_eq!(decoded.last_code, "C24-8");
        assert_eq!(decoded.settings.parallel, 8);
        assert_eq!(decoded.settings.min_split_size, "20M");

        // 两个键都在、不多不少（多一个键就是"内核开始发别的东西了"，那是漂移）。
        let obj = receipt.as_object().expect("回执一定是对象");
        let keys: std::collections::BTreeSet<&str> =
            obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["settings", "last_code"].into_iter().collect(),
            "回执的顶层键只有这两个"
        );

        // ⚠️ **`last_code` 空串是合法值**（内核还没记过任何码时就是它）——
        //    它不是缺键，所以不许为了"更好看"把它变成 `Option` 再当 `null` 发出去。
        let fresh: SettingsResult = serde_json::from_value(json!({
            "settings": {
                "parallel": 8, "connections": 16, "splits": 16,
                "min_split_size": "20M", "limit_mbps": 0,
                "max_tries": 3, "retry_wait": 1
            },
            "last_code": ""
        }))
        .expect("空串必须解得出来");
        assert!(fresh.last_code.is_empty());
    }
}
