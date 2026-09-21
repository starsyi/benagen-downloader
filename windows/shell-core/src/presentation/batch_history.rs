//! batch_history —— 换码面板的**批次历史**：历史本身（增 / 去重 / 排序 / 淘汰 / 解析）、
//! 历史列表的**行呈现**、以及备注编辑框的**草稿判决**。
//!
//! 上游是三份 Swift（本模块把三份合成一个文件，理由见下）：
//!
//!   | 本文件里的东西 | 上游文件 |
//!   |---|---|
//!   | [`BatchHistoryEntry`] / [`BatchHistory`] | `macos/Sources/BenagenCoreKit/Presentation/BatchHistory.swift` |
//!   | [`BatchHistoryRow`] | `macos/Sources/BenagenCoreKit/Presentation/BatchHistoryRow.swift` |
//!   | [`NoteDrafts`] | `macos/Sources/BenagenCoreKit/Presentation/NoteDrafts.swift` |
//!
//! ⚠️ **为什么三份合成一个文件**（控制者给的划分，不是随手合并）：三者在调用点上
//!    是**同一件事的三段**（历史是值、行是它的呈现、草稿是它的编辑态），而
//!    `BatchHistoryRow.swift` 的文档注释**每一段都在往回指 `BatchHistory` 的不变量**
//!    （"顺序是 `BatchHistory` 的不变量，行层只做映射"）。放在一个文件里，
//!    "谁持有哪条不变量"读一遍就清楚；拆成三个文件只会让人来回跳。
//!    三份上游文件的**判据一条都没少**（见每条测试上方的上游测试名）。
//!
//! ⚠️ **本模块不做**下面这些事（各有其归属，重复一次就是同一条规则写两遍）：
//!   · 排序 / 去重 / 淘汰 —— [`BatchHistory`] 的不变量，行层**原样沿用它的顺序**；
//!   · 备注的归一化 —— [`BatchHistory::setting_note`] 负责"写进去的那一份"，
//!     行层只做**显示口径**上的兜底（见 [`BatchHistoryRow::of`]）；
//!   · 时间格式化 —— 复用既有的 `TimestampPresentation`，本模块不另造一套
//!     （上游 `theHistoryTimeColumnReusesTheSharedTimestampPresentation` 钉着它）。
//!
//! ---------------------------------------------------------------------------
//! ⚠️ **形态偏离（W-6）逐条记账** —— 每一条都是"Rust 没有对应的语言设施 / 本 crate 的
//!    依赖白名单锁死"，不是"顺手改一下"：
//!
//!   1. **时间源的类型**：上游是 `recording(…, at date: Date)`（时钟由调用方给）。
//!      本 crate 的直接依赖锁在 `serde` + `serde_json`（`windows/scripts/test.sh` 有守卫），
//!      **不许**引 `chrono`/`time`/`jiff`，**也不许**去改那条白名单。所以这里的参数是
//!      **Unix 秒（`i64`）** —— 调用方 `SystemTime::now().duration_since(UNIX_EPOCH)`
//!      一下就有，"时钟由调用方给"这条性质一字不变（本模块自己不读时钟）。
//!      `Date` → 固定 `+08:00` 的 ISO8601 原文那一步（上游交给 `ISO8601DateFormatter`）
//!      由 [`BatchHistory::timestamp`] **用手写算法**实现，点名 Howard Hinnant 的
//!      `civil_from_days`（见那个函数的注释）。
//!   2. **`serialized()` 的返回类型**：上游 `throws -> Data`，这里
//!      `-> Result<String, serde_json::Error>`（本 crate 的存储层直接吃字符串）。
//!      **保留 `Result`**：上游那个 `throws` 的存在本身就说明"这一步不许静默回落成空"，
//!      而 `unwrap_or_default()` 正是本项目最恨的那种静默降级。
//!   3. **`SourceTimeText` 的复用**：上游 [`BatchHistoryRow`] 直接调
//!      `SourceTimeText.of(_:)`（同一个模块里够得着）。本 crate 里那个类型是
//!      `browser_row.rs` 的**私有项**（`enum SourceTimeText`，没有 `pub`），
//!      而本波次**不许改既有文件** ⇒ 这条判据在这里**抄了第二遍**
//!      （[`BatchHistoryRow::time_text`]）。**抄的那一份由一条关系断言钉住**
//!      （`the_history_time_column_reuses_the_shared_timestamp_presentation`：
//!      它与 `TimestampPresentation::text` 必须是同一个答案）——
//!      第二个真相源靠测试绑回第一个。
//!   4. **`id`**：上游 `Identifiable` 要求 `var id: String { code }`。这里留成
//!      [`BatchHistoryRow::id`]（不在线上形状里，`code` 已经在）——它是"码是唯一键"
//!      这条不变量的可执行写法。
//!
//! ⚠️ 上一条纪律的**反面**同样成立：**不许凭类型名猜语义**。本文件每一条判据、
//!    每一句文案都逐字对着上游源码与上游单测抄，测试上方点名它对应哪一条。
//!
//! ⚠️ **本模块不碰文件系统**（那是存储层的事）：这里只有纯计算，
//!    所以它**不可能**碰到用户真实的 `history.json`。

use crate::presentation::delivery_summary::{DeliveryCodeEntry, TimestampPresentation};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// 历史里的一条
// ---------------------------------------------------------------------------

/// 历史里的一条：一个用过的交付码 + 一句备注 + 它是从哪个交付服务器加载的 + 上次用的时间。
///
/// 上游 `BatchHistory.swift` 的 `BatchHistoryEntry`。
///
/// ⚠️ `code` 是**唯一键**：同码只留一条，再次使用是**更新**而不是新增。
/// ⚠️ `last_used_at` 保存的是**原文**（ISO 8601 带 `+08:00`，秒级），与交付清单的
///    `created_at` 同形 —— 本模块**不把它解析成时间类型再格式化回去**（那会在往返里
///    丢掉原文的形状），显示一律复用既有的 `TimestampPresentation`。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct BatchHistoryEntry {
    pub code: String,
    /// 一句自由文本。**空串合法** = 没写备注。
    pub note: String,
    /// 这一批是从哪个交付服务器加载的。**空串 = 默认服务器**。
    pub base_url: String,
    /// ISO 8601 带偏移的原文，与清单的 `created_at` 同形。
    pub last_used_at: String,
}

impl BatchHistoryEntry {
    /// 要发给内核的那个值：**空串 ⇒ `None`**（请求里不出现 `base_url` 这个键，
    /// 由内核用自己的默认交付服务器）。
    ///
    /// ⚠️ 这一条是**承重的**：从自定义交付服务器加载过的批次，如果再点一次却去问
    ///    **默认**服务器，就会拿到"码不存在"之类的失败。`None` = 请求里**不出现**
    ///    `base_url` 这个键 —— 与"不要显式传一个和默认一样的值"是同一条纪律
    ///    （那会在内核默认值变化时静默分叉）。
    ///
    /// 上游 `BatchHistoryEntry.baseURLOrNil`。
    pub fn base_url_or_nil(&self) -> Option<&str> {
        if self.base_url.is_empty() {
            None
        } else {
            Some(self.base_url.as_str())
        }
    }
}

// ---------------------------------------------------------------------------
// 历史（值类型，不可变）
// ---------------------------------------------------------------------------

/// 一批历史记录（值类型，不可变）。
///
/// 上游 `BatchHistory.swift` 的 `BatchHistory`。
///
/// **不变量**（构造函数与每次修改都维持它）：
///   ① `entries` 按 `last_used_at` **倒序**（最近用的在最前）；
///   ② `code` 唯一；
///   ③ 条数 ≤ [`BatchHistory::MAXIMUM_ENTRIES`]（超出淘汰**最久未用**的）。
///
/// ⚠️ **不变量在这里维持、而不是留给调用方**：换码面板只负责把 `entries` 从上到下画出来。
///    一旦"顺序"变成调用方的责任，第二个读这个类型的地方就得自己再排一遍 ——
///    那正是同一条规则写两遍的开始。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct BatchHistory {
    pub entries: Vec<BatchHistoryEntry>,
}

impl BatchHistory {
    /// 给未来的自己的版本号：**读不出来或不认识 ⇒ 当空历史**。
    ///
    /// 上游 `BatchHistory.version`。
    pub const VERSION: i64 = 1;

    /// 条数上限（上游明写 50）。无限增长的文件是留给下一个人踩的雷。
    ///
    /// 上游 `BatchHistory.maximumEntries`。
    pub const MAXIMUM_ENTRIES: usize = 50;

    /// 交付服务器的固定偏移（`+08:00`）。
    ///
    /// ⚠️ **有意不用本机时区**：交付清单的 `created_at` 是 `+08:00` 形状的，而
    ///    `TimestampPresentation::text` 显示时**丢掉偏移、读的是墙上时间** ——
    ///    若按本机时区写，同一个文件在两个时区的机器上会显示成两个时间
    ///    （"上次使用"是客户与业务方对账的依据，不能随机器的设置变）。
    ///    固定偏移也让测试与机器时区无关。
    ///
    /// 上游 `BatchHistory.deliveryServerOffsetSeconds`。
    pub const DELIVERY_SERVER_OFFSET_SECONDS: i64 = 8 * 3600;

    /// 空历史。**它不是错误态** —— 第一次运行、文件被删、文件坏了都是它。
    ///
    /// 上游 `BatchHistory.empty`。
    pub const fn empty() -> BatchHistory {
        BatchHistory {
            entries: Vec::new(),
        }
    }

    /// 归一化：排倒序 → 去重（同码留最近那条）→ 截到上限。
    ///
    /// 上游 `BatchHistory.init(entries:)`。
    ///
    /// ⚠️ 排序**稳定**（同刻的两条保持传入的相对次序）：拿一个不保证稳定的排序会让
    ///    "同一秒里用的两个码"顺序随机，而那是可观察的。Rust 的 `sort_by` 本身就是
    ///    稳定排序，所以这里不需要上游那句显式的 `offset` 兜底 —— 语义一致。
    pub fn new(entries: Vec<BatchHistoryEntry>) -> BatchHistory {
        let mut ordered = entries;
        ordered.sort_by(|a, b| Self::used_at(b).cmp(&Self::used_at(a)));

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut kept: Vec<BatchHistoryEntry> = Vec::with_capacity(ordered.len());
        for entry in ordered {
            // 空码不是一条交付批次：放进去只会变成列表里一个点了没反应的假条目。
            if entry.code.is_empty() {
                continue;
            }
            if !seen.insert(entry.code.clone()) {
                continue;
            }
            kept.push(entry);
        }
        kept.truncate(Self::MAXIMUM_ENTRIES);
        BatchHistory { entries: kept }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// **"上次用的码"的唯一来源**：历史里最近使用的那一条。
    ///
    /// 上游 `BatchHistory.mostRecent`。
    pub fn most_recent(&self) -> Option<&BatchHistoryEntry> {
        self.entries.first()
    }

    /// 记下一次**成功**的使用（"同码只留一条，再次使用 ⇒ 更新"）。
    ///
    /// 上游 `BatchHistory.recording(code:baseURL:note:at:)`。
    ///
    /// - `note`：`None` = **保持这一条原有的备注**。这一条是承重的：
    ///   用户写下的"客户张三"不该因为又用了一次这个码而消失。`Some("")`
    ///   则是显式把它清空。
    /// - `at_unix_seconds`：由调用方给（本模块是纯函数，不读时钟）。
    pub fn recording(
        &self,
        code: &str,
        base_url: &str,
        note: Option<&str>,
        at_unix_seconds: i64,
    ) -> BatchHistory {
        // 空码不是一条交付批次：放进去只会变成历史列表里一个点了没反应的假条目。
        if code.is_empty() {
            return self.clone();
        }
        let kept_note = match note {
            Some(raw) => Self::normalized_note(raw),
            // ⚠️ 备注**不在**被更新的那批里（见上面那条）。
            None => self
                .entries
                .iter()
                .find(|e| e.code == code)
                .map(|e| e.note.clone())
                .unwrap_or_default(),
        };
        let fresh = BatchHistoryEntry {
            code: code.to_string(),
            note: kept_note,
            base_url: base_url.to_string(),
            last_used_at: Self::timestamp(at_unix_seconds),
        };
        // 其余条目原样留下（它们的相对次序由 `new` 的排序保证）。
        let mut entries = vec![fresh];
        entries.extend(self.entries.iter().filter(|e| e.code != code).cloned());
        BatchHistory::new(entries)
    }

    /// 改一条的备注（就地编辑那一行）。
    ///
    /// 上游 `BatchHistory.settingNote(_:forCode:)`。
    ///
    /// ⚠️ 对一个**不在历史里**的码设备注是**空操作**，不会凭空造出一条没有时间戳的记录
    ///    （历史列表只对屏上那些行提供编辑）。清空备注是传空串，**不是**删掉这一条。
    pub fn setting_note(&self, note: &str, for_code: &str) -> BatchHistory {
        if !self.entries.iter().any(|e| e.code == for_code) {
            return self.clone();
        }
        let normalized = Self::normalized_note(note);
        BatchHistory::new(
            self.entries
                .iter()
                .map(|e| {
                    if e.code == for_code {
                        BatchHistoryEntry {
                            code: e.code.clone(),
                            note: normalized.clone(),
                            base_url: e.base_url.clone(),
                            last_used_at: e.last_used_at.clone(),
                        }
                    } else {
                        e.clone()
                    }
                })
                .collect(),
        )
    }

    /// 从文件内容解析（**任何**问题都当空历史，绝不抛）。
    ///
    /// 上游 `BatchHistory.parse(_:)`（两个重载：`Data` 与 `String`；Rust 这一侧
    /// 只留字符串那一个 —— 存储层读出来的就是字符串）。
    ///
    /// 逐层容错：
    ///   · 不是 JSON / 顶层不是对象 / 没有 `version` / `version` 不等于 [`Self::VERSION`]
    ///     / `entries` 不是数组 ⇒ **整份当空**（这些都是"这份文件不是我们写的"）；
    ///   · 某一行不是对象、或没有非空的 `code`（它是主键）⇒ **只丢那一行**，
    ///     其余照留（一行坏了就让用户"重启后什么都没了"是过度反应）；
    ///   · 某一行的 `last_used_at` 解析不出来 ⇒ 该行**留着**，只是排到最末
    ///     （它是排序键，不是主键）。
    ///
    /// ⚠️ 用 `serde_json::Value` 逐字段看而不是 `#[derive(Deserialize)]`：
    ///    派生出来的解码器遇到一个类型不对的字段就**整份**失败，而这里要的恰恰是
    ///    "逐行容错"（见上）。上游用 `JSONSerialization` 的理由与此逐字同源。
    pub fn parse(text: &str) -> BatchHistory {
        if text.is_empty() {
            return BatchHistory::empty();
        }
        let Ok(root) = serde_json::from_str::<Value>(text) else {
            return BatchHistory::empty();
        };
        let Value::Object(map) = root else {
            return BatchHistory::empty();
        };
        // `version` 必须是**数**且**正好等于**我们认识的那个（字符串 `"1"` 不算）。
        let Some(Value::Number(version)) = map.get("version") else {
            return BatchHistory::empty();
        };
        if version.as_i64() != Some(Self::VERSION) {
            return BatchHistory::empty();
        }
        let Some(Value::Array(rows)) = map.get("entries") else {
            return BatchHistory::empty();
        };
        BatchHistory::new(rows.iter().filter_map(Self::entry_of).collect())
    }

    /// 落盘的形状（字段名逐字：`version` / `entries` / `code` / `note` /
    /// `base_url` / `last_used_at`）。
    ///
    /// 上游 `BatchHistory.serialized()`。
    ///
    /// ⚠️ 空串字段（`note` / `base_url`）**照样写出去**：形状是固定的，
    ///    "键有时在有时不在"会让下一个读这个文件的人多写一个分支。
    ///
    /// ⚠️ 键**排序输出**（`serde_json::Map` 默认是 `BTreeMap`，没有开 `preserve_order`）
    ///    —— 与上游那句 `.sortedKeys` 同一个口径；`serde_json` 也**不转义 `/`**
    ///    （上游那个 `.withoutEscapingSlashes` 的口径）。
    pub fn serialized(&self) -> Result<String, serde_json::Error> {
        let rows: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "code": e.code,
                    "note": e.note,
                    "base_url": e.base_url,
                    "last_used_at": e.last_used_at,
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({
            "version": Self::VERSION,
            "entries": rows,
        }))
    }

    /// 线上的一行 → 一条历史；不是对象 / 没有非空 `code` ⇒ `None`（**只丢那一行**）。
    ///
    /// 上游 `BatchHistory.entry(from:)`。
    ///
    /// ⚠️ 缺字段（`note` / `base_url` / `last_used_at`）与"字段是空串"**同解**，
    ///    与上游逐字一致；**类型不对**（比如 `note` 是个数）同样回落到空串 ——
    ///    这是"逐行容错"的一部分，不是宽容过了头：`code` 那一条仍然是硬的。
    fn entry_of(any: &Value) -> Option<BatchHistoryEntry> {
        let Value::Object(row) = any else {
            return None;
        };
        let code = match row.get("code") {
            Some(Value::String(code)) if !code.is_empty() => code.clone(),
            _ => return None,
        };
        let string_or_empty = |key: &str| match row.get(key) {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        Some(BatchHistoryEntry {
            code,
            note: string_or_empty("note"),
            base_url: string_or_empty("base_url"),
            last_used_at: string_or_empty("last_used_at"),
        })
    }

    /// Unix 秒 → ISO 8601 带 `+08:00`、**秒级**（与清单的 `created_at` 同形）。
    ///
    /// 上游 `BatchHistory.timestamp(_:)`（那里由 `ISO8601DateFormatter` 代劳）。
    ///
    /// 🔴 **手写，不引依赖**（见模块头第 1 条）：`Date` → 年月日那一步用的是
    ///    **Howard Hinnant 的 `civil_from_days`**（`chrono`/`time`/`jiff` 内部用的也是
    ///    这一套；1970-01-01 起的天数反推公历年月日，对格里高利历的闰年规则完全正确）。
    ///    偏移是**固定的** `+08:00`（[`Self::DELIVERY_SERVER_OFFSET_SECONDS`]），
    ///    所以这里没有时区数据库、也没有"夏令时"这类会随机器变的东西。
    ///
    /// ⚠️ 两个除法都用 `div_euclid` / `rem_euclid`（**向下取整**）而不是 `/` 与 `%`
    ///    （向零截断）：1970 年之前的时刻（负数秒）用后者会算错一天。
    ///
    /// 🔴 **它是 `pub`**：落盘那条链路（`history_put` 那条命令）要用它把"现在"
    ///    写成 `last_used_at`。上游的 `recording(at:)` 收的是一个 `Date`、时间戳的生成
    ///    藏在它内部；这里把生成器单独交出来，是因为本 crate 里**秒数的来源在命令层**
    ///    （`SystemTime::now().duration_since(UNIX_EPOCH)`）——
    ///    "时钟由调用方给"这条性质不变，只是那个调用方有时想自己拿一次时间。
    pub fn timestamp(unix_seconds: i64) -> String {
        let shifted = unix_seconds + Self::DELIVERY_SERVER_OFFSET_SECONDS;
        let days = shifted.div_euclid(86_400);
        let secs = shifted.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+08:00",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    }

    /// 排序键。**解析不出来 ⇒ `i64::MIN`**（排到最末，最先被淘汰的位置）。
    ///
    /// 上游 `BatchHistory.usedAt(_:)`（那里回的是 `.distantPast`）。
    ///
    /// ⚠️ 每次调用现算、**不缓存**：这个纯函数没有状态可言，
    ///    缓存只会多一条"什么时候失效"的规则。
    fn used_at(entry: &BatchHistoryEntry) -> i64 {
        iso8601_to_unix(&entry.last_used_at).unwrap_or(i64::MIN)
    }

    /// 备注是"**一句**自由文本"：换行/回车/制表折成空格、首尾空白去掉。
    /// 内部原有的空格**一个都不动** —— 那是用户自己写的排版，本模块不替他重排。
    ///
    /// 上游 `BatchHistory.normalizedNote(_:)`。
    ///
    /// ⚠️ `\r\n`（Windows 换行）**折成一个空格**、不是两个：它是**一个**换行。
    ///    把粘贴进来的文本变成"每个换行多一个空格"是那种一眼看不出、却到处都是的脏数据。
    fn normalized_note(raw: &str) -> String {
        let unfolded = raw.replace("\r\n", "\n").replace('\r', "\n");
        let mut one_line = String::with_capacity(unfolded.len());
        for character in unfolded.chars() {
            match character {
                '\n' | '\t' => one_line.push(' '),
                other => one_line.push(other),
            }
        }
        one_line.trim().to_string()
    }
}

// ---------------------------------------------------------------------------
// 行呈现
// ---------------------------------------------------------------------------

/// 历史列表里的一行：**已经算好的界面值**。
///
/// 上游 `BatchHistoryRow.swift` 的 `BatchHistoryRow`。
///
/// 一行由三样东西组成：① `title`（备注有则显示、无则显示码）；② `code`
/// （交付码本身，**永远显示**，它是这一行的身份）；③ `time_text`（上次使用时间）。
/// 另有两样是**行为**要用的、但不上屏的值：`base_url`（点这一行要去问哪台服务器）
/// 与 [`Self::is_sendable`]（这一行能不能发出去）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct BatchHistoryRow {
    /// 交付码。**逐字**（不截断、不加省略号 —— 它是用户唯一能拿去对账的东西）。
    pub code: String,
    /// 备注原文（去掉首尾空白后）。**空串 = 没写备注**。
    pub note: String,
    /// 行首那一句：备注非空就是备注，否则回落到**码**。
    ///
    /// ⚠️ 回落是**这一行的全部意义**：没有它，一条没写备注的历史在列表里就是一行空白
    ///    （用户会以为它坏了，而不是以为"我还没写备注"）。
    pub title: String,
    /// 「上次使用」那一格。
    pub time_text: String,
    /// 这一条是从哪个交付服务器加载的（空串 = 默认服务器）。
    pub base_url: String,
}

impl BatchHistoryRow {
    /// 这一格没有值时的占位字形。
    ///
    /// ⚠️ 与 `SpeedFormat`/`PercentFormat`、批次摘要那一格是**同一个字形**（`—`）：
    ///    同一件事在壳里长得一样，用户才不用每次重新学一遍"这一格空着是什么意思"。
    ///
    /// 上游 `BatchHistoryRow.noValue`。
    pub const NO_VALUE: &'static str = "—";

    /// 一条历史条目 → 一行。
    ///
    /// 上游 `BatchHistoryRow.init(_ entry:)`。
    ///
    /// ⚠️ **只吃空白字符的备注与空串同等对待**：`"   "` 渲染出来是一行**看起来是空的**
    ///    标题，在界面上与"这一行坏了"分不开。壳自己写出去的备注全是归一化过的
    ///    （[`BatchHistory::setting_note`] / [`BatchHistory::recording`]），
    ///    所以这条兜底只对**手改过的文件**生效 —— 但那一份同样是真实的输入
    ///    （解析不会因为备注是空白就丢掉这一行）。
    ///    内部原有的空格**一个都不动**：那是用户自己写的排版。
    ///
    /// ⚠️ 备注与码**都**空时给占位符（纵深防御）：经解析那条正路造不出这一行
    ///    （空码在 [`BatchHistory::new`] 就被丢掉了），但本函数是 `pub`，
    ///    "标题是空串"这一行**在类型上构造得出来** —— 而一行空标题在界面上
    ///    与"这一行坏了"分不开。
    pub fn of(entry: &BatchHistoryEntry) -> BatchHistoryRow {
        // `str::trim()` 去掉的是 Unicode 的 `White_Space`（含 `\t`/`\n`/`\r`
        // 与各类全角空格如 U+3000），与上游 `whitespacesAndNewlines` 是**同一个集合**。
        let trimmed = entry.note.trim();
        BatchHistoryRow {
            code: entry.code.clone(),
            note: trimmed.to_string(),
            title: if trimmed.is_empty() {
                if entry.code.is_empty() {
                    Self::NO_VALUE.to_string()
                } else {
                    entry.code.clone()
                }
            } else {
                trimmed.to_string()
            },
            time_text: Self::time_text(&entry.last_used_at),
            base_url: entry.base_url.clone(),
        }
    }

    /// 一整份历史 → 屏上从上到下的那些行。
    ///
    /// 上游 `BatchHistoryRow.rows(_:)`。
    ///
    /// ⚠️ **只做映射，不排序也不截断**：顺序是 [`BatchHistory`] 的不变量，这里再排一次
    ///    就等于把同一条规则写第二遍。返回**空 `Vec`** 时调用方整段不渲染
    ///    （列表为空时不显示这一段，不要给空盒子）。
    pub fn rows(history: &BatchHistory) -> Vec<BatchHistoryRow> {
        history.entries.iter().map(Self::of).collect()
    }

    /// 码是唯一键 ⇒ 它就是这一行的身份。
    ///
    /// 上游 `BatchHistoryRow.id`（`Identifiable` 的要求）。
    pub fn id(&self) -> &str {
        &self.code
    }

    /// 点这一行时要发给内核的 `base_url`：**空串 ⇒ `None`**。
    ///
    /// 上游 `BatchHistoryRow.baseURLOrNil`（本模块把它下沉到
    /// [`BatchHistoryEntry::base_url_or_nil`]，两处是同一条判据）。
    pub fn base_url_or_nil(&self) -> Option<&str> {
        if self.base_url.is_empty() {
            None
        } else {
            Some(self.base_url.as_str())
        }
    }

    /// 这一行能不能发出去。
    ///
    /// 上游 `BatchHistoryRow.isSendable`。
    ///
    /// ⚠️ 为什么要有它：`history.json` 是**用户看得见、也改得动**的文件，而解析只要求
    ///    `code` 非空。一条超过 2 KiB 的码点下去，内核**不报错** —— 它只回一条
    ///    `id == 0` 的协议告警、那条请求**永远等不到响应**，客户端那条 FIFO 串行队列
    ///    于是被**永久堵死**。所以判据**复用** [`DeliveryCodeEntry::is_sendable`] 那一个
    ///    实现，绝不在这里另写一套长度判断。
    pub fn is_sendable(&self) -> bool {
        DeliveryCodeEntry::is_sendable(&self.code)
    }

    /// 「上次使用」那一格显示什么。
    ///
    /// 上游 `BatchHistoryRow.timeText(of:)`。
    ///
    /// 输入是**原文**（ISO 8601 带 `+08:00`），两种情形：
    ///   · 空串 ⇒ 占位符 `—`（"这一格没有值"本身也是一件要说出来的事，
    ///     渲染成空串就是什么都没说）；
    ///   · 其余 ⇒ **原样**交给 `TimestampPresentation::text`：它能认的显示
    ///     `2026-09-18 09:12`，认不出来的**原样返回** ——
    ///     所以壳**永远不会**显示 "Invalid Date" 这种自己编的文案。
    fn time_text(raw: &str) -> String {
        if raw.is_empty() {
            Self::NO_VALUE.to_string()
        } else {
            TimestampPresentation::text(raw)
        }
    }
}

// ---------------------------------------------------------------------------
// 备注草稿
// ---------------------------------------------------------------------------

/// 一条要交给历史的备注写入。
///
/// 上游 `NoteDrafts.Write`。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NoteWrite {
    pub code: String,
    pub note: String,
}

/// 一次提交的**判决结果**：要写哪几条 + 交完之后还剩哪些草稿。
///
/// 上游 `NoteDrafts.Outcome`。
///
/// ⚠️ 两个字段**必须一起用**：只取 `writes` 而忘了把 `drafts` 装回去，
///    编辑框里就会一直留着用户敲的原文（与真正存下去的那一份分叉）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NoteCommitOutcome {
    pub writes: Vec<NoteWrite>,
    pub drafts: NoteDrafts,
}

/// 历史列表里那些**还没交出去的备注草稿**。
///
/// 上游 `NoteDrafts.swift` 的 `NoteDrafts`。
///
/// ---------------------------------------------------------------------------
/// 🔴 **对位记账（R-5）：这个类型的判决在**生产路径上零调用者**，行为由前端承接**
/// ---------------------------------------------------------------------------
/// `api/**` 与 `shell-win/**` 里**没有一处**调它 —— 换码面板活在 JS 里
/// （`web/js/switchcode.js`），那个文件的文件头自己就写着"**本文件抄了
/// `presentation::batch_history::NoteDrafts` 一份**"，并如实记账"那四条规则**没有单测压着**"。
///
/// ⚠️ **这正是本代最严重那个缺陷的形状**：判据（连同写着坑名的文档）在 Rust 里、
///    实现被抄进 JS，**两边一分叉谁都不会红**（`ManifestTracking` 那一格就是这么长出来的，
///    最后在真机被用户撞到）。所以这一格现在**有对照判据**：
///    · 用例表由 Rust 倒出来（`shell-core/examples/dump_wire_fixtures.rs` 的
///      `mirrorCases.noteDrafts`，判决**现算**、不是手写）；
///    · 前端在无头 Chromium 里**回放同一组操作**，把发出去的 `history_put` 与
///      Rust 的判决逐条比（`windows/scripts/frontend-stub/panels-harness.html`）；
///    · 坐标与"仍然零调用者"这一句由 `windows/scripts/check_presentation_mirrors.sh` 守
///      （有人把这个类型接上了 ⇒ 那份账会红，逼着账跟着改）。
///    ⚠️ **改这个类型的判决就要想到那一份夹具**：`check_wire_fixtures.sh` 会先红
///      （夹具过期），重跑生成之后前端那一侧才会红 —— 那条顺序是**有意的**：
///      它保证"Rust 改了而 JS 没改"这件事**一定**会被看见。
///
/// 为什么整段搬出来（这个类型存在的全部理由）：这是换码面板里**语义最绕**的一段
/// —— "没草稿不写 / 没变不写 / 交完删草稿 / 草稿优先于现值 / 面板关闭时全部交出去"
/// 五条规则交织在一起，而它的落点原本是视图里的一个状态字典。
/// 抽成纯值类型之后每一条都能被单独钉住。
///
/// ⚠️ **提交时机留在调用方**（回车 / 失焦 / 面板关闭三个点），这里只回答
///    "**这一次**该不该写、写什么"。时机是渲染分派、判决是值计算。
///
/// ⚠️ 内部用 `BTreeMap`（本 crate 通行的有序容器）：不变量只有一条 —— **键是交付码**；
///    `BTreeMap` 顺带让"按码排序"这件事有一个天然的落点。
///
/// ⚠️ 值类型（每次编辑返回新值），所以"边遍历边改"这种 bug 在类型层面就不成立。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct NoteDrafts {
    by_code: BTreeMap<String, String>,
}

impl NoteDrafts {
    /// 一个草稿都没有。
    ///
    /// 上游 `NoteDrafts.init()`。
    pub fn new() -> NoteDrafts {
        NoteDrafts {
            by_code: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }

    /// 这一行现在挂着的草稿；`None` = 用户没动过它。
    ///
    /// 上游 `NoteDrafts.draft(forCode:)`。
    ///
    /// ⚠️ `None` 与 `Some("")` 是**两件事**：`Some("")` 是"用户把它清空了"
    ///    （一个真的草稿），`None` 是"用户没碰过"。混为一谈的后果是
    ///    **清空备注之后旧的那句会自己弹回来**。
    pub fn draft(&self, code: &str) -> Option<&str> {
        self.by_code.get(code).map(String::as_str)
    }

    /// 编辑框该显示什么：**草稿优先于现值**。
    ///
    /// 上游 `NoteDrafts.text(forCode:current:)`。
    ///
    /// 用"有没有这个键"接住（而不是判 `current` 空不空）—— 见 [`Self::draft`] 那条。
    pub fn text(&self, code: &str, current: &str) -> String {
        match self.by_code.get(code) {
            Some(draft) => draft.clone(),
            None => current.to_string(),
        }
    }

    /// 记下用户敲进去的字（返回新值）。
    ///
    /// 上游 `NoteDrafts.editing(_:forCode:)`。
    pub fn editing(&self, text: &str, code: &str) -> NoteDrafts {
        let mut next = self.by_code.clone();
        next.insert(code.to_string(), text.to_string());
        NoteDrafts { by_code: next }
    }

    /// 把**一个**码的草稿交出去。返回"要不要写、写什么"，以及交完之后的状态。
    ///
    /// 上游 `NoteDrafts.committing(code:current:)`。
    ///
    /// 三道闸，每一道都在挡一种具体的伤害：
    ///   ① **没有草稿** ⇒ 一个字节都不写。用户只是点了一下那一行（它会加载），
    ///      不是来改备注的 —— 每次点击都写一次盘是把"编辑"这件事变成噪音；
    ///   ② **这一行已经不在屏上了**（`current == None`）⇒ 丢掉草稿，不写。
    ///      凭空写一条没有时间戳的记录只会变成"排在最末、点了没反应"的假条目；
    ///   ③ **与现值相同** ⇒ 不写（免得每次失焦都写一次盘）。
    ///
    /// ⚠️ **有草稿就一定把它清掉**（连 ①②③ 三支也是）：交完之后编辑框读回**模型那份**
    ///    —— 它在 [`BatchHistory::setting_note`] 里被归一化过（首尾空白去掉、换行折成
    ///    空格），编辑框要如实显示**存下去的那一份**，而不是用户敲的原文。
    pub fn committing(&self, code: &str, current: Option<&str>) -> NoteCommitOutcome {
        let Some(draft) = self.by_code.get(code) else {
            // ① 没有草稿：状态原样不动。
            return NoteCommitOutcome {
                writes: Vec::new(),
                drafts: self.clone(),
            };
        };
        let mut cleared = self.by_code.clone();
        cleared.remove(code);
        let cleared = NoteDrafts { by_code: cleared };
        // ② 这一行已经不在屏上了 ／ ③ 与现值相同 —— 两支护的都是"不写"，
        //    但草稿照样要清掉。
        let should_write = match current {
            Some(current) => current != draft.as_str(),
            None => false,
        };
        let writes = if should_write {
            vec![NoteWrite {
                code: code.to_string(),
                note: draft.clone(),
            }]
        } else {
            Vec::new()
        };
        NoteCommitOutcome {
            writes,
            drafts: cleared,
        }
    }

    /// 把**还挂着的所有**草稿交出去（面板关闭时的那一次兜底）。
    ///
    /// 上游 `NoteDrafts.committingAll(current:)`。
    ///
    /// ⚠️ 判据是**这里还挂着哪些键**，不是"屏上有哪些行"：这时面板正在销毁，
    ///    走一遍屏上的行等于把"用户敲过字"这件事重新押在视图树还在不在上。
    ///
    /// ⚠️ 顺序按**码排序**，不是按用户编辑的先后：顺序必须确定 ——
    ///    否则"到底写了几条、写了哪几条"在不同运行里是两个答案，也没法断言。
    ///    （写盘本身是幂等的逐条覆写，顺序不影响结果，只影响可观察性。）
    ///
    /// ⚠️ **遍历前先取键的快照**：`committing` 会产出一个**新的**值，实现里是
    ///    "边走边换整份状态"——若直接遍历原映射并原地改写，就是边遍历边改
    ///    （`BTreeMap` 的键迭代顺序本身不受影响，但"快照"这条纪律要写死在这里，
    ///    免得日后换成别的容器时它悄悄失效）。
    pub fn committing_all<F>(&self, current: F) -> NoteCommitOutcome
    where
        F: Fn(&str) -> Option<String>,
    {
        let codes: Vec<String> = self.by_code.keys().cloned().collect();
        let mut state = self.clone();
        let mut writes: Vec<NoteWrite> = Vec::new();
        for code in codes {
            let outcome = state.committing(&code, current(&code).as_deref());
            state = outcome.drafts;
            writes.extend(outcome.writes);
        }
        NoteCommitOutcome {
            writes,
            drafts: state,
        }
    }
}

// ---------------------------------------------------------------------------
// 时间：手写的 ISO8601 双向转换
// ---------------------------------------------------------------------------

/// 公历的年月日 → 距 1970-01-01 的天数。
///
/// **Howard Hinnant 的 `days_from_civil`**（公有领域的经典算法，`chrono`/`time` 内部
/// 用的也是这一套）。它只在"必须自己实现时间"时才值得抄一份 —— 而本 crate 正是那种
/// 情况（依赖白名单只有 serde 两个）。
///
/// 🔴 **这里的除法必须保持"向零截断"（`/`），不许改成 `div_euclid`**：
///    `if y >= 0 { y } else { y - 399 }` 里的那个 **`-399`** 正是 Hinnant 为**负数年份**
///    写的补偿 —— 它与后面的 `/ 400`（截断）配套，等价于对 `y` 做向下取整。
///    把 `/ 400` 换成 `div_euclid(400)` 会**破坏**那次补偿，答案差一天
///    （而 1970 年前 / 公元前是这条算法存在的理由，也是最难被测试发现的地方）。
///    同理见 [`civil_from_days`] 里的 `-146_096`。
///    ⚠️ 真正需要 `div_euclid` 的是 [`BatchHistory::timestamp`] 那两行（那里的
///    `shifted` **可以**为负、且没有补偿项）—— 别把两处的口径弄反。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 距 1970-01-01 的天数 → 公历的年月日。
///
/// **Howard Hinnant 的 `civil_from_days`**（与 [`days_from_civil`] 是一对，
/// 互为逆运算；本文件的往返由 `the_timestamp_round_trips_through_the_parser` 钉住）。
///
/// 🔴 同 [`days_from_civil`] 那条：这里的除法**保持截断**，`-146_096` 是给负数写的补偿；
///    改成 `div_euclid` 会算错 1970 年之前的日子。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ISO 8601 原文 → Unix 秒；认不出来 ⇒ `None`。
///
/// **形状**（与内核 `delivery.rs` 的 `parse_rfc3339` 接受的形状一致，也与既有的
/// `TimestampPresentation::text` 一致）：
/// `YYYY-MM-DDTHH:MM:SS[.小数]±HH:MM` 或 `...Z`。任何一条不满足就 `None` ——
/// **不做猜测**（`None` 的两个下游分别是"排到最末"与"原样显示"，都不编造值）。
///
/// ⚠️ 位置检查走 **UTF-8 字节**而不是字符：多字节字符的续字节都 >= 0x80，
///    不可能在这些位置冒充 ASCII 分隔符，所以"通过了检查"就等价于
///    "前 19 字节是纯 ASCII"，后面的切片也就不会切在字符中间。
fn iso8601_to_unix(raw: &str) -> Option<i64> {
    let b = raw.as_bytes();
    if !(b.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && digits(b, 0, 4)
        && digits(b, 5, 2)
        && digits(b, 8, 2)
        && digits(b, 11, 2)
        && digits(b, 14, 2)
        && digits(b, 17, 2))
    {
        return None;
    }

    let num = |from: usize, count: usize| -> i64 {
        let mut value = 0i64;
        for &c in &b[from..from + count] {
            value = value * 10 + (c - b'0') as i64;
        }
        value
    };
    let (year, month, day) = (num(0, 4), num(5, 2), num(8, 2));
    let (hour, minute, second) = (num(11, 2), num(14, 2), num(17, 2));

    // ⚠️ **范围口径与上游逐字对齐，不许"顺手加严"**（修复轮 2 / 复审 I-3：
    //    上一版查了"当月天数"与"时 ≤ 23"，而那是**比上游更严**的，方向错了）。
    //
    //    下面是本机实测（macOS 26.7，Swift 探针跑的就是 `BatchHistory.swift:220` 那套参数
    //    `ISO8601DateFormatter` + `[.withInternetDateTime]`）：
    //
    //      | 输入 | 上游 Foundation | 本节 |
    //      |---|---|---|
    //      | 月 0 / 13、日 0 / 32、时 25、分 60、秒 60 | 拒绝（`nil`） | 拒绝（一致） |
    //      | `2026-02-30` | **受理 → 2026-03-02** | 受理（**溢出**，同一天） |
    //      | `2026-04-31` | **受理 → 2026-05-01** | 受理（同上） |
    //      | 平年 `2026-02-29` | **受理 → 2026-03-01** | 受理（同上） |
    //      | `24:00` / `24:30` | **受理 → 次日 00:00 / 00:30** | 受理（同上） |
    //
    //    ⇒ 这一节只写**上下界**：日 `1..=31`、时 `0..=24`。溢出**交给算术自然发生** ——
    //    `days_from_civil` 对 `(2026,2,30)` 本来就算出与 `2026-03-02` 同一天，
    //    `hour * 3600` 对 24 点本来就会进位一天。**多写一段日历运算反而会把它算成
    //    另一个值**（上一版就是这样，比上游严、还与上游差一天）。
    //
    //    为什么范围仍然要查（哪怕只查这么松）：`history.json` 是用户改得动的文件，
    //    `2026-13-…` 这种上游会拒的串若不拒，就会算出一个**正**值 ⇒ 那条记录
    //    **排到最前、躲过淘汰**，与它该在的位置正相反。
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 24
        || minute > 59
        || second > 59
    {
        return None;
    }

    // 可选的小数秒（`.` 后至少一位；纳秒与毫秒在这里一视同仁 —— 都被丢掉）。
    let mut i = 19;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
    }

    // 偏移必须是**整串的结尾**（多一个字符都不认）：`Z` 或 `±HH:MM`。
    let offset_seconds = if b.len() == i + 1 && b[i] == b'Z' {
        0
    } else if b.len() == i + 6
        && (b[i] == b'+' || b[i] == b'-')
        && digits(b, i + 1, 2)
        && b[i + 3] == b':'
        && digits(b, i + 4, 2)
    {
        let magnitude = num(i + 1, 2) * 3600 + num(i + 4, 2) * 60;
        if b[i] == b'-' {
            -magnitude
        } else {
            magnitude
        }
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds)
}

/// `count` 个连续数字（越界即 `false`，不会 panic）。
fn digits(b: &[u8], from: usize, count: usize) -> bool {
    from + count <= b.len() && (from..from + count).all(|k| b[k].is_ascii_digit())
}

// ⚠️ 这里**没有** `days_in_month` / `is_leap_year` 那样的日历函数，而且**不许加回来**
//    （修复轮 2 / 复审 I-3 删掉的）：上游 Foundation 在"日"与"时"上是**溢出**的
//    （`2026-02-30` → 2026-03-02、`24:00` → 次日 00:00），而 `days_from_civil` 的
//    算术天然就把它们算成同一天 —— 加一段日历校验或日历换算，只会让壳与内核
//    （以及 mac 版）**分叉**。溢出这件事由 [`days_from_civil`] 那一行负责。
//    实测表见 [`iso8601_to_unix`] 里那段注释。

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游三份（逐条对位，每条测试上方点名它对应上游的哪一条）：
    //!   · `macos/Tests/BenagenCoreKitTests/BatchHistoryTests.swift`
    //!   · `macos/Tests/BenagenCoreKitTests/BatchHistoryRowTests.swift`
    //!   · `macos/Tests/BenagenCoreKitTests/NoteDraftsTests.swift`
    //!
    //! ⚠️ 夹具时间戳的 Unix 秒是**独立算出**的字面量
    //!    （`python3 -c "import datetime; print(int(datetime.datetime.fromisoformat('…').timestamp()))"`），
    //!    **不是**被测代码算出来的 —— 拿被测代码生成期望值等于让夹具替变异体打掩护。

    use super::{BatchHistory, BatchHistoryEntry, BatchHistoryRow, NoteDrafts, NoteWrite};
    use crate::presentation::delivery_summary::{DeliveryCodeEntry, TimestampPresentation};

    /// 上游 `BatchHistoryRowTests.swift` 的 `entry(code:note:baseURL:at:)` 默认值：
    /// 「刚用过的那一条」的样子（规格 §1.1 的示例码）。
    const DEFAULT_AT: &str = "2026-09-18T09:12:00+08:00";

    const T0900: &str = "2026-09-18T09:00:00+08:00";
    const T1030: &str = "2026-09-18T10:30:00+08:00";
    const T1100: &str = "2026-09-18T11:00:00+08:00";

    /// 上面三条的 Unix 秒（与 `at(_:)` 那个夹具一一对应）。
    const E0900: i64 = 1_789_693_200;
    const E1030: i64 = 1_789_698_600;
    const E1100: i64 = 1_789_700_400;

    fn entry(code: &str, note: &str, base_url: &str, last_used_at: &str) -> BatchHistoryEntry {
        BatchHistoryEntry {
            code: code.to_string(),
            note: note.to_string(),
            base_url: base_url.to_string(),
            last_used_at: last_used_at.to_string(),
        }
    }

    /// 只有码与时间戳的一条（上游 `.init(code:lastUsedAt:)` 那两个默认参数的形态）。
    fn simple(code: &str, last_used_at: &str) -> BatchHistoryEntry {
        entry(code, "", "", last_used_at)
    }

    // -----------------------------------------------------------------------
    // ① 标题：备注为空时回落到码（`BatchHistoryRowTests.swift`）
    // -----------------------------------------------------------------------

    /// 上游 `theTitleShowsTheNoteWhenThereIsOne`。
    #[test]
    fn the_title_shows_the_note_when_there_is_one() {
        let row = BatchHistoryRow::of(&entry(
            "KUCwdl7d5u.UovnpW_D7",
            "客户张三 / 9月肿瘤数据",
            "",
            DEFAULT_AT,
        ));
        assert_eq!(row.title, "客户张三 / 9月肿瘤数据");
        assert_eq!(row.note, "客户张三 / 9月肿瘤数据");
        // ⚠️ 新增（审查次要 8）：简报点名的那条判据 —— **写了备注也照样显示码**。
        //    它是这一行的身份（用户唯一能拿去对账的东西），有备注时**一个字都不许少**。
        //    上游没有这一条（它只断 title 与 note），所以这是本波次补的。
        assert_eq!(row.code, "KUCwdl7d5u.UovnpW_D7");
    }

    /// 上游 `theTitleFallsBackToTheCodeWhenThereIsNoNote`。
    #[test]
    fn the_title_falls_back_to_the_code_when_there_is_no_note() {
        let code = "KUCwdl7d5u.UovnpW_D7";
        let row = BatchHistoryRow::of(&simple(code, DEFAULT_AT));
        assert_eq!(row.title, code, "没写备注的那一行，标题就是码本身");
        assert!(row.note.is_empty());
    }

    /// 上游 `aWhitespaceOnlyNoteCountsAsNoNote`。
    ///
    /// 为什么要有这条：一行"看起来是空的"标题在界面上就是**什么都没说**
    /// —— 用户会以为这一行坏了，而不是以为"我还没写备注"。手改过的 `history.json` 里
    /// `"note": "   "` 是能读进来的（解析只要求 `code` 非空），所以这条不是假想。
    #[test]
    fn a_whitespace_only_note_counts_as_no_note() {
        let code = "KUCwdl7d5u.UovnpW_D7";
        let row = BatchHistoryRow::of(&entry(code, "   \n\t ", "", DEFAULT_AT));
        assert_eq!(row.title, code);
        assert!(
            row.note.is_empty(),
            "交出去的备注也是空的：编辑框里不该躺着一串看不见的空白"
        );

        // ⚠️ **Unicode 空白，不是只 trim ASCII**：全角空格（U+3000，中文输入法下最容易
        //    敲出来的那个）与不换行空格（U+00A0，从网页/表格里粘出来的）同样算"空白"。
        //    上游 `whitespacesAndNewlines` 与 Rust 的 `str::trim()`（Unicode `White_Space`）
        //    是**同一个集合**；一个只 trim `' '`/`\t`/`\n` 的实现会在这里红 ——
        //    而它的后果是列表里出现一行"看起来是空的"标题。
        for blank in ["\u{3000}", "\u{00A0}", "\u{3000}\u{00A0} \t"] {
            let row = BatchHistoryRow::of(&entry(code, blank, "", DEFAULT_AT));
            assert_eq!(row.title, code, "{blank:?} 也是「没写备注」");
            assert!(row.note.is_empty());
        }
    }

    /// 上游 `theNoteIsTrimmedButItsInnerSpacingIsKept`。
    #[test]
    fn the_note_is_trimmed_but_its_inner_spacing_is_kept() {
        // 内部原有的空格一个都不动（那是用户自己写的排版）。
        let row = BatchHistoryRow::of(&entry("C", "  客户 张三  ", "", DEFAULT_AT));
        assert_eq!(row.note, "客户 张三");
        assert_eq!(row.title, "客户 张三");
    }

    // -----------------------------------------------------------------------
    // ② 时间列：复用既有的 TimestampPresentation
    // -----------------------------------------------------------------------

    /// 上游 `theHistoryTimeColumnReusesTheSharedTimestampPresentation`。
    ///
    /// ⚠️ 本波次里这条断言**多背一件事**（见模块头第 3 条）：`BatchHistoryRow::time_text`
    ///    抄了 `browser_row.rs` 那份**私有** `SourceTimeText` 的判据（既有文件不许改，
    ///    够不着）。下面 ① 那条关系式就是绑住两份实现的绳子。
    #[test]
    fn the_history_time_column_reuses_the_shared_timestamp_presentation() {
        let raw = "2026-09-18T09:12:00+08:00";
        let row = BatchHistoryRow::of(&simple("C", raw));
        // ① 关系式：与既有的那一份**是同一个答案**（不另造格式化）。
        assert_eq!(row.time_text, TimestampPresentation::text(raw));
        // ② 字面量：把形状本身钉死（`+08:00` 同形、秒被丢掉）。
        assert_eq!(row.time_text, "2026-09-18 09:12");
    }

    /// 上游 `aFractionalSecondTimestampIsAlsoUnderstood`。
    #[test]
    fn a_fractional_second_timestamp_is_also_understood() {
        // 清单里的 `created_at` 实测带小数秒；两种都要认。
        let row = BatchHistoryRow::of(&simple("C", "2026-09-18T09:12:00.805751+08:00"));
        assert_eq!(row.time_text, "2026-09-18 09:12");
    }

    /// 上游 `aRowWithoutATimestampShowsTheNoValueGlyph`。
    #[test]
    fn a_row_without_a_timestamp_shows_the_no_value_glyph() {
        // 空串 ⇒ 占位符。**不是**空着：空串在界面上就是"这一格没有值"而没说出口。
        assert_eq!(BatchHistoryRow::of(&simple("C", "")).time_text, "—");
    }

    /// 上游 `anUnparseableTimestampIsShownVerbatim`。
    #[test]
    fn an_unparseable_timestamp_is_shown_verbatim() {
        assert_eq!(BatchHistoryRow::of(&simple("C", "待定")).time_text, "待定");
    }

    // -----------------------------------------------------------------------
    // ③ 顺序：历史给什么顺序就画什么顺序
    // -----------------------------------------------------------------------

    /// 上游 `theRowsKeepTheHistorysOwnOrderNewestFirst`。
    #[test]
    fn the_rows_keep_the_historys_own_order_newest_first() {
        // 故意**乱序传入**：`BatchHistory` 是不变量的持有者（它排倒序），行层只做映射。
        let history = BatchHistory::new(vec![
            simple("OLD", T0900),
            simple("NEW", T1100),
            simple("MID", T1030),
        ]);
        let codes: Vec<String> = BatchHistoryRow::rows(&history)
            .into_iter()
            .map(|r| r.code)
            .collect();
        assert_eq!(codes, vec!["NEW", "MID", "OLD"]);
    }

    /// 上游 `everyEntryBecomesExactlyOneRowIdentifiedByItsCode`。
    #[test]
    fn every_entry_becomes_exactly_one_row_identified_by_its_code() {
        // ⚠️ 两条夹具的时间戳**必须相同**（上游就是同一个默认值）：不同的话
        //    `BatchHistory` 会按时间倒序把 `B` 排到前面，这条用例就变成了在测排序。
        let history = BatchHistory::new(vec![simple("A", T0900), simple("B", T0900)]);
        let rows = BatchHistoryRow::rows(&history);
        assert_eq!(rows.len(), history.entries.len());
        let ids: Vec<&str> = rows.iter().map(BatchHistoryRow::id).collect();
        assert_eq!(ids, vec!["A", "B"], "码是唯一键 ⇒ 它就是这一行的身份");
    }

    /// 上游 `anEmptyHistoryProducesNoRows`。
    #[test]
    fn an_empty_history_produces_no_rows() {
        // 调用方靠这个空 `Vec` 决定**整段不渲染**（列表为空时不显示这一段，不要空盒子）。
        assert!(BatchHistoryRow::rows(&BatchHistory::empty()).is_empty());
    }

    // -----------------------------------------------------------------------
    // 边界：码与备注**都**空的那种行
    // -----------------------------------------------------------------------

    /// 上游 `aRowWithNeitherNoteNorCodeStillSaysSomething`（纵深防御）。
    ///
    /// 经解析那条正路**造不出**这一行（空码在 `BatchHistory::new` 就被丢掉了）；
    /// 但 `BatchHistoryRow::of` 与 `BatchHistoryEntry` 都是公开的、都不过滤，
    /// 所以"码空 + 备注空"这一行在类型上**是构造得出来的** ——
    /// 而它的标题会是**空串**，在界面上与"这一行坏了"分不开。
    #[test]
    fn a_row_with_neither_note_nor_code_still_says_something() {
        let row = BatchHistoryRow::of(&simple("", ""));
        assert_eq!(row.title, "—");
        assert!(!row.title.is_empty());
        assert_eq!(row.time_text, "—");
    }

    /// 上游 `theRowsHelperCanNeverProduceAnEmptyTitledRow`。
    #[test]
    fn the_rows_helper_can_never_produce_an_empty_titled_row() {
        let history = BatchHistory::new(vec![simple("", T0900), simple("REAL", T1030)]);
        let codes: Vec<String> = history.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["REAL"], "空码不是一条交付批次（`new` 丢掉它）");
        assert!(BatchHistoryRow::rows(&history)
            .iter()
            .all(|r| !r.title.is_empty()));
    }

    // -----------------------------------------------------------------------
    // ④ 交付服务器：点一行要问对的那一台
    // -----------------------------------------------------------------------

    /// 上游 `eachRowCarriesTheServerItWasLoadedFrom`。
    #[test]
    fn each_row_carries_the_server_it_was_loaded_from() {
        let row = BatchHistoryRow::of(&entry(
            "C",
            "",
            "http://delivery.example.com:8010",
            DEFAULT_AT,
        ));
        assert_eq!(row.base_url, "http://delivery.example.com:8010");
        assert_eq!(
            row.base_url_or_nil(),
            Some("http://delivery.example.com:8010")
        );
    }

    /// 上游 `theDefaultServerIsCarriedAsNilSoTheKeyStaysOutOfTheRequest`。
    #[test]
    fn the_default_server_is_carried_as_nil_so_the_key_stays_out_of_the_request() {
        let row = BatchHistoryRow::of(&simple("C", DEFAULT_AT));
        assert!(row.base_url.is_empty());
        assert_eq!(row.base_url_or_nil(), None);
    }

    // -----------------------------------------------------------------------
    // 长度上界：手改过的历史文件不得把内核的 FIFO 堵死
    // -----------------------------------------------------------------------

    /// 上游 `anOrdinaryCodeCanBeLoaded`。
    #[test]
    fn an_ordinary_code_can_be_loaded() {
        assert!(BatchHistoryRow::of(&simple("KUCwdl7d5u.UovnpW_D7", DEFAULT_AT)).is_sendable());
    }

    /// 上游 `anOverlongCodeFromAHandEditedHistoryFileCannotBeSent`。
    #[test]
    fn an_overlong_code_from_a_hand_edited_history_file_cannot_be_sent() {
        let huge = "A".repeat(DeliveryCodeEntry::MAXIMUM_BYTES + 1);
        assert!(!BatchHistoryRow::of(&simple(&huge, DEFAULT_AT)).is_sendable());
        // 边界：**正好等于**上界是能发的（`too_long` 用的是 `>`）。
        let exactly = "A".repeat(DeliveryCodeEntry::MAXIMUM_BYTES);
        assert!(BatchHistoryRow::of(&simple(&exactly, DEFAULT_AT)).is_sendable());
    }

    // -----------------------------------------------------------------------
    // 增 / 去重 / 排序（`BatchHistoryTests.swift`）
    // -----------------------------------------------------------------------

    /// 上游 `theSameCodeIsUpdatedNotDuplicated`。
    #[test]
    fn the_same_code_is_updated_not_duplicated() {
        let h = BatchHistory::empty()
            .recording("AAA-1", "", None, E0900)
            .recording("AAA-1", "", None, E1030);
        assert_eq!(h.entries.len(), 1, "同一个码加两次只该有一条");
        assert_eq!(h.entries[0].code, "AAA-1");
        assert_eq!(h.entries[0].last_used_at, T1030, "留下的必须是**后一次**的时间");
    }

    /// 上游 `entriesAreOrderedByLastUseDescending`。
    #[test]
    fn entries_are_ordered_by_last_use_descending() {
        let h = BatchHistory::empty()
            .recording("AAA-1", "", None, E0900)
            .recording("BBB-2", "", None, E1100)
            .recording("CCC-3", "", None, E1030);
        let codes: Vec<String> = h.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["BBB-2", "CCC-3", "AAA-1"]);
    }

    /// 上游 `reUsingAnOldCodeMovesItToTheTop`。
    #[test]
    fn re_using_an_old_code_moves_it_to_the_top() {
        let h = BatchHistory::empty()
            .recording("AAA-1", "", None, E0900)
            .recording("BBB-2", "", None, E1030)
            .recording("AAA-1", "", None, E1100);
        let codes: Vec<String> = h.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["AAA-1", "BBB-2"]);
    }

    /// 上游 `parseOrdersEntriesByLastUseDescending`。
    #[test]
    fn parse_orders_entries_by_last_use_descending() {
        let h = BatchHistory::parse(
            r#"{"version":1,"entries":[
              {"code":"AAA-1","note":"","base_url":"","last_used_at":"2026-09-18T09:00:00+08:00"},
              {"code":"BBB-2","note":"","base_url":"","last_used_at":"2026-09-18T11:00:00+08:00"},
              {"code":"CCC-3","note":"","base_url":"","last_used_at":"2026-09-18T10:30:00+08:00"}]}"#,
        );
        let codes: Vec<String> = h.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["BBB-2", "CCC-3", "AAA-1"]);
    }

    /// 上游 `mostRecentIsTheNewestEntryAndNilWhenEmpty`。
    #[test]
    fn most_recent_is_the_newest_entry_and_nil_when_empty() {
        assert!(BatchHistory::empty().most_recent().is_none());
        assert!(BatchHistory::empty().is_empty());

        let h = BatchHistory::empty()
            .recording("AAA-1", "", None, E0900)
            .recording("BBB-2", "", None, E1100);
        assert_eq!(h.most_recent().map(|e| e.code.as_str()), Some("BBB-2"));
        assert!(!h.is_empty());
    }

    // -----------------------------------------------------------------------
    // 上限淘汰
    // -----------------------------------------------------------------------

    /// 上游 `theHistoryKeepsAtMostFiftyEntries`。
    #[test]
    fn the_history_keeps_at_most_fifty_entries() {
        let mut h = BatchHistory::empty();
        for i in 0..51 {
            h = h.recording(&format!("code-{i}"), "", None, E0900 + i);
        }
        assert_eq!(BatchHistory::MAXIMUM_ENTRIES, 50, "上限是 50");
        assert_eq!(h.entries.len(), 50, "加到 51 条只该剩 50");
        assert!(
            !h.entries.iter().any(|e| e.code == "code-0"),
            "被淘汰的必须是**最久未用**的那条"
        );
        assert_eq!(h.entries[0].code, "code-50", "最近用的还在最前");
        assert!(h.entries.iter().any(|e| e.code == "code-1"));
    }

    /// 上游 `theEvictedEntryIsTheLeastRecentlyUsedNotTheOldestInserted`。
    ///
    /// ⚠️ 这一条才是"最久未用（LRU）"的判据：上面那条里"插入最早"恰好等于"最久未用"，
    ///    换成 FIFO 淘汰也能过。
    #[test]
    fn the_evicted_entry_is_the_least_recently_used_not_the_oldest_inserted() {
        let mut h = BatchHistory::empty();
        for i in 0..50 {
            h = h.recording(&format!("code-{i}"), "", None, E0900 + i);
        }
        h = h.recording("code-0", "", None, E0900 + 100); // 重新使用 code-0
        h = h.recording("code-50", "", None, E0900 + 200); // 第 51 条

        assert_eq!(h.entries.len(), 50);
        assert!(
            h.entries.iter().any(|e| e.code == "code-0"),
            "刚用过的码不该被淘汰"
        );
        assert!(
            !h.entries.iter().any(|e| e.code == "code-1"),
            "被淘汰的是最久未用的那个"
        );
    }

    // -----------------------------------------------------------------------
    // 解析容错
    // -----------------------------------------------------------------------

    /// 上游 `garbageIsTreatedAsAnEmptyHistory`。
    ///
    /// 底线：读不出来 / 解析失败 / `version` 不认识 ⇒ **当空历史**并继续。
    /// 本函数**没有 `Result`** —— 这正是判据。
    #[test]
    fn garbage_is_treated_as_an_empty_history() {
        assert!(BatchHistory::parse("").is_empty(), "空串");
        assert!(BatchHistory::parse("not json at all").is_empty(), "非 JSON");
        assert!(BatchHistory::parse("[1,2,3]").is_empty(), "顶层不是对象");
        assert!(
            BatchHistory::parse(
                r#"{"version":2,"entries":[{"code":"AAA-1","last_used_at":"2026-09-18T09:00:00+08:00"}]}"#
            )
            .is_empty(),
            "version 是**别的数** ⇒ 不认"
        );
        assert!(
            BatchHistory::parse(r#"{"entries":[]}"#).is_empty(),
            "没有 version ⇒ 不认"
        );
        assert!(
            BatchHistory::parse(r#"{"version":"1","entries":[]}"#).is_empty(),
            "version 不是数 ⇒ 不认"
        );
        assert!(
            BatchHistory::parse(r#"{"version":1,"entries":{}}"#).is_empty(),
            "entries 不是数组"
        );
        assert!(
            BatchHistory::parse(r#"{"version":1,"entries":"AAA-1"}"#).is_empty(),
            "entries 是串"
        );
        assert!(
            BatchHistory::parse(r#"{"version":1}"#).is_empty(),
            "没有 entries"
        );
        assert!(
            BatchHistory::parse(r#"{"version":1,"entries":[1,2,3]}"#).is_empty(),
            "行不是对象"
        );
    }

    /// 上游 `aMalformedRowIsDroppedWithoutLosingTheRest`。
    #[test]
    fn a_malformed_row_is_dropped_without_losing_the_rest() {
        let h = BatchHistory::parse(
            r#"{"version":1,"entries":[
              {"code":"AAA-1","note":"好的那条","base_url":"http://dl.example","last_used_at":"2026-09-18T09:00:00+08:00"},
              {"note":"没有 code","last_used_at":"2026-09-18T10:00:00+08:00"},
              {"code":"","note":"空 code","last_used_at":"2026-09-18T10:00:00+08:00"},
              {"code":"BBB-2","last_used_at":"2026-09-18T11:00:00+08:00"}]}"#,
        );
        let codes: Vec<String> = h.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["BBB-2", "AAA-1"], "坏行丢掉、好行留下");
        assert_eq!(h.entries[1].note, "好的那条");
    }

    /// 上游 `anUnparseableTimestampSortsLastAndKeepsTheEntry`。
    #[test]
    fn an_unparseable_timestamp_sorts_last_and_keeps_the_entry() {
        // 时间是**排序键**，不是主键：解析不出来的那条仍然是一条可用的码，
        // 只是它排在最末（最先被淘汰的位置），而不是让整份历史作废。
        let h = BatchHistory::new(vec![simple("AAA-1", "不是时间"), simple("BBB-2", T1100)]);
        let codes: Vec<String> = h.entries.iter().map(|e| e.code.clone()).collect();
        assert_eq!(codes, vec!["BBB-2", "AAA-1"]);
    }

    // -----------------------------------------------------------------------
    // 往返（序列化 ⇄ 解析）
    // -----------------------------------------------------------------------

    /// 上游 `parseOfSerializeIsIdentity`。
    #[test]
    fn parse_of_serialize_is_identity() {
        // 往返必须恒等 —— 包括备注为空、`base_url` 为空这两种"可选字段缺失"的情形。
        let h = BatchHistory::new(vec![
            entry(
                "KUCwdl7d5u.UovnpW_D7",
                "客户张三 / 9月肿瘤数据",
                "http://dl.example",
                T0900,
            ),
            entry("BBB-2", "", "", T1030),
        ]);
        let text = h.serialized().expect("序列化不得失败");
        assert_eq!(BatchHistory::parse(&text), h);
    }

    /// 上游 `theFileShapeUsesTheSpecifiedFieldNames`。
    #[test]
    fn the_file_shape_uses_the_specified_field_names() {
        // 字段名是**逐字**的：`version` / `entries` / `code` / `note` /
        // `base_url` / `last_used_at`。写成 camelCase 会让"下一个读这个文件的人"
        // （包括未来版本的壳）读到一份对不上的东西。
        let h = BatchHistory::new(vec![entry("AAA-1", "备注", "http://dl.example", T0900)]);
        let text = h.serialized().expect("序列化不得失败");

        assert!(text.contains("\"version\""));
        assert!(text.contains("\"entries\""));
        assert!(text.contains("\"code\""));
        assert!(text.contains("\"note\""));
        assert!(text.contains("\"base_url\""));
        assert!(text.contains("\"last_used_at\""));
        assert!(!text.contains("baseUrl"), "键名是线上形状的 snake_case");
        assert!(
            text.contains("http://dl.example"),
            "URL 里的 / 不得被转义成 \\/"
        );
    }

    /// 上游 `theTimestampIsTheSameShapeAsTheManifestCreatedAt`。
    #[test]
    fn the_timestamp_is_the_same_shape_as_the_manifest_created_at() {
        // `last_used_at` 与交付清单的 `created_at` **同形**，直接复用既有的呈现函数显示
        // —— 不另造一套格式化。判据就是"既有的那个呈现函数认得它"
        // （不认得时它会**原样返回**）。
        let h = BatchHistory::empty().recording("AAA-1", "", None, E1030);
        let raw = h.entries[0].last_used_at.clone();

        assert_eq!(raw, T1030, "带 +08:00、秒级");
        assert_eq!(
            TimestampPresentation::text(&raw),
            "2026-09-18 10:30",
            "必须能被既有的 TimestampPresentation 读懂；时间戳的格式只有那一份口径"
        );
    }

    /// 上游 `timestampsUseTheFixedDeliveryServerOffsetNotTheMachineTimeZone`。
    ///
    /// ⚠️ 有意偏离的判据：写出去的时间戳固定用 **+08:00**（交付服务器的形状），
    ///    不是本机时区。若改成"本机时区"，下面那条断言立刻变红
    ///    （本机是 UTC+8 以外的任何地方都会红；本机恰好是 +08:00 时靠下一条跨零点用例兜住）。
    #[test]
    fn timestamps_use_the_fixed_delivery_server_offset_not_the_machine_time_zone() {
        let h = BatchHistory::empty().recording("AAA-1", "", None, 1_789_691_400);
        assert_eq!(h.entries[0].last_used_at, "2026-09-18T08:30:00+08:00");
    }

    /// 🔴 **跨零点**：加 8 小时**跨过了一天**（控制者为手写算法点名要的边界之一）。
    ///
    /// 上游那一条用的时刻是 `2026-09-18T00:30:00Z`（`+08:00` 下同日 08:30），
    /// 跨的是**半天**而不是日期线；这一条把它挪到 UTC 的下午，让日期真的翻页。
    #[test]
    fn the_fixed_offset_can_roll_over_into_the_next_day() {
        // 1789662600 = 2026-09-17T16:30:00Z
        assert_eq!(
            BatchHistory::timestamp(1_789_662_600),
            "2026-09-18T00:30:00+08:00",
            "UTC 的 16:30 在 +08:00 已经是第二天（手写算法最易错的一处）"
        );
    }

    /// 🔴 **闰年**（控制者为手写算法点名要的边界之二）。
    #[test]
    fn the_timestamp_handles_a_leap_day() {
        // 1835402400 = 2028-02-29T02:00:00Z
        assert_eq!(
            BatchHistory::timestamp(1_835_402_400),
            "2028-02-29T10:00:00+08:00"
        );
        // 再减一天就是 02-28（跨过闰日的那一步不能跳错）。
        assert_eq!(
            BatchHistory::timestamp(1_835_402_400 - 86_400),
            "2028-02-28T10:00:00+08:00"
        );
        // 整百年份里 2100 **不是**闰年（格里高利历：能被 100 整除但不能被 400 整除）。
        // 4107463200 = 2100-02-28T10:00:00+08:00 ⇒ 次日必须是 03-01。
        assert_eq!(
            BatchHistory::timestamp(4_107_463_200),
            "2100-02-28T10:00:00+08:00"
        );
        assert_eq!(
            BatchHistory::timestamp(4_107_463_200 + 86_400),
            "2100-03-01T10:00:00+08:00",
            "2100 不是闰年 ⇒ 02-28 之后直接是 03-01"
        );
    }

    /// 手写算法的**双向**对位（新增，上游没有）：`civil_from_days` 与 `days_from_civil`
    /// 必须互为逆运算 —— 上面那些用例各自只走了一个方向。
    #[test]
    fn the_timestamp_round_trips_through_the_parser() {
        for (iso, unix) in [
            ("1970-01-01T08:00:00+08:00", 0_i64),
            ("2026-09-18T09:00:00+08:00", E0900),
            ("2028-02-29T10:00:00+08:00", 1_835_402_400),
            ("2100-02-28T10:00:00+08:00", 4_107_463_200),
            ("2100-03-01T10:00:00+08:00", 4_107_463_200 + 86_400),
            ("2026-09-18T09:12:00+08:00", 1_789_693_920),
        ] {
            assert_eq!(BatchHistory::timestamp(unix), iso, "生成方向");
            assert_eq!(
                super::iso8601_to_unix(iso),
                Some(unix),
                "解析方向（两个方向的算法不同，必须互相印证）"
            );
        }
    }

    /// **修复轮 2（复审 I-3）**：范围口径与上游**逐字对齐**。
    ///
    /// ⚠️ 这条测试的前一版把"比上游更严"固化成"正确"了（它断言 `2026-02-30` 与 `24:00`
    ///    都不认，而实测上游**受理**它们并**溢出**）。现在两半都钉住：
    ///     ① 上游**拒**的（月 0/13、日 0/32、时 25、分/秒 60）⇒ 我们也拒；
    ///     ② 上游**受理并溢出**的 ⇒ 我们算出的秒与上游**逐位相同**（下面那批秒数是
    ///        本机 Swift 探针实测出来的，不是算出来的）。
    ///
    /// 判别力：把日改成 `1..=当月天数`（上一版的错法）⇒ ② 那一半立刻红；
    /// 把范围整段删掉 ⇒ ① 那一半红。
    #[test]
    fn the_range_check_matches_foundation_exactly() {
        // ① 上游也拒的
        for bogus in [
            "2026-13-01T10:00:00+08:00", // 月 13
            "2026-00-01T10:00:00+08:00", // 月 0
            "2026-01-32T10:00:00+08:00", // 日 32
            "2026-01-00T10:00:00+08:00", // 日 0
            "2026-01-01T25:00:00+08:00", // 时 25
            "2026-01-01T10:60:00+08:00", // 分 60
            "2026-01-01T10:00:60+08:00", // 秒 60
        ] {
            assert_eq!(super::iso8601_to_unix(bogus), None, "{bogus} 上游也拒");
        }

        // ② 上游受理并**溢出**的：秒数来自 Swift 探针实测，逐位对照
        for (raw, seconds) in [
            // 2 月 30 日 → 3 月 2 日
            ("2026-02-30T10:00:00+08:00", 1_772_416_800_i64),
            // 4 月 31 日 → 5 月 1 日
            ("2026-04-31T10:00:00+08:00", 1_777_600_800),
            // 平年 2 月 29 日 → 3 月 1 日
            ("2026-02-29T10:00:00+08:00", 1_772_330_400),
            // 24:00 → **次日** 00:00
            ("2026-01-01T24:00:00+08:00", 1_767_283_200),
            // 24:30 → 次日 00:30
            ("2026-01-01T24:30:00+08:00", 1_767_285_000),
            // 跨年：12 月 31 日 24:00 → 次年 1 月 1 日 00:00
            ("2026-12-31T24:00:00+08:00", 1_798_732_800),
        ] {
            assert_eq!(
                super::iso8601_to_unix(raw),
                Some(seconds),
                "{raw} 必须与上游 Foundation 算出同一个时刻"
            );
        }

        // 溢出之后的落点（显示方向）**逐字**对照 —— 上面那批里三个日历溢出的输入，
        // 上游解析成 03-02 / 05-01 / 03-01，`timestamp` 必须原样写得回来。
        assert_eq!(
            BatchHistory::timestamp(1_772_416_800),
            "2026-03-02T10:00:00+08:00"
        );
        assert_eq!(
            BatchHistory::timestamp(1_777_600_800),
            "2026-05-01T10:00:00+08:00"
        );
        assert_eq!(
            BatchHistory::timestamp(1_772_330_400),
            "2026-03-01T10:00:00+08:00"
        );

        // ③ 对照面：**合法边界**照样认得（否则上面可能是"把一切都拒了"）
        assert!(super::iso8601_to_unix("2026-01-31T23:59:59+08:00").is_some());
        assert!(
            super::iso8601_to_unix("2028-02-29T00:00:00+08:00").is_some(),
            "闰日合法"
        );
        assert!(
            super::iso8601_to_unix("2026-02-28T00:00:00+08:00").is_some(),
            "平年 2 月 28 日合法"
        );
    }

    /// 增量（上游只经 `ISO8601DateFormatter` 间接验过，这里把它钉在**排序键**上）：
    /// `Z` 偏移、小数秒、负偏移、以及一批认不出来的形状。
    #[test]
    fn the_sort_key_understands_z_and_fractional_seconds() {
        assert_eq!(
            super::iso8601_to_unix("2026-09-18T01:00:00Z"),
            Some(E0900),
            "`Z` 与 +00:00 同义（与 T0900 是同一个时刻）"
        );
        assert_eq!(
            super::iso8601_to_unix("2026-09-18T09:00:00.805751+08:00"),
            Some(E0900),
            "小数秒被丢掉（与显示口径一致）"
        );
        // 负偏移：`-05:00` 的那一刻在 UTC 看更晚（差 13 小时）。
        assert_eq!(
            super::iso8601_to_unix("2026-09-18T09:00:00-05:00"),
            Some(E0900 + 13 * 3600)
        );
        // 认不出来的一律 `None`（**不猜**）。
        for bogus in [
            "",
            "不是时间",
            "2026-09-18",
            "2026-09-18T09:00:00",
            "2026-09-18T09:00:00+0800",
        ] {
            assert_eq!(super::iso8601_to_unix(bogus), None, "{bogus} 不该被认出来");
        }
    }

    // -----------------------------------------------------------------------
    // 备注（一条自由文本）
    // -----------------------------------------------------------------------

    /// 上游 `theNoteSurvivesTheNextUseOfTheSameCode`。
    #[test]
    fn the_note_survives_the_next_use_of_the_same_code() {
        // ⚠️ 备注**不在**被更新的那批里：用户写下的"客户张三"不该因为又用了一次这个码
        //    而消失。这正是 `recording(note: None)`（= 保持原样）的理由。
        let h = BatchHistory::empty()
            .recording("AAA-1", "", None, E0900)
            .setting_note("客户张三", "AAA-1")
            .recording("AAA-1", "http://dl.example", None, E1030);

        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].note, "客户张三");
        assert_eq!(
            h.entries[0].base_url, "http://dl.example",
            "base_url 是会被更新的那一批"
        );
        assert_eq!(h.entries[0].last_used_at, T1030);
    }

    /// 上游 `notesAreTrimmedToASingleLine`。
    #[test]
    fn notes_are_trimmed_to_a_single_line() {
        let h = BatchHistory::empty().recording("AAA-1", "", None, E0900);
        let note = |raw: &str| -> String {
            h.setting_note(raw, "AAA-1")
                .entries
                .first()
                .map(|e| e.note.clone())
                .unwrap_or_else(|| "（没这条）".to_string())
        };

        assert_eq!(note("  客户张三 / 9月肿瘤数据  "), "客户张三 / 9月肿瘤数据");
        assert_eq!(note("  多行\n备注  "), "多行 备注");
        assert_eq!(note("第一行\r\n第二行"), "第一行 第二行");
        assert_eq!(note("带\t制表符"), "带 制表符");
        assert_eq!(note("A  /  B"), "A  /  B", "内部原有的空格一个都不动");
        assert_eq!(note(""), "", "空串 = 没写备注（合法值）");
        assert_eq!(note("   "), "", "只有空白 = 没写备注");
    }

    /// 上游 `settingANoteOnACodeThatIsNotInTheHistoryDoesNothing`。
    #[test]
    fn setting_a_note_on_a_code_that_is_not_in_the_history_does_nothing() {
        let h = BatchHistory::empty().recording("AAA-1", "", None, E0900);

        assert_eq!(h.setting_note("备注", "ZZZ-9"), h);
        assert_eq!(
            h.setting_note("", "AAA-1").entries[0].note,
            "",
            "清空备注是「设成空串」，不是删掉这一条"
        );
        assert_eq!(
            h.setting_note("", "AAA-1").entries.len(),
            1,
            "清空备注**不删条目**"
        );
    }

    /// 上游 `anEmptyCodeIsNeverRecorded`。
    #[test]
    fn an_empty_code_is_never_recorded() {
        assert!(BatchHistory::empty()
            .recording("", "", None, E0900)
            .is_empty());
        assert!(BatchHistory::empty()
            .recording("", "http://dl.example", None, E0900)
            .is_empty());
    }

    /// 上游 `theDefaultServerIsTheEmptyBaseURL`。
    #[test]
    fn the_default_server_is_the_empty_base_url() {
        assert_eq!(simple("AAA-1", T0900).base_url_or_nil(), None);

        let explicit = entry("AAA-1", "", "http://dl.example", T0900);
        assert_eq!(explicit.base_url_or_nil(), Some("http://dl.example"));
    }

    // -----------------------------------------------------------------------
    // 备注草稿的提交判决（`NoteDraftsTests.swift`）
    // -----------------------------------------------------------------------

    const CODE_A: &str = "KUCwdl7d5u.UovnpW_D7";
    const CODE_B: &str = "AAAA1111bbbb2222CCCC";

    fn write(code: &str, note: &str) -> NoteWrite {
        NoteWrite {
            code: code.to_string(),
            note: note.to_string(),
        }
    }

    /// 上游 `aFreshDraftsStateHasNothingInIt`。
    #[test]
    fn a_fresh_drafts_state_has_nothing_in_it() {
        let drafts = NoteDrafts::new();
        assert!(drafts.is_empty());
        assert_eq!(drafts.draft(CODE_A), None);
    }

    /// 上游 `theFieldShowsTheDraftWhenThereIsOneAndTheStoredNoteOtherwise`。
    #[test]
    fn the_field_shows_the_draft_when_there_is_one_and_the_stored_note_otherwise() {
        let drafts = NoteDrafts::new().editing("客户张三", CODE_A);
        assert_eq!(
            drafts.text(CODE_A, "旧的那句"),
            "客户张三",
            "用户敲进去的字必须压过模型里那份旧的"
        );
        assert_eq!(
            drafts.text(CODE_B, "别的那条"),
            "别的那条",
            "**别的行**不受影响：草稿是按码存的"
        );
    }

    /// 上游 `aClearedFieldIsStillADraft`。
    ///
    /// ⚠️ 这一条是**清空备注**能成立的全部理由。若把"草稿是空串"和"没有草稿"
    ///    混为一谈，用户把备注删空之后界面上会**弹回原来那句** ——
    ///    他明明删掉了，却看着它自己回来，而没有任何报错。
    #[test]
    fn a_cleared_field_is_still_a_draft() {
        let drafts = NoteDrafts::new().editing("", CODE_A);
        assert_eq!(drafts.draft(CODE_A), Some(""));
        assert_eq!(
            drafts.text(CODE_A, "客户张三"),
            "",
            "空串是一个**真的草稿**，不是「没有草稿」"
        );
    }

    /// 上游 `committingWithoutADraftWritesNothing`。
    #[test]
    fn committing_without_a_draft_writes_nothing() {
        let outcome = NoteDrafts::new().committing(CODE_A, Some("客户张三"));
        assert!(outcome.writes.is_empty());
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAnUnchangedDraftWritesNothingAndDropsTheDraft`。
    #[test]
    fn committing_an_unchanged_draft_writes_nothing_and_drops_the_draft() {
        let drafts = NoteDrafts::new().editing("客户张三", CODE_A);
        let outcome = drafts.committing(CODE_A, Some("客户张三"));
        assert!(
            outcome.writes.is_empty(),
            "与现值相同 ⇒ 不写（免得每次失焦都写一次盘）"
        );
        assert!(
            outcome.drafts.is_empty(),
            "草稿照样要清掉：交完之后编辑框读回模型那份"
        );
    }

    /// 上游 `committingAChangedDraftWritesExactlyOnce`。
    #[test]
    fn committing_a_changed_draft_writes_exactly_once() {
        let drafts = NoteDrafts::new().editing("客户张三 / 9月肿瘤数据", CODE_A);
        let outcome = drafts.committing(CODE_A, Some("客户张三"));
        assert_eq!(outcome.writes, vec![write(CODE_A, "客户张三 / 9月肿瘤数据")]);
        // 交完必须删草稿：留着的话编辑框会一直显示用户敲的**原文**
        //（首尾空白、换行都没被归一化过），与真正存下去的那一份分叉。
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingARowThatIsNoLongerOnScreenDropsTheDraftWithoutWriting`。
    #[test]
    fn committing_a_row_that_is_no_longer_on_screen_drops_the_draft_without_writing() {
        // `current == None` = 这个码已经不在历史列表里了（屏上没有那一行）。
        let drafts = NoteDrafts::new().editing("写点什么", CODE_A);
        let outcome = drafts.committing(CODE_A, None);
        assert!(outcome.writes.is_empty());
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAllWritesEveryChangedRowInAStableOrder`。
    #[test]
    fn committing_all_writes_every_changed_row_in_a_stable_order() {
        // 故意**反序**编辑（先 B 后 A），断言交出去的顺序**按码排序**而不是按编辑顺序。
        let first = "CODE-A";
        let second = "CODE-B";
        let drafts = NoteDrafts::new()
            .editing("B 的备注", second)
            .editing("A 的备注", first);
        let outcome = drafts.committing_all(|code| {
            Some(
                if code == first { "旧的 A" } else { "旧的 B" }.to_string(),
            )
        });
        assert_eq!(
            outcome.writes,
            vec![write(first, "A 的备注"), write(second, "B 的备注")]
        );
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAllSkipsRowsWhoseDraftDidNotChange`。
    #[test]
    fn committing_all_skips_rows_whose_draft_did_not_change() {
        let drafts = NoteDrafts::new()
            .editing("没变", CODE_A)
            .editing("变了", CODE_B);
        let outcome = drafts.committing_all(|code| {
            Some(
                if code == CODE_A { "没变" } else { "旧的 B" }.to_string(),
            )
        });
        assert_eq!(outcome.writes, vec![write(CODE_B, "变了")]);
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAllOnAFreshStateIsAQuietNoOp`。
    #[test]
    fn committing_all_on_a_fresh_state_is_a_quiet_no_op() {
        let outcome = NoteDrafts::new().committing_all(|_| Some("什么都好".to_string()));
        assert!(outcome.writes.is_empty());
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAllHandlesManyRowsAtOnce`。
    ///
    /// ⚠️ **遍历时必须先取键的快照**：`committing` 会产出一个**新的**值，
    ///    实现里是"边走边换整份状态"——若直接遍历并原地改写，就是边遍历边改。
    ///    这条用例用的是"每个码都要交"的最坏形态，所以只要实现退化成原地改，
    ///    它就会在这里炸（或者在别的运行里随机少写一条）。
    #[test]
    fn committing_all_handles_many_rows_at_once() {
        let mut drafts = NoteDrafts::new();
        for i in 0..20 {
            drafts = drafts.editing(&format!("备注 {i}"), &format!("CODE-{i}"));
        }
        let outcome = drafts.committing_all(|_| Some("旧的".to_string()));
        assert_eq!(outcome.writes.len(), 20);
        assert!(outcome.drafts.is_empty());
    }

    /// 上游 `committingAllIsExhaustiveOverTheDraftsItWasGiven`。
    ///
    /// 每一条路都必须是同一个判决的另一半：**没变的那条不写，变的那条写**，
    /// 而且**一个草稿都不许剩**。
    #[test]
    fn committing_all_is_exhaustive_over_the_drafts_it_was_given() {
        let drafts = NoteDrafts::new()
            .editing("一样", "CODE-A")
            .editing("不一样", "CODE-B")
            .editing("", "CODE-C"); // 清空也是一次真的改动
        let outcome = drafts.committing_all(|code| {
            Some(
                if code == "CODE-A" { "一样" } else { "旧的" }.to_string(),
            )
        });
        assert_eq!(
            outcome.writes,
            vec![write("CODE-B", "不一样"), write("CODE-C", "")]
        );
        assert!(outcome.drafts.is_empty());
    }

    /// 上游**删掉**的那条同义反复用例（`NoteDraftsTests.swift` 末尾那段注释）的替代：
    /// 同一份草稿，`committing` 与 `committing_all` 对同一条改动给出同一个写入。
    ///
    /// ⚠️ **它的判别力有多大，如实说**（审查次要 5 指出报告高估了它）：
    ///    `committing_all` 今天**就是**在一个循环里调 `committing` —— 两条"路径"共用
    ///    同一个实现，所以**只有将来有人重写 `committing_all` 时它才会红**。
    ///    它不是恒真（两个**函数入口**确实不同），但它是"低判别力"的那一类。
    ///
    /// ⚠️ **更要紧的一句**：上游那条被删用例真正想守的东西是
    ///    "**三个提交点（回车 / 失焦 / 关面板）接没接上**"，而那**在视图里**、
    ///    按本项目的硬约束**不单测** —— 本波次**没有、也无法**覆盖它。
    ///    这条只补了两个**函数入口**的一致性，**不是**那条判据的替代品。
    #[test]
    fn the_single_commit_and_the_bulk_commit_agree_on_the_same_edit() {
        let drafts = NoteDrafts::new().editing("客户张三", CODE_A);
        let by_enter = drafts.committing(CODE_A, Some("旧的"));
        let by_dismiss = drafts.committing_all(|_| Some("旧的".to_string()));
        assert_eq!(by_enter.writes, by_dismiss.writes);
        assert_eq!(by_enter.drafts, by_dismiss.drafts);
    }
}
