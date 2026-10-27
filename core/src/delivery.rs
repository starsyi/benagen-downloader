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

/// 最多下发给 aria2 的 IPv4 备胎数（域名 URI 不计入）。
///
/// 为什么是 16：1 条域名 URI + 16 条备胎 = 17 ≥ `splits` 上限 16
/// （校验见 `settings.rs:111`，加载侧回退见 `settings.rs:324`）⇒ 每个分片都能落在
/// 不同腿上，域名腿不白占名额。再往上加只会把 aria2 的 URI 表撑大，换不来更多分片。
pub const MAX_BACKUP_URIS: usize = 16;

/// 随机码长度（服务端为 20）
const CODE_LEN: usize = 20;

/// 规格 §4 约束的清单请求参数
///
/// ## 2026-10-12 调参（现场：客户机上 `拉取清单失败（已尝试 3 次）…os error 10060`）
///
/// 旧值是「每次 30 秒、共 3 次、退避 0.5 / 1 秒」。10060 是**连接阶段**超时：对端不回 SYN，
/// 一次尝试就白等 30 秒，三次下来 90 秒全花在等一个不会来的回音上，而重试之间几乎没有间隔——
/// 网络恢复需要的是**时间**，不是更多次紧挨着的敲门。新值：
///   * 连接阶段单独设 [`FETCH_CONNECT_TIMEOUT`]（8 秒）：连不上就尽早换下一次；
///   * 单次**读**时限 15 秒（"一个字节都等不到"才算超时；慢链路只要还在来数据就继续读，
///     见 `UreqTransport::new` 里的注释——2026-10-27 十万文件批次就死在改成整体时限之前）；
///   * 共 5 次，退避 1 / 2 / 4 / 8 秒（比旧的拉长一个量级，给网络一个喘息）。
///
/// 最坏耗时 = 5 × 15 + (1+2+4+8) = **90 秒**，仍在原先 91.5 秒的记账之内
/// （`kernel.rs` / `client.rs` 里「三段顺序执行」的预算账不用改；由
/// `fetch_worst_case_stays_inside_the_booked_91_5_seconds` 钉住）。
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// 连接阶段的上限（见上）。
const FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// **总尝试次数**（含首次）：为 5 时实际重试 4 次
const FETCH_RETRIES: usize = 5;
/// 退避的基数：第 n 次重试前等 `基数 × 2^(n-1)`。
const FETCH_BASE_DELAY_PROD: Duration = Duration::from_secs(1);
/// 测试里退避压到毫秒级（逻辑不变，只是别让「5 次全失败」的用例真睡 15 秒）。
#[cfg(not(test))]
const FETCH_BASE_DELAY: Duration = FETCH_BASE_DELAY_PROD;
#[cfg(test)]
const FETCH_BASE_DELAY: Duration = Duration::from_millis(2);
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
        file_url_on(&base(&self.base_url), &self.code, path)
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

/// 把 `file_url` 的公共段抽出来：base + code + 逐段转义的 path。三处（域名 URI、IP URI、
/// urls.txt）必须同一份转义，任何一份另写都会与清单对不上。
pub(crate) fn file_url_on(base: &str, code: &str, path: &str) -> String {
    let segs: Vec<String> = path.split('/').map(escape_segment).collect();
    format!("{}/{}/{}", base, code, segs.join("/"))
}

/// 域名 URI 打头 + IP 备胎（规格 §5.1）。`ips` 为空 ⇒ 只有域名 URI。
/// 每个 IP 的 URI 形如 `http://<ip>[:端口][/base路径]/<code>/<path>`；
/// **只接受 http 的 base**，https 的 base 在上一层就被掐掉（resolve_for_base 返回空），
/// 这里再兜一道。备胎**上限 [`MAX_BACKUP_URIS`] 个**（理由见常量注释）。
pub fn uri_list(m: &Manifest, path: &str, ips: &[std::net::IpAddr]) -> Vec<String> {
    let base = base(&m.base_url); // 空 ⇒ DEFAULT_BASE_URL，与 file_url 同一回退
    let primary = file_url_on(&base, &m.code, path);
    if !base.starts_with("http://") || ips.is_empty() {
        return vec![primary];
    }
    // 把 base 切成 `(host[:port], /base路径…)`：与 `authority_of`/`host_header_for`
    // 共用 `split_base` 这同一份切分（不许写第二份）。base 路径原样保留
    // （不 Join、不规范化，与落盘路径同一纪律）。
    let (authority, base_path) = split_base(&base);
    // 端口后缀：只在 `:` 后**非空且全数字**时保留（堵 `http://host:` 的尾冒号）。
    let port_suffix = match authority.rfind(':') {
        Some(i) if is_port_digits(&authority[i + 1..]) => &authority[i..],
        _ => "",
    };
    let mut out = Vec::with_capacity(1 + ips.len().min(MAX_BACKUP_URIS));
    out.push(primary);
    for ip in ips.iter().take(MAX_BACKUP_URIS) {
        // IPv6 的 Display 不带方括号，必须手动包——否则拼出 `http://2001:db8::1/…` 是畸形 URI。
        let host = match ip {
            std::net::IpAddr::V4(v4) => v4.to_string(),
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        };
        let backup_base = format!("http://{host}{port_suffix}{base_path}");
        out.push(file_url_on(&backup_base, &m.code, path));
    }
    out
}

/// 把一条 URI 的主机换成给定 IP，其余（端口、路径、查询串）**原样保留**。
///
/// 用于换腿线程：`aria2.getServers` 交回的腿可能是域名 URI，而我们要拿新解析出的 IP
/// 造一条可加入的腿——拼出来的串必须与当初 `uri_list` 造的 IP 备份**同一形状**，
/// 否则 aria2 会把同一份文件当成两条不同的 URI（重复下载/校验错位）。
///
/// 只接受 `http://host[:port]/…`：
/// - 非 http（https 换主机名会打坏 SNI 与证书校验，与 [`uri_list`] 的备份同一道门）⇒ `None`；
/// - 没有 authority / 端口不合法 / 带 userinfo / 方括号不闭合 ⇒ `None`（**认不出就不动**，
///   这是安全方向：本函数的调用方一出 `None` 就只是"这一拍不动作"）；
/// - 主机**已经**是同一个 IP ⇒ `None`——换出来会与输入逐字节相同，调用方应当把它当
///   "无需动作"，而不是往 `changeUri` 里塞一条重复 URI（那只会白占一个并发位）。
pub fn uri_with_host(uri: &str, ip: &std::net::IpAddr) -> Option<String> {
    let rest = uri.strip_prefix("http://")?;
    // 与 `uri_list` 共用同一份 authority 切分（`split_base`）：`:` 之后到第一个 `/`
    // 是 `host[:port]`，从 `/` 起是路径。查询串可能紧跟在 authority 后
    // （`http://h?x=1`，没有路径），所以 authority 里出现 `?`/`#` 时也要切掉，
    // 切下来的部分与路径一起原样拼回。
    let (authority_head, base_path) = split_base(rest);
    let cut = authority_head
        .find(['?', '#'])
        .unwrap_or(authority_head.len());
    let authority = &authority_head[..cut];
    let query = &authority_head[cut..];
    if authority.is_empty() {
        return None;
    }
    // 带 userinfo 的 URI 本产品从不生成（凭据走 `header`/`referer`），
    // 认不出它的主机边界就不动——别把 `user:pass@` 当成主机名的一部分替换。
    if authority.contains('@') {
        return None;
    }
    // `bracketed` 记「这个 host 原本在方括号里」——方括号内的冒号是**地址的一部分**，
    // 不是端口分隔符，所以下面那道「host 不许含 `:`」的畸形门不能套在它身上。
    let (host, port_suffix, bracketed) = if let Some(after_open) = authority.strip_prefix('[') {
        // IPv6 方括号形态：`[::1]` 或 `[::1]:8080`。缺右括号 ⇒ 畸形。
        let end = after_open.find(']')?;
        let host = &after_open[..end];
        let after = &after_open[end + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) if is_port_digits(p) => after,
            Some(_) => return None,
            None if after.is_empty() => "",
            // 方括号后还跟着别的东西（`[::1]x`）——不是我们认识的形状。
            None => return None,
        };
        (host, port, true)
    } else {
        match authority.rfind(':') {
            Some(i) if is_port_digits(&authority[i + 1..]) => {
                (&authority[..i], &authority[i..], false)
            }
            // 冒号后不是合法端口 ⇒ 畸形。**包括裸 IPv6**（`http://2001:db8::1/`）：
            // 它没有方括号，任何切法都是猜，不如不动作。
            Some(_) => return None,
            None => (authority, "", false),
        }
    };
    if host.is_empty() || (!bracketed && host.contains(':')) {
        return None;
    }
    // 主机已经是同一个 IP ⇒ `None`（理由见文档注释）。
    if host
        .parse::<std::net::IpAddr>()
        .map(|cur| cur == *ip)
        .unwrap_or(false)
    {
        return None;
    }
    // IPv6 的 Display 不带方括号，必须手动包——与 `uri_list` 的备份构造同一纪律，
    // 否则拼出 `http://2001:db8::1:8080/…` 这种把端口吞进地址的畸形 URI。
    let host = match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    Some(format!("http://{host}{port_suffix}{query}{base_path}"))
}

/// 把 base 切成 `(authority, base路径)`：scheme（`://`）后第一个 `/` 之前是 authority
/// （`host[:port]`），从 `/` 起是 base 路径（可能为空）。
/// **`uri_list` 的备胎构造与 `authority_of`/`host_header_for`/`resolve_for_base`
/// 共用这同一份切分**——不许写第二份。
fn split_base(base: &str) -> (&str, &str) {
    let after_scheme = match base.find("://") {
        Some(i) => &base[i + 3..],
        None => base,
    };
    match after_scheme.find('/') {
        Some(i) => (&after_scheme[..i], &after_scheme[i..]),
        None => (after_scheme, ""),
    }
}

/// base 的 authority（`host[:port]`）——[`split_base`] 的左半。
/// `host_header_for` 与 `resolve_for_base` 共用这同一份切分。
fn authority_of(base: &str) -> &str {
    split_base(base).0
}

/// `:` 后的串是否是**合法端口号**（非空且全数字）。`uri_list` 的 `port_suffix` 与
/// `host_port_of` 的两处端口识别共用**这一份判据**——写第二份就会漂移。
/// 空 `all()` 为真，不加 `!is_empty()` 会把 `:` 的尾冒号当合法端口。
fn is_port_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// 从 authority 拆出 `(host, port)`。端口只在 `:` 后**非空且全数字**时认
/// （与 `uri_list` 的 `port_suffix` 同一判据）；缺省端口 80（http 默认）。
/// IPv6 方括号形态（`[::1]`、`[::1]:8080`）的 host 保留方括号。
fn host_port_of(authority: &str) -> (&str, u16) {
    // [::1]:8080 或 [::1]
    if authority.starts_with('[') {
        if let Some(end) = authority.find(']') {
            let host = &authority[..=end];
            let rest = &authority[end + 1..];
            let port = match rest.strip_prefix(':') {
                Some(p) if is_port_digits(p) => p.parse().unwrap_or(80),
                _ => 80,
            };
            return (host, port);
        }
    }
    // 裸 IPv4 / 域名：`host` 或 `host:port`
    match authority.rfind(':') {
        Some(i) if is_port_digits(&authority[i + 1..]) => {
            let port = authority[i + 1..].parse().unwrap_or(80);
            (&authority[..i], port)
        }
        _ => (authority, 80),
    }
}

/// host 是否是 IP 字面量（含 `[::1]` 方括号形态）。
fn is_ip_literal(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<std::net::IpAddr>().is_ok()
}

/// lookup 可注入：真解析是 ToSocketAddrs（网络 I/O，单测不碰）；测试给假 lookup。
pub fn resolve_for_base(base_url: &str) -> Vec<std::net::IpAddr> {
    resolve_for_base_with(system_lookup, base_url)
}

/// 规格 §5.3 的多 IP 解析与三道门：
/// 1. base 为空 ⇒ 按 [`DEFAULT_BASE_URL`] 处理（与 [`base`] 同一回退）；
/// 2. scheme 非 `http://` ⇒ `[]`（https 整体关闭——裸 IP 破坏 SNI 与证书校验，R3）；
/// 3. host 是 IP 字面量（含 `[::1]` 形态）⇒ `[]`（造备胎只是自重复）；
/// 4. `lookup(host, port)`：只留 IPv4、去重保序、**截 [`MAX_BACKUP_URIS`] 个**；lookup 失败/空 ⇒ `[]`
///    （优雅降级为域名-only——容错机制自己不许变成新的故障源）。
pub(crate) fn resolve_for_base_with(
    lookup: impl Fn(&str, u16) -> Vec<std::net::IpAddr>,
    base_url: &str,
) -> Vec<std::net::IpAddr> {
    let base = base(base_url); // 规则 1：空 ⇒ DEFAULT_BASE_URL
    if !base.starts_with("http://") {
        return Vec::new(); // 规则 2：scheme 门
    }
    let (host, port) = host_port_of(authority_of(&base));
    if is_ip_literal(host) {
        return Vec::new(); // 规则 3：IP 字面量门
    }
    // 规则 4：只留 IPv4、去重保序、截 MAX_BACKUP_URIS 个
    let mut out: Vec<std::net::IpAddr> = Vec::new();
    for ip in lookup(host, port) {
        if !matches!(ip, std::net::IpAddr::V4(_)) {
            continue;
        }
        if out.contains(&ip) {
            continue;
        }
        out.push(ip);
        if out.len() >= MAX_BACKUP_URIS {
            break;
        }
    }
    out
}

/// 真解析：`ToSocketAddrs`。**单一转用点**，真解析薄壳不测网络。
/// 失败 ⇒ 空表（优雅降级为域名-only）。内部也滤一道 IPv4（`resolve_for_base_with`
/// 再滤一道是给假 lookup 兜底，两道都无害）。
fn system_lookup(host: &str, port: u16) -> Vec<std::net::IpAddr> {
    use std::net::ToSocketAddrs;
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    match (bare, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .filter_map(|a| match a.ip() {
                std::net::IpAddr::V4(v4) => Some(std::net::IpAddr::V4(v4)),
                std::net::IpAddr::V6(_) => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Host 头（规格 §5.3 规则 6）：http ⇒ `Some("Host: {authority}")`，非 http ⇒ `None`
///（整机制关着，不该出现 Host 头）。authority 取法与 `uri_list` 同源（[`authority_of`]）。
pub fn host_header_for(base_url: &str) -> Option<String> {
    let base = base(base_url);
    if !base.starts_with("http://") {
        return None;
    }
    Some(format!("Host: {}", authority_of(&base)))
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
            // 连接阶段单独设上限（2026-10-12）：整体超时已降到 15 秒，ureq 的连接默认值是 30 秒，
            // 不写就是「声明 15 秒、连接阶段实际 30 秒」（与 `rpc.rs` 那两处同一类不一致）。
            // 现场的 10060 正是连接阶段超时，所以这里取得比整体更短：连不上就早点换下一次。
            // 2026-10-27：整体时限改成**单次读时限**。整体时限会把"慢但一直在下"的清单
            // 拦腰砍断——十万文件的清单几十 MB，慢链路上 15 秒读不完是常态，而它并没有出错。
            // 现在只要还在来数据就继续读；真卡住（一个字节都不来）仍在 `FETCH_TIMEOUT` 后放弃。
            agent: ureq::AgentBuilder::new()
                .timeout_read(FETCH_TIMEOUT)
                .timeout_connect(FETCH_CONNECT_TIMEOUT)
                .build(),
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

/// 一次交付页往返的**分类** —— 给诊断日志用。
///
/// 🔴 **它存在的全部理由是隐私**：`url` 里含交付码，而 `ureq::Error` 的 `Display`
///    会把 URL 打出来 ⇒ 这一路**必须**只落一个结构化取值，不许落 `e.to_string()`。
///    ⚠️ 这与 A6 那条"接受 aria2 原文的残留"**不同**：那边是**第三方文案我们识别不了**，
///    这边是**我们自己拼的串、我们自己知道里面有交付码** —— 能按构造避开就必须避开。
///
/// ⚠️ **这一格的取值是一份契约，不是随手写的**：`ok` / `status_<码>` / `transport_<Kind>`
///    由 `the_delivery_fetch_outcome_vocabulary_is_a_contract` 逐条钉住
///    （那条用例是把真 `ureq` 打到本地桩上跑的，不是拿一个手工造的 `Error` 顶替）。
///    ⚠️ **它钉住的到此为止**：`delivery_fetch` 那两个 `log_verbose` **调用点本身**
///    仍然没有判据 —— 把 `get`/`head` 里那两段删掉，全套判据照旧全绿。
///    那一半只由真机验收那条（详细档下真的出现 `event=delivery_fetch`）守着，如实记账。
fn outcome_category<T>(outcome: &Result<T, ureq::Error>) -> String {
    match outcome {
        Ok(_) => "ok".to_string(),
        Err(ureq::Error::Status(code, _)) => format!("status_{code}"),
        Err(ureq::Error::Transport(t)) => format!("transport_{:?}", t.kind()),
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let started = std::time::Instant::now();
        let outcome = self.agent.get(url).call();
        crate::diagnostics::log_verbose(
            "delivery_fetch",
            &[
                ("stage", "get".to_string()),
                ("ms", started.elapsed().as_millis().to_string()),
                ("outcome", outcome_category(&outcome)),
            ],
        );
        match outcome {
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
            // ⚠️ 传输层**不造句**（它拿不到语言）：404 走与其它 4xx 同一个 helper，
            //    保留状态码（`http_status` 的判据要靠它），句子由 `fetch_with` 按语言拼。
            Err(ureq::Error::Status(404, _)) => Err(http_status_error(404, "")),
            Err(ureq::Error::Status(code, _)) if code >= 500 => {
                Err(http_status_error(code, &format!("服务端错误 {code}")))
            }
            Err(ureq::Error::Status(code, _)) => Err(http_status_error(code, "")),
            Err(ureq::Error::Transport(e)) => Err(network_error(e)),
        }
    }

    fn head_crc64(&self, url: &str) -> Option<String> {
        let started = std::time::Instant::now();
        let outcome = self.agent.head(url).call();
        crate::diagnostics::log_verbose(
            "delivery_fetch",
            &[
                ("stage", "head".to_string()),
                ("ms", started.elapsed().as_millis().to_string()),
                ("outcome", outcome_category(&outcome)),
            ],
        );
        let resp = outcome.ok()?;
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
pub fn fetch<T: Transport>(
    t: &T,
    base_url: &str,
    code: &str,
    lang: crate::i18n::Lang,
) -> Result<Manifest, String> {
    fetch_with(t, base_url, code, lang, FETCH_BASE_DELAY, std::thread::sleep)
}

/// [`fetch`] 的本体：退避基数与睡眠函数由调用方给，测试借此跑真实循环而不真睡。
fn fetch_with<T: Transport>(
    t: &T,
    base_url: &str,
    code: &str,
    lang: crate::i18n::Lang,
    base_delay: Duration,
    mut sleep: impl FnMut(Duration),
) -> Result<Manifest, String> {
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
            sleep(base_delay * (1u32 << (attempt - 1)));
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
                // 404：一句按客户语言拼好的话（句子只在这一处）
                if http_status(&e) == Some(404) {
                    return Err(crate::i18n::text::tf(
                        lang,
                        crate::i18n::text::MsgKey::DeliveryNotFound,
                        &[("url", &url)],
                    ));
                }
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

    // -------------------------------------------------------------------
    // `uri_list`（规格 §5.1）：域名 URI 打头 + IP 备胎。
    // 域名是 CDN 分发域名（用户约束：不换连接），IP 只做备胎；
    // `--uri-selector=inorder` 保证先用第一个、坏了才轮到下一个。
    // -------------------------------------------------------------------

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().expect("测试里的 IP 字面量必须合法")
    }

    /// 域名 URI 必须排第一，IP 备胎按传入顺序跟在后面。
    ///
    /// 转义判据与 `file_url_escapes_like_server` 同源（`file_url_on` 抽出来后那条用例
    /// 必须仍然绿）：备胎 URI 与域名 URI **同一份转义**，任何一份另写都会与清单对不上。
    #[test]
    fn uri_list_puts_the_domain_first_and_ip_backups_after() {
        let m = manifest("23999-JF", "http://download.benagen.com");
        let got = uri_list(&m, "a/b c.txt", &[ip("1.2.3.4"), ip("5.6.7.8")]);
        assert_eq!(
            got,
            vec![
                "http://download.benagen.com/23999-JF/a/b%20c.txt".to_string(),
                "http://1.2.3.4/23999-JF/a/b%20c.txt".to_string(),
                "http://5.6.7.8/23999-JF/a/b%20c.txt".to_string(),
            ],
            "域名必须打头、IP 备胎按序跟上，且三者同一份转义"
        );
    }

    /// `ips` 为空 ⇒ 只有域名 URI（本任务的生产形态：四个调用点都传空 `ips`）。
    #[test]
    fn uri_list_is_domain_only_when_ips_empty() {
        let m = manifest("23999-JF", "http://download.benagen.com");
        let got = uri_list(&m, "a.txt", &[]);
        assert_eq!(got.len(), 1, "ips 为空时应当只有域名 URI，实际 {got:?}");
        assert_eq!(got[0], "http://download.benagen.com/23999-JF/a.txt");
    }

    /// 备胎**上限 16 个**：20 个 IP ⇒ 域名 + 16 = 17 条。
    /// 16 与 `splits` 上限对齐（`settings.rs:111`）：17 条候选够 16 个分片各占一条腿。
    #[test]
    fn uri_list_caps_backups_at_sixteen() {
        let m = manifest("C", "http://download.benagen.com");
        let ips: Vec<std::net::IpAddr> = (1..=20).map(|i| ip(&format!("10.0.0.{i}"))).collect();
        // 直接把 20 个原始 IP 交给 uri_list：截断必须由 uri_list 自己做。
        // 若先过 resolve_for_base_with（它也截 16 条），uri_list 里的 take 删掉测试照样绿。
        let got = uri_list(&m, "f.txt", &ips);
        assert_eq!(
            got.len(),
            17,
            "备胎上限是 16 条；20 个 IP 应得 1+16=17 条，实际 {got:?}"
        );
        // 裁掉的是**尾部**，不是头部——排在前面的备胎优先级更高
        assert_eq!(got[1], "http://10.0.0.1/C/f.txt");
        assert_eq!(got[16], "http://10.0.0.16/C/f.txt");
        assert!(
            !got.iter().any(|u| u.contains("10.0.0.17")),
            "第 17 个 IP 起必须被截断，实际 {got:?}"
        );
    }

    /// authority 有端口则备胎保留端口（IP 换掉 host，端口不动）。
    #[test]
    fn uri_list_keeps_the_port_in_backup_authority() {
        let m = manifest("C", "http://mirror.local:8080");
        let got = uri_list(&m, "f.txt", &[ip("1.2.3.4")]);
        assert_eq!(
            got,
            vec![
                "http://mirror.local:8080/C/f.txt".to_string(),
                "http://1.2.3.4:8080/C/f.txt".to_string(),
            ],
            "备胎必须保留 base 的端口"
        );
    }

    /// base 非 `http://` ⇒ 只回域名 URI（规格 §5.1 只给 http 场景配 IP 备胎；
    /// https 的 base 在上一层就被掐掉——`resolve_for_base` 返回空——这里再兜一道）。
    #[test]
    fn uri_list_is_domain_only_when_base_is_not_http() {
        let m = manifest("C", "https://download.benagen.com");
        let got = uri_list(&m, "f.txt", &[ip("1.2.3.4")]);
        assert_eq!(
            got,
            vec!["https://download.benagen.com/C/f.txt".to_string()],
            "https 的 base 不该配 IP 备胎（备胎是 http 直连，与 https 的证书校验不兼容）"
        );
    }

    /// base 带路径带端口 ⇒ 备胎保留端口和 base 路径（审查缺陷 2：`rfind(':')` 粗切把
    /// `8080/subdir` 当端口尾巴 ⇒ 全数字判不过 ⇒ 端口和路径全丢）。
    #[test]
    fn uri_list_keeps_base_path_and_port_in_backup() {
        let m = manifest("CODE", "http://mirror.local:8080/subdir");
        let got = uri_list(&m, "f.txt", &[ip("1.2.3.4")]);
        assert_eq!(
            got,
            vec![
                "http://mirror.local:8080/subdir/CODE/f.txt".to_string(),
                "http://1.2.3.4:8080/subdir/CODE/f.txt".to_string(),
            ],
            "备胎必须保留 base 的端口和路径"
        );
    }

    /// base 带路径无端口 ⇒ 备胎保留 base 路径（路径原样，不 Join 不规范化）。
    #[test]
    fn uri_list_keeps_base_path_in_backup() {
        let m = manifest("CODE", "http://mirror.local/subdir");
        let got = uri_list(&m, "f.txt", &[ip("1.2.3.4")]);
        assert_eq!(
            got,
            vec![
                "http://mirror.local/subdir/CODE/f.txt".to_string(),
                "http://1.2.3.4/subdir/CODE/f.txt".to_string(),
            ],
            "备胎必须保留 base 的路径"
        );
    }

    /// IPv6 地址进备胎 authority 时必须加方括号（审查缺陷 1：`IpAddr::V6` 的
    /// Display 不带方括号，拼出 `http://2001:db8::1/…` 是畸形 URI）。
    #[test]
    fn uri_list_brackets_ipv6_in_backup_authority() {
        let m = manifest("C", "http://download.benagen.com");
        let v6: std::net::IpAddr = "2001:db8::1".parse().expect("合法 V6");
        let got = uri_list(&m, "f.txt", &[v6]);
        assert_eq!(
            got,
            vec![
                "http://download.benagen.com/C/f.txt".to_string(),
                "http://[2001:db8::1]/C/f.txt".to_string(),
            ],
            "IPv6 备胎 authority 必须是方括号形态"
        );
    }

    /// `http://host:` 边角（空端口）⇒ 备胎不带尾冒号。
    /// 容忍而非拒绝：空端口等价于「未指定端口」，备胎同样不指定。
    #[test]
    fn uri_list_drops_empty_port_in_backup() {
        let m = manifest("C", "http://mirror.local:");
        let got = uri_list(&m, "f.txt", &[ip("1.2.3.4")]);
        assert_eq!(
            got,
            vec![
                "http://mirror.local:/C/f.txt".to_string(),
                "http://1.2.3.4/C/f.txt".to_string(),
            ],
            "空端口不得带进备胎（尾冒号会让 URI 非法）"
        );
    }

    // -------------------------------------------------------------------
    // `uri_with_host`：换腿线程造 IP 腿用的纯拼装。必须与 `uri_list` 的备胎
    // **同一形状**，否则同一份文件会被 aria2 当成两条不同的 URI。
    // -------------------------------------------------------------------

    /// IPv4：只换主机，端口、路径、查询串逐字保留。
    #[test]
    fn uri_with_host_keeps_port_path_and_query_for_ipv4() {
        let got = uri_with_host("http://download.benagen.com:8080/AbC/f.txt?k=1", &ip("203.0.113.9"));
        assert_eq!(
            got,
            Some("http://203.0.113.9:8080/AbC/f.txt?k=1".to_string()),
            "换 IPv4 必须保留端口、路径与查询串"
        );
    }

    /// IPv6 必须套方括号（与 `uri_list` 的备胎同一纪律），端口留在方括号外。
    #[test]
    fn uri_with_host_brackets_ipv6() {
        let v6: std::net::IpAddr = "2001:db8::1".parse().expect("合法 V6");
        assert_eq!(
            uri_with_host("http://download.benagen.com/C/f.txt", &v6),
            Some("http://[2001:db8::1]/C/f.txt".to_string()),
            "IPv6 主机必须包成方括号形态"
        );
        assert_eq!(
            uri_with_host("http://download.benagen.com:8080/C/f.txt", &v6),
            Some("http://[2001:db8::1]:8080/C/f.txt".to_string()),
            "IPv6 的端口必须留在方括号外"
        );
        // 反向：原来是方括号 IPv6 主机，换成一个不同的 IPv4 —— 方括号里的冒号不是端口，
        // 不能被「host 不许含 `:`」的畸形门误伤。
        assert_eq!(
            uri_with_host("http://[2001:db8::2]:8080/C/f.txt", &ip("203.0.113.9")),
            Some("http://203.0.113.9:8080/C/f.txt".to_string()),
            "方括号 IPv6 主机换成 IPv4 时端口与路径都要保留"
        );
    }

    /// 非 http / 畸形 authority ⇒ `None`（认不出就不动，调用方只是"这一拍不动作"）。
    #[test]
    fn uri_with_host_rejects_non_http_and_malformed() {
        let v4 = ip("203.0.113.9");
        for input in [
            "https://download.benagen.com/C/f.txt", // https：换主机名会打坏 SNI/证书
            "not a uri",                            // 没有 scheme
            "http:///C/f.txt",                      // 没有 authority
            "http://",                              // 空串
            "http://host:/C/f.txt",                 // 尾冒号：端口不合法
            "http://[::1/C/f.txt",                  // 方括号不闭合
            "http://user:pass@host/C/f.txt",        // 带 userinfo：认不出主机边界
            "http://2001:db8::1/C/f.txt",           // 裸 IPv6：没有方括号，切法全靠猜
        ] {
            assert_eq!(uri_with_host(input, &v4), None, "畸形/非 http 串应当返回 None");
        }
    }

    /// 主机已经是同一个 IP ⇒ `None`（换出来与输入逐字节相同，别制造重复 URI）。
    #[test]
    fn uri_with_host_rejects_host_already_that_ip() {
        assert_eq!(
            uri_with_host("http://203.0.113.9:8080/AbC/f.txt", &ip("203.0.113.9")),
            None,
            "主机已是同一个 IPv4 ⇒ 无需动作"
        );
        let v6: std::net::IpAddr = "2001:db8::1".parse().expect("合法 V6");
        assert_eq!(
            uri_with_host("http://[2001:db8::1]/C/f.txt", &v6),
            None,
            "主机已是同一个 IPv6（方括号形态）⇒ 无需动作"
        );
    }

    // -------------------------------------------------------------------
    // `resolve_for_base` / `host_header_for`（规格 §5.3）：scheme 门、IP 字面量门、
    // lookup 注入。lookup 是**唯一**的网络 I/O 点，单测给假 lookup，不碰网络。
    // -------------------------------------------------------------------

    /// lookup 可注入：真解析是 ToSocketAddrs（网络 I/O，单测不碰）；测试给假 lookup。
    fn fake_lookup(ips: Vec<std::net::IpAddr>) -> impl Fn(&str, u16) -> Vec<std::net::IpAddr> {
        move |_, _| ips.clone()
    }

    /// base 为空 ⇒ 按 DEFAULT_BASE_URL 处理（与 `base()` 同一回退）——
    /// 但 scheme 门先于 lookup，空 base 回退后是 http ⇒ 不会因为空 base 而误关。
    #[test]
    fn resolve_for_base_falls_back_to_default_for_empty_base() {
        let got = resolve_for_base_with(fake_lookup(vec![ip("1.2.3.4")]), "");
        // DEFAULT_BASE_URL 是 http + 域名 ⇒ 应当拿到 lookup 的结果
        assert_eq!(got, vec![ip("1.2.3.4")], "空 base 应回退到 DEFAULT_BASE_URL 再走 lookup");
    }

    /// scheme 门：https ⇒ []（哪怕 lookup 会给出 IP）。
    /// 判别力：去掉 scheme 门 ⇒ https 批次会带上 IP 备胎 ⇒ 裸 IP 破坏 SNI 与证书校验。
    #[test]
    fn resolve_for_base_is_empty_for_https() {
        let got = resolve_for_base_with(fake_lookup(vec![ip("1.2.3.4")]), "https://download.benagen.com");
        assert_eq!(got, Vec::<std::net::IpAddr>::new(), "https 必须整体关闭多 IP 机制（R3）");
    }

    /// IP 字面量门：host 是 IP ⇒ []（造备胎只是自重复）。
    /// 判别力：去掉 IP 字面量门 ⇒ 自身 IP 被当备胎 ⇒ URI 列表里出现重复主机。
    #[test]
    fn resolve_for_base_is_empty_when_host_is_an_ip_literal() {
        let got = resolve_for_base_with(fake_lookup(vec![ip("1.2.3.4")]), "http://1.2.3.4/path/file");
        assert_eq!(got, Vec::<std::net::IpAddr>::new(), "IP 字面量主机不该造备胎（自重复）");
    }

    /// IPv6 方括号形态也是 IP 字面量。
    #[test]
    fn resolve_for_base_is_empty_for_bracketed_ipv6_literal() {
        let got = resolve_for_base_with(fake_lookup(vec![ip("1.2.3.4")]), "http://[::1]/p/f");
        assert_eq!(got, Vec::<std::net::IpAddr>::new(), "[::1] 方括号形态也是 IP 字面量，不该造备胎");
    }

    /// lookup 失败/空 ⇒ []（优雅降级为域名-only——容错机制自己不许变成新的故障源）。
    #[test]
    fn resolve_for_base_falls_back_to_domain_only_when_lookup_fails() {
        let got = resolve_for_base_with(fake_lookup(vec![]), "http://download.benagen.com");
        assert_eq!(got, Vec::<std::net::IpAddr>::new(), "lookup 空 ⇒ 优雅降级域名-only");
    }

    /// 只留 IPv4、去重保序、截 16 个。
    /// 判别力：去掉 IPv4 过滤 ⇒ V6 进备胎 ⇒ aria2 恒 --disable-ipv6 连不上；
    ///         去掉截断 ⇒ 20 个 v4 全进 ⇒ 超过 16 条备胎上限，把 aria2 的 URI 表撑大。
    #[test]
    fn resolve_for_base_takes_ipv4_only_and_caps_at_sixteen() {
        let v6: std::net::IpAddr = "2001:db8::1".parse().expect("合法 V6");
        let mut mixed = vec![v6, ip("10.0.0.1")]; // V6 与重复项都必须被滤掉，不占名额
        mixed.extend((1..=20).map(|i| ip(&format!("10.0.0.{i}")))); // 1 是重复
        let got = resolve_for_base_with(fake_lookup(mixed), "http://download.benagen.com");
        let want: Vec<std::net::IpAddr> =
            (1..=16).map(|i| ip(&format!("10.0.0.{i}"))).collect();
        assert_eq!(
            got, want,
            "只留 IPv4、去重保序、截 16 个——16 是 split 上限，腿数必须够"
        );
    }

    /// host_header_for：http ⇒ Some("Host: {authority}")，非 http ⇒ None。
    /// authority = scheme 后第一个 `/` 之前（与 `uri_list` 的切分同一取法）。
    #[test]
    fn host_header_for_is_the_authority_of_the_base() {
        assert_eq!(
            host_header_for("http://download.benagen.com"),
            Some("Host: download.benagen.com".to_string()),
            "http 域名 base 的 Host 头就是 authority"
        );
        assert_eq!(
            host_header_for("http://mirror.local:8080"),
            Some("Host: mirror.local:8080".to_string()),
            "authority 必须保留端口"
        );
        assert_eq!(
            host_header_for("https://download.benagen.com"),
            None,
            "https 整机制关着，不该出现 Host 头"
        );
    }

    /// authority_of：scheme 后第一个 `/` 之前，与 uri_list 的切分同一取法。
    #[test]
    fn authority_of_splits_at_the_first_slash_after_scheme() {
        assert_eq!(authority_of("http://download.benagen.com"), "download.benagen.com");
        assert_eq!(authority_of("http://mirror.local:8080/subdir"), "mirror.local:8080");
        assert_eq!(authority_of("http://mirror.local/subdir/a"), "mirror.local");
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

    /// 调参后的最坏耗时必须仍在原先记账的 91.5 秒之内——`load_delivery` 的「三段顺序执行」
    /// 预算（`client.rs` 的 `LONG_CALL_TIMEOUT` 注释）是按这个数算的。
    /// 最坏耗时 = 循环实际发出的 get 次数 × 单次时限 + 循环实际睡过的退避之和。跑的是真实的
    /// `fetch_with` 循环（退避用生产值，睡眠只记账不真睡），所以循环次数或退避形状一改，
    /// 这里的次数与序列就跟着变，记账即超 ⇒ 红。
    #[test]
    fn fetch_worst_case_stays_inside_the_booked_91_5_seconds() {
        let t = Stub::always(network_error("connection refused"));
        let mut slept: Vec<Duration> = Vec::new();
        let r = fetch_with(&t, STUB_BASE, "X", crate::i18n::Lang::Zh, FETCH_BASE_DELAY_PROD, |d| slept.push(d));
        assert!(r.is_err(), "持续失败时应当报错");
        let worst = t.calls() as f64 * FETCH_TIMEOUT.as_secs_f64()
            + slept.iter().map(Duration::as_secs_f64).sum::<f64>();
        assert!(
            worst <= 91.5,
            "最坏 {worst} 秒，超出了 91.5 秒的记账（get {} 次，睡眠 {slept:?}）",
            t.calls()
        );
        assert_eq!(
            slept,
            [1, 2, 4, 8].map(Duration::from_secs).to_vec(),
            "退避序列应为 1/2/4/8 秒"
        );
        assert!(FETCH_CONNECT_TIMEOUT < FETCH_TIMEOUT, "连接阶段应当比整体更早放弃");
    }

    #[test]
    fn fetch_success() {
        let t = Stub::new(vec![Ok(r#"{"code":"AbC123","files":[]}"#)]);
        let m = match fetch(&t, STUB_BASE, "AbC123", crate::i18n::Lang::Zh) {
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
        if let Err(e) = fetch(&t, STUB_BASE, "X", crate::i18n::Lang::Zh) {
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
            let err = match fetch(&t, STUB_BASE, "X", crate::i18n::Lang::Zh) {
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

    /// 判据（T2 闭集）：**同一个 404**，壳说中文见中文、说英文见英文，
    /// 两句都点名 URL（客户要照着它核对交付码）。句子只在 `fetch` 里造一次。
    #[test]
    fn a_missing_manifest_says_the_same_thing_in_both_languages() {
        let not_found = || http_status_error(404, "");
        let t = Stub::always(not_found());
        let zh = fetch(&t, STUB_BASE, "AbCdEfGhIjKlMnOpQrSt", crate::i18n::Lang::Zh)
            .expect_err("404 必须是错");
        assert!(zh.contains("清单不存在") && zh.contains("AbCdEfGhIjKlMnOpQrSt"), "{zh}");

        let t = Stub::always(not_found());
        let en = fetch(&t, STUB_BASE, "AbCdEfGhIjKlMnOpQrSt", crate::i18n::Lang::En)
            .expect_err("404 必须是错");
        assert!(
            !en.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "英文界面里蹦出了中文: {en}"
        );
        assert!(en.contains("404") && en.contains("AbCdEfGhIjKlMnOpQrSt"), "{en}");
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
        let m = match fetch(&t, STUB_BASE, "X", crate::i18n::Lang::Zh) {
            Ok(m) => m,
            Err(e) => panic!("网络错误应当被重试并在恢复后成功: {e}"),
        };
        assert_eq!(m.code, "X", "code 不对");
        assert_eq!(t.calls(), 3, "get 次数（2 次网络错误 + 1 次成功）");
        assert_eq!(t.ok_calls(), 1, "桩被成功请求次数");

        // 网络错误持续存在时，重试耗尽后必须报错——且必须真的重试过
        let always_fail = Stub::always(network_error("connection refused"));
        assert!(
            fetch(&always_fail, STUB_BASE, "X", crate::i18n::Lang::Zh).is_err(),
            "网络错误且重试耗尽后应当报错"
        );
        assert_eq!(always_fail.calls(), FETCH_RETRIES, "重试耗尽时 get 次数");
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
            let err = match fetch(&t, b, "AbCdEfGhIjKlMnOpQrSt", crate::i18n::Lang::Zh) {
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
        let err = match fetch(&UreqTransport::new(), "http://127.0.0.1:1/", "AbCdEfGhIjKlMnOpQrSt", crate::i18n::Lang::Zh) {
            Err(e) => e,
            Ok(_) => String::new(),
        };
        assert!(
            !err.contains("base_url"),
            "正常基址被 validate_base_url 误拒了: {err}"
        );
    }

    // ------------------------------------------------------------------
    // `delivery_fetch` 那一行的**分类词表**（规格 §2.3 B）
    //
    // ⚠️ 这一条**只钉住词表**，别把它读成"这一路有证据了"：
    //    `delivery.rs` 里那两个 `log_verbose` **调用点本身**仍然没有判据 ——
    //    把 `get`/`head` 里那两段整段删掉，**全套判据照旧全绿**。那一半只有真机验收
    //    能回答（详细档下真的跑一次拉清单 ⇒ `event=delivery_fetch` 的条数 > 0）。
    // ------------------------------------------------------------------

    /// 起一个**只回一行 HTTP 响应**的桩服务，返回它的基址。
    ///
    /// 与 `core/tests/e2e.rs` 的桩同一个形状（裸 `TcpListener`，`wiremock` 不在依赖表里）。
    fn a_one_shot_http_stub(response: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let addr = listener.local_addr().expect("取端口失败");
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}/AbCdEfGhIjKlMnOpQrSt/manifest.json")
    }

    /// 🔴 **分类的取值是一份契约**：`ok` / `status_<码>` / `transport_<Kind>`。
    ///
    /// 为什么值得一条用例（终审重要 ③）：这个函数是**整个"不记 url"决定**在实现上的
    /// 全部落点，而在此之前**没有任何一条测试引用过 `delivery_fetch`**（全仓只有
    /// `delivery_fetch_failed`，那是另一个东西）⇒ 删掉那两个 `log_verbose`、或者把
    /// 分类改成 `format!("{outcome:?}")`（那就把 URL 带出去了），**没有任何东西会红**。
    ///
    /// ⚠️ **它钉住的是词表，不是调用点**（见上面那一段记账）。
    /// ⚠️ 桩走的是**真 ureq**（不是手工造的 `Error`，那个造不出来）：
    ///    本地环回、一次性连接，与 `core/tests/e2e.rs` 同一手法。
    #[test]
    fn the_delivery_fetch_outcome_vocabulary_is_a_contract() {
        let t = UreqTransport::new();

        // ① 200 ⇒ `ok`
        let url = a_one_shot_http_stub("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        let ok = t.agent.get(&url).call();
        assert_eq!(outcome_category(&ok), "ok", "成功那一档的词是 `ok`");

        // ② 404 ⇒ `status_404`（交付码打错那一档，也是这一路最常出现的失败）
        let url = a_one_shot_http_stub("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let not_found = t.agent.get(&url).call();
        assert_eq!(
            outcome_category(&not_found),
            "status_404",
            "状态码要出现在**词**里（`status_<码>`），而不是从文案里截"
        );

        // ③ 连不上 ⇒ `transport_<Kind>`：`127.0.0.1:1` 立刻 ECONNREFUSED
        //    （同文件上面那条"正常基址不被误拒"的用例用的就是它）。
        let refused = t.agent.get("http://127.0.0.1:1/AbCdEfGhIjKlMnOpQrSt/manifest.json").call();
        let category = outcome_category(&refused);
        assert!(
            category.starts_with("transport_") && category.len() > "transport_".len(),
            "传输层失败要带上**种类**（`transport_<Kind>`），实际 {category:?}"
        );

        // 🔴 **而这三档一个都不许带上 URL** —— 那才是这个函数存在的理由：
        //    URL 里含交付码，而 `ureq::Error` 的 `Display` 会把它原样打出来。
        for (what, category) in [
            ("status_404", outcome_category(&not_found)),
            ("transport", category),
        ] {
            assert!(
                !category.contains("AbCdEfGhIjKlMnOpQrSt"),
                "{what} 那一档把交付码带出去了：{category}"
            );
            assert!(!category.contains("http"), "{what} 那一档把 URL 带出去了：{category}");
        }
    }
}
