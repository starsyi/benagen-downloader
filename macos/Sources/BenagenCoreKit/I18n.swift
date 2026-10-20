import Foundation

// ---------------------------------------------------------------------------
// 壳自己写的那几句话的双语表（规格 I-2；样板是 `windows/shell-core/src/i18n.rs`）。
//
// ⚠️ **为什么壳里也要一张表**：内核给的文案已经按语言拼好（`core/src/i18n`），但壳还有
//    一批**它自己写的**句子 —— 引擎徽标、重启进度、以及八类错误卡的那三句
//    （`Presentation/ErrorCard.swift`）。它们没有"内核原文"可以照登（约束 3 只覆盖
//    内核说的那部分），所以必须自己双语。
//
// 两条纪律：
//
//   1. **判据在 Rust、文案在表里、前端只画**（I-1）：视图里不许出现新的界面文案，
//      只把这里给的字放进 `Text`。
//   2. **日志不翻译**（I-2）：这里只管客户看得见的字。
//
// 与内核那套的分工：内核的 `Lang` 由握手落定（`hello` 回执里的 `language`），
// 壳从回执里抄一份到 `AppModel.language`，此后所有壳侧构造点都读那一格 ——
// **壳不自己看系统 locale**，否则两端各算一次、语言就会在一句话里打架。
// ---------------------------------------------------------------------------

/// 界面语言。与内核 `core/src/i18n::Lang` 的**线上三个词**对齐（`system/zh/en`），
/// 但壳这边只保留**已落定**的两档：`system` 早在握手时就被内核解析掉了。
public enum Lang: String, CaseIterable, Equatable, Sendable {
    /// 中文（默认：没握手成功、没拿到语言时按中文画，与 0.2.x 的界面一致）。
    case zh
    /// 英文。
    case en

    /// 线上那个词（`zh` / `en`）——**永远不是** `system`（同内核的
    /// `resolved_is_never_system`）。
    public var wire: String { rawValue }

    /// 从握手回执里读。缺字段 / 不认识的一律当默认中文 ——
    /// **不许**因为一个语言字段把整个握手判失败（客户端启动不了比界面语言错更糟）。
    ///
    /// 与 `windows/shell-core/src/i18n.rs::Lang::from_hello` 逐条同款：
    /// `en` / `en-US` / `en_US` ⇒ 英文，其余（含 `nil`、`system`、`fr`）⇒ 中文。
    public static func fromHello(_ raw: String?) -> Lang {
        switch raw?.trimmingCharacters(in: .whitespacesAndNewlines) {
        case .some("en"), .some("en-US"), .some("en_US"):
            return .en
        default:
            return .zh
        }
    }

    /// 取一句壳侧文案。`system` 不会出现在这里（见 [`Lang`]）。
    ///
    /// 两张 `switch` 保证"漏写一条 = 编译错误"——与 Windows 的那两张 `match` 表同一条纪律。
    public func t(_ key: Key) -> String {
        switch self {
        case .zh: return I18n.zh(key)
        case .en: return I18n.en(key)
        }
    }

    /// 带占位符的版本：按名替换 `{name}`（与内核 `i18n::tf` 同一套写法）。
    ///
    /// 未知占位符**原样留着**（与 Windows 的 `tf` 同款）：漏填是可测的，
    /// 静默删掉则会让一句话缺一块而无迹可查。
    public func tf(_ key: Key, _ args: [String: String] = [:]) -> String {
        var out = t(key)
        for (name, value) in args {
            out = out.replacingOccurrences(of: "{\(name)}", with: value)
        }
        return out
    }
}

/// 壳侧文案的键（闭集：少写一条就是编译错误，见 [`Lang.t`]）。
///
/// ⚠️ 只收**本任务（T7）真的会画上屏**的那几句。其余既有视图的文案仍在原位
///    （中文逐字未动，英文留待后续任务按同一个模式迁进来）—— 不假称整壳已双语。
public enum Key: CaseIterable, Equatable, Sendable {
    // —— 引擎徽标（`Presentation/EngineStatusPresentation.swift`）——
    case engineConnecting
    case engineNotStarted
    case engineRunning
    case engineUnavailablePrefix
    case engineHandshakeTimeout
    case engineHandshakeTooltip
    case engineConnectingTooltip
    case engineNotStartedTooltip
    case engineRunningTooltip
    // —— 引擎横幅那颗唯一的出口按钮（`Views/RootView.swift`）——
    case retry
    // —— 自动重启的三句话（`Presentation/RestartNotice.swift`）——
    case restartWaiting
    case restartVerifying
    case restartExhausted
}

enum I18n {
    /// 中文一列。**逐字保留既有客户看到的句子**（一个字符都不许改）。
    static func zh(_ key: Key) -> String {
        switch key {
        case .engineConnecting: return "正在连接内核…"
        case .engineNotStarted: return "引擎未启动"
        case .engineRunning: return "运行中"
        case .engineUnavailablePrefix: return "引擎不可用：{reason}"
        case .engineHandshakeTimeout: return "内核无响应（等待超过 5 秒）"
        case .engineHandshakeTooltip:
            return "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。"
                + "请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
        case .engineConnectingTooltip: return "内核尚未握手（壳正在启动它）"
        case .engineNotStartedTooltip: return "下载引擎尚未启动 —— 添加下载任务后内核会启动它"
        case .engineRunningTooltip: return "下载引擎正在运行"
        case .retry: return "重试"
        case .restartWaiting: return "正在重启下载引擎（第 {attempt}/{budget} 次）…"
        case .restartVerifying: return "引擎已重启，正在确认它能正常工作…"
        case .restartExhausted: return "自动重启没能恢复下载引擎，请手动点「重试」"
        }
    }

    /// 英文一列（新增）。**不许出现汉字**（`I18nTests.theEnglishTableHasNoCJK` 钉着）。
    ///
    /// 句子逐字对齐 `windows/shell-core/src/i18n.rs` 与
    /// `windows/shell-core/src/presentation/restart_notice.rs` 的英文列。
    static func en(_ key: Key) -> String {
        switch key {
        case .engineConnecting: return "Connecting to the core…"
        case .engineNotStarted: return "Engine not started"
        case .engineRunning: return "Running"
        case .engineUnavailablePrefix: return "Engine unavailable: {reason}"
        case .engineHandshakeTimeout: return "The core is not responding (waited more than 5 seconds)"
        case .engineHandshakeTooltip:
            return "The core may be stuck on a system permission dialog (for example \"wants to access your Downloads folder\"). Check the screen for that dialog, click Allow, then click Retry."
        case .engineConnectingTooltip: return "The core has not completed the handshake yet (the shell is starting it)"
        case .engineNotStartedTooltip:
            return "The download engine is not started yet - the core starts it once you add downloads"
        case .engineRunningTooltip: return "The download engine is running"
        case .retry: return "Retry"
        case .restartWaiting: return "Restarting the download engine (attempt {attempt}/{budget})…"
        case .restartVerifying: return "The engine restarted - checking that it works…"
        case .restartExhausted:
            return "Automatic restarts could not recover the download engine - please click Retry"
        }
    }
}
