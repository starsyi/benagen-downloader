//! 「选择文件夹」的**平台那一半**（对位 macOS `SettingsView.swift:230-247` 的 `NSOpenPanel`）。
//!
//! ## 它为什么存在
//!
//! 设置窗口里改下载目录原先只有一条路：把路径**粘**进那个输入框。
//! 真机上的反馈很直接（2026-09-20）："只能通过复制目录路径才可以，是否可以通过
//! '选择'的方式打开目录进行点击选择"。macOS 侧一直有那颗「选择…」，
//! 本模块就是它在 Windows 上的对位。
//!
//! ## 分工：判据在下面那个纯函数，OS 调用在这里
//!
//! [`pick_outcome`] 回答"用户这一次点选**算不算数**"（取消 / 空串 / 真路径三档），
//! 本模块只做**那一次系统调用**。**本模块不检查路径能不能用** ——
//! "这个文件夹能不能拿去当下载目录"是 `DownloadDirectory::check`（`api::preferences`）
//! 的事，前端拿到路径之后走的是与手工输入**完全相同**的那条路
//! （`settings.js:askDirectory`：检查 → 确认 → 写偏好 → 重启内核）。
//! 两处都判会让"什么算合法"变成两个会漂移的答案。
//!
//! ## ⚠️ 为什么这一整块是平台专属的（每一处 `#[cfg]` 都要能回答这个问题）
//!
//! "让用户点一个文件夹"**没有跨平台的做法**：
//!   * Windows 上的正统入口是 `SHBrowseForFolderW`（`shell32.dll` 的 API，Win32 专有）；
//!     更现代的 `IFileOpenDialog` + `FOS_PICKFOLDERS` 只有 `windows` crate 有绑定，
//!     而本 crate 的依赖口径是 `windows-sys`（计划任务 1 定的）——不为了一个对话框
//!     把整棵绑定树再引一遍。
//!   * macOS 上对应的是 `NSOpenPanel`，命令行入口是 `osascript` 的 `choose folder`；
//!   * Linux 上两个都不是。
//!
//! ⇒ 这不是"懒得抽象"，而是**这件事本身就是平台 API**。宿主那一支存在的唯一理由是
//! **规格 §9.0**：这台 Mac 上要能真跑一遍（否则命令层这条路在交付前一次都没被执行过）。
//! ⚠️ 宿主那一支**不是交付形态**（交付物只有 Windows exe）—— 别把它当成"支持了 macOS"。

/// 一次点选的结果 → 命令层要回给前端的那个值。
///
/// ⚠️ 三档，**每一档都在挡一种具体的坏形态**：
///   * **取消**（`None`）⇒ `None`。取消一个对话框什么都不该发生 ——
///     把它报成错误会让常驻提示行无缘无故亮一条，而用户什么都没做错。
///   * **空串 / 只有空白** ⇒ `None`。**这一档是承重的**：它一旦漏过去，前端拿到 `""`
///     会把它当成"恢复默认"（那是 `preferences_set` 的既有语义：空串 = 回到未配置），
///     于是**用户取消了一次选择、却弹出一个"更改下载目录？"的确认框** ——
///     而且确认下去真的会把目录改掉。
///   * **其余** ⇒ **原样**返回。**不 trim、不规范化、不拼盘符**：路径的原文是
///     `DownloadDirectory::check` 与内核那边的事，壳在这里动一个字符就是在
///     制造第二个真相源（约束 3 的同一条纪律）。
pub fn pick_outcome(picked: Option<String>) -> Option<String> {
    match picked {
        Some(path) if !path.trim().is_empty() => Some(path),
        _ => None,
    }
}

/// 弹一次「选择文件夹」。
///
/// 返回 `None` = 用户取消（或系统那边没给出可用路径）——**不是错误**，见 [`pick_outcome`]。
///
/// ⚠️ `owner` 是**主窗口的 `HWND`**（宿主上传 `null`）。它为什么是承重的：
///    `SHBrowseForFolderW` 的 `hwndOwner` 决定这个对话框**是不是主窗口的模态** ——
///    传 `null` 的话它没有属主，用户可以把它点到底下（或者再点一次「选择…」开出第二个），
///    而这在一个"选完就把目录改掉"的动作上很容易出事。取句柄那一半在
///    `commands::owner_hwnd`（那里才有 Tauri 的窗口对象）。
///    宿主那一支本来就不需要它（`osascript` 那个选择框由系统管模态）。
pub fn pick_folder(owner: *mut std::ffi::c_void) -> Option<String> {
    pick_outcome(browse_for_folder(owner))
}

/// 把"选一个文件夹"交给系统。**平台专属**（见模块头）。
#[cfg(target_os = "windows")]
fn browse_for_folder(owner: *mut std::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::Foundation::MAX_PATH;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_EDITBOX, BIF_NEWDIALOGSTYLE,
        BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };

    // ⚠️ **两个缓冲区都按 `MAX_PATH` 给**（`Win32::Foundation::MAX_PATH` = 260），
    //    而不是"这个路径看起来不长"：`SHGetPathFromIDListW` 的契约是
    //    "调用方提供一个至少 `MAX_PATH` 个 `wchar` 的缓冲区"，它**自己不做长度检查**
    //    —— 给小了就是一次溢出写。
    let mut display_name = [0u16; MAX_PATH as usize];
    let mut path = [0u16; MAX_PATH as usize];
    // 标题那句来自 `shell-core`（R-24：面向用户的字符串不留在命令层）——
    // 再补一个 NUL（`LPCWSTR` 的契约）。
    let title: Vec<u16> = shell_core::api::preferences::picker_title()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut info = BROWSEINFOW {
        // 主窗口的句柄（由 `commands::owner_hwnd` 取来）⇒ 这个对话框是**它的模态**：
        // 用户既点不到底下的界面，也开不出第二个。宿主上是空指针（那一支用不到，
        // 见 [`pick_folder`] 的文档）。
        hwndOwner: owner,
        // 从"桌面"那棵树的根开始，而不是某个自定义命名空间。
        pidlRoot: std::ptr::null_mut(),
        // 这是**出参**：对话框把它选中的显示名写进这个缓冲（我们不用它，
        // 但契约要求给一块至少 `MAX_PATH` 的内存）。
        pszDisplayName: display_name.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        // 三面旗子各自挡一种东西：
        //   · `BIF_RETURNONLYFSDIRS` —— 只回**文件系统**里的目录：没有它，
        //     用户能选中"网络"那种 Shell 命名空间节点，而它没有可用的路径；
        //   · `BIF_NEWDIALOGSTYLE`  —— 新式对话框（可缩放、带"新建文件夹"）：
        //     规格 §2.1 要的 `canCreateDirectories` 就是这一面旗子给的；
        //   · `BIF_EDITBOX`        —— 带一个可以直接粘路径的输入框：
        //     这次改动**不是**为了去掉粘贴这条路，而是给它加一条点选的。
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        // 不挂回调（那个回调是用来定制对话框行为的，本代用不到）。
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };

    // SAFETY：`info` 在本次调用期间一直存活，它里面的两根指针分别指向两个栈上数组
    //         与一个 `Vec`（都在本函数内、都活到调用之后）；
    //         `lpfn: None` 是"没有回调"的合法写法。
    let pidl = unsafe { SHBrowseForFolderW(&mut info) };
    // 空指针 = **用户按了取消**（这是这条 API 唯一说"取消"的方式，没有错误码）。
    if pidl.is_null() {
        return None;
    }
    // SAFETY：`path` 是一块 `MAX_PATH` 个 `u16` 的内存，满足上面那条契约。
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
    // ⚠️ **无论取没取到路径都要把它还回去**：这个 PIDL 是 `shell32` 用
    //    `CoTaskMemAlloc` 分配、**所有权交给了调用方**的一块内存，不释放就是一次泄漏。
    //    之所以是 `CoTaskMemFree` 而不是别的：`SHBrowseForFolderW` 的文档写明了
    //    "it is the caller's responsibility to free the returned PIDL with
    //    CoTaskMemFree"（这条 API 老了，与 `ILFree` 那一路不是同一份契约）。
    // SAFETY：`pidl` 非空（上面判过），且来自这一次调用、还没有被释放过。
    unsafe { CoTaskMemFree(pidl as *const core::ffi::c_void) };
    // 取路径失败（例如那是一个没有文件系统路径的节点）⇒ 与取消同档：
    // 用户能做的下一步是一样的（再选一次），而编一条错误原文出来只会更糊涂。
    if ok == 0 {
        return None;
    }
    let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Some(String::from_utf16_lossy(&path[..end]))
}

/// 宿主（macOS / Linux）那一支：`osascript` 的 `choose folder`。
///
/// ⚠️ **只为本地开发能验**（规格 §9.0 的宿主构建）：交付形态只有 Windows exe。
///    它与 Windows 那一支**语义对位**（选一个文件夹、可以取消、取消不报错），
///    所以宿主上走一遍"命令 → 判据 → 系统调用 → 结果"这条链路的形状是真的。
///
/// ⚠️ `POSIX path of` 是承重的：`choose folder` 回的是一个 **AppleScript 别名**，
///    直接 `as text` 出来的是 `Macintosh HD:Users:…` 那种 HFS 路径，
///    而下载目录那一路要的是 POSIX 路径（`/Users/…`）——
///    少了这一段，用户选出来的目录在界面上会是一串看不懂的冒号，而且**存下去就是错的**。
/// ⚠️ 用户取消时 `osascript` 退出码是 **1**（stderr 上是 `User canceled`），
///    所以"非零退出"在这里**不是失败**，而是那条 API 说"取消"的方式（同 Windows 那支
///    的空指针）。⇒ 这里**一律回 `None`**，不再分"取消"与"真出错"——
///    分不出来的话，前端要么看到一个假的错误，要么在取消时什么都不发生（后者是对的）。
#[cfg(not(target_os = "windows"))]
fn browse_for_folder(_owner: *mut std::ffi::c_void) -> Option<String> {
    // ⚠️ 标题那句**来自 `shell-core`**（R-24），不在这里另写一份：
    //    两处各写一遍的话，Windows 与宿主上的对话框会随着哪边先改而分叉。
    //    ⚠️ 它被拼进一段 AppleScript **源码**里 ⇒ 里面不能有双引号
    //    （`picker_title()` 的判据那句里没有，而它一旦有人加上，宿主这一支会直接
    //    语法错、退出非零 ⇒ 落成 `None`：不是静默错，但也不是像样的报错。
    //     这条约束的可执行版本是下面那条 `the_picker_title_cannot_break_the_applescript`）。
    let script = format!(
        "POSIX path of (choose folder with prompt \"{}\")",
        shell_core::api::preferences::picker_title()
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    // `osascript` 会补一个换行；末尾那个 `/` 是它给目录路径的写法（`/Users/x/`）。
    // ⚠️ 两个都留着也可以（`DownloadDirectory::check` 判的是"能不能用"），
    //    但这里只做一件事：把**行尾那一个换行**去掉 —— 它一定不是路径的一部分。
    Some(text.trim_end_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ **取消与"选了个空"都落成 `None`，而且理由各不同**。
    ///
    /// 判别力（两个方向都会红）：把空串那一支删掉 ⇒ 前端会拿 `""` 走
    /// `preferences_set` 的"恢复默认"语义 ⇒ **用户取消一次却弹出确认框**；
    /// 把 `trim` 去掉 ⇒ 只有空格的那一格会漏过去（同一个后果）。
    #[test]
    fn cancelling_and_an_empty_pick_both_land_on_none() {
        assert_eq!(pick_outcome(None), None, "取消不是错误，但也不该回一个值");
        assert_eq!(pick_outcome(Some(String::new())), None, "空串会被前端读成「恢复默认」");
        assert_eq!(
            pick_outcome(Some("   ".to_string())),
            None,
            "只有空白同样会被读成「恢复默认」"
        );
    }

    /// ⚠️ **真路径原样回来**（一个字符都不改）：不 trim、不补盘符、不把 `\` 换成 `/`。
    ///
    /// 判别力：在返回前加一句 `trim()` 或 `replace('\\', "/")`，这一条会红 ——
    /// 而那一改会让"壳里多出一份路径规范化"，与 `DownloadDirectory::check` 分叉。
    #[test]
    fn a_real_path_comes_back_verbatim() {
        for path in [
            r"C:\Users\客户\交付",
            r"D:\",
            "/Volumes/Data/交付",
            // 前后带空格是一个**合法的**路径（Windows 上真做得到），不许被我们吃掉。
            " /tmp/x ",
        ] {
            assert_eq!(
                pick_outcome(Some(path.to_string())).as_deref(),
                Some(path),
                "路径原文不许被动：{path:?}"
            );
        }
    }

    /// ⚠️ **那句标题里不许出现双引号**：宿主那一支把它拼进一段 AppleScript 的源码里
    /// （`choose folder with prompt "…"`），一个引号就会让整段脚本语法错 ——
    /// 而症状是"点了「选择…」什么都没发生"（退出非零被我们读成取消）。
    /// 判别力：往 `picker_title()` 里塞一个 `"`，这一条立刻红。
    #[test]
    fn the_picker_title_cannot_break_the_applescript() {
        let title = shell_core::api::preferences::picker_title();
        assert!(
            !title.contains('"') && !title.contains('\\'),
            "标题会被拼进 AppleScript 源码，不许带引号或反斜杠：{title:?}"
        );
        assert!(!title.trim().is_empty(), "标题不许是空的（对话框顶上会空一块）");
    }

    /// 宿主上真的走一遍系统调用（会**真的弹出一个选择框**）。
    ///
    /// 同 `reveal.rs` 那条：它是 `#[ignore]` 的，理由是**副作用**（会抢焦点、
    /// 在无人值守的 `test.sh` 里挂在那里等一个人点）。验证方式是手工跑一次：
    /// `cargo test -p shell-win --lib pickdir:: -- --ignored`。
    /// ⚠️ 这一条**不需要**断言"选中了什么"（那要一双手），它验的是"链路通、
    ///    取消/失败都落成 `None` 而不是 panic"—— 所以手工跑时点「取消」即可。
    #[cfg(not(target_os = "windows"))]
    #[ignore = "会真的弹出系统选择框（抢焦点）—— 手工跑：cargo test -p shell-win --lib pickdir:: -- --ignored"]
    #[test]
    fn the_host_branch_really_opens_a_picker() {
        // 宿主那一支不需要属主窗口（`osascript` 那个选择框由系统管模态）。
        let picked = pick_folder(std::ptr::null_mut());
        // 人点「取消」⇒ None；真选了一个 ⇒ 一个非空字符串。两档都算这条链路通。
        if let Some(path) = picked {
            assert!(!path.trim().is_empty(), "选出来的路径不许是空的：{path:?}");
        }
    }
}
