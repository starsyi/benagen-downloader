//! 下载目录那一段的载荷与话术（任务 8 新增：`preferences_get` / `preferences_check` /
//! `preferences_set` 三个命令共用）。
//!
//! 上游：`macos/Sources/BenagenDownloader/Settings/SettingsView.swift` 的下载目录段
//! （视图层）+ `Presentation/DownloadDirectory.swift`（判据与文案）。**判据与文案一条都
//! 不在本文件重写**：显示值、那一整段脚注、三条后果的确认文案、四种回执全都取自
//! `presentation::app_preferences`（`DownloadDirectory` / `DownloadDirChange`）。
//!
//! 本文件只做两件事：
//!   1. 把那些**已经算好的值**装成给前端的信封（`to_wire` 之外唯一的形状是 `json!` 的那几格）；
//!   2. 提供**壳自己写的那三句话**：偏好写盘失败、内核重启没落地、（以及
//!      `DownloadDirectory::check` 的几句本来就在 `presentation` 里）。
//!
//! ## 🔴 为什么"壳自己写的三句话"必须住在这里
//!
//! 规格 §3.2 的同一形态：命令层**一个字都不许自己写**（R-24）。而这三种情形**没有内核
//! 原文可登** —— 内核要么还不知道这件事（写盘失败时它没被碰过），要么还没起来
//! （重启没落地）。落地方式与 `api::NO_KERNEL` / `api::verify::NO_VERIFY_SUMMARY`
//! 逐字同款：一个**有测试**的常量/函数，命令层只写 `api::preferences::…`。

use std::time::Duration;

use serde_json::{json, Value};

use crate::api::envelope;
use crate::presentation::app_preferences::{
    AppPreferences, DiagnosticsToggle, DownloadDirChange, DownloadDirectory, VerboseLoggingChange,
};
use crate::storage::preferences::Preferences;

/// 设置窗口里"下载目录"那一段的**只读部分**。
///
/// ⚠️ 三格各有各的用处，**一格都不能少**：
///   * `dir` —— 偏好里那一份**原文**（空串 = 未配置）。前端拿它跟用户选的目录比，
///     "没改动"那条路要靠它（比 `display` 更早，因为 `display` 对空串是一句人话）；
///   * `display` —— 那一行**显示**什么（未配置时是"默认（…，由内核决定）"，
///     **不是**一个空行 —— 空行在界面上与"这一行坏了"分不开）；
///   * `section_note` —— 那一段的脚注（"没有设置过时壳不指定下载目录…网络盘会让界面变慢"）。
pub fn current(preferences: &Preferences) -> Value {
    envelope::ok(json!({
        "dir": preferences.download_dir,
        "display": DownloadDirectory::display(&preferences.download_dir),
        "section_note": DownloadDirectory::SECTION_NOTE,
        // ---- 「详细日志」那一个勾选框（任务 3，规格 §2.4）--------------------
        // ⚠️ 标签与说明**跟着载荷下来**（不是 `index.html` 里的结构文案）：
        //    那个控件是运行时画的（JS 要按 `verbose_logging` 设 `checked`），
        //    两句话跟它一起走就不必在 HTML 与 Rust 里各存一份。
        //    ⇒ 前端一个字都不用自己造（`check_frontend_copy.sh` 的判据因此不用动）。
        "verbose_logging": preferences.verbose_logging,
        "verbose_label": DiagnosticsToggle::LABEL,
        "verbose_note": DiagnosticsToggle::NOTE,
    }))
}

/// 一次「准备改目录」的结论：**能不能用** + **改了会怎样**。
///
/// ⚠️ 它是**两个问题、两句话**，缺一不可，而且**顺序是承重的**（`DownloadDirectory::check`
///    的头注记着）：目录不可用要**在确认对话框之前**就报出来 —— 等用户看完"正在跑的任务
///    会停"、点了确认、重启完才发现"这个文件夹不可写"，他已经为它停掉了一次正在跑的下载。
///
/// ⚠️ `check` 为 `null` = **可以用**（`DownloadDirectory::check` 的 `None`），
///    不是"没检查过"：前端只有两条路 —— `check` 非空 ⇒ 报错并中止；`null` ⇒ 弹确认。
pub fn plan(check: Option<String>, confirmation: String) -> Value {
    envelope::ok(json!({
        "check": check,
        "confirmation": confirmation,
    }))
}

/// 一次改目录的结果（[`DownloadDirChange`] 的四格 → 四个**已经算好的呈现值**）。
///
/// ⚠️ 四格全部调**既有的方法**（`headline` / `path_detail` / `notice_text` / `is_failure`），
///    本函数不自己成句：
///   * `headline` 与 `path_detail` 是**分开的两格** —— 那是一次真实布局事故的修法
///     （完整路径铺在一行里会把常驻回执那一行撑高，锚在底边的两处一起被顶出窗口）；
///   * `notice_text` 是那条回执**完整的一句话**（防回归的锚）；
///   * `is_failure` 决定前端挑哪个图标与颜色。
pub fn change(change: &DownloadDirChange) -> Value {
    receipt(
        change.headline(),
        change.path_detail(),
        change.notice_text(),
        change.is_failure(),
    )
}

/// 一次「改详细日志」的结果（[`VerboseLoggingChange`] 的四格 → **同一组键**）。
///
/// 🔴 **形状与 [`change`] 逐格相同**（简报要的"复用 `preferences_set` 那一套回执形状"）
///    ⇒ 前端只有**一条**渲染路径（`settings.js:paintChangeReceipt` 读 `headline` /
///    `path_detail` / `is_failure` 那三格），多一个回执不需要多一屏。
///    ⚠️ 但**那三句话取自另一个类型**：`DownloadDirChange` 的三句把主词写死成
///       "下载目录"，拿它报一次勾选框的改动会让用户读到一句假话 ——
///       理由逐条写在 `VerboseLoggingChange` 的类型文档里。
pub fn verbose_change(change: &VerboseLoggingChange) -> Value {
    receipt(
        change.headline(),
        change.path_detail(),
        change.notice_text(),
        change.is_failure(),
    )
}

/// 两个回执共用的**唯一**装帧处（四格：标题 / 路径行 / 完整那句 / 是不是失败）。
///
/// ⚠️ 抽出来只有一个理由：**同一组键只许有一份实现** —— 两个回执各写一遍 `json!`
///    的结局是"某一格哪天在一边改了、另一边没改"，而前端只有一条渲染路径，
///    读到的会是一个 `undefined`（界面安静地少一行，不会有任何东西变红）。
fn receipt(
    headline: String,
    path_detail: Option<String>,
    notice_text: String,
    is_failure: bool,
) -> Value {
    envelope::ok(json!({
        "headline": headline,
        "path_detail": path_detail,
        "notice_text": notice_text,
        "is_failure": is_failure,
    }))
}

/// **偏好（磁盘上那一份）→ 内核的 argv**。
///
/// ⚠️ 壳里有两个"偏好"，与两个"历史"同一种情形（见 `api::history` 的模块头）：
///
/// | 类型 | 哪来的 | 它是谁的 |
/// |---|---|---|
/// | [`Preferences`]（`storage::preferences`） | 任务 3 | **磁盘上那一份**（`version` / E-1 / E-2） |
/// | `AppPreferences`（`presentation::app_preferences`） | 任务 5 | **判据那一份**（归一化 / `is_configured` / argv） |
///
/// 字段只有一格（`download_dir`）、语义完全一样（空串 = 未配置），但**它们是两个类型**
/// —— 而"把偏好接成 argv"这件事要的正是判据那一份。⇒ 桥只有这一座。
///
/// ⚠️ 实现**直接转调** [`DownloadDirectory::core_arguments_for`]（它又转调
///    `client::core_arguments`），**不另拼一份 argv**：拼两份的结局是 flag 名或顺序漂移，
///    而那种漂移不会报错 —— 内核会照单全收，只是行为与壳以为的不一样。
///
/// 🔴 **这条路上有一条承重的纪律（E-5）**：未配置时 argv 里**一个参数都不许有**
///    （不许自己编一个"与内核默认值一样"的路径）。判据在
///    `DownloadDirectory::argument`（`None` ⇒ `--download-dir` 根本不出现），
///    下面那条 `an_unconfigured_preference_never_puts_a_path_in_the_argv` 从**存储层那一份**
///    出发把它再钉一遍（那条缝以前只在呈现层被钉过）。
pub fn core_arguments(preferences: &Preferences) -> Vec<String> {
    // ⚠️ **两格都要接上**：`AppPreferences::new` 只造"下载目录"那一格（详细日志起手是
    //    关着的）—— 少接下面那一句，客户打开开关之后内核**照样按 normal 起**
    //    （壳自己那半边的日志倒是会变详细，于是两个文件对不上，而没有任何东西会变红）。
    let preferences = AppPreferences::new(&preferences.download_dir)
        .setting_verbose_logging(preferences.verbose_logging);
    DownloadDirectory::core_arguments_for(&preferences, None)
}

/// 「偏好**写盘失败**」那句话。
///
/// 🔴 **壳自己写的**（对齐 macOS `AppModel.performDownloadDirChange` 里那一句）：
///    那一刻内核**还不知道**这件事（我们还没动它，也不会动它）—— 没有原文可登。
///    而"一次以为存上了的静默失败"是这个项目明禁的形态：用户要等到**下一次启动**
///    才会发现设置又变回去了，那时已经无从归因（E-2 的下半条）。
///
/// ⚠️ 文案与 macOS 逐字同源（`下载目录没有改成（偏好写入失败：…）`），只把系统的
///    错误原文接在括号里：它是**可复制的证据**（磁盘满 / 权限 / 只读介质各说各的）。
pub fn save_failed(cause: &str) -> String {
    format!("下载目录没有改成（偏好写入失败：{cause}）")
}

/// 「详细日志」那一格**写盘失败**那句话。
///
/// 🔴 与 [`save_failed`] **同一个理由、同一个位置**（壳自己写的：那一刻内核还不知道
///    这件事），但**不是同一句话** —— 那一句的主词是"下载目录"，而这一下用户改的是
///    一个开关、一个目录都没动。两件事共用一句 = 用户读到一句假话。
pub fn verbose_save_failed(cause: &str) -> String {
    format!("详细日志开关没有改成（偏好写入失败：{cause}）")
}

/// 改下载目录之后"内核重启**有没有落地**"的等待上界。
///
/// ⚠️ **这个数是判据，不是拍脑袋**：重启的每一段都有自己的上界 ——
///    起子进程是瞬时的（`CreateProcess` 本身不阻塞），握手有 5 秒的硬上界
///    （`bootstrap::DEFAULT_HANDSHAKE_TIMEOUT`，超时**一定**回一句
///    `HANDSHAKE_TIMEOUT_MESSAGE`）。所以正常情形是几十到几百毫秒；
///    真到了它超过 20 秒的机器上，**继续等下去也不会再有新结论**，不如把
///    "这一次没改成"如实说出来（看得见的失败 > 一直转圈）。
///    与 macOS 的 `waitForTheRestartInFlightToFinish(timeout: 20)` 取的是同一个数。
///
/// ⚠️ 等待期间**不阻塞前端**：`state()` 的轮询跑在别的命令线程上，引擎徽标会一路
///    显示"正在连接内核…"。这里等的是**这一次调用**的结论，不是界面的反应。
pub const RESTART_DEADLINE: Duration = Duration::from_secs(20);

/// 「内核重启**没有在期限内落地**」那句话。
///
/// 🔴 壳自己写的：那一刻内核还没起来，**没有原文可登**。三个半句缺一不可：
///   * 事实（这次重启没有按期落地）；
///   * **已经发生的那一半**（偏好已经写下去了）—— 不说的话用户会以为整个操作都失败了，
///     于是再改一遍（而那会再写一次盘、再重启一次）；
///   * **下一步**（下次启动会按它生效，或者点「重试」）。
///
/// ⚠️ 与 `DownloadDirChange::Failed` 的关系：这条**不是** `Failed` 那一格自带的文案
///    （`Failed` 装的是内核/系统的原文，壳不加工）—— 它是**壳自己的**结论，
///    由命令层装进 `Failed { message }`（那条路是 `DownloadDirChange` 自己的设计：
///    `Failed` 里那句"可能是内核原文，也可能是写盘失败的原文，两种都照登"）。
///
/// ⚠️⚠️ **文案逐字来自 macOS**（`AppModel.swift:937` 的
///    `"内核还在重启中，这一次没有改成。" + "下载目录已经记下来了，下次启动会按它生效。"`），
///    **一个字都没改** —— 这里正确的那半句就是"内核还在重启中"：我们到点（20 s）时
///    引擎那一格**还停在 `Connecting`**，也就是那个内核确实还在起（不是"没落地"）。
///    初稿写的是我自己的句子（「内核重启没有在期限内落地…」并追加了「重试」那半句），
///    控制者按"同一件事不许两处说不同的话"订正掉了 —— 那句补救也已经在 macOS 那句里
///    （"下次启动会按它生效"），不需要我们再加一句。
pub fn restart_timed_out() -> &'static str {
    "内核还在重启中，这一次没有改成。下载目录已经记下来了，下次启动会按它生效。"
}

/// 「详细日志」那一格的**同一件事**：内核重启没有在期限内落地。
///
/// 🔴 与 [`restart_timed_out`] **逐字同构**（前一句与第三句一个字都不改 —— 那两句说的是
///    内核与"下次启动"，与改的是哪一格无关），**只有中间那半句换了主词**：
///    用户改的是一个开关、一个目录都没动，说成"下载目录已经记下来了"就是一句假话。
///
/// ⚠️ 这一句与上面那句由**调用方**挑（`commands::restart_kernel` 收一个"超时该说什么"
///    的参数）：那一刻**只有调用方知道**这一次改的是哪一格，
///    而 `wait_for_the_kernel` 手上只有内核的状态。合成一句的结局就是其中一半场合说错话。
pub fn verbose_restart_timed_out() -> &'static str {
    "内核还在重启中，这一次没有改成。详细日志开关已经记下来了，下次启动会按它生效。"
}

/// 原生「选择文件夹」对话框里那句话（**它自己那一格的标题**）。
///
/// 为什么它住在这里而不是在壳里：那是**一句面向用户的话**，与本节其余几句同一条
/// 纪律（R-24：命令层里不许出现面向用户的字符串）。
/// ⚠️ 它比别的几句更漏网：它出现在**系统对话框**里，`check_frontend_copy.sh` 那种
///    扫 DOM 的守卫**一眼都看不到它** —— 放进壳里就是一句没有任何判据管着的文案。
///
/// ⚠️ 与 macOS 的对应关系（`SettingsView.swift:230-247`）**不是逐字**：
///    `NSOpenPanel` 有 `message`（说明）与 `prompt`（按钮名）**两格**，
///    而 `SHBrowseForFolderW` 只有 `lpszTitle` **一格**（显示在目录树上方）。
///    ⇒ 取 macOS 那句 `message`（"交付文件会下载到这个文件夹"）—— 它说的是
///      **这个文件夹会拿来干什么**，比一个光秃秃的"选择文件夹"有用；
///      按钮名那一格由系统给（中文系统上是"确定"），壳管不着。
///    这是平台能力的差别，不是漏做：记在这里免得后来者去追一句不存在的对位。
pub fn picker_title() -> &'static str {
    "交付文件会下载到这个文件夹"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(dir: &str) -> Preferences {
        Preferences::new(dir)
    }

    /// 三格都在，而且各自干各自的活（未配置那一档显示的是**一句人话**，不是空行）。
    #[test]
    fn the_current_state_carries_the_dir_the_display_and_the_note() {
        let v = current(&prefs("/Volumes/Data/交付"));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["dir"], json!("/Volumes/Data/交付"));
        assert_eq!(v["data"]["display"], json!("/Volumes/Data/交付"));
        assert!(
            v["data"]["section_note"]
                .as_str()
                .unwrap_or_default()
                .contains("默认"),
            "脚注必须解释「没有设置过」指的是什么：{v}"
        );

        // 未配置：`dir` 是空串（判"没改动"要用它），而 `display` 是一句人话。
        let v = current(&prefs(""));
        assert_eq!(v["data"]["dir"], json!(""));
        assert_eq!(
            v["data"]["display"],
            json!("默认（~/Downloads/Benagen，由内核决定）"),
            "未配置时那一行不许是空白：{v}"
        );
    }

    /// 🔴 **两格都接上了**：开关那一格的**值**与它的**标签/说明**都在载荷里。
    ///
    /// 判别力（三条各挡一种写法）：
    ///   · 漏发 `verbose_logging` ⇒ 前端读到的永远是 `undefined` ⇒ 勾选框**永远不勾**，
    ///     用户会以为"打开了但没记住"（而没有任何东西会变红）；
    ///   · 漏发标签/说明 ⇒ 前端要么画一个没有字的框，要么自己造一句中文（§3.2 明禁）；
    ///   · 把值写成恒 `false` ⇒ 第二条断言红（开着的那一份必须发 `true`）。
    #[test]
    fn the_current_state_carries_the_diagnostics_toggle() {
        let off = current(&prefs("/Volumes/Data/交付"));
        assert_eq!(off["data"]["verbose_logging"], json!(false));
        assert_eq!(
            off["data"]["verbose_label"],
            json!(DiagnosticsToggle::LABEL),
            "标签必须跟着载荷下来（前端一个字都不许自己造）"
        );
        assert_eq!(off["data"]["verbose_note"], json!(DiagnosticsToggle::NOTE));

        let on = current(&Preferences::new("/Volumes/Data/交付").setting_verbose_logging(true));
        assert_eq!(on["data"]["verbose_logging"], json!(true), "开着的那一份要发 true");
        // 未配置那一份同样带着这三格（开关与"配没配过目录"是两件事）。
        let empty = current(&Preferences::empty());
        assert_eq!(empty["data"]["verbose_logging"], json!(false));
        assert!(empty["data"]["verbose_label"].is_string());
    }

    /// 两个回执**发的是同一组键**（前端只有一条渲染路径）。
    ///
    /// 判别力：把 [`verbose_change`] 里的某格漏掉（或换成别的键名）⇒ 这一条红 ——
    /// 而真机上的表现是设置窗口里那条回执**安静地少一行**（`undefined`）。
    #[test]
    fn both_receipts_carry_the_same_four_keys() {
        let dir = change(&DownloadDirChange::Changed {
            dir: "/data/交付".to_string(),
        });
        let verbose = verbose_change(&VerboseLoggingChange::Changed { on: true });
        let keys = |v: &Value| {
            let mut k: Vec<String> = v["data"]
                .as_object()
                .expect("回执的 data 一定是个对象")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        };
        assert_eq!(keys(&dir), keys(&verbose), "两个回执的键必须逐字相同");
        assert_eq!(keys(&dir).len(), 4);

        // 而且**那句话说的是对的**（不是"下载目录已改"）。
        assert!(
            verbose["data"]["headline"]
                .as_str()
                .unwrap_or_default()
                .contains("详细日志"),
            "{verbose}"
        );
        assert!(
            !verbose["data"]["headline"]
                .as_str()
                .unwrap_or_default()
                .contains("下载目录"),
            "勾选框那一格一个目录都没改：{verbose}"
        );
        assert_eq!(verbose["data"]["path_detail"], Value::Null, "这一格没有路径行");
        assert_eq!(verbose["data"]["is_failure"], json!(false));
    }

    /// 🔴 **写盘失败那句话点名了根因，而且说的是"详细日志"**。
    ///
    /// 判别力：把它换成 `save_failed`（那一句的主词是"下载目录"），第二条立刻红 ——
    /// 而真机上的表现是用户改了一个开关、却看到一句"下载目录没有改成"。
    #[test]
    fn the_verbose_save_failure_names_the_switch_not_the_directory() {
        let message = verbose_save_failed("磁盘空间不足");
        assert!(message.contains("详细日志"), "主词必须是这一下改的那一格：{message}");
        assert!(message.contains("偏好写入失败"), "要说清是哪一步失败了：{message}");
        assert!(message.contains("磁盘空间不足"), "系统原文必须照登：{message}");
        assert!(!message.contains("下载目录"), "一个目录都没改，不许提目录：{message}");
        assert_ne!(message, save_failed("磁盘空间不足"));
    }

    /// 确认文案**含三条后果**，且 `check` 的两档形状分明（`null` = 可以用）。
    #[test]
    fn the_plan_carries_both_the_check_and_the_confirmation() {
        let confirmation = DownloadDirectory::confirmation("", "/data/交付");
        let blocked = plan(Some("这个位置不存在：/nope".to_string()), confirmation.clone());
        assert_eq!(blocked["data"]["check"], json!("这个位置不存在：/nope"));
        assert!(
            blocked["data"]["confirmation"]
                .as_str()
                .unwrap_or_default()
                .contains("正在跑的任务会停"),
            "确认文案必须说清后果：{blocked}"
        );

        let ok = plan(None, confirmation);
        assert!(
            ok["data"]["check"].is_null(),
            "`null` = 可以用（不是「没检查过」）：{ok}"
        );
        assert!(ok["data"]["confirmation"].is_string());
    }

    /// 四种回执各自发什么（**逐字**：布局事故的修复只许拆结构、不许改措辞）。
    #[test]
    fn every_outcome_is_wired_as_the_presentation_values() {
        let changed = DownloadDirChange::Changed {
            dir: "/data/交付".to_string(),
        };
        let v = change(&changed);
        assert_eq!(v["data"]["headline"], json!("下载目录已改（内核已按新目录重启）"));
        assert_eq!(v["data"]["path_detail"], json!("/data/交付"));
        assert_eq!(
            v["data"]["notice_text"],
            json!("下载目录已改为 /data/交付（内核已按新目录重启）")
        );
        assert_eq!(v["data"]["is_failure"], json!(false));
        // ⚠️ 标题里**不许有路径**（那条布局事故的根因）——哪怕路径很长也一样。
        assert!(
            !v["data"]["headline"]
                .as_str()
                .unwrap_or_default()
                .contains("交付"),
            "标题里混进了路径：{v}"
        );

        // 恢复默认：没有路径行（`null`，不是一个空串）。
        let v = change(&DownloadDirChange::Changed { dir: String::new() });
        assert!(v["data"]["path_detail"].is_null(), "{v}");
        assert_eq!(v["data"]["headline"], json!("已恢复默认下载目录（内核已按默认目录重启）"));

        // 没改动：内核没重启（那句"正在跑的任务会停"说的是**真的改了**的时候）。
        let v = change(&DownloadDirChange::Unchanged {
            dir: "/x".to_string(),
        });
        assert_eq!(v["data"]["headline"], json!("下载目录没有变化，没有重启内核"));
        assert_eq!(v["data"]["is_failure"], json!(false));

        // 失败：**原文照登**（内核/系统的原话），而且 `is_failure` 要翻。
        let v = change(&DownloadDirChange::Failed {
            message: "内核重启失败：内核无响应（等待超过 5 秒）".to_string(),
        });
        assert_eq!(
            v["data"]["headline"],
            json!("内核重启失败：内核无响应（等待超过 5 秒）")
        );
        assert_eq!(v["data"]["notice_text"], v["data"]["headline"]);
        assert_eq!(v["data"]["is_failure"], json!(true));
    }

    /// 🔴 **未配置 ⇒ argv 里一个参数都不许有**（E-5：不许自己编一个"与内核默认值一样"
    /// 的路径）。判别力：把 `core_arguments` 改成"未配置也拼一对参数"，这一条立刻红 ——
    /// 而真机上的表现是"用户改了内核的默认目录之后，壳还是把他带回老地方"（一次没有任何
    /// 提示的分叉）。
    #[test]
    fn an_unconfigured_preference_never_puts_a_path_in_the_argv() {
        for dir in ["", "   "] {
            let argv = core_arguments(&Preferences::new(dir));
            assert!(argv.is_empty(), "未配置 ⇒ 一个参数都没有，实际 {argv:?}");
            assert!(
                !argv.iter().any(|arg| arg == "--download-dir"),
                "那个 flag 根本不该出现：{argv:?}"
            );
        }
        // 反方向：配过就是逐字那一对（否则上面那条可以靠"永远返回空"变绿）。
        assert_eq!(
            core_arguments(&Preferences::new("/data/交付")),
            vec!["--download-dir".to_string(), "/data/交付".to_string()],
            "配过 ⇒ 逐字是那一对（顺序也不能反）"
        );
    }

    /// 🔴 **存储层那一格真的接到了 argv 上**（"偏好在盘上"到"内核的参数"只有这一座桥）。
    ///
    /// 判别力：把 `core_arguments` 里那句 `.setting_verbose_logging(…)` 删掉 ⇒
    /// `AppPreferences::new` 不再带上那一格 ⇒ argv 里没有 `--log-level` ⇒ 这一条红 ——
    /// 而真机上的表现是客户打开了详细日志、**内核那份日志一行都没变详细**，
    /// 而壳自己那一半倒是变详细了（两个文件对不上，**没有任何东西会变红**）。
    #[test]
    fn the_verbose_flag_reaches_the_argv_from_the_stored_preferences() {
        assert_eq!(
            core_arguments(&Preferences::new("/data/交付").setting_verbose_logging(true)),
            vec![
                "--download-dir".to_string(),
                "/data/交付".to_string(),
                "--log-level".to_string(),
                "verbose".to_string(),
            ],
            "开着 ⇒ 那一对必须拼进内核的 argv（内核那一份的档位只在 argv 上）"
        );
        assert!(
            !core_arguments(&Preferences::new("/data/交付"))
                .iter()
                .any(|arg| arg == "--log-level"),
            "关着 ⇒ 那一对一个字都不许出现（缺省就是 normal）"
        );
    }

    /// 🔴 **写盘失败那句话点名了根因（偏好写入失败）**，而且带上了系统的原文。
    ///
    /// 判别力：把它改成一句笼统的"保存失败"，下面那两条立刻红 ——
    /// 而真机上的表现是"用户以为内核没重启成功，于是又改了一遍"。
    #[test]
    fn the_save_failure_names_the_preference_write_and_keeps_the_cause() {
        let message = save_failed("磁盘空间不足");
        assert!(
            message.contains("偏好写入失败"),
            "必须说清是哪一步失败了（偏好写盘，不是内核）：{message}"
        );
        assert!(message.contains("磁盘空间不足"), "系统原文必须照登：{message}");
        assert!(!message.is_empty());
    }

    /// 🔴 **重启超时那句话是 macOS 的原文**（事实 / 已经发生的那一半）。
    ///
    /// 判别力：删掉"已经记下来了"那半句，第二条断言立刻红 —— 而真机上的表现是
    /// 用户以为**整个操作**都失败了，于是**再改一遍**（多写一次盘、多重启一次内核）。
    /// ⚠️ 期望值是**字面量**（不是再调一遍 `restart_timed_out()`）：那样写等于
    ///    `f() == f()`，改一个字都不会红。
    #[test]
    fn the_restart_timeout_is_the_macos_sentence_verbatim() {
        assert_eq!(
            restart_timed_out(),
            "内核还在重启中，这一次没有改成。下载目录已经记下来了，下次启动会按它生效。",
            "这句话逐字来自 macOS 的 AppModel.swift:937（同一件事不许两处说不同的话）"
        );
        assert!(
            restart_timed_out().contains("已经记下来了"),
            "偏好那一半**已经成立**必须说出来（否则用户会再改一遍）"
        );
    }

    /// 🔴 **超时那句话有两句、而且只有中间那半句不同**（改的是哪一格只有调用方知道）。
    ///
    /// 判别力：让 `verbose_logging_set` 去用 `restart_timed_out()`（那一句说"下载目录
    /// 已经记下来了"），第一条断言立刻红 —— 而真机上的表现是用户**改了一个勾选框**、
    /// 却读到一句关于下载目录的话（两件事他都没做错，是界面在说假话）。
    #[test]
    fn the_two_restart_timeouts_differ_only_in_what_was_recorded() {
        let dir = restart_timed_out();
        let verbose = verbose_restart_timed_out();
        assert!(verbose.contains("详细日志"), "要说清记下来的是哪一格：{verbose}");
        assert!(!verbose.contains("下载目录"), "这一个目录都没改：{verbose}");
        // ⚠️ 另两句**逐字相同**（同一件事不许两处说不同的话）：内核那一半、以及
        //    "下次启动会按它生效"那条补救 —— 它们与改的是哪一格无关。
        for shared in ["内核还在重启中，这一次没有改成。", "下次启动会按它生效。"] {
            assert!(dir.contains(shared) && verbose.contains(shared), "共有那半句必须逐字相同：{shared}");
        }
        assert!(dir.contains("下载目录已经记下来了"));
        assert!(verbose.contains("详细日志开关已经记下来了"));
    }

    /// ⚠️ 等待上界是个**有限的数**，而且不是"够用就行"的量级乱写。
    ///
    /// 判别力：把它改成 0（或者一个巨大的数），这一条会红 —— 前者让每一次改目录都
    /// 报"没落地"，后者让用户在真正卡死时永远等不到结论。
    #[test]
    fn the_restart_deadline_is_bounded_and_generous() {
        assert!(RESTART_DEADLINE >= Duration::from_secs(5),
                "它至少要容得下握手的 5 秒上界：{RESTART_DEADLINE:?}");
        assert!(RESTART_DEADLINE <= Duration::from_secs(60),
                "它不许长到让用户以为界面挂了：{RESTART_DEADLINE:?}");
    }
}
