//! 平台标准目录 —— **内核里唯一按平台分叉的路径表**（规格 §8 那张表的内核那一半）。
//!
//! 四处落点（都经本模块，**别在别处再写第二条判据**）：
//!   - [`settings_file`]：内核参数 `settings.json`（`last_code` 由它同目录派生）；
//!   - [`cache_dir`]：内嵌 aria2c 的释放位置；
//!   - [`download_dir`]：没给 `--download-dir` 时的默认下载目录；
//!   - [`diagnostics_log`]：内核的诊断日志（**由 [`settings_file`] 同目录派生**，不新开目录）。
//!
//! 壳自己那几份（`preferences.json` / `history.json` / 内嵌内核 exe 的释放位置）**不在这里**——
//! 按规格 §8 的"谁拥有"那一列，它们归 `windows/shell-core`。
//!
//! # 平台差异落在「以参数注入环境值的纯函数」上（控制者裁决 G）
//!
//! 三个 `*_for(…)` **一个 `#[cfg]` 都没有**：环境值（`%APPDATA%` 的内容、`$HOME` 的内容、
//! 临时目录）全部是**参数**。于是 Windows 的路径逻辑**在 macOS 上就能被真测**，
//! 而不是等真机——与"`shell-core` 是纯逻辑所以能在 macOS 上跑"是同一个手法。
//! 全模块只有两处 cfg：`env_value`（读环境变量）与 `PLATFORM`（选实现）。
//!
//! ⚠️ **这两处都用 `cfg!` 宏而不是 `#[cfg]` 属性，这是有意的、有代价换来的**：
//! `#[cfg]` 会把另一个平台从编译里**整个删掉**，于是 `Platform` 的另一个变体成了
//! "从未被构造"，在**本靶**上凭空多出一条 `dead_code` 告警——而"残留告警不得超出基线那 4 条"
//! 是硬约束（裁决 D-4，且不许用 `#[allow(dead_code)]` 去按）。`cfg!` 让两个分支都参与编译
//! （非本靶那条只是 const 折叠后不可达），两个变体在两边都"被构造"，告警净增 0。
//! 副作用是**要的**：非本靶那条分支同样被类型检查。
//!
//! # 两个平台的失败语义**有意不同**（W-3 与 W-2 的交点，别"统一"它们）
//!
//!   - **Windows**：标准目录取不到 ⇒ **大声失败**，错误里点名缺的是哪个变量、并给出可执行的补救。
//!     理由：Windows 上双击启动时"当前工作目录"是 exe 所在目录（可能是 `Program Files`，
//!     可能不可写），悄悄退到相对路径会把数据丢到一个**用户永远找不到**的地方。
//!   - **其余（macOS / Linux）**：`$HOME` 取不到 ⇒ **回退**到既有的相对/临时路径。
//!     这是**冻结的 Go 行为**（Go 侧 `os.UserHomeDir()` 取不到时走的就是那几条回退），
//!     有从 Go 移植来的测试钉着，改了就是破坏 W-3。
//!     ⚠️ **三条路的回退各不相同**，它们是各自从 Go 的调用点逐字抄来的：
//!     `settings.json`（`settings.DefaultPath`）、`os.TempDir()/BenagenDownloader`
//!     （`engine.go`）、`benagen-downloads`（阶段 A 起的既有行为）。**别顺手"统一"**。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 平台族：**标准目录的取法**按它分叉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    /// Windows：标准目录取不到 ⇒ 大声失败（W-2）。
    Windows,
    /// 其余（macOS / Linux）：标准目录取不到 ⇒ 回退（冻结的 Go 行为，见模块头）。
    Other,
}

/// 本靶属于哪一族——**"选实现"的那一处**。
///
/// 用 `cfg!` 而不是 `#[cfg]`：理由见模块头（`#[cfg]` 会在本靶上造出一条新的
/// "变体从未被构造"的 `dead_code` 告警）。
const PLATFORM: Platform = if cfg!(target_os = "windows") {
    Platform::Windows
} else {
    Platform::Other
};

/// 三条路各自的用途——**只用来挑环境变量名**（Windows 上三者各有各的标准变量）。
#[derive(Debug, Clone, Copy)]
enum Purpose {
    Settings,
    Cache,
    Downloads,
}

/// 一条"取不到"的错误：点名变量、说清后果、给一条**可执行**的补救（W-2）。
///
/// `remedy` 必须是**下一步能做的事**（设哪个变量、加哪个命令行开关）——
/// 一句"请设置环境变量"不算补救。
fn missing(name: &str, what: &str, remedy: &str) -> String {
    format!(
        "取不到 %{name}%，无法确定{what}。\n\
         Windows 上 %{name}% 是系统的标准变量，正常由登录会话设好；\
         在服务/计划任务里启动、或环境被裁剪时会缺失。\n\
         补救：{remedy}\n\
         ⚠️ 这里**不会**退回相对路径：双击启动时\"当前工作目录\"是 exe 所在目录\
         （可能是 Program Files，可能不可写），那样会把文件写到一个用户永远找不到的地方。"
    )
}

/// 非空才算"取到了"。
///
/// Windows 的环境变量可以**存在且为空串**（`var_os` 返回 `Some("")`），空串拼出来的路径
/// 与相对路径无异——那正是 W-2 要拦的形状。Go 的 `os.UserHomeDir()` 对空 `$HOME`
/// 也是当"取不到"处理（`Getenv("HOME") != ""`）。
fn non_empty(v: Option<OsString>) -> Option<OsString> {
    v.filter(|v| !v.is_empty())
}

/// **纯函数**：`purpose` 在 `platform` 上该读哪个环境变量名。
///
/// ⚠️ **cfg-free，而且必须 cfg-free**（这是本模块第三条"平台差异落在以参数注入环境值的
/// 纯函数上"的落点，裁决 G）：这段映射此前**内联在 `env_value` 的 `if cfg!(…windows)`
/// 分支里**，于是**没有任何东西在验那些字符串**——本模块的 10 条用例全部显式注入
/// `Platform::Windows`（它们验的是**拿环境值拼路径**那一半），从不调用 `env_value`；
/// 而 `cfg!` 只保证那段代码被**类型检查**，不保证 `"APPDATA"` 这七个字母写对了。
/// 把平台当参数之后，"Windows 该读哪三个变量"在**任何宿主上**都能被真测。
/// 这是补守卫，不是修 bug——映射本身逐字对过规格 §8 的表，当前没有缺陷。
fn env_var_name(purpose: Purpose, platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => match purpose {
            Purpose::Settings => "APPDATA",
            Purpose::Cache => "LOCALAPPDATA",
            Purpose::Downloads => "USERPROFILE",
        },
        // 其余平台三条路都从 `$HOME` 派生（对应 Go 的 `os.UserHomeDir()`）。
        Platform::Other => "HOME",
    }
}

/// 读**标准目录**那条环境变量（APPDATA / LOCALAPPDATA / USERPROFILE / HOME）——
/// 本模块唯一经由 [`env_var_name`] 取值的地方，也是"平台差异注入"的另一处。
///
/// ⚠️ **措辞订正**（原写"本模块唯一碰环境的地方"，**已不完全成立**）：下面的
/// [`cache_dir`] 还调了 `std::env::temp_dir()`（Unix 上读 `TMPDIR`），所以本模块
/// 碰环境的入口**有两个**。为什么值得专门订正：一句"唯一"会让后来者不再去查另一处——
/// 而 `cache_dir()` 那次读同样受环境裁剪影响（那条回退的注入与它自己的理由写在
/// [`cache_dir`] 上）。承诺了代码没有的性质，比不写注释更糟。
///
/// `purpose` 只在 Windows 那几格参与取值，但**不能**因此在别处把参数删掉：
/// 删了就变成三条路各自读环境，"变量名只由一处决定"这条性质就没了。
/// 变量名由 **cfg-free 的** [`env_var_name`] 给，本函数只负责"读"这一下。
fn env_value(purpose: Purpose) -> Option<OsString> {
    std::env::var_os(env_var_name(purpose, PLATFORM))
}

/// 内核参数 `settings.json` 的路径。
pub fn settings_file() -> Result<PathBuf, String> {
    settings_file_for(PLATFORM, env_value(Purpose::Settings))
}

/// 内嵌 aria2c 的释放目录。
///
/// 临时目录也**由调用方注入**：`std::env::temp_dir()` 同样是一个环境值
/// （Unix 上读 `TMPDIR`），注进去那条回退才在本机可测。
pub fn cache_dir() -> Result<PathBuf, String> {
    cache_dir_for(PLATFORM, env_value(Purpose::Cache), &std::env::temp_dir())
}

/// 默认下载目录（`--download-dir` 未给时）。
pub fn download_dir() -> Result<PathBuf, String> {
    download_dir_for(PLATFORM, env_value(Purpose::Downloads))
}

/// 内核的诊断日志。
///
/// 落在 `settings.json` **同目录**：壳自己的存储目录也是这一个（壳那侧见
/// `windows/shell-core/src/storage/mod.rs` 的 `dir()`），于是客户只要交出**一个目录**，
/// 支持就能拿到全部读数。
///
/// ⚠️ **两个进程各写自己的文件**（内核 `diag-kernel.log`、壳 `diag-shell.log`
///    ——2026-10-06 起**两侧的实现都在了**：壳那份在 `windows/shell-core/src/diagnostics.rs`；
///    但**壳那侧要等"详细日志"那一批的 Task 3 接上级别与开关之后才会真的产出文件**，
///    所以"只有内核这一份"这句话**今天仍然是对的**，只是理由从"没做"变成了"还没接线"）：
///    同一个文件被两个进程追加 + 轮转会打架，而诊断日志**出问题的方式必须是"少记几条"**，
///    不能是"把彼此的账写坏"。
///
/// 它**不新增环境值**：变量名与失败语义全部沿用 [`settings_file`] 那一条
/// （Windows 上取不到 `%APPDATA%` 仍是大声失败，其余的 `$HOME` 回退同理），
/// 本函数只把那个结果换一个文件名。
pub fn diagnostics_log() -> Result<PathBuf, String> {
    Ok(settings_file()?
        .parent()
        .map(|d| d.join("diag-kernel.log"))
        .unwrap_or_else(|| PathBuf::from("diag-kernel.log")))
}

// ---------------------------------------------------------------------------
// 纯函数（**不带 `#[cfg]`**，平台与环境值都是参数）
// ---------------------------------------------------------------------------

/// 纯函数：`settings.json` 的路径。
///
/// - Windows：`%APPDATA%\BenagenDownloader\settings.json`（规格 §8 的表）；
/// - 其余：`~/Library/Application Support/BenagenDownloader/settings.json`，
///   `$HOME` 取不到 ⇒ `settings.json`（相对路径，**冻结行为**）。
fn settings_file_for(platform: Platform, env: Option<OsString>) -> Result<PathBuf, String> {
    match platform {
        Platform::Windows => {
            let base = non_empty(env).ok_or_else(|| {
                missing(
                    "APPDATA",
                    "内核参数（settings.json）的位置",
                    "① 设好 %APPDATA%（通常形如 C:\\Users\\<用户名>\\AppData\\Roaming）；\
                     ② 或用 `--settings <PATH>` 显式指定设置文件。",
                )
            })?;
            Ok(PathBuf::from(base)
                .join("BenagenDownloader")
                .join("settings.json"))
        }
        // ⚠️ 下面这一支是**逐字抄来的**（`join` 链、空 `$HOME` 的回退值都照旧）：
        // 它是 W-3 的落点，动一个字都要先问"Go 侧是同一条吗"。
        Platform::Other => Ok(match non_empty(env) {
            Some(home) => Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join("BenagenDownloader")
                .join("settings.json"),
            None => PathBuf::from("settings.json"),
        }),
    }
}

/// 纯函数：aria2c 的释放目录。
///
/// - Windows：`%LOCALAPPDATA%\BenagenDownloader\cache`（规格 §8 的表）；
/// - 其余：`~/Library/Caches/BenagenDownloader`，`$HOME` 取不到 ⇒ `<临时目录>/BenagenDownloader`。
///
/// ⚠️ **`Platform::Other` 这一支对空 `$HOME` 不做"非空"判断**，与上面 `settings_file_for`
/// 的那一支**不一致**——这不是笔误，是**照抄既有实现**（`engine.go` / 移植过来的 Rust 版本
/// 用的都是 `var_os("HOME")` 的 `Some/None` 两分）。Go 的 `os.UserHomeDir()` 会把空 `$HOME`
/// 当取不到，所以两边在 `HOME=""` 这个病态输入上**本来就有分歧**。
/// 本任务（Windows 平台目录）不碰它：改了就是动 macOS 的行为，而 W-3 说那一个字都不许坏。
fn cache_dir_for(
    platform: Platform,
    env: Option<OsString>,
    temp_dir: &Path,
) -> Result<PathBuf, String> {
    match platform {
        Platform::Windows => {
            let base = non_empty(env).ok_or_else(|| {
                missing(
                    "LOCALAPPDATA",
                    "aria2c 的释放位置",
                    "① 设好 %LOCALAPPDATA%（通常形如 C:\\Users\\<用户名>\\AppData\\Local）；\
                     ② 若在服务/计划任务里启动，请补上用户环境块（本地系统账户下它默认为空）。",
                )
            })?;
            Ok(PathBuf::from(base)
                .join("BenagenDownloader")
                .join("cache"))
        }
        Platform::Other => Ok(match env {
            Some(home) => PathBuf::from(home).join("Library/Caches/BenagenDownloader"),
            None => temp_dir.join("BenagenDownloader"),
        }),
    }
}

/// 纯函数：默认下载目录。
///
/// - Windows：`%USERPROFILE%\Downloads\Benagen`（规格 §8 的表）；
/// - 其余：`~/Downloads/Benagen`，`$HOME` 取不到 ⇒ `benagen-downloads`（相对路径，冻结行为）。
fn download_dir_for(platform: Platform, env: Option<OsString>) -> Result<PathBuf, String> {
    match platform {
        Platform::Windows => {
            let base = non_empty(env).ok_or_else(|| {
                missing(
                    "USERPROFILE",
                    "默认下载目录",
                    "① 设好 %USERPROFILE%（通常形如 C:\\Users\\<用户名>）；\
                     ② 或用 `--download-dir <DIR>` 显式指定下载目录。",
                )
            })?;
            Ok(PathBuf::from(base).join("Downloads").join("Benagen"))
        }
        Platform::Other => Ok(match non_empty(env) {
            Some(home) => Path::new(&home).join("Downloads").join("Benagen"),
            None => PathBuf::from("benagen-downloads"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 上这三个标准变量的真身（形状取自文档，不是从本机读的）。
    const WIN_APPDATA: &str = r"C:\Users\x\AppData\Roaming";
    const WIN_LOCALAPPDATA: &str = r"C:\Users\x\AppData\Local";
    const WIN_USERPROFILE: &str = r"C:\Users\x";

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    // -----------------------------------------------------------------------
    // Windows：标准目录 + 取不到就大声失败（W-2）
    // -----------------------------------------------------------------------

    /// `settings.json` 落在 `%APPDATA%\BenagenDownloader\`（规格 §8 的表）。
    ///
    /// ⚠️ 期望值用 `PathBuf::join` 拼，**不能写成 `r"C:\…\BenagenDownloader\settings.json"` 字面量**：
    /// 在 macOS 上 `\` 是**普通字符**而不是分隔符，拼出来的 PathBuf 与字面量不是同一个东西
    /// （那是简报里那版测试在 arm64 上根本跑不过的第二个原因）。平台的**取值**由 `Platform`
    /// 参数注入，分隔符由宿主决定——这正是"纯函数能在本机真跑"的代价与收益。
    #[test]
    fn windows_settings_path_uses_appdata() {
        let got = settings_file_for(Platform::Windows, os(WIN_APPDATA)).expect("给了 %APPDATA% 就应当成功");
        let want = PathBuf::from(WIN_APPDATA)
            .join("BenagenDownloader")
            .join("settings.json");
        assert_eq!(got, want, "Windows 的 settings.json 必须落在 %APPDATA%\\BenagenDownloader\\ 下");
    }

    /// W-2：`%APPDATA%` 取不到 ⇒ **大声失败**，话术要点名缺的是哪个变量、并给出可执行的补救。
    #[test]
    fn windows_settings_path_fails_loudly_without_appdata() {
        let err = settings_file_for(Platform::Windows, None).unwrap_err();
        assert!(err.contains("APPDATA"), "错误话术必须点出缺的是哪个变量：{err}");
        // 存在但为空串同样算"取不到"：Windows 的环境变量可以**存在且为空**，
        // 而空串拼接出来的路径与相对路径无异。
        let err = settings_file_for(Platform::Windows, os("")).unwrap_err();
        assert!(err.contains("APPDATA"), "空 %APPDATA% 也必须大声失败：{err}");
        // 补救必须**可执行**：点名那条命令行开关，而不是只说"请设置环境变量"。
        assert!(err.contains("--settings"), "补救话术必须给出可执行的下一步：{err}");
    }

    /// 内嵌 aria2c 释放到 `%LOCALAPPDATA%\BenagenDownloader\cache\`（规格 §8 的表）。
    #[test]
    fn windows_cache_dir_uses_localappdata() {
        let got = cache_dir_for(Platform::Windows, os(WIN_LOCALAPPDATA), Path::new(r"C:\Temp"))
            .expect("给了 %LOCALAPPDATA% 就应当成功");
        let want = PathBuf::from(WIN_LOCALAPPDATA)
            .join("BenagenDownloader")
            .join("cache");
        assert_eq!(got, want, "aria2c 的释放位置必须是 %LOCALAPPDATA%\\BenagenDownloader\\cache");
    }

    #[test]
    fn windows_cache_dir_fails_loudly_without_localappdata() {
        let err = cache_dir_for(Platform::Windows, None, Path::new(r"C:\Temp")).unwrap_err();
        assert!(err.contains("LOCALAPPDATA"), "错误话术必须点出缺的是哪个变量：{err}");
    }

    /// 默认下载目录是 `%USERPROFILE%\Downloads\Benagen`（规格 §8 的表）。
    #[test]
    fn windows_download_dir_uses_userprofile() {
        let got = download_dir_for(Platform::Windows, os(WIN_USERPROFILE)).expect("给了 %USERPROFILE% 就应当成功");
        let want = PathBuf::from(WIN_USERPROFILE).join("Downloads").join("Benagen");
        assert_eq!(got, want, "默认下载目录必须是 %USERPROFILE%\\Downloads\\Benagen");
    }

    #[test]
    fn windows_download_dir_fails_loudly_without_userprofile() {
        let err = download_dir_for(Platform::Windows, None).unwrap_err();
        assert!(err.contains("USERPROFILE"), "错误话术必须点出缺的是哪个变量：{err}");
        assert!(err.contains("--download-dir"), "补救话术必须给出可执行的下一步：{err}");
    }

    // -----------------------------------------------------------------------
    // 其余平台（macOS / Linux）：**回退语义是冻结的 Go 行为**，逐条钉住（W-3）
    // -----------------------------------------------------------------------

    /// `$HOME` 在 ⇒ `~/Library/Application Support/BenagenDownloader/settings.json`。
    #[test]
    fn other_settings_path_uses_the_canonical_home() {
        let got = settings_file_for(Platform::Other, os("/Users/x")).expect("有 $HOME 就应当成功");
        let want = Path::new("/Users/x")
            .join("Library")
            .join("Application Support")
            .join("BenagenDownloader")
            .join("settings.json");
        assert_eq!(got, want);
    }

    /// `$HOME` 取不到 ⇒ **回退相对路径**（相当于 Go `DefaultPath` 的 `return "settings.json"`）。
    /// **这不是缺陷，是冻结行为**：与本模块 Windows 那一列的"大声失败"有意不同，别统一。
    #[test]
    fn other_settings_path_falls_back_to_a_relative_path() {
        assert_eq!(
            settings_file_for(Platform::Other, None).expect("回退不是错误"),
            PathBuf::from("settings.json")
        );
        assert_eq!(
            settings_file_for(Platform::Other, os("")).expect("空 $HOME 与未设同义"),
            PathBuf::from("settings.json")
        );
    }

    /// `$HOME` 取不到 ⇒ 回退到临时目录（对应 Go `engine.go` 的 `os.TempDir()`）。
    ///
    /// 临时目录由调用方**注入**（`temp_dir` 参数）：它同样是一个环境值
    /// （`std::env::temp_dir()` 在 Unix 上读 `TMPDIR`），注入之后这条回退才在本机可测。
    #[test]
    fn other_cache_dir_falls_back_to_the_temp_dir() {
        assert_eq!(
            cache_dir_for(Platform::Other, None, Path::new("/tmp/x")).expect("回退不是错误"),
            Path::new("/tmp/x").join("BenagenDownloader")
        );
    }

    /// `$HOME` 取不到 ⇒ 回退相对路径 `benagen-downloads`（阶段 A 起的既有行为）。
    #[test]
    fn other_download_dir_falls_back_to_a_relative_path() {
        assert_eq!(
            download_dir_for(Platform::Other, None).expect("回退不是错误"),
            PathBuf::from("benagen-downloads")
        );
    }

    /// 诊断日志与 `settings.json` **同目录**（规格 §3 A6）——客户只交**一个目录**就够。
    ///
    /// ⚠️ 断言写的是**关系**而不是字面量：本函数没有 `Platform` 参数（它就是拿
    /// [`settings_file`] 的结果换个文件名），写成 `…/BenagenDownloader/diag-kernel.log`
    /// 这种字面量会把宿主的分隔符与 `$HOME` 钉进测试，换个平台就红。
    ///
    /// 两个分支都断言，因为它们**都要紧**：成功那一支钉目录归属，失败那一支钉
    /// "取不到标准目录时沿用 `settings_file` 的失败语义"（Windows 上没有 `%APPDATA%`
    /// 必须仍是**大声失败**，而不是悄悄落到相对路径）。
    #[test]
    fn diagnostics_log_shares_the_settings_directory() {
        let name = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
        match settings_file() {
            Ok(settings) => {
                let log = diagnostics_log().expect("settings 定得下来，诊断日志也应当定得下来");
                assert_eq!(log.parent(), settings.parent(), "诊断日志必须与 settings.json 同目录");
                assert_eq!(name(&log).as_deref(), Some("diag-kernel.log"));
            }
            Err(e) => assert_eq!(
                diagnostics_log().unwrap_err(),
                e,
                "取不到标准目录时，诊断日志必须报**同一条**错（别自己发明一条）"
            ),
        }
    }

    // -----------------------------------------------------------------------
    // 环境变量名映射 + 本靶的 `PLATFORM`——**这两处此前无人钉**（2026-09-18 补）
    //
    // 上面 10 条用例**全部**显式注入 `Platform`，所以它们验的是"拿环境值拼路径"那一半；
    // 而"**读哪个变量**"与"**本靶属于哪一族**"这两处，此前没有任何断言经过。
    // -----------------------------------------------------------------------

    /// Windows 上三条路各读哪个标准变量（规格 §8 的表）。
    ///
    /// ⚠️ **这条测试的存在理由**：把变量名映射抽成纯函数（[`env_var_name`]）之前，
    /// 它内联在 `env_value` 的 `if cfg!(target_os = "windows")` 分支里——
    /// 把 `"LOCALAPPDATA"` 敲错一个字母，**在 macOS 上全套测试依旧全绿**，
    /// 要到客户机器上才以"取不到 %LOCALAPPDATA%"（或更糟：读到别的变量的值）暴露。
    /// `cfg!` 只保证那段代码被**类型检查**，字符串内容它管不着。
    ///
    /// 判别力：改掉任意一个变量名 ⇒ 必红。
    #[test]
    fn windows_env_var_names_are_the_standard_ones() {
        assert_eq!(env_var_name(Purpose::Settings, Platform::Windows), "APPDATA");
        assert_eq!(
            env_var_name(Purpose::Cache, Platform::Windows),
            "LOCALAPPDATA"
        );
        assert_eq!(
            env_var_name(Purpose::Downloads, Platform::Windows),
            "USERPROFILE"
        );
    }

    /// 其余平台三条路都从 `$HOME` 派生（对应 Go 的 `os.UserHomeDir()`）。
    #[test]
    fn other_env_var_name_is_home() {
        for p in [Purpose::Settings, Purpose::Cache, Purpose::Downloads] {
            assert_eq!(env_var_name(p, Platform::Other), "HOME");
        }
    }

    /// **本靶的 `PLATFORM` 必须是本靶**，且 `env_value` 真的去读了那个变量。
    ///
    /// ⚠️ **这条测试的存在理由**：`PLATFORM` 此前同样无人钉——把它改成恒返回
    /// `Platform::Other`（于是 Windows 上三条路全部落到"回退"支、绕过 W-2 的
    /// **大声失败**），上面 10 条用例**依旧全绿**，因为 `Platform` 都是显式传进去的。
    /// 那种漂移只会在客户机器上以"文件被写到一个用户永远找不到的地方"暴露。
    ///
    /// 后半段是**唯一一处真的读环境**的断言：它把 `env_var_name(p, PLATFORM)` 与
    /// `std::env::var_os` 的**实际取值**绑在一起——若 `env_value` 哪天不再经由
    /// `env_var_name`/`PLATFORM`（例如又被内联成一段字符串），这条会红。
    #[test]
    fn host_platform_and_the_variable_it_reads_are_pinned() {
        // `cfg!` 来自编译器、`PLATFORM` 来自本模块，是**两份独立的表述**：
        // 它们一致的唯一可能就是 `PLATFORM` 写对了——这正是要钉的东西。
        assert_eq!(
            PLATFORM == Platform::Windows,
            cfg!(target_os = "windows"),
            "PLATFORM 与本靶的 target_os 不符——\"选实现\"那一处漂了"
        );

        let expected = if cfg!(target_os = "windows") {
            ["APPDATA", "LOCALAPPDATA", "USERPROFILE"]
        } else {
            ["HOME", "HOME", "HOME"]
        };
        for (p, want) in [Purpose::Settings, Purpose::Cache, Purpose::Downloads]
            .into_iter()
            .zip(expected)
        {
            assert_eq!(env_var_name(p, PLATFORM), want, "本靶该读的变量名不对");

            // 真读一次。本机设了 `want` 就必须逐字相符；没设就得两边都是 `None`
            // ——后者同样有判别力：它排除"env_value 读的是另一个恰好有值的变量"。
            // 本仓库没有任何测试改环境变量，所以这里没有并发竞态（已 grep 确认）。
            match std::env::var_os(want) {
                Some(got) => assert_eq!(
                    env_value(p),
                    Some(got),
                    "env_value 取到的值与 %{want}% 不符——它读的不是这个变量"
                ),
                None => assert_eq!(
                    env_value(p),
                    None,
                    "环境里没有 %{want}%，env_value 却取到了值——它读的是别的变量"
                ),
            }
        }
    }
}
