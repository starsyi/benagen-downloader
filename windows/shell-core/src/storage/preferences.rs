//! 壳的偏好文件（**对齐 macOS 的 `AppPreferencesStore.swift` + `Presentation/AppPreferences.swift`**）。
//!
//! 今天只有一项：**下载目录**。它落 `preferences.json`，与历史**同目录、另一个文件**
//! （两者的生命周期与格式由各自负责 —— 内核改 `settings.json` 的形状不该让壳的偏好跟着陪葬）。
//!
//! ## ⚠️ **"未配置"是有效状态，不是错误**
//!
//! 空串 ⇒ 壳**不传** `--download-dir`（E-5），由内核用自己的默认值。不要为了"看起来明确"
//! 而显式传一个"和内核默认一样"的值 —— 那会在内核默认值变化时静默分叉。
//!
//! ## ⚠️ 与 `history.rs` 同一条底线（E-1）
//!
//! 读不出来 / 解析失败 / `version` 不认识 ⇒ **当未配置**并继续启动，绝不让应用起不来。
//! 所以 [`Preferences::load`] **没有 `Result`**（"失败"这条路径根本不存在）——
//! 而 [`Preferences::save`] **返回 `io::Result`**（E-2 的下半条：一次"以为存上了"的
//! 静默失败，用户要等到**下一次启动**才会发现"设置又变回去了"，而那时已经无从归因）。
//!
//! 本文件只做**纯计算 + 收发本文件**（解析 / 序列化 / 判据 / 改写 + 走
//! [`JsonFile`] 的那两步 IO），不碰别的路径、不读时钟、不读环境。
//! 落盘位置的规则在 [`crate::storage::dir`]。

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};

use crate::storage::json_file::JsonFile;

/// 偏好文件的名字（与历史**同目录、另一个文件**）。
/// 对齐 macOS `AppPreferencesStore.fileName`。
pub const FILE_NAME: &str = "preferences.json";

/// 壳的偏好（对齐 macOS `AppPreferences`）。
///
/// ⚠️ **字段名 `download_dir` 逐字对齐磁盘形状**（`AppPreferences.serialized()` 那两行），
///    改它就是改契约。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Preferences {
    /// 用户选的下载目录。**空串 = 未配置**（E-5：不传 `--download-dir`）。
    ///
    /// ⚠️ 字段是 `pub` 的（本 crate 对界面值/配置值一律如此，见 `DirLoadFailure` 的记账），
    ///    所以**手工用结构体字面量构造出来的那一份不会过 [`Preferences::new`] 的规范化**
    ///    —— 要规范化就用构造函数或 [`Preferences::setting_download_dir`]。
    pub download_dir: String,

    /// **详细诊断日志**（规格 §2.4）。开着的时候壳给内核多拼一对
    /// `--log-level verbose`，壳自己那一份也用 [`crate::diagnostics::Level::Verbose`]。
    ///
    /// 🔴 **缺字段 = `false`**（老客户机器上那份 `preferences.json` 里没有这一格）——
    ///    这条不是靠"构造函数给它 `false`"，而是靠**反序列化那一侧的 `#[serde(default)]`**
    ///    （见下面 `Deserialize` 的实现里那段注释）：少了它，老文件会**整份解析失败**，
    ///    而那时候连 `download_dir` 一起丢，**没有任何东西会变红**。
    pub verbose_logging: bool,
}

impl Preferences {
    /// 给未来的自己的版本号：**不认识的版本 ⇒ 当未配置**（对齐
    /// `AppPreferences.version = 1`，E-1 的落点）。
    pub const VERSION: i64 = 1;

    /// 没有配置过任何东西（= 一切走默认）。对齐 `AppPreferences.empty`。
    ///
    /// ⚠️ 详细日志那一格同样是**关着**的：它是"没配过"这个状态的一部分，
    ///    而不是一个"默认打开"的东西（诊断日志开着的时候单文件能长到 4 MiB，
    ///    那是客户报障时才需要的一档）。
    pub fn empty() -> Preferences {
        Preferences { download_dir: String::new(), verbose_logging: false }
    }

    /// 造一份偏好（**过一遍规范化**）。对齐 `AppPreferences.init(downloadDir:)`。
    ///
    /// ⚠️ **只造"下载目录"那一格**：详细日志起手是关着的，要开用
    ///    [`Self::setting_verbose_logging`]。
    pub fn new(download_dir: &str) -> Preferences {
        Preferences { download_dir: normalized(download_dir), verbose_logging: false }
    }

    /// 配过下载目录没有。**它是"要不要传 `--download-dir`"的唯一判据**
    /// （对齐 `AppPreferences.isConfigured`）。
    pub fn is_configured(&self) -> bool {
        !self.download_dir.is_empty()
    }

    /// 改下载目录（返回新值）。传空串 = 回到未配置（设置窗口那颗「恢复默认」）。
    /// 对齐 `AppPreferences.settingDownloadDir(_:)`。
    ///
    /// ⚠️ **详细日志那一格要原样带过去**（不是走 [`Self::new`]）：`new` 把它置回 `false`，
    ///    而"改下载目录"与"关掉详细日志"是**两件事** —— 合成一件的表现是客户在排查
    ///    一次故障的中途换了个目录，诊断日志**静默地退回普通档**，而
    ///    `diag-shell.log` 还照样在长（只是不再记那每一次往返了）。
    pub fn setting_download_dir(&self, path: &str) -> Preferences {
        Preferences {
            download_dir: normalized(path),
            verbose_logging: self.verbose_logging,
        }
    }

    /// 开关详细诊断日志（返回新值）。**这是那一格的唯一入口**。
    ///
    /// ⚠️ 对称的那一条：`download_dir` **原样带过去**（同上，两件事不许互相覆盖）。
    pub fn setting_verbose_logging(&self, on: bool) -> Preferences {
        Preferences {
            download_dir: self.download_dir.clone(),
            verbose_logging: on,
        }
    }

    /// 读。文件不存在、读不动、内容坏了、`version` 不认识 ⇒ **当未配置**
    /// （对齐 `AppPreferencesStore.load()`，E-1：绝不抛、绝不崩，
    /// 更不许把应用拦在启动之外）。
    ///
    /// ⚠️ `path` 是**参数**（默认位置由调用方给 `storage::dir().join(FILE_NAME)`）：
    ///    真机上那个目录里放着人类伙伴**真实在用**的配置，测试必须指到临时目录去。
    pub fn load(path: &Path) -> Preferences {
        JsonFile::read::<Preferences>(path).unwrap_or_else(Preferences::empty)
    }

    /// 原子地把整份偏好写下去（E-2，实现见 [`JsonFile::write`]）。
    ///
    /// ⚠️ 失败**返回 `Err`** 而不是吞掉：调用方（设置窗口那一步）必须有机会**中止**并说一句。
    ///    对齐 `AppPreferencesStore.save(_:)`。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        JsonFile::write(path, self)
    }
}

/// ⚠️ **有意只去掉首尾空白，不做别的路径规范化**（对齐 macOS 的 `AppPreferences.normalized`，
///    即 E-8）：这条路径最终要**原样进内核的 argv**（`--download-dir`，
///    `core/src/main.rs:1678`）。软链接、`..`、结尾的 `/` 都是**文件系统与内核的语义**，
///    壳在这儿替它解释一遍，只会让"壳以为的路径"和"内核实际用的路径"悄悄分叉 ——
///    而分歧的代价是**文件下到别的地方去**。（面板给的是绝对路径，本就不需要展开 `~`。）
///
/// ⚠️ 与 macOS 的一处微小差异：那边 `trimmingCharacters(in: .whitespacesAndNewlines)`
///    只去空白与换行，Rust 的 `str::trim` 去的是**全部 Unicode 空白**（多了少数几个码位）。
///    可观测差别只可能出现在"路径里带 U+00A0 之类的不可见空白"这个病态输入上 ——
///    两边都会把那个字符干掉或留下，谁都不比谁更"对"。记在这里是为了偏离清单完整（W-6）。
fn normalized(raw: &str) -> String {
    raw.trim().to_string()
}

// ---------------------------------------------------------------------------
// 磁盘形状（字段名逐字，见模块头）
// ---------------------------------------------------------------------------

/// 落盘的形状：`{"version": 1, "download_dir": "/abs/path", "verbose_logging": false}`。
impl Serialize for Preferences {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Raw<'a> {
            version: i64,
            download_dir: &'a str,
            // ⚠️ **写出去时这一格永远在场**（读的时候可以缺 —— 见下面 `Deserialize`）：
            //    写一份"能省就省"的形状会让"老文件"与"这份偏好根本没配过"在盘上
            //    长得一样，而它们要区分开（后者是 E-1 那条"整份当未配置"）。
            verbose_logging: bool,
        }
        Raw {
            version: Preferences::VERSION,
            download_dir: &self.download_dir,
            verbose_logging: self.verbose_logging,
        }
        .serialize(serializer)
    }
}

/// 从文件内容解析（E-1：**任何**问题都当未配置，绝不抛）。
///
/// 判据同 macOS `AppPreferences.parse`：
///   * 不是 JSON / 顶层不是对象 / 没有 `version` / `version` 不认识 ⇒ **整份当未配置**；
///   * `download_dir` **缺失或不是字符串** ⇒ **未配置**（它不是损坏 —— 那一条在 macOS
///     侧也是"`as? String ?? ""`"，不折成整份失败）。
///
/// ⚠️ 于是那些"整份当未配置"的情形在这里表现为 `Err`（⇒ [`JsonFile::read`] 给 `None`
///    ⇒ [`Preferences::load`] 给 `empty()`），而"`download_dir` 不是字符串"表现为
///    一个正常的 `Ok` + 空串。**两条路的终点一样、路径不同**，别把它们合并。
impl<'de> Deserialize<'de> for Preferences {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            version: i64,
            #[serde(default)]
            download_dir: serde_json::Value,
            /// 🔴 **`#[serde(default)]` 是"缺字段 = 没开"这条红线的全部依靠**。
            ///
            /// 老客户机器上那份 `preferences.json` 里**没有这一格**。少了这个属性，
            /// `Raw::deserialize` 会以"缺字段"当场失败 ⇒ 整份 `Preferences` 解析不出来
            /// ⇒ [`Preferences::load`] 回 `empty()` ⇒ **下载目录凭空消失**，
            /// 而 `version` 明明是 1、日志里一个字都没有。**别删它**。
            ///
            /// ⚠️ 它**只**管"这一格不在"。这一格在场而**不是布尔**（有人手改成 `"true"`）
            ///    时这里会报错 ⇒ 整份当未配置（连 `download_dir` 一起）——
            ///    与 `download_dir` 不是字符串时的口径**不同**（那一格只退化成"未配置"，
            ///    见本函数上面那段文档）。如实记在这里，免得下一个人以为两处一样严。
            #[serde(default)]
            verbose_logging: bool,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.version != Self::VERSION {
            return Err(serde::de::Error::custom(format!(
                "preferences.json 的 version 是 {}，本壳只认 {}：整份当未配置（E-1）",
                raw.version,
                Self::VERSION
            )));
        }
        Ok(Preferences::new(raw.download_dir.as_str().unwrap_or(""))
            .setting_verbose_logging(raw.verbose_logging))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_support::TempDir;

    /// 磁盘形状（**字段名逐字**）：`version` + `download_dir` + `verbose_logging`，
    /// 三个键、不多不少。
    ///
    /// ⚠️ 第三个键是**写出去时永远在场**的（读进来时可以缺 —— 那是
    ///    `an_old_preferences_file_without_the_new_field_reads_as_off` 钉的事）：
    ///    写一份"省着写"的形状会让"老文件"与"这份偏好根本没配过"在盘上长得一样。
    #[test]
    fn the_disk_shape_is_version_plus_download_dir_and_verbose_logging() {
        let v = serde_json::to_value(Preferences::new("/abs/path")).expect("一定能序列化");
        assert_eq!(
            v,
            serde_json::json!({"version": 1, "download_dir": "/abs/path", "verbose_logging": false})
        );
        // 开着的时候那一格要真的跟着走（否则上面那条可以靠"永远写 false"变绿）。
        let on = Preferences::new("/abs/path").setting_verbose_logging(true);
        assert_eq!(
            serde_json::to_value(on).expect("一定能序列化")["verbose_logging"],
            serde_json::json!(true)
        );
    }

    /// 写出去、读回来是同一份（`load`/`save` 走的是同一条形状）。
    #[test]
    fn a_saved_preference_reads_back_the_same() {
        let dir = TempDir::new("pref-roundtrip");
        let path = dir.path().join(FILE_NAME);
        let saved = Preferences::new("/Users/x/Downloads/Benagen");
        saved.save(&path).expect("写盘要成功");
        assert_eq!(Preferences::load(&path), saved);
    }

    /// ⚠️ **E-1**：文件不在、内容坏了 —— 都是"未配置"，**不是错误**（所以没有 `Result`）。
    #[test]
    fn a_missing_or_broken_file_is_unconfigured_not_an_error() {
        let dir = TempDir::new("pref-e1");
        assert_eq!(Preferences::load(&dir.path().join(FILE_NAME)), Preferences::empty());
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        assert_eq!(Preferences::load(&dir.path().join(FILE_NAME)), Preferences::empty());
    }

    /// ⚠️ **不认识的 `version` ⇒ 整份当未配置**（E-1）。
    ///
    /// 判别力：去掉那道闸，未来版本写下的**别的形状**会被当成今天这份读进来 ——
    /// 而读错的配置比"没有配置"糟得多（它会把用户导向一个错误的下载目录）。
    #[test]
    fn an_unknown_version_falls_back_to_unconfigured() {
        let dir = TempDir::new("pref-version");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"version": 2, "download_dir": "/elsewhere"}"#).unwrap();
        assert_eq!(Preferences::load(&path), Preferences::empty(), "不认识的版本必须当未配置");
        // ⚠️ 反方向：版本是 1 时那一格**要被读进来**（否则上面那条可以靠"永远返回 empty"变绿）。
        std::fs::write(&path, br#"{"version": 1, "download_dir": "/elsewhere"}"#).unwrap();
        assert_eq!(Preferences::load(&path).download_dir, "/elsewhere");
    }

    /// ⚠️ `download_dir` **不是字符串**时是"未配置"，**不是**整份损坏
    /// （对齐 macOS：`root["download_dir"] as? String ?? ""`）。
    ///
    /// 这一条钉的是"逐字段容错"与"整份失败"的分界：手改过这个文件的人
    /// （或者一个把它写成数字的旧版本）不该让用户**连界面都进不去**。
    #[test]
    fn a_non_string_download_dir_is_unconfigured_not_a_broken_file() {
        let dir = TempDir::new("pref-type");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"version": 1, "download_dir": 7}"#).unwrap();
        assert_eq!(Preferences::load(&path), Preferences::empty());
        // 缺这一格同样是"未配置"。
        std::fs::write(&path, br#"{"version": 1}"#).unwrap();
        assert_eq!(Preferences::load(&path), Preferences::empty());
    }

    /// ⚠️ **只去首尾空白**（E-8）：路径**里面**的空格、结尾的 `/`、`..` 一个都不许动 ——
    /// 这条路径要原样进内核的 argv，壳替它解释一遍就会让"壳以为的路径"与
    /// "内核实际用的路径"悄悄分叉（代价是**文件下到别的地方去**）。
    #[test]
    fn normalization_only_trims_the_edges() {
        assert_eq!(Preferences::new("  /a b/c  ").download_dir, "/a b/c");
        assert_eq!(Preferences::new("/a/b/").download_dir, "/a/b/");
        assert_eq!(Preferences::new("/a/../b").download_dir, "/a/../b");
        // 空串（或全空白）= 未配置。
        assert!(!Preferences::new("   ").is_configured());
        assert!(!Preferences::empty().is_configured());
    }

    /// 「恢复默认」= 传空串 ⇒ 回到未配置（**不是**删掉这个文件）。
    #[test]
    fn resetting_the_directory_goes_back_to_unconfigured() {
        let configured = Preferences::new("/abs/path");
        assert!(configured.is_configured());
        let reset = configured.setting_download_dir("");
        assert!(!reset.is_configured());
        assert_eq!(reset, Preferences::empty());
    }

    /// 读进来的那一份**也过规范化**（对齐 macOS：`parse` 走的是同一个 `init`）。
    #[test]
    fn a_loaded_value_is_normalized_too() {
        let dir = TempDir::new("pref-normalize");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"version": 1, "download_dir": "  /abs/path\n"}"#).unwrap();
        assert_eq!(Preferences::load(&path).download_dir, "/abs/path");
    }

    /// 把一段**原文**写进一个一次性的临时文件，返回它的路径。
    ///
    /// ⚠️ 为什么要有它（而不是用既有的 `TempDir` + `std::fs::write` 两行）：下面那条
    ///    "旧客户端的文件"要的输入是**盘上逐字就是那样**的一段字节 —— 拿
    ///    `Preferences` 反过来序列化一份再读，测的就成了"自己写的自己读得回来"，
    ///    而**缺字段**这件事根本不会出现（我们写出去的那一份永远带着新字段）。
    ///
    /// ⚠️ **这个临时目录是有意泄漏的**（`std::mem::forget`）：本函数的签名只回一个
    ///    `PathBuf`，没有东西持有 `TempDir` 的句柄 ⇒ 它一 drop 就把刚写的文件删掉。
    ///    代价如实记：`std::env::temp_dir()` 下会留下几个 `benagen-storage-pref-old-*`
    ///    空目录（每次跑用例一个，名字里带 pid 与纳秒）。它**不碰真机上那份偏好**
    ///    （`storage::dir()` 那个目录），这是本模块用例唯一的硬底线。
    fn write_temp(text: &str) -> std::path::PathBuf {
        let dir = TempDir::new("pref-old");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, text).expect("写临时偏好文件");
        std::mem::forget(dir);
        path
    }

    /// 🔴 **缺字段的文件必须读成"没开"**（规格 §2.4 的红线）。
    ///
    /// 判别力：把 `Raw` 里那个 `#[serde(default)]` 删掉 ⇒ 老客户机器上那份
    /// **只有 `{version, download_dir}` 的 preferences.json 会整份解析失败** ⇒
    /// `load()` 回 `empty()` ⇒ **下载目录凭空消失**，而没有任何东西会变红。
    #[test]
    fn an_old_preferences_file_without_the_new_field_reads_as_off() {
        let old = r#"{"version": 1, "download_dir": "D:\\交付"}"#;
        let parsed = JsonFile::read::<Preferences>(&write_temp(old)).expect("旧文件必须还能读");
        assert_eq!(parsed.download_dir, "D:\\交付", "旧字段不许丢");
        assert!(!parsed.verbose_logging, "缺字段 = 没开详细日志");
    }

    /// ⚠️ **两格互不覆盖**（这是**生产那条路**：`commands::preferences_set` 走的就是
    /// `setting_download_dir`）。
    ///
    /// 判别力：把 `setting_download_dir` 写回 `Preferences::new(path)`（它把那一格置回
    /// `false`），这一条立刻红 —— 而真机上的表现是客户排查到一半换了个下载目录，
    /// 诊断日志**静默地退回普通档**，而 `diag-shell.log` 还照样在长
    /// （只是不再记那每一次往返了），**没有任何东西会变红**。
    #[test]
    fn the_two_settings_never_overwrite_each_other() {
        let on = Preferences::new("/data/交付").setting_verbose_logging(true);
        assert!(on.setting_download_dir("/data/新的").verbose_logging, "改目录把开关带跑了");
        assert_eq!(on.setting_download_dir("/data/新的").download_dir, "/data/新的");
        assert_eq!(
            on.setting_verbose_logging(false).download_dir,
            "/data/交付",
            "改开关把目录带跑了"
        );
        // 「恢复默认」那条路同样是"只动目录那一格"。
        assert!(on.setting_download_dir("").verbose_logging);
    }

    /// 🔴 **版本号不许递增**（规格 §2.4 与全局约束 5）。
    ///
    /// 判别力：把 `VERSION` 改成 2 ⇒ 上面那条用例立刻红（旧文件被当成"不认识的版本"
    /// 整份丢弃）。这一条钉的是那个常量本身，好让"递增"这个动作**必须**先改一条用例。
    #[test]
    fn the_version_does_not_move_when_a_field_is_added() {
        assert_eq!(Preferences::VERSION, 1, "递增版本号会让旧客户端的下载目录被清空");
    }

    /// **开启之后写盘、再读回来还是开着的。**（**真的过一遍盘**）
    ///
    /// 🔴 **判别力（变异实测，见报告「修复轮 1」）**：把 `Deserialize` 末尾那行
    ///    `.setting_verbose_logging(raw.verbose_logging)` 换成
    ///    `.setting_verbose_logging(false)`，**全仓只有这一条会红**。
    ///
    ///    它藏起来的正是这个功能的主链：**勾上 → `save` → 下次启动 `load` →
    ///    `apply_shell_log_level` → `Verbose`**。反序列化在这里错的表现是
    ///    "**客户开了详细日志、重启之后 `diag-shell.log` 一行都没有**" ——
    ///    那正是这个 task 存在的理由。
    ///
    /// ⚠️ 这一条**必须是盘上往返**，不许退回成"内存里的构造函数检查"：
    ///    那种写法（`new()` / `setting_verbose_logging()` 各看一眼）走的是**构造**那一侧，
    ///    反序列化那一侧一个字都碰不到 —— 而错的恰恰可以是那一侧。
    ///    同形的那条在呈现层（`presentation/app_preferences.rs` 的
    ///    `a_preference_without_the_verbose_field_reads_as_off_and_the_version_stays`
    ///    最后那一段），两条各钉各的一层，**别把任何一条删成另一条的重复**。
    #[test]
    fn the_verbose_flag_round_trips() {
        let dir = TempDir::new("pref-verbose-roundtrip");
        let path = dir.path().join(FILE_NAME);

        // 起手（没配过）是**关**的 —— 它是"没配过"这个状态的一部分。
        assert!(!Preferences::new("D:\\交付").verbose_logging);

        // 打开 → **写盘** → 读回来：那一格必须原样还在。
        let on = Preferences::new("D:\\交付").setting_verbose_logging(true);
        assert!(on.verbose_logging);
        on.save(&path).expect("写盘要成功");
        let back = Preferences::load(&path);
        assert_eq!(back, on, "整份要原样回来");
        assert!(
            back.verbose_logging,
            "开着的那一份写下去、读回来必须还是开着的（盘上真的写了这一格）"
        );

        // ⚠️ **反方向**：关着的那一份写下去、读回来也是关的 —— 少了它，上面那条
        //    可以靠"读的时候永远给 true"变绿。
        Preferences::new("D:\\交付").save(&path).expect("写盘要成功");
        assert!(
            !Preferences::load(&path).verbose_logging,
            "关着的那一份读回来不许变成开着的"
        );
    }

    /// ⚠️ **写失败要能到调用方手里**（E-2 的下半条）：
    ///    一次"以为存上了"的静默失败，用户要等到下一次启动才发现设置又变回去了。
    #[test]
    fn a_failed_save_reports_the_failure_instead_of_swallowing_it() {
        let dir = TempDir::new("pref-savefail");
        let path = dir.path().join(FILE_NAME);
        Preferences::new("/a").save(&path).expect("先写一份好的");
        // 占住临时文件名 ⇒ 下一次写必然失败。
        std::fs::create_dir(dir.path().join(format!(".{FILE_NAME}.tmp"))).unwrap();
        let failed = Preferences::new("/b").save(&path);
        assert!(failed.is_err(), "写盘失败必须报给调用方");
        assert_eq!(Preferences::load(&path).download_dir, "/a", "失败的写把旧配置改掉了");
    }
}
