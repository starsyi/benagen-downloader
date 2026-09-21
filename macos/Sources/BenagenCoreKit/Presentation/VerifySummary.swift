import Foundation

// ---------------------------------------------------------------------------
// 校验结果的**呈现模型**：全部是纯函数，无状态、无 SwiftUI 依赖（只 `import Foundation`）。
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件" —— 下面每一条都能写出断言，
//   而且有几条背着硬约束：
//     · 约束 4 的**六类互斥穷尽**（计数为 0 的类也要渲染 —— 靠"数组为空画不出东西"
//       来表达 0，在界面上就是"这一类不见了"）；
//     · 约束 3 的**路径原文照登**（不取最后一段、不规范化、不转义）；
//     · 以及"**壳不得比内核更严**"：`all_good` 不含 `unverifiable`
//       （`core/src/verify.rs` 的 `CheckResult::all_good`），壳不许把它算进去。
//
// ⚠️ 本文件**不要** `import SwiftUI`：颜色用 [`RowColor`] 这个**枚举**表达
//    （同 `BrowserRow` / `TransferRow`），枚举 → `Color` 的那一步留在视图里
//    （`VerifyView.tint(_:)`）—— 它是纯渲染决定、断言不出有意义的东西。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 一类校验结果
// ---------------------------------------------------------------------------

extension VerifyClass {
    /// 这一类在界面上算不算**失败**。
    ///
    /// ⚠️ `unverifiable` **不算** —— 这一条是内核的口径，壳一个字都不许改：
    ///    `CheckResult::all_good()` 的定义是
    ///    `bad.len() + missing.len() + size_mismatch.len() + unreadable.len() == 0`，
    ///    `unverifiable` 不在其中（"清单与 HEAD 都没给出 crc64"⇒ 没有可比的校验值，
    ///    不是"坏"，但也不是"已核对无误"）。壳把它算成失败就是**比内核更严**：
    ///    界面上会出现"总结说全部通过、另一处却挂着一个非零的失败计数"。
    ///    钉住它的是 `allGoodExcludesUnverifiable` / `theVerifyBadgeDoesNotCountUnverifiable`。
    ///
    /// ⚠️ `unreadable` **算**失败（内核的注释："读不了就无法证明它是完整的，
    ///    不能让客户以为交付齐全"）。
    public var isFailure: Bool {
        switch self {
        case .ok, .unverifiable: return false
        case .bad, .missing, .sizeMismatch, .unreadable: return true
        }
    }

    /// 界面上的标签。六条**两两不同**（约束 4：两类画成一个样子，等于其中一类永远看不见）。
    ///
    /// 措辞对着 `core/src/verify.rs` 里 `CheckResult` 各字段的注释取，不自己编语义：
    ///   - `bad` = "crc64 不符，或路径不可信（越界/控制字符），或该路径上不是普通文件"；
    ///   - `missing` = "本地不存在"；
    ///   - `size_mismatch` = "大小不符"；
    ///   - `unverifiable` = "清单与 HEAD 都没给出 crc64"；
    ///   - `unreadable` = "能 stat 到但读不了（权限/IO 错误）"。
    public var label: String {
        switch self {
        case .ok: return "校验通过"
        case .bad: return "内容不符"
        case .missing: return "文件缺失"
        case .sizeMismatch: return "大小不符"
        case .unverifiable: return "无法校验"
        case .unreadable: return "无法读取"
        }
    }

    /// 语义色（规格 §7.2）。**不含 SwiftUI 的 `Color`**，由视图映射。
    ///
    /// 三类分工：`.ok` 成功绿、四个失败类错误红、`unverifiable` **中性灰**。
    ///
    /// ⚠️ **中性色是 `unverifiable` 的专属标记**：别的类也用上 `.secondary`，
    ///    这一行就从"不是失败"退回成"和它们一样"，中性色想表达的那句话当场消失。
    ///    钉住它的是 `theUnverifiableClassIsTheOnlyNeutralColour`。
    ///    （四个失败类同色是有意的：它们在界面上是同一件事"没通过"，
    ///     彼此的区分靠标签、图标与**各自的路径列表**。）
    public var color: RowColor {
        switch self {
        case .ok: return .green
        case .unverifiable: return .secondary
        case .bad, .missing, .sizeMismatch, .unreadable: return .red
        }
    }

    /// 行首那颗 SF Symbol 名（同 `BrowserRow.iconName` / `TransferRow.stateIconName` 的形态）。
    /// 六条**两两不同**（理由同 `label`）。
    public var iconName: String {
        switch self {
        case .ok: return "checkmark.circle.fill"
        case .bad: return "xmark.circle.fill"
        case .missing: return "questionmark.folder"
        case .sizeMismatch: return "ruler"
        case .unverifiable: return "questionmark.circle"
        case .unreadable: return "lock.slash"
        }
    }

    /// 挂在标签下面的一句说明；`nil` = 这一类没有需要消歧的地方。
    ///
    /// ⚠️ 只有 `unverifiable` 有：它旁边永远站着一个"不是失败"的问号 ——
    ///    客户看到"无法校验 3 项"会自然地读成"有 3 项没通过"，而内核的判定正相反
    ///    （`all_good` 不含它）。这句话是**壳自己写的**（同 `AppModel.busyReason` /
    ///    `EngineGate.unavailableHelp` 的性质）：内核那句 `all_good` 是布尔值，
    ///    没有一个"原文"可以把这件事讲给客户听。
    ///    **每一类都渲染**这条规则的一个实例：这一行永远在（因为这一类永远在），
    ///    `VerifyClassRow` 里 `note` 非 nil 就等于视图会画它。
    public var note: String? {
        switch self {
        case .unverifiable: return "这些文件没有可比的校验值，不是失败"
        case .ok, .bad, .missing, .sizeMismatch, .unreadable: return nil
        }
    }
}

/// 一类校验结果在界面上的一行：**已经算好的界面值**，视图侧只剩绑定。
public struct VerifyClassRow: Equatable, Sendable, Identifiable {
    public let kind: VerifyClass
    /// 界面上的标签（六条两两不同）。
    public let label: String
    /// 语义色（枚举，不是 SwiftUI 的 `Color`）。
    public let color: RowColor
    /// 行首那颗图标。
    public let iconName: String
    /// 这一类的路径，**内核原文、逐字**（约束 3）。
    public let paths: [String]
    /// 标签下面那句说明（只有 `unverifiable` 有）。
    public let note: String?

    /// 计数。**恒等于 `paths.count`** —— 0 也是 0，不是"没有这一行"。
    public var count: Int { paths.count }
    /// 「N 项」。
    ///
    /// ⚠️ 它存在的全部理由是约束 4 那一句"计数为 0 的类**显示 0，不得隐藏**"：
    ///    只靠"路径列表为空所以画不出东西"来表达 0，在界面上就是"这一类不见了"。
    public var countText: String { "\(count) 项" }

    /// 这一类算不算失败（口径在 `VerifyClass.isFailure`）。
    public var isFailure: Bool { kind.isFailure }

    public var id: String { kind.rawValue }
}

// ---------------------------------------------------------------------------
// 顶部那一行总结
// ---------------------------------------------------------------------------

/// 顶部总结的四种形态。
///
/// ⚠️ 比简报里那两句（「全部通过」/「N 项未通过」）**多两句**，两处都是有意加的，
///    理由写在各自的 case 上（约束 11：有意偏离要注明理由）。
public enum VerifyVerdict: Equatable, Sendable {
    /// 内核把这一批的 `k.verify` 重置过、到此刻还没有任何文件被归类。
    ///
    /// ⚠️ **这一支是必须的**：`verify_status` 在"还没校验过"时回的正是
    ///    **六类全空 + `all_good == true`**（`core/src/main.rs` 的 `k.verify =
    ///    verify::CheckResult::default()` / `Kernel::new` 的同一个默认值，
    ///    而 `all_good()` 在四项失败全空时为真）。照登 `all_good` 的后果是：
    ///    一批**从没校验过**的文件顶上写着「全部通过」—— 一张空洞的通行证，
    ///    而客户会把它读成"我的数据都核对过了"。这正是约束 4 要防的那类静默失效。
    ///    （**不是**"比内核更严"：壳只是不在"零个文件"上替内核下一个结论，
    ///     `all_good` 本身照原样登在 `VerifySummary.allGood` 上。）
    case notVerified
    case allGood
    /// 六类明细里有 N 项未通过。
    case failed(Int)
    /// 内核说没全过（`all_good == false`），但六类里一个失败项都没有。
    ///
    /// ⚠️ 今天的**正确内核走不到这里**（`all_good()` 就是那四类计数算出来的）。
    ///    留着它是因为另外两条路都不对：说「全部通过」= 照抄一个与明细矛盾的判定；
    ///    说「0 项未通过」= 读起来像"没事"。矛盾必须**说出来**（约束 4），
    ///    钉住它的是 `aKernelJudgementOfNotAllGoodIsNeverDropped`。
    case kernelSaysNotAllGood

    public var text: String {
        switch self {
        case .notVerified: return "尚未校验"
        case .allGood: return "全部通过"
        case .failed(let n): return "\(n) 项未通过"
        case .kernelSaysNotAllGood: return "内核判定未全部通过（六类里没有失败项）"
        }
    }

    public var color: RowColor {
        switch self {
        case .notVerified: return .secondary
        case .allGood: return .green
        case .failed: return .red
        case .kernelSaysNotAllGood: return .orange
        }
    }

    public var iconName: String {
        switch self {
        case .notVerified: return "clock"
        case .allGood: return "checkmark.seal.fill"
        case .failed(_): return "exclamationmark.triangle.fill"
        case .kernelSaysNotAllGood: return "exclamationmark.triangle"
        }
    }
}

// ---------------------------------------------------------------------------
// 六类 + 总结
// ---------------------------------------------------------------------------

/// 一次校验结果的**全部**界面值：六行（恒六条，含计数为 0 的那些）+ 顶部总结。
///
/// 视图侧只做绑定：不拼字符串、不判断状态、不做算术。
public struct VerifySummary: Equatable, Sendable {
    /// **恒为 6 条**，顺序是 `VerifyClass.allCases`（ok / bad / missing /
    /// size_mismatch / unverifiable / unreadable），**空的那几类也在里面**（约束 4）。
    public let rows: [VerifyClassRow]
    /// 内核的 `all_good`，**原样**。
    ///
    /// ⚠️ 它是内核的判定，壳不二次推导（"内核说啥就是啥"，约束 1/3）。
    ///    顶部那一行总结**另有一份**由明细算出来的口径 —— 两者矛盾时以明细为准
    ///    （`theScreenNeverSaysAllClearWhileListingAFailure`），矛盾本身不会被藏起来。
    public let allGood: Bool
    /// 未通过项数 = `bad + missing + size_mismatch + unreadable`（口径见 `VerifyClass.isFailure`）。
    public let failedCount: Int
    /// 六类合计 —— 内核这一批**已经归过类**的文件数。
    /// 它是"这份结果覆盖了多少"的唯一可见证据（0 ⇒ 还没校验过）。
    public let classifiedCount: Int
    /// 顶部那一行总结。
    public let verdict: VerifyVerdict

    /// 顶部总结的正文（视图直接渲染它）。
    public var headline: String { verdict.text }
    /// 顶部总结的颜色（枚举，视图映射成 `Color`）。
    public var headlineColor: RowColor { verdict.color }
    /// 顶部总结的图标。
    public var headlineIcon: String { verdict.iconName }
    /// 「已校验 N 项」—— 让"这份结果覆盖了多少"看得见（同 `ByteFormat` 的"缺值也要说出来"）。
    public var classifiedText: String { "已校验 \(classifiedCount) 项" }

    /// `verify_status` 的结果 → 界面值；**`nil`（还没取过）→ `nil`**。
    ///
    /// ⚠️ `nil` 不能渲染成"六类全 0 + 全部通过"：那是**替内核宣布一句它没说过的话**
    ///    （还没问过它）。视图对 `nil` 走的是"还没有校验结果，点刷新"那一支，
    ///    钉住它的是 `noResultAtAllIsNotAnAllClear`。
    public static func of(_ status: VerifyStatus?) -> VerifySummary? {
        guard let status else { return nil }

        let rows = VerifyClass.allCases.map { kind in
            VerifyClassRow(kind: kind,
                           label: kind.label,
                           color: kind.color,
                           iconName: kind.iconName,
                           paths: status.paths(kind),
                           note: kind.note)
        }
        let failed = rows.reduce(0) { $0 + ($1.isFailure ? $1.count : 0) }
        let classified = rows.reduce(0) { $0 + $1.count }

        // ⚠️ 优先级是刻意的：**明细压过判定**。内核的 `all_good` 与六类计数由内核在
        //    同一个瞬间算出来（同一个 `CheckResult`），正常永远一致；真出现矛盾时，
        //    "屏幕上正列着 3 个『内容不符』、顶上却写『全部通过』"是这一屏最坏的失效，
        //    所以先看明细。`allGood` 本身仍然**原样**带在上面，一个字都没改。
        let verdict: VerifyVerdict
        if classified == 0 {
            verdict = .notVerified
        } else if failed > 0 {
            verdict = .failed(failed)
        } else if status.allGood {
            verdict = .allGood
        } else {
            verdict = .kernelSaysNotAllGood
        }

        return VerifySummary(rows: rows,
                             allGood: status.allGood,
                             failedCount: failed,
                             classifiedCount: classified,
                             verdict: verdict)
    }
}

// ---------------------------------------------------------------------------
// 侧边栏徽标（规格 §7.1「传输列表（带未完成计数徽标）」）
// ---------------------------------------------------------------------------

/// 侧边栏分区徽标的计数。
///
/// ⚠️ 两个数都是"**没落定的那些**"，且都**不含**"已经不用管的"：
///    传输列表不含已完成 / 已移除；校验结果不含 `unverifiable`（内核的口径，见
///    `VerifyClass.isFailure`）。徽标挂着一个红数字、而内容区写着"全部通过"，
///    比不挂徽标更糟。
///
/// ⚠️ SwiftUI 对 `.badge(0)` **什么都不渲染**（`Sidebar` 里那段注释记着这条），
///    所以 0 在这里的呈现就是"这个分区没有未落定的东西"。六类各自的「0 项」是**另一回事**
///    —— 它们在 `VerifyView` 里**必须看得见**（约束 4）。
public enum SidebarBadge {
    /// 传输列表：待下（`waiting`）+ 下载中（`active`）+ 失败（`error`）。
    ///
    /// ⚠️ `removed` 不算：它已经是"不用再管"的一档，出路在列表级的「清空已完成」，
    ///    把它算进"未完成"会让徽标永远减不到 0；`complete` 更不算。
    ///    没有快照（`nil`，还没轮询过）时是 0：没有数据不等于"有一堆没完成"。
    public static func unfinishedTransfers(_ list: TransferListResult?) -> Int {
        guard let list else { return 0 }
        return list.items.filter { isUnfinished($0.state) }.count
    }

    /// 校验结果：未通过项数（口径就是 `VerifySummary.failedCount` 那**一份**）。
    ///
    /// ⚠️ 这里**不再自己求一遍和**：以前它是 `failedCount` 的第二份实现（判据共用
    ///    `isFailure`，但求和写了两遍 —— 加一类失败、或改一次 `isFailure` 的口径，
    ///    两处就会分叉，而症状是"侧边栏徽标与校验屏的失败计数对不上"）。
    ///    `nil`（还没取过）→ 0：没有数据不等于"有一堆没通过"。
    public static func unpassed(_ status: VerifyStatus?) -> Int {
        VerifySummary.of(status)?.failedCount ?? 0
    }

    private static func isUnfinished(_ state: TaskState) -> Bool {
        switch state {
        case .waiting, .active, .error: return true
        case .complete, .removed: return false
        }
    }
}

// ---------------------------------------------------------------------------
// 总进度（`get_tree` 的 `progress`）
// ---------------------------------------------------------------------------

/// 「总进度」那一行：百分比 / 字节 / 速度 / 进度条要的那个数。
///
/// 校验视图顶部与侧边栏底部**共用**这一份口径（两处显示同一个数，
/// 只在一处算 —— 抄第二份就会出现"同一个进度在两处不一样"）。
public struct ProgressSummary: Equatable, Sendable {
    /// 「45%」；总量为 0 时是占位符 `—`（口径同 [`PercentFormat`]）。
    public let percentText: String
    /// 「1.0 KB / 4.0 KB」。
    public let bytesText: String
    /// 「1.2 MB/s」；速度为 0 时是 `—`（口径同 [`SpeedFormat`]）。
    public let speedText: String
    /// `ProgressView(value:)` 要的那个数，**恒在 `0...1`**（含总量为 0 的 0，绝不 NaN）。
    public let fraction: Double

    /// 取总进度；**树必须属于这一批**（`treeCode == code`），否则 `nil`。
    ///
    /// ⚠️ 那道闸不是多余的：`AppModel` 先落 `loadState = .loaded(info)`、**之后**才拉
    ///    `get_tree`，而视图是在 `.loaded` 那一刻出现的 —— 于是这里被问到时，
    ///    `tree` 可能还没到（`nil`）或者**还是上一批的**。把上一批的进度摆在
    ///    这一批的批次摘要底下，是一个没有任何提示的错数（静默失效）。
    ///    判据与 `BrowserSelection.onNewManifest` 的 `treeCode == code` **同一条**，
    ///    钉住它的是 `progressIsNotShownWhenTheTreeBelongsToAnotherBatch`。
    ///
    /// ⚠️ `code == nil`（没有生效批次）也返回 `nil`：那时"总进度"没有归属。
    public static func of(tree: TreeResult?, treeCode: String?, code: String?) -> ProgressSummary? {
        guard let code, let tree, treeCode == code else { return nil }
        let p = tree.progress
        return ProgressSummary(
            // 百分比与传输列表**共用** `PercentFormat`（同一个实现、同一条"总量为 0 说 —"的
            // 口径），不用内核那份 `percent` 字段：两者本来就同源（契约 §1.6），
            // 而自己再算一份就是这条口径的第二个实现。
            percentText: PercentFormat.text(done: p.doneBytes, total: p.totalBytes),
            bytesText: ByteFormat.text(p.doneBytes) + " / " + ByteFormat.text(p.totalBytes),
            speedText: SpeedFormat.text(p.speed),
            // 分数与传输列表**共用** `TransferRow.fraction`（同一份夹取/防 NaN 的实现）。
            fraction: TransferRow.fraction(completed: p.doneBytes, total: p.totalBytes))
    }
}

// ---------------------------------------------------------------------------
// 刷新失败 → 界面上那句话
// ---------------------------------------------------------------------------

/// 校验视图刷新时 `get_tree` 失败之后要显示的那句话。
///
/// ⚠️ **这是给视图用的唯一入口，视图自己不做映射**（全局约束 8：视图里不映射 `CoreError`）。
///    映射本身走 `AppModel.message(of:)` —— 那是 `CoreError` → 用户可见文案的**唯一实现**
///    （`DirLoadFailure.of` / `EnqueueFeedback.failure(of:)` / `TransferActionFailure` 用的也是它）。
///    本类型**不抄第二份**：抄了就会出现"同一个错误在两个界面上措辞不同"。
///
/// ⚠️ `refreshVerify()` 自己的失败**不走这里**：它是非抛错路径，原文由 `AppModel`
///    落在 `lastError` 上、由主区顶部那条常驻横幅（`EngineBanner`）显示 ——
///    在这里再显示一遍同一句话，只会让真正要看的那条变淡（同 `TransferListEmpty` 的理由）。
///    这里兜的是 `getTree()` 的**抛错**路径：它没有别的落点（约束 4）。
public enum VerifyRefreshFailure {
    public static func message(of error: Error) -> String {
        AppModel.message(of: error)
    }
}
