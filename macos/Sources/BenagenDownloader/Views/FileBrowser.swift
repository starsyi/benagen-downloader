import SwiftUI
import BenagenCoreKit

/// 文件浏览（规格 §7.1）：**平铺列表 + 双击进入 + 面包屑导航**（网盘形态，非树形展开）。
///
/// 为什么是平铺而不是 `OutlineGroup`：清单里有 `C24-8_×_25WS024/Figure/QC 图.png`
/// 这类路径，树形展开会把行挤窄、恰好弱化"看得到目录结构"这个原始诉求（规格 §7.1 定稿）。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：
///   - 「内核数据 → 界面值」的纯计算全在 `Presentation/`：层级链与父路径（`Breadcrumb`）、
///     行的四列与四态（`BrowserRow` / `SourceTimeText` / `RowStateStyle`）、`path_not_found` 的回退
///     （`DirLoadFailure`）、默认选中面与全选（`BrowserSelection`）、底部汇总
///     （`SelectionSummary`）—— **每一条都有单测**；
///   - 这里留下的是**渲染决定**：枚举 → `Color`（同 `EngineStatusBadge.tint`）、
///     布局、以及调哪个 async 方法（约束 17：`AppModel` 的请求方法都是 `async`，
///     视图侧 `Task { await … }` / `.task { await … }`）。
///
/// ⚠️ 视图不单测（规格 §10.4）：本文件的行为靠任务 11 的手工清单验收。
struct FileBrowser: View {
    @ObservedObject var model: AppModel

    // -----------------------------------------------------------------------
    // ⚠️ 下面三个状态**由 `RootView` 持有**（`@Binding` 而非 `@State`），理由：
    //
    //    `RootView.mainArea` 是按 `section` 分派的 `switch` —— 切到「传输列表」再切回来，
    //    这个视图会被**重新构造**，`@State` 跟着重建。而切分区**不是换批次**：
    //    简报只允许"换交付码时清空选择"，可"选完文件去看一眼传输列表再回来"恰恰是这个
    //    应用的主流程 —— 留在 `@State` 里的后果是用户的选择与当前目录被**无声地**打回默认面。
    //
    //    为什么不放进 `AppModel`：这三个都是**纯视图状态**（用户在看哪一层、勾了哪些），
    //    不是内核数据的搬运结果。放进模型等于让模型持有界面位置，那才是真正的约束 8 越界
    //    （模型会开始长出"面包屑""选择"这类没有内核对应物的字段）。
    //    `RootView` 本来就已经持有 `section`（同一个性质的状态），放它那里最顺。
    //
    //    `entries` / `phase` / `failure` **仍然留在这里**：它们是"这一帧的加载结果"，
    //    切回来重读一次就能重建，没有"被无声重置"的问题。
    // -----------------------------------------------------------------------

    /// 当前层（清单**原文**路径；根是空串）。壳不规范化它（约束 3）。
    @Binding var currentPath: String
    /// 勾选面：**路径**的集合（跨目录多选，进入目录时不清空）。
    ///
    /// ⚠️ "进目录时不清空"有**一个例外**：被**进入的那个目录自己**要摘掉
    ///    （`BrowserSelection.afterEntering`）—— 那是双击这个动作的副作用写进来的，
    ///    不是用户勾的。留下它的后果是"按下载选中会静默多下整个目录"，见那里的注释。
    @Binding var selection: Set<String>
    /// 与"是哪一批清单"有关的**全部**状态（复位的对照值 + 播种的记账）—— 由 `RootView` 持有
    /// （理由与上面那三个状态相同：换批只发生在模型那一层）。
    ///
    /// ⚠️ 两个动作分开调，别混：**换批那一刻**由 `RootView` 调 `display(code:)`（复位），
    ///    **这一批的树到位那一刻**由这里的 `enter()` 调 `seed(code:treeCode:tree:)`（播种）。
    ///    两个字段的语义为什么必须分开，见 `ManifestTracking` 的注释。
    @Binding var manifest: ManifestTracking

    /// 加入下载。**本视图不自己调 `model.enqueue`** —— 动作的唯一实现在 `RootView.download(_:)`
    /// （工具栏那颗「下载选中」/「全部下载」走的是同一条路）：那里才有"切到传输列表"
    /// （`section` 是 `RootView` 的状态）与"逐条显示拒绝理由"那两个落点，
    /// 两处各写一份迟早会分叉成"工具栏下了、底栏没下"。
    ///
    /// 传的是**路径集合**：双击文件传 `[该文件的 path]`，底部那颗按钮传整个勾选面。
    let onDownload: (Set<String>) -> Void

    /// 当前层的条目（内核原样；行值由 `BrowserRow` 算）。
    @State private var entries: [DirEntry] = []
    /// 一次 `list_dir` 的生命周期。四态都要有**可见**的呈现（简报）。
    @State private var phase: Phase = .idle
    /// 上一次失败的原文与回退落点（非 nil 时列表上方有一条提示）。
    @State private var failure: DirLoadFailure?

    private enum Phase: Equatable { case idle, loading, loaded, failed }

    // MARK: - 主体

    var body: some View {
        VStack(spacing: 0) {
            breadcrumbBar
            Divider()
            if let failure { failureBar(failure) }
            content
            Divider()
            SelectionBar(
                summary: SelectionSummary.of(selected: selection,
                                             sizes: SelectionSummary.sizeIndex(model.tree?.flat ?? [])),
                // 文案由 `DownloadTargets` 算：**没有勾选时是「全部下载」**（发 `paths: []`）。
                title: DownloadTargets.buttonTitle(for: selection),
                // 一项都没勾时**明说**会发生什么（不勾 = 下全部待下载）。不让用户猜 ——
                // 一颗写着「全部下载」的按钮旁边没有这句话，谁都想不到"不勾也是下全部"。
                hint: selection.isEmpty ? DownloadTargets.emptySelectionHint : nil,
                // ⚠️ 底栏这颗「全选」勾的是**整批的全部文件**（`get_tree` 的 `flat`），
                //    不是"这一层" —— 那正是"覆盖整批 ⇒ `paths: []`"（约束 C-3）的入口。
                //    「全选本层」（含目录）是 ⌘A 与右键菜单那条，走 `BrowserSelection.all(in:)`。
                onSelectAll: { selection = BrowserSelection.allFiles(in: model.tree?.flat ?? []) },
                onClearSelection: { selection = [] },
                onDownload: { onDownload(selection) })
        }
        // ⌘A 全选当前层（简报）。见 `selectAllShortcut` 里为什么不靠 `List` 自带的行为。
        .background(selectAllShortcut)
        .task(id: taskKey) { await enter() }
    }

    // MARK: - 面包屑

    private var breadcrumbBar: some View {
        HStack(spacing: 4) {
            // 根显示为**批次号**（简报）。批次号那条呈现规则只在 `DeliverySummary` 里写一次
            // （空码 → `—`），这里复用，不抄第二份。
            crumb(name: DeliverySummary.of(model.loadState)?.code ?? "—", path: "")

            ForEach(breadcrumb.segments, id: \.path) { segment in
                Image(systemName: "chevron.compact.right")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                crumb(name: segment.name, path: segment.path)
            }

            Spacer(minLength: 8)

            // 治的是"我正在这个目录**里面**，却勾不到它"——根目录不加（那时它与「全部下载」同义）。
            if !currentPath.isEmpty {
                Button {
                    onDownload([currentPath])
                } label: {
                    Label("下载本目录", systemImage: "arrow.down.circle")
                }
                .labelStyle(.titleAndIcon)   // 同一个坑：不写就只剩图标
                .buttonStyle(.borderless)
                .help("把当前这个目录（连同子目录）加进下载")
            }

            Button {
                Task { await load(path: currentPath) }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .help("重新读取这一层")
        }
        .font(.callout)
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
    }

    /// 一段面包屑。**四段都可点**（点当前那一段 = 重新读一次这一层）。
    private func crumb(name: String, path: String) -> some View {
        Button {
            navigate(to: path)
        } label: {
            // ⚠️ 原文直出：`×` 与空格由 `Text` 原样渲染（约束 3）。
            Text(name)
                .lineLimit(1)
                .truncationMode(.middle)
                .fontWeight(path == currentPath ? .semibold : .regular)
                .foregroundStyle(path == currentPath ? .primary : .secondary)
        }
        .buttonStyle(.plain)
        .help(path.isEmpty ? "回到批次根目录" : path)
    }

    private var breadcrumb: Breadcrumb { Breadcrumb(path: currentPath) }

    // MARK: - 回退提示（`path_not_found` 就地说明，不停在错误页）

    private func failureBar(_ f: DirLoadFailure) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "info.circle")
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                if let notice = f.notice {
                    // 这一句是**壳自己写的**（描述界面动作），下面那句才是内核原文。
                    Text(notice)
                        .font(.callout.weight(.semibold))
                }
                // 约束 3：内核原文逐字，可选中复制。
                Text(f.message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
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

    // MARK: - 列表 / 四态

    @ViewBuilder private var content: some View {
        switch phase {
        case .loading, .idle:
            centered {
                ProgressView()
                    .controlSize(.small)
                Text("正在读取目录…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        case .failed:
            // 加载失败也要有**可读的呈现**（约束 4）。原文已经在上面那条提示里，
            // 这里给一颗「重试」——否则用户面对的是一个没有出口的页面。
            centered {
                Label("这一层没能读出来", systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Button("重试") { Task { await load(path: currentPath) } }
                    .buttonStyle(.borderless)
            }
        case .loaded where rows.isEmpty:
            centered {
                Image(systemName: "folder")
                    .font(.largeTitle)
                    .foregroundStyle(.tertiary)
                Text("这个目录是空的")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        case .loaded:
            list
        }
    }

    private var list: some View {
        VStack(spacing: 0) {
            columnHeader
            Divider()
            List(rows, id: \.path, selection: $selection) { row in
                rowView(row)
            }
            .listStyle(.inset)
            // -----------------------------------------------------------------
            // ⚠️ 双击**不能**挂在行内容上：`.onTapGesture(count: 2)` 会与 `List` 自带的选中
            //    竞争，在 macOS 上的表现是单击选中变灰 / 延迟 / 失灵 —— 也就是
            //    "文件没法勾选"这件事的根因。Apple 给的正式做法就是这一对：
            //    单击选中由 `List` 自己管，双击落到 `primaryAction` 上。
            //    （`.windowDismissBehavior` 那类"直接用新 API 挡掉"的路子在 macOS 14 上
            //     还不存在 —— 已用编译验证：它报 `only available in macOS 15.0 or newer`。）
            //
            // ⚠️ 右键菜单与双击共用**同一个 `selection`**（`List` 的绑定）：`items` 是
            //    "菜单作用在哪几行上"，不是另开一份选择状态 —— 否则右键与勾选会对不上。
            // -----------------------------------------------------------------
            .contextMenu(forSelectionType: String.self) { items in
                // 空集 = 右键落在没有行的空白处：那就按**当前勾选面**来，不让用户对着
                // 一个空集发请求（那会变成 `paths: []` = 下全部 —— 与他的意图正好相反）。
                Button("下载选中项") { onDownload(items.isEmpty ? selection : items) }
                Divider()
                Button("全选本层") { selection = BrowserSelection.all(in: rows) }
                Button("全不选") { selection = [] }
            } primaryAction: { items in
                // 双击的**语义**在 `BrowserPrimaryAction`（纯函数，有单测）——
                // 这里只剩把结果分派出去（约束 8：视图不做判断）。
                // `dirsInCurrentLevel` 只能是**这一层**的目录：勾选面可以跨目录，
                // 而"跨目录的某个路径是不是目录"壳手里没有那份数据（`flat` 只列文件）。
                switch BrowserPrimaryAction.of(
                    paths: items,
                    dirsInCurrentLevel: Set(rows.filter { $0.kind == .dir }.map(\.path))) {
                case .enter(let path):
                    // ⚠️ **进入目录是导航**：双击的**第一下**已经把这个目录的路径写进了勾选面
                    //    （`List` 的单击选中），这里必须**立刻摘掉**它 ——
                    //    否则那个路径会永远留在勾选面里（而它属于用户已经离开的那一层，
                    //    界面上看不见），下次「下载选中」时内核按**目录前缀**把它展开成
                    //    **整个目录**（`core/src/main.rs:1254`）。
                    //    完整的事故经过、判据与代价见 `BrowserSelection.afterEntering`。
                    selection = BrowserSelection.afterEntering(path, selection: selection)
                    navigate(to: path)
                case .enqueue(let paths): onDownload(paths)
                case .nothing:            break
                }
            }
            // 规格 §7.2：列表插入/删除动画。
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: currentPath)
        }
    }

    /// 四列的表头（名称 · 大小 · **时间** · 状态）。宽度与 `rowView` 一一对应。
    ///
    /// ⚠️ 表头与行**必须逐列同宽**（含对齐方向）：两边对不上时列会错位，
    ///    而"表头写着「时间」的那一列其实是大小"是看屏幕才能发现的错误，
    ///    编译器与单测都拦不住 —— 所以这四个宽度是**照着抄**的，改一处必须改另一处。
    ///
    /// ⚠️ 表头**没有**行首那两格（复选框与图标的占位）——这是任务 3 留下的既有形态
    ///    （复选框是控件，宽度不是常量），本次只加「时间」列、不顺手改它。
    ///    由于「名称」是唯一的弹性列，右侧三列（大小 / 时间 / 状态）在两处**对齐**，
    ///    只有「名称」这一格比它下面的文件名偏左约一个复选框的宽度。
    ///
    /// ⚠️ 「时间」列的 120pt 与 `RootView` 的 `minWidth: 1120` 是一对：
    ///    窗口下界就是照四列的宽度估的（四列约 820px + 侧边栏 + 内边距）。
    private var columnHeader: some View {
        HStack(spacing: 8) {
            Text("名称")
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("大小")
                .frame(width: 96, alignment: .trailing)
            Text("时间")
                .frame(width: 120, alignment: .leading)
            Text("状态")
                .frame(width: 72, alignment: .leading)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 20)
        .padding(.vertical, 6)
    }

    private func rowView(_ row: BrowserRow) -> some View {
        HStack(spacing: 8) {
            // ⚠️ 它是一个**控件**（`Button`），不是手势：我们刚刚才把行级手势删掉，
            //    正是为了不让手势与 `List` 的选中竞争。
            // ⚠️ **已知的不确定点**：控件嵌在可选中的 `List` 行里，在 macOS 上通常不影响
            //    `List` 的选中，但本仓库没有 GUI 自动化可证实。README 第 20 条是它的验收入口；
            //    **若多选被它干扰**，按那一节写好的降级执行（改成纯 `Image` 指示器，3 行改动）。
            Button {
                if selection.contains(row.path) { selection.remove(row.path) }
                else { selection.insert(row.path) }
            } label: {
                Image(systemName: selection.contains(row.path)
                      ? "checkmark.square.fill" : "square")
                    .foregroundStyle(selection.contains(row.path) ? Color.accentColor : Color.secondary)
            }
            .buttonStyle(.borderless)
            .help(selection.contains(row.path) ? "取消勾选这一项" : "勾选这一项")
            Image(systemName: row.iconName)
                .foregroundStyle(row.kind == .dir ? Color.accentColor : Color.secondary)
                .frame(width: 16)
            // ⚠️ 原文直出：文件名里的 `×` 与空格照原样显示（约束 3）。
            Text(row.name)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(row.detailText)
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .frame(width: 96, alignment: .trailing)
            // 「时间」= **源文件**的修改时间（内核原文，`SourceTimeText` 已经算好显示什么：
            //  认不出来的原文原样显示，缺值与空串是 `—`）。这里只有渲染。
            //  ⚠️ 等宽数字同「大小」那一列：比例数字下每行的时分会在列里左右跳。
            Text(row.sourceTimeText)
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
                .frame(width: 120, alignment: .leading)
            Text(row.state.label)
                .font(.caption)
                .foregroundStyle(tint(row.state.color))
                .frame(width: 72, alignment: .leading)
        }
        // ⚠️ 这里**曾经**是 `.contentShape(Rectangle())` + `.onTapGesture(count: 2)`：
        //    它把整行的点按都吞进自己的手势里，与 `List` 的选中竞争 ——
        //    那正是"文件没法勾选"的根因。双击与右键都挪到 `list` 的
        //    `.contextMenu(forSelectionType:primaryAction:)` 上（见那里的注释）。
        // 悬停提示：完整路径（长路径会在行里被截断）。
        .help(row.path)
    }

    /// `RowColor` → SwiftUI 的 `Color`。
    ///
    /// ⚠️ 这一步**故意留在视图里**（同 `EngineStatusBadge.tint`）：它是纯渲染决定、
    ///    断言不出有意义的东西，而 `Presentation/` 不为了它引入 SwiftUI 依赖。
    ///    "哪个状态配哪个语义色"这件事本身已经在 `RowStateStyle.color` 里定好并有单测。
    private func tint(_ c: RowColor) -> Color {
        switch c {
        case .secondary: return .secondary
        case .blue: return .blue
        case .green: return .green
        case .red: return .red
        // 文件浏览器自己的四态（`RowStateStyle`）用不到警告色；成员在枚举里，
        // 这里因为 `switch` 必须穷尽而出现 —— **不要**用 `default` 盖掉，
        // 那样下次给枚举加成员时就不会有编译错误提醒了。
        case .orange: return .orange
        }
    }

    private func centered<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 8) { content() }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(20)
    }

    // MARK: - ⌘A

    /// ⌘A：全选**当前层**（不是整棵树）。全选的规则在 `BrowserSelection.all`（有单测）。
    ///
    /// ⚠️ 显式挂快捷键而不是指望 `List` 自带的行为：macOS 上 `List` 的选择绑定是否响应 ⌘A
    ///    取决于样式与焦点，而"按了没反应"正是约束 4 说的那种静默失效。
    /// ⚠️ 用 `frame(0×0) + opacity(0)` 而**不是** `.hidden()`：`hidden()` 会把按钮从命令
    ///    系统里摘掉，快捷键跟着失效。它是不可见、不可聚焦的（`accessibilityHidden`）。
    private var selectAllShortcut: some View {
        Button("全选当前层") {
            selection = BrowserSelection.all(in: rows)
        }
        .keyboardShortcut("a", modifiers: .command)
        .buttonStyle(.plain)
        .frame(width: 0, height: 0)
        .opacity(0)
        .accessibilityHidden(true)
    }

    // MARK: - 取值

    private var rows: [BrowserRow] { BrowserRow.rows(entries, parent: currentPath) }

    private var code: String? {
        if case .loaded(let info) = model.loadState { return info.code }
        return nil
    }

    /// `.task(id:)` 的触发键：**换批**、**换层**、或**这一批的树刚刚到位**就重新进来一次。
    ///
    /// ⚠️ 取值规则在 `BrowserSelection.taskKey`（**纯函数，有单测**）—— 视图不单测（约束 8），
    ///    而"什么变化要重新进一次"是本项目栽过的那类静默失效，不能只写在这里。
    ///    最要紧的两段：
    ///      - `treeCode`：`AppModel` 先落 `loadState = .loaded(info)`、之后才拉 `get_tree`，
    ///        所以第一次进来时树多半**还没到**；那时 `enter()` 会**推迟**播种，而正是这个 key
    ///        的变化让树到位之后能**再进一次**、把默认选中面播下去。少了它，推迟就变成了
    ///        "永远不播"（同样的静默失效，只是换了个地方）。
    ///      - `generation`（`AppModel.loadGeneration`）：**同码重载**时前三个分量一个字都没变，
    ///        而内核已经重新规划过整批 ⇒ 这一批的默认勾选面要重播。这个分量就是 `enter()`
    ///        被重新触发的唯一入口（少了它，"重播"无从发生）。
    private var taskKey: String {
        BrowserSelection.taskKey(code: code ?? "", path: currentPath,
                                 treeCode: model.treeCode, generation: model.loadGeneration)
    }

    // MARK: - 动作

    /// 进入视图 / 换层 / 树到位时的唯一入口。
    ///
    /// ⚠️ **换批的"立刻复位"不在这里，在 `RootView`**（`onChange(of: loadedCode)` →
    ///    `ManifestTracking.display`）：`code` 一变就要把 `currentPath` 与勾选面清干净，
    ///    **不等这一批的树到**。放这里做不到：本视图在 `loadState != .loaded` 时根本不存在
    ///    （届时只有空态页），而工具栏那颗下载按钮**一直在**（它读的也是同一个勾选面）。
    ///    本函数只负责**播种**那一半：把这一批的默认选中面播下去（**每一次成功的加载**
    ///    恰好一次 —— 同码重载是新的一次加载，所以会再播一遍）。
    ///
    /// ⚠️ **"重播默认面"与"回根目录"是两件事，这里刻意分开**（复审重要 2）：
    ///      - **重播默认面**：每一次成功的加载都做。内核刚重新规划过整批，
    ///        `default_selected` 是**新信息**（含内核崩溃后的自动恢复）。
    ///      - **回根目录**：只在**交付码真的变了**时做（`seeding.isANewBatch`）——
    ///        换批之后那是**另一棵树**，回到一个确定的落点是对的。**同码重载不做**：
    ///        它是一次**非用户动作**引发的加载（内核恢复 / 重试同一个码），
    ///        把用户停在的子目录静默换掉、界面上一句话都没有，是约束 4 要防的那类形态
    ///        （用户攒了半天的勾选位置会凭空消失）。
    ///    判据 `isANewBatch` 是纯值、有单测（`ManifestTracking.seed`），本视图只做分派。
    private func enter() async {
        guard let code else { return }

        // 重设默认选中面（来自 `get_tree` 的 `default_selected`，**不是**当前层的全部条目），
        // 并按需回到根。返回 nil 有两种情况，都不动选择：
        //   ① **这一次加载**已经播过种（目录来回切 / 切分区回来 / 这一批已经播过种）
        //      —— 用户攒的选择不能被抹掉；
        //   ② **这一批的树还没到位**（`treeCode != code`）—— 推迟，**不记账**，
        //      等 `taskKey` 因为树到位而变化之后再进来播（见 `taskKey` 的注释）。
        // ⚠️ `generation` 必须传 `model.loadGeneration`：它是"这是不是一次新加载"的唯一判据
        //    （同码重载时 `code` / `treeCode` 都不变，光看它们分不出来）。见 `ManifestTracking.seed`。
        if let seeding = manifest.seed(code: code, treeCode: model.treeCode,
                                       tree: model.tree, generation: model.loadGeneration) {
            selection = seeding.selection
            // ⚠️ 只有**换批**才回根（同码重载保留用户当前位置）。理由见上面那段。
            if seeding.isANewBatch, currentPath != "" {
                // 回根。`taskKey` 跟着变 ⇒ 这个 `.task` 会再进来一次，那时这一批已经记过账了
                // （`seed` 返回 nil），于是直接去读根目录（不会绕圈）。
                currentPath = ""
                return
            }
        }

        await load(path: currentPath)
    }

    /// 读一层目录。**唯一的 `list_dir` 调用点。**
    private func load(path: String) async {
        phase = .loading
        do {
            let result = try await model.listDir(path)
            entries = result.entries
            phase = .loaded
        } catch let e as CoreError {
            let f = DirLoadFailure.of(e, currentPath: path)
            failure = f
            phase = .failed
            if f.didFallBack {
                // ⚠️ `path_not_found`（内核在路径**指向文件**时也报这个）：退回上一层，
                //    而不是把用户留在错误页（简报点名）。回退后 `taskKey` 变了，
                //    `.task` 会去读那一层 —— 用户看到的是上一层的内容 + 上面那条说明。
                currentPath = f.path
            }
        } catch {
            // 不可达：`listDir` 只抛 `CoreError`（`absorbing` 只重抛 `CoreError`，
            // 形状不符也已经被包成 `malformedResponse`）。留着是为了不让编译器把
            // 这条出口吞掉——真出现了也要**看得见**（约束 4），不是静默。
            failure = DirLoadFailure(message: "\(error)", path: path, notice: nil)
            phase = .failed
        }
    }

    /// 用户主动导航（面包屑 / 双击目录 / 右键菜单）。
    private func navigate(to path: String) {
        // 上一次的回退说明到这里就过期了：用户已经在主动换地方。
        failure = nil
        if path == currentPath {
            Task { await load(path: path) }     // 点当前那一段 = 重新读一次
        } else {
            currentPath = path                  // `taskKey` 变了 ⇒ `.task` 去读
        }
    }

    // ⚠️ 这里曾经有一个 `activate(_ row: BrowserRow)`（按 `row.kind` 决定进入还是入队）。
    //    它已随行级双击手势一起删除 —— 那条规则现在是 `BrowserPrimaryAction`（纯函数、
    //    四条单测钉着），而**它是按"当前层里的目录"判的，不是按 `row.kind`**：
    //    双击的 `items` 可能是一整个跨目录的勾选面，那时壳手里没有"它是不是目录"的数据。
}
