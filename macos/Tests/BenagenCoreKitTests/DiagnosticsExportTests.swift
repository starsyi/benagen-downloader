import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「导出诊断日志」（规格 §2.5 / §2.6 / §2.7，任务 6）
//
// ⚠️ 本文件与 Windows 那一侧的两组用例**逐条对应**：
//     `windows/shell-core/src/export.rs`（说明文件与目录名）与
//     `windows/shell-win/src/export.rs`（真的去拷）。**两边必须成对**：
//     客户从哪个平台导出来的文件夹，分析的人拿到的都该是同一个东西。
//
// ⚠️⚠️ **安全红线**：本文件里的每一次落盘都指向 `temporaryDirectory` 下的一个
//     一次性目录。**没有任何一条**用例碰 `DiagnosticsLog.logURL` /
//     `AppPreferencesStore.defaultURL`（人类伙伴**真实在用**的那两处）。
//     也没有任何一条调 `export(from: DiagnosticsLog.logURL…)`：被测的入口收
//     **显式**的日志目录（`export(from:to:header:now:)`），生产调用点才传那个真目录。
// ---------------------------------------------------------------------------

/// 一个用完就删的一次性临时目录。
private struct TempDir {
    let url: URL

    init(_ tag: String) throws {
        url = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-export-swift-tests", isDirectory: true)
            .appendingPathComponent("\(tag)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    func cleanUp() { try? FileManager.default.removeItem(at: url) }
}

/// 一份**量好的事实**（除名字外都是量出来的，不是编的）。
private func aFileFact() -> DiagnosticsExport.FileFact {
    DiagnosticsExport.FileFact(name: "diag-kernel.log",
                               bytes: 1_234_567,
                               sha256: String(repeating: "bb", count: 32),
                               firstTs: 1_759_742_591,
                               lastTs: 1_759_746_000)
}

/// 一份**填满的**表头（除调用方要改的那一格）。
private func aHeader() -> DiagnosticsExport.Header {
    DiagnosticsExport.Header(
        appVersion: "0.2.0",
        coreSha256: String(repeating: "aa", count: 32),
        protocolVersion: 1,
        logLevel: "verbose",
        os: "macOS Version 15.6.0 (Build 24G84)",
        arch: "arm64",
        // ⚠️ 这一格是**生产那一侧**用 `utcStamp(at:)` 现算的，形状是 `<日期> <时刻> UTC`。
        //    夹具跟着它走 —— 写成 `+08:00` 那种形状的话，本用例断的是一个**再也产不出来**
        //    的输入（Windows 那一侧的修复轮 1 抓到过同一件事）。
        exportedAt: "2026-10-06 14:03:11 UTC",
        files: [aFileFact()])
}

// ---------------------------------------------------------------------------
// 字段表（规格 §2.6 的跨端契约）
// ---------------------------------------------------------------------------

/// 🔴 **说明文件的每一行都在**（规格 §2.6 那张表；逐行点名，不是数个数）。
///
/// 判别力：删掉任何一行 ⇒ 红。尤其 **`log_level` 那一行**：没有它，看日志的人
/// 无法判断"这份日志为什么只有这么几行"（是没问题，还是级别没开？）。
@Test func theHeaderNamesEveryFieldTheSpecAsksFor() {
    let text = DiagnosticsExport.headerText(aHeader())
    let needles = [
        "0.2.0",                                   // 应用版本
        String(repeating: "aa", count: 32),        // 内核二进制的 sha256
        "protocol_version: 1",                     // 内核自报的协议版本
        "log_level: verbose",                      // 本次是哪个档 ← 最关键的一格
        "macOS Version 15.6.0 (Build 24G84)",      // 系统与版本
        "arm64",                                   // 架构
        "2026-10-06 14:03:11 UTC",                 // 导出时刻
        "diag-kernel.log",                         // 每份文件：名字
        "1234567",                                 // …字节数
        String(repeating: "bb", count: 32),        // …sha256
        "1759742591",                              // …与它覆盖的头尾两个时刻
        "1759746000",
    ]
    for needle in needles {
        #expect(text.contains(needle), "说明文件里少了 \(needle)：\n\(text)")
    }
}

/// 🔴 **风险那段必须在第一屏（前 20 行之内），而且钉的是那条风险本身**。
///
/// ⚠️ Windows 那一侧**先出过一次这个缺陷**（判据写成
/// `contains("交付码") || contains("下载地址")`，于是下面"本来就不记的：交付码本身…"
/// 那一句**自己**就满足了它 —— 把整段 ⚠️ 风险删掉，那条判据照旧绿）。规格 §2.7 要的是
/// "第一屏写明**这条风险**"：日志里**可能包含下载地址片段**。
/// ⇒ 这里**钉那个短语、去掉 OR**（删掉那段 ⇒ 本用例红），另外单独钉"那一段确实是风险段"
/// （第一屏里那一段以 `⚠️ 风险先说` 开头）。
@Test func theRiskParagraphIsOnTheFirstScreenAndNamesTheRiskItself() {
    let text = DiagnosticsExport.headerText(aHeader())
    let firstScreen = text.split(separator: "\n", omittingEmptySubsequences: false)
        .prefix(20).joined(separator: "\n")
    #expect(firstScreen.contains("下载地址片段"),
            "风险那段必须在前 20 行里，而且要说的是**那条风险**：\n\(firstScreen)")
    #expect(firstScreen.contains("⚠️ 风险先说"),
            "第一屏里那一段必须是风险段本身（不是别处一句碰巧提到「交付码」的话）：\n\(firstScreen)")
    // 🔴 **"本来就不记的"那一句必须与它的限定句一起出现**（与 Windows 那一侧逐条对齐，
    //    修复轮 2 补的**对位判据**）。
    //
    // ⚠️ 为什么要有它：这两句的文本**两端逐字相同**（规格 §2.6 的契约），改动纪律是
    //    "改一边必须改另一边"—— 但在补这一条之前，**这一侧一个字都没有在钉**：
    //    只把 macOS 那句改回绝对句（Windows 不动）时，全部 Swift 测试照样绿。
    //    ⇒ 现在两侧各有一条只读自己那份文本的判据（Rust 侧那条在
    //    `windows/shell-core/src/export.rs` 的 `the_header_names_every_field_the_spec_asks_for`）。
    //
    // ⚠️ 这两条钉的是**文案的口径**，不是行为：真去抹的那一步在 `CoreClient.setRedactions`
    //    （判据在 `CoreClientTests`），而"调用点有没有被走到"只有真机验收能回答
    //    （`AppModel.rememberRedactions` 的诚实记账）。
    #expect(firstScreen.contains("按字面抹成 [已隐去]"),
            "把绝对句改回来是不行的：交付码与目录名只被**按字面**抹掉，抹不掉的那一份（第三方文案里以别的写法出现的）必须写在旁边：\n\(firstScreen)")
    #expect(firstScreen.contains("内核会用它自己的默认路径"),
            "没配置下载目录时内核那条默认路径抹不掉 —— 这一句必须写在旁边，否则说明文件在默认配置下就是一句假话：\n\(firstScreen)")
}

/// 🔴 **导出时刻那一格：逐字透传 + 形状必须是生产那一侧真的产得出的那一种**。
///
/// 判别力（两条各挡一种写法）：
///   · 把 `exported_at:` 那一行整行删掉 ⇒ 第一条红；
///   · 把夹具改回 `+08:00` 那种形状（今天再也产不出来）⇒ 第二条红 ——
///     **逐字透传**意味着格式漂了不会有任何别的东西红。
///     ⚠️ 它也是"必须写明是 UTC"这一条的守卫。
@Test func theHeaderPrintsTheMomentVerbatimWithItsClock() {
    let text = DiagnosticsExport.headerText(aHeader())
    #expect(text.contains("exported_at: 2026-10-06 14:03:11 UTC"), "\(text)")

    // 生产那一侧产得出的形状（`DiagnosticsExport.header(…)` 用的就是它）。
    let produced = DiagnosticsExport.utcStamp(at: Date(timeIntervalSince1970: 1_759_742_591))
    #expect(produced == "2025-10-06 09:23:11 UTC", "实际：\(produced)")
    let fixture = aHeader().exportedAt
    #expect(fixture.count == produced.count,
            "夹具里那一格的形状与 utcStamp 产出的不同：\(fixture)")
    #expect(fixture.hasSuffix(" UTC"), "导出时刻必须写明是哪一个钟：\(fixture)")
}

/// **一份文件都没量到**（日志还没产生过）⇒ 说明文件照生成，如实写"一份都没有"。
///
/// 判别力：删掉 `headerText` 里那个 `if header.files.isEmpty` 分支 ⇒ 第一条红
/// （那一句**只有**那条分支产得出来）。
@Test func aHeaderWithNoFilesSaysSoInsteadOfBeingEmpty() {
    let text = DiagnosticsExport.headerText(aHeader().withFiles([]))
    #expect(text.contains("没有任何日志文件"),
            "这一句**只有**那条分支产得出来（删掉分支 ⇒ 本用例红）：\n\(text)")
    // ⚠️ 与上面那条**互相独立**：它钉的是"六个名字一个不漏地**各占一行**"
    //    （按**行首的名字**数，不靠上面那句散文）。
    let named = DiagnosticsExport.allLogNames()
        .filter { name in text.split(separator: "\n").contains { $0.hasPrefix("\(name)  ") } }
        .count
    #expect(named == 6, "六个名字（内核三份 + 壳三份）要一个不漏地点名：\n\(text)")
    #expect(text.count > 100, "不许退化成一个空文件：\(text)")
}

/// **覆盖范围缺失时如实写"未知"**（读不到头尾行时分不出是哪一种，而编一个范围比写"未知"坏得多）。
@Test func aMissingCoverageRangeIsStatedNotInvented() {
    let fact = DiagnosticsExport.FileFact(name: "diag-kernel.log", bytes: 10,
                                          sha256: String(repeating: "cc", count: 32),
                                          firstTs: nil, lastTs: nil)
    let text = DiagnosticsExport.headerText(aHeader().withFiles([fact]))
    #expect(text.contains("覆盖范围: 未知"), "\(text)")
}

/// **连不上内核时也要能生成**（协议版本那一格如实写"未连上"，不是失败）。
@Test func aMissingProtocolVersionIsStatedNotInvented() {
    let text = DiagnosticsExport.headerText(
        DiagnosticsExport.Header(appVersion: "0.2.0",
                                 coreSha256: aHeader().coreSha256,
                                 protocolVersion: nil,
                                 logLevel: "normal",
                                 os: "macOS Version 15.6.0 (Build 24G84)",
                                 arch: "arm64",
                                 exportedAt: "2026-10-06 14:03:11 UTC",
                                 files: []))
    #expect(text.contains("protocol_version: 未连上"), "\(text)")
}

/// ⚠️ **没产生的那几代要逐个点名**（"这份日志为什么只有这么几行"的一半答案）。
@Test func aGenerationThatWasNeverWrittenIsNamed() {
    let text = DiagnosticsExport.headerText(aHeader())   // 只有主文件那一份
    for missing in ["diag-kernel.log.2", "diag-shell.log", "diag-shell.log.1"] {
        #expect(text.contains(missing),
                "没产生的那一份也要在说明文件里出现（写「尚未产生」）：少了 \(missing)\n\(text)")
    }
}

// ---------------------------------------------------------------------------
// 要拷哪些
// ---------------------------------------------------------------------------

/// **只拷存在的那些**（详细日志刚打开时后几代还没生成）。**不存在不算失败。**
@Test func onlyTheFilesThatExistAreListed() {
    let got = DiagnosticsExport.filesToCopy(existing: ["diag-kernel.log", "diag-shell.log.2"])
    #expect(got == ["diag-kernel.log", "diag-shell.log.2"], "实际：\(got)")
    #expect(DiagnosticsExport.filesToCopy(existing: []).isEmpty)
    // 目录里多出来的东西（比如 preferences.json）**一个都不许进清单** —— 见
    // `theWhitelistKeepsPreferencesOutOfTheExport`。
    let strangers = DiagnosticsExport.filesToCopy(
        existing: ["preferences.json", "history.json", "diag-shell.log.99", "diag-shell.log"])
    #expect(strangers == ["diag-shell.log"], "只认清单上那六个名字：\(strangers)")
}

/// 🔴 **顺序是我们定的，不是输入给的**（两次导出的说明文件要能直接 diff）。
///
/// 判别力：把 `filesToCopy` 写成"按传入顺序过滤" ⇒ 这一条红 —— 而真机上那是
/// "同一台机器导两次，说明文件的行序不一样"，而人要靠 diff 找两次现场的区别。
@Test func theOrderIsOursNotTheCallers() {
    let scrambled = ["diag-shell.log.2", "diag-kernel.log.1",
                     "diag-shell.log", "diag-kernel.log"]
    let got = DiagnosticsExport.filesToCopy(existing: scrambled)
    #expect(got == ["diag-kernel.log", "diag-kernel.log.1",
                    "diag-shell.log", "diag-shell.log.2"],
            "内核在前、代次由新到旧：\(got)")
}

/// ⚠️ **壳那份的名字**必须来自 `DiagnosticsLog.fileName`（不许在这里抄第二遍）。
///
/// 判别力：把 `diag-shell.log` 在 `allLogNames` 里写死成字面量 ⇒ 改
/// `DiagnosticsLog.fileName` 时导出会**静默地什么也不拷**（文件名对不上），
/// 而说明文件里会写着"尚未产生" —— 一句看起来完全正常的假话。
@Test func theShellLogNameComesFromTheLogger() {
    #expect(DiagnosticsExport.allLogNames().contains(DiagnosticsLog.fileName),
            "壳那份日志的名字必须来自 `DiagnosticsLog.fileName`")
    // 两代轮转都在（详细档留两代：`Level.verbose.generations`）。
    #expect(DiagnosticsExport.allLogNames().contains("\(DiagnosticsLog.fileName).2"))
    #expect(DiagnosticsExport.allLogNames().count == 6, "内核三份 + 壳三份")
    #expect(DiagnosticsExport.allLogNames().first == "diag-kernel.log", "内核在前")
}

// ---------------------------------------------------------------------------
// 历法（本 task 唯一能抓住"算错日子"的判据）
// ---------------------------------------------------------------------------

/// 🔴 **历法的已知答案向量**。
///
/// ⚠️ **这几条向量是算出来的，不是编的**（`python3 -c "import datetime; print(
///    datetime.datetime.fromtimestamp(TS, datetime.timezone.utc))"` 逐条核过；
///    另外拿 300 000 个点与 Python 做过一次差分，读数见本 task 的报告）。
///    尤其是**闰日那一条**：一个"每 4 年加一天"写漏（或写成"能被 100 整除就不闰"
///    而漏掉"能被 400 整除还是闰"）的实现，会在 2024-02-29 之后**整年错一天**，
///    而目录名与日志内容对不上**不会让任何别的东西变红**。
@Test func theCalendarMatchesKnownAnswers() {
    let cases: [(UInt64, String)] = [
        (1_767_225_600, "诊断日志-20260101-000000"),   // 跨年、全零
        (1_709_251_199, "诊断日志-20240229-235959"),   // **闰日**、且是当天的最后一秒
        (1_759_742_591, "诊断日志-20251006-092311"),   // 普通的某一天
        (4_107_542_400, "诊断日志-21000301-000000"),   // **百年不闰**：2100 不是闰年
        (951_782_400, "诊断日志-20000229-000000"),     // **四百年又闰**：2000 是闰年
    ]
    for (ts, want) in cases {
        let got = DiagnosticsExport.subdirectoryName(at: Date(timeIntervalSince1970: TimeInterval(ts)))
        #expect(got == want, "unix \(ts) 的日子算错了：实际 \(got)")
    }
}

/// 🔴 **时间戳子目录**（多次导出不互相覆盖，也不会把两次现场混在一起）。
///
/// 判别力：把它写成固定名字 ⇒ 红；真机上的表现是客户导第二次时
/// **把第一次的现场覆盖掉了**，而两次的时间点不同、恰好是最需要对比的时候。
@Test func theSubdirectoryCarriesATimestampAndIsNotAFixedName() {
    let a = DiagnosticsExport.subdirectoryName(at: Date(timeIntervalSince1970: 1_759_742_591))
    let b = DiagnosticsExport.subdirectoryName(at: Date(timeIntervalSince1970: 1_759_742_592))
    #expect(a != b, "不同时刻必须是不同的目录名")
    #expect(a.hasPrefix("诊断日志-"), "实际：\(a)")
    #expect(a.count == "诊断日志-".count + 15, "YYYYMMDD-HHMMSS：\(a)")
}

/// 导出时刻那一格用的是**同一份历法**（同一天里导两次，两个时间戳要一眼对得上）。
@Test func theExportMomentUsesTheSameCalendar() {
    let at = Date(timeIntervalSince1970: 1_759_742_591)
    #expect(DiagnosticsExport.utcStamp(at: at) == "2025-10-06 09:23:11 UTC")
    #expect(DiagnosticsExport.subdirectoryName(at: at).hasSuffix("20251006-092311"),
            "两处各算一遍历法就会漂")
}

/// 日历那一格用的是 **UTC**，而且**明写了 UTC**（不假装是本机时间）。
///
/// 判别力：把 `utcStamp` 的 `UTC` 后缀去掉，第二条红 —— 而真机上那是看日志的人
/// **按本机时区去对那一刻**，对不上几个小时，而没有任何东西会变红
/// （日志里那个 `ts=` 本来就是 UTC 的 unix 秒）。
@Test func theExportMomentSaysWhichClockItIs() {
    let text = DiagnosticsExport.utcStamp(at: Date(timeIntervalSince1970: 0))
    #expect(text == "1970-01-01 00:00:00 UTC", "实际：\(text)")
    #expect(text.hasSuffix("UTC"), "要说清这是哪一个钟：\(text)")
}

// ---------------------------------------------------------------------------
// 真的去拷（临时目录上跑整条链路）
// ---------------------------------------------------------------------------

/// 🔴 **成功那一条：日志真的进去了，而说明文件写的是它们的**量出来的**事实**。
///
/// 判别力（三条各挡一种写法）：
///   · 只建目录不拷文件 ⇒ 第一条红；
///   · 说明文件里的 sha256 / 字节数**另算一遍**（而不是量落盘那一份）⇒ 第二条红；
///   · 覆盖范围不读文件 ⇒ 第三条红（那两个时刻**只可能**来自文件内容）。
@Test func theExportFolderHoldsTheLogsAndAMeasuredHeader() throws {
    let logs = try TempDir("logs")
    let target = try TempDir("target")
    defer { logs.cleanUp(); target.cleanUp() }

    // 一份**真的**日志（两行，形状与那三份日志器写的逐字相同）。
    let body = "ts=1759742591 event=kernel_call method=ping ok=true\n"
        + "ts=1759746000 event=kernel_call method=getGlobalStat ok=true\n"
    try Data(body.utf8).write(to: logs.url.appendingPathComponent(DiagnosticsLog.fileName))
    let wantSha = DiagnosticsExport.sha256Hex(Data(body.utf8))

    let environment = DiagnosticsExport.header(appVersion: "0.2.0", coreBinary: nil,
                                               protocolVersion: 1, verboseLogging: true,
                                               os: "macOS Version 15.6.0 (Build 24G84)",
                                               arch: "arm64",
                                               at: Date(timeIntervalSince1970: 1_759_742_591))
    let folder = try DiagnosticsExport.export(from: logs.url, to: target.url,
                                              header: environment,
                                              now: Date(timeIntervalSince1970: 1_759_742_591))

    let copied = folder.appendingPathComponent(DiagnosticsLog.fileName)
    #expect((try? String(contentsOf: copied, encoding: .utf8)) == body,
            "拷进去的字节必须与源文件一样")
    let text = try String(contentsOf: folder.appendingPathComponent("说明.txt"), encoding: .utf8)
    #expect(text.contains(wantSha), "sha256 必须是**落盘那一份**的：\n\(text)")
    #expect(text.contains("\(body.utf8.count)"), "字节数要在：\n\(text)")
    #expect(text.contains("1759742591") && text.contains("1759746000"),
            "覆盖范围必须来自文件内容（头尾两行的 ts=）：\n\(text)")
    // 导出到的是**目标文件夹里一个带时间戳的子目录**（不是把文件摊在目标里）。
    #expect(folder.path.hasPrefix(target.url.path))
    #expect(folder.lastPathComponent == "诊断日志-20251006-092311", "实际：\(folder.lastPathComponent)")
    // 🔴 **同一个时刻喂出来的两格必须说同一件事**（2026-10-06 终审 ④b）：文件夹名与
    //    说明文件里那格 `exported_at` 都来自传进来的 `now` —— 两条断言用的是**同一个**
    //    `Date(timeIntervalSince1970: 1_759_742_591)`。判别力：让 `export` 内部
    //    自己读一次时钟（`Date()`），目录名那一条立刻红；而真机上那是"同一个导出里
    //    两个时刻差一秒"，看起来完全正常。
    let moment = Date(timeIntervalSince1970: 1_759_742_591)
    #expect(folder.lastPathComponent == DiagnosticsExport.subdirectoryName(at: moment),
            "文件夹名必须由传进来的 `now` 决定：\(folder.lastPathComponent)")
    #expect(text.contains("exported_at: " + DiagnosticsExport.utcStamp(at: moment)),
            "说明文件那一格必须与文件夹名同源（同一个 `now`）：\n\(text)")
    // 目录里**只有**清单上那几个 + 说明文件（`preferences.json` 那类一个都不许进来）。
    let inside = try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
    #expect(inside == ["diag-shell.log", "说明.txt"], "实际：\(inside)")
}

/// 🔴 **日志文件不存在时导出照常成功**（详细日志刚打开、后几代还没生成）。
///
/// 判别力：把"没有日志"判成失败 ⇒ 红 —— 而真机上那是**刚装好就导出**这一最常见的
/// 情形里，用户看到一句"导出失败"，而他要的东西（说明文件）本来是给得出来的。
@Test func missingLogFilesAreNotAFailure() throws {
    let logs = try TempDir("no-logs")
    let target = try TempDir("no-logs-target")
    defer { logs.cleanUp(); target.cleanUp() }

    let header = DiagnosticsExport.header(appVersion: "0.2.0", coreBinary: nil,
                                          protocolVersion: nil, verboseLogging: false,
                                          os: DiagnosticsExport.osVersion(), arch: "arm64",
                                          at: Date())
    let folder = try DiagnosticsExport.export(from: logs.url, to: target.url, header: header,
                                              now: Date(timeIntervalSince1970: 1_759_742_591))
    let text = try String(contentsOf: folder.appendingPathComponent("说明.txt"), encoding: .utf8)
    #expect(text.contains("尚未产生"), "要如实写「为什么一份都没有」：\n\(text)")
    #expect(text.contains("没有任何日志文件"), "\(text)")
    // ⚠️ 没有内核可哈希 ⇒ 那一格如实写"取不到"，**不编一个哈希**（模块头第 1 条那处不对称）。
    #expect(text.contains(DiagnosticsExport.missingCoreText), "\(text)")
}

/// 🔴 **目标目录不可写 ⇒ 必须回一句人话，不许静默成功**（规格 §4）。
///
/// 判别力（**突变实测**，读数见本 task 的报告）：把 `export` 里三处失败出口
/// 全部 `try?` 掉 ⇒ 第一条断言红（`#expect(throws:)` 直接不成立）—— 而真机上的表现是
/// **界面上说"导出成功"、而客户发回来的文件夹是空的**（那比报错坏得多）。
/// 只吞掉**第一处**（建目录那一处）⇒ **也红**：错误会从后面某一步冒出来，
/// 而那句话说的是**另一个步骤**，与"建不了文件夹"这个结论对不上。
/// ⇒ 断言必须收到**这一档专属的那句话**上（不是 `!message.isEmpty`）。
///
/// 造法（确定性）：目标是一个**已存在的普通文件** ⇒ 在它底下建子目录必然失败。
/// ⚠️ 用的是 `export(from:to:…)`（日志目录是参数），不是就地取 `DiagnosticsLog.logURL`
///    —— 判据绝不许碰人类伙伴真实的那个目录。
@Test func anUnwritableTargetIsReportedInsteadOfSwallowed() throws {
    let logs = try TempDir("unwritable-logs")
    let holder = try TempDir("unwritable-target")
    defer { logs.cleanUp(); holder.cleanUp() }
    let blocker = holder.url.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: blocker)

    let header = DiagnosticsExport.header(appVersion: "0.2.0", coreBinary: nil,
                                          protocolVersion: nil, verboseLogging: false,
                                          os: DiagnosticsExport.osVersion(), arch: "arm64",
                                          at: Date())
    var thrown: DiagnosticsExport.ExportFailure?
    do {
        _ = try DiagnosticsExport.export(from: logs.url, to: blocker, header: header,
                                         now: Date(timeIntervalSince1970: 1_759_742_591))
    } catch let failure as DiagnosticsExport.ExportFailure {
        thrown = failure
    }
    let failure = try #require(thrown, "目标不是一个能建子目录的位置 ⇒ 必须**抛**，不许静默成功")
    #expect(failure.message.contains("建不了文件夹"),
            "要说清是**哪一步**、哪个位置（这一档是建目录）：\(failure.message)")
    #expect(failure.message.contains("补救"), "要给出下一步能做的事：\(failure.message)")
}

/// ⚠️ **写到一半的最后一行不算一条记录**（覆盖范围退回上一条**完整**的）。
///
/// 判别力：把 `tsOf` 里那句"只认完整行"删掉 ⇒ 这一条红 —— 而真机上那是
/// 一个**被截断的数字**当作时刻印进说明文件（`1759746000` 被截成 `1759746`），
/// 它看起来完全正常，只是把"这份日志覆盖到哪一刻"说错了。
@Test func aTornLastLineDoesNotBecomeTheCoverageEnd() {
    let body = Data("ts=1759742591 event=kernel_call ok=true\nts=1759746".utf8)
    let (first, last) = DiagnosticsExport.firstAndLastTs(body)
    #expect(first == 1_759_742_591, "头一行就是那一条完整的记录：\(String(describing: first))")
    #expect(last == 1_759_742_591,
            "半行不许被当成一条记录 —— 要退回上一条**完整**的：\(String(describing: last))")
}

/// 空日志文件（刚建出来、一行还没写）⇒ 覆盖范围两格都是 `None`（说明文件写"未知"）。
@Test func anEmptyLogFileHasNoRange() {
    let (first, last) = DiagnosticsExport.firstAndLastTs(Data())
    #expect(first == nil && last == nil, "空文件没有范围")
}

/// 🔴 **隐私：`preferences.json`（里面有下载目录）**绝不许**被卷进导出。**
///
/// ⚠️ 本模块的写法与 Windows 那一侧**同一条**：`filesToCopy` 拿**固定的六个名字**去与
/// "目录里现有的那些"**求交** —— 于是"这个目录里还有什么别的东西"**根本不进**这个函数
/// 的决策（那份偏好文件里是客户自己的路径，而导出会离开他的机器）。
/// 判别力：把 `filesToCopy` 改成"现有的全都拷" ⇒ 这一条红。
@Test func theWhitelistKeepsPreferencesOutOfTheExport() throws {
    let logs = try TempDir("privacy")
    let target = try TempDir("privacy-target")
    defer { logs.cleanUp(); target.cleanUp() }
    // 真机形状：日志与偏好**同目录**（`ShellStorage.directory`）。
    try Data(#"{"version":1,"download_dir":"/Volumes/客户自己的盘/交付"}"#.utf8)
        .write(to: logs.url.appendingPathComponent(AppPreferencesStore.fileName))
    try Data("ts=1 event=x ok=true\n".utf8)
        .write(to: logs.url.appendingPathComponent(DiagnosticsLog.fileName))

    let header = DiagnosticsExport.header(appVersion: "0.2.0", coreBinary: nil,
                                          protocolVersion: nil, verboseLogging: true,
                                          os: DiagnosticsExport.osVersion(), arch: "arm64",
                                          at: Date())
    let folder = try DiagnosticsExport.export(from: logs.url, to: target.url, header: header,
                                              now: Date(timeIntervalSince1970: 1_759_742_591))

    let inside = try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
    #expect(inside == [DiagnosticsLog.fileName, "说明.txt"], "实际：\(inside)")
    let dumped = inside.joined(separator: "\n")
        + (try String(contentsOf: folder.appendingPathComponent("说明.txt"), encoding: .utf8))
    #expect(!dumped.contains("客户自己的盘"),
            "偏好文件（里面有下载目录）一个字都不许进导出：\n\(dumped)")
}

// ---------------------------------------------------------------------------
// 「系统与版本」「内核身份」那两格
// ---------------------------------------------------------------------------

/// 🔴 **「系统与版本」那一格拼得出真东西、而且不编**。
///
/// 判别力：把 `osField` 改成只回 `"macOS"`（不带版本号）⇒ 第一条红 ——
/// 而真机上的表现是说明文件里那一格**回答不了它存在的问题**
/// （规格 §2.6：这一格的理由是"不同系统版本上是不同的问题"）。
/// 注意：**"macOS（版本取不到）"也不算通过** —— 那是取不到时的话，不是常态下的那一格。
@Test func theOsFieldCarriesARealVersionNumber() {
    #expect(DiagnosticsExport.osField(versionString: "Version 15.6.0 (Build 24G84)")
            == "macOS Version 15.6.0 (Build 24G84)")
    // 三个数（主/次/build）都在，而且**不翻译**成市场名字（"Sonoma"那种今天的映射对不上）。
    let real = DiagnosticsExport.osVersion()
    #expect(!real.isEmpty, "这一格不许是空串（空行在说明文件里等于没有这一格）")
    #expect(real != "macOS", "只回一个平台名等于把这一格废掉")
    #expect(real.rangeOfCharacter(from: .decimalDigits) != nil, "版本号里必须有数字：\(real)")
    #expect(real != DiagnosticsExport.osField(versionString: nil),
            "常态下那一格与「取不到」那一档不是同一句话：\(real)")
    // 取不到时不编：如实说，而且同样不退化成平台名。
    #expect(DiagnosticsExport.osField(versionString: "  ") == "macOS（版本取不到）")
}

/// 🔴 **内核身份那一格是"导出时现算的文件哈希"，不是一个编译期常量**（模块头第 1 条）。
///
/// 判别力：把 `coreSha256(ofBinaryAt:)` 换成回一个常量 ⇒ 第一条红。
/// ⚠️ 期望值用的是 **NIST 的已知答案向量**（`sha256("abc")`），不是"用同一段代码再算一遍"
/// —— 后者是循环论证，算错了照样绿。
@Test func theCoreHashIsComputedFromTheFileAndIsNeverInvented() throws {
    #expect(DiagnosticsExport.sha256Hex(Data("abc".utf8))
            == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-256 的已知答案向量对不上")
    // 空心输入也有已知答案（空串的 sha256）—— 免得"读不到就回空哈希"这种写法混过去。
    #expect(DiagnosticsExport.sha256Hex(Data())
            == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")

    let dir = try TempDir("core")
    defer { dir.cleanUp() }
    let binary = dir.url.appendingPathComponent("benagen-core")
    try Data("abc".utf8).write(to: binary)
    #expect(DiagnosticsExport.coreSha256(ofBinaryAt: binary).hasPrefix("ba7816bf"),
            "这一格必须是**那个文件**的哈希")

    // 拿不到 ⇒ 如实写"取不到"，**不编一个哈希**（`nil` 与"路径不存在"是同一档）。
    #expect(DiagnosticsExport.coreSha256(ofBinaryAt: nil) == DiagnosticsExport.missingCoreText)
    let absent = dir.url.appendingPathComponent("没有这个文件")
    #expect(DiagnosticsExport.coreSha256(ofBinaryAt: absent) == DiagnosticsExport.missingCoreText)
    #expect(!DiagnosticsExport.missingCoreText.contains("ba78"),
            "取不到那一档不许长得像一个哈希")
}

/// 生产那一侧的取值点（`CoreClient.locateCoreBinary()`）**真的能哈希到东西**。
///
/// ⚠️ 仓库里的内核是构建产物，别的机器上可能没有 ⇒ 这条**条件跳过**
///    （与 `CoreClientRealCoreTests` 同一条口径）。
@Test(.enabled(if: CoreClient.devCoreBinaryExists()))
func theShippedCoreBinaryIsHashable() {
    let hash = DiagnosticsExport.coreSha256(ofBinaryAt: CoreClient.repoCoreBinaryURL())
    #expect(hash.count == 64, "sha256 是 64 位十六进制：\(hash)")
    #expect(hash != DiagnosticsExport.missingCoreText, "内核就在那儿，这一格不该是「取不到」")
    #expect(hash.allSatisfy { $0.isHexDigit }, "只许十六进制：\(hash)")
}

/// 「架构」那一格：与 Windows 一样是**编译期**的事实（`std::env::consts::ARCH` 的同位物），
/// 不许是空串（空行在说明文件里等于没有这一格）。
@Test func theArchFieldIsAlwaysSpelled() {
    let arch = DiagnosticsExport.arch
    #expect(!arch.isEmpty, "这一格不许是空串")
    #expect(["arm64", "x86_64", "unknown"].contains(arch), "实际：\(arch)")
}

// ---------------------------------------------------------------------------
// 环境事实 ⇒ 那七格（`header(…)` 那个纯装配点）
// ---------------------------------------------------------------------------

/// 🔴 **`log_level` 那一格跟着偏好走，而且取值与内核 `--log-level` 逐字同两个。**
///
/// 判别力：把 `header(…)` 里的三元写成恒 `"normal"` ⇒ 第二条红 —— 而真机上的表现是
/// **客户开着详细档导出的那份说明文件里写着 normal**，看日志的人据此刻度错"这份日志
/// 为什么这么少"的结论（那一格是"灵魂"的理由正在这里）。
@Test func theHeaderTakesTheLevelAndTheKernelIdentityFromTheCallersFacts() {
    let verbose = DiagnosticsExport.header(appVersion: "9.9.9", coreBinary: nil,
                                           protocolVersion: 3, verboseLogging: true,
                                           os: "macOS Version 15.6.0 (Build 24G84)",
                                           arch: "arm64",
                                           at: Date(timeIntervalSince1970: 1_759_742_591))
    #expect(verbose.logLevel == "verbose")
    #expect(verbose.appVersion == "9.9.9")
    #expect(verbose.protocolVersion == 3)
    #expect(verbose.exportedAt == "2025-10-06 09:23:11 UTC")
    #expect(verbose.files.isEmpty, "文件那几格由 `export` 量完再填（这里不编）")

    let normal = DiagnosticsExport.header(appVersion: "9.9.9", coreBinary: nil,
                                          protocolVersion: nil, verboseLogging: false,
                                          os: "macOS Version 15.6.0 (Build 24G84)",
                                          arch: "arm64", at: Date())
    #expect(normal.logLevel == "normal")
    #expect(normal.protocolVersion == nil, "连不上就是 nil —— **不许**回落成壳自己的常量")
}

// ---------------------------------------------------------------------------
// 回执（规格 §2.5 / §2.7 的第二处明说）
// ---------------------------------------------------------------------------

/// 🔴 **成功那一档必须带上那句提醒**（规格 §2.7 的第 2 处明说）。
///
/// 判别力：把 `privacyNote` 从 `.done` 上摘掉 ⇒ 第一条红 —— 而真机上那是
/// **用户拿着一个包含下载地址片段的文件夹、却没有任何人告诉过他**
/// （那正是规格 §2.7 拒绝做自动过滤之后，唯一还剩下的那道防线）。
@Test func aSuccessfulExportCarriesThePrivacyNote() {
    let done = DiagnosticsExport.Outcome.done(folder: "/Users/x/桌面/诊断日志-20260101-000000")
    #expect(done.privacyNote == DiagnosticsExport.privacyNote)
    #expect(DiagnosticsExport.privacyNote.contains("下载地址片段"),
            "那句话要把风险说清：\(DiagnosticsExport.privacyNote)")
    #expect(DiagnosticsExport.privacyNote.contains("自己打开看一眼"),
            "要给出用户能做的事：\(DiagnosticsExport.privacyNote)")
    // ⚠️ 标题里**不许有路径**（布局事故的修法）——路径单独一格。
    #expect(done.headline == "诊断日志已导出")
    #expect(!done.headline.contains("/"), "标题里混进了路径：\(done.headline)")
    #expect(done.pathDetail == "/Users/x/桌面/诊断日志-20260101-000000")
    #expect(!done.isFailure)
}

/// ⚠️ **取消不是失败**，而且要说得出话。
///
/// 判别力：把取消做成失败 ⇒ 界面会为一次"用户自己按了取消"亮一条红的；
/// 而把回执整个去掉 ⇒ 用户点了「导出」之后界面一动不动（与"卡住了"分不开）。
@Test func cancellingIsNotAFailureButItStillSaysSomething() {
    let cancelled = DiagnosticsExport.Outcome.cancelled
    #expect(!cancelled.isFailure, "取消不是失败")
    #expect(cancelled.pathDetail == nil, "没有文件夹可说")
    #expect(cancelled.privacyNote == nil, "没导出东西，没有什么可提醒的")
    #expect(cancelled.headline.contains("没有导出"),
            "要说清「什么都没发生」，否则与卡住分不开：\(cancelled.headline)")
}

/// **失败那一档照登原文**（不加工、不换一句更客气的话），并且 `isFailure` 翻面。
@Test func aFailedExportReportsTheReasonVerbatim() {
    let why = DiagnosticsExport.folderFailureText(folder: "/只读/诊断日志-20260101-000000",
                                                  cause: "You don’t have permission")
    let failed = DiagnosticsExport.Outcome.failed(message: why)
    #expect(failed.headline == why, "原文逐字，不加工")
    #expect(failed.isFailure, "失败要翻面（图标与颜色靠它）")
    #expect(failed.privacyNote == nil)
    #expect(failed.pathDetail == nil)
}

// ---------------------------------------------------------------------------
// 三档失败话术（规格 §4：一句人话 —— 哪个位置、为什么、下一步）
// ---------------------------------------------------------------------------

/// 建目录那一档失败的话**点名了文件夹与根因**。
@Test func anUnwritableTargetNamesTheFolderAndTheCause() {
    let text = DiagnosticsExport.folderFailureText(folder: "/Volumes/只读/诊断日志-20260101-000000",
                                                   cause: "You don’t have permission")
    #expect(text.contains("诊断日志-20260101-000000"), "要点名是哪个文件夹：\(text)")
    #expect(text.contains("You don’t have permission"), "系统原话必须照登：\(text)")
    #expect(text.contains("补救"), "要给出下一步能做的事：\(text)")
    #expect(text.contains("没有导出"), "要说清这件事的结论：\(text)")
}

/// 🔴 **每一档失败说的都是它真正失败的那一步**（不是三处共用一句）。
///
/// 判别力（**突变实测**，读数见本 task 的报告）：让 `copyFailureText` /
/// `headerFailureText` 改去调 `folderFailureText`（三档合成一句）⇒ 第 2 / 3 条断言红
/// —— 而真机上的表现是**用户拿着"建不了文件夹 …/diag-shell.log"去查一个建得出来的位置**
/// （他真正该看的是那一份文件）。Windows 那一侧的修复轮 1 抓到的就是这个。
@Test func eachFailureNamesTheStepThatActuallyFailed() {
    let folder = "/Volumes/导出/诊断日志-20260101-000000"
    let log = "\(folder)/diag-shell.log"
    let headerFile = "\(folder)/说明.txt"
    let cause = "You don’t have permission"

    let mkdirFailed = DiagnosticsExport.folderFailureText(folder: folder, cause: cause)
    let copyFailed = DiagnosticsExport.copyFailureText(file: log, cause: cause)
    let headerFailed = DiagnosticsExport.headerFailureText(file: headerFile, cause: "磁盘空间不足")

    // ① 建目录那一档：点名那个**文件夹**（而且不该提到还没拷的文件）。
    #expect(mkdirFailed.contains("建不了文件夹"), "\(mkdirFailed)")
    #expect(mkdirFailed.contains(folder), "\(mkdirFailed)")
    #expect(!mkdirFailed.contains("diag-shell.log"), "这一档还没走到拷文件：\(mkdirFailed)")
    // ② 拷一份日志那一档：点名**那个文件**，而且**不**冒充"文件夹建不了"。
    #expect(copyFailed.contains(log), "要点名是哪一份日志：\(copyFailed)")
    #expect(!copyFailed.contains("建不了文件夹"),
            "拷文件失败与建文件夹是两件事（说错了用户会去查错的位置）：\(copyFailed)")
    #expect(copyFailed.contains(cause), "系统原话必须照登：\(copyFailed)")
    // ③ 写说明文件那一档：点名那个文件，同样不冒充建目录。
    #expect(headerFailed.contains(headerFile), "\(headerFailed)")
    #expect(!headerFailed.contains("建不了文件夹"), "\(headerFailed)")
    // 三句必须**真的不一样**（合成一句就会在其中一个场合说错话）。
    #expect(mkdirFailed != copyFailed)
    #expect(copyFailed != headerFailed)
    for text in [mkdirFailed, copyFailed, headerFailed] {
        #expect(text.contains("补救"), "每一档都要给出下一步能做的事：\(text)")
    }
}
