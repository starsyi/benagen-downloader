//! 端到端验收：**真 aria2 引擎 + 限速假边缘**，证明「摘慢腿、补快腿」确实提高总速，
//! 且换腿不会写坏落盘文件。
//!
//! 为什么必须用真引擎：`legtrim.rs` 的判决是纯函数，`rpc.rs` 的参数形状有 HTTP 桩守着，
//! 但「`changeUri` 到底有没有让 aria2 真的换掉连接」「换腿之后总速到底涨没涨」「新连接
//! 写的分段会不会把文件写坏」这三件事，只有真 aria2 才能回答。桩引擎的 `getServers`
//! 是我们自己编的，拿它验证换腿等于拿判决验证判决。
//!
//! 为什么假边缘要**自己**写（不复用 `src/testutil.rs::StubHttp`）：那个桩只会回 200 全量，
//! 不支持 `Range`。而 aria2 的 `split=N` 全靠 `Range` 把文件切成 N 段并发取，回 200 全量
//! 会让它退化成单连接全量下载——那样 4 条腿的速率、`min-split-size` 的作用、
//! 「换腿不损坏数据」全都测不到，测试会以另一种方式变绿。
//!
//! 本文件自带一个 `127.0.0.1:0` 的假边缘农场：每条约 `cap_bps` 的带宽上限（每连接一个
//! 线程、每 8 KiB 补一次 sleep），支持 `Range`、支持 `stop` 打断。这样「两条 10 KB/s 的
//! 慢边缘 + 两条 300 KB/s 的快边缘」就是可复现的现场形状，而不是靠注入假速度。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use benagen_core::crc64xz;
use benagen_core::engine::daemon::{Daemon, DaemonOptions};
use benagen_core::engine::legs::Leg;
use benagen_core::legtrim::TrimState;

const PAYLOAD_LEN: usize = 8 * 1024 * 1024;
const SLOW_BPS: u64 = 10_000;
const FAST_BPS: u64 = 300_000;
const BEAT: Duration = Duration::from_millis(400);
const PATH: &str = "/payload.bin";

// ---------------------------------------------------------------------------
// 假边缘：限速 + Range
// ---------------------------------------------------------------------------

/// 一条假边缘。`Drop` 只置 `stop`：监听线程在 10 ms 内看到它并退出，连接线程的 sleep
/// 也被切成 ≤50 ms 的分片，所以用例结束（或断言失败 panic）时不会留下睡几十秒的线程。
struct Edge {
    addr: SocketAddr,
    cap_bps: u64,
    /// 每条被接受的连接 +1。用例用它证明候选边缘**真的**被连上了，而不只是出现在
    /// `getServers` 的 URI 列表里（后者可能只是「知道这条 URL」，不代表在下载）。
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Edge {
    fn uri(&self) -> String {
        format!("http://{}{}", self.addr, PATH)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 起一条假边缘。监听、线程创建失败都是**测试脚手架**的失败，用 `expect` 立刻报出来；
/// 连接处理路径里不允许 `unwrap`（见 `serve_connection`）。
fn start_edge(payload: Arc<Vec<u8>>, cap_bps: u64) -> Edge {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定假边缘端口失败");
    let addr = listener
        .local_addr()
        .expect("读取假边缘监听地址失败");
    listener
        .set_nonblocking(true)
        .expect("把假边缘监听设为非阻塞失败");

    let stop = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let stop_for_accept = Arc::clone(&stop);
    let hits_for_accept = Arc::clone(&hits);

    std::thread::spawn(move || {
        while !stop_for_accept.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _peer)) => {
                    hits_for_accept.fetch_add(1, Ordering::Relaxed);
                    let payload = Arc::clone(&payload);
                    let stop = Arc::clone(&stop_for_accept);
                    std::thread::spawn(move || {
                        if let Err(e) = serve_connection(stream, &payload, cap_bps, &stop) {
                            // 对端提前关闭（`changeUri` 摘掉腿时就是这种形状）不是测试失败，
                            // 只留下诊断；监听循环本身绝不因为一条连接出错而退出。
                            eprintln!("假边缘连接结束（{e}）");
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    // 不 panic：监听失败只说明这条边缘不再可用，用例会在断言里以
                    // 「腿表没变/速度没涨」的形式报出来，比把 accept 线程炸掉更好查。
                    eprintln!("假边缘 accept 失败: {e}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        // listener 在这里被 drop，端口随之关闭。
    });

    Edge {
        addr,
        cap_bps,
        hits,
        stop,
    }
}

/// 服务一条连接：解析请求行 + `Range`，回 206/200，按 `cap_bps` 节流发送。
///
/// 每条连接只服务一个请求就关闭（响应带 `Connection: close`）。这样实现最简单，
/// 也不会把「上一段没发完的字节」和「下一个请求的头」搅在同一个 TCP 流里；
/// aria2 需要更多分段时会自己重连——那正是我们要它做的事。
fn serve_connection(
    mut stream: TcpStream,
    payload: &[u8],
    cap_bps: u64,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    // 请求头按 `\r\n\r\n` 结束读；一次请求的头很小，16 KiB 足够，也防止一个畸形对端
    // 让我们无限吃内存。
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if head.len() > 16 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "请求头超过 16 KiB",
            ));
        }
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&head);

    let request_line = head.lines().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    if path != PATH {
        write_plain(&mut stream, 404, "Not Found")?;
        return Ok(());
    }

    let total = payload.len();
    let range = parse_range(&head, total);

    // 没有 Range ⇒ 200 + 全量；有 Range ⇒ 206 + 那一段。两者都节流：
    // 假边缘的意义就是让「每条腿的速率」由我们定，而不是由 localhost 的磁盘速度定。
    let (status, start, end) = match range {
        Some((start, end)) => (206, start, end),
        None => (200, 0, total.saturating_sub(1)),
    };
    let body_len = if total == 0 || end < start {
        0
    } else {
        end - start + 1
    };

    let mut resp_head = String::new();
    if status == 206 {
        resp_head.push_str("HTTP/1.1 206 Partial Content\r\n");
        resp_head.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    } else {
        resp_head.push_str("HTTP/1.1 200 OK\r\n");
    }
    resp_head.push_str("Content-Type: application/octet-stream\r\n");
    resp_head.push_str("Accept-Ranges: bytes\r\n");
    resp_head.push_str(&format!("Content-Length: {body_len}\r\n"));
    resp_head.push_str("Connection: close\r\n\r\n");
    stream.write_all(resp_head.as_bytes())?;

    // 先写头再按时间轴节流。节流用「从开始发送到现在应发的字节数」算，而不是
    // 「每块睡固定时长」：后者会把 TCP 缓冲里的瞬时突发也算成带宽，短窗口采样（aria2
    // 是 1 s 粒度的滑动平均）会读到比 cap 高的速率，提速断言就失去判别力。
    let started = Instant::now();
    let body = &payload[start..start + body_len];
    let mut sent = 0usize;
    while sent < body.len() {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let n = (body.len() - sent).min(8 * 1024);
        stream.write_all(&body[sent..sent + n])?;
        sent += n;

        let allowed = Duration::from_secs_f64(sent as f64 / cap_bps as f64);
        while started.elapsed() < allowed {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            // 分片 sleep：最长 50 ms 就能响应 `stop`，用例失败/结束时不会留下长睡线程。
            let remain = allowed.saturating_sub(started.elapsed());
            std::thread::sleep(remain.min(Duration::from_millis(50)));
        }
    }
    stream.flush()
}

/// 解析 `Range: bytes=a-b`（含 `a-` 开区间与 `-n` 后缀区间）。
///
/// 返回 `None` 表示「没带 Range」，调用方回 200 全量；畸形 Range 同样按没带处理
/// （回全量是安全的：最多多传一段，不会把错的数据写到错的位置）。
fn parse_range(head: &str, total: usize) -> Option<(usize, usize)> {
    let mut spec = None;
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("range") {
                spec = Some(value.trim());
                break;
            }
        }
    }
    let spec = spec?;
    let spec = spec.strip_prefix("bytes=")?;
    let spec = spec.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;

    if a.trim().is_empty() {
        let suffix: usize = b.trim().parse().ok()?;
        if suffix == 0 || total == 0 {
            return None;
        }
        return Some((total.saturating_sub(suffix), total - 1));
    }
    let start: usize = a.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if b.trim().is_empty() {
        total - 1
    } else {
        b.trim().parse::<usize>().ok()?.min(total - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end))
}

fn write_plain(stream: &mut TcpStream, status: u16, reason: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// 测试脚手架
// ---------------------------------------------------------------------------

/// 测试用临时目录：进程 id + 自增序号，避免并行跑的测试踩同一个目录。
/// 不依赖 `#[serial]`——端口全用 0、目录全唯一，用例天然可以并行。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "benagen-legs-e2e-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).expect("创建临时目录失败");
        TempDir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 8 MiB 可复现字节（固定种子 LCG）。用真随机数会让失败无法复现；
/// 用全零又会让「换腿后分段写错位置」这种损坏恰好看不出来。
fn payload_bytes(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x9E37_79B9;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((x >> 24) as u8);
    }
    out
}

fn daemon_options(dir: &Path) -> DaemonOptions {
    DaemonOptions {
        download_dir: dir.to_path_buf(),
        global_opts: BTreeMap::new(),
        per_task: BTreeMap::new(),
        binary_path: None, // 由 `Daemon::start` 自己解出内嵌 aria2c，测试不硬编码 target 路径
        force_port: None,  // 让引擎自己探空闲端口；用例之间不会互相抢端口
        counters: None,
    }
}

fn total_speed(legs: &[Leg]) -> u64 {
    legs.iter().map(|leg| leg.speed).sum()
}

fn describe_legs(legs: &[Leg]) -> String {
    legs.iter()
        .map(|leg| format!("{}={}B/s", leg.uri, leg.speed))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 等 `changeUri` 在**引擎**里生效：`del` 全部消失、`add` 全部出现。
///
/// 为什么不只看返回值：`changeUri` 的返回值是「我们发了什么」，不是「引擎改成了什么」。
/// 只有重新读 `getServers` 才能证明真的换了腿——这正是产品代码注释里那句
/// 「下一次 getServers 是事实来源」的可执行版本。
fn wait_for_leg_change(
    daemon: &Daemon,
    gid: &str,
    del: &[String],
    add: &[String],
    timeout: Duration,
) -> Vec<Leg> {
    let deadline = Instant::now() + timeout;
    loop {
        let legs = daemon
            .servers(gid)
            .unwrap_or_else(|e| panic!("换腿后读腿表失败: {e}"));
        let del_gone = del.iter().all(|u| !legs.iter().any(|leg| &leg.uri == u));
        let add_present = add.iter().all(|u| legs.iter().any(|leg| &leg.uri == u));
        if del_gone && add_present {
            return legs;
        }
        if Instant::now() >= deadline {
            panic!(
                "changeUri 返回成功，但 {} ms 内引擎腿表仍未按它改变：\
                 del={del:?}，add={add:?}，实际腿表=[{}]。\
                 这说明「换腿」只改了本地记账/请求参数，没真的落到 aria2 的连接上",
                timeout.as_millis(),
                describe_legs(&legs)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 轮询到任务 `complete`。用 `snapshot().raw_status` 而不是 `tellStatus`：
/// `Daemon` 没有对外的 `tell_status`，而 `snapshot` 的 `raw_status` 是 aria2 的原始
/// 状态串（含 `complete`），且 `tellStopped` 的尾窗足够覆盖刚完成的任务。
fn wait_until_complete(daemon: &Daemon, gid: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let snap = daemon
            .snapshot()
            .unwrap_or_else(|e| panic!("轮询任务状态时读快照失败: {e}"));
        if let Some(status) = snap.raw_status.get(gid) {
            if status == "complete" {
                return;
            }
        }
        if Instant::now() >= deadline {
            panic!(
                "任务 {gid} 在 {} s 内没有完成（最后状态 = {:?}）。\
                 换腿若让 aria2 丢掉了分片或卡在旧连接上，症状就是这里超时",
                timeout.as_secs(),
                snap.raw_status.get(gid)
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 主用例：两条 10 KB/s 慢边缘被摘掉、换成两条 300 KB/s 候选，总速显著提高，
/// 并且换腿之后落盘文件的 CRC 与 payload 一致。
#[test]
fn trimming_two_slow_edges_lifts_total_speed_and_finishes_the_file() {
    let payload = Arc::new(payload_bytes(PAYLOAD_LEN));

    // 4 条在用的边缘：2 慢 2 快。慢的进 `del`，快的必须留下（它们是「有快腿在」的前提，
    // 也是「至少留 MIN_LEGS 条」之外仍然有并行度的保证）。
    let slow = [
        start_edge(Arc::clone(&payload), SLOW_BPS),
        start_edge(Arc::clone(&payload), SLOW_BPS),
    ];
    let fast = [
        start_edge(Arc::clone(&payload), FAST_BPS),
        start_edge(Arc::clone(&payload), FAST_BPS),
    ];
    // 2 条候选：**不进初始 URI 列表**，只能靠 `changeUri` 的 `add` 进来。
    let candidates = [
        start_edge(Arc::clone(&payload), FAST_BPS),
        start_edge(Arc::clone(&payload), FAST_BPS),
    ];

    let initial_uris: Vec<String> = slow
        .iter()
        .chain(fast.iter())
        .map(Edge::uri)
        .collect();
    let candidate_uris: Vec<String> = candidates.iter().map(Edge::uri).collect();
    let slow_uris: BTreeSet<String> = slow.iter().map(Edge::uri).collect();
    let candidate_set: BTreeSet<String> = candidate_uris.iter().cloned().collect();

    let tmp = TempDir::new("trim");
    let dir = tmp.path().to_path_buf();
    let dir_str = dir.to_string_lossy().to_string();

    let daemon = Daemon::start(daemon_options(&dir)).expect("启动真 aria2 失败");
    // 先登记所有边缘的速率，方便失败时一眼看出夹具是不是按预期建起来的。
    for edge in slow.iter().chain(fast.iter()).chain(candidates.iter()) {
        eprintln!("边缘 {} cap={}B/s", edge.uri(), edge.cap_bps);
    }

    let mut add_opts = BTreeMap::new();
    // split=4 是「最多 4 条腿」；min-split-size=1M 是把 8 MiB 切成 4 段的关键
    // （默认 20M 会让 8 MiB 文件只开 1 条连接，慢腿根本不会产生可观测的速率差）。
    // file-allocation=none 让 8 MiB 立刻落盘，不把时间算进提速测量。
    add_opts.insert("split".to_string(), "4".to_string());
    add_opts.insert("max-connection-per-server".to_string(), "4".to_string());
    add_opts.insert("min-split-size".to_string(), "1M".to_string());
    add_opts.insert("file-allocation".to_string(), "none".to_string());

    let gid = daemon
        .add(&initial_uris, &dir_str, "payload.bin", &add_opts)
        .expect("向真 aria2 提交下载任务失败");

    let started = Instant::now();
    let mut trim = TrimState::new();
    let mut samples: Vec<u64> = Vec::new();
    let mut action_beat: u64 = 0;
    let mut action_del: Vec<String> = Vec::new();
    let mut action_add: Vec<String> = Vec::new();
    let mut action_post: Vec<Leg> = Vec::new();

    // 8 拍 × 400 ms = 3.2 s 的观察窗口：第 4/5 拍动作，之后至少还剩 2 拍用于测提速。
    for beat in 1..=8u64 {
        let legs = daemon
            .servers(&gid)
            .unwrap_or_else(|e| panic!("第 {beat} 拍读腿表失败: {e}"));
        let total = total_speed(&legs);
        samples.push(total);
        eprintln!(
            "第 {beat} 拍：总速 {} B/s（{} KiB/s），腿表=[{}]",
            total,
            total / 1024,
            describe_legs(&legs)
        );

        // 换腿之后不再推进判决：本用例验收的是「第一次动作」以及它带来的提速。
        // 继续推进会让第二次动作（如果新腿一时没跑起来）污染「换腿后 2 拍」的测量窗口。
        if action_beat == 0 {
            let plan = trim.plan(beat, &legs, &candidate_uris);
            if beat <= 3 {
                // 承重：暖机期间速率统计还没稳定（首拍常见 0）。这条断言守的是
                // 「冷启动的噪声不许触发换腿」，不是单纯的拍数检查。
                assert!(
                    plan.is_none(),
                    "暖机第 {beat} 拍不得有任何 plan（前 3 拍只观察、不动作），实际 {plan:?}"
                );
            }
            if let Some(plan) = plan {
                action_beat = beat;
                action_del = plan.del.clone();
                action_add = plan.add.clone();
                daemon
                    .change_uri(&gid, 1, &plan.del, &plan.add)
                    .unwrap_or_else(|e| panic!("第 {beat} 拍 changeUri 失败: {e}"));
                // 事实来源是引擎，不是返回值：换完立刻重读腿表对账。
                action_post =
                    wait_for_leg_change(&daemon, &gid, &plan.del, &plan.add, Duration::from_secs(2));
                eprintln!(
                    "第 {beat} 拍动作：del={:?} add={:?}；换腿后腿表=[{}]",
                    plan.del,
                    plan.add,
                    describe_legs(&action_post)
                );
            }
        }
        std::thread::sleep(BEAT);
    }

    // ---- 断言 1：动作必须发生在暖机结束后的第 4 或第 5 拍 ----
    // 承重：前 3 拍无动作是「不许被冷启动噪声骗到」的证据；第 4/5 拍动作是
    // 「连续 2 拍慢」去抖窗口与暖机窗口衔接正确的证据。第 6 拍以后才动作说明
    // 判决把暖机/去抖拍数算错了；第 3 拍以内动作说明暖机失效。
    assert!(
        action_beat == 4 || action_beat == 5,
        "换腿必须发生在暖机后的第 4 或第 5 拍（连续 2 拍慢才摘），实际第 {action_beat} 拍；\
         拍速样本={samples:?}"
    );

    // ---- 断言 2：del 恰好是两条 10 KB/s 慢腿，add 恰好是两条候选 ----
    // 承重：这条把「摘对了没有」钉死。只断言「非空」会让「摘错一条快腿」也变绿。
    let del_set: BTreeSet<String> = action_del.iter().cloned().collect();
    assert_eq!(
        del_set, slow_uris,
        "del 必须恰好是两条 10 KB/s 的慢边缘（不能多摘快腿、不能漏摘慢腿），实际 del={action_del:?}"
    );
    assert_eq!(action_del.len(), 2, "两条慢腿都该摘：房间 = 4 条腿 - MIN_LEGS(2) = 2");
    let add_set: BTreeSet<String> = action_add.iter().cloned().collect();
    assert_eq!(
        add_set, candidate_set,
        "add 必须恰好是两条候选边缘（按候选顺序补，且不重复），实际 add={action_add:?}"
    );

    // ---- 断言 3：changeUri 之后引擎腿表真的换了 ----
    // 承重：`change_uri` 返回 Ok 只证明 RPC 发出去了。这条重新读引擎，证明
    // `del` 已从腿表消失、`add` 已进入腿表——「真引擎真的换掉了，不是本地记账」。
    for uri in &action_del {
        assert!(
            !action_post.iter().any(|leg| &leg.uri == uri),
            "changeUri 之后 del 里的 {uri} 必须从引擎腿表消失，实际腿表=[{}]",
            describe_legs(&action_post)
        );
    }
    for uri in &action_add {
        assert!(
            action_post.iter().any(|leg| &leg.uri == uri),
            "changeUri 之后 add 里的 {uri} 必须出现在引擎腿表，实际腿表=[{}]",
            describe_legs(&action_post)
        );
    }

    // ---- 断言 4：换腿后 2 拍的平均总速 ≥ 换腿前（第 3、4 拍均值）的 1.5 倍 ----
    // 承重：这是整个特性的目的。算术：2×10 + 2×300 = 620 KB/s → 4×300 = 1200 KB/s
    // ≈ 1.94×；取 1.5× 留出连接重建、采样窗口重叠、TCP 慢启动的余量。
    assert!(
        samples.len() >= action_beat as usize + 2,
        "换腿后至少要有 2 拍速率样本才能算平均，实际只有 {} 拍：{samples:?}",
        samples.len()
    );
    let before_avg = (samples[2] + samples[3]) / 2; // 第 3、4 拍
    let after_avg = (samples[action_beat as usize] + samples[action_beat as usize + 1]) / 2;
    assert!(
        before_avg > 0,
        "换腿前（第 3、4 拍）必须已经测到正的总速，否则「提速」无从谈起：{samples:?}"
    );
    assert!(
        after_avg * 2 >= before_avg * 3,
        "换腿后 2 拍平均总速必须 ≥ 换腿前的 1.5 倍：换腿前均值 {before_avg} B/s（{} KiB/s），\
         换腿后均值 {after_avg} B/s（{} KiB/s），实际倍数 {:.2}×；拍速样本={samples:?}",
        before_avg / 1024,
        after_avg / 1024,
        after_avg as f64 / before_avg as f64
    );

    // ---- 断言 5：任务完成 + 落盘文件 CRC 与 payload 一致 ----
    // 承重：这是最硬的一条。换腿要在分片写到一半时丢掉连接、重新铺分段；
    // 任何「旧连接多写了一段」「新连接从错的偏移开始写」都会让 CRC 对不上。
    wait_until_complete(&daemon, &gid, Duration::from_secs(60));
    let elapsed = started.elapsed();
    let path = dir.join("payload.bin");
    let downloaded = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("读落盘文件 {} 失败: {e}", path.display()));
    assert_eq!(
        downloaded.len(),
        payload.len(),
        "落盘文件长度必须与 payload 一致（换腿不能让 aria2 少写一段）：{} vs {}",
        downloaded.len(),
        payload.len()
    );
    assert_eq!(
        crc64xz::sum(&downloaded),
        crc64xz::sum(&payload),
        "换腿后落盘文件的 CRC 必须与 payload 一致——这是「换腿不损坏数据」最硬的一条"
    );
    eprintln!(
        "完成：{} 拍动作，换腿前均值 {} KiB/s，换腿后均值 {} KiB/s，耗时 {:.2} s，CRC 一致",
        action_beat,
        before_avg / 1024,
        after_avg / 1024,
        elapsed.as_secs_f64()
    );

    // 候选边缘必须真的被连上：只出现在 URI 列表里不等于在下载。
    // 承重：这条把「add 只是记账」和「add 真的补了带宽」分开。
    for (i, edge) in candidates.iter().enumerate() {
        assert!(
            edge.hits() > 0,
            "候选边缘 {i}（{}）一次连接都没有被接受：add 只改了 URI 列表，\
             没真的让 aria2 建立下载连接，提速自然是假的",
            edge.uri()
        );
    }

    daemon.close().expect("关闭真 aria2 失败");
    // `tmp` 的 Drop 在函数末尾删目录；显式 close 已确保引擎不再占着文件。
}

/// 对照用例：4 条**同速**边缘（都是 300 KB/s）时，8 拍内一次都不许摘。
///
/// 这是「全慢/等速不摘」的现场形状：瓶颈不在选边（服务端限速、整条链路拥塞），
/// 换腿只会把同一批候选再试一遍，白抖连接。判据里的「全慢不摘」挡的是「所有腿都低于
/// 阈值」；本用例挡的是「等速且都不慢」——两种形状都不该产生 plan。
#[test]
fn equal_edges_are_never_trimmed() {
    let payload = Arc::new(payload_bytes(PAYLOAD_LEN));
    let edges = [
        start_edge(Arc::clone(&payload), FAST_BPS),
        start_edge(Arc::clone(&payload), FAST_BPS),
        start_edge(Arc::clone(&payload), FAST_BPS),
        start_edge(Arc::clone(&payload), FAST_BPS),
    ];
    let uris: Vec<String> = edges.iter().map(Edge::uri).collect();

    let tmp = TempDir::new("equal");
    let dir = tmp.path().to_path_buf();
    let dir_str = dir.to_string_lossy().to_string();

    let daemon = Daemon::start(daemon_options(&dir)).expect("启动真 aria2 失败");
    let mut add_opts = BTreeMap::new();
    add_opts.insert("split".to_string(), "4".to_string());
    add_opts.insert("max-connection-per-server".to_string(), "4".to_string());
    add_opts.insert("min-split-size".to_string(), "1M".to_string());
    add_opts.insert("file-allocation".to_string(), "none".to_string());

    let gid = daemon
        .add(&uris, &dir_str, "payload.bin", &add_opts)
        .expect("向真 aria2 提交下载任务失败");

    let mut trim = TrimState::new();
    for beat in 1..=8u64 {
        let legs = daemon
            .servers(&gid)
            .unwrap_or_else(|e| panic!("第 {beat} 拍读腿表失败: {e}"));
        eprintln!(
            "等速第 {beat} 拍：总速 {} B/s，腿表=[{}]",
            total_speed(&legs),
            describe_legs(&legs)
        );
        let plan = trim.plan(beat, &legs, &[]);
        assert!(
            plan.is_none(),
            "4 条同为 300 KB/s 的边缘是全慢/等速的现场形状，瓶颈不在选边；\
             第 {beat} 拍却要求换腿（plan={plan:?}）——这会把一次正常下载变成不断摘腿补腿"
        );
        std::thread::sleep(BEAT);
    }

    // 不要求下完：本用例只验收「8 拍内一次都不动作」。
    daemon.close().expect("关闭真 aria2 失败");
}
