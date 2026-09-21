//! ⚠️⚠️ **这是一个 vendor 进来的、被打过补丁的 crate**（`webview2-com-sys` 0.38.2）——
//! 与 crates.io 上那份**不是同一份源码**，改它之前先读这段。
//!
//! ## 为什么要在仓库里养一份（完整根因见 docs/superpowers/2026-09-20-tauri2-windows-crosscompile.md §3）
//!
//! 交付形态是**单个 exe**（W-4）。而 0.38.2 原版对**非 MSVC** 目标（我们的
//! `x86_64-pc-windows-gnu`）发的是 `#[link(name = "WebView2Loader.dll")]` ⇒ 链成
//! **动态导入** ⇒ exe 必须和 `WebView2Loader.dll` 一起发，单文件交付当场不成立。
//!
//! MSVC 目标静态链（`WebView2LoaderStatic.lib`），但那条路对 mingw **原理上就堵死**
//! （MSVC 的 C++ 名字修饰 + `__security_cookie` 一类符号，探路 §3.3 有实测报错）。
//!
//! ⇒ 补丁只有一件事：**去掉 gnu 那条 `#[link]`**，让那 5 个符号变成"未定义的普通符号"，
//!   由本仓库自己的 `windows/shell-win/src/wv2.rs` 在**运行期** `GetProcAddress` 转发
//!   （DLL 内嵌在 exe 里，启动时按哈希释放，与内核 exe / aria2c 同一套机制）。
//!
//! ## ⚠️ 升级纪律（这套代码要跟着 Tauri 走）
//!
//! `tauri` 升版 ⇒ `webview2-com-sys` 的版本会跟着变 ⇒ **必须重新 vendor 一次**，
//! 并且重新确认下面那个宏的形状没变（`shell-win/build.rs` 里有一条断言守着这件事，
//! 它读的就是这个文件——**那条断言红了就是升级纪律要你做的事**）。
//! 版本号对不上的表现不是编译错误，而是 `[patch.crates-io]` **静默不生效**
//! （patch 只对"版本相容"的依赖生效）⇒ 补丁白打，而**macOS 上构建照样成功**。
//! 拦住它的只有 `windows/scripts/build_windows.sh` 第 4 步那条导入表判据。

// ⚠️ 下面这一整段（`#[allow(...)]` 与 `pub mod Microsoft`）**是 0.38.2 的原样**，
//    补丁只动了里面那个宏（见 `link_webview2`）。别顺手"整理"它 —— 那会让下一次
//    vendor 时的 diff 变得读不出"哪一处才是我们改的"。
#[allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    clippy::all
)]
pub mod Microsoft {
    pub mod Web {
        pub mod WebView2 {
            pub mod Win32 {
                mod windows_link {
                    macro_rules! link_webview2 {
                        ($library:literal $abi:literal fn $($function:tt)*) => (
                            // 补丁（Windows GNU 交叉编译）：去掉 gnu 目标的链接期导入。
                            // 原版对 not(msvc) 发 link(name = "WebView2Loader.dll")，那会让交付形态
                            // 从"单个 exe"变成"exe + DLL"。gnu 的静态库又用不了（见探路报告 §3.3）。
                            // 这里改成不链接，5 个符号由 shell-win/src/wv2.rs 在运行期转发。
                            //
                            // ⚠️ **MSVC 那一支原样保留，不许动**：动了会坏掉 MSVC 目标的构建，
                            //    而 MSVC 目标在本仓库根本没有构建路径 ⇒ 坏了也不会有人当场发现。
                            #[cfg_attr(target_env = "msvc", link(name = "WebView2LoaderStatic", kind = "static"))]
                            extern $abi {
                                pub fn $($function)*;
                            }
                        )
                    }

                    pub(crate) use link_webview2 as link;
                }

                include!("bindings.rs");
            }
        }
    }
}

pub mod declared_interfaces;

#[cfg(test)]
mod test {
    use windows_core::w;

    use crate::Microsoft::Web::WebView2::Win32::*;

    #[test]
    fn compare_eq() {
        let mut result = 1;
        unsafe { CompareBrowserVersions(w!("1.0.0"), w!("1.0.0"), &mut result) }.unwrap();
        assert_eq!(0, result);
    }

    #[test]
    fn compare_lt() {
        let mut result = 0;
        unsafe { CompareBrowserVersions(w!("1.0.0"), w!("1.0.1"), &mut result) }.unwrap();
        assert_eq!(-1, result);
    }

    #[test]
    fn compare_gt() {
        let mut result = 0;
        unsafe { CompareBrowserVersions(w!("2.0.0"), w!("1.0.1"), &mut result) }.unwrap();
        assert_eq!(1, result);
    }
}
