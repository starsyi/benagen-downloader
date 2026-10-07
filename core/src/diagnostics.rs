//! 内核侧的**有界**诊断日志（规格 §3 A6）。
//!
//! ⚠️ **这个模块一共有三份实现**，必须逐条对齐（行格式、`MAX_VALUE_CHARS` 的截断、
//!    轮转命名、上限与代数）—— 客户会把几个文件一起发回来，**读法必须是同一种**：
//!
//! ⚠️ **下面这一段是 `text` 围栏，不是代码块**（2026-10-07）：它原先靠 4 空格缩进分段，
//!    而本模块当时挂在**二进制** crate 上（`mod diagnostics;` 写在 `main.rs` 里），
//!    rustdoc **不对二进制跑 doctest**，所以那段中文一直没被编译过。搬进 `lib.rs`
//!    （为了让 `delivery` / `engine::daemon` / `settings` 能用 `crate::diagnostics`）之后
//!    它第一次被当成 Rust 去编译，报 29 条错。**别把围栏去掉、也别改回纯缩进。**
//!
//! ```text
//!      ① **本文件** —— 内核，写 `diag-kernel.log`；
//!      ② `windows/shell-core/src/diagnostics.rs` —— Windows 壳，写 `diag-shell.log`；
//!      ③ `macos/Sources/BenagenCoreKit/DiagnosticsLog.swift` —— macOS 壳，写**同一个**
//!         `diag-shell.log`（那一侧 2026-10-06 之前**一行日志都没有**，由任务 5 补上）。
//!
//!    三份之间**唯一的自动判据**在 ② 那里（`the_two_loggers_agree_on_the_shared_constants`）：
//!    它 `include_str!` 本文件的源码，钉住 `MAX_BYTES` / `MAX_VALUE_CHARS` 两边一样。
//!    🔴 **它够不到 ③**：那个守卫是跨 crate 的**文本**比对，而 ③ 是另一门语言、另一个构建
//!    系统（`macos/Package.swift`），`include_str!` 到不了它 ⇒ ③ 与另外两份漂开时
//!    **不会有任何东西变红**（它那一侧只有同形的用例，而那只钉它自己的常量）。
//!    ⇒ **改这里的任何一条共享常量，都必须三处一起看**：② 那条断言会红（只有它知道），
//!      ③ **不会红**。
//!
//!    ⚠️ 本条原先把 ③ 漏了（2026-10-06 由任务 5 的任务审查实测出来：两份 Rust 的模块头
//!    各自只点了对方）—— 最可能来改共享常量的人（读着这个文件、手里有唯一一条自动守卫的人）
//!    恰恰没被指到第三份。补上之后"三份互相点名"这句才是真的。
//! ```
//!
//! # 为什么必须有它
//!
//! 客户报的「下载引擎已断开」**拿不到现场证据**（规格 §2：取不到就是取不到）。
//! 没有它，下一次发生时我们仍然只能猜「aria2 为什么不回话」。
//!
//! # 两条硬约束
//!
//! 1. **有界**：**两档各自有界**，界线由 [`Level`] 给。
//!    - `normal`（**缺省**，与加这一档之前**一字不差**）：单文件上限 [`MAX_BYTES`]（1 MiB），
//!      超了轮转成 `.1`（**只留一代**）；
//!    - `verbose`（"每一次往返"那一档）：单文件 **4 MiB**、留**两代**（`.1` / `.2`）
//!      ⇒ 总上界 12 MiB。
//!    绝不无限增长——这是本仓对"落盘的东西"的一贯口径。
//!    ⚠️ 精确地说，轮转在**每次写入之前**检查，所以单文件瞬时上界是
//!    `max_bytes + 一行`（写第 N+1 行之前才看得见第 N 行把文件顶过了线）、
//!    总占用上界是 `(generations + 1) × (max_bytes + 一行)`——**normal 档展开就是今天
//!    那句 `2 × MAX_BYTES + 两行`**（`generations = 1`），不是字面上的 1 MiB / 2 MiB。
//!    而「一行」自己也有上界：[`MAX_VALUE_CHARS`] 给每个字段值封顶 200 字符。
//!    **唯一**能顶破这个上界的条件是 `rename` 与兜底截断**两条都失败**（见 [`rotate`]）：
//!    那时主文件会接着长——本模块不许为此把产品弄坏，所以它仍然无声。
//! 2. **永不许把产品弄坏**：本模块**没有一个函数返回 `Result`、没有一处我们自己的
//!    `unwrap`/`panic`**。写不进去（目录不可写、磁盘满、路径被占）时**无声跳过**，
//!    只在**第一次**失败时往 stderr 说一句——内核的 stderr 由壳排空，**不会**污染
//!    stdout 那条协议专用通道。
//!
//!    ⚠️ 「没有一处我们自己的」这个限定词是承重的：`eprintln!` 本身在 stderr 不可写时
//!    （壳已死 ⇒ EPIPE）**会 panic**，那是标准库的行为，不是我们写出来的 `unwrap`。
//!    写成绝对句会被这个反例顶到——「注释不许说谎」这条约束不允许那种写法。
//!
//! # 不记什么（"记什么"的另一半，**这一节要说两句，读的时候别只读第一句**）
//!
//! **不记**交付码全文、文件路径全文、`rpc-secret`、客户目录名。只记端口、耗时、阶段、结果。
//! 诊断日志会被客户回传，它不该成为一份数据清单。
//!
//! ⚠️ **但第一句是"我们主动记什么"，不是"这个文件里保证没有它们"**（2026-10-06 终审点名）：
//!   · 本模块**自己**拼的那条拉交付页的路已经按构造避开了（`delivery.rs` 的
//!     `outcome_category` 只落 `ok` / `status_<码>` / `transport_<Kind>`，**不落 url**，
//!     有判据 `the_delivery_fetch_outcome_vocabulary_is_a_contract`）；
//!   · 而 **`aria2 原文` 是第三方的**（`engine/rpc.rs` 的 `aria2_call` 那一行）：它**原样**
//!     透传，里面**可能**带上交付码 —— 那正是规格 §2.7 的**显式接受**（要可靠地识别任意
//!     第三方文案里的交付码，得先能识别它，而我们识别不了）。
//!     ⇒ 别把这一节读成"这份日志干净了"。
//!   · ⚠️ 顺带记一笔：**壳那一侧不共用这条口径** —— `windows/shell-core/src/client.rs` 与
//!     macOS 的 `CoreClient` 会在**落盘之前**按字面抹掉交付码与下载目录
//!     （那两个串是**壳自己拼的**，它知道）。内核这一侧没有对应的动作：这里能记到的
//!     那一条来自第三方，抹不干净。
//!   · ⚠️ **但壳那一侧也只是"按字面"**（修复轮 2 收窄）：**没配置下载目录**（默认状态）时
//!     壳不传 `--download-dir`、推下去的是空串，内核回落到**它自己**的默认路径
//!     （`core/src/paths.rs` 的 `~/Downloads/Benagen` / `C:\Users\<用户名>\Downloads\Benagen`）
//!     ⇒ 上面那句 `preflight` 失败文案里的这条路径**壳抹不掉**。别把"壳会抹掉下载目录"
//!     读成无条件的。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// 单文件上限：1 MiB。轮转只留一代（`.1`）⇒ 总占用上界 `2 × MAX_BYTES` 加两行
/// （见模块头第 1 条：轮转在写入**之前**检查，每行另有 [`MAX_VALUE_CHARS`] 封顶）。
pub const MAX_BYTES: u64 = 1_048_576;

/// 单个字段值的字符上限：200。
///
/// 为什么需要它：`v` 里可能是**第三方**（aria2 / ureq）的原始错误文案，长度不受我们
/// 控制。不截断的话"一行"可以是任意长——那「有界」就只约束了**行数**、没约束**字节数**。
pub const MAX_VALUE_CHARS: usize = 200;

/// 第一次写失败只吵一句（否则每个事件都会往 stderr 刷一行）。
static COMPLAINED: AtomicBool = AtomicBool::new(false);

/// 诊断日志的档位。**缺省是 [`Level::Normal`]**，而 normal **与加这一档之前的行为一字不差**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 只记**事件**：抖动、断开、重试的成败。1 MiB / 留一代。
    Normal,
    /// **每一次往返**都记。4 MiB / 留两代 ⇒ 总上界 12 MiB。
    Verbose,
}

impl Level {
    /// 单文件上限。
    pub fn max_bytes(self) -> u64 {
        match self {
            Level::Normal => MAX_BYTES,
            Level::Verbose => 4 * 1_048_576,
        }
    }

    /// 轮转保留几代（`.1` … `.N`）。
    pub fn generations(self) -> u8 {
        match self {
            Level::Normal => 1,
            Level::Verbose => 2,
        }
    }
}

/// 解析 `--log-level` 的取值。
///
/// ⚠️ **不认识的取值必须报错**（规格 §2.1）：静默退回 normal 的后果是
/// "客户以为开了详细日志、导出发给我们的却是一份普通档的"——而**没有任何东西会变红**。
pub fn parse_level(raw: &str) -> Result<Level, String> {
    match raw {
        "normal" => Ok(Level::Normal),
        "verbose" => Ok(Level::Verbose),
        other => Err(format!(
            "不认识的日志级别 {other:?}：只认 `normal` 与 `verbose`。\
             （写错就悄悄退回 normal 会造出\"以为开了详细日志、导出的却是普通档\"这种查不出来的事）"
        )),
    }
}

/// 进程的级别。**只该在启动时由 [`init`] 写一次**。
///
/// ⚠️ 它做成进程级的静态，是因为 [`log`] 的调用点散在各处而**没有任何一处拿得到配置**。
/// 测试**不需要**碰它：`log()` 走的就是默认的 [`Level::Normal`]，而两档的界由
/// [`write_at`] 直接测（它收一个显式的级别）。
static LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn init(level: Level) {
    LEVEL.store(
        match level {
            Level::Normal => 0,
            Level::Verbose => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn level() -> Level {
    if LEVEL.load(Ordering::Relaxed) == 1 { Level::Verbose } else { Level::Normal }
}

/// 落一条诊断。**永不 panic、永不返回错误**。
///
/// **只记事件**（今天所有调用点用的都是它，语义一个字没变）：按**进程当前的级别**
/// 落——默认档下与加这一档之前逐字等价。
pub fn log(event: &str, fields: &[(&str, String)]) {
    log_at(event, fields, level());
}

/// **只在详细档记**——给"每一次往返"那一类用。
///
/// ⚠️ 它**不是** `log` 的参数版：调用点写 `log_verbose(...)` 表达的语义是
/// "这一行是**详细档才有的**"，而 `if level() == Verbose { log(...) }` 散在十几处
/// 会让"哪些行属于详细档"没有一个能 grep 的答案。
pub fn log_verbose(event: &str, fields: &[(&str, String)]) {
    if level() == Level::Verbose {
        log_at(event, fields, Level::Verbose);
    }
}

/// 落一条诊断的**唯一**出口，收一个**显式**级别（[`log`] 传进程级别、[`log_verbose`] 传
/// [`Level::Verbose`]）。
///
/// ⚠️ 两档的界（上限、留几代、轮转与兜底）**只有 [`write_at`] / [`rotate`] 一份实现** ——
///    本函数只负责"取路径、拼行、写"。
fn log_at(event: &str, fields: &[(&str, String)], level: Level) {
    let Ok(path) = crate::paths::diagnostics_log() else {
        return; // 连路径都定不下来：无声跳过（本模块不许有能力弄坏产品）
    };
    // ⚠️ 记录分隔符（`\n`）由**这里**补，不在 `format_line` 里。原因见测试
    //    `a_line_has_a_stable_shape`：那条按空格切开之后要求最后一格恰好是 `ok=true`，
    //    换行若留在 `format_line` 里就会变成 `ok=true\n`、那条断言必红。
    //    **测试是规格，实现向它看齐**（这条正是写计划时留下的自相矛盾）。
    write_at(&path, &format!("{}\n", format_line(event, fields)), level);
}

/// 字段行：`ts=<unix 秒> event=<名> k=v k=v …`（**不含**行尾换行，见 [`log_at`]）。
///
/// ⚠️ **`\n` 由 `log_at` 补**（不在本函数里）：换行若留在 `format_line` 里，测试
///    `a_line_has_a_stable_shape` 那条按空格切的断言必红（"测试是规格"）。
/// ⚠️ **这是本文件第二次出现"指针被重构弄漂"**（上一次是那句 "见 write_to" —— 那段逻辑
///    搬到 `rotate` 之后它没跟着走）：两次都是**同一个动作**——把某段逻辑从 A 挪到 B，
///    而**指向 A 的那句话不会红**（`cargo build` / `cargo test` 都看不见它）。
///    ⇒ 下次把代码从 A 挪到 B 时，**顺手 grep 一遍 A 的名字**。
///
/// ⚠️ `ts` 是**UTC 的 unix 秒**（本 crate 没有日期库，也不为一行日志加一个）。
/// 换算不在这里自己实现历法，只给命令：macOS `date -r <ts>`、Linux `date -d @<ts>`
/// （收尾阶段会把这一条写进 `windows/README.md`，**此刻那一节还没有**）。
fn format_line(event: &str, fields: &[(&str, String)]) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut s = format!("ts={ts} event={event}");
    for (k, v) in fields {
        // 值里的空格换成 `_`：这一行是按空格切字段读的，一个带空格的值会把字段切开。
        //
        // ⚠️ 同时**截断到 `MAX_VALUE_CHARS`**：`v` 里可能有第三方（aria2 / ureq）的
        // 原始错误文案，长度不受我们控制——不截断的话"一行"可以是任意长，
        // 「有界」就只约束了行数不约束字节数。
        let clean: String = v
            .chars()
            .take(MAX_VALUE_CHARS)
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .collect();
        s.push(' ');
        s.push_str(k);
        s.push('=');
        s.push_str(&clean);
    }
    s
}

/// 写一行并维持上限。**失败无声**（第一次往 stderr 说一句）。
///
/// 界线取 [`Level::max_bytes`]、轮转交 [`rotate`] —— 两档只在这里分叉。
fn write_at(path: &Path, line: &str, level: Level) {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() && std::fs::create_dir_all(dir).is_err() {
            complain_once(dir);
            return;
        }
    }
    // 轮转：超限就把当前文件挪成 `.1`（覆盖上一代），再从头写。
    //
    // ⚠️ **`rename` 失败必须走兜底**，不能只是 `let _ =`：轮转失败而照样 append ⇒
    // 主文件**无界增长**，而这条路径恰恰是唯一能破坏「有界」这条硬约束的条件。
    // 它在真机上不是纸面路径：Windows 上客户用记事本打开日志交给支持 ⇒ `MoveFileEx`
    // 因共享冲突失败（源文件需要 DELETE 访问，记事本不给）。
    // 兜底 = **就地截断**：只丢上一代，绝不丢「有界」。
    //
    // ⚠️ **顺序承重**：先截断、后吵。`complain_once` 里是 `eprintln!`，而它在
    // stderr 不可写时（壳已死 ⇒ EPIPE）**标准库自己会 panic**（见模块头第 2 条）。
    // 两件事同时发生（轮转挪不动 + stderr 断）时，先吵就会让 panic **跳过截断**
    // ——恰好在最需要「有界」的时候失效，而 panic 才是真正"把产品弄坏"。
    // （上面两条都落在 [`rotate`] 里，理由与顺序原样保留。）
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > level.max_bytes() {
        rotate(path, level);
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(_) => complain_once(path),
    }
}

/// 轮转：把最老的一代丢掉，其余各代依次后移，最后把主文件挪成 `.1`。
///
/// ⚠️ **`Normal`（一代）走的是与加这一档之前逐字相同的两步**：循环一次都不转、
///    只做 `path → .log.1`。既有那两条用例钉的就是这条路径。
///
/// ⚠️ **`rename` 失败必须走兜底截断**（今天的注释与理由原样保留，见 [`write_at`]）：
///    轮转失败而照样 append ⇒ 主文件无界增长，而那是唯一能破坏「有界」这条硬约束的条件。
fn rotate(path: &Path, level: Level) {
    for i in (1..level.generations()).rev() {
        let older = path.with_extension(format!("log.{}", i + 1));
        let newer = path.with_extension(format!("log.{i}"));
        let _ = std::fs::rename(&newer, &older);
    }
    let rotated = path.with_extension("log.1");
    if std::fs::rename(path, &rotated).is_err() {
        // ⚠️ **顺序承重：先截断、后吵**（理由见文件里 [`write_at`] 那段注释，一个字都不改）。
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path);
        complain_once(&rotated);
    }
}

fn complain_once(what: &Path) {
    if !COMPLAINED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "诊断日志写不进去（{}）——**不影响下载**，但出问题时就没有现场证据了。\
             请检查该目录是否可写。本句只说一次。",
            what.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本测试里每条记录的字节数（`{i:0>180}` 恰好 180 字节，`write_at` **不补**换行）。
    const LINE_BYTES: u64 = 180;

    /// **有界**：写到超过 1 MiB 必须轮转，且总占用有上限。
    ///
    /// ⚠️ 这条是规格 §3 A6 的硬约束，**不是**"顺手加的"：诊断日志是本仓唯一一处
    /// 会因为客户长期使用而持续增长的落盘物。
    ///
    /// ⚠️ 断言写的是**代码真正保证的**上界，不是"1 MiB"这个整数：轮转在写入**之前**
    /// 检查，单文件可以到 `MAX_BYTES + 一行`（见模块头）。写成 `<= MAX_BYTES` /
    /// `<= 2 × MAX_BYTES` 会在**本测试这个循环次数上直接假红**：主文件到这里正是
    /// `MAX_BYTES + 104` 字节，仍在模块头承诺的 `MAX_BYTES + 一行` 之内。
    ///
    /// 循环次数取 **11652** 是**算过的**，不是凑的：每条 180 字节 ⇒ 每 5826 次写
    /// 把主文件从 0 顶到 `> MAX_BYTES`（写满 5825 条时是 1,048,500 ≤ 上限，第 5826 条
    /// 越过线），于是第 5827 次写发生**第一次**轮转、第 11653 次发生第二次。
    /// 11652 落在第二次轮转**前一次** ⇒ 恰好把主文件顶到其最大值 `MAX_BYTES + 104`，
    /// 即本测试覆盖的是**最坏那一格**，而不是一个松垮的中间值。
    #[test]
    fn the_log_is_bounded_and_rotates() {
        let dir = std::env::temp_dir().join(format!("benagen-diag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("diag-kernel.log");

        // 11652 × 180 = 2,097,360 字节 ≈ 2.0 MiB ⇒ 恰好轮转一次（推导见 doc 注释）。
        // 级别显式写 `Normal`：这条钉的就是**缺省档**那条路径。
        for i in 0..11652 {
            write_at(&path, &format!("{i:0>180}"), Level::Normal);
        }

        let main_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let rotated_len = std::fs::metadata(dir.join("diag-kernel.log.1"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(
            main_len <= MAX_BYTES + LINE_BYTES,
            "主文件 {main_len} 字节，超过上限 {MAX_BYTES} + 一行 {LINE_BYTES}"
        );
        assert!(
            main_len + rotated_len <= MAX_BYTES * 2 + 2 * LINE_BYTES,
            "轮转后总占用 {} 字节，超过两倍上限加两行",
            main_len + rotated_len
        );
        assert!(rotated_len > 0, "从没轮转过 —— 上限没生效");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`rename` 失败必须退化为就地截断**，否则主文件无界增长——`rotate` 里
    /// 唯一能破坏「有界」这条硬约束的条件。
    ///
    /// ⚠️ 造法是**确定性的**：把 `.1` 那个名字占成**目录** ⇒ `rename(文件 → 目录)`
    /// 必然失败（Unix 上 EISDIR/ENOTDIR，Windows 上同样是错误）。它与真机上
    /// "客户正用记事本打开着日志"是同一形状：轮转挪不动。
    ///
    /// 判别力：把兜底那两句（就地截断 + `complain_once`，顺序见 `rotate`）删掉，
    /// 本测试必红——主文件会一路长到 12000 × 180 = 2,160,000 字节。
    #[test]
    fn a_failed_rotation_truncates_instead_of_growing_forever() {
        let dir = std::env::temp_dir().join(format!("benagen-diag-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("diag-kernel.log");
        std::fs::create_dir_all(dir.join("diag-kernel.log.1")).expect("建 .1 目录失败");

        for i in 0..12000 {
            write_at(&path, &format!("{i:0>180}"), Level::Normal);
        }

        let main_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert!(
            main_len <= MAX_BYTES + LINE_BYTES,
            "rename 失败之后主文件长到了 {main_len} 字节 —— 兜底截断没生效"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **写不进去也不许把产品弄坏**：路径是某个文件的子路径时，写入必须失败得无声。
    #[test]
    fn a_write_failure_never_panics() {
        // 拿一个**已存在的普通文件**当目录用 ⇒ create_dir_all/write 必然失败。
        let f = std::env::temp_dir().join(format!("benagen-diag-not-a-dir-{}", std::process::id()));
        std::fs::write(&f, b"x").expect("建文件失败");
        let bogus = f.join("sub").join("diag.log");
        write_at(&bogus, "这一条写不进去，但绝不许 panic", Level::Normal);
        let _ = std::fs::remove_file(&f);
    }

    /// 一行日志的形状：`ts=<unix> event=<名> k=v …`，且**值里的空格不会把字段切开**。
    ///
    /// ⚠️ 最后一个字段是**故意带空格的**：把 `format_line` 里那段
    /// `is_whitespace() → '_'` 整段删掉，本测试必红——那正是"这一行能被按空格切字段读"
    /// 的全部依靠。前两个字段（`read` / `true`）不含空格，光靠它们**守不住**这条性质。
    #[test]
    fn a_line_has_a_stable_shape() {
        let line = format_line(
            "rpc_retry",
            &[
                ("phase", "read".into()),
                ("ok", "true".into()),
                ("why", "action: 探活失败".into()),
            ],
        );
        let parts: Vec<&str> = line.split(' ').collect();
        assert!(parts[0].starts_with("ts="), "第一格必须是 ts=：{line}");
        assert_eq!(parts[1], "event=rpc_retry");
        assert_eq!(parts[2], "phase=read");
        assert_eq!(parts[3], "ok=true");
        assert_eq!(
            parts[4], "why=action:_探活失败",
            "值里的空格必须换成 `_`，否则一个带空格的值会把字段切开：{line}"
        );
        assert_eq!(parts.len(), 5, "带空格的值被切成了多格：{line}");
    }

    /// 🔴 **normal 档一个字都不许变**（规格 §2.2）：上限还是 1 MiB、还是只留一代。
    ///
    /// 判别力：把 `Level::Normal::max_bytes()` 写成 verbose 那个数，这一条立刻红 ——
    /// 而真机上的表现是"客户机器上那份普通日志突然能占 12 MiB"，
    /// **没有任何别的东西会红**（既有那两条轮转用例写的是 `MAX_BYTES` 这个常量，
    /// 而它们走的是默认档）。
    #[test]
    fn the_normal_level_keeps_todays_bounds() {
        assert_eq!(Level::Normal.max_bytes(), MAX_BYTES);
        assert_eq!(Level::Normal.generations(), 1);
        assert_eq!(Level::Verbose.max_bytes(), 4 * 1_048_576);
        assert_eq!(Level::Verbose.generations(), 2);
    }

    /// 🔴 **详细档仍然是有界的**（规格 §2.2 那张表的最后一行）。
    ///
    /// 它记的是"每一次往返"，所以它不是"慢一点"而是"快十几倍" —— 没有上界的话
    /// 客户开一晚上就是一个几 GB 的文件。断言写的是**算得出来的**上界：
    /// 单文件 `max_bytes + 一行`，总占用 `generations × max_bytes + 两行`。
    #[test]
    fn the_verbose_level_is_bounded_too() {
        let dir = std::env::temp_dir().join(format!("benagen-diag-verbose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("diag-kernel.log");

        // 4 MiB 上限、每行 180 字节 ⇒ 25 次写就能顶过一次上限（4800 字节/次）。
        // 写 3 倍于"三代写满"的量 ⇒ 三代都必须存在，而总量仍在界内。
        for i in 0..70_000 {
            write_at(&path, &format!("{i:0>180}"), Level::Verbose);
        }

        let main_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let g1 = std::fs::metadata(dir.join("diag-kernel.log.1")).map(|m| m.len()).unwrap_or(0);
        let g2 = std::fs::metadata(dir.join("diag-kernel.log.2")).map(|m| m.len()).unwrap_or(0);
        assert!(g2 > 0, "详细档要留两代，`.2` 从没出现过");
        assert!(
            main_len <= Level::Verbose.max_bytes() + LINE_BYTES,
            "主文件 {main_len} 字节，超过详细档上限加一行"
        );
        assert!(
            main_len + g1 + g2 <= 3 * Level::Verbose.max_bytes() + 3 * LINE_BYTES,
            "详细档总占用 {} 字节，超过三代上限加三行",
            main_len + g1 + g2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **非法级别不许静默退回 normal**（规格 §2.1）。
    #[test]
    fn an_unknown_level_is_rejected_by_name() {
        assert_eq!(parse_level("normal").expect("normal 合法"), Level::Normal);
        assert_eq!(parse_level("verbose").expect("verbose 合法"), Level::Verbose);
        let err = parse_level("loud").expect_err("不认识的级别必须报错");
        assert!(err.contains("loud"), "要点名是哪个值：{err}");
        assert!(err.contains("normal") && err.contains("verbose"), "要给出合法取值：{err}");
    }
}
