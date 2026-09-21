import SwiftUI
import BenagenCoreKit

/// 底部状态栏（规格 §7.1：「已选 N 项 · 合计大小 · 操作按钮」）。
///
/// ⚠️ 本文件**只有绑定与布局**（全局约束 8）：那两个数（项数、合计大小）都是
///    `SelectionSummary` 算好的字符串，这里只负责把它们排在一行 —— 不拼字符串、
///    不格式化字节、不去 `flat` 里查大小。连中间那个 `·` 都只是一段静态文本
///    （与 `Sidebar.batchSummary` 里那个同形）。
///
/// ⚠️ **有意偏离规格 §7.2 的一处（约束 11：有意偏离必须写明理由）**：
///    §7.2 要求主按钮「`borderedProminent` + `.keyboardShortcut(.defaultAction)`」，
///    本文件只做了前半 —— 「下载选中」**不绑回车**。
///    理由：这颗按钮在**文件浏览器**里，而同一个窗口里 Return 的默认含义是"打开选中项"
///    （访达/网盘的通用语义，也正是双击在做的事）。给「下载选中」绑上回车之后，
///    用户在列表里按回车的后果是**立刻开始下载一批文件**——一个不可撤销、要跑几小时的
///    网络动作，而它与"打开/进入"的肌肉记忆正好相反。
///    规格 §7.2 那条规则来自**表单页**（空态页的「加载」按钮绑了 `.defaultAction`，
///    那里 Return 没有第二种含义），把它照搬到浏览器上会制造这个冲突。
///    所以这里的取舍是：**回车留给"打开"，下载必须是一次显式的点击**。
///
/// ⚠️ 视图不单测（规格 §10.4）。
struct SelectionBar: View {
    let summary: SelectionSummary
    /// 按钮文案。**空勾选时是「全部下载」而不是禁用**（任务 7）：那时发出去的是
    /// `paths: []`（内核语义 = 下全部待下载），按下去确实会发生一件事 ——
    /// 所以"按下去什么都不会发生"这条禁用理由不成立了。文案由
    /// `DownloadTargets.buttonTitle(for:)` 算（纯函数，有单测），这里只摆。
    let title: String
    /// 一项都没勾时那句**明说**（「未勾选任何项，将下载全部待下载文件」），其余时候 `nil`。
    /// 文案由 `DownloadTargets.emptySelectionHint` 算（纯值，有单测），这里只摆。
    ///
    /// ⚠️ 它不是装饰：那时按钮写的是「全部下载」，而**不勾 = 下全部**这件事
    ///    没有任何别的地方说出口 —— 用户以为"我什么都没选，按下去应该什么都没发生"。
    let hint: String?
    /// 「全选」= 勾上**整批的全部文件**（`flat`，不含目录）。全选的规则在
    /// `BrowserSelection.allFiles`（纯函数，有单测），这里只转发。
    let onSelectAll: () -> Void
    /// 「全不选」。就是清空勾选面 —— 清空之后发出去的是 `paths: []`（下全部），
    /// 所以底栏那句话会跟着变成 `hint`（两个状态在界面上分得出来）。
    let onClearSelection: () -> Void
    let onDownload: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            // 提示在**最左边、项数之前**（它解释的是"接下来会发生什么"，
            // 用户从左往右读到的第一句就该是它）。
            if let hint {
                Text(hint)
                    .foregroundStyle(.secondary)
                Text("·")
                    .foregroundStyle(.tertiary)
            }
            Text(summary.countText)
                .foregroundStyle(.secondary)
            Text("·")
                .foregroundStyle(.tertiary)
            Text(summary.sizeText)
                .foregroundStyle(.secondary)
                .monospacedDigit()

            Spacer(minLength: 12)

            // ⚠️ `.borderless`：默认样式在底栏里会变成两颗大按钮，把主按钮挤到边上，
            //    而"全选 / 全不选"是辅助动作、不是这一栏的主角。
            Button("全选", action: onSelectAll)
                .buttonStyle(.borderless)
                .help("勾上整批的全部文件（不含目录）")
            Button("全不选", action: onClearSelection)
                .buttonStyle(.borderless)
                .help("清空勾选面")

            // ⚠️ 任务 7 已接线：`onDownload` 由 `FileBrowser` 转发给 `RootView` 的 `download(_:)`
            //    ——那是本动作的**唯一实现**（工具栏那颗按钮走的是同一条路）。
            Button(title, action: onDownload)
                .buttonStyle(.borderedProminent)
                // 悬停提示与工具栏共用一份（`DownloadTargets.helpText`）：
                // 两个入口各写一句，改一处就会出现"同一个按钮两个说法"。
                .help(DownloadTargets.helpText)
        }
        .font(.callout)
        .lineLimit(1)
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
    }
}
