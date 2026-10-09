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
//! benagen-core [--download-dir <DIR>] [--settings <PATH>] [--log-level <normal|verbose>]
//! ```
//!
//! ⚠️ 这一串与 `parse_args()` 里 `--help` 打的那一串是**两份**，改一处要一起改。
//!
//! 壳必须能决定"文件下到哪"与"设置存哪"，否则它无法实现设计规格 §9 的
//! 「目标目录不可写 / 磁盘不足 → 开工前检查」。两者都是**内核的输入**而不是协议消息，
//! 所以走 argv（结构化传参，不经 shell）。

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};

use benagen_core::diagnostics;
use benagen_core::kernel::{
    dispatch, read_line_capped, spawn_crc_filler, spawn_landing_watcher, spawn_recovery,
    spawn_update_check, spawn_verify_worker, write_line, Kernel, LineOutcome, VerifyJob,
};
use benagen_core::paths;
use benagen_core::protocol;
use benagen_core::protocol::{codes, Response, UNCORRELATED_ID};
use benagen_core::settings;

// ---------------------------------------------------------------------------
// 命令行参数
// ---------------------------------------------------------------------------

struct Args {
    download_dir: PathBuf,
    settings_path: PathBuf,
    log_level: diagnostics::Level,
}

fn parse_args() -> Result<Args, String> {
    let mut download_dir: Option<PathBuf> = None;
    let mut settings_path: Option<PathBuf> = None;
    let mut log_level: Option<diagnostics::Level> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{name} 后面缺少取值"))
        };
        match a.as_str() {
            "--download-dir" => download_dir = Some(PathBuf::from(take("--download-dir")?)),
            "--settings" => settings_path = Some(PathBuf::from(take("--settings")?)),
            "--log-level" => log_level = Some(diagnostics::parse_level(&take("--log-level")?)?),
            "-h" | "--help" => {
                // ⚠️ **stderr 不是审美选择**：stdout 是协议专用通道（全局约束 1），
                // 而 `--help` 是诊断信息、不是协议消息。写 stdout 会让这一行被壳
                // 当成一条响应去解析（它没有 id，配不上任何请求）。
                eprintln!(
                    "用法: benagen-core [--download-dir <DIR>] [--settings <PATH>] \
                     [--log-level <normal|verbose>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("不认识的参数 {other:?}")),
        }
    }
    // 两个默认值都按**平台标准目录**取（规格 §8 的表，判据集中在 `paths`）：
    // macOS 上 `$HOME` 取不到回退相对路径（冻结行为），Windows 上 `%APPDATA%`/`%USERPROFILE%`
    // 取不到就**大声失败**——那时 `?` 会把错误交给 `main()`：诊断进 stderr、退出码 2。
    // 这两条默认值**不是**给壳用的常用路径（壳总是显式传这两个开关），但内核不能因为
    // "反正壳会传"就悄悄降级。
    let download_dir = match download_dir {
        Some(p) => p,
        None => paths::download_dir()?,
    };
    let settings_path = match settings_path {
        Some(p) => p,
        None => settings::default_path()?,
    };
    // 缺省就是 normal（`--log-level` 不出现时**什么都不用做**）。
    let log_level = log_level.unwrap_or(diagnostics::Level::Normal);
    Ok(Args {
        download_dir,
        settings_path,
        log_level,
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

    // ⚠️ **必须早于任何一次 log**：`spawn_*` 那几条线程一跑起来就会记东西，
    //    级别落晚了它们的头几行会按 normal 的规矩走（而那不是我们以为的那一档）。
    diagnostics::init(args.log_level);

    let kernel = Arc::new(Mutex::new(Kernel::new(args.download_dir, args.settings_path)));
    let (tx, rx) = mpsc::channel::<VerifyJob>();
    spawn_verify_worker(Arc::clone(&kernel), rx, tx.clone());
    spawn_landing_watcher(Arc::clone(&kernel), tx.clone());
    spawn_recovery(Arc::clone(&kernel));
    // **后台分批补齐 crc64**（2026-10-08）：清单里空着的 crc64 由它一小批一小批地补，
    // 而不是让某一个命令替整批文件挡在那里（那会把壳的主线程堵住 ⇒ 窗口「未响应」）。
    // 细节与"为什么必须有一条后台线程"见它的文档。
    spawn_crc_filler(Arc::clone(&kernel));
    // **后台查有没有新版本**（规格 §3）：启动即可能查一次，之后每 24 小时一次。
    // 只提示、不下载 —— 结果落在 `update.json`，经 `update_status` 那条只读命令给壳。
    spawn_update_check(Arc::clone(&kernel));

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
