import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 改下载目录接进 `AppModel`（阶段 E 规格 §2.2，任务 3）
//
// 这里钉的是**一条链**，而不是一个函数：
//
//   用户确认 → 偏好落盘（E-2）→ **重启内核**（新进程带 `--download-dir <新目录>`）
//            → 重启流程里**自动重新加载当前批次**（新目录的状态是空的 ⇒ 界面必须反映这一点）
//
// ⚠️ **"重启出来的内核拿到的是新目录"是哪一条**：`changingTheDirectoryRestartsTheKernelWithTheNewDirectory`
//    里的 `factory.downloadDirs == ["/Volumes/Data/交付"]`。判据的落点是**工厂收到的那一格参数**
//    （`AppModel.makeClient` 的 `String?`），也就是最终进 `CoreClient.coreArguments` 的那个值 ——
//    不是壳里某个中间变量。
//
// ⚠️ **E-5 在模型这一侧的判据**：`restoringTheDefaultStopsPassingTheDownloadDir`
//    （未配置 ⇒ 工厂收到的是 `nil`，不是空串、更不是内核默认目录的路径）。
//
// ⚠️⚠️ 安全红线：偏好 store 一律指向临时目录。**没有任何一条**用例调用 `AppModel.live()`
//     —— 它会起真内核、读人类伙伴真实的 `preferences.json`。
// ---------------------------------------------------------------------------

/// 一个用完就删的临时偏好文件（**唯一**的建 store 的方式，见文件头的红线）。
private struct TempPrefs {
    let dir: URL
    let store: AppPreferencesStore

    init() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-model-download-dir-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        store = AppPreferencesStore(url: dir.appendingPathComponent("preferences.json"))
    }

    func cleanUp() { try? FileManager.default.removeItem(at: dir) }
}

/// 换目录之后那一批**在新内核里**的树。**故意与 `Wire.treeA` 不同**
///（文件名不同）：这样"界面显示的还是上一个内核那份计划"能被一眼抓住 ——
/// 换了目录就是换了状态文件，新内核眼里这批文件全是没下过的。
private let treeInTheNewDirectory = #"""
{"tree":{"type":"dir","name":"AAA-1","children":{}},
 "flat":[{"path":"b.bin","name":"b.bin","size":2000,"state":"pending"}],
 "default_selected":["b.bin"],
 "progress":{"total_bytes":3000,"done_bytes":0,"speed":0,"percent":0}}
"""#

/// 一个"交出握手、能重新加载这一批"的替身内核。
private func aKernelThatCanReload(withTree tree: String) -> FakeCore {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", tree)
    return fake
}

// ---------------------------------------------------------------------------
// 改目录 ⇒ 重启内核，新内核拿到**新**目录
// ---------------------------------------------------------------------------

@MainActor
@Test func changingTheDirectoryRestartsTheKernelWithTheNewDirectory() async throws {
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let new = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"], "前提：界面此刻挂着**旧**内核这一批的树")

    let outcome = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    #expect(outcome == .changed(dir: "/Volumes/Data/交付"))
    #expect(factory.downloadDirs == ["/Volumes/Data/交付"],
            "⚠️ 重启出来的内核拿到的必须是**新**目录（这条是简报点名要钉住的那一条）")
    #expect(old.shutdownCount == 1, "旧内核要先被收尾（不许留下孤儿）")
    #expect(model.downloadDir == "/Volumes/Data/交付")
}

@MainActor
@Test func theCurrentBatchIsReloadedByTheRestartedKernel() async throws {
    // 规格 §2.2 第 4 条：重启成功之后**自动重新加载当前批次**。
    // ⚠️ 少了这一步，界面会一直显示**上一个内核**那份"已完成"的清单 ——
    //    而新目录的状态文件是空的，那批"已完成"是**假的**。
    //    （阶段 D 刚修掉的那类缺陷，不要在这儿重新造出来。）
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let new = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    let generationBefore = model.loadGeneration

    _ = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    #expect(new.callCount("load_delivery") == 1, "重启之后必须重新加载当前批次")
    #expect(new.params("load_delivery") == .object(["code": .string("AAA-1"),
                                                   "base_url": .string("http://dl.example")]),
            "用的还是同一个码（与它的 base_url）")
    #expect(model.tree?.flat.map(\.path) == ["b.bin"],
            "界面显示的必须是**新内核**这份计划，不是上一个内核那份")
    #expect(model.treeCode == "AAA-1", "树与'它属于哪一批'照旧一起落")
    #expect(model.loadGeneration == generationBefore + 1,
            "加载代数要推进 —— 视图靠它的变化重播这一批的默认勾选面")
    guard case .loaded = model.loadState else {
        Issue.record("重启后的加载成功了，界面不该停在别处：\(model.loadState)")
        return
    }
}

@MainActor
@Test func changingTheDirectoryDropsTheStaleVerifySnapshot() async throws {
    // 那份快照说的是"**上一个目录里**这些文件的校验结果"。换了下载根之后它一个字都不成立
    //（新目录里一个文件都还没校验过），留着它就是一组**看起来属于这一批**的旧数字，
    // 而侧边栏徽标与整个校验屏都直接读它 —— 与阶段 D 修掉的"已完成假象"是同一类缺陷。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    old.stub("verify_status", Wire.verify)
    let new = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    await model.refreshVerify()
    #expect(model.verify != nil, "前提：屏上有一份快照")

    _ = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    #expect(model.verify == nil,
            "换了下载根 ⇒ '哪些文件校验过'这个问题必须重新问新内核（置 nil = 还不知道）")
}

// ---------------------------------------------------------------------------
// E-5：未配置 ⇒ 不传 `--download-dir`（模型这一侧的判据）
// ---------------------------------------------------------------------------

@MainActor
@Test func restoringTheDefaultStopsPassingTheDownloadDir() async throws {
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let new = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([new])
    // 壳现在是"配过一个目录"的状态（生产里这个初值来自 `AppPreferencesStore.load()`）。
    let model = AppModel(client: old, makeClient: factory.factory,
                         preferencesStore: temp.store, downloadDir: "/Volumes/旧")

    let outcome = await model.changeDownloadDir(to: "")

    #expect(outcome == .changed(dir: ""))
    #expect(factory.downloadDirs == [nil],
            "未配置 ⇒ 传 **nil**（E-5：**不传**那个 flag，不是传一个'和内核默认一样'的值）")
    #expect(temp.store.load() == .empty, "盘上那份也回到未配置")
}

@MainActor
@Test func anUnconfiguredShellKeepsPassingNothing() async throws {
    // 反向：本来就没配过 ⇒ 改目录那条路上凡是没有真的改动的情形，工厂收到的都还是 `nil`。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let first = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let second = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([first, second])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    // 一次真的改动（配上一个目录），再改回未配置 —— 第二跳必须是 nil。
    _ = await model.changeDownloadDir(to: "/Volumes/中间")
    _ = await model.changeDownloadDir(to: "")

    #expect(factory.downloadDirs == ["/Volumes/中间", nil])
}

// ---------------------------------------------------------------------------
// 偏好落盘（E-2）与"存不下去就别动内核"
// ---------------------------------------------------------------------------

@MainActor
@Test func theChosenDirectoryIsPersistedAndIsWhatTheNextLaunchWouldPass() async throws {
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let new = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    _ = await model.changeDownloadDir(to: "/Volumes/Data/交付/有 空格的目录")

    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录"),
            "必须落盘 —— 否则重启应用就白改了")
    // 下一次启动会传什么：生产里 `AppModel.live()` 做的就是这一句
    //（`argument(for: store.load())` 交给工厂、同时把 `downloadDir` 存进模型）。
    #expect(DownloadDirectory.argument(for: temp.store.load()) == "/Volumes/Data/交付/有 空格的目录")
}

@MainActor
@Test func aPreferenceThatCannotBeSavedAbortsWithoutTouchingTheKernel() async throws {
    // 偏好写不下去 ⇒ **中止**，一个内核都不许动：
    // 否则会出现"内核按新目录在跑、盘上还记着旧目录"的分裂 —— 下次启动**静默**换回去，
    // 而用户以为设置已经生效了。
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-model-download-dir-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    // 目标路径的父级是一个**普通文件** ⇒ 建目录必失败 ⇒ 写盘必失败。
    let blocker = dir.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: blocker)
    let store = AppPreferencesStore(url: blocker.appendingPathComponent("preferences.json"))

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let spare = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([spare])
    let model = AppModel(client: old, makeClient: factory.factory,
                         preferencesStore: store, downloadDir: "/Volumes/旧")
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let outcome = await model.changeDownloadDir(to: "/Volumes/新")

    #expect(outcome.isFailure, "存不下去必须报出来（约束 4：不得静默失效）")
    #expect(factory.madeCount == 0, "存不下去就**别重启内核**")
    #expect(old.shutdownCount == 0, "旧内核照旧活着 —— 用户的任务不该因为一次写盘失败而停")
    #expect(model.downloadDir == "/Volumes/旧", "内存里那份也不许改（它必须与盘上一致）")
}

// ---------------------------------------------------------------------------
// 没有改动 ⇒ 什么都不做（E-6 的前提：没改就不该停任何东西）
// ---------------------------------------------------------------------------

@MainActor
@Test func choosingTheSameDirectoryDoesNotRestartTheKernel() async throws {
    // E-6 说的是"改目录会停掉正在跑的任务"。**没改**就不该停 ——
    // 用户在面板里选中了同一个目录（这很容易发生）不该被重启一次内核。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let spare = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([spare])
    // 生产里模型那个初值就是盘上那份（`live()` 把 `store.load()` 的结果同时交给
    // 第一个内核与模型）—— 这里照着搭，免得测一个生产里不存在的状态。
    try temp.store.save(AppPreferences(downloadDir: "/Volumes/一样"))
    let model = AppModel(client: old, makeClient: factory.factory,
                         preferencesStore: temp.store, downloadDir: "/Volumes/一样")

    let outcome = await model.changeDownloadDir(to: "/Volumes/一样")

    #expect(outcome == .unchanged(dir: "/Volumes/一样"))
    #expect(factory.madeCount == 0, "同值不许重启内核")
    #expect(old.shutdownCount == 0)
    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/一样"),
            "盘上那份原样不动")
    _ = await model.changeDownloadDir(to: "  /Volumes/一样  ")
    #expect(factory.madeCount == 0, "首尾空白不算改动（`AppPreferences` 会先归一化）")
}

// ---------------------------------------------------------------------------
// 回执必须**活过设置窗口**（最终审查重要 2）
//
// 改之前：回执是 `SettingsView` 里的一个 `@State`，**窗口一关就没了**。而这里有一格
// 恰恰是"没改成"——`restartKernel()` 被一次在飞的重启挡住时，**内存与盘上都是新目录、
// 内核还在旧目录上跑**，那句话是**唯一**的提示。窗口关掉之后就是"设置里显示新目录、
// 引擎横幅是绿的、没有任何东西再说这件事"⇒ 用户整个会话都以为文件下到新目录去了。
//
// 与 `lastSwitchOutcome` 逐字同一条论证（那边是因为"换码面板会被关掉"才落到模型上的）。
// 所以本文件钉住的是：**每一格回执都落在 `AppModel.downloadDirChange` 上**。
// ---------------------------------------------------------------------------

@MainActor
@Test func everyDownloadDirReceiptLandsOnTheModelSoItOutlivesTheSettingsWindow() async throws {
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let new = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.downloadDirChange == nil, "还没改过 ⇒ 没有回执（那一行不该凭空出现）")

    // ① 真的改了：回执在模型上。
    let changed = await model.changeDownloadDir(to: "/Volumes/Data/交付")
    #expect(model.downloadDirChange == changed,
            "回执必须落在模型上 —— 它是设置窗口关掉之后**唯一**还在的那一份")
    #expect(model.downloadDirChange == .changed(dir: "/Volumes/Data/交付"))

    // ② 没改（选中同一个目录）：同样要有回执（用户得知道自己那次点击有结果）。
    let unchanged = await model.changeDownloadDir(to: "/Volumes/Data/交付")
    #expect(unchanged == .unchanged(dir: "/Volumes/Data/交付"))
    #expect(model.downloadDirChange == unchanged, "没改也是一条要说出来的结果")

    // ③ 收起：两处（`RootView` 那行提示与设置窗口那一段）共用的唯一出口。
    model.dismissDownloadDirChange()
    #expect(model.downloadDirChange == nil)
}

@MainActor
@Test func aFailedChangeLeavesItsReceiptOnTheModelToo() async throws {
    // 最要紧的那一格：**重启没成**（新目录记下来了、内核还在用旧的跑）。
    // 那句话只在设置窗口里活着的年代，"关掉窗口"就等于"没有任何东西再说这件事"。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([])          // 工厂没有替身 ⇒ 重启必失败
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let outcome = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    #expect(model.downloadDirChange == outcome)
    #expect(model.downloadDirChange?.isFailure == true, "这一格是「没成」，视图据此上警告色")
    #expect(model.downloadDirChange?.noticeText.contains("内核重启失败") == true,
            "模型这一份带着内核原文（逐字）：\(model.downloadDirChange?.noticeText ?? "nil")")
    // ⚠️ 别把上面这条读成"界面上显示的就是 `noticeText`"——**不是**（第 2 轮起）：
    //    视图渲染的是 `headline` + `pathDetail` 两行，与 `noticeText` 只共享措辞。
    //    真正要钉的是**失败那一支原样带出了内核原文**，而 `headline` 就是这一句
    //    （`DownloadDirectoryTests.theNoticeTextIsWordForWordWhatItUsedToBe` 逐字钉着
    //    "失败那一支 = 原文照登"），所以这条仍然有效。
}

@MainActor
@Test func aNewChangeDropsThePreviousReceiptWhileItIsStillInFlight() async throws {
    // 同 `switchDelivery` 的那一条纪律：那一行说的是"上一次那一按"，
    // 不该在新一次已经开始之后还挂在屏幕上（否则用户会把它读成这一次的结论）。
    // ⚠️ 判据必须取在**在飞的那一刻**（下面那道闸），只断言"最后换成了新的"
    //    对"有没有先收起"没有判别力。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let gate = TestGate()
    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let first = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let blocked = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    blocked.gate = gate                     // 第二次改动会卡在握手上（"在飞"那一刻）
    let factory = ClientFactory([first, blocked])
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let firstOutcome = await model.changeDownloadDir(to: "/Volumes/一")
    #expect(model.downloadDirChange == firstOutcome, "前提：屏上挂着上一次的回执")

    let second = Task { await model.changeDownloadDir(to: "/Volumes/二") }
    _ = await waitUntil("第二次改动的内存新值已经落定（此刻它正卡在握手）") {
        model.downloadDir == "/Volumes/二"
    }
    #expect(model.downloadDirChange == nil,
            "新一次已经开始 ⇒ 上一次那句结论必须先收起")
    gate.release()
    let secondOutcome = await second.value
    #expect(model.downloadDirChange == secondOutcome,
            "它跑完之后落在同一个字段上（收尾的那一格不许漏）")
}

// ---------------------------------------------------------------------------
// 内核重启那条路的复用：用户动作要能重启（不受自动重启预算的约束）
// ---------------------------------------------------------------------------

@MainActor
@Test func changingTheDirectoryResetsTheAutomaticRestartBudget() async throws {
    // `restartAttempted` 的预算是**按用户动作**计的（见 `restartIfAllowed` 的注释）。
    // 改目录是一个显式的用户动作 —— 自动重启已经用掉预算，这里**仍然要能重启**。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let dying = aKernelThatCanReload(withTree: Wire.treeA)
    let revived = aKernelThatCanReload(withTree: Wire.treeA)
    let afterChange = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([revived, afterChange])
    let model = AppModel(client: dying, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    await model.refreshTransfers()                 // 内核崩了 → 自动重启用掉这一次预算
    #expect(factory.downloadDirs == [nil], "崩之前壳是未配置的（E-5）")

    let outcome = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    #expect(outcome == .changed(dir: "/Volumes/Data/交付"))
    #expect(factory.downloadDirs == [nil, "/Volumes/Data/交付"],
            "用户的显式动作必须拿到一次新的重启（否则改了目录却没有任何事情发生）")
}

@MainActor
@Test func aDirectoryChangeDuringAnAutomaticRestartStillReachesTheNewKernel() async throws {
    // ⚠️ 一条真实的交错（`restartKernel` 的 `restarting` 闸最长占十秒）：
    //    内核刚崩、**自动重启正在飞**（它已经拿着**旧**目录在起新内核了），
    //    用户在这个窗口里确认了一个新目录（他刚看到横幅变红，就去设置里改）。
    //    若"被那道闸挡回来"就放过这次改动，结果是"内核用旧目录在跑、壳记的是新目录"
    //    —— 症状是文件下到了用户以为已经换掉的地方，而且没有任何提示。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let gate = TestGate()
    let dying = aKernelThatCanReload(withTree: Wire.treeA)
    let blocked = aKernelThatCanReload(withTree: Wire.treeA)
    blocked.gate = gate                    // 卡在 `start()` 的握手上（自动重启跑到一半）
    let later = aKernelThatCanReload(withTree: treeInTheNewDirectory)
    let factory = ClientFactory([blocked, later])
    let model = AppModel(client: dying, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    // 管道结束 = 内核进程没了 ⇒ 自动重启（它会被 `blocked` 卡在握手上）。
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let refresh = Task { await model.refreshTransfers() }
    _ = await waitUntil("自动重启已经用上替身（此时它正卡在握手）") { factory.madeCount == 1 }
    #expect(factory.downloadDirs == [nil], "那一刻壳还没配过目录（E-5）")

    let change = Task { await model.changeDownloadDir(to: "/Volumes/新") }
    // 等新值落到内存：此刻它要么正在等那次重启落地，要么已经带着失败返回了。
    _ = await waitUntil("新目录已经落到内存里") { model.downloadDir == "/Volumes/新" }
    gate.release()

    let outcome = await change.value
    await refresh.value

    #expect(outcome == .changed(dir: "/Volumes/新"))
    #expect(factory.downloadDirs == [nil, "/Volumes/新"],
            "改目录**必须**换来一次带着新目录的重启 —— 不许被一次在飞的重启吞掉")
}

@MainActor
@Test func aRestartThatFailsWithTheNewDirectoryIsReported() async throws {
    // 新目录带起来了、但内核没能起来 ⇒ **说出来**（约束 4），别让用户以为已经好了。
    // 偏好**照旧留在盘上**：那是用户的选择，下一次启动仍然按它起内核 ——
    // 把它偷偷改回去会让"我改过"这件事在界面上消失。
    let temp = try TempPrefs()
    defer { temp.cleanUp() }

    let old = aKernelThatCanReload(withTree: Wire.treeA)
    let factory = ClientFactory([])          // 工厂没有替身 ⇒ 重启必失败
    let model = AppModel(client: old, makeClient: factory.factory, preferencesStore: temp.store)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let outcome = await model.changeDownloadDir(to: "/Volumes/Data/交付")

    guard case .failed(let why) = outcome else {
        Issue.record("重启失败必须报出来，实际 \(outcome)")
        return
    }
    #expect(why.contains("内核重启失败"), "要有落点：\(why)")
    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/Data/交付"),
            "偏好仍然留在盘上（用户的选择不该被一次失败悄悄撤销）")
    #expect(model.downloadDir == "/Volumes/Data/交付")
}
