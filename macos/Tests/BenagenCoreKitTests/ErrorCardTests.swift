import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 八类错误卡（T7 / 规格 W2；样板 `windows/shell-core/src/presentation/error_card.rs`）。
//
// 重点不是"有八条"，而是**每一条在两种语言下都真的有三句话、英文里没有汉字、
// 严重度两两不同、引擎类排在最后、次数只进标题**。
// ---------------------------------------------------------------------------

private func hasCJK(_ s: String) -> Bool {
    s.unicodeScalars.contains { (0x4E00...0x9FFF).contains(Int($0.value)) }
}

// MARK: - 线上词

@Test func allEightWireWordsAreTheOnesTheCoreSends() {
    #expect(Set(ErrorClass.allCases.map(\.wire)) == Set([
        "network_unreachable", "dns_failed", "server_refused", "disk_slow",
        "disk_full", "engine_unresponsive", "engine_crashed", "verify_mismatch",
    ]))
}

@Test func unknownWireWordsMapToNil() {
    // 不认识就**不猜**：界面上按"意料之外的错误"处置，不许编一句不相干的解释。
    #expect(ErrorClass.ofWire("network_unreachable") == .networkUnreachable)
    #expect(ErrorClass.ofWire(nil) == nil)
    #expect(ErrorClass.ofWire("") == nil)
    #expect(ErrorClass.ofWire("NETWORK_UNREACHABLE") == nil)
    #expect(ErrorClass.ofWire("network_unreachable ") == nil)
    #expect(ErrorClass.ofWire("something_new") == nil)
}

// MARK: - 三句话

@Test func everyClassHasAllThreeSentencesInBothLanguages() {
    for kind in ErrorClass.allCases {
        for lang in Lang.allCases {
            #expect(!kind.title(lang).isEmpty, "标题缺：\(kind) \(lang)")
            #expect(!kind.explain(lang).isEmpty, "解释缺：\(kind) \(lang)")
            #expect(!kind.action(lang).isEmpty, "动作缺：\(kind) \(lang)")
        }
    }
}

@Test func noCJKInTheEnglishCards() {
    for kind in ErrorClass.allCases {
        #expect(!hasCJK(kind.title(.en)), "英文标题混进汉字：\(kind) => \(kind.title(.en))")
        #expect(!hasCJK(kind.explain(.en)), "英文解释混进汉字：\(kind) => \(kind.explain(.en))")
        #expect(!hasCJK(kind.action(.en)), "英文动作混进汉字：\(kind) => \(kind.action(.en))")
    }
}

@Test func theChineseCardsKeepTheWindowsCopyVerbatim() {
    // 中文逐字照 `windows/shell-core/src/presentation/error_card.rs`。
    #expect(ErrorClass.networkUnreachable.title(.zh) == "连不上服务器")
    #expect(ErrorClass.dnsFailed.title(.zh) == "域名解析失败")
    #expect(ErrorClass.diskFull.explain(.zh) == "目标磁盘放不下这次的数据，或者没有写权限。")
    #expect(ErrorClass.verifyMismatch.action(.zh) == "重新下载这个文件。")
}

@Test func titlesExplainsAndActionsAreAllDistinct() {
    // 判别力：复制粘贴时漏改一条 switch 分支，逐条等值断言可能只有一条红；这条抓"映射塌陷"。
    #expect(Set(ErrorClass.allCases.map { $0.title(.zh) }).count == 8)
    #expect(Set(ErrorClass.allCases.map { $0.explain(.zh) }).count == 8)
    #expect(Set(ErrorClass.allCases.map { $0.title(.en) }).count == 8)
}

// MARK: - 严重度

@Test func severitiesArePairwiseDistinct() {
    // 打平会让"同一秒里两类错误谁置顶"变成数组里碰巧谁在前 —— 不可断言、不可复现。
    let values = ErrorClass.allCases.map(\.severity)
    #expect(Set(values).count == values.count)
}

@Test func engineClassesSitAtTheBottom() {
    let engine = [ErrorClass.engineUnresponsive, .engineCrashed]
    let others = ErrorClass.allCases.filter { !engine.contains($0) && $0 != .verifyMismatch }
    for e in engine {
        for o in others {
            #expect(e.severity < o.severity, "\(e) 应排在 \(o) 之后（严重度更低）")
        }
        #expect(e.severity > ErrorClass.verifyMismatch.severity,
                "verify_mismatch 是最后一档")
    }
    #expect(ErrorClass.engineUnresponsive.severity > ErrorClass.engineCrashed.severity)
    #expect(ErrorClass.engineCrashed.severity > ErrorClass.verifyMismatch.severity)
}

@Test func theSeverityOrderMatchesTheCore() {
    // 与内核 `Class::severity` 同序：60/59/50/40/30/20/19/18。
    #expect(ErrorClass.allCases.map(\.severity) == [60, 59, 50, 40, 30, 20, 19, 18])
}

// MARK: - 次数只在标题里

@Test func aCountAboveOneAppearsExactlyOnceAndOnlyInTheTitle() {
    for lang in Lang.allCases {
        let card = ErrorCard.of(.dnsFailed, lang: lang, count: 3)
        let marker = lang == .zh ? "3 次" : "x3"
        #expect(card.count == 3)
        #expect(card.title.contains(marker))
        #expect(card.explain.contains(marker) == false)
        #expect(card.action.contains(marker) == false)
        // 只出现一次（`…（3 次）` 里就一处）。
        #expect(card.title.components(separatedBy: marker).count == 2)
        // 动作/解释与不带次数时逐字相同。
        #expect(card.explain == ErrorClass.dnsFailed.explain(lang))
        #expect(card.action == ErrorClass.dnsFailed.action(lang))
    }
}

@Test func aCountOfOneAddsNoSuffix() {
    for lang in Lang.allCases {
        let card = ErrorCard.of(.dnsFailed, lang: lang)
        #expect(card.count == 1)
        #expect(card.title == ErrorClass.dnsFailed.title(lang))
        #expect(!card.title.contains("次"))
        #expect(!card.title.contains("x1"))
    }
}

@Test func countIsClampedToOne() {
    let card = ErrorCard.of(.diskFull, lang: .zh, count: 0)
    #expect(card.count == 1)
    #expect(card.title == ErrorClass.diskFull.title(.zh))
}

@Test func theCardSeverityComesFromTheClass() {
    #expect(ErrorCard.of(.networkUnreachable, lang: .en).severity == 60)
    #expect(ErrorCard.of(.verifyMismatch, lang: .en).severity == 18)
}

// MARK: - 合并 / 排序 / 上限

@Test func sameClassIsMergedAndSortedBySeverityDescending() {
    let cards = ErrorCards.from([
        ErrorObservation(errorClass: .diskFull, count: 2),
        ErrorObservation(errorClass: .networkUnreachable, count: 1),
        ErrorObservation(errorClass: .dnsFailed, count: 4),
    ], lang: .zh)

    #expect(cards.map(\.errorClass) == [.networkUnreachable, .dnsFailed, .diskFull])
    #expect(cards.map(\.severity) == [60, 59, 30])
}

@Test func duplicateObservationsOfTheSameClassKeepTheLargestCount() {
    // 取 max 而不是相加：两条观察说的是"同一段时间里的同一类"。
    let cards = ErrorCards.from([
        ErrorObservation(errorClass: .dnsFailed, count: 2),
        ErrorObservation(errorClass: .dnsFailed, count: 5),
    ], lang: .zh)

    #expect(cards.count == 1)
    #expect(cards[0].count == 5)
    #expect(cards[0].title == "域名解析失败（5 次）")
}

@Test func atMostThreeCardsAreShown() {
    let cards = ErrorCards.from(ErrorClass.allCases.map {
        ErrorObservation(errorClass: $0, count: 1)
    }, lang: .en)

    #expect(cards.count == ErrorCards.maxOnScreen)
    #expect(cards.count == 3)
    // 留下的必须是最严重的那三张（网络 / DNS / 服务器拒绝）。
    #expect(cards.map(\.errorClass) == [.networkUnreachable, .dnsFailed, .serverRefused])
}

@Test func noObservationsMeansNoCards() {
    #expect(ErrorCards.from([], lang: .zh).isEmpty)
}

@Test func mergingKeepsTheObservedLanguage() {
    let en = ErrorCards.from([ErrorObservation(errorClass: .engineCrashed, count: 2)],
                             lang: .en)
    #expect(en.count == 1)
    #expect(en[0].title == "The download engine failed to start or exited (x2)")
    let zh = ErrorCards.from([ErrorObservation(errorClass: .engineCrashed, count: 2)],
                             lang: .zh)
    #expect(zh[0].title == "下载引擎启动失败或已退出（2 次）")
}
