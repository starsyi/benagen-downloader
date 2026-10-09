//! 「打开更新下载页」的**平台那一半**（更新提示那条提示条上的「去下载」，R15）。
//!
//! ## 分工：判据（白名单）在这里，OS 调用也在这里 —— 但两者是**两件事**
//!
//! `shell_core::api::update` 回答"提示条该长什么样"；本模块回答**这一条 URL 准不准开**，
//! 以及**真的去把它交给系统浏览器**。前半是**纯函数** `is_allowed`（在宿主上照样断言），
//! 后半是平台专属的系统调用（`ShellExecuteW` / `open`）。
//!
//! ## 🔴 为什么这是一条**窄语义**的命令，而不是通用的 `open_url`
//!
//! 一条"打开任意 URI"的壳命令，等于把 webview 变成一台**启动器**：前端一旦被注入，
//! 就能让壳替它拉起 `file://` / 自定义协议 / 任意站点。而本功能的**安全属性**恰恰是
//! **"客户只可能被带到那一个官方仓库"**（规格 §1 原话：*"写死之后，即使接口被篡改，
//! 客户被带到的仍然是那个官方仓库页面"*）。⇒ 命令名写成 `commands::open_update_url`
//! （**更新**那一个用途），白名单**写死在下面**，不接受任何"再放行一个域名"的参数。
//!
//! ## 🔴 白名单为什么在**这一侧**做，而不是前端
//!
//! 前端是**可被注入的那一侧**（它是浏览器里跑的那半。承诺它自己会判 = 没有承诺）。
//! 白名单是那段威胁模型里唯一还站得住的位置：**在 Rust 这条命令里**判。
//! 壳这一侧的白名单是规格 §1 那句话的第二道锁。
//!
//! ## ⚠️ 与 `reveal.rs` 同款：判据与调用分开，于是判据跨平台可测
//!
//! `is_allowed` 是纯函数（宿主上可断言）；平台调用照抄 `reveal.rs` 那套
//! —— 含 `ShellExecuteW` 那条**返回值 > 32 才算成功**的门限（判据 `crate::reveal::shell_execute_succeeded`，
//! 不在本文件里重写第二份）。

/// **允许打开的前缀** —— 官方发布页（R15 的硬要求：写死；接受任何别的域名的参数**不提供**）。
///
/// ⚠️ 末尾那个 `/` 是**承重的**：少了它，`…/releases.evil.example/x` 也会以
///    `…/releases` 开头 ⇒ 前缀匹配会被一个"看起来像同一个仓库其实是另一个站点"的
///    主机名骗过去（`is_allowed` 的用例钉着这一条）。
pub const ALLOWED_PREFIX: &str = "https://gitee.com/starsyi/benagen-downloader/releases/";

/// 一次「打开更新下载页」的结局。
///
/// ⚠️ **三档不是"成功/失败"两档**：`Refused`（不在白名单里）与 `Failed`（在白名单里、
///    但系统调用没成）是**两件要分开处置的事**（前者是纵深防御拦下的、后者是环境问题）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 已经交给系统浏览器了。
    Opened,
    /// **不在白名单里** ⇒ 拒绝，一次系统调用都不发。
    Refused,
    /// 在白名单里、但系统那一半没能把它打开（原样带回来）。
    Failed(String),
}

/// **判据**：这条 URL 准不准打开（纯函数，宿主上可测）。
///
/// ## 🔴 三道闸，**fail-closed**（2026-10-09 审查 R25）
///
/// 起初这里只有 `url.starts_with(ALLOWED_PREFIX)`。审查者指出：**它挡不住 `..`**——
/// `…/releases/../../evil`、`…/releases/../../../starsyi/fake` 都以那个前缀开头，
/// 却能把客户带离原路径（主机仍被钉在 `gitee.com`，是**路径级**逃逸，不是 authority 级，
/// 但它照样破坏了"客户只可能被带到那一个官方仓库"这条保证 —— 而前端是可被注入的那一侧，
/// 能拿任意字符串调这条命令）。⇒ 现在三道闸：
///
///   ① **前缀**（钉死 scheme + host + owner + repo + `releases/`）；
///   ② **余下部分非空**（光秃秃的 `…/releases/` 不是一个下载页，拒绝）；
///   ③ **余下部分只允许 `[A-Za-z0-9._/-]`，且不含 `..` 段**。
///
/// ⚠️ **③ 是白名单（只放行我们真实会产生的那一类字符），不是黑名单**（不逐个拦 `..` /
///    `%2e` / `\` / 别的编码 —— 那种写法永远会漏下一个）。现有三类资产名
///    （`BenagenDownloader-AppleSilicon.dmg` / `-Intel.dmg` / `-Windows-x86_64.exe`）与
///    版本段（`download/v{major}.{minor}.{patch}/`）**全在**这个字符集内 ⇒ 当下不误伤。
///    大小写、数字、`.`、`_`、`/`、`-` 都放行。
///    ⚠️ `%` 不在字符集里 ⇒ `%2e%2e`、`%2f` 这类编码**整条被拒**（连解都懒得解）。
///
/// ⚠️ 用 `starts_with` 而不是"解析成 URL 再比 host"：本项目**不引入 URL 解析依赖**
///    （`Cargo.toml` 一个字不动）。③ 那道字符集闸把"前缀之后的自由度"收到足够小，
///    于是 `starts_with` 这个近似**够用**了。
///    ⚠️ 别把它改成"包含 `gitee.com` 就行"：那会放行 `https://evil.com/?gitee.com`。
pub fn is_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix(ALLOWED_PREFIX) else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if !rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '_' | '-'))
    {
        return false;
    }
    // ③ 的后半：`..` 段（路径穿越）。⚠️ 拆段判，不是 `contains("..")` ——
    //    后者会把合法的 `x..y` 这类文件名一起拦掉（本条**没有**这种资产，但判据要判
    //    "是不是一段 `..`"，不是"出现过两个点"）。
    !rest.split('/').any(|segment| segment == "..")
}

/// 把这条 URL 交给系统浏览器。**先过白名单**（不过就一次系统调用都不发）。
pub fn open(url: &str) -> Outcome {
    if !is_allowed(url) {
        return Outcome::Refused;
    }
    match open_in_default_browser(url) {
        Ok(()) => Outcome::Opened,
        Err(why) => Outcome::Failed(why),
    }
}

/// 把 URL 交给**系统默认浏览器**。**平台专属**（见模块头）。
#[cfg(target_os = "windows")]
fn open_in_default_browser(url: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// 宽字符 + NUL 结尾（Win32 的 `LPCWSTR` 契约，同 `reveal.rs`）。
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ⚠️ 打开一个 URL 时，**URL 落在第二个参数**（`lpFile`）上，`lpParameters` 传空
    //    —— 这与 `reveal.rs` 那条（`explorer.exe` + `/select,<路径>`）是同一个 API 的
    //    两种用法：`ShellExecuteW` 对"文件/URL"走 `lpFile`、"参数"走 `lpParameters`。
    //    这里的 URL 里**没有空格**（前缀写死、版本段三段纯数字），不需要 `reveal.rs`
    //    那种加引号的处理。
    let operation = wide("open");
    let file = wide(url);
    let parameters = wide("");
    let directory = wide("");

    // SAFETY：四根指针都指向以 NUL 结尾、且在本调用期间一直存活的缓冲区；
    //         `hwnd` 传 null 是"没有父窗口"（GUI 子系统的程序，同 `reveal.rs`）。
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
    let returned = returned as isize as i32;
    // 门限与 `reveal.rs` **同一条**（> 32 才算成功）—— 判据只有那一份实现。
    if crate::reveal::shell_execute_succeeded(returned) {
        Ok(())
    } else {
        Err(format!("ShellExecuteW 返回 {returned}（不超过 32 都是错误码）"))
    }
}

/// 宿主（macOS / Linux）那一支：`open <url>` = 交给默认浏览器。
///
/// ⚠️ **只为本地开发能验**（规格 §9.0 的宿主构建）：交付形态只有 Windows exe。
///    它与 Windows 那一支**语义对位**（把 URL 交给默认浏览器），所以宿主上走一遍
///    "命令 → 白名单 → 系统调用 → 结果"这条链的形状是真的。
#[cfg(not(target_os = "windows"))]
fn open_in_default_browser(url: &str) -> Result<(), String> {
    match std::process::Command::new("open").arg(url).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("open 退出了 {status}")),
        Err(cause) => Err(format!("起不了 open：{cause}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 **白名单就是那个前缀**：官方仓库 releases 下**真会产生的**那一类 URL 都放行。
    ///
    /// 判别力：把 `ALLOWED_PREFIX` 的末尾 `/` 去掉、或整条换成更宽的东西，下面
    /// `the_neighbour_host_is_refused` 那一条立刻红。
    ///
    /// ⚠️ **光秃秃的 `…/releases/` 不在这里**（它被第 ② 道闸拒绝）—— 见 `a_bar` 那条。
    #[test]
    fn the_official_release_urls_are_allowed() {
        for url in [
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/BenagenDownloader-Windows-x86_64.exe",
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/BenagenDownloader-AppleSilicon.dmg",
            "https://gitee.com/starsyi/benagen-downloader/releases/tag/v0.2.5",
        ] {
            assert!(is_allowed(url), "官方发布页下的这条该放行：{url}");
        }
    }

    /// 🔴 **相邻主机名不许骗过前缀判据**（末尾那个 `/` 就是为了这一条）。
    ///
    /// 判别力：把 `ALLOWED_PREFIX` 末尾的 `/` 删掉 ⇒ `…/releases.evil.example/x`
    /// 会以 `…/releases` 开头、被误放行 ⇒ 这一条红。
    #[test]
    fn the_neighbour_host_is_refused() {
        assert!(
            !is_allowed("https://gitee.com/starsyi/benagen-downloader/releases.evil.example/x"),
            "「以 releases 开头」不等于「在 releases 路径下」—— 末尾的 / 是承重的"
        );
    }

    /// 🔴🔴 **路径穿越必须被拒**（2026-10-09 审查 R25 的核心：前缀**挡不住 `..`**）。
    ///
    /// 这三个输入在"只看前缀"的老实现下**全部被放行**（审查者实测），能把客户带到
    /// `gitee.com` 上的任意路径。判别力：把 `is_allowed` 放松回
    /// `url.starts_with(ALLOWED_PREFIX)`（删掉那两道闸）⇒ **这一条立刻红**。
    #[test]
    fn path_traversal_is_refused() {
        for url in [
            "https://gitee.com/starsyi/benagen-downloader/releases/../../evil",
            "https://gitee.com/starsyi/benagen-downloader/releases/../../../starsyi/fake",
            "https://gitee.com/starsyi/benagen-downloader/releases/%2e%2e/%2e%2e/evil",
            // 光天化日之下的编码穿越，以及别的越界字符（第 ③ 道闸是白名单，`%` 不在里面）：
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/..%2f..%2fevil",
            "https://gitee.com/starsyi/benagen-downloader/releases/evil\\..\\x",
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/x.exe?a=b",
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/x.exe#frag",
        ] {
            assert!(!is_allowed(url), "前缀之后**不许**有穿越/越界字符，这条必须被拒：{url}");
        }
    }

    /// 前缀本身（余下为空）不是一个下载页 ⇒ 拒绝（第 ② 道闸）。
    #[test]
    fn a_bare_releases_prefix_is_refused() {
        assert!(
            !is_allowed("https://gitee.com/starsyi/benagen-downloader/releases/"),
            "光秃秃的 `…/releases/` 后面什么都没有 —— 它不是一个下载页，拒绝"
        );
    }

    /// 别的 scheme / 主机 / 仓库 / 空串一律拒绝。
    #[test]
    fn everything_else_is_refused() {
        for url in [
            "",
            "http://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/x.exe", // 非 https
            "https://evil.example/",
            "https://gitee.com/other/benagen-downloader/releases/download/v0.2.5/x.exe", // 别的拥有者
            "file:///C:/Windows/System32/calc.exe",
            "https://gitee.com/starsyi/benagen-downloader/releases?evil",   // 少一个 /
        ] {
            assert!(!is_allowed(url), "这条**不该**放行：{url}");
        }
    }

    /// 🔴 **拒绝的 URL 一次系统调用都不发** —— 这是白名单唯一有牙的地方。
    ///
    /// ⚠️ 这条用例**不碰系统**：`open()` 在白名单那一关就返回 `Refused`，
    ///    根本走不到平台调用（宿主上也就不会真的拉起浏览器）。判别力：把 `open()` 里
    ///    那道 `if !is_allowed` 删掉 ⇒ 这条会去真的打开 `https://evil.example/`
    ///    （在宿主上是一次副作用）—— 那是要靠人眼发现的，所以**判据写在这里**。
    #[test]
    fn a_refused_url_never_reaches_the_browser() {
        assert_eq!(open("https://evil.example/"), Outcome::Refused);
        assert_eq!(open(""), Outcome::Refused);
    }
}
