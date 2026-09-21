//! settings —— 客户可调下发参数的校验、持久化与 aria2 选项映射。
//!
//! 逐条移植自 `downloader/internal/settings/{settings.go,settings_test.go}`（Go 侧已冻结）。
//! 15 条测试与 Go 一一对应，对应表见
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-3-report.md`。
//!
//! ⚠️ **一处有意偏离 Go**：[`Settings::global_options`] 在"不限速"时**显式下发 `"0"`**
//! 而不是省略该键（Go 省略）。理由与实测见该方法的注释，契约 §7 记了这一条。
//! 断点续传（`-c`）不在这里——它恒开，不做开关。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 限速上界：100000 MB/s（100 GB/s）。
///
/// **此上界为防溢出而设，规格未规定限速上界**（行为契约 §2.2 / `#15`）：
/// 下发时该值会乘以 1024*1024 换算成字节/秒，实测 2^50 通过校验后
/// `max-overall-download-limit` 会回绕成 `"0"`——静默变成「不限速」；
/// 其他超大值回绕成负数则会被 aria2 拒绝、引擎启动失败。
/// 两者都被「不得静默失效」明令禁止。
pub const LIMIT_MBPS_MAX: i64 = 100_000;

/// 客户可调的七项（范围见规格 §6）。
///
/// ⚠️ 字段顺序即 `save_to` 落盘的键顺序：Go 用 `json.MarshalIndent`，
/// 顺序由结构体声明顺序决定；`#[derive(Serialize)]` 同样按声明顺序输出。
/// 字段名与 Go 的 `json:` 标签逐字一致（`min_split_size`/`limit_mbps` 等），
/// 故无需 `#[serde(rename)]`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// -j，1–64
    pub parallel: i32,
    /// -x，1–16
    pub connections: i32,
    /// -s，1–16
    pub splits: i32,
    /// -k，1M–100M
    pub min_split_size: String,
    /// 0 = 不限速
    pub limit_mbps: i64,
    /// 1–100
    pub max_tries: i32,
    /// 0–60 秒
    pub retry_wait: i32,
}

/// 规格 §6 逐字给出的默认值。
pub fn default_settings() -> Settings {
    Settings {
        parallel: 8,
        connections: 16,
        splits: 16,
        min_split_size: "20M".to_string(),
        limit_mbps: 0,
        max_tries: 3,
        retry_wait: 1,
    }
}

impl Settings {
    /// 检查每项是否在范围内。
    ///
    /// **唯一的输入闸门**（行为契约 §2.1）：所有下发前都必须先过它。
    /// 越界必须在**输入时**拦下——`-k` 这类值写错会让 aria2 直接启动失败，
    /// 那就成了「启动了才发现参数非法」。
    pub fn validate(&self) -> Result<(), String> {
        if self.parallel < 1 || self.parallel > 64 {
            return Err(format!("并行文件数必须在 1–64 之间，当前 {}", self.parallel));
        }
        if self.connections < 1 || self.connections > 16 {
            return Err(format!(
                "单文件连接数必须在 1–16 之间，当前 {}",
                self.connections
            ));
        }
        if self.splits < 1 || self.splits > 16 {
            return Err(format!("分片数必须在 1–16 之间，当前 {}", self.splits));
        }
        parse_size_mb(&self.min_split_size)?;
        // 上界 LIMIT_MBPS_MAX（100 GB/s）为防溢出而设，规格 §6 未规定限速上界：
        // 换算成字节/秒时会乘以 1024*1024，取值过大则乘法回绕——静默变成「不限速」（0）
        // 或回绕成负数被 aria2 拒绝、引擎启动失败。两者都是全局约束明令禁止的。
        if self.limit_mbps < 0 || self.limit_mbps > LIMIT_MBPS_MAX {
            return Err(format!(
                "限速必须在 0–{LIMIT_MBPS_MAX} MB/s 之间（上界为防溢出而设），当前 {}",
                self.limit_mbps
            ));
        }
        if self.max_tries < 1 || self.max_tries > 100 {
            return Err(format!("重试次数必须在 1–100 之间，当前 {}", self.max_tries));
        }
        if self.retry_wait < 0 || self.retry_wait > 60 {
            return Err(format!("重试间隔必须在 0–60 秒之间，当前 {}", self.retry_wait));
        }
        Ok(())
    }

    /// **逐任务**选项，在 `addUri` 时传入（契约 §2.4）——改完对之后新加的任务立即生效。
    pub fn per_task_options(&self) -> BTreeMap<String, String> {
        // min-split-size 下发的是**解析后的规范值**，不是原始串：Validate 接受 " 20m " 这类写法，
        // 但 aria2 收到带空格或小写后缀的串会直接启动失败——那就成了「启动了才发现参数非法」。
        // 这正是全局约束点名 -k 的原因。解析失败理论上不会发生（Validate 会先拦下），
        // 但兜底给默认值而不是静默省略该键：少下发一个选项同样是「启动了才发现」的老问题。
        let mut min_split_size = default_settings().min_split_size;
        if let Ok(n) = parse_size_mb(&self.min_split_size) {
            min_split_size = format!("{n}M");
        }
        BTreeMap::from([
            (
                "max-connection-per-server".to_string(),
                self.connections.to_string(),
            ),
            ("split".to_string(), self.splits.to_string()),
            ("min-split-size".to_string(), min_split_size),
            ("max-tries".to_string(), self.max_tries.to_string()),
            ("retry-wait".to_string(), self.retry_wait.to_string()),
        ])
    }

    /// **全局**选项，走 `changeGlobalOption`（契约 §2.5：即时生效）。
    ///
    /// ⚠️ **限速键恒插入，不限速时下发 `"0"`——绝不省略。**
    ///
    /// `aria2.changeGlobalOption` 是**部分更新**：对象里没出现的键**保持原值**。
    /// 实测（内嵌 aria2 1.37.0）：
    ///
    /// ```text
    /// changeGlobalOption {"max-overall-download-limit":"5242880"}  → getGlobalOption = '5242880'
    /// changeGlobalOption {"max-concurrent-downloads":"8"}         → getGlobalOption = '5242880'  ← 没被清掉
    /// changeGlobalOption {"max-overall-download-limit":"0"}       → getGlobalOption = '0'        ← 显式 0 才清得掉
    /// ```
    ///
    /// 省略只在本键**本来就是 0**（引擎刚起来）时才等价于"不限速"。在"改小 / 取消"这条路上
    /// 省略等于**什么都没做**：客户把限速从 5 MB/s 改回「不限速」，界面显示"已保存"、
    /// 引擎**继续限速**、没有任何一处报错——正是契约 §2.2/§2.5 与全局约束明令禁止的静默失效。
    ///
    /// ⚠️ **这是对 Go 的有意偏离**（`downloader/internal/settings/settings.go:107-116` 逐字相同：
    /// 它也是 `if s.LimitMBps > 0` 才插入）。Go 那样写不代表它对，而阶段 A 是修复成本最低的位置
    /// （壳还没写、契约刚冻结）。偏离记在契约 §7，与 `ExtractTo` 的私有临时名、`startOnPort`
    /// 的绝对化同例。钉住它的是 `global_options_sends_explicit_zero_when_unlimited`。
    pub fn global_options(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "max-concurrent-downloads".to_string(),
                self.parallel.to_string(),
            ),
            (
                "max-overall-download-limit".to_string(),
                // 0 表示不限速；`0 * 1024 * 1024` 就是 `"0"`
                (self.limit_mbps * 1024 * 1024).to_string(),
            ),
        ])
    }

    /// 写入文件（不存在则创建目录）。
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        // 对应 Go 的 filepath.Dir(path)：无父目录时 Go 建的是 "."（no-op），
        // 而 Rust 的 parent() 给出空路径，create_dir_all("") 会报错——故跳过空目录。
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        // Go 的 json.MarshalIndent(s, "", "  ")：两空格缩进、无尾随换行。
        // serde_json::to_string_pretty 的形状与之一致，键顺序同样来自字段声明顺序。
        let raw = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("序列化失败: {e}"))
        })?;
        std::fs::write(path, raw)
    }
}

/// 解析 `20M` 这类字符串，返回 MB 数。范围 1M–100M。
pub fn parse_size_mb(s: &str) -> Result<i64, String> {
    let trimmed = s.trim().to_uppercase();
    let Some(digits) = trimmed.strip_suffix('M') else {
        return Err(format!("最小分片大小须以 M 结尾（如 20M），当前 {s:?}"));
    };
    let Ok(n) = digits.parse::<i64>() else {
        return Err(format!("最小分片大小无法解析: {s:?}"));
    };
    if !(1..=100).contains(&n) {
        return Err(format!("最小分片大小必须在 1M–100M 之间，当前 {s:?}"));
    }
    Ok(n)
}

/// macOS 规范位置。
///
/// ⚠️ 这里的 `$HOME/Library/Application Support/...` 是**由测试钉死的例外**
/// （`default_path_is_macos_canonical`，对应 Go `settings_test.go` 的
/// `TestDefaultPathIsMacOSCanonical`），不是内核里新引入的平台界面习惯。
/// 内核不含界面概念这条约束照旧：整个内核里只有这一处平台路径，
/// 且删掉它会让已冻结的 Go 测试失去对应物。
///
/// Go 的 `os.UserHomeDir()` 在 Unix 上就是读 `$HOME`，取不到（未设或为空）时回退 `"settings.json"`。
pub fn default_path() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("BenagenDownloader")
            .join("settings.json"),
        _ => PathBuf::from("settings.json"),
    }
}

/// 反序列化的中间形态：**缺失字段留零值**（Go 的 `json.Unmarshal` 语义）。
///
/// 不能直接反序列化到 `Settings`——serde 默认要求字段齐全，
/// 而 Go 侧 `{"parallel":999,"connections":4}` 这种**只有两个键**的输入是合法的，
/// 其余字段落零值后再由 `merge_with_defaults` 逐项处理。
/// `Option` 的 `None` 同时覆盖「键缺失」与「键为 null」两种情形，与 Go 对 null 不赋值的行为一致。
///
/// ⚠️ 五个整型字段用 `Option<i64>` 而不是 `Option<i32>`：Go 的 `int` 在 darwin/arm64 上
/// 是 **64 位**，`json.Unmarshal` 能把 `4294967296` 收进 `Parallel`，随后由
/// `merge_with_defaults` 判它 `> 64`、**逐项**回退。若这里用 `i32`，serde 会在越界时报错，
/// 于是**整份文档**反序列化失败 → 七项全部回默认值：用户手改坏一个字段，其余合法设置被
/// 连坐丢掉；若同时设了限速还会静默变回 0（不限速），与契约 §2.2 反对的「静默变成不限速」
/// 是同一后果。`limit_mbps` 本就是 `i64`（与 Go 的 `int` 同位宽），故无需改动。
/// 超过 `i64` 的输入两边同样都是「整份失败」，行为一致。
#[derive(Debug, Deserialize)]
struct RawSettings {
    parallel: Option<i64>,
    connections: Option<i64>,
    splits: Option<i64>,
    min_split_size: Option<String>,
    limit_mbps: Option<i64>,
    max_tries: Option<i64>,
    retry_wait: Option<i64>,
}

/// 把 JSON 里读到的整数夹进 `i32` 值域；`None` → 0（Go 的零值）。
///
/// **饱和而非 `unwrap_or(0)`**：对 `retry_wait` 这种「0 是合法值」的字段，
/// `unwrap_or(0)` 会把 `-5000000000` 变成合法的 0，而 Go 会判它 `< 0`、回默认值。
///
/// 夹取**保序且保「越界」这一属性**，逐项核对（下界 LO、上界 HI 均远在 i32 值域内）：
/// - `LO ≤ n ≤ HI` → 夹取后仍是 n，合法，保留；
/// - `n > HI`：若 `n ≤ i32::MAX` 则结果就是 n（`> HI`）；否则夹到 `i32::MAX > HI`——两者都仍越界；
/// - `n < LO`：若 `n ≥ i32::MIN` 则结果就是 n（`< LO`）；否则夹到 `i32::MIN < LO`——两者都仍越界。
///
/// 七项逐一核对：`parallel` 1–64、`connections` 1–16、`splits` 1–16、
/// `max_tries` 1–100、`retry_wait` 0–60——上界全都 `< i32::MAX`，下界全都 `> i32::MIN`。
fn saturating_i32(v: Option<i64>) -> i32 {
    match v {
        Some(n) => n.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        None => 0,
    }
}

/// 读文件。**文件缺失或损坏都返回默认值而不是错误**——首次运行与手改坏配置都不该让客户端起不来。
///
/// 越界项**逐项**回退到默认值，合法项保留——避免一个手改坏的数字把整份配置作废。
pub fn load_from(path: &Path) -> Settings {
    let def = default_settings();
    let Ok(raw) = std::fs::read(path) else {
        return def;
    };
    let Ok(loaded) = serde_json::from_slice::<RawSettings>(&raw) else {
        return def;
    };
    let loaded = Settings {
        parallel: saturating_i32(loaded.parallel),
        connections: saturating_i32(loaded.connections),
        splits: saturating_i32(loaded.splits),
        min_split_size: loaded.min_split_size.unwrap_or_default(),
        limit_mbps: loaded.limit_mbps.unwrap_or(0),
        max_tries: saturating_i32(loaded.max_tries),
        retry_wait: saturating_i32(loaded.retry_wait),
    };
    merge_with_defaults(&loaded, &def)
}

/// 保留合法的字段，把越界的换成默认值。
fn merge_with_defaults(loaded: &Settings, def: &Settings) -> Settings {
    let mut out = loaded.clone();
    if loaded.parallel < 1 || loaded.parallel > 64 {
        out.parallel = def.parallel;
    }
    if loaded.connections < 1 || loaded.connections > 16 {
        out.connections = def.connections;
    }
    if loaded.splits < 1 || loaded.splits > 16 {
        out.splits = def.splits;
    }
    if parse_size_mb(&loaded.min_split_size).is_err() {
        out.min_split_size = def.min_split_size.clone();
    }
    // 上界同 Validate：防溢出，规格 §6 未规定限速上界
    if loaded.limit_mbps < 0 || loaded.limit_mbps > LIMIT_MBPS_MAX {
        out.limit_mbps = def.limit_mbps;
    }
    if loaded.max_tries < 1 || loaded.max_tries > 100 {
        out.max_tries = def.max_tries;
    }
    if loaded.retry_wait < 0 || loaded.retry_wait > 60 {
        out.retry_wait = def.retry_wait;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 临时目录夹具已收敛到 `crate::testutil`（任务 4 步骤 5b），
    // 这里只保留名字，行为与原先那份私有副本相同。
    use crate::testutil::TempDir;

    /// 对应 Go `any`：七项的取值类型不同（整型/字符串），用一个窄枚举承载。
    #[derive(Debug, PartialEq)]
    enum Val {
        I(i64),
        S(String),
    }

    /// 表驱动夹具的一行：`(用例名, 改一项的闭包)`。
    ///
    /// 起别名不只是为了短：`Vec<(&str, fn(&mut Settings))>` 这个形状会触发 clippy 的
    /// `type_complexity`（`--all-targets` 门禁下是硬失败）。两条测试共用同一个别名，
    /// 形状改一处即两处同改。
    type Case = (&'static str, fn(&mut Settings));

    /// 对应 Go `TestDefaultsMatchSpec`。
    #[test]
    fn defaults_match_spec() {
        let d = default_settings();
        let checks: Vec<(&str, Val, Val)> = vec![
            ("Parallel", Val::I(d.parallel.into()), Val::I(8)),
            ("Connections", Val::I(d.connections.into()), Val::I(16)),
            ("Splits", Val::I(d.splits.into()), Val::I(16)),
            (
                "MinSplitSize",
                Val::S(d.min_split_size.clone()),
                Val::S("20M".into()),
            ),
            ("MaxTries", Val::I(d.max_tries.into()), Val::I(3)),
            ("RetryWait", Val::I(d.retry_wait.into()), Val::I(1)),
            // 0 = 不限速
            ("LimitMBps", Val::I(d.limit_mbps), Val::I(0)),
        ];
        for (name, got, want) in checks {
            assert_eq!(got, want, "{name} 默认值不符（规格 §6）");
        }
    }

    /// 对应 Go `TestValidateRejectsOutOfRange`。
    #[test]
    fn validate_rejects_out_of_range() {
        let bad: Vec<Case> = vec![
            ("并行文件数为 0", |s: &mut Settings| s.parallel = 0),
            ("并行文件数超上限", |s: &mut Settings| s.parallel = 65),
            ("单文件连接数为 0", |s: &mut Settings| s.connections = 0),
            ("单文件连接数超上限", |s: &mut Settings| s.connections = 17),
            ("分片数为 0", |s: &mut Settings| s.splits = 0),
            ("分片数超上限", |s: &mut Settings| s.splits = 17),
            ("最小分片过小", |s: &mut Settings| s.min_split_size = "500K".into()),
            ("最小分片过大", |s: &mut Settings| s.min_split_size = "200M".into()),
            ("重试次数为 0", |s: &mut Settings| s.max_tries = 0),
            ("重试次数超上限", |s: &mut Settings| s.max_tries = 101),
            ("重试间隔为负", |s: &mut Settings| s.retry_wait = -1),
            ("重试间隔超上限", |s: &mut Settings| s.retry_wait = 61),
            ("限速为负", |s: &mut Settings| s.limit_mbps = -1),
            ("限速超上限", |s: &mut Settings| s.limit_mbps = 100_001),
        ];
        for (name, mut_) in bad {
            let mut s = default_settings();
            mut_(&mut s);
            assert!(s.validate().is_err(), "{name} 应当被拒绝");
        }
    }

    /// 对应 Go `TestValidateAcceptsBoundaries`。
    #[test]
    fn validate_accepts_boundaries() {
        let ok: Vec<Case> = vec![
            ("并行下限", |s: &mut Settings| s.parallel = 1),
            ("并行上限", |s: &mut Settings| s.parallel = 64),
            ("连接上限", |s: &mut Settings| s.connections = 16),
            ("分片上限", |s: &mut Settings| s.splits = 16),
            ("最小分片下限", |s: &mut Settings| s.min_split_size = "1M".into()),
            ("最小分片上限", |s: &mut Settings| s.min_split_size = "100M".into()),
            ("重试间隔 0", |s: &mut Settings| s.retry_wait = 0),
            ("限速为 0（不限）", |s: &mut Settings| s.limit_mbps = 0),
            // 以下五项是 Go 侧审查补的：原接受表只列了 -x/-s 的上限与 retry-wait 的下限，
            // 于是 "Connections < 1→< 2"、"Splits < 1→< 2"、"RetryWait > 60→> 59"、
            // "MaxTries > 100→> 99" 这四个变异体能整套测试通过——下限/上限没被钉住。
            ("连接下限", |s: &mut Settings| s.connections = 1),
            ("分片下限", |s: &mut Settings| s.splits = 1),
            ("重试次数下限", |s: &mut Settings| s.max_tries = 1),
            ("重试次数上限", |s: &mut Settings| s.max_tries = 100),
            ("重试间隔上限", |s: &mut Settings| s.retry_wait = 60),
            // 限速上界为防溢出而设（规格 §6 未规定限速上界），必须被接受
            ("限速上限", |s: &mut Settings| s.limit_mbps = LIMIT_MBPS_MAX),
        ];
        for (name, mut_) in ok {
            let mut s = default_settings();
            mut_(&mut s);
            let r = s.validate();
            assert!(r.is_ok(), "{name} 应当被接受，却报错: {:?}", r.err());
        }
    }

    /// 对应 Go `TestSaveLoadRoundTrip`。
    #[test]
    fn save_load_round_trip() {
        let dir = TempDir::new();
        let path = dir.join("settings.json");

        let mut s = default_settings();
        s.parallel = 32;
        s.limit_mbps = 50;
        s.save_to(&path).expect("保存失败");

        let got = load_from(&path);
        assert!(
            got.parallel == 32 && got.limit_mbps == 50,
            "往返后内容不符: {got:?}"
        );
    }

    /// 对应 Go `TestLoadMissingFileGivesDefaults`。
    #[test]
    fn load_missing_file_gives_defaults() {
        // 文件不存在不是错误——首次运行就该用默认值
        let dir = TempDir::new();
        let got = load_from(&dir.join("不存在.json"));
        assert_eq!(got, default_settings(), "应回退到默认值，实际: {got:?}");
    }

    /// 对应 Go `TestLoadCorruptFileGivesDefaults`。
    #[test]
    fn load_corrupt_file_gives_defaults() {
        let dir = TempDir::new();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{不是 json").expect("写测试文件失败");
        let got = load_from(&path);
        assert_eq!(got, default_settings(), "应回退到默认值，实际: {got:?}");
    }

    /// 对应 Go `TestLoadOutOfRangeFallsBackPerField`。
    ///
    /// 表内每行都给出**完整的七项期望值**（逐行都是精确期望，不用 `!=` 这类弱断言；
    /// 四行的期望值均用真实 Go 包实测取得）。前两行是原夹具，后两行是审查加固补的
    /// （同任务 1/2 的夹具改动先例）。后两行打的是**超出 i32 值域**的输入，两条选值要点：
    ///
    /// ① 越界值必须超出 i32 值域，否则测不到「i32 收不下」这条路——Go 的 `int` 是 64 位，
    ///    能收下 `4294967296` 再逐项回退；若用 `Option<i32>` 反序列化，serde 报错会让
    ///    **整份文档**回默认，同一行其余合法项被连坐重置（第 2 行正是钉这个）。
    /// ② 测「越界」不能选**截断后恰好落回合法区间**的值：`4294967296` 截断成 0（仍越界），
    ///    对截断类错误没有判别力；`4294967300` 截断成 4（看似合法、会被保留）才有（第 4 行）。
    #[test]
    fn load_out_of_range_falls_back_per_field() {
        // 文件被手改坏时，只有越界的那几项回默认，其余保留
        let cases: Vec<(&str, &str, Settings)> = vec![
            (
                "parallel 越界（未写的项落零值）",
                r#"{"parallel":999,"connections":4}"#,
                Settings {
                    parallel: 8,
                    connections: 4,
                    splits: 16,
                    min_split_size: "20M".into(),
                    limit_mbps: 0,
                    max_tries: 3,
                    retry_wait: 0,
                },
            ),
            (
                "parallel 越界且超出 i32 值域，connections 不得被连坐",
                r#"{"parallel":4294967296,"connections":4}"#,
                Settings {
                    parallel: 8,
                    connections: 4,
                    splits: 16,
                    min_split_size: "20M".into(),
                    limit_mbps: 0,
                    max_tries: 3,
                    retry_wait: 0,
                },
            ),
            (
                // 0 对 retry_wait 是**合法值**：把「收不下」实现成 unwrap_or(0) 会得到 0，
                // 而 Go 判它 < 0、回默认 1——故期望必须是精确的 1
                "retry_wait 为负且超出 i32 值域，回默认 1（不是合法值 0）",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":-5000000000}"#,
                Settings {
                    parallel: 32,
                    connections: 4,
                    splits: 5,
                    min_split_size: "30M".into(),
                    limit_mbps: 7,
                    max_tries: 6,
                    retry_wait: 1,
                },
            ),
            (
                // 4294967300 截断成 4，正好落回 1–16 —— 用 as i32 截断的实现会把它当合法值保留
                "connections 超上限且截断后落回合法区间，回默认 16",
                r#"{"parallel":32,"connections":4294967300,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":2}"#,
                Settings {
                    parallel: 32,
                    connections: 16,
                    splits: 5,
                    min_split_size: "30M".into(),
                    limit_mbps: 7,
                    max_tries: 6,
                    retry_wait: 2,
                },
            ),
        ];
        for (name, raw, want) in cases {
            let dir = TempDir::new();
            let path = dir.join("settings.json");
            std::fs::write(&path, raw).expect("写测试文件失败");
            let got = load_from(&path);
            assert_eq!(got, want, "{name}: 输入 {raw}");
        }
    }

    /// 对应 Go `TestAddURIOptionsMapping`。
    #[test]
    fn add_uri_options_mapping() {
        // 逐任务选项（改完对之后新加的任务生效）
        let mut s = default_settings();
        s.connections = 4;
        s.splits = 8;
        s.min_split_size = "10M".into();
        s.max_tries = 5;
        s.retry_wait = 2;
        let opts = s.per_task_options();
        let want: Vec<(&str, &str)> = vec![
            ("max-connection-per-server", "4"),
            ("split", "8"),
            ("min-split-size", "10M"),
            ("max-tries", "5"),
            ("retry-wait", "2"),
        ];
        for (k, v) in want {
            assert_eq!(opts.get(k).map(String::as_str), Some(v), "选项 {k} 不符");
        }
    }

    /// 对应 Go `TestGlobalOptionsMapping`。
    #[test]
    fn global_options_mapping() {
        // 全局选项（走 changeGlobalOption）
        let mut s = default_settings();
        s.parallel = 16;
        s.limit_mbps = 50;
        let g = s.global_options();
        assert_eq!(
            g.get("max-concurrent-downloads").map(String::as_str),
            Some("16"),
            "max-concurrent-downloads 不符"
        );
        // 50 MB/s = 50 * 1024 * 1024 字节/秒
        assert_eq!(
            g.get("max-overall-download-limit").map(String::as_str),
            Some("52428800"),
            "限速换算错误（应为 50*1024*1024）"
        );
    }

    /// **对 Go `TestGlobalOptionsOmitsLimitWhenUnlimited` 的有意偏离**（契约 §7 记一次）。
    ///
    /// Go 在 `limit_mbps == 0` 时**省略**该键（`settings.go:107-116` 逐字如此）。
    /// 那是**错的**：`aria2.changeGlobalOption` 是**部分更新**——对象里没出现的键
    /// **保持原值**。实测（内嵌 aria2 1.37.0）：
    ///
    /// ```text
    /// changeGlobalOption {"max-overall-download-limit":"5242880"}   → getGlobalOption = '5242880'
    /// changeGlobalOption {"max-concurrent-downloads":"8"}          → getGlobalOption = '5242880'  ← 没被清掉
    /// changeGlobalOption {"max-overall-download-limit":"0"}        → getGlobalOption = '0'        ← 显式 0 才清得掉
    /// ```
    ///
    /// 于是"客户把限速从 5 MB/s 改回不限速"会变成**静默失效**：界面显示"已保存"、
    /// 引擎继续限速、没有任何一处报错——正是契约 §2.2/§2.5 与全局约束禁止的形状。
    /// 判别力：把实现改回"0 时省略" → 本测试红。
    #[test]
    fn global_options_sends_explicit_zero_when_unlimited() {
        let mut s = default_settings();
        s.limit_mbps = 0;
        assert_eq!(
            s.global_options()
                .get("max-overall-download-limit")
                .map(String::as_str),
            Some("0"),
            "不限速时必须**显式下发 0**：省略该键等于什么都没做（changeGlobalOption 是部分更新），\
             客户取消限速后会继续被限速且无人报错"
        );
    }

    /// 对应 Go `TestParseSizeMBBoundaries`。
    ///
    /// 越界/畸形：0M 探数值下限、101M 探数值上限（原有 200M 探不到 100 与 101 之差）、
    /// 20 探「须以 M 结尾」、M 与空串探无法解析。
    #[test]
    fn parse_size_mb_boundaries() {
        for input in ["0M", "101M", "20", "M", ""] {
            assert!(
                parse_size_mb(input).is_err(),
                "parse_size_mb({input:?}) 应被拒绝"
            );
        }
        // 边界值本身必须被接受，且换算正确
        for (input, want) in [("1M", 1i64), ("100M", 100), ("20M", 20)] {
            let got = parse_size_mb(input);
            assert_eq!(
                got.as_ref().ok().copied(),
                Some(want),
                "parse_size_mb({input:?}) = {got:?}, 期望 {want}"
            );
        }
    }

    /// 对应 Go `TestLoadOutOfRangeFallsBackPerFieldAllSeven`。
    ///
    /// 每行的 JSON 都写全七项：其中一项越界、其余六项合法且非默认。
    /// 越界那项必须回默认，其余六项必须原样保留——这样才锁住「逐项回退」而不是「整份作废」。
    ///
    /// 之所以每行都把七项写全：JSON 里缺失的字段会解析成 Go 零值，而 0 对 retry_wait（0–60）
    /// 与 limit_mbps（0 = 不限）本就是合法值，与显式写 0 无法区分。写全字段可让每行只考验一项。
    #[test]
    fn load_out_of_range_falls_back_per_field_all_seven() {
        let base_want = Settings {
            parallel: 32,
            connections: 4,
            splits: 5,
            min_split_size: "30M".into(),
            limit_mbps: 7,
            max_tries: 6,
            retry_wait: 2,
        };

        let cases: Vec<(&str, &str, Settings)> = vec![
            (
                "parallel 越界",
                r#"{"parallel":999,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.parallel = 8),
            ),
            (
                "connections 越界",
                r#"{"parallel":32,"connections":17,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.connections = 16),
            ),
            (
                "splits 越界",
                r#"{"parallel":32,"connections":4,"splits":0,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.splits = 16),
            ),
            (
                "min_split_size 越界",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"999M","limit_mbps":7,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.min_split_size = "20M".into()),
            ),
            (
                "limit_mbps 为负",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":-5,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.limit_mbps = 0),
            ),
            (
                "limit_mbps 超上界",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":100001,"max_tries":6,"retry_wait":2}"#,
                mk(&base_want, |s| s.limit_mbps = 0),
            ),
            (
                "max_tries 越界",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":101,"retry_wait":2}"#,
                mk(&base_want, |s| s.max_tries = 3),
            ),
            (
                "retry_wait 越界",
                r#"{"parallel":32,"connections":4,"splits":5,"min_split_size":"30M","limit_mbps":7,"max_tries":6,"retry_wait":61}"#,
                mk(&base_want, |s| s.retry_wait = 1),
            ),
        ];
        for (name, raw, want) in cases {
            let dir = TempDir::new();
            let path = dir.join("settings.json");
            std::fs::write(&path, raw).expect("写测试文件失败");
            let got = load_from(&path);
            assert_eq!(got, want, "{name}: 输入 {raw}");
        }
    }

    /// 复制一份 `Settings` 再改一处，避免每个用例重复整段字面量（对应 Go 的 `mk`）。
    fn mk(s: &Settings, mut_: fn(&mut Settings)) -> Settings {
        let mut out = s.clone();
        mut_(&mut out);
        out
    }

    /// 对应 Go `TestPerTaskOptionsNormalizesMinSplitSize`。
    #[test]
    fn per_task_options_normalizes_min_split_size() {
        // Validate 接受 " 20m " 这类写法，但下发给 aria2 的必须是规范化的 "20M"：
        // 原样转发会让 aria2 直接启动失败，也就是「启动了才发现参数非法」
        let mut s = default_settings();
        s.min_split_size = " 20m ".into();
        assert!(
            s.validate().is_ok(),
            "带空格/小写的写法应被 Validate 接受: {:?}",
            s.validate().err()
        );
        assert_eq!(
            s.per_task_options().get("min-split-size").map(String::as_str),
            Some("20M"),
            "下发的 min-split-size 应为规范化的 \"20M\""
        );

        // 兜底：万一未经 Validate 就下发，也不得把非法值原样喂给 aria2，更不得静默省略该键——
        // 少下发一个选项和下发一个非法值，都是「启动了才发现」的老问题
        s.min_split_size = "不是大小".into();
        let opts = s.per_task_options();
        let got = opts
            .get("min-split-size")
            .expect("解析失败时不得静默省略 min-split-size");
        assert_eq!(
            got,
            &default_settings().min_split_size,
            "解析失败时应兜底下发默认值"
        );
    }

    /// 对应 Go `TestSaveToCreatesMissingDirs`。
    #[test]
    fn save_to_creates_missing_dirs() {
        // 首次运行时 ~/Library/Application Support/BenagenDownloader/ 整条路径都不存在
        let dir = TempDir::new();
        let path = dir.join("BenagenDownloader/nested/settings.json");
        let mut s = default_settings();
        s.retry_wait = 7;
        s.save_to(&path).expect("目标目录不存在时应自动创建");
        let got = load_from(&path);
        assert_eq!(got.retry_wait, 7, "往返后内容不符: {got:?}");
    }

    /// 对应 Go `TestDefaultPathIsMacOSCanonical`。
    #[test]
    fn default_path_is_macos_canonical() {
        // Go 侧取不到家目录时 t.Skipf；Rust 侧同样跳过（HOME 未设或为空）
        let home = match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => PathBuf::from(h),
            _ => return,
        };
        let want = home
            .join("Library")
            .join("Application Support")
            .join("BenagenDownloader")
            .join("settings.json");
        assert_eq!(default_path(), want, "default_path() 不符");
        assert!(
            default_path().to_string_lossy().ends_with("settings.json"),
            "default_path() 应指向 settings.json，实际 {:?}",
            default_path()
        );
    }
}
