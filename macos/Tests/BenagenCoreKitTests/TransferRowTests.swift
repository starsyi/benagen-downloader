import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 传输列表的呈现模型（任务 8）
//
// 夹具走**解码**这条路（`TransferItem.fixture` 在文件末尾，internal，任务 9 可复用）：
// 手搓一个 `TransferItem` 结构体会让"壳解不解得动内核发来的那一行"这件事在测试里凭空消失
// ——而 `path: null`（不是省略键）正是本任务要处理的那种形状。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 行：标题 / 角标 / 错误原文
// ---------------------------------------------------------------------------

@Test func pausedIsRenderedAsABadgeOnADownloadingRow() {
    // 内核把 aria2 的 paused 与 waiting 都映射成 state == "waiting"，
    // 区分只能靠 raw_status（阶段 A 裁决 #88）
    let it = TransferItem.fixture(state: .waiting, rawStatus: "paused")
    #expect(TransferRow(it).showsPausedBadge == true)
    let it2 = TransferItem.fixture(state: .waiting, rawStatus: "waiting")
    #expect(TransferRow(it2).showsPausedBadge == false)
}

@Test func nilPathIsNotRenderedAsTheWordNil() {
    // 内核发 "path": null 而不是省略
    let it = TransferItem.fixture(path: nil)
    #expect(TransferRow(it).title == "（未知路径）")
}

@Test func anEmptyPathIsAlsoUnknownRatherThanABlankRow() {
    // ⚠️ 夹具让"只挡 nil"的实现活着就是给变异体打掩护：`String?` 为空串时
    //    `path ?? 占位` 会渲染出一个**空白行标题**（约束 4 的静默失效）。
    //    内核为空串的语义同样是"不知道"（同 `raw_status` 取不到时传空串）。
    let it = TransferItem.fixture(path: "")
    #expect(TransferRow(it).title == "（未知路径）", "空串标题在界面上就是什么都没说")
    #expect(TransferRow(it).manifestPath == nil, "空串与 nil 在呈现层是同一件事：没有可用的路径")
}

@Test func aPathIsShownVerbatim() {
    // 约束 3：清单原文逐字 —— 不取最后一段、不解码 `×`、不折叠空格。
    let raw = "sub dir/C24-8_×_25WS024/QC 图.png"
    let row = TransferRow(TransferItem.fixture(path: raw))
    #expect(row.title == raw)
    #expect(row.manifestPath == raw)
}

@Test func errorRowsShowTheAriaMessageVerbatim() {
    // aria2 的 errorMessage 不得改写、不得截断、不得清空（约束 3）。
    // 换行是刻意的：多行原文在界面上要**全文**显示（简报）。
    let why = "连接超时（第 3 次重试）\n原因：Connection reset by peer"
    let row = TransferRow(TransferItem.fixture(state: .error, rawStatus: "error", errorMessage: why))
    #expect(row.errorText == why)
    #expect(row.stateLabel == TransferRow(TransferItem.fixture(state: .error)).stateLabel,
            "有没有错误消息不该改变状态那一列")
}

@Test func aRowWithoutAnErrorMessageHasNoErrorLine() {
    // 没有错误消息 ⇒ 不渲染那一行（nil ≠ 空串：空串会在行下方留一条看不见的空行）。
    let row = TransferRow(TransferItem.fixture(state: .active, errorMessage: ""))
    #expect(row.errorText == nil)
}

// ---------------------------------------------------------------------------
// 行：进度
// ---------------------------------------------------------------------------

@Test func progressFractionIsZeroWhenTotalIsZero() {
    // 不得 NaN：`ProgressView(value:)` 拿到 NaN 会画出一条毫无意义的进度条。
    let row = TransferRow(TransferItem.fixture(total: 0, completed: 0))
    #expect(row.progressFraction == 0)
    #expect(!row.progressFraction.isNaN)
    #expect(row.percentText == "—", "总量为 0 时百分比是占位符（口径同 PercentFormat）")
}

@Test func progressFractionIsClampedToTheUnitRange() {
    // 超报（completed > total）夹到 1，负值夹到 0 —— 与 `PercentFormat` 同一条口径。
    #expect(TransferRow(TransferItem.fixture(total: 100, completed: 250)).progressFraction == 1)
    #expect(TransferRow(TransferItem.fixture(total: 100, completed: -5)).progressFraction == 0)
    // 正常值必须**按比例**，不是常数：这一条守着"夹住了就完事"的实现。
    #expect(TransferRow(TransferItem.fixture(total: 400, completed: 100)).progressFraction == 0.25)
    #expect(TransferRow(TransferItem.fixture(total: 400, completed: 100)).percentText == "25%")
}

@Test func theProgressLineReadsCompletedOverTotal() {
    // 三列里的「进度」列 = 已完成/总量 + 百分比（简报）。
    let row = TransferRow(TransferItem.fixture(total: 2048, completed: 1024))
    #expect(row.progressText == "1.0 KB / 2.0 KB")
    #expect(row.percentText == "50%")
    #expect(row.speedText == "512 B/s")
}

// ---------------------------------------------------------------------------
// 行：状态与动作准入
// ---------------------------------------------------------------------------

@Test func everyTaskStateHasAVisibleLabel() {
    // 五档（含 removed）一个不能少，且两两不同（约束 4：不得静默失效 ——
    // 两个状态画成一样，等于其中一个永远不会出现在界面上）。图标同理。
    //
    // ⚠️ **只断言"两两不同"是不够的**（同下面 `everyTaskStateMapsToItsOwnColor` 那段）：
    //    把 `.active`（"下载中"）与 `.complete`（"已完成"）的标签对调、或把 `.waiting`
    //    的 `clock` 与 `.removed` 的 `trash` 对调，`Set(...).count == 5` 照样成立 ——
    //    客户看到的就是"下完的显示下载中、正在下的显示已完成"。所以这里**逐档钉死**
    //    哪一档是哪个词、哪颗图标（做法同 `BrowserRowTests` 把 `FileState` 四条文案
    //    逐字钉死那条）。
    let expected: [TaskState: (label: String, icon: String)] = [
        .waiting: ("等待中", "clock"),
        .active: ("下载中", "arrow.down.circle"),
        .complete: ("已完成", "checkmark.circle"),
        .error: ("失败", "exclamationmark.triangle"),
        .removed: ("已移除", "trash"),
    ]
    #expect(expected.count == TaskState.allCases.count,
            "五档必须一档不漏地钉在这里 —— 漏掉的那档就没人守了")
    for state in TaskState.allCases {
        guard let want = expected[state] else {
            Issue.record("\(state.rawValue) 没有期望值：新加一档就要在上面的表里补一行")
            continue
        }
        let row = TransferRow(TransferItem.fixture(state: state))
        #expect(row.stateLabel == want.label, "\(state.rawValue) 的标签不对")
        #expect(row.stateIconName == want.icon, "\(state.rawValue) 的图标不对")
    }

    let labels = TaskState.allCases.map { TransferRow(TransferItem.fixture(state: $0)).stateLabel }
    #expect(labels.count == 5)
    #expect(labels.allSatisfy { !$0.isEmpty }, "任何一档都不许是空白：\(labels)")
    #expect(Set(labels).count == 5, "五档必须两两不同：\(labels)")

    let icons = TaskState.allCases.map { TransferRow(TransferItem.fixture(state: $0)).stateIconName }
    #expect(icons.allSatisfy { !$0.isEmpty }, "任何一档都不许没有图标：\(icons)")
    #expect(Set(icons).count == 5, "五档的图标必须两两不同：\(icons)")
}

@Test func everyTaskStateMapsToItsOwnColor() {
    // ⚠️ 五档的**颜色**与 label / icon 同等要求：两两不同（约束 4）。
    //    只断言"五个色值互不相等"是不够的（把 error 的红与 complete 的绿对调照样全绿），
    //    所以这里同时把**每一档的具体语义色**钉住。
    let expected: [TaskState: RowColor] = [
        .waiting: .secondary,     // 中性：还没轮到
        .active: .blue,           // 进行中
        .complete: .green,        // 成功
        .error: .red,             // 错误
        .removed: .orange,        // 警告：这一行没有任何可用动作，出路在列表级「清空已完成」
    ]
    for (state, color) in expected {
        #expect(TransferRow(TransferItem.fixture(state: state)).stateColor == color,
                "\(state.rawValue) 的语义色不对")
    }
    let colors = TaskState.allCases.map { TransferRow(TransferItem.fixture(state: $0)).stateColor }
    #expect(Set(colors).count == 5, "五档的颜色必须两两不同：\(colors)")
}

@Test func actionAvailabilityFollowsTheState() {
    // pause 只对 active 可用；unpause 只对 paused 可用；retry/remove 对已停止的可用
    #expect(TransferRow(TransferItem.fixture(state: .active)).availableActions.contains(.pause))
    #expect(!TransferRow(TransferItem.fixture(state: .complete)).availableActions.contains(.pause))
    #expect(TransferRow(TransferItem.fixture(state: .error)).availableActions.contains(.retry))
}

@Test func thePausedRowOffersUnpauseInsteadOfPause() {
    // 「已暂停」那一行要能继续（`unpause`），而不是再给一颗按了没用的「暂停」。
    let paused = TransferRow(TransferItem.fixture(state: .waiting, rawStatus: "paused"))
    #expect(paused.availableActions.contains(.unpause))
    #expect(!paused.availableActions.contains(.pause))

    // 排队等待（没暂停）的那一行相反：能暂停，不能继续。
    let queued = TransferRow(TransferItem.fixture(state: .waiting, rawStatus: "waiting"))
    #expect(queued.availableActions.contains(.pause))
    #expect(!queued.availableActions.contains(.unpause))
}

@Test func finishedRowsOfferRetryAndRemoveButNotPause() {
    for state in [TaskState.complete, .error] {
        let row = TransferRow(TransferItem.fixture(state: state))
        #expect(row.availableActions.contains(.retry), "\(state) 上不能重试")
        #expect(row.availableActions.contains(.remove), "\(state) 上不能移除")
        #expect(!row.availableActions.contains(.pause), "\(state) 上不该有暂停")
        #expect(!row.availableActions.contains(.unpause), "\(state) 上不该有继续")
    }
}

@Test func aRemovedRowOffersNothingAtAll() {
    // `removed` 的 GID 在内核里已经**没有路径映射**（`remove` 会 forget 掉），
    // 于是 retry 必然回 `invalid_params("GID … 没有路径映射")`、remove 也已经无事可做。
    // 给一颗注定报错的按钮比没有按钮更糟（约束 4：宁可什么都不给，也不给假的）。
    #expect(TransferRow(TransferItem.fixture(state: .removed)).availableActions.isEmpty)
}

@Test func noRowOffersTheListWideClearFinishedAction() {
    // `clear_finished` **不是**行级动作（它没有 gid，语义是"清空全部已结束的"）——
    // 它混进行级菜单里的后果是用户以为只清这一行，实际清掉一片。
    for state in TaskState.allCases {
        let row = TransferRow(TransferItem.fixture(state: state))
        #expect(!row.availableActions.contains(.clearFinished), "\(state) 行上不该出现清空")
    }
}

@Test func aRowWithoutAGidOffersNoActions() {
    // 没有 GID 就没有动作可发（内核会回 `invalid_params("动作 … 需要 gid")`）——
    // 让按钮可点等于把一条注定报错的请求发出去。
    #expect(TransferRow(TransferItem.fixture(gid: "", state: .active)).availableActions.isEmpty)
}

@Test func activeRowsCanBeRemovedButNotRetried() {
    // 正在下载的那一行：能暂停、能移除，但**不能重试**（重试是"停止之后再来一次"）。
    let row = TransferRow(TransferItem.fixture(state: .active))
    #expect(row.availableActions.contains(.remove))
    #expect(!row.availableActions.contains(.retry))
}

// ---------------------------------------------------------------------------
// 动作失败 → 界面上那句话
// ---------------------------------------------------------------------------

@Test func actionFailuresShowTheKernelMessageVerbatim() {
    // 动作失败的落点是列表上方那条提示：**内核原文逐字**（约束 3），一个字都不加工。
    // 走的是与 `DirLoadFailure` / `EnqueueFeedback` 同一个映射，不抄第二份。
    let rpc = CoreError.rpc(code: "invalid_params", message: "GID g1 没有路径映射（可能已被移除）")
    #expect(TransferActionFailure.message(of: rpc) == "GID g1 没有路径映射（可能已被移除）")
    #expect(TransferActionFailure.message(of: CoreError.transport("内核进程已退出（管道结束）"))
            == "内核进程已退出（管道结束）")
    // 结构化 `code` **不进文案**（契约 §5.1：壳按 code 分支，给用户看的是 message）。
    #expect(!TransferActionFailure.message(of: rpc).contains("invalid_params"))
}

// ---------------------------------------------------------------------------
// 列表顶部：全局速度与活动数
// ---------------------------------------------------------------------------

@Test func theGlobalSummaryShowsSpeedAndActivity() {
    // 四个数刻意取**互不相同**的值：字段串位（active ↔ waiting ↔ stopped）
    // 必须被这条抓住 —— 同类项恰好相等的夹具是给变异体打掩护。
    let g = GlobalStat(downloadSpeed: 900, numActive: 2, numWaiting: 3, numStopped: 4)
    let s = TransferGlobalSummary.of(g)
    #expect(s.speedText == "900 B/s")
    #expect(s.activityText.contains("2") && s.activityText.contains("3") && s.activityText.contains("4"))
    #expect(s.activityText == "活动 2 · 等待 3 · 已停止 4")
}

@Test func aZeroSpeedIsAPlaceholderNotZeroBytesPerSecond() {
    // 口径同 `SpeedFormat`：速度为 0 时是"不知道/已停"，不是"每秒零字节"。
    #expect(TransferGlobalSummary.of(GlobalStat(downloadSpeed: 0, numActive: 0,
                                                numWaiting: 0, numStopped: 0)).speedText == "—")
}

@Test func clearFinishedIsOfferedOnlyWhenSomethingHasStopped() {
    // 没有已结束的任务时那颗按钮**禁用** —— 否则用户点了之后界面毫无变化，
    // 分不清"清完了"还是"没生效"（约束 4）。
    #expect(TransferGlobalSummary.of(GlobalStat(downloadSpeed: 0, numActive: 1,
                                                numWaiting: 0, numStopped: 0)).canClearFinished == false)
    #expect(TransferGlobalSummary.of(GlobalStat(downloadSpeed: 0, numActive: 1,
                                                numWaiting: 0, numStopped: 1)).canClearFinished == true)
}

// ---------------------------------------------------------------------------
// 顶部横幅（引擎不可用 / 瞬时错误）
// ---------------------------------------------------------------------------

@Test func everyUnavailableEngineGetsABannerWithTheReasonVerbatim() {
    // 横幅必须覆盖 `engine == .unavailable` 的**每一个**理由，不只是
    // `engine_disconnected` / `engine_start_failed`：任务 4b 的握手超时、重启失败
    // 都落在 `.unavailable` 上。文案用 `engine` 里那句原文（约束 3）。
    for why in ["下载引擎已断开：内核进程已退出（管道结束）",
                "引擎启动失败：aria2c 没有在 30 秒内就绪",
                "内核重启失败：找不到内核可执行文件 benagen-core",
                AppModel.handshakeTimeoutMessage] {
        let banner = EngineBanner.of(engine: .unavailable(why), lastError: nil)
        #expect(banner?.text == why, "横幅文案必须是内核那句原文，不是壳编的：\(why)")
        #expect(banner?.showsRetry == true, "引擎不可用时必须有「重试」：\(why)")
    }
}

@Test func theBannerCoversTheHandshakeTimeoutWithItsActionableHint() {
    // 任务 4b 那句超时文案的 tooltip 里写着"…然后点「重试」"—— 本任务就是那颗按钮的落点。
    // 横幅上除了原文（是什么），还要有那句"该怎么办"（壳写的，理由见
    // `EngineStatusPresentation.handshakeTimeoutTooltip`）。
    let banner = EngineBanner.of(engine: .unavailable(AppModel.handshakeTimeoutMessage), lastError: nil)
    #expect(banner?.text == AppModel.handshakeTimeoutMessage)
    #expect(banner?.hint == EngineStatusPresentation.handshakeTimeoutTooltip)
}

@Test func aKernelReportedFailureGetsNoShellWrittenHint() {
    // 内核自己给的失败原因**不套壳的指导语**：那句话是给"内核一个字都没回"的现场写的，
    // 挂在一条内核已经说清楚原因的失败上就是壳在替内核解释（约束 3）。
    let banner = EngineBanner.of(engine: .unavailable("引擎启动失败：aria2c 没有在 30 秒内就绪"),
                                 lastError: nil)
    #expect(banner?.hint == nil)
}

@Test func theRetryButtonIsOfferedOnlyWhenTheEngineItselfIsDown() {
    // 瞬时错误（`engine_rpc_failed` / 一行垃圾）**不该**给「重试」：那颗按钮的语义是
    // "重启内核"，拿它去处理一次可自愈的抖动等于杀掉一个健康的引擎（约束 1）。
    let transient = EngineBanner.of(engine: .running, lastError: "下载引擎 RPC 失败：connection reset")
    #expect(transient?.text == "下载引擎 RPC 失败：connection reset")
    #expect(transient?.showsRetry == false)
    #expect(transient?.hint == nil)
}

@Test func aUsableEngineWithoutAnErrorHasNoBanner() {
    // 没有要说的就不挂一条横幅（空横幅会把"一切正常"也变成一条要读的东西）。
    #expect(EngineBanner.of(engine: .running, lastError: nil) == nil)
    #expect(EngineBanner.of(engine: .notStarted, lastError: nil) == nil)
    #expect(EngineBanner.of(engine: .unknown, lastError: nil) == nil)
}

@Test func theEngineReasonWinsOverAStaleTransientError() {
    // 两个来源同时有话说时，**以 `engine` 为准**：它是当前连接的事实，
    // 而 `lastError` 是"上一拍"的残留（非粘滞出口，下一次成功就清）。
    let banner = EngineBanner.of(engine: .unavailable("下载引擎已断开：内核进程已退出（管道结束）"),
                                 lastError: "上一拍的瞬时错误")
    #expect(banner?.text == "下载引擎已断开：内核进程已退出（管道结束）")
    #expect(banner?.showsRetry == true)
}

// ---------------------------------------------------------------------------
// 空态文案（engine_not_started **不是错误**）
// ---------------------------------------------------------------------------

@Test func engineNotStartedIsPresentedAsAnEmptyStateNotAnError() {
    // 简报：`engine_not_started` → 「下载引擎尚未启动（还没有添加过任务）」，不显示为错误。
    let text = TransferListEmpty.of(engine: .notStarted)
    #expect(text == "下载引擎尚未启动（还没有添加过任务）")
}

@Test func anEmptyRunningListSaysNothingIsInFlight() {
    // 引擎在跑、列表为空 ⇒ 是"没有任务"，不是"引擎没起来"。
    #expect(TransferListEmpty.of(engine: .running) == "没有正在传输的任务")
}

@Test func anUnavailableEngineLeavesTheListAreaToTheBanner() {
    // 引擎不可用时**不在这里再说一遍**：顶部横幅（常驻、带「重试」）已经说了同一句话，
    // 两处重复同一句原文只会让真正要看的那条变淡。
    #expect(TransferListEmpty.of(engine: .unavailable("下载引擎已断开：内核进程已退出（管道结束）")) == nil)
}

// ---------------------------------------------------------------------------
// 准入闸门（约束 4 / 15）
// ---------------------------------------------------------------------------

@Test func theRequestGateIsOpenOnlyForRunningAndNotStarted() {
    // 内核卡死时 FIFO 队列被那条永不返回的请求永久堵死，此后每个新请求都永远挂着 ——
    // 界面会停在「正在读取传输列表…」再也不动（约束 4 明禁的静默挂死）。
    #expect(EngineGate.allowsRequests(.running))
    #expect(EngineGate.allowsRequests(.notStarted))
    #expect(!EngineGate.allowsRequests(.unknown))
    #expect(!EngineGate.allowsRequests(.unavailable("下载引擎已断开：内核进程已退出（管道结束）")))
    #expect(!EngineGate.allowsRequests(.unavailable(AppModel.handshakeTimeoutMessage)))
    // 提示语是壳写的**界面动作说明**（同 `DirLoadFailure.notice` 的性质），不是内核原文。
    #expect(EngineGate.unavailableHelp.contains("重试"), "提示必须指向那颗真的存在的按钮")
}

// ---------------------------------------------------------------------------
// 轮询节拍与「在访达中显示」
// ---------------------------------------------------------------------------

@Test func thePollingCadenceMatchesTheSpec() {
    // 规格 §5.3 定稿 200 ms。这个数同时是"界面跟得上"与"不给内核添无谓负载"的平衡点，
    // 改动必须是有意的。
    #expect(TransferListPoll.intervalNanoseconds == 200_000_000)
}

@Test func theRevealPathIsJoinedOntoTheDownloadRoot() {
    // ⚠️ **`TransferItem.path` 是清单相对路径，不是落盘路径**：内核把它交给 aria2 时
    //    拆成了 `dir`/`out`（`core/src/engine/mod.rs` 的 `new_planned_file`），落盘位置是
    //    `下载根 + "/" + 清单路径`。直接把相对路径当文件路径交给 `NSWorkspace`，
    //    解析出来的是 cwd（GUI 应用是 `/`）下的一个不存在的文件 —— 一颗点了没反应的按钮。
    #expect(TransferReveal.localPath(manifestPath: "sub dir/QC 图.png", home: "/Users/x",
                                     downloadDir: "")
            == "/Users/x/Downloads/Benagen/sub dir/QC 图.png")
    #expect(TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x", downloadDir: "")
            == "/Users/x/Downloads/Benagen/a.bin")
    // 根末尾的 `/` 不产生双斜杠（这是**壳自己的**路径，不是内核原文，不受约束 3 管）。
    #expect(TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x/", downloadDir: "")
            == "/Users/x/Downloads/Benagen/a.bin")
}

@Test func aConfiguredDownloadDirectoryMovesTheRevealRoot() {
    // ⚠️ 阶段 E 的任务 3 让壳**可以**指定下载目录了（argv `--download-dir`）。
    //    配过之后落盘根**就是它**，而不是 `$HOME/Downloads/Benagen` ——
    //    少了这一条，那颗「在访达中显示」会指向一个不存在的文件，正是约束 4 明禁的
    //    静默失效（用户改了目录之后，这个按钮是**唯一**会被悄悄弄坏的东西）。
    #expect(TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x",
                                     downloadDir: "/Volumes/Data/交付")
            == "/Volumes/Data/交付/a.bin")
    #expect(TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x",
                                     downloadDir: "/Volumes/Data/交付/")
            == "/Volumes/Data/交付/a.bin", "末尾的 `/` 同样不产生双斜杠")
    // 与内核默认值**逐字不同**的两侧都要看得见（这条用例的判别力就在这里）。
    #expect(TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x", downloadDir: "")
            != TransferReveal.localPath(manifestPath: "a.bin", home: "/Users/x",
                                        downloadDir: "/Volumes/Data/交付"))
}

@Test func theDownloadRootIsTheConfiguredDirectoryOrTheKernelsDefault() {
    // 判据本身（两个分支各一条），以及"未配置"与"配了一个正好等于默认值的目录"
    // 在这层是**同一件事** —— 区分它们是 argv 那一侧的事（E-5：未配置 ⇒ 不传）。
    #expect(TransferReveal.downloadRoot(home: "/Users/x", downloadDir: "")
            == "/Users/x/Downloads/Benagen")
    #expect(TransferReveal.downloadRoot(home: "/Users/x/", downloadDir: "")
            == "/Users/x/Downloads/Benagen", "HOME 末尾的 `/` 不产生双斜杠")
    #expect(TransferReveal.downloadRoot(home: "/Users/x", downloadDir: "/Volumes/Data/交付")
            == "/Volumes/Data/交付")
    #expect(TransferReveal.downloadRoot(home: "/Users/x", downloadDir: "/Volumes/Data/交付/")
            == "/Volumes/Data/交付")
}

@Test func aPathlessRowCannotBeRevealed() {
    // path == nil 时该菜单项**禁用**，不要静默无效（简报）。
    let row = TransferRow(TransferItem.fixture(path: nil))
    #expect(row.canRevealInFinder == false)
    #expect(TransferReveal.localPath(manifestPath: row.manifestPath, home: "/Users/x",
                                     downloadDir: "") == nil)
    #expect(TransferRow(TransferItem.fixture(path: "")).canRevealInFinder == false)
    #expect(TransferReveal.localPath(manifestPath: "", home: "/Users/x", downloadDir: "") == nil)
    #expect(TransferRow(TransferItem.fixture(path: "a.bin")).canRevealInFinder == true)
}

@Test func theDownloadRootUsesTheSameHomeSourceAsTheKernel() {
    // 内核读的是 **HOME 环境变量**（`core/src/main.rs` 的 `std::env::var_os("HOME")`），
    // 壳必须读同一个来源：走查实测 `FileManager.homeDirectoryForCurrentUser` 走的是
    // getpwuid —— `HOME=/tmp/t8home` 时它照样回 `/Users/starsyi`，而内核把文件下到了
    // `/tmp/t8home/Downloads/Benagen`。照它拼出来的路径指向一个不存在的文件，而
    // 「在访达中显示」指到空处正是简报点名不要的静默无效。
    #expect(TransferReveal.home(environment: ["HOME": "/tmp/t8home"], fallback: "/Users/x")
            == "/tmp/t8home")
    // HOME 缺失/为空时回落（GUI 应用里它总是有；这条只为不让回落变成空串）。
    #expect(TransferReveal.home(environment: [:], fallback: "/Users/x") == "/Users/x")
    #expect(TransferReveal.home(environment: ["HOME": ""], fallback: "/Users/x") == "/Users/x")
}

@Test func theDownloadRootAnchorIsPinnedLocally() {
    // ⚠️ **这不是"内核默认值"的判据，别把它读成那个**（名字原来是
    //    `theDownloadRootMatchesTheKernelsDefault`，那个名字给了过强的暗示，已改）：
    //    它断言的是**壳里这个本地锚点**没有被无声改掉。
    //
    // 壳不传 `--download-dir`（计划里的业务决定），所以内核用的是它自己的默认值
    // `$HOME/Downloads/Benagen`（`core/src/main.rs:1620`）。
    // **内核改了那个默认值，这一条不会红** —— 本项目的测试不许依赖 `core/` 的仓库布局
    // （约束 10 也不许改它），所以两边的一致性只能**人工交叉核对**：
    // 改内核默认值、或让壳显式传 `--download-dir` 时，请一并读 `core/src/main.rs` 的
    // `parse_args` 并更新这个常量。
    #expect(TransferReveal.downloadRootRelativeToHome == "Downloads/Benagen")
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

extension TransferItem {
    /// 一条 `transfer_list` 条目，**经解码构造**（键是内核线上的 snake_case）。
    ///
    /// `path` 传 `nil` 时发的是 `"path":null`（内核的 `Option` 没有 `skip_serializing_if`，
    /// 不是省略键）——夹具的形状与真内核一致，壳解不解得动才是被测的东西。
    ///
    /// ⚠️ **不是 `private`**：任务 9 的传输徽标也用得上（计划里的跨任务接口）。
    ///    值取自 `JSONSerialization`（类型化的入参），`try!` 不可能失败。
    static func fixture(gid: String = "g-1",
                        total: Int64 = 1000,
                        completed: Int64 = 250,
                        speed: Int64 = 512,
                        conns: Int32 = 2,
                        state: TaskState = .active,
                        rawStatus: String = "active",
                        errorMessage: String = "",
                        path: String? = "a.bin") -> TransferItem {
        var obj: [String: Any] = [
            "gid": gid,
            "total": total,
            "completed": completed,
            "speed": speed,
            "conns": Int(conns),
            "state": state.rawValue,
            "raw_status": rawStatus,
            "error_message": errorMessage,
        ]
        obj["path"] = path ?? NSNull()
        let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        return try! CoreJSON.decoder.decode(TransferItem.self, from: data)
    }
}
