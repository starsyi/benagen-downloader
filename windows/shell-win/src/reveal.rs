//! 「在资源管理器中显示」的**平台那一半**（规格 §7 那张表的第三行）。
//!
//! ## 分工：判据在 `shell-core`，OS 调用在这里
//!
//! `shell_core::api::reveal` 回答"**要显示哪一个文件**"（路径怎么拼、算不出来时算什么档），
//! 本模块只做**那一次系统调用**：Windows 走 `ShellExecuteW` + `explorer /select,`，
//! 宿主走 `open -R`。本模块**不拼路径**（那会把"路径怎么算"变成第二个真相源）。
//!
//! ## ⚠️ 为什么这一整块是平台专属的（每一处 `#[cfg]` 都要能回答这个问题）
//!
//! "在文件管理器里选中一个文件"**没有跨平台的做法**：
//!   * Windows 上唯一的做法是把 `/select,<路径>` 交给 `explorer.exe`，而启动它的正统
//!     入口是 `ShellExecuteW`（`shell32.dll` 的 API，Win32 专有）；
//!   * macOS 上对应的是 `open -R`（LaunchServices 的命令行入口）；
//!   * Linux 上两个都不是（各家文件管理器自己的参数）。
//!
//! ⇒ 这不是"懒得抽象"，而是**这件事本身就是平台 API**。宿主那一支存在的唯一理由是
//! **规格 §9.0**：这台 Mac 上要能真跑一遍（否则命令层这条路在交付前一次都没被执行过）。
//! ⚠️ 宿主那一支**不是交付形态**（交付物只有 Windows exe）—— 别把它当成"支持了 macOS"。
//!
//! ⚠️ `ShellExecuteW` 的返回值有一个**古老的坑**：它回的是 `HINSTANCE`，
//! **≤ 32 全是错误码**（0 = 内存不足、2 = 找不到文件、5 = 拒绝访问…）。
//! 判据抽成纯函数 [`shell_execute_succeeded`]，于是那半条**在宿主上也测得到**
//! （同 `spawn.rs` 的 `creation_flags_for`：决策与调用分开，决策就能跨平台断言）。

use std::collections::BTreeMap;

use shell_core::api::reveal::Outcome;
use shell_core::presentation::transfer_row::TransferReveal;

/// 走一遍「显示」：算路径（判据在 `shell-core`）→ 交给系统。
///
/// ⚠️ 返回 [`Outcome::Ready`] 表示**已经把它交给资源管理器了**（系统调用成功）——
///    那一格里带的就是刚交给系统的那个**绝对路径**（成功没有别的信息可说，
///    而这个路径是宿主上验证时唯一想看到的东西）。
///
/// ⚠️ `environment` 是**参数**（`TransferReveal::home` 的契约：本 crate 不含 OS 调用，
///    环境由调用方给）。取 home 的**回落值是空串** —— 宿主上没有 `NSHomeDirectory()` 的
///    对应物，而"猜一个"更糟：取不到就是取不到，它会走 [`Outcome::RootUnknown`]
///    如实说"算不出它在哪"，而**不会**拼出一个基准是进程当前工作目录的相对路径。
pub fn reveal(
    manifest_path: Option<&str>,
    environment: &BTreeMap<String, String>,
    download_dir: &str,
) -> Outcome {
    let home = TransferReveal::home(environment, "");
    match shell_core::api::reveal::local_path(manifest_path, &home, download_dir) {
        Outcome::Ready(path) => match select_in_file_manager(&path) {
            Ok(()) => Outcome::Ready(path),
            Err(why) => Outcome::Failed(why),
        },
        other => other,
    }
}

/// 当前进程的环境变量（`TransferReveal::home` 要的那张表）。
///
/// ⚠️ **不用 `std::env::vars()`**：它在**任何一个变量的值不是合法 UTF-8 时 panic**
///    （Windows 上那是可能的）。命令层 panic 的后果是前端那条 `invoke` **永远等不到回话**
///    （本项目最恨的沉默挂住）⇒ 这里用 `vars_os` 并**跳过**读不成字符串的那几个。
///    ⚠️ 跳过是安全的：这张表只用来找 home 那一级变量，而**取不到 home 有自己的档**
///    （[`Outcome::RootUnknown`]，会说人话），不会静默拼出一个错路径。
pub fn process_environment() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// `ShellExecuteW` 的返回值算不算成功。
///
/// ⚠️ 门限是 **32**，不是 0：Win32 给 `ShellExecute` 家族的契约是
/// "返回值 > 32 才是成功，≤ 32 是一个 `SE_ERR_*` 错误码"。
/// 判成 `> 0` 会把"找不到文件"（2）当成成功 —— 而那正是这条判据要拦的那一类。
///
/// 上游第二代在 `browser.rs` 里有过同名的判据（那条路随第二代整体废弃，
/// 规格 §1.2）；这里的是它的**同一条**（不是新判据）。
pub fn shell_execute_succeeded(return_value: i32) -> bool {
    return_value > 32
}

/// 把"在文件管理器里选中这个文件"交给系统。**平台专属**（见模块头）。
#[cfg(target_os = "windows")]
fn select_in_file_manager(path: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// 宽字符 + NUL 结尾（Win32 的 `LPCWSTR` 契约）。
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // `/select,` 后面那个路径**必须带引号**：不带的话带空格的路径会被 explorer 拆成
    // 两个参数（它只会选中一个莫名其妙的位置）。
    let operation = wide("open");
    let file = wide("explorer.exe");
    let parameters = wide(&format!("/select,\"{path}\""));
    let directory = wide("");

    // SAFETY：四根指针都指向以 NUL 结尾、且在本调用期间一直存活的缓冲区；
    //         `hwnd` 传 null 是"没有父窗口"（这是一个 GUI 子系统的程序，没有控制台窗口
    //         可以借用）—— 这正是 `ShellExecuteW` 的契约。
    let returned = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            directory.as_ptr(),
            SW_SHOWNORMAL,
        )
    };
    // `HINSTANCE` 在这里只当一个整数看（判据就是"它是不是 ≤ 32"），所以按地址转 i32。
    let returned = returned as isize as i32;
    if shell_execute_succeeded(returned) {
        Ok(())
    } else {
        // ⚠️ 这个数不是"给人看的错误原文"（Win32 的错误码表在文档里，不在代码里）：
        //    它是**可查的证据**，与"没找到 open"那类原文并列交给调用方。
        Err(format!("ShellExecuteW 返回 {returned}（不超过 32 都是错误码）"))
    }
}

/// 宿主（macOS / Linux）那一支：`open -R` = 在访达里**选中**这个文件。
///
/// ⚠️ **只为本地开发能验**（规格 §9.0 的宿主构建）：交付形态只有 Windows exe。
///    它与 Windows 那一支**语义对位**（选中而不是打开），所以宿主上走一遍
///    "命令 → 判据 → 系统调用 → 结果"这条链路的形状是真的。
#[cfg(not(target_os = "windows"))]
fn select_in_file_manager(path: &str) -> Result<(), String> {
    match std::process::Command::new("open").arg("-R").arg(path).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("open -R 退出了 {status}")),
        Err(cause) => Err(format!("起不了 open：{cause}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// ⭐ **门限是 32 这件事本身**（这一条能杀掉"判成 `> 0`"那个改法）。
    ///
    /// 判别力（两个方向都会红）：改成 `> 0` ⇒ "找不到文件"（2）会被当成成功；
    /// 改成 `> 30` ⇒ 31（一个真实的错误码）会被当成成功。
    #[test]
    fn the_shell_execute_success_threshold_is_greater_than_thirty_two() {
        for value in [0, 2, 5, 31, 32] {
            assert!(
                !shell_execute_succeeded(value),
                "{value} 是错误码（不超过 32），不许判成成功"
            );
        }
        for value in [33, 34, 42] {
            assert!(shell_execute_succeeded(value), "{value} 该判成成功");
        }
    }

    /// 走一遍**判据那一半**（不碰系统调用）：配过下载目录时算出来的路径是
    /// 目录 + 清单原文；没有路径时是"这一行没有文件"；算不出根时是"定位不到"。
    #[test]
    fn the_judgement_half_is_reachable_without_touching_the_os() {
        // 配过下载目录 ⇒ Ready（本函数**不看** home）。
        assert_eq!(
            shell_core::api::reveal::local_path(Some("a/b.txt"), "/Users/x", "/data/交付"),
            Outcome::Ready("/data/交付/a/b.txt".to_string())
        );
        assert_eq!(
            shell_core::api::reveal::local_path(None, "/Users/x", "/data"),
            Outcome::NoManifestPath
        );
        assert_eq!(
            shell_core::api::reveal::local_path(Some("a.txt"), "", ""),
            Outcome::RootUnknown
        );
    }

    /// ⚠️ **`reveal()` 不接受一个空的 home 与空的下载目录**那一条走的是
    /// `RootUnknown`，**一次系统调用都不发**（否则会拉起一个指向相对路径的资源管理器）。
    #[test]
    fn an_unknown_root_never_reaches_the_file_manager() {
        assert_eq!(
            reveal(Some("a.txt"), &env(&[]), ""),
            Outcome::RootUnknown,
            "算不出根时不许去开资源管理器（那会开到一个毫不相干的地方）"
        );
    }

    /// 环境那一份取自**当前进程**，且至少带着本靶用来找 home 的那一级变量
    /// （本机的 `HOME` / Windows 的 `USERPROFILE`；这里不假定它一定在，
    /// 只钉住"函数跑得通、不 panic、拿到的表与进程看到的一致"）。
    #[test]
    fn the_process_environment_is_read_without_panicking() {
        let environment = process_environment();
        let key = TransferReveal::home_variable();
        match std::env::var(key) {
            Ok(value) => assert_eq!(
                environment.get(key).map(String::as_str),
                Some(value.as_str()),
                "读到的环境与进程看到的不一致"
            ),
            // 变量没设：表里不该有它（这条同样有判别力 —— 它排除"读到的是别的变量"）。
            Err(_) => assert!(!environment.contains_key(key)),
        }
    }

    /// ⚠️ **宿主上真的走一遍系统调用**（`open -R`），且**用的是临时目录里的一个真文件**。
    ///
    /// 它验的是"命令 → 判据 → 系统调用 → 结果"这条链的形状（返回值与错误话术），
    /// **不是**"访达真的选中了它"（那要一双眼睛，规格 §9.2 的口径）。
    ///
    /// 🔴 **它是 `#[ignore]` 的，理由是副作用**：它会**真的拉起访达**（一个窗口、
    ///    还可能抢焦点）。让每一次 `bash windows/scripts/test.sh` 都开一个窗口是
    ///    **坏的邻居行为**（同一条理由让 `presentation/app_preferences.rs` 的测试
    ///    只碰临时目录、不碰用户真实的下载目录）。
    ///    ⇒ 这不是"跳过"：验证方式是**手工跑一次**，命令就在下面那行属性里 ——
    ///    `cargo test -p shell-win --lib reveal:: -- --ignored`（任务 8 的报告里贴了读数）。
    ///
    /// ⚠️ **不要**把它改成"不调用系统、只断言参数"的形态：参数拼装那一半已经有
    ///    `shell_core::api::reveal` 的纯函数用例守着，而这里要验的恰恰是**那一次调用**。
    #[cfg(not(target_os = "windows"))]
    #[ignore = "会真的拉起访达（开出一个窗口）—— 手工跑：cargo test -p shell-win --lib reveal:: -- --ignored"]
    #[test]
    fn the_host_branch_really_calls_the_file_manager() {
        let dir = std::env::temp_dir().join(format!(
            "benagen-reveal-tests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").expect("放一个文件");

        // 存在的文件 ⇒ 成功（`open -R` 退出 0）。
        let outcome = reveal(
            Some("a.txt"),
            &env(&[]),
            dir.to_str().expect("临时目录的路径必须是合法的 UTF-8"),
        );
        assert!(
            matches!(outcome, Outcome::Ready(_)),
            "宿主上 `open -R` 该成功：{outcome:?}"
        );

        // ⚠️ **不存在的文件 ⇒ `Failed`**（`open -R` 退出 1，并把
        //    "The file … does not exist." 打在 stderr 上）。
        //    🔴 **这一条是实测订正的**：本用例初稿断言它"照样成功"（那是凭印象写的，
        //    当时给的理由是"它只是选中一个无效位置"），而 2026-09-20 在宿主上跑了一遍
        //    之后读数相反（`exit status: 1`）。⇒ 断言按**读数**改，并把这件事钉住：
        //    宿主这一支**真的能判"文件还在不在"**，而那比 Windows 那一档更强
        //    （`ShellExecuteW` 那边只有返回值 ≤ 32 这一个数，看不出是哪种失败）。
        //    ⚠️ 别把它读成"交付形态也能判"：交付形态上跑的是 Windows 那一支。
        let outcome = reveal(
            Some("没有这个文件.txt"),
            &env(&[]),
            dir.to_str().expect("临时目录的路径必须是合法的 UTF-8"),
        );
        match outcome {
            Outcome::Failed(why) => assert!(
                why.contains("exit status: 1"),
                "失败话术里要带上 `open -R` 的读数：{why}"
            ),
            other => panic!("不存在的文件该判成 Failed，实际 {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
