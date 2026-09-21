import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 历史文件的位置与"坏就当空"（阶段 E 规格 §1.1 / §1.3，任务 1）
//
// ⚠️⚠️ 安全红线（同 `JsonFileStoreTests`）：下面所有**碰盘**的用例都指向临时目录。
//      唯一一条读默认位置形状的用例（`theStoreLivesNextTo…`）**只构造 URL、不读不写**
//      —— 人类伙伴真实的 `~/Library/Application Support/BenagenDownloader/` 里
//      放着他正在用的 `settings.json` 与 `last_code`。
// ---------------------------------------------------------------------------

private func makeTempDir() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-shell-store-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

private func withTempDir(_ body: (URL) throws -> Void) throws {
    let dir = try makeTempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    try body(dir)
}

// ---------------------------------------------------------------------------
// 位置
// ---------------------------------------------------------------------------

@Test func theHistoryFileLivesNextToTheKernelsSettingsFileButIsAnotherFile() {
    // 规格 §1.1：`~/Library/Application Support/BenagenDownloader/history.json`，
    // 与内核的 `settings.json` / `last_code` **同目录、不同文件**（两者的生命周期与
    // 格式由各自负责 —— 壳的东西不塞进内核的文件里）。
    //
    // ⚠️ 这一条**不碰盘**：只比路径的形状。
    let url = BatchHistoryStore.defaultURL

    #expect(url.lastPathComponent == "history.json")
    #expect(url.deletingLastPathComponent().lastPathComponent == "BenagenDownloader")
    #expect(url.deletingLastPathComponent().deletingLastPathComponent().lastPathComponent
            == "Application Support")
    // 内核的 `settings::default_path()`（`core/src/settings.rs:198-207`）用的是 `$HOME`；
    // 这里钉的是"两边指同一个目录"这条契约的另一半。
    #expect(url.path.hasSuffix("/Library/Application Support/BenagenDownloader/history.json"),
            "实际 \(url.path)")
    #expect(url.path != "/history.json", "HOME 取不到时也不能把文件写到盘的根上")
}

// ---------------------------------------------------------------------------
// 读：坏 ⇒ 空历史（E-1）
// ---------------------------------------------------------------------------

@Test func loadingAMissingFileIsAnEmptyHistory() throws {
    // 第一次运行：文件不存在是**正常态**，不是错误。
    try withTempDir { dir in
        let store = BatchHistoryStore(url: dir.appendingPathComponent("history.json"))
        #expect(store.load().isEmpty)
    }
}

@Test func loadingGarbageIsAnEmptyHistoryAndNeverThrows() throws {
    // E-1 的**底线**：文件存在但读不动 ⇒ 当空历史继续，绝不让应用起不来。
    // 判据是 `load()` **没有 `throws`**（它没有"失败"这条路径）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("history.json")
        let store = BatchHistoryStore(url: url)

        for garbage in ["", "not json at all", "[]", #"{"version":99,"entries":[]}"#,
                        #"{"version":1,"entries":{}}"#, "\u{0}\u{1}\u{FFFD}"] {
            try Data(garbage.utf8).write(to: url)
            #expect(store.load().isEmpty, "坏文件 \(garbage.prefix(20)) 应被当作空历史")
        }
    }
}

@Test func loadingATruncatedFileIsAnEmptyHistory() throws {
    // E-2 的动机现场：假设某次写盘被中断（断电、进程被杀），文件会是半截 JSON。
    // 那种文件必须被当成空历史（**不是**崩溃源）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("history.json")
        let store = BatchHistoryStore(url: url)
        try store.save(BatchHistory.empty.recording(code: "AAA-1", at: Date()))

        let full = try #require(try? Data(contentsOf: url))
        try full.prefix(full.count / 2).write(to: url)   // 截一半

        #expect(store.load().isEmpty)
    }
}

// ---------------------------------------------------------------------------
// 写 + 往返
// ---------------------------------------------------------------------------

@Test func saveThenLoadRoundTripsThroughTheFilesystem() throws {
    try withTempDir { dir in
        let store = BatchHistoryStore(url: dir.appendingPathComponent("history.json"))
        let h = BatchHistory.empty
            .recording(code: "AAA-1", baseURL: "http://dl.example", at: Date(timeIntervalSince1970: 1_700_000_000))
            .settingNote("客户张三", forCode: "AAA-1")

        try store.save(h)

        #expect(store.load() == h)
    }
}

@Test func saveCreatesTheDirectoryOnFirstRun() throws {
    // 首次运行时整条 `…/BenagenDownloader/` 都可能不存在（内核还没写过 settings.json）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("a/b/history.json")
        try BatchHistoryStore(url: url).save(.empty.recording(code: "AAA-1", at: Date()))

        #expect(FileManager.default.fileExists(atPath: url.path))
    }
}

@Test func aFailedSaveIsReportedToTheCaller() throws {
    // 写失败必须**抛得出来**（`JsonFileStoreTests` 已钉住底层；这里钉的是这一层没把它吞掉）
    // —— 不能让人以为"存上了"。
    try withTempDir { dir in
        let file = dir.appendingPathComponent("我是一个文件")
        try Data("x".utf8).write(to: file)

        #expect(throws: (any Error).self) {
            try BatchHistoryStore(url: file.appendingPathComponent("history.json")).save(.empty)
        }
    }
}
