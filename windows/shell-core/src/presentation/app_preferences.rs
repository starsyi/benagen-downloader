//! app_preferences —— 壳的偏好（今天只有一项：**下载目录**）与「改下载目录」那条链路。
//!
//! 上游三处（本模块把三处合成一个文件，理由见下）：
//!
//!   | 本文件里的东西 | 上游 |
//!   |---|---|
//!   | [`AppPreferences`] | `macos/Sources/BenagenCoreKit/Presentation/AppPreferences.swift` |
//!   | [`DownloadDirectory`] | `macos/Sources/BenagenCoreKit/Presentation/DownloadDirectory.swift` |
//!   | [`DownloadDirChange`] | `DownloadDirectory.swift` 下半段（**上游没有独立文件**）+ `macos/Sources/BenagenDownloader/Settings/SettingsView.swift:120-180` 附近那几段 |
//!   | [`DiagnosticsToggle`] / [`VerboseLoggingChange`] | 规格 §2.4 + macOS `SettingsView` 新增的那一节「诊断」（**上游是一句写死在视图里的话**，这一侧必须由壳给：前端不许自造文案） |
//!
//! ⚠️ **为什么合成一个文件**：`DownloadDirChange` 在上游**散在两处**（值类型在
//!    `DownloadDirectory.swift` 里、而它的两个渲染落点与那几句壳自己写的话在
//!    `SettingsView.swift` 的设置段里）。三者是**同一条链路的三段**
//!    （偏好是输入、`DownloadDirectory` 是判据与文案、`DownloadDirChange` 是结果），
//!    拆开只会让"改目录会发生什么"这个问题要跳三个文件才答得完。

//! ---------------------------------------------------------------------------
//! 🔴 **层契约的例外申报**（照抄上游 `DownloadDirectory.swift:20-42` 的那一段）
//! ---------------------------------------------------------------------------
//!
//! `presentation/` 这一层的契约是"**纯计算、不碰文件系统**"，而 [`DownloadDirectory::check`]
//! 要问"这个位置存不存在 / 是不是文件夹 / 能不能写"—— 那**只能**问文件系统
//! （路径字符串上看不出来）。所以把这条例外**申报在这里**：
//!
//!   · **为什么需要它**：规格要求"目录不可用 ⇒ **在确认对话框之前**就报错并中止"。
//!     等重启完才发现"这个文件夹不可写"，用户已经为它停掉了一次正在跑的任务。
//!     放到调用方也不行：那会把一条**判据**塞进命令层，而这一层的规矩是
//!     "能写出断言的都归这里"，而且两个调用点就得各写一遍。
//!   · **为什么可以接受**（代价如实记下）：它破的是"这一层不碰盘"这条**阅读契约**，
//!     不是"这一层可断言"那条**测试契约** —— [`DownloadDirectory::check`] 仍然是纯函数
//!     （同名输入同名输出、没有状态、没有时钟），只是从**磁盘**取输入，
//!     所以它照样有单测（`DownloadDirectoryTests` 那几条边界，只碰临时目录）。
//!   · **本 crate 的先例**：`shell-core/src/embedded_core.rs` 就在做文件 IO
//!     （把内嵌的内核 exe 释放到缓存目录），所以 `std::fs` 在这里不是新面孔。
//!     全 `presentation/` 里**唯一**碰文件系统的仍然只有 `check` 这一个判据 ——
//!     下一个要往这一层加"顺手读一下盘"的人：**别照抄这里**。
//!
//! ⚠️ **与上游的一处实现差异（W-6）**：上游用 `FileManager.isWritableFile(atPath:)`
//!    （POSIX 上就是 `access(W_OK)`）。Rust 的 `std` **没有** `access(2)` 的跨平台封装，
//!    而本 crate **不许引依赖**、也**不许把 `#[cfg]` 撒进来**（全 crate 唯一的 `cfg!`
//!    在 `platform.rs`，那条纪律有账）。所以这里改成**探针写**：
//!    在该目录下**真建一个临时文件再删掉**，建得出来就算可写（见 [`probe_writable`]）。
//!
//!    为什么**不是** `permissions().readonly()`（那条路是错的，值得记一笔）：
//!      · **Windows（我们的交付平台）**：`readonly()` 读的是 `FILE_ATTRIBUTE_READONLY`
//!        那个属性位，而**目录几乎永远不带这个位** ⇒ 这道判据在交付平台上**恒过**，
//!        等于"不可写的目录会被判成可写" —— 那不是精度差一点，是**判据失效**；
//!      · macOS / Linux：`readonly()` 是 `(mode & 0o222) == 0`，不认属主、不认 ACL、
//!        不认有效 uid ⇒ 别人拥有的 `0755` 目录会被判成可写。
//!
//!    探针写比 `access(W_OK)` **更强**（后者有 TOCTOU：检查完到真写之间权限可能变），
//!    而且它**直接回答那个问题本身**："我能不能往这儿写东西"。
//!    我们要对齐的是上游那条判据**断言的行为**（不可写的目录必须被拒），不是它的实现手段。

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 壳的偏好
// ---------------------------------------------------------------------------

/// 壳的偏好（今天只有一项：**下载目录**）。
///
/// 上游 `AppPreferences.swift` 的 `AppPreferences`。
///
/// ⚠️ **"未配置"是有效状态，不是错误**：空串 ⇒ 壳**不传** `--download-dir`，
///    由内核用自己的默认值。不要为了"看起来明确"而显式传一个"和内核默认一样"的值
///    —— 那会在内核默认值变化时静默分叉。
///
/// ⚠️ 与批次历史同一条底线：读不出来 / 解析失败 / `version` 不认识 ⇒ **当未配置**并继续
///    启动，绝不让应用起不来。所以 [`AppPreferences::parse`] **没有 `Result`**。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AppPreferences {
    /// 用户选的下载目录。**空串 = 未配置**（不传 `--download-dir`）。
    pub download_dir: String,

    /// **详细诊断日志**（规格 §2.4）。开着 ⇒ 壳给内核多拼一对
    /// `--log-level verbose`（见 [`DownloadDirectory::core_arguments_for`]）。
    ///
    /// 🔴 **缺字段 = `false`**：老客户那份 `preferences.json` 里没有这一格，
    ///    而 [`Self::parse`] 的办法是 `as_bool().unwrap_or(false)`（见那里）。
    pub verbose_logging: bool,
}

impl AppPreferences {
    /// 给未来的自己的版本号：**不认识的版本 ⇒ 当未配置**。
    ///
    /// 上游 `AppPreferences.version`。
    pub const VERSION: i64 = 1;

    /// 没有配置过任何东西（= 一切走默认）。
    ///
    /// 上游 `AppPreferences.empty`。
    pub const fn empty() -> AppPreferences {
        AppPreferences {
            download_dir: String::new(),
            verbose_logging: false,
        }
    }

    /// 建一份偏好（路径按 [`Self::normalized`] 归一化）。
    ///
    /// 上游 `AppPreferences.init(downloadDir:)`。
    ///
    /// ⚠️ **只造"下载目录"那一格**：详细日志起手是关着的，要开用
    ///    [`Self::setting_verbose_logging`]（与 `storage::preferences` 同一条口径）。
    pub fn new(download_dir: &str) -> AppPreferences {
        AppPreferences {
            download_dir: Self::normalized(download_dir),
            verbose_logging: false,
        }
    }

    /// 配过下载目录没有。**它是"要不要传 `--download-dir`"的唯一判据**。
    ///
    /// 上游 `AppPreferences.isConfigured`。
    pub fn is_configured(&self) -> bool {
        !self.download_dir.is_empty()
    }

    /// 改下载目录（返回新值）。传空串 = 回到未配置（那颗「恢复默认」）。
    ///
    /// 上游 `AppPreferences.settingDownloadDir(_:)`。
    ///
    /// ⚠️ **详细日志那一格原样带过去**（不走 [`Self::new`]，它会把那一格置回 `false`）：
    ///    "改目录"与"关掉详细日志"是两件事，合成一件的表现是客户排查到一半换了个目录、
    ///    日志**静默地退回普通档**。
    pub fn setting_download_dir(&self, path: &str) -> AppPreferences {
        AppPreferences {
            download_dir: Self::normalized(path),
            verbose_logging: self.verbose_logging,
        }
    }

    /// 开关详细诊断日志（返回新值）。**这是那一格的唯一入口**。
    ///
    /// 对称的那一条：`download_dir` **原样带过去**（同上，两件事不许互相覆盖）。
    pub fn setting_verbose_logging(&self, on: bool) -> AppPreferences {
        AppPreferences {
            download_dir: self.download_dir.clone(),
            verbose_logging: on,
        }
    }

    /// 从文件内容解析（**任何**问题都当未配置，绝不抛）。
    ///
    /// 上游 `AppPreferences.parse(_:)`。
    ///
    /// 判据同批次历史的解析：不是 JSON / 顶层不是对象 / 没有 `version` /
    /// `version` 不认识 / `download_dir` 不是字符串 ⇒ 一律"未配置"。
    /// （`download_dir` 缺失或为空串同样是"未配置"，它不是损坏。）
    pub fn parse(text: &str) -> AppPreferences {
        if text.is_empty() {
            return AppPreferences::empty();
        }
        let Ok(root) = serde_json::from_str::<Value>(text) else {
            return AppPreferences::empty();
        };
        let Value::Object(map) = root else {
            return AppPreferences::empty();
        };
        let Some(Value::Number(version)) = map.get("version") else {
            return AppPreferences::empty();
        };
        if version.as_i64() != Some(Self::VERSION) {
            return AppPreferences::empty();
        }
        // 🔴 **缺字段 = `false`**（老客户那份文件里没有这一格）：`as_bool()` 对
        //    "不在"与"不是布尔"都给 `None` ⇒ 一律落回"没开"。
        //    ⚠️ 与 `download_dir` 那条**逐字同一条纪律**：**一格读歪了不牵连整份** ——
        //       这条路的全部意义就是"读不出偏好也照样启动"（所以它没有 `Result`）。
        let verbose_logging = map.get("verbose_logging").and_then(Value::as_bool).unwrap_or(false);
        match map.get("download_dir") {
            Some(Value::String(dir)) => AppPreferences::new(dir),
            _ => AppPreferences::empty(),
        }
        .setting_verbose_logging(verbose_logging)
    }

    /// 落盘的形状（`{"version": 1, "download_dir": "/abs/path", "verbose_logging": false}`，
    /// 键名逐字）。
    ///
    /// 上游 `AppPreferences.serialized()`。
    pub fn serialized(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&json!({
            "version": Self::VERSION,
            "download_dir": self.download_dir,
            "verbose_logging": self.verbose_logging,
        }))
    }

    /// ⚠️ **有意只去掉首尾空白，不做路径规范化**（理由写在这里）：
    ///    这条路径最终要**原样进内核的 argv**（`--download-dir`）。软链接、`..`、
    ///    结尾的 `/` 都是**文件系统与内核的语义**，壳在这儿替它解释一遍，
    ///    只会让"壳以为的路径"和"内核实际用的路径"悄悄分叉 ——
    ///    而分歧的代价是**文件下到别的地方去**。
    ///
    /// 上游 `AppPreferences.normalized(_:)`。
    fn normalized(raw: &str) -> String {
        raw.trim().to_string()
    }
}

// ---------------------------------------------------------------------------
// 下载目录：显示值 / 内核参数 / 可用性 / 确认文案
// ---------------------------------------------------------------------------

/// 可选下载目录的**纯计算**。
///
/// 上游 `DownloadDirectory.swift` 的 `DownloadDirectory`（空 `enum` + `static` 命名空间；
/// 这里对位成空 `enum` + 固有 `impl`，同 `format.rs` 的 `ByteFormat`）。
///
/// 下载目录只能经 **argv** 在内核启动时定（`--download-dir`），协议里的 `set_settings`
/// **没有**这个键 ⇒ "改目录"必然是**重启内核**。
pub enum DownloadDirectory {}

impl DownloadDirectory {
    /// 内核默认目录的**人话形式**。
    ///
    /// ⚠️ **它只是显示，永远不进 argv**："不许显式传一个与内核默认值相同的值"禁的是
    ///    后者，不是"不许告诉用户文件下到哪"。后果的**量级**要看清：内核哪天改了默认值，
    ///    这句话会过时（顶多误导），而**不会**让文件下到别处去 ——
    ///    后者才是要防的静默分叉，而它已经被 [`Self::argument`] 的 `None` 挡死了。
    ///
    /// 上游 `DownloadDirectory.kernelDefaultDisplayPath`。
    pub const KERNEL_DEFAULT_DISPLAY_PATH: &'static str = "~/Downloads/Benagen";

    /// 未配置时在设置窗口里显示的那一行。
    ///
    /// 上游 `DownloadDirectory.defaultDisplay`（那边是 `static let` 内插；
    /// 这里写成函数，从**唯一一份** [`Self::KERNEL_DEFAULT_DISPLAY_PATH`] 派生 ——
    /// 形态偏离同 `delivery_summary.rs` 的 `too_long_hint`，保住的正是"改一处不会打架"）。
    pub fn default_display() -> String {
        format!(
            "默认（{}，由内核决定）",
            Self::KERNEL_DEFAULT_DISPLAY_PATH
        )
    }

    /// 设置窗口里那一段的脚注。**说明"未配置"是什么意思** —— 它是本功能唯一
    /// 反直觉的地方（"没有设置"也是一种设置）。
    ///
    /// ⚠️ 最后那句是**网络盘的提示**：在此之前"下载目录在本地盘上"是**结构上**成立的
    ///    （硬编码），而今天用户一步操作就能把它指到 NFS/SMB 上 ⇒ 内核每次列目录 /
    ///    取树 / 加入下载都要**持着内核锁**对每个路径 stat 一遍，网络盘的 stat 是秒级
    ///    ⇒ 内核的同步读循环停摆。形态是**变慢**、不是变错，所以不改内核，只把这件事
    ///    **说出来**。
    ///    🔴 这句话是**壳自己写的**：内核不会为一件它还没遇到的慢因说话。
    ///
    /// 上游 `DownloadDirectory.sectionNote`（逐字，含那两对 `**`）。
    pub const SECTION_NOTE: &'static str = "没有设置过时，壳**不指定**下载目录，由内核用它自己的默认值。\
改变目录会立刻重启内核（正在跑的任务会停）。\
请尽量选**本机磁盘**上的目录：网络盘（NFS / SMB）会让界面明显变慢。";

    /// 当前下载目录只读显示的文字。空串 = 未配置 ⇒ 说"默认"，而不是画一个空行。
    ///
    /// 上游 `DownloadDirectory.display(_:)`。
    pub fn display(path: &str) -> String {
        if path.is_empty() {
            Self::default_display()
        } else {
            path.to_string()
        }
    }

    /// 该不该给内核传 `--download-dir`：未配置 ⇒ `None`（= 那个 flag 根本不出现）。
    ///
    /// ⚠️ **不许**改成"返回一个与内核默认值相同的路径"：那样只要内核改了默认值，
    ///    壳就会把用户悄悄带回老地方 —— 一次没有任何提示的分叉。
    ///
    /// 上游 `DownloadDirectory.argument(for:)`。
    pub fn argument(preferences: &AppPreferences) -> Option<String> {
        if preferences.is_configured() {
            Some(preferences.download_dir.clone())
        } else {
            None
        }
    }

    /// 目录可用性检查。`None` = 可以用；`Some(…)` = **直接显示给用户**的那句话。
    ///
    /// ⚠️ 时机：**在确认对话框之前**。等重启完才发现"这个文件夹不可写"，
    ///    用户已经为它停掉了一次正在跑的任务。
    ///
    /// ⚠️ 空串**直接放行**：它不是路径，是"回到未配置" —— 内核会用自己的默认值
    ///    并在启动时建它，壳无从检查，也不该替它猜。
    ///
    /// 🔴 这几句是**壳自己写的**：内核**还不知道**这件事（它还没被起起来），
    ///    没有原文可登。而"选了一个不能写的目录"必须有落点 ——
    ///    否则用户会在重启之后看到引擎报一句他看不懂的错，或者更糟：什么都没看到。
    ///
    /// 🔴 **本 crate 里唯一碰文件系统的判据**（例外申报见模块头）。
    ///
    /// 上游 `DownloadDirectory.check(_:)`。
    pub fn check(path: &str) -> Option<String> {
        if path.is_empty() {
            return None;
        }
        if !path_exists(path) {
            return Some(format!("这个位置不存在：{path}"));
        }
        if !path_is_directory(path) {
            return Some(format!("这是一个文件，不是文件夹：{path}"));
        }
        // ⚠️ 探针**只在这两步都过了之后**才跑：路径不存在或不是目录时往里建文件
        //    要么失败得更难看、要么在别的地方留下垃圾。
        if !probe_writable(path) {
            return Some(format!("这个文件夹不可以写入（没有权限）：{path}"));
        }
        None
    }

    /// **把偏好接成内核的 argv**（审查 I-2 补的那根缝，任务 13 会直接用它）。
    ///
    /// 🔴 **这根缝是刻意加在这里的**："未配置 ⇒ argv 里不出现 `--download-dir`"这条判据
    ///    **横跨两个模块**（本模块的 `argument()` 与 `client::core_arguments`）——
    ///    只有让它们在同一条路径上组装，判据才**测得到**（macOS 侧那两条 argv 用例
    ///    正是这个形状：它们喂的是 `CoreClient.coreArguments`）。
    ///    没有它，"E-5 不许静默分叉"这条纪律在仓库里**一条断言都没有** ——
    ///    而那正是"壳自己编一个和内核默认一样的路径"唯一会出事的地方。
    ///
    /// ⚠️ 实现**直接转调** [`crate::client::core_arguments`]，**不另写一份 argv 拼装**：
    ///    拼两份的结局是 flag 名/顺序漂移，而那种漂移不会报错 —— 内核会照单全收，
    ///    只是行为与壳以为的不一样。
    ///
    /// ⚠️ 顺序与 `core/src/main.rs::parse_args` 一致这件事由 `client::core_arguments`
    ///    那一侧负责（它自己的文档注释里写着），这里只负责把偏好接上去。
    pub fn core_arguments_for(
        preferences: &AppPreferences,
        settings_path: Option<&std::path::Path>,
    ) -> Vec<String> {
        let download_dir = Self::argument(preferences);
        crate::client::core_arguments(
            download_dir.as_deref().map(std::path::Path::new),
            settings_path,
            preferences.verbose_logging,
        )
    }

    /// 改目录之前必须让用户看到的那段话。**三条后果逐条写明**。
    ///
    /// ⚠️ 第 2 条的措辞是**「新目录里没有记录的文件会重新校验」**，不是"一定会重新校验"：
    ///    用户完全可能选了一个**已经有状态文件的目录**（例如换回旧目录），
    ///    那时内核会把状态读回来，一个字节都不用重新校验。
    ///    ⚠️ **别把它"说全"**：一句断言式的"所有文件都会重新校验"会让用户为一件
    ///    不会发生的事做决定 —— 而这条文案的全部价值就是让他知道**会发生什么**。
    ///
    /// 上游 `DownloadDirectory.confirmation(from:to:)`（逐字，含那个空行）。
    pub fn confirmation(from: &str, to: &str) -> String {
        format!(
            "下载目录将从「{}」改为「{}」。\n\
             \n\
             改完会立刻重启内核，三条后果：\n\
             1. 正在跑的任务会停（内核重启），没传完的文件要重新点一次下载；\n\
             2. 新目录里没有记录的文件会重新校验（内核的状态文件就在下载目录里）；\n\
             3. 旧目录里的文件不会被删、也不会被搬走。",
            Self::display(from),
            Self::display(to)
        )
    }
}

/// 这个位置在不在（`std::fs::metadata` 成功即"在"）。
///
/// ⚠️ 与上游 `FileManager.fileExists` 的口径一致：**不跟随**软链接与否、
///    是不是目录都不影响"在不在"这个判断。
fn path_exists(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

/// 这个位置是不是一个**目录**（不存在 ⇒ `false`）。
fn path_is_directory(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// **探针写**：在这个目录下真建一个临时文件，建得出来就说明可写；建出来的那个
/// **一定删掉**（成功与失败两条路都清）。
///
/// 🔴 **这是本 crate 里唯一一处"为了回答一个判据而真的写盘"**（见模块头的例外申报与
///    W-6 记账）：上游用 `access(W_OK)`，而 Rust 的 `std` 没有它的跨平台封装。
///    `permissions().readonly()` 那条路**在交付平台（Windows）上恒过** —— 目录几乎
///    永远不带 `FILE_ATTRIBUTE_READONLY` 位 —— 所以它是一条**失效的判据**，
///    不是"精度差一点"。
///
/// ⚠️ **为什么探针写是更强的判据**：`access(W_OK)` 与"真的写"之间存在 TOCTOU
///    （检查完到真写之间权限/磁盘可能变），而探针写**就是**那个操作本身。
///    代价如实记：它会在用户选的目录里**短暂地出现一个临时文件**（毫秒级），
///    名字带前缀便于识别；并且它不能区分"没有权限"与"磁盘满了"——
///    两者对用户是同一句话（"这个文件夹不可以写入"），而**内核**才是最终防线。
///
/// ⚠️ 名字里带 pid 与纳秒：两个进程、或同一进程的两次调用**不会撞名** ——
///    撞名的后果是"我把别人的探针删了"，而对方那一侧正等着它。
fn probe_writable(path: &str) -> bool {
    let probe = std::path::Path::new(path).join(probe_file_name());
    match std::fs::File::create(&probe) {
        Ok(file) => {
            // 先把句柄放掉再删（Windows 上打开着的文件删不掉）。
            drop(file);
            // ⚠️ **无论后面发生什么都要清掉**：删除失败**不**改变判决 ——
            //    "建得出来"这件事已经回答了那个问题（能不能写），删不掉属于另一个
            //    问题（清理），把它算成"不可写"会让用户看到一个与事实相反的拒绝。
            let _ = std::fs::remove_file(&probe);
            true
        }
        // 建不出来就是不可写（权限 / 只读介质 / 磁盘满 —— 对用户是同一句话）。
        Err(_) => false,
    }
}

/// 探针文件的文件名。**必须一眼看得出是谁的**（万一真的残留，用户知道能不能删），
/// 而且要在同一台机器上唯一。
fn probe_file_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(".benagen-write-probe-{}-{nanos}", std::process::id())
}

// ---------------------------------------------------------------------------
// 「详细日志」那一个勾选框：标签 / 说明 / 一次开关的结果
// ---------------------------------------------------------------------------

/// 「详细日志」那一个勾选框的**纯计算**（规格 §2.4）。
///
/// 上游：macOS `SettingsView` 新增的那一节「诊断」（与既有的 `DownloadDirectorySection`
/// 并列）—— 那边是一句写死在视图里的说明，这边**必须由壳给**：
/// 本代的前端是一个字都不许自己造的（`check_frontend_copy.sh` 会把硬编码中文拦下）。
pub enum DiagnosticsToggle {}

impl DiagnosticsToggle {
    /// 勾选框的**标签**（规格 §2.4 逐字：「勾选「详细日志」」）。
    ///
    /// ⚠️ 它**进的是载荷**（不是 `index.html` 的结构文案）：勾选框是运行时由 JS 画出来的
    ///    （`settings.js:paintPreferences` 要按偏好的当前值去设 `checked`），
    ///    字跟着那一格一起下来，就不必在 HTML 与 Rust 里各存一份。
    pub const LABEL: &'static str = "详细日志";

    /// 标签下面那句说明。**三件事缺一不可**：
    ///   * 它**是什么用的**（排查问题）—— 否则用户会以为这是个"性能档"；
    ///   * **用完要关**（详细档是"每一次往返都记"，开着不放会一直占盘）；
    ///   * **改了会重启内核**（与下载目录那条同一后果，不说的话用户会在一次
    ///     正在跑的下载被停掉时才莫名其妙）。
    ///
    /// ⚠️ 最后那半句与 [`DownloadDirectory::SECTION_NOTE`] 说的是**同一件事**
    ///    （那次重启），措辞也尽可能同源 —— 但**不许**为了"少一句话"就把它省掉：
    ///    这是本面板上第二处会让内核重启的控件。
    pub const NOTE: &'static str = "打开后日志会详细很多（用于排查问题），排查完请关掉。\
改动会立刻重启内核（正在跑的任务会停）。";
}

/// 一次「改详细日志」的结果。
///
/// ## 🔴 为什么它是**另一个类型**，而不是复用 [`DownloadDirChange`]
///
/// 简报让 `verbose_logging_set` 复用 `preferences_set` 那一套**回执形状**
/// （"对用户来说这是同一件事：改了一项要重启内核的设置"）——本类型**就是那个形状**
/// （四格：标题 / 路径行 / 完整那句 / 是不是失败，`api::preferences` 装成同一组键）。
///
/// ⚠️ 但**那三句话没有复用**：`DownloadDirChange` 的三句把主词写死成了"下载目录"
///    （`下载目录已改（内核已按新目录重启）` / `已恢复默认下载目录（…）` /
///    `下载目录没有变化，没有重启内核`）。拿它去报一次**勾选框**的改动，用户会在
///    常驻回执上读到一句**假话** —— 他一个目录都没改。这个仓库对"界面上说假话"的
///    口径是明确的（`the_confirmation_never_claims_every_file_will_be_reverified`
///    那条用例就是为同一件事立的），所以宁可多这一个类型。
///
/// ⚠️ 与 [`DownloadDirChange`] 的第二个差别：**没有"路径"那一格**
///    （`path_detail` 恒 `None`）—— 这一格改的是一个开关，没有路径可显示。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VerboseLoggingChange {
    /// 改成了：偏好已落盘、内核已按**新**档位重启、**壳自己那一格也已经就位**。
    Changed {
        /// 改完之后是"开着"吗（它只影响那句话的措辞）。
        on: bool,
    },
    /// 目标与当前一致 ⇒ **什么都没做**（内核没重启、正在跑的任务不受影响）。
    Unchanged {
        /// 当前是"开着"吗。
        on: bool,
    },
    /// 没改成：这是**可直接显示给用户的那句话**（内核原文 / 写盘失败原文，照登）。
    Failed { message: String },
}

impl VerboseLoggingChange {
    /// 成功那支的**固定标题**（**两个方向各一句**：开着与关着是两件事，
    /// 说成同一句会让用户分不清自己那一下把它改成了什么）。
    ///
    /// ⚠️ 它不含任何**长度不受控**的东西（没有路径、没有批次号）——
    ///    与 `DownloadDirChange::headline` 同一条纪律：这条回执是**常驻**的那一行，
    ///    高度不许由外部文本决定（那是一次真实布局事故的根因）。
    const CHANGED_ON: &'static str = "详细日志已打开（内核已按详细档重启）";
    const CHANGED_OFF: &'static str = "详细日志已关闭（内核已按普通档重启）";

    /// 没改动那一支的固定全文（与 `DownloadDirChange::UNCHANGED` 逐字同构）。
    const UNCHANGED_ON: &'static str = "详细日志没有变化，没有重启内核";

    /// **这条回执的第一行**：固定短的标题，任何情况下都不含外部文本。
    ///
    /// ⚠️ `Failed` 那一格**照登原文**（内核的失败原因 / 写盘失败的系统文本），
    ///    不加工、不截断成"标题"—— 与 `DownloadDirChange::headline` 逐字同一条。
    pub fn headline(&self) -> String {
        match self {
            VerboseLoggingChange::Changed { on: true } => Self::CHANGED_ON.to_string(),
            VerboseLoggingChange::Changed { on: false } => Self::CHANGED_OFF.to_string(),
            VerboseLoggingChange::Unchanged { .. } => Self::UNCHANGED_ON.to_string(),
            VerboseLoggingChange::Failed { message } => message.clone(),
        }
    }

    /// **要单独占一行显示的路径**；**恒 `None`**（见类型文档：这一格没有路径）。
    ///
    /// ⚠️ 方法留着、不删：两个回执在 `api::preferences` 里装的是**同一组键**
    ///    （前端只有一条渲染路径），那一格空着比缺着好 —— 缺键会让前端读到的
    ///    是 `undefined` 而不是"没有这一行"。
    pub fn path_detail(&self) -> Option<String> {
        None
    }

    /// 这条回执**完整的一句话**。
    ///
    /// ⚠️ 没有路径可拆 ⇒ 全文就是 [`Self::headline`]（两处不可能分叉）。
    pub fn notice_text(&self) -> String {
        self.headline()
    }

    /// 这一行说的是"没成"吗（调用方据此选图标与颜色）。
    pub fn is_failure(&self) -> bool {
        matches!(self, VerboseLoggingChange::Failed { .. })
    }
}

// ---------------------------------------------------------------------------
// 一次「改下载目录」的结果
// ---------------------------------------------------------------------------

/// 界面只需要知道这三件事。
///
/// 上游 `DownloadDirectory.swift` 的 `DownloadDirChange`。
///
/// ⚠️ 与换码结果同一个形状、同一个理由：结果要**可断言**，而"该说什么"不该由调用方
///    现编。`Failed` 里那句可能是内核原文，也可能是写盘失败的原文 —— 两种都**照登**。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DownloadDirChange {
    /// 改成了：偏好已落盘、内核已按**新**目录重启。`dir` 为空串 = 恢复默认。
    Changed { dir: String },
    /// 目标与当前一致 ⇒ **什么都没做**（内核没重启、正在跑的任务不受影响）。
    ///
    /// ⚠️ 这一格不是装饰：确认文案说"正在跑的任务会停"，而**没改**的时候停掉用户的任务
    ///    是纯损失 —— 用户在面板里选中**同一个目录**是很容易发生的
    ///    （尤其是"我只是想看看现在用的是哪个"）。
    Unchanged { dir: String },
    /// 没改成：这是**可直接显示给用户的那句话**。
    Failed { message: String },
}

impl DownloadDirChange {
    /// 成功那支的**固定标题**（不含路径）。
    ///
    /// ⚠️ 别往这里塞路径、也别塞批次号 / 时间戳之类**长度不受控**的东西：
    ///    它的全部意义就是"**长度固定**"（见下面 `headline` 的注释）。
    ///
    /// 上游 `DownloadDirChange.changedTitle`。
    const CHANGED_TITLE: &'static str = "下载目录已改（内核已按新目录重启）";

    /// 恢复默认那支的**固定全文**（本来就短，没有路径可拆）。
    ///
    /// 上游 `DownloadDirChange.restoredDefault`。
    const RESTORED_DEFAULT: &'static str = "已恢复默认下载目录（内核已按默认目录重启）";

    /// 没改动那一支的固定全文。
    ///
    /// 上游 `DownloadDirChange.headline` 的 `.unchanged` 支。
    const UNCHANGED: &'static str = "下载目录没有变化，没有重启内核";

    /// **这条回执的第一行**：固定短的标题，**任何情况下都不含路径**。
    ///
    /// ⚠️ `dir` 在这里**只用来区分"改了"与"恢复默认"两个格子**，绝不参与拼字符串
    ///    —— 这是一次真实布局事故的回归判据（见本文件那段"高度不许由外部文本决定"）。
    ///    要显示路径请用 [`Self::path_detail`]。
    ///
    /// ⚠️ `Failed` 那一格**照登原文**：那可能是内核的失败原因，也可能是写盘失败的系统
    ///    错误文本，两者都**不许**加工、**不许**截断成"标题"（"失败原文必须完整可见"
    ///    是硬要求）。它同样是**外部文本**，高度由**调用方**统一封顶 ——
    ///    但封的是**高度、不是内容**，全文始终都在，没有截断。
    ///    ⚠️ 别在值这一层替它做长度限制：那是**丢证据**，不是排版。
    ///
    /// 上游 `DownloadDirChange.headline`。
    pub fn headline(&self) -> String {
        match self {
            DownloadDirChange::Changed { dir } => {
                if dir.is_empty() {
                    Self::RESTORED_DEFAULT.to_string()
                } else {
                    Self::CHANGED_TITLE.to_string()
                }
            }
            DownloadDirChange::Unchanged { .. } => Self::UNCHANGED.to_string(),
            DownloadDirChange::Failed { message } => message.clone(),
        }
    }

    /// **要单独占一行显示的路径**；`None` = 这条回执没有路径行可显示。
    ///
    /// ⚠️ 只有 `Changed` 且 `dir` **非空**时才有值：空串是"恢复默认"，它不是路径
    ///    （画一个空行在界面上读起来与"这一行坏了"分不开）。
    ///    其余两支**不许拆**：失败那句是内核/系统的原文，拆开就等于重排用户的证据。
    ///
    /// ⚠️ 这里**原样**返回路径：截断是**调用方**的事（单行 + 中间截断 + 悬停看全文），
    ///    值这一层不许自己动手 —— 一旦在这里截，"悬停看完整路径"这条出口就没了。
    ///
    /// 上游 `DownloadDirChange.pathDetail`。
    pub fn path_detail(&self) -> Option<String> {
        match self {
            DownloadDirChange::Changed { dir } if !dir.is_empty() => Some(dir.clone()),
            _ => None,
        }
    }

    /// 这条回执**完整的一句话**。**逐字保持布局事故修复前的样子**。
    ///
    /// ⚠️ 两个落点渲染的都是 `headline` + `path_detail` 那**两行** —— 把含完整路径的
    ///    整句铺在一行里，正是那次"底部整条被顶出窗口"的根因。那它为什么还在：
    ///      · 它是**防回归的锚**：四支的措辞被逐字钉着，谁改一个字就红；
    ///      · 它是这个结果**完整的一句话**，属于对外语义。
    ///
    /// ⚠️ 它**由 `headline` 与 `path_detail` 组合而成**，不是另抄一份字面量：
    ///    没路径 ⇒ 标题就是全文（这一支两处不可能分叉）；有路径 ⇒ 把路径填进那句话。
    ///    ⚠️ 但**别指望这条来守"标题里混进了路径"**：这一支用的是 `path_detail`、
    ///    根本不经过 `headline`，所以标题被改坏时它照样是绿的 ——
    ///    守那件事的是 `theHeadlineIsFixedNoMatterHowLongThePathIs`。
    ///
    /// 上游 `DownloadDirChange.noticeText`。
    pub fn notice_text(&self) -> String {
        match self {
            DownloadDirChange::Changed { .. } => match self.path_detail() {
                Some(path) => format!("下载目录已改为 {path}（内核已按新目录重启）"),
                None => self.headline(),
            },
            DownloadDirChange::Unchanged { .. } | DownloadDirChange::Failed { .. } => {
                self.headline()
            }
        }
    }

    /// 这一行说的是"没成"吗（调用方据此选图标与颜色）。**有单测**。
    ///
    /// 上游 `DownloadDirChange.isFailure`。
    pub fn is_failure(&self) -> bool {
        matches!(self, DownloadDirChange::Failed { .. })
    }
}

// ---------------------------------------------------------------------------
// 测试（先写测试：它们会先红，见任务 5 的步骤 2）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游两份（逐条对位，每条测试上方点名它对应上游的哪一条）：
    //!   · `macos/Tests/BenagenCoreKitTests/AppPreferencesTests.swift`
    //!   · `macos/Tests/BenagenCoreKitTests/DownloadDirectoryTests.swift`
    //!
    //! ⚠️ 本文件所有碰文件系统的用例都只碰 `std::env::temp_dir()` 下的临时目录
    //!    —— 用户真实的下载目录是**人类伙伴在用的现场**，测试碰它 = 破坏现场。

    use super::{
        AppPreferences, DiagnosticsToggle, DownloadDirChange, DownloadDirectory,
        VerboseLoggingChange,
    };
    use std::path::{Path, PathBuf};

    // -----------------------------------------------------------------------
    // 偏好本身（`AppPreferencesTests.swift`）
    // -----------------------------------------------------------------------

    /// 上游 `anUnconfiguredPreferenceIsTheEmptyString`。
    #[test]
    fn an_unconfigured_preference_is_the_empty_string() {
        assert_eq!(AppPreferences::empty().download_dir, "");
        assert_eq!(AppPreferences::new("").download_dir, "");
        assert!(!AppPreferences::empty().is_configured());
    }

    /// 上游 `aConfiguredPreferenceKnowsIt`。
    #[test]
    fn a_configured_preference_knows_it() {
        assert!(AppPreferences::new("/Volumes/Data/交付").is_configured());
        assert_eq!(
            AppPreferences::new("/Volumes/Data/交付").download_dir,
            "/Volumes/Data/交付"
        );
    }

    /// 上游 `roundTripOfTheConfiguredAndUnconfiguredPreference`。
    #[test]
    fn round_trip_of_the_configured_and_unconfigured_preference() {
        let configured = AppPreferences::new("/Volumes/Data/交付/有 空格的目录");
        assert_eq!(
            AppPreferences::parse(&configured.serialized().expect("序列化不得失败")),
            configured
        );

        assert_eq!(
            AppPreferences::parse(&AppPreferences::empty().serialized().expect("序列化不得失败")),
            AppPreferences::empty(),
            "空串（未配置）也要能原样往返 —— 它是「不传 --download-dir」的判据"
        );
    }

    /// 上游 `garbageIsTreatedAsUnconfigured`。
    ///
    /// 坏文件 ⇒ **当未配置**（不抛、不崩、更不该把应用拦在启动之外）。
    /// 本函数**没有 `Result`** —— 这正是判据。
    #[test]
    fn garbage_is_treated_as_unconfigured() {
        assert_eq!(AppPreferences::parse(""), AppPreferences::empty(), "空串");
        assert_eq!(
            AppPreferences::parse("not json at all"),
            AppPreferences::empty(),
            "非 JSON"
        );
        assert_eq!(
            AppPreferences::parse("[1,2,3]"),
            AppPreferences::empty(),
            "顶层不是对象"
        );
        assert_eq!(
            AppPreferences::parse(r#"{"version":2,"download_dir":"/tmp/x"}"#),
            AppPreferences::empty(),
            "version 不认识"
        );
        assert_eq!(
            AppPreferences::parse(r#"{"download_dir":"/tmp/x"}"#),
            AppPreferences::empty(),
            "没有 version"
        );
        assert_eq!(
            AppPreferences::parse(r#"{"version":"1","download_dir":"/tmp/x"}"#),
            AppPreferences::empty(),
            "version 不是数"
        );
        assert_eq!(
            AppPreferences::parse(r#"{"version":1,"download_dir":5}"#),
            AppPreferences::empty(),
            "目录不是字符串"
        );
        assert_eq!(
            AppPreferences::parse(r#"{"version":1}"#),
            AppPreferences::empty(),
            "没有 download_dir ⇒ 未配置"
        );
    }

    /// 上游 `thePreferencesFileShapeUsesTheSpecifiedFieldNames`。
    #[test]
    fn the_preferences_file_shape_uses_the_specified_field_names() {
        // 形状：`{"version": 1, "download_dir": "/abs/path"}`，键名逐字。
        let text = AppPreferences::new("/Volumes/Data/交付")
            .serialized()
            .expect("序列化不得失败");

        assert!(text.contains("\"version\""));
        assert!(text.contains("\"download_dir\""));
        assert!(!text.contains("downloadDir"));
        assert!(text.contains("/Volumes/Data/交付"), "路径原样写出去（含中文）");
    }

    /// 🔴 **缺字段 = `false`**，而且 **`version` 一个字都不许动**
    /// （递增会让装了旧版程序的客户的下载目录被清空 —— 规格 §2.4 / F6）。
    ///
    /// 判别力：把 `VERSION` 改成 2 ⇒ 第一条立刻红；把 `parse` 里那句
    /// `as_bool().unwrap_or(false)` 删掉 ⇒ 第二条红；把 `serialized` 里那一格删掉 ⇒
    /// 最后那条往返红。
    #[test]
    fn a_preference_without_the_verbose_field_reads_as_off_and_the_version_stays() {
        assert_eq!(AppPreferences::VERSION, 1, "递增版本号会让旧客户端的下载目录被清空");

        let old = AppPreferences::parse(r#"{"version": 1, "download_dir": "/tmp/x"}"#);
        assert_eq!(old.download_dir, "/tmp/x", "旧字段不许丢");
        assert!(!old.verbose_logging, "缺字段 = 没开详细日志");

        // 反方向：写着一格时要真的读进来（否则上面那条可以靠"永远 false"变绿）。
        let written = AppPreferences::parse(
            r#"{"version": 1, "download_dir": "/tmp/x", "verbose_logging": true}"#,
        );
        assert!(written.verbose_logging);

        // 而且它要能原样往返（`serialized` 也得带上那一格）。
        let on = AppPreferences::new("/tmp/x").setting_verbose_logging(true);
        assert_eq!(
            AppPreferences::parse(&on.serialized().expect("序列化不得失败")),
            on
        );
    }

    /// ⚠️ **两格互不覆盖**：改目录不许把详细日志带跑，反之亦然。
    ///
    /// 判别力：把任一 `setting_*` 写成走 `AppPreferences::new`（它把 verbose 置回 `false`），
    /// 这一条立刻红 —— 而真机上的表现是客户排查到一半换了个下载目录，
    /// 诊断日志**静默地退回普通档**（文件还在长，只是不再记那每一次往返了）。
    #[test]
    fn the_two_settings_never_overwrite_each_other() {
        let base = AppPreferences::new("/data/交付").setting_verbose_logging(true);
        assert!(base.setting_download_dir("/data/新的").verbose_logging, "改目录把开关带跑了");
        assert_eq!(base.setting_download_dir("/data/新的").download_dir, "/data/新的");
        assert_eq!(
            base.setting_verbose_logging(false).download_dir,
            "/data/交付",
            "改开关把目录带跑了"
        );
        // 「恢复默认」那条路同样是"只动目录那一格"。
        assert!(base.setting_download_dir("").verbose_logging);
    }

    /// 上游 `settingADirectoryKeepsThePathVerbatimExceptForSurroundingWhitespace`。
    ///
    /// ⚠️ 有意偏离的判据：只去掉**首尾空白**，不做"规范化路径"。理由是这条路径最终要
    ///    原样进内核的 argv（`--download-dir`）：软链接、`..`、结尾的 `/` 都是
    ///    **内核与文件系统的语义**，壳替它解释一遍只会在两边分叉 ——
    ///    而分歧的代价是**文件下到别的地方去**。
    #[test]
    fn setting_a_directory_keeps_the_path_verbatim_except_for_surrounding_whitespace() {
        assert_eq!(
            AppPreferences::new("  /Volumes/Data/交付  ").download_dir,
            "/Volumes/Data/交付"
        );
        assert_eq!(
            AppPreferences::new("/Volumes/Data/../Data/交付/").download_dir,
            "/Volumes/Data/../Data/交付/",
            "`..` 与结尾的 `/` 一个字都不许动"
        );
        assert_eq!(
            AppPreferences::new("   ").download_dir,
            "",
            "只有空白 = 未配置"
        );
    }

    /// 上游 `clearingTheDownloadDirectoryGoesBackToTheUnconfiguredState`。
    #[test]
    fn clearing_the_download_directory_goes_back_to_the_unconfigured_state() {
        let cleared = AppPreferences::new("/Volumes/Data/交付").setting_download_dir("");
        assert_eq!(cleared, AppPreferences::empty());
        assert!(!cleared.is_configured());
    }

    // -----------------------------------------------------------------------
    // 该不该传 `--download-dir`（`DownloadDirectoryTests.swift`）
    // -----------------------------------------------------------------------

    /// 上游 `anUnconfiguredPreferencePassesNoDownloadDir`。
    #[test]
    fn an_unconfigured_preference_passes_no_download_dir() {
        assert_eq!(DownloadDirectory::argument(&AppPreferences::empty()), None);
        assert_eq!(DownloadDirectory::argument(&AppPreferences::new("")), None);
        assert_eq!(
            DownloadDirectory::argument(&AppPreferences::new("   ")),
            None,
            "只有空白 = 未配置（`AppPreferences::new` 的归一化）"
        );
    }

    /// 上游 `noUnconfiguredPreferenceEverPutsAPathInTheArgv`。
    ///
    /// ⚠️ 审查 I-2 之后的形状：它走的是**真正的 argv 构造器**
    ///    （[`DownloadDirectory::core_arguments_for`] → `client::core_arguments`），
    ///    而不是它的输入侧。**旧版**只遍历 `argument()` 断言 `None`，
    ///    与 `an_unconfigured_preference_passes_no_download_dir` 输入与断言逐字相同
    ///    （零新增判别力），而且 E-5 那条纪律在仓库里**零覆盖** —— 已改掉。
    ///
    /// 判别力：把 [`DownloadDirectory::core_arguments_for`] 改成"未配置就传一个
    /// 自己编的默认路径"（= E-5 明禁的那种静默分叉），这一条立刻红。
    #[test]
    fn no_unconfigured_preference_ever_puts_a_path_in_the_argv() {
        for prefs in [
            AppPreferences::empty(),
            AppPreferences::new(""),
            AppPreferences::new("   "),
        ] {
            let argv = DownloadDirectory::core_arguments_for(&prefs, None);
            assert!(
                argv.is_empty(),
                "未配置 ⇒ argv 里一个参数都不许有（不许自己编一个「和内核默认一样」的路径），\
                 实际 {argv:?}"
            );
            assert!(
                !argv.iter().any(|arg| arg == "--download-dir"),
                "那个 flag 根本不该出现：{argv:?}"
            );
            assert!(
                !argv.iter().any(|arg| arg.contains('/')),
                "argv 里不许出现任何路径形式的参数：{argv:?}"
            );
        }
    }

    /// 上游 `theCriterionEndsUpInTheRealArgv`。
    ///
    /// ⚠️ **本波次曾经把这条记成"与上一条合并"**（审查 I-2 指出：那样记账掩盖了
    /// "一条判据没了"）。它现在是一条**独立的**用例，判据与上游逐字相同：
    /// **未配置 ⇒ argv 里没有 `--download-dir`；已配置 ⇒ 逐字是那对参数**。
    ///
    /// 判别力：`core_arguments_for` 若漏了 flag（只推路径）、或把两者顺序写反、
    /// 或对空串也推一对参数，这里红。
    #[test]
    fn the_criterion_ends_up_in_the_real_argv() {
        assert_eq!(
            DownloadDirectory::core_arguments_for(&AppPreferences::empty(), None),
            Vec::<String>::new(),
            "未配置 ⇒ 一个参数都没有（`--download-dir` 这个键根本不出现）"
        );
        assert_eq!(
            DownloadDirectory::core_arguments_for(&AppPreferences::new("/data/交付"), None),
            vec!["--download-dir".to_string(), "/data/交付".to_string()]
        );
        // ⚠️ 顺带钉住这根缝**真的接到了** `client::core_arguments` 上（而不是另写了一份
        //    只认下载目录的拼装）：设置文件那一支也要原样透传。
        assert_eq!(
            DownloadDirectory::core_arguments_for(
                &AppPreferences::new("/data/交付"),
                Some(Path::new("/tmp/settings.json"))
            ),
            vec![
                "--download-dir".to_string(),
                "/data/交付".to_string(),
                "--settings".to_string(),
                "/tmp/settings.json".to_string(),
            ]
        );
    }

    /// 🔴 **开了才拼那个 flag，关了**不拼**（不是拼 `--log-level normal`）。
    #[test]
    fn the_verbose_flag_enters_the_argv_only_when_it_is_on() {
        let off = DownloadDirectory::core_arguments_for(&AppPreferences::new("D:\\交付"), None);
        assert!(!off.iter().any(|a| a == "--log-level"), "没开就不该出现：{off:?}");

        let on = DownloadDirectory::core_arguments_for(
            &AppPreferences::new("D:\\交付").setting_verbose_logging(true), None);
        assert!(on.windows(2).any(|w| w == ["--log-level", "verbose"]), "{on:?}");
    }

    /// 上游 `aConfiguredPreferencePassesTheDirectoryVerbatim`。
    #[test]
    fn a_configured_preference_passes_the_directory_verbatim() {
        let prefs = AppPreferences::new("/Volumes/Data/交付/有 空格的目录");
        assert_eq!(
            DownloadDirectory::argument(&prefs).as_deref(),
            Some("/Volumes/Data/交付/有 空格的目录"),
            "原样进 argv：壳不规范化"
        );
    }

    // -----------------------------------------------------------------------
    // 确认文案（三条后果）
    // -----------------------------------------------------------------------

    /// 上游 `theConfirmationNamesAllThreeConsequences`。
    #[test]
    fn the_confirmation_names_all_three_consequences() {
        let text = DownloadDirectory::confirmation("", "/Volumes/Data/交付");

        assert!(
            text.contains("正在跑的任务会停"),
            "① 重启内核 ⇒ 正在跑的任务会停"
        );
        assert!(
            text.contains("新目录里没有记录的文件会重新校验"),
            "② 状态文件就在下载目录里"
        );
        assert!(
            text.contains("旧目录里的文件不会被删、也不会被搬走"),
            "③ 不删、不搬"
        );
        assert!(text.contains("/Volumes/Data/交付"), "要说清楚改到哪去");
    }

    /// 上游 `theConfirmationNeverClaimsEveryFileWillBeReverified`。
    ///
    /// ⚠️ 用户可能选了一个**已经有状态文件**的目录（例如换回旧目录），那时状态会被内核
    ///    读回来 ⇒ 一个字节都不用重新校验。文案断言"一定会重新校验"就是在**说假话**，
    ///    而用户会照着它做决定。
    #[test]
    fn the_confirmation_never_claims_every_file_will_be_reverified() {
        let text = DownloadDirectory::confirmation("/old", "/new");
        assert!(!text.contains("一定会重新校验"));
        assert!(!text.contains("所有文件"));
        assert!(!text.contains("全部重新校验"));
    }

    /// 上游 `theConfirmationSaysWhatTheCurrentDirectoryIs`。
    #[test]
    fn the_confirmation_says_what_the_current_directory_is() {
        // 两边都要说：只说"改成什么"的话，用户在**换回旧目录**时看不出自己现在在哪。
        let text = DownloadDirectory::confirmation("/Volumes/A", "/Volumes/B");
        assert!(text.contains("/Volumes/A"));
        assert!(text.contains("/Volumes/B"));

        // 未配置那一侧显示的是"默认"，而不是一个空串（空串在界面上读起来像"坏了"）。
        let from_default = DownloadDirectory::confirmation("", "/Volumes/B");
        assert!(from_default.contains("默认"));
        assert!(!from_default.contains("「」"), "不许出现一对空引号");
    }

    /// 上游 `theDisplayFallsBackToTheDefaultLabelWhenUnconfigured`。
    #[test]
    fn the_display_falls_back_to_the_default_label_when_unconfigured() {
        assert_eq!(DownloadDirectory::display("/Volumes/Data/交付"), "/Volumes/Data/交付");
        assert!(DownloadDirectory::display("").contains("默认"));
        assert!(!DownloadDirectory::display("").is_empty());
        assert_eq!(
            DownloadDirectory::display(""),
            "默认（~/Downloads/Benagen，由内核决定）",
            "整句逐字（含全角括号与逗号）"
        );
    }

    // -----------------------------------------------------------------------
    // 回执（设置窗口里那一行）
    // -----------------------------------------------------------------------

    /// 上游 `eachOutcomeSaysWhatHappened`。
    #[test]
    fn each_outcome_says_what_happened() {
        // 🔴 这三句是**壳自己写的**：内核不会为一次成功的重启主动说话。而"我按了确认之后
        //    发生了什么"必须有个落点 —— 否则那个确认框就像按了个寂寞。
        let changed = DownloadDirChange::Changed {
            dir: "/Volumes/Data/交付".to_string(),
        };
        assert!(changed.notice_text().contains("/Volumes/Data/交付"));
        assert!(changed.notice_text().contains("重启"));
        assert!(DownloadDirChange::Changed {
            dir: String::new()
        }
        .notice_text()
        .contains("默认"), "恢复默认那一格不能说成'已改为 '（一个空路径）");
        assert!(
            DownloadDirChange::Unchanged {
                dir: "/x".to_string()
            }
            .notice_text()
            .contains("没有"),
            "没改动就要说没改动（用户得知道自己那次点击是有结果的）"
        );

        // 失败：**原文照登**，不加工、不加前缀。
        let failure = DownloadDirChange::Failed {
            message: "内核重启失败：内核无响应（等待超过 5 秒）".to_string(),
        };
        assert_eq!(
            failure.notice_text(),
            "内核重启失败：内核无响应（等待超过 5 秒）"
        );
        assert!(failure.is_failure());
        assert!(!changed.is_failure());
        assert!(!DownloadDirChange::Unchanged {
            dir: "/x".to_string()
        }
        .is_failure());
    }

    // -----------------------------------------------------------------------
    // 常驻提示行的**高度不许由外部文本决定**（一次真实的布局事故）
    //
    // 现场：改完下载目录、内核按新目录重启之后，**文件页底栏**与**侧边栏的批次摘要**
    // 一起从窗口里消失；"把窗口拉高"或"点「恢复默认」"就恢复。根因在成功那一支把
    // **用户选的完整路径**塞进了那条**常驻**回执，而那一行没有行数上限 ⇒
    // **路径越长这一行越高** ⇒ 内容比窗口高 ⇒ 锚在底边的两处一起被裁掉。
    //
    // 修法的判据落在**这里**（视图不单测是本项目的硬约束）：把"标题"与"路径"拆成两个
    // **纯计算**属性 —— 标题**任何路径下都一样长**，路径单独一行、由调用方去截断。
    // -----------------------------------------------------------------------

    /// 一条**很长**的路径（比任何一句标题都长得多）。守卫用它当输入。
    const A_VERY_LONG_PATH: &str = "/Volumes/数据盘_2026/交付给客户/贝纳基因/2026-09-18/批次_C24-8_×_25WS024/原始数据/测序下机/再一次深层/还要更深";

    /// 上游 `theHeadlineIsFixedNoMatterHowLongThePathIs`（**根因的回归守卫**）。
    #[test]
    fn the_headline_is_fixed_no_matter_how_long_the_path_is() {
        let short = DownloadDirChange::Changed {
            dir: "/x".to_string(),
        }
        .headline();
        let long = DownloadDirChange::Changed {
            dir: A_VERY_LONG_PATH.to_string(),
        }
        .headline();
        assert_eq!(
            short, long,
            "标题不许随路径变化：短路径 {short} / 长路径 {long}"
        );

        // 再把"路径没漏进去"钉死（上面那条只要有人给两支都塞同一段路径就恒真了）。
        assert!(!long.contains('/'), "标题里不许出现任何路径片段：{long}");
        assert!(!long.contains("数据盘_2026"), "标题里不许出现路径里的目录名：{long}");
        assert!(!long.contains(A_VERY_LONG_PATH));
    }

    /// 上游 `theHeadlineSaysWhatHappenedWithoutThePath`。
    #[test]
    fn the_headline_says_what_happened_without_the_path() {
        assert!(
            DownloadDirChange::Changed {
                dir: "/Volumes/A".to_string()
            }
            .headline()
            .contains("重启"),
            "改成功那一支要说内核重启过"
        );
        assert!(
            DownloadDirChange::Changed {
                dir: String::new()
            }
            .headline()
            .contains("默认"),
            "恢复默认那一支要说回到默认（不许说成「已改为 」一个空路径）"
        );
        assert!(
            DownloadDirChange::Unchanged {
                dir: "/x".to_string()
            }
            .headline()
            .contains("没有"),
            "没改动就要说没改动"
        );
        assert_eq!(
            DownloadDirChange::Failed {
                message: "内核重启失败：内核无响应（等待超过 5 秒）".to_string()
            }
            .headline(),
            "内核重启失败：内核无响应（等待超过 5 秒）",
            "失败那一支是**内核原文逐字**：标题就是全文，壳不加工、不加前缀"
        );
    }

    /// 上游 `thePathIsCarriedSeparatelyAndOnlyWhenThereIsOne`。
    #[test]
    fn the_path_is_carried_separately_and_only_when_there_is_one() {
        // 路径单独交给调用方去渲染（拿到它做单行 + 中间截断 + 悬停看全文）。
        assert_eq!(
            DownloadDirChange::Changed {
                dir: "/Volumes/Data/交付".to_string()
            }
            .path_detail()
            .as_deref(),
            Some("/Volumes/Data/交付")
        );
        assert_eq!(
            DownloadDirChange::Changed {
                dir: A_VERY_LONG_PATH.to_string()
            }
            .path_detail()
            .as_deref(),
            Some(A_VERY_LONG_PATH),
            "路径**原样**传出去（截断是调用方的事，值这一层不许自己动手）"
        );

        // 其余各支都没有"路径"这一行可显示。
        assert_eq!(
            DownloadDirChange::Changed {
                dir: String::new()
            }
            .path_detail(),
            None,
            "恢复默认 = 空串 ⇒ **没有路径行**（空行读起来像「这一行坏了」）"
        );
        assert_eq!(
            DownloadDirChange::Unchanged {
                dir: "/Volumes/A".to_string()
            }
            .path_detail(),
            None,
            "没改动 ⇒ 没有路径行"
        );
        assert_eq!(
            DownloadDirChange::Failed {
                message: "这个位置不存在：/Volumes/没了".to_string()
            }
            .path_detail(),
            None,
            "失败那句是内核/系统的原文 ⇒ 作为一个整体显示，不许被拆成「路径」"
        );
    }

    /// 上游 `theNoticeTextIsWordForWordWhatItUsedToBe`。
    ///
    /// ⚠️ 这一条**刻意逐字**（不是 `contains`）：布局事故的修复只许**拆结构**、
    ///    不许**改措辞**。四支各钉一条。
    #[test]
    fn the_notice_text_is_word_for_word_what_it_used_to_be() {
        assert_eq!(
            DownloadDirChange::Changed {
                dir: "/Volumes/Data/交付".to_string()
            }
            .notice_text(),
            "下载目录已改为 /Volumes/Data/交付（内核已按新目录重启）"
        );
        assert_eq!(
            DownloadDirChange::Changed {
                dir: String::new()
            }
            .notice_text(),
            "已恢复默认下载目录（内核已按默认目录重启）"
        );
        assert_eq!(
            DownloadDirChange::Unchanged {
                dir: "/x".to_string()
            }
            .notice_text(),
            "下载目录没有变化，没有重启内核"
        );
        assert_eq!(
            DownloadDirChange::Failed {
                message: "内核重启失败：内核无响应（等待超过 5 秒）".to_string()
            }
            .notice_text(),
            "内核重启失败：内核无响应（等待超过 5 秒）"
        );

        // 而且它必须**仍然由标题与路径组合而成**（不是又抄了一份字面量）。
        let with_path = DownloadDirChange::Changed {
            dir: A_VERY_LONG_PATH.to_string(),
        };
        assert!(
            with_path
                .notice_text()
                .contains(with_path.path_detail().unwrap_or_default().as_str()),
            "路径必须进那句完整的话"
        );
        assert_eq!(
            DownloadDirChange::Changed {
                dir: String::new()
            }
            .notice_text(),
            DownloadDirChange::Changed {
                dir: String::new()
            }
            .headline(),
            "恢复默认：没有路径可拆 ⇒ 全文就是标题（两处不可能分叉）"
        );
    }

    /// 上游 `theSectionNoteExplainsWhatUnconfiguredMeans`。
    #[test]
    fn the_section_note_explains_what_unconfigured_means() {
        // ⚠️ 它是本功能唯一反直觉的地方（"没有设置"也是一种设置），而这一句是用户
        //    唯一能读到它的地方 —— 别在改文案时把它删掉。
        assert!(DownloadDirectory::SECTION_NOTE.contains("默认"));
        assert!(DownloadDirectory::SECTION_NOTE.contains("重启"));
    }

    /// 上游 `theSectionNoteWarnsAboutNetworkVolumes`。
    #[test]
    fn the_section_note_warns_about_network_volumes() {
        // ⚠️ 用户可以一步操作就把下载目录指到 NFS/SMB 上，而内核每次列目录都要
        //    持着锁对每个路径 stat 一遍 —— 网络盘的 stat 是秒级 ⇒ 内核的同步读循环停摆。
        //    形态是**变慢**、不是变错，所以**不改内核**，只把这件事**说出来**。
        assert!(
            DownloadDirectory::SECTION_NOTE.contains("网络盘"),
            "要在设置里说清楚网络盘会变慢：{}",
            DownloadDirectory::SECTION_NOTE
        );
        assert!(
            DownloadDirectory::SECTION_NOTE.contains("本机磁盘"),
            "光说「网络盘」不够 —— 要说出该选什么"
        );
    }

    // -----------------------------------------------------------------------
    // 「详细日志」那一个勾选框（规格 §2.4）
    // -----------------------------------------------------------------------

    /// 标签与说明都是**非空**的，而且说明把该说的三件事都说了。
    ///
    /// 判别力：把说明删成一句"详细日志"（或空串），第二、三条立刻红 ——
    /// 而真机上的表现是**用户打开了详细档然后再也没关过**（他不知道要关、
    /// 也不知道这一下会把正在跑的任务停掉）。
    #[test]
    fn the_diagnostics_toggle_explains_what_it_does_and_what_it_costs() {
        assert_eq!(DiagnosticsToggle::LABEL, "详细日志", "规格 §2.4 逐字");
        let note = DiagnosticsToggle::NOTE;
        assert!(note.contains("排查"), "要说清它是干什么用的：{note}");
        assert!(note.contains("关"), "要提醒用完关掉（详细档一直在记）：{note}");
        assert!(
            note.contains("重启内核"),
            "改了会重启内核 ⇒ 正在跑的任务会停，这件事必须在按之前就说：{note}"
        );
    }

    /// 四种回执各自说什么，而且**一句都不许提"下载目录"**。
    ///
    /// 🔴 最后那条断言是这条用例的**全部要点**：勾选框那一格改的是一个开关，
    ///    一个目录都没动。复用 `DownloadDirChange` 的三句（它们把主词写死成
    ///    "下载目录"）会让用户在常驻回执上读到一句假话 —— 而那句话是**绿的**
    ///    （没有任何东西会因为"文案说错了对象"而变红）。
    #[test]
    fn the_verbose_receipts_never_talk_about_the_download_directory() {
        let opened = VerboseLoggingChange::Changed { on: true };
        assert!(opened.headline().contains("详细日志"));
        assert!(opened.headline().contains("打开"));
        assert!(opened.headline().contains("重启"), "要说出内核重启过");
        assert!(!opened.is_failure());
        assert_eq!(opened.path_detail(), None, "这一格没有路径可显示");
        assert_eq!(
            opened.notice_text(),
            opened.headline(),
            "没有路径可拆 ⇒ 全文就是标题（两处不可能分叉）"
        );

        // 关掉那一支与打开那一支**必须说得出区别**（说成同一句 = 用户不知道自己那一下
        // 把它改成了什么）。
        let closed = VerboseLoggingChange::Changed { on: false };
        assert!(closed.headline().contains("关闭"));
        assert_ne!(closed.headline(), opened.headline());

        // 没改动：内核没重启（那句"正在跑的任务会停"说的是**真的改了**的时候）。
        let unchanged = VerboseLoggingChange::Unchanged { on: true };
        assert!(unchanged.headline().contains("没有"));
        assert!(!unchanged.is_failure());

        // 失败：**原文照登**，不加工、不加前缀。
        let failed = VerboseLoggingChange::Failed {
            message: "内核重启失败：内核无响应（等待超过 5 秒）".to_string(),
        };
        assert_eq!(failed.headline(), "内核重启失败：内核无响应（等待超过 5 秒）");
        assert!(failed.is_failure());
        assert_eq!(failed.path_detail(), None);

        for text in [
            opened.headline(),
            closed.headline(),
            unchanged.headline(),
            failed.headline(),
        ] {
            assert!(
                !text.contains("下载目录"),
                "这一格改的是开关、不是目录：{text}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 目录可用性：不可写 / 不存在 ⇒ 在**确认之前**就报错
    // -----------------------------------------------------------------------

    /// 一个用完就删的临时目录。**本文件所有碰盘的东西都建在它下面**。
    struct TempDirectory {
        url: PathBuf,
    }

    impl TempDirectory {
        fn new() -> TempDirectory {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let url = std::env::temp_dir().join(format!(
                "benagen-download-dir-tests-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&url).expect("建临时目录");
            TempDirectory { url }
        }

        fn path(&self) -> String {
            self.url.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.url);
        }
    }

    /// 上游 `aWritableExistingDirectoryPassesTheCheck`。
    #[test]
    fn a_writable_existing_directory_passes_the_check() {
        let dir = TempDirectory::new();
        assert_eq!(DownloadDirectory::check(&dir.path()), None);
    }

    /// 上游 `aMissingDirectoryIsRejectedWithAnExplanation`。
    #[test]
    fn a_missing_directory_is_rejected_with_an_explanation() {
        let dir = TempDirectory::new();
        let missing = format!("{}/还不存在/再深一层", dir.path());

        let problem = DownloadDirectory::check(&missing);
        assert!(
            problem.is_some(),
            "不存在的目录不许放行 —— 否则要等重启完才发现"
        );
        assert!(
            problem.as_deref().unwrap_or("").contains(&missing),
            "要把是哪个路径说出来：{}",
            problem.unwrap_or_default()
        );
    }

    /// 上游 `aPlainFileIsRejected`。
    #[test]
    fn a_plain_file_is_rejected() {
        let dir = TempDirectory::new();
        let file = format!("{}/我是一个文件", dir.path());
        std::fs::write(&file, b"x").expect("放一个文件");

        let problem = DownloadDirectory::check(&file);
        assert!(
            problem.is_some(),
            "选中的是一个文件 ⇒ 报错，不许当成目录用"
        );
        assert!(problem.as_deref().unwrap_or("").contains(&file));
    }

    /// 上游 `anUnwritableDirectoryIsRejected`。
    ///
    /// 🔴 **本波次的加强版**（控制者裁决 R-14）：判据从"`readonly()` 属性位"改成
    /// **探针写**（见 `probe_writable`），所以这条用例现在真的**造一个写不进去的目录**
    /// 来验它。
    ///
    /// ⚠️ **跳过口径**（如实写清，不假装通过）：`set_readonly(true)` 在 Unix 上把目录
    ///    变成 `0o555`（对非 root 真的写不进去），而在 **Windows 上它只设
    ///    `FILE_ATTRIBUTE_READONLY`，那个位根本不阻止在目录里建文件** ——
    ///    所以这条用例在交付平台上**会跳过**。跳过判据由下面那段**独立探针**给
    ///    （**不是**被测代码那条路：拿被测的 `check` 当跳过判据的话，一个恒返回
    ///    "可写"的实现会让这条用例静默变成空跑）。
    ///    ⇒ **这条判据在 Windows 上的真实强度**：靠的是"探针写与用户真正的写是同一个
    ///    操作"这个**结构性**事实，而不是靠这条用例证明过 —— 要证明它，得在真机上手改
    ///    目录 ACL（需要 winapi，而本 crate 不许引依赖）。**这是已知的、有意接受的边界。**
    #[test]
    fn an_unwritable_directory_is_rejected() {
        let dir = TempDirectory::new();
        let locked = format!("{}/只读", dir.path());
        std::fs::create_dir_all(&locked).expect("建只读目录");

        let mut permissions = std::fs::metadata(&locked).expect("stat").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&locked, permissions).expect("设只读");

        // 独立探针（不经被测代码）：这个环境到底还能不能往里面写？
        let environment_still_writable = {
            let probe = std::path::Path::new(&locked).join(".benagen-test-guard-probe");
            let created = std::fs::File::create(&probe).is_ok();
            let _ = std::fs::remove_file(&probe);
            created
        };

        let problem = DownloadDirectory::check(&locked);

        // 恢复可写（**先恢复再断言**：断言失败时不该把现场留在只读状态，
        // `TempDirectory::drop` 的 `remove_dir_all` 也要能删掉它）。
        let mut permissions = std::fs::metadata(&locked).expect("stat").permissions();
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&locked, permissions);

        if environment_still_writable {
            // 以 root 跑（Unix）或目录的只读属性拦不住（Windows）⇒ 跳过。
            return;
        }

        assert!(problem.is_some(), "不可写的目录不许放行");
        assert!(problem.as_deref().unwrap_or("").contains(&locked));
    }

    /// 🔴 **新增**（控制者裁决 R-14）：探针**不留下残骸**。
    ///
    /// 探针写是"真的在用户选的目录里建一个文件"，所以"记得删掉"是这条判据的**一半**：
    /// 漏删的后果是在**客户的交付目录**里留下一个 `.benagen-write-probe-…` 垃圾文件，
    /// 而它不报错、也没人会发现。判别力：任何漏删的实现（成功路径或失败路径上）
    /// 都会让下面那句"目录里只有原来那一个文件"变红。
    #[test]
    fn the_write_probe_leaves_no_residue() {
        let dir = TempDirectory::new();
        std::fs::write(format!("{}/原有的文件", dir.path()), b"x").expect("放一个文件");

        // 可写目录：走的是"建成功 ⇒ 删掉 ⇒ 返回 nil"那条路。
        assert_eq!(DownloadDirectory::check(&dir.path()), None);

        let mut names: Vec<String> = std::fs::read_dir(&dir.url)
            .expect("读目录")
            .map(|entry| {
                entry
                    .expect("目录项")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["原有的文件".to_string()],
            "探针文件必须被清掉：目录里除了原有内容不许有别的"
        );
    }

    /// 上游 `theUnconfiguredPathIsNotCheckedBecauseTheKernelOwnsItsDefault`。
    #[test]
    fn the_unconfigured_path_is_not_checked_because_the_kernel_owns_its_default() {
        // 「恢复默认」的目标是**空串** —— 它不是一个路径，壳也无从检查
        //（内核会用自己的默认值，并在启动时建它）。
        assert_eq!(DownloadDirectory::check(""), None);
    }
}
