import SwiftUI
import BenagenCoreKit

/// 开源许可：**GPLv2 全文**。
///
/// ⚠️ 这一屏不是装饰，是**法律义务**：应用内嵌 aria2（GPLv2），分发它就要一并给出
///    许可全文与展示入口（规格 §11"GPL 义务随 aria2 内嵌一并沿用"）。
///    全文本体由打包脚本拷进 `Contents/Resources/COPYING-GPLv2.txt`。
///
/// ⚠️ **读不到时不得显示空白页**：空白页既没履行义务，也不会有人去查它 ——
///    所以失败分支是一句说得清"出了什么事、该找谁"的话（`LicenseError.message`，
///    三支各有各的处置）。判据在 `LicenseTextTests`（`LicenseText` 是
///    `Presentation/` 里的纯映射，有单测）。
///
/// ⚠️ 本文件**只有绑定**（全局约束 8）：资源查找、读取、空文件判定、错误文案
///    全在 `LicenseText` 里；这里只把那个 `Result` 摆到屏幕上。
struct LicenseView: View {
    @Environment(\.dismiss) private var dismiss

    /// 进程内只读一次。GPLv2 全文中约 18 KB，而 `body` 会因为滚动、窗口尺寸等
    /// 反复求值 —— 每求值一次读一趟盘是白费的 I/O。
    ///
    /// ⚠️ 缓存的是 `Result` **整个**（含失败）：失败了就一直是那句错误，
    ///    不会出现"这一帧有内容、下一帧空白"的抖动。
    private static let loaded: Result<String, LicenseError> = {
        do {
            return .success(try LicenseText.load(from: LicenseText.resourceURL()))
        } catch let e as LicenseError {
            return .failure(e)
        } catch {
            // 兜底：`LicenseText` 只抛 `LicenseError`，真落到这里也得是一句**看得见**的话
            // （这条路的语义就是"绝不静默空白"）。
            return .failure(.unreadable("\(error)"))
        }
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            switch Self.loaded {
            case .success(let text):
                fullText(text)
            case .failure(let error):
                failure(error)
            }
        }
        // ⚠️ **高度上限是这一行的全部要点**（2026-09-20 现场修复）。
        //
        //    本视图是经 `.sheet` 呈现的（`SettingsFooter` 的「开源许可…」），
        //    而 **sheet 的大小由内容的理想尺寸决定**；下面 `fullText` 里是一个 `ScrollView`
        //    —— `ScrollView` 的理想高度就是**它内容的实际高度**，也就是那份约 18 KB 的
        //    GPLv2 全文排下来的高度。原来这一行只写了 `minWidth: 640, minHeight: 520`
        //    （**只有下限**），于是没有任何东西拦它 ⇒ 客户看到的是
        //    **弹窗占满整个屏幕的高度**（原话："当前的高度占用了整个屏幕的高度"）。
        //
        //    ⇒ 必须给**上限**：`maxHeight` 才是那条拦住它的线。
        //    `idealHeight` 与 `maxHeight` 取**同一个值**是刻意的：两者不同值时
        //    "最终用哪个"由 SwiftUI 的提议过程决定，那是个不值得赌的细节。
        //
        //    数值照 `SettingsView` 那扇窗口的下界（520×460）取 —— 这条 sheet 挂在
        //    **设置窗口**上，不该比它大（客户的判据原话："不应该超过设置的窗口大小"）。
        //    正文是可滚动的，所以矮一点只是多滚两下，不会少字（GPLv2 全文一个字都不许少）。
        .frame(minWidth: 480, idealWidth: 520, maxWidth: 560,
               minHeight: 320, idealHeight: 460, maxHeight: 460)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text("开源许可")
                    .font(.title3.weight(.semibold))
                Spacer(minLength: 8)
                Button("关闭") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            // 说清楚"为什么这一页在这里"：内嵌的 aria2 是 GPLv2。
            Text("本应用内嵌 aria2（GPLv2 许可），因此随附下列许可全文。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    /// 全文（**逐字**，只做排版：等宽字体 + 可选中复制，不截断、不重排文字）。
    private func fullText(_ text: String) -> some View {
        ScrollView {
            Text(text)
                .font(.system(size: 12, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(20)
        }
    }

    /// 读不到全文。**这一支绝不返回空白**：图标 + 一句标题 + 明确的错误原文
    /// （`LicenseError.message` 里带着文件名与"该怎么办"）。
    private func failure(_ error: LicenseError) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.largeTitle)
                .foregroundStyle(.red)
            Text("GPLv2 全文没能读出来")
                .font(.headline)
            Text(error.message)
                .font(.callout)
                .textSelection(.enabled)          // 原文可复制（能拿这句去问分发方）
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}
