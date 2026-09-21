import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 任务 4b：启动握手的**有界等待**与明示
//
// 背景（内核侧的事实，本任务**不**修它——约束 10 不碰 `core/`）：
//   内核在启动时就 `open()` 默认下载目录 `$HOME/Downloads/Benagen`
//   （`core/src/main.rs:1620` 的默认值 + `:202` 的 `Kernel::new` 立刻 `FileState::load`），
//   而 `~/Downloads` 受 macOS TCC 保护 —— **首次启动**（或授权被重置后）系统会弹一个
//   「想访问"下载"文件夹」的授权框，`open()` 阻塞到用户点按为止。
//   那时内核**根本不在读 stdin**，壳发出去的 `hello` 永远等不到响应。
//
// 本文件里「`FakeCore` 永不返回」就是那个内核的替身：`TestGate` 挂住 `callAsync` 的
//   await，谁都不 resume。判据是**壳在这种内核下仍然有界返回、并且明说发生了什么**，
//   **不是**在真机上复现 TCC 卡死 —— 本机 TCC 已授权、复现不出来，
//   也不该为了"复现"去改系统权限（简报明写）。
//
// ⚠️ 超时值**一律注入测试值**（`kTestHandshakeTimeout`，取值依据见它自己那段注释），
//    不真等生产默认的 5 秒。生产默认值 5 秒由 `AppModel.defaultHandshakeTimeout` 钉住，见
//    `theTimeoutMessageNamesThePermissionDialog`。
//
// ⚠️ 每条断言都必须在实现被改坏时**真的红**：本文件写完做过变异自检，
//    记录见任务 4b 的报告（"改了什么 → 哪条测试红了"）。
// ---------------------------------------------------------------------------

/// 注入给测试的握手超时：足够长（远大于假握手的**墙钟**），又不至于让测试白等太久。
///
/// ⚠️ 这个值原先注入的是 50 毫秒，依据是下面这句话：
///
///      「假客户端的正常握手是**微秒级**（没有 I/O），差三个数量级」
///
///    **那个前提是错的，已被实测证伪，别再照着它推。** 在跑满 300+ 用例的并行测试进程里
///    测同一个 `FakeCore`（`hello` + `get_settings` 两次内存调用）的 `start()` 墙钟：
///
///      · 握手**本身**确实快：p50 ≈ 0.2 ms、p99 ≈ 0.25 ms（"微秒级"这句对**工作量**成立）；
///      · 但**调度**不是微秒级：主 actor 与协作线程池被别的用例占住时，`start()` 会出现
///        约 200 ms 的尾巴（实测最坏 ≈ 210 ms；连跑 3 轮、每轮约 1.5 万次采样，
///        每轮都能采到 1 次左右）。
///
///    50 ms 正好落在这条尾巴**里面** —— 于是"快路径不该被判超时"这条**反向用例**
///    （`aFastHandshakeNeverTimesOut`）被误伤；`startTimeoutLeavesTheOldManifestAlone`
///    在第一次 `start()` 上撞的是同一件事（它随后 `loadDelivery` 没跑成、清单根本没填上）。
///    根因**不是**"假握手慢"，而是"该它跑的时候排不上队"。
///
/// 取 1.0 秒：约 5× 于实测最坏 ≈ 210 ms 的调度尾巴；同时**仍然远小于**各处等待窗
/// （`waitUntil` 5 秒 = 5×、`Task.sleep` 1.5 秒 = 1.5×，都 ≥ 要求的 3 倍）——
/// 真等到 1 秒才触发，说明超时实现本身坏了，而不是本机抖了一下。
///
/// ⚠️ 改这个数字时，**凡是与它耦合的等待窗必须一并改**（本文件里的落点：
///    `waitUntil(..., timeout: 5)` 共 7 处、`aFastHandshakeNeverTimesOut` 里那句
///    `Task.sleep(1.5 秒)`），判据是「等待窗 ≥ 注入超时的 3 倍」。
private let kTestHandshakeTimeout: Double = 1.0

// ---------------------------------------------------------------------------
// 1. 有界返回
// ---------------------------------------------------------------------------

@MainActor
@Test func startTimesOutInsteadOfHangingForever() async {
    // 内核永不回答 `hello` ⇒ `start()` 必须**在超时后返回**，engine 不得停在 `.unknown`。
    let fake = FakeCore()
    let gate = TestGate()          // 永不 release ⇒ `hello` 永远不会回来
    fake.gate = gate
    let model = AppModel(client: fake, handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }

    // ⚠️ 这一条**必须由外部计时**，不能被 `await model.start()` 直接等：
    //    若实现里根本没有超时，直接 await 会把测试自己焊死（整轮测试挂住 = 看不出是哪个断言坏了）。
    //    ⚠️ 窗口 5 秒 = 5× `kTestHandshakeTimeout`（本文件里所有 `waitUntil` 窗口同此口径，
    //       都标着 `timeout: 5`；改注入超时就得一并改，要求是 ≥ 3 倍）。
    let returned = await waitUntil("start() 必须在超时后返回", timeout: 5) {
        model.engine != .unknown
    }

    #expect(returned, "内核不答时 start() 必须有界返回（否则徽标永远停在「正在连接内核…」）")
    #expect(model.engine != .unknown, "超时后必须离开 .unknown")

    gate.release()
    await t.value
}

// ---------------------------------------------------------------------------
// 2. 超时不得伪装成成功
// ---------------------------------------------------------------------------

@MainActor
@Test func aTimedOutStartDoesNotReportSuccess() async {
    let fake = FakeCore()
    let gate = TestGate()
    fake.gate = gate
    let model = AppModel(client: fake, handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .unknown })

    // 逐条排除"装作没事"的几种写法。
    #expect(model.engine != .running, "超时不是「运行中」")
    #expect(model.engine != .notStarted, "超时不是「引擎未启动」——那会让用户以为一切正常")
    #expect(model.engine == .unavailable(AppModel.handshakeTimeoutMessage),
            "超时必须是「不可用 + 一句能照着做的话」，实际 \(model.engine)")
    // 握手没成功 ⇒ 那些只有 `hello`/`get_settings` 才拿得到的值一个都不许落。
    #expect(model.minSplitSizeChoices.isEmpty, "超时不得留下半份握手结果")
    #expect(model.lastCode.isEmpty)

    gate.release()
    await t.value
}

// ---------------------------------------------------------------------------
// 3. 反向用例：正常握手**不得**被误判成超时
// ---------------------------------------------------------------------------

@MainActor
@Test func aFastHandshakeNeverTimesOut() async {
    // 防误报：超时太短（或计时器写反）会把好内核判成坏内核，那比不加超时还糟。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    let model = AppModel(client: fake, handshakeTimeout: kTestHandshakeTimeout)

    await model.start()

    #expect(model.engine == .notStarted, "毫秒级的正常握手不得触发超时，实际 \(model.engine)")
    #expect(model.minSplitSizeChoices == ["1M", "2M", "100M"])
    #expect(model.lastCode == "C24-8")

    // ⚠️ 再等一段**超过注入超时**的时间：超时计时器晚到了也不许追写状态。
    //    这抓的是"赛跑只做了一半"（例如计时器无条件 resume / 没有一次性闸门）。
    //    1.5 秒 = 1.5× `kTestHandshakeTimeout`：必须**严格大于**它才算"计时器晚到"
    //    （取 0.3 秒的那种写法在超时被提到 1 秒之后就不再"晚到"了，等于这条断言失去意义）。
    try? await Task.sleep(nanoseconds: 1_500_000_000)
    #expect(model.engine == .notStarted, "计时器晚到不得把已经完成的握手改写成「无响应」")
}

// ---------------------------------------------------------------------------
// 4. 与任务 3 同口径：失败不清已加载的清单
// ---------------------------------------------------------------------------

@MainActor
@Test func startTimeoutLeavesTheOldManifestAlone() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, handshakeTimeout: kTestHandshakeTimeout)
    await model.start()
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])

    // 内核从这一刻起不再回答任何请求（首次启动卡在授权框上的现场就是这样）
    let gate = TestGate()
    fake.gate = gate
    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .notStarted })

    #expect(model.engine == .unavailable(AppModel.handshakeTimeoutMessage))
    // 超时是"这一次握手没成"，不是"这份清单作废"——客户眼前那棵树不许消失。
    guard case .loaded(let info) = model.loadState else {
        Issue.record("超时不得清掉已加载的清单，实际 \(model.loadState)")
        gate.release()
        await t.value
        return
    }
    #expect(info.code == "AAA-1")
    #expect(model.tree?.flat.map(\.path) == ["a.bin"], "清单必须还在")
    #expect(model.lastCode == "AAA-1", "上次用过的交付码也不许被超时抹掉")

    gate.release()
    await t.value
}

// ---------------------------------------------------------------------------
// 5 / 6. 超时之后的恢复路径：重试（重建客户端），且**先收尾旧的**
// ---------------------------------------------------------------------------

@MainActor
@Test func retryAfterATimeoutSpawnsAFreshClient() async {
    // ⚠️ 为什么"再发一次 hello"是错的：`CoreClient` 内部是一条 FIFO 串行队列（约束 15），
    //    那条永不返回的请求会把队列**永久堵死** —— 新请求排在它后面，永远轮不到。
    //    唯一的恢复路径是**新建客户端 + 新内核进程**（`retryEngine` → `restartKernel`）。
    let stuck = FakeCore()
    let gate = TestGate()
    stuck.gate = gate
    let fresh = FakeCore()
    fresh.stub("hello", Wire.hello)
    fresh.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([fresh])
    let model = AppModel(client: stuck, makeClient: factory.factory,
                         handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .unknown })

    #expect(factory.madeCount == 0, "**超时自己不得重建客户端**：重启是用户的动作（「重试」）")
    #expect(stuck.callCount("hello") == 1, "戳在队列里的那条请求就是唯一的、堵死队列的那条")

    await model.retryEngine()          // 用户点了「重试」

    #expect(factory.madeCount == 1, "重试必须新建客户端（那条队列已经废了）")
    #expect(model.engine == .notStarted, "新内核握手成功 ⇒ 引擎未启动（只是还没有任务）")
    #expect(model.minSplitSizeChoices == ["1M", "2M", "100M"], "新内核的枚举面要接上")

    gate.release()
    await t.value
}

@MainActor
@Test func retryAfterATimeoutShutsDownTheStuckClientFirst() async {
    // ⚠️ 为什么要先 `shutdown()` 旧的：那条旧内核正卡在 `open()` 里。不收尾就丢弃它，
    //    留下的是一个**孤儿内核**；用户随后点了「允许」，它还会醒过来 —— 于是**同时跑着两个内核**
    //    （两个都在 `~/Downloads/Benagen` 上写状态文件）。
    //    `CoreClient.shutdown()` 自带**有界**等待（2 秒后强制收尾）且幂等，所以先调它是安全的。
    let events = EventLog()
    let stuck = FakeCore(events: events, name: "stuck")
    let gate = TestGate()
    stuck.gate = gate
    let fresh = FakeCore(events: events, name: "fresh")
    fresh.stub("hello", Wire.hello)
    fresh.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([fresh], events: events)
    let model = AppModel(client: stuck, makeClient: factory.factory,
                         handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .unknown })

    await model.retryEngine()

    #expect(stuck.shutdownCount == 1, "卡死的旧客户端**必须**被收尾（否则留下孤儿内核）")
    // 顺序是这条测试的重点：`shutdown` 必须**在**新客户端被造出来**之前**发生。
    // 只看计数证明不了这件事 —— 所以走 `EventLog`（跨替身共享的调用顺序账本）。
    #expect(events.entries == ["shutdown:stuck", "spawn"],
            "顺序必须是「先收尾旧的、再造新的」，实际 \(events.entries)")
    #expect(factory.madeCount == 1)

    gate.release()
    await t.value
}

@MainActor
@Test func retryAfterATimeoutShutsTheStuckClientDownOffTheMainThread() async {
    // ⚠️ 复审重要 ②（它推翻了简报里"直接同步调 shutdown()"那条指示）：
    //    卡死这条路上 `CoreClient.shutdown()` **必然走强制收尾**：

    //      2 秒等不到队列（那条请求正卡在 `open()` 里，队列被它占着）
    //      → `channel.close()` → 关 stdin → 等 3 秒 → SIGTERM → 等 2 秒   ⇒ **约 5 秒**
    //    （`CoreClient.shutdown()` 自己的文档写的是最坏约 8 秒。）
    //    而这 5 秒正好落在**用户刚点完「重试」之后** —— 主 actor 被焊住 5 秒 = 窗口冻住，
    //    用户会以为应用又卡死了，那恰恰是本任务要消除的观感。
    //
    //    所以判据不是"调了 shutdown"，而是"**它没有挡住主 actor**"。
    //    这条断言直接测要求本身，且是确定性的（同一个线程上取 `Thread.isMainThread`）。
    let stuck = FakeCore()
    let gate = TestGate()
    stuck.gate = gate
    let fresh = FakeCore()
    fresh.stub("hello", Wire.hello)
    fresh.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([fresh])
    let model = AppModel(client: stuck, makeClient: factory.factory,
                         handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .unknown })

    await model.retryEngine()

    #expect(stuck.shutdownCount == 1, "旧客户端仍然必须被收尾（防孤儿）")
    #expect(stuck.lastShutdownWasOnMainThread == false,
            "`shutdown()` 不得在主 actor 上同步跑：它会阻塞约 5 秒，而那 5 秒正好在用户点完「重试」之后")
    #expect(factory.madeCount == 1, "顺序不变：收尾之后才新建")

    gate.release()
    await t.value
}

@MainActor
@Test func aTimedOutRetryKeepsTheActionableTooltip() async {
    // ⚠️ 复审重要 ①：用户**按 tooltip 的指示**点了「重试」，而内核还卡在同一张授权框上 ——
    //    于是重启后的第二次握手**同样超时**。这一刻界面上唯一能告诉他"该去点什么"的信息，
    //    就是那句 tooltip。`restartKernel` 里若把它包成"内核重启失败：内核无响应（等待超过 5 秒）"，
    //    逐字相等的判别式立刻失效：徽标变成长一倍的那串，**浮层退回原文 —— 指导整段消失**。
    let stuck = FakeCore()
    let gate = TestGate()
    stuck.gate = gate
    let fresh = FakeCore()          // ⚠️ 重启后的内核**也卡住**（这正是这条测试与上一条的差别）
    fresh.gate = gate
    let factory = ClientFactory([fresh])
    let model = AppModel(client: stuck, makeClient: factory.factory,
                         handshakeTimeout: kTestHandshakeTimeout)

    let t = Task { await model.start() }
    #expect(await waitUntil("start() 必须在超时后返回", timeout: 5) { model.engine != .unknown })
    #expect(model.engine == .unavailable(AppModel.handshakeTimeoutMessage))

    await model.retryEngine()
    #expect(factory.madeCount == 1, "重试确实新建了客户端")

    // 超时状态**一个字都没被改名**：
    #expect(model.engine == .unavailable(AppModel.handshakeTimeoutMessage),
            "重启失败**不得**给壳自己那句超时文案加前缀，实际 \(model.engine)")
    #expect(EngineStatusPresentation.tooltip(for: model.engine)
            == EngineStatusPresentation.handshakeTimeoutTooltip,
            "这一刻最需要它：徽标浮层必须仍然是「去看授权框、点「允许」、再点「重试」」那句指导")
    #expect(EngineStatusPresentation.text(for: model.engine) == "内核无响应（等待超过 5 秒）",
            "徽标正文仍是简报压短的那一句")

    // 反向钉住：只有**超时**才享受这条放行 —— 别的原因仍然要被包成"内核重启失败：…"。
    #expect(EngineStatusPresentation.isHandshakeTimeout(AppModel.handshakeTimeoutMessage))
    #expect(!EngineStatusPresentation.isHandshakeTimeout("内核重启失败：\(AppModel.handshakeTimeoutMessage)"),
            "带前缀的那串**不是**超时文案（判别式是逐字相等）")

    gate.release()
    await t.value
}

// ---------------------------------------------------------------------------
// 7. 徽标上那句话必须是**可操作的**
// ---------------------------------------------------------------------------

@MainActor
@Test func theTimeoutMessageNamesThePermissionDialog() {
    // 这条抓的是"文案被改成了没有可操作信息的一句话"（例如「内核无响应」四个字就完了）：
    // 用户看到它时唯一能自救的动作就是**去点那个授权框**，所以文案必须点名它。
    #expect(AppModel.defaultHandshakeTimeout == 5, "生产默认超时就是简报定的 5 秒")

    let engine = AppModel.EngineState.unavailable(AppModel.handshakeTimeoutMessage)
    let text = EngineStatusPresentation.text(for: engine)
    let tip = EngineStatusPresentation.tooltip(for: engine)

    // ① 徽标正文里必须有那句话**本身**（约束 4：失败要出现在界面上，不能只活在悬停浮层里）
    #expect(text == "内核无响应（等待超过 5 秒）", "徽标正文逐字，实际「\(text)」")
    #expect(text.contains("5 秒"), "文案里的秒数与 defaultHandshakeTimeout 必须一致")
    #expect(!text.contains("\n"), "工具栏只有一行")

    // ② 悬停浮层给的是"该干什么"：授权框 + 点「允许」+ 点「重试」
    #expect(tip.contains("授权") || tip.contains("允许"), "文案必须点出系统授权对话框：\(tip)")
    #expect(tip.contains("允许"), "要告诉用户点哪个按钮：\(tip)")
    #expect(tip.contains("重试"), "要给出恢复动作：\(tip)")
    #expect(tip.contains("下载"), "要点出是哪个文件夹的授权：\(tip)")
}
