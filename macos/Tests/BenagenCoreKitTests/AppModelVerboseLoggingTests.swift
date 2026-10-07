import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「详细日志」开关接进 `AppModel`（规格 §2.4，任务 5）
//
// 这里钉的是**另一条**与改下载目录**同形**的链：
//
//   用户勾选 → 偏好落盘 → **重启内核**（新进程带 `--log-level verbose`）
//            → 回执落在 `AppModel.verboseLoggingChange` 上（活过设置窗口）
//
// ⚠️ 两个入口（`changeDownloadDir` / `changeVerboseLogging`）的**公共部分**只有一个实现
//    （`performPreferenceChange`），这里与 `AppModelDownloadDirTests` **各钉一条**。
//
// ---------------------------------------------------------------------------
// 🔴 本文件**为什么不测"打开"那一个方向**（这是一条刻意的、有代价的取舍）
// ---------------------------------------------------------------------------
//
// `changeVerboseLogging` 里那一次 `DiagnosticsLog.configure(…)` 写的是**进程级静态**。
// 测试进程里它一开始是 `normal`，而**所有**用默认出口的 `CoreClient` 用例在
// `callSync` 里都会经过 `DiagnosticsLog.logVerbose` —— 那个函数在 `verbose` 档下会
// **真的往 `~/Library/Application Support/BenagenDownloader/diag-shell.log` 里写**
// （`ShellStorage.directory`，这台机器上就是人类伙伴自己那份）。Swift Testing 默认
// **并行**跑用例，所以"打开"那一条会让同期任何一条用例**随机地**往真日志里灌数据 ——
// 那正是"flaky 比没有判据更坏"。
//
// ⇒ 本文件只走**关掉**那个方向（它把级别落回 `normal`，与默认值一致，进程里
//    **不留下任何窗口**），"打开"那一半由两条别处的判据守着：
//      · `CoreClientTests.theVerboseFlagIsOnlySpelledOutWhenItIsOn`（argv 的纯判据）；
//      · 下面 `theRestartedKernelGetsTheLevelTheModelHolds`（**模型持有的那一格
//        真的进了工厂参数**，与开/关无关）。
//    ⚠️ 而"两处 `configure` 各真的被调到"这件事**本机没有判据**，只由**真机验收**
//       那两条守着（"重启应用之后 `diag-shell.log` 里出现 `event=kernel_call`"与
//       "勾上之后**不重启**壳那份当场就变详细"）—— 如实记账，不另造一个会翻全局的用例。
//
// ⚠️⚠️ 安全红线：偏好 store 一律指向临时目录。**没有任何一条**用例调用 `AppModel.live()`
//     —— 它会起真内核、读人类伙伴真实的 `preferences.json`、并按它去落日志级别。
// ---------------------------------------------------------------------------

/// 一个用完就删的临时偏好文件（与 `AppModelDownloadDirTests.TempPrefs` 同一个理由）。
private struct TempVerbosePrefs {
    let dir: URL
    let store: AppPreferencesStore

    init() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-model-verbose-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        store = AppPreferencesStore(url: dir.appendingPathComponent("preferences.json"))
    }

    func cleanUp() { try? FileManager.default.removeItem(at: dir) }
}

/// 一个"交出握手"的替身内核（本文件只关心重启时工厂拿到了什么，不关心树）。
private func aVerboseKernel() -> FakeCore {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    return fake
}

// ---------------------------------------------------------------------------
// 模型持有的那一格 ⇒ 出厂的内核 argv
// ---------------------------------------------------------------------------

/// 🔴 **重启出来的内核拿到的是模型此刻持有的那一格**。
///
/// 这是"`AppPreferences(downloadDir:).verboseLogging` 传进去"那一步的判据：
/// 判据的落点是**工厂收到的那一格参数**（它最终进 `CoreClient.coreArguments`），
/// 不是壳里某个中间变量。开与关各一次 —— 只钉一个方向的话，"恒传 `false`"
/// 这个变异体是绿的。
@MainActor
@Test func theRestartedKernelGetsTheLevelTheModelHolds() async throws {
    let temp = try TempVerbosePrefs()
    defer { temp.cleanUp() }

    let busy = aVerboseKernel()
    let quiet = ClientFactory([aVerboseKernel(), aVerboseKernel()])
    let verboseModel = AppModel(client: busy, makeClient: quiet.factory,
                                preferencesStore: temp.store, verboseLogging: true)
    await verboseModel.retryEngine()
    #expect(quiet.verboseLoggings == [true], "开着 ⇒ 重启出来的内核必须拿到详细档")

    let loud = aVerboseKernel()
    let calm = ClientFactory([aVerboseKernel(), aVerboseKernel()])
    let quietModel = AppModel(client: loud, makeClient: calm.factory,
                              preferencesStore: temp.store, verboseLogging: false)
    await quietModel.retryEngine()
    #expect(calm.verboseLoggings == [false], "关着 ⇒ 不许拼那一对（缺省即 normal）")
}

// ---------------------------------------------------------------------------
// 关掉：落盘 → 重启内核 → 回执落在模型上
// ---------------------------------------------------------------------------

@MainActor
@Test func turningTheLevelOffPersistsAndRestartsTheKernel() async throws {
    let temp = try TempVerbosePrefs()
    defer { temp.cleanUp() }
    // 生产里这两个初值都来自 `store.load()`（`AppModel.live()` 读一次、两处用同一份）。
    try temp.store.save(AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: true))

    let old = aVerboseKernel()
    let new = aVerboseKernel()
    let factory = ClientFactory([new])
    let model = AppModel(client: old, makeClient: factory.factory,
                         preferencesStore: temp.store,
                         downloadDir: "/Volumes/Data/交付", verboseLogging: true)
    #expect(model.verboseLoggingChange == nil, "还没改过 ⇒ 没有回执（那一行不该凭空出现）")

    let outcome = await model.changeVerboseLogging(to: false)

    #expect(outcome == .changed(on: false))
    #expect(!model.verboseLogging, "内存里那一格也要跟着走（与盘上一致）")
    #expect(factory.verboseLoggings == [false], "重启出来的内核换成普通档")
    #expect(old.shutdownCount == 1, "旧内核要先被收尾（不许留下孤儿）")
    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/Data/交付",
                                                verboseLogging: false),
            "必须落盘，而且**不许把下载目录那一格抹掉**（两格互不干扰）")
    // 🔴 **回执落在模型上**：设置窗口是随手就会被关掉的东西，而这一格里有"没改成"
    //    那一支（在飞的重启挡住时，盘上已经变了、内核还在旧档上跑）。
    #expect(model.verboseLoggingChange == outcome,
            "回执必须落在模型上 —— 它是设置窗口关掉之后**唯一**还在的那一份")

    model.dismissVerboseLoggingChange()
    #expect(model.verboseLoggingChange == nil, "收起是唯一出口（设置窗口那段那颗 ×）")
}

/// 同值 ⇒ **什么都不做**（与 `choosingTheSameDirectoryDoesNotRestartTheKernel` 同一条）：
/// 没改就不该停掉用户正在跑的任务，但**仍然要有回执**（用户得知道自己那一下有结果）。
@MainActor
@Test func togglingToTheSameValueDoesNotRestartTheKernel() async throws {
    let temp = try TempVerbosePrefs()
    defer { temp.cleanUp() }

    let old = aVerboseKernel()
    let spare = ClientFactory([aVerboseKernel()])
    let model = AppModel(client: old, makeClient: spare.factory,
                         preferencesStore: temp.store, verboseLogging: true)

    let outcome = await model.changeVerboseLogging(to: true)

    #expect(outcome == .unchanged(on: true))
    #expect(spare.madeCount == 0, "同值不许重启内核")
    #expect(old.shutdownCount == 0)
    #expect(model.verboseLoggingChange == outcome, "没改也是一条要说出来的结果")
}

/// 偏好写不下去 ⇒ **中止，一个内核都不许动**（与改目录那条同一纪律）：
/// 否则会出现"内核按新档位在跑、盘上还记着旧档位"的分裂，下次启动**静默**换回去。
@MainActor
@Test func aFailedSaveAbortsTheVerboseToggleWithoutTouchingTheKernel() async throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-model-verbose-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    // 目标路径的父级是一个**普通文件** ⇒ 建目录必失败 ⇒ 写盘必失败。
    let blocker = dir.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: blocker)
    let store = AppPreferencesStore(url: blocker.appendingPathComponent("preferences.json"))

    let old = aVerboseKernel()
    let spare = ClientFactory([aVerboseKernel()])
    let model = AppModel(client: old, makeClient: spare.factory,
                         preferencesStore: store, verboseLogging: true)

    let outcome = await model.changeVerboseLogging(to: false)

    #expect(outcome.isFailure, "存不下去必须报出来（约束 4：不得静默失效）")
    #expect(outcome.headline.contains("详细日志"), "点名的是哪一格：\(outcome.headline)")
    #expect(spare.madeCount == 0, "存不下去就**别重启内核**")
    #expect(old.shutdownCount == 0, "旧内核照旧活着 —— 用户的任务不该因为一次写盘失败而停")
    #expect(model.verboseLogging, "内存里那份也不许改（它必须与盘上一致）")
}

// ---------------------------------------------------------------------------
// 偏好那一格 ⇒ 壳自己日志的档位（**映射本身**，两个方向）
// ---------------------------------------------------------------------------

/// 🔴 **两个方向都要钉住**（规格 §2.4）。
///
/// 为什么必须有它：`AppModel.logLevel(for:)` 早就存在、也早就是纯函数，但**从来没有
/// 一条断言读过它** ⇒ 把 `preferences.verboseLogging ? .verbose : .normal` 的**两支对调**，
/// 全套判据照旧全绿。真机上那是本功能**最坏的那个形状**：用户勾上「详细日志」，
/// **内核**那一档确实变详细了（`coreArguments` 的拼装有判据），而**壳自己那份日志
/// 停在 normal** ⇒ `diag-shell.log` **根本不会被创建**，客户回传的仍然只有内核那一份。
///
/// 判别力：把 `logLevel(for:)` 的两支对调 ⇒ 下面第二条断言红。
/// ⚠️ **它钉住的是映射本身**：`DiagnosticsLog.configure(...)` 那两个**调用点**
///    （`live()` 与 `changeVerboseLogging`）有没有被走到仍然没有判据 ——
///    理由在本文件头注那一段（翻进程静态 / 往真日志里写），只由真机验收那两条守着。
@Test func theVerboseSwitchMapsOntoTheShellsOwnLogLevelBothWays() {
    #expect(AppModel.logLevel(for: .empty) == .normal, "没配过 = 普通档（缺省）")
    #expect(AppModel.logLevel(for: AppPreferences(downloadDir: "/tmp/out", verboseLogging: true))
            == .verbose, "勾上必须是详细档")
    // 反向：**认的不是"有没有这个字段"，是它的取值** —— "恒返回 .verbose" 这种单向
    // 实现只有在这一条上才露馅。
    #expect(AppModel.logLevel(for: AppPreferences(downloadDir: "/tmp/out", verboseLogging: false))
            == .normal, "关回去要跟着落回普通档")
}
