import Foundation
import ImageIO

// ---------------------------------------------------------------------------
// 品牌资产的**来源与失败分类**（任务 12：应用图标 / 空态页 logo / 关于窗口）
//
// ⚠️ 本文件**不要** `import SwiftUI`、也不要 `import AppKit`：它做的是
//    "从包里找到那份图片 → 把字节读出来 → 确认它真是一张能解码的图"，
//    全是 Foundation + ImageIO 的事，所以整份能进 `Tests/` 被单测。
//    `Data → NSImage → 屏幕上那张图` 这一段留给视图（那里没有可断言的判据，约束 8）。
//
// ⚠️⚠️ **这里与 `LicenseText` 有一处有意为之的不同，理由写在这里（约束 11）：**
//
//    GPL 全文读不到时，界面**必须**大声报错 —— 那是**法律义务**（应用内嵌 aria2，GPLv2），
//    一个空白许可页等于没履行义务。**logo 不是义务，它是装饰性资产。**
//    读不到时视图**不渲染它、也不弹任何错误**：在空态页上挂一条"logo 丢失"的横幅
//    是在打扰用户（他们既看不懂、也修不了），而这个缺陷的正确发现地点是**打包时**——
//    `macos/scripts/build_app_macos.sh` 会把这三个资源逐个做存在性自查，
//    **缺任何一个就让构建失败**。**判据的严重度要放在它能生效的地方。**
//
//    所以下面这个 `BrandAssetError` **只给开发 / 打包侧看**（定位用），刻意**不带**
//    `message` 之类的用户文案：那种文案在这个设计里没有任何落点。
// ---------------------------------------------------------------------------

/// 读一份品牌资产的四种失败。**只给开发 / 打包侧诊断用**（见文件顶部那段）。
///
/// ⚠️ 分成四支而不是一个"读取失败"：这四件事的**处置不同** ——
///    `.missingResource` / `.emptyResource` / `.notAnImage` 都是**打包缺陷**
///    （重新打包即可；`.notAnImage` 还额外说明入库的产物本身坏了），
///    而 `.unreadable` 是这台机器上这一次读盘的问题（原文要带出去）。
///    合成一支的后果是查的人分不清"是没打进去""打进去的是坏的"还是"这台机器读不了"。
public enum BrandAssetError: Error, Equatable {
    /// 包里没有这份资源（名字已带上，能照着去包里找）。
    case missingResource(String)
    /// 找到了，但 0 字节 —— 界面上与"没有"完全一样（都不渲染）。
    case emptyResource(String)
    /// 读到了字节，但 ImageIO 解不开。第二个值是诊断原文
    /// （不认得格式 / 取不到类型 / 数据被截断 —— 三者指向的修法不同）。
    case notAnImage(String, String)
    /// 路径在，但读不出来（权限 / IO 错误）。第二个值是系统原文。
    case unreadable(String, String)
}

/// 品牌资产的查找与读取。
///
/// 形状是刻意的（同 `LicenseText`）：**资源查找**（`url(named:in:)`）与
/// **读取**（`load(from:named:)`）分开，于是两者都能在测试里被驱动 ——
/// 前者用"肯定没有这份资源的 bundle"验失败分支，后者喂临时文件验成功分支，
/// 都不依赖仓库外的构建产物（测试进程的 `Bundle.main` 是 xctest，不是应用包）。
public enum BrandAssets {

    // MARK: - 与打包脚本的契约（macos/scripts/build_app_macos.sh 里那三个 cp 的目标文件名）

    /// 应用图标。`Info.plist` 的 `CFBundleIconFile` 指的就是这个名字
    /// （**不带扩展名**，这是那个键的约定）。
    public static let appIconName = "AppIcon"
    public static let appIconExtension = "icns"

    /// 图形标：空态页顶部那一张（透明底原件；白底由 SwiftUI 画，见 `EmptyState`）。
    public static let markName = "benagen-mark"
    public static let markExtension = "png"

    /// 含中英文全称的横版 logo：关于窗口里那一张。
    public static let fullLogoName = "benagen-full-logo"
    public static let fullLogoExtension = "png"

    /// 名字 → 扩展名。
    ///
    /// ⚠️ 这层映射**不能省**：`Bundle.url(forResource:withExtension: nil)` 在 macOS 上
    ///    **匹配不到带扩展名的文件**（实测：包内明明有 `COPYING-GPLv2.txt`，
    ///    传 `nil` 与传 `""` 都得到 `nil`，只有传 `"txt"` 才找得到）。
    ///    而上面三个名字常量是**不带扩展名**的（`CFBundleIconFile` 的约定），
    ///    所以"哪个名字配哪个扩展名"必须显式写下来 —— 配错的后果是"标不见了"，
    ///    而它**不报任何错**。
    private static let extensionsByName: [String: String] = [
        appIconName: appIconExtension,
        markName: markExtension,
        fullLogoName: fullLogoExtension,
    ]

    /// 应用包里那份资产的路径；没有时 `nil`（**不猜路径、不回落到别的近似文件**）。
    ///
    /// 不认识的名字同样回 `nil`：这里没有"就近找一张图"这种回落 ——
    /// 那会让一个拼错的名字悄悄显示成另一张图。
    public static func url(named name: String, in bundle: Bundle = .main) -> URL? {
        guard let ext = extensionsByName[name] else { return nil }
        return bundle.url(forResource: name, withExtension: ext)
    }

    /// 读出字节。**永不静默返回空**（同 `LicenseText.load`）：找不到 / 空文件 / 解不开，
    /// 三者都是错误。
    ///
    /// ⚠️ 传 `nil` 即"包里没有这份资源"，与"读失败"一样是错误 ——
    ///    视图那边读不到就整块不渲染，但**读**这一层不得把"没有"伪装成"读到了 0 字节"。
    public static func load(from url: URL?, named name: String) throws -> Data {
        guard let url else { throw BrandAssetError.missingResource(name) }

        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            throw BrandAssetError.unreadable(name, "\(error)")
        }
        // ⚠️ 这一条不是"多余的健壮性"：0 字节的 logo 在界面上与"没有 logo"完全一样
        //    （都不渲染），所以它必须在**读**这一层被拦下，而不是当成一份合法的成功载荷
        //    交给视图 —— 否则"产物是空的"这件事永远不会有任何地方知道。
        guard !data.isEmpty else { throw BrandAssetError.emptyResource(name) }

        try requireDecodableImage(data, named: name)
        return data
    }

    /// 便利入口：按资源名从包里找，再读。视图那边只调这一个。
    public static func load(named name: String, from bundle: Bundle = .main) throws -> Data {
        try load(from: url(named: name, in: bundle), named: name)
    }

    /// 确认这份字节真的解得成一张图。
    ///
    /// ⚠️ 这条同样不是"多余的健壮性"：一个 `.png` 里装着别的东西（例如把服务器返回的
    ///    错误页当源图存了下来）在界面上与"没有 logo"**看起来一模一样** ——
    ///    `NSImage(data:)` 解不开时回 `nil`，视图拿到 `nil` 就是不渲染。
    ///    在**读**这一层报出来，`.notAnImage` 这句诊断才有地方说得出口。
    private static func requireDecodableImage(_ data: Data, named name: String) throws {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else {
            throw BrandAssetError.notAnImage(name, "ImageIO 不认得这份数据：不是任何已知的图像格式")
        }
        guard let uti = CGImageSourceGetType(source) else {
            throw BrandAssetError.notAnImage(name, "ImageIO 取不到图像类型（UTI）")
        }
        guard CGImageSourceCreateImageAtIndex(source, 0, nil) != nil else {
            throw BrandAssetError.notAnImage(name, "ImageIO 解不开 \(uti) 图像：数据可能被截断")
        }
    }
}
