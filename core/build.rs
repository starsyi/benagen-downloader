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

/// **`core/assets/` 里哪些文件算"入库资产"**——本表是这件事的权威清单。
///
/// ⚠️ **每个受支持 (操作系统, 架构) 的 aria2c 都要在这里**：本表是"`core/assets/` 里哪些
/// 文件算入库资产"的**权威清单**，漏一个的表现是——那份资产只在 `core/assets/`
/// 里有副本、一旦有人从 `downloader/` 侧重新生成就**不会同步过去**（而
/// `core/src/engine/daemon.rs` 的 `include_bytes!` 指的正是 `core/assets/` 这一份）。
/// 各平台的资产必须同时在场：少了哪个，哪个平台的内核就直接编译失败。
///
/// ⚠️ **Windows 那份在 `downloader/internal/engine/assets/` 侧不存在，而且短期内也不会存在**
/// （任务 2 的实测结论，与任务 1 在这段里写下的预期不同，所以那句预期已经改掉）：
/// Windows 交付走的是 **Rust 内核**（`windows/shell-win` 内嵌的是 `core` 编出来的
/// `benagen-core.exe`），Go 那套引擎只服务 macOS（`go:embed` 里只有 arm64 一份 aria2c）。
/// 往 `downloader/` 放一份 5.6 MB 的 Windows PE 没有消费者，只会让仓库白白变重。
/// 后果：这一行对 Windows 资产是**长期惰性**的——`std::fs::read` 读不到就 `continue`，
/// 不报错、也不会把 `core/assets/` 里那份覆盖掉（这是**要**的：那份由
/// `core/scripts/fetch_windows_aria2c.sh` 从官方包落位并自验 sha256，见该脚本的文件头）。
/// 将来 Go 侧真的要用它时，把它放进源目录即可——这条清单会让它同步过去。
///
/// ⚠️ **表里有两类条目，别把它们看成一类**（任务 21 的复审抓的"清单漏项"）：
///   · **上游在 Go 侧有原件**（`aria2c-macos-*`、`aria2c-linux-x86_64`、`COPYING-GPLv2.txt`）：
///     上面那条"copy 过来"的路径对它们生效，本表的作用是"别忘了同步"。
///   · **上游不在 Go 侧**（`aria2c-windows-x86_64.exe`、`COPYING-OFL-1.1.txt`）：
///     前者由 `core/scripts/fetch_windows_aria2c.sh` 落位；后者随**字体**分发，
///     源头是 `windows/assets/`，由 `windows/scripts/make_font_subset.sh` 从官方包里落下来。
///     对它们这一行是**无操作**（源文件不存在 ⇒ 下面的循环 `continue`），列在这里是为了让
///     "`core/assets/` 里都有什么、各自从哪来"只有**一个**地方要看。
///     ⇒ 加它的时候**不要**顺手去 `downloader/` 里造一份副本，那会变成第二份真相。
const ASSETS: [&str; 6] = [
    "aria2c-macos-arm64",
    "aria2c-macos-x86_64",
    "aria2c-linux-x86_64",
    "aria2c-windows-x86_64.exe",
    "COPYING-GPLv2.txt",
    // 界面字体（Noto Sans SC 子集）的 SIL OFL 1.1 全文，与 `windows/assets/OFL-1.1.txt`
    // **逐字节相同**（两边都是官方包内 LICENSE 的原文）。⚠️ 它**不是** GPLv2 那一份：
    // 两份许可随附的是**不同的内嵌组件**（aria2c / 字体），义务各算各的。
    "COPYING-OFL-1.1.txt",
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
