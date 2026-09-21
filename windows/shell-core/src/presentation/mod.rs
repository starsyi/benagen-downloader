//! presentation —— macOS 版 `Presentation/` 的 **Rust 移植**（规格 §4.3）。
//!
//! 上游是 `macos/Sources/BenagenCoreKit/Presentation/` 下的那 9 个文件：它们全都被
//! 刻意写成**纯计算**（无状态、无 locale 依赖、不 `import SwiftUI`），所以能原样搬到
//! 本 crate 里来、在 macOS 上照跑不误。搬过来的东西只有两点不同：
//!
//!   1. Swift 的 `enum` + `static func` 命名空间在这里是**空 `enum` + 固有 `impl`**
//!      （Rust 没有"不能实例化、只当命名空间用"的类型特性，空 `enum` 是最接近的写法：
//!      它**照样不可构造**，语义与 Swift 那边逐字一致）；
//!   2. 集合类型换成本 crate 通行的 `BTreeSet`/`Vec`（见各文件的模块注释）。
//!
//! ⚠️ **本模块不是"另写一份"**：每一条规则都以 Swift 源为准逐字照抄，包括边界值、
//!    占位符（`—`）与**截断而非四舍五入**这类口径。偏离一律写在它自己的注释里（W-6）。
//!
//! ⚠️ 纪律：**谁创建模块文件，谁负责在本文件里加 `pub mod X;`**。一个建了却没被声明的
//!    `.rs` 文件，它的测试**一条都不会跑**，而 `cargo test` 照样全绿（见 `lib.rs` 头部）。

use serde::Serialize;

pub mod about_info;
pub mod app_preferences;
pub mod batch_history;
pub mod breadcrumb;
pub mod browser_primary_action;
pub mod browser_row;
pub mod delivery_summary;
pub mod delivery_switch;
pub mod download_targets;
pub mod engine_status;
pub mod error_text;
pub mod format;
pub mod kernel_death;
pub mod resident_notice;
pub mod settings_form;
pub mod transfer_row;
pub mod verify_summary;

/// 行状态的**语义色**。枚举，**不是具体的颜色值**——本 crate 不引入任何绘制依赖，
/// "枚举 → 实际颜色"的那一步留在 `shell-win`（与 macOS 侧留的那一步同形）。
///
/// ⚠️ 成员是规格 §7.2「语义色（成功/警告/错误）」的那几个 + 中性：
///    [`RowColor::Green`] = 成功、[`RowColor::Orange`] = 警告、[`RowColor::Red`] = 错误、
///    [`RowColor::Blue`] = 进行中、[`RowColor::Secondary`] = 中性 / 未开始。
///
/// ⚠️ **加成员时每一处 `match` 都会因为不再穷尽而编译不过**——那是刻意的，
///    别用 `_ =>` 把它盖掉（macOS 源里同一句话针对的是 `switch` + `default`；
///    这是移植时唯一的改写，语义不变）。
///
/// ⚠️ **记账：本枚举是 5 个成员，不是 6 个——计划表与源在这里对不上。**
///    **本计划**（`docs/superpowers/plans/2026-09-18-windows-client-phase1.md`，
///    标题「Windows 客户端（阶段 1）实现计划」）的任务表写的是
///    `Red, Green, Orange, Gray, Blue, Secondary`（6 个）
///    并把它归到 `BrowserRow.swift:186`，但**那一行的实际内容只有 5 个**：
///    `case secondary, blue, green, red, orange`（`Gray` 在计划里被凭空多写了，
///    源侧全仓 `grep -rn "\.gray" Sources/BenagenCoreKit/` **零命中**，
///    任务 13/14/15 的 API 块里也一处都没提到它）。**按源取 5 个**：
///    "源是权威、计划的摘要表不是"——本阶段已经用同一条口径纠过一次出处错误（账本缺陷 #10）。
///    这段记录**故意留着**：下一个照计划表数成员的人会撞上同一处矛盾，
///    届时以本文件与 `BrowserRow.swift:186` 为准，不要再把 `Gray` 加回来。
///
/// ⚠️ 变体名**逐字**来自源，顺序也照源（源是 `case secondary, blue, green, red, orange`；
///    Rust 的变体名按语言惯例写成 PascalCase，其余一字不改）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum RowColor {
    Secondary,
    Blue,
    Green,
    Red,
    Orange,
}
