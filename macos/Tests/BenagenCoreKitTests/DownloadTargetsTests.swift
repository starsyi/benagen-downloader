import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「下载」这个动作的呈现模型（全局约束 8：`Presentation/` 里每条纯计算都要有单测）
//
// ⚠️ 夹具的**形状**必须与内核真会发的对得上（键是 snake_case，经 `CoreJSON.decoder` 解成
//    `EnqueueResult`，走与真内核同一条解码口径）。**`enqueue` 成功路径上真正会发的回执有两种**：
//      ① **部分成功**：`added` 非空、`rejected` 可空 —— `core/src/main.rs:1413` 那条 `Ok(...)`；
//      ② **一件都没加（且没有任何拒绝）**：`added` 与 `rejected` **都空**，`paths` 为空
//         （界面「全部下载」）而内核算出的待下载集合为空 —— **阶段 D 的 D-2**：
//         这是"没有待下载的"这条**正常**情形的回执，不是错误，也不是"死形状"。
//    （改之前内核在这条路上回的是 `Err(invalid_params("没有匹配到任何文件"))` ——
//      `resolve_targets` 的 `out.is_empty()` 早退，`core/src/main.rs:1260-1261` ——
//      于是"合法为空"被当成错误，那正是本阶段修的那个 bug。
//      那条"`Ok` 路径上 `added` 必然非空"的推导描述的是**改之前**的内核，**不要再拿它当
//      "这种回执不可能出现"的证据**；它现在只剩一个用途：解释为什么 D-2 必须存在。）
//    另外两条形状仍然走**错误通道**、不是回执（所以这里没有它们的夹具）：
//      - "一个都没加进去 + `rejected` 非空"⇒ `Err(invalid_params("没有任何文件被加入下载：{rejected:?}"))`
//        （`:1335-1339`），壳整条原文照登，走 `EnqueueFeedback.failure(of:)`，
//        由 `anAllRejectedEnqueueArrivesAsAnErrorAndIsShownWhole` 钉着；
//      - `ENGINE_DISCONNECTED` 早退（`:1387-1407` 那个 `for` 循环里唯一的第三个出口，
//        判据在 `:1401-1403`）。
//    所以：**没有**"全拒"形状的夹具；`nothingAtAllWire` 那条**是①之外的那条真回执**（见它自己的注释）。
//
// ⚠️ **夹具的条数是故意不对称的**（`added` 3 条、`rejected` 2 条）。
//    两边都写 1 条的话，"摘要里数错了哪一边"这件事在断言上**不可观测**
//    （`added.count` 与 `rejected.count` 对调也能过）—— 任务 6 那条活下来的变异体
//    就是这么被夹具掩护掉的（夹具里两组恰好都是降序）。
//
// ⚠️ 两条拒绝理由都是**内核里真实存在的那两句**（`core/src/main.rs:1390` 与 `:1255-1257` 的
//    `format!`），路径含 `×` 与空格 —— 不是自己编的措辞。
// ---------------------------------------------------------------------------

/// `enqueue` 的线上原文（`core/src/protocol.rs` 的 `EnqueueResult`）：**3 个加进去、2 个被拒**。
private let partialWire = #"""
{"added":[{"gid":"g1","path":"a.bin"},{"gid":"g2","path":"b.bin"},{"gid":"g3","path":"c.bin"}],
 "rejected":[{"path":"z.bin","reason":"路径不安全（越界/控制字符/空段）"},
             {"path":"C24-8_×_25WS024/QC 图.png","reason":"清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"}]}
"""#

/// ⚠️ **这条曾经被写成"内核的死形状"——那句话现在是错的，别再写回去**（阶段 D 的 D-2）：
/// 它是内核在「`paths` 为空（界面「全部下载」）且算出的待下载集合为空」时回的**正常回执**
/// —— **成功、`added` 空数组**，不再是 `invalid_params("没有匹配到任何文件")`。
/// 旧推导（`resolve_targets` 的 `out.is_empty()` 早退 + 逐条分桶 ⇒ `Ok` 路径上 `added` 非空）
/// 描述的是**改之前**的内核；那两句仍然解释了"为什么"这条回执现在必须存在：
/// 合法为空**不是**错误，把它当错误回给壳就是本次要修的 bug。
private let nothingAtAllWire = #"{"added":[],"rejected":[]}"#

/// **全拒**时内核回的是错误（不是回执）——原文由 `core/src/main.rs:1409-1411` 的
/// `format!("没有任何文件被加入下载：{rejected:?}")` 拼出来（`{rejected:?}` 是 Rust 的
/// `Debug`，所以那一段是内核自己拼的串，**不是结构化字段**）。
private let allRejectedError = #"没有任何文件被加入下载：[Object {"path": "z.bin", "reason": "路径不安全（越界/控制字符/空段）"}]"#

private func enqueued(_ json: String) throws -> EnqueueResult {
    try CoreJSON.decoder.decode(EnqueueResult.self, from: Data(json.utf8))
}

// ---------------------------------------------------------------------------
// `paths(for:)`：勾选面 → `enqueue` 的 paths
// ---------------------------------------------------------------------------

// ⚠️ 下面四条都传了 `allPaths`：新签名**没有旧签名的重载**（裁定 C2）——
//    一个"传没传整批都能编过"的重载，正好能让"整批全选必须发 `[]`"被悄悄绕过。
//    这几条测的是**显式列表**那一支，所以 `allPaths` 挑的都是**与勾选面不相等**的
//    一批文件（多数还含一个勾选面里没有的路径），把"整批"那一支排除在外。

@Test func aDirectoryPathIsSentAsIs() {
    // ⚠️ 目录路径**原样**交给内核 —— 内核的 `resolve_targets` 把它当**前缀**展开
    //    （`f.path == p || f.path.starts_with("{p}/")`）。壳**不自己展开目录**（约束 1）。
    //    这一批的文件在目录**里面**（`flat` 只列文件，目录不在里面）⇒ 两边不相等 ⇒ 显式列表。
    #expect(DownloadTargets.paths(for: ["a/b"], allPaths: ["a/b/one.bin"]) == ["a/b"])
}

@Test func selectionIsSortedForDeterminism() {
    // 勾选面是 `Set`，遍历顺序不稳定：不排序 ⇒ 每次请求的 `paths` 顺序都不同，
    // 排障时没法比对两次请求（简报点名）。
    #expect(DownloadTargets.paths(for: ["c.bin", "a/b"], allPaths: ["c.bin", "a/b/one.bin"])
            == ["a/b", "c.bin"])
}

@Test func noSelectionMeansDownloadEverythingPending() {
    // 内核语义：`paths` 为空 = 下全部待下载。**空集合 → 空数组**（不是"什么都不发"）。
    // ⚠️ 这一批是有文件的（`allPaths` 非空）：否则"空勾选 → 空数组"会被误读成
    //    "整批全选"那一支的功劳 —— 两者在这里返回值相同，但走的是不同的分支。
    #expect(DownloadTargets.paths(for: [], allPaths: ["a.bin"]).isEmpty)
}

@Test func pathsAreNotTrimmedOrNormalized() {
    // 约束 3：落盘/下发路径 = 清单原文。壳不 trim 空白、不折叠 `//`、不解析 `.`
    // （`standardizingPath` 会把这三样全改掉），也不动非 ASCII。
    //
    // ⚠️ 夹具是**故意挑的六项**（不是随手四五个）：
    //    - 期望顺序既不是输入顺序、也不是它的倒序；
    //    - 大小写、空格、`.`、`//`、`×` 都塞进去了，"顺手规范化"必红；
    //    - 项数越多，"不排序（Set 原序）"这个变异体**恰好撞上正确顺序**的概率越低
    //      （n! 分之一）—— 六项是 1/720，而两项是 1/2。**Set 的遍历顺序与进程随机种子有关**，
    //      项数太少的话那个变异体会时红时绿（实测：两项那条测试在一次复跑里放过了它）。
    let selection: Set<String> = ["b.bin", " C24-8_×_25WS024/Figure", "a/./c.bin",
                                 "a//d", "a/b.bin", "Z.bin"]
    // 这一批的文件是勾选面里除目录（` C24-8_×_25WS024/Figure`）以外的那五项 ——
    // 两边不相等 ⇒ 显式列表（不是"整批全选"那一支）。
    let files: Set<String> = ["b.bin", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"]
    #expect(DownloadTargets.paths(for: selection, allPaths: files)
            == [" C24-8_×_25WS024/Figure", "Z.bin", "a/./c.bin", "a//d", "a/b.bin", "b.bin"])
}

// ---------------------------------------------------------------------------
// 整批全选与体量预算（全局约束 C-3：超限的请求**不报错**，只把客户端挂死）
// ---------------------------------------------------------------------------

@Test("勾选面覆盖整批 ⇒ 发空数组（内核语义＝全部待下载，请求体恒定）")
func selectingEverythingSendsAnEmptyList() {
    // ⚠️ 这不是优化，是**正确性**：`enqueue.paths` 的请求体随勾选面线性增长，
    //    而内核的行长上限是 8 MiB（`core/src/main.rs:113`）。超长行**不报错** ——
    //    内核只回一条 `id == 0` 的协议告警、那条请求永远等不到响应、
    //    `CoreClient` 的串行队列被**永久堵死**，而界面上一个字都不说。
    //    发 `[]` 是**语义等价**的（内核的 `paths` 为空 = 全部**待下载**）。
    let all: Set<String> = ["a", "b", "c"]
    #expect(DownloadTargets.paths(for: all, allPaths: all) == [])
}

@Test("差一项 ⇒ 发显式列表（不能被当成整批）")
func missingOneSendsTheExplicitList() {
    // 判据是**相等**，不是"差不多"：少勾一项就必须走保守的那一边（显式列表）。
    let all: Set<String> = ["a", "b", "c"]
    #expect(DownloadTargets.paths(for: ["a", "b"], allPaths: all) == ["a", "b"])
}

@Test("一批是空的时，空勾选仍然是空数组（不能反过来被当成整批）")
func anEmptyBatchIsNotEverything() {
    // `allPaths` 为空时**不算**整批 —— 否则"一批里一个文件都没有"会被读成"全选"。
    // 两条分支在这里的**返回值恰好相同**（都是 `[]`），所以这条测试钉的是**不崩、不错**，
    // 判别力靠下面那条"一批为空但勾了东西"的对照来补。
    #expect(DownloadTargets.paths(for: [], allPaths: []) == [])
    // 对照：同一批为空、勾选面非空 ⇒ 绝不能因为"整批是空的"就把勾选面丢掉。
    #expect(DownloadTargets.paths(for: ["x.bin"], allPaths: []) == ["x.bin"])
}

@Test("请求体量的预算：明显超限的路径集合必须被判为超预算")
func anOversizedRequestIsDetected() {
    // 20 万条、每条约 50 字节 ⇒ 约 10 MB，远超 4 MiB 的预算。
    let many = (0..<200_000).map { "dir\($0)/file-with-a-fairly-long-name-\($0).bin" }
    #expect(DownloadTargets.exceedsRequestBudget(many) == true)
}

@Test("正常规模不误伤")
func aNormalRequestIsWithinBudget() {
    #expect(DownloadTargets.exceedsRequestBudget(["a.txt", "dir/b.bin"]) == false)
}

@Test("预算是内核上限的一半（留一半给信封：id / method 与任何我们没算进去的部分）")
func theBudgetKeepsHalfTheKernelLimit() {
    // 内核的行长上限是 8 MiB（`core/src/main.rs:113`）。守卫**宁可早一点拒绝**，
    // 也不能放过一条会永久堵死客户端的请求 —— 所以取一半。
    // ⚠️ 钉的是**简报指定的那个精确值**（"安全预算"是一个被决定过的数，
    //    不是可以从别处推出来的；改了它就得有人重新论证一次）。
    #expect(DownloadTargets.requestBudgetBytes == 4 << 20)
}

@Test("预算的边界：恰好等于预算不算超，多一个字节才算")
func theBudgetBoundaryIsInclusive() {
    // ⚠️ 这条补的是上面两条测不到的东西：`anOversizedRequestIsDetected`（约 10 MB）与
    //    `aNormalRequestIsWithinBudget`（20 字节）只把预算夹在一个**很宽**的区间里 ——
    //    把 `requestBudgetBytes` 从 4 MiB 改成 1 MiB，那两条**照样全绿**。
    //    边界这条把判据钉死在指定的那个数上，并钉住"`>` 而不是 `>=`"
    //    （差一个字节就拒绝，等于把上界悄悄挪小）。
    //
    // 单路径的编码是 `{"paths":["…"]}` ⇒ 字节数 = 信封 + 路径长度。用被测的
    // `requestBytes` 自己量出信封（不硬编码 14 —— 那是把实现抄进测试）。
    let envelope = DownloadTargets.requestBytes([""])
    let exact = DownloadTargets.requestBudgetBytes - envelope
    #expect(exact > 0, "信封本身不该就超过预算（否则这个预算毫无意义）")

    let onTheLine = [String(repeating: "x", count: exact)]
    #expect(DownloadTargets.requestBytes(onTheLine) == DownloadTargets.requestBudgetBytes,
            "夹具没搭准：这一条应当**恰好**用满预算")
    #expect(DownloadTargets.exceedsRequestBudget(onTheLine) == false, "恰好等于预算不算超（`>`）")

    let oneByteOver = [String(repeating: "x", count: exact + 1)]
    #expect(DownloadTargets.exceedsRequestBudget(oneByteOver) == true, "多一个字节就该拒绝")
}

@Test func theEmptySelectionHintSaysWhatWillActuallyHappen() {
    // 一项都没勾时发的是 `paths: []` = 全部待下载 —— 底栏那句话必须**明说**这件事
    // （"按下去会下全部"，不是"按下去什么都没发生"）。空白文案 = 静默（约束 4）。
    #expect(DownloadTargets.emptySelectionHint == "未勾选任何项，将下载全部待下载文件")
}

@Test func theButtonSaysDownloadAllWhenNothingIsSelected() {
    // 简报：未选中任何项时按钮文案变成「全部下载」，发出去的是 `paths: []`。
    // 所以这颗按钮**不是禁用**，是换一句话说（"按下去什么都不会发生"才该禁用）。
    #expect(DownloadTargets.buttonTitle(for: []) == "全部下载")
    #expect(DownloadTargets.buttonTitle(for: ["a.bin"]) == "下载选中")
    #expect(DownloadTargets.buttonTitle(for: ["a.bin", "b/c"]) == "下载选中",
            "选中几项都是同一句话：文案只说「有 / 没有勾选」这一件事")
    // 两个文案必须**不同** —— 相同的话，"有没有勾选"在界面上就看不出来，
    // 而这两个状态发出去的请求是两件完全不同的事（`[路径]` vs 全部待下载）。
    #expect(DownloadTargets.buttonTitle(for: []) != DownloadTargets.buttonTitle(for: ["a.bin"]))
    #expect(!DownloadTargets.helpText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            "悬停提示不得空白（约束 4），且工具栏与底部状态栏共用这一份")
}

// ---------------------------------------------------------------------------
// `EnqueueFeedback`：`added` 与 `rejected` 两边都要有落点（约束 4）
// ---------------------------------------------------------------------------

@Test func addedTasksSwitchToTheTransfersSection() throws {
    let f = EnqueueFeedback.of(try enqueued(partialWire))

    #expect(f.switchesToTransfers, "`added` 非空 → 切到传输列表")
    #expect(f.summary == "已加入 3 个下载任务",
            "数的是 `added` 的条数（夹具 added=3 / rejected=2，两边不同才看得见数错哪一边）")
    #expect(f.rejections.count == 2, "切视图不等于可以少交：`rejected` 也要逐条交出来")
}

@Test func rejectionsAreListedOneByOneVerbatim() throws {
    // 约束 4：不得静默少交 —— 每一条 `path` 与 `reason` 都要能摆到界面上。
    // ⚠️ 挂的是**真实形状**：部分成功（`added` 非空 + `rejected` 非空）。
    //    全拒那条路上内核回的是**错误**、根本没有回执 —— 见
    //    `anAllRejectedEnqueueArrivesAsAnErrorAndIsShownWhole`。
    let f = EnqueueFeedback.of(try enqueued(partialWire))

    // **逐条**、**原文**（约束 3：不转义、不规范化），顺序按内核给的；
    // 两条理由都是内核里真实存在的那两句，其中一条带 `×`/空格、一条带内核自己 Debug 引号。
    #expect(f.rejections.map(\.path) == ["z.bin", "C24-8_×_25WS024/QC 图.png"])
    #expect(f.rejections == [
        EnqueueFeedback.Rejection(path: "z.bin", reason: "路径不安全（越界/控制字符/空段）"),
        EnqueueFeedback.Rejection(path: "C24-8_×_25WS024/QC 图.png",
                                  reason: "清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"),
    ])
}

@Test func anEmptyAddedReceiptSaysThereIsNothingToDownload() throws {
    // ⚠️ **阶段 D 的 D-2 改写了这条回执的地位**：`{"added":[],"rejected":[]}` 从
    //    "内核的死形状"变成了**内核真会发的一条正常回执** ——
    //    `paths` 为空（界面「全部下载」）且算出的 pending 为空时，内核回**成功**、
    //    `added` 空数组，而**不再**回 `invalid_params("没有匹配到任何文件")`。
    //    （旧的那句推导留在文件头与 `nothingAtAllWire` 的注释里，那是**历史**，
    //     不要再拿它当"这种回执不可能出现"的证据。）
    //
    // ⚠️ **这句话是壳自己写的**（约束 C-7/C-10 要求写明理由）：这条路上**内核一个字都没说**
    //    —— 它不是错误，没有 `message` 可登（约束 3 的例外），而"点了下载、界面什么都不说"
    //    正是约束 4 明禁的静默失效。所以壳把**事实**说出来：
    //    内核算出来的待下载集合是空的，即**没有需要下载的文件**。
    //    ⚠️ 措辞只陈述事实：**不加判断**（不说"可能已经全部下载完成"——那是壳替内核下结论，
    //       而壳手里没有"为什么是空的"这份知识）、**不加建议**（不说"请稍后再试"）。
    let nothing = EnqueueFeedback.of(try enqueued(nothingAtAllWire))

    #expect(!nothing.switchesToTransfers,
            "没有任务在传输列表里 —— 切过去只会看到一片空，还会把这句话吞掉")
    #expect(nothing.summary == "没有需要下载的文件")
    #expect(nothing.rejections.isEmpty)
}

@Test func anAllRejectedEnqueueArrivesAsAnErrorAndIsShownWhole() {
    // ⚠️ **全拒场景的处置**（有意偏离，约束 11）：内核在"一个都没加进去 + `rejected` 非空"时
    //    **不回回执**，而是回 `Err(invalid_params("没有任何文件被加入下载：{rejected:?}"))`
    //    （`core/src/main.rs:1409-1411`）。于是它走**抛错路径**、由这条工厂接手：
    //    内核原文**整条照登**，壳**不解析**那句 Rust `Debug` 串去做逐条渲染。
    //    理由：`{rejected:?}` 不是结构化字段，切它等于把壳焊在内核的一句措辞上（约束 2/3），
    //    而"逐条"那条路只在**部分成功**这条真实形状上做（约束 10：不为这个改 core）。
    //    约束 4 仍然满足：用户看得到内核说的**全部**信息（每条 path 与 reason 都在那串里）。
    // 变异体：哪天有人"顺手"去解析这句串、把 rejections 填出来，这条断言就会红。
    let f = EnqueueFeedback.failure(of: CoreError.rpc(code: "invalid_params",
                                                     message: allRejectedError))

    #expect(f.summary == allRejectedError, "内核原文逐字（约束 3），不做任何加工")
    #expect(f.rejections.isEmpty, "壳不解析内核拼的 Debug 串 —— 逐条渲染只在部分成功那条路上做")
    #expect(!f.switchesToTransfers, "失败要**原地**显示")
}

@Test func failuresShowTheKernelTextAndDoNotSwitchViews() {
    // 简报：`engine_start_failed` / `preflight_failed` → **原地显示内核原文**，不切视图。
    // 走**真错误**（而不是直接塞一个字符串）：这条链是 `CoreError` → `AppModel.message(of:)`
    // → 界面上那句话，中间任何一段换成"壳自己编的措辞"都会被下面的逐字断言判红。
    let f = EnqueueFeedback.failure(of: CoreError.rpc(code: "engine_start_failed",
                                                      message: "启动下载引擎失败：端口 6800 被占用"))

    #expect(f.summary == "启动下载引擎失败：端口 6800 被占用", "内核原文逐字（约束 3）")
    #expect(!f.switchesToTransfers, "失败要**原地**显示 —— 切走就等于把这句话吞掉")
    #expect(f.rejections.isEmpty, "失败没有「哪几个文件」这回事")
}

// ---------------------------------------------------------------------------
// `DownloadNotice`：回执 + 它属于哪一批（审查次要 ④）
// ---------------------------------------------------------------------------

@Test func aNoticeKnowsWhichBatchItBelongsTo() throws {
    let n = DownloadNotice.of(try enqueued(partialWire), code: "AAA-1")

    #expect(n.belongsTo(to: "AAA-1"))
    #expect(n.summary(currentCode: "AAA-1") == "已加入 3 个下载任务",
            "属于当前批：不加前缀（否则每一行都挂一个批次码，噪声）")
    #expect(!n.belongsTo(to: "BBB-2"))
}

@Test func aStaleNoticeSaysWhichBatchItCameFrom() throws {
    // ⚠️ 换批**不**把这条清掉（约束 4：拒绝理由不能因为用户换了批次就消失），
    //    代价是它可能属于上一批 —— 所以**不属于当前批时前面必须带批次码**，
    //    否则"已加入 3 个下载任务"会被读成**新批次**的结果（审查次要 ④）。
    let n = DownloadNotice.of(try enqueued(partialWire), code: "AAA-1")

    #expect(n.summary(currentCode: "BBB-2") == "批次 AAA-1：已加入 3 个下载任务")
    #expect(n.feedback.rejections.count == 2, "换批之后拒绝理由仍然在手（它们还能被摆出来）")
}

@Test func aFailedNoticeCarriesItsBatchToo() throws {
    let f = DownloadNotice.failure(of: CoreError.rpc(code: "preflight_failed",
                                                     message: "磁盘空间不足：需要 10 GB，可用 2 GB"),
                                   code: "AAA-1")

    #expect(f.summary(currentCode: "AAA-1") == "磁盘空间不足：需要 10 GB，可用 2 GB")
    #expect(f.summary(currentCode: "BBB-2") == "批次 AAA-1：磁盘空间不足：需要 10 GB，可用 2 GB")
    #expect(!f.feedback.switchesToTransfers)

    // 没有批次码（理论上到不了：没有生效批次时那颗按钮是禁用的）时不该凭空冒出一个批次字样。
    let noCode = DownloadNotice.of(try enqueued(nothingAtAllWire), code: nil)
    #expect(noCode.belongsTo(to: nil), "nil == nil 算属于自己，别在界面上凭空写「批次 —」")
    #expect(noCode.summary(currentCode: nil) == "没有需要下载的文件")
    #expect(noCode.summary(currentCode: "AAA-1") == "批次 —：没有需要下载的文件",
            "真有码可比且对不上时，宁可写占位也不冒充新批次")
}
