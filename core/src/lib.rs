//! `benagen-core` 的库形态。
//!
//! ⚠️ 为什么要有这个文件（2026-09-21）：本 crate 现在有**两个入口** ——
//!    `benagen-core`（图形客户端的后端，走 stdio 行协议）与 `benagen-dl`（命令行工具）。
//!    两者的编排语义必须**只有一份**，所以操作层（`kernel`）与引擎模块都搬进库里，
//!    两个 bin 都只是它的壳。详见 `docs/superpowers/specs/2026-09-21-benagen-cli-design.md` §4。

pub mod cli;
pub mod crc64xz;
pub mod delivery;
pub mod engine;
pub mod kernel;
pub mod planner;
pub mod protocol;
pub mod settings;
pub mod state;
pub mod verify;
pub mod view;

#[cfg(test)]
pub mod testutil;

// ---------------------------------------------------------------------------
// 本内核新增的错误码（整块自 `main.rs` 搬来，只把 `mod` 升成 `pub mod`）
// ---------------------------------------------------------------------------

/// `protocol::codes` 之外的业务错误码。
///
/// ⚠️ `protocol.rs` 的文件头写着"各业务模块的码由各自的响应构造点给出，
/// 届时一并列在这里以便壳侧有一份完整清单"——本任务**只被授权改 protocol.rs 的两处注释**，
/// 所以这份清单暂时留在这里。壳侧的完整码表 = `protocol::codes` ∪ `kcodes`。
/// 这是本任务报告里点名的一处待裁决项，**不是**遗漏。
pub mod kcodes {
    /// 请求了一个内核不认识的 `method`。
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    /// 还没有 `load_delivery`，或上一批已经被换码作废。
    pub const NO_DELIVERY: &str = "no_delivery";
    /// 拉清单失败（网络、404、清单本身不合法）。
    pub const DELIVERY_FETCH_FAILED: &str = "delivery_fetch_failed";
    /// 开工前检查没过（目标目录不可写 / 磁盘不足）。**引擎没有被启动。**
    pub const PREFLIGHT_FAILED: &str = "preflight_failed";
    /// 引擎还没起来（`enqueue` 之外的读方法会走到这里）。
    pub const ENGINE_NOT_STARTED: &str = "engine_not_started";
    /// 引擎启动失败（端口、二进制、RPC 不可达）。
    pub const ENGINE_START_FAILED: &str = "engine_start_failed";
    /// 引擎已被判定断开，且**这次重连也没成功**（设计规格 §9）。
    pub const ENGINE_DISCONNECTED: &str = "engine_disconnected";
    /// 一次 RPC 调用失败，但重连探活是好的（瞬时故障，界面可以重试）。
    pub const ENGINE_RPC_FAILED: &str = "engine_rpc_failed";
    /// `list_dir` 的路径不在清单里。
    pub const PATH_NOT_FOUND: &str = "path_not_found";
}
