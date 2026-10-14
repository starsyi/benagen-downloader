//! autoretry —— 失败任务的**自动重试**（规格 2026-10-12 §7）。
//!
//! 现场症状：下载到 78%（6.9 GB）时服务器突然开始 `Connection refused`。aria2 自己的重试
//! 在这里几乎没用：`--retry-wait` 只对 HTTP 503 生效，连接被拒绝时它**不等待**，
//! `--max-tries`（默认 3）次尝试在一秒内烧完，任务落到 `error`，之后就没有人再管它了——
//! 客户得守着屏幕手动点「重试」。服务器的这类抖动通常几十秒到几分钟就过去。
//!
//! 所以在内核里补一层：**瞬时性**的失败（网络 / 超时 / 5xx / 429）按退避间隔自动重新入队
//! （`-c` 续传，已下的字节不丢），有次数上限；**确定性**的失败（404、磁盘满、权限）绝不重试。
//!
//! 本文件是**纯判据**（哪些错该重试、等多久、记几次）；线程与 RPC 在 `kernel.rs`。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// 同一个文件最多自动重试几次。8 次 ≈ 30 + 60 + 120 + 240 + 300×4 ≈ 27 分钟，
/// 覆盖「服务器重启 / 限流窗口 / 运维抖动」，又不至于对一个真坏了的源无限敲门。
pub const MAX_ATTEMPTS: u32 = 8;

/// 第 `attempt`（从 0 数）次重试之前要等多久：30s 起步，翻倍，封顶 300s。
///
/// 起步 30 秒而不是 1 秒：被拒绝说明对端此刻不想接客，立刻再敲只会让限流窗口续期。
pub fn backoff(attempt: u32) -> Duration {
    let secs = 30u64.saturating_mul(1u64 << attempt.min(10));
    Duration::from_secs(secs.min(300))
}

/// aria2 的错误文案是不是**瞬时性**的。
///
/// 为什么按文案而不是 `errorCode`：`tell*` 只取七个键（`errorCode` 不在其中，加键要动 37 处
/// `Task` 字面量与线上夹具）。文案以生产配置下的 RPC 实测为准（见 `transient_table_measured_under_max_tries3_retry_wait1`）；
/// Windows 版 c-ares 的解析失败原因只核对过二进制字符串，未实测。
/// 判据只看 `cause:` 之后的原因：路径、主机名在它之前，里面的 `disk`、`timeout` 之类不影响结果。
/// 认不出的一律当作**不可重试**（宁可让客户手动点，也不对一个确定性错误无限敲门）。
pub fn is_transient(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    // 解析失败：aria2 对「域名不存在」与「断网」给的是同一句，分不开，一律按瞬时处理（次数有上限）。
    if e.contains("failed to resolve the hostname") {
        return true;
    }
    let cause = e.rfind("cause:").map_or(e.as_str(), |i| &e[i + "cause:".len()..]);
    const PERMANENT: &[&str] = &[
        "no space",
        "disk",
        "permission",
        "not found",
        "status=401",
        "status=403",
        "status=404",
        "status=410",
        "checksum",
        "file already exists",
    ];
    if PERMANENT.iter().any(|p| cause.contains(p)) {
        return false;
    }
    const TRANSIENT_PREFIX: &[&str] = &[
        "failed to establish connection",
        "failed to connect to the host",
        "network problem has occurred",
    ];
    if TRANSIENT_PREFIX.iter().any(|t| e.contains(t)) {
        return true;
    }
    // 生产配置下中途断线的 RPC 文案是这一句（见测试）；超时则是 `Timeout.`，已在上表。
    const TRANSIENT_CAUSE: &[&str] = &[
        "connection refused",
        "connection reset",
        "connection timed out",
        "timed out",
        "timeout",
        "no uri available",
        // `--lowest-speed-limit` 触发的报错（格式串 `Too slow Downloading speed: %d <= %d(B/s), host:%s`，
        // 2026-10-13 审查 `strings` 取证）：判成确定性失败的话，「零速率挂死」就永不会被救。
        "too slow downloading speed",
        "could not contact dns servers",
        "status=429",
        "status=500",
        "status=502",
        "status=503",
        "status=504",
    ];
    TRANSIENT_CAUSE.iter().any(|t| cause.contains(t))
}

/// 每个路径的重试记账。
#[derive(Debug, Default)]
pub struct Tracker {
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    attempts: u32,
    due: Instant,
}

impl Tracker {
    /// 这个路径此刻该不该重试？第一次见到它失败时只**登记**到期时间（不立刻重试）。
    pub fn should_retry(&mut self, path: &str, now: Instant, scale: f64) -> bool {
        let entry = self.entries.entry(path.to_string()).or_insert_with(|| Entry {
            attempts: 0,
            due: now + scaled(backoff(0), scale),
        });
        entry.attempts < MAX_ATTEMPTS && now >= entry.due
    }

    /// 记一次已发出的重试，并排下一次的到期时间。
    pub fn record_attempt(&mut self, path: &str, now: Instant, scale: f64) {
        if let Some(e) = self.entries.get_mut(path) {
            e.attempts += 1;
            e.due = now + scaled(backoff(e.attempts), scale);
        }
    }

    /// 已经不在失败态的路径（重试成功 / 完成 / 被移除）：忘掉它的记账。
    pub fn retain_failing(&mut self, failing: &std::collections::BTreeSet<String>) {
        self.entries.retain(|p, _| failing.contains(p));
    }

    pub fn attempts(&self, path: &str) -> u32 {
        self.entries.get(path).map_or(0, |e| e.attempts)
    }
}

/// 把退避按比例缩放（生产恒 1.0；测试用 0 让间隔归零）。
fn scaled(d: Duration, scale: f64) -> Duration {
    Duration::from_secs_f64(d.as_secs_f64() * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn backoff_doubles_from_thirty_seconds_and_caps_at_five_minutes() {
        let secs: Vec<u64> = (0..6).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, vec![30, 60, 120, 240, 300, 300]);
        // 巨大的次数也不溢出。
        assert_eq!(backoff(u32::MAX).as_secs(), 300);
    }

    /// 生产配置下实测的 RPC `errorMessage`。取证口径：内嵌 aria2 1.37.0（`core/assets/aria2c-macos-arm64`），
    /// `addUri` 下发与生产 `per_task_options` 相同的 `max-tries=3`、`retry-wait=1`，其余取 aria2 默认（超时 60 秒）。
    /// 注意：`max-tries=1`、`retry-wait=0`、`--timeout=2` 下量得的文案与此不同，不得混用。
    /// 判别力：删掉 `TRANSIENT_CAUSE` 里的 `no uri available` ⇒ 中途断线这一行变红。
    #[test]
    fn transient_table_measured_under_max_tries3_retry_wait1() {
        for (msg, want) in [
            ("Failed to establish connection, cause: Connection refused", true),
            ("Failed to resolve the hostname no-such-host.benagen-invalid.example, cause: nodename nor servname provided, or not known", true),
            ("The response status is not successful. status=503", true),
            ("Resource not found", false),
            ("Timeout.", true),
            ("No URI available.", true),
        ] {
            assert_eq!(is_transient(msg), want, "{msg:?}");
        }
    }

    /// 未经实测、用于钉住判据规则的合成用例：文案按 aria2 的模板构造，**不是**线上见过的串。
    /// 判别力：`cause:` 之前的路径与主机名不得影响结果（`Read-only file system` 那行含路径里的 `timeout`，
    /// 若整句参与匹配会误判为瞬时）；把 `PERMANENT` 的排除删掉 ⇒ 权限、磁盘用例变红。
    #[test]
    fn synthetic_cases_pin_the_classifier_rules() {
        for (msg, want) in [
            ("Failed to resolve the hostname disk-cdn.example.com, cause: nodename nor servname provided, or not known", true),
            ("Failed to open the file /Volumes/disk1/x.bin, cause: Permission denied", false),
            ("Failed to write into the file /data/disk1/x.bin, cause: No space left on device", false),
            ("Failed to open the file /srv/timeout-test/x.bin, cause: Read-only file system", false),
            ("Failed to connect to the host mirror.example:443, cause: Connection refused", true),
            ("Network problem has occurred.", true),
            ("The response status is not successful. status=429", true),
            ("The response status is not successful. status=404", false),
            ("Permission denied", false),
            ("", false),
            ("某个从没见过的错误", false),
            // `--lowest-speed-limit` 触发时的报错原文，取自内嵌 aria2 的格式串
            // `Too slow Downloading speed: %d <= %d(B/s), host:%s`（2026-10-13 审查 `strings` 取证）。
            // 它必须是**瞬时**的：正是「零速率挂死」被这条门槛拦下的形态，判成确定性失败等于死路。
            // 判别力：删掉 `TRANSIENT_CAUSE` 里的 `too slow downloading speed` ⇒ 这行变红。
            ("Too slow Downloading speed: 0 <= 1024(B/s), host:cdn.example.com", true),
        ] {
            assert_eq!(is_transient(msg), want, "{msg:?}");
        }
    }

    #[test]
    fn the_first_failure_only_registers_it_and_waits_out_the_backoff() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        assert!(!t.should_retry("a", t0, 1.0), "刚失败不该立刻重试");
        assert!(!t.should_retry("a", t0 + Duration::from_secs(29), 1.0));
        assert!(t.should_retry("a", t0 + Duration::from_secs(31), 1.0));
    }

    /// 次数上限：到顶后永远 false。判据：把 `<` 改成 `<=` ⇒ 多重试一次 ⇒ 红。
    #[test]
    fn stops_after_the_attempt_cap() {
        let mut t = Tracker::default();
        let mut now = Instant::now();
        t.should_retry("a", now, 0.0); // 登记
        let mut sent = 0;
        for _ in 0..(MAX_ATTEMPTS + 5) {
            now += Duration::from_secs(1);
            if t.should_retry("a", now, 0.0) {
                t.record_attempt("a", now, 0.0);
                sent += 1;
            }
        }
        assert_eq!(sent, MAX_ATTEMPTS);
        assert_eq!(t.attempts("a"), MAX_ATTEMPTS);
    }

    #[test]
    fn recovered_paths_are_forgotten() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.should_retry("a", now, 0.0);
        t.should_retry("b", now, 0.0);
        t.record_attempt("a", now, 0.0);
        t.retain_failing(&BTreeSet::from(["b".to_string()]));
        assert_eq!(t.attempts("a"), 0, "a 恢复了，记账清零");
    }
}
