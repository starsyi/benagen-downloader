//! 壳侧的**有界**诊断日志（规格 §2.3 / §3 A6 的壳那一半）。
//!
//! # 为什么必须有它
//!
//! 内核那一份（`core/src/diagnostics.rs`）回答的是"**内核**当时在做什么"；
//! 客户报"下载引擎已断开"时，另一半问题——"**壳**当时在做什么、它给内核发了什么、
//! 内核怎么回的"——内核那一份里**一个字都没有**。没有这一份，回传的两个日志合起来
//! 仍然拼不出一次故障的完整现场。
//!
//! # 三件必须写在最前面的事
//!
//! 1. **级别不需要下发**：壳自己就知道自己开的是哪一档（偏好是壳自己存的），
//!    所以这里没有内核那条 `--log-level` 参数、也没有"把级别传给子进程"这一步。
//!    ⚠️ 内核那一半**需要**下发（它是个独立进程，读不到壳的偏好）；
//!    两边"级别的来源"不同，但**档位的语义与界**必须一致（见第 3 条）。
//!
//! 2. **这个模块一共有三份实现**（内核 Rust / 本文件 / macOS 壳 Swift），这是**架构强制**的：
//!    `shell-core/Cargo.toml` 头部写着本 crate **不依赖 `core`** —— 依赖它会把
//!    "语言中立的进程边界"降级成"两个 crate 的编译期耦合"（规格 §5 要点 1），
//!    日后内核换语言时这条边界就报错了。
//!    ⇒ 于是同一个模块写了两遍（外加 macOS 那一份 Swift 的），**改动必须三处成对**：
//!      ① `core/src/diagnostics.rs` —— 内核，写 `diag-kernel.log`；
//!      ② **本文件** —— Windows 壳，写 `diag-shell.log`；
//!      ③ `macos/Sources/BenagenCoreKit/DiagnosticsLog.swift` —— macOS 壳，写**同一个**
//!         `diag-shell.log`（那一侧 2026-10-06 之前**一行日志都没有**，由任务 5 补上）。
//!    动这里之前先打开另外两份对着改，反之亦然。
//!    ⚠️ "必须成对"这件事今天**有一条自动判据**（本文件的
//!    `the_two_loggers_agree_on_the_shared_constants`）：它 `include_str!` 内核那份源码，
//!    钉住 [`MAX_BYTES`] / [`MAX_VALUE_CHARS`] 两个常量两边一样。
//!    🔴 **它够不到 ③**：那是另一门语言、另一个构建系统的源码，`include_str!` 到不了它
//!    ⇒ ③ 与另外两份漂开时**不会有任何东西变红**（`DiagnosticsLogTests` 里那几条同形的
//!    用例只钉它自己的常量）。**改共享常量要三处一起看。**
//!    它**只**管得住这两个常量——函数体、行格式、轮转代数**没有**跨 crate 的判据，
//!    靠的是这份互相点名的注释与照抄。
//!
//!    ⚠️ 上面 ③ 那一格是**补的**（2026-10-06 任务 5 的任务审查实测出来的）：本条原先写
//!    "两份实现"，而"三份互相点名"那句当时**不成立** —— 两份 Rust 的模块头各自只点了对方，
//!    最可能来改共享常量的人恰恰没被指到第三份。
//!
//! 3. **行格式与截断规则必须与内核那份一致**：客户会把 `diag-shell.log` 与
//!    `diag-kernel.log` **一起**发给我们。两个文件的读法必须是**同一种**——
//!    字段行是 `ts=<unix 秒> event=<名> k=v k=v …`（见下面的 `format_line`），
//!    每个字段值截到 [`MAX_VALUE_CHARS`] 字符（见下）。
//!
//! # 两条硬约束（与内核那份逐字相同）
//!
//! 1. **有界**：**两档各自有界**，界线由 [`Level`] 给。
//!    - `normal`（**缺省**）：单文件上限 [`MAX_BYTES`]（1 MiB），超了轮转成 `.1`
//!      （**只留一代**）；
//!    - `verbose`（"每一次往返"那一档）：单文件 **4 MiB**、留**两代**（`.1` / `.2`）
//!      ⇒ 总上界 12 MiB。
//!    绝不无限增长——这是本仓对"落盘的东西"的一贯口径。
//!    ⚠️ 精确地说，轮转在**每次写入之前**检查，所以单文件瞬时上界是
//!    `max_bytes + 一行`（写第 N+1 行之前才看得见第 N 行把文件顶过了线）、
//!    总占用上界是 `(generations + 1) × (max_bytes + 一行)`。
//!    而「一行」自己也有上界：[`MAX_VALUE_CHARS`] 给每个字段值封顶 200 字符。
//!    **唯一**能顶破这个上界的条件是 `rename` 与兜底截断**两条都失败**（见下面的 `rotate`）：
//!    那时主文件会接着长——本模块不许为此把产品弄坏，所以它仍然无声。
//! 2. **永不许把产品弄坏**：本模块**没有一个函数把错误交出去**、没有一处我们自己的
//!    `unwrap`/`panic`**。写不进去（目录不可写、磁盘满、路径被占）时**无声跳过**，
//!    只在**第一次**失败时往 stderr 说一句。
//!
//!    ⚠️ **这一句与内核那份"逐字相同"的部分到此为止，后半句必须按本文件改**（2026-10-06，
//!    Task 2 的任务审查抓到）：内核那份的同一条写的是"没有一个函数返回 `Result`"，
//!    而**本文件里有一个** —— [`log_path`] 回 `Result<PathBuf, String>`，只是 [`log_at`]
//!    用 `let Ok(path) = … else { return; }` **就地把它吞掉了**。内核那份之所以照旧成立，
//!    是因为它的路径查询住在**模块外**（`core/src/paths.rs` 的 `diagnostics_log()`）。
//!    ⇒ 判据是"**不许有错误逃出本模块**"，不是"不许有函数返回 `Result`"。
//!
//!    ⚠️ 「没有一处我们自己的」这个限定词是承重的：`eprintln!` 本身在 stderr 不可写时
//!    **会 panic**，那是标准库的行为，不是我们写出来的 `unwrap`。
//!    ⚠️ **壳这一侧的诚实记账**：交付靶是 `windows` 子系统（没有控制台，
//!    见 `shell-win/src/main.rs` 的落点①）⇒ 那句 `eprintln!` 在 release 里**可能没有去处**。
//!    这不改本条的设计（"不弄坏产品"仍然成立，且开发构建里照样看得见），
//!    但**不许把它说成"失败了用户一定看得见"**——那是内核那一侧的情形
//!    （内核的 stderr 由壳排空），不是壳这一侧的。
//!
//! # 不记什么（"记什么"的另一半，**这一节的绝对句在 2026-10-06 之前是假的**）
//!
//! **不记**交付码全文、文件路径全文、`rpc-secret`、客户目录名。只记方法名、耗时、结果。
//! 诊断日志会被客户回传，它不该成为一份数据清单。
//!
//! 🔴 **但只写上面那一句会说谎**（终审实测）：`kernel_call` 那一行的 `why` 是**内核原文**，
//!    而交付码是**我们自己**拼进内核文案里的 —— `core/src/delivery.rs` 那条 404
//!    （`清单不存在（404）——请确认交付码是否正确：{url}`）把 `…/{交付码}/manifest.json`
//!    整条 URL 送了进来，`core/src/main.rs` 的 `preflight` 那一档同样带着客户目录名。
//!    ⇒ 正确的口径是**两条**，读的时候别只读上面那一句：
//!
//!   1. **按构造抹掉**：交付码与下载目录在**写进这个文件之前**被 [`redact`] 换成
//!      [`REDACTED`]。要抹的串由壳在**它自己知道的那一刻**推下来
//!      （`client.rs` 的 `CoreClient::set_redactions`，调用点
//!      `shell-win/src/session.rs` 的 `remember_for_redaction`：**请求里那个码** +
//!      偏好里那个目录）。
//!      ⚠️ **这一条只对"壳知道的那两个串"成立**（修复轮 2 收窄）：**没配置下载目录**
//!      （默认状态）时推下去的是空串（[`redact`] 跳过它）、也不传 `--download-dir`，
//!      内核回落到**它自己**的默认路径（`core/src/paths.rs`）—— 那条路径壳不知道，
//!      因此它出现在内核 `preflight` 失败文案里（`目标目录不可写（{dir}）`）时
//!      **抹不掉**。别把这一条读成"下载目录一定被抹掉了"。
//!   2. **仍然可能记到**：第三方（aria2 / 内核）的原始报错文案。它**原样**透传，
//!      里面也可能带上交付码 —— 那一份抹不掉：要可靠地识别任意第三方文案里的交付码，
//!      得先能识别它，而我们识别不了（规格 §2.7 的显式接受）。
//!
//!    ⚠️ [`redact`] 是**字面子串**替换，不是"保证干净了"：码以别的写法出现
//!    （换了大小写、被 URL 编码、被拆成两段）时它**不保证**能抹掉。
//!    本模块没有、也不假装有一张"这条路一定干净"的保证书。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// 单文件上限：1 MiB。轮转只留一代（`.1`）⇒ 总占用上界 `2 × MAX_BYTES` 加两行
/// （见模块头第 1 条：轮转在写入**之前**检查，每行另有 [`MAX_VALUE_CHARS`] 封顶）。
///
/// ⚠️ **必须与 `core/src/diagnostics.rs` 的同一个常量相等** —— 由
/// `the_two_loggers_agree_on_the_shared_constants` 钉住。
pub const MAX_BYTES: u64 = 1_048_576;

/// 单个字段值的字符上限：200。
///
/// 为什么需要它：`why` 里可能是**第三方**（内核 / 系统）的原始错误文案，长度不受我们
/// 控制。不截断的话"一行"可以是任意长——那「有界」就只约束了**行数**、没约束**字节数**。
///
/// ⚠️ **必须与 `core/src/diagnostics.rs` 的同一个常量相等**（同上，由那条跨 crate 用例钉住）。
pub const MAX_VALUE_CHARS: usize = 200;

/// 壳侧的日志文件名（与内核的 `diag-kernel.log` 同一个目录、不同的文件）。
///
/// ⚠️ **同目录**这件事靠的是既有的 `windows/scripts/check_shared_paths.sh`
///    （它守的是"壳与内核必须落在同一个目录、读同一个环境变量"），
///    **不是**构造保证：壳并不给内核传 `--settings`（`api/preferences.rs` 传的是 `None`），
///    两边是**各自**从 `%APPDATA%` 推出的同一个目录。
///    ⚠️ 两个进程**各写自己的文件**：同一个文件被两个进程追加 + 轮转会打架，
///    而诊断日志出问题的方式必须是"少记几条"，不能是"把彼此的账写坏"。
pub const FILE_NAME: &str = "diag-shell.log";

/// 第一次写失败只吵一句（否则每个事件都会往 stderr 刷一行）。
static COMPLAINED: AtomicBool = AtomicBool::new(false);

/// 诊断日志的档位。**缺省是 [`Level::Normal`]** —— 与内核那份同一套语义与界。
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

/// 解析日志档位的取值（壳从自己的偏好里读出来的那个串）。
///
/// ⚠️ **与内核那份逐字同形**（连报错话术也一样）：客户回传的两个文件要被同一种读法读，
///    档位的名字自然也得是同两个。
/// ⚠️ **不认识的取值必须报错**：静默退回 normal 的后果是
///    "客户以为开了详细日志、导出发给我们的却是一份普通档的"——而**没有任何东西会变红**。
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

/// 进程的级别。
///
/// ⚠️ 与内核那份同形：它做成进程级的静态，是因为 [`log`] 的调用点散在各处而
/// **没有任何一处拿得到配置**（壳的偏好读在别的地方，而日志是"哪里出错都要能记"的东西）。
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
/// **只记事件**：按**进程当前的级别**落。
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

/// 壳的日志文件在哪 —— 与内核的 `diag-kernel.log` 同一个目录（见 [`FILE_NAME`]）。
///
/// ⚠️ 取目录走的是 [`crate::storage::dir`]（壳自己那份标准目录表），
///    **不是**内核的 `paths::diagnostics_log()`：本 crate 看不见内核（见模块头第 2 条）。
///    "两边落到同一个目录"由 `windows/scripts/check_shared_paths.sh` 守着。
fn log_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::storage::dir()?.join(FILE_NAME))
}

/// 被 [`redact`] 抹掉的那一段文字（**两端、两种语言都用这一个形状**）。
///
/// ⚠️ 它**故意不是空串**：抹成空串的话，"这里本来有个码"这件事就看不出来了 ——
///    而看日志的人恰恰需要知道"内核那句原文里有一段被我们拿掉了"（否则他会以为
///    内核本来就只说了半句话）。同理也**不是** `***`：那不是中文语境里的话。
pub const REDACTED: &str = "[已隐去]";

/// **写盘之前**把 `value` 里出现的每一个 `secret` 换成 [`REDACTED`]（规格 §2.3 B 的
/// "能按构造就避开，就必须避开"）。
///
/// 它是**纯函数**：不读时钟、不碰文件、不看进程静态 —— 于是"抹不抹得掉"是一条能在
/// 宿主上跑的断言，而不是"等真机上导一次看看"。
///
/// ⚠️ **它是字面子串替换，不是过滤器，也不是保证**：
///   · 空串 `secret` 一律跳过（`str::replace("")` 会在每个字符之间插一段，把整行毁掉）；
///   · 码以别的写法出现（大小写不同、被 URL 编码、被拆成两段）时**抹不掉**；
///   · 它**不试图**去识别"任意第三方文案里像交付码的东西"——那是 §2.7 论证过做不到的事。
///     ⇒ 别把这一条读成"这份日志干净了"，它的口径见模块头那一节。
pub fn redact(value: &str, secrets: &[String]) -> String {
    let mut out = value.to_string();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        out = out.replace(secret.as_str(), REDACTED);
    }
    out
}

/// 落一条诊断的**唯一**出口，收一个**显式**级别（[`log`] 传进程级别、[`log_verbose`] 传
/// [`Level::Verbose`]）。
///
/// ⚠️ 两档的界（上限、留几代、轮转与兜底）**只有 [`write_at`] / [`rotate`] 一份实现** ——
///    本函数只负责"取路径、拼行、写"。
fn log_at(event: &str, fields: &[(&str, String)], level: Level) {
    let Ok(path) = log_path() else {
        return; // 连路径都定不下来：无声跳过（本模块不许有能力弄坏产品）
    };
    // ⚠️ 记录分隔符（`\n`）由**这里**补，不在 `format_line` 里。原因见测试
    //    `a_line_has_a_stable_shape`：那条按空格切开之后要求最后一格恰好是 `ok=true`，
    //    换行若留在 `format_line` 里就会变成 `ok=true\n`、那条断言必红。
    //    **测试是规格，实现向它看齐**（内核那份同一条）。
    write_at(&path, &format!("{}\n", format_line(event, fields)), level);
}

/// 字段行：`ts=<unix 秒> event=<名> k=v k=v …`（**不含**行尾换行，见 [`log_at`]）。
///
/// ⚠️ **`\n` 由 `log_at` 补**（不在本函数里）：换行若留在 `format_line` 里，测试
///    `a_line_has_a_stable_shape` 那条按空格切的断言必红（"测试是规格"）。
/// ⚠️ **这一行必须与内核那份逐字同形**（模块头第 3 条）：客户把两个文件一起发回来，
///    读法必须是同一种。
///
/// ⚠️ `ts` 是**UTC 的 unix 秒**（本 crate 没有日期库，也不为一行日志加一个）。
/// 换算不在这里自己实现历法，只给命令：macOS `date -r <ts>`、Linux `date -d @<ts>`。
fn format_line(event: &str, fields: &[(&str, String)]) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut s = format!("ts={ts} event={event}");
    for (k, v) in fields {
        // 值里的空格换成 `_`：这一行是按空格切字段读的，一个带空格的值会把字段切开。
        //
        // ⚠️ 同时**截断到 `MAX_VALUE_CHARS`**：`v` 里可能有第三方（内核 / 系统）的
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
    // stderr 不可写时**标准库自己会 panic**（见模块头第 2 条）。
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
/// ⚠️ **`Normal`（一代）就是两步**：循环一次都不转、只做 `path → .log.1`。
///
/// ⚠️ **`rename` 失败必须走兜底截断**（理由与顺序见 [`write_at`]）：
///    轮转失败而照样 append ⇒ 主文件无界增长，而那是唯一能破坏「有界」这条硬约束的条件。
fn rotate(path: &Path, level: Level) {
    for i in (1..level.generations()).rev() {
        let older = path.with_extension(format!("log.{}", i + 1));
        let newer = path.with_extension(format!("log.{i}"));
        let _ = std::fs::rename(&newer, &older);
    }
    let rotated = path.with_extension("log.1");
    if std::fs::rename(path, &rotated).is_err() {
        // ⚠️ **顺序承重：先截断、后吵**（理由见 [`write_at`] 那段注释，一个字都不改）。
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
    //! 与 `core/src/diagnostics.rs` 的测试**逐条对应**（模块头第 2 条：两边必须成对）。
    //!
    //! ⚠️ 下面这七条**照抄内核那份**：本模块与它同形，那么"界、轮转兜底、行形状、
    //!    档位取值"这几条判据也必须是同几条。多出来的第八条是本侧特有的
    //!    （跨 crate 的常量对齐），第九条在 `client.rs` 的 `mod tests` 里
    //!    （`kernel_call` 那一行）。

    use super::*;

    /// 本测试里每条记录的字节数（`{i:0>180}` 恰好 180 字节，`write_at` **不补**换行）。
    const LINE_BYTES: u64 = 180;

    /// **有界**：写到超过 1 MiB 必须轮转，且总占用有上限。
    ///
    /// ⚠️ 断言写的是**代码真正保证的**上界，不是"1 MiB"这个整数：轮转在写入**之前**
    /// 检查，单文件可以到 `MAX_BYTES + 一行`（见模块头）。
    ///
    /// 循环次数取 **11652** 是**算过的**，不是凑的：每条 180 字节 ⇒ 每 5826 次写
    /// 把主文件从 0 顶到 `> MAX_BYTES`，于是第 5827 次写发生**第一次**轮转、
    /// 第 11653 次发生第二次。11652 落在第二次轮转**前一次** ⇒ 恰好把主文件顶到
    /// 其最大值 `MAX_BYTES + 104`，即本测试覆盖的是**最坏那一格**。
    #[test]
    fn the_log_is_bounded_and_rotates() {
        let dir = std::env::temp_dir().join(format!("benagen-diag-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("diag-shell.log");

        // 11652 × 180 = 2,097,360 字节 ≈ 2.0 MiB ⇒ 恰好轮转一次（推导见 doc 注释）。
        // 级别显式写 `Normal`：这条钉的就是**缺省档**那条路径。
        for i in 0..11652 {
            write_at(&path, &format!("{i:0>180}"), Level::Normal);
        }

        let main_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let rotated_len = std::fs::metadata(dir.join("diag-shell.log.1"))
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
        let path = dir.join("diag-shell.log");
        std::fs::create_dir_all(dir.join("diag-shell.log.1")).expect("建 .1 目录失败");

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
    /// `is_whitespace() → '_'` 整段删掉，本测试必红。
    /// ⚠️ 这一条**同时**是与内核那份"读法相同"的判据：两个文件用的是同一个形状。
    #[test]
    fn a_line_has_a_stable_shape() {
        let line = format_line(
            "kernel_call",
            &[
                ("method", "get_state".into()),
                ("ok", "true".into()),
                ("why", "内核没了".into()),
            ],
        );
        let parts: Vec<&str> = line.split(' ').collect();
        assert!(parts[0].starts_with("ts="), "第一格必须是 ts=：{line}");
        assert_eq!(parts[1], "event=kernel_call");
        assert_eq!(parts[2], "method=get_state");
        assert_eq!(parts[3], "ok=true");
        assert_eq!(
            parts[4], "why=内核没了",
            "值里的空格必须换成 `_`，否则一个带空格的值会把字段切开：{line}"
        );
        assert_eq!(parts.len(), 5, "带空格的值被切成了多格：{line}");
    }

    /// 🔴 **normal 档一个字都不许变**：上限还是 1 MiB、还是只留一代。
    /// 两档的界必须与内核那份**逐格相同**（客户回传的两个文件按同一种读法读）。
    #[test]
    fn the_normal_level_keeps_todays_bounds() {
        assert_eq!(Level::Normal.max_bytes(), MAX_BYTES);
        assert_eq!(Level::Normal.generations(), 1);
        assert_eq!(Level::Verbose.max_bytes(), 4 * 1_048_576);
        assert_eq!(Level::Verbose.generations(), 2);
    }

    /// 🔴 **详细档仍然是有界的**。它记的是"每一次往返"，所以它不是"慢一点"而是
    /// "快十几倍" —— 没有上界的话客户开一晚上就是一个几 GB 的文件。
    /// 断言写的是**算得出来的**上界：单文件 `max_bytes + 一行`，总占用 `三代 + 三行`。
    #[test]
    fn the_verbose_level_is_bounded_too() {
        let dir = std::env::temp_dir().join(format!("benagen-diag-verbose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("diag-shell.log");

        // 4 MiB 上限、每行 180 字节 ⇒ 每 23302 行顶过一次上限。写 3 倍于"三代写满"的量
        // ⇒ 三代都必须存在，而总量仍在界内。
        for i in 0..70_000 {
            write_at(&path, &format!("{i:0>180}"), Level::Verbose);
        }

        let main_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let g1 = std::fs::metadata(dir.join("diag-shell.log.1")).map(|m| m.len()).unwrap_or(0);
        let g2 = std::fs::metadata(dir.join("diag-shell.log.2")).map(|m| m.len()).unwrap_or(0);
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

    /// **非法档位不许静默退回 normal**。
    #[test]
    fn an_unknown_level_is_rejected_by_name() {
        assert_eq!(parse_level("normal").expect("normal 合法"), Level::Normal);
        assert_eq!(parse_level("verbose").expect("verbose 合法"), Level::Verbose);
        let err = parse_level("loud").expect_err("不认识的档位必须报错");
        assert!(err.contains("loud"), "要点名是哪个值：{err}");
        assert!(err.contains("normal") && err.contains("verbose"), "要给出合法取值：{err}");
    }

    /// 🔴 **空串不许当 `secret`**：`str::replace("")` 会在**每个字符之间**插一段
    /// [`REDACTED`]，把整行毁掉（而它看起来只是"没抹掉什么"）。
    ///
    /// 判别力：把 `redact` 里那句 `if secret.is_empty() { continue; }` 删掉 ⇒ 本用例红。
    /// 真机上这一格是可达的：偏好里那个下载目录**空串 = 未配置**（`Preferences` 的
    /// 模块头写着这条），而空串正是"没配过"的取值 ⇒ 它会**每次都**被推下来。
    #[test]
    fn an_empty_secret_is_skipped_instead_of_shredding_the_line() {
        let secrets = vec![String::new(), "ABC123".to_string()];
        assert_eq!(redact("交付码 ABC123 不对", &secrets), "交付码 [已隐去] 不对");
        // 没有可抹的串时**逐字**原样交出去（这一条同时钉住"不许顺手改写别的东西"）。
        assert_eq!(redact("原样", &[]), "原样");
    }

    /// 🔴 **两份实现的常量必须一样**（架构强制了两份代码，那就必须有东西守着它们不漂）。
    ///
    /// 判别力：改这一侧的 `MAX_BYTES` 或 `MAX_VALUE_CHARS` 而没改内核那一侧 ⇒ 红。
    /// 它读的是**内核源码**（`include_str!`，跨 crate 但只是文本，不产生依赖）。
    ///
    /// ⚠️ 它守着的是**两个常量**，不是整个模块：行格式、轮转代数、`format_line` 的
    ///    行为都**没有**跨 crate 判据。别把这一条读成"两边已经对齐了"。
    /// ⚠️ `include_str!` 的路径若写错，**编译期就红**（那是好事）。
    #[test]
    fn the_two_loggers_agree_on_the_shared_constants() {
        let core_src = include_str!("../../../core/src/diagnostics.rs");
        assert!(core_src.contains("pub const MAX_BYTES: u64 = 1_048_576;"), "内核那份的上限变了");
        assert!(core_src.contains("pub const MAX_VALUE_CHARS: usize = 200;"), "内核那份的截断变了");
        assert_eq!(MAX_BYTES, 1_048_576);
        assert_eq!(MAX_VALUE_CHARS, 200);
    }
}
