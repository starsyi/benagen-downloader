//! 目标盘的**写入速度**实测（只为那句"盘比网慢"的提示服务）。
//!
//! 做法就一条：往目标目录**真写** 8 MiB，量用时；最多量 2 秒。写不动就按已写量算 ——
//! 于是"极慢的盘"也能得到一个真实的小数字，而不是卡住两分钟。
//!
//! ⚠️ 为什么是**真写**而不是问系统：Windows 上读缓存会把顺序写测成几百 MB/s
//!    （"看着很快、实际很慢"），而那正是要识别的那一类盘。这里写完 `sync_all()`
//!    再算，测的是**落盘**速度。
//! ⚠️ 探针文件写在目标目录里、**立刻删掉**；名字以 `.` 开头且带 `benagen`，
//!    不进任何清单、不参与校验（校验只按 `manifest.json` 里那几条路径找文件）。
//! ⚠️ 任何一步失败都回 `None`（= 不知道）。**不许拿猜的数字提示客户**。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 实测上限：写满这么多就停（8 MiB 在 20 MB/s 的盘上约 0.4 秒）。
const BYTES: u64 = 8 << 20;
/// 最长量多久（再慢的盘也给个下限结论）。
const MAX_SPENT: Duration = Duration::from_secs(2);
/// 探针文件名（以 `.` 开头：即便删除失败，也不会被客户当成交付文件）。
const PROBE: &str = ".benagen-write-probe";

/// 一个进程里每个目录只量一次的结果（目录 → MB/s）。换目录时用 [`fresh`] 重测。
static LAST: Mutex<Option<(PathBuf, Option<u64>)>> = Mutex::new(None);

/// 量（缓存命中就直接回）。同一个目录在一次运行里只写一次盘。
pub fn cached(dir: &Path) -> Option<u64> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    if let Ok(guard) = LAST.lock() {
        if let Some((d, mbps)) = guard.as_ref() {
            if d == dir {
                return *mbps;
            }
        }
    }
    let mbps = measure(dir);
    if let Ok(mut guard) = LAST.lock() {
        *guard = Some((dir.to_path_buf(), mbps));
    }
    mbps
}

/// 强制重测（换下载目录之后用）。
pub fn fresh(dir: &Path) -> Option<u64> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    let mbps = measure(dir);
    if let Ok(mut guard) = LAST.lock() {
        *guard = Some((dir.to_path_buf(), mbps));
    }
    mbps
}

/// 真写一遍。**它会阻塞调用线程**（最多 [`MAX_SPENT`] + 收尾时间）。
pub fn measure(dir: &Path) -> Option<u64> {
    let path = dir.join(PROBE);
    let mut file = std::fs::File::create(&path).ok()?;
    let chunk = vec![0u8; 1 << 20];
    let started = Instant::now();
    let mut written: u64 = 0;
    while written < BYTES {
        if file.write_all(&chunk).is_err() {
            break;
        }
        written += chunk.len() as u64;
        if started.elapsed() >= MAX_SPENT {
            break;
        }
    }
    // `sync_all` 失败（盘被拔掉 / 写保护）⇒ 这一次读数不可信。
    let synced = file.sync_all().is_ok();
    let elapsed = started.elapsed();
    drop(file);
    let _ = std::fs::remove_file(&path);
    if !synced || written == 0 {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some((written as f64 / secs / 1_000_000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真写一遍能拿到一个**正数**（临时目录，跑完自己删）。
    /// 判别力：把 `written == 0` 那一支去掉、或把除数写错 ⇒ 读到 0 / panic ⇒ 红。
    #[test]
    fn a_real_write_yields_a_number() {
        let dir = std::env::temp_dir().join(format!("benagen-diskspeed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let mbps = measure(&dir).expect("临时目录写得动");
        assert!(mbps > 0, "写入速度必须是正数，拿到 {mbps}");
        assert!(
            !dir.join(PROBE).exists(),
            "探针文件必须删掉（客户会看到这个目录）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不存在 ⇒ `None`（不许 panic、不许编一个数字）。
    #[test]
    fn a_missing_dir_is_unknown_not_zero() {
        let dir = std::env::temp_dir().join("benagen-diskspeed-does-not-exist-xyz");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(measure(&dir), None);
    }

    /// 空目录字符串（偏好"未配置"，由内核决定落盘根）⇒ 不去量。
    /// 判别力：删掉那两处 `is_empty` 判断 ⇒ 会去量进程的当前目录、报出一个无关的数。
    #[test]
    fn an_unset_dir_is_not_measured() {
        assert_eq!(cached(Path::new("")), None);
        assert_eq!(fresh(Path::new("")), None);
    }
}
