//! state —— "已下载"状态文件。
//!
//! 逐条移植自 `downloader/internal/state/{state.go,state_test.go}`（Go 侧已冻结）。
//! 7 条测试与 Go 一一对应，对应表见
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-4-report.md`。
//!
//! ⚠️ 这个文件是**优化**不是**真相**。它只用来加速"跳过已完整下载的文件"这个决策；
//! 正确性由 CRC64 复校验保证。因此：文件损坏、丢失、或与真实文件不符时，
//! 行为必须退化为"重新校验"，绝不能因为状态文件里有记录就跳过校验。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 状态文件名（规格 §7）
pub const FILE_NAME: &str = ".benagen-state.json";

/// 状态文件格式版本
pub const VERSION: i32 = 1;

/// 记录一个文件的已知状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub size: i64,
    pub mtime: f64,
    pub crc64: String,
}

impl Entry {
    /// 判断磁盘上的现状是否与记录一致（用于跳过决策）。
    ///
    /// ⚠️ **精确相等，不要加容差。** Go 是 `e.Size == size && e.MTime == mtime`，
    /// 两个 `f64` 直接比。`mtime` 来自文件系统的元数据、原样往返，不存在"浮点漂移"需要照顾。
    /// 加 epsilon 是那种看起来更稳健、实则改变行为的"改良"——它会把"文件被替换过"
    /// 从「不匹配」变成「匹配」，于是该重下的文件被跳过（**静默少交**，本项目最忌讳的失败模式）。
    pub fn matches(&self, size: i64, mtime: f64) -> bool {
        // 逐字对应 Go 的 `e.Size == size && e.MTime == mtime`：两个 f64 直接比，
        // **不加任何 epsilon**（理由见上面的文档注释）。
        self.size == size && self.mtime == mtime
    }
}

/// 一批交付的本地状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: i32,
    pub code: String,
    // ⚠️ 两个属性缺一不可：Go 的标签是 `json:"generated_at,omitempty"`。
    // 只写 `default` 会让**空** generated_at 落盘成 `""`，而 Go 是**整个键不出现**——
    // 这是一处真实的落盘差异，`skip_serializing_if` 才是 `omitempty` 的对应物。
    //
    // 关于判别力（控制者裁定 3，2026-09-16）：`skip_serializing_if` **没有、也不可能有**测试覆盖——
    // `save` 恒写非空时间戳，这个分支经公开 API（load/put/bind/save）结构性地走不到。
    // 保留它是"显式表达意图 + 纵深防御"：将来若有人改 `save` 不再赋时间戳，语义仍然是 Go 的
    // `omitempty`（整个键不出现），而不是落一个 `""`。这不是"漏了测试"，是"没有可测的面"。
    // Go 侧的 `omitempty` 同样没有测试覆盖。反向的那一面（**必须有** generated_at）是守住了的：
    // `persisted_format_matches_spec` 断言四个顶层键存在，把 `save` 里那行赋值删掉它就会红。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generated_at: String,
    pub files: BTreeMap<String, Entry>,
    /// 状态文件所在目录。**不落盘**（Go 里是小写字段 `dir`，同样不进 JSON）。
    #[serde(skip)]
    dir: Option<PathBuf>,
}

impl State {
    /// 从目录读取状态文件。
    ///
    /// 文件不存在、内容损坏、或版本不认识时，都返回一个**空状态**而不是错误——
    /// 调用方据此自然退化为"全部重新校验"，这正是我们想要的行为。
    ///
    /// Go 侧的 `Files == nil`（`"files"` 键缺失或为 `null`）检查在 Rust 里没有对应物：
    /// `BTreeMap` 不是 `Option`，键缺失或为 `null` 都会让 `serde_json` 反序列化失败，
    /// 直接落到下面那条"损坏"分支，结果同样是空状态。
    pub fn load(dir: &Path) -> State {
        let empty = || State {
            version: VERSION,
            code: String::new(),
            generated_at: String::new(),
            files: BTreeMap::new(),
            dir: dir_of(dir),
        };
        let Ok(raw) = std::fs::read(dir.join(FILE_NAME)) else {
            return empty(); // 不存在就是空的
        };
        // 损坏（含 `"files"` 缺失/null、字段类型不对）一律当空处理
        let Ok(mut loaded) = serde_json::from_slice::<State>(&raw) else {
            return empty();
        };
        if loaded.version != VERSION {
            return empty(); // 版本不符：当空处理
        }
        loaded.dir = dir_of(dir);
        loaded
    }

    /// 把一个条目绑定到某个交付码。
    ///
    /// 换码意味着这是另一批数据，上一批的条目一律作废——否则客户换一批下载时
    /// 会拿旧记录去跳过新文件，那是静默少交。
    pub fn bind(&mut self, code: &str) {
        if self.code == code {
            return;
        }
        self.code = code.to_string();
        // Go 里是 `s.Files = map[string]Entry{}`（换一张新 map）；这里 `clear()` 的
        // 可观测行为相同——都是"非 nil 的空 map"，而 Rust 的 `BTreeMap` 本来就没有 nil 态。
        self.files.clear();
    }

    /// 查询一个条目
    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.files.get(path)
    }

    /// 记录一个条目
    pub fn put(&mut self, path: &str, e: Entry) {
        // Go 里有一段 `if s.Files == nil` 的兜底（零值 `State` 的 map 是 nil）。
        // Rust 的 `BTreeMap` 恒为已初始化，没有对应物。
        self.files.insert(path.to_string(), e);
    }

    /// 写回状态文件（**原子替换**，避免中途崩溃留下半个文件）。
    ///
    /// ⚠️ 绝不能改成 `File::create` 直写目标文件：那样第二次写入会因目标只读而被拒，
    /// 且崩溃时会留下半个文件。`save_uses_atomic_replace` 守这一点。
    ///
    /// Go 的 `Save` 是就地改自己的 `Version`/`GeneratedAt` 再 Marshal 自己；
    /// Rust 侧签名是 `&self`（简报给定），因此这里构造一份**快照**再序列化。
    /// 差异只在"调用后 `self.generated_at` 是否被就地写脏"——Go 会，Rust 不会，
    /// 而该字段的唯一用途就是落盘。
    pub fn save(&self) -> io::Result<()> {
        let Some(dir) = self.dir.as_deref() else {
            return Ok(()); // 没有目录（零值状态）：不写盘，也不算错误
        };
        let snapshot = State {
            version: VERSION,
            code: self.code.clone(),
            generated_at: now_rfc3339(),
            files: self.files.clone(),
            dir: None,
        };
        let raw = serde_json::to_vec_pretty(&snapshot)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // 临时文件与目标同目录：跨目录/跨设备的 rename 不是原子的
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, &raw)?;
        std::fs::rename(&tmp, dir.join(FILE_NAME))
    }
}

/// 把传给 `load` 的目录归一化成 `Option<PathBuf>`。
///
/// Go 的 `Save` 有一句 `if s.dir == "" { return nil }`，它同时覆盖两种情形：
/// 零值 `State`（`var s State`）与显式传入空串。Rust 用 `Option::None` 表达零值，
/// 空串则在这里归一化为 `None`——两者都不落盘，与 Go 同义。
fn dir_of(dir: &Path) -> Option<PathBuf> {
    if dir.as_os_str().is_empty() {
        None
    } else {
        Some(dir.to_path_buf())
    }
}

/// 对应 Go `Save` 里的 `time.Now().Format(time.RFC3339)`，即规格 §7 的 `generated_at`。
///
/// Go 写的是**本地时区**偏移（如 `2026-09-14T16:20:00+08:00`）。Rust 的 std 没有任何
/// 时区 API（拿本地偏移必须加依赖，本任务不许），因此这里写 UTC 的 `Z` 形态
/// （`2026-09-14T08:20:00Z`）——两者都是合法 RFC3339，`time.Parse(time.RFC3339, …)`
/// 都认，落盘语义（"这批状态是什么时候写的"）不受影响。
fn now_rfc3339() -> String {
    let secs = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0, // 系统时钟早于 1970：不可能，真出现就落 1970-01-01T00:00:00Z
    };
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// 距 1970-01-01 的天数 → (年, 月, 日)，UTC。
///
/// 不引入日期库（不许加依赖）：`std::time` 只给"距 1970 的秒数"，日历换算自己写
/// （Howard Hinnant 的 `civil_from_days`，与 `delivery.rs` 手写 RFC3339 解析同一路数）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 }; // 1/2 月属于上一个日历年
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    /// **状态文件里的 `mtime` 必须逐位往返**（2026-09-21，任务 7 的 R30 ②）。
    ///
    /// 为什么值得单独一条：`Entry::matches` 是**逐位比较、没有 epsilon**（与 Go 同形），
    /// 而 `serde_json` 默认的浮点解析**不是正确舍入**的 —— 差 1 ULP 就足以把一个
    /// "盘上已经完整"的文件判成待下载，于是**每次重跑都重下一遍**。
    /// 这条测试与 `Cargo.toml` 的 `serde_json/float_roundtrip` 是一对：
    /// 那个 feature 被谁去掉，这里就红。
    ///
    /// ⚠️ 判据是"**往返**"，不是"抄一个期望值"：`1789940395.3978717` 这个字面量是从
    /// 真跑出来的 `.benagen-state.json` 里抄的（mtime 字段），它正是让旧解析器差 1 ULP 的那个。
    /// 期望值用 `str::parse::<f64>`（Rust 的十进制解析是**正确舍入**的）现算。
    #[test]
    fn mtime_survives_a_json_round_trip_verbatim() {
        for lit in [
            "1789940395.3978717",
            "1789940619.3875642",
            "1789940619.3892076",
            "1789940423.3767445",
            "0.0",
        ] {
            let exact: f64 = lit.parse().expect("字面量必须是合法十进制");
            let through: f64 = serde_json::from_str(lit).expect("serde_json 必须解得出来");
            assert_eq!(
                through.to_bits(),
                exact.to_bits(),
                "{lit} 经 serde_json 解析后不是同一个 double（差 {} ULP）—— \
                 `serde_json` 的 `float_roundtrip` feature 掉了？\
                 掉了的后果是：状态文件里的 mtime 差 1 ULP ⇒ 已完整的文件被判成待下载 ⇒ 每次重跑重下一遍。",
                (through.to_bits() as i64 - exact.to_bits() as i64).abs()
            );
            // 再走一遍真正的那条链：State → JSON 文本 → State。
            let mut s = State::load(TempDir::new().path());
            s.put("a.txt", Entry { size: 1, mtime: exact, crc64: "1".into() });
            let text = serde_json::to_string(&s).expect("状态必须能序列化");
            let back: State = serde_json::from_str(&text).expect("状态必须能反序列化");
            let got = back.get("a.txt").expect("条目必须还在").mtime;
            assert_eq!(
                got.to_bits(),
                exact.to_bits(),
                "mtime 经状态文件往返后变了：{lit}"
            );
        }
    }

    /// 对应 Go 的 `Entry{Size: n}` 部分字面量：其余字段取零值（`MTime: 0`、`CRC64: ""`）。
    fn zero_entry(size: i64) -> Entry {
        Entry {
            size,
            mtime: 0.0,
            crc64: String::new(),
        }
    }

    /// 对应 Go 的 `keysOf`：报错信息里列出实际键名，方便定位落盘格式的偏差。
    fn keys_of(m: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
        let mut ks: Vec<String> = m.keys().cloned().collect();
        ks.sort();
        ks
    }

    /// 对应 Go `TestRoundTrip`。
    #[test]
    fn round_trip() {
        let dir = TempDir::new();
        // Go 侧断言 `Load` 空目录**不报错**；Rust 的 `load` 返回 `State` 而非 `Result`，
        // 这条断言在类型上就成立了。
        let mut s = State::load(dir.path());
        s.put(
            "t/Readme.txt",
            Entry {
                size: 6,
                mtime: 123.5,
                crc64: "5432380796884633278".to_string(),
            },
        );
        s.save().expect("Save 失败");

        let s2 = State::load(dir.path());
        let e = s2.get("t/Readme.txt");
        assert!(
            matches!(e, Some(e) if e.size == 6 && e.mtime == 123.5 && e.crc64 == "5432380796884633278"),
            "往返后内容不符: {e:?}"
        );
    }

    /// 对应 Go `TestPersistedFormatMatchesSpec`。
    ///
    /// 状态文件的字段名是规格 §7 钉死的磁盘格式（会随数据目录一起被别的工具/后续版本读）。
    /// `round_trip` 走的是同一个结构体的 Marshal→Unmarshal 往返，字段名写错它照样绿，
    /// 所以这里直接读原始 JSON 把键名钉住。
    #[test]
    fn persisted_format_matches_spec() {
        let dir = TempDir::new();
        let mut s = State::load(dir.path());
        s.bind("uDRR8xT9._tE6uRDnhls");
        s.put(
            "test/Readme.txt",
            Entry {
                size: 6,
                mtime: 1757855151.123,
                crc64: "5432380796884633278".to_string(),
            },
        );
        s.save().expect("Save 失败");

        let raw = std::fs::read(dir.join(FILE_NAME)).expect("读回状态文件失败");
        let doc: serde_json::Value =
            serde_json::from_slice(&raw).expect("状态文件不是合法 JSON");
        let doc = doc.as_object().expect("状态文件顶层应是对象");
        for k in ["version", "code", "generated_at", "files"] {
            assert!(
                doc.contains_key(k),
                "顶层缺少规格 §7 的键 {k:?}，实际={:?}",
                keys_of(doc)
            );
        }
        assert_eq!(doc["version"], serde_json::json!(VERSION), "version 落盘不符");
        assert_eq!(
            doc["code"],
            serde_json::json!("uDRR8xT9._tE6uRDnhls"),
            "code 落盘不符"
        );
        // generated_at 必须真的写出去且格式合法（只判非空会漏掉"赋了值但格式错"）
        let ga = doc["generated_at"]
            .as_str()
            .unwrap_or_else(|| panic!("generated_at 应是字符串，实际={}", doc["generated_at"]));
        assert!(!ga.is_empty(), "generated_at 不应为空");
        assert!(
            parse_rfc3339(ga).is_some(),
            "generated_at 不是合法 RFC3339: {ga:?}"
        );
        let files = doc["files"]
            .as_object()
            .unwrap_or_else(|| panic!("\"files\" 应是 path→Entry 的对象，实际={}", doc["files"]));
        let entry = files
            .get("test/Readme.txt")
            .unwrap_or_else(|| panic!("files 下应有以交付内路径为键的条目，实际={files:?}"))
            .as_object()
            .expect("条目应是对象");
        for k in ["size", "mtime", "crc64"] {
            assert!(
                entry.contains_key(k),
                "条目缺少规格 §7 的键 {k:?}，实际={:?}",
                keys_of(entry)
            );
        }
        assert_eq!(entry["size"].as_i64(), Some(6), "size 落盘不符");
        assert_eq!(
            entry["mtime"].as_f64(),
            Some(1757855151.123),
            "mtime 落盘不符"
        );
        assert_eq!(
            entry["crc64"].as_str(),
            Some("5432380796884633278"),
            "crc64 落盘不符"
        );
    }

    /// 对应 Go `TestSaveUsesAtomicReplace`。
    ///
    /// 原子替换的确定性判据：rename 只需要**目录**写权限，不需要目标文件本身可写；
    /// 而"直写目标文件"必须打开目标文件写入，目标只读时会 EACCES。
    /// 这个权限不对称与并发时序无关，因此是确定性的（不是概率性的撕裂读窗口）。
    #[cfg(unix)]
    #[test]
    fn save_uses_atomic_replace() {
        use std::os::unix::fs::PermissionsExt;

        // 对应 Go 的 `if os.Geteuid() == 0 { t.Skip(...) }`：root 绕过权限检查，本测试失去判别力。
        // Rust 的 std 没有取 euid 的 API，本任务又不许新增依赖（libc/nix 都不行），
        // 因此改用**不依赖 libc 的等价判据**：把一个只读文件摆在那里，探一次"能否打开写入"。
        // 探得"能写"即说明当前身份不受权限约束（root 或等价），判据不成立 → 跳过。
        let probe_dir = TempDir::new();
        let probe = probe_dir.join("probe");
        std::fs::write(&probe, b"x").expect("写探测文件失败");
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o444))
            .expect("设置探测文件权限失败");
        let writable = std::fs::OpenOptions::new().write(true).open(&probe).is_ok();
        if writable {
            // 与 `os.Geteuid() == 0` 表达的是同一件事——"本判据在此环境下不成立"。
            eprintln!("跳过 save_uses_atomic_replace：只读文件仍可写，当前身份不受权限约束");
            return;
        }

        let dir = TempDir::new();
        let mut s = State::load(dir.path());
        s.bind("CODE_A");
        s.put("a", zero_entry(1));
        s.save().expect("首次 Save 失败");

        let target = dir.join(FILE_NAME);
        // 目标只读，目录仍可写
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444))
            .expect("把目标改成只读失败");

        s.put("b", zero_entry(2));
        s.save()
            .expect("原子替换应成功（rename 只需目录写权限）");

        let after = State::load(dir.path());
        assert!(after.get("b").is_some(), "替换后应含新条目 b");
    }

    // Windows 上这条测试（以及它的 Go 原文）必须跳过：
    // `MoveFileEx(REPLACE_EXISTING)` 同样无法覆盖**只读**目标文件，
    // "rename 只需目录写权限"这个前提在 Windows 上不成立 → 正确实现也会失败（假红）。
    // 这是本任务唯一需要平台 cfg 的地方，理由如上，不是随手加的。

    /// 对应 Go `TestUnknownVersionDegradesInsteadOfFailing`。
    ///
    /// 版本不认识时必须退化为空状态——这是"优化不是真相"的一部分：
    /// 未来版本可能改了字段语义，拿旧版本去解读会得出错误的跳过决策。
    #[test]
    fn unknown_version_degrades_instead_of_failing() {
        let dir = TempDir::new();
        let future = r#"{"version": 999, "code": "CODE_A", "files": {"a": {"size": 1, "mtime": 2, "crc64": "7"}}}"#;
        std::fs::write(dir.join(FILE_NAME), future).expect("写状态文件失败");

        // Go 侧断言 `Load` 不报错；Rust 的 `load` 返回 `State` 而非 `Result`，类型上即成立。
        let s = State::load(dir.path());
        assert!(s.get("a").is_none(), "版本不认识时不应提供任何条目");
        assert_eq!(s.files.len(), 0, "版本不认识时应是空状态");
    }

    /// 对应 Go `TestCorruptFileDegradesInsteadOfFailing`。
    #[test]
    fn corrupt_file_degrades_instead_of_failing() {
        let dir = TempDir::new();
        // 写一个坏的状态文件
        std::fs::write(dir.join(FILE_NAME), "{不是 json").expect("写状态文件失败");

        let mut s = State::load(dir.path());
        assert!(s.get("anything").is_none(), "损坏文件不应提供任何条目");
        // 并且应能正常写入覆盖它
        s.put("a", zero_entry(1));
        s.save().expect("覆盖损坏文件失败");

        // 覆盖必须是真正的覆盖（截断重写），而不是把新内容接在坏内容后面
        let again = State::load(dir.path());
        match again.get("a") {
            Some(e) if e.size == 1 => {}
            other => panic!("覆盖后应能读回新条目 a(Size=1)，实际 {other:?}"),
        }
    }

    /// 对应 Go `TestDifferentCodeResets`。
    #[test]
    fn different_code_resets() {
        let dir = TempDir::new();
        let mut s = State::load(dir.path());
        s.bind("CODE_A");
        s.put("a", zero_entry(1));
        s.save().expect("Save 失败");

        let mut s2 = State::load(dir.path());
        // 前置条件：换码前该条目确实落盘了。否则下面的断言会因状态为空而恒真。
        assert!(
            s2.get("a").is_some(),
            "前置条件不成立：换码前条目 a 应已持久化"
        );
        // 绑定到另一个码时，旧条目不应被沿用
        s2.bind("CODE_B");
        assert!(s2.get("a").is_none(), "换码后不应保留上一批的条目");
        // 同码腿（修复轮 1，控制者裁定 1）：Go 的 `Bind` 有 `if s.Code == code { return }`
        // 早返回——**同一个码**再绑一次必须是 no-op，不能把这一批已记录的条目清空。
        // 断言顺序有意如此：上面先验"换码该清空"，这里再验"同码不该清空"，两者不互相掩盖。
        // 若同码也清空，后果是已校验记录全丢 → 整批重新下载（契约 §1.7 记的那次 50GB 白传）。
        s2.put("b", zero_entry(2));
        s2.bind("CODE_B"); // 同一个码：早返回，必须是 no-op
        assert!(
            s2.get("b").is_some(),
            "同码重绑不应清空条目（早返回缺失会让这一批已校验记录全丢）"
        );
    }

    /// 对应 Go `TestMatches`。
    #[test]
    fn matches() {
        let e = Entry {
            size: 6,
            mtime: 123.5,
            crc64: String::new(),
        };
        assert!(e.matches(6, 123.5), "size 与 mtime 都相同应命中");
        assert!(!e.matches(7, 123.5), "size 不同不应命中");
        assert!(!e.matches(6, 124.0), "mtime 不同不应命中");
        // 补丁（修复轮 1，控制者裁定 2）：上面那条只差 0.5，任何合乎常理的 epsilon 都照样
        // 判「不命中」，因此**夹不住"加容差"这个错法**（Go 的 `TestMatches` 同样夹不住）。
        // 这里取"只差 1e-9"——远在 f64 的 ulp（此处约 1.4e-14）之上、又远在任何 epsilon 之下，
        // 精确相等的实现必须判「不命中」。把 abs 差加容差的实现会在这里红。
        assert!(!e.matches(6, 123.5 + 1e-9), "mtime 只差 1e-9 也不得判为匹配");
    }

    /// 手写 RFC3339 解析，返回 Unix 秒；只认 Go `time.Parse(time.RFC3339, …)` 认得的形态。
    ///
    /// 与 `delivery.rs` 里的同名实现同形，但**不跨模块引用**（那是已审文件的私有函数，
    /// 一行都不许改），所以在这里独立写一份。
    fn parse_rfc3339(s: &str) -> Option<i64> {
        let b = s.as_bytes();
        if b.len() < 20
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
        {
            return None;
        }
        let num = |i: usize, n: usize| -> Option<i64> {
            let mut v = 0i64;
            for &c in &b[i..i + n] {
                if !c.is_ascii_digit() {
                    return None;
                }
                v = v * 10 + i64::from(c - b'0');
            }
            Some(v)
        };
        let (y, mo, da) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
        let (h, mi, se) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
        if !(1..=12).contains(&mo) || da < 1 || da > days_in_month(y, mo) {
            return None;
        }
        if h > 23 || mi > 59 || se > 59 {
            return None;
        }
        let mut i = 19;
        if b[i] == b'.' {
            // 可选小数秒：只校验形态，不参与秒数
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == start {
                return None;
            }
        }
        let offset = match b.get(i)? {
            b'Z' if i + 1 == b.len() => 0,
            sign @ (b'+' | b'-') => {
                if i + 6 != b.len() || b[i + 3] != b':' {
                    return None;
                }
                let (oh, om) = (num(i + 1, 2)?, num(i + 4, 2)?);
                if oh > 23 || om > 59 {
                    return None;
                }
                let v = oh * 3600 + om * 60;
                if *sign == b'-' {
                    -v
                } else {
                    v
                }
            }
            _ => return None,
        };
        Some(days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + se - offset)
    }

    fn days_in_month(y: i64, m: i64) -> i64 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            2 => 28,
            _ => 0,
        }
    }

    /// civil → 距 1970-01-01 的天数。与实现里的 `civil_from_days` 是**互逆的两套写法**，
    /// 因此两者互为交叉验证：任一处算错，`persisted_format_matches_spec` 都会红。
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }
}
