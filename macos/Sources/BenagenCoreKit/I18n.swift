import Foundation

// ---------------------------------------------------------------------------
// 壳自己写的那几句话的双语表（规格 I-2；样板是 `windows/shell-core/src/i18n.rs`）。
//
// ⚠️ **为什么壳里也要一张表**：内核给的文案已经按语言拼好（`core/src/i18n`），但壳还有
//    一批**它自己写的**句子 —— 引擎徽标、重启进度、错误卡、设置面板、文件页、传输列表、
//    校验摘要、诊断导出说明……它们没有"内核原文"可以照登（约束 3 只覆盖内核说的那部分），
//    所以必须自己双语。
//
// 两条纪律：
//
//   1. **判据在 Rust、文案在表里、前端只画**（I-1）：视图里不许出现界面文案，
//      只把这里给的字放进 `Text`。
//   2. **日志不翻译**（I-2）：这里只管客户看得见的字。
//
// 与内核那套的分工：内核的 `Lang` 由握手落定（`hello` 回执里的 `language`），
// 壳从回执里抄一份到 `AppModel.language`，此后所有壳侧构造点都读那一格 ——
// **壳不自己看系统 locale**，否则两端各算一次、语言就会在一句话里打架。
// ---------------------------------------------------------------------------

/// 界面语言。与内核 `core/src/i18n::Lang` 的**线上三个词**对齐（`system/zh/en`），
/// 但壳这边只保留**已落定**的两档：`system` 早在握手时就被内核解析掉了。
public enum Lang: String, CaseIterable, Equatable, Sendable {
    /// 中文（默认：没握手成功、没拿到语言时按中文画，与 0.2.x 的界面一致）。
    case zh
    /// 英文。
    case en

    /// 线上那个词（`zh` / `en`）——**永远不是** `system`（同内核的
    /// `resolved_is_never_system`）。
    public var wire: String { rawValue }

    /// 从握手回执里读。缺字段 / 不认识的一律当默认中文 ——
    /// **不许**因为一个语言字段把整个握手判失败（客户端启动不了比界面语言错更糟）。
    ///
    /// 与 `windows/shell-core/src/i18n.rs::Lang::from_hello` 逐条同款：
    /// `en` / `en-US` / `en_US` ⇒ 英文，其余（含 `nil`、`system`、`fr`）⇒ 中文。
    public static func fromHello(_ raw: String?) -> Lang {
        switch raw?.trimmingCharacters(in: .whitespacesAndNewlines) {
        case .some("en"), .some("en-US"), .some("en_US"):
            return .en
        default:
            return .zh
        }
    }

    /// 取一句壳侧文案。`system` 不会出现在这里（见 [`Lang`]）。
    ///
    /// 两张 `switch` 保证"漏写一条 = 编译错误"——与 Windows 的那两张 `match` 表同一条纪律。
    public func t(_ key: Key) -> String {
        switch self {
        case .zh: return I18n.zh(key)
        case .en: return I18n.en(key)
        }
    }

    /// 带占位符的版本：按名替换 `{name}`（与内核 `i18n::tf` 同一套写法）。
    ///
    /// 未知占位符**原样留着**（与 Windows 的 `tf` 同款）：漏填是可测的，
    /// 静默删掉则会让一句话缺一块而无迹可查。
    public func tf(_ key: Key, _ args: [String: String] = [:]) -> String {
        var out = t(key)
        for (name, value) in args {
            out = out.replacingOccurrences(of: "{\(name)}", with: value)
        }
        return out
    }
}

/// 壳侧文案的键（闭集：少写一条就是编译错误，见 [`Lang.t`]）。
///
/// ⚠️ 收的是**整个壳里会画上屏的字**（0.3.0 双语化收尾）。内核原文不在这里：
///    它已经按语言拼好，壳只负责照登（约束 3）。
public enum Key: CaseIterable, Equatable, Sendable {
    case engineConnecting
    case engineNotStarted
    case engineRunning
    case engineUnavailablePrefix
    case engineHandshakeTimeout
    case engineHandshakeTooltip
    case engineConnectingTooltip
    case engineNotStartedTooltip
    case engineRunningTooltip
    case languageTitle
    case languageNote
    case languageFollowSystem
    case languageChinese
    case languageEnglish
    case languageSheetTitle
    case languageSheetNote
    case languageSheetConfirm
    case retry
    case restartWaiting
    case restartVerifying
    case restartExhausted
    case commonOK
    case commonCancel
    case commonSave
    case commonSelect
    case commonChoose
    case commonDismissNotice
    case settingsUnavailableTitle
    case settingsDownloadTo
    case settingsChooseDirectoryHelp
    case settingsRestoreDefault
    case settingsRestoreDefaultHelp
    case settingsChangeDirConfirmButton
    case settingsDirProblemTitle
    case settingsChangeDirTitle
    case settingsPanelDirMessage
    case settingsExportTo
    case settingsExportUnexpectedFailure
    case diagnosticsTitle
    case diagnosticsNote
    case verboseLoggingOn
    case verboseLoggingOff
    case verboseLoggingUnchanged
    case settingsCheckUpdateOnLaunch
    case settingsCheckUpdateOnLaunchHelp
    case settingsCheckNow
    case settingsCheckNowHelp
    case updateReceiptNewer
    case updateReceiptDisabled
    case updateReceiptUpToDate
    case updateReceiptUnreachable
    case updateReceiptNoAnswer
    case settingsParallelLabel
    case settingsConnectionsLabel
    case settingsSplitsLabel
    case settingsMaxTriesLabel
    case settingsRetryWaitLabel
    case settingsMinSplitLabel
    case settingsLimitLabel
    case settingsLimitPlaceholder
    case settingsMinSplitNote
    case settingsLimitNote
    case settingsApplyNote
    case settingsSaveHelpPending
    case settingsSaveHelpUnchanged
    case settingsLicenseEntry
    case settingsLicenseHelp
    case settingsUnsaved
    case settingsNoChanges
    case downloadDirDefaultDisplay
    case downloadDirSectionNote
    case downloadDirCheckMissing
    case downloadDirCheckNotDirectory
    case downloadDirCheckNotWritable
    case downloadDirConfirmation
    case downloadDirChangedTitle
    case downloadDirRestoredDefault
    case downloadDirUnchanged
    case downloadDirChangedNotice
    case engineGateUnavailableHelp
    case coreResponseTimedOut
    case diagnosticsExportDone
    case diagnosticsExportCancelled
    case diagnosticsPrivacyNote
    case diagnosticsExportButton
    case diagnosticsExportButtonHelp
    case diagnosticsExportPanelMessage
    case verifyClassifiedCount
    case coreNotStarted
    case engineRestartFailed
    case enqueueNotSubmitted
    case commonLoad
    case commonRefresh
    case commonClose
    case selectAll
    case deselectAll
    case deliveryCode
    case fileSelectAllCurrentLevel
    case switchSwitchedNotice
    case appTitle
    case appAboutWindow
    case appMenuShowMainWindow
    case appMenuSettings
    case licenseTitle
    case licenseIntro
    case licenseReadFailed
    case emptyTitle
    case emptySubtitle
    case emptyAdvanced
    case emptyBaseURLNote
    case deliveryCodeTooLong
    case deliveryBaseURLTooLong
    case downloadAll
    case downloadSelected
    case downloadTargetsHelp
    case emptySelectionHint
    case selectionCount
    case selectionTotal
    case countItems
    case rowStatePending
    case rowStateDownloading
    case rowStateComplete
    case rowStateFailed
    case fileDownloadHere
    case fileDownloadHereHelp
    case fileReloadLevel
    case fileBackToRoot
    case fileLoading
    case fileLoadFailed
    case fileEmpty
    case fileDownloadSelected
    case fileSelectAllLevel
    case fileColumnName
    case fileColumnSize
    case fileColumnTime
    case fileColumnStatus
    case fileSelectItem
    case fileDeselectItem
    case selectAllHelp
    case clearSelectionHelp
    case sidebarFiles
    case sidebarTransfers
    case sidebarVerify
    case sidebarBatchSummary
    case sidebarNoBatch
    case deliveryFilesCount
    case deliveryValidUntil
    case deliveryExpired
    case rootSwitchCodeHelp
    case rootSettings
    case rootOpenSettingsHelp
    case rootGoDownload
    case rootGoDownloadHelp
    case switchTitle
    case switchWarning
    case switchHistoryTitle
    case switchHistoryHint
    case switchLoadThis
    case switchNotePlaceholder
    case transferPausedBadge
    case transferRowActions
    case transferPause
    case transferResume
    case transferReveal
    case transferRemove
    case transferWaiting
    case transferActive
    case transferComplete
    case transferError
    case transferRemoved
    case transferUnknownPath
    case transfersActivity
    case transferListEngineNotStarted
    case transferListEmpty
    case transfersLoading
    case transfersPauseAll
    case transfersPauseAllHelp
    case transfersResumeAll
    case transfersResumeAllHelp
    case transfersRefreshHelp
    case transfersClearFinished
    case transfersClearFinishedHelp
    case transfersLoadingList
    case downloadNoticeBatch
    case enqueueNothing
    case enqueueAdded
    case enqueueAddedPending
    case verifyOverallProgress
    case verifyRefreshHelp
    case verifyNoResults
    case verifyNoResultsHint
    case verifyClassOK
    case verifyClassBad
    case verifyClassMissing
    case verifyClassSizeMismatch
    case verifyClassUnverifiable
    case verifyClassUnreadable
    case verifyUnverifiableNote
    case verifyNotVerified
    case verifyAllGood
    case verifyFailedCount
    case verifyKernelNotAllGood
    case batchMigrateRunning
    case batchMigrateStartFailed
    case batchMigrateProgressLost
    case batchMigrateErrors
    case batchMigrateSkipped
    case aboutVersionUnknown
    case licenseMissingResource
    case licenseEmptyResource
    case licenseUnreadable
    case licenseNotUTF8
    case updateNoticeNewer
    case switchBusy
    case historyWriteFailed
    case downloadDirSaveFailed
    case downloadDirRestartBlocked
    case verboseLoggingSaveFailed
    case verboseLoggingRestartBlocked
    case deliveryCodeTooLongReason
    case deliveryBaseURLTooLongReason
    case busyLoadingDelivery
    case busyAddingTasks
    case enqueuePathTooLong
    case engineDisconnected
    case engineRestartingBusy
    case kernelRestartFailed
    case protocolMismatchNote
    case engineReasonNoHandshake
    case engineReasonNotStarted
    case engineReasonRunning
    case coreNotFoundEnv
    case coreNotFound
    case coreChannelShutdown
    case coreChannelDead
    case corePipeClosed
    case coreResponseMismatch
    case coreLineNotJSON
    case jsonNotAValue
    case fileStoreMakeDir
    case fileStoreWriteTemp
    case fileStoreRename
    case protocolMissingResult
    case protocolShapeMismatch
    case protocolUnknownNode
    case protocolUnknownEntry
    case clientProtocolShapeMismatch
    case coreStartFailed
    case pipeWriteFailed
    case pipeReadFailed
    case breadcrumbBackNotice
    case diagHeaderTitle
    case diagHeaderIntro
    case diagHeaderRisk
    case diagHeaderNotRecorded
    case diagHeaderContents
    case diagProtocolNotConnected
    case diagLogFilesHeading
    case diagNoLogFiles
    case diagFileLine
    case diagAbsentLine
    case diagNotDoingHeading
    case diagNotDoing
    case diagHowToReadHeading
    case diagHowToRead
    case diagCoverageUnknown
    case diagFolderFailed
    case diagCopyFailed
    case diagHeaderWriteFailed
    case diagMissingCore
    case diagOsUnknown
}

enum I18n {
    /// 中文一列。**逐字保留既有客户看到的句子**（一个字符都不许改）。
    static func zh(_ key: Key) -> String {
        switch key {
        case .engineConnecting: return "正在连接内核…"
        case .engineNotStarted: return "引擎未启动"
        case .engineRunning: return "运行中"
        case .engineUnavailablePrefix: return "引擎不可用：{reason}"
        case .engineHandshakeTimeout: return "内核无响应（等待超过 5 秒）"
        case .engineHandshakeTooltip: return "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
        case .engineConnectingTooltip: return "内核尚未握手（壳正在启动它）"
        case .engineNotStartedTooltip: return "下载引擎尚未启动 —— 添加下载任务后内核会启动它"
        case .engineRunningTooltip: return "下载引擎正在运行"
        case .languageTitle: return "界面语言"
        case .languageNote: return "选中后立刻生效，不用重启客户端。日志仍按开发语言记录，便于排查。"
        case .languageFollowSystem: return "跟随系统"
        case .languageChinese: return "中文"
        case .languageEnglish: return "English"
        case .languageSheetTitle: return "选择界面语言"
        case .languageSheetNote: return "默认跟随系统语言，之后可以随时在「设置 → 界面语言」里改。"
        case .languageSheetConfirm: return "就这样用"
        case .retry: return "重试"
        case .restartWaiting: return "正在重启下载引擎（第 {attempt}/{budget} 次）…"
        case .restartVerifying: return "引擎已重启，正在确认它能正常工作…"
        case .restartExhausted: return "自动重启没能恢复下载引擎，请手动点「重试」"
        case .commonOK: return "知道了"
        case .commonCancel: return "取消"
        case .commonSave: return "保存"
        case .commonSelect: return "选择"
        case .commonChoose: return "选择…"
        case .commonDismissNotice: return "收起这条提示"
        case .settingsUnavailableTitle: return "内核还没有交出参数面板"
        case .settingsDownloadTo: return "下载到"
        case .settingsChooseDirectoryHelp: return "挑一个文件夹，交付文件会下到这里（改完会重启内核）"
        case .settingsRestoreDefault: return "恢复默认"
        case .settingsRestoreDefaultHelp: return "不再指定下载目录，由内核用它自己的默认值（改完会重启内核）"
        case .settingsChangeDirConfirmButton: return "更改并重启内核"
        case .settingsDirProblemTitle: return "这个文件夹不能用"
        case .settingsChangeDirTitle: return "更改下载目录？"
        case .settingsPanelDirMessage: return "交付文件会下载到这个文件夹"
        case .settingsExportTo: return "导出到"
        case .settingsExportUnexpectedFailure: return "诊断日志没有导出：{error}"
        case .diagnosticsTitle: return "详细日志"
        case .diagnosticsNote: return "打开后日志会详细很多（用于排查问题），排查完请关掉。改动会立刻重启内核（正在跑的任务会停）。"
        case .verboseLoggingOn: return "详细日志已打开（内核已按详细档重启）"
        case .verboseLoggingOff: return "详细日志已关闭（内核已按普通档重启）"
        case .verboseLoggingUnchanged: return "详细日志没有变化，没有重启内核"
        case .settingsCheckUpdateOnLaunch: return "启动时检查更新"
        case .settingsCheckUpdateOnLaunchHelp: return "默认打开。每次启动问一次官方仓库有没有新版本（只提示，不下载、不安装）"
        case .settingsCheckNow: return "立即检测"
        case .settingsCheckNowHelp: return "现在就去问一次官方仓库（不受 24 小时节流限制）"
        case .updateReceiptNewer: return "有新版本 {latest}（当前 {current}）。"
        case .updateReceiptDisabled: return "自动检查更新是关着的，刚才没有去查。补救：打开上面那个开关，再点一次「立即检测」。"
        case .updateReceiptUpToDate: return "已经是最新的（当前 {current}）。"
        case .updateReceiptUnreachable: return "没查成：这次没连上发布页。补救：确认这台机器能上网，或者过一会儿再点一次。"
        case .updateReceiptNoAnswer: return "没查到结果：内核回的这份检查状态不全。补救：重开一次客户端再试。"
        case .settingsParallelLabel: return "并行文件数（-j）"
        case .settingsConnectionsLabel: return "单文件连接数（-x）"
        case .settingsSplitsLabel: return "分片数（-s）"
        case .settingsMaxTriesLabel: return "重试次数"
        case .settingsRetryWaitLabel: return "重试间隔（秒）"
        case .settingsMinSplitLabel: return "最小分片大小（-k）"
        case .settingsLimitLabel: return "限速（MB/s）"
        case .settingsLimitPlaceholder: return "限速"
        case .settingsMinSplitNote: return "当前值「{value}」不在内核给出的取值集合里。保存会按这个原值下发，由内核校验。"
        case .settingsLimitNote: return "0 = 不限速（不限制总下载速度）"
        case .settingsApplyNote: return "改动会写盘保存（重开应用仍是这一份）。并行数与限速对已启动的引擎即时生效；其余各项在添加新任务时生效，已在传输的任务不受影响。"
        case .settingsSaveHelpPending: return "把七项参数下发给内核（set_settings）"
        case .settingsSaveHelpUnchanged: return "与内核当前参数一致：没有要保存的改动"
        case .settingsLicenseEntry: return "开源许可…"
        case .settingsLicenseHelp: return "查看内嵌组件（aria2）的 GPLv2 全文"
        case .settingsUnsaved: return "有未保存的改动"
        case .settingsNoChanges: return "与内核当前参数一致"
        case .downloadDirDefaultDisplay: return "默认（{path}，由内核决定）"
        case .downloadDirSectionNote: return "没有设置过时，壳**不指定**下载目录，由内核用它自己的默认值。改变目录会立刻重启内核（正在跑的任务会停）。请尽量选**本机磁盘**上的目录：网络盘（NFS / SMB）会让界面明显变慢。"
        case .downloadDirCheckMissing: return "这个位置不存在：{path}"
        case .downloadDirCheckNotDirectory: return "这是一个文件，不是文件夹：{path}"
        case .downloadDirCheckNotWritable: return "这个文件夹不可以写入（没有权限）：{path}"
        case .downloadDirConfirmation: return """
            下载目录将从「{current}」改为「{target}」。

            改完会立刻重启内核，三条后果：
            1. 正在跑的任务会停（内核重启），没传完的文件要重新点一次下载；
            2. 新目录里没有记录的文件会重新校验（内核的状态文件就在下载目录里）；
            3. 旧目录里的文件不会被删、也不会被搬走。
            """
        case .downloadDirChangedTitle: return "下载目录已改（内核已按新目录重启）"
        case .downloadDirRestoredDefault: return "已恢复默认下载目录（内核已按默认目录重启）"
        case .downloadDirUnchanged: return "下载目录没有变化，没有重启内核"
        case .downloadDirChangedNotice: return "下载目录已改为 {path}（内核已按新目录重启）"
        case .engineGateUnavailableHelp: return "引擎不可用，请先点顶部的「重试」"
        case .coreResponseTimedOut: return "内核响应超时"
        case .diagnosticsExportDone: return "诊断日志已导出"
        case .diagnosticsExportCancelled: return "没有导出（你取消了选择文件夹）"
        case .diagnosticsPrivacyNote: return "这份日志可能包含下载地址片段，发送前你可以自己打开看一眼。"
        case .diagnosticsExportButton: return "导出诊断日志…"
        case .diagnosticsExportButtonHelp: return "把日志与一份说明文件拷进你选的文件夹（里面会新建一个带时间戳的子目录，导多少次都不会互相覆盖）"
        case .diagnosticsExportPanelMessage: return "诊断日志会导出到这个文件夹里"
        case .verifyClassifiedCount: return "已校验 {count} 项"
        case .coreNotStarted: return "内核没有起来（{reason}）"
        case .engineRestartFailed: return "内核重启失败：{reason}"
        case .enqueueNotSubmitted: return "未提交：{reason}"
        case .commonLoad: return "加载"
        case .commonRefresh: return "刷新"
        case .commonClose: return "关闭"
        case .selectAll: return "全选"
        case .deselectAll: return "全不选"
        case .deliveryCode: return "交付码"
        case .fileSelectAllCurrentLevel: return "全选当前层"
        case .switchSwitchedNotice: return "已换到批次 {code}"
        case .appTitle: return "Benagen 数据下载工具"
        case .appAboutWindow: return "关于 Benagen 数据下载工具"
        case .appMenuShowMainWindow: return "显示主窗口"
        case .appMenuSettings: return "设置…"
        case .licenseTitle: return "开源许可"
        case .licenseIntro: return "本应用内嵌 aria2（GPLv2 许可），因此随附下列许可全文。"
        case .licenseReadFailed: return "GPLv2 全文没能读出来"
        case .emptyTitle: return "输入交付码开始下载"
        case .emptySubtitle: return "交付码在交付邮件或交付页链接里（整条链接也可以直接粘进来）"
        case .emptyAdvanced: return "高级：自定义下载地址"
        case .emptyBaseURLNote: return "留空即用默认交付服务器"
        case .deliveryCodeTooLong: return "交付码过长（超过 {limit}），请只粘贴交付码本身或交付页链接。"
        case .deliveryBaseURLTooLong: return "自定义下载地址过长（超过 {limit}），请确认粘贴的内容确实是交付页的地址。"
        case .downloadAll: return "全部下载"
        case .downloadSelected: return "下载选中"
        case .downloadTargetsHelp: return "把选中的文件加入下载队列（未选中时 = 全部待下载文件）"
        case .emptySelectionHint: return "未勾选任何项，将下载全部待下载文件"
        case .selectionCount: return "已选 {count} 项"
        case .selectionTotal: return "合计 {size}"
        case .countItems: return "{count} 项"
        case .rowStatePending: return "待下载"
        case .rowStateDownloading: return "下载中"
        case .rowStateComplete: return "已完成"
        case .rowStateFailed: return "失败"
        case .fileDownloadHere: return "下载本目录"
        case .fileDownloadHereHelp: return "把当前这个目录（连同子目录）加进下载"
        case .fileReloadLevel: return "重新读取这一层"
        case .fileBackToRoot: return "回到批次根目录"
        case .fileLoading: return "正在读取目录…"
        case .fileLoadFailed: return "这一层没能读出来"
        case .fileEmpty: return "这个目录是空的"
        case .fileDownloadSelected: return "下载选中项"
        case .fileSelectAllLevel: return "全选本层"
        case .fileColumnName: return "名称"
        case .fileColumnSize: return "大小"
        case .fileColumnTime: return "时间"
        case .fileColumnStatus: return "状态"
        case .fileSelectItem: return "勾选这一项"
        case .fileDeselectItem: return "取消勾选这一项"
        case .selectAllHelp: return "勾上整批的全部文件（不含目录）"
        case .clearSelectionHelp: return "清空勾选面"
        case .sidebarFiles: return "文件"
        case .sidebarTransfers: return "传输列表"
        case .sidebarVerify: return "校验结果"
        case .sidebarBatchSummary: return "批次摘要"
        case .sidebarNoBatch: return "尚未加载交付批次"
        case .deliveryFilesCount: return "{count} 个文件"
        case .deliveryValidUntil: return "有效期至 {time}"
        case .deliveryExpired: return "已过期"
        case .rootSwitchCodeHelp: return "换一个交付码（当前：{code}）"
        case .rootSettings: return "设置"
        case .rootOpenSettingsHelp: return "打开设置（⌘,）"
        case .rootGoDownload: return "去下载"
        case .rootGoDownloadHelp: return "在系统默认浏览器里打开官方下载页"
        case .switchTitle: return "加载另一个交付码"
        case .switchWarning: return "换码会把当前的下载任务从引擎里摘掉（含等待中的）；已经下载到磁盘的文件不会被删除。"
        case .switchHistoryTitle: return "历史批次"
        case .switchHistoryHint: return "点一行即可切换到那一批；备注写在这里，重启应用后仍在。"
        case .switchLoadThis: return "加载这一批"
        case .switchNotePlaceholder: return "备注"
        case .transferPausedBadge: return "已暂停"
        case .transferRowActions: return "这一行的操作"
        case .transferPause: return "暂停"
        case .transferResume: return "继续"
        case .transferReveal: return "在访达中显示"
        case .transferRemove: return "移除"
        case .transferWaiting: return "等待中"
        case .transferActive: return "下载中"
        case .transferComplete: return "已完成"
        case .transferError: return "失败"
        case .transferRemoved: return "已移除"
        case .transferUnknownPath: return "（未知路径）"
        case .transfersActivity: return "活动 {active} · 等待 {waiting} · 已停止 {stopped}"
        case .transferListEngineNotStarted: return "下载引擎尚未启动（还没有添加过任务）"
        case .transferListEmpty: return "没有正在传输的任务"
        case .transfersLoading: return "读取中…"
        case .transfersPauseAll: return "暂停全部"
        case .transfersPauseAllHelp: return "立刻暂停全部任务（已下载的部分保留，可随时一键开始）"
        case .transfersResumeAll: return "一键开始"
        case .transfersResumeAllHelp: return "继续全部已暂停的任务，并把还没下完的补回队列"
        case .transfersRefreshHelp: return "立刻取一次传输列表快照"
        case .transfersClearFinished: return "清空已完成"
        case .transfersClearFinishedHelp: return "移除全部已结束的任务"
        case .transfersLoadingList: return "正在读取传输列表…"
        case .downloadNoticeBatch: return "批次 {code}：{summary}"
        case .enqueueNothing: return "没有需要下载的文件"
        case .enqueueAdded: return "已加入 {added} 个下载任务"
        case .enqueueAddedPending: return "已加入 {added} 个下载任务，另有 {pending} 个正在后台继续加入"
        case .verifyOverallProgress: return "总进度"
        case .verifyRefreshHelp: return "重新取一次内核当前的校验结果与总进度"
        case .verifyNoResults: return "还没有校验结果"
        case .verifyNoResultsHint: return "点右上角「刷新」取一次"
        case .verifyClassOK: return "校验通过"
        case .verifyClassBad: return "内容不符"
        case .verifyClassMissing: return "文件缺失"
        case .verifyClassSizeMismatch: return "大小不符"
        case .verifyClassUnverifiable: return "无法校验"
        case .verifyClassUnreadable: return "无法读取"
        case .verifyUnverifiableNote: return "这些文件没有可比的校验值，不是失败"
        case .verifyNotVerified: return "尚未校验"
        case .verifyAllGood: return "全部通过"
        case .verifyFailedCount: return "{count} 项未通过"
        case .verifyKernelNotAllGood: return "内核判定未全部通过（六类里没有失败项）"
        case .batchMigrateRunning: return "正在搬运已下载的文件：{done}/{total} 个（{doneBytes} / {totalBytes}）"
        case .batchMigrateStartFailed: return "旧目录里已下载的文件没能搬到新目录，会从头下载（旧文件仍在原目录）。原因：{reason}"
        case .batchMigrateProgressLost: return "没能取到搬运进度，旧文件可能没有搬完。"
        case .batchMigrateErrors: return "有 {count} 个文件没能搬到新目录，会从头下载（旧文件仍在原目录）。第一个：{path}：{reason}"
        case .batchMigrateSkipped: return "有 {count} 个文件在新目录里已经存在，没有覆盖。"
        case .aboutVersionUnknown: return "版本未知（应用包缺少 CFBundleShortVersionString / CFBundleVersion）"
        case .licenseMissingResource: return "应用包内找不到 GPLv2 全文（Contents/Resources/COPYING-GPLv2.txt）。这是打包缺陷，不是您操作有误：请重新安装客户端；若手上这份是别人拷来的，请向分发方索取一份许可副本。"
        case .licenseEmptyResource: return "应用包内的 GPLv2 全文是空的（Contents/Resources/COPYING-GPLv2.txt）。这是打包缺陷，不是您操作有误：请重新安装客户端，并向分发方索取许可副本。"
        case .licenseUnreadable: return "读取 GPLv2 全文失败：{reason}"
        case .licenseNotUTF8: return "文件不是 UTF-8 文本（{name}）"
        case .updateNoticeNewer: return "有新版本 {latest}（当前 {current}）"
        case .switchBusy: return "上一次换码还在进行中，请等它结束。"
        case .historyWriteFailed: return "历史记录写入失败：{reason}"
        case .downloadDirSaveFailed: return "下载目录没有改成（偏好写入失败：{reason}）"
        case .downloadDirRestartBlocked: return "内核还在重启中，这一次没有改成。下载目录已经记下来了，下次启动会按它生效。"
        case .verboseLoggingSaveFailed: return "详细日志没有改成（偏好写入失败：{reason}）"
        case .verboseLoggingRestartBlocked: return "内核还在重启中，这一次没有改成。详细日志开关已经记下来了，下次启动会按它生效。"
        case .deliveryCodeTooLongReason: return "交付码长 {bytes} 字节，已超过安全上限（{limit}）。超限的请求内核不会回话、也不会报错，只会一直等下去——请只粘贴交付码本身或交付页链接。"
        case .deliveryBaseURLTooLongReason: return "自定义下载地址长 {bytes} 字节，已超过安全上限（{limit}）。超限的请求内核不会回话、也不会报错，只会一直等下去——请确认粘贴的内容确实是交付页的地址。"
        case .busyLoadingDelivery: return "正在加载交付清单…"
        case .busyAddingTasks: return "正在添加下载任务…"
        case .enqueuePathTooLong: return "这一项路径太长，超过单次请求上限，没有发送"
        case .engineDisconnected: return "下载引擎已断开：{reason}"
        case .engineRestartingBusy: return "下载引擎已断开，正在重启…"
        case .kernelRestartFailed: return "内核重启失败：{reason}"
        case .protocolMismatchNote: return "客户端与内核版本不匹配，请更新客户端或内核后重试（内核：{kernel}）"
        case .engineReasonNoHandshake: return "内核还没有握手"
        case .engineReasonNotStarted: return "下载引擎尚未启动"
        case .engineReasonRunning: return "内核正在运行"
        case .coreNotFoundEnv: return "（环境变量 BENAGEN_CORE 未设置）"
        case .coreNotFound: return """
            找不到内核可执行文件 benagen-core（找过：
            {paths}）
            """
        case .coreChannelShutdown: return "内核连接已关闭（shutdown 已调用）"
        case .coreChannelDead: return "内核连接已判死（读超时后未能重新对齐）"
        case .corePipeClosed: return "内核进程已退出（管道结束）"
        case .coreResponseMismatch: return "内核回的响应 id 是 {got}，而当前在等 {want}（请求-响应失步）"
        case .coreLineNotJSON: return "内核发来的一行不是合法 JSON：{line}"
        case .jsonNotAValue: return "不是合法的 JSON 值（null/bool/number/string/array/object 都不是）"
        case .fileStoreMakeDir: return "建目录失败：{path}（{reason}）"
        case .fileStoreWriteTemp: return "写临时文件失败：{path}（{reason}）"
        case .fileStoreRename: return "替换 {path} 失败：rename(2) errno={code}"
        case .protocolMissingResult: return "ok:true 的响应里没有 result（id {id}）"
        case .protocolShapeMismatch: return "id {id} 的 result 与 {type} 的形状不符: {reason}"
        case .protocolUnknownNode: return "未知的节点类型 {type}"
        case .protocolUnknownEntry: return "未知的条目类型 {type}"
        case .clientProtocolShapeMismatch: return "result 与 {type} 的形状不符: {reason}"
        case .coreStartFailed: return "内核进程启动失败（{path}）：{reason}"
        case .pipeWriteFailed: return "向内核写入请求失败（管道已断）：{reason}"
        case .pipeReadFailed: return "读取内核响应失败（管道已断）：{reason}"
        case .breadcrumbBackNotice: return "已返回上一层"
        case .diagHeaderTitle: return "诊断日志导出说明"
        case .diagHeaderIntro: return """
            这份文件是这次导出的清单：它说的不是「日志里有什么内容」，
            而是「这个文件夹里有哪些文件、每一份覆盖了哪一段时间」。
            """
        case .diagHeaderRisk: return """
            ⚠️ 风险先说：这份日志会离开你的机器，里面可能有下载地址片段
            （下载引擎的原始报错会原样带上它）。我们没有做自动过滤 —— 要可靠地
            抹掉它，得先能识别它，而我们识别不了任意第三方文案；做一个不可靠的
            过滤器比不做更糟（它会让人以为已经安全了）。
            发送之前，你可以自己打开看一眼。
            """
        case .diagHeaderNotRecorded: return """
            本来就不记的：交付码本身、文件路径本身、rpc-secret、客户的目录名。
            （其中交付码与下载目录在写进日志之前会被按字面抹成 [已隐去]；但没配置
            下载目录时，内核会用它自己的默认路径——那一条壳不知道，抹不掉。抹不
            掉的还有第三方文案里以别的写法出现的那一份——见上。）
            """
        case .diagHeaderContents: return "这里面有什么"
        case .diagProtocolNotConnected: return "未连上"
        case .diagLogFilesHeading: return "日志文件（每份：名字 / 字节数 / sha256 / 覆盖时间范围）"
        case .diagNoLogFiles: return "没有任何日志文件（下面每一行都写着「尚未产生」）。"
        case .diagFileLine: return "{name}  {bytes} 字节  sha256={sha}  覆盖范围: {coverage}"
        case .diagAbsentLine: return "{name}  尚未产生（这一份日志还没有生成过）"
        case .diagNotDoingHeading: return "没有什么（都是显式的选择，不是漏做）"
        case .diagNotDoing: return """
            · 没有压缩包：导出就是一个文件夹，本文件是它的清单；
            · 没有加锁、也没有做快照：导出时日志可能仍在被写 ⇒
              某一份的最后一行可能是写到一半的（代价远大于收益，规格 §4）；
            · 没有敏感信息自动过滤（理由见第一屏那段）。注意「把交付码与下载目录按字面
              抹成 [已隐去]」不是过滤：抹的是壳自己知道的那两个串本身，不是去识别
              任意文案里像不像它。
            """
        case .diagHowToReadHeading: return "怎么读"
        case .diagHowToRead: return """
            每份日志一行一条记录，形状是：ts=<unix 秒> event=<事件名> 键=值 …
            那个时间是 UTC 的 unix 秒（macOS `date -r <ts>`、Linux `date -d @<ts>`）。
            「覆盖范围」那一格取的是这份日志的头尾两行：一眼就能看出它有没有
            覆盖到出问题的那一段时间。没覆盖到的那一刻只有等下一次现场 ——
            所以请在复现之前把详细日志打开，导出之后可以再关掉。
            """
        case .diagCoverageUnknown: return "未知"
        case .diagFolderFailed: return "诊断日志没有导出：建不了文件夹 {folder}（系统原话：{cause}）。补救：换一个能写的文件夹再试一次（比如桌面），或检查那个位置是不是一个文件、磁盘是不是满了。"
        case .diagCopyFailed: return "诊断日志没有导出：拷不进 {file}（系统原话：{cause}）。补救：确认那一份文件还在、且没有被别的程序独占（比如你正用文本编辑打开着它），然后重试一次。"
        case .diagHeaderWriteFailed: return "诊断日志没有导出：说明文件写不进去（{file}；系统原话：{cause}）。补救：检查目标磁盘是不是满了、那个文件夹是不是只读，然后重试一次。"
        case .diagMissingCore: return "（取不到：内核可执行文件不在预期位置）"
        case .diagOsUnknown: return "macOS（版本取不到）"
        }
    }

    /// 英文一列。**不许出现汉字**（`I18nTests.theEnglishTableHasNoCJK` 钉着），
    /// 只有「中文 / English」这两个语言名本身例外。
    static func en(_ key: Key) -> String {
        switch key {
        case .engineConnecting: return "Connecting to the core…"
        case .engineNotStarted: return "Engine not started"
        case .engineRunning: return "Running"
        case .engineUnavailablePrefix: return "Engine unavailable: {reason}"
        case .engineHandshakeTimeout: return "The core is not responding (waited more than 5 seconds)"
        case .engineHandshakeTooltip: return "The core may be stuck on a system permission dialog (for example \"wants to access your Downloads folder\"). Check the screen for that dialog, click Allow, then click Retry."
        case .engineConnectingTooltip: return "The core has not completed the handshake yet (the shell is starting it)"
        case .engineNotStartedTooltip: return "The download engine is not started yet - the core starts it once you add downloads"
        case .engineRunningTooltip: return "The download engine is running"
        case .languageTitle: return "Interface language"
        case .languageNote: return "Takes effect immediately - no restart. Logs stay in the development language so they remain searchable."
        case .languageFollowSystem: return "Follow system"
        case .languageChinese: return "中文"
        case .languageEnglish: return "English"
        case .languageSheetTitle: return "Choose your language"
        case .languageSheetNote: return "By default it follows your system language. You can change it any time in Settings → Interface language."
        case .languageSheetConfirm: return "Use this"
        case .retry: return "Retry"
        case .restartWaiting: return "Restarting the download engine (attempt {attempt}/{budget})…"
        case .restartVerifying: return "The engine restarted - checking that it works…"
        case .restartExhausted: return "Automatic restarts could not recover the download engine - please click Retry"
        case .commonOK: return "OK"
        case .commonCancel: return "Cancel"
        case .commonSave: return "Save"
        case .commonSelect: return "Select"
        case .commonChoose: return "Choose…"
        case .commonDismissNotice: return "Dismiss this notice"
        case .settingsUnavailableTitle: return "The core has not provided its settings yet"
        case .settingsDownloadTo: return "Download to"
        case .settingsChooseDirectoryHelp: return "Pick a folder; delivery files will download here (the core restarts after the change)"
        case .settingsRestoreDefault: return "Restore default"
        case .settingsRestoreDefaultHelp: return "Stop specifying a download folder; the core uses its own default (the core restarts after the change)"
        case .settingsChangeDirConfirmButton: return "Change and restart the core"
        case .settingsDirProblemTitle: return "That folder cannot be used"
        case .settingsChangeDirTitle: return "Change the download folder?"
        case .settingsPanelDirMessage: return "Delivery files will be downloaded into this folder"
        case .settingsExportTo: return "Export to"
        case .settingsExportUnexpectedFailure: return "The diagnostic logs were not exported: {error}"
        case .diagnosticsTitle: return "Verbose logging"
        case .diagnosticsNote: return "When on, logs are much more detailed (for troubleshooting). Turn it off when done. Changing this restarts the core immediately (running tasks stop)."
        case .verboseLoggingOn: return "Verbose logging is on (the core restarted in verbose mode)"
        case .verboseLoggingOff: return "Verbose logging is off (the core restarted in normal mode)"
        case .verboseLoggingUnchanged: return "Verbose logging did not change; the core was not restarted"
        case .settingsCheckUpdateOnLaunch: return "Check for updates at launch"
        case .settingsCheckUpdateOnLaunchHelp: return "On by default. At each launch, ask the official repository whether a new version exists (notification only - nothing is downloaded or installed)"
        case .settingsCheckNow: return "Check now"
        case .settingsCheckNowHelp: return "Ask the official repository right now (not subject to the 24-hour throttle)"
        case .updateReceiptNewer: return "A new version is available: {latest} (current: {current})."
        case .updateReceiptDisabled: return "Automatic update checks are off, so nothing was checked. Fix: turn on the switch above, then click Check now again."
        case .updateReceiptUpToDate: return "Already up to date (current: {current})."
        case .updateReceiptUnreachable: return "Could not check: the release page was unreachable. Fix: make sure this machine can access the internet, or try again in a while."
        case .updateReceiptNoAnswer: return "No result: the core's check status was incomplete. Fix: restart the app and try again."
        case .settingsParallelLabel: return "Parallel files (-j)"
        case .settingsConnectionsLabel: return "Connections per file (-x)"
        case .settingsSplitsLabel: return "Splits (-s)"
        case .settingsMaxTriesLabel: return "Max retries"
        case .settingsRetryWaitLabel: return "Retry interval (seconds)"
        case .settingsMinSplitLabel: return "Minimum split size (-k)"
        case .settingsLimitLabel: return "Speed limit (MB/s)"
        case .settingsLimitPlaceholder: return "Limit"
        case .settingsMinSplitNote: return "The current value “{value}” is not in the set provided by the core. It will be sent as-is and validated by the core."
        case .settingsLimitNote: return "0 = no limit (no cap on total download speed)"
        case .settingsApplyNote: return "Changes are saved to disk (they persist after restarting the app). Parallel files and the speed limit apply immediately to a running engine; the other settings apply when new tasks are added, and tasks already transferring are not affected."
        case .settingsSaveHelpPending: return "Send the seven settings to the core (set_settings)"
        case .settingsSaveHelpUnchanged: return "Matches the core's current settings: nothing to save"
        case .settingsLicenseEntry: return "Open-source licenses…"
        case .settingsLicenseHelp: return "View the full GPLv2 text for the embedded component (aria2)"
        case .settingsUnsaved: return "Unsaved changes"
        case .settingsNoChanges: return "Matches the core's settings"
        case .downloadDirDefaultDisplay: return "Default ({path}, decided by the core)"
        case .downloadDirSectionNote: return "When nothing is set, the shell **does not specify** a download folder; the core uses its own default. Changing the folder restarts the core immediately (running tasks stop). Prefer a folder on a **local disk**: network shares (NFS / SMB) make the UI noticeably slower."
        case .downloadDirCheckMissing: return "This location does not exist: {path}"
        case .downloadDirCheckNotDirectory: return "This is a file, not a folder: {path}"
        case .downloadDirCheckNotWritable: return "This folder is not writable (no permission): {path}"
        case .downloadDirConfirmation: return """
            The download folder will change from “{current}” to “{target}”.

            The core restarts immediately afterwards. Three consequences:
            1. Running tasks stop (the core restarts), and unfinished files need another click on Download;
            2. Files not recorded in the new folder will be verified again (the core's state file lives in the download folder);
            3. Files in the old folder are not deleted and are not moved.
            """
        case .downloadDirChangedTitle: return "Download folder changed (the core restarted with the new folder)"
        case .downloadDirRestoredDefault: return "Restored the default download folder (the core restarted with the default folder)"
        case .downloadDirUnchanged: return "The download folder did not change; the core was not restarted"
        case .downloadDirChangedNotice: return "The download folder is now {path} (the core restarted with the new folder)"
        case .engineGateUnavailableHelp: return "The engine is unavailable - click Retry at the top first"
        case .coreResponseTimedOut: return "The core timed out"
        case .diagnosticsExportDone: return "Diagnostic logs exported"
        case .diagnosticsExportCancelled: return "Nothing was exported (you cancelled the folder picker)"
        case .diagnosticsPrivacyNote: return "These logs may contain fragments of download URLs; you can open them and take a look before sending."
        case .diagnosticsExportButton: return "Export diagnostic logs…"
        case .diagnosticsExportButtonHelp: return "Copy the logs and a README into the folder you pick (a timestamped subfolder is created inside, so repeated exports never overwrite each other)"
        case .diagnosticsExportPanelMessage: return "The diagnostic logs will be exported into this folder"
        case .verifyClassifiedCount: return "{count} verified"
        case .coreNotStarted: return "The core did not start ({reason})"
        case .engineRestartFailed: return "The core failed to restart: {reason}"
        case .enqueueNotSubmitted: return "Not submitted: {reason}"
        case .commonLoad: return "Load"
        case .commonRefresh: return "Refresh"
        case .commonClose: return "Close"
        case .selectAll: return "Select all"
        case .deselectAll: return "Deselect all"
        case .deliveryCode: return "Delivery code"
        case .fileSelectAllCurrentLevel: return "Select all in this current folder"
        case .switchSwitchedNotice: return "Switched to batch {code}"
        case .appTitle: return "Benagen Data Download Tool"
        case .appAboutWindow: return "About Benagen Data Download Tool"
        case .appMenuShowMainWindow: return "Show Main Window"
        case .appMenuSettings: return "Settings…"
        case .licenseTitle: return "Open Source Licenses"
        case .licenseIntro: return "This app bundles aria2 (licensed under GPLv2), so the full license text is included below."
        case .licenseReadFailed: return "Could not read the full GPLv2 text"
        case .emptyTitle: return "Enter a delivery code to start downloading"
        case .emptySubtitle: return "You can find the delivery code in the delivery email or in the delivery page link (you can paste the whole link too)."
        case .emptyAdvanced: return "Advanced: custom download URL"
        case .emptyBaseURLNote: return "Leave blank to use the default delivery server"
        case .deliveryCodeTooLong: return "The delivery code is too long (over {limit}). Paste only the code itself or the delivery page link."
        case .deliveryBaseURLTooLong: return "The custom download URL is too long (over {limit}). Check that you pasted the actual delivery page address."
        case .downloadAll: return "Download All"
        case .downloadSelected: return "Download Selected"
        case .downloadTargetsHelp: return "Add the selected files to the download queue (with nothing selected, all pending files are added)."
        case .emptySelectionHint: return "Nothing is selected, so all pending files will be downloaded."
        case .selectionCount: return "{count} selected"
        case .selectionTotal: return "Total {size}"
        case .countItems: return "{count} items"
        case .rowStatePending: return "Pending"
        case .rowStateDownloading: return "Downloading"
        case .rowStateComplete: return "Completed"
        case .rowStateFailed: return "Failed"
        case .fileDownloadHere: return "Download this folder"
        case .fileDownloadHereHelp: return "Add this folder (and its subfolders) to the download"
        case .fileReloadLevel: return "Reload this level"
        case .fileBackToRoot: return "Back to the batch root"
        case .fileLoading: return "Loading folder…"
        case .fileLoadFailed: return "Could not load this level"
        case .fileEmpty: return "This folder is empty"
        case .fileDownloadSelected: return "Download selected items"
        case .fileSelectAllLevel: return "Select all in this level"
        case .fileColumnName: return "Name"
        case .fileColumnSize: return "Size"
        case .fileColumnTime: return "Time"
        case .fileColumnStatus: return "Status"
        case .fileSelectItem: return "Select this item"
        case .fileDeselectItem: return "Deselect this item"
        case .selectAllHelp: return "Select every file in the batch (folders excluded)"
        case .clearSelectionHelp: return "Clear the selection"
        case .sidebarFiles: return "Files"
        case .sidebarTransfers: return "Transfers"
        case .sidebarVerify: return "Verification"
        case .sidebarBatchSummary: return "Batch summary"
        case .sidebarNoBatch: return "No delivery batch loaded yet"
        case .deliveryFilesCount: return "{count} files"
        case .deliveryValidUntil: return "Valid until {time}"
        case .deliveryExpired: return "Expired"
        case .rootSwitchCodeHelp: return "Switch to another delivery code (current: {code})"
        case .rootSettings: return "Settings"
        case .rootOpenSettingsHelp: return "Open Settings (⌘,)"
        case .rootGoDownload: return "Go to Download"
        case .rootGoDownloadHelp: return "Open the official download page in your default browser"
        case .switchTitle: return "Load another delivery code"
        case .switchWarning: return "Switching removes the current download tasks from the engine (including queued ones); files already downloaded to disk are not deleted."
        case .switchHistoryTitle: return "Past batches"
        case .switchHistoryHint: return "Click a row to switch to that batch; notes are saved here and persist across restarts."
        case .switchLoadThis: return "Load this batch"
        case .switchNotePlaceholder: return "Note"
        case .transferPausedBadge: return "Paused"
        case .transferRowActions: return "Actions for this row"
        case .transferPause: return "Pause"
        case .transferResume: return "Resume"
        case .transferReveal: return "Show in Finder"
        case .transferRemove: return "Remove"
        case .transferWaiting: return "Waiting"
        case .transferActive: return "Downloading"
        case .transferComplete: return "Completed"
        case .transferError: return "Failed"
        case .transferRemoved: return "Removed"
        case .transferUnknownPath: return "(unknown path)"
        case .transfersActivity: return "Active {active} · Waiting {waiting} · Stopped {stopped}"
        case .transferListEngineNotStarted: return "The download engine is not running yet (no tasks have been added)."
        case .transferListEmpty: return "No transfers in progress"
        case .transfersLoading: return "Loading…"
        case .transfersPauseAll: return "Pause All"
        case .transfersPauseAllHelp: return "Pause all tasks now (downloaded data is kept; you can start them again at any time)"
        case .transfersResumeAll: return "Start All"
        case .transfersResumeAllHelp: return "Resume all paused tasks and put unfinished ones back in the queue"
        case .transfersRefreshHelp: return "Fetch a fresh snapshot of the transfer list now"
        case .transfersClearFinished: return "Clear Completed"
        case .transfersClearFinishedHelp: return "Remove every finished task"
        case .transfersLoadingList: return "Loading the transfer list…"
        case .downloadNoticeBatch: return "Batch {code}: {summary}"
        case .enqueueNothing: return "There are no files to download"
        case .enqueueAdded: return "Added {added} download tasks"
        case .enqueueAddedPending: return "Added {added} download tasks; {pending} more are still being added in the background"
        case .verifyOverallProgress: return "Overall progress"
        case .verifyRefreshHelp: return "Fetch the core's current verification results and overall progress again"
        case .verifyNoResults: return "No verification results yet"
        case .verifyNoResultsHint: return "Click Refresh in the top-right corner to fetch them"
        case .verifyClassOK: return "Verified"
        case .verifyClassBad: return "Content mismatch"
        case .verifyClassMissing: return "Missing files"
        case .verifyClassSizeMismatch: return "Size mismatch"
        case .verifyClassUnverifiable: return "Cannot verify"
        case .verifyClassUnreadable: return "Cannot read"
        case .verifyUnverifiableNote: return "These files have no checksum to compare against; this is not a failure"
        case .verifyNotVerified: return "Not verified yet"
        case .verifyAllGood: return "All passed"
        case .verifyFailedCount: return "{count} failed"
        case .verifyKernelNotAllGood: return "The core reports not everything passed (there are no failed items in the six categories)"
        case .batchMigrateRunning: return "Moving downloaded files: {done}/{total} ({doneBytes} / {totalBytes})"
        case .batchMigrateStartFailed: return "Files already downloaded in the old folder could not be moved to the new one and will be downloaded again from the start (the old files are still there). Reason: {reason}"
        case .batchMigrateProgressLost: return "Could not fetch the move progress; the old files may not have been fully moved."
        case .batchMigrateErrors: return "{count} files could not be moved to the new folder and will be downloaded again from the start (the old files are still there). First: {path}: {reason}"
        case .batchMigrateSkipped: return "{count} files already existed in the new folder and were not overwritten."
        case .aboutVersionUnknown: return "Version unknown (the app bundle is missing CFBundleShortVersionString / CFBundleVersion)"
        case .licenseMissingResource: return "The full GPLv2 text is missing from the app bundle (Contents/Resources/COPYING-GPLv2.txt). This is a packaging defect, not a mistake on your part: reinstall the client; if this copy came from someone else, ask the distributor for a license copy."
        case .licenseEmptyResource: return "The GPLv2 text inside the app bundle is empty (Contents/Resources/COPYING-GPLv2.txt). This is a packaging defect, not a mistake on your part: reinstall the client and ask the distributor for a license copy."
        case .licenseUnreadable: return "Could not read the full GPLv2 text: {reason}"
        case .licenseNotUTF8: return "The file is not UTF-8 text ({name})"
        case .updateNoticeNewer: return "A new version {latest} is available (current {current})"
        case .switchBusy: return "A delivery switch is still in progress; wait for it to finish."
        case .historyWriteFailed: return "Could not write the history: {reason}"
        case .downloadDirSaveFailed: return "The download folder was not changed (failed to write preferences: {reason})"
        case .downloadDirRestartBlocked: return "The core is still restarting, so this change did not take effect. The download folder has been recorded and will apply on the next launch."
        case .verboseLoggingSaveFailed: return "Verbose logging was not changed (failed to write preferences: {reason})"
        case .verboseLoggingRestartBlocked: return "The core is still restarting, so this change did not take effect. The verbose-logging switch has been recorded and will apply on the next launch."
        case .deliveryCodeTooLongReason: return "The delivery code is {bytes} bytes long, over the safety limit ({limit}). The core never answers or reports an error for such a request, it just waits forever - paste only the delivery code itself or the delivery page link."
        case .deliveryBaseURLTooLongReason: return "The custom download address is {bytes} bytes long, over the safety limit ({limit}). The core never answers or reports an error for such a request, it just waits forever - make sure what you pasted really is the delivery page address."
        case .busyLoadingDelivery: return "Loading the delivery manifest..."
        case .busyAddingTasks: return "Adding download tasks..."
        case .enqueuePathTooLong: return "This path is too long for a single request and was not sent"
        case .engineDisconnected: return "The download engine disconnected: {reason}"
        case .engineRestartingBusy: return "The download engine disconnected; restarting..."
        case .kernelRestartFailed: return "The core failed to restart: {reason}"
        case .protocolMismatchNote: return "The client and core versions do not match; update the client or the core and try again (core: {kernel})"
        case .engineReasonNoHandshake: return "The core has not completed the handshake"
        case .engineReasonNotStarted: return "The download engine has not started"
        case .engineReasonRunning: return "The core is running"
        case .coreNotFoundEnv: return "(environment variable BENAGEN_CORE is not set)"
        case .coreNotFound: return """
            Could not find the core executable benagen-core (looked in:
            {paths})
            """
        case .coreChannelShutdown: return "The core connection is closed (shutdown was called)"
        case .coreChannelDead: return "The core connection was declared dead (could not resynchronize after a read timeout)"
        case .corePipeClosed: return "The core process exited (pipe closed)"
        case .coreResponseMismatch: return "The core replied with response id {got} while {want} was expected (request/response out of sync)"
        case .coreLineNotJSON: return "A line from the core is not valid JSON: {line}"
        case .jsonNotAValue: return "Not a valid JSON value (not null/bool/number/string/array/object)"
        case .fileStoreMakeDir: return "Could not create the folder: {path} ({reason})"
        case .fileStoreWriteTemp: return "Could not write the temporary file: {path} ({reason})"
        case .fileStoreRename: return "Could not replace {path}: rename(2) errno={code}"
        case .protocolMissingResult: return "An ok:true response has no result (id {id})"
        case .protocolShapeMismatch: return "The result for id {id} does not match the shape of {type}: {reason}"
        case .protocolUnknownNode: return "Unknown node type {type}"
        case .protocolUnknownEntry: return "Unknown entry type {type}"
        case .clientProtocolShapeMismatch: return "The result does not match the shape of {type}: {reason}"
        case .coreStartFailed: return "Could not start the core process ({path}): {reason}"
        case .pipeWriteFailed: return "Could not write the request to the core (pipe closed): {reason}"
        case .pipeReadFailed: return "Could not read the core response (pipe closed): {reason}"
        case .breadcrumbBackNotice: return "Returned to the previous level"
        case .diagHeaderTitle: return "Diagnostic Log Export Overview"
        case .diagHeaderIntro: return """
            This file is the manifest of this export: it does not describe what the logs
            contain, but which files are in this folder and which time range each one covers.
            """
        case .diagHeaderRisk: return """
            ⚠️ Risk first: these logs leave your machine, and they may contain fragments of
            download URLs (raw errors from the download engine carry them verbatim). We do
            not filter automatically - reliably removing it requires recognising it first,
            and arbitrary third-party text cannot be recognised; an unreliable filter is
            worse than none (it makes people believe they are already safe).
            You may open the files and take a look before sending them.
            """
        case .diagHeaderNotRecorded: return """
            Never recorded: the delivery code itself, file paths themselves, rpc-secret,
            and the customer's folder names.
            (The delivery code and download folder are redacted literally as [redacted]
            before being written; but when no download folder is configured, the core falls
            back to its own default path - the shell does not know that one, so it cannot
            redact it. Neither can it redact a differently written copy inside third-party
            text - see above.)
            """
        case .diagHeaderContents: return "What is in here"
        case .diagProtocolNotConnected: return "not connected"
        case .diagLogFilesHeading: return "Log files (each: name / bytes / sha256 / covered time range)"
        case .diagNoLogFiles: return "There are no log files at all (every line below says \"not created yet\")."
        case .diagFileLine: return "{name}  {bytes} bytes  sha256={sha}  covers: {coverage}"
        case .diagAbsentLine: return "{name}  not created yet (this log has never been written)"
        case .diagNotDoingHeading: return "What is not done (all deliberate choices, not omissions)"
        case .diagNotDoing: return """
            · No archive: the export is just a folder, and this file is its manifest;
            · No locking and no snapshot: logs may still be written during the export, so the
              last line of a file may be half-written (the cost far outweighs the benefit, spec §4);
            · No automatic filtering of sensitive data (see the first section). Note that
              redacting the delivery code and download folder literally as [redacted] is not
              filtering: it removes the two strings the shell knows, not anything that looks
              like them in arbitrary text.
            """
        case .diagHowToReadHeading: return "How to read these logs"
        case .diagHowToRead: return """
            Each log has one record per line: ts=<unix seconds> event=<event name> key=value ...
            That time is UTC unix seconds (macOS `date -r <ts>`, Linux `date -d @<ts>`).
            The "covers" field takes the first and last line of the file, so you can see at a
            glance whether it covers the period when the problem happened. If it does not, the
            only option is to catch the next occurrence - so turn verbose logging on before
            reproducing, and you can turn it off again after exporting.
            """
        case .diagCoverageUnknown: return "unknown"
        case .diagFolderFailed: return "The diagnostic logs were not exported: could not create the folder {folder} (system says: {cause}). Fix: pick a writable folder and try again (for example the Desktop), or check whether that location is actually a file or the disk is full."
        case .diagCopyFailed: return "The diagnostic logs were not exported: could not copy into {file} (system says: {cause}). Fix: make sure that file still exists and is not exclusively held by another program (such as TextEdit), then try again."
        case .diagHeaderWriteFailed: return "The diagnostic logs were not exported: the overview file could not be written ({file}; system says: {cause}). Fix: check whether the target disk is full or the folder is read-only, then try again."
        case .diagMissingCore: return "(unavailable: the core executable is not where it is expected)"
        case .diagOsUnknown: return "macOS (version unavailable)"
        }
    }
}
