//! `shell-core` —— Windows 壳的**跨平台纯逻辑**层（规格 §4.3）。
//!
//! 这一层放协议镜像、macOS 版 `Presentation/` 的移植、本地存储，以及所有
//! "把内核数据变成界面值"的纯计算。它**不含 OS 调用、不含任何界面概念**——
//! 「资源管理器」「窗口」「按钮」一律只许出现在 `shell-win`。
//!
//! 于是它能在 **macOS 上** `cargo test`。规格 §4.3 把这条当成结论写下来、
//! §14 待定项第 2 条却老实承认"这是推断，不是实测"；本 crate 的测试就是那条实测
//! （命令与输出见任务 6 报告）。
//!
//! ⚠️ **纪律：谁创建模块文件，谁负责在本文件里加 `pub mod X;`。**
//!    一个建了却**没被声明**的 `.rs` 文件，它的测试**一条都不会跑**，
//!    而 `cargo test` 照样全绿——这正是本项目最怕的假绿。
//!    （`windows/scripts/test.sh` 的"空跑绿灯判为失败"是同一件事的第二道网：
//!    它统计 `running N tests` 的总和，为 0 就 `exit 1`。）
#![forbid(unsafe_code)] // shell-core 是纯逻辑；需要 unsafe 就说明放错地方了

pub mod api;
pub mod client;
pub mod embedded_core;
pub mod platform;
pub mod presentation;
pub mod protocol;
// ⚠️ **本文件是"谁建模块谁登记"的那一处**（见文件头）：`embedded_core.rs` 与
//    `sha256.rs` 都由任务 22 建（内嵌内核 exe 的释放与它的身份判据）。
//    `sha256.rs` **同时**被 `shell-win/build.rs` 用 `#[path = …]` 编进去 ——
//    所以它必须自足（只许用 `std`），理由写在那个文件头上。
pub mod session_view;
pub mod sha256;
pub mod storage;

#[cfg(test)]
mod tests {
    /// 最朴素的一条：证明这个 crate 的测试**真的在宿主上跑起来了**。
    ///
    /// 它的价值不在断言本身，而在于 `cargo test -p shell-core` 那行
    /// `running 1 test` 是真实执行出来的——规格 §14.2 的推断至此变成实测。
    #[test]
    fn shell_core_tests_run_on_the_host() {
        assert_eq!(2 + 2, 4);
    }
}
