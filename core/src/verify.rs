//! verify —— 下载完成后逐个文件复校验 CRC-64/XZ。
//!
//! 逐条移植自 `downloader/internal/verify/{verify.go,verify_test.go}`（Go 侧已冻结）。
//! 16 条测试与 Go 一一对应，对应表见
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-7-report.md`。
//!
//! 这是整个客户端的核心承诺：aria2 不知道 CRC64，只有外壳能判定"落盘的内容确实是服务端那份"。
//! 校验失败的必须重新入队，绝不能当成成功。
//!
//! ⚠️ **六类互斥、穷尽**：每个文件必须落进恰好一类，不允许有文件落在任何一类之外——
//! 每一类都会出现在最终报告里，这是"不得静默少交"的具体落实（契约 §5.1）。
//!
//! ⚠️ **本模块不持任何锁、不接触任何全局可变状态**（契约 §5.2）。50 GB 批量下
//! 这里是一连几分钟的 CRC64 磁盘 I/O；`check` 的返回形状
//! （`(CheckResult, Vec<(String, Entry)>)`）就是为了让调用方把"长 I/O"与"短写入"分开：
//! 待记录的条目**由调用方在持锁时写入**。

use std::path::Path;

use crate::crc64xz;
use crate::delivery::File;
use crate::engine::{mtime_secs, safe_rel_path};
use crate::state::Entry;

/// 一次全量校验的分类结果。
///
/// 六类互斥，且**不允许有文件落在任何一类之外**——每一类都会出现在最终报告里，
/// 这是"不得静默少交"的具体落实。
///
/// ⚠️ 类型名是 `CheckResult` 而不是 Go 的 `verify.Result`：本模块同时要用标准库的
/// `Result<T, E>`，直接叫 `Result` 会在模块内**遮蔽标准 `Result`**，`check` 的返回类型
/// 当场编译不过。**这是有意的改名，不是笔误。**
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckResult {
    /// 校验通过
    pub ok: Vec<String>,
    /// crc64 不符，或路径不可信（越界/控制字符），或该路径上不是普通文件（目录、FIFO、设备）
    pub bad: Vec<String>,
    /// 本地不存在
    pub missing: Vec<String>,
    /// 大小不符
    pub size_mismatch: Vec<String>,
    /// 清单与 HEAD 都没给出 crc64
    pub unverifiable: Vec<String>,
    /// 能 stat 到但读不了（权限/IO 错误）
    pub unreadable: Vec<String>,
}

impl CheckResult {
    /// 表示没有需要处理、或无法确认的文件。
    ///
    /// `unreadable` 也算"未确认"，因此计入 `false`：读不了就无法证明它是完整的，
    /// 不能让客户以为交付齐全。
    ///
    /// `unverifiable` **不**计入（没有 crc64 可比，重下也不产生可比对象），
    /// 但它必须由调用方单独报出——这条区分由 `all_good_semantics` 钉住。
    pub fn all_good(&self) -> bool {
        // 逐字对应 Go 的 `len(Bad)+len(Missing)+len(SizeMismatch)+len(Unreadable) == 0`。
        // `unverifiable` **不**在其中——这条区分由 `all_good_semantics` 钉住。
        self.bad.len() + self.missing.len() + self.size_mismatch.len() + self.unreadable.len() == 0
    }

    /// 返回需要重新入队的文件（按清单顺序，不重复）。
    ///
    /// 规格 §8：**校验不符 → 重新入队**；再次不符才记入未完成清单。
    ///
    /// ⚠️ 返回 `bad + missing + size_mismatch`——**不含 `unverifiable`**
    /// （没有 crc64 可比，重下也不产生可比对象），**也不含 `unreadable`**
    /// （本地权限/IO 问题，重下解决不了；报告里单列并提示客户处理即可）。
    /// 这条区分由 `result_failures_selects_reenqueue_set` 守。
    pub fn failures(&self) -> Vec<String> {
        // 顺序照 Go：bad、missing、size_mismatch（`result_failures_selects_reenqueue_set`
        // 按这个顺序断言）。
        let mut out =
            Vec::with_capacity(self.bad.len() + self.missing.len() + self.size_mismatch.len());
        out.extend_from_slice(&self.bad);
        out.extend_from_slice(&self.missing);
        out.extend_from_slice(&self.size_mismatch);
        out
    }
}

/// 把第二轮（对失败项重下后的复校验）结果并入第一轮。
///
/// 语义：失败项在第二轮里的归类才是最终归类，所以 `bad`/`missing`/`size_mismatch`
/// 取 `retry` 的；其余三类在"本轮全表"与"上轮失败项"之间不可能相交，取并集。
///
/// ⚠️ **`first` 的三张失败表根本不参与输出**（`bad`/`missing`/`size_mismatch` **整体
/// 取 `retry` 的**，不是并集）——本函数只看 `first` 的 `ok`/`unverifiable`/`unreadable`。
/// 这条曾经被写成一个比事实严重的"调用方契约"（"少一个就会让那个文件从报告里消失"），
/// **在当前调用点上不成立**，已改准：
///
///   - "多一个"不可能：调用方（`main.rs` 的 `spawn_verify_worker`）传的就是
///     `res.failures() ∩ !retried`，而 `res` 是这一轮对 `job.files` 逐条判出来的——
///     `retry ⊆ first.failures()` **恒成立**；
///   - "少一个"（某个失败项在上一轮已经重试过、这一轮不再重试）**不会让它从报告里消失**：
///     救它的是**累积器**而不是本函数——`commit(&mut k, res)` 在发复校验 job **之前**
///     就**无条件**执行过了，那一整轮结果早已写进 `k.verify`；随后的
///     `commit(&mut k, merged)` 只摘 `merged` 里**出现过**的路径，所以未被重试的
///     失败项**保持它的第一轮归类**。
///
/// 钉住这条的是 `partial_retry_keeps_unretried_failures_in_their_first_round_class`
/// （一条测试而不是 `debug_assert!`：断言会把一个今天无害的条件变成崩溃点，测试把语义固定下来）。
///
/// 因此这里只做归类替换，不做集合校验。
pub fn merge(first: CheckResult, retry: CheckResult) -> CheckResult {
    // `ok`/`unverifiable`/`unreadable` 取并集（first 在前，与 Go 的 append 顺序一致）。
    let mut ok = first.ok;
    ok.extend(retry.ok);
    let mut unverifiable = first.unverifiable;
    unverifiable.extend(retry.unverifiable);
    let mut unreadable = first.unreadable;
    unreadable.extend(retry.unreadable);
    CheckResult {
        ok,
        // 失败项在第二轮里的归类才是最终归类：这三类**整体替换**而不是并集。
        bad: retry.bad,
        missing: retry.missing,
        size_mismatch: retry.size_mismatch,
        unverifiable,
        unreadable,
    }
}

/// 逐文件校验 `dir` 下的内容是否与清单一致。
///
/// 任何单个文件的问题都不会中止循环：每个文件必定落进六类之一。
///
/// ⚠️ **返回待记录的条目，由调用方在持锁时写入**（契约 §5.2）：
/// 50 GB 批量下这是**几分钟的 CRC64 磁盘 I/O**，若在锁内跑，其他调用全会阻塞。
/// 返回 `(结果, 待写入的 (path, Entry) 列表)` 是把"长 I/O"与"短写入"分开的最直接形状。
/// **本函数自己不持任何锁、不接触任何全局可变状态。**
///
/// 判定顺序（**有顺序，不要重排**）：
///   1. `safe_rel_path` 不过 → `bad` + continue
///   2. `symlink_metadata` 出错 → `missing` + continue
///   3. 非普通文件（目录、FIFO、设备、socket）→ `bad` + continue
///   4. 大小不符 → `size_mismatch` + continue
///   5. `crc64` 为空 → `unverifiable` + continue
///   6. `crc64xz::sum_file` 出错 → `unreadable` + continue
///   7. 数值相符 → `ok`，并推入待写列表
///   8. 数值不符 → `bad`
pub fn check(dir: &Path, files: &[File]) -> (CheckResult, Vec<(String, Entry)>) {
    let mut res = CheckResult::default();
    let mut records: Vec<(String, Entry)> = Vec::new();

    for f in files {
        // 报告侧的越界判据必须与落盘侧（`engine::safe_rel_path`）**同一条**：
        // 落盘时被拒的路径如果能走到这里，就会拼出一个目标目录之外的绝对路径去
        // stat/读——那是在校验客户目录之外的东西，结论既不可信，也把"报告只描述目标目录"
        // 的边界打破了。
        // **直接复用引擎的判据而不是在本地重写一份**：两份判据一旦漂移，就会重新出现
        // "落盘侧拒了、报告侧却认了"的缝。（verify → engine 不成环：engine 只依赖 delivery。）
        if !safe_rel_path(&f.path) {
            res.bad.push(f.path.clone());
            continue;
        }
        let full = dir.join(&f.path);
        // 用 `symlink_metadata`（= Go 的 `os.Lstat`）而不是 `metadata`（= `os.Stat`）：
        // 要看得出"这个位置**本身**是什么"，而不是它指向什么——符号链接在本内核里
        // 不是"我们要的那份文件"，见下面非普通文件那一段。
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            res.missing.push(f.path.clone());
            continue;
        };
        // 非普通文件（目录、FIFO、设备、socket、符号链接）一律归 bad。
        //
        // FIFO 这一条是硬要求：`stat().size()` 对 FIFO 恒为 0，而清单里 `size: 0`
        // 的空文件在生产交付里是正常的——于是一个本地恰好是 FIFO 的位置会**通过大小判据**、
        // 进入读文件，而 FIFO 在没有写者时 `open` 会**永久阻塞**。那会让整个校验再也
        // 回不来（契约 §5.2 之前，它是在界面锁内被调的），客户只能强杀。
        // 宁可判"不符、需要重下"，也不能让一次校验把应用钉死。
        // （目录也在此列：期望是文件、路径上却是个目录，本地这份不是我们要的东西。）
        if !meta.file_type().is_file() {
            res.bad.push(f.path.clone());
            continue;
        }
        if meta.len() as i64 != f.size {
            res.size_mismatch.push(f.path.clone());
            continue;
        }
        if f.crc64.is_empty() {
            // 没有校验值可比 —— 单列，绝不当作"通过"。
            // **不重入队**（`failures()` 不含这一类）：重下也不产生可比对象。
            res.unverifiable.push(f.path.clone());
            continue;
        }
        let sum = match crc64xz::sum_file(&full) {
            Ok(s) => s,
            // 读不了（权限/IO）。**不中止整个循环**——中止会让其余文件得不到分类，
            // "不得静默少交"就落空了。单列出来，由上层在报告里列出。
            // （也不重入队：那是本地权限问题，重下解决不了。）
            Err(_) => {
                res.unreadable.push(f.path.clone());
                continue;
            }
        };
        if sum.to_string() != f.crc64 {
            res.bad.push(f.path.clone());
            continue;
        }
        res.ok.push(f.path.clone());
        // **只有校验通过的才进待写列表**：写错会让下一次扫描直接跳过坏文件
        // （planner 的跳过决策全部建立在"状态里的 crc64 是上次校验通过的证据"上）。
        //
        // 记录**由调用方在持锁时写入**（契约 §5.2）——本函数只是把它交出去，
        // 自己不持锁、不接触任何全局可变状态。
        let mtime = match meta.modified() {
            Ok(t) => mtime_secs(t),
            // 元数据拿不到 mtime（在支持的文件系统上不会发生）。失败方向是安全的：
            // 记 0.0 只会让下一次扫描匹配不上、退化成重新下载，绝不会造成"无证据的跳过"。
            // 绝不在这里另写一份换算——量纲只有 `engine::mtime_secs` 一份。
            Err(_) => 0.0,
        };
        records.push((
            f.path.clone(),
            Entry {
                size: meta.len() as i64,
                mtime,
                crc64: f.crc64.clone(),
            },
        ));
    }

    (res, records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as StdPath;

    use crate::testutil::TempDir;

    /// `"readme"` 的真实 TOS CRC-64/XZ（与 Go 侧同名常量逐字相同）。
    const README_CRC: &str = "5432380796884633278";

    /// 对应 Go 的 `write(t, dir, rel, data)`。
    fn write_file(dir: &StdPath, rel: &str, data: &[u8]) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录失败");
        }
        std::fs::write(&full, data).expect("写文件失败");
    }

    /// 对应 Go 的 `delivery.File{Path: p, Size: n, CRC64: c}`。
    fn file(path: &str, size: i64, crc64: &str) -> File {
        File {
            path: path.to_string(),
            size,
            crc64: crc64.to_string(),
            source_mtime: String::new(),
        }
    }

    /// 磁盘上这个文件真实的 mtime（状态文件记录用的量纲）。
    ///
    /// ⚠️ 与 Go 的 `mtimeOf` 助手一样，这里与实现共用 `engine::mtime_secs`，
    /// 因此**测不出**"换算公式本身被换成另一种写法"——那条性质由 `engine` 里的
    /// `mtime_secs_matches_go_unixnano_formula` 单独钉住（任务 7 步骤 0）。
    /// 这条助手在这里的作用只有一个：证明 `verify` 记的是**文件自己的** mtime，
    /// 而不是某个常量。
    fn mtime_of(dir: &StdPath, rel: &str) -> f64 {
        let meta = std::fs::metadata(dir.join(rel)).expect("stat 失败");
        mtime_secs(meta.modified().expect("mtime 不可用"))
    }

    /// 对应 Go 测试里的 `if os.Geteuid() == 0 { t.Skip(...) }`。
    ///
    /// Rust 的 std 没有 `geteuid`（本任务不许加依赖），这里改用**直接探针**：
    /// 对一个已经 `chmod 000` 的文件试着 `open`，能打开就说明权限检查在这个环境里
    /// 不生效（root、或忽略权限的文件系统），此时"读不了"这条测试失去判别力，跳过。
    /// 比 `geteuid()==0` 更准：它探的是这条测试真正依赖的那个前提。
    fn perms_enforced(locked: &StdPath) -> bool {
        std::fs::File::open(locked).is_err()
    }

    /// 对应 Go 的 `syscall.Mkfifo`。Rust 的 std 没有 `mkfifo`，本任务又不许加依赖，
    /// 因此走 POSIX 的 `mkfifo(1)`；失败（无此命令 / 文件系统不支持）时与 Go 一样跳过。
    fn mkfifo(path: &StdPath) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// 对应 Go `TestCheckAllGood`。
    #[test]
    fn check_all_good() {
        let dir = TempDir::new();
        write_file(dir.path(), "a/b.txt", b"readme");
        let (res, _records) = check(dir.path(), &[file("a/b.txt", 6, README_CRC)]);
        assert!(
            res.ok.len() == 1
                && res.bad.len() + res.missing.len() + res.size_mismatch.len() + res.unverifiable.len()
                    == 0,
            "应全部通过，得到 {res:?}"
        );
        assert!(res.all_good(), "all_good 应为真");
    }

    /// 对应 Go `TestCheckDetectsCorruption`。
    #[test]
    fn check_detects_corruption() {
        // 大小相同、内容不同——这正是"只比大小"会漏掉、而 CRC64 能抓到的情形
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readme");
        let (res, _records) = check(dir.path(), &[file("a.txt", 6, "12345")]);
        assert_eq!(res.bad.len(), 1, "应检出 1 项不符，得到 {res:?}");
        assert!(!res.all_good(), "all_good 应为假");
    }

    /// 对应 Go `TestCheckMissingAndSizeMismatch`。
    #[test]
    fn check_missing_and_size_mismatch() {
        let dir = TempDir::new();
        write_file(dir.path(), "short.txt", b"abc");
        let (res, _records) = check(
            dir.path(),
            &[
                file("nope.txt", 1, README_CRC),
                file("short.txt", 999, README_CRC),
            ],
        );
        assert_eq!(res.missing.len(), 1, "应记 1 项缺失: {res:?}");
        assert_eq!(res.size_mismatch.len(), 1, "应记 1 项大小不符: {res:?}");
    }

    /// 对应 Go `TestCheckEmptyCRCIsUnverifiable`。
    #[test]
    fn check_empty_crc_is_unverifiable() {
        // crc64 为空（清单没给、HEAD 也没补到）时，既不得判"通过"也不得判"损坏"，
        // 必须单列，由上层在最终报告里明确标注——这是"不得静默"的具体落实。
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readme");
        let (res, _records) = check(dir.path(), &[file("a.txt", 6, "")]);
        assert_eq!(res.unverifiable.len(), 1, "空校验值应记入 unverifiable: {res:?}");
        assert!(
            res.ok.is_empty() && res.bad.is_empty(),
            "不得计入 ok 或 bad: {res:?}"
        );
    }

    /// 对应 Go `TestCheckUpdatesState`。
    ///
    /// ⚠️ **签名偏离**：Go 的第三个参数是可选的 `*state.State`，Rust 侧没有它
    /// （契约 §5.2——长 I/O 不得在锁内跑）。等价物是**断言返回的待写入列表**：
    /// "通过的文件才产生待写条目、不符的文件一条都不产生"。
    #[test]
    fn check_updates_state() {
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readme");
        let (res, records) = check(dir.path(), &[file("a.txt", 6, README_CRC)]);
        assert_eq!(res.ok.len(), 1, "应通过: {res:?}");
        assert_eq!(records.len(), 1, "通过的文件应产生一条待写记录: {records:?}");
        let (path, e) = &records[0];
        assert_eq!(path, "a.txt", "状态键必须是清单里的相对路径原文");
        assert_eq!(e.crc64, README_CRC, "记录里的 crc64 不对: {e:?}");
        assert_eq!(e.size, 6, "记录里的 size 不对: {e:?}");
        assert_eq!(
            e.mtime,
            mtime_of(dir.path(), "a.txt"),
            "记录里的 mtime 必须来自文件系统、且用与 planner 同一个换算函数: {e:?}"
        );

        // 不符的文件**不得**产生待写记录——否则下次会被误跳过
        write_file(dir.path(), "b.txt", b"readme");
        let (res2, records2) = check(dir.path(), &[file("b.txt", 6, "999")]);
        assert_eq!(res2.bad.len(), 1, "b.txt 应判不符");
        assert!(
            records2.is_empty(),
            "校验不符的文件不得产生待写记录: {records2:?}"
        );
    }

    /// 对应 Go `TestCheckSameSizeDifferentContentIsBad`。
    ///
    /// 构造"落盘内容与服务端那份不同、但字节数恰好相同"的情形：清单给的是 `"readme"`
    /// 的真实 CRC64，而盘上是另一个同样 6 字节的内容 `"readmi"`。
    ///
    /// 加它的理由：`check_detects_corruption` 用的清单值是随手编的 `"12345"`，
    /// 并不对应任何内容，因此它证明的是"比对了 crc64"，还不是"内容不同能被 CRC64 抓到"。
    /// 真正的承重场景是：大小一模一样、只比对大小必然放过、只有复算 CRC64 才能发现。
    #[test]
    fn check_same_size_different_content_is_bad() {
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readmi"); // 与 "readme" 等长，内容不同
        let (res, _records) = check(dir.path(), &[file("a.txt", 6, README_CRC)]);
        assert_eq!(
            res.bad,
            vec!["a.txt".to_string()],
            "等长但内容不同必须判 crc64 不符: {res:?}"
        );
        assert!(
            res.ok.len() + res.missing.len() + res.size_mismatch.len() + res.unverifiable.len() == 0,
            "除 bad 外不得落进其它类: {res:?}"
        );
        assert!(!res.all_good(), "all_good 应为假");
    }

    /// 对应 Go `TestCheckStateOnlyWrittenForVerified`。
    ///
    /// 把"只有校验通过（且 crc64 非空）的文件才允许写入状态文件"这条不变式钉住。
    ///
    /// 为什么这条不变式是承重的：planner 在"清单 crc64 为空 + HEAD 失败 + 状态命中"时
    /// 会跳过文件，其全部正当性来自"状态里的 crc64 是上次校验通过的证据"。
    /// 一旦不符/缺失/大小不符/无法校验/读取失败的条目也被写进去，跳过就变成了无证据的跳过。
    #[test]
    fn check_state_only_written_for_verified() {
        let dir = TempDir::new();
        write_file(dir.path(), "ok.txt", b"readme");
        write_file(dir.path(), "bad.txt", b"readme");
        write_file(dir.path(), "short.txt", b"abc");
        write_file(dir.path(), "nocrc.txt", b"readme"); // 必须真的存在，否则会先落进 missing
        write_file(dir.path(), "locked.txt", b"readme");
        let locked = dir.join("locked.txt");
        set_mode(&locked, 0o000);
        if !perms_enforced(&locked) {
            set_mode(&locked, 0o644);
            eprintln!("跳过：权限检查在本环境不生效（root？），locked.txt 不会落进 unreadable");
            return;
        }
        std::fs::create_dir_all(dir.join("adir")).expect("建目录失败");

        let (res, records) = check(
            dir.path(),
            &[
                file("ok.txt", 6, README_CRC),
                file("bad.txt", 6, "999"),
                file("short.txt", 999, README_CRC),
                file("missing.txt", 6, README_CRC),
                file("nocrc.txt", 6, ""),
                file("locked.txt", 6, README_CRC),
                file("adir", 4096, README_CRC),
            ],
        );
        set_mode(&locked, 0o644);
        // 六类互斥且穷尽：7 个文件必须正好落满 6 类，一个不漏、一个不重。
        // （bad 2 项：crc64 不符的 bad.txt + 路径上是目录的 adir。）
        assert!(
            res.ok.len() == 1
                && res.bad.len() == 2
                && res.missing.len() == 1
                && res.size_mismatch.len() == 1
                && res.unverifiable.len() == 1
                && res.unreadable.len() == 1,
            "六类应分别 1/2/1/1/1/1 项: {res:?}"
        );

        // 只有 ok.txt 有资格被记入状态。
        // 用条目总数兜底：任何"多写了一条"的实现都会在这里现形。
        assert_eq!(records.len(), 1, "状态记录应只有 1 条，实际: {records:?}");
        assert_eq!(records[0].0, "ok.txt", "只有通过校验的文件才可被记录");
        assert_eq!(records[0].1.crc64, README_CRC);
    }

    /// 对应 Go `TestCheckNonASCIIAndSpacedPaths`。
    ///
    /// 覆盖规格 §9.4：含 ×、空格、中文的路径在校验这一处也必须正确——既要比对得上，
    /// 也要以**清单里的原文**作为状态键（planner 用 `f.path` 反查状态，
    /// 键一旦被规范化/转码，跳过就永远命中不了）。
    #[test]
    fn check_non_ascii_and_spaced_paths() {
        const REL: &str = "测试 目录/a ×b.txt";
        let dir = TempDir::new();
        write_file(dir.path(), REL, b"readme");
        let (res, records) = check(dir.path(), &[file(REL, 6, README_CRC)]);
        assert_eq!(res.ok, vec![REL.to_string()], "非 ASCII 路径应校验通过且保持原文: {res:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].0, REL,
            "状态键必须是清单里的相对路径原文——planner 用 f.path 反查，用绝对路径就再也命中不了"
        );
        assert_eq!(records[0].1.crc64, README_CRC);
    }

    /// 对应 Go `TestAllGoodSemantics`。
    ///
    /// 把 `all_good` 的判据钉死："没有需要处理、或无法确认的文件"。
    ///
    /// `unverifiable` 不计入其中，但它**必须**由调用方单独报出；
    /// `unreadable` 则**计入**：读不了就无法证明它是完整的，不能让客户以为交付齐全
    /// ——这与"无法校验"的区别是，前者根本没读到内容。
    /// 这条测试的意义是让"哪些类算需要处理"成为被显式固定的契约，而非注释里的口头约定。
    #[test]
    fn all_good_semantics() {
        let cases: [(&str, CheckResult, bool); 8] = [
            (
                "全通过",
                CheckResult {
                    ok: vec!["a".into()],
                    ..Default::default()
                },
                true,
            ),
            (
                "crc64 不符",
                CheckResult {
                    bad: vec!["a".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                "缺失",
                CheckResult {
                    missing: vec!["a".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                "大小不符",
                CheckResult {
                    size_mismatch: vec!["a".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                "读取失败",
                CheckResult {
                    unreadable: vec!["a".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                "仅无法校验",
                CheckResult {
                    unverifiable: vec!["a".into()],
                    ..Default::default()
                },
                true,
            ),
            (
                "通过 + 无法校验",
                CheckResult {
                    ok: vec!["a".into()],
                    unverifiable: vec!["b".into()],
                    ..Default::default()
                },
                true,
            ),
            (
                "通过 + 读取失败",
                CheckResult {
                    ok: vec!["a".into()],
                    unreadable: vec!["b".into()],
                    ..Default::default()
                },
                false,
            ),
        ];
        for (name, res, want) in cases {
            let got = res.all_good();
            assert_eq!(got, want, "{name}: all_good = {got}, 期望 {want}");
        }
    }

    /// 对应 Go `TestCheckNilState`。
    ///
    /// ⚠️ **签名偏离**：Go 传 `nil` 状态。Rust 侧没有状态参数，等价的断言是
    /// "没有状态只影响记录、不影响校验结论，且待写列表照常返回、由调用方决定要不要写"。
    #[test]
    fn check_nil_state() {
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readme");
        let (res, records) = check(dir.path(), &[file("a.txt", 6, README_CRC)]);
        assert!(
            res.ok.len() == 1 && res.all_good(),
            "没有状态只影响记录，不影响校验结论: {res:?}"
        );
        assert_eq!(
            records.len(),
            1,
            "待写记录照常返回，由调用方决定是否写入: {records:?}"
        );
    }

    /// 对应 Go `TestCheckUnreadableFileIsReportedNotFatal`。
    ///
    /// 钉住：文件能 stat 到、但读不出来时，**不得中止整个校验循环**——中止会让其后
    /// 所有文件都得不到分类，"不得静默少交"就落空了。
    ///
    /// 它必须单列进 `unreadable`：既不能当成通过（没读过的内容不能判成完好），
    /// 也不能当成 `missing`（文件明明在）或 `bad`（未经比对不能断言内容不符）。
    #[test]
    fn check_unreadable_file_is_reported_not_fatal() {
        let dir = TempDir::new();
        write_file(dir.path(), "a.txt", b"readme");
        write_file(dir.path(), "locked.txt", b"readme");
        let locked = dir.join("locked.txt");
        set_mode(&locked, 0o000);
        if !perms_enforced(&locked) {
            set_mode(&locked, 0o644);
            eprintln!("跳过：权限检查在本环境不生效（root？），本测试失去判别力");
            return;
        }

        let (res, records) = check(
            dir.path(),
            &[file("a.txt", 6, README_CRC), file("locked.txt", 6, README_CRC)],
        );
        set_mode(&locked, 0o644);
        assert_eq!(res.ok.len(), 1, "a.txt 应通过: {res:?}");
        assert_eq!(
            res.unreadable,
            vec!["locked.txt".to_string()],
            "locked.txt 应记入 unreadable: {res:?}"
        );
        assert!(!res.all_good(), "有读不了的文件时 all_good 应为假");
        assert_eq!(
            records.len(),
            1,
            "读不了的文件不得产生待写记录: {records:?}"
        );
    }

    /// 对应 Go `TestCheckPathThatIsFIFOIsBadNotBlocked`。
    ///
    /// 钉住：清单里的位置在本地是个 FIFO 时，必须判 `bad`，**绝不能**走到 `sum_file`。
    ///
    /// 为什么这条后果最重：FIFO 的 `stat().size()` 恒为 0，而清单里 `size: 0` 的条目
    /// （空文件）在生产交付里是正常的——于是它**通过大小判据**、进入读文件，
    /// 而 FIFO 在没有写者时 `open` 会**永久阻塞**。那会让整个校验再也回不来，
    /// 客户只能强杀应用。
    ///
    /// 用超时兜底：若实现退化成"只挡目录"，这条测试会**卡住**而不是断言失败——
    /// 卡住同样是判别力（且正是客户会遭遇的现象），用 `recv_timeout` 把它转成可读的失败。
    #[test]
    fn check_path_that_is_fifo_is_bad_not_blocked() {
        let dir = TempDir::new();
        let fifo = dir.join("pipe");
        if !mkfifo(&fifo) {
            eprintln!("跳过：本平台/本文件系统不支持 mkfifo");
            return;
        }

        // size 声明为 0：正是"能通过大小判据"的那个值（FIFO 的 stat.size 也是 0）
        let (tx, rx) = std::sync::mpsc::channel();
        let d = dir.path().to_path_buf();
        std::thread::spawn(move || {
            let res = check(&d, &[file("pipe", 0, README_CRC)]);
            let _ = tx.send(res);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok((res, records)) => {
                assert_eq!(
                    res.bad,
                    vec!["pipe".to_string()],
                    "FIFO 应归入 bad（不可信、需重下）: {res:?}"
                );
                assert!(
                    res.ok.len()
                        + res.missing.len()
                        + res.size_mismatch.len()
                        + res.unverifiable.len()
                        + res.unreadable.len()
                        == 0,
                    "FIFO 不得落进其它类: {res:?}"
                );
                assert!(records.is_empty(), "FIFO 不得产生待写记录: {records:?}");
            }
            Err(_) => panic!(
                "校验在 FIFO 上卡住了——这正是要防的那个 bug（无写者时 open 永久阻塞，客户只能强杀）"
            ),
        }
    }

    /// 对应 Go `TestCheckUnsafePathIsBad`。
    ///
    /// 钉住：报告侧的落点判据必须与落盘侧（`engine::safe_rel_path`）**同一条**。
    /// 含 `..` 的清单路径会在拼接之后指向目标目录**之外**，对那个位置 stat/读，
    /// 等于在校验客户目录之外的东西——结论不可信，也越界了。
    #[test]
    fn check_unsafe_path_is_bad() {
        let dir = TempDir::new();
        // 造一个"目标目录之外"的真实文件：若实现不挡，`../outside.txt` 会命中它，
        // 内容还恰好与清单一致——于是校验会"通过"，把越界读出来的东西当成客户的数据。
        //
        // ⚠️ **"目录外"必须用另一个 `TempDir` 来表达，不能写 `dir.path().parent()`**：
        // 那是**系统临时目录本身**（`$TMPDIR` 根），而固定文件名 + 只在成功路径清理会带来
        // 三个**仓库之外**的副作用——① 断言失败（panic 展开）时 `remove_file` 不执行，
        // 文件留在系统临时目录里；② 成功路径会删掉任何同名的、与本项目无关的文件；
        // ③ 并发跑两份测试（两个 CI job，或 CI 与本地同用户）时互相删对方的夹具 ⇒ 假红。
        // 自己的 `TempDir` 的 `Drop` 在失败路径上同样生效，名字还带 pid + 序号。
        // 语义一字不变：这个文件仍在 `dir` **之外**。
        let outside_dir = TempDir::new();
        let outside = outside_dir.join("outside.txt");
        std::fs::write(&outside, b"readme").expect("写 outside.txt 失败");

        for p in [
            "../outside.txt",
            "a/../../outside.txt",
            "/etc/hosts",
            "x\n  dir=/tmp",
        ] {
            let (res, records) = check(dir.path(), &[file(p, 6, README_CRC)]);
            assert_eq!(res.bad.len(), 1, "越界/控制字符路径 {p:?} 应归入 bad: {res:?}");
            assert!(res.ok.is_empty(), "越界路径 {p:?} 不得判为通过: {res:?}");
            assert!(records.is_empty(), "越界路径 {p:?} 不得产生待写记录");
        }
        // 不需要手动删：`outside_dir` 的 `Drop` 会连目录一起收掉（失败路径上也是）。
    }

    /// 对应 Go `TestResultFailuresSelectsReenqueueSet`。
    ///
    /// 钉住"哪些类要重新入队"（规格 §4/§8）。
    ///
    /// `unverifiable` 不在其中：没有 crc64 可比，重下也不产生可比对象。
    /// `unreadable` 也不在：那是本地权限/IO 问题，重下解决不了（重下了还是读不了）。
    /// 这两类若被塞进重试集，只会白下一轮、并在第二轮里被重新归成同一类。
    #[test]
    fn result_failures_selects_reenqueue_set() {
        let r = CheckResult {
            ok: vec!["ok".into()],
            bad: vec!["b".into()],
            missing: vec!["m".into()],
            size_mismatch: vec!["s".into()],
            unverifiable: vec!["u".into()],
            unreadable: vec!["r".into()],
        };
        assert_eq!(
            r.failures(),
            vec!["b".to_string(), "m".to_string(), "s".to_string()],
            "failures() 应只取 bad+missing+size_mismatch，且按这个顺序"
        );
        // Go 版还有一条 `got == nil` 的断言（非 nil 的空切片）；Rust 的 `Vec` 没有 nil 态，
        // 因此这里只剩空集那一半。
        assert!(
            CheckResult {
                ok: vec!["a".into()],
                ..Default::default()
            }
            .failures()
            .is_empty(),
            "全通过时应为空集"
        );
    }

    /// 对应 Go `TestMergeRetryOverridesFailureClass`。
    ///
    /// 钉住重试合并的语义（规格 §8：不符 → 重新入队；再次不符则记入未完成清单）。
    ///
    /// 两个方向都要钉：
    ///   - 重下**成功**的必须从失败名单里消失并出现在 `ok` 里（否则报告永远少报一个可用
    ///     文件，客户会去重下已经好的数据）
    ///   - 重下**仍失败**的必须留在最终报告里（否则就是静默少交）
    #[test]
    fn merge_retry_overrides_failure_class() {
        let first = CheckResult {
            ok: vec!["a".into()],
            bad: vec!["b".into()],
            missing: vec!["m".into()],
            ..Default::default()
        };

        // 方向一：b 重下成功，m 仍缺失
        let got = merge(
            first.clone(),
            CheckResult {
                ok: vec!["b".into()],
                missing: vec!["m".into()],
                ..Default::default()
            },
        );
        assert_eq!(got.ok, vec!["a".to_string(), "b".to_string()], "重下成功的 b 应并入 ok");
        assert!(got.bad.is_empty(), "b 已重下成功，不该还留在 bad: {:?}", got.bad);
        assert_eq!(got.missing, vec!["m".to_string()], "m 仍缺失，必须留在 missing");

        // 方向二：两者都仍失败 —— 一条都不许丢
        let got2 = merge(
            first.clone(),
            CheckResult {
                bad: vec!["b".into()],
                missing: vec!["m".into()],
                ..Default::default()
            },
        );
        assert_eq!(got2.bad, vec!["b".to_string()], "重下仍失败的 b 必须留在报告里");
        assert_eq!(got2.missing, vec!["m".to_string()], "重下仍失败的 m 必须留在报告里");
        assert_eq!(got2.ok, vec!["a".to_string()], "第一轮通过的一项不该被动过");

        // 总量守恒：六类合起来必须仍等于两轮输入文件的并集（不得有文件消失）
        fn total(r: &CheckResult) -> usize {
            r.ok.len()
                + r.bad.len()
                + r.missing.len()
                + r.size_mismatch.len()
                + r.unverifiable.len()
                + r.unreadable.len()
        }
        for (i, g) in [&got, &got2].iter().enumerate() {
            assert_eq!(total(g), 3, "第 {} 个合并结果的六类总量应为 3: {g:?}", i + 1);
        }
    }

    /// 对应 Go `TestCheckPathThatIsDirectoryIsBad`（**简报清单里故意漏掉的那一条**）。
    ///
    /// 钉住：清单期望是文件、路径上却是目录，本地这份就不是我们要的东西，归入 `bad`。
    ///
    /// 注意这条测试的判别方向：目录的 size 在 macOS 上是 64，而清单声明 4096，
    /// 所以"删掉非普通文件分支"的实现会把它判成 `size_mismatch` 而不是 `bad`——
    /// 失败原因正是"没有把目录单独认出来"，不会与大小判据混淆。
    /// 反过来，若目录大小恰好相符（Linux 上是 4096），删掉该分支的实现会让读文件去读一个
    /// 目录（EISDIR），那就是 `unreadable` 而非 `bad`——两个方向都由下面的断言堵住。
    #[test]
    fn check_path_that_is_directory_is_bad() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.join("d")).expect("建目录失败");
        let (res, records) = check(dir.path(), &[file("d", 4096, README_CRC)]);
        assert_eq!(res.bad.len(), 1, "目录应归入 bad: {res:?}");
        assert!(
            res.ok.is_empty() && res.unreadable.is_empty(),
            "目录不得判为通过、也不该是读取失败: {res:?}"
        );
        assert!(records.is_empty(), "目录不得产生待写记录: {records:?}");
    }

    /// **修复轮 1 新增**（**不计入 16 条移植**，按全局约束 6 的例外显式记账）。
    ///
    /// 钉住"我们比 Go 更严"这个**有意偏离**：Go 的 `Check` 用 `os.Stat`（**跟随**符号链接），
    /// 本内核用 `symlink_metadata`（看链接本身），因此一个指向文件的符号链接在这里归 `bad`。
    /// Go 那边会跟随链接、把**目标**读出来算 CRC——一个指向 `/etc/passwd` 的链接在 Go 侧
    /// 会被当成客户的数据去校验。不跟随是更安全的一侧，所以保留这个偏离。
    ///
    /// 为什么这条测试必须存在：M8 变异实测（`symlink_metadata` → `metadata`）在补它之前
    /// **全套 83 条全绿**——即这条偏离当时没有任何测试守着。下一个人"顺手改回 `metadata`"
    /// 不会有任何提示，而**跨出目录读文件**这件事一旦回退是静默的。
    ///
    /// ⚠️ `#[cfg(unix)]`：Windows 上建符号链接需要额外权限（开发者模式/管理员），且
    /// "目录符号链接 / 文件符号链接"要靠额外参数区分、语义与 Unix 不同。本内核的目标是
    /// macOS（阶段 1），这条只在 Unix 上跑。（同一族还有下面的 `set_mode`，它用
    /// `std::os::unix::fs::PermissionsExt`，同样是 Unix 专有。）
    ///
    /// ⚠️ 建链接失败**不许静默跳过**：跳过会让这条保障重演它刚刚暴露的那个问题
    /// （"有偏离、没测试"），所以直接 `expect` 让它响亮地失败。
    #[cfg(unix)]
    #[test]
    fn check_symlink_is_bad_not_followed() {
        let dir = TempDir::new();
        write_file(dir.path(), "real.txt", b"readme");

        // 情形一：链接指向**目录内**的真实文件——最小可区分输入。
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link.txt"))
            .expect("建符号链接失败——这条测试不允许静默跳过（跳过就等于没有保障）");

        // 情形二：链接指到**目标目录之外**——正是要防的那个后果：
        // 不跟随链接，就不会去读客户目录之外的东西；跟随了就会把目录外的内容
        // 当成客户的数据校验通过。
        //
        // ⚠️ 同 `check_unsafe_path_is_bad`："目录外"由**另一个 `TempDir`** 表达，
        // 不用 `dir.path().parent()`（那是系统临时目录本身，会在仓库之外留下副作用、
        // 并发跑时还会互相删）。语义不变：目标仍在 `dir` 之外。
        let outside_dir = TempDir::new();
        let outside = outside_dir.join("symlink-outside-target.txt");
        std::fs::write(&outside, b"readme").expect("写目录外目标失败");
        std::os::unix::fs::symlink(&outside, dir.join("escape.txt"))
            .expect("建符号链接失败——这条测试不允许静默跳过");

        let (res, records) = check(
            dir.path(),
            &[
                file("link.txt", 6, README_CRC),
                file("escape.txt", 6, README_CRC),
            ],
        );

        assert_eq!(
            res.bad,
            vec!["link.txt".to_string(), "escape.txt".to_string()],
            "符号链接必须归 bad（看链接本身、不跟随）——这是比 Go 更严的那一侧: {res:?}"
        );
        assert!(
            res.ok.is_empty() && res.unreadable.is_empty() && res.size_mismatch.is_empty(),
            "符号链接不得落进其它类: {res:?}"
        );
        assert!(records.is_empty(), "符号链接不得产生待写记录: {records:?}");

        // 不需要手动删：`outside_dir` 的 `Drop` 会连目录一起收掉（失败路径上也是）。
    }

    /// 改权限。Unix 专有；本项目的生产与开发环境都是 Unix。
    fn set_mode(p: &StdPath, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).expect("chmod 失败");
    }
}
