import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 壳的**第一次写盘**（阶段 E 规格 §1.1 / §2.1，任务 1）
//
// ⚠️⚠️ 安全红线：这些测试**全部**落在 `FileManager.temporaryDirectory` 下的临时目录里。
//      `~/Library/Application Support/BenagenDownloader/` 里放着人类伙伴**真实在用的**
//      `settings.json` 与 `last_code` —— 覆盖它们 = 破坏他的现场。
//      所以 `JsonFileStore` / `BatchHistoryStore` **都接受一个 URL 参数**（默认值才指向
//      Application Support），测试拿到的每一个句柄都指向临时目录。
//      这个文件里**没有任何一处**构造默认 URL 的实例。
// ---------------------------------------------------------------------------

/// 造一个**新的**临时目录（每次调用一个独立名字，测试之间不共享状态）。
///
/// 不 `try?`、不吞错：临时目录建不出来是环境坏了，测试应当**红**，不是静默跳过。
private func makeTempDir() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-shell-store-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

/// 跑一段用到临时目录的测试，**结束时删掉它**（临时目录也是要收拾的）。
private func withTempDir(_ body: (URL) throws -> Void) throws {
    let dir = try makeTempDir()
    defer { try? FileManager.default.removeItem(at: dir) }
    try body(dir)
}

// ---------------------------------------------------------------------------
// 读：失败 ⇒ `nil`（E-1 的"当空"由调用方按语义处理）
// ---------------------------------------------------------------------------

@Test func readingAMissingFileReturnsNilInsteadOfThrowing() throws {
    // E-1 的**第一道**：读不出来**不是**异常路径，它是最常见的路径（第一次运行）。
    try withTempDir { dir in
        let store = JsonFileStore()
        #expect(store.read(dir.appendingPathComponent("没有这个文件.json")) == nil)
    }
}

@Test func readingReturnsTheBytesVerbatimWithNoJSONAssumptions() throws {
    // `JsonFileStore` 是**通用**的字节存取，不是 JSON 校验器：判断"这份 JSON 还能不能用"
    // 是 `BatchHistory` / `AppPreferences` 的事（它们的"坏就当归零"各有各的语义）。
    // 在这里替它们判会把"一份坏文件"变成 `nil`（= "没有文件"），两件事就再也分不开了。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("x.json")
        let store = JsonFileStore()
        let garbage = Data([0x00, 0x01, 0xFF, 0xFE])
        try store.write(garbage, to: url)

        #expect(store.read(url) == garbage)
    }
}

// ---------------------------------------------------------------------------
// 写：原子（E-2）
// ---------------------------------------------------------------------------

@Test func writeCreatesTheMissingDirectoriesOnFirstRun() throws {
    // 首次运行时 `~/Library/Application Support/BenagenDownloader/` 整条路径都可能不存在
    // —— 写盘不能因此失败（那会让"第一次运行"这个最常见的情形变成错误）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("nested/更深一层/history.json")
        try JsonFileStore().write(Data("{}".utf8), to: url)

        #expect(FileManager.default.fileExists(atPath: url.path))
    }
}

@Test func writeReplacesTheWholeFileAndLeavesNoTemporaryFileBehind() throws {
    // 原子写的可观测面：写完之后目标文件是**完整的新内容**（不是追加、也不是半个），
    // 而且同目录里**不留**临时文件（那会是一个永远不会被清理的垃圾）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("history.json")
        let store = JsonFileStore()

        try store.write(Data("第一版".utf8), to: url)
        try store.write(Data("第二版（更长一些）".utf8), to: url)

        #expect(store.read(url) == Data("第二版（更长一些）".utf8))
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: dir.path)
        #expect(leftovers == ["history.json"], "同目录里只该有目标文件，实际 \(leftovers)")
    }
}

@Test func aFailedWriteThrowsAndLeavesThePreviousFileIntact() throws {
    // E-2 的**判据**（"半个文件比没有文件更糟"）：写失败时**旧文件逐字还在**，
    // 而不是被截成半截。这里把临时文件名占成一个**目录**来制造失败
    // （临时文件与目标同目录、名字由 `JsonFileStore.temporaryName(for:)` 一处定义）。
    try withTempDir { dir in
        let url = dir.appendingPathComponent("history.json")
        let store = JsonFileStore()
        try store.write(Data("完整的第一版".utf8), to: url)

        let blocker = dir.appendingPathComponent(JsonFileStore.temporaryName(for: url))
        try FileManager.default.createDirectory(at: blocker, withIntermediateDirectories: true)

        #expect(throws: (any Error).self) {
            try store.write(Data("第二版（这一版的长度与上一版不同）".utf8), to: url)
        }
        #expect(store.read(url) == Data("完整的第一版".utf8),
                "写失败之后目标文件必须**逐字还是旧的** —— 半个文件比没有文件更糟")
    }
}

@Test func writeToADirectoryThatCannotBeCreatedThrows() throws {
    // 写失败**必须能被上报**（简报步骤 4）：不能让调用方以为"存上了"。
    // 这里让目标路径的**父级是一个普通文件** —— 建目录必然失败。
    try withTempDir { dir in
        let file = dir.appendingPathComponent("我是一个文件")
        try Data("x".utf8).write(to: file)

        #expect(throws: (any Error).self) {
            try JsonFileStore().write(Data("{}".utf8), to: file.appendingPathComponent("history.json"))
        }
    }
}
