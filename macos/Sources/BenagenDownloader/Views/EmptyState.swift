import SwiftUI
import AppKit                  // NSImage：品牌标那一张（Data → 图）
import BenagenCoreKit

/// 空态页：交付码入口（`.idle` / `.loading` / `.failed` 三种 loadState 都落在这里）。
///
/// 规格 §14.2 的三段式：标题 + 输入行 + 主按钮，失败时**原地**显示内核原文与「重试」。
/// 加载中也留在这里（`ProgressView` + `busyReason`），因为这一刻还没有清单可看。
///
/// ⚠️ 本文件**只有绑定与分派**（全局约束 8）：
///   - 「把内核数据变成界面值」的纯计算在图里没有 —— 交付码的回落规则在
///     `DeliveryCodeEntry.resolve`（有单测），失败文案是 `Text(why)` 直通（约束 3：
///     内核原文一个字不改，没有拼接、没有兜底文案、没有默认值）；
///   - 回落规则的**输入**（"上次用的码是哪个"、"上次问的是哪台交付服务器"）
///     来自**壳的历史**（E-4 / E2①，见 `lastUsedCode` 与 `baseURLToLoad`）
///     —— 那一个来源与自动加载用的是同一个，两处不会打架；
///     ⚠️ **两个回落值必须成对**（码回落到哪条记录，地址就回落到**同一条**），
///        否则会发出"B 的码 + A 的服务器"，见 `baseURLToLoad`。
///   - 下面那个 `switch` 是**渲染分派**（这一段显示表单、那一段显示错误），不是值计算 ——
///     与 R19 对 `SidebarSection.title` 的判断同类：没有可断言的值。
///
/// ⚠️ 视图不单测（规格 §10.4）：这个文件的行为靠任务 11 的手工清单验收。
struct EmptyState: View {
    @ObservedObject var model: AppModel

    @State private var code: String = ""
    @State private var baseURL: String = ""
    /// 「高级：自定义下载地址」**默认收起**（简报明文）。
    @State private var showAdvanced = false

    var body: some View {
        VStack(spacing: 16) {
            header
            entryCard
            status
        }
        .frame(maxWidth: 460)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // 规格 §7.2：内容区 20–24px 内边距。
        .padding(20)
        // 规格 §7.2：状态切换用弹簧动画（加载中 → 失败 → 表单）。
        .animation(.spring(response: 0.3, dampingFraction: 0.7), value: model.loadState)
    }

    // MARK: - 顶部说明

    private var header: some View {
        VStack(spacing: 6) {
            mark
            Text("输入交付码开始下载")
                .font(.title2.weight(.semibold))
            Text("交付码在交付邮件或交付页链接里（整条链接也可以直接粘进来）")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
    }

    /// 品牌图形标（**换掉了原来那个 SF Symbol `shippingbox`**，不是又加一个）。
    ///
    /// ⚠️ 白底**由 SwiftUI 画**（`RoundedRectangle`），不烘进图里：入库的是透明底原件，
    ///    烘进去的话深色模式下就是一块脏白方块。观感与 `AppIcon.icns` 一致
    ///    （图标那边必须自己烘白底 —— 系统不会替第三方应用切这个壳，见生成器脚本）。
    ///
    /// ⚠️ 读不到时**整块不渲染**（不占位、不弹错）：logo 是装饰性资产，
    ///    判据在打包时（`build_app_macos.sh` 的存在性自查），
    ///    完整理由见 `BrandAssets.swift` 顶部那段与全局约束 11。
    @ViewBuilder private var mark: some View {
        if let image = Self.markImage {
            Image(nsImage: image)
                .resizable()
                .scaledToFit()
                // 图形标占白底的 0.60（与生成器里的 MARK_FRACTION 一致）。
                .frame(width: 58, height: 58)
                .padding(19)
                .background(RoundedRectangle(cornerRadius: 10).fill(.white))
        }
    }

    /// 进程内只读一次（形状同 `LicenseView.loaded`）：`body` 会因为动画与窗口尺寸反复
    /// 求值，每求值一次读一趟盘是白费的 I/O。
    ///
    /// ⚠️ `try?` 在这里**是刻意的**：装饰性资产读不到就不渲染它 ——
    ///    与 `LicenseText` 那套"读不到必须报错"是**有意为之的不同**（约束 11），
    ///    完整理由见 `BrandAssets.swift` 顶部。
    private static let markImage: NSImage? = {
        guard let data = try? BrandAssets.load(named: BrandAssets.markName) else { return nil }
        return NSImage(data: data)
    }()

    // MARK: - 输入卡片

    private var entryCard: some View {
        VStack(spacing: 12) {
            TextField("交付码", text: $code)
                .textFieldStyle(.roundedBorder)
                .font(.body)
                .disabled(isBusy)
                .onSubmit(load)

            // 过长时**立刻**说清楚为什么（不是只把按钮变灰）：约束 4 要求用户看得见
            // "为什么按不动"，而文案是 `DeliveryCodeEntry.tooLongHint`（壳写的，理由在那边）。
            if codeTooLong {
                Label(DeliveryCodeEntry.tooLongHint, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            Button("加载", action: load)
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                // `codeTooLong` / `baseURLTooLong`：超过 2 KiB 的串**不许发**（约束 C-3 ——
                // 超限的请求内核不报错、只把客户端静默堵死）。判据在 `DeliveryCodeEntry`，
                // 与换码面板同一个（那条路没有 `base_url` 字段，所以只有码那一半）。
                .disabled(isBusy || codeToLoad.isEmpty || codeTooLong || baseURLTooLong)

            // 对应 `load_delivery` 的 `base_url`；默认收起，普通客户永远用不到。
            DisclosureGroup("高级：自定义下载地址", isExpanded: $showAdvanced) {
                VStack(alignment: .leading, spacing: 4) {
                    TextField("http://download.benagen.com", text: $baseURL)
                        .textFieldStyle(.roundedBorder)
                        .font(.body)
                        .disabled(isBusy)
                    // 过长时的说明**不在这里**：它会禁用「加载」那颗按钮，所以必须摆在
                    // 收起这个分组也看得见的地方（见 `entryCard` 里那一句与它的注释）。
                    Text("留空即用默认交付服务器")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
                .padding(.top, 4)
            }
            .font(.callout)

            // ⚠️ 这一句**在 `DisclosureGroup` 之外**（收起分组也看得见）：它会禁用上面那颗
            //    「加载」，而"按钮灰着、界面上什么都不说"正是约束 4 要防的形态 ——
            //    用户完全可能填完地址之后把「高级」收起来再点加载。
            if baseURLTooLong {
                Label(DeliveryCodeEntry.baseURLTooLongHint,
                      systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(16)
        // 规格 §7.2：圆角 8–12px。
        .background(RoundedRectangle(cornerRadius: 10).fill(.quaternary.opacity(0.5)))
    }

    // MARK: - 进行中 / 失败

    @ViewBuilder private var status: some View {
        switch model.loadState {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                // 进行中要说清楚**在做什么**：`busyReason` 那句就是（约束 4）。
                if let busy = model.busyReason {
                    Text(busy)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        case .failed(let why):
            VStack(spacing: 8) {
                // ⚠️ 内核原文**逐字**显示（约束 3），可选中复制。
                Label {
                    Text(why)
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                }
                .font(.callout)
                .foregroundStyle(.red)
                .multilineTextAlignment(.leading)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)

                Button("重试", action: load)
                    .buttonStyle(.borderless)
                    // 同「加载」：过长的码/地址不许发（「重试」要重试的那两个值同样可能过长
                    // —— 它们是从输入框或**壳的历史**里来的，见 `codeToLoad` 与 `baseURLToLoad`）。
                    // ⚠️ 判据必须与那颗「加载」**逐字相同**：少一项就是"按钮点得动、
                    //    按下去什么都没发生"（`load()` 里那道 guard 会直接 return）。
                    .disabled(codeToLoad.isEmpty || codeTooLong || baseURLTooLong)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        default:
            EmptyView()
        }
    }

    // MARK: - 动作

    /// 加载中时输入框与按钮都禁用 —— 一次只发一条（约束 15 的单飞语义在 `AppModel` 那侧，
    /// 这里只是别让用户排队点上十次）。
    private var isBusy: Bool { model.loadState == .loading }

    /// 这次要加载的码：用户敲的优先，输入框为空时回落到**壳的历史里最近使用的那条**
    /// （规则与理由见 `DeliveryCodeEntry.resolve`，那里有单测）。
    private var codeToLoad: String {
        DeliveryCodeEntry.resolve(typed: code, remembered: lastUsedCode)
    }

    /// 「上次用的码」——它的权威**只有一个**：**壳的历史**（阶段 E 规格 §1.3 / E-4）。
    ///
    /// ⚠️ **这一处曾是 E-4 的漏网之鱼**（任务 1 复审重要 ①）：上面那颗「重试」发的也是
    ///    `codeToLoad`，而失败分支**不会**执行 `performLoadDelivery` 里的 `lastCode = info.code`
    ///    （它只在成功路径上）。于是"自动加载失败 → 点重试"这条真实路径上，
    ///    回落到内核的 `last_code` 会发出**另一个码**（内核文件里那个旧的），
    ///    而屏幕上、以及壳刚刚试图加载的，都是历史里那条 —— 正是 E-4 要防的
    ///    "**壳说 A、实际加载 B**"，只是发生在重试而不是自动加载上。
    ///    实测现场：壳历史最近是 `C24-8`、内核 `last_code` 是 `DDDD-9`
    ///    （`AppModelHistoryTests.afterTheSeedTheKernelsLastCodeIsNeverConsultedAgain` 那一族）。
    ///
    /// ⚠️ `model.lastCode` **只作为"历史意外为空"的兜底**保留：种子之后它几乎不会出现
    ///    （自动加载会先把内核那条迁进历史），留它是为了不把这条回落**变成空串** ——
    ///    那会让「重试」变成一颗按下去什么都不发生的按钮（约束 4 的静默失效）。
    ///    判据与「自动加载用哪个码」（`AppModel.rememberedCode()`）**同一个来源**，
    ///    两处不会分叉。
    private var lastUsedCode: String {
        model.history.mostRecent?.code ?? model.lastCode
    }

    /// 这次要发的码**太长**吗（判据与理由见 `DeliveryCodeEntry.tooLong`）。
    /// 它同时是那颗按钮的禁用条件与上面那句提示的显示条件 —— **同一个判据**，
    /// 不会出现"按钮灰着、界面不说什么"（约束 4）。
    private var codeTooLong: Bool {
        DeliveryCodeEntry.tooLong(codeToLoad)
    }

    /// 这次要发的自定义下载地址（约束：空串 = 不带 `base_url`，由内核用它自己的默认值）。
    ///
    /// ⚠️ **判据与"码"那一路是同一套语义**（Ruling E2①，最终审查顺手 1）：
    ///    用户敲的优先，否则回落到**记住的那一条**的 `base_url`。
    ///    改之前这一格直接发输入框里的值（启动时是空串）—— 于是"自动加载用自定义服务器
    ///    失败 → 点「重试」"会打到**默认**服务器：发出去的是**同一个码、另一个服务器**，
    ///    用户看到的是一句与刚才不同的失败（E2 原文：**码同源了，请求仍不是刚才失败的那一个**）。
    ///
    /// ⚠️ **回落必须与码一起回落**（`code.isEmpty` 也在判据里）：
    ///    用户敲了 B 码、却带着 A 条目的 `base_url` 去问 A 服务器的话，得到的是一句
    ///    "码不存在"—— 而他明明就站在 B 的清单前。所以只有"码也是回落来的"时才回落地址。
    /// ⚠️ 历史为空（`mostRecent == nil`）时回落成空串 = 内核默认服务器 —— 与
    ///    `AppModel.rememberedCode()` 在那一支上返回 `(seed, nil)` 同一条口径。
    private var baseURLToLoad: String {
        guard baseURL.isEmpty, code.isEmpty else { return baseURL }
        return model.history.mostRecent?.baseURL ?? ""
    }

    /// 「高级」里那个自定义下载地址太长了（判据同上；空串不算 —— 它是"用默认"的合法表示）。
    ///
    /// ⚠️ 判据用的是**这次真正要发的那个值**（`baseURLToLoad`），不是输入框原文 ——
    ///    与 `codeTooLong` 用的是 `codeToLoad` 逐字同款（"按钮的禁用条件与那句提示的
    ///    显示条件是**同一个判据**"）。回落来的地址同样可能过长（`history.json` 是
    ///    用户改得动的文件），而超限的请求内核**不报错**、只把客户端静默堵死（约束 C-3）。
    private var baseURLTooLong: Bool {
        !baseURLToLoad.isEmpty && DeliveryCodeEntry.tooLong(baseURLToLoad)
    }

    /// 发请求（约束 17：`AppModel` 的方法都是 `async`，视图用 `Task { await … }` 调）。
    private func load() {
        let target = codeToLoad
        guard !target.isEmpty else { return }
        // 回车/键盘快捷键不经过按钮的禁用状态，所以这里要再挡一次（与换码面板同款）。
        // **不是静默**：两个 `…TooLong` 为真时，上面那句提示已经摆在屏幕上了（约束 4）。
        guard DeliveryCodeEntry.isSendable(target), !baseURLTooLong else { return }
        // ⚠️ 地址与码**一起**算（`baseURLToLoad` 的理由见它的注释）：两处如果各回落各的，
        //    就会出现"码是 B、服务器是 A"的错配。
        Task { await model.loadDelivery(code: target, baseURL: baseURLToLoad) }
    }
}
