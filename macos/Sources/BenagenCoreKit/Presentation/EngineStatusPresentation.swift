import Foundation

// ---------------------------------------------------------------------------
// 引擎状态的**呈现模型**：全部是纯函数，无状态、无 SwiftUI 依赖（只 `import Foundation`）。
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件" —— 下面这四条映射
//   每一条都能写出断言（而且 `singleLine` 还背着约束 3 的一条有语义的边界），
//   所以它们属于这里。视图侧只剩绑定。
//
// ⚠️ 本文件**不要** `import SwiftUI`。唯一需要 SwiftUI 的是 `tint`（返回 `Color`），
//    它是纯渲染决定、断言不出有意义的东西，留在 `EngineStatusBadge` 里。
// ---------------------------------------------------------------------------

/// `AppModel.EngineState` → 工具栏徽标上的字、图标、与浮层提示（规格 §7、约束 4）。
///
/// 约束 4「不得静默失效」在界面上的**第一个落点**：内核没起来时，`text(for:)` 里那句
/// 原因就是客户唯一的线索 —— 所以它必须**看得见**，不能只塞进 `.help` 浮层。
/// 视图侧用 `.labelStyle(.titleAndIcon)` 保证标题真的渲染出来（见 `EngineStatusBadge`）。
public enum EngineStatusPresentation {

    /// 握手超时的浮层提示（**悬停才可见**，所以它只补充、不承担"失败出现在界面上"）。
    ///
    /// ⚠️ 这一处的文案是**壳自己写的**（不是"唯一一处"——`AppModel.absorb` 的
    ///    `.protocolMismatch` 分支、`onKernelDeath`、`restartKernel` 里也有壳写的话；
    ///    其余一律"内核原文照登"，约束 3）。
    ///    这一处的**理由**：超时是**壳探测到的**状况，内核没给任何 message，没有"原文"可登。
    ///    详细论证在 `AppModel.start()` 的 `catch is HandshakeTimeout` 分支（约束 11 要求注明）。
    ///
    /// 内容分两层：**是什么**（可能卡在系统授权对话框上）+ **怎么办**（看一眼屏幕、点「允许」、
    /// 回来点「重试」）。没有第二层的那句话，用户能做的就只剩"再等等"。
    public static var handshakeTimeoutTooltip: String { handshakeTimeoutTooltip(lang: .zh) }

    /// 同上，按当前语言取句（视图用这一支）。
    public static func handshakeTimeoutTooltip(lang: Lang) -> String {
        lang.t(.engineHandshakeTooltip)
    }

    /// 这一条 `reason` 是不是"握手超时"（而不是内核给的失败原因）？
    ///
    /// 判别式是**逐字相等**，不是包含 —— 内核原文（约束 3 照登）里万一出现同样的字样，
    /// 也不该被壳抢过来改写成自己的文案。钉住这条的是
    /// `engineStatusTimeoutDiscriminatorIsExactNotFuzzy`。
    ///
    /// 为什么需要它：`EngineState` 只有 `.unavailable(String)` 这一个"不可用"分支
    /// （任务 3 定下的形状），而超时与"内核说它自己不可用"在**界面上**要给出完全不同的两句话 ——
    /// 前者是壳写的、可操作的；后者是内核原文、一个字不改。判别式就是这两者的分水岭。
    public static func isHandshakeTimeout(_ reason: String) -> Bool {
        reason == AppModel.handshakeTimeoutMessage
    }

    /// 徽标正文。
    ///
    /// `.unavailable` 的括号里是**内核原文照登**（约束 3：壳不加工），只做折行处理 ——
    /// 见 `singleLine(_:)`。
    ///
    /// ⚠️ 唯一的例外是握手超时：那时**徽标上必须有那句话本身**（`handshakeTimeoutMessage`），
    ///    不再套「引擎不可用：」的前缀 —— 它本身就是一句完整的话，而工具栏只有一行、
    ///    还要留给「内核无响应（等待超过 5 秒）」这个长度（约束 4：失败要**出现在界面上**，
    ///    不能只活在悬停浮层里；任务 4 正是栽在徽标文字整个掉进 tooltip 上）。
    ///
    /// ⚠️ `lang` **默认中文**：既有调用点与既有测试（`EngineStatusPresentationTests`）
    ///    读的都是中文原句；新代码显式传 `model.language`。
    public static func text(for engine: AppModel.EngineState, lang: Lang = .zh) -> String {
        switch engine {
        case .unknown:
            return lang.t(.engineConnecting)
        case .notStarted:
            return lang.t(.engineNotStarted)
        case .running:
            return lang.t(.engineRunning)
        case .unavailable(let reason):
            if isHandshakeTimeout(reason) { return lang.t(.engineHandshakeTimeout) }
            return lang.tf(.engineUnavailablePrefix, ["reason": singleLine(reason)])
        }
    }

    /// 徽标图标（SF Symbol 名）。
    public static func systemImage(for engine: AppModel.EngineState) -> String {
        switch engine {
        case .unknown: return "questionmark.circle"
        case .notStarted: return "circle.dashed"
        case .running: return "bolt.fill"
        case .unavailable: return "exclamationmark.triangle.fill"
        }
    }

    /// 鼠标悬停浮层。
    ///
    /// ⚠️ `.unavailable` 这里给的是**完整原文**（含换行），而 `text(for:)` 给的是折行版：
    ///    工具栏只有一行，折行只为排版；浮层放得下换行，所以一个字符都不动。
    ///
    /// ⚠️ 唯一的例外仍是握手超时：内核一个字都没回，那句"原文"不存在 ——
    ///    浮层给的是壳写的[`handshakeTimeoutTooltip`]（"可能卡在授权框上，去点「允许」再回来重试"）。
    public static func tooltip(for engine: AppModel.EngineState, lang: Lang = .zh) -> String {
        switch engine {
        case .unknown:
            return lang.t(.engineConnectingTooltip)
        case .notStarted:
            return lang.t(.engineNotStartedTooltip)
        case .running:
            return lang.t(.engineRunningTooltip)
        case .unavailable(let reason):
            if isHandshakeTimeout(reason) { return lang.t(.engineHandshakeTooltip) }
            return reason
        }
    }

    /// 把多行原文折成**一行**，供工具栏使用。
    ///
    /// 内核对 `coreNotFoundError` 的原话就带换行（「找不到内核可执行文件 benagen-core（找过：\n<三条路径>\n）」）。
    ///
    /// 契约（约束 3「壳不加工」在排版上的唯一让步）：
    ///   - 换行符折成**一个空格**；
    ///   - **除换行以外，一个字符都不增删改** —— 非换行的字符原样保留、顺序不变。
    ///   - 注意 `split` 默认 `omittingEmptySubsequences: true`，所以**连续/首尾的换行会被合并**：
    ///     `"\n\na"` → `"a"`（不是 `"  a"`）。这是有意的：折行的目的是排版，
    ///     留下前导空格会让工具栏文字看着莫名缩进一格。
    public static func singleLine(_ text: String) -> String {
        text.split(whereSeparator: \.isNewline).joined(separator: " ")
    }
}
