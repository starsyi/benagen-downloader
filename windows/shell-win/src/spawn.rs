//! `spawn` —— **内核子进程该怎么被拉起**（规格 §7.3 的落点②，Windows 专有）。
//!
//! 这一层只做一件事：把"壳要拉起的那个进程"构造成一条 `std::process::Command`，
//! 并在 Windows 上给它套上 `CREATE_NO_WINDOW`。
//!
//! ## 为什么需要它（探路真机观察，别照 macOS 的印象写）
//!
//! 探路包在真 Windows 上跑的时候，人类伙伴报回：**有黑色控制台窗口，一直在窗口后显示**。
//! 那是**两件事**，缺一不可（`2026-09-18-windows-spike.md` §6.1）：
//!
//!   ① **壳自己**是 console 子系统程序 ⇒ 它自带一个控制台窗口。
//!      治它的是 `main.rs` 顶上的 `windows_subsystem = "windows"`（不是本模块）。
//!   ② **内核子进程**的控制台是**另一回事，①消不掉它**：
//!      GUI 子系统的父进程 spawn 一个 console 程序时，Windows 会给子进程**新开**一个控制台。
//!      治它的就是本模块的 `CREATE_NO_WINDOW`。
//!
//! ⇒ **只做①会留下"闪一下的黑窗"**。macOS 上 `Stdio::null()` 就够了的形态在这里**不成立**。
//!
//! ## ⚠️ 本模块的测试能证明什么、不能证明什么（规格 §11.1 第三行）
//!
//! 本机是 macOS。**能在本机断言的只有"标志位被传了"**；窗口有没有真的消失，
//! **只有真 Windows 能判**。所以 `spawn_flags_suppress_the_child_console` 这个名字
//! 说的是"标志位"而不是"窗口不出现"——那半句在本机是**测不到的**，写成那样就是
//! 把一条没验过的结论说成验过了。

use shell_core::client::{ClientError, CoreClient, ProcessChannel};
use std::path::Path;
use std::process::Command;

/// `CREATE_NO_WINDOW` —— Win32 进程创建标志（`CreateProcess` 的 `dwCreationFlags`）。
///
/// 值取自 Windows SDK（`winbase.h`）：`0x0800_0000`。
/// ⚠️ 这个十六进制**必须逐位对**：相邻的 `CREATE_NEW_CONSOLE` 是 `0x0000_0010`、
/// `DETACHED_PROCESS` 是 `0x0000_0008`——写错了不会报错，只会安静地开出一个控制台窗口，
/// 而那正是这条要治的东西。所以下面那条测试把它**当字面值**钉住。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 目标平台。**做成显式参数而不是 `cfg!(windows)`**，理由见 [`creation_flags_for`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    /// 非 Windows（macOS / Linux 的构建与测试）。
    Other,
}

impl HostOs {
    /// 本次编译的宿主平台。
    pub fn current() -> Self {
        if cfg!(windows) {
            HostOs::Windows
        } else {
            HostOs::Other
        }
    }
}

/// 给定的平台上，内核子进程必须套的 creation flags。
///
/// ⚠️ **为什么吃一个平台参数，而不是直接读 `cfg!(windows)`**（这是一处刻意的形状选择）：
///    `cfg!(windows)` 在 macOS 上恒为 `false`，那条分支在本机**根本不执行**——
///    测试就只能断言 `0`，一点信息都没有。做成纯函数之后，**本机能断言 Windows 那一支的
///    返回值**，而那正是规格 §11.3 要求的那条（"`CREATE_NO_WINDOW` 确实被传"）。
///    这是 §4.3 那条"能写出断言的代码一律搬出视图目标"的同一纪律，落在十六行代码上：
///    **决策**（什么值）与**应用**（怎么套到 `Command` 上）分开，前者跨平台可测。
fn creation_flags_for(os: HostOs) -> u32 {
    match os {
        HostOs::Windows => CREATE_NO_WINDOW,
        // 非 Windows 上 creation flags 这个概念不存在，也没有控制台窗口要消。
        // ⚠️ 这里返回 0 不是"降级"，是**这个平台本来就没有这件事**
        //    （与 W-2 说的"不许静默降级"不是一回事：没有可降的东西）。
        HostOs::Other => 0,
    }
}

/// 组一条用来拉起内核的命令（**生产入口**）。
///
/// ⚠️ **不要**在这里顺手加 `.stdin/stdout/stderr` 的管道设置：三根管道由
///    `ProcessChannel` 接管（见 [`spawn_core`]），两处都设会让"谁负责哪根"说不清。
pub fn core_command(exe: &Path) -> Command {
    core_command_with(exe, HostOs::current(), real_applier)
}

/// 组一条命令：**平台与应用点都可注入**。生产走 [`core_command`]（平台取宿主、应用点取
/// [`real_applier`]），测试用这个入口把两个量都钉住。
///
/// ⚠️ **`apply` 是这条接缝的全部要点**（复审抓的一处：重要 #1）：
///    "标志位被传了"这件事**只有在应用点是可注入的时候才成为一条真断言**。
///    第一版不是这样——应用点把自己的入参原样 `return`，而测试断言的就是那个返回值：
///    那是一条**恒真**的断言（在跑这条测试的宿主上是 `0 == 0`），
///    **把 `cmd.creation_flags(flags)` 那一行删掉它照样全绿**。
///    一个恒真的断言与没有断言，在客户机器上的后果完全一样。
///    现在改成：值**只能**经 `apply` 流出去，测试传一个记录用的闭包——
///    删掉下面那行 `apply(...)`，闭包不会被执行，测试**红**（判别力在报告里贴过）。
///
/// ⚠️ **本机（macOS）能断言到哪里、断不到哪里**（规格 §11.1 第三行）：
///    · 能：决策值（`creation_flags_for(HostOs::Windows)`）**被交给了应用点**；
///    · 能（靠 `build_windows.sh` 的交叉目标编译检查，不是靠跑）：`real_applier` 在
///      Windows 上真的调得动 `CommandExt::creation_flags`；
///    · **不能**："Win32 真的收到了这个标志位" —— `std` 在 1.89 上**没有**读回 creation
///      flags 的 API（`CommandExt` 只有 setter：`creation_flags` / `show_window` /
///      `force_quotes` / `raw_arg` / `startupinfo_*` / `async_pipes`；这条是查过 1.89 的
///      API 列表得出的，不是印象），所以那一跳没有 API 可查；
///    · **不能**：窗口有没有真的消失 —— 只有真 Windows 能判。
fn core_command_with(exe: &Path, os: HostOs, apply: impl FnOnce(&mut Command, u32)) -> Command {
    let mut cmd = Command::new(exe);
    apply(&mut cmd, creation_flags_for(os));
    cmd
}

/// 生产用的应用点：Windows 上把标志位真的交给 `CreateProcess`。
///
/// **平台分支只在这里，且只有这几行**。传成函数指针给 [`core_command_with`]，
/// 所以"改传别的值 / 忘了调它"这两个错法都会在那条测试里现形。
///
/// ⚠️ 本函数在宿主（macOS）上是个**空操作**——这是对的（那个平台没有 creation flags
/// 这回事），但也意味着"它真的调了 setter"这件事**在本机跑不出来**，只能靠
/// `build_windows.sh` 的 `cargo check --tests --target x86_64-pc-windows-gnu` 把
/// `#[cfg(windows)]` 那一支**编译**一遍（它当初就是这样抓出 `get_creation_flags` 不存在的）。
fn real_applier(cmd: &mut Command, flags: u32) {
    #[cfg(windows)]
    {
        // `std::os::windows::process::CommandExt`：这是**平台 API**，
        // 所以它在 `shell-win` 而不在 `shell-core`（后者必须保持跨平台，
        // `test.sh` 的依赖守卫与 `lib.rs` 的 `#![forbid(unsafe_code)]` 是同一件事的两面）。
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(flags);
    }
    #[cfg(not(windows))]
    {
        // 本平台没有 creation flags。**什么都不做**，但把参数"用掉"，
        // 免得零告警口径下报一个未使用参数的告警（本仓库的口径是"净增 0"，
        // 基线那 4 条在 `core/`，与本 crate 无关，见 test.sh 头部）。
        let _ = (cmd, flags);
    }
}

/// 起一个**真内核**子进程，并接上 `shell-core` 的三根管道。
///
/// ⚠️ **为什么要经 `ProcessChannel::with_command` 中转，而不是直接 `CoreClient::spawn`**：
///    `CoreClient::spawn` / `ProcessChannel::new` 内部自己 `Command::new(executable)`，
///    外面**没有接缝**能把创建标志塞进去（那是平台专属的进程创建参数）——那样落点②
///    就是一句够不着的注释。`with_command` 就是为这件事加的接缝：**命令由壳
///    （`shell-win`）组**，`shell-core` 只管"起进程 + 接管三条管道 + 收尾"。
///    平台知识留在壳里，纯逻辑留在 `shell-core` 里，两边都没有越界。
///
/// ⚠️ **调用方负责把它放到后台线程上**：`CoreClient::call` 没有每请求超时
///    （与 macOS 一致，刻意的），在 UI 线程上调用等于让窗口冻死——而"冻死"是**沉默的**。
///    见 `main.rs` 顶部的裁决 Z 不变量。
pub fn spawn_core(exe: &Path, args: &[String]) -> Result<CoreClient, ClientError> {
    Ok(CoreClient::new(Box::new(ProcessChannel::with_command(
        core_argv_cmd(exe, args),
        None,
    )?)))
}

/// 把内核的 argv 挂上去。**内核参数的拼接只在这一处**（任务 9 复审：次要 #4）。
///
/// ⚠️ **为什么要抽成独立函数**：上一版的测试把 `spawn_core` 的正文抄了一遍来断言 argv，
///    于是它钉的是**测试自己抄的那份**——把生产路径里的 `cmd.args(args);` 删掉，
///    测试照样全绿，而线上会静默丢掉 `--download-dir` / `--settings`
///    （内核于是去写用户家目录，规格 §8）。抽出来之后，测试与生产调的是**同一个函数**，
///    删掉那一行就变红。
fn core_argv_cmd(exe: &Path, args: &[String]) -> Command {
    let mut cmd = core_command(exe);
    cmd.args(args);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// ⚠️ **名字是"标志位被传了"，不是"窗口不出现"**：本机（macOS）能观察到的最强一条
    /// 就是"`CREATE_NO_WINDOW` 会走到应用点"；**窗口有没有真的消失只有真 Windows 能判**
    /// （规格 §11.1 第三行）。
    ///
    /// ⚠️ **判别力在 (b) 那一段**（复审抓的一处：重要 #1）。上一版的 (b) 断言的是
    /// `core_command` 的返回值，而那个返回值就是它自己的入参 ⇒ **恒真**，
    /// 删掉应用点那一行照样全绿。现在值**只能**经注入的 `apply` 流出去，
    /// 所以"决策值真的被交给了应用点"这句话是有判别力的（报告里贴了删一行的对照输出）。
    #[test]
    fn spawn_flags_suppress_the_child_console() {
        // ---- (a) 决策：Windows 上内核子进程必须带 CREATE_NO_WINDOW ----
        // 它同时钉住"哪一个平台"和"哪一个值"。
        assert_eq!(
            creation_flags_for(HostOs::Windows),
            CREATE_NO_WINDOW,
            "Windows 上内核子进程必须带 CREATE_NO_WINDOW —— 少了它，GUI 壳的每个 console \
             子进程都会新开一个控制台窗口（探路真机观察，spike §6.1）"
        );
        // 值本身当字面值钉住：写错一个十六进制位不会报错，只会安静地开出控制台窗口。
        assert_eq!(
            CREATE_NO_WINDOW, 0x0800_0000,
            "CREATE_NO_WINDOW 的字面值取自 Windows SDK（winbase.h）"
        );
        // 非 Windows 上不该有这件事（返回 0 是"平台没有这个概念"，不是降级）。
        assert_eq!(
            creation_flags_for(HostOs::Other),
            0,
            "非 Windows 上没有 Win32 的 creation flags"
        );

        // ---- (b) 应用：**决策值真的到了应用点** ----
        //
        // 这一段是这条测试的承重墙。`apply` 是个记录用的闭包：
        // 把 `core_command_with` 里那行 `apply(&mut cmd, creation_flags_for(os));` 删掉，
        // 闭包就不会被执行，`seen` 停在 `None` ⇒ **红**（报告 §修复里贴了那一次的输出）。
        let exe = PathBuf::from("benagen-core.exe");
        let mut seen: Option<u32> = None;
        let cmd = core_command_with(&exe, HostOs::Windows, |_cmd, flags| seen = Some(flags));
        assert_eq!(
            seen,
            Some(CREATE_NO_WINDOW),
            "决策出的标志位没有被交给应用点 —— 内核子进程会开出自己的控制台窗口"
        );
        // 程序名必须原样传下去（别在这里"顺手"拼路径或加扩展名）。
        assert_eq!(cmd.get_program(), exe.as_os_str());

        // 非 Windows 上应用点收到的是 0（同样要真的走一遍应用点，别只断言决策）。
        let mut seen_host: Option<u32> = None;
        core_command_with(&exe, HostOs::Other, |_cmd, flags| seen_host = Some(flags));
        assert_eq!(seen_host, Some(0), "非 Windows 上不该套 Win32 的 creation flags");

        // 生产入口照样构造得出来（在宿主上应用点是空操作，这一行只证明它不 panic）。
        assert_eq!(core_command(&exe).get_program(), exe.as_os_str());

        // ⚠️ **本机能断言的到此为止**，别把上面任何一句读成更强的话：
        //    "Win32 真的收到了这个标志位"在 std 层面**没有 API 可查**
        //    （`CommandExt` 只有 setter，没有 getter —— 见 `core_command_with` 的说明），
        //    而"窗口有没有真的消失"**只有真 Windows 能判**（规格 §11.1 第三行）。
        //    生产应用点（`real_applier`）的 Windows 分支在本机不执行，它靠
        //    `build_windows.sh` 的交叉目标编译检查保证"编得过"，不保证"跑得对"。
    }

    /// **argv 必须真的挂到命令上**，而且挂的是 `core_arguments` 给的那两条
    /// （复审：次要 #4）。
    ///
    /// ⚠️ 这条测试调的是**生产用的那个函数**（`core_argv_cmd`），不是把它的正文抄一遍——
    ///    上一版就是抄的，于是把 `spawn_core` 里那行 `cmd.args(args);` 删掉，
    ///    测试照样全绿，而线上会静默丢掉 `--download-dir` / `--settings`。
    #[test]
    fn spawn_core_arguments_are_appended_to_the_command() {
        let exe = PathBuf::from("benagen-core.exe");
        let args = shell_core::client::core_arguments(
            Some(Path::new("C:\\Users\\x\\Downloads\\Benagen")),
            Some(Path::new("C:\\Users\\x\\AppData\\Roaming\\BenagenDownloader\\settings.json")),
        );
        let cmd = core_argv_cmd(&exe, &args);
        // `Command` 只给得出程序名（argv[0]），拿不到后面那些参数——
        // 所以这里钉的是**组 argv 的那一步**：它必须来自 `core_arguments`，
        // 且这两条参数一个都不许少（少了内核会去写用户家目录，规格 §8）。
        let debug = format!("{cmd:?}");
        assert_eq!(cmd.get_program(), exe.as_os_str());
        assert!(
            debug.contains("--download-dir") && debug.contains("--settings"),
            "内核的 argv 必须来自 core_arguments（壳不自己拼参数名）：{debug}"
        );
        assert_eq!(args.len(), 4, "两条参数各带一个值：{args:?}");
    }
}
