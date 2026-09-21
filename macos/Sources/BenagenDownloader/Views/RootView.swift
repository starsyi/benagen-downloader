import SwiftUI
import BenagenCoreKit

// ---------------------------------------------------------------------------
// 🔴 常驻提示行的**高度不许由外部文本 / 外部数据决定**
//
// 本文件里挂着**五条常驻提示行**（引擎横幅 / 下载回执 / 换码结果 / 历史写盘失败 /
// 改下载目录回执）。**第 6 个落点**在 `SettingsView` 里（同一条改目录回执的另一个显示位置，
// 共用 `DownloadDirChangeNotice`）—— 它是"另一个落点"，**不是**本文件里的第 6 条行。
// ⚠️ 数"有多少块被上限管着"又是另一个数（引擎横幅一条行里有两块：正文 + 补充说明），
//    那个数写在 `ResidentNotice.swift` 的 `residentNoticeTextMaxHeight` 上面，**别混**。
// 它们排布在窗口内容的上方，而窗口内容的下方锚着真正要用的东西
// （文件页底栏「全选 / 全不选 / 下载选中」、侧边栏的批次摘要）——
// **任何一条常驻行被撑高，底下的东西就一起被顶出可见区**。
// 阶段 E 的那次布局事故就是这么发生的：`downloadDirBar` 把用户选的完整路径拼进了正文，
// 路径越长该行越高（实测 68 字符 15pt → 1088 字符 **195pt**），
// 于是"改完下载目录、内核重启之后底部整条不见了"，而用户完全没有线索。
//
// **机制（实现与全部实测数据）在 `Views/ResidentNotice.swift` 的文件头**，
// 这里只留使用纪律：
//   · 常驻行里的**散文**（内核原文 / 系统错误原文 / 壳写的通告）一律走
//     `BoundedNoticeText` —— 高度有界，溢出进 `ScrollView`，**全文都在**（约束 3/4/C-7）；
//   · 常驻行里的**列表**（逐条拒绝理由）走 `BoundedNoticeArea` 并给列表那一档上限；
//   · **路径这类"数据"**用 `lineLimit(1)` + `.truncationMode(.middle)` + `.help(完整值)`
//     （`DownloadDirChangeNotice` 是现成的一份）：数据截断是**可读性**问题，
//     散文截断是**丢证据**问题 —— 两者不许用同一套处理。
//   · 新增常驻行时**照抄上面的三条**，别直接摆一个裸 `Text` ——
//     裸 `Text` + `fixedSize(vertical:)` 就是这次事故的成因。
//
// ⚠️ **上面这三条只管常驻提示行**（本文件这 5 条 + `SettingsView` 那个落点），
//    **不是**一条"`lineLimit` 只许用在数据上"的全局规矩 —— `macos/Sources` 里现在就有
//    两处**有意**的 `lineLimit(1)`，**都没问题、别去"修"**：
//    `EngineStatusBadge.swift`（工具栏徽标，恒定一行的徽标 + `.middle` + `.help`）
//    与 `SelectionBar.swift`（整个底栏，它自己就是被顶出去的那一位）。
//    判据是**位置上**的：**行在窗口内容的上方、且下面锚着别的东西** ⇒ 归这三条管。
// ---------------------------------------------------------------------------

/// 主窗口骨架：侧边栏（三分区）+ 主区 + 工具栏。
///
/// ⚠️ **RootView 是接线中枢，后续四个任务都在这里加分支，别另起一个顶层视图**：
///    任务 5（加载分支：`.loaded` 进主界面，`.idle`/`.failed` 进 `EmptyState`）、
///    任务 7（工具栏「下载选中」）、任务 8（轮询启停 + 传输列表顶部横幅）、
///    任务 10（设置场景）。本任务只证明**这条链路能起真窗口**。
struct RootView: View {
    @ObservedObject var model: AppModel

    /// 当前分区。任务的边界见 `SidebarSection` 的注释 —— 枚举成员不许改。
    @State private var section: SidebarSection = .files

    // -----------------------------------------------------------------------
    // 文件浏览器的三个状态**放在这里**（而不是 `FileBrowser` 的 `@State` 里）。
    //
    // 理由：`mainArea` 是按 `section` 分派的 `switch` —— 切到「传输列表」再切回来，
    // `FileBrowser` 会被重新构造、它的 `@State` 跟着重建。而**切分区不是换批次**：
    // 简报只允许"换交付码时清空选择"，可"选完文件去看一眼传输列表再回来"恰恰是这个应用的
    // 主流程。留在视图的 `@State` 里的后果是：用户跨目录攒的选择与当前所在的目录
    // 被**无声地**打回 `default_selected` + 根目录。
    //
    // 为什么不放进 `AppModel`：这三个都是**纯视图状态**（用户在看哪一层、勾了哪些），
    // 不是内核数据的搬运结果 —— 放进模型等于让模型持有界面位置，那才是约束 8 的越界
    // （模型会长出"面包屑""选择"这类没有内核对应物的字段）。`RootView` 本来就持有
    // `section`（同一性质），放它这里最顺。
    //
    // 为什么不用 `ZStack` + `opacity` 保活两支视图：那样两个分区会**同时活着**，
    // 各自的 `.task`（任务 8 起还有 200ms 轮询）在不可见时照跑 —— 拿"隐形的常驻视图"
    // 换三个变量，代价更大、也更容易养出"为什么它还在加载"的怪事。
    // -----------------------------------------------------------------------

    /// 浏览器当前所在的层（清单原文路径；根是空串）。
    @State private var browserPath: String = ""
    /// 浏览器的勾选面（路径集合）。
    @State private var browserSelection: Set<String> = []
    /// 浏览器与"是哪一批"有关的**全部**状态（换批复位的对照值 + 播种的记账）——
    /// 承重事项 A 的两半都在这一个值里，迁移是纯函数、有单测（见 `ManifestTracking`）。
    ///
    /// ⚠️ 它必须跟着视图活（`@State` 而不是 `FileBrowser` 的 `@State`）：切分区回来时
    /// 它要是丢了，用户攒的选择就会被重新播成默认面（任务 6 的复审结论）。
    /// 也**不能**放进 `AppModel`：它是纯视图状态，模型里没有对应物（约束 8）。
    @State private var manifest = ManifestTracking()

    /// `enqueue` 回执 + **它属于哪一批**（nil = 没有要说的，不显示那条）。
    /// 任务 8 的引擎横幅是**另一条**、挂在它上面：两条都是独立的行，同时出现时
    /// 垂直排开、互不遮蔽（见 `body` 里那段注释）。
    ///
    /// ⚠️ **两个回收点**，缺一不可：
    ///   ① 用户点它自己的 `×`（`downloadFeedbackBar` 里那颗）；
    ///   ② **一次成功的加载**（`onChange(of: model.loadGeneration)`，理由写在那里）——
    ///      约束 4 要的是"失败不许**自己**消失"，不是"旧结论可以永远冒充现状"。
    @State private var downloadNotice: DownloadNotice?

    /// 换码面板开着吗（工具栏那颗写着当前交付码的按钮打开它，任务 2）。
    @State private var showingSwitch = false

    var body: some View {
        NavigationSplitView {
            Sidebar(selection: $section, model: model)
        } detail: {
            // ⚠️ 这里挂着**两条**常驻提示，顺序是刻意的：
            //    ① 引擎横幅（`EngineBanner`）在最上面 —— 它是"现在有东西坏了"，
            //       而且那颗「重试」是 4b 那句超时文案的落点，必须**常驻可见**；
            //    ② 下载回执（`downloadNotice`）在它下面 —— 那是"你刚才那个动作的结果"，
            //       带自己的收起按钮。
            //    两条是 `VStack` 里**各自一行**：不叠放、不浮层、不互相遮挡，
            //    同时出现时只是把主区往下推一点（各自高度固定，没有 ZStack 的覆盖风险）。
            //    顺序也保证"谁能救场"（重试）永远在"发生了什么"（回执）之上。
            VStack(spacing: 0) {
                if let banner = EngineBanner.of(engine: model.engine, lastError: model.lastError) {
                    engineBannerBar(banner)
                }
                if let downloadNotice { downloadFeedbackBar(downloadNotice) }
                // ③ 换码结果（任务 2 复审重要 ②/③）在最下面：它说的是"你刚才换的那一次
                //    怎么了"，与下载回执同一性质（都是"你刚才那个动作的结果"），带收起按钮。
                //    ⚠️ 它必须**常驻在主区**、而不是只活在换码面板里：面板的「取消」不被禁用
                //    （面板关不关得掉不该由一次网络请求决定），面板一关，那句内核原文就没了 ——
                //    与"不得静默失效"冲突；成功方向同理（用户按了取消却看到整批被换掉，
                //    没有任何来源说明，见 `DeliverySwitch.noticeText` 的注释）。
                if let outcome = model.lastSwitchOutcome { switchOutcomeBar(outcome) }
                // ④ 历史写盘失败（最终审查重要 1）。**在这一格之前，它没有任何界面落点** ——
                //    `historyWriteFailure` 的注释与 `BatchHistoryStore` 的注释都写着
                //    "调用方必须有机会说一句 / 不许静默失效"，而全仓没有一处读它。
                //    症状是：人类伙伴敲完一句备注 → 写盘失败（磁盘满 / Application Support
                //    权限异常）→ **应用一个字都不说** → 重启后备注不见了，且**无从归因**；
                //    而"备注重启后仍在"正是这条功能的验收项。
                //    ⚠️ 它必须挂在**主区**（不是只挂在换码面板里）：写盘也发生在
                //    "一次成功的加载"（`recordLoadedBatch`）与"内核 last_code 播种"
                //    那两条路上，那两条路上**面板根本没开**。
                if let failure = model.historyWriteFailure { historyWriteFailureBar(failure) }
                // ⑤ 改下载目录的回执（最终审查重要 2）。**它必须活过设置窗口**：
                //    设置窗口是随手就会被关掉的东西，而"重启被在飞的那次挡住 ⇒ 这一次没改成"
                //    那一格回执是**唯一**的提示（那一刻内核还在用旧目录跑）。
                //    权威在模型上（`AppModel.downloadDirChange`），设置窗口那一段读的是
                //    同一个值 —— 两个落点、同一个值，不会分叉（同 `lastSwitchOutcome` 的做法）。
                if let change = model.downloadDirChange { downloadDirChangeBar(change) }
                mainArea
            }
        }
        .toolbar {
            ToolbarItem {
                downloadButton
            }
            ToolbarItem {
                // 任务 2：规格 §7.1 的「交付码显示 + 重新加载」。在此之前，加载成功后
                // `EmptyState`（全仓唯一有交付码输入框的视图）就再也不参与渲染，
                // 界面上**没有任何回程** —— 用户想换一批只能重启应用。
                //
                // 显示的就是**当前交付码**（不是"交付码"三个字）：用户一眼能确认
                // 屏上这批文件是哪一批的，而 `.help` 里那句把"这颗按钮是干什么的"说全。
                // 没有生效批次时禁用（那时主区是空态页，换码入口在那儿）。
                Button {
                    showingSwitch = true
                } label: {
                    // ⚠️ `.labelStyle(.titleAndIcon)` **不许省**：macOS 的统一工具栏默认把
                    //    `Label` 渲染成只有图标，文字会掉进 `.help` 浮层里（本仓库栽过三次，
                    //    同款证据见 `EngineStatusBadge.swift:32-36`）。而"看得见当前交付码"
                    //    恰恰是这颗按钮存在的全部意义。
                    Label(loadedCode ?? "交付码", systemImage: "qrcode")
                }
                .labelStyle(.titleAndIcon)
                .disabled(!hasLoadedManifest)
                .help("换一个交付码（当前：\(loadedCode ?? "—")）")
            }
            ToolbarItem {
                // 任务 10：设置入口。与菜单里那条「设置…」（`CommandGroup(replacing: .appSettings)`）
                // 走**同一个** `SettingsEntry`，所以 ⌘, 与点这颗按钮打开的是同一扇窗。
                // 规格 §7.1 把「设置」列在工具栏上，而设置窗口是客户唯一能改内核参数的地方
                // —— 只藏在菜单里，很多客户一辈子找不到它。
                SettingsEntry {
                    Label("设置", systemImage: "gearshape")
                }
                // ⚠️ 这一行不是装饰：macOS 的统一工具栏默认把 `Label` 渲染成**只有图标**，
                //    于是「设置」这两个字会掉进 `.help` 浮层里（同款实测证据见
                //    `EngineStatusBadge.swift:32-36`；工具栏那颗下载按钮也显式写了这一行）。
                .labelStyle(.titleAndIcon)
                .help("打开设置（⌘,）")
            }
            ToolbarItem {
                // 约束 4 的落点：内核没起来时，工具栏上那句原因就是客户唯一的线索。
                EngineStatusBadge(engine: model.engine)
            }
        }
        // -------------------------------------------------------------------
        // ⚠️ 承重事项 A：**换批那一刻就复位浏览位置与勾选面，不等这一批的树到。**
        //
        // 为什么必须在这里、而不是 `FileBrowser.enter()`：`enter()` 只在文件浏览器活着的时候
        // 跑（`loadState != .loaded` 时它根本不存在），而**工具栏那颗下载按钮一直在**——
        // 它读的也是 `browserSelection`。不复位的后果不是"界面有点旧"：
        //   - `FileBrowser` 会对**上一批的路径**发 `list_dir`（那些路径在新批次里可能指向别的文件）；
        //   - 底部「下载选中」/ 工具栏那颗按钮会拿着**上一批的路径**去 `enqueue` ——
        //     用户以为在下 B 批的文件，实际发出的是 A 批的路径。
        //
        // ⚠️ 复位与播种的**两半**都由 `manifest.display(code:)` 这一次迁移覆盖：
        //    它用的对照值是"上一次**显示**过的码"（不是"最后一次播种成功的码"），
        //    并且**换批时把 `seededCode` 一起清掉** —— 少了清这一下，播种那一半就是坏的
        //    （A→B，B 的树还没到就又回 A ⇒ A 的默认选中面永远播不下来 ⇒ 静默显示「已选 0 项」）。
        //    迁移规则（含 `code == nil` 不推进）全在 `ManifestTracking` 里（纯函数，有单测）。
        //
        // ⚠️ 这里**不清** `downloadNotice`（审查次要 ④ 的取舍）：约束 4 要求拒绝理由留在界面上，
        //    所以不能无条件清。代价是"已加入 N 个下载任务"有可能属于上一批 —— 这个歧义由
        //    `DownloadNotice.summary(currentCode:)` 消掉：**不属于当前批时它前面会带批次码**。
        //    （**一次成功的加载**会清它 —— 那条线在下面 `onChange(of: model.loadGeneration)`，
        //     两者的分工写在那里。这一条只管"换批复位"。）
        // -------------------------------------------------------------------
        // ⚠️ **单参形式**（部署目标 13.0 下双参要 14.0）。它给的是**新值**，
        //    正好就是这里要的那个码。
        .onChange(of: loadedCode) { code in
            guard manifest.display(code: code) else { return }   // false = 同一批，什么都不动
            browserPath = ""
            browserSelection = []
        }
        // -------------------------------------------------------------------
        // 一次**成功的加载** ⇒ 屏幕上那条下载回执已经过期，清掉它。
        //
        // 🔴 这是**有意偏离审查次要 ④**（上面那段注释的取舍），理由写在这里（约束 D-6）：
        //    - **为什么必须清**：诊断报告 §6.1 的猜想 1（**已证实**）。回执原先只在用户点那个
        //      `×` 时才清，而"重新加载批次码"这条路上**没有任何东西**会碰它 —— 于是屏幕上那句
        //      "没有匹配到任何文件"**原样留着**。用户读到的不是"我重载了、这是新的一次加载"，
        //      而是"我重载了**还是**报这个错"（人类伙伴的原始描述里有那半句）。
        //      而它此刻**确实是假的**：成功的 `load_delivery` 会让内核重新规划整批
        //      （重算 completion、重算 pending），那条回执描述的那份状态已经不存在了。
        //      约束 4 要的是"失败不许**自己**消失"，不是"旧结论可以永远冒充现状"。
        //    - **为什么不会伤到约束 4**：
        //      ① 触发条件是**一次成功的加载** —— 用户自己的动作（换码面板「加载」/ 空态页
        //         「重试」/ 内核崩溃后的恢复），不是定时器、不是重绘、不是切分区；
        //      ② **失败的加载不推进** `loadGeneration`（见它的注释）⇒ 一次失败的「重试」
        //         之后，屏幕上那条拒绝理由**一个字都不会少**（`aFailedLoadDoesNotAdvance...`
        //         钉着这一条）；
        //      ③ 用户随时可以点那个 `×` 提前收起 —— 那条路照旧。
        //    - ⚠️ **一处如实记下的窄口子**：`loadGeneration` 只在**整次加载（含 `get_tree`）
        //      成功**时才推进（复审重要 1 要求的那个不变量），所以 `get_tree` 失败的那次加载
        //      **不会**清掉这条回执。那两条路上界面**不是静默的** —— 失败**必落成顶部横幅
        //      或退回空态**（逐条分派见 `AppModel.absorb`，这里**不逐条复述**：Ruling D7
        //      那条窄口子正押在这句话上，别说满、也别把分派说反）。用户看到的是
        //      "引擎出问题了"，而不是"那句旧结论还在冒充现状"。
        //      ⚠️ 这里曾经说成"`transport` → 引擎横幅；其余错误码 → `lastError`"，**是反的**：
        //      `engine_disconnected` / `engine_start_failed` / `protocol_mismatch` 同样走
        //      引擎横幅，`engine_not_started` / `no_delivery` 则什么都不写（后者退回空态），
        //      只有 `engine_rpc_failed` / `malformed_response` 才真的落到 `lastError`。
        //    - **代价（如实记下）**：一次成功加载之后，"已加入 N 个下载任务"这类**成功**回执
        //      也一并消失（不再走 `summary(currentCode:)` 的批次码前缀那条路）。这是可接受的：
        //      换批/重载之后内核手里的队列已经不是那句话描述的那一份了，而任务本身在
        //      「传输列表」里看得见（`added` 非空时本来就自动切过去）。
        //      `summary(currentCode:)` 的前缀仍然有用 —— 失败的加载会让回执活过换批
        //      （比如换个不存在的码失败之后，那句拒绝理由属于上一批，前面就会带批次码）。
        // -------------------------------------------------------------------
        .onChange(of: model.loadGeneration) { _ in
            downloadNotice = nil
        }
        // -------------------------------------------------------------------
        // 传输列表的轮询：**只在它可见时跑**（简报）。
        //
        // 为什么挂在"分区 + 有没有生效批次"上而不是视图自己的 `.task` 里：
        //   ① 省电、少给内核添无谓负载（规格 §5.3 的拉模型按 200 ms 一拍）；
        //   ② 切到「文件」「校验结果」时它必须**立刻停**，而不是等某个视图消失。
        // `initial: true` 让"启动时已经在传输列表"这条也走到（不然只有切换才会启动）。
        // -------------------------------------------------------------------
        // ⚠️ **`.task(id:)` 而不是 `.onChange(of:initial:)`**：后者是 14+ 的写法，
        //    而前者的语义**恰好就是这里要的** —— "出现时跑一次 + id 变化时重跑"。
        //    它连同替掉了原来的两处手工管理：
        //      · `restartPolling(true)`（视图出现 / 切到传输列表时启动）；
        //      · `.onDisappear { restartPolling(false) }`（视图消失时停）。
        //    因为 SwiftUI 在**视图消失或 id 变化时会取消这个 task**，与 cancel+start 等价；
        //    `pollLoop` 自己的 `try await Task.sleep` 抛出即 return，取消立刻生效。
        //    ⚠️ 别再补一个 `pollTask` 自己管：那是把 `.task` 已经保证的东西再写一遍，
        //    而两份记账迟早会分叉（"切走了还在拍"就是这么来的）。
        .task(id: shouldPollTransfers) {
            guard shouldPollTransfers else { return }
            await pollLoop()
        }
        // 规格 §14.2 的定稿。配合 App.swift 的 `.windowResizability(.contentMinSize)`，
        // 这个 frame 就是**窗口拖不小到的下界**：
        //   侧边栏 220–280 + 四列平铺列表（名称/大小/时间/状态）约需 820px + 内边距 20×2 + 工具栏。
        // 这个值是**实测前定的** —— 任务 11 的手工清单第 2 条要求把窗口拖到最小；
        // 若发现列被挤变形就调这个数，并把新值写进 README。
        // ⚠️ 1000 → 1120 是「时间」列（120pt）带来的：列宽在 `FileBrowser.columnHeader`
        //    与 `rowView` 里逐列写死，这一行必须跟着它们走（README §21 是验收入口）。
        .frame(minWidth: 1120, minHeight: 640)
        // 换码面板（任务 2）。**换批之后的复位不用新写**：上面那条
        // `onChange(of: loadedCode)` 已经会调 `manifest.display(code:)` 并清掉
        // `browserPath` / `browserSelection`（阶段 B 建的那条线）。
        .sheet(isPresented: $showingSwitch) {
            SwitchDeliverySheet(model: model)
        }
    }

    /// 加载分支（简报定的）：**只有 `.loaded` 进主界面**，其余（未加载 / 加载中 / 失败）
    /// 都进 `EmptyState` —— 加载中那一刻还没有清单可看，失败要原地给原文与「重试」，
    /// 而"没有生效的批次"正是空态页要说的事。
    ///
    /// 这里的分派是**渲染分派**、不是值计算（约束 8）：`.loaded` 带出来的 `DeliveryInfo`
    /// 由各自的内容视图与 `DeliverySummary` 去解释，本文件不碰它。
    /// 任务 9 会把剩下那一支换成 `VerifyView`。
    @ViewBuilder private var mainArea: some View {
        if case .loaded = model.loadState {
            switch section {
            case .files:
                // 任务 6：文件浏览器（平铺列表 + 面包屑 + 双击进入 + 选择）。
                // 三个 `@Binding` 的状态由本视图持有（理由见上面那段注释）——
                // 切分区回来时用户的目录与选择**原样还在**。
                FileBrowser(model: model,
                            currentPath: $browserPath,
                            selection: $browserSelection,
                            manifest: $manifest,
                            // 底栏那颗按钮与工具栏那颗走**同一条路**（见 `download(_:)`）。
                            onDownload: { download($0) })
            case .transfers:
                // 任务 8：传输列表。**轮询的启停在本文件**（见 `shouldPollTransfers`）——
                // 视图只渲染，定时器跟着"这个分区是否可见"走。
                TransfersView(model: model)
            case .verify:
                // 任务 9：校验视图（六类齐备 + 总进度）。**刷新由视图自己发起**
                // （它在 `.task` 里刷一次，另有「刷新」按钮）—— 与传输列表那条定时器不同：
                // 校验没有"要盯着看"的连续变化，切进来取一次就够（简报）。
                VerifyView(model: model)
            }
        } else {
            EmptyState(model: model)
        }
    }

    // MARK: - 传输列表的轮询（任务 8）

    /// 传输列表现在可见吗（⇒ 该跑 200 ms 定时器）。
    ///
    /// ⚠️ 两个条件都要：**分区是传输列表**（可见）**且这一批清单已经加载**
    ///    （传输列表只在 `.loaded` 时存在，没加载时轮询只是白问内核）。
    private var shouldPollTransfers: Bool {
        section == .transfers && hasLoadedManifest
    }

    /// 200 ms 一拍（节拍常量在 `TransferListPoll`，规格 §5.3）。
    ///
    /// ⚠️ **引擎不可用时一拍都不发**（`EngineGate`，与视图里那些禁用同源）：
    ///    内核卡死时 `CoreClient` 的 FIFO 队列已经被那条永不返回的请求永久堵死
    ///    （约束 15），再发一条只会永远挂着 —— 而 `pollInFlight` 会一直为真，
    ///    于是**此后每一拍都被跳过**，界面停在上一次快照上再也不动。
    ///    那种形态下该说话的是顶部横幅（带「重试」），不是这个定时器。
    ///
    /// 第一拍用 `refreshTransfers()`（它与 `pollTick` 共用单飞纪律，但**没有**"引擎必须在跑"
    /// 这道理）：刚切进这个分区时用户要的是"现在就给我一份快照"，而 `engine_not_started`
    /// 正是要靠它换出那句空态文案（简报：那不是错误）。之后的每一拍才走 `pollTick`。
    private func pollLoop() async {
        var primed = false
        while !Task.isCancelled {
            if EngineGate.allowsRequests(model.engine) {
                if primed {
                    await model.pollTick()
                } else {
                    await model.refreshTransfers()
                    primed = true
                }
            }
            do {
                try await Task.sleep(nanoseconds: TransferListPoll.intervalNanoseconds)
            } catch {
                return      // 取消（切走分区 / 视图消失）⇒ 立刻停，不再拍
            }
        }
    }

    // MARK: - 引擎横幅（任务 8）

    /// 主区顶部那条常驻横幅：「引擎/内核现在有问题」。
    ///
    /// ⚠️ 文案是 `EngineBanner.text` —— **内核原文逐字**（约束 3），这里不加工、不加前缀。
    /// ⚠️ 那颗「重试」调 `AppModel.retryEngine()` —— 它就是任务 4b 那句
    ///    「内核无响应（等待超过 5 秒）」的 tooltip 里写的"…然后点「重试」"的**落点**。
    ///    它是**常驻**的（不是悬停浮层、不是菜单项）：约束 4 要的是失败出现在界面上，
    ///    而这颗按钮是用户唯一的出口。
    private func engineBannerBar(_ banner: EngineBanner) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: banner.kind == .engineUnavailable
                      ? "exclamationmark.triangle.fill"
                      : "exclamationmark.circle")
                    .foregroundStyle(banner.kind == .engineUnavailable ? Color.red : Color.orange)
                // ⚠️ 内核原文是**外部文本**（长度不受壳控制，`coreNotFoundError`
                //    那一句本身就带三行候选路径）⇒ 高度必须有界。
                //    **有界 ≠ 截断**：溢出时给的是可滚动的全文（见文件头那段纪律）。
                BoundedNoticeText(text: banner.text, font: .callout.weight(.semibold))
                Spacer(minLength: 8)
                if banner.showsRetry {
                    Button("重试") {
                        Task { await model.retryEngine() }
                    }
                    // 正在重启（`busyReason` 非 nil）时禁用：重启有防重入闸门，
                    // 第二次点击会**立刻返回、什么都不做** —— 一颗看起来能点、
                    // 点了没反应的按钮正是约束 4 要防的形态（`retryAfterATimeout*`
                    // 那几条测试盯的就是"重启真的会跑"）。
                    .disabled(model.busyReason != nil)
                }
            }
            // 补充说明（只有"内核一个字都没回"的握手超时才有）：
            // 内核自己说清楚的失败原因**不套壳的话**（约束 3）。
            if let hint = banner.hint {
                // 同一道闸门：这一行也是常驻的（同一条横幅的第二行）。
                // 它是**壳自己写死的常量**（长度壳完全掌握），走散文那一档就够。
                BoundedNoticeText(text: hint, font: .caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4))
    }

    // MARK: - 换码结果（任务 2）

    /// 换码结果那一行。**文案全部来自 `DeliverySwitch.noticeText`**（`Presentation/`，有单测）：
    /// 失败那支就是内核原文逐字（约束 3），成功那支是壳写的一句陈述（理由写在那边）。
    /// 本视图只做**渲染分派**（选图标与颜色）。
    ///
    /// ⚠️ 可选中复制（约束 3 / C-7）：客户要能把失败那句话原样发给业务方。
    private func switchOutcomeBar(_ outcome: DeliverySwitch) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: outcome.isFailure ? "exclamationmark.triangle.fill"
                                                : "arrow.triangle.2.circlepath")
                .foregroundStyle(outcome.isFailure ? Color.red : Color.secondary)
            // 失败那一支是**内核原文**（长度不受壳控制）⇒ 高度必须有界、但**不截断**。
            BoundedNoticeText(text: outcome.noticeText, font: .callout.weight(.semibold))
            Spacer(minLength: 8)
            Button {
                model.dismissSwitchOutcome()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("收起这条提示")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4))
    }

    // MARK: - 历史写盘失败 / 下载目录的改动（阶段 E 修复波）

    /// 历史（`history.json`）写盘失败那一行。
    ///
    /// ⚠️ 文案是**壳自己写的**（`AppModel.writeHistory` 拼的原文 + 系统错误文本）——
    ///    这件事内核一个字都没说（它是壳自己的偏好/历史文件），所以没有"内核原文"可登，
    ///    与 `lastError` 的契约（照登内核原文）不同，**它不落那里**（理由在那个字段的注释里）。
    /// ⚠️ 可选中复制：用户要能把系统给的错误原文发给人看。
    ///
    /// ⚠️ 它**只由这里与换码面板那行**呈现（两处读的是同一个字段）：`×` 调
    ///    `AppModel.dismissHistoryWriteFailure()`；而**一次成功的写盘会自己清掉它**
    ///    （`writeHistory` 的成功分支），那才是它主要的消失方式 —— 用户收起之后
    ///    若问题还在、下一次写盘又会让它回来。
    private func historyWriteFailureBar(_ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(Color.orange)
            // 系统错误原文（文件系统给的，长度不受壳控制）⇒ 高度必须有界、但**不截断**。
            BoundedNoticeText(text: text, font: .callout.weight(.semibold))
            Spacer(minLength: 8)
            Button {
                model.dismissHistoryWriteFailure()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("收起这条提示")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4))
    }

    /// 改下载目录的回执。
    ///
    /// ⚠️ 文案全部来自 `DownloadDirChange`（`Presentation/`，有单测）—— 失败那支是
    ///    **可直接显示给用户的那句话**（写盘失败是壳写的、重启失败那句也是
    ///    壳在 `restartKernel` 里写的），本视图只做**渲染分派**（图标与颜色 + 截断）。
    /// ⚠️ **常驻**（不是只活在设置窗口里）：`下载目录记下来了、但内核还在用旧目录跑`
    ///    那一格回执是唯一的提示，而设置窗口会被关掉 —— 关掉之后它必须还在。
    /// ⚠️ 可选中复制（约束 3 / C-7）。
    ///
    /// 🔴 **这一条就是那次布局事故的现场**（判据与理由见本文件文件头那段：
    ///    常驻提示行的高度不许由外部文本 / 外部数据决定）：
    ///    改成功时正文里带着**用户选的完整路径**，而路径是外部文本、长度不受壳控制 ——
    ///    它一长，这行就高，锚在窗口底边的文件页底栏与侧边栏摘要就被顶出可见区。
    ///    现在拆成**两行**（正文块抽成了 `DownloadDirChangeNotice`，设置窗口那一段
    ///    用的是**同一个**类型 —— 那里是同一条回执的另一个落点，形态不许分叉）：
    ///      ① `headline` —— 固定短的标题（**任何路径下逐字相同**，`Presentation/` 里的
    ///         `theHeadlineIsFixedNoMatterHowLongThePathIs` 钉着这一条）；
    ///      ② `pathDetail` —— 路径**单独一行**，`lineLimit(1)` + **中间截断**
    ///         （中间截断而不是尾部：路径的头尾恰恰是"在哪个卷、哪个批次"这两件
    ///         用户最需要辨认的事）+ `.help` 悬停看完整路径。
    ///    于是**无论路径多长，这条常驻提示的高度都是固定的**。
    ///    ⚠️ 别把两行合回一行：`noticeText` 那个**值**还在、测试还在逐字读它，
    ///    所以"把整句铺进常驻行"看起来"很自然"—— 但那条**整句话的文案不是给常驻行用的**
    ///    （第 2 轮起 `Sources/` 里**已经没有任何视图显示它**：两个落点渲染的都是
    ///    `headline` + `pathDetail`）。把它合回去 = 复现这次事故。
    private func downloadDirChangeBar(_ change: DownloadDirChange) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: change.isFailure ? "exclamationmark.triangle.fill"
                                               : "folder.badge.gearshape")
                .foregroundStyle(change.isFailure ? Color.red : Color.secondary)
            DownloadDirChangeNotice(change: change)
            Spacer(minLength: 8)
            Button {
                model.dismissDownloadDirChange()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
            .help("收起这条提示")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4))
    }

    // MARK: - 下载（任务 7）

    /// 当前生效的交付码（没有生效批次时 nil）。**换批复位的判据**读它（见 `body` 里那段注释）。
    private var loadedCode: String? {
        if case .loaded(let info) = model.loadState { return info.code }
        return nil
    }

    /// 有没有可下的清单。没有（空态页 / 加载中 / 失败）时工具栏那颗按钮**禁用** ——
    /// 那时 `browserSelection` 里一个当前批次的路径都没有，让它可点等于把上一次的残留提交给内核。
    private var hasLoadedManifest: Bool {
        if case .loaded = model.loadState { return true }
        return false
    }

    /// 工具栏那颗「下载选中」/「全部下载」。
    ///
    /// ⚠️ 与文件浏览器底部的 `SelectionBar` **共用同一个实现**（`download(_:)`）与同一份文案
    ///    （`DownloadTargets`）：两个入口、一条逻辑，不会出现"工具栏下了、底栏没下"这种分叉。
    ///    没有勾选时它**不是禁用**，而是变成「全部下载」——那时发出去的是 `paths: []`
    ///    （内核语义 = 下全部待下载）。
    private var downloadButton: some View {
        Button {
            download(browserSelection)
        } label: {
            Label(DownloadTargets.buttonTitle(for: browserSelection), systemImage: "arrow.down.circle")
        }
        // ⚠️ **这一行不是装饰**：macOS 的统一工具栏默认把 `Label` 渲染成**只有图标**，
        //    于是「下载选中」/「全部下载」这句话会掉进 `.help` 浮层里 ——
        //    而"这两个状态在界面上分得出来"正是 `buttonTitle` 存在的全部意义
        //    （它们发出去的请求是两件完全不同的事：`[勾选的路径]` vs **全部待下载**）。
        //    用户分不出来的后果是他以为在下勾选项、实际下了全量。
        //    同款实测证据在 `EngineStatusBadge.swift:32-36`（那一段的末句：不加这行时
        //    "截图里工具栏只有一个圆形箭头图标、没有任何文字"）—— 全仓没有 App 级/工具栏级的
        //    兜底（`grep -rn labelStyle Sources/` 逐处都在，而且只会越来越多），
        //    所以每一处都得自己写。
        .labelStyle(.titleAndIcon)
        .disabled(!hasLoadedManifest)
        .help(DownloadTargets.helpText)
    }

    /// **下载动作的唯一实现**：工具栏那颗按钮与文件浏览器底部那颗按钮都走它。
    ///
    /// 分工（约束 8）：发什么由 `DownloadTargets.paths(for:allPaths:)` 算
    /// （排序；**整批全选 → `[]`**；空勾选 → `[]`），
    /// 回执怎么说由 `DownloadNotice` / `EnqueueFeedback` 算 —— 这里只负责把算好的值
    /// 摆到屏幕上（切分区、写 `downloadNotice`）。
    ///
    /// ⚠️ **`allPaths` 必须在这里取**（`get_tree` 的 `flat`，经 `BrowserSelection.allFiles`）：
    ///    判"勾选面是不是覆盖了整批"靠它，而那是约束 C-3 的硬要求 ——
    ///    整批全选必须发 `paths: []`（请求体恒定），**绝不能**把整批路径塞进请求
    ///    （超 8 MiB 的那条请求内核**不报错**，只把客户端**静默堵死**）。
    ///    两个入口（工具栏 / 底栏）共用这一个实现，所以这条纪律只有一处。
    ///
    /// ⚠️ **轮询不在这里手动启**（任务 8 补齐了那条线）：这里只把 `section` 拨到 `.transfers`，
    ///    而 `shouldPollTransfers` 跟着变 ⇒ 上面那个 `.task(id:)` 会自己起停。
    ///    在动作里手动启一次定时器是多余的，而且会让"轮询跟着可见性走"
    ///    这条规则散成两处（哪天漏了一处就是"切走了还在拍"）。
    private func download(_ selection: Set<String>) {
        Task {
            let code = loadedCode      // 这条回执属于哪一批（见 `DownloadNotice`）
            // 整批的**文件**全集（`flat` 只列文件 —— 目录不在里面）。
            let allPaths = BrowserSelection.allFiles(in: model.tree?.flat ?? [])
            do {
                let result = try await model.enqueue(
                    paths: DownloadTargets.paths(for: selection, allPaths: allPaths))
                let notice = DownloadNotice.of(result, code: code)
                downloadNotice = notice
                if notice.feedback.switchesToTransfers { section = .transfers }
            } catch {
                // 抛错路径（`engine_start_failed` / `preflight_failed` / 引擎不可用时那条闸门 /
                // **全拒时内核回的 `invalid_params`**）：**原地显示内核原文**（约束 3），
                // **不切视图**（简报）—— 切走了那句话就没人看见了。
                // 原文的映射在 `EnqueueFeedback.failure(of:)` 里（内部走 `AppModel.message(of:)`，
                // 那是 `CoreError` → 用户可见文案的唯一实现）—— 视图这一层不映射、不编文案。
                downloadNotice = DownloadNotice.failure(of: error, code: code)
            }
        }
    }

    /// `enqueue` 的结果条：「加了几个 / 哪几个没加进去」。
    ///
    /// ⚠️ 拒绝理由**逐条**摆出来（`path` + `reason`，都是内核原文、可选中复制，约束 3/4）：
    ///    "这个文件没能加进下载列表"必须出现在界面上，不能只报一个总数。
    /// ⚠️ 顶上那句由 `summary(currentCode:)` 算：**不属于当前批时带批次码**（见 `DownloadNotice`）。
    private func downloadFeedbackBar(_ notice: DownloadNotice) -> some View {
        let f = notice.feedback
        let title = notice.summary(currentCode: loadedCode)
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: f.rejections.isEmpty ? "arrow.down.circle" : "exclamationmark.triangle")
                    .foregroundStyle(f.rejections.isEmpty ? Color.secondary : Color.orange)
                // 失败时这句是**内核原文**（**全拒**那条路上它甚至是内核拼的整段
                // Rust `Debug` 串，长到没法看 —— 见 `EnqueueFeedback.of` 的注释）
                // ⇒ 高度有界、但**不截断**。
                BoundedNoticeText(text: title, font: .callout.weight(.semibold))
                Spacer(minLength: 8)
                Button {
                    downloadNotice = nil
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .help("收起这条提示")
            }
            // 🔴 **这一串的行数由内核决定，不是壳能控制的**（第 2 轮补的那一处）：
            //    内核回几条拒绝理由 = 有几个文件没加进下载列表
            //    （`core/src/main.rs:1403` 起，**每个失败文件一条、没有任何上限**；
            //    磁盘满 / 目录不可用那类**整批性**故障会让它一次返回几千条）。
            //    ⇒ 与"一行太长"是**同一类**问题的另一种形态：**常驻行的高度不许由外部
            //    数据决定**。处理也一样是"有界 + 不丢"：
            //      · 有界：`BoundedNoticeArea` 的列表上限（≈4–5 条，见 `ResidentNotice.swift`）；
            //      · 不丢：超出部分进 `ScrollView`，**一条都不许藏** ——
            //        "这个文件没能加进下载列表"必须逐条出现在界面上（约束 4），
            //        所以**没有**用"只显示前 N 条 + 还有 M 条"那种做法：
            //        它会把拒绝理由本身藏起来，而用户要的就是"到底哪些文件、为什么"。
            BoundedNoticeArea(maxHeight: residentNoticeListMaxHeight) {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(f.rejections, id: \.self) { r in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Image(systemName: "minus.circle")
                                .foregroundStyle(.tertiary)
                            Text(r.path)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Text("·")
                                .foregroundStyle(.tertiary)
                            Text(r.reason)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .textSelection(.enabled)         // 原文可选中复制（约束 3）
                    }
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4))
    }
}

// ⚠️ 任务 4 留在这里的 `MainAreaPlaceholder` 已在任务 9 删掉：三个分区分支
//    （`FileBrowser` / `TransfersView` / `VerifyView`）全部接上了真视图，它没有任何调用点了。
