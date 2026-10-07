import Foundation

// ---------------------------------------------------------------------------
// 壳的偏好（阶段 E 规格 §2.1 + 详细诊断日志规格 §2.4）
//
// 今天有两项：**下载目录** 与 **详细日志**（任务 5 加的那一格）。它落
// `~/Library/Application Support/BenagenDownloader/preferences.json`，
// 与历史**同目录、另一个文件**（两者的生命周期与格式由各自负责）。
//
// ⚠️ **"未配置"是有效状态，不是错误**：空串 ⇒ 壳**不传** `--download-dir`
//    （E-5），由内核用自己的默认值。不要为了"看起来明确"而显式传一个
//    "和内核默认一样"的值 —— 那会在内核默认值变化时静默分叉。
//
// ⚠️ 与 `BatchHistory` 同一条底线（E-1）：读不出来 / 解析失败 / `version` 不认识
//    ⇒ **当未配置**并继续启动，绝不让应用起不来。所以 [`AppPreferences.parse`]
//    **没有 `throws`**（"失败"这条路径根本不存在）。
//
// 本文件只做**纯计算**（解析 / 序列化 / 判据 / 改写），不碰文件系统 —— 落盘走
// `JsonFileStore`（见 `BatchHistoryStore` 的同款做法），改目录那条链路在任务 3。
// ---------------------------------------------------------------------------

public struct AppPreferences: Equatable, Sendable {
    /// 给未来的自己的版本号：**不认识的版本 ⇒ 当未配置**（E-1）。
    ///
    /// 🔴 **加字段时不许递增它**（详细日志规格 §2.4 的红线）：上面那条语义意味着
    /// 递增之后**装了旧版程序的客户**读到自己那份新格式的文件 ⇒ 当未配置 ⇒
    /// 下载目录被悄悄换回默认值。新字段一律让"缺字段的旧文件读成默认值"，
    /// 版本号因此不必动（判据：`AppPreferencesTests.theVersionDoesNotMoveWhenAFieldIsAdded`）。
    public static let version = 1

    /// 没有配置过任何东西（= 一切走默认）。
    public static let empty = AppPreferences(downloadDir: "", verboseLogging: false)

    /// 用户选的下载目录。**空串 = 未配置**（E-5：不传 `--download-dir`）。
    public let downloadDir: String

    /// 详细诊断日志（规格 §2.4）。**缺字段 = `false`**（老客户那份 `preferences.json`
    /// 里没有它）。
    ///
    /// ⚠️ 它进内核 argv 的方式与下载目录**完全不同**：目录是
    ///    `--download-dir <路径>`（`DownloadDirectory.argument(for:)`），
    ///    而它是 `--log-level verbose`（**开着才拼**，关着什么都不拼 ——
    ///    内核的缺省就是 normal，多拼一对 `--log-level normal` 只多一处会漂的东西）。
    public let verboseLogging: Bool

    /// ⚠️ `downloadDir` 有默认值（"什么都没配"是一个**真实存在的状态**，E-5），
    ///    而 `verboseLogging` **没有**：这一格是 2026-10-06 新加的，给它一个默认值就会让
    ///    `AppPreferences(downloadDir: path)` 这种写法**静默地**把详细日志关掉 ——
    ///    这正是本功能修掉的那个 bug（`settingDownloadDir` 的旧写法）的同一形状。
    ///    **少传一个实参应该是编译错误**（与 `CoreClient.coreArguments` / `live` 同一条）。
    public init(downloadDir: String = "", verboseLogging: Bool) {
        self.downloadDir = Self.normalized(downloadDir)
        self.verboseLogging = verboseLogging
    }

    /// 配过下载目录没有。**它是"要不要传 `--download-dir`"的唯一判据**。
    public var isConfigured: Bool { !downloadDir.isEmpty }

    /// 改下载目录（返回新值）。传空串 = 回到未配置（设置窗口那颗「恢复默认」）。
    ///
    /// ⚠️ 必须**带上另一格**：写成 `AppPreferences(downloadDir: path)` 的结局是
    ///    用户改一次下载目录，详细日志就被**静默关掉**（盘上也是）。
    public func settingDownloadDir(_ path: String) -> AppPreferences {
        AppPreferences(downloadDir: path, verboseLogging: verboseLogging)
    }

    /// 改详细日志那一格（返回新值）。同 [`settingDownloadDir`]：**另一格原样带着走**。
    public func settingVerboseLogging(_ on: Bool) -> AppPreferences {
        AppPreferences(downloadDir: downloadDir, verboseLogging: on)
    }

    // MARK: - 磁盘形状

    /// 从文件内容解析（E-1：**任何**问题都当未配置，绝不抛）。
    ///
    /// 判据同 `BatchHistory.parse`：不是 JSON / 顶层不是对象 / 没有 `version` /
    /// `version` 不认识 / `download_dir` 不是字符串 ⇒ 一律 `.empty`。
    /// （`download_dir` 缺失或为空串同样是"未配置"，它不是损坏。）
    public static func parse(_ data: Data) -> AppPreferences {
        guard !data.isEmpty,
              let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let version = root["version"] as? Int, version == Self.version
        else { return .empty }

        return AppPreferences(downloadDir: root["download_dir"] as? String ?? "",
                              // ⚠️ **缺字段 = 关**：老客户那份文件里没有这一格。
                              //    写成 `?? true` 的表现是"从没开过详细日志的人，
                              //    升级之后日志突然开始详细记录"，而没有任何东西会红。
                              verboseLogging: root["verbose_logging"] as? Bool ?? false)
    }

    /// 同上（给测试与调用方一个不用自己转 `Data` 的入口）。
    public static func parse(_ text: String) -> AppPreferences {
        parse(Data(text.utf8))
    }

    /// 落盘的形状（规格 §2.1 / §2.4：
    /// `{"version": 1, "download_dir": "/abs/path", "verbose_logging": false}`，键名逐字）。
    ///
    /// 🔴 **`version` 一个字都不动**（递增会让旧客户端的下载目录被清空，而且静默 ——
    /// 见 [`version`] 与 `AppPreferencesTests.theVersionDoesNotMoveWhenAFieldIsAdded`）。
    public func serialized() throws -> Data {
        try JSONSerialization.data(withJSONObject: ["version": Self.version,
                                                    "download_dir": downloadDir,
                                                    "verbose_logging": verboseLogging],
                                   options: [.prettyPrinted, .sortedKeys,
                                             .withoutEscapingSlashes])
    }

    // MARK: - 路径

    /// ⚠️ **有意只去掉首尾空白，不做 `standardizingPath`**（E-8，理由写在这里）：
    ///    这条路径最终要**原样进内核的 argv**（`--download-dir`，`core/src/main.rs:1678`）。
    ///    软链接、`..`、结尾的 `/` 都是**文件系统与内核的语义**，壳在这儿替它解释一遍，
    ///    只会让"壳以为的路径"和"内核实际用的路径"悄悄分叉 —— 而分歧的代价是
    ///    **文件下到别的地方去**。（面板给的是绝对路径，本就不需要展开 `~`。）
    private static func normalized(_ raw: String) -> String {
        raw.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// ---------------------------------------------------------------------------
// 「详细日志」那一个勾选框：标签 / 说明 / 一次开关的结果
//
// 与 `windows/shell-core/src/presentation/app_preferences.rs` 里**同名的那两个类型**
// 逐条对位（那边的模块头逐字记着"上游是 macOS `SettingsView` 新增的那一节「诊断」"）。
// 两端的**文案可以有细微差别，字段与语义不能少**（规格 §3 的跨端契约口径）。
// ---------------------------------------------------------------------------

/// 「详细日志」那一个勾选框的**纯计算**（规格 §2.4）。
///
/// ⚠️ 放在 `Presentation/` 而不是视图里：这一层的规矩是"能写出断言的都归这里"
///    （全局约束 8），而视图那一层不单测。
public enum DiagnosticsToggle {
    /// 勾选框的标签（规格 §2.4 逐字：「勾选「详细日志」」）。
    public static let label = "详细日志"

    /// 标签下面那句说明。**三件事缺一不可**：
    ///   * 它**是什么用的**（排查问题）—— 否则用户会以为这是个"性能档"；
    ///   * **用完要关**（详细档是"每一次往返都记"，开着不放会一直占盘）；
    ///   * **改了会重启内核**（与下载目录那条同一后果，不说的话用户会在一次
    ///     正在跑的下载被停掉时才莫名其妙）。
    ///
    /// ⚠️ 最后那半句与 `DownloadDirectory.sectionNote` 说的是**同一件事**（那次重启）——
    ///    但**不许**为了"少一句话"就把它省掉：这是本面板上第二处会让内核重启的控件。
    /// ⚠️ 与 Windows 那份的 `DiagnosticsToggle::NOTE` **逐字同源**（两端给客户看的是
    ///    同一句话）。
    public static let note = "打开后日志会详细很多（用于排查问题），排查完请关掉。"
        + "改动会立刻重启内核（正在跑的任务会停）。"
}

/// 一次「改详细日志」的结果。
///
/// ## 🔴 为什么它是**另一个类型**，而不是复用 [`DownloadDirChange`]
///
/// 形状确实是复用的（与 `DownloadDirChange` 同款：标题 / 是不是失败 / 完整那句），
/// 但**那三句话没有复用**：`DownloadDirChange` 的三句把主词写死成了"下载目录"
/// （`下载目录已改（内核已按新目录重启）` / `已恢复默认下载目录（…）` /
/// `下载目录没有变化，没有重启内核`）。拿它去报一次**勾选框**的改动，用户会在
/// 常驻回执上读到一句**假话** —— 他一个目录都没改。本仓对"界面上说假话"的口径是
/// 明确的（`DownloadDirectory.confirmation` 那条判据就是为同一件事立的），
/// 所以宁可多这一个类型（Windows 那一侧为同一件事也多了同名的一个）。
///
/// ⚠️ 与 [`DownloadDirChange`] 的第二个差别：**没有"路径"那一格**
///    （这一格改的是一个开关，没有路径可显示）。
public enum VerboseLoggingChange: Equatable, Sendable {
    /// 改成了：偏好已落盘、内核已按**新**档位重启、**壳自己那一格也已经就位**。
    /// `on` = 改完之后是"开着"吗（它只影响那句话的措辞）。
    case changed(on: Bool)
    /// 目标与当前一致 ⇒ **什么都没做**（内核没重启、正在跑的任务不受影响）。
    case unchanged(on: Bool)
    /// 没改成：这是**可直接显示给用户的那句话**（内核原文 / 写盘失败原文，照登）。
    case failed(message: String)

    /// 成功那支的**固定标题**（**两个方向各一句**：开着与关着是两件事，
    /// 说成同一句会让用户分不清自己那一下把它改成了什么）。
    ///
    /// ⚠️ 它不含任何**长度不受控**的东西（没有路径、没有批次号）——
    ///    与 `DownloadDirChange.headline` 同一条纪律：这条回执会画在常驻那一行上，
    ///    高度不许由外部文本决定（那是一次真实布局事故的根因）。
    private static let changedOn = "详细日志已打开（内核已按详细档重启）"
    private static let changedOff = "详细日志已关闭（内核已按普通档重启）"
    /// 没改动那一支的固定全文（与 `DownloadDirChange` 的那一句逐字同构）。
    private static let unchangedText = "详细日志没有变化，没有重启内核"

    /// **这条回执的第一行**：固定短的标题，任何情况下都不含外部文本。
    ///
    /// ⚠️ `.failed` 那一格**照登原文**（内核的失败原因 / 写盘失败的系统文本），
    ///    不加工、不截断成"标题"—— 与 `DownloadDirChange.headline` 逐字同一条。
    ///    它同样是**外部文本**，高度由**视图**那边统一封顶
    ///    （`Views/ResidentNotice.swift` 的 `BoundedNoticeText`），**全文始终都在**。
    public var headline: String {
        switch self {
        case .changed(let on): return on ? Self.changedOn : Self.changedOff
        case .unchanged: return Self.unchangedText
        case .failed(let message): return message
        }
    }

    /// 这一行说的是"没成"吗（视图据此选图标与颜色）。
    public var isFailure: Bool {
        if case .failed = self { return true }
        return false
    }
}
