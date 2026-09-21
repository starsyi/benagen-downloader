import Foundation

// ---------------------------------------------------------------------------
// 备注编辑框的**草稿状态机**（阶段 E 规格 §1.4「每行可就地编辑备注」）
//
// 为什么整段搬出来（本文件存在的全部理由）：这是换码面板里**语义最绕**的一段
// —— "没草稿不写 / 没变不写 / 交完删草稿 / 草稿优先于现值 / 面板关闭时全部交出去"
// 五条规则交织在一起，而它的落点原本是视图里的一个 `@State` 字典。
// 视图不单测（本项目硬约束）⇒ 那五条规则**一条都没有覆盖**。
// 搬进 `Presentation/` 之后每一条都能被单独钉住（E-7）。
//
// ⚠️ **提交时机留在视图里**（回车 / 失焦 / 面板关闭三个点），这里只回答
//    "**这一次**该不该写、写什么"。时机是渲染分派、判决是值计算 —— 这条分工与
//    `DeliverySwitch`（`AppModel` 算结果、面板决定怎么显示）是同一条。
//
// ⚠️ 本文件**不碰文件系统、不碰 `AppModel`、不拼任何上屏文案**：它只把一个
//    `[码: 用户正在敲的那串字]` 变成"要写哪几条备注"。真正的写入是
//    `AppModel.setHistoryNote(_:forCode:)`（它复用 `BatchHistory.settingNote` 的归一化）。
// ---------------------------------------------------------------------------

/// 历史列表里那些**还没交出去的备注草稿**。
///
/// 值类型（每次编辑返回新值），所以"边遍历边改"这种 bug 在类型层面就不成立。
/// 不变量只有一条：**键是交付码**（历史里那一行的唯一键）。
public struct NoteDrafts: Equatable, Sendable {

    /// 一条要交给历史的备注写入。
    public struct Write: Equatable, Sendable {
        public let code: String
        public let note: String

        public init(code: String, note: String) {
            self.code = code
            self.note = note
        }
    }

    /// 一次提交的**判决结果**：要写哪几条 + 交完之后还剩哪些草稿。
    ///
    /// ⚠️ 两个字段**必须一起用**：只取 `writes` 而忘了把 `drafts` 装回去，
    ///    编辑框里就会一直留着用户敲的原文（与真正存下去的那一份分叉）。
    public struct Outcome: Equatable, Sendable {
        public let writes: [Write]
        public let drafts: NoteDrafts
    }

    private var byCode: [String: String]

    /// 一个草稿都没有。
    public init() { byCode = [:] }

    private init(byCode: [String: String]) { self.byCode = byCode }

    public var isEmpty: Bool { byCode.isEmpty }

    /// 这一行现在挂着的草稿；`nil` = 用户没动过它。
    ///
    /// ⚠️ `nil` 与 `""` 是**两件事**：`""` 是"用户把它清空了"（一个真的草稿），
    ///    `nil` 是"用户没碰过"。混为一谈的后果是**清空备注之后旧的那句会自己弹回来**。
    public func draft(forCode code: String) -> String? { byCode[code] }

    /// 编辑框该显示什么：**草稿优先于现值**。
    ///
    /// 用 `??` 接住 `nil`（而不是判 `isEmpty`）—— 见 `draft(forCode:)` 那条。
    public func text(forCode code: String, current: String) -> String {
        byCode[code] ?? current
    }

    /// 记下用户敲进去的字（返回新值）。
    public func editing(_ text: String, forCode code: String) -> NoteDrafts {
        var next = byCode
        next[code] = text
        return NoteDrafts(byCode: next)
    }

    // MARK: - 提交

    /// 把**一个**码的草稿交出去。返回"要不要写、写什么"，以及交完之后的状态。
    ///
    /// 三道闸，每一道都在挡一种具体的伤害：
    ///   ① **没有草稿** ⇒ 一个字节都不写。用户只是点了一下那一行（它会加载），
    ///      不是来改备注的 —— 每次点击都写一次盘是把"编辑"这件事变成噪音；
    ///   ② **这一行已经不在屏上了**（`current == nil`）⇒ 丢掉草稿，不写。
    ///      凭空写一条没有时间戳的记录只会变成"排在最末、点了没反应"的假条目
    ///      （`BatchHistory.settingNote` 对不在历史里的码同样是空操作，这里只是不含糊地表达它）；
    ///   ③ **与现值相同** ⇒ 不写（免得每次失焦都写一次盘）。
    ///
    /// ⚠️ **有草稿就一定把它清掉**（连 ①②③ 三支也是）：交完之后编辑框读回**模型那份**
    ///    —— 它在 `BatchHistory.settingNote` 里被归一化过（首尾空白去掉、换行折成空格），
    ///    编辑框要如实显示**存下去的那一份**，而不是用户敲的原文。
    public func committing(code: String, current: String?) -> Outcome {
        guard let draft = byCode[code] else {
            return Outcome(writes: [], drafts: self)          // ①
        }
        let cleared = NoteDrafts(byCode: byCode.filter { $0.key != code })
        guard let current, draft != current else {
            return Outcome(writes: [], drafts: cleared)       // ② / ③
        }
        return Outcome(writes: [Write(code: code, note: draft)], drafts: cleared)
    }

    /// 把**还挂着的所有**草稿交出去（面板关闭时的那一次兜底）。
    ///
    /// ⚠️ 判据是**这里还挂着哪些键**，不是"屏上有哪些行"：这时面板正在销毁，
    ///    走一遍屏上的行等于把"用户敲过字"这件事重新押在视图树还在不在上。
    ///
    /// ⚠️ 顺序按**码排序**，不是按用户编辑的先后：顺序必须确定 ——
    ///    否则"到底写了几条、写了哪几条"在不同运行里是两个答案，也没法断言。
    ///    （写盘本身是幂等的逐条覆写，顺序不影响结果，只影响可观察性。）
    public func committingAll(current: (String) -> String?) -> Outcome {
        var state = self
        var writes: [Write] = []
        for code in byCode.keys.sorted() {
            let outcome = state.committing(code: code, current: current(code))
            state = outcome.drafts
            writes += outcome.writes
        }
        return Outcome(writes: writes, drafts: state)
    }
}
