import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 九类错误卡（T7 / 规格 W2；样板 `windows/shell-core/src/presentation/error_card.rs`）。
//
// 重点不是"有九条"，而是**每一条在两种语言下都真的有三句话、英文里没有汉字、
// 严重度两两不同、引擎类排在最后、次数只进标题**。
// ---------------------------------------------------------------------------

private func hasCJK(_ s: String) -> Bool {
    s.unicodeScalars.contains { (0x4E00...0x9FFF).contains(Int($0.value)) }
}

// MARK: - 线上词

@Test func allNineWireWordsAreTheOnesTheCoreSends() {
    // 第九类 `name_invalid` 是 209e0236 加的（Windows 建不出来的文件名）；
    // 词表里少了它，内核发来这类错误时卡片会整类消失，所以这里按逐字集合钉死。
    #expect(Set(ErrorClass.allCases.map(\.wire)) == Set([
        "network_unreachable", "dns_failed", "server_refused", "disk_slow",
        "disk_full", "engine_unresponsive", "engine_crashed", "verify_mismatch",
        "name_invalid",
    ]))
    #expect(ErrorClass.ofWire("name_invalid") == .nameInvalid)
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
    // 第九类（209e0236）：Windows 建不出来的文件名。
    #expect(ErrorClass.nameInvalid.title(.zh) == "有文件名在 Windows 上建不出来")
    #expect(ErrorClass.nameInvalid.explain(.zh)
            == "清单里有名字带了 Windows 不允许的字符（例如冒号），系统建不出这些文件——文件本身是好的。")
    #expect(ErrorClass.nameInvalid.action(.zh) == "联系交付方改名，或改用 macOS / Linux 客户端。")
}

@Test func titlesExplainsAndActionsAreAllDistinct() {
    // 判别力：复制粘贴时漏改一条 switch 分支，逐条等值断言可能只有一条红；这条抓"映射塌陷"。
    // 九类的标题/解释必须两两不同（`name_invalid` 那三句也是独立的，不许和别的类撞车）。
    #expect(ErrorClass.allCases.count == 9)
    #expect(Set(ErrorClass.allCases.map { $0.title(.zh) }).count == 9)
    #expect(Set(ErrorClass.allCases.map { $0.explain(.zh) }).count == 9)
    #expect(Set(ErrorClass.allCases.map { $0.title(.en) }).count == 9)
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
    // 与内核 `Class::severity` 同序：60/59/50/40/30/20/19/18/28。
    // `name_invalid` 声明在最后，severity 28 落在 disk_full(30) 与 engine_unresponsive(20) 之间；
    // 顺序必须逐格对齐 `windows/shell-core/src/presentation/error_card.rs::ClassKind::ALL`。
    #expect(ErrorClass.allCases.map(\.severity) == [60, 59, 50, 40, 30, 20, 19, 18, 28])
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
