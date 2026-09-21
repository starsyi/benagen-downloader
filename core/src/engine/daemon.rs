//! 常驻 aria2 进程的生命周期管理。
//!
//! 这是**唯一**管进程生命周期的地方：启动（含端口重试）、加任务、读快照、关闭，
//! 外加内嵌 aria2c 的释放。对应 Go 的 `daemon.go` + `embedded.go` + `engine.go`
//! 的 `ExtractTo`/`DefaultCacheDir` 部分。
//!
//! # 四条承重事实（行为契约 §4，全部来自实测，**不得"优化"掉**）
//!
//! 1. **启动失败必须快速失败**（§4.1）：收尸由启动阶段建立的 `done` 承担，
//!    等 RPC 就绪时是**三路赛跑**（进程已退出 / Ping 就绪 / 超时）。
//!    实测：修好后撞端口换端口 **216.96 ms**；回退成"只轮询 Ping"是 **10.41 s**。
//! 2. **`close` 的 5 秒兜底维持 5 秒**（§4.2）：收到 `shutdown` 的 aria2 在父进程
//!    立刻退出时**不会自己走**（实测孤儿存活 > 48 秒）——兜底的 `kill` 是承重的。
//! 3. **`free_port` 的重探是无覆盖的防御代码**（§4.4）：保留，并如实标注没有测试。
//! 4. **就绪地板约 200 ms**（§4.5）来自轮询间隔，**不是缺陷**，别为此改轮询节奏。
//!
//! # 安全前提：一切输入只经 JSON-RPC 的结构化字段
//!
//! 契约 §7 记着 Go 侧的真实越界缺口：`validateBaseURL` 不拒空白字符，
//! `base_url="http://x/ dir=/tmp/evil"` 能过——而它越界的**唯一**触发条件是
//! `-i` 输入文件按**空白分词**（`-i` 是行式格式）。裁定 #21「Rust 侧不存在该缺口」
//! 成立的前提就是本模块：
//!
//! > **URL 与选项只经 `addUri`/`changeGlobalOption` 的结构化字段传递，
//! > 本模块不生成任何 `-i` 文件、也不把 URL/路径拼进任何按空白或换行分词的文本。**
//!
//! 逐处核对（详见本任务报告）：`add` → `RpcClient::add_uri`（`params[1]` 是 URI 数组、
//! `params[2]` 是选项对象）；`apply_global` → `change_global_option`（一个 JSON 对象）；
//! 启动只走 `Command::new(bin).args(launch_args)`——**argv 是结构化传参**，
//! 每个选项是一个独立的 `OsString`，不存在"再分词"这一步（不经 shell，也不经 aria2 的行解析器）。
//!
//! # 一处与 Go 的实现差异（有意，非"优化"）
//!
//! Go 的 `cmd.Wait()` 能阻塞在收尸 goroutine 里，是因为 `os.Process`（可 `Kill`）与
//! `exec.Cmd`（可 `Wait`）是**两个**对象。Rust 的 `Child` 把两者合成了一个：
//! 阻塞在 `Child::wait()` 就必须独占 `&mut Child`，超时兜底的 `Child::kill()` 也就拿不到它。
//! 所以收尸线程改用 `Child::try_wait()`（同样是 `waitpid(WNOHANG)`，**收尸语义不变**）
//! 按 `REAP_POLL` 的粒度观察退出。两条承重性质都保住了：`done` 的广播照旧（延迟 ≤ 25 ms），
//! 兜底的 `kill` 随时可用。**这不改变 §4.1–§4.5 的任何一条取值。**

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex, Once};
use std::time::{Duration, Instant};

use super::rpc::{GlobalStat, RpcClient};
use super::status::{raw_status_to_state, Task, TaskState};

/// RPC 就绪的等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// RPC 就绪的轮询间隔。
///
/// ⚠️ **它同时就是"引擎就绪"的下限**（契约 §4.5，实测）：首探在 ~604 µs 就拿到
/// connection refused，就绪仍落在第一个 tick 的 **206.7 ms**。
/// **地板来自这个间隔，不是实现缺陷**——不要为了那几个毫秒改成自适应节奏
/// （前 1 秒按 20 ms 探之类），那是为一个人眼无感的数值引入额外复杂度。
/// 壳侧的 200 ms 快照间隔（设计规格 §5.3）与它是同一个量级，不是巧合。
const READY_POLL: Duration = Duration::from_millis(200);

/// 收尸线程观察进程退出的间隔。
///
/// 25 ms 是"done 的广播延迟"的上界：撞端口时 aria2 实测 ~10 ms 就退出，
/// 换端口的总耗时因此仍在一百多毫秒量级（实测见报告），§4.1 的快速失败不受影响。
const REAP_POLL: Duration = Duration::from_millis(25);

/// `close()` 收到 `shutdown` 之后等进程自己走的时限，超时则 `kill` 兜底。
///
/// ⚠️ **维持 5 秒，不要调到 8–10 秒**（契约 §4.2）：兜底走 SIGKILL 是安全的
/// （`-c` 恒开，续传成立），调大会让"应用退出"在异常路径上白卡 5 秒。
/// 两害相权，宁可偶尔白杀一次也不让退出迟滞。
const CLOSE_FALLBACK: Duration = Duration::from_secs(5);

/// 端口尝试次数（`force_port` 只在第一次尝试时使用，之后一律动态探测）。
const PORT_ATTEMPTS: usize = 3;

/// 动态探测端口时的重探次数（见 [`free_port`] 的注释：这是**无覆盖**的防御代码）。
const FREE_PORT_RETRIES: usize = 5;

// ---------------------------------------------------------------------------
// 内嵌资源
// ---------------------------------------------------------------------------

/// 内嵌的自包含 aria2c——**按（平台，架构）选**。
///
/// 字节来自 `core/assets/`——那是 `build.rs` 从 `downloader/internal/engine/assets/`
/// 拷进来的副本（Go 侧用 `go:embed` 内嵌同一份文件）。拷一份而不是直接引用另一个
/// crate 的源码目录，是为了不让两个 crate 的目录结构耦死。
///
/// ⚠️ **为什么是每个平台各一条 `#[cfg]`、各带一份 thin 二进制，而不是一份 fat /
/// 通用二进制**（全局约束 F-6：故意偏离必须写理由）：
///
///   1. **分发形态就是"每个平台/架构一个独立包"**（阶段 F 的决定：Apple Silicon 版与
///      Intel 版分开出，**不做**通用二进制）。既然如此，内嵌 fat 只是让**每个包都白白
///      多带**另一架构的 aria2c（入库两份实测 arm64 2.4 MB / x86_64 2.1 MB），
///      换不来任何东西；
///   2. fat 化会**削弱**本文件 `embedded_binary_present` 里那条 thin 魔数断言——
///      它的全部意义是"内嵌的确实是一个能直接 `posix_spawn` 的 thin Mach-O"，
///      fat 的 `cafebabe` 得放行才过得去；
///   3. 选择必须**看得见、可 grep**：`grep -n 'assets/aria2c' core/src/` 一眼就能
///      看出哪个平台带哪份文件。改成由 `build.rs` 在构建期拼 `include_bytes!` 的路径
///      就做不到这点——**路径拼错时编译期不报错**，要到目标机器上才炸，
///      而"静默产出错的东西"正是本项目最忌讳的形态。
///
/// ⚠️ **不要在这里加"兜底分支"**（例如 `#[cfg(not(any(…)))]` 回退到 arm64）：那正是
/// 全局约束 F-2 禁止的静默降级——在别的靶架构上照样编得过，跑起来才
/// `Bad CPU type in executable`。不支持的架构**必须编不过**，所以下面用的是
/// `compile_error!` 而不是 `#[cfg]` 的兜底。
///
/// 名称单独成一个常量、而不是让调用方去解析 `include_bytes!` 的字面量：
/// 资产文件名是**契约**（壳侧与测试都要引它），而 `include_bytes!` 的路径
/// 拼错时编译期照样过。
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ARIA2C_ASSET_NAME: &str = "aria2c-macos-arm64";
/// 见上。
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const ARIA2C_ASSET_NAME: &str = "aria2c-macos-x86_64";
/// 见上。Linux x86_64 那份是 musl 静态链接的（客户机上不依赖 glibc 版本），
/// 由 `core/scripts/build_aria2_linux.sh` 在构建机上产出。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const ARIA2C_ASSET_NAME: &str = "aria2c-linux-x86_64";

/// 见上。
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const ARIA2C_BIN: &[u8] = include_bytes!("../../assets/aria2c-macos-arm64");

/// 见上。这几个 `#[cfg]` 与 `daemon.rs` 测试里的
/// `embedded_aria2c_arch_matches_kernel_arch` 是同一件事的两面：
/// **内嵌的这份必须与内核自己的架构一致**。
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const ARIA2C_BIN: &[u8] = include_bytes!("../../assets/aria2c-macos-x86_64");

/// 见上。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const ARIA2C_BIN: &[u8] = include_bytes!("../../assets/aria2c-linux-x86_64");

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
)))]
compile_error!(
    "这个平台没有内嵌 aria2c（现只有 macOS arm64/x86_64 与 Linux x86_64 三份，\
     见 core/assets/）。要加平台：先按 core/scripts/build_aria2_linux.sh 的路子产出资产，\
     再在本文件与 core/build.rs 的 ASSETS 表里各加一行。\
     绝不能回退到别的平台的内嵌二进制：那样编得出来，要到目标机器上才炸。"
);

/// 内嵌的 GPLv2 全文。
///
/// 内嵌分发 GPL 二进制**必须随附许可文本，且必须让用户能看到**——
/// 界面入口是 Go 侧的"开源许可"按钮。文本由构建脚本从同一份源码拷入，与二进制版本一致。
const ARIA2_LICENSE: &str = include_str!("../../assets/COPYING-GPLv2.txt");

// ⚠️ 这里曾经有一个 `pub const ARIA2_SOURCE_URL = "https://github.com/aria2/aria2";`
// ——**已删除**：它在整个内核里**没有任何消费者**（连测试都没有），而"没人读的常量"
// 正是契约 §1.8 说的那种误导。许可全文（`license_text`）留着，因为它是 GPL 分发义务的
// **载体**、阶段 B 要给它接上展示入口；"对应源码获取途径"的具体呈现方式（随 `.app` 的
// COPYING 一起给，还是由内核经协议给出）是那个入口的一部分，
// 届时应**连同入口一起**定，而不是先在这里留一个空常量。
// 阶段 B 待办里有一条显式的「GPL 展示入口」。

/// 内嵌的 aria2 许可全文（供界面展示）。
pub fn license_text() -> &'static str {
    ARIA2_LICENSE
}

// ---------------------------------------------------------------------------
// 启动参数
// ---------------------------------------------------------------------------

/// 启动参数。
#[derive(Clone)]
pub struct DaemonOptions {
    /// aria2 的工作目录（既是 `--dir=`，也是子进程的工作目录）。
    pub download_dir: PathBuf,
    /// 全局选项，走 `changeGlobalOption`（就绪之后下发一次）。
    pub global_opts: BTreeMap<String, String>,
    /// 逐任务选项的默认值，`add` 时合并（`addUri` 的结构化字段）。
    pub per_task: BTreeMap<String, String>,
    /// 留空则用内嵌的 aria2c。
    pub binary_path: Option<PathBuf>,
    /// 仅供测试：非零时先试这个端口，用于构造"端口被占"的场景。
    /// **不得出现在生产调用路径上。**
    pub force_port: Option<u16>,
}

/// 某一刻的任务状态。
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 引擎里的任务。**同一个 GID 已在生产端折叠**（见 [`fold_duplicate_gids`]），
    /// 因此它与 [`Snapshot::raw_status`] **一一对应**——两个出口对同一个 GID
    /// 说的是同一句话，消费者**不需要**再自己折叠一次。
    pub tasks: Vec<Task>,
    pub global: GlobalStat,
    /// gid → manifest 相对路径的快照
    pub path_map: BTreeMap<String, String>,
    /// gid → aria2 的**原始状态串**（`active`/`waiting`/`paused`/`complete`/…）。
    ///
    /// 领域模型 `TaskState` **有意**把 `paused` 与 `waiting` 映成同一个变体
    /// （契约 §8 表的 `#14`，Go 侧就是如此，保留以表达意图），所以那个区分
    /// **只能从这里取**。设计规格 §8.4 要求界面把暂停的任务「归入下载中并标注已暂停」
    /// ——**壳没有这个串就渲染不出那个标注**。
    ///
    /// ⚠️ **不要用 `Task::state` 反推**：那正是本字段要消灭的东西（反推出来的
    /// `paused` 会变成 `waiting`）。填充点是 [`Daemon::snapshot`] 里那趟
    /// `list()` 拿到的 `RawTask::status`，**零额外 RPC**。
    /// 钉住它的是 `daemon_snapshot_carries_raw_aria2_status`。
    ///
    /// 覆盖面是**全部任务**，不限于 `path_map` 里有映射的那些：两者回答的是
    /// 不同的问题（"这个 GID 在 aria2 里是什么状态" / "它是清单里的哪个文件"）。
    ///
    /// ⚠️ 与 [`Snapshot::tasks`] **一一对应**：`tasks` 已按**同一条规则**（同一 GID 取
    /// 最后一次出现）在生产端折过，所以两个出口不会对同一件事说两句话。
    pub raw_status: BTreeMap<String, String>,
}

/// 把 `list()` 三段拼接里的**重复 GID** 折叠掉，规则与 [`Snapshot::raw_status`] 逐字相同：
/// 保留**最后一次**出现，顺序沿用 `list()` 的原始次序。
///
/// ⚠️ **为什么折叠必须发生在生产者这里**：`list()` 是 `active ++ waiting ++ stopped`
/// 三段拼接，同一个任务在"刚传完"那一瞬间会被 `tellActive` 与 `tellStopped` 各报一次。
/// 这是**本模块的实现细节**，可消费者看到的却是两个形状不同的出口——
/// `raw_status` 是 `BTreeMap`（天然取最后一次），`tasks` 是 `Vec`（两次都在）。
/// 于是壳的传输列表里同一行出现两遍：一遍 `active`/0 字节、一遍 `complete`，
/// **互相矛盾**（实测：`enqueue` 之后头几百毫秒）。
///
/// 让"要折叠"成为每个消费者的义务是错的——**不变式的持有者是生产者**。
/// 那正是契约 §1.7 的教训：同一条规则多份实现，迟早漂移出真缺陷
/// （当时是"两个调用点记得、另外两个不记得"）。
fn fold_duplicate_gids(tasks: Vec<Task>) -> Vec<Task> {
    let mut last: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, t) in tasks.iter().enumerate() {
        last.insert(t.gid.as_str(), i);
    }
    // 保留**最后一个**下标之后按原始次序还原——`BTreeMap` 的键序不是到达序，必须排序。
    let mut idx: Vec<usize> = last.into_values().collect();
    idx.sort_unstable();
    idx.into_iter().map(|i| tasks[i].clone()).collect()
}

/// "进程已被回收"的一次性广播。
///
/// ⚠️ 它是"**收尸只做一次**"这条约定的载体（对应 Go 的 `daemon.done` channel）：
/// 收尸线程在启动阶段起一次，之后启动阶段的等待与 `close()` 都只是**读**它。
/// 谁再补一次 `wait()`/`try_wait()`，就会与它争抢同一个 `Child`。
struct Done {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Done {
    fn new() -> Self {
        Self {
            flag: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    fn is_done(&self) -> bool {
        *self.flag.lock().unwrap()
    }

    fn mark(&self) {
        *self.flag.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// 等到达或超时。`None` 表示无界等待（对应 Go 的 `<-d.done`）。
    fn wait(&self, timeout: Option<Duration>) -> bool {
        let mut g = self.flag.lock().unwrap();
        if *g {
            return true;
        }
        match timeout {
            // ⚠️ **必须循环**：`Condvar::wait_timeout` 允许**虚假唤醒**（spurious wakeup），
            // 一次虚假唤醒就让"等满 5 秒"降级成立刻返回 `false`——而 `close()` 拿到 `false`
            // 的第一件事就是 SIGKILL。那等于把 §4.2 特意留出的优雅退出宽限期**随机砍掉**，
            // 且症状是"偶尔白杀一次"（契约 §4.2 认了这个代价，但那是"两害相权"，
            // 不是"被虚假唤醒随机触发"）。所以按 `wait_timeout` 的契约循环到真超时为止。
            Some(t) => {
                let deadline = Instant::now() + t;
                loop {
                    let (g2, _) = self
                        .cv
                        .wait_timeout(g, deadline.saturating_duration_since(Instant::now()))
                        .unwrap();
                    g = g2;
                    if *g {
                        return true;
                    }
                    if Instant::now() >= deadline {
                        return false;
                    }
                }
            }
            None => {
                // 无界等待同样要循环：虚假唤醒在这里只会让它多转一圈（判据是 `flag` 本身）。
                while !*g {
                    g = self.cv.wait(g).unwrap();
                }
                true
            }
        }
    }
}

/// `by_gid` 与 `per_task` 共用一把锁。
///
/// ⚠️ **共用是承重的**（契约 §2.4 / `#40`）：`add`（下载线程）读 `per_task`、写 `by_gid`；
/// `set_per_task`（界面线程）写 `per_task`；`path_for`/`snapshot`（界面线程）读 `by_gid`。
/// 若 `per_task` 另用一把锁，"读它在自己的锁内"照样成立，但那把锁与 `by_gid` 的锁
/// 会形成两处独立的临界区——`add` 需要**同时**持有两者语义（"这个 GID 是按这份选项加的"）
/// 时就没有原子性可言了。Go 侧就是一把 `mu` 守两者。
#[derive(Default)]
struct State {
    by_gid: BTreeMap<String, String>,
    per_task: BTreeMap<String, String>,
}

struct Inner {
    /// 子进程句柄。收尸线程用它收尸，`close()` 的兜底路径用它 `kill`。
    /// HTTP 桩造出来的 Daemon（不需要真进程的那几条测试）这里是 `None`。
    cmd: Mutex<Option<Child>>,
    done: Done,
    state: Mutex<State>,
}

impl Inner {
    fn new(cmd: Option<Child>) -> Self {
        Self {
            cmd: Mutex::new(cmd),
            done: Done::new(),
            state: Mutex::new(State::default()),
        }
    }
}

/// 一个常驻的 aria2 进程。
pub struct Daemon {
    inner: Arc<Inner>,
    client: Arc<RpcClient>,
    url: String,
    /// RPC 的唯一凭据。收在结构体里是为了让"argv 里的 secret 与客户端用的必须是同一个"
    /// 这条断言有地方可查（对应 Go 测试里的 `d.client.secret`）。
    secret: String,
    /// 实际传给 aria2c 的完整 argv（含 argv[0]），对应 Go 的 `cmd.Args`。
    argv: Vec<OsString>,
    /// 关闭只做一次，**且所有调用者都等到进程真的被回收之后才返回**。
    ///
    /// 只用 bool 守卫是不够的：并发的第二个调用者会立刻拿到 `Ok(())`，
    /// 而那时 aria2 可能还没退出（实测优雅退出要约 4 秒）。父进程若紧接着退出，
    /// 孩子就成了孤儿——这正是 §4.2 那条实测（孤儿存活 > 48 秒）的形状。
    close_once: Once,
}

impl Daemon {
    /// 启动 aria2 并等到 RPC 可用。
    pub fn start(opts: DaemonOptions) -> Result<Daemon, String> {
        if opts.download_dir.as_os_str().is_empty() {
            return Err("DownloadDir 不能为空".to_string());
        }
        let bin = match &opts.binary_path {
            Some(p) => p.clone(),
            None => extract_to(&default_cache_dir())
                .map_err(|e| format!("释放内嵌 aria2c 失败: {e}"))?,
        };

        let secret = random_secret()?;

        // 端口：先试 force_port（仅测试），否则探一个空闲端口。
        // aria2 因端口失败时**重试一个新端口**——探测与释放之间有 TOCTOU 窗口。
        let mut tried: BTreeSet<u16> = BTreeSet::new();
        let mut first_port = opts.force_port;

        let mut last_err = String::new();
        for attempt in 0..PORT_ATTEMPTS {
            let mut port = first_port.unwrap_or(0);
            if attempt > 0 || port == 0 {
                // `free_port` 会在返回前关掉 listener，理论上可能把**已经试过的**端口
                // 再发一次；那样这一次尝试就白耗了（总共只有 3 次），所以重探几次。
                //
                // ⚠️ **这段重探是无覆盖的防御代码**（契约 §4.4）：要构造"`free_port`
                // 连发旧端口"需要给它一个接缝，而它是包级函数、无法注入。
                // 保留，但如实标注它**没有测试**——这与"什么都不断言的假测试"不同：
                // 那是一段假测试，这是一段真代码配一句诚实的"无覆盖"。
                let mut p = 0u16;
                for _ in 0..FREE_PORT_RETRIES {
                    p = free_port()?;
                    if !tried.contains(&p) {
                        break;
                    }
                }
                if tried.contains(&p) {
                    continue; // 5 次都撞上旧端口（实际不会发生），放过这一次尝试
                }
                port = p;
            }
            tried.insert(port);

            match start_on_port(&bin, &secret, port, opts.clone()) {
                Ok(d) => return Ok(d),
                Err(e) => {
                    last_err = e;
                    first_port = None; // 后续都走动态探测
                }
            }
        }
        Err(format!(
            "aria2 启动失败（已试 {} 个端口）: {last_err}",
            tried.len()
        ))
    }

    pub fn rpc_url(&self) -> &str {
        &self.url
    }

    pub fn ping(&self) -> Result<(), String> {
        self.client.ping()
    }

    /// 加一个下载任务，并**记下 GID → manifest 相对路径的映射**。
    ///
    /// 映射必须我们自己维护：aria2 只回 GID，它的 `files[].path` 是落盘路径
    /// （带着下载根），反推既不可靠也没必要。
    pub fn add(&self, url: &str, dir: &str, out: &str) -> Result<String, String> {
        // PerTask 必须在这里交付：任务诞生这一刻是逐任务选项唯一干净的落地点
        // （`-x`/`-s`/`-k`/`--max-tries`/`--retry-wait` 的事后补救为什么都不干净，
        // 见 `RpcClient::add_uri` 的注释）。
        //
        // ⚠️ 读它必须在**锁之下**：`set_per_task` 会改它（界面线程），这里是下载线程——
        // 无锁读就是一个真竞态，症状是"任务偶尔按旧参数下载"，极难复现。
        // 这里只把锁持到"拷出一份"为止，**不跨 RPC**（Go 侧同样是拷完就放锁）——
        // 持着锁做 10 秒的 HTTP 调用会让界面整段冻住（契约 §5.2 的同一类毛病）。
        let per_task = {
            let st = self.inner.state.lock().unwrap();
            st.per_task.clone()
        };

        let gid = self.client.add_uri(url, dir, out, &per_task)?;

        // URL 与选项只经 `addUri` 的**结构化字段**（`params[1]` 的 URI 数组、
        // `params[2]` 的选项对象）传递——见文件头的"安全前提"一节。
        let rel = join_rel(dir, out);
        {
            let mut st = self.inner.state.lock().unwrap();
            st.by_gid.insert(gid.clone(), rel);
        }
        Ok(gid)
    }

    /// 某个 GID 对应的 manifest 相对路径（不存在则 `None`，对应 Go 的空串）。
    pub fn path_for(&self, gid: &str) -> Option<String> {
        self.inner.state.lock().unwrap().by_gid.get(gid).cloned()
    }

    /// 替换逐任务选项的默认值（`-x`/`-s`/`-k`/`--max-tries`/`--retry-wait`）。
    /// **不需要重启引擎**——之后新加的任务会带上新的值（规格 §6）。
    ///
    /// 按值收走 `opts`，这**就是** Go 侧那次防御性拷贝：调用方此后无法再改它
    /// （Go 之所以要显式拷一份，是因为它按引用共享 map；Rust 的所有权把这件事
    /// 从"纪律"变成了"类型系统保证"）。
    pub fn set_per_task(&self, opts: BTreeMap<String, String>) {
        self.inner.state.lock().unwrap().per_task = opts;
    }

    /// 改全局选项（`-j` 并行数、限速），**不需要重启引擎**（契约 §2.5：即时生效）。
    ///
    /// 空选项是"没改"，不该白跑一趟 RPC（界面上取消勾选会走到这里）。
    pub fn apply_global(&self, opts: &BTreeMap<String, String>) -> Result<(), String> {
        if opts.is_empty() {
            return Ok(());
        }
        // 只经 `changeGlobalOption` 的结构化字段（一个 JSON 对象）传递。
        self.client.change_global_option(opts)
    }

    /// 读一次状态。
    ///
    /// 用列表方法而非按 GID 的 `tellStatus`——后者在 GID 失效时会 400（`Invalid GID`），
    /// 列表方法不需要我们维护 GID 生命周期。
    ///
    /// ⚠️ `list` 在前、`global` 在后，且**第一次出错就返回**（`?`）：契约 §4.3 的代价
    /// 论证建立在这上面——"一次快照失败即判定引擎断开"的实际代价是 **1 次** RPC 失败
    /// （最坏约 10 秒，connection refused 约 0 秒），而不是 4 次 × 10 秒。
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let raw = self.client.list()?;
        let global = self.client.global()?;
        // 折叠重复 GID：**必须在生产者这里做**（`raw_status` 是 map、天然取最后一次，
        // 不折叠的话两个出口对同一个 GID 说法相反）。见 `fold_duplicate_gids`。
        let tasks: Vec<Task> = fold_duplicate_gids(raw.iter().map(|r| r.to_task()).collect());

        // aria2 的原始状态串，**与 tasks 同源同一趟 `list()`**：领域模型的这次转换
        // 会把 `paused` 抹成 `waiting`（契约 §8 的 #14），而壳要拿它标注「已暂停」
        // （设计规格 §8.4），所以在这里**顺手**带出去——零额外 RPC、零额外解析。
        // 用 `collect` 而不是逐个 `insert`：gid 是 aria2 分配的、列表内不重复。
        let raw_status: BTreeMap<String, String> = raw
            .iter()
            .map(|r| (r.gid.clone(), r.status.clone()))
            .collect();

        // 必须是**副本**：界面拿到活 map 的话，它一边遍历我们一边 `add`，
        // 就是一场 `state` 这把锁守不到的 data race（Go 侧同一处注释）。
        let path_map = { self.inner.state.lock().unwrap().by_gid.clone() };

        Ok(Snapshot {
            tasks,
            global,
            path_map,
            raw_status,
        })
    }

    // -----------------------------------------------------------------------
    // 传输列表动作（任务 11 新增，**非移植**：Go 内核没有 pause/unpause/remove 的封装）
    // -----------------------------------------------------------------------

    /// 暂停一个任务（传输列表的"暂停"）。
    pub fn pause(&self, gid: &str) -> Result<(), String> {
        self.client.pause(gid)
    }

    /// 继续一个被暂停的任务（传输列表的"继续"）。
    pub fn unpause(&self, gid: &str) -> Result<(), String> {
        self.client.unpause(gid)
    }

    /// 移除一个任务，并**忘掉它的 GID 映射**。
    ///
    /// ⚠️ **必须按任务状态分派到两个不同的 aria2 方法**（本任务新增的语义，Go 无先例）：
    /// `aria2.remove` **只对活动/等待中的任务有效**——对已完成条目实测（aria2 1.37.0）
    /// 返回 400 `Active Download not found for GID#…`（code 1）；
    /// 已完成/已失败/已移除的条目要用 `removeDownloadResult`，反过来对活动任务调
    /// `removeDownloadResult` 同样报 400（`Could not remove download result of GID#…`）。
    /// 而传输列表里两种都有，用户点"移除"时不会区分——**分派是本方法的责任**。
    ///
    /// 判据是**本地状态**（该 GID 停在哪个 `TaskState` 上），**不是** aria2 的错误文案：
    /// 靠"先调 remove、失败了再试另一个"要匹配错误串，而字符串契约没有测试守护
    /// （任务 3 的审查记过同类脆弱性）。
    ///
    /// ⚠️ 判据走 **`list()`** 而**不是** `snapshot()`（修复轮 2 的 ②）：
    /// `snapshot()` = `list()` 的 3 条 + `global()` 的 1 条，而 `getGlobalStat` 对
    /// "该用哪个方法"**零贡献**。更要紧的是失败模式——它一挂，移除连试都不会试就返回
    /// 错误，**一个与移除毫不相干的 RPC 能把用户点的"移除"挡住**。
    /// 控制者的计划原文写的是 `snapshot()`，那句话**想窄了**：`snapshot()` 是给四态合成用的
    /// （它需要 `path_map` 与统计），而这里只需要回答"这个 GID 停没停"。
    /// **用一个大而全的读去回答一个小问题，就会继承它那些与自己无关的失败模式。**
    ///
    /// （它与 `clear_finished` 的过滤集合回答**同一个问题**、改动要**同向**：
    /// 两处的三个状态字面量必须一致。数据来源不同是刻意的——见上。两处是独立的 locus，
    /// 各由测试单独守着，**不抽公共谓词**：抽出来会让"改一处等于同时改两处语义"。）
    ///
    /// ⚠️ **两条路径走完都要 `forget`**：Go 的 `byGID` 只增不删（契约 §7 记为已接受的弱点）
    /// ——那是在**没有"移除"入口**的前提下。现在有了入口，不 forget 就意味着
    /// "移除过的任务永远还挂着一条路径映射"：界面拿 `path_for` 还能认出一个
    /// 已经不在 aria2 里的任务，表现为传输列表里移除不掉的一行。
    ///
    /// RPC 失败时**不** forget（错误原样带出去）：GID 在 aria2 那边还在，
    /// 映射也应当留着——界面下一次快照还能把它认出来。
    pub fn remove(&self, gid: &str) -> Result<(), String> {
        // 判据是**这一次列表读**里该 GID 的状态。取不到（不在列表里）时按"没停"处理：
        // 那样会走 `aria2.remove`，于是 aria2 报 `GID … is not found` 并被带出去
        // ——比"猜一个方法然后吞掉错误"诚实。
        let stopped = self.client.list()?.iter().any(|t| {
            t.gid == gid
                && matches!(
                    raw_status_to_state(&t.status),
                    TaskState::Complete | TaskState::Error | TaskState::Removed
                )
        });
        if stopped {
            self.client.remove_download_result(gid)?;
        } else {
            self.client.remove(gid)?;
        }
        // 两条路径走完都到这里：都在 aria2 那边成功过，映射就不再有任何依据。
        self.forget(gid);
        Ok(())
    }

    /// 清空已完成/失败的历史条目，并忘掉对应映射。
    ///
    /// ⚠️ **必须先取 GID 快照，再 purge**：`purgeDownloadResult` 一执行，`tellStopped`
    /// 就空了，此时再去取 GID 列表会拿到**空集**——映射就永久留在 `by_gid` 里。
    /// 顺序是：`snapshot()` 记下所有 `Complete`/`Error`/`Removed` 的 GID → `purge` → 逐个 `forget`。
    /// **症状是静默的**：列表看着干净，只是内部映射越积越多、`snapshot()` 的
    /// `path_map` 越带越大，而没有任何一处报错。
    ///
    /// ⚠️ **忘掉的是 `Complete`/`Error`/`Removed` 三种**，与 `purgeDownloadResult` 的
    /// 实际作用域**逐一对齐**（实测：purge 会一并清掉 `removed` 条目）。
    /// 曾经只写 `Complete`/`Error`，理由是"能进 `Removed` 的必然已经过 `Daemon::remove`
    /// 的 forget"——**那个理由不成立**：它依赖"没有别的路径能让 aria2 把任务置成 removed"，
    /// 而那是 **aria2 的行为**，不是本代码的性质（`d.client.remove` 就是一条这样的路径）。
    /// 纳入的代价严格为零（对已 forget 过的 GID 再 forget 是 no-op），方向单向安全：
    /// **不纳入是可能泄漏，纳入是最多多做一次无用功**。
    ///
    /// （本处的过滤集合与 `Daemon::remove` 的分派表回答**同一个问题**——"这个任务是否已停止"，
    /// **改动要同向**：两处的三个状态字面量必须一致。数据来源不同是刻意的——
    /// `remove` 用 `list()`（它只需要回答"停没停"），本处用 `snapshot()`（它本来就要读全量任务）。
    /// 两处是独立的 locus，各由测试单独守着，**不抽公共谓词**：抽出来会让"改一处等于同时改
    /// 两处语义"。反过来那边也有同一句，改这里之前先读它。）
    pub fn clear_finished(&self) -> Result<(), String> {
        // ⚠️ 顺序承重：**先取快照**。purge 之后 `tellStopped` 就空了，
        // 那时再取 GID 集合只会得到空集，一条映射都忘不掉。
        let finished: Vec<String> = self
            .snapshot()?
            .tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    TaskState::Complete | TaskState::Error | TaskState::Removed
                )
            })
            .map(|t| t.gid.clone())
            .collect();
        self.client.purge_download_result()?;
        for gid in &finished {
            self.forget(gid);
        }
        Ok(())
    }

    /// 忘掉一个 GID → manifest 相对路径的映射。
    ///
    /// 只删这一条，**不动** `per_task`：那是逐任务选项的默认值（`set_per_task` 的产物），
    /// 与"有哪些任务"无关。
    fn forget(&self, gid: &str) {
        self.inner.state.lock().unwrap().by_gid.remove(gid);
    }

    /// 关闭 aria2 并回收进程。**可重复调用，且并发调用也安全**：
    /// 每个调用者都等那唯一的一次关闭真正做完（含收尸）之后才返回。
    ///
    /// 收尸不在这里做：`try_wait` 已经在启动阶段起了唯一的一次，这里只是等它的结果
    /// （为什么不能在这里再收一次，见 [`Done`]）。
    ///
    /// 返回值恒为 `Ok(())`：Go 的 `closeErr` 从未被赋值过（`shutdown`/`Kill` 的错误
    /// 都被显式丢弃），签名保留 `Result` 只是为了与 Go 的 `Close() error` 对齐。
    pub fn close(&self) -> Result<(), String> {
        self.close_once.call_once(|| {
            let _ = self.client.shutdown();
            // `cmd == None` 有两种含义，两种都**没有东西可收**，直接返回：
            //   ① 收尸已经做完（收尸线程是先清空槽位、再置 `done`）；
            //   ② 这是 HTTP 桩造出来的 Daemon，从来没起过进程（只有测试会这么造）。
            // ⚠️ 少了这道判断，②会掉进下面的无界等待里**永久挂住**——那是测试脚手架
            // 才有的构造，别让后来者踩（Go 对同样的构造会 panic：`d.cmd` 是 nil）。
            if self.inner.cmd.lock().unwrap().is_none() {
                return;
            }
            if !self.inner.done.wait(Some(CLOSE_FALLBACK)) {
                // 兜底：**绝不能把进程留在那里**（契约 §4.2）。
                // 收到 shutdown 的 aria2 在父进程立刻退出时不会自己走（实测孤儿存活 > 48 秒），
                // 所以这一 Kill 是承重的，不是装饰。
                if let Some(child) = self.inner.cmd.lock().unwrap().as_mut() {
                    let _ = child.kill();
                }
                // Kill 之后还要**等收尸**：等不到就说明收尸没接上，那是 bug，
                // 不是可以忽略的情况（Go 在这里也是无界等待）。
                self.inner.done.wait(None);
            }
        });
        Ok(())
    }

    /// 等到 RPC 能应答（最多 [`READY_TIMEOUT`]）。
    ///
    /// ⚠️ **三路赛跑：进程已退出 / Ping 就绪 / 超时**（契约 §4.1，承重）。
    fn wait_ready(&self, port: u16, opts: &DaemonOptions) -> Result<(), String> {
        let deadline = Instant::now() + READY_TIMEOUT;

        // 先立刻探一次，别等满一个 tick（原形状是"先 Ping 再睡"）。
        //
        // ⚠️ 这一探必须与 done / 超时**赛跑**，不能直接调用：端口被占时它会打到
        // "占着端口却只接受不回话"的程序上，要等满 RPC 客户端 10 秒的超时——
        // 那样端口冲突的快速失败就没了。这个线程只可能在这条路上被吊住，
        // 且 channel 带缓冲，最多多活一个 http 超时，不会阻塞任何人。
        let (tx, rx) = mpsc::channel();
        let probe = Arc::clone(&self.client);
        std::thread::spawn(move || {
            let _ = tx.send(probe.ping());
        });

        let mut first: Option<mpsc::Receiver<Result<(), String>>> = Some(rx);
        let mut next_poll = Instant::now();
        loop {
            // ① 进程自己退了——端口被占时 aria2 绑不上会立刻退出（实测 ~10 ms）。
            //    这时必须立刻换端口重试，而不是干等满 10 秒：界面在这期间是冻住的。
            if self.inner.done.is_done() {
                return Err(format!("aria2 在端口 {port} 上已退出"));
            }

            // ② 首次探活的结论
            if let Some(rx) = &first {
                match rx.try_recv() {
                    Ok(Ok(())) => return self.ready(opts),
                    Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                        // 首次探活失败（连接被拒最快，通常是几百微秒）：转入周期轮询。
                        // ⚠️ 就绪因此有一个约 READY_POLL 的地板（契约 §4.5），**不是缺陷**。
                        first = None;
                        next_poll = Instant::now() + READY_POLL;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }

            // ③ 超时：兜底 Kill 并等收尸，绝不把起不来的进程留在机器上。
            if Instant::now() >= deadline {
                self.kill_and_reap();
                return Err(format!(
                    "端口 {port} 上 RPC 未在 {} 秒内就绪",
                    READY_TIMEOUT.as_secs()
                ));
            }

            // ④ 周期探活。
            // ⚠️ 走到这一 Ping 之前必须确认进程还在（第 ① 步刚做过）：否则这一 Ping 会
            // 打到占用端口的那个程序身上——它若只接受连接、从不回话，这里要白等满
            // http 客户端的超时，且每次 tick 都白等一次。
            if first.is_none() && Instant::now() >= next_poll {
                if self.client.ping().is_ok() {
                    return self.ready(opts);
                }
                next_poll = Instant::now() + READY_POLL;
            }

            std::thread::sleep(REAP_POLL);
        }
    }

    /// 探活成功：把全局选项落下去，再交还 `Daemon`。
    ///
    /// 失败被丢弃（Go 是 `_ =`）：启动时下发全局选项是"尽力而为"，
    /// 真正的错误会在随后每一次 RPC 上暴露。
    fn ready(&self, opts: &DaemonOptions) -> Result<(), String> {
        if !opts.global_opts.is_empty() {
            let _ = self.client.change_global_option(&opts.global_opts);
        }
        Ok(())
    }

    /// 兜底：Kill 之后**还要等收尸**，否则留下的是僵尸进程。
    fn kill_and_reap(&self) {
        if let Some(child) = self.inner.cmd.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        self.inner.done.wait(None);
    }
}

// ---------------------------------------------------------------------------
// 启动路径
// ---------------------------------------------------------------------------

/// 把目标目录变成绝对路径（对应 Go 的 `filepath.Abs`）。
///
/// ⚠️ 承重：`--dir=` 是相对**子进程工作目录**解析的，而我们又把子进程的工作目录设成了
/// 同一个目录。调用方给相对路径时，两者会**互相叠一层**（`<目标>/<目标>/…`），
/// 落盘基准就歪了——Go 的 `Run` 注释记着这件事（GUI 从 Finder 启动时 cwd 是 `/`）。
fn absolute_dir(dir: &Path) -> Result<PathBuf, String> {
    // `std::path::absolute` 与 Go 的 `filepath.Abs` 同义：**不碰文件系统**
    // （不做 `canonicalize`，因此不解析符号链接），相对路径接在当前工作目录之后。
    std::path::absolute(dir).map_err(|e| format!("目标目录路径无效: {e}"))
}

// ---------------------------------------------------------------------------
// 系统 CA 证书包（**只对 Linux 生效**）
// ---------------------------------------------------------------------------

/// 系统 CA 证书包的候选路径，按发行版布局差异排序。
///
/// **为什么要有这个**：Linux 那份 aria2c 的 OpenSSL 在**构建期**就把 CA 文件编死成了
/// 构建树里的 `/root/benagen-cli-build/build/ssl/cert.pem`（`strings core/assets/aria2c-linux-x86_64`
/// 可见，Task 1 已记账）。客户机上不存在这个路径 ⇒ 用 https 镜像下载时 TLS 握手直接
/// `unable to get local issuer certificate`；Task 1 的两次"成功下载"都是**显式传了
/// `--ca-certificate`** 才成的。所以在启动 aria2c 时把系统自带的 CA 包指给它，
/// 才是"https 镜像在客户机上真的能用"的那一步。
///
/// 三条候选对应三类常见布局（**哪个存在因发行版而异，所以要探而不是写死一条**）：
///   - `/etc/pki/tls/certs/ca-bundle.crt`——RHEL / Oracle / CentOS / Fedora
///   - `/etc/ssl/certs/ca-certificates.crt`——Debian / Ubuntu / Alpine
///   - `/etc/ssl/cert.pem`——Alpine 与 macOS 的另一种布局
///
/// ⚠️ **顺序是规格的一部分**：RHEL 系 → Debian/Alpine 系 → 另一布局。
/// 调换顺序或清空这张表，R9 就**在没人察觉的情况下失效**（"探不到 CA 包"正是它要根除的
/// 那个失效形态），所以 `system_ca_bundles_are_the_three_documented_layouts_in_order`
/// 把它逐字钉住。
///
/// ⚠️ **这张表不带 `#[cfg]`**：数据就是数据，与主机平台无关。带 cfg 的话
/// 测试就只能跟着 `#[cfg(target_os = "linux")]` 走，而那条链在 macOS 上就没人验了。
/// **平台门在调用点**（`launch_args`）：只有 Linux 才会拿它去探。
const SYSTEM_CA_BUNDLES: &[&str] = &[
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/ssl/cert.pem",
];

/// 从候选里挑**第一个**"谓词说存在"的路径；一个都没有时返回 `None`。
///
/// 抽成纯函数（存在性谓词由调用方注入）是为了**它能被单测**：
/// "取第一个存在的"这个判定不该长在 I/O 边上——那样只能靠伪造文件系统去测、或者干脆不测。
fn first_existing_path(candidates: &[&str], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates
        .iter()
        .map(Path::new)
        .find(|p| exists(p))
        .map(Path::to_path_buf)
}

/// 从 [`SYSTEM_CA_BUNDLES`] 里挑中那条做成 `--ca-certificate=<路径>`；一条都不存在时
/// 返回 `None`。
///
/// `None` 时**不传**（而不是传一条空路径）：不传只是回到"用产物里编死的那个路径"——
/// 那个路径在客户机上不存在，于是 https 镜像会失败，但 http 镜像与本地文件下载照旧可用。
/// 两害相权：宁可在没有 CA 包的机器上只坏 https，也不要让引擎整个起不来。
///
/// ⚠️ **本函数不判平台**：它是"表 + 谓词 → 参数"的纯函数，好让整条链（表 → 取第一条 →
/// 拼参数）在**任何平台上都能被单测**。**平台门在调用点**——`launch_args` 只在
/// Linux 上调它。别把 `cfg!(target_os = "linux")` 挪进来：那样测试就只能整条 `#[cfg]` 掉，
/// 这条链在 macOS 上就没人验了（审查修复轮 Minor ① 点的正是这个）。
fn ca_bundle_arg(exists: impl Fn(&Path) -> bool) -> Option<OsString> {
    let path = first_existing_path(SYSTEM_CA_BUNDLES, exists)?;
    let mut arg = OsString::from("--ca-certificate=");
    arg.push(path);
    Some(arg)
}

/// 常驻模式的启动 argv（不含 argv[0]）。
///
/// 逐项都有出处，**不是随手排的**（规格 §3 的三条硬约束在这里落地）：
///   - `--rpc-listen-all=false`：只监听回环。aria2 默认也只听回环，但没有这条
///     就没有任何一处**声明**它——重构掉这一条普通功能测试全绿。
///   - `--rpc-secret`：RPC 的唯一凭据，每次启动新生成。
///   - `-c`：断点续传恒开（规格 §3 约束 5）。
///   - `--auto-file-renaming=false` + `--allow-overwrite=true`：直接决定
///     "落盘路径 = manifest.path 原文"——没有它们，重下会写出 `name.1`。
fn launch_args(dir: &Path, secret: &str, port: u16) -> Vec<OsString> {
    // `--dir=` 与 `--rpc-secret=` 都是**单个 argv 元素**：结构化传参，
    // 里面的空格/引号/换行不会被任何人再分词（不经 shell，也不经 aria2 的行解析器）。
    // 路径本身按 `OsString` 拼接而不是 `to_string_lossy`——非 UTF-8 的目录名也走得通。
    let mut dir_arg = OsString::from("--dir=");
    dir_arg.push(dir.as_os_str());

    let mut args = vec![
        OsString::from("--enable-rpc"),
        // 只监听回环。aria2 的默认值恰好也是 false，所以这条掉了功能测试不会红——
        // 正因如此才要单独钉住（`daemon_launch_args_pin_loopback_secret_and_resume`）。
        OsString::from("--rpc-listen-all=false"),
        OsString::from(format!("--rpc-secret={secret}")),
        OsString::from(format!("--rpc-listen-port={port}")),
        dir_arg,
        // 断点续传恒开（规格 §3 约束 5）。
        OsString::from("-c"),
        OsString::from("--file-allocation=none"),
        // 下面两条直接决定"落盘路径 = manifest.path 原文"：`--auto-file-renaming`
        // 默认是 true，重下会写出 `name.1`，落盘基准就与清单对不上了。
        OsString::from("--auto-file-renaming=false"),
        OsString::from("--allow-overwrite=true"),
        OsString::from("--summary-interval=1"),
    ];
    // 系统 CA 证书包（见 `SYSTEM_CA_BUNDLES` 的长注释）：**平台门就在这一句上**。
    //   - Linux：探三条候选，第一个存在的用 `--ca-certificate=` 传进去；都没有就不传。
    //   - macOS：**不探也不传**——那两份产物是在本机编的、OpenSSL 指的系统 CA 就在机器上，
    //     两个架构都已实测通过，argv 必须与改动前**逐字相同**。
    //
    // 这条尾巴是 `args_defaults` 里"平台特有的尾巴"那一段判的（macOS 判它必须为空）。
    if cfg!(target_os = "linux") {
        args.extend(ca_bundle_arg(|p| p.is_file()));
    }
    args
}

/// 在指定端口上起一次 aria2 并等它就绪。
///
/// 与 [`Daemon::start`] 的分工：外层负责"换端口重试"，这里只负责**一次**尝试。
fn start_on_port(
    bin: &Path,
    secret: &str,
    port: u16,
    opts: DaemonOptions,
) -> Result<Daemon, String> {
    // `--dir=` 与子进程的工作目录必须指向**同一个绝对路径**（见 `absolute_dir`）。
    let dir = absolute_dir(&opts.download_dir)?;
    let args = launch_args(&dir, secret, port);

    let mut argv: Vec<OsString> = Vec::with_capacity(args.len() + 1);
    argv.push(bin.as_os_str().to_os_string());
    argv.extend(args.iter().cloned());

    let child = Command::new(bin)
        .args(&args)
        .current_dir(&dir)
        // ⚠️ 全局约束 1：stdout 是内核的**协议专用通道**，子进程的输出绝不能漏上去。
        // 这里取 `Stdio::null()`（= Go 的 `cmd.Stdout = nil`，即 /dev/null）：
        // 既堵死了"漏进协议通道"这条路，也不会让 aria2 的 `--summary-interval=1`
        // 摘要堵在一条没人读的管道里（管道写满会让 aria2 自己卡住）。
        // **不要**改成 `inherit`。
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 aria2c 失败: {e}"))?;

    let inner = Arc::new(Inner::new(Some(child)));
    // 逐任务选项的**初始值**必须在任何任务诞生之前就位：`add` 从这里取，
    // 而 `set_per_task` 之后会整体替换它（`daemon_add_delivers_per_task_options`
    // 钉住这一步——少了它，`add` 读到的永远是空 map，任务按 aria2 的默认值下载）。
    inner.state.lock().unwrap().per_task = opts.per_task.clone();
    // 全程唯一的一次收尸（理由见 `Done`）：它既回收进程，又通过 `done` 把"进程没了"
    // 广播给启动阶段的等待与 `close()`。
    spawn_reaper(Arc::clone(&inner));

    let url = format!("http://127.0.0.1:{port}/jsonrpc");
    let client = Arc::new(RpcClient::new(&url, secret));
    let d = Daemon {
        inner,
        client,
        url,
        secret: secret.to_string(),
        argv,
        close_once: Once::new(),
    };

    d.wait_ready(port, &opts)?;
    Ok(d)
}

/// 收尸线程：观察子进程退出，收尸之后把 `done` 广播出去。
///
/// ⚠️ **收尸只此一处**（见 [`Done`]）。`try_wait` 就是 `waitpid(WNOHANG)`：
/// 它返回 `Ok(Some(_))` 时进程**已经被回收**、PID 已被系统收回——这正是
/// `daemon_start_timeout_kills_and_reaps_the_process` 用 `kill -0` 断言的性质。
/// 换回阻塞 `wait()` 就需要独占 `&mut Child`，`close()` 的兜底 `kill` 就再也拿不到它了
/// （文件头"一处与 Go 的实现差异"记着这件事）。
fn spawn_reaper(inner: Arc<Inner>) {
    std::thread::spawn(move || {
        loop {
            {
                let mut slot = inner.cmd.lock().unwrap();
                match slot.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(_)) => {
                            // 已收尸：把句柄丢掉（`Child` 的 Drop 既不再 wait 也不 kill）。
                            *slot = None;
                            break;
                        }
                        Ok(None) => {} // 还在跑
                        Err(_) => {
                            // 拿不到退出状态（理论上只有"已经被别人收过"这一种）。
                            // 不能死循环，也不能让 `close()` 永远等下去——当作已收尸。
                            *slot = None;
                            break;
                        }
                    },
                    None => break,
                }
            }
            std::thread::sleep(REAP_POLL);
        }
        inner.done.mark();
    });
}

/// 探一个空闲端口。
///
/// ⚠️ 探测与释放之间有 TOCTOU 窗口（别的进程可能抢走）——调用方必须容忍后续 aria2
/// 启动失败并换端口重试，**不得**把这里的结果当作保证。
fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("探空闲端口失败: {e}"))?;
    let port = l
        .local_addr()
        .map_err(|e| format!("探空闲端口失败: {e}"))?
        .port();
    drop(l); // 立刻放开，交给 aria2 去绑
    Ok(port)
}

/// 生成 16 字节的随机 secret（32 个十六进制字符）。
///
/// secret 每次启动都必须新生成：它是 RPC 唯一的凭据，固定下来等于把端口交给别人用。
///
/// ⚠️ 随机源是 `/dev/urandom`：本 crate 不许新增依赖（没有 `rand`/`getrandom`），
/// 而 **std 里没有 CSPRNG**。`/dev/urandom` 是 macOS/Linux 内核的密码学随机源，
/// 正是 Go 的 `crypto/rand` 在同一个平台底下用的东西。
/// 拿时间戳/进程号凑一个"看着随机"的串是**不行**的——那会让 RPC 凭据可预测。
fn random_secret() -> Result<String, String> {
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("读取系统随机源失败: {e}"))?;
    Ok(hex(&buf))
}

// ---------------------------------------------------------------------------
// 内嵌 aria2c 的释放
// ---------------------------------------------------------------------------

/// 释放位置——Caches 下，不污染数据目录。
pub fn default_cache_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("Library/Caches/BenagenDownloader"),
        None => std::env::temp_dir().join("BenagenDownloader"),
    }
}

/// 释放文件名 / `--version` 里用的摘要长度：sha256 十六进制串的**前 12 位**。
///
/// ⚠️ **这个 `12` 只有这一份**（2026-09-21，任务 6）：两个消费者——`extract_to` 拼的
/// 释放文件名 `aria2c-{前 12 位}`，与 `benagen-dl --version` 打给客户看的那一串——
/// 说的必须是同一个值。客户正是拿 `--version` 那串去对缓存目录里的文件名；
/// 两处各写一个 `[..12]`，改了其中一处就会**静默地**对不上（不报错，只是对不上）。
///
/// ⚠️ **它公开是"为了给 bin 用"，不是"给外部消费者用的"**：`benagen-dl` 是**另一个
/// crate**（`src/bin/benagen-dl.rs` 与 lib 是两个 crate），它要够得着就只能 `pub`。
/// （与 `embed_hash_hex` 同一条理由，一并记在这里。）
pub const EMBED_HASH_PREFIX_LEN: usize = 12;

/// 把内嵌的 aria2c 释放到 `cache_dir`，返回可执行文件路径。
///
/// 若已存在且哈希相符则复用（避免每次运行都写盘，也减少被杀软盯上的机会）；
/// 若哈希不符（被篡改或不完整）则覆盖重写，**绝不**使用来路不明的文件。
///
/// 落位方式是"写 `.tmp` 再 `rename`"：`rename` 在同一文件系统内是原子的，
/// 客户机上同时起来两个客户端也不会读到写了一半的二进制。
pub fn extract_to(cache_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;
    // 文件名由内容哈希派生，天生恒定——这也是"复用"必须拿 inode/mtime 当证据的原因。
    let name = format!("aria2c-{}", &embed_hash_hex()[..EMBED_HASH_PREFIX_LEN]);
    let path = cache_dir.join(&name);

    if let Ok(existing) = std::fs::read(&path) {
        if sha256(&existing) == sha256(ARIA2C_BIN) {
            return Ok(path);
        }
    }

    let tmp = private_tmp(&path);
    write_executable(&tmp, ARIA2C_BIN)?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("落位 aria2c 失败: {e}"))?;
    Ok(path)
}

/// 每次写入用自己的**私有**临时名（`<name>.<pid>.<序号>.tmp`）。
///
/// ⚠️ 这是相对 Go 的一处**有意偏离**（Go 是共用一个 `<name>.tmp`），理由是实测到的缺陷：
/// 并发调用 `extract_to` 时（同一进程的多个线程，或客户机上同时起来的两个客户端），
/// 先完成的那次 `rename` 会把后来者**正在写**的那个 `.tmp` 搬走，后来者的 `rename`
/// 立刻报 `ENOENT`——`Daemon::start` 直接失败（实测报错：
/// `释放内嵌 aria2c 失败: 落位 aria2c 失败: No such file or directory`）。
/// 私有临时名 + 原子 `rename` 之后，并发写入退化成"最后完成者胜"，而两次写入的字节
/// **完全相同**，所以落位结果与单次写入没有区别。可观察语义不变（见 `extract_to`）。
fn private_tmp(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// 写入一个**可执行**文件（对应 Go 的 `os.WriteFile(path, data, 0o755)`：
/// 权限是在**创建时**给定的，不是写完再 chmod——中间那一瞬间不能留下一个
/// 没有执行位、却被另一个进程看见的文件）。
fn write_executable(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .map_err(|e| format!("释放 aria2c 失败: {e}"))?;
    f.write_all(data)
        .map_err(|e| format!("释放 aria2c 失败: {e}"))?;
    Ok(())
}

/// SHA-256（FIPS 180-4）。
///
/// ⚠️ **为什么是手写的**：`extract_to` 的"已存在且哈希相符则复用"与"绝不使用来路不明的
/// 文件"两条语义都要求一个**密码学**哈希，而本 crate 的依赖表里没有 `sha2`，
/// 本任务又不许新增依赖。crate 里已有的 CRC 不是密码学哈希——碰撞构造成本极低，
/// 拿它当判据等于把"被篡改的文件"判成"相符"。
/// 实现由 `sha256_known_answer_vectors` 用**独立来源**的标准向量钉住
/// （该测试不计入移植条数，理由见报告：手写哈希没有别的东西能证明它是对的）。
fn sha256(data: &[u8]) -> [u8; 32] {
    /// SHA-256 的轮常数：前 64 位圆周率小数部分的前 32 位（FIPS 180-4 §4.2.2）。
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    // 初始状态：前 8 个素数平方根小数部分的前 32 位（FIPS 180-4 §5.3.3）。
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // 填充：0x80 → 若干 0 → 64 位大端比特长度。填充后的长度必是 64 的整数倍。
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// 小写十六进制（Go 的 `hex.EncodeToString`）。
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

/// 内嵌二进制的 sha256 十六进制串（Go 在包初始化时算好同名的那一份）。
///
/// 算一次就缓存：Go 的 `var embedHashHex = func() string {...}()` 是**包初始化时算一次**，
/// 这里用 `OnceLock` 对齐同一语义（不这么做的话每次 `extract_to` 都要把 2.4 MB 重哈希一遍；
/// Go 侧每次调用只哈希两次，多出来那次纯属浪费）。
///
/// ⚠️ **`pub`（2026-09-21，任务 6 加）**：`benagen-dl --version` 要打它的**前 12 位**
/// （spec §1 第 3 条 / §2 的表）。那不是"另算一个值"，而是**同一个来源的另一处消费**——
/// `extract_to` 拼的释放文件名就是 `aria2c-{前 12 位}`，客户报出来的那串必须能与
/// 他机器上缓存目录里的文件名直接对上（排障时第一个要问的东西）。
/// 所以不另开一个 `pub fn`，只把这个已有的函数开出去：多一个入口就是多一份会漂的真相。
///
/// ⚠️ **它公开是"为了给 bin 用"，不是"给外部消费者用的"**：`benagen-dl` 是**另一个
/// crate**（`src/bin/benagen-dl.rs` 与 lib 是两个 crate），它要够得着就只能 `pub`。
/// 将来有人看这份公开面（T2 已经把 19 项开成了 `pub`）时，要能一眼分出这两类——
/// 所以这一句写在函数上。**下一轮再想扩大公开面时，先回答"是哪个 bin/crate 要"**。
pub fn embed_hash_hex() -> String {
    use std::sync::OnceLock;
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hex(&sha256(ARIA2C_BIN))).clone()
}

/// 由 `dir` 与 `out` 还原 `manifest.path` **原文**。
///
/// ⚠️ 这里**不做** Go 的 `filepath.Join(dir, out)`：那个调用会顺带 `Clean`，
/// 把 `a/./b.txt` 折成 `a/b.txt`——与契约 §3.1「`PathFor` 必须与 `manifest.path`
/// **逐字一致**」冲突（`dir` 来自 `manifest.path` 的父目录，`out` 是文件名，
/// 两者拼回去就是原文；顶层文件的 `dir` 是 `.`，此时原文就是 `out`）。
/// 后果的形状是**静默错位、不报错**：路径与清单对不上，整棵树的四态显示错的状态。
fn join_rel(dir: &str, out: &str) -> String {
    // 顶层文件的 `dir` 是 `.`：此时原文就是 `out` 本身（拼成 `./x` 反而与清单对不上）。
    if dir == "." || dir.is_empty() {
        out.to_string()
    } else {
        format!("{dir}/{out}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{RawResponse, StubHttp, TempDir};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    // -----------------------------------------------------------------------
    // 脚手架（对应 Go 的 `writeStubBinary` / `readPID` / `startDummyListener` 等）
    // -----------------------------------------------------------------------

    /// 对应 Go 的 `DaemonOptions{DownloadDir: dir}`：其余字段取零值。
    fn options(dir: &Path) -> DaemonOptions {
        DaemonOptions {
            download_dir: dir.to_path_buf(),
            global_opts: BTreeMap::new(),
            per_task: BTreeMap::new(),
            binary_path: None,
            force_port: None,
        }
    }

    /// 测试里的收尸保险，对应 Go 的 `defer d.Close()`。
    ///
    /// Rust 没有 `defer`：断言失败 panic 时，函数末尾那句 `close()` 不会被执行，
    /// aria2 就留成孤儿了——**绝不许留孤儿进程**。这个 guard 让 panic 路径也收尸。
    struct Guard<'a>(&'a Daemon);

    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = self.0.close();
        }
    }

    fn guard(d: &Daemon) -> Guard<'_> {
        Guard(d)
    }

    /// 工作目录守卫：Drop 时恢复调用方原来的工作目录（对应 Go 的 `t.Cleanup`）。
    struct CwdGuard(PathBuf);

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    /// 写一个忽略所有参数的可执行桩，返回它的路径（对应 Go 的 `writeStubBinary`）。
    /// 桩先把 `$$`（自己的 PID）写进 `pid_file`，再执行 `body`。
    fn stub_binary(dir: &Path, pid_file: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-aria2c");
        let script = format!("#!/bin/sh\necho $$ > {}\n{}", pid_file.display(), body);
        write_exec(&path, &script);
        path
    }

    fn write_exec(path: &Path, script: &str) {
        write_executable(path, script.as_bytes()).expect("写桩失败");
    }

    /// 读桩写出的 PID 文件（对应 Go 的 `readPID`）。
    fn read_pid(path: &Path) -> i32 {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("桩没写出 PID 文件（{e}）——它可能根本没被启动"));
        let pid: i32 = raw.trim().parse().expect("PID 文件内容不合法");
        assert!(pid > 0, "PID 文件内容不合法: {raw:?}");
        pid
    }

    /// `kill(pid, 0)` 对**活着**和**没被收尸**的进程都成功，只有真的 wait 过、
    /// PID 被系统收回之后才失败。所以这一条同时钉住了 Kill 与收尸。
    ///
    /// Rust 的 std 没有 kill-by-pid，用 `/bin/kill -0`（结构化 argv，不经 shell）——
    /// 与 Go 的 `syscall.Kill(pid, 0)` 等价。
    fn process_gone(pid: i32) -> bool {
        // ⚠️ `kill` 起不来时必须**响亮地失败**：`unwrap_or(false)` 会让 `assert!(process_gone(pid))`
        // 在"前提根本没成立"时照样通过——那正是本项目反复栽的形状（**测试绕过了会出错的那条路**）。
        // 这里的 `kill` 是**观察手段**（问系统要进程状态），它失败意味着断言失去意义，不是"进程没了"。
        let status = StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            // `kill -0` 对不存在的进程会往 stderr 写一句 "No such process"——那是预期内的噪音，丢掉。
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("无法执行 `/bin/kill -0` —— 这条断言的前提（能问系统要进程状态）不成立");
        !status.success()
    }

    /// 占住一个端口，用于构造"端口冲突"场景。
    ///
    /// ⚠️ **必须同时占住 IPv4 与 IPv6 回环**，否则"冲突"根本不成立：aria2 的 RPC 在两个
    /// 协议栈上各绑一次，只占 IPv4 时它会 `ERROR IPv4 RPC: failed to bind` 之后照样
    /// `NOTICE IPv6 RPC: listening` **活下去**（实测）——进程不退，那走的是
    /// "Ping 不到、耗满 10 秒超时再换端口"那条路，而不是这里要测的"端口被占、立刻换"。
    /// Go 版的桩只占了 IPv4，于是那两条测试是**准永真**的；Rust 侧这条缺口必须堵上。
    ///
    /// 两个 listener **从不 accept**：内核的 backlog 会完成 TCP 握手，于是客户端
    /// connect 得上、却永远等不到响应——这正是 §4.1 要防的那种"占着端口不回话"的程序。
    struct DummyListener {
        port: u16,
        _l4: TcpListener,
        _l6: TcpListener,
    }

    impl DummyListener {
        /// 造不出来（例如没有 IPv6 回环）返回 `None`，调用方**跳过**——
        /// 对应 Go 的 `t.Skip`。
        fn start() -> Option<Self> {
            let l4 = TcpListener::bind("127.0.0.1:0").ok()?;
            let port = l4.local_addr().ok()?.port();
            let l6 = TcpListener::bind(("::1", port)).ok()?;
            Some(Self {
                port,
                _l4: l4,
                _l6: l6,
            })
        }
    }

    /// 造一个**没有子进程**的 Daemon（对应 Go 的 `&Daemon{client: ..., byGID: ...}`）。
    /// 只给"不需要真 aria2"的那几条用（RPC 走 HTTP 桩）。
    fn stub_daemon(base_url: &str, secret: &str, by_gid: BTreeMap<String, String>) -> Daemon {
        let url = format!("{base_url}/jsonrpc");
        // 一次构造出来，不要 `State::default()` 之后再逐字段赋值
        // （clippy 的 `field_reassign_with_default` 在 `-D warnings` 下不接受那种写法）。
        let state = State {
            by_gid,
            ..Default::default()
        };
        Daemon {
            inner: Arc::new(Inner {
                cmd: Mutex::new(None),
                done: Done::new(),
                state: Mutex::new(state),
            }),
            client: Arc::new(RpcClient::new(&url, secret)),
            url,
            secret: secret.to_string(),
            argv: Vec::new(),
            close_once: Once::new(),
        }
    }

    /// 直接问 aria2 本人：某个任务身上的选项（`aria2.getOption`）。
    ///
    /// ⚠️ 为什么在测试里手写这一趟 RPC，而不是复用 `RpcClient`：`RpcClient::call` 是
    /// `rpc` 模块的**私有**方法，`daemon` 是它的**兄弟**模块、看不到它，
    /// 而本任务只获准改 `engine/mod.rs` 一行，不许动已过审的 `rpc.rs`。
    /// 手写这一趟反而**更独立**：它证明的是"aria2 真的收到了这些选项"，
    /// 而不是"我们的客户端以为自己发了"。线上形状照 `RpcClient::call`：
    /// POST JSON-RPC，`params[0]` 是 `token:<secret>`。
    fn get_option(d: &Daemon, gid: &str) -> Result<BTreeMap<String, String>, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "aria2.getOption",
            "params": [format!("token:{}", d.secret), gid],
        })
        .to_string();
        let text = ureq::post(&d.url)
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(|e| format!("getOption 请求失败: {e}"))?
            .into_string()
            .map_err(|e| format!("getOption 响应读取失败: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("getOption 响应不是 JSON: {e}"))?;
        let result = v
            .get("result")
            .ok_or_else(|| format!("getOption 没有 result 字段: {text}"))?;
        serde_json::from_value(result.clone()).map_err(|e| format!("解析 getOption 失败: {e}"))
    }

    /// 从原始请求体里取 `(method, params)`。
    fn method_and_params(raw: &str) -> (String, Vec<serde_json::Value>) {
        let v: serde_json::Value = serde_json::from_str(raw).expect("请求体必须是 JSON");
        (
            v["method"].as_str().unwrap_or("").to_string(),
            v["params"].as_array().cloned().unwrap_or_default(),
        )
    }

    /// 与 Go 的 `os.SameFile` 同义：比 (dev, ino)。
    fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }

    // -----------------------------------------------------------------------
    // 移植自 daemon_test.go
    // -----------------------------------------------------------------------

    /// 对应 Go `TestDaemonStartsAndStops`。
    #[test]
    fn daemon_starts_and_stops() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        assert!(!d.rpc_url().is_empty(), "应给出 RPC 地址");
        // 探活：Ping 走 getGlobalStat，会校验 secret
        d.ping().expect("RPC 探活失败");
    }

    /// 对应 Go `TestDaemonCloseIsIdempotent`。
    #[test]
    fn daemon_close_is_idempotent() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);
        d.close().expect("首次关闭失败");
        d.close().expect("重复关闭不应报错");
    }

    /// 对应 Go `TestDaemonAddTracksGIDToPath`。
    #[test]
    fn daemon_add_tracks_gid_to_path() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        // 用一个必然连不上的地址，只为验证"加了任务就记下映射"
        let gid = d
            .add("http://127.0.0.1:1/none", "PFX/sub", "a.txt")
            .expect("addUri 不应失败");
        assert_eq!(
            d.path_for(&gid).as_deref(),
            Some("PFX/sub/a.txt"),
            "GID→路径映射错误"
        );

        // ⚠️ **不得规范化**（契约 §3.1，承重）：`PathFor` 是 §3.1 的**第二个实现 locus**
        // （第一个是任务 5 的 `new_planned_file`），两边各错各的、**互不代偿**——
        // 上面那条用的是规范路径（`PFX/sub` + `a.txt`），把实现换成"先 Clean 再拼"
        // 也照样绿，所以只有「安全但非规范」的 `dir` 能把这条钉住。
        //
        // 后果的形状是**静默错位、不报错**：`a/./b.txt` 被折成 `a/b.txt` 之后，
        // 界面拿 `path_for` 去认"哪个文件"就与清单对不上，整棵树的四态显示错的状态，
        // 而没有任何一处报错。（Go 侧同样没有守这条：`filepath.Join` 自己就会 Clean，
        // 在非规范路径上**本来就违反** §3.1。）
        let gid2 = d
            .add("http://127.0.0.1:1/none", "a/.", "b.txt")
            .expect("addUri 不应失败");
        assert_eq!(
            d.path_for(&gid2).as_deref(),
            Some("a/./b.txt"),
            "PathFor 必须与 manifest.path 逐字一致（契约 §3.1）：不得规范化"
        );

        // ⚠️ 上面那条与这一条**合起来**才把 `join_rel` 封住，缺一条都会让人以为整个函数守住了：
        //   - 上面：**非规范路径不得被规范化**（`a/.` + `b.txt` → `a/./b.txt`）；
        //   - 下面：**`.` 不得被拼进去**（`dir="."` 是**最常走**的那条路——根目录下的文件）。
        // 把 `join_rel` 整个换成朴素的 `format!("{dir}/{out}")`，只有这一条会红。
        let gid3 = d
            .add("http://127.0.0.1:1/none", ".", "top.txt")
            .expect("addUri 不应失败");
        assert_eq!(
            d.path_for(&gid3).as_deref(),
            Some("top.txt"),
            "顶层文件（dir=\".\"）不得拼成 \"./top.txt\""
        );
    }

    /// 对应 Go `TestDaemonSnapshotSeesAddedTask`。
    #[test]
    fn daemon_snapshot_sees_added_task() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        d.add("http://127.0.0.1:1/none", ".", "x.txt")
            .expect("加任务失败");
        // 给 aria2 一点时间把任务报出来
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let snap = d.snapshot().expect("读快照失败");
            if !snap.tasks.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("5 秒内没在快照里看到刚加的任务");
    }

    /// 对应 Go `TestDaemonRetriesOnPortConflict`。
    #[test]
    fn daemon_retries_on_port_conflict() {
        // 占住一个端口后启动，应当自行换端口成功——不得因撞端口而整体失败
        let Some(blocker) = DummyListener::start() else {
            eprintln!("跳过：无法构造端口占用场景（没有 IPv6 回环？）");
            return;
        };
        let dir = TempDir::new();
        let mut o = options(dir.path());
        o.force_port = Some(blocker.port); // 测试用钩子：强制先试这个被占用的端口

        let d = Daemon::start(o).expect("撞端口后应当换端口重试，而不是失败");
        let _g = guard(&d);
        d.ping().expect("换端口后 RPC 应可用");
    }

    /// 对应 Go `TestDaemonPortConflictFailsFast`。
    ///
    /// 撞端口要**快速**换端口，不能把 10 秒的 RPC 就绪等待期耗满——界面在这期间是冻住的。
    ///
    /// 3 秒这个上界是为判别而设的，不是性能指标：端口真被占住时 aria2 两个协议栈都绑不上，
    /// ~10ms 就退出，走"进程没了就立刻换"这条路本测试不到 1 秒；而退回"只轮询 Ping"的写法时
    /// （或只占 IPv4、让 aria2 在 IPv6 上活着但在 IPv4 上不可达），必然耗满 10 秒。
    #[test]
    fn daemon_port_conflict_fails_fast() {
        let Some(blocker) = DummyListener::start() else {
            eprintln!("跳过：无法构造端口占用场景（没有 IPv6 回环？）");
            return;
        };
        let dir = TempDir::new();
        let mut o = options(dir.path());
        o.force_port = Some(blocker.port);

        let started = Instant::now();
        let d = Daemon::start(o).expect("撞端口后应当换端口重试");
        let elapsed = started.elapsed();
        let _g = guard(&d);
        assert!(
            elapsed <= Duration::from_secs(3),
            "撞端口后换端口用了 {elapsed:?}，说明在干等就绪超时，而不是「进程一退出就换端口」"
        );
        eprintln!("撞端口后换端口耗时 {elapsed:?}");
    }

    /// 对应 Go `TestDaemonForcePortIsHonoredFirst`。
    ///
    /// forcePort 钩子必须真的被优先使用——否则 `daemon_retries_on_port_conflict`
    /// 就是一句空话：把钩子忽略掉、永远直接探空闲端口，它照样通过。这里给它一个**空闲**端口，
    /// 断言 Daemon 确实落在那个端口上（忽略钩子就会落到随机端口，断言即红）。
    #[test]
    fn daemon_force_port_is_honored_first() {
        let l = TcpListener::bind("127.0.0.1:0").expect("绑定端口失败");
        let port = l.local_addr().expect("取端口失败").port();
        // 放开它让 aria2 能绑上。这里的 TOCTOU 窗口与 free_port 同源：真被抢走的话
        // 换端口重试会让下面的断言红，而失败信息会直说是端口对不上。
        drop(l);

        let dir = TempDir::new();
        let mut o = options(dir.path());
        o.force_port = Some(port);
        let d = Daemon::start(o).unwrap_or_else(|e| panic!("在指定端口 {port} 上启动失败: {e}"));
        let _g = guard(&d);

        let want = format!(":{port}/jsonrpc");
        assert!(
            d.rpc_url().ends_with(&want),
            "forcePort 指定的端口没被优先使用: RPCURL={:?}，期望以 {want:?} 结尾",
            d.rpc_url()
        );
    }

    /// 对应 Go `TestDaemonAddDeliversPerTaskOptions`。
    ///
    /// Add 必须把 PerTask 真的交给 aria2，而不是只存在结构体里。
    ///
    /// 判别方式是问 aria2 本人（`aria2.getOption` 返回该任务身上的选项），而不是断言
    /// 我们发出去的请求体——后者只能证明"我们自以为传了"。split 的默认值是 5，
    /// 所以 7 这个取值一旦丢了就看得出来。
    #[test]
    fn daemon_add_delivers_per_task_options() {
        let dir = TempDir::new();
        let mut o = options(dir.path());
        o.per_task = BTreeMap::from([
            ("split".to_string(), "7".to_string()),
            ("max-tries".to_string(), "9".to_string()),
        ]);
        let d = Daemon::start(o).expect("启动失败");
        let _g = guard(&d);

        let gid = d
            .add("http://127.0.0.1:1/none", ".", "p.txt")
            .expect("加任务失败");
        let got = get_option(&d, &gid).expect("getOption 失败");

        assert_eq!(
            got.get("split").map(String::as_str),
            Some("7"),
            "逐任务选项 split 没有随任务送达（aria2 报 {:?}，默认值是 5）",
            got.get("split")
        );
        assert_eq!(
            got.get("max-tries").map(String::as_str),
            Some("9"),
            "逐任务选项 max-tries 没有随任务送达（aria2 报 {:?}）",
            got.get("max-tries")
        );
    }

    /// 对应 Go `TestSetPerTaskAppliesToNextAdd`。
    ///
    /// SetPerTask 让"改设置不必重启引擎"成立：客户改完 `-x`/`-s`/`-k`/重试次数之后，
    /// **之后新加的任务**必须带上新值（设置面板就是这么对客户承诺的）。
    ///
    /// 判别方式同上：问 aria2 本人（`getOption`），而不是断言我们发出去的请求体。
    /// split 的默认值是 5、min-split-size 的默认值是 1M，所以 3 / 50M 一旦丢了就看得出来。
    #[test]
    fn set_per_task_applies_to_next_add() {
        let dir = TempDir::new();
        let mut o = options(dir.path());
        o.per_task = BTreeMap::from([("split".to_string(), "7".to_string())]);
        let d = Daemon::start(o).expect("启动失败");
        let _g = guard(&d);

        let ask = |out: &str| -> BTreeMap<String, String> {
            let gid = d
                .add("http://127.0.0.1:1/none", ".", out)
                .expect("加任务失败");
            get_option(&d, &gid).expect("getOption 失败")
        };

        // 先确认启动时那份**真的**送达了（否则下面"改了才生效"无从谈起）
        let before = ask("before.txt");
        assert_eq!(
            before.get("split").map(String::as_str),
            Some("7"),
            "前置不成立：启动时的 PerTask 没送达，aria2 报 split={:?}",
            before.get("split")
        );

        // 改设置：引擎不重启，新加的任务必须按新值走
        d.set_per_task(BTreeMap::from([
            ("split".to_string(), "3".to_string()),
            ("min-split-size".to_string(), "50M".to_string()),
        ]));
        let got = ask("after.txt");
        assert_eq!(
            got.get("split").map(String::as_str),
            Some("3"),
            "SetPerTask 之后新任务仍按旧值下载：aria2 报 split={:?}，期望 3",
            got.get("split")
        );
        // aria2 的 min-split-size 以字节为单位：50M = 52428800
        assert_eq!(
            got.get("min-split-size").map(String::as_str),
            Some("52428800"),
            "新的 min-split-size 没有送达：aria2 报 {:?}，期望 52428800（50M）",
            got.get("min-split-size")
        );

        // ⚠️ Go 版在这里还有一段「防御性拷贝」断言：调用方改自己手上的 map，引擎不受影响。
        // **Rust 侧结构性不可表达**：`set_per_task` 按值收走 map，调用方此后无法再碰它，
        // 别名根本构造不出来。**不造假对应物**（见报告里的"结构性不适用"清单）。
    }

    /// 对应 Go `TestSetPerTaskIsRaceFreeWithAdd`。
    ///
    /// SetPerTask（界面线程）与 Add（下载线程）会**同时**碰到 `per_task`，
    /// 所以 Add 里那次读必须在锁之下。
    ///
    /// ⚠️ 与 Go 版的判别力**不同**：Go 靠 `-race`（退回无锁读时普通测试全绿、只有 -race 报
    /// DATA RACE）。Rust 侧无锁读根本编译不过（`Mutex` 不给 `&mut`），这条语义由类型系统兜住。
    /// 本条剩下的是**烟雾测试**：并发的 SetPerTask/Add 不得死锁、不得 panic
    /// （例如"在持锁时做 RPC"这种写法会把 5 个线程串死在同一条连接上）。
    #[test]
    fn set_per_task_is_race_free_with_add() {
        let srv = StubHttp::start_with(|_req| {
            RawResponse::new(200)
                .body(r#"{"jsonrpc":"2.0","id":"1","result":"gid-1"}"#.to_string())
        });
        let d = stub_daemon(&srv.base(), "s", BTreeMap::new());
        let d = &d; // 只借用不搬走（Daemon 不是 Copy）

        std::thread::scope(|s| {
            for i in 0..4 {
                s.spawn(move || {
                    for j in 0..30 {
                        d.set_per_task(BTreeMap::from([("split".to_string(), format!("{i}{j}"))]));
                    }
                });
            }
            s.spawn(|| {
                for _ in 0..30 {
                    d.add("http://127.0.0.1:1/x", ".", "f.txt")
                        .expect("加任务失败");
                }
            });
        });
    }

    /// 对应 Go `TestDaemonSnapshotCarriesPathMapCopy`。
    ///
    /// Snapshot 的 `path_map` 是界面把 aria2 的 GID 认回"哪个文件"的唯一凭据
    /// （view 层拿它做 gidToPath）。这里用桩而不是真进程：要断言的只是
    /// "map 被完整带出来了，而且是副本"，不需要起 aria2。
    #[test]
    fn daemon_snapshot_carries_path_map_copy() {
        let srv = StubHttp::start_with(|req| {
            let (method, _) = method_and_params(&req.body);
            let result = if method == "aria2.getGlobalStat" {
                serde_json::json!({
                    "downloadSpeed": "0", "numActive": "0",
                    "numWaiting": "0", "numStopped": "0"
                })
            } else {
                serde_json::json!([]) // tellActive / tellWaiting / tellStopped
            };
            RawResponse::new(200).body(
                serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string(),
            )
        });
        let d = stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::from([
                ("g1".to_string(), "PFX/sub/a.txt".to_string()),
                ("g2".to_string(), "PFX/b.txt".to_string()),
            ]),
        );

        let mut snap = d.snapshot().expect("读快照失败");
        assert!(
            snap.path_map.len() == 2
                && snap.path_map.get("g1").map(String::as_str) == Some("PFX/sub/a.txt")
                && snap.path_map.get("g2").map(String::as_str) == Some("PFX/b.txt"),
            "path_map 没有完整带出来: {:?}",
            snap.path_map
        );
        // 必须是副本：界面拿到的是活 map 的话，它一边遍历我们一边 Add，就是 data race。
        snap.path_map.insert("g1".to_string(), "被改了".to_string());
        assert_eq!(
            d.path_for("g1").as_deref(),
            Some("PFX/sub/a.txt"),
            "Snapshot 交出去的是内部 map 本身，调用方改一下就污染了 Daemon 的记录"
        );
    }

    /// 快照必须带上 aria2 的**原始状态串**（Ruling #88 / 设计规格 §8.4；**非移植**）。
    ///
    /// 钉的是"领域模型**有意**丢掉、但壳需要"的那个区分：aria2 的 `paused` 与 `waiting`
    /// 在 `TaskState` 里是**同一个变体**（契约 §8 表的 `#14`），所以
    /// 设计规格 §8.4 要的「归入下载中并标注已暂停」**只能**从原始串里取。
    ///
    /// 用桩而不是真进程：要断言的是"这趟 `list()` 拿到的原始串被带出来了"，
    /// 不需要起 aria2（也**不该**起——真进程里 `paused` 要靠 pause 一个慢任务才造得出来）。
    ///
    /// 判别力（两条，各钉一件事）：
    ///   ① 填成 `to_task()` 之后的**领域状态串**（`paused` → `"waiting"`）→ 第一条断言红；
    ///   ② 只填 `path_map` 里有的 GID（g2 没有映射）→ 第二条断言红。
    #[test]
    fn daemon_snapshot_carries_raw_aria2_status() {
        let srv = StubHttp::start_with(|req| {
            let (method, _) = method_and_params(&req.body);
            let result = match method.as_str() {
                "aria2.getGlobalStat" => serde_json::json!({
                    "downloadSpeed": "0", "numActive": "1",
                    "numWaiting": "1", "numStopped": "0"
                }),
                // 暂停的任务在 aria2 里落在 `tellWaiting`、状态字面量是 `paused`
                // （任务 11 实测，见 `rpc::pause` 的注释）。
                "aria2.tellWaiting" => serde_json::json!([{
                    "gid": "g1", "status": "paused", "totalLength": "65536",
                    "completedLength": "32768", "downloadSpeed": "0",
                    "connections": "1", "errorMessage": ""
                }]),
                "aria2.tellActive" => serde_json::json!([{
                    "gid": "g2", "status": "active", "totalLength": "1024",
                    "completedLength": "512", "downloadSpeed": "128",
                    "connections": "2", "errorMessage": ""
                }]),
                _ => serde_json::json!([]), // tellStopped
            };
            RawResponse::new(200).body(
                serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string(),
            )
        });
        // g1 有路径映射、g2 **没有**（线上有这种任务：不是从本批清单加进去的）
        let d = stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::from([("g1".to_string(), "PFX/a.txt".to_string())]),
        );

        let snap = d.snapshot().expect("读快照失败");

        // 领域状态：`paused` 与 `waiting` 同义（契约 §8 的 #14）——这里**必须是** Waiting，
        // 否则说明有人给 TaskState 加了变体（那会扰动一张承重的排名表）
        assert_eq!(
            state_of(&snap, "g1"),
            Some(TaskState::Waiting),
            "领域状态不该区分 paused/waiting: {:?}",
            snap.tasks
        );
        // 但原始串必须逐字带出来：壳靠它画「已暂停」
        assert_eq!(
            snap.raw_status.get("g1").map(String::as_str),
            Some("paused"),
            "快照没有带出 aria2 的原始状态串（从 TaskState 反推会得到 \"waiting\"）: {:?}",
            snap.raw_status
        );
        // 覆盖面是全部任务，不是只有 path_map 里那些
        assert_eq!(
            snap.raw_status.get("g2").map(String::as_str),
            Some("active"),
            "没有路径映射的任务也必须带出原始状态: {:?}",
            snap.raw_status
        );
        assert_eq!(
            snap.raw_status.len(),
            2,
            "raw_status 应与 tasks 一一对应: {:?}",
            snap.raw_status
        );
    }

    /// **不计入移植条数**（全局约束 6 的例外，显式记账）：
    /// 同一个 GID 在 `list()` 的三段拼接里出现两次时，`tasks` 与 `raw_status`
    /// **必须说同一句话**（都在生产者这里收敛）。
    ///
    /// 为什么必须有：`raw_status` 是 `BTreeMap`，天然取**最后**一次；`tasks` 是 `Vec`，
    /// 两次都在。于是壳的传输列表里同一行出现两遍——一遍 `active`/0 字节、
    /// 一遍 `complete`，两行互相矛盾（实测：`enqueue` 之后头几百毫秒就是这个形状）。
    /// 而"要折叠"这件事若靠**每个消费者自觉**，今天两个调用点记得、另外两个不记得
    /// ——那正是"同一条规则多份实现"的前身（契约 §1.7 的教训）。
    ///
    /// **判别力**：去掉 `snapshot` 里那次折叠（`tasks` 直接用 `raw.iter().map(to_task)`）
    /// → 第一条断言红（长度 2 而不是 1）。
    ///
    /// 夹具照 aria2 的真实行为写：同一秒里 `tellActive` 还报着它、`tellStopped` 已经在报完成。
    #[test]
    fn daemon_snapshot_folds_duplicate_gids_the_same_way_as_raw_status() {
        let srv = StubHttp::start_with(|req| {
            let (method, _) = method_and_params(&req.body);
            let result = match method.as_str() {
                "aria2.getGlobalStat" => serde_json::json!({
                    "downloadSpeed": "0", "numActive": "1",
                    "numWaiting": "0", "numStopped": "1"
                }),
                // 同一 GID：active 段先报（0 字节、还在传），stopped 段后报（已传完）
                "aria2.tellActive" => serde_json::json!([{
                    "gid": "dup", "status": "active", "totalLength": "1024",
                    "completedLength": "0", "downloadSpeed": "0",
                    "connections": "1", "errorMessage": ""
                }]),
                "aria2.tellStopped" => serde_json::json!([{
                    "gid": "dup", "status": "complete", "totalLength": "1024",
                    "completedLength": "1024", "downloadSpeed": "0",
                    "connections": "0", "errorMessage": ""
                }]),
                _ => serde_json::json!([]), // tellWaiting
            };
            RawResponse::new(200).body(
                serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string(),
            )
        });
        let d = stub_daemon(&srv.base(), "s", BTreeMap::new());

        let snap = d.snapshot().expect("读快照失败");

        assert_eq!(
            snap.tasks.len(),
            1,
            "同一个 GID 出现两次必须在生产者这里折叠掉，否则壳画出两条互相矛盾的行: {:?}",
            snap.tasks
        );
        assert_eq!(snap.tasks[0].gid, "dup", "{:?}", snap.tasks);
        // 与 raw_status 同一条规则：取**最后**一次（= stopped 段那一份）
        assert_eq!(
            snap.tasks[0].state,
            TaskState::Complete,
            "折叠必须取最后一次出现（与 raw_status 的 map 语义一致），实际 {:?}",
            snap.tasks[0].state
        );
        assert_eq!(snap.tasks[0].completed, 1024, "{:?}", snap.tasks[0]);
        assert_eq!(
            snap.raw_status.get("dup").map(String::as_str),
            Some("complete"),
            "两个出口对同一个 GID 必须说同一句话: {:?}",
            snap.raw_status
        );
        assert_eq!(
            snap.raw_status.len(),
            snap.tasks.len(),
            "raw_status 与 tasks 必须一一对应: {:?} / {:?}",
            snap.raw_status,
            snap.tasks
        );
    }

    /// 对应 Go `TestApplyGlobalSendsChangeGlobalOption`。
    ///
    /// ApplyGlobal 是"改参数不重启引擎"的入口，界面会用它，不能是没人走过的路。
    #[test]
    fn apply_global_sends_change_global_option() {
        let srv = StubHttp::start_with(|_req| {
            RawResponse::new(200).body(r#"{"jsonrpc":"2.0","id":"1","result":"OK"}"#.to_string())
        });
        let d = stub_daemon(&srv.base(), "s", BTreeMap::new());

        // 空选项是"没改"，不该白跑一趟 RPC（界面上取消勾选会走到这里）
        d.apply_global(&BTreeMap::new()).expect("空选项不应报错");
        d.apply_global(&BTreeMap::new()).expect("空选项不应报错");
        assert_eq!(
            srv.requests().len(),
            0,
            "空选项不该发 RPC，实际发了 {} 次",
            srv.requests().len()
        );

        d.apply_global(&BTreeMap::from([(
            "max-overall-download-limit".to_string(),
            "1M".to_string(),
        )]))
        .expect("ApplyGlobal 不应失败");
        let reqs = srv.requests();
        assert_eq!(reqs.len(), 1, "非空选项必须发一次 RPC");
        let (method, params) = method_and_params(&reqs[0].body);
        assert_eq!(
            method, "aria2.changeGlobalOption",
            "应走 changeGlobalOption，实际走了 {method}"
        );
        assert_eq!(
            params[1]["max-overall-download-limit"], "1M",
            "全局选项没有送达，实际 {params:?}"
        );
    }

    /// 对应 Go `TestDaemonSecondCloseDoesNotTouchAria2`。
    ///
    /// 可重入不只是"第二次不报错"：第二次必须**什么都不做**。
    ///
    /// 判别点是把 client 换成一个会记账的桩——守卫若被去掉，第二次 Close 会再
    /// shutdown 一次，计数就不再是 0。（只断言"返回 Ok"是抓不住的：Close 本来就
    /// 吞掉错误，重复收尸也只得到一句被忽略的 error。）
    #[test]
    fn daemon_second_close_does_not_touch_aria2() {
        let dir = TempDir::new();
        let mut d = Daemon::start(options(dir.path())).expect("启动失败");
        // 本测试自己控制 Close 的时机（不能 defer），所以失败路径要自己兜底：
        // 一旦在 Close 生效前 panic，进程就没人收了。
        //
        // 依据只是"失败路径也不能留进程"这个道理，别把它读成"aria2 收到 shutdown
        // 也不会自己走"——Go 侧在一次变异跑里观察到过一个存活 >48 秒的孤儿，
        // **但触发条件一直没查明白**。这里只做兜底，不主张任何机制。
        let prec_close = guard(&d);
        d.close().expect("首次关闭失败");
        // 进程真的被回收了：`done` 只在收尸完成之后才置位。少了这条，Close 可能
        // 只是"发完 shutdown 就返回"，把进程留在那里没人收（探路时就这么留过）。
        assert!(
            d.inner.done.is_done(),
            "Close 返回后进程未被回收（done 未置位）"
        );
        // 收尸已经做完，兜底 guard 的任务结束（也让 `d` 的借用到此为止）。
        drop(prec_close);

        let hits = Arc::new(AtomicUsize::new(0));
        let hits_in = Arc::clone(&hits);
        let srv = StubHttp::start_with(move |_req| {
            hits_in.fetch_add(1, AtomicOrdering::SeqCst);
            RawResponse::new(200).body(r#"{"jsonrpc":"2.0","id":"1","result":"OK"}"#.to_string())
        });
        d.client = Arc::new(RpcClient::new(&format!("{}/jsonrpc", srv.base()), "s"));

        d.close().expect("重复关闭不应报错");
        assert_eq!(
            hits.load(AtomicOrdering::SeqCst),
            0,
            "重复关闭不该再动 aria2"
        );
    }

    /// 对应 Go `TestRandomSecretIsFreshEachTime`。
    ///
    /// secret 每次启动都必须新生成：它是 RPC 唯一的凭据，固定下来等于把端口交给别人用。
    #[test]
    fn random_secret_is_fresh_each_time() {
        let mut seen = BTreeSet::new();
        for _ in 0..8 {
            let s = random_secret().expect("生成 secret 失败");
            // 16 字节 → 32 个十六进制字符
            assert_eq!(s.len(), 32, "secret 长度应为 32 个十六进制字符，实际 {s:?}");
            assert!(
                s.bytes().all(|b| b.is_ascii_hexdigit()),
                "secret 应为十六进制，实际 {s:?}"
            );
            assert!(seen.insert(s.clone()), "secret 重复出现：{s:?} —— 随机性没了");
        }
    }

    /// 对应 Go `TestDaemonLaunchArgsPinLoopbackSecretAndResume`。
    ///
    /// 启动参数里的三条硬约束（规格 §3）：只监听回环、必须带 secret、续传恒开。
    /// 断言的是实际传给 aria2 的 argv——这几条一旦某次重构掉了，普通的功能测试
    /// 全都还是绿的（aria2 的默认值恰好等于其中两条），所以得单独钉住。
    #[test]
    fn daemon_launch_args_pin_loopback_secret_and_resume() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let has_arg = |want: &str| d.argv.iter().any(|a| a == want);
        assert!(has_arg("--enable-rpc"), "没开 --enable-rpc");
        assert!(
            has_arg("--rpc-listen-all=false"),
            "RPC 必须只监听回环：缺 --rpc-listen-all=false"
        );
        assert!(has_arg("-c"), "断点续传必须恒开：缺 -c");
        assert!(!d.secret.is_empty(), "RPC 没带 secret");
        assert!(
            has_arg(&format!("--rpc-secret={}", d.secret)),
            "argv 里的 secret 与 client 用的不一致，argv={:?}",
            d.argv
        );
    }

    /// 对应 Go `TestDaemonConcurrentCloseBothWaitForReap`。
    ///
    /// 并发调用 Close 时，**每个**调用者都必须等那唯一的一次关闭真正做完再返回。
    ///
    /// 只断言"第二次不报错"是抓不住的：bool 守卫下第二个调用者会立刻拿到 Ok，
    /// 而那时 aria2 还要优雅退出约 4 秒。GUI 的"关窗口"与"应用终止"两个回调都调
    /// Close 就是这种局面，一个提前返回、紧接着进程退出，孩子就留下了。
    #[test]
    fn daemon_concurrent_close_both_wait_for_reap() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    d.close().expect("关闭失败");
                    // 每个调用者返回时，进程都必须已被回收
                    assert!(
                        d.inner.done.is_done(),
                        "Close 返回时进程尚未回收——并发调用者可能先于收尸返回"
                    );
                });
            }
        });
    }

    // ⚠️ Go 的 `TestDaemonOptionsAreCopiedNotAliased` 在 Rust 侧**结构性不适用**：
    // 那条测试改的是"调用方手上的 map"（`perTask["split"] = "被改了"`），
    // 而 `DaemonOptions` 的字段是**按值持有**的 `BTreeMap`——`start(opts)` 一收走，
    // 调用方连可变借用都拿不到，别名**构造不出来**。断言"Daemon 里那份没被改"
    // 在这里恒真，是一条假测试。按全局约束 6 显式记录、不造假对应物。

    /// 对应 Go `TestDaemonStartTimeoutKillsAndReapsTheProcess`。
    ///
    /// 就绪超时那条路：必须 Kill **并等收尸**，绝不把起不来的进程留在机器上。
    ///
    /// 用一个忽略所有参数、长期不退出的桩（BinaryPath 在真 aria2c 上构造不出超时：
    /// 只要端口能给它就乖乖就绪）逼出这条路径。
    ///
    /// 这里直接叫 `start_on_port` 而不是 `Daemon::start`：外层会对**每次**尝试都重试，
    /// 超时三次就是 30 秒；而这条测试要钉的是"超时那一次做了 Kill + 收尸"。
    /// `Daemon::start` 的错误传播由 `daemon_all_port_attempts_fail_returns_error` 覆盖。
    #[test]
    fn daemon_start_timeout_kills_and_reaps_the_process() {
        let dir = TempDir::new();
        let pid_file = dir.join("aria.pid");
        let stub = stub_binary(dir.path(), &pid_file, "exec sleep 60\n");
        let port = free_port().expect("探端口失败");

        let started = Instant::now();
        let res = start_on_port(&stub, "s3cr3t", port, options(dir.path()));
        let elapsed = started.elapsed();
        if let Ok(d) = res {
            let _ = d.close();
            panic!("桩不提供 RPC，必须超时返回 error，而不是把没就绪的引擎交出去");
        }
        assert!(
            elapsed >= Duration::from_secs(5),
            "只用了 {elapsed:?} 就返回，不像走过了 10 秒的就绪等待（桩应当一直不退出）"
        );
        let pid = read_pid(&pid_file);
        // kill(pid, 0) 对**活着**和**没被收尸**的进程都成功，只有真的 wait 过、
        // PID 被系统收回之后才返回 ESRCH。所以这一条同时钉住了 Kill 与收尸。
        assert!(
            process_gone(pid),
            "超时返回后进程 {pid} 还在（活着或成了僵尸）——超时分支必须 Kill 并等收尸"
        );
    }

    /// 对应 Go `TestDaemonAllPortAttemptsFailReturnsError`。
    ///
    /// 三次尝试全失败那条路：必须返回 error，且每次失败都要立刻换端口而不是干等。
    #[test]
    fn daemon_all_port_attempts_fail_returns_error() {
        let dir = TempDir::new();
        let pid_file = dir.join("aria.pid");
        let stub = stub_binary(dir.path(), &pid_file, "exit 1\n"); // 起来就退

        let started = Instant::now();
        let err = match Daemon::start(DaemonOptions {
            binary_path: Some(stub),
            ..options(dir.path())
        }) {
            Ok(d) => {
                let _ = d.close();
                panic!("三次尝试都失败时必须返回 error");
            }
            Err(e) => e,
        };
        let elapsed = started.elapsed();
        assert!(
            elapsed <= Duration::from_secs(3),
            "三次快速失败用了 {elapsed:?}，说明没走「进程一退就换端口」那条路"
        );
        assert!(
            err.contains("已试 3 个端口"),
            "错误信息应说明试了几个端口，实际: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // 移植自 engine_test.go（任务 5 未领走的那部分）
    // -----------------------------------------------------------------------

    /// 对应 Go `TestExtractWritesBinaryAndVerifiesHash`。
    #[test]
    fn extract_writes_binary_and_verifies_hash() {
        let cache = TempDir::new();
        let path = extract_to(cache.path()).expect("释放失败");
        let md = std::fs::metadata(&path).expect("产物不存在");
        assert!(
            md.permissions().mode() & 0o111 != 0,
            "产物不可执行: {:o}",
            md.permissions().mode()
        );
        // 再释放一次应复用同一个文件（内容一致）
        let raw1 = std::fs::read(&path).expect("读产物失败");
        let path2 = extract_to(cache.path()).expect("第二次释放失败");
        assert_eq!(path2, path, "第二次释放应复用");
        let raw2 = std::fs::read(&path2).expect("读产物失败");
        assert_eq!(sha256(&raw1), sha256(&raw2), "复用后内容不一致");

        // 上面那三条**证明不了"复用"**：路径由哈希派生，天生恒定；内容相同也是
        // 必然（写的就是同一份字节）。一个每次都无脑重写的实现照样全绿。
        // "复用"的实质是**不碰已有的正确文件**，所以要拿 inode 与 mtime 当证据。
        extract_to(cache.path()).expect("第三次释放失败");
        let md2 = std::fs::metadata(&path).expect("读产物元数据失败");
        assert!(
            same_file(&md, &md2),
            "第二次释放换掉了文件（inode 变了）——应当复用，不该重写"
        );
        assert_eq!(
            md.modified().expect("取 mtime 失败"),
            md2.modified().expect("取 mtime 失败"),
            "第二次释放改动了文件时间戳——应当复用，不该重写"
        );
    }

    /// 对应 Go `TestExtractReplacesTamperedFile`。
    #[test]
    fn extract_replaces_tampered_file() {
        let cache = TempDir::new();
        let path = extract_to(cache.path()).expect("释放失败");
        // 篡改已释放的文件
        std::fs::write(&path, b"tampered").expect("篡改失败");
        let path2 = extract_to(cache.path()).expect("再次释放失败");
        let raw = std::fs::read(&path2).expect("读产物失败");
        assert_eq!(
            hex(&sha256(&raw)),
            embed_hash_hex(),
            "被篡改的文件没有被重新释放覆盖"
        );
    }

    /// 对应 Go `TestArgsDefaults`（判定：**移植**，但断言的是**常驻 RPC 模式**的那份 argv）。
    ///
    /// `DefaultArgs()` 是 Go 版**一次性 `-i` 运行**的参数表；Rust 版从第一天就是常驻
    /// RPC（契约 §8 的 `#5`），与之对应的行为是"为 `aria2c --enable-rpc …` 构造 argv
    /// 并 spawn"。那条测试的**判别意图**逐字保留：Go 的注释说得很清楚——
    /// 子串匹配对"多给参数"与"改掉其它参数"都不红，而
    /// `--auto-file-renaming=false` / `--allow-overwrite=true` 直接决定
    /// "落盘路径 = manifest.path 原文"（没有它们，重下会写出 `name.1`），
    /// 所以整条参数表必须**逐字**钉死。
    ///
    /// **不适用**的是 `-i` 那几条（`-i <inputFile>`、`-d`、`-j4` 这类一次性运行的参数），
    /// 见报告里的逐条判定。
    ///
    /// ⚠️ **本次之后这条 argv 按平台分叉了**（裁定 R32 + R9）：Linux 上会多一条
    /// `--ca-certificate=<系统 CA 包>`，macOS 上**一条都不多**。
    /// 所以下面按"**跨平台必须逐字相同的前十条** + **平台特有的尾巴**"两段判：
    ///   - 前十条：**一条都不放宽**（macOS 那串是已经验过的）；
    ///   - 尾巴：macOS 必须**为空**（这正是 R9 那条"不许因为这次改动多一个参数"的
    ///     可执行判据）；Linux 至多一条，且必须真的指向一个存在的文件。
    #[test]
    fn args_defaults() {
        let argv = launch_args(Path::new("/tmp/基准目录"), "s3cr3t", 6800);
        // 「跨平台必须相同」的那十条。**单一来源**：平台特有的尾巴另判，不复制这张表。
        let want: Vec<OsString> = [
            "--enable-rpc",
            "--rpc-listen-all=false",
            "--rpc-secret=s3cr3t",
            "--rpc-listen-port=6800",
            "--dir=/tmp/基准目录",
            "-c",
            "--file-allocation=none",
            "--auto-file-renaming=false",
            "--allow-overwrite=true",
            "--summary-interval=1",
        ]
        .iter()
        .map(OsString::from)
        .collect();

        assert!(
            argv.len() >= want.len(),
            "启动参数比十条基线还短（{} < {}）：{argv:?}",
            argv.len(),
            want.len()
        );
        assert_eq!(
            &argv[..want.len()],
            &want[..],
            "常驻模式的启动参数与规格 §6 不一致（`--auto-file-renaming=false`/\
             `--allow-overwrite=true` 决定落盘路径是否等于 manifest.path 原文，缺一不可）"
        );

        // 平台特有的尾巴。
        let tail = &argv[want.len()..];
        if cfg!(target_os = "macos") {
            // R9 的机械判据：macOS 那条已经验过的 argv **不许因为这次改动多一个参数**。
            assert!(
                tail.is_empty(),
                "macOS 的启动 argv 不该多出参数（argv 是已验过的那十条）：{tail:?}"
            );
        } else if cfg!(target_os = "linux") {
            assert!(
                tail.len() <= 1,
                "Linux 的启动 argv 至多多一条 CA 参数，实际多出 {tail:?}"
            );
            if let Some(arg) = tail.first() {
                let s = arg.to_string_lossy();
                let path = s.strip_prefix("--ca-certificate=").unwrap_or_else(|| {
                    panic!("Linux 多出的那条参数只应是 `--ca-certificate=<路径>`，实际 {s:?}")
                });
                // 探到才传：传进去的必须是**真的存在**的 CA 包，不然等于回到"指了个空路径"。
                assert!(
                    Path::new(path).is_file(),
                    "`--ca-certificate=` 指到的必须是存在的文件，实际 {path:?}"
                );
            }
        }

        // 上面那五条（`-j8`/`-x16`/`-s16`/`--max-tries=3`/`--retry-wait=1`）在 Go 里属于
        // 一次性运行；本内核里同一批量的等价物是**逐任务/全局选项**，经 `addUri`/
        // `changeGlobalOption` 的**结构化字段**下发（`daemon_add_delivers_per_task_options`
        // 与 `set_per_task_applies_to_next_add` 钉住它们真的送达 aria2）。
        // 因此它们**不出现在启动 argv 里**，不是漏了。
    }

    // -----------------------------------------------------------------------
    // 系统 CA 证书包的探测（**R9，非移植**）
    //
    // 为什么要单测这四条：探测的判定是"**取第一个存在的**"，它承重的原因是
    // Linux 那份 aria2c 把构建树的 CA 路径编死了（见 `SYSTEM_CA_BUNDLES`）。
    // 判定写错（比如把 `find` 写成 `last`、或对空表返回了某条默认值）的表现是
    // **静默用了错的 CA 包**——那是"能跑但握手失败"或者"用了别发行版的证书"，
    // 没有任何一条功能测试会红。所以把判定抽成纯函数在这里逐条钉住。
    //
    // 谓词是**注入**的：测试因此不碰文件系统，四个分支（第一个存在 / 第二个存在 /
    // 都不存在 / 多个都存在）在同一个进程里都能构造出来，不需要去 chroot 或造目录。
    // -----------------------------------------------------------------------

    /// 谓词：只有列在 `present` 里的路径算"存在"（用来构造"第 N 个存在"的场景）。
    fn exists_only<'a>(present: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |p: &Path| present.iter().any(|q| Path::new(q) == p)
    }

    /// **候选表本身**：三条路径、**按规格的顺序**、且非空。
    ///
    /// 为什么必须有（审查修复轮 Minor ①）：实测过——把 `SYSTEM_CA_BUNDLES` **清空**、
    /// 或**调换顺序**，全套测试**照样绿**。而"探不到 CA 包"正是 R9 要根除的那个失效形态：
    /// 也就是说 **R9 的修复本身可以在没人察觉的情况下失效**。
    /// 下面那句断言把规格（顺序 + 条数）逐字钉住。
    ///
    /// **不带 `#[cfg]`**：表与主机平台无关，所以这条在任何平台上都跑。
    #[test]
    fn system_ca_bundles_are_the_three_documented_layouts_in_order() {
        let want: &[&str] = &[
            // RHEL / Oracle / CentOS / Fedora
            "/etc/pki/tls/certs/ca-bundle.crt",
            // Debian / Ubuntu / Alpine
            "/etc/ssl/certs/ca-certificates.crt",
            // Alpine 与 macOS 的另一种布局
            "/etc/ssl/cert.pem",
        ];
        assert_eq!(
            SYSTEM_CA_BUNDLES, want,
            "系统 CA 候选表被改了：顺序是规格的一部分（RHEL 系 → Debian/Alpine 系 → 另一布局），\
             清空或调换它都会让 R9 静默失效。真要改，先改规格再改这里。"
        );
        assert!(
            !SYSTEM_CA_BUNDLES.is_empty(),
            "候选表为空 ⇒ 永远探不到 CA 包 ⇒ 等于 R9 没修（https 镜像会以 \
             `unable to get local issuer certificate` 失败）"
        );
    }

    /// **整条链**：表 → 取第一个存在的 → 拼成 `--ca-certificate=<那一条>`。
    ///
    /// 与上面四条 `first_existing_path_*` 的分工：那四条钉"取第一个"的判定，
    /// 这条钉"**表里的哪一条**会被挑中、以及拼出来的参数长什么样"——
    /// 判定对而表错，R9 一样是坏的。
    ///
    /// 谓词是**注入**的（`exists_only`），所以这条**在任何平台上都跑**，
    /// 也不会去碰真实文件系统。**别**为了让它在 macOS 上过而把谓词换成编好的常量 ——
    /// 那样测的就不是这条链了（审查修复轮 Minor ① 明确点过）。
    #[test]
    fn ca_bundle_arg_uses_the_first_existing_candidate() {
        let first = SYSTEM_CA_BUNDLES[0];
        assert_eq!(
            ca_bundle_arg(exists_only(&[first])),
            Some(OsString::from(format!("--ca-certificate={first}"))),
            "只有第一条存在时应挑它，且参数是 `--ca-certificate=<那一条>`"
        );

        let last = SYSTEM_CA_BUNDLES[SYSTEM_CA_BUNDLES.len() - 1];
        assert_eq!(
            ca_bundle_arg(exists_only(&[last])),
            Some(OsString::from(format!("--ca-certificate={last}"))),
            "只有最后一条存在时也应挑到它（不能只认第一条）"
        );

        assert_eq!(
            ca_bundle_arg(|_| false),
            None,
            "一条都不存在 ⇒ `None` ⇒ 调用方不传这个参数（见 `ca_bundle_arg` 的注释）"
        );
    }

    /// 第一个候选存在 ⇒ 取它。
    #[test]
    fn first_existing_path_takes_the_first_candidate() {
        let candidates = ["/a", "/b", "/c"];
        let got = first_existing_path(&candidates, exists_only(&["/a"]));
        assert_eq!(got, Some(PathBuf::from("/a")));
    }

    /// **只有**第二个存在 ⇒ 取第二个（第一个不存在时不能停在 None 上）。
    #[test]
    fn first_existing_path_skips_missing_ones() {
        let candidates = ["/a", "/b", "/c"];
        let got = first_existing_path(&candidates, exists_only(&["/b"]));
        assert_eq!(got, Some(PathBuf::from("/b")));
    }

    /// 一个都不存在 ⇒ `None`（调用方据此**不传** `--ca-certificate`，
    /// 见 `ca_bundle_arg` 的注释：宁可不传，也不要传一条空路径让 aria2c 直接报错）。
    #[test]
    fn first_existing_path_is_none_when_nothing_exists() {
        let candidates = ["/a", "/b", "/c"];
        assert_eq!(first_existing_path(&candidates, |_| false), None);
        // 空候选表也一样：这一条钉的是 `first_existing_path` 的**契约**（纯函数在空表上
        // 必然给 `None`），不是在描述某个平台的现状。
        // ⚠️ **今天没有任何平台会传空表进来** —— `SYSTEM_CA_BUNDLES` 是三条路径的普通
        // `const`（不带 `#[cfg]`），平台门在调用点 `launch_args` 上，而 macOS 那支
        // **根本不调** `ca_bundle_arg`。（这里原来写的是"macOS 走的就是这条"，那是
        // 加平台门之前的形态，已经不成立了。）
        assert_eq!(first_existing_path(&[], |_| true), None);
    }

    /// **多个都存在时取第一个**：顺序是承重的（RHEL 的布局排在最前），
    /// 不能"随便挑一个存在的"。
    #[test]
    fn first_existing_path_prefers_the_earliest_candidate() {
        let candidates = ["/a", "/b", "/c"];
        // 三条全"存在"，只有 `/a` 是对的。
        let got = first_existing_path(&candidates, |_| true);
        assert_eq!(got, Some(PathBuf::from("/a")));
        // 换个顺序，结论跟着换——证明它真的是按**表里的顺序**取第一个，
        // 而不是按谓词的访问顺序或某种字典序。
        let other = ["/c", "/b", "/a"];
        assert_eq!(
            first_existing_path(&other, |_| true),
            Some(PathBuf::from("/c"))
        );
    }

    /// 对应 Go `TestRunSetsWorkingDirToTargetAndPassesArgs`。
    ///
    /// **判定：移植**（`-i` 相关的具体断言不适用，见 `args_defaults`）。
    /// Go 那条钉住两件事：(a) 子进程的工作目录必须是客户选定的目标目录；
    /// (b) 命令行拼装正确。Rust 侧的对应物是 `start_on_port` 的
    /// `current_dir(download_dir)` 与 `args(launch_args)`。
    ///
    /// Go 的注释解释了 (a) 为什么承重：`dir=` 是相对**进程工作目录**解析的，
    /// 工作目录若不对，文件会落到客户端进程的 CWD（GUI 从 Finder 启动时是 `/`）
    /// ——即"落盘基准不一致"。
    #[test]
    fn run_sets_working_dir_to_target_and_passes_args() {
        let tmp = TempDir::new();
        let target = tmp.join("target");
        std::fs::create_dir_all(&target).expect("建目标目录失败");
        let cwd_file = tmp.join("cwd.txt");
        let args_file = tmp.join("args.txt");
        let pid_file = tmp.join("aria.pid");
        // 桩写出 argv 与自己解析后的工作目录，然后**起来就退**：
        // `start_on_port` 走"进程已退出"那条路立刻返回，不必等满 10 秒就绪超时。
        let stub = stub_binary(
            tmp.path(),
            &pid_file,
            &format!(
                ": > '{}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\npwd -P > '{}'\nexit 1\n",
                args_file.display(),
                args_file.display(),
                cwd_file.display(),
            ),
        );
        let port = free_port().expect("探端口失败");
        let _ = start_on_port(&stub, "s", port, options(&target));

        let raw_cwd =
            std::fs::read_to_string(&cwd_file).expect("桩没跑起来或没写出工作目录");
        let got_cwd = std::fs::canonicalize(raw_cwd.trim()).expect("解析工作目录失败");
        let want_cwd = std::fs::canonicalize(&target).expect("解析目标目录失败");
        assert_eq!(got_cwd, want_cwd, "子进程工作目录不对");

        // 期望的 `--dir=` 用**未做符号链接解析**的绝对路径：`absolute_dir` 与 Go 的
        // `filepath.Abs` 一样不碰文件系统，所以 `/var/…` 不会被折成 `/private/var/…`
        // （上面比工作目录时用的是 `pwd -P` 的**物理**路径，两者因此要分别对齐）。
        let raw_args = std::fs::read_to_string(&args_file).expect("读 argv 失败");
        let got_args: Vec<String> = raw_args.lines().map(str::to_string).collect();
        let want_args: Vec<String> = launch_args(&target, "s", port)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(got_args, want_args, "命令行拼装不对");
    }

    // 对应 Go `TestRunSendsOutputToWriter`。
    //
    // **判定：结构性不适用**。Go 那条钉的是**一次性 `-i` 运行**的 `Run(…, out io.Writer)`：
    // aria2 的 stdout/stderr 必须交回调用方（GUI 用它抓进日志框）。本内核走常驻 RPC，
    // 没有 `Run` 这条路径；对应的 Go 版本是 `startOnPort`，而它自己写的是
    // `cmd.Stdout = nil` / `cmd.Stderr = nil`（→ `/dev/null`）——常驻进程的输出**不交回**。
    //
    // Rust 侧照此实现（`Stdio::null()`），并且这条**必须**是 null 而不是 inherit：
    // 全局约束 1 说 stdout 是协议专用通道，子进程的输出**绝不许漏到内核的 stdout 上**。
    // 这个性质在进程内无法断言（libtest 的捕获是线程局部的，接不住子进程写 fd 1），
    // 所以这里**不造假对应物**。（这三行原本是 `///`，即一条**悬空**的文档注释后面
    // 跟着空行再接另一条文档注释——clippy 的 `empty_line_after_doc_comments` 会报它。
    // 它本来就不是某个条目的文档，改成普通注释才是它真实的样子。）

    /// 对应 Go `TestRunResolvesRelativeDirAgainstCallersCWD`。
    ///
    /// 调用方给相对目录时，`--dir=` 与子进程工作目录必须指向**同一个绝对路径**。
    /// 否则子进程换了工作目录之后，`--dir=` 里那个"相对"的含义也跟着变，
    /// 结果会多套一层（`<目标>/<目标>/…`）——落盘基准又歪了。
    ///
    /// ⚠️ 本测试改的是**进程级**工作目录，而 Rust 的测试默认并行。这里安全的原因：
    /// 其余测试用到的路径（`TempDir`、桩二进制、内嵌资源）**全是绝对路径**，
    /// 没有第二条依赖 cwd 的测试。
    #[test]
    fn run_resolves_relative_dir_against_callers_cwd() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.join("target")).expect("建目标目录失败");

        let cwd_before = std::env::current_dir().expect("取当前目录失败");
        std::env::set_current_dir(tmp.path()).expect("切换工作目录失败");
        let _cwd = CwdGuard(cwd_before);

        let cwd_file = tmp.join("cwd.txt");
        let args_file = tmp.join("args.txt");
        let pid_file = tmp.join("aria.pid");
        let stub = stub_binary(
            tmp.path(),
            &pid_file,
            &format!(
                ": > '{}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\npwd -P > '{}'\nexit 1\n",
                args_file.display(),
                args_file.display(),
                cwd_file.display(),
            ),
        );
        let port = free_port().expect("探端口失败");
        let _ = start_on_port(&stub, "s", port, options(Path::new("target")));

        let want = std::fs::canonicalize(tmp.join("target")).expect("解析目标目录失败");
        let raw_cwd = std::fs::read_to_string(&cwd_file).expect("桩没写出工作目录");
        let got_cwd = std::fs::canonicalize(raw_cwd.trim()).expect("解析工作目录失败");
        assert_eq!(got_cwd, want, "子进程工作目录不对");

        let raw_args = std::fs::read_to_string(&args_file).expect("读 argv 失败");
        let args: Vec<String> = raw_args.lines().map(str::to_string).collect();
        let got_dir = args
            .iter()
            .find_map(|a| a.strip_prefix("--dir="))
            .expect("参数里没有 --dir=<目录>");
        assert!(
            Path::new(got_dir).is_absolute(),
            "--dir 必须是绝对路径（工作目录已换，相对路径会被二次解析）: {got_dir:?}"
        );
        let resolved = std::fs::canonicalize(got_dir).expect("解析 --dir 失败");
        assert_eq!(
            resolved, want,
            "--dir 与工作目录不是同一个位置\n--dir 解析为: {}\n工作目录: {}",
            resolved.display(),
            got_cwd.display()
        );
    }

    /// 对应 Go `TestExtractTargetIsOutsideDataDir`。
    ///
    /// 释放位置必须在 Caches 下，不得落在数据目录里。
    #[test]
    fn extract_target_is_outside_data_dir() {
        let home = std::env::var("HOME").expect("取 HOME 失败");
        let want = Path::new(&home)
            .join("Library")
            .join("Caches")
            .join("BenagenDownloader");
        // Go 用的是字符串前缀（`strings.HasPrefix`）；这里用**按路径分量**的前缀，
        // 意图相同而更严：`…/BenagenDownloaderX` 会被它拒掉（字符串前缀会放过）。
        assert!(
            default_cache_dir().starts_with(&want),
            "缓存目录应为 {}...，得到 {}",
            want.display(),
            default_cache_dir().display()
        );
    }

    // -----------------------------------------------------------------------
    // 移植自 embedded_test.go
    // -----------------------------------------------------------------------

    /// 内嵌资产必须满足的"**本平台**能直接执行的可执行文件"判据——按平台分派。
    ///
    /// - macOS：thin 64 位 Mach-O（小端 `MH_MAGIC_64`）。**fat / 通用二进制是
    ///   `cafebabe`/`cafebabf`，同样不放行**——它也是 Mach-O，但不是本内核要的形态
    ///   （见 `ARIA2C_BIN` 的说明：分发形态本就是"每个平台/架构一个独立包"）。
    /// - Linux：64 位小端 ELF。
    ///
    /// 不支持的平台返回 `false`（走不到：本文件顶部的 `compile_error!` 先拦住了）。
    fn is_native_executable(bytes: &[u8]) -> bool {
        if cfg!(target_os = "macos") {
            const MH_MAGIC_64_LE: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
            bytes.len() >= 4 && bytes[..4] == MH_MAGIC_64_LE
        } else if cfg!(target_os = "linux") {
            is_elf64_le(bytes)
        } else {
            false
        }
    }

    /// 失败文案里那句"本平台应该长什么样"——与 `is_native_executable` 同源，免得两处漂。
    fn native_executable_hint() -> &'static str {
        if cfg!(target_os = "macos") {
            "应是**能直接 posix_spawn 的 thin** 64 位 Mach-O（fat/通用二进制是 cafebabe，\
             那同样是 Mach-O 但不是本内核要的形态——见 ARIA2C_BIN 的说明）。"
        } else if cfg!(target_os = "linux") {
            "应是 64 位小端 ELF。"
        } else {
            "本平台不在支持列表里。"
        }
    }

    /// 对应 Go `TestEmbeddedBinaryPresent`。
    #[test]
    fn embedded_binary_present() {
        assert!(
            ARIA2C_BIN.len() >= 1_000_000,
            "内嵌的 aria2c 只有 {} 字节，看起来没被正确 embed",
            ARIA2C_BIN.len()
        );
        // ⚠️ 这条的本意是"内嵌的确实是一个**本平台能直接执行的**可执行文件"，所以魔数
        //    **按平台分派**（macOS 是 Mach-O、Linux 是 ELF），而**不是**整条 `#[cfg]` 掉——
        //    后者等于 Linux 上不再检查这件事（裁定 R32 点了名）。
        assert!(
            is_native_executable(ARIA2C_BIN),
            "内嵌内容不是本平台的可执行文件（前 4 字节 {:?}）：{}",
            &ARIA2C_BIN[..4.min(ARIA2C_BIN.len())],
            native_executable_hint()
        );
        // 光看大小与魔数不够：任何 ≥1MB 的可执行文件都能通过。
        // 断言里面确实有 aria2 的版本串，才算"是它本人"。
        assert!(
            ARIA2C_BIN.windows(6).any(|w| w == b"aria2/"),
            "内嵌内容里找不到 aria2 版本串，可能不是 aria2 的二进制"
        );
    }

    /// 对应 Go `TestEmbeddedLicenseIsGPLv2`。
    #[test]
    fn embedded_license_is_gplv2() {
        let text = license_text();
        assert!(
            !text.is_empty(),
            "license_text() 为空——许可全文没有被 embed 进去，GPL 分发义务未履行"
        );
        // GPLv2 全文约 18KB；太小说明 embed 指错了文件（比如指到了别的短文件）。
        assert!(
            text.len() >= 10_000,
            "许可全文只有 {} 字节，不像 GPLv2 全文（应约 18KB）",
            text.len()
        );
        // 大小够也可能指错文件；断言标题与版本两处特征串。
        // 特意用全大写的标题：内嵌二进制里只有混合大小写的 "GNU General Public License"
        // （运行时打印的声明），拿它冒充通不过这两条。
        for want in ["GNU GENERAL PUBLIC LICENSE", "Version 2"] {
            assert!(
                text.contains(want),
                "许可全文里找不到 {want:?}，可能不是 GPLv2 全文"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 按平台内嵌 aria2c（**非移植**：Go 版只有 arm64 一份资产，没有可对应的测试源）
    // -----------------------------------------------------------------------

    /// 两个架构在 **Mach-O 头**里的 `cputype` 与人类可读名。
    ///
    /// 这是个**查表**（`arch_of_binary` 拿 `cputype` 反查名字用），不是"支持哪些架构"的
    /// 唯一真相——ELF 那一侧看的是 `e_machine`（见 `arch_of_binary`），Linux 的架构名
    /// 由 `ARIA2C_ASSET_NAME` 给。架构名本身跨两种格式是同一套（`arm64`/`x86_64`），
    /// 所以两边能对上。
    ///
    /// 定义在测试模块里而不是生产代码里：生产侧需要知道的只有 `ARIA2C_BIN` 那几条
    /// `#[cfg(all(target_os = …, target_arch = …))]`，多一份枚举就多一处会漂移的真相。
    /// 这里这份的用途是**独立复述"哪个架构对应哪个 cputype"**，好让断言不靠实现自证。
    #[derive(Clone, Copy)]
    struct Arch {
        name: &'static str,
        /// `CPU_ARCH_ABI64 | CPU_TYPE_*`，取值见 `<mach/machine.h>`。
        cpu_type: u32,
    }

    /// `CPU_TYPE_ARM64 | CPU_ARCH_ABI64`。
    const ARM64: Arch = Arch {
        name: "arm64",
        cpu_type: 0x0100_000c,
    };
    /// `CPU_TYPE_X86_64 | CPU_ARCH_ABI64`。
    const X86_64: Arch = Arch {
        name: "x86_64",
        cpu_type: 0x0100_0007,
    };

    /// 本靶应该带哪一份资产——**必须与 `ARIA2C_ASSET_NAME` 的三条 `#[cfg]` 一一对应**。
    ///
    /// ⚠️ 用 `cfg!` **独立复述**，而不是读 `ARIA2C_ASSET_NAME`：读后者是**同义反复**
    /// （同一条 `#[cfg]` 选出来的），发现不了"某条 cfg 指错了平台"。
    ///
    /// 不支持的组合返回 `None`：本文件顶部的 `compile_error!` 已经先一步拦住了，
    /// 走不到这里。
    fn kernel_asset_name() -> Option<&'static str> {
        if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            Some("aria2c-macos-arm64")
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
            Some("aria2c-macos-x86_64")
        } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            Some("aria2c-linux-x86_64")
        } else {
            None
        }
    }

    /// 从 Mach-O 头里取 `cputype`；不是 thin 64 位 Mach-O 时返回 `None`。
    ///
    /// 入参是**任意字节**（下面要拿它验磁盘上的资产）。
    ///
    /// ⚠️ `cffaedfe` 是 `MH_MAGIC_64`（`0xfeedfacf`）**按小端落盘**的样子——字节序已经
    /// 编码进魔数本身。**arm64 与 x86_64 的 thin 产物用的是同一个魔数**，所以魔数断言
    /// 只按**平台**分叉（macOS 是 Mach-O、Linux 是 ELF，见 `is_native_executable`）、
    /// 不需要按**架构**分叉（x86_64 的产物实测过，见任务 1 报告）。
    /// `cputype` 才是区分两个 macOS 架构的字段。
    fn macho_cpu_type(bytes: &[u8]) -> Option<u32> {
        const MH_MAGIC_64_LE: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
        if bytes.len() < 8 || bytes[..4] != MH_MAGIC_64_LE {
            return None;
        }
        Some(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    /// **内嵌的 aria2c 必须与内核自身的（平台，架构）一致**（全局约束 F-1 落到代码上的那条）。
    ///
    /// 为什么这条是承重的：`ARIA2C_BIN` 由 `include_bytes!` 编进内核，运行时释放成独立
    /// 文件再执行。它**不是**由目标机器的架构挑的——内嵌错平台/架构时，
    /// **编译期毫无提示**，而且在"本机 CPU 恰好能原生跑内嵌那一份"的开发机上连跑都跑得过，
    /// 只有拿到真目标机器上才会以 `Bad CPU type in executable`（macOS）/
    /// `cannot execute binary file`（Linux）炸掉。
    ///
    /// 判据是"**两者一致**"，不是"必须是 x86_64"——所以这条在 arm64 构建下同样必须通过。
    ///
    /// 分两步（裁定 R32：**按平台分派**，不再只认 Mach-O）：
    ///   (a) `ARIA2C_ASSET_NAME` 必须是**本靶**该带的那一份（`kernel_asset_name()` 独立复述）；
    ///   (b) `ARIA2C_BIN` 的**字节**必须真的是那个架构（`arch_of_binary` 按文件格式分派）。
    /// **(a) 管"cfg 有没有指对平台"，(b) 管"指到的那份是不是那个架构"**——少了 (a)，
    /// "某条 cfg 分支指回了别的平台的资产"就没人发现。
    ///
    /// 判别力：把某条 `#[cfg]` 指向另一个平台/架构的资产 → (a) 或 (b) 必红
    /// （任务 1 报告「变异自证」一节有实测输出）。在"本机架构恰好等于内嵌那份"的构建下
    /// (b) 是盲区，那由 `shipped_aria2c_asset_matches_its_name` 补。
    #[test]
    fn embedded_aria2c_arch_matches_kernel_arch() {
        let want = kernel_asset_name()
            .expect("本靶不在支持列表里——与 ARIA2C_ASSET_NAME 的 cfg 分支已脱节");

        // (a) 名字这一层：cfg 选出来的资产名必须是本靶该带的那一份。
        assert_eq!(
            ARIA2C_ASSET_NAME, want,
            "cfg 选出来的内嵌资产不是本靶该带的那一份（本靶应为 {want}）——\
             这份内核在目标机器上起 aria2c 会直接失败。"
        );

        // (b) 字节这一层：内嵌的字节必须真的是那个架构。
        let want_arch = want.rsplit('-').next().expect("资产名里连一个 '-' 都没有");
        let got = arch_of_binary(ARIA2C_BIN);
        assert_eq!(
            got,
            Some(want_arch),
            "内嵌的 aria2c 与内核自身的架构不一致：内核是 {want_arch}（{want}），\
             内嵌的那份是 {}。这份内核在目标机器上起 aria2c 会直接失败。",
            got.unwrap_or("不是本平台认识的 64 位可执行格式")
        );
    }

    /// 是不是 **64 位小端 ELF**——`e_ident[EI_CLASS]=2`、`e_ident[EI_DATA]=1`。
    ///
    /// 只看 `\x7fELF` 是不够的：那会把 32 位或大端的文件也当成"是 ELF"，
    /// 于是下面按偏移读出来的"架构"是垃圾值。`is_native_executable` 也用它。
    fn is_elf64_le(bytes: &[u8]) -> bool {
        const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
        bytes.len() >= 20 && bytes[..4] == ELF_MAGIC && bytes[4] == 2 && bytes[5] == 1
    }

    /// 从 ELF 头里取 `e_machine`；不是 64 位小端 ELF 时返回 `None`。
    ///
    /// 布局（`<elf.h>`）：`e_ident[16]` 之后紧跟 `e_type`(2) `e_machine`(2)，
    /// 所以 `e_machine` 在偏移 18，小端 2 字节。
    fn elf_machine(bytes: &[u8]) -> Option<u16> {
        if !is_elf64_le(bytes) {
            return None;
        }
        Some(u16::from_le_bytes([bytes[18], bytes[19]]))
    }

    /// 从二进制字节里读出它**实际**是什么架构；认不出来时返回 `None`。
    ///
    /// 按格式分派：macOS 的产物是 Mach-O（看 `cputype`），Linux 的产物是 ELF（看 `e_machine`）。
    /// 两条都不能只看魔数——魔数只说明**文件格式**，架构在头里的另一个字段。
    ///
    /// ⚠️ **返回的是"资产名里那个架构词"，不是 ELF 的规范名**（审查修复轮 Minor ②）：
    /// ELF 管 64 位 ARM 叫 `aarch64`（`EM_AARCH64`），而本仓库的资产名用的是 `arm64`
    /// （`aria2c-macos-arm64`）。两边**必须同一套**，否则 `shipped_aria2c_asset_matches_its_name`
    /// 会拿"名字说 arm64、字节说 aarch64"判一份**完全正确**的资产为红。
    /// 所以这里把 `EM_AARCH64` 映射回 `arm64`——**加新平台时，
    /// 新资产名的架构词要与本函数这里的映射一并定**。
    /// （`macho_cpu_type` 那条路返回的是 `Arch::name`，两个 macOS 架构也是 `arm64`/`x86_64`，
    /// 与这里自洽。）
    fn arch_of_binary(bytes: &[u8]) -> Option<&'static str> {
        if let Some(cpu) = macho_cpu_type(bytes) {
            return [ARM64, X86_64]
                .iter()
                .find(|a| a.cpu_type == cpu)
                .map(|a| a.name);
        }
        match elf_machine(bytes)? {
            0x3e => Some("x86_64"), // EM_X86_64
            0xb7 => Some("arm64"),  // EM_AARCH64——映射到资产名那套词，见上
            _ => None,
        }
    }

    /// 入库的**本靶**那份 aria2c 资产，**文件名说的架构与它真的是的必须一致**。
    ///
    /// 为什么必须有：资产按平台分文件入库，`ARIA2C_BIN` 的 `#[cfg]` 分支靠 `ARIA2C_ASSET_NAME`
    /// 指过去。某份资产名不符实（最典型的成因：构建脚本漏了 `-arch`，于是 `arch -x86_64 clang`
    /// **静默编出 arm64**，见 `downloader/scripts/build_aria2_macos.sh` 里那段注释）时，
    /// **编译期一无所知**——文件在场、名字对得上（`include_bytes!` 只读字节、不认格式），
    /// 只有真机器上跑才会炸。
    ///
    /// ⚠️ **这是"按平台"枚举，不是过去那种写死 `[ARM64, X86_64]` 的"按架构"枚举**：
    /// 选资产的是 `ARIA2C_ASSET_NAME`，测试就照它走——少一处写死的清单，就少一处会漂移的真相
    /// （旧版本那条注释里记着的"加了架构却忘了补这里"的坑，正是那么来的）。
    ///
    /// ⚠️ **代价（如实记账）**：本测试因此只看**本靶**那一份，不再顺带检查别的平台的资产。
    /// 也就是说，在 macOS arm64 上跑 `cargo test` **不再**会发现 `aria2c-macos-x86_64`
    /// 被换坏了（过去会发现）。别的平台那一份由该平台的构建脚本与
    /// `embedded_aria2c_arch_matches_kernel_arch` 在那边的构建里管。
    ///
    /// 判别力：把 `assets/<ARIA2C_ASSET_NAME>` 换成另一平台/架构的那份 → 必红；
    /// 删掉 → 先红在 `include_bytes!`（编译失败），即使编译过了也会红在下面 `unwrap_or_else`。
    #[test]
    fn shipped_aria2c_asset_matches_its_name() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let path = dir.join(ARIA2C_ASSET_NAME);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "读不到 {}（{e}）——本靶的内核会直接编译失败（include_bytes! 找不到文件）。\
                 用 core/scripts/build_aria2_linux.sh（Linux）或 \
                 downloader/scripts/build_aria2_macos.sh（macOS）产出。",
                path.display()
            )
        });
        // 期望的架构名从**资产名本身**推（`aria2c-<平台>-<架构>` 的最后一段），
        // 而不是另立一张表：这样"名字说的架构"与"字节里的架构"是同一句话的两半。
        let want = ARIA2C_ASSET_NAME
            .rsplit('-')
            .next()
            .expect("资产名里连一个 '-' 都没有");
        let got = arch_of_binary(&bytes);
        assert_eq!(
            got,
            Some(want),
            "{} 的架构与文件名不符：名字说 {}，实际是 {}。\
             名叫某个架构、其实是别的架构的产物，正是本阶段要根除的失效形态。",
            path.display(),
            want,
            got.unwrap_or("不是本平台认识的 64 位可执行格式")
        );
    }

    // -----------------------------------------------------------------------
    // **不计入移植条数**的一条（全局约束 6 的例外，显式记账）
    // -----------------------------------------------------------------------

    /// 手写 SHA-256 的已知答案向量。
    ///
    /// 为什么必须有：Go 的 `TestExtractReplacesTamperedFile` 里那个 `sha256` 来自
    /// `crypto/sha256`——**独立来源**。Rust 侧没有 `sha2` 依赖（本任务不许新增依赖），
    /// 实现是手写的；若测试也用这同一份实现去算，一个"稳定但错"的哈希（例如只吃了
    /// 前 1 KB、或 padding 写错）会**自洽地**通过移植过来的那几条——那正是本项目
    /// 反复踩过的"测试绕开了会出错的那条路"。
    /// 所以这里用 NIST 的公开向量 + **内嵌资产在外部算出的真实摘要**当独立证据。
    ///
    /// ⚠️ **下面每一条向量的期望值都不是从本实现反推的**：五条都在外部用两个独立工具
    /// 逐条核对过（`shasum -a 256` 与 `openssl dgst -sha256`，输入文件按**精确字节数**
    /// 生成、不加换行：0 B / 3 B / 56 B / 1 000 000 B），并且把**本实现的输出**与那两个工具的
    /// 输出逐条 diff 过——五条全一致（命令与输出见任务 10 报告「修复轮 1」一节）。
    /// 在从本实现反推期望值的世界里，这条测试会变成同义反复；上面的交叉验证是它的**前提**，
    /// 不是装饰。改这里的字面量之前，先重跑那两条外部命令。
    ///
    /// 判别力：把实现改成"只哈希前 1000 字节"→ 第 4、5 条必红；padding 写错 → 第 2、4 条必红；
    /// 常量表抄错一位 → 全部必红。
    ///
    /// ⚠️ 期望摘要**按（平台，架构）各一份**（内嵌资产本来就是按平台分文件入库的，
    /// 见 `ARIA2C_ASSET_NAME`）——所以下面那三条 `#[cfg]` 的维度必须与生产侧一致：
    /// 只按架构分的话，Linux x86_64 会**同时命中** macOS x86_64 那条与 Linux 那条，
    /// 直接是"重复定义"的编译错误。
    /// 取值的口径不变：**都在外部用 `shasum -a 256` 与 `openssl dgst -sha256`
    /// 两个独立工具算过**（每一份都核过，结果逐字一致），改字面量之前先重跑：
    /// `shasum -a 256 core/assets/<ARIA2C_ASSET_NAME>`。
    ///
    /// 🔴 **2026-09-20 两个架构的字面量同时换过**：内嵌的 aria2c 在那天按
    /// `-mmacosx-version-min=13.0` 重编（客户那台 Intel 封顶 macOS 13，
    /// 而旧产物声明的是 `minos 26.0`，装不上）。两个架构都重建了，所以
    /// **两份摘要都必须更新**——只改一个的表现是另一条 `cargo test` 红。
    /// 顺带：重建清掉了旧 arm64 里 `-g` 留下的调试映射（本文件与
    /// `downloader/scripts/build_aria2_macos.sh` 都有记账）。
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    const ARIA2C_EMBED_SHA256: &str =
        "2e9ead567d00d51b93b5cd46a9fbcc6afdc03ac834c45a6d0306613c51e07df4";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    const ARIA2C_EMBED_SHA256: &str =
        "bc1cce95471ed0c792be5723b146413b2093c5248f28483b1b4cf4c1de1cafd4";
    /// Linux x86_64 那一份（musl 静态链接，Task 1 的产物）。
    ///
    /// 取值口径与上面两条**逐字相同**：`shasum -a 256` 与 `openssl dgst -sha256`
    /// 各算一遍、逐字一致才填进来（两条命令的读数见 task-8 报告）。
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const ARIA2C_EMBED_SHA256: &str =
        "e167cf7dc0b1d4ab079a0d8ca98b29b02f380b417af0c40fe21c372690e6aee7";
    #[test]
    fn sha256_known_answer_vectors() {
        let cases: [(&[u8], &str); 4] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                &[b'a'; 1_000_000], // FIPS 180-4 的百万 'a' 向量：跨块 + 多轮 padding
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(
                hex(&sha256(input)),
                want,
                "SHA-256 已知答案不符（输入 {} 字节）",
                input.len()
            );
        }

        // 内嵌资产的摘要用系统工具（`shasum -a 256`）在**外部**算过一次，
        // 写死在这里当第二重独立证据——它同时钉住"内嵌的确实是那一份文件"
        // （**本靶**的那一份，见 `ARIA2C_ASSET_NAME` / `ARIA2C_EMBED_SHA256`）。
        //
        // 失败文案里的文件名直接用 `ARIA2C_ASSET_NAME`，不再像过去那样
        // `if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" }`：
        // 那个写法把平台维悄悄抹成了"不是 arm64 就是 macOS x86_64"，
        // 加进 Linux 之后它会指着一个**不存在**的 `core/assets/aria2c-macos-x86_64`
        // 让人去查——把"资产名"这件事收敛到一处，它就不会再漂。
        assert_eq!(
            embed_hash_hex(),
            ARIA2C_EMBED_SHA256,
            "内嵌 aria2c 的摘要与外部算出的不一致——先确认 core/assets/{ARIA2C_ASSET_NAME} \
             是脚本产出的那份，再核对 ARIA2C_EMBED_SHA256"
        );
    }

    // -----------------------------------------------------------------------
    // 传输列表动作（**任务 11 新增，非移植**）
    //
    // 这一层跑**真实 aria2c**，理由（简报里给过）：
    //   - `Daemon` 持有的是真实进程，要拿 HTTP 桩去测它就得先给它加一个注入口，
    //     那是**不必要的接口**——`daemon_snapshot_carries_path_map_copy` 那种
    //     "不需要真进程"的测试才用 `stub_daemon`；
    //   - 本层要测的恰恰是**只有真实进程才能暴露的东西**：`forget` 有没有真的接上
    //     （`by_gid` 是内存态，桩测不出"移除之后还挂着映射"的后果）、
    //     分派有没有打在 aria2 真正接受的那个方法上、清空的顺序对不对。
    // -----------------------------------------------------------------------

    /// 轮询快照直到 `pred` 成立；超时则 panic（带上 `what` 说明在等什么）。
    ///
    /// 用轮询而不是固定 `sleep`：aria2 把任务从 active 挪到 stopped 的时刻不由我们决定，
    /// 固定睡眠要么慢、要么在负载高的机器上偶发假红。
    fn wait_until<F>(d: &Daemon, what: &str, pred: F) -> Snapshot
    where
        F: Fn(&Snapshot) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snap = d.snapshot().expect("读快照失败");
            if pred(&snap) {
                return snap;
            }
            if Instant::now() >= deadline {
                panic!("10 秒内没等到：{what}（当前快照: {snap:?}）");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 某个 GID 在快照里的状态；不在快照里则 `None`。
    fn state_of(snap: &Snapshot, gid: &str) -> Option<TaskState> {
        snap.tasks.iter().find(|t| t.gid == gid).map(|t| t.state)
    }

    /// 一个只监听、**从不 accept 也从不回话**的下载源。
    ///
    /// 用它当 URL 的任务会**永远停在 `active`**：TCP 握手由内核 backlog 完成
    /// （客户端 connect 成功），此后一个字节都不回。这正是构造"活动任务"夹具所需的
    /// 确定性——用真实文件反而会因为下载太快而**偶发地**在断言之前就完成。
    ///
    /// ⚠️ 不能用 `DummyListener` 顶替：那个是"占住端口造端口冲突"的，
    /// 语义不同（虽然实现相近），借用它会让两条测试的意图互相污染。
    struct StuckSource {
        port: u16,
        _l: TcpListener,
    }

    impl StuckSource {
        fn start() -> Self {
            let l = TcpListener::bind("127.0.0.1:0").expect("绑定下载源端口失败");
            let port = l.local_addr().expect("取下载源端口失败").port();
            Self { port, _l: l }
        }

        fn url(&self, name: &str) -> String {
            format!("http://127.0.0.1:{}/{name}", self.port)
        }
    }

    /// 一份只有一个固定小文件的 HTTP 源（用现成的 `StubHttp`）：任务能**真的下完**。
    ///
    /// ⚠️ **每个并发下载要各配一个 `StubHttp`**。`StubHttp` 的接收线程是
    /// 「一次只服务一条连接」的：内层 `while let Some(req) = read_request(...)`
    /// 会在同一条连接上一直等到**读超时（10 秒）**才回去 `accept` 下一条。
    /// aria2 下完第一个文件后**不关连接**，于是第二个下载的连接躺在 backlog 里
    /// 干等满 10 秒——实测症状是"一个文件 Complete、另一个永远 Active 且 total=0"，
    /// 看着像引擎坏了，其实是桩的吞吐限制。（`testutil.rs` 本任务不许改，用多开绕开。）
    fn file_source() -> StubHttp {
        StubHttp::start_with(|_req| RawResponse::new(200).body("hello"))
    }

    /// 一个**没有东西在监听**的地址：用它当 URL 的任务会立刻失败（ECONNREFUSED），
    /// 最终落在 `TaskState::Error`。用来造"已失败"这个已停止形态的夹具。
    fn closed_url(name: &str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").expect("探端口失败");
        let port = l.local_addr().expect("取端口失败").port();
        drop(l); // 一放开就没人监听了；aria2 连过去必然被拒
        format!("http://127.0.0.1:{port}/{name}")
    }

    /// 移除一个**活动**任务：走 `aria2.remove`，且必须忘掉 GID 映射。
    ///
    /// 判别力：
    ///   - 去掉 `forget` → 第一条断言必红（映射还在）；
    ///   - `remove` 一律改调 `removeDownloadResult` → 实测 aria2 回 400
    ///     `Could not remove download result of GID#…`，`expect` 必红。
    #[test]
    fn daemon_remove_active_task_dispatches_to_remove_and_forgets() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let src = StuckSource::start();
        let gid = d.add(&src.url("slow.bin"), ".", "slow.bin")
            .expect("加任务失败");
        wait_until(&d, "任务变成 active", |s| {
            state_of(s, &gid) == Some(TaskState::Active)
        });
        assert_eq!(d.path_for(&gid).as_deref(), Some("slow.bin"), "前置不成立：映射没建立");

        d.remove(&gid)
            .expect("移除活动任务必须成功（这条走 aria2.remove）");

        // ① 映射必须被忘掉。不 forget 的表现是**传输列表里移除不掉的一行**：
        //    界面拿 path_for 还能认出一个已经不在 aria2 里的任务。
        assert!(
            d.path_for(&gid).is_none(),
            "移除之后 GID→路径映射必须被忘掉，实际仍是 {:?}",
            d.path_for(&gid)
        );
        // ② aria2 侧真的不动了。⚠️ 这条**不能**断"从快照里消失"：`aria2.remove`
        //    会把活动任务挪进 `tellStopped`（状态 `removed`），它照样出现在快照里。
        let snap = d.snapshot().expect("读快照失败");
        assert_ne!(
            state_of(&snap, &gid),
            Some(TaskState::Active),
            "移除之后任务还在下载中"
        );

        // ③ 同一行**再点一次移除**：`aria2.remove` 把活动任务挪进 `tellStopped` 时
        //    状态是 `removed`——那是分派表里的第三种"已停止"。少了这一条，
        //    `matches!` 里的 `Removed` 那一支**没有任何测试走过**（`remove` 的分派是
        //    本任务新增的语义，这一支同样没有先例可依）。
        //    先等到它真的落到 `Removed` 再点：不等的话第二次会赶在 aria2 落状态之前，
        //    走的还是 `aria2.remove`，那就测不到这一支了。
        wait_until(&d, "任务落到 removed", |s| {
            state_of(s, &gid) == Some(TaskState::Removed)
        });
        d.remove(&gid)
            .expect("对已经移除过的任务再点一次移除，应当仍然成功（走 removeDownloadResult）");
        assert!(
            d.path_for(&gid).is_none(),
            "重复移除之后映射仍必须是空的"
        );
    }

    /// 移除一个**已完成**任务：必须走 `removeDownloadResult`，且必须忘掉 GID 映射。
    ///
    /// 这是本任务新增的语义，Go 无先例。判别力：`remove` 不分派、一律调 `aria2.remove`
    /// → 实测 aria2 回 400 `Active Download not found for GID#…`，`expect` 必红。
    /// **没有这一条，"移除一个已完成的任务"从一开始就是坏的**——而传输列表里
    /// 已完成的条目恰恰是最常被移除的那种。
    #[test]
    fn daemon_remove_completed_task_dispatches_to_remove_download_result() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let srv = file_source();
        let gid = d.add(&format!("{}/f.txt", srv.base()), ".", "f.txt")
            .expect("加任务失败");
        wait_until(&d, "任务下载完成", |s| {
            state_of(s, &gid) == Some(TaskState::Complete)
        });

        d.remove(&gid)
            .expect("移除已完成的任务必须成功（这条走 aria2.removeDownloadResult）");

        assert!(
            d.path_for(&gid).is_none(),
            "移除之后 GID→路径映射必须被忘掉，实际仍是 {:?}",
            d.path_for(&gid)
        );
        // 已完成条目被 removeDownloadResult 真的清掉了：它应当从 tellStopped 里消失，
        // 于是整份快照里再也看不到它（这同时证明方法真的打对了，而不是"恰好没报错"）。
        let snap = d.snapshot().expect("读快照失败");
        assert_eq!(
            state_of(&snap, &gid),
            None,
            "已完成条目仍留在 tellStopped 里，说明它没被真的清掉"
        );
    }

    /// `clear_finished`：清空之后 `by_gid` 里**不得残留**那些 GID，且不得动在下载的任务。
    ///
    /// 判别力：
    ///   - 把顺序改成"先 purge 再取 GID 快照"（那个已知陷阱）→ 两条"忘掉"的断言必红
    ///     （purge 之后 `tellStopped` 已空，取到的 GID 集合是空的，一条都没 forget）；
    ///   - 把过滤器收窄成只认 `Complete` → `b.txt`（那条**失败**的）必红；
    ///   - 把 `path_map` 整份清掉/忘掉所有 GID → `live`（在下载的）那条必红。
    #[test]
    fn daemon_clear_finished_forgets_stopped_gids_and_keeps_live_ones() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        // 两个"已停止"的夹具**故意取两种不同的形态**：一个 `Complete`、一个 `Error`。
        // 只用一种的话，过滤器里的另一支没有任何测试走过——把 `matches!` 改成
        // 只认 `Complete`，测试照样全绿（本项目反复栽过的"只守住了一半"）。
        let srv_a = file_source();
        let done1 = d.add(&format!("{}/a.txt", srv_a.base()), ".", "a.txt")
            .expect("加任务失败");
        let done2 = d.add(&closed_url("b.txt"), ".", "b.txt")
            .expect("加任务失败");
        // 一个永远下不完的任务：它既不在 tellStopped 里、也不该被 forget。
        let src = StuckSource::start();
        let live = d.add(&src.url("live.bin"), ".", "live.bin")
            .expect("加任务失败");

        wait_until(&d, "一个下完、一个失败", |s| {
            state_of(s, &done1) == Some(TaskState::Complete)
                && state_of(s, &done2) == Some(TaskState::Error)
        });
        wait_until(&d, "第三个任务在下载中", |s| {
            state_of(s, &live) == Some(TaskState::Active)
        });

        d.clear_finished().expect("清空已完成不应失败");

        for (gid, name) in [(&done1, "a.txt"), (&done2, "b.txt")] {
            assert!(
                d.path_for(gid).is_none(),
                "{name} 被清空后仍留着 GID→路径映射（先 purge 再取快照就会这样：\
                 tellStopped 已空，取到的 GID 集合是空的），实际仍是 {:?}",
                d.path_for(gid)
            );
        }
        assert_eq!(
            d.path_for(&live).as_deref(),
            Some("live.bin"),
            "清空已完成不得动在下载的任务——否则界面会认不出它"
        );
        let snap = d.snapshot().expect("读快照失败");
        assert_eq!(state_of(&snap, &done1), None, "已完成条目应已被 purge");
        assert_eq!(state_of(&snap, &done2), None, "已失败条目应已被 purge");
        assert_eq!(
            state_of(&snap, &live),
            Some(TaskState::Active),
            "清空已完成不得影响在下载的任务"
        );
    }

    /// `clear_finished` 也必须忘掉 `Removed` 条目。
    ///
    /// 为什么这一支必须纳入（修复轮 1，控制者裁定 Q1）：`purgeDownloadResult` 会
    /// **一并清掉 `removed` 条目**（实测），清掉之后 `by_gid` 里若还留着它们，
    /// 那条映射就**永久残留**——正是"先取快照再 purge"这条顺序规则存在的理由。
    /// 对一个已经被 forget 过的 GID 再 forget 一次是 no-op，所以**纳入的代价严格为零、
    /// 方向单向安全**："不纳入是可能泄漏，纳入是最多多做一次无用功"。
    ///
    /// ⚠️ 夹具的构造方式：直接调 `d.client.remove(gid)`，**绕过 `Daemon::remove`**。
    /// 后者在两条路径上都会 `forget`，所以它**构造不出**这个状态；
    /// 而这里要测的恰恰是"aria2 那边已经进了 removed、我们这边的映射还在"。
    /// 在本模块内直接碰私有字段 `d.client` 有先例
    /// （`daemon_second_close_does_not_touch_aria2` 就整个换过它）——这是测试侧的构造手段，
    /// **不是**给生产加接口。写法照 `Error` 那一支（`daemon_clear_finished_...`）的样式。
    ///
    /// 判别力：把 `Removed` 从过滤器里去掉 → `path_for` 仍是 `Some`，本条必红。
    #[test]
    fn daemon_clear_finished_forgets_removed_entries_too() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let src = StuckSource::start();
        let gid = d.add(&src.url("r.bin"), ".", "r.bin")
            .expect("加任务失败");
        wait_until(&d, "任务变成 active", |s| {
            state_of(s, &gid) == Some(TaskState::Active)
        });

        // 让 aria2 把任务置成 removed，而**我们的映射留着**（见上文：绕过 Daemon::remove）
        d.client.remove(&gid).expect("aria2.remove 不应失败");
        wait_until(&d, "任务落到 removed", |s| {
            state_of(s, &gid) == Some(TaskState::Removed)
        });
        assert_eq!(
            d.path_for(&gid).as_deref(),
            Some("r.bin"),
            "前置不成立：这一支要测的正是「aria2 已 removed、映射还在」"
        );

        d.clear_finished().expect("清空不应失败");

        assert!(
            d.path_for(&gid).is_none(),
            "`Removed` 条目已被 purge 清掉，映射却留在 by_gid 里（永久残留），实际仍是 {:?}",
            d.path_for(&gid)
        );
        let snap = d.snapshot().expect("读快照失败");
        assert_eq!(state_of(&snap, &gid), None, "removed 条目应已被 purge 清掉");
    }

    /// 暂停 / 继续一个任务：走真实 aria2，两个动作都要真的改变任务的状态。
    ///
    /// 判别力：`Daemon::pause` 若只是空转（或调错成 `unpause`），第二条 `wait_until`
    /// 必红（任务停在 active）；`unpause` 空转则最后一条必红。
    ///
    /// 顺带钉住一个**映射事实**：aria2 把暂停的任务放在 `tellWaiting` 里、状态字面量是
    /// `paused`，而 `status::raw_status_to_state` 把 `paused` 与 `waiting` 同义处理
    /// （契约 §8 的 `#14`），所以这里看到的是 `Waiting`。暂停**不该**动 GID 映射。
    #[test]
    fn daemon_pause_then_unpause_round_trips() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let src = StuckSource::start();
        let gid = d.add(&src.url("p.bin"), ".", "p.bin")
            .expect("加任务失败");
        wait_until(&d, "任务变成 active", |s| {
            state_of(s, &gid) == Some(TaskState::Active)
        });

        d.pause(&gid).expect("暂停不应失败");
        wait_until(&d, "任务离开 active（进入 tellWaiting）", |s| {
            state_of(s, &gid) == Some(TaskState::Waiting)
        });
        assert_eq!(
            d.path_for(&gid).as_deref(),
            Some("p.bin"),
            "暂停不该动 GID→路径映射"
        );

        d.unpause(&gid).expect("继续不应失败");
        wait_until(&d, "任务回到 active", |s| {
            state_of(s, &gid) == Some(TaskState::Active)
        });
    }

    /// 移除一个 aria2 根本不认识的 GID：错误必须**带出来**。
    ///
    /// 判别力：把 `?` 换成 `let _ =`（吞掉错误）→ 本条必红。
    /// 为什么必须有：这三个动作的调用方是界面，错误被吞掉之后用户点了"移除"
    /// 什么都不会发生、也没有任何提示（规格 §9 那条"任何一类失败都必须出现在界面上"）。
    ///
    /// 断言里带 GID 而不是断言 aria2 的完整文案：**文案会随版本变**
    /// （实测 1.37.0 是 `GID <gid> is not found`，简报里记的 `Invalid GID` 是旧的），
    /// 而"错误里认得出是哪个 GID"这件事不会变。
    #[test]
    fn daemon_remove_unknown_gid_surfaces_the_error() {
        let dir = TempDir::new();
        let d = Daemon::start(options(dir.path())).expect("启动失败");
        let _g = guard(&d);

        let err = d
            .remove("0123456789abcdef")
            .expect_err("移除一个 aria2 不认识的 GID 必须报错，不能吞成 Ok(())");
        assert!(
            err.contains("0123456789abcdef"),
            "错误信息里应认得出是哪个 GID（aria2 的 message 必须带出来），实际: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 修复轮 2：`remove` 的两条**桩**测试
    //
    // 为什么这两条用 `stub_daemon` + `StubHttp` 而不是真进程：它们测的是
    // 「判据走哪一趟读」与「某趟读失败时怎么办」，而"让真实的 aria2 在一条**合法**移除上
    // 报错"（或"让 `getGlobalStat` 单独挂掉"）没有不引入新接口的做法。
    // 先例：`daemon_snapshot_carries_path_map_copy`。
    // -----------------------------------------------------------------------

    /// 桩的一条成功响应（JSON-RPC 200）。
    fn rpc_ok(result: serde_json::Value) -> RawResponse {
        RawResponse::new(200).body(
            serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": result}).to_string(),
        )
    }

    /// 桩的一条错误响应（aria2 把错误放在 `error` 里并回 400）。
    fn rpc_err(message: &str) -> RawResponse {
        RawResponse::new(400).body(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "1",
                "error": {"code": 1, "message": message},
            })
            .to_string(),
        )
    }

    /// 桩里的一条 `tellActive` 条目（线上形态：数值都是字符串）。
    fn active_entry(gid: &str) -> serde_json::Value {
        serde_json::json!({
            "gid": gid,
            "status": "active",
            "totalLength": "0",
            "completedLength": "0",
            "downloadSpeed": "0",
            "connections": "1",
            "errorMessage": "",
        })
    }

    /// `remove` 失败时**不得**丢掉映射（错误路径上**没有** forget）。
    ///
    /// 判别力：把 `self.forget(gid)` 从 RPC **之后**提到**之前**（= 失败也 forget）
    /// → 本条必红（`path_for` 变 `None`）。修复轮 2 的审查实测过：不加这条时，
    /// 那个变异体在**全套 62 条 engine 测试下存活**。
    ///
    /// 后果的形状是**不得静默少交**：`remove` 失败而映射没了，界面手上就出现
    /// 一个"aria2 里活着、却没有路径可落位"的任务，而且此后任何 `clear_finished` 都
    /// 认不回它（它不在 stopped 里）。
    ///
    /// ⚠️ 这里刻意让任务在 aria2 那边是 **active**（不是"不在快照里"）：
    /// 报告「自审发现 7」曾说这件事"没有可观察差异"——那只对"未知 GID"那个场景成立
    /// （那里 `path_for` 本来就是 `None`）。**对一般的 RPC 失败不成立**，本条就是那个反例。
    #[test]
    fn daemon_remove_failure_keeps_the_gid_mapping() {
        let gid = "aaaaaaaaaaaaaaaa";
        let srv = StubHttp::start_with(move |req| {
            let (method, _) = method_and_params(&req.body);
            match method.as_str() {
                // 这个任务在 aria2 那边是**活着**的，映射也在
                "aria2.tellActive" => rpc_ok(serde_json::json!([active_entry(gid)])),
                "aria2.tellWaiting" | "aria2.tellStopped" => rpc_ok(serde_json::json!([])),
                "aria2.getGlobalStat" => rpc_ok(serde_json::json!({
                    "downloadSpeed": "0", "numActive": "1",
                    "numWaiting": "0", "numStopped": "0",
                })),
                // 而"移除"这一趟失败
                "aria2.remove" => rpc_err("boom"),
                other => rpc_err(&format!("意外的方法: {other}")),
            }
        });
        let d = stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::from([(gid.to_string(), "keep.txt".to_string())]),
        );

        assert!(
            d.remove(gid).is_err(),
            "桩对 aria2.remove 回了 400，remove 必须报错"
        );
        assert_eq!(
            d.path_for(gid).as_deref(),
            Some("keep.txt"),
            "移除失败时映射必须留着——丢掉它就是「静默少交」：\
             aria2 里任务还活着，界面却没有路径可以落位，此后 clear_finished 也认不回它"
        );
    }

    /// `remove` 的判据**不得**依赖 `getGlobalStat`。
    ///
    /// `snapshot()` = `list()` 的 3 条 + `global()` 的 1 条，而 `getGlobalStat` 对
    /// "该用哪个方法"**零贡献**。更要紧的是失败模式：它一挂，"移除"就连试都不会试地
    /// 返回错误——**一个与移除毫不相干的 RPC 能把用户点的移除挡住**。
    /// 用一个大而全的读去回答一个小问题，就会继承它那些与自己无关的失败模式。
    ///
    /// 判别力：把 `remove` 的判据换回 `snapshot()` → 本条必红
    /// （`global()` 那趟 400 会直接把 `remove` 顶回去）。跑全套。
    #[test]
    fn daemon_remove_does_not_depend_on_global_stat() {
        let gid = "bbbbbbbbbbbbbbbb";
        let srv = StubHttp::start_with(move |req| {
            let (method, _) = method_and_params(&req.body);
            match method.as_str() {
                "aria2.tellActive" => rpc_ok(serde_json::json!([active_entry(gid)])),
                "aria2.tellWaiting" | "aria2.tellStopped" => rpc_ok(serde_json::json!([])),
                // 与"移除"毫不相干的一趟读，坏掉
                "aria2.getGlobalStat" => rpc_err("getGlobalStat 挂了"),
                "aria2.remove" => rpc_ok(serde_json::json!("OK")),
                other => rpc_err(&format!("意外的方法: {other}")),
            }
        });
        let d = stub_daemon(
            &srv.base(),
            "s",
            BTreeMap::from([(gid.to_string(), "live.bin".to_string())]),
        );

        d.remove(gid)
            .expect("getGlobalStat 挂掉不该挡住移除——它对这个决定零贡献");
        assert!(
            d.path_for(gid).is_none(),
            "移除成功后映射必须被忘掉，实际仍是 {:?}",
            d.path_for(gid)
        );
    }

    // -----------------------------------------------------------------------
    // 结构性不适用清单（对应 Go 的测试，逐条判定后**不移植**）
    // -----------------------------------------------------------------------

    // ⚠️ `TestInputFileFormat` —— **不适用**（简报与契约 §8 的 `#5` 都已判定）：
    // Rust 版从第一天就走 JSON-RPC，`BuildInputFile` 生成的 `-i` 输入文件在本内核里
    // 不存在；而"空白分词即选项边界"那条注入路径**与它同生共死**（契约 §7 的
    // `validateBaseURL` 缺口，唯一触发条件就是 `-i` 的空白分词）。造一个假的对应物
    // 等于把一条已经不存在的边界画回图上。

    // ⚠️ 传输列表动作**没有可移植的测试源**：Go 内核没有 `pause`/`unpause`/`remove`
    // 的封装（设计规格 §8.4：这一段是相对 Go 版的新增）。本节的 **8** 条测试（任务 11
    // 首轮 5 条 + 修复轮 1 的 `Removed` 1 条 + 修复轮 2 的桩测试 2 条）+ `rpc.rs`
    // 那一节的 10 条 = **18 条**全部是**新写**的，按全局约束 6 在报告里逐条记账。
    //
    // ⚠️ **这个数字是活账**：再加测试时必须同步改这里。数字错了，下一个人就核不上账，
    // 而"记账"这条约束的全部价值就在于它**能被核对**。
}
