//! `/api/tree` 那两格的载荷（搬自 `shell-win/src/server/routes.rs:261` 的 `tree` 与
//! `:306` 的 `tree_json`）。
//!
//! ## ⚠️ 两条分支发的是**两件不同的东西**，这不是笔误
//!
//!   * 有 `path` ⇒ 前端要的是"这一层有哪些行"（`list_dir` ⇒ `BrowserRow`）——
//!     成 [`level`]；读那一层失败则成 [`level_failure`]；
//!   * 没有 `path` ⇒ 前端要的是"这一批多大、勾选面该是什么"（`get_tree` ⇒
//!     总进度 + 勾选摘要）—— 成 [`whole`]。
//!
//! ⚠️ [`whole`] **不发 `flat`**（裁定逐字）：前端不需要那份扁平表，
//!    而底栏那一行由服务端算（`SelectionSummary::of`）。
//!
//! ## 🔴 [`level`] 的每一行比 `BrowserRow` 多两格（`state_label` / `state_color`）
//!
//! 它们是**状态列那一格的显示值**，而它们**不在 `BrowserRow` 的派生形状里**
//! —— `RowStateStyle::label()` / `color()` 是**方法**，派生出来的 `state` 只是变体名。
//! 少了这两格的后果是前端**状态列整列空白**（详见 [`row_wire`] 的文档）。
//! 这是 R-35 那条形状（"Rust 的方法不在载荷里，前端就得自己拼"）在文件页上的落点。
//!
//! ## ⚠️ [`DownloadAction`] 是任务 10 补的第三条缺口（底栏那颗按钮与它那句话）
//!
//! 底栏按 macOS 的 `SelectionBar.swift` 要三样东西：`summary`（**已有**）、
//! `DownloadTargets::buttonTitle(for:)` 与 `DownloadTargets.emptySelectionHint`。
//! 后两样**早就有实现与单测**（`presentation/download_targets.rs`），
//! 但从来**没有任何命令把它们送出去** —— 于是前端要么自己拼一句（§3.2 明禁），
//! 要么让那一格空着（约束 4 明禁的静默失效）。这是 `api` 层的缺口，补在这里。
//!
//! ⚠️ **它是一组、不是一个字段**，所以打包成一个对象而不是往 `data` 上加两个平铺键：
//!    ① 两个值来自**同一组纯函数、同一个勾选面**，拆成两个顶层键之后
//!       "它们是同一件事"这条信息就只剩注释里有了；
//!    ② 两个都是 `&str` ⇒ 平铺成六个位置参数时，调用点把
//!       `button_title` 与 `empty_selection_hint` 写反**照样编译得过**
//!       （而那种错误在界面上是"按钮写着提示句、旁边写着按钮名"，不会有东西变红）；
//!    ③ 日后 `DownloadTargets::help_text()` 要接上时是**往这个对象里加一格**，
//!       而不是再往顶层加一个键（那里已经有五格了）。
//!
//! ⚠️ **③ 那条路在任务 17 第一次真的走了一遍**：加进来的是
//!    [`DownloadAction::blocked_reason`]（C-3 的另一半："按不下去时说什么"）。
//!    `help_text()` 仍然没有接上（它今天没有消费者），本文件**没有**顺手把它塞进来。
//!
//! ⚠️ **本层不重算**：这两个字符串由命令层调 `DownloadTargets` 算好交进来
//!    （同 `SelectionSummary::of` 与 `ProgressSummary::of` 的既有分工 ——
//!    "判断在 `shell-core` 里，命令层只做取数据"）。
//!
//! ## ⚠️ 第二代那个 `tree()` 里**没有搬过来**的东西：对内核的调用
//!
//! 上游那个函数一进门就问 `session.client()`、调 `kernel::list_dir` / `kernel::get_tree`、
//! 失败时把原文落到 `session` 上。那些全是**命令层**的事（它才拿得到 `Session` 与内核连接）。
//! 本层只回答"给定这些界面值，前端该收到什么"。
//!
//! ⚠️ 于是"没有内核 ⇒ 503 + [`NO_KERNEL`] 那句话"也留在命令层 —— 那句原文（壳自己写的，
//!    没有内核原文可登）现在仍在 `shell-win` 里，等任务 7 把它交给命令层。
//!    它**不是**本层的输入：本层的三个函数都假定"手上已经有数据了"。

use serde::Serialize;
use serde_json::Value;

use crate::api::{envelope, to_wire};
use crate::presentation::breadcrumb::{Breadcrumb, DirLoadFailure};
use crate::presentation::browser_row::{BrowserRow, SelectionSummary};
use crate::presentation::verify_summary::ProgressSummary;

/// 读到了某一层：面包屑 + 那一层的行。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:261` 的 `tree`（有 `path` 那一支）。
/// **对齐 macOS**：`Presentation/Breadcrumb.swift`（`Breadcrumb`，**已移植**到
/// `presentation/breadcrumb.rs`）与 `Presentation/BrowserRow.swift`（`BrowserRow.rows`，
/// **已移植**到 `presentation/browser_row.rs`）—— 本函数只把那两份界面值装进信封。
///
/// ⚠️ **面包屑由壳拼**（`list_dir` 的目录项**没有 `path` 键**，路径只能壳自己拼 ——
///    `BrowserRow::of` 的文档里写着这条）。它是**入参**，本函数不重算：
///    "界面上的层级链与列表必须是同一层"这条判据（见 [`level_failure`]）靠调用方
///    把**同一个** `Breadcrumb` 交给两边。
pub fn level(bc: &Breadcrumb, rows: &[BrowserRow]) -> Value {
    envelope::ok(serde_json::json!({
        "breadcrumb": to_wire(bc),
        // ⚠️ 逐行过一遍 [`row_wire`]（不是 `to_wire(rows)`）：那一行要多带两格，
        //    而少了它们的后果是**状态列整列空白**（见那个函数的文档）。
        "rows": rows.iter().map(row_wire).collect::<Vec<Value>>(),
    }))
}

/// 一行 [`BrowserRow`] → 线上那一格（`to_wire(row)` + 两格状态列的显示值）。
///
/// ## 🔴 这两格为什么必须在这里补上（这是一个**真实交付出去的缺陷**的修复）
///
/// `RowStateStyle` 的 `label()` 与 `color()` 是**方法**，`Serialize` 派生出来的
/// `state` 只是那个**变体名**（`"Complete"` / `"Pending"` / `"Downloading"` /
/// `"Failed"` / `"None"`）。也就是说：**这两句显示值根本不在载荷里**。
///
/// 于是前端只有两条路，两条都是错的：
///   · 自己按变体名拼一句/挑一个颜色 ⇒ 把 `presentation/browser_row.rs` 里那张
///     （有四态两两不同的单测钉着的）映射抄进 JS，从此与 macOS 各走各的（§3.2 明禁）；
///   · 什么都不显示 ⇒ **状态列整列空白**，而它**不会有任何东西变红**
///     （空着不等于报错），客户看到的是一列没有内容的格子。
/// ⇒ 补在**这一层**（载荷拼装层）是唯一不动 `presentation` 类型、也不让前端造字的落点。
///
/// ⚠️ **两格都调 `presentation` 的方法取来**（`RowStateStyle::label()` / `::color()`），
///    本文件**一个字都不重写**：重写一份的下场是它与 `browser_row.rs` 那几条单测各走各的。
///
/// ⚠️ **`state`（变体名）在这里被删掉**（R-35 的另一半：**别发没人读的字段**）。
///    它是 `Serialize` 派生出来的 `"Complete"` / `"Pending"` / …，而前端要看的是
///    上面那两格 —— 留着它就是一个**只有形状、没有消费者**的格子。
///    ⚠️ **这条改判过一次，账记在这里**：本函数的第一版留着它，理由是
///    "删它就得手写投影（`api/mod.rs` 明禁的那件事）"—— 而那个理由是**不成立的**：
///    本函数**本身**就是那处"手写投影"（于是它才被登记成 `api/mod.rs` 的第四处例外）。
///    既然已经在这里做投影了，"多删一格"与"多补两格"是同一个动作的两半，
///    没有理由只做一半。**结论：删。**
///
/// ⚠️ **线上的形状是契约，改它要连测试一起改**：钉住它的是
///    `every_row_carries_its_state_label_and_color`（逐字期望值 + `state` 必须不在）、
///    `a_loaded_level_carries_the_breadcrumb_and_the_rows`（九格恒在）与
///    `the_wire_forms_of_kind_and_color_are_pinned`（`kind` 与 `state_color`
///    这两个**枚举名**的线上形态 —— 前端就是照那两个字符串比的）。
fn row_wire(row: &BrowserRow) -> Value {
    let mut wire = match to_wire(row) {
        Value::Object(map) => map,
        // 到不了：`BrowserRow` 是个结构体，`to_wire` 出来一定是对象。
        // 但真到了也要有个明确结论（回它自己），而不是把整条载荷吞掉。
        other => return other,
    };
    // 先**删**掉那个没有消费者的派生格（它的值是 Rust 的变体名，前端一个都不该认）。
    wire.remove("state");
    // 再补上状态列真正显示的那两句（值来自 `presentation` 的方法，见上面那段）。
    wire.insert(
        "state_label".to_string(),
        Value::String(row.state.label().to_string()),
    );
    wire.insert("state_color".to_string(), to_wire(&row.state.color()));
    Value::Object(wire)
}

/// 读某一层**失败**：面包屑 + 那一份失败说明。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:261` 的 `tree`（`Err(why)` 那一支）。
/// **对齐 macOS**：`Presentation/Breadcrumb.swift:85` 的 `DirLoadFailure`
/// （**已移植**到 `presentation/breadcrumb.rs`，字段与 `of(_:currentPath:)` 的判据逐字）。
///
/// ⚠️ **它是 `ok` 信封，不是 `err`**（上游 `routes.rs:285` 逐字：`status: 200` +
///    `envelope::ok(…)`）。这不是笔误：`list_dir` 读不到某一层是**界面上的一个状态**
///    （`DirLoadFailure` 是界面值：内核原文 + 该停在哪一层 + 有没有退过），
///    前端要把它摆在列表原来那一块；而 `err` 是"这一次动作整个没成"。
///    ⇒ 把它写成 `err` 会让前端进错分支（**整屏**报错，而不是那一块）。
///
/// ⚠️ 面包屑跟着**退到的那一层**走（`DirLoadFailure::path`）：界面上的层级链与列表
///    必须是同一层，否则用户看到的是"面包屑说我在这儿、列表却是上一层的"。
///    所以调用方要传的是 `Breadcrumb::new(&failure.path)`，不是失败前那一层。
pub fn level_failure(bc: &Breadcrumb, f: &DirLoadFailure) -> Value {
    envelope::ok(serde_json::json!({
        "breadcrumb": to_wire(bc),
        "failure": to_wire(f),
    }))
}

/// 底栏那个「下载」动作的呈现值（`DownloadTargets` 那一组纯函数在**这一个勾选面**上的产物）。
///
/// ⚠️ **它为什么在 `api` 层**：它是**回执里的一个格子**（`data.action`），与
///    [`SelectionSummary`] 同一个性质 —— 只是那两个值由 `presentation` 的既有纯函数算，
///    本层只负责装。所以它没有自己的算法，只有形状。
///
/// ⚠️ **字段全是 `pub`、由结构体字面量构造**：调用方（命令层）拿到的是
///    `DownloadTargets::{button_title, empty_selection_hint}` 的返回值，
///    中间不经过任何本层的构造函数 —— 那样的构造函数只会成为第二处需要跟着改的地方
///    （同 `breadcrumb.rs` 的 `DirLoadFailure` 那条记账）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct DownloadAction {
    /// 按钮上的文案（`DownloadTargets::button_title`）：空勾 ⇒ 全部下载、有勾 ⇒ 下载选中。
    ///
    /// ⚠️ 同一个算式的结果**还要喂给工具栏那颗按钮**（`RootView.swift` 的注释逐字钉着
    ///    "两处必须完全一致"）—— 所以它是回执里的数据，不是某一屏自己的文案。
    pub button_title: String,
    /// 「一项都没勾，按下去会下全部」这句**明说**；有勾选时是 `None`。
    ///
    /// ⚠️ **`None` 不是空串**（同 `DeliverySummary::expired_badge_text` 的口径）：
    ///    前者是"这一格此刻不该出现"，后者会渲染成一个空的占位。
    ///    判据 `selection.isEmpty ? … : nil` 在 macOS 侧住视图里
    ///    （`SelectionBar.swift:33` 的 `hint`），本代落在命令层那一步（取数据的那一侧）。
    pub empty_selection_hint: Option<String>,
    /// 「这个动作此刻**按不下去**」的理由（`DownloadTargets::blocked_reason`）；可以按是 `None`。
    ///
    /// 🔴 **这一格是任务 17 补的**（本代 C-3 的另一半），也正是本类型第 ③ 条记账
    ///    （"日后要接上时是**往这个对象里加一格**"）第一次真的发生。
    ///
    /// ⚠️ **判据不在本层**：什么时候算"按不下去"由 `DownloadTargets::blocked_reason` 说了算
    ///    （它的入参是**真正要发出去的那一串**，所以"整批全选 ⇒ 发空数组"那条出路永远不被挡）。
    ///    本层只把那个值装进这一个格子，**不重算、也不加工**。
    ///
    /// ⚠️ **`None` 序列化成 `null`**（与 [`Self::empty_selection_hint`] 同一条口径：
    ///    `None` = 此刻没有这句话，**不是空串**）。前端的读法是"这一格是字符串 ⇒
    ///    那个动作此刻不可用，并把这句话摆出来"。
    pub blocked_reason: Option<String>,
}

/// 整棵树：面包屑（恒是根）+ 总进度 + 勾选摘要 + 内核给的选择面 + 底栏那个动作。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:306` 的 `tree_json`。
/// **对齐 macOS**：`Presentation/Breadcrumb.swift`（恒为根的那个面包屑）、
/// `Presentation/VerifySummary.swift` 的 `ProgressSummary`、
/// `Presentation/BrowserRow.swift` 的 `SelectionSummary`（三份都已移植）。
/// ⚠️ **`selected` 那一格没有 macOS 对应物**：SwiftUI 那一代把"哪些文件还没下完"
///    直接喂给视图的 `@State` 勾选面，没有一条串行化的边界；web 这一代必须有
///    （见 `api/mod.rs` 文件头第 3 条例外）。
///
/// ## ⚠️ `progress` 是 `Option`，**不是**简报里那个 `&ProgressSummary`
///
/// 计划（`docs/superpowers/plans/2026-09-20-windows-tauri-client.md:306`）写的签名是
/// `progress: &ProgressSummary`。那样**表达不了 `null`**，而上游真的会发 `null`：
/// `ProgressSummary::of` 返回 `Option`，`LoadState` 不是 `Loaded` 时它是 `None`
/// ——"整棵树拿到了、但还没有生效批次"是可达的（`code == None` 就是那道闸的本意：
/// **不把上一批的进度摆在这一批底下**）。写死成 `&ProgressSummary` 只有两条路：
/// 要么编一个"0 / 0 / 0"（替内核说一句它没说过的话，与 `transfers` 那条
/// 不许编 `global` 的判据同源），要么干脆不让调用方表达这个状态（静默失效）。
/// ⇒ 加一个 `Option` 是**唯一**能保住那个形状的写法。**这是本任务对计划签名的
/// 一处有意偏离，已记在任务 2/3 报告里**（W-6：偏离一律写在它出现的地方）。
///
/// ⚠️ 调用方算 `ProgressSummary::of(...)` 时那三道闸（`tree` 有没有、`tree_code == code`、
///    `code` 有没有）在**上游**那个类型里，别在这里重写一份 —— 那会让同一条判据
///    有两个实现，而它们漂移时不会有东西变红。
///
/// ⚠️ `selected` 是**原样透传**的（`api/mod.rs` 文件头第 3 条例外）：它是**内核给的选择面**
///    （一串路径原文），壳没有算它、也不该算它（"哪些文件还没下完"是内核的知识，约束 1）。
///    壳算的那份界面值是 `sel`（`SelectionSummary::of`）。
///
/// ⚠️ `bc` 恒是 `Breadcrumb::new("")`（根）：整棵树那一支没有"当前在哪一层"。
///
/// ⚠️ `action` 与 `selected` **必须来自同一个勾选面**：底栏那几格说的是同一件事
///    （已选几项 / 合计多大 / 按下去会怎样 / 现在能不能按），任何一格用了另一个面，
///    用户看到的就是一句自相矛盾的底栏 —— 而它们都是字符串，**不会有任何东西变红**。
///
/// 🔴 **任务 17 起，`sel` 与 `action` 是按"前端报上来的那个勾选面"算的**（在此之前是
///    内核的 `default_selected`）—— 见 `commands.rs:whole_tree`。这一格新增的
///    `action.blocked_reason` 正是**逼出**这件事的那一格：它说的是"**当前**这个勾选面
///    按下去会不会被挡"，用另一个面算出来的答案**没有意义**（而旁边那两格若还按老面算，
///    底栏就会自相矛盾）。`selected` 仍然是内核给的选择面**原样透传**（它管的是"勾选面该
///    播种成什么"，只在首帧用一次），两张面在首帧那一刻是同一个值。
pub fn whole(
    bc: &Breadcrumb,
    progress: Option<&ProgressSummary>,
    sel: &SelectionSummary,
    selected: &[String],
    action: &DownloadAction,
) -> Value {
    envelope::ok(serde_json::json!({
        "breadcrumb": to_wire(bc),
        "progress": to_wire(&progress),
        "selection": to_wire(sel),
        "selected": selected,
        "action": to_wire(action),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::browser_row::{BrowserRowKind, RowStateStyle};
    use crate::presentation::download_targets::DownloadTargets;

    /// 一份底栏动作值。**算子与命令层调用点逐字相同**（`DownloadTargets` 的两个纯函数），
    /// 不是手写的字面量：手写的话，"回执里发的是不是 `DownloadTargets` 算的那一句"
    /// 在断言上不可观测（改了 `presentation` 的文案，这些测试照样全绿）。
    fn an_action(selected: &[&str]) -> DownloadAction {
        let set: std::collections::BTreeSet<String> =
            selected.iter().map(|s| (*s).to_string()).collect();
        DownloadAction {
            button_title: DownloadTargets::button_title(&set),
            empty_selection_hint: if set.is_empty() {
                Some(DownloadTargets::empty_selection_hint().to_string())
            } else {
                None
            },
            // ⚠️ 这一格**不手写**：它由判据算（`blocked_reason`），入参是"真正要发出去的那一串"
            //    —— 这里给的就是那个集合本身（这批是一个正常规模的勾选面 ⇒ 恒 `None`）。
            //    手写 `None` 的话，"这一格到底带没带上判据"在断言上不可观测。
            blocked_reason: DownloadTargets::blocked_reason(&set.iter().cloned().collect::<Vec<_>>())
                .map(str::to_string),
        }
    }

    fn a_row(name: &str) -> BrowserRow {
        BrowserRow {
            kind: BrowserRowKind::File,
            name: name.to_string(),
            path: format!("01.RawData/{name}"),
            children_count: None,
            size: Some(1024),
            detail_text: "1.0 KB".to_string(),
            source_time_text: "—".to_string(),
            state: RowStateStyle::of(None),
            icon_name: "file".to_string(),
        }
    }

    /// 读到了某一层：`breadcrumb` + `rows` 两格，行**逐格透传**（`BrowserRow` 是界面值）。
    #[test]
    fn a_loaded_level_carries_the_breadcrumb_and_the_rows() {
        let bc = Breadcrumb::new("01.RawData/02.CleanData");
        let v = level(&bc, &[a_row("a.fq.gz"), a_row("b.fq.gz")]);
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["data"]["breadcrumb"]["path"], serde_json::json!("01.RawData/02.CleanData"));
        assert_eq!(v["data"]["rows"][0]["name"], serde_json::json!("a.fq.gz"));
        assert_eq!(v["data"]["rows"].as_array().map(|a| a.len()), Some(2));
        // 行里的每一格都在（少一格 ⇒ 界面上某一栏空白，而不会有东西变红）。
        // ⚠️ 这是**九格**的清单（`state` 不在里面 —— 它被 `row_wire` 删掉了，
        //    理由见那个函数：它是没有消费者的派生变体名）。
        for key in [
            "kind",
            "name",
            "path",
            "children_count",
            "size",
            "detail_text",
            "source_time_text",
            "icon_name",
            // ⚠️ 这两格是**状态列真正显示的那两句**（值来自 `RowStateStyle` 的两个方法）。
            "state_label",
            "state_color",
        ] {
            assert!(v["data"]["rows"][0].get(key).is_some(), "行里缺了 {key}：{}", v["data"]["rows"][0]);
        }
        // 🔴 **反向的一半**：`state`（`Serialize` 派生出来的变体名）**不许**出现在回执里。
        //    判别力：把 `row_wire` 里那行 `remove("state")` 删掉，这一条立刻红 ——
        //    而真机上没人会发现（多一个没人读的键，界面照旧）。
        //    R-35 的另一半（"别发没人读的字段"）就是靠这一条守的。
        assert!(
            v["data"]["rows"][0].get("state").is_none(),
            "回执里出现了 `state`（派生出来的变体名，没有消费者 —— 前端读的是 state_label / state_color）：{}",
            v["data"]["rows"][0]
        );
        // 而且**整行就是那 11 格**（多一格少一格都要有人来看一眼）。
        let keys: Vec<&String> = v["data"]["rows"][0].as_object().expect("一行是个对象").keys().collect();
        assert_eq!(
            keys,
            vec![
                "children_count", "detail_text", "icon_name", "kind", "name", "path",
                "size", "source_time_text", "state_color", "state_label"
            ],
            "行的形状变了（前端的读法要跟着改）：{}",
            v["data"]["rows"][0]
        );
    }

    // -----------------------------------------------------------------------
    // 状态列那两格（R-35：Rust 的方法不在载荷里 ⇒ 前端会拿不到显示值）
    // -----------------------------------------------------------------------

    /// 内核会发的那种线上原文（`list_dir` 的 result）：一个目录 + 四态的四个文件。
    ///
    /// ⚠️ 夹具走的是**"内核 JSON → `BrowserRow`"这条真路**（不是手搓结构体）：
    ///    手搓的话，"壳解不解得动内核的输出"这件事在测试里凭空消失
    ///    （`browser_row.rs` 的夹具头记着同一条理由）。
    /// ⚠️ **四态一个都不能少**：只放一两态的话，"标签取错了列"这类错误在断言上不可观测。
    const LEVEL_WIRE: &str = r#"{"path":"01.RawData",
     "entries":[
       {"type":"dir","name":"Figure","children_count":2},
       {"type":"file","name":"a.fq.gz","path":"01.RawData/a.fq.gz","crc64":"","size":1024,
        "completed":0,"total":1024,"speed":0,"state":"pending","err":"",
        "source_mtime":"2026-09-14T12:00:00+08:00"},
       {"type":"file","name":"b.fq.gz","path":"01.RawData/b.fq.gz","crc64":"","size":2048,
        "completed":1,"total":2048,"speed":10,"state":"downloading","err":"","source_mtime":""},
       {"type":"file","name":"c.fq.gz","path":"01.RawData/c.fq.gz","crc64":"","size":4096,
        "completed":4096,"total":4096,"speed":0,"state":"complete","err":"","source_mtime":""},
       {"type":"file","name":"d.fq.gz","path":"01.RawData/d.fq.gz","crc64":"","size":8192,
        "completed":0,"total":8192,"speed":0,"state":"failed","err":"磁盘满了","source_mtime":""}]}"#;

    fn level_rows_from_kernel_wire(json: &str, parent: &str) -> Vec<BrowserRow> {
        let result: crate::protocol::ListDirResult =
            serde_json::from_str(json).expect("这是内核会发的线上 JSON，必须解得出");
        BrowserRow::rows(&result.entries, parent)
    }

    fn level_wire_rows(json: &str) -> Value {
        let rows = level_rows_from_kernel_wire(json, "01.RawData");
        level(&Breadcrumb::new("01.RawData"), &rows)
    }

    /// 🔴 **每一行都要带上状态列的那两句显示值**（`state_label` / `state_color`）。
    ///
    /// 判别力：把 `row_wire` 里那两行删掉（回到"只发派生形状"），这一条立刻红 ——
    /// 而真机上的表现是**状态列整列空白**（前端 `row.state_label` 取到 `undefined`），
    /// 那在没有这条断言时**不会有任何东西变红**。
    ///
    /// ⚠️ 期望值**逐字写死**，不从 `RowStateStyle` 现取：现取的话，改了 `presentation`
    ///    里的文案两边会一起变，这条断言就变成恒真的了（"用被测对象算期望值"）。
    #[test]
    fn every_row_carries_its_state_label_and_color() {
        let v = level_wire_rows(LEVEL_WIRE);
        let rows = v["data"]["rows"].as_array().expect("rows 是个数组");
        // 顺序 = `BrowserRow::rows` 的既有判据：目录在前，同组按名字升序。
        let want = [
            ("Figure", "—", "Secondary"),
            ("a.fq.gz", "待下载", "Secondary"),
            ("b.fq.gz", "下载中", "Blue"),
            ("c.fq.gz", "已完成", "Green"),
            ("d.fq.gz", "失败", "Red"),
        ];
        assert_eq!(rows.len(), want.len(), "{v}");
        for (row, (name, label, color)) in rows.iter().zip(want) {
            assert_eq!(row["name"], serde_json::json!(name), "{v}");
            assert_eq!(row["state_label"], serde_json::json!(label), "{row}");
            assert_eq!(row["state_color"], serde_json::json!(color), "{row}");
        }
    }

    /// ⚠️ **`kind` 与 `state_color` 的线上形态是契约** —— 前端就是照这两个字符串比的
    /// （`js/screens/files.js` 的 `KIND_DIR` 与 `ROW_COLOR_CLASS`）。
    ///
    /// 这条钉的是"改线上形态要连测试一起改"：把 `BrowserRowKind` 改成
    /// `#[serde(rename_all = "lowercase")]`（或给 `RowColor` 换个名字），
    /// **这一条立刻红**，而不是让前端在下一次运行时静默地比不中
    /// （那会让"双击目录"变成"下载整个目录"，或者整列状态没有颜色）。
    ///
    /// ⚠️ 两个形态**都不是随手定的**：它们是 `Serialize` 派生自这两个枚举的
    ///    **变体名**（`BrowserRowKind::Dir` ⇒ `"Dir"`、`RowColor::Blue` ⇒ `"Blue"`）。
    ///    本次**没有**把它们改成小写：那是改一个已经被 232 条测试覆盖着的
    ///    `presentation` 类型的线上形状，收益只是"好看一点"，而代价是
    ///    前端与 macOS 两侧的对照表都要跟着动（详见任务 10 报告 §8）。
    #[test]
    fn the_wire_forms_of_kind_and_color_are_pinned() {
        let v = level_wire_rows(LEVEL_WIRE);
        let rows = v["data"]["rows"].as_array().expect("rows 是个数组");
        let kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap_or("")).collect();
        assert_eq!(
            kinds,
            vec!["Dir", "File", "File", "File", "File"],
            "行判别键的线上形态变了（前端按它判双击是「进入」还是「入队」）：{v}"
        );
        let colors: Vec<&str> = rows.iter().map(|r| r["state_color"].as_str().unwrap_or("")).collect();
        assert_eq!(
            colors,
            vec!["Secondary", "Secondary", "Blue", "Green", "Red"],
            "状态色的线上形态变了（前端按它选 CSS 类）：{v}"
        );
        // 目录那一行的标签是占位字形（`RowStateStyle::None`），**不是**空白 ——
        // 空白在界面上就是"这一格什么都没说"（约束 4）。
        assert_eq!(rows[0]["state_label"], serde_json::json!("—"), "{v}");
    }

    /// ⚠️ **读不到某一层仍然是 `ok` 信封**（上游 `routes.rs:285` 逐字是 `status: 200` +
    ///    `envelope::ok`）。判别力：把它改成 `envelope::err(…)`，这一条立刻红 ——
    ///    而真机上的表现是**整屏**报错，而不是列表那一块显示"这一层没读成"。
    #[test]
    fn a_failed_level_is_still_an_ok_envelope() {
        let bc = Breadcrumb::new("");
        let f = DirLoadFailure {
            message: "内核原文".to_string(),
            path: String::new(),
            notice: Some("已返回上一层".to_string()),
        };
        let v = level_failure(&bc, &f);
        assert_eq!(v["ok"], serde_json::json!(true), "读不到某一层是界面状态，不是整屏错误：{v}");
        assert!(v.get("error").is_none(), "{v}");
        assert_eq!(v["data"]["failure"]["message"], serde_json::json!("内核原文"));
        assert_eq!(v["data"]["failure"]["notice"], serde_json::json!("已返回上一层"));
        // 面包屑跟着**退到的那一层**走（这里退到了根）。
        assert_eq!(v["data"]["breadcrumb"]["path"], serde_json::json!(""));
    }

    /// ⚠️ **没有生效批次时 `progress` 必须是 `null`**，不是编一个"0% / 0 B"。
    ///
    /// 判别力：把 `progress` 改成 `&ProgressSummary` 再编一个零值，这一条立刻红 ——
    /// 那正是本函数文档里记的那条偏离存在的理由。
    #[test]
    fn a_whole_tree_without_an_active_batch_reports_a_null_progress() {
        let sel = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let v = whole(&Breadcrumb::new(""), None, &sel, &[], &an_action(&[]));
        assert_eq!(v["data"]["progress"], serde_json::Value::Null, "没有生效批次 ⇒ progress 是 null：{v}");
    }

    /// 有进度时是那四格界面值（`ProgressSummary`）。
    #[test]
    fn a_whole_tree_carries_the_progress_summary_when_there_is_one() {
        let sel = SelectionSummary { count_text: "已选 1 项".to_string(), size_text: "合计 1.0 KB".to_string() };
        let progress = ProgressSummary {
            percent_text: "50%".to_string(),
            bytes_text: "1.0 KB / 2.0 KB".to_string(),
            speed_text: "—".to_string(),
            fraction: 0.5,
        };
        let v = whole(&Breadcrumb::new(""), Some(&progress), &sel, &["a".to_string()], &an_action(&["a"]));
        assert_eq!(v["data"]["progress"]["percent_text"], serde_json::json!("50%"));
        assert_eq!(v["data"]["selection"]["count_text"], serde_json::json!("已选 1 项"));
        assert_eq!(v["data"]["selected"], serde_json::json!(["a"]));
    }

    /// ⚠️ **`selected` 原样透传**（`api/mod.rs` 文件头第 3 条例外）：它是**内核给的**一串
    ///    路径原文，壳没有加工它 —— 所以它必须**逐字**出现在回执里（约束 3）。
    ///    判别力：哪天有人"顺手"把它换成壳算的那份（`sel` 的入参面），这一条会红。
    #[test]
    fn the_selected_paths_are_passed_through_verbatim() {
        let sel = SelectionSummary { count_text: "已选 2 项".to_string(), size_text: "合计 0 B".to_string() };
        let raw = vec!["01.RawData/带中文的文件.fq.gz".to_string(), "b".to_string()];
        let v = whole(&Breadcrumb::new(""), None, &sel, &raw, &an_action(&["a", "b"]));
        assert_eq!(v["data"]["selected"], serde_json::json!(raw));
    }

    /// 整棵树那一支**不发 `flat`**（裁定逐字）：它是壳的中间产物，不进回执。
    #[test]
    fn the_flat_list_never_reaches_the_client() {
        let sel = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let v = whole(&Breadcrumb::new(""), None, &sel, &[], &an_action(&[]));
        let keys: Vec<&String> = v["data"].as_object().expect("data 是个对象").keys().collect();
        assert_eq!(
            keys,
            vec!["action", "breadcrumb", "progress", "selected", "selection"],
            "整棵树那一支的格子是固定的：{v}"
        );
    }

    // -----------------------------------------------------------------------
    // 底栏那个动作（任务 10 补的第三条缺口）
    // -----------------------------------------------------------------------

    /// ⚠️ **`action.button_title` 必须逐字是 `DownloadTargets::button_title` 的结果**
    ///    —— 它同时喂给底栏与工具栏两颗按钮（`RootView.swift` 钉着"两处永远相同"）。
    ///
    /// 判别力：把发出去的值改成一个手写的字面量、或换成 `help_text`，这一条立刻红。
    /// 夹具**刻意覆盖"空勾"与"有勾"两态**：只测一态的话，"常数"与"算式"分不出来。
    #[test]
    fn the_download_action_carries_the_buttons_own_wording() {
        let empty = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let none = whole(&Breadcrumb::new(""), None, &empty, &[], &an_action(&[]));
        assert_eq!(none["data"]["action"]["button_title"], serde_json::json!("全部下载"), "{none}");

        let some = SelectionSummary { count_text: "已选 1 项".to_string(), size_text: "合计 1.0 KB".to_string() };
        let one = whole(&Breadcrumb::new(""), None, &some, &["a".to_string()], &an_action(&["a"]));
        assert_eq!(one["data"]["action"]["button_title"], serde_json::json!("下载选中"), "{one}");
    }

    /// ⚠️ **一句都没勾时那句「明说」必须在场**（约束 4：不许让用户自己猜
    ///    "不勾 = 下全部"）；有勾选时它是 `null`（**不是空串** —— 空串会渲染成一句空话）。
    ///
    /// 判别力：把它写成"恒发那句话"，或把 `None` 换成 `Some("")`，这一条立刻红。
    #[test]
    fn the_empty_selection_hint_appears_only_when_nothing_is_selected() {
        let empty = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let none = whole(&Breadcrumb::new(""), None, &empty, &[], &an_action(&[]));
        assert_eq!(
            none["data"]["action"]["empty_selection_hint"],
            serde_json::json!("未勾选任何项，将下载全部待下载文件"),
            "{none}"
        );

        let some = SelectionSummary { count_text: "已选 1 项".to_string(), size_text: "合计 1.0 KB".to_string() };
        let one = whole(&Breadcrumb::new(""), None, &some, &["a".to_string()], &an_action(&["a"]));
        assert_eq!(
            one["data"]["action"]["empty_selection_hint"],
            serde_json::Value::Null,
            "有勾选时那一格是 null（**不是空串**）：{one}"
        );
    }

    /// `action` 的三个格子**恒在**（`DownloadAction` 加字段、或本层顺手改键名时，
    /// 前端那一格会以 `undefined` 收场 —— 而那是**静默**的）。
    ///
    /// ⚠️ **`blocked_reason` 也在里面，哪怕它的值是 `null`**（任务 17）：这一格的口径与
    ///    `empty_selection_hint` 逐字相同 —— `None` **照样发出去**（序列化成 `null`），
    ///    不因为"此刻没这句话"就省略这个键。判别力：把那一格改成
    ///    `#[serde(skip_serializing_if = "Option::is_none")]`（缺席代替 `null`）⇒ 这一条红
    ///    —— 而真机上没人会发现（前端把"缺席"与 `null` 读成同一件事），
    ///    wire 里却多出了**第二条**"怎么表示没有"的约定（本代只认 `null` 这一条）。
    #[test]
    fn the_action_group_has_a_fixed_shape() {
        let sel = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let v = whole(&Breadcrumb::new(""), None, &sel, &[], &an_action(&[]));
        let keys: Vec<&String> = v["data"]["action"].as_object().expect("action 是个对象").keys().collect();
        assert_eq!(
            keys,
            vec!["blocked_reason", "button_title", "empty_selection_hint"],
            "动作那一组的格子是固定的：{v}"
        );
        assert_eq!(
            v["data"]["action"]["blocked_reason"],
            serde_json::Value::Null,
            "可以按时它是 null，**不是省掉这个键**：{v}"
        );
    }

    /// 🔴 **"按不下去"那一格要原样到达前端**（任务 17：C-3 的另一半）。
    ///
    /// 判别力：把命令层换成"永远传 `None`"、或让 `whole` 把这一格丢掉 ⇒ 这一条红 ——
    /// 而真机上那是**界面永远不拦**：用户勾了大半个超大批次、点下去撞上一句他看不懂的错，
    /// 界面从头到尾没告诉他门槛在哪。
    ///
    /// ⚠️ 期望值是**手写的字面量**（不从 `DownloadTargets` 现取）：现取的话，
    ///    改了那句话两边会一起变，这条断言就恒真了（"用被测对象算期望值"）。
    #[test]
    fn an_unavailable_action_carries_its_reason() {
        let sel = SelectionSummary { count_text: "已选 1 项".to_string(), size_text: "合计 0 B".to_string() };
        let blocked = DownloadAction {
            button_title: "下载选中".to_string(),
            empty_selection_hint: None,
            blocked_reason: Some("选中的项太多，超过一次能发出的上限：请少选一些".to_string()),
        };
        let v = whole(&Breadcrumb::new(""), None, &sel, &[], &blocked);
        assert_eq!(
            v["data"]["action"]["blocked_reason"],
            serde_json::json!("选中的项太多，超过一次能发出的上限：请少选一些"),
            "这一格是给前端禁用那个动作用的，必须逐字到达：{v}"
        );
        // ⚠️ 同一组里的另外两格**不受它影响**（"能不能按"与"按钮叫什么"是两件事：
        //    按钮仍然叫「下载选中」—— 它只是此刻按不下去）。
        assert_eq!(v["data"]["action"]["button_title"], serde_json::json!("下载选中"), "{v}");
        assert_eq!(v["data"]["action"]["empty_selection_hint"], serde_json::Value::Null, "{v}");
    }

    /// ⚠️ **"按不下去"与"一项都没勾"互斥**（两者同时出现就是一句自相矛盾的底栏）。
    ///
    /// 判别力：把 `an_action` 里的 `blocked_reason` 改成"恒 `Some(...)`"（先挡住再说）
    /// ⇒ 空勾选那一支会同时带上"未勾选任何项…"与"选中的项太多…"两条 —— 而空勾选
    /// 发出去的是**空数组**（请求体恒定），它**不可能**超预算。
    #[test]
    fn the_blocked_reason_never_shows_up_next_to_the_empty_selection_hint() {
        let empty = SelectionSummary { count_text: "已选 0 项".to_string(), size_text: "合计 0 B".to_string() };
        let v = whole(&Breadcrumb::new(""), None, &empty, &[], &an_action(&[]));
        assert_eq!(
            v["data"]["action"]["empty_selection_hint"],
            serde_json::json!("未勾选任何项，将下载全部待下载文件"),
            "{v}"
        );
        assert_eq!(v["data"]["action"]["blocked_reason"], serde_json::Value::Null, "{v}");
    }
}
