import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// F11：壳对内核的读加上时限
//
// 缺口：`CoreClient.callAsync` 是 `queue.async` + continuation，不响应取消；
// 真正的读是 `LineChannel.readLine()`，没有超时；而那条 queue 是串行的
// ⇒ 内核活着但卡住时，一次调用永久挂住并堵死其后所有内核调用。
//
// 本文件的判据覆盖：
//   ① 有期限的读在时限附近返回 `CoreError.timedOut`，不许真挂住；
//   ② 超时之后**把流重新对齐**：排空掉失步的旧应答、返回自己那条，不抛失步错；
//   ③ 排空也有上界；上界到了 ⇒ 通道标记为已死，后续调用立刻失败；
//   ④ timedOut ≠ 内核死亡（不走 onKernelDeath / 自动重启）；
//   ⑤ 半行在超时后留在缓冲里，下一次读拼完整行（不丢不重）。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 测试替身
// ---------------------------------------------------------------------------

/// 可编程的通道：按脚本吐结果（`timeout` / `line` / `eof`），用于造"超时后失步"的现场。
///
/// `readLine()` 与 `readLine(deadline:)` 共用同一段脚本 —— 被测的是 `CoreClient` 的
/// 排空对齐逻辑，不是通道自己的超时机制（那件事由真子进程用例钉住）。
private final class ScriptedChannel: LineChannel, @unchecked Sendable {
    enum Step {
        case timeout
        case line(String)
        case eof
    }

    var steps: [Step] = []
    private var idx = 0
    private(set) var sent: [String] = []
    private(set) var closed = false

    func writeLine(_ s: String) throws { sent.append(s) }

    func readLine() throws -> String? { try nextStep() }

    func readLine(deadline: Duration) throws -> String? { try nextStep() }

    func close() { closed = true }

    private func nextStep() throws -> String? {
        guard idx < steps.count else { return nil }
        let step = steps[idx]
        idx += 1
        switch step {
        case .timeout: throw CoreError.timedOut
        case .line(let s): return s
        case .eof: return nil
        }
    }
}

/// 造一个"永不出数据"的真子进程：读走 stdin 一行之后一直睡，stdout 一个字节都不写。
private func makeChannelThatNeverSpeaks() throws -> ProcessChannel {
    try ProcessChannel(
        executableURL: URL(fileURLWithPath: "/bin/sh"),
        arguments: ["-c", "read line; sleep 30"])
}

// ---------------------------------------------------------------------------
// ① 有期限的读：超时而不是永久挂住
// ---------------------------------------------------------------------------

@Test func aReadThatNeverAnswersTimesOutInsteadOfHangingForever() async throws {
    let ch = try makeChannelThatNeverSpeaks()
    defer { ch.close() }
    let started = Date()
    do {
        _ = try ch.readLine(deadline: .seconds(1))
        Issue.record("本该超时")
    } catch CoreError.timedOut {
        // 对
    }
    // 宽容上界：`< 5` 秒（不是 `< 3`）—— 这条用例在并发套件里跑，CI 负载会让墙钟
    // 抖动；判据要的是"在时限附近返回"而不是"精确 1 秒"，过窄只会造出假红。
    #expect(Date().timeIntervalSince(started) < 5, "必须在时限附近返回，不许真挂住")
}

// ---------------------------------------------------------------------------
// ② 超时之后的排空对齐：丢掉失步的旧应答，返回自己那条
// ---------------------------------------------------------------------------

@Test func afterATimeoutTheDrainSkipsStaleLinesAndReturnsTheMatchingOne() async throws {
    // 喂两条应答：第一条 id 不对（上一次调用的残留），第二条才是等的那个。
    // 排空必须把第一条丢掉、返回第二条 —— 且**不抛失步错**。
    //
    // 判别力：去掉排空、直接 throw ⇒ 红（要么抛 timedOut，要么抛失步错）。
    let ch = ScriptedChannel()
    ch.steps = [
        .timeout,                                           // 第一次读超时（内核慢）
        .line(#"{"id":99,"ok":true,"result":{}}"#),          // 残留的旧应答（id 不对）
        .line(#"{"id":1,"ok":true,"result":{"v":42}}"#),     // 本次应答（迟到但在排空窗口内）
    ]
    let c = CoreClient(channel: ch)
    let result = try c.callSync("hello", .object([:]))
    #expect(result == .object(["v": .integer(42)]),
            "排空应跳过 id=99 的残留、返回 id=1 的应答，实际 \(result)")
}

// ---------------------------------------------------------------------------
// ③ 排空也有上界：到点把通道标记为已死，后续调用立刻失败
// ---------------------------------------------------------------------------

@Test func afterFailedRealignTheChannelIsMarkedDeadAndSubsequentCallsFailFast() async throws {
    // 第一次读超时，排空也超时（内核真的不吭声了）⇒ 通道判死。
    // 后续调用必须**立刻**以"内核没了"那类错误失败 —— 不许留着一条错位的流继续用。
    let ch = ScriptedChannel()
    ch.steps = [
        .timeout,       // 第一次读超时
        .timeout,       // 排空也超时
    ]
    let c = CoreClient(channel: ch)

    // 第一次调用：超时收场
    do {
        _ = try c.callSync("hello", .object([:]))
        Issue.record("本该超时")
    } catch CoreError.timedOut {
        // 对：超时 ≠ 内核死亡，但排空也失败了
    }

    // 第二次调用：通道已死，立刻失败（不写请求、不读响应）
    let sentBefore = ch.sent.count
    do {
        _ = try c.callSync("hello", .object([:]))
        Issue.record("通道判死后本该立刻失败")
    } catch let e as CoreError {
        guard case .transport = e else {
            Issue.record("判死后应是 transport（内核没了那一类），实际 \(e)")
            return
        }
    }
    #expect(ch.sent.count == sentBefore, "通道判死后不该再往管道上写")
}

// ---------------------------------------------------------------------------
// 时限分两档：对齐 Windows A5 修正后的取值（600 秒长调用族）
// ---------------------------------------------------------------------------

@Test func timeoutsAreTieredByMethod() {
    // 默认档 120 秒：绝大多数方法（内核正常最坏是百毫秒级）
    #expect(CoreClient.callTimeout(for: "hello") == .seconds(120))
    #expect(CoreClient.callTimeout(for: "transfer_list") == .seconds(120))
    #expect(CoreClient.callTimeout(for: "task_action") == .seconds(120))

    // 长调用族 600 秒：load_delivery 三段合计（fetch ≤91.5 + clear_engine_batch ≤约40 + plan 30×N）
    // 名单与 Windows 的 LONG_CALL_METHODS 逐字对齐
    #expect(CoreClient.callTimeout(for: "load_delivery") == .seconds(600))
    #expect(CoreClient.callTimeout(for: "list_dir") == .seconds(600))
    #expect(CoreClient.callTimeout(for: "get_tree") == .seconds(600))
    #expect(CoreClient.callTimeout(for: "plan") == .seconds(600))
    #expect(CoreClient.callTimeout(for: "enqueue") == .seconds(600))

    // ⚠️ `clear_engine_batch` / `tree_json` 是 Rust 内部函数，**不是** RPC 方法名
    //    （kernel.rs:1682 / :1863）—— 放进表里是永远匹配不到的死分支。
    //    验证它们落到默认档（= 不存在这两个方法）。
    #expect(CoreClient.callTimeout(for: "clear_engine_batch") == .seconds(120))
    #expect(CoreClient.callTimeout(for: "tree_json") == .seconds(120))
}

// ---------------------------------------------------------------------------
// ④ timedOut ≠ 内核死亡：不走 onKernelDeath（自动重启）
// ---------------------------------------------------------------------------

@MainActor
@Test func timedOutIsNotTreatedAsKernelDeath() async {
    // 🔴 判别力：`CoreError.timedOut` 喂进 `AppModel.absorb` 的 `.timedOut` 分支时，
    //    **不许**走 `onKernelDeath`（那会把 engine 写成 .unavailable 并触发自动重启）。
    //    变异验证：把 `.timedOut` 分支改成 `await onKernelDeath(...)` ⇒ 本用例必红。
    //
    //    断言的是**可观测副作用**（engine 不被改写、重启不被触发），
    //    不是"枚举不等式"那类同义反复。
    let fake = FakeCore()
    fake.stub("load_delivery", throwing: .timedOut)
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    // transport 会走 onKernelDeath ⇒ engine = .unavailable("下载引擎已断开：...")
    // timedOut **不许**碰 engine
    if case .unavailable = model.engine {
        Issue.record("timedOut 不该触发 onKernelDeath（engine 被写成 .unavailable，实际 \(model.engine)）")
    }
    // transport 会走 restartIfAllowed ⇒ factory.madeCount > 0
    #expect(factory.madeCount == 0, "timedOut 不该触发自动重启，实际重启了 \(factory.madeCount) 次")
    // absorb 对 timedOut 返回 .verbatim ⇒ loadState = .failed("内核响应超时")
    // （这证明 absorb 确实被调用了，不是错误被吞掉）
    guard case .failed = model.loadState else {
        Issue.record("loadState 应是 .failed（absorb 返回 .verbatim），实际 \(model.loadState)")
        return
    }
}

// ---------------------------------------------------------------------------
// ⑤ 半行在超时后留在缓冲里，下一次读拼完整行
// ---------------------------------------------------------------------------

@Test func aHalfLineSurvivesATimeoutAndCompletesOnTheNextRead() async throws {
    // 先写半行 `{"id":1,"ok":tr`，等 2 秒（读超时），再补 `ue,"result":{}}\n`。
    // 第一次读：拿到半行但没换行 ⇒ 超时（半行留在 LineReader 的缓冲里）。
    // 第二次读：拿到剩余部分 ⇒ 拼成完整行返回，**不丢前半截、不重后半截**。
    //
    // 判别力：把缓冲在超时时清掉 ⇒ 第二次读只拿到 `ue,...}` 的后半截 ⇒ 本用例红。
    //
    // 用 Python 做无缓冲写：`os.write` 直接写 fd，不受 stdout 块缓冲影响
    // （shell 的 printf/echo 在管道上是块缓冲的，半行可能要等很久才到）。
    let script = """
    import os, time
    os.write(1, b'{"id":1,"ok":tr')
    time.sleep(2)
    os.write(1, b'ue,"result":{}}\\n')
    """
    let ch = try ProcessChannel(
        executableURL: URL(fileURLWithPath: "/usr/bin/python3"),
        arguments: ["-c", script])
    defer { ch.close() }

    // 第一次读：半行在缓冲里，没有换行 ⇒ 超时
    do {
        _ = try ch.readLine(deadline: .seconds(1))
        Issue.record("半行不该当作完整行返回")
    } catch CoreError.timedOut {
        // 对：没有换行，不算一行
    }

    // 第二次读：拿到剩余部分，拼成完整行
    let line = try ch.readLine(deadline: .seconds(5))
    #expect(line == #"{"id":1,"ok":true,"result":{}}"#,
            "半行必须在缓冲里拼完整，实际 \(line ?? "nil")")
}
