import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 真内核冒烟测试（不是夹具——夹具挡不住"壳与真内核对不上"）
// ---------------------------------------------------------------------------
//
// ⚠️ **这三条用 `.enabled(if:)`，缺二进制时会静默变成"通过"。**
//    这是阶段 A 反复栽过的那一类（裁决 #42、任务 7 次要 2）。计划明确授权了这个例外
//    （`core/target/` 是构建产物、可能不存在），代价是：**它们跑了没有，必须由报告写明**，
//    而"必须驱动真内核"的验收责任在任务 11 的那条**无条件**端到端测试上。
//
// 一次性内核：`--settings` 指到临时目录（内核从**它的父目录**读 `last_code`，
// 所以临时目录同时保证了"上次交付码"是空的），`--download-dir` 也指到临时目录。
// 这几条都不碰网络、不起 aria2（`hello` / `get_settings` / `get_state` / 未知方法都不需要引擎）。

/// 起一个一次性内核（临时 settings / 临时下载目录）。
private func liveClient() throws -> CoreClient {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-core-smoke-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return try CoreClient.live(settingsPath: root.appendingPathComponent("settings.json").path,
                               downloadDir: root.appendingPathComponent("downloads").path)
}

@Test(.enabled(if: CoreClient.devCoreBinaryExists()))
func realCoreAnswersHello() throws {
    let c = try liveClient()
    defer { c.shutdown() }

    let v = try c.callSync("hello", .object(["protocol": .integer(Int64(kProtocolVersion))]))
    let hello = try v.decoded(HelloResult.self)

    #expect(hello.protocolVersion == 1)
    // ⚠️ **100 项，不是 3 项**：任务 1 的夹具把 `min_split_size_choices` 截短过，
    //    这条专门挡"夹具与真内核的偏差"（枚举控件是照它建的，见契约 §2.3）。
    #expect(hello.minSplitSizeChoices.count == 100)
    #expect(hello.minSplitSizeChoices.first == "1M")
    #expect(hello.minSplitSizeChoices.last == "100M")
    #expect(Set(hello.minSplitSizeChoices).count == 100)   // 不得有重复项
    #expect(c.protocolAlerts.isEmpty)
}

@Test(.enabled(if: CoreClient.devCoreBinaryExists()))
func realCoreReportsSettingsAndState() throws {
    let c = try liveClient()
    defer { c.shutdown() }

    let hello = try c.callSync("hello", .object(["protocol": .integer(Int64(kProtocolVersion))]))
        .decoded(HelloResult.self)

    let r = try c.callSync("get_settings", .object([:])).decoded(SettingsResult.self)
    // 七个字段逐个核对区间（`core/src/settings.rs` 的 `validate`，契约 §2.1）。
    // 不写死具体默认值：客户机上可能已经有一份 settings.json，写死会变成一条假测试。
    #expect((1...64).contains(Int(r.settings.parallel)), "-j 越界：\(r.settings.parallel)")
    #expect((1...16).contains(Int(r.settings.connections)), "-x 越界：\(r.settings.connections)")
    #expect((1...16).contains(Int(r.settings.splits)), "-s 越界：\(r.settings.splits)")
    #expect((1...100).contains(Int(r.settings.maxTries)), "--max-tries 越界：\(r.settings.maxTries)")
    #expect((0...60).contains(Int(r.settings.retryWait)), "--retry-wait 越界：\(r.settings.retryWait)")
    #expect(r.settings.limitMbps >= 0, "0 表示不限速，不得为负：\(r.settings.limitMbps)")

    // `-k` 的形状是 `20M` 这类；取值必须在 `1M…100M`，**且必须是 hello 给出的枚举面里的一项**
    // （枚举面与校验边界同源，见内核 `min_split_size_choices_agree_with_settings_validation`）。
    #expect(r.settings.minSplitSize.hasSuffix("M"), "-k 的形状不对：\(r.settings.minSplitSize)")
    let mb = Int(r.settings.minSplitSize.dropLast())
    #expect(mb != nil, "-k 不是 `<数字>M`：\(r.settings.minSplitSize)")
    #expect(mb.map { (1...100).contains($0) } == true, "-k 越界：\(r.settings.minSplitSize)")
    #expect(hello.minSplitSizeChoices.contains(r.settings.minSplitSize),
            "-k 的值 \(r.settings.minSplitSize) 不在 hello 给的枚举面里")

    // 一次性内核（临时 settings 目录）：还没有用过交付码
    #expect(r.lastCode == "")

    // `get_state` 不需要清单：没加载过交付时是空的
    let state = try c.callSync("get_state", .object([:])).decoded(StateFile.self)
    #expect(state.version == 1)
    #expect(state.code == "")
    #expect(state.files.isEmpty)
}

@Test(.enabled(if: CoreClient.devCoreBinaryExists()))
func realCoreUnknownMethodYieldsUnknownMethodCode() throws {
    let c = try liveClient()
    defer { c.shutdown() }

    var thrown: Error?
    do { _ = try c.callSync("definitely_not_a_method", .object([:])) } catch { thrown = error }
    guard case .rpc(let code, let message)? = thrown as? CoreError else {
        Issue.record("内核必须回一条 ok:false 的 rpc 错误，实际 \(String(describing: thrown))")
        return
    }
    // 不是"抛了个错就算"：错误码必须**逐字**是 unknown_method
    #expect(code == ErrorCode.unknownMethod.rawValue)
    #expect(code == "unknown_method")
    #expect(!message.isEmpty)
    #expect(message.contains("definitely_not_a_method"), "消息里没带上是哪个方法：\(message)")

    // 一条业务错误不得把连接带坏：同一条连接上的下一条请求照常
    let hello = try c.callSync("hello", .object(["protocol": .integer(Int64(kProtocolVersion))]))
        .decoded(HelloResult.self)
    #expect(hello.protocolVersion == 1)
}
