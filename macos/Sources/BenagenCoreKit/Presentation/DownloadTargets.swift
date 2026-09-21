import Foundation

// ---------------------------------------------------------------------------
// 「下载」这个动作的呈现模型。
//
// 全局约束 8：一切"把界面动作 / 内核数据变成界面值"的纯计算都放 `Presentation/`，
// **每条都要有单测**；视图里只留绑定与渲染分派。本文件不 `import SwiftUI`。
//
// 本文件装的是同一个动作的两个纯计算：
//   ① `DownloadTargets` —— 点下去该发什么（勾选面 → `paths`、按钮文案）；
//   ② `EnqueueFeedback` —— 内核回了什么、界面该怎么说（`added` / `rejected` 两边都要有落点）。
// ---------------------------------------------------------------------------

/// 勾选面 → `enqueue` 的 `paths`，以及那颗按钮怎么呈现。
///
/// ⚠️ **壳不展开目录**（约束 1）：内核的 `resolve_targets` 把每个 path 当**前缀**处理
///    （`f.path == p || f.path.starts_with("{p}/")`），所以**目录路径原样交给它**就行，
///    它会展开成这个目录下的所有文件。壳自己展开会把"哪些文件属于这个目录"这条语义
///    复制到壳里，而且两边的边界条件（空目录、路径末尾的 `/`）会漂移。
///
/// ⚠️ **排序**（简报点名）：勾选面是 `Set<String>`，遍历顺序不稳定 —— 不排序会让每一次
///    请求的 `paths` 顺序都不同，排障时没法比对两次请求。用 Swift 的 `<`
///    （与语言环境无关，同 `BrowserRow.rows` 的口径），不是 `localizedStandardCompare`。
public enum DownloadTargets {

    /// 勾选面 → `enqueue` 的 `paths`。**整批全选是一个特例**。
    ///
    /// ⚠️ **整批全选必须发空数组，这不是优化，是正确性**（全局约束 C-3）：
    ///    `enqueue.paths` 的请求体随勾选面线性增长，而内核的行长上限是 8 MiB
    ///    （`core/src/main.rs:113`）。超长行的后果**不是报错**：内核只回一条 `id == 0`
    ///    的协议告警、那条请求**永远等不到响应**、`CoreClient` 的串行队列**被永久堵死**，
    ///    而界面上一个字都不说。
    ///    整批全选发 `[]` 是**语义等价**的：内核的 `paths` 为空 = 全部**待下载**
    ///    （`view::pending_paths`），而且请求体**恒定**（不随批次大小增长）。
    ///
    /// ⚠️ 判据是 `selection == allPaths`（**文件**路径的全集，来自 `get_tree` 的 `flat`）。
    ///    勾选面里可能还有目录，但那只会让两边**不相等** ⇒ 走显式列表，也就是保守的一边。
    ///    `allPaths` 为空时**不算**整批 —— 否则"一批里一个文件都没有"会被当成"全选"。
    ///
    /// ⚠️ **没有旧签名的重载**（裁定 C2）：一个"传没传整批都能编过"的重载，
    ///    正好能让上面这条硬要求被悄悄绕过去。
    ///
    /// **空集合 → 空数组**：内核的语义是「`paths` 为空 = 下全部待下载」，
    /// 所以这里不能"什么都不发"，只能发一个空数组（`AppModel.enqueue` 会把 `[]` 原样发出去）。
    public static func paths(for selection: Set<String>, allPaths: Set<String>) -> [String] {
        if !allPaths.isEmpty && selection == allPaths { return [] }
        return selection.sorted()
    }

    /// 一条 `enqueue` 请求的参数体**编码后**有多少字节。
    ///
    /// 用与线上**同一个**编码器（`CoreJSON.encoder`）量，不是估算：
    /// 估算要么把合法的请求误杀，要么把超限的放过 —— 两种都是这个守卫的本意要防的。
    /// 编码失败按 `Int.max` 计（保守：宁可拒绝，不发一条可能卡死的请求）。
    public static func requestBytes(_ paths: [String]) -> Int {
        let params = JSONValue.object(["paths": .array(paths.map { .string($0) })])
        guard let data = try? CoreJSON.encoder.encode(params) else { return Int.max }
        return data.count
    }

    /// 安全预算：内核上限（8 MiB）的一半。
    ///
    /// ⚠️ 留一半的余量给**信封**（`id` / `method`）与任何我们没算进去的部分。
    ///    这个守卫宁可早一点拒绝，也不能放过一条会**永久堵死客户端**的请求。
    public static let requestBudgetBytes = 4 << 20

    public static func exceedsRequestBudget(_ paths: [String]) -> Bool {
        requestBytes(paths) > requestBudgetBytes
    }

    /// 一项都没勾时，底栏那一行要说的话（**明说**会发生什么，不让用户猜）。
    public static let emptySelectionHint = "未勾选任何项，将下载全部待下载文件"

    /// 那颗按钮的文案。**未选中任何项时是「全部下载」**（简报）。
    ///
    /// 它不是"禁用"而是"换一句话说"：那时发出去的是 `paths: []`（= 全部待下载），
    /// 文案必须与真正会发生的事一致 —— 一颗写着「下载选中」却下了全部的按钮是在骗人。
    ///
    /// ⚠️ 数值上只说"有 / 没有勾选"这一件事，**不拼项数**：项数已经由底部状态栏的
    ///    `SelectionSummary.countText`（「已选 N 项」）说过了，两处各说一遍迟早会不一致。
    public static func buttonTitle(for selection: Set<String>) -> String {
        selection.isEmpty ? "全部下载" : "下载选中"
    }

    /// 那颗按钮的悬停提示。**工具栏与底部状态栏共用这一份** ——
    /// 两个入口各写一句文案，改一处就会出现"同一个按钮两个说法"。
    public static let helpText = "把选中的文件加入下载队列（未选中时 = 全部待下载文件）"
}

/// `enqueue` 的**回执** → 界面上要说的话。
///
/// ⚠️ 约束 4（不得静默少交）：`added` 与 `rejected` **两边都要有落点**。
///    - `added` 非空 → 切到传输列表（那里面有它们；任务 8 的交付物）；
///    - `rejected` 非空 → **逐条**把 `path` 与 `reason` 摆出来 ——
///      "这个文件没能加进下载列表"必须出现在界面上。
///
/// ⚠️ 这也是为什么 `added` 为空时**不切**分区：一切，用户就再也看不到那些拒绝理由了
///    （传输列表里根本没有这些文件）。留在原地把理由说清楚才是约束 4 要的。
///    这一条在"D-2 那条空回执"（`added` 空、`rejected` 也空）上同样成立、理由更直接：
///    传输列表里一个任务都没有，切过去只会看到一片空，还会把壳写的那句话吞掉。
///
/// 这个类型是**纯值**：视图只负责把 `summary` 与 `rejections` 摆到屏幕上。
public struct EnqueueFeedback: Equatable, Sendable {

    /// 一条没能加进下载列表的路径。两个字段都是**内核原文，逐字**（约束 3）。
    public struct Rejection: Equatable, Hashable, Sendable {
        public let path: String
        public let reason: String

        public init(path: String, reason: String) {
            self.path = path
            self.reason = reason
        }
    }

    /// 顶上那句话。成功时是壳写的**计数**（内核只给了数组，没给"几个"这句话 ——
    /// `added` 为空时那句同样是壳写的，理由见 `of`）；
    /// 失败时它就是**内核原文**（约束 3：一个字都不加）。
    public let summary: String

    /// 逐条拒绝理由（可能为空）。
    public let rejections: [Rejection]

    /// `added` 非空 → 切到传输列表。
    public let switchesToTransfers: Bool

    /// 内核回执 → 要说的话。
    ///
    /// ⚠️ **`added` 为空是一条真实回执，而且是本阶段修的那个 bug 的正面**（D-2）：
    ///    `paths` 为空（界面「全部下载」）且内核算出的待下载集合为空时，内核回**成功**、
    ///    `added` 空数组。改之前它回的是 `invalid_params("没有匹配到任何文件")`
    ///    （`resolve_targets` 的 `out.is_empty()` 早退，`core/src/main.rs:1211-1213`），
    ///    于是"合法为空"被当成错误 —— 那正是用户看到的那句报错。
    ///    （"`Ok` 路径上 `added` 必然非空"那条推导描述的是**改之前**的内核，别再拿它当
    ///     "这种回执不可能出现"的证据；诊断报告 §7 第 3 条就是它。）
    ///
    /// 🔴 **"没有需要下载的文件"这句是壳自己写的**（约束 C-7/C-10 要求写明理由）：
    ///    这条回执上**内核一个字都没说** —— 它不是错误，没有 `message` 可登（约束 3 的例外
    ///    就是这种"内核没有原文"的场合），而"点了下载、界面什么都不说"恰恰是约束 4 明禁的
    ///    静默失效。所以壳把**事实**说出来：内核算出的待下载集合是空的 ⇒ 没有需要下载的文件。
    ///    ⚠️ 措辞的纪律：**只陈述事实** ——
    ///      - **不加判断**：不说"可能已经全部下载完成"（"为什么是空的"这份知识壳手里没有，
    ///        那是内核算出来的，壳替它下结论就是在编）；
    ///      - **不加建议**：不说"请稍后再试""请检查文件"之类（壳不知道该怎么办，
    ///        也不该替用户决定）；
    ///      - 不写"成功/失败"这种评价（这既不是失败，也不该被读成"下好了"）。
    ///
    /// ⚠️ 于是"**逐条** `path` + `reason`"只在**部分成功**（`added` 非空且 `rejected` 非空）
    ///    这条真实形状上做。**全拒那条路上不做逐条渲染，这是有意偏离（约束 11）**，
    ///    理由：那种情形下的 `rejected` 只活在**内核拼的 Rust `Debug` 串**里
    ///    （`[Object {"path": …, "reason": …}]`），**不是结构化字段**。壳要"逐条"就得去切
    ///    一句内核随时会改的措辞 —— 而"壳不解析内核文案"（约束 2/3）与"不改 `core/`"（约束 10）
    ///    是底线；宁可整条原文照登（约束 4 的"不静默少交"已经满足：用户看得到全部信息）。
    public static func of(_ result: EnqueueResult) -> EnqueueFeedback {
        EnqueueFeedback(summary: result.added.isEmpty
                            ? "没有需要下载的文件"
                            : "已加入 \(result.added.count) 个下载任务",
                        rejections: result.rejected.map { Rejection(path: $0.path, reason: $0.reason) },
                        switchesToTransfers: !result.added.isEmpty)
    }

    /// 失败（`engine_start_failed` / `preflight_failed` / 引擎不可用时那条闸门）：
    /// **原地显示内核原文**、**不切视图**（简报 —— 切走了，那句话就没人看见了）。
    ///
    /// 原文映射走 `AppModel.message(of:)` —— 那是 `CoreError` → 用户可见文案的**唯一**实现
    /// （`DirLoadFailure.of` 用的也是它）。本文件不抄第二份，视图里也不映射：
    /// 视图只把这个值摆到屏幕上（约束 8）。
    public static func failure(of error: Error) -> EnqueueFeedback {
        EnqueueFeedback(summary: AppModel.message(of: error),
                        rejections: [],
                        switchesToTransfers: false)
    }
}

/// 一条下载回执 **+ 它属于哪一批**。
///
/// ⚠️ 为什么需要"属于哪一批"：回执条挂在主区上方、**切分区也不消失**，而它可能在**换批之后**
///    还留在屏幕上（约束 4：拒绝理由不能因为用户换了批次就消失 —— 那是"静默少交"）。
///    但"已加入 4 个下载任务"旁边一旦**没有**批次码，用户会把它读成**新批次**的结果。
///    取舍：**不无条件清掉**（约束 4），而是把批次码摆在前面 —— 见 `summary(currentCode:)`。
public struct DownloadNotice: Equatable, Sendable {
    /// 这条回执是哪一批的（点击那一刻 `loadState` 里的码；理论上不会是 nil，
    /// 因为工具栏那颗按钮在"没有生效批次"时是禁用的 —— 留着是为了不把话说不出来）。
    public let code: String?
    public let feedback: EnqueueFeedback

    public static func of(_ result: EnqueueResult, code: String?) -> DownloadNotice {
        DownloadNotice(code: code, feedback: .of(result))
    }

    public static func failure(of error: Error, code: String?) -> DownloadNotice {
        DownloadNotice(code: code, feedback: .failure(of: error))
    }

    /// 这条回执属于当前这批吗。`nil == nil` 算"属于"（没有码就没有"哪一批"可比，
    /// 这时不该在界面上凭空多出一个"批次"字样）。
    public func belongsTo(to currentCode: String?) -> Bool {
        code == currentCode
    }

    /// 回执条顶上那句话。**不属于当前批时前面加上批次码** ——
    /// 用户必须一眼看出"这条不是我刚加载的这批的结果"。
    public func summary(currentCode: String?) -> String {
        guard !belongsTo(to: currentCode) else { return feedback.summary }
        return "批次 \(code ?? "—")：\(feedback.summary)"
    }
}
