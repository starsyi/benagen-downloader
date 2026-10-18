import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 手动检测更新的**判决**（规格 §2 那颗「立即检测」按钮）
//
// 这些判据盯的是"**不许撒谎**"这一条：`update_status` 的 `has_newer` 在
// 「没查成」与「查成了、确实没新版」两种情形下**长得一模一样** —— 只看它，断网时
// 客户会读到一句「已经是最新的」。所以判决必须先看开关、再看 `last_check_ok`，
// **最后**才轮到版本号（`UpdateCheckReceipt.of` 的判据顺序）。
//
// 与 Windows 那侧 `windows/shell-core/src/presentation/update_check.rs` 的
// `mod tests` **逐条同源**（那五句话的**逐字**一致由
// `windows/scripts/check_presentation_mirrors.sh` 的第 ③ 段判；这里判**什么时候说哪一句**）。
//
// 判别力：把 `of` 里 `last_check_ok` 那一段去掉（退回"只看 has_newer"）⇒
//    第 1 条与第 3 条立刻红（它们会说成"已经是最新的"）。
// ---------------------------------------------------------------------------

private func wire(hasNewer: Bool, latest: String? = nil, current: String = "0.2.7",
                  enabled: Bool = true, lastCheckOk: Bool? = true) -> UpdateStatus {
    UpdateStatus(hasNewer: hasNewer, latest: latest, current: current, url: nil,
                 enabled: enabled, lastCheckOk: lastCheckOk)
}

/// 🔴 **这个类型存在的理由**：没查成时**绝不许**说"已经是最新的"（约束 4，界面不撒谎）。
@Test func 没查成时绝不说已经是最新的() {
    let receipt = UpdateCheckReceipt.of(wire(hasNewer: false, lastCheckOk: false))
    #expect(receipt.text == "没查成：这次没连上发布页。补救：确认这台机器能上网，或者过一会儿再点一次。")
    #expect(receipt.isFailure, "没查成是失败档：按成功档画会让它配一个绿勾")
}

/// 🔴 **判据顺序是承重的**：开关**关着**的时候内核一次都没查，`has_newer` 是**上一次**
///    （可能几天前）的旧结论 ⇒ 说什么都不能拿它下结论。
///
/// 判别力：把开关那一段挪到版本号之后 ⇒ 这一条立刻红（会说出"有新版本 0.2.8"）。
@Test func 开关关着的时候不看版本号() {
    let receipt = UpdateCheckReceipt.of(
        wire(hasNewer: true, latest: "v0.2.8", enabled: false, lastCheckOk: true))
    #expect(receipt.text == "自动检查更新是关着的，刚才没有去查。补救：打开上面那个开关，再点一次「立即检测」。")
    #expect(receipt.isFailure)
}

/// 🔴 **三档不许并成两档**：`last_check_ok` 缺席（旧内核 / 协议缺格）是"**不知道**"，
///    不是"查成了、没有新版" —— 照样不许说"已经是最新的"。
@Test func 内核没给那一格时说不清() {
    let receipt = UpdateCheckReceipt.of(wire(hasNewer: false, lastCheckOk: nil))
    #expect(receipt.text == "没查到结果：内核回的这份检查状态不全。补救：重开一次客户端再试。")
    #expect(receipt.isFailure)
}

/// 还没问过内核（`nil`）**也走"说不清"**：这一格不许当成"查过了"。
///
/// 判别力：把 `guard let st = status else` 那一支改成"回成功档" ⇒ 这一条立刻红。
@Test func 还没问过内核时也说不清() {
    let receipt = UpdateCheckReceipt.of(nil)
    #expect(receipt.text == "没查到结果：内核回的这份检查状态不全。补救：重开一次客户端再试。")
    #expect(receipt.isFailure)
}

/// **只有"查成了"且"没有新版"这一支**配说「已经是最新的」（成功档、绿勾）。
@Test func 只有查成了才配说已经是最新的() {
    let receipt = UpdateCheckReceipt.of(wire(hasNewer: false, current: "0.2.7", lastCheckOk: true))
    #expect(receipt.text == "已经是最新的（当前 0.2.7）。")
    #expect(!receipt.isFailure, "这一支才是真的成功")
}

/// 查到新版：**两个版本号都在那句话里**（客户据此判断"我装的是哪个、新的是哪个"）。
@Test func 查到新版时两个版本号都在那句话里() {
    let receipt = UpdateCheckReceipt.of(
        wire(hasNewer: true, latest: "v0.2.8", current: "0.2.7", lastCheckOk: true))
    #expect(receipt.text == "有新版本 v0.2.8（当前 0.2.7）。")
    #expect(!receipt.isFailure)
}

/// 说"有新版"却**说不出是哪个版本** ⇒ 不猜（宁可不说话，也不摆一句缺了半个数的句子）。
@Test func 说有新版却说不出是哪个版本时不猜() {
    let missing = UpdateCheckReceipt.of(wire(hasNewer: true, latest: nil, lastCheckOk: true))
    let empty = UpdateCheckReceipt.of(wire(hasNewer: true, latest: "", lastCheckOk: true))
    #expect(missing.text == empty.text)
    #expect(missing.text.hasPrefix("没查到结果"))
    #expect(missing.isFailure && empty.isFailure)
}

/// 五档**各有各的那句话**：两句变一句的地方就是一次"说不清被说成了结论"。
///
/// 判别力：把任意两档合并（回同一个字符串）⇒ 这个集合立刻少于 5。
@Test func 五档各有各的那句话() {
    let sentences: Set<String> = [
        UpdateCheckReceipt.of(nil).text,
        UpdateCheckReceipt.of(wire(hasNewer: false, enabled: false)).text,
        UpdateCheckReceipt.of(wire(hasNewer: false, lastCheckOk: false)).text,
        UpdateCheckReceipt.of(wire(hasNewer: true, latest: "v0.2.8", lastCheckOk: true)).text,
        UpdateCheckReceipt.of(wire(hasNewer: false, current: "0.2.7", lastCheckOk: true)).text,
    ]
    #expect(sentences.count == 5, "五档不许有两档说同一句话")
    // 失败那三档都是失败（判据是"这一下有没有拿到结论"，不是"命令有没有报错"）。
    #expect(UpdateCheckReceipt.of(nil).isFailure)
    #expect(UpdateCheckReceipt.of(wire(hasNewer: false, enabled: false)).isFailure)
    #expect(UpdateCheckReceipt.of(wire(hasNewer: false, lastCheckOk: false)).isFailure)
}
