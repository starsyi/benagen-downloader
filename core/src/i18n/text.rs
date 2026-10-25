//! 客户看得见的每一句话（规格 §2 的 W1）。
//!
//! 规矩只有三条，但每条都有用例钉着：
//!
//! 1. **两张表都必须写全**。`zh()`/`en()` 是对 `MsgKey` 的 `match`，少写一条是
//!    **编译错误**而不是运行时的空串——这是本模块存在的理由。
//! 2. **英文表里不许出现汉字**（`no_cjk_in_the_english_table` 遍历全部 key）。
//!    漏翻一条就会在英文界面上蹦出一句中文，那比不翻更难看。
//! 3. **中文表就是 0.2.9 的原句**（`zh_keeps_the_old_wording` 逐字比对）。翻译
//!    不该顺手改口径——文案的每一次改动都该是独立的一次决定。
//!
//! 内部日志、`eprintln!` 排障行、以及测试断言里的中文**不进这张表**：它们不是
//! 客户可见文案，翻它们只会让排障时 grep 不到（规格 I-2）。

use super::Lang;

/// 一条客户可见文案的键。
///
/// 命名规矩：`<对象>_<情形>`，对象在内核里是固定的几个（引擎、清单、磁盘、校验）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKey {
    /// 引擎被判定为"真的没了"（探活不通 + 进程也不动）。
    EngineDisconnected,
    /// 引擎回不上话，但进程在写盘/烧 CPU。
    EngineRpcSlow,
    /// 引擎回不上话，但探活是通的。
    EngineRpcFailed,
    /// 还没起过引擎就来要传输列表。
    EngineNotStarted,
    /// 引擎进程起不来。
    EngineStartFailed,
    /// 目标目录建不出来/不可用。
    PreflightDirUnusable,
    /// 目标目录能建但写不进去（只读盘、权限、盘被拔）。
    PreflightNotWritable,
    /// 剩余空间不够这一批。
    PreflightNoSpace,
    /// CLI 收尾的成功行。
    CliAllDone,
    /// CLI 收尾：有文件没下完。
    CliIncomplete,
    /// CLI 收尾：校验没通过。
    CliVerifyFailed,
    /// 清单 404——最常见的失败，值得一句能照做的提示。
    DeliveryNotFound,
    /// 清单里有 Windows 建不出来的文件名（冒号、通配符、保留设备名…）。
    InvalidFilename,

    // ── CLI（T8）───────────────────────────────────────────────────────────
    // 帮助与用法错误发生在**内核起来之前**，所以它们的语言由 `args::pick_lang`
    // 按「--lang > BENAGEN_DL_LANG > 系统 locale」先定下来。
    /// `--help` 全文。
    CliHelp,
    /// 用法里那一截 `<交付码或完整链接> [选项]`（多条错误消息共用）。
    CliUsageSnippet,
    CliErrOnlyOneCode,
    CliErrMissingFolder,
    CliErrMissingInteger,
    CliErrUnknownOption,
    CliErrNotAnInteger,
    CliErrNoCode,
    CliErrBadLang,
    CliErrLangNoValue,
    /// 内核失败那一行的**壳的外框**（内核原文逐字进 `{message}`）。
    CliKernelError,
    /// 内核失败但没给详情时的兜底。
    CliNoErrorDetail,
    /// `--version` 那一行。
    CliVersion,
    /// 收尾关引擎失败。
    CliEngineCloseFailed,
    /// 被 Ctrl-C 中断。
    CliInterrupted,
    /// 复核发现不完整、补发入队之后（确实加了）。
    CliRecheckAdded,
    /// 复核发现不完整、补发入队之后（一个都没加）。
    CliRecheckNotAdded,
    /// 复核与内核两边对不上，停下。
    CliRecheckMismatch,
    /// 上面那一句的下一行：现在的六类计数。
    CliNowCounts,
    /// `enqueue` 回执里有文件被拒时的那行标题。
    CliRejectedHeader,
    /// 内核没给路径时的占位（拒绝清单与停滞清单共用）。
    CliNoPathFromKernel,
    /// 任务失败但没有 error_message 时的占位。
    CliNoFailureReason,
    /// 传输列表里没有还在进行中的条目时的占位。
    CliNoActiveTransfer,
    /// 下载阶段停滞的标题。
    CliStallHeading,
    /// 校验阶段停滞的标题。
    CliVerifyStallHeading,
    /// 校验阶段停滞的尾行（Ctrl-C 这条出口）。
    CliVerifyStallTail,
    /// 校验六类计数的写法（停滞汇报与校验失败那行共用）。
    CliVerifyCounts,
    /// 进度行。
    CliProgressLine,
    /// 进度行（有失败项时）。
    CliProgressLineFailed,
    /// 退出码 0 的收口语。
    CliExitOk,
    /// 退出码 1 的收口语。
    CliExitUsage,
    /// 退出码 2 的收口语。
    CliExitManifest,
    /// 退出码 3 的收口语。
    CliExitPreflight,
    /// 退出码 4 的收口语。
    CliExitEngine,
    /// 退出码 5 的收口语。
    CliExitIncomplete,
    /// 退出码 6 的收口语。
    CliExitVerifyFailed,
    /// 退出码 130 的收口语。
    CliExitInterrupted,
    SettingsParallelRange,
    SettingsConnectionsRange,
    SettingsSplitsRange,
    SettingsLimitRange,
    SettingsTriesRange,
    SettingsWaitRange,
    SettingsSizeSuffix,
    SettingsSizeUnparsable,
    SettingsSizeRange,
}

/// 查一句文案。
///
/// `Lang::System` 在这里被当成英文处理——**调用方必须先 [`Lang::resolve`]**，
/// 这一条只是"真有人忘了"时不至于 panic 的兜底（是英文而不是中文，理由同规格 §2.1：
/// 看不懂的英文强于看不懂的中文）。
pub fn t(lang: Lang, key: MsgKey) -> &'static str {
    match lang {
        Lang::Zh => zh(key),
        Lang::En | Lang::System => en(key),
    }
}

/// 查一句带占位符的文案，并按顺序替换 `{name}`。
///
/// 占位符用大括号而不是 `{}`：这些句子会原样进日志与界面，`{op}` 这种写法
/// 在人工阅读时也一眼能看出"这里要填东西"。
pub fn tf(lang: Lang, key: MsgKey, args: &[(&str, &str)]) -> String {
    let mut out = t(lang, key).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

fn zh(key: MsgKey) -> &'static str {
    match key {
        MsgKey::EngineDisconnected => {
            "下载引擎已断开：{cause}；已下载的文件保留在目标目录，可直接重试"
        }
        MsgKey::EngineRpcSlow => {
            "下载引擎响应很慢（仍在写盘，任务没有停，稍后会自动恢复）：{op}：{err}"
        }
        MsgKey::EngineRpcFailed => "下载引擎调用失败（重连探活正常，可重试）：{op}：{err}",
        MsgKey::EngineNotStarted => "下载引擎尚未启动（先 enqueue 才会起引擎）",
        MsgKey::EngineStartFailed => "启动下载引擎失败：{err}",
        MsgKey::PreflightDirUnusable => "目标目录不可用（{dir}）：{err}",
        MsgKey::PreflightNotWritable => "目标目录不可写（{dir}）：{err}",
        MsgKey::PreflightNoSpace => {
            "磁盘剩余空间不足：本次需要 {need} 字节，{dir} 只剩 {free} 字节"
        }
        MsgKey::CliAllDone => "全部完成并校验通过 —— {files} 个文件，{bytes}，{dir}",
        MsgKey::CliIncomplete => "有 {failed} 个文件没能下完：",
        MsgKey::CliVerifyFailed => "校验没通过 —— {counts}",
        MsgKey::DeliveryNotFound => "清单不存在（404）——请确认交付码是否正确：{url}",
        MsgKey::InvalidFilename => "这批数据里有 {count} 个文件名在 Windows 上建不出来（{reason}，示例：{example}）。文件本身没问题，是名字里带了 Windows 不允许的东西。请联系交付方改名，或改用 macOS / Linux 客户端下载。",

        // ── CLI（T8）───────────────────────────────────────────────────────
        // ⚠️ 下面这些中文是 0.2.9 里**逐字存在**的原句：只许搬进这张表，
        // 一个字都不许顺手改（`zh_keeps_the_cli_wording` 逐条钉着）。
        MsgKey::CliHelp => r#"benagen-dl —— 把一批交付数据完整下载到本地

用法：
  benagen-dl <交付码或完整链接> [选项]

选项：
  -o <目录>     下载到哪（默认 $HOME/Downloads/Benagen）
  -j <N>        并行文件数，1–64（默认 5）
  -x <N>        单文件连接数，1–16（默认 8）
  -s <N>        分片数，1–16（默认 8）
  --lang <L>    界面语言：zh、en 或 system（默认跟随系统）
  --quiet       只打印错误与最终结论
  --license     打印内嵌组件（aria2）的 GPLv2 全文
  --version     打印版本与内嵌 aria2c 的摘要
  --help        打印这段说明

环境变量：
  BENAGEN_BASE_URL   清单与文件的基址（默认 http://download.benagen.com）
  BENAGEN_DL_LANG    界面语言，--lang 优先于它（默认跟随系统）

退出码：
  0 全部完成且校验通过   1 用法错误   2 拉清单失败   3 目标目录不可用
  4 引擎起不来           5 下载未完成 6 校验不通过   130 被 Ctrl-C 中断（可续传）

中断或断网后，**再跑一次同一条命令**就会接着下。
"#,
        MsgKey::CliUsageSnippet => "benagen-dl <交付码或完整链接> [选项]",
        MsgKey::CliErrOnlyOneCode => "只能给一个交付码。用法：{usage}",
        MsgKey::CliErrMissingFolder => "{arg} 后面要跟一个目录。用法：{usage}",
        MsgKey::CliErrMissingInteger => "{arg} 后面要跟一个整数。用法：{usage}",
        MsgKey::CliErrUnknownOption => "不认识的选项 {other}。用法：{usage}",
        MsgKey::CliErrNotAnInteger => "{arg} 需要一个整数，收到 {value}",
        MsgKey::CliErrNoCode => {
            "缺交付码。用法：{usage}，例如 benagen-dl ABCdef1234567890abcd"
        }
        MsgKey::CliErrBadLang => "--lang 只认 zh、en 或 system，收到 {value}。用法：{usage}",
        MsgKey::CliErrLangNoValue => "--lang 后面要跟 zh、en 或 system。用法：{usage}",
        MsgKey::CliKernelError => "benagen-dl: {message}（{code}）",
        MsgKey::CliNoErrorDetail => "内核没有给出错误详情",
        MsgKey::CliVersion => "benagen-dl {version}（内嵌 aria2c {aria2c}）",
        MsgKey::CliEngineCloseFailed => "benagen-dl: 关闭下载引擎时出错: {err}",
        MsgKey::CliInterrupted => "benagen-dl: 被 Ctrl-C 中断 —— 再跑一次同样的命令会接着下",
        MsgKey::CliRecheckAdded => {
            "benagen-dl: 复核时发现这批文件已经不再完整（有文件不在盘上或被改动过）\
             —— 已经把缺的重新加入下载，继续等。"
        }
        MsgKey::CliRecheckNotAdded => {
            "benagen-dl: 复核时发现这批文件已经不再完整（有文件不在盘上或被改动过）\
             —— 这次没有新加的任务（已经有任务在跑），继续等。"
        }
        MsgKey::CliRecheckMismatch => {
            "benagen-dl: 复核说这批文件不完整，而内核说没有待下载的文件 —— 两边对不上，停下。"
        }
        MsgKey::CliNowCounts => "  现在：{counts}",
        MsgKey::CliRejectedHeader => {
            "benagen-dl: 有 {count} 个文件没能进入下载队列（下面的行是内核原文）："
        }
        MsgKey::CliNoPathFromKernel => "（内核没给路径）",
        MsgKey::CliNoFailureReason => "（内核没有给出失败原因）",
        MsgKey::CliNoActiveTransfer => "（传输列表里没有还在进行中的条目）",
        MsgKey::CliStallHeading => "benagen-dl: 连续 10 分钟没有任何进展，停下。停着的是：",
        MsgKey::CliVerifyStallHeading => {
            "benagen-dl: 下载都已经结束，卡在校验阶段 —— 已经 {minutes} 分钟没有任何变化："
        }
        MsgKey::CliVerifyStallTail => {
            "benagen-dl: 大文件的 crc64 校验可能要很久（单个文件不可再分）。\
             按 Ctrl-C 可以停下，再跑一次会接着下。"
        }
        MsgKey::CliVerifyCounts => {
            "通过 {ok}，不符 {bad}，缺失 {missing}，大小不符 {size_mismatch}，读不了 {unreadable}，无法校验 {unverifiable}"
        }
        MsgKey::CliProgressLine => {
            "已下 {done_files}/{total_files} · {done_bytes} / {total_bytes} · {percent} · {speed}"
        }
        MsgKey::CliProgressLineFailed => {
            "已下 {done_files}/{total_files} · {done_bytes} / {total_bytes} · {percent} · {speed} · 失败 {failed}"
        }
        MsgKey::CliExitOk => "全部完成，校验通过",
        MsgKey::CliExitUsage => "命令行用法不对——照上面的说明改一下再跑",
        MsgKey::CliExitManifest => "清单没拉到——先确认交付码与网络，再跑一次",
        MsgKey::CliExitPreflight => "这个目录用不了——换一个可写、空间够的目录（-o）再跑",
        MsgKey::CliExitEngine => "下载引擎没起来——这是本机环境的问题，把上面的报错发给我们",
        MsgKey::CliExitIncomplete => "下载没有完成——再跑一次会接着下",
        MsgKey::CliExitVerifyFailed => "文件都下完了，但校验没通过与下一行一起看是哪几类",
        MsgKey::CliExitInterrupted => "被中断（再跑一次会接着下）",
        MsgKey::SettingsParallelRange => "并行文件数必须在 1–64 之间，当前 {value}",
        MsgKey::SettingsConnectionsRange => "单文件连接数必须在 1–16 之间，当前 {value}",
        MsgKey::SettingsSplitsRange => "分片数必须在 1–16 之间，当前 {value}",
        MsgKey::SettingsLimitRange => {
            "限速必须在 0–{max} MB/s 之间（上界为防溢出而设），当前 {value}"
        }
        MsgKey::SettingsTriesRange => "重试次数必须在 1–100 之间，当前 {value}",
        MsgKey::SettingsWaitRange => "重试间隔必须在 0–60 秒之间，当前 {value}",
        MsgKey::SettingsSizeSuffix => "最小分片大小须以 M 结尾（如 20M），当前 {value}",
        MsgKey::SettingsSizeUnparsable => "最小分片大小无法解析: {value}",
        MsgKey::SettingsSizeRange => "最小分片大小必须在 1M–100M 之间，当前 {value}",
    }
}

fn en(key: MsgKey) -> &'static str {
    match key {
        MsgKey::EngineDisconnected => {
            "The download engine has stopped responding: {cause}. Files already downloaded are kept in the target folder, so you can retry right away."
        }
        MsgKey::EngineRpcSlow => {
            "The download engine is slow to answer, but it is still writing to disk and your tasks have not stopped. It should recover on its own: {op}: {err}"
        }
        MsgKey::EngineRpcFailed => {
            "The download engine call failed, but the engine answers pings, so retrying is safe: {op}: {err}"
        }
        MsgKey::EngineNotStarted => "The download engine has not started yet (it starts on the first enqueue).",
        MsgKey::EngineStartFailed => "Failed to start the download engine: {err}",
        MsgKey::PreflightDirUnusable => "The target folder is unusable ({dir}): {err}",
        MsgKey::PreflightNotWritable => "The target folder is not writable ({dir}): {err}",
        MsgKey::PreflightNoSpace => {
            "Not enough free space: this job needs {need} bytes, but {dir} has only {free} bytes left."
        }
        MsgKey::CliAllDone => "All files finished and verified - {files} files, {bytes}, {dir}",
        MsgKey::CliIncomplete => "{failed} file(s) could not be finished:",
        MsgKey::CliVerifyFailed => "Verification failed - {counts}",
        MsgKey::DeliveryNotFound => "The file list was not found (404) - please check the delivery code: {url}",
        MsgKey::InvalidFilename => "This batch has {count} file name(s) that Windows cannot create ({reason}, for example: {example}). The files themselves are fine - the names just contain characters Windows does not allow. Ask the sender to rename them, or download with the macOS / Linux client.",

        // ── CLI (T8)─────────────────────────────────────────────────────────
        MsgKey::CliHelp => r#"benagen-dl - download a delivery batch in full

Usage:
  benagen-dl <delivery code or full link> [options]

Options:
  -o <folder>   where to download (default $HOME/Downloads/Benagen)
  -j <N>        parallel files, 1-64 (default 5)
  -x <N>        connections per file, 1-16 (default 8)
  -s <N>        splits, 1-16 (default 8)
  --lang <L>    interface language: zh, en or system (default: follow the system)
  --quiet       print only errors and the final result
  --license     print the full GPLv2 text of the bundled component (aria2)
  --version     print the version and the digest of the bundled aria2c
  --help        print this text

Environment:
  BENAGEN_BASE_URL   base URL for the file list and the files (default http://download.benagen.com)
  BENAGEN_DL_LANG    interface language; --lang wins over it (default: follow the system)

Exit codes:
  0 all files done and verified   1 usage error   2 file list failed   3 target folder unusable
  4 engine failed to start        5 download incomplete   6 verification failed
  130 interrupted by Ctrl-C (resumable)

After an interruption or a network drop, running the same command again will resume the download.
"#,
        MsgKey::CliUsageSnippet => "benagen-dl <delivery code or full link> [options]",
        MsgKey::CliErrOnlyOneCode => "Only one delivery code can be given. Usage: {usage}",
        MsgKey::CliErrMissingFolder => "{arg} needs a folder after it. Usage: {usage}",
        MsgKey::CliErrMissingInteger => "{arg} needs an integer after it. Usage: {usage}",
        MsgKey::CliErrUnknownOption => "Unknown option {other}. Usage: {usage}",
        MsgKey::CliErrNotAnInteger => "{arg} needs an integer, got {value}",
        MsgKey::CliErrNoCode => {
            "Missing delivery code. Usage: {usage}, for example benagen-dl ABCdef1234567890abcd"
        }
        MsgKey::CliErrBadLang => {
            "--lang accepts only zh, en or system; got {value}. Usage: {usage}"
        }
        MsgKey::CliErrLangNoValue => "--lang needs zh, en or system after it. Usage: {usage}",
        MsgKey::CliKernelError => "benagen-dl: {message} ({code})",
        MsgKey::CliNoErrorDetail => "the kernel gave no error detail",
        MsgKey::CliVersion => "benagen-dl {version} (bundled aria2c {aria2c})",
        MsgKey::CliEngineCloseFailed => "benagen-dl: failed to close the download engine: {err}",
        MsgKey::CliInterrupted => {
            "benagen-dl: interrupted by Ctrl-C - running the same command again will resume the download"
        }
        MsgKey::CliRecheckAdded => {
            "benagen-dl: the final check found that this batch is no longer complete (some files are missing from disk or were changed) \
             - the missing files have been queued again, so we keep waiting."
        }
        MsgKey::CliRecheckNotAdded => {
            "benagen-dl: the final check found that this batch is no longer complete (some files are missing from disk or were changed) \
             - no new tasks were added this time (some are already running), so we keep waiting."
        }
        MsgKey::CliRecheckMismatch => {
            "benagen-dl: the final check says this batch is incomplete, but the kernel says there is nothing left to download \
             - the two do not agree, so we stop."
        }
        MsgKey::CliNowCounts => "  Now: {counts}",
        MsgKey::CliRejectedHeader => {
            "benagen-dl: {count} file(s) could not be queued for download (the lines below are the kernel's own words):"
        }
        MsgKey::CliNoPathFromKernel => "(the kernel gave no path)",
        MsgKey::CliNoFailureReason => "(the kernel gave no failure reason)",
        MsgKey::CliNoActiveTransfer => "(the transfer list has no entry still in progress)",
        MsgKey::CliStallHeading => "benagen-dl: no progress for 10 minutes, stopping. What is stuck:",
        MsgKey::CliVerifyStallHeading => {
            "benagen-dl: all downloads have finished and verification is still running - no change for {minutes} minute(s):"
        }
        MsgKey::CliVerifyStallTail => {
            "benagen-dl: crc64 verification of a large file can take a long time (a single file cannot be split further). \
             Press Ctrl-C to stop; running the command again will resume."
        }
        MsgKey::CliVerifyCounts => {
            "passed {ok}, mismatched {bad}, missing {missing}, size mismatch {size_mismatch}, unreadable {unreadable}, unverifiable {unverifiable}"
        }
        MsgKey::CliProgressLine => {
            "Downloaded {done_files}/{total_files} · {done_bytes} / {total_bytes} · {percent} · {speed}"
        }
        MsgKey::CliProgressLineFailed => {
            "Downloaded {done_files}/{total_files} · {done_bytes} / {total_bytes} · {percent} · {speed} · {failed} failed"
        }
        MsgKey::CliExitOk => "All done, verification passed",
        MsgKey::CliExitUsage => "The command line is wrong - fix it as described above and run again",
        MsgKey::CliExitManifest => {
            "Could not fetch the file list - check the delivery code and your network, then run again"
        }
        MsgKey::CliExitPreflight => {
            "That folder cannot be used - pick a writable folder with enough space (-o) and run again"
        }
        MsgKey::CliExitEngine => {
            "The download engine did not start - this is a problem with this machine's environment; send us the error above"
        }
        MsgKey::CliExitIncomplete => "The download did not finish - running again will resume",
        MsgKey::CliExitVerifyFailed => {
            "All files have downloaded, but verification failed - read the line below to see which categories"
        }
        MsgKey::CliExitInterrupted => "Interrupted (running again will resume)",
        MsgKey::SettingsParallelRange => "Parallel files must be between 1 and 64, got {value}",
        MsgKey::SettingsConnectionsRange => {
            "Connections per file must be between 1 and 16, got {value}"
        }
        MsgKey::SettingsSplitsRange => "Splits must be between 1 and 16, got {value}",
        MsgKey::SettingsLimitRange => {
            "The speed limit must be between 0 and {max} MB/s (the upper bound prevents overflow), got {value}"
        }
        MsgKey::SettingsTriesRange => "Retries must be between 1 and 100, got {value}",
        MsgKey::SettingsWaitRange => "Retry wait must be between 0 and 60 seconds, got {value}",
        MsgKey::SettingsSizeSuffix => {
            "The minimum split size must end with M (for example 20M), got {value}"
        }
        MsgKey::SettingsSizeUnparsable => "The minimum split size could not be parsed: {value}",
        MsgKey::SettingsSizeRange => {
            "The minimum split size must be between 1M and 100M, got {value}"
        }
    }
}

/// 全部 key（穷尽性靠它 + `match` 双重保险：加了新 key 忘了进表，这条用例会红）。
#[cfg(test)]
pub const ALL: &[MsgKey] = &[
    MsgKey::EngineDisconnected,
    MsgKey::EngineRpcSlow,
    MsgKey::EngineRpcFailed,
    MsgKey::EngineNotStarted,
    MsgKey::EngineStartFailed,
    MsgKey::PreflightDirUnusable,
    MsgKey::PreflightNotWritable,
    MsgKey::PreflightNoSpace,
    MsgKey::CliAllDone,
    MsgKey::CliIncomplete,
    MsgKey::CliVerifyFailed,
    MsgKey::DeliveryNotFound,
    MsgKey::InvalidFilename,
    MsgKey::CliHelp,
    MsgKey::CliUsageSnippet,
    MsgKey::CliErrOnlyOneCode,
    MsgKey::CliErrMissingFolder,
    MsgKey::CliErrMissingInteger,
    MsgKey::CliErrUnknownOption,
    MsgKey::CliErrNotAnInteger,
    MsgKey::CliErrNoCode,
    MsgKey::CliErrBadLang,
    MsgKey::CliErrLangNoValue,
    MsgKey::CliKernelError,
    MsgKey::CliNoErrorDetail,
    MsgKey::CliVersion,
    MsgKey::CliEngineCloseFailed,
    MsgKey::CliInterrupted,
    MsgKey::CliRecheckAdded,
    MsgKey::CliRecheckNotAdded,
    MsgKey::CliRecheckMismatch,
    MsgKey::CliNowCounts,
    MsgKey::CliRejectedHeader,
    MsgKey::CliNoPathFromKernel,
    MsgKey::CliNoFailureReason,
    MsgKey::CliNoActiveTransfer,
    MsgKey::CliStallHeading,
    MsgKey::CliVerifyStallHeading,
    MsgKey::CliVerifyStallTail,
    MsgKey::CliVerifyCounts,
    MsgKey::CliProgressLine,
    MsgKey::CliProgressLineFailed,
    MsgKey::CliExitOk,
    MsgKey::CliExitUsage,
    MsgKey::CliExitManifest,
    MsgKey::CliExitPreflight,
    MsgKey::CliExitEngine,
    MsgKey::CliExitIncomplete,
    MsgKey::CliExitVerifyFailed,
    MsgKey::CliExitInterrupted,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据：英文表里**一个汉字都不能有**。
    #[test]
    fn no_cjk_in_the_english_table() {
        for k in ALL {
            let s = en(*k);
            assert!(
                !s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{k:?} 的英文里混进了中文: {s}"
            );
        }
    }

    /// 判据：中文表与 0.2.9 的原句**逐字相同**（翻译不许顺手改口径）。
    #[test]
    fn zh_keeps_the_old_wording() {
        assert_eq!(
            t(Lang::Zh, MsgKey::EngineDisconnected),
            "下载引擎已断开：{cause}；已下载的文件保留在目标目录，可直接重试"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::EngineRpcSlow),
            "下载引擎响应很慢（仍在写盘，任务没有停，稍后会自动恢复）：{op}：{err}"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::EngineRpcFailed),
            "下载引擎调用失败（重连探活正常，可重试）：{op}：{err}"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::EngineNotStarted),
            "下载引擎尚未启动（先 enqueue 才会起引擎）"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::EngineStartFailed),
            "启动下载引擎失败：{err}"
        );
    }

    /// 判据：CLI 那一批中文是 0.2.9 的原句**逐字**搬进来的（T8 只加英文，不改中文）。
    ///
    /// ⚠️ 为什么值得单列：这些句子原先散在 `cli/args.rs`、`cli/exit.rs` 里，
    /// 搬进表时最容易"顺手润色"一下——而客户已经见过它们，改口径要单独一次决定。
    #[test]
    fn zh_keeps_the_cli_wording() {
        assert_eq!(
            t(Lang::Zh, MsgKey::CliExitOk),
            "全部完成，校验通过"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::CliExitInterrupted),
            "被中断（再跑一次会接着下）"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::CliProgressLineFailed),
            "已下 {done_files}/{total_files} · {done_bytes} / {total_bytes} · {percent} · {speed} · 失败 {failed}"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::CliVersion),
            "benagen-dl {version}（内嵌 aria2c {aria2c}）"
        );
        assert_eq!(
            t(Lang::Zh, MsgKey::CliErrNoCode),
            "缺交付码。用法：{usage}，例如 benagen-dl ABCdef1234567890abcd"
        );
        assert_eq!(t(Lang::Zh, MsgKey::CliUsageSnippet), "benagen-dl <交付码或完整链接> [选项]");
        // 帮助里原来那一句承诺与默认值都还在（只多了 --lang / BENAGEN_DL_LANG 两行）
        let help = t(Lang::Zh, MsgKey::CliHelp);
        for must in [
            "benagen-dl —— 把一批交付数据完整下载到本地",
            "-o <目录>     下载到哪（默认 $HOME/Downloads/Benagen）",
            "-j <N>        并行文件数，1–64（默认 5）",
            "--lang <L>    界面语言：zh、en 或 system（默认跟随系统）",
            "BENAGEN_DL_LANG    界面语言，--lang 优先于它（默认跟随系统）",
            "中断或断网后，**再跑一次同一条命令**就会接着下。",
        ] {
            assert!(help.contains(must), "帮助里少了 {must:?}：\n{help}");
        }
    }

    /// 判据：两张表都不为空，且都不带没替换掉的占位符（漏传参数会在这里露头）。
    #[test]
    fn placeholders_are_filled_or_at_least_named() {
        for k in ALL {
            for lang in [Lang::Zh, Lang::En] {
                let s = t(lang, *k);
                assert!(!s.trim().is_empty(), "{k:?}/{lang:?} 是空串");
                // 占位符必须是 `{ident}` 形状，不能是裸的 `{}`（裸的没法按名替换）
                let mut rest = s;
                while let Some(i) = rest.find('{') {
                    let j = rest[i..].find('}').expect("占位符没闭合") + i;
                    let name = &rest[i + 1..j];
                    assert!(
                        !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                        "{k:?}/{lang:?} 的占位符不像名字: {{{name}}}"
                    );
                    rest = &rest[j + 1..];
                }
            }
        }
    }

    /// 判据：`tf` 真的替换，且替换后不再残留该占位符。
    #[test]
    fn tf_substitutes_every_named_slot() {
        let s = tf(Lang::Zh, MsgKey::EngineStartFailed, &[("err", "端口被占")]);
        assert_eq!(s, "启动下载引擎失败：端口被占");
        let s = tf(
            Lang::En,
            MsgKey::EngineRpcFailed,
            &[("op", "tellActive"), ("err", "timeout")],
        );
        assert!(s.contains("tellActive") && s.contains("timeout"), "{s}");
        assert!(!s.contains("{op}") && !s.contains("{err}"), "{s}");
    }

    /// 判据：`System` 落到英文（调用方本该先 resolve；真忘了也不许吐中文给英文用户）。
    #[test]
    fn an_unresolved_system_lang_falls_back_to_english() {
        assert_eq!(t(Lang::System, MsgKey::EngineNotStarted), en(MsgKey::EngineNotStarted));
    }
}
