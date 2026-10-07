import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 壳的偏好（阶段 E 规格 §2.1，任务 1）
//
// 今天只有一项：下载目录。**未配置 = 空串**，而"未配置"是**有效状态**，不是错误
// ——它对应 E-5：不传 `--download-dir`，由内核用自己的默认值。
//
// ⚠️ 与 `BatchHistory` 同一条底线（E-1）：读不出来 / 解析失败 / `version` 不认识
//    ⇒ **当未配置**并继续启动。这个类型同样不碰文件系统。
// ---------------------------------------------------------------------------

@Test func anUnconfiguredPreferenceIsTheEmptyString() {
    #expect(AppPreferences.empty.downloadDir == "")
    #expect(AppPreferences(downloadDir: "", verboseLogging: false).downloadDir == "")
    #expect(!AppPreferences.empty.isConfigured)
}

@Test func aConfiguredPreferenceKnowsIt() {
    #expect(AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: false).isConfigured)
    #expect(AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: false).downloadDir == "/Volumes/Data/交付")
}

@Test func roundTripOfTheConfiguredAndUnconfiguredPreference() throws {
    let configured = AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录", verboseLogging: false)
    #expect(AppPreferences.parse(try configured.serialized()) == configured)

    #expect(AppPreferences.parse(try AppPreferences.empty.serialized()) == .empty,
            "空串（未配置）也要能原样往返 —— 它是 E-5 的判据")
}

@Test func garbageIsTreatedAsUnconfigured() {
    // E-1：坏文件 ⇒ **当未配置**（不抛、不崩、更不该把应用拦在启动之外）。
    #expect(AppPreferences.parse(Data()) == .empty, "空文件")
    #expect(AppPreferences.parse("") == .empty, "空串")
    #expect(AppPreferences.parse("not json at all") == .empty, "非 JSON")
    #expect(AppPreferences.parse("[1,2,3]") == .empty, "顶层不是对象")
    #expect(AppPreferences.parse(#"{"version":2,"download_dir":"/tmp/x"}"#) == .empty, "version 不认识")
    #expect(AppPreferences.parse(#"{"download_dir":"/tmp/x"}"#) == .empty, "没有 version")
    #expect(AppPreferences.parse(#"{"version":"1","download_dir":"/tmp/x"}"#) == .empty, "version 不是数")
    #expect(AppPreferences.parse(#"{"version":1,"download_dir":5}"#) == .empty, "目录不是字符串")
    #expect(AppPreferences.parse(#"{"version":1}"#) == .empty, "没有 download_dir ⇒ 未配置")
}

@Test func thePreferencesFileShapeUsesTheSpecifiedFieldNames() throws {
    // 规格 §2.1 的形状：`{"version": 1, "download_dir": "/abs/path"}`，键名逐字。
    let text = String(decoding: try AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: false).serialized(),
                      as: UTF8.self)

    #expect(text.contains("\"version\""))
    #expect(text.contains("\"download_dir\""))
    #expect(!text.contains("downloadDir"))
    #expect(text.contains("/Volumes/Data/交付"), "路径原样写出去（含中文）")
}

@Test func settingADirectoryKeepsThePathVerbatimExceptForSurroundingWhitespace() {
    // ⚠️ 有意偏离（E-8，理由写在这里）：只去掉**首尾空白**，不做 `standardizingPath`。
    //    理由是这条路径最终要原样进内核的 argv（`--download-dir`）：软链接、`..`、
    //    结尾的 `/` 都是**内核与文件系统的语义**，壳替它解释一遍只会在两边分叉。
    //    （面板给的是绝对路径，本就不需要展开 `~`。）
    #expect(AppPreferences(downloadDir: "  /Volumes/Data/交付  ", verboseLogging: false).downloadDir == "/Volumes/Data/交付")
    #expect(AppPreferences(downloadDir: "/Volumes/Data/../Data/交付/", verboseLogging: false).downloadDir == "/Volumes/Data/../Data/交付/")
    #expect(AppPreferences(downloadDir: "   ", verboseLogging: false).downloadDir == "", "只有空白 = 未配置")
}

@Test func clearingTheDownloadDirectoryGoesBackToTheUnconfiguredState() {
    // 设置窗口那颗「恢复默认」= 回到**未配置**（于是不传 `--download-dir`）。
    // 它不是"传一个和内核默认值一样的值"——那正是 E-5 明禁的静默分叉。
    let cleared = AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: false).settingDownloadDir("")
    #expect(cleared == .empty)
    #expect(!cleared.isConfigured)
}

// ---------------------------------------------------------------------------
// 「详细日志」那一格（规格 §2.4，任务 5）
// ---------------------------------------------------------------------------

/// 🔴 **版本号不许递增**（递增会让旧客户端的下载目录被清空，而且静默）。
///
/// `AppPreferences.parse` 对**不认识的 version** 的处置是"整份文件当未配置"（E-1）。
/// 于是"加了字段就顺手把版本号 +1"的结局是：装了旧版程序的客户，读到自己那份
/// 新格式的 `preferences.json` ⇒ **当未配置** ⇒ 下载目录被悄悄换回默认值。
/// 新字段必须让**缺字段的旧文件读成默认值**（`?? false`），版本号因此不必动。
@Test func theVersionDoesNotMoveWhenAFieldIsAdded() {
    #expect(AppPreferences.version == 1)
    #expect(AppPreferences.parse(#"{"version": 1, "download_dir": "/tmp/x"}"#).downloadDir == "/tmp/x")
    #expect(AppPreferences.parse(#"{"version": 1, "download_dir": "/tmp/x"}"#).verboseLogging == false)
}

/// 老客户那份 `preferences.json` 里**没有**这一格 ⇒ 读成 `false`（规格 §2.4）。
///
/// 判别力：把 `parse` 里的 `?? false` 写成 `?? true`，本条必红 ——
/// 而真机上的表现是"从没开过详细日志的人，升级之后日志突然开始详细记录"。
@Test func aMissingVerboseFieldReadsAsOff() {
    #expect(AppPreferences.parse(#"{"version": 1, "download_dir": "/tmp/x"}"#).verboseLogging == false)
    #expect(AppPreferences.parse(#"{"version": 1}"#).verboseLogging == false)
    #expect(AppPreferences(downloadDir: "", verboseLogging: false).verboseLogging == false)
    #expect(AppPreferences.empty.verboseLogging == false)
    // 坏文件同样回 `.empty`（那两格一起回默认值）。
    #expect(AppPreferences.parse("not json at all").verboseLogging == false)
    // 类型不对 ⇒ **不猜**：当它没给（同 `download_dir` 那一格的口径）。
    #expect(AppPreferences.parse(#"{"version":1,"verbose_logging":"yes"}"#).verboseLogging == false)
}

/// 落盘的键名（规格 §2.4：`verbose_logging`，**逐字**）与往返。
@Test func theDiskShapeCarriesTheVerboseFlag() throws {
    let text = String(decoding: try AppPreferences(downloadDir: "/Volumes/Data/交付",
                                                   verboseLogging: true).serialized(),
                      as: UTF8.self)
    #expect(text.contains("\"verbose_logging\""))
    #expect(!text.contains("verboseLogging"), "键名是下划线那种（与 `download_dir` 同一套）")

    let on = AppPreferences(downloadDir: "/Volumes/Data/交付", verboseLogging: true)
    #expect(AppPreferences.parse(try on.serialized()) == on, "开着那一格也要能原样往返")
    #expect(AppPreferences.parse(try AppPreferences.empty.serialized()).verboseLogging == false)
}

/// 🔴 **两格互不干扰**：改一格不许把另一格抹掉。
///
/// 判别力：把 `settingDownloadDir` 写成 `AppPreferences(downloadDir: path)`
/// （漏掉第二格）⇒ 用户改一次下载目录，详细日志就被**静默关掉**（盘上也是）。
/// 反方向同理。两个 setter 各钉一条。
@Test func changingOneSettingKeepsTheOther() {
    let on = AppPreferences(downloadDir: "/data/交付", verboseLogging: true)
    #expect(on.settingVerboseLogging(false).downloadDir == "/data/交付", "关详细日志不许动目录")
    #expect(on.settingVerboseLogging(true).verboseLogging)
    #expect(on.settingDownloadDir("/data/新").verboseLogging, "改目录不许把详细日志那一格抹掉")
    #expect(on.settingDownloadDir("").verboseLogging, "恢复默认目录也不许")
    #expect(on.settingDownloadDir("").downloadDir == "")
}

// ---------------------------------------------------------------------------
// 「诊断」那一节的文案与回执（跨端：与 Windows `presentation/app_preferences.rs` 同源）
// ---------------------------------------------------------------------------

/// 勾选框的说明必须说全三件事（规格 §2.4 / Windows 那份的 `NOTE` 逐字同源）：
/// 它**是干什么用的**、**用完要关**、**改了会重启内核**。
@Test func theDiagnosticsNoteSaysAllThreeThings() {
    #expect(DiagnosticsToggle.label == "详细日志")
    let note = DiagnosticsToggle.note
    #expect(note.contains("排查"), "要说清它是干什么用的：\(note)")
    #expect(note.contains("关掉"), "要说清用完要关（详细档一直开着会一直占盘）：\(note)")
    #expect(note.contains("重启内核"), "要说清改了会重启内核（正在跑的任务会停）：\(note)")
}

/// 🔴 **回执不许把主词写错**：开着与关着是**两句不同的话**。
///
/// 判别力：两支写成同一句 ⇒ 本条红 —— 而用户会分不清自己那一下到底把它改成了什么。
/// ⚠️ 借用 `DownloadDirChange` 那三句（主词写死成"下载目录"）来报一次勾选框的改动
///    会让用户在常驻回执上读到一句**假话**，所以它是另一个类型（理由见类型文档）。
@Test func theVerboseReceiptNamesBothDirections() {
    #expect(VerboseLoggingChange.changed(on: true).headline.contains("打开"))
    #expect(VerboseLoggingChange.changed(on: false).headline.contains("关闭"))
    #expect(VerboseLoggingChange.changed(on: true).headline
            != VerboseLoggingChange.changed(on: false).headline)
    #expect(VerboseLoggingChange.unchanged(on: true).headline.contains("没有变化"))
    #expect(!VerboseLoggingChange.changed(on: true).headline.contains("下载目录"),
            "主词不许写错（这一格改的不是目录）")
    #expect(VerboseLoggingChange.failed(message: "内核重启失败：内核没了").headline
            == "内核重启失败：内核没了", "失败那一格**原文照登**（约束 3）")
    #expect(VerboseLoggingChange.failed(message: "x").isFailure)
    #expect(!VerboseLoggingChange.changed(on: true).isFailure)
}
