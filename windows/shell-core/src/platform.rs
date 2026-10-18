//! platform —— **平台族的判定与"平台差异"的唯一落点**（`shell-core` 里唯一一处 `cfg!`）。
//!
//! 为什么这个模块存在（控制者裁定 JJ）：**`cfg!(windows)` 的那一支在本机根本不执行**。
//! 本 crate 的测试跑在 macOS 上，`cfg!(windows)` 恒为 `false` —— 于是"Windows 上该读哪个
//! 环境变量"这件事**没有任何东西在验**（`cfg!` 只保证那段代码被类型检查，
//! 不保证 `"USERPROFILE"` 这十一个字母写对了）。
//!
//! ⚠️ 这条纪律在本仓库里**已经有两处先例**，本模块是第三处，形状照抄它们：
//!    · `shell-win/src/spawn.rs` 的 `creation_flags_for(os)` —— 注释原话：
//!      "**为什么吃一个平台参数，而不是直接读 `cfg!(windows)`**……`cfg!(windows)` 在
//!      macOS 上恒为 `false`，那条分支在本机**根本不执行**，测试就只能断言 `0`，
//!      一点信息都没有。做成纯函数之后，**本机能断言 Windows 那一支的返回值**"；
//!    · `core/src/paths.rs` 的 `env_var_name(purpose, platform)`（**cfg-free 纯函数**）
//!      + `PLATFORM` 常量（`:53-57`，那里唯一一处 `cfg!`）——本模块逐字对位它。
//!
//! ⚠️ **判断"该不该搬到这里"的判据**：一段代码里有 `cfg!`／`#[cfg]` 时，先问
//!    "另一支能不能被写成**吃的参数**？"能，就搬进来（决策留下、应用留给调用方）——
//!    §4.3 那条"能写出断言的代码一律搬出平台目标"落在几十行代码上就是这个意思。

/// 平台族：**取标准目录的方式**按它分叉。
///
/// ⚠️ **W-6（来源）**：上游（macOS 版 `TransferRow.swift`）没有这个概念 —— 它只跑在一个
///    平台上，`home(environment:fallback:)` 直接读 `"HOME"`。本枚举对位的是**内核**
///    （`core/src/paths.rs` 的 `enum Platform`，同样两个变体、同样不派生 serde），
///    因为"壳与内核取同一个 home 源"这条约定在 Windows 上要跨平台成立。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows：home 取自 `%USERPROFILE%`。
    Windows,
    /// 其余（macOS / Linux）：home 取自 `$HOME`。
    Other,
}

/// 本靶属于哪一族 —— **"选实现"的那一处，也是本 crate 唯一一处 `cfg!`**。
///
/// 用 `cfg!` 而不是 `#[cfg]`：理由同内核那处（`#[cfg]` 会在本靶上造出一条
/// "变体从未被构造"的 `dead_code` 告警，而本 crate 是零告警纪律）。
pub const PLATFORM: Platform = if cfg!(target_os = "windows") {
    Platform::Windows
} else {
    Platform::Other
};

/// **纯函数**：`platform` 上，取 home 该读哪个环境变量名。
///
/// ⚠️ **必须 cfg-free，而且必须能被任意宿主调用**：这是本函数存在的**全部理由** ——
///    做成参数之后，"Windows 该读 `USERPROFILE`"在 macOS 上**也能被真测**
///    （见本模块的 `env_var_name_pins_both_branches_on_any_host`）。
///    把它内联回 `if cfg!(windows)` 会让那条断言重新变回"只能断言 0"。
///
/// ⚠️ **逐字对位 `core/src/paths.rs` 的 `env_var_name(purpose, platform)`**：
///    对应的是那里 `Purpose::Downloads` 那一格（`Windows => "USERPROFILE"`、
///    `Other => "HOME"`）。**内核改那一格，这里要跟着改** —— 两边一旦分叉，
///    壳算出来的落盘路径就指向一个不存在的文件，而"定位到磁盘上的文件"指到空处
///    正是简报点名不要的静默无效（这条耦合由 `transfer_row.rs` 的
///    `the_download_root_uses_the_same_home_source_as_the_kernel` 钉住）。
///
/// ⚠️ 参数现在是**平台**而不是内核那边的**用途**：本 crate 只需要一个用途（home），
///    再多一个"用途"时请照内核的形状扩参（`env_var_name(purpose, platform)`），
///    而不是在这里再长一个函数。
pub const fn env_var_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "USERPROFILE",
        Platform::Other => "HOME",
    }
}

/// **纯函数**：`platform` 上，路径里那一级分隔符。
///
/// ⚠️ **这条判据的来历是一次真实的定位失败**（"在资源管理器中显示"点了没反应，只在
///    Windows 上）：那一刻交给资源管理器的**是一个字符串参数**，而解析它的是**那个程序**，
///    不是宿主 —— MS-DOS 一脉的程序把 `/` 当**选项引导符**（GNU Emacs bug#66514 里
///    Eli Zaretskii 的原话："MS-Windows programs frequently treat the forward slash as the
///    option character"；Qt 的 `Use backslashes for UNC paths` 是同一条）。于是
///    `/select,"C:\Users\x/Downloads/Benagen/a.bin"` 被它切成几截、**不报错**，
///    只打开一个默认窗口 —— 而"点了什么都没定位到"正是约束 4 明禁的静默失效。
///    macOS 那一支（`open -R`）拿到的路径本来就是纯 `/`，所以这个缺陷**只在 Windows 上显形**，
///    在开发机上永远看不出（同 [`env_var_name`] 那条"本机测不到"的论证）。
///
/// ⚠️ **方向是"内核怎么落地、壳就怎么写"**：内核用 `PathBuf::join` 拼默认下载目录
///    （`core/src/paths.rs` 的 `download_dir_for`），在 Windows 靶上出来的就是 `\`。
///    壳这一侧镜像它 —— 这条耦合由 `transfer_row.rs` 的
///    `a_windows_path_uses_backslashes_for_the_file_manager` 钉住（那条用例在**任意宿主上**
///    都断言 Windows 那一支的逐字形状）。
pub const fn path_separator(platform: Platform) -> char {
    match platform {
        Platform::Windows => '\\',
        Platform::Other => '/',
    }
}

/// **纯函数**：`c` 在 `platform` 上算不算分隔符。
///
/// ⚠️ Windows 上 `/` 与 `\` **都是**分隔符（内核的 `PathBuf` 在那一靶上同样两个都认），
///    其余平台上 `\` 是一个**普通的文件名字符**（所以那里的路径一个字符都不许动）。
///    本函数只用来去掉**壳自己拼出来的**那个末尾分隔符，不是拿来解析路径的。
pub const fn is_path_separator(platform: Platform, c: char) -> bool {
    c == path_separator(platform) || (matches!(platform, Platform::Windows) && c == '/')
}

/// **纯函数**：`path` 在 `platform` 上是不是一条**绝对**路径。
///
/// ⚠️ 判据搬到这里的理由同 [`env_var_name`]：`std::path::Path::is_absolute` 用的是
///    **宿主**的文件系统语义，而调用方（`api::reveal::local_path` 的前置闸）问的是
///    **目标平台**的事 —— 在 macOS 上问 `C:\Users\x` 它回 `false`，
///    于是"Windows 上这条路算不算绝对"在本机**永远不可断言**，两条分支里只有一条被测到。
///
/// ⚠️ 写法是**字符串判据**，不是 OS 调用：盘符（`C:\` / `C:/`）与 UNC（`\\server\share`）
///    两种绝对形式。
pub fn is_absolute(platform: Platform, path: &str) -> bool {
    match platform {
        Platform::Other => path.starts_with('/'),
        Platform::Windows => {
            let bytes = path.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/');
            drive || path.starts_with(r"\\")
        }
    }
}

#[cfg(test)]
mod tests {
    //! 上游没有对应物（上游只跑在一个平台上）。这里钉的是**"平台差异被参数化"这件事**：
    //! 两支都要在**任意宿主上**有断言 —— 这正是搬进本模块之前缺失的那半条判别力。

    use super::{env_var_name, is_absolute, is_path_separator, path_separator, Platform, PLATFORM};

    /// 两支逐字钉死，**在任何宿主上都执行**。
    ///
    /// 判别力：把 `env_var_name` 内联回 `cfg!(windows)`（或在两个分支里把两个名字写反），
    /// 这一条立刻红 —— 而在此之前，写反了也**没有任何测试会红**（本机永远是 `Other` 那一支）。
    #[test]
    fn env_var_name_pins_both_branches_on_any_host() {
        assert_eq!(
            env_var_name(Platform::Windows),
            "USERPROFILE",
            "Windows 上内核取 home 读的是这个变量（core/src/paths.rs 的 Purpose::Downloads）"
        );
        assert_eq!(
            env_var_name(Platform::Other),
            "HOME",
            "其余平台上内核取 home 读的是这个变量"
        );
        assert_ne!(
            env_var_name(Platform::Windows),
            env_var_name(Platform::Other),
            "两个平台读同一个变量的话，「取同一个源」这条要求就无从谈起了"
        );
    }

    /// `PLATFORM` 与本靶一致。
    ///
    /// ⚠️ 这一条的判别力**不在于分支覆盖**（`PLATFORM` 在本机只可能是 `Other`），而在于：
    ///    把 `PLATFORM` 写死成 `Platform::Windows`（或把两个变体写反）会**立刻红** ——
    ///    那正是"本机测不到"的那个错误最可能的样子。
    ///
    /// ⚠️ 期望值取自 `std::env::consts::OS` 而**不是** `cfg!(target_os = "windows")`：
    ///    后者会让 `cfg!` 从 1 处变成 2 处，而裁定 JJ 要的是"**全 crate 唯一一处**"。
    ///    `std::env::consts::OS` 是**编译期常量**（目标三元组里的 OS），
    ///    **不是 OS 调用**（本 crate 的章程禁的是后者，见 `lib.rs` 头部）。
    #[test]
    fn the_platform_constant_matches_the_target() {
        let want = if std::env::consts::OS == "windows" {
            Platform::Windows
        } else {
            Platform::Other
        };
        assert_eq!(PLATFORM, want);
    }

    /// 🔴 两支逐字钉死，**在任何宿主上都执行**。
    ///
    /// 判别力（这是缺陷 1 的那一条）：把 Windows 那一支改回 `'/'`（也就是修之前的写法），
    /// 这一条立刻红 —— 而它对应的真机现象是"点了『在资源管理器中显示』，资源管理器开了，
    /// 但没选中任何文件"（explorer.exe 把 `/` 当选项引导符，路径被切成几截、**不报错**）。
    #[test]
    fn the_path_separator_pins_both_branches_on_any_host() {
        assert_eq!(
            path_separator(Platform::Windows),
            '\\',
            "交给 explorer 的参数必须用 `\\`（它按 MS-DOS 的约定把 `/` 当选项引导符）"
        );
        assert_eq!(
            path_separator(Platform::Other),
            '/',
            "其余平台（含交付形态之外的宿主）就是 `/`"
        );
        assert_ne!(
            path_separator(Platform::Windows),
            path_separator(Platform::Other),
            "两个平台同一个分隔符的话，这条判据就白设了"
        );
    }

    /// Windows 上 `/` 与 `\` 都算分隔符；其余平台上 `\` **是普通字符**。
    ///
    /// 判别力：把 `is_path_separator` 写成"只认 `path_separator` 那一个"，
    /// 用户配了一个 `D:/交付` 时末尾那个 `/` 就留在路径里（拼出 `D:/交付\a.bin`）；
    /// 反过来，若让它无条件认 `\`，macOS 上一个名字里真带 `\` 的文件会被削掉尾字符。
    #[test]
    fn only_windows_treats_the_forward_slash_as_a_separator_too() {
        assert!(is_path_separator(Platform::Windows, '/'));
        assert!(is_path_separator(Platform::Windows, '\\'));
        assert!(is_path_separator(Platform::Other, '/'));
        assert!(
            !is_path_separator(Platform::Other, '\\'),
            "`\\` 在其余平台上是一个普通文件名字符"
        );
    }

    /// 绝对路径的判据两支都钉住，且**不借宿主的文件系统语义**。
    ///
    /// 判别力：把 `is_absolute` 换回 `std::path::Path::is_absolute`，
    /// 最后那一条（Windows 的 `C:\Users\x`）在 macOS 上立刻红 —— 而那正是"在开发机上
    /// 永远测不到 Windows 那一支"的老毛病。
    #[test]
    fn the_absolute_rule_is_the_platforms_own_not_the_hosts() {
        assert!(is_absolute(Platform::Windows, r"C:\Users\x"));
        assert!(is_absolute(Platform::Windows, "C:/Users/x"));
        assert!(is_absolute(Platform::Windows, r"\\server\share"));
        assert!(!is_absolute(Platform::Windows, r"Users\x"));
        assert!(!is_absolute(Platform::Windows, "C:"));
        assert!(!is_absolute(Platform::Windows, ""));

        assert!(is_absolute(Platform::Other, "/Users/x"));
        assert!(!is_absolute(Platform::Other, "Users/x"));
        assert!(
            is_absolute(Platform::Windows, r"C:\Users\x"),
            "Windows 上这条路是绝对的 —— 宿主是 macOS 时它照样得是绝对的"
        );
    }
}
