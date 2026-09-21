import Testing
@testable import BenagenCoreKit

@Suite("主窗口重开的判据")
struct MainWindowPolicyTests {

    @Test("从来没有过窗口时不抢开窗口——启动那一刻 SwiftUI 本来就会建一个")
    func neverOpensBeforeAWindowEverExisted() {
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: false,
                                                hasEverShownWindow: false) == false)
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: true,
                                                hasEverShownWindow: false) == false)
    }

    @Test("有过窗口、现在一个可见的都没有 ⇒ 重开")
    func reopensWhenEveryWindowIsGone() {
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: false,
                                                hasEverShownWindow: true) == true)
    }

    @Test("还有可见窗口 ⇒ 不动（否则每次激活都会多开一个）")
    func doesNotOpenWhileAWindowIsVisible() {
        #expect(MainWindowPolicy.shouldOpenMain(anyVisibleWindow: true,
                                                hasEverShownWindow: true) == false)
    }
}
