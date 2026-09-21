//! verify_summary —— 校验结果的**呈现模型**：六行归类 + 顶部那一行总结 + 分区徽标计数 +
//! 总进度 + 刷新失败的那句话。全部是纯函数，无状态。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/VerifySummary.swift`（逐字对位）。
//!
//! 为什么这些映射在这里而不在别处（上游文件头那条全局约束 8）：
//!   「把内核数据变成呈现值」的纯计算一律放 `presentation/`，**每条都要有单测**。
//!   判据是"一段代码只要你能写出一个断言，它就该住在这一层，而不是住进只做组装的地方" ——
//!   下面每一条都能写出断言，而且有几条背着硬约束：
//!     · 六类**互斥穷尽**（计数为 0 的类也要有它的那一行 —— 靠"路径数组为空所以没东西可给"
//!       来表达 0，在客户眼里就是"这一类不见了"，约束 4）；
//!     · 路径**原文照登**（不取最后一段、不规范化、不转义，约束 3）；
//!     · 以及"**壳不得比内核更严**"：`all_good` 不含 `unverifiable`
//!       （`core/src/verify.rs` 的 `CheckResult::all_good`），壳不许把它算进去。
//!
//! ⚠️ 本文件**不含界面概念**（章程见 `lib.rs` 头部）：颜色用 [`RowColor`] 这个**枚举**表达
//!    （同 `BrowserRow` / `TransferRow`），"枚举 → 实际颜色"的那一步留在 `shell-win`。
//!
//! ⚠️ **W-6（有意偏离：不改上游的名字）**：上游那几颗图标名是 **SF Symbol 名**，
//!    这里**原样移植**。按本 crate 的章程（`lib.rs` 头部：不含界面概念），
//!    本可以把这一列删掉、或换成中性的名字 —— **没有那样做**，理由是：
//!    这一列是**上游呈现语义的一部分**（六条两两不同，由 `every_class_has_a_distinct_label`
//!    钉着），把它改写成别的名字会让壳侧无从与上游逐条对位；至于这个名字怎么落地
//!    由 `shell-win` 决定。**不许**把它改写成"没有图标"。

use serde::Serialize;
use super::error_text::error_text;
use super::format::{ByteFormat, PercentFormat, SpeedFormat};
use super::transfer_row::TransferRow;
use super::RowColor;
use crate::client::ClientError;
use crate::protocol::{TransferListResult, TreeResult, VerifyClass, VerifyStatus};

// ---------------------------------------------------------------------------
// 一类校验结果
// ---------------------------------------------------------------------------

/// 上游 `extension VerifyClass`（`VerifySummary.swift:26-110`）。
impl VerifyClass {
    /// 这一类算不算**失败**。
    ///
    /// ⚠️ `unverifiable` **不算** —— 这一条是内核的口径，壳一个字都不许改：
    ///    `CheckResult::all_good()` 的定义是
    ///    `bad.len() + missing.len() + size_mismatch.len() + unreadable.len() == 0`，
    ///    `unverifiable` 不在其中（"清单与 HEAD 都没给出 crc64" ⇒ 没有可比的校验值，
    ///    不是"坏"，但也不是"已核对无误"）。壳把它算成失败就是**比内核更严**：
    ///    客户面前会出现"总结说全部通过、另一处却挂着一个非零的失败计数"。
    ///    钉住它的是 `all_good_excludes_unverifiable` / `the_verify_badge_does_not_count_unverifiable`。
    ///
    /// ⚠️ `unreadable` **算**失败（内核的注释："读不了就无法证明它是完整的，
    ///    不能让客户以为交付齐全"）。
    pub fn is_failure(&self) -> bool {
        match self {
            VerifyClass::Passed | VerifyClass::Unverifiable => false,
            VerifyClass::Mismatched
            | VerifyClass::Missing
            | VerifyClass::SizeMismatch
            | VerifyClass::Unreadable => true,
        }
    }

    /// 呈现标签。六条**两两不同**（约束 4：两类长得一样，等于其中一类永远看不见）。
    ///
    /// 措辞对着 `core/src/verify.rs` 里 `CheckResult` 各字段的注释取，不自己编语义：
    ///   - `bad` = "crc64 不符，或路径不可信（越界/控制字符），或该路径上不是普通文件"；
    ///   - `missing` = "本地不存在"；
    ///   - `size_mismatch` = "大小不符"；
    ///   - `unverifiable` = "清单与 HEAD 都没给出 crc64"；
    ///   - `unreadable` = "能 stat 到但读不了（权限/IO 错误）"。
    pub fn label(&self) -> &'static str {
        match self {
            VerifyClass::Passed => "校验通过",
            VerifyClass::Mismatched => "内容不符",
            VerifyClass::Missing => "文件缺失",
            VerifyClass::SizeMismatch => "大小不符",
            VerifyClass::Unverifiable => "无法校验",
            VerifyClass::Unreadable => "无法读取",
        }
    }

    /// 语义色（规格 §7.2）。**不含具体的颜色值**（本 crate 不引入绘制依赖），由 `shell-win` 映射。
    ///
    /// 三类分工：`Passed` 成功绿、四个失败类错误红、`Unverifiable` **中性灰**。
    ///
    /// ⚠️ **中性色是 `Unverifiable` 的专属标记**：别的类也用上 `Secondary`，
    ///    这一行就从"不是失败"退回成"和它们一样"，中性色想表达的那句话当场消失。
    ///    钉住它的是 `the_unverifiable_class_is_the_only_neutral_colour`。
    ///    （四个失败类同色是有意的：它们在客户眼里是同一件事"没通过"，
    ///     彼此的区分靠标签、图标与**各自的路径列表**。）
    pub fn color(&self) -> RowColor {
        match self {
            VerifyClass::Passed => RowColor::Green,
            VerifyClass::Unverifiable => RowColor::Secondary,
            VerifyClass::Mismatched
            | VerifyClass::Missing
            | VerifyClass::SizeMismatch
            | VerifyClass::Unreadable => RowColor::Red,
        }
    }

    /// 行首那颗图标的**名字**（同 `BrowserRow.iconName` / `TransferRow.stateIconName` 的形态）。
    /// 六条**两两不同**（理由同 [`VerifyClass::label`]）。
    ///
    /// ⚠️ **W-6（有意偏离：不改上游的名字）**：原样移植上游的六个 **SF Symbol 名** ——
    ///    它们是"每个状态有自己的名字"这条规则的落点（理由见本文件头注释），
    ///    `shell-win` 负责把它换成能落地的东西。
    pub fn icon_name(&self) -> &'static str {
        match self {
            VerifyClass::Passed => "checkmark.circle.fill",
            VerifyClass::Mismatched => "xmark.circle.fill",
            VerifyClass::Missing => "questionmark.folder",
            VerifyClass::SizeMismatch => "ruler",
            VerifyClass::Unverifiable => "questionmark.circle",
            VerifyClass::Unreadable => "lock.slash",
        }
    }

    /// 挂在标签下面的一句说明；`None` = 这一类没有需要消歧的地方。
    ///
    /// ⚠️ 只有 `Unverifiable` 有：它旁边永远站着一个"不是失败"的问号 ——
    ///    客户看到"无法校验 3 项"会自然地读成"有 3 项没通过"，而内核的判定正相反
    ///    （`all_good` 不含它）。这句话是**壳自己写的**（同 `AppModel.busyReason` /
    ///    `EngineGate.unavailableHelp` 的性质）：内核那句 `all_good` 是布尔值，
    ///    没有一个"原文"可以把这件事讲给客户听。
    ///    **每一类都出这一行的结果**：这一行永远在（因为这一类永远在），
    ///    `VerifyClassRow` 里 `note` 非 `None` 就等于那一句会被给出来。
    pub fn note(&self) -> Option<&'static str> {
        match self {
            VerifyClass::Unverifiable => Some("这些文件没有可比的校验值，不是失败"),
            VerifyClass::Passed
            | VerifyClass::Mismatched
            | VerifyClass::Missing
            | VerifyClass::SizeMismatch
            | VerifyClass::Unreadable => None,
        }
    }
}

/// 本枚举的**变体个数**。
///
/// ⚠️ **它是本文件里唯一一处"手维护"的、而且两个方向都没有网的常量**（两条都实测过，
///    残余的洞在 [`ALL_CLASSES`] 的说明里交代）：
///      · **调大**（如 7）⇒ 编译不过，但红的是 [`class_at`] 的 `_ => panic!` 支
///        （`E0080: evaluation panicked: VerifyClass 的变体数变了…`），
///        **不是**下面那条自检的 `assert`；
///      · **调小**（如 5）⇒ **编译通过**（自检里 `i < 5` 全部成立），
///        只有**测试**兜得住（实测 `FAILED. 165 passed; 5 failed`）。
///    ⇒ **别把"改了它会有东西响"当成保证：只有调大才响，调小不响。**
const CLASS_COUNT: usize = 6;

/// 序号 → 变体，顺序即上游 `VerifyClass` 的**声明序**。
const fn class_at(index: usize) -> VerifyClass {
    match index {
        0 => VerifyClass::Passed,
        1 => VerifyClass::Mismatched,
        2 => VerifyClass::Missing,
        3 => VerifyClass::SizeMismatch,
        4 => VerifyClass::Unverifiable,
        5 => VerifyClass::Unreadable,
        // 兜底支是给"序号越界"用的：真正看住"枚举有几个变体"的是 [`class_index`]。
        _ => panic!("VerifyClass 的变体数变了：CLASS_COUNT 与 class_at 必须一起改"),
    }
}

/// 变体 → 序号。
///
/// ⚠️ **这是一个穷尽 `match`（没有 `_` 支），它就是"新变体会编译不过"的那道守卫**：
///    给 [`VerifyClass`] 加第七个归类、而这里没跟着加 ⇒ `non-exhaustive patterns`，
///    **编译错误**。少了它，新类会**静默地不出现在 [`ALL_CLASSES`] 里**
///    ——"这一类不见了"正是全局约束 4 要防的形态（计数为 0 的类也必须有它的那一行）。
const fn class_index(class: VerifyClass) -> usize {
    match class {
        VerifyClass::Passed => 0,
        VerifyClass::Mismatched => 1,
        VerifyClass::Missing => 2,
        VerifyClass::SizeMismatch => 3,
        VerifyClass::Unverifiable => 4,
        VerifyClass::Unreadable => 5,
    }
}

/// 六个归类的**声明序**（上游 `VerifyClass.allCases`，由 `CaseIterable` 合成）。
///
/// ⚠️ 顺序是承重的：[`VerifySummary::rows`] 的顺序必须与它逐个对齐
///    （`all_six_classes_are_always_present_even_when_empty` 钉着）。
///
/// ⚠️ **它是"半派生"的，别读成"从枚举完全派生"** —— 这是本文件唯一一处需要读者记住的洞
///    （控制者 Ruling PP 原话是"关掉那个静默不出现的口子"，**实测只关了一半**；
///    本注释的上一版写成"不可能发生"，被一个全绿反例证伪，已按实测改写）：
///      · **派生的那一半**：数组元素**不是手抄的**，由 [`class_at`] 按序号填出来；
///        且 [`class_index`] 的穷尽 `match` + 下面那条编译期自检把"序号 ↔ 变体一一对应"
///        （⇒ 无重复、无遗漏）钉死。
///      · **没网的那一半**：**长度来自手维护的 [`CLASS_COUNT`]**。
///        ⇒ 给 `VerifyClass` 加第七个变体、把**所有**穷尽 `match` 补齐（含 `class_index` 里
///        补一行），但**不动 `class_at`/`CLASS_COUNT`**：**编译通过、170 条测试全绿**，
///        而新类在 `ALL_CLASSES`（⇒ `VerifySummary::rows`）里**一条都不出现**。
///        （复审实测，本轮已独立复现。）根因：[`class_index`] 的 `E0004` 只**逼人补一行
///        index**，之后就只剩"记得改 `CLASS_COUNT`"这个人工动作。
///    ⇒ **"新类静默不出现"这条口子只关了一半，另一半是写在明处的**。
///      一个不响的守卫与没有守卫，在客户机器上的后果一样；而"被一句『不可能发生』盖住的
///      洞"比"写在明处的洞"危险得多 —— 所以这里如实写出残余的洞。
///      （控制者已裁定**不**为此引入 `macro_rules!`：本 crate 的第一个宏、为这个风险
///       不成比例。⇒ **把它记清楚**就是全部要求。）
///
/// ⚠️ 与 `transfer_row.rs` 的 `ALL_STATES` **用途相同**（都是各自枚举的 `allCases` 对位）。
///    差别在**守卫**：那一份是纯手抄清单；这一份的**元素**是派生的、且有"序号 ↔ 变体"的
///    编译期自检 —— 但如上一段所说，**长度那一环两者一样没有网**。
///
/// ⚠️ 下面那条编译期自检**只管"重复/遗漏"**（`ALL_CLASSES[i]` 的序号必须就是 `i`），
///    **不管长度**：`CLASS_COUNT` 调小它照样全成立（见 [`CLASS_COUNT`] 的说明）。
///
/// ⚠️ 钉它的测试**用六个变体的字面量**当期望值（不拿本常量当期望值——
///    那就成了自己证明自己）。
pub const ALL_CLASSES: [VerifyClass; CLASS_COUNT] = {
    let mut all = [VerifyClass::Passed; CLASS_COUNT];
    let mut i = 0;
    while i < CLASS_COUNT {
        all[i] = class_at(i);
        i += 1;
    }
    all
};

/// 编译期自检：`ALL_CLASSES[序号]` 的序号必须**就是**那个序号（⇒ 无重复、无遗漏）。
///
/// ⚠️ 它在**编译期**求值：不成立就编译不过（实测：把 `class_at` 里两类对调 ⇒
///    `E0080: evaluation panicked: ALL_CLASSES 与 VerifyClass 的变体对不上`）。
///
/// ⚠️ **它管的是"重复/遗漏"，不是"长度"**：`CLASS_COUNT` 被调小时，循环体
///    `i < CLASS_COUNT` 全部成立 ⇒ 这条自检**不响**（那一路由测试兜，见
///    [`CLASS_COUNT`] 的说明）。所以它**不是**"`ALL_CLASSES` 与枚举不会漂移"的完整保证。
const _: () = {
    let mut i = 0;
    while i < CLASS_COUNT {
        assert!(
            class_index(class_at(i)) == i,
            "ALL_CLASSES 与 VerifyClass 的变体对不上（有重复或遗漏）"
        );
        i += 1;
    }
};

/// 一类校验结果的一行：**已经算好的呈现值**。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct VerifyClassRow {
    pub kind: VerifyClass,
    /// 呈现标签（六条两两不同）。
    pub label: String,
    /// 语义色（枚举，不是具体的颜色值）。
    pub color: RowColor,
    /// 行首那颗图标的**名字**。
    pub icon_name: String,
    /// 这一类的路径，**内核原文、逐字**（约束 3）。
    pub paths: Vec<String>,
    /// 标签下面那句说明（只有 `Unverifiable` 有）。
    pub note: Option<String>,
}

impl VerifyClassRow {
    /// 计数。**恒等于 `paths.len()`** —— 0 也是 0，不是"没有这一行"。
    pub fn count(&self) -> usize {
        self.paths.len()
    }

    /// 「N 项」。
    ///
    /// ⚠️ 它存在的全部理由是约束 4 那一句"计数为 0 的类**显示 0，不得隐藏**"：
    ///    只靠"路径列表为空所以没东西可给"来表达 0，在客户眼里就是"这一类不见了"。
    pub fn count_text(&self) -> String {
        // 上游是 `"\(count) 项"`；两处共用一个格式串，改一处就两处都改。
        format!("{} 项", self.count())
    }

    /// 这一类算不算失败（口径在 [`VerifyClass::is_failure`]）。
    pub fn is_failure(&self) -> bool {
        self.kind.is_failure()
    }

    /// 稳定身份。上游是 `id: String { kind.rawValue }`，而 Swift 的 `rawValue`
    /// **正是线上键名**；Rust 侧那份翻译只有一份：[`VerifyClass::wire_key`]
    /// （**不重写第二份**——抄一份就会出现"同一类在两处身份不同"）。
    pub fn id(&self) -> &'static str {
        self.kind.wire_key()
    }
}

// ---------------------------------------------------------------------------
// 顶部那一行总结
// ---------------------------------------------------------------------------

/// 顶部总结的四种形态。
///
/// ⚠️ 比简报里那两句（「全部通过」/「N 项未通过」）**多两句**，两处都是有意加的，
///    理由写在各自的变体上（约束 11：有意偏离要注明理由）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub enum VerifyVerdict {
    /// 内核把这一批的 `k.verify` 重置过、到此刻还没有任何文件被归类。
    ///
    /// ⚠️ **这一支是必须的**：`verify_status` 在"还没校验过"时回的正是
    ///    **六类全空 + `all_good == true`**（`core/src/main.rs` 把 `k.verify` 重置成
    ///    `verify::CheckResult::default()`，而 `CheckResult::all_good()` 在四项失败全空时为真）。
    ///    照登 `all_good` 的后果是：一批**从没校验过**的文件顶上写着「全部通过」——
    ///    一张空洞的通行证，而客户会把它读成"我的数据都核对过了"。
    ///    这正是约束 4 要防的那类静默失效。
    ///    （**不是**"比内核更严"：壳只是不在"零个文件"上替内核下一个结论，
    ///    `all_good` 本身照原样登在 [`VerifySummary::all_good`] 上。）
    NotVerified,
    AllGood,
    /// 六类明细里有 N 项未通过。
    Failed(usize),
    /// 内核说没全过（`all_good == false`），但六类里一个失败项都没有。
    ///
    /// ⚠️ 今天的**正确内核走不到这里**（`all_good()` 就是那四类计数算出来的）。
    ///    留着它是因为另外两条路都不对：说「全部通过」= 照抄一个与明细矛盾的判定；
    ///    说「0 项未通过」= 读起来像"没事"。矛盾必须**说出来**（约束 4），
    ///    钉住它的是 `a_kernel_judgement_of_not_all_good_is_never_dropped`。
    KernelSaysNotAllGood,
}

impl VerifyVerdict {
    pub fn text(&self) -> String {
        match self {
            VerifyVerdict::NotVerified => "尚未校验".to_string(),
            VerifyVerdict::AllGood => "全部通过".to_string(),
            VerifyVerdict::Failed(n) => format!("{n} 项未通过"),
            VerifyVerdict::KernelSaysNotAllGood => {
                "内核判定未全部通过（六类里没有失败项）".to_string()
            }
        }
    }

    pub fn color(&self) -> RowColor {
        match self {
            VerifyVerdict::NotVerified => RowColor::Secondary,
            VerifyVerdict::AllGood => RowColor::Green,
            VerifyVerdict::Failed(_) => RowColor::Red,
            VerifyVerdict::KernelSaysNotAllGood => RowColor::Orange,
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            VerifyVerdict::NotVerified => "clock",
            VerifyVerdict::AllGood => "checkmark.seal.fill",
            VerifyVerdict::Failed(_) => "exclamationmark.triangle.fill",
            VerifyVerdict::KernelSaysNotAllGood => "exclamationmark.triangle",
        }
    }
}

// ---------------------------------------------------------------------------
// 六类 + 总结
// ---------------------------------------------------------------------------

/// 一次校验结果的**全部**呈现值：六行（恒六条，含计数为 0 的那些）+ 顶部总结。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct VerifySummary {
    /// **恒为 6 条**，顺序是 [`ALL_CLASSES`]（ok / bad / missing /
    /// size_mismatch / unverifiable / unreadable），**空的那几类也在里面**（约束 4）。
    pub rows: Vec<VerifyClassRow>,
    /// 内核的 `all_good`，**原样**。
    ///
    /// ⚠️ 它是内核的判定，壳不二次推导（"内核说啥就是啥"，约束 1/3）。
    ///    顶部那一行总结**另有一份**由明细算出来的口径 —— 两者矛盾时以明细为准
    ///    （`the_summary_never_says_all_clear_while_listing_a_failure`），矛盾本身不会被藏起来。
    pub all_good: bool,
    /// 未通过项数 = `bad + missing + size_mismatch + unreadable`
    /// （口径见 [`VerifyClass::is_failure`]）。
    pub failed_count: usize,
    /// 六类合计 —— 内核这一批**已经归过类**的文件数。
    /// 它是"这份结果覆盖了多少"的唯一可见证据（0 ⇒ 还没校验过）。
    pub classified_count: usize,
    /// 顶部那一行总结。
    pub verdict: VerifyVerdict,
}

impl VerifySummary {
    /// 顶部总结的正文。
    pub fn headline(&self) -> String {
        self.verdict.text()
    }

    /// 顶部总结的颜色（枚举，由 `shell-win` 映射）。
    pub fn headline_color(&self) -> RowColor {
        self.verdict.color()
    }

    /// 顶部总结的图标。
    pub fn headline_icon(&self) -> &'static str {
        self.verdict.icon_name()
    }

    /// 「已校验 N 项」—— 让"这份结果覆盖了多少"看得见（同 `ByteFormat` 的"缺值也要说出来"）。
    pub fn classified_text(&self) -> String {
        format!("已校验 {} 项", self.classified_count)
    }

    /// `verify_status` 的结果 → 呈现值；**`None`（还没取过）→ `None`**。
    ///
    /// ⚠️ `None` 不能变成"六类全 0 + 全部通过"：那是**替内核宣布一句它没说过的话**
    ///    （还没问过它）。`shell-win` 对 `None` 走的是"还没有校验结果，点刷新"那一支，
    ///    钉住它的是 `no_result_at_all_is_not_an_all_clear`。
    pub fn of(status: Option<&VerifyStatus>) -> Option<VerifySummary> {
        let status = status?;

        // 恒六条：遍历的是 `ALL_CLASSES`（不是"内核给了哪些桶"）—— 空的那几类也在里面，
        // 计数为 0 的那一行同样带着它的标签、颜色、图标与「0 项」（约束 4）。
        let rows: Vec<VerifyClassRow> = ALL_CLASSES
            .iter()
            .map(|kind| VerifyClassRow {
                kind: *kind,
                label: kind.label().to_string(),
                color: kind.color(),
                icon_name: kind.icon_name().to_string(),
                // 路径是**内核原文、逐字**（约束 3）：直接搬，不加工。
                paths: status.paths(*kind).to_vec(),
                note: kind.note().map(|n| n.to_string()),
            })
            .collect();

        let failed_count: usize = rows
            .iter()
            .filter(|r| r.is_failure())
            .map(VerifyClassRow::count)
            .sum();
        let classified_count: usize = rows.iter().map(VerifyClassRow::count).sum();

        // ⚠️ 优先级是刻意的：**明细压过判定**。内核的 `all_good` 与六类计数由内核在
        //    同一个瞬间算出来（同一个 `CheckResult`），正常永远一致；真出现矛盾时，
        //    "正列着 3 个『内容不符』、顶上却写『全部通过』"是最坏的失效，
        //    所以先看明细。`all_good` 本身仍然**原样**带在上面，一个字都没改。
        let verdict = if classified_count == 0 {
            VerifyVerdict::NotVerified
        } else if failed_count > 0 {
            VerifyVerdict::Failed(failed_count)
        } else if status.all_good {
            VerifyVerdict::AllGood
        } else {
            VerifyVerdict::KernelSaysNotAllGood
        };

        Some(VerifySummary {
            rows,
            all_good: status.all_good,
            failed_count,
            classified_count,
            verdict,
        })
    }
}

// ---------------------------------------------------------------------------
// 分区徽标（规格 §7.1「传输列表（带未完成计数徽标）」）
// ---------------------------------------------------------------------------

/// 分区徽标的计数（上游 `SidebarBadge`；名字是上游的，怎么呈现是 `shell-win` 的事）。
///
/// ⚠️ 两个数都是"**没落定的那些**"，且都**不含**"已经不用管的"：
///    传输列表不含已完成 / 已移除；校验结果不含 `unverifiable`（内核的口径，见
///    [`VerifyClass::is_failure`]）。徽标挂着一个红数字、而内容区写着一句"全部通过"，
///    比不挂徽标更糟。
///
/// ⚠️ 0 在这里的意思是"这个分区没有未落定的东西"。六类各自的「0 项」是**另一回事**
///    —— 它们**必须看得见**（约束 4，钉住它的是 `all_six_classes_are_always_present_even_when_empty`）。
pub enum SidebarBadge {}

impl SidebarBadge {
    /// 传输列表：待下（`Waiting`）+ 下载中（`Active`）+ 失败（`Error`）。
    ///
    /// ⚠️ `Removed` 不算：它已经是"不用再管"的一档，出路在列表级的「清空已完成」，
    ///    把它算进"未完成"会让徽标永远减不到 0；`Complete` 更不算。
    ///    没有快照（`None`，还没轮询过）时是 0：没有数据不等于"有一堆没完成"。
    pub fn unfinished_transfers(list: Option<&TransferListResult>) -> usize {
        let Some(list) = list else { return 0 };
        list.items
            .iter()
            // ⚠️ 判据只有一份：[`TransferRow::is_unfinished`]（本模块**不抄第二遍** ——
            //    分叉的症状是"侧栏徽标上的数与传输列表里看得见的东西对不上"）。
            .filter(|i| TransferRow::is_unfinished(i.state))
            .count()
    }

    /// 从**已经呈现好的那些行**里数未落定 —— [`SidebarBadge::unfinished_transfers`]
    /// 的第二种入口（判据同一条，见那里的注释）。
    ///
    /// ⚠️ **为什么要有第二个入口**：`api::transfers` 手上拿到的是
    ///    `TransferRow`（内核的 `TransferItem` 已经被命令层过了一遍呈现），
    ///    而不是 `TransferListResult` —— 于是"数一数有几行没落定"这件事在那里
    ///    **没法用上面那一个**。两条路各数各的会让"哪些算未落定"出现第二份实现，
    ///    所以判据随行带过来（`TransferRow::counts_as_unfinished`），这里只数。
    pub fn unfinished_of_rows(rows: &[TransferRow]) -> usize {
        rows.iter().filter(|r| r.counts_as_unfinished).count()
    }

    /// 校验结果：未通过项数（口径就是 [`VerifySummary::failed_count`] 那**一份**）。
    ///
    /// ⚠️ 这里**不再自己求一遍和**：以前它是 `failedCount` 的第二份实现（判据共用
    ///    `isFailure`，但求和写了两遍 —— 加一类失败、或改一次 `isFailure` 的口径，
    ///    两处就会分叉，而症状是"两处的失败计数对不上"）。
    ///    `None`（还没取过）→ 0：没有数据不等于"有一堆没通过"。
    pub fn unpassed(status: Option<&VerifyStatus>) -> usize {
        VerifySummary::of(status).map_or(0, |s| Self::unpassed_of_summary(&s))
    }

    /// 从**已经算好的摘要**里取未通过项数 —— [`SidebarBadge::unpassed`] 的第二种入口。
    ///
    /// ⚠️ **它不是第二份实现**：`unpassed` 的正文逐字就是
    ///    `VerifySummary::of(status).map_or(0, |s| s.failed_count)`，也就是**这一格**。
    ///    两条路只差"手上拿到的是内核回执还是摘要"：`api::verify` 手上是摘要
    ///    （那一屏的载荷本来就是 `VerifySummary`），所以它走这一条。
    ///    分叉的症状是"侧栏徽标的数与校验屏顶上那句「N 项未通过」不一样"。
    pub fn unpassed_of_summary(summary: &VerifySummary) -> usize {
        summary.failed_count
    }
}

// ---------------------------------------------------------------------------
// 总进度（`get_tree` 的 `progress`）
// ---------------------------------------------------------------------------

/// 「总进度」那一行：百分比 / 字节 / 速度 / 完成比例。
///
/// 两个消费方**共用**这一份口径（两处显示同一个数，只在一处算 ——
/// 抄第二份就会出现"同一个进度在两处不一样"）。
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct ProgressSummary {
    /// 「45%」；总量为 0 时是占位符 `—`（口径同 [`PercentFormat`]）。
    pub percent_text: String,
    /// 「1.0 KB / 4.0 KB」。
    pub bytes_text: String,
    /// 「1.2 MB/s」；速度为 0 时是 `—`（口径同 [`SpeedFormat`]）。
    pub speed_text: String,
    /// 完成比例，**恒在 `0...1`**（含总量为 0 的 0，绝不 NaN）。
    pub fraction: f64,
}

impl ProgressSummary {
    /// 取总进度；**树必须属于这一批**（`tree_code == code`），否则 `None`。
    ///
    /// ⚠️ 那道闸不是多余的：壳先落 `load_state = .loaded(info)`、**之后**才拉 `get_tree`，
    ///    而那一行是在 `.loaded` 那一刻出现的 —— 于是这里被问到时，`tree` 可能还没到
    ///    （`None`）或者**还是上一批的**。把上一批的进度摆在**这一批**的批次摘要底下，
    ///    是一个没有任何提示的错数（静默失效）。
    ///    判据与 `BrowserSelection.on_new_manifest` 的 `tree_code == code` **同一条**，
    ///    钉住它的是 `progress_is_not_shown_when_the_tree_belongs_to_another_batch`。
    ///
    /// ⚠️ `code == None`（没有生效批次）也返回 `None`：那时"总进度"没有归属。
    pub fn of(
        tree: Option<&TreeResult>,
        tree_code: Option<&str>,
        code: Option<&str>,
    ) -> Option<ProgressSummary> {
        // 上游是 `guard let code, let tree, treeCode == code else { return nil }`：
        // 三道闸顺序无关，缺一即 `None`（`tree_code == None` 与 `code == None` 都拦得住）。
        let code = code?;
        let tree = tree?;
        if tree_code != Some(code) {
            return None;
        }

        let p = &tree.progress;
        Some(ProgressSummary {
            // 百分比与传输列表**共用** `PercentFormat`（同一个实现、同一条"总量为 0 说 —"的
            // 口径），不用内核那份 `percent` 字段：两者本来就同源（契约 §1.6），
            // 而自己再算一份就是这条口径的第二个实现。
            percent_text: PercentFormat::text(p.done_bytes, p.total_bytes),
            bytes_text: format!(
                "{} / {}",
                ByteFormat::text(p.done_bytes),
                ByteFormat::text(p.total_bytes)
            ),
            speed_text: SpeedFormat::text(p.speed),
            // 分数与传输列表**共用** `TransferRow::fraction`（同一份夹取/防 NaN 的实现）。
            fraction: TransferRow::fraction(p.done_bytes, p.total_bytes),
        })
    }
}

// ---------------------------------------------------------------------------
// 刷新失败 → 要说的那句话
// ---------------------------------------------------------------------------

/// 刷新时 `get_tree` 失败之后要说的那句话。
///
/// ⚠️ **这是给 `shell-win` 用的唯一入口，调用方自己不做映射**（全局约束 8）。
///    映射本身走 [`error_text`] —— 那是 `ClientError` → 客户可见文案的**唯一实现**
///    （`DirLoadFailure::of` / `EnqueueFeedback::failure` / `TransferActionFailure` 用的也是它）。
///    本类型**不抄第二份**：抄了就会出现"同一个错误在两处措辞不同"。
///
/// ⚠️ `refresh_verify()` 自己的失败**不走这里**：它是非抛错路径，原文由壳落在
///    `last_error` 上、由主区顶部那条常驻横幅显示 —— 在这里再显示一遍同一句话，
///    只会让真正要看的那条变淡（同 `TransferListEmpty` 的理由）。
///    这里兜的是 `get_tree()` 的**抛错**路径：它没有别的落点（约束 4）。
///
/// ⚠️ **形态偏离（W-6）**：上游是 `message(of error: Error) -> String`（收**任意** `Error`），
///    Rust 侧的对应物**只能收 [`ClientError`]**（本 crate 是强类型的，没有"任意错误"这个类型）
///    —— 与 `presentation::error_text` 记的是同一条偏离，理由与影响面见那个文件的头注释。
pub enum VerifyRefreshFailure {}

impl VerifyRefreshFailure {
    pub fn message_of(error: &ClientError) -> String {
        error_text(error)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/VerifySummaryTests.swift`（20 条，逐条对位）。
    //!
    //! 夹具走**解码**这条路：键是内核线上的 snake_case（`size_mismatch` 带下划线），
    //! 手搓一个结构体会让"壳解不解得动内核发来的那一行"这件事在测试里凭空消失 ——
    //! 而那正是裁决 V 那个坑要处理的形状。
    //!
    //! ⚠️ 本文件背着三条硬约束：
    //!   - 六类互斥穷尽，**计数为 0 的类也要有它的那一行**（显示「0 项」，不得隐藏）；
    //!   - 路径**原文照登**（不取最后一段、不规范化、不转义）；
    //!   - 以及"壳不得比内核更严"：`all_good` 不含 `unverifiable`，壳不许把它算进去。

    use super::*;
    use crate::protocol::{GlobalStat, TaskState, TransferItem};

    // -----------------------------------------------------------------------
    // 夹具
    // -----------------------------------------------------------------------

    /// 路径数组的简写（`vec!["a".to_string()]` 太长）。
    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// 一次 `verify_status` 的结果，**经解码构造**（键是内核线上的 snake_case）。
    ///
    /// ⚠️ 六个键名**逐字手写** —— 就是 `core/src/main.rs` 的 `op_verify_status`
    ///    那个 `json!` 的键，也就是 `VerifyClass::wire_key()` 给出的那六个
    ///    （变体 → 键名那张表由 `protocol.rs` 的
    ///    `verify_class_wire_keys_match_the_verify_status_receipt` 逐条钉住）。
    ///    这里的六个字面量是那条映射的**反方向证据**：信封真的按它们解。
    ///
    /// `all_good` 默认按**内核的口径**算（`bad + missing + size_mismatch + unreadable`
    /// 是否全空，见 `core/src/verify.rs` 的 `CheckResult::all_good`）—— 夹具本身不能说假话。
    /// 要造"内核与明细矛盾"的现场（`the_summary_never_says_all_clear_while_listing_a_failure` /
    /// `a_kernel_judgement_of_not_all_good_is_never_dropped`）才显式传。
    #[derive(Clone)]
    struct VerifyFx {
        ok: Vec<String>,
        bad: Vec<String>,
        missing: Vec<String>,
        size_mismatch: Vec<String>,
        unverifiable: Vec<String>,
        unreadable: Vec<String>,
        all_good: Option<bool>,
    }

    impl Default for VerifyFx {
        fn default() -> Self {
            Self {
                ok: Vec::new(),
                bad: Vec::new(),
                missing: Vec::new(),
                size_mismatch: Vec::new(),
                unverifiable: Vec::new(),
                unreadable: Vec::new(),
                all_good: None,
            }
        }
    }

    impl VerifyFx {
        fn build(&self) -> VerifyStatus {
            let all_good = self.all_good.unwrap_or(
                self.bad.is_empty()
                    && self.missing.is_empty()
                    && self.size_mismatch.is_empty()
                    && self.unreadable.is_empty(),
            );
            let obj = serde_json::json!({
                "ok": self.ok,
                "bad": self.bad,
                "missing": self.missing,
                "size_mismatch": self.size_mismatch,
                "unverifiable": self.unverifiable,
                "unreadable": self.unreadable,
                "all_good": all_good,
            });
            serde_json::from_value(obj).expect("夹具必须能解：它写的就是内核线上的那七个键")
        }
    }

    fn summary(fx: VerifyFx) -> VerifySummary {
        VerifySummary::of(Some(&fx.build())).expect("有结果就一定有一份总结")
    }

    fn row_of(s: &VerifySummary, kind: VerifyClass) -> &VerifyClassRow {
        s.rows
            .iter()
            .find(|r| r.kind == kind)
            .unwrap_or_else(|| panic!("六类里没有 {kind:?} —— 它被漏掉了"))
    }

    /// 一次 `get_tree` 的结果，**经解码构造**（只用到 `progress` 那一部分）。
    #[derive(Clone)]
    struct TreeFx {
        total_bytes: i64,
        done_bytes: i64,
        speed: i64,
        percent: i32,
    }

    impl Default for TreeFx {
        fn default() -> Self {
            Self {
                total_bytes: 4096,
                done_bytes: 1024,
                speed: 512,
                percent: 25,
            }
        }
    }

    impl TreeFx {
        fn build(&self) -> TreeResult {
            let obj = serde_json::json!({
                "tree": {},
                "flat": [],
                "default_selected": [],
                "progress": {
                    "total_bytes": self.total_bytes,
                    "done_bytes": self.done_bytes,
                    "speed": self.speed,
                    "percent": self.percent,
                },
            });
            serde_json::from_value(obj).expect("夹具必须能解：它写的就是 get_tree 的形状")
        }
    }

    /// `transfer_list` 的一项，**经解码构造**（同 `transfer_row.rs` 的 `Fixture::build`）。
    fn item(gid: &str, state: TaskState, raw_status: &str) -> TransferItem {
        let obj = serde_json::json!({
            "gid": gid,
            "total": 1000,
            "completed": 250,
            "speed": 0,
            "conns": 1,
            "state": state,
            "raw_status": raw_status,
            "error_message": "",
            "path": format!("{gid}.bin"),
        });
        serde_json::from_value(obj).expect("夹具必须能解")
    }

    fn list(items: Vec<TransferItem>) -> TransferListResult {
        TransferListResult {
            items,
            global: GlobalStat {
                download_speed: 0,
                num_active: 1,
                num_waiting: 1,
                num_stopped: 3,
            },
        }
    }

    // -----------------------------------------------------------------------
    // 六类：齐备、互斥、可判读
    // -----------------------------------------------------------------------

    /// 上游 `allSixClassesAreAlwaysPresentEvenWhenEmpty`。
    #[test]
    fn all_six_classes_are_always_present_even_when_empty() {
        // 约束 4：六类互斥穷尽，计数为 0 的类显示 0，不得隐藏。
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            ..Default::default()
        });
        assert_eq!(s.rows.len(), 6);
        assert_eq!(
            s.rows.iter().map(VerifyClassRow::count).collect::<Vec<_>>(),
            [1, 0, 0, 0, 0, 0]
        );
        // 「0 项」必须是一句**看得见的话**：靠"路径数组为空所以没东西可给"来表达 0，
        // 在客户眼里就是"这一类不见了"（约束 4 的静默失效）。顺序也一并钉住。
        assert_eq!(
            s.rows
                .iter()
                .map(VerifyClassRow::count_text)
                .collect::<Vec<String>>(),
            vec!["1 项", "0 项", "0 项", "0 项", "0 项", "0 项"]
        );
        // ⚠️ 期望值写的是**六个变体的字面量**（不是 `ALL_CLASSES`）：
        //    拿被测的那个常量当期望值就是自己证明自己。
        assert_eq!(
            s.rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [
                VerifyClass::Passed,
                VerifyClass::Mismatched,
                VerifyClass::Missing,
                VerifyClass::SizeMismatch,
                VerifyClass::Unverifiable,
                VerifyClass::Unreadable,
            ]
        );
    }

    /// 上游 `everyClassHasADistinctLabel`。
    #[test]
    fn every_class_has_a_distinct_label() {
        // ⚠️ **只断言"两两不同"是不够的**（同 `TransferRowTests` 里那段）：把 `Missing`
        //    （"文件缺失"）与 `SizeMismatch`（"大小不符"）对调、或把 `Passed` 与 `Mismatched`
        //    的图标对调，数一数照样是 6 —— 客户看到的就是"两类说法反了"。
        //    所以这里**逐类钉死**哪一类是哪个词、哪颗图标（措辞的来源见 `VerifyClass::label`）。
        // ⚠️ 这张表**不写死长度**（没有 `; 6]`）：它的长度由这六行字面量**数出来**，
        //    而下面那条断言拿它与**从枚举派生出来的** [`ALL_CLASSES`] 比 —— 两边是两个
        //    独立来源，所以**它现在能失败**。
        //    （原先写的是 `; 6]` + `expected.len() == 6`：右边那个 6 由**类型注解**保证，
        //     恒真、与"没有断言"等价 —— 复审点名，按 Ruling PP 改掉。
        //     上游那句拿的是 `VerifyClass.allCases.count`，同理是**算出来的**值。）
        let expected = [
            (VerifyClass::Passed, "校验通过", "checkmark.circle.fill"),
            (VerifyClass::Mismatched, "内容不符", "xmark.circle.fill"),
            (VerifyClass::Missing, "文件缺失", "questionmark.folder"),
            (VerifyClass::SizeMismatch, "大小不符", "ruler"),
            (VerifyClass::Unverifiable, "无法校验", "questionmark.circle"),
            (VerifyClass::Unreadable, "无法读取", "lock.slash"),
        ];
        assert_eq!(
            expected.len(),
            ALL_CLASSES.len(),
            "六类必须一类不漏地钉在这里 —— 漏掉的那类就没人守了"
        );

        let s = summary(VerifyFx::default());
        for kind in ALL_CLASSES {
            let want = expected.iter().find(|(k, _, _)| *k == kind).unwrap_or_else(|| {
                panic!(
                    "{} 没有期望值：新加一类就要在上面的表里补一行",
                    kind.wire_key()
                )
            });
            let row = row_of(&s, kind);
            assert_eq!(row.label, want.1, "{} 的标签不对", kind.wire_key());
            assert_eq!(row.icon_name, want.2, "{} 的图标不对", kind.wire_key());
        }

        // 六条文案两两不同：长得一样等于其中一类永远看不见。
        let mut labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 6, "六条文案两两不同");
        assert!(s.rows.iter().all(|r| !r.label.is_empty()));
        let mut icons: Vec<&str> = s.rows.iter().map(|r| r.icon_name.as_str()).collect();
        icons.sort_unstable();
        icons.dedup();
        assert_eq!(
            icons.len(),
            6,
            "图标同理（同 `TransferRow.icon_name` 的纪律）"
        );
    }

    /// 上游 `eachClassListsItsPathsVerbatim`。
    #[test]
    fn each_class_lists_its_paths_verbatim() {
        // 约束 3：路径原文照登 —— 不取最后一段、不规范化、不转义。
        let weird = "client-test/C24-8_×_25WS024/reads 1.fq.gz";
        let dotty = "a/./b.txt";
        let s = summary(VerifyFx {
            ok: v(&[weird]),
            bad: v(&[dotty]),
            ..Default::default()
        });
        assert_eq!(
            row_of(&s, VerifyClass::Passed).paths,
            vec![weird.to_string()]
        );
        assert_eq!(
            row_of(&s, VerifyClass::Mismatched).paths,
            vec![dotty.to_string()]
        );
        // 空的那几类**什么都不列**（不是列一个占位符、也不是列别的类的路径）。
        assert!(row_of(&s, VerifyClass::Unverifiable).paths.is_empty());
    }

    // -----------------------------------------------------------------------
    // `all_good`：内核的口径，一个字都不改
    // -----------------------------------------------------------------------

    /// 上游 `allGoodExcludesUnverifiable`。
    #[test]
    fn all_good_excludes_unverifiable() {
        // ⚠️ 内核的 `all_good` 不含 `unverifiable` —— 壳不得"更严格"地把它算进去。
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            unverifiable: v(&["u"]),
            ..Default::default()
        });
        assert!(s.all_good);
        assert_eq!(s.failed_count, 0);
        assert_eq!(s.headline(), "全部通过");
        assert_eq!(s.headline_color(), RowColor::Green);
    }

    /// 上游 `theScreenNeverSaysAllClearWhileListingAFailure`。
    ///
    /// ⚠️ **W-6（改名，判据未动）**：上游函数名里的 `Screen` 是一个**呈现对象的俗称**，
    ///    而本 crate 不含界面概念（章程见 `lib.rs` 头部）⇒ 换成中性的 `the_summary`。
    ///    断言、夹具与判据一个字节都没改。
    #[test]
    fn the_summary_never_says_all_clear_while_listing_a_failure() {
        // 内核的 `all_good` 是权威，但"一份含失败项的结果被总结成『全部通过』"是这份结果
        // 最坏的失效。总结那一行以**明细**为准；内核的判定**原样**留在 `all_good` 上
        // （照登，不二次推导）。
        let s = summary(VerifyFx {
            bad: v(&["b"]),
            all_good: Some(true),
            ..Default::default()
        });
        assert!(s.all_good);
        assert_eq!(s.headline(), "1 项未通过");
        assert_eq!(s.headline_color(), RowColor::Red);
    }

    /// 上游 `aKernelJudgementOfNotAllGoodIsNeverDropped`。
    #[test]
    fn a_kernel_judgement_of_not_all_good_is_never_dropped() {
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            all_good: Some(false),
            ..Default::default()
        });
        assert!(!s.all_good);
        assert_ne!(s.headline(), "全部通过");
        // 只有内核自相矛盾时才走得到这一支（六类里没有失败项、内核却说没全过）。
        assert_eq!(s.headline(), "内核判定未全部通过（六类里没有失败项）");
    }

    /// 上游 `anEmptyResultIsNotAnAllClear`。
    #[test]
    fn an_empty_result_is_not_an_all_clear() {
        // ⚠️ 内核在"这一批还没有文件被归类"时回的正是**六类全空 + all_good == true**
        //    （`core/src/main.rs` 把 `k.verify` 重置成 `CheckResult::default()`，
        //     而 `CheckResult::all_good()` 在四项失败全空时为真）。
        //    照登 all_good 会让一批**从没校验过**的文件显示「全部通过」—— 一张空洞的通行证。
        let s = summary(VerifyFx::default());
        assert_eq!(s.classified_count, 0);
        assert_eq!(s.headline(), "尚未校验");
        assert_eq!(s.headline_color(), RowColor::Secondary);
        assert!(
            s.all_good,
            "内核的判定照样登在 all_good 上，只是总结那一行不说『全部通过』"
        );
    }

    /// 上游 `noResultAtAllIsNotAnAllClear`。
    #[test]
    fn no_result_at_all_is_not_an_all_clear() {
        // `verify == None`（还没取过）≠「六类全 0 + 全部通过」。
        // 变成后者就是在替内核宣布一句它根本没说过的话。
        assert!(VerifySummary::of(None).is_none());
    }

    /// 上游 `everyVerdictHasItsOwnWording`。
    #[test]
    fn every_verdict_has_its_own_wording() {
        // ⚠️ 同 `every_class_has_a_distinct_label`：只断言"两两不同"会让"哪一档是哪句话"
        //    无人守（把「尚未校验」与「全部通过」对调照样全绿 —— 而那是这份结果最坏的
        //    一句话反转）。所以逐档钉死文案、颜色与图标。
        let expected: [(VerifyVerdict, &str, RowColor, &str); 5] = [
            (
                VerifyVerdict::NotVerified,
                "尚未校验",
                RowColor::Secondary,
                "clock",
            ),
            (
                VerifyVerdict::AllGood,
                "全部通过",
                RowColor::Green,
                "checkmark.seal.fill",
            ),
            (
                VerifyVerdict::Failed(1),
                "1 项未通过",
                RowColor::Red,
                "exclamationmark.triangle.fill",
            ),
            // ⚠️ 两档 `Failed` 是**刻意**的：只钉 `Failed(1)` 挡不住"把计数丢掉"的实现
            //    （写死一句「未通过」也满足上面那一行）。
            (
                VerifyVerdict::Failed(3),
                "3 项未通过",
                RowColor::Red,
                "exclamationmark.triangle.fill",
            ),
            (
                VerifyVerdict::KernelSaysNotAllGood,
                "内核判定未全部通过（六类里没有失败项）",
                RowColor::Orange,
                "exclamationmark.triangle",
            ),
        ];
        for (verdict, text, color, icon) in &expected {
            assert_eq!(verdict.text(), *text, "{} 的文案不对", verdict.text());
            assert_eq!(verdict.color(), *color, "{} 的颜色不对", verdict.text());
            assert_eq!(verdict.icon_name(), *icon, "{} 的图标不对", verdict.text());
        }

        let texts = [
            VerifyVerdict::NotVerified.text(),
            VerifyVerdict::AllGood.text(),
            VerifyVerdict::Failed(1).text(),
            VerifyVerdict::KernelSaysNotAllGood.text(),
        ];
        let distinct: std::collections::BTreeSet<&String> = texts.iter().collect();
        assert_eq!(distinct.len(), 4);
        assert!(texts.iter().all(|t| !t.is_empty()));
    }

    // -----------------------------------------------------------------------
    // `unverifiable`：中性色 + 一句"不是失败"
    // -----------------------------------------------------------------------

    /// 上游 `theUnverifiableClassIsTheOnlyNeutralColour`。
    #[test]
    fn the_unverifiable_class_is_the_only_neutral_colour() {
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            unverifiable: v(&["u"]),
            ..Default::default()
        });
        let unverifiable = row_of(&s, VerifyClass::Unverifiable);
        assert_eq!(unverifiable.color, RowColor::Secondary);
        assert!(
            s.rows
                .iter()
                .filter(|r| r.kind != VerifyClass::Unverifiable)
                .all(|r| r.color != RowColor::Secondary),
            "中性色是『不是失败』的专属标记：别的类也用上它就分不出来了"
        );
        assert!(!unverifiable.is_failure());
    }

    /// 上游 `theUnverifiableClassIsExplainedAsNotAFailure`。
    #[test]
    fn the_unverifiable_class_is_explained_as_not_a_failure() {
        let s = summary(VerifyFx {
            unverifiable: v(&["u"]),
            ..Default::default()
        });
        assert_eq!(
            row_of(&s, VerifyClass::Unverifiable).note.as_deref(),
            Some("这些文件没有可比的校验值，不是失败")
        );
        assert_eq!(
            s.rows
                .iter()
                .filter(|r| r.note.is_some())
                .map(|r| r.kind)
                .collect::<Vec<_>>(),
            [VerifyClass::Unverifiable],
            "说明只挂在 unverifiable 上（别的类没有这条歧义要消）"
        );
    }

    // -----------------------------------------------------------------------
    // 计数：哪些类算"未通过"
    // -----------------------------------------------------------------------

    /// 上游 `theOnlyGreenClassIsThePassingOne`。
    #[test]
    fn the_only_green_class_is_the_passing_one() {
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            bad: v(&["b"]),
            missing: v(&["m"]),
            ..Default::default()
        });
        assert_eq!(
            s.rows
                .iter()
                .filter(|r| r.color == RowColor::Green)
                .map(|r| r.kind)
                .collect::<Vec<_>>(),
            [VerifyClass::Passed]
        );
        assert!(s
            .rows
            .iter()
            .filter(|r| r.is_failure())
            .all(|r| r.color == RowColor::Red));
        assert_eq!(s.failed_count, 2);
        assert_eq!(s.headline(), "2 项未通过");
    }

    /// 上游 `theFailureCountCoversEveryFailureClassIncludingUnreadable`。
    #[test]
    fn the_failure_count_covers_every_failure_class_including_unreadable() {
        let s = summary(VerifyFx {
            ok: v(&["a"]),
            bad: v(&["b"]),
            missing: v(&["m"]),
            size_mismatch: v(&["s"]),
            unverifiable: v(&["v"]),
            unreadable: v(&["u"]),
            ..Default::default()
        });
        assert_eq!(
            s.failed_count, 4,
            "bad + missing + size_mismatch + unreadable：一个都不能漏"
        );
        assert_eq!(s.classified_count, 6);
        assert_eq!(s.headline(), "4 项未通过");
        assert_eq!(s.classified_text(), "已校验 6 项");
    }

    // -----------------------------------------------------------------------
    // 分区徽标
    // -----------------------------------------------------------------------

    /// 上游 `sidebarBadgeCountsEverythingNotPassed`。
    #[test]
    fn sidebar_badge_counts_everything_not_passed() {
        // 未完成 = 待下 + 下载中 + 失败（已完成与已移除都不算）。
        let list = list(vec![
            item("w", TaskState::Waiting, "waiting"),
            item("a", TaskState::Active, "active"),
            item("e", TaskState::Error, "error"),
            item("c", TaskState::Complete, "complete"),
            item("r", TaskState::Removed, "removed"),
        ]);
        assert_eq!(SidebarBadge::unfinished_transfers(Some(&list)), 3);

        // 校验结果：未通过 = bad + missing + size_mismatch + unreadable。
        let status = VerifyFx {
            ok: v(&["a"]),
            bad: v(&["b"]),
            missing: v(&["m"]),
            size_mismatch: v(&["s"]),
            unverifiable: v(&["v"]),
            unreadable: v(&["u"]),
            ..Default::default()
        }
        .build();
        assert_eq!(SidebarBadge::unpassed(Some(&status)), 4);
    }

    /// 上游 `theVerifyBadgeDoesNotCountUnverifiable`。
    #[test]
    fn the_verify_badge_does_not_count_unverifiable() {
        // 内核的 all_good 不含 unverifiable ⇒ 徽标也不能把它算成"未通过"
        // （否则徽标会挂一个红色计数、而总结那一行同时写着「全部通过」——自相矛盾）。
        let only_unverifiable = VerifyFx {
            ok: v(&["a"]),
            unverifiable: v(&["u", "v"]),
            ..Default::default()
        }
        .build();
        assert_eq!(SidebarBadge::unpassed(Some(&only_unverifiable)), 0);

        let two_failures = VerifyFx {
            bad: v(&["b"]),
            unreadable: v(&["u"]),
            ..Default::default()
        }
        .build();
        assert_eq!(SidebarBadge::unpassed(Some(&two_failures)), 2);
        assert_eq!(SidebarBadge::unpassed(None), 0);
    }

    /// 🔴 **两种入口必须数出同一个数**（`unfinished_of_rows` 对 `unfinished_transfers`）。
    ///
    /// 为什么需要这一条：`api::transfers` 手上是**已经呈现好的行**
    /// （`TransferRow`），用不了那个收 `TransferListResult` 的入口 ⇒ 判据随行带过来
    /// （`TransferRow::new` 里那一格）。两条入口各写一份"哪些算未落定"就会分叉，
    /// 而症状是**侧栏徽标上的数与传输列表里看得见的东西对不上**（没有任何东西会红）。
    ///
    /// 判别力：把 `TransferRow::new` 里那一格改成恒 `true`、或把 `is_unfinished` 的
    /// 口径只改一处 ⇒ 这一条立刻红（`3` 会变成 `5` 或 `0`）。
    #[test]
    fn the_two_badge_entries_count_the_same_thing() {
        let items = vec![
            item("w", TaskState::Waiting, "waiting"),
            item("a", TaskState::Active, "active"),
            item("e", TaskState::Error, "error"),
            item("c", TaskState::Complete, "complete"),
            item("r", TaskState::Removed, "removed"),
        ];
        let list = list(items.clone());
        let rows: Vec<TransferRow> = items.iter().map(TransferRow::new).collect();

        assert_eq!(SidebarBadge::unfinished_transfers(Some(&list)), 3);
        assert_eq!(SidebarBadge::unfinished_of_rows(&rows), 3);
        // ⚠️ **逐行**也钉一遍：只比总数的话，"少算一行、多算另一行"两边照样都是 3。
        assert_eq!(
            rows.iter().map(|r| r.counts_as_unfinished).collect::<Vec<_>>(),
            [true, true, true, false, false],
            "五档逐档：等待中/下载中/失败算，已完成/已移除不算"
        );
        // 空列表也是 0（不是"这一格不见了"）。
        assert_eq!(SidebarBadge::unfinished_of_rows(&[]), 0);
    }

    /// 🔴 **徽标的数与屏幕上那句「N 项未通过」必须是同一个数**（两条入口，一份口径）。
    ///
    /// 判别力：把 `unpassed_of_summary` 改成自己求一遍和（比如把 `unverifiable` 也算进去）
    /// ⇒ 第一条断言红；分叉的症状是"侧栏挂着 2、校验屏顶上写着 1"。
    #[test]
    fn the_summary_badge_entry_agrees_with_the_screen() {
        let status = VerifyFx {
            ok: v(&["a"]),
            bad: v(&["b"]),
            unverifiable: v(&["u"]),
            ..Default::default()
        }
        .build();
        let s = VerifySummary::of(Some(&status)).expect("有回执就恒有摘要");
        assert_eq!(SidebarBadge::unpassed_of_summary(&s), 1);
        assert_eq!(SidebarBadge::unpassed(Some(&status)), 1);
        assert_eq!(
            SidebarBadge::unpassed_of_summary(&s),
            s.failed_count,
            "侧栏那一格就是顶上那一行里的 N"
        );
    }

    /// 上游 `noTransferSnapshotMeansNoBadge`。
    #[test]
    fn no_transfer_snapshot_means_no_badge() {
        assert_eq!(SidebarBadge::unfinished_transfers(None), 0);
        let all_done = list(vec![item("c", TaskState::Complete, "complete")]);
        assert_eq!(SidebarBadge::unfinished_transfers(Some(&all_done)), 0);
    }

    // -----------------------------------------------------------------------
    // 总进度（`get_tree` 的 `progress`）
    // -----------------------------------------------------------------------

    /// 上游 `progressComesFromTheTreeAndUsesTheSharedPercentRule`。
    #[test]
    fn progress_comes_from_the_tree_and_uses_the_shared_percent_rule() {
        let tree = TreeFx {
            speed: 2048,
            ..Default::default()
        }
        .build();
        let p = ProgressSummary::of(Some(&tree), Some("C1"), Some("C1"))
            .expect("树属于这一批，就该给出总进度");
        assert_eq!(p.percent_text, "25%");
        assert_eq!(p.fraction, 0.25);
        assert_eq!(p.bytes_text, "1.0 KB / 4.0 KB");
        assert_eq!(p.speed_text, "2.0 KB/s");
    }

    /// 上游 `progressIsNotShownWhenTheTreeBelongsToAnotherBatch`。
    #[test]
    fn progress_is_not_shown_when_the_tree_belongs_to_another_batch() {
        // 与 `BrowserSelection.on_new_manifest` 同一条纪律：树必须**属于这一批**。
        // 少了这道闸，换批时会把**上一批**的进度摆在**这一批**的批次摘要底下（静默错数）。
        let tree = TreeFx::default().build();
        assert!(ProgressSummary::of(Some(&tree), Some("A"), Some("B")).is_none());
        assert!(ProgressSummary::of(Some(&tree), None, Some("B")).is_none());
        assert!(ProgressSummary::of(Some(&tree), Some("B"), None).is_none());
        assert!(ProgressSummary::of(None, Some("B"), Some("B")).is_none());
        assert!(ProgressSummary::of(Some(&tree), Some("B"), Some("B")).is_some());
    }

    /// 上游 `aBatchWithNothingToDownloadDoesNotClaimZeroPercent`。
    #[test]
    fn a_batch_with_nothing_to_download_does_not_claim_zero_percent() {
        // total == 0 时与传输列表同一口径（`PercentFormat`）：说「—」，不说「0%」。
        let tree = TreeFx {
            total_bytes: 0,
            done_bytes: 0,
            speed: 0,
            percent: 0,
        }
        .build();
        let p = ProgressSummary::of(Some(&tree), Some("C"), Some("C"))
            .expect("树属于这一批，就该给出总进度");
        assert_eq!(p.percent_text, "—");
        assert_eq!(p.fraction, 0.0);
    }

    // -----------------------------------------------------------------------
    // 刷新失败的那句话
    // -----------------------------------------------------------------------

    /// 上游 `refreshFailuresShowTheKernelMessageVerbatim`。
    ///
    /// ⚠️ **W-6（错误类型）**：上游第二例是 `CoreError.transport("管道断了")`，
    ///    Rust 侧没有带载荷的"传输失败"变体，对位的是 [`ClientError::KernelGone`]
    ///    —— 它的 `Display` 正文就是那句话在 `client.rs` 里的家
    ///    （`transfer_row.rs` 的 `action_failures_show_the_kernel_message_verbatim`
    ///    记着同一条偏离）。第一例（内核原文逐字）逐字对位。
    #[test]
    fn refresh_failures_show_the_kernel_message_verbatim() {
        // 调用方自己不做映射：`VerifyRefreshFailure` 是这一层的入口，
        // 内部走 `error_text`（`ClientError` → 客户可见文案的唯一实现）。
        assert_eq!(
            VerifyRefreshFailure::message_of(&ClientError::Kernel {
                code: "invalid_params".to_string(),
                message: "内核原文".to_string(),
            }),
            "内核原文"
        );
        assert_eq!(
            VerifyRefreshFailure::message_of(&ClientError::KernelGone),
            "内核进程已退出（管道结束）"
        );
    }
}
