import Foundation

// ---------------------------------------------------------------------------
// 自动重启的进度提示（规格 W3；样板是
// `windows/shell-core/src/presentation/restart_notice.rs`）。
//
// 内核的 `restart_progress` 只给**结构化状态 + 两个数**（`attempt` / `budget`），
// 一句话也不给 —— 因为"重启到第几次了"在客户眼里是"一行进度"，而它属于壳的呈现层。
// 四档（含 `idle`）与两句中英文都照 Windows 那份逐字抄来：两个壳说的是同一件事。
//
// ⚠️ **未知状态 ⇒ `nil`（什么都不显示），不是"原样显示"**：
//    这条提示只在**重启进行中**才有意义；一个壳不认识的状态（协议演进 / 内核笔误）
//    如果说成"正在重启"是撒谎，如果说成"重启失败"更糟（`exhausted` 才是那一档）。
//    什么都不说至少不会误导；真正的内核故障另有引擎横幅。
// ---------------------------------------------------------------------------

/// 一条已经成句的重启进度（视图直接画 `text`，不再拼字符串）。
public struct RestartNotice: Equatable, Sendable {
    /// 客户可见的那句话（含次数）。
    public let text: String

    public init(text: String) {
        self.text = text
    }

    /// `state` 的线上词 → 一句话。`waiting` / `verifying` / `exhausted` 之外一律 `nil`
    /// （含 `idle`：没在重启就没有提示）。
    ///
    /// - Parameters:
    ///   - state: 内核 `restart_progress.state`，逐字（`idle`/`waiting`/`verifying`/`exhausted`）。
    ///   - attempt: 第几次（1 起）。
    ///   - budget: 预算上限（与 `attempt` 一起进 `{attempt}/{budget}`）。
    ///   - lang: 已落定的界面语言（来自握手回执）。
    public static func of(state: String,
                          attempt: UInt32,
                          budget: UInt32,
                          lang: Lang) -> RestartNotice? {
        switch state {
        case "waiting":
            return RestartNotice(text: lang.tf(.restartWaiting,
                                               ["attempt": "\(attempt)", "budget": "\(budget)"]))
        case "verifying":
            return RestartNotice(text: lang.t(.restartVerifying))
        case "exhausted":
            return RestartNotice(text: lang.t(.restartExhausted))
        default:
            return nil
        }
    }
}
