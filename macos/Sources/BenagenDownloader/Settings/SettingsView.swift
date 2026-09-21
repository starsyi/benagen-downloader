import SwiftUI
import AppKit               // NSOpenPanel：选下载目录那一下
import BenagenCoreKit

/// 设置窗口（⌘,）：**壳自己的**下载目录 + 七个可调参数 + 开源许可入口。
///
/// ⚠️ **这是客户唯一能改内核参数的地方**（规格 §7.1 工具栏的「设置」、菜单
///    `CommandGroup(replacing: .appSettings)` 都指向这个场景）。它做四件事：
///      ① 「下载目录」（阶段 E §2）—— **壳自己的偏好**，与内核参数面板无关；
///      ② 把**内核手里那一份**参数摊成可编辑的表（初值来自 `get_settings` 的回执，
///         不是壳里的一份默认值 —— 壳不造值）；
///      ③ 「保存」→ `AppModel.applySettings(_:)` → `set_settings`；
///         失败时**内核原文照登**（`invalid_params` 那句"并行文件数必须在 1–64 之间，
///         当前 99"就长在这里，客户照着就能改）；
///      ④ 「开源许可…」→ `LicenseView`（GPLv2 全文，**分发义务**）。
///
/// ⚠️ **「下载目录」那一段在 `if let form` 之外**（任务 3 的一个判断）：内核参数面板
///    要等握手才画得出来，而"改下载目录"恰恰是**内核起不来时**最可能要去改的一件事
///    （状态文件就在下载目录里）。把它塞进 `SettingsEditor` 的话，内核一挂它就跟着消失
///    —— 而那正是用户最需要它的时候。
///
/// ⚠️ 本文件**只有绑定与渲染分派**（全局约束 8）：七个值的来龙去脉、
///    `-k` 的枚举面、七个线上键、六个区间、那两句有语义的文案，全部在
///    `BenagenCoreKit/Presentation/SettingsForm.swift` 里（有单测）；
///    下载目录的判据与文案在 `Presentation/DownloadDirectory.swift` 里（有单测）。
///    这里不映射 `CoreError`（走 `AppModel.message(of:)`，那是唯一实现）。
///
/// ⚠️ **`last_code` 不出现在这一屏**（阶段 A 裁决 #25）：它是运行状态，不是用户参数。
struct SettingsView: View {
    @ObservedObject var model: AppModel

    /// 表里的那一份。`nil` = 内核还没把参数交过来（握手还没完 / 内核起不来）。
    /// **不填默认值**：壳里没有"客户端认为合理的一组参数"，那组数是内核的事。
    @State private var form: SettingsForm?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // -------------------------------------------------------------------
            // ⚠️ **这里只有一个 `Form`**（2026-09-20 现场修复）。改之前是**两个**：
            //    这一行一个 `Form { DownloadDirectorySection }`，而 `SettingsEditor`
            //    内部**又是一个** `Form`（七项参数），两个平级叠在 `VStack` 里。
            //    `Form/.formStyle(.grouped)` 在 macOS 上是**贪心**的（它背后是一个会占满
            //    所给空间的列表），两个贪心子视图在 `VStack` 里会**对半分高度** ——
            //    于是只装「下载到」一行的那个表单占了**半个窗口**，那一行下面是一大片空白，
            //    参数表被挤到下半屏（客户的原话："『下载到』指定目录，在弹框中高度比较高，
            //    导致与下面的设置内容，有很多的空白"）。
            //
            //    ⚠️ 合成一个 `Form` **不动**「下载目录那一段永远在」这条设计
            //    （文件头 §「下载目录」那一段的①）：它仍然是 `if let form` **之外**的
            //    第一个 `Section`，内核起不来、参数还没到手时照样画得出来。
            // -------------------------------------------------------------------
            Form {
                // ① 壳自己的偏好。**永远在**（理由见文件头那一段）。
                DownloadDirectorySection(model: model)

                // ② 内核那份参数。⚠️ `Binding($form)` 是 SwiftUI 自带的"可选 Binding 拆包"：
                //    编辑器的 `Stepper` / `Picker` / `TextField` 都要**非可选**的
                //    `Binding<SettingsForm>`。
                if let form = Binding($form) {
                    SettingsParameterSections(form: form)
                } else {
                    Section { unavailable }
                }
            }
            .formStyle(.grouped)

            // 保存条（含「开源许可…」入口）**只在拿到内核那份参数之后**才存在 ——
            // 与改之前逐字相同：那时 `SettingsEditor` 整个不渲染，它内部的 footer 自然也不在。
            if let form = Binding($form) {
                Divider()
                SettingsFooter(form: form, model: model)
            }
        }
        .frame(minWidth: 520, minHeight: 460)
        .onAppear { seed() }
        // 内核手里那份变了就重新播种：保存成功后的**回执**要接管表单（`-k` 会被内核
        // 归一成规范串，客户看得见它变了）；内核崩溃重启之后也是这一条路。
        // ⚠️ **单参形式**：部署目标是 13.0 时它是**唯一**可用的写法（双参数形式要 14.0）。
        //    单参闭包拿到的**是新值**（SDK：`perform action: @escaping (newValue: V) -> Void`）
        //    —— 这里两个值都不用，所以无所谓。
        //    ⚠️ 部署目标哪天回到 ≥14，这一行会出弃用告警（约束 12 会红），到时改双参。
        .onChange(of: model.settings) { _ in seed() }
    }

    /// 用**内核手里那一份**重新填表，并接上内核给的 `-k` 枚举面。
    ///
    /// ⚠️ 两样都从 `model` 取：`settings` 是 `get_settings` 的回执，
    ///    `minSplitSizeChoices` 是 `hello` 给的（契约 §2.3）—— 壳不生成枚举面，
    ///    也不在内核没给值的时候编一份出来（编出来的表现是"客户看到一组他从没设过的参数"）。
    private func seed() {
        guard let settings = model.settings else { return }
        form = SettingsForm(settings: settings, choices: model.minSplitSizeChoices)
    }

    /// 内核还没交出参数面板。**不画一张空表**：空表会让客户以为参数全没了，
    /// 而真实原因是"还没问到"。原因照登（`EngineStatusPresentation.text` —— 徽标上那句）。
    private var unavailable: some View {
        VStack(spacing: 8) {
            Image(systemName: "gearshape")
                .font(.largeTitle)
                .foregroundStyle(.tertiary)
            Text("内核还没有交出参数面板")
                .font(.callout)
            Text(EngineStatusPresentation.text(for: model.engine))
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        // ⚠️ 这里**没有** `maxHeight: .infinity`（2026-09-20 改）：它现在长在 `Form` 的
        //    一个 `Section` 里，而 `Form` 的行本来就只有内容那么高 —— 硬要无限高
        //    就是把刚修掉的那个"贪心撑开"换个地方复现。用 `minHeight` 给一块像样的空地。
        .frame(maxWidth: .infinity, minHeight: 200)
        .padding(24)
    }
}

// ---------------------------------------------------------------------------
// 下载目录（阶段 E §2，任务 3）
// ---------------------------------------------------------------------------

/// 「下载目录」那一段：**当前路径只读显示 + 「选择…」+ 「恢复默认」**（规格 §2.1）。
///
/// 交互链（每一步的判据都在 `Presentation/DownloadDirectory.swift`，有单测）：
///   选目录 → ① `check(_:)` 先检查（不可用 ⇒ 就地报错并中止，§2.3）
///          → ② `confirmation(from:to:)` 让用户确认三条后果（E-6）
///          → ③ `AppModel.changeDownloadDir(to:)`（落盘 → 重启内核 → 重载当前批次）
///          → ④ 回执落在 **`AppModel.downloadDirChange`** 上（渲染的是它的
///            `headline` + `pathDetail` 两行；`noticeText` 是整句话、**界面上不显示**），
///            这一屏这一段与 `RootView` 那行常驻提示**读的是同一个值** ——
///            窗口关掉了那句话也还在（最终审查重要 2）。
///
/// ⚠️ **只有一条 `.alert`**：同一个视图上挂两个 `.alert` 是 SwiftUI 里出了名的
///    "只有一个真的出得来"，而这两个弹窗（不可用 / 确认）都**必须**出得来 ——
///    所以它们由同一个 `Prompt` 枚举驱动。
private struct DownloadDirectorySection: View {
    @ObservedObject var model: AppModel

    /// 屏幕上那一个弹窗（`nil` = 没有）。
    @State private var prompt: Prompt?
    /// 一次改动在飞：按钮禁用 + 转圈（改目录要重启内核，最长几秒）。
    @State private var changing = false

    /// 最近一次改动的回执。**读模型那一份，不是这里的 `@State`**（最终审查重要 2）：
    /// 设置窗口是随手就会被关掉的东西，而这条回执里有一格是"**没改成**"
    /// （在飞的重启挡住了这一次 ⇒ 内存与盘上都是新目录、而内核还在用旧目录跑）——
    /// 那句话是唯一的提示，窗口一关它不能跟着消失。
    /// 权威在 `AppModel.downloadDirChange`，`RootView` 那行常驻提示读的是**同一个值**
    /// （两个落点、同一个值，不会分叉）。
    private var outcome: DownloadDirChange? { model.downloadDirChange }

    private enum Prompt: Equatable {
        /// 选中的这个目录不能用（§2.3：**在确认之前**就报错并中止）。
        case problem(String)
        /// 确认改目录（E-6 的三条后果）。
        case confirm(String)
    }

    var body: some View {
        Section {
            LabeledContent("下载到") {
                HStack(spacing: 8) {
                    Text(DownloadDirectory.display(model.downloadDir))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(DownloadDirectory.display(model.downloadDir))
                    if changing { ProgressView().controlSize(.small) }
                    Spacer(minLength: 8)
                    Button("选择…") { choose() }
                        .disabled(changing)
                        .help("挑一个文件夹，交付文件会下到这里（改完会重启内核）")
                    Button("恢复默认") { ask(to: "") }
                        .disabled(changing || !isConfigured)
                        .help("不再指定下载目录，由内核用它自己的默认值（改完会重启内核）")
                }
            }
            if let outcome {
                // 回执（🔴 这三句是壳自己写的：内核不会为一次成功的重启主动说话）。
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: outcome.isFailure ? "exclamationmark.triangle"
                                                        : "checkmark.circle")
                        .foregroundStyle(outcome.isFailure ? Color.red : Color.green)
                    // 🔴 **这一处曾经是那次布局事故的另一个现场**（第 2 轮修的）：
                    //    它渲染的是与主区那条常驻行**同一条回执**
                    //    （`AppModel.downloadDirChange`；两处都走 `DownloadDirChangeNotice`），
                    //    同样含**用户选的完整路径**、同样没有高度上限 ——
                    //    而设置窗口比主区更窄，长路径在这里只会更早把窗口撑高。
                    //    ⇒ 正文块用**同一个** `DownloadDirChangeNotice`（标题一行 + 路径一行，
                    //    截断与上限的规矩只此一份，两处不可能漂移）；
                    //    只有**呈现**不同：这里字号是表单的 `.caption`，图标是"刚按完确认"
                    //    的绿色对勾（主区那条用文件夹图标），那两样留在调用点。
                    DownloadDirChangeNotice(change: outcome, titleFont: .caption)
                    Spacer(minLength: 8)
                    Button {
                        // 收起的是**模型那一份**（这一屏与 `RootView` 那行提示是同一个值，
                        // 所以两处的 × 都走这一个出口）。
                        model.dismissDownloadDirChange()
                    } label: {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.borderless)
                    .help("收起这条提示")
                }
            }
        } footer: {
            Text(DownloadDirectory.sectionNote)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .alert(alertTitle,
               isPresented: Binding(get: { prompt != nil }, set: { if !$0 { prompt = nil } }),
               presenting: prompt) { p in
            switch p {
            case .problem:
                Button("知道了", role: .cancel) { prompt = nil }
            case .confirm(let target):
                Button("取消", role: .cancel) { prompt = nil }
                // ⚠️ 这颗按钮的标题把**后果**再说一遍（"重启内核"），而不是一句
                //    空泛的「确定」—— 用户点它之前要能看出自己答应的是什么。
                Button("更改并重启内核") { Task { await apply(target) } }
            }
        } message: { p in
            switch p {
            case .problem(let text):
                Text(text)
            case .confirm(let target):
                // E-6 的三条后果（逐字来自 `DownloadDirectory.confirmation`，**有单测**）。
                Text(DownloadDirectory.confirmation(from: model.downloadDir, to: target))
            }
        }
    }

    private var isConfigured: Bool {
        AppPreferences(downloadDir: model.downloadDir).isConfigured
    }

    private var alertTitle: String {
        switch prompt {
        case .problem: return "这个文件夹不能用"
        case .confirm: return "更改下载目录？"
        case nil: return ""
        }
    }

    /// 发起一次改动（`target` 空串 = 恢复默认）：先检查、再确认。
    private func ask(to target: String) {
        if let problem = DownloadDirectory.check(target) {
            prompt = .problem(problem)      // §2.3：确认**之前**就中止
            return
        }
        prompt = .confirm(target)
    }

    /// 「选择…」→ `NSOpenPanel`（规格 §2.1：`canChooseDirectories` + `canCreateDirectories`）。
    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "选择"
        panel.message = "交付文件会下载到这个文件夹"
        // 从当前目录开始找（它不在时别把它设成起始目录：面板会开在一个奇怪的地方）。
        if !model.downloadDir.isEmpty,
           FileManager.default.fileExists(atPath: model.downloadDir) {
            panel.directoryURL = URL(fileURLWithPath: model.downloadDir)
        }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        ask(to: url.path)
    }

    /// 用户点了「更改并重启内核」之后才走到这里。
    ///
    /// ⚠️ **回执不在这里装**：`changeDownloadDir` 自己把它写进
    /// `AppModel.downloadDirChange`（上一条注释里那张回执**唯一**的落点）——
    /// 这一屏与 `RootView` 那行常驻提示读的是同一个值。
    private func apply(_ target: String) async {
        prompt = nil
        changing = true
        defer { changing = false }
        _ = await model.changeDownloadDir(to: target)
    }
}

// ---------------------------------------------------------------------------
// 编辑器（只在拿到内核那一份参数之后才存在）
// ---------------------------------------------------------------------------

/// 七项参数的**那一段**（一个 `Section` + 它的 footer 说明）。
///
/// 单独一个类型只为了一件事：`SettingsView` 里那份表单是 `SettingsForm?`
/// （内核还没给值时是 nil），而这里的 `@Binding var form: SettingsForm` 是**非可选**的
/// —— 七个控件都绑在它上面，Optional 版本的 Binding 会让每一处都要拆包一次。
///
/// ⚠️ **它不再自带 `Form`**（2026-09-20 现场修复）：原来它是 `SettingsEditor`、自己一个
///    `Form`，而 `SettingsView` 顶上还有一个 —— 两个平级 `Form` 在 `VStack` 里**对半分高度**，
///    只装「下载到」一行的那个于是占了半屏空白。现在表单由 `SettingsView` **统一持有**，
///    这里只产出**段**（形态与 `DownloadDirectorySection` 一致：两者都是"一个 Section"）。
///
/// ⚠️ 本类型**只需要 `form`**：七行控件读的全是它自己的值
///    （`minSplitSizeOptions` / `minSplitSizeNote` / `SettingsForm.limits`），
///    没有一处要问 `model` —— 别为了"看起来对称"把 `model` 传进来。
private struct SettingsParameterSections: View {
    @Binding var form: SettingsForm

    var body: some View {
        Section {
            int32Row("并行文件数（-j）", $form.parallel, in: SettingsForm.limits.parallel)
            int32Row("单文件连接数（-x）", $form.connections, in: SettingsForm.limits.connections)
            int32Row("分片数（-s）", $form.splits, in: SettingsForm.limits.splits)
            minSplitSizeRow
            limitMbpsRow
            int32Row("重试次数", $form.maxTries, in: SettingsForm.limits.maxTries)
            int32Row("重试间隔（秒）", $form.retryWait, in: SettingsForm.limits.retryWait)
        } footer: {
            Text(SettingsForm.applyNote)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: - 七行

    /// 一个整数项：值 + 上下调节。`in:` 是内核的区间（`SettingsForm.limits`），
    /// 让客户**输不出**越界值（契约 §2.1"越界必须在输入时拦下"）。
    ///
    /// ⚠️ 值要**画出来**：macOS 的 `Stepper` 只画上下箭头，不画当前值 ——
    ///    五个参数都看不见数字，客户只能靠数的下数猜现在是多少。
    private func int32Row(_ title: String,
                          _ value: Binding<Int32>,
                          in range: ClosedRange<Int32>) -> some View {
        LabeledContent(title) {
            HStack(spacing: 8) {
                Text(value.wrappedValue, format: .number)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                Stepper(value: value, in: range) { EmptyView() }
            }
        }
    }

    /// `-k`：**枚举控件**，取值集合来自内核的 `hello`（不是壳里的硬编码，契约 §2.3）。
    ///
    /// ⚠️ 那个 `if let note` 不是装饰：当前值不在内核给的集合里时（`settings.json`
    ///    被手改过），列表里会多出这一项、并附一句说明 —— 否则 Picker 会把一个
    ///    **不在 tag 集合里**的值画成空白或第一项，客户看到一个他从没设过的数。
    private var minSplitSizeRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            Picker("最小分片大小（-k）", selection: $form.minSplitSize) {
                // 顺序就是内核给的顺序（`minSplitSizeOptions` 不排序、不去重）。
                ForEach(form.minSplitSizeOptions, id: \.self) { Text($0).tag($0) }
            }
            if let note = form.minSplitSizeNote {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// `limit_mbps`：输入框 + 上下调节（简报指定），并写明 `0 = 不限速`。
    private var limitMbpsRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            LabeledContent("限速（MB/s）") {
                HStack(spacing: 8) {
                    TextField("限速", value: $form.limitMbps, format: .number)
                        .textFieldStyle(.roundedBorder)     // 规格 §7.2「圆角输入框」
                        .multilineTextAlignment(.trailing)
                        .labelsHidden()
                        .frame(width: 96)
                    Stepper(value: $form.limitMbps, in: SettingsForm.limits.limitMbps) { EmptyView() }
                }
            }
            Text(SettingsForm.limitMbpsNote)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

/// 保存条：未保存提示 + 「保存」+ **「开源许可…」入口**（GPLv2 全文，分发义务）。
///
/// ⚠️ 单独一个类型是因为它**有状态**（在飞的保存、内核原文的失败、许可那一页的 sheet），
///    而那些状态与"七行控件"无关：合成一个类型的话，`form` 的每一笔编辑都会重建它们。
///    （`form` 在这里是 `@Binding` 只读用 —— 这个类型不改它的任何一个字段。）
///
/// ⚠️ 它**只在拿到内核那份参数之后**才存在（`SettingsView.body` 里那个 `if let form`），
///    与改之前逐字相同：那时它是 `SettingsEditor` 的 footer，而 `SettingsEditor` 整个不渲染。
private struct SettingsFooter: View {
    @Binding var form: SettingsForm
    @ObservedObject var model: AppModel

    /// 一次保存在飞（禁用按钮，避免点两下排两条）。
    @State private var saving = false
    /// 保存失败的**内核原文**（约束 3：壳不加工、不重写）。
    @State private var failure: String?
    /// 「开源许可」那一页（sheet）。
    @State private var showingLicense = false

    var body: some View {
        footer
            // 许可那一页挂在**这里**（与改之前挂在 `SettingsEditor` 的 VStack 上等价：
            // 两者都在设置窗口里，`LicenseView` 的尺寸由它自己的 `frame` 定 —— 见那个文件）。
            .sheet(isPresented: $showingLicense) { LicenseView() }
    }

    // MARK: - 保存条

    /// 引擎现在能不能发请求（`EngineGate`，有单测）。保存与禁用同源。
    private var engineAllowsActions: Bool { EngineGate.allowsRequests(model.engine) }

    /// 有没有要保存的改动（判据在 `SettingsForm.matches`，有单测）。
    private var hasUnsavedChanges: Bool { !form.matches(model.settings) }

    private var canSave: Bool { engineAllowsActions && hasUnsavedChanges && !saving }

    private var saveHelp: String {
        if !engineAllowsActions { return EngineGate.unavailableHelp }
        return hasUnsavedChanges ? "把七项参数下发给内核（set_settings）"
                                 : "与内核当前参数一致：没有要保存的改动"
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let failure {
                // **内核原文逐字**（约束 3）、可选中复制、可收起（形态同
                // `VerifyView.failureBar` / `TransfersView.failureBar`）。
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                    Text(failure)
                        .font(.caption)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 8)
                    Button {
                        self.failure = nil
                    } label: {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.borderless)
                    .help("收起这条提示")
                }
            }

            HStack(spacing: 8) {
                Button {
                    showingLicense = true
                } label: {
                    Label("开源许可…", systemImage: "doc.text")
                }
                .buttonStyle(.borderless)
                // ⚠️ 与 `VerifyView` / `EngineStatusBadge` / 工具栏那颗下载按钮同款：
                //    `Label` 在某些容器里默认只画图标，把标题掉进 `.help` 浮层。
                //    这一条不是工具栏，但"GPL 全文的入口看得见"是**分发义务**的一部分，
                //    所以这里显式要求"标题与图标都要"（同款实测证据见
                //    `EngineStatusBadge.swift:32-36`）。
                .labelStyle(.titleAndIcon)
                .help("查看内嵌组件（aria2）的 GPLv2 全文")

                Spacer(minLength: 8)

                if saving {
                    ProgressView().controlSize(.small)
                }
                Text(hasUnsavedChanges ? "有未保存的改动" : "与内核当前参数一致")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                Button("保存") {
                    Task { await save() }
                }
                .buttonStyle(.borderedProminent)            // 规格 §7.2「主按钮」
                .keyboardShortcut(.defaultAction)           // …且回车触发
                .disabled(!canSave)
                .help(saveHelp)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: - 保存

    /// 保存：`set_settings`。**失败就把内核原文摆出来**（约束 3/4），不重写、不吞掉。
    ///
    /// ⚠️ 成功之后**不在这里重填表单**：内核回执会更新 `model.settings`，
    ///    而 `SettingsView` 的 `.onChange(of: model.settings)` 会用**回执那一份**
    ///    重填（`-k` 被内核归一成规范串时客户看得见）。两处都填等于有两个播种者，
    ///    "以哪一份为准"就会变成一个要现场推的问题。
    ///
    /// ⚠️ 引擎不可用时 `applySettings` 会**快速失败且不发请求**（第六条防线）：
    ///    那颗按钮同时是禁用的，这里是"渲染与点击之间引擎刚好翻成不可用"的那一下。
    private func save() async {
        saving = true
        defer { saving = false }
        do {
            try await model.applySettings(form.settings)
            failure = nil
        } catch {
            failure = SettingsSaveFailure.message(of: error)
        }
    }
}
