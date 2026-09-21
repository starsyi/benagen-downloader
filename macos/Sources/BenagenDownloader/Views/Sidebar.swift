import SwiftUI
import BenagenCoreKit

// ---------------------------------------------------------------------------
// 侧边栏分区
// ---------------------------------------------------------------------------

/// 侧边栏的三个分区。
///
/// ⚠️ **成员由任务 4 定义，后续任务只引用、不改**（任务 5 填批次摘要、任务 9 加未完成计数
///    徽标，都不得增删这里的 case）。三个 case 与规格 §7.1 的信息架构一一对应：
///    文件 / 传输列表 / 校验结果。
///
/// 注意**没有** `log` 这类分区：规格 §14.3 定稿不做独立日志页 —— 独立日志页恰好是把失败
/// 藏进另一个标签，与规格 §9「任何一类失败都必须出现在界面上」相反。错误一律**就地**显示
///（加载失败在空态页、下载失败在传输列表的行里、引擎问题在工具栏徽标 + 传输列表顶部横幅）。
public enum SidebarSection: String, CaseIterable, Hashable {
    case files, transfers, verify

    var title: String {
        switch self {
        case .files: return "文件"
        case .transfers: return "传输列表"
        case .verify: return "校验结果"
        }
    }

    var systemImage: String {
        switch self {
        case .files: return "folder"
        case .transfers: return "arrow.down.circle"
        case .verify: return "checkmark.seal"
        }
    }
}

// ---------------------------------------------------------------------------
// 侧边栏视图
// ---------------------------------------------------------------------------

struct Sidebar: View {
    @Binding var selection: SidebarSection

    /// ⚠️ 本任务还用不上它：任务 5（底部批次摘要的真值）与任务 9（未完成计数徽标）都要读
    ///    `model` —— 这里先把线接好，省得那两个任务各自改一次 `RootView` 的调用点。
    @ObservedObject var model: AppModel

    var body: some View {
        List(selection: selectionBinding) {
            ForEach(SidebarSection.allCases, id: \.self) { section in
                Label(section.title, systemImage: section.systemImage)
                    .badge(unfinishedCount(for: section))
                    .tag(section)
            }
        }
        // 规格 §7.2：侧边栏 220–280px。**不要手工叠加 NSVisualEffectView** ——
        // NavigationSplitView 的 sidebar 列自带系统材质，叠一层会得到双层材质、观感发灰。
        .navigationSplitViewColumnWidth(min: 220, ideal: 240, max: 280)
        .safeAreaInset(edge: .bottom, spacing: 0) { batchSummary }
    }

    /// `List` 的单选绑定是 `SelectionValue?`，而 `RootView` 持的是非可选的 `SidebarSection`
    /// （语义是「总有一个分区被选中」）。`nil`（用户点了列表空白处）按「不变」处理。
    private var selectionBinding: Binding<SidebarSection?> {
        Binding(get: { selection }, set: { if let picked = $0 { selection = picked } })
    }

    /// 未完成计数徽标 —— 任务 9 接上真值。
    ///
    /// ⚠️ SwiftUI 对 `.badge(0)` **什么都不渲染**：0 在侧边栏里的呈现在就是"这个分区
    ///    没有未落定的东西"。**这不违反约束 4 的"计数为 0 也不得隐藏"** —— 那一条管的是
    ///    `VerifyView` 里的**六个分区**（它们每一类都画出「0 项」，有单测钉着）；
    ///    侧边栏这个徽标是导航上的角标，规格 §7.1 只点名了传输列表那一个。
    ///
    /// 两个数都是"**没落定的那些**"（口径在 `SidebarBadge`，有单测）：传输列表不含
    /// 已完成 / 已移除，校验结果不含 `unverifiable`（内核的 `all_good` 不含它）。
    /// 徽标挂着一个数与内容区的话自相矛盾，比不挂徽标更糟。
    ///
    /// 文件分区不挂徽标：未完成量由传输列表那一个数表达，规格 §7.1 也只点名了传输列表。
    private func unfinishedCount(for section: SidebarSection) -> Int {
        switch section {
        case .files: return 0
        case .transfers: return SidebarBadge.unfinishedTransfers(model.transfers)
        case .verify: return SidebarBadge.unpassed(model.verify)
        }
    }

    /// 底部常驻批次摘要：批次号 / 文件数 / 总大小 / 有效期（规格 §7.1）。
    ///
    /// ⚠️ 四个值全部来自 `DeliverySummary`（`Presentation/`，有单测）—— 本文件**不拼字符串、
    ///    不格式化、不判断过期**（全局约束 8）：这里只有绑定与布局，连 `HStack` 里那个
    ///    `·` 都只是一段静态文本（与 `SidebarSection.title` 同类）。
    @ViewBuilder private var batchSummary: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("批次摘要")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)

            if let summary = DeliverySummary.of(model.loadState) {
                HStack(spacing: 4) {
                    Text(summary.code)
                        .font(.caption2.weight(.semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    if let badge = summary.expiredBadgeText {
                        Text(badge)
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.red)
                    }
                }
                HStack(spacing: 4) {
                    Text(summary.filesText)
                    Text("·")
                    Text(summary.sizeText)
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
                Text(summary.validityText)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            } else {
                Text("尚未加载交付批次")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }

            // 总进度（任务 9）：`get_tree` 的 `progress`，与校验视图顶部**同一个**
            // `ProgressSummary`（同一份口径，只算一处）。侧边栏是常驻的，所以这里给的是
            // 最省的形态：一条进度条 + 百分比；字节与速度在 `VerifyView` 那一行里。
            //
            // ⚠️ 树**必须属于当前这一批**（判据在 `ProgressSummary.of`，有单测）：
            //    `AppModel` 先落 `loadState = .loaded`、之后才拉 `get_tree`，把上一批的
            //    进度摆在这一批的批次号底下是一个没有任何提示的错数。
            if let progress = progressSummary {
                Divider().padding(.top, 4)
                HStack(spacing: 6) {
                    ProgressView(value: progress.fraction)
                        .progressViewStyle(.linear)
                    Text(progress.percentText)
                        .font(.caption2.weight(.semibold))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .overlay(alignment: .top) { Divider() }
        // 换批次/过期时换一批字，别硬切。
        .animation(.spring(response: 0.3, dampingFraction: 0.7), value: model.loadState)
    }

    /// 总进度（`get_tree` 的 `progress`）；树还没到、或还不是这一批的 → `nil`。
    ///
    /// 三个值全部交给 `ProgressSummary`（`Presentation/`，有单测）—— 本文件不判断、
    /// 不格式化、不自己夹进度（全局约束 8）。
    private var progressSummary: ProgressSummary? {
        guard case .loaded(let info) = model.loadState else { return nil }
        return ProgressSummary.of(tree: model.tree, treeCode: model.treeCode, code: info.code)
    }
}
