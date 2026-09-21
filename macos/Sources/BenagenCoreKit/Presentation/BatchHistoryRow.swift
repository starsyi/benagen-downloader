import Foundation

// ---------------------------------------------------------------------------
// 换码面板里那个历史列表的**行呈现模型**（阶段 E 规格 §1.4）
//
// 全局约束 8 / E-7：视图里不许有非渲染逻辑，所以"这一行画出来是什么字"在这里定死；
// `Sources/BenagenDownloader/Views/SwitchDeliverySheet.swift` 只做绑定与渲染分派，
// 它**一个字符串都不拼、一个判断都不做**。
//
// 本文件**不做**下面这几件事（各有其归属，重复一次就是同一条规则写两遍）：
//   · 排序 / 去重 / 淘汰 —— `BatchHistory` 的不变量，行层**原样沿用它的顺序**
//     （`rows(_:)` 只做映射，不重排：视图侧一旦自己 `sorted` 一次，第二处消费
//     `entries` 的地方就得再排一遍，两处迟早对不上）；
//   · 备注的归一化 —— `BatchHistory.normalizedNote` 负责"写进去的那一份"，
//     这里只做**显示口径**上的兜底（见 `init`）；
//   · 时间格式化 —— 复用既有的 [`TimestampPresentation`]（规格 §1.1 明写），
//     本文件不另造一套（`theHistoryTimeColumnReusesTheSharedTimestampPresentation` 钉着它）。
// ---------------------------------------------------------------------------

/// 历史列表里的一行：**已经算好的界面值**。
///
/// 一行在屏上由三样东西组成（规格 §1.4）：
///   ① `title` —— 备注（有则显示，无则显示码）；
///   ② `code` —— 交付码本身（永远显示，它是这一行的身份）；
///   ③ `timeText` —— 上次使用时间。
/// 另有两样是**行为**要用的、但不上屏的值：`baseURLOrNil`（点这一行要去问哪台服务器）
/// 与 `isSendable`（这一行能不能发出去）。
public struct BatchHistoryRow: Equatable, Sendable, Identifiable {
    /// 交付码。**逐字**（约束 3：不截断、不加省略号 —— 它是用户唯一能拿去对账的东西）。
    public let code: String
    /// 备注原文（去掉首尾空白后）。**空串 = 没写备注**，也是编辑框的初值。
    public let note: String
    /// 行首那一句：备注非空就是备注，否则回落到**码**（规格 §1.4）。
    ///
    /// ⚠️ 回落是**这一行的全部意义**：没有它，一条没写备注的历史在列表里就是一行空白
    ///    （约束 4 明禁的静默失效 —— 用户会以为它坏了，而不是以为"我还没写备注"）。
    public let title: String
    /// 「上次使用」那一格。口径见 [`timeText(of:)`]。
    public let timeText: String
    /// 这一条是从哪个交付服务器加载的（空串 = 默认服务器）。
    public let baseURL: String

    /// 码是唯一键（`BatchHistory` 的②号不变量）⇒ 它就是这一行的身份。
    public var id: String { code }

    /// 点这一行时要发给内核的 `base_url`：**空串 ⇒ `nil`**。
    ///
    /// ⚠️ 这一条是**承重的**（阶段 C 的 Ruling C10 记下的那个坑，本阶段由历史来填）：
    ///    从自定义交付服务器加载过的批次，如果再点一次却去问**默认**服务器，就会拿到
    ///    "码不存在"之类的失败。`nil` = 请求里**不出现** `base_url` 这个键 ⇒ 内核用
    ///    它自己的默认交付服务器 —— 与 E-5 同一条纪律（不要显式传一个"和默认一样"的值，
    ///    那会在内核默认值变化时静默分叉）。
    public var baseURLOrNil: String? { baseURL.isEmpty ? nil : baseURL }

    /// 这一行能不能发出去。
    ///
    /// ⚠️ 为什么要有它：`history.json` 是**用户看得见、也改得动**的文件，而
    ///    `BatchHistory.parse` 只要求 `code` 非空。一条超过 2 KiB 的码点下去，内核
    ///    **不报错** —— 它只回一条 `id == 0` 的协议告警、那条请求**永远等不到响应**，
    ///    `CoreClient` 那条 FIFO 串行队列于是被**永久堵死**（约束 C-3 / 15，
    ///    与 `DeliveryCodeEntry` 上那段注释同一个后果）。所以判据**复用**那一个实现，
    ///    绝不在这里另写一套长度判断。
    public var isSendable: Bool { DeliveryCodeEntry.isSendable(code) }

    /// 一条历史条目 → 一行。
    public init(_ entry: BatchHistoryEntry) {
        // ⚠️ **只吃空白字符的备注与空串同等对待**（E-8：这是一个有意的显示口径选择）：
        //    `"   "` 渲染出来是一行**看起来是空的**标题，在界面上与"这一行坏了"分不开
        //    （约束 4）。壳自己写出去的备注全是归一化过的（`BatchHistory.settingNote` /
        //    `recording(note:)`），所以这条兜底只对手改过的文件生效 —— 但那一份同样是
        //    真实的输入（`parse` 不会因为备注是空白就丢掉这一行）。
        //    内部原有的空格**一个都不动**：那是用户自己写的排版（同 `normalizedNote`）。
        let trimmed = entry.note.trimmingCharacters(in: .whitespacesAndNewlines)
        self.code = entry.code
        self.note = trimmed
        // 备注空 ⇒ 回落到码；**码也空** ⇒ 占位符（纵深防御，见下面 `noValue` 的注释）。
        self.title = trimmed.isEmpty ? (entry.code.isEmpty ? Self.noValue : entry.code) : trimmed
        self.timeText = Self.timeText(of: entry.lastUsedAt)
        self.baseURL = entry.baseURL
    }

    /// 一整份历史 → 屏上从上到下的那些行。
    ///
    /// ⚠️ **只做映射，不排序也不截断**：顺序是 `BatchHistory` 的不变量（按 `last_used_at`
    ///    倒序、同码唯一、不超上限），这里再排一次就等于把同一条规则写第二遍。
    ///    返回**空数组**时视图整段不渲染（规格 §1.4：列表为空时不显示这一段，不要给空盒子）。
    public static func rows(_ history: BatchHistory) -> [BatchHistoryRow] {
        history.entries.map(BatchHistoryRow.init)
    }

    // MARK: - 时间列

    /// 「上次使用」那一格显示什么。
    ///
    /// 输入是**原文**（ISO 8601 带 `+08:00`，与清单的 `created_at` 同形），两种情形：
    ///   · 空串 ⇒ 占位符 `—`（口径同 `SourceTimeText.noValue` / `DeliverySummary.noValue`：
    ///     "这一格没有值"本身也是一件要说出来的事，渲染成空串就是什么都没说）；
    ///   · 其余 ⇒ **原样**交给 [`TimestampPresentation.text`]：它能认的就显示
    ///     `2026-09-18 09:12`，认不出来的**原样返回** ——
    ///     所以壳**永远不会**显示 "Invalid Date" 这种自己编的文案（约束 C-7 / 3）。
    ///
    /// ⚠️ **复用 `SourceTimeText.of`，不在这里再抄一遍**（E-8：这条写的是"为什么是这个"，
    ///    而不是"为什么不复用"）："空/缺 ⇒ `—`，否则原样交给 [`TimestampPresentation`]"
    ///    这条规则在本仓库里已经有三个落点（`SourceTimeText.of`、`DeliverySummary` 的
    ///    `validityText`、以及本文件），抄第四遍就是把同一个答案写第四份。
    ///    抽一个共享类型的正确落点是 `Format.swift`（那才是"呈现基础件"的家），
    ///    但那要同时改三处调用点 —— 那是**独立的清理**，不属于本任务。
    ///    代价：这里依赖了一个"名字像文件浏览器"的 internal 类型（同模块够得着）；
    ///    换来的是**规则只有一份**（复审第 2 条）。
    static func timeText(of raw: String) -> String {
        SourceTimeText.of(raw)
    }

    /// 这一格没有值时的占位字形。
    ///
    /// ⚠️ 与 `Format.swift` 的 `SpeedFormat`/`PercentFormat`、`DeliverySummary.noValue`、
    ///    `SourceTimeText.noValue` 是**同一个字形**（`—`）：同一件事在四个地方长得一样，
    ///    用户才不用每次重新学一遍"这一格空着是什么意思"。
    ///
    /// ⚠️ 它现在**只**用在"码与备注都空"那个纵深防御的分支上（`init` 里）：
    ///    正路上 `BatchHistory.init(entries:)` 会把空码丢掉，所以那一行造不出来；
    ///    但 `BatchHistoryRow.init` 是 `public`，"标题是空串"这一行**在类型上构造得出来**
    ///    —— 而一行空标题在界面上与"这一行坏了"分不开（约束 4）。
    ///    兜底口径与 `DeliverySummary.swift:39` 的 `code` 那一格**同源**。
    static let noValue = "—"
}
