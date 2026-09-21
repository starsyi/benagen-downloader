import Foundation

/// 「要不要把主窗口开回来」的判据。
///
/// ⚠️ 为什么抽成纯函数（全局约束 C-6）：`AppDelegate` 活在可执行目标里、做不了单测，
///    而这条判据有**真实的出错形态** —— 多开一个窗口。`applicationDidBecomeActive`
///    在**启动那一刻**也会触发，而它与 SwiftUI 建第一个窗口**没有先后保证**：
///    少了 `hasEverShownWindow` 这一条，用户在启动瞬间就会被多开一个空窗口。
public enum MainWindowPolicy {

    /// 有过窗口、而现在一个可见的都没有 ⇒ 把主窗口开回来。
    ///
    /// - `anyVisibleWindow`：`NSApp.windows` 里**存在** `isVisible == true` 的窗口。
    ///   ⚠️ 它是**字面意义上的"在屏幕上"**：缩到 Dock 的窗口 `isVisible == false`，
    ///   所以**不算**（虽然 `applicationShouldHandleReopen` 的 `hasVisibleWindows` 参数
    ///   把最小化的窗口也算作可见）。这里是**有意**只认"在屏幕上"，理由写在
    ///   `AppDelegate.anyVisibleWindow` 的注释里（裁定 **C14**：错误方向的不对称）。
    /// - `hasEverShownWindow`：本进程里主窗口**真的出现过**。
    ///   置位点是 `MainWindowBridge.onAppear`（`App.swift`）—— **不是**"某个回调看到过可见窗口"。
    ///   后者留了一条致命的缝隙：回调可能**早于**窗口可见，于是闩一直是 `false`，
    ///   而此后应用一直活跃、不会再有 `didBecomeActive` —— 用户"关掉窗口、再点 Dock"时
    ///   两条通道会一起哑火（本函数恒为 `false`）。见 `MainWindow.hasEverShownWindow`。
    ///   它挡住的是"启动瞬间抢开窗口"，见类型注释。
    public static func shouldOpenMain(anyVisibleWindow: Bool, hasEverShownWindow: Bool) -> Bool {
        hasEverShownWindow && !anyVisibleWindow
    }
}
