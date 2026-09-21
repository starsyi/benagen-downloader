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
    #expect(AppPreferences().downloadDir == "")
    #expect(!AppPreferences.empty.isConfigured)
}

@Test func aConfiguredPreferenceKnowsIt() {
    #expect(AppPreferences(downloadDir: "/Volumes/Data/交付").isConfigured)
    #expect(AppPreferences(downloadDir: "/Volumes/Data/交付").downloadDir == "/Volumes/Data/交付")
}

@Test func roundTripOfTheConfiguredAndUnconfiguredPreference() throws {
    let configured = AppPreferences(downloadDir: "/Volumes/Data/交付/有 空格的目录")
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
    let text = String(decoding: try AppPreferences(downloadDir: "/Volumes/Data/交付").serialized(),
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
    #expect(AppPreferences(downloadDir: "  /Volumes/Data/交付  ").downloadDir == "/Volumes/Data/交付")
    #expect(AppPreferences(downloadDir: "/Volumes/Data/../Data/交付/").downloadDir == "/Volumes/Data/../Data/交付/")
    #expect(AppPreferences(downloadDir: "   ").downloadDir == "", "只有空白 = 未配置")
}

@Test func clearingTheDownloadDirectoryGoesBackToTheUnconfiguredState() {
    // 设置窗口那颗「恢复默认」= 回到**未配置**（于是不传 `--download-dir`）。
    // 它不是"传一个和内核默认值一样的值"——那正是 E-5 明禁的静默分叉。
    let cleared = AppPreferences(downloadDir: "/Volumes/Data/交付").settingDownloadDir("")
    #expect(cleared == .empty)
    #expect(!cleared.isConfigured)
}
