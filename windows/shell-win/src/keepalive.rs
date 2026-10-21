//! 防休眠 + 看护线程（判据在 `shell_core::lifecycle`，这里只有 OS 调用与装配）。
//!
//! * 防休眠：`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`——阻止系统**进入睡眠**，
//!   不阻止关屏（客户锁屏、关显示器都不影响下载）。**该状态是线程级的**：必须由同一条线程
//!   反复设置并保持存活，所以放在看护线程里做，不在命令线程里做。
//! * 看护线程：窗口藏起来时前端不轮询，壳自己按节拍问内核的传输列表——
//!   让"内核 → 引擎"那条连接一直有人用，并据此决定防休眠。
//!   它只读；不写任何界面状态（界面仍由前端轮询决定，避免两处写同一格）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shell_core::lifecycle;
use shell_core::presentation::kernel_death::CallFailure;
use shell_core::presentation::transfer_row::TransferRow;

use crate::kernel::{self, TransferPoll};
use crate::session::Session;

/// 窗口当前是否可见（关窗收托盘时置 false，托盘"显示"时置 true）。
pub static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

#[cfg(windows)]
fn set_system_required(on: bool) {
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    // SAFETY：纯值参数、无指针；返回旧状态，丢弃。
    unsafe {
        SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS });
    }
}

/// 宿主（macOS/Linux）：没有这件事，如实 no-op（宿主只为本地开发能编能跑）。
#[cfg(not(windows))]
fn set_system_required(_on: bool) {}

/// 起看护线程。`on_count` 每拍收到"未落定的任务数"（给托盘更新悬停文案）。
pub fn spawn(session: Arc<Session>, on_count: Arc<dyn Fn(usize) + Send + Sync>) {
    let _ = std::thread::Builder::new().name("keepalive".to_string()).spawn(move || {
        let mut tracker = lifecycle::AwakeTracker::default();
        // 上一拍的"未落定数"。**每拍都回调**（不只是有变化时）：标题与托盘话术还跟着
        // 界面语言走，而握手落定（语言可能从系统语言变成内核回执那一门）时必须有东西
        // 把新语言刷上去 —— 见 `main.rs` 的 `on_count`。
        let mut last = 0usize;
        loop {
            std::thread::sleep(lifecycle::watch_interval(WINDOW_VISIBLE.load(Ordering::Relaxed)));
            let lang = session.lang();
            let reading = match session.client() {
                None => lifecycle::Reading::Miss,
                Some(client) => match kernel::transfer_list(lang, &client) {
                    TransferPoll::Snapshot(result) => lifecycle::Reading::Count(
                        result
                            .items
                            .iter()
                            .filter(|item| lifecycle::is_busy(item.state, TransferRow::is_paused(item)))
                            .count(),
                    ),
                    // 引擎还没起 / 这一批没了：确实没有进行中的活。
                    TransferPoll::Absorbed | TransferPoll::NoDelivery => lifecycle::Reading::Count(0),
                    TransferPoll::Failed(CallFailure::KernelGone { .. }) => lifecycle::Reading::Gone,
                    TransferPoll::Failed(_) => lifecycle::Reading::Miss,
                },
            };
            let (want, unfinished) = tracker.observe(reading);
            // 该状态要**反复设置**才稳（线程状态可能被别的调用冲掉），所以每拍都设。
            set_system_required(want);
            if let Some(n) = unfinished {
                last = n;
            }
            on_count(last);
        }
    });
}
