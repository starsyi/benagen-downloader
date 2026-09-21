import SwiftUI
import BenagenCoreKit

// ---------------------------------------------------------------------------
// 🔴 常驻提示行的**高度上限**（阶段 E 修复波 —— 一次真实的布局事故，不是预防性措施）
//
// 现场（人类伙伴逐字）：改完下载目录、内核按新目录重启之后，
// 「最下方的状态栏（全选 / 全不选 / 下载选中）都看不到了」，
// 「左侧的侧边栏的「批次摘要」信息也超过了软件的界面，看不到了」；
// **把窗口拉高就恢复**、**把窗口整体上拖没用**、**点「恢复默认」也恢复**。
//
// 根因：`downloadDirBar` 那条**常驻**回执把**用户选的完整路径**放进了正文，
// 而正文没有行数上限、还带 `fixedSize(vertical:)`（允许折行）⇒ **路径越长这一行越高**
// ⇒ 窗口**内容**比窗口高 ⇒ 锚在内容底边的两样东西一起被裁掉。
// 实测（可用宽度 800）：同一句话在路径 68 字符时 15pt、1088 字符时 **195pt**，
// 而无上限时路径再长还会继续长。
//
// ⚠️ **本文件的数字是哪种工具量的**（第 3 轮的教训，别再混）：
//    · **高度**（本文件绝大多数数字）是**布局**量，`ImageRenderer` 量得准 ——
//      已用真 `NSHostingView` 独立复量，差值 ≤1pt；
//    · 但 `ImageRenderer` **不画 `ScrollView` 的内容**（它出的图里正文是空白）⇒
//      "字有没有真的画出来""图标贴在第几行"这类结论**一律用真 `NSHostingView` 量**。
//
// 🔴 **不变量（有意偏离的说明在这里，约束 D-6）**：
//    **常驻提示行的高度，不许由外部文本 / 外部数据的长度决定。**
//    这些行和窗口内容一起排布，而窗口内容的下方锚着真正要用的东西
//    （文件页底栏、侧边栏摘要）—— 一段长文本或一长串数据就能把它们顶出可见区，
//    而用户看到的形态是"**底部整条不见了**"，没有任何线索指向"因为你选了个深层目录"。
//
// ⚠️ **这条不变量的管辖范围就到这里为止：常驻提示行** ——
//    `RootView` 的那 5 条 + `SettingsView` 里改目录回执那一个落点。
//    **它不许被读成一条全局规矩**（"`lineLimit` 只能用在数据上"那种），
//    因为 `macos/Sources` 里今天就有两处**有意**的例外，它们**都没问题、别去"修"**：
//      · `EngineStatusBadge.swift` 的 `.lineLimit(1)`：作用在工具栏徽标那句
//        「引擎不可用：<内核原因>」（**按本仓分类那是散文**）—— 徽标的高度**本来就该
//        恒定一行**，它配了 `.truncationMode(.middle)` + `.help(tooltip)`（悬停给全文），
//        而"内核不可用"这种状态的全文本来也只有一句；
//      · `SelectionBar.swift` 的 `.lineLimit(1)`：作用在**整个底栏**上 ——
//        底栏是"恒定一行的高度锚点"（它自己就是被顶出去的那一位，不可能反过来去顶别人）。
//    判据是**位置上**的：**行在窗口内容的上方、且下面锚着别的东西** ⇒ 归这条管。
//    工具栏徽标与底栏都不满足 ⇒ 不归这条管。
//
// ⚠️ **为什么是"有高度上限的 ScrollView"，而不是"行数截断"**（第 2 轮的改动）：
//    第 1 轮用的是 `lineLimit(3)`。那**违反约束 4 / C-7** ——
//    内核对"找不到内核可执行文件"的原话就带**三行候选路径**
//    （`CoreClient.coreNotFoundError`，最少 5 行），把它截成 3 行等于
//    **用"看不见"换了"放得下"**。而"失败原文必须完整可见"是硬要求。
//    所以溢出时给的是 `ScrollView`：高度仍然有界（不变量成立），
//    但**全文/全部条目都在**，可滚动、可选中复制（约束 3 / C-7）。
//
// ⚠️ **为什么高度是由一个自定义 `Layout` 算的，而不是 `ViewThatFits`**（实测教训，别改回去）：
//    "放得下就铺开、放不下才滚"这句话，用 `ViewThatFits` 写出来的**行为是错的**：
//    `ViewThatFits` 拿到的提案高度由**兄弟节点**决定（那一行里有图标与 × 按钮），
//    装不装得下这个判断因此在 16pt 上下反复横跳；更要命的是第二支的 `ScrollView`
//    **是贪婪的** —— 一旦选中，它会把 `maxHeight` 全吃掉，**哪怕正文只有一行**。
//    实测（"常驻行 + 弹性主区"的真窗口形状，cap=48；`ImageRenderer` 量高度，
//         `NSHostingView` 复量为 33 / 66pt，差值 ≤1pt）：
//        ViewThatFits + 一行短文  → 这一行 **65pt**（正文 16pt，白白多出 49pt）
//        自定义 Layout + 一行短文 → 这一行 **34pt**（= 正文自然高 + 行内边距）
//        自定义 Layout + 5 行原文 → **67pt**（= 上限 48 + 行内边距）
//        自定义 Layout + 3000 字  → **67pt**（同上：**与正文长度无关**）
//    同屏的常驻行一多，65 vs 34 的差别就是"底部整条还在不在"。
//
// ⚠️ 只有**路径**那种"数据"用 `lineLimit(1)` + 中间截断 + `.help`（见
//    `DownloadDirChangeNotice`）：数据截断是可读性问题，散文截断是**丢证据**问题。
// ---------------------------------------------------------------------------

/// 常驻提示行里**散文**（内核 / 系统原文）的高度上限。
///
/// 取 48pt 的算法：`.callout` 一行约 15–16pt，48pt ≈ **3 行**（实测 3 行 = 45pt，留 3pt 余量）。
/// 常见的内核 / 系统错误是 1–2 行 ⇒ 绝大多数情况下**根本不触发滚动**，
/// 只有 `coreNotFoundError` 那类多行原文才滚 —— 而那正是"必须完整可见"的场合。
/// **数一遍**（第 3、5 轮各更正过一次计数，这里给**可复核、且不会自匹配**的算法 ——
/// 别再凭印象写数字）：
///     grep -rn "BoundedNoticeText(" macos/Sources/ | grep -v "///"   → **5 处**，全在 RootView：
///         引擎横幅正文 / 引擎横幅的补充说明 / 换码结果 / 历史写盘失败 / 下载回执标题
///     grep -rn "BoundedNoticeArea(" macos/Sources/ | grep -v "///"   → **3 个调用点**，其中
///         DownloadDirChangeNotice 那一处有**两个落点**（主区那行 + SettingsView）
///     ⚠️ **`| grep -v "///"` 不能省**：上面这几行注释自己就含 `BoundedNoticeText(`，
///        不加会多算 1 行（实得 6 / 4，而真正的调用点是 5 / 3）。
/// ⇒ **实例数 7**（5 + 2，**跨两个窗口**）；**主区同屏最坏 6**（5 处散文 + 主区那份
///   改目录回执的标题），再加**列表**那一档 1 处（80pt，见下面那个常量）
///   ⇒ **主区同屏最坏 = 6 × 48 + 80 = 368pt**。
///   ⚠️ **实例数 ≠ 同屏数**（第 5 轮改准的）：`SettingsView` 那一块在**另一个窗口**
///      （`App.swift` 的 `Settings` 场景），**永远不可能**与主区那几条行叠在一起 ——
///      拿 7 × 48 去论证单窗口的预算是错的（那正是这条注释上一版犯的错）。
/// 主窗口内容区下界是 640pt（`RootView` 末尾那个 `frame`），368pt 也只是"占掉一半多"、
/// **有界**（不变量成立）；而且那要求引擎不可用 + 换码失败 + 历史写盘失败 + 改目录失败
/// 同时发生 —— 常态是这些行**根本不出现**（没话说就不挂）。
let residentNoticeTextMaxHeight: CGFloat = 48

/// 常驻提示行里**列表**（逐条拒绝理由）的高度上限。
///
/// 取 80pt 的算法：一条拒绝理由约 16–17pt（`.callout` 的路径 + `.caption` 的理由），
/// 80pt ≈ **4–5 条**。比散文那一档宽：列表的价值就在"一眼看出是哪些文件没进去"，
/// 只露 2 条等于逼用户去滚 —— 而"逐条摆出来"是约束 4 对这条回执的硬要求。
/// ⚠️ 上限归上限：**一条都不许藏**（见 `BoundedNoticeArea` 那一段），只是超出后要滚。
let residentNoticeListMaxHeight: CGFloat = 80

/// **高度有界**的一块内容：**放得下就按自然高铺开，放不下就钉在上限上可滚。**
///
/// ⚠️ 这是本文件的核心机制，两个消费者：散文（`BoundedNoticeText`）与列表（拒绝理由）。
///    它的三个性质缺一不可：
///      ① **有界** —— 这块内容的高度永远 ≤ `maxHeight`（不变量）；
///      ② **不丢** —— 放不下时给的是 `ScrollView`（全文/全部条目都在），不是截断；
///      ③ **基线** —— 对外**仍然提供"第一行的 firstTextBaseline"**（见下面那条坑）。
///
/// 实现分两半，**两半都不能省**：
///   · `BoundedNoticeLayout`（自定义 `Layout`）决定**这块内容有多高**：
///     先按"高度提案 = 无限"量出内容的自然高，再取 `min(自然高, 上限)`；
///   · 里面的 `ScrollView` 负责"放不下时还能看到全部" —— 它被放在
///     `min(自然高, 上限)` 那个高度里，内容比它高时就能滚。
///
/// 🔴 **③ 基线那条坑（第 3 轮修的那次回归）**：常驻行里的图标与 × 要贴着**第一行**，
///    所以那些行用的是 `HStack(alignment: .firstTextBaseline)`。**按调用点数清楚**
///    （第 4 轮更正过一次 —— 原来这里写"五个调用点都是"，不成立）：
///      · **4 处** `BoundedNoticeText` 直接在这样的 `HStack` 里：引擎横幅正文、
///        换码结果、历史写盘失败、下载回执标题；
///      · 第 5 处 `BoundedNoticeText`（横幅的**补充说明**，`RootView` 里那一条）
///        在**普通 `VStack`** 里，不吃基线；
///      · `DownloadDirChangeNotice` 的**两个落点**（主区那条 + `SettingsView` 那一段）
///        把标题 Area 包在一层 `VStack` 里 —— 基线还要由那层 `VStack` 再往外透一次。
///    ⇒ 结论不变：**不给 `explicitAlignment`，凡是吃基线的地方就一起塌到底边。**
///    而**自定义 `Layout` 不转发子视图的基线**，`ScrollView` 自己也不提供文本基线
///    （实测：`subviews[ScrollView].dimensions(...)[.firstTextBaseline]` 返回的就是它的
///    **底边**）⇒ 改完之后图标与 × 会掉到**内容底边**。
///    **量化**（`NSHostingView`，宽 700，图标相对行顶的偏移；只能人眼在真窗口里复核）：
///      修复前（裸 `Text`）**+8.0pt** ← 基准（= 行内边距 8，图标贴着内容顶）
///      坏状态（没有 `explicitAlignment`）2 行正文 **+25.0pt**、封顶的长正文 **+43.0pt**
///      修好之后 **+8.0pt** —— **与基准逐位相同**（这组数是复审用真
///      `NSHostingView` 独立复量的，两边差值 ≤1pt）。
///    修法：给 `Layout` 一个**只为量基线而存在的隐藏 `Text`**（子视图 0），
///    在 `explicitAlignment` 里返回它的 `firstTextBaseline`。
///    ⚠️ 别删那个隐藏 `Text`、也别改成去量 `ScrollView` —— 前者让对齐再次塌到底边，
///    后者量到的是底边（实测：ScrollView 报 30.0 = 它的高度）。
///
/// ⚠️ 别把 `ScrollView` 换成 `lineLimit`：那会把多行的内核原文砍掉（见文件头）。
/// ⚠️ 别把 `Layout` 换回 `ViewThatFits` + `.frame(maxHeight:)`：**实测那一版把一行正文
///    的行高从 34pt 撑到 65pt**（贪婪的 `ScrollView` 会吃满上限），见文件头那段实测。
struct BoundedNoticeArea<Content: View>: View {
    let maxHeight: CGFloat
    /// **量"第一行基线"用的字体** —— 必须与这块内容正文实际用的字体一致
    /// （差一个字体会差 1pt 上下，两种字体的 ascent 不同）。
    /// 默认是常驻行的标题字体；`.caption` 那两个调用点（横幅的补充说明、设置窗口）显式传。
    var baselineFont: Font = .callout.weight(.semibold)
    @ViewBuilder var content: Content

    var body: some View {
        BoundedNoticeLayout(maxHeight: maxHeight) {
            // 子视图 0：**只为提供一条真实的第一行基线**，永远不显示（放在画布外 + `.hidden()`）。
            // ⚠️ `.accessibilityHidden(true)` 是**为读屏加的**：这个 `Text` 虽然
            //    `.hidden()` + 零尺寸 + 画布外 + 内容只有一个空格，机制上不该被读到，
            //    但"它到底会不会出现在辅助功能树里"**在本机验证不了**
            //    （取证环境没有辅助功能权限：`AXIsProcessTrusted()` 为假，
            //    `AXUIElementCopyAttributeValue` 返回 -25208，而 `accessibilityChildren()`
            //    对明摆着的可见 `Text` 也返回 0 个元素 ⇒ 这个 API 在这里信息量为零）。
            //    既然证不了，就用这行**零风险**地把它排除掉。
            Text(" ").font(baselineFont).hidden().accessibilityHidden(true)
            // 子视图 1：真正的内容（可滚的那一份）。
            ScrollView(.vertical) { content }
        }
    }
}

/// `BoundedNoticeArea` 的尺子：**高度由内容决定（但封顶）**，并且**对外转发第一行基线**。
///
/// 它做的两件"不平凡"的事：
///   · **自己控制提案**：量自然高时把高度提案设成无限（`height: nil`），
///     所以这个判断**与兄弟节点无关** —— 这正是 `ViewThatFits` 那一版失败的地方
///     （它拿到的提案高度由同一行里的图标与按钮决定）；
///   · **转发基线**：`explicitAlignment` 返回子视图 0（那个隐藏 `Text`）的第一行基线。
///
/// ⚠️ 子视图的**顺序是契约**：0 = 量基线的隐藏 `Text`，1 = 真正的内容
///    （**独子形态的约定、以及"删了隐藏 `Text` 要同时改什么"，写在下面那两个取值函数
///    中间那段注释里 —— 那是唯一一份，别在这里复述出第二份来**）。
///
/// 🔴 **这条契约在代码里有兜底**（第 4 轮加的）：两个下标都走 `guard count > N` 的退化路径。
///    它防的**不是**"今天会崩" —— 今天不会（唯一生产者 `BoundedNoticeArea.body` 的
///    ViewBuilder 恒定两个语句，每次布局实测 `count = 2`）；它防的是**以后有人改了
///    那个 ViewBuilder 的顺序或数量**（例如把隐藏 `Text` 挪到后面、或拆成条件分支）：
///    那时 `subviews[1]` 会**直接下标越界、进程 SIGTRAP 退出 133**（复审实测），
///    而"顺序是契约"这句话原本**只写在注释里**、编译器不管。
///    与其让下一个人在运行时撞上一个没有堆栈的崩溃，不如在这里退化成"能显示就显示"。
private struct BoundedNoticeLayout: Layout {
    let maxHeight: CGFloat

    // -----------------------------------------------------------------------
    // 子视图约定 —— **下面两个函数共用同一条，改一个必须看另一个**（第 5 轮统一的）
    //
    //   · **两个子视图**（正常形态）：0 = 量基线的隐藏 `Text`（探针），1 = 内容；
    //   · **只有一个子视图**：**它就是内容，没有探针**。
    //
    // 为什么这样定：这个 Layout 最可能发生的未来改动是"有人把那个隐藏 `Text` 删了"
    // （它看起来像个多余的怪东西）。那时"独子 = 内容"**仍然是对的**（内容照常显示），
    // 而"没有探针"会诚实地退化成 **`explicitAlignment` 返回 `nil` = 我没有基线**。
    //
    // 🔴 **删掉那个隐藏 `Text` 时必须同时做的事**（不写下来就没人知道）：
    //    `explicitAlignment` 得**另找一条真实的第一行基线**，否则第 3 轮修好的
    //    "图标与 × 贴着第一行"会**无声回归**成"贴到内容底边"——
    //    不崩、不报错、测试也抓不到（视图层没有单测），只有人眼能看出来。
    //    ⚠️ 这里**不许**改成"退化时拿 `subviews[0]`（那多半是 `ScrollView`）当探针"：
    //    实测 `ScrollView` 的 `firstTextBaseline` 返回的是**它的底边**，
    //    那样等于**用一条错的基线冒充对的**（比没有基线更坏：它看起来"有位可贴"）。
    // -----------------------------------------------------------------------

    /// 子视图 1 = 真正的内容；**只有一个子视图时，那个就是内容**（见上面那条约定）。
    private func content(_ subviews: Subviews) -> LayoutSubview? {
        guard !subviews.isEmpty else { return nil }
        return subviews.count > 1 ? subviews[1] : subviews[0]
    }

    /// 子视图 0 = 量基线的隐藏 `Text`；**只有在真的有两个子视图时它才是探针** ——
    /// 独子（= 有人删了那个 `Text`）时**返回 `nil`**：没有探针可量，如实说"我没有基线"，
    /// **不拿 `ScrollView` 的底边冒充**（见上面那条约定里标了 🔴 的那段）。
    /// 没有子视图时同样返回 `nil`（`Layout` 的 `subviews` 不会为空，
    /// 但下标本身要先守住，别让一行防御变成另一个越界点）。
    private func baselineProbe(_ subviews: Subviews) -> LayoutSubview? {
        subviews.count > 1 ? subviews[0] : nil
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        // ⚠️ 宽度**不是**我们要主张的东西：这块内容永远是"给我多宽我用多宽"。
        //    没有宽度提案时一律报 **0**，绝不能报内容的自然宽度 ——
        //    一句 3000 字的内核原文自然宽度是 **21567pt**（实测），照报会把整扇窗口的
        //    **理想宽度**撑到两万点（`.windowResizability(.contentMinSize)` 下，
        //    初始窗口尺寸就跟着走）。高度才是这里唯一要管的事。
        guard let content = content(subviews) else {
            return CGSize(width: proposal.width ?? 0, height: 0)   // 退化：没有子视图 ⇒ 零高
        }
        let natural = content.sizeThatFits(ProposedViewSize(width: proposal.width, height: nil))
        return CGSize(width: proposal.width ?? 0, height: min(natural.height, maxHeight))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        // 量基线那个隐藏 `Text`：**放到画布外、给零尺寸**，它不参与任何可见布局。
        // ⚠️ 只在"真的有两个子视图"时才这么做 —— 退化形态（只有一个）时，
        //    下面那个 `content(subviews)` 拿到的**就是子视图 0**，对它调两次 `place`
        //    意味着**去依赖"对同一个子视图重复 `place` 后者覆盖前者"这个未定义行为**。
        //    实测（第 5 轮，复审的探针）：后者**确实**覆盖前者、内容照常显示、frame 逐位相同；
        //    但那是**观测到的行为、不是文档承诺**，所以这里不省这个条件。
        if subviews.count > 1, let probe = baselineProbe(subviews) {
            probe.place(at: CGPoint(x: -10000, y: -10000),
                        proposal: ProposedViewSize(width: 0, height: 0))
        }
        // 内容按**这块内容自己的高度**放：比上限矮就原样显示（视口 = 内容，滚不动也不需要滚），
        // 比上限高就被钉在上限里 —— 那个高度差就是 `ScrollView` 要滚的距离。
        content(subviews)?.place(at: bounds.origin, anchor: .topLeading,
                                 proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
    }

    /// 对外报"第一行的基线"。
    ///
    /// ⚠️ 量的必须是**子视图 0**（真的 `Text`），不能是子视图 1（`ScrollView`）——
    ///    实测：`ScrollView` 这一项返回的是它的**高度**（= 底边），照着它对齐
    ///    就会把图标与 × 贴到内容底边去。
    /// ⚠️ 退化形态（没有子视图）返回 `nil` = "我没有基线" ⇒ 由外面的对齐容器
    ///    退回默认行为（底边）。兜底只保证**不崩**，不假装能给出一条正确的基线。
    func explicitAlignment(of guide: VerticalAlignment, in bounds: CGRect, proposal: ProposedViewSize,
                           subviews: Subviews, cache: inout ()) -> CGFloat? {
        guard guide == .firstTextBaseline, let probe = baselineProbe(subviews) else { return nil }
        return probe.dimensions(in: .unspecified)[VerticalAlignment.firstTextBaseline]
    }
}

/// 常驻提示行里的**散文正文**（内核原文 / 系统错误原文 / 壳写的通告）。
///
/// ⚠️ 可选中复制（约束 3 / C-7）：客户要能把这段话原样发给业务方。
/// ⚠️ `.help(text)` 与滚动视口**并存**：滚动手势不方便时，悬停也能看到全文。
struct BoundedNoticeText: View {
    let text: String
    var font: Font = .callout.weight(.semibold)

    var body: some View {
        // ⚠️ `baselineFont` 必须跟着 `font` 走：图标与 × 是**按第一行基线**贴的，
        //    量错了字体（`.callout` vs `.caption` 的 ascent 不同）就会差 1pt 上下。
        BoundedNoticeArea(maxHeight: residentNoticeTextMaxHeight, baselineFont: font) {
            Text(text)
                .font(font)
                .textSelection(.enabled)
                .help(text)
        }
    }
}

/// 「改下载目录」那条回执的**正文块**：标题一行 + 路径一行。
///
/// 🔴 **为什么抽成一个类型**：这条回执有**两个落点** ——
///    主区那条常驻行（`RootView.downloadDirChangeBar`）与设置窗口那一段
///    （`SettingsView`）。两处显示的是**同一条回执**，而"路径必须单独一行、
///    固定一行高、中间截断、悬停看全文"这条规矩**必须一致**：
///    各写一份的结局是"改了一处、另一处照旧把长路径整句铺开"——
///    那次布局事故就会在设置窗口里**原样重演**（设置窗口更窄，只会更早出问题）。
///    所以正文块只此一份，两处都用它。
///
/// ⚠️ **图标与 × 按钮留给调用点**（两处**有意**不同，不是漏抽）：
///    主区那行用 `folder.badge.gearshape`（一条状态回执），设置窗口那一段用
///    `checkmark.circle`（用户刚在这里按了确认，要的是"成了"）；两处的字号
///    （`.callout` vs 表单里的 `.caption`）也由调用点通过 `titleFont` 传进来。
///    这些差异是**呈现**的差异，不是**规矩**的差异 —— 规矩（截断 / 上限）在这一个类型里。
struct DownloadDirChangeNotice: View {
    let change: DownloadDirChange
    var titleFont: Font = .callout.weight(.semibold)

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            // ① 固定短的标题。`.failed` 那一支是内核 / 系统原文 ⇒ 用有界可滚的散文
            //    （**不许截断**：失败原文必须完整可见，约束 4 / C-7）。
            BoundedNoticeArea(maxHeight: residentNoticeTextMaxHeight, baselineFont: titleFont) {
                Text(change.headline)
                    .font(titleFont)
                    .textSelection(.enabled)
                    .help(change.headline)
            }
            // ② 路径**单独一行**（这次的修复落点）：固定一行高，
            //    中间截断（头尾才是有辨识度的部分：哪个卷、哪一层），悬停看完整路径。
            if let path = change.pathDetail {
                Text(path)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)     // 完整路径可选中复制（约束 3 / C-7）
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(path)
            }
        }
    }
}
