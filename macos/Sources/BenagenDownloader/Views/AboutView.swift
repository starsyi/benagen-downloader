import SwiftUI
import AppKit
import BenagenCoreKit

/// 「关于」窗口（⌘ 菜单 →「关于 Benagen 数据下载工具」，见 `App.swift`）。
///
/// 三件东西：**全称 logo**（`benagen_full-logo.png` 的入库产物，打包脚本拷进
/// `Contents/Resources/`）、应用名、版本号。版本号的回落规则在 `AboutInfo`
/// （`Presentation/`，有单测）—— 这里只把它摆到屏幕上。
///
/// ⚠️ 本文件**只有绑定与布局**（全局约束 8）：没有可断言的值计算，
///    所以它靠手工清单验收（`macos/README.md` 第 17 条）。视图不单测（规格 §10.4）。
struct AboutView: View {
    var body: some View {
        VStack(spacing: 14) {
            logoCard
            Text("Benagen 数据下载工具")
                .font(.title3.weight(.semibold))
            // ⚠️ 前面**不加**"版本"两个字：`AboutInfo` 的兜底文案本身就是一句完整的话
            //    （"版本未知（…）"），加上去会变成"版本 版本未知（…）"。
            Text(AboutInfo.version(in: .main))
                .font(.callout)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)      // 报障时能把版本号原样复制出来
        }
        .padding(28)
        .frame(minWidth: 420)
    }

    // MARK: - 全称 logo

    /// 白底圆角卡片里的横版全称 logo。
    ///
    /// ⚠️ **宽度必须显式约束**：源图是 3451×934 的横版，`.resizable()` 之后不管它，
    ///    它会按原尺寸把窗口撑爆（`.scaledToFit()` 只保证比例，不限制大小）。
    ///
    /// ⚠️ 读不到时**整块不渲染** —— 不是空白卡片、不是占位符、更不是错误横幅：
    ///    logo 是装饰性资产，理由与"判据在打包时"的分工见 `BrandAssets.swift` 顶部那段。
    @ViewBuilder private var logoCard: some View {
        if let logo = Self.logo {
            Image(nsImage: logo)
                .resizable()
                .scaledToFit()
                .frame(width: 360)
                .padding(16)                                   // 白底卡片留白（与图标观感一致）
                .background(RoundedRectangle(cornerRadius: 12).fill(.white))
        }
    }

    /// 进程内只读一次（形状同 `LicenseView.loaded`）：`body` 会因窗口尺寸、主题切换等
    /// 反复求值，每求值一次读一趟盘是白费的 I/O。
    ///
    /// ⚠️ `try?` 在这里**是刻意的**：装饰性资产读不到就不渲染（不报错、不打扰用户）。
    ///    这与 `LicenseText` 那套"读不到必须大声报错"是**有意为之的不同**，
    ///    完整理由见 `BrandAssets.swift` 顶部；判据在打包脚本的存在性自查那一边。
    private static let logo: NSImage? = {
        guard let data = try? BrandAssets.load(named: BrandAssets.fullLogoName) else { return nil }
        return NSImage(data: data)
    }()
}
