//! `benagen-core` 入口 —— **只剩 stdio 协议循环**。
//!
//! 编排（`Kernel` + `op_*` + 两个后台 worker）已整段搬进库的 `kernel.rs`（见它的文件头），
//! 与 `benagen-dl` 共用同一份。本文件只做三件事：解析 argv、把 stdin 的每一行交给
//! `kernel::dispatch`、把响应写回 stdout。
//!
//! ⚠️ **stdout 是协议专用通道**（全局约束 1）：每行一条 JSON，除此之外不写 stdout，
//! 诊断一律 `eprintln!`（stderr）——`--help` 也走 stderr，理由见 `parse_args`。
//!
//! # 命令行参数（阶段 A 新增，设计规格未规定）
//!
//! ```text
//! benagen-core [--download-dir <DIR>] [--settings <PATH>]
//! ```
//!
//! 壳必须能决定"文件下到哪"与"设置存哪"，否则它无法实现设计规格 §9 的
//! 「目标目录不可写 / 磁盘不足 → 开工前检查」。两者都是**内核的输入**而不是协议消息，
//! 所以走 argv（结构化传参，不经 shell）。

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};

use benagen_core::kernel::{
    dispatch, read_line_capped, spawn_landing_watcher, spawn_verify_worker, write_line, Kernel,
    LineOutcome, VerifyJob,
};
use benagen_core::protocol;
use benagen_core::protocol::{codes, Response, UNCORRELATED_ID};
use benagen_core::settings;

// ---------------------------------------------------------------------------
// 命令行参数
// ---------------------------------------------------------------------------

struct Args {
    download_dir: PathBuf,
    settings_path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut download_dir: Option<PathBuf> = None;
    let mut settings_path: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> Result<PathBuf, String> {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} 后面缺少路径"))
        };
        match a.as_str() {
            "--download-dir" => download_dir = Some(take("--download-dir")?),
            "--settings" => settings_path = Some(take("--settings")?),
            "-h" | "--help" => {
                // ⚠️ **stderr 不是审美选择**：stdout 是协议专用通道（全局约束 1），
                // 而 `--help` 是诊断信息、不是协议消息。写 stdout 会让这一行被壳
                // 当成一条响应去解析（它没有 id，配不上任何请求）。
                eprintln!("用法: benagen-core [--download-dir <DIR>] [--settings <PATH>]");
                std::process::exit(0);
            }
            other => return Err(format!("不认识的参数 {other:?}")),
        }
    }
    let download_dir = match download_dir {
        Some(p) => p,
        None => match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => Path::new(&h).join("Downloads").join("Benagen"),
            _ => PathBuf::from("benagen-downloads"),
        },
    };
    Ok(Args {
        download_dir,
        settings_path: settings_path.unwrap_or_else(settings::default_path),
    })
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("benagen-core: {e}");
            std::process::exit(2);
        }
    };

    let kernel = Arc::new(Mutex::new(Kernel::new(args.download_dir, args.settings_path)));
    let (tx, rx) = mpsc::channel::<VerifyJob>();
    spawn_verify_worker(Arc::clone(&kernel), rx, tx.clone());
    spawn_landing_watcher(Arc::clone(&kernel), tx.clone());

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        match read_line_capped(&mut reader) {
            Ok(LineOutcome::Eof) => break, // stdin 断开 → 正常收尾
            Ok(LineOutcome::TooLong) => {
                // 超长行：结构化报错。**不是** EOF，连接继续（见 MAX_LINE_BYTES）。
                let r = Response::err(
                    UNCORRELATED_ID,
                    codes::BAD_REQUEST,
                    "请求行超过 8 MiB 上限",
                );
                if write_line(&mut out, &r).is_err() {
                    break;
                }
            }
            Ok(LineOutcome::Line(bytes)) => {
                // ⚠️ **解码失败与 EOF 必须走不同的分支**（简报第 13 条）：
                // `read_request` 的签名是 `&str`，非法字节进不去，所以在这里就得判掉。
                // 把它当成 EOF 静默丢连接，壳会看到"内核没反应"——那是最难查的一类故障。
                let Ok(line) = std::str::from_utf8(&bytes) else {
                    let r = Response::err(
                        UNCORRELATED_ID,
                        codes::BAD_REQUEST,
                        "请求行不是合法的 UTF-8",
                    );
                    if write_line(&mut out, &r).is_err() {
                        break;
                    }
                    continue;
                };
                let (resp, stop) = match protocol::read_request(line) {
                    Ok(Some(req)) => dispatch(&kernel, &req),
                    Ok(None) => continue, // 空行：跳过，不回任何东西
                    // 简报第 12 条：**能配对就配对**。`{"id":5}` 这类"id 读得出来、
                    // 其余字段坏了"的输入必须回 5，否则壳那条请求永远等不到配对响应。
                    Err(e) => (
                        Response::err(e.responder_id(), &e.body.code, &e.body.message),
                        false,
                    ),
                };
                if write_line(&mut out, &resp).is_err() {
                    break;
                }
                if stop {
                    break;
                }
            }
            Err(e) => {
                // 真的读不了（管道坏了之类）：不是协议错误，收尾就是正确处理。
                eprintln!("benagen-core: 读取请求失败，收尾退出: {e}");
                break;
            }
        }
    }

    // 收尾：关掉 aria2 并等它真的被回收（三层里任何一层崩溃，下游都要能被回收）。
    // 不持锁做这件事——`close` 最坏要等满 5 秒的兜底。
    let daemon = {
        let mut k = kernel.lock().unwrap_or_else(PoisonError::into_inner);
        k.take_daemon()
    };
    if let Some(d) = daemon {
        if let Err(e) = d.close() {
            eprintln!("benagen-core: 关闭下载引擎时出错: {e}");
        }
    }
    let _ = out.flush();
}
