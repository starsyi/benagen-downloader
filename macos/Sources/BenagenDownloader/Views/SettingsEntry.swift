import AppKit
import SwiftUI

// ---------------------------------------------------------------------------
// 「设置」入口 —— **可用性分支的唯一落点**
// ---------------------------------------------------------------------------
// `SettingsLink` 是 macOS **14.0+** 才有的（它就是"打开本应用的 `Settings` 场景"）。
// 本代的部署目标是 **13.0**（客户那台 Intel 机封顶就是 13），所以它必须有一个 13 能走的
// 分支 —— 而 macOS 13 上打开设置场景的正统办法是 AppKit 那条 `showSettingsWindow:` action。
//
// 🔴 **为什么把它抽成一个类型，而不是在两个调用点各写一遍 `if #available`**
//    （两个调用点：`App.swift` 的「设置…」菜单项、`RootView` 工具栏那颗）
//    两处各写一遍的失效形态是**分叉**：哪天只改了一处，就会出现
//    "菜单能开、工具栏那颗点了没反应"（或者反过来）—— 而那正是这两个入口
//    当初被要求**共用一个实现**的理由（`RootView` 那段注释逐字写着这条）。
//    分支只许存在一处。
//
// ⚠️ **冒烟验证的边界**：13 那一支在**开发机（macOS 26）上跑不到** —— 那里 `#available`
//    永远走 14 那一支。所以"13 上点得开"这件事只有**真 13.x 机器**能验，
//    它进了真机验收清单（`macos/README.md` 的「只能真 Intel 验」）。
// ---------------------------------------------------------------------------

/// 「打开设置」那颗入口。用法的形状与 `SettingsLink` 一样：闭包给 `Label`。
///
/// ⚠️ `.labelStyle` / `.keyboardShortcut` / `.help` 这些**挂在外层**（调用点写），
///    两种分支都接受 —— 别把它们塞进这个类型里，那会让菜单与工具栏的写法被迫一致。
struct SettingsEntry<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        if #available(macOS 14.0, *) {
            SettingsLink(label: content)
        } else {
            Button(action: Self.openLegacy, label: content)
        }
    }

    /// macOS 13 那一支：走 AppKit 的 `showSettingsWindow:`。
    ///
    /// ⚠️ **`@MainActor` 不是装饰**：`NSApp` 与 `sendAction` 都是主 actor 隔离的，
    ///    少了它这里会有两条并发告警（而本仓库对告警敏感）。
    ///
    /// ⚠️ **必须消费返回值**：`sendAction` 在"没人接这条 action"时回 `false` 且
    ///    **什么都不发生** —— 也就是"点了没反应"，而界面上一句话都没有（约束 4 明禁的
    ///    静默失效）。selector 名字是**字符串**（`showSettingsWindow:`，macOS 13 的名字；
    ///    `showPreferencesWindow:` 是 12 及以下），拼错时同样静默 ⇒ 这里把 false 落到
    ///    控制台日志上（`App.swift` 的 `shellLog`，同 `MainWindow.openNow()` 那条先例）。
    ///
    /// ⚠️ 它是**唯一**一处 `Selector(...)`：全仓 grep 得到它 + 这条注释，
    ///    复审时按这条线检查（编译器看不见字符串 selector）。
    @MainActor
    private static func openLegacy() {
        let handled = NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        if !handled {
            shellLog.error("showSettingsWindow: 没人接 —— 设置窗口打不开（macOS 13 的兜底路径）")
        }
    }
}
