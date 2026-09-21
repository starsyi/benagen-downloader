import Foundation

// ---------------------------------------------------------------------------
// 历史文件的位置与读写（阶段 E 规格 §1.1）
//
// 职责边界：**位置 + "坏就当空"这两个决定在这里**，字节搬运在 `JsonFileStore`，
// 增删排序淘汰在 `BatchHistory`。这一层刻意薄到只剩两件事：
//   · `load()` —— 读不出来 / 解析失败 / `version` 不认识 ⇒ **空历史**（E-1，
//     所以它**没有 `throws`**：没有"失败"这条路径可走）；
//   · `save(_:)` —— 一个**原子**的整份覆写（E-2），失败**抛**给调用方
//     （不能让人以为存上了）。
//
// ⚠️ **不在这里做"读-改-写"**：那是"一次成功加载之后记一条"的语义，属于 `AppModel`
//    （它知道什么算成功、时间是什么时候、内存里那份权威是谁）。
//
// ⚠️⚠️ **`url` 是可注入的**（默认才是 Application Support）：测试必须能把它指到
//     临时目录 —— `~/Library/Application Support/BenagenDownloader/` 里是
//     人类伙伴真实在用的数据，覆盖它 = 破坏他的现场。
// ---------------------------------------------------------------------------

public struct BatchHistoryStore: Sendable {
    /// 历史文件的名字（与内核的 `settings.json` / `last_code` 同目录、**不同文件**）。
    public static let fileName = "history.json"

    /// `~/Library/Application Support/BenagenDownloader/history.json`。
    ///
    /// ⚠️ 只**构造**路径、不碰盘：默认位置在这里是一个可读的事实，
    ///    而"有没有那个文件/那个目录"是运行期才知道的事。
    public static var defaultURL: URL {
        ShellStorage.directory.appendingPathComponent(fileName)
    }

    public let url: URL

    /// ⚠️ 默认参数指向**人类伙伴真实的目录**，所以**测试里永远显式传 `url:`**。
    public init(url: URL = BatchHistoryStore.defaultURL) {
        self.url = url
    }

    private let files = JsonFileStore()

    /// 读。文件不存在、读不动、内容坏了 ⇒ **空历史**（E-1：绝不抛、绝不崩）。
    public func load() -> BatchHistory {
        guard let data = files.read(url) else { return .empty }
        return BatchHistory.parse(data)
    }

    /// 原子地把整份历史写下去（先由 `BatchHistory` 归一化，写出去的永远是"排好序、
    /// 去过重、不超上限"的那一份）。
    ///
    /// ⚠️ 失败**抛**而不是吞：历史存不下来是一件用户能感知的事（重启后什么都没了），
    ///    调用方必须有机会说一句（`AppModel.historyWriteFailure`）。
    public func save(_ history: BatchHistory) throws {
        try files.write(try history.serialized(), to: url)
    }
}
