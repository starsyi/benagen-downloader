//! procstat —— 引擎进程的资源读数，用来回答「它卡住的时候到底在干什么」（规格 2026-10-12 §4）。
//!
//! 现场的症状是 aria2 长时间不应答（45~142 秒，另有一次约 52 分钟）。日志里只有
//! 「10 秒超时」，分不出这三种完全不同的情形：
//!   · **CPU 增量大**：事件循环在空转 / 在算什么；
//!   · **IO 字节增量大**：它在忙着读写磁盘（补零、杀软扫描拖慢的写）；
//!   · **两者都≈0**：它被阻塞在某个系统调用上（等锁、等磁盘、被挂起）。
//! 每一种指向的修法都不一样，所以在判「断开」的那一刻采一次样，与上一次做差，写进诊断日志。
//!
//! ⚠️ 只有 **Windows** 有真实读数（这个症状只在 Windows 上报过，且 API 是 Windows 专用的）。
//!    其他平台 [`read`] 恒返回 `Err`，调用方据此**不写**这条日志——不编一个假读数。
//! ⚠️ 做差与成句是纯函数（[`delta`]），在任何平台上都能被单测；真实读数那一层只有薄薄一圈 FFI。

/// 一次采样的累计读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    /// 内核态 + 用户态累计 CPU 时间（毫秒）。
    pub cpu_ms: u64,
    /// 累计读 / 写的字节数。
    pub io_read: u64,
    pub io_write: u64,
    /// 进程当前持有的句柄数。
    pub handles: u32,
}

/// 与上一次采样的差（第一次采样没有「上一次」⇒ 增量记 0，并标 `first`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delta {
    pub cpu_ms: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub handles: u32,
    pub first: bool,
}

/// 做差。**饱和减**：进程被换掉之后计数器从头再来，`cur < prev` 时不能下溢出一个天文数字。
pub fn delta(prev: Option<&Counters>, cur: &Counters) -> Delta {
    match prev {
        None => Delta {
            cpu_ms: 0,
            io_read: 0,
            io_write: 0,
            handles: cur.handles,
            first: true,
        },
        Some(p) => Delta {
            cpu_ms: cur.cpu_ms.saturating_sub(p.cpu_ms),
            io_read: cur.io_read.saturating_sub(p.io_read),
            io_write: cur.io_write.saturating_sub(p.io_write),
            handles: cur.handles,
            first: false,
        },
    }
}

/// 一次资源读数是否说明**进程在干活**（写盘 或 烧 CPU）。
///
/// 这是"RPC 不回话 ≠ 引擎死了"那条判据的**唯一实现**：
///   * 真死掉的进程不再有任何读写、CPU 也不再增长；
///   * 正在写盘的进程 IO 计数每秒都在涨，哪怕它的 RPC 线程被写盘卡住。
///
/// ⚠️ **第一次采样必须回答"不知道"（`None`）**：没有基线时增量恒为 0，若把它读成"闲"
///    就会在引擎刚起来、还没写第一块的那一拍判它死 —— 那正是最不该重启的时刻。
pub fn is_busy(d: &Delta) -> Option<bool> {
    if d.first {
        None
    } else {
        Some(d.io_write > 0 || d.cpu_ms > 0)
    }
}

/// 采一次样并回答"这个进程在干活吗"（`prev` 由调用方持有，每拍更新）。
///
/// 读不到（非 Windows / 进程已退出 / 权限不足）⇒ `None`：**不知道**，
/// 调用方必须退回旧口径（按"没在干活"处理），不得把它当成"闲"。
pub fn busy(pid: Option<u32>, prev: &mut Option<Counters>) -> Option<bool> {
    let pid = pid?;
    let cur = read(pid).ok()?;
    let d = delta(prev.as_ref(), &cur);
    *prev = Some(cur);
    is_busy(&d)
}

/// 读 `pid` 的累计读数。非 Windows 平台恒为 `Err("unsupported")`。
#[cfg(not(windows))]
pub fn read(_pid: u32) -> Result<Counters, String> {
    Err("unsupported".to_string())
}

/// 读 `pid` 的累计读数（`GetProcessTimes` / `GetProcessIoCounters` / `GetProcessHandleCount`）。
///
/// 与 `random_secret` / `free_bytes` 同一条手法：`#[link]` + 裸 `extern "system"`，不引入 crate。
/// 只要 `PROCESS_QUERY_LIMITED_INFORMATION`——对同一用户的子进程总是拿得到。
#[cfg(windows)]
pub fn read(pid: u32) -> Result<Counters, String> {
    use core::ffi::c_void;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_ops: u64,
        write_ops: u64,
        other_ops: u64,
        read_bytes: u64,
        write_bytes: u64,
        other_bytes: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn GetProcessTimes(
            h: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GetProcessIoCounters(h: *mut c_void, io: *mut IoCounters) -> i32;
        fn GetProcessHandleCount(h: *mut c_void, count: *mut u32) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let ticks = |t: FileTime| ((t.high as u64) << 32) | t.low as u64;

    // SAFETY：每个出参都是本函数栈上的值，指针有效且可写；句柄在所有路径上都被关掉。
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return Err(format!("OpenProcess 失败：{}", std::io::Error::last_os_error()));
        }
        let (mut c, mut e, mut k, mut u) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        let mut io = IoCounters::default();
        let mut handles = 0u32;
        let ok_times = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0;
        let ok_io = GetProcessIoCounters(h, &mut io) != 0;
        let ok_handles = GetProcessHandleCount(h, &mut handles) != 0;
        let last = std::io::Error::last_os_error();
        CloseHandle(h);
        if !(ok_times && ok_io && ok_handles) {
            return Err(format!("读取进程读数失败：{last}"));
        }
        // FILETIME 的单位是 100 纳秒 ⇒ 10_000 刻 = 1 毫秒。
        Ok(Counters {
            cpu_ms: (ticks(k) + ticks(u)) / 10_000,
            io_read: io.read_bytes,
            io_write: io.write_bytes,
            handles,
        })
    }
}

/// 采一次样并写 `engine_stall_sample`。`prev` 是上一次的读数（调用方持有，用完更新）。
///
/// 读不到（非 Windows / 进程已没了 / 权限不够）⇒ 平台不支持时**什么都不写**，
/// 其余失败写一条带 `err` 的 `engine_stall_sample`——「采不到」本身也是证据。
/// `when` 说明是哪一刻采的（`disconnected` / `before_restart`），好把两次读数对上号。
pub fn log_sample(pid: Option<u32>, prev: &mut Option<Counters>, when: &str) {
    let Some(pid) = pid else { return };
    match read(pid) {
        Ok(cur) => {
            let d = delta(prev.as_ref(), &cur);
            *prev = Some(cur);
            crate::diagnostics::log(
                "engine_stall_sample",
                &[
                    ("when", when.to_string()),
                    ("first", d.first.to_string()),
                    ("cpu_ms_delta", d.cpu_ms.to_string()),
                    ("io_read_delta", d.io_read.to_string()),
                    ("io_write_delta", d.io_write.to_string()),
                    ("handles", d.handles.to_string()),
                ],
            );
        }
        Err(e) if e == "unsupported" => {}
        Err(e) => crate::diagnostics::log(
            "engine_stall_sample",
            &[("when", when.to_string()), ("err", e)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(cpu: u64, r: u64, w: u64, h: u32) -> Counters {
        Counters { cpu_ms: cpu, io_read: r, io_write: w, handles: h }
    }

    #[test]
    fn the_first_sample_has_no_baseline() {
        let d = delta(None, &c(5000, 9, 9, 120));
        assert!(d.first);
        assert_eq!((d.cpu_ms, d.io_read, d.io_write, d.handles), (0, 0, 0, 120));
    }

    #[test]
    fn later_samples_report_increments() {
        let d = delta(Some(&c(1000, 100, 200, 90)), &c(1700, 150, 260, 95));
        assert!(!d.first);
        assert_eq!((d.cpu_ms, d.io_read, d.io_write, d.handles), (700, 50, 60, 95));
    }

    /// 引擎被换掉后计数器重新开始：差必须饱和到 0，而不是下溢成 ~1.8e19。
    /// 判别力：把 `saturating_sub` 换成 `-` ⇒ debug 构建 panic / release 回绕 ⇒ 红。
    #[test]
    fn a_counter_reset_saturates_to_zero() {
        let d = delta(Some(&c(9000, 900, 900, 50)), &c(10, 1, 2, 30));
        assert_eq!((d.cpu_ms, d.io_read, d.io_write), (0, 0, 0));
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_there_is_no_reading() {
        assert_eq!(read(1).unwrap_err(), "unsupported");
    }

    /// 首拍没有基线 ⇒ **不知道**（不是"闲"）。
    /// 判别力：把 `d.first` 那一支改成 `Some(false)` ⇒ 红。
    #[test]
    fn the_first_sample_answers_unknown_not_idle() {
        assert_eq!(is_busy(&delta(None, &c(5000, 9, 9, 120))), None);
    }

    /// 写盘算在干活；只有读也算（引擎可能正在读缓存/校验）。
    /// 判别力：把判据写成 `d.io_write > 0` ⇒ 第二个断言红。
    #[test]
    fn writing_or_burning_cpu_counts_as_busy() {
        assert_eq!(is_busy(&delta(Some(&c(1000, 0, 0, 90)), &c(1000, 0, 65536, 90))), Some(true));
        assert_eq!(is_busy(&delta(Some(&c(1000, 0, 0, 90)), &c(1200, 0, 0, 90))), Some(true));
    }

    /// 一点都不动才是"闲"——真死掉的引擎就是这个样子。
    /// ⚠️ 句柄数**不算**在动：进程死了句柄也会掉，那不是"干活"。
    /// 判别力：把 `handles` 加进判据 ⇒ 这个用例红。
    #[test]
    fn an_idle_process_is_the_only_thing_called_dead() {
        assert_eq!(is_busy(&delta(Some(&c(1000, 5, 5, 90)), &c(1000, 5, 5, 87))), Some(false));
    }

    /// 读不到进程（已退出 / 非 Windows）⇒ `None`，调用方退回旧口径。
    #[test]
    fn no_reading_answers_unknown() {
        let mut prev = None;
        assert_eq!(busy(None, &mut prev), None);
        #[cfg(not(windows))]
        assert_eq!(busy(Some(1), &mut prev), None);
    }

    /// 非 Windows 上不写任何日志；这里只验证它不 panic、不更新基线。
    #[cfg(not(windows))]
    #[test]
    fn off_windows_logging_a_sample_is_a_silent_noop() {
        let mut prev = None;
        log_sample(Some(1), &mut prev, "disconnected");
        assert!(prev.is_none());
    }
}
