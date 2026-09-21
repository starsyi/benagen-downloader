import SwiftUI
import os
import BenagenCoreKit

@main
struct BenagenApp: App {
    /// 进程收尾用（规格 §5.4）。`@NSApplicationDelegateAdaptor` 建的实例由 SwiftUI 持有，
    /// 这里只把它接进 `body` 才能拿到它。
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    // live() 不抛错：找不到内核时它把原因装进 model.engine == .unavailable(...)，
    // 由 EngineStatusBadge 呈现（约束 4 —— 不得静默失效）
    /// ⚠️ **必须是 `@StateObject`**：`ObservableObject` 拿 `@State` 持**不会订阅**
    ///    —— 编译通过、界面永不刷新（`@Observable` 时代 `@State` 是对的，那是这次
    ///    换回 `ObservableObject` 时最容易漏、且编译器不报的一处）。
    @StateObject private var model = AppModel.live()

    var body: some Scene {
        WindowGroup(id: mainWindowID) {
            RootView(model: model)
                // 「把主窗口开回来」这件事需要 `OpenWindowAction`，而它**只能从视图的
                // `@Environment` 取**（`AboutCommand` 就是为同一个理由单独建的一个 View）。
                // 这个空视图只做一件事：把它存进 `MainWindow`，给 AppDelegate 用。
                .background(MainWindowBridge())
                // ⚠️ 换成了 `bootstrap()`：它内部就是原来那两行（`start()` 紧跟
                //    `loadRememberedDelivery()`），但**只跑一次**——第二个窗口不得重做。
                //    原来那段"必须在 `start()` 之后、且在同一条 `.task` 里"的注释
                //    **照旧成立**，已经搬进 `bootstrap()`。
                .task { await model.bootstrap() }
                // 收尾闸门接线：把模型交给 delegate，`applicationWillTerminate` 才找得到它。
                // 见 AppDelegate 的注释（为什么光靠"管道断了内核自己退"不够）。
                .onAppear { AppDelegate.model = model }
        }
        .defaultSize(width: 1100, height: 720)
        // 窗口下界由内容自己说了算（RootView 的 .frame(minWidth: 1000, minHeight: 640)）。
        .windowResizability(.contentMinSize)
        // ⚠️ 这个块里**必须有真东西**：空的 `CommandsBuilder` 产出 `EmptyView`，而
        //    `EmptyView: Commands` 这个 conformance 要 macOS 27.0+，在
        //    `platforms: [.macOS(.v14)]` 下直接编译失败（已实测）：
        //      error: conformance of 'EmptyView' to 'Commands' is only available in macOS 27.0 or newer
        .commands {
            // 「显示主窗口」。它是**显式的兜底**：自动重开万一在某种情形下没触发
            // （那条路依赖 AppKit 回调真的送到），用户至少有一条点得着的路。
            // ⚠️ **不要 `replacing: .newItem`**：`File > New Window`（⌘N）得留着，
            //    而 `replacing:` 是把**整条**换掉。
            // ⚠️ **有意不经 `MainWindowPolicy`**：显式入口不该被"曾经有过窗口吗"挡住
            //    —— 用户点得到它，就说明应用在跑，那一刻他就是要窗口。
            // ⚠️ **归属「窗口」菜单是裁定 C9 的结果**：规格 §2.1 那一行写的是「File 菜单」，
            //    而 §2.3 的做法表给的是 `CommandGroup(before: .windowList)` —— 两处冲突。
            //    **取 §2.3**：窗口类命令归「窗口」菜单是 macOS 的平台惯例。
            // ⚠️ 文案是**壳自己写的**（全局约束 C-7）：内核一个字都没提到过菜单，
            //    没有原文可登 —— 这是菜单项的标题，只能由壳起名。
            CommandGroup(before: .windowList) {
                Button("显示主窗口") { MainWindow.openNow() }
                    .keyboardShortcut("0", modifiers: .command)
            }

            // 「关于 …」那一项。同样用 `replacing:` 顶掉系统那条**默认项**：
            // 默认那条弹的是系统的关于面板（一个通用图标 + 版本号），
            // 而这一版要的是**自己的品牌资产**（全称 logo）与自己的窗口。
            CommandGroup(replacing: .appInfo) {
                AboutCommand()
            }

            // 「设置…」那一项。用 `replacing:` 顶掉系统那条**默认项**：默认项打开的是
            // 一个空白的设置面板（没有 `Settings` 场景时），而这一屏是客户**唯一**
            // 能改内核参数的地方（规格 §7.1）。
            CommandGroup(replacing: .appSettings) {
                // `SettingsEntry` = "打开本应用的 `Settings` 场景"，也就是下面那个
                // `Settings { SettingsView(model:) }`。它里面那一个 `#available` 分支是
                // **全仓唯一**的一处（macOS 14+ 用 `SettingsLink`、13 走 AppKit 的
                // `showSettingsWindow:`）—— 菜单项与工具栏那颗（`RootView`）共用它，
                // 所以不存在"菜单能开、按钮没反应"的分叉。
                SettingsEntry {
                    Label("设置…", systemImage: "gearshape")
                }
                // ⚠️ **⌘, 要自己写**：那个快捷键原本挂在系统那条默认的「设置…」上，
                //    而 `replacing:` 是把**整条**换掉 —— 不写这一行，⌘, 就没反应了
                //    （而它是 macOS 上打开设置的习惯键，也是本任务标题里的那个键）。
                .keyboardShortcut(",", modifiers: .command)
            }
        }

        // 设置场景（⌘, 落到这里）。`SettingsView` 自己从 `model` 取内核手里那一份参数，
        // 场景本身不持有任何状态。
        Settings {
            SettingsView(model: model)
        }

        // 关于场景（⌘ 菜单 →「关于 Benagen 数据下载工具」落到这里）。
        // 用 `Window` 而不是 `WindowGroup`：它是**单例**窗口 —— 没有"关于"开第二个的道理，
        // 而且 `openWindow(id:)` 对已存在的窗口是**置前**而不是再开一个。
        // 内容不持有任何状态（`AboutView` 只读包里的资源与 Info.plist）。
        Window(aboutWindowTitle, id: aboutWindowID) {
            AboutView()
        }
    }
}

// ---------------------------------------------------------------------------
// 窗口的 id
// ---------------------------------------------------------------------------

/// 主窗口的场景 id。`WindowGroup(id:)` 与 `openWindow(id:)` **必须**用同一个串 ——
/// 对不上的表现是"点了没反应"（不报错、不弹窗），与 `aboutWindowID` 是同一类坑。
private let mainWindowID = "main"

/// `Window(_:id:)` 与 `openWindow(id:)` **必须**用同一个串 —— 对不上的表现是
/// "菜单项点了没反应"（不报错、不弹窗）。这里只定义一次，两处引用同一个常量。
private let aboutWindowID = "about"

/// 窗口标题，同时也是菜单项的文字。
private let aboutWindowTitle = "关于 Benagen 数据下载工具"

/// ⌘ 菜单里那条「关于 …」。
///
/// ⚠️ `openWindow` 是 `@Environment` 的值，**只能从视图里取** —— 所以这里必须是一个
///    独立的 `View`：把它直接写在 `BenagenApp.body` 的 `CommandGroup` 里取不到
///    （`CommandGroup` 的内容虽然是个 `ViewBuilder`，但那是 `App` 的上下文）。
private struct AboutCommand: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button {
            openWindow(id: aboutWindowID)
        } label: {
            // ⚠️ `.labelStyle(.titleAndIcon)` **不能省**（本项目栽过三次，见 README 第 11 条）：
            //    macOS 的菜单/工具栏默认可能只画图标，文字掉进**悬停才可见**的浮层里 ——
            //    那样这一条就只剩一个圈里一个 "i" 的图标，用户根本不知道它是"关于"。
            Label(aboutWindowTitle, systemImage: "info.circle")
                .labelStyle(.titleAndIcon)
        }
    }
}

// ---------------------------------------------------------------------------
// 把主窗口开回来
// ---------------------------------------------------------------------------

/// 壳自己的诊断日志（只落「控制台.app」，不打扰用户）。
///
/// ⚠️ 这个应用此前**一条日志都没有**，所以这里说明一下它为什么出现：全局约束 4 对
///    "静默失效"一贯敏感，而下面 `MainWindow.openNow()` 在 `reopen` 还是 `nil` 时
///    **只会什么都不做** —— 界面上没有任何落点，至少要在控制台留得下痕迹。
///    subsystem 用打包脚本里的 `CFBundleIdentifier`（`scripts/build_app_macos.sh:51`），
///    在控制台里按它过滤就能看到。
/// ⚠️ 不是 `private`：`SettingsEntry`（macOS 13 的兜底路径）也要用它 —— Swift 的
///    `private` 在文件作用域上是**文件私有**，跨文件就用不到了。
let shellLog = Logger(subsystem: "com.benagen.downloader", category: "shell")

/// 「把主窗口开回来」的唯一持有者。
///
/// ⚠️ **为什么是一个可空的静态闭包**：`OpenWindowAction` 只能从视图的 environment 取，
///    而需要它的 `AppDelegate` 不是视图。窗口**被销毁之后**这份闭包仍然有效
///    —— 这是整条修法成立的前提（`OpenWindowAction` 背后是应用的场景系统，不是那个窗口）。
///
/// ⚠️ `@MainActor`：`App` / `View` / `AppDelegate` 都在主 actor 上，写它和读它的只有这三者。
///    标注的另一个作用是满足 Swift 6 的严格并发（可变全局状态不得无隔离）。
///
/// ⚠️ **两个事实都放在这里、而不是放在 `AppDelegate` 上**：写它们的一个是 `AppDelegate`
///    （回调），另一个是 `MainWindowBridge`（视图）。视图拿不到 delegate 的**实例**
///    （`@NSApplicationDelegateAdaptor` 建的那个实例在 `BenagenApp` 手里），
///    而这两件事都只关于主窗口，收在这里最省事也最少分叉。
@MainActor
enum MainWindow {
    /// 「开一个主窗口」这个动作本身。
    static var reopen: (() -> Void)?

    /// **互锁的状态机**（`MainWindowReopenGate`，`Presentation/`，**有单测**）。
    ///
    /// ⚠️ 这一段状态转移原本就写在这个枚举里 —— 而它活在**可执行目标**里，全仓唯一被断言
    ///    的是 `MainWindowPolicy.shouldOpenMain`（一个 `a && !b`）；三个转移本身
    ///    **一条判据都没有**。而它恰恰是本阶段被反复修坏的那一段（3 轮修复里有 2 轮由复审
    ///    推翻，账本 C14 / C15 / C17 / C18）。所以状态机搬进 `BenagenCoreKit`，
    ///    这里退化成**薄绑定**：只做两件状态机做不了的事 —— 调 `openWindow`、记日志。
    ///    两个布尔的**全部**语义（谁什么时候置位、两个解除点各自等什么事实、
    ///    以及"取消掉一次成功重开之后必须还能再开"那条可达序列）都在那个类型的注释里。
    private static var gate = MainWindowReopenGate()

    /// 本进程里主窗口**真的出现过**吗（判据 `MainWindowPolicy.shouldOpenMain` 读它）。
    static var hasEverShownWindow: Bool { gate.hasEverShownWindow }

    /// **一次"没有可见窗口"的时段里最多请求一次**（自动重开走这一条）。
    static func requestAutomaticReopen() {
        switch gate.requestAutomaticReopen() {
        case .request:
            openNow()
        case .skip:
            // ⚠️ **早退不许静默**（约束 4 / 复审裁定 ②）：这一次早退有两个来源 ——
            //    一个是**正常**的（同一次激活的第二个回调，互锁本来就为它而设），
            //    另一个是**卡住**的（上一次请求没能让窗口出现，此后每次激活都在这里早退，
            //    而界面上就是「点了 Dock 什么都没发生」—— 正是本任务要消灭的症状）。
            //    后者必须留得下痕迹才知道发生过。等级用 `notice` 而不是 `error`：
            //    正常那一支在每次"点 Dock 重开"时都会走一遍，`error` 会天天喊狼来了。
            shellLog.notice("自动重开早退：上一次请求还没有落地（主窗口仍未回到屏幕上）")
        }
    }

    /// 显式重开（⌘0 /「窗口」菜单 →「显示主窗口」）。
    ///
    /// ⚠️ **它不查 `MainWindowPolicy`，也不受上面那道互锁限制**（裁定 ②）：
    ///    用户**点得到它，就说明应用在跑、而他此刻就是要窗口** ——
    ///    被"曾经有过窗口吗"或者"上一次请求还没落地"挡住，菜单项就会变成
    ///    「点了没反应」，而那是比多开一个窗口更糟的失效（约束 4）。
    static func openNow() {
        guard let reopen else {
            // 🔴 全局约束 4（不许静默失效）：`reopen` 是 `MainWindowBridge.onAppear` 装进来的，
            //    为 nil ⇔ 主窗口的内容**从来没上屏过**（例如启动瞬间就按了 ⌘0）。
            //    这一次点击不会有任何效果，界面也不会说任何话 —— 所以至少留下日志。
            shellLog.error("「显示主窗口」被触发，但 MainWindow.reopen 还是 nil（主窗口的内容还没出现过）；这次请求没有效果")
            return
        }
        reopen()
    }

    /// 主窗口的**内容**上屏了（`MainWindowBridge.onAppear` 调）：置闩 + 解除互锁。
    /// 两个事实各自的理由见 `MainWindowReopenGate.noteMainWindowAppeared()`。
    static func noteMainWindowAppeared() {
        gate.noteMainWindowAppeared()
    }

    /// **有窗口在屏幕上了** —— 互锁的**另一个**解除点（`AppDelegate` 观察到 `isVisible`
    /// 为真时调）。理由见 `MainWindowReopenGate.noteWindowOnScreen()`。
    static func noteWindowOnScreen() {
        gate.noteWindowOnScreen()
    }
}

private struct MainWindowBridge: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Color.clear.onAppear {
            MainWindow.reopen = { openWindow(id: mainWindowID) }
            // 一次两件事：**闩**的唯一置位点（窗口被建出来过），以及**互锁**的解除点之一
            // （我们那次重开请求已经落地）。两者为什么都要、以及各自在等什么事实，
            // 见 `MainWindow.noteMainWindowAppeared()` 的注释 —— 别把互锁那半句删掉：
            // 删了之后，"成功重开 → 直接关掉 → 再激活"那一路会被静默吞掉。
            MainWindow.noteMainWindowAppeared()
        }
    }
}

// ---------------------------------------------------------------------------
// 收尾
// ---------------------------------------------------------------------------

/// 应用退出时的收尾（规格 §5.4 的明文要求）。
///
/// 三层进程是 **壳 → 内核 → aria2c**，退出时任何一层留下孤儿都是故障：
/// 「壳退出 → 管道断开 → 内核自己退」这条路**不保证 aria2c 被关掉** ——
/// 内核要跑完 `Daemon::close()` 才退，而上界是 5 s，来不及就是三个进程里的
/// 第二个先死、第三个没人管。所以这里显式发 `shutdown` 并**等内核回收完**。
///
/// `AppModel.shutdown()` 是唯一**同步**的方法（全局约束 17）：`CoreClient.shutdown()`
/// 自带超时收尾（最坏约 10 秒）且幂等，而 `applicationWillTerminate` 里没有可以
/// `await` 的余地 —— 这正是那条例外存在的理由。
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// 壳的状态中枢。`weak`：它由 `BenagenApp` 的 `@State` 持有，这里只借来看一眼。
    static weak var model: AppModel?

    func applicationWillTerminate(_ notification: Notification) {
        Self.model?.shutdown()
    }

    // MARK: - 把主窗口开回来

    /// ⚠️ **主通道是它，不是 `applicationShouldHandleReopen`。**
    ///    在纯 SwiftUI 应用里，那个回调**经常根本不被调用**（多份报告一致；
    ///    同一批报告里 `applicationDidFinishLaunching` 之类都正常触发，所以不是链接问题）。
    ///    把"窗口回得来"这件事押在它身上，等于赌一个已知会输的赌局。
    func applicationDidBecomeActive(_ notification: Notification) {
        openMainWindowIfNothingIsVisible()
    }

    /// 副通道：保留它（AppKit 的文档承诺它会在 Dock 重新激活应用时送到）。
    /// 它**可能**不来，所以主通道不能是它。
    ///
    /// ⚠️ 这里**故意不用** `flag` 参数，而是与 `applicationDidBecomeActive` 走**同一个**
    ///    `anyVisibleWindow`：两个通道用不同的判据会互相打架
    ///    （一个认为"最小化的窗口算有窗口"、另一个不算，于是同一次点击里一个开、一个不开）。
    ///    ⚠️ 这个口径与 `flag` **故意不一致**：`flag`（`hasVisibleWindows`）把最小化的窗口
    ///    也算作可见，而 `anyVisibleWindow` 只认"在屏幕上" —— 取哪一边是控制者的裁定
    ///    （**C14**，理由写在 `anyVisibleWindow` 的注释里）。
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        openMainWindowIfNothingIsVisible()
        return true
    }

    /// 判据在纯函数里（`MainWindowPolicy`，有单测），这里只负责把 AppKit 的事实变成布尔量。
    ///
    /// ⚠️ **它不在这里写 `hasEverShownWindow`**（任务 1 复审重要 ①）：那个闩的置位点是
    ///    `MainWindowBridge.onAppear`（窗口的内容真的上屏了）。见 `MainWindow.hasEverShownWindow`。
    ///    "一次激活最多开一个窗口"那道互锁在 `MainWindow.requestAutomaticReopen()` 里 ——
    ///    它必须**包住两个回调**（本节的两个方法都走它），放在任何一个回调里都挡不住另一个。
    private func openMainWindowIfNothingIsVisible() {
        let anyVisible = anyVisibleWindow
        // 互锁的解除点**之一**：`anyVisible` 就是"窗口**真的在屏幕上**"这个事实，
        // 而下面的判据本来就要算它 —— 顺手解除，不必再求第二遍。
        // ⚠️ 它是**双保险**，不是唯一的一处：主解除点在 `MainWindowBridge.onAppear`
        //    （那次重开请求"落地"了）。为什么两处都要（只留本处 ⇒ 下一次重开被静默吞掉），
        //    见 `MainWindow.noteMainWindowAppeared()` 的注释。
        if anyVisible { MainWindow.noteWindowOnScreen() }
        guard MainWindowPolicy.shouldOpenMain(anyVisibleWindow: anyVisible,
                                              hasEverShownWindow: MainWindow.hasEverShownWindow) else { return }
        MainWindow.requestAutomaticReopen()
    }

    /// 「现在屏幕上还有窗口吗」。
    ///
    /// ⚠️ **只看 `isVisible`，有意不含 `isMiniaturized`**（裁定 **C14**：回退了本任务第二轮
    ///    一度加上的 `|| $0.isMiniaturized`）。最小化的窗口 `isVisible == false`
    ///    但**仍在 `NSApp.windows` 里**，所以加上那一项能挡住"最小化之后点 Dock 又开一个窗口"。
    ///    回退它的理由是**错误方向的不对称**：
    ///      - 留着它 ⇒ 最小化后点 Dock **什么都不开**，把恢复**完全押在 AppKit 的默认重开行为上**
    ///        —— 而这正是整套设计从一开始就要避开的赌局（见上面副通道那段注释）；
    ///        赌输的症状是「点了 Dock 什么都没发生」，正是本任务存在的理由，而且是一条新开出来的路。
    ///      - 删掉它 ⇒ 最坏**多出一个窗口**：看得见、用户自己关得掉，并且明确告诉他"有东西发生了"。
    ///
    ///    **想再把它加回来的人先读上面两行** —— 那是控制者的裁定，不是遗漏。
    private var anyVisibleWindow: Bool {
        NSApp.windows.contains { $0.isVisible }
    }
}
