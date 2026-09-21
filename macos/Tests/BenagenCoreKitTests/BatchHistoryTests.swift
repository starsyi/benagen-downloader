import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 批次历史的纯计算（阶段 E 规格 §1.1，任务 1）
//
// 这个文件覆盖的是**规格 §3 点名要有测试的三条**里的两条
// （E-1 解析容错、E-3 上限淘汰），加上 §1.1 的
// 「同码只留一条 / 按 `last_used_at` 倒序 / 备注是一条自由文本」。
//
// ⚠️ `BatchHistory` **不碰文件系统**（那是 `BatchHistoryStore` 的事），
//    所以这个文件里一个 URL 都不出现 —— 也因此它**不可能**碰到人类伙伴真实的
//    `~/Library/Application Support/BenagenDownloader/`（那条安全红线见
//    `BatchHistoryStoreTests` / `JsonFileStoreTests` 的文件头）。
// ---------------------------------------------------------------------------

/// ISO 8601 → `Date`（**只给测试用**，生产侧的时间源一律由调用方给）。
///
/// ⚠️ 用字面量时间戳当夹具、不调被测代码去生成：下面的期望值因此是**逐字钉死**的
///    （`+08:00` 的偏移与"秒级"这两条形状，只有拿字面量比才判得出来）。
private func at(_ iso: String) -> Date {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    guard let d = f.date(from: iso) else {
        Issue.record("夹具不是合法 ISO8601：\(iso)")
        return Date(timeIntervalSince1970: 0)
    }
    return d
}

private let t0900 = "2026-09-18T09:00:00+08:00"
private let t1030 = "2026-09-18T10:30:00+08:00"
private let t1100 = "2026-09-18T11:00:00+08:00"

// ---------------------------------------------------------------------------
// 增 / 去重 / 排序
// ---------------------------------------------------------------------------

@Test func theSameCodeIsUpdatedNotDuplicated() {
    // 规格 §1.1：`code` 是**唯一键**，同码只留一条，再次使用 ⇒ 更新 `last_used_at`。
    let h = BatchHistory.empty
        .recording(code: "AAA-1", at: at(t0900))
        .recording(code: "AAA-1", at: at(t1030))

    #expect(h.entries.count == 1, "同一个码加两次只该有一条")
    #expect(h.entries.first?.code == "AAA-1")
    #expect(h.entries.first?.lastUsedAt == t1030, "留下的必须是**后一次**的时间")
}

@Test func entriesAreOrderedByLastUseDescending() {
    // 规格 §1.1：按 `last_used_at` 倒序（最近用的在最上面）。
    let h = BatchHistory.empty
        .recording(code: "AAA-1", at: at(t0900))
        .recording(code: "BBB-2", at: at(t1100))
        .recording(code: "CCC-3", at: at(t1030))

    #expect(h.entries.map(\.code) == ["BBB-2", "CCC-3", "AAA-1"])
}

@Test func reUsingAnOldCodeMovesItToTheTop() {
    // 这一条与上一条不同：上一条测的是"插入顺序恰好就是时间顺序"，
    // 这一条测的是**重新使用一个旧码**会不会把它挪到最前（历史列表的"最近使用"语义）。
    let h = BatchHistory.empty
        .recording(code: "AAA-1", at: at(t0900))
        .recording(code: "BBB-2", at: at(t1030))
        .recording(code: "AAA-1", at: at(t1100))

    #expect(h.entries.map(\.code) == ["AAA-1", "BBB-2"])
}

@Test func parseOrdersEntriesByLastUseDescending() {
    // 文件是可以被手改的（也可以被"未来某个版本"写成别的顺序），所以**解析**也要归一化，
    // 不能只在 `recording` 那条路上维持顺序。
    let h = BatchHistory.parse(#"""
    {"version":1,"entries":[
      {"code":"AAA-1","note":"","base_url":"","last_used_at":"2026-09-18T09:00:00+08:00"},
      {"code":"BBB-2","note":"","base_url":"","last_used_at":"2026-09-18T11:00:00+08:00"},
      {"code":"CCC-3","note":"","base_url":"","last_used_at":"2026-09-18T10:30:00+08:00"}]}
    """#)

    #expect(h.entries.map(\.code) == ["BBB-2", "CCC-3", "AAA-1"])
}

@Test func mostRecentIsTheNewestEntryAndNilWhenEmpty() {
    #expect(BatchHistory.empty.mostRecent == nil)
    #expect(BatchHistory.empty.isEmpty)

    let h = BatchHistory.empty
        .recording(code: "AAA-1", at: at(t0900))
        .recording(code: "BBB-2", at: at(t1100))
    #expect(h.mostRecent?.code == "BBB-2")
    #expect(!h.isEmpty)
}

// ---------------------------------------------------------------------------
// 上限淘汰（E-3）
// ---------------------------------------------------------------------------

@Test func theHistoryKeepsAtMostFiftyEntries() {
    // E-3：条数有**硬上限 50**，无限增长的文件是留给下一个人踩的雷。
    var h = BatchHistory.empty
    for i in 0..<51 {
        h = h.recording(code: "code-\(i)", at: at(t0900).addingTimeInterval(Double(i)))
    }

    #expect(BatchHistory.maximumEntries == 50, "上限是 50（规格 §1.1 明写）")
    #expect(h.entries.count == 50, "加到 51 条只该剩 50")
    #expect(!h.entries.contains { $0.code == "code-0" }, "被淘汰的必须是**最久未用**的那条")
    #expect(h.entries.first?.code == "code-50", "最近用的还在最前")
    #expect(h.entries.contains { $0.code == "code-1" })
}

@Test func theEvictedEntryIsTheLeastRecentlyUsedNotTheOldestInserted() {
    // ⚠️ 这一条才是"最久未用（LRU）"的判据：上面那条里"插入最早"恰好等于"最久未用"，
    //    换成 FIFO 淘汰也能过。这里先把 `code-0` **再用一次**（它变成最新），
    //    于是最久未用的换成了 `code-1` —— 只有这样才分得开 LRU 与 FIFO。
    var h = BatchHistory.empty
    for i in 0..<50 {
        h = h.recording(code: "code-\(i)", at: at(t0900).addingTimeInterval(Double(i)))
    }
    h = h.recording(code: "code-0", at: at(t0900).addingTimeInterval(100))   // 重新使用 code-0
    h = h.recording(code: "code-50", at: at(t0900).addingTimeInterval(200))  // 第 51 条

    #expect(h.entries.count == 50)
    #expect(h.entries.contains { $0.code == "code-0" }, "刚用过的码不该被淘汰")
    #expect(!h.entries.contains { $0.code == "code-1" }, "被淘汰的是最久未用的那个")
}

// ---------------------------------------------------------------------------
// 解析容错（E-1）
// ---------------------------------------------------------------------------

@Test func garbageIsTreatedAsAnEmptyHistory() {
    // E-1（底线）：读不出来 / 解析失败 / `version` 不认识 ⇒ **当空历史**并继续，
    // 绝不抛异常、绝不让应用起不来。这个函数**没有 `throws`** —— 这正是判据：
    // 一旦有人把它改成会抛/会崩，这些行连编译都过不去。
    #expect(BatchHistory.parse(Data()).isEmpty, "空文件")
    #expect(BatchHistory.parse("").isEmpty, "空串")
    #expect(BatchHistory.parse("not json at all").isEmpty, "非 JSON")
    #expect(BatchHistory.parse("[1,2,3]").isEmpty, "顶层不是对象")
    #expect(BatchHistory.parse(#"{"version":2,"entries":[{"code":"AAA-1","last_used_at":"2026-09-18T09:00:00+08:00"}]}"#).isEmpty,
            "version 是**别的数** ⇒ 不认")
    #expect(BatchHistory.parse(#"{"entries":[]}"#).isEmpty, "没有 version ⇒ 不认")
    #expect(BatchHistory.parse(#"{"version":"1","entries":[]}"#).isEmpty, "version 不是数 ⇒ 不认")
    #expect(BatchHistory.parse(#"{"version":1,"entries":{}}"#).isEmpty, "entries 不是数组")
    #expect(BatchHistory.parse(#"{"version":1,"entries":"AAA-1"}"#).isEmpty, "entries 是串")
    #expect(BatchHistory.parse(#"{"version":1}"#).isEmpty, "没有 entries")
    #expect(BatchHistory.parse(#"{"version":1,"entries":[1,2,3]}"#).isEmpty, "行不是对象")
}

@Test func aMalformedRowIsDroppedWithoutLosingTheRest() {
    // 逐行容错：一行坏了不该把整份历史清掉（那会让用户"重启后什么都没了"）。
    // `code` 是唯一键 —— 没有码的行留不下来（它自己就是主键）。
    let h = BatchHistory.parse(#"""
    {"version":1,"entries":[
      {"code":"AAA-1","note":"好的那条","base_url":"http://dl.example","last_used_at":"2026-09-18T09:00:00+08:00"},
      {"note":"没有 code","last_used_at":"2026-09-18T10:00:00+08:00"},
      {"code":"","note":"空 code","last_used_at":"2026-09-18T10:00:00+08:00"},
      {"code":"BBB-2","last_used_at":"2026-09-18T11:00:00+08:00"}]}
    """#)

    #expect(h.entries.map(\.code) == ["BBB-2", "AAA-1"], "坏行丢掉、好行留下")
    #expect(h.entries.last?.note == "好的那条")
}

@Test func anUnparseableTimestampSortsLastAndKeepsTheEntry() {
    // 时间是**排序键**，不是主键：解析不出来的那条仍然是一条可用的码，
    // 只是它排在最末（最先被淘汰的位置），而不是让整份历史作废。
    let h = BatchHistory(entries: [
        .init(code: "AAA-1", lastUsedAt: "不是时间"),
        .init(code: "BBB-2", lastUsedAt: t1100),
    ])

    #expect(h.entries.map(\.code) == ["BBB-2", "AAA-1"])
}

// ---------------------------------------------------------------------------
// 往返（序列化 ⇄ 解析）
// ---------------------------------------------------------------------------

@Test func parseOfSerializeIsIdentity() throws {
    // 往返必须恒等 —— 包括备注为空、`base_url` 为空这两种"可选字段缺失"的情形
    // （它们**照样要写出去**，见 theFileShapeUsesTheSpecifiedFieldNames）。
    let h = BatchHistory(entries: [
        .init(code: "KUCwdl7d5u.UovnpW_D7", note: "客户张三 / 9月肿瘤数据",
              baseURL: "http://dl.example", lastUsedAt: t0900),
        .init(code: "BBB-2", note: "", baseURL: "", lastUsedAt: t1030),
    ])

    #expect(BatchHistory.parse(try h.serialized()) == h)
}

@Test func theFileShapeUsesTheSpecifiedFieldNames() throws {
    // 规格 §1.1 的字段名是**逐字**的：`version` / `entries` / `code` / `note` /
    // `base_url` / `last_used_at`。写成 camelCase 会让"下一个读这个文件的人"
    // （包括未来版本的壳）读到一份对不上的东西。
    let h = BatchHistory(entries: [
        .init(code: "AAA-1", note: "备注", baseURL: "http://dl.example", lastUsedAt: t0900),
    ])
    let text = String(decoding: try h.serialized(), as: UTF8.self)

    #expect(text.contains("\"version\""))
    #expect(text.contains("\"entries\""))
    #expect(text.contains("\"code\""))
    #expect(text.contains("\"note\""))
    #expect(text.contains("\"base_url\""))
    #expect(text.contains("\"last_used_at\""))
    #expect(!text.contains("baseUrl"), "键名是线上形状的 snake_case")
    #expect(text.contains("http://dl.example"), "URL 里的 / 不得被转义成 \\/")
}

@Test func theTimestampIsTheSameShapeAsTheManifestCreatedAt() {
    // 规格 §1.1：`last_used_at` 与交付清单的 `created_at` **同形**，
    // 直接复用 `TimestampPresentation` 显示 —— 不另造一套格式化。
    // 判据就是"既有的那个呈现函数认得它"（不认得时它会**原样返回**）。
    let h = BatchHistory.empty.recording(code: "AAA-1", at: at(t1030))
    let raw = h.entries.first?.lastUsedAt ?? ""

    #expect(raw == t1030, "带 +08:00、秒级")
    #expect(TimestampPresentation.text(raw) == "2026-09-18 10:30",
            "必须能被既有的 TimestampPresentation 读懂；时间戳的格式只有那一份口径")
}

@Test func timestampsUseTheFixedDeliveryServerOffsetNotTheMachineTimeZone() {
    // ⚠️ 有意偏离（E-8，理由写在这里）：写出去的时间戳固定用 **+08:00**（交付服务器的形状），
    //    不是本机时区。理由与 `TimestampPresentation` 丢掉偏移的理由是同一条：
    //    同一份历史在不同时区的机器上必须显示同一个时间 —— 而 `TimestampPresentation`
    //    读的是**墙上时间**，用本机时区写会让同一个文件在两个时区读出两个时间。
    //    （若改成 `.current`，上面那条 `raw == t1030` 立刻变红。）
    let h = BatchHistory.empty.recording(code: "AAA-1", at: at("2026-09-18T00:30:00Z"))
    #expect(h.entries.first?.lastUsedAt == "2026-09-18T08:30:00+08:00")
}

// ---------------------------------------------------------------------------
// 备注（一条自由文本）
// ---------------------------------------------------------------------------

@Test func theNoteSurvivesTheNextUseOfTheSameCode() {
    // 规格 §1.1「再次使用 ⇒ 更新 `last_used_at` 与可选字段」——
    // ⚠️ 备注**不在**被更新的那批里：用户写下的"客户张三"不该因为又用了一次这个码而消失。
    //    这正是 `recording(note:)` 默认 `nil`（= 保持原样）的理由。
    let h = BatchHistory.empty
        .recording(code: "AAA-1", at: at(t0900))
        .settingNote("客户张三", forCode: "AAA-1")
        .recording(code: "AAA-1", baseURL: "http://dl.example", at: at(t1030))

    #expect(h.entries.count == 1)
    #expect(h.entries.first?.note == "客户张三")
    #expect(h.entries.first?.baseURL == "http://dl.example", "base_url 是会被更新的那一批")
    #expect(h.entries.first?.lastUsedAt == t1030)
}

@Test func notesAreTrimmedToASingleLine() {
    // 备注是"**一句**自由文本"（人类伙伴选定），所以换行折成空格、首尾空白去掉；
    // 内部原有的空格**一个都不动**（那是他自己写的排版，壳不替他重排）。
    let h = BatchHistory.empty.recording(code: "AAA-1", at: at(t0900))
    func note(_ raw: String) -> String {
        h.settingNote(raw, forCode: "AAA-1").entries.first?.note ?? "（没这条）"
    }

    #expect(note("  客户张三 / 9月肿瘤数据  ") == "客户张三 / 9月肿瘤数据")
    #expect(note("  多行\n备注  ") == "多行 备注")
    #expect(note("第一行\r\n第二行") == "第一行 第二行")
    #expect(note("带\t制表符") == "带 制表符")
    #expect(note("A  /  B") == "A  /  B", "内部原有的空格一个都不动")
    #expect(note("") == "", "空串 = 没写备注（合法值）")
    #expect(note("   ") == "", "只有空白 = 没写备注")
}

@Test func settingANoteOnACodeThatIsNotInTheHistoryDoesNothing() {
    // 清空备注是「设成空串」，**不是**"删掉这一条"；而对一条不存在的记录设备注
    // 也不该凭空造出一条没有时间戳的记录（历史列表只对屏上那些行提供编辑）。
    let h = BatchHistory.empty.recording(code: "AAA-1", at: at(t0900))

    #expect(h.settingNote("备注", forCode: "ZZZ-9") == h)
    #expect(h.settingNote("", forCode: "AAA-1").entries.first?.note == "")
}

@Test func anEmptyCodeIsNeverRecorded() {
    // 空码不是一条交付批次：它进历史只会变成一个点了没反应的假条目。
    #expect(BatchHistory.empty.recording(code: "", at: at(t0900)).isEmpty)
    #expect(BatchHistory.empty.recording(code: "", baseURL: "http://dl.example", at: at(t0900)).isEmpty)
}

@Test func theDefaultServerIsTheEmptyBaseURL() {
    // 规格 §1.1：`base_url` 空串 = **默认服务器**（下一次自动加载时**不带**这个键，
    // 由内核用自己的默认值 —— 与 E-5 同一条纪律）。
    let entry = BatchHistoryEntry(code: "AAA-1", lastUsedAt: t0900)
    #expect(entry.baseURLOrNil == nil)

    let explicit = BatchHistoryEntry(code: "AAA-1", baseURL: "http://dl.example", lastUsedAt: t0900)
    #expect(explicit.baseURLOrNil == "http://dl.example")
}
