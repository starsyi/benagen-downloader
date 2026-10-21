import Foundation
import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 0.3.0「语言入口」的判据（macOS 壳曾整个缺这一块 —— 客户在 0.3.0 的 DMG 里
// 找不到任何切换语言的地方，这是补上它的同时钉住的四件事）：
//
//   1. 设置里**有** `language` 这一格，且**缺键时兜 `system`**（旧设置文件）；
//   2. 回传时**带上**这一格 —— 漏了它，内核整份替换就会把语言重置（客户改一次
//      并行数，界面语言悄悄变回中文）；
//   3. `hello` 回执缺 `language_chosen` ⇒ `false`（旧内核 ⇒ 弹一次首启窗）；
//   4. `SettingsForm` 是唯一组装点，语言必须跟着走完整趟。
// ---------------------------------------------------------------------------

@Test func settingsDecodeDefaultsTheLanguageToSystem() throws {
    // 0.2.x 内核回的 `settings` 里没有 `language`（那时还没有这一格）。
    let old = Data(#"{"parallel":8,"connections":16,"splits":16,"min_split_size":"20M","limit_mbps":0,"max_tries":3,"retry_wait":1}"#.utf8)
    let s = try CoreJSON.decoder.decode(Settings.self, from: old)
    #expect(s.language == "system")
    #expect(s.parallel == 8)
}

@Test func settingsEncodeCarriesTheLanguage() throws {
    // 反面：把 `language` 从 CodingKeys 里去掉（或忘记放进 `SettingsForm.settings`），
    // 编出来的 payload 里就没有这一格，内核会把语言重置 —— 这里必红。
    var s = Settings(parallel: 8, connections: 16, splits: 16, minSplitSize: "20M",
                     limitMbps: 0, maxTries: 3, retryWait: 1)
    s.language = "en"
    let body = try CoreJSON.requestEncoder.encode(s)
    let obj = try #require(try JSONSerialization.jsonObject(with: body) as? [String: Any])
    #expect(obj["language"] as? String == "en")
}

@Test func theFormKeepsTheLanguageThroughAdrawAndRoundTrip() throws {
    var s = Settings(parallel: 8, connections: 16, splits: 16, minSplitSize: "20M",
                     limitMbps: 0, maxTries: 3, retryWait: 1)
    s.language = "zh"
    var form = SettingsForm(settings: s, choices: ["20M"])
    #expect(form.language == "zh")
    // 用户只改了并行数：语言必须原样跟着回去（这是那个"整份替换"坑的正面判据）。
    form.parallel = 4
    #expect(form.settings.language == "zh")
    #expect(form.settings.parallel == 4)
}

@Test func theThreeLanguageChoicesAreTheWireWords() {
    // 顺序 = 界面顺序：跟随系统在前（默认），然后中文、English。
    #expect(SettingsForm.languageChoices == ["system", "zh", "en"])
}

@Test func helloWithoutLanguageChosenMeansNotChosenYet() throws {
    // 旧内核（0.2.x）没有 `language_chosen`：缺格必须当"没选过"，而不是让握手解码失败。
    let old = Data(#"{"protocol":1,"minSplitSizeChoices":["20M"],"language":"zh"}"#.utf8)
    let h = try CoreJSON.decoder.decode(HelloResult.self, from: old)
    #expect(h.languageChosen == false)
    #expect(h.language == "zh")
}

@Test func helloWithLanguageChosenReadsTheFlag() throws {
    let now = Data(#"{"protocol":2,"minSplitSizeChoices":["20M"],"language":"en","language_chosen":true}"#.utf8)
    let h = try CoreJSON.decoder.decode(HelloResult.self, from: now)
    #expect(h.languageChosen == true)
    #expect(h.language == "en")
}

@Test func theFirstRunSheetHasBothLanguages() {
    // 三句话都必须双语（英文一列不许有汉字 —— 与 `I18nTests.theEnglishTableHasNoCJK` 同一条纪律，
    // 这里只钉新增的那几个键真的存在且两种语言不同）。
    for key in [Key.languageTitle, .languageNote, .languageFollowSystem,
                .languageSheetTitle, .languageSheetNote] {
        #expect(Lang.zh.t(key) != Lang.en.t(key))
        #expect(!Lang.en.t(key).contains(where: { $0.unicodeScalars.contains { (0x4E00...0x9FFF).contains($0.value) } }))
    }
    // 选项名本身不翻译：中文永远显示「中文」，英文永远显示「English」。
    #expect(Lang.zh.t(.languageChinese) == Lang.en.t(.languageChinese))
    #expect(Lang.zh.t(.languageEnglish) == Lang.en.t(.languageEnglish))
}
