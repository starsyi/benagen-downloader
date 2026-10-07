//! 导出诊断日志的**纯逻辑**（规格 §2.5 / §2.6）。
//!
//! 这个模块只做三件事，**一件 IO 都没有**：
//!
//!   1. [`subdirectory_name`] —— 目标文件夹里那个**带时间戳的子目录**叫什么
//!      （`诊断日志-YYYYMMDD-HHMMSS`）。本仓没有日期库（`diagnostics.rs` 那条注释写着
//!      "不为一行日志加一个日期库"），所以历法在这里**自己算**，而它**只有这一份**
//!      （[`utc_stamp`] 用的是同一个函数 —— 两处各算一遍就会漂）；
//!   2. [`files_to_copy`] —— **拷哪些**（六个名字与"现有的那些"求交，顺序固定）；
//!   3. [`header_text`] —— 那份 `说明.txt` 的**全部文字**。它收一份 [`Header`]
//!      （**已经量好的事实**）排出文本，**不碰文件系统** —— 于是"说明文件的字段对不对"
//!      是一条能在宿主上跑的断言，而不是"等真机上导一次看看"。
//!
//! ## 🔴 事实是**量出来的**，不是这个模块去量的
//!
//! 每份文件的字节数 / sha256 / 覆盖时间范围住在 [`FileFact`] 里：那是导出那一侧
//! （`shell-win/src/export.rs`）**逐份文件量完之后**填进来的。本模块**不**再扫一遍文件
//! —— 扫两遍就会有第二个真相源，而两个真相源在"日志正在被写"的时候**必然对不上**。
//!
//! ## 说明文件是**跨端契约**（规格 §2.6）
//!
//! macOS 那一侧要写一份字段**逐字相同**的（文案可以有细微差别，字段不能少）。
//! 下面 [`header_text`] 的测试逐行点名了那张表的每一行 —— 改字段之前先读它。

use std::time::SystemTime;

/// 导出目录的名字前缀。**它也是跨端契约的一部分**（两端生成的目录同名同形）。
pub const DIR_PREFIX: &str = "诊断日志-";

/// 清单文件的名字（导出文件夹里**唯一**那份不是日志的文件）。
pub const HEADER_FILE_NAME: &str = "说明.txt";

/// **内核**那份日志的名字。
///
/// ⚠️ 本 crate 看不见内核（见 `diagnostics.rs` 模块头第 2 条），所以这个名字是
///    **照抄**的：它的家是 `core/src/paths.rs` 的 `diagnostics_log()`。
///    壳那份的名字**不在这里抄** —— 它取自 [`crate::diagnostics::FILE_NAME`]
///    （见 [`all_log_names`] 的注释：抄第二遍的后果是改名之后**静默什么也不拷**）。
pub const KERNEL_LOG_NAME: &str = "diag-kernel.log";

/// 导出文件夹里**可能有**的全部日志名，顺序固定：**内核在前、代次由新到旧**。
///
/// ⚠️ 顺序是**我们定的**，不是调用方给的：同一台机器导两次，两份说明文件要能直接 diff
///    （否则"两次现场的区别"要靠人眼对行）。
///
/// ⚠️ 代数取自 [`crate::diagnostics::Level::Verbose::generations`]（**不是**写死的 2）：
///    内核与壳那两份日志器的代数由 `diagnostics.rs` 说了算，导出跟着它走 ——
///    写死一个 2 的话，哪天详细档改成留三代，导出的清单会**安静地少一份**。
pub fn all_log_names() -> Vec<String> {
    let mut names = Vec::new();
    for base in [KERNEL_LOG_NAME, crate::diagnostics::FILE_NAME] {
        names.push(base.to_string());
        for generation in 1..=crate::diagnostics::Level::Verbose.generations() {
            names.push(format!("{base}.{generation}"));
        }
    }
    names
}

/// **要拷哪些**：六个可能的名字与"真的存在的那些"求交，顺序见 [`all_log_names`]。
///
/// 详细日志刚打开时后几代本来就还没生成 ⇒ **不存在的不算失败**，不拷它，
/// 由说明文件如实写"尚未产生"（规格 §4）。
pub fn files_to_copy(existing: &[String]) -> Vec<String> {
    all_log_names()
        .into_iter()
        .filter(|name| existing.iter().any(|have| have == name))
        .collect()
}

/// **一份日志文件的事实** —— 全部由导出那一侧**量出来**（见模块头那段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFact {
    /// 文件名（`diag-kernel.log` / `diag-shell.log.1` …）。
    pub name: String,
    /// 字节数。**原样印出来**（不加千位分隔符）：看日志的人可能要拿它去比对。
    pub bytes: u64,
    /// sha256（64 位小写十六进制）：传输损坏与"客户改过内容"都靠它判。
    pub sha256: String,
    /// 这份日志**头一行**的 `ts=`（unix 秒）。读不到（空文件）⇒ `None`。
    pub first_ts: Option<u64>,
    /// 这份日志**最后一行**的 `ts=`（unix 秒）。读不到 ⇒ `None`。
    ///
    /// ⚠️ 日志正在被写 ⇒ 最后一行可能是**写到一半的**：那时这一格量不出来 ⇒
    ///    说明文件如实写"未知"（编一个范围比写"未知"坏得多）。
    pub last_ts: Option<u64>,
}

/// **说明文件要写的全部事实**（规格 §2.6 那张表）。
///
/// ⚠️ 除 [`Header::files`] 之外都是**导出那一刻的环境**：应用版本、内嵌内核的 sha256
///    （它**任何时候都拿得到** —— 壳在编译期就持有它）、内核自报的协议版本、当前日志级别、
///    系统与架构、导出时刻。**级别那一格是灵魂**：没有它，看日志的人无法判断
///    "这份日志为什么只有这么几行"（是没问题，还是级别没开？）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub app_version: String,
    pub core_sha256: String,
    /// 内核自报的协议版本。**连不上 ⇒ `None`**（如实写"未连上"，不是失败）。
    pub protocol_version: Option<i32>,
    pub log_level: String,
    pub os: String,
    pub arch: String,
    pub exported_at: String,
    pub files: Vec<FileFact>,
}

/// 那份 `说明.txt` 的全文（**纯函数**：同样的 [`Header`] ⇒ 同样的文本）。
///
/// 三段（规格 §2.6 / §2.7）：**风险**（在第一屏）、**有什么**（字段表）、**没有什么**
/// ——最后那段说的是"这份导出**不**做什么"（不打包、不加锁、不过滤），
/// 它们各自都是一种**显式的接受**，写在纸上比默认发生强。
pub fn header_text(header: &Header) -> String {
    // ⚠️ **这是一份给人看的纯文本**（他在记事本里打开它）⇒ 正文里**不用**
    //    Markdown 的强调记号（`**` 在那里只是噪音）。分节与缩进是它唯一的排版。
    let mut out = String::new();
    out.push_str("诊断日志导出说明\n");
    out.push_str("================\n");
    out.push_str("这份文件是这次导出的清单：它说的不是「日志里有什么内容」，\n");
    out.push_str("而是「这个文件夹里有哪些文件、每一份覆盖了哪一段时间」。\n\n");

    // ---- 第一屏：风险（规格 §2.7 的第 1 处明说）--------------------------------
    out.push_str("⚠️ 风险先说：这份日志会离开你的机器，里面可能有下载地址片段\n");
    out.push_str("（下载引擎的原始报错会原样带上它）。我们没有做自动过滤 —— 要可靠地\n");
    out.push_str("抹掉它，得先能识别它，而我们识别不了任意第三方文案；做一个不可靠的\n");
    out.push_str("过滤器比不做更糟（它会让人以为已经安全了）。\n");
    out.push_str("发送之前，你可以自己打开看一眼。\n\n");
    // ⚠️ **这一段在 2026-10-06 之前是一句绝对句，而它是假的**（终审实测）：交付码与
    //    客户目录名从**内核原文**（那条 404 的 URL、preflight 那句）进得来。
    //    ⇒ 现在分两句：**按构造抹掉的**（那两个串，壳自己知道）与**抹不掉的**
    //    （第三方文案里以别的写法出现的片段）。**两端逐字相同**（规格 §2.6 的契约）。
    // ⚠️⚠️ **"抹不掉的"是两条，不是一条**（修复轮 2 订正）：除了第三方写法那一条，
    //    还有**内核自己那条默认下载目录** —— **没配置下载目录**（默认状态）时壳推下去的
    //    是空串（`redact` 跳过它）、也不传 `--download-dir`，内核于是回落到**它自己**的
    //    默认路径（`core/src/paths.rs` → `C:\Users\<用户名>\Downloads\Benagen` /
    //    `~/Downloads/Benagen`）。那个串**壳不知道**（macOS 的 `kernelDefaultDisplayPath`
    //    明说了它只是显示用的），而它出现在内核 `preflight` 的失败文案里时**是逐字落的**
    //    （`core/src/main.rs`：`目标目录不可写（{dir}）`）⇒ 壳抹不掉它。
    //    **行为是对的**（猜一条客户机上的路径去抹，比不抹更糟）；错的是把话说满。
    out.push_str("本来就不记的：交付码本身、文件路径本身、rpc-secret、客户的目录名。\n");
    out.push_str("（其中交付码与下载目录在写进日志之前会被按字面抹成 [已隐去]；但没配置\n");
    out.push_str("下载目录时，内核会用它自己的默认路径——那一条壳不知道，抹不掉。抹不\n");
    out.push_str("掉的还有第三方文案里以别的写法出现的那一份——见上。）\n\n");

    // ---- 有什么（规格 §2.6 那张表，逐行点名）-----------------------------------
    out.push_str("这里面有什么\n");
    out.push_str("------------\n");
    out.push_str(&format!("app_version: {}\n", header.app_version));
    out.push_str(&format!("core_sha256: {}\n", header.core_sha256));
    match header.protocol_version {
        Some(version) => out.push_str(&format!("protocol_version: {version}\n")),
        // ⚠️ 连不上内核**不是失败**：这一格如实写"未连上"（编一个版本号比不写更坏）。
        None => out.push_str("protocol_version: 未连上\n"),
    }
    out.push_str(&format!("log_level: {}\n", header.log_level));
    out.push_str(&format!("os: {}\n", header.os));
    out.push_str(&format!("arch: {}\n", header.arch));
    out.push_str(&format!("exported_at: {}\n", header.exported_at));
    out.push('\n');

    // ---- 每份文件（名字 / 字节数 / sha256 / 覆盖范围）--------------------------
    out.push_str("日志文件（每份：名字 / 字节数 / sha256 / 覆盖时间范围）\n");
    out.push_str("----------------------------------------------------\n");
    let present: Vec<String> = header.files.iter().map(|f| f.name.clone()).collect();
    let absent: Vec<String> = all_log_names()
        .into_iter()
        .filter(|name| !present.iter().any(|have| have == name))
        .collect();
    if header.files.is_empty() {
        // ⚠️ 括号里那句**指路要指对**（修复轮 1 订正）：原来写的是"见下面「没有什么」"，
        //    而那一节讲的是"没有压缩包 / 没有加锁 / 没有过滤"——**回答不了**这里的
        //    "为什么一份都没有"。答案就在**紧下面那六行**（每一行都写着「尚未产生」），
        //    以及上面那两格（`log_level` 与 `exported_at`）。
        out.push_str("没有任何日志文件（下面每一行都写着「尚未产生」）。\n");
    }
    for file in &header.files {
        out.push_str(&format!(
            "{}  {} 字节  sha256={}  覆盖范围: {}\n",
            file.name,
            file.bytes,
            file.sha256,
            coverage(file)
        ));
    }
    for name in &absent {
        out.push_str(&format!("{name}  尚未产生（这一份日志还没有生成过）\n"));
    }
    out.push('\n');

    // ---- 没有什么 --------------------------------------------------------------
    out.push_str("没有什么（都是显式的选择，不是漏做）\n");
    out.push_str("--------------------------------------\n");
    out.push_str("· 没有压缩包：导出就是一个文件夹，本文件是它的清单；\n");
    out.push_str("· 没有加锁、也没有做快照：导出时日志可能仍在被写 ⇒\n");
    out.push_str("  某一份的最后一行可能是写到一半的（代价远大于收益，规格 §4）；\n");
    out.push_str("· 没有敏感信息自动过滤（理由见第一屏那段）。注意「把交付码与下载目录按字面\n");
    out.push_str("  抹成 [已隐去]」不是过滤：抹的是壳自己知道的那两个串本身，不是去识别\n");
    out.push_str("  任意文案里像不像它。\n\n");

    // ---- 怎么读 ----------------------------------------------------------------
    out.push_str("怎么读\n");
    out.push_str("------\n");
    out.push_str("每份日志一行一条记录，形状是：ts=<unix 秒> event=<事件名> 键=值 …\n");
    out.push_str("那个时间是 UTC 的 unix 秒（macOS `date -r <ts>`、Linux `date -d @<ts>`）。\n");
    out.push_str("「覆盖范围」那一格取的是这份日志的头尾两行：一眼就能看出它有没有\n");
    out.push_str("覆盖到出问题的那一段时间。没覆盖到的那一刻只有等下一次现场 ——\n");
    out.push_str("所以请在复现之前把详细日志打开，导出之后可以再关掉。\n");
    out
}

/// 一份文件的"覆盖时间范围"那一格：头尾两个 `ts=`，**读不到就写"未知"**。
fn coverage(file: &FileFact) -> String {
    match (file.first_ts, file.last_ts) {
        (Some(first), Some(last)) => format!("{first} → {last}"),
        _ => "未知".to_string(),
    }
}

/// 时间戳子目录的名字：`诊断日志-YYYYMMDD-HHMMSS`（**UTC**，见 [`utc_parts`]）。
///
/// 判别力：写成固定名字 ⇒ 客户导第二次时**把第一次的现场覆盖掉了**，
/// 而两次的时间点不同、恰好是最需要对比的时候。
pub fn subdirectory_name(at: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(unix_seconds(at));
    format!("{DIR_PREFIX}{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// 导出时刻那一格（`2025-10-06 09:23:11 UTC`）。
///
/// ⚠️ **与 [`subdirectory_name`] 用同一个历法**（同一个 [`utc_parts`]）：两处各算一遍的
///    话，同一份导出里"文件夹名"与"导出时刻"会漂开一个时区，而没有任何东西会变红。
///
/// ⚠️ 标的是 **UTC** 而不是"本机时区"：本 crate 拿不到本机时区偏移（没有日期库、
///    也没有平台 API），而把 UTC 写成"本机时间"是一句假话 —— 排查的人会按错误的
///    偏移去对。日志里的 `ts=` 本来就是 UTC 的 unix 秒，两边同源反而好对。
pub fn utc_stamp(at: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(unix_seconds(at));
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// **建不出那个子目录**那句话（规格 §4：一句人话 —— 哪个位置、为什么、下一步）。
///
/// 🔴 **三处失败各有各的说法**（本函数 / [`copy_failure_text`] / [`header_failure_text`]）：
///    它们**不是同一件事**，共用一句模板的后果是**对用户说错东西** —— 拷一份日志失败时
///    告诉他"建不了文件夹"，他会去查一个**建得出来**的位置，而真正出错的是那一份文件
///    （任务审查在修复轮 1 抓的）。
///
/// ⚠️ 它们住在这里（而不是在 `shell-win/src/export.rs` 里就地 `format!` 一句）：
///    那是**面向用户的话**，本仓的纪律是"能写出断言的都归 `shell-core`"，
///    而且三档各有断言（`each_failure_names_the_step_that_actually_failed`）。
pub fn folder_failure_text(folder: &str, cause: &str) -> String {
    format!(
        "诊断日志没有导出：建不了文件夹 {folder}（系统原话：{cause}）。\
         补救：换一个能写的文件夹再试一次（比如桌面），或检查那个位置是不是一个文件、磁盘是不是满了。"
    )
}

/// **某一份日志拷不进去**那句话（第二档：建目录成了、拷这一份时失败）。
///
/// 补救说的是这一档真正能做的事：某个文件被别的程序独占（**客户正用记事本开着日志**
/// 是真机上最常见的形状）时，关掉它再试一次就有用 —— 而"换个文件夹"没用。
pub fn copy_failure_text(file: &str, cause: &str) -> String {
    format!(
        "诊断日志没有导出：拷不进 {file}（系统原话：{cause}）。\
         补救：确认那一份文件还在、且没有被别的程序独占（比如你正用记事本打开着它），然后重试一次。"
    )
}

/// **说明文件写不进去**那句话（第三档：日志都拷好了、清单写不下去）。
pub fn header_failure_text(file: &str, cause: &str) -> String {
    format!(
        "诊断日志没有导出：说明文件写不进去（{file}；系统原话：{cause}）。\
         补救：检查目标磁盘是不是满了、那个文件夹是不是只读，然后重试一次。"
    )
}

/// 此刻的 unix 秒（`SystemTime` 早于 1970 时回 `0` —— 那种机器上本来就没有正确的"此刻"，
/// 而编一个负数只会让日期算到 1969 年去）。
fn unix_seconds(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// unix 秒 → **UTC** 的 (年, 月, 日, 时, 分, 秒)。
///
/// 本仓没有日期库（不为一次导出引一个），所以历法**在这里自己算** —— 用的是
/// Howard Hinnant 那套 `civil_from_days`（把"纪元日"换算成公历年月日），
/// 它**天然带闰年规则**（含"能被 100 整除不闰、能被 400 整除又闰"那两条）。
///
/// ⚠️ **这一段是本模块技术含量最高的一小段**：算错的后果是**目录名与日志内容对不上**，
///    而它**不会自己变红** —— 判据是 `the_calendar_matches_known_answers` 那几条
///    **算出来的**向量（含闰日、百年不闰、四百年又闰三档）。
fn utc_parts(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (unix / 86_400) as i64;
    let rest = unix % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = (rest / 3_600) as u32;
    let minute = ((rest % 3_600) / 60) as u32;
    let second = (rest % 60) as u32;
    (year, month, day, hour, minute, second)
}

/// 纪元日（1970-01-01 = 0）→ 公历 (年, 月, 日)。闰年规则在这 8 行里。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // 把纪元挪到 0000-03-01：这样"闰日"落在**这一年的最后**，
    // 于是"每 4 年一闰"是一条不打断的循环（3 月 1 日起算，2 月 29 日就是年末那一天）。
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    // 400 年里每天的位置 → 年内的天序号（去掉每 4 年那一闰、加回每 100 年那一不闰、
    // 再去掉每 400 年那一闰）。
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // 3 月 1 日起算的"月序号" → 真月（`(5 * doy + 2) / 153` 是那条固定的分段线性映射）。
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// [`subdirectory_name`] 之外**唯一**被 `shell-win` 用到的一次换算：
/// 从**握手的 `hello` 回执原文**里取内核自报的协议版本（规格 §2.6）。
///
/// ⚠️ **连不上 ⇒ `None`**（回执不在场、或者解不出来）—— 那一格由 [`header_text`]
///    如实写成"未连上"。**不许**回落到壳自己那个 `PROTOCOL_VERSION` 常量：
///    那一格回答的是"**内核**自报的是哪一套协议"，编一个壳侧的常量进去，
///    它就从"内核说的"变成了"我们以为的"，而这正是排查时**最不能有**的那种假话。
pub fn protocol_version_from_handshake(handshake_reply: Option<&str>) -> Option<i32> {
    handshake_reply
        .and_then(|raw| serde_json::from_str::<crate::protocol::HelloResult>(raw).ok())
        .map(|hello| hello.protocol as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 一份**量好的事实**（除名字外都是量出来的，不是编的）。
    fn a_file_fact() -> FileFact {
        FileFact {
            name: "diag-kernel.log".into(),
            bytes: 1_234_567,
            sha256: "bb".repeat(32),
            first_ts: Some(1_759_742_591),
            last_ts: Some(1_759_746_000),
        }
    }

    /// 一份**填满的**表头（除调用方要改的那一格）。
    fn a_header() -> Header {
        Header {
            app_version: "0.2.0".into(),
            core_sha256: "aa".repeat(32),
            protocol_version: Some(1),
            log_level: "verbose".into(),
            os: "Windows 10 22H2".into(),
            arch: "x86_64".into(),
            // ⚠️ 这一格是**生产那一侧**（`commands::diagnostics_export`）用
            //    [`utc_stamp`] 现算的，形状是 `<日期> <时刻> UTC`。夹具跟着它走 ——
            //    写成 `+08:00` 那种形状的话，本用例断的是一个**再也产不出来**的输入
            //    （任务审查在修复轮 1 抓的）。
            exported_at: "2026-10-06 14:03:11 UTC".into(),
            files: vec![a_file_fact()],
        }
    }

    /// 🔴 **说明文件的每一行都在**（规格 §2.6 那张表；审定者要逐行点名，不是数个数）。
    ///
    /// 判别力：删掉任何一行 ⇒ 红。尤其 **`level` 那一行**：没有它，看日志的人
    /// 无法判断"这份日志为什么只有这么几行"（是没问题，还是级别没开？）。
    #[test]
    fn the_header_names_every_field_the_spec_asks_for() {
        let text = header_text(&a_header());
        for needle in [
            "0.2.0",                      // 应用版本
            &"aa".repeat(32),             // 内嵌内核的 sha256
            "protocol_version: 1",        // 协议版本
            "log_level: verbose",         // 本次是哪个档 ← 最关键的一格
            "Windows 10 22H2",            // 系统
            "x86_64",                     // 架构
            "2026-10-06 14:03:11 UTC", // 导出时刻（⚠️ 形状必须是 `utc_stamp` 真的产得出的那一种 —— 见下面那条用例）
            "diag-kernel.log",            // 每份文件：名字
            "1234567",                    // …字节数
            &"bb".repeat(32),             // …sha256
            "1759742591",                 // …与它覆盖的头尾两个时刻
            "1759746000",
        ] {
            assert!(text.contains(needle), "说明文件里少了 {needle:?}：\n{text}");
        }
        // 🔴 隐私那段必须在**第一屏**（前 20 行之内），而且钉的是**那条风险本身**。
        //
        // ⚠️ **订正（修复轮 1，实测）**：原来写的是 `contains("交付码") || contains("下载地址")`
        //    —— 而下面"本来就不记的：**交付码**本身…"那一句**自己**就满足它 ⇒
        //    **把整段 ⚠️ 风险（第 6–10 行）删掉，这一条照旧绿**。规格 §2.7 要的是
        //    "第一屏写明**这条风险**"：日志里**可能包含下载地址片段**。⇒ 改成钉那个短语，
        //    去掉 OR（删掉那段 ⇒ 本用例红）。
        let first_screen: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
        assert!(
            first_screen.contains("下载地址片段"),
            "风险那段必须在前 20 行里，而且要说的是**那条风险**（可能包含下载地址片段）：\n\
             {first_screen}"
        );
        // 🔴 **"本来就不记的"那一句必须与它的限定句一起出现**（2026-10-06 终审）。
        //
        // ⚠️ 这一条钉的是**文案的口径**，不是行为：原来那一句是**绝对句**
        //    （"本来就不记的：交付码本身、…"），而它是**假的** —— 交付码与客户目录名
        //    从内核原文（那条 404 的 URL、preflight 那句）进得来。真正抹掉它们的是
        //    `client.rs` 的 `CoreClient::set_redactions`（那条行为有判据：
        //    `the_delivery_code_and_download_directory_never_reach_the_written_line`）。
        //    这里钉的是"说明文件没有把话说满" —— 删掉限定句、把那一行改回绝对句 ⇒ 红。
        assert!(
            first_screen.contains("按字面抹成 [已隐去]"),
            "把绝对句改回来是不行的：交付码与目录名只被**按字面**抹掉，\
             抹不掉的那一份（第三方文案里以别的写法出现的）必须写在旁边：\n{first_screen}"
        );
        // 🔴 **"抹不掉的"是两条**（修复轮 2 订正）：第三方写法那一条，以及**没配置下载
        //    目录时内核自己那条默认路径** —— 后者的串壳不知道（它推下去的是空串），
        //    而它出现在内核 `preflight` 的失败文案里时是**逐字落的**。
        //
        // ⚠️ 判别力：把这一句删掉（或者把上面那句改回"下载目录一定被抹掉"）⇒ 红。
        //    它钉的是**这一句为什么必须有限定**：没有它，读说明文件的人会以为
        //    "既然写了会抹成 [已隐去]，那日志里就不会有我的下载目录"——而默认状态下
        //    恰恰**有**（`C:\Users\<用户名>\Downloads\Benagen` / `~/Downloads/Benagen`）。
        //    ⚠️ 它**不是**"抹到了"的判据：真去抹的那一步在
        //    `client.rs` 的 `the_delivery_code_and_download_directory_never_reach_the_written_line`，
        //    而这一条只管文案不许把话说满（与上面那条同一个性质）。
        assert!(
            first_screen.contains("内核会用它自己的默认路径"),
            "没配置下载目录时内核那条默认路径抹不掉 —— 这一句必须写在旁边，\
             否则说明文件在默认配置下就是一句假话：\n{first_screen}"
        );
    }

    /// 🔴 **导出时刻那一格：逐字透传 + 形状必须是生产那一侧真的产得出的那一种**。
    ///
    /// 判别力（两条各挡一种写法）：
    ///   · 把 `exported_at:` 那一行整行删掉 ⇒ 第一条红；
    ///   · 把夹具改回 `+08:00` 那种形状（**今天再也产不出来**）⇒ 第二条红 ——
    ///     而那是"字段表那条用例在断一个不存在的输入"这件事唯一的判据：
    ///     **逐字透传**意味着格式漂了**不会有任何别的东西红**。
    ///     ⚠️ 它也是"必须写明是 UTC"这一条的守卫：`… UTC` 与 `… +08:00` 长度不同。
    #[test]
    fn the_header_prints_the_moment_verbatim_with_its_clock() {
        let text = header_text(&a_header());
        assert!(text.contains("exported_at: 2026-10-06 14:03:11 UTC"), "{text}");

        // 生产那一侧产得出的形状（`commands::diagnostics_export` 用的就是它）。
        let produced = utc_stamp(SystemTime::UNIX_EPOCH + Duration::from_secs(1_759_742_591));
        assert_eq!(produced, "2025-10-06 09:23:11 UTC");
        let fixture = a_header().exported_at;
        assert_eq!(
            fixture.len(),
            produced.len(),
            "夹具里那一格的形状与 utc_stamp 产出的不同（`+08:00` 那种今天再也产不出来）：{fixture}"
        );
        assert!(fixture.ends_with(" UTC"), "导出时刻必须写明是哪一个钟：{fixture}");
    }

    /// **一份文件都没量到**（日志还没产生过）⇒ 说明文件照生成，如实写"一份都没有"。
    ///
    /// 判别力（**突变实测**，修复轮 1）：删掉 [`header_text`] 里那个
    /// `if header.files.is_empty()` 分支 ⇒ 这一条**红**（退出码非 0）。
    ///
    /// ⚠️ **订正（修复轮 1，实测）**：原来这两条断言是
    ///    `contains("尚未产生") || contains("没有日志文件")` + `len > 100` ——
    ///    `files: Vec::new()` 时下面那六行**每一行都是「尚未产生」**，散文本身也远超
    ///    100 字 ⇒ **把那条分支整个删掉，它照旧全绿**（实测 `16 passed; 0 failed`）。
    ///    ⇒ 现在钉的是**只有那条分支才产得出的那一句**，外加一条**互相独立**的
    ///    "六代一个不漏地点名"。
    #[test]
    fn a_header_with_no_files_says_so_instead_of_being_empty() {
        let text = header_text(&Header {
            files: Vec::new(),
            ..a_header()
        });
        assert!(
            text.contains("没有任何日志文件"),
            "这一句**只有**那条分支产得出来（删掉分支 ⇒ 本用例红）：\n{text}"
        );
        // ⚠️ 这一条与上面那条**互相独立**：它钉的是"六代一个不漏地**各占一行**"，
        //    不靠上面那句散文（⚠️ 那句话说"下面每一行都写着「尚未产生」"，
        //    所以按 `contains` 数会把**它自己**也数进去 —— 这里按**行首的名字**数）。
        let named = all_log_names()
            .iter()
            .filter(|name| text.lines().any(|line| line.starts_with(&format!("{name}  "))))
            .count();
        assert_eq!(named, 6, "六个名字（内核三份 + 壳三份）要一个不漏地点名：\n{text}");
        assert!(text.len() > 100, "不许退化成一个空文件：{text}");
    }

    /// **覆盖范围缺失时如实写"未知"**（读不到头尾行时分不出是哪一种，
    /// 而编一个范围比写"未知"坏得多）。
    #[test]
    fn a_missing_coverage_range_is_stated_not_invented() {
        let text = header_text(&Header {
            files: vec![FileFact {
                first_ts: None,
                last_ts: None,
                ..a_file_fact()
            }],
            ..a_header()
        });
        assert!(text.contains("覆盖范围: 未知"), "{text}");
    }

    /// **连不上内核时也要能生成**（协议版本那一格如实写"未连上"，不是失败）。
    #[test]
    fn a_missing_protocol_version_is_stated_not_invented() {
        let text = header_text(&Header {
            protocol_version: None,
            ..a_header()
        });
        assert!(text.contains("protocol_version: 未连上"), "{text}");
    }

    /// 🔴 **不许拿壳自己的常量去填内核那一格**（那是"我们以为的"，不是"内核说的"）。
    ///
    /// 判别力：把 `protocol_version_from_handshake` 写成"解不出来就回
    /// `Some(PROTOCOL_VERSION)`"，第二条断言立刻红 —— 而真机上的表现是
    /// **连不上内核的那一份导出里写着"protocol_version: 1"**（一句没人说过的假话）。
    #[test]
    fn the_protocol_version_comes_from_the_kernel_not_from_us() {
        assert_eq!(
            protocol_version_from_handshake(Some(r#"{"protocol":1,"min_split_size_choices":[]}"#)),
            Some(1)
        );
        assert_eq!(protocol_version_from_handshake(None), None, "没握上手 ⇒ 未连上");
        assert_eq!(
            protocol_version_from_handshake(Some("这不是 JSON")),
            None,
            "解不出来同样是「未连上」，不是回落成我们自己的常量"
        );
    }

    /// **只拷存在的那些**（详细日志刚打开时后几代还没生成）。
    #[test]
    fn only_the_files_that_exist_are_listed() {
        let got = files_to_copy(&[
            "diag-kernel.log".to_string(),
            "diag-shell.log.2".to_string(),
        ]);
        assert_eq!(got, vec!["diag-kernel.log", "diag-shell.log.2"]);
        assert!(files_to_copy(&[]).is_empty());
    }

    /// 🔴 **顺序是我们定的，不是输入给的**（两次导出的说明文件要能直接 diff）。
    ///
    /// 判别力：把 `files_to_copy` 写成"按传入顺序过滤" ⇒ 这一条红 —— 而真机上那是
    /// "同一台机器导两次，说明文件的行序不一样"，而人要靠 diff 找两次现场的区别。
    #[test]
    fn the_order_is_ours_not_the_callers() {
        let scrambled: Vec<String> = vec![
            "diag-shell.log.2".to_string(),
            "diag-kernel.log.1".to_string(),
            "diag-shell.log".to_string(),
            "diag-kernel.log".to_string(),
        ];
        assert_eq!(
            files_to_copy(&scrambled),
            vec![
                "diag-kernel.log",
                "diag-kernel.log.1",
                "diag-shell.log",
                "diag-shell.log.2",
            ],
            "内核在前、代次由新到旧"
        );
    }

    /// ⚠️ **壳那份的名字**与 `diagnostics::FILE_NAME` 是**同一个**（不许在这里抄第二遍）。
    ///
    /// 判别力：把 `diag-shell.log` 在 `all_log_names` 里写死成字面量 ⇒ 改
    /// `diagnostics::FILE_NAME` 时导出会**静默地什么也不拷**（文件名对不上），
    /// 而说明文件里会写着"尚未产生" —— 一句看起来完全正常的假话。
    #[test]
    fn the_shell_log_name_comes_from_the_logger() {
        assert!(
            all_log_names().contains(&crate::diagnostics::FILE_NAME.to_string()),
            "壳那份日志的名字必须来自 `diagnostics::FILE_NAME`"
        );
        // 两代轮转都在（详细档留两代：`Level::Verbose.generations()`）。
        assert!(all_log_names().contains(&format!("{}.2", crate::diagnostics::FILE_NAME)));
    }

    /// ⚠️ **没产生的那几代要逐个点名**（"这份日志为什么只有这么几行"的一半答案）。
    #[test]
    fn a_generation_that_was_never_written_is_named() {
        let text = header_text(&a_header()); // 只有主文件那一份
        for missing in ["diag-kernel.log.2", "diag-shell.log"] {
            assert!(
                text.contains(missing),
                "没产生的那一份也要在说明文件里出现（写「尚未产生」）：少了 {missing}\n{text}"
            );
        }
    }

    /// 🔴 **时间戳子目录**（多次导出不互相覆盖，也不会把两次现场混在一起）。
    ///
    /// 判别力：把它写成固定名字 ⇒ 红；真机上的表现是客户导第二次时
    /// **把第一次的现场覆盖掉了**，而两次的时间点不同、恰好是最需要对比的时候。
    #[test]
    fn the_subdirectory_carries_a_timestamp_and_is_not_a_fixed_name() {
        let a = subdirectory_name(SystemTime::UNIX_EPOCH + Duration::from_secs(1_759_742_591));
        let b = subdirectory_name(SystemTime::UNIX_EPOCH + Duration::from_secs(1_759_742_592));
        assert_ne!(a, b, "不同时刻必须是不同的目录名");
        assert!(a.starts_with("诊断日志-"));
        assert_eq!(a.len(), "诊断日志-".len() + 15, "YYYYMMDD-HHMMSS：{a}");
    }

    /// 🔴 **历法的已知答案向量**（本 task 唯一能抓住"算错日子"的判据）。
    ///
    /// ⚠️ **这几条向量是算出来的，不是编的**（`python3 -c "import datetime; print(
    ///    datetime.datetime.fromtimestamp(TS, datetime.timezone.utc))"` 逐条核过）。
    ///    尤其是**闰日那一条**：一个"每 4 年加一天"写漏（或写成"能被 100 整除就不闰"
    ///    而漏掉"能被 400 整除还是闰"）的实现，会在 2024-02-29 之后**整年错一天**，
    ///    而目录名与日志内容对不上**不会让任何别的东西变红**。
    #[test]
    fn the_calendar_matches_known_answers() {
        let cases = [
            (1_767_225_600u64, "诊断日志-20260101-000000"), // 跨年、全零
            (1_709_251_199u64, "诊断日志-20240229-235959"), // **闰日**、且是当天的最后一秒
            (1_759_742_591u64, "诊断日志-20251006-092311"), // 普通的某一天
            (4_107_542_400u64, "诊断日志-21000301-000000"), // **百年不闰**：2100 不是闰年
            (951_782_400u64, "诊断日志-20000229-000000"),   // **四百年又闰**：2000 是闰年
        ];
        for (ts, want) in cases {
            assert_eq!(
                subdirectory_name(SystemTime::UNIX_EPOCH + Duration::from_secs(ts)),
                want,
                "unix {ts} 的日子算错了"
            );
        }
    }

    /// 导出时刻那一格用的是**同一份历法**（同一天里导两次，两个时间戳要一眼对得上）。
    #[test]
    fn the_export_moment_uses_the_same_calendar() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_759_742_591);
        assert_eq!(utc_stamp(at), "2025-10-06 09:23:11 UTC");
        // 与目录名说的是**同一个时刻**（两处各算一遍历法就会漂）。
        assert!(subdirectory_name(at).ends_with("20251006-092311"));
    }

    /// 日历那一格用的是 **UTC**，而且**明写了 UTC**（不假装是本机时间）。
    ///
    /// 判别力：把 `utc_stamp` 的 `UTC` 后缀去掉，第二条红 —— 而真机上那是看日志的人
    /// **按本机时区去对那一刻**，对不上几个小时，而没有任何东西会变红。
    #[test]
    fn the_export_moment_says_which_clock_it_is() {
        let text = utc_stamp(SystemTime::UNIX_EPOCH);
        assert_eq!(text, "1970-01-01 00:00:00 UTC");
        assert!(text.ends_with("UTC"), "要说清这是哪一个钟：{text}");
    }

    /// 建目录那一档失败的话**点名了文件夹与根因**（规格 §4：一句人话 —— 哪个位置、为什么）。
    ///
    /// 判别力：把它压成一句"导出失败"，下面三条断言一起红 —— 而真机上那是用户
    /// **不知道去看哪个位置**（他刚选的那个目标文件夹？还是日志目录？）【W-2】。
    #[test]
    fn an_unwritable_target_names_the_folder_and_the_cause() {
        let text = folder_failure_text("D:\\导出\\诊断日志-20260101-000000", "拒绝访问 (os error 5)");
        assert!(text.contains("诊断日志-20260101-000000"), "要点名是哪个文件夹：{text}");
        assert!(text.contains("拒绝访问"), "系统原话必须照登：{text}");
        assert!(text.contains("补救"), "要给出下一步能做的事：{text}");
        assert!(text.contains("没有导出"), "要说清这件事的结论：{text}");
    }

    /// 🔴 **每一档失败说的都是它真正失败的那一步**（不是三处共用一句）。
    ///
    /// 判别力（**突变实测**）：让 `copy_failure_text` / `header_failure_text` 改去调
    /// `folder_failure_text`（三档合成一句）⇒ 第 2 / 3 条断言红 —— 而真机上的表现是
    /// **用户拿着"建不了文件夹 D:\…\diag-shell.log"去查一个建得出来的位置**
    /// （他真正该看的是那一份文件）。任务审查在修复轮 1 抓的。
    #[test]
    fn each_failure_names_the_step_that_actually_failed() {
        let folder = "D:\\导出\\诊断日志-20260101-000000";
        let log = format!("{folder}\\diag-shell.log");
        let header = format!("{folder}\\说明.txt");
        let cause = "拒绝访问 (os error 5)";

        let mkdir_failed = folder_failure_text(folder, cause);
        let copy_failed = copy_failure_text(&log, cause);
        let header_failed = header_failure_text(&header, "磁盘空间不足");

        // ① 建目录那一档：点名那个**文件夹**（而且不该提到还没拷的文件）。
        assert!(mkdir_failed.contains("建不了文件夹"), "{mkdir_failed}");
        assert!(mkdir_failed.contains(folder), "{mkdir_failed}");
        assert!(!mkdir_failed.contains("diag-shell.log"), "这一档还没走到拷文件：{mkdir_failed}");
        // ② 拷一份日志那一档：点名**那个文件**，而且**不**冒充"文件夹建不了"。
        assert!(copy_failed.contains(&log), "要点名是哪一份日志：{copy_failed}");
        assert!(
            !copy_failed.contains("建不了文件夹"),
            "拷文件失败与建文件夹是两件事（说错了用户会去查错的位置）：{copy_failed}"
        );
        assert!(copy_failed.contains(cause), "系统原话必须照登：{copy_failed}");
        // ③ 写说明文件那一档：点名那个文件，同样不冒充建目录。
        assert!(header_failed.contains(&header), "{header_failed}");
        assert!(!header_failed.contains("建不了文件夹"), "{header_failed}");
        // 三句必须**真的不一样**（合成一句就会在其中一个场合说错话）。
        assert_ne!(mkdir_failed, copy_failed);
        assert_ne!(copy_failed, header_failed);
        for text in [&mkdir_failed, &copy_failed, &header_failed] {
            assert!(text.contains("补救"), "每一档都要给出下一步能做的事：{text}");
        }
    }

}
