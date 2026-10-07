import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 测试替身
// ---------------------------------------------------------------------------

/// 测试替身：可编程地吐行、记录收到的行、可模拟 EOF（`toSend` 空 → `readLine` 返回 nil）。
///
/// `closed` 是 `FakeChannel` 自己的实现细节（简报片段里用了它但没声明）——
/// 由 `LineChannel.close()` 被调用来置位，`shutdown` 那条测试靠它证明通道被关了。
final class FakeChannel: LineChannel, @unchecked Sendable {
    var toSend: [String] = []
    private(set) var sent: [String] = []
    private(set) var closed = false
    func writeLine(_ s: String) throws { sent.append(s) }
    func readLine() throws -> String? { toSend.isEmpty ? nil : toSend.removeFirst() }
    func close() { closed = true }
}

/// 慢通道：`readLine` 阻塞在闸门上，用来证明"同一时刻至多一条在飞的请求"（约束 15）。
///
/// 判别方式：`writeLine` 若发现**已经有一条请求在读**（`readInFlight > 0`）就置位 `overlap`。
/// 串行实现里第二条请求连 `writeLine` 都发不出去；换成并发队列才会重叠。
final class SlowChannel: LineChannel, @unchecked Sendable {
    private let gate = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var reads = 0
    private var readInFlight = 0
    private var writes = 0
    private var overlapped = false
    private var isClosed = false

    func writeLine(_ s: String) throws {
        lock.lock(); defer { lock.unlock() }
        if readInFlight > 0 { overlapped = true }
        writes += 1
    }

    func readLine() throws -> String? {
        lock.lock()
        reads += 1
        let n = reads
        readInFlight += 1
        lock.unlock()

        gate.wait()

        lock.lock()
        readInFlight -= 1
        let wasClosed = isClosed
        lock.unlock()
        // 通道被强制关掉之后，卡在读上的那条必须**以 EOF 收场**——与 `ProcessChannel`
        // 的行为一致（子进程被回收 → 读端 EOF），别让替身比真货"好说话"。
        return wasClosed ? nil : #"{"id":\#(n),"ok":true,"result":{}}"#
    }

    func close() {
        lock.lock(); isClosed = true; lock.unlock()
        gate.signal()      // 放行可能正卡在读上的那一条
    }

    func release(_ n: Int) { for _ in 0..<n { gate.signal() } }

    var closed: Bool { lock.lock(); defer { lock.unlock() }; return isClosed }
    var writeCount: Int { lock.lock(); defer { lock.unlock() }; return writes }
    var readCount: Int { lock.lock(); defer { lock.unlock() }; return reads }
    var overlapDetected: Bool { lock.lock(); defer { lock.unlock() }; return overlapped }
}

/// 线程安全的行收集器（stderr 排水用）。
final class LineSink: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []
    func append(_ s: String) { lock.lock(); lines.append(s); lock.unlock() }
    var snapshot: [String] { lock.lock(); defer { lock.unlock() }; return lines }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 有界轮询（不引定时器/轮询进生产代码，这里只是测试的同步手段）。
private func waitFor(_ what: String, within seconds: Double = 5,
                     _ cond: @escaping @Sendable () -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        if cond() { return true }
        try? await Task.sleep(nanoseconds: 5_000_000)
    }
    return cond()
}

private let HELLO_OK_LINE = #"{"id":1,"ok":true,"result":{}}"#

// ---------------------------------------------------------------------------
// 请求-响应配对
// ---------------------------------------------------------------------------

@Test func pairsResponseToItsRequest() throws {
    let ch = FakeChannel()
    ch.toSend = [HELLO_OK_LINE]
    let c = CoreClient(channel: ch)
    _ = try c.callSync("shutdown", .object([:]))
    #expect(ch.sent == [#"{"id":1,"method":"shutdown","params":{}}"#])
}

@Test func idsStartAtOneAndIncrease() throws {
    let ch = FakeChannel()
    ch.toSend = [HELLO_OK_LINE, #"{"id":2,"ok":true,"result":{}}"#]
    let c = CoreClient(channel: ch)
    _ = try c.callSync("hello", .object([:]))
    _ = try c.callSync("hello", .object([:]))
    #expect(ch.sent[0].contains(#""id":1"#))
    #expect(ch.sent[1].contains(#""id":2"#))   // 约束 5：从 1 起，0 不用
}

@Test func idZeroResponseIsAProtocolAlertNotAResult() throws {
    // 内核用 id==0 表示协议级错误（畸形 JSON/超长行/非法 UTF-8）
    let ch = FakeChannel()
    ch.toSend = [#"{"id":0,"ok":false,"error":{"code":"bad_request","message":"请求行不是合法的 UTF-8"}}"#,
                 #"{"id":1,"ok":true,"result":{}}"#]
    let c = CoreClient(channel: ch)
    let r: JSONValue = try c.callSync("hello", .object([:]))   // 不得被 id==0 那条顶掉
    #expect(r == .object([:]))
    #expect(c.protocolAlerts.map(\.code) == ["bad_request"])
}

@Test func idZeroResponseIsNotMistakenForTheReply() throws {
    // 判别力：把 id==0 那条当成"当前请求的响应"→ 上面那条会红；把 id==0 直接丢掉、
    // 且**不**记进 protocolAlerts → `protocolAlerts` 那条会红。这条额外钉住
    // "协议级告警必须带着内核的原文 message 与 code 一路留到界面"（约束 4）。
    let ch = FakeChannel()
    ch.toSend = [#"{"id":0,"ok":false,"error":{"code":"bad_request","message":"请求行超过 8 MiB 上限"}}"#,
                 #"{"id":1,"ok":true,"result":{"ok":true}}"#]
    let c = CoreClient(channel: ch)
    _ = try c.callSync("hello", .object([:]))
    #expect(c.protocolAlerts.count == 1)
    #expect(c.protocolAlerts.first?.code == "bad_request")
    #expect(c.protocolAlerts.first?.message == "请求行超过 8 MiB 上限")
}

@Test func responseWithAnUnexpectedIdIsRejectedNotMispaired() throws {
    // 单飞语义下不可能出现"别人的响应"。真出现就是收发失步，必须吵，
    // 而不是把一条无关的 result 当成自己的（那会让界面显示另一个请求的数据）。
    let ch = FakeChannel()
    ch.toSend = [#"{"id":7,"ok":true,"result":{"别把这条当成我的结果":1}}"#]
    let c = CoreClient(channel: ch)
    var thrown: Error?
    do { _ = try c.callSync("hello", .object([:])) } catch { thrown = error }
    guard case .malformedResponse(let text)? = thrown as? CoreError else {
        Issue.record("应与 id 不符的响应必须报 malformedResponse，实际 \(String(describing: thrown))")
        return
    }
    #expect(text.contains("7") && text.contains("1"))
}

@Test func rpcErrorIsThrownWithCodeAndMessage() throws {
    let ch = FakeChannel()
    ch.toSend = [#"{"id":1,"ok":false,"error":{"code":"engine_not_started","message":"下载引擎尚未启动（先 enqueue 才会起引擎）"}}"#]
    let c = CoreClient(channel: ch)
    #expect(throws: CoreError.rpc(code: "engine_not_started",
                                  message: "下载引擎尚未启动（先 enqueue 才会起引擎）")) {
        try c.callSync("transfer_list", .null)
    }
}

@Test func callSyncAndUnwrapShareTheRpcErrorMessage() throws {
    // `callSync` 的 ok:false 分支与 `RawEnvelope.unwrap` 的 ok:false 分支**必须是同一份代码**。
    // 判别力：这里同时钉死文案，并对两条入口的产物做全等比较——把任何一份改字（或
    // 在 `performCall` 里重新内联一份自己的守卫）这条都会红。
    let body = #"{"id":1,"ok":false,"error":{"code":"no_delivery","message":"还没有加载交付清单（先调 load_delivery）"}}"#

    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(body.utf8))
    var fromUnwrap: Error?
    do { _ = try env.unwrap(JSONValue.self) } catch { fromUnwrap = error }

    let ch = FakeChannel()
    ch.toSend = [body]
    var fromCall: Error?
    do { _ = try CoreClient(channel: ch).callSync("get_tree", .object([:])) } catch { fromCall = error }

    #expect(fromCall as? CoreError == .rpc(code: "no_delivery",
                                           message: "还没有加载交付清单（先调 load_delivery）"))
    #expect(fromUnwrap as? CoreError == fromCall as? CoreError,
            "两条入口必须给出一模一样的错误，实际 \(String(describing: fromCall)) vs \(String(describing: fromUnwrap))")
}

@Test func callSyncAndUnwrapShareTheMissingResultMessage() throws {
    // `ok:true` 却没有 `result`：内核的 `skip_serializing_if` 保证线上不会出现，
    // 但守卫**必须只有一份**——两份复制粘贴的后果是改一份、另一条路径静默保持旧措辞，
    // 而这条文案会显示给客户。
    let body = #"{"id":1,"ok":true}"#

    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(body.utf8))
    var fromUnwrap: Error?
    do { _ = try env.unwrap(JSONValue.self) } catch { fromUnwrap = error }

    let ch = FakeChannel()
    ch.toSend = [body]
    var fromCall: Error?
    do { _ = try CoreClient(channel: ch).callSync("hello", .object([:])) } catch { fromCall = error }

    #expect(fromCall as? CoreError == .malformedResponse("ok:true 的响应里没有 result（id 1）"))
    #expect(fromUnwrap as? CoreError == fromCall as? CoreError,
            "两条入口必须给出一模一样的错误，实际 \(String(describing: fromCall)) vs \(String(describing: fromUnwrap))")
}

@Test func eofSurfacesAsTransportError() throws {
    let ch = FakeChannel()          // toSend 为空 → readLine 返回 nil
    let c = CoreClient(channel: ch)
    #expect(throws: CoreError.transport("内核进程已退出（管道结束）")) {
        try c.callSync("hello", .object([:]))
    }
}

@Test func garbledLineSurfacesAsMalformedResponse() throws {
    let ch = FakeChannel()
    ch.toSend = ["这不是 JSON"]
    let c = CoreClient(channel: ch)
    #expect(throws: CoreError.malformedResponse("内核发来的一行不是合法 JSON：这不是 JSON")) {
        try c.callSync("hello", .object([:]))
    }
}

@Test func garbledLineIsTruncatedTo200Characters() throws {
    // 一整行垃圾不许塞进界面（错误文案会显示给客户）
    let ch = FakeChannel()
    let garbage = String(repeating: "x", count: 5000)
    ch.toSend = [garbage]
    let c = CoreClient(channel: ch)
    var thrown: Error?
    do { _ = try c.callSync("hello", .object([:])) } catch { thrown = error }
    guard case .malformedResponse(let text)? = thrown as? CoreError else {
        Issue.record("必须报 malformedResponse，实际 \(String(describing: thrown))")
        return
    }
    #expect(text == "内核发来的一行不是合法 JSON：" + String(repeating: "x", count: 200))
}

@Test func resultIsCarriedOverVerbatim() throws {
    // `callSync` 把内核发来的 result **原样**交出去：一个字节一个数都不能变。
    // 判别力：内核的 `size`/`completed`/`total` 是 i64，19 位整数是家常便饭
    // （见 `JSONValue` 的类型注释）；任何"顺手转成 Double 再转回来"的实现
    // （或自己写个 `JSONSerialization` 解析）都会在第一条上红。
    let ch = FakeChannel()
    ch.toSend = [#"{"id":1,"ok":true,"result":{"整数":9007199254740993,"小数":2.5,"负数":-0.5,"数组":[1,"二",null],"嵌套":{"a":true}}}"#]
    let c = CoreClient(channel: ch)
    let r = try c.callSync("hello", .object([:]))
    guard case .object(let o) = r else { Issue.record("应是对象，实际 \(r)"); return }
    #expect(o["整数"] == .integer(9007199254740993))   // 走 Double 会变成 9007199254740992
    #expect(o["小数"] == .number(2.5))
    #expect(o["负数"] == .number(-0.5))
    #expect(o["数组"] == .array([.integer(1), .string("二"), .null]))
    #expect(o["嵌套"] == .object(["a": .bool(true)]))

    // ⚠️ **顺带钉住一条既有口径**（`JSONValue.init(from:)` 的探测顺序：整数先于 Double）：
    // 线上写 `2.0` 解出来是 `.integer(2)`，不是 `.number(2.0)`。
    // 所以 `.number` 只会出现在"真有小数部分"的数上（如 `2.5`）——这条断言不是本任务
    // 引入的行为，而是把它写成明账，免得后来人以为 `.number(2.0)` 会出现。
    let ch2 = FakeChannel()
    ch2.toSend = [#"{"id":1,"ok":true,"result":{"整数值的浮点":2.0}}"#]
    let r2 = try CoreClient(channel: ch2).callSync("hello", .object([:]))
    guard case .object(let o2) = r2 else { Issue.record("应是对象，实际 \(r2)"); return }
    #expect(o2["整数值的浮点"] == .integer(2))
}

// ---------------------------------------------------------------------------
// callAsync：只是把 callSync 甩到后台，**不改并发度**（约束 15）
// ---------------------------------------------------------------------------

@Test func callAsyncReturnsTheSameResultAndKeepsTheIdSequence() async throws {
    let ch = FakeChannel()
    ch.toSend = [HELLO_OK_LINE, #"{"id":2,"ok":true,"result":{"n":2}}"#]
    let c = CoreClient(channel: ch)
    let a = try await c.callAsync("hello", .object([:]))
    let b = try await c.callAsync("hello", .object([:]))
    #expect(a == .object([:]))
    #expect(b == .object(["n": .integer(2)]))
    #expect(ch.sent[0].contains(#""id":1"#))
    #expect(ch.sent[1].contains(#""id":2"#))
}

@Test func callAsyncDoesNotRaiseConcurrency() async throws {
    let ch = SlowChannel()
    let c = CoreClient(channel: ch)

    let t1 = Task { try await c.callAsync("hello", .object([:])) }
    let entered = await waitFor("第一条请求进入读") { ch.readCount == 1 }
    #expect(entered, "第一条请求没有在 5 秒内进入读，SlowChannel 的闸门没被等到")

    let t2 = Task { try await c.callAsync("hello", .object([:])) }
    // 串行实现里第二条连 write 都发不出去（第一条还卡在读上）。
    // 若把 `.serial` 队列换成并发队列，这里会看到 2 条 write + overlap。
    try await Task.sleep(nanoseconds: 300_000_000)
    #expect(ch.writeCount == 1, "第二条请求在第一条还没回来时就写出去了——并发度被提高了")
    #expect(ch.overlapDetected == false)

    ch.release(2)
    _ = try await t1.value
    _ = try await t2.value
    #expect(ch.writeCount == 2)
}

// ---------------------------------------------------------------------------
// shutdown
// ---------------------------------------------------------------------------

@Test func shutdownSendsTheRequestThenClosesTheChannel() throws {
    let ch = FakeChannel()
    ch.toSend = [HELLO_OK_LINE]
    let c = CoreClient(channel: ch)
    c.shutdown()
    #expect(ch.sent.first?.contains(#""method":"shutdown""#) == true)
    #expect(ch.closed == true)
}

@Test func shutdownIsIdempotent() throws {
    // 幂等：退出路径可能被调用两次（比如 deinit 与显式 shutdown 撞上）
    let ch = FakeChannel()
    let c = CoreClient(channel: ch)
    c.shutdown()
    c.shutdown()
    ch.close()
    #expect(ch.sent.count == 1)
}

@Test func callsAfterShutdownFailWithATransportError() throws {
    // 关掉之后不能再写：否则用户看到的是 FileHandle 的原始报错（或更糟，静默失败）
    let ch = FakeChannel()
    let c = CoreClient(channel: ch)
    c.shutdown()
    #expect(throws: CoreError.transport("内核连接已关闭（shutdown 已调用）")) {
        try c.callSync("hello", .object([:]))
    }
    #expect(ch.sent.count == 1)   // 失败的那次不得把请求写出去
}

// ---------------------------------------------------------------------------
// shutdown 的**上界**：内核活着但不答时也必须能收尾
// ---------------------------------------------------------------------------
//
// 简报把 `shutdown` 的语义定成"发 shutdown → **响应可能拿不到，超时后照常收尾**"。
// 真实场景：内核正在跑 `load_delivery`（实测约 91.5 秒）/ `verify`，用户点了退出。
// 那时"在飞的请求"占着那条串行队列，收尾路径**根本够不着通道**——若收尾是无界的
// `queue.sync`，退出应用就变成"卡住不动"（约束 4 说的那种"点了没反应"）。

/// `shutdown()` 的收尾上界。构成：等队列 2 s + 关通道（等退出 3 s + SIGTERM 2 s + SIGKILL 1 s）
/// + 排水线程 2 s + 富余。比它长就是"卡住"。
private let SHUTDOWN_BOUND_SECONDS: Double = 12.0

/// 在后台线程上调 `shutdown()` 并**有界**地等它返回。
/// 返回耗时；超时返回 nil（判红）——**不是**让整轮测试挂死。
private func boundedShutdown(_ c: CoreClient, within seconds: Double = SHUTDOWN_BOUND_SECONDS + 5) async -> Double? {
    let sem = DispatchSemaphore(value: 0)
    let start = Date()
    DispatchQueue.global().async { c.shutdown(); sem.signal() }
    let finished = await withCheckedContinuation { (cont: CheckedContinuation<Bool, Never>) in
        DispatchQueue.global().async {
            cont.resume(returning: sem.wait(timeout: .now() + seconds) == .success)
        }
    }
    let elapsed = Date().timeIntervalSince(start)
    guard finished else {
        Issue.record("shutdown() 在 \(seconds) 秒内没返回——收尾路径被一条在飞请求无限期挡住了")
        return nil
    }
    return elapsed
}

@Test func shutdownReturnsWithinItsBoundWhileARequestIsInFlight() async throws {
    let ch = SlowChannel()
    let c = CoreClient(channel: ch)

    let inflightDone = Box<Bool>(false)
    let inflightError = Box<String?>(nil)
    DispatchQueue.global().async {
        do { _ = try c.callSync("load_delivery", .object([:])) }
        catch { inflightError.set(String(describing: error)) }
        inflightDone.set(true)
    }
    let entered = await waitFor("在飞请求进入读") { ch.readCount == 1 }
    #expect(entered, "在飞请求没有进到读，这条测试就失去意义了")

    guard let elapsed = await boundedShutdown(c) else { return }
    #expect(elapsed < SHUTDOWN_BOUND_SECONDS, "shutdown 花了 \(elapsed) 秒")
    #expect(ch.closed, "shutdown 之后通道必须是关的")
    // 幂等：超时之后的第二次调用不得再炸
    c.shutdown()

    // 被卡住的那条请求也必须被**放行**（否则串行队列永远被占着，下一次请求也进不来）
    let freed = await waitFor("在飞请求被放行", within: 5) { inflightDone.value }
    #expect(freed, "强制关通道之后，在飞的请求仍然卡在读上")
    #expect(inflightError.value == String(describing: CoreError.transport("内核进程已退出（管道结束）")),
            "在飞请求应以 EOF 收场，实际 \(inflightError.value ?? "无错误（返回值）")")
}

@Test func shutdownReturnsWithinItsBoundWhenTheRealChildNeverAnswers() async throws {
    // 真子进程（不是替身）：读走一行之后**永不回答**，模拟内核在跑长请求时用户点退出。
    // 用 `/bin/sh` 是因为"内核活着但不答"这件事只有真进程能复现：
    // 替身的 `readLine` 是同步的，构造不出"内核不回但也没死"。
    //
    // ⚠️ **必须观测"请求真的进了子进程"**（同上面那条的 `entered` 守门）：以前这里是
    //    一句固定的 `Task.sleep(500ms)` —— 那是**没有观测的等待**。`/bin/sh` 起不来、
    //    或者 500 ms 内 `writeLine` 还没发生，`shutdown()` 就会在"没有在飞请求"的情况下
    //    秒回，于是 `elapsed < BOUND` 与 `freed` **双双退化成平凡真**，这条测试静默空跑。
    //    所以让子进程**自己报信**：`read line` 读到那一行之后往 stderr 打一个标记，
    //    测试轮询那个标记（stderr 由 `ProcessChannel` 的排水线程送进 `onStderrLine`）。
    let marker = LineSink()
    let ch = try ProcessChannel(executableURL: URL(fileURLWithPath: "/bin/sh"),
                                arguments: ["-c", "read line; echo REQUEST-RECEIVED >&2; sleep 30"],
                                onStderrLine: { marker.append($0) })
    let c = CoreClient(channel: ch)

    let inflightDone = Box<Bool>(false)
    DispatchQueue.global().async {
        _ = try? c.callSync("load_delivery", .object([:]))     // 卡在 read 上
        inflightDone.set(true)
    }
    // 等它把请求写出去，**并且被子进程读走**（`read line` 返回 ⇒ 标记出现在 stderr）。
    // 这时收尾路径才真的是"够不着通道"。
    let entered = await waitFor("在飞请求进入子进程") { marker.snapshot.contains("REQUEST-RECEIVED") }
    #expect(entered, "在飞请求没有进到子进程，这条测试就失去意义了")

    guard let elapsed = await boundedShutdown(c) else { return }
    #expect(elapsed < SHUTDOWN_BOUND_SECONDS, "shutdown 花了 \(elapsed) 秒")

    // 子进程必须真的被回收（否则那条在飞请求会一直卡着读）
    let freed = await waitFor("在飞请求被放行（子进程已回收）", within: 10) { inflightDone.value }
    #expect(freed, "收尾之后在飞请求仍然卡在读上——子进程没被回收")
}

// ---------------------------------------------------------------------------
// 找内核二进制
// ---------------------------------------------------------------------------

/// 造一个"像 .app"的目录并把 benagen-core 放进去（**可执行位**）。
private func makeFakeBundle(withCore: Bool) throws -> (bundle: Bundle, core: URL) {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-test-\(UUID().uuidString)")
    let app = root.appendingPathComponent("BenagenDownloader.app")
    let resources = app.appendingPathComponent("Contents/Resources")
    try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
    let plist = #"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict></dict></plist>"#
    try Data(plist.utf8).write(to: app.appendingPathComponent("Contents/Info.plist"))
    let core = resources.appendingPathComponent("benagen-core")
    if withCore {
        try Data("#!/bin/sh\n".utf8).write(to: core)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: core.path)
    }
    guard let bundle = Bundle(path: app.path) else {
        throw CoreError.transport("测试夹具建不出 Bundle：\(app.path)")
    }
    return (bundle, core)
}

@Test func locateCoreBinaryPrefersTheEnvVar() throws {
    let (bundle, core) = try makeFakeBundle(withCore: true)
    let found = try CoreClient.locateCoreBinary(environment: ["BENAGEN_CORE": core.path],
                                                bundle: bundle)
    #expect(found.path == core.path)
}

@Test func locateCoreBinaryPrefersTheEnvVarOverEverythingElse() throws {
    // 环境变量是为了"临时换一个内核来试"——它必须压过包内那份
    let (bundle, core) = try makeFakeBundle(withCore: true)
    let explicit = bundle.bundleURL.deletingLastPathComponent()
        .appendingPathComponent("别处的-benagen-core")
    try Data("#!/bin/sh\n".utf8).write(to: explicit)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: explicit.path)

    let found = try CoreClient.locateCoreBinary(environment: ["BENAGEN_CORE": explicit.path],
                                                bundle: bundle)
    #expect(found.path == explicit.path)
    #expect(found.path != core.path)
}

@Test func locateCoreBinaryFindsTheBundledCopy() throws {
    // ② 必须是"包内"，且在 ③（仓库里的开发构建）之前——任务 4 的打包脚本产出的就是它
    let (bundle, core) = try makeFakeBundle(withCore: true)
    let found = try CoreClient.locateCoreBinary(environment: [:], bundle: bundle)
    #expect(found.path == core.path)
}

@Test func coreBinaryCandidatePathsAlwaysListThreeEntries() {
    // 三条路径必须**都**写进"找不到"的错误消息里——"点了没反应"最常见的原因就是它，
    // 而约束 4 要求它出现在界面上。
    let withEnv = CoreClient.coreBinaryCandidatePaths(environment: ["BENAGEN_CORE": "/自定义/benagen-core"],
                                                      bundle: .main)
    #expect(withEnv.count == 3)
    #expect(withEnv[0] == "/自定义/benagen-core")
    #expect(withEnv[1].hasSuffix("Contents/Resources/benagen-core"))
    #expect(withEnv[2].hasSuffix("core/target/release/benagen-core"))

    let withoutEnv = CoreClient.coreBinaryCandidatePaths(environment: [:], bundle: .main)
    #expect(withoutEnv.count == 3)
    #expect(withoutEnv[0].contains("BENAGEN_CORE"))       // 未设置也要说清楚找过哪儿
    #expect(withoutEnv[1] == withEnv[1])
    #expect(withoutEnv[2] == withEnv[2])
}

@Test func notFoundErrorListsEveryPathItLookedAt() {
    let err = CoreClient.coreNotFoundError(examined: ["/a/benagen-core", "/b/benagen-core", "/c/benagen-core"])
    guard case .transport(let text) = err else {
        Issue.record("应是 transport，实际 \(err)")
        return
    }
    #expect(text.hasPrefix("找不到内核可执行文件 benagen-core（找过："))
    #expect(text.contains("/a/benagen-core"))
    #expect(text.contains("/b/benagen-core"))
    #expect(text.contains("/c/benagen-core"))
    #expect(text.contains("\n"))    // 三条路径分开写，挤成一行看不清
}

@Test func coreArgumentsArePassedAsArgvNotThroughAShell() {
    #expect(CoreClient.coreArguments(settingsPath: nil, downloadDir: nil, verboseLogging: false) == [])
    #expect(CoreClient.coreArguments(settingsPath: "/s.json", downloadDir: nil, verboseLogging: false)
            == ["--settings", "/s.json"])
    #expect(CoreClient.coreArguments(settingsPath: nil, downloadDir: "/d", verboseLogging: false)
            == ["--download-dir", "/d"])
    #expect(CoreClient.coreArguments(settingsPath: "/s.json", downloadDir: "/d",
                                     verboseLogging: false)
            == ["--download-dir", "/d", "--settings", "/s.json"])
    // 带空格的路径必须原样进 argv（不许过 shell）
    #expect(CoreClient.coreArguments(settingsPath: nil, downloadDir: "/有 空格/目录",
                                     verboseLogging: false)
            == ["--download-dir", "/有 空格/目录"])
}

/// 🔴 **开了才拼那一对；关了什么都不拼**（规格 §2.1 / §5.1 的 argv 行）。
///
/// ⚠️ **关掉时拼 `--log-level normal` 是错的**：内核的缺省就是 normal
///    （`parse_args` 的 `log_level.unwrap_or(Level::Normal)`），多拼一对只是多一处
///    会漂的东西 —— 而且它漂起来的方式是静默的（两种写法在真机上行为一样，
///    只有并排读两份 argv 时才看得出来）。
@Test func theVerboseFlagIsOnlySpelledOutWhenItIsOn() {
    #expect(CoreClient.coreArguments(settingsPath: nil, downloadDir: nil, verboseLogging: false) == [])
    #expect(!CoreClient.coreArguments(settingsPath: nil, downloadDir: "/d", verboseLogging: false)
        .contains("--log-level"),
        "关着的时候一个 `--log-level` 都不许出现（缺省即 normal，多拼一处就是多一处会漂的）")

    #expect(CoreClient.coreArguments(settingsPath: nil, downloadDir: "/d", verboseLogging: true)
            == ["--download-dir", "/d", "--log-level", "verbose"])
    // ⚠️ 拼接次序与 Windows 那份（`windows/shell-core/src/client.rs` 的 `core_arguments`）
    //    **逐个一致**：`--log-level` 排在最后。内核的 `parse_args` 认顺序无关，
    //    但两边"什么时候有哪几格"要对得上 —— 不然并排读两份 argv 时像是壳少拼了一个。
    #expect(CoreClient.coreArguments(settingsPath: "/s.json", downloadDir: "/d", verboseLogging: true)
            == ["--download-dir", "/d", "--settings", "/s.json", "--log-level", "verbose"])
}

// ---------------------------------------------------------------------------
// ProcessChannel：真子进程（不依赖 core/，用 /bin/sh 当对手）
// ---------------------------------------------------------------------------

/// 在后台线程里读一行，**有界**等待。
/// 判别：若 stderr 没人排空，子进程会阻塞在 write 上、这一行永远不来——
/// 那时这里是"20 秒后判红"，而不是整轮测试挂死。
private func readLineWithTimeout(_ ch: ProcessChannel, seconds: Double = 20) -> String? {
    let out = Box<String?>(nil)
    let failure = Box<String?>(nil)
    let sem = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        do { out.set(try ch.readLine()) } catch { failure.set(String(describing: error)) }
        sem.signal()
    }
    let finished = sem.wait(timeout: .now() + seconds) == .success
    #expect(finished, "\(seconds) 秒内没读到那一行——子进程多半阻塞在 stderr 的 write 上了（排水线程没干活）")
    #expect(failure.value == nil, "readLine 抛了错：\(failure.value ?? "")")
    return finished ? out.value : nil
}

final class Box<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var v: T
    init(_ v: T) { self.v = v }
    var value: T { lock.lock(); defer { lock.unlock() }; return v }
    func set(_ nv: T) { lock.lock(); v = nv; lock.unlock() }
}

@Test func processChannelRoundTripsALineAndForwardsStderr() async throws {
    let sink = LineSink()
    let ch = try ProcessChannel(
        executableURL: URL(fileURLWithPath: "/bin/sh"),
        arguments: ["-c", #"echo '诊断一句' >&2; echo '{"id":1,"ok":true,"result":{}}'"#],
        onStderrLine: { sink.append($0) })
    defer { ch.close() }

    #expect(readLineWithTimeout(ch) == HELLO_OK_LINE)

    // stderr 不是"接出来就完了"：读到就必须转成日志行，不许丢（排障信息）
    let forwarded = await waitFor("stderr 那一句被转成日志行") { sink.snapshot.contains("诊断一句") }
    #expect(forwarded, "stderr 的行没有被转发出来，实际收到 \(sink.snapshot)")

    // close() 必须有界返回：真退出不了时也不许把界面卡住
    let start = Date()
    ch.close()
    #expect(Date().timeIntervalSince(start) < 3.0, "close() 卡了 \(Date().timeIntervalSince(start)) 秒")
}

@Test func writingToADeadChildsPipeSurfacesAsATransportErrorAndTheShellSurvives() throws {
    // ⚠️ **这条测试的判别力在于"进程必须活着走到断言"。**
    //    没有 `ProcessChannel.init` 里那句 `signal(SIGPIPE, SIG_IGN)`，下面第一次
    //    写向死管道的 `writeLine` 会拿到 EPIPE，而默认处置是**向进程送 SIGPIPE** ——
    //    测试进程当场消失（**不是断言失败，是整轮 `swift test` 以 141 收场**，
    //    连 `✘` 都打不出来）。所以这条测试的"红"长这样：跑不到断言这一行。
    //
    // 现场（为什么这条路径真的会被走到）：`CoreClient.performCall` **只守卫
    // `requestsClosed`、不守卫"内核已死"** —— 请求 A 在飞时内核死了，A 抛 transport，
    // 而**排在后面的 B 会接着跑、往死管道写**。B 这一写抛出的 `CoreError.transport`
    // 正是 `AppModel.onKernelDeath` 自动重启链路的输入端；进程被信号杀掉时，
    // 那条链路永远等不到它的输入。
    let ch = try ProcessChannel(executableURL: URL(fileURLWithPath: "/bin/sh"),
                               arguments: ["-c", "exit 0"])
    defer { ch.close() }

    // 子进程 `exit 0` 之后它的 stdin 读端就没了。重试若干次而不是靠一次 sleep：
    // 头几次写可能正好落在"子进程还没退出"的窗口里（那时数据进管道缓冲、写成功），
    // 这样不依赖调度时序，也不靠一个猜出来的 sleep 长度。
    var failure: CoreError?
    for _ in 0..<200 {
        do { try ch.writeLine("{}") }
        catch let e as CoreError { failure = e; break }
        catch { Issue.record("抛出来的不是 CoreError：\(error)"); return }
        usleep(20_000)
    }

    guard let failure else {
        Issue.record("写向已死子进程的管道一直没有失败 —— 夹具没造出那个现场"); return
    }
    guard case .transport(let why) = failure else {
        Issue.record("期望 .transport（管道断了是 transport 家族），实际 \(failure)"); return
    }
    #expect(why.hasPrefix("向内核写入请求失败（管道已断）："), "实际：\(why)")

    // ⚠️ 这两句是"进程还活着"这件事的**显式落点**：被 SIGPIPE 杀掉的话，
    //    上面那个 `catch` 之后的任何一行都不会执行（信号不是 Swift 错误，
    //    `do/catch` 接不住它）。断言本身平凡，**到达这里**才是有信息量的那部分。
    var survivedTheDeadPipeWrite = false
    survivedTheDeadPipeWrite = true
    #expect(survivedTheDeadPipeWrite, "进程必须活着走到这里 —— 到不了这一行就是被信号带走了")

    // 而且失败是**可重复**的：不是"第一次侥幸成功、之后静默"。
    #expect(throws: CoreError.self) { try ch.writeLine("{}") }
}

@Test func processChannelDrainsStderrBeyondThePipeBuffer() async throws {
    // macOS 的管道缓冲只有几十 KB。若壳接了一根 Pipe 却不读，内核往 stderr 写满缓冲后
    // 会**阻塞在 write 上**，整个请求-响应循环死锁，症状是"壳卡住不动"、极难归因。
    // 这里让子进程先喷 40000 行 stderr（≈ 2 MB）再回响应：排水线程被改坏时，
    // readLineWithTimeout 会红，而不是静默地慢。
    let sink = LineSink()
    let script = """
    i=0
    while [ $i -lt 40000 ]; do echo "诊断行 $i 占位占位占位占位占位占位占位占位" >&2; i=$((i+1)); done
    echo '{"id":1,"ok":true,"result":{}}'
    """
    let ch = try ProcessChannel(executableURL: URL(fileURLWithPath: "/bin/sh"),
                                arguments: ["-c", script],
                                onStderrLine: { sink.append($0) })
    defer { ch.close() }

    #expect(readLineWithTimeout(ch) == HELLO_OK_LINE)

    let drained = await waitFor("stdout 那一行之后 stderr 继续被排空", within: 20) { sink.snapshot.count > 100 }
    #expect(drained, "stderr 只收到 \(sink.snapshot.count) 行——排水线程没在跑")
}

// ---------------------------------------------------------------------------
// 详细档"每一次壳→内核的调用"那一行（规格 §2.3 的内核侧 C）
//
// 为什么必须有这一条：`performCall` 里那一句记账是**整档新增行为在壳侧的收口**，
// 而同一个缺口在内核那侧已经撞过一次（任务 1 的任务审查：`aria2_call` 那一行 ——
// 那个功能的主角 —— 一条判据都没有，**删掉记账 / 把 `why` 改成无条件 / 把闸门反过来，
// 全套判据仍然全绿**）。Windows 壳那侧的同位缺口由任务 2 补上，这里是第三份。
//
// 🔴 **两条硬约束**（下面那条用例逐条绕开，机制见 `CoreClient.verboseSink`）：
//   ① **不许往真的用户日志目录里写**：`DiagnosticsLog.logVerbose` 走
//      `ShellStorage.directory`（= `~/Library/Application Support/BenagenDownloader/`，
//      **这台机器上就是人类伙伴自己那一份**）—— 用例往那里写 = "跑一次测试"变成
//      "往他的日志里灌测试数据"。
//   ② **不许翻进程级静态**：`DiagnosticsLog.configure(.verbose)` 写的是全局，而同一进程里
//      假设普通档的用例（含 `DiagnosticsLogTests` 的轮转用例、以及**所有**用默认出口的
//      `CoreClient` 用例）会**随调度随机红 / 随机往真日志里写** —— 那种 flaky 比没有判据更坏。
//
// 注入 sink 把两条**同时**绕开：用例既不取那条路径（不碰 ①），也不读不写级别
// （不碰 ②）。残余：`logVerbose` 里那道**闸门本身**（普通档不记）仍然没有判据 ——
// 要判它就得动上面两样中的一样；它与 Windows 那侧的同位缺口是同一个，如实记账。
//
// ⚠️ 顺带说明**生产那一行的接线**（把 `verboseSink` 的初值换成空闭包，
//    本文件全绿）：它没有本机判据，只由**真机验收**那条
//    "打开详细日志 + 跑一次下载 ⇒ `diag-shell.log` 里 `event=kernel_call` 的条数 > 0" 守着。
// ---------------------------------------------------------------------------

/// 一个**记账替身**：把每一行收进内存，**不碰文件系统、不碰全局**。
private final class VerboseRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [(String, [(String, String)])] = []

    /// 注入用的那个出口。
    var sink: @Sendable (String, [(String, String)]) -> Void {
        { event, fields in
            self.lock.lock()
            self.lines.append((event, fields))
            self.lock.unlock()
        }
    }

    /// 读记录用的句柄。
    var recorded: [(String, [(String, String)])] {
        lock.lock(); defer { lock.unlock() }
        return lines
    }
}

/// 取一条记录里某个字段的值（没有这个字段 ⇒ `nil`）。
private func fieldOf(_ line: (String, [(String, String)]), _ key: String) -> String? {
    line.1.first { $0.0 == key }?.1
}

/// 🔴 **每一次壳→内核的调用都落一行；失败那一行带着壳自己给用户看的那句话**。
///
/// 三拍成功 + 一拍失败 ⇒ **四行**，且：
///   · 成功那三条**不带 `why`**（补一个空字段会让"这一行有几个字段"随结果变）；
///   · 失败那一条的 `why` **就是 `AppModel.message(of:)` 给的那句话**（与界面上同一份）。
///
/// 判别力（两条都实测过，读数见任务 5 的报告）：
///   · 把 `performCall` 里那句 `recordKernelCall(…)` 删掉 ⇒ 记录 **0 行**，第一条断言红；
///   · 把 `why` 改成**无条件**追加 ⇒ 成功那三条多出 `why=`，最后那道"不许带 why"红。
@Test func everyKernelCallIsLoggedWithTheShellsOwnWordsOnFailure() throws {
    let ch = FakeChannel()
    ch.toSend = [
        #"{"id":1,"ok":true,"result":{"protocol":1}}"#,
        #"{"id":2,"ok":true,"result":{}}"#,
        #"{"id":3,"ok":true,"result":{}}"#,
        #"{"id":4,"ok":false,"error":{"code":"no_delivery","message":"这一批不存在或已过期"}}"#,
    ]
    let recorder = VerboseRecorder()
    let client = CoreClient(channel: ch)
    // ⚠️ 只换出口：**不给 setter**（那会多出一条只在测试里用的公开面，
    //    还会给非测试构建带出一条 `dead_code` 之类的告警）。
    //    这里能直接赋，是因为 `verboseSink` 是 internal 而本文件是 `@testable import`。
    client.verboseSink = recorder.sink

    _ = try client.callSync("hello", .object(["protocol": .integer(1)]))
    _ = try client.callSync("get_state", .null)
    _ = try client.callSync("get_state", .null)
    let failure = #expect(throws: CoreError.self) { _ = try client.callSync("verify", .null) }

    let lines = recorder.recorded
    #expect(lines.count == 4,
            "每一次调用都要落一行（3 拍成功 + 1 拍失败 = 4 行），实际 \(lines.map(\.0))")
    guard lines.count == 4 else { return }

    for line in lines {
        #expect(line.0 == "kernel_call", "事件名必须是 kernel_call：\(line)")
        #expect(fieldOf(line, "ms") != nil, "每一行都要带耗时：\(line)")
        #expect(fieldOf(line, "ok") != nil, "每一行都要带成败：\(line)")
    }
    #expect(lines.compactMap { fieldOf($0, "method") } == ["hello", "get_state", "get_state", "verify"],
            """
            记的是方法名（参数里可能有交付码之类，一律不许进来）；顺序也要对得上 —— \
            一行一次的对应关系本身是判据
            """)

    // 成功那三条：`ok=true`，且**没有 `why`**
    for line in lines.prefix(3) {
        #expect(fieldOf(line, "ok") == "true", "\(line)")
        #expect(fieldOf(line, "why") == nil,
                """
                成功时**不许**带 `why` —— 补一个空字段会让这一行的字段数随结果变，\
                而按空格切字段读它的下一个人会读到空值：\(line)
                """)
    }
    // 失败那一条：`ok=false`，且 `why` 是壳给用户看的那句话（内核原文，逐字）
    let last = lines[3]
    #expect(fieldOf(last, "ok") == "false", "\(last)")
    #expect(fieldOf(last, "why") == "这一批不存在或已过期",
            """
            失败时 `why` 必须是壳既有那句人话（`AppModel.message(of:)`），\
            不是这里另写的、也不是 `\(String(describing: CoreError.self))` 那种调试形态：\(last)
            """)
    if let failure {
        #expect(fieldOf(last, "why") == AppModel.message(of: failure),
                "`why` 与界面上给用户看的那句必须是**同一份**")
    }
}

/// 造一条**内核回错**的信封（键是 snake_case，与线上逐字相同）。
///
/// ⚠️ 正文必须交给 `CoreJSON.encoder` 去转义：夹具里那个目录名带着 `/` 与中文，
///    手写字符串拼出来的那一行**不是合法 JSON**，`decodeEnvelope` 会先把它拒掉 ——
///    于是用例会以"内核发来的一行不是合法 JSON"红，而它想测的东西一个字节都没走到。
private func errorEnvelopeJSON(id: Int, code: String, message: String) -> String {
    let value = JSONValue.object([
        "id": .integer(Int64(id)),
        "ok": .bool(false),
        "error": .object(["code": .string(code), "message": .string(message)]),
    ])
    let data = (try? CoreJSON.encoder.encode(value)) ?? Data()
    return String(decoding: data, as: UTF8.self)
}

/// 🔴 **交付码与下载目录**绝不许出现在**写下去的那一行**里（隐私，规格 §2.3 B）。
///
/// 为什么必须有它：这一批把**内核**那条拉交付页的路收窄成了"只记分类"
/// （`core/src/delivery.rs` 的 `outcomeCategory`），正是**因为 URL 里含交付码** ——
/// 而同一个码从**另一条**路回来了：内核 404 那句文案是**我们自己**拼的
/// （`清单不存在（404）——请确认交付码是否正确：{url}`），它经 RPC 错误体流进 `why`，
/// 而 `why` 是**逐字**落的。`core/src/main.rs` 的 `preflight` 同样把客户目录名带进来。
/// ⇒ 壳在**它自己知道的那一刻**把这两个串推下来（`AppModel.rememberRedactions`），
/// 由 `CoreClient.setRedactions` 在落盘前抹掉。
///
/// 判别力（两条都实测过，读数见终审修复报告）：
///   · 把 `recordKernelCall` 里那句 `redact(…)` 换回 `AppModel.message(of: error)`（即不抹）⇒
///     本用例两条断言都红 —— 而真机上的表现是**交付码与客户目录名被回传给我们**；
///   · 把 `setRedactions` 的调用点从**发请求之前**挪到**失败之后**（`AppModel`）⇒
///     那一次失败的日志里仍然带着码（`setRedactions` 的文档记着这条）。
///
/// ⚠️ **它钉住的是"抹掉"这一步本身**，不是"壳有没有把那两个串推下来"：
///    推的那一处（`rememberRedactions`）的两个调用点判据不在本文件 —— 如实记账。
@Test func theDeliveryCodeAndDownloadDirectoryNeverReachTheWrittenLine() throws {
    // 形状按真的来：码是 20 位随机码里的那种、目录是客户机上那种带用户名的绝对路径。
    let code = "C24-8ZQ7K3M9P1W5X7T2"
    let dir = "/Users/zhangming/Downloads/Benagen"
    // 内核 404 那句（`core/src/delivery.rs`）—— 码**是 URL 的一个路径段**。
    let notFound = "清单不存在（404）——请确认交付码是否正确："
        + "http://download.benagen.com/\(code)/manifest.json"
    // 内核 preflight 那句（`core/src/main.rs`）—— 客户目录名在里面。
    let preflight = "目标目录不可写（\(dir)）：拒绝访问"

    let ch = FakeChannel()
    ch.toSend = [
        errorEnvelopeJSON(id: 1, code: "delivery_fetch_failed", message: notFound),
        errorEnvelopeJSON(id: 2, code: "preflight_failed", message: preflight),
    ]
    let recorder = VerboseRecorder()
    let client = CoreClient(channel: ch)
    client.verboseSink = recorder.sink
    client.setRedactions([dir, code])

    _ = try? client.callSync("load_delivery", .object(["code": .string(code)]))
    _ = try? client.callSync("enqueue", .null)

    let lines = recorder.recorded
    #expect(lines.count == 2, "两次失败两次记账：\(lines.map(\.0))")
    guard lines.count == 2 else { return }

    let first = fieldOf(lines[0], "why") ?? ""
    #expect(!first.contains(code), "交付码进了日志：\(first)")
    // **诊断价值要留住**：抹掉的是那两个串，不是整句话 —— URL 那个**路径段**
    // 只剩 `[已隐去]`，而主机名、文件名、那句人话都还在（看日志的人仍然看得出
    // "问题出在拉清单这一路上"）。
    #expect(first.contains("清单不存在（404）"), "只抹那两个串，别把话抹没了：\(first)")
    #expect(first.contains("download.benagen.com"), "别把整条 URL 抹掉：\(first)")
    #expect(first.contains("manifest.json"), "别把整条 URL 抹掉：\(first)")
    #expect(first.contains(DiagnosticsLog.redacted), "要看得出来这里被抹过：\(first)")

    let second = fieldOf(lines[1], "why") ?? ""
    #expect(!second.contains(dir), "客户目录名进了日志：\(second)")
    #expect(!second.contains("zhangming"), "目录名的任一段都不该在：\(second)")
    #expect(second.contains("目标目录不可写"), "只抹那两个串，别把话抹没了：\(second)")
    #expect(second.contains("拒绝访问"), "内核原文的其余部分必须逐字留着：\(second)")
}

/// ⚠️ **没被告知要抹什么时，`why` 逐字透传**（这道防线不是"顺手改文案"的地方）。
///
/// 判别力：把 `redact` 改成"无条件把整段 `why` 抹成 `DiagnosticsLog.redacted`" ⇒ 本用例红
/// —— 而真机上那是**最需要的那半句诊断信息没了**（`why` 存在的全部意义就是
/// "引擎为什么没回话"）。它同时钉住"新连接的可抹清单是空的"这件事。
@Test func withoutSecretsTheWhyIsVerbatim() throws {
    let ch = FakeChannel()
    ch.toSend = [#"{"id":1,"ok":false,"error":{"code":"no_delivery","message":"这一批不存在或已过期"}}"#]
    let recorder = VerboseRecorder()
    let client = CoreClient(channel: ch)
    client.verboseSink = recorder.sink

    _ = try? client.callSync("load_delivery", .null)

    let lines = recorder.recorded
    #expect(fieldOf(lines[0], "why") == "这一批不存在或已过期",
            "没有要抹的串时一个字都不许动：\(lines)")
}

