import Foundation

// ---------------------------------------------------------------------------
// 壳的偏好（阶段 E 规格 §2.1）
//
// 今天只有一项：**下载目录**。它落 `~/Library/Application Support/BenagenDownloader/
// preferences.json`，与历史**同目录、另一个文件**（两者的生命周期与格式由各自负责）。
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
    public static let version = 1

    /// 没有配置过任何东西（= 一切走默认）。
    public static let empty = AppPreferences()

    /// 用户选的下载目录。**空串 = 未配置**（E-5：不传 `--download-dir`）。
    public let downloadDir: String

    public init(downloadDir: String = "") {
        self.downloadDir = Self.normalized(downloadDir)
    }

    /// 配过下载目录没有。**它是"要不要传 `--download-dir`"的唯一判据**。
    public var isConfigured: Bool { !downloadDir.isEmpty }

    /// 改下载目录（返回新值）。传空串 = 回到未配置（设置窗口那颗「恢复默认」）。
    public func settingDownloadDir(_ path: String) -> AppPreferences {
        AppPreferences(downloadDir: path)
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

        return AppPreferences(downloadDir: root["download_dir"] as? String ?? "")
    }

    /// 同上（给测试与调用方一个不用自己转 `Data` 的入口）。
    public static func parse(_ text: String) -> AppPreferences {
        parse(Data(text.utf8))
    }

    /// 落盘的形状（规格 §2.1：`{"version": 1, "download_dir": "/abs/path"}`，键名逐字）。
    public func serialized() throws -> Data {
        try JSONSerialization.data(withJSONObject: ["version": Self.version,
                                                    "download_dir": downloadDir],
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
