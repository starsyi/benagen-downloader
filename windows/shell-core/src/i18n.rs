//! 壳自己写的那几句话的双语表（规格 W1 / I-2）。
//!
//! ⚠️ **为什么壳里也要一张表**：内核给的文案已经双语（`core/src/i18n`），但壳还有一批
//!    *它自己写的* 句子 —— 引擎徽标、校验摘要的行标签、慢盘提醒、握手超时提示……
//!    它们没有"内核原文"可以照登（约束 3 只覆盖内核说的那部分），所以必须同样双语。
//!
//! 两条纪律：
//!
//!   1. **判据在 Rust、文案在表里、前端只画**（I-1）：`windows/web` 里不许出现任何
//!      新的界面文案，`render.js` 只把这里给的字放进 DOM。
//!   2. **日志不翻译**（I-2）：这里只管客户看得见的字。
//!
//! 与内核那套的分工：内核的 `Lang` 由握手落定（`hello` 回执里的 `language`），壳从回执里
//! 抄一份到 [`crate::session_view::SessionView::lang`]，此后所有壳侧构造点都读那一格 ——
//! **壳不自己看环境变量**，否则两端各算一次、语言就会在一句话里打架。

/// 界面语言。与内核 `core/src/i18n::Lang` 的**线上三个词**对齐（`system/zh/en`），
/// 但壳这边只保留**已落定**的两档：`System` 早在握手时就被内核解析掉了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// 中文（默认：没握手成功、没拿到语言时按中文画，与 0.2.x 的界面一致）。
    #[default]
    Zh,
    En,
}

impl Lang {
    /// 线上那个词（`zh` / `en`）——**永远不是** `system`（同内核的 `resolved_is_never_system`）。
    pub fn wire(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// 从握手回执里读。缺字段 / 不认识的一律当默认中文——
    /// **不许**因为一个语言字段把整个握手判失败（客户端启动不了比界面语言错更糟）。
    pub fn from_hello(language: Option<&str>) -> Lang {
        match language.map(str::trim) {
            Some("en") | Some("en-US") | Some("en_US") => Lang::En,
            _ => Lang::Zh,
        }
    }
}

/// 壳侧文案的键（闭集：少写一条就是编译错误，见 [`t`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // —— 引擎徽标 ——
    EngineConnecting,
    EngineNotStarted,
    EngineRunning,
    EngineUnavailablePrefix,
    EngineHandshakeTimeout,
    EngineHandshakeTooltip,
    EngineConnectingTooltip,
    EngineNotStartedTooltip,
    EngineRunningTooltip,
    // —— 下载动作那一组 ——
    DownloadAll,
    DownloadSelected,
    DownloadHelp,
    EmptySelectionHint,
    OverBudgetHint,
    // —— 设置页：下载目录与详细日志 ——
    PrefDirSectionNote,
    PrefVerboseLabel,
    PrefVerboseNote,
    PrefVerboseChangedOn,
    PrefVerboseChangedOff,
    PrefVerboseUnchangedOn,
    PrefDirChangedTitle,
    PrefDirChangedFull,
    PrefDirRestoredDefault,
    PrefDirUnchangedTitle,
    // —— 设置页：七项参数 ——
    SettingsParamParallel,
    SettingsParamConnections,
    SettingsParamSplits,
    SettingsParamMinSplit,
    SettingsParamLimit,
    SettingsParamMaxTries,
    SettingsParamRetryWait,
    SettingsMinSplitNote,
    SettingsLimitNote,
    SettingsApplyNote,
    SettingsUnsavedBanner,
    SettingsCleanBanner,
    // —— 设置页：界面语言那一节 ——
    SettingsLanguageTitle,
    SettingsLanguageNote,
    // —— 慢盘提醒 ——
    DiskSlow,
    // —— 校验摘要 ——
    VerifyPassed,
    VerifyMismatched,
    VerifyMissing,
    VerifySizeMismatch,
    VerifyUnverifiable,
    VerifyUnreadable,
    VerifyUnverifiableNote,
    VerifyNotVerified,
    VerifyAllGood,
    VerifyFailedPrefix,
    VerifyUnaccounted,
    VerifyCheckedPrefix,
    VerifyItemsSuffix,
}

/// 取一句壳侧文案。`System` 不会出现在这里（见 [`Lang`]）。
pub fn t(lang: Lang, key: Key) -> &'static str {
    match lang {
        Lang::Zh => zh(key),
        Lang::En => en(key),
    }
}

/// 带占位符的版本：按名替换 `{name}`（与内核 `i18n::tf` 同一套写法）。
pub fn tf(lang: Lang, key: Key, args: &[(&str, &str)]) -> String {
    let mut out = t(lang, key).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// 下载目录那段的脚注（中文）。带 `**` 是给前端加粗用的，逐字保留。
const DI_DIR_NOTE_ZH: &'static str = "没有设置过时，壳**不指定**下载目录，由内核用它自己的默认值。\
改变目录会立刻重启内核（正在跑的任务会停）。\
请尽量选**本机磁盘**上的目录：网络盘（NFS / SMB）会让界面明显变慢。";

/// 同一段脚注的英文版。
const DI_DIR_NOTE_EN: &'static str = "When nothing is set, the shell **does not choose** a download folder: the engine uses its own default.\
Changing it restarts the engine right away (running tasks stop).\
Prefer a folder on a **local disk**: network shares (NFS / SMB) noticeably slow the UI down.";

fn zh(key: Key) -> &'static str {
    match key {
        Key::PrefDirSectionNote => DI_DIR_NOTE_ZH,
        Key::PrefVerboseLabel => "详细日志",
        Key::PrefVerboseNote => "打开后日志会详细很多（用于排查问题），排查完请关掉。改动会立刻重启内核（正在跑的任务会停）。",
        Key::PrefVerboseChangedOn => "详细日志已打开（内核已按详细档重启）",
        Key::PrefVerboseChangedOff => "详细日志已关闭（内核已按普通档重启）",
        Key::PrefVerboseUnchangedOn => "详细日志没有变化，没有重启内核",
        Key::PrefDirChangedTitle => "下载目录已改（内核已按新目录重启）",
        Key::PrefDirChangedFull => "下载目录已改为 {path}（内核已按新目录重启）",
        Key::PrefDirRestoredDefault => "已恢复默认下载目录（内核已按默认目录重启）",
        Key::PrefDirUnchangedTitle => "下载目录没有变化，没有重启内核",
        Key::SettingsParamParallel => "并行文件数（-j）",
        Key::SettingsParamConnections => "单文件连接数（-x）",
        Key::SettingsParamSplits => "分片数（-s）",
        Key::SettingsParamMinSplit => "最小分片大小（-k）",
        Key::SettingsParamLimit => "限速（MB/s）",
        Key::SettingsParamMaxTries => "重试次数",
        Key::SettingsParamRetryWait => "重试间隔（秒）",
        Key::SettingsMinSplitNote => "当前值「{value}」不在内核给出的取值集合里。保存会按这个原值下发，由内核校验。",
        Key::SettingsLimitNote => "0 = 不限速（不限制总下载速度）",
        Key::SettingsApplyNote => "改动会写盘保存（重开应用仍是这一份）。并行数与限速对已启动的引擎即时生效；其余各项在添加新任务时生效，已在传输的任务不受影响。",
        Key::SettingsUnsavedBanner => "有未保存的改动",
        Key::SettingsCleanBanner => "与内核当前参数一致",
        Key::DownloadAll => "全部下载",
        Key::DownloadSelected => "下载选中",
        Key::DownloadHelp => "把选中的文件加入下载队列（未选中时 = 全部待下载文件）",
        Key::EmptySelectionHint => "未勾选任何项，将下载全部待下载文件",
        Key::OverBudgetHint => "选中的项太多，超过一次能发出的上限：请少选一些",
        Key::EngineConnecting => "正在连接内核…",
        Key::EngineNotStarted => "引擎未启动",
        Key::EngineRunning => "运行中",
        Key::EngineUnavailablePrefix => "引擎不可用：{reason}",
        Key::EngineHandshakeTimeout => "内核无响应（等待超过 5 秒）",
        Key::EngineHandshakeTooltip => {
            "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
        }
        Key::EngineConnectingTooltip => "内核尚未握手（壳正在启动它）",
        Key::EngineNotStartedTooltip => "下载引擎尚未启动 —— 添加下载任务后内核会启动它",
        Key::EngineRunningTooltip => "下载引擎正在运行",
        Key::SettingsLanguageTitle => "界面语言",
        Key::SettingsLanguageNote => "选择后可以随时在设置里改。",
        Key::DiskSlow => {
            "目标磁盘写入约 {mbps} MB/s，比下载速度慢：下载会跟着慢下来，长时间写不动还会让任务看起来卡住。换到内置盘，或把「并行文件数」调到 1–2，通常能明显好转。"
        }
        Key::VerifyPassed => "校验通过",
        Key::VerifyMismatched => "内容不符",
        Key::VerifyMissing => "文件缺失",
        Key::VerifySizeMismatch => "大小不符",
        Key::VerifyUnverifiable => "无法校验",
        Key::VerifyUnreadable => "无法读取",
        Key::VerifyUnverifiableNote => "这些文件没有可比的校验值，不是失败",
        Key::VerifyNotVerified => "尚未校验",
        Key::VerifyAllGood => "全部通过",
        Key::VerifyFailedPrefix => "{n} 项未通过",
        Key::VerifyUnaccounted => "内核判定未全部通过（六类里没有失败项）",
        Key::VerifyCheckedPrefix => "已校验 {n} 项",
        Key::VerifyItemsSuffix => "{n} 项",
    }
}

fn en(key: Key) -> &'static str {
    match key {
        Key::PrefDirSectionNote => DI_DIR_NOTE_EN,
        Key::PrefVerboseLabel => "Detailed logging",
        Key::PrefVerboseNote => "Turns on much more verbose logs (for troubleshooting) - switch it back off afterwards. Changing it restarts the engine right away (running tasks stop).",
        Key::PrefVerboseChangedOn => "Detailed logging turned on (the engine restarted in detailed mode)",
        Key::PrefVerboseChangedOff => "Detailed logging turned off (the engine restarted in normal mode)",
        Key::PrefVerboseUnchangedOn => "Detailed logging did not change; the engine was not restarted",
        Key::PrefDirChangedTitle => "Download folder changed (the engine restarted in the new folder)",
        Key::PrefDirChangedFull => "Download folder changed to {path} (the engine restarted in the new folder)",
        Key::PrefDirRestoredDefault => "Back to the default download folder (the engine restarted in the default folder)",
        Key::PrefDirUnchangedTitle => "Download folder did not change; the engine was not restarted",
        Key::SettingsParamParallel => "Parallel files (-j)",
        Key::SettingsParamConnections => "Connections per file (-x)",
        Key::SettingsParamSplits => "Splits (-s)",
        Key::SettingsParamMinSplit => "Minimum split size (-k)",
        Key::SettingsParamLimit => "Speed limit (MB/s)",
        Key::SettingsParamMaxTries => "Retries",
        Key::SettingsParamRetryWait => "Retry wait (s)",
        Key::SettingsMinSplitNote => "The current value \"{value}\" is not one of the choices the core offers. Saving sends it as-is and the core decides.",
        Key::SettingsLimitNote => "0 = no limit (the total download speed is not capped)",
        Key::SettingsApplyNote => "Changes are saved to disk (they survive a restart). Parallel files and the speed limit apply to the running engine at once; the other values apply to newly added tasks - tasks already transferring are not affected.",
        Key::SettingsUnsavedBanner => "Unsaved changes",
        Key::SettingsCleanBanner => "Same as the core's current settings",
        Key::DownloadAll => "Download all",
        Key::DownloadSelected => "Download selected",
        Key::DownloadHelp => "Queue the selected files (nothing selected = all pending files)",
        Key::EmptySelectionHint => "Nothing selected: all pending files will be downloaded",
        Key::OverBudgetHint => "Too many items for one request: please select fewer",
        Key::EngineConnecting => "Connecting to the core…",
        Key::EngineNotStarted => "Engine not started",
        Key::EngineRunning => "Running",
        Key::EngineUnavailablePrefix => "Engine unavailable: {reason}",
        Key::EngineHandshakeTimeout => "The core is not responding (waited more than 5 seconds)",
        Key::EngineHandshakeTooltip => {
            "The core may be stuck on a system permission dialog (for example \"wants to access your Downloads folder\"). Check the screen for that dialog, click Allow, then click Retry."
        }
        Key::EngineConnectingTooltip => "The core has not completed the handshake yet (the shell is starting it)",
        Key::EngineNotStartedTooltip => {
            "The download engine has not started yet — the core starts it once you add a task"
        }
        Key::EngineRunningTooltip => "The download engine is running",
        Key::SettingsLanguageTitle => "Interface language",
        Key::SettingsLanguageNote => "You can change this later in Settings.",
        Key::DiskSlow => {
            "The target disk writes at about {mbps} MB/s, slower than the download: the download will slow down with it, and a long stall can make tasks look stuck. Switch to an internal disk, or set \"parallel files\" to 1–2 — that usually helps a lot."
        }
        Key::VerifyPassed => "Verified",
        Key::VerifyMismatched => "Content differs",
        Key::VerifyMissing => "File missing",
        Key::VerifySizeMismatch => "Size differs",
        Key::VerifyUnverifiable => "Cannot verify",
        Key::VerifyUnreadable => "Cannot read",
        Key::VerifyUnverifiableNote => {
            "These files have no checksum to compare against — this is not a failure"
        }
        Key::VerifyNotVerified => "Not verified yet",
        Key::VerifyAllGood => "All passed",
        Key::VerifyFailedPrefix => "{n} did not pass",
        Key::VerifyUnaccounted => "The core reports a failure, but no failure class accounts for it",
        Key::VerifyCheckedPrefix => "Checked {n}",
        Key::VerifyItemsSuffix => "{n}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 闭集自检：每个键都必须两种语言都有字（`match` 已经保证编译期；这里保证**非空**）。
    #[test]
    fn every_key_has_both_languages() {
        for key in [
            Key::EngineConnecting,
            Key::EngineNotStarted,
            Key::EngineRunning,
            Key::EngineUnavailablePrefix,
            Key::EngineHandshakeTimeout,
            Key::EngineHandshakeTooltip,
            Key::EngineConnectingTooltip,
            Key::EngineNotStartedTooltip,
            Key::EngineRunningTooltip,
            Key::SettingsLanguageTitle,
            Key::SettingsLanguageNote,
            Key::DiskSlow,
            Key::VerifyPassed,
            Key::VerifyMismatched,
            Key::VerifyMissing,
            Key::VerifySizeMismatch,
            Key::VerifyUnverifiable,
            Key::VerifyUnreadable,
            Key::VerifyUnverifiableNote,
            Key::VerifyNotVerified,
            Key::VerifyAllGood,
            Key::VerifyFailedPrefix,
            Key::VerifyUnaccounted,
            Key::VerifyCheckedPrefix,
            Key::VerifyItemsSuffix,
        ] {
            assert!(!t(Lang::Zh, key).trim().is_empty(), "{key:?} 中文空");
            assert!(!t(Lang::En, key).trim().is_empty(), "{key:?} 英文空");
            assert_ne!(t(Lang::Zh, key), t(Lang::En, key), "{key:?} 两种语言一样");
        }
    }

    /// 判据（规格 I-2）：**英文表里不许出现汉字**——漏译会以"半句中文"的形态活下来。
    #[test]
    fn the_english_table_has_no_cjk() {
        for key in [
            Key::EngineConnecting,
            Key::EngineUnavailablePrefix,
            Key::EngineHandshakeTimeout,
            Key::EngineHandshakeTooltip,
            Key::DiskSlow,
            Key::VerifyPassed,
            Key::VerifyUnverifiableNote,
            Key::VerifyAllGood,
            Key::VerifyFailedPrefix,
        ] {
            let text = t(Lang::En, key);
            assert!(
                !text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
                "{key:?} 的英文里有汉字：{text}"
            );
        }
    }

    #[test]
    fn the_hello_language_never_becomes_system() {
        assert_eq!(Lang::from_hello(Some("en")), Lang::En);
        assert_eq!(Lang::from_hello(Some("zh")), Lang::Zh);
        assert_eq!(Lang::from_hello(Some("system")), Lang::Zh, "壳没有 system 这一档");
        assert_eq!(Lang::from_hello(None), Lang::Zh, "没握上手也要画得出来");
        assert_eq!(Lang::from_hello(Some("fr")), Lang::Zh, "不认识就按默认，不许炸");
    }

    #[test]
    fn tf_fills_named_slots() {
        let text = tf(Lang::Zh, Key::DiskSlow, &[("mbps", "6")]);
        assert!(text.contains("约 6 MB/s"), "{text}");
        assert!(!text.contains("{mbps}"), "占位符没填：{text}");
    }
}
