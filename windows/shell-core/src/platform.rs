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

#[cfg(test)]
mod tests {
    //! 上游没有对应物（上游只跑在一个平台上）。这里钉的是**"平台差异被参数化"这件事**：
    //! 两支都要在**任意宿主上**有断言 —— 这正是搬进本模块之前缺失的那半条判别力。

    use super::{env_var_name, Platform, PLATFORM};

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
}
