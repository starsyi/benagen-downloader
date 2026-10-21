import Foundation

// ---------------------------------------------------------------------------
// 手动检测更新（设置里那颗「立即检测」按钮）的**判决与那句话**（规格 §2）。
//
// ## 它为什么存在：规格 §5 留下的一个洞
//
// 内核的 `update_status` 在**「这次没查成」**与**「查成了、确实没有新版」**两种情形下
// 长得**一模一样**（`has_newer:false` + `last_seen` 不变）。对那条**后台**提示条来说
// 这是对的：客户没做任何动作，界面不该冒话（规格 §5「失败一律静默」）。
// 但设置里那颗按钮**是客户点出来的** —— 客户问的是"有没有新版本"，而在断网时只按
// `has_newer` 判就只剩一句**"已经是最新的"**：那是**界面在撒谎**（约束 4）。
//
// ⇒ 内核因此多了一格 `last_check_ok`（`core/src/kernel.rs` 的 `Kernel::update_last_check_ok`），
//   本类型把它 + `has_newer` + 版本号判成**五档**，每档一句人话。
//   **三档不许并成两档**：开关关着 / 没查成 / 内核没给那一格，都不配说"已经是最新的"。
//
// ## 🔴 两端对位（本文件与 Windows 那侧是**同一份判决的两份实现**）
//
// Windows 壳的同款判决在 `windows/shell-core/src/presentation/update_check.rs`
// （`UpdateCheck::of` + `headline()` + `is_failure()`），**那五句话逐字相同** ——
// 客户在两个平台上读到的是同一句话。改这里就要同时改那边：
// `windows/scripts/check_presentation_mirrors.sh` 的第 ③ 段就是盯着这五句的
// （两端逐字比对，红在 CI 里，而不是等客户发现两个客户端说得不一样）。
//
// ⚠️ **为什么不共用一份**（与 `UpdateNotice` 的 R21 那一段同一条理由）：壳是 Swift、
//    内核与另一个壳是 Rust，**跨语言调不到**。这是**有意的逻辑重复**，代价由那条比对判据控住。
//
// ## 与 Windows 那侧的两处**有意**差别（各自都记在这里，不是疏漏）
//
//  ① macOS 这一侧先把载荷解成 [`UpdateStatus`]（强类型）再看，而 Windows 那侧直接读
//     `serde_json::Value`。⇒ 一份**缺 `has_newer` / 缺 `current`** 的载荷在 macOS 上
//     **解不开**：那次命令以解码失败的原文收场（`AppModel.checkUpdateNow` 回原文，
//     视图摆出来），而 Windows 上会走到 `.noAnswer`。
//     两边的**结论一样**（都不许说"已经是最新的"），只是**说话的位置**不同。
//  ② 因此 macOS 上 `.noAnswer` 实际上只由**一种**情形触发：`last_check_ok` 缺席
//     （旧内核）。那一档照样留着 —— 它是"说不清就说不清"这条纪律的落点，而不是
//     某个载荷形状的补丁。
//
// ## 全局约束 8
//
// 「把内核数据变成界面值的纯计算」都放 `Presentation/`、每条都要有单测；
// 视图里只留绑定与渲染。本文件**不 `import SwiftUI`、也不 `import AppKit`**。
// ---------------------------------------------------------------------------

/// 一次「立即检测」的结局 → **要摆到屏幕上的那句话** + 它是成功档还是失败档。
public struct UpdateCheckReceipt: Equatable, Sendable {
    /// 判决本身（五档之一）。**话在取用时才成句** ⇒ 同一份判决可以按当前界面语言
    /// 重画（`text(lang:)`），切换语言后回执不会停在旧语言上。
    private enum Verdict: Equatable, Sendable {
        case disabled
        case upToDate(current: String)
        case unreachable
        case noAnswer
        case newer(latest: String, current: String)
    }

    private let verdict: Verdict

    /// 视图据此挑图标（`exclamationmark.triangle` / `checkmark.circle`）。
    ///
    /// ⚠️ **前两档也是"失败"**（开关关着：你这一下没查；内核没给那一格：说不清）——
    ///    按"是不是错误"来分会让那句"刚才没有去查"配一个绿勾，读起来像"已经是最新的"。
    public let isFailure: Bool

    private init(_ verdict: Verdict, isFailure: Bool) {
        self.verdict = verdict
        self.isFailure = isFailure
    }

    /// **这句话的中文版**（既有调用点与单测读它）。
    public var text: String { text(lang: .zh) }

    /// 同一份判决按**当前界面语言**成句。
    public func text(lang: Lang) -> String {
        switch verdict {
        case .disabled: return lang.t(.updateReceiptDisabled)
        case .upToDate(let current): return lang.tf(.updateReceiptUpToDate, ["current": current])
        case .unreachable: return lang.t(.updateReceiptUnreachable)
        case .noAnswer: return lang.t(.updateReceiptNoAnswer)
        case .newer(let latest, let current):
            return lang.tf(.updateReceiptNewer, ["latest": latest, "current": current])
        }
    }

    /// 判决：`update_check_now`（或 `update_status`）回的**那一份状态** → 五档之一。
    ///
    /// ⚠️ **判据顺序是承重的**（与 Windows 那侧逐条同源）：先看开关（关着 ⇒ **根本没查**，
    ///    说什么都错），再看"这次查成没有"，**最后**才轮到"有没有新版"。
    ///    把顺序换过来（先看 `hasNewer`）会得到这样一份载荷的错判决：
    ///    `{enabled:false, last_check_ok:true, has_newer:true, …}` —— 开关关着的时候
    ///    内核一次都没查，`hasNewer` 是**上一次**（可能几天前）的旧结论。
    ///
    /// ⚠️ `nil`（还没问过内核 / 内核没回）**也走"说不清"**：这一格不许当成"查过了"。
    public static func of(_ status: UpdateStatus?) -> UpdateCheckReceipt {
        // ① 开关关着 ⇒ 这一下没有查（内核的 `run_update_check` 第一道闸就返回了）。
        guard let st = status else { return noAnswer() }
        if st.enabled == false { return disabled() }

        // ② 这次查成没有 —— 这一格就是整个类型存在的理由。
        switch st.lastCheckOk {
        case .some(false):
            // ③ 没查成：网络/发布页不可达，或最新那条是预发布。
            return unreachable()
        case .none:
            // ④ 内核没给这一格：**不许**当成查成了（旧内核 / 协议缺格）。
            return noAnswer()
        case .some(true):
            // ⑤ 查成了，才轮到"有没有新版" —— **只有这一支**配说"已经是最新的"。
            guard st.hasNewer else { return upToDate(st.current) }
            guard let latest = st.latest, !latest.isEmpty else {
                // "有新版"却说不出是哪个版本 ⇒ 不猜（宁可不说话）。
                return noAnswer()
            }
            return UpdateCheckReceipt(.newer(latest: latest, current: st.current), isFailure: false)
        }
    }

    // -----------------------------------------------------------------------
    // 那五句话（与 Windows 那侧 `update_check.rs` 的 `headline()` **逐字相同**）
    //
    // ⚠️ 失败那三档都点名了**是哪一步** + **可执行的补救**（W-2 口径）：只报一句
    //    "没查成"而不说下一步，等于把客户扔在原地。
    // ⚠️ 改这里的字之前先跑 `bash windows/scripts/make_font_subset.sh --check`：
    //    新字必须在内嵌字体子集里（那一步会红）。
    // -----------------------------------------------------------------------

    private static func disabled() -> UpdateCheckReceipt {
        UpdateCheckReceipt(.disabled, isFailure: true)
    }

    private static func upToDate(_ current: String) -> UpdateCheckReceipt {
        UpdateCheckReceipt(.upToDate(current: current), isFailure: false)
    }

    private static func unreachable() -> UpdateCheckReceipt {
        UpdateCheckReceipt(.unreachable, isFailure: true)
    }

    private static func noAnswer() -> UpdateCheckReceipt {
        UpdateCheckReceipt(.noAnswer, isFailure: true)
    }
}
