import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 壳侧的有界诊断日志（规格 §2.3 的 macOS 那一半，任务 5）
//
// ⚠️ 本文件与另外**两个**日志器的用例**逐条对应**：
//     · `core/src/diagnostics.rs`（内核，写 `diag-kernel.log`）；
//     · `windows/shell-core/src/diagnostics.rs`（Windows 壳，写 `diag-shell.log`）。
//    三份的"界、轮转兜底、行形状、档位取值"必须是同几条判据 ——
//    客户会把几个文件**一起**发回来，读法必须是同一种。
//
// ⚠️⚠️ **安全红线**：本文件里的每一次落盘都指向 `temporaryDirectory` 下的一个
//     一次性目录。**没有任何一条**用例碰 `DiagnosticsLog.logURL` ——
//     那是 `~/Library/Application Support/BenagenDownloader/diag-shell.log`，
//     人类伙伴**真实在用**的那一份，往里写 = 破坏他的现场。
//     （`logURL` 本身有一条只读的判据，见最后那一条用 tests。）
// ---------------------------------------------------------------------------

/// 一个用完就删的临时日志目录。
private struct TempLogDir {
    let dir: URL

    init() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-diag-swift-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    /// 日志本体（与 `DiagnosticsLog.write` 收到的路径同形：同目录、同文件名）。
    var log: URL { dir.appendingPathComponent(DiagnosticsLog.fileName) }

    /// 第 `n` 代（`.1` / `.2`）。
    func generation(_ n: Int) -> URL { URL(fileURLWithPath: log.path + ".\(n)") }

    func size(_ url: URL) -> UInt64 {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              let size = attrs[.size] as? NSNumber else { return 0 }
        return size.uint64Value
    }

    func cleanUp() { try? FileManager.default.removeItem(at: dir) }
}

/// 本测试里每条记录的字节数（`padded` 恰好 180 个 ASCII 字符 = 180 字节，
/// `write` **不补**换行）。
private let lineBytes: UInt64 = 180

/// Rust 那两份测试里 `format!("{i:0>180}")` 的 Swift 版。
private func padded(_ i: Int) -> String {
    let s = String(i)
    return String(repeating: "0", count: max(0, 180 - s.count)) + s
}

/// 取 `formatLine` 那一行的**字段**（按空格切；值里的空格必须已经被换成 `_`）。
private func parts(_ line: String) -> [String] {
    line.split(separator: " ").map(String.init)
}

/// 取某一格 `k=v` 的 `v`（按**第一个** `=` 切，值里可以再有 `=`）。
private func value(of key: String, in line: String) -> String? {
    for part in parts(line) where part.hasPrefix("\(key)=") {
        return String(part.dropFirst(key.count + 1))
    }
    return nil
}

// ---------------------------------------------------------------------------
// 行形状：与 Rust 那两份逐字同形
// ---------------------------------------------------------------------------

/// 🔴 **行格式与 Rust 那两份逐字同形** —— 客户回传时三个文件要能对着读。
///
/// ⚠️ 最后一个字段是**故意带空格的**：把 `formatLine` 里那段
///    `isWhitespace → "_"` 整段删掉，本测试必红 —— 那正是"这一行能被按空格切字段读"
///    的全部依靠。前两个字段不含空格，光靠它们**守不住**这条性质。
@Test func aLineHasTheSameShapeAsTheRustLoggers() {
    let line = DiagnosticsLog.formatLine("kernel_call", [
        ("method", "get_state"),
        ("ms", "12"),
        ("ok", "true"),
        ("why", "action: 探活失败"),
    ])
    let p = parts(line)

    #expect(p.count == 6, "带空格的值被切成了多格（应是 6 格：ts/event/四格字段）：\(line)")
    #expect(p[0].hasPrefix("ts="), "第一格必须是 ts=：\(line)")
    #expect(p[1] == "event=kernel_call")
    #expect(p[2] == "method=get_state")
    #expect(p[3] == "ms=12")
    #expect(p[4] == "ok=true")
    #expect(p[5] == "why=action:_探活失败",
            "值里的空格必须换成 `_`，否则一个带空格的值会把字段切开：\(line)")
    #expect(value(of: "why", in: line) == "action:_探活失败")
}

/// `ts=` 那一格是**UTC 的 unix 秒**（规格 §2.6 的订正：日志与导出时刻同一条时间轴）。
@Test func theTimestampIsUnixSeconds() {
    let line = DiagnosticsLog.formatLine("kernel_call", [], now: Date(timeIntervalSince1970: 1_759_742_591))
    #expect(line == "ts=1759742591 event=kernel_call", "实际：\(line)")
}

// ---------------------------------------------------------------------------
// 两档的界
// ---------------------------------------------------------------------------

/// 🔴 **normal 档一个字都不许变**（规格 §2.2）：上限还是 1 MiB、还是只留一代。
///
/// 判别力：把 `Level.normal.maxBytes` 写成 verbose 那个数，这一条立刻红 ——
/// 而真机上的表现是"客户机器上那份普通日志突然能占 12 MiB"，**没有任何别的东西会红**。
@Test func theNormalLevelKeepsTheOneMiBBound() {
    #expect(DiagnosticsLog.level == .normal, "缺省必须是普通档（进程刚起来，没人 configure 过）")
    #expect(DiagnosticsLog.Level.normal.maxBytes == DiagnosticsLog.maxBytes)
    #expect(DiagnosticsLog.Level.normal.maxBytes == 1_048_576)
    #expect(DiagnosticsLog.Level.normal.generations == 1)
    #expect(DiagnosticsLog.Level.verbose.maxBytes == 4 * 1_048_576)
    #expect(DiagnosticsLog.Level.verbose.generations == 2)
}

/// **有界**：写到超过 1 MiB 必须轮转，且总占用有上限（与 Rust 那两份同一条）。
///
/// ⚠️ 断言写的是**代码真正保证的**上界，不是"1 MiB"这个整数：轮转在写入**之前**
///    检查，所以单文件可以到 `maxBytes + 一行`。写成 `<= maxBytes` / `<= 2 × maxBytes`
///    会在**本测试这个循环次数上直接假红**：主文件到这里正是 `maxBytes + 104` 字节。
///
/// 循环次数取 **11652** 与 Rust 那两份**逐字相同**（算过的，不是凑的）：每条 180 字节
/// ⇒ 每 5826 次写把主文件从 0 顶到 `> maxBytes`，于是第 5827 次写发生**第一次**轮转、
/// 第 11653 次发生第二次。11652 落在第二次轮转**前一次** ⇒ 恰好把主文件顶到其最大值
/// `maxBytes + 104`，即本测试覆盖的是**最坏那一格**。
@Test func theLogIsBoundedAndRotates() throws {
    let temp = try TempLogDir()
    defer { temp.cleanUp() }

    for i in 0..<11652 {
        DiagnosticsLog.write(padded(i), to: temp.log, level: .normal)
    }

    let main = temp.size(temp.log)
    let rotated = temp.size(temp.generation(1))
    #expect(main <= DiagnosticsLog.maxBytes + lineBytes,
            "主文件 \(main) 字节，超过上限 \(DiagnosticsLog.maxBytes) + 一行 \(lineBytes)")
    #expect(main + rotated <= DiagnosticsLog.maxBytes * 2 + 2 * lineBytes,
            "轮转后总占用 \(main + rotated) 字节，超过两倍上限加两行")
    #expect(rotated > 0, "从没轮转过 —— 上限没生效")
}

/// 🔴 **详细档仍然是有界的**（规格 §2.2 那张表的最后一行）。
///
/// 它记的是"每一次往返"，所以它不是"慢一点"而是"快十几倍" —— 没有上界的话
/// 客户开一晚上就是一个几 GB 的文件。断言写的是**算得出来的**上界：
/// 单文件 `maxBytes + 一行`，总占用 `三代 + 三行`。
@Test func theVerboseLevelKeepsTwoGenerationsAndIsStillBounded() throws {
    let temp = try TempLogDir()
    defer { temp.cleanUp() }

    // 4 MiB 上限、每行 180 字节 ⇒ 约 23302 行顶过一次上限。写 3 倍于"三代写满"的量
    // ⇒ 三代都必须存在，而总量仍在界内。
    for i in 0..<70_000 {
        DiagnosticsLog.write(padded(i), to: temp.log, level: .verbose)
    }

    let main = temp.size(temp.log)
    let g1 = temp.size(temp.generation(1))
    let g2 = temp.size(temp.generation(2))
    #expect(g2 > 0, "详细档要留两代，`.2` 从没出现过")
    #expect(!FileManager.default.fileExists(atPath: temp.generation(3).path),
            "详细档只留两代 —— `.3` 不该存在")
    #expect(main <= DiagnosticsLog.Level.verbose.maxBytes + lineBytes,
            "主文件 \(main) 字节，超过详细档上限加一行")
    #expect(main + g1 + g2 <= 3 * DiagnosticsLog.Level.verbose.maxBytes + 3 * lineBytes,
            "详细档总占用 \(main + g1 + g2) 字节，超过三代上限加三行")
}

/// **`moveItem` 失败必须退化为就地截断**，否则主文件无界增长 —— `rotate` 里
/// 唯一能破坏「有界」这条硬约束的条件。理由与 Rust 那两份逐字相同。
///
/// ⚠️ 造法是**确定性的**：把 `.1` 那个名字占成**目录** ⇒ 轮转挪不动。它与真机上
///    "客户正用别的东西开着日志 / 那个名字被占住"是同一形状。
///
/// 判别力：把兜底那两句（就地截断 + `complainOnce`，顺序见 `rotate`）删掉，
/// 本测试必红 —— 主文件会一路长到 12000 × 180 = 2,160,000 字节。
@Test func aFailedRotationTruncatesInsteadOfGrowingForever() throws {
    let temp = try TempLogDir()
    defer { temp.cleanUp() }
    try FileManager.default.createDirectory(at: temp.generation(1), withIntermediateDirectories: true)

    for i in 0..<12000 {
        DiagnosticsLog.write(padded(i), to: temp.log, level: .normal)
    }

    let main = temp.size(temp.log)
    #expect(main <= DiagnosticsLog.maxBytes + lineBytes,
            "轮转失败之后主文件长到了 \(main) 字节 —— 兜底截断没生效")
}

/// **写不进去也不许把产品弄坏**：路径是某个普通文件的子路径时，写入必须失败得无声。
@Test func aWriteFailureNeverPanics() throws {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-diag-not-a-dir", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    // 拿一个**已存在的普通文件**当目录用 ⇒ 建目录必然失败。
    let file = dir.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: file)

    DiagnosticsLog.write("这一条写不进去，但绝不许 panic",
                         to: file.appendingPathComponent("sub").appendingPathComponent("diag.log"),
                         level: .normal)
}

// ---------------------------------------------------------------------------
// 一个字段值封顶 200 个 **Unicode 标量**（⚠️ 不是 `Character` —— 那是扩展字形簇）
// ---------------------------------------------------------------------------

/// 🔴 **200 是 Unicode 标量数，不是字节数**（Rust 那两份用的是 `chars().take(200)`）。
/// ⚠️ 也**不是** Swift 的 `Character`（扩展字形簇）：这条用例的夹具是汉字，两者恰好都是 200
/// ⇒ **它分不出"按标量截"与"按字形簇截"**，那件事由下面那几条组合重音的用例负责。
///
/// 判别力：改成按**字节**截断（`utf8.prefix(200)` 或 `Data` 那一档）⇒
/// 300 个汉字只剩 66 个字符，本测试必红。
/// 为什么需要它：`why` 里可能是第三方（内核 / 系统）的原始错误文案，
/// 长度不受我们控制 —— 不截断的话"一行"可以是任意长。
@Test func aValueIsTruncatedToTwoHundredCharacters() {
    let long = String(repeating: "汉", count: 300)
    let line = DiagnosticsLog.formatLine("kernel_call", [("why", long)])

    let why = value(of: "why", in: line) ?? ""
    #expect(why.count == 200, "截断到 200 个 Unicode 标量（夹具是汉字，所以也算 200 个 Character），实际 \(why.count)")
    #expect(why.utf8.count == 600,
            "按字符截断 ⇒ 200 个汉字是 600 字节；按字节截断的话这里是 200 —— 实际 \(why.utf8.count)")
    #expect(why.hasPrefix("汉"), "截的是**尾巴**（前 200 个标量留着），不是头")
}

/// 🔴 **200 的单位是 Unicode 标量，不是 Swift 的 `Character`（扩展字形簇）。**
///
/// Rust 那两份的 `v.chars().take(200)` 里 `char` 是**一个标量**；而 `Character` 是
/// **扩展字形簇**（可以含任意多个标量）⇒ `value.prefix(200)` 与它**不等价**。
///
/// ⚠️ 这**不是纸面差异**：macOS 的文件名是 **NFD（分解形）**，所以 `why` 里带重音的文件名
///    是现实情形 —— 按字形簇截会让"一个字段值"长到两倍，而**"每一行有上界"**这条硬约束
///    正是被那一格顶破的。
///
/// 取 `"e" + U+0301`（e = 1 字节 / 1 标量，U+0301 = 2 字节 / 1 标量，合起来是**一个**字形簇）：
///   · 按**标量**截 200 ⇒ 100 个字形簇、**300 字节**（= Rust 那边的读数）；
///   · 按**字形簇**截 200 ⇒ 200 个字形簇、400 标量、**600 字节**（旧写法）。
///
/// 判别力：把 `unicodeScalars.prefix(…)` 退回 `prefix(…)`，**本测试必红**（300 → 600）。
@Test func aValueIsTruncatedToTwoHundredScalars() {
    let combining = String(repeating: "e\u{0301}", count: 300)
    let line = DiagnosticsLog.formatLine("kernel_call", [("why", combining)])
    let why = value(of: "why", in: line) ?? ""

    #expect(why.unicodeScalars.count == 200,
            "截断单位是**标量**（Rust 的 `char`）：实际 \(why.unicodeScalars.count) 个标量")
    #expect(why.count == 100,
            "200 个标量 = 100 个字形簇（按字形簇截会留下 200 个）：实际 \(why.count)")
    #expect(why.utf8.count == 300,
            "100 × (1 + 2) 字节 = 300（按字形簇截会是 600）：实际 \(why.utf8.count) 字节")
    // 前提自查：这个夹具确实是"一个字形簇、两个标量"（不然上面三条没有判别力）。
    #expect(combining.count == 300 && combining.unicodeScalars.count == 600,
            "夹具坏了：300 个组合对应该是 300 个字形簇 / 600 个标量")
}

/// 空白判定也按**标量**：`CR LF` 在 Swift 里是**一个** `Character`（换行是一个字形簇），
/// 按 `Character.isWhitespace` 判只会出一个 `_` —— 而 Rust 那边是两个。
///
/// 判别力：把 `formatLine` 里那一圈换回 `Character`（`value.prefix(n).map { $0.isWhitespace … }`）
/// 或者换成 `String(value.prefix(n)).map { $0.isWhitespace … }`，本测试必红（`a__b` → `a_b`）。
@Test func carriageReturnAndLineFeedMakeTwoUnderscores() {
    let line = DiagnosticsLog.formatLine("x", [("why", "a\r\nb")])
    #expect(value(of: "why", in: line) == "a__b",
            "CR 与 LF 是两个标量 ⇒ 两个 `_`（Swift 的 `Character` 会把它们并成一个）：实际 \(value(of: "why", in: line) ?? "nil")")
}

/// 截断发生在**换空格之前**（Rust 那两份是 `take(n).map(…)`，次序一样）：
/// 第 200 个标量之后的内容一个字都不该出现。
@Test func theTruncationHappensBeforeTheWhitespaceReplacement() {
    let line = DiagnosticsLog.formatLine("x", [("v", String(repeating: "a", count: 199) + " 尾巴")])
    let v = value(of: "v", in: line) ?? ""
    #expect(v.count == 200, "第 200 个字符是空格 ⇒ 换成 `_`；再往后的一个字都不留：\(v.count)")
    #expect(v.hasSuffix("_"), "刚切在第 200 个字符（空格）上，它要换成 `_`：\(v.suffix(4))")
}

// ---------------------------------------------------------------------------
// 落到哪：与 `preferences.json` 同一个目录（同一处来源）
// ---------------------------------------------------------------------------

/// 壳的日志落在 `ShellStorage.directory`（= `AppPreferencesStore.defaultURL` 那个目录）
/// —— 与内核的 `diag-kernel.log` 同一处，客户一次就能把几个文件拿齐。
///
/// ⚠️ **只读**：本测试只算路径、**不写**。那个目录是开发者自己真实的
///    `~/Library/Application Support/BenagenDownloader/`。
@Test func theShellLogLivesNextToThePreferences() {
    #expect(DiagnosticsLog.fileName == "diag-shell.log")
    #expect(DiagnosticsLog.logURL.deletingLastPathComponent() == ShellStorage.directory)
    #expect(DiagnosticsLog.logURL.deletingLastPathComponent()
            == AppPreferencesStore.defaultURL.deletingLastPathComponent(),
            "与 preferences.json 同一处来源 —— 两处各推一次会分叉")
    #expect(!DiagnosticsLog.logURL.path.hasSuffix("diag-kernel.log"),
            "壳与内核各写自己的文件：同一个文件被两个进程追加 + 轮转会打架")
}

// ---------------------------------------------------------------------------
// 写盘前抹掉的那几个字面串（规格 §2.3 B 的"按构造避开"）
// ---------------------------------------------------------------------------

/// 🔴 **空串不许当 `secret`**：那两处推下来的清单里，"没有配置下载目录"**就是**空串
/// （`AppPreferences.downloadDir` 的"未配置"取值），所以它每次都在里面。
///
/// 判别力：把 `redact` 里那句 `where !secret.isEmpty` 删掉 ⇒ 本用例红
/// （`replacingOccurrences(of: "")` 的行为没有意义，而它会把整行变成一串 `[已隐去]`）。
@Test func anEmptySecretIsSkippedInsteadOfShreddingTheLine() {
    let secrets = ["", "ABC123"]
    #expect(DiagnosticsLog.redact("交付码 ABC123 不对", secrets: secrets) == "交付码 [已隐去] 不对",
            "空串要跳过，而非空的那个要抹掉")
    #expect(DiagnosticsLog.redact("原样", secrets: []) == "原样",
            "没有可抹的串时**逐字**原样交出去（不许顺手改写别的东西）")
}

/// **它抹的是字面子串，不是"过滤器"**：认出的是那两个串本身。
///
/// ⚠️ 这一条钉的是**边界**，不是能力：码以别的写法出现（大小写不同、被 URL 编码、
///    被拆成两段）时抹不掉 —— 那是规格 §2.7 显式接受的那一半，别把它读成"这份日志
///    干净了"。写在这里是为了让下一个人知道**哪里是尽头**，而不是去加强它。
@Test func redactionIsLiteralAndBounded() {
    #expect(DiagnosticsLog.redact("C24-8ZQ7", secrets: ["C24-8ZQ7"]) == DiagnosticsLog.redacted)
    #expect(DiagnosticsLog.redact("c24-8zq7", secrets: ["C24-8ZQ7"]) == "c24-8zq7",
            "大小写不同 ⇒ 抹不掉（这是**账**，不是要修的缺陷）")
    // 多个串**按顺序**各抹一遍 ⇒ 谁先被扫到谁先抹。⚠️ 于是"短的串是长的串的前缀"
    // 时，长的那一条会被吃掉一半（`C24` 先抹掉之后 `C24-8` 再也匹配不上）。
    // 现实里构造不出这一格（那两个串是交付码与下载目录，一个不可能含在另一个里面），
    // 所以**不做特殊处理**——把它记在这里，而不是顺手加一段"按长度排序"的代码。
    #expect(DiagnosticsLog.redact("C24-8", secrets: ["C24", "C24-8"]) == "[已隐去]-8",
            "先出现的那个串先抹（顺序由调用方给的清单决定）")
}
