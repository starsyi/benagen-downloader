//! **单实例**（规格 §7 那张表的第五行、待定项 D-3）。
//!
//! ## 它治的是什么
//!
//! 用户双击两次图标（或者从任务栏再点一次）会起来**两份**壳，于是**两个内核进程**
//! 同时往同一个下载目录写、两条 `history.json` 写路径互相覆盖、两个窗口显示同一批
//! 任务的两份进度。规格 §2.2 明写"Windows 版是**单实例单窗口**"。
//!
//! ## 机制：一个命名互斥体（**Windows 专属落点**）
//!
//! `CreateMutexW` 建一个**有名字**的互斥体：名字是全局的（本会话内），
//! 第二份进程建同名互斥体时拿到的是同一把锁、并且 `GetLastError()` 回
//! `ERROR_ALREADY_EXISTS` ⇒ 那就是"已经有一份在跑"的**唯一**判据。
//!
//! ⚠️ **为什么不用"扫进程名"**：那是一份**可能出错**的猜测 —— 同名的别的程序、
//!    权限不足读不到别人的进程、进程正在退出还没被回收，三种情形都会让判据说谎。
//!    内核对象的名字是**内核对同一件事的权威回答**。
//!
//! ⚠️ **为什么是 `#[cfg]` 分叉、宿主上是 no-op**：
//!   * `CreateMutexW` 是 Win32 API（`kernel32.dll`），宿主上**不存在**；
//!   * 单实例在宿主上**没有对应物**（拿一个 `flock` 顶替是不同的东西：它管的是文件，
//!     不是"这份程序跑了几次"）；
//!   * 宿主构建是**开发期的脚手架**（规格 §9.0），交付形态只有 Windows exe。
//!   ⇒ 宿主上恒回 [`Verdict::First`]。**这不是静默降级**：宿主上没有任何东西被保护，
//!     也没什么可保护的（本地开发的第二个实例不写客户的交付目录）。
//!   ⚠️ 有一条**残留代价**如实记下：这条判据因此**在本机一条断言都没有**
//!     （规格 §9.1 那张表把 `CreateMutexW 单实例判据` 明确列在"只能在真 Windows 上判"里）。
//!     本文件能测的只有"读到的那个读数怎么判"（[`verdict`]）与那句文案。

/// 互斥体的名字。
///
/// ⚠️ **前缀 `Local\` 是承重的**：不带前缀时名字落在**全局**命名空间里 ——
///    那需要 `SeCreateGlobalPrivilege`，而一般用户**没有**它 ⇒ `CreateMutexW`
///    会失败（回 `null`），于是"单实例"变成"永远放行"（一个**静默失效**的判据）。
///    `Local\` = 本登录会话，正是我们要的粒度（同一台机器上两个用户各跑一份是对的）。
/// ⚠️ 它是 `pub` 的**不是为了让人用**，而是因为本 crate 的公开面**不算死代码**
///    （`lib.rs` 的 D1 那条理由）：它在宿主上没有任何调用者（`acquire` 那一支是
///    `#[cfg]` 掉的），而本仓库是**零告警**口径 —— 一个 `const` 在宿主上凭空多出一条
///    `dead_code` 就是一条需要解释的残留告警。`pub` 是这里唯一诚实的写法
///    （写 `#[allow(dead_code)]` 等于把"它在本靶上真的没被用到"这件事按下去）。
pub const MUTEX_NAME: &str = r"Local\BenagenDownloader.SingleInstance";

/// 判据的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 本程序还没有在跑（这份进程拿到了名额）。
    First,
    /// **已经有一份在跑了** —— 调用方要说一句话然后退出。
    AlreadyRunning,
}

/// `GetLastError()` 的读数 → 结论。**纯函数**（Win32 那一半只负责把读数交进来）。
///
/// ⚠️ 抽成纯函数是为了**判别力**：真正的 `CreateMutexW` 在本机跑不了，
///    于是"这两个读数各自判成什么"必须能在宿主上被断言（同 `spawn.rs` 的
///    `creation_flags_for`、`platform.rs` 的 `env_var_name`：决策与调用分开）。
pub fn verdict(already_exists: bool) -> Verdict {
    if already_exists {
        Verdict::AlreadyRunning
    } else {
        Verdict::First
    }
}

/// 第二份进程要说的那句话（**原生消息框**，因为那一刻它马上就要退出、没有窗口）。
///
/// 🔴 规格 **D-3** 的第一版口径：**做不到把已有窗口抬到前面**，所以第一版如实说这一句。
///    三件事缺一不可：
///   * 事实（已经在运行了）；
///   * **下一步**（去用已经打开的那个窗口）；
///   * **为什么你看不到它**（它可能被别的窗口盖着）—— 少了这半句，用户会以为程序坏了、
///     于是反复双击（每双击一次就多看到一次这个框）。
///
/// ⚠️ 措辞里**不许出现"历史""备注"那类字**的教训见 `storage/mod.rs` 的模块头
///    （第二条 `--check` 会拦：内嵌字体子集必须覆盖源码里的每一个非 ASCII 字）。
pub fn already_running_message() -> &'static str {
    "Benagen 数据下载工具已经在运行了。请用已经打开的那个窗口。\
     这一版不会把那个窗口带到最前面来——它可能被别的窗口盖着。"
}

/// 建互斥体、读一次 `GetLastError()`，把结论交回来。
///
/// ⚠️ **拿到名额的那一个句柄是故意不关的**：它必须活到**进程退出**为止 ——
///    一关，第二份进程就能进来了（互斥体的存活期 = 句柄的存活期）。而本函数只回一个
///    结论、没有地方安放那个句柄 ⇒ `First` 那一支**有意让它泄漏**：
///    进程退出时内核会收掉它，而那正是我们要的时机（同 `main.rs` 里那些"活到进程结束"
///    的东西）。`AlreadyRunning` 那一支拿到的句柄**立刻关掉**（这份进程马上要退出了）。
///
/// ⚠️ **建不出来（回 `null`）时回 `First`**：那不是"已经有一份在跑"（`GetLastError()`
///    在那种情况下是别的错误），而是"这道判据今天用不了"。此时**放行**是唯一合理的选择
///    —— 反过来（挡住）会让用户在一种我们没预料到的环境里**完全打不开程序**，
///    而放行的代价只是回到"没有单实例判据"的那个老行为（多一个窗口，不是不能用）。
#[cfg(target_os = "windows")]
pub fn acquire() -> Verdict {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = MUTEX_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY：`name` 指向以 NUL 结尾、在本调用期间一直存活的缓冲区；
    //         `null` 的安全属性 = 默认（子进程不继承）；`binitialowner = 0` =
    //         我们**不要求**初始所有权（只要那个名字存在就够了，判据在 `GetLastError`）。
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // 见函数文档最后一段：判据用不了 ⇒ 放行。
        return Verdict::First;
    }
    // SAFETY：`GetLastError()` 无参数、无副作用（它读的是本线程的最后一个错误码）。
    //         ⚠️ **必须在 `CreateMutexW` 之后、任何别的 Win32 调用之前读**：错误码是
    //         **本线程共享**的一格，中间插一个别的调用就会把它冲掉。
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    match verdict(already_exists) {
        Verdict::AlreadyRunning => {
            // SAFETY：这个句柄是刚拿到的、还没被关过；关掉它是为了不留一个空名额。
            unsafe { CloseHandle(handle) };
            Verdict::AlreadyRunning
        }
        // ⚠️ **这一支故意不关**（理由见函数文档）：句柄本身**就是**那个名额。
        Verdict::First => Verdict::First,
    }
}

/// 宿主（macOS / Linux）那一支：**恒 `First`**（理由见模块头：宿主上没有这件事）。
///
/// ⚠️ 它**不是**"编一个能过的判据"：本函数一个系统调用都不发，也不假装检查过什么 ——
///    返回值说的是"本平台不做单实例"，而调用方那一侧的动作（已经有一份在跑就退出）
///    在宿主上**永远不会发生**。这是一条**如实的**平台落差，不是降级。
#[cfg(not(target_os = "windows"))]
pub fn acquire() -> Verdict {
    Verdict::First
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ **两个读数各判成什么**（这一条是"单实例判据"在本机唯一的牙齿，
    /// 理由见模块头：真正的 `CreateMutexW` 只有真 Windows 判得了）。
    ///
    /// 判别力（两个方向都会红）：把两个分支写反 —— 那在真机上表现为
    /// **第一份进程自己弹一句"已经在运行"然后退出**（程序根本打不开），
    /// 而第二份进程兴高采烈地起来（单实例保护整个失效）。
    #[test]
    fn the_two_verdicts_are_not_swapped() {
        assert_eq!(verdict(false), Verdict::First);
        assert_eq!(verdict(true), Verdict::AlreadyRunning);
        assert_ne!(verdict(false), verdict(true));
    }

    /// ⚠️ 互斥体名**必须带 `Local\` 前缀**（不带就是一个永远失败的判据，见常量的文档）。
    ///
    /// 判别力：把前缀删掉（一个很自然的"清理"），这一条立刻红 ——
    /// 而真机上的表现是"单实例**默默地**不起作用"（`CreateMutexW` 因权限回 `null`，
    /// `acquire()` 走"建不出来 ⇒ 放行"那一档），两份进程又都起来了。
    #[test]
    fn the_mutex_name_stays_in_the_local_namespace() {
        assert!(
            MUTEX_NAME.starts_with(r"Local\"),
            "不带 `Local\\` 就需要 SeCreateGlobalPrivilege ⇒ 而那个权限默认没有，建不出来：{MUTEX_NAME}"
        );
        assert!(
            MUTEX_NAME.contains("BenagenDownloader"),
            "名字里要有这个程序的名字（免得与别的程序的这个名字撞上）：{MUTEX_NAME}"
        );
        // 不是空串、也不是一个只剩前缀的名字。
        assert!(MUTEX_NAME.len() > r"Local\".len() + 4);
    }

    /// 🔴 那句话三件事都在（事实 / 下一步 / 为什么你看不到那个窗口）。
    ///
    /// 判别力：删掉"被别的窗口盖着"那半句，第三条断言立刻红 —— 而真机上的表现是
    /// 用户以为程序坏了、于是**反复双击**（每双击一次就多看到一次这个框）。
    #[test]
    fn the_message_says_it_is_running_and_where_to_look() {
        let message = already_running_message();
        assert!(message.contains("已经在运行"), "要说清事实：{message}");
        assert!(message.contains("已经打开的那个窗口"), "要给出下一步：{message}");
        assert!(
            message.contains("盖着"),
            "要说清「为什么看不到那个窗口」（否则用户会反复双击）：{message}"
        );
        assert!(!message.is_empty());
    }

    /// 宿主上 `acquire()` 恒 `First`（**如实**：本平台不做单实例，见模块头）。
    ///
    /// ⚠️ 这条断言**在本机是恒真的**（它是"平台落差"的可执行写法），所以它守的不是
    ///    "判据对不对"，而是"**别有人在宿主上顺手加一个假的检查**"：
    ///    那样做的后果是本地开发时第二个实例会被拒（而那正是任务 7 报告 §4.4
    ///    那条观察的反面），而**没有任何东西会因此变红**。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn the_host_does_not_pretend_to_have_single_instance() {
        assert_eq!(acquire(), Verdict::First);
    }
}
