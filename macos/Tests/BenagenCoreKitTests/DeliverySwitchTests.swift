import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// `DeliverySwitch` 的契约测试
//
// ⚠️ 这个类型几乎**没有算法**：它就是"换码这件事的两种结果 + 各自带的那一句话"。
//    所以这里断言的多是**载荷契约**，而这几条恰好是承重的：
//      ① 成功与失败**分得开**（面板靠它决定"关掉"还是"留在原地显示原文"）；
//      ② 失败那句来自**唯一**那处映射（约束 3 / C-7：壳不加工、不加前缀、不 trim、
//         不另写第二份映射）；
//      ③ 主区那一行常驻提示要显示什么（`noticeText` / `isFailure`）—— 那两行是
//         "面板被取消之后结果还在"的唯一落点，所以它们有可断言的值，就必须有测试。
//    会变的那部分行为（失败保留原批次、成功才提交、回执落在模型上）在
//    `AppModelTests.swift` 里，因为那是 `AppModel` 的状态机、不是这个值类型。
// ---------------------------------------------------------------------------

@Suite("换码结果的呈现模型")
struct DeliverySwitchTests {

    @Test("成功与失败是两种结果，载荷不同也不相等")
    func theTwoOutcomesAreDistinguishable() {
        #expect(DeliverySwitch.switched(code: "BBBB") == .switched(code: "BBBB"))
        #expect(DeliverySwitch.switched(code: "BBBB") != .switched(code: "AAAA"))
        #expect(DeliverySwitch.failed(message: "x") == .failed(message: "x"))
        #expect(DeliverySwitch.failed(message: "x") != .failed(message: "y"))
        // 同一个字符串在两支里**不得**互相相等 —— 面板那句 `if case .failed` 就靠这一条。
        #expect(DeliverySwitch.switched(code: "同一句话") != .failed(message: "同一句话"))
    }

    // ⚠️ 这里曾经有一条 `theFailureCarriesTheKernelTextVerbatim`：它把一句话放进
    //    `.failed(message:)` 再取出来比相等 —— **恒真**，任何生产代码改动都不会让它变红
    //    （复审顺手修 ③把它删掉了）。"壳不加工内核原文"这条纪律的真落点有两个：
    //    ① 下面 `theFailureMessageComesFromTheSingleMapping`（文案来自唯一那处映射）；
    //    ② `AppModelTests.aFailedSwitchKeepsTheCurrentBatch`（那条断言逐字比内核原文）。

    @Test("失败那支拿到的就是 AppModel.message(of:) 给的那一句")
    func theFailureMessageComesFromTheSingleMapping() {
        // C-7：`AppModel.message(of:)` 是 `CoreError` → 用户可见文案的**唯一**实现。
        // 面板显示的那句话必须是它的产物 —— 这条钉住"两支之间没有第二份映射"。
        let e = CoreError.rpc(code: "delivery_fetch_failed",
                             message: "拉取交付清单失败：HTTP 404")
        #expect(DeliverySwitch.failed(message: AppModel.message(of: e))
                == .failed(message: "拉取交付清单失败：HTTP 404"))
    }

    // -----------------------------------------------------------------------
    // 主区那一行常驻提示（任务 2 复审重要 ②/③）
    // -----------------------------------------------------------------------

    @Test("失败那支的提示就是内核原文逐字，壳不加前缀、不改写")
    func theNoticeOfAFailureIsTheKernelTextVerbatim() {
        // ⚠️ 这一条**不是**恒真的：它断言的是 `noticeText` 这个**实现**的行为
        //    （把那句话加工一下、加个「换码失败：」前缀，这里立刻红）。
        //    内核原文的形状是内核自己的事：前后空白、换行、中文标点都可能是它的一部分。
        let kernelSaid = "  拉取交付清单失败：HTTP 404（第 2 行）\n"
        #expect(DeliverySwitch.failed(message: kernelSaid).noticeText == kernelSaid,
                "主区那一行必须逐字是内核原文（约束 3 / C-7）")
    }

    @Test("成功那支的提示说清楚换到了哪个批次")
    func theNoticeOfASuccessNamesTheNewBatch() {
        // 为什么这条有判别力：用户点「取消」之后那次换码照样会成功，并把整批换掉
        // （`RootView.onChange(of: loadedCode)` 复位浏览位置与勾选面）——
        // 没有这一行，用户看到的是"我按了取消、界面自己换了一批"。
        let text = DeliverySwitch.switched(code: "C24-8_×_25WS024").noticeText
        #expect(text.contains("C24-8_×_25WS024"), "必须报出**新批次**的码（内核回的那个 code）")
        // 成功那支不得混进任何失败措辞（反过来也一样，见上一条的逐字相等）。
        for bogus in ["失败", "错误", "重试"] {
            #expect(!text.contains(bogus), "成功那支出现了失败措辞「\(bogus)」：\(text)")
        }
    }

    @Test("两种结果在提示里分得开（视图靠它选图标与颜色）")
    func theNoticeTellsSuccessAndFailureApart() {
        #expect(DeliverySwitch.failed(message: "x").isFailure)
        #expect(DeliverySwitch.switched(code: "x").isFailure == false,
                "反向的一半也要断言，否则恒 true 的变异体也能活下来")
    }
}
