import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 壳侧文案的双语表（T7 / 规格 I-1、I-2；样板 `windows/shell-core/src/i18n.rs`）。
//
// 这套断言要能抓住的错：
//   · 漏写一条 ⇒ 编译错误（两张 `switch` 已经把这条路堵死，这里补值层面的断言）；
//   · 英文列里混进汉字 ⇒ `theEnglishTableHasNoCJK`；
//   · 中文列被人"顺手润色" ⇒ 逐字钉住既有客户看到的句子；
//   · `system` 漏进壳 ⇒ `fromHello` 只回 zh/en。
// ---------------------------------------------------------------------------

/// 有没有汉字（U+4E00–U+9FFF，与 Windows `the_english_table_has_no_cjk` 同一区间）。
private func hasCJK(_ s: String) -> Bool {
    s.unicodeScalars.contains { (0x4E00...0x9FFF).contains(Int($0.value)) }
}

// MARK: - 握手回执 → 语言

@Test func fromHelloMapsTheSixKnownShapes() {
    // 与内核 / Windows 同一张映射：只有英文的三个线上写法进英文，其余一律中文。
    #expect(Lang.fromHello("en") == .en)
    #expect(Lang.fromHello("en-US") == .en)
    #expect(Lang.fromHello("en_US") == .en)
    #expect(Lang.fromHello("zh") == .zh)
    #expect(Lang.fromHello(nil) == .zh)          // 旧内核没这一格 ⇒ 维持中文
    #expect(Lang.fromHello("system") == .zh)      // 内核不会回这个词；真回了也不许透传
    #expect(Lang.fromHello("fr") == .zh)          // 不认识 ⇒ 默认中文，不是英文
}

@Test func resolvedLanguageIsNeverSystem() {
    // `system` 必须在内核那侧被解析掉；壳这边连"可能是 system"的形状都没有。
    #expect(Lang.allCases.map(\.wire) == ["zh", "en"])
    #expect(!Lang.allCases.map(\.wire).contains("system"))
}

@Test func fromHelloTrimsSurroundingWhitespace() {
    // 线上 JSON 不会带空白，但"多一个空格就退回中文"是不可接受的静默降级。
    #expect(Lang.fromHello(" en-US ") == .en)
}

// MARK: - 两张表

@Test func everyKeyHasBothLanguages() {
    for key in Key.allCases {
        #expect(!Lang.zh.t(key).isEmpty, "中文缺：\(key)")
        #expect(!Lang.en.t(key).isEmpty, "英文缺：\(key)")
    }
}

/// 「选项名不翻译」的那两条 —— 「中文」与「English」各自永远以自己的语言显示
/// （客户在英文界面里也要一眼认出中文那一项；Windows 侧的 `language_labels` 同一条规矩）。
/// 除此之外的每个键都必须中英不同、英文里不许有汉字。
private let properNounKeys: Set<Key> = [.languageChinese, .languageEnglish]

@Test func theEnglishTableHasNoCJK() {
    for key in Key.allCases where !properNounKeys.contains(key) {
        #expect(!hasCJK(Lang.en.t(key)), "英文列混进汉字：\(key) => \(Lang.en.t(key))")
    }
}

@Test func theChineseTableKeepsTheExistingCustomerCopyVerbatim() {
    // 这些句子是 0.2.x 客户已经看过的原文 —— 一个字符都不许改。
    #expect(Lang.zh.t(.engineConnecting) == "正在连接内核…")
    #expect(Lang.zh.t(.engineNotStarted) == "引擎未启动")
    #expect(Lang.zh.t(.engineRunning) == "运行中")
    #expect(Lang.zh.t(.engineUnavailablePrefix) == "引擎不可用：{reason}")
    #expect(Lang.zh.t(.engineHandshakeTimeout) == "内核无响应（等待超过 5 秒）")
    #expect(Lang.zh.t(.retry) == "重试")
    #expect(Lang.zh.t(.restartWaiting) == "正在重启下载引擎（第 {attempt}/{budget} 次）…")
    #expect(Lang.zh.t(.restartVerifying) == "引擎已重启，正在确认它能正常工作…")
    #expect(Lang.zh.t(.restartExhausted) == "自动重启没能恢复下载引擎，请手动点「重试」")
}

@Test func theEnglishTablePinsTheRestartSentences() {
    // 与 `windows/shell-core/src/presentation/restart_notice.rs` 的英文列逐字相同。
    #expect(Lang.en.t(.restartWaiting) == "Restarting the download engine (attempt {attempt}/{budget})…")
    #expect(Lang.en.t(.restartVerifying) == "The engine restarted - checking that it works…")
    #expect(Lang.en.t(.restartExhausted)
            == "Automatic restarts could not recover the download engine - please click Retry")
}

@Test func tfFillsNamedSlots() {
    #expect(Lang.zh.tf(.restartWaiting, ["attempt": "3", "budget": "5"])
            == "正在重启下载引擎（第 3/5 次）…")
    #expect(Lang.en.tf(.restartWaiting, ["attempt": "3", "budget": "5"])
            == "Restarting the download engine (attempt 3/5)…")
    #expect(Lang.zh.tf(.engineUnavailablePrefix, ["reason": "内核崩了"]) == "引擎不可用：内核崩了")
    #expect(Lang.en.tf(.engineUnavailablePrefix, ["reason": "core crashed"])
            == "Engine unavailable: core crashed")
}

@Test func tfLeavesUnknownPlaceholdersAlone() {
    // 漏填是可测的（占位符还看得见）；静默删掉会让一句话缺一块而无迹可查。
    #expect(Lang.zh.tf(.restartWaiting, [:]) == "正在重启下载引擎（第 {attempt}/{budget} 次）…")
}

@Test func everyKeyDiffersBetweenLanguages() {
    // 判别力：若有人把英文分支 return 成中文（或反过来），逐条非空断言仍可能全绿。
    for key in Key.allCases where key != .engineUnavailablePrefix && !properNounKeys.contains(key) {
        #expect(Lang.zh.t(key) != Lang.en.t(key), "中英两列相同：\(key)")
    }
}

@Test func engineStatusFollowsTheResolvedLanguage() {
    // 徽标 / 设置页那句同样读 `model.language`（视图只传值，映射在 Presentation 层）。
    #expect(EngineStatusPresentation.text(for: .notStarted, lang: .en) == "Engine not started")
    #expect(EngineStatusPresentation.text(for: .unavailable("core crashed"), lang: .en)
            == "Engine unavailable: core crashed")
    #expect(EngineStatusPresentation.text(for: .unavailable(AppModel.handshakeTimeoutMessage),
                                           lang: .en)
            == "The core is not responding (waited more than 5 seconds)")
    // 默认参数仍然是中文 —— 既有调用点与既有测试不用改。
    #expect(EngineStatusPresentation.text(for: .notStarted) == "引擎未启动")
}
