import Foundation

// ---------------------------------------------------------------------------
// 「把主窗口开回来」那道互锁的**状态机**
//
// ⚠️ 为什么它在这里，而不是留在 `Sources/BenagenDownloader/App.swift` 的 `MainWindow` 里
//    （全局约束 C-6）：这一段状态转移此前活在**可执行目标**里，而 `Package.swift` 只有
//    一个 test target、只依赖 `BenagenCoreKit` —— 于是它**一条判据都没有**。
//    而它恰恰是本阶段被反复修坏的那一段：针对它做过 3 轮修复，其中 **2 轮由复审推翻**
//    （账本 C14 / C15 / C17 / C18）。反复修坏又测不到的东西，必须先把状态机搬进
//    能被测的那一层，`AppDelegate` / `MainWindow` 退化成薄绑定。
//
// ⚠️ **抽取时不许改行为**：下面三个转移与 `App.swift` 里原有的实现**逐条等价**
//    （只把"谁来开窗""早退时记不记日志"这两件**副作用**留在了绑定那一层）。
//    可达序列见 `MainWindowReopenGateTests`。
// ---------------------------------------------------------------------------

/// 主窗口重开的互锁。
///
/// 它守着的是一个**时序**问题：应用不活跃时点 Dock，`applicationDidBecomeActive` 与
/// `applicationShouldHandleReopen` **两个回调都会来**，而 `openWindow(id:)`
/// **不是同步的** —— 第二个回调进来时新窗口往往还没 `isVisible`，于是它会**再请求一次**，
/// 一次点击开出两个窗口。少了这道互锁，"窗口回不来"就只是被换成了"一开开两个"。
///
/// 类型的**全部**状态就是这两个布尔（没有别的隐式状态），所以它是 `Equatable` 的值语义类型：
/// 测试可以直接断言"这一串转移之后状态是什么"。
public struct MainWindowReopenGate: Equatable, Sendable {

    /// 本进程里主窗口**真的出现过**吗。见 `MainWindowPolicy` 的注释：
    /// 少了它，启动那一瞬间的 `applicationDidBecomeActive` 会多开一个空窗口。
    ///
    /// ⚠️ 它的置位点**只有一个**：`noteMainWindowAppeared()`（窗口的内容 `onAppear` 了），
    ///    **不是**由 AppKit 回调"看到可见窗口"时置位。靠回调置位有一个致命缝隙：
    ///    启动那一刻 `applicationDidBecomeActive` 可能**早于窗口可见**，于是闩一直是
    ///    `false`，而此后应用一直活跃、**不会再有** `didBecomeActive` ——
    ///    用户"关掉窗口、再点 Dock"时两条通道同时哑火（`shouldOpenMain` 恒为 false）。
    ///    而"窗口的内容 `onAppear` 了"是**窗口真的出现过**的直接证据，与回调的先后无关。
    public private(set) var hasEverShownWindow: Bool

    /// 「自动重开已经请求过一次、而窗口还没回来」的互锁。
    ///
    /// **有两个解除点，缺一不可**：`noteMainWindowAppeared()`（"请求**已经落地**"，
    /// 主解除点）与 `noteWindowOnScreen()`（"窗口**在屏幕上**"，双保险）。
    /// 理由见各自的方法注释。
    public private(set) var automaticReopenRequested: Bool

    public init(hasEverShownWindow: Bool = false, automaticReopenRequested: Bool = false) {
        self.hasEverShownWindow = hasEverShownWindow
        self.automaticReopenRequested = automaticReopenRequested
    }

    /// 一次"没有可见窗口"的时段里最多请求一次（自动重开走这一条）的结果。
    ///
    /// ⚠️ 状态机**只回答"要不要请求"**，"请求"具体是什么（`openWindow(id:)`）
    /// 由绑定那一层做 —— 那样这个类型才能在没有 AppKit 的环境里被断言。
    public enum ReopenDecision: Equatable, Sendable {
        /// 去请求重开。
        case request
        /// 早退。⚠️ **调用方不许静默**（约束 4）：这一次早退有两个来源 ——
        /// 一个是**正常**的（同一次激活的第二个回调，互锁本来就为它而设），
        /// 另一个是**卡住**的（上一次请求没能让窗口出现，此后每次激活都在这里早退，
        /// 而界面上就是「点了 Dock 什么都没发生」—— 正是本任务要消灭的症状）。
        /// 后者必须留得下痕迹才知道发生过。
        case skip
    }

    /// **一次"没有可见窗口"的时段里最多请求一次**（自动重开走这一条）。
    ///
    /// 转移的**等价性依据**（与抽取前的实现逐字对应）：
    ///   ① 已请求过 ⇒ `.skip`（**不改任何状态** —— 早退必须是无副作用的，否则
    ///      第二次回调会把互锁"续期"，第一条请求落地时的解除就再也等不到）；
    ///   ② 没请求过 ⇒ 置位**并**返回 `.request`。
    ///      置位发生在**返回之前、且没有 await**，所以并发的第二条只会看到已置位的状态。
    public mutating func requestAutomaticReopen() -> ReopenDecision {
        guard !automaticReopenRequested else { return .skip }
        automaticReopenRequested = true
        return .request
    }

    /// 主窗口的**内容**上屏了（绑定那一层在 `MainWindowBridge.onAppear` 调）。
    ///
    /// **它置闩，也解除互锁** —— 两件事在这里各取所需：
    ///   - **闩**（`hasEverShownWindow`）要的事实是「窗口**被建出来过**」；
    ///     见 `MainWindowPolicy` 的注释（它挡的是"启动瞬间抢开窗口"）。
    ///   - **互锁**（`automaticReopenRequested`）要的事实是「**我们那次请求已经落地**」，
    ///     而"窗口的内容被建出来了"正是它。
    ///
    /// ⚠️ **为什么互锁必须在这里也解除**（第 3 轮复审重要 ①）：
    ///    `noteWindowOnScreen()` 只在**两个激活回调**里被轮询到，而**发起重开的那一次激活
    ///    按构造看不到可见窗口**（判据要求 `!anyVisible`）—— 窗口是**随后**才上屏的，
    ///    那一刻**没有任何回调会来**。于是"成功重开"之后互锁一直挂着，用户**下一次**
    ///    关窗再激活时会被吞掉。可达序列（**不需要任何异常**）：
    ///      ① 点 Dock 重开 → W2 上屏；
    ///      ② 用户**直接点红叉关掉 W2**（应用全程活跃，中间没有任何"有窗口在屏"的激活）；
    ///      ③ 切走 → 再点 Dock ⇒ **什么都不开**（只在控制台留一条 notice）。
    ///    那正是本任务要消灭的症状，README 第 18 条的验收流程会直接走到这条路上。
    ///    （`MainWindowReopenGateTests.reopeningStillWorksAfterTheNewWindowIsClosedImmediately`
    ///    就是这条序列。）
    ///
    /// ⚠️ **这条取舍是想过的，不是漏的**（复审者与控制者都过了一遍）：在这里解除
    ///    **早于**"窗口在屏幕上"这个事实。理论上留了一条**很窄**的缝 ——
    ///    两个激活回调跨了 runloop 轮次，而 SwiftUI 恰好在中间建窗、跑了 `onAppear`、
    ///    窗口尚未 `isVisible` —— 那时第二个回调会**再开一个窗口**。
    ///    控制者**接受**这条残余，理由是**错误方向的不对称**：最坏结果是"多一个窗口"
    ///    （看得见、关得掉、用户自己就能收拾），而换成"等窗口真的在屏才解除"换来的
    ///    是上面那条**静默不开** —— 后者正是本任务存在的理由。取轻的那个。
    public mutating func noteMainWindowAppeared() {
        hasEverShownWindow = true
        automaticReopenRequested = false
    }

    /// **有窗口在屏幕上了** —— 互锁的**另一个**解除点（绑定那一层观察到 `isVisible`
    /// 为真时调）。
    ///
    /// ⚠️ **它与 `noteMainWindowAppeared()` 两个都要留**，各自等的是**不同的事实**：
    ///   - 本方法等的是「窗口**在屏幕上**」—— 覆盖"窗口在屏、但没有走过 `onAppear` 那条观测"；
    ///   - `noteMainWindowAppeared()` 等的是「**那次请求已经落地**」—— 覆盖"窗口已经建出来，
    ///     而用户没再触发过任何一次能被回调看见的激活"这一支（**主要的那一条**）。
    ///    少了本方法只是少一层保险；少了另一个则是**静默吞掉下一次重开**。
    ///
    /// ⚠️ 它**不置闩**（`hasEverShownWindow` 不动）：这里要的事实是"窗口在屏"，
    ///    而不是"主窗口的内容被建出来过" —— 后者是 `noteMainWindowAppeared()` 的观测，
    ///    两者混起来会让"启动瞬间"这个判据的语义漂掉。
    ///
    /// ⚠️ 解除只发生在 `anyVisible == true` 的调用里，而那里 `MainWindowPolicy` 的判据
    ///    必为 `false`、随即返回 —— 解除与请求在同一次调用内互斥，所以本方法**不会**
    ///    造成"解除过早 ⇒ 开两个窗口"。
    ///
    /// ⚠️ **一条仍未解套的残留**（第 2 轮报告 §C 已上报，控制者尚未裁决；照实留在这里）：
    ///    万一 `openWindow(id:)` **静默失败**（窗口永远不出现 ⇒ 上面两条解除都不会被触发），
    ///    互锁会一直挂着，此后的自动重开全部早退（每次都有 `notice`，所以不静默）。
    ///    用户仍有路：⌘0 /「窗口」菜单那条**显式**通道不查互锁（见绑定那一层的 `openNow()`）。
    ///    彻底关掉它要把互锁做成有界的（例如记时间戳、超过若干秒即视为上次已失败）。
    public mutating func noteWindowOnScreen() {
        automaticReopenRequested = false
    }
}
