//! CLI：**纯逻辑**（`args` / `exit` / `progress` 三个子模块）+ **编排壳**（本文件的 [`run`]）。
//!
//! ⚠️ 三个子模块**不读环境、不碰文件系统、不启动引擎**——所有外部输入（`home`、
//! 环境变量）都由调用方传进来。这样它们才是纯函数、才能完全单测；平台相关的取值
//! （`$HOME`、`$BENAGEN_BASE_URL`）只有一处，在壳里。
//!
//! [`run`] **就是那个壳**：读环境、起引擎、装信号处理器、收尾关引擎。它放在 lib 里
//! 而不是 `src/bin/benagen-dl.rs`，理由是那条既有纪律「**能写出断言的东西不放入口**」
//! ——在 lib 里 `cargo test` 一视同仁地跑它，在 bin 里就只有端到端测试碰得到。
//!
//! ⚠️ **编排不许自己再写一遍**：本文件的每一条副作用都经 `kernel::dispatch` 走
//! 图形客户端那条同一个入口（`call()` 的注释）。这是 spec §5 的流程图，也是
//! "两个壳、一份语义"的全部内容。

pub mod args;
pub mod exit;
pub mod progress;

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::engine;
use crate::kcodes;
use crate::kernel::{
    dispatch, spawn_landing_watcher, spawn_verify_worker, ErrorBody, Kernel, Request, VerifyJob,
};
use crate::settings;

use self::args::{Action, Options};
use self::exit::ExitCode;

/// `--version` 那一行。
///
/// 内容 = 版本号 + **内嵌 aria2c 的 sha256 前 12 位**（spec §1 第 3 条）。
///
/// ⚠️ **摘要必须与内核释放临时文件时的命名同源**：`engine::daemon::extract_to` 把
/// 二进制释放成 `aria2c-{sha256 前 12 位}`。所以客户在 `--version` 里报出来的那一串，
/// 运维能直接对上他机器上 `$HOME/Library/Caches/BenagenDownloader/` 里的文件名——
/// 这正是排障时第一个要问的东西，两处**必须**是同一个来源（`embed_hash_hex()`），
/// 而不是各算各的。
///
/// 取前 N 位是**截断**而不是"另一个值"：sha256 的十六进制串恒为 64 字符
/// （`sha256` 返回 `[u8; 32]`），所以这里的切片不会越界。`N` 走内核那个常量
/// （`EMBED_HASH_PREFIX_LEN`），**不在壳里再写一个 12**——两处各写一遍，
/// 改了一处就会变成"客户手上那串对不上缓存目录里的文件名"，而且不报错。
pub fn version_line() -> String {
    let sha = engine::daemon::embed_hash_hex();
    format!(
        "benagen-dl {}（内嵌 aria2c {}）",
        env!("CARGO_PKG_VERSION"),
        &sha[..engine::daemon::EMBED_HASH_PREFIX_LEN]
    )
}

// ---------------------------------------------------------------------------
// 信号：Ctrl-C 的唯一落点
// ---------------------------------------------------------------------------

/// 为什么值得装这一小块（而不是让默认处置直接杀进程）：默认处置下进程一死，
/// **子进程 aria2c 会变成孤儿继续下载** —— 客户机器上多出一个谁也管不着的进程。
/// 所以：处理器**只置一个位**（信号处理器里不许分配、不许 I/O），收尾由轮询循环做。
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// `SIGINT` 的编号与 `SIG_DFL`（"默认处置"）。POSIX 把这两个值定死了，
/// 本轮**不引 libc**（全局约束 7），所以自己写。
const SIGINT: i32 = 2;
const SIG_DFL: usize = 0;

/// **Ctrl-C 的两次口径**（客户可见的契约，README 会照抄这一段）。
///
/// * **第一次**：置位，交给轮询循环做**优雅收尾**——关掉引擎（不留孤儿 aria2c）、
///   退出码 `130`；状态文件本来就是每拍原子写，所以**再跑一次会接着下**。
///   ⚠️ 这一下**不会立刻见效**：收尾要等**当前那次内核调用返回**（最坏是 `load_delivery`
///   的 91.5 秒或一次 30 秒 × N 的 `plan`），因为轮询循环还在那次调用里面。
/// * **第二次**：**立刻终止**。处理器把 `SIGINT` 恢复成默认处置，于是这一下是
///   内核直接杀进程——客户按第二下就是在说"我不想等了"。
///   ⚠️ **代价如实记下：这条路可能留下孤儿 aria2c**（进程没有任何机会跑收尾）。
///   那是**客户自己按的第二下**，不是我替他决定的；而堵死这个出口更糟——
///   网络卡住时连按几次都没反应，客户只会去 `kill -9`，那**同样**留孤儿，
///   而且是"工具不听话"逼出来的。
///
/// ⚠️ **处理器里只许做信号安全的事**：一次原子 `store` + 一次 `signal`
/// （两者都在 POSIX 的 async-signal-safe 名单上）。**不许打印、不许分配**——
/// 那两样在信号处理器里是 UB 的温床。
extern "C" fn on_sigint(_sig: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
    // 第二次 Ctrl-C 立刻终止：第一次已经交给我们做优雅收尾，若客户再按一次，
    // 那是"我不想等了" —— 恢复默认处置让信号直接杀进程。
    //
    // 显式恢复而不是指望平台的语义：`signal()` 在 Linux/macOS 上是 BSD 语义
    // （处理器**常驻**，不会自动复位），所以不复位的话第二次按下去还是进这里、
    // 还是只置一个位——**那正是要修的那个"逃生口被堵上"**。
    unsafe { signal(SIGINT, SIG_DFL) };
}

// 不引 crate（全局约束 7）：`signal` 是 C 标准函数，macOS 与 Linux 的 libc 里都有。
//
// `handler` 写成 `usize`（而不是 `extern "C" fn(i32)`）：C 的 `sighandler_t` 就是一个
// 函数指针，与本平台的指针同宽，而 `fn signal(2, on_sigint)` 这种写法要依赖 Rust 的
// 函数指针 ABI 与 C 的**逐位一致**——`usize` 这条更笨、但把"我不假设 ABI"写在了脸上。
// `SIG_DFL`（0）也走同一条：C 里它是一个 `(void (*)(int))0`，按同一个"位宽相同"的
// 假设传 `usize` 的 0。
//
// ⚠️ 这一段**必须是 `//` 不是 `///`**：`extern` 块上方的文档注释是 `unused_doc_comments`
// 警告（rustdoc 不为 extern 块生成文档），而本仓库的纪律是**新增告警 0**。
extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

fn install_sigint() {
    unsafe { signal(SIGINT, on_sigint as usize) };
}

// ---------------------------------------------------------------------------
// 一次调用
// ---------------------------------------------------------------------------

/// 走 `dispatch`（**不是**直接调 `op_*`）：CLI 与图形客户端经过的是同一个入口，
/// 将来 `dispatch` 里加了方法级守卫，CLI 不会漏掉。
///
/// 失败时**逐字打印内核原文**（全局约束 8：壳不加工内核文案），再把它翻译成退出码。
fn call(
    kernel: &Arc<Mutex<Kernel>>,
    n: u64,
    method: &str,
    params: Value,
) -> Result<Value, ExitCode> {
    call_full(kernel, n, method, params).map_err(|(code, _)| code)
}

/// 与 [`call`] **同一条路径**，但**把内核的码也带回来**。
///
/// ⚠️ 只有一处需要它（复核不成立那一支的传输侧观测）：`engine_not_started` 是
/// "**没有引擎可观测**"（`enqueue` 因为没活干而故意没起引擎，那是**合法**状态），
/// 而 `engine_start_failed` / `engine_disconnected` / `engine_rpc_failed` /
/// `protocol_mismatch` / `internal` 是**引擎层真的坏了**（退出码 4 的语义）。
/// 只看 `ExitCode` 的话这两类被压成同一个值，那一支就会**把真的坏了报成"停下"**。
fn call_full(
    kernel: &Arc<Mutex<Kernel>>,
    n: u64,
    method: &str,
    params: Value,
) -> Result<Value, (ExitCode, String)> {
    let (resp, _stop) = dispatch(
        kernel,
        &Request {
            id: n,
            method: method.to_string(),
            params,
        },
    );
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        let body = resp
            .error
            .unwrap_or_else(|| ErrorBody::new("unknown", "内核没有给出错误详情"));
        eprintln!("benagen-dl: {}（{}）", body.message, body.code);
        Err((exit::of_error(&body), body.code))
    }
}

/// 一次运行的全程。**每一支都在这里收口**（返回值直接就是进程退出码）。
pub fn run(argv: Vec<String>) -> ExitCode {
    // ① 解析。--help/--license/--version 在这里就返回（三个"打印点"不碰网络、不碰引擎）。
    let action = match args::parse(
        &argv,
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var("BENAGEN_BASE_URL").ok(),
    ) {
        Ok(a) => a,
        Err(e) => {
            // 解析失败时**不必**再打 `ExitCode::Usage::message()`：这一条本身已经是一句
            // 用法说明（"缺交付码。用法：…"），再补一句"命令行用法不对"是噪声。
            eprintln!("benagen-dl: {}", e.message);
            return ExitCode::Usage;
        }
    };
    let opts = match action {
        Action::Help => {
            println!("{}", args::HELP);
            return ExitCode::Ok;
        }
        Action::Version => {
            println!("{}", version_line());
            return ExitCode::Ok;
        }
        Action::License => {
            // GPLv2 全文（内嵌资产，分发义务的载体）。`print!` 而不是 `println!`：
            // 正文自带结尾换行，再补一个会多出一空行。
            print!("{}", engine::daemon::license_text());
            return ExitCode::Ok;
        }
        Action::Run(o) => o,
    };

    // ② 内核：下载目录与 settings 路径都交给它。
    //    默认下载目录**不在这里建**——`preflight`（`enqueue` 的第一步）会 create_dir_all，
    //    目标目录可不可用只有那一处判据；壳提前建一遍就是第二份会漂移的判据。
    //    settings 路径用内核的默认（**只读**）：`-j/-x/-s` 只改内存，不落盘（spec §2）。
    let mut k = Kernel::new(opts.download_dir.clone(), settings::default_path());
    // ⚠️ R11：签名是三个 `Option<i32>`，不是 `&Overrides`（内核不得依赖 `cli` 的类型）。
    k.apply_overrides_in_memory(
        opts.overrides.parallel,
        opts.overrides.connections,
        opts.overrides.splits,
    );
    let kernel = Arc::new(Mutex::new(k));

    // ③ 起那两个后台 worker（少一个 ⇒ 校验永远不收敛，spec §5 的 ⚠️）。
    //    与 `benagen-core` 的 `main()` 是同一对函数、同一条拓扑。
    let (tx, rx) = mpsc::channel::<VerifyJob>();
    spawn_verify_worker(Arc::clone(&kernel), rx, tx.clone());
    spawn_landing_watcher(Arc::clone(&kernel), tx.clone());
    install_sigint();

    // ④ 拉清单 → 全量入队 → 轮询 → 等校验收敛
    let code = download_and_verify(&kernel, &opts);

    // ⑤ 收尾：**无论走哪条出口都要关引擎**（否则留下孤儿 aria2c）。
    //    `close` 必须在**锁外**做：它最坏要等满 5 秒的兜底，持着内核锁等会把校验线程
    //    与落地监听器一起冻住。
    let daemon = {
        let mut g = kernel.lock().unwrap_or_else(PoisonError::into_inner);
        g.take_daemon()
    };
    if let Some(d) = daemon {
        if let Err(e) = d.close() {
            eprintln!("benagen-dl: 关闭下载引擎时出错: {e}");
        }
    }

    // ⑥ 最后收一句中文（spec §2）：内核原文刚才已经逐字打在**前一行**（`call()` 里），
    //    这一句只说"这是哪一步坏的、接下来怎么办"（`ExitCode::message`）。
    //    成功时不打——成功那一行已经把目录、文件数说清楚了，多一句是噪声。
    if code != ExitCode::Ok {
        eprintln!("benagen-dl: {}", code.message());
    }
    code
}

/// **收敛判据**（spec §5 的那条 ⚠️，承重）：两条**同时**成立才算成功。
///
/// ⚠️ **不许写成裸的 `all_good`**：`verify::CheckResult` 初始是**全空的**，而
/// `all_good()` 的定义是"`bad + missing + size_mismatch + unreadable` 都空"——
/// 空集合同样满足 ⇒ **一个文件都还没下的时候它就是 `true`**，工具会一启动就报成功、
/// 退出码 0，而客户手里一个字节都没有。
///
/// 第 2 条（`ok + unverifiable == 清单文件总数`）才排除了"什么都还没验"那一态。
/// `unverifiable`（清单没给 crc64）**算数**，是因为它没有可比对象、重下也不产生；
/// 但它仍然必须已下载 —— 那由第 1 条里的 `missing` / `size_mismatch` 挡住。
///
/// ⚠️ **第 2 条里"运行前就已完整的"那一项是承重的**（修复轮 1 的 ①）：
/// `ok` 只装**本次运行真的校验过**的文件，而 `enqueue {paths: []}` 只入队**待下载**的
/// （已完整的被 `view::pending_paths` 跳过）、`load_delivery` 又**无条件清空** `k.verify`。
/// ⇒ 凡盘上已有完整文件的运行（**就是帮助文本公开承诺的"再跑一次同一条命令"**），
/// `ok + unverifiable` 永远够不到 `total_files`，判据**永不成立**；
/// 再配上"校验阶段不许退出"，工具就**永久挂住、连退出码都不给**。
/// 那一项取自 `get_tree` 的 `flat`（每条自带 `state`，见 [`flat_complete_count`]）。
///
/// ⚠️ 用 `>=` 而不是 `==`：三组**会重叠**（一个文件可以既"运行前就完整"又被本次校验到，
/// 也可以既是 `unverifiable` 又…），要求恰好相等会把合法的成功判成不收敛。
///
/// ⚠️ **抽成纯函数是为了让它有单测守**（它原先只是 `download_and_verify` 里的一句 `if`）：
/// 它长在轮询循环里、要造出"任务都下完但校验还没跑"的窗口才碰得到，而那个窗口在
/// 端到端里是**时序**，不好稳定复现。判据本身是纯的，就该被纯地钉住。
fn converged(
    all_good: bool,
    ok: usize,
    unverifiable: usize,
    already_complete: usize,
    total_files: usize,
) -> bool {
    all_good && ok + unverifiable + already_complete >= total_files
}

/// `get_tree` 的 `flat` 里的文件总数（收敛判据的分母）。
fn flat_len(tree: &Value) -> usize {
    tree.get("flat")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// 成功那一行的两个数：**文件数 + 总字节**。纯函数，**两者同口径**（修复轮 5）。
///
/// 两个数**都取自同一份 `get_tree` 的同一批文件**：文件数 = `flat` 的条数（清单全量），
/// 字节 = `progress.total_bytes`（内核自己算的整批字节和，与 `flat` 同一份清单）。
///
/// ⚠️ **不许拿 `transfer_list` 的 `total` 之和来配它**（修之前就是那么写的）：
/// 那是**本次运行入队的那几个文件**的字节（`view.total_bytes`），而文件数是**整批**——
/// 续传时会打出"5 个文件，1.2 GB"，而 1.2 GB 只是其中 2 个文件的量。
/// 客户读这一行是在确认"这一批到底落了多少"，两个数必须是同一批东西；
/// 而且这一行的主语是**整批**（"全部完成并校验通过"），口径只能是整批。
///
/// 参数只有 `tree`、**没有 `view`**：混口径在结构上就写不出来——想混也没第二个来源可传。
fn success_tally(tree: &Value) -> (usize, i64) {
    (
        flat_len(tree),
        tree.get("progress")
            .and_then(|p| p.get("total_bytes"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
}

/// 从 `verify_status` 的响应里取 `all_good`。缺键 / 类型不对当 `false`
/// ——保守方向是**宁可不宣布成功**。
fn all_good_of(v: &Value) -> bool {
    v.get("all_good").and_then(Value::as_bool).unwrap_or(false)
}

/// **复核的判据**（纯函数，R-1）：拿**新鲜**的 `get_tree` 与 `verify_status` 再判一次。
///
/// ⚠️ **为什么宣布成功之前必须复核**（修复轮 2 的 R-1）：`already_complete` / `total_files`
/// 取自**开跑那一瞬**的快照（`already_complete` 只取一次，轮询里不重取 `get_tree`），
/// 而客户完全可能在下载途中删掉一个"早已完整"的文件。那份旧快照仍然说 `complete`、
/// `all_good` 也是真（那个文件**从没被交给校验**）⇒ 我们会**报成功而数据是缺的**：
/// 退出码 0、还打"全部完成并校验通过"。而 `0` 是脚本眼里"全都在盘上且 crc64 对得上"
/// 的唯一凭证（spec §0 那条判据）——**把没做到的说成做到了**是最重的一类错。
///
/// 重取 `get_tree` 会顺带走 `ensure_complete` 的 `recheck_complete_on_disk`
/// ——那份"廉价核对"就是为"磁盘被内核之外的力量改动"写的，只 stat、不读内容。
///
/// ⚠️ **只在"看起来已经成功"那一刻复核一次**，不要每拍都做：`get_tree` 在大批次上是
/// O(n) 的（还可能带一次 plan），而"看起来成功"只发生一次。
///
/// ---
///
/// # 复核的判据**由哪几项构成**（修复轮 5 —— 这份判据是**第二次**栽在"哪几项该相加"上，
/// 下面这段就是给下一个动它的人写的）
///
/// 判据只有两项相加：
///
/// ```text
/// all_good(fresh_verify)  &&  unverifiable(fresh_verify) + complete(fresh_tree) >= flat.len(fresh_tree)
///                        ↑                                            ↑
///                   本次没可比对象的那些                盘上**现在**完整的那些（新鲜的）
/// ```
///
/// ## 为什么**没有** `ok`（这一条是本轮的全部）
///
/// 因为 **`ok ⊆ complete(fresh_tree)`** —— 它们**不是两个并列的来源**，是同一批文件。
/// 已经核实过的包含链条（读的是代码，不是推断）：
///
/// 1. `kernel::spawn_verify_worker` 的锁内那一段：**先** `k.state.put(&path, entry)` 落记录、
///    `k.state.save()`，**再** `k.complete_dirty = true` —— 两步在**同一个锁作用域**里，
///    对我们后来的 `get_tree` 是一次原子的；
/// 2. `get_tree` 走 `ensure_complete`，`complete_dirty` 为真 ⇒ 重跑 `plan` ⇒
///    `k.complete = complete_from(&todo)`（`kind == Skip` 的那些）；
/// 3. `Skip` 的门是 `planner::classify_with_state` 的判定链，最后一条是
///    `entry_proves_complete(e, size, mtime, manifest_crc64)`：**size + mtime 命中**
///    且 `e.crc64 == 清单 crc64`；
/// 4. 而 `verify::check` 写进记录里的正是这三样：`meta.len()`（同一个文件）、
///    `engine::mtime_secs(meta.modified())`（**与 `planner::stat_secs` 同一个换算函数**，
///    那里有一条测试专门钉"两条路不许各算一份 mtime"）、`e.crc64 = f.crc64`（就是清单里那个）。
///
/// ⇒ 凡是被我们验成 `ok` 的文件，只要它的内容没被改动，下一次 `get_tree` 都**必然**在
/// `complete` 里。把 `ok` 再加一次，`>=` 就凭空多出 **"本次验过几个就有几个"** 个余量：
/// 最小复现是"2 个文件、`a` 早已完整、`b` 本次下完并验过、客户在途中删了 `a`" ——
/// `1(ok) + 0 + 1(complete) >= 2` ⇒ **报成功而盘上少一个**。真实续传里余量更大
/// （50 个已完整 + 50 个本次下完 ⇒ 50 个单位的余量 ⇒ 对"先前已完整文件"的删除全被吸收）。
///
/// ## 为什么**要**留 `unverifiable`
///
/// 因为它**不在** `complete` 里，不是重复计数：清单没给 crc64 的文件，
/// `verify::check` 判 `unverifiable` 时**不落状态记录**，而 `entry_proves_complete` 在
/// 清单 crc64 为空时要求"记录里有曾经校验通过的非空值"⇒ 它们过不了 `Skip` 的门。
/// 删掉这一项，**整个"清单没给 crc64"的批次就永远通不过复核**——那就是把判据收紧成
/// "续传/这类批次永远不成功"，是这条判据的**第一版**，已经被同一条审查流程打回过。
///
/// ## 给下一个人：这里的通用规则
///
/// **相加之前先问一句"这两组会不会指向同一个文件"**。这一处之所以特殊，是因为
/// `complete` 是**新鲜**的（本次跑出来的**结果**已经在里面了）；而主循环那份
/// [`converged`] 之所以能加 `ok`，是因为它的"已完整"是**开跑那一瞬的快照**——
/// 那时本次下载还没发生，`ok` 还空着，两组**必然不重叠**。同一份判据换个来源，
/// 相加的项就得跟着换（[`complete_not_accounted_by`] 就是在做这次换算）。
///
/// ⚠️ 这里**仍然用不着** `converged`：它算的是"快照 + 本次"那套账，与本函数不是同一份
/// 来源。硬套会让人以为两处判据是同一个东西，而本轮修的正是"以为可以相加"。
fn reconfirm_verdict(fresh_tree: &Value, fresh_verify: &Value) -> bool {
    let c = verify_counts(fresh_verify);
    // ⚠️ **没有 `c.ok`**：见上面那段 —— `ok ⊆ flat_complete_count(fresh_tree)`。
    all_good_of(fresh_verify) && c.unverifiable + flat_complete_count(fresh_tree) >= flat_len(fresh_tree)
}

/// 复核不成立、并且**已经把缺的重新入队之后**，这一拍该怎么走（纯函数，R26）。
///
/// 返回 `Some(退出码)` = **停下来报这个码**；`None` = 继续轮询。
///
/// ⚠️ **为什么复核不成立之后必须自己重新入队**（裁定 R26）：那一刻那批文件
/// **不在任何一类失败里**（它们从没被交给校验——`all_good` 仍为真），
/// 内核也**不会**替我们重新入队（`spawn_verify_worker` 只重入队**校验不符**的）。
/// 光"继续等"就是**永久等一个永远不会变的状态**——与"报假成功"同样不可接受，
/// 而且它是**可检出**的，可检出就必须有出路。
/// 捡回来的语义与"再跑一次会接着下"**同一条**：`paths: []` = 全部待下载。
///
/// ⚠️ **兜底不许少**：若这一次 `added` 为空**且**没有活跃任务 ⇒ 那是个**真的说不通的状态**
/// （内核说"没有待下载的"，而我们说"有文件不完整"）⇒ 如实报告并退出码 `5`，
/// **不许再回去接着等**。
///
/// （spec 那条兜底写的是三个条件——"`added` 为空**而仍未收敛**、且没有活跃任务"。
/// **"仍未收敛"这一条是构造上成立的**：能进到这一步就说明复核刚刚判过"不收敛"
/// （`reconfirm_verdict` 为假），所以这里不必再传一个布尔量进来。）
///
/// `added` = 这次 `enqueue` 回执里 `added` 的条数；`active` = 这一拍的活跃任务数。
fn after_bad_recheck(added: usize, active: usize) -> Option<ExitCode> {
    if added == 0 && active == 0 {
        Some(ExitCode::Incomplete)
    } else {
        None
    }
}

/// 复核不成立、又没走到"说不通"那一步时，收的那一句话（纯函数，修复轮 5）。
///
/// ⚠️ **两条支各自说的是自己的事实**：调用点能走到这里只有两种情形——
/// ① `added > 0`：这次**真的**把缺的加进队列了；② `added == 0`（而 `active > 0`，
/// 否则上面那条兜底已经 `return` 了）：这次**一个都没加**，是本来就有任务在跑。
///
/// 而修之前只有**一句话**（"已经把它重新加入下载，继续等"）走两支：第 ② 支**什么都没加**
/// 却宣称加了 —— 那是**把没发生的事说成发生了**，与"把自己的话标成内核原文"同一类。
/// 判据只有一处（这里读的就是 `after_bad_recheck` 读过的那个 `added`），
/// 所以两支的事实不可能与那一句对不上。
fn recheck_note(added: usize) -> &'static str {
    if added > 0 {
        "benagen-dl: 复核时发现这批文件已经不再完整（有文件不在盘上或被改动过）\
         —— 已经把缺的重新加入下载，继续等。"
    } else {
        "benagen-dl: 复核时发现这批文件已经不再完整（有文件不在盘上或被改动过）\
         —— 这次没有新加的任务（已经有任务在跑），继续等。"
    }
}

/// 校验失败**现在就能下结论吗**（纯函数，R-2）。
///
/// 入参是"这一拍看起来是个已经定下来的失败态"的那个记号（见调用点的 `now_failure`）：
/// `Some(计数)` = 没有活跃任务、没有任务级失败、而 `all_good` 已经不成立；`None` = 不是。
///
/// ⚠️ **为什么要连续两拍**（修复轮 2 的 R-2）：校验 worker 的次序是
/// **先 `commit` 第一轮结果（锁内）、再 `d.add` 重新入队（锁外，几次 RPC）**。
/// 这两步之间 `active == 0`、`failed == 0`、而 `!all_good` —— 主循环（200 ms 一拍）
/// **有可能正好在这一拍里看见它**，于是把一次**马上就会重试**的运行判成
/// `VerifyFailed`(6) 退出。那个窗口是毫秒级，但它**把可能成功的报成了失败**，
/// 与 R-1 是同一类错（错退出码）。
///
/// 重试的那几次 `add` 在毫秒内就完成 ⇒ 再等一拍（200 ms）之后 `active` 必然已经非零，
/// `now_failure` 变成 `None` ⇒ 不会下结论。**这不是"随便加一拍延迟"**：
/// 它精确地跨过那个由"先 commit 再 add"造成的窗口。
///
/// ⚠️ 要求两拍的计数**一模一样**（不是只要求两次都 `!all_good`）：计数动了就是有进展，
/// 那是"还没定下来"，不该在这一拍下结论。
fn failure_is_conclusive(prev: Option<VerifyCounts>, now: Option<VerifyCounts>) -> bool {
    now.is_some() && prev == now
}

/// `get_tree` 的 `flat` 里**完整的**文件数。
///
/// `flat` 的每一条本来就带 `state`（`op_get_tree` 给的，取值是 `TaskState` 的小写串：
/// `pending` / `downloading` / `complete` / `failed`）。取 `complete` 的条数即可——
/// 调用方在**开跑时**取一次，那里它就是"**运行前**就已完整"那个数；复核时取的是新鲜的
/// `get_tree`，那时它是"**现在**盘上完整"那个数（同一个函数，两个时刻 —— 相加的项
/// 因此不同，见 [`reconfirm_verdict`] 与 [`complete_not_accounted_by`]）。
///
/// ⚠️ **私有**（修复轮 5 收回）：它只服务本文件的判据，模块外没有消费者；
/// 本仓库为"公开面被无谓扩大"立过规矩（`engine::daemon` 那两处 `pub` 是**有 bin 要够**
/// 才开的，理由写在它们的 doc 上）。测试走 `super::*`，收成私有照样钉得住。
fn flat_complete_count(tree: &Value) -> usize {
    tree.get("flat")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| f.get("state").and_then(Value::as_str) == Some("complete"))
                .count()
        })
        .unwrap_or(0)
}

/// 复核之后把"已完整"**塞回主判据**时，该塞多少（纯函数，修复轮 5）。
///
/// 主判据是 `ok + unverifiable + already_complete >= total_files`，它的 `already_complete`
/// 原本取自**开跑那一瞬**的快照 —— 那时本次下载还没发生、`ok` 还空着，两组**必然不重叠**
/// （[`converged`]）。而复核之后手里这份完整集是**新鲜的**：本次验过的**必然也在里面**
/// （包含关系与它的证据链见 [`reconfirm_verdict`]）⇒ 原样塞回去就是把 `ok` 数第二遍。
/// 后果不是"判错退出码"，而是**每一拍都"看起来成功"**：每拍重取一次 O(n) 的 `get_tree`、
/// 每拍**重发一次** `enqueue`、每拍打一行 —— 而那两件事的注释里都写着"只发一次""不要每拍都做"。
///
/// 所以这里做一次**集合差**：新鲜完整集里，**没有被 `ok` / `unverifiable` 交代过**的那些。
/// 摘干净之后主判据的三个加数**两两不相交**，"相加"就等于"求并集"，
/// 算的是**被交代过的文件总数**，不会再凭空多出余量。
///
/// ⚠️ `unverifiable` 也一并摘掉：它已经由判据里自己那一项数着了（它本来就不在 `complete`
/// 里，摘它只是把"不重叠"这件事做成**构造上成立**，不靠"它俩恰好不相交"这个观察）。
fn complete_not_accounted_by(tree: &Value, verify: &Value) -> usize {
    let paths = |key: &str| -> Vec<&str> {
        verify
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    };
    let accounted: BTreeSet<&str> = paths("ok").into_iter().chain(paths("unverifiable")).collect();
    tree.get("flat")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| f.get("state").and_then(Value::as_str) == Some("complete"))
                .filter(|f| {
                    !f.get("path")
                        .and_then(Value::as_str)
                        .is_some_and(|p| accounted.contains(p))
                })
                .count()
        })
        .unwrap_or(0)
}

/// 这一拍有没有**进展**（**纯函数**）。
///
/// 两个信号取**并集**：
///   ① `done_bytes` 变了（传输侧）；
///   ② 校验六类计数（`VerifyCounts`）变了（校验侧）。
///
/// ⚠️ **两个都要**：下完之后 `done_bytes` 会**冻结**，而校验正好发生在它之后
/// （大文件的 crc64 是 CPU 密集、单个文件不可再分）——只认 ① 会把"校验还在慢慢推进"
/// 判成"卡住"（C-1 那个"把成功报成 5"）。
///
/// ⚠️ **抽成纯函数是修复轮 1 的 ⑥**：审查者实测"把并集那一整块删掉，236 条测试全绿"——
/// 它承重却只靠读代码。判据是纯的，就该被纯地钉住（R22 那一课）。
fn made_progress(
    prev_bytes: i64,
    now_bytes: i64,
    prev_counts: VerifyCounts,
    now_counts: VerifyCounts,
) -> bool {
    now_bytes != prev_bytes || now_counts != prev_counts
}

/// `enqueue` 回执里 `rejected` 那几条的**人话**（**纯函数**）。
///
/// ⚠️ **不许把它丢掉**（修复轮 1 的 ③）：`op_enqueue` 的 `rejected` 说的是
/// "**这些文件根本没能进队列**"（路径不安全 / `addUri` 失败）。丢掉它既违反
/// "不得静默少交"，也**必然导致不收敛**——那一批里少了几个任务，判据永远够不到总数。
/// `path` 与 `reason` 都是**内核原文**，逐字打出去（全局约束 8）。
/// `enqueue` 回执里若有人被拒 ⇒ **逐条打出来并判定失败**（返回 `Some(退出码)`）。
///
/// ⚠️ **两个调用点共用这一份**（启动时那一次、复核不成立补发那一次，裁定 R27）：
/// 措辞与判据都只有一处，不会各写一套。丢掉 `rejected` 就是**静默少交**——
/// 那些文件既没进队列、也不会出现在任何一类校验结果里，客户永远等不到它们。
fn report_rejections(rejected: &[Value]) -> Option<ExitCode> {
    if rejected.is_empty() {
        return None;
    }
    eprintln!(
        "\nbenagen-dl: 有 {} 个文件没能进入下载队列（下面的行是内核原文）：",
        rejected.len()
    );
    for line in rejection_lines(rejected) {
        eprintln!("  {line}");
    }
    Some(ExitCode::Incomplete)
}

fn rejection_lines(rejected: &[Value]) -> Vec<String> {
    rejected
        .iter()
        .map(|r| {
            let path = r
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("（内核没给路径）");
            let reason = r.get("reason").and_then(Value::as_str).unwrap_or("");
            if reason.is_empty() {
                path.to_string()
            } else {
                format!("{path} —— {reason}")
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 停滞判据：**两个信号、两段分流**（控制者裁定，见 `stall_action`）
// ---------------------------------------------------------------------------

/// 多久没有任何**进展信号**算停滞。
const STALL_AFTER: Duration = Duration::from_secs(600);

/// 校验六类计数（`verify_status` 的六个数组的长度）。
///
/// ⚠️ **它整体是一个"进度信号"**：任意一类变了就是有进展（见 [`stall_action`]）。
/// 抽成结构体而不是六个散装的 `usize`，是为了让"变了没有"这件事**能直接比**——
/// 散装的话就得写六个 `!=`，加一类就会漏一个（而漏掉的那一类恰好是"最后才动"的那种）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VerifyCounts {
    ok: usize,
    bad: usize,
    missing: usize,
    size_mismatch: usize,
    unverifiable: usize,
    unreadable: usize,
}

/// 从 `verify_status` 的响应里取六类计数。缺键当 0（内核保证了形状，这里是纵深防御）。
fn verify_counts(v: &Value) -> VerifyCounts {
    let n = |k: &str| v.get(k).and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    VerifyCounts {
        ok: n("ok"),
        bad: n("bad"),
        missing: n("missing"),
        size_mismatch: n("size_mismatch"),
        unverifiable: n("unverifiable"),
        unreadable: n("unreadable"),
    }
}

/// 六类计数的**人话**（停滞那行与校验失败那行共用一种写法）。
fn counts_text(c: &VerifyCounts) -> String {
    format!(
        "通过 {}，不符 {}，缺失 {}，大小不符 {}，读不了 {}，无法校验 {}",
        c.ok, c.bad, c.missing, c.size_mismatch, c.unreadable, c.unverifiable
    )
}

/// 停滞判据的**唯一退出决策点**（纯函数，所以它可测）。
///
/// `idle` 是"距上次任何一个进度信号变化"的时长；`has_active_tasks` 是
/// `poll_once` 给出的活跃任务数（含 `waiting`）是否非零——它同时也是**阶段的判据**：
/// 还有活跃任务 ⇒ 下载阶段；没有 ⇒ 校验阶段。
///
/// 返回 `Some(退出码)` = 该停下；**`None` = 继续等**。
///
/// ⚠️ **为什么是 `Option<ExitCode>` 而不是一个"三态枚举"**（控制者裁定 R22）：
/// 枚举那种形状下，调用点必然长成一个多臂 `match`，而"校验阶段那一臂**必须记得别
/// 写 `return`**"就成了一条**只靠人记得**的纪律——将来有人补一行
/// `return ExitCode::Incomplete`，回来的就是"把成功报成 5"（本函数存在的全部理由）。
/// 换成 `Option<ExitCode>` 之后，**校验阶段在类型上就给不出退出码**：
/// 调用点只剩一条 `if let Some(code) = … { return code; }`，**没有第二个臂可写错**；
/// 而"校验阶段永远是 `None`"这条性质落进下面的单测，成了**机械可钉的**。
///
/// **它只判定、不打印**：汇报（[`verify_stall_lines`]）留在调用点，因为
/// "上一次汇报是什么时候"那份节流状态在壳里。
///
/// ⚠️ **为什么不给校验阶段一个退出码**：那一刻两个候选码**都会说假话**——
/// `5` 的收口文案是「下载没有完成——再跑一次会接着下」（而下载其实早就完成了），
/// `6` 是「校验不通过」（而它**没有**不通过，只是慢：大文件的 crc64 是 CPU 密集、
/// **单个文件不可再分**，几十 GB 的一个文件验十几分钟完全正常）。
/// 在"只能撒谎"和"保持沉默"之间选沉默，但要把话说清楚（见 [`verify_stall_lines`]），
/// 并且 **Ctrl-C 这条出口一直都在**（每拍开头就检查）。
///
/// 对照：**下载阶段照旧退出码 5**——任务还在排队/传输中却十分钟一个字节都没推进，
/// 就是真的出事了。
fn stall_action(idle: Duration, has_active_tasks: bool) -> Option<ExitCode> {
    if idle < STALL_AFTER {
        return None;
    }
    if has_active_tasks {
        Some(ExitCode::Incomplete)
    } else {
        None
    }
}

/// 该不该**如实汇报**校验阶段的停滞（纯函数）。
///
/// ⚠️ 它与 [`stall_action`] **不是二选一**，而是两件事：`stall_action` 决定**退不退出**
/// （校验阶段恒 `None`），这个决定**要不要说话**。
/// 它返回 `bool`、**没有任何退出语义**——从签名上就看不出一条通往进程退出的路。
fn verify_stalled(idle: Duration, has_active_tasks: bool) -> bool {
    idle >= STALL_AFTER && !has_active_tasks
}

/// 下载 + 校验，返回退出码。**与收尾解耦**，这样每条出口都只是 `return`
/// （收尾只有 [`run`] 一处，不可能漏）。
fn download_and_verify(kernel: &Arc<Mutex<Kernel>>, opts: &Options) -> ExitCode {
    let mut n = 1u64;
    let mut next = || {
        n += 1;
        n
    };

    // ── 拉清单（最坏 91.5 秒，在 `op_load_delivery` 里于锁外跑）──────────────
    let mut params = json!({ "code": opts.code });
    if let Some(b) = &opts.base_url {
        params["base_url"] = json!(b);
    }
    if let Err(c) = call(kernel, next(), "load_delivery", params) {
        return c;
    }

    let tree = match call(kernel, next(), "get_tree", json!({})) {
        Ok(v) => v,
        Err(c) => return c,
    };
    // 收敛判据那两条要用它。**两个数都在这里取一次**：`total_files` 是分母，
    // `already_complete` 是"开跑前就完整"的那部分（见 `converged` 的 ①）。
    // ⚠️ **两个都是 `mut`**：复核发现"这批已经不再完整"时，它们会被换成复核那一份
    // （R-1 —— 不换的话下一拍又会去复核一次，而 `get_tree` 是 O(n) 的）。
    let mut total_files = flat_len(&tree);
    let mut already_complete = flat_complete_count(&tree);

    // 空数组 = 全部待下载（与图形界面「全部下载」同一条语义，同一个 `op_enqueue`）
    let enqueued = match call(kernel, next(), "enqueue", json!({ "paths": [] })) {
        Ok(v) => v,
        Err(c) => return c,
    };
    // ⚠️ **回执里的 `rejected` 不许丢**（修复轮 1 的 ③）：那是"这些文件根本没能进队列"
    // （路径不安全 / `addUri` 失败）。丢掉它既违反"不得静默少交"，也**必然导致不收敛**
    // ——那一批里少了几个任务，判据永远够不到总数，配上"校验阶段不许退出"就是永久挂住。
    let rejected = items_of_key(&enqueued, "rejected");
    if let Some(code) = report_rejections(&rejected) {
        return code;
    }

    let tty = std::io::stdout().is_terminal(); // std::io::IsTerminal（1.70+，无需 crate）
    // ── 停滞判据的两个信号 ────────────────────────────────────────────────
    // `last_progress` 是**两个信号取并集**后的"上次有进展"时刻：
    //   ① `done_bytes` 变了（传输侧，见下面的 `last_bytes`）；
    //   ② 校验六类计数变了（校验侧，见下面的 `last_counts`）。
    //
    // ⚠️ **少了 ② 会误报**：所有任务下完之后 `done_bytes` 会**冻结**，而校验正好发生在
    // 那之后——大文件的 crc64 是 CPU 密集、单个文件不可再分，几十分钟完全正常。
    // 只认 ① 的话，会把"校验还在慢慢推进"判成"卡住"（见 `stall_action`）。
    let mut last_progress = Instant::now();
    let mut last_bytes = -1i64;
    let mut last_counts = VerifyCounts::default();
    // 上一次**如实汇报**校验阶段停滞的时刻。它单独一个（不复用 `last_progress`）：
    // 汇报本身**不是进展**，所以不刷新 `last_progress`——否则报出来的"已经多久没变了"
    // 永远是 10 分钟（而不是真的等了多久），而且会一直重复报。
    // （`//` 不是 `///`：`let` 语句上的文档注释是 `unused_doc_comments` 警告。）
    let mut last_report = Instant::now();
    let mut prev_percent = -1i64;
    // 上一拍的"待定失败态"记号（见 [`failure_is_conclusive`] 的 R-2）。
    // `None` 开头 ⇒ **第一拍永远不下结论**，至少要两拍。
    let mut prev_failure: Option<VerifyCounts> = None;

    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            // 状态文件本来就是每拍原子写，所以这里**不做任何落盘**——
            // "再跑一次会接着下"靠的是已经落盘的那份状态。
            eprintln!("\nbenagen-dl: 被 Ctrl-C 中断 —— 再跑一次同样的命令会接着下");
            return ExitCode::Interrupted;
        }

        // ── 第一段：**收敛判据**（控制者裁定 R30）—— 它**不需要引擎**，必须排在传输侧之前 ─
        //
        // ⚠️ **这一段的位置是一条真端到端发现的洞**（`core/tests/cli_e2e.rs` 的 R28）。
        //    原来的顺序是"先 `poll_once` 再判收敛"，而 `poll_once` 里的 `transfer_list`
        //    **需要引擎**。可是 `op_enqueue` 在"没有待下载"时**故意早退、不起引擎**
        //    （那是它的 D-2 语义：界面上「全部下载」在整批都完整时走的就是那条路），
        //    而**那正是**帮助文本公开承诺的"再跑一次同一条命令"（整批都已完整）。
        //    ⇒ `transfer_list` 必然回 `engine_not_started`，那个 `?` 把整个运行判成
        //    退出码 4（"引擎没起来——这是本机环境的问题"），**在收敛判据有机会求值之前**。
        //    ⇒ `converged` 里"开跑前就完整"那一项（修复轮 1 的 ①）**结构上不可达**，
        //    而这一态正是它存在的全部理由。
        //
        // 这一段的两个输入都**不碰引擎**：`verify_status` 只读内核的 `k.verify`；
        // 复核要的那份新鲜 `get_tree` 也只在磁盘上 stat（`recheck_complete_on_disk`），
        // 一个网络请求都不发。所以"整批已完整"这一态现在是：循环第一拍 ⇒ 收敛 ⇒ 退出码 0，
        // **引擎根本不需要起** —— 那才是这条路的正确形状。
        //
        // ⚠️ **不许**改用"捕获 `engine_not_started` 当成 0 个任务"来绕过：那是给错误的
        // 顺序打补丁，而且"没有观测"会被读成"没有活跃任务"（本仓库反复记账的形态）。
        let v = match call(kernel, next(), "verify_status", json!({})) {
            Ok(v) => v,
            Err(c) => return c,
        };
        let c = verify_counts(&v);
        let all_good = all_good_of(&v);

        if converged(all_good, c.ok, c.unverifiable, already_complete, total_files) {
            // ⚠️ **宣布成功之前再复核一次**（R-1）：上面那两个数取自**开跑那一瞬**的
            // 快照，而客户可能在下载途中删掉了某个"早已完整"的文件。复核会重取
            // `get_tree`（顺带走只 stat 的 `recheck_complete_on_disk`）与 `verify_status`，
            // 用**新鲜的**输入再判一次。**两拍都成立才宣布成功。**
            let id_tree = next();
            let id_verify = next();
            let fresh_tree = match call(kernel, id_tree, "get_tree", json!({})) {
                Ok(v) => v,
                Err(c) => return c,
            };
            let fresh_verify = match call(kernel, id_verify, "verify_status", json!({})) {
                Ok(v) => v,
                Err(c) => return c,
            };
            if reconfirm_verdict(&fresh_tree, &fresh_verify) {
                if !opts.quiet && tty {
                    println!(); // 把还在原地刷新的那一行收掉
                }
                // spec §2：成功时打印**落盘目录、文件数、总字节**。
                // ⚠️ **两个数同口径**（修复轮 5）：都取复核那一份新鲜 tree 的**同一批文件**——
                // 修之前文件数是整批（`total_files`）而字节是本次传输列表的（`view.total_bytes`），
                // 续传时会打出"5 个文件，1.2 GB"而 1.2 GB 只是其中 2 个文件。
                // 字节走 `progress::bytes`——与进度行同一个格式化口径（不另写换算）。
                let (files, batch_bytes) = success_tally(&fresh_tree);
                println!(
                    "benagen-dl: 全部完成并校验通过 —— {} 个文件，{}，{}",
                    files,
                    progress::bytes(batch_bytes),
                    opts.download_dir.display()
                );
                return ExitCode::Ok;
            }
            // 复核不成立 ⇒ 有文件"本该完整、现在不在了"。
            // ⚠️ 先把判据的输入换成**复核那一份**：否则下一拍还会"看起来成功"、
            // 于是每 200 ms 重取一次 `get_tree`（O(n)）——正是不能做的事。
            // 它同时让下面的重新入队**只发这一次**（下一拍 `converged` 不再成立）。
            // ⚠️ **换成"新鲜"的就必须跟着换口径**（修复轮 5）：主判据另外两项
            // （`ok` / `unverifiable`）数着的文件，新鲜完整集里**也有**；
            // 直接把整份新鲜完整集塞进去就是同一批数两遍 ⇒ 上面那两句的保证**同时失效**
            // （每拍重取 + 每拍重发）。所以走 [`complete_not_accounted_by`] 做集合差。
            // ⚠️ 一并**如实记下**这一支还剩的一条窄缝：若"已由 `ok` 交代过"的那个文件是
            // **在校验之后**才被删的（`ok` 的说法过期了，而新鲜 tree 推翻不了它——
            // 判据数的是并集），下一拍主判据仍会成立 ⇒ 这一支每拍重走一次，直到那个文件
            // 被补回来。**它不会造成假成功**（复核那一份判据仍然判不成立），只是噪声；
            // 要消掉它得把 `ok` 从主判据里彻底换成新鲜来源，那是每拍一次 O(n) 的 `get_tree`。
            total_files = flat_len(&fresh_tree);
            already_complete = complete_not_accounted_by(&fresh_tree, &fresh_verify);

            // ⚠️ **自己把它捡回来**（裁定 R26）：那些文件不在任何一类失败里，内核不会替我们
            // 重新入队（worker 只重入队**校验不符**的）。语义与"再跑一次会接着下"同一条：
            // `paths: []` = 全部待下载。
            let picked = match call(kernel, next(), "enqueue", json!({ "paths": [] })) {
                Ok(v) => v,
                Err(c) => return c,
            };
            let added = items_of_key(&picked, "added").len();

            // ⚠️ **这一支的 `rejected` 也要接住**（裁定 R27），与启动时那一次**同一形状、
            // 同一份措辞**（`report_rejections`）。走到这里还能被拒的文件：既不完整、
            // 也不是"待下载"（内核认为没活可干）⇒ 它**不在任何一类失败里**、
            // `all_good` 仍真、`accounted < total` ⇒ 没有这一下就又是一次**静默等待**，
            // 而且丢掉 `rejected` 本身就是"静默少交"。
            let rejected_now = items_of_key(&picked, "rejected");
            if let Some(code) = report_rejections(&rejected_now) {
                return code;
            }

            // 兜底（R26）：内核说"没有待下载的"，而我们说"有文件不完整" —— 说不通。
            // 只陈述事实、不替内核解释为什么对不上；**不许再回去接着等**。
            //
            // ⚠️ **这一支要自己看一眼传输侧**（R30 的连带改动）：收敛判据已经排到
            // `poll_once` 前面，所以走到这里时**本拍还没观测过**"引擎里有没有活着的任务"。
            // 观测的代价只是一次 `transfer_list`；**不把"没观测到"读成"没有活跃任务"**
            // （那会把"还在下"读成"没活干"）—— 真没引擎可看时，这一支的结论本来就是"停下"，
            // 那就换一条**不依赖观测**的出口：同一个退出码 5（`after_bad_recheck` 的兜底
            // 也是 5），同一句话，理由见下。
            //
            // ⚠️ **必须按具体的码分派**（审查修复轮 1 的 ①），不能写成 `Err(ExitCode::Engine)`：
            // 那个退出码还承接 `engine_start_failed` / `engine_disconnected` /
            // `engine_rpc_failed` / `protocol_mismatch` / `internal` —— 那些是**引擎层真的坏了**
            // （该按 4 退），把它们一起读成"没有引擎可观测 ⇒ 停下"就是把真故障报成了
            // "两边对不上"。所以这里走 [`call_full`]，只有 `engine_not_started` 才换出口。
            let verdict = match call_full(kernel, next(), "transfer_list", json!({})) {
                Ok(tl) => {
                    let (active, _, _) = tally(&tl);
                    after_bad_recheck(added, active)
                }
                // 引擎**没起来**（`enqueue` 因为没活干而故意没起，那是合法状态）
                // ⇒ 传输侧无从观测 ⇒ 按"两边对不上"停下。
                // ⚠️ 这不是把错误读成 0：它是**换了一条不需要观测的出口**，
                // 而 5 的含义（"下载没有完成——再跑一次会接着下"）在这一态成立。
                Err((ExitCode::Engine, code)) if code == kcodes::ENGINE_NOT_STARTED => {
                    Some(ExitCode::Incomplete)
                }
                // 其余（含其它引擎层故障）**原样按它自己的退出码退**。
                Err((c, _)) => return c,
            };
            if let Some(code) = verdict {
                eprintln!(
                    "\nbenagen-dl: 复核说这批文件不完整，而内核说没有待下载的文件 —— 两边对不上，停下。"
                );
                eprintln!("  现在：{}", counts_text(&c));
                return code;
            }
            // 继续轮询，节拍自然恢复。⚠️ 收的这一句按 `added` **分两支**（见 `recheck_note`）：
            // 没加进去的那一支不许说"已经加入下载了"。
            eprintln!();
            eprintln!("{}", recheck_note(added));
        }

        // ── 第二段：传输侧。**需要引擎**，所以只在"还没收敛"之后才走 ──────────────
        let (active, failed, view) = match poll_once(kernel, next()) {
            Ok(v) => v,
            Err(c) => return c,
        };

        if !opts.quiet {
            let s = progress::line(&view);
            if tty {
                // TTY：原地刷新，不掉历史
                print!("\r{s}");
                let _ = std::io::stdout().flush();
            } else if progress::should_emit(
                prev_percent,
                progress::percent(view.done_bytes, view.total_bytes),
                false,
            ) {
                // 非 TTY（重定向、管道、CI）：只在整数百分比变化时打一行，
                // 否则日志里会是一堆一模一样的字，有用的一行被冲得找不着。
                println!("{s}");
            }
        }
        let pct = progress::percent(view.done_bytes, view.total_bytes);
        if pct != prev_percent {
            prev_percent = pct;
        }

        // ── 两个进展信号取**并集**（见 [`made_progress`]）─────────────────────
        // 必须在**取完计数之后、判定停滞之前**：下完之后 `done_bytes` 冻结，
        // 校验阶段的唯一进展证据就是计数那一半。
        if made_progress(last_bytes, view.done_bytes, last_counts, c) {
            last_progress = Instant::now();
        }
        last_bytes = view.done_bytes;
        last_counts = c;

        // 这一拍看起来是不是一个"已经定下来的失败态"（没有活跃任务、没有任务级失败、
        // 而 `all_good` 已经不成立）。它要**连续两拍一模一样**才算数 —— 见 R-2。
        let now_failure = if active == 0 && failed == 0 && !all_good {
            Some(c)
        } else {
            None
        };

        if active == 0 {
            if failed > 0 {
                // 任务自己报了错 ⇒ 已经下不动了。**把内核原文真打出来**（修复轮 1 的 ④）：
                // 这里原先写的是"（下面的行是内核原文）"，而紧接着的输出其实是**壳自己的**
                // `ExitCode::message()` —— 壳的话被贴上了"内核原文"的标签，两行还同前缀，
                // 客户分不出谁说的。`error_message` 那条现成的路就在手边（`stalled_lines`）。
                eprintln!("\nbenagen-dl: 有 {} 个文件没能下完：", failed);
                let items = transfer_items(kernel, next());
                for line in stalled_lines(&items, "（内核没有给出失败原因）") {
                    eprintln!("  {line}");
                }
                return ExitCode::Incomplete;
            }
            // ⚠️ 判据是 `!all_good` 本身（**不是**手数那几类，修复轮 1 的 ②），
            // 而且**要连续两拍都成立**（修复轮 2 的 R-2，见 `failure_is_conclusive`）。
            if failure_is_conclusive(prev_failure, now_failure) {
                eprintln!("\nbenagen-dl: 校验没通过 —— {}", counts_text(&c));
                return ExitCode::VerifyFailed;
            }
            // 没有活跃任务、也没问题项，但还没验完（或还没定下来）⇒ 校验 worker 正在跑
            // （它是最坏几十分钟的 CRC64 磁盘 I/O），继续等 —— 由下面的停滞判据兜底。
        }

        // 记下这一拍的待定失败态，供下一拍判"连续两拍"（R-2）。
        prev_failure = now_failure;

        // ── 停滞判据 ────────────────────────────────────────────────────────
        // 放在这里而不是循环开头：① 收敛与"已经明确失败"那两条出口要先说话
        //（它们给出的结论比"卡住了"精确得多）；② 校验阶段那一支要用这一拍的计数。
        //
        // ⚠️ **这一段里只有一个 `return`，它的值只可能来自 `stall_action`**——
        // 而 `stall_action` 在**校验阶段永远返回 `None`**（R22：类型上给不出退出码）。
        // 这就是"校验阶段不许退出"从"纪律"变成"类型"的地方：**没有第二个臂可写错**。
        if let Some(code) = stall_action(last_progress.elapsed(), active > 0) {
            // 下载阶段：真的卡住了，停。（文案与判据都**照旧**，一个字没改。）
            eprintln!("\nbenagen-dl: 连续 10 分钟没有任何进展，停下。停着的是：");
            let items = transfer_items(kernel, next());
            for line in stalled_lines(&items, "（传输列表里没有还在进行中的条目）") {
                eprintln!("  {line}");
            }
            return code;
        }

        // 校验阶段（`stall_action` 已经说了"不许退出"）：如实汇报，然后继续等。
        // 汇报按 `STALL_AFTER` 节流——不节流的话每 200 ms 一行，把界面上别的东西冲没了。
        // ⚠️ `last_progress` **不在这里刷新**：汇报**不是进展**，所以报出来的
        // "已经多久没变了"是**真的**等了多久，而不是每次都恰好 10 分钟。
        if verify_stalled(last_progress.elapsed(), active > 0)
            && last_report.elapsed() >= STALL_AFTER
        {
            last_report = Instant::now();
            let items = transfer_items(kernel, next());
            eprintln!();
            for line in verify_stall_lines(&items, &c, last_progress.elapsed()) {
                eprintln!("{line}");
            }
        }

        std::thread::sleep(Duration::from_millis(200)); // 与图形界面同一拍
    }
}

/// 从 `transfer_list` 的响应里取条目。**两处（轮询与诊断）共用的唯一一份**。
fn items_of(tl: &Value) -> Vec<Value> {
    items_of_key(tl, "items")
}

/// 从一份回执里取某个数组字段（缺键 / 类型不对 ⇒ 空）。`enqueue` 的
/// `added` / `rejected` 与 `transfer_list` 的 `items` 都走它。
fn items_of_key(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 只为**诊断**取一次任务列表：取不到就当"没有条目"（卡住那一刻不该再因为一次 RPC
/// 失败而什么都不说）。轮询路径**不要**用它，理由见 `poll_once` 里的注释。
fn transfer_items(kernel: &Arc<Mutex<Kernel>>, n: u64) -> Vec<Value> {
    let Ok(tl) = call(kernel, n, "transfer_list", json!({})) else {
        return Vec::new();
    };
    items_of(&tl)
}

/// 一拍：返回（活跃任务数、失败任务数、进度视图）。
fn poll_once(
    kernel: &Arc<Mutex<Kernel>>,
    n: u64,
) -> Result<(usize, usize, progress::View), ExitCode> {
    // ⚠️ 这里**保留 `?`**（`transfer_list` 失败就是这一拍失败），不要图省事换成
    // `transfer_items`——那个是给**诊断路径**用的，它把取不到当成"没有条目"。
    // 轮询路径上把一次 RPC 失败当成"列表是空的"，会被读成"全都下完了"。
    let tl = call(kernel, n, "transfer_list", json!({}))?;
    Ok(tally(&tl))
}

/// 从 `transfer_list` 的一份回执里数出（活跃任务数、失败任务数、进度视图）。
///
/// 抽出来是因为有两处要它：常规的 [`poll_once`]，以及复核不成立那一支
/// （那里必须是 [`call_full`] —— 它要按**具体的码**分派，见那个函数的说明）。
fn tally(tl: &Value) -> (usize, usize, progress::View) {
    let items = items_of(tl);
    let state_of = |i: &Value| {
        i.get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // ⚠️ `waiting` 也算活跃 —— 否则 `enqueue` 刚回来那一拍列表还空着（或者任务还在
    // 排队），会被误判成"下完了"而提前收工。
    let active = items
        .iter()
        .filter(|i| matches!(state_of(i).as_str(), "waiting" | "active"))
        .count();
    let failed = items.iter().filter(|i| state_of(i) == "error").count();
    let sum = |k: &str| {
        items
            .iter()
            .filter_map(|i| i.get(k).and_then(Value::as_i64))
            .sum::<i64>()
    };
    let view = progress::View {
        done_files: items.iter().filter(|i| state_of(i) == "complete").count(),
        total_files: items.len(),
        done_bytes: sum("completed"),
        total_bytes: sum("total"),
        speed: sum("speed"),
        failed,
    };
    (active, failed, view)
}

/// 卡住那一刻，把"还没结束的那些任务"逐条说清楚（路径 + 内核给的错误原文）。
/// 只说事实、不猜原因 —— 壳不知道 aria2 为什么停着，编不出来就别编。
///
/// ⚠️ **绝不返回空列表**（控制者裁定）：**校验阶段**停滞时所有任务都是 `complete`，
/// 按"还没结束"筛出来**一条都没有**——客户看到的就是一个没有下文的标题
/// （「停着的是：」后面什么都没有），人和脚本都拿不到任何线索。
/// 所以列表为空时用调用方给的 `fallback` 顶上去；校验阶段那里传的是**六类计数**
/// （那一刻唯一有信息量的东西）。
///
/// ⚠️ **抽成纯函数**（收 `&[Value]` 而不是 kernel）：这样"空列表不许返回"这条
/// 可以直接被单测钉住，而它原先要一台真引擎才碰得到。
/// **校验阶段**停滞那一刻要打的**那几行**（纯函数——文本本身因此可被单测钉住，
/// 而且能在报告里逐字贴出来）。
///
/// 三段：① 标题（含**已经多久没变了**）；② [`stalled_lines`] 给出的条目——校验阶段
/// 必然走到它的 `fallback`（所有任务都是 `complete`，逐条筛出来是空的），
/// 那正是**六类计数**；③ Ctrl-C 这条出口的指引。
///
/// ⚠️ **只有"说明"，没有"结论"**：这一刻我们**不知道**它会不会验完，所以不写
/// "失败了""卡住了"这种话——那是壳替内核编结论。说清事实，把出口指给客户，就这样。
fn verify_stall_lines(items: &[Value], counts: &VerifyCounts, idle: Duration) -> Vec<String> {
    let mut out = vec![format!(
        "benagen-dl: 下载都已经结束，卡在校验阶段 —— 已经 {} 分钟没有任何变化：",
        idle.as_secs() / 60
    )];
    out.extend(
        stalled_lines(items, &counts_text(counts))
            .into_iter()
            .map(|l| format!("  {l}")),
    );
    out.push(
        "benagen-dl: 大文件的 crc64 校验可能要很久（单个文件不可再分）。\
         按 Ctrl-C 可以停下，再跑一次会接着下。"
            .to_string(),
    );
    out
}

fn stalled_lines(items: &[Value], fallback: &str) -> Vec<String> {
    let lines: Vec<String> = items
        .iter()
        .filter(|i| {
            !matches!(
                i.get("state").and_then(Value::as_str).unwrap_or(""),
                "complete" | "removed"
            )
        })
        .map(|i| {
            let path = i
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("（内核没给路径）");
            let err = i
                .get("error_message")
                .and_then(Value::as_str)
                .unwrap_or("");
            if err.is_empty() {
                path.to_string()
            } else {
                format!("{path} —— {err}")
            }
        })
        .collect();
    if lines.is_empty() {
        vec![fallback.to_string()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// `--version` 那一行的三样都要在，而且**摘要必须是截断过的那 12 位**。
    ///
    /// 判别力：把它换成"打全 64 位"（客户要自己数前 12 位才能与缓存目录里的文件名对上）、
    /// 或者把版本号漏掉，都会红。`&sha[..12]` 与 `embed_hash_hex()` 同源这件事本身是
    /// 实现形态，不是这条测试的判据——判据是"行里出现的是**前 12 位**"。
    #[test]
    fn the_version_line_carries_the_version_and_a_truncated_digest() {
        let line = version_line();
        let sha = engine::daemon::embed_hash_hex();
        assert!(
            line.contains(env!("CARGO_PKG_VERSION")),
            "版本号不在里面：{line}"
        );
        assert!(line.contains(&sha[..12]), "摘要前 12 位不在里面：{line}");
        assert!(
            !line.contains(&sha[12..24]),
            "摘要没有被截断（第 13–24 位也出现了）：{line}"
        );
        assert!(!line.contains('\n'), "这是一行，不带换行：{line}");
    }

    /// 收敛判据：`all_good` **单独**不许算数（spec §5 点名的那个陷阱）。
    ///
    /// ⚠️ 这条**守的是判据本身，不是它的调用点**：`download_and_verify` 传进来的
    /// `total_files` / `already_complete` 取自 `get_tree` 的 `flat`，那一步接线没有单测
    /// （它要一台真引擎 + 真服务端）——由端到端测试（T7）承重。这里能钉住的是：
    /// 任何一个"用裸 `all_good` 就算收敛"的实现都会当场红。
    #[test]
    fn convergence_needs_both_conditions_not_just_all_good() {
        // 刚起来那一态：`CheckResult` 全空 ⇒ 内核的 `all_good()` 是 `true`，
        // 而一个文件都还没验。**这一条就是那个陷阱本身。**
        assert!(!converged(true, 0, 0, 0, 3), "什么都没验 ⇒ 不许算收敛");
        // 验了一半
        assert!(!converged(true, 2, 0, 0, 3));
        // `unverifiable` 算数（没有 crc64 可比 ⇒ 重下也不产生），但它必须已下载
        assert!(converged(true, 2, 1, 0, 3));
        // 有问题项（`all_good == false`）时，计数对上了也不许收敛
        assert!(!converged(false, 3, 0, 0, 3));
        // 空批次（清单里一个文件都没有）：两条都成立 ⇒ 收敛。那不是"还没验"
        assert!(converged(true, 0, 0, 0, 0));
    }

    /// **修复轮 1 的 ①**：**续传**这条常规流必须能收敛。
    ///
    /// 这一条钉的是一个**改之前不可能成立**的状态：盘上 3 个文件早就完整
    /// （`load_delivery` 清空 `k.verify`，`enqueue` 又把它们跳过 ⇒ **本次一个都不会被验**）、
    /// 另外 2 个这次下完并验过。改之前第 2 条是 `ok + unverifiable == total_files`
    /// ⇒ `0 + 0 ≠ 3` 恒不成立 ⇒ 判据永不收敛 ⇒ 配上"校验阶段不许退出"就是**永久挂住**。
    ///
    /// 判别力：把 `already_complete` 那一项从 `converged` 里去掉，这条的第一组断言当场红。
    #[test]
    fn a_resumed_run_converges_on_what_was_already_complete() {
        // 3 个早就完整、2 个这次下完并验过 ⇒ 5/5，收敛
        assert!(converged(true, 2, 0, 3, 5), "续传：已完整的必须算数");
        // 极端形态：**一个文件都不用下**（全都早就完整）—— 这次 `ok` 是 0，仍要收敛
        assert!(
            converged(true, 0, 0, 5, 5),
            "全都早就完整 ⇒ 0 个本次校验过的也必须收敛（改之前这一态不可能成立）"
        );
        // 但"已完整"也不能凭空把事情说成做完了：还不够数就不许收敛
        assert!(!converged(true, 0, 0, 4, 5), "还差一个 ⇒ 不收敛");
        // ⚠️ 三组**会重叠**（同一个文件既"早就完整"又被本次验到）⇒ 判据是 `>=` 不是 `==`：
        // 要求恰好相等会把合法的成功判成不收敛。
        assert!(converged(true, 3, 0, 3, 5), "重叠计数（3+3 > 5）也应收敛");
    }

    /// `flat_complete_count` 数的是 `flat` 里 `state == "complete"` 的条数。
    ///
    /// 判别力：把判据写成别的字段、或忘了过滤（数成 `flat.len()`），下面几条会红。
    /// 尤其是最后一条：**数成"全部"会让任何批次一启动就收敛** —— 那正是这个函数的
    /// 危险方向（与 `all_good` 那个陷阱同源）。
    #[test]
    fn flat_complete_count_counts_only_the_complete_ones() {
        let tree = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "pending"},
            {"path": "c", "state": "complete"},
            {"path": "d", "state": "downloading"},
            {"path": "e", "state": "failed"},
        ]});
        assert_eq!(flat_complete_count(&tree), 2);
        // 一个都没有（全新批次）——**这才是刚开跑时该有的数**
        assert_eq!(flat_complete_count(&json!({"flat": []})), 0);
        // 形状不对时当 0（不是 panic，也不是"全算完整"）
        assert_eq!(flat_complete_count(&json!({})), 0);
        assert_eq!(flat_complete_count(&json!({"flat": "不是数组"})), 0);
    }

    /// **修复轮 5**：复核之后塞回主判据的那份"已完整"**不许把本次验过的再数一遍**。
    ///
    /// 判别力：把 `complete_not_accounted_by` 换回 `flat_complete_count`（塞进整份新鲜
    /// 完整集 = 本次修之前的形态），第 ① 组的 `converged` 断言当场红 —— 那一拍就会
    /// "看起来成功"、于是每 200 ms 重取一次 `get_tree` 并重发一次 `enqueue`。
    /// 反方向（把"没被交代过的"也一起摘掉、塞 0）⇒ 第 ② 组红：续传会被永久挂住。
    #[test]
    fn the_swapped_in_complete_count_does_not_double_count_the_verified_ones() {
        // ① 最小复现那一态：`a` 被删（pending）、`b` 本次验过（ok）⇒ 新鲜的完整集里
        //    那一个已经由 `ok` 交代过 ⇒ 没被交代的是 0 ⇒ 主判据 1 < 2 ⇒ **不再进复核**
        let a_deleted = json!({"flat": [
            {"path": "a", "state": "pending"},
            {"path": "b", "state": "complete"},
        ]});
        let ok_b = json!({"ok": ["b"], "all_good": true});
        assert_eq!(
            complete_not_accounted_by(&a_deleted, &ok_b),
            0,
            "b 已经由 ok 交代过 ⇒ 没被交代的是 0（塞成 1 就是把 ok 数了第二遍）"
        );
        assert!(
            !converged(
                true,
                1,
                0,
                complete_not_accounted_by(&a_deleted, &ok_b),
                flat_len(&a_deleted)
            ),
            "换成新鲜完整集之后判据还成立 ⇒ 每一拍都会重取 get_tree、重发 enqueue"
        );

        // ② 反方向：续传（3 个早已完整 + 2 个本次验过）——那 5 个里只有 3 个没被交代过，
        //    主判据照样 5/5 ⇒ 复核不许把正常的续传挡在外面
        let resumed = json!({"flat": [
            {"path": "a", "state": "complete"}, {"path": "b", "state": "complete"},
            {"path": "c", "state": "complete"}, {"path": "d", "state": "complete"},
            {"path": "e", "state": "complete"},
        ]});
        let resumed_verify = json!({"ok": ["d", "e"], "all_good": true});
        assert_eq!(complete_not_accounted_by(&resumed, &resumed_verify), 3);
        assert!(converged(
            true,
            2,
            0,
            complete_not_accounted_by(&resumed, &resumed_verify),
            flat_len(&resumed)
        ));

        // ③ `unverifiable` 那一类也一并摘掉（它由判据里自己那一项数着）⇒ 不重叠
        let with_unver = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "c", "state": "complete"},
        ]});
        let uv = json!({"unverifiable": ["c"], "all_good": true});
        assert_eq!(complete_not_accounted_by(&with_unver, &uv), 1);
        assert!(converged(
            true,
            0,
            1,
            complete_not_accounted_by(&with_unver, &uv),
            flat_len(&with_unver)
        ));

        // ④ 形状不对时当 0（不是 panic）——与 `flat_complete_count` 同一条纵深防御
        assert_eq!(complete_not_accounted_by(&json!({}), &ok_b), 0);
        assert_eq!(complete_not_accounted_by(&json!({"flat": []}), &json!({})), 0);
    }

    /// **修复轮 5**：成功那一行的**两个数同口径**（都来自同一份 tree 的同一批文件）。
    ///
    /// 判别力：把字节改成 `progress.done_bytes`，或者换回 `transfer_list` 那份
    /// （本次入队的那几个文件）⇒ 第 ① 组断言当场红。
    ///
    /// ⚠️ **它守的是取数口径，不是"调用点真的传了复核那一份 tree"**——那一步在循环体里
    /// （要一台真引擎 + 真服务端），归 T7。这里能保证的是：**只要传进来一份 tree，
    /// 那一行里的两个数就必然出自同一批文件**。
    #[test]
    fn the_success_line_takes_both_numbers_from_the_same_batch() {
        // 续传现场的形态：整批 5 个文件，而本次只传了其中一部分的字节。
        // `progress.total_bytes` 是**整批**的和；`done_bytes` 与它不等（收尾那一刻才相等）
        // —— 取 `done_bytes`、或者另取一份"本次传输列表"的和，都会红。
        let tree = json!({
            "flat": [
                {"path": "a", "state": "complete"}, {"path": "b", "state": "complete"},
                {"path": "c", "state": "complete"}, {"path": "d", "state": "complete"},
                {"path": "e", "state": "complete"},
            ],
            "progress": {"total_bytes": 1_200_000_000i64, "done_bytes": 400_000_000i64},
        });
        assert_eq!(
            success_tally(&tree),
            (5, 1_200_000_000),
            "文件数是整批、字节也必须是整批（不许是本次传的那一部分）"
        );
        // 形状不对 ⇒ 0，不 panic（纵深防御）；数不出文件数时字节也不许"独立地"有数
        assert_eq!(success_tally(&json!({"flat": []})), (0, 0));
        assert_eq!(success_tally(&json!({})), (0, 0));
    }

    /// **修复轮 1 的 ⑥**：并集那处接线（原先审查者把整块删掉、236 条测试全绿）。
    ///
    /// 判别力：把实现改成只认字节（`now_bytes != prev_bytes`），第二组断言当场红；
    /// 改成只认计数，第一组红；改成恒 `true`，第三组红。
    #[test]
    fn progress_is_the_union_of_bytes_and_verify_counts() {
        let a = VerifyCounts::default();
        let b = VerifyCounts { ok: 1, ..a };
        // ① 字节变了、计数没变 ⇒ 有进展
        assert!(made_progress(100, 200, a, a));
        // ② 字节没变、计数变了 ⇒ **有进展**（下完之后 done_bytes 冻结，
        //    校验阶段唯一的进展证据就是它 —— C-1 那个"把成功报成 5"就死在这条上）
        assert!(made_progress(100, 100, a, b), "校验侧推进必须算进展");
        // ③ 两个都没变 ⇒ 没进展（停滞判据的输入）
        assert!(!made_progress(100, 100, a, a));
        assert!(!made_progress(0, 0, b, b));
        // ④ 两个都变了 ⇒ 有进展
        assert!(made_progress(100, 200, a, b));
        // ⑤ 六类里**任意**一类变了都算（不是只看某一类）
        for m in [
            VerifyCounts { ok: 9, ..a },
            VerifyCounts { bad: 9, ..a },
            VerifyCounts { missing: 9, ..a },
            VerifyCounts { size_mismatch: 9, ..a },
            VerifyCounts { unverifiable: 9, ..a },
            VerifyCounts { unreadable: 9, ..a },
        ] {
            assert!(made_progress(7, 7, a, m), "某一类变了却没算进展：{m:?}");
        }
    }

    /// **修复轮 2 的 R-1**：宣布成功之前那次**复核**必须能发现"这批已经不再完整"。
    ///
    /// 这一条钉的是 R-1 的**判据**（`reconfirm_verdict`）：它吃**新鲜的**两份响应。
    /// 判别力：把实现改成"恒 true"（= 相信旧快照）或"不再重取"，第一组断言当场红。
    ///
    /// ⚠️ 它守的是判据本身，**不是"调用点真的调了它"**——那一步在循环体里（要一台真引擎
    /// + 真服务端），归 T7。这里能保证的是：**只要它被调用，旧快照那一态就骗不过去**。
    #[test]
    fn the_recheck_catches_a_batch_that_stopped_being_complete() {
        // 场景：3 个文件开跑时**全都完整**（于是旧快照判成功），客户在途中删掉了 c。
        // 复核时 `recheck_complete_on_disk` 已经把 c 从 `complete` 里摘出去了
        // （它就长在 `get_tree` 那条路上），所以新鲜的 `flat` 说 c 是 pending。
        let fresh_tree = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "complete"},
            {"path": "c", "state": "pending"},   // ← 被删了
        ]});
        // 本次一个文件都没下过 ⇒ 六类全空、`all_good` 为真（那个陷阱态）
        let fresh_verify = json!({"ok": [], "all_good": true});
        assert!(
            !reconfirm_verdict(&fresh_tree, &fresh_verify),
            "复核必须发现它已经不再完整 —— 否则就是报成功而数据是缺的"
        );

        // 对照：三个都还在盘上 ⇒ 复核成立，放行
        let still_complete = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "complete"},
            {"path": "c", "state": "complete"},
        ]});
        assert!(reconfirm_verdict(&still_complete, &fresh_verify));

        // 续传那条常规流（3 个早就完整 + 2 个这次下完并验过）复核也必须放行，
        // 否则 R-1 会把 ① 修好的东西又堵回去
        let resumed = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "complete"},
            {"path": "c", "state": "complete"},
            {"path": "d", "state": "complete"},
            {"path": "e", "state": "complete"},
        ]});
        let resumed_verify = json!({"ok": ["d", "e"], "all_good": true});
        assert!(reconfirm_verdict(&resumed, &resumed_verify), "续传不许被复核挡住");

        // 复核那两拍里有问题项时也不行（`all_good` 为假）
        let bad_verify = json!({"ok": [], "bad": ["c"], "all_good": false});
        assert!(!reconfirm_verdict(&still_complete, &bad_verify));
    }

    /// **修复轮 5 的最小复现**：本次校验过的文件**不许**在复核里被数第二遍。
    ///
    /// 场景（审查者给的）：清单 2 个文件；`a` 开跑时就完整（`enqueue` 跳过它，
    /// 它**从没被交给校验**）；`b` 本次下完并校验通过（`ok = ["b"]`）；
    /// 客户在下载途中删掉了 `a`。复核时新鲜 tree 里 `a` 已被 `recheck_complete_on_disk`
    /// 摘掉（只剩 `b` 完整），而 `all_good` 仍真（`a` 谁也没验过）。
    ///
    /// ⚠️ **为什么旧判据必然放行**：`ok` 里那一个**也在**新鲜完整集里
    /// （`commit` 先把状态记录落进去、再置 `complete_dirty` ⇒ 下一次 `get_tree` 重算出的
    /// `complete` 必然含它）—— 把两组相加，`>=` 就凭空多出"本次验过几个就有几个"的余量。
    /// 于是"删掉一个早已完整的文件"这条缺陷一字未改地活着：**报成功而盘上缺一个**。
    /// 真实续传里余量更大（50 个已完整 + 50 个本次下完 ⇒ 50 个单位的余量 ⇒
    /// 任何一次对"先前已完整文件"的删除都会被吸收掉）。
    ///
    /// 判别力：把 `ok` 加回复核判据里（修复前的形态），①②③ 三组断言当场红。
    #[test]
    fn the_recheck_does_not_count_this_runs_verifications_twice() {
        // ① 最小复现：`a` 已被删（pending）、`b` 本次下完并验过 ⇒ **不成功**
        let a_deleted = json!({"flat": [
            {"path": "a", "state": "pending"},   // ← 被删了
            {"path": "b", "state": "complete"},  // ← 本次下完并验过
        ]});
        let ok_b = json!({"ok": ["b"], "all_good": true});
        assert!(
            !reconfirm_verdict(&a_deleted, &ok_b),
            "本次验过的那一个不许替被删掉的那一个顶账 —— 判成成功就是报成功而盘上缺一个"
        );
        // ② 反方向：两个都在盘上 ⇒ 必须放行（不许把判据收紧成"续传永远不成功"）
        let both = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "complete"},
        ]});
        assert!(reconfirm_verdict(&both, &ok_b), "两个都在盘上 ⇒ 复核必须放行");
        // ③ 本次下完的那个**自己**被删了 ⇒ 同样不许放行（余量在两个方向上都不成立）
        let b_deleted = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "b", "state": "pending"},   // ← 本次下完又被删了
        ]});
        assert!(
            !reconfirm_verdict(&b_deleted, &ok_b),
            "本次下完的那个没了，也不许报成功"
        );
        // ④ 一个都没下（`ok` 空）、删掉一个早已完整的 ⇒ 仍不成功（R-1 那一态，不许回归）
        assert!(!reconfirm_verdict(&a_deleted, &json!({"ok": [], "all_good": true})));
        // ⑤ 清单没给 crc64 的那些由 `unverifiable` 交代 —— 它们**不在** `complete` 里
        //    （`verify::check` 不给它们落状态记录）⇒ 这一项仍然承重，不许一起删掉
        let unver = json!({"flat": [
            {"path": "a", "state": "complete"},
            {"path": "c", "state": "pending"},
        ]});
        assert!(reconfirm_verdict(
            &unver,
            &json!({"unverifiable": ["c"], "all_good": true})
        ));
        // ⑥ 有问题项 ⇒ 一律不放行
        assert!(!reconfirm_verdict(
            &both,
            &json!({"ok": ["b"], "bad": ["a"], "all_good": false})
        ));
    }

    /// **裁定 R26**：复核不成立之后，什么时候继续轮询、什么时候必须停下来报 5。
    ///
    /// 判别力（两个方向都堵死）：
    /// * 改成恒 `None`（"永远接着等"）⇒ 第 3 组断言红，而那正是 R26 要防的
    ///   "永久等一个永远不会变的状态"；
    /// * 改成恒 `Some`（"一不成立就报错"）⇒ 第 1、2 组红 —— 客户明明能自己好起来。
    ///
    /// ⚠️ **它守的是决定表，不是"真的发了 enqueue"**：那一步是循环体里的一个调用
    /// （要一台真引擎 + 真服务端），归 T7。这里能保证的是：**只要发了，
    /// 三种结果各自走对的那条路**。
    #[test]
    fn a_failed_recheck_keeps_polling_only_when_something_is_queued_or_running() {
        // ① 重新入队成功 ⇒ 继续轮询（缺的已经在路上了）
        assert_eq!(after_bad_recheck(3, 0), None);
        // ② 这次没新加的，但本来就有任务在跑 ⇒ 继续轮询（别抢在它们前面下结论）
        assert_eq!(after_bad_recheck(0, 2), None);
        // ③ **对不上**：内核说"没有待下载的"、我们说"有文件不完整" ⇒ 停下来报 5
        assert_eq!(
            after_bad_recheck(0, 0),
            Some(ExitCode::Incomplete),
            "可检出的说不通状态必须有出路 —— 不许再回去接着等"
        );
    }

    /// **修复轮 5**：复核不成立之后收的那一句，**两支各自说的是自己的事实**。
    ///
    /// 判别力：把它改回"一句话走两支"（修复前的形态）⇒ 第 ② 组断言当场红 ——
    /// 那一刻 `added == 0`、**什么都没加**，而那句话宣称"已经把它重新加入下载"。
    #[test]
    fn the_recheck_note_only_claims_what_actually_happened() {
        // ① 真加进去了 ⇒ 说"已经把缺的重新加入下载"（并且说清是哪些：缺的那些）
        let added = recheck_note(2);
        assert!(added.contains("重新加入下载"), "加进去了就要说加进去了：{added}");
        assert!(!added.contains("没有新加"), "加进去了就别否认：{added}");
        // ② 一个都没加（缺的本来就在队列里/在跑）⇒ **不许**宣称加了
        let nothing = recheck_note(0);
        assert!(
            !nothing.contains("重新加入下载"),
            "一个都没加进去，却把没发生的事说成发生了：{nothing}"
        );
        assert!(
            nothing.contains("没有新加"),
            "这一支要如实说清「这次没新加」：{nothing}"
        );
        assert!(!nothing.contains('\n'), "这是一行（调用点自己补换行）：{nothing}");
    }

    /// **裁定 R27**：`rejected` 非空 ⇒ 判失败；空 ⇒ 不表态。
    ///
    /// 判别力：把它改成恒 `None`（= 丢掉 `rejected`，修复前的形态），第一组断言红。
    /// ⚠️ 这条守的是**决定**；那几行**文本**由 `rejected_files_are_reported_verbatim`
    /// 与 `rejection_lines` 守（`path` + `reason` 内核原文逐字）。
    /// 而"+两处调用点共用同一份措辞"这件事是**结构**——两个调用点各自 `return` 这个
    /// 函数的返回值，没有第二处措辞可写。
    #[test]
    fn rejected_files_are_a_failure_and_nothing_else_is() {
        assert_eq!(report_rejections(&[]), None, "没人被拒 ⇒ 不表态");
        assert_eq!(
            report_rejections(&[json!({"path": "a.txt", "reason": "x"})]),
            Some(ExitCode::Incomplete),
            "有文件没能进队列 ⇒ 判失败（静默少交是底线）"
        );
    }

    /// **修复轮 2 的 R-2**：校验失败要**连续两拍**才算数。
    ///
    /// 判别力：把 `failure_is_conclusive` 改成 `now.is_some()`（一拍就下结论 = 修复前的
    /// 形态），第二组断言当场红 —— 而那正是"把马上会重试的运行判成失败"的入口。
    #[test]
    fn a_verification_failure_needs_two_identical_ticks() {
        let a = VerifyCounts {
            bad: 1,
            ..Default::default()
        };
        let b = VerifyCounts {
            bad: 2,
            ..Default::default()
        };
        // ① 没有待定失败态（在下载、或 `all_good` 还成立）⇒ 永远不下结论
        assert!(!failure_is_conclusive(None, None));
        assert!(!failure_is_conclusive(Some(a), None), "失败态消失了 ⇒ 不下结论");
        // ② **第一拍不下结论**（上一拍还没有记号）——这就是那个毫秒级窗口的出口
        assert!(
            !failure_is_conclusive(None, Some(a)),
            "一拍就下结论 ⇒ 会把马上要重试的运行判成失败"
        );
        // ③ 连续两拍**一模一样** ⇒ 下结论
        assert!(failure_is_conclusive(Some(a), Some(a)));
        // ④ 两拍都在失败、但计数变了 ⇒ 那是**有进展**，还没定下来，再等
        assert!(!failure_is_conclusive(Some(a), Some(b)));
    }

    /// **修复轮 1 的 ③**：`enqueue` 回执里的 `rejected` 要能变成人话（内核原文逐字）。
    #[test]
    fn rejected_files_are_reported_verbatim() {
        let rejected = vec![
            json!({"path": "a/b.txt", "reason": "路径不安全（越界/控制字符/空段）"}),
            json!({"path": "c.txt", "reason": "aria2 拒绝：连接超时"}),
            json!({"path": "d.txt"}), // 内核没给 reason ⇒ 至少要有路径
        ];
        let lines = rejection_lines(&rejected);
        assert_eq!(lines.len(), 3, "一条都不许丢");
        assert_eq!(lines[0], "a/b.txt —— 路径不安全（越界/控制字符/空段）");
        assert!(lines[2].contains("d.txt"));
        assert!(rejection_lines(&[]).is_empty());
    }

    // -----------------------------------------------------------------------
    // 停滞判据（控制者裁定 C-1）：两个信号、两段分流
    // -----------------------------------------------------------------------

    /// `verify_counts` 把六类**都**读出来；缺键当 0。
    ///
    /// 判别力：漏读任何一类，它在下面的"计数变了算进展"里就永远不参与——
    /// 而漏掉的那一类恰好可能是"最后才动"的那种（`unreadable` 是最后才可能出现的）。
    #[test]
    fn verify_counts_reads_all_six_categories() {
        let v = json!({
            "ok": ["a", "b"],
            "bad": ["c"],
            "missing": [],
            "size_mismatch": ["d", "e", "f"],
            "unverifiable": ["g"],
            "unreadable": ["h", "i"],
            "all_good": false,
        });
        let c = verify_counts(&v);
        assert_eq!(
            c,
            VerifyCounts {
                ok: 2,
                bad: 1,
                missing: 0,
                size_mismatch: 3,
                unverifiable: 1,
                unreadable: 2,
            }
        );
        // 缺键（纵深防御）：全 0，不是 panic
        assert_eq!(verify_counts(&json!({})), VerifyCounts::default());
    }

    /// **六类里任意一类变了都算"有进展"** —— 这是校验阶段唯一的进度信号。
    ///
    /// 判别力：把 `VerifyCounts` 的比较改成只看其中几类，这里对应的那几行就红。
    /// 为什么它要紧：下完之后 `done_bytes` 冻结，校验阶段的"还在动"就只剩这一个证据；
    /// 少认一类，那一类正在推进的批次会被判成"卡住"。
    #[test]
    fn any_of_the_six_counts_counts_as_progress() {
        let base = VerifyCounts {
            ok: 1,
            bad: 2,
            missing: 3,
            size_mismatch: 4,
            unverifiable: 5,
            unreadable: 6,
        };
        assert_eq!(base, base, "自己与自己相等（前提）");
        let mutations = [
            VerifyCounts { ok: 99, ..base },
            VerifyCounts { bad: 99, ..base },
            VerifyCounts { missing: 99, ..base },
            VerifyCounts { size_mismatch: 99, ..base },
            VerifyCounts { unverifiable: 99, ..base },
            VerifyCounts { unreadable: 99, ..base },
        ];
        for (i, m) in mutations.iter().enumerate() {
            assert_ne!(base, *m, "第 {i} 类变了却没被认成「有进展」");
        }
    }

    /// **R22 的那条性质**：**校验阶段永远给不出退出码**（对"多久没变"这条轴**穷举**）。
    ///
    /// 这是"把纪律搬进类型"之后换来的东西：调用点只剩
    /// `if let Some(code) = stall_action(…) { return code; }`，**没有第二个臂可写错**；
    /// 而"校验阶段恒 `None`"在这里被机械钉住。
    ///
    /// 判别力（两个方向都堵死了）：
    /// * 把校验阶段那一支改成 `Some(…)`（"停滞就停"）⇒ 第 1 组断言红
    ///   —— 那正是"该报 0 的报成 5"的唯一入口；
    /// * 把整个函数改成**恒 `None`** ⇒ 第 2 组的**对照**断言红
    ///   —— 少了它，上面那一组会被一个什么都不做的实现白嫖过去。
    #[test]
    fn the_verify_phase_can_never_produce_an_exit_code() {
        // ① 校验阶段（没有活跃任务）在**任何**时长下都不给退出码，直到天长地久
        for secs in [0u64, 1, 599, 600, 601, 3_600, 86_400, 7 * 86_400] {
            let idle = Duration::from_secs(secs);
            assert!(
                stall_action(idle, false).is_none(),
                "{secs}s：校验阶段给出了退出码 —— 那一刻两个候选码都会说假话"
            );
        }
        // ② 对照：同一条时间轴上，**下载阶段**该退出就得退出
        assert_eq!(stall_action(Duration::from_secs(599), true), None);
        assert_eq!(
            stall_action(STALL_AFTER, true),
            Some(ExitCode::Incomplete),
            "下载阶段到了阈值却不退出"
        );
        assert_eq!(
            stall_action(Duration::from_secs(86_400), true),
            Some(ExitCode::Incomplete)
        );
    }

    /// `verify_stalled` 只决定**要不要说话**：判据与 `stall_action` 那条同源，
    /// 但它返回 `bool`、**没有任何退出语义**（签名上就看不出一条通往退出的路）。
    ///
    /// 判别力：把 `!has_active_tasks` 去掉（下载阶段也去汇报"卡在校验阶段"），
    /// 或者漏掉阈值判断，对应的断言当场红。
    #[test]
    fn verify_stalled_only_decides_whether_to_speak() {
        for secs in [0u64, 599, 600, 3_600] {
            let idle = Duration::from_secs(secs);
            assert_eq!(
                verify_stalled(idle, false),
                secs >= 600,
                "{secs}s：校验阶段的汇报时机不对"
            );
            // 下载阶段**永远不汇报**（那是另一条出口：直接停）
            assert!(!verify_stalled(idle, true), "{secs}s：下载阶段不该走汇报");
        }
    }

    /// `stalled_lines` **绝不返回空列表**（控制者裁定 C-1 的第 4 条）。
    ///
    /// 判别力：删掉那个 `if lines.is_empty()` 兜底，第一条断言当场红——
    /// 而它的现场是"所有任务都 complete、卡在校验阶段"，那时客户看到的是一个
    /// **没有下文的标题**，人和脚本都拿不到线索。
    #[test]
    fn stalled_lines_never_comes_back_empty() {
        // 校验阶段那一刻的真实形态：全部 complete ⇒ 筛完是空的
        let all_done = vec![
            json!({"state": "complete", "path": "a.txt"}),
            json!({"state": "removed", "path": "b.txt"}),
        ];
        let fallback = counts_text(&VerifyCounts {
            ok: 120,
            bad: 0,
            missing: 0,
            size_mismatch: 0,
            unverifiable: 3,
            unreadable: 0,
        });
        assert_eq!(stalled_lines(&all_done, &fallback), vec![fallback.clone()]);
        assert!(stalled_lines(&[], &fallback)[0].contains("通过 120"));
        // 连空 fallback 也不返回空列表（调用方给错也不该让客户看到一个空标题）
        assert_eq!(stalled_lines(&[], ""), vec![String::new()]);

        // 有没结束的任务时：逐条说清（路径 + 内核原文），**不用** fallback
        let stuck = vec![
            json!({"state": "active", "path": "a.txt", "error_message": ""}),
            json!({"state": "error", "path": "b.txt", "error_message": "连接超时"}),
            json!({"state": "complete", "path": "c.txt"}),
        ];
        assert_eq!(
            stalled_lines(&stuck, "不该出现"),
            vec!["a.txt".to_string(), "b.txt —— 连接超时".to_string()]
        );
    }

    /// 校验阶段停滞那一刻打的那几行：**三样都在**，且**不含结论**。
    ///
    /// 判别力：漏掉"多久没变"（客户无法判断该不该继续等）、漏掉六类计数（那一刻唯一的
    /// 事实）、漏掉 Ctrl-C 指引（客户不知道还有出口）——任意一样漏了这条就红。
    ///
    /// ⚠️ **不逐字钉整段文案**（那是措辞，不是契约；T4 的裁定同此）：钉的是"这三样
    /// 必须在"。所以改标点、改语序不会红，删掉一样会红。
    ///
    /// ⚠️ 也**不许**出现"失败/卡住/校验不通过"这类**结论**：壳不知道它会不会验完，
    /// 编不出来就别编（`download_and_verify` 里那两个退出码的教训）。
    #[test]
    fn the_verify_stall_note_says_what_is_known_and_offers_the_way_out() {
        let counts = VerifyCounts {
            ok: 120,
            bad: 0,
            missing: 0,
            size_mismatch: 0,
            unverifiable: 3,
            unreadable: 0,
        };
        // 校验阶段的真实形态：所有任务都 complete
        let all_done = vec![json!({"state": "complete", "path": "a.txt"})];
        let lines = verify_stall_lines(&all_done, &counts, Duration::from_secs(12 * 60));
        let text = lines.join("\n");

        assert!(text.contains("12 分钟"), "要有「已经多久没变了」：{text}");
        assert!(text.contains("校验阶段"), "要点明卡在哪一段：{text}");
        assert!(
            text.contains("通过 120") && text.contains("无法校验 3"),
            "要有六类计数：{text}"
        );
        assert!(
            text.contains("Ctrl-C") && text.contains("接着下"),
            "要指出 Ctrl-C 这条出口：{text}"
        );
        assert!(
            !text.contains("校验不通过") && !text.contains("下载没有完成"),
            "只报事实，不替内核下结论：{text}"
        );
        assert!(
            lines.len() >= 3,
            "标题 + 计数 + 指引，至少三行：{lines:?}"
        );
    }

    /// `counts_text` 的六个数字都在（它是停滞汇报**唯一**的内容来源）。
    #[test]
    fn counts_text_carries_all_six_numbers() {
        let s = counts_text(&VerifyCounts {
            ok: 1,
            bad: 2,
            missing: 3,
            size_mismatch: 4,
            unverifiable: 5,
            unreadable: 6,
        });
        for want in ["通过 1", "不符 2", "缺失 3", "大小不符 4", "读不了 6", "无法校验 5"] {
            assert!(s.contains(want), "{want:?} 不在 {s:?} 里");
        }
    }
}
