import Foundation
import CryptoKit   // 🔴 **系统框架**（与 AppKit / Foundation 同一类），不是 SwiftPM 包

// ---------------------------------------------------------------------------
// 「导出诊断日志」：时间戳目录名、要拷哪些文件、那份 `说明.txt` 的**全部文字**，
// 以及真的去拷的那三次系统调用（建目录 / 拷文件 / 写清单）
// —— 详细诊断日志规格 §2.5 / §2.6 / §2.7，任务 6
//
// # 它是 Windows 那一侧的**对应物**（改动必须成对）
//
// 那一边把它拆成了两个文件，本文件把两者合在一处（macOS 这个包里没有第二个 crate 可分）：
//   · `windows/shell-core/src/export.rs` —— 纯逻辑（目录名、拷哪些、说明文件全文、失败话术）；
//   · `windows/shell-win/src/export.rs` —— 真的去拷，外加**逐份文件量事实**。
// **改这里任何一处共享的东西（字段名、目录名形状、失败话术的骨架）都要回去看它们。**
//
// # 🔴 `说明.txt` 的字段表是**跨端契约**（规格 §2.6）
//
// 两端各写一份**字段逐行一致**的说明文件：改字段之前先读 `theHeaderNamesEveryFieldTheSpecAsksFor`
// 那条用例（它逐行点名了那张表，删掉任意一行 ⇒ 红）。**文案允许有细微差别，字段一个都不许少。**
// 本实现的正文与 Windows 那一份**逐字相同**（连"怎么读"那一节都是）—— 两端导出的
// 说明文件因此可以直接对读，这正是"拿到手最先要看的那一页"该有的样子。
//
// # 🔴 事实是**量出来的**，不是这个模块去编的
//
// 每份文件的字节数 / sha256 / 覆盖时间范围住在 `FileFact` 里，由 `export(from:to:header:now:)`
// **逐份文件量完之后**填进去。它量的是**落进导出文件夹的那一份**，不是源文件 ——
// 日志正在被写，从 `copy` 到"量字节数"之间源文件还会长，两边各量一次的话说明文件里
// 的字节数与 sha256 会**互相矛盾**（它们量的是不同的两份内容）。
//
// # 与 Windows 的两处**有意的不对称**（写在这里，不许让它变成"静默地不一样"）
//
// 1. **内核 sha256 的来源**：Windows 的内核是**内嵌在 exe 里**的（`embed.rs` 的
//    `CORE_SHA256`，构建脚本算出来的**编译期常量**）；macOS 的内核是**随 DMG 交付的
//    `benagen-core` 文件**（`CoreClient.locateCoreBinary()` 找到的那一份）⇒ 这一格是
//    **导出时现算**的（[`coreSha256(ofBinaryAt:)`]，走 CryptoKit）。
//    **字段名一样（`core_sha256`），来源不同** —— 看说明文件的人不必知道这件事，
//    但**改这里的人必须知道**：Windows 那一格"任何时候都拿得到"，
//    而这一格拿不到时只能如实写"取不到"（见 [`missingCoreText`]）。
// 2. **OS 那一格**：Windows 走 `GetVersionExW`（`os_from_version` 把三个数拼成
//    `Windows 10.0.19045`）；macOS 走 `ProcessInfo.processInfo.operatingSystemVersionString`
//    （**系统框架自带，不需要任何新依赖**）。两端**都不**把它翻译成市场名字 ——
//    规格 §2.6 给这一格的理由是"放一晚上就断"和"休眠回来就断"**在不同系统版本上是
//    不同的问题**，只写一个平台名等于把这一格废掉。
//
// # 历法自己算（本仓不为一行时间戳引一个日期库）
//
// [`utcParts(_:)`] 用的是 Howard Hinnant 那套 `civil_from_days`，与 Windows 那一份
// **同一个算法、同一份已知答案向量**（含闰日 / 百年不闰 / 四百年又闰三档）。
// ⚠️ 本机有 Foundation 的 `Calendar` 可用，**但这里不用**：两端必须是**同一份实现**，
// 否则"同一台机器上的两次导出"与"两个平台上的两次导出"会用两套历法去算目录名，
// 而那两套哪天分歧了**不会有任何东西变红**。
// ---------------------------------------------------------------------------

public enum DiagnosticsExport {

    // MARK: - 跨端常量

    /// 导出目录的名字前缀（**跨端契约的一部分**：两端生成的目录同名同形）。
    public static let dirPrefix = "诊断日志-"

    /// 清单文件的名字（导出文件夹里**唯一**那份不是日志的文件）。
    public static let headerFileName = "说明.txt"

    /// **内核**那份日志的名字。
    ///
    /// ⚠️ 本包看不见内核写入路径的实现（它是另一个进程、另一个语言），所以这个名字是
    ///    **照抄**的：它的家是 `core/src/paths.rs` 的 `diagnostics_log()`（由
    ///    `settings_file()` 同目录派生）。壳那份的名字**不在这里抄** ——
    ///    它取自 `DiagnosticsLog.fileName`（见 [`allLogNames`] 的注释：抄第二遍的后果是
    ///    改名之后**静默什么也不拷**）。
    public static let kernelLogName = "diag-kernel.log"

    /// 导出文件夹里**可能有**的全部日志名，顺序固定：**内核在前、代数由新到旧**。
    ///
    /// ⚠️ 顺序是**我们定的**，不是调用方给的：同一台机器导两次，两份说明文件要能直接 diff
    ///    （否则"两次现场的区别"要靠人眼对行）。
    ///
    /// ⚠️ 代数取自 `DiagnosticsLog.Level.verbose.generations`（**不是**写死的 2）：
    ///    两档各留几代由 `DiagnosticsLog` 说了算，导出跟着它走 —— 写死一个 2 的话，
    ///    哪天详细档改成留三代，导出的清单会**安静地少一份**。
    ///    ⚠️ 用**详细档**那个数（不是"当前档位"那个数）是刻意的、与 Windows 那一份
    ///    逐字同一条：清单要覆盖"可能存在过的全部文件"，而当前档位决定的是
    ///    "哪些真的生成过"——后者由 [`filesToCopy`] 求交那一层回答。
    ///    ⚠️ 内核那三份的代数同样是 2，但**这一份够不到内核的常量**：
    ///    内核改了代数而壳没改，这里会漏拷（如实记账，与 Rust 那两份的跨语言记账同形）。
    public static func allLogNames() -> [String] {
        var names: [String] = []
        for base in [kernelLogName, DiagnosticsLog.fileName] {
            names.append(base)
            // ⚠️ `stride(through:)` 而不是 `1...n`：后者在 `n == 0` 时是**运行期崩溃**
            //    （"Can't form Range with upperBound < lowerBound"），
            //    而 Rust 那一份的 `1..=n` 在 `n == 0` 时是**空集**。
            //    今天 `generations` 只可能是 1 或 2，但"哪天有人加一档忘了这一处"
            //    绝不该让**导出**把应用带崩（那是本功能最不该发生的一种失败）。
            for generation in stride(from: 1, through: DiagnosticsLog.Level.verbose.generations,
                                     by: 1) {
                names.append("\(base).\(generation)")
            }
        }
        return names
    }

    /// **要拷哪些**：六个可能的名字与"真的存在的那些"求交，顺序见 [`allLogNames`]。
    ///
    /// 详细日志刚打开时后几代本来就还没生成 ⇒ **不存在的不算失败**，不拷它，
    /// 由说明文件如实写"尚未产生"（规格 §4）。
    public static func filesToCopy(existing: [String]) -> [String] {
        allLogNames().filter { existing.contains($0) }
    }

    // MARK: - 说明文件要写的全部事实（规格 §2.6）

    /// **一份日志文件的事实** —— 全部由导出那一侧**量出来**（见模块头那段）。
    public struct FileFact: Equatable, Sendable {
        /// 文件名（`diag-kernel.log` / `diag-shell.log.1` …）。
        public let name: String
        /// 字节数。**原样印出来**（不加千位分隔符）：看日志的人可能要拿它去比对。
        public let bytes: UInt64
        /// sha256（64 位小写十六进制）：传输损坏与"客户改过内容"都靠它判。
        public let sha256: String
        /// 这份日志**头一行**的 `ts=`（unix 秒）。读不到（空文件）⇒ `nil`。
        public let firstTs: UInt64?
        /// 这份日志**最后一行**的 `ts=`（unix 秒）。读不到 ⇒ `nil`。
        ///
        /// ⚠️ 日志正在被写 ⇒ 最后一行可能是**写到一半的**：那时这一格量不出来 ⇒
        ///    说明文件如实写"未知"（编一个范围比写"未知"坏得多）。
        public let lastTs: UInt64?

        public init(name: String, bytes: UInt64, sha256: String,
                    firstTs: UInt64?, lastTs: UInt64?) {
            self.name = name
            self.bytes = bytes
            self.sha256 = sha256
            self.firstTs = firstTs
            self.lastTs = lastTs
        }
    }

    /// **说明文件要写的全部事实**（规格 §2.6 那张表）。
    ///
    /// ⚠️ 除 `files` 之外都是**导出那一刻的环境**。**级别那一格是灵魂**：没有它，
    ///    看日志的人无法判断"这份日志为什么只有这么几行"（是没问题，还是级别没开？）。
    public struct Header: Equatable, Sendable {
        public let appVersion: String
        /// 内核那份二进制的 sha256。⚠️ macOS 上它是**导出时现算**的（模块头第 1 条）。
        public let coreSha256: String
        /// 内核自报的协议版本。**连不上 ⇒ `nil`**（如实写"未连上"，不是失败）。
        public let protocolVersion: Int?
        public let logLevel: String
        public let os: String
        public let arch: String
        public let exportedAt: String
        public let files: [FileFact]

        public init(appVersion: String, coreSha256: String, protocolVersion: Int?,
                    logLevel: String, os: String, arch: String, exportedAt: String,
                    files: [FileFact]) {
            self.appVersion = appVersion
            self.coreSha256 = coreSha256
            self.protocolVersion = protocolVersion
            self.logLevel = logLevel
            self.os = os
            self.arch = arch
            self.exportedAt = exportedAt
            self.files = files
        }

        /// 同一份环境事实 + **另一组量好的文件**（`export` 用它在拷完之后收口）。
        public func withFiles(_ facts: [FileFact]) -> Header {
            Header(appVersion: appVersion, coreSha256: coreSha256,
                   protocolVersion: protocolVersion, logLevel: logLevel, os: os,
                   arch: arch, exportedAt: exportedAt, files: facts)
        }
    }

    /// **导出那一刻的环境事实**（`files` 留空 —— 那几格由 [`export`] 量完再填）。
    ///
    /// 这个函数存在的理由是**把"取数据"与"成句"分开**（与 Windows 那一侧
    /// `commands::diagnostics_export` 先取齐七格、再交给 `header_text` 同一条）：
    /// 于是"级别那一格写的是什么""内核 sha256 从哪来"这些**决定**能在宿主上被断言，
    /// 而视图那一层只剩一句构造。
    ///
    /// ⚠️ `protocolVersion` **没有回落**：调用方给的就是 `AppModel` 从内核的 `hello`
    ///    回执上记下来的那一格（壳**不**拿自己的 `kProtocolVersion` 去填 ——
    ///    那一格回答的是"**内核**自报的是哪一套协议"，填我们自己的常量就把它从
    ///    "内核说的"变成了"我们以为的"）。判据：`theProtocolVersionIsTheKernelsNotOurs`。
    public static func header(appVersion: String, coreBinary: URL?, protocolVersion: Int?,
                              verboseLogging: Bool, os: String, arch: String,
                              at: Date) -> Header {
        Header(appVersion: appVersion,
               coreSha256: coreSha256(ofBinaryAt: coreBinary),
               protocolVersion: protocolVersion,
               // 取值与内核的 `--log-level` **逐字同两个**（`diagnostics::parse_level` 认的就是
               // 这两个串）—— 看日志的人靠这一格判断"这份日志为什么只有这么几行"。
               logLevel: verboseLogging ? "verbose" : "normal",
               os: os,
               arch: arch,
               exportedAt: utcStamp(at: at),
               files: [])
    }

    /// 那份 `说明.txt` 的全文（**纯函数**：同样的 [`Header`] ⇒ 同样的文本）。
    ///
    /// 三段（规格 §2.6 / §2.7）：**风险**（在第一屏）、**有什么**（字段表）、**没有什么**
    /// ——最后那段说的是"这份导出**不**做什么"（不打包、不加锁、不过滤），
    /// 它们各自都是一种**显式的接受**，写在纸上比默认发生强。
    public static func headerText(_ header: Header) -> String {
        // ⚠️ **这是一份给人看的纯文本**（他在文本编辑里打开它）⇒ 正文里**不用**
        //    Markdown 的强调记号（`**` 在那里只是噪音）。分节与缩进是它唯一的排版。
        var out = ""
        out += "诊断日志导出说明\n"
        out += "================\n"
        out += "这份文件是这次导出的清单：它说的不是「日志里有什么内容」，\n"
        out += "而是「这个文件夹里有哪些文件、每一份覆盖了哪一段时间」。\n\n"

        // ---- 第一屏：风险（规格 §2.7 的第 1 处明说）--------------------------------
        out += "⚠️ 风险先说：这份日志会离开你的机器，里面可能有下载地址片段\n"
        out += "（下载引擎的原始报错会原样带上它）。我们没有做自动过滤 —— 要可靠地\n"
        out += "抹掉它，得先能识别它，而我们识别不了任意第三方文案；做一个不可靠的\n"
        out += "过滤器比不做更糟（它会让人以为已经安全了）。\n"
        out += "发送之前，你可以自己打开看一眼。\n\n"
        // ⚠️ **这一段在 2026-10-06 之前是一句绝对句，而它是假的**（终审实测）：交付码与
        //    客户目录名从**内核原文**（那条 404 的 URL、preflight 那句）进得来。
        //    ⇒ 现在分两句：**按构造抹掉的**（那两个串，壳自己知道）与**抹不掉的**
        //    （第三方文案里以别的写法出现的片段）。**与 Windows 那一份逐字相同**
        //    （规格 §2.6 的契约：文案可以有细微差别，这两句**故意**没有）。
        // ⚠️⚠️ **"抹不掉的"是两条，不是一条**（修复轮 2 订正，与 Windows 同一条）：
        //    除了第三方写法那一条，还有**内核自己那条默认下载目录** —— **没配置下载目录**
        //    （默认状态）时壳推下去的是空串（`redact` 跳过它）、也不传 `--download-dir`，
        //    内核于是回落到**它自己**的默认路径（`core/src/paths.rs` →
        //    `C:\Users\<用户名>\Downloads\Benagen` / `~/Downloads/Benagen`）。那个串
        //    **壳不知道**（本仓的 `kernelDefaultDisplayPath` 明说了它只是显示用的），而它
        //    出现在内核 `preflight` 的失败文案里时**是逐字落的**（`core/src/main.rs`：
        //    `目标目录不可写（{dir}）`）⇒ 壳抹不掉它。
        //    **行为是对的**（猜一条客户机上的路径去抹，比不抹更糟）；错的是把话说满。
        out += "本来就不记的：交付码本身、文件路径本身、rpc-secret、客户的目录名。\n"
        out += "（其中交付码与下载目录在写进日志之前会被按字面抹成 [已隐去]；但没配置\n"
        out += "下载目录时，内核会用它自己的默认路径——那一条壳不知道，抹不掉。抹不\n"
        out += "掉的还有第三方文案里以别的写法出现的那一份——见上。）\n\n"

        // ---- 有什么（规格 §2.6 那张表，逐行点名）-----------------------------------
        out += "这里面有什么\n"
        out += "------------\n"
        out += "app_version: \(header.appVersion)\n"
        out += "core_sha256: \(header.coreSha256)\n"
        if let version = header.protocolVersion {
            out += "protocol_version: \(version)\n"
        } else {
            // ⚠️ 连不上内核**不是失败**：这一格如实写"未连上"（编一个版本号比不写更坏）。
            out += "protocol_version: 未连上\n"
        }
        out += "log_level: \(header.logLevel)\n"
        out += "os: \(header.os)\n"
        out += "arch: \(header.arch)\n"
        out += "exported_at: \(header.exportedAt)\n"
        out += "\n"

        // ---- 每份文件（名字 / 字节数 / sha256 / 覆盖范围）--------------------------
        out += "日志文件（每份：名字 / 字节数 / sha256 / 覆盖时间范围）\n"
        out += "----------------------------------------------------\n"
        let present = header.files.map(\.name)
        let absent = allLogNames().filter { !present.contains($0) }
        if header.files.isEmpty {
            // ⚠️ 括号里那句**指路要指对**（与 Windows 那一份逐字同一条订正）：指的
            //    是**紧下面那六行**（每一行都写着「尚未产生」）以及上面那两格
            //    （`log_level` 与 `exported_at`），**不是**"没有什么"那一节
            //    （那一节讲的是"没有压缩包 / 没有加锁 / 没有过滤"，回答不了"为什么一份都没有"）。
            out += "没有任何日志文件（下面每一行都写着「尚未产生」）。\n"
        }
        for file in header.files {
            out += "\(file.name)  \(file.bytes) 字节  sha256=\(file.sha256)  覆盖范围: \(coverage(file))\n"
        }
        for name in absent {
            out += "\(name)  尚未产生（这一份日志还没有生成过）\n"
        }
        out += "\n"

        // ---- 没有什么 --------------------------------------------------------------
        out += "没有什么（都是显式的选择，不是漏做）\n"
        out += "--------------------------------------\n"
        out += "· 没有压缩包：导出就是一个文件夹，本文件是它的清单；\n"
        out += "· 没有加锁、也没有做快照：导出时日志可能仍在被写 ⇒\n"
        out += "  某一份的最后一行可能是写到一半的（代价远大于收益，规格 §4）；\n"
        out += "· 没有敏感信息自动过滤（理由见第一屏那段）。注意「把交付码与下载目录按字面\n"
        out += "  抹成 [已隐去]」不是过滤：抹的是壳自己知道的那两个串本身，不是去识别\n"
        out += "  任意文案里像不像它。\n\n"

        // ---- 怎么读 ----------------------------------------------------------------
        out += "怎么读\n"
        out += "------\n"
        out += "每份日志一行一条记录，形状是：ts=<unix 秒> event=<事件名> 键=值 …\n"
        out += "那个时间是 UTC 的 unix 秒（macOS `date -r <ts>`、Linux `date -d @<ts>`）。\n"
        out += "「覆盖范围」那一格取的是这份日志的头尾两行：一眼就能看出它有没有\n"
        out += "覆盖到出问题的那一段时间。没覆盖到的那一刻只有等下一次现场 ——\n"
        out += "所以请在复现之前把详细日志打开，导出之后可以再关掉。\n"
        return out
    }

    /// 一份文件的"覆盖时间范围"那一格：头尾两个 `ts=`，**读不到就写"未知"**。
    static func coverage(_ file: FileFact) -> String {
        if let first = file.firstTs, let last = file.lastTs {
            return "\(first) → \(last)"
        }
        return "未知"
    }

    // MARK: - 时刻（UTC，历法自己算）

    /// 时间戳子目录的名字：`诊断日志-YYYYMMDD-HHMMSS`（**UTC**，见 [`utcParts`]）。
    ///
    /// 判别力：写成固定名字 ⇒ 客户导第二次时**把第一次的现场覆盖掉了**，
    /// 而两次的时间点不同、恰好是最需要对比的时候。
    public static func subdirectoryName(at: Date) -> String {
        let (year, month, day, hour, minute, second) = utcParts(unixSeconds(at))
        return "\(dirPrefix)\(pad(year, 4))\(pad(month, 2))\(pad(day, 2))"
            + "-\(pad(hour, 2))\(pad(minute, 2))\(pad(second, 2))"
    }

    /// 导出时刻那一格（`2025-10-06 09:23:11 UTC`）。
    ///
    /// ⚠️ **与 [`subdirectoryName`] 用同一个历法**（同一个 [`utcParts`]）：两处各算一遍的
    ///    话，同一份导出里"文件夹名"与"导出时刻"会漂开一个时区，而没有任何东西会变红。
    ///
    /// ⚠️ 标的是 **UTC** 而不是"本机时区"。日志里的 `ts=` 本来就是 UTC 的 unix 秒
    ///    （`DiagnosticsLog.formatLine`），两边同源反而好对；而写"本机时间"是一句假话
    ///    —— 排查的人会按错误的偏移去对。
    public static func utcStamp(at: Date) -> String {
        let (year, month, day, hour, minute, second) = utcParts(unixSeconds(at))
        return "\(pad(year, 4))-\(pad(month, 2))-\(pad(day, 2)) "
            + "\(pad(hour, 2)):\(pad(minute, 2)):\(pad(second, 2)) UTC"
    }

    /// 零填充（Swift 没有 `format!("{year:04}")` 的直译；`String(format:)` 走的是
    /// `NSNumber`，为这一点格式引一条 Foundation 的格式化路径不值当）。
    private static func pad(_ value: Int, _ width: Int) -> String {
        let text = String(value)
        if text.count >= width { return text }
        return String(repeating: "0", count: width - text.count) + text
    }

    /// 此刻的 unix 秒。
    ///
    /// ⚠️ 早于 1970 时回 `0`（与 Windows 那一侧的 `unwrap_or(0)` 同义）：那种时钟下
    ///    本来就没有正确的"此刻"，而编一个负数只会把日期算到 1969 年去。
    ///    `Date` 的秒是浮点，这里**截断**（不是四舍五入）—— 与 Rust 的 `as_secs()` 同义。
    static func unixSeconds(_ at: Date) -> UInt64 {
        let seconds = Int64(at.timeIntervalSince1970)
        return seconds < 0 ? 0 : UInt64(seconds)
    }

    /// unix 秒 → **UTC** 的 (年, 月, 日, 时, 分, 秒)。
    ///
    /// 本仓没有日期库可用/可用也**不用**（模块头那段），所以历法**在这里自己算**：
    /// 用的是 Howard Hinnant 那套 `civil_from_days`（把"纪元日"换算成公历年月日），
    /// 它**天然带闰年规则**（含"能被 100 整除不闰、能被 400 整除又闰"那两条）。
    ///
    /// ⚠️ **这一段是本模块技术含量最高的一小段**：算错的后果是**目录名与日志内容对不上**，
    ///    而它**不会自己变红** —— 判据是 `theCalendarMatchesKnownAnswers` 那几条
    ///    **算出来的**向量（含闰日、百年不闰、四百年又闰三档）。
    static func utcParts(_ unix: UInt64) -> (year: Int, month: Int, day: Int,
                                             hour: Int, minute: Int, second: Int) {
        // 每天的秒数远小于 Int64 的上界（`Int64.max / 86_400` 约 1.07e14 天），
        // 而 `UInt64` 的最大值也在这个范围内 ⇒ 这两次转换不会溢出。
        let days = Int(unix / 86_400)
        let rest = Int(unix % 86_400)
        let (year, month, day) = civilFromDays(days)
        return (year, month, day, rest / 3_600, (rest % 3_600) / 60, rest % 60)
    }

    /// 纪元日（1970-01-01 = 0）→ 公历 (年, 月, 日)。闰年规则在这几行里。
    ///
    /// ⚠️ 与 `windows/shell-core/src/export.rs` 的同名函数**逐行同构**（连注释里的
    ///    数字都一样）：两端各写一份是**架构强制**的（另一种语言），
    ///    但算法必须是一份，否则"两个平台导出的目录名"哪天分歧了不会有东西变红。
    static func civilFromDays(_ days: Int) -> (year: Int, month: Int, day: Int) {
        // 把纪元挪到 0000-03-01：这样"闰日"落在**这一年的最后**，
        // 于是"每 4 年一闰"是一条不打断的循环（3 月 1 日起算，2 月 29 日就是年末那一天）。
        let shifted = days + 719_468
        // ⚠️ Swift 的 `/` 与 Rust 一样是**向零截断**（不是向下取整），所以负数的修法
        //    （`shifted - 146_096`）在两边是同一个语义。本函数的调用方只喂正数，
        //    但这一行照抄，免得哪天有人扩了取值范围而这里静默地不一样。
        let era = (shifted >= 0 ? shifted : shifted - 146_096) / 146_097
        let dayOfEra = shifted - era * 146_097          // [0, 146096]
        // 400 年里每天的位置 → 年内的天序号（去掉每 4 年那一闰、加回每 100 年那一不闰、
        // 再去掉每 400 年那一闰）。
        let yearOfEra = (dayOfEra - dayOfEra / 1_460 + dayOfEra / 36_524
                         - dayOfEra / 146_096) / 365
        let year = yearOfEra + era * 400
        let dayOfYear = dayOfEra - (365 * yearOfEra + yearOfEra / 4 - yearOfEra / 100)
        // 3 月 1 日起算的"月序号" → 真月（`(5 * doy + 2) / 153` 是那条固定的分段线性映射）。
        let monthIndex = (5 * dayOfYear + 2) / 153
        let day = dayOfYear - (153 * monthIndex + 2) / 5 + 1
        let month = monthIndex < 10 ? monthIndex + 3 : monthIndex - 9
        return (month <= 2 ? year + 1 : year, month, day)
    }

    // MARK: - 三次系统调用（建目录 / 拷文件 / 写清单）

    /// 建不出目录 / 拷不进一份日志 / 写不了说明文件时**抛出去的那句话**。
    ///
    /// ⚠️ 它**只装那句话**（可直接显示的完整句子：哪一步、哪个位置、系统原话、下一步）——
    ///    与 Windows 那一侧的 `Result<String, String>` 同一条口径。
    ///    三档各有各的句子（见三个 `…FailureText`）：**共用一句模板的后果是对用户说错东西**。
    public struct ExportFailure: Error, Equatable, Sendable {
        public let message: String
        public init(message: String) { self.message = message }
    }

    /// **建不出那个子目录**那句话（规格 §4：一句人话 —— 哪个位置、为什么、下一步）。
    ///
    /// 🔴 **三处失败各有各的说法**（本函数 / [`copyFailureText`] / [`headerFailureText`]）：
    ///    它们**不是同一件事**，共用一句模板的后果是**对用户说错东西** —— 拷一份日志失败时
    ///    告诉他"建不了文件夹"，他会去查一个**建得出来**的位置，而真正出错的是那一份文件。
    public static func folderFailureText(folder: String, cause: String) -> String {
        "诊断日志没有导出：建不了文件夹 \(folder)（系统原话：\(cause)）。"
            + "补救：换一个能写的文件夹再试一次（比如桌面），或检查那个位置是不是一个文件、磁盘是不是满了。"
    }

    /// **某一份日志拷不进去**那句话（第二档：建目录成了、拷这一份时失败）。
    ///
    /// 补救说的是这一档真正能做的事：某个文件被别的程序占着（**用户正用文本编辑开着日志**
    /// 是真机上最常见的形状）时，关掉它再试一次就有用 —— 而"换个文件夹"没用。
    ///
    /// ⚠️ Windows 那一份这里是"记事本"（`windows/shell-core/src/export.rs:248`）：**文案
    ///    允许有细微差别**（规格 §2.6 的口径），macOS 上对应的程序叫"文本编辑"。
    ///    这是两端**有意**的一处用词差异，不是漏改。
    public static func copyFailureText(file: String, cause: String) -> String {
        "诊断日志没有导出：拷不进 \(file)（系统原话：\(cause)）。"
            + "补救：确认那一份文件还在、且没有被别的程序独占（比如你正用文本编辑打开着它），然后重试一次。"
    }

    /// **说明文件写不进去**那句话（第三档：日志都拷好了、清单写不下去）。
    public static func headerFailureText(file: String, cause: String) -> String {
        "诊断日志没有导出：说明文件写不进去（\(file)；系统原话：\(cause)）。"
            + "补救：检查目标磁盘是不是满了、那个文件夹是不是只读，然后重试一次。"
    }

    /// 把日志与说明文件拷进 `<目标>/诊断日志-YYYYMMDD-HHMMSS/`。**返回那个目录**。
    ///
    /// ⚠️ 日志正在被写 ⇒ 拷贝可能拿到"写到一半的最后一行"。**如实记在说明文件里**
    ///    （规格 §4），**不做加锁、不做快照** —— 代价远大于收益。
    ///
    /// ⚠️ **目标目录不可写 ⇒ 抛一句人话**（哪个目录、为什么），**绝不静默成功**：
    ///    界面上说"导出成功"、而客户发回来的文件夹是空的，比报错坏得多。
    ///
    /// ⚠️ `logDir` 是**参数**（不是就地取 `DiagnosticsLog.logURL`）：判据要能在临时目录上
    ///    真跑一遍这条链路，而 `DiagnosticsLog.logURL` 指向的是**真人正在用的那个目录**
    ///    （测试绝不许往那儿写东西）。生产调用点传的正是它 —— 见 `SettingsView`。
    ///
    /// 🔴 **`now` 没有默认值，而且必须与 `header(at:)` 那一个是同一个值**
    ///    （2026-10-06 终审）：这个时刻有**两个**落点 —— 文件夹名
    ///    （`诊断日志-YYYYMMDD-HHMMSS`）与 `说明.txt` 里那格 `exported_at`。
    ///    两处各读一次时钟的话，跨过秒边界时**同一个导出里那两个时刻会差一秒**，
    ///    而它看起来完全正常（两个都是"合理的时间"，谁也看不出它们对不上）。
    ///    ⇒ 与 Windows 那一侧（`shell-win/src/commands.rs` 的 `at`）**逐条同款**：
    ///    **读时钟的那一处只有一个**（`SettingsView.runExport`），其余全是纯参数。
    ///    ⚠️ **不给默认值**是本仓既有的一条纪律（`CoreClient.coreArguments` 的
    ///    `verboseLogging` 同款）：给了默认值之后，漏传的新调用点会**静默地**读第二次
    ///    时钟 —— 而"少传一个实参应该是编译错误"。
    ///
    /// ⚠️ 残余（如实记账）：**"调用方那两个实参是不是同一个值"没有判据** ——
    ///    那一句住在 `SettingsView` 里，而本项目的硬约束是"视图不单测"。
    ///    本文件能钉住的是"`now` 决定了文件夹名"（`theExportFolderHoldsTheLogsAndAMeasuredHeader`
    ///    把那个时刻同时印在目录名与 `exported_at` 上）。
    public static func export(from logDir: URL, to target: URL,
                              header: Header, now: Date) throws -> URL {
        let folder = target.appendingPathComponent(subdirectoryName(at: now), isDirectory: true)
        // 🔴 **不许吞掉**：建不出来就往上抛（界面要说"这次没成"）。
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            throw ExportFailure(message: folderFailureText(folder: folder.path,
                                                           cause: error.localizedDescription))
        }

        // 只拷**存在的**那些：详细日志刚打开时后几代还没有（不存在不算失败）。
        let existing = fileNames(in: logDir)
        var facts: [FileFact] = []
        for name in filesToCopy(existing: existing) {
            let to = folder.appendingPathComponent(name)
            do {
                try FileManager.default.copyItem(at: logDir.appendingPathComponent(name), to: to)
            } catch {
                throw ExportFailure(message: copyFailureText(file: to.path,
                                                             cause: error.localizedDescription))
            }
            // ⚠️ 事实从**落进导出文件夹的那一份**上量（模块头那段：源文件还在长）。
            facts.append(measure(to, name: name))
        }

        // 说明文件最后写：它要说的是"这个文件夹里**已经**有什么"。
        let text = headerText(header.withFiles(facts))
        let headerPath = folder.appendingPathComponent(headerFileName)
        do {
            try Data(text.utf8).write(to: headerPath)
        } catch {
            throw ExportFailure(message: headerFailureText(file: headerPath.path,
                                                           cause: error.localizedDescription))
        }
        return folder
    }

    /// 日志目录里**现有的文件名**（读不动 ⇒ 空集：那与"一份都还没生成"是同一档处置）。
    static func fileNames(in dir: URL) -> [String] {
        let keys: [URLResourceKey] = [.isRegularFileKey]
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: dir, includingPropertiesForKeys: keys) else { return [] }
        return entries
            .filter { (try? $0.resourceValues(forKeys: Set(keys)).isRegularFile) ?? false }
            .map(\.lastPathComponent)
    }

    /// 量一份**已经在导出文件夹里**的日志：字节数、sha256、覆盖的头尾两个时刻。
    ///
    /// ⚠️ 三格**一次量完**（都在同一份字节上）：分几次读的话，日志一长就会量出
    ///    "sha256 是这一份、字节数是那一份"那种自相矛盾（而它看不出来）。
    ///    日志上界 12 MiB ⇒ 整份读进来是几十毫秒的事，用不着像 Windows 那样只读末尾 8 KiB
    ///    （那边要先算 sha256、又要单独找最后一行，所以分了两次读）。
    /// ⚠️ 读不动 ⇒ 全 `nil` / 0（与 Rust 那两份的 `unwrap_or_default()` 同义）：
    ///    说明文件会如实写"未知"与 0 字节，而不是把导出整个弄失败。
    static func measure(_ url: URL, name: String) -> FileFact {
        let bytes = (try? Data(contentsOf: url)) ?? Data()
        let (first, last) = firstAndLastTs(bytes)
        return FileFact(name: name, bytes: UInt64(bytes.count),
                        sha256: sha256Hex(bytes), firstTs: first, lastTs: last)
    }

    /// 一份日志**头尾两行**的 `ts=`（读不到 ⇒ `nil`，说明文件如实写"未知"）。
    static func firstAndLastTs(_ data: Data) -> (UInt64?, UInt64?) {
        // `String(decoding:)` 对非法 UTF-8 是**替换**而不是失败（同 Rust 的 `from_utf8_lossy`）：
        // 这一行只用来定位 `ts=`，坏字节不该让整份导出失败。
        let text = String(decoding: data, as: UTF8.self)
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        let first = lines.first.flatMap(tsOf)
        let last = lines.reversed().lazy.compactMap(tsOf).first
        return (first, last)
    }

    /// 一行里的 `ts=`（`ts=<unix 秒> event=<名> …`，形状由那三份日志器的 `formatLine` 钉着）。
    ///
    /// ⚠️ **只认完整的记录**（`ts=` 之后还得有 ` event=`）：日志正在被写时末尾可能是半条
    ///    记录 —— 认它会把一个**被截断的数字**当成时刻（`1759746000` 被截成 `1759746`），
    ///    而那是"覆盖范围"那一格里一个**看起来很正常**的错值。
    static func tsOf(_ line: Substring) -> UInt64? {
        guard line.hasPrefix("ts=") else { return nil }
        let rest = line.dropFirst(3)
        guard rest.contains(" event=") else { return nil }   // 写到一半的一行
        guard let value = rest.split(separator: " ").first else { return nil }
        return UInt64(value)
    }

    // MARK: - 环境事实

    /// 内核那份二进制的 sha256（说明文件里"内核是哪一份"那一格）。
    ///
    /// ⚠️ **与 Windows 有意的不对称**（模块头第 1 条）：那边这一格是 `embed::CORE_SHA256`
    ///    这个**编译期常量**（内核被内嵌进 exe）；macOS 的内核是**随包交付的一个文件**
    ///    （`CoreClient.locateCoreBinary()` 找到的那一份）⇒ 这里**现算**。
    ///    **字段名一样（`core_sha256`），来源不同** —— 那一格回答的问题（"内核是哪一份"）
    ///    两端一致，这正是规格 §2.6 要的（README §10 R8 的口径：sha 才是身份）。
    ///
    /// ⚠️ 拿不到（文件不在预期位置、读不动）⇒ 如实写"取不到"，**不编一个哈希**。
    ///    Windows 那一侧的对应文案是"（宿主构建：没有内嵌内核）"—— 那边宿主**根本没有**内核，
    ///    而这边是"有这个文件但没找到"，两句话说的是两件事，所以不逐字照抄。
    public static func coreSha256(ofBinaryAt url: URL?) -> String {
        guard let url, let data = try? Data(contentsOf: url) else { return missingCoreText }
        return sha256Hex(data)
    }

    /// [`coreSha256(ofBinaryAt:)`] 取不到那一格时写的字（**不是**一个哈希值）。
    public static let missingCoreText = "（取不到：内核可执行文件不在预期位置）"

    /// sha256（64 位小写十六进制）。
    ///
    /// 🔴 `import CryptoKit` 是**系统框架**（与 AppKit / Foundation 同一类），
    ///    **不是**一个 SwiftPM 包 ⇒ `Package.swift` 的 `dependencies: []` 一个字没动，
    ///    本仓"零新依赖"那条约束（它管的是包）没有被碰。
    public static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// 说明文件里「`os:`」那一格：**系统与版本**（规格 §2.6）。
    ///
    /// ⚠️ **为什么不是一个平台名**：规格给这一格的理由是"**"放一晚上就断"和"休眠回来就断"
    ///    在不同系统版本上是不同的问题**" —— 只写 `macOS` 等于把那一格废掉。
    ///    这里取的是系统自己那句话（`Version 15.6.0 (Build 24G84)` 那种形状），
    ///    **原样附在平台名后面**：三个数（主/次/build）一个不少，而**不解析、不翻译**它 ——
    ///    换个系统版本那句文案的形状就可能变，解析出来的东西一旦对不上就是一句
    ///    "看起来很正常"的假话（Windows 那一份拒绝把 build 号翻译成"Win10/11"
    ///    是同一条理由）。
    public static func osVersion() -> String {
        osField(versionString: ProcessInfo.processInfo.operatingSystemVersionString)
    }

    /// **拼装那一格**（纯函数 —— 判据在宿主上跑得了，`os:` 那一格才有判别力）。
    ///
    /// 取不到（空串 / `nil`）⇒ 如实写"版本取不到"，**不编一个数**，也**不退化成平台名**
    /// （退化的表现是这一格回答不了它存在的问题，而没有任何东西会变红）。
    public static func osField(versionString: String?) -> String {
        let text = (versionString ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return "macOS（版本取不到）" }
        return "macOS \(text)"
    }

    /// 说明文件里「`arch:`」那一格。
    ///
    /// ⚠️ 与 Windows 那一侧的 `std::env::consts::ARCH` **同一条口径：编译期**。
    ///    它在 Rosetta 下报的是**这个二进制**的架构（x86_64），而那正是有用的那一格
    ///    （"内核跑在哪个架构上"取决于壳起的那个进程）。
    public static var arch: String {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }

    // MARK: - 那一次动作的回执（规格 §2.5 / §2.7）

    /// 导出成功后界面上必须出现的那句话（规格 §2.7 的**第 2 处明说**）。
    ///
    /// 🔴 **它是一条显式接受的风险，不是一个"已经处理好了"的保证**：日志里可能有下载地址
    ///    片段（引擎的原始报错会原样带上它），而**没有**做自动过滤（做一个不可靠的过滤器
    ///    比不做更糟 —— 它会让人以为已经安全了）。⇒ 于是把它**说出来**，
    ///    由用户自己决定要不要先打开看一眼。
    /// ⚠️ 与 Windows 那一份的 `api::diagnostics::PRIVACY_NOTE` **逐字同源**。
    public static let privacyNote = "这份日志可能包含下载地址片段，发送前你可以自己打开看一眼。"

    /// 那颗按钮的标题（与 Windows 那一侧 `index.html` 的 `#set-export` **逐字同源**：
    /// 两端给客户看的是同一个入口名）。
    public static let buttonLabel = "导出诊断日志…"

    /// 那颗按钮的 `.help`（说明它**做什么**，以及"会建一个子目录"这件用户看不见的事）。
    public static let buttonHelp = "把日志与一份说明文件拷进你选的文件夹"
        + "（里面会新建一个带时间戳的子目录，导多少次都不会互相覆盖）"

    /// 选择目标文件夹那张面板上的一句话（与既有那个选目录面板同款）。
    public static let panelMessage = "诊断日志会导出到这个文件夹里"

    /// 一次「导出诊断日志…」的结果。**三档都不是错误**（取消也一样），
    /// `isFailure` 决定界面画什么图标/颜色。
    ///
    /// ⚠️ 形状与 `DownloadDirChange` / `VerboseLoggingChange` / Windows 那一侧的
    ///    `api::diagnostics` **同形**：**标题里绝不含路径**（路径单独一格、单行中间截断）——
    ///    那是一次真实布局事故的修法。
    ///
    /// ⚠️ 它落在**视图的 `@State`** 上，不落模型（与上面两个回执**不同**）：那两个改的是
    ///    **内核的 argv**，用户关掉设置窗口之后还在被它影响（"没改成"那一格是唯一的提示）；
    ///    导出是**一次同步完成的动作** —— 回执写下的那一刻用户还没拿回控制权（系统对话框
    ///    与拷贝都在同一次调用里跑完），窗口关掉之后也没有任何"还在生效的状态"会骗人。
    ///    这就是 Windows 那一侧的形状（它把回执画在**本节**里，同样不落会话状态）。
    public enum Outcome: Equatable, Sendable {
        /// 导出成了：`folder` 是那个带时间戳的子目录。
        case done(folder: String)
        /// 用户取消了「选择文件夹」：**什么都没发生，而且这不是失败**。
        case cancelled
        /// 没成：这是**可直接显示给用户的那句话**（三个 `…FailureText` 拼好的，照登）。
        case failed(message: String)

        /// **这条回执的第一行**：固定短的标题，**任何情况下都不含路径**
        /// （`.failed` 那一格是例外：它是系统/内核原文，照登 —— 但它是**路径**那一格的来源
        /// 不是标题的来源，视图那边照 `DownloadDirChange` 的规矩把高度封顶）。
        public var headline: String {
            switch self {
            case .done: return "诊断日志已导出"
            case .cancelled: return "没有导出（你取消了选择文件夹）"
            case .failed(let message): return message
            }
        }

        /// 要单独占一行显示的那条路径（`nil` = 这一档没有路径可说）。
        public var pathDetail: String? {
            if case .done(let folder) = self { return folder }
            return nil
        }

        /// 这一行说的是"没成"吗（视图据此选图标与颜色）。**取消不是失败。**
        public var isFailure: Bool {
            if case .failed = self { return true }
            return false
        }

        /// 那句隐私提醒。**只在成功那一档有值**（没导出东西的时候没有什么可提醒的）。
        public var privacyNote: String? {
            if case .done = self { return DiagnosticsExport.privacyNote }
            return nil
        }
    }
}
