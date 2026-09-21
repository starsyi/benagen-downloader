import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 文件浏览器的行 / 选择 / 汇总（全局约束 8：`Presentation/` 里每条纯计算都要有单测）
//
// ⚠️ 夹具一律是**内核会发的那种线上 JSON 文本**（键是 snake_case，目录项**没有 `path` 键**），
//    经 `CoreJSON.decoder` 解成 `ListDirResult` —— 手搓结构体会让"壳解不解得动内核的输出"
//    在测试里凭空消失（`AppModelTests` 顶部的同一条理由）。
//
// ⚠️ 夹具的顺序是**故意打乱**的（文件、目录、文件、目录），所以下面那条排序断言
//    对"不排序 / 只按名字排 / 文件在前"三种变异体都有判别力 —— 见报告里的变异自检。
// ---------------------------------------------------------------------------

/// 一层目录的线上原文（`list_dir` 的 result）。
/// 路径是清单原文：`×` 与空格逐字，不得转义。
private let listDirWire = #"""
{"path":"client-test/C24-8_×_25WS024",
 "entries":[{"type":"file","name":"QC 图.png",
             "path":"client-test/C24-8_×_25WS024/QC 图.png","crc64":"",
             "size":7000,"completed":0,"total":7000,"speed":0,"state":"pending","err":""},
            {"type":"dir","name":"Zeta","children_count":1},
            {"type":"file","name":"reads.fq.gz",
             "path":"client-test/C24-8_×_25WS024/reads.fq.gz","crc64":"9988776655443322110",
             "size":2048,"completed":2048,"total":2048,"speed":0,"state":"complete","err":""},
            {"type":"dir","name":"Figure","children_count":3}]}
"""#

/// 根那一层：只有一个顶层目录（用来钉"根下不得出现前导 /"）。
private let rootWire = #"""
{"path":"","entries":[{"type":"dir","name":"client-test","children_count":2}]}
"""#

private let parent = "client-test/C24-8_×_25WS024"

private func entries(_ json: String) throws -> [DirEntry] {
    try CoreJSON.decoder.decode(ListDirResult.self, from: Data(json.utf8)).entries
}

private func rows(_ json: String, parent: String) throws -> [BrowserRow] {
    BrowserRow.rows(try entries(json), parent: parent)
}

private func row(_ name: String) throws -> BrowserRow {
    try row(in: listDirWire, parent: parent, named: name)
}

/// 任意一份夹具里的某一行。
private func row(in json: String, parent: String, named name: String) throws -> BrowserRow {
    let all = try rows(json, parent: parent)
    guard let hit = all.first(where: { $0.name == name }) else {
        Issue.record("夹具里没有 \(name)")
        throw CoreError.transport("夹具里没有 \(name)")
    }
    return hit
}

/// 一层目录里的**文件叶节点**（`DirEntry` → `FileNode`；目录项没有 `FileNode`）。
private func fileNodes(_ json: String) throws -> [FileNode] {
    try entries(json).compactMap {
        guard case .file(let f) = $0 else { return nil }
        return f
    }
}

/// 某个文件叶节点的 `sourceMtime`。
///
/// ⚠️ 用这个而不是在断言里写 `fileNodes(…).first { … }?.sourceMtime`：后者是
///    `String??`，与字符串字面量比较时靠 `Optional` 的隐式提升，读起来是"比较两个
///    可选值"还是"比较值与 nil"分不清（`?? nil` 把它压回 `String?`）。
private func sourceMtime(_ json: String, _ name: String) throws -> String? {
    try fileNodes(json).first { $0.name == name }?.sourceMtime ?? nil
}

// ---------------------------------------------------------------------------
// 行的判别键与路径
// ---------------------------------------------------------------------------

@Test func rowsCarryTheTypeDiscriminatorFromTheCore() throws {
    // 判别键是 `"type"`（`"dir"` / `"file"`），**不是** `is_dir` 之类的布尔
    // （`core/src/main.rs` 的 `entries_json`）。
    #expect(try row("Figure").kind == .dir)
    #expect(try row("reads.fq.gz").kind == .file)
}

@Test func dirRowsGetTheirPathRebuiltByTheShell() throws {
    // ⚠️ `list_dir` 的目录项**没有 `path` 键**（只有 `children_count`）——
    //    所以目录的路径只能由壳拼：父路径 + "/" + name。
    //    变异体：把目录项的 path 直接取 `row.name`（或留空）⇒ 双击进去会发一条错路径。
    #expect(try row("Figure").path == "client-test/C24-8_×_25WS024/Figure")
    #expect(try row("Zeta").path == "client-test/C24-8_×_25WS024/Zeta")

    // 根那一层：不得出现前导 "/"（`join(parent: "", …)` 那条规则在真实输入上的样子）。
    let top = try rows(rootWire, parent: "")
    #expect(top.map(\.path) == ["client-test"])
}

@Test func fileRowsKeepTheKernelPathVerbatim() throws {
    // 文件**有** `path` 键，逐字用它的（约束 3：不规范化、不转义）。
    #expect(try row("QC 图.png").path == "client-test/C24-8_×_25WS024/QC 图.png")
    #expect(try row("QC 图.png").name == "QC 图.png", "`×` 与空格原样显示")
}

// ---------------------------------------------------------------------------
// 三列的值
// ---------------------------------------------------------------------------

@Test func dirRowShowsChildCountAndNoSize() throws {
    let d = try row("Figure")

    #expect(d.childrenCount == 3)
    // 「没有大小这回事」与「大小是 0」不是一件事：目录的大小得等内核展开才知道
    // （壳不展开目录，约束 1），所以它必须是 nil，而不是 0。
    #expect(d.size == nil)
    #expect(d.detailText == "3 项", "目录那一格显示的是子项数，不是大小")
    #expect(d.state == .none, "目录没有四态")
    #expect(d.iconName == "folder")
}

@Test func fileRowShowsSizeAndStateColor() throws {
    let f = try row("QC 图.png")

    #expect(f.size == 7000)
    #expect(f.childrenCount == nil)
    #expect(f.detailText == ByteFormat.text(7000), "口径复用 ByteFormat（1024 进制、一位小数）")
    #expect(f.detailText == "6.8 KB")
    #expect(f.state == .pending)
    #expect(f.state.color == .secondary)
    #expect(f.iconName == "doc")

    // 已完成那个也要看一眼：`complete` 与 `pending` 在界面上必须是两回事。
    #expect(try row("reads.fq.gz").state == .complete)
    #expect(try row("reads.fq.gz").state.color == .green)
}

@Test func eachOfTheFourStatesMapsToItsOwnLabelAndColor() throws {
    // 内核的四态（`FileState`）一个都不能少、一个都不能并。
    #expect(FileState.allCases.count == 4, "内核的四态")

    let styles = FileState.allCases.map { RowStateStyle.of($0) }

    #expect(styles == [.pending, .downloading, .complete, .failed], "顺序即 FileState.allCases")
    #expect(Set(styles.map(\.label)).count == 4, "四个标签必须两两不同")
    #expect(Set(styles.map(\.color)).count == 4, "四个颜色必须两两不同")
    #expect(styles.map(\.label) == ["待下载", "下载中", "已完成", "失败"])
    #expect(styles.map(\.color) == [.secondary, .blue, .green, .red])
}

@Test func stateLabelIsNeverEmpty() {
    // 任何一格都不许是空白 —— 空白在界面上就是"什么都没有"，而约束 4 要的是
    // 「不得静默失效」：状态说不出来，也是一件要说出来的事。
    #expect(RowStateStyle.allCases.count == 5, "四态 + 「没有状态」（目录行）")
    for s in RowStateStyle.allCases {
        #expect(!s.label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                "\(s) 的标签是空白")
        #expect(s.label != "nil", "不得把 Swift 的 nil 漏到界面上")
    }
    #expect(RowStateStyle.of(nil) == .none)
    #expect(RowStateStyle.of(nil).label == "—", "占位字形与 Format.swift 的 SpeedFormat 同一个")
}

// ---------------------------------------------------------------------------
// 排序
// ---------------------------------------------------------------------------

@Test func rowsAreSortedDirsFirstThenByName() throws {
    // 目录在前、同组内按名字升序。夹具顺序是
    // `[QC 图.png, Zeta, reads.fq.gz, Figure]`（故意打乱），所以：
    //   不排序            → [QC…, Zeta, reads…, Figure]
    //   只按名字排        → [Figure, QC 图.png, Zeta, reads…]（目录被拆散）
    //   文件在前          → [QC…, reads…, Figure, Zeta]
    //   分组但不排序      → [Zeta, Figure, QC…, reads…]
    //   比较器恒真        → 每组按夹具顺序**倒序** → [Figure, Zeta, reads…, QC…]（文件那组反了）
    // 五种都会被下面这条断言判红。
    //
    // ⚠️ **夹具的同类项必须有一组是"升序"的**（这里文件组是 `[QC 图.png, reads.fq.gz]`）。
    //    这条不是随手排的：上一版夹具两组**恰好都是降序**（`[reads…, QC…]` / `[Zeta, Figure]`），
    //    于是"比较器恒真 ⇒ 每组倒序"的输出**正好等于期望顺序**，那个变异体活了下来。
    //    夹具给变异体打掩护，是本项目栽过八次的那类问题 —— 见报告 §5.12 与本任务修复报告 §2。
    #expect(try rows(listDirWire, parent: parent).map(\.name)
            == ["Figure", "Zeta", "QC 图.png", "reads.fq.gz"])
}

// ---------------------------------------------------------------------------
// 选择：默认选中面 / 全选当前层 / 底部汇总
// ---------------------------------------------------------------------------

private func tree(_ json: String) throws -> TreeResult {
    try CoreJSON.decoder.decode(TreeResult.self, from: Data(json.utf8))
}

private let treeWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"a.bin","name":"a.bin","size":2048,"state":"pending"},
         {"path":"b.bin","name":"b.bin","size":3072,"state":"complete"}],
 "default_selected":["a.bin"],
 "progress":{"total_bytes":5120,"done_bytes":3072,"speed":0,"percent":60}}
"""#

/// **上一批**的树。它的 `default_selected` 与 [`treeWire`] **不同**（`z.bin`）——
/// 这样"新批次被播上了旧批次的路径"才是**可观测**的（两边一样的话，播错了也看不出来）。
private let treeOldWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"z.bin","name":"z.bin","size":1024,"state":"pending"}],
 "default_selected":["z.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}
"""#

/// **另一批**的树：`default_selected` 与 [`treeWire`]、[`treeOldWire`] 都不同（`b.bin`）——
/// 用来钉"换批播的是**这一批**的面，不是在别处播过的那一份"。
private let treeBWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"b.bin","name":"b.bin","size":1024,"state":"pending"}],
 "default_selected":["b.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}
"""#

@Test func aNewManifestReseedsTheSelectionFromTheCore() throws {
    // ⚠️ 简报点名的关键：批次加载后的默认选择来自 `get_tree` 的 `default_selected`
    //    （内核给的"所有非 complete 的文件"）—— **不是**当前层的全部条目。
    let seeded = BrowserSelection.onNewManifest(code: "C24-8", previousCode: nil,
                                                treeCode: "C24-8", tree: try tree(treeWire))
    #expect(seeded == ["a.bin"])
    #expect(seeded?.contains("b.bin") == false, "已完成的文件不该被默认勾上")
}

@Test func theSameManifestDoesNotWipeTheUsersSelection() throws {
    // 同一批（刷新 / 目录来回切 / **切分区回来**）时返回 nil = **不动**：用户跨目录攒的选择
    // 不能被一次平凡的重绘抹掉。
    #expect(BrowserSelection.onNewManifest(code: "C24-8", previousCode: "C24-8",
                                           treeCode: "C24-8", tree: try tree(treeWire)) == nil)

    // 换一批才重设（而且是**清空后重设**，不是并集）。
    #expect(BrowserSelection.onNewManifest(code: "BBB-2", previousCode: "C24-8",
                                           treeCode: "BBB-2", tree: try tree(treeWire)) == ["a.bin"])
}

@Test func seedingWaitsForThisBatchesTree() throws {
    // ⚠️⚠️ 这条挡的是**一条真实的竞速**（审查重要 ①）：
    //     `AppModel` 先落 `loadState = .loaded(info)`，**之后**才去拉 `get_tree`
    //     （`performLoadDelivery` → `surfacing { getTree() }`），而视图是在 `.loaded`
    //     那一刻出现的 —— 所以壳来问"该播什么"时，树**多半还没到**。
    //
    //     判据两条（简报/审查给的）：
    //       ① 一个批次的默认选择，**绝不能被另一个批次的树播种**；
    //       ② 树还没到时，播种**不得被消费** —— 要推迟到这一批的树到位那一刻再播。
    //
    //     "消费"这件事发生在这个纯函数的**调用方**（视图把 `previousCode` 记成当前码）。
    //     所以这里用"返回 nil = 不动"来表达"不许消费"：调用方只有在拿到非 nil 时才记账。
    let newTree = try tree(treeWire)
    let oldTree = try tree(treeOldWire)

    // ① 树还没到（`treeCode == nil`）：推迟 —— **不是**"播一个空集"。
    //    播空集就等于消费掉了这一次机会，而视图的 taskKey 里当时没有树，
    //    此后**再没有任何 re-seed 路径** ⇒ 用户开箱看到「已选 0 项」，
    //    简报点名的"点开就能直接点下载"**静默失效**。
    #expect(BrowserSelection.onNewManifest(code: "NEW", previousCode: nil,
                                           treeCode: nil, tree: nil) == nil)

    // ①' 树还是**上一批**的：更不能播（那些路径在这一批里可能指向别的文件）。
    #expect(BrowserSelection.onNewManifest(code: "NEW", previousCode: nil,
                                           treeCode: "OLD", tree: oldTree) == nil)
    // 反向的一半：真拿上一批的树去播，播出来的确实是**别的路径** —— 这条断言
    // 让"不播"这件事有判别力（否则"恒返回 nil"也能让上面两条过）。
    #expect(oldTree.defaultSelected == ["z.bin"], "上一批的默认选中面与这一批不同")

    // ② 这一批的树到位了：播。
    #expect(BrowserSelection.onNewManifest(code: "NEW", previousCode: nil,
                                           treeCode: "NEW", tree: newTree) == ["a.bin"])
}

@Test func aTreeClaimingThisBatchButMissingIsNotSeeded() {
    // 防御性的一支：`treeCode` 说"就是这一批"、`tree` 却是 nil（正常构造不出来 ——
    // `AppModel` 里两行是一起落的）。这时**推迟**比"播空集"安全：真出现了也要能自愈。
    #expect(BrowserSelection.onNewManifest(code: "NEW", previousCode: nil,
                                           treeCode: "NEW", tree: nil) == nil)
}

@Test func selectAllTakesOnlyTheCurrentLevel() throws {
    let level = try rows(listDirWire, parent: parent)
    let all = BrowserSelection.all(in: level)

    #expect(all == Set(level.map(\.path)))
    #expect(all.count == 4, "当前层四项")
    // 目录也被选上（内核在 enqueue 里按前缀展开目录 —— 展开是内核的事，约束 1）。
    #expect(all.contains("client-test/C24-8_×_25WS024/Figure"))
    // 别的层不在里面。
    #expect(all.contains("client-test") == false)
}

/// `flat` 里 `path` 与 `name` **不同**（有前缀、带 `×` 与空格）—— 用来钉"取的是 `path`"。
/// `treeWire` 那两条的 path 与 name 恰好一样，"取错字段"在它上面**不可观测**。
private let treeWithPrefixWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"C24-8_×_25WS024/Figure/QC 图.png","name":"QC 图.png","size":1,"state":"pending"}],
 "default_selected":[],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}
"""#

@Test func allFilesTakesEveryFilePathVerbatim() throws {
    // `allFiles` 是**整批的文件**（`flat` 只列文件，目录不在里面），
    // 它是"勾选面覆盖了整批吗"的对照值（`DownloadTargets.paths(for:allPaths:)`）——
    // 判据是"覆盖整批 ⇒ 发 `[]`"（约束 C-3：超限请求不报错，只把客户端静默堵死）。
    let files = BrowserSelection.allFiles(in: try tree(treeWire).flat)
    #expect(files == ["a.bin", "b.bin"], "整批的文件，一条不少")

    // ⚠️ 取的是 **`path`**，不是 `name`：内核给的原样（约束 3）。
    //    这一条有判别力 —— 夹具里两者**不同**（`treeWire` 里它们恰好一样，取错也看不出来）。
    #expect(BrowserSelection.allFiles(in: try tree(treeWithPrefixWire).flat)
            == ["C24-8_×_25WS024/Figure/QC 图.png"])

    // 空批次 → 空集（`DownloadTargets.paths` 靠它把"一批里一个文件都没有"与"全选"分开：
    // `allPaths` 为空时那一步**不算整批**）。
    #expect(BrowserSelection.allFiles(in: []).isEmpty)
}

/// **双击进入一个目录之后，那个目录的路径必须从勾选面里消失。**
///
/// ⚠️ 这条钉的是一个**真实交付事故**（2026-09-20 客户现场）：
///    双击进入 `A/B` 时，`List` 的选中绑定会先把 `"A/B"` 写进勾选面 ——
///    双击的**第一下就是一次单击选中**（`FileBrowser` 里 `.contextMenu` 那段注释
///    写着同一件事："在 macOS 上，双击前单击已经把它选出来了"）。
///    那之后底栏显示「已选 1 项」（**用户什么都没勾**），再点一个文件就成了「已选 2 项」，
///    而按「下载选中」时内核把 `"A/B"` 当**目录前缀**展开
///    （`core/src/main.rs:1254` 的 `resolve_targets`）⇒ **整个目录都被下了**。
///
/// 判据：进目录是**导航**，不是"我要下这个目录"。所以摘掉的是**被进入的那一个**，
/// 勾选面里其余的一切原样不动（进目录不是换批，不许清勾选面 —— README 11c）。
@Test func enteringADirectoryDropsThatDirectoryFromTheSelection() {
    let selection: Set<String> = ["C24-8_×_25WS024/Figure", "C24-8_×_25WS024/Figure/QC 图.png"]

    let after = BrowserSelection.afterEntering("C24-8_×_25WS024/Figure", selection: selection)

    #expect(after == ["C24-8_×_25WS024/Figure/QC 图.png"],
            "被进入的那个目录路径必须摘掉（它是双击的副作用），用户勾的文件一个不能少")
    #expect(after.contains("C24-8_×_25WS024/Figure") == false)
}

/// `afterEntering` 的另外三面：**除了那一个，什么都不动**。
@Test func enteringADirectoryChangesNothingElse() {
    let selection: Set<String> = ["A", "B", "B/x.bin", "C/y.bin"]

    // ⚠️ **精确匹配，不是前缀**：摘掉的只有 `"B"` 这一条。
    //    `"B/x.bin"` 是**用户自己勾的**（他在里面下过一个文件），必须留着 ——
    //    按前缀摘会把用户攒的勾选一起清掉，那正是这次修复要避免的另一半。
    #expect(BrowserSelection.afterEntering("B", selection: selection)
            == ["A", "B/x.bin", "C/y.bin"])
    // 不在勾选面里的路径：原样返回。**幂等** —— 进同一层两次不会出别的事，
    // 而"目录不在勾选面里"恰恰是修好之后最常见的形状（双击第一下写的那个已被摘掉）。
    #expect(BrowserSelection.afterEntering("Z", selection: selection) == selection)
    // 空勾选面：还是空（不许把"没勾任何东西"变成别的什么）。
    #expect(BrowserSelection.afterEntering("A", selection: []).isEmpty)
}

@Test func selectionSummaryCountsItemsAndSumsKnownSizes() throws {
    let index = SelectionSummary.sizeIndex(try tree(treeWire).flat)

    let s = SelectionSummary.of(selected: ["a.bin", "b.bin"], sizes: index)
    #expect(s.countText == "已选 2 项")
    #expect(s.sizeText == "合计 5.0 KB", "2048 + 3072 = 5120 → 5.0 KB（1024 进制）")

    // 空选择：说 0，不是空白（约束 4）。
    let empty = SelectionSummary.of(selected: [], sizes: index)
    #expect(empty.countText == "已选 0 项")
    #expect(empty.sizeText == "合计 0 B")
}

@Test func selectionSummaryIgnoresDirsAndUnknownPaths() throws {
    let index = SelectionSummary.sizeIndex(try tree(treeWire).flat)

    // 目录在 `flat` 里没有大小（`flat` 只有文件）。它的**项数**照样算，
    // 大小只能按 0 计 —— 壳不自己展开目录去猜（约束 1）。
    let s = SelectionSummary.of(selected: ["client-test/C24-8_×_25WS024/Figure", "a.bin"],
                                sizes: index)
    #expect(s.countText == "已选 2 项")
    #expect(s.sizeText == "合计 2.0 KB")
}

// ---------------------------------------------------------------------------
// 换批复位的判据（任务 7 承重事项 A）与 `.task(id:)` 的触发键（承重事项 B）
// ---------------------------------------------------------------------------

@Test func onlyAChangedCodeCountsAsANewManifest() {
    // ⚠️ **这条只钉"码变了吗"这条取值规则本身** —— 它**不是**承重事项 A 的守卫：
    //    A 的性质是"复位发生在 code 变化那一刻、而且对照的是上一次**显示**过的码"，
    //    那两件事都发生在视图里（`RootView.onChange` → `BrowserSelection.onLoad`），
    //    视图不单测（约束 8）。真正守 A 的是 `onLoad` 那两条（下一个测试）。
    //    （审查次要 ④：原来的名字/注释读起来像是在守 A，其实零判别力 —— 改名。）
    #expect(BrowserSelection.isANewManifest(code: "BBB-2", previousCode: "AAA-1"))
    #expect(BrowserSelection.isANewManifest(code: "AAA-1", previousCode: nil),
            "第一次加载（还没有上个码）也是一批新清单")
    #expect(!BrowserSelection.isANewManifest(code: "AAA-1", previousCode: "AAA-1"))
}

@Test func aLoadBackToThePreviousBatchIsStillANewManifest() {
    // ⚠️ **承重事项 A 的守卫（审查重要 ③）**：`A→B→A` 的第三步必须复位。
    //
    // 现场：① 显示 A（记下 "A"）→ ② 切到 B（复位一次，"记住的码"变成 "B"）→
    //       ③ 用户在 B 的树到之前勾了几项（勾选不依赖树）→ ④ **又回到 A**。
    // 第 ④ 步如果拿**"已经播过种的码"**（`ManifestTracking.seededCode`）当对照，算出来是"同一批"（它停在 "A"，因为
    // 复位有意不消费它、B 又还没播种）⇒ 不复位 ⇒ **B 的勾选面被拿去给 A 发 `enqueue`** ——
    // 正是 A 要修的那个缺陷，只是换了顺序；同一现场还有"静默"那一半（A 的默认选中面
    // 再也播不下来）。对照"上一次**显示**过的码"就不一样：第 ④ 步是换批。
    let first = BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: nil)
    #expect(first.isANewManifest, "第一次加载就是一批新清单")
    #expect(first.lastDisplayedCode == "AAA-1")

    let second = BrowserSelection.onLoad(code: "BBB-2", lastDisplayedCode: first.lastDisplayedCode)
    #expect(second.isANewManifest)
    #expect(second.lastDisplayedCode == "BBB-2", "记住的码要推进到 B")

    let back = BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: second.lastDisplayedCode)
    #expect(back.isANewManifest, "A→B→A 的第三步是换批：不复位就会把 B 的路径发给 A")
    #expect(back.lastDisplayedCode == "AAA-1")
}

@Test func aNilLoadNeitherResetsNorForgetsTheRememberedCode() {
    // 加载中 / 加载失败 / 内核回"没有生效的批次"时码是 nil（`loadState` 不再是 `.loaded`）：
    // **既不复位、也不推进**那个记住的码。两半都要：
    //   - 不复位：`.loading` 那一下抖动不该抹掉用户在同一批里攒的选择；
    //   - 不推进（尤其**不能冲成 nil**）：否则紧接着的同码加载会被误判成换批，
    //     把同一批里的选择抹掉（T6 的 `theSameManifestDoesNotWipeTheUsersSelection`）。
    let mid = BrowserSelection.onLoad(code: nil, lastDisplayedCode: "AAA-1")
    #expect(!mid.isANewManifest)
    #expect(mid.lastDisplayedCode == "AAA-1", "nil 不得把记住的码冲掉")

    let sameAgain = BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: mid.lastDisplayedCode)
    #expect(!sameAgain.isANewManifest, "同一批（重新加载）不复位")
    #expect(sameAgain.lastDisplayedCode == "AAA-1")

    // 从"还没有任何码"开始的第一次 nil：也不该凭空造出一个码。
    let cold = BrowserSelection.onLoad(code: nil, lastDisplayedCode: nil)
    #expect(!cold.isANewManifest)
    #expect(cold.lastDisplayedCode == nil)
}

// ---------------------------------------------------------------------------
// `ManifestTracking`：承重事项 A 的**两半**（复位 + 播种）
// ---------------------------------------------------------------------------

@Test func theFirstLoadSeedsItsDefaultSelection() throws {
    // ⚠️ 这条钉的是"播种的对照值**不能**用 `lastDisplayedCode`"：
    //    显示那一刻它就已经等于新码了，`seed` 会判成"这一批已经播过" ⇒
    //    **第一次加载的默认选中面永远播不下来**（开箱看到「已选 0 项」，界面上一点异常都没有）。
    //    所以"播种"必须有自己的记账（`seededCode`，只有播成功才推进）。
    //
    // ⚠️ `display` / `seed` 都是 `mutating` 的，**本文件一律先把返回值落到 `let` 再断言**。
    //    不是语法上做不到 —— 实测（Swift 6 + 本机工具链）：
    //      - `#expect(t.seed(...)?.selection == ["a.bin"])` **能编过、且调用真的生效**
    //        （宏把两个操作数各绑一次，调用发生在绑定表达式里）；
    //      - 但**裸调用**那种写法（`#expect(t.seed(...))` / `#expect(t.display(code:), "说明")`）
    //        **编译不过**：`cannot use mutating member on immutable value: '$0' is immutable`。
    //    两种写法的差别不写在脸上，所以统一落 `let`——读的人不必去猜哪种能编过。
    var t = ManifestTracking()

    let isNew = t.display(code: "AAA-1")
    #expect(isNew, "第一次加载就是换批")
    let seeded = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)
    #expect(seeded?.selection == ["a.bin"])
}

@Test func theSameLoadIsNotSeededTwice() throws {
    // **同一次加载内**（`generation` 不变）目录来回切 / 切分区回来 / 树到位后的再进来：
    // **不能再播一次**（那会把用户攒的选择抹掉）。
    //
    // ⚠️ 判据是「哪一次**加载**」而不是「哪一批」（阶段 D 任务 B 改的）：同码重载时
    //    `code` 一个字都没变，内核却已经重新规划过整批 —— 按 `code` 记账就等于
    //    "同码重载永远不重播默认面"（诊断报告 §6.1 的猜想 2）。那一条由下面
    //    `aSameCodeReloadReseedsTheDefaultFace` 单独钉住。
    var t = ManifestTracking()
    _ = t.display(code: "AAA-1")
    let first = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)
    #expect(first?.selection == ["a.bin"])

    let again = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)
    #expect(again == nil, "同一次加载只播一次")

    let sameBatch = t.display(code: "AAA-1")
    #expect(sameBatch == false, "同一批不算换批")

    let afterRedisplay = t.seed(code: "AAA-1", treeCode: "AAA-1",
                                tree: try tree(treeWire), generation: 1)
    #expect(afterRedisplay == nil)
}

@Test func aSameCodeReloadReseedsTheDefaultFace() throws {
    // ⚠️⚠️ **阶段 D 任务 B 修的那一条**（诊断报告 §6.1 猜想 2，**已证实**）：
    //    同码重载**是一次新加载**，内核刚刚重新规划过整批 ⇒ `default_selected` 是新信息，
    //    必须重播。改之前它静默不播：`RootView` 那条 `.onChange(of: loadedCode)` 在同码时
    //    产生不了任何复位（`.switching` 那条路上 `loadedCode` 根本没变），而 `seed` 只按
    //    `code` 记账 ⇒ 判成"这一批已经播过种" ⇒ 返回 nil。
    //    后果是用户读成"我重载了，界面一点变化都没有"——正是"重新加载批次码也不行"那半句。
    //
    // ⚠️ 这里用**两棵不同的树**当"重载前后"，否则"重播了"与"没重播"在断言上不可观测。
    var t = ManifestTracking()
    _ = t.display(code: "C24-8")
    let before = t.seed(code: "C24-8", treeCode: "C24-8", tree: try tree(treeWire), generation: 1)
    #expect(before?.selection == ["a.bin"])

    // 同码重载（同一批、**新的一代**）⇒ 播成内核刚算出来的那一份。
    let afterReload = t.seed(code: "C24-8", treeCode: "C24-8",
                             tree: try tree(treeBWire), generation: 2)
    #expect(afterReload?.selection == ["b.bin"],
            "同码重载（新的一代）必须重播默认面 —— 不播就是「我重载了、界面一点没变」")

    // 同一代里再进来：还是不播（否则每次重绘都会抹掉用户攒的选择）。
    let sameLoadAgain = t.seed(code: "C24-8", treeCode: "C24-8",
                               tree: try tree(treeBWire), generation: 2)
    #expect(sameLoadAgain == nil)
}

@Test func onlyANewBatchAsksTheBrowserToGoBackToTheRoot() throws {
    // ⚠️ **复审重要 2**：`FileBrowser.enter()` 里"播了默认面就回到根目录"这件事，对
    //    **换批**是对的（那是另一棵树），对**同码重载**——包括**内核崩溃后的自动恢复**——
    //    是一次**非用户动作**引发的用户可见状态改写：用户停在子目录、攒了半天的勾选会被
    //    静默替换掉，而界面上没有一句话说明。所以两件事必须**解耦**：
    //      - **重播勾选面**：照旧（内核刚重新规划过整批，`default_selected` 是**新信息**）；
    //      - **复位浏览位置**：只在**交付码真的变了**时做。
    //    判据落在这里（纯值、有单测），视图只做分派（约束 8）。
    //
    //    ⚠️ 判据必须是"**上一次播种的是不是这一批**"（`seededCode != code`），
    //       不能是"这一代有没有播过种" —— 后者对同码重载同样是 true（那正是要区分的两件事）。
    var t = ManifestTracking()

    // ① 第一次见到这一批 ⇒ 换批（这时浏览位置本来就是根，复位是空操作）。
    _ = t.display(code: "AAA-1")
    let first = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)
    #expect(first?.isANewBatch == true, "第一次见到这一批 ⇒ 换批")

    // ② **同码重载**（新的一代）：要**重播**默认面，但**不算换批** ⇒ 不许打回根目录。
    let sameCode = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeBWire), generation: 2)
    #expect(sameCode?.selection == ["b.bin"], "同码重载要重播（这一条不能回退）")
    #expect(sameCode?.isANewBatch == false, "同码重载不是换批 —— 不许把浏览位置打回根目录")

    // ③ 换一个码 ⇒ 换批 ⇒ 复位浏览位置。
    _ = t.display(code: "BBB-2")
    let other = t.seed(code: "BBB-2", treeCode: "BBB-2", tree: try tree(treeBWire), generation: 3)
    #expect(other?.isANewBatch == true, "换批要复位浏览位置（那是另一棵树）")

    // ④ `A → B(树没到) → A`：回到更早播过种的那一批，也是**换批**。
    //    （这条路上 `seededCode` 停在 "B"，所以判据与 ③ 不同源，值得单独钉。）
    _ = t.display(code: "BBB-2")
    _ = t.seed(code: "BBB-2", treeCode: "BBB-2", tree: try tree(treeBWire), generation: 4)
    _ = t.display(code: "AAA-1")
    let backToA = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 5)
    #expect(backToA?.isANewBatch == true, "回到另一批也是换批 ⇒ 复位浏览位置")
}

@Test func aNewBatchIsSeededOnlyOnceItsOwnTreeArrives() throws {
    // 树没到就**推迟**（不记账），树到位才播、且**不拿上一批的树播**。
    var t = ManifestTracking()
    _ = t.display(code: "AAA-1")
    _ = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)

    let switched = t.display(code: "BBB-2")
    #expect(switched, "换批")

    // ⚠️ 新一代（换批本来就是一次新加载）—— 这一代里 B 一次都没播过。
    let noTree = t.seed(code: "BBB-2", treeCode: nil, tree: nil, generation: 2)
    #expect(noTree == nil, "树还没到：推迟")

    let staleTree = t.seed(code: "BBB-2", treeCode: "AAA-1",
                           tree: try tree(treeWire), generation: 2)
    #expect(staleTree == nil, "还是上一批的树：更不能播（那些路径在这一批里可能指向别的文件）")

    let ownTree = t.seed(code: "BBB-2", treeCode: "BBB-2",
                         tree: try tree(treeBWire), generation: 2)
    #expect(ownTree?.selection == ["b.bin"], "这一批的树到位：播")
}

@Test func aLateDisplayInvalidationStillLetsTheNextEntryReseed() throws {
    // ⚠️ 这条守的是 README 第 11c 条那个**已知的潜在竞态**留下的自愈路：
    //    `RootView` 的 `.onChange`（换批复位，走 `display`）与 `FileBrowser` 的
    //    `.task(id:)`（播种，走 `seed`）在 SwiftUI 里**没有次序保证**。万一 `display`
    //    落在 `seed` **后面**，这一批的播种记账就被清了、而视图手上那份默认面还没落稳
    //    （复位那一半会把勾选面清空）。
    //    所以换批必须**两个记账一起**作废：只清 `seededCode` 不清 `seededGeneration`，
    //    下一次 `taskKey` 变化就会返回 nil ⇒ **补播永远不会发生** ⇒ 用户停在「已选 0 项」
    //    而界面上没有任何提示（那正是承重事项 A 的另一半）。
    var t = ManifestTracking()
    _ = t.display(code: "AAA-1")

    // 竞态里先跑的那一拍：树到了、先播了一次。
    let raced = t.seed(code: "BBB-2", treeCode: "BBB-2",
                       tree: try tree(treeBWire), generation: 2)
    #expect(raced?.selection == ["b.bin"])

    // 后跑的那一拍：换批复位（同一个码、同一代）。
    let lateReset = t.display(code: "BBB-2")
    #expect(lateReset, "换批")

    // 用户随后动一下（换目录 / 树再到位）⇒ 必须还能补播。
    let healed = t.seed(code: "BBB-2", treeCode: "BBB-2",
                        tree: try tree(treeBWire), generation: 2)
    #expect(healed?.selection == ["b.bin"],
            "换批把记账清掉之后，下一次进来必须还能补播（否则「已选 0 项」再也回不来）")
}

@Test func goingBackToThePreviousBatchSeedsItsDefaultSelectionAgain() throws {
    // ⚠️⚠️ **承重事项 A 的"静默那一半"（审查承重 ⑤）**：`A → B（树未到）→ A`。
    //
    // 现场：A 播过种（`seededCode = "A"`）→ 切到 B（**B 的树还没到**，所以 B 一个都没播）→
    //       用户在 B 的树到之前又回到 A。
    // 坏掉的样子：`seededCode` 还停在 "A" ⇒ `onNewManifest` 判成"同一批" ⇒
    //       **A 的默认选中面永远播不下来** ⇒ 用户回到 A 看到「已选 0 项」（而复位那一半
    //       刚把他的选择清空过）—— **界面上一点异常都没有**。
    // 修法：`display` 在**换批那一刻**把 `seededCode` 一起清掉。
    var t = ManifestTracking()

    _ = t.display(code: "AAA-1")
    let firstSeeding = t.seed(code: "AAA-1", treeCode: "AAA-1",
                              tree: try tree(treeWire), generation: 1)
    #expect(firstSeeding?.selection == ["a.bin"])

    let toB = t.display(code: "BBB-2")
    #expect(toB, "换批")

    // B 的树一直没到（这里根本不调 seed）—— 用户直接回到 A。
    let backToA = t.display(code: "AAA-1")
    #expect(backToA, "回到 A 也是换批（上一次显示的是 B）")

    let reseeded = t.seed(code: "AAA-1", treeCode: "AAA-1",
                          tree: try tree(treeWire), generation: 3)
    #expect(reseeded?.selection == ["a.bin"],
            "A 的默认选中面必须**再播一次** —— 不播就是「已选 0 项」而界面上没有任何提示")
}

@Test func aSameBatchLoadingBlipDoesNotReseedByItself() throws {
    // 同一批的 `.loading` 抖动（`loadState` 短暂离开 `.loaded` ⇒ 码是 nil）**本身**
    // 不得把用户攒的选择播成默认面：`display(code: nil)` 既不复位、也不推进记账。
    //
    // ⚠️ 与 `aSameCodeReloadReseedsTheDefaultFace` 的分工（别把这两条读成矛盾）：
    //    真正让"同码重载重播"的是**一次成功的加载**（`generation` 推进），
    //    不是界面上那次 nil 抖动。所以这里的 `generation` **故意不变**。
    var t = ManifestTracking()
    _ = t.display(code: "AAA-1")
    _ = t.seed(code: "AAA-1", treeCode: "AAA-1", tree: try tree(treeWire), generation: 1)

    let duringLoading = t.display(code: nil)
    #expect(duringLoading == false, "加载中：不复位")

    let sameBatch = t.display(code: "AAA-1")
    #expect(sameBatch == false, "同一批：不复位")

    let reseeded = t.seed(code: "AAA-1", treeCode: "AAA-1",
                          tree: try tree(treeWire), generation: 1)
    #expect(reseeded == nil, "没有新的一次加载就不重播：用户在这批里攒的选择不能被抹掉")
}

@Test func theTaskKeyChangesWhenThisBatchesTreeArrives() {
    // ⚠️ 承重事项 B：`AppModel` 先落 `loadState = .loaded`、**之后**才拉 `get_tree`
    //    （`performLoadDelivery` → `surfacing { getTree() }`），而视图是在 `.loaded` 那一刻
    //    出现的 —— 所以第一次进来时 `treeCode == nil`，`onNewManifest` 会**推迟**播种。
    //    **只有 key 在这之后变了**，`.task` 才会再进一次、才播得下去。
    //    删掉 key 里那一段 ⇒ "推迟播种"变成"永不播种"（默认选中面永远播不下来，
    //    而界面上一点异常都看不出来）。
    let waiting = BrowserSelection.taskKey(code: "AAA-1", path: "", treeCode: nil, generation: 1)
    let arrived = BrowserSelection.taskKey(code: "AAA-1", path: "", treeCode: "AAA-1", generation: 1)

    #expect(waiting != arrived, "这一批的树到位必须让 key 变 —— re-seed 只有这一个触发点")
}

@Test func aSameCodeReloadChangesTheTaskKey() {
    // ⚠️ 阶段 D 任务 B：**同码重载必须让 `.task` 重进一次** —— 否则"重播默认面"根本没有入口
    //    （`FileBrowser.enter()` 是那个入口，而它只在 `taskKey` 变化时才跑）。
    //    同码重载时 `code` / `path` / `treeCode` 三个分量**一个字都没变**，
    //    所以新一代必须自己进 key。
    let base = BrowserSelection.taskKey(code: "AAA-1", path: "", treeCode: "AAA-1", generation: 7)

    #expect(BrowserSelection.taskKey(code: "AAA-1", path: "", treeCode: "AAA-1", generation: 8) != base,
            "同码重载（新的一代）要重进")
    #expect(BrowserSelection.taskKey(code: "AAA-1", path: "", treeCode: "AAA-1", generation: 7) == base,
            "同一代内不变（`.task(id:)` 不能每次重绘都重进）")
}

@Test func everySegmentOfTheTaskKeyIsLoadBearing() {
    let code = "AAA-1", path = "a/b", tree = "BBB-2"
    let base = BrowserSelection.taskKey(code: code, path: path, treeCode: tree, generation: 5)

    #expect(BrowserSelection.taskKey(code: code, path: path, treeCode: tree, generation: 5) == base,
            "同输入同 key（`.task(id:)` 不能每次重绘都重进）")
    #expect(BrowserSelection.taskKey(code: "CCC-3", path: path, treeCode: tree,
                                     generation: 5) != base, "换批要重进")
    #expect(BrowserSelection.taskKey(code: code, path: "a/c", treeCode: tree,
                                     generation: 5) != base, "换层要重进")
    #expect(BrowserSelection.taskKey(code: code, path: path, treeCode: nil,
                                     generation: 5) != base, "树到位要重进")
    #expect(BrowserSelection.taskKey(code: code, path: path, treeCode: tree,
                                     generation: 6) != base, "新的一次加载要重进")
}

@Test func theTaskKeyIsUnambiguousAcrossSegmentBoundaries() {
    // ⚠️ 几段**不能裸拼**（`"\(code)|\(path)|\(treeCode)"` 那种）：路径是清单原文，
    //    里面可以有 `|`（约束 3：不规范化、不转义），裸拼会让下面两对输入撞成同一个键 ——
    //    撞了就是"该重进的时候没重进"，也就是这条防线要防的那种静默失效。
    #expect(BrowserSelection.taskKey(code: "A|B", path: "", treeCode: nil, generation: 1)
            != BrowserSelection.taskKey(code: "A", path: "B|", treeCode: nil, generation: 1))
    #expect(BrowserSelection.taskKey(code: "A", path: "", treeCode: "B|", generation: 1)
            != BrowserSelection.taskKey(code: "A", path: "B|", treeCode: "", generation: 1))
    // 代数那一段是**纯整数、且排在最后**：它前面那段（`treeCode`）是长度前缀的、自定界的，
    // 所以追加一个 `|<数字>` 不会与树码里的 `|` 混淆（裸拼的话下面这两条就会撞）。
    #expect(BrowserSelection.taskKey(code: "A", path: "", treeCode: "B|1", generation: 0)
            != BrowserSelection.taskKey(code: "A", path: "", treeCode: "B", generation: 1))
}

// ---------------------------------------------------------------------------
// 时间列（阶段 C / 任务 4c）：「源文件的修改时间」
//
// ⚠️ 夹具**不动**上面那两个。`listDirWire` 里**没有** `source_mtime` 这个键，
//    它本身就是"老清单"那一半的现场（约束 C-2）——"缺键"只有靠它才验得到，
//    而"缺键"与"空串"是**两种不同的输入**，两种都要落到 `—`。
// ---------------------------------------------------------------------------

/// 新交付的线上原文：三个文件各带一种 `source_mtime`，外加一个目录。
/// 键是 `source_mtime`（snake_case），由 `CoreJSON.decoder` 的 `.convertFromSnakeCase`
/// 转到 `sourceMtime` —— 这条对照关系本身就是断言的一部分（写错一个字母这里就先红了）。
private let listDirWireWithTimes = #"""
{"path":"t",
 "entries":[{"type":"file","name":"has-time.txt",
             "path":"t/has-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":"2026-09-14T12:00:00+08:00"},
            {"type":"file","name":"empty-time.txt",
             "path":"t/empty-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":""},
            {"type":"file","name":"junk-time.txt",
             "path":"t/junk-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":"待定"},
            {"type":"dir","name":"d","children_count":2}]}
"""#

/// 老清单的**整棵树**（`get_tree` 的 result）：文件叶节点里**没有** `source_mtime` 键
/// （形状与 `listDirWire` 同一个来源：`view::Node` 的两条 JSON 出口）。
private let oldTreeWire = #"""
{"tree":{"type":"dir","name":"","children":{
    "t":{"type":"dir","name":"t","children":{
        "a.txt":{"type":"file","name":"a.txt","path":"t/a.txt","crc64":"",
                 "size":1,"completed":0,"total":1,"speed":0,"state":"pending","err":""}}}}},
 "flat":[{"path":"t/a.txt","name":"a.txt","size":1,"state":"pending"}],
 "default_selected":["t/a.txt"],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}
"""#

@Test func anOldPayloadWithoutTheKeyStillDecodesAndReadsNil() throws {
    // 约束 C-2：已交付的老清单里**没有** `source_mtime` 这个键。
    // ⚠️ 这条断言的前半句是"它解得动"：非可选 `String` 会让**整条载荷解码失败**
    //    （`BrowserRow.swift` 里记着的那次教训），而那时是抛在这里、后面的断言
    //    一行都到不了 —— 所以它在，且必须先于一切。
    let old = try fileNodes(listDirWire)

    #expect(old.count == 2, "夹具本身：这一层两个文件")
    #expect(old.allSatisfy { $0.sourceMtime == nil }, "缺键 → nil（不是空串）")
}

@Test func anOldTreePayloadWithoutTheKeyStillLoads() throws {
    // 约束 C-2 的**另一条入口**：`load_delivery` / `get_tree` 的整棵树里同样是 `FileNode`。
    // 这里坏掉的后果比文件页那一列重得多 —— 不是"时间列空着"，而是**整批加载失败**
    // （用户看到的是空态页 + 内核原文），所以老清单在这条路上也必须解得动。
    // 前半句同样是"它解得动"：非可选的 `String` 会让 `try tree(…)` 直接抛在这里。
    //
    // ⚠️ 本仓库的 e2e 覆盖不到这一条：它的清单走 `delivery_manifest.build_manifest`，
    //    而那个函数**始终**写这个键（3 元组条目补空串）⇒ 线上是"键在、值为空串"。
    //    「键真的不在」只有这段载荷能给。
    let t = try tree(oldTreeWire)

    #expect(t.flat.map(\.path) == ["t/a.txt"], "夹具本身")
    #expect(t.defaultSelected == ["t/a.txt"], "夹具本身")
    guard case .dir(_, let level1) = t.tree,
          case .some(.dir(_, let level2)) = level1["t"],
          case .some(.file(let leaf)) = level2["a.txt"] else {
        Issue.record("夹具不是「根 → t → a.txt」那棵树")
        return
    }
    #expect(leaf.sourceMtime == nil, "缺键 → nil（不是空串）")
}

@Test func aPayloadWithTheKeyCarriesItVerbatim() throws {
    // 内核当**不透明字符串**搬运，壳也不加工（约束 3）：不解析、不换时区、不校验形状。
    // 三种输入逐字带过来 —— 怎么显示是 `SourceTimeText` 那一层的事。
    #expect(try sourceMtime(listDirWireWithTimes, "has-time.txt") == "2026-09-14T12:00:00+08:00")
    #expect(try sourceMtime(listDirWireWithTimes, "empty-time.txt") == "", "空串与缺键是两种输入")
    #expect(try sourceMtime(listDirWireWithTimes, "junk-time.txt") == "待定")
}

@Test func theTimeColumnShowsADashWhenThereIsNoValue() {
    // ⚠️ 缺值与空串都出 `—`（那是"这一格没有值"，不是"时间是空的"）。
    //    变异体：`raw ?? ""`（老清单那一半）或直接透传空串 ⇒ 界面上什么都没有，
    //    而约束 C-2 明文要求显示 `—`、**不得**显示空白。
    #expect(SourceTimeText.of(nil) == "—")
    #expect(SourceTimeText.of("") == "—")
}

@Test func theTimeColumnReusesTheSharedTimestampPresentation() {
    // 与侧边栏的 `created_at` / `expires_at` **同一个**格式化器（同形的输入、同一口径）：
    // 壳不另造一套（约束 C-7）。
    #expect(SourceTimeText.of("2026-09-14T12:00:00+08:00") == "2026-09-14 12:00")
    #expect(SourceTimeText.of("2026-09-14T12:00:00+08:00")
            == TimestampPresentation.text("2026-09-14T12:00:00+08:00"),
            "两条必须是同一个实现（这条等值比较钉住「没另造一套」）")
}

@Test func theTimeColumnNeverInventsWordingOfItsOwn() {
    // 约束 C-7：内核给的原文由壳**原样呈现**。认不出来的形状就照抄原文 ——
    // **不得**出现 `Invalid Date` 这种壳自己编的文案（`TimestampPresentation`
    // 的既有契约就是"任何一条不满足就原样返回"，这里沿用它、不另造一套）。
    // 下面五个都是**形状**不合法（不是日历值离谱）：壳只按形状判，不校验日历。
    for raw in ["待定", "2026-09-14", "2026-09-14T12:00",
                "2026-10-14T16:13:34+0800", "2026-10-14T16:13:34+08:00 尾巴"] {
        #expect(SourceTimeText.of(raw) == raw, "\(raw) 必须原样返回")
    }
}

@Test func fileRowsCarryTheTimeTextAndDirRowsNeverDo() throws {
    func text(_ name: String) throws -> String {
        try row(in: listDirWireWithTimes, parent: "t", named: name).sourceTimeText
    }

    #expect(try text("has-time.txt") == "2026-09-14 12:00")
    #expect(try text("empty-time.txt") == "—")
    #expect(try text("junk-time.txt") == "待定", "认不出来就原样显示，不是 `—`、更不是 `Invalid Date`")
    // 目录**没有**"源文件时间"这回事（与它的大小同理，`BrowserRow.size` 是 nil）：
    // 恒 `—`，不因为它有几个子项而变。
    #expect(try text("d") == "—")
}

@Test func oldPayloadRowsShowADashNotABlank() throws {
    // 约束 C-2 的界面那一半：老清单的行**必须显示 `—`，不得显示空白**。
    // 变异体：`f.sourceMtime ?? ""` ⇒ 这一格什么都没有，用户看到的是"界面坏了"。
    #expect(try row("QC 图.png").sourceTimeText == "—")
    #expect(try row("reads.fq.gz").sourceTimeText == "—")
}
