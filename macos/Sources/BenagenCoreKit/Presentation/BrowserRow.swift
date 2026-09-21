import Foundation

// ---------------------------------------------------------------------------
// 文件浏览器那一列表的**呈现模型**：行、状态样式、选择、底部汇总。
//
// 全部是纯函数（只 `import Foundation`，**不 import SwiftUI**）——
// 颜色用[`RowColor`]这个**枚举**表示，不是 SwiftUI 的 `Color`：
// 在 Kit 里引入 SwiftUI 会破坏"Kit 不含 UI"这条线（约束 8 / 本任务简报）。
// 枚举 → `Color` 的那一步（`EngineStatusBadge.tint` 的同类）留在视图里，
// 它是纯渲染决定、断言不出有意义的东西。
//
// ⚠️ 为什么这些映射在这里而不在视图里（全局约束 8）：
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件"。下面每一条都能写出断言，
//   其中有几条背着硬约束：目录路径要壳自己拼（`list_dir` 的目录项没有 `path` 键）、
//   四态两两不同（少一个状态就是静默失效，约束 4）、排序（不平铺就是另一种界面）。
// ---------------------------------------------------------------------------

/// 平铺列表里的一行：**已经算好的界面值**，视图侧只剩绑定。
///
/// 「名称 · 大小 · 时间 · 状态」四列的值都在这里成形，`FileBrowser` 里不拼字符串、不判断类型。
public struct BrowserRow: Equatable, Sendable, Identifiable {
    /// 判别键来自内核的 `"type"`（`"dir"` / `"file"`），**不是** `is_dir` 之类的布尔。
    public enum Kind: String, Equatable, Sendable {
        case dir, file
    }

    public let kind: Kind
    /// 显示名（路径最后一段的原文）。
    public let name: String
    /// 完整路径（约束 3：原文，逐字）。
    public let path: String
    /// 目录的子项数；文件是 `nil`。
    public let childrenCount: Int?
    /// 文件大小；**目录是 `nil`**（"没有大小这回事" ≠ "大小是 0"：目录的大小要等内核
    /// 展开才知道，而壳不展开目录，约束 1）。
    public let size: Int64?
    /// 「大小」那一列显示的文字：文件是 [`ByteFormat`]，目录是「N 项」。
    public let detailText: String
    /// 「时间」那一列显示的文字（**源文件**的修改时间，内核原文经 [`SourceTimeText`]）。
    public let sourceTimeText: String
    /// 「状态」那一列。
    public let state: RowStateStyle
    /// 「名称」那一列的 SF Symbol 名（同 `EngineStatusPresentation.systemImage` 的形态）。
    public let iconName: String

    public var id: String { path }

    /// 一个目录项 → 一行。
    ///
    /// ⚠️ `parent` 只有**目录项**用得上：`list_dir` 的目录项**没有 `path` 键**，
    ///    路径只能由壳拼（[`Breadcrumb.join`]）；文件项有 `path`，逐字用它 ——
    ///    不要"顺手"重新拼一遍文件路径，那是在拿壳的拼接覆盖内核给的原文（约束 3）。
    public static func of(_ entry: DirEntry, parent: String) -> BrowserRow {
        switch entry {
        case .dir(let name, let childrenCount):
            return BrowserRow(kind: .dir,
                              name: name,
                              path: Breadcrumb.join(parent: parent, name: name),
                              childrenCount: childrenCount,
                              size: nil,
                              detailText: "\(childrenCount) 项",
                              // 目录**没有**"源文件时间"这回事（与它的大小同理：那得等内核
                              // 展开才知道，而壳不展开目录，约束 1）—— 恒为占位字形。
                              sourceTimeText: SourceTimeText.noValue,
                              state: RowStateStyle.of(nil),
                              iconName: "folder")
        case .file(let f):
            return BrowserRow(kind: .file,
                              name: f.name,
                              path: f.path,
                              childrenCount: nil,
                              size: f.size,
                              detailText: ByteFormat.text(f.size),
                              sourceTimeText: SourceTimeText.of(f.sourceMtime),
                              state: RowStateStyle.of(f.state),
                              iconName: "doc")
        }
    }

    /// 一层目录 → 排好序的一列表。
    ///
    /// **目录在前、同组内按名字升序。** 用 Swift 的 `<`（Unicode 规范序、与语言环境无关），
    /// **不用 `localizedStandardCompare`**：后者的结果随机器语言环境变，
    /// 于是"排序对不对"这件事在客户机上和在测试里是两个答案
    /// （同 `ByteFormat` 不用 `ByteCountFormatter` 的理由）。
    public static func rows(_ entries: [DirEntry], parent: String) -> [BrowserRow] {
        entries.map { of($0, parent: parent) }
            .sorted { a, b in
                if a.kind != b.kind { return a.kind == .dir }
                return a.name < b.name
            }
    }
}

/// 「时间」那一列显示什么。
///
/// 输入是内核给的**原文**（`FileNode.sourceMtime`，ISO 8601 带 `+08:00`），
/// 内核当不透明字符串搬运、壳也不解析它（约束 3）。
///
/// ⚠️ 三件事：① 缺值与空串都出 `—`（那是"这一格没有值"，不是"时间是空的"）；
///    ② 能认的走 `TimestampPresentation`（它已经保证**解析不了就原样返回**）；
///    ③ 所以壳**永远不会**显示 `Invalid Date` 这种自己编的文案（约束 C-7）——
///       "原样返回"这条契约由 `TimestampPresentation` 一处定义，这里只是沿用它，
///       **不要**在本文件另造一套格式化（那是把同一件事写出两个会分叉的答案）。
///
/// ⚠️ 缺值与空串在这里**故意合流**：调用方（视图）要的只是"这一格显示什么"，
///    而"键不在"与"值是空串"在界面上本来就该长得一样。两者的**区分**留在
///    `FileNode.sourceMtime` 那一层（`String?`：`nil` vs `""`）—— 在那里合并，
///    就再也分不出"解码把键丢了"与"内核真的没给值"。
enum SourceTimeText {
    /// 这一格没有值时的占位字形。
    ///
    /// ⚠️ 与 `Format.swift` 的 `SpeedFormat`/`PercentFormat`、`DeliverySummary.noValue`
    ///    同一个字形（`—`）：空串在界面上就是"什么都没说"，而"这一格没有值"
    ///    本身也是一件要说出来的事（约束 4）。
    ///    目录行也用它（`BrowserRow.of` 的目录支）—— 目录没有"源文件时间"这回事。
    static let noValue = "—"

    static func of(_ raw: String?) -> String {
        guard let raw, !raw.isEmpty else { return noValue }
        return TimestampPresentation.text(raw)
    }
}

/// 「状态」那一列的呈现：标签 + 颜色。
///
/// ⚠️ 四态**两两不同**是硬要求（约束 4：不得静默失效 —— 把两种状态画成一样，
///    等于其中一个永远不会出现在界面上）。钉住它的是
///    `eachOfTheFourStatesMapsToItsOwnLabelAndColor`。
///
/// ⚠️ 这里**没有**"未知状态"这一支，而且这是有意的：状态来自 `FileState`（内核的
///    `state_name()` 四态），一个壳不认识的字符串会让 `DirEntry`/`FileNode` 的**解码**
///    直接失败（`CoreError.malformedResponse`，界面会显示原文）—— 不会静默造一个假状态，
///    所以也轮不到这里兜底。剩下唯一一种"没有状态"是**目录行**：它在四态模型里
///    本来就不参与合成（`flat` 只列文件），落到[`none`]。
public enum RowStateStyle: String, CaseIterable, Equatable, Sendable {
    case pending, downloading, complete, failed, none

    /// 界面上的标签。**任何一支都不许是空白**（约束 4）。
    public var label: String {
        switch self {
        case .pending: return "待下载"
        case .downloading: return "下载中"
        case .complete: return "已完成"
        case .failed: return "失败"
        // 占位字形与 `Format.swift` 的 `SpeedFormat`/`PercentFormat` 同一个（`—`）：
        // 空串在界面上就是"什么都没说"，而"这一格没有值"本身也是一件要说出来的事。
        case .none: return "—"
        }
    }

    /// 语义色（规格 §7.2）。**不含 SwiftUI 的 `Color`**，由视图映射。
    public var color: RowColor {
        switch self {
        case .pending: return .secondary
        case .downloading: return .blue
        case .complete: return .green
        case .failed: return .red
        case .none: return .secondary
        }
    }

    /// 内核的四态 → 样式；`nil`（目录行）→ [`none`]。
    public static func of(_ state: FileState?) -> RowStateStyle {
        switch state {
        case .pending: return .pending
        case .downloading: return .downloading
        case .complete: return .complete
        case .failed: return .failed
        case nil: return .none
        }
    }
}

/// 行状态的语义色。**枚举，不是 `Color`**（`Presentation/` 不引入 SwiftUI）。
///
/// ⚠️ 成员是规格 §7.2「语义色（成功/警告/错误）」的那几个 + 中性灰：
///    `.green` = 成功、`.orange` = 警告、`.red` = 错误、`.secondary` = 中性/未开始、
///    `.blue` = 进行中。**加成员时每一处 `tint(_:)` 都会因为 `switch` 不再穷尽而编译不过**
///    —— 那是刻意的，别用 `default` 盖掉。
///
/// ⚠️ **这里刻意不写"有几处"、也不列是哪几个文件**：这个数字一直在长（任务 8 时是三处，
///    任务 9 又加了一处），而这段注释的用途正是"提醒后来人别用 `default` 把编译期保护盖掉" ——
///    一个数错自己保护范围的提醒会先把这个用途废掉。要数就现数：
///    `grep -rn "func tint(" Sources/`。
public enum RowColor: Equatable, Sendable {
    case secondary, blue, green, red, orange
}

// ---------------------------------------------------------------------------
// 选择
// ---------------------------------------------------------------------------

/// `onLoad` 的结果：要不要复位 + 把"上一次显示过的码"记成什么。
public struct ManifestLoad: Equatable, Sendable {
    /// 这是一批**新**清单吗（⇒ 浏览位置与勾选面立刻复位，不等这一批的树到）。
    public let isANewManifest: Bool
    /// 新的"上一次**显示**过的码"。
    public let lastDisplayedCode: String?
}

/// 一次播种的结果：**该播成什么** + **这次播种属不属于换批**。
///
/// ⚠️ 两件事必须能分开（复审重要 2）：`FileBrowser.enter()` 拿到 `selection` 就播下去，
///    但"**顺带把浏览位置打回根目录**"只允许在 `isANewBatch` 为真时做 ——
///    同码重载（含**内核崩溃后的自动恢复**）是一次**非用户动作**引发的加载，
///    把用户停在的子目录静默换掉、界面上一句话都没有，是约束 4 要防的那类形态。
///    判据是纯值（有单测），视图只做分派（约束 8）。
public struct ManifestSeeding: Equatable, Sendable {
    /// 该播成的默认勾选面（`get_tree` 的 `default_selected`）。
    public let selection: Set<String>
    /// 这是一批**新**清单吗（交付码与"上一次播种过的那一批"不同）。
    ///
    /// ⚠️ 判据是**"上一次播种的是不是这一批"**，不是"这一代有没有播过种"：
    ///    后者对同码重载同样为真，而那恰恰是要与换批区分开的两种情形。
    public let isANewBatch: Bool

    public init(selection: Set<String>, isANewBatch: Bool) {
        self.selection = selection
        self.isANewBatch = isANewBatch
    }
}

/// 文件浏览器与"是哪一批清单"有关的那部分状态 —— **纯值，迁移全在这里**。
///
/// ⚠️ 承重事项 A 有**两半**，这个类型把它们收在同一个值里，好让两半都有能被单测钉住的落点：
///    - **复位那一半**（`display`）：`code` 一变就把"当前在哪一层 / 勾了什么"作废，**不等树到**；
///    - **播种那一半**（`seed`）：换批之后**必须**还能让这一批的 `default_selected` 播下来。
///      这一半坏掉的样子是**静默**的：换批时若把"已经为哪一批播过种"记着不放，
///      那么用户在 B 的树到之前回到 A 时，`seededCode` 还是 "A" ⇒ `onNewManifest` 判成"同一批"
///      ⇒ **A 的默认选中面永远播不下来** ⇒ 用户开箱看到「已选 0 项」，而**界面上一点异常都没有**。
///      所以换批时 `seededCode` 跟着一起清 —— `display` 的返回值就是那个时刻。
///
/// ⚠️ 三个字段的语义**必须分开**（混用就是 `A→B→A` 那一族缺陷的来源）：
///    - `lastDisplayedCode` = "上一次**显示**过的码"：**显示**那一刻推进，是**复位**的对照值；
///    - `seededCode` = "**已经为哪一批播过种**"（nil = 当前显示的这一批还没播过）：
///      **播种成功**那一刻才推进，且**换批时清空**，是**播种**的批次对照值；
///    - `seededGeneration` = "**已经为哪一次加载播过种**"（`AppModel.loadGeneration`）：
///      同样是播种成功那一刻才推进。它是**播种的次数**上的对照值，与批次无关 ——
///      两者都要，缺一不可（见下面 `seed` 里那两段注释）。
///
/// ⚠️ 因此**不能**拿 `lastDisplayedCode` 去当 `seed` 的对照值（这是审查建议里被否决的那半）：
///    显示那一刻它就已经等于新码了，`seed` 会判成"这一批已经播过" ⇒ **第一次加载的
///    默认选中面永远播不下来**（`theFirstLoadSeedsItsDefaultSelection` 钉着这一点）。
///    反过来说，"播种"这件事必须有一个**独立于显示**的记账，才能既"每批只播一次"、
///    又"换批后还能再播"。
public struct ManifestTracking: Equatable, Sendable {
    private var lastDisplayedCode: String?
    private var seededCode: String?
    /// 已经为**哪一次加载**播过种（`AppModel.loadGeneration`；nil = 还没播过）。
    ///
    /// ⚠️ **阶段 D 任务 B 加的**：原先只有 `seededCode`（按**批次**记账），于是
    ///    **同码重载**被判成"这一批已经播过种" ⇒ 返回 nil ⇒ 内核明明刚重新规划过整批
    ///    （`default_selected` 是新信息）、界面却一点都不动 —— 用户读成"我重载了，
    ///    什么也没发生"（诊断报告 §6.1 的猜想 2，已用测试证实）。
    ///    判据从"哪一批"换成"哪一次加载"之后，同码重载天然会重播；而
    ///    "同一批里换目录 / 切分区回来 / 树到位后再进来"这些**没有发生加载**的重入
    ///    仍然落在同一代里 ⇒ **照样不重播**（用户跨目录攒的选择还是不会被抹掉）。
    private var seededGeneration: Int?

    public init() {}

    /// 界面显示了一批清单（`loadState` 变成 `.loaded`）。
    ///
    /// **返回值 = 换批**（`true`）⇒ 调用方复位浏览位置与勾选面。树到没到**不影响**这个判断。
    /// ⚠️ 这个返回值不要丢掉：它就是"复位"这件事的唯一触发点。
    public mutating func display(code: String?) -> Bool {
        let step = BrowserSelection.onLoad(code: code, lastDisplayedCode: lastDisplayedCode)
        lastDisplayedCode = step.lastDisplayedCode
        // 换批 ⇒ 这一批还没播过种（哪怕它的码在更早的时候播过 —— A→B→A 就是这个情形）。
        // ⚠️ 两个记账**一起**作废：`seededCode` 管"哪一批"、`seededGeneration` 管"哪一次加载"，
        //    只清一个就留下"半个已播种"的中间态。顺带保住一条自愈：`display` 与 `seed`
        //    在 SwiftUI 里的**次序没有保证**（README 第 11c 条那个已知的潜在竞态），
        //    万一 `display` 排在 `seed` 后面把这一批的记账清掉，也必须留下一条
        //    "下一次 `taskKey` 变化还能补播"的路 —— 清掉代数那一段就是那条路。
        if step.isANewManifest { seededCode = nil; seededGeneration = nil }
        return step.isANewManifest
    }

    /// 这一批的树到位时该播什么 + **这次播种算不算换批**
    /// （`nil` = 不播：这一次加载已经播过了，或树还不是这一批的）。
    ///
    /// **只有真的播成功才推进那两个记账** —— 树没到就一直推迟，而"树到"会让 `.task` 的 key
    /// 变化、再进来播一次（那正是 `taskKey` 里 `treeCode` 那一段的存在理由）。
    ///
    /// ⚠️ `generation` 是 `AppModel.loadGeneration`（**一次成功的 `load_delivery` = 新的一代**）。
    ///    两个对照值各管一件事，**都不能删**：
    ///      - `seededGeneration`：**同一次加载内不重播**。少掉它就退化成"每次 `taskKey`
    ///        变化都重播"——用户在同一个批次里进个目录、切一下分区回来，攒的选择就被抹掉
    ///        （那正是承重事项 A 的另一半）；
    ///      - `seededCode`（经 `previousCode` 传给 `onNewManifest`）：**同码重载**要能重播。
    ///        同一次加载里它总是等于当前码，所以下面先按"新一代"把它作废，`onNewManifest`
    ///        那条按批次判定的守卫才会放行。
    public mutating func seed(code: String,
                              treeCode: String?,
                              tree: TreeResult?,
                              generation: Int) -> ManifestSeeding? {
        guard seededGeneration != generation else { return nil }
        // ⚠️ **先判"算不算换批"，再动作**：判据是"上一次播种的是不是这一批"
        //    （`seededCode != code`）。它必须在下面那行把记账清掉**之前**读 ——
        //    清完就再也分不出"同码重载"与"换了一批"了（两者在这里都是"新一代"）。
        let isANewBatch = seededCode != code
        // 新的一次加载 ⇒ 上一次那条"已经为这一批播过种"的记账作废。**同码重载正是靠这一行
        // 才播得下来**：少了它，`onNewManifest` 会按 `code` 判成"同一批" ⇒ 静默不播。
        if seededCode == code { seededCode = nil }
        guard let picked = BrowserSelection.onNewManifest(code: code,
                                                          previousCode: seededCode,
                                                          treeCode: treeCode,
                                                          tree: tree) else { return nil }
        seededCode = code
        seededGeneration = generation
        return ManifestSeeding(selection: picked, isANewBatch: isANewBatch)
    }
}

/// 勾选面的规则（默认选中面 / 全选当前层 / 换批复位的判据 / `.task` 的触发键）。
public enum BrowserSelection {

    /// `.task(id:)` 的触发键 —— 换批、换层、或**这一批的树刚刚到位**就重新进来一次。
    ///
    /// ⚠️ 它是个**纯函数**（所以能被单测钉住）：视图不单测（约束 8），
    ///    而"什么变化要重新进一次"恰恰是本项目栽过的那类**静默失效**。四段缺一不可：
    ///      - `code`：换批；
    ///      - `path`：换层；
    ///      - `treeCode`：**这一批的树到位**。`AppModel` 先落 `loadState = .loaded`、
    ///        **之后**才拉 `get_tree`（同 `onNewManifest` 那段注释里的竞速），所以视图第一次
    ///        进来时树多半还没到（`treeCode == nil`），`onNewManifest` 会**推迟**播种。
    ///        正是这一段让树到位之后能**再进一次**、把默认选中面播下去；删了它，
    ///        "推迟播种"就变成"永不播种"（用户开箱看到「已选 0 项」，而界面上一点异常都没有）。
    ///      - `generation`：**这一批又被成功加载了一次**（`AppModel.loadGeneration`）。
    ///        同码重载时前三个分量**一个字都没变**，而内核已经重新规划过整批 ⇒
    ///        默认勾选面必须重播。这一段就是那个唯一的入口：少了它，`enter()` 根本不会
    ///        再跑一次，"同码重载重播默认面"就无从发生（诊断报告 §6.1 的猜想 2）。
    ///
    /// ⚠️ 前三段用**长度前缀**而不是裸拼 `|`：这三个值都是清单/内核给的原文，里面可以有 `|`
    ///    （约束 3：壳不规范化、不转义），裸拼会让 `("A|B", "", nil)` 与 `("A", "B|", nil)`
    ///    撞成同一个键 —— 撞了就是"该重进的时候没重进"，也就是这条防线要防的那件事。
    ///    `generation` 是**纯整数、且排在最后**，前面那段又是长度前缀、自定界的，
    ///    所以直接接在 `|` 后面不会与树码里的 `|` 混淆。
    public static func taskKey(code: String,
                               path: String,
                               treeCode: String?,
                               generation: Int) -> String {
        "\(code.count):\(code)|\(path.count):\(path)|"
            + (treeCode.map { "\($0.count):\($0)" } ?? "nil")
            + "|\(generation)"
    }

    /// `code` 与"拿来对照的那个码"不同 ⇒ **这是一批新清单**。
    ///
    /// ⚠️ **它只是"码变了吗"这一条取值规则**，不是"复位"这件事本身 ——
    ///    两个调用点传进来的对照值是**两种不同语义**，共用一个函数并**不**保证两边一致：
    ///      - `onNewManifest`（播种）传的是「**已经为哪一批播过种**」（`ManifestTracking.seededCode`）；
    ///      - `onLoad`（复位）传的是**上一次显示过的码**（`ManifestTracking.lastDisplayedCode`）。
    ///    两者在 `A→B→A` 这条路上会分叉，分叉的后果与堵法见 `onLoad` 与 `ManifestTracking`。
    ///    （别把这条函数当"承重事项 A 的守卫"读：它守不住 A 的任何东西。）
    public static func isANewManifest(code: String, previousCode: String?) -> Bool {
        code != previousCode
    }

    /// 加载到一批清单时的**显示状态迁移**：`(上一次**显示**过的码, 这一次加载到的码)` →
    /// `(要不要复位, 新的"上一次显示过的码")`。
    ///
    /// 复位 = "浏览器当前在哪一层"与勾选面**立刻**作废，**不等这一批的树到**。
    ///
    /// ⚠️ 为什么"不等树"是硬要求：`onNewManifest` 里那条"树还没到就推迟"推迟的是**播种**，
    ///    **不是复位**。把复位一起推迟的后果，是"码已变、这一批的树还没到"的窗口里，
    ///    浏览器带着**上一批**的 `currentPath` 与勾选面去面对新批次：
    ///      - 它会对**上一批的路径**发 `list_dir`（那些路径在新批次里可能指向别的文件）；
    ///      - 底部「下载选中」/ 工具栏那颗按钮继续拿着**上一批的路径** ——
    ///        用户以为在下 B 批的文件，实际发出去的 `enqueue` 是 A 批的路径。
    ///
    /// ⚠️⚠️ **对照值必须是"上一次显示过的码"，不能是"已经播过种的码"**（`seededCode`）。
    ///    后者只在**播种成功**时写，于是 `A→B→A` 这条路上它是坏的：
    ///      ① 显示 A（`seededCode = "A"`）→ ② 切到 B（复位一次，但 `seededCode` **仍是 "A"** ——
    ///      复位有意不消费它）→ ③ 用户在 B 的树到之前勾了几项（勾选不依赖树）→
    ///      ④ 又回到 A：拿 `seededCode` 当对照 ⇒ `"A" != "A"` 为假 ⇒ **不复位** ⇒
    ///      B 的勾选面被拿去给 A 发 `enqueue`。**这正是承重事项 A 要修的那个缺陷，只是换了顺序。**
    ///      （同一现场还有"静默"那一半：A 的 `default_selected` 再也播不下来 ——
    ///       `seededCode` 还是 "A"，`onNewManifest` 判成"同一批"，于是永远不播种。
    ///       那一半由 `ManifestTracking.display` 在换批时把 `seededCode` 清掉来堵。）
    ///
    /// ⚠️ `code == nil`（加载中 / 失败 / 内核回"没有生效的批次"）时**既不复位、也不推进**
    ///    那个记住的码：一次 nil 抖动不该让"紧接着的同一批加载"被误判成换批
    ///    （那会把用户在同一批里攒的选择抹掉），也不该把已经记住的码冲成 nil
    ///    （那会让下一次同批加载又变成"换批"）。
    public static func onLoad(code: String?, lastDisplayedCode: String?) -> ManifestLoad {
        guard let code else {
            return ManifestLoad(isANewManifest: false, lastDisplayedCode: lastDisplayedCode)
        }
        return ManifestLoad(isANewManifest: isANewManifest(code: code, previousCode: lastDisplayedCode),
                            lastDisplayedCode: code)
    }

    /// 新一批清单到达时，选择该变成什么。
    ///
    /// - 返回 `nil` = **保持不动**（既不改选择，也**不消费** `previousCode`）。
    /// - 返回一个集合 = 换批了，重设成这个。
    ///
    /// ⚠️ `previousCode` 的语义是「**已经为哪一批播过种**」（`ManifestTracking.seededCode`），
    ///    **不是**"上一次显示过的码" —— 后者在显示那一刻就等于新码了，会把第一次播种也判成
    ///    "已经播过"。两者为什么必须分开：见 `ManifestTracking` 的注释。
    ///
    /// ⚠️ 默认选中面来自 `get_tree` 的 `default_selected`（内核给的"所有非 complete 的文件"），
    ///    **不是**当前层的全部条目 —— 这是"点开就能直接点下载"的关键（简报点名）。
    ///
    /// ⚠️ **树必须属于这一批**（`treeCode == code`），否则**推迟**（返回 `nil`，不消费）。
    ///    理由是一条真实的竞速：`AppModel` 先落 `loadState = .loaded(info)`，之后才去拉
    ///    `get_tree`；而视图是在 `.loaded` 那一刻出现的 —— 于是这里被问到时，
    ///    `tree` 可能**还没到**（`nil`）或者**还是上一批的**。
    ///    - 前者若当作"空集"播下去，这一次记账就被消费掉了，**此后没有任何 re-seed 路径**
    ///      （视图的 taskKey 里没有树），用户开箱看到「已选 0 项」而"点开就能直接点下载"**静默失效**；
    ///    - 后者更糟：把**上一批的路径**播进新批次（那些路径在这一批里可能指向别的文件）。
    ///    越大的批次（树越大、`get_tree` 越慢）越容易输掉这场竞速 —— 所以判据放在这里（纯函数）
    ///    而不是视图里：视图不做单测（约束 8）。
    ///    钉住它的是 `seedingWaitsForThisBatchesTree`。
    public static func onNewManifest(code: String,
                                     previousCode: String?,
                                     treeCode: String?,
                                     tree: TreeResult?) -> Set<String>? {
        // 还是同一批（刷新、目录来回切、切分区回来）：不动，别抹掉用户攒起来的选择。
        // 共用的只是"码变了吗"这条**规则**（`isANewManifest`）；这里的 `previousCode` 是
        // **最后一次播种用过的码**，与 `onLoad` 传的"上一次显示过的码"是两种语义 ——
        // 别把这条 guard 当成"复位也已经做过了"的保证。
        guard isANewManifest(code: code, previousCode: previousCode) else { return nil }
        // 这一批的树还没到位（没到 / 还是上一批的）：**推迟**，不消费 `previousCode`。
        guard treeCode == code, let tree else { return nil }
        return Set(tree.defaultSelected)
    }

    /// `⌘A`：**当前这一层**的全部条目（不是整棵树、也不是 `default_selected`）。
    ///
    /// 目录也在里面：内核在 `enqueue` 里按前缀展开目录 —— 展开是内核的事（约束 1）。
    public static func all(in rows: [BrowserRow]) -> Set<String> {
        Set(rows.map(\.path))
    }

    /// 整批的**全部文件**（`get_tree` 的 `flat` 只列文件 —— 目录不在里面）。
    ///
    /// 两个用途，**别混**：
    ///   ① 判断"勾选面是不是覆盖了整批"（`DownloadTargets.paths(for:allPaths:)`）——
    ///      覆盖了就必须发 `[]`（约束 C-3：超限请求不会被报错，只会把客户端静默堵死）；
    ///   ② 底栏那颗「全选」的输入 —— 它的语义就是"整批全部文件"，
    ///      按下去正好落进 ① 那一支（全选 ⇒ `paths: []` ⇒ 下全部待下载）。
    ///
    /// ⚠️ 「全选**本层**」是**另一件事**，用 `all(in:)`：它选的是**这一层看得见的行（含目录）**。
    ///    （⌘A 与右键菜单那条「全选本层」都走它。）
    ///
    /// ⚠️ **有意偏离简报里的一段注释**（约束 11）：简报给的原文说"**不要**用它去当『全选』
    ///    的输入"，但同一份简报的步骤 13 与 README 第 20 条第 5 款都说底栏那颗「全选」
    ///    勾的是"整批全部文件"。按**行为**取步骤 13，理由：只有"勾选面 == 整批文件"
    ///    才会走到 `paths: []` 那一支 —— 而"支持全选，即下载全部文件"正是本任务要交付的东西；
    ///    改用 `all(in:)` 的话，只要批次里还有子目录，两边就永远不相等，
    ///    那条 `[]` 分支形同虚设（请求体随批次大小线性增长，正是 C-3 要防的）。
    public static func allFiles(in flat: [FlatEntry]) -> Set<String> {
        Set(flat.map(\.path))
    }

    /// **双击进入一个目录之后**，勾选面该变成什么：把**被进入的那一个**摘掉，其余原样。
    ///
    /// ⚠️ 这不是优化，是一次**真实交付事故**的修复（2026-09-20 客户现场）：
    ///    双击进入 `A/B` 时，`List` 的选中绑定会先把 `"A/B"` 写进勾选面 ——
    ///    双击的**第一下就是一次单击选中**（`FileBrowser` 里 `.contextMenu` 那段注释
    ///    写着同一件事："在 macOS 上，双击前单击已经把它选出来了"）。
    ///    而 `navigate(to:)` 换层时**不动勾选面**（进目录不清空，那是有意的：
    ///    README 11c "进目录再返回，用户自己攒的勾选不该被清掉"）——
    ///    于是那个目录路径**永远留在了勾选面里**，而且**看不见**
    ///    （它属于用户已经离开的那一层）。
    ///
    ///    后果不是"勾选面多一项"这么轻：
    ///      - 底栏显示「已选 1 项」而用户**什么都没勾**（客户的原话：
    ///        "只要点击了进了目录，即使没有选中任何文件，都有一个『已选 1 项』"）；
    ///      - 再勾一个文件就成了「已选 2 项」，按「下载选中」发出去的是
    ///        `[目录路径, 文件路径]`，而内核把每个 path 当**前缀**展开
    ///        （`core/src/main.rs:1254` 的 `resolve_targets`）⇒
    ///        **整个目录的文件都被下了**（客户的原始报告："选择单个文件…结果下载了整个目录的文件"）。
    ///
    /// ⚠️ 判据是"**进目录是导航，不是『我要下这个目录』**"（README 20 第 9 条：
    ///    双击目录 = 进入；第 7 条才是"勾一个目录行 = 下这个目录"，那条走的是复选框/单击，
    ///    不经这里，所以不受影响）。
    ///
    /// ⚠️ **代价（如实记下）**：用户**先勾了一个目录行、再双击进它**时，那个勾会被摘掉 ——
    ///    "双击进入"这个动作本身没有携带"我是不是故意勾的"这个信息，壳分辨不出来。
    ///    取舍：代价是**可见且可恢复**的（再勾一次），而不修的代价是
    ///    "按下载选中会静默多下整个目录"（不可见、不可撤销、要走网络跑几小时）。
    ///
    /// ⚠️ **幂等**：路径不在勾选面里时原样返回 —— 修好之后这恰恰是最常见的形状
    ///    （双击第一下写的那个已经被摘掉，用户再进同一层时它早就不在了）。
    public static func afterEntering(_ path: String, selection: Set<String>) -> Set<String> {
        selection.subtracting([path])
    }
}

/// 底部状态栏那一行：「已选 N 项 · 合计大小」。
public struct SelectionSummary: Equatable, Sendable {
    /// 「已选 3 项」。
    public let countText: String
    /// 「合计 6.8 KB」。
    public let sizeText: String

    /// 勾选面 → 那一行。
    ///
    /// - `countText` 数的是**勾选的条目数**（目录也算一项 —— 它确实被选中了）；
    /// - `sizeText` 只累加**在 `sizes` 里有大小**的那些。目录在 `flat` 里没有大小
    ///   （`get_tree` 的 `flat` 只列文件），所以它对合计的贡献是 0。
    ///   壳**不**自己展开目录去猜一份大小（约束 1：展开是内核的事）。
    public static func of(selected: Set<String>, sizes: [String: Int64]) -> SelectionSummary {
        // `+=` 会在 Int64 上溢出崩溃；用 `addingReportingOverflow` 夹住——一组畸形
        // （或恶意）的大小不该让整个界面崩掉（同 `PercentFormat` 的乘法守卫）。
        var total: Int64 = 0
        for path in selected {
            guard let one = sizes[path] else { continue }
            let (sum, over) = total.addingReportingOverflow(max(0, one))
            total = over ? Int64.max : sum
        }
        return SelectionSummary(countText: "已选 \(selected.count) 项",
                                sizeText: "合计 " + ByteFormat.text(total))
    }

    /// `get_tree` 的 `flat[]` → 路径 → 大小。**跨目录**的合计要靠它（当前层只有一部分）。
    public static func sizeIndex(_ flat: [FlatEntry]) -> [String: Int64] {
        var out: [String: Int64] = [:]
        for f in flat { out[f.path] = f.size }
        return out
    }
}
