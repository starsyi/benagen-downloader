import Foundation

// ---------------------------------------------------------------------------
// 行通道：壳与内核之间那条 stdio 管道的抽象
// ---------------------------------------------------------------------------
//
// 协议是 **JSON Lines**：一行一条消息、`\n` 结尾（`core/src/main.rs` 的 `write_line`）。
// 于是"一条通道"需要的能力只有三件：写一行、读一行、关掉。
//
// ⚠️ **这是测试接缝，不要给它加额外方法**：加了之后每个测试替身都得跟着实现一遍，
//    而它们要证明的东西（配对、告警、EOF、收尾）与通道的实现细节无关。

/// 一条双向的行通道。
///
/// `readLine()` 返回 `nil` 表示 **EOF**（对端关了写端 / 进程已退出）——
/// 它是"内核没了"的唯一可靠信号，必须与"读到一行空串"区分开。
public protocol LineChannel: AnyObject {
    func writeLine(_ s: String) throws
    func readLine() throws -> String?     // nil = EOF
    func close()
}

// ---------------------------------------------------------------------------
// 把一根管道切成行
// ---------------------------------------------------------------------------

/// 行读取器。**同步、阻塞**，一次读一行；EOF 返回 `nil`。
///
/// ⚠️ **不许换成 `Pipe` 的 `readabilityHandler`**：那是个回调，会和同步的
/// `readLine()` 抢同一个 fd（同一个 fd 上两套读法必然互相吞数据）。
/// 这个类型两边都用——stdout 的协议行、stderr 的诊断行——所以它只做"切行"这一件事。
///
/// ⚠️ **也不许换成 `FileHandle.read(upToCount:)`**：它在 Darwin 上落到
/// `-[NSConcreteFileHandle readDataOfLength:]`，那个方法是**读满**语义
/// ——要不到 64 KB 就继续阻塞。而内核是"一请求一响应"的：它不会再多说一个字，
/// 于是壳在等满 64 KB、内核在等下一条请求，**双双卡死**（实测：真内核那三条冒烟
/// 测试全挂在这里，而 `/bin/sh` 的替身因为写完就退出、EOF 把不满的读"补"齐了，
/// 所以它是绿的——这正是"必须拿真内核跑一遍"的价值）。
/// 所以这里直接用 POSIX `read(2)`：它**有数据就返回**。
struct LineReader {
    private let fd: Int32
    private var buffer = Data()

    init(handle: FileHandle) { self.fd = handle.fileDescriptor }

    /// 读一行（不含结尾的 `\n`）。`nil` = EOF。
    mutating func nextLine() throws -> String? {
        while true {
            if let idx = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<idx]
                buffer.removeSubrange(buffer.startIndex...idx)
                return String(decoding: line, as: UTF8.self)
            }
            var chunk = [UInt8](repeating: 0, count: 64 * 1024)
            let n = chunk.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, $0.count) }
            if n > 0 {
                buffer.append(contentsOf: chunk[0..<n])
                continue
            }
            if n < 0 && errno == EINTR { continue }     // 被信号打断：重来，不是错误
            // 其余 n < 0（含强制收尾时读端已被关掉给的 EBADF）一律抛出去，
            // 由 `readLine` 转成显式的 transport 错误——不允许静默失败。
            if n < 0 { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            // n == 0：EOF。缓冲里剩下的半行也交出去——内核总是带换行，但真遇上截断时
            // "把垃圾交给上层去报错"好过"静默吞掉"。
            guard !buffer.isEmpty else { return nil }
            let rest = String(decoding: buffer, as: UTF8.self)
            buffer.removeAll()
            return rest
        }
    }
}

// ---------------------------------------------------------------------------
// 生产形态：一个真子进程 + 三条管道
// ---------------------------------------------------------------------------

/// `Foundation.Process` 形态的行通道：内核对面的 stdout/stdin 是协议通道，
/// stderr 是诊断通道。
///
/// # stderr **必须主动排空**，不只是"接出来"
///
/// 接一根 `Pipe` 却不读它，内核往 stderr 写满管道缓冲区（macOS 上几十 KB）之后会
/// **阻塞在 `write` 上**——它不再读 stdin、也不再写 stdout，整个请求-响应循环就此死锁，
/// 而症状是"壳卡住不动"，极难归因。所以这里有一条**专门的线程**把 stderr 读干，
/// 每读到一行就交给 `onStderrLine`（默认转发到壳自己的 stderr）。
///
/// 这条线程随子进程退出（写端关闭 → EOF）自行结束；`close()` 里有界地等它。
public final class ProcessChannel: LineChannel, @unchecked Sendable {
    private let process: Process
    private let stdinHandle: FileHandle
    private let stdoutHandle: FileHandle
    private let stderrHandle: FileHandle

    private let readLock = NSLock()
    private var stdoutReader: LineReader

    private let writeLock = NSLock()
    private let closeLock = NSLock()
    private var closed = false

    private let stderrQueue: DispatchQueue
    private let stderrGroup = DispatchGroup()

    /// 起一个子进程并接上三条管道。
    ///
    /// - Parameter onStderrLine: 内核 stderr 的每一行。`nil`（默认）时转发到**壳自己的
    ///   stderr**——不是 stdout（那是协议专用通道），也不是丢弃（丢的是排障信息）。
    public init(executableURL: URL, arguments: [String] = [],
                onStderrLine: (@Sendable (String) -> Void)? = nil) throws {
        // -------------------------------------------------------------------
        // ⚠️ **SIGPIPE 必须被忽略，否则内核一死、壳会被信号整个带走。**
        //
        // 现场（`CoreClient.performCall` **只守卫 `requestsClosed`、不守卫"内核已死"**）：
        //   请求 A 在飞 → 用户触发请求 B（排到同一条串行队列）→ 内核死 →
        //   A 的 `readLine()` 拿到 nil、抛 transport → **B 接着跑、往死管道 `writeLine`**。
        //   那一写拿到的是 EPIPE，而进程级的默认处置是**向自己发 SIGPIPE**：
        //   `writeLine` 里那个 `catch`（"向内核写入请求失败（管道已断）：…"）**一行都跑不到**。
        //
        // ⚠️ 这**不只是**一条崩溃：那个 `catch` 产出的 `CoreError.transport` 正是
        //    `AppModel.onKernelDeath` → 自动重启链路的**输入端**。少了这一句，
        //    "内核崩溃后自动重启"在它真正要应对的场景里是**死的** —— 内核一死，
        //    用户看到的不是横幅与「重试」，而是应用凭空消失。
        //    （任务 8 把这个暴露度放大了：200 ms 轮询 + 一颗专门在内核死后才被点的
        //     「重试」，而"内核在两拍之间死掉"只能由一次失败的请求发现 ——
        //     下一拍就是一次死管道写。）
        //
        // 为什么是**进程级**的 `signal(SIGPIPE, SIG_IGN)`，而不是只对这根 fd 下
        // `fcntl(F_SETNOSIGPIPE)`：后者要逐 fd 记账，而**每建一次内核（每次自动重启）
        // 都是一根新管道** —— 漏一处就等于没修，而漏掉时的症状是"应用凭空消失"，
        // 没有任何本地证据。这一句是**幂等**的（重复调用是空操作），且只改变
        // "写向已关闭管道"这一种情形：忽略之后同一次写照常返回 EPIPE，
        // 于是被 `writeLine` 的 `catch` 转成 `CoreError.transport` ——
        // 正是那段代码本来就想给的东西。
        //
        // 放在**建管道的地方**（而不是 `CoreClient.live()`）：谁能拿到一个 `ProcessChannel`，
        // 谁就受它保护 —— 真壳、走查探针、以及本文件那条
        // `writingToADeadChildsPipeSurfacesAsATransportErrorAndTheShellSurvives` 都走这一个入口。
        // -------------------------------------------------------------------
        signal(SIGPIPE, SIG_IGN)

        let stdinPipe = Pipe()
        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()

        let p = Process()
        p.executableURL = executableURL
        p.arguments = arguments
        p.standardInput = stdinPipe
        p.standardOutput = stdoutPipe
        p.standardError = stderrPipe
        do {
            try p.run()
        } catch {
            // 起不来：三根管道都在我们手上，关掉，别把 fd 漏在那儿
            try? stdinPipe.fileHandleForWriting.close()
            try? stdoutPipe.fileHandleForReading.close()
            try? stderrPipe.fileHandleForReading.close()
            throw CoreError.transport(
                "内核进程启动失败（\(executableURL.path)）：\(error.localizedDescription)")
        }

        self.process = p
        self.stdinHandle = stdinPipe.fileHandleForWriting
        self.stdoutHandle = stdoutPipe.fileHandleForReading
        self.stderrHandle = stderrPipe.fileHandleForReading
        self.stdoutReader = LineReader(handle: stdoutPipe.fileHandleForReading)

        let forward = onStderrLine ?? Self.forwardToOurStderr
        let queue = DispatchQueue(label: "com.benagen.core-client.stderr")
        self.stderrQueue = queue

        // 排水线程。**在 `run()` 成功之后才起**：起失败时写端由上面关掉，
        // 否则这条线程会永远阻塞在一根没人写的管道上。
        let stderrReadHandle = stderrPipe.fileHandleForReading
        let group = stderrGroup
        group.enter()
        queue.async {
            defer { group.leave() }
            var reader = LineReader(handle: stderrReadHandle)
            while true {
                // `try?` 会把 `String?` 拍平成一个 `String?`：读失败与 EOF 一起收场
                guard let line = try? reader.nextLine() else { break }
                forward(line)
            }
        }
    }

    deinit { close() }

    // MARK: - LineChannel

    /// 写一行。
    ///
    /// ⚠️ 这个 `catch` **只有在 SIGPIPE 被忽略时才真的可达**（见 `init` 里那一段）：
    ///    内核已死时这一写会拿到 EPIPE，而进程级的默认处置是**先被信号杀掉**、
    ///    根本轮不到这里。所以"写入失败会变成一条 transport 错误"这句话，
    ///    靠的是 `init` 里那句 `signal(SIGPIPE, SIG_IGN)` —— 两者是一对，别拆。
    public func writeLine(_ s: String) throws {
        writeLock.lock()
        defer { writeLock.unlock() }
        do {
            try stdinHandle.write(contentsOf: Data((s + "\n").utf8))
        } catch {
            throw CoreError.transport(
                "向内核写入请求失败（管道已断）：\(error.localizedDescription)")
        }
    }

    public func readLine() throws -> String? {
        readLock.lock()
        defer { readLock.unlock() }
        do {
            return try stdoutReader.nextLine()
        } catch {
            // 读端被关掉（强制收尾）时这里会拿到 EBADF；子进程没了则是 EOF（走 nil 那条）。
            // 两条都变成一条**显式的** transport 错误交给上层——绝不允许静默失败。
            throw CoreError.transport(
                "读取内核响应失败（管道已断）：\(error.localizedDescription)")
        }
    }

    /// 收尾，**幂等**，且有上界：
    ///
    /// ① 关 stdin 写端 → 内核主循环读到 EOF 自行收尾退出（它自己会关掉 aria2）；
    /// ② 有界地等它退出，超时 `terminate()`（SIGTERM），再超时 `SIGKILL` 兜底；
    /// ③ 等排水线程收到 EOF 结束；
    /// ④ 关读端。
    ///
    /// ⚠️ **不要在排水线程里调 `close()`**（步骤 ③ 要等那条线程结束）。
    ///
    /// ⚠️ **`close()` 会在"有人正阻塞在 `readLine()` 上"时被调用**——`CoreClient.shutdown()`
    /// 的强制收尾路径就是这样（内核活着但不答，磁盘队列被占着）。所以顺序是
    /// "先让子进程死掉（①②），再关读端（④）"：子进程一死，那个卡住的读就拿到 EOF。
    /// 万一它拿到的还是 EBADF（读端已关），`readLine` 会把它转成显式的 transport 错误
    /// ——两种结局都是"在飞请求立刻以错误收场"，不会留下静默失败的路径。
    public func close() {
        closeLock.lock()
        if closed { closeLock.unlock(); return }
        closed = true
        closeLock.unlock()

        // ① 给内核一个"正常收尾"的机会（它要关 aria2、落盘状态）
        try? stdinHandle.close()

        // ② 有界等待 → SIGTERM → SIGKILL
        if !waitForExit(3.0) {
            process.terminate()
            if !waitForExit(2.0) {
                kill(process.processIdentifier, SIGKILL)
                _ = waitForExit(1.0)
            }
        }

        // ③ 排水线程随 EOF 结束（子进程已经没了，写端已关，它马上就返回）
        _ = stderrGroup.wait(timeout: .now() + 2.0)

        // ④ 只在子进程确实已经退出时才关读端：否则可能有线程正阻塞在 read 上，
        //    关掉 fd 会让它拿到 EBADF（宁可漏两个 fd，也不要在排水线程里炸）。
        if !process.isRunning {
            try? stdoutHandle.close()
            try? stderrHandle.close()
        }
    }

    /// 轮询等子进程退出，返回它是否已经退出。
    ///
    /// 不用 `waitUntilExit()`：它没有超时，内核卡住时会把壳一起卡住，
    /// 而这里要的是"有上界"（`Process` 的退出监控会自行把 `isRunning` 置假，实测 ~10ms）。
    private func waitForExit(_ seconds: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while process.isRunning && Date() < deadline {
            usleep(10_000)
        }
        return !process.isRunning
    }

    /// 默认的 stderr 去处：壳自己的 stderr，带前缀。
    ///
    /// 用 `fputs` 而不是 `FileHandle.standardError.write`：后者在写端已关时可能抛
    /// ObjC 异常（Swift 接不住），而这里**绝不允许**因为写日志把排水线程弄死。
    private static func forwardToOurStderr(_ line: String) {
        fputs("[benagen-core] \(line)\n", stderr)
    }
}
