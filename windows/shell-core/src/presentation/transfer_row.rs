//! transfer_row —— 传输列表的**呈现模型**：行 / 全局汇总 / 顶部横幅 / 空态 / 准入闸门 /
//! 轮询节拍 / 清单路径 → 落盘路径。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/TransferRow.swift`（逐字对位）。
//!
//! 全部是纯函数：无状态、无 locale 依赖、不引入任何绘制依赖（颜色用 [`RowColor`] 枚举表达，
//! "枚举 → 实际颜色"的那一步留在 `shell-win`，与上游把 `Color` 留在只负责画的那一层同形）。
//!
//! 约束 4「不得静默失效」在本模块的落点最密：五档状态两两不同（少一个就是客户看不出来）、
//! 动作准入（点一处注定报错的地方比没有更糟）、`raw_status` 才是「已暂停」的来源（裁决 #88）、
//! 内核原文照登（约束 3），以及准入闸门（约束 4/15：内核卡死时静默挂死是本项目明禁的形态）。
//!
//! ⚠️ 本 crate **不含界面概念**（章程见 `lib.rs` 头部）：本文件里没有一处**描述长什么样**
//!    （"画成什么颜色/什么图形"全在 `shell-win`）。**值本身**可以带着"客户看得见"的语义
//!    （例如 [`TransferRow::can_reveal_in_finder`] 说"没有路径时那一项不可用"）——
//!    那是判据，不是外观；判据属于这里。名字里带 `finder` 的那个方法见它自己的注释。
//!
//! ⚠️ **W-6（命名）**：所有公开名字取自任务简报的 API 块（它把上游的 `camelCase` 换成
//!    `snake_case`）；上游有而简报没点名的那些（`fraction`/`label`/`color`/`icon_name`/
//!    `actions`）保持**私有** —— 它们只被 [`TransferRow::new`] 消费，多一个公开面就多一处
//!    日后要跟着改的 `use` 路径（同 `engine_status.rs` 对 `HANDSHAKE_TIMEOUT_MESSAGE` 的记账）。

use serde::Serialize;
use crate::client::ClientError;
use crate::presentation::engine_status::EngineStatusPresentation;
use crate::presentation::error_text::error_text;
use crate::presentation::format::{ByteFormat, PercentFormat, SpeedFormat};
use crate::presentation::RowColor;
use crate::protocol::{EngineState, GlobalStat, TaskAction, TaskState, TransferItem};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// 行：标题 / 进度 / 状态 / 动作
// ---------------------------------------------------------------------------

/// 传输列表里的一行：**已经算好的呈现值**。
///
/// 「文件名/相对路径 · 进度 · 速度 · 状态」四列的值都在这里成形，`shell-win` 侧只剩绑定。
///
/// ⚠️ 与上游同形（`Equatable, Sendable`）：这里派生 `Clone, PartialEq, Debug`。
///    `Sendable` 在 Rust 里由类型系统默认保证（没有内部可变性/裸指针就自动成立），
///    而 **`Eq` 刻意不派生** —— `progress_fraction` 是 `f64`，派生 `Eq` 编不过，
///    而且这里要比的是"算出来的值相不相等"，不是全序。
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct TransferRow {
    /// aria2 的 GID —— 这一行的身份，也是 `task_action` 要的那个 gid。
    pub gid: String,
    /// 「文件名/相对路径」那一列。**清单原文**（约束 3），`None` 映射时是占位符。
    pub title: String,
    /// 内核给的清单路径**原文**；`None` = 内核说这个 GID 没有路径映射
    /// （空串也算没有：见 [`TransferReveal::manifest_path`]）。
    pub manifest_path: Option<String>,
    /// 「进度」那一列：`已完成 / 总量`。
    pub progress_text: String,
    /// 「进度」那一列的百分比。总量为 0 时是占位符 `—`（口径同 `PercentFormat`）。
    pub percent_text: String,
    /// 进度条控件要的那个数，**恒在 `0.0..=1.0`**（含总量为 0 的 0，绝不 NaN）。
    pub progress_fraction: f64,
    /// 「速度」那一列。`0` 显示 `—`（口径同 `SpeedFormat`）。
    pub speed_text: String,
    /// 「状态」那一列的文字。五档**两两不同**（约束 4）。
    pub state_label: String,
    /// 「状态」那一列的颜色（枚举，不是具体的颜色值）。
    pub state_color: RowColor,
    /// 行首那颗状态图标的名字（形态同 `BrowserRow` 的 `icon_name`）。
    ///
    /// ⚠️ 名字放呈现层而不是 `shell-win`，与 `BrowserRow.icon_name` 同一条理由：
    ///    它是"内核状态 → 呈现值"的映射，且**五档两两不同**这件事能被断言。
    ///    Windows **没有 SF Symbols**：这些名字在这里是**语义标签**，
    ///    "标签 → 实际画成什么"的那一步留在 `shell-win`。
    pub state_icon_name: String,
    /// 行首那颗「已暂停」角标。
    ///
    /// ⚠️ **只能**来自 `raw_status`（裁决 #88 / 设计规格 §8.4）：领域状态把 aria2 的
    ///    `paused` 与 `waiting` 合成同一个 `Waiting`，由 `state` 反推出来的「已暂停」
    ///    永远是假的。`raw_status` 取不到时内核传空串（"不知道"），那就**不显示**角标
    ///    —— 不要替内核猜。
    pub shows_paused_badge: bool,
    /// 行下方那条错误原文；`None` = 没有错误消息（内核的 `error_message` 是空串）。
    ///
    /// ⚠️ **不改写、不截断、不折叠**（约束 3）：aria2 的原话就是客户唯一能拿去搜索的东西。
    pub error_text: Option<String>,
    /// 这一行允许的动作。**空集 = 一个都不给**（比给一处注定报错的地方好）。
    pub available_actions: BTreeSet<TaskAction>,
    /// 这一行算不算「未落定」—— 侧栏那颗徽标计数的口径（判据见 [`TransferRow::is_unfinished`]）。
    ///
    /// 🔴 **它不是界面值，所以不进线上形状**（`#[serde(skip)]`）：前端的每一格都从上面
    ///    那几格取，这一格**一个消费者都没有**（R-35：别发没人读的字段）。它的用处只有一个：
    ///    `api::transfers` 收到的是**已经呈现好的行**（不是内核的 `TransferItem`），
    ///    而"数一数有几行没落定"这件事必须在**壳**这一侧算好再发出去
    ///    （规格 §3.2：JS 不许自己数）。⇒ 壳把判据随行带过来，`api` 只把它数一遍。
    #[serde(skip)]
    pub counts_as_unfinished: bool,
}

impl TransferRow {
    /// 内核没有给路径映射时的行标题（`path: null`）。
    ///
    /// 这一句是**壳自己写的**（同 `AppModel.busyReason` 的性质）：内核发的是一个 `null`，
    /// 没有可登的原文，而把 `null` 呈现成空白或字面量 "nil" 都是约束 4 说的静默失效。
    pub const UNKNOWN_PATH: &'static str = "（未知路径）";

    /// aria2 里"已暂停"的原始状态串（`core/src/engine/status.rs` 的透传值）。
    pub const PAUSED_RAW_STATUS: &'static str = "paused";

    /// `transfer_list` 的一条 → 一行。
    ///
    /// ⚠️ **W-6（命名）**：上游是 `init(_ item:)`，这里按简报的 API 块写成 `new(item)` ——
    ///    Rust 没有"与类型同名的构造器"那个惯例位置，`new` 是这个语言里的对位写法。
    pub fn new(item: &TransferItem) -> Self {
        let path = TransferReveal::manifest_path(item.path.as_deref());
        let is_paused = item.raw_status == Self::PAUSED_RAW_STATUS;
        Self {
            gid: item.gid.clone(),
            title: path.unwrap_or(Self::UNKNOWN_PATH).to_string(),
            manifest_path: path.map(str::to_string),
            progress_text: format!(
                "{} / {}",
                ByteFormat::text(item.completed),
                ByteFormat::text(item.total)
            ),
            percent_text: PercentFormat::text(item.completed, item.total),
            progress_fraction: Self::fraction(item.completed, item.total),
            speed_text: SpeedFormat::text(item.speed),
            state_label: Self::label(item.state).to_string(),
            state_color: Self::color(item.state),
            state_icon_name: Self::icon_name(item.state).to_string(),
            shows_paused_badge: is_paused,
            // 空串 = 内核说"没有错误消息"（`error_message` 的约定）。两者在呈现侧是同一件事：
            // 都没有那一行可显示。
            error_text: if item.error_message.is_empty() {
                None
            } else {
                Some(item.error_message.clone())
            },
            available_actions: Self::actions(item.state, is_paused, &item.gid),
            counts_as_unfinished: Self::is_unfinished(item.state),
        }
    }

    /// 这一档算不算「未落定」—— 侧栏「传输列表」那颗徽标的计数的**唯一判据**。
    ///
    /// ⚠️ `Removed` **不算**：它已经是"不用再管"的一档，出路在列表级的「清空已完成」，
    ///    把它算进"未完成"会让徽标永远减不到 0；`Complete` 更不算。
    ///
    /// 🔴 **两份消费者共用这一份判据**（谁都不许抄第二遍 —— 分叉的症状是"侧栏徽标上的数
    ///    与传输列表里看得见的东西对不上"）：
    ///      · `SidebarBadge::unfinished_transfers` 数的是**内核原始条目**（`TaskState`）；
    ///      · `SidebarBadge::unfinished_of_rows` 数的是**已经呈现好的行**（这条注释上面那一格）。
    ///    两者只在"手上拿到的是哪一种东西"上不同，口径必须逐字同源。
    ///
    /// ⚠️ 可见性与 [`TransferRow::fraction`] 同一条（`pub(crate)`）：`presentation` 里的
    ///    `verify_summary` 要用它，crate 外不需要。
    pub(crate) fn is_unfinished(state: TaskState) -> bool {
        match state {
            TaskState::Waiting | TaskState::Active | TaskState::Error => true,
            TaskState::Complete | TaskState::Removed => false,
        }
    }

    /// 这一行能不能被"定位到磁盘上的文件"。
    ///
    /// ⚠️ **W-6（命名；本模块唯一一处带平台色彩的名字）**：上游叫 `canRevealInFinder`
    ///    （访达 = macOS 的文件管理器），Windows 侧的对应动作叫「在资源管理器中显示」
    ///    （设计规格 §9）。**本 crate 只移植准入判据**（有没有 `manifest_path`），
    ///    那个动作本身由 `shell-win` 做 —— 于是这里保留上游的名字与判据，
    ///    "叫什么、怎么点"留在 `shell-win`（本 crate 不含界面概念，章程见 `lib.rs` 头部）。
    ///
    /// ⚠️ 这就是简报里那句「`path == nil` 时**该项禁用**，不要静默无效」的落点：
    ///    没有路径就没有文件可定位，所以这一行**根本不该提供那个动作**
    ///    （而不是提供了却什么也不发生）。**"禁用"怎么画**是 `shell-win` 的事；
    ///    这里只交判据本身。
    ///    判据与 [`TransferReveal::local_path`] 同源（都走 `manifest_path` 那一份归一化），
    ///    所以"能定位"与"算得出路径"不会分叉。
    pub fn can_reveal_in_finder(&self) -> bool {
        self.manifest_path.is_some()
    }

    /// 完成量 / 总量，**夹到 `0.0..=1.0` 且绝不为 NaN**。
    ///
    /// 口径与 `PercentFormat` 逐条对齐：总量 <= 0 → 0（清单还没加载时总量就是 0），
    /// 超报（`completed > total`）夹到 1（任务超报不得画出超过满格的进度条）。
    ///
    /// ⚠️ `total > 0` 由第一句保证 ⇒ 除法不会是 0/0（那才会得到 NaN）。
    ///
    /// ⚠️ **W-6（可见性，任务 14 改动，就改了这一处修饰符）**：上游是
    ///    `static func fraction`（`TransferRow.swift:107`）——**没有 `public`**，
    ///    即"本模块内可见"，而 `ProgressSummary`（同一个 `BenagenCoreKit` 模块）正是
    ///    它的第二个消费方（`VerifySummary.swift:357` 的"分数与传输列表**共用**
    ///    `TransferRow.fraction`"）。Rust 侧的对位是 **`pub(crate)`**：
    ///    同一个 crate 的 `presentation::verify_summary` 能 `use` 到它，
    ///    而 crate 外照旧看不见 —— 可见性与上游逐条对齐，语义一个字节都没动。
    ///    ⚠️ 不许在 `verify_summary.rs` 里抄第二份：抄了就会出现"同一个进度在两处
    ///    不一样"（源里那句记账的原话），而症状是"总进度条与百分比对不上"。
    pub(crate) fn fraction(completed: i64, total: i64) -> f64 {
        if total <= 0 {
            return 0.0;
        }
        // `as f64` 是**饱和转换**、不会 panic；夹过之后两边都远在 f64 的精确整数区内。
        completed.clamp(0, total) as f64 / total as f64
    }

    /// `TaskState` 五档 → 呈现标签。**任何一档都不许是空白**（约束 4）。
    fn label(state: TaskState) -> &'static str {
        match state {
            TaskState::Waiting => "等待中",
            TaskState::Active => "下载中",
            TaskState::Complete => "已完成",
            TaskState::Error => "失败",
            TaskState::Removed => "已移除",
        }
    }

    /// 语义色（规格 §7.2）。**不是具体的颜色值**，由 `shell-win` 映射。
    ///
    /// ⚠️ 五档**两两不同**（与 `label` / `icon_name` 同等要求：两个状态画成一个样子，
    ///    等于其中一个永远不会出现在客户面前）——所以 `Waiting` 的中性灰**不能**也发给
    ///    `Removed`。`Removed` 拿的是规格里那个**警告色**：它是五档里唯一
    ///    **一个动作都不给**的状态（`remove` 已经把它自己的 GID→路径映射 forget 掉了），
    ///    出路在列表级的「清空已完成」——用警告色把它标出来，正好提示"这一行要你在别处处理"。
    ///    钉住它的是 `every_task_state_maps_to_its_own_color`。
    fn color(state: TaskState) -> RowColor {
        match state {
            TaskState::Waiting => RowColor::Secondary,
            TaskState::Active => RowColor::Blue,
            TaskState::Complete => RowColor::Green,
            TaskState::Error => RowColor::Red,
            TaskState::Removed => RowColor::Orange,
        }
    }

    /// 状态 → 行首那颗图标的名字。五档**两两不同**（同 `label`：两个状态画成一样，
    /// 等于其中一个永远不会出现在客户面前）。
    fn icon_name(state: TaskState) -> &'static str {
        match state {
            TaskState::Waiting => "clock",
            TaskState::Active => "arrow.down.circle",
            TaskState::Complete => "checkmark.circle",
            TaskState::Error => "exclamationmark.triangle",
            TaskState::Removed => "trash",
        }
    }

    /// 这一行允许哪些动作。
    ///
    /// ⚠️ 判据是**领域状态 + 是否暂停 + 有没有 GID**，不是"这个动作在内核里存不存在"：
    ///    那一处出现一个按下去只会回错的东西，与约束 4 的"失败必须出现在客户面前"是两回事
    ///    —— 前者是壳明知故犯地把错误送到客户面前。
    ///
    /// ⚠️ **`clear_finished` 不在这里**：它没有 gid、语义是"清空全部已结束的"，
    ///    混进行级动作的后果是用户以为只清这一行、实际清掉一片。它是列表级的动作
    ///    （见 [`TransferGlobalSummary::can_clear_finished`]）。
    fn actions(state: TaskState, is_paused: bool, gid: &str) -> BTreeSet<TaskAction> {
        // 没有 GID 就没有动作可发（内核会回 `invalid_params("动作 … 需要 gid")`）。
        if gid.is_empty() {
            return BTreeSet::new();
        }
        match state {
            // 正在下载：能暂停、能移除；**不能重试**（重试是"停下来之后再来一次"）。
            TaskState::Active => [TaskAction::Pause, TaskAction::Remove].into_iter().collect(),
            // 排队等着的：能暂停、能移除。已暂停的那一条相反 —— 给它「继续」。
            TaskState::Waiting => {
                if is_paused {
                    [TaskAction::Unpause, TaskAction::Remove]
                } else {
                    [TaskAction::Pause, TaskAction::Remove]
                }
                .into_iter()
                .collect()
            }
            // 已停止的两档：重试（重新入队）与移除。
            TaskState::Complete | TaskState::Error => {
                [TaskAction::Retry, TaskAction::Remove].into_iter().collect()
            }
            // ⚠️ 一个都不给。`remove` 会把 GID 的路径映射 `forget` 掉，所以 `Removed`
            //    这一档既没有路径可重试（内核回 `invalid_params("GID … 没有路径映射")`）、
            //    也没有东西可再移除。要清掉它们用列表级的「清空已完成」。
            TaskState::Removed => BTreeSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 列表顶部：全局速度与活动数（`transfer_list` 的 `global`）
// ---------------------------------------------------------------------------

/// `global` → 列表顶部那一行。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct TransferGlobalSummary {
    /// 「1.2 MB/s」；速度为 0 时是 `—`（口径同 `SpeedFormat`）。
    pub speed_text: String,
    /// 「活动 2 · 等待 3 · 已停止 4」。
    pub activity_text: String,
    /// 「清空已完成」那一项能不能用。
    pub can_clear_finished: bool,
}

impl TransferGlobalSummary {
    /// ⚠️ 三个数**一个都不能省**（约束 4）：只显示"活动"的话，一个"排队等着的"
    /// 和一个"已经失败的"在客户面前就分不出来了。
    pub fn of(global: &GlobalStat) -> Self {
        Self {
            speed_text: SpeedFormat::text(global.download_speed),
            activity_text: format!(
                "活动 {} · 等待 {} · 已停止 {}",
                global.num_active, global.num_waiting, global.num_stopped
            ),
            // 没有已结束的任务时**禁用**那一项：点了之后毫无变化的话，
            // 用户分不清"清完了"还是"没生效"（约束 4）。
            can_clear_finished: global.num_stopped > 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 动作失败 → 要说的那句话
// ---------------------------------------------------------------------------

/// 一次 `task_action` 失败之后要显示的那句话。
///
/// ⚠️ **这是给 `shell-win` 用的唯一入口，那边自己不做映射**（全局约束 8：
///    那里不映射 `ClientError`）。映射本身走 [`crate::presentation::error_text::error_text`]
///    —— 那是 `ClientError` → 客户可见文案的**唯一实现**
///    （`DirLoadFailure.of` / `EnqueueFeedback::failure` 用的也是它）。
///    本类型**不抄第二份**：抄了就会出现"同一个错误在传输列表和别处措辞不同"。
///
/// 文案**逐字是内核原文**（约束 3）：动作失败的现场（`invalid_params("GID … 没有路径映射")`
/// 之类）只有原文说得清发生了什么。
///
/// ⚠️ **W-6（错误类型）**：上游收的是 `Error`（任意错误）并转交给 `AppModel.message(of:)`；
///    Rust 侧的对应物只能收 [`ClientError`] —— 本 crate 是强类型的，没有"任意错误"这个类型
///    （同 `error_text.rs` 头部记的那一处形态偏离）。
pub enum TransferActionFailure {}

impl TransferActionFailure {
    /// 一条失败 → 一句话。**逐字**交出去，不加工、不加前缀。
    pub fn message_of(error: &ClientError) -> String {
        error_text(error)
    }
}

// ---------------------------------------------------------------------------
// 顶部常驻横幅
// ---------------------------------------------------------------------------

/// 横幅的种类（上游是 `EngineBanner.Kind`）。
///
/// ⚠️ **W-6（形态）**：Rust 没有"在类型里声明类型"这件事（`impl` 里只能放关联类型），
///    所以上游那个嵌套的 `Kind` 在这里是一个**同级**枚举，名字取 `EngineBannerKind`。
///    两个变体名与上游逐字相同（`engineUnavailable` / `transientError`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum EngineBannerKind {
    /// 引擎/内核不可用（`EngineState::Unavailable`）。
    EngineUnavailable,
    /// 瞬时错误（`last_error`）：下一拍成功就自己消失。
    TransientError,
}

/// 主区顶部那条横幅：「引擎/内核现在有问题」的唯一常驻落点。
///
/// ⚠️ 它是**两个来源**的落点，且优先级是**固定的**：
///   ① `engine == Unavailable` —— 引擎/内核真的不可用（断连、启动失败、握手超时、
///      重启失败），文案是 `engine` 里那句**原文**（约束 3），并且**带「重试」**；
///   ② `last_error` —— 非抛错路径（轮询 / 校验刷新）上"其余错误码"的**非粘滞**出口
///      （任务 3 建立，到本任务才有消费者）。它**不带**「重试」：那一处的语义是
///      "重启内核"，拿它去处理一次可自愈的抖动等于杀掉一个健康的引擎（约束 1）。
///
/// ⚠️ 两者同时有话说时**以 `engine` 为准**：它是当前连接的事实，而 `last_error` 是
///    上一拍的残留（下一次成功请求就清掉）。
///
/// ⚠️ **W-6（形态）**：上游是 `struct EngineBanner` —— 简报的 API 块把它写成 `enum`，
///    那只是"里面有 kind/text/hint/shows_retry"的速记。**以源为准**（裁决 T）：
///    这里是对位上游的结构体 + [`EngineBannerKind`]。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct EngineBanner {
    pub kind: EngineBannerKind,
    /// 给客户看的正文。**内核原文逐字**（约束 3）—— 壳不加工、不加前缀。
    pub text: String,
    /// 壳写的补充说明；只有"内核一个字都没回"的握手超时才有一句
    /// （见 `EngineStatusPresentation::handshake_timeout_tooltip`）。
    pub hint: Option<String>,
    /// 要不要显示那处**真的会生效**的「重试」（`AppModel.retryEngine()` 的对位动作）。
    pub shows_retry: bool,
}

impl EngineBanner {
    /// `engine` 与 `last_error` → 横幅；两处都没话说时 `None`。
    pub fn of(engine: &EngineState, last_error: Option<&str>) -> Option<Self> {
        if let EngineState::Unavailable(why) = engine {
            // ⚠️ 覆盖 `Unavailable` 的**每一个**理由，不只是 `engine_disconnected` /
            //    `engine_start_failed`：任务 4b 的握手超时与重启失败都落在这里，
            //    而超时那句 tooltip 里写着"…然后点「重试」" —— 那一处就是它的落点。
            return Some(Self {
                kind: EngineBannerKind::EngineUnavailable,
                text: why.clone(),
                // 超时是**壳探测到的**状况（内核一个字都没回，没有原文可登），
                // 所以只有这一支补一句"该去点什么"；内核自己给的失败原因不套壳的话。
                hint: if EngineStatusPresentation::is_handshake_timeout(why) {
                    Some(EngineStatusPresentation::handshake_timeout_tooltip().to_string())
                } else {
                    None
                },
                shows_retry: true,
            });
        }
        match last_error {
            Some(text) if !text.is_empty() => Some(Self {
                kind: EngineBannerKind::TransientError,
                text: text.to_string(),
                hint: None,
                shows_retry: false,
            }),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 空态文案
// ---------------------------------------------------------------------------

/// 列表区在没有行可显示时说什么。
///
/// ⚠️ `engine_not_started` **不是错误**（简报）：它只是"还没添加过任务"，
///    所以给的是空态文案，没有红色、没有惊叹号。
pub enum TransferListEmpty {}

impl TransferListEmpty {
    /// 引擎尚未启动（内核的 `engine_not_started`）。**逐字**（简报）。
    pub const ENGINE_NOT_STARTED: &'static str = "下载引擎尚未启动（还没有添加过任务）";
    /// 引擎在跑、但确实没有任务。
    pub const NOTHING_IN_FLIGHT: &'static str = "没有正在传输的任务";

    /// `None` = 这里不说话。
    ///
    /// ⚠️ 引擎不可用时返回 `None`：顶部横幅（常驻、带「重试」）已经在说同一句话，
    ///    两处重复同一句原文只会让真正要看的那条变淡。
    ///
    /// ⚠️ **W-6（返回类型）**：上游是 `String?`，这里给 `Option<&'static str>` ——
    ///    两支都是常量，借用出去比每次造一个 `String` 更贴近"它就是那句话本身"。
    pub fn of(engine: &EngineState) -> Option<&'static str> {
        match engine {
            EngineState::NotStarted => Some(Self::ENGINE_NOT_STARTED),
            EngineState::Running | EngineState::Connecting => Some(Self::NOTHING_IN_FLIGHT),
            EngineState::Unavailable(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 准入闸门（约束 4 / 15）
// ---------------------------------------------------------------------------

/// 「这个引擎状态能不能发请求」——**壳侧**的准入判据。
///
/// ⚠️ 为什么必须有它（约束 4 明禁的形态）：内核卡死时（首次启动卡在系统授权框上的那次
///    `open()` 里），那条请求**永不返回**，而 `CoreClient` 内部是一条 FIFO **串行**队列
///    （约束 15 的单飞语义就靠它）—— 队列被永久堵死，此后每个新请求都排在它后面、
///    永远轮不到。于是用户看到的是一处再也没有回音的入口、或者一句
///    「正在读取传输列表…」永远转下去。**静默挂死**正是本约束要防的那件事。
///
/// `AppModel` 里还有一道同源的闸门（`requireUsableEngine`，任务 5/6/7 建立），
/// 两者是**互补**的：那里的判据只看 `Unavailable`（`Connecting` = 握手还没结论，
/// 请求排在握手后面是正常的，而且握手有 5 秒上界）；这里把 `Connecting` 也关掉，
/// 因为壳此刻能给的替代动作是"等"或"点重试"，而不是发一条可能永远排不到的请求。
pub enum EngineGate {}

impl EngineGate {
    /// 会发请求的壳侧动作能不能用。
    pub fn allows_requests(engine: &EngineState) -> bool {
        match engine {
            EngineState::Running | EngineState::NotStarted => true,
            EngineState::Connecting | EngineState::Unavailable(_) => false,
        }
    }

    /// 动作被禁用时挂上去的那句话。
    ///
    /// 它是**壳自己写的**（同 `AppModel.busyReason` / `DirLoadFailure.notice` 的性质）：
    /// 它描述的是**壳侧动作**（"去点那一处"），内核不知道也没有对应的话可说。
    /// 约束 3 管的是"内核说了什么"，不是"壳对自己的元素怎么指路"。
    pub const UNAVAILABLE_HELP: &'static str = "引擎不可用，请先点顶部的「重试」";
}

// ---------------------------------------------------------------------------
// 轮询节拍
// ---------------------------------------------------------------------------

/// 轮询节拍。**规格 §5.3 定稿 200 ms**（拉、不推）。
///
/// 这个值同时是"壳跟得上"与"不给内核添无谓负载"的平衡点；定时器**只在传输列表
/// 可见时**跑（切走就停）。放在呈现层而不是 `shell-win`，是为了让这个数与
/// "它凭什么这么大"的论证待在一起、并且能被单测钉住。
///
/// ⚠️ **W-6（形态；控制者裁定 JJ 要求单独记账）**：简报的 API 块把 `TransferListPoll`
///    与 [`TransferReveal`] 都写成 `pub struct`，这里两个都是**空 `enum`** ——
///    取的**不是**简报，而是**源 + 本 crate 的既定惯例**：上游两者都是 `enum` +
///    `static` 的命名空间（Rust 没有"只当命名空间用、不可构造"的类型特性，
///    空 `enum` 是最接近的写法，照样不可构造），而本 crate 从 `format.rs` 起就把这条
///    写成了惯例（见 `presentation/mod.rs` 头部第 1 条）。简报那两行是速记，不是规格。
pub enum TransferListPoll {}

impl TransferListPoll {
    pub const INTERVAL_NANOSECONDS: u64 = 200_000_000;
}

// ---------------------------------------------------------------------------
// 「定位到磁盘上的文件」：清单路径 → 落盘路径
// ---------------------------------------------------------------------------

/// 把内核给的**清单相对路径**换算成落盘绝对路径。
///
/// ⚠️ **`TransferItem.path` 不是文件系统路径**，这一条是本节存在的全部理由：
///    - 它是清单里的相对路径（`core/src/engine/daemon.rs` 的 `path_map`，测试夹具
///      `PFX/sub/a.txt` 就是它）；
///    - 内核交给 aria2 时把它**拆成 `dir`/`out`**（`core/src/engine/mod.rs` 的
///      `new_planned_file`），而 aria2 的工作目录就是下载根 —— 所以落盘位置是
///      `下载根 + "/" + 清单路径`；
///    - 于是把清单路径当成文件路径解析出来的是**进程 cwd**（双击启动时是 `/`）
///      下的一个不存在的文件：一处点了没反应的静默失效（约束 4 明禁）。
///
/// ⚠️ 落盘根为什么不从协议里来：协议里**没有**这个字段（`load_delivery` 的树是相对清单），
///    它只有两个来源，都在这层：
///      ① **壳配过下载目录**（阶段 E 任务 3：argv `--download-dir`）⇒ 落盘根就是它；
///      ② 没配过 ⇒ 内核用它自己的默认值 `$HOME/Downloads/Benagen`（`core/src/paths.rs`
///         的 `download_dir_for`），而那一级环境变量的取法与内核**逐字一致**
///         （见 [`TransferReveal::home_variable`]）。
///    所以这里钉的是**两边的约定**：内核改了那个默认值，
///    [`TransferReveal::DOWNLOAD_ROOT_RELATIVE_TO_HOME`] 这个常量要跟着改
///    （`the_download_root_anchor_is_pinned_locally` 守着它）。
///
/// ⚠️ **阶段 E 之前这里只有一个分支**（壳从不传 `--download-dir`），所以落盘根是个常量；
///    现在它由 [`TransferReveal::download_root`] 算 —— 少了这一步，用户配了自定义目录之后
///    那次定位会指向 `~/Downloads/Benagen` 下的一个**不存在**的文件，
///    而那正是约束 4 明禁的静默失效（壳报出一个路径、客户照它去却什么也找不到）。
///
/// ⚠️ **W-6（形态）**：同 [`TransferListPoll`] —— 简报写 `struct`，这里取**源**与本 crate
///    惯例的**空 `enum`**（理由写在那一处的注释里）。
pub enum TransferReveal {}

impl TransferReveal {
    /// 内核默认下载目录相对 home 的那一段。
    pub const DOWNLOAD_ROOT_RELATIVE_TO_HOME: &'static str = "Downloads/Benagen";

    /// 内核给的路径 → 可用的清单路径。
    ///
    /// `None`（内核发 `"path": null`）与**空串**都是"没有映射"：空串呈现出来是一个
    /// 空白行标题，与 `None` 在客户面前的差别只有"看起来像坏了"。
    /// 非空时**逐字**返回（约束 3：不取最后一段、不解码、不折叠）。
    ///
    /// ⚠️ **W-6（返回类型）**：上游是 `String?`（拷一份），这里给 `Option<&str>` ——
    ///    判据（有没有、空不空）逐字照抄，只是省掉一次拷贝。
    pub fn manifest_path(path: Option<&str>) -> Option<&str> {
        match path {
            Some(p) if !p.is_empty() => Some(p),
            _ => None,
        }
    }

    /// **内核取 home 的那一级环境变量名**。
    ///
    /// ⚠️ **W-6（本模块唯一一处新增的 API；简报步骤 1 特别点名的那一条）**：
    ///    上游的 `home(environment:fallback:)` 直接读 `"HOME"` —— 那是 macOS 的正确写法。
    ///    Windows 内核读的是 **`%USERPROFILE%`**（任务 3 把这条判据集中在
    ///    `core/src/paths.rs`），所以壳必须读**同一个名字**：读错一级就与内核分叉，
    ///    拼出来的路径指向一个不存在的文件，而"定位到磁盘上的文件"指到空处正是简报
    ///    点名不要的静默无效。**在 macOS 上这个分叉永远看不出来**（两级在那里结果相同）。
    ///
    /// ⚠️ **判据不在本函数里**（控制者裁定 JJ）：它**委托**给
    ///    [`crate::platform::env_var_name`]——那是 **cfg-free 纯函数**，
    ///    对应内核 `core/src/paths.rs` 的 `env_var_name(purpose, platform)` 里
    ///    **`Purpose::Downloads`** 那一格（`Windows => "USERPROFILE"`、`Other => "HOME"`）。
    ///    本函数只保留**呈现层的这个名字**（与上游对位、也让 `TransferReveal` 的公开面自洽）。
    ///    **本文件里没有 `cfg!`**——crate 里唯一那一处住在 `platform::PLATFORM`。
    ///
    /// ⚠️ 为什么判据必须搬出去："本靶是哪个平台"若内联在这里，`USERPROFILE` 那一支
    ///    **在本机根本不执行**，于是那半条没有任何东西在验（同 `shell-win/src/spawn.rs`
    ///    对 `cfg!(windows)` 的那段论证）。搬进 `platform.rs` 之后，
    ///    `platform::tests::env_var_name_pins_both_branches_on_any_host` 在**任意宿主上**
    ///    把两支都断言了。
    ///
    /// ⚠️ 这不是"壳里抄了内核第二份实现"：`shell-core` **不依赖 `core`**（协议镜像同理，
    ///    见 `protocol.rs` 头部），这条跨 crate 的约定只能**镜像 + 用测试钉住**。
    ///    钉它的是 `the_download_root_uses_the_same_home_source_as_the_kernel`
    ///    （它断言的是"本函数 == `env_var_name(PLATFORM)`"＋两支的名字）与上面那条
    ///    `platform.rs` 的用例。内核改动 `paths.rs` 的取值时，两处要一并改。
    pub fn home_variable() -> &'static str {
        crate::platform::env_var_name(crate::platform::PLATFORM)
    }

    /// 内核解析下载根用的那个 home。
    ///
    /// ⚠️ **必须与内核读的是同一个来源**：内核的默认下载根是 `<home>/Downloads/Benagen`，
    ///    而 `<home>` 取自 [`Self::home_variable`] 那一级环境变量（macOS 侧是
    ///    `core/src/main.rs` 的 `std::env::var_os("HOME")`）。壳起内核时 `ProcessChannel`
    ///    继承环境，所以壳读同一个变量就与内核**逐字一致**。
    ///
    /// ⚠️ **不要用"问系统要登录用户主目录"那条路**（macOS 的
    ///    `FileManager.homeDirectoryForCurrentUser` 走的是 getpwuid，**不看环境变量**）：
    ///    本任务的走查实测撞上过这个分叉 —— `HOME=/tmp/t8home` 时它照样回
    ///    `/Users/starsyi`，而内核把文件下到了 `/tmp/t8home/Downloads/Benagen`；
    ///    照它拼出来的路径是一个**不存在**的文件，而那正是简报点名不要的静默无效。
    ///    两条路在"用户正常双击启动"时结果相同，但**相同是巧合，不是保证**。
    ///
    /// ⚠️ 环境是**参数**（不是在这里调 `std::env::vars()`）：本 crate 不含 OS 调用，
    ///    由 `shell-win` 把当前进程的环境交进来 —— 上游同样把 `ProcessInfo` 的字典当参数收，
    ///    这是同一条分工。
    pub fn home(environment: &BTreeMap<String, String>, fallback: &str) -> String {
        match environment.get(Self::home_variable()) {
            Some(h) if !h.is_empty() => h.clone(),
            _ => fallback.to_string(),
        }
    }

    /// 落盘根：**配过下载目录就是它本身**，没配过才是 `<home>/Downloads/Benagen`。
    ///
    /// ⚠️ `download_dir` 是**用户配的那个目录原文**（`AppModel.downloadDir`，空串 = 未配置）。
    ///    这里**不做任何规范化**（不去 `..`、不展开软链接、不改大小写）：它就是内核
    ///    argv 上那一格（`--download-dir`），壳替它解释一遍只会让"壳以为的路径"与
    ///    "内核实际写的路径"悄悄分叉 —— 而分歧的代价是**定位到别处**
    ///    （同 `AppPreferences` 的 `normalized` 那条纪律）。只去掉末尾多余的 `/`
    ///    （那是**壳自己的**拼接产物，不是内核原文，不受约束 3 管）。
    pub fn download_root(home: &str, download_dir: &str) -> String {
        if !download_dir.is_empty() {
            return Self::trimming_trailing_slashes(download_dir);
        }
        let h = Self::trimming_trailing_slashes(home);
        let prefix = if h.is_empty() || h.ends_with('/') {
            h
        } else {
            h + "/"
        };
        prefix + Self::DOWNLOAD_ROOT_RELATIVE_TO_HOME
    }

    /// 清单路径 → 落盘绝对路径；没有路径时 `None`（调用方据此**禁用**那一项）。
    pub fn local_path(manifest_path: Option<&str>, home: &str, download_dir: &str) -> Option<String> {
        let rel = Self::manifest_path(manifest_path)?;
        // 这里是**壳自己的**路径拼接，不受约束 3 管：末尾多一个 `/` 只是拼出 `//`，
        // 先去干净（清单路径那一段一个字符都不动）。
        let root = Self::download_root(home, download_dir);
        let prefix = if root.is_empty() {
            String::new()
        } else {
            root + "/"
        };
        Some(prefix + rel)
    }

    /// 去掉末尾的 `/`（`/` 本身与空串除外 —— 把它去没了就不是一条路径了）。
    fn trimming_trailing_slashes(path: &str) -> String {
        let mut trimmed = path;
        while trimmed.chars().count() > 1 && trimmed.ends_with('/') {
            trimmed = &trimmed[..trimmed.len() - 1];
        }
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/TransferRowTests.swift`（39 条，逐条对位）。

    use super::*;
    // ⚠️ **控制者裁决 EE**：这句里那个常量**只许 `use`，不许重写第二份字面量** ——
    //    上游那两条测试（`TransferRowTests.swift:269`/`:280`）引用的是
    //    `AppModel.handshakeTimeoutMessage`，本波次不移植 `AppModel`，它的家在
    //    `presentation::engine_status`（`engine_status.rs` 的头注记了这一次裁决）。
    use crate::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;

    /// 上游 `TaskState.allCases`（Swift 的 `CaseIterable`）。
    ///
    /// ⚠️ **W-6**：Rust 侧没有 `CaseIterable` 的对应物，而 `TaskState` 定义在
    /// `protocol.rs`（**不许为了测试往那儿加东西**：它是协议镜像，多一个成员就多一处漂移面）。
    /// 所以这份"五档"清单**只活在测试模块里** —— 它的作用与上游的 `allCases` 相同：
    /// **漏掉一档就没人守**（`every_task_state_has_a_visible_label` 会先撞上
    /// `expected.len() == ALL_STATES.len()` 那条断言）。
    const ALL_STATES: [TaskState; 5] = [
        TaskState::Waiting,
        TaskState::Active,
        TaskState::Complete,
        TaskState::Error,
        TaskState::Removed,
    ];

    /// 上游 `TransferItem.fixture(...)`（`TransferRowTests.swift:452-483`）。
    ///
    /// 与上游同形：**经解码构造**（键是内核线上的 snake_case）、`path` 为 `None` 时发
    /// `"path":null`（不是省略键）。手搓一个 `TransferItem` 结构体会让"壳解不解得动内核
    /// 发来的那一行"这件事在测试里凭空消失，而 `path: null`（不是省略键）正是本任务要处理的形状。
    ///
    /// ⚠️ Rust 没有默认参数，所以默认值走 `Default`，调用点写
    /// `Fixture { state: TaskState::Error, ..Default::default() }` —— 与 Swift 的具名默认参数同形。
    #[derive(Clone)]
    struct Fixture {
        gid: String,
        total: i64,
        completed: i64,
        speed: i64,
        conns: i32,
        state: TaskState,
        raw_status: String,
        error_message: String,
        path: Option<String>,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                gid: "g-1".to_string(),
                total: 1000,
                completed: 250,
                speed: 512,
                conns: 2,
                state: TaskState::Active,
                raw_status: "active".to_string(),
                error_message: String::new(),
                path: Some("a.bin".to_string()),
            }
        }
    }

    impl Fixture {
        fn build(&self) -> TransferItem {
            let mut obj = serde_json::json!({
                "gid": &self.gid,
                "total": self.total,
                "completed": self.completed,
                "speed": self.speed,
                "conns": self.conns,
                "state": serde_json::to_value(self.state).expect("TaskState 一定序列化得出来"),
                "raw_status": &self.raw_status,
                "error_message": &self.error_message,
            });
            obj["path"] = match &self.path {
                Some(p) => serde_json::Value::String(p.clone()),
                None => serde_json::Value::Null,
            };
            serde_json::from_value(obj).expect("夹具必须与内核发来的那一行同形")
        }
    }

    fn row(f: Fixture) -> TransferRow {
        TransferRow::new(&f.build())
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 行：标题 / 角标 / 错误原文
    // -----------------------------------------------------------------------

    /// 上游 `pausedIsRenderedAsABadgeOnADownloadingRow`。
    #[test]
    fn paused_is_rendered_as_a_badge_on_a_downloading_row() {
        // 内核把 aria2 的 paused 与 waiting 都映射成 state == "waiting"，
        // 区分只能靠 raw_status（阶段 A 裁决 #88）
        let it = row(Fixture {
            state: TaskState::Waiting,
            raw_status: "paused".to_string(),
            ..Default::default()
        });
        assert_eq!(it.shows_paused_badge, true);
        let it2 = row(Fixture {
            state: TaskState::Waiting,
            raw_status: "waiting".to_string(),
            ..Default::default()
        });
        assert_eq!(it2.shows_paused_badge, false);
    }

    /// 上游 `nilPathIsNotRenderedAsTheWordNil`。
    #[test]
    fn nil_path_is_not_rendered_as_the_word_nil() {
        // 内核发 "path": null 而不是省略
        let it = row(Fixture {
            path: None,
            ..Default::default()
        });
        assert_eq!(it.title, "（未知路径）");
    }

    /// 上游 `anEmptyPathIsAlsoUnknownRatherThanABlankRow`。
    #[test]
    fn an_empty_path_is_also_unknown_rather_than_a_blank_row() {
        // ⚠️ 夹具让"只挡 nil"的实现活着就是给变异体打掩护：`String?` 为空串时
        //    `path ?? 占位` 会渲染出一个**空白行标题**（约束 4 的静默失效）。
        //    内核为空串的语义同样是"不知道"（同 `raw_status` 取不到时传空串）。
        let it = row(Fixture {
            path: Some(String::new()),
            ..Default::default()
        });
        assert_eq!(it.title, "（未知路径）", "空串标题在呈现侧就是什么都没说");
        assert_eq!(
            it.manifest_path, None,
            "空串与 nil 在呈现层是同一件事：没有可用的路径"
        );
    }

    /// 上游 `aPathIsShownVerbatim`。
    #[test]
    fn a_path_is_shown_verbatim() {
        // 约束 3：清单原文逐字 —— 不取最后一段、不解码 `×`、不折叠空格。
        let raw = "sub dir/C24-8_×_25WS024/QC 图.png";
        let r = row(Fixture {
            path: Some(raw.to_string()),
            ..Default::default()
        });
        assert_eq!(r.title, raw);
        assert_eq!(r.manifest_path.as_deref(), Some(raw));
    }

    /// 上游 `errorRowsShowTheAriaMessageVerbatim`。
    #[test]
    fn error_rows_show_the_aria_message_verbatim() {
        // aria2 的 errorMessage 不得改写、不得截断、不得清空（约束 3）。
        // 换行是刻意的：多行原文要**全文**显示。
        let why = "连接超时（第 3 次重试）\n原因：Connection reset by peer";
        let r = row(Fixture {
            state: TaskState::Error,
            raw_status: "error".to_string(),
            error_message: why.to_string(),
            ..Default::default()
        });
        assert_eq!(r.error_text.as_deref(), Some(why));
        assert_eq!(
            r.state_label,
            row(Fixture {
                state: TaskState::Error,
                ..Default::default()
            })
            .state_label,
            "有没有错误消息不该改变状态那一列"
        );
    }

    /// 上游 `aRowWithoutAnErrorMessageHasNoErrorLine`。
    #[test]
    fn a_row_without_an_error_message_has_no_error_line() {
        // 没有错误消息 ⇒ 不渲染那一行（None ≠ 空串：空串会在行下方留一条看不见的空行）。
        let r = row(Fixture {
            state: TaskState::Active,
            error_message: String::new(),
            ..Default::default()
        });
        assert_eq!(r.error_text, None);
    }

    // -----------------------------------------------------------------------
    // 行：进度
    // -----------------------------------------------------------------------

    /// 上游 `progressFractionIsZeroWhenTotalIsZero`。
    #[test]
    fn progress_fraction_is_zero_when_total_is_zero() {
        // 不得 NaN：进度条控件拿到 NaN 会画出一条毫无意义的进度条。
        let r = row(Fixture {
            total: 0,
            completed: 0,
            ..Default::default()
        });
        assert_eq!(r.progress_fraction, 0.0);
        assert!(!r.progress_fraction.is_nan());
        assert_eq!(
            r.percent_text, "—",
            "总量为 0 时百分比是占位符（口径同 PercentFormat）"
        );
    }

    /// 上游 `progressFractionIsClampedToTheUnitRange`。
    #[test]
    fn progress_fraction_is_clamped_to_the_unit_range() {
        // 超报（completed > total）夹到 1，负值夹到 0 —— 与 `PercentFormat` 同一条口径。
        assert_eq!(
            row(Fixture {
                total: 100,
                completed: 250,
                ..Default::default()
            })
            .progress_fraction,
            1.0
        );
        assert_eq!(
            row(Fixture {
                total: 100,
                completed: -5,
                ..Default::default()
            })
            .progress_fraction,
            0.0
        );
        // 正常值必须**按比例**，不是常数：这一条守着"夹住了就完事"的实现。
        assert_eq!(
            row(Fixture {
                total: 400,
                completed: 100,
                ..Default::default()
            })
            .progress_fraction,
            0.25
        );
        assert_eq!(
            row(Fixture {
                total: 400,
                completed: 100,
                ..Default::default()
            })
            .percent_text,
            "25%"
        );
    }

    /// 上游 `theProgressLineReadsCompletedOverTotal`。
    #[test]
    fn the_progress_line_reads_completed_over_total() {
        // 三列里的「进度」列 = 已完成/总量 + 百分比。
        let r = row(Fixture {
            total: 2048,
            completed: 1024,
            ..Default::default()
        });
        assert_eq!(r.progress_text, "1.0 KB / 2.0 KB");
        assert_eq!(r.percent_text, "50%");
        assert_eq!(r.speed_text, "512 B/s");
    }

    // -----------------------------------------------------------------------
    // 行：状态与动作准入
    // -----------------------------------------------------------------------

    /// 上游 `everyTaskStateHasAVisibleLabel`。
    #[test]
    fn every_task_state_has_a_visible_label() {
        // 五档（含 removed）一个不能少，且两两不同（约束 4：不得静默失效 ——
        // 两个状态画成一样，等于其中一个永远不会出现在客户面前）。图标同理。
        //
        // ⚠️ **只断言"两两不同"是不够的**（同下面 `every_task_state_maps_to_its_own_color` 那段）：
        //    把 `Active`（"下载中"）与 `Complete`（"已完成"）的标签对调、或把 `Waiting`
        //    的 `clock` 与 `Removed` 的 `trash` 对调，"互不相同"照样成立 ——
        //    客户看到的就是"下完的显示下载中、正在下的显示已完成"。所以这里**逐档钉死**
        //    哪一档是哪个词、哪颗图标。
        let expected: [(TaskState, &str, &str); 5] = [
            (TaskState::Waiting, "等待中", "clock"),
            (TaskState::Active, "下载中", "arrow.down.circle"),
            (TaskState::Complete, "已完成", "checkmark.circle"),
            (TaskState::Error, "失败", "exclamationmark.triangle"),
            (TaskState::Removed, "已移除", "trash"),
        ];
        assert_eq!(
            expected.len(),
            ALL_STATES.len(),
            "五档必须一档不漏地钉在这里 —— 漏掉的那档就没人守了"
        );
        for state in ALL_STATES {
            let Some((_, want_label, want_icon)) = expected.iter().find(|(s, _, _)| *s == state)
            else {
                panic!("{state:?} 没有期望值：新加一档就要在上面的表里补一行");
            };
            let r = row(Fixture {
                state,
                ..Default::default()
            });
            assert_eq!(r.state_label, *want_label, "{state:?} 的标签不对");
            assert_eq!(r.state_icon_name, *want_icon, "{state:?} 的图标不对");
        }

        let labels: Vec<String> = ALL_STATES
            .iter()
            .map(|s| {
                row(Fixture {
                    state: *s,
                    ..Default::default()
                })
                .state_label
            })
            .collect();
        assert_eq!(labels.len(), 5);
        assert!(
            labels.iter().all(|l| !l.is_empty()),
            "任何一档都不许是空白：{labels:?}"
        );
        let unique: BTreeSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), 5, "五档必须两两不同：{labels:?}");

        let icons: Vec<String> = ALL_STATES
            .iter()
            .map(|s| {
                row(Fixture {
                    state: *s,
                    ..Default::default()
                })
                .state_icon_name
            })
            .collect();
        assert!(
            icons.iter().all(|i| !i.is_empty()),
            "任何一档都不许没有图标：{icons:?}"
        );
        let unique: BTreeSet<&String> = icons.iter().collect();
        assert_eq!(unique.len(), 5, "五档的图标必须两两不同：{icons:?}");
    }

    /// 上游 `everyTaskStateMapsToItsOwnColor`。
    #[test]
    fn every_task_state_maps_to_its_own_color() {
        // ⚠️ 五档的**颜色**与 label / icon 同等要求：两两不同（约束 4）。
        //    只断言"五个色值互不相等"是不够的（把 error 的红与 complete 的绿对调照样全绿），
        //    所以这里同时把**每一档的具体语义色**钉住。
        let expected: [(TaskState, RowColor); 5] = [
            (TaskState::Waiting, RowColor::Secondary), // 中性：还没轮到
            (TaskState::Active, RowColor::Blue),       // 进行中
            (TaskState::Complete, RowColor::Green),    // 成功
            (TaskState::Error, RowColor::Red),         // 错误
            // 警告：这一行没有任何可用动作，出路在列表级「清空已完成」
            (TaskState::Removed, RowColor::Orange),
        ];
        for (state, color) in expected {
            assert_eq!(
                row(Fixture {
                    state,
                    ..Default::default()
                })
                .state_color,
                color,
                "{state:?} 的语义色不对"
            );
        }
        // ⚠️ `RowColor` **故意不派生 `Ord`**（它是呈现层的语义枚举，不是排序键），
        //    所以这里用两两比对而不是 `BTreeSet` —— 判据与上游 `Set(colors).count == 5` 等价。
        let colors: Vec<RowColor> = ALL_STATES
            .iter()
            .map(|s| {
                row(Fixture {
                    state: *s,
                    ..Default::default()
                })
                .state_color
            })
            .collect();
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                assert_eq!(i == j, a == b, "五档的颜色必须两两不同：{colors:?}");
            }
        }
    }

    /// 上游 `actionAvailabilityFollowsTheState`。
    #[test]
    fn action_availability_follows_the_state() {
        // pause 只对 active 可用；unpause 只对 paused 可用；retry/remove 对已停止的可用
        assert!(row(Fixture {
            state: TaskState::Active,
            ..Default::default()
        })
        .available_actions
        .contains(&TaskAction::Pause));
        assert!(!row(Fixture {
            state: TaskState::Complete,
            ..Default::default()
        })
        .available_actions
        .contains(&TaskAction::Pause));
        assert!(row(Fixture {
            state: TaskState::Error,
            ..Default::default()
        })
        .available_actions
        .contains(&TaskAction::Retry));
    }

    /// 上游 `thePausedRowOffersUnpauseInsteadOfPause`。
    #[test]
    fn the_paused_row_offers_unpause_instead_of_pause() {
        // 「已暂停」那一行要能继续（`unpause`），而不是再给一处按了没用的「暂停」。
        let paused = row(Fixture {
            state: TaskState::Waiting,
            raw_status: "paused".to_string(),
            ..Default::default()
        });
        assert!(paused.available_actions.contains(&TaskAction::Unpause));
        assert!(!paused.available_actions.contains(&TaskAction::Pause));

        // 排队等待（没暂停）的那一行相反：能暂停，不能继续。
        let queued = row(Fixture {
            state: TaskState::Waiting,
            raw_status: "waiting".to_string(),
            ..Default::default()
        });
        assert!(queued.available_actions.contains(&TaskAction::Pause));
        assert!(!queued.available_actions.contains(&TaskAction::Unpause));
    }

    /// 上游 `finishedRowsOfferRetryAndRemoveButNotPause`。
    #[test]
    fn finished_rows_offer_retry_and_remove_but_not_pause() {
        for state in [TaskState::Complete, TaskState::Error] {
            let r = row(Fixture {
                state,
                ..Default::default()
            });
            assert!(
                r.available_actions.contains(&TaskAction::Retry),
                "{state:?} 上不能重试"
            );
            assert!(
                r.available_actions.contains(&TaskAction::Remove),
                "{state:?} 上不能移除"
            );
            assert!(
                !r.available_actions.contains(&TaskAction::Pause),
                "{state:?} 上不该有暂停"
            );
            assert!(
                !r.available_actions.contains(&TaskAction::Unpause),
                "{state:?} 上不该有继续"
            );
        }
    }

    /// 上游 `aRemovedRowOffersNothingAtAll`。
    #[test]
    fn a_removed_row_offers_nothing_at_all() {
        // `removed` 的 GID 在内核里已经**没有路径映射**（`remove` 会 forget 掉），
        // 于是 retry 必然回 `invalid_params("GID … 没有路径映射")`、remove 也已经无事可做。
        // 给一处注定报错的入口比没有入口更糟（约束 4：宁可什么都不给，也不给假的）。
        assert!(row(Fixture {
            state: TaskState::Removed,
            ..Default::default()
        })
        .available_actions
        .is_empty());
    }

    /// 上游 `noRowOffersTheListWideClearFinishedAction`。
    #[test]
    fn no_row_offers_the_list_wide_clear_finished_action() {
        // `clear_finished` **不是**行级动作（它没有 gid，语义是"清空全部已结束的"）——
        // 它混进行级动作里的后果是用户以为只清这一行，实际清掉一片。
        for state in ALL_STATES {
            let r = row(Fixture {
                state,
                ..Default::default()
            });
            assert!(
                !r.available_actions.contains(&TaskAction::ClearFinished),
                "{state:?} 行上不该出现清空"
            );
        }
    }

    /// 上游 `aRowWithoutAGidOffersNoActions`。
    #[test]
    fn a_row_without_a_gid_offers_no_actions() {
        // 没有 GID 就没有动作可发（内核会回 `invalid_params("动作 … 需要 gid")`）——
        // 让那一处可用等于把一条注定报错的请求发出去。
        assert!(row(Fixture {
            gid: String::new(),
            state: TaskState::Active,
            ..Default::default()
        })
        .available_actions
        .is_empty());
    }

    /// 上游 `activeRowsCanBeRemovedButNotRetried`。
    #[test]
    fn active_rows_can_be_removed_but_not_retried() {
        // 正在下载的那一行：能暂停、能移除，但**不能重试**（重试是"停止之后再来一次"）。
        let r = row(Fixture {
            state: TaskState::Active,
            ..Default::default()
        });
        assert!(r.available_actions.contains(&TaskAction::Remove));
        assert!(!r.available_actions.contains(&TaskAction::Retry));
    }

    // -----------------------------------------------------------------------
    // 动作失败 → 要说的那句话
    // -----------------------------------------------------------------------

    /// 上游 `actionFailuresShowTheKernelMessageVerbatim`。
    ///
    /// ⚠️ **W-6（错误类型）**：上游比较的两种错误是 `CoreError.rpc` 与 `CoreError.transport`，
    ///    Rust 侧的对应物是 [`ClientError::Kernel`] 与 [`ClientError::KernelGone`]
    ///    （`client.rs` 的 `Display` 给的是同一句话「内核进程已退出（管道结束）」——
    ///    这不是巧合，它正是 `CoreError.transport` 那句话在 Rust 侧的家）。
    #[test]
    fn action_failures_show_the_kernel_message_verbatim() {
        // 动作失败的落点是列表上方那条提示：**内核原文逐字**（约束 3），一个字都不加工。
        // 走的是与 `DirLoadFailure` / `EnqueueFeedback` 同一个映射，不抄第二份。
        let rpc = ClientError::Kernel {
            code: "invalid_params".to_string(),
            message: "GID g1 没有路径映射（可能已被移除）".to_string(),
        };
        assert_eq!(
            TransferActionFailure::message_of(&rpc),
            "GID g1 没有路径映射（可能已被移除）"
        );
        assert_eq!(
            TransferActionFailure::message_of(&ClientError::KernelGone),
            "内核进程已退出（管道结束）"
        );
        // 结构化 `code` **不进文案**（契约 §5.1：壳按 code 分支，给客户看的是 message）。
        assert!(!TransferActionFailure::message_of(&rpc).contains("invalid_params"));
    }

    // -----------------------------------------------------------------------
    // 列表顶部：全局速度与活动数
    // -----------------------------------------------------------------------

    /// 上游 `theGlobalSummaryShowsSpeedAndActivity`。
    #[test]
    fn the_global_summary_shows_speed_and_activity() {
        // 四个数刻意取**互不相同**的值：字段串位（active ↔ waiting ↔ stopped）
        // 必须被这条抓住 —— 同类项恰好相等的夹具是给变异体打掩护。
        let g = GlobalStat {
            download_speed: 900,
            num_active: 2,
            num_waiting: 3,
            num_stopped: 4,
        };
        let s = TransferGlobalSummary::of(&g);
        assert_eq!(s.speed_text, "900 B/s");
        assert!(
            s.activity_text.contains('2')
                && s.activity_text.contains('3')
                && s.activity_text.contains('4')
        );
        assert_eq!(s.activity_text, "活动 2 · 等待 3 · 已停止 4");
    }

    /// 上游 `aZeroSpeedIsAPlaceholderNotZeroBytesPerSecond`。
    #[test]
    fn a_zero_speed_is_a_placeholder_not_zero_bytes_per_second() {
        // 口径同 `SpeedFormat`：速度为 0 时是"不知道/已停"，不是"每秒零字节"。
        assert_eq!(
            TransferGlobalSummary::of(&GlobalStat {
                download_speed: 0,
                num_active: 0,
                num_waiting: 0,
                num_stopped: 0,
            })
            .speed_text,
            "—"
        );
    }

    /// 上游 `clearFinishedIsOfferedOnlyWhenSomethingHasStopped`。
    #[test]
    fn clear_finished_is_offered_only_when_something_has_stopped() {
        // 没有已结束的任务时那一处**禁用** —— 否则用户点了之后毫无变化，
        // 分不清"清完了"还是"没生效"（约束 4）。
        assert_eq!(
            TransferGlobalSummary::of(&GlobalStat {
                download_speed: 0,
                num_active: 1,
                num_waiting: 0,
                num_stopped: 0,
            })
            .can_clear_finished,
            false
        );
        assert_eq!(
            TransferGlobalSummary::of(&GlobalStat {
                download_speed: 0,
                num_active: 1,
                num_waiting: 0,
                num_stopped: 1,
            })
            .can_clear_finished,
            true
        );
    }

    // -----------------------------------------------------------------------
    // 顶部横幅（引擎不可用 / 瞬时错误）
    // -----------------------------------------------------------------------

    /// 上游 `everyUnavailableEngineGetsABannerWithTheReasonVerbatim`。
    #[test]
    fn every_unavailable_engine_gets_a_banner_with_the_reason_verbatim() {
        // 横幅必须覆盖 `Unavailable` 的**每一个**理由，不只是
        // `engine_disconnected` / `engine_start_failed`：握手超时、重启失败
        // 都落在 `Unavailable` 上。文案用 `engine` 里那句原文（约束 3）。
        for why in [
            "下载引擎已断开：内核进程已退出（管道结束）",
            "引擎启动失败：aria2c 没有在 30 秒内就绪",
            "内核重启失败：找不到内核可执行文件 benagen-core",
            // ⚠️ 裁决 EE：这个常量**是 `use` 进来的**，不是在本文件里又写了一遍字面量。
            HANDSHAKE_TIMEOUT_MESSAGE,
        ] {
            let banner = EngineBanner::of(&EngineState::Unavailable(why.to_string()), None);
            assert_eq!(
                banner.as_ref().map(|b| b.text.as_str()),
                Some(why),
                "横幅文案必须是内核那句原文，不是壳编的：{why}"
            );
            assert_eq!(
                banner.as_ref().map(|b| b.shows_retry),
                Some(true),
                "引擎不可用时必须有「重试」：{why}"
            );
        }
    }

    /// 上游 `theBannerCoversTheHandshakeTimeoutWithItsActionableHint`。
    #[test]
    fn the_banner_covers_the_handshake_timeout_with_its_actionable_hint() {
        // 超时那句提示里写着"…然后点「重试」"—— 本任务就是那一处的落点。
        // 横幅上除了原文（是什么），还要有那句"该怎么办"（壳写的）。
        let banner = EngineBanner::of(
            &EngineState::Unavailable(HANDSHAKE_TIMEOUT_MESSAGE.to_string()),
            None,
        )
        .expect("不可用的引擎必须给出横幅");
        assert_eq!(banner.text, HANDSHAKE_TIMEOUT_MESSAGE);
        assert_eq!(
            banner.hint.as_deref(),
            Some(EngineStatusPresentation::handshake_timeout_tooltip())
        );
    }

    /// 上游 `aKernelReportedFailureGetsNoShellWrittenHint`。
    #[test]
    fn a_kernel_reported_failure_gets_no_shell_written_hint() {
        // 内核自己给的失败原因**不套壳的指导语**：那句话是给"内核一个字都没回"的现场写的，
        // 挂在一条内核已经说清楚原因的失败上就是壳在替内核解释（约束 3）。
        let banner = EngineBanner::of(
            &EngineState::Unavailable("引擎启动失败：aria2c 没有在 30 秒内就绪".to_string()),
            None,
        );
        assert_eq!(banner.and_then(|b| b.hint), None);
    }

    /// 上游 `theRetryButtonIsOfferedOnlyWhenTheEngineItselfIsDown`。
    #[test]
    fn the_retry_button_is_offered_only_when_the_engine_itself_is_down() {
        // 瞬时错误（`engine_rpc_failed` / 一行垃圾）**不该**给「重试」：那一处的语义是
        // "重启内核"，拿它去处理一次可自愈的抖动等于杀掉一个健康的引擎（约束 1）。
        let transient = EngineBanner::of(
            &EngineState::Running,
            Some("下载引擎 RPC 失败：connection reset"),
        )
        .expect("瞬时错误必须给出横幅");
        assert_eq!(transient.text, "下载引擎 RPC 失败：connection reset");
        assert_eq!(transient.shows_retry, false);
        assert_eq!(transient.hint, None);
    }

    /// 上游 `aUsableEngineWithoutAnErrorHasNoBanner`。
    #[test]
    fn a_usable_engine_without_an_error_has_no_banner() {
        // 没有要说的就不挂一条横幅（空横幅会把"一切正常"也变成一条要读的东西）。
        assert_eq!(EngineBanner::of(&EngineState::Running, None), None);
        assert_eq!(EngineBanner::of(&EngineState::NotStarted, None), None);
        assert_eq!(EngineBanner::of(&EngineState::Connecting, None), None);
    }

    /// 上游 `theEngineReasonWinsOverAStaleTransientError`。
    #[test]
    fn the_engine_reason_wins_over_a_stale_transient_error() {
        // 两个来源同时有话说时，**以 `engine` 为准**：它是当前连接的事实，
        // 而 `last_error` 是"上一拍"的残留（非粘滞出口，下一次成功就清）。
        let banner = EngineBanner::of(
            &EngineState::Unavailable("下载引擎已断开：内核进程已退出（管道结束）".to_string()),
            Some("上一拍的瞬时错误"),
        )
        .expect("不可用的引擎必须给出横幅");
        assert_eq!(banner.text, "下载引擎已断开：内核进程已退出（管道结束）");
        assert_eq!(banner.shows_retry, true);
    }

    // -----------------------------------------------------------------------
    // 空态文案（engine_not_started **不是错误**）
    // -----------------------------------------------------------------------

    /// 上游 `engineNotStartedIsPresentedAsAnEmptyStateNotAnError`。
    #[test]
    fn engine_not_started_is_presented_as_an_empty_state_not_an_error() {
        // `engine_not_started` → 「下载引擎尚未启动（还没有添加过任务）」，不显示为错误。
        let text = TransferListEmpty::of(&EngineState::NotStarted);
        assert_eq!(text, Some("下载引擎尚未启动（还没有添加过任务）"));
    }

    /// 上游 `anEmptyRunningListSaysNothingIsInFlight`。
    #[test]
    fn an_empty_running_list_says_nothing_is_in_flight() {
        // 引擎在跑、列表为空 ⇒ 是"没有任务"，不是"引擎没起来"。
        assert_eq!(
            TransferListEmpty::of(&EngineState::Running),
            Some("没有正在传输的任务")
        );
    }

    /// 上游 `anUnavailableEngineLeavesTheListAreaToTheBanner`。
    #[test]
    fn an_unavailable_engine_leaves_the_list_area_to_the_banner() {
        // 引擎不可用时**不在这里再说一遍**：顶部横幅（常驻、带「重试」）已经说了同一句话，
        // 两处重复同一句原文只会让真正要看的那条变淡。
        assert_eq!(
            TransferListEmpty::of(&EngineState::Unavailable(
                "下载引擎已断开：内核进程已退出（管道结束）".to_string()
            )),
            None
        );
    }

    // -----------------------------------------------------------------------
    // 准入闸门（约束 4 / 15）
    // -----------------------------------------------------------------------

    /// 上游 `theRequestGateIsOpenOnlyForRunningAndNotStarted`。
    #[test]
    fn the_request_gate_is_open_only_for_running_and_not_started() {
        // 内核卡死时 FIFO 队列被那条永不返回的请求永久堵死，此后每个新请求都永远挂着 ——
        // 呈现侧会停在「正在读取传输列表…」再也不动（约束 4 明禁的静默挂死）。
        assert!(EngineGate::allows_requests(&EngineState::Running));
        assert!(EngineGate::allows_requests(&EngineState::NotStarted));
        assert!(!EngineGate::allows_requests(&EngineState::Connecting));
        assert!(!EngineGate::allows_requests(&EngineState::Unavailable(
            "下载引擎已断开：内核进程已退出（管道结束）".to_string()
        )));
        assert!(!EngineGate::allows_requests(&EngineState::Unavailable(
            HANDSHAKE_TIMEOUT_MESSAGE.to_string()
        )));
        // 提示语是壳写的**动作说明**（同 `DirLoadFailure.notice` 的性质），不是内核原文。
        assert!(
            EngineGate::UNAVAILABLE_HELP.contains("重试"),
            "提示必须指向那处真的存在的动作"
        );
    }

    // -----------------------------------------------------------------------
    // 轮询节拍与「定位到磁盘上的文件」
    // -----------------------------------------------------------------------

    /// 上游 `thePollingCadenceMatchesTheSpec`。
    #[test]
    fn the_polling_cadence_matches_the_spec() {
        // 规格 §5.3 定稿 200 ms。这个数同时是"壳跟得上"与"不给内核添无谓负载"的平衡点，
        // 改动必须是有意的。
        assert_eq!(TransferListPoll::INTERVAL_NANOSECONDS, 200_000_000);
    }

    /// 上游 `theRevealPathIsJoinedOntoTheDownloadRoot`。
    #[test]
    fn the_reveal_path_is_joined_onto_the_download_root() {
        // ⚠️ **`TransferItem.path` 是清单相对路径，不是落盘路径**：内核把它交给 aria2 时
        //    拆成了 `dir`/`out`（`core/src/engine/mod.rs` 的 `new_planned_file`），落盘位置是
        //    `下载根 + "/" + 清单路径`。直接把相对路径当文件路径交出去，
        //    解析出来的是进程 cwd（双击启动时是 `/`）下的一个不存在的文件
        //    —— 一处点了没反应的失效。
        assert_eq!(
            TransferReveal::local_path(Some("sub dir/QC 图.png"), "/Users/x", "").as_deref(),
            Some("/Users/x/Downloads/Benagen/sub dir/QC 图.png")
        );
        assert_eq!(
            TransferReveal::local_path(Some("a.bin"), "/Users/x", "").as_deref(),
            Some("/Users/x/Downloads/Benagen/a.bin")
        );
        // 根末尾的 `/` 不产生双斜杠（这是**壳自己的**路径，不是内核原文，不受约束 3 管）。
        assert_eq!(
            TransferReveal::local_path(Some("a.bin"), "/Users/x/", "").as_deref(),
            Some("/Users/x/Downloads/Benagen/a.bin")
        );
    }

    /// 上游 `aConfiguredDownloadDirectoryMovesTheRevealRoot`。
    #[test]
    fn a_configured_download_directory_moves_the_reveal_root() {
        // ⚠️ 阶段 E 的任务 3 让壳**可以**指定下载目录了（argv `--download-dir`）。
        //    配过之后落盘根**就是它**，而不是 `<home>/Downloads/Benagen` ——
        //    少了这一条，那次「定位到磁盘上的文件」会指向一个不存在的文件，正是约束 4 明禁的
        //    静默失效（用户改了目录之后，这一处是**唯一**会被悄悄弄坏的东西）。
        assert_eq!(
            TransferReveal::local_path(Some("a.bin"), "/Users/x", "/Volumes/Data/交付").as_deref(),
            Some("/Volumes/Data/交付/a.bin")
        );
        assert_eq!(
            TransferReveal::local_path(Some("a.bin"), "/Users/x", "/Volumes/Data/交付/")
                .as_deref(),
            Some("/Volumes/Data/交付/a.bin"),
            "末尾的 `/` 同样不产生双斜杠"
        );
        // 与内核默认值**逐字不同**的两侧都要看得见（这条用例的判别力就在这里）。
        assert_ne!(
            TransferReveal::local_path(Some("a.bin"), "/Users/x", ""),
            TransferReveal::local_path(Some("a.bin"), "/Users/x", "/Volumes/Data/交付")
        );
    }

    /// 上游 `theDownloadRootIsTheConfiguredDirectoryOrTheKernelsDefault`。
    #[test]
    fn the_download_root_is_the_configured_directory_or_the_kernels_default() {
        // 判据本身（两个分支各一条），以及"未配置"与"配了一个正好等于默认值的目录"
        // 在这层是**同一件事** —— 区分它们是 argv 那一侧的事（E-5：未配置 ⇒ 不传）。
        assert_eq!(
            TransferReveal::download_root("/Users/x", ""),
            "/Users/x/Downloads/Benagen"
        );
        assert_eq!(
            TransferReveal::download_root("/Users/x/", ""),
            "/Users/x/Downloads/Benagen",
            "home 末尾的 `/` 不产生双斜杠"
        );
        assert_eq!(
            TransferReveal::download_root("/Users/x", "/Volumes/Data/交付"),
            "/Volumes/Data/交付"
        );
        assert_eq!(
            TransferReveal::download_root("/Users/x", "/Volumes/Data/交付/"),
            "/Volumes/Data/交付"
        );
    }

    /// 上游 `aPathlessRowCannotBeRevealed`。
    #[test]
    fn a_pathless_row_cannot_be_revealed() {
        // path == nil 时那一处**禁用**，不要静默无效。
        let r = row(Fixture {
            path: None,
            ..Default::default()
        });
        assert_eq!(r.can_reveal_in_finder(), false);
        assert_eq!(
            TransferReveal::local_path(r.manifest_path.as_deref(), "/Users/x", ""),
            None
        );
        assert_eq!(
            row(Fixture {
                path: Some(String::new()),
                ..Default::default()
            })
            .can_reveal_in_finder(),
            false
        );
        assert_eq!(TransferReveal::local_path(Some(""), "/Users/x", ""), None);
        assert_eq!(
            row(Fixture {
                path: Some("a.bin".to_string()),
                ..Default::default()
            })
            .can_reveal_in_finder(),
            true
        );
    }

    /// 上游 `theDownloadRootUsesTheSameHomeSourceAsTheKernel`。
    #[test]
    fn the_download_root_uses_the_same_home_source_as_the_kernel() {
        // 内核读的是**环境变量里的一级**（macOS 侧是 `core/src/main.rs` 的
        // `std::env::var_os("HOME")`），壳必须读同一个来源：走查实测
        // `FileManager.homeDirectoryForCurrentUser` 走的是 getpwuid —— `HOME=/tmp/t8home` 时
        // 它照样回 `/Users/starsyi`，而内核把文件下到了 `/tmp/t8home/Downloads/Benagen`。
        // 照它拼出来的路径指向一个不存在的文件，而"定位到磁盘上的文件"指到空处
        // 正是简报点名不要的静默无效。
        //
        // ⚠️ 夹具用的是**内核那一级**的变量名（而不是写死 `HOME`）—— 这样这条用例在
        //    Windows 上照样成立（那边是 `USERPROFILE`），见下面那两条。
        let key = TransferReveal::home_variable();
        assert_eq!(
            TransferReveal::home(&env(&[(key, "/tmp/t8home")]), "/Users/x"),
            "/tmp/t8home"
        );
        // 变量缺失/为空时回落（双击启动时它总是有；这条只为不让回落变成空串）。
        assert_eq!(TransferReveal::home(&env(&[]), "/Users/x"), "/Users/x");
        assert_eq!(
            TransferReveal::home(&env(&[(key, "")]), "/Users/x"),
            "/Users/x"
        );

        // ⚠️ **Windows 侧的对应用例**（简报步骤 1 特别点名的那一条）：内核在 Windows 上
        //    读的是 `%USERPROFILE%`（任务 3 的 `core/src/paths.rs`），**不是 `HOME`** ——
        //    壳读错一级，拼出来的就是一个不存在的路径，而这一点在 macOS 上永远看不出来。
        //
        // ⚠️ **W-6（为什么不单开第 40 条测试；控制者裁定 JJ 要求记账）**：简报说"这条在
        //    Windows 上要**新增对应用例**"，而这里把它**折进了本用例**，没有在本文件新增一条。
        //    理由：这两条断言与本用例断的是**同一件事**（"壳取的是内核那一级 home 源"），
        //    拆开只会让同一处判据有两个家；而 Windows 那一支**真正的判别力本来就不该由
        //    "多一条测试"来承担**——它落在 `platform.rs` 的
        //    `env_var_name_pins_both_branches_on_any_host` 上（那份判据是 **cfg-free** 的，
        //    两支在**任何宿主上**都断言；在本文件再抄一遍只会多出第三份）。
        //    本用例负责钉**另一半**：`home_variable()` 确实**委托**给了
        //    `platform::env_var_name(platform::PLATFORM)`，而不是自己另写一个判断。
        //    **代价如实记账**：`home_variable()` 若被改成硬编码 `"HOME"`，在 macOS 上
        //    **本用例不会红**（本靶期望的就是 `HOME`）—— 这条盲区是"单宿主"本身的极限，
        //    只有 Windows 宿主能盖住它（任务 23 的真机验收）。
        assert_eq!(
            key,
            crate::platform::env_var_name(crate::platform::PLATFORM),
            "home_variable() 必须是 platform 那条判据的转发，不是这里另写的一份"
        );
        // 两级都给时，取的是**实现读的那一级**。
        //
        // ⚠️ 这条**刻意写成 cfg-free**：判据是"取出来的值 == 那一级键下的值"，
        //    而两级的值**刻意不同**（`/unix-home` vs `/win-home`）⇒ 硬编码另一级的实现
        //    **在任何宿主上都红**。写成 `if cfg!(windows) { "/win-home" } else { … }`
        //    反而把"本靶是哪个平台"这件事又抄了一遍（裁定 JJ 要的正是别再有第二处）。
        let both = env(&[("HOME", "/unix-home"), ("USERPROFILE", "/win-home")]);
        assert_eq!(
            TransferReveal::home(&both, "/fallback"),
            both[key].clone(),
            "壳必须读内核那一级（本靶读的是 {key}），不是另一个平台的"
        );
    }

    /// 上游 `theDownloadRootAnchorIsPinnedLocally`。
    #[test]
    fn the_download_root_anchor_is_pinned_locally() {
        // ⚠️ **这不是"内核默认值"的判据，别把它读成那个**（名字原来是
        //    `theDownloadRootMatchesTheKernelsDefault`，那个名字给了过强的暗示，已改）：
        //    它断言的是**壳里这个本地锚点**没有被无声改掉。
        //
        // 壳不传 `--download-dir`（计划里的业务决定），所以内核用的是它自己的默认值
        // `<home>/Downloads/Benagen`。
        // **内核改了那个默认值，这一条不会红** —— 本项目的测试不许依赖 `core/` 的仓库布局
        // （约束 10 也不许改它），所以两边的一致性只能**人工交叉核对**：
        // 改内核默认值、或让壳显式传 `--download-dir` 时，请一并读 `core/src/paths.rs` 的
        // `download_dir_for` 并更新这个常量。
        assert_eq!(
            TransferReveal::DOWNLOAD_ROOT_RELATIVE_TO_HOME,
            "Downloads/Benagen"
        );
    }
}
