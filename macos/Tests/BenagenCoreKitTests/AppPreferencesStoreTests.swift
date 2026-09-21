import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 偏好文件的位置与读写（阶段 E 规格 §2.1，任务 3）
//
// 这一层与 `BatchHistoryStore` **同款**、也是**同样薄**：位置 + "坏就当未配置"。
// 字节搬运在 `JsonFileStore`，解析/判据在 `AppPreferences`（那一份在
// `AppPreferencesTests` 里）。
//
// ⚠️⚠️ 安全红线：本文件**没有任何一处**构造指向 Application Support 的 store。
//      `~/Library/Application Support/BenagenDownloader/preferences.json` 一旦存在，
//      就是**人类伙伴真实在用**的配置 —— 测试只碰临时目录。
// ---------------------------------------------------------------------------

/// 一个用完就删的临时目录（每次一个新的名字，用例之间不共享状态）。
private struct TempDir {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-prefs-store-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    var store: AppPreferencesStore { AppPreferencesStore(url: url.appendingPathComponent("preferences.json")) }

    func cleanUp() { try? FileManager.default.removeItem(at: url) }
}

// ---------------------------------------------------------------------------
// 位置
// ---------------------------------------------------------------------------

@Test func thePreferencesFileSitsNextToTheHistoryButIsAnotherFile() {
    // 规格 §2.1：与历史**同目录、另一个文件**。只构造路径、不碰盘 ——
    // "有没有那个文件"是运行期才知道的事。
    #expect(AppPreferencesStore.defaultURL.lastPathComponent == "preferences.json")
    #expect(AppPreferencesStore.defaultURL.deletingLastPathComponent() == ShellStorage.directory)
    #expect(AppPreferencesStore.defaultURL != BatchHistoryStore.defaultURL,
            "壳的偏好不塞进历史文件里 —— 两者的格式与生命周期由各自负责")
}

// ---------------------------------------------------------------------------
// 读：坏就"当未配置"（E-1）
// ---------------------------------------------------------------------------

@Test func aMissingFileMeansUnconfiguredAndNeverThrows() throws {
    // 最常见的那条路径：**第一次运行**。它不是错误。
    let temp = try TempDir()
    defer { temp.cleanUp() }

    #expect(temp.store.load() == .empty,
            "读不出来 ⇒ 当未配置（E-1）。`load()` 连 throws 都没有 —— '失败'这条路不存在")
    #expect(DownloadDirectory.argument(for: temp.store.load()) == nil,
            "而且未配置 ⇒ 不传 `--download-dir`（E-5）")
}

@Test func aGarbageFileMeansUnconfiguredAndTheAppStillStarts() throws {
    // E-1 的现场：盘上是一段乱码 ⇒ **当未配置**并继续启动，绝不把应用拦在启动之外。
    // （"应用照常启动"这条只能人眼验，见 README 的手工验收一节；
    //   这里钉的是它**唯一的机器可判据**：`load()` 不抛、且返回 `.empty`。）
    let temp = try TempDir()
    defer { temp.cleanUp() }
    try Data([0x00, 0x01, 0xFF, 0xFE]).write(to: temp.store.url)

    #expect(temp.store.load() == .empty)
}

@Test func anUnknownVersionMeansUnconfigured() throws {
    let temp = try TempDir()
    defer { temp.cleanUp() }
    try Data(#"{"version":99,"download_dir":"/tmp/别的版本"}"#.utf8).write(to: temp.store.url)

    #expect(temp.store.load() == .empty, "不认识的 version ⇒ 当未配置（给未来的自己留的那条路）")
}

// ---------------------------------------------------------------------------
// 写：原子（E-2）、失败要能被上报
// ---------------------------------------------------------------------------

@Test func saveThenLoadRoundTripsTheDirectory() throws {
    let temp = try TempDir()
    defer { temp.cleanUp() }

    try temp.store.save(AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录"))
    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录"))
}

@Test func saveCreatesTheMissingDirectoryOnFirstRun() throws {
    let temp = try TempDir()
    defer { temp.cleanUp() }
    let nested = AppPreferencesStore(url: temp.url
        .appendingPathComponent("深一层/再深一层/preferences.json"))

    try nested.save(AppPreferences(downloadDir: "/Volumes/Data"))

    #expect(FileManager.default.fileExists(atPath: nested.url.path))
}

@Test func aFailedSaveThrowsAndLeavesThePreviousPreferenceIntact() throws {
    // E-2 的判据（"半个文件比没有文件更糟"）在 `JsonFileStoreTests` 里；这里钉的是
    // **store 不吞错**这一半：写不下去必须抛出去，让调用方有机会说一句
    //（一次"以为存上了"的静默失败，用户要等到重启之后才发现 —— 而那时已经无从归因）。
    let temp = try TempDir()
    defer { temp.cleanUp() }
    try temp.store.save(AppPreferences(downloadDir: "/Volumes/旧"))

    // 把临时文件名占成一个**目录** ⇒ 这次写必失败（同 `JsonFileStoreTests` 的手法）。
    let blocker = temp.url.appendingPathComponent(JsonFileStore.temporaryName(for: temp.store.url))
    try FileManager.default.createDirectory(at: blocker, withIntermediateDirectories: true)

    #expect(throws: (any Error).self) {
        try temp.store.save(AppPreferences(downloadDir: "/Volumes/新"))
    }
    #expect(temp.store.load() == AppPreferences(downloadDir: "/Volumes/旧"),
            "写失败之后盘上那份必须**逐字还是旧的**")
}
