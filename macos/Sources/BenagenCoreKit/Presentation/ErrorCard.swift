import Foundation

// ---------------------------------------------------------------------------
// 错误卡片：内核给的那一格分型（`ErrorBody.class`）→ **一句人话 + 一个动作**
// （规格 W2 / I-2；样板是 `windows/shell-core/src/presentation/error_card.rs`）。
//
// ## 为什么这一层在壳里（而不是内核直接发三句话）
//
// 内核对"哪里坏了"最清楚，但"同类错误 30 秒内只更新计数、多条并存按严重度置顶"
// 是**界面行为**；而中英两列句子必须与内核 `error_recent` 给的那三句**逐字一致**，
// 才能在语言切换 / 两端对不上时有可对照的基准。⇒ 分工：内核给 `class`（线上词）
// 与计数，壳给句子与次序。
//
// ## 两张表必须都在
//
// `title` / `explain` / `action` 三个函数都对 [`ErrorClass`] 做 `switch` ——
// 少写一条是**编译错误**（与 `core/src/errors/class.rs` 同一条纪律）；
// 英文表里有汉字会被 `ErrorCardTests.noCJKInTheEnglishCards` 抓住。
// ---------------------------------------------------------------------------

/// 八类病灶（线上词见 `rawValue`，与 `core/src/errors/class.rs::Class::wire` 逐字相同）。
public enum ErrorClass: String, CaseIterable, Equatable, Sendable {
    case networkUnreachable = "network_unreachable"
    case dnsFailed = "dns_failed"
    case serverRefused = "server_refused"
    case diskSlow = "disk_slow"
    case diskFull = "disk_full"
    case engineUnresponsive = "engine_unresponsive"
    case engineCrashed = "engine_crashed"
    case verifyMismatch = "verify_mismatch"

    /// 线上词（`rawValue` 的别名，读起来与内核那侧同名）。
    public var wire: String { rawValue }

    /// 从线上词反解（不认识 ⇒ `nil`：**不猜**，界面按"意料之外的错误"处置）。
    public static func ofWire(_ wire: String?) -> ErrorClass? {
        guard let wire else { return nil }
        return ErrorClass(rawValue: wire)
    }

    /// 严重度（大的先显示；与内核 `Class::severity` **同源同序**，规格 §2.2）。
    ///
    /// 两两不同是承重的：同类错误的排序若打平，同一秒里发生两类错误时
    /// 谁置顶就变成"数组里碰巧谁在前"——不可断言、也不可复现。
    public var severity: Int {
        switch self {
        case .networkUnreachable: return 60
        case .dnsFailed: return 59
        case .serverRefused: return 50
        case .diskSlow: return 40
        case .diskFull: return 30
        case .engineUnresponsive: return 20
        case .engineCrashed: return 19
        case .verifyMismatch: return 18
        }
    }

    /// 卡片标题。中文逐字照 `error_card.rs`；英文照它的英文列。
    public func title(_ lang: Lang) -> String {
        switch (self, lang) {
        case (.networkUnreachable, .zh): return "连不上服务器"
        case (.networkUnreachable, .en): return "Cannot reach the server"
        case (.dnsFailed, .zh): return "域名解析失败"
        case (.dnsFailed, .en): return "Domain lookup failed"
        case (.serverRefused, .zh): return "服务器拒绝了这次请求"
        case (.serverRefused, .en): return "The server refused this request"
        case (.diskSlow, .zh): return "目标磁盘写得太慢"
        case (.diskSlow, .en): return "The target disk is too slow"
        case (.diskFull, .zh): return "目标磁盘空间不足或不可写"
        case (.diskFull, .en): return "The target disk is full or not writable"
        case (.engineUnresponsive, .zh): return "下载引擎没有响应"
        case (.engineUnresponsive, .en): return "The download engine stopped answering"
        case (.engineCrashed, .zh): return "下载引擎启动失败或已退出"
        case (.engineCrashed, .en): return "The download engine failed to start or exited"
        case (.verifyMismatch, .zh): return "文件校验没有通过"
        case (.verifyMismatch, .en): return "File verification failed"
        }
    }

    /// 卡片解释（"为什么"）。中文逐字照 `error_card.rs`。
    public func explain(_ lang: Lang) -> String {
        switch (self, lang) {
        case (.networkUnreachable, .zh):
            return "这次没连上服务器：网络断了、被防火墙拦了，或对端暂时不可用。"
        case (.networkUnreachable, .en):
            return "This request did not reach the server: the network may be down, blocked, or the host is unavailable."
        case (.dnsFailed, .zh):
            return "域名没解析出地址：DNS 配错了，或这台机器的网络还没就绪。"
        case (.dnsFailed, .en):
            return "The domain did not resolve: DNS may be misconfigured or the network is not ready yet."
        case (.serverRefused, .zh):
            return "服务器回了一个拒绝：交付码可能不对、链接过期了，或那一侧限流。"
        case (.serverRefused, .en):
            return "The server refused: the code may be wrong, the link expired, or it is rate limiting."
        case (.diskSlow, .zh):
            return "目标磁盘写得比下载慢：下载会跟着慢，长时间写不动还会看起来像卡住。"
        case (.diskSlow, .en):
            return "The target disk writes slower than the download: transfers slow down and may look stuck."
        case (.diskFull, .zh):
            return "目标磁盘放不下这次的数据，或者没有写权限。"
        case (.diskFull, .en):
            return "The target disk cannot hold this data, or it is not writable."
        case (.engineUnresponsive, .zh):
            return "下载引擎连续几次没有回应，自动重启的预算也用完了。"
        case (.engineUnresponsive, .en):
            return "The engine did not answer several times in a row and the automatic restart budget is used up."
        case (.engineCrashed, .zh):
            return "下载引擎没能起来，或者起来之后退出了。"
        case (.engineCrashed, .en):
            return "The engine could not start, or it exited after starting."
        case (.verifyMismatch, .zh):
            return "下载完的内容与清单里的校验值对不上。"
        case (.verifyMismatch, .en):
            return "The downloaded content does not match the checksum in the file list."
        }
    }

    /// 卡片动作（"下一步做什么"）。中文逐字照 `error_card.rs`。
    public func action(_ lang: Lang) -> String {
        switch (self, lang) {
        case (.networkUnreachable, .zh), (.dnsFailed, .zh):
            return "重试；还不行就换一个网络。"
        case (.networkUnreachable, .en), (.dnsFailed, .en):
            return "Retry; if it persists, switch networks."
        case (.serverRefused, .zh): return "核对交付码，稍后再重试。"
        case (.serverRefused, .en): return "Check the code and retry later."
        case (.diskSlow, .zh): return "换到内置盘，或把「并行文件数」调到 1–2。"
        case (.diskSlow, .en): return "Use the internal disk, or set parallel files to 1-2."
        case (.diskFull, .zh): return "换一个下载目录，或清理出空间。"
        case (.diskFull, .en): return "Pick another folder, or free up space."
        case (.engineUnresponsive, .zh), (.engineCrashed, .zh):
            return "重启客户端；仍然失败就导出诊断日志。"
        case (.engineUnresponsive, .en), (.engineCrashed, .en):
            return "Restart the app; if it still fails, export diagnostics."
        case (.verifyMismatch, .zh): return "重新下载这个文件。"
        case (.verifyMismatch, .en): return "Download this file again."
        }
    }
}

/// 一张要上屏的卡片（**句子已经按语言成好**，视图只摆位置）。
public struct ErrorCard: Equatable, Sendable {
    public let errorClass: ErrorClass
    public let title: String
    public let explain: String
    public let action: String
    public let severity: Int
    /// 同类在 30 秒窗口里出现过几次（1 = 只出现一次）。
    public let count: Int

    /// 一张卡。`count > 1` 时**标题里带上次数**（规格 W2 的"30 秒内合并"要让客户看得见，
    /// 否则他会以为只错了一次）。
    ///
    /// ⚠️ 次数那句话**在这里拼**（同 Windows 的 `ErrorCard::of`），视图不拼字符串；
    ///    而 `explain` / `action` 里**永远**不出现次数（`ErrorCardTests` 钉着"只在标题里出现一次"）。
    public static func of(_ errorClass: ErrorClass, lang: Lang, count: Int = 1) -> ErrorCard {
        let n = max(1, count)
        let title: String
        if n > 1 {
            switch lang {
            case .zh: title = "\(errorClass.title(lang))（\(n) 次）"
            case .en: title = "\(errorClass.title(lang)) (x\(n))"
            }
        } else {
            title = errorClass.title(lang)
        }
        return ErrorCard(errorClass: errorClass,
                         title: title,
                         explain: errorClass.explain(lang),
                         action: errorClass.action(lang),
                         severity: errorClass.severity,
                         count: n)
    }

    /// 带 `class:` 标签的同义写法（规格 W2 的签名 `ErrorCard.of(class:lang:count:)`；
    /// 线上键名就叫 `class`，调用点读起来与内核那侧对齐）。
    public static func of(class errorClass: ErrorClass,
                          lang: Lang,
                          count: Int = 1) -> ErrorCard {
        of(errorClass, lang: lang, count: count)
    }
}

/// 一条"这一类错了 N 次"的观察（内核 `error_recent` 的 `entries` 里那两格）。
///
/// 独立于协议解码类型：`Presentation/` 这层只认**语义**，不认线上键名
/// （`AppModel` 负责把 `ErrorRecentEntry` 映射成它）。
public struct ErrorObservation: Equatable, Sendable {
    public let errorClass: ErrorClass
    public let count: Int

    public init(errorClass: ErrorClass, count: Int) {
        self.errorClass = errorClass
        self.count = count
    }
}

/// 观察到的一批错误 → 界面该摆的那几张卡（规格 W2 / Windows `CardBook::cards` 的语义）。
public enum ErrorCards {
    /// 同时上屏的卡片上限（再多就把客户淹了；严重度最高的那几张才是要看的）。
    public static let maxOnScreen = 3

    /// 同类合并（取最大计数）、按严重度降序、最多 [`maxOnScreen`] 张。
    ///
    /// ⚠️ 内核的 `error_recent` 已经合并过一遍（30 秒窗口、同类只算一条带次数）；
    ///    这里再做一次是**防御性**的：同一类万一以两条观察进来（协议演进、错误重放），
    ///    界面也不该冒出两张一模一样的卡。合并取 `max` 而不是相加 —— 两条观察
    ///    说的是"同一段时间里的同一类"，相加会把次数说多。
    public static func from(_ observations: [ErrorObservation], lang: Lang) -> [ErrorCard] {
        var counts: [ErrorClass: Int] = [:]
        var order: [ErrorClass] = []
        for o in observations {
            if counts[o.errorClass] == nil { order.append(o.errorClass) }
            counts[o.errorClass] = max(counts[o.errorClass] ?? 1, max(1, o.count))
        }
        var cards = order.map { ErrorCard.of($0, lang: lang, count: counts[$0] ?? 1) }
        // 严重度降序；两两不同（`severity` 的文档），所以没有"次序靠数组"的歧义。
        cards.sort { $0.severity > $1.severity }
        if cards.count > maxOnScreen {
            cards.removeLast(cards.count - maxOnScreen)
        }
        return cards
    }
}
