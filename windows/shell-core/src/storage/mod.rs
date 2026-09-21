//! 壳的本地存储层（规格 §5.2）。
//!
//! 这一层对齐 macOS 的三个文件（`JsonFileStore.swift` / `AppPreferencesStore.swift` /
//! `BatchHistoryStore.swift`），分成三格：
//!
//! | 本模块 | 管什么 | 对齐 |
//! |---|---|---|
//! | [`json_file`] | 字节的原子读写（E-1 / E-2 两条底线） | `JsonFileStore.swift` |
//! | [`preferences`] | `preferences.json` 的形状与"未配置" | `AppPreferencesStore.swift` |
//! | [`history`] | `history.json` 的形状 + 增/改/去重/排序/淘汰 | `BatchHistoryStore.swift` |
//!
//! 外加本文件自己负责的一件事：[`dir`] —— **壳的文件放在哪个目录**。
//!
//! ## ⚠️⚠️ 存储目录**与内核 `core/src/paths.rs` 同源**：不许另造一套规则
//!
//! 内核已经有一份**平台标准目录表**（规格 §8 那张表的内核那一半，K3 的账）。
//! 壳自己再写一份"差不多的"就是**第二个真相源** —— 两份规则一旦漂移，
//! 表现是"壳的偏好在一个目录、内核的 `settings.json` 在另一个目录"，
//! 而**没有任何东西会因此变红**（客户排障时看到的只是"东西不在说好的地方"）。
//!
//! 所以这里的规则是**照 `core/src/paths.rs` 重算一遍**，逐段点名：
//!
//!   * **变量名映射**：[`env_var_name`] ←→ `paths.rs:100-110` 的 `env_var_name`
//!     在 `Platform::Windows` / `Purpose::Settings` 那一格给的是 `"APPDATA"`
//!     （壳的存储根与内核的 `settings.json` **同一个变量**：两者本来就同目录）；
//!   * **拼法**：[`dir_for`] 的 `Platform::Windows` 那一支 ←→ `paths.rs:155-181` 的
//!     `settings_file_for`（`%APPDATA%\BenagenDownloader\` + 一个文件名）；
//!     另一支 ←→ 同函数 `Platform::Other` 那半
//!     （`~/Library/Application Support/BenagenDownloader/` + 一个文件名）。
//!
//! ⚠️ **本 crate 不 `use` 那个模块**（那会让"语言中立的进程边界"降级成两个 crate 的
//!    编译期耦合，`shell-core/Cargo.toml` 的注释与规格 §5 要点 1 都写着这条），
//!    所以是**重算**而不是**调用** —— 于是下面那两条测试（变量名 + 本靶的 `PLATFORM`）
//!    就是这条"同源"声明的牙齿：它们钉的正是"重算出来的规则与内核那份是同一套"里
//!    最容易敲错、而**在 macOS 上全套测试依旧全绿**的那两处。
//!
//! ⚠️ **`core/` 一个字都没动**（本任务的硬约束）：这里只是照它的规则重算。
//!
//! ## ⚠️ 平台差异落在「以参数注入环境值的纯函数」上（与 `paths.rs` 同款）
//!
//! [`dir_for`] **一个 `#[cfg]` 都没有**：环境值（`%APPDATA%` 的内容、`$HOME` 的内容）
//! 是**参数**。于是 Windows 的路径逻辑**在 macOS 上就能被真测**，而不是等真机
//! —— 与 `core/src/paths.rs` 的"裁决 G"、以及 `embedded_core::cache_dir_from`
//! 是同一个手法。全模块只有两处 cfg：`env_value`（读环境变量）与
//! [`PLATFORM`]（选实现），**两处都用 `cfg!` 宏而不是 `#[cfg]` 属性**
//! （理由照抄 `paths.rs`：`#[cfg]` 会把另一个平台整个删掉，于是 `Platform` 的另一个
//! 变体成了"从未被构造"，在本靶上凭空多出一条 `dead_code` 告警 ——
//! 而"残留告警不得超出基线"是硬约束，且不许用 `#[allow(dead_code)]` 去按）。
//!
//! ## ⚠️ 取不到标准目录时：**大声失败**，不悄悄退到相对路径（W-2）
//!
//! 两个平台**都是** [`Err`]。与 `paths.rs` 有一处**有意不同**（W-6，理由如下）：
//! 那边 `Platform::Other`（macOS / Linux）取不到 `$HOME` 时**回退**到既有的相对路径
//! （那是**冻结的 Go 行为**，有从 Go 移植来的测试钉着，改了就是破坏 W-3）。
//! 壳这边**没有**那条历史包袱，而 macOS 自己的壳（`ShellStorage.directory`）用的是
//! `FileManager.homeDirectoryForCurrentUser` 兜底 —— Rust 的 `std` 没有对应物
//! （`std::env::home_dir` 已废弃）。于是只剩两条路：悄悄退到一个**相对路径**
//! （双击启动时"当前工作目录"是 exe 所在目录），或者大声失败。选后者：
//! 把用户的偏好写到进程的当前工作目录里，与 `paths.rs` 那个 W-2 要拦的形状
//! **一模一样**（"文件被写到一个用户永远找不到的地方"）。

//! ## ⚠️ 本模块**字面量**里的用字受内嵌字体子集的约束（不是文风问题，别改回去）
//!
//! `windows/scripts/make_font_subset.sh --check`（`test.sh` 第 0.5 步的硬闸）要求
//! **源码里的字面量用字 ⊆ 已入库的 `windows/assets/ui-subset.otf`** ——
//! 少一个字，那台机器上就是一块**豆腐块**，而编译与其余测试**全都过得去**。
//!
//! 抽取器只认**字面量**（不认注释，`required_chars` 的 `lex()` 就是这条的判据），
//! 所以**测试断言里的字也在要求集里**。本文件的几条错误话术与 `history.rs` 测试模块里
//! 那几句断言用的是「**记录**」「**批注**」「**存放根**」「**新添**」「**写的先后**」
//! 「**丢掉**」这些词 —— 它们**全部**是被这条判据挑剩下的写法，不是笔误、也不是口味：
//! 「历史」「备注」「存储」「新增」「淘汰」「插入」这些更顺口的词里各有一两个
//! 不在子集内的字。**改文案之前先跑那条 `--check`**，或者连同字体子集一起重生成
//! （那是**另一个任务**的边界：本层不负责刷新 `windows/assets/` 那份资产）。
//!
//! ⚠️ 这一节写在这里，是因为"为什么这里不用更常见的那个词"这个问题**一定会**被问到，
//!    而答案不在任何一行代码里（`history.rs` 的测试模块里有一条短指针指回本段）。

pub mod history;
pub mod json_file;
pub mod preferences;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 壳的目录名 —— 与内核 `settings.json` 所在的那个目录**同名同源**
/// （`core/src/paths.rs` 里那三处 `join("BenagenDownloader")`）。
const APP_DIR_NAME: &str = "BenagenDownloader";

/// 平台族：**标准目录的取法**按它分叉（与 `paths.rs` 的 `Platform` 逐格对应，
/// 只是那边还按用途分了三个变量、壳这里只有一种用途）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    /// Windows：`%APPDATA%`。
    Windows,
    /// 其余（macOS / Linux）：`$HOME`。
    Other,
}

/// 本靶属于哪一族 —— **"选实现"的那一处**（与 `paths.rs:53-57` 的 `PLATFORM` 同形）。
///
/// 用 `cfg!` 而不是 `#[cfg]`：理由见模块头（`#[cfg]` 会在本靶上造出一条新的
/// "变体从未被构造"的 `dead_code` 告警）。
const PLATFORM: Platform = if cfg!(target_os = "windows") {
    Platform::Windows
} else {
    Platform::Other
};

/// **纯函数**：`platform` 该读哪个环境变量。
///
/// ⚠️ **cfg-free，而且必须 cfg-free**（理由与 `paths.rs` 里同名那条注释逐字相同）：
/// 内联进 `env_value` 的分支里的话，"`"APPDATA"` 这七个字母写对了没有"在 macOS 上
/// **没有任何东西在验** —— `cfg!` 只保证那段代码被**类型检查**，字符串内容它管不着。
/// 把平台当参数之后，"Windows 该读哪个变量"在**任何宿主上**都能被真测。
fn env_var_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "APPDATA",
        Platform::Other => "HOME",
    }
}

/// 读**标准目录**那条环境变量 —— 本模块唯一碰环境的地方（与 `paths.rs` 的
/// `env_value` 同形；那边还多一处 `temp_dir()`，壳这边没有）。
fn env_value() -> Option<OsString> {
    std::env::var_os(env_var_name(PLATFORM))
}

/// 非空才算"取到了"。
///
/// Windows 的环境变量可以**存在且为空串**（`var_os` 返回 `Some("")`），空串拼出来的
/// 路径与相对路径无异 —— 那正是 W-2 要拦的形状（同 `paths.rs` 的 `non_empty`）。
fn non_empty(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

/// **壳的文件放在哪个目录**。
///
/// - Windows：`%APPDATA%\BenagenDownloader\`；
/// - 其余：`~/Library/Application Support/BenagenDownloader/`。
///
/// ⚠️ 它**只构造路径、不碰盘**（同 `BatchHistoryStore.defaultURL` 那条注释）：
///    "有没有那个目录"是运行期才知道的事，由 [`json_file::JsonFile::write`] 在写之前
///    建出来（E-2 / 首次运行）。
/// ⚠️ **调用方拿到的是目录**，文件名由各模块自己的 `FILE_NAME` 拼
///    （`preferences.json` / `history.json` 是**两个文件**，格式与生命周期各自负责）。
pub fn dir() -> Result<PathBuf, String> {
    dir_for(PLATFORM, env_value())
}

/// 一条"取不到"的错误：点名变量、说清后果、给一条**可执行**的补救（W-2）。
///
/// `remedy` 必须是**下一步能做的事** —— 一句"请设置环境变量"不算补救
/// （话术形状照 `paths.rs:71-80` 的 `missing`，但**内容**是壳自己的：那条命的后果
/// 是"偏好与历史存不下来"，与内核的"参数读不到"不是同一件事）。
fn missing(name: &str, remedy: &str) -> String {
    format!(
        "取不到 %{name}%，无法确定壳的配置与记录该放在哪里。\n\
         Windows 上 %{name}% 是系统的标准变量，正常由登录会话设好；\
         在服务/计划任务里启动、或环境被裁剪时会缺失。\n\
         补救：{remedy}\n\
         ⚠️ 这里**不会**退回相对路径：双击启动时\"当前工作目录\"是 exe 所在目录\
         （可能是 Program Files，可能不可写），那样会把偏好与记录写到一个用户永远找不到的地方\
         —— 而那正是内核 `core/src/paths.rs` 的 W-2 要拦的形状。"
    )
}

/// 纯函数：`dir()` 的全部判据（**平台与环境值都是参数**，于是本机可测）。
///
/// 规则逐段对齐 `core/src/paths.rs`（见模块头那张点名表）：
///   * Windows ←→ `settings_file_for` 的 `Platform::Windows` 那一支（`paths.rs:155-181`）；
///   * 其余 ←→ 同函数 `Platform::Other` 那一支。
///
/// ⚠️ 两支都不做"路径规范化"（不展开 `~`、不去 `..`、不动分隔符）：
///    这里拼出来的是**我们自己的**目录，交给 `std::fs` 直接用即可。
fn dir_for(platform: Platform, env: Option<OsString>) -> Result<PathBuf, String> {
    match platform {
        Platform::Windows => {
            let base = non_empty(env).ok_or_else(|| {
                missing(
                    "APPDATA",
                    "① 设好 %APPDATA%（通常形如 C:\\Users\\<用户名>\\AppData\\Roaming）；\
                     ② 若在服务/计划任务里启动，请补上用户环境块。",
                )
            })?;
            Ok(PathBuf::from(base).join(APP_DIR_NAME))
        }
        Platform::Other => {
            let home = non_empty(env).ok_or_else(|| {
                missing("HOME", "① 设好 $HOME；② 或 `HOME=/Users/<用户名> ./BenagenDownloader` 显式给它一个值。")
            })?;
            Ok(Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR_NAME))
        }
    }
}

/// 测试用的临时目录（**不引 `tempfile`**：本 crate 的直接依赖白名单只有
/// `serde` / `serde_json`，`windows/scripts/test.sh` 有守卫会拦）。
///
/// ⚠️ **它存在本身就是一条纪律**：壳的存储目录在真机上放着人类伙伴**真实在用**的
///    偏好与历史，测试**一律**要把句柄指到临时目录去（macOS 侧
///    `JsonFileStore.swift` / `AppPreferencesStore.swift` / `BatchHistoryStore.swift`
///    三个文件头都用同一句话点这条）。所以下面每个用例都不接受"默认位置"这个选项。
///
/// 形态照抄 `embedded_core.rs` 测试里那个同名助手（同一套 tag + pid + 纳秒的命名法），
/// 理由也一样：本 workspace 不许为测试引依赖。
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    /// 一个用完就删的临时目录。
    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        /// 建一个**本次运行唯一**的目录（`tag` 只做人眼区分用）。
        pub(crate) fn new(tag: &str) -> TempDir {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "benagen-storage-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p).expect("建临时目录");
            TempDir(p)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 上这两个标准变量的真身（形状取自文档，不是从本机读的）。
    const WIN_APPDATA: &str = r"C:\Users\x\AppData\Roaming";

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    /// 壳的存储根落在 `%APPDATA%\BenagenDownloader\`（规格 §5.2）。
    ///
    /// ⚠️ 期望值用 `PathBuf::join` 拼，**不能写成 `r"C:\…\BenagenDownloader"` 字面量**：
    /// 在 macOS 上 `\` 是**普通字符**而不是分隔符，拼出来的 PathBuf 与字面量不是同一个东西
    /// （这条坑 `core/src/paths.rs` 的测试里记着，本文件照抄它的写法）。平台的**取值**
    /// 由 `Platform` 参数注入、分隔符由宿主决定 —— 这正是"纯函数能在本机真跑"的收益。
    #[test]
    fn windows_storage_dir_uses_appdata() {
        let got = dir_for(Platform::Windows, os(WIN_APPDATA)).expect("给了 %APPDATA% 就应当成功");
        let want = PathBuf::from(WIN_APPDATA).join("BenagenDownloader");
        assert_eq!(
            got, want,
            "壳的存放根必须与内核 settings.json 同目录（%APPDATA%\\BenagenDownloader\\）"
        );
    }

    /// W-2：`%APPDATA%` 取不到（或存在且为空串）⇒ **大声失败**，话术要点名缺的是哪个变量。
    #[test]
    fn windows_storage_dir_fails_loudly_without_appdata() {
        let err = dir_for(Platform::Windows, None).unwrap_err();
        assert!(err.contains("APPDATA"), "错误话术必须点出缺的是哪个变量：{err}");
        let err = dir_for(Platform::Windows, os("")).unwrap_err();
        assert!(err.contains("APPDATA"), "空 %APPDATA% 也必须大声失败：{err}");
        // 话术必须说清"这里不会退到相对路径"（W-2 的实质是**不静默降级**）。
        assert!(err.contains("相对路径"), "必须说清不会退回相对路径：{err}");
    }

    /// 其余平台：`~/Library/Application Support/BenagenDownloader/`
    /// （与 macOS 的 `ShellStorage.directory`、内核 `paths.rs` 的 `Platform::Other` 同一条）。
    #[test]
    fn other_storage_dir_uses_the_canonical_home() {
        let got = dir_for(Platform::Other, os("/Users/x")).expect("有 $HOME 就应当成功");
        let want = Path::new("/Users/x")
            .join("Library")
            .join("Application Support")
            .join("BenagenDownloader");
        assert_eq!(got, want, "其余平台的壳目录必须与内核 settings.json 同目录");
    }

    /// 其余平台取不到 `$HOME` ⇒ **也大声失败**（壳这边没有 Go 那条冻结的回退行为，
    /// 理由写在模块头：悄悄退到相对路径会把偏好写到 exe 所在目录）。
    #[test]
    fn other_storage_dir_fails_loudly_without_home() {
        let err = dir_for(Platform::Other, None).unwrap_err();
        assert!(err.contains("HOME"), "错误话术必须点出缺的是哪个变量：{err}");
        assert!(dir_for(Platform::Other, os("")).is_err(), "空 $HOME 与未设同义");
    }

    // -----------------------------------------------------------------------
    // "与内核 paths.rs 同源"这条声明的牙齿
    //
    // 上面四条用例**全部**显式注入 `Platform` 与环境值 ⇒ 它们验的是"拿环境值拼路径"
    // 那一半。而"**读哪个变量**"与"**本靶属于哪一族**"这两处，若不钉住，
    // 敲错一个字母在 macOS 上**全套测试依旧全绿**，要到客户机器上才以
    // "取不到 %APPDATA%"（或更糟：读到别的变量的值）暴露。
    // -----------------------------------------------------------------------

    /// Windows 上读的是 `APPDATA` —— **与 `core/src/paths.rs:100-110` 的
    /// `Purpose::Settings` 那一格逐字相同**（壳的存储根与内核的 `settings.json` 同目录，
    /// 所以必须读同一个变量）。判别力：改掉这个字面量 ⇒ 必红。
    #[test]
    fn the_windows_variable_is_appdata_same_as_the_kernel() {
        assert_eq!(env_var_name(Platform::Windows), "APPDATA");
        assert_eq!(env_var_name(Platform::Other), "HOME");
    }

    /// **本靶的 `PLATFORM` 必须是本靶**，且 `env_value` 真的去读了那个变量。
    ///
    /// ⚠️ 形状照抄 `paths.rs` 的同名用例（那一条是"两处此前无人钉"的补网）：
    ///    把 `PLATFORM` 改成恒返回 `Platform::Other`，上面四条用例**依旧全绿**，
    ///    因为 `Platform` 都是显式传进去的。
    #[test]
    fn host_platform_and_the_variable_it_reads_are_pinned() {
        // `cfg!` 来自编译器、`PLATFORM` 来自本模块，是**两份独立的表述**：
        // 它们一致的唯一可能就是 `PLATFORM` 写对了。
        assert_eq!(
            PLATFORM == Platform::Windows,
            cfg!(target_os = "windows"),
            "PLATFORM 与本靶的 target_os 不符——\"选实现\"那一处漂了"
        );

        let want = if cfg!(target_os = "windows") { "APPDATA" } else { "HOME" };
        assert_eq!(env_var_name(PLATFORM), want, "本靶该读的变量名不对");
        // 真读一次。本机设了 `want` 就必须逐字相符；没设就得两边都是 `None`
        // ——后者同样有判别力：它排除"env_value 读的是另一个恰好有值的变量"。
        // 本仓库没有任何测试改环境变量，所以这里没有并发竞态（`paths.rs` 的同一句话）。
        match std::env::var_os(want) {
            Some(got) => assert_eq!(env_value(), Some(got), "env_value 读的不是 %{want}%"),
            None => assert_eq!(env_value(), None, "环境里没有 %{want}%，env_value 却取到了值"),
        }
    }

    /// ⚠️ 两个 `FILE_NAME` 是**两个文件、同一个目录**（规格 §5.2 与 macOS 的三份文件头）。
    ///    它们还不同名 —— 合成一个会把"偏好"与"历史"的生命周期绑在一起。
    #[test]
    fn the_two_files_live_in_one_directory_but_are_two_files() {
        assert_eq!(preferences::FILE_NAME, "preferences.json");
        assert_eq!(history::FILE_NAME, "history.json");
        assert_ne!(preferences::FILE_NAME, history::FILE_NAME);
    }
}
