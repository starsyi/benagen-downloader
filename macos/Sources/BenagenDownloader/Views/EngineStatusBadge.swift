import SwiftUI
import BenagenCoreKit

/// 引擎状态徽标：工具栏上的「正在连接内核 / 引擎未启动 / 运行中 / 引擎不可用（原因）」。
///
/// 这是全局约束 4「不得静默失效」在界面上的**第一个落点** —— 内核没起来时，
/// 原因必须看得见，而不是让客户对着一个点了没反应的窗口发呆。
///
/// ⚠️ 它读 **`model.engine`**，不读 `model.lastError`。两者是不同的东西：
///    - `engine` 是引擎/内核的**生命周期状态**，只有 `AppModel.absorb` 按结构化错误码
///      写它（外加握手与轮询成功时的观测）；
///    - `lastError` 是轮询/校验这类**不抛错路径**上「其余错误码 → 原文」的不粘滞出口，
///      任务 8/9 的横幅读它，每次成功请求都会被清掉。
///    把徽标接到 `lastError` 上，一次瞬时的 `engine_rpc_failed` 就会让徽标显示「不可用」，
///    而那正是 `AppModel` 特意不让那些错误写 `engine` 的原因（写了会永久关掉轮询）。
///
/// ⚠️ 这里**不重试、不重启**：用户动作由任务 8 的横幅「重试」按钮触发（`AppModel.retryEngine`）。
///    徽标只负责**说清楚现在是什么状态**。
///
/// ⚠️ 本文件**只有绑定**（全局约束 8）：四条映射（正文/图标/浮层/折行）都在
///    `BenagenCoreKit/Presentation/EngineStatusPresentation.swift` 里，且都有单测。
///    这里保留的 `tint` 是唯一的例外 —— 它返回 SwiftUI 的 `Color`，是纯渲染决定，
///    断言不出有意义的东西；`Presentation/` 不为了它引入 SwiftUI 依赖。
struct EngineStatusBadge: View {
    let engine: AppModel.EngineState

    var body: some View {
        Label(
            EngineStatusPresentation.text(for: engine),
            systemImage: EngineStatusPresentation.systemImage(for: engine)
        )
        // ⚠️ 这一行是**修 bug**，不是装饰：macOS 的统一工具栏默认把 `Label` 渲染成
        //    **只有图标**，于是 `text(for:)` 里那句「引擎不可用：<原因>」会掉进 `.help`
        //    浮层里 —— 那恰好就是约束 4 要防的"静默失效"，整个徽标就白做了。
        //    （已实测：不加这行时截图里工具栏只有一个圆形箭头图标、没有任何文字。）
        //    显式要求"标题与图标都要"，才让那句话真的出现在屏幕上。
        .labelStyle(.titleAndIcon)
        .font(.callout)
        .foregroundStyle(tint)
        .lineLimit(1)
        .truncationMode(.middle)
        .help(EngineStatusPresentation.tooltip(for: engine))
    }

    /// `.notStarted` 用 secondary 而**不是**红/黄：没 enqueue 过就没有引擎，
    /// 「引擎未启动」是**正常态**（见 `AppModel.EngineState.notStarted` 的注释），
    /// 把它染成警告色会让每个刚打开客户端的用户以为出了问题。
    private var tint: Color {
        switch engine {
        case .unknown, .notStarted: return .secondary
        case .running: return .green
        case .unavailable: return .red
        }
    }
}
