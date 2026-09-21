//! 批次历史文件（**对齐 macOS 的 `BatchHistoryStore.swift` + `Presentation/BatchHistory.swift`**）。
//!
//! 职责边界：**位置 + "坏就当空"这两个决定在这里**，字节搬运在 [`JsonFile`]，
//! **去重 / 排序 / 截到 50** 在 [`History::new`]。这一层刻意薄到只剩两件事：
//!   * [`History::load`] —— 读不出来 / 解析失败 / `version` 不认识 ⇒ **空历史**（E-1，
//!     所以它**没有 `Result`**：没有"失败"这条路径可走）；
//!   * [`History::save`] —— 一个**原子**的整份覆写（E-2），失败**返回 `Err`** 给调用方
//!     （不能让人以为存上了）。
//!
//! ⚠️ **不在这里做"读-改-写"**：那是"一次成功加载之后记一条"的语义，属于命令层
//!    （它知道什么算成功、时间是什么时候、内存里那份权威是谁）—— 对齐 macOS 那条
//!    一模一样的边界（`BatchHistoryStore.swift` 的头注释）。
//!
//! ## ⚠️ 三条不变量（[`History::new`] 维持它 —— 它在**读进来**时执行）
//!
//!   ① `entries` 按 `last_used_at` **倒序**（最近用的在最前）；
//!   ② `code` 唯一；
//!   ③ 条数 ≤ [`History::MAXIMUM_ENTRIES`]（超出淘汰**最久未用**的）。
//!
//! ⚠️ **不变量在这里维持、而不是留给界面**：换码面板只负责把 `entries` 从上到下画出来。
//!    一旦"顺序"变成视图的责任，第二个读这个类型的地方（下次自动加载、搜索、导出）
//!    就得自己再排一遍 —— 那正是同一条规则写两遍的开始。
//!
//! ## ⚠️ 写路径上的两条规则**不在本文件**（任务 8 收口的地方，见下面 `put` 那一段）
//!
//! `presentation::batch_history::BatchHistory` 里也有同样的三条不变量，**而且多两条**：
//! "`note` 传 `None` 就保持这一条原有的备注"与"空码是空操作"。那两条是**从 macOS 逐条
//! 移植过来的判断**，它们的家是呈现层（那里有逐条对位的用例）。
//! ⇒ 命令层的写路径（`api::history::apply`）以**那一份**为准，本文件只负责它的落盘形状。

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::storage::json_file::JsonFile;

/// 历史文件的名字（与偏好**同目录、另一个文件**）。
/// 对齐 macOS `BatchHistoryStore.fileName`。
pub const FILE_NAME: &str = "history.json";

/// 历史里的一条：一个用过的交付码 + 一句备注 + 它是从哪个交付服务器加载的 + 上次用的时间。
/// （对齐 macOS `BatchHistoryEntry`。）
///
/// ⚠️ `code` 是**唯一键**（规格 §1.1）：同码只留一条，再次使用是**更新**而不是新增。
/// ⚠️ `last_used_at` 保存的是**原文**（ISO 8601 带 `+08:00`，秒级），与交付清单的
///    `created_at` 同形 —— 壳**不把它解析成时间再格式化回去**（那会在往返里丢掉原文的
///    形状），显示一律复用既有的 `presentation::delivery_summary::TimestampPresentation`。
///    ⚠️ 排序**确实要**解析它（[`used_at_key`]），但那只用来比大小、不写回。
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 交付码。**非空**（空码进不来，见 [`History::new`]）。
    pub code: String,
    /// 一句自由文本（人类伙伴选定）。**空串合法** = 没写备注。
    #[serde(default)]
    pub note: String,
    /// 这一批是从哪个交付服务器加载的。**空串 = 默认服务器**（规格 §1.1）。
    #[serde(default)]
    pub base_url: String,
    /// ISO 8601 带偏移的原文，与清单的 `created_at` 同形。
    #[serde(default)]
    pub last_used_at: String,
}

impl HistoryEntry {
    /// 要发给内核的那个值：**空串 ⇒ `None`**（请求里不出现 `base_url` 这个键，
    /// 由内核用自己的默认交付服务器）。对齐 macOS `BatchHistoryEntry.baseURLOrNil`。
    ///
    /// ⚠️ 与 E-5 是同一条纪律：不要显式传一个"和默认一样"的值 ——
    ///    那会在默认值变化时静默分叉。
    pub fn base_url_or_nil(&self) -> Option<&str> {
        if self.base_url.is_empty() { None } else { Some(&self.base_url) }
    }
}

/// 一批历史记录（**不变量见模块头**）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    /// 给未来的自己的版本号：**读不出来或不认识 ⇒ 当空历史**（E-1）。
    /// 对齐 macOS `BatchHistory.version`。
    pub const VERSION: i64 = 1;

    /// 条数上限（规格 §5.2 明写 50）。无限增长的文件是留给下一个人踩的雷（E-3）。
    /// 对齐 macOS `BatchHistory.maximumEntries`。
    pub const MAXIMUM_ENTRIES: usize = 50;

    /// 空历史。**它不是错误态** —— 第一次运行、文件被删、文件坏了都是它（E-1）。
    /// 对齐 macOS `BatchHistory.empty`。
    pub fn empty() -> History {
        History { entries: Vec::new() }
    }

    /// 归一化：排倒序 → 去重（同码留最近那条）→ 截到上限。
    ///
    /// ⚠️ 排序**稳定**（同刻的两条保持传入的相对次序）：拿一个不保证稳定的排序
    ///    会让"同一秒里用的两个码"顺序随机，而那是可观察的。
    ///    Rust 的 `sort_by` 是稳定的，对齐 macOS 那边手写的 `a.offset < b.offset` 兜底。
    ///
    /// ⚠️ 空码的行**直接丢掉**（它不是一条交付批次：放进去只会变成历史列表里一个
    ///    点了没反应的假条目）。对齐 macOS `init(entries:)` 里那个 `where !entry.code.isEmpty()`。
    pub fn new(entries: Vec<HistoryEntry>) -> History {
        // ① 按 `last_used_at` **倒序**（解析不出来的排到最后，见 `used_at_key`）。
        let mut ordered = entries;
        ordered.sort_by(|a, b| used_at_key(&b.last_used_at).cmp(&used_at_key(&a.last_used_at)));

        // ② 去重 + 丢空码。排序之后**先出现的那条就是最近的**，留它。
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut kept: Vec<HistoryEntry> = Vec::with_capacity(ordered.len());
        for entry in ordered {
            if entry.code.is_empty() {
                continue;
            }
            if seen.insert(entry.code.clone()) {
                kept.push(entry);
            }
        }

        // ③ 超上限就截断（排序之后**末尾就是最久未用的**）。
        kept.truncate(Self::MAXIMUM_ENTRIES);
        History { entries: kept }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// **"上次用的码"的唯一来源**（E-4）：历史里最近使用的那一条。
    /// 对齐 macOS `BatchHistory.mostRecent`。
    pub fn most_recent(&self) -> Option<&HistoryEntry> {
        self.entries.first()
    }

    // ⚠️⚠️ **这里曾经有一个 `pub fn put(&self, entry) -> History`**（任务 3 交付的）。
    //    任务 8 的**审查修订轮**把它删掉了（控制者裁决：一个"有测试、没有生产调用者"
    //    的函数正是"测试给人虚假信心"的形状 —— 它的测试全绿，而线上一次都没跑过）。
    //    它的用例是**删 3 改 2**（三条随它一起删、另两条改走 [`History::new`]），
    //    **一条判断都没丢**（接手关系见下面那段）。删它的理由逐条如下：
    //
    //      · **它没有覆盖 `history_put` 命令那条路**：那条路要三个判断
    //        （"`note` 传 `None` 就保持原有的那一句" / 空码空操作 / 时间戳怎么成串），
    //        而 `put` 只做最后一个的一半（收整条 `HistoryEntry`）；
    //      · 那三个判断**已经有一处实现**：`presentation::batch_history::BatchHistory`
    //        的 `recording` / `setting_note`（任务 4 按 macOS 逐条移植、各有单测）。
    //        走 `put` 就意味着在 `api::history` 里把"备注保持"那条规则**再抄一遍** ——
    //        而"同一条规则写两遍"是本仓库最贵的债；
    //      · 于是 `put` 是那个规则的**第二实现**（且判别力更弱）：留着它，
    //        两条路都会"测试全绿"，而只有一条真的在服务客户。
    //
    //    **判据没有丢**：`repair` 那一半（排序 / 去重 / 截到 50）仍然由 [`History::new`]
    //    在**读进来**的时候执行（`Deserialize` 就是它），而"同码更新 / 排到最前 /
    //    超过 50 条淘汰最旧的 / 空码空操作"四条判断的用例**在呈现层那一份里全都在**
    //    （`presentation::batch_history` 的 `the_same_code_is_updated_not_duplicated` /
    //    `re_using_an_old_code_moves_it_to_the_top` / `the_history_keeps_at_most_fifty_entries` /
    //    `the_evicted_entry_is_the_least_recently_used_not_the_oldest_inserted`），
    //    外加任务 8 在 `api::history` 里的一条端到端用例（`a_use_with_an_empty_code_is_a_no_op`）。
    //
    //    ⇒ **本类型现在的职责只剩两件**：**磁盘形状**（`version` / 字段名 / 逐行容错）
    //    与**两条 IO 底线**（E-1 读坏当空 / E-2 原子写）。**判断一条历史"怎么写"是呈现层的事**。

    /// 读。文件不存在、读不动、内容坏了、`version` 不认识 ⇒ **空历史**
    /// （对齐 `BatchHistoryStore.load()`，E-1：绝不抛、绝不崩）。
    ///
    /// ⚠️ `path` 是**参数**（默认位置由调用方给 `storage::dir().join(FILE_NAME)`）：
    ///    真机上那个目录里是人类伙伴**真实在用**的数据，测试必须指到临时目录去。
    pub fn load(path: &Path) -> History {
        JsonFile::read::<History>(path).unwrap_or_else(History::empty)
    }

    /// 原子地把整份历史写下去（**写出去的永远是排好序、去过重、不超上限的那一份**）。
    /// （对齐 `BatchHistoryStore.save(_:)`，E-2。）
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        JsonFile::write(path, self)
    }
}

// ---------------------------------------------------------------------------
// 磁盘形状（字段名逐字，见模块头与规格 §5.2）
// ---------------------------------------------------------------------------

/// 落盘的形状：`{"version": 1, "entries": [{"code": …}]}`。
///
/// ⚠️ 空串字段（`note` / `base_url`）**照样写出去**：形状是固定的，
///    "键有时在有时不在"会让下一个读这个文件的人（包括未来版本的壳）多写一个分支。
///    对齐 macOS `BatchHistory.serialized()` 那条注释。
impl Serialize for History {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Raw<'a> {
            version: i64,
            entries: &'a [HistoryEntry],
        }
        Raw { version: History::VERSION, entries: &self.entries }.serialize(serializer)
    }
}

/// 从文件内容解析（E-1：**任何**问题都当空历史，绝不抛）。
///
/// 逐层容错（判据逐条照抄 macOS `BatchHistory.parse`）：
///   * 不是 JSON / 顶层不是对象 / 没有 `version` / `version` 不认识 /
///     `entries` 不是数组 ⇒ **整份当空**（这些都是"这份文件不是我们写的"）；
///   * 某一行不是对象、或没有非空的 `code`（它是主键）⇒ **只丢那一行**，
///     其余照留（一行坏了就让用户"重启后什么都没了"是过度反应）；
///   * 某一行的 `last_used_at` 解析不出来 ⇒ 该行**留着**，只是排到最末
///     （见 [`used_at_key`]：它是排序键，不是主键）。
///
/// ⚠️ **前三条走 `Err`**（⇒ [`JsonFile::read`] 给 `None` ⇒ [`History::load`] 给空历史），
///    **后两条走"只丢那一行"**（正常的 `Ok`）。两条路的终点一样、路径不同，别合并。
///    这正是 macOS 侧"用 `JSONSerialization` 而不是 `Codable`"那条注释的判据：
///    合成的解码器遇到一个类型不对的字段就**整份**失败，而这里要的是逐行容错。
impl<'de> Deserialize<'de> for History {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            version: i64,
            entries: Vec<serde_json::Value>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.version != Self::VERSION {
            return Err(serde::de::Error::custom(format!(
                "history.json 的 version 是 {}，本壳只认 {}：整份当空（E-1）",
                raw.version,
                Self::VERSION
            )));
        }
        let rows: Vec<HistoryEntry> = raw
            .entries
            .iter()
            .filter_map(|row| serde_json::from_value::<HistoryEntry>(row.clone()).ok())
            .collect();
        // ⚠️ 走 `new` 而不是直接装：读进来的那份也要**排好序、去过重、截到上限**
        //    （对齐 macOS `BatchHistory(entries:)` —— 磁盘上那份可能是手改过的）。
        Ok(History::new(rows))
    }
}

// ---------------------------------------------------------------------------
// 排序键：`last_used_at` 的原文 → 可比较的瞬间
// ---------------------------------------------------------------------------

/// 解析不出来时的键：**最小**（排到最末，也就是最先被淘汰的位置）。
///
/// 对齐 macOS `BatchHistory.usedAt` 的 `.distantPast`。
const UNPARSEABLE: (i64, u32) = (i64::MIN, 0);

/// `last_used_at` → `(UTC 秒, 纳秒)`。**解析不出来 ⇒ [`UNPARSEABLE`]**（排到最末）。
///
/// 对齐 macOS `BatchHistory.usedAt(_:)`：它先按"秒级带偏移"试一次，再按
/// "带小数秒"试一次，都不成才 `.distantPast`。这里一次认完两种写法。
///
/// ⚠️ **为什么不是字符串直接比大小**：本壳自己写出去的确实是同格式同偏移
///    （`+08:00`、秒级）的一串，那时字典序与时间序恰好一致 —— 但 `last_used_at`
///    也会从**清单的 `created_at`** 那一路进来（实测带小数秒，且偏移不保证是 `+08:00`）。
///    两个不同偏移的串按字典序比会排错（`…T09:12:00+08:00` 早于 `…T02:12:00Z`，
///    而字典序说相反）⇒ 那会变成"历史里最近用的那条不是最近用的"这种**静默**错误。
///
/// ⚠️ 只认 `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` 这一族（ISO 8601 的这两支）；
///    别的写法一律排到最后。**日期部分只做范围粗检**（`1..=31`），不做日历校验 ——
///    排序键只需要"同格式的串能比出正确的先后"，而 `2026-02-31` 这种输入
///    在任何实现下都是垃圾进垃圾出（macOS 那边同样会 `.distantPast`）。
///
/// ⚠️ 尾随多余字符**不判错**（手改过的文件末尾多一个换行不该让它排到最末）；
///    前缀必须逐字合法，否则就是 `UNPARSEABLE`。
fn used_at_key(raw: &str) -> (i64, u32) {
    let b = raw.as_bytes();
    // 最短的合法形状："YYYY-MM-DDTHH:MM:SSZ" = 20 字节。
    if b.len() < 20 {
        return UNPARSEABLE;
    }
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return UNPARSEABLE;
    }
    if !(b[10] == b'T' || b[10] == b't') {
        return UNPARSEABLE;
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        digits(b, 0, 4),
        digits(b, 5, 2),
        digits(b, 8, 2),
        digits(b, 11, 2),
        digits(b, 14, 2),
        digits(b, 17, 2),
    ) else {
        return UNPARSEABLE;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60
    {
        return UNPARSEABLE;
    }

    // 小数秒（可选）：秒后面那一段，折成纳秒。
    let mut at = 19;
    let mut nanos: u32 = 0;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return UNPARSEABLE; // 一个光秃秃的小数点不是时间
        }
        let frac = &b[start..at];
        let take = frac.len().min(9);
        let mut value: u32 = 0;
        for digit in &frac[..take] {
            value = value * 10 + u32::from(digit - b'0');
        }
        for _ in take..9 {
            value *= 10;
        }
        nanos = value;
    }

    // 偏移（必需）：`Z` 或 `±HH:MM`。
    let offset_seconds = match b.get(at) {
        Some(&c) if c == b'Z' || c == b'z' => 0,
        Some(&c) if c == b'+' || c == b'-' => {
            if b.get(at + 3) != Some(&b':') {
                return UNPARSEABLE;
            }
            let (Some(off_hour), Some(off_minute)) = (digits(b, at + 1, 2), digits(b, at + 4, 2))
            else {
                return UNPARSEABLE;
            };
            let magnitude = off_hour * 3600 + off_minute * 60;
            if c == b'+' { magnitude } else { -magnitude }
        }
        _ => return UNPARSEABLE,
    };

    (
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_seconds,
        nanos,
    )
}

/// 从 `at` 开始取 `n` 位十进制数。**越界或非数字 ⇒ `None`**（绝不 panic、绝不截断）。
fn digits(bytes: &[u8], at: usize, n: usize) -> Option<i64> {
    let end = at.checked_add(n)?;
    let slice = bytes.get(at..end)?;
    let mut value: i64 = 0;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

/// 公历年月日 → **自 1970-01-01 起的天数**（霍华德·辛南特的 `days_from_civil`）。
///
/// ⚠️ 自己写而不是引日期库：本 crate 的直接依赖白名单只有 `serde` / `serde_json`
///    （`windows/scripts/test.sh` 的守卫会拦），而这里需要的只是"同格式的串能比出先后"。
///    这个算法是**纯整数、无查表、无闰年分支**的（把三月当岁首之后，闰年那条规则
///    退化成 `yoe/4 - yoe/100` 两项），1970 年之前同样成立。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    //! ⚠️ 下面几句断言里的用字（「记录」「批注」「新添」「写的先后」「丢掉」「最新」）
    //!    **不是口味问题** —— 内嵌字体子集的覆盖判据连**测试字面量**一起算，
    //!    更顺口的那些词各有一两个字不在子集里。理由与判据见 `storage/mod.rs` 模块头
    //!    那一节"本模块字面量里的用字"。**改这些词之前先跑 `make_font_subset.sh --check`。**
    use super::*;
    use crate::storage::test_support::TempDir;

    fn an_entry(code: &str, at: &str) -> HistoryEntry {
        HistoryEntry {
            code: code.to_string(),
            note: String::new(),
            base_url: String::new(),
            last_used_at: at.to_string(),
        }
    }

    /// 一串**递增**的时间戳（模拟"一次次地用"）。
    fn at(second: u32) -> String {
        format!("2026-09-18T09:{second:02}:00+08:00")
    }

    /// 磁盘形状（**字段名逐字**，规格 §5.2）：`version` + `entries`，
    /// 每条是 `code` / `note` / `base_url` / `last_used_at` 四格。
    #[test]
    fn the_disk_shape_uses_the_macos_field_names_verbatim() {
        let mut entry = an_entry("C24-8", "2026-09-18T09:12:00+08:00");
        entry.note = "客户名".to_string();
        entry.base_url = "https://d.example".to_string();
        let v = serde_json::to_value(History::new(vec![entry])).expect("一定能序列化");
        assert_eq!(
            v,
            serde_json::json!({
                "version": 1,
                "entries": [{
                    "code": "C24-8",
                    "note": "客户名",
                    "base_url": "https://d.example",
                    "last_used_at": "2026-09-18T09:12:00+08:00",
                }],
            })
        );
    }

    /// ⚠️ 判据 ③：**超过 50 条 ⇒ 截断最旧的**。
    #[test]
    fn going_over_the_cap_evicts_the_oldest() {
        // 55 条，时间递增 ⇒ 最旧的是 `E00`…`E04` 那五条。
        let many: Vec<HistoryEntry> =
            (0..55).map(|i| an_entry(&format!("E{i:02}"), &at(i))).collect();
        let history = History::new(many);
        assert_eq!(history.entries.len(), History::MAXIMUM_ENTRIES);
        assert_eq!(History::MAXIMUM_ENTRIES, 50, "上限是 50（规格 §5.2 明写）");
        assert_eq!(history.entries.first().map(|e| e.code.as_str()), Some("E54"), "最新的在最前");
        assert_eq!(history.entries.last().map(|e| e.code.as_str()), Some("E05"), "截掉的必须是最旧的");
        assert!(
            !history.entries.iter().any(|e| e.code == "E04"),
            "最旧的那几条没有被丢掉：{:?}",
            history.entries
        );
        // ⚠️ 淘汰**在每一次 `new` 时**发生，不是只在装载时：`History::new` 是**唯一**
        //    那一处维持不变量的地方（写路径也一样 —— 呈现层的每一次改动都以它结尾）。
        let again = History::new(history.entries.clone());
        assert_eq!(again.entries.len(), History::MAXIMUM_ENTRIES, "再过一遍仍然是最多 50 条");
    }

    /// 去重是**按 `code`**，且同码留**最近**那条（磁盘上那份可能是手改过的）。
    #[test]
    fn duplicate_codes_are_collapsed_to_the_most_recent_one() {
        let history = History::new(vec![
            an_entry("AAA", &at(5)),
            an_entry("AAA", &at(1)),
            an_entry("BBB", &at(3)),
        ]);
        assert_eq!(history.entries.len(), 2, "{:?}", history.entries);
        assert_eq!(history.entries[0].code, "AAA");
        assert_eq!(history.entries[0].last_used_at, at(5), "留下的该是最新那条");
    }

    /// 空码的行**进不来**（它只会变成历史列表里一个点了没反应的假条目）。
    ///
    /// ⚠️ 另一半（"空码的**写入**是空操作"）在呈现层那一份里
    /// （`presentation::batch_history::BatchHistory::recording` 的 `guard`），
    /// 端到端那一条在 `api::history` 的 `a_use_with_an_empty_code_is_a_no_op`。
    #[test]
    fn an_empty_code_is_dropped() {
        let history = History::new(vec![an_entry("", &at(1)), an_entry("AAA", &at(2))]);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].code, "AAA");
    }

    /// ⚠️ **时间戳解析不出来 ⇒ 那一行留着，只是排到最末**（它是排序键，不是主键）。
    #[test]
    fn an_unparseable_timestamp_keeps_the_row_but_sorts_it_last() {
        let history = History::new(vec![
            an_entry("BROKEN", "不是时间"),
            an_entry("AAA", &at(1)),
        ]);
        assert_eq!(history.entries.len(), 2, "坏时间戳不该让那一行消失：{:?}", history.entries);
        assert_eq!(history.entries.last().map(|e| e.code.as_str()), Some("BROKEN"));
        assert_eq!(history.entries.first().map(|e| e.code.as_str()), Some("AAA"));
    }

    /// 排序键认得 macOS 认的那两种写法（秒级 + 小数秒），且**偏移要参与换算**。
    #[test]
    fn the_sort_key_understands_both_iso_shapes_and_the_offset() {
        // 秒级 vs 带小数秒：小数秒更大。
        assert!(used_at_key("2026-09-18T09:12:01+08:00") > used_at_key("2026-09-18T09:12:00.805751+08:00"));
        // 偏移参与换算：`+08:00` 的 09:00 是 UTC 01:00，而 `Z` 的 02:00 是 UTC 02:00 ⇒ 后者更晚。
        assert!(used_at_key("2026-09-18T09:00:00+08:00") < used_at_key("2026-09-18T02:00:00Z"));
        // 解析不出来的一律是同一个最小值（排到最后、最先被淘汰）。
        for bad in ["", "不是时间", "2026-09-18", "2026-09-18T09:12:00", "2026-13-18T09:12:00Z"] {
            assert_eq!(used_at_key(bad), UNPARSEABLE, "这个输入不该解析成功：{bad}");
        }
        // 尾随多余字符不判错（手改过的文件末尾多一个换行）。
        assert_ne!(used_at_key("2026-09-18T09:12:00+08:00\n"), UNPARSEABLE);
    }

    /// 写出去、读回来是同一份（`load` / `save` 走同一条形状）。
    #[test]
    fn a_saved_history_reads_back_the_same() {
        let dir = TempDir::new("hist-roundtrip");
        let path = dir.path().join(FILE_NAME);
        let history = History::new(vec![an_entry("AAA", &at(2)), an_entry("BBB", &at(1))]);
        history.save(&path).expect("写盘要成功");
        assert_eq!(History::load(&path), history);
    }

    /// ⚠️ **E-1**：文件不在、内容坏了 —— 都是"空历史"，**不是错误**（所以没有 `Result`）。
    #[test]
    fn a_missing_or_broken_file_is_an_empty_history_not_an_error() {
        let dir = TempDir::new("hist-e1");
        assert_eq!(History::load(&dir.path().join(FILE_NAME)), History::empty());
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        assert_eq!(History::load(&dir.path().join(FILE_NAME)), History::empty());
    }

    /// ⚠️ **不认识的 `version` ⇒ 整份当空历史**（E-1）。
    #[test]
    fn an_unknown_version_falls_back_to_an_empty_history() {
        let dir = TempDir::new("hist-version");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, br#"{"version": 2, "entries": [{"code": "AAA"}]}"#).unwrap();
        assert_eq!(History::load(&path), History::empty(), "不认识的版本必须当空记录");
        // ⚠️ 反方向：版本是 1 时那些条目**要被读进来**（否则上面那条可以靠"永远返回空"变绿）。
        std::fs::write(&path, br#"{"version": 1, "entries": [{"code": "AAA"}]}"#).unwrap();
        assert_eq!(History::load(&path).entries.len(), 1);
    }

    /// ⚠️ **逐行容错**：某一行坏了**只丢那一行**，其余照留
    /// （一行坏了就让用户"重启后什么都没了"是过度反应 —— macOS 那条注释的原话）。
    #[test]
    fn one_bad_row_does_not_take_the_whole_history_down() {
        let dir = TempDir::new("hist-rows");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            r#"{"version": 1, "entries": [
                {"code": "GOOD", "last_used_at": "2026-09-18T09:01:00+08:00"},
                "这不是一个对象",
                {"note": "没有 code"},
                {"code": "", "note": "空码"},
                {"code": 7},
                {"code": "ALSO_GOOD", "note": "批注"}
            ]}"#,
        )
        .unwrap();
        let history = History::load(&path);
        assert_eq!(
            history.entries.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec!["GOOD", "ALSO_GOOD"],
            "只有那两条合法的该留下：{:?}",
            history.entries
        );
        // 缺 `last_used_at` 的那一行留着（排到最末），缺 `note` / `base_url` 的按空串补。
        assert_eq!(history.entries[1].note, "批注");
        assert_eq!(history.entries[0].base_url, "");
    }

    /// `entries` 那一格缺失或不是数组 ⇒ **整份当空**（"这份文件不是我们写的"）。
    #[test]
    fn a_missing_or_mistyped_entries_array_is_an_empty_history() {
        let dir = TempDir::new("hist-shape");
        let path = dir.path().join(FILE_NAME);
        for body in [r#"{"version": 1}"#, r#"{"version": 1, "entries": {}}"#, r#"{"entries": []}"#] {
            std::fs::write(&path, body).unwrap();
            assert_eq!(History::load(&path), History::empty(), "这份内容该整份当空：{body}");
        }
    }

    /// `most_recent` 是"上次用的码"的**唯一**来源（E-4）。
    #[test]
    fn the_most_recent_entry_is_the_first_one() {
        assert!(History::empty().most_recent().is_none());
        let history = History::new(vec![an_entry("AAA", &at(1)), an_entry("BBB", &at(7))]);
        assert_eq!(history.most_recent().map(|e| e.code.as_str()), Some("BBB"));
    }

    /// ⚠️ 空串 `base_url` ⇒ `None`（请求里**不出现** `base_url` 这个键，
    ///    由内核用自己的默认交付服务器）。对齐 macOS `baseURLOrNil`。
    #[test]
    fn an_empty_base_url_means_use_the_kernels_default() {
        assert_eq!(an_entry("AAA", &at(1)).base_url_or_nil(), None);
        let mut custom = an_entry("AAA", &at(1));
        custom.base_url = "https://d.example".to_string();
        assert_eq!(custom.base_url_or_nil(), Some("https://d.example"));
    }

    /// ⚠️ **写失败要能到调用方手里**（E-2 的下半条）：历史存不下来是一件用户能感知的事
    ///    （重启后什么都没了），调用方必须有机会说一句。
    #[test]
    fn a_failed_save_reports_the_failure_instead_of_swallowing_it() {
        let dir = TempDir::new("hist-savefail");
        let path = dir.path().join(FILE_NAME);
        History::new(vec![an_entry("AAA", &at(1))]).save(&path).expect("先写一份好的");
        std::fs::create_dir(dir.path().join(format!(".{FILE_NAME}.tmp"))).unwrap();
        assert!(History::empty().save(&path).is_err(), "写盘失败必须报给调用方");
        assert_eq!(History::load(&path).entries.len(), 1, "失败的写把旧记录改掉了");
    }
}
