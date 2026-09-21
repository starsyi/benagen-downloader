//! delivery —— 交付清单的拉取、解析与 URL 构造。
//!
//! 逐条移植自 `downloader/internal/delivery/{manifest.go,fetch.go}`（Go 侧已冻结）。
//! 15 条测试与 Go 的两个测试文件一一对应，对应表见
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-2-report.md`。

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 清单里 base_url 缺失时的回退值（规格 §4）
pub const DEFAULT_BASE_URL: &str = "http://download.benagen.com";

/// 交付页文件名，客户端用它提示客户
pub const PAGE_NAME: &str = "index.html";

/// 随机码长度（服务端为 20）
const CODE_LEN: usize = 20;

/// 规格 §4 约束的清单请求参数
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// **总尝试次数**（含首次）：为 3 时实际只重试 2 次
const FETCH_RETRIES: usize = 3;
const FETCH_BASE_DELAY: Duration = Duration::from_millis(500);
/// 清单响应体上限（Go 侧 `io.LimitReader(resp.Body, 64<<20)`）
const MAX_MANIFEST_BYTES: u64 = 64 << 20;

/// 清单里的一个文件条目。
///
/// ⚠️ `#[serde(default)]` 不是随手加的：Go 的 `json.Unmarshal` 对**缺失字段留零值**
/// （`TestFetchSuccess` 的 `{"code":"AbC123","files":[]}` 就是这种形态），
/// serde 默认要求字段齐全，不写就会在 Go 能解析的输入上失败。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct File {
    /// 相对码目录；落盘时**原样**使用（约束 3）
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    /// 可能为空（服务端回读失败）
    #[serde(default)]
    pub crc64: String,
    /// 源文件的修改时间。**原样搬运的不透明字符串**（ISO 8601 带 +08:00），
    /// 内核不解析、不校验、不做时区换算 —— 与 `created_at` 的待遇完全一致。
    /// 可能为空（老清单里没有这个键）。
    #[serde(default)]
    pub source_mtime: String,
}

/// 服务端预生成的交付清单。字段宽容度同 `File`：缺失即零值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub total_files: i64,
    #[serde(default)]
    pub total_bytes: i64,
    #[serde(default)]
    pub files: Vec<File>,
}

impl Manifest {
    /// 某个清单文件的下载 URL。编码方式必须与服务端 `encode_url_path` 一致。
    ///
    /// ⚠️ 逐段百分号编码，且**只保留 A-Za-z0-9-_.~**——必须与服务端
    /// `delivery_manifest.encode_url_path`（Python 的 `quote(seg, safe="")`）逐字节一致，
    /// 否则同一批数据客户端拼出的 URL 与 `urls.txt` 里的不一致，难以排查。
    /// 注意 Go 的 `url.PathEscape` 会保留子分隔符（`& ' ( ) * + , ; =`），与 Python 不同，
    /// 因此不能用它。
    pub fn file_url(&self, path: &str) -> String {
        let segs: Vec<String> = path.split('/').map(escape_segment).collect();
        format!("{}/{}/{}", base(&self.base_url), self.code, segs.join("/"))
    }

    /// 交付是否已过期。`now` 显式传入——Go 的 `Expired(now time.Time)` 同形，
    /// 测试要能传一个固定时刻。
    ///
    /// 解析失败时返回 `false`——宁可让请求去撞 404，也不要因为解析问题把有效交付误判为过期。
    pub fn expired(&self, now: SystemTime) -> bool {
        if self.expires_at.is_empty() {
            return false;
        }
        // 解析失败与「时钟早于 1970」都按「不判过期」处理，理由同上。
        let Some((exp_secs, exp_nanos)) = parse_rfc3339(&self.expires_at) else {
            return false;
        };
        let Ok(since_epoch) = now.duration_since(UNIX_EPOCH) else {
            return false;
        };
        let now_secs = since_epoch.as_secs() as i64;
        now_secs > exp_secs || (now_secs == exp_secs && since_epoch.subsec_nanos() > exp_nanos)
    }
}

/// 实际使用的基址（清单优先，回退默认常量）。对应 Go 的 `(*Manifest).base`。
fn base(base_url: &str) -> String {
    if base_url.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    }
}

/// 从整条链接或裸随机码中提取交付码。
///
/// 两种输入都要支持（规格 §2 约束 3）：客户可能直接粘贴邮件里的链接，
/// 也可能只输入别人告诉他的那串码。
pub fn extract_code(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("请输入交付链接或随机码".to_string());
    }

    // 裸码：长度与字符集都对得上就认为是码本身
    if t.len() == CODE_LEN && is_code_chars(t) {
        return Ok(t.to_string());
    }

    // 链接：取路径里的第一段。
    //
    // ⚠️ **必须带 scheme**：不带 scheme 的链接（如
    // `download.benagen.com/AbCdEfGhIjKlMnOpQrSt/index.html`）里，第一段是主机名，
    // 而 "download.benagen.com" 恰好全是合法码字符且正好 20 字符——
    // 一旦放宽「必须有主机」这条，它就会被误当成交付码（审阅记录 D6）。
    if let Some((scheme, rest)) = t.split_once("://") {
        if !scheme.is_empty() {
            let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            if !rest[..host_end].is_empty() {
                let after_host = &rest[host_end..];
                let path = &after_host[..after_host
                    .find(['?', '#'])
                    .unwrap_or(after_host.len())];
                let seg = path.trim_matches('/').split('/').next().unwrap_or("");
                if !seg.is_empty() && is_code_chars(seg) {
                    return Ok(seg.to_string());
                }
                return Err(format!("链接里没有找到有效的交付码: {s}"));
            }
        }
    }

    // ⚠️ 这段文案会原样出现在错误提示里，**不带 markdown 渲染**：
    // 强调记号（`**`）会被客户当成正文里的星号读出来。要强调就用中文引号。
    Err(format!(
        "无法识别的输入：请粘贴带 http:// 或 https:// 的完整链接，或输入 {CODE_LEN} 位交付码（收到的是 {s:?}）"
    ))
}

/// 解析清单字节，并**校验其中的不可信字段**。
///
/// 为什么在这里校验而不只反序列化：`code` 与 `base_url` 会被原样拼进下载 URL，
/// 而这个清单来自**网络**——是不可信数据进入系统的边界，校验放在这里最省事也最不容易漏。
///
/// ⚠️ **理由必须说对**（口径与 [`crate::engine::safe_rel_path`] 的注释一致，照它写）：
/// Go 侧的真实缺口是 aria2 的 `-i` 输入文件按**空白分词**，URL 里的换行会变成
/// **选项边界**——一个 `"code": "x\n  dir=/tmp/evil"` 的清单能让 `-i` 多出一行，
/// 把落盘位置改到目标目录之外。
/// **但 Rust 不生成 `-i` 文件**（契约 §8 的 `#5`：从第一天就走 JSON-RPC，
/// URL 与选项只经 `addUri` 的结构化字段传递）——**那条注入路径在本内核里不存在**。
///
/// 校验**照样保留**，理由换成另外两条：
///   a. 这份清单是不可信输入：控制字符出现在 `code`/`base_url` 里，就是**清单已损坏
///      或被构造**的信号；
///   b. 纵深防御：这两个值还会进入错误提示、日志与将来的任何消费者，将来若有人在某处
///      把它序列化成行式文本，守卫已经在了。
///
/// 按 Go 原样拒绝是**对的**（两边行为一致正是逐条移植要保的东西），
/// 只是**别把理由写错**——写错的理由会让后来者以为守着一道已经不存在的边界。
pub fn parse(data: &[u8]) -> Result<Manifest, String> {
    let m: Manifest =
        serde_json::from_slice(data).map_err(|e| format!("清单 JSON 解析失败: {e}"))?;
    if m.code.is_empty() {
        return Err("清单缺少 code 字段".to_string());
    }
    if !is_code_chars(&m.code) {
        return Err(format!("清单的 code 含非法字符: {:?}", m.code));
    }
    if !m.base_url.is_empty() {
        validate_base_url(&m.base_url)?;
    }
    Ok(m)
}

/// 要求 `base_url` 是一个干净的 http/https 地址。
/// 理由同 `parse` 的注释：它同样会被拼进 URL 行。
fn validate_base_url(s: &str) -> Result<(), String> {
    if s.contains(['\n', '\r', '\0']) {
        return Err("清单的 base_url 含控制字符".to_string());
    }
    // 其余控制字符同样非法——Go 侧由 `net/url.Parse` 拦下（实测：制表符报
    // "invalid control character in URL"），这里显式补上，免得漏网。
    if s.bytes().any(|c| c < 0x20 || c == 0x7F) {
        return Err("清单的 base_url 无法解析: 含非法控制字符".to_string());
    }

    // 协议：`scheme://` 的 scheme（Go 的 url.Parse 会把它小写化）
    let scheme = match s.find("://") {
        Some(i) => s[..i].to_ascii_lowercase(),
        None => String::new(),
    };
    if scheme != "http" && scheme != "https" {
        return Err(format!("清单的 base_url 协议不支持: {scheme:?}"));
    }

    // 主机名：`://` 之后到第一个 '/'、'?'、'#' 之前
    let rest = &s[s.find("://").expect("前面已确认存在") + 3..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..host_end].is_empty() {
        return Err("清单的 base_url 缺少主机名".to_string());
    }
    Ok(())
}

/// 逐段百分号编码，只保留 `A-Za-z0-9-_.~`（Python `quote(seg, safe="")` 的保留集）。
fn escape_segment(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 0x0F) as usize] as char);
        }
    }
    out
}

/// 交付码字符集：字母 + 数字 + `-_.`（服务端 `RANDOM_ALPHABET`）。
fn is_code_chars(s: &str) -> bool {
    s.bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// HTTP 传输抽象。生产用 `UreqTransport`，测试注入桩——
/// 与 Go 的 `roundTripFunc` 同形，让 15 条测试能逐条平移。
///
/// ⚠️ **错误串契约**：`get` 的错误串若以 `HTTP <状态码>` 开头（由
/// [`http_status_error`] 构造），`fetch` 按状态码判定是否重试——`>= 500` 可重试，
/// 其余（4xx 等）是确定性失败；其他一切错误（连接失败、超时、读体失败）视为可重试。
/// Go 侧由 `once` 的第二个返回值 `retryable` 表达同一件事，这里因为签名固定为
/// `Result<Vec<u8>, String>` 而压进错误串。
pub trait Transport {
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
    /// 对对象发 HEAD，读 **`X-Tos-Hash-Crc64ecma`** 响应头。
    ///
    /// ⚠️ 返回的是 **CRC**，不是长度。清单里该字段为空时（服务端回读失败）
    /// 靠它补齐；拿不到就返回 `None`（调用方记入 `unverifiable`，**仍然下载**）。
    /// 与 Go 的 `headCRC64` 逐字对应（见 `planner.go:120-143`）。
    fn head_crc64(&self, url: &str) -> Option<String>;
}

/// 生产用传输：`ureq` 实现。
///
/// ⚠️ `agent` 是**结构体字段**，不是每次调用新建：`ureq::Agent` 自带连接池，
/// 它就是 Go 侧 `http.DefaultTransport` 的等价物（`planner.go:120-143` 的 `headCRC64`
/// 拿的是调用方传入的 client，落在共享的 transport 上，逐文件 HEAD **复用连接**）。
/// `head_crc64` 是「清单里 crc64 为空」时的**逐文件**路径：一次交付可能上千个文件，
/// 每个都重来一次 TCP(+TLS) 握手是白扔的开销。
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(FETCH_TIMEOUT).build(),
        }
    }
}

// 与 `new()` 成对（clippy 的 `new_without_default`）：让调用方与测试都能写
// `UreqTransport::default()`，不必记住构造器名字。
impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        match self.agent.get(url).call() {
            Ok(resp) => {
                let mut buf = Vec::new();
                // 与 Go 的 `io.LimitReader(resp.Body, 64<<20)` 同义：超过上限**截断**而非报错
                resp.into_reader()
                    .take(MAX_MANIFEST_BYTES)
                    .read_to_end(&mut buf)
                    .map_err(network_error)?;
                Ok(buf)
            }
            // 404 单独给文案：交付码打错是最常见的失败，值得一句能照做的提示。
            Err(ureq::Error::Status(404, _)) => Err(http_status_error(
                404,
                &format!("清单不存在（404）——请确认交付码是否正确：{url}"),
            )),
            Err(ureq::Error::Status(code, _)) if code >= 500 => {
                Err(http_status_error(code, &format!("服务端错误 {code}")))
            }
            Err(ureq::Error::Status(code, _)) => Err(http_status_error(code, "")),
            Err(ureq::Error::Transport(e)) => Err(network_error(e)),
        }
    }

    fn head_crc64(&self, url: &str) -> Option<String> {
        let resp = self.agent.head(url).call().ok()?;
        // Go 侧 `resp.Header.Get` 在没有该头时返回 ""，这里用 `None` 表示同一件事：
        // 只有拿到**非空**的 CRC 才算补齐成功。
        resp.header("X-Tos-Hash-Crc64ecma")
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    }
}

/// 拉取清单：超时 + 指数退避重试；**4xx 不重试**。
///
/// 重试覆盖网络错误与 5xx；4xx 是确定性失败，不重试——重试一个 404 只是浪费客户时间。
pub fn fetch<T: Transport>(t: &T, base_url: &str, code: &str) -> Result<Manifest, String> {
    // 调用方给的基址同样会被拼进下载 URL，所以和清单里带的一样不可信——
    // 在入口处一并校验，别留第二条路径（审阅记录 D21）。
    // （这条校验在 Rust 侧的理由见 `parse` 的注释：我们**不生成 `-i` 文件**，
    // 拒的是"不可信输入里出现控制字符"这个信号本身，不是那条已经不存在的选项边界。）
    validate_base_url(base_url)?;
    let url = format!("{}/{}/manifest.json", base(base_url), code);

    let mut last_err = String::new();
    for attempt in 0..FETCH_RETRIES {
        if attempt > 0 {
            // Go 侧是 `time.After(baseDelay * 1<<(attempt-1))`，指数退避
            std::thread::sleep(FETCH_BASE_DELAY * (1u32 << (attempt - 1)));
        }
        match t.get(&url) {
            Ok(raw) => {
                let mut m = parse(&raw)?;
                // 清单没声明 base_url 时，填上我们**实际取得它的那个基址**——
                // 文件与清单必然同源，用取回地址比回退到硬编码常量更可靠，
                // 也让测试可以指向本地桩（否则会打到真实 CDN）。
                if m.base_url.is_empty() {
                    m.base_url = base_url.to_string();
                }
                return Ok(m);
            }
            Err(e) => {
                // 确定性失败立即返回（Go 侧 `once` 的 retryable=false），不包"已尝试"文案
                if !is_retryable(&e) {
                    return Err(e);
                }
                last_err = e;
            }
        }
    }
    // 注意：`FETCH_RETRIES` 是**总尝试次数**（含首次），所以文案说"已尝试"而不是"已重试"——
    // 否则会在用户可见的报错里与事实差一（3 时实际只重试了 2 次）。
    Err(format!(
        "拉取清单失败（已尝试 {FETCH_RETRIES} 次）: {last_err}"
    ))
}

/// 判断 `Transport::get` 的错误是否可重试（错误串契约见 [`Transport`]）。
pub fn is_retryable(err: &str) -> bool {
    match http_status(err) {
        Some(code) => code >= 500,
        None => true,
    }
}

/// 从错误串里取出 `Transport` 写入的 HTTP 状态码。
pub fn http_status(err: &str) -> Option<u16> {
    let rest = err.strip_prefix("HTTP ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// 构造带状态码的错误串（**`Transport` 实现用它表达确定性失败**）。
pub fn http_status_error(status: u16, detail: &str) -> String {
    if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    }
}

/// 网络/传输层错误（可重试）。
fn network_error(detail: impl std::fmt::Display) -> String {
    format!("网络错误: {detail}")
}

/// 手写 RFC3339 解析，返回 `(Unix 秒, 纳秒)`。
///
/// 只认 Go `time.Parse(time.RFC3339, …)` 认得的形态（已用 Go 1.26 实测逐条核对）：
/// `YYYY-MM-DDTHH:MM:SS`（`T`/`Z` 必须大写）＋可选小数秒＋`Z` 或 `±HH:MM`
/// （偏移的冒号不可省），且**不接受**尾随内容、不接受越界的月/日/时/分/秒。
///
/// 不引入日期库：`std::time` 只给「距 1970 的秒数」，日历换算自己写（约 20 行）。
/// 解析失败返回 `None`——调用方据此「不判过期」。
fn parse_rfc3339(s: &str) -> Option<(i64, u32)> {
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
    let year = digits(b, 0, 4)?;
    let month = digits(b, 5, 2)?;
    let day = digits(b, 8, 2)?;
    let hour = digits(b, 11, 2)?;
    let minute = digits(b, 14, 2)?;
    let second = digits(b, 17, 2)?;
    // 写成 `!(1..=n).contains(..)` 而不是 `x < 1 || x > n`：语义逐字相同，
    // 只是 clippy 的 `manual_range_contains` 在 `-D warnings` 下不接受后者。
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut i = 19;
    let mut nanos = 0u32;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None; // Go 侧：'.' 后至少要有一位数字
        }
        // 只取前 9 位（Go 截断到纳秒）
        let mut scale = 100_000_000u32;
        for &c in &b[start..(start + 9).min(i)] {
            nanos += u32::from(c - b'0') * scale;
            scale /= 10;
        }
    }

    let offset_secs: i64 = match b.get(i)? {
        b'Z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') => {
            if i + 6 != b.len() || b[i + 3] != b':' {
                return None;
            }
            let oh = digits(b, i + 1, 2)?;
            let om = digits(b, i + 4, 2)?;
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

    let days = days_from_civil(year, month, day);
    Some((
        days * 86400 + hour * 3600 + minute * 60 + second - offset_secs,
        nanos,
    ))
}

/// 读 `b[i..i+n]` 的 n 位十进制数；位数不足或有非数字则返回 `None`。
fn digits(b: &[u8], i: usize, n: usize) -> Option<i64> {
    let mut v = 0i64;
    for &c in b.get(i..i + n)? {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + i64::from(c - b'0');
    }
    Some(v)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// 公历日期 → 距 1970-01-01 的天数（Howard Hinnant 的 `days_from_civil`，全整数运算）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // 3 月 = 0
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// 测试桩基址：**必须是合法基址**（要过 `validate_base_url`），但不会被真正连上——
    /// `Stub` 不碰网络。对应 Go 侧 `httptest.NewServer` 给出的 `srv.URL`。
    const STUB_BASE: &str = "http://127.0.0.1:18080";

    /// 与 Go 的 `roundTripFunc` 同形的桩传输：按脚本逐次返回，用完后重复最后一项。
    struct Stub {
        calls: Cell<usize>,
        ok_calls: Cell<usize>,
        last_url: RefCell<String>,
        script: Vec<Result<&'static str, String>>,
    }

    impl Stub {
        fn new(script: Vec<Result<&'static str, String>>) -> Self {
            Self {
                calls: Cell::new(0),
                ok_calls: Cell::new(0),
                last_url: RefCell::new(String::new()),
                script,
            }
        }

        /// 每次都返回同一个错误（用于「不该发出请求」「重试耗尽」两类断言）。
        fn always(err: String) -> Self {
            Self::new(vec![Err(err)])
        }

        fn calls(&self) -> usize {
            self.calls.get()
        }

        fn ok_calls(&self) -> usize {
            self.ok_calls.get()
        }

        fn last_url(&self) -> String {
            self.last_url.borrow().clone()
        }
    }

    impl Transport for Stub {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            *self.last_url.borrow_mut() = url.to_string();
            let idx = (self.calls.get() - 1).min(self.script.len() - 1);
            match &self.script[idx] {
                Ok(body) => {
                    self.ok_calls.set(self.ok_calls.get() + 1);
                    Ok(body.as_bytes().to_vec())
                }
                Err(e) => Err(e.clone()),
            }
        }

        fn head_crc64(&self, _url: &str) -> Option<String> {
            None
        }
    }

    /// Go 侧 `&Manifest{Code: "AbC", BaseURL: ...}` 结构体字面量的等价物。
    fn manifest(code: &str, base_url: &str) -> Manifest {
        Manifest {
            code: code.to_string(),
            base_url: base_url.to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 0,
            total_bytes: 0,
            files: Vec::new(),
        }
    }

    fn manifest_with_expiry(expires_at: &str) -> Manifest {
        Manifest {
            expires_at: expires_at.to_string(),
            ..manifest("X", "")
        }
    }

    /// 把清单序列化成 JSON。
    ///
    /// ⚠️ 必须用 serde_json **序列化**而不是手拼字符串：base_url 里的真换行若直接拼进
    /// JSON 文本，JSON 本身就失效了——`parse` 会先报语法错误，在走到 `validate_base_url`
    /// **之前**就返回。那样测的是「JSON 语法」，控制字符这条规则根本没被验证
    /// （变异体删掉控制字符检查，测试照样绿）。
    fn must_manifest_json(base_url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "code": "AbCdEfGhIjKlMnOpQrSt",
            "base_url": base_url,
            "files": [],
        }))
        .expect("序列化清单失败")
    }

    #[test]
    fn extract_code_from_url() {
        let cases = [
            ("http://download.benagen.com/AbC123._-x/index.html", "AbC123._-x"),
            ("http://download.benagen.com/AbC123._-x/test/a/b.txt", "AbC123._-x"),
            ("https://download.benagen.com/AbC123._-x/", "AbC123._-x"),
            // 裸码：必须是 20 位（规格 §4「输入本身就是 20 位 [A-Za-z0-9._-]」），
            // 且符合服务端 RANDOM_ALPHABET（字母+数字+"-_."）
            ("AbC123._-xyzAbC123._", "AbC123._-xyzAbC123._"),
        ];
        for (input, want) in cases {
            match extract_code(input) {
                Err(e) => panic!("extract_code({input:?}) 报错: {e}"),
                Ok(got) => assert_eq!(got, want, "extract_code({input:?})"),
            }
        }
    }

    #[test]
    fn extract_code_rejects() {
        for input in ["", "   ", "短", "http://x/", "含有 空格 的码"] {
            assert!(
                extract_code(input).is_err(),
                "extract_code({input:?}) 应当报错"
            );
        }
    }

    #[test]
    fn extract_code_rejects_scheme_less_link() {
        // 不带 scheme 的链接必须被拒绝：其第一段是主机名，而
        // "download.benagen.com" 全是合法码字符且正好 20 字符，
        // 一旦放宽就会被误当成码。
        assert!(
            extract_code("download.benagen.com/AbCdEfGhIjKlMnOpQrSt/index.html").is_err(),
            "不带 scheme 的链接应当报错"
        );
    }

    #[test]
    fn extract_code_bare_code_must_be_exact_length() {
        assert!(
            extract_code("AbC123._-x").is_err(),
            "11 位的裸码应当报错（规格 §4 要求 20 位）"
        );
        let good = "AbCdEfGhIjKlMnOpQrSt"; // 20 位
        match extract_code(good) {
            Ok(got) => assert_eq!(got, good, "20 位裸码应被接受"),
            Err(e) => panic!("20 位裸码应被接受: err={e}"),
        }
    }

    #[test]
    fn file_url_escapes_like_server() {
        // 必须与服务端 delivery_manifest.encode_url_path 逐字节一致
        // （Python: quote(seg, safe="")，即只保留 A-Za-z0-9-_.~）
        let m = manifest("AbC", "http://download.benagen.com");
        let cases = [
            (
                "test/C24-8_×_25WS024/Figure/QC 图.png",
                "http://download.benagen.com/AbC/test/C24-8_%C3%97_25WS024/Figure/QC%20%E5%9B%BE.png",
            ),
            (
                "test/a/b/c/deep.txt",
                "http://download.benagen.com/AbC/test/a/b/c/deep.txt",
            ),
            ("x/&?#.txt", "http://download.benagen.com/AbC/x/%26%3F%23.txt"),
            // '~' 属于 Python quote 的 always_safe，必须原样保留；
            // 子分隔符 '()*+,;=!' 与 '%' 则必须编码——url.PathEscape 会漏掉前一组，
            // 直接把 '%' 原样输出则会让 URL 出现歧义。
            ("t/a~b.txt", "http://download.benagen.com/AbC/t/a~b.txt"),
            (
                "t/'()*+,;=!%.txt",
                "http://download.benagen.com/AbC/t/%27%28%29%2A%2B%2C%3B%3D%21%25.txt",
            ),
        ];
        for (path, want) in cases {
            assert_eq!(m.file_url(path), want, "file_url({path:?})");
        }
    }

    #[test]
    fn base_url_fallback() {
        // 清单里 base_url 为空时回退到默认常量
        let m = manifest("X", "");
        assert_eq!(
            m.file_url("a.txt"),
            format!("{DEFAULT_BASE_URL}/X/a.txt"),
            "回退失败"
        );
    }

    #[test]
    fn expired() {
        // 固定时刻：2026-10-14T12:00:00Z。
        //
        // ⚠️ 这个取值**贴着边界**，不是随手取的：两条真用例分别在它前后 20 小时与 8 小时，
        // 于是「时区偏移符号取反」这类缺陷会翻转判决（`+08:00` 被读成 `-08:00`，
        // 已过期的那条会跑到 now 之后）。旧取值（距边界 13 天 / 30 天）翻不动任何一条，
        // 变异体 M7 因此在旧夹具下存活——夹具是为判别力服务的，故按此重取。
        // 两种偏移方向上的日历误差同样各有一条守着：未过期那条 `-1` 天会翻、已过期那条 `+1` 天会翻。
        let now = UNIX_EPOCH + Duration::from_secs(1_791_979_200);
        // 2026-10-15T16:00:00+08:00 = 2026-10-15T08:00:00Z
        let m = manifest_with_expiry("2026-10-15T16:00:00+08:00");
        assert!(!m.expired(now), "未过期却判为过期");
        // 2026-10-14T12:00:00+08:00 = 2026-10-14T04:00:00Z（now 前 8 小时）
        let m2 = manifest_with_expiry("2026-10-14T12:00:00+08:00");
        assert!(m2.expired(now), "已过期却判为未过期");
        // 时间无法解析时不得判为过期：宁可让请求去撞 404，
        // 也不要因为解析问题把有效交付误报为过期而挡住下载。
        // `2026-10-14 06:00:00` 是**过去**的时刻（若换行外的空格被误当成合法分隔符，
        // 解析出来会判过期，本断言即变红）——换成未来时刻这条就成空断言了。
        for bad in ["", "不是时间", "2026-10-14 06:00:00"] {
            let m3 = manifest_with_expiry(bad);
            assert!(!m3.expired(now), "expires_at={bad:?} 无法解析，却判为过期");
        }
    }

    #[test]
    fn parse_rejects_injected_code() {
        // code 会被拼进下载 URL。**Rust 侧不生成 `-i` 文件**（契约 §8 的 `#5`），
        // 所以拒它的理由不是"选项边界"，而是"不可信清单里出现控制字符 =
        // 清单已损坏或被构造"这个信号——完整口径见 `parse` 的注释。
        let raw = br#"{"code":"x\n  dir=/tmp/evil","files":[]}"#;
        assert!(parse(raw).is_err(), "含换行的 code 应当被拒");
        // 正常码不得被误拒
        let good = br#"{"code":"AbCdEfGhIjKlMnOpQrSt","files":[]}"#;
        if let Err(e) = parse(good) {
            panic!("正常 code 不应被拒: {e}");
        }
    }

    #[test]
    fn parse_rejects_bad_base_url() {
        let bad = [
            ("http://x/\n  dir=/tmp/evil", "控制字符——换行就是 aria2 -i 的选项边界"),
            ("file:///etc", "协议不支持"),
            ("/no-scheme", "缺协议"),
            // 下面两条各自**只能**被一条规则拦下，用来把三条规则分别钉住：
            // 去掉任一条规则，就有一条用例变红（否则三条规则可以互相顶替）。
            ("ftp://host/x", "协议不支持（主机名非空，协议规则是唯一拦截者）"),
            ("http:///x", "缺主机名（协议合法，主机规则是唯一拦截者）"),
        ];
        for (url, why) in bad {
            assert!(
                parse(&must_manifest_json(url)).is_err(),
                "base_url {url:?} 应当被拒（{why}）"
            );
        }
        // 正常基址不得被误拒（含带端口与路径的形态）
        for good in ["http://download.benagen.com", "https://a.b:8443/x"] {
            if let Err(e) = parse(&must_manifest_json(good)) {
                panic!("正常 base_url {good:?} 不应被拒: {e}");
            }
        }
    }

    #[test]
    fn parse_manifest() {
        let raw = r#"{
          "code":"AbC","base_url":"http://download.benagen.com",
          "expires_at":"2026-10-14T16:13:34+08:00",
          "total_files":2,"total_bytes":26,
          "files":[
            {"path":"t/Readme.txt","size":6,"crc64":"5432380796884633278"},
            {"path":"t/中文 名.bin","size":20,"crc64":""}
          ]}"#;
        let m = match parse(raw.as_bytes()) {
            Ok(m) => m,
            Err(e) => panic!("解析失败: {e}"),
        };
        assert!(
            m.code == "AbC" && m.files.len() == 2 && m.total_bytes == 26,
            "解析结果不符: {m:?}"
        );
        assert_eq!(m.files[1].crc64, "", "空 crc64 应保持为空，不得填默认值");
    }

    /// 已交付的老清单里**没有** `source_mtime` 这个键。向后兼容是硬要求：
    /// 必须照常解析成功，该字段是**空串**（与 `crc64` 同一种做法——
    /// 「可能为空」的字符串，不是可选键）。
    #[test]
    fn parse_tolerates_a_manifest_without_source_mtime() {
        let raw = r#"{
          "code":"AbC","base_url":"http://download.benagen.com",
          "files":[{"path":"t/a.txt","size":6,"crc64":"1"}]}"#;
        let m = match parse(raw.as_bytes()) {
            Ok(m) => m,
            Err(e) => panic!("老清单（没有 source_mtime）必须照常解析成功: {e}"),
        };
        assert_eq!(
            m.files[0].source_mtime, "",
            "缺这个键时必须是空串（壳据此判断「无时间可显示」），不得报错、不得填占位值"
        );
    }

    /// 新清单里的时间**逐字**留着：内核不解析、不校验、不做时区换算
    /// （与 `created_at` 的待遇完全一致）。
    ///
    /// 判别力来自取值本身：一旦被「顺手规范化」成 UTC，`+08:00` 会变成 `Z`/`+00:00`，
    /// 逐字比较立刻变红。第二条用例喂一个**根本不是时间**的值——内核没有校验这一环，
    /// 它必须原样通过（加了校验就会红）。
    #[test]
    fn parse_keeps_source_mtime_verbatim() {
        for mtime in ["2026-09-14T12:00:00+08:00", "昨天下午"] {
            let raw = format!(
                r#"{{"code":"AbC","files":[{{"path":"t/a.txt","size":6,"crc64":"1","source_mtime":"{mtime}"}}]}}"#
            );
            let m = match parse(raw.as_bytes()) {
                Ok(m) => m,
                Err(e) => panic!("source_mtime={mtime:?} 不该让解析失败（内核不校验它）: {e}"),
            };
            assert_eq!(
                m.files[0].source_mtime, mtime,
                "必须逐字保留，不得解析/换算时区/校验"
            );
        }
    }

    #[test]
    fn fetch_success() {
        let t = Stub::new(vec![Ok(r#"{"code":"AbC123","files":[]}"#)]);
        let m = match fetch(&t, STUB_BASE, "AbC123") {
            Ok(m) => m,
            Err(e) => panic!("拉取失败: {e}"),
        };
        assert_eq!(m.code, "AbC123", "code 不对");
        // 清单未声明 base_url，必须填回我们实际取得它的那个基址。
        // 否则回退到 DEFAULT_BASE_URL（真实 CDN），后续文件请求会打到线上，
        // 本地桩的测试就失去意义了。
        assert_eq!(m.base_url, STUB_BASE, "base_url 应回填为取回地址");
        // 请求路径必须是 <基址>/<码>/manifest.json
        assert_eq!(
            t.last_url(),
            format!("{STUB_BASE}/AbC123/manifest.json"),
            "请求路径不对"
        );
    }

    #[test]
    fn fetch_retries_then_succeeds() {
        // 前两次 503，第三次成功
        let t = Stub::new(vec![
            Err(http_status_error(503, "服务端错误 503")),
            Err(http_status_error(503, "服务端错误 503")),
            Ok(r#"{"code":"X"}"#),
        ]);
        if let Err(e) = fetch(&t, STUB_BASE, "X") {
            panic!("重试后仍失败: {e}");
        }
        assert_eq!(t.calls(), 3, "请求次数");
    }

    #[test]
    fn fetch_does_not_retry_4xx() {
        // 必须断言**请求次数**：只断言 err 非空的话，
        // 「404 被重试了 3 次」同样能通过——那正是本测试要防的行为。
        //
        // 表驱动覆盖 403/404/410：Go 侧 404 走专用分支、从不执行通用 4xx 分支；
        // Rust 侧的确定性失败分类统一由 `http_status_error` 带出状态码，
        // 三种码都要测，通用分支才有人守护。
        for status in [403u16, 404, 410] {
            let t = Stub::always(http_status_error(status, ""));
            let err = match fetch(&t, STUB_BASE, "X") {
                Err(e) => e,
                Ok(_) => panic!("HTTP {status} 应当报错"),
            };
            assert_eq!(
                t.calls(),
                1,
                "4xx 是确定性失败，不应重试（HTTP {status}）；实际报错 {err}"
            );
        }
    }

    #[test]
    fn fetch_retries_network_error() {
        // 网络错误是暂时性的，必须在**同一次 fetch 内**重试并最终成功。
        //
        // ⚠️ 判别力：若写成「第一次 fetch 失败 → 恢复 → 再来一次 fetch 成功」，
        // 完全不重试网络错误的实现也能通过（第一次照样报错，第二次照样成功）。
        // 所以必须让「失败 → 恢复」发生在同一次 fetch 内，并断言 get 次数。
        let t = Stub::new(vec![
            Err(network_error("connection refused")),
            Err(network_error("connection refused")),
            Ok(r#"{"code":"X"}"#),
        ]);
        let m = match fetch(&t, STUB_BASE, "X") {
            Ok(m) => m,
            Err(e) => panic!("网络错误应当被重试并在恢复后成功: {e}"),
        };
        assert_eq!(m.code, "X", "code 不对");
        assert_eq!(t.calls(), 3, "get 次数（2 次网络错误 + 1 次成功）");
        assert_eq!(t.ok_calls(), 1, "桩被成功请求次数");

        // 网络错误持续存在时，重试耗尽后必须报错——且必须真的重试过
        let always_fail = Stub::always(network_error("connection refused"));
        assert!(
            fetch(&always_fail, STUB_BASE, "X").is_err(),
            "网络错误且重试耗尽后应当报错"
        );
        assert_eq!(always_fail.calls(), 3, "重试耗尽时 get 次数");
    }

    #[test]
    fn get_rejects_bad_caller_base_url() {
        // 调用方给的基址同样是不可信输入，不能绕过校验（Rust 侧的理由见 `parse` 的注释：
        // 我们不生成 `-i` 文件，拒的是"不可信值里有控制字符"这个信号）。
        //
        // ⚠️ 只断言 err 非空是**不判别**的：把校验整段删掉，下面两个基址照样会失败——
        // `file:///etc` 与带 CR 的基址都会在更下层撞墙——err 一样非空，测试照样绿。
        // 必须断言**错误来自校验本身**：`validate_base_url` 的四种报错都含字面量
        // "base_url"，而网络/解析层的报错都不含。再加一条行为证据：
        // 非法基址**一个请求都不该发出**。
        for b in ["file:///etc", "http://x/\r  dir=/tmp"] {
            let t = Stub::always(network_error("不该走到网络层"));
            let err = match fetch(&t, b, "AbCdEfGhIjKlMnOpQrSt") {
                Err(e) => e,
                Ok(_) => panic!("基址 {b:?} 应当被拒"),
            };
            assert!(
                err.contains("base_url"),
                "基址 {b:?} 应当**在校验处**被拒，实际报错来自别处: {err}"
            );
            assert_eq!(
                t.calls(),
                0,
                "基址 {b:?} 不应发出任何请求（说明它没在校验处被拦下）"
            );
        }
        // 正常基址不得被误拒——注意这里不需要真的连上，只要不是「被 validate_base_url
        // 拒绝」即可；用一个必然连不上的地址验证它走到了网络层而不是被校验拦下。
        // 这一条走**真实** `UreqTransport`（127.0.0.1:1 立刻 ECONNREFUSED），
        // 与 Go 侧用真实 http.Client 的形态一致。
        let err = match fetch(&UreqTransport::new(), "http://127.0.0.1:1/", "AbCdEfGhIjKlMnOpQrSt") {
            Err(e) => e,
            Ok(_) => String::new(),
        };
        assert!(
            !err.contains("base_url"),
            "正常基址被 validate_base_url 误拒了: {err}"
        );
    }
}
