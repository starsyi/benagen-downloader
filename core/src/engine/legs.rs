//! 腿表判据 —— 「每条腿此刻多快」「多慢算该摘」（规格 2026-10-18 §2 的 D6/D7）。
//!
//! 现场实测（见规格 §1 的 F1–F7）：aria2 的 `split=N` 实际是**最多 N 条腿、每条腿 1 条连接**，
//! 总量 ≈ 各腿速率之和；不同边缘 IP 之间的速率差 **5–8 倍**，而**引擎自己不按快慢挑腿、
//! 下载中也不换腿**。所以「把远低于中位数的 IP 腿换成更好的候选」只能我们自己做。
//!
//! 本文件是**纯判据**：从 `aria2.getServers` 的 `result` 里读出每条腿的 URI 与速率、
//! 算中位数、算摘腿阈值、认出 IP 字面量腿。**线程与 RPC 不在这里**：
//! 定时拍线程 `spawn_leg_trim` 在 `core/src/kernel.rs`，跨拍判决（纯函数 `plan`）在
//! `core/src/legtrim.rs`，`getServers`/`changeUri` 的薄封装在 `core/src/engine/rpc.rs`
//! 与 `core/src/engine/daemon.rs`。
//! 这个分工与 `core/src/autoretry.rs` 同形——那边只判「哪些错该重试、等多久」，
//! 重试线程与 `tellStatus` 调用在 `core/src/kernel.rs`；**别把两者混在一起**，
//! 判据能单测，线程和 RPC 只能靠联调。
//!
//! 判据对坏数据的态度是**永远不 panic、只让这一轮不摘腿**（返回空表/0）：
//! 它的输入是网络来的结构，一次字段缺失或形状变化不该升级成「下载失败」，
//! 也不该让后台线程死掉（摘腿是优化，不是正确性——规格 §5 的 I3）。

use serde_json::Value;

/// 摘腿阈值的**下限** 131072 字节/秒（= 128 KiB/s，即规格 §3.1 伪代码里的 `128 * 1024`）。
///
/// 规格 §2 的 D6 表格写的是「128 KB/s」，§3.1 的伪代码写的是「128 KiB/s」；这里按 §3.1
/// 取 `128 * 1024`。**不要按 KB=1000 把它改成 128000**：那样下限比规格伪代码低 2.4%，
/// 且没有任何实测依据支持这个偏移，只会让后来者对着规格对不上号。
///
/// 为什么必须有下限而不是「median/3 就完了」：所有腿一起慢时（服务端限速、冷启动还没跑起来）
/// 中位数本身很小，`median/3` 会让阈值贴到 0 附近，于是**一半以上的腿都"低于阈值"**
/// ——此时换掉它们也没有更好的可换，只是白烧 RPC、白抖连接（规格 D6 的裁决）。
/// 下限把这种「整批都慢」的情形整轮跳过。
const MIN_TRIM_BYTES_PER_SEC: u64 = 131_072;

/// 一条腿：一个下发给 aria2 的 URI，以及它此刻的下载速率（字节/秒）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leg {
    pub uri: String,
    pub speed: u64,
}

/// 解析 `aria2.getServers` 的 `result` 字段，取本次任务的全部腿。
///
/// 线上的形状是**一层数组**，元素是**文件条目**（真实 aria2 1.37.0 抓包，见
/// `parse_servers_parses_real_capture` 的测试夹具）：
///
/// ```json
/// [{"index":"1","servers":[
///   {"currentUri":"http://127.0.0.1:8733/big.bin","downloadSpeed":"4962498","uri":"http://127.0.0.1:8733/big.bin"},
///   {"currentUri":"http://localhost:8733/big.bin","downloadSpeed":"4853929","uri":"http://localhost:8733/big.bin"}
/// ]}]
/// ```
///
/// 注意 `index` 是**文件条目序号**（同一次响应里第几个文件），**不是**「第几个下载」。
/// 别拿它去索引外层数组——那是把一层响应当两层读，对真实响应会恒返回空表，
/// 整个换腿特性随之静默失效（本函数第一版就栽在这里，测试夹具用了两层数组所以是假绿）。
///
/// **本产品只用 `addUri` 下单个文件，`result` 恒一条 file 条目**，所以这里直接遍历
/// 全部条目、把它们所有腿拼起来即可。若将来支持多文件（torrent / metalink），
/// 每个条目会各自列出**同一组 peer**，同一批 URI 会被重复计入；届时必须先按 uri 去重、
/// 或只取第一条条目。
///
/// 🔴 **按 URI 聚合是承重的：aria2 的 `servers` 是「按连接」列的，不是「按 URI」。**
/// 实测（内嵌 aria2 1.37.0，`split=4` + `max-connection-per-server=4`、2 条 URI）：
/// `getServers` 回 **4 条 server 条目、同一 URI 出现两次**，每条的速度是**单条连接**的速度，
/// 四条之和约等于 `tellStatus` 的总速。于是同一 URI 的多条连接必须**速度相加**——
/// 那才是这条腿的真实速率。不聚合的后果不是"多列一行"：
///  * `legtrim::plan` 的连续 2 拍去抖会对同一个 key 一拍加两次，两拍退化成 1 拍；
///  * 同一 URI 一快一慢时，判决取决于腿表顺序，而不是这条腿到底快不快。
///
/// 聚合规则：**同 URI 速度相加（饱和加法，网络数据不 panic），位置取首次出现，其余字段
/// 取第一次见到的**。`find` 是线性的，但腿数上限是 `split`（个位数），不值得为它上 map
/// 并放弃"首次出现顺序"这个可断言的语义。
///
/// 两种字段形态都必须吃下：`downloadSpeed` 在线上是**字符串**（aria2 的整数一律以字符串返回），
/// 但在夹具/其他传输里可能是数字；缺键视为 0（还没跑起来，不是坏数据）。
///
/// 形状坏了就返回**空表**（不是 panic、也不是"跳过这一条"）：本判据的调用方是后台线程，
/// 空表只会让这一轮不摘腿（I3），而 panic 会把整条裁剪链带走。
/// 判定为坏形状的有：`result` 不是数组、某个文件条目缺 `servers` 或 `servers` 不是数组。
/// 空 `result` 数组同样得到空表（没有条目＝这一轮没腿可判，与坏形状的处置一致）。
/// 单条 `servers` 元素缺 `uri` 时只跳过那一条（没有 URI 就无法 `changeUri`，
/// 但其余腿的速率仍然可信）。
pub fn parse_servers(result: &Value) -> Vec<Leg> {
    let Some(entries) = result.as_array() else {
        return Vec::new();
    };
    let mut legs: Vec<Leg> = Vec::new();
    for entry in entries {
        let Some(servers) = entry.get("servers").and_then(Value::as_array) else {
            return Vec::new();
        };
        for server in servers {
            let Some(uri) = server.get("uri").and_then(Value::as_str) else {
                continue;
            };
            let speed = speed_of(server.get("downloadSpeed"));
            match legs.iter_mut().find(|leg| leg.uri == uri) {
                // 同一 URI 的另一条连接：速度**相加**才是这条腿的真实速率（见函数 doc）。
                // 位置不动：首次出现的顺序就是腿表顺序，判决不该被连接数的多寡打乱。
                // `saturating_add` 而不是 `+`：速率来自网络，宁可饱和也不许 panic。
                Some(leg) => leg.speed = leg.speed.saturating_add(speed),
                None => legs.push(Leg {
                    uri: uri.to_string(),
                    speed,
                }),
            }
        }
    }
    legs
}

/// `downloadSpeed` 的三种形态 → `u64`：字符串（线上）、数字（夹具）、缺失/认不出（0）。
///
/// 认不出的字符串按 0 处理而不是整表失败：速率读不出来只影响这一条腿的判定，
/// 而把它当坏数据会让**整张腿表**失效、这一轮全不摘。
fn speed_of(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// 中位数：空表 → 0；偶数个取**较小**的那个（保证「半数以上」的语义）。
///
/// 为什么是**中位数**而不是平均数：一条 8 倍快的离群腿就能把平均数抬到正常腿之上。
/// `[1, 1, 8]` MB/s 的平均是 3.33 MB/s，`mean/3` 约 1.11 MB/s —— 比两条正常腿都快，
/// 于是**多数正常腿**会被判成「低于阈值」而整批误摘（D6 的相反面）。
/// 中位数对离群值不敏感：同一组的中位数仍是 1 MB/s，`median/3` 约 0.33 MB/s，
/// 只摘真正慢的那条。
///
/// 偶数取较小者也不是随手的选择：阈值是「低于中位数的腿该摘」，若偶数取较大者，
/// `[1,2,3,4]` 的阈值基准从 2 变 3，本该被摘的 2 反而留下——「远低于中位数」的门槛整体上移。
/// 空表返回 0 而不是 panic：调用方拿到 0 会经 [`trim_threshold`] 落回 128 KiB/s 的下限，
/// 正好是「这一轮不摘」。
pub fn median(speeds: &[u64]) -> u64 {
    if speeds.is_empty() {
        return 0;
    }
    let mut v = speeds.to_vec();
    v.sort_unstable();
    // 奇数：`len/2` 就是中间；偶数：`(len-1)/2 == len/2 - 1` 是较小的中间值。
    // 一个式子同时覆盖两种情况，避免两套分支各写错一次。
    v[(v.len() - 1) / 2]
}

/// 摘腿阈值：`max(median_speed / 3, 128 KiB/s)`。
///
/// `median_speed = 0` 时返回的是 **131072 而不是 0**：阈值为 0 时任何 0 速率的腿都「低于阈值」，
/// 包括刚起、还没跑起来的腿（它们的 `downloadSpeed` 天然是 0）——首拍就整批误杀。
pub fn trim_threshold(median_speed: u64) -> u64 {
    (median_speed / 3).max(MIN_TRIM_BYTES_PER_SEC)
}

/// 这条腿是不是 IP 字面量腿（域名腿**永不摘**，见规格 D7）。
///
/// D7 的理由是代价不对称：域名那条是唯一「IP 变了还能活」的腿，把它摘掉等于把任务
/// 交给一组可能全死的 IP；而放过一条 IP 腿只是少摘一条。
///
/// 为什么不用字符串包含判断（`uri.contains("://1.")` 之类）：域名里带点号太常见，
/// `http://ip-1.2.3.4.example.com/x` 会被误判成 IP 腿——正是 D7 要保住的那条腿。
/// 所以这里按 `scheme://authority/path` 切出 host，再交给 `std::net` 的类型解析器；
/// 仓库没有 `url` 依赖（`core/Cargo.toml` 只有 serde/serde_json/crc/ureq），
/// 而且这里只需要 host 一个字段，为它引一个 URL 库不划算。
///
/// 解析不了的输入（空串、垃圾、没有 `://`、host 不是合法 IP）一律 `false`——
/// 方向是安全的：认不出就当域名腿，永不摘。
pub fn leg_is_ip_literal(uri: &str) -> bool {
    // 只认带 scheme 的绝对 URI：`1.2.3.4/x`、`//1.2.3.4/x` 这种半截输入在 aria2 的腿表里
    // 不会出现，真出现了也说明数据已经坏了，按域名腿处理（不摘）——宁可少摘，不可错摘。
    let Some((_, rest)) = uri.split_once("://") else {
        return false;
    };
    // authority 到第一个 `/`、`?`、`#` 为止。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 去掉 `user:pass@` 前缀（取最后一个 `@`，密码里可能含 `@` 的转义形态）。
    let host_and_port = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    if host_and_port.is_empty() {
        return false;
    }
    // IPv6 字面量在 URI 里必须带方括号：`[::1]`、`[2001:db8::1]:8080`。
    if let Some(rest) = host_and_port.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        // 方括号之后只允许**空**（无端口）或 **`:` 开头的端口**。否则 `[::1]evil.example`
        // 这种「合法 IPv6 + 非法尾巴」的拼接会落进 `rest[..end]` 的解析里成功，
        // 把一个域名 host 判成 IP 腿（危险方向：本该永不摘的域名腿会被摘掉）。
        let after = &rest[end + 1..];
        if !(after.is_empty() || after.starts_with(':')) {
            return false;
        }
        return rest[..end].parse::<std::net::Ipv6Addr>().is_ok();
    }
    // IPv4：`1.2.3.4` 或 `1.2.3.4:8080`。只切端口，不校验端口本身——
    // 端口写错的 URI 同样下不动，但它是不是 IP 腿不受影响。
    let host = match host_and_port.rfind(':') {
        Some(i) => &host_and_port[..i],
        None => host_and_port,
    };
    host.parse::<std::net::Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 真实 aria2 1.37.0 `getServers` 抓包，2026-10-18（仓库自带的
    /// `core/assets/aria2c-macos-arm64` 起 RPC 抓的 `aria2.getServers` 原文）。
    ///
    /// 这条测试钉住**承重事实：`result` 只有一层数组**，元素是文件条目，`index` 是文件序号
    /// 而不是「第几个下载」。把形状改回两层（或按 `index` 索引外层）会让这条测试立刻红，
    /// 而不是像上一版那样由夹具陪着一起错、对真实响应恒返回空表却全绿。
    #[test]
    fn parse_servers_parses_real_capture() {
        let result = json!([{"index":"1","servers":[
            {"currentUri":"http://127.0.0.1:8733/big.bin","downloadSpeed":"4962498","uri":"http://127.0.0.1:8733/big.bin"},
            {"currentUri":"http://localhost:8733/big.bin","downloadSpeed":"4853929","uri":"http://localhost:8733/big.bin"}
        ]}]);
        assert_eq!(
            parse_servers(&result),
            vec![
                Leg {
                    uri: "http://127.0.0.1:8733/big.bin".to_string(),
                    speed: 4_962_498
                },
                Leg {
                    uri: "http://localhost:8733/big.bin".to_string(),
                    speed: 4_853_929
                },
            ],
            "真实抓包必须解析出 2 条腿，且 uri 与 speed 逐字与抓包一致（result 只有一层数组）"
        );
    }

    /// 🔴 同一 URI 的两条连接必须聚成**一条腿、速度相加**。
    ///
    /// 夹具来自实测（aria2 1.37.0，`split=4` + `max-connection-per-server=4`、2 条 URI）：
    /// `getServers` 按**连接**列，同一个 URI 会以两条连接出现。不聚合的后果是
    /// `legtrim` 一拍对同一 key 连加两次（两拍去抖退化成 1 拍），判决还取决于腿表顺序。
    ///
    /// 判别力：把实现改回「见一条 push 一条」⇒ 这里得到 2 条腿（150000 与 50000）⇒ 红。
    #[test]
    fn parse_servers_sums_speed_of_the_same_uri() {
        let result = json!([{
            "index": "1",
            "servers": [
                {"uri": "http://1.2.3.4/x", "downloadSpeed": "150000"},
                {"uri": "http://1.2.3.4/x", "downloadSpeed": "50000"}
            ]
        }]);
        assert_eq!(
            parse_servers(&result),
            vec![Leg {
                uri: "http://1.2.3.4/x".to_string(),
                speed: 200_000
            }],
            "同一 URI 的两条连接必须聚成一条腿、速度相加（150000+50000=200000），位置不变"
        );
    }

    /// 聚合必须**保留首次出现顺序**：重复的那条不许把它挪到后面，也不许另起一条。
    ///
    /// 判别力：用「先收集再按 key 排序」的写法 ⇒ 顺序断言红；用「后者覆盖前者」的写法
    /// ⇒ 速度断言红（变成 1000 或 3000，而不是 4000）。
    #[test]
    fn parse_servers_keeps_first_seen_order_when_merging() {
        let result = json!([{
            "index": "1",
            "servers": [
                {"uri": "http://1.2.3.4/x", "downloadSpeed": "1000"},
                {"uri": "http://5.6.7.8/y", "downloadSpeed": "2000"},
                {"uri": "http://1.2.3.4/x", "downloadSpeed": "3000"}
            ]
        }]);
        assert_eq!(
            parse_servers(&result),
            vec![
                Leg {
                    uri: "http://1.2.3.4/x".to_string(),
                    speed: 4_000
                },
                Leg {
                    uri: "http://5.6.7.8/y".to_string(),
                    speed: 2_000
                },
            ],
            "两条不同 URI + 一条重复 ⇒ 恰好 2 条腿；重复那条留在原位且速度相加"
        );
    }

    /// 线上 `downloadSpeed` 是**字符串**（aria2 把所有整数都以字符串返回），
    /// 但同一份判据也会吃到测试夹具/其他传输喂的数字；缺键视为 0。
    /// 这三种形态读错任意一种，腿表就会整表偏 0 或整表漏掉一条，中位数随之失真。
    /// 夹具形状与真实抓包一致（一层数组），否则测试会在错误的形状上假绿。
    #[test]
    fn parse_servers_reads_speed_as_string_or_number() {
        let result = json!([{
            "index": "1",
            "servers": [
                {
                    "uri": "http://1.2.3.4/x",
                    "currentUri": "http://1.2.3.4/x",
                    "downloadSpeed": "300000"
                },
                {
                    "uri": "http://5.6.7.8/x",
                    "currentUri": "http://5.6.7.8/x",
                    "downloadSpeed": 42
                },
                {
                    "uri": "http://9.9.9.9/x",
                    "currentUri": "http://9.9.9.9/x"
                }
            ]
        }]);
        assert_eq!(
            parse_servers(&result),
            vec![
                Leg {
                    uri: "http://1.2.3.4/x".to_string(),
                    speed: 300_000
                },
                Leg {
                    uri: "http://5.6.7.8/x".to_string(),
                    speed: 42
                },
                Leg {
                    uri: "http://9.9.9.9/x".to_string(),
                    speed: 0
                },
            ],
            "downloadSpeed 的字符串/数字形态或「缺键算 0」有一条不对"
        );
    }

    /// 判据吃的是网络来的结构，坏形状只能让**这一轮不摘腿**，不能 panic。
    /// 这里每一类都对应一个真实可能出现的坏响应：空 `result`、`result` 不是数组、
    /// 文件条目不是对象、`servers` 不是数组、`servers` 缺键。
    #[test]
    fn parse_servers_returns_empty_on_bad_shape() {
        assert!(
            parse_servers(&json!([])).is_empty(),
            "空 result 数组必须返回空表（这一轮没腿可判，不得 panic）"
        );
        assert!(
            parse_servers(&json!({"servers": []})).is_empty(),
            "result 不是数组时必须返回空表"
        );
        assert!(
            parse_servers(&json!([{}])).is_empty(),
            "文件条目缺 servers 键时必须返回空表"
        );
        assert!(
            parse_servers(&json!([{"index": "1", "servers": 7}])).is_empty(),
            "文件条目的 servers 不是数组时必须返回空表"
        );
        assert!(
            parse_servers(&json!(["not-an-object"])).is_empty(),
            "文件条目不是对象时必须返回空表（不得 panic）"
        );
        // 反证：旧版夹具用的两层形状（外层「第几个下载」、内层文件条目）在真实响应里不存在。
        // 若有人把实现改回两层索引，这条与 `parse_servers_parses_real_capture` 会同时红。
        assert!(
            parse_servers(&json!([[{"index": "1", "servers": []}]])).is_empty(),
            "两层数组是虚构形状，必须返回空表（真实 getServers 只有一层）"
        );
    }

    /// 中位数是相对阈值（D6）的分母：空表 → 0；奇数取中间；**偶数取较小的那个**。
    /// 偶数取较大者会让「半数以上」的语义不成立（`[1,2,3,4]` 变 3 而不是 2），
    /// 阈值随之抬高，本该被摘的慢腿反而留下。
    #[test]
    fn median_takes_lower_middle_for_even_lengths() {
        assert_eq!(median(&[]), 0, "空表的中位数必须是 0（不得 panic）");
        assert_eq!(median(&[5]), 5, "单元素的中位数就是它自己");
        assert_eq!(
            median(&[3, 1, 2]),
            2,
            "奇数个必须取排序后的中间那个（与输入顺序无关）"
        );
        assert_eq!(
            median(&[1, 2, 3, 4]),
            2,
            "偶数个必须取较小的中间值 2（取 3 就是取较大者，违反「半数以上」语义）"
        );
    }

    /// 钉住「为什么用中位数而不是平均数」：`[1, 1, 8]` MB/s 里一条 8 倍快的离群腿
    /// 把平均数抬到 3.33 MB/s，`mean/3` 约 1.11 MB/s——比两条正常腿都快，
    /// 换成平均数为分母会把**多数正常腿**判成该摘；中位数仍是 1 MB/s，
    /// 阈值 0.33 MB/s，只摘真正慢的腿。
    #[test]
    fn median_is_not_lifted_by_one_fast_outlier() {
        let speeds = [1_000_000u64, 1_000_000, 8_000_000];
        let mean = speeds.iter().sum::<u64>() / speeds.len() as u64;
        assert_eq!(
            median(&speeds),
            1_000_000,
            "中位数对 8 倍快的离群腿不敏感，必须仍是正常腿的 1 MB/s"
        );
        assert!(
            median(&speeds) / 3 < 1_000_000,
            "中位数阈值必须低于正常腿（否则正常腿也会被误摘）"
        );
        assert!(
            mean / 3 > 1_000_000,
            "这就是不能把平均数当中位数的原因：mean/3 被离群腿抬到正常腿之上"
        );
    }

    /// 阈值是 `max(median/3, 128 KiB/s)`。**下限不能是 0**：冷启动的腿速率还都是 0，
    /// 阈值为 0 时 `speed < 0` 虽不成立，但只要实现写成 `speed <= 阈值` 或别处
    /// 用 0 当「无数据」标记，刚起的腿就会被整批摘掉（D6 的两拍去抖救不了首拍）。
    /// 131072 = 128 KiB/s，与规格 §3.1 的伪代码 `128 * 1024` 一致。
    #[test]
    fn trim_threshold_floors_at_128_kib() {
        assert_eq!(
            trim_threshold(0),
            131_072,
            "median=0 时必须返回 128 KiB/s，返回 0 会让刚起的腿被判成该摘"
        );
        assert_eq!(
            trim_threshold(393_215),
            131_072,
            "median/3 恰好差 1 到 128 KiB/s 时，下限仍然生效"
        );
        assert_eq!(
            trim_threshold(393_216),
            131_072,
            "median/3 正好等于 131072 时取该值（下限不抬高它）"
        );
        assert_eq!(
            trim_threshold(3_000_000),
            1_000_000,
            "median 足够大时阈值必须是 median/3，下限不得继续压着它"
        );
    }

    /// D7：**域名腿永不摘**——域名那条是唯一「IP 变了还能活」的腿。
    /// 判错的代价不对称：把域名腿当 IP 腿摘掉，任务就只剩一组可能全死的 IP；
    /// 把 IP 腿当域名腿放过，只是少摘一条。
    #[test]
    fn leg_is_ip_literal_separates_ip_from_domain() {
        for u in [
            "http://1.2.3.4/x",
            "http://1.2.3.4:8080/x",
            "https://1.2.3.4/x",
            "http://[::1]/x",
            "http://[2001:db8::1]:8080/x",
        ] {
            assert!(
                leg_is_ip_literal(u),
                "{u:?} 的 host 是 IP 字面量，必须判定为 IP 腿（可摘）"
            );
        }
        for u in [
            "http://download.benagen.com/x",
            "http://download.benagen.com:8080/x",
            "",
            "garbage",
            "1.2.3.4/x",
            "http://1.2.3.4.5/x",
        ] {
            assert!(
                !leg_is_ip_literal(u),
                "{u:?} 不是 IP 字面量腿（域名腿永不摘、坏输入不 panic），必须判 false"
            );
        }
    }

    /// 方括号之后只允许空或 `:`。`http://[::1]evil.example/x` 的方括号内是合法 IPv6，
    /// 但尾巴是域名的一部分——旧实现只解析 `[` 到 `]` 之间的内容就返回 true，
    /// 会把这条域名腿误判成 IP 腿（危险方向：D7 要保住的那条腿被摘掉）。
    #[test]
    fn leg_is_ip_literal_rejects_bracket_with_bad_tail() {
        for u in [
            "http://[::1]evil.example/x",
            "http://[::1]evil.example:8080/x",
            "http://[2001:db8::1]x/y",
        ] {
            assert!(
                !leg_is_ip_literal(u),
                "{u:?} 的方括号后是域名尾巴而不是端口，必须判 false（这是域名腿）"
            );
        }
        assert!(
            leg_is_ip_literal("http://[::1]:8080/x"),
            "方括号后是 `:端口` 的合法 IPv6 腿必须判 true"
        );
    }

    /// 安全方向的「近似值」回归：这些都**看起来像** IP 腿，实际都不是 IP 字面量。
    /// 每一条都能让一个真实的 client 输入走到危险方向（把域名腿当 IP 腿摘掉），
    /// 或者反过来把一个合法 IP 腿漏掉；实测值已知，直接钉住。
    #[test]
    fn leg_is_ip_literal_rejects_near_misses() {
        assert!(
            leg_is_ip_literal("HTTP://1.2.3.4/x"),
            "scheme 大小写不影响判定：大写 scheme 的 IP 腿仍必须判 true"
        );
        for u in [
            // 结尾带点：DNS 绝对域名写法，`Ipv4Addr` 解析不认——必须按域名腿处理。
            "http://download.benagen.com./x",
            // 前导零：`std::net::Ipv4Addr` 拒绝（与 inet_aton 的八进制歧义保持距离）。
            "http://01.2.3.4/x",
            // 没有 scheme 的半截输入。
            "1.2.3.4/x",
            "//1.2.3.4/x",
            // IP 出现在域名里：正是 D7 要保住的那条腿。
            "http://1.2.3.4.example.com/x",
            "http://ip-1.2.3.4.example.com/x",
        ] {
            assert!(
                !leg_is_ip_literal(u),
                "{u:?} 不是 IP 字面量腿（域名/坏输入永不摘），必须判 false"
            );
        }
    }
}
