import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// AppModel 的测试
//
// 这个文件里的夹具全部是**内核会发的那种 JSON 文本**（键是 snake_case），
// 经 `CoreJSON.decoder` 解成 `JSONValue` 再喂给 `FakeCore` —— 走的是与真内核
// 同一条解码口径。手搓 `JSONValue` 字面量会让"壳解不解得动内核的输出"这件事
// 在测试里凭空消失。
//
// ⚠️ `FakeCore` **不替内核做串行**（真 `CoreClient` 有一条串行队列，那是任务 2
//    已经钉住的东西）。它按调用原样并发执行，好让"壳有没有单飞"变成可观测的。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 测试替身
// ---------------------------------------------------------------------------

/// 一道闸门：让一条 `callAsync` 停在半路，用来制造"上一拍还没回来"的现场。
///
/// ⚠️ 挂住的是 `callAsync` 的 **await**（不是阻塞主 actor）——否则测试自己就把
/// 主 actor 焊死了，第二条 `pollTick` 根本没机会跑，"跳拍"就测不出来。
final class TestGate: @unchecked Sendable {
    private let lock = NSLock()
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var held = true

    func wait() async {
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            lock.lock()
            if held {
                waiters.append(c)
                lock.unlock()
            } else {
                lock.unlock()
                c.resume()
            }
        }
    }

    func release() {
        lock.lock()
        held = false
        let pending = waiters
        waiters = []
        lock.unlock()
        for c in pending { c.resume() }
    }
}

/// **跨替身共享**的调用顺序账本。
///
/// 为什么需要它（任务 4b）：`FakeCore.shutdownCount` 只能证明"`shutdown()` 被调过"，
/// 证明不了"**在**新客户端被造出来**之前**调过" —— 而任务 4b 要的恰恰是那个顺序
/// （先收尾卡死的旧内核、再新建，否则留下孤儿内核、甚至同时跑起两个内核）。
/// 把两个替身的动作记进同一条流水，顺序就变成可断言的了。
final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [String] = []

    func record(_ event: String) {
        lock.lock(); events.append(event); lock.unlock()
    }

    var entries: [String] {
        lock.lock(); defer { lock.unlock() }
        return events
    }
}

/// 测试替身：可编程返回结果或抛错，**并记录调用顺序与并发度**。
final class FakeCore: CoreCalling, @unchecked Sendable {
    private let lock = NSLock()
    private var stubs: [String: Result<JSONValue, CoreError>] = [:]
    private var fallback: Result<JSONValue, CoreError> = .success(.object([:]))
    private var log: [(method: String, params: JSONValue)] = []
    private var inflight = 0
    private var peak = 0
    private var syncCalls = 0
    private var alerts: [ErrorBody] = []
    private var shutdowns = 0

    /// 非 nil 时 `callAsync` 记完账就停在这里，直到 `release()`。
    var gate: TestGate?

    /// 调用顺序账本与自己在账本上的名字（任务 4b 用；不传就是"不上账"）。
    private let events: EventLog?
    private let name: String

    init(events: EventLog? = nil, name: String = "") {
        self.events = events
        self.name = name
    }

    // MARK: 编程

    /// 用**线上 JSON 文本**打一条桩（键是 snake_case，与内核一致）。
    func stub(_ method: String, _ json: String) {
        guard let value = try? CoreJSON.decoder.decode(JSONValue.self, from: Data(json.utf8)) else {
            Issue.record("夹具不是合法 JSON：\(json)")
            return
        }
        lock.lock(); stubs[method] = .success(value); lock.unlock()
    }

    func stub(_ method: String, throwing error: CoreError) {
        lock.lock(); stubs[method] = .failure(error); lock.unlock()
    }

    /// 所有没有单独打桩的方法都失败（模拟"内核整个没了"）。
    func failEverything(with error: CoreError) {
        lock.lock(); fallback = .failure(error); stubs = [:]; lock.unlock()
    }

    func setAlerts(_ list: [ErrorBody]) {
        lock.lock(); alerts = list; lock.unlock()
    }

    // MARK: 观察

    func callCount(_ method: String) -> Int {
        lock.lock(); defer { lock.unlock() }
        return log.filter { $0.method == method }.count
    }

    /// 第 `occurrence` 次调用该方法时发的 `params`（默认第一次）。
    func params(_ method: String, occurrence: Int = 0) -> JSONValue? {
        lock.lock(); defer { lock.unlock() }
        let hits = log.filter { $0.method == method }
        guard hits.indices.contains(occurrence) else { return nil }
        return hits[occurrence].params
    }

    var maxInFlight: Int {
        lock.lock(); defer { lock.unlock() }
        return peak
    }

    var inFlight: Int {
        lock.lock(); defer { lock.unlock() }
        return inflight
    }

    var syncCallCount: Int {
        lock.lock(); defer { lock.unlock() }
        return syncCalls
    }

    var shutdownCount: Int {
        lock.lock(); defer { lock.unlock() }
        return shutdowns
    }

    // MARK: CoreCalling

    /// ⚠️ **故意不挂闸门**：壳一旦（违反裁定 ①）在主 actor 上同步调它，测试会**记账**
    /// 而不是把自己焊死——`syncCallCount` 就是那条断言的落点。
    @discardableResult
    func callSync(_ method: String, _ params: JSONValue) throws -> JSONValue {
        lock.lock()
        syncCalls += 1
        log.append((method, params))
        let r = stubs[method] ?? fallback
        lock.unlock()
        return try r.get()
    }

    @discardableResult
    func callAsync(_ method: String, _ params: JSONValue) async throws -> JSONValue {
        // `withLock` 而不是 `lock()/unlock()`：Swift 6 不允许在**异步上下文里**直接
        // 调那两个（`NS_SWIFT_UNAVAILABLE_FROM_ASYNC`）。锁本身也不会跨 await 持有 ——
        // 下面挂闸门之前先把锁放掉。
        let (r, g): (Result<JSONValue, CoreError>, TestGate?) = lock.withLock {
            log.append((method, params))
            inflight += 1
            peak = max(peak, inflight)
            return (stubs[method] ?? fallback, gate)
        }

        if let g { await g.wait() }

        lock.withLock { inflight -= 1 }
        return try r.get()
    }

    var protocolAlerts: [ErrorBody] {
        lock.lock(); defer { lock.unlock() }
        return alerts
    }

    /// 每次 `shutdown()` 是不是跑在**主线程**上（任务 4b 复审重要 ②）。
    ///
    /// ⚠️ 记下来由测试断言，而不是在 `shutdown()` 里直接 `#expect(!Thread.isMainThread)`：
    ///    `AppModel.shutdown()`（应用退出那条路，约束 17 的唯一同步例外）**本来就要求**
    ///    在主 actor 上同步调 —— 一个无条件的就地断言会把那条合法的路也判红。
    ///    这条记录是**确定性**的（同一个线程上取 `Thread.isMainThread`），不会 flaky。
    private var shutdownThreads: [Bool] = []

    func shutdown() {
        let onMainThread = Thread.isMainThread
        lock.lock(); shutdowns += 1; shutdownThreads.append(onMainThread); lock.unlock()
        events?.record("shutdown:\(name)")
    }

    /// 最近一次 `shutdown()` 是否发生在主线程上（nil = 还没调过）。
    var lastShutdownWasOnMainThread: Bool? {
        lock.lock(); defer { lock.unlock() }
        return shutdownThreads.last
    }

    /// 每一次 `shutdown()` 是否发生在主线程上，按调用顺序。
    var shutdownWasOnMainThread: [Bool] {
        lock.lock(); defer { lock.unlock() }
        return shutdownThreads
    }
}

/// 测试用的"内核工厂"：按队列发放替身，**并记录被调用了几次**（防重入的判据）。
final class ClientFactory: @unchecked Sendable {
    private let lock = NSLock()
    private var pending: [any CoreCalling]
    private var made = 0
    /// 每一次 `make(downloadDir:)` **被要求**的 `--download-dir` 值，按调用顺序。
    ///
    /// ⚠️ 记的是**被要求**的那一格参数（`nil` = 不传），**在发放之前**就记下来 ——
    ///    "壳要求新内核用什么目录"与"工厂还有没有替身"是两件事，后者不该让前者隐形。
    ///    这是阶段 E 任务 3 的判据：**重启出来的内核拿到的是新目录**
    ///    （`AppModelDownloadDirTests`）。
    private var requestedDirs: [String?] = []
    /// 调用顺序账本（任务 4b）：`spawn` 这一笔记在**发放**的那一刻，
    /// 所以它相对 `shutdown:<名字>` 的先后就是"先收尾旧的、再造新的"的判据。
    private let events: EventLog?

    init(_ clients: [any CoreCalling], events: EventLog? = nil) {
        pending = clients
        self.events = events
    }

    var madeCount: Int {
        lock.lock(); defer { lock.unlock() }
        return made
    }

    /// 每一次被要求的 `--download-dir`（`nil` = 不传那个 flag）。
    var downloadDirs: [String?] {
        lock.lock(); defer { lock.unlock() }
        return requestedDirs
    }

    func make(downloadDir: String?) throws -> any CoreCalling {
        lock.lock(); defer { lock.unlock() }
        requestedDirs.append(downloadDir)
        guard !pending.isEmpty else {
            // 走到这里 ⇔ 壳多重启了一次 —— 用一条明确的错误把那次多选题变成必答题。
            throw CoreError.transport("测试里没有更多内核替身了（工厂被多调了一次？）")
        }
        made += 1
        events?.record("spawn")
        return pending.removeFirst()
    }

    /// 直接当 `makeClient` 用（`AppModel` 的工厂签名带一格里内核的 `--download-dir`）。
    var factory: @Sendable (String?) throws -> any CoreCalling { { dir in try self.make(downloadDir: dir) } }
}

// ---------------------------------------------------------------------------
// 线上夹具
// ---------------------------------------------------------------------------

/// ⚠️ 不是 `private`：`AppModelTimeoutTests.swift`（任务 4b）共用这套线上夹具 ——
///    两份各自抄一份，迟早会分叉成"这条测试跑的是另一种 hello"。
enum Wire {
    static let hello = #"{"protocol":1,"min_split_size_choices":["1M","2M","100M"]}"#
    static let settings = #"{"parallel":8,"connections":16,"splits":16,"min_split_size":"20M","limit_mbps":0,"max_tries":3,"retry_wait":1}"#
    static let getSettings = #"{"settings":\#(settings),"last_code":"C24-8"}"#
    /// `set_settings` 的回执：内核回的是**归一化后的那一份**（`parallel` 与发的不同，
    /// 用来证明壳落的是回执而不是自己发出去的那份）。
    static let settingsEcho = #"{"settings":{"parallel":2,"connections":4,"splits":2,"min_split_size":"1M","limit_mbps":100,"max_tries":1,"retry_wait":0},"last_code":"C24-8"}"#

    static let deliveryA = #"""
    {"code":"AAA-1","page_url":"http://dl.example/AAA-1/index.html","base_url":"http://dl.example",
     "created_at":"2026-09-01T10:00:00+08:00","expires_at":"2026-10-01T10:00:00+08:00",
     "expired":false,"total_files":2,"total_bytes":3000,
     "tree":{"type":"dir","name":"AAA-1","children":{}}}
    """#

    static let deliveryB = #"""
    {"code":"BBB-2","page_url":"http://dl.example/BBB-2/index.html","base_url":"http://dl.example",
     "created_at":"2026-09-02T11:00:00+08:00","expires_at":"2026-10-02T11:00:00+08:00",
     "expired":true,"total_files":5,"total_bytes":4100,
     "tree":{"type":"dir","name":"BBB-2","children":{}}}
    """#

    static let treeA = #"""
    {"tree":{"type":"dir","name":"AAA-1","children":{}},
     "flat":[{"path":"a.bin","name":"a.bin","size":1000,"state":"pending"}],
     "default_selected":["a.bin"],
     "progress":{"total_bytes":3000,"done_bytes":1000,"speed":42,"percent":33}}
    """#

    static let treeB = #"""
    {"tree":{"type":"dir","name":"BBB-2","children":{}},
     "flat":[{"path":"b.bin","name":"b.bin","size":2000,"state":"downloading"}],
     "default_selected":["b.bin"],
     "progress":{"total_bytes":4100,"done_bytes":2700,"speed":77,"percent":66}}
    """#

    static let transfers = #"""
    {"items":[{"gid":"g1","total":1000,"completed":250,"speed":900,"conns":4,
               "state":"active","raw_status":"active","error_message":"","path":"a.bin"}],
     "global":{"download_speed":900,"num_active":1,"num_waiting":0,"num_stopped":0}}
    """#

    static let verify = #"""
    {"ok":["a.bin"],"bad":["b.bin"],"missing":[],"size_mismatch":[],"unverifiable":["c.bin"],
     "unreadable":[],"all_good":false}
    """#

    static let enqueue = #"""
    {"added":[{"gid":"g1","path":"a.bin"}],
     "rejected":[{"path":"z.bin","reason":"路径不安全（越界/控制字符/空段）"}]}
    """#

    static let listDir = #"""
    {"path":"client-test/C24-8_×_25WS024",
     "entries":[{"type":"dir","name":"Figure","children_count":3},
                {"type":"file","name":"reads.fq.gz","path":"client-test/C24-8_×_25WS024/reads.fq.gz",
                 "crc64":"9988776655443322110","size":7000,"completed":7000,"total":7000,
                 "speed":0,"state":"complete","err":""}]}
    """#

    static let transportEnded = "内核进程已退出（管道结束）"
    /// `on_rpc_failure` 探活成功时回的那条 —— 原文就是"可重试"（`core/src/main.rs`）。
    static let rpcFailed = "下载引擎 RPC 失败（快照（tellActive/tellWaiting/tellStopped））：connection reset"
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 有界等待（测试自己的同步手段，不进生产代码）。
///
/// 闭包在**主 actor** 上求值：它读的是 `AppModel` 的状态。
/// ⚠️ 不是 `private`：`AppModelTimeoutTests.swift`（任务 4b）也用它。
@MainActor
func waitUntil(_ what: String, timeout: Double = 3, _ cond: () -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return true }
        try? await Task.sleep(nanoseconds: 2_000_000)
    }
    let ok = cond()
    if !ok { Issue.record("等不到：\(what)") }
    return ok
}

/// 造一个"引擎已在跑"的模型。
///
/// 走**真流程**（`enqueue` 成功 ⇒ 引擎起来了）而不是给测试开后门改 `engine`：
/// `engine` 是 `private(set)`，本来也改不了；顺带证明"轮询的前置条件"确实是
/// 由 enqueue 立起来的。
@MainActor
private func runningModel(_ fake: FakeCore,
                          makeClient: (@Sendable (String?) throws -> any CoreCalling)? = nil) async -> AppModel {
    let model = makeClient.map { AppModel(client: fake, makeClient: $0) } ?? AppModel(client: fake)
    fake.stub("enqueue", Wire.enqueue)
    _ = try? await model.enqueue(paths: [])
    return model
}

// ---------------------------------------------------------------------------
// 启动：握手 / 设置 / 上次交付码
// ---------------------------------------------------------------------------

@MainActor
@Test func startReadsHandshakeSettingsAndLastCode() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    let model = AppModel(client: fake)

    await model.start()

    #expect(model.minSplitSizeChoices == ["1M", "2M", "100M"], "枚举面必须来自 hello，不是壳里的一份硬编码")
    #expect(model.settings == Settings(parallel: 8, connections: 16, splits: 16,
                                       minSplitSize: "20M", limitMbps: 0, maxTries: 3, retryWait: 1))
    #expect(model.lastCode == "C24-8")
    // 内核只在 enqueue 里起引擎（`op_enqueue` 的 `ensure_engine`），而壳每次都是新起一个内核进程
    // —— 所以握手完成后「引擎未启动」是**事实**，不是猜测。
    #expect(model.engine == .notStarted)
    #expect(model.loadState == .idle)
}

@MainActor
@Test func startSendsTheProtocolVersionInHello() async {
    // ⚠️ `hello` **不是无参方法**：设计规格 §5.1 的原文是
    // `{"id":1,"method":"hello","params":{"protocol":1}}`，缺了它内核回 invalid_params。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    let model = AppModel(client: fake)

    await model.start()

    #expect(fake.params("hello") == .object(["protocol": .integer(Int64(kProtocolVersion))]))
    #expect(fake.callCount("hello") == 1)
}

@MainActor
@Test func startWithoutAClientDoesNothing() async {
    // `AppModel.live()` 起不来（找不到内核二进制）时造的就是这种模型：原因已经装在
    // `engine` 里（约束 4 要它出现在界面上），而 `start()` 不该再去敲一个不存在的客户端、
    // 更不该把那条原因冲掉。
    let model = AppModel(engine: .unavailable("找不到内核可执行文件 benagen-core（找过：\n/a\n/b）"),
                         makeClient: { _ in throw CoreError.transport("没有内核") })

    await model.start()

    #expect(model.engine == .unavailable("找不到内核可执行文件 benagen-core（找过：\n/a\n/b）"))
    #expect(model.minSplitSizeChoices.isEmpty)
    #expect(model.loadState == .idle)
}

@MainActor
@Test func bootstrapRunsOnlyOnce() async {
    // 为什么这条必须有（任务 1）：`.task { await model.bootstrap() }` 挂在 `WindowGroup`
    // 的**内容**上，每开一个新窗口就重跑一次，而 `model` 是 App 级的、所有窗口共用。
    // 重跑的后果不是"多花点时间"：`start()` 会把 `engine` 打回 `.notStarted`，
    // `loadRememberedDelivery()` 会把 `loadState` 打回 `.loading`（`load_delivery`
    // 实测约 91.5 秒）—— 对用户来说，"关掉窗口再点 Dock、窗口回来了但空白 91 秒"
    // 与"窗口打不开"是同一件事。
    //
    // ⚠️ 计数用本文件既有的 `callCount(_:)`（`startSendsTheProtocolVersionInHello` 就是这么数的），
    //    **不新造一套替身**：握手是 `hello` + `get_settings` 两条请求，两条都数。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    await model.bootstrap()
    await model.bootstrap()

    #expect(fake.callCount("hello") == 1, "第二个窗口不得重做握手（hello 只发一次）")
    #expect(fake.callCount("get_settings") == 1, "第二个窗口不得重做握手（get_settings 只发一次）")
    #expect(fake.callCount("load_delivery") == 1, "第二个窗口不得重拉清单（load_delivery 只发一次）")
}

// ---------------------------------------------------------------------------
// 协议级告警
// ---------------------------------------------------------------------------

@MainActor
@Test func protocolAlertsAreCollectedNotThrown() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.setAlerts([ErrorBody(code: "bad_request", message: "第 7 行不是合法 JSON")])
    let model = AppModel(client: fake)

    await model.start()

    #expect(model.protocolAlerts.map(\.code) == ["bad_request"])
    #expect(model.protocolAlerts.first?.message == "第 7 行不是合法 JSON")

    // 请求**失败**也不能把告警吞掉（这两条路是分开的：一个是内核的协议级错误，
    // 一个是某条请求的失败）。
    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed", message: "拉取交付清单失败：HTTP 404"))
    fake.setAlerts([ErrorBody(code: "bad_request", message: "第 7 行不是合法 JSON"),
                    ErrorBody(code: "bad_request", message: "第 9 行超长（> 8 MiB）")])
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.protocolAlerts.count == 2, "失败的请求也不能让告警丢掉")
    #expect(model.protocolAlerts.last?.message == "第 9 行超长（> 8 MiB）")
}

@MainActor
@Test func aNewProtocolAlertReachesTheErrorBannerExactlyOnce() async {
    // 约束 5：`id == 0` 的协议级告警要**单独上报**。在这条链补上之前它停在
    // `protocolAlerts` 这个属性上就没了 —— 全壳**唯一**一类没有任何界面落点的失败。
    // 现在它经 `lastError` 上 `RootView` 主区顶部那条常驻横幅（`EngineBanner`）。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("transfer_list", Wire.transfers)
    let model = AppModel(client: fake)
    await model.start()

    fake.setAlerts([ErrorBody(code: "bad_request", message: "请求行超过 8 MiB 上限")])
    await model.refreshTransfers()

    #expect(model.protocolAlerts.map(\.code) == ["bad_request"], "告警照旧留档")
    #expect(model.lastError == "请求行超过 8 MiB 上限", "协议级告警必须登上错误出口")
    #expect(EngineBanner.of(engine: model.engine, lastError: model.lastError)?.text
            == "请求行超过 8 MiB 上限",
            "而且真的落在那条横幅上 —— 否则它仍然没有界面落点")

    // ⚠️ **同一条不得每拍重写一遍**：`lastError` 是**瞬时**出口，
    //    `EngineBanner.transientError` 的语义逐字是"下一拍成功就自己消失"。
    //    每拍重写会把这条告警变成永远消不掉的粘滞项（并把瞬时语义一起毁掉）。
    await model.refreshTransfers()
    #expect(model.protocolAlerts.count == 1, "告警本身是累积的，不会因为上报过就消失")
    #expect(model.lastError == nil, "同一条告警被重写了一遍 —— `lastError` 变成了粘滞出口")

    // 去重 ≠ 只报一次：**新**出现的那条照样要上报。
    fake.setAlerts([ErrorBody(code: "bad_request", message: "请求行超过 8 MiB 上限"),
                    ErrorBody(code: "bad_request", message: "第 9 行超长（> 8 MiB）")])
    await model.refreshTransfers()
    #expect(model.lastError == "第 9 行超长（> 8 MiB）", "新出现的那条要上报")
}

// ---------------------------------------------------------------------------
// 错误映射（附录 A 的处置列）
// ---------------------------------------------------------------------------

@MainActor
@Test func engineNotStartedIsNotAnError() async {
    // `engine_not_started` 是**正常态**（没 enqueue 过就没引擎）：传输列表显示空态，不报错。
    let fake = FakeCore()
    fake.stub("transfer_list", throwing: .rpc(code: "engine_not_started",
                                              message: "下载引擎尚未启动（先 enqueue 才会起引擎）"))
    let model = AppModel(client: fake)

    await model.refreshTransfers()

    #expect(model.engine == .notStarted, "这条码不是「不可用」")
    #expect(model.transfers?.items.isEmpty == true, "传输列表回空态")
    #expect(model.loadState == .idle, "它不是交付清单的失败")
}

@MainActor
@Test func engineDisconnectedSurfacesToTheUI() async {
    let fake = FakeCore()
    fake.stub("transfer_list", throwing: .rpc(code: "engine_disconnected",
                                              message: "下载引擎已断开，重连也没成功：connect refused"))
    let model = AppModel(client: fake)

    await model.refreshTransfers()

    // 逐字是内核那句（约束：不得自己编文案）。
    #expect(model.engine == .unavailable("下载引擎已断开，重连也没成功：connect refused"))
}

@MainActor
@Test func engineStartFailedSurfacesVerbatim() async {
    let fake = FakeCore()
    fake.stub("transfer_list", throwing: .rpc(code: "engine_start_failed",
                                              message: "启动下载引擎失败：端口 6800 被占用"))
    let model = AppModel(client: fake)

    await model.refreshTransfers()

    #expect(model.engine == .unavailable("启动下载引擎失败：端口 6800 被占用"))
}

@MainActor
@Test func protocolMismatchIsReportedAndNotRetried() async {
    // 版本不匹配：文案由壳写（壳自己编的文案之一 —— 另一处是握手超时，见 AppModelTimeoutTests），
    // **且不得重启内核**
    // —— 重启不会让版本变对，只会变成一个漂亮的死循环。
    let fake = FakeCore()
    fake.failEverything(with: .rpc(code: "protocol_mismatch",
                                   message: "协议版本不匹配：内核是 1，请求方是 2"))
    let factory = ClientFactory([])
    let model = AppModel(client: fake, makeClient: factory.factory)

    await model.start()

    guard case .unavailable(let why) = model.engine else {
        Issue.record("版本不匹配必须明确说出来，实际 engine = \(model.engine)")
        return
    }
    #expect(why.contains("版本不匹配"), "文案要让人看懂是什么不匹配：\(why)")
    #expect(factory.madeCount == 0, "版本不匹配不得触发重启")
}

@MainActor
@Test func noDeliveryReturnsToTheEmptyState() async {
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])

    // 内核说"还没有生效的批次"（换码作废 / 重启后的内核）→ 回空态，不是"加载失败"。
    fake.stub("load_delivery", throwing: .rpc(code: "no_delivery",
                                              message: "还没有加载交付清单（先调 load_delivery）"))
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.loadState == .idle, "回空态")
    #expect(model.tree == nil, "清单已经作废，不得留上一批的树")
}

@MainActor
@Test func kernelDeathWhileLoadingDoesNotLeaveTheSpinner() async {
    // ⚠️ 这是审查发现的**卡死**：`load_delivery` 最坏 91.5 秒，内核在这中间死掉正是
    //    规格 §9 存在的理由 —— 而 transport 走的是 `absorb` 的 `.absorbed` 那一支，
    //    一个没兜住就是**永远转圈**（`busyReason` 已被 `defer` 清掉，没有任何请求在飞，
    //    再点一次加载还是同一条路）。
    let fake = FakeCore()
    fake.stub("load_delivery", throwing: .transport(Wire.transportEnded))
    let model = AppModel(client: fake)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.loadState != .loading, "不得停在 .loading")
    #expect(model.loadState == .failed(Wire.transportEnded), "而且要有一句可读的原因")
    #expect(model.busyReason == nil)
}

@MainActor
@Test func loadDeliveryWithoutAKernelStillSaysWhy() async {
    // 同一条链的另一个真实入口：`live()` 找不到内核二进制 ⇒ `client == nil`，
    // 但用户**仍然能粘交付码**（空态页上就摆着那个输入框）。重启工厂也起不来，
    // 于是这里必须给出一句能照着排查的话（约束 4：不得静默少交）。
    let model = AppModel(engine: .unavailable("找不到内核可执行文件 benagen-core（找过：\n/a\n/b）"),
                         makeClient: { _ in throw CoreError.transport("还是没有内核") })

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    guard case .failed(let why) = model.loadState else {
        Issue.record("必须给出一句可读的失败原因，实际 \(model.loadState)")
        return
    }
    #expect(why.contains("找不到内核可执行文件"), "原因要能照着排查：\(why)")
    #expect(model.busyReason == nil)
}

@MainActor
@Test func loadDeliveryWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 简报点名的**硬要求**：内核一旦卡死（例如首次启动卡在 TCC 授权框上，任务 4b 查明），
    //    `CoreClient` 的内部 FIFO 队列会被那条**永不返回**的请求**永久堵死**（约束 15），
    //    此后每个新请求都永远挂在它后面。用户点「加载」的观测结果就是界面**永远停在
    //    「正在加载交付清单…」** —— 那正是约束 4 明禁的静默失效。
    //
    // 现场：客户端还在，但引擎已经被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**全都是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西：只要发了，就是发了条注定挂死的请求。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.start()
    #expect(model.engine == .unavailable("下载引擎已断开，重连也没成功：connection refused"),
            "前置现场没搭起来（engine 不是 .unavailable）")

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(fake.callCount("load_delivery") == 0, "不得发出那条注定挂死的请求")
    #expect(fake.callCount("get_tree") == 0, "树也不去拉：同一条 FIFO 队列，一样会挂死")
    #expect(model.loadState == .failed("下载引擎已断开，重连也没成功：connection refused"),
            "用内核那句原文（约束 3），不是壳自己编的话")
    #expect(model.busyReason == nil, "不得留下一个没有任何请求在飞的转圈")
}

@MainActor
@Test func anOversizedDeliveryCodeNeverReachesTheKernel() async {
    // ⚠️ 全局约束 C-3：「壳侧**绝不**允许发出可能超限的请求」。内核的行长上限是 8 MiB
    //    （`core/src/main.rs:113`），而超限的处置**不是报错** —— `:1657-1661` 只回一条
    //    `id == 0` 的协议告警，那条请求**永远等不到响应**，`CoreClient` 那条 FIFO
    //    **串行**队列于是**被永久堵死**（约束 15），界面上一个字都不说 ——
    //    用户看到的是"应用整个卡死"，只有重启能恢复。
    //
    // 两条覆盖（模型层这道闸是**结构性**的：视图那两处按钮禁用不算证据，因为
    // `loadDelivery` / `switchDelivery` 是 public 的，任何新调用点都会绕过视图）：
    //   ① `.initial` 模式（`loadDelivery`）：失败落在 `loadState` 上、供空态页显示；
    //   ② `.switching` 模式（`switchDelivery`）：失败交给调用方，原批次一个字都不动。
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    // 重启工厂备好一个替身：**一旦它被调用，就说明这条壳自己造的错漏进了 `absorb`**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)

    // 9 MiB —— 复审点名的那条**真的会超过内核 8 MiB 行上限**的量级。
    let huge = String(repeating: "A", count: 9 << 20)

    await model.loadDelivery(code: huge, baseURL: nil)

    #expect(fake.callCount("load_delivery") == 0,
            "不得发出那条永不返回、会把 FIFO 队列永久堵死的请求")
    #expect(fake.callCount("get_tree") == 0, "树也不去拉：同一条 FIFO 队列，一样堵死")
    // 失败要有落点（约束 4）：空态页显示这句 + 「重试」。
    if case .failed(let why) = model.loadState {
        // 说清楚是**长度**问题、给出**上界**、说明**后果**（内核不回话、一直等下去）
        // —— 这句是壳写的（内核收不到这条请求，没有原文可登），所以它必须自己讲全。
        #expect(why.contains("交付码长 \(huge.utf8.count) 字节"), "要报出实际的字节数：\(why)")
        #expect(why.contains("安全上限"), "要说清楚触发的是**长度上界**，不是形状校验：\(why)")
        #expect(why.contains(DeliveryCodeEntry.maximumText), "要给出上界：\(why)")
        #expect(why.contains("一直等下去"), "要说清楚后果（约束 4：不是光说'不行'）：\(why)")
    } else {
        Issue.record("超长交付码必须落成 .failed（空态页才说得出话），实际：\(model.loadState)")
    }
    #expect(factory.madeCount == 0, "壳自己造的错误不得触发重启 —— 用户只是粘错了一次内容")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.busyReason == nil, "闸门在忙指示之前：不得留下一个没有任何请求在飞的转圈")

    // ② 换码那条路：原批次一个字都不动，失败经回执交给调用方与主区那一行。
    fake.stub("load_delivery", switchWireAAAA)
    await model.loadDelivery(code: "AAAA", baseURL: nil)
    guard case .loaded(let before) = model.loadState else {
        Issue.record("前置条件不成立：第一批没加载上")
        return
    }
    let loadsBefore = fake.callCount("load_delivery")

    let outcome = await model.switchDelivery(code: huge, baseURL: nil)

    #expect(fake.callCount("load_delivery") == loadsBefore, "换码那条路也不得把它发出去")
    if case .failed(let why) = outcome {
        #expect(why.contains("安全上限") && why.contains(DeliveryCodeEntry.maximumText),
                "面板与主区拿到的都该是这句（同一条闸门、同一句话）：\(why)")
    } else {
        Issue.record("超长交付码换码必须回 .failed，实际：\(outcome)")
    }
    #expect(model.loadState == .loaded(before), "原批次原样还在（换码失败不写 loadState）")
    #expect(model.lastSwitchOutcome == outcome, "回执照样要落在模型上（主区那一行）")
    #expect(factory.madeCount == 0, "换码那条路同样不得触发重启")
}

@MainActor
@Test func anOversizedBaseURLNeverReachesTheKernel() async {
    // ⚠️ 与上一条**同一条请求、同一个理由**（约束 C-3）：`base_url` 是空态页「高级」里
    //    用户手输的另一个串，它与 `code` 装在**同一条** `load_delivery` 里 ——
    //    只挡 `code` 那一半的话，粘贴一大段东西到这个字段里照样能把客户端**静默堵死**。
    //
    // ⚠️ 这条修正**不在复审清单上**，是我在实现 C1 时顺着同一条请求找出来的相邻缺口：
    //    判据与上界复用 `DeliveryCodeEntry` 那一份（它管的是"用户手输进这条请求的串"），
    //    所以不存在两套阈值。
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)

    let huge = String(repeating: "/very-long-path-segment", count: 400_000)   // 约 9.6 MB
    await model.loadDelivery(code: "AAA-1", baseURL: huge)

    #expect(fake.callCount("load_delivery") == 0, "不得把超长的 base_url 发出去")
    if case .failed(let why) = model.loadState {
        #expect(why.contains("自定义下载地址"), "要说清楚过长的是**哪一个字段**：\(why)")
        #expect(why.contains(DeliveryCodeEntry.maximumText), "要给出上界：\(why)")
    } else {
        Issue.record("超长 base_url 必须落成 .failed，实际：\(model.loadState)")
    }
    #expect(factory.madeCount == 0, "壳自己造的错误不得触发重启")
    #expect(model.busyReason == nil, "闸门在忙指示之前")

    // 反向的一半：正常长度的地址**照发**（空串同样照发 —— 它是"用默认交付服务器"的合法表示）。
    fake.stub("load_delivery", Wire.deliveryA)
    let normal = "http://download.benagen.com/" + String(repeating: "a", count: 500)
    await model.loadDelivery(code: "AAA-1", baseURL: normal)
    #expect(fake.callCount("load_delivery") == 1, "正常长度的 base_url 不得被误伤")
    await model.loadDelivery(code: "AAA-1", baseURL: "")
    #expect(fake.callCount("load_delivery") == 2, "空串是合法的（用默认交付服务器），不得被误伤")

    // 参数面：空串时**不带** `base_url` 键（既有纪律，`loadDeliverySendsCodeAndBaseURLOnlyWhenGiven` 也钉着）。
    #expect(fake.params("load_delivery", occurrence: 1) == .object(["code": .string("AAA-1")]),
            "空串要退化成不带这个键，而不是发一个空串出去")
}

@MainActor
@Test func loadDeliveryFailureKeepsTheOldManifest() async {
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])

    // 刷新失败（网络抖一下）：**不得清空已加载的清单**，原文照登。
    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed",
                                              message: "拉取交付清单失败：HTTP 404"))
    await model.loadDelivery(code: "BBB-2", baseURL: nil)

    #expect(model.loadState == .failed("拉取交付清单失败：HTTP 404"), "其余错误码呈现内核原文")
    #expect(model.tree?.flat.map(\.path) == ["a.bin"], "清单必须还在")
}

@MainActor
@Test func startupLoadsTheRememberedDeliveryCode() async {
    // 规格 §8.3「记住交付码」：内核记着上次用过的码（`get_settings` 回的 `last_code`），
    // 壳启动时自动加载一次 —— 用户重开应用就能看到上次那批。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)     // last_code = "C24-8"
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.params("load_delivery") == .object(["code": .string("C24-8")]),
            "要用**内核记着的那个码**去加载（不是空串、也不是壳猜的）")
    guard case .loaded = model.loadState else {
        Issue.record("自动加载成功应进 .loaded，实际 \(model.loadState)")
        return
    }
}

@MainActor
@Test func startupWithoutARememberedCodeDoesNotLoadAnything() async {
    // 内核没记着码（首次使用）→ **一个请求都不发**。发了就是把空串递给内核换一条
    // invalid_params 回来，然后每次启动都在空态页上摆一个错误 —— 而那不是故障。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", #"{"settings":\#(Wire.settings),"last_code":""}"#)
    fake.stub("load_delivery", Wire.deliveryA)
    let model = AppModel(client: fake)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.callCount("load_delivery") == 0)
    #expect(model.loadState == .idle, "界面停在空态页，不是错误页")
}

// ---------------------------------------------------------------------------
// 交付清单与树
// ---------------------------------------------------------------------------

@MainActor
@Test func loadDeliveryRefreshesTheTreeSnapshot() async {
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])

    fake.stub("load_delivery", Wire.deliveryB)
    fake.stub("get_tree", Wire.treeB)
    await model.loadDelivery(code: "BBB-2", baseURL: nil)

    // 换批次后 tree 必须跟着换 —— 留着上一批的树 = 客户点了下载下到别批的文件。
    #expect(model.tree?.flat.map(\.path) == ["b.bin"])
    #expect(model.tree?.progress.percent == 66)
    #expect(model.lastCode == "BBB-2", "上次用过的码跟着内核回执走")
    guard case .loaded(let info) = model.loadState else {
        Issue.record("加载成功应进 .loaded，实际 \(model.loadState)")
        return
    }
    #expect(info.code == "BBB-2")
    #expect(info.expired, "过期必须让客户看得见")
}

@MainActor
@Test func loadDeliverySendsCodeAndBaseURLOnlyWhenGiven() async {
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(fake.params("load_delivery") == .object(["code": .string("AAA-1")]),
            "没给 base_url 就不发这个键（内核会用自己的默认值）")

    await model.loadDelivery(code: "AAA-1", baseURL: "http://dl.example")
    #expect(fake.params("load_delivery", occurrence: 1) == .object(["code": .string("AAA-1"),
                                                                    "base_url": .string("http://dl.example")]))
}

@MainActor
@Test func getTreePopulatesTheTreeSnapshot() async throws {
    let fake = FakeCore()
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    let r = try await model.getTree()

    #expect(r.defaultSelected == ["a.bin"])
    #expect(fake.params("get_tree") == .null, "get_tree 无参")
    #expect(model.tree?.flat.map(\.path) == ["a.bin"], "顺便刷新 self.tree")
}

@MainActor
@Test func theTreeIsStampedWithTheBatchItCameFrom() async {
    // ⚠️ 审查重要 ① 的另一半：`loadState = .loaded(info)` 在拉 `get_tree` **之前**就落了，
    //    而视图是在 `.loaded` 那一刻出现的 —— 于是它可能读到 `tree == nil`，
    //    更糟的是可能读到**上一批的树**（`tree` 只在 `no_delivery` 时才清）。
    //    `treeCode` 就是让视图分得清"手上这棵树是不是这一批的"的那个信号；
    //    没有它，`BrowserSelection.onNewManifest` 只能瞎猜。
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    #expect(model.treeCode == nil, "还没有任何树")

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.tree?.flat.map(\.path) == ["a.bin"])
    #expect(model.treeCode == "AAA-1", "树要盖上「它属于哪一批」")

    // 换一批：`load_delivery` 与 `get_tree` 都换成 B 批的。
    fake.stub("load_delivery", Wire.deliveryB)
    fake.stub("get_tree", Wire.treeB)
    await model.loadDelivery(code: "BBB-2", baseURL: nil)

    #expect(model.tree?.flat.map(\.path) == ["b.bin"])
    #expect(model.treeCode == "BBB-2", "换批后必须盖新码 —— 否则新批次会被**旧树**播种")
}

@MainActor
@Test func listDirSendsThePathVerbatim() async throws {
    let fake = FakeCore()
    fake.stub("list_dir", Wire.listDir)
    let model = AppModel(client: fake)

    let r = try await model.listDir("client-test/C24-8_×_25WS024")

    // 约束 3：空格与非 ASCII 不得规范化、不得转义。
    #expect(fake.params("list_dir") == .object(["path": .string("client-test/C24-8_×_25WS024")]))
    #expect(r.path == "client-test/C24-8_×_25WS024")
    #expect(r.entries.count == 2)
    if case .dir(let name, let children) = r.entries[0] {
        #expect(name == "Figure")
        #expect(children == 3)
    } else {
        Issue.record("第一项应该是目录")
    }
    if case .file(let f) = r.entries[1] {
        #expect(f.size == 7000)
    } else {
        Issue.record("第二项应该是文件")
    }
}

@MainActor
@Test func listDirWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 简报点名的**硬要求**（与任务 5 的 `loadDelivery` 闸门同一条）：`list_dir` /
    //    `get_tree` 是**新的挂死入口** —— 内核一旦卡死，`CoreClient` 的 FIFO 队列
    //    被那条永不返回的请求永久堵死（约束 15），用户双击一个目录后看到的是
    //    **永远停在加载态**（约束 4 明禁的静默失效）。
    //
    // 现场：客户端还在，但引擎已被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**全是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("list_dir", Wire.listDir)
    fake.stub("get_tree", Wire.treeA)
    // 重启工厂备好一个替身：**一旦它被调用，就说明闸门漏了一条路出去**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)
    await model.start()
    let unusable = AppModel.EngineState.unavailable("下载引擎已断开，重连也没成功：connection refused")
    #expect(model.engine == unusable, "前置现场没搭起来（engine 不是 .unavailable）")

    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        _ = try await model.listDir("client-test/C24-8_×_25WS024")
    }
    #expect(fake.callCount("list_dir") == 0, "不得发出那条注定挂死的请求")
    // ⚠️ 这一条盯的是**闸门的位置**：造一个假 `CoreError.transport` 再把它喂进 `absorb`
    //    （也就是把闸门放进 `absorbing` 里面），会走 `.transport` → `onKernelDeath`
    //    → `restartIfAllowed` —— 于是**双击一个目录就重启一次内核**，还把 `engine`
    //    改写成"下载引擎已断开：…"。用户只是浏览目录，凭什么换掉他的内核进程？
    #expect(factory.madeCount == 0, "不得顺手重启内核")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.engine == unusable, "闸门不改引擎状态（原文照旧）")

    // `get_tree` 走同一条防线（同一个漏斗口）。
    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        _ = try await model.getTree()
    }
    #expect(fake.callCount("get_tree") == 0, "树也不去拉：同一条 FIFO 队列，一样会挂死")
    #expect(factory.madeCount == 0)
    #expect(model.tree == nil, "树快照不得被一次没发出去的请求改动")
}

@MainActor
@Test func theEngineGateDoesNotSealOffTheUnknownState() async throws {
    // 闸门只看 `.unavailable`，**不看 `.unknown`**：`.unknown` 是"握手还没结论"，
    // 那时请求排在握手后面是正常的（会轮到）。闸门开宽了会把"引擎还没问过"也一并封死 ——
    // 上面那些 `engine == .unknown` 下正常取树的测试会全红，这条把那条边界钉明白。
    let fake = FakeCore()
    fake.stub("list_dir", Wire.listDir)
    let model = AppModel(client: fake)
    #expect(model.engine == .unknown, "前置现场：还没握过手")

    let r = try await model.listDir("client-test/C24-8_×_25WS024")

    #expect(fake.callCount("list_dir") == 1, "`.unknown` 不是「不可用」，请求照发")
    #expect(r.entries.count == 2)
}

// ---------------------------------------------------------------------------
// 下载与任务动作
// ---------------------------------------------------------------------------

@MainActor
@Test func enqueueSendsPathsAsGivenAndStartsTheEngine() async throws {
    let fake = FakeCore()
    fake.stub("enqueue", Wire.enqueue)
    let model = AppModel(client: fake)

    let r = try await model.enqueue(paths: ["b.bin", "a.bin"])

    // 顺序原样、**不展开目录**（展开是内核 `resolve_targets` 的事，约束 1）。
    #expect(fake.params("enqueue") == .object(["paths": .array([.string("b.bin"), .string("a.bin")])]))
    #expect(r.added.map(\.gid) == ["g1"])
    #expect(r.rejected.map(\.reason) == ["路径不安全（越界/控制字符/空段）"])
    #expect(model.engine == .running, "enqueue 成功 ⇔ 内核已经起了引擎")
}

@MainActor
@Test func enqueueWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 裁决 ③：`enqueue` 是**又一个挂死入口** —— 与 `loadDelivery`（任务 5）、
    //    `listDir`/`getTree`（任务 6）同一条最小防线。内核一旦卡死（首次启动卡在 TCC
    //    授权框上的 `open()` 里），`CoreClient` 内部那条 FIFO **串行**队列会被永不返回的
    //    请求**永久堵死**（约束 15）：用户点「下载」看到的是一颗按下去就再也没有回音的按钮
    //    —— 那正是约束 4 明禁的静默失效。
    //
    // 现场：客户端还在，但引擎已被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**全是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("enqueue", Wire.enqueue)
    // 重启工厂备好一个替身：**一旦它被调用，就说明闸门漏了一条路出去**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)
    await model.start()
    let unusable = AppModel.EngineState.unavailable("下载引擎已断开，重连也没成功：connection refused")
    #expect(model.engine == unusable, "前置现场没搭起来（engine 不是 .unavailable）")

    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        _ = try await model.enqueue(paths: ["a.bin"])
    }
    #expect(fake.callCount("enqueue") == 0, "不得发出那条注定挂死的请求")
    // ⚠️ 这一条盯的是**闸门的位置**：把闸门挪进 `absorbing` 里面（或者把它喂给 `absorb`），
    //    这条壳自己造的 transport 会走 `onKernelDeath` → `restartIfAllowed` ——
    //    于是**点一次「下载」就杀掉并重启一次内核**，还把 `engine` 改写成
    //    "下载引擎已断开：<原来那句>"。用户只是点了一下下载，凭什么换掉他的内核进程？
    #expect(factory.madeCount == 0, "不得顺手重启内核")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.engine == unusable, "闸门不改引擎状态（原文照旧）")
    #expect(model.busyReason == nil, "闸门在忙指示之前：不得留下一个没有任何请求在飞的忙指示")
}

@MainActor
@Test func anOversizedEnqueueNeverReachesTheKernel() async {
    // ⚠️ 全局约束 C-3：内核的行长上限是 8 MiB（`core/src/main.rs:113`），超限的**处置不是报错** ——
    //    `:1657-1661` 只回一条 `id == 0` 的协议告警，那条请求**永远等不到响应**，
    //    `CoreClient` 那条 FIFO **串行**队列于**被永久堵死**（约束 15），界面上一个字都不说。
    //    所以壳必须在**发出去之前**就拒绝，并且把"该怎么做"说出来。
    let fake = FakeCore()
    fake.stub("enqueue", Wire.enqueue)
    // 重启工厂备好一个替身：**一旦它被调用，就说明这条壳自己造的错漏进了 `absorb`**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)

    // 20 万条、每条约 50 字节 ⇒ 约 10 MB，远超 4 MiB 的预算（也远超内核那 8 MiB 的上界）。
    let many = (0..<200_000).map { "dir\($0)/file-with-a-fairly-long-name-\($0).bin" }

    var thrown: (any Error)?
    do {
        _ = try await model.enqueue(paths: many)
    } catch {
        thrown = error
    }

    // 是**这条**守卫的错，不是别的路：形状是 `transport`（壳自己造的，内核一个字没说），
    // 且把"超了多少 / 该怎么做"说出来了。**不逐字抄整句文案** —— 那等于把实现抄一遍；
    // 这里只钉住它在讲这件事（内核上界、项数、出路）而非别的错。
    guard case .transport(let why)? = thrown as? CoreError else {
        Issue.record("超预算的 enqueue 必须抛 CoreError.transport，实际：\(String(describing: thrown))")
        return
    }
    // ⚠️ 措辞的事实基础（复审顺手修 ②）：**触发这条守卫的是壳自己的预算（4 MiB）**，
    //    不是内核那 8 MiB 上限 —— 5 MiB 的请求并不超限。所以说"超过内核的 8 MiB 上限"
    //    是一句假话，而排障的人会照着它得出错误结论。下面两条一起钉住"说法与事实一致"：
    //    ① 说清楚是**安全预算**触发的；② 内核的真实上界照样给出来，人才对得上量。
    #expect(why.contains("预算"),
            "必须说清楚触发它的是壳的**安全预算**（4 MiB），不是内核那 8 MiB 上限 —— 5 MiB 的请求并不超限")
    #expect(why.contains("8 MiB"), "内核的真实上界（8 MiB）照样要给出，人才对得上'大概多少项会超'")
    #expect(why.contains("\(many.count) 项"), "必须说清楚超了多少项")
    #expect(why.contains("全选"), "必须给出出路（全选 / 分批），不是光说'不行'")

    // ⚠️ 下面四条才是这条测试的重点。
    #expect(fake.callCount("enqueue") == 0, "不得发出那条永不返回的请求（它会把 FIFO 队列永久堵死）")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(factory.madeCount == 0, "壳自己造的错误不得触发重启 —— 用户只是勾多了几项")
    #expect(model.busyReason == nil, "闸门在忙指示之前：不得留下一个没有任何请求在飞的忙指示")
}

@MainActor
@Test func aNormalSizedEnqueueIsNotBlockedByTheBudgetGuard() async throws {
    // 预算守卫**不得误伤**正常请求：它是"宁可早一点拒绝"，不是"稍微大点就拒绝"。
    // 它真误伤的样子是**静默**的（下载按钮按下去报一句莫名其妙的上限）—— 所以要有这条对照。
    let fake = FakeCore()
    fake.stub("enqueue", Wire.enqueue)
    let model = AppModel(client: fake)

    // 5000 条 × 约 26 字节 ≈ 130 KB，离 4 MiB 还远。
    let batch = (0..<5000).map { "dir\($0)/file-\($0).bin" }
    _ = try await model.enqueue(paths: batch)

    #expect(fake.callCount("enqueue") == 1, "正常规模的请求照样发得出去")
}

@MainActor
@Test func taskActionSendsActionAndOptionalGid() async throws {
    let fake = FakeCore()
    fake.stub("task_action", #"{"action":"clear_finished"}"#)
    let model = AppModel(client: fake)

    try await model.taskAction(.clearFinished, gid: nil)
    #expect(fake.params("task_action") == .object(["action": .string("clear_finished")]))

    fake.stub("task_action", #"{"action":"pause"}"#)
    try await model.taskAction(.pause, gid: "g1")
    #expect(fake.params("task_action", occurrence: 1) == .object(["action": .string("pause"),
                                                                  "gid": .string("g1")]))
}

@MainActor
@Test func taskActionWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 任务 8 补的**第四条**最小防线（前三条：`loadDelivery` / `listDir`+`getTree` /
    //    `enqueue`）。`task_action` 是**又一个**挂死入口：内核一旦卡死，
    //    `CoreClient` 内部那条 FIFO **串行**队列会被那条永不返回的请求**永久堵死**
    //    （约束 15），此后每个新请求都排在那条后面、永远轮不到 —— 用户点「暂停 / 移除」
    //    看到的是一颗按下去再也没有回音的按钮（约束 4 明禁的静默失效）。
    //
    // 现场：客户端还在，但引擎已被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**全是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("task_action", #"{"action":"pause"}"#)
    // 重启工厂备好一个替身：**一旦它被调用，就说明闸门漏了一条路出去**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)
    await model.start()
    let unusable = AppModel.EngineState.unavailable("下载引擎已断开，重连也没成功：connection refused")
    #expect(model.engine == unusable, "前置现场没搭起来（engine 不是 .unavailable）")

    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        try await model.taskAction(.pause, gid: "g1")
    }
    #expect(fake.callCount("task_action") == 0, "不得发出那条注定挂死的请求")
    // ⚠️ 这一条盯的是**闸门的位置**：把闸门挪进 `absorbing` 里面（或者把它喂给 `absorb`），
    //    这条壳自己造的 transport 会走 `onKernelDeath` → `restartIfAllowed` ——
    //    于是**点一次「暂停」就杀掉并重启一次内核**，还把 `engine` 改写成
    //    "下载引擎已断开：<原来那句>"。用户只是点了下暂停，凭什么换掉他的内核进程？
    #expect(factory.madeCount == 0, "不得顺手重启内核")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.engine == unusable, "闸门不改引擎状态（原文照旧）")

    // 列表级的那一条（`clear_finished`，没有 gid）走同一个漏斗口。
    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        try await model.taskAction(.clearFinished, gid: nil)
    }
    #expect(fake.callCount("task_action") == 0)
    #expect(factory.madeCount == 0)
}

@MainActor
@Test func applySettingsSendsTheSevenWireKeys() async throws {
    // ⚠️ 这条盯的是**「类型化载荷走 requestEncoder」**这个口径：属性名是 camelCase，
    //    而内核 `set_settings` 走 `serde_json::from_value::<Settings>`（`Settings` 没有
    //    任何 rename）。发成 camelCase 的 payload **看起来完全正常（也是七个键）**，
    //    内核只会回一条 invalid_params。
    let sent = Settings(parallel: 8, connections: 16, splits: 16, minSplitSize: "20M",
                        limitMbps: 0, maxTries: 3, retryWait: 1)
    let fake = FakeCore()
    fake.stub("set_settings", Wire.settingsEcho)
    let model = AppModel(client: fake)

    try await model.applySettings(sent)

    let params = fake.params("set_settings")
    guard case .object(let top)? = params, case .object(let inner)? = top["settings"] else {
        Issue.record("set_settings 的 params 形状不对：\(String(describing: params))")
        return
    }
    #expect(Set(inner.keys) == ["parallel", "connections", "splits", "min_split_size",
                                "limit_mbps", "max_tries", "retry_wait"],
            "七个键一个不能少，且必须是线上键名")
    #expect(inner["min_split_size"] == .string("20M"))
    #expect(model.settings?.parallel == 2, "落状态的是内核回执那一份")
    #expect(model.lastCode == "C24-8")
}

@MainActor
@Test func applySettingsWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 任务 10 补的**第六条**防线（前五条：`loadDelivery` / `getTree`+`listDir` /
    //    `enqueue` / `taskAction` / `refreshVerify`），与它们逐字同款：`set_settings`
    //    是**又一个**挂死入口 —— 内核一旦卡死，`CoreClient` 内部那条 FIFO **串行**队列
    //    会被那条永不返回的请求**永久堵死**（约束 15），此后每个新请求都排在那条后面、
    //    永远轮不到。设置窗口是**唯一**能改内核参数的地方，这条路静默失效的后果是
    //    "参数再也改不了"，而用户看到的是一颗按下去没有回音的「保存」（约束 4 明禁）。
    //
    // 现场：客户端还在，但引擎已被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("set_settings", Wire.settingsEcho)
    // 重启工厂备好一个替身：**一旦它被调用，就说明闸门漏了一条路出去**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)
    await model.start()
    let unusable = AppModel.EngineState.unavailable("下载引擎已断开，重连也没成功：connection refused")
    #expect(model.engine == unusable, "前置现场没搭起来（engine 不是 .unavailable）")

    let sent = Settings(parallel: 3, connections: 4, splits: 5, minSplitSize: "100M",
                        limitMbps: 100_000, maxTries: 7, retryWait: 60)
    await #expect(throws: CoreError.transport("下载引擎已断开，重连也没成功：connection refused")) {
        try await model.applySettings(sent)
    }
    #expect(fake.callCount("set_settings") == 0, "不得发出那条注定挂死的请求")
    // ⚠️ 这一条盯的是**闸门的位置**：把它挪进 `absorbing` 里面（或者喂给 `absorb`），
    //    这条壳自己造的 transport 会走 `onKernelDeath` → `restartIfAllowed` ——
    //    于是**点一次「保存」就杀掉并重启一次内核**，还把 `engine` 改写成
    //    "下载引擎已断开：<原来那句>"。用户只是改了个参数，凭什么换掉他的内核进程？
    #expect(factory.madeCount == 0, "不得顺手重启内核")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.engine == unusable, "闸门不改引擎状态（原文照旧）")
    #expect(model.settings == nil, "更不得凭空写状态：内核一个回执都没给")
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

@MainActor
@Test func refreshVerifyPopulatesTheSixBuckets() async {
    let fake = FakeCore()
    fake.stub("verify_status", Wire.verify)
    let model = AppModel(client: fake)

    await model.refreshVerify()

    #expect(model.verify?.ok == ["a.bin"])
    #expect(model.verify?.bad == ["b.bin"])
    #expect(model.verify?.missing.isEmpty == true)
    #expect(model.verify?.unverifiable == ["c.bin"])
    #expect(model.verify?.allGood == false)
}

@MainActor
@Test func loadingAnotherBatchDropsThePreviousVerifySnapshot() async {
    // ⚠️ 任务 9 复审的**重要 ①**：`verify` 是按批次的快照，而 `VerifyStatus` 没有交付码字段
    //    （协议里就没有），所以"这份结果属于哪一批"只能由壳记着。`performLoadDelivery`
    //    从任务 3 到任务 8 都没碰过 `verify` ⇒ 换批之后它还是**上一批**那份。
    //    后果不是一个转瞬即逝的中间帧：`Sidebar.swift` 的校验徽标与 `VerifyView` 都直接读
    //    `model.verify`，用户加载 B 批后若一直待在「文件」/「传输列表」，徽标会**一直**
    //    挂着 A 批的未通过数，而唯一的自愈路径是"进一次校验屏、且那一次刷新成功"。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    fake.stub("verify_status", Wire.verify)
    let model = AppModel(client: fake)
    await model.start()
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    await model.refreshVerify()
    #expect(model.verify != nil, "前置现场没搭起来：A 批的校验结果要先真的拿到")
    // `Wire.verify` 的六类：ok=["a.bin"]、bad=["b.bin"]、unverifiable=["c.bin"]、其余空
    // ⇒ 未通过 = bad 那 1 项（`unverifiable` **不算**，内核口径）
    #expect(SidebarBadge.unpassed(model.verify) == 1, "A 批这份快照的可见投影")

    // 换到 B 批
    fake.stub("load_delivery", Wire.deliveryB)
    fake.stub("get_tree", Wire.treeB)
    await model.loadDelivery(code: "BBB-2", baseURL: nil)

    #expect(model.verify == nil, "换批后不得再挂着上一批的校验结果")
    #expect(SidebarBadge.unpassed(model.verify) == 0, "徽标的可见投影必须跟着归零")
}

@MainActor
@Test func reloadingTheSameBatchKeepsItsVerifySnapshot() async {
    // 同一条纪律的**另一半**（复审明文："同一个码重复 load 不该把刷新也当成换批"）：
    // 重新加载**同一个**交付码是刷新，不是换批 —— 那份快照说的还是这一批，不能抹掉。
    // `loadDelivery` 的两条路都可能这样：空态页那颗「重试」用的还是同一个码；
    // 内核崩溃后的恢复走的是 `performLoadDelivery(code: loadedCode)`。
    // ⚠️ 钉住这一条是为了挡住"换批就清"被简化成"每次 load 都清"（变异 M13b）。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    fake.stub("verify_status", Wire.verify)
    let model = AppModel(client: fake)
    await model.start()
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    await model.refreshVerify()
    #expect(model.verify != nil)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)   // 同一个码 = 刷新

    #expect(model.verify != nil, "同批刷新不是换批：这份快照说的还是这一批")
    #expect(SidebarBadge.unpassed(model.verify) == 1)
}

@MainActor
@Test func refreshVerifyWithAnUnavailableEngineFailsFastWithoutCallingTheKernel() async {
    // ⚠️ 任务 9 补的**第五条**防线（前四条：`loadDelivery` / `getTree`+`listDir` /
    //    `enqueue` / `taskAction`），与它们逐字同款：`verify_status` 是**又一个**挂死入口
    //    —— 内核一旦卡死，`CoreClient` 的 FIFO 队列被那条永不返回的请求永久堵死
    //    （约束 15），用户点「刷新」看到的是一颗再也没有回音的按钮（约束 4）。
    //
    // 现场：客户端还在，但引擎已被判为不可用（内核回了 engine_disconnected）。
    // ⚠️ 桩**是好的** —— 挂死不是因为内核会回错，而是因为请求根本发不出去。
    //    所以"调用次数为 0"才是这条测试真正盯的东西。
    let fake = FakeCore()
    fake.stub("hello", throwing: .rpc(code: "engine_disconnected",
                                      message: "下载引擎已断开，重连也没成功：connection refused"))
    fake.stub("verify_status", Wire.verify)
    // 重启工厂备好一个替身：**一旦它被调用，就说明闸门漏了一条路出去**。
    let factory = ClientFactory([FakeCore()])
    let model = AppModel(client: fake, makeClient: factory.factory)
    await model.start()
    let unusable = AppModel.EngineState.unavailable("下载引擎已断开，重连也没成功：connection refused")
    #expect(model.engine == unusable, "前置现场没搭起来（engine 不是 .unavailable）")

    // ⚠️ 这条路的出口不是 `throws`（没有调用方可以 catch）：**一个字都不该写**
    //    —— 不发请求、不造 `lastError`、不动 `verify`。原因在界面上由顶部横幅给出。
    await model.refreshVerify()

    #expect(fake.callCount("verify_status") == 0, "不得发出那条注定挂死的请求")
    #expect(model.verify == nil, "一次没发出去的刷新不得改动校验快照")
    #expect(model.lastError == nil, "更不得凭空造出一句错误原文")
    // ⚠️ 这一条盯的是**闸门的形态**：把闸门做成"抛错、再让 `surfacing` 吸收"，
    //    这条壳自己造的 transport 会走 `onKernelDeath` → `restartIfAllowed` ——
    //    于是**点一次「刷新」就杀掉并重启一次内核**，还把 `engine` 改写成
    //    "下载引擎已断开：<原来那句>"。用户只是点了下刷新，凭什么换掉他的内核进程？
    #expect(factory.madeCount == 0, "不得顺手重启内核")
    #expect(fake.shutdownCount == 0, "旧客户端也不该被收掉")
    #expect(model.engine == unusable, "闸门不改引擎状态（原文照旧）")
}

// ---------------------------------------------------------------------------
// 跳拍（全局约束 15）与单飞
// ---------------------------------------------------------------------------

@MainActor
@Test func pollDoesNotStartBeforeTheEngineIsRunning() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("transfer_list", Wire.transfers)
    let model = AppModel(client: fake)
    await model.start()                       // engine == .notStarted
    #expect(model.engine == .notStarted)

    await model.pollTick()

    #expect(fake.callCount("transfer_list") == 0, "引擎没起来就不该去问传输列表")
}

@MainActor
@Test func pollSkipsATickWhenThePreviousOneIsStillRunning() async {
    let fake = FakeCore()
    fake.stub("transfer_list", Wire.transfers)
    let model = await runningModel(fake)
    let gate = TestGate()
    fake.gate = gate

    let first = Task { await model.pollTick() }
    #expect(await waitUntil("第一拍进入内核") { fake.inFlight == 1 })

    let second = Task { await model.pollTick() }        // 上一拍还没回来
    try? await Task.sleep(nanoseconds: 150_000_000)     // 给第二拍足够的机会跑起来

    #expect(fake.callCount("transfer_list") == 1, "上一拍没回来就跳过这一拍，不排队")

    gate.release()
    await first.value
    await second.value
    #expect(fake.callCount("transfer_list") == 1)
    #expect(fake.syncCallCount == 0, "所有请求都必须走 callAsync —— callSync 是阻塞的（裁定 ①）")
}

@MainActor
@Test func atMostOneRequestIsInFlight() async {
    let fake = FakeCore()
    fake.stub("transfer_list", Wire.transfers)
    let model = await runningModel(fake)
    let gate = TestGate()
    fake.gate = gate

    // 两个入口（定时器那一拍 + 动作后的立即刷新）同时来
    let a = Task { await model.pollTick() }
    #expect(await waitUntil("第一条进入内核") { fake.inFlight == 1 })
    let b = Task { await model.refreshTransfers() }
    try? await Task.sleep(nanoseconds: 150_000_000)

    #expect(fake.maxInFlight == 1, "同一时刻至多一条在飞（约束 15）")
    #expect(fake.callCount("transfer_list") == 1)

    gate.release()
    await a.value
    await b.value
}

@MainActor
@Test func transientEngineRPCFailureDoesNotStallThePolling() async {
    // ⚠️ 审查发现的第二条：内核的 `engine_rpc_failed` **不是**致命码 ——
    //    `on_rpc_failure` 只在**探活成功**时才回它（`core/src/main.rs`），语义是
    //    "这一次调用失败了，可以重试"。壳把它写成 `.unavailable` 的后果是：`pollTick`
    //    的准入是 `engine == .running`，于是**一次抖动永久关掉轮询**，而自愈恰恰需要
    //    一次成功的 `transfer_list` —— 它再也进不去了。
    let fake = FakeCore()
    fake.stub("transfer_list", Wire.transfers)
    let model = await runningModel(fake)

    fake.stub("transfer_list", throwing: .rpc(code: "engine_rpc_failed", message: Wire.rpcFailed))
    await model.pollTick()

    #expect(model.engine == .running, "瞬时的 RPC 失败不得判成引擎不可用")
    #expect(model.lastError == Wire.rpcFailed, "原文仍然要看得见（约束 4）")

    // 下一拍照常问得动 —— 这就是"可重试"
    fake.stub("transfer_list", Wire.transfers)
    await model.pollTick()
    #expect(fake.callCount("transfer_list") == 2, "轮询没有被关掉")
    #expect(model.transfers?.items.first?.gid == "g1")
    #expect(model.lastError == nil, "拿到好数据之后不该再挂着旧错误")
}

@MainActor
@Test func aMalformedResponseDuringAPollDoesNotKillThePolling() async {
    // 同一条纪律的另一半：非抛错路径上的**任何**原文都只落 `lastError`，不写 `engine`。
    let fake = FakeCore()
    fake.stub("transfer_list", Wire.transfers)
    let model = await runningModel(fake)

    fake.stub("transfer_list", throwing: .malformedResponse("内核发来的一行不是合法 JSON：{"))
    await model.pollTick()

    #expect(model.engine == .running, "一行垃圾不是「引擎不可用」")
    #expect(model.lastError == "内核发来的一行不是合法 JSON：{")

    fake.stub("transfer_list", Wire.transfers)
    await model.pollTick()
    #expect(fake.callCount("transfer_list") == 2, "下一拍照常")
}

@MainActor
@Test func aSuccessfulPollKeepsTheEngineRunning() async {
    // ⚠️ **不能用 `runningModel`**（这里以前用的就是它）：那条助手先跑 `enqueue`，
    //    而 `enqueue` 成功时自己就写了 `engine = .running` —— 于是把 `fetchTransfers`
    //    末尾那句 `.running` 删掉，这条断言照样全绿，等于零判别力。
    //    改走 `start()`：握手成功落了 `.notStarted`，`.running` 就**只可能**来自这次
    //    `refreshTransfers()`。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("transfer_list", Wire.transfers)
    let model = AppModel(client: fake)
    await model.start()
    #expect(model.engine == .notStarted, "前置条件：握手刚完、还没 enqueue 过 ⇒ 引擎未启动")

    await model.refreshTransfers()

    #expect(model.transfers?.items.first?.gid == "g1")
    #expect(model.transfers?.global.downloadSpeed == 900)
    #expect(model.engine == .running, "传输列表拿到了 ⇔ 引擎在跑")
}

// ---------------------------------------------------------------------------
// 忙指示
// ---------------------------------------------------------------------------

@MainActor
@Test func busyReasonIsSetWhileLoadingAndClearedAfter() async {
    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    let gate = TestGate()
    fake.gate = gate

    let t = Task { await model.loadDelivery(code: "AAA-1", baseURL: nil) }
    #expect(await waitUntil("加载中") { model.busyReason != nil })
    let during = model.busyReason
    #expect(during != nil && !(during!.isEmpty), "进行中要有话说")

    gate.release()
    await t.value

    #expect(model.busyReason == nil, "结束后必须清掉")
    guard case .loaded(let info) = model.loadState else {
        Issue.record("加载成功应进 .loaded，实际 \(model.loadState)")
        return
    }
    #expect(info.code == "AAA-1")
}

// ---------------------------------------------------------------------------
// 内核崩溃：提示 + 至多一次自动重启
// ---------------------------------------------------------------------------

@MainActor
@Test func kernelDeathTriggersExactlyOneRestart() async {
    let dying = FakeCore()
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let revived = FakeCore()
    revived.stub("hello", Wire.hello)
    revived.stub("get_settings", Wire.getSettings)
    revived.stub("transfer_list", Wire.transfers)
    let factory = ClientFactory([revived])
    let model = await runningModel(dying, makeClient: factory.factory)

    await model.refreshTransfers()          // 管道结束 = 内核进程没了

    #expect(factory.madeCount == 1, "自动重启恰好一次")
    #expect(model.engine == .notStarted, "重启成功：新内核是全新的，还没 enqueue 过")
    #expect(model.transfers == nil, "**不**自动恢复传输列表（规格 §8.3：断点续传恒开）")
    #expect(model.busyReason == nil, "重启做完要把忙指示清掉")
}

@MainActor
@Test func secondKernelDeathDoesNotRestartAgain() async {
    let dying = FakeCore()
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let revived = FakeCore()
    revived.stub("hello", Wire.hello)
    revived.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([revived])
    let model = await runningModel(dying, makeClient: factory.factory)
    await model.refreshTransfers()
    #expect(factory.madeCount == 1)

    // 重启后的内核又死了：**不再自动重启**（否则内核反复崩就是无限重启循环）。
    revived.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    await model.refreshTransfers()

    #expect(factory.madeCount == 1)
    #expect(model.engine == .unavailable("下载引擎已断开：\(Wire.transportEnded)"),
            "把断开这件事说出来（约束 4：不得静默失效）")
}

@MainActor
@Test func restartFailureSurfacesInsteadOfRetryingForever() async {
    let dying = FakeCore()
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let broken = FakeCore()
    broken.failEverything(with: .transport(Wire.transportEnded))
    // ⚠️ 队列里**故意留一个能用的替身**：不然"多试了一次"只会撞上工厂抛的错，
    //    `madeCount` 还是 1 —— 那条断言就变成了恒真（这条测试的第一版正是如此）。
    let spare = FakeCore()
    spare.stub("hello", Wire.hello)
    spare.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([broken, spare])
    let model = await runningModel(dying, makeClient: factory.factory)

    await model.refreshTransfers()

    #expect(factory.madeCount == 1, "只试一次")
    guard case .unavailable(let why) = model.engine else {
        Issue.record("重启失败必须显式置为不可用，实际 \(model.engine)")
        return
    }
    #expect(why.contains("内核重启失败"), "失败原因要看得见：\(why)")

    // 再抖一次也不再试：状态必须**仍然停在"不可用"上**（偷着重启一次会让它变回 .notStarted
    // 或 .running —— 那就是"假装没事"）。
    await model.refreshTransfers()
    #expect(factory.madeCount == 1, "重启失败后不得反复重试")
    guard case .unavailable = model.engine else {
        Issue.record("不得因为一次偷偷的重启而装作没事，实际 \(model.engine)")
        return
    }
}

@MainActor
@Test func kernelDeathRestoresTheLoadedDelivery() async {
    let dying = FakeCore()
    dying.stub("hello", Wire.hello)
    dying.stub("get_settings", Wire.getSettings)
    dying.stub("load_delivery", Wire.deliveryA)
    dying.stub("get_tree", Wire.treeA)
    let revived = FakeCore()
    revived.stub("hello", Wire.hello)
    revived.stub("get_settings", Wire.getSettings)
    revived.stub("load_delivery", Wire.deliveryA)
    revived.stub("get_tree", Wire.treeA)
    let factory = ClientFactory([revived])
    let model = await runningModel(dying, makeClient: factory.factory)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])

    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    await model.refreshTransfers()

    #expect(factory.madeCount == 1)
    // 「恢复交付码与视图」：新内核手里没有清单，不重拉的话界面就是一片空。
    #expect(revived.params("load_delivery") == .object(["code": .string("AAA-1"),
                                                        "base_url": .string("http://dl.example")]),
            "用同一个交付码（和它的 base_url）把视图恢复回来")
    #expect(model.tree?.flat.map(\.path) == ["a.bin"])
    #expect(model.lastCode == "AAA-1")
}

@MainActor
@Test func userReloadResetsTheRestartBudget() async {
    // ⚠️ 这是简报留给我判断的那一条的落点：`restartAttempted` **什么时候复位**。
    //    选择 = 用户的显式动作（重新加载交付码 / 手动重试）才复位，成功的自动重启**不**复位。
    //    理由：自动重启若能自我复位，内核在同一个请求上反复崩就是无限重启循环。
    let dying = FakeCore()
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let revived = FakeCore()
    revived.stub("hello", Wire.hello)
    revived.stub("get_settings", Wire.getSettings)
    revived.stub("load_delivery", Wire.deliveryB)
    revived.stub("get_tree", Wire.treeB)
    let second = FakeCore()
    second.stub("hello", Wire.hello)
    second.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([revived, second])
    let model = await runningModel(dying, makeClient: factory.factory)
    await model.refreshTransfers()
    #expect(factory.madeCount == 1)

    // 用户的显式动作：重新加载交付码 ⇒ 新一轮 ⇒ 预算复位
    await model.loadDelivery(code: "BBB-2", baseURL: nil)
    #expect(model.engine == .notStarted)

    revived.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    await model.refreshTransfers()

    #expect(factory.madeCount == 2, "用户重新开始一轮之后，自动重启预算是新的")
    #expect(model.engine == .notStarted)
}

@MainActor
@Test func manualRetryAfterAFailedRestartRecovers() async {
    let dying = FakeCore()
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    let broken = FakeCore()
    broken.failEverything(with: .transport(Wire.transportEnded))
    let healthy = FakeCore()
    healthy.stub("hello", Wire.hello)
    healthy.stub("get_settings", Wire.getSettings)
    let factory = ClientFactory([broken, healthy])
    let model = await runningModel(dying, makeClient: factory.factory)

    await model.refreshTransfers()
    #expect(factory.madeCount == 1)
    guard case .unavailable = model.engine else {
        Issue.record("重启失败后应处于不可用，实际 \(model.engine)")
        return
    }

    await model.retryEngine()               // 用户点「重试」

    #expect(factory.madeCount == 2)
    #expect(model.engine == .notStarted, "重试成功后引擎可用（只是还没有任务）")
}

// ---------------------------------------------------------------------------
// 收尾
// ---------------------------------------------------------------------------

@MainActor
@Test func shutdownForwardsToTheClient() {
    let fake = FakeCore()
    let model = AppModel(client: fake)

    model.shutdown()
    model.shutdown()          // 幂等由 CoreClient 保证，壳只负责转发

    #expect(fake.shutdownCount == 2)
    #expect(fake.syncCallCount == 0)
}

// ---------------------------------------------------------------------------
// 换交付码（任务 2）
//
// 规格 §3.1 第 3 条是这一节存在的全部理由：**换码失败必须保留原批次**。
// 一个用户手误打错的码，不该把已经加载好、可能正在下载的那一批从界面上弄丢 ——
// 而 `loadDelivery` 今天那条路会先把 `loadState` 置成 `.loading`、失败再置成 `.failed`，
// 于是一次手误就把人踢回了空态页。`switchDelivery` 就是为这条规格补的那条路。
//
// ⚠️ 线上夹具的形状与 `Wire.deliveryA/B` 相同，只有码是简报点名的 "AAAA"/"BBBB" ——
//    `switchDelivery` 回的是 `DeliveryInfo.code`（**内核归一后的那个**），
//    所以夹具里的码必须与请求里的码一致，断言才有意义。
// ---------------------------------------------------------------------------

private let switchWireAAAA = #"""
{"code":"AAAA","page_url":"http://dl.example/AAAA/index.html","base_url":"http://dl.example",
 "created_at":"2026-09-01T10:00:00+08:00","expires_at":"2026-10-01T10:00:00+08:00",
 "expired":false,"total_files":2,"total_bytes":3000,
 "tree":{"type":"dir","name":"AAAA","children":{}}}
"""#

private let switchWireBBBB = #"""
{"code":"BBBB","page_url":"http://dl.example/BBBB/index.html","base_url":"http://dl.example",
 "created_at":"2026-09-02T11:00:00+08:00","expires_at":"2026-10-02T11:00:00+08:00",
 "expired":true,"total_files":5,"total_bytes":4100,
 "tree":{"type":"dir","name":"BBBB","children":{}}}
"""#

@MainActor
@Test("换码失败 ⇒ 原批次一个字都不动")
func aFailedSwitchKeepsTheCurrentBatch() async {
    // 替身：先让 `load_delivery` 成功一次，再让它失败（同文件既有的 `FakeCore`）。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)     // 先加载好一批
    guard case .loaded(let before) = model.loadState else {
        Issue.record("前置条件不成立：第一批没加载上")
        return
    }

    // 内核原文（面板显示的就是这一句，壳不加工，约束 3 / C-7）。
    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed",
                                              message: "拉取交付清单失败：HTTP 404"))

    let outcome = await model.switchDelivery(code: "BBBB", baseURL: nil)

    #expect(outcome == .failed(message: "拉取交付清单失败：HTTP 404"))
    #expect(model.loadState == .loaded(before))              // 原批次原样还在
    #expect(model.lastCode == "AAAA", "记住的码也是原批的（换码没成功）")
    #expect(model.tree?.flat.map(\.path) == ["a.bin"], "树也不得动：换码失败 = 一个状态都不写")
    #expect(model.busyReason == nil, "失败之后不得留下一个没有请求在飞的忙指示")
}

@MainActor
@Test("换码成功 ⇒ 切到新批次，且能下到新批的文件")
func aSuccessfulSwitchCommits() async {
    // 同款替身：第一次 "AAAA" 成功，第二次 "BBBB" 也成功。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)

    fake.stub("load_delivery", switchWireBBBB)
    fake.stub("get_tree", Wire.treeB)
    // ⚠️ 简报写的是 `fake.getTreeCount == 1`，但本仓的替身没有这个"换个码之后的计数"
    //    属性，只有累计的 `callCount(_:)` —— 所以这里断言的是"**又**拉了一次"
    //    （换码前 +1），语义与简报那句一字不差。
    let treesBefore = fake.callCount("get_tree")

    let outcome = await model.switchDelivery(code: "BBBB", baseURL: nil)

    #expect(outcome == .switched(code: "BBBB"))
    // 三件事都要断言：界面进了新批次、记住的码换了、get_tree 被重新拉过一次。
    if case .loaded(let info) = model.loadState {
        #expect(info.code == "BBBB")
    } else {
        Issue.record("换码成功后 loadState 不是 .loaded：\(model.loadState)")
    }
    #expect(model.lastCode == "BBBB")
    #expect(fake.callCount("get_tree") == treesBefore + 1)
    // 只有"拉了树"还不够：手上这份必须是**新批**的树，否则用户以为在下 B 批、下的是 A 批。
    #expect(model.tree?.flat.map(\.path) == ["b.bin"], "新批次的树要真的到位")
}

@MainActor
@Test func aSwitchInFlightKeepsTheCurrentBatchOnScreen() async {
    // 换码**期间**当前批次不得被打回 `.loading` —— 那正是本任务要修的观感：
    // 一次手误、或者一次慢网络，屏上那棵树不该消失。
    //
    // ⚠️ 这条测试的**判别力**是刻意的：没有它，`performLoadDelivery` 里那句
    //    `if mode == .initial { loadState = .loading }` 可以被改回无条件赋值，
    //    而这一节其余测试**全都照样绿**（它们断言的都是"动作做完之后"的状态）。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)
    guard case .loaded(let before) = model.loadState else {
        Issue.record("前置条件不成立：第一批没加载上")
        return
    }

    // 挂住请求：停在"换码进行中"这一刻。
    let gate = TestGate()
    fake.gate = gate
    fake.stub("load_delivery", switchWireBBBB)
    fake.stub("get_tree", Wire.treeB)
    let t = Task { await model.switchDelivery(code: "BBBB", baseURL: nil) }
    #expect(await waitUntil("换码中") { model.switching })

    #expect(model.loadState == .loaded(before), "换码期间原批次必须还在屏上（不是 .loading）")
    #expect(model.busyReason != nil, "但忙指示要有：用户正在等一个结果")

    gate.release()
    let outcome = await t.value
    #expect(outcome == .switched(code: "BBBB"))
    #expect(model.switching == false, "换完之后闸门放开，别把面板那颗按钮永久禁掉")
}

@MainActor
@Test func switchingAfterTheEngineDiedFailsFastAndKeepsTheBatch() async {
    // 现场：批次已经加载好，之后内核死在别处（`engine` 落到 `.unavailable`）。
    // 此时换码必须做到三件事：
    //   ① **一个请求都不发**（内核卡死时那条 FIFO 队列已被堵死，再发只会永远挂着，约束 15）；
    //   ② **不触发重启** —— 那条 `transport` 是**壳自己造的**（内核没死，只是我们不发），
    //      喂进 `absorb` 就会走 `onKernelDeath → restartIfAllowed`，于是"换一次码"变成
    //      "杀掉并重启一次内核"（理由与 `enqueue` 那道引擎闸门逐字同款）；
    //   ③ 原批次一个字都不动，失败那句话交给面板。
    let dying = FakeCore()
    dying.stub("hello", Wire.hello)
    dying.stub("get_settings", Wire.getSettings)
    dying.stub("load_delivery", switchWireAAAA)
    dying.stub("get_tree", Wire.treeA)
    let broken = FakeCore()
    broken.failEverything(with: .transport(Wire.transportEnded))
    let factory = ClientFactory([broken])
    let model = AppModel(client: dying, makeClient: factory.factory)

    await model.loadDelivery(code: "AAAA", baseURL: nil)
    guard case .loaded(let before) = model.loadState else {
        Issue.record("前置条件不成立：第一批没加载上")
        return
    }

    // 内核没了 ⇒ 自动重启恰好一次 ⇒ 重启失败 ⇒ `.unavailable`（重启那条路不复位预算）。
    dying.stub("transfer_list", throwing: .transport(Wire.transportEnded))
    await model.refreshTransfers()
    #expect(factory.madeCount == 1)
    guard case .unavailable(let why) = model.engine else {
        Issue.record("前置现场没搭起来（engine 不是 .unavailable）：\(model.engine)")
        return
    }

    let outcome = await model.switchDelivery(code: "BBBB", baseURL: nil)

    #expect(outcome == .failed(message: why), "内核一句话都没给，就把 engine 里那句原文交给面板")
    #expect(broken.callCount("load_delivery") == 0, "不得发出那条注定挂死的请求")
    #expect(broken.callCount("get_tree") == 0, "树也不去拉：同一条 FIFO 队列，一样会挂死")
    #expect(factory.madeCount == 1, "壳自己造的错误不得触发重启")
    #expect(model.loadState == .loaded(before), "原批次原样还在")
}

// ---------------------------------------------------------------------------
// 换码结果必须活过面板（任务 2 复审重要 ②/③）
//
// 现场是**换码面板的「取消」按钮不被禁用**（`AppModel.switching` 的注释写了理由：
// 面板关不关得掉，不该由一次网络请求决定）。于是那次换码的结果**不能**只活在面板的
// `@State` 里 —— 面板一关就没了：失败方向上是"内核原文零痕迹"（约束 4 明禁的静默失效），
// 成功方向上是"我按了取消、界面自己换了一批"（用户无法归因）。
// ---------------------------------------------------------------------------

@MainActor
@Test func theFailureOutcomeSurvivesTheSheet() async {
    // 先加载好一批，再让换码失败（内核原文）—— 之后**面板被取消**（这里等价于：
    // 不再有人读那个返回值），模型手里那份必须还在。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)
    #expect(model.lastSwitchOutcome == nil, "还没换过码时主区不该挂着一行结果")

    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed",
                                              message: "拉取交付清单失败：HTTP 404"))
    let outcome = await model.switchDelivery(code: "BBBB", baseURL: nil)

    #expect(outcome == .failed(message: "拉取交付清单失败：HTTP 404"))
    #expect(model.lastSwitchOutcome == outcome,
            "结果必须落在模型上（主区那一行读它），而不是只交给面板那个会被销毁的 @State")
    #expect(model.lastSwitchOutcome?.noticeText == "拉取交付清单失败：HTTP 404",
            "主区要显示的就是内核原文逐字（约束 3）")

    // 用户收起那一行（视图上那个 ×）—— 这是**用户主动**的，不算静默失效。
    model.dismissSwitchOutcome()
    #expect(model.lastSwitchOutcome == nil, "收起之后那一行必须真的消失")
}

@MainActor
@Test func theSuccessOutcomeSurvivesTheSheet() async {
    // 成功方向（复审补充的那一半）：用户点了「取消」，请求照跑并**成功**，整批被换掉
    // （`RootView.onChange(of: loadedCode)` 静默复位浏览位置与勾选面）——
    // 主区那一行是这次"界面自己换了"唯一的来源说明。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)

    fake.stub("load_delivery", switchWireBBBB)
    fake.stub("get_tree", Wire.treeB)
    let outcome = await model.switchDelivery(code: "BBBB", baseURL: nil)

    #expect(outcome == .switched(code: "BBBB"))
    #expect(model.lastSwitchOutcome == .switched(code: "BBBB"), "成功那支也要有落点")
    #expect(model.lastSwitchOutcome?.noticeText.contains("BBBB") == true,
            "必须报出**新批次**的码：\(model.lastSwitchOutcome?.noticeText ?? "nil")")
    #expect(model.lastSwitchOutcome?.isFailure == false, "成功那支不得被渲染成失败（图标/颜色靠它）")
}

@MainActor
@Test func startingANewSwitchClearsThePreviousOutcome() async {
    // 主区那一行说的是"最近一次换码"。新一次开始之后还挂着上一次的结论，用户会把它
    // 当成**这一次**的结果 —— 那是一条新的假信息。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)
    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed", message: "第一次失败"))
    _ = await model.switchDelivery(code: "BBBB", baseURL: nil)
    #expect(model.lastSwitchOutcome == .failed(message: "第一次失败"))

    // 第二次换码（挂住，停在"进行中"这一刻）：上一句必须已经被收起。
    let gate = TestGate()
    fake.gate = gate
    fake.stub("load_delivery", switchWireBBBB)
    fake.stub("get_tree", Wire.treeB)
    let t = Task { await model.switchDelivery(code: "BBBB", baseURL: nil) }
    #expect(await waitUntil("换码中") { model.switching })
    #expect(model.lastSwitchOutcome == nil, "新一次换码开始之后，上一次的结论不得还挂在主区")

    gate.release()
    #expect(await t.value == .switched(code: "BBBB"))
    #expect(model.lastSwitchOutcome == .switched(code: "BBBB"))
}

@MainActor
@Test func aSecondSwitchWhileOneIsInFlightIsRefused() async {
    // 延后清单第 9 条：`switchDelivery` 在**模型层**没有重入闸。视图那侧本来就禁用按钮，
    // 这里要的是**结构性**的第二道：两条并发换码会在内核那条 FIFO 队列上排队，而它们
    // 的**响应顺序**与用户看到的顺序可能相反（第二次先回来 ⇒ 屏上是第一批、壳记的是第二批）。
    let fake = FakeCore()
    fake.stub("load_delivery", switchWireAAAA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.loadDelivery(code: "AAAA", baseURL: nil)
    let loadsBefore = fake.callCount("load_delivery")

    // 挂住第一条，让它停在"进行中"这一刻。
    let gate = TestGate()
    fake.gate = gate
    fake.stub("load_delivery", switchWireBBBB)
    fake.stub("get_tree", Wire.treeB)
    let first = Task { await model.switchDelivery(code: "BBBB", baseURL: nil) }
    #expect(await waitUntil("换码中") { model.switching })

    // ⚠️ 第二次**不 `await` 它的返回值**，而是先观察"它有没有动内核"：
    //    去掉重入闸的变异体会让这条也发一条 `load_delivery`，而那条请求正卡在闸门上 ——
    //    直接 `await second` 会把这条测试**焊死**（挂起来，而不是变红）
    //    （实测：变异检查跑满 600 秒超时）。所以观察点是**调用账本**，它没有这个问题。
    let second = Task { await model.switchDelivery(code: "CCCC", baseURL: nil) }
    try? await Task.sleep(nanoseconds: 200_000_000)   // 让第二条跑起来（它全程同步，不需要这么久）

    // 第二次**一个请求都没发**：`load_delivery` 的调用数只有第一条那一次。
    #expect(fake.callCount("load_delivery") == loadsBefore + 1,
            "同时只能有一次换码在飞（第二次不得也发一条）")

    gate.release()
    #expect(await first.value == .switched(code: "BBBB"))
    // ⚠️ 早退**不静默**：返回一句能显示的话，而不是一个"按了没反应"的空壳（约束 4）。
    //    这句话是壳写的（内核没有原文可登：请求根本没发出去），理由在 `switchDelivery` 里。
    let secondOutcome = await second.value
    if case .failed(let why) = secondOutcome {
        #expect(why.contains("进行中"), "早退要说清楚原因：\(why)")
    } else {
        Issue.record("重入的那一次必须回 .failed（一句能显示的话），实际：\(secondOutcome)")
    }
    #expect(model.switching == false, "第一条结束之后闸门要放开")
    #expect(model.loadState != .idle, "早退那条路不得把当前批次弄丢")
}

// ---------------------------------------------------------------------------
// `loadGeneration`：**同码重载也是一次新加载**（阶段 D 任务 B）
//
// ⚠️ 为什么非要一个新信号：同码重载是用户点得到、也真的会发生的一条路
//    （换码面板里重新提交同一个码 / 空态页那颗「重试」/ 内核崩溃后的恢复），
//    而它在**壳已经观察的全部状态上一个字都不变** —— `loadState` 还是
//    `.loaded(同一份 info)`、`loadedCode` 还是那个码、`treeCode` 还是那个码。
//    视图据此**分不出**"刚刚重载过"与"什么都没发生"，于是：
//      - 屏幕上那条已经过期的下载回执（`RootView.downloadNotice`）没人清 ⇒
//        "我重载了，那句报错还挂在上面"（诊断报告 §6.1 的猜想 1，已证实）；
//      - 这一批的默认勾选面不会重播 ⇒ 界面看起来什么都没变（同报告猜想 2，已证实）。
//    它同时是两条修复的**唯一**信号源，所以它只描述"加载成功了几次"，
//    不描述"为什么"（哪种模式、换没换码）—— 那些各自有别的字段管。
// ---------------------------------------------------------------------------

@MainActor
@Test func loadGenerationAdvancesOnEverySuccessfulLoadEvenForTheSameCode() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.start()
    let before = model.loadGeneration

    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    let afterFirst = model.loadGeneration
    #expect(afterFirst == before + 1, "一次成功的加载要推进一代")

    // ⚠️ **同一个码、同一份 info、同一棵树**：壳手上可观察的状态逐字相同，
    //    只有这个计数器在动 —— 这正是它存在的理由。
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.loadGeneration == afterFirst + 1,
            "**同码重载**也是一次新加载（「重载了界面一点没变」那个观感的信号源）")
    guard case .loaded(let info) = model.loadState else {
        Issue.record("前置现场没搭起来：同码重载之后应停在 .loaded，实际 \(model.loadState)")
        return
    }
    #expect(info.code == "AAA-1", "重载的还是那一批 —— 码没变，所以光看它是分不出重载的")
}

@MainActor
@Test func aFailedLoadDoesNotAdvanceTheLoadGeneration() async {
    // ⚠️ 这条是**约束 4 的那一半，别删**：失败的加载**什么都不作废**。
    //    屏幕上那条下载回执（`RootView.downloadNotice`）里的拒绝理由必须留着 ——
    //    留到用户自己收起它，或者留到**一次成功的加载**把它取代。
    //    若失败的加载也推进代数，一句刚说出来的失败原文会被它自己那次失败的加载清掉
    //    —— 那就是约束 4 明禁的静默失效（"我点了重试，理由自己没了"）。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.start()
    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    let afterSuccess = model.loadGeneration

    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed",
                                              message: "拉取交付清单失败：404"))
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.loadGeneration == afterSuccess, "失败的加载不得推进代数（失败理由要留在界面上）")
    // 反证：它确实是一次**失败**（不是"没发请求"被当成没推进）。
    guard case .failed(let why) = model.loadState else {
        Issue.record("前置现场没搭起来：这次加载应当失败，实际 \(model.loadState)")
        return
    }
    #expect(why == "拉取交付清单失败：404", "内核原文照登（约束 3）")
}

@MainActor
@Test func aFailedTreeFetchDoesNotAdvanceTheGeneration() async {
    // ⚠️ **复审重要 1（阶段 D 任务 B 的修复复审）**：`getTree()` 失败时 `surfacing` 会
    //    **吞掉错误**并返回 nil（`AppModel.swift` 的 `surfacing`），而 `getTree` 只在
    //    **成功**时才写 `tree` / `treeCode`（失败时旧快照原样留着）。如果代数照样推进，
    //    就会出现这个形态（复审者在副本里实测构造出来了）：
    //      ① 代数从 1 推到 2；
    //      ② `treeCode` 还是同一个码 —— 那是**同一批的陈旧树** ⇒ `ManifestTracking.seed`
    //         唯一的新鲜度守卫 `treeCode == code` 对它**没有判别力**（它只比"哪一批"，
    //         比不出"同一批、但是上一代的那棵树"）；
    //      ③ 于是视图拿**上一棵树**的 `default_selected` 播下去 ——
    //         拿一份过期结论当现状，正是本任务要消灭的形态。
    //    触发条件（"两次加载之间内核死了 / 引擎翻成不可用"）不是纯理论路径：
    //    `performLoadDelivery` 自己的注释就把 91.5 秒窗口里的 `getTree` 失败当真实情形。
    //
    //    所以"代数只在**整次加载（含 `getTree`）成功**之后才推进"必须是**真话** ——
    //    那条不变量是 `ManifestTracking.seed` 那侧唯一的新鲜度保证，也正是
    //    `loadGeneration` 的文档里承诺的东西。这条测试钉的就是那句话。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)
    await model.start()

    await model.loadDelivery(code: "AAA-1", baseURL: nil)
    #expect(model.loadGeneration == 1, "整次加载（含树）成功 ⇒ 一代")
    let freshDefaults = model.tree?.defaultSelected

    // 视图侧已经为这一代播过种。
    var tracking = ManifestTracking()
    _ = tracking.display(code: "AAA-1")
    let firstSeeding = tracking.seed(code: "AAA-1", treeCode: model.treeCode,
                                     tree: model.tree, generation: model.loadGeneration)
    #expect(firstSeeding?.selection == ["a.bin"], "前置现场没搭起来：第一次要真的播下去")

    // **同码重载，但 `get_tree` 失败**（内核在这两次之间死了 / 引擎翻成不可用）。
    fake.stub("get_tree", throwing: .rpc(code: "engine_disconnected",
                                         message: "下载引擎已断开，重连也没成功：connection refused"))
    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.loadGeneration == 1, "整次加载**没成功**（树没换）⇒ 不得推进代数")
    #expect(model.treeCode == "AAA-1", "树没换成新的：`treeCode` 还是上一代那个（隐患的来源）")
    #expect(model.tree?.defaultSelected == freshDefaults, "旧快照原样留着（`getTree` 只在成功时写）")

    // **关键断言**：拿这一代去问"该播什么"时**必须什么都拿不到** ——
    // 否则视图会用上面那棵"同码、但上一代"的树重播默认勾选面。
    let afterFailedTree = tracking.seed(code: "AAA-1", treeCode: model.treeCode,
                                        tree: model.tree, generation: model.loadGeneration)
    #expect(afterFailedTree == nil, "不得拿陈旧树（同码、上一代）的默认面重播")
}
