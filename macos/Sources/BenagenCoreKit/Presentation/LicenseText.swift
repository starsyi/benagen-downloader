import Foundation

// ---------------------------------------------------------------------------
// GPL 许可全文的**来源与失败文案**（任务 10）
//
// 为什么这段映射不在视图里（全局约束 8）：
//   `LicenseView` 是**法律义务**的落点 —— 应用内嵌 aria2（GPLv2），分发它就必须一并给出
//   许可全文与展示入口。视图那一层能写的断言只有"画了个 ScrollView"，而这里有两件
//   能写出判据、并且真的会失效的事：
//     · **读到的必须是全文**（逐字，不做整理、不截断 —— 见 `theWholeFileComesBackVerbatim`）；
//     · **读不到时绝不能是空白页**（`missingResource` / `emptyResource` / `unreadable`
//       三支各有一句说得清"出了什么事、该找谁"的话 —— 静默空页等于没履行义务，
//       而且没有人会去查一个空白的许可页）。
//
// ⚠️ 本文件**不要** `import SwiftUI`：它是无状态的纯映射 + 一次 `Bundle` 查找
//    （Foundation 足够），所以能进 `Tests/` 被单测。视图那一层只剩"把 `Result`
//    摆到屏幕上"。
// ---------------------------------------------------------------------------

/// 读 GPL 全文的三种失败。**每一种都带着一句给用户看的话**（`message`）。
///
/// ⚠️ 分成三支而不是一个"读取失败"：这三件事的**处置不同** ——
///    `.missingResource` / `.emptyResource` 是**打包缺陷**（该重装、该找分发方），
///    `.unreadable` 是这台机器上这一次读盘的问题（原文要带出去）。
///    合成一支的后果是那句"该怎么办"只能写成一句谁都对不上号的废话。
public enum LicenseError: Error, Equatable {
    /// 应用包里没有 `Contents/Resources/COPYING-GPLv2.txt`。
    case missingResource
    /// 找到了，但内容是空的（或只有空白字符）—— 界面表现与"找不到"一样是空白页。
    case emptyResource
    /// 路径在，但读不出来（权限 / IO 错误 / 非 UTF-8）。括号里是系统原文。
    case unreadable(String)

    /// 给用户看的那句话。**恒非空**（`aMissingResourceIsAnExplicitErrorNotABlankPage` 钉住）。
    ///
    /// 这是**壳自己写的话**（同 `EngineGate.unavailableHelp` 的性质）：内核在这里没有
    /// 任何 message 可登（约束 3 管的是"内核说了什么"，而这件事内核根本不知道）。
    /// 两处细节是刻意的：把**文件名**写出来（能照着找、能拿这句去问分发方），
    /// 以及明确说"这是打包缺陷"（否则用户会以为是自己点错了什么）。
    public var message: String {
        switch self {
        case .missingResource:
            return "应用包内找不到 GPLv2 全文（Contents/Resources/COPYING-GPLv2.txt）。"
                + "这是打包缺陷，不是您操作有误：请重新安装客户端；"
                + "若手上这份是别人拷来的，请向分发方索取一份许可副本。"
        case .emptyResource:
            return "应用包内的 GPLv2 全文是空的（Contents/Resources/COPYING-GPLv2.txt）。"
                + "这是打包缺陷，不是您操作有误：请重新安装客户端，并向分发方索取许可副本。"
        case .unreadable(let why):
            return "读取 GPLv2 全文失败：\(why)"
        }
    }
}

/// GPLv2 全文的读取。
///
/// 形状是刻意的：**资源查找**（`resourceURL(in:)`）与**读取**（`load(from:)`）分开，
/// 于是两者都能在测试里被驱动 —— 前者用"肯定没有这份资源的 bundle"验失败分支，
/// 后者喂一个临时文件验成功分支，都不依赖仓库外的构建产物（`Bundle.main` 在
/// 测试进程里是 xctest，不是应用包）。
public enum LicenseText {
    /// 打包脚本拷进 `Contents/Resources/` 的那个文件名（`macos/scripts/build_app_macos.sh`）。
    /// **改这里必须同时改那一边**：对不上的表现就是应用里只有一个错误页。
    public static let resourceName = "COPYING-GPLv2"
    public static let resourceExtension = "txt"

    /// 应用包里那份全文的路径；没有时 `nil`（**不猜路径、不回落到别的近似文件**）。
    public static func resourceURL(in bundle: Bundle = .main) -> URL? {
        bundle.url(forResource: resourceName, withExtension: resourceExtension)
    }

    /// 读出全文。**永不返回空白**：`url == nil`、空文件、读不出来，三者都是错误。
    ///
    /// ⚠️ 逐字返回（`theWholeFileComesBackVerbatim` 钉住首尾空白与空行）：
    ///    许可全文不是一个可以"整理"的文本 —— 裁剪过的 GPL 不再是 GPL。
    public static func load(from url: URL?) throws -> String {
        guard let url else { throw LicenseError.missingResource }

        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            throw LicenseError.unreadable("\(error)")
        }
        guard let text = String(data: data, encoding: .utf8) else {
            throw LicenseError.unreadable("文件不是 UTF-8 文本（\(url.lastPathComponent)）")
        }
        // ⚠️ 这一条不是"多余的健壮性"：一个 0 字节的 COPYING 在界面上与"找不到"完全一样
        //    —— 都是一个空白页，而空白页**没有任何提示**，谁都不会去查（GPL 分发义务里
        //    最要紧的那半句话正是"让人看得见"）。
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw LicenseError.emptyResource
        }
        return text
    }
}
