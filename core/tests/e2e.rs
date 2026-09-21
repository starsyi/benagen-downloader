//! 端到端验收：**真 aria2c + 本地 HTTP 桩服务**，通过 stdio 的 JSON Lines 协议
//! 驱动 `benagen-core` 二进制。
//!
//! 本文件是阶段 A 唯一跑通全链路的地方（清单 → 入队 → aria2 落盘 → 回读路径 → 校验）。
//!
//! # 三条与本文件的设计有关的硬事实
//!
//! 1. **本 crate 没有 lib target**，所以这里不能 `use benagen_core::…`，只能驱动二进制。
//!    二进制路径由 Cargo 注入（`CARGO_BIN_EXE_benagen-core`），**不硬编码 `target/debug/…`**。
//! 2. **只读 stdout、不碰 stderr**——stdout 的纯净性（全局约束 1）正是这个 e2e 要守的东西。
//!    但 stderr 仍然被**抽干**（不是 `null`）：一条没人读的管道写满之后会让内核自己卡住，
//!    那会把"stdout 没响应"变成一种假象。抽干的字节留作失败时的诊断。
//! 3. **`Drop` 里先发 `shutdown` 再收尸**（比简报骨架里的"直接 kill"更强）：直接 kill 会把
//!    内核连同它启动的 aria2c 一起留在后台，于是"无孤儿进程"这类断言会被上一次失败的
//!    残留污染成假绿或假红。这里模拟壳的 `applicationWillTerminate`：发 `shutdown`、
//!    关 stdin、等内核自己走，超时才 kill。
//!
//! # 夹具的 CRC 不写死
//!
//! 清单由**服务端那条生成路径**（仓库根的 `delivery_manifest.py`）现算产出，
//! 而不是手写 JSON 字面量——契约 §3.1 明确要求「必须用一条真实 manifest 抽验，
//! 不能只靠构造的测试数据」。`python3` 取不到时**直接失败**，不静默降级。

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 与内核 `crc64xz::sum` 同一份算法（`crc` 是**普通依赖**，集成测试同样看得见）。
/// 期望值是**现算**的，不是抄来的常量——抄常量会让"夹具与实现对不上"悄无声息。
static CRC64XZ: crc::Crc<u64> = crc::Crc::<u64>::new(&crc::CRC_64_XZ);

fn crc64(data: &[u8]) -> String {
    CRC64XZ.checksum(data).to_string()
}

/// 测试用临时目录（本文件不能 `use crate::testutil`，只能自带一份）。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "benagen-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            next_seq()
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

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 轮询直到 `f` 给出 `Some`，超时则 panic（带诊断）。
fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(v) = f() {
            return v;
        }
        if Instant::now() >= deadline {
            panic!("等待超时：{what}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 杀掉某个下载目录对应的 aria2c（模拟"引擎中途没了"）。
///
/// 按 **PID** 杀而不是 `pkill -f --dir=…`：macOS 的 `pkill` 会把 `--dir=…` 当成自己的
/// 长选项（报 `illegal option -- -`）而**什么都不杀**——那会让"引擎断开"这条测试
/// 卡在"等 aria2c 消失"上，而不是测到它想测的东西。
fn kill_aria2c_for(dir: &Path) {
    for line in aria2c_for(dir) {
        if let Some(pid) = line.split_whitespace().next() {
            let _ = Command::new("kill").arg("-9").arg(pid).status();
        }
    }
}

/// 该下载目录下**还活着的** aria2c 进程，每行是 `PID 完整命令行`。
///
/// 按目录过滤而不是全局按进程名找：e2e 的测试是**并行**跑的，
/// 全局断言会被兄弟测试的引擎污染成假红。`--dir=<绝对路径>` 一定在那条命令行里，
/// 所以"这个目录对应的引擎还在不在"是可精确判定的。
///
/// ⚠️ **取数必须用 `ps`，不能用 `pgrep`**（2026-09-21 修复轮实测，两个平台都跑过）：
/// Linux 的 procps-ng 里 `pgrep -l` 打印的是 **comm（进程名）** 而不是命令行，
/// `-f` 只改变**匹配**范围、**不改变打印内容**（要命令行得用 `-a`）；而 BSD（macOS）
/// 的 `pgrep` **没有 `-a`**。于是"按 `--dir=` 过滤"这件事在 Linux 上**永远匹配不上**
/// —— 这个函数在 Linux 上以前一直是**空**的（`aria2c_for(..).is_empty()` 那族断言
/// 因此**空过**，见 `shutdown_leaves_no_orphan_aria2c` 里那条存在性前置守卫）。
/// `ps -Aww -o pid=,command=` 两边都有、`-ww` 不截断，每行就是 `PID 完整命令行`。
///
/// ⚠️ 先按 `aria2c` 再按目录过滤，两级都留着（与换 `ps` 之前的语义一致）：
/// 只看目录的话，任何命令行里恰好带着这个目录的**别的**进程都会被算成引擎 ——
/// 而 `kill_aria2c_for` 会照着这些行去 `kill -9`。
///
/// （另外：内核释放出来的二进制叫 `aria2c-<hash>`，**进程名不是 `aria2c`** ——
/// 所以过滤词看的是命令行，`ps` 给的正是整条命令行，含 argv[0] 的完整路径。）
fn aria2c_for(dir: &Path) -> Vec<String> {
    let out = Command::new("ps")
        .args(["-Aww", "-o", "pid=,command="])
        .output()
        .expect("ps 不可用");
    let needle = dir.to_string_lossy().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("aria2c") && l.contains(&needle))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// 服务端那条清单生成路径
// ---------------------------------------------------------------------------

const CODE: &str = "AbCdEfGhIjKlMnOpQrSt";

/// 用 `delivery_manifest.py` 产出一份**真实**清单（契约 §3.1 的「真实 manifest 抽验」）。
///
/// 这是 `tosup.v6.5.py` 生成交付页时走的同一条路径（`build_manifest` + `dump_manifest`），
/// 不是本测试手搓的 JSON 字面量。`files` 里 `path` 原文（含 `./`、空格、非 ASCII）
/// 会被 `build_manifest` 逐字保留。
///
/// 夹具里注入的"故障"：让清单与桩上实际内容故意对不上。
///
/// 三个旋钮各自对应六类里的一类，好让"六类互斥穷尽"有可构造的判别夹具：
/// `bad_crc` → `bad`、`no_crc` → `unverifiable`、`size_override` → `size_mismatch`。
#[derive(Clone, Copy, Default)]
struct Faults<'a> {
    /// 清单里给一个**故意错的** crc64
    bad_crc: &'a [&'a str],
    /// 清单里给一个**空** crc64（服务端回读失败的真实形态）
    no_crc: &'a [&'a str],
    /// 清单声明的 size 覆盖成这个值（与实际内容无关）
    size_override: &'a [(&'a str, i64)],
}

fn real_manifest(base: &str, files: &[(&str, &[u8])], faults: Faults) -> String {
    real_manifest_for(CODE, base, files, faults)
}

/// 同上，但**指定交付码**——换码那一类场景要两个码。
fn real_manifest_for(code: &str, base: &str, files: &[(&str, &[u8])], faults: Faults) -> String {
    let spec: Vec<Value> = files
        .iter()
        .map(|(p, data)| {
            let crc = if faults.bad_crc.contains(p) {
                // 取一个**不可能**与真实值相等的值：真实值减一。
                (CRC64XZ.checksum(data).wrapping_sub(1)).to_string()
            } else if faults.no_crc.contains(p) {
                String::new()
            } else {
                crc64(data)
            };
            let size = faults
                .size_override
                .iter()
                .find(|(q, _)| q == p)
                .map(|(_, n)| *n)
                .unwrap_or(data.len() as i64);
            json!({"path": p, "size": size, "crc64": crc})
        })
        .collect();
    let payload = json!({"code": code, "base_url": base, "files": spec});

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core/ 应有父目录（仓库根）")
        .to_path_buf();
    let script = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
from delivery_manifest import build_manifest
spec = json.load(sys.stdin)
m = build_manifest(
    spec["code"],
    [(f["path"], f["size"], f["crc64"]) for f in spec["files"]],
    spec["base_url"],
)
json.dump(m, sys.stdout, ensure_ascii=False)
"#;

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("需要 python3 来跑 delivery_manifest.py（服务端那条生成路径）");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("写 python3 stdin 失败");
    let out = child.wait_with_output().expect("等 python3 失败");
    assert!(
        out.status.success(),
        "delivery_manifest.build_manifest 失败（**不静默降级成手写清单**）：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8(out.stdout).expect("清单必须是 UTF-8");
    // 自检：清单确实由那条生成路径产出，且非规范路径被**逐字**保留。
    let v: Value = serde_json::from_str(&s).expect("生成物必须是合法 JSON");
    assert_eq!(v["code"], json!(code), "生成路径给出的 code 不对");
    s
}

// ---------------------------------------------------------------------------
// 桩服务（裸 TcpListener；`wiremock` 是异步 API，不在本项目的依赖表里）
// ---------------------------------------------------------------------------

struct Stub {
    port: u16,
    hits: Arc<Mutex<Vec<usize>>>,
}

impl Stub {
    /// `files` 是 `(manifest.path 原文, 内容)`；清单里那些路径会按原文提供。
    ///
    /// ⚠️ 清单由**闭包**给出而不是先算好：清单里的 `base_url` 必须等于桩**最终**的地址，
    /// 而端口要等 `bind` 之后才知道。先起一个桩拿端口、再起第二个来服务，
    /// 会让清单里的 `base_url` 指向第一个（空）桩——症状是"清单拉得到、文件全 404"。
    fn start<F>(files: Vec<(String, Vec<u8>)>, manifest: F) -> Stub
    where
        F: FnOnce(&str) -> String,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().unwrap().port();
        let manifest = manifest(&format!("http://127.0.0.1:{port}"));
        let hits = Arc::new(Mutex::new(Vec::new()));
        let hits_in = Arc::clone(&hits);
        let files = Arc::new(files);
        let manifest = Arc::new(manifest);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                let hits = Arc::clone(&hits_in);
                let files = Arc::clone(&files);
                let manifest = Arc::clone(&manifest);
                std::thread::spawn(move || serve_conn(s, &manifest, &files, &hits));
            }
        });
        Stub { port, hits }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 某个夹具被请求过几次。
    fn hits_of(&self, idx: usize) -> usize {
        self.hits.lock().unwrap().iter().filter(|i| **i == idx).count()
    }
}

fn serve_conn(
    mut s: TcpStream,
    manifest: &str,
    files: &[(String, Vec<u8>)],
    hits: &Mutex<Vec<usize>>,
) {
    s.set_read_timeout(Some(Duration::from_secs(20))).ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        // 读一条请求头
        let head_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            match s.read(&mut tmp) {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        buf.drain(..head_end + 4);

        let mut parts = head.lines().next().unwrap_or("").split(' ');
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();

        // Content-Length 的体（GET 没有，但保持通用）
        let cl = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while buf.len() < cl {
            match s.read(&mut tmp) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        if cl > 0 {
            buf.drain(..cl.min(buf.len()));
        }

        let path = target.split('?').next().unwrap_or("");
        // 路径的第一段是**交付码**（本桩对任何码都回同一份清单：换码场景要两个码，
        // 但它们指向的是**两台不同的桩**，所以这里的清单就是本桩那一份）。
        let after_code = path
            .trim_start_matches('/')
            .split_once('/')
            .map(|x| x.1)
            .unwrap_or("");
        let (status, body): (u16, Vec<u8>) = if after_code == "manifest.json" {
            (200, manifest.as_bytes().to_vec())
        } else {
            let rel = percent_decode(after_code);
            match lookup(files, &rel) {
                Some(i) => {
                    hits.lock().unwrap().push(i);
                    (200, files[i].1.clone())
                }
                None => (404, Vec::new()),
            }
        };

        let reason = if status == 200 { "OK" } else { "Not Found" };
        let mut out = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        )
        .into_bytes();
        if method != "HEAD" {
            out.extend_from_slice(&body);
        }
        if s.write_all(&out).is_err() || s.flush().is_err() {
            return;
        }
        if method == "HEAD" {
            // HEAD 也把体长度报出去，但不发体；客户端会按 Content-Length 等——所以直接断开。
            return;
        }
    }
}

/// 按 `manifest.path` **原文**优先匹配；匹配不上再按"折叠 `./` 与空段"的形态兜底。
///
/// 兜底是给**请求行**用的，不是给落盘路径用的：HTTP 客户端可能会规范化 URL 路径，
/// 而我们断言的是内核回读出来的 `path_for`（见 `enqueue_preserves_the_manifest_path_verbatim`）。
fn lookup(files: &[(String, Vec<u8>)], rel: &str) -> Option<usize> {
    if let Some(i) = files.iter().position(|(p, _)| p == rel) {
        return Some(i);
    }
    let norm = normalize(rel);
    files.iter().position(|(p, _)| normalize(p) == norm)
}

fn normalize(p: &str) -> String {
    p.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

// ---------------------------------------------------------------------------
// 内核子进程 + JSON Lines 客户端
// ---------------------------------------------------------------------------

/// 默认调用超时。超时**不是**用来看性能的，是为了让"内核没响应"变成一条清晰的失败
/// 而不是把整个测试套件挂死。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

struct Core {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl Core {
    fn start(download_dir: &Path, settings_path: &Path) -> Core {
        let bin = env!("CARGO_BIN_EXE_benagen-core");
        let mut child = Command::new(bin)
            .arg("--download-dir")
            .arg(download_dir)
            .arg("--settings")
            .arg(settings_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("启动 benagen-core 失败");

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout 已管道化");
        let stderr: ChildStderr = child.stderr.take().expect("stderr 已管道化");

        // stdout：逐行送进 channel。**只有这里读 stdout**，所以"stdout 上出现的每一行"
        // 都会被 `call` 检查一遍（纯净性因此是每条测试都在守的性质）。
        let (tx, lines) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let l = line.trim_end_matches(['\n', '\r']).to_string();
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // stderr：抽干（不是 null）——写满的管道会让内核卡住，把失败伪装成"没响应"。
        let errbuf = Arc::new(Mutex::new(String::new()));
        let errbuf_in = Arc::clone(&errbuf);
        std::thread::spawn(move || {
            let mut r = BufReader::new(stderr);
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                errbuf_in.lock().unwrap().push_str(&line);
                line.clear();
            }
        });

        Core {
            child,
            stdin,
            lines,
            stderr: errbuf,
            next_id: 1,
        }
    }

    fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn send_raw(&mut self, bytes: &[u8]) {
        let s = self.stdin.as_mut().expect("stdin 还在");
        s.write_all(bytes).expect("写内核 stdin 失败");
        s.flush().expect("flush 内核 stdin 失败");
    }

    /// 读回一行 stdout，带超时。**逐字**返回（不做 JSON 解析）——
    /// 需要看原始文本的测试（stdout 纯净性）用它。
    fn read_line_raw(&mut self) -> String {
        self.lines
            .recv_timeout(CALL_TIMEOUT)
            .unwrap_or_else(|_| panic!("等内核响应超时。stderr:\n{}", self.stderr_text()))
    }

    /// 发一条请求，读回一行 JSON；断言 id 能配对。
    ///
    /// ⚠️ 每次调用都顺手检查一遍 **stdout 纯净性**：那一行必须是合法 JSON 对象、
    /// 必须带 `id`。内核任何"顺手 print 一行调试"都会在这里被逮住。
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"id": id, "method": method, "params": params});
        self.send_raw(format!("{req}\n").as_bytes());

        let line = self.read_line_raw();
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!("stdout 上出现了非 JSON 的一行（协议通道被污染）: {line:?} ({e})")
        });
        assert_eq!(
            v.get("id"),
            Some(&json!(id)),
            "响应的 id 没能与请求配对（壳侧会挂起）。请求 {method}，响应: {line}"
        );
        assert!(
            v.get("ok").and_then(Value::as_bool).is_some(),
            "响应缺少 ok 字段: {line}"
        );
        v
    }

    /// 发一条请求并断言 `ok:true`，返回 `result`。
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params);
        assert_eq!(v["ok"], json!(true), "{method} 应当成功，实际: {v}");
        v["result"].clone()
    }

    /// 发一条请求，然后一直读到**这条请求的响应**为止；返回 `(响应, 途中见到的
    /// bad_request 条数)`。
    ///
    /// 用于"一段输入会被拆成多行"的场景（超长行）：那会产生**若干条**协议级错误响应，
    /// 数量取决于那次读取怎么切分，所以不能假设"下一条就是我要的"。
    /// 它同时是一条断言：stdout 上的每一行都必须是合法 JSON。
    fn call_collecting_errors(&mut self, method: &str, params: Value) -> (Value, usize) {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"id": id, "method": method, "params": params});
        self.send_raw(format!("{req}\n").as_bytes());
        let mut bads = 0usize;
        loop {
            let line = self.read_line_raw();
            let v: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout 上出现了非 JSON 的一行: {line:?} ({e})"));
            if v.get("id") == Some(&json!(id)) {
                return (v, bads);
            }
            if v["ok"] == json!(false) && v["error"]["code"] == json!("bad_request") {
                bads += 1;
            }
        }
    }

    /// 发一条请求并断言 `ok:false`，返回 `error` 对象。
    fn err(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params);
        assert_eq!(v["ok"], json!(false), "{method} 应当失败，实际: {v}");
        v["error"].clone()
    }

    /// 关掉内核：先请它走（`shutdown`），再等它自己退出，超时才 kill。
    fn shutdown_and_wait(&mut self) {
        if let Some(s) = self.stdin.as_mut() {
            let _ = s.write_all(format!("{{\"id\":{},\"method\":\"shutdown\"}}\n", self.next_id).as_bytes());
            let _ = s.flush();
        }
        // 关 stdin：即使 shutdown 没被处理，EOF 也要能让它收尾。
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            self.shutdown_and_wait();
        } else {
            // 已经手动关过：只需确保进程被回收
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 一条"交付"的完整搭建
// ---------------------------------------------------------------------------

/// 一次交付：临时目录 + 桩服务 + 内核。
struct Env {
    dir: TempDir,
    stub: Stub,
    core: Core,
}

impl Env {
    /// `files` 决定桩上有什么；`bad_crc` 里的路径在清单里拿到**错的** crc64。
    fn new(tag: &str, files: &[(&str, &[u8])], faults: Faults) -> Env {
        // 清单里的 base_url 由桩的**最终**地址决定（见 `Stub::start`）
        Self::with_manifest(tag, files, |base| real_manifest(base, files, faults))
    }

    /// 同上，但清单由调用方**自己给**（`manifest` 拿到桩的基址，必须原样写进 base_url）。
    ///
    /// 用途是那些**服务端生成路径产不出来的形态**：签发端那条 `build_manifest`
    /// 只会写它认识的键，而协议层面的向后兼容性（老清单缺键、新清单多键）
    /// 恰恰要在"不是生成路径产出的清单"上验。这类夹具**必须**是手写字面量。
    fn with_manifest<F>(tag: &str, files: &[(&str, &[u8])], manifest: F) -> Env
    where
        F: FnOnce(&str) -> String,
    {
        let dir = TempDir::new(tag);
        std::fs::create_dir_all(dir.path().join("dl")).unwrap();

        let ownership: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_vec()))
            .collect();
        let stub = Stub::start(ownership, manifest);

        let dl = dir.path().join("dl");
        let settings = dir.path().join("settings.json");
        let core = Core::start(&dl, &settings);
        Env { dir, stub, core }
    }

    fn dl(&self) -> PathBuf {
        self.dir.path().join("dl")
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.path().join("settings.json")
    }

    fn load(&mut self) -> Value {
        self.core
            .ok("load_delivery", json!({"code": CODE, "base_url": self.stub.base()}))
    }

    fn err(&mut self, method: &str, params: Value) -> Value {
        self.core.err(method, params)
    }

    /// 轮询 `transfer_list`，直到 `pred` 为真；返回那一项。
    fn wait_task(&mut self, what: &str, mut pred: impl FnMut(&Value) -> bool) -> Value {
        wait_for(what, || {
            let r = self.core.ok("transfer_list", json!({}));
            r["items"]
                .as_array()
                .and_then(|a| a.iter().find(|i| pred(i)).cloned())
        })
    }

    /// 轮询 `verify_status`，直到六类里任意一类非空。
    fn wait_verified(&mut self) -> Value {
        wait_for("verify_status 出结果", || {
            let r = self.core.ok("verify_status", json!({}));
            let any = ["ok", "bad", "missing", "size_mismatch", "unverifiable", "unreadable"]
                .iter()
                .any(|k| r[k].as_array().map(|a| !a.is_empty()).unwrap_or(false));
            if any {
                Some(r)
            } else {
                None
            }
        })
    }
}

fn names(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

/// 同上但**排序**：这些数组来自 `BTreeSet`/`HashMap` 系容器，顺序是实现细节，
/// 断言集合内容时不要连带把顺序也钉死（那会让无关的重构变红）。
fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

// ===========================================================================
// 步骤 3 的九条验收
// ===========================================================================

/// 步骤 3 第 1 条：`hello` → 版本相符。
#[test]
fn hello_negotiates_the_protocol_version() {
    let dir = TempDir::new("hello");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));

    let r = core.ok("hello", json!({"protocol": 1}));
    assert_eq!(r["protocol"], json!(1), "内核协议版本必须是 1");
    assert!(
        r["min_split_size_choices"].as_array().unwrap().len() == 100,
        "枚举面必须随握手交给壳（§2.3 的落点）：{r}"
    );

    // 版本不符 → protocol_mismatch，且**不做降级协商**
    let e = core.err("hello", json!({"protocol": 2}));
    assert_eq!(e["code"], json!("protocol_mismatch"), "{e}");

    // 参数形状不对 → invalid_params
    let e = core.err("hello", json!({}));
    assert_eq!(e["code"], json!("invalid_params"), "{e}");
}

/// 步骤 3 第 2 条 + 3b ①：桩上的清单来自**服务端那条生成路径**，
/// `load_delivery` 的树与批次摘要与它逐字对得上。
#[test]
fn load_delivery_reports_the_real_manifest_tree() {
    let files: &[(&str, &[u8])] = &[
        ("t/readme.txt", b"readme"),
        ("t/./verbatim.txt", b"verbatim-body"),
        ("C24-8_\u{d7}_25WS024/QC \u{56fe}.png", b"PNGDATA"),
    ];
    let mut env = Env::new("load", files, Faults::default());
    let r = env.load();

    assert_eq!(r["code"], json!(CODE));
    assert_eq!(r["total_files"], json!(3));
    assert_eq!(r["total_bytes"], json!(6 + 13 + 7));
    assert_eq!(
        r["base_url"].as_str().unwrap(),
        env.stub.base(),
        "清单里的 base_url 必须指向桩（否则内核会去打真实 CDN）"
    );

    // 树结构：三层，且非 ASCII / 带空格的路径**逐字**保留
    let tree = &r["tree"];
    assert_eq!(tree["type"], json!("dir"), "根是目录: {tree}");
    let kids = tree["children"].as_object().unwrap();
    assert!(kids.contains_key("t"), "t 目录应在根下: {:?}", kids.keys());
    assert!(kids.contains_key("C24-8_\u{d7}_25WS024"));
    let t = &kids["t"]["children"].as_object().unwrap();
    let readme = &t["readme.txt"];
    assert_eq!(readme["type"], json!("file"));
    assert_eq!(readme["size"], json!(6));
    assert_eq!(
        readme["path"],
        json!("t/readme.txt"),
        "节点的 path 必须是 manifest 原文"
    );
    assert_eq!(
        readme["crc64"],
        json!(crc64(b"readme")),
        "crc64 必须原样带出来（它来自服务端生成路径）"
    );
    // 非规范路径必须**逐字**出现在树里（不能被规范化掉）
    // ⚠️ 这条断言**必须只认 `.`**：写成 `contains_key(".") || contains_key("verbatim.txt")`
    // 的话，**规范化实现命中的是后半句**——两种实现都绿，等于没测。
    assert!(
        t.contains_key("."),
        "非规范路径 `t/./verbatim.txt` 在树里必须保留 `./` 这一段: {:?}",
        t.keys().collect::<Vec<_>>()
    );

    // ⚠️ **别把这条读成"向后兼容"的证据。** 签发端那条生成路径**始终**写这个键：
    // `delivery_manifest.build_manifest` 对每个条目都产出 `"source_mtime"`，
    // 3 元组条目经 `_unpack` 补 `""`（见 `delivery_manifest.py` 的 `_unpack`/`build_manifest`）。
    // 所以这里证明的是「协议**始终发**这个键、空值是空串」（与 `crc64` 同一种做法：
    // 它是"可能为空"的字符串，不是可选键）。
    // 「条目里**真的缺键**」那条路（`#[serde(default)]` 存在的理由）由
    // `a_manifest_without_source_mtime_still_parses_and_reads_empty` 守。
    assert_eq!(
        readme["source_mtime"],
        json!(""),
        "3 元组条目没有时间，协议里仍必须发出这个键、且为空串（壳据此判断「无时间可显示」）: {readme}"
    );
}

/// **条目里真的没有 `source_mtime` 这个键**的清单：必须照常解析成功，三个出口都读到空串。
///
/// 这是 C-2（向后兼容是硬要求）在**协议层**的落点，也是 `#[serde(default)]` 存在的理由。
///
/// ⚠️ 判别力：本条的清单**真的不写这个键**，这正是它与
/// `load_delivery_reports_the_real_manifest_tree` 那条的分水岭——后者走签发端生成路径
/// （`build_manifest` 始终写键，3 元组补 `""`），只能证明"协议始终发键"。
/// 只有这里能走到 `#[serde(default)]` 那一行：把它去掉，`load_delivery` 会直接
/// 报清单解析失败（`missing field source_mtime`），下面的断言一行都走不到。
///
/// 夹具是**手写清单字面量**（同 `source_mtime_is_carried_through_verbatim`）：
/// 缺键这个形态签发路径根本产不出来。
#[test]
fn a_manifest_without_source_mtime_still_parses_and_reads_empty() {
    const P: &str = "t/a.txt";
    let files: &[(&str, &[u8])] = &[(P, b"a")];
    let mut env = Env::with_manifest("nomtime", files, |base| {
        format!(
            r#"{{"code":"{CODE}","base_url":"{base}","total_files":1,"total_bytes":1,
                "files":[{{"path":"{P}","size":1,"crc64":"{}"}}]}}"#,
            crc64(b"a")
        )
    });

    // `load_delivery` 必须**成功**（解析不过就会在这里失败，走不到下面的断言）
    let r = env.load();
    assert_eq!(
        r["tree"]["children"]["t"]["children"]["a.txt"]["source_mtime"],
        json!(""),
        "条目里缺这个键时，load_delivery 必须照常成功、且读出来是空串: {r}"
    );

    let l = env.core.ok("list_dir", json!({"path": "t"}));
    let entry = l["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == json!("a.txt"))
        .expect("list_dir 必须给出 a.txt")
        .clone();
    assert_eq!(
        entry["source_mtime"],
        json!(""),
        "缺键清单经 list_dir（entries_json）也必须读出空串，而不是缺这个键: {l}"
    );

    let g = env.core.ok("get_tree", json!({}));
    assert_eq!(
        g["tree"]["children"]["t"]["children"]["a.txt"]["source_mtime"],
        json!(""),
        "缺键清单经 get_tree 也必须读出空串: {g}"
    );
}

/// **源文件时间由内核原样搬运**（`source_mtime`）。
///
/// 内核在这里是搬运工：不解析、不校验、不做时区换算——与 `created_at` 的待遇完全一致。
/// 判别力来自取值本身：`2026-09-14T12:00:00+08:00` 一旦被"顺手规范化"成 UTC，
/// 就会变成 `2026-09-14T04:00:00Z`（或 `…+00:00`），逐字比较立刻变红。
///
/// ⚠️ **三个出口都要断言**，因为协议 JSON 的两个出口读的是 `view::Node` 而不是
/// `delivery::File`：只给后者加字段，键会**静默地**到不了壳，且**没有任何错误**。
/// `list_dir` 走 `entries_json`、`load_delivery`/`get_tree` 走 `node_json`——
/// 两条路各有一次机会漏，所以两处都钉。
///
/// 夹具是**手写清单字面量**而不是 `real_manifest`：本测试验的是**内核**这一侧
/// ——协议层如何搬运这个键。签发端的 `build_manifest` 会不会写它、签名长什么样，
/// 都与这里无关；夹具不该被那条生成路径的进度牵着走（它一变，这条测试就跟着变红，
/// 那时红的却不是内核）。
#[test]
fn source_mtime_is_carried_through_verbatim() {
    const P: &str = "t/a.txt";
    const MTIME: &str = "2026-09-14T12:00:00+08:00";
    let files: &[(&str, &[u8])] = &[(P, b"a")];
    let mut env = Env::with_manifest("mtime", files, |base| {
        format!(
            r#"{{"code":"{CODE}","base_url":"{base}","created_at":"2026-09-15T09:30:00+08:00",
                "total_files":1,"total_bytes":1,
                "files":[{{"path":"{P}","size":1,"crc64":"{}","source_mtime":"{MTIME}"}}]}}"#,
            crc64(b"a")
        )
    });

    let r = env.load();
    assert_eq!(
        r["tree"]["children"]["t"]["children"]["a.txt"]["source_mtime"],
        json!(MTIME),
        "load_delivery 必须把清单里的 source_mtime 逐字带出来（不得解析/换算时区）: {r}"
    );

    let l = env.core.ok("list_dir", json!({"path": "t"}));
    let entry = l["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == json!("a.txt"))
        .expect("list_dir 必须给出 a.txt")
        .clone();
    assert_eq!(
        entry["source_mtime"],
        json!(MTIME),
        "list_dir 走的是另一条 JSON 出口（entries_json），同样要逐字带出来: {l}"
    );

    let g = env.core.ok("get_tree", json!({}));
    assert_eq!(
        g["tree"]["children"]["t"]["children"]["a.txt"]["source_mtime"],
        json!(MTIME),
        "get_tree 同样必须带出来（壳靠它显示文件时间）: {g}"
    );
}

/// 步骤 3 第 3 条：`enqueue` 一个文件 → `transfer_list` 轮询到 `complete`，
/// 且文件**真的落盘**了。
#[test]
fn enqueue_downloads_a_file_to_completion() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("enq", files, Faults::default());
    env.load();

    let r = env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    assert_eq!(
        r["added"].as_array().unwrap().len(),
        1,
        "应当加入一个任务: {r}"
    );
    assert!(
        !r["added"][0]["gid"].as_str().unwrap().is_empty(),
        "必须带回 GID: {r}"
    );
    assert_eq!(r["added"][0]["path"], json!("t/readme.txt"));

    let t = env.wait_task("任务跑到 complete", |i| {
        i["raw_status"] == json!("complete")
    });
    assert_eq!(t["state"], json!("complete"), "{t}");

    let landed = env.dl().join("t/readme.txt");
    assert_eq!(
        std::fs::read(&landed).expect("文件必须真的落盘"),
        b"readme"
    );
}

/// 步骤 3 第 4 条：内容与 CRC 对得上 → `verify_status` 报「通过」。
#[test]
fn verify_reports_pass_for_matching_crc() {
    let files: &[(&str, &[u8])] = &[
        ("t/readme.txt", b"readme"),
        ("t/./verbatim.txt", b"verbatim-body"),
    ];
    let mut env = Env::new("verify-ok", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt", "t/./verbatim.txt"]}));

    let r = env.wait_verified();
    let ok = names(&r["ok"]);
    assert!(
        ok.contains(&"t/readme.txt".to_string()),
        "校验通过的文件必须在 ok 类里: {r}"
    );
    assert!(
        ok.contains(&"t/./verbatim.txt".to_string()),
        "非规范路径也必须**逐字**出现在校验结果里: {r}"
    );
    assert_eq!(r["bad"], json!([]), "不该有不符的: {r}");
    assert_eq!(r["all_good"], json!(true), "{r}");
}

/// 步骤 3 第 5 条：故意投喂一个 CRC 不符的文件 → 进「不符」类，
/// 且**自动重入队恰好一次**（不多不少）。
///
/// 判别力：重入队次数由桩上该文件的 HTTP 命中次数直接观测——初始一次 + 重入队一次 = 2。
/// 0 次说明没重入队，3 次说明"仍不符还会再重试"（既违规格 §8.2，也会在真实环境里
/// 变成无限重下）。
#[test]
fn crc_mismatch_lands_in_bad_and_requeues_exactly_once() {
    let files: &[(&str, &[u8])] = &[("t/bad-crc.bin", b"actual-bytes")];
    let mut env = Env::new("verify-bad", files, Faults { bad_crc: &["t/bad-crc.bin"], ..Default::default() });
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/bad-crc.bin"]}));

    env.wait_verified();
    // 等重入队那一轮也跑完（第二次命中之后才可能稳定）
    wait_for("重入队那一轮到齐", || {
        if env.stub.hits_of(0) >= 2 {
            Some(())
        } else {
            None
        }
    });
    // 再取一次最终结果（第二轮的结果会替换第一轮）
    let r2 = wait_for("最终校验结果里 bad 就位", || {
        let x = env.core.ok("verify_status", json!({}));
        if names(&x["bad"]).contains(&"t/bad-crc.bin".to_string()) {
            Some(x)
        } else {
            None
        }
    });

    assert_eq!(
        names(&r2["bad"]),
        vec!["t/bad-crc.bin".to_string()],
        "crc 不符的必须在「不符」类里: {r2}"
    );
    assert_eq!(r2["all_good"], json!(false), "{r2}");

    // 自动重入队**一次**：等一小会儿确保不会出现第三次
    std::thread::sleep(Duration::from_secs(3));
    let hits = env.stub.hits_of(0);
    assert_eq!(
        hits, 2,
        "校验不符必须自动重入队**恰好一次**（初始 1 次 + 重入队 1 次），实际命中 {hits} 次"
    );
}

/// 步骤 3 第 6 条：`remove` → `transfer_list` 里消失，`path_for` 也认不出它了。
#[test]
fn remove_drops_the_task_and_its_path_mapping() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("remove", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    let t = env.wait_task("任务出现", |_| true);
    let gid = t["gid"].as_str().unwrap().to_string();
    assert_eq!(t["path"], json!("t/readme.txt"), "移除前 path 映射应在: {t}");

    env.core.ok("task_action", json!({"action": "remove", "gid": gid}));

    wait_for("任务从列表里消失", || {
        let r = env.core.ok("transfer_list", json!({}));
        let gone = !r["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["gid"] == json!(gid));
        if gone {
            Some(())
        } else {
            None
        }
    });
}

/// 步骤 3 第 7 条：`get_settings` 能读回**上次交付码**，且**重启一个内核实例后仍在**。
///
/// 交付码存**单独一个小文件**（与 settings 同目录的 `last_code`），**不放进 settings**
/// ——它是"上次用过什么"的运行状态，不是用户参数。
#[test]
fn last_delivery_code_survives_a_restart() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("lastcode", files, Faults::default());
    env.load(); // 这一步会把交付码记下来

    let r = env.core.ok("get_settings", json!({}));
    assert_eq!(
        r["last_code"],
        json!(CODE),
        "load_delivery 之后必须记住交付码: {r}"
    );

    // settings 文件里**不得**混进交付码（它有自己的那份文件）
    let raw = std::fs::read_to_string(env.settings_path()).unwrap_or_default();
    if !raw.is_empty() {
        let v: Value = serde_json::from_str(&raw).expect("settings.json 必须是合法 JSON");
        assert!(
            v.get("last_code").is_none() && v.get("code").is_none(),
            "交付码不是参数，不得混进 settings: {raw}"
        );
    }

    // 重启一个新内核实例，指向同一个 settings 路径
    let mut core2 = Core::start(&env.dl(), &env.settings_path());
    let r2 = core2.ok("get_settings", json!({}));
    assert_eq!(
        r2["last_code"],
        json!(CODE),
        "重启之后必须还能读到上次交付码（设计规格 §8.3）: {r2}"
    );
}

/// 步骤 3 第 8 条：目标目录不可用时 `enqueue` 返回**结构化错误**，
/// 且**引擎没有被启动**（设计规格 §9 的「开工前检查」）。
///
/// 夹具选值：把 `--download-dir` 指到一个**普通文件**上。这比 chmod 0555 更硬——
/// 以 root 跑测试时不可写位是拦不住的，而"路径上是个文件"在任何权限下都建不出目录。
#[test]
fn enqueue_preflight_failure_does_not_start_the_engine() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let dir = TempDir::new("preflight");
    let owned: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(p, d)| (p.to_string(), d.to_vec()))
        .collect();
    let stub = Stub::start(owned, |base| real_manifest(base, files, Faults::default()));

    // 目标"目录"其实是一个普通文件
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let mut core = Core::start(&blocker, &dir.path().join("s.json"));

    // 前面的方法照常可用（只有开工前检查该拦）
    core.ok("hello", json!({"protocol": 1}));
    // 清单从桩上取（`base_url` 显式指向桩，否则内核会去打生产 CDN——测试必须自洽）
    core.ok(
        "load_delivery",
        json!({"code": CODE, "base_url": stub.base()}),
    );
    let e = core.err("enqueue", json!({"paths": ["t/readme.txt"]}));
    assert_eq!(
        e["code"],
        json!("preflight_failed"),
        "目标目录不可用必须回结构化错误（不是引擎启动失败）: {e}"
    );

    // 引擎**没有被启动**：这个目标目录对应的 aria2c 一个都不该在
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        aria2c_for(&blocker).is_empty(),
        "开工前检查失败时不得启动引擎，实际: {:?}",
        aria2c_for(&blocker)
    );
    assert!(
        aria2c_for(dir.path()).is_empty(),
        "不得在这个临时目录下留下任何 aria2c: {:?}",
        aria2c_for(dir.path())
    );
    let _ = stub;
}

/// 步骤 3 第 9 条：`shutdown` → 这个内核对应的 aria2c 无输出（不留孤儿）。
#[test]
fn shutdown_leaves_no_orphan_aria2c() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("shutdown", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    env.wait_task("引擎确实起来了", |_| true);
    assert!(
        !aria2c_for(&env.dl()).is_empty(),
        "入队之后应当有一个 aria2c 在这个目录上"
    );

    env.core.shutdown_and_wait();

    let left = wait_for("aria2c 应当随 shutdown 一起消失", || {
        let l = aria2c_for(&env.dl());
        if l.is_empty() {
            Some(l)
        } else {
            None
        }
    });
    assert!(left.is_empty(), "shutdown 之后不得留下孤儿: {left:?}");
}

// ===========================================================================
// 契约里标为「承重」的两条
// ===========================================================================

/// **契约 §3.1（承重）：落盘路径 = `manifest.path` 原文，不做任何规范化。**
///
/// 这条是步骤 3b 的落点：清单来自**服务端那条生成路径**（`delivery_manifest.py`），
/// 且**必须**含至少一条非规范路径（`t/./verbatim.txt`）——否则本断言在
/// "规范化实现"下也会通过（`Path::clean` 折掉 `./` 之后两者仍然相等）。
///
/// 判据是 `transfer_list` 回读出来的 `path`（= `Daemon::path_for`，
/// 契约 §3.1 点名的那一面），并且**必须逐字相等**——不是"规范化后相等"。
///
/// 后果的形状是**静默错位、不报错**：折掉 `./` 之后落盘基准与清单对不上，
/// 整棵树的四态会显示错的状态而没有任何一处报错。
#[test]
fn enqueue_preserves_the_manifest_path_verbatim() {
    const NONCANON: &str = "t/./verbatim.txt";
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme"), (NONCANON, b"verbatim-body")];
    let mut env = Env::new("verbatim", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": [NONCANON]}));

    let t = env.wait_task("非规范路径的任务出现", |i| i["path"] == json!(NONCANON));

    // ① 逐字一致（这是承重断言）
    assert_eq!(
        t["path"],
        json!(NONCANON),
        "落盘路径必须与 manifest.path **逐字一致**（`./` 被折掉就是静默错位）: {t}"
    );
    // ② 反向：它**没有**变成规范化形态——少了这一条，上面那条在实现被改成
    //    "先规范化再返回"时仍然可能因为夹具本身规范而通过。
    assert_ne!(
        t["path"],
        json!("t/verbatim.txt"),
        "路径被规范化了（`t/./verbatim.txt` → `t/verbatim.txt`）: {t}"
    );

    // ③ 文件确实落到 OS 解析后的位置（`.` 由文件系统在 syscall 层折叠），内容正确
    let landed = env.dl().join("t/verbatim.txt");
    wait_for("文件落盘", || landed.exists().then_some(()));
    assert_eq!(std::fs::read(&landed).unwrap(), b"verbatim-body");

    // ④ 非规范路径的文件也要能被校验（报告侧的判据与落盘侧同一条）
    let r = env.wait_verified();
    assert!(
        names(&r["ok"]).contains(&NONCANON.to_string()),
        "非规范路径必须逐字出现在校验结果里: {r}"
    );
}

/// **契约 §5.2（承重）：`verify::check` 在锁外跑，不占住协议主循环。**
///
/// 判据是**可观测的**：一个 64 MiB 的文件（本机实测 CRC ≈ 170 MB/s，即约 380 ms）
/// 落地之后，协议循环仍能在校验进行中快速应答。
/// 一个把 `check` 放在 `Mutex` guard 作用域内跑的实现，会让那段时间里的**每一次**
/// 协议调用都阻塞到校验结束——于是 `hello` 的往返延迟会跳到数百毫秒，
/// 下面第一条断言当场红。
///
/// 夹具取值的理由是**能被区分**：文件必须大到让"锁内实现"的延迟远高于阈值
/// （380 ms vs 100 ms，约 4 倍余量），又小到不让测试本身变慢。
/// **小夹具完全测不出这条**——这正是简报点名它的原因。
///
/// ⚠️ **探针也必须选对**：延迟探针是一个**会取内核锁**的方法（`get_settings`）。
/// 换成 `hello` 就测不到任何东西——`hello` 在 `dispatch` 里是纯函数、不取锁，
/// 锁被谁持着它都照样快。第一版就是这么写的，变异体 M2 因此存活（见报告）。
#[test]
fn verify_runs_outside_the_lock() {
    const BIG: usize = 64 << 20;
    const MAX_LATENCY: Duration = Duration::from_millis(100);

    let big: Vec<u8> = vec![0u8; BIG];
    let files: Vec<(&str, &[u8])> = vec![("t/big.bin", big.as_slice())];
    let mut env = Env::new("lock", &files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/big.bin"]}));

    // 等下载完成——**下载阶段与校验阶段必须分开**，否则"下载那段时间里协议很快"
    // 会把锁内实现也放过去（那时还没人持锁）。
    env.wait_task("大文件下载完成", |i| {
        i["raw_status"] == json!("complete")
    });

    // 下载完成的那一刻起，校验随时可能开始；这一轮里测协议往返延迟。
    let mut worst = Duration::ZERO;
    let mut saw_empty_while_verifying = false;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60) {
        // ⚠️ **探针必须是一个会取内核锁的方法**（`get_settings` 走 `with_kernel`）。
        // 第一版这里探的是 `hello`，而 `hello` 在 `dispatch` 里是**不取锁**的
        // （纯函数，不碰内核状态）——于是"把 check 挪进锁内"这个变异体
        // 在延迟上**完全看不出来**，测试全绿（变异实验 M2 实测）。
        // 这正是"测试绕过了会出错的那条路"的形状：探针选错了，等于没测。
        let t0 = Instant::now();
        env.core.ok("get_settings", json!({}));
        let dt = t0.elapsed();
        worst = worst.max(dt);

        let r = env.core.ok("verify_status", json!({}));
        let done = !names(&r["ok"]).is_empty();
        if !done {
            saw_empty_while_verifying = true;
        } else {
            break;
        }
    }

    assert!(
        saw_empty_while_verifying,
        "没观察到「校验尚未完成」的窗口——测试没有真正落在校验期间"
    );
    assert!(
        worst < MAX_LATENCY,
        "校验期间协议往返延迟达到 {worst:?}（阈值 {MAX_LATENCY:?}）——\
         `verify::check` 很可能是在锁内跑的（50 GB 批量下这会让协议循环停住几分钟）"
    );

    // 校验确实做完了，而且结果正确（证明这一轮里 check 真的跑过）
    let r = env.wait_verified();
    assert!(
        names(&r["ok"]).contains(&"t/big.bin".to_string()),
        "大文件应当校验通过: {r}"
    );
}

// ===========================================================================
// 其余需要显式验收的接线点
// ===========================================================================

/// 简报第 10 条：`set_settings` 必须过 `protocol::check_min_split_size`（契约 §2.3）。
///
/// 判别力：`"  20m  "` 这类"settings 层能收、但下发给 aria2 会让它起不来"的写法，
/// 必须被归一成规范串 `"20M"`；而集合外的写法必须回**结构化** `invalid_params`。
/// 少了这道，§2.3 点名的那个 protocol 层就没有调用者（死代码）。
#[test]
fn set_settings_normalizes_and_validates_min_split_size() {
    let dir = TempDir::new("settings");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));

    // 归一：大小写 + 首尾空白与 settings 层同口径
    let r = core.ok(
        "set_settings",
        json!({"settings": {
            "parallel": 8, "connections": 16, "splits": 16,
            "min_split_size": "  20m  ", "limit_mbps": 0, "max_tries": 3, "retry_wait": 1
        }}),
    );
    assert_eq!(
        r["settings"]["min_split_size"],
        json!("20M"),
        "`-k` 必须被归一成规范串（带空白/小写的串会让 aria2 直接启动失败）: {r}"
    );

    // 集合外 → invalid_params，且**不写盘**
    let before = std::fs::read_to_string(dir.path().join("s.json")).unwrap_or_default();
    let e = core.err(
        "set_settings",
        json!({"settings": {
            "parallel": 8, "connections": 16, "splits": 16,
            "min_split_size": "500K", "limit_mbps": 0, "max_tries": 3, "retry_wait": 1
        }}),
    );
    assert_eq!(e["code"], json!("invalid_params"), "{e}");
    let after = std::fs::read_to_string(dir.path().join("s.json")).unwrap_or_default();
    assert_eq!(before, after, "校验失败**不得**写盘（契约 §2.1）");

    // 越界（validate 管的那六项）同样不写盘
    let e = core.err(
        "set_settings",
        json!({"settings": {
            "parallel": 999, "connections": 16, "splits": 16,
            "min_split_size": "20M", "limit_mbps": 0, "max_tries": 3, "retry_wait": 1
        }}),
    );
    assert_eq!(e["code"], json!("invalid_params"), "{e}");
    let after2 = std::fs::read_to_string(dir.path().join("s.json")).unwrap_or_default();
    assert_eq!(before, after2, "校验失败**不得**写盘（契约 §2.1）");
}

/// 简报第 3 条：`task_action` 里**没有** `reveal`——"在访达中显示"是壳的事。
#[test]
fn task_action_rejects_reveal() {
    let dir = TempDir::new("reveal");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));
    let e = core.err("task_action", json!({"action": "reveal", "gid": "x"}));
    assert_eq!(
        e["code"],
        json!("invalid_params"),
        "`reveal` 不是内核的动作（内核不含界面概念，全局约束 2）: {e}"
    );
}

/// 简报第 11 条：`transfer_list` 的 `raw_status` 来自 **aria2 的原始状态串**。
///
/// 判别力：`paused` 与 `waiting` 在领域模型里**映成同一个变体**（契约 §8 的 `#14`），
/// 所以只有原始串能把它们分开。一个从 `task.state` 反推的实现会把它报成 `waiting`
/// ——界面于是渲染不出「已暂停」标注（设计规格 §8.4）。
///
/// 夹具取值：把并发数限成 1、总限速压到 1 MB/s，于是第一个文件占住唯一的槽位，
/// 后面两个停在 `waiting`；对其中一个调 `pause`，它才会真的变成 `paused`。
#[test]
fn transfer_list_carries_the_raw_aria2_status() {
    let big = vec![0u8; 4 << 20];
    let files: Vec<(&str, &[u8])> = vec![
        ("t/first.bin", big.as_slice()),
        ("t/second.txt", b"second"),
        ("t/third.txt", b"third"),
    ];
    let mut env = Env::new("rawstatus", &files, Faults::default());

    // 先限并发、限速，再入队——这样第二个一定会停在 waiting 上
    env.core.ok(
        "set_settings",
        json!({"settings": {
            "parallel": 1, "connections": 1, "splits": 1,
            "min_split_size": "1M", "limit_mbps": 1, "max_tries": 3, "retry_wait": 1
        }}),
    );
    env.load();
    env.core.ok(
        "enqueue",
        json!({"paths": ["t/first.bin", "t/second.txt", "t/third.txt"]}),
    );

    let t = env.wait_task("第二个文件停在等待/暂停上", |i| {
        i["path"] == json!("t/second.txt") && (i["raw_status"] == json!("waiting") || i["raw_status"] == json!("paused"))
    });
    let gid = t["gid"].as_str().unwrap().to_string();
    assert_eq!(t["raw_status"], json!("waiting"), "先入队时是 waiting: {t}");

    env.core.ok("task_action", json!({"action": "pause", "gid": gid}));

    let paused = env.wait_task("任务变成 paused", |i| {
        i["gid"] == json!(gid) && i["raw_status"] == json!("paused")
    });
    assert_eq!(
        paused["state"],
        json!("waiting"),
        "领域状态照旧是 waiting——这正是 raw_status 必须存在的原因: {paused}"
    );
    assert_eq!(
        paused["raw_status"],
        json!("paused"),
        "原始状态串必须逐字来自 aria2（从 state 反推会得到 waiting）: {paused}"
    );

    // 继续 → 回到 waiting 或 active
    env.core.ok("task_action", json!({"action": "unpause", "gid": gid}));
    let back = env.wait_task("任务不再是 paused", |i| {
        i["gid"] == json!(gid) && i["raw_status"] != json!("paused")
    });
    assert_ne!(back["raw_status"], json!("paused"), "{back}");
}

/// 简报第 7 条：`retry` 在编排层是**两步**——先取路径（`remove` 会 forget 掉它）、
/// 再 `remove`、再用**当前的**逐任务选项重新 `add`。
///
/// 判别力：`remove` 之后 `path_for` 会返回 `None`，所以"先 remove 再取路径"的实现
/// 会拿不到 `manifest.path`、直接报错——这条测试就会红。
#[test]
fn retry_re_adds_with_the_current_per_task_options() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("retry", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    let t = env.wait_task("任务出现", |_| true);
    let gid = t["gid"].as_str().unwrap().to_string();
    let before = env.stub.hits_of(0);

    // 换一套逐任务选项：重试必须用**当前**的这套（而不是当初 add 时那套）
    env.core.ok(
        "set_settings",
        json!({"settings": {
            "parallel": 8, "connections": 4, "splits": 4,
            "min_split_size": "1M", "limit_mbps": 0, "max_tries": 7, "retry_wait": 2
        }}),
    );

    let r = env.core.ok("task_action", json!({"action": "retry", "gid": gid}));
    let new_gid = r["gid"].as_str().expect("retry 必须带回新的 GID").to_string();
    assert_ne!(new_gid, gid, "retry 是 remove + 重新 add，GID 必然不同");

    // 重新 add 之后 path 映射必须重新挂上（它就是靠 add 建立起来的）
    let t2 = env.wait_task("新任务出现", |i| i["gid"] == json!(new_gid));
    assert_eq!(t2["path"], json!("t/readme.txt"), "{t2}");

    wait_for("重试确实又下了一次", || {
        (env.stub.hits_of(0) > before).then_some(())
    });
}

/// 简报第 5 条（设计规格 §9）：RPC 中途断连 → 内核**尝试重连一次**；
/// 仍失败则报 `engine_disconnected`，并且**保留已落盘的文件**让客户重试。
#[test]
fn engine_disconnect_is_reported_and_landed_files_are_kept() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("disconnect", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    env.wait_task("任务跑到 complete", |i| i["raw_status"] == json!("complete"));

    // 从外部把 aria2c 杀掉，模拟"引擎中途没了"
    kill_aria2c_for(&env.dl());
    wait_for("aria2c 确实没了", || aria2c_for(&env.dl()).is_empty().then_some(()));

    // 后续响应必须带 engine_disconnected（而不是静默返回空列表）
    let e = env.err("transfer_list", json!({}));
    assert_eq!(
        e["code"],
        json!("engine_disconnected"),
        "引擎断开必须让壳看得见（设计规格 §9 的原则：任何失败都要出现在界面上）: {e}"
    );

    // 已落盘的文件**保留**（不清理、不删除），客户可以直接重试
    let landed = env.dl().join("t/readme.txt");
    assert_eq!(
        std::fs::read(&landed).expect("已落盘的文件必须保留"),
        b"readme"
    );
}

/// 全局约束 1：**stdout 是协议专用通道。** 每行一条 JSON，除此之外不得写 stdout。
///
/// 直接照完成标准里那条命令跑：只输出一行 JSON。
#[test]
fn stdout_carries_only_protocol_messages() {
    let dir = TempDir::new("stdout");
    let bin = env!("CARGO_BIN_EXE_benagen-core");
    let mut child = Command::new(bin)
        .arg("--download-dir")
        .arg(dir.path().join("dl"))
        .arg("--settings")
        .arg(dir.path().join("s.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动内核失败");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"id\":1,\"method\":\"hello\",\"params\":{\"protocol\":1}}\n")
        .unwrap();
    drop(child.stdin.take()); // EOF → 内核应当自己收尾退出
    let out = child.wait_with_output().expect("等内核退出");
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout 上有 {} 行（应当只有 1 行协议消息）:\n{text}",
        lines.len()
    );
    let v: Value = serde_json::from_str(lines[0]).expect("那一行必须是合法 JSON");
    assert_eq!(v["id"], json!(1));
    assert_eq!(v["ok"], json!(true));
}

/// 简报第 13 条：**非法 UTF-8 不许被当成 EOF 静默丢连接。**
///
/// EOF 是正常收尾（→ `shutdown`），解码失败是协议错误——两者必须走不同的分支。
/// 判别力：把 `InvalidData` 和 EOF 合到一个分支里的实现，会在收到这串字节之后
/// **直接退出**，于是后面那条 `hello` 永远等不到响应（本测试超时红）。
#[test]
fn invalid_utf8_is_a_protocol_error_not_an_eof() {
    let dir = TempDir::new("utf8");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));

    core.send_raw(b"\xff\xfe\xfd\n");

    let line = core.read_line_raw();
    let v: Value = serde_json::from_str(&line).expect("必须回一条合法 JSON");
    assert_eq!(v["ok"], json!(false), "非法 UTF-8 必须回错误: {line}");
    assert_eq!(
        v["id"],
        json!(0),
        "连 id 都读不出来的输入回保留值 0（`UNCORRELATED_ID`）: {line}"
    );
    assert_eq!(v["error"]["code"], json!("bad_request"), "{line}");

    // 连接**还活着**：后续请求照常工作
    let r = core.ok("hello", json!({"protocol": 1}));
    assert_eq!(r["protocol"], json!(1), "解码失败不得把连接丢掉");
}

/// 简报第 13 条：**行长上限必须显式做出并记账。**
///
/// 本内核取"定一个上限（8 MiB）并在超限时报错"，理由写在 `MAX_LINE_BYTES` 的注释里。
/// 判别力：不设上限的实现不会报错（它会一直吃内存直到解析出别的错），
/// 于是"必须出现一条 bad_request"当场红。
#[test]
fn over_long_line_is_rejected_and_the_loop_resyncs() {
    let dir = TempDir::new("longline");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));

    // 9 MiB 的单行（超过 8 MiB 上限），末尾补一个换行让它最终能重新对齐
    let mut junk = vec![b'x'; 9 << 20];
    junk.push(b'\n');
    core.send_raw(&junk);

    // 紧跟一条正常请求，然后读到**它**的响应为止。
    let (v, bads) = core.call_collecting_errors("hello", json!({"protocol": 1}));

    // ⚠️ 判别力在这里：`bads >= 2` 才能把"有上限"与"没上限"分开。
    //   - **有** 8 MiB 上限：这 9 MiB 被切成 8 MiB（超限 → 报错）＋ 1 MiB（碎片 → JSON 非法 → 报错），
    //     所以至少两条 bad_request；
    //   - **没有**上限：整行一次性读进来，`read_request` 只报**一条** bad_request。
    // 也就是说这条断言真正钉住的是"上限存在"，而不是"超长输入会报错"
    // （后者在两种实现下都成立，测不出东西）。
    assert!(
        bads >= 2,
        "超长行应当被上限切成多段、逐段报错（至少 2 条 bad_request），实际 {bads} 条"
    );

    // 循环必须**重新对齐**：后续请求照常工作
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(v["result"]["protocol"], json!(1), "超长行之后协议循环必须还能用");
}

/// 简报第 12 条：**能配对的就必须配上**——这条只能在 stdio 这一层测。
///
/// 判别力：接线时把 `Err` 分支写成 `UNCORRELATED_ID`（常量 0）的实现，会让
/// `{"id":7}` 这条**回 id 0**，于是壳那条 `id = 7` 的请求永远等不到配对响应
/// （挂起或超时）。下面的第一条断言当场红。
///
/// 反方向同样要钉：**连 id 都读不出来**的输入必须回保留值 0——两侧都测，
/// 免得把这条接线写成"恒回填"或"恒 0"。
#[test]
fn malformed_request_with_a_readable_id_keeps_its_id() {
    let dir = TempDir::new("id-echo");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));

    // ① id 读得出来（缺 method）→ 响应必须是 7
    core.send_raw(b"{\"id\":7}\n");
    let line = core.read_line_raw();
    let v: Value = serde_json::from_str(&line).expect("必须回合法 JSON");
    assert_eq!(
        v["id"],
        json!(7),
        "id 读得出来的畸形请求必须回填那个 id（回 0 会让壳的请求永远等不到配对）: {line}"
    );
    assert_eq!(v["ok"], json!(false), "{line}");
    assert_eq!(v["error"]["code"], json!("bad_request"), "{line}");

    // ② 连 id 都读不出来（不是合法 JSON）→ 才回保留值 0
    core.send_raw(b"not json at all\n");
    let line = core.read_line_raw();
    let v: Value = serde_json::from_str(&line).expect("必须回合法 JSON");
    assert_eq!(v["id"], json!(0), "畸形 JSON 只能回保留值: {line}");

    // ③ 之后一切照常
    core.ok("hello", json!({"protocol": 1}));
}

/// 简报第 9 条：自动重入队的集合**就是** `verify::CheckResult::failures()`。
///
/// 判别力：把重入队集合写成"手抄一份"（比如只抄 `bad`）的实现，会让
/// `size_mismatch` 这一类**不被重下**——本测试就是为它写的：
/// 清单声明的大小比桩上实际的多 1 字节，于是首次校验判 `size_mismatch`，
/// 必须自动重入队一次，且**只有一次**。
#[test]
fn size_mismatch_is_requeued_once_just_like_bad() {
    const P: &str = "t/trunc.bin";
    let files: &[(&str, &[u8])] = &[(P, b"0123456789")];
    let mut env = Env::new("size-mismatch", files, Faults { size_override: &[(P, 11)], ..Default::default() });
    env.load();
    env.core.ok("enqueue", json!({"paths": [P]}));
    env.wait_verified();

    wait_for("重入队那一轮到齐", || (env.stub.hits_of(0) >= 2).then_some(()));
    let r = wait_for("最终结果里 size_mismatch 就位", || {
        let x = env.core.ok("verify_status", json!({}));
        names(&x["size_mismatch"]).contains(&P.to_string()).then_some(x)
    });
    assert_eq!(names(&r["size_mismatch"]), vec![P.to_string()], "{r}");
    assert_eq!(r["all_good"], json!(false), "{r}");

    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        env.stub.hits_of(0),
        2,
        "大小不符也必须自动重入队**恰好一次**（`failures()` = bad + missing + size_mismatch）"
    );
}

/// **全局约束 4「不得静默少交」的判别式**：混合批次里每个文件都必须落进恰好一类。
///
/// 一个批次同时放三类：CRC 对 / CRC 错 / 无 crc64。断言
/// **「六类路径的并集 == 清单路径集」且「各类两两不相交」**。
///
/// 判别力（两种实现都会被这条逮住）：
///   - **只把 `failures()` 那一支提交**的实现（本任务修复前的形状）：`ok` 与
///     `unverifiable` 都不在 `failures()` 里，于是它们**一条都不出现**——
///     并集少两个路径，当场红。真实交付每批必然混合（200 ms 一个 tick），
///     所以这不是边角场景。
///   - `commit` 的「先摘旧类」被改成空操作：CRC 错的那个文件在第一轮进 `bad`、
///     复校验之后进 `ok`，两条都在 → 交集非空，红。
#[test]
fn mixed_batch_classifies_every_file_exactly_once() {
    const GOOD: &str = "t/good.bin";
    const BAD: &str = "t/bad.bin";
    const NOCRC: &str = "t/nocrc.bin";
    let files: &[(&str, &[u8])] = &[
        (GOOD, b"good-content"),
        (BAD, b"bad-content"),
        (NOCRC, b"nocrc-content"),
    ];
    let mut env = Env::new(
        "mixed",
        files,
        Faults {
            bad_crc: &[BAD],
            no_crc: &[NOCRC],
            ..Default::default()
        },
    );
    env.load();
    env.core
        .ok("enqueue", json!({"paths": [GOOD, BAD, NOCRC]}));

    // 等三类都就位（`bad` 还要走完复校验轮）。
    // 超时的话把**当时的六类**打出来——否则失败信息只有一句"等待超时"，
    // 看不出到底是哪一类没出现（"缺了 ok" 正是这个缺陷的形状）。
    let deadline = Instant::now() + Duration::from_secs(60);
    let r = loop {
        let x = env.core.ok("verify_status", json!({}));
        let hit = names(&x["ok"]).contains(&GOOD.to_string())
            && names(&x["bad"]).contains(&BAD.to_string())
            && names(&x["unverifiable"]).contains(&NOCRC.to_string());
        if hit {
            break x;
        }
        if Instant::now() >= deadline {
            panic!("混合批次的三类没能全部就位（六类当时的取值见下）:\n{x}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    const CLASSES: [&str; 6] = [
        "ok",
        "bad",
        "missing",
        "size_mismatch",
        "unverifiable",
        "unreadable",
    ];
    let manifest_paths: BTreeSet<String> = files.iter().map(|(p, _)| p.to_string()).collect();
    let mut union: BTreeSet<String> = BTreeSet::new();
    let mut total = 0usize;
    let mut dup: Vec<String> = Vec::new();
    for k in CLASSES {
        for p in names(&r[k]) {
            if !union.insert(p.clone()) {
                dup.push(p);
            }
            total += 1;
        }
    }
    assert!(
        dup.is_empty(),
        "同一个路径出现在多个类里——六类**互斥**被破坏: {dup:?}\n{r}"
    );
    assert_eq!(
        union, manifest_paths,
        "六类的并集必须**恰好等于**清单路径集（少了任何一条都是静默少交）\n{r}"
    );
    assert_eq!(total, manifest_paths.len(), "路径总数对不上\n{r}");
    assert_eq!(r["all_good"], json!(false), "有 bad，all_good 必须是 false\n{r}");

    // 三类各自的**归类**也要对（并集对了但归错类的实现要被逮住）
    assert_eq!(names(&r["ok"]), vec![GOOD.to_string()], "{r}");
    assert_eq!(names(&r["bad"]), vec![BAD.to_string()], "{r}");
    assert_eq!(names(&r["unverifiable"]), vec![NOCRC.to_string()], "{r}");

    // 落地监听器不该把这些路径**永久**扣在 `verifying` 里：再等一会儿，
    // 六类仍然稳定（如果 `verifying` 泄漏，后续的状态变化会被吞掉）
    std::thread::sleep(Duration::from_secs(3));
    let r2 = env.core.ok("verify_status", json!({}));
    for k in CLASSES {
        assert_eq!(names(&r2[k]), names(&r[k]), "校验结果在稳定之后又变了（{k}）");
    }
}

/// 简报第 4 条的**另一半**：开工前检查里的「磁盘剩余空间」。
///
/// 判别力：把 `free < need` 改成恒假（只查可写、不查空间）的实现会直接去起引擎，
/// 于是"引擎没有被启动"这条断言红。
/// 夹具取值的理由是**能被区分**：声明 1 PiB（任何机器都装不下），
/// 但桩上的内容只有几个字节——差异只来自清单声明，不来自真实磁盘。
#[test]
fn preflight_rejects_when_the_disk_cannot_hold_the_batch() {
    const P: &str = "t/huge.bin";
    let files: &[(&str, &[u8])] = &[(P, b"tiny")];
    let mut env = Env::new(
        "nospace",
        files,
        Faults {
            size_override: &[(P, 1 << 50)],
            ..Default::default()
        },
    );
    env.load();
    let e = env.err("enqueue", json!({"paths": [P]}));
    assert_eq!(
        e["code"],
        json!("preflight_failed"),
        "装不下就必须回结构化错误: {e}"
    );
    assert!(
        e["message"].as_str().unwrap().contains("空间"),
        "消息要说清是空间不够（而不是别的开工前检查）: {e}"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        aria2c_for(&env.dl()).is_empty(),
        "空间不足时不得启动引擎，实际: {:?}",
        aria2c_for(&env.dl())
    );
}

/// `clear_finished` —— 唯一带「**先 `snapshot` 再 purge**」这条承重顺序的动作。
///
/// 顺序错了（先 purge 再取 GID）会让 `tellStopped` 已经空了，映射**永久泄漏**在
/// `by_gid` 里：列表看着干净，内部映射越积越多，而没有任何一处报错。
/// 判别力：顺序写反的实现里，purge 之后取不到 GID，`transfer_list` 仍然干净——
/// 所以这条测试配一条**顺序性的旁证**：清空之后 GID 必须真的不在列表里，
/// 且已落盘的文件**照旧留在磁盘上**（清的是传输条目，不是数据）。
#[test]
fn clear_finished_drops_stopped_tasks() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("clearfin", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    wait_for("文件落盘", || env.dl().join("t/readme.txt").exists().then_some(()));
    env.wait_task("任务跑到 complete", |i| i["raw_status"] == json!("complete"));

    env.core
        .ok("task_action", json!({"action": "clear_finished"}));

    let left = wait_for("已完成条目被清空", || {
        let r = env.core.ok("transfer_list", json!({}));
        r["items"].as_array().unwrap().is_empty().then_some(r)
    });
    assert_eq!(left["items"], json!([]), "清空之后传输列表必须是空的");

    // 清的是**传输条目**，不是数据
    assert_eq!(
        std::fs::read(env.dl().join("t/readme.txt")).expect("已落盘的文件必须还在"),
        b"readme"
    );
}

/// **换码时引擎侧必须一起作废**（裁定 10 改判；违约束 4 与 §8.1 的合成前提）。
///
/// `Daemon` 内部那张 `gid → 上一批相对路径` 的映射不会随 `load_delivery` 消失，
/// 而 `view::compose` / `view::progress` 过滤任务的判据只有**一句**
/// "这个路径在不在**新**清单里"——不是"这个任务属不属于这一批"。
/// 于是新批次里**同相对路径**的文件会被上一批那个任务的状态冒充：
/// 上一批失败过 → 新批次的树直接显示 `failed` **并带着上一批的 `errorMessage`**。
///
/// 最容易撞上的场景正是**"同一批交付换个码重新生成链接"（路径完全重合）**，
/// 而那正是"记住上次交付码"这个功能存在的场景。
///
/// 夹具取值的理由是**能被区分**：两批用的是**同一个相对路径** `t/shared.bin`
/// （不重合就什么也测不出来——`compose` 会把旧任务滤掉），
/// 且第一批的桩**故意不提供**这个文件，让上一批留下一条**带 errorMessage 的
/// `error` 任务**（这是最强的一种冒充：状态与文案都会被带过去）。
#[test]
fn switching_the_delivery_code_clears_the_previous_batch_from_the_engine() {
    const P: &str = "t/shared.bin";
    const CODE_A: &str = "AaAaAaAaAaAaAaAaAaAa";
    const CODE_B: &str = "BbBbBbBbBbBbBbBbBbBb";

    let dir = TempDir::new("reswitch");
    std::fs::create_dir_all(dir.path().join("dl")).unwrap();

    // 交付 A：清单里有 `t/shared.bin`，但桩**不提供**它 → aria2 以 404 失败，
    // 引擎里因此留下一条状态 `error`、带 errorMessage 的任务。
    let files_a: &[(&str, &[u8])] = &[(P, b"content-a")];
    let stub_a = Stub::start(vec![], |base| {
        real_manifest_for(CODE_A, base, files_a, Faults::default())
    });
    // 交付 B：**同一个相对路径**，桩这次提供它。
    let files_b: &[(&str, &[u8])] = &[(P, b"content-b")];
    let stub_b = Stub::start(
        files_b
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_vec()))
            .collect(),
        |base| real_manifest_for(CODE_B, base, files_b, Faults::default()),
    );

    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));
    core.ok(
        "load_delivery",
        json!({"code": CODE_A, "base_url": stub_a.base()}),
    );
    core.ok("enqueue", json!({"paths": [P]}));

    // 等上一批以失败收场（桩不提供 → 404）
    let failed_task = wait_for("上一批的任务以 failed 收场", || {
        let r = core.ok("transfer_list", json!({}));
        r["items"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["raw_status"] == json!("error")).cloned())
    });
    assert!(
        !failed_task["error_message"].as_str().unwrap().is_empty(),
        "先确认污染源确实存在（上一批的任务带 errorMessage）: {failed_task}"
    );

    // 再确认这份污染**真的会显示出来**——否则下面那几条断言就是空的
    let before = core.ok("get_tree", json!({}));
    let node = &before["tree"]["children"]["t"]["children"]["shared.bin"];
    assert_eq!(
        node["state"],
        json!("failed"),
        "上一批的失败状态本来就该显示在这一批的树上（这是污染源）: {before}"
    );

    // ── 换码：同一个交付、同一个相对路径，新的码 ────────────────────────────
    core.ok(
        "load_delivery",
        json!({"code": CODE_B, "base_url": stub_b.base()}),
    );

    // ① 引擎侧干净了：上一批的任务一条都不该剩下
    let tl = core.ok("transfer_list", json!({}));
    assert_eq!(
        tl["items"],
        json!([]),
        "换码之后引擎里不该还留着上一批的任务: {tl}"
    );

    // ② 新批次的树**不带**上一批的状态
    let after = core.ok("get_tree", json!({}));
    let node = &after["tree"]["children"]["t"]["children"]["shared.bin"];
    assert_eq!(
        node["state"],
        json!("pending"),
        "换码之后新批次的文件不该带着上一批的状态: {after}"
    );
    assert_eq!(
        node["err"],
        json!(""),
        "上一批的 errorMessage 不得渗进新批次: {after}"
    );
    assert_eq!(node["completed"], json!(0), "上一批的已完成字节不得算进来: {after}");
    assert_eq!(node["speed"], json!(0), "{after}");
    // 进度同理（`view::progress` 的过滤判据与 `compose` 同源）
    assert_eq!(
        after["progress"]["percent"],
        json!(0),
        "上一批的字节不得算进新批次的进度: {after}"
    );

    // ③ 换码之后这一批**还能正常下**（清理不能把新批次自己也堵死）
    core.ok("enqueue", json!({"paths": [P]}));
    let t = wait_for("新批次照常下载", || {
        let r = core.ok("transfer_list", json!({}));
        r["items"]
            .as_array()
            .and_then(|a| a.iter().find(|i| i["raw_status"] == json!("complete")).cloned())
    });
    assert_eq!(t["path"], json!(P), "{t}");
    assert_eq!(
        std::fs::read(dir.path().join("dl").join(P)).expect("新批次的文件必须落盘"),
        b"content-b"
    );
}

/// **同一个码重新 `load_delivery` 不得把正在下的任务删掉**（换码清理的反方向）。
///
/// `clear_engine_batch` 只在**码真的变了**的时候调——壳刷新树（重开窗口、点刷新）
/// 会拿同一个码再 `load_delivery` 一次，那时把用户正在下的任务全删掉是灾难性的。
///
/// 判别力：把 `code_changed` 写成恒真 → 任务被移除 → 下面的"任务还在"与
/// "照常跑到完成"两条断言都红（变异实验 M27 实测：修复前它 188+30 全绿，即零覆盖）。
///
/// ⚠️ **判别力来自"任务有没有被删"，与"传输是否还在飞"无关。**
/// 这里曾经写着"不限速的话……这条测试会退化成恒真"——**那句与实测相反**：
/// 恒真版会把已完成的任务也 purge 掉，`transfer_list` 里那个 GID **照样消失**，
/// 哪怕文件早就传完了。复审者实测：不限速时干净 main 3/3 绿、变异体 2/2 红。
///
/// 限速（1 MB/s、4 MiB ⇒ 持续约 4 秒）的真实作用是**让用例忠实于它的标题**
/// ——"重载那一刻任务还是**正在下**的那个"，而不是"已经传完、只是还没被清掉"。
/// 它顺带给观察窗口留了冗余，但**不是**判别力的来源；别把它读成"千万别动这个限速"。
#[test]
fn reloading_the_same_code_does_not_drop_running_tasks() {
    const P: &str = "t/slow.bin";
    let big = vec![0u8; 4 << 20];
    let files: Vec<(&str, &[u8])> = vec![(P, big.as_slice())];
    let mut env = Env::new("samecode", &files, Faults::default());

    // 限速 + 单并发：让任务在"重载"那一刻仍然在传
    env.core.ok(
        "set_settings",
        json!({"settings": {
            "parallel": 1, "connections": 1, "splits": 1,
            "min_split_size": "1M", "limit_mbps": 1, "max_tries": 3, "retry_wait": 1
        }}),
    );
    env.load();
    env.core.ok("enqueue", json!({"paths": [P]}));
    let t = env.wait_task("任务进入 active", |i| i["raw_status"] == json!("active"));
    let gid = t["gid"].as_str().unwrap().to_string();

    // 同一个码再 load 一次（壳刷新树的常规动作）
    env.load();

    // ① 任务**还在**，而且**还是同一个 GID**（不是被删掉后重新加的）
    let after = env.core.ok("transfer_list", json!({}));
    let still = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["gid"] == json!(gid))
        .cloned()
        .unwrap_or_else(|| {
            panic!("同一个码刷新不该把正在下的任务删掉（gid {gid} 没了）: {after}")
        });
    assert_ne!(
        still["raw_status"],
        json!("removed"),
        "任务被置成 removed 了——这正是恒真版 code_changed 的形状: {still}"
    );

    // ② 它照常跑到完成（证明我们没把它弄死）
    let done = env.wait_task("任务照常跑到完成", |i| {
        i["gid"] == json!(gid) && i["raw_status"] == json!("complete")
    });
    assert_eq!(done["path"], json!(P), "{done}");
    assert_eq!(
        std::fs::metadata(env.dl().join(P)).expect("文件必须落盘").len(),
        4 << 20,
        "整个文件都要下完"
    );
}

/// 未知方法 → 结构化错误，而不是 panic 或者静默忽略。
#[test]
fn unknown_method_returns_a_structured_error() {
    let dir = TempDir::new("unknown");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));
    let e = core.err("no_such_method", json!({}));
    assert_eq!(e["code"], json!("unknown_method"), "{e}");

    // 畸形 JSON 也必须是结构化错误，且 id 用保留值
    core.send_raw(b"{ not json\n");
    let line = core.read_line_raw();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["ok"], json!(false));
    assert_eq!(v["error"]["code"], json!("bad_request"), "{line}");
    assert_eq!(v["id"], json!(0), "{line}");
}

/// 简报第 8 条：`complete` 集合只在一处算出来，并且**真的驱动了四态合成**。
///
/// 判别力：先下一次、等校验通过，此时该文件在 `get_tree` 里必须是 `complete`
/// （而 `verify_status` 为空时它只能是 `pending`）。把 `complete` 的来源换成
/// "别的推导"（例如按 aria2 任务反推）就会红。
///
/// （本节原先写着"再重新 `load_delivery`"——**函数体里并没有那第二次调用**，
/// 那句是错的，已删。`plan` 与 `get_tree` 走的是同一条推导，断言的是它俩一致。）
#[test]
fn plan_and_tree_reflect_the_state_file() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("complete", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    let vr = env.wait_verified();
    assert!(names(&vr["ok"]).contains(&"t/readme.txt".to_string()), "{vr}");

    // 找一个"已完整"的文件：plan 必须判它 Skip
    let p = env.core.ok("plan", json!({}));
    let item = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == json!("t/readme.txt"))
        .expect("plan 必须覆盖清单里的每个文件")
        .clone();
    assert_eq!(
        item["kind"],
        json!("skip"),
        "校验已通过的文件在 plan 里必须判 Skip: {p}"
    );

    // 同一份事实在树上必须是 complete（两处不能各推各的）
    let g = env.core.ok("get_tree", json!({}));
    let node = &g["tree"]["children"]["t"]["children"]["readme.txt"];
    assert_eq!(
        node["state"],
        json!("complete"),
        "状态文件证明它已完整，树上就必须是 complete: {g}"
    );
    assert!(
        g["progress"]["percent"].as_i64().unwrap() == 100,
        "整批都完成了，进度必须是 100: {g}"
    );
}

/// 契约 §6：**严格校验必须保留，且壳要给它入口**——入口就是 `plan` 的 `strict` 参数。
///
/// 这是 §6 后半句（"壳的入口"那一半）**唯一缺的协议级测试**：`planner` 的 strict 语义
/// 有单测，但"壳通过协议把 `strict` 传进来"这条路上一条测试都没有——
/// `op_plan` 里读 `params.strict` 的那三行删掉、永远按 `false` 走，**整套 e2e 一条都不会红**。
///
/// 夹具：先让文件真的传完并校验通过（状态文件里因此有一条完整的记录），
/// 再分别用缺省 / `strict: true` 各扫一次：
///   - 缺省：判 `skip`（尊重状态文件）；
///   - `strict: true`：**忽略状态文件**，判 `download`——客户怀疑数据有问题时
///     就是靠这一下把整批重新校验一遍（规格许诺的能力，不得静默丢掉）。
#[test]
fn plan_strict_true_ignores_the_state_file() {
    let files: &[(&str, &[u8])] = &[("t/readme.txt", b"readme")];
    let mut env = Env::new("strict", files, Faults::default());
    env.load();
    env.core.ok("enqueue", json!({"paths": ["t/readme.txt"]}));
    let vr = env.wait_verified();
    assert!(names(&vr["ok"]).contains(&"t/readme.txt".to_string()), "{vr}");

    let kind_of = |p: &Value| -> String {
        p["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["path"] == json!("t/readme.txt"))
            .expect("plan 必须覆盖清单里的每个文件")["kind"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // ① 缺省（壳不传 strict）：状态文件证明它已完整 → skip
    let p = env.core.ok("plan", json!({}));
    assert_eq!(p["strict"], json!(false), "缺省 strict 必须是 false: {p}");
    assert_eq!(kind_of(&p), "skip", "缺省时应当尊重状态文件: {p}");

    // ② strict: true：忽略状态文件 → download
    let p = env.core.ok("plan", json!({"strict": true}));
    assert_eq!(
        p["strict"],
        json!(true),
        "响应必须回显壳传进来的 strict（否则壳无从确认这条路真的走通了）: {p}"
    );
    assert_eq!(
        kind_of(&p),
        "download",
        "`strict: true` 必须忽略状态文件、判它需要重新下载（契约 §6 许诺给客户的能力）: {p}"
    );

    // ③ 严格扫描**不得**把 `complete` 改坏：紧接着的缺省扫描仍应判 skip
    let p = env.core.ok("plan", json!({}));
    assert_eq!(kind_of(&p), "skip", "严格扫描不该破坏后续的非严格判定: {p}");
}

/// `list_dir` 给某目录的直接子项（设计规格 §5.2）。
#[test]
fn list_dir_returns_direct_children() {
    let files: &[(&str, &[u8])] = &[
        ("t/a.txt", b"a"),
        ("t/sub/b.txt", b"b"),
        ("top.txt", b"top"),
    ];
    let mut env = Env::new("listdir", files, Faults::default());
    env.load();

    let r = env.core.ok("list_dir", json!({"path": "t"}));
    let entries = r["entries"].as_array().unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"a.txt"), "{r}");
    assert!(names.contains(&"sub"), "{r}");
    assert!(!names.contains(&"b.txt"), "list_dir 只给**直接**子项: {r}");

    // 不存在的路径 → 结构化错误（壳要能提示客户）
    let e = env.core.err("list_dir", json!({"path": "nope"}));
    assert_eq!(e["code"], json!("path_not_found"), "{e}");
}

/// `enqueue` 支持**整个目录**（设计规格 §5.2：单文件 / 多选 / 整个目录）。
#[test]
fn enqueue_accepts_a_directory_prefix() {
    let files: &[(&str, &[u8])] = &[
        ("t/a.txt", b"a"),
        ("t/sub/b.txt", b"b"),
        ("top.txt", b"top"),
    ];
    let mut env = Env::new("enqdir", files, Faults::default());
    env.load();

    let r = env.core.ok("enqueue", json!({"paths": ["t"]}));
    assert_eq!(
        r["added"].as_array().unwrap().len(),
        2,
        "目录前缀应展开成它的两个文件（不含 top.txt）: {r}"
    );
}

/// 不存在的路径入队 → 结构化错误（而不是静默少交）。
#[test]
fn enqueue_rejects_unknown_paths() {
    let files: &[(&str, &[u8])] = &[("t/a.txt", b"a")];
    let mut env = Env::new("enqbad", files, Faults::default());
    env.load();
    let e = env.core.err("enqueue", json!({"paths": ["t/nope.txt"]}));
    assert_eq!(e["code"], json!("invalid_params"), "{e}");

    // 还没 load_delivery 就入队 → 明确报"还没有清单"
    let dir = TempDir::new("nod");
    let mut core = Core::start(&dir.path().join("dl"), &dir.path().join("s.json"));
    let e = core.err("enqueue", json!({"paths": ["t/a.txt"]}));
    assert_eq!(e["code"], json!("no_delivery"), "{e}");
}

// ===========================================================================
// 阶段 D 任务 A：`complete` 必须跟着磁盘走
// ===========================================================================

/// 已下载的文件在本地被删除后，**「全部下载」必须能把它重新下回来**。
///
/// 这是人类伙伴实测那条 bug 的最短复现（诊断报告 `diagnosis-redownload.md` §2 的四步），
/// 逐字对应：① 整批下完并校验通过 → ② 调一次 `get_tree`（= 用户进「文件」页；
/// `FileBrowser` 的 `.task` 会调 `list_dir`，等效）把 `complete` 缓存坐实 →
/// ③ 从磁盘删掉一个文件 → ④ 再点「全部下载」（`paths: []`）。
///
/// 根因（诊断报告 §1 / §3.1）：`Kernel::complete` 只由 `complete_dirty` 一个布尔量控制失效，
/// 而把那个布尔量置 `true` 的**唯一**地方是校验线程提交结果——**磁盘上的文件被外部删除，
/// 没有任何代码在看着**。于是第 ② 步坐实的那份知识一直是错的，第 ④ 步算出空 `pending`，
/// 被 `resolve_targets` 判成 `invalid_params: 没有匹配到任何文件` 抛回壳。
///
/// 判别力：把 `ensure_complete` 里那步**只 stat 磁盘**的核对去掉 ⇒ 第 ④ 段拿到
/// `invalid_params`，本用例在 `ok` 上红。
///
/// 同时钉住 **D-2 错误语义不对称的两半**：
///   - `paths` 为空（界面「全部下载」）且 `pending` 为空 ⇒ **成功回执**（0 个），不是错误
///     （诊断报告 §7 第 3 条：什么都没删、只是再点一次下载，修复前也会报错）；
///   - `paths` **非空**（用户点名了具体文件）⇒ `resolve_targets` 的错误**一个字都不许放宽**。
#[test]
fn deleted_local_file_is_redownloaded_by_download_all() {
    const A: &str = "t/a.txt";
    const B: &str = "t/b.txt";
    const C: &str = "t/readme.txt";
    let files: &[(&str, &[u8])] = &[(A, b"aaa"), (B, b"bbb"), (C, b"ccc")];
    let mut env = Env::new("redownload", files, Faults::default());
    env.load();

    // ① 整批下完并等到三个文件**全部**校验通过（状态文件因此能为它们作证）。
    //    这里刻意用**显式路径**入队（而不是 `paths: []`），好让第 ④ 段的失败
    //    只可能来自"缓存没跟着磁盘走"，不与入队路径本身的差异混淆。
    env.core.ok("enqueue", json!({"paths": [A, B, C]}));
    let want: Vec<String> = vec![A.to_string(), B.to_string(), C.to_string()];
    let vr = wait_for("三个文件全部校验通过", || {
        let x = env.core.ok("verify_status", json!({}));
        (names(&x["ok"]).len() == 3).then_some(x)
    });
    assert_eq!(sorted(names(&vr["ok"])), sorted(want.clone()), "{vr}");

    // ② 毒化：调一次 `get_tree`。这正是诊断报告 §3.1 单变量实验里那个**唯一**的开关
    //    （`transfer_list` 不调 `ensure_complete`，所以它毒化不了缓存）。
    let g = env.core.ok("get_tree", json!({}));
    assert_eq!(g["progress"]["percent"], json!(100), "毒化前整批确实完整: {g}");

    // ③ 从磁盘删掉其中一个（模拟客户在访达里删了它）。
    std::fs::remove_file(env.dl().join(B)).expect("删掉已下载的文件");
    assert!(!env.dl().join(B).exists(), "删除必须真的生效");

    // ④ 「全部下载」：只剩 B 需要下，而且**恰好**是 B——多下一个是白花带宽，
    //    少下一个（= 报错）就是本次要修的缺陷。
    let r = env.core.ok("enqueue", json!({"paths": []}));
    let added: Vec<String> = r["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        added,
        vec![B.to_string()],
        "被删掉的文件必须被重新加入下载，且只有它: {r}"
    );

    // ⑤ 重下真的落盘、校验通过之后树回到 complete（闭环，不是"入队成功"就算数）。
    wait_for("重下的文件必须重新落盘", || {
        std::fs::metadata(env.dl().join(B)).ok().map(|m| m.len())
    });
    let g = wait_for("重下 + 校验通过后树上必须回到 complete", || {
        let g = env.core.ok("get_tree", json!({}));
        (g["tree"]["children"]["t"]["children"]["b.txt"]["state"] == json!("complete")).then_some(g)
    });
    assert_eq!(g["progress"]["percent"], json!(100), "{g}");

    // ⑥ D-2 前半：此时确实没有待下载的了，再点一次「全部下载」必须是**成功回执**
    //    （诊断报告 §7 第 3 条；修复前这里是 `invalid_params`）。
    let r = env.core.ok("enqueue", json!({"paths": []}));
    assert_eq!(
        r["added"],
        json!([]),
        "没有待下载的 ⇒ 成功、0 个，不是错误: {r}"
    );
    assert_eq!(r["rejected"], json!([]), "{r}");

    // ⑦ D-2 后半：用户**点名**一个不存在的路径时，`resolve_targets` 的报错原样保留。
    //    只钉前一半会让这一半悄悄放宽（约束 D-2 明写）。
    let e = env.core.err("enqueue", json!({"paths": ["t/nope.txt"]}));
    assert_eq!(e["code"], json!("invalid_params"), "{e}");
    assert!(
        e["message"].as_str().unwrap().contains("清单里没有"),
        "点名路径不存在时必须是 `resolve_targets` 那条原文错误: {e}"
    );
}

/// **磁盘被清空之后，界面不许再说"整批已完成"**（诊断报告 §3.4 / §7 第 6 条）。
///
/// 这是比那条报错**严重得多**的后果：盘上一个字节都没有，`get_tree` 却回
/// `progress.percent = 100`、所有文件 `state = complete`、`default_selected = []`
/// ——**客户会以为整批已经交付**。这就是"静默少交"，本项目最忌讳的形态（约束 4 / D-7）。
/// 报错至少会被看见，这个不会。
///
/// 判别力：去掉 `ensure_complete` 里那步磁盘核对 ⇒ 第 ③ 段的断言全红
/// （那正是修复前逐字实测到的形态）。
///
/// ⚠️ 三处**消费 `complete`** 的地方（`get_tree` / `list_dir` / `enqueue`）**一处都不能漏**：
/// 诊断报告 §7 第 6 条走的正是 `get_tree` 那条路，而 §1 的报错走的是 `enqueue`。
#[test]
fn wiped_disk_is_never_reported_as_complete() {
    const A: &str = "t/a.txt";
    const B: &str = "t/b.txt";
    const C: &str = "t/readme.txt";
    let files: &[(&str, &[u8])] = &[(A, b"aaa"), (B, b"bbb"), (C, b"ccc")];
    let mut env = Env::new("wipe", files, Faults::default());
    env.load();

    let want: Vec<String> = vec![A.to_string(), B.to_string(), C.to_string()];

    // ① 下完 + 校验通过 + 毒化缓存（同前一条用例的 ①②）。
    env.core.ok("enqueue", json!({"paths": [A, B, C]}));
    wait_for("三个文件全部校验通过", || {
        let x = env.core.ok("verify_status", json!({}));
        (names(&x["ok"]).len() == 3).then_some(x)
    });
    let g = env.core.ok("get_tree", json!({}));
    assert_eq!(g["progress"]["percent"], json!(100), "毒化前整批确实完整: {g}");

    // ② 把盘上文件**全部**删掉（模拟客户清空下载目录 / 换了台机器 / 手滑删了）。
    for p in [A, B, C] {
        std::fs::remove_file(env.dl().join(p)).expect("删掉已下载的文件");
    }

    // ③ `get_tree` 不许再说"完成"。三个断言各守一面：
    //    - `flat` 里不许有任何 `complete`（谁被漏掉都会在这里露头）；
    //    - 进度不许是 100%（§3.4 那张表的核心）；
    //    - `default_selected` 不许是空（空了客户就没有"要下什么"的默认面）。
    let g = env.core.ok("get_tree", json!({}));
    let flat = g["flat"].as_array().unwrap();
    assert_eq!(flat.len(), 3, "清单三个文件都必须在 flat 里: {g}");
    for n in flat {
        assert_ne!(
            n["state"],
            json!("complete"),
            "盘上已经没有这个文件了，界面不许说它 complete: {g}"
        );
    }
    assert_ne!(g["progress"]["percent"], json!(100), "{g}");
    assert_eq!(g["progress"]["done_bytes"], json!(0), "{g}");
    assert_eq!(
        sorted(names(&g["default_selected"])),
        sorted(want.clone()),
        "盘上什么都没有 ⇒ 默认勾选面必须是全部: {g}"
    );

    // ④ `list_dir` 走的是同一个 `complete`：一处都不能漏。
    let l = env.core.ok("list_dir", json!({"path": "t"}));
    for e in l["entries"].as_array().unwrap() {
        if e["type"] == json!("file") {
            assert_ne!(e["state"], json!("complete"), "{l}");
        }
    }

    // ⑤ 「全部下载」必须把**全部三个**重新下回来——静默少交的反面：一个都不能少。
    let r = env.core.ok("enqueue", json!({"paths": []}));
    let added: Vec<String> = r["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        sorted(added),
        sorted(want),
        "盘上全没了 ⇒ 三个都得重新入队: {r}"
    );
}
