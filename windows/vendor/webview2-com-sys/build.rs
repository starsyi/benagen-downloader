//! 补丁版 `build.rs` —— 与 crates.io 上 0.38.2 那份**不是同一个东西**（详见 `src/lib.rs` 的头部）。
//!
//! ## 原版做什么、为什么必须整个去掉
//!
//! 原版 build.rs 会把 `x64/`、`x86/`、`arm64/` 三份 **MSVC 静态库**
//! （`WebView2LoaderStatic.lib`，x64 那一份就有 10 MB）拷进 `OUT_DIR`，
//! 再对它们发 `cargo:rustc-link-search`。
//!
//! ⚠️ **那正是"导入表里多一行 `WebView2Loader.dll`"的来源**（探路报告 §3.2/§3.4）：
//! 链接器按那些 search path 找 `/DEFAULTLIB` 指定的库，找不到 MSVC 那份就落到同目录的
//! `WebView2Loader.dll.lib` 上 ⇒ 链成动态导入 ⇒ 单文件交付被破坏。
//!
//! ## 补丁版做什么
//!
//! 只留一行链接库声明。**`advapi32` 是系统 DLL**（在 `build_windows.sh` 的白名单里），
//! 它原本就在链接行上，删掉它反而会改变链进去的符号集合 —— 那是一次没有理由的偏离。
//!
//! ⚠️ **本文件不碰 `x64/WebView2Loader.dll`**：那份 DLL 由 `shell-win/build.rs` 拷进
//!    壳的 `OUT_DIR`（并且**断言它的 sha256**）。两处都拷会造成两份真相，
//!    而"哪一份进了 exe"就再也说不清了。

fn main() {
    println!("cargo:rustc-link-lib=advapi32");
}
