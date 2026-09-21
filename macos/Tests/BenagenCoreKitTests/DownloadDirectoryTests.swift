import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 可选下载目录的**纯计算**（阶段 E 规格 §2，任务 3）
//
// 这一份钉住三件事，逐条对着全局约束：
//   · **E-5**：未配置 ⇒ **不传** `--download-dir`（`argument(for:)` 返回 `nil`），
//     判据就是本文件第 1、2 条 —— 而且第 2 条刻意断言"壳**不会**自己编一个
//     '和内核默认一样'的值"，因为那正是 E-5 明禁的静默分叉；
//   · **E-6**：确认文案必须**逐条**写明三条后果，且**不许**说成"一定会重新校验"
//     （用户可能选了一个已经有 `.benagen-state.json` 的目录 —— 规格 §2.3）；
//   · **§2.3 的边界**：目录不可写 / 不存在 ⇒ 在**确认之前**就报错并中止。
//
// ⚠️ 本文件所有碰文件系统的用例都只碰 `FileManager.temporaryDirectory` 下的临时目录。
// ---------------------------------------------------------------------------

/// 一个用完就删的临时目录。**本文件所有碰盘的东西都建在它下面**
///（`~/Library/Application Support/BenagenDownloader/` 是**人类伙伴真实在用**的现场，
/// 测试碰它 = 破坏现场）。形态与 `AppModelHistoryTests.TempHistory` 同款。
private struct TempDirectory {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-download-dir-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    func cleanUp() { try? FileManager.default.removeItem(at: url) }
}

// ---------------------------------------------------------------------------
// E-5：该不该传 `--download-dir`
// ---------------------------------------------------------------------------

@Test func anUnconfiguredPreferencePassesNoDownloadDir() {
    // E-5 的全部内容：**未配置 ⇒ 不传**。传 `nil` 而不是"一个和内核默认一样的值" ——
    // 后者会在内核改默认值的那天**静默分叉**（壳以为还是那个目录，内核已经换了）。
    #expect(DownloadDirectory.argument(for: .empty) == nil)
    #expect(DownloadDirectory.argument(for: AppPreferences(downloadDir: "")) == nil)
    #expect(DownloadDirectory.argument(for: AppPreferences(downloadDir: "   ")) == nil,
            "只有空白 = 未配置（`AppPreferences` 的归一化）")
}

@Test func noUnconfiguredPreferenceEverPutsAPathInTheArgv() {
    // ⚠️ 这一条是上一条的**判别式**：它防的是"顺手写死一个 $HOME/Downloads/Benagen"。
    //    只要壳自己算出那个路径，内核哪天改了默认值，壳就会把用户**悄悄**带回老地方 ——
    //    而这一次"分叉"在界面上一个字都不会说。
    //
    // ⚠️ **这里曾经是一条恒真断言**（E-8 ②，最终审查顺手 5）：
    //    `argument(for: .empty) != kernelDefaultDisplayPath` 左边是 `String?`、右边是
    //    `String`，`nil` 与任何非空串都不等 ⇒ **怎么写都是绿的**，判别力全在上面那句
    //    `== nil` 上（而且与上一条用例重复）。真正要钉的是 **argv**：未配置的**任何**
    //    形态（空串 / 只有空白）下，argv 里都不许出现**路径形式的**参数 ——
    //    把"壳自己造了一个默认路径"这个变异体放进来，下面三行立刻红。
    let unconfigured = [AppPreferences.empty,
                        AppPreferences(downloadDir: ""),
                        AppPreferences(downloadDir: "   ")]
    for prefs in unconfigured {
        let argv = CoreClient.coreArguments(settingsPath: nil,
                                            downloadDir: DownloadDirectory.argument(for: prefs))
        #expect(argv.isEmpty,
                "未配置 ⇒ argv 里一个参数都不许有（不许自己编一个「和内核默认一样」的路径），实际 \(argv)")
    }
}

@Test func aConfiguredPreferencePassesTheDirectoryVerbatim() {
    let prefs = AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录")
    #expect(DownloadDirectory.argument(for: prefs) == "/Volumes/Data/交付/有 空格的目录",
            "原样进 argv：壳不规范化（理由见 `AppPreferences.normalized` 的注释）")
}

@Test func theCriterionEndsUpInTheRealArgv() {
    // 判据与**真正的 argv 构造器**对上（`CoreClient.coreArguments` 是那唯一一份实现）：
    // 未配置 ⇒ argv 里**没有** `--download-dir` 这个 flag（E-5 的"逐字节保持今天的形态"）。
    #expect(CoreClient.coreArguments(settingsPath: nil,
                                     downloadDir: DownloadDirectory.argument(for: .empty)) == [])
    #expect(CoreClient.coreArguments(
        settingsPath: nil,
        downloadDir: DownloadDirectory.argument(for: AppPreferences(downloadDir: "/data/交付")))
        == ["--download-dir", "/data/交付"])
}

// ---------------------------------------------------------------------------
// E-6：确认文案
// ---------------------------------------------------------------------------

@Test func theConfirmationNamesAllThreeConsequences() {
    let text = DownloadDirectory.confirmation(from: "", to: "/Volumes/Data/交付")

    #expect(text.contains("正在跑的任务会停"), "① 重启内核 ⇒ 正在跑的任务会停")
    #expect(text.contains("新目录里没有记录的文件会重新校验"),
            "② 状态文件就在下载目录里（`core/src/state.rs:18-21`）")
    #expect(text.contains("旧目录里的文件不会被删、也不会被搬走"), "③ 不删、不搬")
    #expect(text.contains("/Volumes/Data/交付"), "要说清楚改到哪去")
}

@Test func theConfirmationNeverClaimsEveryFileWillBeReverified() {
    // ⚠️ 规格 §2.3：用户可能选了一个**已经有 `.benagen-state.json`** 的目录（例如换回旧目录），
    //    那时状态会被内核读回来 ⇒ 一个字节都不用重新校验。
    //    文案断言"一定会重新校验"就是在**说假话**，而用户会照着它做决定。
    let text = DownloadDirectory.confirmation(from: "/old", to: "/new")
    #expect(!text.contains("一定会重新校验"))
    #expect(!text.contains("所有文件"))
    #expect(!text.contains("全部重新校验"))
}

@Test func theConfirmationSaysWhatTheCurrentDirectoryIs() {
    // 两边都要说：只说"改成什么"的话，用户在**换回旧目录**时看不出自己现在在哪。
    let text = DownloadDirectory.confirmation(from: "/Volumes/A", to: "/Volumes/B")
    #expect(text.contains("/Volumes/A"))
    #expect(text.contains("/Volumes/B"))

    // 未配置那一侧显示的是"默认"，而不是一个空串（空串在界面上读起来像"坏了"）。
    let fromDefault = DownloadDirectory.confirmation(from: "", to: "/Volumes/B")
    #expect(fromDefault.contains("默认"))
    #expect(!fromDefault.contains("「」"), "不许出现一对空引号")
}

@Test func theDisplayFallsBackToTheDefaultLabelWhenUnconfigured() {
    #expect(DownloadDirectory.display("/Volumes/Data/交付") == "/Volumes/Data/交付")
    #expect(DownloadDirectory.display("").contains("默认"))
    #expect(!DownloadDirectory.display("").isEmpty)
}

// ---------------------------------------------------------------------------
// 回执（设置窗口里那一行）
// ---------------------------------------------------------------------------

@Test func eachOutcomeSaysWhatHappened() {
    // 🔴 这三句是**壳自己写的**：内核不会为一次成功的重启主动说话。而"我按了确认之后
    //    发生了什么"必须有个落点 —— 否则那个确认框就像按了个寂寞。
    #expect(DownloadDirChange.changed(dir: "/Volumes/Data/交付").noticeText.contains("/Volumes/Data/交付"))
    #expect(DownloadDirChange.changed(dir: "/Volumes/Data/交付").noticeText.contains("重启"))
    #expect(DownloadDirChange.changed(dir: "").noticeText.contains("默认"),
            "恢复默认那一格不能说成'已改为 '（一个空路径）")
    #expect(DownloadDirChange.unchanged(dir: "/x").noticeText.contains("没有"),
            "没改动就要说没改动（用户得知道自己那次点击是有结果的）")

    // 失败：**原文照登**（约束 3），不加工、不加前缀。
    let failure = DownloadDirChange.failed(message: "内核重启失败：内核无响应（等待超过 5 秒）")
    #expect(failure.noticeText == "内核重启失败：内核无响应（等待超过 5 秒）")
    #expect(failure.isFailure)
    #expect(!DownloadDirChange.changed(dir: "/x").isFailure)
    #expect(!DownloadDirChange.unchanged(dir: "/x").isFailure)
}

// ---------------------------------------------------------------------------
// 常驻提示行的**高度不许由外部文本决定**（阶段 E 修复波）
//
// 现场（人类伙伴实测）：改完下载目录、内核按新目录重启之后，**文件页底栏**
// （全选 / 全不选 / 下载选中）与**侧边栏的批次摘要**一起从窗口里消失；
// "把窗口拉高" 或 "点「恢复默认」" 就恢复。根因在**本文件的 `noticeText`**：
// 成功那一支把**用户选的完整路径**塞进了那条**常驻**回执，而那一行没有行数上限、
// 还带 `fixedSize(vertical:)` ⇒ **路径越长这一行越高** ⇒ 内容比窗口高 ⇒ 锚在底边的
// 两处一起被裁掉。恢复默认那一句**不含路径、固定短**，所以它一按就恢复（观测 ④）。
//
// 修法的判据落在**这里**（视图不单测是本项目的硬约束）：把"标题"与"路径"拆成两个
// **纯计算**属性 —— 标题**任何路径下都一样长**，路径单独一行、由视图去截断。
// 下面第一条就是**根因的回归守卫**：只要有人再往标题里塞一次路径，它立刻红。
// ---------------------------------------------------------------------------

/// 一条**很长**的路径（比任何一句标题都长得多）。守卫用它当输入。
private let aVeryLongPath =
    "/Volumes/数据盘_2026/交付给客户/贝纳基因/2026-09-18/批次_C24-8_×_25WS024/原始数据/测序下机/再一次深层/还要更深"

@Test func theHeadlineIsFixedNoMatterHowLongThePathIs() {
    // 🔴 **根因的回归守卫**（本次布局事故）。标题是这条回执的**第一行**，它必须
    //    与路径长短**完全无关** —— 否则"选了一个深层目录"就等于"把界面顶高几行"，
    //    而常驻行的任何一点额外高度都会把锚在窗口底边的内容（文件页底栏 / 侧边栏摘要）
    //    挤出可见区。判别式：两条**逐字相同**。
    let short = DownloadDirChange.changed(dir: "/x").headline
    let long = DownloadDirChange.changed(dir: aVeryLongPath).headline
    #expect(short == long,
            "标题不许随路径变化：短路径 \(short) / 长路径 \(long)")

    // 再把"路径没漏进去"钉死（上面那条只要有人给两支都塞同一段路径就恒真了）。
    #expect(!long.contains("/"), "标题里不许出现任何路径片段：\(long)")
    #expect(!long.contains("数据盘_2026"), "标题里不许出现路径里的目录名：\(long)")
    #expect(!DownloadDirChange.changed(dir: aVeryLongPath).headline.contains(aVeryLongPath))
}

@Test func theHeadlineSaysWhatHappenedWithoutThePath() {
    // 标题不是空串、也不许把路径挖掉之后留下一句读不通的话：四支各自的**语义**要在。
    #expect(DownloadDirChange.changed(dir: "/Volumes/A").headline.contains("重启"),
            "改成功那一支要说内核重启过")
    #expect(DownloadDirChange.changed(dir: "").headline.contains("默认"),
            "恢复默认那一支要说回到默认（不许说成「已改为 」一个空路径）")
    #expect(DownloadDirChange.unchanged(dir: "/x").headline.contains("没有"),
            "没改动就要说没改动")
    #expect(DownloadDirChange.failed(message: "内核重启失败：内核无响应（等待超过 5 秒）").headline
            == "内核重启失败：内核无响应（等待超过 5 秒）",
            "失败那一支是**内核原文逐字**（约束 3）：标题就是全文，壳不加工、不加前缀")
}

@Test func thePathIsCarriedSeparatelyAndOnlyWhenThereIsOne() {
    // 路径单独交给视图去渲染（视图拿它做 `lineLimit(1)` + 中间截断 + 悬停看全文）。
    #expect(DownloadDirChange.changed(dir: "/Volumes/Data/交付").pathDetail == "/Volumes/Data/交付")
    #expect(DownloadDirChange.changed(dir: aVeryLongPath).pathDetail == aVeryLongPath,
            "路径**原样**传出去（截断是视图的事，值这一层不许自己动手）")

    // 其余三支都没有"路径"这一行可显示。
    #expect(DownloadDirChange.changed(dir: "").pathDetail == nil,
            "恢复默认 = 空串 ⇒ **没有路径行**（空行在界面上读起来像「这一行坏了」）")
    #expect(DownloadDirChange.unchanged(dir: "/Volumes/A").pathDetail == nil,
            "没改动 ⇒ 没有路径行")
    #expect(DownloadDirChange.failed(message: "这个位置不存在：/Volumes/没了").pathDetail == nil,
            "失败那句是内核/系统的原文（约束 3）⇒ 作为一个整体显示，不许被拆成「路径」")
}

@Test func theNoticeTextIsWordForWordWhatItUsedToBe() {
    // ⚠️ 这一条**刻意逐字**（不是 `contains`）：布局事故的修复只许**拆结构**、
    //    不许**改措辞**（约束 3：失败那句是原文；成功那两句是壳写的既有文案）。
    //    ⚠️ 它**不是**"界面上显示的就是这句"的证据 —— 界面渲染的是 `headline` +
    //    `pathDetail` 两行，`noticeText` 在 `Sources/` 里**零引用**；
    //    这一条守的是**措辞**（任何人顺手改一个字，这里就红），
    //    也就是那句话作为 Kit 对外语义的那一份。
    //    四支各钉一条。
    #expect(DownloadDirChange.changed(dir: "/Volumes/Data/交付").noticeText
            == "下载目录已改为 /Volumes/Data/交付（内核已按新目录重启）")
    #expect(DownloadDirChange.changed(dir: "").noticeText
            == "已恢复默认下载目录（内核已按默认目录重启）")
    #expect(DownloadDirChange.unchanged(dir: "/x").noticeText
            == "下载目录没有变化，没有重启内核")
    #expect(DownloadDirChange.failed(message: "内核重启失败：内核无响应（等待超过 5 秒）").noticeText
            == "内核重启失败：内核无响应（等待超过 5 秒）")

    // 而且它必须**仍然由标题与路径组合而成**（不是又抄了一份字面量）：
    // 有路径 ⇒ 那句完整的话里含标题的固定部分与路径；没路径 ⇒ 标题就是全文。
    let withPath = DownloadDirChange.changed(dir: aVeryLongPath)
    #expect(withPath.noticeText.contains(withPath.pathDetail ?? ""),
            "路径必须进那句完整的话（设置窗口里那一段仍然要把完整路径显示出来）")
    #expect(DownloadDirChange.changed(dir: "").noticeText
            == DownloadDirChange.changed(dir: "").headline,
            "恢复默认：没有路径可拆 ⇒ 全文就是标题（两处不可能分叉）")
}

@Test func theSectionNoteExplainsWhatUnconfiguredMeans() {
    // ⚠️ 它是本功能唯一反直觉的地方（"没有设置"也是一种设置），而这一句是用户
    //    唯一能读到它的地方 —— 别在改文案时把它删掉。
    #expect(DownloadDirectory.sectionNote.contains("默认"))
    #expect(DownloadDirectory.sectionNote.contains("重启"))
}

@Test func theSectionNoteWarnsAboutNetworkVolumes() {
    // ⚠️ Ruling D3 的复核结论（最终审查重要 3）：阶段 D 判"核对在锁内做"可以接受，
    //    前提是**下载目录在本地盘上**——而那时这是结构上成立的（硬编码
    //    `$HOME/Downloads/Benagen`）。阶段 E 让用户可以把它指到任何地方，
    //    于是"网络盘上的秒级 stat 会拖住内核"变成**用户一步操作就能到达**的场景。
    //    裁定是**不改内核**，但要求"用户看得见"：这一句就是那个落点
    //    （另一处是 `macos/README.md` 的已知限制）。
    //    这条断言钉的是它**还在**且说的是**网络盘**这件事 —— 删掉这一句就红。
    #expect(DownloadDirectory.sectionNote.contains("网络盘"),
            "要在设置里说清楚网络盘会变慢：\(DownloadDirectory.sectionNote)")
    #expect(DownloadDirectory.sectionNote.contains("本机磁盘"),
            "光说「网络盘」不够 —— 要说出该选什么")
}

// ---------------------------------------------------------------------------
// §2.3 边界：不可写 / 不存在 ⇒ 确认**之前**就报错
// ---------------------------------------------------------------------------

@Test func aWritableExistingDirectoryPassesTheCheck() throws {
    let dir = try TempDirectory()
    defer { dir.cleanUp() }

    #expect(DownloadDirectory.check(dir.url.path) == nil)
}

@Test func aMissingDirectoryIsRejectedWithAnExplanation() throws {
    let dir = try TempDirectory()
    defer { dir.cleanUp() }
    let missing = dir.url.appendingPathComponent("还不存在/再深一层")

    let problem = DownloadDirectory.check(missing.path)
    #expect(problem != nil, "不存在的目录不许放行 —— 否则要等重启完才发现")
    #expect(problem?.contains(missing.path) == true, "要把是哪个路径说出来：\(problem ?? "nil")")
}

@Test func aPlainFileIsRejected() throws {
    let dir = try TempDirectory()
    defer { dir.cleanUp() }
    let file = dir.url.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: file)

    let problem = DownloadDirectory.check(file.path)
    #expect(problem != nil, "选中的是一个文件 ⇒ 报错，不许当成目录用")
    #expect(problem?.contains(file.path) == true)
}

@Test func anUnwritableDirectoryIsRejected() throws {
    let dir = try TempDirectory()
    defer { dir.cleanUp() }
    let locked = dir.url.appendingPathComponent("只读", isDirectory: true)
    try FileManager.default.createDirectory(at: locked, withIntermediateDirectories: true)
    try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: locked.path)
    defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755],
                                                  ofItemAtPath: locked.path) }

    // root 对任何目录都可写 ⇒ 这条用例在 root 下没有判别力，**如实跳过**（不假装通过）。
    guard !FileManager.default.isWritableFile(atPath: locked.path) else { return }

    let problem = DownloadDirectory.check(locked.path)
    #expect(problem != nil, "不可写的目录不许放行")
    #expect(problem?.contains(locked.path) == true)
}

@Test func theUnconfiguredPathIsNotCheckedBecauseTheKernelOwnsItsDefault() {
    // 「恢复默认」的目标是**空串** —— 它不是一个路径，壳也无从检查
    //（内核会用自己的默认值，并在启动时建它）。
    #expect(DownloadDirectory.check("") == nil)
}
