//! `SessionView` —— 一次读取拿到的那份**会话状态快照**（**整体搬自**
//! `shell-win/src/session.rs:27`，一个字节的语义都没改）。
//!
//! ## ⚠️ 它为什么搬到 `shell-core`
//!
//! 它是**纯数据**（两个枚举 + 三个 `Option<String>`），没有一点 OS 调用、也不碰锁 ——
//! 而 `api` 层的每一个载荷函数都吃它（[`crate::api::state::state`] 直接收 `&SessionView`）。
//! 留在 `shell-win` 的话，"前端收到的东西"那条契约就没法在 macOS 上单测，
//! 而那正是本代把 `api` 层搬过来的全部理由（规格 §5.3）。
//!
//! ⚠️ **`shell-win/src/session.rs` 里现在仍有一份同名同形的定义**：本任务
//! （计划任务 2）**只搬、不改 `shell-win`** —— 删掉那一份是任务 7 的事
//! （它同一批把 `SessionView` 改成引用本文件这一份）。在那之前两份并存是**预期的**，
//! 不是漂移；别为了"看起来干净"去动 `shell-win`（那是另一个任务的边界）。
//!
//! ⚠️ **本文件只有那个**值类型：`Session` 自己（`Mutex<Inner>`、心跳、注入的连接器、
//! `spawn_load` / `spawn_connect` 那两条后台线程）**留在 `shell-win/src/session.rs`**，
//! 因为它要 `Arc<CoreClient>`、要起线程、要读时钟 —— 那些是平台侧的事。

use crate::protocol::{EngineState, LoadState};

/// 一次读取拿到的那份状态快照（`/api/state` 就返回它）。
///
/// ⚠️ 它是**值**不是引用：调用方拿到之后锁就放掉了（见 `session.rs` 文件头那段锁的纪律）。
#[derive(Clone, PartialEq, Debug)]
pub struct SessionView {
    pub engine: EngineState,
    pub load: LoadState,
    /// 非抛错路径上的错误（上游 `AppModel.lastError`）。**非粘滞**：下一个成功请求就清。
    pub last_error: Option<String>,
    /// 「这次用的**不是**内嵌那一份内核」的披露（W-2）。**粘滞**：没有"已恢复"这个事件。
    ///
    /// ⚠️ **它必须是自己一格**：与 `last_error` 的契约**互斥**。合成一格的结果是实测过的
    ///    —— 一次成功的请求就会把这条披露抹掉，且没有东西会把它写回来。
    pub fallback_notice: Option<String>,
    /// `hello` 回执的**原文**（JSON 一行）。
    ///
    /// ⚠️ 它是给**人**看的证据（探路 §6 第 3 条："整条架构通了"就是靠这一行证明的），
    ///    也是诊断区唯一保留的自报项。还没连上时是 `None`。
    pub handshake_reply: Option<String>,
}
