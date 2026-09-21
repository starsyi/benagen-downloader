import Foundation

/// 协议里那些"形状随方法变"的位置的通用 JSON 值：请求的 `params`、响应的 `result`。
///
/// ⚠️ **`integer(Int64)` 与 `number(Double)` 必须分开，不能合成一个 `number(Double)`**：
/// 协议里 `size` / `completed` / `total` / `speed` 是 i64，而 19 位的数（CRC 的整数形态、
/// 大文件字节数）**装不进 Double 的 53 位尾数**——`5432380796884633278` 走一趟 Double
/// 回来就变成 `5.432380796884634e+18`，那是**另一个数**。
/// 钉住它的是 `jsonValueKeepsNineteenDigitIntegersExact`。
///
/// （`crc64` 在协议里是 i64 的**字符串**形态 `"5432380796884633278"`，所以它走 `.string`，
/// 与这里无关；但 `get_state` 的字段名与取值形状说明"19 位整数在这个协议里是家常便饭"。）
public enum JSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case integer(Int64)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])
}

extension JSONValue {
    /// 按 [`JSONValue`] 文档注释里的顺序逐个探测。
    ///
    /// ⚠️ **整数必须排在 `Double` 前面**：反过来的话，一个 JSON 整数会先被
    /// `decode(Double.self)` 接住，19 位的值当场丢精度（见类型注释）。
    /// 这一条与"枚举成员的声明顺序"不同——**探测顺序是按判别力定的，不是照抄声明顺序**。
    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
            return
        }
        // `Bool` 排在整数前面是安全的：JSON 的 `1` 解不成 `Bool`（实测），
        // 而 `true` 解不成 `Int64`。两者不会互相吞。
        if let b = try? c.decode(Bool.self) {
            self = .bool(b)
            return
        }
        if let i = try? c.decode(Int64.self) {
            self = .integer(i)
            return
        }
        if let d = try? c.decode(Double.self) {
            self = .number(d)
            return
        }
        if let s = try? c.decode(String.self) {
            self = .string(s)
            return
        }
        if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
            return
        }
        if let o = try? c.decode([String: JSONValue].self) {
            self = .object(o)
            return
        }
        throw DecodingError.dataCorrupted(.init(
            codingPath: decoder.codingPath,
            debugDescription: "不是合法的 JSON 值（null/bool/number/string/array/object 都不是）"))
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let b): try c.encode(b)
        case .number(let d): try c.encode(d)
        case .integer(let i): try c.encode(i)      // 走 Int64 的重载，不走 Double
        case .string(let s): try c.encode(s)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }
}

// ---------------------------------------------------------------------------
// 协议统一的编解码口径
// ---------------------------------------------------------------------------

/// 协议层**唯一**的 JSON 编解码口径。壳里每一处解析内核输出、每一处构造请求，
/// 都必须走这里——各写各的 `JSONDecoder()` 会让键策略在两处悄悄分叉。
///
/// ⚠️ **三个口径，别串**（`decoder` 一个，`encoder` / `requestEncoder` 两个）：
///
/// | 走哪条 | 用在 | 键 |
/// |---|---|---|
/// | `decoder` | 解析内核发来的一切 | `.convertFromSnakeCase` |
/// | `encoder` | [`JSONValue`]（`params` / `result` / 任何"已是线上形状"的值） | **不动** |
/// | `requestEncoder` | **类型化载荷**（`Settings`） | `.convertToSnakeCase` |
///
/// 一句话判据：**值的键名是"Swift 属性名"就用 `requestEncoder`，是"线上键名"就用 `encoder`。**
/// 串了口径的后果写在 [`CoreJSON.requestEncoder`] 的注释里；
/// `settingsEncodeToTheKernelWireKeys` 钉住前者的正确输出，
/// `theTwoEncodersAreNotInterchangeable` 钉住两者没有被合并。
public enum CoreJSON {
    /// 解码器：键策略固定为 `.convertFromSnakeCase`。
    ///
    /// 内核的线上形态是 **snake_case**（`min_split_size_choices` / `raw_status` /
    /// `size_mismatch` / `page_url` / `children_count` / `total_bytes` …），
    /// 而 Swift 侧的属性名是 camelCase。策略与显式 `CodingKeys` 的交互
    /// **已被测试钉住**（`snakeCaseStrategyReachesExplicitAndImplicitKeys` 等），不是假设：
    /// decoder 先把 JSON 键转成 camelCase，再拿它去比对 `CodingKeys` 的 `stringValue`，
    /// 所以 `case protocolVersion = "protocol"` 必须写成**转换后**的 `"protocol"` 才对得上。
    ///
    /// 返回**新实例**而不是共享单例：`JSONDecoder` 不是并发安全的，
    /// 一个 `static let` 会让两处并行解析共享可变状态（Swift 6 的隔离检查也会拦）。
    public static var decoder: JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    /// 编码器（[`JSONValue`] 专用）：键**原样搬运**，键序稳定（排障时两次请求的
    /// `params` 能逐字比对），且不把 `/` 转义成 `\/`。
    ///
    /// **没有键策略**（进来的值已经是线上形状：`params` 由调用方按线上键名拼好，
    /// `result` 是内核原样发来的）。诚实说明一句：**今天就算给它加上
    /// `.convertToSnakeCase` 也不会改变 `JSONValue` 的输出**——`JSONValue.encode(to:)`
    /// 走 `singleValueContainer`，键策略到不了那条路径（实测见
    /// `theTwoEncodersAreNotInterchangeable` 的注释）。所以"不加"是为了**意图清楚**、
    /// 也是为了不给 `JSONValue` 的未来实现埋雷，而不是因为它今天会出错。
    public static var encoder: JSONEncoder {
        let e = JSONEncoder()
        e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return e
    }

    /// 编码器：**类型化载荷**用这个（目前只有 `Settings`，`set_settings` 要把它整个发出去）。
    ///
    /// ⚠️ **两个编码器的分工不能串**，理由各是一条真实的错：
    ///   - 拿 [`encoder`] 编 `Settings` → 属性名是 camelCase，出去的是
    ///     `minSplitSize`/`limitMbps`/…，而内核 `set_settings` 走
    ///     `serde_json::from_value::<Settings>`（`core/src/main.rs:1546`）且 `Settings`
    ///     没有任何 `rename`（`core/src/settings.rs:32-47`）→ 缺 `min_split_size`
    ///     → 回 `invalid_params`。**payload 看起来完全正常（也是七个键）**，
    ///     要查到这一层才知道是键名不对。钉住它的是 `settingsEncodeToTheKernelWireKeys`。
    ///   - 拿 [`requestEncoder`] 编 [`JSONValue`] —— 结论同样是"别这么干"，但**理由不同**：
    ///     不是它今天会改坏键（`singleValueContainer` 让键策略够不着，见 [`encoder`] 的注释），
    ///     而是 [`JSONValue`] 的键是**线上键名**（`params` 由调用方逐字拼好，`result`
    ///     是内核原文），两个编码器混用会让"这个键该不该转"变成一件要现场推的事。
    ///     两个编码器是否分开，由 `theTwoEncodersAreNotInterchangeable` 钉住。
    ///
    /// 判据一句话：**值的键名是"Swift 属性名"就用这个，是"线上键名"就用 [`encoder`]。**
    public static var requestEncoder: JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return e
    }
}
