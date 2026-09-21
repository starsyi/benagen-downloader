import Testing
@testable import BenagenCoreKit

// 全局约束 8：`Presentation/` 里的每条纯函数都要有单测。
//
// ⚠️ 本文件的断言要求**有判别力**：把实现改坏时它必须真的红。
//    本项目已经栽过六次"断言存在但无判别力"（例如 `#expect(body.count == 7)` 对键名零判别力），
//    所以下面刻意避开了"只看形状/只看数量"的写法 ——
//    每条断言都钉住**具体的值**，并且额外有一条"四者互不相同"的断言去抓
//    "复制粘贴时漏改 switch 分支"这类错误（那种错误下逐个等值断言可能恰好都过）。

/// `#expect` 的宏展开对「取反 + `contains(where:)` + keypath」这种组合会报
/// `call can throw, but it is not marked with 'try'`（`rethrows` 推断的坑）。
/// 包成普通函数既能绕开它，也让断言读起来更直白。
private func hasNewline(_ s: String) -> Bool {
    s.contains(where: \.isNewline)
}

// MARK: - text(for:)

@Test func engineStatusTextPinsEveryCase() {
    #expect(EngineStatusPresentation.text(for: .unknown) == "正在连接内核…")
    #expect(EngineStatusPresentation.text(for: .notStarted) == "引擎未启动")
    #expect(EngineStatusPresentation.text(for: .running) == "运行中")
    #expect(EngineStatusPresentation.text(for: .unavailable("内核崩了")) == "引擎不可用：内核崩了")
}

@Test func engineStatusTextDistinguishesAllFourCases() {
    // 判别力：四个 case 的正文必须两两不同。若有人把某个分支 return 成另一个分支的字面量，
    // 逐条等值断言里可能只有一条红；这条会直接把"映射塌陷"抓出来。
    let all = [
        EngineStatusPresentation.text(for: .unknown),
        EngineStatusPresentation.text(for: .notStarted),
        EngineStatusPresentation.text(for: .running),
        EngineStatusPresentation.text(for: .unavailable("x")),
    ]
    #expect(Set(all).count == 4)
}

@Test func engineStatusTextFoldsNewlinesButKeepsTheReasonVerbatim() {
    // 约束 3 + 约束 4 的交点：原因**看得见**且**不加工**。
    // 内核 coreNotFoundError 的原文就长这样（带换行与三条路径）。
    let reason = "找不到内核可执行文件 benagen-core（找过：\n/a\n/b\n/c）"
    let text = EngineStatusPresentation.text(for: .unavailable(reason))

    #expect(text == "引擎不可用：找不到内核可执行文件 benagen-core（找过： /a /b /c）")
    // 折行是排版让步：正文里不许再有换行（工具栏只有一行）
    #expect(!hasNewline(text))
    // 前缀 + 折行后的原因，逐字
    #expect(text.hasPrefix("引擎不可用："))
}

// MARK: - 握手超时（任务 4b：壳自己写文案的那一处）

@Test func engineStatusTextAndTooltipPinTheHandshakeTimeoutCopy() {
    // ⚠️ 这一处的文案是**壳自己写的**（不是"唯一一处"——`.protocolMismatch` 分支与几处
    //    `"…：\(原因)"` 的包装同样是壳写的；其余一律"内核原文照登"，约束 3）。
    //    理由：超时是**壳探测到的**状况，内核没给任何 message，没有"原文"可登。
    //    所以这两句逐字钉在这里 —— 改文案要连测试一起改，不许悄悄漂。
    let engine = AppModel.EngineState.unavailable(AppModel.handshakeTimeoutMessage)

    #expect(AppModel.handshakeTimeoutMessage == "内核无响应（等待超过 5 秒）")
    // 徽标正文：**那句话本身**（不是「引擎不可用：<原话>」那种套壳），因为它要出现在屏幕上
    // ——任务 4 栽过的那次正是"徽标文字整个掉进 tooltip"，约束 4 不允许再来一次。
    #expect(EngineStatusPresentation.text(for: engine) == "内核无响应（等待超过 5 秒）")
    // 浮层：给的是"该干什么"。
    #expect(EngineStatusPresentation.tooltip(for: engine)
            == "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。"
             + "请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。")
    // 超时也是"不可用"，图标沿用同一支（红色警告三角）——这里只是把它钉住，不许它变成 `nil` 之类。
    #expect(EngineStatusPresentation.systemImage(for: engine) == "exclamationmark.triangle.fill")
}

@Test func engineStatusTimeoutDiscriminatorIsExactNotFuzzy() {
    // 判别力：判别式必须是**逐字相等**，不能是"包含"。
    // 否则内核某天回一句带同样字样的原文（或原文被折行后恰好相等）也会被壳当成自己的文案，
    // 那就把约束 3「内核原文照登」悄悄破坏掉了。
    let nearMiss = AppModel.handshakeTimeoutMessage + "。"        // 多一个句号
    #expect(EngineStatusPresentation.text(for: .unavailable(nearMiss))
            == "引擎不可用：\(nearMiss)")
    #expect(EngineStatusPresentation.tooltip(for: .unavailable(nearMiss)) == nearMiss)
    // 反过来：只有**那一句**会被当成超时
    #expect(!EngineStatusPresentation.isHandshakeTimeout(nearMiss))
    #expect(EngineStatusPresentation.isHandshakeTimeout(AppModel.handshakeTimeoutMessage))
}

// MARK: - systemImage(for:)

@Test func engineStatusSystemImagePinsEveryCase() {
    #expect(EngineStatusPresentation.systemImage(for: .unknown) == "questionmark.circle")
    #expect(EngineStatusPresentation.systemImage(for: .notStarted) == "circle.dashed")
    #expect(EngineStatusPresentation.systemImage(for: .running) == "bolt.fill")
    #expect(EngineStatusPresentation.systemImage(for: .unavailable("x")) == "exclamationmark.triangle.fill")
}

@Test func engineStatusSystemImageDistinguishesAllFourCases() {
    // 判别力：四个 SF Symbol 名必须两两不同，否则界面上四种状态长一个样 ——
    // 那正是约束 4 要防的"分不清发生了什么的界面"。
    let all = [
        EngineStatusPresentation.systemImage(for: .unknown),
        EngineStatusPresentation.systemImage(for: .notStarted),
        EngineStatusPresentation.systemImage(for: .running),
        EngineStatusPresentation.systemImage(for: .unavailable("x")),
    ]
    #expect(Set(all).count == 4)
}

// MARK: - tooltip(for:)

@Test func engineStatusTooltipPinsEveryCase() {
    #expect(EngineStatusPresentation.tooltip(for: .unknown) == "内核尚未握手（壳正在启动它）")
    #expect(EngineStatusPresentation.tooltip(for: .notStarted) == "下载引擎尚未启动 —— 添加下载任务后内核会启动它")
    #expect(EngineStatusPresentation.tooltip(for: .running) == "下载引擎正在运行")
}

@Test func engineStatusTooltipKeepsReasonUntouchedWhileTextFoldsIt() {
    // 这一对是关键的判别性对照：同一份原文，浮层**一个字符都不动**（含换行），
    // 正文**折行**。实现若把两者接到同一个函数上，这条必红。
    let reason = "找不到内核可执行文件（找过：\n/a\n/b）"
    #expect(EngineStatusPresentation.tooltip(for: .unavailable(reason)) == reason)
    #expect(EngineStatusPresentation.tooltip(for: .unavailable(reason)) !=
            EngineStatusPresentation.text(for: .unavailable(reason)))
    // 反向钉住：正文确实折行了，浮层确实没折
    #expect(!hasNewline(EngineStatusPresentation.text(for: .unavailable(reason))))
    #expect(hasNewline(EngineStatusPresentation.tooltip(for: .unavailable(reason))))
}

// MARK: - singleLine(_:)

@Test func singleLineFoldsNewlinesIntoExactlyOneSpace() {
    #expect(EngineStatusPresentation.singleLine("a\nb") == "a b")
    #expect(EngineStatusPresentation.singleLine("a\nb\nc") == "a b c")
    // CRLF 也算换行（`Character.isNewline` 覆盖 \r）—— 内核原文若来自 Windows 侧会带 \r\n
    #expect(EngineStatusPresentation.singleLine("a\r\nb") == "a b")
    #expect(EngineStatusPresentation.singleLine("a\rb") == "a b")
}

@Test func singleLineLeavesNewlineFreeTextCompletelyUntouched() {
    // 「除换行外一个字符都不增删改」的**最强形式**：没有换行时输出与输入逐字相同。
    // 中英文混排、标点、空格、emoji、已有空格 —— 全都不许动。
    let samples = [
        "",
        "abc",
        "引擎不可用：内核崩了",
        "a  b",                       // 已有连续空格：不许被折成一个
        "  前导与尾随空格  ",            // 不许 trim
        "mixed 中英文 ASCII 123 ！@#",
        "emoji 🙂 也要原样",
        "tab\tinside",                // 制表符不是换行：不许动
    ]
    for s in samples {
        #expect(EngineStatusPresentation.singleLine(s) == s)
    }
}

@Test func singleLinePreservesEveryNonNewlineCharacterInOrder() {
    // 判别力：对**含换行**的输入，逐个字符核对"非换行的字符一个不少、顺序不变"。
    // 这抓的是"折行时把内容吃掉了"（例如误用 `replacingOccurrences` 时写错、
    // 或者用了 `filter` 而不是 `split`）。
    let input = "找过：\n/path/a\n/path/b\n"
    let out = EngineStatusPresentation.singleLine(input)

    // 换行一个不剩
    #expect(!hasNewline(out))
    // 去掉所有空白后，两者必须逐字相同（既没少字符，也没改字符）
    let strip: (String) -> [Character] = { $0.filter { !$0.isWhitespace } }
    #expect(strip(out) == strip(input))
    // 且输出确实是把换行换成了空格（不是全删）
    #expect(out == "找过： /path/a /path/b")
}

@Test func singleLineKeepsTheAnswerOneLine() {
    // 折行的**目的**：工具栏只渲染一行。首尾/连续换行会被 `split` 合并掉
    // （见 `singleLine` 的文档注释），但输出永远不含换行、且不含前导/尾随空格。
    #expect(EngineStatusPresentation.singleLine("\n\na") == "a")
    #expect(EngineStatusPresentation.singleLine("a\n\n") == "a")
    #expect(EngineStatusPresentation.singleLine("\n") == "")
    for s in ["\n\na", "a\n\n", "\n", "a\n\nb"] {
        let out = EngineStatusPresentation.singleLine(s)
        #expect(!hasNewline(out))
        #expect(out == out.trimmingCharacters(in: .whitespaces))
    }
}
