import Foundation

// ---------------------------------------------------------------------------
// macOS 壳侧的**有界**诊断日志（规格 §2.3 的壳那一半，任务 5）
//
// ⚠️ **这是同一个模块的第三份实现**，另外两份是：
//     · `core/src/diagnostics.rs`（内核，写 `diag-kernel.log`）；
//     · `windows/shell-core/src/diagnostics.rs`（Windows 壳，写 `diag-shell.log`）。
//     三份**必须逐条对齐**（行格式、`maxValueChars` 的截断、轮转命名、上限与代数）——
//     客户会把几个文件**一起**发回来，**读法必须是同一种**。
//     ⇒ **改这里的任何一条共享常量，都必须去看另外两份**；
//       而那两份各自的模块头也都点名了其余两份（内核那一侧的反向指针是 2026-10-06 补的）。
//
// # 为什么必须有它
//
// 内核那一份回答的是"**内核**当时在做什么"；客户报"下载引擎已断开"时，另一半问题
// ——"**壳**当时在做什么、它给内核发了什么、内核怎么回的"——在今天**一个字都没有**：
// macOS 壳此前**一行日志都没有**（本模块就是 2026-10-06 补上的那一份；
// 「壳那侧尚未落地」是当时的原话，那一句在本批里已经被改掉了）。
// 没有这一份，回传的日志合起来仍然拼不出一次故障的完整现场。
//
// # 三件必须写在最前面的事
//
// 1. **级别不需要下发**：壳自己就知道自己开的是哪一档（偏好是壳自己存的），
//    所以这里没有内核那条 `--log-level` 参数、也没有"把级别传给子进程"这一步。
//    ⚠️ 内核那一半**需要**下发（它是个独立进程，读不到壳的偏好）；
//    两边"级别的来源"不同，但**档位的语义与界**必须一致（见第 3 条）。
//
// 2. **它与 Rust 那两份是各自独立的实现**，这是**架构强制**的（同 Windows 那一侧）：
//    同一个模块写三遍，**改动必须成对**。那边唯一一条**自动**判据
//    （`windows/shell-core/src/diagnostics.rs` 的 `the_two_loggers_agree_on_the_shared_constants`）
//    `include_str!` 的是**内核**那份源码 —— 它**够不到这一份**。
//    ⇒ 本文件靠的是：模块头这份互相点名的注释 + `DiagnosticsLogTests` 里**逐条对应**的用例
//      （界、轮转兜底、行形状、档位取值那几条与 Rust 那两份是同几条）。
//    别把那条用例读成"三边已经对齐了"：它守着的是两个常量，且只覆盖 Rust 那两份。
//
// 3. **行格式与截断规则必须与那两份一致**：字段行是
//    `ts=<unix 秒> event=<名> k=v k=v …`（见 `formatLine`），
//    每个字段值截到 `maxValueChars` **个 Unicode 标量**（不是字节，也不是 Swift 的
//    `Character` —— 后者的粒度是扩展字形簇，与 Rust 的 `chars()` 不等价，见 [`formatLine`]）。
//
// # 两条硬约束（与那两份逐字相同）
//
// 1. **有界**：**两档各自有界**，界线由 [`Level`] 给。
//    - `normal`（**缺省**）：单文件上限 [`maxBytes`]（1 MiB），超了轮转成 `.1`
//      （**只留一代**）；
//    - `verbose`（"每一次往返"那一档）：单文件 **4 MiB**、留**两代**（`.1` / `.2`）
//      ⇒ 总上界 12 MiB。
//    绝不无限增长——这是本仓对"落盘的东西"的一贯口径。
//    ⚠️ 精确地说，轮转在**每次写入之前**检查，所以单文件瞬时上界是
//    `maxBytes + 一行`（写第 N+1 行之前才看得见第 N 行把文件顶过了线）、
//    总占用上界是 `(generations + 1) × (maxBytes + 一行)`。
//    而「一行」自己也有上界：[`maxValueChars`] 给每个字段值封顶 200 个 **Unicode 标量**。
//    🔴 **说"字符"会把人引回 bug**：Swift 里 `Character` 是**扩展字形簇**（一串组合字符算一个），
//    而 `prefix(200)` 正是按它截的 —— 那正是本项目修过的那个分歧（200 个字形簇可以是
//    400~5000 个标量）。**单位只能是标量**，别写成"字符"。
//    **唯一**能顶破这个上界的条件是"轮转失败"与"兜底截断失败"**两条都发生**
//    （见 [`rotate`]）：那时主文件会接着长——本模块不许为此把产品弄坏，所以它仍然无声。
// 2. **永不许把产品弄坏**：本模块**没有一个函数把错误交出去**——[`write`]、[`rotate`]、
//    [`complainOnce`] 全都是"失败就无声跳过"。
//    ⚠️ 判据是"**不许有错误逃出本模块**"，不是"不许有函数返回 `Result`"
//    （Windows 那一份的模块头记着这条差别：它有一个 `log_path() -> Result`，
//    只是就地被吞掉了）。本文件里连那样一个都没有：路径是**纯计算**（见 [`logURL`]）。
//    写不进去（目录不可写、磁盘满、路径被占）时**无声跳过**，
//    只在**第一次**失败时往 stderr 说一句。
//    ⚠️ 壳这一侧的诚实记账：它是应用内的库，release 里 stderr 可能**没有去处**
//    （GUI 应用从 Finder 启动时不在终端里）。这不改本条的设计
//    （"不弄坏产品"仍然成立，且用终端跑时照样看得见），但**不许把它说成
//    "失败了用户一定看得见"**。
//
// # 不记什么（"记什么"的另一半，**这一节的绝对句在 2026-10-06 之前是假的**）
//
// **不记**交付码全文、文件路径全文、`rpc-secret`、客户目录名。只记方法名、耗时、结果。
// 诊断日志会被客户回传，它不该成为一份数据清单。
//
// 🔴 **但只写上面那一句会说谎**（终审实测，与 Windows 那一侧同一条）：`kernel_call`
//    那一行的 `why` 是**内核原文**，而交付码是**我们自己**拼进内核文案里的 ——
//    `core/src/delivery.rs` 那条 404（`清单不存在（404）——请确认交付码是否正确：{url}`）
//    把 `…/{交付码}/manifest.json` 整条 URL 送了进来，`core/src/main.rs` 的 `preflight`
//    那一档同样带着客户目录名。⇒ 正确的口径是**两条**：
//
//    1. **按构造抹掉**：交付码与下载目录在**写进这个文件之前**被 [`redact`] 换成
//       [`redacted`]。要抹的串由壳在**它自己知道的那一刻**推下来
//       （`CoreClient.setRedactions`，调用点 `AppModel` 的两处：发 `load_delivery`
//       之前那个码 + 偏好里那个目录）。
//       ⚠️ **这一条只对"壳知道的那两个串"成立**（修复轮 2 收窄，与 Windows 那一侧同一条）：
//       **没配置下载目录**（默认状态）时推下去的是空串（[`redact`] 跳过它）、也不传
//       `--download-dir`，内核回落到**它自己**的默认路径（`core/src/paths.rs` 的
//       `~/Downloads/Benagen`）—— 那条路径壳不知道（本仓的 `kernelDefaultDisplayPath`
//       明说了它只是显示用的），因此它出现在内核 `preflight` 失败文案里
//       （`目标目录不可写（{dir}）`）时**抹不掉**。别把这一条读成"下载目录一定被抹掉了"。
//    2. **仍然可能记到**：第三方（内核 / aria2）的原始报错文案。它**原样**透传，
//       里面也可能带上交付码 —— 那一份抹不掉：要可靠地识别任意第三方文案里的交付码，
//       得先能识别它，而我们识别不了（规格 §2.7 的显式接受）。
//
//    ⚠️ [`redact`] 是**字面子串**替换，不是"保证干净了"：码以别的写法出现
//    （换了大小写、被 URL 编码、被拆成两段）时它**不保证**能抹掉。
//
// ⚠️ 诊断日志里那个 `ts=` 是 **UTC 的 unix 秒**（与 Rust 那两份同一条）。
//    换算不在这里自己实现历法，只给命令：macOS `date -r <ts>`、Linux `date -d @<ts>`。
// ---------------------------------------------------------------------------

public enum DiagnosticsLog {

    // MARK: - 与 Rust 那两份共享的常量

    /// 单文件上限：1 MiB。轮转只留一代（`.1`）⇒ 总占用上界 `2 × maxBytes` 加两行
    /// （见模块头第 1 条：轮转在写入**之前**检查，每行另有 [`maxValueChars`] 封顶）。
    ///
    /// ⚠️ **必须与 `core/src/diagnostics.rs` / `windows/shell-core/src/diagnostics.rs`
    /// 的同一个常量相等**（那两个常量由 Rust 侧的一条跨 crate 用例钉住；
    /// 这一份够不到它，靠 `DiagnosticsLogTests.theNormalLevelKeepsTheOneMiBBound`）。
    public static let maxBytes: UInt64 = 1_048_576

    /// 单个字段值的**Unicode 标量**上限：200。
    ///
    /// 为什么需要它：`why` 里可能是**第三方**（内核 / 系统）的原始错误文案，长度不受我们
    /// 控制。不截断的话"一行"可以是任意长——那「有界」就只约束了**行数**、没约束**字节数**。
    ///
    /// 🔴 **单位是标量，不是字节，也不是 Swift 的 `Character`（扩展字形簇）。**
    ///    Rust 那两份用的是 `chars().take(200)`，而 Rust 的 `char` 就是**一个标量**
    ///    ⇒ 这一侧必须按 `unicodeScalars` 截，见 [`formatLine`] 里那一段。
    ///    按 `Character` 截会让"一个字段值"长到几倍（组合重音 ⇒ 2 倍、ZWJ emoji ⇒ 更极端），
    ///    而**"每一行有上界"**这条硬约束正是被那一格顶破的。
    ///    判据：`DiagnosticsLogTests.aValueIsTruncatedToTwoHundredScalars`。
    public static let maxValueChars: Int = 200

    /// 壳侧的日志文件名（与内核的 `diag-kernel.log` 同一个目录、不同的文件）。
    ///
    /// ⚠️ **同目录**这件事靠的是 [`logURL`] 与 `ShellStorage.directory` 同源
    ///    （即 `AppPreferencesStore.defaultURL` 那个目录），有判据（见 `DiagnosticsLogTests`）。
    ///    ⚠️ 两个进程**各写自己的文件**：同一个文件被两个进程追加 + 轮转会打架，
    ///    而诊断日志出问题的方式必须是"少记几条"，不能是"把彼此的账写坏"。
    public static let fileName = "diag-shell.log"

    /// 诊断日志的档位。**缺省是 [`Level.normal`]** —— 与 Rust 那两份同一套语义与界。
    public enum Level: Equatable, Sendable {
        /// 只记**事件**：抖动、断开、重试的成败。1 MiB / 留一代。
        case normal
        /// **每一次往返**都记。4 MiB / 留两代 ⇒ 总上界 12 MiB。
        case verbose

        /// 单文件上限。
        public var maxBytes: UInt64 {
            switch self {
            case .normal: return DiagnosticsLog.maxBytes
            case .verbose: return 4 * 1_048_576
            }
        }

        /// 轮转保留几代（`.1` … `.N`）。
        public var generations: Int {
            switch self {
            case .normal: return 1
            case .verbose: return 2
            }
        }
    }

    // MARK: - 进程的级别

    /// 进程级别的格子。**做成可加的锁而不是 `nonisolated(unsafe) var`**：
    /// `configure` 有**两处**调用点（启动时 / 开关被改时，见 `AppModel`），
    /// 而 `log` 的调用点散在各处 —— 级别被读到一半改掉的窗口不该存在。
    ///
    /// ⚠️ 与 Rust 那两份同形：它做成进程级的静态，是因为 `log` 的调用点散在各处而
    /// **没有任何一处拿得到配置**。测试**不需要**碰它：`log` 走的就是默认的
    /// [`Level.normal`]，而两档的界由 [`write`] 直接测（它收一个**显式**的级别）。
    private final class LevelBox: @unchecked Sendable {
        private let lock = NSLock()
        private var value: Level = .normal

        var level: Level {
            lock.lock(); defer { lock.unlock() }
            return value
        }

        func set(_ level: Level) {
            lock.lock(); value = level; lock.unlock()
        }
    }

    private static let box = LevelBox()

    /// 落一次级别。**只该在启动时与开关被改时各调一次**（见 `AppModel` 的两处调用点）。
    public static func configure(_ level: Level) { box.set(level) }

    /// 此刻的级别。**缺省是 [`Level.normal`]** —— 与"加这一档之前"逐字等价。
    public static var level: Level { box.level }

    // MARK: - 落一行

    /// 落一条诊断。**永不 panic、永不抛错**。
    ///
    /// **只记事件**：按**进程当前的级别**落 —— 默认档下与加这一档之前逐字等价。
    public static func log(_ event: String, _ fields: [(String, String)]) {
        logAt(event, fields, level: level)
    }

    /// **只在详细档记**——给"每一次往返"那一类用（壳侧就是 `CoreClient` 里那一行
    /// `kernel_call`）。
    ///
    /// ⚠️ 它**不是** `log` 的参数版：调用点写 `logVerbose(...)` 表达的语义是
    /// "这一行是**详细档才有的**"，而 `if level == .verbose { log(...) }` 散在十几处
    /// 会让"哪些行属于详细档"没有一个能 grep 的答案。
    public static func logVerbose(_ event: String, _ fields: [(String, String)]) {
        if level == .verbose { logAt(event, fields, level: .verbose) }
    }

    // MARK: - 写盘前抹掉的那几个字面串

    /// 被 [`redact`] 抹掉的那一段文字（**与 Windows 那一侧的 `REDACTED` 逐字相同** ——
    /// 客户回传的两个文件要能对着读，被抹掉的记号自然也得是同一种）。
    ///
    /// ⚠️ 它**故意不是空串**：抹成空串的话，"这里本来有个码"这件事就看不出来了 ——
    ///    而看日志的人恰恰需要知道"内核那句原文里有一段被我们拿掉了"。
    public static let redacted = "[已隐去]"

    /// **写盘之前**把 `text` 里出现的每一个 `secret` 换成 [`redacted`]（规格 §2.3 B 的
    /// "能按构造就避开，就必须避开"）。
    ///
    /// 它是**纯函数**：不读时钟、不碰文件、不看进程静态 —— 于是"抹不抹得掉"是一条能在
    /// 本机跑的断言，而不是"等真机上导一次看看"。
    ///
    /// ⚠️ **它是字面子串替换，不是过滤器，也不是保证**：
    ///   · 空串 `secret` 一律跳过（`replacingOccurrences(of: "")` 的行为没有意义，
    ///     而"没有配置下载目录"那一格**就是**空串，它每次都会被推下来）；
    ///   · 码以别的写法出现（大小写不同、被 URL 编码、被拆成两段）时**抹不掉**；
    ///   · 它**不试图**去识别"任意第三方文案里像交付码的东西"——那是 §2.7 论证过做不到的事。
    ///     ⇒ 别把这一条读成"这份日志干净了"，口径见模块头那一节。
    public static func redact(_ text: String, secrets: [String]) -> String {
        var out = text
        for secret in secrets where !secret.isEmpty {
            out = out.replacingOccurrences(of: secret, with: redacted)
        }
        return out
    }

    /// 壳的日志文件在哪。
    ///
    /// ⚠️ 取目录走的是 [`ShellStorage.directory`]（壳自己那份标准目录表），
    ///    与 `preferences.json` / `history.json` **同一处来源** ——
    ///    两处各推一次路径的结局是它们哪天分叉，而客户回传时会少拿到一个文件。
    public static var logURL: URL {
        ShellStorage.directory.appendingPathComponent(fileName)
    }

    /// 落一条诊断的**唯一**出口，收一个**显式**级别（[`log`] 传进程级别、
    /// [`logVerbose`] 传 [`Level.verbose`]）。
    ///
    /// ⚠️ 两档的界（上限、留几代、轮转与兜底）**只有 [`write`] / [`rotate`] 一份实现** ——
    ///    本函数只负责"取路径、拼行、写"。
    private static func logAt(_ event: String, _ fields: [(String, String)], level: Level) {
        // ⚠️ 记录分隔符（`\n`）由**这里**补，不在 `formatLine` 里。原因见测试
        //    `aLineHasTheSameShapeAsTheRustLoggers`：那条按空格切开之后要求最后一格
        //    恰好是 `why=…`，换行若留在 `formatLine` 里就会变成 `why=…\n`、那条断言必红。
        //    **测试是规格，实现向它看齐**（Rust 那两份同一条）。
        write(formatLine(event, fields) + "\n", to: logURL, level: level)
    }

    /// 字段行：`ts=<unix 秒> event=<名> k=v k=v …`（**不含**行尾换行，见 [`logAt`]）。
    ///
    /// ⚠️ `\n` 由 [`logAt`] 补（不在本函数里）：换行若留在本函数里，测试
    ///    `aLineHasTheSameShapeAsTheRustLoggers` 那条按空格切的断言必红（"测试是规格"）。
    /// ⚠️ **这一行必须与 Rust 那两份逐字同形**（模块头第 3 条）。
    ///
    /// ⚠️ `now` 是**可注入的**（默认取此刻）：`ts` 是外部输入，把它钉死才能断言
    ///    那一格真的是 unix 秒、而不是别的什么东西格式化出来的。
    static func formatLine(_ event: String,
                           _ fields: [(String, String)],
                           now: Date = Date()) -> String {
        var line = "ts=\(Int(now.timeIntervalSince1970)) event=\(event)"
        for (key, value) in fields {
            // 值里的空格换成 `_`：这一行是按空格切字段读的，一个带空格的值会把字段切开。
            //
            // ⚠️ 同时**截断到 `maxValueChars` 个 Unicode 标量**（**不是** `Character`）：`value` 里可能有第三方
            //    （内核 / 系统）的原始错误文案，长度不受我们控制——不截断的话
            //    "一行"可以是任意长，「有界」就只约束了行数不约束字节数。
            // ⚠️ **次序承重：先截断、后换空格**（Rust 那两份是 `take(n).map(…)`）。
            //
            // 🔴 **截断与空白判定都走 `unicodeScalars`，不是 `Character`。**
            //    Rust 那两份的 `v.chars().take(200)` 里，`char` 是**一个 Unicode 标量**；
            //    而 Swift 的 `Character` 是**扩展字形簇**（可以含任意多个标量）⇒
            //    `value.prefix(200)` 与它**不等价**，差得还很远：
            //      · `"e" + U+0301`（组合重音）是一个 `Character`、**两个**标量 ⇒
            //        按字形簇截 200 会留 600 字节，按标量截 200 是 300 字节；
            //      · ZWJ emoji 更极端；
            //      · `CR LF` 在 Swift 里是**一个** `Character` ⇒ 按 `Character.isWhitespace`
            //        判只出一个 `_`，而 Rust 出两个。
            //    ⚠️ 这**不是纸面差异**：macOS 的文件名是 **NFD（分解形）**，所以 `why` 里
            //    带重音的文件名是现实情形 —— 而它顶破的恰恰是"每一行有上界"这条硬约束。
            //    判据：`aValueIsTruncatedToTwoHundredScalars` 与
            //    `carriageReturnAndLineFeedMakeTwoUnderscores`。
            var clean = ""
            clean.reserveCapacity(maxValueChars)
            for scalar in value.unicodeScalars.prefix(maxValueChars) {
                clean.unicodeScalars.append(scalar.properties.isWhitespace ? "_" : scalar)
            }
            line += " \(key)=\(clean)"
        }
        return line
    }

    // MARK: - 写与轮转（两档只在这里分叉）

    /// 第一次写失败只吵一句（否则每个事件都会往 stderr 刷一行）。
    private final class ComplaintBox: @unchecked Sendable {
        private let lock = NSLock()
        private var complained = false

        /// `true` = 这一次是**第一次**（该吵）。
        func firstTime() -> Bool {
            lock.lock(); defer { lock.unlock() }
            if complained { return false }
            complained = true
            return true
        }
    }

    private static let complaint = ComplaintBox()

    /// 写一行并维持上限。**失败无声**（第一次往 stderr 说一句）。
    ///
    /// 界线取 [`Level.maxBytes`]、轮转交 [`rotate`] —— 两档只在这里分叉。
    static func write(_ line: String, to url: URL, level: Level) {
        let fm = FileManager.default
        let dir = url.deletingLastPathComponent()
        if !dir.path.isEmpty {
            do {
                try fm.createDirectory(at: dir, withIntermediateDirectories: true)
            } catch {
                complainOnce(dir)
                return
            }
        }
        // 轮转：超限就把当前文件挪成 `.1`（覆盖上一代），再从头写。
        //
        // ⚠️ **轮转失败必须走兜底**，不能只是 `try?`：轮转失败而照样追加 ⇒
        //    主文件**无界增长**，而这条路径恰恰是唯一能破坏「有界」这条硬约束的条件。
        //    它在真机上不是纸面路径：日志被别的进程占着 / `.1` 那个名字被占住 ⇒
        //    挪不动。兜底 = **就地截断**：只丢上一代，绝不丢「有界」。
        //
        // ⚠️ **顺序承重：先截断、后吵**。`complainOnce` 要往 stderr 写，而那一步在
        //    某些环境下会失败（甚至带走进程）。两件事同时发生（轮转挪不动 + stderr 不可写）时，
        //    先吵就会让那一步**跳过截断**——恰好在最需要「有界」的时候失效。
        //    （上面两条都落在 [`rotate`] 里，理由与顺序原样保留。）
        if fileSize(url) > level.maxBytes {
            rotate(url, level: level)
        }
        if !fm.fileExists(atPath: url.path) {
            fm.createFile(atPath: url.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: url) else {
            complainOnce(url)
            return
        }
        defer { try? handle.close() }
        _ = try? handle.seekToEnd()
        try? handle.write(contentsOf: Data(line.utf8))
    }

    /// 轮转：把最老的一代丢掉，其余各代依次后移，最后把主文件挪成 `.1`。
    ///
    /// ⚠️ **`normal`（一代）就是两步**：循环一次都不转、只做 `path → .log.1`。
    ///    既有那两条用例钉的就是这条路径。
    ///
    /// ⚠️ **挪不动必须走兜底截断**（理由与顺序见 [`write`]）：
    ///    轮转失败而照样追加 ⇒ 主文件无界增长，而那是唯一能破坏「有界」这条硬约束的条件。
    private static func rotate(_ url: URL, level: Level) {
        if level.generations > 1 {
            for i in stride(from: level.generations - 1, through: 1, by: -1) {
                let older = URL(fileURLWithPath: url.path + ".\(i + 1)")
                let newer = URL(fileURLWithPath: url.path + ".\(i)")
                _ = moveItem(newer, to: older)
            }
        }
        let rotated = URL(fileURLWithPath: url.path + ".1")
        if !moveItem(url, to: rotated) {
            // ⚠️ **顺序承重：先截断、后吵**（理由见 [`write`] 那段注释，一个字都不改）。
            try? Data().write(to: url)
            complainOnce(rotated)
        }
    }

    /// 把 `from` 挪成 `to`，**覆盖**同名的那一代。返回值 = 挪成功没有。
    ///
    /// ⚠️ `FileManager.moveItem` 在**目标已存在**时抛错，而 Rust 那两份的 `rename` 是
    ///    **覆盖** ⇒ 不先删掉的话 `.1`/`.2` 会越攒越多，而"只留 N 代"这条硬约束就破了。
    /// ⚠️ **只删普通文件、不删目录**：目标是个目录时**不删**，让它照 Rust 那边一样失败
    ///    —— 那正是"轮转挪不动"这条真机上会发生的情形，也正是
    ///    `DiagnosticsLogTests.aFailedRotationTruncatesInsteadOfGrowingForever` 的造法。
    private static func moveItem(_ from: URL, to: URL) -> Bool {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        if fm.fileExists(atPath: to.path, isDirectory: &isDirectory), !isDirectory.boolValue {
            try? fm.removeItem(at: to)
        }
        do {
            try fm.moveItem(at: from, to: to)
            return true
        } catch {
            return false
        }
    }

    /// 文件有多少字节（拿不到 ⇒ 0，与 Rust 那两份的 `unwrap_or(0)` 同义）。
    private static func fileSize(_ url: URL) -> UInt64 {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else { return 0 }
        return size.uint64Value
    }

    /// 第一次失败时往 stderr 说一句（**只说一次**，否则每个事件都会刷一行）。
    ///
    /// ⚠️ 走 `try?` 而不是那个不抛错的 `write(_:)`：后者在写失败时会**抛 ObjC 异常**，
    ///    而"诊断日志写不进去"绝不许把产品带走（模块头第 2 条）。
    private static func complainOnce(_ what: URL) {
        guard complaint.firstTime() else { return }
        let text = "诊断日志写不进去（\(what.path)）——**不影响下载**，但出问题时就没有现场证据了。"
            + "请检查该目录是否可写。本句只说一次。\n"
        try? FileHandle.standardError.write(contentsOf: Data(text.utf8))
    }
}
