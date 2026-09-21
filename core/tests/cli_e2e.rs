//! 真 CLI + 真 aria2c + 桩 HTTP 服务。
//!
//! 判据是**退出码**与**落盘逐字一致** —— 这是"CLI 真能交付"的唯一自动判据。
//!
//! ⚠️ 夹具必须由仓库根的**真实生成器**产出（`delivery_manifest.py`），不许手写清单（契约 §3.1）。
//!    python3 取不到时**直接失败**，不静默跳过 —— 静默跳过的绿灯比红灯危险。
//! ⚠️ 每个用例都给子进程一个**临时 HOME**：CLI 按 `$HOME` 找 `settings.json`，
//!    不隔开就会去读开发者自己那份，断言会随人而变。
//! ⚠️ 落盘布局的判据**以现有 e2e 为准**
//!    （`macos/Tests/BenagenCoreKitTests/EndToEndTests.swift` 那条已在跑的真链路）：
//!    `-o` 给出的目录 + 清单 `path` 原文，**不带随机码那一段**。
//!
//! # 本文件里三处"不这么写就会变成假绿"的地方
//!
//! ## R28：同一个下载目录、同一个交付码，**连跑两次**
//!
//! T6 花了五轮修的就是"续传能不能收敛"（`cli::converged` / `reconfirm_verdict`：
//! `ok` 只装**本次真的校验过**的，而 `enqueue {paths: []}` 只入队**待下载**的，
//! 已完整的被跳过 —— 少了"开跑前就完整"那一项，判据永不成立、工具**永久挂住**）。
//! 那条判据长在轮询循环里，单测进不去，**只有这里跑得到**：
//! 第一次全下完并校验过 ⇒ 第二次 `enqueue` 跳过已完整的 ⇒
//! `all_good && unverifiable + complete >= 总数` 才成立 ⇒ 退出码 0。
//! 第二次**必须**退出 0，且**不能**因为"没有新任务"就挂住（挂住 ⇒ 超时 panic）。
//!
//! ## R29：每次用例结束都断言"没有本次运行的 aria2c 残留"
//!
//! T6 有一条具名风险是"漏关引擎 ⇒ 留下孤儿 aria2c 在客户机器上跑"。它**只有真跑**才验得到。
//! 本机（乃至本测试进程）可能有别人的 aria2c，所以过滤器是**本用例那个唯一的临时根目录**
//! （引擎的 argv 里必然有 `--dir=<下载目录>`，见 `engine::daemon::launch_args`），
//! 再叠一条**本机总数跑前 == 跑后**。四个用例之间持一把串行锁，这条计数才谈得上确定。
//!
//! ⚠️ 这条断言**必须配一个前置守卫**（`peak_aria2c > 0`）：过滤器一旦失灵
//! （比如匹配不上 `aria2c-<摘要>` 这种进程名），"没有孤儿"会变成**一条永远为真的绿灯**。
//!
//! ## 桩服务的输出**落文件**，不走管道
//!
//! `python3 -m http.server` 每处理一个请求就往 `stderr` 写一行访问日志。
//! 若那一头是**管道**、而读端被丢掉（`start()` 返回就丢），下一个请求的
//! `log_request()` 会先于响应头写出去并撞上 `EPIPE` —— 客户端看到的是**连接被掐断**，
//! 症状是"清单/文件下不下来"，与本测试想验的东西毫无关系。
//! （同一个坑在 Swift 侧是 `StubServer` 的日志文件；这里照做，顺带留下请求痕迹当证据。）

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

const CODE: &str = "CliE2ETest0000000000"; // 20 位，合法码字符
const FIXTURE: &[(&str, &str)] = &[
    ("Readme.txt", "hello\n"),
    ("C24-8_×_25WS024/Figure/QC 图.png", "PNGDATA"),
    ("C24-8_×_25WS024/Figure/deep/a.txt", "deep"),
];

/// 一次下载的墙钟上界。夹具只有 17 字节、桩在本机，正常几秒；
/// 给足余量是为了"真卡住"与"机器慢"能分开 —— 前者必须红，后者不该红。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// 负例（不发下载请求）的上界。
const NEGATIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// 采样"本用例的 aria2c 还在不在"的间隔。
const SAMPLE_EVERY: Duration = Duration::from_millis(20);

/// 四个用例**串行**跑。
///
/// 不是为了省事：R29 的"本机 aria2c 总数跑前 == 跑后"在并行下会被兄弟用例的引擎
/// 污染成假红（本文件里每个用例都会起一次引擎）。持锁之后这条计数才有确定含义。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 仓库根 = `core/` 的上一层。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core/ 必须有父目录（仓库根）")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// 夹具（真生成器）
// ---------------------------------------------------------------------------

/// 夹具生成：**导入仓库根的真实生成器**，crc64 对真实字节现算（不抄常量）。
/// 自检照抄 Swift e2e 的已知向量 —— 算错就当场失败，不许安静产出一份 crc64 全错的清单。
fn make_fixture(stub_root: &Path, base_url: &str) {
    let script = r#"
import base64, json, os, sys
sys.path.insert(0, sys.argv[1])
from delivery_manifest import build_manifest, dump_manifest

def crc64xz(data):
    crc = 0xFFFFFFFFFFFFFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ (0xC96C5795D7870F42 if crc & 1 else 0)
    return crc ^ 0xFFFFFFFFFFFFFFFF

assert crc64xz(b"123456789") == 0x995DC9BBDF1939FA, "crc64xz 与标准校验值不符"

spec = json.load(sys.stdin)
code_dir = os.path.join(spec["stub_root"], spec["code"])
os.makedirs(code_dir, exist_ok=True)
entries = []
for f in spec["files"]:
    data = base64.b64decode(f["data_b64"])
    dest = os.path.join(code_dir, f["path"])
    parent = os.path.dirname(dest)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(dest, "wb") as fh:
        fh.write(data)
    entries.append((f["path"], len(data), str(crc64xz(data))))

dump_manifest(build_manifest(spec["code"], entries, spec["base_url"]), code_dir)
"#;

    let payload = serde_json::json!({
        "code": CODE,
        "base_url": base_url,
        "stub_root": stub_root.to_str().unwrap(),
        "files": FIXTURE.iter().map(|(p, body)| serde_json::json!({
            "path": p,
            "data_b64": base64_encode(body.as_bytes()),
        })).collect::<Vec<_>>(),
    });

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(repo_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起不动 python3 —— 本测试依赖它，不静默跳过");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("写 python3 stdin 失败");
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "delivery_manifest.build_manifest 失败（不静默降级成手写清单）：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 自检：清单确实来自那条生成路径，且**路径逐字保留**（不是这里自己拼的 JSON）。
    let manifest_path = stub_root.join(CODE).join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("生成器没有写出 {}: {e}", manifest_path.display()));
    let v: serde_json::Value = serde_json::from_str(&raw).expect("清单必须是合法 JSON");
    assert_eq!(v["code"], serde_json::json!(CODE), "清单的 code 不对");
    // 比**集合**不比顺序：`build_manifest` 自己按 path 排过序（那是它的契约），
    // 而夹具的顺序是"按判别力挑的"。顺序一比就是拿生成器的排序当缺陷。
    let got: std::collections::BTreeSet<&str> = v["files"]
        .as_array()
        .expect("清单必须有 files 数组")
        .iter()
        .map(|e| e["path"].as_str().expect("每条都得有 path"))
        .collect();
    let want: std::collections::BTreeSet<&str> = FIXTURE.iter().map(|(p, _)| *p).collect();
    assert_eq!(
        got, want,
        "清单里的路径与夹具对不上（生成器把路径规范化了？）：{got:?}"
    );
}

/// 极小的 base64（只为把夹具字节塞进 JSON —— **不为此引依赖**，约束 7）。
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 桩服务：`python3 -u -m http.server 0`，**端口由它自己报**（见文件头那段 ⚠️）
// ---------------------------------------------------------------------------

struct Stub {
    child: Child,
    base_url: String,
    log_path: PathBuf,
}

impl Stub {
    fn start(root: &Path) -> Stub {
        let log_path = root.join("stub-server.log");
        let log = std::fs::File::create(&log_path).expect("建桩服务日志失败");
        let err = log.try_clone().expect("复制桩服务日志句柄失败");
        let child = Command::new("python3")
            // `-u`：stdout 被重定向到文件时 python 会改成块缓冲，那行端口播报
            // 要等缓冲区满才落盘 —— 读日志的循环会白等到超时。
            .args(["-u", "-m", "http.server", "0", "--bind", "127.0.0.1"])
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .expect("起不动桩服务（本测试依赖 python3，不静默跳过）");
        let mut stub = Stub {
            child,
            base_url: String::new(),
            log_path,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(port) = announced_port(&stub.log()) {
                stub.base_url = format!("http://127.0.0.1:{port}");
                return stub;
            }
            if Instant::now() >= deadline {
                let _ = stub.child.kill();
                panic!("桩服务没有在 10 秒内报出端口：\n{}", stub.log());
            }
            if let Ok(Some(status)) = stub.child.try_wait() {
                panic!("桩服务退出了（{status}）：\n{}", stub.log());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 日志全文（端口播报 + **每个请求的访问行**，后者是报告里的"请求痕迹"）。
    ///
    /// ⚠️ **读不到就 panic，绝不 `unwrap_or_default()`**（审查修复轮 1 的 ②）：
    /// 返回空串会让 `log_mark = 0`、`trace = ""`，于是 R28 那条最强的断言
    /// （"第二次一个数据文件都不许拉"）**恒真** —— 一条什么都不验的绿灯。
    /// "把读失败吞成默认值 ⇒ 空断言"正是本仓库反复记账的形态。
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path)
            .unwrap_or_else(|e| panic!("读桩日志 {} 失败: {e}", self.log_path.display()))
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 从播报行里取端口：`Serving HTTP on 127.0.0.1 port 52791 (http://…) ...`
///
/// ⚠️ **要等到 `(http://` 也到齐**：日志是**边跑边读**的，半行里取数字会把 `52791`
/// 读成 `52`（然后连到一个根本不是桩服务的端口上，症状是"清单下不下来"）。
fn announced_port(log: &str) -> Option<u32> {
    if !log.contains("(http://") {
        return None;
    }
    let marker = log.find("Serving HTTP on")?;
    let rest = &log[marker..];
    let at = rest.find("port ")?;
    rest[at + "port ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

// ---------------------------------------------------------------------------
// 用例工作区 + 跑 CLI
// ---------------------------------------------------------------------------

/// 一个用例的全部目录。**根目录是唯一的**（pid + 序号），所以它同时就是 R29 的进程过滤器。
struct Case {
    root: PathBuf,
    home: PathBuf,
    stub_root: PathBuf,
    dl: PathBuf,
}

impl Case {
    fn new(tag: &str) -> Case {
        let raw = std::env::temp_dir().join(format!(
            "cli-e2e-{tag}-{}-{}",
            std::process::id(),
            next_seq()
        ));
        std::fs::create_dir_all(&raw).expect("建用例临时目录失败");
        // ⚠️ **规范化**（`/var` 在 macOS 上是 `/private/var` 的符号链接）：引擎的
        // `--dir=` 用的是**我们传给 `-o` 的那个字符串**（`std::path::absolute` 不解析
        // 符号链接），所以根目录必须是同一个形态 —— 否则 R29 的过滤器看不清自己的引擎，
        // "没有孤儿"就成了一条空断言。
        let root = std::fs::canonicalize(&raw).expect("规范化用例临时目录失败");
        let home = root.join("home");
        let stub_root = root.join("stub");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&stub_root).unwrap();
        Case {
            dl: root.join("dl"),
            root,
            home,
            stub_root,
        }
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 一次 CLI 运行的读数。
struct CliRun {
    code: i32,
    stdout: String,
    stderr: String,
    elapsed: Duration,
    /// 运行期间**采样到的**属于本用例的 aria2c 行数的峰值。
    peak_aria2c: usize,
    /// 运行前本机上 aria2c 的总条数（R29 的计数口径）。
    aria2c_before: usize,
}

impl Case {
    fn run(&self, args: &[&str], base_url: &str, timeout: Duration) -> CliRun {
        let (out_path, err_path) = (self.root.join("cli.stdout.log"), self.root.join("cli.stderr.log"));
        let out = std::fs::File::create(&out_path).unwrap();
        let err = std::fs::File::create(&err_path).unwrap();

        let mut c = Command::new(env!("CARGO_BIN_EXE_benagen-dl"));
        c.args(args)
            .env("HOME", &self.home)
            .env("BENAGEN_BASE_URL", base_url)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));

        let needle = self.root.to_string_lossy().to_string();
        let aria2c_before = aria2c_lines().len();
        let mut child = c.spawn().expect("起不动 benagen-dl");
        let started = Instant::now();
        let mut peak_aria2c = 0usize;
        let status = loop {
            if let Some(s) = child.try_wait().expect("等 benagen-dl 失败") {
                break Some(s);
            }
            peak_aria2c = peak_aria2c.max(aria2c_lines_for(&needle).len());
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(SAMPLE_EVERY);
        };

        let elapsed = started.elapsed();
        // ⚠️ 同上（审查修复轮 1 的 ②）：读不到就 panic。空串会让失败消息里
        // "stdout/stderr 是空的"看起来像"CLI 什么都没说"，把诊断引偏。
        let stdout = std::fs::read_to_string(&out_path)
            .unwrap_or_else(|e| panic!("读 {} 失败: {e}", out_path.display()));
        let stderr = std::fs::read_to_string(&err_path)
            .unwrap_or_else(|e| panic!("读 {} 失败: {e}", err_path.display()));
        let code = match status {
            Some(s) => s.code().unwrap_or_else(|| {
                panic!("benagen-dl 被信号杀死（不该发生）\nstdout:\n{stdout}\nstderr:\n{stderr}")
            }),
            None => panic!(
                "benagen-dl 在 {} 秒内没有退出（挂住了）\nstdout:\n{stdout}\nstderr:\n{stderr}",
                timeout.as_secs()
            ),
        };
        CliRun {
            code,
            stdout,
            stderr,
            elapsed,
            peak_aria2c,
            aria2c_before,
        }
    }
}

// ---------------------------------------------------------------------------
// R29：aria2c 进程观测
// ---------------------------------------------------------------------------

/// 本机上**命令行里含 `aria2c`** 的那些进程，每行是 `PID 完整命令行`。
///
/// ⚠️ **取数必须用 `ps`，不能用 `pgrep`**（2026-09-21 修复轮实测，两个平台都跑过）：
///   · Linux（RHEL 9.5 的 procps-ng 3.3.17）：`pgrep -l` 打印的是 **comm（进程名）**，
///     `-f` 只改变**匹配**范围、**不改变打印内容** ——
///         `pgrep -fl sleep`  →  `2903033 sleep`      ← 只有进程名，没有参数
///         `pgrep -a  -x sleep` → `2903033 sleep 60`    ← `-a` 才是命令行
///     于是"按下载目录过滤"在 Linux 上**永远匹配不上**（进程名长 `aria2c-<摘要>`）。
///   · BSD（macOS）：`pgrep` **根本没有 `-a`**
///     （`usage: pgrep [-Lfilnoqvx] …`）。两个旗标合不到一处，所以不是"按平台分叉"
///     能解决的，要一条两边都能用的取数方式。
///   · `ps -Aww -o pid=,command=` 两边都有；`-ww` 保证**不截断**，
///     `pid=` / `command=` 末尾那个 `=` 是"不要表头"，于是每行正好是
///     `PID 完整命令行`（`kill_aria2c_for` 里 `split_whitespace().next()` 取 PID 那条
///     也因此照旧成立）。
///
/// ⚠️ 过滤词是命令行里的 `aria2c` 字样（不是进程名）：内核把内嵌引擎释放成
/// `aria2c-<摘要>`（`engine::daemon::extract_to`），**进程名不是 `aria2c`**；
/// 而 `ps` 给的正是整条命令行（含 argv[0] 的完整路径），所以匹配得上。
fn aria2c_lines() -> Vec<String> {
    let out = Command::new("ps")
        .args(["-Aww", "-o", "pid=,command="])
        .output()
        .expect("ps 不可用");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("aria2c"))
        .map(str::to_string)
        .collect()
}

/// **属于本用例**的 aria2c（命令行里含本用例那个唯一的根目录）。
///
/// 引擎的 argv 里必然有 `--dir=<下载目录>`，而下载目录在本用例根目录下 ——
/// 所以"这一个用例的引擎还在不在"是可精确判定的，不会被别人的 aria2c 污染。
fn aria2c_lines_for(needle: &str) -> Vec<String> {
    aria2c_lines()
        .into_iter()
        .filter(|l| l.contains(needle))
        .collect()
}

/// **观测**收尾（不 panic）：等到本用例的 aria2c 消失，返回（残留行, 跑后本机总数）。
///
/// ⚠️ 观测与断言**分开**是必须的，不是洁癖：R28 那条用例要在**同一次运行里**同时拿到
/// "退出码"与"有没有孤儿"两个读数，而先断言退出码会在孤儿检查之前 panic、把读数丢掉。
fn observe_end(case: &Case) -> (Vec<String>, usize) {
    let needle = case.root.to_string_lossy().to_string();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut orphans = aria2c_lines_for(&needle);
    while !orphans.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        orphans = aria2c_lines_for(&needle);
    }
    (orphans, aria2c_lines().len())
}

/// 收尾断言（R29：**成功与失败两条路都要过**）。
fn assert_end_clean(run: &CliRun, end: (Vec<String>, usize), what: &str) {
    let (orphans, after) = end;
    assert!(
        orphans.is_empty(),
        "{what}：还留着属于本次运行的 aria2c 孤儿（客户机器上会一直跑下去）：{orphans:#?}"
    );
    // 计数口径：跑前与跑后**本机上的 aria2c 总数相同**（不多不少）。
    assert_eq!(
        after,
        run.aria2c_before,
        "{what}：本机 aria2c 数量变了（跑前 {} → 跑后 {}）",
        run.aria2c_before,
        after
    );
}

fn assert_no_orphan_aria2c(case: &Case, run: &CliRun, what: &str) {
    let end = observe_end(case);
    assert_end_clean(run, end, what);
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// R28：**这条用例跑两次**。第一次全量下载并校验；第二次同目录、同码 ——
/// `enqueue` 会跳过已完整的，判据必须靠"开跑前就完整"那一项成立，**也必须退出 0**。
///
/// 第二次有**三条**红的路径，各自是不同的事：
///   * 挂住不退出 → `Case::run` 的超时 panic（`cli::converged` 不收敛）；
///   * 退出码不是 0 → 见下面那条断言的消息；
///   * 退出码是 0、**却又拉了一个数据文件** → 见"请求痕迹"那条断言
///     （绿得不对：它收敛是因为**有活干**，不是因为"已完整的被跳过"）。
#[test]
fn 全量下载之后退出码是_0_且文件逐字一致_再跑一次也是_0() {
    let _serial = serial();
    let case = Case::new("ok");
    let stub = Stub::start(&case.stub_root);
    make_fixture(&case.stub_root, &stub.base_url);
    let dl = case.dl.to_str().unwrap();

    // ── 第一次：全量下载 + 校验 ────────────────────────────────────────────
    let first = case.run(&[CODE, "-o", dl], &stub.base_url, DOWNLOAD_TIMEOUT);
    assert_eq!(
        first.code, 0,
        "全量下载应当成功（退出码 0）\nstdout:\n{}\nstderr:\n{}\n桩日志:\n{}",
        first.stdout, first.stderr, stub.log()
    );
    for (path, body) in FIXTURE {
        let got = std::fs::read(case.dl.join(path))
            .unwrap_or_else(|e| panic!("{path} 没落盘: {e}（盘上：{:?}）", list_files(&case.dl)));
        assert_eq!(got, body.as_bytes(), "{path} 的内容与夹具不一致");
    }
    // R29 的前置守卫：这次运行**确实起过引擎**。过滤器一旦失灵（进程名匹配不上），
    // 下面那条"没有孤儿"就是一条永远为真的绿灯 —— 这一句就是防那个的。
    assert!(
        first.peak_aria2c > 0,
        "整个下载期间一次都没看到属于本用例的 aria2c —— 要么引擎没起来，\
         要么过滤器失灵（后者会让'没有孤儿'变成空断言）\n桩日志:\n{}",
        stub.log()
    );
    assert_no_orphan_aria2c(&case, &first, "第一次跑完之后");
    println!(
        "[R29] 第一次：退出码 {}，耗时 {:?}，采样到的本用例 aria2c 峰值 {}，跑前本机总数 {}",
        first.code, first.elapsed, first.peak_aria2c, first.aria2c_before
    );

    // 第二次**不许重下**。判据取桩的**请求痕迹**（日志是只追加的，按字节位置切出
    // 第二次那一段）：`enqueue` 跳过已完整的那些 ⇒ 第二次除了 manifest.json
    // **一个数据文件都不该拉**。
    //
    // ⚠️ 这一条比"退出码 0"强，而且是**必要的**：只有退出码时，一个"又下了一遍"
    // 的实现在某些时序下**照样绿**（实测：它拉了一个数据文件、花掉与第一次相当的
    // 4.9 秒，退出码也是 0）—— 那正是 R28 要钉的"跳过已完整的"，它没跳过。
    let log_mark = stub.log().len();

    // ── 第二次（R28）：同一个下载目录、同一个交付码 ────────────────────────
    let second = case.run(&[CODE, "-o", dl], &stub.base_url, DOWNLOAD_TIMEOUT);
    let trace = stub.log()[log_mark..].to_string();
    // 先把 R29 的读数**取下来**（不 panic）并打出来，否则下面那条退出码断言一红，
    // 孤儿这一项就没人看了 —— 而"漏关引擎留孤儿"正是 R29 要验的那件事。
    let end_second = observe_end(&case);
    println!(
        "[R28] 第二次：退出码 {}，耗时 {:?}，采样到的本用例 aria2c 峰值 {}，跑前本机总数 {} \
         （收尾读数：残留 {} 条，跑后本机总数 {}）",
        second.code,
        second.elapsed,
        second.peak_aria2c,
        second.aria2c_before,
        end_second.0.len(),
        end_second.1
    );
    assert_eq!(
        second.code, 0,
        "第二次跑（同一个目录、同一个码）必须也退出 0 —— 上一次已经全下完并校验过，\
         这一跑应当直接收敛。非 0 ⇒ `cli::converged` / `reconfirm_verdict` 的判据还有洞\
         （T6 修的就是这个）\nstdout:\n{}\nstderr:\n{}\n桩日志:\n{}",
        second.stdout, second.stderr, stub.log()
    );
    for (path, _) in FIXTURE {
        let got = std::fs::read(case.dl.join(path))
            .unwrap_or_else(|e| panic!("第二次跑完 {path} 不见了: {e}"));
        assert!(!got.is_empty(), "第二次跑完 {path} 变成空文件了");
    }
    // ⚠️ **先证"这一段痕迹确实捕到了东西"**（审查修复轮 1 的 ③）：第二次跑**必然**拉过清单
    // （`load_delivery` 是成功门的前置，退出码 0 就意味着它成功了）。少了这一句，
    // 下面那条"没有数据文件"在日志格式一变（或切片没捕到）时会**静默退化成空断言**
    // —— 一条只断言"空集合是空的"的绿灯。
    assert!(
        trace.contains("manifest.json"),
        "第二次跑的桩痕迹里没有 manifest.json —— 这一段日志没捕到东西，\
         下面那条断言会退化成空断言\n第二次的桩痕迹：\n{trace}"
    );
    let refetched: Vec<&str> = trace
        .lines()
        .filter(|l| l.contains("GET ") && !l.contains("manifest.json"))
        .collect();
    assert!(
        refetched.is_empty(),
        "第二次跑又去拉了数据文件（已完整的不该再入队）：{refetched:?}\n\
         第二次的桩痕迹：\n{trace}"
    );
    assert_end_clean(&second, end_second, "第二次跑完之后");
    println!("[桩] 端口 {}，请求痕迹：\n{}", stub.base_url, stub.log());
}

#[test]
fn 不存在的交付码退出码是_2() {
    let _serial = serial();
    let case = Case::new("404");
    // 桩根是**空的** ⇒ 任何码都 404（`delivery::fetch` 对 4xx 是确定性失败，不重试）。
    let stub = Stub::start(&case.stub_root);
    let dl = case.dl.to_str().unwrap();

    let run = case.run(&["NotFoundCode00000000", "-o", dl], &stub.base_url, NEGATIVE_TIMEOUT);
    assert_eq!(
        run.code, 2,
        "拉清单失败应当是 2\nstdout:\n{}\nstderr:\n{}\n桩日志:\n{}",
        run.stdout,
        run.stderr,
        stub.log()
    );
    // 失败路上**也没有孤儿**（R29）：这一支引擎根本没起来过。
    assert!(
        run.peak_aria2c == 0,
        "拉清单就失败了，不该有引擎被启动过：峰值 {}",
        run.peak_aria2c
    );
    assert_no_orphan_aria2c(&case, &run, "404 那一次跑完之后");
    println!(
        "[404] 退出码 {}，耗时 {:?}，桩日志:\n{}",
        run.code,
        run.elapsed,
        stub.log()
    );
}

#[test]
fn 目标目录不可写时退出码是_3() {
    let _serial = serial();
    let case = Case::new("pre");
    let stub = Stub::start(&case.stub_root);
    make_fixture(&case.stub_root, &stub.base_url);

    // 拿一个**普通文件**当 `-o` 的目录：`create_dir_all` 必然失败 ⇒ preflight 判死。
    // （比 chmod 只读目录更干净：不依赖跑测试的人是不是 root。）
    let not_a_dir = case.root.join("not-a-dir");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let run = case.run(
        &[CODE, "-o", not_a_dir.to_str().unwrap()],
        &stub.base_url,
        NEGATIVE_TIMEOUT,
    );
    assert_eq!(
        run.code, 3,
        "目标目录不可用应当是 3\nstdout:\n{}\nstderr:\n{}\n桩日志:\n{}",
        run.stdout,
        run.stderr,
        stub.log()
    );
    // preflight 失败 ⇒ 引擎**没被启动**（`op_enqueue` 的注释：先于引擎启动）。
    assert!(
        run.peak_aria2c == 0,
        "preflight 就判死了，不该有引擎被启动过：峰值 {}",
        run.peak_aria2c
    );
    assert_no_orphan_aria2c(&case, &run, "preflight 失败那一次跑完之后");
    println!(
        "[preflight] 退出码 {}，耗时 {:?}，stderr:\n{}",
        run.code, run.elapsed, run.stderr
    );
}

#[test]
fn 交付码格式不对时退出码是_1_且不发任何请求() {
    let _serial = serial();
    let case = Case::new("fmt");
    // 判据：BASE_URL 指向**没人监听的端口**。若实现发过请求，那必然变成 2（网络失败）；
    // 得到 1 才证明"格式在发请求之前就判死了"（约束 2 的分界点）。
    let dl = case.dl.to_str().unwrap();
    let run = case.run(&["abc", "-o", dl], "http://127.0.0.1:1", NEGATIVE_TIMEOUT);
    assert_eq!(
        run.code, 1,
        "交付码格式不对应当是 1\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.peak_aria2c == 0,
        "用法错误不该启动引擎：峰值 {}",
        run.peak_aria2c
    );
    assert_no_orphan_aria2c(&case, &run, "格式错误那一次跑完之后");
    println!(
        "[usage] 退出码 {}，耗时 {:?}，stderr:\n{}",
        run.code, run.elapsed, run.stderr
    );
}

/// 盘上**除状态文件外**的全部普通文件（相对 `-o` 的路径）。
///
/// 只用在失败消息里：`{path} 没落盘` 那一刻最需要知道的是"盘上到底有什么"
/// ——百分比编码形态的文件名会在这里一眼露出来。
fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }
    out.sort();
    out
}
