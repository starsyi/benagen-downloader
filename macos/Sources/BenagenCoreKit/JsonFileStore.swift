import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

// ---------------------------------------------------------------------------
// 壳的**第一次写盘**：一份 JSON 文件的原子读写（阶段 E 规格 §1.1 / §2.1）
//
// 两条底线在这一层：
//   · **E-1 —— 读失败不是异常路径**：`read` 返回 `nil` 而不是抛
//     （最常见的情形就是"第一次运行、文件还不存在"）；
//   · **E-2 —— 写盘必须原子**：同目录临时文件 + `rename(2)`
//     （与内核 `core/src/state.rs:146-150` 的既有做法同款）。半个文件比没有文件更糟：
//     半截 JSON 会让下一次启动读到一份"能解析出前半段"的垃圾，而正确的结果是"当空"。
//     路径只有一个占位：`Data.write(options: .atomic)` 也做临时文件，
//     但它**不保证**临时文件与目标同目录（跨设备 rename 不是原子的），
//     而且"怎么写的"从此不可见 —— 这里把它写成看得见的两步。
//
// ⚠️ 这一层**不懂 JSON**：它只搬字节。"这份字节还能不能用"由调用方按自己的语义判
//    （`BatchHistory.parse` / `AppPreferences.parse` 各自"坏就当归零"）。
//    在这里替它们判会把"坏文件"和"没有文件"混成同一个 `nil`。
//
// ⚠️⚠️ **测试一律用临时目录**：`read` / `write` 都**接受 URL 参数**，默认值才指向
//     `~/Library/Application Support/BenagenDownloader/`。那里放着人类伙伴
//     真实在用的数据 —— 测试必须把句柄指到临时目录去。
// ---------------------------------------------------------------------------

/// 原子的 JSON 文件读写。无状态，可以随便建。
public struct JsonFileStore: Sendable {
    public init() {}

    /// 读。**读不出来 ⇒ `nil`，绝不抛**（E-1：调用方按"当空"处理）。
    ///
    /// 覆盖：文件不存在、路径是个目录、权限不够、磁盘上是一堆二进制垃圾 …
    /// 这些在壳里**都不是**故障路径 —— 它们全部退化成"这次没有历史/没有偏好"。
    public func read(_ url: URL) -> Data? {
        try? Data(contentsOf: url)
    }

    /// 原子写：**同目录**临时文件 → `rename(2)` 覆盖目标。
    ///
    /// - 目标目录不存在时**先建出来**（首次运行时整条
    ///   `…/Application Support/BenagenDownloader/` 都可能不存在）；
    /// - 任何一步失败都**抛**（简报：写失败要能被上报，不能让用户以为存上了）；
    /// - 失败时目标文件**逐字保持原样**（要么是旧的完整内容，要么是新的完整内容，
    ///   永远不会是半截）。
    public func write(_ data: Data, to url: URL) throws {
        let directory = url.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            throw JsonFileStoreError("建目录失败：\(directory.path)（\(error.localizedDescription)）")
        }

        let temporary = directory.appendingPathComponent(Self.temporaryName(for: url))
        do {
            try data.write(to: temporary)
        } catch {
            // 清掉可能写了一半的临时文件（`try?`：它也可能压根没被创建）。
            // 这个路径是**我们的保留名**（`temporaryName(for:)`），删它是安全的。
            try? FileManager.default.removeItem(at: temporary)
            throw JsonFileStoreError("写临时文件失败：\(temporary.path)（\(error.localizedDescription)）")
        }

        do {
            try Self.replaceAtomically(temporary, url)
        } catch {
            // 目标没被碰过，但临时文件留下了 ⇒ 清掉（不然它会永远躺在那里）。
            try? FileManager.default.removeItem(at: temporary)
            throw error
        }
    }

    /// 临时文件的名字（与目标**同目录**、点开头、以 `.tmp` 结尾）。
    ///
    /// ⚠️ 公开是有意的：它让"写失败时旧文件还在"这条性质**可以被测试构造**
    ///    （`JsonFileStoreTests.aFailedWriteThrowsAndLeavesThePreviousFileIntact`
    ///    把临时文件名占成一个目录来制造一次真实的失败）。
    public static func temporaryName(for url: URL) -> String {
        ".\(url.lastPathComponent).tmp"
    }

    /// `rename(2)`：**原子替换**，目标存在与否都成立。
    ///
    /// ⚠️ 不用 `FileManager.replaceItemAt`（它要求目标**已经存在**，首次写盘会失败），
    ///    也不用 `moveItem`（目标存在时的行为随平台而变）—— 原子替换的语义只有
    ///    `rename(2)` 一处定义。
    /// ⚠️ 名字刻意**不叫 `rename`**：那样它会与下面这个 C 函数同名，`rename(a, b)`
    ///    会解析成对自己的一次递归调用（已实测：报 "cannot convert 'String' to 'URL'"）。
    private static func replaceAtomically(_ temporary: URL, _ destination: URL) throws {
        #if canImport(Darwin)
        let result = Darwin.rename(temporary.path, destination.path)
        #elseif canImport(Glibc)
        let result = Glibc.rename(temporary.path, destination.path)
        #endif
        guard result == 0 else {
            let code = errno
            throw JsonFileStoreError("替换 \(destination.path) 失败：rename(2) errno=\(code)"
                                     + "（\(String(cString: strerror(code)))）")
        }
    }
}

/// 写盘失败的原文。**它必须能到调用方手里** —— 一次"以为存上了"的静默失败，
/// 用户直到重启后才会发现，而那时已经无从归因。
public struct JsonFileStoreError: Error, Equatable, CustomStringConvertible {
    public let message: String

    public init(_ message: String) {
        self.message = message
    }

    public var description: String { message }
}

// ---------------------------------------------------------------------------
// 壳的落盘根
// ---------------------------------------------------------------------------

/// 壳自己的文件放哪：`~/Library/Application Support/BenagenDownloader/`。
///
/// ⚠️ **与内核的 `settings.json` / `last_code` 同目录**是刻意的（规格 §1.1）：
///    客户排障时"这个应用的东西都在一个文件夹里"比省一个目录重要得多；
///    但**是另一个文件** —— 壳的历史/偏好不塞进内核的文件里，两者的格式与生命周期
///    由各自负责（内核改 `settings.json` 的形状不该让壳的历史跟着陪葬）。
///
/// ⚠️ 先取 `$HOME`、再回退 `homeDirectoryForCurrentUser`：内核的
///    `settings::default_path()`（`core/src/settings.rs:198-207`）用的是 `$HOME`
///    （Go 的 `os.UserHomeDir()` 在 Unix 上就是它），而"同目录"这条契约要两边
///    算出**同一个**目录才算数。`$HOME` 在 GUI 应用里通常就是用户主目录，
///    但它是**环境变量**、可以被改；这里跟着内核取同一个来源，避免"壳写在 A、
///    内核写在 B"这种只有排障时才发现的错位。
public enum ShellStorage {
    public static var directory: URL {
        let home = ProcessInfo.processInfo.environment["HOME"]
            .flatMap { $0.isEmpty ? nil : $0 }
            ?? FileManager.default.homeDirectoryForCurrentUser.path
        return URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("Application Support", isDirectory: true)
            .appendingPathComponent("BenagenDownloader", isDirectory: true)
    }
}
