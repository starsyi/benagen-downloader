import Foundation

// ---------------------------------------------------------------------------
// 偏好文件的位置与读写（阶段 E 规格 §2.1，任务 3）
//
// 与 `BatchHistoryStore` **同款、也同样薄**，职责边界逐字相同：
//   · 位置 + "坏就当未配置"这两个决定在这里；
//   · 字节搬运在 `JsonFileStore`（原子写 = E-2）；
//   · 解析 / 判据 / 改写（`argument(for:)`、`confirmation`、`check`）在
//     `Presentation/AppPreferences.swift` 与 `Presentation/DownloadDirectory.swift`。
//
// ⚠️ 为什么在模块根而不是 `Presentation/`：`Presentation/` 那一层的契约是
//    **纯计算、不碰文件系统**（见 `AppPreferences.swift` 开头那段）。这个文件碰盘，
//    与它同款的那一份（`BatchHistoryStore.swift`）也在模块根 —— 放这里才是一致的。
//
// ⚠️⚠️ **`url` 是可注入的**（默认才是 Application Support）：测试必须能把它指到
//     临时目录 —— `~/Library/Application Support/BenagenDownloader/` 里放着
//     人类伙伴**真实在用**的配置，覆盖它 = 破坏他的现场。
// ---------------------------------------------------------------------------

public struct AppPreferencesStore: Sendable {
    /// 偏好文件的名字（与历史**同目录、另一个文件** —— 两者的格式与生命周期由各自负责）。
    public static let fileName = "preferences.json"

    /// `~/Library/Application Support/BenagenDownloader/preferences.json`。
    ///
    /// ⚠️ 只**构造**路径、不碰盘（同 `BatchHistoryStore.defaultURL`）。
    public static var defaultURL: URL {
        ShellStorage.directory.appendingPathComponent(fileName)
    }

    public let url: URL

    /// ⚠️ 默认参数指向**人类伙伴真实的目录**，所以**测试里永远显式传 `url:`**。
    public init(url: URL = AppPreferencesStore.defaultURL) {
        self.url = url
    }

    private let files = JsonFileStore()

    /// 读。文件不存在、读不动、内容坏了、`version` 不认识 ⇒ **当未配置**（E-1：
    /// 绝不抛、绝不崩，更不许把应用拦在启动之外 —— 这是壳第一次写盘那条底线的下半条）。
    ///
    /// ⚠️ 没有 `throws`：**"失败"这条路径根本不存在**（同 `BatchHistoryStore.load()`）。
    public func load() -> AppPreferences {
        guard let data = files.read(url) else { return .empty }
        return AppPreferences.parse(data)
    }

    /// 原子地把整份偏好写下去（E-2，实现见 `JsonFileStore.write`）。
    ///
    /// ⚠️ 失败**抛**而不是吞：一次"以为存上了"的静默失败，用户要等到**下一次启动**
    ///    才会发现（设置又变回去了）—— 而那时已经无从归因。所以调用方
    ///    （`AppModel.changeDownloadDir`）必须有机会**中止**并说一句。
    public func save(_ preferences: AppPreferences) throws {
        try files.write(try preferences.serialized(), to: url)
    }
}
