import Foundation

// ---------------------------------------------------------------------------
// stdio 上的 JSON Lines 协议（内核侧：`core/src/protocol.rs`、`core/src/main.rs`）
//
// ⚠️ 本文件是**抄写**，不是设计。每个键名、每个枚举字面量、每个错误码都逐字对齐内核；
//    改这里的任何一处之前，先回去读内核那两处并跑一次真内核（`--debug` 式的取证命令见
//    `ProtocolTests.swift` 顶部的注释）。壳是**另一个语言、另一个阶段的产物**，
//    它拿不到内核的常量——协议就是唯一的接缝。
// ---------------------------------------------------------------------------

/// 协议版本。握手时协商（`core/src/protocol.rs:26` 的 `PROTOCOL_VERSION`）。
public let kProtocolVersion: UInt32 = 1

// ---------------------------------------------------------------------------
// 信封
// ---------------------------------------------------------------------------

/// 内核回的**原始信封**：`{"id":N,"ok":bool,"result":{…}}` 或
/// `{"id":N,"ok":false,"error":{"code":…,"message":…}}`。
///
/// ⚠️ `result` 与 `error` 由内核的 `skip_serializing_if` 保证**互斥**：
/// `ok:true` 的响应里没有 `error` 键，`ok:false` 里没有 `result` 键
/// （内核的 `ok_response_omits_the_error_key` / `err_response_omits_the_result_key`
/// 断言的正是序列化后的**文本**）。壳这边同样按"二选一"读，不假设另一个是 `null`。
public struct RawEnvelope: Decodable, Sendable {
    public let id: UInt64
    public let ok: Bool
    public let result: JSONValue?
    public let error: ErrorBody?
}

extension RawEnvelope {
    /// 取出 `result`（**不做任何二次编解码**，原样交出去）。
    ///
    /// 这是"信封 → result"的**唯一**实现：`ok:false` 与"`ok:true` 却缺 `result`"
    /// 两条守卫只此一份。`unwrap`（要强类型）与 `CoreClient.callSync`（要原始
    /// [`JSONValue`]）都走它。
    ///
    /// ⚠️ **不许在别处复制这两个守卫**（哪怕文案看起来一样）：`CoreClient` 曾经抄过一份，
    /// 结果是改一处、另一条路径静默保持旧措辞——而这两条文案都会显示给客户。
    /// 钉住它的是 `callSyncAndUnwrapShareTheRpcErrorMessage` /
    /// `callSyncAndUnwrapShareTheMissingResultMessage`（两条入口产物全等比较 + 文案全等）。
    public func unwrapResult() throws -> JSONValue {
        guard ok else {
            throw CoreError.rpc(code: error?.code ?? "", message: error?.message ?? "")
        }
        guard let result else {
            throw CoreError.malformedResponse("ok:true 的响应里没有 result（id \(id)）")
        }
        return result
    }

    /// 取出 `result` 并解成 `T`。
    ///
    /// `ok == false` 时抛 [`CoreError.rpc`]——**带上内核的结构化 `code`**，
    /// 调用方按 `code` 分支（契约 §5.1：壳不得靠 `message` 的措辞判断错误类型）。
    ///
    /// 只在"调用方自己要强类型"时才走这里的编解码；只要原始 [`JSONValue`] 的调用方
    /// （`CoreClient.callSync`）走 [`unwrapResult`]，不必来回编码一趟
    /// （`.integer` / `.number` 是分开的两个 case，二次编解码会丢这个区分）。
    public func unwrap<T: Decodable>(_ t: T.Type) throws -> T {
        let value = try unwrapResult()
        do {
            let data = try CoreJSON.encoder.encode(value)
            return try CoreJSON.decoder.decode(T.self, from: data)
        } catch {
            throw CoreError.malformedResponse("id \(id) 的 result 与 \(T.self) 的形状不符: \(error)")
        }
    }
}

/// 内核的错误体（`core/src/protocol.rs` 的 `ErrorBody`）。
public struct ErrorBody: Decodable, Equatable, Sendable {
    /// 结构化错误码，取值见 [`ErrorCode`]。**壳按它分支，不按 `message` 措辞。**
    public let code: String
    /// 给人看的一句话。**措辞会变，不要匹配它。**
    public let message: String
}

/// 壳侧的错误。
public enum CoreError: Error, Equatable {
    /// 内核明确回了一条 `ok:false` 的响应。`code` 是 [`ErrorCode`] 的 raw value。
    case rpc(code: String, message: String)
    /// 连不上内核进程 / 子进程退出 / 管道断开之类，**协议之外**的失败。
    case transport(String)
    /// 收到的字节不是一条合法响应（或 `result` 的形状与请求的方法对不上）。
    case malformedResponse(String)
}

/// 十三种结构化错误码，**逐字**对齐内核。
///
/// ⚠️ 内核把这份清单**分裂在两处**：`core/src/protocol.rs:33-46` 四个
/// （协议层自己会回的：`bad_request` / `invalid_params` / `protocol_mismatch` / `internal`）
/// 与 `core/src/main.rs:74-93` 九个（各业务方法回的）。**两处都要抄全**——
/// 少一个的后果是壳把一条可识别的人话错误降级成"未知错误"。
public enum ErrorCode: String, CaseIterable, Sendable {
    // core/src/protocol.rs
    case badRequest = "bad_request"
    case invalidParams = "invalid_params"
    case protocolMismatch = "protocol_mismatch"
    case internalError = "internal"
    // core/src/main.rs
    case unknownMethod = "unknown_method"
    case noDelivery = "no_delivery"
    case deliveryFetchFailed = "delivery_fetch_failed"
    case preflightFailed = "preflight_failed"
    case engineNotStarted = "engine_not_started"
    case engineStartFailed = "engine_start_failed"
    case engineDisconnected = "engine_disconnected"
    case engineRpcFailed = "engine_rpc_failed"
    case pathNotFound = "path_not_found"
}

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

/// 请求行的编码。
///
/// 形状固定为 `{"id":N,"method":"m","params":<json>}`，**一行、不含换行符**
/// （JSON Lines：换行由调用方补，`stdout` 是协议专用通道）。
public enum RequestLine {
    public static func encode(id: UInt64, method: String, params: JSONValue) throws -> String {
        // 两段都交给 `JSONEncoder` 以求**正确的转义**（`method` 里可能出现引号、
        // `params` 里可能出现换行），但外层三个键自己拼：键序是契约的一部分，
        // 不依赖 `sortedKeys` 恰好也排成这个次序。
        let m = String(decoding: try CoreJSON.encoder.encode(method), as: UTF8.self)
        let p = String(decoding: try CoreJSON.encoder.encode(params), as: UTF8.self)
        return #"{"id":\#(id),"method":\#(m),"params":\#(p)}"#
    }
}

// ---------------------------------------------------------------------------
// 枚举字面量（**逐字**，见内核的 `rename_all` 与各 `json!` 构造点）
// ---------------------------------------------------------------------------

/// `TransferItem.state`：领域状态四态合成的结果（`core/src/engine/status.rs`）。
///
/// ⚠️ aria2 的 `paused` 与 `waiting` **映到同一个 `Waiting`**（契约 §8 表的 `#14`，
/// 有意为之）。要区分「已暂停」只能看 [`TransferItem.rawStatus`]。
public enum TaskState: String, CaseIterable, Codable, Sendable {
    case waiting, active, complete, error, removed
}

/// 每个文件的四态（`core/src/view.rs` 的 `State`，经 `state_name()` 输出）。
/// 树、`flat`、`list_dir` 的 `state` 键共用这一套。
public enum FileState: String, CaseIterable, Codable, Sendable {
    case pending, downloading, complete, failed
}

/// `verify_status` 的六个分桶（**是键名，不是值**）。
///
/// `size_mismatch` 的 raw value 带下划线（内核的键就是它），
/// 而属性名是 camelCase——两者并存是有意的：raw value 走协议，属性名走 Swift。
public enum VerifyClass: String, CaseIterable, Sendable {
    case ok, bad, missing
    case sizeMismatch = "size_mismatch"
    case unverifiable, unreadable
}

/// `plan.items[].kind`（`core/src/planner.rs` 的 `Kind`）。
public enum PlanKind: String, CaseIterable, Codable, Sendable {
    case skip, download
}

/// `task_action` 的 `action`（`core/src/main.rs` 的 `op_task_action`）。
public enum TaskAction: String, CaseIterable, Sendable {
    case pause, unpause, retry, remove
    case clearFinished = "clear_finished"
}

// ---------------------------------------------------------------------------
// 各方法的结果类型
// ---------------------------------------------------------------------------

/// `hello` 的结果（`core/src/protocol.rs` 的 `HelloResult`）。
///
/// ⚠️ 线上键是 `protocol`（不是 `protocol_version`）。`CodingKeys` 里写的
/// `"protocol"` 是**转换后**的键名——`CoreJSON.decoder` 的 `.convertFromSnakeCase`
/// 对无下划线的键原样透传，所以两者对得上。这条交互由 `decodesHelloResult` 钉住。
public struct HelloResult: Decodable, Sendable {
    public let protocolVersion: UInt32
    /// `-k`（最小分片大小）的取值集合，真内核回 `1M`–`100M` 整整 100 项。
    ///
    /// **它必须从协议里取**，不能写成壳里的一份硬编码：客户端的枚举控件就是照它建的
    /// （契约 §2.3 的"让客户不可能输错"）。
    public let minSplitSizeChoices: [String]

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol"
        case minSplitSizeChoices
    }
}

/// 参数面板的七项（`core/src/settings.rs` 的 `Settings`）。**七个键一个不能少。**
///
/// ⚠️ **本类型是壳里唯一一个"要被编码发出去"的类型化载荷**（`set_settings`）。
/// 发的时候**必须**用 [`CoreJSON.requestEncoder`]，不能用 [`CoreJSON.encoder`]：
/// 属性名是 camelCase，而内核 `set_settings` 走 `serde_json::from_value::<Settings>`
/// （`core/src/main.rs:1546`）、`Settings` 没有任何 `rename`（`core/src/settings.rs:32-47`），
/// 所以线上键是 `min_split_size` / `limit_mbps` / `max_tries` / `retry_wait`。
/// 用错编码器出来的 payload **看起来完全正常（也是七个键）**，只是内核一定回
/// `invalid_params`。钉住它的是 `settingsEncodeToTheKernelWireKeys`。
public struct Settings: Codable, Equatable, Sendable {
    public var parallel: Int32        // -j，1–64
    public var connections: Int32     // -x，1–16
    public var splits: Int32          // -s，1–16
    public var minSplitSize: String   // -k，1M–100M
    public var limitMbps: Int64       // 0 = 不限速
    public var maxTries: Int32        // 1–100
    public var retryWait: Int32       // 0–60 秒

    public init(parallel: Int32, connections: Int32, splits: Int32, minSplitSize: String,
                limitMbps: Int64, maxTries: Int32, retryWait: Int32) {
        self.parallel = parallel
        self.connections = connections
        self.splits = splits
        self.minSplitSize = minSplitSize
        self.limitMbps = limitMbps
        self.maxTries = maxTries
        self.retryWait = retryWait
    }
}

/// `get_settings` / `set_settings` 的结果。
///
/// `lastCode` 是**上次用过的交付码**，单独返回、不在 `settings` 里面
/// （它是运行状态，不是参数面板上的那一项）。
public struct SettingsResult: Decodable, Sendable {
    public let settings: Settings
    public let lastCode: String
}

/// `load_delivery` 的结果。
///
/// `Equatable` 是任务 3 加的：`AppModel.LoadState.loaded(DeliveryInfo)` 要整体可比
/// （简报钉死了 `LoadState: Equatable`）。成员本来就都是 Equatable，合成实现在同一文件里。
public struct DeliveryInfo: Decodable, Equatable, Sendable {
    public let code: String
    /// 交付页地址（`base_url/code/index.html`）——"去哪儿看交付页"就是它。
    public let pageUrl: String
    public let baseUrl: String
    /// ISO8601 带时区的字符串（**可能是空串**，不要当日期解）。
    public let createdAt: String
    public let expiresAt: String
    /// 已过期要让客户看见，而不是等下载一个文件一个文件地 404。
    public let expired: Bool
    public let totalFiles: Int64
    public let totalBytes: Int64
    public let tree: TreeNode
}

/// 交付清单的树。
///
/// 判别键是 `"type"`（`"dir"` / `"file"`），**不是 `is_dir` 之类的布尔**
/// （`core/src/main.rs` 的 `node_json`）。
public indirect enum TreeNode: Decodable, Equatable, Sendable {
    case dir(name: String, children: [String: TreeNode])
    case file(FileNode)
    /// ⚠️ **清单零文件时内核发的是空对象 `{}`**，不是 `{"type":"dir",…}`。
    /// 真实抓取：`"tree":{}`。少了这一支，零文件的交付批次会让壳解码失败。
    case empty

    private enum CodingKeys: String, CodingKey { case type, name, children }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if c.allKeys.isEmpty {
            self = .empty
            return
        }
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "dir":
            self = .dir(name: try c.decode(String.self, forKey: .name),
                        children: try c.decode([String: TreeNode].self, forKey: .children))
        case "file":
            self = .file(try FileNode(from: decoder))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: c, debugDescription: "未知的节点类型 \(type)")
        }
    }
}

/// 树 / `list_dir` 里的文件叶节点（`core/src/main.rs` 的 `one()` 与 `entries_json()`）。
public struct FileNode: Decodable, Equatable, Sendable {
    public let name: String
    /// 清单里的**原文**相对路径（全局约束 3：不得规范化），含空格与非 ASCII。
    public let path: String
    /// CRC64 的**十进制字符串**；清单回读失败时是**空串**。
    public let crc64: String
    public let size: Int64
    public let completed: Int64
    public let total: Int64
    public let speed: Int64
    public let state: FileState
    /// 失败原因，无错时是空串。
    public let err: String
    /// 源文件的修改时间（内核原文，ISO 8601 带 +08:00）。**必须可选**：
    /// 已交付的老清单里没有这个键，而一个非可选 `String` 会让**整条载荷解码失败**
    /// （同 `BrowserRow.swift` 里记着的那次教训：一个不认识的枚举串就是这个后果）。
    ///
    /// ⚠️ **缺键 → `nil`，键在但值是空串 → `""`**（那是两种输入，不是一件事）：
    ///    内核 `node_json` / `entries_json` 两条出口**始终发这个键**、无值时发空串
    ///    （任务 4b），所以空串是"内核说这一条没有时间"，而 `nil` 是"这份清单比这个字段还老"。
    ///    两者在界面上的落点相同（`SourceTimeText` 都给 `—`），但**不要**在这里把它们合并：
    ///    合并之后就再也分不出"解码时把键丢了"与"内核真的没给值"。
    public let sourceMtime: String?
}

/// `list_dir` 的一项。目录项**没有 `path` 键**，只有子项数。
public enum DirEntry: Decodable, Equatable, Sendable {
    case dir(name: String, childrenCount: Int)
    case file(FileNode)

    private enum CodingKeys: String, CodingKey { case type, name, childrenCount }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "dir":
            self = .dir(name: try c.decode(String.self, forKey: .name),
                        childrenCount: try c.decode(Int.self, forKey: .childrenCount))
        case "file":
            self = .file(try FileNode(from: decoder))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: c, debugDescription: "未知的条目类型 \(type)")
        }
    }
}

public struct ListDirResult: Decodable, Sendable {
    /// **空串 = 根**（内核把根目录的 `path` 回成 `""`，不是 `"/"`）。
    public let path: String
    public let entries: [DirEntry]
}

/// `get_tree` 的 `flat[]`：树序的扁平列表（供壳免递归渲染大目录）。
public struct FlatEntry: Decodable, Sendable {
    public let path: String
    public let name: String
    public let size: Int64
    public let state: FileState
}

/// `get_tree` 的 `progress`。
public struct Progress: Decodable, Sendable {
    public let totalBytes: Int64
    public let doneBytes: Int64
    public let speed: Int64
    /// 内核已经算好的整数百分比（**向零截断**，不是四舍五入，契约 §1.6）。
    public let percent: Int32
}

public struct TreeResult: Decodable, Sendable {
    public let tree: TreeNode
    public let flat: [FlatEntry]
    /// 给勾选框的默认选中面（待下载 / 失败的那些路径）。
    public let defaultSelected: [String]
    public let progress: Progress
}

public struct PlanItem: Decodable, Sendable {
    public let path: String
    public let size: Int64
    public let crc64: String
    public let kind: PlanKind
}

public struct PlanResult: Decodable, Sendable {
    public let strict: Bool
    public let items: [PlanItem]
    /// 取不到 crc64 的文件（**它们仍然要下载**）。
    public let unverifiable: [String]
    public let complete: [String]
}

public struct AddedTask: Decodable, Sendable {
    public let gid: String
    public let path: String
}

public struct RejectedTask: Decodable, Sendable {
    public let path: String
    public let reason: String
}

public struct EnqueueResult: Decodable, Sendable {
    public let added: [AddedTask]
    public let rejected: [RejectedTask]
}

/// `transfer_list` 的一项（`core/src/protocol.rs` 的 `TransferItem`）。
public struct TransferItem: Decodable, Sendable {
    public let gid: String
    public let total: Int64
    public let completed: Int64
    public let speed: Int64
    public let conns: Int32
    /// 领域状态。**它把 aria2 的 `paused` 与 `waiting` 合成同一个 `waiting`。**
    public let state: TaskState
    /// **aria2 的原始状态串，逐字**（`active`/`waiting`/`paused`/`complete`/…）。
    ///
    /// ⚠️ 界面上那个「已暂停」角标**只能**靠它（设计规格 §8.4 / 裁决 #88）：
    /// 领域状态里 `paused` 与 `waiting` 长得一模一样。
    /// **不得由 `state` 反推**——反推出来的 `paused` 会变成 `waiting`。
    /// 取不到时内核传**空串**（"不知道"），不要替它猜。
    public let rawStatus: String
    /// aria2 的 `errorMessage`，无错时是空串。
    public let errorMessage: String
    /// 清单相对路径（GID 映射）；**没有映射时内核发 `null` 而不是省略键**。
    public let path: String?
}

public struct GlobalStat: Decodable, Sendable {
    public let downloadSpeed: Int64
    public let numActive: Int64
    public let numWaiting: Int64
    public let numStopped: Int64
}

public struct TransferListResult: Decodable, Sendable {
    public let items: [TransferItem]
    public let global: GlobalStat
}

/// `verify_status` 的结果。六个分桶都是路径数组。
public struct VerifyStatus: Decodable, Sendable {
    public let ok: [String]
    public let bad: [String]
    public let missing: [String]
    public let sizeMismatch: [String]
    public let unverifiable: [String]
    public let unreadable: [String]
    /// ⚠️ 内核的 `all_good()` **不含 `unverifiable`**（`core/src/verify.rs`）：
    /// "取不到 crc64 所以只比了大小"不等于"坏"，但它也不是"已核对无误"。
    public let allGood: Bool

    public func paths(_ c: VerifyClass) -> [String] {
        switch c {
        case .ok: return ok
        case .bad: return bad
        case .missing: return missing
        case .sizeMismatch: return sizeMismatch
        case .unverifiable: return unverifiable
        case .unreadable: return unreadable
        }
    }
}

/// `.benagen-state.json` 的线上形态（`get_state`）。
public struct StateFile: Decodable, Sendable {
    public let version: Int32
    public let code: String
    public let files: [String: StateEntry]
}

public struct StateEntry: Decodable, Sendable {
    public let size: Int64
    /// 秒级浮点时间戳（`engine::mtime_secs` 那一份换算的产物）。
    public let mtime: Double
    public let crc64: String
}
