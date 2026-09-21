import Foundation

// ---------------------------------------------------------------------------
// 传输列表的**呈现模型**：全部是纯函数，无状态、无 SwiftUI 依赖（只 `import Foundation`）。
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件" —— 下面每一条都能写出断言，
//   而且有几条背着硬约束：五档状态两两不同（约束 4）、动作准入（点一颗注定报错的按钮比
//   没有按钮更糟）、`raw_status` 才是「已暂停」的来源（裁决 #88）、引擎原文照登（约束 3）、
//   以及准入闸门（约束 4/15：内核卡死时静默挂死是本项目明禁的形态）。
//
// ⚠️ 本文件**不要** `import SwiftUI`：四条颜色决定用 `RowColor` 枚举表达
//    （同 `BrowserRow` / `RowStateStyle`），枚举 → `Color` 的那一步留在视图里。
// ---------------------------------------------------------------------------

/// 传输列表里的一行：**已经算好的界面值**，视图侧只剩绑定。
///
/// 「文件名/相对路径 · 进度 · 速度 · 状态」四列的值都在这里成形，视图里不拼字符串、
/// 不判断状态、不做格式化。
public struct TransferRow: Equatable, Sendable, Identifiable {
    /// aria2 的 GID —— 这一行的身份，也是 `task_action` 要的那个 gid。
    public let gid: String
    /// 「文件名/相对路径」那一列。**清单原文**（约束 3），`nil` 映射时是占位符。
    public let title: String
    /// 内核给的清单路径**原文**；`nil` = 内核说这个 GID 没有路径映射
    /// （空串也算没有：见 [`TransferReveal.manifestPath`]）。
    public let manifestPath: String?
    /// 「进度」那一列：`已完成 / 总量`。
    public let progressText: String
    /// 「进度」那一列的百分比。总量为 0 时是占位符 `—`（口径同 [`PercentFormat`]）。
    public let percentText: String
    /// `ProgressView(value:)` 要的那个数，**恒在 `0...1`**（含总量为 0 的 0，绝不 NaN）。
    public let progressFraction: Double
    /// 「速度」那一列。`0` 显示 `—`（口径同 [`SpeedFormat`]）。
    public let speedText: String
    /// 「状态」那一列的文字。五档**两两不同**（约束 4）。
    public let stateLabel: String
    /// 「状态」那一列的颜色（枚举，不是 SwiftUI 的 `Color`）。
    public let stateColor: RowColor
    /// 行首那颗状态图标（SF Symbol 名，同 `BrowserRow.iconName` 的形态）。
    ///
    /// 名字放 `Presentation/` 而不是视图里，与 `BrowserRow.iconName` 同一条理由：
    /// 它是"内核状态 → 界面值"的映射，且**五档两两不同**这件事能被断言。
    public let stateIconName: String
    /// 行首那颗「已暂停」角标。
    ///
    /// ⚠️ **只能**来自 `raw_status`（裁决 #88 / 设计规格 §8.4）：领域状态把 aria2 的
    ///    `paused` 与 `waiting` 合成同一个 `.waiting`，由 `state` 反推出来的「已暂停」
    ///    永远是假的。`raw_status` 取不到时内核传空串（"不知道"），那就**不显示**角标
    ///    —— 不要替内核猜。
    public let showsPausedBadge: Bool
    /// 行下方那条错误原文；`nil` = 没有错误消息（内核的 `error_message` 是空串）。
    ///
    /// ⚠️ **不改写、不截断、不折叠**（约束 3）：aria2 的原话就是客户唯一能拿去搜索的东西。
    public let errorText: String?
    /// 这一行允许的动作。**空集 = 一个都不给**（比给一颗注定报错的按钮好）。
    public let availableActions: Set<TaskAction>

    /// GID 就是这一行的身份（内核在 `Daemon::snapshot` 里已经折过重复 GID）。
    public var id: String { gid }

    /// 这一行能不能「在访达中显示」。
    ///
    /// ⚠️ 这就是简报里那句「`path == nil` 时**该菜单项禁用**，不要静默无效」的落点：
    ///    没有路径就没有文件可定位，菜单项**变灰**而不是点下去什么都不发生。
    ///    判据与 [`TransferReveal.localPath`] 同源（都走 `manifestPath` 那一份归一化），
    ///    所以"能显示"与"算得出路径"不会分叉。
    public var canRevealInFinder: Bool { manifestPath != nil }

    /// `transfer_list` 的一条 → 一行。
    public init(_ item: TransferItem) {
        let path = TransferReveal.manifestPath(item.path)
        self.gid = item.gid
        self.title = path ?? Self.unknownPath
        self.manifestPath = path
        self.progressText = ByteFormat.text(item.completed) + " / " + ByteFormat.text(item.total)
        self.percentText = PercentFormat.text(done: item.completed, total: item.total)
        self.progressFraction = Self.fraction(completed: item.completed, total: item.total)
        self.speedText = SpeedFormat.text(item.speed)
        self.stateLabel = Self.label(item.state)
        self.stateColor = Self.color(item.state)
        self.stateIconName = Self.iconName(item.state)
        self.showsPausedBadge = item.rawStatus == Self.pausedRawStatus
        // 空串 = 内核说"没有错误消息"（`error_message` 的约定）。两者在界面上是同一件事：
        // 都没有那一行可显示。
        self.errorText = item.errorMessage.isEmpty ? nil : item.errorMessage
        self.availableActions = Self.actions(state: item.state,
                                             isPaused: item.rawStatus == Self.pausedRawStatus,
                                             gid: item.gid)
    }

    /// 内核没有给路径映射时的行标题（`path: null`）。
    ///
    /// 这一句是**壳自己写的**（同 `AppModel.busyReason` 的性质）：内核发的是一个 `null`，
    /// 没有可登的原文，而把 `null` 渲染成空白或字面量 "nil" 都是约束 4 说的静默失效。
    static let unknownPath = "（未知路径）"

    /// aria2 里"已暂停"的原始状态串（`core/src/engine/status.rs` 的透传值）。
    static let pausedRawStatus = "paused"

    /// 完成量 / 总量，**夹到 `0...1` 且绝不为 NaN**。
    ///
    /// 口径与 [`PercentFormat`] 逐条对齐：总量 <= 0 → 0（清单还没加载时总量就是 0），
    /// 超报（`completed > total`）夹到 1（任务超报不得画出超过满格的进度条）。
    static func fraction(completed: Int64, total: Int64) -> Double {
        guard total > 0 else { return 0 }
        return Double(min(max(0, completed), total)) / Double(total)
    }

    /// `TaskState` 五档 → 界面标签。**任何一档都不许是空白**（约束 4）。
    static func label(_ state: TaskState) -> String {
        switch state {
        case .waiting: return "等待中"
        case .active: return "下载中"
        case .complete: return "已完成"
        case .error: return "失败"
        case .removed: return "已移除"
        }
    }

    /// 语义色（规格 §7.2）。**不含 SwiftUI 的 `Color`**，由视图映射。
    ///
    /// ⚠️ 五档**两两不同**（与 `label` / `iconName` 同等要求：两个状态画成一个样子，
    ///    等于其中一个永远不会出现在界面上）——所以 `.waiting` 的中性灰**不能**也发给
    ///    `.removed`。`.removed` 拿的是规格里那个**警告色**：它是五档里唯一
    ///    **一个动作都不给**的状态（`remove` 已经把它自己的 GID→路径映射 forget 掉了），
    ///    出路在列表级的「清空已完成」——用警告色把它标出来，正好提示"这一行要你在别处处理"。
    ///    钉住它的是 `everyTaskStateMapsToItsOwnColor`。
    static func color(_ state: TaskState) -> RowColor {
        switch state {
        case .waiting: return .secondary
        case .active: return .blue
        case .complete: return .green
        case .error: return .red
        case .removed: return .orange
        }
    }

    /// 状态 → 行首那颗 SF Symbol 名。五档**两两不同**（同 `label`：两个状态画成一样，
    /// 等于其中一个永远不会出现在界面上）。
    static func iconName(_ state: TaskState) -> String {
        switch state {
        case .waiting: return "clock"
        case .active: return "arrow.down.circle"
        case .complete: return "checkmark.circle"
        case .error: return "exclamationmark.triangle"
        case .removed: return "trash"
        }
    }

    /// 这一行允许哪些动作。
    ///
    /// ⚠️ 判据是**领域状态 + 是否暂停 + 有没有 GID**，不是"这个动作在 aria2 里存不存在"：
    ///    菜单上出现一颗按下去只会回错的按钮，与约束 4 的"失败必须出现在界面上"是两回事
    ///    —— 前者是壳明知故犯地把错误送到用户面前。
    ///
    /// ⚠️ **`clear_finished` 不在这里**：它没有 gid、语义是"清空全部已结束的"，
    ///    混进行级菜单的后果是用户以为只清这一行、实际清掉一片。它是列表级的动作
    ///    （见 [`TransferGlobalSummary.canClearFinished`]）。
    static func actions(state: TaskState, isPaused: Bool, gid: String) -> Set<TaskAction> {
        // 没有 GID 就没有动作可发（内核会回 `invalid_params("动作 … 需要 gid")`）。
        guard !gid.isEmpty else { return [] }
        switch state {
        case .active:
            // 正在下载：能暂停、能移除；**不能重试**（重试是"停下来之后再来一次"）。
            return [.pause, .remove]
        case .waiting:
            // 排队等着的：能暂停、能移除。已暂停的那一条相反 —— 给它「继续」。
            return isPaused ? [.unpause, .remove] : [.pause, .remove]
        case .complete, .error:
            // 已停止的两档：重试（重新入队）与移除。
            return [.retry, .remove]
        case .removed:
            // ⚠️ 一个都不给。`remove` 会把 GID 的路径映射 `forget` 掉，所以 `removed`
            //    这一档既没有路径可重试（内核回 `invalid_params("GID … 没有路径映射")`）、
            //    也没有东西可再移除。要清掉它们用列表级的「清空已完成」。
            return []
        }
    }
}

// ---------------------------------------------------------------------------
// 列表顶部：全局速度与活动数（`transfer_list` 的 `global`）
// ---------------------------------------------------------------------------

/// `global` → 列表顶部那一行。
public struct TransferGlobalSummary: Equatable, Sendable {
    /// 「1.2 MB/s」；速度为 0 时是 `—`（口径同 [`SpeedFormat`]）。
    public let speedText: String
    /// 「活动 2 · 等待 3 · 已停止 4」。
    public let activityText: String
    /// 「清空已完成」那颗按钮能不能点。
    public let canClearFinished: Bool

    /// ⚠️ 三个数**一个都不能省**（约束 4）：只显示"活动"的话，一个"排队等着的"
    /// 和一个"已经失败的"在界面上就分不出来了。
    public static func of(_ global: GlobalStat) -> TransferGlobalSummary {
        TransferGlobalSummary(
            speedText: SpeedFormat.text(global.downloadSpeed),
            activityText: "活动 \(global.numActive) · 等待 \(global.numWaiting)"
                + " · 已停止 \(global.numStopped)",
            // 没有已结束的任务时**禁用**那颗按钮：点了之后界面毫无变化的话，
            // 用户分不清"清完了"还是"没生效"（约束 4）。
            canClearFinished: global.numStopped > 0)
    }
}

// ---------------------------------------------------------------------------
// 动作失败 → 界面上那句话
// ---------------------------------------------------------------------------

/// 一次 `task_action` 失败之后要显示的那句话。
///
/// ⚠️ **这是给视图用的唯一入口，视图自己不做映射**（全局约束 8：视图里不映射 `CoreError`）。
///    映射本身走 `AppModel.message(of:)` —— 那是 `CoreError` → 用户可见文案的**唯一实现**
///    （`DirLoadFailure.of` / `EnqueueFeedback.failure(of:)` 用的也是它）。
///    本类型**不抄第二份**：抄了就会出现"同一个错误在传输列表和别处措辞不同"。
///
/// 文案**逐字是内核原文**（约束 3）：动作失败的现场（`invalid_params("GID … 没有路径映射")`
/// 之类）只有原文说得清发生了什么。
public enum TransferActionFailure {
    public static func message(of error: Error) -> String {
        AppModel.message(of: error)
    }
}

// ---------------------------------------------------------------------------
// 顶部常驻横幅
// ---------------------------------------------------------------------------

/// 主区顶部那条横幅：「引擎/内核现在有问题」的唯一常驻落点。
///
/// ⚠️ 它是**两个来源**的落点，且优先级是**固定的**：
///   ① `engine == .unavailable` —— 引擎/内核真的不可用（断连、启动失败、握手超时、
///      重启失败），文案是 `engine` 里那句**原文**（约束 3），并且**带「重试」按钮**；
///   ② `lastError` —— 非抛错路径（轮询 / 校验刷新）上"其余错误码"的**非粘滞**出口
///      （任务 3 建立，到本任务才有消费者）。它**不带**「重试」：那颗按钮的语义是
///      "重启内核"，拿它去处理一次可自愈的抖动等于杀掉一个健康的引擎（约束 1）。
///
/// ⚠️ 两者同时有话说时**以 `engine` 为准**：它是当前连接的事实，而 `lastError` 是
///    上一拍的残留（下一次成功请求就清掉）。
public struct EngineBanner: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        /// 引擎/内核不可用（`engine == .unavailable`）。
        case engineUnavailable
        /// 瞬时错误（`lastError`）：下一拍成功就自己消失。
        case transientError
    }

    public let kind: Kind
    /// 给用户看的正文。**内核原文逐字**（约束 3）—— 壳不加工、不加前缀。
    public let text: String
    /// 壳写的补充说明；只有"内核一个字都没回"的握手超时才有一句（见
    /// [`EngineStatusPresentation.handshakeTimeoutTooltip`]）。
    public let hint: String?
    /// 要不要显示那颗**真的会生效**的「重试」按钮（`AppModel.retryEngine()`）。
    public let showsRetry: Bool

    public static func of(engine: AppModel.EngineState, lastError: String?) -> EngineBanner? {
        if case .unavailable(let why) = engine {
            // ⚠️ 覆盖 `.unavailable` 的**每一个**理由，不只是 `engine_disconnected` /
            //    `engine_start_failed`：任务 4b 的握手超时与重启失败都落在这里，
            //    而超时那句 tooltip 里写着"…然后点「重试」" —— 这颗按钮就是它的落点。
            return EngineBanner(
                kind: .engineUnavailable,
                text: why,
                // 超时是**壳探测到的**状况（内核一个字都没回，没有原文可登），
                // 所以只有这一支补一句"该去点什么"；内核自己给的失败原因不套壳的话。
                hint: EngineStatusPresentation.isHandshakeTimeout(why)
                    ? EngineStatusPresentation.handshakeTimeoutTooltip
                    : nil,
                showsRetry: true)
        }
        guard let lastError, !lastError.isEmpty else { return nil }
        return EngineBanner(kind: .transientError, text: lastError, hint: nil, showsRetry: false)
    }
}

// ---------------------------------------------------------------------------
// 空态文案
// ---------------------------------------------------------------------------

/// 列表区在没有行可显示时说什么。
///
/// ⚠️ `engine_not_started` **不是错误**（简报）：它只是"还没添加过任务"，
///    所以给的是空态文案，没有红色、没有惊叹号。
public enum TransferListEmpty {
    /// 引擎尚未启动（内核的 `engine_not_started`）。**逐字**（简报）。
    public static let engineNotStarted = "下载引擎尚未启动（还没有添加过任务）"
    /// 引擎在跑、但确实没有任务。
    public static let nothingInFlight = "没有正在传输的任务"

    /// `nil` = 这里不说话。
    ///
    /// ⚠️ 引擎不可用时返回 `nil`：顶部横幅（常驻、带「重试」）已经在说同一句话，
    ///    两处重复同一句原文只会让真正要看的那条变淡。
    public static func of(engine: AppModel.EngineState) -> String? {
        switch engine {
        case .notStarted: return engineNotStarted
        case .running, .unknown: return nothingInFlight
        case .unavailable: return nil
        }
    }
}

// ---------------------------------------------------------------------------
// 准入闸门（约束 4 / 15）
// ---------------------------------------------------------------------------

/// 「这个引擎状态能不能发请求」——**界面侧**的准入判据。
///
/// ⚠️ 为什么必须有它（约束 4 明禁的形态）：内核卡死时（首次启动卡在 TCC 授权框上的
///    `open()` 里），那条请求**永不返回**，而 `CoreClient` 内部是一条 FIFO **串行**队列
///    （约束 15 的单飞语义就靠它）—— 队列被永久堵死，此后每个新请求都排在它后面、
///    永远轮不到。于是用户点下去看到的是一颗再也没有回音的按钮、或者一句
///    「正在读取传输列表…」永远转下去。**静默挂死**正是本约束要防的那件事。
///
/// `AppModel` 里还有一道同源的闸门（`requireUsableEngine`，任务 5/6/7 建立），
/// 两者是**互补**的：那里的判据只看 `.unavailable`（`.unknown` = 握手还没结论，
/// 请求排在握手后面是正常的，而且握手有 5 秒上界）；这里把 `.unknown` 也关掉，
/// 因为界面此刻能给的替代动作是"等"或"点重试"，而不是发一条可能永远排不到的请求。
public enum EngineGate {
    /// 会发请求的界面动作能不能点。
    public static func allowsRequests(_ engine: AppModel.EngineState) -> Bool {
        switch engine {
        case .running, .notStarted: return true
        case .unknown, .unavailable: return false
        }
    }

    /// 动作被禁用时挂在 `.help` 上的那句话。
    ///
    /// 它是**壳自己写的**（同 `AppModel.busyReason` / `DirLoadFailure.notice` 的性质）：
    /// 它描述的是**界面动作**（"去点那颗按钮"），内核不知道也没有对应的话可说。
    /// 约束 3 管的是"内核说了什么"，不是"壳对自己的界面元素怎么指路"。
    public static let unavailableHelp = "引擎不可用，请先点顶部的「重试」"
}

/// 轮询节拍。**规格 §5.3 定稿 200 ms**（拉、不推）。
///
/// 这个值同时是"界面跟得上"与"不给内核添无谓负载"的平衡点；定时器**只在传输列表
/// 可见时**跑（切走就停）。放在 `Presentation/` 而不是视图里，是为了让这个数
/// 与"它凭什么这么大"的论证待在一起、并且能被单测钉住。
public enum TransferListPoll {
    public static let intervalNanoseconds: UInt64 = 200_000_000
}

// ---------------------------------------------------------------------------
// 「在访达中显示」：清单路径 → 落盘路径
// ---------------------------------------------------------------------------

/// 把内核给的**清单相对路径**换算成落盘绝对路径。
///
/// ⚠️ **`TransferItem.path` 不是文件系统路径**，这一条是本节存在的全部理由：
///    - 它是清单里的相对路径（`core/src/engine/daemon.rs` 的 `path_map`，测试夹具
///      `PFX/sub/a.txt` 就是它）；
///    - 内核交给 aria2 时把它**拆成 `dir`/`out`**（`core/src/engine/mod.rs` 的
///      `new_planned_file`），而 aria2 的工作目录就是下载根 —— 所以落盘位置是
///      `下载根 + "/" + 清单路径`；
///    - 于是 `URL(fileURLWithPath: 清单路径)` 解析出来的是**进程 cwd**（GUI 应用是 `/`）
///      下的一个不存在的文件：一颗点了没反应的按钮（约束 4 明禁的静默失效）。
///
/// ⚠️ 落盘根为什么不从协议里来：协议里**没有**这个字段（`load_delivery` 的树是相对清单），
///    它只有两个来源，都在这层：
///      ① **壳配过下载目录**（阶段 E 任务 3：argv `--download-dir`）⇒ 落盘根就是它；
///      ② 没配过 ⇒ 内核用它自己的默认值 `$HOME/Downloads/Benagen`（`core/src/main.rs:1620`），
///         而 `$HOME` 这一段的取法与内核**逐字一致**（见 [`home(environment:fallback:)`]）。
///    所以这里钉的是**两边的约定**：内核改了那个默认值，[`downloadRootRelativeToHome`]
///    这个常量要跟着改（`theDownloadRootAnchorIsPinnedLocally` 守着它）。
///
/// ⚠️ **阶段 E 之前这里只有一个分支**（壳从不传 `--download-dir`），所以落盘根是个常量；
///    现在它由 [`downloadRoot(home:downloadDir:)`] 算 —— 少了这一步，用户配了自定义目录之后
///    「在访达中显示」会指向 `~/Downloads/Benagen` 下的一个**不存在**的文件，
///    而那正是约束 4 明禁的静默失效（按钮点得动、什么都没发生）。
///
/// 本类型**不 import AppKit**（`Presentation/` 不含 UI）：`NSWorkspace` 那一句留在视图里。
public enum TransferReveal {
    /// 内核默认下载目录相对 `$HOME` 的那一段。
    public static let downloadRootRelativeToHome = "Downloads/Benagen"

    /// 内核给的路径 → 可用的清单路径。
    ///
    /// `nil`（内核发 `"path": null`）与**空串**都是"没有映射"：空串渲染出来是一个
    /// 空白行标题，与 `nil` 在界面上的差别只有"看起来像坏了"。
    /// 非空时**逐字**返回（约束 3：不取最后一段、不解码、不折叠）。
    public static func manifestPath(_ path: String?) -> String? {
        guard let path, !path.isEmpty else { return nil }
        return path
    }

    /// 内核解析下载根用的那个 HOME。
    ///
    /// ⚠️ **必须与内核读的是同一个来源**：内核的默认下载根是 `$HOME/Downloads/Benagen`，
    ///    而 `core/src/main.rs` 读的是 **HOME 环境变量**（`std::env::var_os("HOME")`）。
    ///    壳起内核时 `ProcessChannel` 继承环境，所以壳读同一个变量就与内核**逐字一致**。
    ///
    /// ⚠️ **不要用 `FileManager.homeDirectoryForCurrentUser`**（那是 getpwuid 的登录用户
    ///    主目录，**不看 HOME 环境变量**）：本任务的走查实测撞上过这个分叉 ——
    ///    `HOME=/tmp/t8home` 时它照样回 `/Users/starsyi`，而内核把文件下到了
    ///    `/tmp/t8home/Downloads/Benagen`；照它拼出来的路径是一个**不存在**的文件，
    ///    而"在访达中显示"指到空处正是简报点名不要的静默无效。
    ///    两条路在"用户正常双击启动"时结果相同，但**相同是巧合，不是保证**。
    public static func home(environment: [String: String], fallback: String) -> String {
        guard let h = environment["HOME"], !h.isEmpty else { return fallback }
        return h
    }

    /// 落盘根：**配过下载目录就是它本身**，没配过才是 `$HOME/Downloads/Benagen`。
    ///
    /// ⚠️ `downloadDir` 是**用户配的那个目录原文**（`AppModel.downloadDir`，空串 = 未配置）。
    ///    这里**不做任何规范化**（不去 `..`、不展开软链接、不改大小写）：它就是内核
    ///    argv 上那一格（`--download-dir`），壳替它解释一遍只会让"壳以为的路径"与
    ///    "内核实际写的路径"悄悄分叉 —— 而分歧的代价是**按钮指到别处**
    ///    （同 `AppPreferences.normalized` 那条纪律）。只去掉末尾多余的 `/`
    ///    （那是**壳自己的**拼接产物，不是内核原文，不受约束 3 管）。
    public static func downloadRoot(home: String, downloadDir: String) -> String {
        if !downloadDir.isEmpty { return trimmingTrailingSlashes(downloadDir) }
        let h = trimmingTrailingSlashes(home)
        let prefix = h.isEmpty ? "" : (h.hasSuffix("/") ? h : h + "/")
        return prefix + downloadRootRelativeToHome
    }

    /// 清单路径 → 落盘绝对路径；没有路径时 `nil`（调用方据此**禁用**菜单项）。
    public static func localPath(manifestPath: String?, home: String, downloadDir: String) -> String? {
        guard let rel = Self.manifestPath(manifestPath) else { return nil }
        // 这里是**壳自己的**路径拼接，不受约束 3 管：末尾多一个 `/` 只是拼出 `//`，
        // 先去干净（清单路径那一段一个字符都不动）。
        let root = downloadRoot(home: home, downloadDir: downloadDir)
        let prefix = root.isEmpty ? "" : root + "/"
        return prefix + rel
    }

    /// 去掉末尾的 `/`（`/` 本身与空串除外 —— 把它去没了就不是一条路径了）。
    private static func trimmingTrailingSlashes(_ path: String) -> String {
        var trimmed = path
        while trimmed.count > 1, trimmed.hasSuffix("/") { trimmed.removeLast() }
        return trimmed
    }
}
