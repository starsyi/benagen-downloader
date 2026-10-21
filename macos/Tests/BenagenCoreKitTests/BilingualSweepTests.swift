@testable import BenagenCoreKit
import Foundation
import Testing

/// 守门用例：**视图与呈现层不许再出现中文字面量**。
///
/// 0.3.0 的双语化把客户可见的句子全部收进 `I18n.swift` 的两张表
/// （`I18n.zh` / `I18n.en` 是两个穷尽的 `switch`：漏一条编译不过）。
/// 但"忘了走表"这件事**不会**编译失败 —— 它会静默地让一句中文在英文界面里露出来；
/// 而且漏网的那句往往只在一条罕见分支里出现（比如"诊断导出第三档失败"），
/// 跑不到那条分支就永远不红。所以这道闸查**源码**，不查运行时。
///
/// 白名单只放两类，且都是**逐条**的（文件 + 片段），不是整目录放行：
///   1. `I18n.swift`（两张表本身就是句子）与 `DiagnosticsLog.swift`
///      （日志正文按规格 I-2 保持开发语言，不随界面语言变）；
///   2. 每条写明理由的字面量：`os_log`/日志行、跨端稳定的文件名与字段名、
///      以及 `ErrorCard` 这份在本轮双语化之前就已经自带 zh/en 两张表的旧实现。
@Suite("Bilingual sweep")
struct BilingualSweepTests {

    // MARK: - 白名单（每条都得说清为什么可以留下）

    private struct Allowance {
        /// 相对 `Sources/` 的路径（POSIX 分隔符）。
        let file: String
        /// 字面量里必须出现的一段（够独特即可，免得把整句长文案抄一遍）。
        let marker: String
        /// 为什么这句可以留中文。
        let reason: String

        var description: String { "\(file) ~ 「\(marker)」：\(reason)" }
    }

    /// 整份文件豁免：只有"本身就是句子表"或"就是日志正文"这两种。
    private static let allowedFiles: [String: String] = [
        "BenagenCoreKit/I18n.swift":
            "两张表本身就是句子（zh/en 各一支，漏一支编译不过）。",
        "BenagenCoreKit/DiagnosticsLog.swift":
            "日志正文按规格 I-2 保持中文（排查用，不随界面语言变）。",
        "BenagenCoreKit/Presentation/ErrorCard.swift":
            "本轮双语化之前就已自带 zh/en 两张表（每个错误类都同时有中英两种写法）；"
                + "迁进 I18n 是一笔独立重构，不在 0.3.0 这一轮。",
    ]

    /// 逐条豁免：日志行、跨端稳定的名字、以及没有语言上下文的诊断。
    private static let allowedLiterals: [Allowance] = [
        // ---- os_log / 日志正文（规格 I-2：日志不翻译）----
        .init(file: "BenagenDownloader/App.swift",
              marker: "自动重开早退：",
              reason: "os_log 正文（规格 I-2：日志不翻译）。"),
        .init(file: "BenagenDownloader/App.swift",
              marker: "「显示主窗口」被触发",
              reason: "os_log 正文（规格 I-2：日志不翻译）。"),
        .init(file: "BenagenDownloader/Views/SettingsEntry.swift",
              marker: "showSettingsWindow: 没人接",
              reason: "os_log 正文（macOS 13 设置窗口兜底路径的排查线索）。"),

        // ---- 跨端稳定的文件/字段名（排查时被 grep，不能随语言变）----
        .init(file: "BenagenCoreKit/DiagnosticsExport.swift",
              marker: "诊断日志-",
              reason: "导出子目录名的前缀：与 Windows 的导出目录名同形，"
                  + "且客户与支持人员按这个名字找现场（改成英文会让两端对不上）。"),
        .init(file: "BenagenCoreKit/DiagnosticsExport.swift",
              marker: "说明.txt",
              reason: "说明文件的固定文件名：与 Windows 导出同形，换语言会让客户找不到它。"),

        // ---- 没有语言上下文的诊断 ----
        .init(file: "BenagenCoreKit/AppModel.swift",
              marker: "内核无响应（等待超过 5 秒）",
              reason: "握手超时那句**与内核原文逐字比对**（`start()` 用相等判据区分"
                  + "\"超时\"与\"内核回了别的错\"），改字会改判据。"),
        .init(file: "BenagenCoreKit/AppModel.swift",
              marker: "测试接缝没有提供重启工厂",
              reason: "`init(client:makeClient:)` 的测试接缝：只在单测里用，"
                  + "永远不会进客户界面（生产路径总会注入工厂）。"),
        .init(file: "BenagenCoreKit/CoreClient.swift",
              marker: "环境变量 BENAGEN_CORE 未设置",
              reason: "内核定位失败时的补充说明：这条错误在**握手之前**生成，"
                  + "那时语言还没有解析出来（默认 zh），且只在开发/打包缺失时出现。"),
        .init(file: "BenagenCoreKit/CoreClient.swift",
              marker: "）",
              reason: "上面那条\"找不到内核可执行文件\"消息的收尾括号片断："
                  + "与同一句同一条白名单（握手前生成、语言尚未解析）；"
                  + "它单独成串是因为中间要拼内核候选路径。"),
        .init(file: "BenagenCoreKit/CoreClient.swift",
              marker: "找不到内核可执行文件 benagen-core",
              reason: "同上：握手之前生成（语言尚未解析），且是打包事故的现场描述。"),
        .init(file: "BenagenCoreKit/JSONValue.swift",
              marker: "不是合法的 JSON 值",
              reason: "`Decodable` 的 `init(from:)` 没有语言上下文；"
                  + "这句只在自定义解码器（测试/诊断）里被抛出，正常 RPC 路径不走它。"),
        .init(file: "BenagenCoreKit/Protocol.swift",
              marker: "未知的节点类型",
              reason: "`TreeNode.init(from:)` 是 `Decodable`：没有语言上下文；"
                  + "只在协议载荷畸形（内核版本不匹配）时出现。"),
        .init(file: "BenagenCoreKit/Protocol.swift",
              marker: "未知的条目类型",
              reason: "`DirEntry.init(from:)` 同上（`Decodable`，无语言上下文）。"),
        .init(file: "BenagenCoreKit/Presentation/BrandAssets.swift",
              marker: "ImageIO",
              reason: "资源打包事故的现场描述（图标/品牌图读不出来），"
                  + "只在屏幕外日志与崩溃诊断里出现，不是客户文案。"),
        .init(file: "BenagenCoreKit/Presentation/SettingsForm.swift",
              marker: "Settings 编码失败",
              reason: "`preconditionFailure`：编程错误（不是运行时状况），"
                  + "按规格 I-2 这类开发者诊断保持中文。"),
    ]

    // MARK: - 用例

    @Test("sources 里除白名单外没有中文字符串字面量")
    func noChineseLiteralsOutsideTheAllowedList() throws {
        let files = try Self.swiftFiles()
        // 路径推导错的话这道闸会"永远绿"——那比没有闸更糟，所以先钉住"确实扫到了东西"。
        #expect(files.count > 20, "只扫到 \(files.count) 个 .swift 文件，Sources 路径大概推导错了")

        var offenders: [String] = []
        for file in files {
            let rel = Self.relativePath(of: file)
            if Self.allowedFiles[rel] != nil { continue }
            let source = try String(contentsOf: file, encoding: .utf8)
            for literal in Self.stringLiterals(in: source) where Self.containsCJK(literal) {
                let excused = Self.allowedLiterals.contains {
                    $0.file == rel && literal.contains($0.marker)
                }
                if excused { continue }
                offenders.append("\(rel)：\(Self.preview(literal))")
            }
        }

        #expect(offenders.isEmpty, """
            发现 \(offenders.count) 处中文字面量没有走 I18n 表：
            \(offenders.joined(separator: "\n"))
            ⇒ 客户可见的句子请写进 Sources/BenagenCoreKit/I18n.swift 的 zh/en 两张表；
              日志正文（规格 I-2）请按上面的白名单**逐条**登记并写明理由。
            """)
    }

    @Test("白名单里没有已经失效的条目")
    func everyAllowanceStillMatchesSomething() throws {
        let files = try Self.swiftFiles()
        var healthy: [String: String] = [:]
        for file in files {
            let rel = Self.relativePath(of: file)
            guard Self.allowedFiles[rel] == nil else { continue }
            let source = try String(contentsOf: file, encoding: .utf8)
            for literal in Self.stringLiterals(in: source) where Self.containsCJK(literal) {
                healthy[rel, default: ""] += literal + "\u{0}"
            }
        }
        let stale = Self.allowedLiterals.filter { entry in
            !(healthy[entry.file] ?? "").contains(entry.marker)
        }
        #expect(stale.isEmpty, """
            白名单里有 \(stale.count) 条已经对不上任何字面量（文件改过或豁免过时了）：
            \(stale.map(\.description).joined(separator: "\n"))
            ⇒ 失效的豁免会让闸悄悄变松，请删掉或更新它。
            """)
    }

    @Test("诊断说明的英文版里没有中文")
    func diagnosticHeaderEnglishHasNoCJK() {
        let header = DiagnosticsExport.header(
            appVersion: "0.3.0",
            coreBinary: nil,
            protocolVersion: nil,
            verboseLogging: true,
            os: "Version 15.6.0 (Build 24G84)",
            arch: "arm64",
            at: Date(timeIntervalSince1970: 1_700_000_000),
            lang: .en)
        let text = DiagnosticsExport.headerText(header, lang: .en)
        #expect(!Self.containsCJK(text), "英文版说明.txt 里混进了中文")
    }

    // MARK: - 路径

    private static var sourcesDirectory: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // BenagenCoreKitTests
            .deletingLastPathComponent()   // Tests
            .deletingLastPathComponent()   // macos
            .appendingPathComponent("Sources", isDirectory: true)
    }

    private static func relativePath(of file: URL) -> String {
        let base = sourcesDirectory.standardizedFileURL.path
        let full = file.standardizedFileURL.path
        guard full.hasPrefix(base + "/") else { return full }
        return String(full.dropFirst(base.count + 1))
    }

    private static func swiftFiles() throws -> [URL] {
        let fm = FileManager.default
        guard let walker = fm.enumerator(at: sourcesDirectory,
                                         includingPropertiesForKeys: [.isRegularFileKey]) else {
            return []
        }
        var out: [URL] = []
        for case let url as URL in walker where url.pathExtension == "swift" {
            out.append(url)
        }
        return out.sorted { $0.path < $1.path }
    }

    // MARK: - 词法：只取**非注释**的字符串字面量

    /// 扫描一份 Swift 源码里的字符串字面量（`//`、`///`、可嵌套的 `/* */` 一律跳过）。
    ///
    /// 处理这些形态：
    ///   - `"..."`（含 `\\`、`\"`、`\n`、`\u{XXXX}` 转义）
    ///   - `"""..."""`（多行串；行尾空格不是问题，这里只关心内容）
    ///   - `#"..."#`、`##"..."##`（原始串，`\#(` 插值也照收不误）
    ///
    /// **不**处理：字符串插值里再嵌一个字符串字面量这种极端形态（本仓没有；
    /// 真出现了也只会多收几个字符，不会漏掉中文）。
    static func stringLiterals(in source: String) -> [String] {
        let chars = Array(source)
        var literals: [String] = []
        var i = 0
        var blockDepth = 0

        while i < chars.count {
            let c = chars[i]

            if blockDepth > 0 {
                if c == "/", i + 1 < chars.count, chars[i + 1] == "*" {
                    blockDepth += 1; i += 2; continue
                }
                if c == "*", i + 1 < chars.count, chars[i + 1] == "/" {
                    blockDepth -= 1; i += 2; continue
                }
                i += 1; continue
            }

            if c == "/", i + 1 < chars.count {
                if chars[i + 1] == "/" {            // 含 `///`：行注释一直吃到换行
                    while i < chars.count, chars[i] != "\n" { i += 1 }
                    continue
                }
                if chars[i + 1] == "*" {
                    blockDepth = 1; i += 2; continue
                }
            }

            // 原始串：`#` 的数量决定定界符。
            if c == "#" {
                var hashes = 0
                var j = i
                while j < chars.count, chars[j] == "#" { hashes += 1; j += 1 }
                if j < chars.count, chars[j] == "\"" {
                    var k = j + 1
                    var body = ""
                    var closed = false
                    while k < chars.count {
                        if chars[k] == "\"" {
                            var h = 0
                            while k + 1 + h < chars.count, chars[k + 1 + h] == "#" { h += 1 }
                            if h >= hashes {
                                closed = true
                                i = k + 1 + hashes
                                break
                            }
                        }
                        body.append(chars[k]); k += 1
                    }
                    if closed { literals.append(body) } else { i = chars.count }
                    continue
                }
                i += 1; continue
            }

            if c == "\"" {
                // 多行串 `"""`（后面紧跟换行/内容；`""""` 这种四引号按空串处理也够用）。
                if i + 2 < chars.count, chars[i + 1] == "\"", chars[i + 2] == "\"" {
                    var k = i + 3
                    var body = ""
                    while k + 2 < chars.count {
                        if chars[k] == "\\", k + 1 < chars.count {
                            Self.appendEscape(chars[k + 1], into: &body,
                                              index: k, chars: chars, next: &k)
                            continue
                        }
                        if chars[k] == "\"", chars[k + 1] == "\"", chars[k + 2] == "\"" { break }
                        body.append(chars[k]); k += 1
                    }
                    literals.append(body)
                    i = min(k + 3, chars.count)
                    continue
                }
                // 普通串。
                var k = i + 1
                var body = ""
                while k < chars.count {
                    let d = chars[k]
                    if d == "\\", k + 1 < chars.count {
                        Self.appendEscape(chars[k + 1], into: &body,
                                          index: k, chars: chars, next: &k)
                        continue
                    }
                    if d == "\"" || d == "\n" { break }
                    body.append(d); k += 1
                }
                literals.append(body)
                i = k + 1
                continue
            }

            i += 1
        }
        return literals
    }

    /// ⚠️ 每一种转义都**必须**把 `next` 推到转义序列之后 —— 漏一条就是一个死循环
    ///    （`case "n"` 只 append 不推位置的话，扫描器会在那一格原地转圈）。
    private static func appendEscape(_ e: Character, into body: inout String,
                                     index: Int, chars: [Character], next: inout Int) {
        switch e {
        case "n": body.append("\n")
        case "t": body.append("\t")
        case "r": body.append("\r")
        case "0": body.append("\0")
        case "u":
            if index + 2 < chars.count, chars[index + 2] == "{" {
                var m = index + 3
                var hex = ""
                while m < chars.count, chars[m] != "}" { hex.append(chars[m]); m += 1 }
                if let value = UInt32(hex, radix: 16), let scalar = Unicode.Scalar(value) {
                    body.append(Character(scalar))
                }
                next = m + 1
                return
            }
            body.append("u")
        default:
            body.append(e)
        }
        next = index + 2
    }

    // MARK: - CJK 判定

    /// 汉字、CJK 标点（。、「」等）、全角形式都算"中文"——
    /// 英文界面上出现这些任何一个都是漏网。
    static func containsCJK(_ text: String) -> Bool {
        text.unicodeScalars.contains { scalar in
            switch scalar.value {
            case 0x3000...0x303F,      // CJK Symbols and Punctuation
                 0x3400...0x4DBF,      // CJK Unified Ideographs Extension A
                 0x4E00...0x9FFF,      // CJK Unified Ideographs
                 0xF900...0xFAFF,      // CJK Compatibility Ideographs
                 0xFE30...0xFE4F,      // CJK Compatibility Forms
                 0xFF00...0xFFEF:      // Halfwidth and Fullwidth Forms
                return true
            default:
                return false
            }
        }
    }

    private static func preview(_ literal: String) -> String {
        let flat = literal.replacingOccurrences(of: "\n", with: "\\n")
        return flat.count <= 60 ? "「\(flat)」" : "「\(flat.prefix(57))…」"
    }
}
