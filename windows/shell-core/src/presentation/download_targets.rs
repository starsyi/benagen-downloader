//! download_targets —— 「下载」这个动作的呈现模型。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/DownloadTargets.swift`（逐字对位）。
//!
//! 全局约束 8：一切"把动作 / 内核数据变成可显示值"的纯计算都放 `Presentation/`，
//! **每条都要有单测**。本文件装的是同一个动作的两组纯计算：
//!
//!   ① [`DownloadTargets`] —— 点下去该发什么（勾选面 → `paths`、动作的文案）；
//!   ② [`EnqueueFeedback`] —— 内核回了什么、该怎么说（`added` / `rejected` 两边都要有落点）。
//!
//! ⚠️ **本模块不展开目录**（约束 1）：内核的 `resolve_targets` 把每个 path 当**前缀**处理
//!    （`f.path == p || f.path.starts_with("{p}/")`），所以目录路径**原样**交给它就行。
//!    壳自己展开会把"哪些文件属于这个目录"这条语义复制到壳里，而且两边的边界条件
//!    （空目录、路径末尾的 `/`）会漂移。
//!
//! ⚠️ **集合类型是 `BTreeSet`**（上游是 `Set<String>`）：判据是**集合相等**，与遍历顺序无关；
//!    而 `BTreeSet` 顺带按 `Ord` 有序，于是"每次请求的 `paths` 顺序都一样"这件事在 Rust 侧
//!    是**结构上成立**的（上游要靠一次显式 `sorted()`，因为 Swift 的 `Set` 遍历顺序不稳定）。
//!    这条差异对本模块是**加强**而不是偏离，具体写在 [`DownloadTargets::paths`] 的文档里。
//!
//! ⚠️ 散文纪律（`lib.rs` 的章程 + 本任务纪律 3）：本 crate **不含界面概念**，
//!    所以本文件的注释说的是「动作的文案」「那一段文字」——一个界面名词都不写。
//!    `button_title` 这个**标识符**例外：它按上游的名字逐字保留（见它的文档）。

use serde::Serialize;
use crate::client::ClientError;
use crate::presentation::error_text::error_text;
use crate::protocol::EnqueueResult;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// 勾选面 → 请求
// ---------------------------------------------------------------------------

/// 勾选面 → `enqueue` 的 `paths`，以及这个动作怎么呈现。
///
/// 上游 `DownloadTargets.swift:24-88`。Swift 那边是 `enum` + `static func` 的命名空间用法；
/// 这里用**空 `enum` + 固有 `impl`** 对位（空 `enum` 同样不可构造，语义一致）。
pub enum DownloadTargets {
    // 空：本类型只当命名空间用，一个实例都不该有（同 `format.rs` 的三个 *Format）。
}

impl DownloadTargets {
    /// 安全预算：内核上限（8 MiB）的一半。
    ///
    /// ⚠️ 留一半的余量给**信封**（`id` / `method`）与任何我们没算进去的部分。
    ///    这个守卫宁可早一点拒绝，也不能放过一条会**永久堵死客户端**的请求。
    ///
    /// ⚠️ **取值是被决定过的**（上游 `DownloadTargets.swift:65` 的 `4 << 20`），不是从别处
    ///    推出来的：它对应契约 C-3 那次"交付码没有长度上限"的同族防御。内核那一侧的额度是
    ///    [`crate::client::MAX_REQUEST_LINE_BYTES`]（8 MiB，`core/src/main.rs:113`）。
    ///    改这个数字就得有人重新论证一次——钉住它的是一条测试
    ///    （`the_budget_keeps_half_the_kernel_limit`）。
    pub const REQUEST_BUDGET_BYTES: usize = 4 << 20;

    /// 勾选面 → `enqueue` 的 `paths`。**整批全选是一个特例**。
    ///
    /// ⚠️ **整批全选必须发空数组，这不是优化，是正确性**（全局约束 C-3）：
    ///    `enqueue.paths` 的请求体随勾选面线性增长，而内核的行长上限是 8 MiB
    ///    （`core/src/main.rs:113`）。超长行的后果**不是报错**：内核只回一条 `id == 0`
    ///    的协议告警、那条请求**永远等不到响应**、[`crate::client::CoreClient`] 的串行
    ///    请求-响应循环**被永久堵死**，而用户一个字都看不到。
    ///    整批全选发 `[]` 是**语义等价**的：内核的 `paths` 为空 = 全部**待下载**
    ///    （`view::pending_paths`），而且请求体**恒定**（不随批次大小增长）。
    ///
    /// ⚠️ 判据是 `selection == all_paths`（**文件**路径的全集，来自 `get_tree` 的 `flat`）。
    ///    勾选面里可能还有目录，但那只会让两边**不相等** ⇒ 走显式列表，也就是保守的一边。
    ///    `all_paths` 为空时**不算**整批 —— 否则"一批里一个文件都没有"会被当成"全选"。
    ///
    /// ⚠️ **没有旧签名的重载**（裁定 C2）：一个"传没传整批都能编过"的重载，
    ///    正好能让上面这条硬要求被悄悄绕过去。
    ///
    /// **空集合 → 空数组**：内核的语义是「`paths` 为空 = 下全部待下载」，
    /// 所以这里不能"什么都不发"，只能发一个空数组。
    ///
    /// ⚠️ 上游在这里要显式 `selection.sorted()`，因为 Swift 的 `Set` 遍历顺序**不稳定**
    ///    （不排序会让每一次请求的 `paths` 顺序都不同，排障时没法比对两次请求）。
    ///    Rust 侧的入参是 `BTreeSet`，`iter()` 本身就是**按 `Ord` 升序**的，所以这里
    ///    **不需要**再排一次——顺序确定这件事在这个类型里是结构性的。
    ///    口径与上游一致：用语言的默认序（Rust 的 `String` 按 UTF-8 字节序，对 ASCII 与
    ///    码点序同序），**不**做任何 locale 相关的比较。
    pub fn paths(selection: &BTreeSet<String>, all_paths: &BTreeSet<String>) -> Vec<String> {
        if !all_paths.is_empty() && selection == all_paths {
            return Vec::new();
        }
        selection.iter().cloned().collect()
    }

    /// 一条 `enqueue` 请求的参数体**编码后**有多少字节。
    ///
    /// 用与线上**同一个**编码器量，不是估算：估算要么把合法的请求误杀，要么把超限的放过
    /// ——两种都是这个守卫的本意要防的。
    ///
    /// ⚠️ Rust 侧的"同一个编码器"就是 `serde_json`（[`crate::client::CoreClient::call`]
    ///    用 `serde_json::to_string(&Request{..})` 把请求写上线），所以这里量的字节数
    ///    与真正发出去的**是同一批**。顺带说明与上游的一处无害差异：上游的
    ///    `CoreJSON.encoder` 带 `.withoutEscapingSlashes`，而 `serde_json` **本来就不转义
    ///    `/`**，两者在这一点上同形；非 ASCII 也都是原样 UTF-8 输出。
    ///
    /// 编码失败按 `usize::MAX` 计（保守：宁可拒绝，不发一条可能卡死的请求）。
    /// 上游对应的是 `Int.max` —— 同一个判据在两种语言里的字节数表示。
    pub fn request_bytes(paths: &[String]) -> usize {
        let params = serde_json::json!({ "paths": paths });
        match serde_json::to_string(&params) {
            Ok(text) => text.len(),
            // 编码失败**只在理论上**可能（`params` 是一个 `Value`，永远序列化得出来），
            // 但仍然要有一个明确结论：取最大值 = "这条请求一定超预算"。理由同上：
            // 宁可拒绝一条合法的请求，也不发一条可能把客户端永久堵死的请求。
            Err(_) => usize::MAX,
        }
    }

    /// 参数体是否**超过**安全预算。
    ///
    /// ⚠️ 判据是 `>`（**严格**大于）：恰好等于预算**不算超**。差一个字节就拒绝，
    ///    等于把上界悄悄挪小——边界由 `the_budget_boundary_is_inclusive` 钉住。
    pub fn exceeds_request_budget(paths: &[String]) -> bool {
        Self::request_bytes(paths) > Self::REQUEST_BUDGET_BYTES
    }

    /// 这个动作此刻**按不下去**的理由（`None` = 可以按）。
    ///
    /// ## ⚠️ 入参是"真正要发出去的那一串"，**不是**用户的勾选面
    ///
    /// 传进来的就是 [`Self::paths`] 的返回值（也就是 `enqueue` 的 `params.paths` 的内容）。
    /// 两条理由：① 量的是**请求体**，而请求体由那一串决定，不是由勾选面决定；
    /// ② 整批全选会被 `paths` 收敛成**空数组** ⇒ 它**永远**不超预算 —— 这正是 C-3
    /// 留下的那条出路：判据若挂在勾选面上，"全选一个超大批次"这条**唯一走得通的路**
    /// 会被自己挡下来（而它恰恰是最需要走得通的那一条）。
    ///
    /// ## ⚠️ 它是"会不会被挡 + 挡下来时说什么"的**唯一**落点（任务 17）
    ///
    /// 两个调用点共用它，**不各写一份**：
    ///   * 命令层拼**载荷**那一格（`api::tree::DownloadAction::blocked_reason`）——
    ///     让界面在用户**点之前**就说得出"此刻按不下去"（工具栏那颗据此禁用）
    ///     并把这句话摆在底栏；
    ///   * `enqueue` 自己那道**兜底闸**（`commands::enqueue_with`）—— 前端那一格有刷新延迟
    ///     （勾选面变了、载荷还没回来），窗口里点下去仍会撞上它；那条路上必须
    ///     **一个字节都不发出去**，并把同一句话当成这一发 `enqueue` 的回执交给用户。
    ///
    /// ## ⚠️ 措辞的纪律（与 [`Self::empty_selection_hint`] 同一条）
    ///
    /// 只陈述事实 + 给一个**界面上真的做得到**的动作。不写"把 enqueue 分批发"——
    /// 界面里没有"分批"这个动作，那是在教用户做一件他做不到的事
    /// （`client.rs` 原来那句就是这么写的，本任务一并改正）。
    /// 也不报数字：这道闸是壳的**安全预算**（[`Self::REQUEST_BUDGET_BYTES`]），
    /// 把它当"内核的上限"说出去，对着日志排障的人会得出错误的结论
    /// （上游为同一件事记过一笔账：`macos/Sources/BenagenCoreKit/AppModel.swift:1370-1375`）。
    pub fn blocked_reason(paths: &[String]) -> Option<&'static str> {
        if Self::exceeds_request_budget(paths) {
            Some(Self::over_budget_hint())
        } else {
            // 没有理由 = 可以按。**空数组走的就是这一支**（见本函数文档的 ②）。
            None
        }
    }

    /// 超预算时那句话。**私有**：调用方只该问 [`Self::blocked_reason`] ——
    /// "什么时候说"与"说什么"分家，就会有人只挑走一半（判据留在 `blocked_reason` 里）。
    fn over_budget_hint() -> &'static str {
        "选中的项太多，超过一次能发出的上限：请少选一些"
    }

    /// 一项都没勾时要说的那句话（**明说**会发生什么，不让用户猜）。
    pub fn empty_selection_hint() -> &'static str {
        "未勾选任何项，将下载全部待下载文件"
    }

    /// 这个动作的文案。**未选中任何项时是「全部下载」**（上游 `:81-83`）。
    ///
    /// ⚠️ **`button_title` 这个名字是按上游逐字保留的**（`buttonTitle`）：它是那条
    ///    "同一件事在两处各写一句文案就会漂"的记账的锚点，改名会让后来者找不到出处。
    ///    本文件**散文**里不写界面名词（`lib.rs` 的章程），但**标识符**照上游。
    ///
    /// 它不是"禁用"而是"换一句话说"：那时发出去的是 `paths: []`（= 全部待下载），
    /// 文案必须与真正会发生的事一致 —— 一处写着「下载选中」却下了全部的文案是在骗人。
    ///
    /// ⚠️ 数值上只说"有 / 没有勾选"这一件事，**不拼项数**：项数已经由另一处
    ///    （`SelectionSummary` 的「已选 N 项」）说过了，两处各说一遍迟早会不一致。
    pub fn button_title(selection: &BTreeSet<String>) -> String {
        if selection.is_empty() {
            "全部下载".to_string()
        } else {
            "下载选中".to_string()
        }
    }

    /// 这个动作的提示文案。**两处入口共用这一份** ——
    /// 两个入口各写一句文案，改一处就会出现"同一件事两个说法"。
    pub fn help_text() -> &'static str {
        "把选中的文件加入下载队列（未选中时 = 全部待下载文件）"
    }
}

// ---------------------------------------------------------------------------
// 内核回执 → 要说的话
// ---------------------------------------------------------------------------

/// 一条没能加进下载列表的路径。两个字段都是**内核原文，逐字**（约束 3）。
///
/// 上游是 `EnqueueFeedback.Rejection`（嵌套类型）；Rust 没有嵌套类型，所以它平铺在
/// 模块里，名字逐字保留。
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct Rejection {
    pub path: String,
    pub reason: String,
}

/// `enqueue` 的**回执** → 要说的话。
///
/// ⚠️ 约束 4（不得静默少交）：`added` 与 `rejected` **两边都要有落点**。
///    - `added` 非空 → 切到传输列表（那里面有它们）；
///    - `rejected` 非空 → **逐条**把 `path` 与 `reason` 摆出来 ——
///      "这个文件没能加进下载列表"必须出现在用户眼前。
///
/// ⚠️ 这也是为什么 `added` 为空时**不切**分区：一换，用户就再也看不到那些拒绝理由了
///    （传输列表里根本没有这些文件）。留在原地把理由说清楚才是约束 4 要的。
///    这一条在"D-2 那条空回执"（`added` 空、`rejected` 也空）上同样成立、理由更直接：
///    传输列表里一个任务都没有，切过去只会看到一片空，还会把壳写的那句话吞掉。
///
/// 这个类型是**纯值**：上层只负责把 `summary` 与 `rejections` 摆出来。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct EnqueueFeedback {
    /// 顶上那句话。成功时是壳写的**计数**（内核只给了数组，没给"几个"这句话 ——
    /// `added` 为空时那句同样是壳写的，理由见 [`EnqueueFeedback::of`]）；
    /// 失败时它就是**内核原文**（约束 3：一个字都不加）。
    pub summary: String,

    /// 逐条拒绝理由（可能为空）。
    pub rejections: Vec<Rejection>,

    /// `added` 非空 → 切到传输列表。
    pub switches_to_transfers: bool,
}

impl EnqueueFeedback {
    /// 内核回执 → 要说的话。
    ///
    /// ⚠️ **`added` 为空是一条真实回执，而且是本阶段修的那个 bug 的正面**（D-2）：
    ///    `paths` 为空（「全部下载」）且内核算出的待下载集合为空时，内核回**成功**、
    ///    `added` 空数组。改之前它回的是 `invalid_params("没有匹配到任何文件")`
    ///    （`resolve_targets` 的 `out.is_empty()` 早退），于是"合法为空"被当成错误 ——
    ///    那正是用户看到的那句报错。
    ///    （"`Ok` 路径上 `added` 必然非空"那条推导描述的是**改之前**的内核，别再拿它当
    ///    "这种回执不可能出现"的证据。）
    ///
    /// 🔴 **"没有需要下载的文件"这句是壳自己写的**（约束 C-7/C-10 要求写明理由）：
    ///    这条回执上**内核一个字都没说** —— 它不是错误，没有 `message` 可登（约束 3 的例外
    ///    就是这种"内核没有原文"的场合），而"点了下载、什么都不说"恰恰是约束 4 明禁的
    ///    静默失效。所以壳把**事实**说出来：内核算出的待下载集合是空的 ⇒ 没有需要下载的文件。
    ///    ⚠️ 措辞的纪律：**只陈述事实** ——
    ///      - **不加判断**：不说"可能已经全部下载完成"（"为什么是空的"这份知识壳手里没有，
    ///        那是内核算出来的，壳替它下结论就是在编）；
    ///      - **不加建议**：不说"请稍后再试""请检查文件"之类（壳不知道该怎么办，
    ///        也不该替用户决定）；
    ///      - 不写"成功/失败"这种评价（这既不是失败，也不该被读成"下好了"）。
    ///
    /// ⚠️ 于是"**逐条** `path` + `reason`"只在**部分成功**（`added` 非空且 `rejected` 非空）
    ///    这条真实形状上做。**全拒那条路上不做逐条渲染，这是有意偏离（约束 11）**，
    ///    理由：那种情形下的 `rejected` 只活在**内核拼的 Rust `Debug` 串**里
    ///    （`[Object {"path": …, "reason": …}]`），**不是结构化字段**。壳要"逐条"就得去切
    ///    一句内核随时会改的措辞 —— 而"壳不解析内核文案"（约束 2/3）与"不改 `core/`"
    ///    （约束 10）是底线；宁可整条原文照登（约束 4 的"不静默少交"已经满足：
    ///    用户看得到全部信息）。
    pub fn of(result: &EnqueueResult) -> EnqueueFeedback {
        EnqueueFeedback {
            summary: if result.added.is_empty() {
                "没有需要下载的文件".to_string()
            } else {
                format!("已加入 {} 个下载任务", result.added.len())
            },
            rejections: result
                .rejected
                .iter()
                .map(|r| Rejection {
                    path: r.path.clone(),
                    reason: r.reason.clone(),
                })
                .collect(),
            switches_to_transfers: !result.added.is_empty(),
        }
    }

    /// 失败（`engine_start_failed` / `preflight_failed` / 引擎不可用时那条闸门）：
    /// **原地显示内核原文**、**不切走**（切走了，那句话就没人看见了）。
    ///
    /// 原文映射走 [`error_text`] —— 规格 §10 要求"错误的呈现文案**只许有一份**"，
    /// 而那份的唯一实现就是它（上游这里调的是 `AppModel.message(of:)`）；
    /// 本模块不抄第二份，上层也不映射。
    ///
    /// ⚠️ **形态偏离（W-6），三处**：
    ///   1. **收窄了类型**：上游是 `failure(of error: Error)`（`DownloadTargets.swift:168`），
    ///      收**任意** `Error`；这里只能收 `&ClientError`。被砍掉的是 `message(of:)` 里那条
    ///      `guard let e = error as? CoreError else { return "\(error)" }` 的兜底
    ///      （`AppModel.swift:1702`）—— Rust 侧没有"任意错误"这个类型，所以那条路**整个消失**，
    ///      记账写在 `presentation/error_text.rs` 的模块头（连同"日后引入第二种错误类型时
    ///      这一步要在调用点补"的处置）。
    ///   2. **丢了实参标签 `of`**：`of` 在 Rust 里不是关键字，写不出 `failure(of error: …)`
    ///      这种形状 ⇒ 签名是 `failure(error:)`。**方法名逐字保留**，丢的只是标签。
    ///   3. 同 [`EnqueueFeedback::of`] 的 `of(_ result:)`（上游的 `_` 标签在 Rust 里也不存在）。
    pub fn failure(error: &ClientError) -> EnqueueFeedback {
        EnqueueFeedback {
            summary: error_text(error),
            rejections: Vec::new(),
            switches_to_transfers: false,
        }
    }

    /// 这一发 `enqueue` **没有发出去**（壳自己把它挡下了）：把理由当作这一发的回执。
    ///
    /// ## ⚠️ 它为什么不走 [`Self::failure`]
    ///
    /// 这不是内核报的错、也不是内核不回话 —— 是**壳在自保**：请求体超过安全预算
    /// （[`DownloadTargets::blocked_reason`]），发出去只会换来一次本地报错、或者一条
    /// 内核读不全的请求。内核在这件事上**一个字都没说**，所以这句话只能由壳写
    /// （约束 3 的例外），形态与 [`Self::of`] 那条"没有需要下载的文件"逐字同款。
    ///
    /// ⚠️ `switches_to_transfers` 恒 `false`：**一件都没加进去**，切过去只会看到一片空，
    ///    还会把这句话吞掉（同 [`Self::of`] 那条 D-2 的判断）。
    ///
    /// ⚠️ `reason` 由调用方从 [`DownloadTargets::blocked_reason`] 取来 —— 本构造函数
    ///    **不重新判断**，也不添一个字（"什么时候说"与"说什么"都只有那一处）。
    ///    ⚠️ 取 `&str` 而不是 `Option`：调用方手上已经有一个 `Option`，
    ///    在 `None` 的分支上根本不该调它（那正是"只在被挡下时才成句"这条判据）。
    pub fn blocked(reason: &str) -> EnqueueFeedback {
        EnqueueFeedback {
            summary: reason.to_string(),
            rejections: Vec::new(),
            switches_to_transfers: false,
        }
    }
}

// ---------------------------------------------------------------------------
// 回执 + 它属于哪一批
// ---------------------------------------------------------------------------

/// 一条下载回执 **+ 它属于哪一批**。
///
/// ⚠️ 为什么需要"属于哪一批"：这条回执**不会因为用户换了批次就消失**
///    （约束 4：拒绝理由不能因为换了批次就没了 —— 那是"静默少交"），
///    而它可能在**换批之后**还留在原地。
///    但"已加入 4 个下载任务"旁边一旦**没有**批次码，用户会把它读成**新批次**的结果。
///    取舍：**不无条件清掉**（约束 4），而是把批次码摆在前面 —— 见 [`DownloadNotice::summary`]。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DownloadNotice {
    /// 这条回执是哪一批的（发起这次请求那一刻生效的交付码；理论上不会是 `None`，
    /// 因为"没有生效批次"时那个动作根本不可用 —— 留着是为了不把话说不出来）。
    pub code: Option<String>,
    pub feedback: EnqueueFeedback,
}

impl DownloadNotice {
    /// 内核回执 → 带批次码的这条回执。
    ///
    /// ⚠️ `code` 取 `Option<&str>` 而不是 `Option<String>`：这是 Rust 侧对 `String?` 的
    ///    常用形态，调用方给字面量或借用都不必先造一个 `String`。
    pub fn of(result: &EnqueueResult, code: Option<&str>) -> DownloadNotice {
        DownloadNotice {
            code: code.map(str::to_string),
            feedback: EnqueueFeedback::of(result),
        }
    }

    /// 失败（抛错路径）→ 带批次码的这条回执。
    pub fn failure(error: &ClientError, code: Option<&str>) -> DownloadNotice {
        DownloadNotice {
            code: code.map(str::to_string),
            feedback: EnqueueFeedback::failure(error),
        }
    }

    /// 这条回执属于当前这批吗。`None == None` 算"属于"（没有码就没有"哪一批"可比，
    /// 这时不该凭空多出一个"批次"字样）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游的实参标签 `to`（`belongsTo(to:)`）在 Rust 里没有对应物
    ///    ⇒ 签名是 `belongs_to(current_code:)`。**方法名逐字保留**，丢的只是标签
    ///    （同 [`EnqueueFeedback::failure`] 那两处）。
    pub fn belongs_to(&self, current_code: Option<&str>) -> bool {
        self.code.as_deref() == current_code
    }

    /// 这条回执顶上那句话。**不属于当前批时前面加上批次码** ——
    /// 用户必须一眼看出"这条不是我刚加载的这批的结果"。
    ///
    /// 上游 `:203-206`：`"批次 \(code ?? "—")：\(feedback.summary)"`。占位符 `—` 逐字保留
    /// （与 `format.rs` 的 `—` 同一个口径：没有值的时候不编一个值出来）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游的实参标签 `currentCode`（`summary(currentCode:)`）
    ///    在 Rust 里没有对应物 ⇒ 签名是 `summary(current_code:)`。**方法名逐字保留**。
    pub fn summary(&self, current_code: Option<&str>) -> String {
        if self.belongs_to(current_code) {
            return self.feedback.summary.clone();
        }
        format!(
            "批次 {}：{}",
            self.code.as_deref().unwrap_or("—"),
            self.feedback.summary
        )
    }
}

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/DownloadTargetsTests.swift`（21 条，逐条对位）。
    //!
    //! ⚠️ 夹具的**形状**必须与内核真会发的对得上（键是 snake_case，与真内核同一条解码口径）。
    //!    **`enqueue` 成功路径上真正会发的回执有两种**：
    //!      ① **部分成功**：`added` 非空、`rejected` 可空 —— `core/src/main.rs:1413` 那条 `Ok(...)`；
    //!      ② **一件都没加（且没有任何拒绝）**：`added` 与 `rejected` **都空**，`paths` 为空
    //!         （「全部下载」）而内核算出的待下载集合为空 —— **阶段 D 的 D-2**：
    //!         这是"没有待下载的"这条**正常**情形的回执，不是错误，也不是"死形状"。
    //!    另外两条形状仍然走**错误通道**、不是回执（所以这里没有它们的夹具）：
    //!      - "一个都没加进去 + `rejected` 非空"⇒ `Err(invalid_params("没有任何文件被加入下载：{rejected:?}"))`
    //!        （`:1335-1339`），壳整条原文照登，走 [`super::EnqueueFeedback::failure`]，
    //!        由 `an_all_rejected_enqueue_arrives_as_an_error_and_is_shown_whole` 钉着；
    //!      - `ENGINE_DISCONNECTED` 早退（`:1387-1407` 那个 `for` 循环里唯一的第三个出口）。
    //!    所以：**没有**"全拒"形状的夹具。
    //!
    //! ⚠️ **夹具的条数是故意不对称的**（`added` 3 条、`rejected` 2 条）。
    //!    两边都写 1 条的话，"摘要里数错了哪一边"这件事在断言上**不可观测**
    //!    （`added.len()` 与 `rejected.len()` 对调也能过）。
    //!
    //! ⚠️ 两条拒绝理由都是**内核里真实存在的那两句**（`core/src/main.rs:1390` 与
    //!    `:1255-1257` 的 `format!`），路径含 `×` 与空格 —— 不是自己编的措辞。

    use super::{DownloadNotice, DownloadTargets, EnqueueFeedback, Rejection};
    use crate::client::{ClientError, MAX_REQUEST_LINE_BYTES};
    use crate::protocol::EnqueueResult;
    use std::collections::BTreeSet;

    /// `enqueue` 的线上原文：**3 个加进去、2 个被拒**。
    ///
    /// 上游是 `#"""…"""#` 原样字符串；Rust 的 `r#"…"#` 同形（两者都不做转义处理，
    /// 所以 JSON 里的 `\"` 在这两种写法里都是"反斜杠 + 引号"两个字面字符，解析后都是 `"`）。
    const PARTIAL_WIRE: &str = r#"{"added":[{"gid":"g1","path":"a.bin"},{"gid":"g2","path":"b.bin"},{"gid":"g3","path":"c.bin"}],
 "rejected":[{"path":"z.bin","reason":"路径不安全（越界/控制字符/空段）"},
             {"path":"C24-8_×_25WS024/QC 图.png","reason":"清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"}]}"#;

    /// ⚠️ **这条曾经被写成"内核的死形状"——那句话现在是错的，别再写回去**（阶段 D 的 D-2）：
    /// 它是内核在「`paths` 为空（「全部下载」）且算出的待下载集合为空」时回的**正常回执**
    /// —— **成功、`added` 空数组**，不再是 `invalid_params("没有匹配到任何文件")`。
    const NOTHING_AT_ALL_WIRE: &str = r#"{"added":[],"rejected":[]}"#;

    /// **全拒**时内核回的是错误（不是回执）——原文由 `core/src/main.rs:1409-1411` 的
    /// `format!("没有任何文件被加入下载：{rejected:?}")` 拼出来（`{rejected:?}` 是 Rust 的
    /// `Debug`，所以那一段是内核自己拼的串，**不是结构化字段**）。
    const ALL_REJECTED_ERROR: &str =
        r#"没有任何文件被加入下载：[Object {"path": "z.bin", "reason": "路径不安全（越界/控制字符/空段）"}]"#;

    fn enqueued(json: &str) -> EnqueueResult {
        serde_json::from_str(json).expect("夹具是本 crate 的 EnqueueResult 的线上原文，必须能解")
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    /// 一串**确实超预算**的路径（任务 17 那两条用例共用的夹具）。
    ///
    /// ⚠️ 量级是承重的（同 `a_huge_batch_does_not_blow_up_the_request` 的夹具头）：
    ///    20 万条、每条约 49 字节 ⇒ 约 9.8 MB，**远超** 4 MiB 的预算。
    ///    条数少一个数量级的话，那两条用例会退化成"一条本来就合法的请求也合法"（恒真）
    ///    —— 本条注释写下这个数的理由，就是让下一个改它的人先算一遍。
    fn an_over_budget_set() -> Vec<String> {
        (0..200_000)
            .map(|i| format!("dir{i}/file-with-a-fairly-long-name-{i}.bin"))
            .collect()
    }

    // -----------------------------------------------------------------------
    // `paths()`：勾选面 → `enqueue` 的 paths
    // -----------------------------------------------------------------------
    //
    // ⚠️ 下面四条都传了 `all_paths`：新签名**没有旧签名的重载**（裁定 C2）——
    //    一个"传没传整批都能编过"的重载，正好能让"整批全选必须发 `[]`"被悄悄绕过。
    //    这几条测的是**显式列表**那一支，所以 `all_paths` 挑的都是**与勾选面不相等**的
    //    一批文件（多数还含一个勾选面里没有的路径），把"整批"那一支排除在外。

    /// 上游 `aDirectoryPathIsSentAsIs`。
    #[test]
    fn a_directory_path_is_sent_as_is() {
        // ⚠️ 目录路径**原样**交给内核 —— 内核的 `resolve_targets` 把它当**前缀**展开
        //    （`f.path == p || f.path.starts_with("{p}/")`）。壳**不自己展开目录**（约束 1）。
        //    这一批的文件在目录**里面**（`flat` 只列文件，目录不在里面）⇒ 两边不相等 ⇒ 显式列表。
        assert_eq!(
            DownloadTargets::paths(&set(&["a/b"]), &set(&["a/b/one.bin"])),
            v(&["a/b"])
        );
    }

    /// 上游 `selectionIsSortedForDeterminism`。
    #[test]
    fn selection_is_sorted_for_determinism() {
        // 勾选面是集合，遍历顺序不稳定：不排序 ⇒ 每次请求的 `paths` 顺序都不同，
        // 排障时没法比对两次请求（上游简报点名）。
        //
        // ⚠️ **本条的判别力比上游弱，如实记账**：Rust 侧的入参是 `BTreeSet`，
        //    `iter()` **本身就升序**，所以"排序"在这里是结构性的——把实现写成
        //    `selection.iter().cloned().collect()` 也能过这条。它仍然承重：钉住的是
        //    **返回值的顺序口径**（若有人改成 `HashSet` 入参、或手工逆序，这条立刻红），
        //    而"输入顺序 ≠ 期望顺序"这个夹具形状正是为此挑的（`c.bin` 在 `a/b` 之前给出）。
        assert_eq!(
            DownloadTargets::paths(&set(&["c.bin", "a/b"]), &set(&["c.bin", "a/b/one.bin"])),
            v(&["a/b", "c.bin"])
        );
    }

    /// 上游 `noSelectionMeansDownloadEverythingPending`。
    #[test]
    fn no_selection_means_download_everything_pending() {
        // 内核语义：`paths` 为空 = 下全部待下载。**空集合 → 空数组**（不是"什么都不发"）。
        // ⚠️ 这一批是有文件的（`all_paths` 非空）：否则"空勾选 → 空数组"会被误读成
        //    "整批全选"那一支的功劳 —— 两者在这里返回值相同，但走的是不同的分支。
        assert!(DownloadTargets::paths(&set(&[]), &set(&["a.bin"])).is_empty());
    }

    /// 上游 `pathsAreNotTrimmedOrNormalized`。
    #[test]
    fn paths_are_not_trimmed_or_normalized() {
        // 约束 3：落盘/下发路径 = 清单原文。壳不 trim 空白、不折叠 `//`、不解析 `.`
        // （那些"顺手规范化"会把这三样全改掉），也不动非 ASCII。
        //
        // ⚠️ 夹具是**故意挑的六项**（不是随手四五个）：
        //    - 期望顺序既不是输入顺序、也不是它的倒序；
        //    - 大小写、空格、`.`、`//`、`×` 都塞进去了，"顺手规范化"必红；
        //    - 项数越多，"不排序（原序）"这个变异体**恰好撞上正确顺序**的概率越低。
        let selection = set(&[
            "b.bin",
            " C24-8_×_25WS024/Figure",
            "a/./c.bin",
            "a//d",
            "a/b.bin",
            "Z.bin",
        ]);
        // 这一批的文件是勾选面里除目录（` C24-8_×_25WS024/Figure`）以外的那五项 ——
        // 两边不相等 ⇒ 显式列表（不是"整批全选"那一支）。
        let files = set(&["b.bin", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"]);
        assert_eq!(
            DownloadTargets::paths(&selection, &files),
            v(&[
                " C24-8_×_25WS024/Figure",
                "Z.bin",
                "a/./c.bin",
                "a//d",
                "a/b.bin",
                "b.bin",
            ])
        );
    }

    // -----------------------------------------------------------------------
    // 整批全选与体量预算（全局约束 C-3：超限的请求**不报错**，只把客户端挂死）
    // -----------------------------------------------------------------------

    /// 上游 `selectingEverythingSendsAnEmptyList`（勾选面覆盖整批 ⇒ 发空数组）。
    #[test]
    fn selecting_everything_sends_an_empty_list() {
        // ⚠️ 这不是优化，是**正确性**：`enqueue.paths` 的请求体随勾选面线性增长，
        //    而内核的行长上限是 8 MiB（`core/src/main.rs:113`）。超长行**不报错** ——
        //    内核只回一条 `id == 0` 的协议告警、那条请求永远等不到响应、
        //    串行的请求-响应循环被**永久堵死**，而用户一个字都看不到。
        //    发 `[]` 是**语义等价**的（内核的 `paths` 为空 = 全部**待下载**）。
        let all = set(&["a", "b", "c"]);
        assert!(DownloadTargets::paths(&all, &all).is_empty());
    }

    /// 上游 `missingOneSendsTheExplicitList`（差一项 ⇒ 发显式列表）。
    #[test]
    fn missing_one_sends_the_explicit_list() {
        // 判据是**相等**，不是"差不多"：少勾一项就必须走保守的那一边（显式列表）。
        let all = set(&["a", "b", "c"]);
        assert_eq!(DownloadTargets::paths(&set(&["a", "b"]), &all), v(&["a", "b"]));
    }

    /// 上游 `anEmptyBatchIsNotEverything`（一批是空的时，空勾选仍然是空数组）。
    #[test]
    fn an_empty_batch_is_not_everything() {
        // `all_paths` 为空时**不算**整批 —— 否则"一批里一个文件都没有"会被读成"全选"。
        // 两条分支在这里的**返回值恰好相同**（都是 `[]`），所以这条测试钉的是**不崩、不错**，
        // 判别力靠下面那句"一批为空但勾了东西"的对照来补。
        assert!(DownloadTargets::paths(&set(&[]), &set(&[])).is_empty());
        // 对照：同一批为空、勾选面非空 ⇒ 绝不能因为"整批是空的"就把勾选面丢掉。
        assert_eq!(
            DownloadTargets::paths(&set(&["x.bin"]), &set(&[])),
            v(&["x.bin"])
        );
    }

    /// 上游 `anOversizedRequestIsDetected`（明显超限的路径集合必须被判为超预算）。
    #[test]
    fn an_oversized_request_is_detected() {
        // 20 万条、每条约 50 字节 ⇒ 约 10 MB，远超 4 MiB 的预算。
        let many: Vec<String> = (0..200_000)
            .map(|i| format!("dir{i}/file-with-a-fairly-long-name-{i}.bin"))
            .collect();
        assert!(DownloadTargets::exceeds_request_budget(&many));
    }

    /// 上游 `aNormalRequestIsWithinBudget`（正常规模不误伤）。
    #[test]
    fn a_normal_request_is_within_budget() {
        assert!(!DownloadTargets::exceeds_request_budget(&v(&["a.txt", "dir/b.bin"])));
    }

    /// 上游 `theBudgetKeepsHalfTheKernelLimit`（预算是内核上限的一半）。
    #[test]
    fn the_budget_keeps_half_the_kernel_limit() {
        // 内核的行长上限是 8 MiB（`core/src/main.rs:113`）。守卫**宁可早一点拒绝**，
        // 也不能放过一条会永久堵死客户端的请求 —— 所以取一半。
        // ⚠️ 钉的是**指定过的那个精确值**（"安全预算"是一个被决定过的数，
        //    不是可以从别处推出来的；改了它就得有人重新论证一次）。
        assert_eq!(DownloadTargets::REQUEST_BUDGET_BYTES, 4 << 20);
        // 与内核那一侧的额度对照（不改判据，只把"一半"这层关系摆在测试里）：
        assert_eq!(DownloadTargets::REQUEST_BUDGET_BYTES * 2, MAX_REQUEST_LINE_BYTES);
    }

    /// 上游 `theBudgetBoundaryIsInclusive`（恰好等于预算不算超，多一个字节才算）。
    #[test]
    fn the_budget_boundary_is_inclusive() {
        // ⚠️ 这条补的是上面两条测不到的东西：`an_oversized_request_is_detected`（约 10 MB）与
        //    `a_normal_request_is_within_budget`（约 20 字节）只把预算夹在一个**很宽**的区间里
        //    —— 把预算从 4 MiB 改成 1 MiB，那两条**照样全绿**。
        //    边界这条把判据钉死在指定的那个数上，并钉住"`>` 而不是 `>=`"
        //    （差一个字节就拒绝，等于把上界悄悄挪小）。
        //
        // 单路径的编码是 `{"paths":["…"]}` ⇒ 字节数 = 信封 + 路径长度。用被测的
        // `request_bytes` 自己量出信封（**不硬编码 14** —— 那是把实现抄进测试）。
        let envelope = DownloadTargets::request_bytes(&v(&[""]));
        let exact = DownloadTargets::REQUEST_BUDGET_BYTES - envelope;
        assert!(exact > 0, "信封本身不该就超过预算（否则这个预算毫无意义）");

        let on_the_line = v(&[&"x".repeat(exact)]);
        assert_eq!(
            DownloadTargets::request_bytes(&on_the_line),
            DownloadTargets::REQUEST_BUDGET_BYTES,
            "夹具没搭准：这一条应当**恰好**用满预算"
        );
        assert!(
            !DownloadTargets::exceeds_request_budget(&on_the_line),
            "恰好等于预算不算超（`>`）"
        );

        let one_byte_over = v(&[&"x".repeat(exact + 1)]);
        assert!(
            DownloadTargets::exceeds_request_budget(&one_byte_over),
            "多一个字节就该拒绝"
        );
    }

    /// 🔴 **超过预算的请求会被挡下，而且挡下来的那句话是"做得到的事"**（任务 17）。
    ///
    /// 判别力（两条，都要**突变实测**）：
    ///   · 把 `blocked_reason` 改成恒 `None`（判据没有调用点 ⇒ 请求照发）⇒ 第一段红；
    ///   · 把 `over_budget_hint` 改回"把 enqueue 分批发"那类措辞 ⇒ 第二段红
    ///     —— 界面里没有"分批"这个动作，写它就是教用户做一件他做不到的事
    ///     （`client.rs` 原来那句正是这么写的，本任务一并改正）。
    #[test]
    fn an_over_budget_request_is_blocked_with_a_sentence_the_ui_can_act_on() {
        let why =
            DownloadTargets::blocked_reason(&an_over_budget_set()).expect("超预算必须有理由（它会挡住这一发）");
        assert_eq!(
            why, "选中的项太多，超过一次能发出的上限：请少选一些",
            "这句话是**壳自己写的**（内核在这件事上一个字都没说），逐字钉住"
        );
        // ⚠️ 出路必须**明说**（约束 4：不许只丢下一句"不行"）。
        assert!(
            why.contains("少选"),
            "必须给出路，不是光说「不行」：{why}"
        );
        // ⚠️ 而且那个出路**必须在界面上真的做得到**：本代**没有**"分批"这个动作。
        assert!(
            !why.contains("分批"),
            "不许把「分批」当成一个做得出来的动作写进这句话（界面里没有它）：{why}"
        );
        // 也不把壳的**安全预算**说成"内核的上限"（上游为同一件事记过一笔账）。
        assert!(
            !why.contains("内核") && !why.contains("8 MiB") && !why.contains("4 MiB"),
            "这句话不报数字、不提内核：报错了数字会让排障的人得出错误的结论：{why}"
        );
    }

    /// ⚠️ **没超预算时不许冒出理由** —— 尤其是"整批全选"那条**唯一的出路**。
    ///
    /// 判别力：把 `blocked_reason` 改成恒 `Some(...)`（"先挡住再说"）⇒ 这一条红 ——
    /// 而真机上那是"超大批次再也下不了"：整批全选发的是**空数组**（请求体恒定），
    /// 它必须永远走得通（C-3 的全部意义就是把它从"发不出去"变成"发得出去"）。
    #[test]
    fn the_empty_and_the_normal_request_are_never_blocked() {
        // ① 空数组（= 全部待下载）：请求体恒定在十几字节 ⇒ 永远不超预算。
        assert!(
            !DownloadTargets::exceeds_request_budget(&[]),
            "空数组是整批全选那条出路，它必须永远在预算之内"
        );
        assert!(DownloadTargets::blocked_reason(&[]).is_none());
        // ② 正常规模不许误伤（否则用户连三个文件都下不了）。
        assert!(DownloadTargets::blocked_reason(&v(&["a.txt", "dir/b.bin"])).is_none());
        // ③ 与 `exceeds_request_budget` **是同一个判据**（不是各算各的）：边界两侧都对得上。
        let envelope = DownloadTargets::request_bytes(&v(&[""]));
        let exact = DownloadTargets::REQUEST_BUDGET_BYTES - envelope;
        assert!(DownloadTargets::blocked_reason(&v(&[&"x".repeat(exact)])).is_none(), "恰好等于预算不算超");
        assert!(DownloadTargets::blocked_reason(&v(&[&"x".repeat(exact + 1)])).is_some(), "多一个字节就该挡");
    }

    /// **被挡下的那一发要有一句说得出话的回执**（不能"点了没反应"）。
    ///
    /// 判别力：把 `blocked` 的 `summary` 改成空串（或塞一句"出错了"）⇒ 这一条红 ——
    /// 而真机上那是约束 4 明禁的静默失效：用户点了下载，界面一个字都不说。
    #[test]
    fn a_blocked_enqueue_says_why_and_does_not_switch_views() {
        let why = DownloadTargets::blocked_reason(&an_over_budget_set()).expect("夹具必须超预算");
        let f = EnqueueFeedback::blocked(why);

        assert_eq!(f.summary, why, "回执顶上那句就是判据说出的那句话（一个字都不加）");
        assert_eq!(f.summary, "选中的项太多，超过一次能发出的上限：请少选一些");
        assert!(!f.switches_to_transfers, "一件都没加进去，切过去只会看到一片空");
        assert!(f.rejections.is_empty(), "没有「哪几个文件被拒」这回事：这一发根本没发出去");
    }

    /// 上游 `theEmptySelectionHintSaysWhatWillActuallyHappen`。
    #[test]
    fn the_empty_selection_hint_says_what_will_actually_happen() {
        // 一项都没勾时发的是 `paths: []` = 全部待下载 —— 那句提示必须**明说**这件事
        // （"按下去会下全部"，不是"按下去什么都没发生"）。空白文案 = 静默（约束 4）。
        assert_eq!(
            DownloadTargets::empty_selection_hint(),
            "未勾选任何项，将下载全部待下载文件"
        );
    }

    /// 上游 `theButtonSaysDownloadAllWhenNothingIsSelected`。
    #[test]
    fn the_button_says_download_all_when_nothing_is_selected() {
        // 未选中任何项时文案变成「全部下载」，发出去的是 `paths: []`。
        // 所以这个动作**不是禁用**，是换一句话说（"按下去什么都不会发生"才该禁用）。
        assert_eq!(DownloadTargets::button_title(&set(&[])), "全部下载");
        assert_eq!(DownloadTargets::button_title(&set(&["a.bin"])), "下载选中");
        assert_eq!(
            DownloadTargets::button_title(&set(&["a.bin", "b/c"])),
            "下载选中",
            "选中几项都是同一句话：文案只说「有 / 没有勾选」这一件事"
        );
        // 两个文案必须**不同** —— 相同的话，"有没有勾选"就看不出来，
        // 而这两个状态发出去的请求是两件完全不同的事（`[路径]` vs 全部待下载）。
        assert_ne!(
            DownloadTargets::button_title(&set(&[])),
            DownloadTargets::button_title(&set(&["a.bin"]))
        );
        assert!(
            !DownloadTargets::help_text().trim().is_empty(),
            "提示文案不得空白（约束 4），且两处入口共用这一份"
        );
    }

    // -----------------------------------------------------------------------
    // `EnqueueFeedback`：`added` 与 `rejected` 两边都要有落点（约束 4）
    // -----------------------------------------------------------------------

    /// 上游 `addedTasksSwitchToTheTransfersSection`。
    #[test]
    fn added_tasks_switch_to_the_transfers_section() {
        let f = EnqueueFeedback::of(&enqueued(PARTIAL_WIRE));

        assert!(f.switches_to_transfers, "`added` 非空 → 切到传输列表");
        assert_eq!(
            f.summary, "已加入 3 个下载任务",
            "数的是 `added` 的条数（夹具 added=3 / rejected=2，两边不同才看得见数错哪一边）"
        );
        assert_eq!(f.rejections.len(), 2, "切走不等于可以少交：`rejected` 也要逐条交出来");
    }

    /// 上游 `rejectionsAreListedOneByOneVerbatim`。
    #[test]
    fn rejections_are_listed_one_by_one_verbatim() {
        // 约束 4：不得静默少交 —— 每一条 `path` 与 `reason` 都要能摆到用户眼前。
        // ⚠️ 挂的是**真实形状**：部分成功（`added` 非空 + `rejected` 非空）。
        //    全拒那条路上内核回的是**错误**、根本没有回执 —— 见
        //    `an_all_rejected_enqueue_arrives_as_an_error_and_is_shown_whole`。
        let f = EnqueueFeedback::of(&enqueued(PARTIAL_WIRE));

        // **逐条**、**原文**（约束 3：不转义、不规范化），顺序按内核给的；
        // 两条理由都是内核里真实存在的那两句，其中一条带 `×`/空格、一条带内核自己 Debug 引号。
        let paths: Vec<&str> = f.rejections.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["z.bin", "C24-8_×_25WS024/QC 图.png"]);
        assert_eq!(
            f.rejections,
            vec![
                Rejection {
                    path: "z.bin".to_string(),
                    reason: "路径不安全（越界/控制字符/空段）".to_string(),
                },
                Rejection {
                    path: "C24-8_×_25WS024/QC 图.png".to_string(),
                    reason: "清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"
                        .to_string(),
                },
            ]
        );
    }

    /// 上游 `anEmptyAddedReceiptSaysThereIsNothingToDownload`。
    #[test]
    fn an_empty_added_receipt_says_there_is_nothing_to_download() {
        // ⚠️ **阶段 D 的 D-2 改写了这条回执的地位**：`{"added":[],"rejected":[]}` 从
        //    "内核的死形状"变成了**内核真会发的一条正常回执** ——
        //    `paths` 为空（「全部下载」）且算出的待下载集合为空时，内核回**成功**、
        //    `added` 空数组，而**不再**回 `invalid_params("没有匹配到任何文件")`。
        //
        // ⚠️ **这句话是壳自己写的**（约束 C-7/C-10 要求写明理由）：这条路上**内核一个字都没说**
        //    —— 它不是错误，没有 `message` 可登（约束 3 的例外），而"点了下载、什么都不说"
        //    正是约束 4 明禁的静默失效。所以壳把**事实**说出来：
        //    内核算出来的待下载集合是空的，即**没有需要下载的文件**。
        //    ⚠️ 措辞只陈述事实：**不加判断**（不说"可能已经全部下载完成"——那是壳替内核下结论，
        //       而壳手里没有"为什么是空的"这份知识）、**不加建议**（不说"请稍后再试"）。
        let nothing = EnqueueFeedback::of(&enqueued(NOTHING_AT_ALL_WIRE));

        assert!(
            !nothing.switches_to_transfers,
            "没有任务在传输列表里 —— 切过去只会看到一片空，还会把这句话吞掉"
        );
        assert_eq!(nothing.summary, "没有需要下载的文件");
        assert!(nothing.rejections.is_empty());
    }

    /// 上游 `anAllRejectedEnqueueArrivesAsAnErrorAndIsShownWhole`。
    #[test]
    fn an_all_rejected_enqueue_arrives_as_an_error_and_is_shown_whole() {
        // ⚠️ **全拒场景的处置**（有意偏离，约束 11）：内核在"一个都没加进去 + `rejected` 非空"时
        //    **不回回执**，而是回 `Err(invalid_params("没有任何文件被加入下载：{rejected:?}"))`
        //    （`core/src/main.rs:1409-1411`）。于是它走**抛错路径**、由这条工厂接手：
        //    内核原文**整条照登**，壳**不解析**那句 Rust `Debug` 串去做逐条渲染。
        //    理由：`{rejected:?}` 不是结构化字段，切它等于把壳焊在内核的一句措辞上（约束 2/3），
        //    而"逐条"那条路只在**部分成功**这条真实形状上做（约束 10：不为这个改 core）。
        //    约束 4 仍然满足：用户看得到内核说的**全部**信息（每条 path 与 reason 都在那串里）。
        // 变异体：哪天有人"顺手"去解析这句串、把 rejections 填出来，这条断言就会红。
        let f = EnqueueFeedback::failure(&ClientError::Kernel {
            code: "invalid_params".to_string(),
            message: ALL_REJECTED_ERROR.to_string(),
        });

        assert_eq!(f.summary, ALL_REJECTED_ERROR, "内核原文逐字（约束 3），不做任何加工");
        assert!(
            f.rejections.is_empty(),
            "壳不解析内核拼的 Debug 串 —— 逐条渲染只在部分成功那条路上做"
        );
        assert!(!f.switches_to_transfers, "失败要**原地**显示");
    }

    /// 上游 `failuresShowTheKernelTextAndDoNotSwitchViews`。
    #[test]
    fn failures_show_the_kernel_text_and_do_not_switch_views() {
        // `engine_start_failed` / `preflight_failed` → **原地显示内核原文**，不切走。
        // 走**真错误**（而不是直接塞一个字符串）：这条链是 `ClientError` → 呈现文案
        // → 用户眼前那句话，中间任何一段换成"壳自己编的措辞"都会被下面的逐字断言判红。
        let f = EnqueueFeedback::failure(&ClientError::Kernel {
            code: "engine_start_failed".to_string(),
            message: "启动下载引擎失败：端口 6800 被占用".to_string(),
        });

        assert_eq!(f.summary, "启动下载引擎失败：端口 6800 被占用", "内核原文逐字（约束 3）");
        assert!(!f.switches_to_transfers, "失败要**原地**显示 —— 切走就等于把这句话吞掉");
        assert!(f.rejections.is_empty(), "失败没有「哪几个文件」这回事");
    }

    // -----------------------------------------------------------------------
    // `DownloadNotice`：回执 + 它属于哪一批
    // -----------------------------------------------------------------------

    /// 上游 `aNoticeKnowsWhichBatchItBelongsTo`。
    #[test]
    fn a_notice_knows_which_batch_it_belongs_to() {
        let n = DownloadNotice::of(&enqueued(PARTIAL_WIRE), Some("AAA-1"));

        assert!(n.belongs_to(Some("AAA-1")));
        assert_eq!(
            n.summary(Some("AAA-1")),
            "已加入 3 个下载任务",
            "属于当前批：不加前缀（否则每一行都挂一个批次码，噪声）"
        );
        assert!(!n.belongs_to(Some("BBB-2")));
    }

    /// 上游 `aStaleNoticeSaysWhichBatchItCameFrom`。
    #[test]
    fn a_stale_notice_says_which_batch_it_came_from() {
        // ⚠️ 换批**不**把这条清掉（约束 4：拒绝理由不能因为用户换了批次就消失），
        //    代价是它可能属于上一批 —— 所以**不属于当前批时前面必须带批次码**，
        //    否则"已加入 3 个下载任务"会被读成**新批次**的结果。
        let n = DownloadNotice::of(&enqueued(PARTIAL_WIRE), Some("AAA-1"));

        assert_eq!(n.summary(Some("BBB-2")), "批次 AAA-1：已加入 3 个下载任务");
        assert_eq!(n.feedback.rejections.len(), 2, "换批之后拒绝理由仍然在手（它们还能被摆出来）");
    }

    /// 上游 `aFailedNoticeCarriesItsBatchToo`。
    #[test]
    fn a_failed_notice_carries_its_batch_too() {
        let f = DownloadNotice::failure(
            &ClientError::Kernel {
                code: "preflight_failed".to_string(),
                message: "磁盘空间不足：需要 10 GB，可用 2 GB".to_string(),
            },
            Some("AAA-1"),
        );

        assert_eq!(f.summary(Some("AAA-1")), "磁盘空间不足：需要 10 GB，可用 2 GB");
        assert_eq!(
            f.summary(Some("BBB-2")),
            "批次 AAA-1：磁盘空间不足：需要 10 GB，可用 2 GB"
        );
        assert!(!f.feedback.switches_to_transfers);

        // 没有批次码（理论上到不了：没有生效批次时那个动作不可用）时不该凭空冒出一个批次字样。
        let no_code = DownloadNotice::of(&enqueued(NOTHING_AT_ALL_WIRE), None);
        assert!(no_code.belongs_to(None), "None == None 算属于自己，别凭空写「批次 —」");
        assert_eq!(no_code.summary(None), "没有需要下载的文件");
        assert_eq!(
            no_code.summary(Some("AAA-1")),
            "批次 —：没有需要下载的文件",
            "真有码可比且对不上时，宁可写占位也不冒充新批次"
        );
    }
}
