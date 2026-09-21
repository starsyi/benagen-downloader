import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 任务 11：端到端验收 —— 真内核 + 真 aria2c + 本地桩服务，**由壳自己的 CoreClient 驱动**
// ---------------------------------------------------------------------------
//
// 这一条验的是"客户端真的能交付"：它跑的不是替身，而是 `.app` 里的那两层
// （`CoreClient` + 协议类型）加上真内核、真 aria2c。
//
//   真 delivery_manifest.py 产清单 → python3 -m http.server 当桩 → 真 benagen-core
//   → 落盘 → 回读路径 → 校验 → 状态文件 → 收尾（不留孤儿）
//
// ⚠️ **三条最容易违反的硬约束：**
//
//   1. **绝不用 `CoreClient.live()` 的默认值。** 默认下载目录是 `$HOME/Downloads/Benagen`
//      （`core/src/main.rs:1619`），而那正是人类伙伴真实测试交付下来的目录
//      （`.benagen-state.json` 就在里面）。用默认值跑这条 e2e = **拿一条夹具清单覆盖真实
//      状态文件**，是本任务唯一能造成真实数据破坏的地方。这里一律走
//      `CoreClient.live(settingsPath:downloadDir:)` 把两个目录都指到临时目录——
//      并且有一条断言直接守着它（见步骤 4 的「内核 argv」）。
//
//   2. **夹具清单必须由仓库根的真实生成器产出**（`delivery_manifest.py` 的
//      `build_manifest` + `dump_manifest`，即 `tosup.v6.5.py` 生成交付页走的同一条路径），
//      **不得手写 JSON 字面量**——行为契约 §3.1 的「必须用一条真实 manifest 抽验」。
//      `python3` 取不到时**直接失败**，不静默降级。
//
//   3. **本测试无条件执行**（不是 `.enabled(if: CoreClient.devCoreBinaryExists())`）。
//      `core/target/release/benagen-core` 不在时它会红——这正是要的：内核没构建
//      是**脚本该报的错**（`macos/scripts/e2e_shell_macos.sh` 先 `cargo build --release`），
//      不是一条静默跳过的测试。
//
// ⚠️ **判别力（约束 12）：夹具里的"非规范路径"是按断言的观测面挑的。**
//   - **字符串面**的断言（`list_dir` / `get_tree` 回来的 `path` == 清单原文）用
//     `PFX/./Note.txt`：会规范化路径的实现给出 `PFX/Note.txt`，字符串一比就露。
//   - **文件系统面**的断言（落盘路径逐字一致）**不能**用 `./` —— `Path.write("a/./b.txt")`
//     与 `Path.write("a/b.txt")` 落**同一个 inode**，操作系统自己就把 `./` 吃掉了，
//     规范化与不规范化结果一样，那条断言会变成**一条什么都验不出的绿灯**。
//     用的是"规范化实现会落到**别处**"的形态：`×` / 空格 / 字面 `%`。
//     内核把 `×` 编成 `%C3%97`、空格编成 `%20`、`%` 编成 `%25`，所以一个
//     **把 URL 编码后的串写到盘上**的实现会造出 `QC%20%E5%9B%BE.png` —— 逐字断言立刻红。
//
// ⚠️ **`python3 -m http.server` 会对请求路径做 `normpath`**：`/C/a/./b.txt` 被它折成
//    `/C/a/b.txt`。所以 `./` 那一条**只在清单字符串里有戏**，别指望它经 HTTP 往返还活着。
//    反过来说：`PFX/./Note.txt` 仍然下得下来（桩按折叠后的路径供文件、内核按原文落盘），
//    只是落盘位置由操作系统折叠成 `PFX/Note.txt`。
//
// ⚠️ 跑法：`bash macos/scripts/test.sh`（聚焦：`bash macos/scripts/test.sh --filter EndToEnd`）。
//    **不要裸调 `swift test`**：本机宏插件路径问题会让它约一半概率随机红，而且
//    `--filter` 打错字会以 0 退出、空跑绿灯（脚本把这种情况判为失败）。

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 交付码：**恰好 20 个字符**、字符集 `[A-Za-z0-9-_.]`（`delivery.rs` 的
/// `CODE_LEN` + `is_code_chars`，即 `delivery::extract_code` 的校验面）。
private let kE2ECode = "AbCdEfGhIjKlMnOpQrSt"

/// 清单里声明的**原文路径**与内容。
///
/// 路径是刻意挑的（见文件头的判别力说明），不是随手造的样例数据：
///   ① 根级文件带 `×` 与空格 —— 让 `list_dir` **根**那一层就有文件项可断言；
///   ② `PFX/./Note.txt` —— 字符串面的判别夹具（规范化实现会给出 `PFX/Note.txt`）；
///   ③ `PFX/100% 覆盖.bin` —— 字面 `%`（`%`→`%25`）+ 空格 + 中文，文件系统面的判别夹具；
///   ④ `test/C24-8_×_25WS024/Figure/QC 图.png` —— 仓库里现成的**真实**交付路径形态。
private let kE2EFixture: [(path: String, body: String)] = [
    ("Readme ×.txt", "根级文件：文件名里有 × 与空格\n"),
    ("PFX/Readme.txt", "readme\n"),
    ("PFX/./Note.txt", "note\n"),
    ("PFX/100% 覆盖.bin", "字面百分号 + 空格 + 中文\n"),
    ("PFX/a/b/deep.txt", "深层嵌套\n"),
    ("test/C24-8_×_25WS024/Figure/QC 图.png", "× 与空格在目录名里\n"),
]

/// 夹具路径的集合与字节总数（断言里反复用，算一次）。
private var kE2EPaths: Set<String> { Set(kE2EFixture.map(\.path)) }
private var kE2ETotalBytes: Int64 { Int64(kE2EFixture.reduce(0) { $0 + $1.body.utf8.count }) }

/// 走**服务端那条生成路径**（仓库根的 `delivery_manifest.py`）产出夹具 + 清单，返回清单 JSON。
///
/// 传出去的是 base64 的内容：crc64 由 python 侧对**真实字节**现算（
/// 期望值不是抄来的常量——抄常量会让"夹具与实现对不上"悄无声息；
/// 而且内核的 `verify` 会独立复算一遍 crc64，算错当场判 `bad`，这条 e2e 会红）。
private func makeFixture(stubRoot: URL, baseURL: String) throws -> [String: Any] {
    let payload: [String: Any] = [
        "code": kE2ECode,
        "base_url": baseURL,
        "stub_root": stubRoot.path,
        "files": kE2EFixture.map { ["path": $0.path,
                                    "data_b64": Data($0.body.utf8).base64EncodedString()] },
    ]

    let script = #"""
    import base64, json, os, sys
    sys.path.insert(0, sys.argv[1])
    from delivery_manifest import build_manifest, dump_manifest

    def crc64xz(data):
        crc = 0xFFFFFFFFFFFFFFFF
        for b in data:
            crc ^= b
            for _ in range(8):
                crc = (crc >> 1) ^ (0xC96C5795D7870F42 if crc & 1 else 0)
        return crc ^ 0xFFFFFFFFFFFFFFFF

    # 自检：两个**独立来源**的已知值。实现写错就当场失败，
    # 不允许"安静地产出一份 crc64 全错的清单"——那会让整条 e2e 变成假绿。
    assert crc64xz(b"123456789") == 0x995DC9BBDF1939FA, "crc64xz 与标准校验值不符"
    assert crc64xz(b"readme") == 5432380796884633278, "crc64xz 与 TOS 实测值不符"

    spec = json.load(sys.stdin)
    code_dir = os.path.join(spec["stub_root"], spec["code"])
    os.makedirs(code_dir, exist_ok=True)

    entries = []
    for f in spec["files"]:
        data = base64.b64decode(f["data_b64"])
        dest = os.path.join(code_dir, f["path"])      # 清单原文落盘
        parent = os.path.dirname(dest)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(dest, "wb") as fh:
            fh.write(data)
        entries.append((f["path"], len(data), str(crc64xz(data))))

    m = build_manifest(spec["code"], entries, spec["base_url"])
    dump_manifest(m, code_dir)
    json.dump(m, sys.stdout, ensure_ascii=False)
    """#

    // 仓库根：与 `CoreClient.repoCoreBinaryURL()` 同源（……/core/target/release/benagen-core
    // 上溯 4 层），不另写一份"仓库在哪"的判断。
    let repoRoot = CoreClient.repoCoreBinaryURL()
        .deletingLastPathComponent()   // benagen-core → release
        .deletingLastPathComponent()   // release → target
        .deletingLastPathComponent()   // target → core
        .deletingLastPathComponent()   // core → 仓库根

    // ⚠️ **SIGPIPE 必须被忽略**（理由同 `ProcessChannel.init` 里那一大段，只是这里更直接）：
    //    python 若在启动瞬间就失败退出（例如 `delivery_manifest.py` 导不进来），
    //    下面那次 `write` 拿到的就是 EPIPE —— 而进程级的默认处置是**向自己发 SIGPIPE**，
    //    整个测试进程会被信号带走（`swift test` 那一轮就此崩掉，且看起来像"崩溃"而不是"失败"）。
    //    这一句是幂等的，放在这里是为了**不依赖**别的测试恰好已经调过 `ProcessChannel.init`。
    signal(SIGPIPE, SIG_IGN)

    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/env")   // python3 在 PATH 里，位置不写死
    p.arguments = ["python3", "-c", script, repoRoot.path]
    let stdinPipe = Pipe(), stdoutPipe = Pipe(), stderrPipe = Pipe()
    p.standardInput = stdinPipe
    p.standardOutput = stdoutPipe
    p.standardError = stderrPipe
    try p.run()

    stdinPipe.fileHandleForWriting.write(try JSONSerialization.data(withJSONObject: payload))
    try stdinPipe.fileHandleForWriting.close()

    let out = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
    let err = stderrPipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()

    guard p.terminationStatus == 0 else {
        throw FixtureError.pythonFailed(
            "delivery_manifest.build_manifest 失败（**不静默降级成手写清单**）：\n"
            + String(decoding: err, as: UTF8.self))
    }
    guard let obj = try JSONSerialization.jsonObject(with: out) as? [String: Any] else {
        throw FixtureError.pythonFailed("生成器没有吐出一份 JSON 对象："
                                        + String(decoding: out, as: UTF8.self))
    }
    // 自检：清单确实来自那条生成路径，且**非规范路径被逐字保留**（约束 3）。
    guard obj["code"] as? String == kE2ECode,
          let files = obj["files"] as? [[String: Any]],
          Set(files.compactMap { $0["path"] as? String }) == kE2EPaths else {
        throw FixtureError.pythonFailed(
            "生成器给出的清单与夹具对不上（code/路径集合）：\(obj["code"] ?? "nil") "
            + "\(((obj["files"] as? [[String: Any]]) ?? []).compactMap { $0["path"] as? String })")
    }
    return obj
}

enum FixtureError: Error { case pythonFailed(String) }

// ---------------------------------------------------------------------------
// 桩服务：python3 -m http.server
// ---------------------------------------------------------------------------

/// 在夹具根里起一个 `python3 -m http.server`。
///
/// **端口由服务自己报**（`python3 -u -m http.server 0` 会打印
/// `Serving HTTP on 127.0.0.1 port NNNN`）——先抢一个空闲端口再交给它是有竞态的
/// （抢到与用上之间可能被别的进程拿走），而端口报错时的症状是"清单拉不到"，
/// 与"桩没起来"分不开。
private final class StubServer {
    let process: Process
    let port: Int
    var base: String { "http://127.0.0.1:\(port)" }

    private let logURL: URL

    private init(process: Process, port: Int, logURL: URL) {
        self.process = process
        self.port = port
        self.logURL = logURL
    }

    static func start(serving directory: URL, logURL: URL) throws -> StubServer {
        FileManager.default.createFile(atPath: logURL.path, contents: Data())
        let log = try FileHandle(forWritingTo: logURL)
        let errLog = try FileHandle(forWritingTo: logURL)

        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        // `-u` 是必须的：stdout 被重定向到文件时 python 会改成块缓冲，
        // 那行端口播报要等缓冲区满才落盘——读日志的循环会白等到超时。
        p.arguments = ["python3", "-u", "-m", "http.server", "0", "--bind", "127.0.0.1"]
        p.currentDirectoryURL = directory
        // 不从测试进程继承 stdin：桩服务不读它，而"继承"意味着它的读端握在别人手里
        p.standardInput = FileHandle.nullDevice
        p.standardOutput = log
        p.standardError = errLog
        try p.run()

        let deadline = Date().addingTimeInterval(10)
        var port: Int?
        while port == nil && Date() < deadline {
            if let text = try? String(contentsOf: logURL, encoding: .utf8) {
                port = announcedPort(in: text)
            }
            if p.isRunning == false { break }        // 起不来就别干等
            usleep(50_000)
        }
        guard let port else {
            p.terminate()
            let text = (try? String(contentsOf: logURL, encoding: .utf8)) ?? ""
            throw FixtureError.pythonFailed("桩服务没有报出端口就退出了：\n" + text)
        }
        return StubServer(process: p, port: port, logURL: logURL)
    }

    /// 从播报行里取端口：`Serving HTTP on 127.0.0.1 port 52791 (http://…) ...`
    ///
    /// ⚠️ **要等到整行都到齐**：日志是**边跑边读**的，半行里取数字会把 `52791`
    /// 读成 `52`（然后连到一个根本不是桩服务的端口上，症状是"10 秒都没服务起来"）。
    /// 判据是那一行末尾的 `(http://` 也出现了——python 是一次 `write(2)` 把整行写出去的，
    /// 两截必然同时在（不依赖任何跨调用的原子性假设之外的东西）。
    private static func announcedPort(in log: String) -> Int? {
        guard log.contains("(http://"),
              let marker = log.range(of: "Serving HTTP on"),
              let r = log.range(of: "port ", range: marker.upperBound..<log.endIndex) else {
            return nil
        }
        let digits = log[r.upperBound...].prefix { $0.isNumber }
        return digits.isEmpty ? nil : Int(digits)
    }

    func stop() {
        if process.isRunning { process.terminate() }
        let deadline = Date().addingTimeInterval(5)
        while process.isRunning && Date() < deadline { usleep(20_000) }
        if process.isRunning { kill(process.processIdentifier, SIGKILL) }
    }

    var logTail: String { (try? String(contentsOf: logURL, encoding: .utf8)) ?? "" }
}

/// 桩的 HTTP 客户端。**绕开缓存**：同一个 URL 在一次测试里要探很多次，
/// 命中的缓存会把它变成一条永远为真的断言。
private let kE2EHTTP: URLSession = {
    let c = URLSessionConfiguration.ephemeral
    c.requestCachePolicy = .reloadIgnoringLocalCacheData
    c.timeoutIntervalForRequest = 5
    return URLSession(configuration: c)
}()

private func httpStatus(_ url: URL) async -> Int? {
    var req = URLRequest(url: url)
    req.cachePolicy = .reloadIgnoringLocalCacheData
    guard let (_, resp) = try? await kE2EHTTP.data(for: req) else { return nil }
    return (resp as? HTTPURLResponse)?.statusCode
}

// ---------------------------------------------------------------------------
// 轮询 / 进程观测
// ---------------------------------------------------------------------------

/// 反复取一个观测值直到满足条件（或超时），**把最后一次观测到的值交回给调用方**——
/// 断言写在调用方，失败消息里带上实际值；只回一个 Bool 的话"没等到"无法归因。
private func poll<T>(_ timeout: Double = 30, every: Double = 0.25,
                     _ probe: () async -> T, done: (T) -> Bool) async -> T {
    let deadline = Date().addingTimeInterval(timeout)
    var last = await probe()
    while !done(last) && Date() < deadline {
        try? await Task.sleep(nanoseconds: UInt64(every * 1_000_000_000))
        last = await probe()
    }
    return last
}

/// `pgrep -fl <pattern>` 的输出行，再按 `needle` 过滤。
///
/// ⚠️ **按 needle 过滤是必须的**：整轮测试是并行跑的，别的测试也会起内核；
/// 不过滤的话"内核还在不在"会被兄弟测试的进程污染成假红。
/// 而 `needle` 就是本测试那个**唯一**的临时目录——本测试起的进程命令行里必然有它。
private func processLines(matching pattern: String, containing needle: String) -> [String] {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
    p.arguments = ["-fl", pattern]
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = FileHandle.nullDevice
    guard (try? p.run()) != nil else { return [] }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return String(decoding: data, as: UTF8.self)
        .split(separator: "\n").map(String.init).filter { $0.contains(needle) }
}

/// 某个下载目录下**还活着的** aria2c。
///
/// ⚠️ 进程名不是 `aria2c` —— 内核把内嵌引擎释放成 `aria2c-<内容摘要>`（`daemon.rs:881`），
/// 所以 `pgrep -x aria2c` 对它**根本不匹配**，只能用 `-f` 匹配命令行。
private func aria2cProcesses(forDownloadDir dir: URL) -> [String] {
    processLines(matching: "aria2c", containing: dir.path)
}

/// `downloadDir` 下的全部**普通文件**的相对路径（`./` 已被 `standardizedFileURL` 折叠）。
private func relativeFilePaths(under root: URL) throws -> [String] {
    let fm = FileManager.default
    let prefix = root.standardizedFileURL.path + "/"
    guard let walker = fm.enumerator(at: root,
                                     includingPropertiesForKeys: [.isRegularFileKey]) else { return [] }
    var out: [String] = []
    for case let u as URL in walker {
        guard (try? u.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true else {
            continue
        }
        let path = u.standardizedFileURL.path
        if path.hasPrefix(prefix) { out.append(String(path.dropFirst(prefix.count))) }
    }
    return out
}

/// "百分号转义序列"的判据：`%` 后面跟两个十六进制位（`%20` / `%C3` / `%25` …）。
private let kPercentEscape = "%[0-9A-Fa-f]{2}"

private func isPercentEncoded(_ name: String) -> Bool {
    name.range(of: kPercentEscape, options: .regularExpression) != nil
}

/// 夹具路径的**逐段百分号编码**形态（与内核 `delivery::escape_segment`、
/// 服务端 `delivery_manifest.encode_url_path` 同一套保留集）。
///
/// 它只在**反向断言**里用：一个把 URL 编码后的串写进文件名的实现会造出 `%20` / `%C3%97`
/// 这种名字——本测试断言盘上**没有**这样的名字。
private func percentEncodedPath(_ path: String) -> String {
    let keep = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~")
    return path.split(separator: "/", omittingEmptySubsequences: false).map { seg in
        seg.utf8.map { b -> String in
            let c = Character(UnicodeScalar(b))
            return keep.contains(c) ? String(c) : String(format: "%%%02X", b)
        }.joined()
    }.joined(separator: "/")
}

// ---------------------------------------------------------------------------
// 端到端
// ---------------------------------------------------------------------------

// ⚠️ **不加 `.serialized`**：它对非参数化测试函数没有任何效果，却会产出一条编译告警
//    （约束 12 要求零告警）。本测试本来就不需要它——它碰的每一个全局资源都是自己的：
//    临时目录按 UUID 隔离、`pgrep` 按这个临时目录过滤、桩服务端口由服务自己报。
@Test func endToEndThroughTheShellsOwnClient() async throws {
    // ── 0) 一次性工作区：桩根 / 下载目录 / 设置 / 日志**全在**这里 ─────────────
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-e2e-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    let stubRoot = root.appendingPathComponent("stub")
    let downloadDir = root.appendingPathComponent("downloads")
    try FileManager.default.createDirectory(at: stubRoot, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: downloadDir, withIntermediateDirectories: true)

    // ── 1) 桩服务 ────────────────────────────────────────────────────────────
    let server = try StubServer.start(serving: stubRoot,
                                      logURL: root.appendingPathComponent("stub-server.log"))
    defer { server.stop() }
    print("[e2e] 桩服务 \(server.base)，根 \(stubRoot.path)")

    // ── 2) 夹具（真生成器）────────────────────────────────────────────────────
    // 桩可以先起：http.server 是**按需读盘**的，夹具后放进去照样服务得到。
    let manifest = try makeFixture(stubRoot: stubRoot, baseURL: server.base)
    print("[e2e] 清单 total_files=\(manifest["total_files"] ?? "?") "
          + "total_bytes=\(manifest["total_bytes"] ?? "?")")

    let manifestURL = URL(string: "\(server.base)/\(kE2ECode)/manifest.json")!
    let preflight = await poll(10, every: 0.1, { await httpStatus(manifestURL) }, done: { $0 == 200 })
    // 注：`#expect` 的说明参数是 `Comment`（只吃**一个字面量**，不吃 `String` 拼接），
    // 所以下面每条说明都写成单条字面量，需要换行就写 `\n`。
    #expect(preflight == 200,
            "桩服务没有在 10 秒内把清单服务起来（HTTP \(preflight.map(String.init) ?? "无响应")）；服务日志：\n\(server.logTail)")
    guard preflight == 200 else { return }      // 桩都不通，后面的断言只会一片红

    // ── 3) 起真内核（**两个目录都指到临时目录**）──────────────────────────────
    let client = try CoreClient.live(settingsPath: root.appendingPathComponent("settings.json").path,
                                     downloadDir: downloadDir.path)
    defer { client.shutdown() }

    // ── 3a) hello 握手 ──────────────────────────────────────────────────────
    let hello = try await client.callAsync("hello", .object(["protocol": .integer(1)]))
        .decoded(HelloResult.self)
    #expect(hello.protocolVersion == kProtocolVersion, "握手协议号不是 1")
    #expect(client.protocolAlerts.isEmpty, "内核发过协议级告警：\(client.protocolAlerts)")

    // ── 4) 内核 argv：**证明它没有去碰 `~/Downloads/Benagen`** ────────────────
    // 这一条不是形式主义：用 `CoreClient.live()` 的默认值跑这条测试，
    // 就是拿夹具清单去覆盖人类伙伴真实测试交付下来的状态文件。
    let kernelLines = await poll(5, { processLines(matching: "benagen-core",
                                                   containing: root.path) },
                                 done: { !$0.isEmpty })
    #expect(kernelLines.contains { $0.contains("--download-dir \(downloadDir.path)") },
            "内核的 argv 里没有 `--download-dir \(downloadDir.path)`（实际：\(kernelLines)）—— 若这里空了，说明内核是用默认下载目录起的，会覆盖 \(NSHomeDirectory())/Downloads/Benagen 里的真实状态")

    // ── 5) load_delivery：指向桩服务 ─────────────────────────────────────────
    let info = try await client.callAsync("load_delivery",
                                          .object(["code": .string(kE2ECode),
                                                   "base_url": .string(server.base)]))
        .decoded(DeliveryInfo.self)
    #expect(info.code == kE2ECode, "清单的 code 不对：\(info.code)")
    #expect(info.totalFiles == Int64(kE2EFixture.count), "total_files 不对：\(info.totalFiles)")
    #expect(info.totalBytes == kE2ETotalBytes, "total_bytes 不对：\(info.totalBytes)")
    #expect(info.expired == false, "刚生成的清单被判成过期了（expires_at=\(info.expiresAt)）")
    #expect(info.pageUrl == "\(server.base)/\(kE2ECode)/index.html",
            "交付页地址不对：\(info.pageUrl)")
    // 树里的 `path` 必须是**清单原文**（约束 3）：含 `./` 的那一条会露。
    let treePaths = filePaths(in: info.tree)
    #expect(Set(treePaths) == kE2EPaths,
            "树里的文件路径与清单原文对不上：\(treePaths.sorted())")

    // ── 6) list_dir 根 ──────────────────────────────────────────────────────
    let rawDir = try client.callSync("list_dir", .object(["path": .string("")]))
    guard case .object(let dirObj) = rawDir,
          case .array(let dirEntries)? = dirObj["entries"] else {
        Issue.record("list_dir 的形状不对：\(rawDir)")
        return
    }
    #expect(dirObj["path"] == .string(""), "根的 path 必须是空串（内核回 \"\"，不是 \"/\"）")
    var sawDir = false, sawRootFile = false
    for e in dirEntries {
        guard case .object(let o) = e, case .string(let type)? = o["type"] else {
            Issue.record("list_dir 的条目形状不对：\(e)")
            continue
        }
        if type == "dir" {
            sawDir = true
            // 目录项**没有 path 键**（契约：目录不是清单条目，没有可下载的路径）
            #expect(Set(o.keys) == ["type", "name", "children_count"],
                    "目录项的键不对（多了 path 就是把目录当文件卖）：\(o.keys.sorted())")
        } else {
            sawRootFile = true
            #expect(Set(o.keys) == ["type", "name", "path", "size", "crc64",
                                    "state", "completed", "total", "speed", "err",
                                    "source_mtime"],
                    "文件项的键不对：\(o.keys.sorted())")
            // 源文件的修改时间**必须始终有键**（`crc64` 的既有做法同款）：
            // 无值时发**空串**，不是省略这个键。
            //
            // ⚠️ **这一条证明的是"协议始终发这个键"，不是"值被逐字透传"**（复审顺手修 ⑤
            //    改掉了一句过度的声称）。判别力上的事实是：本测试的清单由内嵌的
            //    `delivery_manifest.py` 用 **3 元组**条目产出（`(path, size, crc64)`），
            //    `_unpack` 会给第 4 项补空串，所以期望值 `""` 与"字段根本没送达"
            //    （内核省略该键、或壳把它解成了空串）**同形** —— 一个恒发空串的实现
            //    能让这条断言永远为真。见 `delivery_manifest.py` 的 `_unpack` 与账本 Ruling C1。
            //
            // 真正的**值级**透传由 Rust 侧两条守着（都在另一条车道上，改动本测试**替不了**它们）：
            //   - `core/src/view.rs` 的 `tree_node_carries_source_mtime_verbatim`（单测）；
            //   - `core/tests/e2e.rs` 的 `source_mtime_is_carried_through_verbatim`
            //     （期望值 `2026-09-14T12:00:00+08:00`）。
            // 「上传机上真实的文件时间一路显示到界面」由**任务 5**用一份真交付验（本文件
            // 的内嵌夹具**刻意不动**，它与 `core/tests/e2e.rs` 共用同一个 3 元组约定）。
            #expect(o["source_mtime"] == .string(""),
                    "无值的 source_mtime 必须是空串而不是缺键：\(String(describing: o["source_mtime"]))")
            // 根级那一个文件：`path` 是清单原文（含 × 与空格，逐字）
            #expect(o["path"] == .string("Readme ×.txt"),
                    "根级文件项的 path 不是清单原文：\(String(describing: o["path"]))")
        }
    }
    #expect(sawDir && sawRootFile, "根本该既有目录项又有文件项（sawDir=\(sawDir) sawRootFile=\(sawRootFile)）")

    // ── 7) get_tree：default_selected == 非 complete 的路径集合 ──────────────
    // 此刻盘上一片空白 ⇒ **整棵树**都该默认勾选。下载完再回来看一次（步骤 13），
    // 那时它必须变空——两条合起来才真正钉住"等于非 complete 的集合"。
    let before = try await client.callAsync("get_tree", .object([:])).decoded(TreeResult.self)
    #expect(Set(before.flat.map(\.path)) == kE2EPaths,
            "flat 里的路径与清单原文对不上：\(before.flat.map(\.path).sorted())")
    #expect(Set(before.defaultSelected) == kE2EPaths,
            "还没下载时 default_selected 应当是全部 \(kE2EFixture.count) 项，实际 \(before.defaultSelected.sorted())")
    #expect(before.progress.totalBytes == kE2ETotalBytes, "progress.total_bytes 不对：\(before.progress.totalBytes)")
    #expect(before.progress.doneBytes == 0 && before.progress.percent == 0,
            "还没下载就不该有已完成字节：\(before.progress)")

    // ── 8) enqueue：**目录路径原样交给内核**，由它展开 ───────────────────────
    // 一次请求里混三类目标，为的是在同一条链路上同时钉住三件事：
    //   `PFX`（目录 → 内核按前缀展开成它下面的 4 个文件）、
    //   `Readme ×.txt`（**非 ASCII 原文路径**由壳发出去、内核原样匹配）、
    //   `test`（另一个目录）。
    // 壳侧的 `DownloadTargets` 也**不展开目录**（约束 1），这里用的就是它那种形态。
    let targets = ["PFX", "Readme ×.txt", "test"]
    let enq = try await client.callAsync("enqueue",
                                         .object(["paths": .array(targets.map { .string($0) })]))
        .decoded(EnqueueResult.self)
    #expect(enq.rejected.isEmpty, "有文件被拒：\(enq.rejected.map { "\($0.path)：\($0.reason)" })")
    #expect(Set(enq.added.map(\.path)) == kE2EPaths,
            "入队的路径不是全部 \(kE2EFixture.count) 项（目录没被展开？）：\(enq.added.map(\.path).sorted())")
    #expect(Set(enq.added.map(\.gid)).count == enq.added.count, "有重复的 gid")

    // ── 8a) 引擎真的起来了（否则下面的"无孤儿"是一条空断言）────────────────
    // ⚠️ 这一条是**守门**：`pgrep` 的过滤器一旦写错（比如匹配进程名而进程叫 `aria2c-<hash>`），
    //    步骤 15 的"没有孤儿"会变成**一条永远为真的绿灯**。先证明过滤器能看见活着的引擎。
    let liveAria2c = await poll(15, { aria2cProcesses(forDownloadDir: downloadDir) },
                                done: { !$0.isEmpty })
    #expect(!liveAria2c.isEmpty,
            "下载期间没看到属于 \(downloadDir.path) 的 aria2c 进程——要么引擎没起来，要么本测试的进程过滤器失灵（后者会让步骤 15 变成空断言）")

    // ── 9) 轮询 transfer_list 直到全部结束（上界 60 秒）──────────────────────
    let settled = await poll(60, every: 0.3,
                             { await transferList(client) },
                             done: { lst in
                                 guard let lst, !lst.items.isEmpty else { return false }
                                 return lst.items.allSatisfy { $0.state != .waiting && $0.state != .active }
                             })
    guard let settled else {
        Issue.record("transfer_list 解不出来（60 秒内没拿到可用的列表）")
        return
    }
    #expect(settled.items.count == kE2EFixture.count,
            "传输列表的项数不对：\(settled.items.count)")
    #expect(settled.items.allSatisfy { $0.state == .complete },
            "有任务没传完：\(settled.items.map { "\($0.path ?? "?")=\($0.state.rawValue)/\($0.rawStatus)" })")
    #expect(settled.items.allSatisfy { $0.errorMessage.isEmpty },
            "有任务带错误：\(settled.items.map(\.errorMessage))")
    // `rawStatus` 是 aria2 的原文（界面那个「已暂停」角标只能靠它），完成时必须是 `complete`
    #expect(settled.items.allSatisfy { $0.rawStatus == "complete" },
            "rawStatus 不是 complete：\(settled.items.map(\.rawStatus))")
    // GID → 清单相对路径的映射必须建立（`path` 缺失 = 界面上一行"未知文件"）
    #expect(Set(settled.items.compactMap(\.path)) == kE2EPaths,
            "传输项没有带回清单路径：\(settled.items.map { $0.path ?? "nil" })")
    #expect(settled.global.numActive == 0, "还有活跃任务：\(settled.global)")
    // `total` 就是清单里的字节数（`progress` 的分母）
    let sizes = Dictionary(uniqueKeysWithValues: kE2EFixture.map { ($0.path, Int64($0.body.utf8.count)) })
    for item in settled.items {
        guard let p = item.path else { continue }
        #expect(item.total == sizes[p], "\(p) 的 total 不是清单字节数：\(item.total) vs \(sizes[p] ?? -1)")
    }

    // ── 10) 落盘：**路径与清单原文逐字一致**（不是"规范化后一致"）────────────
    for f in kE2EFixture {
        let landed = downloadDir.appendingPathComponent(f.path)
        let isFile = (try? landed.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
        #expect(isFile, "没落盘：\(landed.path)")
        let size = (try? landed.resourceValues(forKeys: [.fileSizeKey]))?.fileSize
        #expect(size == f.body.utf8.count,
                "\(f.path) 的字节数不对：\(size.map(String.init) ?? "nil") vs \(f.body.utf8.count)")
    }
    // **反向断言**：盘上不得出现 URL 编码形态的文件名。
    // 一个"把 percent-encoded 串当文件名写"的实现会造出 `QC%20%E5%9B%BE.png`
    // ——上面那几条逐字断言正是冲着它去的，这里再把它显式钉一次。
    //
    // ⚠️ 判据不能写成"名字里有 `%`"：夹具自己就有一个**字面** `%`（`PFX/100% 覆盖.bin`），
    //    那样写会把一份正确落盘判成红的。判据是"有没有出现**百分号转义序列**"
    //    （`%` + 两个十六进制位），字面 `%` 后面跟的是空格，不算。
    let names = try relativeFilePaths(under: downloadDir)
    let encodedForm = percentEncodedPath("PFX/100% 覆盖.bin")
    // 先验对照物本身能被判成编码形态——否则下面那条是**空断言**（约束 12）。
    #expect(isPercentEncoded(encodedForm),
            "对照物 \(encodedForm) 没被 `\(kPercentEscape)` 判成编码形态——下面那条断言就是空断言")
    #expect(isPercentEncoded("PFX/100% 覆盖.bin") == false,
            "字面 `%` 被判成了编码形态——上面那条会把正确落盘判成红的")
    let encoded = names.filter(isPercentEncoded)
    #expect(encoded.isEmpty,
            "盘上出现了 URL 编码形态的文件名（落盘路径被百分号编码了）：\(encoded)")
    #expect(names.contains("test/C24-8_×_25WS024/Figure/QC 图.png"),
            "那个真实路径形态没有逐字落盘；盘上实际是：\(names.sorted())")
    #expect(names.contains(encodedForm) == false, "字面 `%` 被编码成了 `%25` 落盘")

    // ── 11) verify_status：六类齐备、ok 覆盖全部文件 ─────────────────────────
    let rawVerify = try client.callSync("verify_status", .object([:]))
    guard case .object(let vObj) = rawVerify else {
        Issue.record("verify_status 的形状不对：\(rawVerify)")
        return
    }
    // **六类齐备**：逐字比对**六个键名**（`size_mismatch` 是带下划线的那个）。
    // 少了任何一类，界面上的六类计数就少一格——而"某一类永远不出现"是最难发现的那种静默。
    #expect(Set(vObj.keys) == ["ok", "bad", "missing", "size_mismatch",
                               "unverifiable", "unreadable", "all_good"],
            "verify_status 的键不对：\(vObj.keys.sorted())")

    // 校验是**落地监听器**（200 ms 一拍）异步派出去的，所以要等
    let verify = await poll(30, { await verifyStatus(client) },
                            done: { ($0?.ok.count ?? 0) == kE2EFixture.count })
    guard let verify else {
        Issue.record("verify_status 解不出来")
        return
    }
    let verifyDetail = "ok=\(verify.ok.sorted()) missing=\(verify.missing.sorted()) "
        + "bad=\(verify.bad.sorted()) size_mismatch=\(verify.sizeMismatch.sorted()) "
        + "unverifiable=\(verify.unverifiable.sorted()) unreadable=\(verify.unreadable.sorted())"
    #expect(Set(verify.ok) == kE2EPaths,
            "ok 不是全部 \(kE2EFixture.count) 项：\(verifyDetail)")
    #expect(verify.bad.isEmpty && verify.missing.isEmpty && verify.sizeMismatch.isEmpty
            && verify.unverifiable.isEmpty && verify.unreadable.isEmpty,
            "除 ok 外还该是空的：\(verify)")
    #expect(verify.allGood, "all_good 应当是 true")

    // ── 12) get_state：每条落盘文件的记录都写进了状态文件 ─────────────────────
    let state = await poll(15, { await stateFile(client) },
                           done: { ($0?.files.count ?? 0) == kE2EFixture.count })
    guard let state else {
        Issue.record("get_state 解不出来")
        return
    }
    #expect(state.version == 1, "状态文件版本不是 1：\(state.version)")
    #expect(state.code == kE2ECode, "状态文件记的码不对：\(state.code)")
    #expect(Set(state.files.keys) == kE2EPaths,
            "状态文件里的记录不是全部 \(kE2EFixture.count) 条：\(state.files.keys.sorted())")
    for f in kE2EFixture {
        let e = state.files[f.path]
        #expect(e != nil, "\(f.path) 没有状态记录")
        #expect(e?.size == Int64(f.body.utf8.count), "\(f.path) 的记录 size 不对：\(String(describing: e?.size))")
        #expect(e?.crc64.isEmpty == false, "\(f.path) 的记录 crc64 是空的——planner 之后会拿它当跳过依据")
        #expect((e?.mtime ?? 0) > 0, "\(f.path) 的记录 mtime 不合法：\(String(describing: e?.mtime))")
    }

    // ── 13) get_tree（第二次）：非 complete 的集合**现在是空的** ─────────────
    let after = try await client.callAsync("get_tree", .object([:])).decoded(TreeResult.self)
    #expect(after.defaultSelected.isEmpty,
            "全部校验通过之后 default_selected 应当是空的（否则客户一点「下载选中」就把下好的又传一遍）：\(after.defaultSelected.sorted())")
    #expect(after.flat.allSatisfy { $0.state == .complete },
            "还有文件不是 complete：\(after.flat.filter { $0.state != .complete }.map(\.path))")
    #expect(after.progress.doneBytes == kE2ETotalBytes && after.progress.percent == 100,
            "进度不是 100%：\(after.progress)")

    // ── 14) shutdown：内核进程退出 ───────────────────────────────────────────
    client.shutdown()
    let kernelGone = await poll(10, every: 0.1,
                                { processLines(matching: "benagen-core", containing: root.path) },
                                done: { $0.isEmpty })
    #expect(kernelGone.isEmpty,
            "shutdown() 之后内核进程还在：\(kernelGone)——收尾没有把子进程带走")
    // 壳这一侧也必须显式收场：连接关上之后不再接新请求（不是静默失败）
    var thrown: Error?
    do { _ = try client.callSync("hello", .object(["protocol": .integer(1)])) } catch { thrown = error }
    guard case .transport(let msg)? = thrown as? CoreError else {
        Issue.record("shutdown 之后的请求必须回一条 transport 错误，实际 \(String(describing: thrown))")
        return
    }
    #expect(msg.contains("已关闭"), "收尾后的错误文案不对：\(msg)")

    // ── 15) 不留孤儿：本测试的下载目录对应的 aria2c 必须消失 ─────────────────
    // ⚠️ 这一条**不是**在第 8a 步单独成立的：8a 证明过滤器看得见活着的引擎，
    //    这里才谈得上"没有孤儿"。
    let orphans = await poll(10, every: 0.1,
                             { aria2cProcesses(forDownloadDir: downloadDir) },
                             done: { $0.isEmpty })
    #expect(orphans.isEmpty, "shutdown() 之后还留着 aria2c 孤儿：\(orphans)")
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

private func transferList(_ c: CoreClient) async -> TransferListResult? {
    guard let v = try? await c.callAsync("transfer_list", .object([:])) else { return nil }
    return try? v.decoded(TransferListResult.self)
}

private func verifyStatus(_ c: CoreClient) async -> VerifyStatus? {
    guard let v = try? await c.callAsync("verify_status", .object([:])) else { return nil }
    return try? v.decoded(VerifyStatus.self)
}

private func stateFile(_ c: CoreClient) async -> StateFile? {
    guard let v = try? await c.callAsync("get_state", .object([:])) else { return nil }
    return try? v.decoded(StateFile.self)
}

/// 树里所有**文件**叶节点的 `path`（清单原文）。
private func filePaths(in node: TreeNode) -> [String] {
    switch node {
    case .empty: return []
    case .file(let f): return [f.path]
    case .dir(_, let children): return children.values.flatMap { filePaths(in: $0) }
    }
}
