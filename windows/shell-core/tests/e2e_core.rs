//! e2e —— 对着**真实内核二进制**的端到端（规格 §5 第 2 条）。
//!
//! ⚠️ **这条套件的价值全在"真"**：它同时钉住协议镜像的字段名、serde 的默认值语义，
//!    以及超长行那类"只有真内核才会给"的行为（回执**不带 id**、连接不断）。
//!    换成桩就全丢了——所以这里**不起桩**，也不因为"找不到二进制"而跳过：
//!    找不到就**响亮失败**（W-2），并给出可执行的补救。
//!
//! ⚠️ 跑它的入口是 `bash windows/scripts/test.sh`（它先 `cargo build --release` 把内核编出来、
//!    再验产物路径）。裸 `cargo test` 会因为内核不存在/是旧的而给出假红或假绿——
//!    `windows/scripts/test.sh` 的头注释记着 macOS 侧栽过的那一次（账本 Ruling C19）。

use shell_core::client::{
    core_arguments, ClientError, CoreClient, LineChannel, ProcessChannel, MAX_REQUEST_LINE_BYTES,
};
use shell_core::protocol::{codes, HelloResult, Response, VerifyClass, PROTOCOL_VERSION, UNCORRELATED_ID};

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 找内核 / 造临时目录
// ---------------------------------------------------------------------------

/// 内核二进制在**宿主平台**上的路径。
///
/// - 先看 `BENAGEN_CORE`（"临时换一个内核来试"，与 macOS `locateCoreBinary` 的第 ① 条同形）；
/// - 否则取仓库内的 release 产物。产物名**按宿主平台取**：Windows 上是 `benagen-core.exe`
///   （写死无扩展名的那个，在 Windows 上会以"内核不存在"这条**错误的根因**失败，
///    而真正的根因只是平台扩展名——本项目对"根因不许说错"有纪律）。
fn core_binary_path() -> PathBuf {
    if let Some(p) = std::env::var_os("BENAGEN_CORE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let name = if cfg!(windows) {
        "benagen-core.exe"
    } else {
        "benagen-core"
    };
    // `env!("CARGO_MANIFEST_DIR")` = <repo>/windows/shell-core ⇒ 上溯两级到仓库根的 core/
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("core")
        .join("target")
        .join("release")
        .join(name)
}

/// 内核产物**必须存在**。不存在就大声失败并给出补救——**不跳过**。
fn require_core_binary() -> PathBuf {
    let p = core_binary_path();
    if !p.is_file() {
        panic!(
            "找不到内核产物 {}。\n\
             e2e 必须对着**真**内核跑，所以这里不跳过（跳过 = 这条网不在）。\n\
             补救：用 `bash windows/scripts/test.sh`——它会先 `cargo build --release` 编出内核、\n\
             再核对产物路径，然后才跑本套件。",
            p.display()
        );
    }
    p
}

/// 一次性的临时目录（std 自带，**不引入 `tempfile`**：`shell-core` 的依赖白名单只有
/// serde/serde_json，加一个 dev-dependency 就会被 `test.sh` 的依赖守卫点名）。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "benagen-shell-e2e-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).expect("临时目录必须建得出来");
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

/// 起内核所需的 argv 与两个一次性目录。
///
/// ⚠️ 两个目录**必须**是临时的：内核默认写 `~/Library/Application Support/…/settings.json`，
///    拿真实路径跑测试等于拿用户的家目录当靶子。这也正是 macOS `CoreClient.live(settingsPath:
///    downloadDir:)` 存在的理由——argv 是**内核的输入**，不是协议消息。
fn core_argv(tag: &str) -> (PathBuf, Vec<String>, TempDir) {
    let dir = TempDir::new(tag);
    let settings = dir.path().join("settings.json");
    let args = core_arguments(Some(dir.path()), Some(settings.as_path()));
    (require_core_binary(), args, dir)
}

/// 起一个真内核的客户端（连同它的临时目录，返回后必须一直持有到测试结束）。
fn spawn_core(tag: &str) -> (CoreClient, TempDir) {
    let (bin, args, dir) = core_argv(tag);
    let client = CoreClient::spawn(&bin, &args).unwrap_or_else(|e| {
        panic!(
            "起不了内核 {}：{e}\n补救：用 `bash windows/scripts/test.sh` 跑本套件。",
            bin.display()
        )
    });
    (client, dir)
}

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

/// 握手走一遍：spawn → `hello` → `ok:true` 且 `result.protocol == 1` → `shutdown`。
///
/// ⚠️ 解的是**强类型** `HelloResult`（不是拿 `Value` 东摸一个键）：字段名对不上就是
///    一条 `missing field` 解码错误——那是漂移探测器的落点。判别力自证见 task-8 报告：
///    把 `min_split_size_choices` 拼错一个字母，这一条立刻红。
#[test]
fn hello_round_trips_against_the_real_core() {
    let (client, _dir) = spawn_core("hello");

    let result = client
        .call("hello", json!({"protocol": PROTOCOL_VERSION}))
        .expect("真内核必须握手成功");
    let hello: HelloResult =
        serde_json::from_value(result).expect("握手回执必须解成 HelloResult（字段名漂移就会在这里红）");

    assert_eq!(hello.protocol, PROTOCOL_VERSION, "版本必须协商成 1");
    assert!(
        !hello.min_split_size_choices.is_empty(),
        "`-k` 的取值集合必须随握手下发（设计规格 §2.3：壳不得自带一份硬编码的副本）"
    );
    for c in &hello.min_split_size_choices {
        assert!(
            c.ends_with('M') && c[..c.len() - 1].parse::<u32>().is_ok(),
            "取值集合的形状是 {{\"<数字>M\"}}，实际 {c:?}"
        );
    }

    client.shutdown();
}

/// `hello` 的版本不符**不是**降级协商，是一条 `protocol_mismatch`。
///
/// 同时钉住"错误码按值取"这条路：壳按 `code` 分支，不按 message 措辞。
#[test]
fn a_wrong_protocol_version_comes_back_as_protocol_mismatch() {
    let (client, _dir) = spawn_core("version");

    match client
        .call("hello", json!({"protocol": PROTOCOL_VERSION + 1}))
        .expect_err("版本不符必须报错")
    {
        ClientError::Kernel { code, .. } => {
            assert_eq!(code, codes::PROTOCOL_MISMATCH, "码必须逐字");
        }
        other => panic!("应为 Kernel，实际 {other:?}"),
    }

    client.shutdown();
}

/// 不认识的 `method` → `unknown_method`（壳靠这个值决定"这个动作不存在"，
/// 而不是把它当成"引擎没起来"）。
#[test]
fn an_unknown_method_comes_back_as_unknown_method() {
    let (client, _dir) = spawn_core("unknown-method");

    match client
        .call("reveal_in_explorer", Value::Null)
        .expect_err("不认识的 method 必须报错")
    {
        ClientError::Kernel { code, .. } => {
            assert_eq!(code, codes::UNKNOWN_METHOD, "码必须逐字");
        }
        other => panic!("应为 Kernel，实际 {other:?}"),
    }

    client.shutdown();
}

// ---------------------------------------------------------------------------
// 六个校验键（裁决 V 的真内核一侧）
// ---------------------------------------------------------------------------

/// `verify_status` 回执里的六个键**就是** `VerifyClass::wire_key()` 那六个。
///
/// 这是 `wire_key()` 那张表唯一能对着**真内核**验的机会：表是壳自己抄的，
/// 抄错了单元测试照样绿（判据是同一份抄件）；只有真回执能证伪。
/// `all_good` 不进 `VerifyClass`（它是聚合位，不是归类），所以单独排除。
#[test]
fn verify_status_carries_exactly_the_six_keys_the_shell_maps() {
    let (client, _dir) = spawn_core("verify-keys");

    let result = client
        .call("verify_status", Value::Null)
        .expect("verify_status 不该要求先 load_delivery");
    let obj = result.as_object().expect("回执必须是 JSON 对象");

    const ALL: [VerifyClass; 6] = [
        VerifyClass::Passed,
        VerifyClass::Mismatched,
        VerifyClass::Missing,
        VerifyClass::SizeMismatch,
        VerifyClass::Unverifiable,
        VerifyClass::Unreadable,
    ];
    for class in ALL {
        assert!(
            obj.contains_key(class.wire_key()),
            "回执里没有 {:?} 的线上键 {:?}（实际键：{:?}）",
            class,
            class.wire_key(),
            obj.keys().collect::<Vec<_>>()
        );
    }
    // 反方向：回执里除 all_good 之外，不许有映射表里没有的键
    for k in obj.keys() {
        if k == "all_good" {
            continue;
        }
        assert!(
            ALL.iter().any(|c| c.wire_key() == k),
            "回执里有映射表外的键 {k:?}（新增归类必须同时进 VerifyClass 与 wire_key）"
        );
    }

    client.shutdown();
}

// ---------------------------------------------------------------------------
// 超长行（通道层：这条只有真内核给得出）
// ---------------------------------------------------------------------------

/// 超过 8 MiB 的一行：内核回一条 `id == 0` 的 `bad_request`，**然后继续服务**。
///
/// 为什么这条必须对着真内核跑：这里钉的三件事全是内核的行为，桩里没有——
///   1. 超限的**码**是 `bad_request`；
///   2. 回执的 `id` 是 0（内核读不到 id）⇒ 壳"等自己那条响应"就是永久挂死，
///      所以壳侧必须在**写之前**就拦住超长请求（`ClientError::RequestTooLong`）；
///   3. 报错之后**不断管道**：剩下那半行被当成碎片、再回一条 `bad_request`，
///      之后协议重新对齐、握手照常成功。
///
/// 这条用例用 `ProcessChannel` 直接写（绕过 `CoreClient` 的本地拦截）——
/// 那正是"内核会不会挂"要问的问题。
#[test]
fn an_over_long_line_is_answered_with_a_structured_error_and_the_core_keeps_serving() {
    let (bin, args, _dir) = core_argv("over-long");
    let channel = ProcessChannel::new(&bin, &args, None).expect("必须能起真内核");

    let payload = "x".repeat(MAX_REQUEST_LINE_BYTES + 1);
    channel
        .write_line(&payload)
        .expect("写本身不该失败（管道是好的，超限是内核那边的事）");
    channel
        .write_line(r#"{"id":1,"method":"hello","params":{"protocol":1}}"#)
        .expect("超长行之后必须还能写");

    let mut alerts: Vec<String> = Vec::new();
    let mut hello: Option<Value> = None;
    for _ in 0..8 {
        let Some(line) = channel.read_line().expect("读不该失败") else {
            break;
        };
        let r: Response = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("内核只许回 JSON 行，实际 {line:?}: {e}"));
        if r.id == UNCORRELATED_ID {
            let e = r.error.expect("id==0 的响应必须带 error");
            assert!(!e.message.is_empty(), "message 是人看的那句话，不该是空串");
            alerts.push(e.code);
            continue;
        }
        assert_eq!(r.id, 1, "壳自己那条必须是 id 1");
        assert!(r.ok, "超长行之后内核必须还能正常完成一次握手");
        hello = r.result;
        break;
    }

    assert!(
        alerts.iter().any(|c| c == codes::BAD_REQUEST),
        "内核必须先回一条 bad_request（id 0）——实际收到的 id==0 码：{alerts:?}"
    );
    assert!(
        hello.is_some(),
        "超长行**不许**把连接搞死：内核必须继续服务（管道没断、协议重新对齐）"
    );

    channel.close();
}

/// **请求上限的边界（一个字节的差），拿真内核复核。**
///
/// 壳侧的判据是 `正文 >= MAX_REQUEST_LINE_BYTES ⇒ 本地拒绝`，理由是内核的额度里
/// **含换行符**：正文 MAX 字节的一行后面还得跟一个 `\n`，内核读到 `MAX` 个字节、
/// 末尾不是 `\n`、于是判超限（`core/src/main.rs:118-136`）。
/// 这条断言把"壳抄的那个数字"与"内核真正的阈值"对在一起：**判据差一个字节就是永久挂死**，
/// 而单元测试的判据是同一份抄件（抄错照样绿），只有真内核能证伪。
///
/// 两侧各来一条：
///   - 正文 `MAX` ⇒ 内核回 `id == 0` 的 `bad_request`（它读不到我们的 id）；
///   - 正文 `MAX - 1` ⇒ 内核把它当**一条正常请求**读进去，回 `id == 1`
///     （内容是缺 `protocol` 的 `hello`，所以 `ok` 是 false——**回执的 id 才是判据**）。
#[test]
fn the_request_cap_boundary_matches_the_real_kernel() {
    let (bin, args, _dir) = core_argv("cap-boundary");
    let channel = ProcessChannel::new(&bin, &args, None).expect("必须能起真内核");

    let over = request_line_of_len(MAX_REQUEST_LINE_BYTES);
    channel.write_line(&over).expect("写本身不该失败");
    let r = read_one_response(&channel);
    assert_eq!(
        r.id, UNCORRELATED_ID,
        "正文正好 MAX 字节：内核读不到这一行的 id，回执必须是 id 0"
    );
    assert_eq!(
        r.error.expect("id==0 的响应必须带 error").code,
        codes::BAD_REQUEST,
        "超限的码必须逐字"
    );

    let fits = request_line_of_len(MAX_REQUEST_LINE_BYTES - 1);
    channel.write_line(&fits).expect("写本身不该失败");
    let r = read_one_response(&channel);
    assert_eq!(
        r.id, 1,
        "正文 MAX-1 字节（+ 换行 = 正好 MAX）：内核必须按一条**正常请求**读它"
    );

    channel.close();
}

/// 造一行正文**正好** `target` 字节的 `hello` 请求（不含换行；换行由通道补）。
/// 夹具自己断言长度，偏一字节这条边界就白测了。
fn request_line_of_len(target: usize) -> String {
    let base = r#"{"id":1,"method":"hello","params":{"pad":""}}"#.len();
    assert!(target > base, "目标长度装不下请求壳（{target} <= {base}）");
    let line = format!(
        r#"{{"id":1,"method":"hello","params":{{"pad":"{}"}}}}"#,
        "x".repeat(target - base)
    );
    assert_eq!(line.len(), target, "夹具自身没对上目标长度");
    line
}

/// 读一行并解成 `Response`（内核只许回 JSON 行）。
fn read_one_response(channel: &ProcessChannel) -> Response {
    let line = channel
        .read_line()
        .expect("读不该失败")
        .expect("还没到 EOF，必须有一行响应");
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("内核只许回 JSON 行，实际 {line:?}: {e}"))
}

// ---------------------------------------------------------------------------
// 收尾
// ---------------------------------------------------------------------------

/// `shutdown` 之后内核**自行退出**（不留孤儿）；之后再写是一个结构化错误。
#[test]
fn the_core_exits_after_shutdown_and_leaves_no_orphan() {
    let (bin, args, _dir) = core_argv("shutdown");
    let channel = ProcessChannel::new(&bin, &args, None).expect("必须能起真内核");

    channel
        .write_line(r#"{"id":1,"method":"shutdown","params":null}"#)
        .expect("shutdown 请求必须写得出去");

    let deadline = Instant::now() + Duration::from_secs(15);
    while channel.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !channel.is_running(),
        "shutdown 之后内核必须自行退出（留着孤儿进程就是客户机器上那个「关不掉的 benagen-core」）"
    );

    // 进程没了 → 读端是 EOF；再把剩下的响应行读干净（内核先回执再退出）
    let mut saw_ok = false;
    while let Some(line) = channel.read_line().expect("读不该失败") {
        if let Ok(r) = serde_json::from_str::<Response>(&line) {
            if r.id == 1 && r.ok {
                saw_ok = true;
            }
        }
    }
    assert!(saw_ok, "shutdown 的回执必须是一条 ok:true（id 1）");

    // 死管道上再写：结构化错误，不是崩溃（SIGPIPE 那一课，见 client.rs 的同名用例）
    let err = channel.write_line(r#"{"id":2}"#).expect_err("写向死管道必须报错");
    assert!(matches!(err, ClientError::WriteFailed { .. }), "实际 {err:?}");

    channel.close();
}

/// `CoreClient` 收尾之后再调用：**快速的结构化失败**（不是挂死、不是 panic）。
///
/// 它同时证明 `CoreClient::spawn` 拼出来的 argv 真的能驱动真内核——
/// 上面几条走的是 `ProcessChannel`，这一条走完整的上层。
#[test]
fn a_call_after_shutdown_fails_fast_instead_of_hanging() {
    let (client, _dir) = spawn_core("after-shutdown");
    client
        .call("hello", json!({"protocol": PROTOCOL_VERSION}))
        .expect("先握一次手，证明确实连上了");
    client.shutdown();

    match client.call("get_tree", Value::Null) {
        Err(ClientError::Closed) => {}
        other => panic!("收尾之后必须快速失败成 Closed，实际 {other:?}"),
    }
}
