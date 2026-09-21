import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 历史接进 `AppModel`（阶段 E 规格 §1.2 / §1.3，任务 1）
//
// 这里钉的是三件事：
//   ① **只在成功的加载之后**才写历史（§1.2，与内核 `remember_last_code` 的时机一致）；
//   ② 启动自动加载的码来自**壳的历史**（§1.3 / E-4），内核的 `last_code` **只在历史为空时**
//      做一次性播种，之后不再参与决策；
//   ③ 写盘失败**不毁掉一次成功的加载**，但要能被上报。
//
// ⚠️⚠️ 安全红线：这里的每一个 store 都指向 `FileManager.temporaryDirectory` 下的临时目录。
//      `~/Library/Application Support/BenagenDownloader/` 里是**人类伙伴真实在用的**
//      `settings.json` 与 `last_code`（以及本任务之后会产生的新历史）——测试碰它们
//      = 破坏他的现场。所以**没有一处**使用 `BatchHistoryStore()` 的默认参数。
// ---------------------------------------------------------------------------

/// 临时目录 + 一个指向它的 store（**唯一的**建 store 的方式，见文件头的红线）。
private struct TempHistory {
    let dir: URL
    let store: BatchHistoryStore

    init() throws {
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("benagen-shell-history-tests", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        store = BatchHistoryStore(url: dir.appendingPathComponent("history.json"))
    }

    func cleanUp() { try? FileManager.default.removeItem(at: dir) }
}

/// 一个 `code` 为 `C24-8` 的交付清单 —— `Wire` 那两条的码都不是它，而下面有好几条测试
/// 要用内核握手里那个 `last_code`（`Wire.getSettings` 给的正是 `C24-8`）走真实链路。
private let deliveryC24 = #"""
{"code":"C24-8","page_url":"http://dl.example/C24-8/index.html","base_url":"http://dl.example",
 "created_at":"2026-09-18T09:00:00+08:00","expires_at":"2026-10-18T09:00:00+08:00",
 "expired":false,"total_files":3,"total_bytes":9000,
 "tree":{"type":"dir","name":"C24-8","children":{}}}
"""#

// ---------------------------------------------------------------------------
// 写：只在成功路径上（§1.2）
// ---------------------------------------------------------------------------

@MainActor
@Test func aSuccessfulLoadIsRecordedInTheHistory() async throws {
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let h = temp.store.load()
    #expect(h.entries.count == 1)
    #expect(h.entries.first?.code == "AAA-1")
    #expect(h.entries.first?.baseURL == "", "没指定交付服务器 ⇒ 空串 = 默认服务器（§1.1）")
    // 「同形」的判据：写出去的时间戳必须**被既有的呈现实现认得**
    // （认不得时 `TimestampPresentation.text` 会原样返回）。
    let raw = h.entries.first?.lastUsedAt ?? ""
    #expect(TimestampPresentation.text(raw) != raw,
            "last_used_at 必须是与 created_at 同形的 ISO8601（实际 \(raw)）")
    // 内存里那份也是权威（换码面板要读它）。
    #expect(model.history == h)
}

@MainActor
@Test func aFailedLoadIsNotRecorded() async throws {
    // §1.2：失败的加载**不写** —— 否则历史里会堆满打错的码。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("load_delivery", throwing: .rpc(code: "delivery_fetch_failed",
                                              message: "拉取交付清单失败：HTTP 404"))
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.loadDelivery(code: "TYPO-9", baseURL: nil)

    #expect(temp.store.load().isEmpty)
    #expect(!FileManager.default.fileExists(atPath: temp.store.url.path),
            "一次失败的加载不该在新盘上留下任何文件")
}

@MainActor
@Test func theHistoryIsLoadedBeforeTheFirstWriteSoAManualLoadNeverWipesIt() async throws {
    // ⚠️ 这条防的是一个**数据丢失**：如果成功路径直接拿"内存里的空历史"去写盘，
    //    用户在空态页手输一个码（没经过自动加载那条路）就会把盘上已有的 50 条**全抹掉**。
    let temp = try TempHistory()
    defer { temp.cleanUp() }
    try temp.store.save(BatchHistory.empty
        .recording(code: "OLD-1", at: Date(timeIntervalSince1970: 1_700_000_000)))

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    let codes = Set(temp.store.load().entries.map(\.code))
    #expect(codes == ["OLD-1", "AAA-1"], "手输的那次加载只能**追加**，不能覆盖盘上已有的历史")
}

// ---------------------------------------------------------------------------
// 权威：壳的历史（§1.3 / E-4）
// ---------------------------------------------------------------------------

@MainActor
@Test func theShellsHistoryWinsOverTheKernelsLastCode() async throws {
    // E-4 的一半：**"上次用的码"的权威只有一个 = 壳的历史**。
    // 这里刻意让内核的 `last_code`（C24-8）与历史里最近的那条（BBB-2）**不同** ——
    // 否则这条测试对"壳到底读了谁"没有判别力。
    let temp = try TempHistory()
    defer { temp.cleanUp() }
    try temp.store.save(BatchHistory.empty
        .recording(code: "AAA-1", at: Date(timeIntervalSince1970: 1_700_000_000))
        .recording(code: "BBB-2", at: Date(timeIntervalSince1970: 1_700_000_100)))

    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)          // last_code = "C24-8"
    fake.stub("load_delivery", Wire.deliveryB)           // code = "BBB-2"
    fake.stub("get_tree", Wire.treeB)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.params("load_delivery") == .object(["code": .string("BBB-2")]),
            "自动加载的码必须来自**壳的历史里最近使用的那条**，不是内核的 last_code")
    #expect(fake.callCount("load_delivery") == 1, "一次都不该多发")
    guard case .loaded = model.loadState else {
        Issue.record("自动加载成功应进 .loaded，实际 \(model.loadState)")
        return
    }
}

@MainActor
@Test func anEmptyHistorySeedsOnceFromTheKernelsLastCode() async throws {
    // E-4 的另一半：壳的历史**为空**时（第一次升级到这个版本），
    // 读内核的 `last_code` 作为**一次性种子**，写进历史。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)          // last_code = "C24-8"
    fake.stub("load_delivery", deliveryC24)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.params("load_delivery") == .object(["code": .string("C24-8")]),
            "历史为空 ⇒ 用内核记着的那条播种并加载")
    #expect(temp.store.load().entries.map(\.code) == ["C24-8"], "种子要写进历史")
}

@MainActor
@Test func afterTheSeedTheKernelsLastCodeIsNeverConsultedAgain() async throws {
    // ⚠️ 这一条是 E-4 的**判别式**："迁移之后壳不再读它"。
    //    内核这一轮给的是 DDDD-9（比如用户在别的机器/别的版本上用过），
    //    而壳必须继续用历史里那条 —— 否则就会出现规格明写要防的分裂：
    //    "壳显示 A、内核偷偷加载 B"。
    let temp = try TempHistory()
    defer { temp.cleanUp() }
    try temp.store.save(BatchHistory.empty
        .recording(code: "C24-8", at: Date(timeIntervalSince1970: 1_700_000_000)))

    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", #"{"settings":\#(Wire.settings),"last_code":"DDDD-9"}"#)
    fake.stub("load_delivery", deliveryC24)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.params("load_delivery") == .object(["code": .string("C24-8")]),
            "历史非空 ⇒ 内核的 last_code 不再参与决策")
}

@MainActor
@Test func anEmptyHistoryAndNoKernelCodeLoadsNothingAndWritesNothing() async throws {
    // 全新安装：没有历史、内核也没记着码 ⇒ **一个请求都不发**，盘上**一个字节都不写**
    //（凭空造一个空的 history.json 只会让"到底有没有历史"这件事变模糊）。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", #"{"settings":\#(Wire.settings),"last_code":""}"#)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.callCount("load_delivery") == 0)
    #expect(model.loadState == .idle, "界面停在空态页，不是错误页")
    #expect(!FileManager.default.fileExists(atPath: temp.store.url.path))
}

@MainActor
@Test func withoutAPersistenceSeamTheKernelLastCodeStillDecides() async throws {
    // 没注入 store（= 不是生产形态的接缝，见 `AppModel.init`）时，行为与改动前**逐字相同**：
    // 内核的 `last_code` 直接用来加载，且**不碰盘**。
    // 这条也是"测试替身默认不写盘"这条设计的判据（见 `AppModel.live()` 的注释）。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)          // last_code = "C24-8"
    fake.stub("load_delivery", deliveryC24)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake)

    await model.start()
    await model.loadRememberedDelivery()

    #expect(fake.params("load_delivery") == .object(["code": .string("C24-8")]))
}

// ---------------------------------------------------------------------------
// base_url 一起记（§1.1 / 步骤 5.3）
// ---------------------------------------------------------------------------

@MainActor
@Test func theBaseURLIsRecordedAndComesBackOnTheNextAutoLoad() async throws {
    // `base_url` 记下"这一批是从哪个交付服务器加载的"（空串 = 默认服务器）。
    // ⚠️ 记的是**用户这一次用的那个值**（输入的 `baseURL`），不是内核回显的
    //    `info.base_url`：后者在"用户没指定"时是内核的默认服务器地址，把它写进历史
    //    再显式传回去，就会在**内核默认值变化时静默分叉** —— 那正是 E-5 要防的形态。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    let outcome = await model.switchDelivery(code: "AAA-1", baseURL: "http://alt.example")
    #expect(outcome == .switched(code: "AAA-1"))
    #expect(temp.store.load().entries.first?.baseURL == "http://alt.example",
            "指定了服务器 ⇒ 记下来")

    // 下一次启动（新的内核、同一个历史）会自动带着这个地址加载。
    let next = FakeCore()
    next.stub("hello", Wire.hello)
    next.stub("get_settings", #"{"settings":\#(Wire.settings),"last_code":"别的码"}"#)
    next.stub("load_delivery", Wire.deliveryA)
    next.stub("get_tree", Wire.treeA)
    let restarted = AppModel(client: next, historyStore: temp.store)
    await restarted.start()
    await restarted.loadRememberedDelivery()

    #expect(next.params("load_delivery")
            == .object(["code": .string("AAA-1"), "base_url": .string("http://alt.example")]))
}

@MainActor
@Test func aDefaultServerBatchIsRecordedWithAnEmptyBaseURLAndLoadedWithoutOne() async throws {
    // 反向：**没指定**服务器的那一批 ⇒ `base_url` 是空串 ⇒ 下一次自动加载**不带**
    // `base_url`（E-5 的同一条纪律：不要显式传一个"和默认一样"的值）。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)     // 回显的 base_url 是 http://dl.example
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(temp.store.load().entries.first?.baseURL == "")
    #expect(temp.store.load().entries.first?.baseURLOrNil == nil)

    let next = FakeCore()
    next.stub("hello", Wire.hello)
    next.stub("get_settings", #"{"settings":\#(Wire.settings),"last_code":""}"#)
    next.stub("load_delivery", Wire.deliveryA)
    next.stub("get_tree", Wire.treeA)
    let restarted = AppModel(client: next, historyStore: temp.store)
    await restarted.start()
    await restarted.loadRememberedDelivery()

    #expect(next.params("load_delivery") == .object(["code": .string("AAA-1")]),
            "空串 = 默认服务器 ⇒ 请求里不出现 base_url 这个键")
}

// ---------------------------------------------------------------------------
// 写盘失败：上报，但不毁掉一次成功的加载
// ---------------------------------------------------------------------------

@MainActor
@Test func aFailedHistoryWriteIsReportedAndDoesNotBreakTheLoad() async throws {
    // 简报步骤 4："写失败要能被上报（不要让用户以为存上了）"。
    // 但**加载本身是成功的** —— 历史存不下来不该把一次成功的加载判成失败。
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("benagen-shell-history-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    // 目标路径的**父级是一个普通文件** ⇒ 建目录必失败 ⇒ 写盘必失败。
    let blocker = dir.appendingPathComponent("我是一个文件")
    try Data("x".utf8).write(to: blocker)
    let store = BatchHistoryStore(url: blocker.appendingPathComponent("history.json"))

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: store)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    guard case .loaded = model.loadState else {
        Issue.record("写历史失败不该影响加载，实际 \(model.loadState)")
        return
    }
    #expect(model.historyWriteFailure != nil, "写失败必须有落点（否则用户以为存上了）")
    #expect(model.history.entries.first?.code == "AAA-1", "内存里那份仍然是权威")

    // ⚠️ **最终审查重要 1**：这个字段曾经**没有任何视图读它** —— 注释承诺了落点，
    //    实现没兑现（症状：敲完备注、写盘失败、应用一个字都不说，重启后备注不见了，
    //    而且**无从归因**）。现在落点有两处（`RootView` 主区那行常驻提示 +
    //    换码面板的历史那一段，两处读的是同一个字段），视图接线按本项目硬约束不单测
    //    （`macos/README.md` 第 19c 条是人眼验收入口）—— 这里钉住的是模型那半边：
    //    **原文看得见（非空）**、**能被收起**（两处那颗 `×` 的同一个出口）、
    //    **收起后不会自己回来**（下一个写盘动作才会重新写上）。
    #expect(model.historyWriteFailure?.isEmpty == false,
            "落点要显示的是**原文**，不是一句空话：\(model.historyWriteFailure ?? "nil")")
    model.dismissHistoryWriteFailure()
    #expect(model.historyWriteFailure == nil, "收起之后那一行消失（两处落点读同一个字段）")
}

@MainActor
@Test func aSuccessfulWriteLeavesNoFailureBehind() async throws {
    // 反向：写成功时那个字段必须是空的 —— 否则主区会**长期**挂着一条早就不成立的
    // 警告（"历史记录写入失败"），而用户没有任何办法知道它已经过去了。
    let temp = try TempHistory()
    defer { temp.cleanUp() }

    let fake = FakeCore()
    fake.stub("load_delivery", Wire.deliveryA)
    fake.stub("get_tree", Wire.treeA)
    let model = AppModel(client: fake, historyStore: temp.store)

    await model.loadDelivery(code: "AAA-1", baseURL: nil)

    #expect(model.historyWriteFailure == nil, "写盘成功 ⇒ 那一行不该出现")
}
