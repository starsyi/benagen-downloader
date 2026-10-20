import Testing
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 只读装饰（T7）：语言从握手回执里来、错误卡 / 重启进度 / 解析进度从三条
// 只读命令来。**取不到就回空/保持上一帧，不升级成红条**（与 Windows
// `restart_progress` / `delivery_progress` 两条命令同款）。
//
// 夹具全部是内核会发的线上 JSON（键 snake_case），走真解码路径。
// ---------------------------------------------------------------------------

@MainActor
@Test func theLanguageComesFromTheHelloReply() async {
    let fake = FakeCore()
    fake.stub("hello", #"{"protocol":2,"min_split_size_choices":["1M"],"language":"en-US"}"#)
    fake.stub("get_settings", Wire.getSettings)
    let model = AppModel(client: fake)

    await model.start()

    #expect(model.language == .en)
}

@MainActor
@Test func anOldKernelWithoutALanguageFieldStaysChinese() async {
    // `Wire.hello` 是旧内核的形状（没有 `language` 那一格）—— 不许因此握手失败，
    // 也不许猜成英文。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    let model = AppModel(client: fake)

    await model.start()

    #expect(model.language == .zh)
}

@MainActor
@Test func errorCardsComeFromErrorRecentInTheKernelsOrder() async {
    let fake = FakeCore()
    fake.stub("hello", #"{"protocol":2,"min_split_size_choices":["1M"],"language":"en"}"#)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("error_recent", #"""
    {"entries":[
      {"class":"disk_full","severity":30,"count":2,"repeats":false},
      {"class":"network_unreachable","severity":60,"count":1,"repeats":false},
      {"class":"dns_failed","severity":59,"count":3,"repeats":true}
    ]}
    """#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshErrorCards()

    #expect(model.errorCards.map(\.errorClass) == [.networkUnreachable, .dnsFailed, .diskFull])
    #expect(model.errorCards.map(\.severity) == [60, 59, 30])
    // 语言是 `en` ⇒ 三句话都是英文；次数在标题里。
    #expect(model.errorCards[1].title == "Domain lookup failed (x3)")
    #expect(model.errorCards[1].count == 3)
    #expect(!model.errorCards[1].explain.contains("x3"))
}

@MainActor
@Test func anUnknownClassIsSkippedInsteadOfGuessed() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("error_recent", #"{"entries":[{"class":"future_class","count":9}]}"#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshErrorCards()

    #expect(model.errorCards.isEmpty, "不认识的 class 不许显示成任何一张卡")
}

@MainActor
@Test func aFailedErrorRecentKeepsThePreviousCards() async {
    // 内核重启的瞬间会短暂取不到 `error_recent` —— 没有理由清屏。
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("error_recent", #"{"entries":[{"class":"disk_slow","count":1}]}"#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshErrorCards()
    #expect(model.errorCards.map(\.errorClass) == [.diskSlow])

    fake.stub("error_recent", throwing: CoreError.transport("内核进程已退出（管道结束）"))
    await model.refreshErrorCards()
    #expect(model.errorCards.map(\.errorClass) == [.diskSlow], "取不到 ⇒ 保持上一帧")
    #expect(model.lastError == nil, "只读装饰的失败不许写 lastError（那是引擎横幅的事）")
}

@MainActor
@Test func restartProgressBecomesASentenceThroughTheResolvedLanguage() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("restart_progress", #"{"attempt":2,"budget":4,"state":"waiting","sinceMs":1200,"fails":1}"#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshRestartNotice()

    #expect(model.restartNotice?.text == "正在重启下载引擎（第 2/4 次）…")
}

@MainActor
@Test func restartProgressIdleMeansNoNoticeAndFailureClearsIt() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("restart_progress", #"{"attempt":1,"budget":3,"state":"verifying"}"#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshRestartNotice()
    #expect(model.restartNotice != nil)

    fake.stub("restart_progress", #"{"attempt":0,"budget":3,"state":"idle"}"#)
    await model.refreshRestartNotice()
    #expect(model.restartNotice == nil, "idle 不是一条提示")

    fake.stub("restart_progress", throwing: CoreError.transport("内核没了"))
    await model.refreshRestartNotice()
    #expect(model.restartNotice == nil, "取不到 ⇒ 回空，不升级成红条")
    #expect(model.lastError == nil)
}

@MainActor
@Test func deliveryProgressLinesAreShownVerbatimOnePerLine() async {
    // 每一行都是内核按当前语言成好的**整句**；壳不许拼接、不许换序、不许翻译。
    let fake = FakeCore()
    fake.stub("hello", #"{"protocol":2,"min_split_size_choices":["1M"],"language":"zh"}"#)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("delivery_progress", #"""
    {"state":"waiting","origin":"http://download.benagen.com","lines":[
      "正在连接交付服务器…",
      "已取到清单，共 12 个文件",
      "第二条 / 第二句 —— 原样"
    ]}
    """#)
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshDeliveryProgress()

    #expect(model.deliveryProgressLines == [
        "正在连接交付服务器…",
        "已取到清单，共 12 个文件",
        "第二条 / 第二句 —— 原样",
    ])
}

@MainActor
@Test func aFailedDeliveryProgressIsJustNoLines() async {
    let fake = FakeCore()
    fake.stub("hello", Wire.hello)
    fake.stub("get_settings", Wire.getSettings)
    fake.stub("delivery_progress", throwing: CoreError.transport("内核没了"))
    let model = AppModel(client: fake)

    await model.start()
    await model.refreshDeliveryProgress()

    #expect(model.deliveryProgressLines.isEmpty)
    #expect(model.lastError == nil)
}
