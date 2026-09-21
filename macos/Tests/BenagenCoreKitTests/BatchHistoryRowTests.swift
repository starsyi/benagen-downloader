import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 换码面板里那个历史列表的**行文本**（阶段 E 规格 §1.4）
//
// 全局约束 8：视图里不许有非渲染逻辑，所以"这一行画出来是什么字"必须在这里定死并且可断言
// —— 视图（`SwitchDeliverySheet`）只做绑定与渲染分派，它**一个字符串都不拼**。
//
// 这一份钉住的是四件事，每一件都有判别力（写错了下面就会有红）：
//   ① **备注为空时回落到码**（规格 §1.4 明写："有则显示，无则显示码"）；
//   ② **时间列复用既有的 `TimestampPresentation`**，不另造一套格式化
//      （规格 §1.1：`last_used_at` 与清单的 `created_at` 同形，`+08:00` 同形）；
//   ③ **顺序就是历史给的顺序**（`BatchHistory` 是不变量的持有者，行层不重排 —— 重排一次
//      就是同一条规则写两遍，两处迟早分叉）；
//   ④ **`base_url` 跟着行走**（点一行要拿它去问**对的**交付服务器，见 `baseURLOrNil` 的注释）。
// ---------------------------------------------------------------------------

/// 造一条历史条目。默认值取"刚用过的那一条"的样子（规格 §1.1 的示例码）。
private func entry(code: String,
                   note: String = "",
                   baseURL: String = "",
                   at lastUsedAt: String = "2026-09-18T09:12:00+08:00") -> BatchHistoryEntry {
    BatchHistoryEntry(code: code, note: note, baseURL: baseURL, lastUsedAt: lastUsedAt)
}

// MARK: - ① 标题：备注为空时回落到码

@Test func theTitleShowsTheNoteWhenThereIsOne() {
    let row = BatchHistoryRow(entry(code: "KUCwdl7d5u.UovnpW_D7", note: "客户张三 / 9月肿瘤数据"))
    #expect(row.title == "客户张三 / 9月肿瘤数据")
    #expect(row.note == "客户张三 / 9月肿瘤数据")
}

@Test func theTitleFallsBackToTheCodeWhenThereIsNoNote() {
    let code = "KUCwdl7d5u.UovnpW_D7"
    let row = BatchHistoryRow(entry(code: code))
    #expect(row.title == code, "没写备注的那一行，标题就是码本身（规格 §1.4）")
    #expect(row.note.isEmpty)
}

/// 只有空白字符的备注**与空串同等对待**。
///
/// 为什么要有这条：一行"看起来是空的"标题在界面上就是**什么都没说**（约束 4 明禁的静默失效）
/// —— 用户会以为这一行坏了，而不是以为"我还没写备注"。手改过的 `history.json` 里
/// `"note": "   "` 是能读进来的（`BatchHistory.parse` 只要求 `code` 非空），所以这条不是假想。
@Test func aWhitespaceOnlyNoteCountsAsNoNote() {
    let code = "KUCwdl7d5u.UovnpW_D7"
    let row = BatchHistoryRow(entry(code: code, note: "   \n\t "))
    #expect(row.title == code)
    #expect(row.note.isEmpty, "交出去的备注也是空的：编辑框里不该躺着一串看不见的空白")
}

@Test func theNoteIsTrimmedButItsInnerSpacingIsKept() {
    // 内部原有的空格一个都不动（那是用户自己写的排版，同 `BatchHistory.normalizedNote`）。
    let row = BatchHistoryRow(entry(code: "C", note: "  客户 张三  "))
    #expect(row.note == "客户 张三")
    #expect(row.title == "客户 张三")
}

// MARK: - ② 时间列：复用既有的 TimestampPresentation

/// ⚠️ 函数名与 `BrowserRowTests` 里那条**必须不同**：swift-testing 的 `@Test func` 在文件
///    作用域里就是普通全局函数，重名是**编译错误**（本文件第一版就撞上了那一条）。
@Test func theHistoryTimeColumnReusesTheSharedTimestampPresentation() {
    let raw = "2026-09-18T09:12:00+08:00"
    let row = BatchHistoryRow(entry(code: "C", at: raw))
    // ① 关系式：与既有的那一份**是同一个答案**（不另造格式化，规格 §1.1）。
    #expect(row.timeText == TimestampPresentation.text(raw))
    // ② 字面量：把形状本身钉死（`+08:00` 同形、秒被丢掉）。
    #expect(row.timeText == "2026-09-18 09:12")
}

@Test func aFractionalSecondTimestampIsAlsoUnderstood() {
    // 清单里的 `created_at` 实测带小数秒；两种都要认（`BatchHistory.usedAt` 同一条口径）。
    let row = BatchHistoryRow(entry(code: "C", at: "2026-09-18T09:12:00.805751+08:00"))
    #expect(row.timeText == "2026-09-18 09:12")
}

@Test func aRowWithoutATimestampShowsTheNoValueGlyph() {
    // 空串 ⇒ 占位符。**不是**空着：空串在界面上就是"这一格没有值"而没说出口
    // （口径同 `SourceTimeText.noValue` / `DeliverySummary.noValue`，同一个字形 `—`）。
    #expect(BatchHistoryRow(entry(code: "C", at: "")).timeText == "—")
}

@Test func anUnparseableTimestampIsShownVerbatim() {
    // 非空但认不出来 ⇒ **原样**显示（`TimestampPresentation` 的既有契约），
    // 绝不显示 "Invalid Date" 这种壳自己编的文案（约束 C-7 / 3）。
    #expect(BatchHistoryRow(entry(code: "C", at: "待定")).timeText == "待定")
}

// MARK: - ③ 顺序：历史给什么顺序就画什么顺序

@Test func theRowsKeepTheHistorysOwnOrderNewestFirst() {
    // 故意**乱序传入**：`BatchHistory` 是不变量的持有者（它排倒序），行层只做映射。
    let history = BatchHistory(entries: [
        entry(code: "OLD", at: "2026-09-01T08:00:00+08:00"),
        entry(code: "NEW", at: "2026-09-18T09:12:00+08:00"),
        entry(code: "MID", at: "2026-09-10T12:00:00+08:00"),
    ])
    #expect(BatchHistoryRow.rows(history).map(\.code) == ["NEW", "MID", "OLD"])
}

@Test func everyEntryBecomesExactlyOneRowIdentifiedByItsCode() {
    let history = BatchHistory(entries: [entry(code: "A"), entry(code: "B")])
    let rows = BatchHistoryRow.rows(history)
    #expect(rows.count == history.entries.count)
    #expect(rows.map(\.id) == ["A", "B"], "码是唯一键 ⇒ 它就是这一行的身份")
}

@Test func anEmptyHistoryProducesNoRows() {
    // 视图靠这个空数组决定**整段不渲染**（规格 §1.4：列表为空时不显示这一段，不要空盒子）。
    #expect(BatchHistoryRow.rows(.empty).isEmpty)
}

// MARK: - 边界：码与备注**都**空的那种行

/// **纵深防御**（复审点名要核的第三条边界）。
///
/// 经 `BatchHistory.parse` 与 `rows(_:)` 这条正路**造不出**这一行：空码在
/// `BatchHistory.init(entries:)` 就被丢掉了（它不是一条交付批次）。但
/// `BatchHistoryRow.init` 与 `BatchHistoryEntry.init` **都是 `public`、都不过滤**
/// （`BatchHistoryRow.init(_ entry:)` 直接照抄条目里的字段；`BatchHistoryEntry.init`
/// 更是只往四个字段里赋值），所以"码空 + 备注空"这一行在类型上**是构造得出来的** ——
/// 而它的标题会是**空串**，在界面上与"这一行坏了"分不开（约束 4）。
/// ⚠️ 这段话原先自相矛盾（同一句里说 `BatchHistory.init(entries:)` "丢掉空码"、
/// 又说它"不过滤"）——**过滤空码的是 `BatchHistory.init(entries:)`，条目那条路才是不设防的**
/// （最终审查顺手 7）。
///
/// ⚠️ 兜底的字形（`—`）**与 `DeliverySummary` 的 `code` 那一格同源**
///    （`DeliverySummary.swift:39`：`info.code.isEmpty ? noValue : info.code`）——
///    同一个"这一格没有值"在壳里只有一种长相。
@Test func aRowWithNeitherNoteNorCodeStillSaysSomething() {
    let row = BatchHistoryRow(BatchHistoryEntry(code: "", note: "", lastUsedAt: ""))
    #expect(row.title == "—")
    #expect(!row.title.isEmpty)
    #expect(row.timeText == "—")
}

@Test func theRowsHelperCanNeverProduceAnEmptyTitledRow() {
    // 正路上的守卫在 `BatchHistory` 那边（空码进不去历史）。
    // 这条用例钉住的是**两边的接口关系**：只要走 `rows(_:)`，标题就不可能空。
    let history = BatchHistory(entries: [entry(code: ""), entry(code: "REAL")])
    #expect(history.entries.map(\.code) == ["REAL"], "空码不是一条交付批次（`BatchHistory.init` 丢掉它）")
    #expect(BatchHistoryRow.rows(history).allSatisfy { !$0.title.isEmpty })
}

// MARK: - ④ 交付服务器：点一行要问对的那一台

@Test func eachRowCarriesTheServerItWasLoadedFrom() {
    let fromCustom = BatchHistoryRow(entry(code: "C", baseURL: "http://delivery.example.com:8010"))
    #expect(fromCustom.baseURL == "http://delivery.example.com:8010")
    #expect(fromCustom.baseURLOrNil == "http://delivery.example.com:8010")
}

@Test func theDefaultServerIsCarriedAsNilSoTheKeyStaysOutOfTheRequest() {
    // 空串 ⇒ `nil` ⇒ 请求里**不出现** `base_url` 这个键（E-5 的同一条纪律：
    // 不要显式传一个"和默认一样"的值，那会在内核默认值变化时静默分叉）。
    let fromDefault = BatchHistoryRow(entry(code: "C", baseURL: ""))
    #expect(fromDefault.baseURL.isEmpty)
    #expect(fromDefault.baseURLOrNil == nil)
}

// MARK: - 长度上界：手改过的历史文件不得把内核的 FIFO 堵死

@Test func anOrdinaryCodeCanBeLoaded() {
    #expect(BatchHistoryRow(entry(code: "KUCwdl7d5u.UovnpW_D7")).isSendable)
}

@Test func anOverlongCodeFromAHandEditedHistoryFileCannotBeSent() {
    // `history.json` 是**用户看得见、改得动**的文件，`BatchHistory.parse` 只要求 `code` 非空。
    // 一条超过 2 KiB 的码点下去会把内核那条 FIFO 串行队列**永久堵死**
    // （约束 C-3：内核不报错、只把客户端静默堵死）—— 所以这一行必须发不出去。
    let huge = String(repeating: "A", count: DeliveryCodeEntry.maximumBytes + 1)
    #expect(BatchHistoryRow(entry(code: huge)).isSendable == false)
    // 边界：**正好等于**上界是能发的（`DeliveryCodeEntry.tooLong` 用的是 `>`）。
    let exactly = String(repeating: "A", count: DeliveryCodeEntry.maximumBytes)
    #expect(BatchHistoryRow(entry(code: exactly)).isSendable)
}
