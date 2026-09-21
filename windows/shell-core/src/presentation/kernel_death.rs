//! kernel_death —— **「内核进程没了」那一档的共享判定**（规格 §9 的第一步）。
//!
//! 上游：`macos/Sources/BenagenCoreKit/AppModel.swift` 的 `absorb(_:)` 与 `onKernelDeath(_:)`。
//! 上游把这件事**收在一处**（`AppModel` 是唯一的错误处置点），于是"哪个页面先看见内核死了"
//! 不影响结论 —— 引擎那一格只被写过一次、横幅只有一条、闸门随后对**所有**页面关闭。
//!
//! ## ⚠️ 为什么必须是一处（集成树上的实测形态，2026-09-19 最终审查「关键 1」）
//!
//! 内核死亡**不是某一页的私事**：文件页的 `list_dir`、传输列表的 `transfer_list`、
//! 校验页的 `verify_status` 撞上的是**同一个** [`ClientError::KernelGone`]。三处各写一份
//! 就会变成三份判据、三种时机（谁先谁后决定横幅什么时候出现、说的是哪一句），
//! 迟早不一致；而上游那套处置里还有"**至多一次**自动重启"这种**有状态**的东西 ——
//! 在三处各写一遍就是重启三次。
//!
//! 本模块只做**判定**（一条错误是不是"内核没了"、写进引擎那一格的话是哪句）；
//! "把它写下去、且**至多一次**"落在壳侧唯一的那一格上（`shell-win` 的
//! `ShellState::note_kernel_death`）。两半合起来才是上游那一处处置。
//!
//! ## ⚠️ 两个分支的**措辞不同**，这是照源的，不是笔误
//!
//! 上游 `absorb` 把"内核没了"分成两支，给的话**不一样**：
//!
//! | 上游分支 | 写进 `engine` 的 | 本模块 |
//! |---|---|---|
//! | `.transport(why)` ⇒ `onKernelDeath(why)` | `"下载引擎已断开：\(why)"`（**壳加前缀**） | [`KernelDeath::PREFIX`] + `why` |
//! | `.rpc(code: engine_disconnected \| engine_start_failed, message)` | `message` —— **内核原文逐字，不加前缀** | [`error_text`] 原样 |
//!
//! ⚠️ 两支都套前缀会造出一句**重复的话**：内核那支的 `message` 本身常常就是
//!    「下载引擎已断开」，套上去成了「下载引擎已断开：下载引擎已断开」。
//!    约束 3（壳不加工内核的话）在这里就是判据：**内核亲口说的，原样登。**
//!
//! ## ⚠️ 两个**内核报的**码**并列**（照上游；控制者 2026-09-19 裁定）
//!
//! 上游那一支是 `case .engineDisconnected?, .engineStartFailed?` —— 两个码都翻 `Unavailable`。
//! 本模块起初只认 `engine_disconnected`（简报当时只点了那一个），**这是一处可达的显示分歧**：
//! `core/src/main.rs` 真的会发 `engine_start_failed`（例如端口被占、引擎起不来），
//! 那一刻 macOS 出的是**带「重试」的引擎横幅 + 闸门关闭**，而 Windows 只会把它当一条
//! 瞬时错误 ⇒ 与"报错与显示与功能与 macOS 完全一致"这条总要求直接冲突。
//! ⇒ **两个码并列**（与控制者的裁定一致）。
//!
//! ⚠️ 这一条**不是**"凡是内核错误都翻引擎"：`engine_rpc_failed`（可自愈的抖动）
//!    与 `engine_not_started`（正常态）都不在这里，判据的判别力由
//!    [`tests::everything_else_is_not_a_kernel_death`] 正面守住。
//!
//! ⚠️ **形态（W-6）**：上游没有这个类型 —— 那两行代码住在**未移植**的 `AppModel` 里。
//!    本 crate 的既定惯例是"源在 `AppModel.swift` 里、本波次又必须存在的纯计算"
//!    一律落 `presentation/` 下按主题命名的 `pub` 模块、自带单测（裁决 GG，与
//!    `error_text.rs` 同一条）。

use crate::client::ClientError;
use crate::presentation::error_text::error_text;
use crate::protocol::codes;

/// 「内核没了」的判定与它那句原文。
pub enum KernelDeath {}

impl KernelDeath {
    /// 传输层那一支的前缀。**逐字**是上游 `onKernelDeath` 里那个字符串插值的前半截
    /// （`AppModel.swift:1501`：`"下载引擎已断开：\(reason)"`）。
    pub const PREFIX: &'static str = "下载引擎已断开：";

    /// 这一条 `ClientError` 是不是"内核没了"？
    ///
    /// 是 ⇒ 交出**要写进 `EngineState::Unavailable` 的那句话**（上游 `engine = .unavailable(…)`
    /// 的实参，一个字的加工都没有）；不是 ⇒ `None`，调用方照原样走它本来那条路。
    ///
    /// ⚠️ **判据一律看结构化 `code`**（契约 §5.1：不看 `message` 的措辞）——
    ///    内核那支认的是 [`codes::ENGINE_DISCONNECTED`] 这个**码**，不是那句话长什么样。
    pub fn reason_of(error: &ClientError) -> Option<String> {
        match error {
            // ① 传输层：管道结束 ⇒ 内核进程没了（`client.rs`：这是"内核已死"的唯一可靠信号）。
            ClientError::KernelGone => Some(format!("{}{}", Self::PREFIX, error_text(error))),
            // ② 内核**自己**报的那两个码（上游 `case .engineDisconnected?, .engineStartFailed?`）：
            //    "引擎已被判定断开、这次重连也没成功"（设计规格 §9）与"引擎起不来"
            //    （端口被占之类）。**两个并列**，因为上游就是并列的、两边的可见结果相同。
            //    ⚠️ 原文字面照登（约束 3）：**不加**上面的前缀，理由见模块头那张表。
            ClientError::Kernel { code, .. }
                if code == codes::ENGINE_DISCONNECTED || code == codes::ENGINE_START_FAILED =>
            {
                Some(error_text(error))
            }
            _ => None,
        }
    }
}

/// 一条壳侧内核调用的失败：**内核没了**，还是**别的一句话**。
///
/// ⚠️ 它存在的理由是**类型上分不开**：两条路在调用方看来都是 `Err`，而落点完全不同 ——
///    内核死亡要交回**共享判定**（翻引擎那一格，随后横幅 / 闸门 / 三页的停手全部自动生效），
///    其余失败只是"这一屏这一句话"。用一个 `String` 承两种契约，就是本项目反复记账的
///    那种"一格两义"（`ShellState::last_error` 那一跤的同型）。
///
/// ## ⚠️ 内核没了那一支**带两句话**（照上游 `absorbing`：吸收 + **重抛**）
///
/// 上游的 `AppModel.absorbing` 是：
/// ```text
/// catch let e as CoreError { _ = await absorb(e)   // ← 处置引擎那一格（本模块的 reason）
///                            throw e }             // ← **照样抛给调用方**
/// ```
/// 调用方（视图）于是**就地**把这次动作的失败显示出来（`DownloadNotice.failure`）。
/// ⇒ **两件事都要做**，而且**两句话不是同一句**：
///
///   * [`CallFailure::KernelGone::reason`] —— 写进 `engine` 那一格的话（**引擎横幅**用）；
///   * [`CallFailure::KernelGone::text`] —— 内核原文（[`error_text`]），给**这一屏**就地显示
///     "这一次动作没成"（**不带**壳给引擎横幅加的那半句前缀，免得与横幅逐字重复）。
///
/// ⚠️ **少了 `text` 那一半就是一次静默失效**（集成树复审实测）：调用方只翻引擎、
///    不写自己那条提示 ⇒ **用户按下去一点反应都没有**（引擎横幅早就挂在那里了，
///    它是**上一刻**的状态，不是"你刚才那一下没成"的回答）。
///
/// ⚠️ 它**不为 `list_dir` 服务**：那条路的失败要带**结构化错误码**回去给
///    `DirLoadFailure::of` 认（`path_not_found` 的回退判据），所以它走的是
///    `Result<_, ClientError>`，由 `poll` 在拿到 `ClientError` 的地方直接问
///    [`KernelDeath::reason_of`]。**而那一支本来就会显示**（`DirLoadFailure` 就是它的落点）。
pub enum CallFailure {
    /// 内核没了：**翻引擎**（`reason`）+ **就地显示这一次动作没成**（`text`）。
    KernelGone {
        /// 写进 `engine` 那一格的话（[`KernelDeath::reason_of`] 的产物）。
        reason: String,
        /// 这一屏要显示的那句话 —— 内核原文（[`error_text`]）。
        text: String,
    },
    /// **内核亲口说：这一批不存在或已过期**（结构化码 `no_delivery`，见 [`is_no_delivery`]）。
    ///
    /// ## 🔴 它为什么要单独一格（而不是并进下面的 `Text`）
    ///
    /// 因为调用方**据此作废当前批次**（`Session::reset_to_empty_state`，对齐 macOS
    /// `AppModel.resetToEmptyState()`）。少了这一格，一件不响亮的事就会发生：
    /// 批次作废之后**"当前批次"那一格还留着那个死码**，于是下一次内核重启
    /// （改下载目录 / 点「重试」）会把它**重放**一遍 —— 用户看到一次
    /// **本不该出现的失败**，而 macOS 在那一刻是**空态**。
    ///
    /// ⚠️ macOS 的对应物是 `absorb` 的 `.absorbed` 那一支（`noDelivery` →
    ///    `resetToEmptyState()`）：那一支**不显示**这句话（它是正常态，不是错误），
    ///    只把界面退回空态。我们这一侧"显示"那一半的处置见 `commands::note_failure`。
    ///
    /// ⚠️ 载荷是**内核原文**（`error_text`，约束 3：逐字、不加工）—— 一个字符都没变
    ///    （与 `Text` 的唯一区别是**调用方拿它做什么**）。
    NoDelivery(String),
    /// **内核亲口说：下载引擎还没起来**（结构化码 `engine_not_started`，
    /// 见 [`is_engine_not_started`]）。
    ///
    /// ## 🔴 它为什么要单独一格
    ///
    /// 因为调用方**据此把引擎那一格翻成 `NotStarted`**（对齐 macOS `AppModel.absorb` 的
    /// `case .engineNotStarted?: engine = .notStarted`）。少了这一格，一件用户**看得见**的
    /// 事就会发生：**引擎徽标停在「运行中」，而内核刚说过引擎根本没起来** ——
    /// 徽标是工具栏四项之一，用户会照着它判断"任务在跑"。
    ///
    /// ⚠️ 它是**正常态**（macOS 的 `.absorbed`）：**不写 `last_error`**、不上屏。
    /// ⚠️ 载荷是内核原文（`error_text`，约束 3）。
    EngineNotStarted(String),
    /// 其余失败：要显示的那句话（内核原文，或壳自己写的现场诊断）。
    Text(String),
}

impl CallFailure {
    /// 一条 [`ClientError`] → 该走哪条路。**唯一**的分派点。
    ///
    /// ⚠️ **判据的次序**：先问"内核是不是死了"（那一档要翻引擎），再问"内核是不是说
    ///    这一批没了"（那一档要作废当前批次），其余才是普通失败。
    ///    两条判断**不会同时成立**（一个是 transport / 引擎家族的码，一个是 `no_delivery`）。
    pub fn of(error: &ClientError) -> Self {
        if let Some(reason) = KernelDeath::reason_of(error) {
            return CallFailure::KernelGone {
                reason,
                text: error_text(error),
            };
        }
        if is_engine_not_started(error) {
            return CallFailure::EngineNotStarted(error_text(error));
        }
        if is_no_delivery(error) {
            return CallFailure::NoDelivery(error_text(error));
        }
        CallFailure::Text(error_text(error))
    }

    /// 壳自己写的一句话（没有内核原文可登的那些：解不动回执、后台线程炸了…）。
    pub fn shell(text: String) -> Self {
        CallFailure::Text(text)
    }

    /// 这一屏**就地显示**的那句话。
    ///
    /// ⚠️ `KernelGone` 那一支给的是 `text`（内核原文）而**不是** `reason`
    ///    （带壳那半句前缀的引擎版本）—— 两句话不重复：横幅说"引擎没了"，
    ///    这一句说"你刚才那一下没成"（`views/file_browser.rs` 的记账）。
    pub fn text(&self) -> &str {
        match self {
            CallFailure::KernelGone { text, .. } => text,
            CallFailure::EngineNotStarted(t) => t,
            CallFailure::NoDelivery(t) => t,
            CallFailure::Text(t) => t,
        }
    }
}

/// **内核亲口说"这一批不存在或已过期"吗** —— 判据是**结构化错误码** `no_delivery`，
/// **不是措辞**（契约 §5.1：壳不得靠字符串匹配判断错误类型）。
///
/// ⚠️ 它与 [`KernelDeath::reason_of`] 是**同一个 `ClientError` 上的两条独立判断**，
///    所以住在一起（都是"读一个错误、得出一个结论"的纯函数：
///    一个回答"内核还在吗"，一个回答"当前批次还有效吗"）。
///    ⚠️ 两个消费者：
///      * [`CallFailure::of`] —— 非传输那条路（tree / enqueue / verify / …）；
///      * `shell-win` 的 `kernel::transfer_list` —— 轮流询那条路（它把 `no_delivery`
///        当作**正常态**吸收，所以不走 `CallFailure`）。
///    **两处必须是同一条判据** —— 分开写两份的结局是"两条路对同一件事有两种结论"。
///
/// ⚠️ macOS 的对应物是 `AppModel.absorb` 里 `case .noDelivery?: resetToEmptyState()`。
pub fn is_no_delivery(error: &ClientError) -> bool {
    matches!(error, ClientError::Kernel { code, .. } if code == codes::NO_DELIVERY)
}

/// **内核亲口说"下载引擎还没起来"吗** —— 判据同样是**结构化错误码** `engine_not_started`。
///
/// ⚠️ 它与 [`is_no_delivery`] 是**两个不同的正常态**，别合成一条：
///   * `engine_not_started` ⇒ 引擎那一格翻 `NotStarted`（批次**可能好好的** —— 用户还没
///     添加过任务，或者刚才那一批还在）；
///   * `no_delivery` ⇒ **这一批作废**（`Session::reset_to_empty_state`）。
///   合成一条的后果：要么把一批好端端的批次清掉（每加任务之前都会发生），
///   要么徽标一直停在「运行中」。
///
/// ⚠️ macOS 的对应物：`AppModel.absorb` 的 `case .engineNotStarted?`（`AppModel.swift:1634`）。
pub fn is_engine_not_started(error: &ClientError) -> bool {
    matches!(error, ClientError::Kernel { code, .. } if code == codes::ENGINE_NOT_STARTED)
}

#[cfg(test)]
mod tests {
    //! 四条各钉一个**不同的失效形态**，缺一条就有一种改法不会变红。

    use super::{is_engine_not_started, is_no_delivery, CallFailure, KernelDeath};
    use crate::client::ClientError;

    /// 传输层那一支：**壳加前缀**，而且前缀是逐字的那一个。
    ///
    /// 判别力：把 `format!` 换成 `error_text(error)`（丢前缀），这一条立刻红 ——
    /// 而那句前缀正是客户在界面上认出"这是内核没了，不是这一屏的事"的那半句话。
    #[test]
    fn a_transport_death_gets_the_shells_prefix_in_front_of_the_clients_text() {
        let reason = KernelDeath::reason_of(&ClientError::KernelGone)
            .expect("管道结束就是内核已死（`client.rs` 的记账）");
        assert_eq!(reason, "下载引擎已断开：内核进程已退出（管道结束）");
        assert!(
            reason.starts_with(KernelDeath::PREFIX),
            "传输层那一支必须带上壳写的前缀：{reason}"
        );
    }

    /// 内核**亲口**报的那一支：**原文字字照登，不加前缀**（约束 3）。
    ///
    /// 判别力：把这一支也套上 `PREFIX`（"两支一样处理"看起来更整齐），这一条立刻红 ——
    /// 而它造出的是一句**重复的话**（内核那句 `message` 本身常常就是「下载引擎已断开」）。
    #[test]
    fn a_kernel_reported_disconnect_is_verbatim_without_a_second_prefix() {
        let message = "下载引擎已断开";
        let error = ClientError::Kernel {
            code: "engine_disconnected".to_string(),
            message: message.to_string(),
        };
        let reason = KernelDeath::reason_of(&error).expect("这个码就是引擎断开");
        assert_eq!(
            reason, message,
            "内核自己说的那句要**原样**进引擎那一格（约束 3：壳不加工内核的话）"
        );
        assert!(
            !reason.contains(KernelDeath::PREFIX),
            "不许再套一层前缀 —— 那会造出「下载引擎已断开：下载引擎已断开」：{reason}"
        );
    }

    /// 内核报的 **`engine_start_failed`**（引擎起不来：端口被占之类）**也算**内核死亡 ——
    /// 上游 `case .engineDisconnected?, .engineStartFailed?` 是**两个码并列**的。
    ///
    /// ⚠️ 这一条是**控制者 2026-09-19 的裁定**（照上游）：本模块起初只认
    ///    `engine_disconnected`，而那是一处**可达的显示分歧** —— `core/src/main.rs`
    ///    真的会发这个码，macOS 那一刻出的是**带「重试」的引擎横幅 + 闸门关闭**，
    ///    而 Windows 只会把它当一条瞬时错误。本阶段的核心要求是
    ///    "报错与显示与功能与 macOS 完全一致" ⇒ 两个码必须并列。
    ///
    /// 判别力：把 `reason_of` 里那个 `|| code == codes::ENGINE_START_FAILED` 删掉
    /// ⇒ 本条立刻红（回到那处显示分歧）。
    #[test]
    fn an_engine_that_failed_to_start_is_a_kernel_death_too() {
        let message = "引擎启动失败：aria2c 没有在 30 秒内就绪";
        let error = ClientError::Kernel {
            code: "engine_start_failed".to_string(),
            message: message.to_string(),
        };
        // ⚠️ 断言是**相等**（不是"包含"）：它同时钉住两件事 ——
        //    ① 这个码被认出来了（`Some(…)` 而不是 `None`）；
        //    ② 与"断开"那一支同样**不加**前缀（约束 3：内核原文字面照登）。
        //    只钉 ① 的话，"两支都套前缀"那种改法不会红。
        assert_eq!(
            KernelDeath::reason_of(&error).as_deref(),
            Some(message),
            "这个码必须把引擎那一格改掉（上游与 `engine_disconnected` 并列），而且原文照登"
        );
    }

    /// **别的一律不是**：这不是"凡是错误都翻引擎"。
    ///
    /// 判别力：把 `_ => None` 改成"任何错误都算"（例如把兜底写成 `Some(error_text(error))`），
    /// 这一条立刻红 —— 而那个改动会把一次**可自愈的** RPC 抖动（`engine_rpc_failed` 是
    /// 上游明确**不**吸收的一类）翻成一条要求客户点「重试」的引擎横幅：拿它去处理一次抖动
    /// 等于杀掉一个健康的引擎。
    #[test]
    fn everything_else_is_not_a_kernel_death() {
        let not_death = [
            ClientError::Kernel {
                code: "engine_rpc_failed".to_string(),
                message: "connection reset".to_string(),
            },
            ClientError::Kernel {
                code: "engine_not_started".to_string(),
                message: "下载引擎尚未启动".to_string(),
            },
            ClientError::Kernel {
                code: "path_not_found".to_string(),
                message: "清单里没有目录".to_string(),
            },
            // 内核还在，只是发来的东西壳解不动（上游 `.malformedResponse` → `.verbatim`）。
            ClientError::LineNotUtf8,
            ClientError::ReadFailed {
                cause: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "管道断了"),
            },
        ];
        for error in &not_death {
            assert_eq!(
                KernelDeath::reason_of(error),
                None,
                "这一条不是「内核没了」，不许动引擎那一格：{error:?}"
            );
        }
    }

    /// [`CallFailure`] 的两支**互斥且穷尽**；内核没了那一支要**同时**带两句话
    /// （翻引擎用 `reason`、就地显示用 `text` —— 见那个类型自己的文档）。
    ///
    /// 判别力（两个方向都实测过）：
    ///   * 把 `of` 写成常量 `CallFailure::Text(error_text(error))`（即"忘了分派"）⇒ 红，
    ///     而那个改动会让三页继续各自把内核死亡当成一条瞬时错误（横幅没有「重试」、闸门不关、
    ///     轮询继续拍一个死内核）；
    ///   * 把 `text` 那一半丢掉（`KernelGone` 只留 `reason`）⇒ 红 —— 而那个改动是
    ///     集成树复审实测过的**回归**：调用方只翻引擎、不写自己那条提示 ⇒ 用户按下去
    ///     **一点反应都没有**（引擎横幅早就挂在那里了，它不是"你刚才那一下没成"的回答）。
    #[test]
    fn a_call_failure_keeps_the_two_contracts_and_both_messages_apart() {
        let death = CallFailure::of(&ClientError::KernelGone);
        match death {
            CallFailure::KernelGone { reason, text } => {
                assert_eq!(
                    reason, "下载引擎已断开：内核进程已退出（管道结束）",
                    "`reason` 是写进引擎那一格的那句话（带壳写的前缀）"
                );
                assert_eq!(
                    text, "内核进程已退出（管道结束）",
                    "`text` 是给这一屏就地显示的内核原文 —— **不带**那半句前缀（免得与横幅逐字重复）"
                );
                assert_ne!(reason, text, "两句话是两件事，不许退化成同一句");
            }
            CallFailure::Text(text) => panic!("内核没了却走了「别的一句话」：{text}"),
            CallFailure::NoDelivery(text) => {
                panic!("内核没了却被判成「这一批没了」：{text}")
            }
            CallFailure::EngineNotStarted(text) => {
                panic!("内核没了却被判成「引擎没起来」：{text}")
            }
        }

        let other = CallFailure::of(&ClientError::Kernel {
            code: "engine_rpc_failed".to_string(),
            message: "connection reset".to_string(),
        });
        match other {
            CallFailure::Text(text) => assert_eq!(text, "connection reset"),
            CallFailure::KernelGone { reason, .. } => {
                panic!("一次 RPC 报错不是内核没了（那会关掉一个还能用的引擎）：{reason}")
            }
            CallFailure::NoDelivery(text) => {
                panic!("一次 RPC 报错不是「这一批没了」（那会作废一批好好的批次）：{text}")
            }
            CallFailure::EngineNotStarted(text) => {
                panic!("一次 RPC 报错不是「引擎没起来」（那会把引擎那一格翻成未启动）：{text}")
            }
        }
        // 壳自己写的那一支（没有内核原文可登）。
        match CallFailure::shell("后台线程炸了".to_string()) {
            CallFailure::Text(text) => assert_eq!(text, "后台线程炸了"),
            CallFailure::KernelGone { .. } => panic!("壳自己写的诊断不是内核没了"),
            CallFailure::NoDelivery(_) => panic!("壳自己写的诊断不是「这一批没了」"),
            CallFailure::EngineNotStarted(_) => panic!("壳自己写的诊断不是「引擎没起来」"),
        }
    }

    /// 🔴 **`engine_not_started` 单独占一格**（它要把引擎那一格翻成 `NotStarted`）。
    ///
    /// 判别力（三个方向都会红）：
    ///   * 不认这个码（并进 `Text`）⇒ 第一条断言红 —— 而真机上的表现是
    ///     **引擎徽标停在「运行中」，而内核刚说过引擎根本没起来**（用户照着它做判断）；
    ///   * 把它与 `no_delivery` 合成一格 ⇒ 第二条断言红 —— 那会在**每次添加任务之前**
    ///     把一批好端端的批次清掉（`engine_not_started` 是"还没添加过任务"的常态）；
    ///   * 把它判成内核死亡 ⇒ 第三条断言红（那会去翻引擎横幅、甚至重启内核）。
    #[test]
    fn engine_not_started_is_its_own_kind_of_failure() {
        let not_started = ClientError::Kernel {
            code: "engine_not_started".to_string(),
            message: "下载引擎尚未启动（先 enqueue 才会起引擎）".to_string(),
        };
        assert!(is_engine_not_started(&not_started), "这个码就是「引擎没起来」");
        match CallFailure::of(&not_started) {
            CallFailure::EngineNotStarted(text) => assert_eq!(
                text, "下载引擎尚未启动（先 enqueue 才会起引擎）",
                "内核原文逐字（约束 3）"
            ),
            other => panic!("这个码必须走它自己那一格，实际是 {}", other.text()),
        }

        // ⚠️ 反方向：**两个正常态不许互相串**（一个是"批次好好的"，一个是"这一批没了"）。
        let expired = ClientError::Kernel {
            code: "no_delivery".to_string(),
            message: "这一批不存在或已过期".to_string(),
        };
        assert!(!is_engine_not_started(&expired), "`no_delivery` 不是「引擎没起来」");
        assert!(!is_no_delivery(&not_started), "`engine_not_started` 不是「这一批没了」");
        assert!(matches!(
            CallFailure::of(&expired),
            CallFailure::NoDelivery(_)
        ));

        // 内核没了那一档也**不许**被这一格抢走。
        assert!(!is_engine_not_started(&ClientError::KernelGone));
    }

    /// 🔴 **`no_delivery` 单独占一格**（它要触发"作废当前批次"，见那个变体的文档）。
    ///
    /// 判别力（三个方向都会红）：
    ///   * 不认这个码（并进 `Text`）⇒ 第一条断言红 —— 而真机上的表现是
    ///     "批次作废之后，下一次内核重启把那个死码**重放**一遍"，用户看到一次本不该出现的失败；
    ///   * **认错了码**（比如把 `engine_not_started` 也当成本批没了）⇒ 第二条断言红 ——
    ///     而那是**每加一个新批次之前**都会遇到的状态，误判会把好端端的一批作废掉；
    ///   * 把 `no_delivery` 判成内核死亡 ⇒ 第三条断言红（那会去翻引擎、甚至重启内核）。
    #[test]
    fn no_delivery_is_its_own_kind_of_failure() {
        let expired = ClientError::Kernel {
            code: "no_delivery".to_string(),
            message: "这一批不存在或已过期".to_string(),
        };
        assert!(is_no_delivery(&expired), "这个码就是「这一批没了」");
        match CallFailure::of(&expired) {
            CallFailure::NoDelivery(text) => {
                assert_eq!(text, "这一批不存在或已过期", "内核原文逐字（约束 3）")
            }
            other => panic!("`no_delivery` 必须走它自己那一格，实际是 {}", other.text()),
        }

        // ⚠️ 反方向：**别的码不许进这一格** —— 尤其是 `engine_not_started`
        //    （它是**另一个正常态**：引擎没起来、而批次好好的 —— 它有自己的那一格，
        //    见 `engine_not_started_is_its_own_kind_of_failure`）
        //    与 `engine_rpc_failed`（可重试的抖动）。
        for code in ["engine_rpc_failed", "path_not_found"] {
            let error = ClientError::Kernel {
                code: code.to_string(),
                message: "另外一件事".to_string(),
            };
            assert!(!is_no_delivery(&error), "{code} 不是「这一批没了」");
            assert!(
                matches!(CallFailure::of(&error), CallFailure::Text(_)),
                "{code} 该走「别的失败」那一格"
            );
        }

        // 内核没了那一档也**不许**被这一格抢走（那要翻引擎，不是作废批次）。
        assert!(!is_no_delivery(&ClientError::KernelGone));
        assert!(matches!(
            CallFailure::of(&ClientError::KernelGone),
            CallFailure::KernelGone { .. }
        ));
    }
}
