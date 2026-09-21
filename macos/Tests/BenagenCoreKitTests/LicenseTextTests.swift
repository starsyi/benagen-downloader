import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// GPL 许可全文的读取（任务 10）
//
// **这不是装饰**：应用内嵌 aria2（GPLv2），分发它就要一并给出许可全文与展示入口。
// 所以本文件盯的不是"能不能读文件"，而是那两条**义务**：
//   · 读到的必须是**逐字全文**（不做截断、不做编码转换、不是一句摘要）；
//   · 读不到时**绝不显示空白页** —— 必须是一句说得清"出了什么事、该找谁"的错误
//     （静默空页等于没履行义务，而且没有人会去查一个空白的许可页）。
//
// ⚠️ 这里**不碰 `Bundle.main`**：`load(from:)` 收的是一个**已经是路径**的 `URL?`，
//    真正的资源查找（`resourceURL(in:)`）单独一条、用"肯定没有这个资源的 bundle"验。
//    这样测试既不依赖仓库外的构建产物，也能覆盖到"找不到"这条分支。
// ---------------------------------------------------------------------------

/// 锚点类：只是为了拿到**测试包**这个 `Bundle`（它一定没有 `COPYING-GPLv2.txt`）。
private final class LicenseTestBundleAnchor {}

@Test func theResourceNameMatchesWhatTheBuildScriptInstalls() {
    // 打包脚本把仓库根目录的 GPL 全文拷进 `Contents/Resources/`（简报：任务 4 已做）。
    // 这两个常量就是那个文件名；改名而没改另一边 ⇒ 应用里只会显示一个错误页。
    #expect(LicenseText.resourceName == "COPYING-GPLv2")
    #expect(LicenseText.resourceExtension == "txt")
}

@Test func aBundleWithoutTheResourceResolvesToNilNotAGuess() {
    let bundle = Bundle(for: LicenseTestBundleAnchor.self)
    #expect(LicenseText.resourceURL(in: bundle) == nil,
            "测试包里没有这份资源：查找必须回 nil，而不是猜一个路径")
}

@Test func aMissingResourceIsAnExplicitErrorNotABlankPage() {
    #expect(throws: LicenseError.missingResource) {
        try LicenseText.load(from: nil)
    }
    // 文案必须是**一句看得见的话**，且要指得出文件放哪儿了
    let message = LicenseError.missingResource.message
    #expect(!message.isEmpty)
    #expect(message.contains("COPYING-GPLv2"))
}

@Test func theWholeFileComesBackVerbatim() throws {
    // 逐字：首尾空格、空行、缩进、标点一个都不能动 —— 许可全文不是一个可以"整理"的文本。
    let text = "GNU GENERAL PUBLIC LICENSE\nVersion 2, June 1991\n\n  <indented>\n\n"
    let url = try temporaryFile(text)
    defer { try? FileManager.default.removeItem(at: url) }

    #expect(try LicenseText.load(from: url) == text)
}

@Test func anEmptyFileIsAlsoAFailure() throws {
    // 空白页是**最坏**的形态：界面有、内容没有，谁都不会去查。空文件必须与"找不到"同等处理。
    let url = try temporaryFile("")
    defer { try? FileManager.default.removeItem(at: url) }
    #expect(throws: LicenseError.emptyResource) { try LicenseText.load(from: url) }

    let blank = try temporaryFile("\n   \n\t\n")
    defer { try? FileManager.default.removeItem(at: blank) }
    #expect(throws: LicenseError.emptyResource) { try LicenseText.load(from: blank) }
    #expect(!LicenseError.emptyResource.message.isEmpty)
}

@Test func anUnreadablePathIsAFailureNotACrash() {
    // 路径存在但读不了（这里是"文件不存在"；权限那种同理）——
    // 必须是**抛错**，不是把空串当成成功内容返回。原文要带在 message 里。
    let url = URL(fileURLWithPath: "/tmp/benagen-license-does-not-exist-\(UUID().uuidString)")
    do {
        _ = try LicenseText.load(from: url)
        Issue.record("读不到的文件不得当成成功")
    } catch let e as LicenseError {
        guard case .unreadable(let why) = e else {
            Issue.record("应当是 .unreadable，实际是 \(e)"); return
        }
        #expect(!why.isEmpty, "错误原文不得是空串")
        #expect(!e.message.isEmpty)
    } catch {
        Issue.record("不该抛出别的错误：\(error)")
    }
}

/// 写一个临时文件（内容按 UTF-8 原样落盘）。
private func temporaryFile(_ content: String) throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-license-\(UUID().uuidString).txt")
    try content.write(to: url, atomically: true, encoding: .utf8)
    return url
}
