//! `/api/transfers` 那一格的载荷（搬自 `shell-win/src/server/routes.rs:389` 的 `transfers`）。
//!
//! ⚠️ **三格的含义各不相同**（上游那三个 `TransferPoll` 分支各发一种）：
//!
//! | 内核那边 | 这里发什么 |
//! |---|---|
//! | 快照（`TransferPoll::Snapshot`） | `rows` 有内容 + `global` 有值 |
//! | 内核亲口说的**正常态**（还没添加过任务 / 这一批没了） | `rows: []` + **`global: null`** |
//! | 失败（`TransferPoll::Failed`） | 走 [`crate::api::failure`]，不进本函数 |
//!
//! ⚠️ **`global` 为 `null` 与 `rows: []` 是两件事**，别把它们一起折成"空列表"：
//!    空列表是**事实**（确实没有任务），`global` 缺是**内核这条回执里根本没有那个字段**
//!    ——替内核填三个 0 就是替它说了一句它没说的话（见下）。

use serde_json::Value;

use crate::api::{envelope, to_wire};
use crate::presentation::transfer_row::{TransferGlobalSummary, TransferRow};
use crate::presentation::verify_summary::SidebarBadge;

/// 传输列表：行 + 全局摘要 + 空态那句话。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:389` 的 `transfers`。
/// **对齐 macOS**：`Presentation/TransferRow.swift`（`TransferRow` / `TransferGlobalSummary`
/// / `TransferListEmpty`，三份都已移植到 `presentation/transfer_row.rs`）。
///
/// - `rows`：由调用方把内核的 `TransferItem` 逐条过 [`TransferRow::new`] 之后的界面值；
/// - `global`：`None` ⇒ 发 `null`（**不是**编一个"0 / 0 / 0"：该内核回执里根本没有
///   `global` 这个字段 —— 上游 `routes.rs:411` 逐字记着这条）；
/// - `empty`：空列表那句话由 `TransferListEmpty::of` 给（"还没有添加过任务"与
///   "确实没有任务"是**两句不同的话**，而引擎不可用时它**不说话** ——
///   顶部横幅已经在说同一句了）。`None` ⇒ 发 `null`。
///
/// ⚠️ 本函数对入参**不做任何省略**：`rows` / `global` / `empty_text` / `badge_count`
///    四格恒在（形状固定 ⇒ 前端只有一条读法）。"有没有内容"由值表达，不由"键在不在"表达。
///
/// ## 🔴 `badge_count`：侧栏那颗徽标的计数（**第五格，补上的**）
///
/// 它是「传输列表」分区那颗未完成计数徽标的数 —— 与**这一屏**同一个快照算出来的，
/// 所以它跟着这一格载荷一起下来，而不是从第二条命令来。
///
/// ⚠️ **它为什么必须由壳算**（规格 §3.2）：那颗徽标要显示的是"还有几个没落定的"，
///    而"哪些算未落定"是 [`TransferRow::is_unfinished`] 那一条判据 ——
///    交给 JS 数就等于把那条判据抄进前端（`Removed`/`Complete` 不算这件事
///    一旦有第二份实现，症状是"徽标上的数与列表里看得见的东西对不上"）。
///    ⇒ 判据在 `presentation`，本层**只把它数一遍**（[`SidebarBadge::unfinished_of_rows`]）。
///
/// ⚠️ **对齐 macOS**：那边的徽标读的是 `AppModel.transfers`
///    （`Sidebar.swift:83` → `SidebarBadge.unfinishedTransfers`），而那一份快照
///    **只在传输列表分区可见时**被刷新（`RootView.pollLoop` 的闸就是
///    `section == .transfers`）⇒ 徽标反映的本来就是"最后一次看到它时的样子"。
///    这一格走同一条口径：随这一屏的载荷下来，不额外起一条轮询。
pub fn transfers(
    rows: &[TransferRow],
    global: Option<&TransferGlobalSummary>,
    empty: Option<&str>,
) -> Value {
    envelope::ok(serde_json::json!({
        "rows": to_wire(rows),
        "global": to_wire(&global),
        // ⚠️ `empty` 是 `&str` 那一份的原文（`TransferListEmpty::of` 的产物），
        //    壳不再加工它一个字（约束 3 的同一条纪律：交给前端的就是最终显示的那句话）。
        "empty_text": empty,
        // ⚠️ **空列表也是 `0`**（不是"没有这一格"）：键恒在，值由数表达
        //    —— 前端只有一条读法（与上面三格同一条纪律）。
        "badge_count": SidebarBadge::unfinished_of_rows(rows),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::RowColor;

    fn a_row(gid: &str) -> TransferRow {
        TransferRow {
            gid: gid.to_string(),
            title: format!("任务 {gid}"),
            manifest_path: Some("/tmp/a.json".to_string()),
            progress_text: "1.0 KB / 2.0 KB".to_string(),
            percent_text: "50%".to_string(),
            progress_fraction: 0.5,
            speed_text: "1.2 MB/s".to_string(),
            state_label: "下载中".to_string(),
            state_color: RowColor::Blue,
            state_icon_name: "arrow-down".to_string(),
            shows_paused_badge: false,
            error_text: None,
            available_actions: Default::default(),
            // 这一行的状态是「下载中」⇒ 算未落定（判据在 `TransferRow::is_unfinished`）。
            counts_as_unfinished: true,
        }
    }

    /// 一行**已落定**的（「已完成」）：徽标不许把它算进去。
    fn a_finished_row(gid: &str) -> TransferRow {
        TransferRow {
            counts_as_unfinished: false,
            ..a_row(gid)
        }
    }

    /// 快照那一支：行与全局摘要都在，空态那句**也在**（形状固定）。
    #[test]
    fn a_snapshot_carries_the_rows_and_the_global_summary() {
        let global = TransferGlobalSummary {
            speed_text: "1.2 MB/s".to_string(),
            activity_text: "1 个任务进行中".to_string(),
            can_clear_finished: false,
        };
        let v = transfers(&[a_row("g1")], Some(&global), None);
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["data"]["rows"][0]["gid"], serde_json::json!("g1"));
        assert_eq!(v["data"]["global"]["speed_text"], serde_json::json!("1.2 MB/s"));
        assert_eq!(v["data"]["empty_text"], serde_json::Value::Null);
    }

    /// 🔴 **侧栏那颗徽标的计数过线**（`badge_count`）—— 它**曾经整格不存在**，
    ///    而那一格不存在时前端**不会报错**：它只会把徽标永远藏着（`windows/web/index.html`
    ///    的三个 `#badge-*` 恒 `hidden`），macOS 上那句"去那一屏看看"的提示当场消失。
    ///
    /// 判别力（每一半都会红）：
    ///   * 把 `badge_count` 从 `transfers()` 里删掉 ⇒ `get(...)` 是 `None` ⇒ 第一条断言红；
    ///   * 把"哪些算未落定"改宽（把已落定的那一行也算进去）⇒ 第二条断言红；
    ///     ⚠️ 这一半才是要紧的那一半：只断言"这个键在"的测试**挡不住**"数错了"。
    ///   * 把计数搬去 JS（规格 §3.2 明禁）⇒ 这一格不会被发出来，第一条也红。
    #[test]
    fn the_sidebar_badge_count_reaches_the_client() {
        let rows = [a_row("g1"), a_finished_row("g2"), a_row("g3")];
        let v = transfers(&rows, None, None);
        assert!(
            v["data"]["badge_count"].is_u64(),
            "badge_count 那一格没有发出来（侧栏那一格会永远是 hidden）：{v}"
        );
        assert_eq!(
            v["data"]["badge_count"],
            serde_json::json!(2),
            "未落定的行数是 2（g1 / g3；g2 是已落定的那一行）：{v}"
        );
        // ⚠️ 空列表也是 `0`，不是"这一格不见了"（键恒在，值由数表达）。
        assert_eq!(transfers(&[], None, None)["data"]["badge_count"], serde_json::json!(0));
    }

    /// ⚠️ **`global: null` 不是"0 / 0 / 0"**（上游 `routes.rs:411` 那条注释的判据）。
    ///
    /// 判别力：把 `global` 改成"没有就编三个 0"，这一条立刻红 —— 而真机上的表现是
    /// 底栏显示一个**内核从没说过的**速度/活动量（静默失效，且没人查得出来）。
    #[test]
    fn a_missing_global_is_null_and_never_a_made_up_zero_row() {
        let v = transfers(&[], None, Some("没有正在传输的任务"));
        assert_eq!(v["data"]["rows"], serde_json::json!([]));
        assert_eq!(v["data"]["global"], serde_json::Value::Null, "缺 global 就是 null：{v}");
        assert_eq!(v["data"]["empty_text"], serde_json::json!("没有正在传输的任务"));
    }

    /// 空列表那句话**逐字**进回执（它是 `TransferListEmpty` 的原文，壳不加工）。
    #[test]
    fn the_empty_text_is_passed_through_verbatim() {
        let raw = "下载引擎尚未启动（还没有添加过任务）";
        let v = transfers(&[], None, Some(raw));
        assert_eq!(v["data"]["empty_text"], serde_json::json!(raw));
    }

    /// **四格恒在**，哪怕四样都是空的（形状固定 ⇒ 前端只有一条读法）。
    ///
    /// ⚠️ 键名**逐字**列在这里（不是"长度是 4"）：那样才挡得住"少一格、多一格长得一样"
    ///    —— `badge_count` 就是这么补进来的（它曾经整格不存在，而前端只会把徽标永远藏着）。
    #[test]
    fn the_four_keys_are_always_present() {
        let v = transfers(&[], None, None);
        let keys: Vec<&String> = v["data"].as_object().expect("data 是个对象").keys().collect();
        assert_eq!(
            keys,
            vec!["badge_count", "empty_text", "global", "rows"],
            "四格恒在：{v}"
        );
    }
}
