//! migrate —— 换下载目录时，把旧目录里已下载的（半截的 / 已完成的）文件搬到新目录
//! （规格 2026-10-12 §6）。
//!
//! 为什么需要：改下载目录会重启内核，新目录里什么都没有，续传无从谈起；
//! 磁盘快满、要换盘的场景里，从头再下一遍几十 GB 是客户最不能接受的结果。
//!
//! ## 搬运的不变量（承重，每条都有测试）
//!
//! 1. **绝不覆盖**：目标已存在 ⇒ 跳过并记录，不动目标也不动源。
//! 2. **绝不丢数据**：同卷用 `rename`（原子）；跨卷先复制到 `<目标>.benagen-part`，
//!    **长度核对一致后才改名，改名成功后才删源**。任何一步失败，源文件都还在。
//! 3. **控制文件先于主文件**：aria2 的断点信息在 `<文件>.aria2` 里（多连接分段下载时，
//!    主文件是稀疏的，只有控制文件知道哪些片段是真的）。若先搬主文件、中途失败，
//!    新目录里会出现「有数据没控制文件」——aria2 会把稀疏的洞当成已下载，**静默损坏**。
//!    先搬控制文件的话，最坏是「有控制文件没主文件」，aria2 会老老实实从头下。
//! 4. **路径一律过 [`crate::engine::safe_rel_path`]**（越界 / 控制字符 / 空段一律拒绝）。
//!
//! 搬运跑在后台线程里（跨卷复制几十 GB 要几分钟到几十分钟），壳轮询 [`Status`]。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 控制文件后缀（aria2 的断点信息）。
const CONTROL_SUFFIX: &str = ".aria2";
/// 跨卷复制时的临时后缀。
const PART_SUFFIX: &str = ".benagen-part";
/// 复制的块大小。
const CHUNK: usize = 1 << 20;

/// 一条逐文件的问题（跳过 / 失败）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub reason: String,
}

/// 壳轮询的状态。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Status {
    /// 后台线程还在跑。
    pub running: bool,
    /// 至少完整跑过一轮（`running == false && finished` ⇒ 可以开始下载了）。
    pub finished: bool,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// 目标已存在 / 源不是普通文件等「不需要搬」的。
    pub skipped: Vec<Problem>,
    /// 搬失败的（源文件仍在原处），以及「已到新目录、但删旧文件失败」的（旧文件需手动删）。
    pub errors: Vec<Problem>,
}

/// 一个要搬的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub rel: String,
    /// 主文件 + 控制文件的字节数（进度条的分母）。
    pub bytes: u64,
}

/// 扫描：在 `wanted`（清单相对路径）里找出「旧目录里有、新目录里没有」的。
pub fn scan(from: &Path, to: &Path, wanted: &[String]) -> (Vec<Item>, Vec<Problem>) {
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for rel in wanted {
        if !crate::engine::safe_rel_path(rel) {
            skipped.push(Problem {
                path: rel.clone(),
                reason: "路径不安全（越界/控制字符/空段）".to_string(),
            });
            continue;
        }
        let src = from.join(rel);
        let Ok(meta) = std::fs::metadata(&src) else {
            continue; // 旧目录里没有这个文件：不是问题，只是不用搬
        };
        if !meta.is_file() {
            continue;
        }
        if to.join(rel).exists() {
            skipped.push(Problem {
                path: rel.clone(),
                reason: "新目录里已有同名文件，未覆盖".to_string(),
            });
            continue;
        }
        let ctl = std::fs::metadata(with_suffix(&src, CONTROL_SUFFIX))
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .unwrap_or(0);
        items.push(Item {
            rel: rel.clone(),
            bytes: meta.len() + ctl,
        });
    }
    (items, skipped)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// 搬一个文件（含它的控制文件）。`on_bytes` 每搬完一块回调一次增量。
/// 返回的是「已搬到目标、但有残留」的提示（删旧文件失败）；`Err` 表示没有落到目标。
/// `force_copy` 只给测试用：强制走跨卷的复制路径。
pub fn move_item(
    from: &Path,
    to: &Path,
    item: &Item,
    force_copy: bool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<Vec<String>, String> {
    let src = from.join(&item.rel);
    let dst = to.join(&item.rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let src_ctl = with_suffix(&src, CONTROL_SUFFIX);
    let dst_ctl = with_suffix(&dst, CONTROL_SUFFIX);
    let mut residue = Vec::new();

    // ① 控制文件先搬（不变量 3）。没有控制文件是常态（已完成的文件没有）。
    let moved_ctl = if src_ctl.is_file() {
        if dst_ctl.exists() {
            return Err("新目录里已有同名控制文件，未覆盖".to_string());
        }
        residue.extend(move_file(&src_ctl, &dst_ctl, force_copy, on_bytes)?);
        true
    } else {
        false
    };
    // ② 再搬主文件。主文件没能落到目标 ⇒ 把已搬走的控制文件搬回源，保证源这一侧仍是一对。
    //    主文件已落到目标（哪怕只是删源失败）则不回滚，目标这一侧必须是完整的一对。
    match move_file(&src, &dst, force_copy, on_bytes) {
        Ok(r) => residue.extend(r),
        Err(e) => {
            if moved_ctl {
                if let Err(re) = move_file(&dst_ctl, &src_ctl, force_copy, &mut |_| {}) {
                    return Err(format!("{e}；控制文件回滚也失败：{re}"));
                }
            }
            return Err(e);
        }
    }
    Ok(residue)
}

/// 搬单个文件：同卷 `rename`，否则「复制 → 核对长度 → 改名 → 删源」。
///
/// `Err` ⇒ 没有落到目标，源原封不动。`Ok(Some(_))` ⇒ 已落到目标，只是删源失败、源残留。
fn move_file(
    src: &Path,
    dst: &Path,
    force_copy: bool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<Option<String>, String> {
    if dst.exists() {
        return Err(format!("目标已存在，未覆盖：{}", dst.display()));
    }
    let len = std::fs::metadata(src)
        .map_err(|e| format!("读取源文件失败：{e}"))?
        .len();
    if !force_copy && std::fs::rename(src, dst).is_ok() {
        on_bytes(len);
        return Ok(None);
    }

    // 复制路径。任何一步失败都清掉临时文件，源原封不动。
    let part = with_suffix(dst, PART_SUFFIX);
    let result = (|| -> Result<(), String> {
        let mut r = std::fs::File::open(src).map_err(|e| format!("打开源文件失败：{e}"))?;
        let mut w = std::fs::File::create(&part).map_err(|e| format!("创建目标失败：{e}"))?;
        let mut buf = vec![0u8; CHUNK];
        let mut copied: u64 = 0;
        loop {
            let n = r.read(&mut buf).map_err(|e| format!("读取源文件失败：{e}"))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).map_err(|e| format!("写入目标失败：{e}"))?;
            copied += n as u64;
            on_bytes(n as u64);
        }
        w.sync_all().map_err(|e| format!("落盘失败：{e}"))?;
        drop(w);
        // 不变量 2：长度核对一致才算数（源在搬运期间被改了也会在这里露出来）。
        let src_len_now = std::fs::metadata(src).map(|m| m.len()).unwrap_or(len);
        if copied != src_len_now {
            return Err(format!("复制后长度不一致（复制 {copied}，源 {src_len_now}）"));
        }
        std::fs::rename(&part, dst).map_err(|e| format!("改名失败：{e}"))?;
        Ok(())
    })();
    match result {
        // 目标已就位：删源失败只是留了一份多余的旧文件，报出来，但目标不回滚。
        Ok(()) => Ok(std::fs::remove_file(src).err().map(|e| {
            format!("已复制到新目录，但删除旧文件失败（可手动删除）：{}：{e}", src.display())
        })),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// 同步地跑完整个搬运，把进度写进 `status`。后台线程与测试共用这一个函数。
pub fn run(
    from: &Path,
    to: &Path,
    wanted: &[String],
    force_copy: bool,
    status: &std::sync::Mutex<Status>,
) {
    let (items, skipped) = scan(from, to, wanted);
    {
        let mut s = lock(status);
        s.files_total = items.len();
        s.bytes_total = items.iter().map(|i| i.bytes).sum();
        s.skipped = skipped;
    }
    for item in &items {
        let r = move_item(from, to, item, force_copy, &mut |n| {
            lock(status).bytes_done += n;
        });
        let mut s = lock(status);
        s.files_done += 1;
        match r {
            Ok(residue) => s.errors.extend(residue.into_iter().map(|reason| Problem {
                path: item.rel.clone(),
                reason,
            })),
            Err(reason) => s.errors.push(Problem {
                path: item.rel.clone(),
                reason,
            }),
        }
    }
    let mut s = lock(status);
    s.running = false;
    s.finished = true;
}

fn lock(m: &std::sync::Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::sync::Mutex;

    fn put(dir: &Path, rel: &str, body: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn rels(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn fresh() -> (TempDir, PathBuf, PathBuf) {
        let t = TempDir::new();
        let (a, b) = (t.join("old"), t.join("new"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        (t, a, b)
    }

    #[test]
    fn moves_a_partial_file_together_with_its_control_file() {
        let (_t, a, b) = fresh();
        put(&a, "x/p.bin", b"partial");
        put(&a, "x/p.bin.aria2", b"ctl");
        let st = Mutex::new(Status { running: true, ..Default::default() });

        run(&a, &b, &rels(&["x/p.bin"]), false, &st);

        let s = lock(&st).clone();
        assert!(s.finished && !s.running && s.errors.is_empty(), "{s:?}");
        assert_eq!(std::fs::read(b.join("x/p.bin")).unwrap(), b"partial");
        assert_eq!(std::fs::read(b.join("x/p.bin.aria2")).unwrap(), b"ctl");
        assert!(!a.join("x/p.bin").exists() && !a.join("x/p.bin.aria2").exists());
        assert_eq!((s.files_done, s.files_total), (1, 1));
        assert_eq!(s.bytes_done, s.bytes_total);
    }

    /// 跨卷路径（强制复制）：结果与同卷一致，且不留 `.benagen-part`。
    #[test]
    fn the_copy_path_gives_the_same_result_and_leaves_no_temp_file() {
        let (_t, a, b) = fresh();
        put(&a, "p.bin", &vec![7u8; CHUNK + 5]);
        put(&a, "p.bin.aria2", b"ctl");
        let st = Mutex::new(Status::default());

        run(&a, &b, &rels(&["p.bin"]), true, &st);

        let s = lock(&st).clone();
        assert!(s.errors.is_empty(), "{s:?}");
        assert_eq!(std::fs::metadata(b.join("p.bin")).unwrap().len(), (CHUNK + 5) as u64);
        assert!(!b.join("p.bin.benagen-part").exists());
        assert!(!a.join("p.bin").exists(), "复制成功后源应被删除");
        assert_eq!(s.bytes_done, (CHUNK + 5 + 3) as u64);
    }

    /// 不变量 1：目标已存在 ⇒ 不覆盖、源原封不动、记入 skipped。
    /// 判别力：去掉 `scan` 里的 `exists()` 判断 ⇒ 目标被覆盖 ⇒ 红。
    #[test]
    fn never_overwrites_an_existing_target() {
        let (_t, a, b) = fresh();
        put(&a, "p.bin", b"old-copy");
        put(&b, "p.bin", b"already-here");
        let st = Mutex::new(Status::default());

        run(&a, &b, &rels(&["p.bin"]), false, &st);

        let s = lock(&st).clone();
        assert_eq!(std::fs::read(b.join("p.bin")).unwrap(), b"already-here");
        assert_eq!(std::fs::read(a.join("p.bin")).unwrap(), b"old-copy");
        assert_eq!(s.skipped.len(), 1, "{s:?}");
        assert_eq!(s.files_total, 0);
    }

    /// 不变量 4：越界路径被拒，且不碰任何文件。
    #[test]
    fn unsafe_paths_are_rejected_without_touching_the_disk() {
        let (t, a, b) = fresh();
        put(t.path(), "secret.txt", b"s");
        let st = Mutex::new(Status::default());

        run(&a, &b, &rels(&["../secret.txt", "/etc/passwd", "a//b"]), false, &st);

        let s = lock(&st).clone();
        assert_eq!(s.skipped.len(), 3, "{s:?}");
        assert!(t.join("secret.txt").exists());
        assert!(!b.join("secret.txt").exists());
    }

    /// 旧目录里没有的文件不是问题（大多数清单文件本来就没下过）。
    #[test]
    fn files_absent_from_the_old_dir_are_silently_ignored() {
        let (_t, a, b) = fresh();
        let st = Mutex::new(Status::default());
        run(&a, &b, &rels(&["nope.bin"]), false, &st);
        let s = lock(&st).clone();
        assert!(s.finished && s.errors.is_empty() && s.skipped.is_empty(), "{s:?}");
        assert_eq!(s.files_total, 0);
    }

    /// 已完成的文件（没有控制文件）也一并搬走。
    #[test]
    fn a_finished_file_without_a_control_file_moves_too() {
        let (_t, a, b) = fresh();
        put(&a, "done.bin", b"all");
        let st = Mutex::new(Status::default());
        run(&a, &b, &rels(&["done.bin"]), false, &st);
        assert_eq!(std::fs::read(b.join("done.bin")).unwrap(), b"all");
        assert!(lock(&st).errors.is_empty());
    }

    /// 不变量 3：主文件搬失败时，控制文件被搬回源，源这一侧仍是自洽的一对。
    /// 造法：让目标主文件的位置被一个**目录**占着 ⇒ 主文件的 rename/复制必败。
    #[test]
    fn a_failed_main_move_puts_the_control_file_back() {
        let (_t, a, b) = fresh();
        put(&a, "p.bin", b"partial");
        put(&a, "p.bin.aria2", b"ctl");
        std::fs::create_dir_all(b.join("p.bin")).unwrap(); // 占位：scan 里 exists() 会先拦住
        // 绕过 scan，直接调 move_item，才能走到「主文件失败」那一支。
        let item = Item { rel: "p.bin".to_string(), bytes: 10 };

        let r = move_item(&a, &b, &item, false, &mut |_| {});

        assert!(r.is_err());
        assert_eq!(std::fs::read(a.join("p.bin")).unwrap(), b"partial", "主文件必须还在源");
        assert_eq!(std::fs::read(a.join("p.bin.aria2")).unwrap(), b"ctl", "控制文件必须被搬回");
        assert!(!b.join("p.bin.aria2").exists());
    }

    /// 删源失败（复制已落盘，旧文件删不掉）：主文件与控制文件都已完整落到新目录，**不回滚**，
    /// 只记入 errors。造法：源目录只读——复制（读源、写目标）能成功，删源需要源目录可写。
    /// 判别力：把删源失败改回 `Err` ⇒ 主文件没搬走、控制文件被搬回 ⇒ 下面的断言红。
    #[cfg(unix)]
    #[test]
    fn a_failed_source_delete_keeps_the_target_pair_and_does_not_roll_back() {
        use std::os::unix::fs::PermissionsExt;
        let (_t, a, b) = fresh();
        put(&a, "p.bin", b"partial");
        put(&a, "p.bin.aria2", b"ctl");
        std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = std::fs::File::create(a.join("probe")).is_ok();
        let st = Mutex::new(Status::default());

        run(&a, &b, &rels(&["p.bin"]), true, &st);

        std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            // root（含构建机 172.20.204.11 上以 root 跑的 Linux 靶套件）无视目录只读位，
            // 「删源失败」这个形态**构造不出来**。大声跳过而不是假红：判据本身由非 root
            // 平台（macOS 套件）钉住，这里的失败只证明「我在 root 下」，不证明代码坏。
            eprintln!("SKIP a_failed_source_delete_keeps_the_target_pair_and_does_not_roll_back:                        root 下造不出删源失败形态（只读目录仍可写）");
            return;
        }
        let s = lock(&st).clone();
        assert_eq!(std::fs::read(b.join("p.bin")).unwrap(), b"partial", "{s:?}");
        assert_eq!(std::fs::read(b.join("p.bin.aria2")).unwrap(), b"ctl", "控制文件不得被回滚");
        assert!(a.join("p.bin").exists(), "删源失败，旧文件应当仍在");
        assert_eq!(s.files_done, 1, "{s:?}");
        assert!(
            s.errors.iter().any(|p| p.path == "p.bin" && p.reason.contains("删除旧文件失败")),
            "{s:?}"
        );
        assert!(!b.join("p.bin.benagen-part").exists());
    }
}
