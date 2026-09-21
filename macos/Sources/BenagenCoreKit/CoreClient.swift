import Foundation

// ---------------------------------------------------------------------------
// 驱动内核子进程：发一条请求、等它那条响应、收尾
// ---------------------------------------------------------------------------
//
// 内核是 `benagen-core`（Rust，225 个测试），壳通过 stdio 上的 JSON Lines 协议驱动它。
// 这一层只做四件事，**不含任何业务判断**（约束 1）：
//
//   1. 分配请求 id（从 1 起，0 是内核的保留值）；
//   2. 把请求写成一行、读回配对的那一行；
//   3. 把 `id == 0` 的协议级错误留出来（它不是任何请求的结果）；
//   4. 有界地收尾，别把子进程留成孤儿。
//
// **同一时刻至多一条在飞的请求**（约束 15）由一条 `.serial` 队列保证。
// `callAsync` 只是把同一段代码甩到后台，**不提高并发度**：
// 内核的信号量只有一条管道，两条请求同时发出去只会让响应配对失序。

public final class CoreClient: CoreCalling, @unchecked Sendable {
    private let channel: LineChannel

    /// **串行**队列（`DispatchQueue` 默认就是 `.serial`）：约束 15 的那条"单飞"语义
    /// 全靠它。`callSync` 与 `callAsync` 走的是同一条，所以两者之间也是互斥的。
    private let queue: DispatchQueue

    /// 下一个请求 id。只在 `queue` 上读写。
    private var nextId: UInt64 = 1

    /// `shutdown()` 的"只做一次"与"关掉之后不再收新请求"两个标志。
    ///
    /// ⚠️ 用**锁**而不是队列：`shutdown()` 的强制收尾路径（队列被在飞请求占着时）
    /// 是在调用方线程上写的，而 `performCall` 在队列上读——两者必须互斥，否则是数据竞争。
    private let stateLock = NSLock()
    private var shutdownStarted = false
    private var closedForRequests = false

    /// 协议级错误（`id == 0`）。用锁而不是队列：读它的是界面线程，
    /// 而 `queue.sync` 在读的时候会把界面卡在一条慢请求后面。
    private let alertLock = NSLock()
    private var alerts: [ErrorBody] = []

    /// 测试用：接一条现成的通道。
    public init(channel: LineChannel) {
        self.channel = channel
        self.queue = DispatchQueue(label: "com.benagen.core-client", qos: .userInitiated)
    }

    deinit {
        // 不在 deinit 里走 `queue.sync`：最后一次引用有可能正好在队列块里被释放，
        // 那时同步派发会自己锁死。关掉通道就够了——内核读到 stdin 的 EOF 会自行退出。
        channel.close()
    }

    // MARK: - 生产形态

    /// 起一个真内核（`locateCoreBinary()` + `ProcessChannel`），用内核自己的默认值
    /// （下载目录 `~/Downloads/Benagen`、设置 `~/Library/Application Support/…`）。
    public static func live() throws -> CoreClient {
        try live(settingsPath: nil, downloadDir: nil)
    }

    /// 起一个真内核，并指定"设置存哪"与"文件下到哪"。
    ///
    /// 两者都是**内核的输入**而不是协议消息，所以走 argv（结构化传参，不经 shell）。
    public static func live(settingsPath: String?, downloadDir: String?) throws -> CoreClient {
        let binary = try locateCoreBinary()
        let channel = try ProcessChannel(
            executableURL: binary,
            arguments: coreArguments(settingsPath: settingsPath, downloadDir: downloadDir))
        return CoreClient(channel: channel)
    }

    /// 内核的 argv（顺序与 `core/src/main.rs` 的 `parse_args` 一致）。
    static func coreArguments(settingsPath: String?, downloadDir: String?) -> [String] {
        var args: [String] = []
        if let downloadDir { args += ["--download-dir", downloadDir] }
        if let settingsPath { args += ["--settings", settingsPath] }
        return args
    }

    // MARK: - 找内核

    /// 找 `benagen-core` 可执行文件。三处，**按顺序**：
    ///
    /// ① 环境变量 `BENAGEN_CORE`（"临时换一个内核来试"用）；
    /// ② 应用包内 `Contents/Resources/benagen-core`（任务 4 的打包脚本产出）；
    /// ③ 仓库内 `core/target/release/benagen-core`（**仅供 `swift run` 开发用**）。
    ///
    /// 都没有就抛 `CoreError.transport`，**把找过的三个路径都写进消息**：
    /// "点了没反应"最常见的原因就是它，而约束 4 要求这句话出现在界面上。
    public static func locateCoreBinary() throws -> URL {
        try locateCoreBinary(environment: ProcessInfo.processInfo.environment, bundle: .main)
    }

    static func locateCoreBinary(environment: [String: String], bundle: Bundle) throws -> URL {
        for url in coreBinaryURLs(environment: environment, bundle: bundle) {
            if let url, FileManager.default.isExecutableFile(atPath: url.path) { return url }
        }
        throw coreNotFoundError(examined: coreBinaryCandidatePaths(environment: environment,
                                                                   bundle: bundle))
    }

    /// 三条候选路径，**顺序即查找顺序**。未设置的项是 `nil`（跳过）。
    static func coreBinaryURLs(environment: [String: String], bundle: Bundle) -> [URL?] {
        [
            environment["BENAGEN_CORE"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) },
            bundle.bundleURL.appendingPathComponent("Contents/Resources/benagen-core"),
            repoCoreBinaryURL(),
        ]
    }

    /// 三条候选路径的**人话形式**（写进"找不到"的错误消息）。
    ///
    /// 与 [`coreBinaryURLs`] 逐项对应，只差"环境变量没设"这一格的说明。
    static func coreBinaryCandidatePaths(environment: [String: String], bundle: Bundle) -> [String] {
        [
            environment["BENAGEN_CORE"].flatMap { $0.isEmpty ? nil : $0 }
                ?? "(环境变量 BENAGEN_CORE 未设置)",
            bundle.bundleURL.appendingPathComponent("Contents/Resources/benagen-core").path,
            repoCoreBinaryURL().path,
        ]
    }

    static func coreNotFoundError(examined: [String]) -> CoreError {
        .transport("找不到内核可执行文件 benagen-core（找过：\n"
                   + examined.joined(separator: "\n") + "）")
    }

    /// 仓库内的开发构建：`#filePath` 是 `…/macos/Sources/BenagenCoreKit/CoreClient.swift`，
    /// 上溯到仓库根（4 层）再进 `core/target/release/`。
    static func repoCoreBinaryURL() -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()      // …/BenagenCoreKit
            .deletingLastPathComponent()      // …/Sources
            .deletingLastPathComponent()      // …/macos
            .deletingLastPathComponent()      // 仓库根
            .appendingPathComponent("core/target/release/benagen-core")
    }

    /// ③ 是否存在（可执行）。给测试的 `.enabled(if:)` 用——`core/target` 是构建产物，
    /// 在别的机器上可能没有，那时真内核那几条会**跳过**（任务 11 有无条件的那条兜底）。
    public static func devCoreBinaryExists() -> Bool {
        FileManager.default.isExecutableFile(atPath: repoCoreBinaryURL().path)
    }

    // MARK: - 请求

    /// 协议级错误（`id == 0`，内核回给"连 id 都读不出来"的输入）。
    public var protocolAlerts: [ErrorBody] {
        alertLock.lock()
        defer { alertLock.unlock() }
        return alerts
    }

    @discardableResult
    public func callSync(_ method: String, _ params: JSONValue) throws -> JSONValue {
        try queue.sync { try performCall(method, params) }
    }

    @discardableResult
    public func callAsync(_ method: String, _ params: JSONValue) async throws -> JSONValue {
        try await withCheckedThrowingContinuation { cont in
            // 直接排到**同一条**串行队列上：这就是"不提高并发度"的实现方式。
            queue.async {
                do { cont.resume(returning: try self.performCall(method, params)) }
                catch { cont.resume(throwing: error) }
            }
        }
    }

    /// 请求-响应循环。**必须在 `queue` 上跑**（它读写 `nextId`，且独占通道）。
    private func performCall(_ method: String, _ params: JSONValue) throws -> JSONValue {
        guard !requestsClosed else {
            throw CoreError.transport("内核连接已关闭（shutdown 已调用）")
        }

        let id = nextId
        nextId += 1
        try channel.writeLine(try RequestLine.encode(id: id, method: method, params: params))

        while true {
            guard let line = try channel.readLine() else {
                throw CoreError.transport("内核进程已退出（管道结束）")
            }
            let env = try decodeEnvelope(line)

            // `id == 0` 是内核的保留值（连 id 都读不出来的畸形输入）。它不是
            // 任何请求的结果，所以既不能当结果、也不能当异常：留进告警，接着等自己那条。
            if env.id == 0 {
                if let error = env.error { appendAlert(error) }
                continue
            }

            // 单飞语义下"别人的响应"不可能出现。真出现就是收发失步——
            // 把一条无关的 result 当成自己的（界面显示另一个请求的数据）比吵一句糟得多。
            guard env.id == id else {
                throw CoreError.malformedResponse(
                    "内核回的响应 id 是 \(env.id)，而当前在等 \(id)（请求-响应失步）")
            }
            // ⚠️ `ok:false` 与"缺 result"两条守卫**只此一份**（在 `RawEnvelope.unwrapResult`），
            // 不在这里重新内联——复制粘贴的文案会静默分叉（见那段注释）。
            // 也**不**用 `unwrap`：那会为了要一个 `JSONValue` 来回编码一趟，而
            // `.integer` / `.number` 是分开的两个 case，二次编解码会丢这个区分。
            return try env.unwrapResult()
        }
    }

    private func decodeEnvelope(_ line: String) throws -> RawEnvelope {
        do {
            return try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(line.utf8))
        } catch {
            // 截断到 200 字符：这一行可能是几 MB 的垃圾，而它会显示在界面上
            throw CoreError.malformedResponse("内核发来的一行不是合法 JSON：\(line.prefix(200))")
        }
    }

    private func appendAlert(_ error: ErrorBody) {
        alertLock.lock()
        alerts.append(error)
        alertLock.unlock()
    }

    // MARK: - 收尾

    /// `shutdown()` 里"等正在跑的请求让出队列"的上界。超过它就不等了，直接强制关通道。
    ///
    /// 2 秒的理由：正常情况这条路径**立刻**返回（队列空闲）；只有"内核活着但不答"
    /// （在跑 `load_delivery` 那种最长约 91.5 秒的请求）时才会走满，而那时多等无益。
    static let shutdownGraceSeconds: Double = 2.0

    /// 发 `shutdown` → 关通道。**幂等**，且**任何情况下都有上界**。
    ///
    /// ⚠️ **不等它的响应**：内核可能已经退出、管道已经断了，而 `readLine()` 没有超时
    /// ——等下去会把"退出应用"变成"卡住不动"。内核收到 EOF 一样会收尾（关 aria2、落盘），
    /// 所以"没读到那条响应"不影响正确性。
    ///
    /// ⚠️ **也不能无界地等队列**：内核正在跑长请求（`load_delivery` 实测约 91.5 秒 /
    /// `verify`）时，用户点退出，那条请求正卡在 `readLine` 上占着串行队列——
    /// 而唯一能让它松开的 `channel.close()` 就在等队列之后：`queue.sync` 会把收尾
    /// 路径堵死，症状是"点了没反应"（约束 4）。所以：
    ///
    ///  1. 先在**有界**的时间里把 `shutdown` 请求塞进队列（正常路径，毫秒级）；
    ///  2. 超时就走**强制收尾**：不等队列，直接关通道——`ProcessChannel.close()` 内部
    ///     有界（关 stdin → 等 3 s → SIGTERM → 等 2 s → SIGKILL），子进程被回收后
    ///     那条卡住的读会以 EOF 收场，队列随之让出。
    ///
    /// 最坏耗时约 8 秒（2 + 3 + 2 + 1），正常路径 < 10 毫秒。
    public func shutdown() {
        stateLock.lock()
        if shutdownStarted { stateLock.unlock(); return }
        shutdownStarted = true
        stateLock.unlock()

        // ① 有界地等队列空出来：把 shutdown 请求发出去（它同时也让队列别再收新请求）
        let handled = DispatchSemaphore(value: 0)
        queue.async {
            let request = try? RequestLine.encode(id: self.nextId, method: "shutdown",
                                                  params: .object([:]))
            if let request { try? self.channel.writeLine(request) }
            self.nextId += 1
            self.closeRequests()
            handled.signal()
        }
        if handled.wait(timeout: .now() + Self.shutdownGraceSeconds) == .success {
            channel.close()     // 队列已空：此刻没有人在读，通道从容收尾
            return
        }

        // ② 超时：队列被一条在飞请求占着。不等了——`close()` 会把子进程收掉，
        //    那条卡在读上的请求会拿到 EOF 并抛出 transport 错误，队列随即让出。
        //
        //    ⚠️ 这一路的代价**必须显式接住**：关读端时那个读线程可能正阻塞在 read(2) 上，
        //    它会拿到 EOF 或 EBADF——两条路都被 `LineReader` 显式转成
        //    `CoreError.transport("读取内核响应失败（管道已断）…")` 或上面的 EOF 错误，
        //    **不会**变成静默失败（没有哪条路径会静默吞掉它）。
        closeRequests()
        channel.close()
    }

    private func closeRequests() {
        stateLock.lock()
        closedForRequests = true
        stateLock.unlock()
    }

    private var requestsClosed: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return closedForRequests
    }
}
