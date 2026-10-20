import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 自动重启的进度提示（T7 / 规格 W3；样板
// `windows/shell-core/src/presentation/restart_notice.rs`）。
//
// 四档状态里只有三档有话；`idle` 与任何不认识的状态都必须 **nil**（不显示）。
// 中英文句子与 Windows 那份逐字一致 —— 两个壳说的是同一件事。
// ---------------------------------------------------------------------------

@Test func waitingCarriesTheAttemptAndBudgetInBothLanguages() {
    let zh = RestartNotice.of(state: "waiting", attempt: 2, budget: 4, lang: .zh)
    #expect(zh?.text == "正在重启下载引擎（第 2/4 次）…")

    let en = RestartNotice.of(state: "waiting", attempt: 2, budget: 4, lang: .en)
    #expect(en?.text == "Restarting the download engine (attempt 2/4)…")
}

@Test func waitingUsesTheRealNumbersNotAFixedSentence() {
    // 判别力：写死成"第 1/3 次"的实现在上面那条也会绿（夹具碰巧是 2/4 时才红）。
    let notice = RestartNotice.of(state: "waiting", attempt: 9, budget: 12, lang: .zh)
    #expect(notice?.text == "正在重启下载引擎（第 9/12 次）…")
}

@Test func verifyingAndExhaustedHaveTheirOwnSentences() {
    #expect(RestartNotice.of(state: "verifying", attempt: 1, budget: 3, lang: .zh)?.text
            == "引擎已重启，正在确认它能正常工作…")
    #expect(RestartNotice.of(state: "verifying", attempt: 1, budget: 3, lang: .en)?.text
            == "The engine restarted - checking that it works…")

    #expect(RestartNotice.of(state: "exhausted", attempt: 3, budget: 3, lang: .zh)?.text
            == "自动重启没能恢复下载引擎，请手动点「重试」")
    #expect(RestartNotice.of(state: "exhausted", attempt: 3, budget: 3, lang: .en)?.text
            == "Automatic restarts could not recover the download engine - please click Retry")
}

@Test func idleMeansNoNotice() {
    // 没在重启就没有提示 —— 不是"显示一句空话"。
    #expect(RestartNotice.of(state: "idle", attempt: 0, budget: 3, lang: .zh) == nil)
    #expect(RestartNotice.of(state: "idle", attempt: 0, budget: 3, lang: .en) == nil)
}

@Test func unknownStateMeansNoNotice() {
    // 不认识的状态：说"正在重启"是撒谎，说"重启失败"更糟（那是 exhausted）。
    #expect(RestartNotice.of(state: "starting", attempt: 1, budget: 3, lang: .zh) == nil)
    #expect(RestartNotice.of(state: "", attempt: 1, budget: 3, lang: .zh) == nil)
    #expect(RestartNotice.of(state: "WAITING", attempt: 1, budget: 3, lang: .en) == nil)
}

@Test func theThreeVisibleStatesDifferInBothLanguages() {
    // 判别力：若把某个分支的文案复制粘贴成另一个分支，逐条断言里可能只有一条红。
    for lang in Lang.allCases {
        let texts = ["waiting", "verifying", "exhausted"].compactMap {
            RestartNotice.of(state: $0, attempt: 1, budget: 3, lang: lang)?.text
        }
        #expect(Set(texts).count == 3, "三档重启提示不许有重复句子（\(lang)）")
    }
}
