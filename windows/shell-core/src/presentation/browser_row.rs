//! browser_row —— 文件浏览器那一列表的**呈现模型**：行、状态样式、选择、底部汇总。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/BrowserRow.swift`（逐字对位）。
//!
//! ⚠️ 全部是**纯函数**（上游只 `import Foundation`、**不 import SwiftUI**）：颜色用
//!    [`RowColor`] 这个**枚举**表示，不是某一种具体的颜色值 —— 在 `shell-core` 里引入
//!    绘制依赖会破坏"本 crate 不含界面概念"这条线。枚举 → 实际颜色的那一步留在
//!    `shell-win`，它是纯绘制决定、断言不出有意义的东西（与 `engine_status.rs` 的
//!    同一处留白同形）。
//!
//! ⚠️ **为什么这些映射在这里、不在 `shell-win`**（全局约束 8）：判据是"如果一段代码你能
//!    写出一个断言，它就不属于 `shell-win`"。下面每一条都能写出断言，其中有几条背着硬约束：
//!    目录路径要壳自己拼（`list_dir` 的目录项没有 `path` 键）、四态两两不同（少一个状态
//!    就是静默失效，约束 4）、排序（不平铺就是另一种表现）。

use serde::Serialize;
use crate::presentation::breadcrumb::Breadcrumb;
use crate::presentation::delivery_summary::TimestampPresentation;
use crate::presentation::format::ByteFormat;
use crate::presentation::RowColor;
use crate::protocol::{DirEntry, FileState, FlatEntry, TreeNode, TreeResult};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// 行的判别键。**来自内核的 `"type"`**（`"dir"` / `"file"`），
/// **不是** `is_dir` 之类的布尔。
///
/// ⚠️ **形态偏离（W-6），两处（都记账，不省）**：
///    ① **位置**：上游是嵌在 `BrowserRow` 里的 `BrowserRow.Kind`；Rust 不能在 `struct` 里
///       定义类型，所以提到模块层并冠以宿主名（同 `transfer_row.rs` 的 `EngineBannerKind`）。
///    ② **变体名**：上游写的是 `case dir, file`（小写，因为那个枚举是 `: String` 带原始值），
///       这里按 Rust 的变体命名惯例写成 `Dir` / `File` —— **这是改名，不是"一字没改"**，
///       同 `presentation/mod.rs` 的 `RowColor`（`case secondary, blue, …` → `Secondary, Blue, …`）
///       与 `protocol.rs` 的 `FileState`（`pending` → `Pending`）那两处的做法。
///    ③ **原始值被丢掉**：上游那个 `String` 原始值就是内核给的两个字面量 `"dir"` / `"file"`，
///       本移植**没有**留一个 `raw_value()` 之类的出口。实测 `grep -rn "kind.rawValue" macos/`
///       **零命中**（那一侧同样没有一处读它）⇒ 这处差异**不可观测**。记在这里是为了
///       "偏离清单完整"：日后若真要用它，**不要另写一份字面量**，按 `protocol.rs` 里
///       `VerifyClass::wire_key()` 那样从类型派生。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum BrowserRowKind {
    Dir,
    File,
}

/// 平铺列表里的一行：**已经算好的呈现值**，调用方只剩绑定。
///
/// 「名称 · 大小 · 时间 · 状态」四列的值都在这里成形，`shell-win` 里不拼字符串、不判断类型。
///
/// ⚠️ **形态偏离（W-6）**：上游写的是 `Equatable, Sendable, Identifiable`，这里对位成
///    `Clone, PartialEq, Eq, Debug` —— `Sendable` 在 Rust 里由类型系统默认保证
///    （没有内部可变性/裸指针就自动成立），`Debug` 是断言失败时要看得见值才加的
///    （`assert_eq!` 需要它）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct BrowserRow {
    /// 判别键，见 [`BrowserRowKind`]。
    pub kind: BrowserRowKind,
    /// 显示名（路径最后一段的原文）。
    pub name: String,
    /// 完整路径（约束 3：原文，逐字）。
    pub path: String,
    /// 目录的子项数；文件是 `None`。
    pub children_count: Option<i64>,
    /// 文件大小；**目录是 `None`**（"没有大小这回事" ≠ "大小是 0"：目录的大小要等内核
    /// 展开才知道，而壳不展开目录，约束 1）。
    pub size: Option<i64>,
    /// 「大小」那一列显示的文字：文件是 [`ByteFormat`] 的结果，目录是「N 项」。
    pub detail_text: String,
    /// 「时间」那一列显示的文字（**源文件**的修改时间，内核原文经 `SourceTimeText` ——
    /// 它与上游一样是**模块私有**的，所以这里写成代码字形而不是文档链接）。
    pub source_time_text: String,
    /// 「状态」那一列。
    pub state: RowStateStyle,
    /// 「名称」那一列的系统图标名（上游是 SF Symbol 名，与 `EngineStatusPresentation`
    /// 的 `systemImage` 同形；本 crate 只搬运这个字符串，不认识它）。
    pub icon_name: String,
}

impl BrowserRow {
    /// 这一行的身份 = 它的路径。
    ///
    /// ⚠️ 上游还有 `var id: String { path }`，是给那个"按身份做差分"的列表协议用的
    ///    （`Identifiable`）。本 crate 不依赖任何界面框架，但这一条**照搬**：它是 `path`
    ///    的别名、零语义，删掉只会让移植面多一处说不清的缺口。
    pub fn id(&self) -> &str {
        &self.path
    }

    /// 一个目录项 → 一行。
    ///
    /// ⚠️ `parent` 只有**目录项**用得上：`list_dir` 的目录项**没有 `path` 键**，
    ///    路径只能由壳拼（[`Breadcrumb::join`]）；文件项有 `path`，逐字用它 ——
    ///    不要"顺手"重新拼一遍文件路径，那是在拿壳的拼接覆盖内核给的原文（约束 3）。
    pub fn of(entry: &DirEntry, parent: &str) -> BrowserRow {
        match entry {
            DirEntry::Dir {
                name,
                children_count,
            } => BrowserRow {
                kind: BrowserRowKind::Dir,
                name: name.clone(),
                path: Breadcrumb::join(parent, name),
                children_count: Some(*children_count),
                size: None,
                detail_text: format!("{children_count} 项"),
                // 目录**没有**"源文件时间"这回事（与它的大小同理：那得等内核展开才知道，
                // 而壳不展开目录，约束 1）—— 恒为占位字形。
                source_time_text: SourceTimeText::NO_VALUE.to_string(),
                state: RowStateStyle::of(None),
                icon_name: "folder".to_string(),
            },
            DirEntry::File(f) => BrowserRow {
                kind: BrowserRowKind::File,
                name: f.name.clone(),
                path: f.path.clone(),
                children_count: None,
                size: Some(f.size),
                detail_text: ByteFormat::text(f.size),
                source_time_text: SourceTimeText::of(f.source_mtime.as_deref()),
                state: RowStateStyle::of(Some(f.state)),
                icon_name: "doc".to_string(),
            },
        }
    }

    /// 一层目录 → 排好序的一列表。
    ///
    /// **目录在前、同组内按名字升序。**
    ///
    /// ⚠️ 排序用 `str` 的 `Ord`（**与语言环境无关**），绝不用任何"本地化比较"：后者的结果
    ///    随机器的语言环境变，于是"排序对不对"这件事在客户机上和在测试里是两个答案
    ///    （同 `ByteFormat` 不用 `ByteCountFormatter` 的理由）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游的 `<` 走 Swift 的 Unicode 规范序（先按规范等价折叠、
    ///    再比码位），Rust 的 `str: Ord` 是**码位序**（UTF-8 字节序与码位序一致）。
    ///    两者在"同一个字的两种归一化写法不会同时出现"的输入上逐一相同；只在同一段文字
    ///    既有 NFC 又有 NFD 写法时才有先后之别 —— 而清单里的名字是内核从文件系统逐字搬来的
    ///    原文（约束 3），要客户端机器上真的并存两份仅归一化形态不同的名字才会撞上。
    ///    **判据（目录在前 + 名字升序 + 与语言环境无关）逐条不变。**
    pub fn rows(entries: &[DirEntry], parent: &str) -> Vec<BrowserRow> {
        let mut out: Vec<BrowserRow> = entries.iter().map(|e| BrowserRow::of(e, parent)).collect();
        // 上游的 `if a.kind != b.kind { return a.kind == .dir }`：两组之间恒目录在前，
        // 组内才比名字。写成穷尽的 `match`，别用 `default` 盖掉。
        out.sort_by(|a, b| match (a.kind, b.kind) {
            (BrowserRowKind::Dir, BrowserRowKind::File) => Ordering::Less,
            (BrowserRowKind::File, BrowserRowKind::Dir) => Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        out
    }
}

/// 「时间」那一列显示什么。
///
/// 输入是内核给的**原文**（`FileNode.source_mtime`，ISO 8601 带 `+08:00`），内核当不透明
/// 字符串搬运、壳也不解析它（约束 3）。
///
/// ⚠️ 三件事：① 缺值与空串都出 `—`（那是"这一格没有值"，不是"时间是空的"）；
///    ② 能认的走 [`TimestampPresentation`]（它已经保证**解析不了就原样返回**）；
///    ③ 所以壳**永远不会**显示 `Invalid Date` 这种自己编的文案（约束 C-7）——
///      "原样返回"这条契约由 `TimestampPresentation` 一处定义，这里只是沿用它，
///      **不要**在本文件另造一套格式化（那是把同一件事写出两个会分叉的答案）。
///
/// ⚠️ 缺值与空串在这里**故意合流**：调用方要的只是"这一格显示什么"，而"键不在"与"值是空串"
///    本来就该长得一样。两者的**区分**留在 `FileNode::source_mtime` 那一层
///    （`Option<String>`：`None` vs `Some("")`）—— 在那里合并，就再也分不出
///    "解码把键丢了"与"内核真的没给值"。
enum SourceTimeText {}

impl SourceTimeText {
    /// 这一格没有值时的占位字形。
    ///
    /// ⚠️ 与 `format.rs` 的 `SpeedFormat`/`PercentFormat` 同一个字形（`—`）：
    ///    空串在界面上就是"什么都没说"，而"这一格没有值"本身也是一件要说出来的事（约束 4）。
    ///    目录行也用它（[`BrowserRow::of`] 的目录支）—— 目录没有"源文件时间"这回事。
    const NO_VALUE: &'static str = "—";

    fn of(raw: Option<&str>) -> String {
        match raw {
            // `guard let raw, !raw.isEmpty else { return noValue }` 的两个落点。
            None | Some("") => Self::NO_VALUE.to_string(),
            Some(r) => TimestampPresentation::text(r),
        }
    }
}

/// 「状态」那一列的呈现：标签 + 颜色。
///
/// ⚠️ 四态**两两不同**是硬要求（约束 4：不得静默失效 —— 把两种状态画成一样，等于其中一个
///    永远不会出现在界面上）。钉住它的是
///    `each_of_the_four_states_maps_to_its_own_label_and_color`。
///
/// ⚠️ 这里**没有**"未知状态"这一支，而且这是有意的：状态来自 [`FileState`]（内核的
///    `state_name()` 四态），一个壳不认识的字符串会让 [`DirEntry`] 与 `FileNode` 的**解码**
///    直接失败（那是一种响亮失败）—— 不会静默造一个假状态，所以也轮不到这里兜底。
///    剩下唯一一种"没有状态"是**目录行**：它在四态模型里本来就不参与合成
///    （`get_tree` 的 `flat` 只列文件），落到 [`RowStateStyle::None`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum RowStateStyle {
    Pending,
    Downloading,
    Complete,
    Failed,
    /// 没有状态（目录行）。占位字形，不是"第五种状态"。
    None,
}

impl RowStateStyle {
    /// **链**：每一格指向它的下一格，链尾是 `Option::None`。
    ///
    /// ⚠️ **穷尽 `match`** ⇒ 本枚举多一格、而这里没跟着处理 ⇒ `E0004` **编不过**。
    ///    于是"加了一格"这件事**不可能被静默忽略**：它必须先经过这条链。
    const fn next(self) -> Option<RowStateStyle> {
        match self {
            RowStateStyle::Pending => Some(RowStateStyle::Downloading),
            RowStateStyle::Downloading => Some(RowStateStyle::Complete),
            RowStateStyle::Complete => Some(RowStateStyle::Failed),
            RowStateStyle::Failed => Some(RowStateStyle::None),
            RowStateStyle::None => Option::None,
        }
    }

    /// 本枚举的格数 —— **沿上面那条链数出来的**，不是手写的数字。
    ///
    /// ⚠️ 这正是 Ruling QQ 要的形状：手写"5"的话，它与枚举之间的关系只有"人记得"这一条；
    ///    从链上数出来之后，链一变、它就跟着变（而链**必须**变 —— `E0004` 挡着）。
    pub const COUNT: usize = {
        let mut n = 1usize;
        let mut cur = RowStateStyle::Pending;
        loop {
            match cur.next() {
                Some(next) => {
                    cur = next;
                    n += 1;
                }
                Option::None => break,
            }
        }
        n
    };

    /// 全部格 —— 对位上游的 `CaseIterable`（`allCases`）。
    ///
    /// ⚠️ **由上面那条链展开**，不是手写的第二份清单（源那句 `allCases.count` 是**活**的，
    ///    而"手写数组 + `[..; 5]`"是死的：长度写在类型注解里 ⇒ 那条 `len()` 断言恒真）。
    ///    链一变，本清单自动跟着长；长过头了则由**与字面量对照**的那条断言接住
    ///    （`state_label_is_never_empty`）。
    pub const ALL: [RowStateStyle; Self::COUNT] = {
        let mut out = [RowStateStyle::Pending; Self::COUNT];
        let mut i = 0usize;
        let mut cur = RowStateStyle::Pending;
        loop {
            out[i] = cur;
            i += 1;
            match cur.next() {
                Some(next) => cur = next,
                Option::None => break,
            }
        }
        out
    };

    /// 呈现用的标签。**任何一支都不许是空白**（约束 4）。
    pub fn label(&self) -> &'static str {
        match self {
            RowStateStyle::Pending => "待下载",
            RowStateStyle::Downloading => "下载中",
            RowStateStyle::Complete => "已完成",
            RowStateStyle::Failed => "失败",
            // 占位字形与 `format.rs` 的 `SpeedFormat`/`PercentFormat` 同一个（`—`）：
            // 空串在界面上就是"什么都没说"，而"这一格没有值"本身也是一件要说出来的事。
            RowStateStyle::None => "—",
        }
    }

    /// 语义色（规格 §7.2）。**不含任何具体颜色值**，由 `shell-win` 映射。
    pub fn color(&self) -> RowColor {
        match self {
            RowStateStyle::Pending => RowColor::Secondary,
            RowStateStyle::Downloading => RowColor::Blue,
            RowStateStyle::Complete => RowColor::Green,
            RowStateStyle::Failed => RowColor::Red,
            RowStateStyle::None => RowColor::Secondary,
        }
    }

    /// 内核的四态 → 样式；`None`（目录行）→ [`RowStateStyle::None`]。
    ///
    /// ⚠️ 这个 `match` **穷尽**在 `Option<FileState>` 上：内核日后多出第五态时它会**编译不过**
    ///    ——那正是上游那条 `FileState.allCases.count == 4` 运行时断言想要的效果，
    ///    而这里提前到了编译期。
    pub fn of(state: Option<FileState>) -> RowStateStyle {
        match state {
            Some(FileState::Pending) => RowStateStyle::Pending,
            Some(FileState::Downloading) => RowStateStyle::Downloading,
            Some(FileState::Complete) => RowStateStyle::Complete,
            Some(FileState::Failed) => RowStateStyle::Failed,
            None => RowStateStyle::None,
        }
    }
}

// ---------------------------------------------------------------------------
// 选择
// ---------------------------------------------------------------------------

/// `on_load` 的结果：要不要复位 + 把"上一次显示过的码"记成什么。
///
/// ⚠️ **形态偏离（W-6）**：上游的字段是 `let`、由 `init` 写入，这里字段全 `pub`、
///    用结构体字面量构造（同 `breadcrumb.rs` 的 `DirLoadFailure` 那条记录），
///    不另加一个只会被绕过的构造函数。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestLoad {
    /// 这是一批**新**清单吗（⇒ 浏览位置与勾选面立刻复位，不等这一批的树到）。
    pub is_a_new_manifest: bool,
    /// 新的"上一次**显示**过的码"。
    pub last_displayed_code: Option<String>,
}

/// 一次播种的结果：**该播成什么** + **这次播种属不属于换批**。
///
/// ⚠️ 两件事必须能分开（复审重要 2）：拿到 `selection` 就播下去，但"**顺带把浏览位置打回
///    根目录**"只允许在 `is_a_new_batch` 为真时做 —— 同码重载（含**内核崩溃后的自动恢复**）
///    是一次**非用户动作**引发的加载，把用户停在的子目录静默换掉、界面上却一句话都没有，
///    是约束 4 要防的那类形态。判据是纯值（有单测），调用方只做分派（约束 8）。
///
/// ⚠️ **形态偏离（W-6）**：上游那个 `public init(selection:isANewBatch:)` 在这里没有对应物
///    —— 字段全 `pub`，要手工构造时用结构体字面量（同 `DirLoadFailure` 那条记录）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestSeeding {
    /// 该播成的默认勾选面（`get_tree` 的 `default_selected`）。
    pub selection: BTreeSet<String>,
    /// 这是一批**新**清单吗（交付码与"上一次播种过的那一批"不同）。
    ///
    /// ⚠️ 判据是**"上一次播种的是不是这一批"**，不是"这一代有没有播过种"：后者对同码重载
    ///    同样为真，而那恰恰是要与换批区分开的两种情形。
    pub is_a_new_batch: bool,
}

/// 文件浏览器与"是哪一批清单"有关的那部分状态 —— **纯值，迁移全在这里**。
///
/// ---------------------------------------------------------------------------
/// 🔴 **对位记账（R-5）：这个类型的判决在**生产路径上零调用者**，行为由前端承接**
/// ---------------------------------------------------------------------------
/// `api/**` 与 `shell-win/**` 里**没有一处**调它 —— 文件页活在 JS 里
/// （`web/js/screens/files.js`），那边的注释**逐字**把 `ManifestTracking` 当作对位。
///
/// ⚠️ **这一格就是本代最严重那个缺陷的现场**：下面 `seed` 的文档逐字写着
///    「少了 `seeded_generation`，用户在同一个批次里进个目录、切一下分区回来，
///    **攒的选择就被抹掉**」—— 判据早在 Rust 里、有测试、有文档，
///    而它**没有被接上**，于是同一个缺陷在 JS 里又长了一遍，最后在真机被用户撞到。
///
/// ⚠️ **它与 `NoteDrafts` / `SettingsForm::matches` 不同：这一格没有差分对照，这不是漏了。**
///    JS 那一侧**不是抄件、是另一套模型**：它用"这一屏还挂着没有"
///    （`kept.loaded === undefined`）代替了 Rust 的"代数"（`seeded_generation`）——
///    两边的**输入字母表都不一样**，"同一组输入喂给两边"这句话在那里没有定义，
///    硬做出来的差分只会钉住一种**编码选择**，不是行为。
///    ⇒ 它的回归网在**行为**那一侧：`windows/scripts/check_files_screen.sh` 的 `keep` 档
///      （选一个文件 → 去传输列表看一眼 → 切回来 → 点下载 ⇒ 下的必须是那一个）。
///    ⇒ 三处坐标（本判据 / `files.js` / `keep` 档）由
///      `windows/scripts/check_presentation_mirrors.sh` 守着 —— 承接点被删掉/搬走时它会红。
///
/// ⚠️ 承重事项 A 有**两半**，这个类型把它们收在同一个值里，好让两半都有能被单测钉住的落点：
///    - **复位那一半**（[`ManifestTracking::display`]）：`code` 一变就把"当前在哪一层 /
///      勾了什么"作废，**不等树到**；
///    - **播种那一半**（[`ManifestTracking::seed`]）：换批之后**必须**还能让这一批的
///      `default_selected` 播下来。这一半坏掉的样子是**静默**的：换批时若把"已经为哪一批
///      播过种"记着不放，那么用户在 B 的树到之前回到 A 时，`seeded_code` 还是 `"A"`
///      ⇒ `on_new_manifest` 判成"同一批" ⇒ **A 的默认选中面永远播不下来** ⇒ 用户开箱看到
///      「已选 0 项」，而**界面上一点异常都没有**。所以换批时 `seeded_code` 跟着一起清
///      —— `display` 的返回值就是那个时刻。
///
/// ⚠️ 三个字段的语义**必须分开**（混用就是 `A→B→A` 那一族缺陷的来源）：
///    - `last_displayed_code` = "上一次**显示**过的码"：**显示**那一刻推进，是**复位**的对照值；
///    - `seeded_code` = "**已经为哪一批播过种**"（`None` = 当前显示的这一批还没播过）：
///      **播种成功**那一刻才推进，且**换批时清空**，是**播种**的批次对照值；
///    - `seeded_generation` = "**已经为哪一次加载播过种**"（上游是 `AppModel.loadGeneration`）：
///      同样是播种成功那一刻才推进。它是**播种的次数**上的对照值，与批次无关 ——
///      两者都要，缺一不可（见下面 `seed` 里那两段注释）。
///
/// ⚠️ 因此**不能**拿 `last_displayed_code` 去当 `seed` 的对照值（这是审查建议里被否决的那半）：
///    显示那一刻它就已经等于新码了，`seed` 会判成"这一批已经播过" ⇒ **第一次加载的
///    默认选中面永远播不下来**（`the_first_load_seeds_its_default_selection` 钉着这一点）。
///    反过来说，"播种"这件事必须有一个**独立于显示**的记账，才能既"每批只播一次"、
///    又"换批后还能再播"。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManifestTracking {
    last_displayed_code: Option<String>,
    seeded_code: Option<String>,
    /// 已经为**哪一次加载**播过种（上游是 `AppModel.loadGeneration`；`None` = 还没播过）。
    ///
    /// ⚠️ **上游阶段 D 任务 B 加的**：原先只有 `seeded_code`（按**批次**记账），于是
    ///    **同码重载**被判成"这一批已经播过种" ⇒ 返回 `None` ⇒ 内核明明刚重新规划过整批
    ///    （`default_selected` 是新信息）、界面上却一点都不动 —— 用户读成"我重载了，
    ///    什么也没发生"（诊断报告 §6.1 的猜想 2，已用测试证实）。
    ///    判据从"哪一批"换成"哪一次加载"之后，同码重载天然会重播；而"同一批里换目录 /
    ///    切分区回来 / 树到位后再进来"这些**没有发生加载**的重入仍然落在同一代里
    ///    ⇒ **照样不重播**（用户跨目录攒的选择还是不会被抹掉）。
    seeded_generation: Option<i64>,
}

impl ManifestTracking {
    pub fn new() -> ManifestTracking {
        ManifestTracking {
            last_displayed_code: None,
            seeded_code: None,
            seeded_generation: None,
        }
    }

    /// 界面显示了一批清单（`load_state` 变成"已加载"）。
    ///
    /// **返回值 = 换批**（`true`）⇒ 调用方复位浏览位置与勾选面。树到没到**不影响**这个判断。
    /// ⚠️ 这个返回值不要丢掉：它就是"复位"这件事的唯一触发点。
    pub fn display(&mut self, code: Option<&str>) -> bool {
        let step = BrowserSelection::on_load(code, self.last_displayed_code.as_deref());
        self.last_displayed_code = step.last_displayed_code;
        // 换批 ⇒ 这一批还没播过种（哪怕它的码在更早的时候播过 —— A→B→A 就是这个情形）。
        // ⚠️ 两个记账**一起**作废：`seeded_code` 管"哪一批"、`seeded_generation` 管
        //    "哪一次加载"，只清一个就留下"半个已播种"的中间态。顺带保住一条自愈：
        //    `display` 与 `seed` 的**次序没有保证**（上游记着的那个已知潜在竞态）——
        //    万一 `display` 排在 `seed` 后面把这一批的记账清掉，也必须留下一条
        //    "下一次 `task_key` 变化还能补播"的路 —— 清掉代数那一段就是那条路。
        if step.is_a_new_manifest {
            self.seeded_code = None;
            self.seeded_generation = None;
        }
        step.is_a_new_manifest
    }

    /// 这一批的树到位时该播什么 + **这次播种算不算换批**
    /// （`None` = 不播：这一次加载已经播过了，或树还不是这一批的）。
    ///
    /// **只有真的播成功才推进那两个记账** —— 树没到就一直推迟，而"树到"会让 `.task` 的 key
    /// 变化、再进来播一次（那正是 `task_key` 里 `tree_code` 那一段的存在理由）。
    ///
    /// ⚠️ `generation` 在一次成功的加载里推进（上游是 `AppModel.loadGeneration`）。
    ///    两个对照值各管一件事，**都不能删**：
    ///      - `seeded_generation`：**同一次加载内不重播**。少掉它就退化成"每次 `task_key`
    ///        变化都重播"——用户在同一个批次里进个目录、切一下分区回来，攒的选择就被抹掉
    ///        （那正是承重事项 A 的另一半）；
    ///      - `seeded_code`（经 `previous_code` 传给 `on_new_manifest`）：**同码重载**要能
    ///        重播。同一次加载里它总是等于当前码，所以下面先按"新一代"把它作废，
    ///        `on_new_manifest` 那条按批次判定的守卫才会放行。
    pub fn seed(
        &mut self,
        code: &str,
        tree_code: Option<&str>,
        tree: Option<&TreeResult>,
        generation: i64,
    ) -> Option<ManifestSeeding> {
        // `guard seededGeneration != generation else { return nil }`：`None != 任何代数`
        // 为真，所以"还没播过种"照样往下走。
        if self.seeded_generation == Some(generation) {
            return None;
        }
        // ⚠️ **先判"算不算换批"，再动作**：判据是"上一次播种的是不是这一批"
        //    （`seeded_code != code`）。它必须在下面那行把记账清掉**之前**读 ——
        //    清完就再也分不出"同码重载"与"换了一批"了（两者在这里都是"新一代"）。
        let is_a_new_batch = self.seeded_code.as_deref() != Some(code);
        // 新的一次加载 ⇒ 上一次那条"已经为这一批播过种"的记账作废。**同码重载正是靠这一行
        // 才播得下来**：少了它，`on_new_manifest` 会按 `code` 判成"同一批" ⇒ 静默不播。
        if self.seeded_code.as_deref() == Some(code) {
            self.seeded_code = None;
        }
        // `?`：推迟（树还没到 / 还是上一批的 / 判成同一批）。**不消费任何记账**——
        // 这正是"推迟"="之后还能补播"的落点。
        let picked =
            BrowserSelection::on_new_manifest(code, self.seeded_code.as_deref(), tree_code, tree)?;
        self.seeded_code = Some(code.to_string());
        self.seeded_generation = Some(generation);
        Some(ManifestSeeding {
            selection: picked,
            is_a_new_batch,
        })
    }
}

/// 勾选面的规则（默认选中面 / 全选当前层 / 换批复位的判据 / `.task` 的触发键）。
///
/// ⚠️ **形态偏离（W-6）**：上游是 `public enum BrowserSelection`（只当命名空间用），
///    这里用空 `enum` + 固有 `impl` 对位（同 `breadcrumb.rs` / `format.rs` 的写法）。
pub enum BrowserSelection {}

impl BrowserSelection {
    /// `.task(id:)` 的触发键 —— 换批、换层、或**这一批的树刚刚到位**就重新进来一次。
    ///
    /// ⚠️ 它是个**纯函数**（所以能被单测钉住）：那一层不单测（约束 8），而"什么变化要重新
    ///    进一次"恰恰是本项目栽过的那类**静默失效**。四段缺一不可：
    ///      - `code`：换批；
    ///      - `path`：换层；
    ///      - `tree_code`：**这一批的树到位**。内核侧先落"已加载"、**之后**才拉 `get_tree`
    ///        （那是一条真实的竞速），所以第一次进来时树多半还没到（`tree_code == None`），
    ///        `on_new_manifest` 会**推迟**播种。正是这一段让树到位之后能**再进一次**、
    ///        把默认选中面播下去；删了它，"推迟播种"就变成"永不播种"（用户开箱看到
    ///        「已选 0 项」，而界面上一点异常都没有）。
    ///      - `generation`：**这一批又被成功加载了一次**（上游是 `AppModel.loadGeneration`）。
    ///        同码重载时前三个分量**一个字都没变**，而内核已经重新规划过整批 ⇒ 默认勾选面
    ///        必须重播。这一段就是那个唯一的入口：少了它，重进根本不会发生，
    ///        "同码重载重播默认面"就无从谈起（诊断报告 §6.1 的猜想 2）。
    ///
    /// ⚠️ 前三段用**长度前缀**而不是裸拼 `|`：这三个值都是清单/内核给的原文，里面可以有 `|`
    ///    （约束 3：壳不规范化、不转义），裸拼会让 `("A|B", "", None)` 与 `("A", "B|", None)`
    ///    撞成同一个键 —— 撞了就是"该重进的时候没重进"，也就是这条防线要防的那件事。
    ///    `generation` 是**纯整数、且排在最后**，前面那段又是长度前缀、自定界的，
    ///    所以直接接在 `|` 后面不会与树码里的 `|` 混淆。
    ///
    /// ⚠️ **形态偏离（W-6）**：长度取的是 `chars().count()`。上游的 `code.count` 数的是
    ///    **字素簇**（grapheme cluster），Rust 的 `chars()` 数的是 **Unicode 标量**；
    ///    两者在"一个字素簇就是一个标量"的输入上完全相同，只在组合序列（基字 + 组合记号、
    ///    ZWJ 表情）上不同。**这条差异不影响本函数的性质**：长度前缀要的只是"这一段文本
    ///    的长度能把它自定界、别的切分解析不通"，任何单射的长度函数都满足；而且这把键
    ///    只在壳自己这一侧用，**不跨语言、不与内核交换**。
    pub fn task_key(code: &str, path: &str, tree_code: Option<&str>, generation: i64) -> String {
        let tree_segment = match tree_code {
            Some(t) => format!("{}:{}", t.chars().count(), t),
            None => "nil".to_string(),
        };
        format!(
            "{}:{}|{}:{}|{}|{}",
            code.chars().count(),
            code,
            path.chars().count(),
            path,
            tree_segment,
            generation
        )
    }

    /// `code` 与"拿来对照的那个码"不同 ⇒ **这是一批新清单**。
    ///
    /// ⚠️ **它只是"码变了吗"这一条取值规则**，不是"复位"这件事本身 —— 两个调用点传进来的
    ///    对照值是**两种不同语义**，共用一个函数并**不**保证两边一致：
    ///      - `on_new_manifest`（播种）传的是「**已经为哪一批播过种**」（`seeded_code`）；
    ///      - `on_load`（复位）传的是**上一次显示过的码**（`last_displayed_code`）。
    ///    两者在 `A→B→A` 这条路上会分叉，分叉的后果与堵法见 `on_load` 与 [`ManifestTracking`]。
    ///    （别把这条函数当"承重事项 A 的守卫"读：它守不住 A 的任何东西。）
    pub fn is_a_new_manifest(code: &str, previous_code: Option<&str>) -> bool {
        Some(code) != previous_code
    }

    /// 加载到一批清单时的**显示状态迁移**：`(上一次**显示**过的码, 这一次加载到的码)` →
    /// `(要不要复位, 新的"上一次显示过的码")`。
    ///
    /// 复位 = "浏览器当前在哪一层"与勾选面**立刻**作废，**不等这一批的树到**。
    ///
    /// ⚠️ 为什么"不等树"是硬要求：`on_new_manifest` 里那条"树还没到就推迟"推迟的是**播种**，
    ///    **不是复位**。把复位一起推迟的后果，是"码已变、这一批的树还没到"的那段间隙里，
    ///    浏览器带着**上一批**的 `current_path` 与勾选面去面对新批次：
    ///      - 它会对**上一批的路径**发 `list_dir`（那些路径在新批次里可能指向别的文件）；
    ///      - 汇总行那颗「下载选中」继续拿着**上一批的路径** —— 用户以为在下 B 批的文件，
    ///        实际发出去的 `enqueue` 是 A 批的路径。
    ///
    /// ⚠️⚠️ **对照值必须是"上一次显示过的码"，不能是"已经播过种的码"**（`seeded_code`）。
    ///    后者只在**播种成功**时写，于是 `A→B→A` 这条路上它是坏的：
    ///      ① 显示 A（`seeded_code = "A"`）→ ② 切到 B（复位一次，但 `seeded_code` **仍是 "A"**
    ///      —— 复位有意不消费它）→ ③ 用户在 B 的树到之前勾了几项（勾选不依赖树）→
    ///      ④ 又回到 A：拿 `seeded_code` 当对照 ⇒ `"A" != "A"` 为假 ⇒ **不复位** ⇒
    ///      B 的勾选面被拿去给 A 发 `enqueue`。**这正是承重事项 A 要修的那个缺陷，
    ///      只是换了顺序。**（同一现场还有"静默"那一半：A 的 `default_selected` 再也播不下来
    ///      —— `seeded_code` 还是 `"A"`，`on_new_manifest` 判成"同一批"，于是永远不播种。
    ///      那一半由 [`ManifestTracking::display`] 在换批时把 `seeded_code` 清掉来堵。）
    ///
    /// ⚠️ `code == None`（加载中 / 失败 / 内核回"没有生效的批次"）时**既不复位、也不推进**
    ///    那个记住的码：一次 `None` 抖动不该让"紧接着的同一批加载"被误判成换批（那会把用户
    ///    在同一批里攒的选择抹掉），也不该把已经记住的码冲成 `None`（那会让下一次同批加载
    ///    又变成"换批"）。
    pub fn on_load(code: Option<&str>, last_displayed_code: Option<&str>) -> ManifestLoad {
        match code {
            None => ManifestLoad {
                is_a_new_manifest: false,
                last_displayed_code: last_displayed_code.map(|c| c.to_string()),
            },
            Some(c) => ManifestLoad {
                is_a_new_manifest: BrowserSelection::is_a_new_manifest(c, last_displayed_code),
                last_displayed_code: Some(c.to_string()),
            },
        }
    }

    /// 新一批清单到达时，选择该变成什么。
    ///
    /// - 返回 `None` = **保持不动**（既不改选择，也**不消费** `previous_code`）。
    /// - 返回一个集合 = 换批了，重设成这个。
    ///
    /// ⚠️ `previous_code` 的语义是「**已经为哪一批播过种**」（`seeded_code`），
    ///    **不是**"上一次显示过的码" —— 后者在显示那一刻就等于新码了，会把第一次播种也判成
    ///    "已经播过"。两者为什么必须分开：见 [`ManifestTracking`] 的注释。
    ///
    /// ⚠️ 默认选中面来自 `get_tree` 的 `default_selected`（内核给的"所有非 complete 的文件"），
    ///    **不是**当前层的全部条目 —— 这是"点开就能直接点下载"的关键。
    ///
    /// ⚠️ **树必须属于这一批**（`tree_code == code`），否则**推迟**（返回 `None`，不消费）。
    ///    理由是一条真实的竞速：内核侧先落"已加载"，之后才去拉 `get_tree`；而那一层是在
    ///    "已加载"那一刻出现的 —— 于是这里被问到时，`tree` 可能**还没到**（`None`）或者
    ///    **还是上一批的**。
    ///    - 前者若当作"空集"播下去，这一次记账就被消费掉了，**此后没有任何重播路径**
    ///      （`task_key` 里没有树），用户开箱看到「已选 0 项」而"点开就能直接点下载"
    ///      **静默失效**；
    ///    - 后者更糟：把**上一批的路径**播进新批次（那些路径在这一批里可能指向别的文件）。
    ///    越大的批次（树越大、`get_tree` 越慢）越容易输掉这场竞速 —— 所以判据放在这里
    ///    （纯函数）而不是调用方那里（约束 8）。钉住它的是 `seeding_waits_for_this_batches_tree`。
    pub fn on_new_manifest(
        code: &str,
        previous_code: Option<&str>,
        tree_code: Option<&str>,
        tree: Option<&TreeResult>,
    ) -> Option<BTreeSet<String>> {
        // 还是同一批（刷新、目录来回切、切分区回来）：不动，别抹掉用户攒起来的选择。
        // 共用的只是"码变了吗"这条**规则**（`is_a_new_manifest`）；这里的 `previous_code` 是
        // **最后一次播种用过的码**，与 `on_load` 传的"上一次显示过的码"是两种语义 ——
        // 别把这条 guard 当成"复位也已经做过了"的保证。
        if !BrowserSelection::is_a_new_manifest(code, previous_code) {
            return None;
        }
        // 这一批的树还没到位（没到 / 还是上一批的）：**推迟**，不消费 `previous_code`。
        if tree_code != Some(code) {
            return None;
        }
        let tree = tree?;
        Some(tree.default_selected.iter().cloned().collect())
    }

    /// `⌘A`：**当前这一层**的全部条目（不是整棵树、也不是 `default_selected`）。
    ///
    /// 目录也在里面：内核在 `enqueue` 里按前缀展开目录 —— 展开是内核的事（约束 1）。
    pub fn all(rows: &[BrowserRow]) -> BTreeSet<String> {
        rows.iter().map(|r| r.path.clone()).collect()
    }

    /// 整批的**全部文件**（`get_tree` 的 `flat` 只列文件 —— 目录不在里面）。
    ///
    /// 两个用途，**别混**：
    ///   ① 判断"勾选面是不是覆盖了整批"（`DownloadTargets::paths` 的 `all_paths`）——
    ///      覆盖了就必须发 `[]`（约束 C-3：超限请求不会被报错，只会把客户端静默堵死）；
    ///   ② 汇总行「全选」这一项的输入 —— 它的语义就是"整批全部文件"，按下去正好落进
    ///      ① 那一支（全选 ⇒ `paths: []` ⇒ 下全部待下载）。
    ///
    /// ⚠️ 「全选**本层**」是**另一件事**，用 [`BrowserSelection::all`]：它选的是**这一层
    ///    看得见的行（含目录）**。（⌘A 与右键菜单那条「全选本层」都走它。）
    ///
    /// ⚠️ **有意偏离简报里的一段注释**（约束 11）：简报给的原文说"**不要**用它去当『全选』
    ///    的输入"，但同一份简报的步骤 13 与 README 第 20 条第 5 款都说「全选」勾的是
    ///    "整批全部文件"。按**行为**取步骤 13，理由：只有"勾选面 == 整批文件"才会走到
    ///    `paths: []` 那一支 —— 而"支持全选，即下载全部文件"正是本任务要交付的东西；
    ///    改用 [`BrowserSelection::all`] 的话，只要批次里还有子目录，两边就永远不相等，
    ///    那条 `[]` 分支形同虚设（请求体随批次大小线性增长，正是 C-3 要防的）。
    /// ⚠️ **C-3 判据在生产上的那份 `all_paths` 走的是它的兄弟
    /// [`BrowserSelection::all_files_of`]**（`load_delivery` 的树那一侧 —— 命令层手上
    /// 有的就是它）；本函数留在 `flat` 这一侧（手上有 `get_tree` 回执的地方用得到）。
    /// 两者是**同一个集合**，等价性同样是那条用例钉着。
    pub fn all_files(flat: &[FlatEntry]) -> BTreeSet<String> {
        flat.iter().map(|f| f.path.clone()).collect()
    }

    /// 整批的**全部文件**，取自 **`load_delivery` 回执里那棵树**（[`TreeNode`]）——
    /// [`BrowserSelection::all_files`] 的另一个来源，**算的是同一个集合**。
    ///
    /// ## ⚠️ 为什么会有第二个来源（这不是重复实现，是"手上已经有答案了"）
    ///
    /// [`BrowserSelection::all_files`] 吃 `get_tree` 的 `flat[]`，而那份扁平表**只有拉过
    /// `get_tree` 的那段代码手上有**。命令层做 `enqueue` 时手边有的是**加载回执**
    /// （`SessionView::load` 里那个 `DeliveryInfo`），它带着**同一棵树**。
    /// 内核那两条出口本来就是同一份数据：`op_get_tree` 的 `flat` 取
    /// `view::build_tree(&m.files)` 的文件叶节点，`op_load_delivery` 的 `tree` 取
    /// 同一个 `build_tree(&m.files)` 的 `node_json` 形状（`core/src/main.rs` 各一处）
    /// ⇒ **文件集合逐字相同**。
    ///
    /// ⇒ 于是"整批全选 ⇒ 发 `[]`"（约束 C-3）不必为了一个已经拿到的答案**再问内核一次**：
    ///    `get_tree` 会把整棵树重新传一遍，而那份清单就在会话里。两条来源的等价由
    ///    `the_two_receipts_of_one_manifest_name_the_same_files` 钉着（它**不**用同一个夹具
    ///    喂两边：两份回执各写一份线上原文，那是内核真会发的两种形状）。
    ///
    /// ⚠️ **目录不进这个集合**（树里的目录节点不带路径，文件叶节点的 `path` 才是清单原文）
    ///    —— 与 `flat` 只列文件同一个事实。
    ///
    /// ⚠️ 顺序：结果是 `BTreeSet`，遍历顺序本身就是按 `path` 升序（与 `all_files` 同一个
    ///    口径，理由见 `download_targets.rs` 的模块头）。树是 `BTreeMap`，递归顺序也稳定。
    pub fn all_files_of(tree: &TreeNode) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        Self::collect_files(tree, &mut out);
        out
    }

    /// [`BrowserSelection::all_files_of`] 的递归那一半（`Dir` 的 `children` 要一路走到底）。
    ///
    /// ⚠️ **空树**（`"tree": {}` ⇒ [`TreeNode::Empty`]，零文件的交付批次）落到第一个分支
    ///    ⇒ 空集。那正是 C-3 那条判据里"一批是空的时不算整批"那一支的输入。
    fn collect_files(node: &TreeNode, out: &mut BTreeSet<String>) {
        match node {
            TreeNode::Empty => {}
            TreeNode::File(leaf) => {
                // 约束 3：`path` 是清单原文，逐字收下（不 trim、不折叠 `//`、不动非 ASCII）。
                out.insert(leaf.path.clone());
            }
            // 目录节点**不**带路径：`node_json` 只给文件叶节点发 `path`
            // （目录的名字在 `children` 的键上）。所以这里只往下走，不往集合里放东西。
            TreeNode::Dir { children, .. } => {
                for child in children.values() {
                    Self::collect_files(child, out);
                }
            }
        }
    }
}

/// 底部状态栏那一行：「已选 N 项 · 合计大小」。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct SelectionSummary {
    /// 「已选 3 项」。
    pub count_text: String,
    /// 「合计 6.8 KB」。
    pub size_text: String,
}

impl SelectionSummary {
    /// 勾选面 → 那一行。
    ///
    /// - `count_text` 数的是**勾选的条目数**（目录也算一项 —— 它确实被选中了）；
    /// - `size_text` 只累加**在 `sizes` 里有大小**的那些。目录在 `flat` 里没有大小
    ///   （`get_tree` 的 `flat` 只列文件），所以它对合计的贡献是 0。壳**不**自己展开目录
    ///   去猜一份大小（约束 1：展开是内核的事）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游用 `addingReportingOverflow`（`+=` 在 Swift 里溢出会
    ///    trap），这里用 `checked_add` —— 判据逐字一致（溢出即夹到 `i64::MAX`，
    ///    此后任何非负增量都保持夹住），只是 Rust 的对应物。一组畸形（或恶意）的大小
    ///    不该让整个进程垮掉（同 `PercentFormat` 的乘法守卫）。
    pub fn of(selected: &BTreeSet<String>, sizes: &BTreeMap<String, i64>) -> SelectionSummary {
        let mut total: i64 = 0;
        for path in selected {
            let Some(one) = sizes.get(path) else {
                continue;
            };
            total = total.checked_add((*one).max(0)).unwrap_or(i64::MAX);
        }
        SelectionSummary {
            count_text: format!("已选 {} 项", selected.len()),
            size_text: "合计 ".to_string() + &ByteFormat::text(total),
        }
    }

    /// `get_tree` 的 `flat[]` → 路径 → 大小。**跨目录**的合计要靠它（当前层只有一部分）。
    pub fn size_index(flat: &[FlatEntry]) -> BTreeMap<String, i64> {
        let mut out: BTreeMap<String, i64> = BTreeMap::new();
        for f in flat {
            out.insert(f.path.clone(), f.size);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/BrowserRowTests.swift`（39 条，逐条对位）。
    //!
    //! ⚠️ 夹具一律是**内核会发的那种线上 JSON 文本**（键是 snake_case，目录项**没有 `path` 键**），
    //!    经 `serde_json` 解成 [`ListDirResult`] / [`TreeResult`] —— 手搓结构体会让
    //!    "壳解不解得动内核的输出"在测试里凭空消失（`protocol.rs` 顶部的同一条理由）。

    use super::{
        BrowserRow, BrowserRowKind, BrowserSelection, ManifestTracking, RowStateStyle,
        SelectionSummary, SourceTimeText,
    };
    use crate::presentation::delivery_summary::TimestampPresentation;
    use crate::presentation::format::ByteFormat;
    use crate::presentation::RowColor;
    use crate::protocol::{DirEntry, FileNode, FileState, ListDirResult, TreeNode, TreeResult};
    use std::collections::{BTreeMap, BTreeSet};

    /// 一层目录的线上原文（`list_dir` 的 result）。
    /// 路径是清单原文：`×` 与空格逐字，不得转义。
    const LIST_DIR_WIRE: &str = r#"{"path":"client-test/C24-8_×_25WS024",
 "entries":[{"type":"file","name":"QC 图.png",
             "path":"client-test/C24-8_×_25WS024/QC 图.png","crc64":"",
             "size":7000,"completed":0,"total":7000,"speed":0,"state":"pending","err":""},
            {"type":"dir","name":"Zeta","children_count":1},
            {"type":"file","name":"reads.fq.gz",
             "path":"client-test/C24-8_×_25WS024/reads.fq.gz","crc64":"9988776655443322110",
             "size":2048,"completed":2048,"total":2048,"speed":0,"state":"complete","err":""},
            {"type":"dir","name":"Figure","children_count":3}]}"#;

    /// 根那一层：只有一个顶层目录（用来钉"根下不得出现前导 /"）。
    const ROOT_WIRE: &str = r#"{"path":"","entries":[{"type":"dir","name":"client-test","children_count":2}]}"#;

    const PARENT: &str = "client-test/C24-8_×_25WS024";

    // -----------------------------------------------------------------------
    // 选择那几条用的树夹具
    // -----------------------------------------------------------------------

    const TREE_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"a.bin","name":"a.bin","size":2048,"state":"pending"},
         {"path":"b.bin","name":"b.bin","size":3072,"state":"complete"}],
 "default_selected":["a.bin"],
 "progress":{"total_bytes":5120,"done_bytes":3072,"speed":0,"percent":60}}"#;

    /// **上一批**的树。它的 `default_selected` 与 [`TREE_WIRE`] **不同**（`z.bin`）——
    /// 这样"新批次被播上了旧批次的路径"才是**可观测**的（两边一样的话，播错了也看不出来）。
    const TREE_OLD_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"z.bin","name":"z.bin","size":1024,"state":"pending"}],
 "default_selected":["z.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}"#;

    /// **另一批**的树：`default_selected` 与上面两份**都不同**（`b.bin`）—— 用来钉
    /// "换批播的是**这一批**的面，不是在别处播过的那一份"。
    const TREE_B_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"b.bin","name":"b.bin","size":1024,"state":"pending"}],
 "default_selected":["b.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}"#;

    /// `flat` 里 `path` 与 `name` **不同**（有前缀、带 `×` 与空格）—— 用来钉"取的是 `path`"。
    /// [`TREE_WIRE`] 那两条的 path 与 name 恰好一样，"取错字段"在它上面**不可观测**。
    const TREE_WITH_PREFIX_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"C24-8_×_25WS024/Figure/QC 图.png","name":"QC 图.png","size":1,"state":"pending"}],
 "default_selected":[],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}"#;

    // -----------------------------------------------------------------------
    // 时间列那几条用的夹具
    //
    // ⚠️ 夹具**不动**上面那两个。`LIST_DIR_WIRE` 里**没有** `source_mtime` 这个键，
    //    它本身就是"老清单"那一半的现场（约束 C-2）——"缺键"只有靠它才验得到，
    //    而"缺键"与"空串"是**两种不同的输入**，两种都要落到 `—`。
    // -----------------------------------------------------------------------

    /// 新交付的线上原文：三个文件各带一种 `source_mtime`，外加一个目录。
    /// 键是 `source_mtime`（snake_case），与 [`FileNode::source_mtime`] 的字段名同形 ——
    /// 这条对照关系本身就是断言的一部分（写错一个字母这里就先红了）。
    const LIST_DIR_WIRE_WITH_TIMES: &str = r#"{"path":"t",
 "entries":[{"type":"file","name":"has-time.txt",
             "path":"t/has-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":"2026-09-14T12:00:00+08:00"},
            {"type":"file","name":"empty-time.txt",
             "path":"t/empty-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":""},
            {"type":"file","name":"junk-time.txt",
             "path":"t/junk-time.txt","crc64":"","size":1,"completed":0,"total":1,
             "speed":0,"state":"pending","err":"",
             "source_mtime":"待定"},
            {"type":"dir","name":"d","children_count":2}]}"#;

    /// 老清单的**整棵树**（`get_tree` 的 result）：文件叶节点里**没有** `source_mtime` 键
    /// （形状与 [`LIST_DIR_WIRE`] 同一个来源：内核那两条 JSON 出口）。
    const OLD_TREE_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{
    "t":{"type":"dir","name":"t","children":{
        "a.txt":{"type":"file","name":"a.txt","path":"t/a.txt","crc64":"",
                 "size":1,"completed":0,"total":1,"speed":0,"state":"pending","err":""}}}}},
 "flat":[{"path":"t/a.txt","name":"a.txt","size":1,"state":"pending"}],
 "default_selected":["t/a.txt"],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}"#;

    // -----------------------------------------------------------------------
    // 助手
    // -----------------------------------------------------------------------

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn entries(json: &str) -> Vec<DirEntry> {
        serde_json::from_str::<ListDirResult>(json)
            .expect("这是内核会发的线上 JSON，必须解得出")
            .entries
    }

    fn rows(json: &str, parent: &str) -> Vec<BrowserRow> {
        BrowserRow::rows(&entries(json), parent)
    }

    fn row(name: &str) -> BrowserRow {
        row_in(LIST_DIR_WIRE, PARENT, name)
    }

    /// 任意一份夹具里的某一行。
    fn row_in(json: &str, parent: &str, name: &str) -> BrowserRow {
        let all = rows(json, parent);
        match all.iter().find(|r| r.name == name) {
            Some(hit) => hit.clone(),
            None => panic!("夹具里没有 {name}"),
        }
    }

    /// 一层目录里的**文件叶节点**（`DirEntry` → `FileNode`；目录项没有 `FileNode`）。
    fn file_nodes(json: &str) -> Vec<FileNode> {
        entries(json)
            .into_iter()
            .filter_map(|e| match e {
                DirEntry::File(f) => Some(f),
                DirEntry::Dir { .. } => None,
            })
            .collect()
    }

    /// 某个文件叶节点的 `source_mtime`。
    ///
    /// ⚠️ 用这个而不是在断言里写 `file_nodes(…).iter().find(…).map(…)`：后者是两层 `Option`
    ///    叠在一起，读起来是"比较两个可选值"还是"比较值与 None"分不清（本助手把它压回一层）。
    fn source_mtime(json: &str, name: &str) -> Option<String> {
        file_nodes(json)
            .into_iter()
            .find(|f| f.name == name)
            .flatten_source_mtime()
    }

    /// `Option<FileNode>` 里那一层 `source_mtime`（只为把上面那条压回一层 `Option`）。
    trait FlattenSourceMtime {
        fn flatten_source_mtime(self) -> Option<String>;
    }
    impl FlattenSourceMtime for Option<FileNode> {
        fn flatten_source_mtime(self) -> Option<String> {
            self.and_then(|f| f.source_mtime)
        }
    }

    fn tree(json: &str) -> TreeResult {
        serde_json::from_str(json).expect("这是内核会发的线上 JSON，必须解得出")
    }

    /// **两两不同**（对位上游的 `Set(...).count == 4`）。
    ///
    /// ⚠️ `RowColor` 只派生 `PartialEq`（本文件的移植面不许改 `presentation/mod.rs` 里
    ///    那个既有枚举的派生列表），所以不能用集合去重 —— 这条助手是等价写法。
    fn pairwise_distinct<T: PartialEq>(items: &[T]) -> bool {
        (0..items.len()).all(|i| ((i + 1)..items.len()).all(|j| items[i] != items[j]))
    }

    /// 内核四态（`FileState`）的**链**：每一格指向下一格，链尾是 `Option::None`。
    ///
    /// ⚠️ **穷尽 `match`** ⇒ `FileState` 多一格（内核日后真的加了第五态）而这里没跟着处理
    ///    ⇒ `E0004` **编不过**。这就是源的 `FileState.allCases` 那条线在 Rust 里的落点：
    ///    `CaseIterable` 是自动的，Rust 没有 ⇒ 用一条**必须穷尽**的 `match` 把它变成
    ///    "改不动"（而不是"忘了改"）。
    const fn file_state_next(state: FileState) -> Option<FileState> {
        match state {
            FileState::Pending => Some(FileState::Downloading),
            FileState::Downloading => Some(FileState::Complete),
            FileState::Complete => Some(FileState::Failed),
            FileState::Failed => Option::None,
        }
    }

    /// 内核四态**有几个** —— 沿链数出来的，不是手写的数字（同 [`RowStateStyle::COUNT`]）。
    const FILE_STATE_COUNT: usize = {
        let mut n = 1usize;
        let mut cur = FileState::Pending;
        loop {
            match file_state_next(cur) {
                Some(next) => {
                    cur = next;
                    n += 1;
                }
                Option::None => break,
            }
        }
        n
    };

    /// 内核四态的清单 —— **由上面那条链展开**。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游写的是 `FileState.allCases`（运行时枚举）；
    ///    Rust 侧没有 `CaseIterable`，这里用"穷尽 `match` 的链 + 从链里展开的清单"对位。
    ///    **它保证的事与它不保证的事都要说清**（本项目的纪律：范围说准，别说大）：
    ///      · **保证**（实测）：内核加第五态之后，**连"补一个顺手的映射"这个最小补丁都编不过**
    ///        —— `E0004` 会同时点在上面这条链与 [`RowStateStyle::of`] 上。
    ///        这一条正好是"显式数组 + `len()`"那版漏掉的：那版能编过、而且 189 条全绿。
    ///      · **保证**：把新格子**接进链里**之后，本清单**自动跟着长**（长度与内容都来自链）
    ///        ⇒ `each_of_the_four_states_maps_to_its_own_label_and_color` 里那条**与字面量
    ///        对照**的断言立刻红（左边 5 格、右边 4 格）。
    ///      · **不保证**（实测，如实记账）：把新格子接成"**链尾之外的第二个终点**"
    ///        （写 `Cancelled => Option::None` 而不动 `Failed => Option::None`）⇒ 它从这条链里
    ///        **不可达**，清单不会变长，**没有任何检查够得着那一格**。
    ///        这是"清单由派生驱动"这件事的固有边界：Rust **没有变体反射**，而任何由派生驱动的
    ///        检查都到不了"从派生里不可达的那一格"。补救只有"引入变体枚举宏"（**新增依赖**，
    ///        与本 crate 的依赖白名单冲突）——所以这一格**留在账上**，不假装它被守住了。
    const ALL_FILE_STATES: [FileState; FILE_STATE_COUNT] = {
        let mut out = [FileState::Pending; FILE_STATE_COUNT];
        let mut i = 0usize;
        let mut cur = FileState::Pending;
        loop {
            out[i] = cur;
            i += 1;
            match file_state_next(cur) {
                Some(next) => cur = next,
                Option::None => break,
            }
        }
        out
    };

    // -----------------------------------------------------------------------
    // 行的判别键与路径
    // -----------------------------------------------------------------------

    /// 上游 `rowsCarryTheTypeDiscriminatorFromTheCore`。
    #[test]
    fn rows_carry_the_type_discriminator_from_the_core() {
        // 判别键是 `"type"`（`"dir"` / `"file"`），**不是** `is_dir` 之类的布尔
        // （`core/src/main.rs` 的 `entries_json`）。
        assert_eq!(row("Figure").kind, BrowserRowKind::Dir);
        assert_eq!(row("reads.fq.gz").kind, BrowserRowKind::File);
    }

    /// 上游 `dirRowsGetTheirPathRebuiltByTheShell`。
    #[test]
    fn dir_rows_get_their_path_rebuilt_by_the_shell() {
        // ⚠️ `list_dir` 的目录项**没有 `path` 键**（只有 `children_count`）——
        //    所以目录的路径只能由壳拼：父路径 + "/" + name。
        //    变异体：把目录项的 path 直接取 `row.name`（或留空）⇒ 双击进去会发一条错路径。
        assert_eq!(row("Figure").path, "client-test/C24-8_×_25WS024/Figure");
        assert_eq!(row("Zeta").path, "client-test/C24-8_×_25WS024/Zeta");

        // 根那一层：不得出现前导 "/"（`join(parent: "", …)` 那条规则在真实输入上的样子）。
        let top = rows(ROOT_WIRE, "");
        assert_eq!(
            top.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            vec!["client-test"]
        );
    }

    /// 上游 `fileRowsKeepTheKernelPathVerbatim`。
    #[test]
    fn file_rows_keep_the_kernel_path_verbatim() {
        // 文件**有** `path` 键，逐字用它的（约束 3：不规范化、不转义）。
        assert_eq!(
            row("QC 图.png").path,
            "client-test/C24-8_×_25WS024/QC 图.png"
        );
        assert_eq!(row("QC 图.png").name, "QC 图.png", "`×` 与空格原样显示");
    }

    // -----------------------------------------------------------------------
    // 三列的值
    // -----------------------------------------------------------------------

    /// 上游 `dirRowShowsChildCountAndNoSize`。
    #[test]
    fn dir_row_shows_child_count_and_no_size() {
        let d = row("Figure");

        assert_eq!(d.children_count, Some(3));
        // 「没有大小这回事」与「大小是 0」不是一件事：目录的大小得等内核展开才知道
        // （壳不展开目录，约束 1），所以它必须是 None，而不是 0。
        assert_eq!(d.size, None);
        assert_eq!(d.detail_text, "3 项", "目录那一格显示的是子项数，不是大小");
        assert_eq!(d.state, RowStateStyle::None, "目录没有四态");
        assert_eq!(d.icon_name, "folder");
    }

    /// 上游 `fileRowShowsSizeAndStateColor`。
    #[test]
    fn file_row_shows_size_and_state_color() {
        let f = row("QC 图.png");

        assert_eq!(f.size, Some(7000));
        assert_eq!(f.children_count, None);
        assert_eq!(
            f.detail_text,
            ByteFormat::text(7000),
            "口径复用 ByteFormat（1024 进制、一位小数）"
        );
        assert_eq!(f.detail_text, "6.8 KB");
        assert_eq!(f.state, RowStateStyle::Pending);
        assert_eq!(f.state.color(), RowColor::Secondary);
        assert_eq!(f.icon_name, "doc");

        // 已完成那个也要看一眼：`Complete` 与 `Pending` 必须是两回事。
        assert_eq!(row("reads.fq.gz").state, RowStateStyle::Complete);
        assert_eq!(row("reads.fq.gz").state.color(), RowColor::Green);
    }

    /// 上游 `eachOfTheFourStatesMapsToItsOwnLabelAndColor`。
    #[test]
    fn each_of_the_four_states_maps_to_its_own_label_and_color() {
        // 内核的四态（`FileState`）一个都不能少、一个都不能并。
        //
        // ⚠️ **这条断言（而不是一条 `len()`）承担"就是四态"**：左边的 `styles` 是从**链里
        //    展开的清单**跑出来的，右边是**字面量** ——
        //      · 内核加第五态 ⇒ 先撞 `E0004`（`file_state_next` / `RowStateStyle::of`）；
        //      · 把它接进链之后 ⇒ 清单自动变长 ⇒ **左边 5 格、右边 4 格 ⇒ 这条红**。
        //    （上一版这里写的是 `assert_eq!(ALL_FILE_STATES.len(), 4)`：长度写在类型注解里
        //     ⇒ 常量比常量 ⇒ **永远不可能失败**。Ruling QQ 之后换成现在这条。）
        let styles: Vec<RowStateStyle> = ALL_FILE_STATES
            .iter()
            .map(|s| RowStateStyle::of(Some(*s)))
            .collect();
        let labels: Vec<&str> = styles.iter().map(|s| s.label()).collect();
        let colors: Vec<RowColor> = styles.iter().map(|s| s.color()).collect();

        assert_eq!(
            styles,
            vec![
                RowStateStyle::Pending,
                RowStateStyle::Downloading,
                RowStateStyle::Complete,
                RowStateStyle::Failed
            ],
            "内核的四态，一个都不能少、一个都不能并；顺序即 FileState 的声明序"
        );
        assert!(pairwise_distinct(&labels), "四个标签必须两两不同");
        assert!(pairwise_distinct(&colors), "四个颜色必须两两不同");
        assert_eq!(labels, vec!["待下载", "下载中", "已完成", "失败"]);
        assert_eq!(
            colors,
            vec![
                RowColor::Secondary,
                RowColor::Blue,
                RowColor::Green,
                RowColor::Red
            ]
        );
    }

    /// 上游 `stateLabelIsNeverEmpty`。
    #[test]
    fn state_label_is_never_empty() {
        // 任何一格都不许是空白 —— 空白在界面上就是"什么都没有"，而约束 4 要的是
        // 「不得静默失效」：状态说不出来，也是一件要说出来的事。
        //
        // ⚠️ **"一共五格、且就是这五格"由这条与字面量对照的断言承担**（同
        //    `each_of_the_four_states…` 那条的纪律）：左边是从**链里展开**的清单，
        //    右边是字面量 ⇒ 本枚举加一格（先撞 `E0004`）再接进链之后，**这条红**。
        //    （上一版这里写的是 `assert_eq!(RowStateStyle::ALL.len(), 5)`：长度写在
        //     `ALL` 的类型注解里 ⇒ 常量比常量 ⇒ 永远不可能失败。Ruling QQ 之后换掉。）
        assert_eq!(
            RowStateStyle::ALL,
            [
                RowStateStyle::Pending,
                RowStateStyle::Downloading,
                RowStateStyle::Complete,
                RowStateStyle::Failed,
                RowStateStyle::None
            ],
            "四态 + 「没有状态」（目录行）—— 一共五格，顺序即声明序"
        );
        for s in RowStateStyle::ALL {
            assert!(!s.label().trim().is_empty(), "{s:?} 的标签是空白");
            assert_ne!(s.label(), "nil", "不得把 `nil` 这种字面量漏到界面上");
            // ⚠️ **形态偏离（W-6）**：上游那句守的是 Swift 的 `nil`；Rust 侧对应的
            //    漏字面量是 `None``（字符串化的 `Option`），同一处防线一并钉住。
            assert_ne!(s.label(), "None", "不得把 `None` 这种字面量漏到界面上");
        }
        assert_eq!(RowStateStyle::of(None), RowStateStyle::None);
        assert_eq!(
            RowStateStyle::of(None).label(),
            "—",
            "占位字形与 `format.rs` 的 SpeedFormat 同一个"
        );
    }

    // -----------------------------------------------------------------------
    // 排序
    // -----------------------------------------------------------------------

    /// 上游 `rowsAreSortedDirsFirstThenByName`。
    #[test]
    fn rows_are_sorted_dirs_first_then_by_name() {
        // 目录在前、同组内按名字升序。夹具顺序是
        // `[QC 图.png, Zeta, reads.fq.gz, Figure]`（故意打乱），所以：
        //   不排序            → [QC…, Zeta, reads…, Figure]
        //   只按名字排        → [Figure, QC 图.png, Zeta, reads…]（目录被拆散）
        //   文件在前          → [QC…, reads…, Figure, Zeta]
        //   分组但不排序      → [Zeta, Figure, QC…, reads…]
        //   比较器恒真        → 每组按夹具顺序**倒序** → [Figure, Zeta, reads…, QC…]（文件那组反了）
        // 五种都会被下面这条断言判红。
        //
        // ⚠️ **夹具的同类项必须有一组是"升序"的**（这里文件组是 `[QC 图.png, reads.fq.gz]`）。
        //    这条不是随手排的：上一版夹具两组**恰好都是降序**（`[reads…, QC…]` / `[Zeta, Figure]`），
        //    于是"比较器恒真 ⇒ 每组倒序"的输出**正好等于期望顺序**，那个变异体活了下来。
        //    夹具给变异体打掩护，是本项目栽过八次的那类问题。
        let names: Vec<String> = rows(LIST_DIR_WIRE, PARENT)
            .iter()
            .map(|r| r.name.clone())
            .collect();
        assert_eq!(names, vec!["Figure", "Zeta", "QC 图.png", "reads.fq.gz"]);
    }

    // -----------------------------------------------------------------------
    // 选择：默认选中面 / 全选当前层 / 底部汇总
    // -----------------------------------------------------------------------

    /// 上游 `aNewManifestReseedsTheSelectionFromTheCore`。
    #[test]
    fn a_new_manifest_reseeds_the_selection_from_the_core() {
        // ⚠️ 关键：批次加载后的默认选择来自 `get_tree` 的 `default_selected`
        //    （内核给的"所有非 complete 的文件"）—— **不是**当前层的全部条目。
        let seeded = BrowserSelection::on_new_manifest(
            "C24-8",
            None,
            Some("C24-8"),
            Some(&tree(TREE_WIRE)),
        );
        assert_eq!(seeded, Some(set(&["a.bin"])));
        assert_eq!(
            seeded.map(|s| s.contains("b.bin")),
            Some(false),
            "已完成的文件不该被默认勾上"
        );
    }

    /// 上游 `theSameManifestDoesNotWipeTheUsersSelection`。
    #[test]
    fn the_same_manifest_does_not_wipe_the_users_selection() {
        // 同一批（刷新 / 目录来回切 / **切分区回来**）时返回 None = **不动**：用户跨目录攒的
        // 选择不能被一次平凡的重绘抹掉。
        assert_eq!(
            BrowserSelection::on_new_manifest(
                "C24-8",
                Some("C24-8"),
                Some("C24-8"),
                Some(&tree(TREE_WIRE))
            ),
            None
        );

        // 换一批才重设（而且是**清空后重设**，不是并集）。
        assert_eq!(
            BrowserSelection::on_new_manifest(
                "BBB-2",
                Some("C24-8"),
                Some("BBB-2"),
                Some(&tree(TREE_WIRE))
            ),
            Some(set(&["a.bin"]))
        );
    }

    /// 上游 `seedingWaitsForThisBatchesTree`。
    #[test]
    fn seeding_waits_for_this_batches_tree() {
        // ⚠️⚠️ 这条挡的是**一条真实的竞速**：内核侧先落"已加载"、**之后**才去拉 `get_tree`，
        //    而那一层是在"已加载"那一刻出现的 —— 所以壳来问"该播什么"时，树**多半还没到**。
        //
        //     判据两条：
        //       ① 一个批次的默认选择，**绝不能被另一个批次的树播种**；
        //       ② 树还没到时，播种**不得被消费** —— 要推迟到这一批的树到位那一刻再播。
        //
        //     "消费"这件事发生在这个纯函数的**调用方**。所以这里用"返回 None = 不动"来表达
        //     "不许消费"：调用方只有在拿到非 None 时才记账。
        let new_tree = tree(TREE_WIRE);
        let old_tree = tree(TREE_OLD_WIRE);

        // ① 树还没到（`tree_code == None`）：推迟 —— **不是**"播一个空集"。
        //    播空集就等于消费掉了这一次机会，而 `task_key` 里当时没有树，
        //    此后**再没有任何重播路径** ⇒ 用户开箱看到「已选 0 项」，
        //    "点开就能直接点下载"**静默失效**。
        assert_eq!(
            BrowserSelection::on_new_manifest("NEW", None, None, None),
            None
        );

        // ①' 树还是**上一批**的：更不能播（那些路径在这一批里可能指向别的文件）。
        assert_eq!(
            BrowserSelection::on_new_manifest("NEW", None, Some("OLD"), Some(&old_tree)),
            None
        );
        // 反向的一半：真拿上一批的树去播，播出来的确实是**别的路径** —— 这条断言
        // 让"不播"这件事有判别力（否则"恒返回 None"也能让上面两条过）。
        assert_eq!(
            old_tree.default_selected,
            vec!["z.bin"],
            "上一批的默认选中面与这一批不同"
        );

        // ② 这一批的树到位了：播。
        assert_eq!(
            BrowserSelection::on_new_manifest("NEW", None, Some("NEW"), Some(&new_tree)),
            Some(set(&["a.bin"]))
        );
    }

    /// 上游 `aTreeClaimingThisBatchButMissingIsNotSeeded`。
    #[test]
    fn a_tree_claiming_this_batch_but_missing_is_not_seeded() {
        // 防御性的一支：`tree_code` 说"就是这一批"、`tree` 却是 None（正常构造不出来 ——
        // 内核侧两行是一起落的）。这时**推迟**比"播空集"安全：真出现了也要能自愈。
        assert_eq!(
            BrowserSelection::on_new_manifest("NEW", None, Some("NEW"), None),
            None
        );
    }

    /// 上游 `selectAllTakesOnlyTheCurrentLevel`。
    #[test]
    fn select_all_takes_only_the_current_level() {
        let level = rows(LIST_DIR_WIRE, PARENT);
        let all = BrowserSelection::all(&level);

        assert_eq!(all, level.iter().map(|r| r.path.clone()).collect());
        assert_eq!(all.len(), 4, "当前层四项");
        // 目录也被选上（内核在 enqueue 里按前缀展开目录 —— 展开是内核的事，约束 1）。
        assert!(all.contains("client-test/C24-8_×_25WS024/Figure"));
        // 别的层不在里面。
        assert!(!all.contains("client-test"));
    }

    /// 上游 `allFilesTakesEveryFilePathVerbatim`。
    #[test]
    fn all_files_takes_every_file_path_verbatim() {
        // `all_files` 是**整批的文件**（`flat` 只列文件，目录不在里面），它是"勾选面覆盖了
        // 整批吗"的对照值 —— 判据是"覆盖整批 ⇒ 发 `[]`"（约束 C-3：超限请求不报错，
        // 只把客户端静默堵死）。
        let files = BrowserSelection::all_files(&tree(TREE_WIRE).flat);
        assert_eq!(files, set(&["a.bin", "b.bin"]), "整批的文件，一条不少");

        // ⚠️ 取的是 **`path`**，不是 `name`：内核给的原样（约束 3）。
        //    这一条有判别力 —— 夹具里两者**不同**（`TREE_WIRE` 里它们恰好一样，取错也看不出来）。
        assert_eq!(
            BrowserSelection::all_files(&tree(TREE_WITH_PREFIX_WIRE).flat),
            set(&["C24-8_×_25WS024/Figure/QC 图.png"])
        );

        // 空批次 → 空集（"一批里一个文件都没有"与"全选"是两件事：`all_paths` 为空时
        // "覆盖整批"那一步**不算成立**）。
        assert!(BrowserSelection::all_files(&[]).is_empty());
    }

    /// 同一份清单的**两种回执形状** —— `get_tree` 的 `flat[]` 与 `load_delivery` 的 `tree`
    /// （内核 `core/src/main.rs` 的 `op_get_tree` 与 `op_load_delivery` 各一处出口）。
    ///
    /// ⚠️ **两份是各写各的**（不拿同一个夹具喂两边）：共用一份的话，
    ///    "两条来源算不算同一个集合"这件事在断言上**不可观测**。
    ///    这一对夹具就是 [`BrowserSelection::all_files_of`] 那段文档里"文件集合逐字相同"
    ///    那句话说出口的地方。
    ///
    /// ⚠️ 夹具里的清单**故意不好看**：嵌套目录（`dir/nested.bin`）、带 `×` 与空格的目录名
    ///    （`C24-8_×_25WS024/QC 图.png`）、根下的文件（`a.bin`）各一条 ——
    ///    "只收顶层""把目录名拼进去""按 `name` 而不是 `path` 收"这几种改法都会在这里露出来。
    ///    两份夹具的**书写顺序也故意不同**（集合相等与顺序无关，但"顺手按下标取"会露出来）。
    const MANIFEST_AS_FLAT_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"a.bin","name":"a.bin","size":1,"state":"pending"},
         {"path":"dir/nested.bin","name":"nested.bin","size":2,"state":"pending"},
         {"path":"C24-8_×_25WS024/QC 图.png","name":"QC 图.png","size":3,"state":"pending"}],
 "default_selected":["a.bin"],
 "progress":{"total_bytes":6,"done_bytes":0,"speed":0,"percent":0}}"#;

    /// 同一份清单的加载回执（`load_delivery`）：树里就是上面那三个文件。
    /// ⚠️ 文件叶节点的键与 `FileNode` 逐字同形（少一个必需键就是解码失败 ——
    ///    那也正是"'内核真会发这个形状吗'这件事在这里已经验过一遍"的意思）。
    const MANIFEST_AS_TREE_WIRE: &str = r#"{"code":"C24-8","page_url":"https://d.example/C24-8/index.html",
 "base_url":"https://d.example","created_at":"","expires_at":"","expired":false,
 "total_files":3,"total_bytes":6,
 "tree":{"type":"dir","name":"","children":{
   "C24-8_×_25WS024":{"type":"dir","name":"C24-8_×_25WS024","children":{
     "QC 图.png":{"type":"file","name":"QC 图.png","path":"C24-8_×_25WS024/QC 图.png",
                  "crc64":"","size":3,"completed":0,"total":3,"speed":0,"state":"pending","err":""}}},
   "a.bin":{"type":"file","name":"a.bin","path":"a.bin",
            "crc64":"","size":1,"completed":0,"total":1,"speed":0,"state":"pending","err":""},
   "dir":{"type":"dir","name":"dir","children":{
     "nested.bin":{"type":"file","name":"nested.bin","path":"dir/nested.bin",
                   "crc64":"","size":2,"completed":0,"total":2,"speed":0,"state":"pending","err":""}}}}}}"#;

    /// 一批清单的两种回执**指向同一个集合**（任务 16 把判据接上时的那条前提）。
    ///
    /// 判别力：把 `collect_files` 改成"只收第一层"（或只收 `children` 里第一个），
    /// 这一条立刻红 —— 而真机上的表现是**某几条路径被判成"没覆盖整批"** ⇒
    /// 本该发 `[]` 的请求把整批路径塞给了内核（C-3 要防的那件事）。
    #[test]
    fn the_two_receipts_of_one_manifest_name_the_same_files() {
        let flat_receipt: TreeResult =
            serde_json::from_str(MANIFEST_AS_FLAT_WIRE).expect("这是 get_tree 的回执原文");
        let tree_receipt: crate::protocol::DeliveryInfo =
            serde_json::from_str(MANIFEST_AS_TREE_WIRE).expect("这是 load_delivery 的回执原文");

        // 期望值**逐字写死**（不从被测函数现取）：现取的话，"两边都算错、而且错得一样"
        // 也能全绿。
        let want = set(&["C24-8_×_25WS024/QC 图.png", "a.bin", "dir/nested.bin"]);
        assert_eq!(
            BrowserSelection::all_files(&flat_receipt.flat),
            want,
            "`flat[]` 那一侧"
        );
        assert_eq!(
            BrowserSelection::all_files_of(&tree_receipt.tree),
            want,
            "`load_delivery` 的树那一侧（命令层手上有的就是它）"
        );
        assert_eq!(
            BrowserSelection::all_files_of(&tree_receipt.tree),
            BrowserSelection::all_files(&flat_receipt.flat),
            "两条来源必须是**同一个集合** —— 否则 C-3 那条判据在不同入口上口径不一"
        );
    }

    /// 零文件的批次：空树（`"tree": {}`）⇒ 空集。
    #[test]
    fn an_empty_tree_has_no_files() {
        // 与 `all_files(&[])` 同一件事的另一个来源。空集是 C-3 判据里"一批是空的时
        // 不算整批"那一支的输入（`download_targets.rs` 的 `an_empty_batch_is_not_everything`）。
        assert!(BrowserSelection::all_files_of(&TreeNode::Empty).is_empty());
        // 对照：有目录、没有文件的树也是空集（目录**不**进这个集合）。
        let dirs_only = TreeNode::Dir {
            name: String::new(),
            children: BTreeMap::from([(
                "d".to_string(),
                TreeNode::Dir {
                    name: "d".to_string(),
                    children: BTreeMap::new(),
                },
            )]),
        };
        assert!(BrowserSelection::all_files_of(&dirs_only).is_empty());
    }

    /// 上游 `selectionSummaryCountsItemsAndSumsKnownSizes`。
    #[test]
    fn selection_summary_counts_items_and_sums_known_sizes() {
        let index = SelectionSummary::size_index(&tree(TREE_WIRE).flat);

        let s = SelectionSummary::of(&set(&["a.bin", "b.bin"]), &index);
        assert_eq!(s.count_text, "已选 2 项");
        assert_eq!(s.size_text, "合计 5.0 KB", "2048 + 3072 = 5120 → 5.0 KB（1024 进制）");

        // 空选择：说 0，不是空白（约束 4）。
        let empty = SelectionSummary::of(&set(&[]), &index);
        assert_eq!(empty.count_text, "已选 0 项");
        assert_eq!(empty.size_text, "合计 0 B");
    }

    /// 上游 `selectionSummaryIgnoresDirsAndUnknownPaths`。
    #[test]
    fn selection_summary_ignores_dirs_and_unknown_paths() {
        let index = SelectionSummary::size_index(&tree(TREE_WIRE).flat);

        // 目录在 `flat` 里没有大小（`flat` 只有文件）。它的**项数**照样算，
        // 大小只能按 0 计 —— 壳不自己展开目录去猜（约束 1）。
        let s = SelectionSummary::of(
            &set(&["client-test/C24-8_×_25WS024/Figure", "a.bin"]),
            &index,
        );
        assert_eq!(s.count_text, "已选 2 项");
        assert_eq!(s.size_text, "合计 2.0 KB");
    }

    // -----------------------------------------------------------------------
    // 换批复位的判据（承重事项 A）与 `.task(id:)` 的触发键（承重事项 B）
    // -----------------------------------------------------------------------

    /// 上游 `onlyAChangedCodeCountsAsANewManifest`。
    #[test]
    fn only_a_changed_code_counts_as_a_new_manifest() {
        // ⚠️ **这条只钉"码变了吗"这条取值规则本身** —— 它**不是**承重事项 A 的守卫：
        //    A 的性质是"复位发生在 code 变化那一刻、而且对照的是上一次**显示**过的码"，
        //    那两件事都发生在 `shell-win` 的调用点，调用点不单测（约束 8）。
        //    真正守 A 的是 `on_load` 那两条（下一个测试）。
        assert!(BrowserSelection::is_a_new_manifest("BBB-2", Some("AAA-1")));
        assert!(
            BrowserSelection::is_a_new_manifest("AAA-1", None),
            "第一次加载（还没有上个码）也是一批新清单"
        );
        assert!(!BrowserSelection::is_a_new_manifest("AAA-1", Some("AAA-1")));
    }

    /// 上游 `aLoadBackToThePreviousBatchIsStillANewManifest`。
    #[test]
    fn a_load_back_to_the_previous_batch_is_still_a_new_manifest() {
        // ⚠️ **承重事项 A 的守卫**：`A→B→A` 的第三步必须复位。
        //
        // 现场：① 显示 A（记下 "A"）→ ② 切到 B（复位一次，"记住的码"变成 "B"）→
        //       ③ 用户在 B 的树到之前勾了几项（勾选不依赖树）→ ④ **又回到 A**。
        // 第 ④ 步如果拿**"已经播过种的码"**（`seeded_code`）当对照，算出来是"同一批"
        // （它停在 "A"，因为复位有意不消费它、B 又还没播种）⇒ 不复位 ⇒ **B 的勾选面被拿去
        // 给 A 发 `enqueue`**；同一现场还有"静默"那一半（A 的默认选中面再也播不下来）。
        // 对照"上一次**显示**过的码"就不一样：第 ④ 步是换批。
        let first = BrowserSelection::on_load(Some("AAA-1"), None);
        assert!(first.is_a_new_manifest, "第一次加载就是一批新清单");
        assert_eq!(first.last_displayed_code.as_deref(), Some("AAA-1"));

        let second = BrowserSelection::on_load(Some("BBB-2"), first.last_displayed_code.as_deref());
        assert!(second.is_a_new_manifest);
        assert_eq!(
            second.last_displayed_code.as_deref(),
            Some("BBB-2"),
            "记住的码要推进到 B"
        );

        let back = BrowserSelection::on_load(Some("AAA-1"), second.last_displayed_code.as_deref());
        assert!(
            back.is_a_new_manifest,
            "A→B→A 的第三步是换批：不复位就会把 B 的路径发给 A"
        );
        assert_eq!(back.last_displayed_code.as_deref(), Some("AAA-1"));
    }

    /// 上游 `aNilLoadNeitherResetsNorForgetsTheRememberedCode`。
    #[test]
    fn a_nil_load_neither_resets_nor_forgets_the_remembered_code() {
        // 加载中 / 加载失败 / 内核回"没有生效的批次"时码是 None（不再"已加载"）：
        // **既不复位、也不推进**那个记住的码。两半都要：
        //   - 不复位：加载中那一下抖动不该抹掉用户在同一批里攒的选择；
        //   - 不推进（尤其**不能冲成 None**）：否则紧接着的同码加载会被误判成换批，
        //     把同一批里的选择抹掉（`the_same_manifest_does_not_wipe_the_users_selection`）。
        let mid = BrowserSelection::on_load(None, Some("AAA-1"));
        assert!(!mid.is_a_new_manifest);
        assert_eq!(
            mid.last_displayed_code.as_deref(),
            Some("AAA-1"),
            "None 不得把记住的码冲掉"
        );

        let same_again = BrowserSelection::on_load(Some("AAA-1"), mid.last_displayed_code.as_deref());
        assert!(!same_again.is_a_new_manifest, "同一批（重新加载）不复位");
        assert_eq!(same_again.last_displayed_code.as_deref(), Some("AAA-1"));

        // 从"还没有任何码"开始的第一次 None：也不该凭空造出一个码。
        let cold = BrowserSelection::on_load(None, None);
        assert!(!cold.is_a_new_manifest);
        assert_eq!(cold.last_displayed_code, None);
    }

    // -----------------------------------------------------------------------
    // `ManifestTracking`：承重事项 A 的**两半**（复位 + 播种）
    // -----------------------------------------------------------------------

    /// 上游 `theFirstLoadSeedsItsDefaultSelection`。
    #[test]
    fn the_first_load_seeds_its_default_selection() {
        // ⚠️ 这条钉的是"播种的对照值**不能**用 `last_displayed_code`"：显示那一刻它就已经
        //    等于新码了，`seed` 会判成"这一批已经播过" ⇒ **第一次加载的默认选中面永远播
        //    不下来**（开箱看到「已选 0 项」，界面上一点异常都没有）。所以"播种"必须有自己的
        //    记账（`seeded_code`，只有播成功才推进）。
        let mut t = ManifestTracking::new();

        let is_new = t.display(Some("AAA-1"));
        assert!(is_new, "第一次加载就是换批");
        let seeded = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(seeded.map(|s| s.selection), Some(set(&["a.bin"])));
    }

    /// 上游 `theSameLoadIsNotSeededTwice`。
    #[test]
    fn the_same_load_is_not_seeded_twice() {
        // **同一次加载内**（`generation` 不变）目录来回切 / 切分区回来 / 树到位后的再进来：
        // **不能再播一次**（那会把用户攒的选择抹掉）。
        //
        // ⚠️ 判据是「哪一次**加载**」而不是「哪一批」：同码重载时 `code` 一个字都没变，
        //    内核却已经重新规划过整批 —— 按 `code` 记账就等于"同码重载永远不重播默认面"。
        let mut t = ManifestTracking::new();
        let _ = t.display(Some("AAA-1"));
        let first = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(first.map(|s| s.selection), Some(set(&["a.bin"])));

        let again = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(again, None, "同一次加载只播一次");

        let same_batch = t.display(Some("AAA-1"));
        assert!(!same_batch, "同一批不算换批");

        let after_redisplay = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(after_redisplay, None);
    }

    /// 上游 `aSameCodeReloadReseedsTheDefaultFace`。
    #[test]
    fn a_same_code_reload_reseeds_the_default_face() {
        // ⚠️⚠️ **同码重载是一次新加载**，内核刚刚重新规划过整批 ⇒ `default_selected` 是新信息，
        //    必须重播。改之前它静默不播：同码时产生不了任何复位，而 `seed` 只按 `code` 记账
        //    ⇒ 判成"这一批已经播过种" ⇒ 返回 None。后果是用户读成"我重载了，界面一点变化
        //    都没有"。
        //
        // ⚠️ 这里用**两棵不同的树**当"重载前后"，否则"重播了"与"没重播"在断言上不可观测。
        let mut t = ManifestTracking::new();
        let _ = t.display(Some("C24-8"));
        let before = t.seed("C24-8", Some("C24-8"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(before.map(|s| s.selection), Some(set(&["a.bin"])));

        // 同码重载（同一批、**新的一代**）⇒ 播成内核刚算出来的那一份。
        let after_reload = t.seed("C24-8", Some("C24-8"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(
            after_reload.map(|s| s.selection),
            Some(set(&["b.bin"])),
            "同码重载（新的一代）必须重播默认面 —— 不播就是「我重载了、界面一点没变」"
        );

        // 同一代里再进来：还是不播（否则每次重绘都会抹掉用户攒的选择）。
        let same_load_again = t.seed("C24-8", Some("C24-8"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(same_load_again, None);
    }

    /// 上游 `onlyANewBatchAsksTheBrowserToGoBackToTheRoot`。
    #[test]
    fn only_a_new_batch_asks_the_browser_to_go_back_to_the_root() {
        // ⚠️ **复审重要 2**："播了默认面就回到根目录"这件事，对**换批**是对的（那是另一棵树），
        //    对**同码重载**——包括**内核崩溃后的自动恢复**——是一次**非用户动作**引发的
        //    用户可见状态改写：用户停在子目录、攒了半天的勾选会被静默替换掉，而界面上
        //    没有一句话说明。所以两件事必须**解耦**：
        //      - **重播勾选面**：照旧（内核刚重新规划过整批，`default_selected` 是**新信息**）；
        //      - **复位浏览位置**：只在**交付码真的变了**时做。
        //
        //    ⚠️ 判据必须是"**上一次播种的是不是这一批**"（`seeded_code != code`），
        //       不能是"这一代有没有播过种" —— 后者对同码重载同样是 true。
        let mut t = ManifestTracking::new();

        // ① 第一次见到这一批 ⇒ 换批（这时浏览位置本来就是根，复位是空操作）。
        let _ = t.display(Some("AAA-1"));
        let first = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(
            first.map(|s| s.is_a_new_batch),
            Some(true),
            "第一次见到这一批 ⇒ 换批"
        );

        // ② **同码重载**（新的一代）：要**重播**默认面，但**不算换批** ⇒ 不许打回根目录。
        let same_code = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(
            same_code.as_ref().map(|s| s.selection.clone()),
            Some(set(&["b.bin"])),
            "同码重载要重播（这一条不能回退）"
        );
        assert_eq!(
            same_code.map(|s| s.is_a_new_batch),
            Some(false),
            "同码重载不是换批 —— 不许把浏览位置打回根目录"
        );

        // ③ 换一个码 ⇒ 换批 ⇒ 复位浏览位置。
        let _ = t.display(Some("BBB-2"));
        let other = t.seed("BBB-2", Some("BBB-2"), Some(&tree(TREE_B_WIRE)), 3);
        assert_eq!(
            other.map(|s| s.is_a_new_batch),
            Some(true),
            "换批要复位浏览位置（那是另一棵树）"
        );

        // ④ `A → B(树没到) → A`：回到更早播过种的那一批，也是**换批**。
        //    （这条路上 `seeded_code` 停在 "B"，所以判据与 ③ 不同源，值得单独钉。）
        let _ = t.display(Some("BBB-2"));
        let _ = t.seed("BBB-2", Some("BBB-2"), Some(&tree(TREE_B_WIRE)), 4);
        let _ = t.display(Some("AAA-1"));
        let back_to_a = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 5);
        assert_eq!(
            back_to_a.map(|s| s.is_a_new_batch),
            Some(true),
            "回到另一批也是换批 ⇒ 复位浏览位置"
        );
    }

    /// 上游 `aNewBatchIsSeededOnlyOnceItsOwnTreeArrives`。
    #[test]
    fn a_new_batch_is_seeded_only_once_its_own_tree_arrives() {
        // 树没到就**推迟**（不记账），树到位才播、且**不拿上一批的树播**。
        let mut t = ManifestTracking::new();
        let _ = t.display(Some("AAA-1"));
        let _ = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);

        let switched = t.display(Some("BBB-2"));
        assert!(switched, "换批");

        // ⚠️ 新一代（换批本来就是一次新加载）—— 这一代里 B 一次都没播过。
        let no_tree = t.seed("BBB-2", None, None, 2);
        assert_eq!(no_tree, None, "树还没到：推迟");

        let stale_tree = t.seed("BBB-2", Some("AAA-1"), Some(&tree(TREE_WIRE)), 2);
        assert_eq!(
            stale_tree, None,
            "还是上一批的树：更不能播（那些路径在这一批里可能指向别的文件）"
        );

        let own_tree = t.seed("BBB-2", Some("BBB-2"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(
            own_tree.map(|s| s.selection),
            Some(set(&["b.bin"])),
            "这一批的树到位：播"
        );
    }

    /// 上游 `aLateDisplayInvalidationStillLetsTheNextEntryReseed`。
    #[test]
    fn a_late_display_invalidation_still_lets_the_next_entry_reseed() {
        // ⚠️ 这条守的是那个**已知的潜在竞态**留下的自愈路：换批复位（走 `display`）与播种
        //    （走 `seed`）的**次序没有保证**。万一 `display` 落在 `seed` **后面**，这一批的
        //    播种记账就被清了、而调用方手上那份默认面还没落稳（复位那一半会把勾选面清空）。
        //    所以换批必须**两个记账一起**作废：只清 `seeded_code` 不清 `seeded_generation`，
        //    下一次 `task_key` 变化就会返回 None ⇒ **补播永远不会发生** ⇒ 用户停在
        //    「已选 0 项」而界面上没有任何提示（那正是承重事项 A 的另一半）。
        let mut t = ManifestTracking::new();
        let _ = t.display(Some("AAA-1"));

        // 竞态里先跑的那一拍：树到了、先播了一次。
        let raced = t.seed("BBB-2", Some("BBB-2"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(raced.map(|s| s.selection), Some(set(&["b.bin"])));

        // 后跑的那一拍：换批复位（同一个码、同一代）。
        let late_reset = t.display(Some("BBB-2"));
        assert!(late_reset, "换批");

        // 用户随后动一下（换目录 / 树再到位）⇒ 必须还能补播。
        let healed = t.seed("BBB-2", Some("BBB-2"), Some(&tree(TREE_B_WIRE)), 2);
        assert_eq!(
            healed.map(|s| s.selection),
            Some(set(&["b.bin"])),
            "换批把记账清掉之后，下一次进来必须还能补播（否则「已选 0 项」再也回不来）"
        );
    }

    /// 上游 `goingBackToThePreviousBatchSeedsItsDefaultSelectionAgain`。
    #[test]
    fn going_back_to_the_previous_batch_seeds_its_default_selection_again() {
        // ⚠️⚠️ **承重事项 A 的"静默那一半"**：`A → B（树未到）→ A`。
        //
        // 现场：A 播过种（`seeded_code = "A"`）→ 切到 B（**B 的树还没到**，所以 B 一个都没播）
        //       → 用户在 B 的树到之前又回到 A。
        // 坏掉的样子：`seeded_code` 还停在 "A" ⇒ `on_new_manifest` 判成"同一批" ⇒
        //       **A 的默认选中面永远播不下来** ⇒ 用户回到 A 看到「已选 0 项」（而复位那一半
        //       刚把他的选择清空过）—— **界面上一点异常都没有**。
        // 修法：`display` 在**换批那一刻**把 `seeded_code` 一起清掉。
        let mut t = ManifestTracking::new();

        let _ = t.display(Some("AAA-1"));
        let first_seeding = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(first_seeding.map(|s| s.selection), Some(set(&["a.bin"])));

        let to_b = t.display(Some("BBB-2"));
        assert!(to_b, "换批");

        // B 的树一直没到（这里根本不调 seed）—— 用户直接回到 A。
        let back_to_a = t.display(Some("AAA-1"));
        assert!(back_to_a, "回到 A 也是换批（上一次显示的是 B）");

        let reseeded = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 3);
        assert_eq!(
            reseeded.map(|s| s.selection),
            Some(set(&["a.bin"])),
            "A 的默认选中面必须**再播一次** —— 不播就是「已选 0 项」而界面上没有任何提示"
        );
    }

    /// 上游 `aSameBatchLoadingBlipDoesNotReseedByItself`。
    #[test]
    fn a_same_batch_loading_blip_does_not_reseed_by_itself() {
        // 同一批那一下加载抖动（短暂离开"已加载" ⇒ 码是 None）**本身**不得把用户攒的选择
        // 播成默认面：`display(None)` 既不复位、也不推进记账。
        //
        // ⚠️ 与 `a_same_code_reload_reseeds_the_default_face` 的分工（别把这两条读成矛盾）：
        //    真正让"同码重载重播"的是**一次成功的加载**（`generation` 推进），
        //    不是界面上那次 None 抖动。所以这里的 `generation` **故意不变**。
        let mut t = ManifestTracking::new();
        let _ = t.display(Some("AAA-1"));
        let _ = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);

        let during_loading = t.display(None);
        assert!(!during_loading, "加载中：不复位");

        let same_batch = t.display(Some("AAA-1"));
        assert!(!same_batch, "同一批：不复位");

        let reseeded = t.seed("AAA-1", Some("AAA-1"), Some(&tree(TREE_WIRE)), 1);
        assert_eq!(
            reseeded, None,
            "没有新的一次加载就不重播：用户在这批里攒的选择不能被抹掉"
        );
    }

    /// 上游 `theTaskKeyChangesWhenThisBatchesTreeArrives`。
    #[test]
    fn the_task_key_changes_when_this_batches_tree_arrives() {
        // ⚠️ 承重事项 B：内核侧先落"已加载"、**之后**才拉 `get_tree`，而那一层是在"已加载"
        //    那一刻出现的 —— 所以第一次进来时 `tree_code == None`，`on_new_manifest` 会
        //    **推迟**播种。**只有 key 在这之后变了**，才会再进一次、才播得下去。
        //    删掉 key 里那一段 ⇒ "推迟播种"变成"永不播种"（默认选中面永远播不下来，
        //    而界面上一点异常都看不出来）。
        let waiting = BrowserSelection::task_key("AAA-1", "", None, 1);
        let arrived = BrowserSelection::task_key("AAA-1", "", Some("AAA-1"), 1);

        assert_ne!(
            waiting, arrived,
            "这一批的树到位必须让 key 变 —— 重播只有这一个触发点"
        );
    }

    /// 上游 `aSameCodeReloadChangesTheTaskKey`。
    #[test]
    fn a_same_code_reload_changes_the_task_key() {
        // ⚠️ **同码重载必须让那一层重进一次** —— 否则"重播默认面"根本没有入口。
        //    同码重载时 `code` / `path` / `tree_code` 三个分量**一个字都没变**，
        //    所以新一代必须自己进 key。
        let base = BrowserSelection::task_key("AAA-1", "", Some("AAA-1"), 7);

        assert_ne!(
            BrowserSelection::task_key("AAA-1", "", Some("AAA-1"), 8),
            base,
            "同码重载（新的一代）要重进"
        );
        assert_eq!(
            BrowserSelection::task_key("AAA-1", "", Some("AAA-1"), 7),
            base,
            "同一代内不变（不能每次重绘都重进）"
        );
    }

    /// 上游 `everySegmentOfTheTaskKeyIsLoadBearing`。
    #[test]
    fn every_segment_of_the_task_key_is_load_bearing() {
        let (code, path, tr) = ("AAA-1", "a/b", "BBB-2");
        let base = BrowserSelection::task_key(code, path, Some(tr), 5);

        assert_eq!(
            BrowserSelection::task_key(code, path, Some(tr), 5),
            base,
            "同输入同 key（不能每次重绘都重进）"
        );
        assert_ne!(
            BrowserSelection::task_key("CCC-3", path, Some(tr), 5),
            base,
            "换批要重进"
        );
        assert_ne!(
            BrowserSelection::task_key(code, "a/c", Some(tr), 5),
            base,
            "换层要重进"
        );
        assert_ne!(
            BrowserSelection::task_key(code, path, None, 5),
            base,
            "树到位要重进"
        );
        assert_ne!(
            BrowserSelection::task_key(code, path, Some(tr), 6),
            base,
            "新的一次加载要重进"
        );
    }

    /// 上游 `theTaskKeyIsUnambiguousAcrossSegmentBoundaries`。
    #[test]
    fn the_task_key_is_unambiguous_across_segment_boundaries() {
        // ⚠️ 几段**不能裸拼**（`"{code}|{path}|{tree_code}"` 那种）：路径是清单原文，
        //    里面可以有 `|`（约束 3：不规范化、不转义），裸拼会让下面两对输入撞成同一个键
        //    —— 撞了就是"该重进的时候没重进"，也就是这条防线要防的那种静默失效。
        assert_ne!(
            BrowserSelection::task_key("A|B", "", None, 1),
            BrowserSelection::task_key("A", "B|", None, 1)
        );
        assert_ne!(
            BrowserSelection::task_key("A", "", Some("B|"), 1),
            BrowserSelection::task_key("A", "B|", Some(""), 1)
        );
        // 代数那一段是**纯整数、且排在最后**：它前面那段（`tree_code`）是长度前缀的、
        // 自定界的，所以追加一个 `|<数字>` 不会与树码里的 `|` 混淆（裸拼的话下面这两条
        // 就会撞）。
        assert_ne!(
            BrowserSelection::task_key("A", "", Some("B|1"), 0),
            BrowserSelection::task_key("A", "", Some("B"), 1)
        );
    }

    // -----------------------------------------------------------------------
    // 时间列：「源文件的修改时间」
    // -----------------------------------------------------------------------

    /// 上游 `anOldPayloadWithoutTheKeyStillDecodesAndReadsNil`。
    #[test]
    fn an_old_payload_without_the_key_still_decodes_and_reads_nil() {
        // 约束 C-2：已交付的老清单里**没有** `source_mtime` 这个键。
        // ⚠️ 这条断言的前半句是"它解得动"：非可选字段会让**整条载荷解码失败**，
        //    而那时是抛在上面那句 `expect` 里、后面的断言一行都到不了 —— 所以它在，
        //    且必须先于一切。
        let old = file_nodes(LIST_DIR_WIRE);

        assert_eq!(old.len(), 2, "夹具本身：这一层两个文件");
        assert!(
            old.iter().all(|f| f.source_mtime.is_none()),
            "缺键 → None（不是空串）"
        );
    }

    /// 上游 `anOldTreePayloadWithoutTheKeyStillLoads`。
    #[test]
    fn an_old_tree_payload_without_the_key_still_loads() {
        // 约束 C-2 的**另一条入口**：整棵树里同样是 `FileNode`。这里坏掉的后果比文件页那一列
        // 重得多 —— 不是"时间列空着"，而是**整批加载失败**（用户看到的是错误正文），
        // 所以老清单在这条路上也必须解得动。
        // 前半句同样是"它解得动"：非可选字段会让 `tree(…)` 直接抛在上面那句 `expect` 里。
        //
        // ⚠️ 本仓库的 e2e 覆盖不到这一条：它的清单走 `delivery_manifest.build_manifest`，
        //    而那个函数**始终**写这个键（3 元组条目补空串）⇒ 线上是"键在、值为空串"。
        //    「键真的不在」只有这段载荷能给。
        let t = tree(OLD_TREE_WIRE);

        assert_eq!(
            t.flat.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
            vec!["t/a.txt"],
            "夹具本身"
        );
        assert_eq!(t.default_selected, vec!["t/a.txt"], "夹具本身");

        let leaf = match &t.tree {
            TreeNode::Dir { children, .. } => children.get("t").and_then(|n| match n {
                TreeNode::Dir { children, .. } => children.get("a.txt"),
                _ => None,
            }),
            _ => None,
        };
        let Some(TreeNode::File(leaf)) = leaf else {
            panic!("夹具不是「根 → t → a.txt」那棵树");
        };
        assert_eq!(leaf.source_mtime, None, "缺键 → None（不是空串）");
    }

    /// 上游 `aPayloadWithTheKeyCarriesItVerbatim`。
    #[test]
    fn a_payload_with_the_key_carries_it_verbatim() {
        // 内核当**不透明字符串**搬运，壳也不加工（约束 3）：不解析、不换时区、不校验形状。
        // 三种输入逐字带过来 —— 怎么显示是 [`SourceTimeText`] 那一层的事。
        assert_eq!(
            source_mtime(LIST_DIR_WIRE_WITH_TIMES, "has-time.txt").as_deref(),
            Some("2026-09-14T12:00:00+08:00")
        );
        assert_eq!(
            source_mtime(LIST_DIR_WIRE_WITH_TIMES, "empty-time.txt").as_deref(),
            Some(""),
            "空串与缺键是两种输入"
        );
        assert_eq!(
            source_mtime(LIST_DIR_WIRE_WITH_TIMES, "junk-time.txt").as_deref(),
            Some("待定")
        );
    }

    /// 上游 `theTimeColumnShowsADashWhenThereIsNoValue`。
    #[test]
    fn the_time_column_shows_a_dash_when_there_is_no_value() {
        // ⚠️ 缺值与空串都出 `—`（那是"这一格没有值"，不是"时间是空的"）。
        //    变异体：`raw.unwrap_or("")`（老清单那一半）或直接透传空串 ⇒ 界面上什么都没有，
        //    而约束 C-2 明文要求显示 `—`、**不得**显示空白。
        assert_eq!(SourceTimeText::of(None), "—");
        assert_eq!(SourceTimeText::of(Some("")), "—");
    }

    /// 上游 `theTimeColumnReusesTheSharedTimestampPresentation`。
    #[test]
    fn the_time_column_reuses_the_shared_timestamp_presentation() {
        // 与侧边栏的 `created_at` / `expires_at` **同一个**格式化器（同形的输入、同一口径）：
        // 壳不另造一套（约束 C-7）。
        assert_eq!(
            SourceTimeText::of(Some("2026-09-14T12:00:00+08:00")),
            "2026-09-14 12:00"
        );
        assert_eq!(
            SourceTimeText::of(Some("2026-09-14T12:00:00+08:00")),
            TimestampPresentation::text("2026-09-14T12:00:00+08:00"),
            "两条必须是同一个实现（这条等值比较钉住「没另造一套」）"
        );
    }

    /// 上游 `theTimeColumnNeverInventsWordingOfItsOwn`。
    #[test]
    fn the_time_column_never_invents_wording_of_its_own() {
        // 约束 C-7：内核给的原文由壳**原样呈现**。认不出来的形状就照抄原文 ——
        // **不得**出现 `Invalid Date` 这种壳自己编的文案（`TimestampPresentation` 的既有
        // 契约就是"任何一条不满足就原样返回"，这里沿用它、不另造一套）。
        // 下面五个都是**形状**不合法（不是日历值离谱）：壳只按形状判，不校验日历。
        for raw in [
            "待定",
            "2026-09-14",
            "2026-09-14T12:00",
            "2026-10-14T16:13:34+0800",
            "2026-10-14T16:13:34+08:00 尾巴",
        ] {
            assert_eq!(SourceTimeText::of(Some(raw)), raw, "{raw} 必须原样返回");
        }
    }

    /// 上游 `fileRowsCarryTheTimeTextAndDirRowsNeverDo`。
    #[test]
    fn file_rows_carry_the_time_text_and_dir_rows_never_do() {
        let text = |name: &str| row_in(LIST_DIR_WIRE_WITH_TIMES, "t", name).source_time_text;

        assert_eq!(text("has-time.txt"), "2026-09-14 12:00");
        assert_eq!(text("empty-time.txt"), "—");
        assert_eq!(
            text("junk-time.txt"),
            "待定",
            "认不出来就原样显示，不是 `—`、更不是 `Invalid Date`"
        );
        // 目录**没有**"源文件时间"这回事（与它的大小同理，`BrowserRow::size` 是 None）：
        // 恒 `—`，不因为它有几个子项而变。
        assert_eq!(text("d"), "—");
    }

    /// 上游 `oldPayloadRowsShowADashNotABlank`。
    #[test]
    fn old_payload_rows_show_a_dash_not_a_blank() {
        // 约束 C-2 的呈现那一半：老清单的行**必须显示 `—`，不得显示空白**。
        // 变异体：`f.source_mtime.unwrap_or_default()` ⇒ 这一格什么都没有，用户看到的是
        // "这里坏了"。
        assert_eq!(row("QC 图.png").source_time_text, "—");
        assert_eq!(row("reads.fq.gz").source_time_text, "—");
    }
}
