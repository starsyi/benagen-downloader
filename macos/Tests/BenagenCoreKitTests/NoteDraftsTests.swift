import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 换码面板里"备注草稿"的提交判决（阶段 E 规格 §1.4）
//
// 为什么这一段必须抽出来（本文件存在的全部理由）：它是本次**语义最绕**的一段
// —— "没草稿不写 / 没变不写 / 交完删草稿 / 草稿优先于现值 / 面板关闭时全部交出去"
// 五条规则交织在一起，而它的落点原本在一个 `@State` 字典上，视图不单测（本项目硬约束）
// ⇒ **零覆盖**。抽成纯值类型之后，每一条规则都能被单独钉住。
//
// 三条"伤害"各有一条测试守着（都是静默失效，约束 4 明禁的那一类）：
//   ① 用户**敲了字**却被丢掉 ⇒ `committingAChangedDraftWritesExactlyOnce`；
//   ② 用户**只是点了那一行**、没动备注，却被写了一次盘 ⇒ `committingWithoutADraftWritesNothing`；
//   ③ 用户**清空**了备注，界面却还显示旧的那句 ⇒ `aClearedFieldIsStillADraft`。
// ---------------------------------------------------------------------------

private let codeA = "KUCwdl7d5u.UovnpW_D7"
private let codeB = "AAAA1111bbbb2222CCCC"

@Test func aFreshDraftsStateHasNothingInIt() {
    let drafts = NoteDrafts()
    #expect(drafts.isEmpty)
    #expect(drafts.draft(forCode: codeA) == nil)
}

// MARK: - 读取：草稿优先于现值

@Test func theFieldShowsTheDraftWhenThereIsOneAndTheStoredNoteOtherwise() {
    let drafts = NoteDrafts().editing("客户张三", forCode: codeA)
    #expect(drafts.text(forCode: codeA, current: "旧的那句") == "客户张三",
            "用户敲进去的字必须压过模型里那份旧的")
    #expect(drafts.text(forCode: codeB, current: "别的那条") == "别的那条",
            "**别的行**不受影响：草稿是按码存的")
}

/// ⚠️ 这一条是**清空备注**能成立的全部理由。
///
/// 若把"草稿是空串"和"没有草稿"混为一谈（例如用 `?? ` 去接），用户把备注删空之后
/// 界面上会**弹回原来那句** —— 他明明删掉了，却看着它自己回来，而没有任何报错。
@Test func aClearedFieldIsStillADraft() {
    let drafts = NoteDrafts().editing("", forCode: codeA)
    #expect(drafts.draft(forCode: codeA) == "")
    #expect(drafts.text(forCode: codeA, current: "客户张三") == "",
            "空串是一个**真的草稿**，不是「没有草稿」")
}

// MARK: - 提交：没草稿 ⇒ 一个字节都不写

@Test func committingWithoutADraftWritesNothing() {
    // 用户点了一下那一行（它自己会加载），但没有碰备注框。这时**不许**写盘：
    // 写一次就是把 `last_used_at` 之外的东西也动了一遍，而且每次点击都写。
    let outcome = NoteDrafts().committing(code: codeA, current: "客户张三")
    #expect(outcome.writes.isEmpty)
    #expect(outcome.drafts.isEmpty)
}

// MARK: - 提交：没变 ⇒ 不写

@Test func committingAnUnchangedDraftWritesNothingAndDropsTheDraft() {
    let drafts = NoteDrafts().editing("客户张三", forCode: codeA)
    let outcome = drafts.committing(code: codeA, current: "客户张三")
    #expect(outcome.writes.isEmpty, "与现值相同 ⇒ 不写（免得每次失焦都写一次盘）")
    #expect(outcome.drafts.isEmpty, "草稿照样要清掉：交完之后编辑框读回模型那份")
}

// MARK: - 提交：变了 ⇒ 写一次，并且把草稿交回去

@Test func committingAChangedDraftWritesExactlyOnce() {
    let drafts = NoteDrafts().editing("客户张三 / 9月肿瘤数据", forCode: codeA)
    let outcome = drafts.committing(code: codeA, current: "客户张三")
    #expect(outcome.writes == [NoteDrafts.Write(code: codeA, note: "客户张三 / 9月肿瘤数据")])
    // 交完必须删草稿：留着的话编辑框会一直显示用户敲的**原文**（首尾空白、换行都没被归一化过），
    // 与真正存下去的那一份分叉。
    #expect(outcome.drafts.isEmpty)
}

// MARK: - 提交：这一行已经不在屏上了 ⇒ 丢掉草稿，但不写

@Test func committingARowThatIsNoLongerOnScreenDropsTheDraftWithoutWriting() {
    // `current == nil` = 这个码已经不在历史列表里了（屏上没有那一行）。
    // 凭空写一条**没有时间戳**的记录只会变成"排在最末、点了没反应"的假条目
    // （`BatchHistory.settingNote` 对不在历史里的码同样是空操作）。
    let drafts = NoteDrafts().editing("写点什么", forCode: codeA)
    let outcome = drafts.committing(code: codeA, current: nil)
    #expect(outcome.writes.isEmpty)
    #expect(outcome.drafts.isEmpty)
}

// MARK: - 面板关闭时：把还挂着的草稿全部交出去

@Test func committingAllWritesEveryChangedRowInAStableOrder() {
    // ⚠️ 用**局部**的码，让"按码排序"这件事在断言里读得出来（`codeA` / `codeB` 那两个
    //    常量的字典序是 `AAAA…` < `KUCw…`，写出来容易看反 —— 第一版就写反了）。
    let first = "CODE-A", second = "CODE-B"
    // 故意**反序**编辑（先 B 后 A），断言交出去的顺序**按码排序**而不是按编辑顺序 ——
    // 顺序必须是确定的，否则"到底写了几条、写了哪几条"在不同运行里是两个答案。
    let drafts = NoteDrafts()
        .editing("B 的备注", forCode: second)
        .editing("A 的备注", forCode: first)
    let outcome = drafts.committingAll(current: { $0 == first ? "旧的 A" : "旧的 B" })
    #expect(outcome.writes == [NoteDrafts.Write(code: first, note: "A 的备注"),
                               NoteDrafts.Write(code: second, note: "B 的备注")])
    #expect(outcome.drafts.isEmpty)
}

@Test func committingAllSkipsRowsWhoseDraftDidNotChange() {
    let drafts = NoteDrafts()
        .editing("没变", forCode: codeA)
        .editing("变了", forCode: codeB)
    let outcome = drafts.committingAll(current: { $0 == codeA ? "没变" : "旧的 B" })
    #expect(outcome.writes == [NoteDrafts.Write(code: codeB, note: "变了")])
    #expect(outcome.drafts.isEmpty)
}

@Test func committingAllOnAFreshStateIsAQuietNoOp() {
    let outcome = NoteDrafts().committingAll(current: { _ in "什么都好" })
    #expect(outcome.writes.isEmpty)
    #expect(outcome.drafts.isEmpty)
}

/// ⚠️ **遍历时必须先取键的快照**：`committing` 会产出一个**新的** `NoteDrafts`（值类型），
/// 但实现里是"边走边换整份状态"——若直接遍历 `byCode.keys` 并原地改写，
/// 就是边遍历边改（未定义行为）。这条用例用的是"每个码都要交"的最坏形态，
/// 所以只要实现退化成原地改，它就会在这里炸（或者在别的运行里随机少写一条）。
@Test func committingAllHandlesManyRowsAtOnce() {
    var drafts = NoteDrafts()
    for i in 0..<20 {
        drafts = drafts.editing("备注 \(i)", forCode: "CODE-\(i)")
    }
    let outcome = drafts.committingAll(current: { _ in "旧的" })
    #expect(outcome.writes.count == 20)
    #expect(outcome.drafts.isEmpty)
}

/// 每一条路都必须是同一个判决的另一半：**没变的那条不写，变的那条写**，
/// 而且**一个草稿都不许剩**。
@Test func committingAllIsExhaustiveOverTheDraftsItWasGiven() {
    let drafts = NoteDrafts()
        .editing("一样", forCode: "CODE-A")
        .editing("不一样", forCode: "CODE-B")
        .editing("", forCode: "CODE-C")          // 清空也是一次真的改动
    let outcome = drafts.committingAll(current: { $0 == "CODE-A" ? "一样" : "旧的" })
    #expect(outcome.writes == [NoteDrafts.Write(code: "CODE-B", note: "不一样"),
                               NoteDrafts.Write(code: "CODE-C", note: "")])
    #expect(outcome.drafts.isEmpty)
}

// MARK: - 与视图那三个提交点的关系
//
// ⚠️ 这里**曾经有一条同义反复的用例**（最终审查顺手 6）：
//    同一函数、同一实参调两次，再断言两次结果相等（`byEnter.writes == byFocusLoss.writes`）
//    —— 纯函数上这是**恒真**的，它钉不住"视图那三个提交点接没接上"（那在视图里，
//    本项目硬约束不单测），只占着一个"看起来有覆盖"的位置。
//    **降级成这段注释**：三个提交点（`onSubmit` 回车 / `onChange(of: editingNote)` 失焦 /
//    `onDisappear` 关面板）走的是**同一个** `committing`，视图只负责选时机 ——
//    这条是**接线事实**，人眼在 `macos/README.md` 第 19c 条里验（三个点各试一次），
//    不是单测能替的。
//    上面那两条（`committingAllWritesEveryChangedRowInAStableOrder` /
//    `committingAllSkipsRowsWhoseDraftDidNotChange`）才是真正有判别力的那一半。
