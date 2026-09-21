import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 校验结果的呈现模型（任务 9）
//
// 夹具走**解码**这条路（`VerifyStatus.fixture` / `TreeResult.fixture` 在文件末尾）：
// 键是内核线上的 snake_case（`size_mismatch` 带下划线），手搓一个结构体会让
// "壳解不解得动内核发来的那一行"这件事在测试里凭空消失 —— 而那正是本任务要处理的形状。
//
// ⚠️ 本文件背着三条硬约束：
//   - 约束 4：六类互斥穷尽，**计数为 0 的类也要渲染**（显示「0 项」，不得隐藏）；
//   - 约束 3：路径**原文照登**（不取最后一段、不规范化、不转义）；
//   - 以及"壳不得比内核更严"：`all_good` 不含 `unverifiable`，壳不许把它算进去。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 六类：齐备、互斥、可判读
// ---------------------------------------------------------------------------

@Test func allSixClassesAreAlwaysPresentEvenWhenEmpty() {
    // 约束 4：六类互斥穷尽，计数为 0 的类显示 0，不得隐藏
    let s = VerifySummary.of(.fixture(ok: ["a"], bad: [], missing: [],
                                      sizeMismatch: [], unverifiable: [], unreadable: []))!
    #expect(s.rows.count == 6)
    #expect(s.rows.map(\.count) == [1, 0, 0, 0, 0, 0])
    // 「0 项」必须是一句**看得见的话**：靠"路径数组为空所以画不出东西"来表达 0，
    // 在界面上就是"这一类不见了"（约束 4 的静默失效）。顺序也一并钉住。
    #expect(s.rows.map(\.countText) == ["1 项", "0 项", "0 项", "0 项", "0 项", "0 项"])
    #expect(s.rows.map(\.kind) == VerifyClass.allCases)
}

@Test func everyClassHasADistinctLabel() {
    // ⚠️ **只断言"两两不同"是不够的**（同 `TransferRowTests.everyTaskStateMapsToItsOwnColor`
    //    那段）：把 `.missing`（"文件缺失"）与 `.sizeMismatch`（"大小不符"）对调、
    //    或把 `.ok` 与 `.bad` 的图标对调，`Set(...).count == 6` 照样成立 ——
    //    客户看到的就是"两类说法反了"。所以这里**逐类钉死**哪一类是哪个词、哪颗图标
    //    （措辞的来源见 `VerifyClass.label` 上面那段注释）。
    let expected: [VerifyClass: (label: String, icon: String)] = [
        .ok: ("校验通过", "checkmark.circle.fill"),
        .bad: ("内容不符", "xmark.circle.fill"),
        .missing: ("文件缺失", "questionmark.folder"),
        .sizeMismatch: ("大小不符", "ruler"),
        .unverifiable: ("无法校验", "questionmark.circle"),
        .unreadable: ("无法读取", "lock.slash"),
    ]
    let s = VerifySummary.of(.fixture())!
    #expect(expected.count == VerifyClass.allCases.count,
            "六类必须一类不漏地钉在这里 —— 漏掉的那类就没人守了")
    for kind in VerifyClass.allCases {
        guard let want = expected[kind] else {
            Issue.record("\(kind.rawValue) 没有期望值：新加一类就要在上面的表里补一行")
            continue
        }
        let row = s.rows.first { $0.kind == kind }
        #expect(row?.label == want.label, "\(kind.rawValue) 的标签不对")
        #expect(row?.iconName == want.icon, "\(kind.rawValue) 的图标不对")
    }

    let labels = s.rows.map(\.label)
    #expect(Set(labels).count == 6, "六条文案两两不同：画成一样等于其中一类永远看不见")
    #expect(labels.allSatisfy { !$0.isEmpty })
    #expect(Set(s.rows.map(\.iconName)).count == 6, "图标同理（同 TransferRow.iconName 的纪律）")
}

@Test func eachClassListsItsPathsVerbatim() {
    // 约束 3：路径原文照登 —— 不取最后一段、不规范化、不转义
    let weird = "client-test/C24-8_×_25WS024/reads 1.fq.gz"
    let dotty = "a/./b.txt"
    let s = VerifySummary.of(.fixture(ok: [weird], bad: [dotty]))!
    #expect(s.rows.first { $0.kind == .ok }?.paths == [weird])
    #expect(s.rows.first { $0.kind == .bad }?.paths == [dotty])
    // 空的那几类**什么都不列**（不是列一个占位符、也不是列别的类的路径）
    #expect(s.rows.first { $0.kind == .unverifiable }?.paths == [])
}

// ---------------------------------------------------------------------------
// `all_good`：内核的口径，一个字都不改
// ---------------------------------------------------------------------------

@Test func allGoodExcludesUnverifiable() {
    // ⚠️ 内核的 all_good 不含 unverifiable —— 壳不得"更严格"地把它算进去
    let s = VerifySummary.of(.fixture(ok: ["a"], bad: [], missing: [],
                                      sizeMismatch: [], unverifiable: ["u"], unreadable: []))!
    #expect(s.allGood == true)
    #expect(s.failedCount == 0)
    #expect(s.headline == "全部通过")
    #expect(s.headlineColor == .green)
}

@Test func theScreenNeverSaysAllClearWhileListingAFailure() {
    // 内核的 `all_good` 是权威，但"一份含失败项的结果被总结成『全部通过』"是这一屏最坏的失效。
    // 总结那一行以**明细**为准；内核的判定**原样**留在 `allGood` 上（照登，不二次推导）。
    let s = VerifySummary.of(.fixture(bad: ["b"], allGood: true))!
    #expect(s.allGood == true)
    #expect(s.headline == "1 项未通过")
    #expect(s.headlineColor == .red)
}

@Test func aKernelJudgementOfNotAllGoodIsNeverDropped() {
    let s = VerifySummary.of(.fixture(ok: ["a"], allGood: false))!
    #expect(s.allGood == false)
    #expect(s.headline != "全部通过")
    // 只有内核自相矛盾时才走得到这一支（六类里没有失败项、内核却说没全过）
    #expect(s.headline == "内核判定未全部通过（六类里没有失败项）")
}

@Test func anEmptyResultIsNotAnAllClear() {
    // ⚠️ 内核在"这一批还没有文件被归类"时回的正是**六类全空 + all_good == true**
    //    （`core/src/main.rs` 把 `k.verify` 重置成 `CheckResult::default()`，
    //     而 `CheckResult::all_good()` 在四项失败全空时为真）。
    //    照登 all_good 会让一批**从没校验过**的文件显示「全部通过」—— 一张空洞的通行证。
    let s = VerifySummary.of(.fixture())!
    #expect(s.classifiedCount == 0)
    #expect(s.headline == "尚未校验")
    #expect(s.headlineColor == .secondary)
    #expect(s.allGood == true, "内核的判定照样登在 allGood 上，只是总结那一行不说『全部通过』")
}

@Test func noResultAtAllIsNotAnAllClear() {
    // `model.verify == nil`（还没取过）≠「六类全 0 + 全部通过」。
    // 渲染成后者就是在替内核宣布一句它根本没说过的话。
    #expect(VerifySummary.of(nil) == nil)
}

@Test func everyVerdictHasItsOwnWording() {
    // ⚠️ 同 `everyClassHasADistinctLabel`：只断言"两两不同"会让"哪一档是哪句话"无人守
    //    （把「尚未校验」与「全部通过」对调照样全绿 —— 而那是这一屏最坏的一句话反转）。
    //    所以逐档钉死文案、颜色与图标。
    // （`VerifyVerdict` 只 `Equatable`，不是 `Hashable`，所以是数组不是字典。）
    let expected: [(verdict: VerifyVerdict, text: String, color: RowColor, icon: String)] = [
        (.notVerified, "尚未校验", .secondary, "clock"),
        (.allGood, "全部通过", .green, "checkmark.seal.fill"),
        (.failed(1), "1 项未通过", .red, "exclamationmark.triangle.fill"),
        // ⚠️ 两档 `failed` 是**刻意**的：只钉 `failed(1)` 挡不住"把计数丢掉"的实现
        //    （写死一句「未通过」也满足上面那一行）。
        (.failed(3), "3 项未通过", .red, "exclamationmark.triangle.fill"),
        (.kernelSaysNotAllGood, "内核判定未全部通过（六类里没有失败项）", .orange,
         "exclamationmark.triangle"),
    ]
    for e in expected {
        #expect(e.verdict.text == e.text, "\(e.verdict) 的文案不对")
        #expect(e.verdict.color == e.color, "\(e.verdict) 的颜色不对")
        #expect(e.verdict.iconName == e.icon, "\(e.verdict) 的图标不对")
    }

    let texts = [VerifyVerdict.notVerified.text, VerifyVerdict.allGood.text,
                 VerifyVerdict.failed(1).text, VerifyVerdict.kernelSaysNotAllGood.text]
    #expect(Set(texts).count == 4)
    #expect(texts.allSatisfy { !$0.isEmpty })
}

// ---------------------------------------------------------------------------
// `unverifiable`：中性色 + 一句"不是失败"
// ---------------------------------------------------------------------------

@Test func theUnverifiableClassIsTheOnlyNeutralColour() {
    let s = VerifySummary.of(.fixture(ok: ["a"], unverifiable: ["u"]))!
    let unverifiable = s.rows.first { $0.kind == .unverifiable }!
    #expect(unverifiable.color == .secondary)
    #expect(s.rows.filter { $0.kind != .unverifiable }.allSatisfy { $0.color != .secondary },
            "中性色是『不是失败』的专属标记：别的类也用上它就分不出来了")
    #expect(unverifiable.isFailure == false)
}

@Test func theUnverifiableClassIsExplainedAsNotAFailure() {
    let s = VerifySummary.of(.fixture(unverifiable: ["u"]))!
    #expect(s.rows.first { $0.kind == .unverifiable }?.note == "这些文件没有可比的校验值，不是失败")
    #expect(s.rows.filter { $0.note != nil }.map(\.kind) == [.unverifiable],
            "说明只挂在 unverifiable 上（别的类没有这条歧义要消）")
}

// ---------------------------------------------------------------------------
// 计数：哪些类算"未通过"
// ---------------------------------------------------------------------------

@Test func theOnlyGreenClassIsThePassingOne() {
    let s = VerifySummary.of(.fixture(ok: ["a"], bad: ["b"], missing: ["m"]))!
    #expect(s.rows.filter { $0.color == .green }.map(\.kind) == [.ok])
    #expect(s.rows.filter(\.isFailure).allSatisfy { $0.color == .red })
    #expect(s.failedCount == 2)
    #expect(s.headline == "2 项未通过")
}

@Test func theFailureCountCoversEveryFailureClassIncludingUnreadable() {
    let s = VerifySummary.of(.fixture(ok: ["a"], bad: ["b"], missing: ["m"],
                                      sizeMismatch: ["s"], unverifiable: ["v"], unreadable: ["u"]))!
    #expect(s.failedCount == 4, "bad + missing + size_mismatch + unreadable：一个都不能漏")
    #expect(s.classifiedCount == 6)
    #expect(s.headline == "4 项未通过")
    #expect(s.classifiedText == "已校验 6 项")
}

// ---------------------------------------------------------------------------
// 侧边栏徽标
// ---------------------------------------------------------------------------

@Test func sidebarBadgeCountsEverythingNotPassed() {
    // 未完成 = 待下 + 下载中 + 失败（已完成与已移除都不算）
    let list = TransferListResult(
        items: [TransferItem.fixture(gid: "w", state: .waiting, rawStatus: "waiting"),
                TransferItem.fixture(gid: "a", state: .active, rawStatus: "active"),
                TransferItem.fixture(gid: "e", state: .error, rawStatus: "error"),
                TransferItem.fixture(gid: "c", state: .complete, rawStatus: "complete"),
                TransferItem.fixture(gid: "r", state: .removed, rawStatus: "removed")],
        global: GlobalStat(downloadSpeed: 0, numActive: 1, numWaiting: 1, numStopped: 3))
    #expect(SidebarBadge.unfinishedTransfers(list) == 3)

    // 校验结果：未通过 = bad + missing + size_mismatch + unreadable
    let verify = VerifyStatus.fixture(ok: ["a"], bad: ["b"], missing: ["m"],
                                      sizeMismatch: ["s"], unverifiable: ["v"], unreadable: ["u"])
    #expect(SidebarBadge.unpassed(verify) == 4)
}

@Test func theVerifyBadgeDoesNotCountUnverifiable() {
    // 内核的 all_good 不含 unverifiable ⇒ 徽标也不能把它算成"未通过"
    // （否则徽标会挂一个红色计数、而总结那一行同时写着「全部通过」——自相矛盾）。
    #expect(SidebarBadge.unpassed(.fixture(ok: ["a"], unverifiable: ["u", "v"])) == 0)
    #expect(SidebarBadge.unpassed(.fixture(bad: ["b"], unreadable: ["u"])) == 2)
    #expect(SidebarBadge.unpassed(nil) == 0)
}

@Test func noTransferSnapshotMeansNoBadge() {
    #expect(SidebarBadge.unfinishedTransfers(nil) == 0)
    let allDone = TransferListResult(
        items: [TransferItem.fixture(state: .complete, rawStatus: "complete")],
        global: GlobalStat(downloadSpeed: 0, numActive: 0, numWaiting: 0, numStopped: 1))
    #expect(SidebarBadge.unfinishedTransfers(allDone) == 0)
}

// ---------------------------------------------------------------------------
// 总进度（`get_tree` 的 `progress`）
// ---------------------------------------------------------------------------

@Test func progressComesFromTheTreeAndUsesTheSharedPercentRule() {
    let tree = TreeResult.fixture(totalBytes: 4096, doneBytes: 1024, speed: 2048)
    let p = ProgressSummary.of(tree: tree, treeCode: "C1", code: "C1")!
    #expect(p.percentText == "25%")
    #expect(p.fraction == 0.25)
    #expect(p.bytesText == "1.0 KB / 4.0 KB")
    #expect(p.speedText == "2.0 KB/s")
}

@Test func progressIsNotShownWhenTheTreeBelongsToAnotherBatch() {
    // 与 `BrowserSelection.onNewManifest` 同一条纪律：树必须**属于这一批**。
    // 少了这道闸，换批时会把**上一批**的进度摆在**这一批**的批次摘要底下（静默错数）。
    let tree = TreeResult.fixture()
    #expect(ProgressSummary.of(tree: tree, treeCode: "A", code: "B") == nil)
    #expect(ProgressSummary.of(tree: tree, treeCode: nil, code: "B") == nil)
    #expect(ProgressSummary.of(tree: tree, treeCode: "B", code: nil) == nil)
    #expect(ProgressSummary.of(tree: nil, treeCode: "B", code: "B") == nil)
    #expect(ProgressSummary.of(tree: tree, treeCode: "B", code: "B") != nil)
}

@Test func aBatchWithNothingToDownloadDoesNotClaimZeroPercent() {
    // total == 0 时与传输列表同一口径（`PercentFormat`）：说「—」，不说「0%」
    let p = ProgressSummary.of(tree: TreeResult.fixture(totalBytes: 0, doneBytes: 0, speed: 0),
                               treeCode: "C", code: "C")!
    #expect(p.percentText == "—")
    #expect(p.fraction == 0)
}

// ---------------------------------------------------------------------------
// 刷新失败的那句话
// ---------------------------------------------------------------------------

@Test func refreshFailuresShowTheKernelMessageVerbatim() {
    // 视图不映射 `CoreError`（约束 8）：`VerifyRefreshFailure.message(of:)` 是这一层的入口，
    // 内部走 `AppModel.message(of:)`（`CoreError` → 用户可见文案的唯一实现）。
    #expect(VerifyRefreshFailure.message(of: CoreError.rpc(code: "invalid_params",
                                                           message: "内核原文")) == "内核原文")
    #expect(VerifyRefreshFailure.message(of: CoreError.transport("管道断了")) == "管道断了")
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

extension VerifyStatus {
    /// 一次 `verify_status` 的结果，**经解码构造**（键是内核线上的 snake_case）。
    ///
    /// `allGood` 默认按**内核的口径**算（`bad + missing + size_mismatch + unreadable` 是否全空，
    /// 见 `core/src/verify.rs` 的 `CheckResult::all_good`）——夹具本身不能说假话。
    /// 要造"内核与明细矛盾"的现场（`theScreenNeverSaysAllClearWhileListingAFailure` /
    /// `aKernelJudgementOfNotAllGoodIsNeverDropped`）才显式传。
    static func fixture(ok: [String] = [], bad: [String] = [], missing: [String] = [],
                        sizeMismatch: [String] = [], unverifiable: [String] = [],
                        unreadable: [String] = [], allGood: Bool? = nil) -> VerifyStatus {
        let obj: [String: Any] = [
            "ok": ok, "bad": bad, "missing": missing,
            "size_mismatch": sizeMismatch,
            "unverifiable": unverifiable, "unreadable": unreadable,
            "all_good": allGood ?? (bad.isEmpty && missing.isEmpty
                                    && sizeMismatch.isEmpty && unreadable.isEmpty),
        ]
        let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        return try! CoreJSON.decoder.decode(VerifyStatus.self, from: data)
    }
}

extension TreeResult {
    /// 一次 `get_tree` 的结果，**经解码构造**（只用到 `progress` 那一部分）。
    static func fixture(totalBytes: Int64 = 4096, doneBytes: Int64 = 1024, speed: Int64 = 512,
                        percent: Int32 = 25) -> TreeResult {
        let obj: [String: Any] = [
            "tree": [:],
            "flat": [],
            "default_selected": [],
            "progress": ["total_bytes": totalBytes, "done_bytes": doneBytes,
                         "speed": speed, "percent": Int(percent)],
        ]
        let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        return try! CoreJSON.decoder.decode(TreeResult.self, from: data)
    }
}
