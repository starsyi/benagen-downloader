import Foundation
// ⚠️ **部署目标 13.0 起**：`@Observable`（Observation 框架）是 macOS 14+ 才有的，
//    而客户机是 13.7.8 ⇒ 这里改回 Combine 的 `ObservableObject`。
//    ⚠️ Combine **也是系统框架**（不是外部依赖）：阶段 B 约束 9 的允许清单原本只点了
//    SwiftUI / AppKit / Foundation / Observation，这一行是那次约束的一次修订 ——
//    理由与日期见 `macos/README.md` 的「部署目标」那一段。
import Combine

// ---------------------------------------------------------------------------
// AppModel —— 壳的状态中枢
// ---------------------------------------------------------------------------
//
// 全局约束 1：壳不含业务逻辑。这个类只做三件事：
//   ① 把内核的响应**搬进**可观察的状态（`loadState` / `tree` / `transfers` / …）；
//   ② 按内核给的**结构化错误码**（不是 `message` 的措辞）决定这些状态怎么变
//      —— 逐条对着设计规格附录 A 的处置列；
//   ③ 记住「当前在忙什么」（`busyReason`）。
//
// 它**不**判断哪些文件该下载、不自己展开目录、不替内核重试下载、不自己编错误文案
// （唯一的例外是 `protocol_mismatch`，见 `absorb`）。
//
// ⚠️ **所有要发内核请求的方法都是 `async`，内部走 `callAsync`**（裁定 ①）。
//    `CoreClient.callSync` 是**阻塞**的（内部是一个无超时的 `read(2)`），而
//    `load_delivery` 实测约 91.5 秒 —— 在主 actor 上同步调它，界面会被冻住整个时长，
//    而那恰好是规格 §7 与全局约束 15 要防的那件事。唯一的同步例外是 `shutdown()`：
//    它有界，且由 `applicationWillTerminate` 调用，那里没有 await 的余地。

/// `LoadState` 要 `Equatable`（简报要求的形状），而 `DeliveryInfo` 的成员
/// （String / Bool / Int64 / `TreeNode`）本来就都是 Equatable —— 合成实现在
/// `Protocol.swift` 里一并声明了。
@MainActor
public final class AppModel: ObservableObject {
    /// 交付清单的加载状态。
    public enum LoadState: Equatable {
        case idle
        case loading
        case loaded(DeliveryInfo)
        /// 失败原文 —— **内核怎么说就怎么写**，壳不加工。
        case failed(String)
    }

    /// `performLoadDelivery` 的两种模式（换交付码是本任务引入的第二种）。
    ///
    /// - `.initial`：**今天的行为** —— 失败的落点是 `loadState = .failed(...)`（空态页显示原文与「重试」）。
    /// - `.switching`：**成功才提交** —— 失败**一个状态都不写**，把错误抛给换码面板去显示。
    ///   理由见 `performLoadDelivery` 与 `switchDelivery` 的注释：换码是"在当前批次之上再加载一批"，
    ///   它的失败不该毁掉当前批次。
    public enum LoadMode: Sendable {
        case initial
        case switching
    }

    /// 下载引擎（aria2）的状态。`.unknown` = 还没问过内核（启动完成前）。
    public enum EngineState: Equatable {
        case unknown
        /// 引擎尚未启动。**这是正常态**：没 enqueue 过就没引擎。
        case notStarted
        case running
        /// 引擎/内核不可用，括号里是内核给的原文或壳给的一句明确原因。
        case unavailable(String)
    }

    // MARK: - 可观察状态

    // -------------------------------------------------------------------------
    // 🔴 纪律：**视图会读的可变状态，唯一的标记就是 `@Published`**
    //
    // 下面这一组是"视图会读"的那一批（`@Published`）；再往下那些私有旗标是"只有这个类
    // 自己读"的（普通存储属性，不发布）。**加新状态时忘了标 `@Published`，编译器不会报错**
    // —— 症状是"那一格永远不刷新"，而它在类型上、在测试里都看不出来。
    // 这条纪律是 2026-09-20 换掉 `@Observable` 时立的：`@Observable` 会自动追踪每个属性的
    // 读取，`ObservableObject` 不会 —— 它只认 `@Published`。
    //
    // ⚠️ 另一半同样承重：`@Published` 的失效粒度**比 `@Observable` 粗**
    // （任何一个发布属性变了 ⇒ 所有订阅者重算 body），所以下面三处写护栏
    // （`lastError` / `protocolAlerts` / `engine`）不是优化，是**把语义改回原状**。
    // -------------------------------------------------------------------------
    @Published public private(set) var loadState: LoadState = .idle
    @Published public private(set) var engine: EngineState = .unknown
    /// `get_tree` 的快照 —— 任务 6 的默认选择与任务 9 的总进度都读它。
    @Published public private(set) var tree: TreeResult?
    /// **`tree` 属于哪一批**（`nil` = 手上没有属于任何一批的树）。
    ///
    /// `TreeResult` 自己没有交付码字段，所以"这棵树是不是当前这批的"只能由壳记着。
    /// 它挡的是一条真实的竞速：`loadState = .loaded` 在**拉树之前**就落了，
    /// 视图在那一刻出现并可能读到 `tree == nil` 或**上一批的** `tree` —— 两者都会让
    /// 默认选中面播错（见 `BrowserSelection.onNewManifest` 的注释）。
    @Published public private(set) var treeCode: String?
    /// **成功加载了几次交付清单**（`loadGeneration`）——每次 `load_delivery` **成功**就 +1。
    ///
    /// ⚠️ **它存在的唯一理由是"同码重载在别处看不出来"**（阶段 D 任务 B）。同码重载是用户
    ///    点得到、也真的会发生的一条路（换码面板里重新提交同一个码 / 空态页那颗「重试」/
    ///    内核崩溃后的恢复），而它在壳已经观察的**全部**状态上一个字都不变：
    ///    `loadState` 还是 `.loaded(同一份 info)`、`loadedCode` 还是那个码、`treeCode`
    ///    还是那个码。视图据此**分不出**"刚刚重载过"与"什么都没发生"，于是
    ///    `RootView` 的 `.onChange(of: loadedCode)` 不触发、那一批的默认勾选面也不重播
    ///    —— 用户看到的是"我重载了，界面一点变化都没有"（诊断报告 §6.1 的两条猜想，
    ///    都已用测试证实）。
    ///
    /// ⚠️ **只在整次加载（含 `getTree()`）成功之后才推进**。三条都是硬要求：
    ///      - **失败的加载不推进** ⇒ 屏幕上那条下载回执里的拒绝理由**留着**（约束 4：
    ///        不许自己消失；`RootView` 正是拿这个计数器的变化去清它的）；
    ///      - **`getTree()` 也必须成功**（复审重要 1）⇒ 这一条**不变量**才成立：
    ///        **代数一变，`tree` / `treeCode` 就是这一代的**。少了这道闸，会出现
    ///        "代数推了、`treeCode` 还是同一个码、`tree` 还是上一棵树"的中间态 ——
    ///        而 `ManifestTracking.seed` 唯一的新鲜度守卫 `treeCode == code` 对
    ///        "**同码但过期**"的树**没有判别力** ⇒ 视图拿上一棵树重播默认勾选面。
    ///        （`getTree` 失败时 `surfacing` 吞错返回 nil、且旧快照原样留着，所以这条
    ///         闸不能靠"它自己会作废"来代替 —— 实测构造得出这个现场。）
    ///      - 壳自己造的那些"拒发"早退（引擎不可用 / 交付码过长 / 体量守卫）**不经过这里**
    ///        ⇒ 它们什么都不会作废，与改动前逐字相同。
    ///
    /// ⚠️ **一处的代价**：`getTree()` 失败的那次加载不算一代 ⇒ 那条过期的下载回执
    ///    **不会**被清掉。那两条路上界面本来就**不是静默的**（失败必落成顶部横幅：
    ///    `transport` 走 `onKernelDeath` 的引擎横幅，其余错误码走 `lastError`），
    ///    用户看到的是"引擎出问题了"，而不是"旧结论还在冒充现状"。
    ///
    /// ⚠️ 它**不是**"当前批次是哪个"的来源（那是 `loadState` / `loadedCode`），
    ///    也不是单调递增的世代号意义上的"版本"—— 它只是"这一批被内核重新规划过几次"。
    @Published public private(set) var loadGeneration: Int = 0
    @Published public private(set) var transfers: TransferListResult?
    /// `verify_status` 的快照。
    ///
    /// ⚠️ 它是**按批次**的，而且"这份结果属于哪一批"只有壳记着（`VerifyStatus` 自己没有
    ///    交付码字段）—— 记法就是 `loadedCode`：**换批时 `performLoadDelivery` 会把它清成
    ///    `nil`**（`resetToEmptyState` 那条路同样清）。旁路它就意味着侧边栏徽标与整个校验屏
    ///    显示上一批的数字（任务 9 复审的重要 ①）。
    ///    同一个码重复 load 是**刷新**、不清（详见 `performLoadDelivery` 里那段注释）。
    @Published public private(set) var verify: VerifyStatus?
    @Published public private(set) var settings: Settings?
    /// 上次用过的交付码（**内核**单独记着的那一个，不是"用户这次输入的"）。
    ///
    /// ⚠️ **阶段 E 起它不是"上次用的码"的权威**（E-4）：权威是 [`history`]。
    ///    这个字段仍然从握手（`get_settings.last_code`）里读进来，但**只在壳的历史
    ///    为空时**当一次性迁移的种子用（见 `rememberedCode()`）—— 之后壳不再读它做决策。
    ///    继续留着它是因为"内核记着哪个码"仍然是一个可观察的事实（换设置的回执里也带它），
    ///    而不是因为有人拿它做判断。
    @Published public private(set) var lastCode: String = ""
    /// **壳的批次历史** —— 「上次用的码」的**唯一权威**（阶段 E 规格 §1.3 / E-4）。
    ///
    /// ⚠️ 顺序、去重、上限都由 `BatchHistory` 自己维持（它是不变量），
    ///    所以换码面板只要从上到下画 `entries` 就行（任务 2 消费它）。
    /// ⚠️ `private(set)`：**只有** `recordLoadedBatch`（成功加载）与
    ///    `rememberedCode`（一次性播种）能改它 —— 视图改历史必须走那两条路，
    ///    否则"盘上那份"与"内存里那份"会分叉。
    @Published public private(set) var history: BatchHistory = .empty
    /// **历史写盘失败**的原文（`nil` = 没出问题）。
    ///
    /// ⚠️ 为什么不落 `lastError`：那条通道的契约是「**内核原文**照登」（约束 3），
    ///    而写盘失败是**壳自己的事**、内核一个字都没说（E-8：偏离的理由写在这里）。
    ///    混进 `lastError` 还会被"下一次成功的内核请求"清掉 —— 那是一条只对内核错误成立的规则。
    /// ⚠️ 它**不阻塞任何东西**：一次**成功**的加载不会因为"历史存不下来"被判成失败。
    ///
    /// ⚠️ **落点**（最终审查重要 1 补的；在这之前这条注释承诺的东西**不存在** ——
    ///    全仓没有任何视图读它，症状是"敲完备注、写盘失败、应用一个字都不说，
    ///    重启后备注不见了且无从归因"）：`RootView` 主区那行常驻提示 +
    ///    换码面板的历史那一段，**两处读的是同一个字段**；收起走
    ///    [`dismissHistoryWriteFailure()`]。写盘的三条路（播种 / 成功加载 / 改备注）
    ///    全都落在这一处，所以面板没开时（自动加载那两条）也看得见。
    @Published public private(set) var historyWriteFailure: String?
    /// 用户选的下载目录。**空串 = 未配置**（E-5：壳**不传** `--download-dir`，
    /// 由内核用自己的默认值）。
    ///
    /// ⚠️ **它是"此刻生效的是什么"的唯一来源**：内核的 argv 在**启动时**就定死了
    ///    （`--download-dir` 是 argv，不是协议消息），所以这个值改动的**唯一**合法路径是
    ///    [`changeDownloadDir(to:)`]——它会**重启内核**，让新 argv 生效。
    ///    直接改这个字段（如果将来有人把它变成 `var`）只会造成"界面显示新目录、
    ///    内核还在往旧的写"的分裂。
    @Published public private(set) var downloadDir: String = ""
    /// 最近一次**改下载目录**的结果（`nil` = 还没改过，或用户已经收起了那一行）。
    ///
    /// ⚠️ **它存在的唯一理由与 `lastSwitchOutcome` 逐字相同：那句话必须活过那扇窗**
    ///    （最终审查重要 2）。设置窗口是天然的、随手就会被关掉的东西 —— 回执原先是一个
    ///    `@State`（`SettingsView`），**窗口一关它就没了**。而这里有一格回执恰恰是
    ///    "**没改成**"：`restartKernel()` 被一次在飞的重启挡住（返回 `false`）时，
    ///    **内存与盘上都是新目录、内核在用旧目录跑** —— 正是这个功能要消灭的那种分裂，
    ///    而那句话是**唯一**的提示。少了这个落点，用户会在整个会话里以为文件下到新目录
    ///    去了（设置里显示的是**新**目录、引擎横幅是绿的、没有任何东西再说这件事）。
    ///
    /// ⚠️ **落点有两处、同一个值**（`RootView` 那行常驻提示 + 设置窗口那一段），
    ///    与 `lastSwitchOutcome` 同款：两个落点读的是同一个字段，不会分叉。
    ///    收起走 [`dismissDownloadDirChange()`]（两处那颗 `×` 都调它）。
    ///
    /// ⚠️ 它**不是**"当前用的目录是哪个"的来源 —— 那是 [`downloadDir`]。
    ///    这个字段是**一条已经发生过的动作的回执**，与 `downloadNotice` 同一性质。
    @Published public private(set) var downloadDirChange: DownloadDirChange?
    /// `-k` 的枚举面。**来自 `hello`**，不是壳里的硬编码（契约 §2.3）。
    @Published public private(set) var minSplitSizeChoices: [String] = []
    /// 内核回的协议级错误（`id == 0`）。它们是**当前连接**的产物：内核崩溃重启后
    /// 从新的空列表重新累积（旧内核那几条早就显示过了）。
    @Published public private(set) var protocolAlerts: [ErrorBody] = []
    /// 非 nil 时界面显示「进行中」。
    @Published public private(set) var busyReason: String?
    /// 正在换交付码（`switchDelivery` 在跑）。换码面板据此禁用「加载」与输入框 ——
    /// 一次只发一条（约束 15 的单飞语义在 `AppModel` 这侧，这里只是别让用户连点上十次）。
    /// 「取消」**不**跟着禁用：面板关不关得掉，不该由一次网络请求决定。
    ///
    /// ⚠️ 它**不是**「有没有任务在跑」的判据（那是内核的事，壳只知道 `transfers`，
    ///    而那个快照只在用户进过传输列表之后才有值）。
    @Published public private(set) var switching: Bool = false
    /// 最近一次**不抛错路径**（轮询 / 校验刷新 / 握手）上的内核错误原文。
    ///
    /// ⚠️ 它是这些路径上「其余错误码 → 原文呈现」的落点（附录 A 的处置列）。
    /// **它不改 `engine`** —— `pollTick` 的准入是 `guard case .running = engine`，
    /// 让一次瞬时的轮询失败（`engine_rpc_failed` / 一行垃圾）去写 `engine`，等于
    /// 自己把轮询永久关掉，而自愈恰恰需要下一次成功的 `transfer_list`。
    /// 下一次**成功**的请求会把它清掉：问题还在就会在下一拍重新写上，问题过去了就不该
    /// 挂着一条旧错误吓人。抛错路径（`listDir`/`enqueue`/…）由调用方在原地显示，不落这里。
    @Published public private(set) var lastError: String?

    /// 最近一次**换码**的结果（`nil` = 还没有换过码，或用户已经收起了那一行）。
    ///
    /// ⚠️ **它存在的唯一理由是"结果必须活过面板"**（任务 2 复审重要 ②/③）：
    ///    换码面板是模态的，而它的「取消」按钮**不被禁用** —— 面板一关，面板里那个
    ///    `@State` 就跟着没了。失败方向上是"内核原文零痕迹"（约束 4 明禁的静默失效），
    ///    成功方向上是"我按了取消、界面自己换了一批"（用户无法归因）。
    ///    落在模型上之后，`RootView` 那行常驻提示负责呈现，它是**可断言的**
    ///    （见 `theFailureOutcomeSurvivesTheSheet` / `theSuccessOutcomeSurvivesTheSheet`）。
    ///
    /// ⚠️ 它**不是**"当前批次是哪个"的来源 —— 那是 `loadState` / `loadedCode`。
    ///    这个字段是**一条已经发生过的动作的回执**，与 `downloadNotice` 同一性质：
    ///    换批**不清**它（约束 4：刚说过的话不能被另一次动作抹掉），由用户点收起或
    ///    下一次换码开始把它清掉。
    ///
    /// ⚠️ 与 `lastError` 的分工：`lastError` 是**轮询/校验那些没有调用方可以 catch 的
    ///    路径**的出口；换码有调用方（面板），它的回执走这条专用通道 —— 混在一起会让
    ///    "下一拍成功就自己消失"（`EngineBanner.transientError`）把一句换码的原文也吞掉。
    @Published public private(set) var lastSwitchOutcome: DeliverySwitch?

    // MARK: - 握手超时（任务 4b）

    /// 生产默认的握手超时（秒）。**与 `handshakeTimeoutMessage` 里那句「5 秒」必须一致** ——
    /// 改一个就改另一个（`theTimeoutMessageNamesThePermissionDialog` 钉着这对关系）。
    ///
    /// ⚠️ `nonisolated`：`AppModel` 是 `@MainActor`，类里的 `static let` 默认也带上那个隔离，
    ///    于是 ① 它不能当 `init` 的默认参数（默认参数在**调用方**的上下文里求值）、
    ///    ② `Presentation/` 那层（无隔离的纯函数）碰不到它。这两个都是常量，本来也不需要隔离。
    public nonisolated static let defaultHandshakeTimeout: Double = 5

    /// 握手超时后装进 `engine` 的那句话。**它是壳自己写的**，理由见 `start()` 里那段注释。
    ///
    /// ⚠️ 别把它读成"壳只在这一处写文案"——那是**错的**，而且一 grep 就能证伪：
    ///    `absorb` 的 `.protocolMismatch` 分支（"客户端与内核版本不匹配…"）、
    ///    `onKernelDeath`（"下载引擎已断开：…"）、`restartKernel`（"内核重启失败：…"）
    ///    同样是壳写的话。说"唯一"会把约束 11 的正当性论证（"这是有意偏离，所以要注明理由"）架空，
    ///    所以这里只陈述**这一处**的理由：超时是**壳探测到的**状况，内核一个 message 都没给，
    ///    没有"原文"可登。
    ///
    /// ⚠️ 这个字符串同时是一个**判别式**：`EngineStatusPresentation` 靠它把"超时"与
    ///    "内核给的失败原因"分开（前者要说"该去点授权框"，后者要原文照登）。
    ///    所以它必须**逐字固定**——不要往里插值（例如把注入的超时值拼进去），
    ///    否则测试里的 50 ms 超时会得到一句话、生产里的 5 秒得到另一句话，
    ///    而"用户看到的文案只有一句"这件事就没了。
    ///
    /// ⚠️ `nonisolated`：同 `defaultHandshakeTimeout` —— `Presentation/` 那层是无隔离的纯函数，
    ///    要拿它做判别（见 `EngineStatusPresentation.isHandshakeTimeout`）。
    public nonisolated static let handshakeTimeoutMessage = "内核无响应（等待超过 5 秒）"

    // MARK: - 注入（不参与观察）

    private var client: (any CoreCalling)?
    /// 重启内核的唯一入口。生产 = `CoreClient.live(settingsPath:downloadDir:)`；测试 = 再造一个替身。
    ///
    /// ⚠️ **参数是 `--download-dir` 的值**（`nil` = **不传那个 flag**，E-5），不是"下载目录"
    ///    这个业务概念：内核要的就是 argv 上那一格，壳把"该不该传"的判断留给
    ///    `DownloadDirectory.argument(for:)` 一处（有单测）。
    ///
    /// ⚠️ **它是重启与首启共用的同一个闭包**（`spawnClient()` 与 `live()` 各调一次），
    ///    所以"改目录"只要让这个闭包在**下一次**被调用时拿到新值就够了 ——
    ///    两个调用点各改一次的实现方式迟早会分叉。
    private let makeClient: @Sendable (String?) throws -> any CoreCalling

    /// 握手的**有界**等待（秒）。生产 = [`defaultHandshakeTimeout`]（5 秒）；
    /// 测试注入一个小值，否则要么真等 5 秒、要么根本测不了。
    private let handshakeTimeout: Double

    /// 「上一拍还没回来」的标志。**跳拍语义的落点**（全局约束 15，见 `pollTick`）。
    private var pollInFlight = false

    /// 已经呈现过的协议级告警**条数**（去重记账，见 `syncProtocolAlerts`）——
    /// 只用来判断"哪几条是新出现的"，不是可观察状态的一部分。
    private var surfacedAlerts = 0

    /// 自动重启的**防重入**标志。语义与复位时机见 `restartIfAllowed`。
    private var restartAttempted = false
    /// 「正在重启中」——比 `restartAttempted` 更细：同一时刻只跑一次重启。
    private var restarting = false

    /// 「应用级启动跑过了」的**一次性闸门**。语义与理由见 `bootstrap()`。
    private var didBootstrap = false

    /// 壳的持久化（历史文件）。`nil` = **没有持久化接缝**（测试替身的默认）。
    ///
    /// ⚠️ **生产形态由 `AppModel.live()` 给**（那里是唯一的构造入口）。
    ///    测试接缝默认 `nil` 是**刻意的、也是必须的**：默认值要是 `BatchHistoryStore()`
    ///    （指向人类伙伴真实的 `~/Library/Application Support/BenagenDownloader/`），
    ///    那么**每一条**"加载成功"的既有测试都会往他真实的历史文件里写东西 ——
    ///    那是破坏现场，不是测试。要测持久化就显式注入一个指向临时目录的 store
    ///    （见 `AppModelHistoryTests`）。
    /// ⚠️ 没有 store 时历史**仍然在内存里维护**（可观察状态一致），只是不落盘。
    private let historyStore: BatchHistoryStore?

    /// 「历史已经从盘上读过一次了」的闸门。**语义与理由见 `loadHistoryIfNeeded()`。**
    private var historyLoaded = false

    /// 壳的偏好（下载目录）的持久化。`nil` = **没有持久化接缝**（测试替身的默认）。
    ///
    /// ⚠️ 与 `historyStore` 同一条纪律：默认 `nil` 是**刻意的、也是必须的** ——
    ///    默认值指向的是人类伙伴真实的 `~/Library/Application Support/BenagenDownloader/`，
    ///    那会让**每一条**改目录的用例都去动他真实的配置文件。生产形态只由 `live()` 给。
    /// ⚠️ 没有 store 时 [`changeDownloadDir(to:)`] **不落盘**、但仍然换内存里的值与重启内核
    ///    （可观察状态一致），只是这次改动活不过下次启动。
    private let preferencesStore: AppPreferencesStore?

    /// 当前已在视图上的批次（重启后靠它把交付码与视图恢复回来）。
    private var loadedCode: String?
    private var loadedBaseURL: String?

    // MARK: - 构造

    /// 测试接缝：注入一个假客户端。
    ///
    /// ⚠️ 这条路上**没有**重启工厂：万一遇到内核崩溃，壳会拿到一条明确的失败
    /// （`.unavailable("内核重启失败：测试接缝没有提供重启工厂…")`），而不是凭空
    /// 起一个真子进程。要测重启就用 [`init(client:makeClient:)`]。
    ///
    /// ⚠️ `historyStore` 默认 `nil`（= **不落盘**）：这里给不出一个安全的默认目录
    ///    （默认目录就是人类伙伴真实的 Application Support），所以测试要测持久化必须
    ///    显式注入一个指向临时目录的 store。生产入口只有一个：`live()`。理由见
    ///    `historyStore` 属性的注释。
    public init(client: CoreCalling,
                handshakeTimeout: Double = AppModel.defaultHandshakeTimeout,
                historyStore: BatchHistoryStore? = nil,
                preferencesStore: AppPreferencesStore? = nil,
                downloadDir: String = "") {
        self.client = client
        self.historyStore = historyStore
        self.preferencesStore = preferencesStore
        self.makeClient = { _ in
            throw CoreError.transport("测试接缝没有提供重启工厂（要用 init(client:makeClient:) 注入）")
        }
        self.handshakeTimeout = handshakeTimeout
        // 与传给第一个内核的那份**同一个来源**（生产里由 `live()` 从盘上读一次、两处都用它）
        // —— 所以"内存里的值"与"第一个内核拿到的 argv"不可能分叉。
        self.downloadDir = AppPreferences(downloadDir: downloadDir).downloadDir
    }

    /// 测试接缝（带重启工厂）。
    public init(client: CoreCalling,
                makeClient: @escaping @Sendable (String?) throws -> CoreCalling,
                handshakeTimeout: Double = AppModel.defaultHandshakeTimeout,
                historyStore: BatchHistoryStore? = nil,
                preferencesStore: AppPreferencesStore? = nil,
                downloadDir: String = "") {
        self.client = client
        self.historyStore = historyStore
        self.preferencesStore = preferencesStore
        self.makeClient = makeClient
        self.handshakeTimeout = handshakeTimeout
        self.downloadDir = AppPreferences(downloadDir: downloadDir).downloadDir
    }

    /// 「内核压根没起来」的形态（`live()` 找不到二进制时用它）：没有客户端，
    /// 但原因已经装在 `engine` 里 —— 约束 4 要求它出现在界面上。
    init(engine: EngineState,
         makeClient: @escaping @Sendable (String?) throws -> any CoreCalling,
         handshakeTimeout: Double = AppModel.defaultHandshakeTimeout,
         historyStore: BatchHistoryStore? = nil,
         preferencesStore: AppPreferencesStore? = nil,
         downloadDir: String = "") {
        self.client = nil
        self.historyStore = historyStore
        self.preferencesStore = preferencesStore
        self.makeClient = makeClient
        self.handshakeTimeout = handshakeTimeout
        self.downloadDir = AppPreferences(downloadDir: downloadDir).downloadDir
        self.engine = engine
    }

    /// 生产形态：`App.swift` 唯一的构造入口。**不抛错** —— 起不来时把原因装进
    /// `model.engine == .unavailable(…)`，由工具栏的引擎徽标呈现（约束 4）。
    ///
    /// ⚠️ **壳的持久化只在这里被接上**（阶段 E）：历史落在
    ///    `~/Library/Application Support/BenagenDownloader/history.json`，
    ///    偏好（下载目录）落在同目录的 `preferences.json`。
    ///    两个分支都带同一个 store —— 内核起不来时历史与新目录也该是同一份。
    ///
    /// ⚠️ **下载目录的偏好在这里读一次**，而且**两处用的是同一个值**：一给 `make(...)`
    ///    （第一个内核的 argv），一给模型（界面显示与下一次重启的依据）。
    ///    读一次是硬要求 —— 读两次（比如让模型自己再读一遍）就有了两个来源，
    ///    而"界面显示的目录"与"内核实际用的目录"一旦分叉，症状是**文件下到别处去**，
    ///    没有任何提示。
    /// ⚠️ 坏文件（`preferences.json` 是一段乱码）⇒ `load()` 回 `.empty` ⇒ `argument(for:)`
    ///    返回 `nil` ⇒ 内核走自己的默认值，**应用照常启动**（E-1 + E-5）。
    public static func live() -> AppModel {
        let make: @Sendable (String?) throws -> any CoreCalling = { dir in
            try CoreClient.live(settingsPath: nil, downloadDir: dir)
        }
        let historyStore = BatchHistoryStore()
        let preferencesStore = AppPreferencesStore()
        let preferences = preferencesStore.load()
        do {
            return AppModel(client: try make(DownloadDirectory.argument(for: preferences)),
                            makeClient: make,
                            historyStore: historyStore,
                            preferencesStore: preferencesStore,
                            downloadDir: preferences.downloadDir)
        } catch {
            return AppModel(engine: .unavailable(Self.message(of: error)),
                            makeClient: make,
                            historyStore: historyStore,
                            preferencesStore: preferencesStore,
                            downloadDir: preferences.downloadDir)
        }
    }

    // MARK: - 请求通道

    /// **所有**请求的唯一出口：发出去、把协议级告警收进来。
    ///
    /// ⚠️ 这里**不做**错误处置 —— 处置分两种落点，由下面两个包装器决定
    /// （抛错路径由调用方在原地显示原文，非抛错路径落在 `lastError` 上）。
    private func call(_ method: String, _ params: JSONValue) async throws -> JSONValue {
        guard let client else {
            throw CoreError.transport("内核没有起来（\(engineReason)）")
        }
        do {
            let value = try await client.callAsync(method, params)
            // 这一次成了 ⇒ 上一条非抛错路径上的错误原文已经过期（下一拍再失败会重新写上）。
            // ⚠️ **值没变就不写**：`@Published` 与 `@Observable` 不同 —— 后者对 Equatable
            //    属性做等值抑制，前者不做。这一行在**每条成功请求**上都会跑（含 200 ms 的
            //    传输轮询），无条件写会让整棵视图树每秒重算 5 次。
            if lastError != nil { lastError = nil }
            // ⚠️ **必须在清空 `lastError` 之后**：新出现的协议级告警也是写进它的
            //    （约束 5 的界面落点，见 `syncProtocolAlerts`），先收告警再清空
            //    就把刚收到的告警自己抹掉了。
            syncProtocolAlerts()
            return value
        } catch {
            syncProtocolAlerts()     // 请求失败了，告警照样要收（它们不是一回事）
            throw error
        }
    }

    /// 抛错路径的包装：按附录 A 处置状态（含重启），然后**原样抛出** ——
    /// 调用方（视图）在事发地显示内核原文。
    private func absorbing<T>(_ body: () async throws -> T) async throws -> T {
        do {
            return try await body()
        } catch let e as CoreError {
            _ = await absorb(e)
            throw e
        }
    }

    /// **不抛错**路径的包装（轮询 / 校验刷新）：处置状态，并把"没被状态吸收掉"的原文
    /// 落到 `lastError` —— 这几条路没有调用方可以 catch，而约束 4 不允许静默失效。
    /// 返回 nil 表示这次没拿到数据（界面保持上一帧快照）。
    private func surfacing<T>(_ body: () async throws -> T) async -> T? {
        do {
            return try await body()
        } catch let e as CoreError {
            if case .verbatim(let text) = await absorb(e) { surface(text) }
            return nil
        } catch {
            surface("\(error)")
            return nil
        }
    }

    /// 把客户端累积的协议级告警（`id == 0`）搬进可观察状态，并**让新出现的那条上界面**。
    ///
    /// ⚠️ 约束 5 要求 `id == 0` 的协议级告警"**单独上报**"。`protocolAlerts` 有两条测试
    ///    守着，但**没有任何视图读它** —— 在这条链补上之前，它是全壳唯一一类**没有任何
    ///    界面落点**的失败。这里复用已有的错误出口 `lastError`（`EngineBanner` 读它），
    ///    **不新增通道**（裁定 ① / 约束 17）。
    ///
    /// ⚠️ **只写新出现的**：内核那份列表在一个连接里**只增不减**（每次 `id == 0` 追加）。
    ///    每拍都把同一条重写一遍会毁掉 `lastError` 的"瞬时"语义 ——
    ///    `EngineBanner` 的 `.transientError` 明说"下一拍成功就自己消失"。
    ///    记账用**条数**（`surfacedAlerts`）：变短了就是换了内核（重启后从空列表重新累积），
    ///    那时从头重数。
    ///
    /// ⚠️ 文本**逐字是内核的 `message`**（约束 3）：`EngineBanner` 的正文不允许壳加前缀。
    private func syncProtocolAlerts() {
        let fresh = client?.protocolAlerts ?? []
        // ⚠️ 同上：这一句在**每条请求**上都跑，而 `protocolAlerts` 绝大多数时候没变。
        //    值不变就不写（`ErrorBody` 因此加了 `Equatable`，见 `Protocol.swift`）。
        if protocolAlerts != fresh { protocolAlerts = fresh }

        if fresh.count < surfacedAlerts { surfacedAlerts = 0 }
        defer { surfacedAlerts = fresh.count }
        guard let newest = fresh.dropFirst(surfacedAlerts).last else { return }
        lastError = newest.message
    }

    // MARK: - 启动

    /// 握手 + 读设置 + 读上次交付码。**有界**：超过 `handshakeTimeout` 就放弃等待并明示。
    ///
    /// 三件事**要么一起成、要么一起不成**：拿不到 `min_split_size_choices` 就没有
    /// 参数面板的枚举面，拿不到 `last_code` 就没有"自动加载上次那批"。
    /// 重启之后壳也调它（见 `restartKernel`）。
    ///
    /// ⚠️ **为什么需要超时**（任务 4b 的全部理由）：内核在**启动时**就 `open()` 默认下载目录
    ///    `~/Downloads/Benagen`（`core/src/main.rs:1620` 的默认值 + `:202` 的 `Kernel::new`
    ///    立刻 `FileState::load`），而 `~/Downloads` 受 macOS TCC 保护 —— **首次启动**
    ///    会弹一个系统授权框，`open()` 阻塞到用户点按为止。那一刻内核**根本不在读 stdin**，
    ///    我们发出去的 `hello` 永远等不到响应：没有超时，`start()` 就永不返回，
    ///    徽标永远停在「正在连接内核…」，而用户看到的是一个卡死的应用
    ///    （那个授权框还可能正压在窗口后面）。**内核侧不修**（约束 10），壳只负责
    ///    "有界等待 + 把该点什么、该看哪里说清楚"。
    ///
    /// ⚠️ **超时之后，"再发一次 hello"是没用的**：`CoreClient` 内部是一条 FIFO **串行**队列
    ///    （约束 15 的单飞语义就靠它），那条永不返回的请求会把队列**永久堵死** ——
    ///    新请求排在它后面，永远轮不到。唯一的恢复路径是**新建客户端 + 新内核进程**
    ///    （用户点的「重试」→ `retryEngine` → `restartKernel`）。
    ///    **所以这里不重试、也不自动重启**：超时是"这个内核还没准备好说话"，
    ///    而自动重启会把用户正在看的授权框底下的进程换掉，反而更乱。
    public func start() async {
        guard client != nil else { return }   // 连内核都没有（live() 失败）：engine 里已有原因

        do {
            let handshake = try await withTimeout(handshakeTimeout) { try await self.performHandshake() }
            // ⚠️ 状态只在这里落：超时那条路是**另一条**分支，输掉的握手任务即使稍后回来了，
            //    它带回来的也只是一个被丢弃的返回值（`performHandshake` 不写任何状态）——
            //    不会出现"超时之后又被一次迟到的成功改写成 .notStarted"。
            minSplitSizeChoices = handshake.choices
            settings = handshake.settings
            lastCode = handshake.lastCode
            // 内核只在 `enqueue` 里起引擎（`op_enqueue` 的 `ensure_engine`），而这个内核进程
            // 是壳刚起的 —— 所以握手完成后「引擎未启动」是**事实**，不是猜测。
            engine = .notStarted
        } catch is HandshakeTimeout {
            // 🔴 这一处是**壳自己写用户可见文案**（不是"唯一一处"—— 同一个文件里
            //    `absorb` 的 `.protocolMismatch` 分支、`onKernelDeath` 的"下载引擎已断开：…"、
            //    `restartKernel` 的"内核重启失败：…"同样是壳写的话；其余一律"内核原文照登"，约束 3）。
            //    理由：超时是**壳探测到的**状况 —— 内核一个字都没回，没有"原文"可登，
            //    而"什么都没发生"恰恰是约束 4 要防的静默失效。
            //    有意偏离内核行为，理由写在这里（约束 11）。
            //    文案本身在 `handshakeTimeoutMessage`（它同时是 `EngineStatusPresentation`
            //    用来区分"超时"与"内核给的失败原因"的判别式）。
            engine = .unavailable(Self.handshakeTimeoutMessage)
        } catch let e as CoreError {
            switch await absorb(e) {
            case .verbatim(let text):
                // 握手是**一次性**的（不像轮询有下一拍），而它失败 ⇔ 这个内核讲不通话。
                // 所以这里不用 `lastError`：徽标上必须留着那句话（约束 4）。
                engine = .unavailable(text)
            case .absorbed:
                break
            }
        } catch {
            engine = .unavailable("\(error)")
        }
    }

    /// 应用级启动：握手 + 自动加载记住的交付码。**只跑一次。**
    ///
    /// ⚠️ 为什么必须有这道一次性的闸：`.task` 挂在 `WindowGroup` 的**内容**上
    ///    （`App.swift`），每开一个新窗口就重跑一次，而 `model` 是 App 级单例、所有窗口共用。
    ///    重跑的后果**不是**"多花点时间"：
    ///      - `start()` 会把 `engine` 打回 `.notStarted`（丢掉此前 `.unavailable` / 已启动的事实）；
    ///      - `loadRememberedDelivery()` 会把 `loadState` 打回 `.loading`
    ///        （`performLoadDelivery`），主区于是走 `else` 分支显示空态页，
    ///        而 `load_delivery` 实测约 91.5 秒。
    ///    **对用户来说，"窗口回来了但空白 91 秒"与"打不开"是同一件事** ——
    ///    所以修主窗口重开时，这一条必须一起修，否则症状只是换了个样子。
    ///
    /// 实现那道闸的 `didBootstrap` 在「注入（不参与观察）」一节（与那些**不加
    /// `@Published`** 的私有旗标放在一起 —— 它们只有这个类自己读，见那一节顶上的说明）
    /// —— **不能夹在这段文档和函数之间**：
    /// 那样这段 `///` 会挂到那个私有属性上，`public` 的 `bootstrap()` 反而没有文档。
    public func bootstrap() async {
        guard !didBootstrap else { return }
        didBootstrap = true
        await start()
        await loadRememberedDelivery()
    }

    /// 握手的**载荷**：两条请求的结果，只有整体成功才作数（`hello` 与 `get_settings`
    /// 要么一起拿到、要么一起没有）。
    ///
    /// ⚠️ 它是个**纯返回值**、不写任何状态 —— 这才是"输掉的那次握手不会在超时之后
    ///    偷偷改状态"的保证（见 `start()`）。
    private struct Handshake: Sendable {
        let choices: [String]
        let settings: Settings
        let lastCode: String
    }

    /// 真正发请求的那一段。**在后台的竞速任务里跑**，所以它是 `@Sendable` 的调用目标。
    private func performHandshake() async throws -> Handshake {
        let hello = try await call("hello",
                                   .object(["protocol": .integer(Int64(kProtocolVersion))]))
            .decoded(HelloResult.self)
        let current = try await call("get_settings", .null).decoded(SettingsResult.self)
        return Handshake(choices: hello.minSplitSizeChoices,
                         settings: current.settings,
                         lastCode: current.lastCode)
    }

    /// 有界等待：把 `operation` 与一个计时器赛跑，**谁先完成用谁**。
    ///
    /// 超时抛 [`HandshakeTimeout`] —— 它**故意不是 `CoreError`**：它不是内核给的，
    /// 没有结构化 `code` 可分支，也**不该**被 `absorb`（附录 A 的处置表）当成内核错误
    /// 那样触发自动重启（理由见 `start()`）。
    ///
    /// ⚠️ 输掉的那条 `Task` **不取消、也不等**：
    ///   - 取消不了 —— `CoreClient.callAsync` 内部是一个不响应取消的 continuation，
    ///     它得等内核回话或管道断开才会醒（那正是它卡住的原因）；
    ///   - 不能等 —— 等它就是把壳焊死在同一个地方，超时也就白加了。
    ///   它活着不影响正确性（`RaceClaim` 保证只有一个赢家能 resume），代价只是多挂一个
    ///   continuation，直到那个内核进程被 `shutdown()` 收掉、它的读以 EOF 收场为止。
    ///
    /// ⚠️ 这里**不用 `withThrowingTaskGroup`**：任务组在作用域退出时会**等所有子任务**，
    ///   而那个卡住的子任务恰恰等不到 —— 于是"超时"会变成"换个地方继续卡"。
    private func withTimeout<T: Sendable>(
        _ seconds: Double,
        _ operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        let claim = RaceClaim()
        return try await withCheckedThrowingContinuation { cont in
            Task {
                do {
                    let value = try await operation()
                    if claim.take() { cont.resume(returning: value) }
                } catch {
                    if claim.take() { cont.resume(throwing: error) }
                }
            }
            Task {
                try? await Task.sleep(nanoseconds: Self.nanoseconds(seconds))
                if claim.take() { cont.resume(throwing: HandshakeTimeout()) }
            }
        }
    }

    /// 秒 → 纳秒（给 `Task.sleep` 用）。负值夹到 0（`UInt64` 不接受负数）。
    private static func nanoseconds(_ seconds: Double) -> UInt64 {
        guard seconds > 0 else { return 0 }
        return UInt64(seconds * 1_000_000_000)
    }

    // MARK: - 交付清单

    /// 加载（或刷新）一个交付批次。
    ///
    /// ⚠️ 复位自动重启预算：这是**用户的显式动作**（换交付码 / 手动重新加载 = 新一轮）。
    /// 时机与理由见 `restartIfAllowed`。
    ///
    /// ⚠️ `try?`：这条路上（`.initial` 模式）**失败不抛给调用方** —— 它已经落在
    ///    `loadState` / `engine` 上了（空态页显示内核原文 + 「重试」）。抛出来只是
    ///    `performLoadDelivery` 为了同时服务两种模式而有的形状（见那里的注释）。
    public func loadDelivery(code: String, baseURL: String?) async {
        restartAttempted = false
        _ = try? await performLoadDelivery(code: code, baseURL: baseURL)
    }

    /// 换一个交付码。**失败时原批次一个字都不动。**
    ///
    /// ⚠️ 为什么不能直接复用 `loadDelivery`：那条路会先把 `loadState` 置成 `.loading`、
    ///    失败时再置成 `.failed` —— 用户在**已经加载好、可能正在下载**的批次里
    ///    手误打错一个码，就会被踢回空态页，原来那批的界面状态全没了。
    ///    换码是"在当前批次之上再加载一批"，它的失败**不该**毁掉当前批次。
    ///
    /// 实现上只有一处与 `performLoadDelivery` 不同：**它不写 `loadState` 的 loading/failed**，
    /// 只在成功那一刻提交。为此 `performLoadDelivery` 增加一个 `mode` 参数
    /// （`.initial` = 今天的行为，`.switching` = 成功才提交）。
    ///
    /// ⚠️ 返回值而不是抛错：面板要的就是"成了没有 + 失败该显示哪句话"，
    ///    而"失败时该说什么"由 `Self.message(of:)`（`CoreError` → 文案的**唯一**实现，
    ///    约束 C-7）算好交出去 —— 视图一个字都不编（约束 3）。
    ///
    /// ⚠️ 唯一的例外是"内核引擎不可用"：那时 `performLoadDelivery` 抛的是
    ///    **壳自己造的** `CoreError.transport(engine 里那句原文)`，同样经 `message(of:)`
    ///    变成给用户看的那句话。
    public func switchDelivery(code: String, baseURL: String?) async -> DeliverySwitch {
        // ⚠️ **重入闸**（延后清单第 9 条）：一次只跑一次换码。视图那侧本来就禁用按钮
        //    （`model.switching`），这道闸是**结构性的第二道** —— 与 `listDir` / `enqueue`
        //    那几处"渲染与点击之间状态可能翻"是同一条纪律。没有它，两条并发换码会在
        //    内核那条 FIFO 队列上排队，两次请求的**结果顺序**与用户在界面上看到的顺序
        //    可能相反（第二次的响应先回来 ⇒ 屏上是第一批，而壳记的是第二批）。
        //
        // 返回值是 `.failed` 而不是"悄悄返回一个空壳"：调用方拿到的是一句**能显示**的话，
        // 而不是一个"按了没反应"的按钮（约束 4）。
        // 🔴 这句话是**壳自己写的**（约束 C-10/11 要求写明理由）：这一次换码**根本没发出去**
        //    —— 是我们自己在壳里挡住的，内核没有原文可登（约束 3 的例外）。
        //
        // ⚠️ 这条早退**不写 `lastSwitchOutcome`**：那条通道归**正在飞的那一次**所有，
        //    由它结束时落定。在这里写会被它随即覆盖，只会留下一次无意义的闪动。
        guard !switching else {
            return .failed(message: "上一次换码还在进行中，请等它结束。")
        }
        // 同 `loadDelivery`：这是用户的显式动作 = 新一轮，复位自动重启预算。
        restartAttempted = false
        switching = true
        defer { switching = false }

        // 上一轮的结果先收起：主区那一行说的是"最近一次换码"，不该在**新一次**开始之后
        // 还挂着上一次的结论（那会让用户以为它就是这次的结果）。
        lastSwitchOutcome = nil

        let outcome: DeliverySwitch
        do {
            let info = try await performLoadDelivery(code: code, baseURL: baseURL, mode: .switching)
            outcome = .switched(code: info.code)
        } catch let e as CoreError {
            outcome = .failed(message: Self.message(of: e))
        } catch {
            outcome = .failed(message: "\(error)")
        }
        // ⚠️ **回执落在模型上**（不只是交给面板）：面板会被「取消」关掉，而那句话
        //    必须活下来（详见 `lastSwitchOutcome` 的注释）。返回同一个值给面板，
        //    是为了让面板在**自己还开着**的时候也能就地显示 —— 两个落点、同一个值，
        //    不会分叉。
        lastSwitchOutcome = outcome
        return outcome
    }

    /// 收起主区那一行"换码结果"提示（视图上那个 × 的唯一出口）。
    ///
    /// 同 `downloadNotice` 的收起：**用户主动**收起不算静默失效（约束 4 要的是
    /// "它出现过、且出现过够久"，不是"永远不许消失"）。
    public func dismissSwitchOutcome() {
        lastSwitchOutcome = nil
    }

    /// 规格 §8.3「记住交付码」：内核记着上次用过的码就自动加载一次。
    ///
    /// ⚠️ **必须在 `start()` 之后调**（调用点在 `App.swift` 的同一条 `.task` 里，紧跟
    ///    `await model.start()`）：`lastCode` 正是**握手的产物**（`get_settings` 回的
    ///    `last_code`），握手完成前它必然是空串 —— 早一步调它，这条自动加载就永远不触发，
    ///    **而且不报任何错**（约束 4 最讨厌的那种失效）。
    ///    为此**不**把它放进某个视图的 `.task`：视图的 `.task` 与外层 `App.swift` 的
    ///    `.task` 谁先跑没有保证，靠它去等握手等于把正确性押在 SwiftUI 的调度顺序上。
    ///
    /// 失败不用特殊处理：`loadDelivery` 自己不抛错，失败时把 `loadState` 置成 `.failed`，
    /// 界面走到空态页并显示内核原文 + 「重试」——这正是"自动加载失败不得把界面卡在错误页"。
    ///
    /// ⚠️ **阶段 E 改了"码从哪来"**（规格 §1.3 / E-4）：不再是内核的 `lastCode`，
    ///    而是**壳的历史里最近使用的那一条**（`rememberedCode()`）。
    ///    "没有任何码可加载 ⇒ 一个请求都不发"这条行为**原样保留**（它在
    ///    `rememberedCode()` 返回空串时仍然成立）。
    public func loadRememberedDelivery() async {
        let remembered = rememberedCode()
        guard !remembered.code.isEmpty else { return }
        await loadDelivery(code: remembered.code, baseURL: remembered.baseURL)
    }

    // MARK: - 批次历史（阶段 E）

    /// 决定"这次自动加载哪个码"，并在壳的历史为空时把内核的 `last_code` **一次性**播种进来。
    ///
    /// **E-4：权威只有一个。**
    /// - 壳的历史非空 ⇒ 用它最近使用的那条（`base_url` 一起带回去 —— 空串 = 默认服务器）；
    ///   内核的 `lastCode` **一个字都不读**。这是要防的那种分裂：
    ///   "界面显示 A（壳的历史）、内核却自动加载 B（内核的 last_code）"。
    /// - 壳的历史为空（第一次升级到这个版本、用户删掉了文件）⇒ 读内核的 `lastCode`
    ///   作为**一次性种子**，写进历史，之后不再读它。
    ///
    /// ⚠️ 播种与"只在成功加载之后才写"（§1.2）**不冲突**：种子说的是**内核那边已经
    ///    成功加载过的那一批**（内核只在 `load_delivery` 成功路径上写 `last_code`，
    ///    `core/src/main.rs:905-923`）——它不是"这一次加载成功了"。
    /// ⚠️ 种子**先落下再加载**（而不是"加载成功才写"）：这样"之后壳不再读它"才是一条
    ///    真的承诺 —— 否则内核那份 `last_code` 会在每一次历史为空的启动里重新参与决策。
    ///    代价是：紧接着的那次加载若失败（网络、码过期），历史里也会留着这个码。
    ///    那是可接受的 —— 它是**内核记着的那个码**，不是用户打错的码（§1.2 要防的是后者），
    ///    而且界面上那次失败是**看得见**的（空态页的内核原文 + 「重试」）。
    private func rememberedCode() -> (code: String, baseURL: String?) {
        loadHistoryIfNeeded()
        if let newest = history.mostRecent {
            return (newest.code, newest.baseURLOrNil)
        }
        // ⚠️ `lastCode` 是内核那个单行文件里的原文（可能带结尾换行），trim 一下再用：
        //    它要被当成壳这份历史的主键，带空白的主键会让"同一个码"看起来是两条。
        let seed = lastCode.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !seed.isEmpty else { return ("", nil) }
        writeHistory(history.recording(code: seed, at: Date()))
        return (seed, nil)
    }

    /// 把历史从盘上读进来。**每次进程只读一次。**
    ///
    /// ⚠️ 为什么"必须在第一次写之前读"是一条硬要求：写路径是**读-改-写**
    ///    （`history` 是内存里那份权威）。少了这一步，用户第一次手输一个码就会拿
    ///    **空历史**去覆盖盘上那份（最多 50 条、还有他自己写的备注）—— 一次静默的
    ///    数据丢失，而且没有任何报错。钉住它的是
    ///    `theHistoryIsLoadedBeforeTheFirstWriteSoAManualLoadNeverWipesIt`。
    ///
    /// ⚠️ 只读一次（不是每次写前都读）：进程内 `history` 与盘上那份在每次
    ///    `writeHistory` 之后是相等的，重复读只会把"另一个进程刚好也在写"这种
    ///    不属于本阶段的情形引进来。
    private func loadHistoryIfNeeded() {
        guard !historyLoaded else { return }
        historyLoaded = true
        guard let historyStore else { return }   // 没有持久化接缝：历史只在内存里
        history = historyStore.load()
    }

    /// 一次**成功**的加载之后把它记进壳的历史（规格 §1.2）。
    ///
    /// ⚠️ **只在成功分支上调**，与内核 `remember_last_code` 的时机一致
    ///    （`core/src/main.rs:905-923`：只在 `load_delivery` 成功路径上写）。
    ///    失败的加载写进去，历史里就会堆满打错的码 —— 那是这个功能最容易变成噪音的方式。
    ///
    /// ⚠️ 记的是**这次请求用的** `baseURL`（不是内核回显的 `info.baseUrl`）：
    ///    回显那份在"用户没指定"时是内核的默认交付服务器地址，把它写进历史、
    ///    下次再显式传回去，会在**内核默认值变化时静默分叉**（E-5 的同一条纪律）。
    private func recordLoadedBatch(code: String, baseURL: String) {
        loadHistoryIfNeeded()
        writeHistory(history.recording(code: code, baseURL: baseURL, at: Date()))
    }

    /// 改一条历史记录的**备注**（换码面板里就地编辑那一行，规格 §1.4）。
    ///
    /// ⚠️ **它只改备注**（`BatchHistory.settingNote` 只碰 `note` 这一个字段）：
    ///    写一句备注**不是"又用了一次"**。把 `last_used_at` 也顶上去的话，这一条会当场
    ///    跳到列表最上面 —— 用户刚看着的那一行从他眼皮底下跑了，而他只是写了个备注。
    ///
    /// ⚠️ 对一个**不在历史里**的码调它是**空操作**（判据在 `BatchHistory.settingNote`）：
    ///    换码面板只对屏上那些行提供编辑，凭空造一条没有时间戳的记录只会变成
    ///    "排在最末、点了没反应"的假条目。
    ///
    /// ⚠️ 写盘失败**不抛**：落点同"记住一次成功加载"（`historyWriteFailure`，
    ///    由界面呈现）—— 写一句备注失败不该让面板弹出别的东西。
    public func setHistoryNote(_ note: String, forCode code: String) {
        loadHistoryIfNeeded()
        writeHistory(history.settingNote(note, forCode: code))
    }

    /// 收起「历史写入失败」那行提示（主区那行与换码面板里那行共用的唯一出口）。
    ///
    /// 同 `dismissSwitchOutcome` / `dismissDownloadDirChange`：**用户主动**收起不算
    /// 静默失效（约束 4 要的是"它出现过、且出现过够久"，不是"永远不许消失"）。
    /// ⚠️ 它**主要**的消失方式其实不是这一颗 `×`，而是**一次成功的写盘**
    ///    （`writeHistory` 的成功分支把字段清回 `nil`）—— 用户收起之后若问题还在，
    ///    下一次写盘会重新写上它。
    public func dismissHistoryWriteFailure() {
        historyWriteFailure = nil
    }

    /// 落历史：先落内存（可观察状态），再尽力落盘。
    ///
    /// ⚠️ **写盘失败既不抛也不改 `loadState`**：一次**成功**的加载不该因为
    ///    "历史存不下来"被判成失败（那会让用户以为文件也没加载上）。
    ///    原文落在 `historyWriteFailure` 上 —— 不许静默失效（约束 4）。
    ///
    /// ⚠️ **同步写、不甩后台任务**（E-8：这是一个有意的选择）：这份文件最多 50 条、
    ///    几十 KB，一次 `write + rename` 是微秒级；换来的是"记历史"这件事**在加载返回
    ///    之前就已经落定**（与内核 `remember_last_code` 同帧）。甩到后台会有两个代价：
    ///    ① 用户加载完立刻退出应用，那一条就丢了；② 测试只能靠 `waitUntil` 猜它写完了没
    ///    （时序 flaky 的经典来源）。真到了"写盘慢到看得见"的那天（网络盘上的
    ///    Application Support），该改的是把整份历史放到后台队列上串行化，而不是这里加 `Task`。
    private func writeHistory(_ updated: BatchHistory) {
        history = updated
        guard let historyStore else { return }
        do {
            try historyStore.save(updated)
            historyWriteFailure = nil
        } catch {
            historyWriteFailure = "历史记录写入失败：\(error)"
        }
    }

    // MARK: - 下载目录（阶段 E 规格 §2）

    /// 改下载目录（用户**已经确认**过 E-6 那三条后果）。**唯一**能让新目录生效的入口。
    ///
    /// 顺序是硬要求，四步：
    ///   ① **偏好先落盘**（失败 ⇒ 中止，一个内核都不许动）；
    ///   ② 落内存（`downloadDir`）—— `spawnClient()` 就是从它算 argv 的；
    ///   ③ **重启内核**（`restartKernel()`：收尾旧的 → 新进程带 `--download-dir <新目录>` → `start()`）；
    ///   ④ 重启流程自己会**重新加载当前批次**（`restartKernel` 里那条
    ///      `performLoadDelivery(code: loadedCode, …)`）—— 新目录的状态文件是空的，
    ///      界面必须反映这一点，否则会显示一批"已完成"的假象
    ///      （阶段 D 刚修掉的那类缺陷，不能在这里重新造出来）。
    ///
    /// ⚠️ **顺序不能反**（①必须在③之前）：先重启再落盘的话，重启中途失败/崩溃就会留下
    ///    "内核在新目录里跑、盘上还记着旧目录"的分裂 —— 下次启动**静默**换回去。
    /// ⚠️ **写盘失败 ⇒ 不动内核**：同一条理由的反面。一次"以为改了"的静默失败，
    ///    用户要等到下次启动才会发现（设置又变回去了），而那时已经无从归因。
    /// ⚠️ **同值 ⇒ 什么都不做**（`.unchanged`）：E-6 那句"正在跑的任务会停"说的是
    ///    **真的改了**的时候。用户在面板里选中**同一个**目录是很可能发生的
    ///    （"我只是想确认一下现在用的是哪个"），不该为此停掉他正在下的东西。
    /// ⚠️ 这是**用户的显式动作** ⇒ 复位自动重启预算（同 `loadDelivery` / `retryEngine`）：
    ///    否则"自动重启已经用掉预算"会让改目录变成一件按下去没有回音的事（约束 4）。
    /// ⚠️ 重启失败时返回 `.failed`（原文就是 `restartKernel` 写进 `engine` 的那句
    ///    "内核重启失败：…"，同时也挂在引擎横幅上），而**盘上的偏好照旧留着** ——
    ///    那是用户的选择，不该被一次失败悄悄撤销：下次启动仍然按它起内核。
    ///    代价如实记下：这一段时间里"界面说的目录"与"内核实际用的目录"不一致，
    ///    而它**不是静默的**（回执那行 + 引擎横幅都在说）。
    ///
    /// ⚠️ **回执落在模型上**（`downloadDirChange`），不只是交给调用方 —— 设置窗口会被
    ///    关掉，而"没改成"那句话必须活下来（理由写在那个字段的注释里）。
    ///    实现方式就是下面这个包装：**不论从哪一格返回，回执都已经落定了**，
    ///    结构上不可能漏掉任何一条出口（`SettingsView` 那侧读的是同一个值）。
    @discardableResult
    public func changeDownloadDir(to path: String) async -> DownloadDirChange {
        // 上一轮的结果先收起（同 `switchDelivery`）：那一行说的是"上一次那一按"，
        // 不该在新一次已经开始之后还挂在屏幕上。
        downloadDirChange = nil
        let outcome = await performDownloadDirChange(to: path)
        downloadDirChange = outcome
        return outcome
    }

    /// 收起主区那行"下载目录"提示（两处那颗 `×` 的唯一出口）。
    ///
    /// 同 `dismissSwitchOutcome`：**用户主动**收起不算静默失效（约束 4 要的是
    /// "它出现过、且出现过够久"，不是"永远不许消失"）。下一次改动会重新写上它。
    public func dismissDownloadDirChange() {
        downloadDirChange = nil
    }

    /// 改下载目录的**实际动作**（唯一的出口在 `changeDownloadDir`，回执在那里落定）。
    private func performDownloadDirChange(to path: String) async -> DownloadDirChange {
        let updated = AppPreferences(downloadDir: path)
        guard updated.downloadDir != downloadDir else {
            return .unchanged(dir: downloadDir)
        }

        if let preferencesStore {
            do {
                try preferencesStore.save(updated)
            } catch {
                // 🔴 这句话是**壳自己写的**（约束 3 的例外，理由写明）：内核**还不知道**
                //    这件事（我们还没动它），没有原文可登 —— 而写盘失败必须有落点，
                //    否则就是一次"以为存上了"的静默失败（E-2 的注释里那句）。
                return .failed(message: "下载目录没有改成（偏好写入失败：\(error)）")
            }
        }

        downloadDir = updated.downloadDir
        // ⚠️ **校验快照跟着作废**（同 `performLoadDelivery` 里那条"换批即作废"的纪律，
        //    理由换了一个：这次**批次没换，但下载根换了**）。那份快照说的是
        //    "**上一个目录里**这些文件的校验结果"，而新目录里它们一个都还没校验过 ——
        //    留着它，侧边栏那颗徽标与整个校验屏会显示一组**看起来属于这一批**的旧数字，
        //    与阶段 D 修掉的"已完成假象"是同一类缺陷（用户照着它做决定）。
        //    ⚠️ 置 nil 的语义是"**还不知道**"（不是"校验没过"）：校验屏照常显示它的空态，
        //       用户点一次「刷新」就会从新内核手里拿一份真的。
        //    ⚠️ 这一行必须在**确认改动已经成立之后**（写盘成功之后）：写盘失败那条早退
        //       什么都不该动。
        verify = nil
        restartAttempted = false

        // ⚠️ **必须确保"这一次重启真的发生了"**：`restartKernel()` 里有一道 `restarting`
        //    闸（同时只跑一次重启），而内核刚崩时的自动重启会占着它最长约十秒
        //    （收尾 ≤8s + 握手 ≤5s）。用户在这个窗口里确认一个新目录是**完全可能的**
        //    ——他刚看到横幅变红，就去设置里改目录——而那一刻在飞的那次重启
        //    **已经拿着旧目录**在起新内核了。若就此放过，结果是"内核用旧目录在跑、
        //    壳记的是新目录"，症状是文件下到了用户以为已经换掉的地方。
        //    所以：先等它落地，再重启一次（那一次一定拿得到新值）。
        //
        //    ⚠️ 等**一次**就够：自动重启用完预算后 `restartAttempted` 为真，
        //    在用户动作复位它之前不会有第二次自动重启插进来（`restartIfAllowed`）；
        //    而用户动作与我们同在主 actor 上，插不进来。
        await waitForTheRestartInFlightToFinish()
        guard await restartKernel() else {
            return .failed(message: "内核还在重启中，这一次没有改成。"
                           + "下载目录已经记下来了，下次启动会按它生效。")
        }
        // 内核没起来 ⇒ 如实说（那句话同时也是顶部引擎横幅的内容）。
        if case .unavailable(let why) = engine { return .failed(message: why) }
        return .changed(dir: updated.downloadDir)
    }

    /// 等一次**在飞的重启**落地（`restartKernel` 的 `restarting` 闸）。**有界。**
    ///
    /// 为什么需要它：见 `changeDownloadDir` 里那一段（那次重启已经拿着旧目录在起内核了，
    /// 而我们被那道闸挡回来就什么都不会发生）。
    ///
    /// ⚠️ 上界 20 秒：`restartKernel` 的每一段都有上界（同步收尾 ≤8s、起进程几十毫秒、
    ///    握手 ≤5s，见各处的注释），所以正常情形下这个循环几十毫秒就结束。真到了它
    ///    超过 20 秒的那天（说明某一段的上界破了），**继续等下去也不再有用** ——
    ///    退回"这一次没改成"，那是看得见的失败，不是静默的分叉。
    /// ⚠️ 每 10 ms 让一次：`Task.sleep` 会真正把主 actor 让出去，在飞的那条重启
    ///    （也是主 actor 上的任务）才跑得下去 —— 用 `Task.yield()` 这种忙等会把它饿死。
    private func waitForTheRestartInFlightToFinish(timeout: Double = 20) async {
        let deadline = Date().addingTimeInterval(timeout)
        while restarting, Date() < deadline {
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
    }

    /// 真正干活的那一段。重启后的"恢复视图"也走它 —— **故意不复位重启预算**，
    /// 否则"崩 → 重启 → 恢复 → 又崩"会自我续命成无限循环。
    ///
    /// ⚠️ **`mode` 是"失败写不写 `loadState`"的总闸**（`.initial` 写、`.switching` 一个都不写）：
    ///    它存在的唯一理由就是换交付码那条路（见 `switchDelivery`），逐处分支见下面四段注释。
    ///
    /// ⚠️ **失败在两种模式下都会 `throw`**，但对外行为完全不同：
    ///    - `.switching`：错误**原样**交给调用方（换码面板显示内核原文），状态一个字都不写；
    ///    - `.initial`：**状态照旧写完**（与改动前逐字相同），然后抛出去 ——
    ///      调用方是 `loadDelivery` / 重启恢复，两边都用 `try?` 吞掉。
    ///      那个抛出**不改变 `.initial` 的对外行为**，它只是让"一个函数同时服务两种模式"
    ///      这段控制流成为可能（`.initial` 的失败没有 `DeliveryInfo` 可返回）。
    private func performLoadDelivery(code: String,
                                     baseURL: String?,
                                     mode: LoadMode = .initial) async throws -> DeliveryInfo {
        // ⚠️ **引擎不可用时快速失败，不把请求发出去**（本任务的最小防线，约束 4）。
        //
        // 理由（任务 4b 查明的事实）：内核卡死时（首次启动卡在 TCC 授权框上的 `open()` 里），
        // 那条 `hello` **永不返回**，而 `CoreClient` 内部是一条 FIFO **串行**队列（约束 15 的
        // 单飞语义就靠它）—— 队列被永久堵死，**此后每个新请求都排在那条后面，永远轮不到**。
        // 于是用户点「加载」看到的是一句「正在加载交付清单…」**永远转下去**：
        // 一个没有任何请求会返回的转圈，而 `loadState` 停在 `.loading` 这件事，
        // 用户再点多少次加载都改不掉。这正是约束 4 明禁的**静默失效**。
        //
        // 所以：与其发一条注定挂死的请求，不如**现在**就走界面本来就会显示的失败态
        // （`EmptyState` 显示内核原文 + 「重试」）。文案用 `engine` 里那句原文（约束 3），
        // 壳一个字都不编 —— 引擎不可用的原因（超时 / 内核自述 / 重启失败）已经在那里了。
        //
        // ⚠️ 判据只看 `.unavailable` 一个分支，**不看 `.unknown`**：
        //    `.unknown` 是"握手还没结论"，那时请求排在握手后面是**正常**的（会轮到），
        //    而且任务 4b 的握手有 5 秒上界，超时后引擎自然落到 `.unavailable`、由这里兜住。
        //    （任务 8 会在横幅上给「重试」，并把其余入口一并闸住。）
        if case .unavailable(let why) = engine {
            // ① 引擎不可用的快速失败，按模式分岔（原文照登，约束 3）：
            //    - `.initial` ⇒ 走界面本来就有的失败态（空态页显示原文 + 「重试」），与改动前相同；
            //    - `.switching` ⇒ 一个状态都不写，把错误抛给换码面板。
            //
            // ⚠️ 抛出点在 `absorb` **之外**，这一点是刻意的、与 `enqueue` 的引擎闸门逐字同款：
            //    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发），
            //    喂进 `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` ——
            //    于是**换一次码就杀掉并重启一次内核**，还把 `engine` 改写成
            //    "下载引擎已断开：<原来那句>"。钉住它的是
            //    `switchingAfterTheEngineDiedFailsFastAndKeepsTheBatch` 里的
            //    `factory.madeCount == 1`（没有新内核被造出来）。
            if mode == .initial { loadState = .failed(why) }
            throw CoreError.transport(why)
        }

        // ⚠️ **交付码过长 ⇒ 拒发**（本任务补的第二道闸，全局约束 C-3）。
        //
        // 它与上面那条引擎闸门（以及 `enqueue` 的体量守卫）**逐字同款**，理由也相同：
        // 内核的行长上限是 8 MiB（`core/src/main.rs:113`），而超限的处置**不是报错** ——
        // `:1657-1661` 只回一条 `id == 0` 的协议告警，那条请求**永远等不到响应**，
        // `CoreClient` 那条 FIFO **串行**队列于是**被永久堵死**（约束 15），
        // 而界面上一个字都不说。这正是约束 4 明禁的静默失效，且形态是"应用整个卡死"。
        //
        // ⚠️ **为什么模型层还要再挡一次**（视图那两处已经禁用按钮 + 明说了）：
        //    C-3 这条要求不该只靠视图层守。视图今天挡住了，但 `loadDelivery` /
        //    `switchDelivery` 是 `public` 的，任何一个新调用点（或某次"顺手"的
        //    `Task { }`）都会绕过视图那道闸；而这条链的代价是**整个应用卡死**。
        //    结构性防线放这里，视图那两处只是"更早告诉用户"。
        //
        // ⚠️ **闸门在 `absorb` 之外**（与上面那条引擎闸门、`enqueue` 的体量守卫同款）：
        //    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发），
        //    喂进 `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` ——
        //    于是**粘错一次内容就杀掉并重启一次内核**。之所以写在这里（而不是像
        //    `enqueue` 那样在调用前判），是因为本函数的失败落点本来就分两种模式：
        //    `.initial` 写 `loadState`（空态页显示这句 + 「重试」），`.switching` 抛给面板。
        //    钉住它的是 `anOversizedDeliveryCodeNeverReachesTheKernel` 里的
        //    `factory.madeCount == 0`（没有新内核被造出来）与 `callCount(...) == 0`。
        //
        // ⚠️ 位置：必须在 `loadState = .loading` / `busyReason` **之前** ——
        //    否则会留下一个"没有任何请求在飞的转圈"（同 `enqueue` 那条守卫的纪律）。
        if DeliveryCodeEntry.tooLong(code) {
            let why = "交付码长 \(code.utf8.count) 字节，已超过安全上限（\(DeliveryCodeEntry.maximumText)）。"
                + "超限的请求内核不会回话、也不会报错，只会一直等下去——"
                + "请只粘贴交付码本身或交付页链接。"
            if mode == .initial { loadState = .failed(why) }
            throw CoreError.transport(why)
        }

        // ⚠️ **同一条请求上的另一个用户可输入字段**（空态页「高级：自定义下载地址」）：
        //    它与 `code` 装在**同一条** `load_delivery` 里，所以超了行长上限的后果
        //    一模一样，这里不能只挡一个（判据与上界都是 `DeliveryCodeEntry` 那一份 ——
        //    该类型管的是"用户手输进这条请求的串"，见它的注释）。
        //
        // ⚠️ 空串**不算**过长，而且它是合法值（`params["base_url"]` 在下游被跳过 ⇒
        //    内核用自己的默认交付服务器）。所以判据要先排掉空串，否则
        //    `isEmpty` 的语义会在两个地方各判一次、迟早分叉。
        if let baseURL, !baseURL.isEmpty, DeliveryCodeEntry.tooLong(baseURL) {
            let why = "自定义下载地址长 \(baseURL.utf8.count) 字节，已超过安全上限"
                + "（\(DeliveryCodeEntry.maximumText)）。超限的请求内核不会回话、也不会报错，"
                + "只会一直等下去——请确认粘贴的内容确实是交付页的地址。"
            if mode == .initial { loadState = .failed(why) }
            throw CoreError.transport(why)
        }

        // ② `loadState` **只在 `.initial` 时设**：换码不该把当前批次打回「正在加载」（那正是
        //    本任务要修的观感，转圈期间用户手上的清单会消失）。`busyReason` 两种模式都设 ——
        //    它是"正在做事"的指示（工具栏/侧边栏据此显示进行中），与"哪个批次在屏上"无关。
        if mode == .initial { loadState = .loading }
        busyReason = "正在加载交付清单…"
        defer { busyReason = nil }

        var params: [String: JSONValue] = ["code": .string(code)]
        if let baseURL, !baseURL.isEmpty { params["base_url"] = .string(baseURL) }

        do {
            let info = try await call("load_delivery", .object(params)).decoded(DeliveryInfo.self)
            // ⚠️ **换批即作废**：`verify` 是**按批次**的快照，而 `VerifyStatus` 自己没有交付码
            //    字段（协议里就没有），所以"这份结果属于哪一批"只能由壳记着 —— 记法就是
            //    `loadedCode`。不消掉它的后果是**一个没有任何提示的错数**：
            //    侧边栏「校验结果」那颗徽标（`SidebarBadge.unpassed(model.verify)`）与整个
            //    校验屏都直接读 `model.verify`，而 `performLoadDelivery` 从任务 3 到任务 8
            //    都没碰过 `verify` —— 用户加载 B 批后若一直待在「文件」/「传输列表」，
            //    徽标会**一直**挂着 A 批的未通过数（他完全可能读成"新这批有 3 个没通过"），
            //    而唯一的自愈路径是"进一次校验屏、且那一次刷新成功"。
            //    （任务 9 之前徽标恒 0，所以这是任务 9 引入的缺口，在任务 9 收掉。）
            //
            // ⚠️ **判据是"码变了没有"，不是"又 load 了一次"**：同一个码重复 load 是**刷新**
            //    （`loadDelivery` 的两条调用路径都可能这样：空态页那颗「重试」用的还是同一个码、
            //    重启恢复走的是 `performLoadDelivery(code: loadedCode)`），那时这份快照说的
            //    还是**这一批**，不该被抹掉。这条取舍与 `RootView` 里
            //    `onChange(of: loadedCode) { manifest.display(code:) }` 同一条纪律
            //    （换批才复位）。
            //
            // ⚠️ **一处如实记下的差异**：内核对 `k.verify` 是**每一次** `load_delivery` 都
            //    **无条件**重置的（`core/src/main.rs:878-880`，那一句没有 `code_changed` 守卫；
            //    同一段上方 `:850-856` 的 `code_changed` 只管**引擎任务**的清理）。
            //    所以"同批重载"之后内核手里那份已经是空的，而壳这里**有意留着**旧快照：
            //    那组数字说的是**同一批文件**（不是别的批次），语义是"这一批最后一次校验的结果"，
            //    把它也清掉会让一次「刷新」把用户正在看的校验结果抹掉。
            //    复审（重要 ①）要的是"**换批**不得挂旧数"，这一条满足；
            //    要更严的口径就把下面这行改成无条件 `verify = nil` ——
            //    `reloadingTheSameBatchKeepsItsVerifySnapshot` 会立刻变红，
            //    那条测试就是为这个决定立的桩。
            //
            // ⚠️ **这一行必须与 `loadState` 同帧落**（下面三行之间没有 `await`）：否则会出现
            //    "新批次已经上屏、徽标还挂着上一批的数"的中间帧 —— 那正是要修的东西。
            //    比较用的 `loadedCode` 是**上一批**的码，所以它必须在下面被改写**之前**读。
            if let previous = loadedCode, previous != info.code { verify = nil }
            // ③ 成功分支：这几行**两种模式都要走** —— 换码成功就是"提交"，它与首次加载
            //    没有区别（界面进新批次、记住的码换了、换批作废校验快照、拉新树）。
            loadState = .loaded(info)
            lastCode = info.code
            loadedCode = info.code
            // `base_url` 可能是空串（内核的 `expires_at` 那种"可能是空串"的字段同理），
            // 存成 nil 让下一次请求不带这个键、由内核用自己的默认值。
            loadedBaseURL = info.baseUrl.isEmpty ? nil : info.baseUrl
            // 阶段 E §1.2：**成功的加载**才记进壳的历史（失败的不记 —— 否则历史里
            // 会堆满打错的码）。时机与内核 `remember_last_code` 一致。
            //
            // ⚠️ 放在这几行里、且**与它们之间没有 `await`**：历史与"当前批次"同帧落定
            //    （不会出现"界面已经是新批次、历史还是旧的"的中间帧）。
            // ⚠️ 传的是**这次请求用的** `baseURL`，不是内核回显的 `info.baseUrl`
            //    —— 理由写在 `recordLoadedBatch`。
            recordLoadedBatch(code: info.code, baseURL: baseURL ?? "")
            // 换批次后 tree 必须跟着换（`getTree` 自己会把 `self.tree` 刷成新的）
            // —— 留着上一批的树 = 客户点了下载下到别批的文件。
            //
            // ⚠️ **树的成功与否决定这一次加载算不算"一代"**（复审重要 1，这一行是承重的）：
            //    `surfacing` 会**吞掉错误**并返回 nil，而 `getTree` 只在**成功**时才写
            //    `tree`/`treeCode`（失败时旧快照原样留着）。不看这个返回值就推进代数的话：
            //      ① 代数从 1 推到 2；
            //      ② `treeCode` 还是同一个码 —— 那是**同一批的陈旧树**，而
            //         `ManifestTracking.seed` 唯一的新鲜度守卫 `treeCode == code`
            //         对它**没有判别力**（它只比"哪一批"，比不出"同一批、但上一代的那棵树"）；
            //      ③ 于是视图拿**上一棵树**的 `default_selected` 播下去 ——
            //         拿一份过期结论当现状，正是本任务要消灭的形态。
            //    所以"代数只在**整次加载（含 `getTree`）成功**之后才推进"必须是真的，
            //    下面 `loadGeneration` 的文档里承诺的不变量才成立
            //    （钉住它的是 `aFailedTreeFetchDoesNotAdvanceTheGeneration`）。
            //
            // ⚠️ **代价（如实记下）**：树没取到时代数也不推进 ⇒ 屏幕上那条下载回执
            //    **不会**被这次加载清掉。可接受，那两条路上界面都**不是静默的**：
            //    `getTree` 走的是 `absorbing`/`surfacing`，而这两条路的失败**必落成
            //    顶部横幅或退回空态**（逐条分派见 `absorb`，这里**不逐条复述** ——
            //    下面那句只说"不是静默"，因为 Ruling D7 那条窄口子就押在它上面）。
            //    用户看到的是"引擎出问题了"或"这批没了"，而不是"那句旧结论还在冒充现状"。
            //
            //    ⚠️ **这里曾经把分派说反**（最终审查顺手 2）：原文写的是"其余错误码 →
            //    `lastError`"，而 `absorb` 的实情是：`transport` → `onKernelDeath`
            //    （引擎横幅）；`engine_disconnected` / `engine_start_failed` /
            //    `protocol_mismatch` → `.absorbed` → **也是引擎横幅**（不是 `lastError`）；
            //    `engine_not_started` / `no_delivery` → `.absorbed` → **什么都不写**
            //    （后者退回空态）；**只有** `engine_rpc_failed` / `malformed_response`
            //    才真的走 `lastError`（`surfacing` → `surface`）。
            let fetchedTree = await surfacing { try await self.getTree() }
            guard fetchedTree != nil else { return info }
            // ⚠️ 这一代的加载落定（视图靠它的变化知道"刚刚成功加载过一次"——
            //    同码重载时它是**唯一**会动的信号：码、树码、`loadState` 全都逐字不变）。
            loadGeneration += 1
            return info
        } catch let e as CoreError {
            // ④ 失败分支：「其余 → 内核原文」。两种模式下都**照旧过 `absorb`** ——
            //    内核死了（transport）那件事必须被处置（提示 + 至多一次重启），
            //    它与"哪个模式在加载"无关；分岔的只是**要不要写 `loadState`**。
            switch await absorb(e) {
            case .verbatim(let text):
                // `.switching` 时**一个字都不写**：那句话交给换码面板显示，
                // 当前批次（`loadState`）原样留在屏上（本任务的规格 §3.1 第 3 条）。
                if mode == .initial { loadState = .failed(text) }
            case .absorbed:
                // 走到这里的是 transport（内核在拉清单的途中死了 —— 那正是 91.5 秒的窗口，
                // 也正是规格 §9 存在的理由）以及引擎家族的码。它们的原文**要么**已经被
                // 重启流程处理掉（重启成功 + 视图恢复 ⇒ 里面那次 `performLoadDelivery`
                // 已经把 `loadState` 写成 `.loaded`），**要么**一个字都没写。
                //
                // ⚠️ 少了这段兜底，界面就永远停在 `.loading`：`busyReason` 已经被 `defer`
                //    清掉，于是那是一个**没有任何请求在飞的转圈**，而 `loadState` 是
                //    `.loading` 这件事没有任何用户动作能改掉（再点一次加载还是同一条路）。
                //    只有"还停在 loading"时才兜底 —— 否则会盖掉重启流程刚恢复好的视图。
                // `.switching` 时 `mode == .initial` 为假 ⇒ 连这条兜底都不进：
                // 它自己的 `loadState` 从来没被置成 `.loading`（见 ②），没什么可兜的。
                if mode == .initial, case .loading = loadState {
                    loadState = .failed(Self.message(of: e))
                }
            }
            throw e
        } catch {
            if mode == .initial { loadState = .failed("\(error)") }
            throw error
        }
    }

    // MARK: - 文件树

    /// **引擎不可用时，绝不把请求发出去**（任务 6 的最小防线，约束 4 / 15）。
    ///
    /// 理由与 `performLoadDelivery` 里那条**完全相同**：内核一旦卡死（首次启动卡在 TCC
    /// 授权框上的 `open()` 里，任务 4b 查明），那条 `hello` **永不返回**，而 `CoreClient`
    /// 内部是一条 FIFO **串行**队列（约束 15 的单飞语义就靠它）—— 队列被永久堵死，
    /// 此后每个新请求都排在那条后面，永远轮不到。
    /// 任务 5 只堵了 `load_delivery` 一条路；`get_tree` / `list_dir` 是**新的挂死入口**：
    /// 用户在引擎断开后双击一个目录，会看到界面**永远停在加载态**（约束 4 明禁的静默失效）。
    ///
    /// 形态沿用 `call()` 里那条现成的 "`client == nil` → `CoreError.transport(内核那句原文)`"
    /// （**不发明新的错误码**，约束 2；原文逐字照登，约束 3）。
    ///
    /// ⚠️ **闸门在 `absorbing` 之外**，这一点是刻意的、不是随手放的：
    ///    这个错误是**壳自己造的**（内核没死，只是我们不发），把它喂进 `absorb`
    ///    会走 `.transport` 那一支 → `onKernelDeath` → `restartIfAllowed` ——
    ///    于是**双击一个目录就会重启一次内核**，还把 `engine` 改写成
    ///    "下载引擎已断开：<原来那句>"。在握手超时那种现场里尤其糟：
    ///    那正是 `start()` 里写明「不自动重启」的状态（用户正对着系统授权框，
    ///    重启会把框底下的进程换掉），`EngineStatusPresentation.isHandshakeTimeout`
    ///    存在的意义就是"这一句要留给用户看一眼，然后由他点「重试」"。
    ///    放在外面 ⇒ 错误照样抛给**视图那两个入口**的调用方在原地显示（约束 4）。
    ///    钉住它的是 `listDirWithAnUnavailableEngineFailsFastWithoutCallingTheKernel`
    ///    里的 `factory.madeCount == 0`（"没有新内核被造出来"）。
    ///
    /// ⚠️ **准确的说法是"视图那两个入口不会触发重启"，不是"这个错误永远不会被 absorb"。**
    ///    它挡不住的是**别的**调用点：`performLoadDelivery` 里那条
    ///    `_ = await surfacing { try await self.getTree() }` —— `surfacing` 对**任何**
    ///    `CoreError` 都调 `absorb(e)`，于是同一个闸门错误在那条路上会走
    ///    `onKernelDeath → restartIfAllowed`（也就是会重启）。
    ///    **今天走不到**：`performLoadDelivery` 的第一句已经对 `.unavailable` 早退，
    ///    `engine` 不可能是 `.unavailable` 却还跑到那一步。
    ///    **但这是"当前调用点恰好不会"，不是"结构上不可能"** —— 任务 8 引入轮询/重试并发、
    ///    或者把 `surfacing` 的适用范围改宽，这里立刻变成现成的坑。
    ///    真要做成结构性的保证，得让"壳自己造的错误"带一个可识别标记、
    ///    或者让 `surfacing` 只吸收"内核真的回了话"的错误 —— 那属于任务 8 的准入闸门。
    ///
    /// ⚠️ 判据只看 `.unavailable`，**不看 `.unknown`**：`.unknown` 是"握手还没结论"，
    ///    那时请求排在握手后面是**正常**的（会轮到），而且握手有 5 秒上界、
    ///    超时后引擎自然落到 `.unavailable`、由这里兜住（与 `performLoadDelivery` 同）。
    ///
    /// ⚠️ **任务 8 之后的状态**（免得下一个读注释的人以为这里还欠着什么）：
    ///    - 四条防线齐了：`loadDelivery` / `getTree`+`listDir` / `enqueue` / `taskAction`；
    ///    - 界面侧多了一道更严的准入（`EngineGate`：`.unavailable` 与 `.unknown` 都关），
    ///      配顶部横幅 + 「重试」（`EngineBanner` / `RootView`）—— 那是 4b 那句
    ///      「内核无响应（等待超过 5 秒）」的 tooltip 所指的那颗按钮；
    ///    - 上面那条"`surfacing` 会吸收壳自己造的错误"的**洞仍然在**：任务 8 没有改
    ///      `surfacing` 的语义（改它要动任务 3/5/6 已复审的行为），也没有把闸门错误
    ///      喂进任何 `surfacing` 调用点。它今天的唯一入口仍是 `performLoadDelivery`
    ///      里那条 `getTree`（早退挡着），而任务 8 新增的轮询走的是**另一条**路
    ///      （`fetchTransfers` 直接调 `call`，不经过 `requireUsableEngine`）。
    private func requireUsableEngine() throws {
        if case .unavailable(let why) = engine {
            throw CoreError.transport(why)
        }
    }

    /// 取交付清单的树快照，并**顺便刷新 `self.tree`**。
    @discardableResult
    public func getTree() async throws -> TreeResult {
        try requireUsableEngine()
        let fresh = try await absorbing {
            try await call("get_tree", .null).decoded(TreeResult.self)
        }
        tree = fresh
        treeCode = loadedCode
        // ⚠️ `tree` 与"它属于哪一批"必须**一起**落，两行之间**没有 await**（所以不会有
        //    中间帧让视图看到"树是这一批的、treeCode 还是上一批的"）。
        //    这一个字段就是那场竞速的防线：`AppModel` 先落 `loadState = .loaded(info)`、
        //    之后才拉 `get_tree`，而视图是在 `.loaded` 那一刻出现的 —— 视图据此知道
        //    "手上这棵树是不是这一批的"，不是的话就**推迟**播种（`BrowserSelection.onNewManifest`）。
        return fresh
    }

    /// 某个目录的**直接**子项。`path` 是清单里的原文路径（根是空串）——
    /// 壳不规范化它（约束 3），内核自己会 `trim_matches('/')`。
    public func listDir(_ path: String) async throws -> ListDirResult {
        try requireUsableEngine()
        return try await absorbing {
            try await call("list_dir", .object(["path": .string(path)])).decoded(ListDirResult.self)
        }
    }

    // MARK: - 传输列表

    /// 200 ms 定时器的入口（任务 8 的传输列表用它）。
    ///
    /// **上一拍没回来就跳过这一拍**（全局约束 15）—— **不排队**：排队会让长操作
    /// （如 `enqueue` 500 个文件）后面堆几百条轮询，全部在操作结束后一次性返回，
    /// 界面先冻结、再抖动。跳拍则让界面保持"上一帧数据 + 一个进行中的指示"。
    ///
    /// ⚠️ 标志在**第一个 `await` 之前**置位、结束后清除（见 `fetchTransfers`），
    /// 中间不会有第二条请求溜进来。`pollInFlight` 就是这条纪律的落点，
    /// `pollSkipsATickWhenThePreviousOneIsStillRunning` 与 `atMostOneRequestIsInFlight`
    /// 守着它。
    ///
    /// ⚠️ 这里**没有超时、也不重试**：内核活着但不回答时，表现就是"界面活着、
    /// 数据停在上一次成功的快照上、不报错" —— 这正是约束 15 想要的观感。
    /// 加超时/重试等于把业务判断塞进壳里（约束 1）。
    public func pollTick() async {
        guard !pollInFlight else { return }
        guard case .running = engine else { return }
        await fetchTransfers()
    }

    /// 立即刷新一次（任务 8 的"动作成功后不等下一拍"）。
    ///
    /// 与 `pollTick` **共用同一条单飞纪律**：正在飞的那条不打断，也不排第二条。
    /// 它**没有**"引擎必须在跑"这道理 —— 用户显式要看传输列表时，正该让内核
    /// 回一句 `engine_not_started` 好把空态显示出来（见 `engineNotStartedIsNotAnError`）。
    public func refreshTransfers() async {
        guard !pollInFlight else { return }
        await fetchTransfers()
    }

    private func fetchTransfers() async {
        // ⚠️ 置位必须在第一个 await **之前**：`pollTick`/`refreshTransfers` 是主 actor 上
        //    同步进来调它的，所以这中间没有别的代码能插进来。
        pollInFlight = true
        defer { pollInFlight = false }

        let fresh = await surfacing {
            try await call("transfer_list", .null).decoded(TransferListResult.self)
        }
        guard let fresh else { return }        // 失败：保持上一帧快照，不报错（约束 15）
        transfers = fresh
        // 拿到了快照 ⇔ 内核的引擎真的在跑（`op_transfer_list` 走 `read_engine`，
        // 没有引擎它只会回 engine_not_started）。所以这一句也是"引擎活过来了"的观测点。
        // ⚠️ **值没变就不写**：这是三处护栏里最要紧的一处 —— 它**每一拍（200 ms）**都会走到，
        //    而引擎状态绝大多数时候就是 `.running`。无条件写的话，`RootView` / `Sidebar` /
        //    `TransfersView` 会跟着每一拍全量重算 body。
        if engine != .running { engine = .running }
    }

    // MARK: - 下载与任务动作

    /// 加任务。`paths` **原样**交给内核（`[]` = 下全部待下载）——目录由内核按前缀展开，
    /// 壳不自己展开（约束 1）。
    ///
    /// ⚠️ **引擎不可用时快速失败，不把请求发出去**（与 `getTree` / `listDir` 同一条最小防线）。
    ///    理由**完全相同**：内核一旦卡死（首次启动卡在 TCC 授权框上的 `open()` 里，任务 4b 查明），
    ///    `CoreClient` 内部那条 FIFO **串行**队列会被那条永不返回的请求**永久堵死**（约束 15），
    ///    此后每个新请求都排在那条后面、永远轮不到。`enqueue` 是**又一个**这样的挂死入口：
    ///    用户点「下载」/ 双击一个文件，看到的是一颗按下去再也没有回音的按钮。
    ///
    /// ⚠️ **闸门在 `absorbing` 之外**（这一点是刻意的，与 `listDir` 逐字同款）：
    ///    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发）。把它喂进
    ///    `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` —— 于是
    ///    **点一次「下载」就杀掉并重启一次内核**，还把 `engine` 改写成
    ///    "下载引擎已断开：<原来那句>"。用户只是点了一下下载，凭什么换掉他的内核进程？
    ///    钉住它的是 `enqueueWithAnUnavailableEngineFailsFastWithoutCallingTheKernel`
    ///    里的 `factory.madeCount == 0`。
    ///
    /// ⚠️ 判据只看 `.unavailable`，**不看 `.unknown`**（`.unknown` = 握手还没结论，
    ///    那时请求排在握手后面是正常的，与 `performLoadDelivery` / `getTree` 同）。
    ///
    /// ⚠️ **还有一条体量守卫**（本任务，全局约束 C-3）：请求体超过安全预算时**拒发**。
    ///    它与上面那条引擎闸门的纪律逐字同款 —— **闸门在 `absorbing` 之外**，理由见下。
    ///
    ///    **为什么必须拒发**：内核的行长上限是 8 MiB（`core/src/main.rs:113`），
    ///    而它的超限处置**不是报错** —— `:1657-1661` 只回一条 `id == 0` 的协议告警，
    ///    那条请求**永远等不到响应**，`CoreClient` 那条 FIFO **串行**队列于**被永久堵死**
    ///    （约束 15），而界面上一个字都不说。这正是约束 4 明禁的静默失效，
    ///    且它的形态是"应用整个卡死"，比"这一次下载没成功"严重得多。
    ///
    ///    **为什么这句话由壳来写**（约束 3 的例外，理由写在这里）：内核**一个字都没说** ——
    ///    它根本收不到这条请求，没有"原文"可登。而"点了下载、界面毫无反应"是绝对不能接受的。
    ///    所以壳把**用户该怎么做**说清楚（「全选」或分批），而不是把 8 MiB 这种内核参数
    ///    当成人话发给客户。文案里的 8 MiB 是内核的真实上界（不是预算值），
    ///    好让人对得上"大概多少项会超"。
    ///
    ///    ⚠️ 判据用的是**编码后的字节数**（`DownloadTargets.requestBytes`，与线上同一个编码器），
    ///    不是"项数"的估算 —— 估算要么误杀合法请求、要么放过超限请求。
    ///
    ///    ⚠️ **闸门在 `absorbing` 之外**（同上面那条引擎闸门，这点是刻意的）：
    ///    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发），
    ///    喂进 `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` ——
    ///    于是**勾多了几个文件就会杀掉并重启一次内核**，还把 `engine` 改写成
    ///    "下载引擎已断开：<原来那句>"。用户只是勾多了几项，凭什么换掉他的内核进程？
    ///    钉住它的是 `anOversizedEnqueueNeverReachesTheKernel` 里的 `factory.madeCount == 0`。
    public func enqueue(paths: [String]) async throws -> EnqueueResult {
        try requireUsableEngine()
        guard !DownloadTargets.exceedsRequestBudget(paths) else {
            // ⚠️ 措辞**必须与事实一致**（复审顺手修 ②）：触发这条守卫的是**壳自己的安全预算**
            //    （`DownloadTargets.requestBudgetBytes` = 4 MiB），不是内核那 8 MiB 上限 ——
            //    说"超过内核的 8 MiB 上限"会让 5 MiB 的请求（它并不超限）读到一句假话，
            //    对着日志排障的人会得出错误的结论。所以：说"超过安全预算"，
            //    同时把内核的真实上界（8 MiB）括起来给出，人才对得上"大概多少项会超"。
            throw CoreError.transport(
                "勾选了 \(paths.count) 项，这一条请求已超过安全预算（内核上限 8 MiB）。"
                + "超限的请求不会报错，只会一直等下去——请改用「全选」下载整批，或分批下载。")
        }
        busyReason = "正在添加下载任务…"
        defer { busyReason = nil }
        let result = try await absorbing {
            try await call("enqueue", .object(["paths": .array(paths.map { .string($0) })]))
                .decoded(EnqueueResult.self)
        }
        // 成功 ⇒ 内核在 `op_enqueue` 里已经 `ensure_engine` 过了。
        engine = .running
        return result
    }

    /// 暂停 / 继续 / 重试 / 移除 / 清空已完成。`action` 需要 gid 时由调用方给。
    ///
    /// ⚠️ **引擎不可用时快速失败，不把请求发出去**（任务 8 补上的第四条防线，
    ///    与 `getTree` / `listDir` / `enqueue` 逐字同款）。理由**完全相同**：内核一旦卡死，
    ///    `CoreClient` 内部那条 FIFO **串行**队列会被那条永不返回的请求**永久堵死**
    ///    （约束 15），此后每个新请求都排在那条后面、永远轮不到 —— 用户点「暂停 / 移除」
    ///    看到的是一颗按下去再也没有回音的按钮（约束 4 明禁的静默失效）。
    ///    界面上这些菜单项同时是**禁用**的（`EngineGate`），这里是结构性的第二道：
    ///    渲染与点击之间 `engine` 可能刚好翻成 `.unavailable`，那一下不能挂死。
    ///
    /// ⚠️ **闸门在 `absorbing` 之外**（同 `listDir` / `enqueue`，这点是刻意的）：
    ///    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发），把它喂进
    ///    `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` ——
    ///    于是**点一次「暂停」就杀掉并重启一次内核**，还把 `engine` 改写成
    ///    "下载引擎已断开：<原来那句>"。用户只是点了下暂停，凭什么换掉他的内核进程？
    ///    钉住它的是 `taskActionWithAnUnavailableEngineFailsFastWithoutCallingTheKernel`
    ///    里的 `factory.madeCount == 0`。
    public func taskAction(_ a: TaskAction, gid: String?) async throws {
        try requireUsableEngine()
        var params: [String: JSONValue] = ["action": .string(a.rawValue)]
        if let gid, !gid.isEmpty { params["gid"] = .string(gid) }
        _ = try await absorbing { try await call("task_action", .object(params)) }
        // 同上：动作成功 ⇔ `require_engine_for_action` 过了。
        engine = .running
    }

    // MARK: - 参数面板

    /// 写参数。**必须**用 `CoreJSON.requestEncoder`（`.convertToSnakeCase`）—— 属性名是
    /// camelCase，而内核 `set_settings` 走 `serde_json::from_value::<Settings>` 且
    /// `Settings` 没有任何 `rename`。发错键名出去的 payload 看起来完全正常（也是七个键），
    /// 内核只会回一条 `invalid_params`。
    ///
    /// ⚠️ **引擎不可用时快速失败，不把请求发出去**（任务 10 补上的第六条防线，与
    ///    `getTree` / `listDir` / `enqueue` / `taskAction` / `refreshVerify` 逐字同款）。
    ///    理由**完全相同**：内核一旦卡死（首次启动卡在 TCC 授权框上的 `open()` 里，
    ///    任务 4b 查明），`CoreClient` 内部那条 FIFO **串行**队列会被那条永不返回的请求
    ///    **永久堵死**（约束 15），此后每个新请求都排在那条后面、永远轮不到 ——
    ///    而设置窗口是**唯一**能改内核参数的地方，这条路静默失效的后果是"参数再也改不了"，
    ///    用户看到的是一颗按下去没有回音的「保存」（约束 4 明禁的静默失效）。
    ///    界面上那颗按钮同时是**禁用**的（`EngineGate`），这里是结构性的第二道：
    ///    渲染与点击之间 `engine` 可能刚好翻成 `.unavailable`，那一下不能挂死。
    ///
    /// ⚠️ **闸门在 `absorbing` 之外**（同 `listDir` / `enqueue` / `taskAction`，刻意的）：
    ///    这个 `CoreError.transport` 是**壳自己造的**（内核没死，只是我们不发），把它喂进
    ///    `absorb` 会走 `.transport` → `onKernelDeath` → `restartIfAllowed` ——
    ///    于是**点一次「保存」就杀掉并重启一次内核**，还把 `engine` 改写成
    ///    "下载引擎已断开：<原来那句>"。用户只是改了个参数，凭什么换掉他的内核进程？
    ///    钉住它的是 `applySettingsWithAnUnavailableEngineFailsFastWithoutCallingTheKernel`
    ///    里的 `factory.madeCount == 0`。
    ///
    /// ⚠️ 判据只看 `.unavailable`，**不看 `.unknown`**（与 `requireUsableEngine` 同）：
    ///    `.unknown` 是"握手还没结论"，那时请求排在握手后面是正常的（握手有 5 秒上界）。
    public func applySettings(_ s: Settings) async throws {
        try requireUsableEngine()
        let echoed = try await absorbing { () -> SettingsResult in
            let data = try CoreJSON.requestEncoder.encode(s)
            let payload = try CoreJSON.decoder.decode(JSONValue.self, from: data)
            return try await call("set_settings", .object(["settings": payload]))
                .decoded(SettingsResult.self)
        }
        // 落状态的是**内核回执**那一份（`-k` 会被内核归一成规范串）。
        settings = echoed.settings
        lastCode = echoed.lastCode
    }

    // MARK: - 校验

    /// 取一次 `verify_status`（校验视图的刷新按钮与"切到本视图"共用它）。
    ///
    /// ⚠️ **引擎不可用时一个请求都不发**（任务 9 补上的第五条防线，与
    ///    `getTree` / `listDir` / `enqueue` / `taskAction` 逐字同款）。理由**完全相同**：
    ///    内核一旦卡死（首次启动卡在 TCC 授权框上的 `open()` 里，任务 4b 查明），
    ///    `CoreClient` 内部那条 FIFO **串行**队列会被那条永不返回的请求**永久堵死**
    ///    （约束 15），此后每个新请求都排在那条后面、永远轮不到 ——
    ///    用户点「刷新」看到的是一颗再也没有回音的按钮（约束 4 明禁的静默失效）。
    ///
    /// ⚠️ 这一条的**出口不是 `throws`**（与那四条不同）：它没有调用方可以 catch，
    ///    原文落在 `lastError` 上、由主区顶部那条常驻横幅显示。所以这里不能写成
    ///    `try requireUsableEngine()` 放进 `surfacing`：`surfacing` 对**任何** `CoreError`
    ///    都调 `absorb`，而这条 `transport` 是**壳自己造的**（内核没死，只是我们不发）——
    ///    喂进去会走 `.transport` → `onKernelDeath` → `restartIfAllowed`，于是
    ///    **点一次「刷新」就重启一次内核**，还把 `engine` 改写成"下载引擎已断开：…"。
    ///    所以闸门是「不发 + 直接返回」：`engine` 里那句原文照旧留在界面上（约束 3/4）。
    ///
    /// ⚠️ 判据只看 `.unavailable`，**不看 `.unknown`**（与 `requireUsableEngine` 同）：
    ///    `.unknown` 是"握手还没结论"，请求排在握手后面是正常的，而且握手有 5 秒上界。
    ///    界面侧另有一道更严的准入（`EngineGate`：`.unknown` 也关），两者互补。
    public func refreshVerify() async {
        if case .unavailable = engine { return }
        // 失败时保留上一份结果（与传输列表同一条：数据停在上一次成功的快照上）。
        if let fresh = await surfacing({ try await call("verify_status", .null).decoded(VerifyStatus.self) }) {
            verify = fresh
        }
    }

    // MARK: - 收尾

    /// 收尾。**唯一的同步方法**：`CoreClient.shutdown()` 自己有上界（最坏约 10 秒）
    /// 且幂等，而它由 `applicationWillTerminate` 调用 —— 那里没有可以 await 的余地（任务 4）。
    public func shutdown() {
        client?.shutdown()
    }

    // MARK: - 内核崩溃：提示 + 至多一次自动重启

    /// 内核进程没了（管道结束 / 写失败）时的处置。规格 §9 的三步都在这里：
    ///   ① 提示「下载引擎已断开」—— **不得静默失效**（约束 4）；
    ///   ② **自动重启一次**：新客户端 → `start()` → 恢复交付码与视图，
    ///      但**不自动恢复传输列表**（规格 §8.3：断点续传恒开，用户重新点下载即可接着传）；
    ///   ③ 重启失败（或本次已是重启后的失败）不再重试，置 `.unavailable` 等用户动作。
    private func onKernelDeath(_ reason: String) async {
        engine = .unavailable("下载引擎已断开：\(reason)")
        await restartIfAllowed()
    }

    /// 自动重启的闸门。**`restartAttempted` 的复位时机是简报留给我判断的那一条**：
    ///
    /// - 置位：在**第一次 `await` 之前**（下面没有 await 抢在它前面），所以并发的
    ///   第二条崩溃也只会看到 `true`。
    /// - **成功的自动重启不复位**。理由：重启里要跑 `start()`，而 `start()` 自己也可能
    ///   拿到 transport 错误 —— 一复位就是"崩 → 重启 → 崩 → 重启"的自我续命循环。
    ///   `kernelDeathRestoresTheLoadedDelivery` 那条路（重启后立刻重拉 91.5 秒的清单）
    ///   尤其经不起这个循环。
    /// - **只在用户显式开始新一轮时复位**：`loadDelivery`（换交付码 / 重新加载）与
    ///   `retryEngine`（用户点「重试」）。也就是说「一次自动重启」的预算是**按用户动作**
    ///   计的，不是按 app 生命周期计的 —— 内核在几小时后死在另一批上时，用户重新加载
    ///   就会拿到一次新的自动重启。
    private func restartIfAllowed() async {
        guard !restartAttempted, !restarting else { return }
        restartAttempted = true
        await restartKernel()
    }

    /// 用户显式重试（界面上那颗「重试」）：复位预算，再走一次重启。
    public func retryEngine() async {
        restartAttempted = false
        await restartKernel()
    }

    /// ⚠️ **返回"这一次重启真的跑了没有"**：`restarting` 闸会让调用被静默弹回，
    ///    而 `changeDownloadDir` 必须知道这件事 —— 被弹回意味着"内核还是旧目录"，
    ///    那时报"已改成"就是一句假话（见那里第 ③ 步的注释）。
    ///    另两个调用点（`restartIfAllowed` / `retryEngine`）不看返回值。
    @discardableResult
    private func restartKernel() async -> Bool {
        guard !restarting else { return false }
        restarting = true
        busyReason = "下载引擎已断开，正在重启…"
        defer { restarting = false; busyReason = nil }

        // ⚠️ **先收尾旧的客户端，再新建**（任务 4b）——顺序不能反，也不能省：
        //    超时那条路上旧内核正卡在 `open()` 里（等 TCC 授权框）。直接把它丢掉 = 留下一个
        //    **孤儿内核**；而用户随后点了「允许」，那个旧内核还会醒过来 —— 于是**同时跑着
        //    两个内核**，两个都在 `~/Downloads/Benagen` 上写同一个状态文件。
        //    `CoreClient.shutdown()` 自带**有界**等待（2 秒后强制收尾：关 stdin → SIGTERM →
        //    SIGKILL）且**幂等**，所以先调它是安全且必要的。
        //
        // ⚠️ 但**不许在主 actor 上同步调它**（复审推翻了简报里"直接调"的那条指示）：
        //    卡死这条路上 `shutdown()` **必然走强制收尾** —— 2 秒等不到队列（那条请求正卡在
        //    `open()` 里）→ `channel.close()` → 关 stdin → 等 3 秒 → SIGTERM → 等 2 秒，
        //    合计**约 5 秒**（`CoreClient.shutdown()` 自己的文档写的是最坏约 8 秒）。
        //    这 5 秒正好落在**用户刚点完「重试」之后**：界面会冻住，而用户会以为应用又卡死了 ——
        //    那恰恰是本任务要消除的观感（⌘Q 那条路无所谓，它发生在退出时）。
        //    所以把同步调用挪到主 actor **之外**：`CoreCalling` 是 `Sendable`、`shutdown()` 幂等，
        //    这样挪是安全的；`await ….value` 仍然保证"旧客户端收尾完之后才新建"这个**顺序**
        //    （顺序是硬要求：防孤儿内核、防同时跑两个内核）。
        if let dying = client {
            await Task.detached { dying.shutdown() }.value
        }

        do {
            client = try await spawnClient()
            // 上一个内核的传输列表属于上一个内核：不自动恢复（规格 §8.3）。
            transfers = nil
            await start()
            if case .unavailable(let why) = engine {
                // ⚠️ 壳自己那句**超时**文案必须**逐字放行**，不能加前缀（复审重要 ①）：
                //    `EngineStatusPresentation.isHandshakeTimeout` 是**逐字相等**的判别式，
                //    一旦被包成"内核重启失败：内核无响应（等待超过 5 秒）"，判别式立刻失效 ——
                //    徽标退回那条长一倍、且**浮层里那句「去看屏幕上有没有授权框、点「允许」、
                //    再点「重试」」的整段指导会消失**。
                //    而这一刻恰恰是最需要它的现场：用户按提示点了「重试」，内核**还卡在同一张
                //    授权框上**，第二次握手同样超时（`retryEngine` 与内核断开后的自动重启
                //    都会走到这里）。此时界面上唯一能告诉他"该去点什么"的信息就是那句话。
                //    另一条路（把它改成前缀/后缀匹配）是错的：那会牺牲判别式的精确性，
                //    并把这份脆弱性复制到两个地方。
                engine = why == Self.handshakeTimeoutMessage
                    ? .unavailable(why)
                    : .unavailable("内核重启失败：\(why)")
                // 这一次重启**跑了**（旧内核被收尾、新进程被起过）—— 只是它没能说话。
                return true
            }
            guard let code = loadedCode else { return true }
            // `.initial`：失败写 `loadState`（与改动前逐字相同），抛出来的那份在这里丢掉。
            _ = try? await performLoadDelivery(code: code, baseURL: loadedBaseURL)
        } catch {
            engine = .unavailable("内核重启失败：\(Self.message(of: error))")
        }
        return true
    }

    /// 造一个新客户端。`CoreClient.live()` 要起子进程（阻塞几十毫秒），所以甩到后台 ——
    /// 重启路径同样不许把主 actor 焊住（裁定 ① 的同一条理由）。
    ///
    /// ⚠️ **`--download-dir` 的值在这里、也在 `live()` 里算**（同一份判据
    ///    `DownloadDirectory.argument(for:)`），而且它读的是**此刻**的 `downloadDir`
    ///    —— 这正是"改目录之后重启出来的内核拿到的是**新**目录"的实现方式：
    ///    只有一处闭包、两个调用点，没有第二个"当前目录"的副本。
    private func spawnClient() async throws -> any CoreCalling {
        let make = makeClient
        let dir = DownloadDirectory.argument(for: AppPreferences(downloadDir: downloadDir))
        return try await Task.detached { try make(dir) }.value
    }

    // MARK: - 错误映射

    /// 一条内核错误的处置结果。
    private enum Outcome {
        /// 已经变成某个状态（引擎徽标 / 空态 / 重启流程）—— **调用方不要再编文案**。
        case absorbed
        /// 没被状态吸收：把**内核原文**呈现出来（抛错路径抛给调用方，其余路径落 `lastError`）。
        ///
        /// ⚠️ 落点**不是** `engine`（这里曾经写错成它）：`engine` 是"当前连接的事实"，
        ///    而 `pollTick` 的准入就是 `guard case .running = engine` —— 让一次瞬时的
        ///    轮询失败去写 `engine` 等于自己把轮询永久关掉（见 `surface` 那段注释）。
        case verbatim(String)
    }

    /// **内核错误的唯一处置点**，逐条对着设计规格附录 A 的处置列。
    ///
    /// ⚠️ 分支一律看**结构化 `code`**，不看 `message` 的措辞（契约 §5.1）。
    @discardableResult
    private func absorb(_ error: CoreError) async -> Outcome {
        switch error {
        case .transport(let why):
            await onKernelDeath(why)
            return .absorbed
        case .malformedResponse(let why):
            // 内核还在，但它发来的东西不是一条合法响应 —— 这是壳/内核之间的契约问题，
            // 不是引擎状态，原文照样得让人看见。
            return .verbatim(why)
        case .rpc(let code, let message):
            switch ErrorCode(rawValue: code) {
            case .engineNotStarted?:
                engine = .notStarted      // 正常态，**不是错误**
                transfers = .empty        // 传输列表回空态
                return .absorbed
            case .engineDisconnected?, .engineStartFailed?:
                engine = .unavailable(message)   // 内核原文，一个字都不改
                return .absorbed
            // ⚠️ `engine_rpc_failed` **不在**这两条里，它走下面的 `default`（附录 A 的
            //    处置列：「显示原文」）。内核那边的语义是**瞬时可重试**：`on_rpc_failure`
            //    只在**探活成功**时才回这条码（`core/src/main.rs`），消息原文就是"可重试"。
            //    把它判成 `.unavailable` 的后果是：`pollTick` 的准入是
            //    `guard case .running = engine`，一次瞬时的 RPC 抖动就会**永久关掉轮询**，
            //    而自愈恰恰需要一次成功的 `transfer_list` —— 它再也进不去了。
            case .protocolMismatch?:
                // 壳自己写文案的地方之一（不是唯一一处，另一处是 `start()` 的握手超时）：
                // 内核那句"内核是 1，请求方是 2"客户看不懂，
                // 而这句是客户唯一能照着做点什么的话。**不重试** —— 重启不会让版本变对
                // （自动重启只由 transport 触发，这一条天然不会被重试）。
                engine = .unavailable("客户端与内核版本不匹配，请更新客户端或内核后重试（内核：\(message)）")
                return .absorbed
            case .noDelivery?:
                resetToEmptyState()      // 批次已经作废/还没加载 → 回空态
                return .absorbed
            default:
                return .verbatim(message)
            }
        }
    }

    /// 非抛错路径上"没被状态吸收"的原文的唯一出口 —— 落 `lastError`，**不碰 `engine`**。
    ///
    /// 轮询与校验刷新没有调用方可以 catch，而 `loadState` 属于交付清单、不该被轮询的错误
    /// 改写，所以它们需要 `lastError` 这个出口（约束 4：不得静默失效）。
    /// 反过来说：**也不能写 `engine`** —— `pollTick` 的准入就是 `engine == .running`，
    /// 一次瞬时失败（`engine_rpc_failed` / 一行垃圾）写出 `.unavailable` 之后，
    /// 轮询再也进不去，而它自己正是唯一的自愈路径。
    private func surface(_ text: String) {
        lastError = text
    }

    /// 回空态：内核说"没有生效的批次"时，界面必须退回到"还没加载"的样子，
    /// 而不是继续显示一批已经不存在的文件。
    private func resetToEmptyState() {
        loadState = .idle
        tree = nil
        treeCode = nil        // 树与"它属于哪一批"必须一起清（同 `getTree` 的纪律）
        transfers = nil
        verify = nil
        loadedCode = nil
        loadedBaseURL = nil
    }

    /// `EngineState` → 一句人话（"内核没有起来"那条 transport 错误用它）。
    private var engineReason: String {
        switch engine {
        case .unknown: return "内核还没有握手"
        case .notStarted: return "下载引擎尚未启动"
        case .running: return "内核正在运行"
        case .unavailable(let why): return why
        }
    }

    /// `CoreError` → 给用户看的那句话（内核原文照登，约束 3）。**这是唯一的映射实现** ——
    /// 别处再抄一份，改一处就会出现"同一个错误在两个界面上措辞不同"。
    ///
    /// ⚠️ `nonisolated`：它是个**纯函数**（不碰任何实例状态），而 `Presentation/` 那层是
    ///    无隔离的纯函数（`DirLoadFailure.of` 就用它）。理由与 `defaultHandshakeTimeout`
    ///    相同：把隔离去掉不会让任何可变状态暴露出去，而留着它只会逼出一个副本。
    nonisolated static func message(of error: Error) -> String {
        guard let e = error as? CoreError else { return "\(error)" }
        switch e {
        case .transport(let m), .malformedResponse(let m): return m
        case .rpc(_, let m): return m
        }
    }
}

// ---------------------------------------------------------------------------
// 握手的超时（任务 4b 的零件）
// ---------------------------------------------------------------------------

/// 握手没在 `handshakeTimeout` 内完成。
///
/// ⚠️ **故意不是 `CoreError`**：内核一个字都没回，没有 `code` 也没有 `message`。
///    把它做成 `CoreError` 的后果是它会流进 `absorb`（附录 A 的处置表）——
///    那张表会把 `.transport` 判成"内核死了"并**自动重启**，而超时的正确处置是
///    "有界停下 + 明示 + 等用户点重试"（`start()` 的注释里写了为什么不能自动重启）。
struct HandshakeTimeout: Error, Equatable {}

/// 「谁先到谁说话」的一次性闸门：`take()` 只有一个调用者能拿到 `true`。
///
/// 它守着的是 `CheckedContinuation` 的**唯一一次 resume** —— 赛跑的两条路可能几乎同时到达，
/// 二次 resume 是运行时的致命错误（crash），不是可以忽略的小毛病。
private final class RaceClaim: @unchecked Sendable {
    private let lock = NSLock()
    private var taken = false

    func take() -> Bool {
        lock.lock(); defer { lock.unlock() }
        if taken { return false }
        taken = true
        return true
    }
}

// ---------------------------------------------------------------------------
// 空快照
// ---------------------------------------------------------------------------

extension TransferListResult {
    /// 空快照：`engine_not_started` 是正常态，传输列表显示空态（不是错误）。
    static var empty: TransferListResult {
        TransferListResult(items: [],
                           global: GlobalStat(downloadSpeed: 0, numActive: 0,
                                              numWaiting: 0, numStopped: 0))
    }
}
