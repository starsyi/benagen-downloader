//! 「导出诊断日志」里**真的去拷文件**的那一半（规格 §2.5）。
//!
//! 分工（与 `pickdir.rs` / `reveal.rs` 同一条）：
//!   * **判据与文字**都在 `shell_core`（时间戳目录名、要拷哪些、说明文件的全文、
//!     失败那句话 —— 见 `shell_core/src/export.rs`）；
//!   * 本模块只做**那两次系统调用**：`create_dir_all` 与 `fs::copy`，
//!     外加"**逐份文件量事实**"（字节数、sha256、头尾两行的 `ts=`）。
//!
//! ## ⚠️ 量的是**落进导出文件夹的那一份**，不是源文件
//!
//! 日志**正在被写**：从 `copy` 到"量字节数"之间源文件还会长。若两边各量一次，
//! 说明文件里的字节数与 sha256 会**互相矛盾**（他们量的是不同的两份内容）。
//! ⇒ 事实一律从**目标文件夹里那一份**量（那也正是客户发回给我们的那一份）。
//!
//! ⚠️ **这一节是设计意图，不是一条被钉住的事实**（2026-10-06 终审点名）：夹具里那份
//!    源日志是**静态的**，拷贝前后逐字节相同 ⇒ 把 [`export_from`] 里那句
//!    `measure(&to, name)` 换成 `measure(&logs.join(&name), name)`，**全套判据照旧全绿**。
//!    要让它可判，得能在 `copy` 与 `measure` 之间把源文件改长 —— 那需要给这段循环
//!    开一个只为测试存在的钩子，代价比它挡住的东西大。⇒ **如实记账**：这一条靠的是
//!    上面那句理由与下一个人读得懂它，不是靠判据。
//!
//! ## ⚠️ 不做加锁、不做快照（规格 §4，显式的选择）
//!
//! 代价如实记：拷到的那一份，**最后一行可能是写到一半的**。说明文件里写着这条
//! （`shell_core::export::header_text` 的"没有什么"那一段），而 [`last_ts`] 会
//! **跳过**那种半行（认**完整**的记录，见 `ts_of`）。
//!
//! ## ⚠️ 宿主那一支（macOS / Linux）
//!
//! 与 `pickdir.rs` 的宿主分支同一个理由（规格 §9.0）：本机要能**真跑一遍**这条链路
//! （否则"导出"这段代码在交付前一次都没被执行过）。交付形态只有 Windows exe。

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

use shell_core::export::{self, FileFact, Header};
use shell_core::sha256::sha256_hex;

/// 读文件**末尾**多少字节去找最后一条记录。
///
/// 一条记录的长度上界是 `MAX_VALUE_CHARS` 撑出来的（壳与内核那两份日志器都写着这条），
/// 8 KiB 足够装下好几条 —— 用不着为一个 4 MiB 的文件把它整份读进来（那是白做功）。
const TAIL_BYTES: u64 = 8 * 1024;

/// 把日志与说明文件拷进 `<目标>/诊断日志-YYYYMMDD-HHMMSS/`。**返回那个目录**。
///
/// ⚠️ 日志正在被写 ⇒ 拷贝可能拿到"写到一半的最后一行"。**如实记在说明文件里**
///    （规格 §4），**不做加锁、不做快照** —— 代价远大于收益。
///
/// ⚠️ **目标目录不可写 ⇒ 一句人话**（哪个目录、为什么），**绝不静默成功**：
///    界面上说"导出成功"、而客户发回来的文件夹是空的，比报错坏得多。
pub fn export_to(target: &str, header: &Header, at: SystemTime) -> Result<String, String> {
    // 日志目录与内核同源（`storage::dir()` 的判据见它的模块头）——
    // 壳**不给**内核传 `--settings`，两边是各自从 `%APPDATA%` 推出的同一个目录，
    // 这条同源由 `windows/scripts/check_shared_paths.sh` 守着。
    let log_dir = shell_core::storage::dir()?;
    export_from(&log_dir, target, header, at)
}

/// [`export_to`] 的**命令体**：日志目录与**时刻**都是**参数**（于是它能在临时目录上被真跑一遍）。
///
/// ⚠️ 拆出来的理由与 `commands::enqueue_with` 同一条：判据要能在宿主上跑，
///    而 `storage::dir()` 指向的是**真人正在用的那个目录**
///    （测试绝不许往那儿写东西）。
///
/// 🔴 **`at` 是参数，不是这里现读的时钟**（2026-10-06 终审）：这个时刻有**两个**落点
///    —— 文件夹名（`诊断日志-YYYYMMDD-HHMMSS`）与 `说明.txt` 里那格 `exported_at`
///    （`shell_core::export::header_text`）。两处各读一次时钟的话，跨过秒边界时
///    **同一个导出里那两个时刻会差一秒**，而它看起来完全正常（两个都是"合理的时间"）。
///    ⇒ 与 `core/src/diagnostics.rs` 那条"不为一行日志加一个日期库"同一条分工：
///    **读时钟的那一处只有一个**（`commands::diagnostics_export`），其余全是纯函数。
pub fn export_from(
    log_dir: &Path,
    target: &str,
    header: &Header,
    at: SystemTime,
) -> Result<String, String> {
    let folder = Path::new(target).join(export::subdirectory_name(at));
    let folder_text = folder.to_string_lossy().to_string();
    // 🔴 **不许 `let _ =`**：建不出来就往上报（回执要说"这次没成"）。
    std::fs::create_dir_all(&folder)
        .map_err(|cause| export::folder_failure_text(&folder_text, &cause.to_string()))?;

    // 只拷**存在的**那些：详细日志刚打开时后几代还没有（不存在不算失败）。
    let existing = file_names(log_dir);
    let mut facts = Vec::new();
    for name in export::files_to_copy(&existing) {
        let to = folder.join(&name);
        std::fs::copy(log_dir.join(&name), &to)
            .map_err(|cause| export::copy_failure_text(&to.to_string_lossy(), &cause.to_string()))?;
        // ⚠️ 事实从**落进导出文件夹的那一份**上量（模块头那段：源文件还在长）。
        facts.push(measure(&to, name));
    }

    // 说明文件最后写：它要说的是"这个文件夹里**已经**有什么"。
    let text = export::header_text(&Header {
        files: facts,
        ..header.clone()
    });
    let header_path = folder.join(export::HEADER_FILE_NAME);
    std::fs::write(&header_path, text)
        .map_err(|cause| export::header_failure_text(&header_path.to_string_lossy(), &cause.to_string()))?;
    Ok(folder_text)
}

/// 内嵌内核的 sha256（说明文件里"内核是哪一份"那一格）。
///
/// ⚠️ **交付形态（Windows）下它任何时候都拿得到**：`embed::CORE_SHA256` 是构建脚本
///    在把内核 exe 拷进 `OUT_DIR` 那一刻算出来的**编译期常量**（`embed.rs` 的头注）。
///    它**不需要**连上内核 —— 这正是规格 §2.6 要的那一格。
///
/// ⚠️ 宿主（macOS / Linux）上**没有内嵌内核**（`embed::CORE_BIN` 只在 Windows 靶上有，
///    见 `embed.rs` 的文件头）⇒ 那一格如实写"宿主构建"。**宿主构建不是交付形态**，
///    它不进任何一条分发路径（理由同 `pickdir.rs` 的宿主分支）。
pub fn embedded_core_sha256() -> String {
    #[cfg(target_os = "windows")]
    {
        crate::embed::CORE_SHA256.to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "（宿主构建：没有内嵌内核）".to_string()
    }
}

/// 说明文件里「`os:`」那一格：**系统与版本**（规格 §2.6）。
///
/// ⚠️ **为什么不是一个平台名**：规格给这一格的理由是"**"放一晚上就断"和"休眠回来就断"
///    在不同系统版本上是不同的问题**" —— 只写 `windows` 等于把那一格废掉。
///
/// ⚠️ 宿主（macOS / Linux）那一支只回平台名：那儿**没有**这一格要回答的问题
///    （宿主构建不是交付形态，理由同 `pickdir.rs` 的宿主分支）。
pub fn os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        windows_version().unwrap_or_else(|| "Windows（版本取不到）".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::consts::OS.to_string()
    }
}

/// **把版本号拼成那一格**（纯函数 —— Windows 那一支的判据在宿主上跑得了）。
///
/// ⚠️ 形状是 `Windows <major>.<minor>.<build>`，**就是系统 API 报的那三个数**：
///    不把它翻译成"Windows 10 / 11"那种市场名字 —— 那个映射今天在 Windows 11 上
///    **是错的**（`GetVersionExW` 对 Win11 照样回 `10.0.<build>`），而看日志的人
///    按 build number 判断版本比按名字准（"放一晚上就断"这类问题就落在 build 上）。
pub fn os_from_version(major: u32, minor: u32, build: u32) -> String {
    format!("Windows {major}.{minor}.{build}")
}

/// 真去问系统要版本号（**平台专属**，见 `os_version` 的文档）。
///
/// ⚠️ `GetVersionExW` **只有在进程有 manifest 时才是真的**（没有 manifest 的 exe 会被
///    兼容性垫片谎报成 6.2）—— 本 exe 的 manifest 声明了 `supportedOS` = Windows 10
///    （`shell-win/build.rs` 的 `manifest_xml`，W-7），所以这一格可信。
///    取不到（回 0）⇒ `None` ⇒ 那一格如实写"版本取不到"，**不编一个数**。
#[cfg(target_os = "windows")]
fn windows_version() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};

    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        dwMajorVersion: 0,
        dwMinorVersion: 0,
        dwBuildNumber: 0,
        dwPlatformId: 0,
        szCSDVersion: [0u16; 128],
    };
    // SAFETY：`info` 在本次调用期间一直存活，而 `dwOSVersionInfoSize` 按这条 API 的
    //         契约填的是**我们这个结构体自己的大小**（填错它只会回失败，不会越界写）。
    let ok = unsafe { GetVersionExW(&mut info) };
    if ok == 0 {
        return None;
    }
    Some(os_from_version(
        info.dwMajorVersion,
        info.dwMinorVersion,
        info.dwBuildNumber,
    ))
}

/// 日志目录里**现有的文件名**（读不动 ⇒ 空集：那与"一份都还没生成"是同一档处置）。
fn file_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// 量一份**已经在导出文件夹里**的日志：字节数、sha256、覆盖的头尾两个时刻。
///
/// ⚠️ 三格**一次量完**（都在同一份字节上）：分几次读的话，日志一长就会量出
///    "sha256 是这一份、字节数是那一份"那种自相矛盾（而它看不出来）。
fn measure(path: &Path, name: String) -> FileFact {
    let bytes = std::fs::read(path).unwrap_or_default();
    let (first_ts, last_ts) = first_and_last_ts(path);
    FileFact {
        name,
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        first_ts,
        last_ts,
    }
}

/// 一份日志**头尾两行**的 `ts=`（读不到 ⇒ `None`，说明文件如实写"未知"）。
fn first_and_last_ts(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let first = {
        // ⚠️ `BufReader` 借的是 `&mut file`，所以它必须在**这个块里**析构 ——
        //    不然下面那次 `seek` 借不到 `file`（E0499）。
        let mut reader = BufReader::new(&mut file);
        let mut line = String::new();
        match reader.read_line(&mut line) {
            // 空文件：`read_line` 回 0（那不是错误，是"这份日志还是空的"）。
            Ok(0) | Err(_) => None,
            Ok(_) => ts_of(&line),
        }
    };
    (first, last_ts(&mut file))
}

/// 最后一条**完整**记录的 `ts=`（从文件末尾往回找）。
///
/// ⚠️ **跳过写到一半的那一行**：日志正在被写时，末尾可能是半条记录 ——
///    认它会把一个**被截断的数字**当成时刻（例如 `1759746000` 被截成 `1759746`），
///    而那是"覆盖范围"那一格里一个**看起来很正常**的错值。
fn last_ts(file: &mut std::fs::File) -> Option<u64> {
    let length = file.metadata().ok()?.len();
    let window = length.min(TAIL_BYTES);
    file.seek(SeekFrom::Start(length - window)).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer);
    // 倒着找第一条**完整**的记录（`ts_of` 认的就是"完整"这件事）。
    text.lines().rev().find_map(ts_of)
}

/// 一行里的 `ts=`（`ts=<unix 秒> event=<名> …`，形状由那两份日志器的 `format_line` 钉着）。
///
/// ⚠️ **只认完整的记录**（`ts=` 之后还得有 ` event=`）：末尾那半行不给时刻，
///    由 [`last_ts`] 往回找上一条 —— 见那里的注释。
fn ts_of(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("ts=")?;
    if !rest.contains(" event=") {
        return None; // 写到一半的一行（或被截断的那一行）
    }
    rest.split(' ').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 一个一次性的临时目录（**只碰 `temp_dir()`**，不碰真机上那份偏好与日志）。
    fn a_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "benagen-export-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        dir
    }

    /// 判据用的那个**固定时刻**。
    ///
    /// ⚠️ **判据里不读时钟**：`at` 是 [`export_from`] 的参数（生产那一处是
    ///    `commands::diagnostics_export` 现读的唯一一次），这里给一个写死的值，
    ///    于是"文件夹名"与"`exported_at`"这两格在测试里是**同一个**时刻 ——
    ///    与生产同一条形状（`export_from` 的文档里那段理由）。
    fn a_test_instant() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_759_741_491)
    }

    /// 一份**只有环境事实**的表头（文件由被测代码填）。
    fn a_minimal_header() -> Header {
        Header {
            app_version: "0.2.0".into(),
            core_sha256: "aa".repeat(32),
            protocol_version: None,
            log_level: "verbose".into(),
            os: "windows".into(),
            arch: "x86_64".into(),
            exported_at: shell_core::export::utc_stamp(a_test_instant()),
            files: Vec::new(),
        }
    }

    /// 🔴 **目标目录不可写 ⇒ 必须回一句人话，不许静默成功**（规格 §4）。
    ///
    /// 判别力（**突变实测**，2026-10-06 + 修复轮 1）：
    ///   · 把**三处**失败出口全吞掉（`create_dir_all` / `copy` / `write` 一起换成
    ///     `let _ =`）⇒ 这一条红（`expect_err` 直接不成立）—— 而真机上的表现是
    ///     **界面上说"导出成功"、而客户发回来的文件夹是空的**（那比报错坏得多）；
    ///   · 只吞 `create_dir_all` 那一处 ⇒ **也红**（修复轮 1 实测，退出码 101）：
    ///     错误会从后面某一步冒出来，而那句话说的是**另一个步骤**（"说明文件写不进去"），
    ///     与"建不了文件夹"这个结论对不上。
    ///     ⚠️ 这一条**是修复轮 1 才长出来的牙**：在那之前断言只有 `!err.is_empty()`,
    ///     于是"只吞那一处"照样绿（简报 Step 4 里"把 `?` 换成 `let _ =` ⇒ 红"那句话
    ///     就是在这种情况下不成立的）。⇒ 断言必须收到**这一档专属的那句话**上。
    ///     ⚠️ 造法做不到把那一处单独隔出来（目标是一个普通文件时，建目录与往里写文件
    ///     必然一起失败）—— 但**消息**把它分开了，而这一条钉的正是"**每一档说的都得
    ///     是它真正失败的那一步**"（`each_failure_names_the_step_that_actually_failed`）。
    ///
    /// 造法（确定性）：把目标指到一个**已存在的普通文件**底下 ⇒ 建子目录必然失败。
    ///
    /// ⚠️ **用的是 `export_from`（日志目录是参数），不是 `export_to`**（修复轮 1 订正）：
    ///    `export_to` 的**第一步**是 `shell_core::storage::dir()?`（`HOME`/`APPDATA`
    ///    没设时就回 `Err`）—— 那样这一条会**因为第一步失败而绿**，而它**根本没碰到
    ///    这个夹具**（连临时目录都不会建）。兄弟用例用的都是 `export_from`。
    ///    顺带把断言收到**这一档专属的那句话**上（"建不了文件夹"）：
    ///    `!err.is_empty()` 连"回了一句别的话"都算过。
    #[test]
    fn an_unwritable_target_is_reported_instead_of_swallowed() {
        let logs = a_temp_dir("unwritable-logs");
        let f = std::env::temp_dir().join(format!("benagen-export-not-a-dir-{}", std::process::id()));
        std::fs::write(&f, b"x").expect("建文件失败");

        let err = export_from(&logs, &f.to_string_lossy(), &a_minimal_header(), a_test_instant()).expect_err("必须回错误");
        assert!(
            err.contains("建不了文件夹"),
            "要说清是**哪一步**、哪个位置（这一档是建目录）：{err}"
        );
        assert!(!err.is_empty(), "错误话术不许是空串");

        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_dir_all(&logs);
    }

    /// **日志文件不存在时导出照常成功**（详细日志刚打开、后几代还没生成）。
    ///
    /// 判别力：把"没有日志"判成失败 ⇒ 红 —— 而真机上那是**刚装好就导出**这一最常见
    /// 的情形里，用户看到一句"导出失败"，而他要的东西（说明文件）本来是给得出来的。
    #[test]
    fn missing_log_files_are_not_a_failure() {
        let logs = a_temp_dir("no-logs");
        let target = a_temp_dir("no-logs-target");

        let folder = export_from(&logs, &target.to_string_lossy(), &a_minimal_header(), a_test_instant())
            .expect("没有日志不是失败");
        let text = std::fs::read_to_string(Path::new(&folder).join(export::HEADER_FILE_NAME))
            .expect("说明文件必须生成");
        assert!(text.contains("尚未产生"), "要如实写「为什么一份都没有」：{text}");
        let _ = std::fs::remove_dir_all(&logs);
        let _ = std::fs::remove_dir_all(&target);
    }

    /// 🔴 **成功那一条：日志真的进去了，而说明文件写的是它们的**量出来的**事实**。
    ///
    /// 判别力（三条各挡一种写法）：
    ///   · 只建目录不拷文件 ⇒ 第一条红；
    ///   · 说明文件里的 sha256 / 字节数**另算一遍**（而不是量落盘那一份）⇒ 第二条红；
    ///   · 覆盖范围不读文件 ⇒ 第三条红（那两个时刻**只可能**来自文件内容）。
    #[test]
    fn the_export_folder_holds_the_logs_and_a_measured_header() {
        let logs = a_temp_dir("with-logs");
        let target = a_temp_dir("with-logs-target");
        // 一份**真的**日志（两行，形状与那两份日志器写的逐字相同）。
        let body = "ts=1759742591 event=kernel_call method=ping ok=true\n\
                    ts=1759746000 event=kernel_call method=getGlobalStat ok=true\n";
        std::fs::write(logs.join(shell_core::diagnostics::FILE_NAME), body).expect("写日志失败");
        let want_sha = shell_core::sha256::sha256_hex(body.as_bytes());

        let folder = export_from(&logs, &target.to_string_lossy(), &a_minimal_header(), a_test_instant())
            .expect("这一步必须成");
        let copied = Path::new(&folder).join(shell_core::diagnostics::FILE_NAME);
        assert_eq!(
            std::fs::read_to_string(&copied).expect("日志必须真的拷进去"),
            body,
            "拷进去的字节必须与源文件一样"
        );
        let text = std::fs::read_to_string(Path::new(&folder).join(export::HEADER_FILE_NAME))
            .expect("说明文件必须生成");
        assert!(text.contains(&want_sha), "sha256 必须是**落盘那一份**的：\n{text}");
        assert!(text.contains(&body.len().to_string()), "字节数要在：\n{text}");
        assert!(
            text.contains("1759742591") && text.contains("1759746000"),
            "覆盖范围必须来自文件内容（头尾两行的 ts=）：\n{text}"
        );
        // 导出到的是**目标文件夹里一个带时间戳的子目录**（不是把文件摊在目标里）。
        assert!(folder.starts_with(&target.to_string_lossy().to_string()));
        assert!(
            Path::new(&folder).file_name().unwrap().to_string_lossy().starts_with(export::DIR_PREFIX)
        );
        // 🔴 **同一个时刻喂出来的两格必须说同一件事**（2026-10-06 终审 ④b）：文件夹名与
        //    说明文件里那格 `exported_at` 都来自传进来的 `at` —— `a_minimal_header()`
        //    用的就是这个时刻，所以这一格与目录名是**同源**的。
        //    判别力：让 `export_from` 内部自己读一次时钟（`SystemTime::now()` 而不是
        //    用 `at`）⇒ 本断言红；而真机上那是"同一个导出里两个时刻差一秒"，
        //    看起来完全正常（两个都是"合理的时间"，谁也看不出它们对不上）。
        assert_eq!(
            Path::new(&folder).file_name().unwrap().to_string_lossy(),
            export::subdirectory_name(a_test_instant()),
            "文件夹名必须由传进来的 `at` 决定（不是这个函数自己读的时钟）"
        );
        assert!(
            text.contains(&shell_core::export::utc_stamp(a_test_instant())),
            "说明文件那一格必须与文件夹名同源（同一个 `at`）：\n{text}"
        );
        let _ = std::fs::remove_dir_all(&logs);
        let _ = std::fs::remove_dir_all(&target);
    }

    /// ⚠️ **写到一半的最后一行不算一条记录**（覆盖范围退回上一条**完整**的）。
    ///
    /// 判别力：把 `ts_of` 里那句"只认完整行"删掉 ⇒ 这一条红 —— 而真机上那是
    /// 一个**被截断的数字**当作时刻印进说明文件（`1759746000` 被截成 `1759746`），
    /// 它看起来完全正常，只是把"这份日志覆盖到哪一刻"说错了。
    #[test]
    fn a_torn_last_line_does_not_become_the_coverage_end() {
        let logs = a_temp_dir("torn");
        std::fs::write(
            logs.join(shell_core::diagnostics::FILE_NAME),
            "ts=1759742591 event=kernel_call ok=true\nts=1759746",
        )
        .expect("写日志失败");

        let (first, last) = first_and_last_ts(&logs.join(shell_core::diagnostics::FILE_NAME));
        assert_eq!(first, Some(1_759_742_591));
        assert_eq!(
            last,
            Some(1_759_742_591),
            "半行不许被当成一条记录 —— 要退回上一条**完整**的（那个被截断的数字看起来很正常）"
        );
        let _ = std::fs::remove_dir_all(&logs);
    }

    /// 🔴 **「系统与版本」那一格拼得出真东西、而且不编**。
    ///
    /// 判别力：把 `os_from_version` 改成只回 `"Windows"`（不带版本号）⇒ 第一条红 ——
    /// 而真机上的表现是说明文件里那一格**回答不了它存在的问题**
    /// （规格 §2.6：这一格的理由是"不同系统版本上是不同的问题"）。
    /// ⚠️ 拼装那半是纯函数，所以它**在宿主上跑得了**（Windows 那一支的调用点跑不到，
    ///    与本 crate 其余平台分支同一条账）。取不到版本号那一档**不编数**。
    #[test]
    fn the_os_field_carries_a_real_version_number() {
        assert_eq!(os_from_version(10, 0, 19_045), "Windows 10.0.19045");
        // ⚠️ 不把它翻译成"Windows 10 / 11"那种市场名字：`GetVersionExW` 对 Windows 11
        //    照样回 `10.0.<build>`，那个映射**今天是错的** —— build number 才是真的。
        assert!(os_from_version(10, 0, 22_621).contains("22621"), "build 号必须在");
        assert!(!os_version().is_empty(), "这一格不许是空串（空行在说明文件里等于没有这一格）");
    }

    /// 空日志文件（刚建出来、一行还没写）⇒ 覆盖范围两格都是 `None`（说明文件写"未知"）。
    #[test]
    fn an_empty_log_file_has_no_range() {
        let logs = a_temp_dir("empty");
        let path = logs.join(shell_core::diagnostics::FILE_NAME);
        std::fs::write(&path, b"").expect("写日志失败");
        assert_eq!(first_and_last_ts(&path), (None, None));
        let _ = std::fs::remove_dir_all(&logs);
    }
}
