import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 主窗口重开的互锁（任务 1 复审重要 ① / 次要 ③）
//
// ⚠️ 这一节补的是**本阶段最贵的一个洞**：这段状态转移此前活在可执行目标
//    （`Sources/BenagenDownloader/App.swift`）里，而 `Package.swift` 只有一个 test target、
//    只依赖 `BenagenCoreKit` —— 于是"互锁什么时候解除"这件事**一条判据都没有**，
//    而它被反复修坏：针对它做过 3 轮修复，其中 2 轮由复审推翻（账本 C14 / C15 / C17 / C18）。
//    抽取后（`MainWindowReopenGate`）三个转移第一次可断言。
//
// ⚠️ 文件名与 `MainWindowPolicyTests` 分开：那个是**判据**（要不要开），这个是**状态机**
//    （一次没有可见窗口的时段里能请求几次、什么时候解锁）。两者互补、各自有测试。
// ---------------------------------------------------------------------------

@Suite("主窗口重开的互锁")
struct MainWindowReopenGateTests {

    // -----------------------------------------------------------------------
    // 一次激活最多请求一个窗口
    // -----------------------------------------------------------------------

    @Test("没请求过 ⇒ 放行；同一段里第二次请求 ⇒ 早退（否则一次点击开出两个窗口）")
    func aSecondRequestInTheSameStretchIsRefused() {
        var gate = MainWindowReopenGate(hasEverShownWindow: true)

        #expect(gate.requestAutomaticReopen() == .request, "第一次请求要落到 openWindow 上")
        #expect(gate.requestAutomaticReopen() == .skip,
                "同一次激活的第二个回调（didBecomeActive + shouldHandleReopen）不得再开一个窗口")
        #expect(gate.automaticReopenRequested, "早退不等于解锁：互锁必须还是挂着的")
    }

    @Test("早退是**无副作用**的：它不会把互锁续期，所以第一次请求落地时照样能解除")
    func aSkippedRequestDoesNotDisturbTheState() {
        // ⚠️ 这一条盯的是一个真实的错法：把早退写成"顺便再置一次位"的变异体在
        //    `aSecondRequestInTheSameStretchIsRefused` 下**照样是绿的**（第二次仍返回 skip），
        //    但它会让"窗口出现 ⇒ 解锁"这条链在第二次回调之后依然有效——看起来没事，
        //    直到有人把它和"按时间戳边界"之类的方案混在一起。
        //    判据取最直接的那一个：早退**前后状态完全相同**。
        var gate = MainWindowReopenGate(hasEverShownWindow: true)
        _ = gate.requestAutomaticReopen()
        let after = gate

        #expect(gate.requestAutomaticReopen() == .skip)
        #expect(gate == after, "早退不得改动任何状态")
    }

    // -----------------------------------------------------------------------
    // 账本 C17 的那条可达序列（第 2 轮修复栽掉的那条路径）
    // -----------------------------------------------------------------------

    @Test("请求重开 → 窗口出现 → 立刻被关掉 → 再次激活且无可见窗口 ⇒ **必须再次请求**")
    func reopeningStillWorksAfterTheNewWindowIsClosedImmediately() {
        // ⚠️ 这是**第 2 轮修复栽掉的那条路径**，也是本次抽取要守住的第一条：
        //      ① 点 Dock 重开 → W2 上屏；
        //      ② 用户**直接点红叉关掉 W2**（应用全程活跃，中间没有任何"有窗口在屏"的激活
        //         —— 所以 `noteWindowOnScreen()` 那条解除点**永远不会被调到**）；
        //      ③ 切走 → 再点 Dock ⇒ 必须**再次**请求。
        //    少了 `noteMainWindowAppeared()` 里的解除（只留 `noteWindowOnScreen()`），
        //    第 ③ 步会在这里被吞掉：界面上就是「点了 Dock 什么都没发生」。
        var gate = MainWindowReopenGate(hasEverShownWindow: true)

        #expect(gate.requestAutomaticReopen() == .request, "① 点 Dock 重开")
        gate.noteMainWindowAppeared()                    // ② W2 上屏（请求落地）
        #expect(gate.automaticReopenRequested == false, "请求落地 ⇒ 互锁解除")
        // ② 的后半：用户直接关掉 W2 —— **这里没有任何转移可调**（没有回调会来），
        //    所以下面那一行 `#expect` 才是"关掉之后还能不能再开"的判据本身。

        // ③ 再点 Dock，此时没有任何可见窗口：
        //    判据（`MainWindowPolicy`）先放行，状态机再放行。
        let anyVisibleWindow = false
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: anyVisibleWindow,
                                                hasEverShownWindow: gate.hasEverShownWindow),
                "关掉窗口之后判据必须放行（闩在窗口出现过时就该置上）")
        #expect(gate.requestAutomaticReopen() == .request,
                "③ **必须再次请求** —— 这正是第 2 轮修复栽掉、第 3 轮修回来的那条路")
    }

    @Test("同一次激活里第二个回调不得重复请求（反向的一半）")
    func theSecondCallbackInTheSameActivationDoesNotAskTwice() {
        // 与上一条配对的**反向**断言：把互锁删掉（每次都返回 .request）的实现能让上面那条
        // 通过，却会在"一次点击开出两个窗口"上翻车 —— 两条合起来才钉得住"一次激活一个窗口"。
        var gate = MainWindowReopenGate(hasEverShownWindow: true)

        #expect(gate.requestAutomaticReopen() == .request)
        #expect(gate.requestAutomaticReopen() == .skip)
        #expect(gate.requestAutomaticReopen() == .skip, "第三个回调（同一次激活）同样不得再开")
    }

    // -----------------------------------------------------------------------
    // 两个解除点各自等的事实
    // -----------------------------------------------------------------------

    @Test("窗口在屏幕上（另一个解除点）也能解锁：不是只有 onAppear 那条路")
    func noteWindowOnScreenAlsoUnlocks() {
        var gate = MainWindowReopenGate(hasEverShownWindow: true)
        _ = gate.requestAutomaticReopen()

        gate.noteWindowOnScreen()

        #expect(gate.automaticReopenRequested == false, "窗口真的在屏幕上 ⇒ 互锁解除")
        #expect(gate.requestAutomaticReopen() == .request, "解除之后可以再请求")
    }

    @Test("「窗口在屏幕上」不置闩：闩只由 onAppear 那条观测置位")
    func noteWindowOnScreenDoesNotSetTheLatch() {
        // ⚠️ 两个事实必须分开（`MainWindowReopenGate.noteWindowOnScreen` 的注释）：
        //    闩要的是「主窗口的内容**被建出来过**」，而那只有 `MainWindowBridge.onAppear`
        //    知道。把它在这里也置上，会让"启动瞬间"这条判据的语义漂掉 ——
        //    而那正是 `hasEverShownWindow` 存在的全部理由（防启动瞬间多开一个空窗口）。
        var gate = MainWindowReopenGate()
        gate.noteWindowOnScreen()

        #expect(gate.hasEverShownWindow == false,
                "「有窗口可见」不等于「主窗口的内容被建出来过」—— 闩不许在这里置位")
    }

    @Test("请求本身不置闩：闩只由窗口上屏置位（否则启动瞬间会多开一个空窗口）")
    func requestingDoesNotSetTheLatch() {
        var gate = MainWindowReopenGate(hasEverShownWindow: false)
        _ = gate.requestAutomaticReopen()

        #expect(gate.hasEverShownWindow == false, "请求 ≠ 窗口出现过")
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: false,
                                                hasEverShownWindow: gate.hasEverShownWindow) == false,
                "启动那一刻（闩还是 false）不许抢开窗口")
    }

    @Test("窗口上屏既置闩、又解除互锁（`MainWindowBridge.onAppear` 那一次调用的两件事）")
    func appearingSetsTheLatchAndUnlocks() {
        var gate = MainWindowReopenGate()
        _ = gate.requestAutomaticReopen()

        gate.noteMainWindowAppeared()

        #expect(gate.hasEverShownWindow, "窗口的内容上屏了 ⇒ 闩置位")
        #expect(gate.automaticReopenRequested == false, "同一次调用顺手解除互锁")
    }
}
