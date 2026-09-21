//! 把内嵌资源从 Go 侧的对口目录拷进本 crate 的 `assets/`。
//!
//! **为什么要有这一步**：aria2c 二进制与 GPLv2 全文的**唯一真相**在
//! `downloader/internal/engine/assets/`——那边 `go:embed` 要求它们真实存在于包目录里。
//! Rust 侧要用 `include_bytes!` 内嵌**同一份字节**，但不该
//! `include_bytes!("../../downloader/internal/engine/assets/…")`：那会让两个 crate 的
//! 源码目录耦死，动一下目录结构就编译不过（任务 10 简报也点了这件事）。
//! 折中：构建时拷一份到本 crate 的 `assets/`，与 Go 侧逐字节相同。
//!
//! 两条性质：
//!   - **幂等**：只在内容不同时才写盘，因此不会让 cargo 反复重建；
//!   - **可缺省**：源目录不在场（只 checkout 了 `core/`）时保持原样、不报错——
//!     `assets/` 里的副本是**入库**的，正常构建本来就不需要源目录。

use std::path::Path;

/// 与 `downloader/internal/engine/assets/` 逐字对应。
///
/// ⚠️ **每个平台的 aria2c 都要在这里**：本表是"哪些文件算入库资产"的清单，
/// 漏一个的表现是——那份资产只在 `core/assets/` 里有副本、一旦有人从 `downloader/`
/// 侧重新生成就**不会同步过去**（而 `core/src/engine/daemon.rs` 的 `include_bytes!`
/// 指的正是 `core/assets/` 这一份）。每份资产都必须同时在场：
/// 少了哪个，那个平台的内核就直接编译失败（`include_bytes!` 找不到文件）。
const ASSETS: [&str; 4] = [
    "aria2c-macos-arm64",
    "aria2c-macos-x86_64",
    "aria2c-linux-x86_64",
    "COPYING-GPLv2.txt",
];

/// 源目录：相对本 crate 的根（`core/`）。
const SRC: &str = "../downloader/internal/engine/assets";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let src = Path::new(SRC);
    if !src.is_dir() {
        // 源不在场：`assets/` 里已入库的副本就是真相，什么都不用做。
        return;
    }
    // 只在源在场时才声明依赖，否则 cargo 会因为它"缺失"而每次重建。
    println!("cargo:rerun-if-changed={SRC}");

    let dst = Path::new("assets");
    if std::fs::create_dir_all(dst).is_err() {
        return;
    }
    for name in ASSETS {
        let (from, to) = (src.join(name), dst.join(name));
        let Ok(bytes) = std::fs::read(&from) else {
            continue;
        };
        if std::fs::read(&to).ok().as_deref() == Some(bytes.as_slice()) {
            continue; // 逐字节相同：不碰它（碰了就会改 mtime，可能触发无谓的重建）
        }
        if std::fs::write(&to, &bytes).is_err() {
            continue;
        }
        // 可执行位与 Go 侧一致（`extract_to` 释放时还会再设一次 0o755，这里是给仓库里的副本对齐）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if name.ends_with(".txt") { 0o644 } else { 0o755 };
            let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode));
        }
    }
}
