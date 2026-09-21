//! 测试脚手架（**只在测试构建里存在**）。
//!
//! 门控在**调用方**那一行：`main.rs` 的 `#[cfg(test)] mod testutil;`——
//! 它让这个模块在 `cargo build` 的构建里**整个消失**，因此不会给
//! "分批接线期 dead_code 告警属预期"（全局约束 13）的计数添任何一条。
//!
//! ⚠️ 这里曾经**同时**写着 `#![cfg(test)]`（两道门控），clippy 的
//! `duplicated_attributes` 在 `--all-targets` 门禁下把它判为冗余。已去掉内层那一道：
//! 门控只留 `main.rs` 那一处，**行为完全不变**（非测试构建里这个模块依然不存在）。
//!
//! 这个文件是 `crc64xz.rs` 与 `settings.rs` 里两份逐字相同的 30 行 `TempDir`
//! 的收敛结果（`state` 是第三份拷贝），见任务 4 步骤 5b。
//!
//! 第二件收敛物是 `StubHttp`（见文件末），它原先是 `planner.rs` 测试模块里的一份私有桩，
//! 任务 9 把它提上来，见 Ruling #56。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 测试用临时目录：不引入额外依赖，`Drop` 时删除。
///
/// 名字里带 pid 与自增序号，避免并行跑的测试互相踩。
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("benagen-core-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&p).expect("创建临时目录失败");
        TempDir(p)
    }

    /// 目录本身。`state::load`/`state::save` 要的是**目录**而不是目录下的某个文件，
    /// 这是收敛时在三份副本之外新增的唯一一个方法（两份旧副本只有 `new`/`join`）。
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// HTTP 桩服务
// ---------------------------------------------------------------------------

/// 一条桩响应：状态码 + 附加响应头 + 响应体。
///
/// 头名/头值取 `&'static str`（桩只发静态头），响应体取 `String`——
/// 任务 9 的 RPC 桩要**按请求现算** JSON 体，静态 `&str` 表达不了。
///
/// ⚠️ 不要自己塞 `Content-Length`：`write_response` 会按 `body` 的**字节**长度补一条，
/// 重复的头会让客户端行为未定义。
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: String,
}

impl RawResponse {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    pub fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
}

/// 一条**已收到**的请求。`body` 是原始请求体（逐字，未解析、未校验编码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenRequest {
    pub method: String,
    pub path: String,
    pub body: String,
}

/// 极简 HTTP/1.1 桩服务：只够测试用，**不引入任何依赖**。
///
/// 为什么不用简报建议的 `wiremock`：它是纯异步 API，驱动它必须先有一个 tokio 运行时；
/// 而 tokio 不在本项目的依赖表里——要用它就得往 `Cargo.toml` 加一行 dev-dep，那是任务 6
/// 明令禁止的（"不要新增任何依赖"，且审查方把"零新依赖"当验收项核对）。
/// 计划正文自己也写了两种桩：「`wiremock` 或裸 `TcpListener`」，这里取后者。
///
/// 它必须是**真的 HTTP 服务**，而不是 `delivery.rs` 里那种传输层桩：测试的全部意义
/// 就是让错误串的生产端与解析端被**同一个真实响应**钉在一起
/// （任务 2 审查 D21 要防的「两边同时改掉前缀」这类协调性改动）。
///
/// 两种用法，共用同一条收发路径：
///   - `start(script)`：按脚本逐条回，用完重复最后一条（`planner.rs` 用它）；
///   - `start_with(handler)`：每个请求现算一条响应（`engine::rpc` 用它——
///     "token 不对就回 400"这类**取决于请求**的响应，脚本表达不了）。
///
/// 两种用法都：每个连接循环服务到对端关闭/读超时为止（agent 自带连接池，会复用连接），
/// 并把**每一条**请求记进 `requests()` 供断言。
pub struct StubHttp {
    port: u16,
    seen: std::sync::Arc<std::sync::Mutex<Vec<SeenRequest>>>,
}

impl StubHttp {
    /// 脚本化：`script` 逐条给出响应，用完最后一条后**重复最后一条**
    /// （与 `delivery.rs` 测试里的 `Stub` 同义）。
    pub fn start(script: Vec<RawResponse>) -> Self {
        assert!(!script.is_empty(), "桩至少要有一条响应");
        let served = std::sync::Mutex::new(0usize);
        Self::start_with(move |_req| {
            let mut n = served.lock().unwrap();
            let resp = script[(*n).min(script.len() - 1)].clone();
            *n += 1;
            resp
        })
    }

    /// 动态：每个请求交给 `handler` 现算响应。`handler` 在桩线程里跑，
    /// panic 会让连接被重置（测试失败，但不是静默通过）。
    pub fn start_with<F>(handler: F) -> Self
    where
        F: Fn(&SeenRequest) -> RawResponse + Send + 'static,
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定桩端口失败");
        let port = listener.local_addr().expect("取桩端口失败").port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_in_thread = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                while let Some(req) = read_request(&mut s) {
                    // 先记账再回响应：客户端拿到响应的那一刻，请求必定已经在 `requests()` 里。
                    seen_in_thread.lock().unwrap().push(req.clone());
                    if write_response(&mut s, &handler(&req)).is_err() {
                        break;
                    }
                }
            }
        });
        Self { port, seen }
    }

    /// 桩服务的基址（对应 Go 的 `srv.URL`）。
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 已收到的请求 `(方法, 路径)`——只关心这两者的调用方用这个简写。
    pub fn seen(&self) -> Vec<(String, String)> {
        self.requests()
            .into_iter()
            .map(|r| (r.method, r.path))
            .collect()
    }

    /// 已收到的全部请求（含**原始请求体**），按到达顺序。
    pub fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

/// 读到请求头结束（`\r\n\r\n`）+ 完整的请求体为止；对端关闭或读超时返回 `None`。
///
/// 请求体按 `Content-Length` 读——**必须读**：POST 的 JSON 体与下一个请求的头部
/// 在同一个 TCP 流上，留下不读的字节会被当成下一个请求的起始行。
fn read_request(s: &mut std::net::TcpStream) -> Option<SeenRequest> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .ok()?;
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        match s.read(&mut tmp) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return None, // 读超时 / 连接被断开
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or("").split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // 头名大小写不敏感（HTTP 本就如此），逐个找。
    let content_length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);

    // 与头一起到达的那部分体已经躺在 `buf` 里，别丢。
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        match s.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
            Err(_) => break,
        }
    }
    body.truncate(content_length);

    Some(SeenRequest {
        method,
        path,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// 写一条响应。`Content-Length` 按**字节**数补（响应体可能是多字节的中文）。
fn write_response(s: &mut std::net::TcpStream, resp: &RawResponse) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", resp.body.len()));
    s.write_all(out.as_bytes())?;
    s.write_all(resp.body.as_bytes())?;
    s.flush()
}

/// 状态行里的原因短语（ureq 只用状态码，这里写全是为了这是一份像样的 HTTP 响应）。
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        410 => "Gone",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
