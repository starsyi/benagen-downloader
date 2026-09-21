import SwiftUI
import AppKit
import BenagenCoreKit

/// 传输列表（规格 §7.1 / §8.4）：全局速度与活动数 + 任务行 + 六个动作。
///
/// ⚠️ **轮询不在这里**：200 ms 定时器由 `RootView` 按"传输列表可见"启停
///    （简报把这条职责写在 `RootView` 上）。本视图只负责渲染与派发动作。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：
///   - 「内核数据 → 界面值」的纯计算全在 `Presentation/TransferRow.swift`：四列的值、
///     五档状态、动作准入、全局汇总、空态文案、准入闸门 —— **每一条都有单测**；
///   - 这里留下的是**渲染决定**（布局、`RowColor` → `Color`、`NSWorkspace` 那句调用）、
///     以及"点下去调哪个 async 方法"（约束 17：`AppModel` 的请求方法都是 `async`，
///     视图侧 `Task { await … }`）。
///
/// ⚠️ 视图不单测（规格 §10.4）：本文件的行为靠任务 11 的手工清单验收。
struct TransfersView: View {
    @ObservedObject var model: AppModel

    /// 上一次动作失败的**内核原文**（非 nil 时列表上方有一条提示）。
    ///
    /// ⚠️ 动作失败**必须就地可见**（约束 4）：`taskAction` 的抛错路径没有别的落点，
    ///    而"点了没反应"正是本约束要防的形态。原文映射走 `AppModel.message(of:)` ——
    ///    那是 `CoreError` → 用户可见文案的**唯一**实现，本文件不抄第二份。
    @State private var failure: String?

    /// 「在访达中显示」要用的 home。
    ///
    /// ⚠️ **阶段 E 起下载根有两个来源**：用户配过下载目录（`model.downloadDir`，
    ///    经 argv `--download-dir` 交给内核）⇒ 落盘根就是它；没配过 ⇒ 内核用默认值
    ///    `$HOME/Downloads/Benagen`，这时才轮到下面这个 `home`。两者的判断与拼接都在
    ///    `TransferReveal.downloadRoot` / `localPath`（纯函数，有单测，含"为什么不能用
    ///    相对路径"的论证）。**改了目录却没把新值传下去 ⇒ 这颗按钮会指到空处**。
    ///
    /// ⚠️ HOME 的来源**必须与内核一致**（内核读的是 HOME 环境变量）——
    ///    `TransferReveal.home` 里写了为什么不能用 `FileManager.homeDirectoryForCurrentUser`
    ///    （走查实测的分叉：那条路不看 HOME 环境变量）。
    private static let home = TransferReveal.home(
        environment: ProcessInfo.processInfo.environment,
        fallback: FileManager.default.homeDirectoryForCurrentUser.path)

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let failure { failureBar(failure) }
            content
        }
        // 规格 §7.2：列表插入/删除动画。
        .animation(.spring(response: 0.3, dampingFraction: 0.7), value: model.transfers?.items.count)
    }

    // MARK: - 顶部：全局速度与活动数 + 列表级动作

    private var header: some View {
        HStack(spacing: 12) {
            if let global = model.transfers?.global {
                let summary = TransferGlobalSummary.of(global)
                Image(systemName: "arrow.down.circle")
                    .foregroundStyle(.secondary)
                Text(summary.speedText)
                    .font(.callout.weight(.semibold))
                    .monospacedDigit()
                Text(summary.activityText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            } else {
                // 还没有快照：不编数字，也不留一段空白（那会看着像坏了）。
                Text("读取中…")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }

            Spacer(minLength: 8)

            Button {
                Task { await model.refreshTransfers() }
            } label: {
                Label("刷新", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .labelStyle(.titleAndIcon)
            .disabled(!engineAllowsActions)
            .help(engineAllowsActions ? "立刻取一次传输列表快照" : EngineGate.unavailableHelp)

            // 「清空已完成」是**列表级**动作（没有 gid）：行级菜单里没有它，
            // 否则用户以为只清那一行、实际清掉一片。
            Button {
                run(.clearFinished, gid: nil)
            } label: {
                Label("清空已完成", systemImage: "trash")
            }
            .buttonStyle(.borderless)
            .labelStyle(.titleAndIcon)
            // 没有已结束的任务时禁用（判据在 `TransferGlobalSummary.canClearFinished`）：
            // 点了界面毫无变化的话，用户分不清"清完了"还是"没生效"（约束 4）。
            .disabled(!engineAllowsActions || !(model.transfers.map { TransferGlobalSummary.of($0.global).canClearFinished } ?? false))
            .help(engineAllowsActions ? "移除全部已结束的任务" : EngineGate.unavailableHelp)
        }
        .font(.callout)
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
    }

    // MARK: - 列表 / 空态

    @ViewBuilder private var content: some View {
        let rows = (model.transfers?.items ?? []).map(TransferRow.init)
        if !rows.isEmpty {
            list(rows)
        } else if model.transfers == nil && engineAllowsActions {
            // 还没有快照、但引擎能发请求：轮询的第一拍马上就到（`RootView` 的定时器）。
            centered {
                ProgressView()
                    .controlSize(.small)
                Text("正在读取传输列表…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        } else if let text = TransferListEmpty.of(engine: model.engine) {
            // `engine_not_started` 落在这里：**空态文案，不是错误**（简报）。
            centered {
                Image(systemName: "arrow.down.circle")
                    .font(.largeTitle)
                    .foregroundStyle(.tertiary)
                Text(text)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        } else {
            // 引擎不可用且还没有快照：**这里不放转圈**。
            // 一个没有任何请求会返回的转圈正是约束 4 明禁的静默挂死；原因与出口
            // （「重试」）都在主区顶部那条常驻横幅上（`RootView`）。
            Color.clear.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func list(_ rows: [TransferRow]) -> some View {
        List(rows) { row in
            TransferRowView(row: row,
                            engineAllowsActions: engineAllowsActions,
                            onAction: { run($0, gid: row.gid) },
                            onReveal: { reveal(row) })
        }
        .listStyle(.inset)
    }

    private func centered<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 8) { content() }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(20)
    }

    /// 动作失败的提示条：**内核原文逐字**（约束 3）、可选中复制、可收起。
    ///
    /// 形态与 `FileBrowser.failureBar` 同款：失败要**就地**出现，不能只活在别处。
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

    /// 引擎现在能不能发请求（`EngineGate`，有单测）。整套动作的准入都读它。
    private var engineAllowsActions: Bool { EngineGate.allowsRequests(model.engine) }

    // MARK: - 动作

    /// 发一条 `task_action`，成功后**立即刷新一次**（不等下一拍）。
    ///
    /// ⚠️ 后半条是简报点名的：移除/清空之后要能**看见行消失**，否则用户会以为没生效
    ///    （约束 4）。刷新与轮询共用 `AppModel` 里那条单飞纪律（跳拍），所以这里不会
    ///    与下一拍撞车：正在飞的那条不打断，最迟 200 ms 后也会收敛。
    private func run(_ action: TaskAction, gid: String?) {
        Task {
            do {
                try await model.taskAction(action, gid: gid)
                failure = nil
                await model.refreshTransfers()
            } catch {
                // 映射走 `TransferActionFailure.message(of:)`（Kit 侧的公开入口，
                // 内部就是 `AppModel.message(of:)` 那唯一一份实现）—— 视图**不映射**。
                failure = TransferActionFailure.message(of: error)
            }
        }
    }

    /// 「在访达中显示」：**由壳自己做**（规格 §5.2），不走协议。
    ///
    /// 路径由 `TransferReveal.localPath` 把清单相对路径拼到落盘根上（纯函数，有单测）；
    /// 没有路径时菜单项已经是禁用的（`TransferRow.canRevealInFinder`），这里的 `guard`
    /// 是第二道 —— 一个 `nil` 不该变成 `/` 上的一个不存在的文件。
    private func reveal(_ row: TransferRow) {
        guard let path = TransferReveal.localPath(manifestPath: row.manifestPath,
                                                  home: Self.home,
                                                  downloadDir: model.downloadDir) else {
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    /// `RowColor` → SwiftUI 的 `Color`（同 `TransferRowView.tint` 的理由：纯渲染决定）。
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
