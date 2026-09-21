import SwiftUI
import BenagenCoreKit

/// 换码面板：工具栏那颗**写着当前交付码**的按钮打开它（规格 §7.1「交付码显示 + 重新加载」）。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：
///   - 「这次要加载哪个码」沿用现成的 `DeliveryCodeEntry.resolve`（`Presentation/`，**已有单测**），
///     不另写一套解析；
///   - 「失败该说什么」写在 `AppModel.switchDelivery` 的返回值 `DeliverySwitch` 里 ——
///     那句话就是内核原文（或壳在"内核一个字都没说"时的说明），本文件**一个字都不编**（约束 3 / C-7）；
///   - 下面那个 `if case .failed` 与那条 `onChange` 是**渲染分派**（成功了关面板、
///     失败了把内核原文摆在面板里），不是值计算。
///
/// **阶段 E 起多了历史列表这一段**（规格 §1.4，「历史批次」）：每一行上屏的字
/// （备注为空时回落到码、时间、顺序、`base_url`）全部由
/// `BatchHistoryRow`（`Presentation/`，**有单测**）算好，本文件只做四件渲染分派的事：
///   - **点一行 = 直接加载那一条**（`switchTo`，与输入框那条路各走各的），
///     `base_url` 跟着一起发 —— 见那边注释里那条 Ruling C10 的坑；
///   - 备注就地编辑（回车 / 失焦 / 面板关闭三个提交点，见 `commitNote`）；
///   - **列表为空时整段不渲染**（`historySection`）；
///   - **历史写盘失败就地显示**（`historySection` 里那行；主区另有一行常驻提示，
///     两处读的是同一个字段 —— 最终审查重要 1）。
///
/// ⚠️ 视图不单测（规格 §10.4）：行为靠 `macos/README.md` 第 19 条与**第 19c 条**手工验收。
struct SwitchDeliverySheet: View {
    @ObservedObject var model: AppModel

    /// 关面板的出口。**只在成功时**用它（失败要留在原地显示内核原文）。
    @Environment(\.dismiss) private var dismiss

    /// 输入框里的码。初始值 = **当前批次码**（见下面 `onAppear`）。
    @State private var code: String = ""
    /// 面板**这一次**换码的结果。nil = 还没点过「加载」（或刚打开面板）。
    ///
    /// ⚠️ 它是一份**面板本地的镜像**，不是权威副本 —— 权威副本在
    ///    `AppModel.lastSwitchOutcome`（`switchDelivery` 每次都会写），由主区那行常驻提示
    ///    呈现。为什么面板不直接读模型那一份：`@State` 在**面板呈现那一刻**才建，
    ///    而模型那份可以带着**上一轮**的结果进面板（一进来就显示上一轮的结论，
    ///    成功那支还会瞬间把面板自己关掉）。两份来自**同一次调用**的返回值，
    ///    不会分叉；面板关了，模型那份照样在。
    @State private var switchOutcome: DeliverySwitch?

    /// 历史列表里那些**还没交出去的备注草稿**。
    ///
    /// ⚠️ 为什么要草稿而不是直接改模型：`model.setHistoryNote` 是**同步写盘**的，
    ///    逐字符绑上去等于每敲一个字写一次文件 —— 这里只在**回车 / 这一行失焦 /
    ///    面板关闭**三个点交出去（三个点缺一不可，各见 `commitNote` 与 `onDisappear`）。
    /// ⚠️ **判决全在 [`NoteDrafts`] 里**（`Presentation/`，有单测）：什么时候提交是视图的事
    ///    （渲染分派），"这一次该不该写、写什么"是值计算。
    @State private var noteDrafts = NoteDrafts()

    /// 现在**焦点在哪一行的备注框里**（`nil` = 不在任何一行里）。用来在焦点离开时提交。
    @FocusState private var editingNote: String?

    /// **上一次**焦点在哪一行 —— 只给下面那条 `onChange` 用。
    ///
    /// ⚠️ 为什么需要它：部署目标 13.0 下只有**单参** `onChange(of:perform:)`
    ///    （双参形式要 14.0），而单参闭包拿到的是**新值**（SDK：
    ///    `perform action: @escaping (newValue: V) -> Void`）。
    ///    这里要的却是**旧值**（刚离开的那一行），所以自己记一个。
    ///    ⚠️ **别改成"直接用单参的新值"**：那会把备注写进**刚获得焦点的那一行**里
    ///    —— 静默、而且写错行。
    @State private var lastNoteFocus: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("加载另一个交付码")
                .font(.title3.weight(.semibold))

            // 后果说明**常驻**，每一条都是：
            //   - **不依赖 `model.transfers`**：它只在用户进过传输列表之后才有值（200 ms 轮询跟着
            //     分区可见性走），拿它判断"有没有任务在跑"会把这句话吞掉 —— 而内核手里明明有一堆；
            //   - **壳自己写的**（这是有意偏离"只登内核原文"，约束 11）：内核不会为一次**还没
            //     发生**的操作主动说话，而这句后果必须在用户点「加载」**之前**就在屏幕上。
            //     事实依据是内核的明文设计（`core/src/main.rs` 的 `clear_engine_batch`：
            //     换码时逐个 remove 在跑的任务，**不碰 download_dir**）。
            Text("换码会把当前的下载任务从引擎里摘掉（含等待中的）；已经下载到磁盘的文件不会被删除。")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            TextField("交付码", text: $code)
                .textFieldStyle(.roundedBorder)
                .disabled(model.switching)
                .onSubmit(load)

            // 过长时**立刻**说清楚为什么（不是只把按钮变灰）：约束 4 要求用户看得见
            // "为什么按不动"。文案是 `DeliveryCodeEntry.tooLongHint` —— 壳写的，
            // 理由（内核收不到这条请求、没有原文可登）写在那边的注释里。
            if codeTooLong {
                Label(DeliveryCodeEntry.tooLongHint, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            // 失败原文**留在面板里**（约束 4：不得静默失效）。可选中复制（约束 3 / C-7）：
            // 客户要能把这句原话发给业务方。
            if let outcome = switchOutcome, case .failed(let message) = outcome {
                Label {
                    Text(message)
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                }
                .font(.callout)
                .foregroundStyle(.red)
                .multilineTextAlignment(.leading)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            // 历史批次（规格 §1.4）。空列表时**这一段整个不出现** —— 见 `historySection`。
            historySection

            HStack(spacing: 8) {
                Spacer(minLength: 8)
                // ⚠️ 「取消」**有意不禁用**（`AppModel.switching` 的注释里写了理由）：
                //    面板关不关得掉，不该由一次网络请求决定。代价是那次换码的结果
                //    会失去面板这个落点 —— 所以结果**同时**落在 `model.lastSwitchOutcome`
                //    上，由主区那一行常驻提示呈现（失败：内核原文；成功：已换到批次 X）。
                Button("取消") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("加载", action: load)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    // 正在换的时候禁用（`model.switching`）：一次只发一条 —— 防连点。
                    // 另两个条件是"没有码可发"（空/没解析出码，规则在 `codeToLoad` 里）
                    // 与"码太长"（`codeTooLong`，约束 C-3：超限的请求内核不报错、
                    // 只把客户端**静默堵死**）。两者都会在上面给出可见的说明。
                    .disabled(model.switching || codeToLoad.isEmpty || codeTooLong)
            }
        }
        .padding(20)
        .frame(width: 440)
        // 输入框的初始值 = **当前批次码**（简报明文）。
        // ⚠️ 用 `onAppear` 而不是在 `init` 里写 `@State` 初值：`View` 的 `init` 不是主 actor
        //    隔离的，而 `model.loadState` 是（`AppModel` 是 `@MainActor`）。`onAppear` 在面板
        //    呈现那一刻只跑一次，之后用户敲进去的字不会被它覆盖（判据是 `code.isEmpty`）。
        .onAppear { if code.isEmpty { code = currentCode } }
        // 成功 ⇒ 关面板。**新批次由 `RootView.onChange(of: loadedCode)` 自动复位**
        // （清浏览位置与勾选面），这里不重复做那件事。
        // 失败 ⇒ 什么也不做：面板不关，上面那段原文就摆在用户眼前。
        // 单参形式（13.0 下唯一可用的那个），拿到的是新值 —— 这里要的正是它。
        .onChange(of: switchOutcome) { outcome in
            if case .switched = outcome { dismiss() }
        }
        // 焦点**离开**某一行 ⇒ 把那一行交出去（回车那条路走 `onSubmit`）。
        // ⚠️ 两条路都要：只留 `onSubmit` 的话，用户敲完备注直接去点另一行/关面板，
        //    **那一句备注就丢了**，而界面上一个字都不说（约束 4 明禁的静默失效）。
        // 🔴 这里是 6 处里**唯一**不能照搬单参写法的地方：要的是**旧值**（刚离开的那一行），
        //    而 13.0 唯一可用的单参形式给的是**新值** —— 照抄 = 把备注提交到
        //    **刚获得焦点的那一行**上（静默、写错行）。见 `lastNoteFocus` 的注释。
        //    ⚠️ 它丢了也不丢字：面板关闭那条路是**兜底**（`onDisappear` 会把还挂着的
        //       草稿全部交出去），所以这一格复位最多让"提前提交"这一次不发生。
        .onChange(of: editingNote) { focus in
            if let previous = lastNoteFocus { commitNote(previous) }
            lastNoteFocus = focus
        }
        // 面板**关掉**时把还挂着的草稿全部交出去。
        //
        // ⚠️ 这一条不是"多一层保险"，它是**唯一的兜底**：用户敲完备注**直接关面板**
        //    （点「取消」、按 Esc、点输入框再回车……）时，上面那条焦点变化**不保证**会跑
        //    （面板正在被销毁）—— 少了它，那句备注就**无声地丢了**，
        //    而「重启后仍在」正是这一段功能的验收项。静默丢用户敲进去的字是约束 4 明禁的那类失效。
        .onDisappear { commitAllNoteDrafts() }
    }

    // MARK: - 历史列表（规格 §1.4）

    /// 屏上要画的历史行。**顺序就是历史给的顺序**（视图不排 —— 见 `BatchHistoryRow.rows`）。
    private var historyRows: [BatchHistoryRow] {
        BatchHistoryRow.rows(model.history)
    }

    /// 「历史批次」那一段。**空列表时返回空视图**（规格 §1.4：不要给一个空盒子）——
    /// 判据是 `historyRows.isEmpty`，不是"模型里有没有历史"：视图只关心**它要画的那一份**。
    ///
    /// ⚠️ **历史写盘失败那行挂在这里**（最终审查重要 1；主区那行常驻提示是它的第二落点，
    ///    两处读的是同一个字段 `AppModel.historyWriteFailure`）。为什么就地也要有一份：
    ///    "敲完一句备注 → 写盘失败"这条路上，用户正对着这个面板 —— 而主区那行在模态面板
    ///    **背后**，他要先关掉面板才看得见。落点不能只在窗口背后那一个。
    @ViewBuilder
    private var historySection: some View {
        if !historyRows.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text("历史批次")
                    .font(.callout.weight(.semibold))

                // 最多 50 条（`BatchHistory.maximumEntries`），所以给一个上界高度、
                // 超了就在这一段里滚 —— 否则列表会把底下的「取消 / 加载」顶出屏幕。
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(historyRows) { row in
                            historyRow(row)
                        }
                    }
                    .padding(.vertical, 1)   // 给行的圆角留一点余量，免得描边被 ScrollView 裁掉
                }
                .frame(maxHeight: 220)

                // ⚠️ **历史写盘失败必须在这里说得出来**（最终审查重要 1）：这一段的
                //    承诺就是"重启应用后仍在"，而写盘失败时那句承诺**是假的** ——
                //    不说的话，用户敲完备注、关掉面板、重启，备注没了，**无从归因**。
                //    样式与同屏那些提示同款（警告色 + 原文可选中 + 收起）。
                //    ⚠️ 原文来自 `AppModel.writeHistory`（壳自己拼的 + 系统错误文本）——
                //    这件事内核一个字都没说，所以没有"内核原文"可登，视图也不编（约束 3）。
                if let failure = model.historyWriteFailure {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundStyle(Color.orange)
                        Text(failure)
                            .font(.caption)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 8)
                        Button {
                            // 与主区那行提示是**同一个值、同一个出口**。
                            model.dismissHistoryWriteFailure()
                        } label: {
                            Image(systemName: "xmark")
                        }
                        .buttonStyle(.borderless)
                        .help("收起这条提示")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }

                Text("点一行即可切换到那一批；备注写在这里，重启应用后仍在。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// 历史列表里的一行：**点它就直接加载这一条** + 就地编辑备注。
    ///
    /// ⚠️ **点击 ≠ "填进输入框再点加载"**（规格 §1.4，人类伙伴的原话是"可以直接通过选择
    ///    进行切换"）。所以这一颗 `Button` 的动作是 `switchTo(_:)`，与输入框那条路各走各的。
    ///
    /// ⚠️ 上屏的字**一个都不在这里拼**：`title` / `code` / `timeText` 都是
    ///    [`BatchHistoryRow`] 算好的（全局约束 8）。
    ///
    /// ⚠️ **有意不把"显示"与"编辑"合二为一**（E-8；有备注时同一句话会出现两次，那是想过的）：
    ///    合成一个"`prompt` 就是备注/码"的编辑框更省地方，但代价是真的 ——
    ///    ① 这一行**面积最大的那块**会从"点一下切过去"（规格 §1.4 的头条要求、最常做的事）
    ///       变成"点一下进编辑态"，把命中率最高的区域从事务性操作改成了改备注；
    ///    ② `prompt` 是**占位符不是值**：灰的、一聚焦就消失、**不可选中复制** ——
    ///       而"看到这个码、把它复制走"正是这一行要支持的用法。
    private func historyRow(_ row: BatchHistoryRow) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                switchTo(row)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title)
                        .font(.callout)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    HStack(spacing: 6) {
                        // ⚠️ `lineLimit(1)` 是**像素**上的事，不是**值**上的事：
                        //    `BatchHistoryRow.code` 仍然逐字不截断（约束 3），这里只是不让一个
                        //    手改出来的几千字符的码把这一行渲染成一大坨折行的等宽字。
                        //    中间截断 + `.help(原文)`：看得见首尾、悬停能看到全部。
                        Text(row.code)
                            .font(.caption.monospaced())
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(row.code)
                        Text(row.timeText)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                // 整块可点：`Text` 本身只有字形那一小块能吃点击（同 `EmptyState` 的做法）。
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            // 正在换码时禁用它 —— 与「加载」那颗按钮同一条纪律（一次只发一条，防连点）。
            // ⚠️ 过长的码**发不出去**（约束 C-3）也在这里挡住：`history.json` 是用户改得动的
            //    文件，而这条请求超限的后果是内核那条 FIFO 被**永久堵死**。
            //    `.help` 给的是**既有**的那句话，视图不另编（`EngineGate.unavailableHelp`
            //    是同一个做法：禁用 + 把"为什么"挂在 `.help` 上）。
            .disabled(model.switching || !row.isSendable)
            .help(row.isSendable ? "加载这一批" : DeliveryCodeEntry.tooLongHint)

            // 就地一行文本输入（规格 §1.4）。空串 = 没写备注。
            TextField("备注", text: noteBinding(row))
                .textFieldStyle(.roundedBorder)
                .font(.caption)
                .focused($editingNote, equals: row.code)
                .onSubmit { commitNote(row.code) }
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 6).fill(.quaternary.opacity(0.4)))
    }

    /// 这一行的备注框绑定到的那个串：**先看草稿，没有草稿就是模型里那一份**。
    /// （判决在 `NoteDrafts.text(forCode:current:)` 里，有单测。）
    private func noteBinding(_ row: BatchHistoryRow) -> Binding<String> {
        Binding(get: { noteDrafts.text(forCode: row.code, current: row.note) },
                set: { noteDrafts = noteDrafts.editing($0, forCode: row.code) })
    }

    /// 某个码**现在**在模型里的备注；`nil` = 这个码已经不在历史列表里了。
    private func currentNote(_ code: String) -> String? {
        historyRows.first { $0.code == code }?.note
    }

    /// 把这一行的备注草稿交出去（回车、或焦点离开这一行时各调一次）。
    private func commitNote(_ code: String) {
        apply(noteDrafts.committing(code: code, current: currentNote(code)))
    }

    /// 把**还挂着的所有**草稿交出去（面板关闭时跑一次）。
    ///
    /// ⚠️ 这一条是**唯一的兜底**：用户敲完备注**直接关面板**（点「取消」/ Esc）时，
    ///    上面那条焦点变化**不保证**会跑（面板正在被销毁）—— 少了它，那句备注就
    ///    **无声地丢了**，而"重启后仍在"正是这一段功能的验收项。
    private func commitAllNoteDrafts() {
        apply(noteDrafts.committingAll(current: currentNote))
    }

    /// 把一次判决的结果落地。⚠️ **两件都要做**：只写不装回草稿，编辑框会一直显示用户敲的
    /// 原文（与真正存下去的那一份分叉）；只装回不写，备注就丢了。
    private func apply(_ outcome: NoteDrafts.Outcome) {
        noteDrafts = outcome.drafts
        for write in outcome.writes {
            model.setHistoryNote(write.note, forCode: write.code)
        }
    }

    // MARK: - 绑定（没有值计算）

    /// 当前生效的批次码 —— 输入框的初始值就是它。没有生效批次时是空串
    /// （那种情况下工具栏那颗按钮是禁用的，面板打不开）。
    private var currentCode: String {
        if case .loaded(let info) = model.loadState { return info.code }
        return ""
    }

    /// 这次要加载的码：用户敲的优先，输入框为空时回落到**当前批次码**
    /// （规则与理由见 `DeliveryCodeEntry.resolve` —— 与空态页同一个实现，不另写一套）。
    ///
    /// ⚠️ `resolve` **只做"用哪个"的回落**，它**不做任何解析/校验**（源码就是
    ///    `typed.isEmpty ? remembered : typed`）—— 面板与空态页的合法性判据在**下面那一条**
    ///    （`codeTooLong`）与 `AppModel.performLoadDelivery` 的闸门上，别指望 `resolve` 挡什么。
    private var codeToLoad: String {
        DeliveryCodeEntry.resolve(typed: code, remembered: currentCode)
    }

    /// 这次要发的码**太长**吗（判据与理由见 `DeliveryCodeEntry.tooLong`）。
    /// 它同时是那颗按钮的禁用条件与上面那句提示的显示条件 —— **同一个判据**，
    /// 不会出现"按钮灰着、界面不说什么"（约束 4）。
    private var codeTooLong: Bool {
        DeliveryCodeEntry.tooLong(codeToLoad)
    }

    /// 发请求（约束 17：`AppModel` 的方法都是 `async`，视图用 `Task { await … }` 调）。
    ///
    /// ⚠️ `baseURL: nil` = 不带 `base_url` 键 ⇒ 内核用它自己的默认交付服务器
    ///    （与空态页「高级」收起、以及今天的自动加载同一条口径）。
    private func load() {
        // 「加载」按钮与输入框在 `model.switching` 时都是禁用的（见 `body`），但**回车**不经过
        // 那颗按钮的禁用状态 —— 这一句把那条缝堵上：一次只发一条。
        guard !model.switching else { return }
        let target = codeToLoad
        guard !target.isEmpty else { return }
        // 过长的码**不发**（约束 C-3）。**不是静默**：这一刻上面那句提示已经摆在屏幕上了。
        guard DeliveryCodeEntry.isSendable(target) else { return }
        Task {
            // 上一轮的结果先清掉：面板上不该留着一句属于**上一次**尝试的错误。
            switchOutcome = nil
            switchOutcome = await model.switchDelivery(code: target, baseURL: nil)
        }
    }

    /// 点历史里的一行 = **直接加载那一条**（规格 §1.4）。
    ///
    /// ⚠️⚠️ **`baseURL` 必须跟着一起传下去**（本任务最容易漏的一条）：
    ///    这一条记着它上次是从哪台交付服务器加载的（`BatchHistoryEntry.baseURL`），
    ///    不带它，从**自定义**服务器加载过的批次再点一次就会去问**默认**服务器 ——
    ///    拿到的是"码不存在"之类的失败，而用户刚刚明明看见它列在历史里。
    ///    这正是阶段 C 的 Ruling C10 记下的那个坑（当时判定"不补"，因为规格没要求）；
    ///    现在历史里记了 `base_url`，这个坑**由历史来填**。
    ///
    /// ⚠️ `baseURLOrNil` 空串 ⇒ `nil` ⇒ 请求里不出现 `base_url` 键 ⇒ 内核用自己的默认值
    ///    （与输入框那条路、以及启动时的自动加载同一条口径，见 E-5）。
    private func switchTo(_ row: BatchHistoryRow) {
        // 「加载」与输入框在 `model.switching` 时都是禁用的，这一颗按钮也是（见 `historyRow`），
        // 但这里再挡一次：一次只发一条（`switchDelivery` 里还有一道结构性的重入闸）。
        guard !model.switching else { return }
        guard row.isSendable else { return }
        Task {
            // 上一轮的结果先清掉：面板上不该留着一句属于**上一次**尝试的错误。
            switchOutcome = nil
            switchOutcome = await model.switchDelivery(code: row.code, baseURL: row.baseURLOrNil)
        }
    }
}
