import Testing
import Foundation
import AppKit
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 品牌资产的**查找与读取**（任务 12）
//
// 这里盯的不是"能不能读文件"，而是三件**会静默失效**的事：
//   · **名字对不上**：`BrandAssets` 的常量与 `build_app_macos.sh` 里那三个 `cp` 的目标
//     文件名不一致时的表现是"应用里没有图标、空态页没有标"——**两件都不报错**；
//   · **扩展名**：`Bundle.url(forResource:withExtension: nil)` 在 macOS 上**匹配不到**
//     带扩展名的文件（实测：包内明明有 `COPYING-GPLv2.txt`，传 `nil` 得到 `nil`）。
//     而我们的三个名字常量是**不带扩展名**的，所以 name→extension 的对应关系必须显式
//     钉住：配错的后果同样是"标不见了"，同样不报错；
//   · **空载荷**：读不到 / 空文件 / 解不开，三者都必须是错误（同 `LicenseText.load`）。
//
// ⚠️ 这里**不碰 `Bundle.main`**：测试进程的 `Bundle.main` 是 xctest，不是应用包。
//    "找不到"用**自己造的临时 bundle**（实测 `Bundle(path:)` 对普通目录也成立）与
//    测试包本身验；"读到了"用临时 bundle 里的真资源验。
// ---------------------------------------------------------------------------

/// 锚点类：只是为了拿到**测试包**这个 `Bundle`（它一定没有品牌资产）。
private final class BrandTestBundleAnchor {}

/// 取一段必然抛错的代码抛出的 `BrandAssetError`。
///
/// 不用 `#expect(throws: 某个实例)`：`.notAnImage` 的第二个关联值是**诊断原文**，
/// 逐字比不了；这里要验的是**哪一支**。抛出来的是别的类型时让它直接失败（这是真错误）。
private func capturedError(_ body: () throws -> Void) throws -> BrandAssetError {
    do {
        try body()
    } catch let e as BrandAssetError {
        return e
    }
    throw CaptureFailure.nothingWasThrown
}

/// `capturedError` 里"什么都没抛"的信号 —— 那本身就是测试失败（被测代码没报错）。
private enum CaptureFailure: Error { case nothingWasThrown }

/// 夹具自己坏掉时的信号（不是被测代码的问题，但同样必须红）。
private enum FixtureFailure: Error { case temporaryBundleDidNotResolve }

// MARK: - 名字与扩展名（与打包脚本的契约）

@Test func resourceNamesMatchWhatTheBuildScriptCopies() {
    // 这三个常量与 `build_app_macos.sh` 里那三个 `cp` 的目标文件名逐字一致。
    // 对不上的表现是：应用里没有图标、空态页没有标 —— 而这两件都不会报错。
    #expect(BrandAssets.markName == "benagen-mark")
    #expect(BrandAssets.fullLogoName == "benagen-full-logo")
    #expect(BrandAssets.appIconName == "AppIcon")
}

@Test func resourceExtensionsMatchWhatTheBuildScriptCopies() {
    // ⚠️ 名字常量**不带**扩展名（`CFBundleIconFile` 的约定就是不写 `.icns`），
    //    所以"哪个名字配哪个扩展名"是查找函数自己的一份显式映射 —— 配错 = 标消失且不报错。
    #expect(BrandAssets.markExtension == "png")
    #expect(BrandAssets.fullLogoExtension == "png")
    #expect(BrandAssets.appIconExtension == "icns")
}

// MARK: - 查找

// ⚠️ 用例名是**模块级**的自由函数（swift-testing）：与 `LicenseTextTests` 里的同名用例会
//    "invalid redeclaration" 直接编译失败 —— 所以这里的每条名字都带着 Brand 的语义。
@Test func aBundleWithoutTheBrandResourceResolvesToNilNotAGuess() {
    let bundle = Bundle(for: BrandTestBundleAnchor.self)
    #expect(BrandAssets.url(named: BrandAssets.markName, in: bundle) == nil,
            "测试包里没有这份资源：查找必须回 nil，而不是猜一个路径")
    #expect(BrandAssets.url(named: BrandAssets.fullLogoName, in: bundle) == nil)
    #expect(BrandAssets.url(named: BrandAssets.appIconName, in: bundle) == nil)
}

@Test func theLookupFindsExactlyTheFileNamesTheBuildScriptCopies() throws {
    // 夹具用的就是打包脚本拷进 `Contents/Resources/` 的那三个**文件名**（逐字）。
    // 这条把 name + extension 的配对钉死在真实文件名上：把 `markExtension` 改成 "jpeg"，
    // 或者把 `markName` 改成别的，都会在**这里**红，而不是在用户机器上静默消失。
    let files = ["AppIcon.icns", "benagen-mark.png", "benagen-full-logo.png"]
    let (bundle, dir) = try temporaryBundle(Array(repeating: Data("x".utf8), count: files.count),
                                            named: files)
    defer { try? FileManager.default.removeItem(at: dir) }

    for name in [BrandAssets.appIconName, BrandAssets.markName, BrandAssets.fullLogoName] {
        let url = try #require(BrandAssets.url(named: name, in: bundle),
                               "包里有这个文件，查找却回 nil：名字或扩展名对不上")
        #expect(files.contains(url.lastPathComponent))
    }
}

@Test func anUnknownNameIsNilNotASearchForSomethingSimilar() throws {
    // 不认识的资源名不许"就近找"（同 `LicenseText.resourceURL` 的口径：不猜路径）——
    // 回落成"找个名字差不多的"会让一个拼错的名字**悄悄显示成另一张图**。
    let (bundle, dir) = try temporaryBundle([Data("x".utf8)], named: ["benagen-mark.png"])
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(BrandAssets.url(named: "benagen-mark-2", in: bundle) == nil)
    #expect(BrandAssets.url(named: "benagen-mark.png", in: bundle) == nil,
            "带扩展名的名字不是我们的口径（常量本身不带扩展名），不该被当资源名接受")
}

// MARK: - 读取（三种失败 + 一种成功）

@Test func aMissingResourceIsAnExplicitErrorNotAnEmptyPayload() throws {
    // 传一个肯定没有这份资源的 bundle（测试进程的 `Bundle.main` 是 xctest）。
    #expect(throws: BrandAssetError.self) {
        _ = try BrandAssets.load(named: BrandAssets.markName, from: Bundle(for: BrandTestBundleAnchor.self))
    }
    // 是哪一支：`.missingResource` 且**名字带上**（能照着去找是哪个文件没打进去）
    let e = try capturedError {
        _ = try BrandAssets.load(named: BrandAssets.markName, from: Bundle(for: BrandTestBundleAnchor.self))
    }
    #expect(e == .missingResource(BrandAssets.markName))

    let (emptyBundle, dir) = try temporaryBundle([], named: [])
    defer { try? FileManager.default.removeItem(at: dir) }
    #expect(try capturedError { _ = try BrandAssets.load(named: BrandAssets.markName, from: emptyBundle) }
            == .missingResource(BrandAssets.markName))
}

@Test func anEmptyFileIsRejectedLikeAMissingOne() throws {
    // 0 字节的 logo 与"没有 logo"在界面上是同一件事（都不渲染），所以必须同等处理：
    // 让它在**读**这一层就报出来，而不是把一个空 Data 当成成功交给视图。
    let (bundle, dir) = try temporaryBundle([Data()], named: [BrandAssets.markName + ".png"])
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(try capturedError { _ = try BrandAssets.load(named: BrandAssets.markName, from: bundle) }
            == .emptyResource(BrandAssets.markName))
}

@Test func aNonImageFileIsRejectedInsteadOfBeingHandedToTheView() throws {
    // 扩展名对了、内容不是图像（例如把 HTML 错误页当成 png 存下来了）：
    // 必须在读这一层报错，而不是把一个解不开的 Data 递给 `NSImage`（那会静默变成空图）。
    let (bundle, dir) = try temporaryBundle([Data("<html>not a png</html>".utf8)],
                                            named: [BrandAssets.markName + ".png"])
    defer { try? FileManager.default.removeItem(at: dir) }

    let e = try capturedError { _ = try BrandAssets.load(named: BrandAssets.markName, from: bundle) }
    guard case .notAnImage(let name, let why) = e else {
        Issue.record("应当是 .notAnImage，实际是 \(e)"); return
    }
    #expect(name == BrandAssets.markName)
    #expect(!why.isEmpty, "诊断原文不得是空串（要能看出是格式不对还是数据被截断）")
}

@Test func anUnreadableBrandFileIsAFailureNotACrash() throws {
    let url = URL(fileURLWithPath: "/tmp/benagen-brand-does-not-exist-\(UUID().uuidString).png")
    let e = try capturedError { _ = try BrandAssets.load(from: url, named: BrandAssets.markName) }
    guard case .unreadable(let name, let why) = e else {
        Issue.record("应当是 .unreadable，实际是 \(e)"); return
    }
    #expect(name == BrandAssets.markName)
    #expect(!why.isEmpty, "错误原文不得是空串")
}

@Test func theLoadedBytesAreADecodablePNG() throws {
    // **真资源**走完整条 `load(named:from:)`：临时 bundle 里放的就是入库的那张品标。
    // 断言两件事：真是 PNG（魔数），且 `NSImage` 解得出 —— 后者正是视图拿到它之后做的事。
    let (bundle, dir) = try temporaryBundle([try Data(contentsOf: committedMarkURL)],
                                            named: [BrandAssets.markName + ".png"])
    defer { try? FileManager.default.removeItem(at: dir) }

    let data = try BrandAssets.load(named: BrandAssets.markName, from: bundle)
    #expect(data.starts(with: [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]), "不是 PNG 魔数")
    let image = try #require(NSImage(data: data), "NSImage 解不开这份数据（视图那一层就会是空标）")
    #expect(image.size.width > 0 && image.size.height > 0)
}

// MARK: - 入库的产物本身

@Test func theCommittedArtifactsAreAllPresentAndDecodable() throws {
    // 这三件是**入库的静态产物**（`make_brand_assets.py` 手动跑出来的，构建不生成它们）。
    // 一条 `test.sh` 就能挡住"没提交"“提交了个 0 字节的文件”“把 JPEG 存成了 .png”，
    // 而这三种在界面上都只是"标不见了"（不报错）。AppIcon.icns 由 LaunchServices 读，
    // 壳里根本没有代码碰它 —— 不在这里看一眼，它坏了要等到用户在 Dock 上发现。
    for (url, isPNG) in [(committedMarkURL, true),
                         (committedFullLogoURL, true),
                         (committedAppIconURL, false)] {
        let data = try Data(contentsOf: url)
        #expect(!data.isEmpty, "\(url.lastPathComponent) 是空文件")
        if isPNG {
            #expect(data.starts(with: [0x89, 0x50, 0x4E, 0x47]), "\(url.lastPathComponent) 不是 PNG")
        }
        #expect(NSImage(data: data) != nil, "\(url.lastPathComponent) 解不开")
    }
}

// MARK: - 夹具

/// `macos/Resources/`（打包脚本就是从这里拷进 `Contents/Resources/` 的）。
///
/// 用 `#filePath` 上溯，不依赖测试的工作目录（`test.sh` 会 cd 到 `macos/`，
/// 但裸调 `swift test` 时不一定）。
private var resourcesDir: URL {
    URL(fileURLWithPath: #filePath)          // macos/Tests/BenagenCoreKitTests/BrandAssetsTests.swift
        .deletingLastPathComponent()         // macos/Tests/BenagenCoreKitTests
        .deletingLastPathComponent()         // macos/Tests
        .deletingLastPathComponent()         // macos
        .appendingPathComponent("Resources")
}

private var committedMarkURL: URL { resourcesDir.appendingPathComponent("benagen-mark.png") }
private var committedFullLogoURL: URL { resourcesDir.appendingPathComponent("benagen-full-logo.png") }
private var committedAppIconURL: URL { resourcesDir.appendingPathComponent("AppIcon.icns") }

/// 造一个临时目录当 bundle 用（`Bundle(path:)` 对普通目录成立，实测），
/// 把给定的内容以给定的**文件名**放进去。
///
/// 这样"查找 → 读 → 校验"整条 `load(named:from:)` 都能被驱动，
/// 既不依赖仓库外的构建产物，也能覆盖到每一条失败分支。
private func temporaryBundle(_ contents: [Data], named names: [String]) throws -> (Bundle, URL) {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-brand-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    for (data, name) in zip(contents, names) {
        try data.write(to: dir.appendingPathComponent(name))
    }
    guard let bundle = Bundle(path: dir.path) else {
        throw FixtureFailure.temporaryBundleDidNotResolve
    }
    return (bundle, dir)
}
