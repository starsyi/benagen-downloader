import SwiftUI
import BenagenCoreKit

/// 校验结果（规格 §7.1 / §14.3）：顶部总进度 + 一行总结 + **六个分区**。
///
/// ⚠️ **这一屏是约束 4「六类结果互斥穷尽」唯一的落地处**，所以它的硬要求是
///    「**计数为 0 的类也要渲染**」—— 空的类显示「0 项」，不隐藏、不省略。
///    六类的标签/颜色/图标、计数、路径全部来自 `VerifySummary`（`Presentation/`，有单测）。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：不拼字符串、不做算术、
///    不判断哪一类算失败、不映射 `CoreError`（映射走 `VerifyRefreshFailure.message(of:)`）。
///
/// ⚠️ 错误往哪摆（**与顶部横幅不互相遮蔽**）：
///   - `refreshVerify()` 自己的失败**不在这里显示** —— 它是非抛错路径，原文落在
///     `model.lastError` 上，由 `RootView` 主区顶部那条常驻横幅显示（`EngineBanner`）。
///     同一句原文显示两遍，只会让真正要看的那条变淡（同 `TransferListEmpty` 的理由）；
///   - `getTree()` 的**抛错**路径没有别的落点（约束 4），所以这里有一条自己的失败条
///     （`failure`），位置在六类列表**正上方**、与顶部横幅是两条独立的行，谁也盖不住谁。
///     引擎不可用时**不显示它**：那时顶部横幅已经在说同一句话。
///   - 刷新按钮与自动刷新都受 `EngineGate`（界面侧准入）约束，闸门本身还有
///     `AppModel.refreshVerify` 里那条结构性防线（引擎不可用时一个请求都不发）。
struct VerifyView: View {
    @ObservedObject var model: AppModel

    /// 一次刷新是否在飞（用于禁用按钮，避免点两下排两条）。
    @State private var refreshing = false
    /// 刷新时 `get_tree` 抛错的**内核原文**（非 nil 时列表上方有一条提示）。
    @State private var failure: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let failure { failureBar(failure) }
            content
        }
        // 切到本视图时刷新一次（简报）：视图由 `RootView` 的分区 `switch` 构造，
        // 每次切进来都是一次新的出现 ⇒ 这条 `.task` 就会再跑一次。
        .task { await refresh() }
    }

    // MARK: - 顶部：总进度 + 总结 + 刷新

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("总进度")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                if let progress = progressSummary {
                    // 进度条 + 三个数：百分比 / 已下载 / 速度（全部来自 `ProgressSummary`）。
                    ProgressView(value: progress.fraction)
                        .progressViewStyle(.linear)
                        .frame(width: 160)
                    Text(progress.percentText)
                        .font(.callout.weight(.semibold))
                        .monospacedDigit()
                    Text(progress.bytesText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                    Text(progress.speedText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                } else {
                    // 树还没到（或还不是这一批的）：**说「—」**，不留空白、不编数字
                    // （口径同 `SpeedFormat`/`PercentFormat` 的占位符）。
                    Text("—")
                        .font(.callout)
                        .foregroundStyle(.tertiary)
                }

                Spacer(minLength: 8)

                Button {
                    Task { await refresh() }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                // ⚠️ 这一行不是装饰：macOS 的统一工具栏默认把 `Label` 渲染成只有图标，
                //    而这条提示条不在工具栏里，留着它是为了与 `RootView` / `TransfersView`
                //    里那两处同款（标题真的画出来，别掉进 `.help` 浮层）。
                .labelStyle(.titleAndIcon)
                .disabled(refreshing || !engineAllowsActions)
                // ⚠️ 文案原先写的是「…（立刻跑一次真实校验）」，那句话**与内核行为不符**：
                //    `verify_status` 是**纯读**（`op_verify_status` 只取 `&k.verify`，
                //    不触发任何文件校验），校验由内核的 `spawn_landing_watcher` 按 200ms
                //    一拍**自动**推进。按旧文案理解，客户会以为"点一下才会校验"，
                //    而事实是"点不点都在校验，这里只是取一次当前结果"。
                .help(engineAllowsActions ? "重新取一次内核当前的校验结果与总进度"
                                          : EngineGate.unavailableHelp)
            }

            if let summary = verifySummary {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: summary.headlineIcon)
                        .foregroundStyle(tint(summary.headlineColor))
                    Text(summary.headline)
                        .font(.title3.weight(.semibold))
                        .textSelection(.enabled)
                    Text(summary.classifiedText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
            } else {
                Text("还没有校验结果")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
    }

    // MARK: - 六类

    @ViewBuilder private var content: some View {
        if let summary = verifySummary {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    // **恒六条**（含计数为 0 的类）—— 顺序由 `VerifySummary` 钉住。
                    ForEach(summary.rows) { row in
                        classBlock(row)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            // 还没取过校验结果：**不画六个 0**（那会读成"全部通过"，而这句话谁都没说过）。
            // 底部那句是"下一步做什么"，不是错误 —— 所以没有红色、没有惊叹号。
            VStack(spacing: 8) {
                Image(systemName: "checkmark.seal")
                    .font(.largeTitle)
                    .foregroundStyle(.tertiary)
                Text("还没有校验结果")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Text(engineAllowsActions ? "点右上角「刷新」取一次" : EngineGate.unavailableHelp)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(20)
        }
    }

    /// 一类：标签 + 计数 +（说明）+ 这一类自己的路径。
    ///
    /// ⚠️ **计数与路径都照登**：计数为 0 的类照常画出「0 项」（约束 4），
    ///    路径是内核原文、可选中复制（约束 3），不做任何截断与规范化。
    private func classBlock(_ row: VerifyClassRow) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: row.iconName)
                    .foregroundStyle(tint(row.color))
                Text(row.label)
                    .font(.callout.weight(.semibold))
                Text(row.countText)
                    .font(.callout)
                    .monospacedDigit()
                    .foregroundStyle(row.count == 0 ? .tertiary : .secondary)
                Spacer(minLength: 0)
            }
            if let note = row.note {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // 用下标当 id：六类互斥保证同一类的路径不重复，但一份**畸形**的结果不该
            // 让 `ForEach` 报重复 id（那是壳自己造出来的噪音，不是内核的问题）。
            ForEach(Array(row.paths.enumerated()), id: \.offset) { _, path in
                Text(path)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)      // 约束 3：原文可复制
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.leading, 24)
            }
            Divider().padding(.top, 6)
        }
        .padding(.top, 8)
    }

    /// 刷新失败的提示条：**内核原文逐字**（约束 3）、可选中复制、可收起。
    /// 形态与 `TransfersView.failureBar` / `FileBrowser` 同款。
    private func failureBar(_ message: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(tint(.red))
            Text(message)
                .font(.caption)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            Button {
                failure = nil
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("收起这条提示")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .background(.quaternary.opacity(0.4))
    }

    // MARK: - 取值

    /// 六类 + 总结的界面值（`nil` = 还没取过校验结果）。
    private var verifySummary: VerifySummary? { VerifySummary.of(model.verify) }

    /// 总进度（`get_tree` 的 `progress`）。树不属于这一批时是 `nil`（见 `ProgressSummary.of`）。
    private var progressSummary: ProgressSummary? {
        guard case .loaded(let info) = model.loadState else { return nil }
        return ProgressSummary.of(tree: model.tree, treeCode: model.treeCode, code: info.code)
    }

    /// 引擎现在能不能发请求（`EngineGate`，有单测）。刷新按钮与自动刷新都读它。
    private var engineAllowsActions: Bool { EngineGate.allowsRequests(model.engine) }

    // MARK: - 刷新

    /// 刷一次：**校验结果 + 总进度一起**。
    ///
    /// ⚠️ 只刷一半会留下一个"看起来刷新了、其中一格没动"的屏幕 —— 而这一屏上
    ///    总进度与六类是同一次交付的两个侧面，客户按「刷新」的意思就是"现在怎么样"。
    ///
    /// ⚠️ 两条请求的失败**落点不同**，这是有意的：
    ///   - `refreshVerify()` 不抛错：原文落 `model.lastError` → 顶部常驻横幅（`EngineBanner`）；
    ///   - `getTree()` 抛错：没有别的落点，**就地**显示（约束 4）。
    ///     引擎不可用时**不显示它** —— 那一刻顶部横幅正在说同一句话，重复只会让
    ///     真正要看的那条变淡（`refreshVerify` 的闸门也会让它快速失败）。
    private func refresh() async {
        guard engineAllowsActions else { return }
        refreshing = true
        defer { refreshing = false }

        await model.refreshVerify()
        do {
            _ = try await model.getTree()
            failure = nil
        } catch {
            failure = engineAllowsActions ? VerifyRefreshFailure.message(of: error) : nil
        }
    }

    /// `RowColor` → SwiftUI 的 `Color`（同 `TransferRowView.tint` / `TransfersView.tint` 的理由：
    /// 纯渲染决定、断言不出有意义的东西，所以它留在视图里而不是 `Presentation/`）。
    ///
    /// ⚠️ 这里**没有 `default`**：`RowColor` 加成员时这个 `switch` 会编译不过 —— 那是刻意的。
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
