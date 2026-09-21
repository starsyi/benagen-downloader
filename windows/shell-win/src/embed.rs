//! `embed` —— **内嵌的内核 exe**（规格 §7.2；W-4：客户只拿一个 exe）。
//!
//! 这个文件只有两件事：
//!   1. **内嵌的那份字节是什么**（`include_bytes!` + 编译期常量）；
//!   2. **把它释放到哪儿**（缓存目录）—— 机制本身在 `shell_core::embedded_core`，
//!      那里有单测（规格 §4.3：能写出断言的代码一律搬出平台目标）。
//!
//! ## ⚠️⚠️ 构建顺序是硬约束（本文件是那个约束的落点）
//!
//! 这里内嵌的是**已经编好的** `benagen-core.exe`：
//!
//! ```text
//! core/target/x86_64-pc-windows-gnu/release/benagen-core.exe
//!        │  （shell-win/build.rs 读它、断言 PE、拷进 OUT_DIR、记下 sha256）
//!        ▼
//! $OUT_DIR/benagen-core.exe  ──include_bytes!──▶  本文件
//! ```
//!
//! **cargo 自己不知道这层依赖**（它看见的只是 `OUT_DIR` 里那份副本）⇒ 必须先编内核、
//! 再编壳。**排顺序是 `windows/scripts/build_windows.sh` 的活**，而
//! `shell-win/build.rs` 负责"内核不在场就大声失败 + 把 sha256 记成常量"。
//!
//! ⚠️ **不许用"再编一遍、比哈希/比大小"做新鲜度判据**（实测，任务 5 的审查）：
//!    内核 exe 的 sha256 **不可复现**（PE COFF 头每次写链接时刻），**大小也不是指纹**
//!    （同一份源码在两处编出差 98 字节）。任何那样的判据都会**永远红**。
//!    能比的只有**同一份产物文件**：`build.rs` 比的正是"`core/target/…` 里那份原件"
//!    与 "`OUT_DIR` 里那份副本"，而 `build_windows.sh` 比的是"内嵌进 exe 的 sha256 常量"
//!    与"内核 exe 文件的 sha256"。
//!
//! ## ⚠️ 只有 Windows 靶有"内嵌"这一说
//!
//! `CORE_BIN` 被 `#[cfg(target_os = "windows")]` 关着，因为 `build.rs` **只在 Windows 靶**
//! 才产出那份副本（宿主上 `cargo test -p shell-win` 不该被要求先备好一个 Windows 内核）。
//! 这不是"静默降级"：宿主的产物**本来就不是交付形态**，它不进任何一条分发路径。
//! 而**交付形态那一条（Windows 靶）没有开关**：`build.rs` 一定做那两个检查，
//! `locate_core_binary` 一定先走释放这条路。

// ⚠️ 只在 Windows 靶上需要（下面那个 `extract_core` 的返回类型）；宿主上 import 了会
//    多一条 `unused_imports` 告警 —— 而本 workspace 是**零告警**口径。
#[cfg(target_os = "windows")]
use std::path::PathBuf;

/// 内嵌的内核 exe 的字节。**Windows 靶专有**（理由见文件头）。
///
/// ⚠️ 路径写的是 `OUT_DIR` 里**构建脚本拷进来的副本**，不是 `core/target/…` 里的原件：
///    直接 `include_bytes!("../../core/target/…")` 也能编过，但那会让"内核不在场"
///    从**构建期错误**变成"读到上一次的旧文件"（cargo 不会因此重编本 crate），
///    而那正是本任务要防的那件事（macOS 侧栽过同形的坑：内核改了、端到端零覆盖、
///    套件照样全绿）。
#[cfg(target_os = "windows")]
pub const CORE_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/benagen-core.exe"));

/// 内嵌的那份内核的 sha256（64 位小写十六进制）——**由 `build.rs` 在拷贝那一刻算出来**。
///
/// ⚠️ 它同时是两件事的判据：
///   * 运行期：缓存目录里那份文件**必须**与这个值相符才复用（[`extract_core`]）；
///   * 构建期：`build_windows.sh` 拿它核对"包里那份内核确实是 `core/target/…` 里这一份"
///     —— 那是本机（macOS）**唯一**能证明"内嵌链路没接错、没嵌到旧货"的证据。
#[cfg(target_os = "windows")]
pub const CORE_SHA256: &str = env!("BENAGEN_CORE_SHA256");

/// 把内嵌的内核释放到 `%LOCALAPPDATA%\BenagenDownloader\cache\`，返回它的路径。
///
/// 机制（幂等、原子换位、私有临时名、落地后再核对）在
/// `shell_core::embedded_core::extract_into` —— **那里有单测**，本函数只负责
/// "字节从哪来"（[`CORE_BIN`]）、"哈希从哪来"（[`CORE_SHA256`]：编译期常量，
/// 于是运行期**不必**把十几 MB 的内嵌字节再哈希一遍）与"**释放的是什么**"
/// （`BENAGEN_CORE`：文件名形状 + 话术里指名道姓的那几处）。
///
/// ⚠️ **`BENAGEN_CORE` 这个规格住在 `shell-core`**（不像 WebView2Loader 那份住在
///    `shell-win/src/wv2.rs`）：它描述的就是本模块释放的这件东西，参数化之前那几处
///    字符串**本来就写在 `embedded_core.rs` 里** —— 搬出来只会让这次重构多一次
///    "哪些字变了"的疑问（话术是复审判过的，逐字不动）。
///
/// 失败时返回一句**能直接显示给用户的话**（W-2：磁盘满 / 权限不足 / 被杀软拦截
/// 三档分开说，规格 §10 要求可执行的补救）。
#[cfg(target_os = "windows")]
pub fn extract_core() -> Result<PathBuf, String> {
    let cache_dir = shell_core::embedded_core::cache_dir_from(
        &shell_core::embedded_core::BENAGEN_CORE,
        std::env::var_os("LOCALAPPDATA"),
    )?;
    shell_core::embedded_core::extract_into(
        &shell_core::embedded_core::BENAGEN_CORE,
        CORE_BIN,
        CORE_SHA256,
        &cache_dir,
    )
}

// ## ⚠️ 本文件**没有** `#[test]`（刻意的，理由在下面）
//
// 这里没有一条**可断言**的逻辑：三个 `const` 是编译期常量，`extract_core` 是
// "取常量 + 转调"两行。能写出断言的那部分（命名、复用判据、并发、失败三档）
// 全在 `shell_core::embedded_core`，那里有一整套宿主上真跑的用例 —— 这正是
// 规格 §4.3 那条「能写出断言的代码一律搬出平台目标」的落点。
//
// 剩下那两件事在**本机**（macOS）确实测不了，它们的证据在别处，别处也算数：
//   * **"内嵌的确实是内核"**：`shell-win/build.rs` 在拷贝那一刻断言 PE 头/架构/子系统，
//     并把 sha256 记成常量；`windows/scripts/build_windows.sh` 再拿它跟
//     `core/target/…` 里那份内核 exe 的 sha256 对一次（本机能拿到的最强证据）；
//   * **"释放到真 Windows 上成不成"**：只有真机能判（规格 §11.1）—— 不许在本机
//     声称"Windows 上跑过了"。
