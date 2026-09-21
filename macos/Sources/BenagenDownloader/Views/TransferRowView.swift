import SwiftUI
import BenagenCoreKit

/// 传输列表里的一行（规格 §7.1 的四列：文件名/相对路径 · 进度 · 速度 · 状态）。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：
///   - 「内核数据 → 界面值」的纯计算全在 `Presentation/TransferRow.swift`：四列的文字、
///     进度分数（含总量为 0 与超报的夹取）、五档状态的标签与颜色、「已暂停」角标的判据
///     （只能来自 `raw_status`，裁决 #88）、错误原文、以及**这一行允许哪些动作** ——
///     **每一条都有单测**；
///   - 这里留下的是**渲染决定**：`RowColor` → `Color`（同 `EngineStatusBadge.tint`）、
///     布局、SF Symbol 名、以及"点下去调哪个回调"。
///
/// ⚠️ 视图不单测（规格 §10.4）：本文件的行为靠任务 11 的手工清单验收。
struct TransferRowView: View {
    let row: TransferRow

    /// 引擎现在能不能发请求。`false` 时**每一个动作都禁用**（约束 4/15）。
    ///
    /// ⚠️ 判据来自 `EngineGate.allowsRequests`（有单测），不是这里现编的：
    ///    内核卡死时 `CoreClient` 的 FIFO 队列已被那条永不返回的请求永久堵死，
    ///    点下去只会得到一颗再也回不了话的按钮 —— **静默挂死**是约束 4 明禁的形态。
    let engineAllowsActions: Bool

    /// 行级动作（暂停 / 继续 / 重试 / 移除）。`clear_finished` **不在这里**：它没有 gid、
    /// 是列表级动作，混进行级菜单会让用户以为只清这一行。
    let onAction: (TaskAction) -> Void

    /// 「在访达中显示」。**由壳自己做**（`NSWorkspace`），不走协议 ——
    /// 实现落在 `TransfersView`（那里才有 home 与 AppKit），本视图只负责那一项**什么时候可点**。
    let onReveal: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: stateIcon)
                .foregroundStyle(tint(row.stateColor))
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 4) {
                titleLine
                progressLine
                // 错误原文**全文**显示在行下方（简报）：不截断、不折叠。
                // 多行原文（aria2 的 errorMessage 里会有换行）原样展开。
                if let errorText = row.errorText {
                    Text(errorText)
                        .font(.caption)
                        .foregroundStyle(tint(.red))
                        .textSelection(.enabled)
                        // 不设 lineLimit ⇒ 不截断；fixedSize 让它按内容撑开高度，
                        // 而不是被这一行的布局压成一行。
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }

            Text(row.speedText)
                .font(.caption)
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 88, alignment: .trailing)

            Text(row.stateLabel)
                .font(.caption)
                .foregroundStyle(tint(row.stateColor))
                .frame(width: 64, alignment: .leading)

            actionsMenu
        }
        .padding(.vertical, 2)
        .contentShape(Rectangle())
        // 同一份菜单挂两份：`Menu` 是看得见的入口，右键菜单是习惯动作。
        .contextMenu { actionItems }
        .help(row.manifestPath ?? row.title)
    }

    // MARK: - 名称 / 角标

    private var titleLine: some View {
        HStack(spacing: 6) {
            // ⚠️ 原文直出：文件名里的 `×` 与空格由 `Text` 原样渲染（约束 3）。
            // 长路径在行里截断只影响排版，完整原文在 `.help` 里。
            Text(row.title)
                .lineLimit(1)
                .truncationMode(.middle)
            if row.showsPausedBadge {
                // 「已暂停」角标：`state` 里 paused 与 waiting 长得一模一样，
                // 这个角标是两者在界面上唯一的区别（裁决 #88 / 规格 §8.4）。
                Text("已暂停")
                    .font(.caption2.weight(.semibold))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(.quaternary.opacity(0.6)))
                    .foregroundStyle(.secondary)
            }
        }
    }

    // MARK: - 进度

    private var progressLine: some View {
        HStack(spacing: 8) {
            // 值恒在 0...1（`TransferRow.progressFraction`），不会出现 NaN 或超格。
            ProgressView(value: row.progressFraction)
                .progressViewStyle(.linear)
                .frame(width: 150)
            Text(row.progressText)
                .font(.caption)
                .monospacedDigit()
                .foregroundStyle(.secondary)
            Text(row.percentText)
                .font(.caption)
                .monospacedDigit()
                .foregroundStyle(row.stateColor == .red ? tint(.red) : .secondary)
        }
    }

    // MARK: - 动作

    private var actionsMenu: some View {
        Menu {
            actionItems
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(engineAllowsActions ? "这一行的操作" : EngineGate.unavailableHelp)
    }

    /// 行级菜单项。**禁用而不是"点了没反应"**（简报）：
    /// 每一项的可用性都来自 `TransferRow.availableActions`（有单测），并且整体再与引擎闸门相与。
    @ViewBuilder private var actionItems: some View {
        Button("暂停") { onAction(.pause) }
            .disabled(!canAct(.pause))
        Button("继续") { onAction(.unpause) }
            .disabled(!canAct(.unpause))
        Button("重试") { onAction(.retry) }
            .disabled(!canAct(.retry))

        Divider()

        // ⚠️ 「在访达中显示」**不走协议**（规格 §5.2：内核不能知道"访达"这个概念，
        //    否则 Windows 壳就要面对一个叫 reveal 却要打开资源管理器的方法）。
        //    `path == nil` 时**禁用**：没有路径就没有文件可定位，点下去什么都不发生
        //    是简报点名不要的静默无效。
        Button("在访达中显示") { onReveal() }
            .disabled(!row.canRevealInFinder)

        Divider()

        Button("移除", role: .destructive) { onAction(.remove) }
            .disabled(!canAct(.remove))
    }

    /// 这一项现在能不能点：**行自己的准入** 且 **引擎能给出发得出去的请求**。
    private func canAct(_ action: TaskAction) -> Bool {
        engineAllowsActions && row.availableActions.contains(action)
    }

    /// 状态 → SF Symbol 名。名字来自 `TransferRow.stateIconName`（呈现层，有单测钉住
    /// 五档两两不同），这里只做渲染。
    private var stateIcon: String { row.stateIconName }

    /// `RowColor` → SwiftUI 的 `Color`。
    ///
    /// ⚠️ 这一步**故意留在视图里**（同 `EngineStatusBadge.tint` / `FileBrowser.tint`）：
    ///    它是纯渲染决定、断言不出有意义的东西，而 `Presentation/` 不为了它引入 SwiftUI 依赖。
    ///    "哪个状态配哪个语义色"这件事本身已经在 `TransferRow.color` 里定好并有单测。
    private func tint(_ c: RowColor) -> Color {
        switch c {
        case .secondary: return .secondary
        case .blue: return .blue
        case .green: return .green
        case .red: return .red
        case .orange: return .orange
        }
    }
}
