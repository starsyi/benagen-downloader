//! planner —— 本地扫描：把清单里的文件分成「跳过 / 需下载」，并补齐缺失的 crc64。
//!
//! 逐条移植自 `downloader/internal/planner/{planner.go,planner_test.go}`（Go 侧已冻结）。
//! 11 条测试与 Go 一一对应，另有 1 条 `UreqTransport` 的真 HTTP 测试（简报步骤 6b，
//! **不属于 11 条移植**）。对应表见
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-6-report.md`。
//!
//! ⚠️ **跳过必须凭证据**：`Kind::Skip` 只在状态文件里存在一条「码已绑定、size 与 mtime
//! 都相符、且 crc64 能对上这一批数据」的记录时才成立。只凭「本地有同名文件」就跳过，
//! 会让一个其实没下完的文件被判定为不用下载——**静默少交**，本项目最忌讳的失败模式。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// mtime 的换算在整个内核里**只有 `engine::mtime_secs` 这一份**（任务 7 步骤 0 硬要求）：
// planner（读）与 verify（写）必须调同一个函数，否则状态条目永远匹配不上。
use crate::engine::mtime_secs;
use crate::delivery::{File, Manifest, Transport};
use crate::state::{Entry, State};

/// 分类结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 需要下载（含重下）
    Download,
    /// 已完整，跳过
    Skip,
}

/// 一个待处理条目
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub file: File,
    pub kind: Kind,
}

/// 规划结果
#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub items: Vec<Item>,
    /// 无法取得 crc64 的文件（HEAD 也失败），本轮仅比对大小。**它们仍然要下载。**
    pub unverifiable: Vec<String>,
}

/// 规划选项
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// 忽略状态文件，全部重新校验/下载。**规格许诺给客户的能力，不得删**（契约 §6）：
    /// 客户怀疑数据有问题时，用它把整批重新校验一遍。
    pub strict: bool,
}

/// 扫描与分类
pub struct Planner<'a, T: Transport> {
    pub dir: PathBuf,
    /// `&mut` 而非 `&`：`plan` 内部要调 `State::bind`——那是**纵深防御**，
    /// 使「换码即作废」不依赖调用方纪律（`planner.go:66-71`）。
    /// 漏调它会让上一批交付的记录被用来跳过这一批的文件，**静默少交**。
    /// `plan_binds_state_to_manifest_code` 守这条。
    pub state: Option<&'a mut State>,
    pub transport: &'a T,
}

impl<'a, T: Transport> Planner<'a, T> {
    /// 生成待办清单。
    ///
    /// 分类规则（`planner.go` 的判定链，**有顺序，不要重排**）：
    ///   1. 本地不存在 / 是目录 → 下载
    ///   2. 大小不符 → 下载（重下）
    ///   3. 严格模式或无状态文件 → 下载
    ///   4. 状态里没有该文件的条目 → 下载
    ///   5. 状态条目必须同时满足：size+mtime 相符、且 crc64 与清单对得上
    ///      （清单为空时要求记录里有上一次校验通过的非空值）→ 才可跳过
    ///   6. 清单 crc64 为空 → 先 HEAD 补齐；补不到则记入 `unverifiable` 并**仍然下载**
    ///
    /// ⚠️ 取 `&mut self` 是因为本函数要经 `self.state` 调 `bind`。
    /// ⚠️ `opt` 是**逐次**参数（对应 Go 的 `Plan(ctx, m, opt)`）：简报骨架里的签名漏写了它，
    /// 而 `Options` 若无处可传，契约 §6 许诺给客户的「严格校验」就没有入口。
    pub fn plan(&mut self, m: &Manifest, opt: Options) -> Todo {
        let mut todo = Todo {
            items: Vec::new(),
            unverifiable: Vec::new(),
        };

        // 纵深防御：`bind` 是幂等的，这里再调一次，使「换码即作废」不依赖调用方纪律。
        // 否则调用方一旦漏调 `bind`，就会拿上一批交付的记录去跳过这一批的文件——静默少交。
        if let Some(st) = self.state.as_deref_mut() {
            st.bind(&m.code);
        }

        for f in &m.files {
            // 补齐缺失的 crc64
            let mut f = f.clone();
            if f.crc64.is_empty() {
                let url = m.file_url(&f.path);
                match self.transport.head_crc64(&url) {
                    Some(crc) if !crc.is_empty() => f.crc64 = crc,
                    // 补不到：记入「本轮无法独立校验」，**但仍然下载**（由 classify 判定）
                    _ => todo.unverifiable.push(f.path.clone()),
                }
            }

            let kind = self.classify(&f, opt);
            todo.items.push(Item { file: f, kind });
        }
        todo
    }

    /// 单个文件的分类。见 `plan` 的判定链。
    fn classify(&self, f: &File, opt: Options) -> Kind {
        classify_with_state(&self.dir, self.state.as_deref(), f, opt)
    }
}

// ---------------------------------------------------------------------------
// 阶段 D 任务 A（内核）：本文件里**唯一**一处 Go 侧没有对应物的新增 —— 先读这段再读代码
// ---------------------------------------------------------------------------
//
// ⚠️ **本文件的性质**：`planner.rs` 整个是**逐条移植自 Go** 的
// （`downloader/internal/planner/{planner.go,planner_test.go}`，**Go 侧已冻结**，模块
// 文档写明"11 条测试与 Go 一一对应"）。改动这个文件里**属于移植的那部分**代码，
// 仍然要走移植自己的流程（对照 Go、保持两边同形）。
//
// ⚠️ **下面两个符号是 Rust 内核独有的，Go 侧没有对应物**，所以它们既不属于、也**不改变**
// 那 11 条对应关系：
//   - `classify_with_state`：只是把 `Planner::classify` **原本就有**的函数体**原样**搬出来
//     （零行为变化），好让下面那个函数共用同一份判定链；
//   - `recheck_complete`：内核内存里那份 `complete` 缓存的**廉价磁盘复核**。
//     Go 客户端没有这个缓存——它是本内核 `ensure_complete` 三段式的产物——
//     所以这条规则在 Go 侧**没有可分歧的对象**。
//
// ⚠️ **这一段不是"这个文件从此可以随便改"的口子**。它是一处**有理由、被控制者裁定保留**
// 的偏离（任务 A 简报把本文件标为"只读，不要改"，实现时改了，理由如下）。下一个要动
// 这个文件的人：**移植那部分照旧对照 Go；要往这里再加 Rust 独有的语义，先走一遍
// 与这次相同的偏离申报**——判据里最贵的错误就是"同一条规则有两份实现并漂移"。
//
// ⚠️ **为什么不把它放进 `main.rs`**（任务 A 的 D-1 要求判据**复用** `planner` 的判定链、
// 不许另写一套会漂移的规则）：放别处只有两条路——要么把那条判定链复制一份
// （那正是 D-1 要禁止的漂移源），要么拿不到 `classify` 需要的私有输入
// （`stat_secs` / `entry_proves_complete` / 状态条目）。放在这里，目的是让
// **"判据只有一份"成为结构保证**，而不是为了让 Rust 与 Go 分叉。
// ---------------------------------------------------------------------------

/// [`Planner::classify`] 的**唯一实现**（两处共用，见 [`recheck_complete`]）。
///
/// ⚠️ **不碰网络**：判定链里没有 HEAD / 没有 fetch。整个 `planner` 里唯一会发请求的是
/// `plan` 的 crc64 回补那一段，它**不在这里**——所以本函数被谁调都**结构上**不会
/// 触发那 N×30 秒的 I/O（任务 A 的 D-1）。
fn classify_with_state(dir: &Path, state: Option<&State>, f: &File, opt: Options) -> Kind {
    // 本地不存在 / 不是普通文件（含目录）→ 下载
    let Some((size, mtime)) = stat_secs(dir, &f.path) else {
        return Kind::Download;
    };
    if size != f.size {
        return Kind::Download; // 大小不符
    }

    // 大小已相符。严格模式下不看状态文件——状态文件只是「跳过」的加速器，
    // 任何不确定都应退化为下载 + 校验（规格 §7）。
    if opt.strict {
        return Kind::Download;
    }
    let Some(st) = state else {
        return Kind::Download; // 无状态文件
    };
    let Some(e) = st.get(&f.path) else {
        return Kind::Download; // 状态里没有该文件的条目
    };
    if entry_proves_complete(e, size, mtime, &f.crc64) {
        Kind::Skip
    } else {
        Kind::Download
    }
}

/// **只碰磁盘**的重新核对：把 `complete` 里"盘上**现在**已经不满足完整性"的路径摘掉。
///
/// 阶段 D 任务 A 引入。背景（诊断报告 `diagnosis-redownload.md` §1）：
/// `Kernel::complete` 只由 `complete_dirty` 一个布尔量控制失效，而把那个布尔量置 `true`
/// 的**唯一**地方是校验线程提交结果——**磁盘上的文件被外部删除，没有任何代码在看着**。
/// 于是这份知识会一直错下去，直接后果有两个（都不是边角）：
///   - 「全部下载」算出空 `pending`，被判成 `invalid_params: 没有匹配到任何文件`；
///   - `get_tree` 继续报 100% / 全部 `complete`（**静默少交**，比报错严重得多）。
///
/// ⚠️ **为什么不是"每次都重跑 `plan`"**（D-1）：`plan` 要遍历**整个清单**，且对每个
/// crc64 为空的文件发一次 HEAD（每次上限 30 秒）——那是按文件数 × 30 秒的网络 I/O。
/// 这里的代价是：**整份清单**建一次 `BTreeMap` 索引（O(n log n)，见下面那行注释），
/// 再对 `complete`（⊆ 清单）里每个路径做**一次 `stat`**。**一个请求都不发。**
/// 实测 10 000 个文件 ≈ 20–40 ms（release / debug，见任务 A 报告 §5）。
///
/// ⚠️ 判据**逐字复用** [`classify_with_state`]（同一套判定链）——**不要**在这里另写一遍
/// "文件在不在、大小对不对"的规则。
/// 特别地，它连 mtime 一起比：原地改写文件内容（诊断报告 §7 第 4 条）会掉 mtime，
/// 于是判 `Download`——被改坏的那份不会被继续标成 `complete`（那一版的界面假象
/// 没有报错可看，只能靠这一层拦）。
///
/// ⚠️ **但"与 `plan` 不可能漂移"是把话说满了**（最终审查顺手 10）：**判定链**同一份，
/// **输入**却不同 —— `plan` 传给 `classify` 的是**经 HEAD 补齐过 crc64 的 clone**
/// （上面 `plan` 里那三行），而本函数发不出任何请求（D-1：`recheck_complete` 的签名里
/// 没有 transport），拿到的是**清单原文**。于是**清单 crc64 为空、而 HEAD 又补到了值时，
/// 两者就会不一致**：
/// 本函数只能**凭状态记录**判定（`entry_proves_complete` 那条
/// "记录里有曾经校验通过的非空 crc64 即可跳过"的分支），而 `plan` 会拿 HEAD 补来的
/// crc64 去比 —— 即**本函数更宽松**：它摘掉的路径集合是 `plan` 的**子集**
/// （只会**少摘**、不会多摘）。代价是"清单 crc64 为空、且服务器上那个文件已被换过
/// （大小相同）"这一类要等到下一次 `complete_dirty` 触发的完整 `plan` 才会被摘掉 ——
/// 这是"本步骤一个网络请求都不发"换来的，如实记在这里；要更严就得在这里 HEAD，
/// 而那正是 D-1 禁止的。
///
/// ⚠️ 只会**摘掉**、不会补回来：盘上多出一个文件不会让 `complete` 变多。那是安全的
/// 方向——多算一个待下载只是白花一次带宽（`-c` 续传 + 校验兜底），少算一个才是少交。
pub fn recheck_complete(
    dir: &Path,
    state: Option<&State>,
    manifest: &Manifest,
    complete: &mut BTreeSet<String>,
) {
    // 先建索引再用：`complete` 里每个路径都去 `manifest.files` 线性找一遍是 O(文件数²)，
    // 上万文件的清单上那是几亿次比较（D-1 要求这一步是**廉价**的）。
    let by_path: BTreeMap<&str, &File> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    complete.retain(|p| match by_path.get(p.as_str()) {
        Some(f) => classify_with_state(dir, state, f, Options { strict: false }) == Kind::Skip,
        // 清单里没有的路径：`complete` 的推导式（`main.rs::complete_from`）只会放清单里的
        // 路径进来，所以这里理论上到不了。保守起见按"不完整"摘掉——留着它只可能让
        // 「全部下载」少下一个文件，那正是静默少交。
        None => false,
    });
}

/// 读一个文件的 (size, mtime)，与 Go 的 `os.Stat` + `fi.Size()`/`fi.ModTime()` 同形。
///
/// 拿不到元数据时返回 `None`（调用方据此判「下载」）。`modified()` 失败时同样返回 `None`
/// 而不是编一个 0：编出来的值有可能与某条记录撞上，那是**无证据的跳过**。
fn stat_secs(dir: &std::path::Path, rel: &str) -> Option<(i64, f64)> {
    let meta = std::fs::metadata(dir.join(rel)).ok()?;
    if meta.is_dir() {
        return None;
    }
    Some((meta.len() as i64, mtime_secs(meta.modified().ok()?)))
}

/// 供 `classify` 与测试共用的「状态条目能否证明这个文件已完整」判定。
///
/// 对应 `planner.go:104-118` 的那段：size+mtime 命中，且 crc64 能对上，
/// 才允许跳过；清单没带 crc64 时，记录里必须另有**上一次校验通过**的值。
fn entry_proves_complete(e: &Entry, size: i64, mtime: f64, manifest_crc64: &str) -> bool {
    if !e.matches(size, mtime) {
        return false;
    }
    if !manifest_crc64.is_empty() && e.crc64 != manifest_crc64 {
        return false;
    }
    // 清单没带 crc64（HEAD 也失败）时，跳过的前提是**记录里有曾经校验通过的值**。
    // D14 的整个依据就是「记录里的 crc64 是上次校验通过的证据」——所以这里必须真的检查它。
    if manifest_crc64.is_empty() && e.crc64.is_empty() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::Path;

    use crate::delivery::{http_status, is_retryable, UreqTransport};
    // HTTP 桩原先是本模块的一份私有副本（任务 6 写的），任务 9 提进了 `testutil`
    // 供 `engine::rpc` 共用（Ruling #56）。语义一字未改，只换了住处。
    use crate::testutil::{RawResponse, StubHttp, TempDir};

    /// 桩基址。与 `delivery.rs` 的测试同形：必须是合法基址，但**不会被真正连上**——
    /// `Stub` 不碰网络。
    const STUB_BASE: &str = "http://127.0.0.1:18081";

    /// 对应 Go 的 `writeFile`。
    fn write_file(dir: &Path, rel: &str, data: &[u8]) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录失败");
        }
        std::fs::write(&full, data).expect("写文件失败");
    }

    /// 对应 Go 的 `mtimeOf`：返回与实现同样的 mtime 刻度（秒 + 纳秒小数），
    /// 这样测试记录的状态条目与 `Planner` 读到的值逐位相等。
    ///
    /// ⚠️ 与 Go 的测试助手一样，这里与实现共用同一个换算函数，因此**测不出**
    /// 「换算公式换成了另一种等价的写法」——那条性质由实现里的 `mtime_secs` 注释说明，
    /// 是 planner（读）与 verify（写）之间的跨任务契约。
    fn mtime_of(dir: &Path, rel: &str) -> f64 {
        let meta = std::fs::metadata(dir.join(rel)).expect("stat 失败");
        mtime_secs(meta.modified().expect("mtime 不可用"))
    }

    /// 对应 Go 的 `manifest(base)`。`base` 必须指向桩服务——否则空 crc64 的条目
    /// 会触发 HEAD 补齐并打到真实网络。
    fn manifest(base: &str) -> Manifest {
        Manifest {
            code: "CODE".to_string(),
            base_url: base.to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 0,
            total_bytes: 0,
            files: vec![
                File {
                    path: "t/present.txt".to_string(),
                    size: 3,
                    crc64: "1".to_string(),
                    source_mtime: String::new(),
                },
                File {
                    path: "t/missing.txt".to_string(),
                    size: 2,
                    crc64: "2".to_string(),
                    source_mtime: String::new(),
                },
                File {
                    path: "t/wrongsize.txt".to_string(),
                    size: 99,
                    crc64: "3".to_string(),
                    source_mtime: String::new(),
                },
                File {
                    path: "t/nocrc.txt".to_string(),
                    size: 1,
                    crc64: String::new(),
                    source_mtime: String::new(),
                },
            ],
        }
    }

    /// 对应 Go 的 `&delivery.Manifest{Code: "CODE", BaseURL: "http://x", Files: [...]}`。
    fn one_file_manifest(code: &str, base: &str, path: &str, size: i64, crc64: &str) -> Manifest {
        Manifest {
            code: code.to_string(),
            base_url: base.to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 0,
            total_bytes: 0,
            files: vec![File {
                path: path.to_string(),
                size,
                crc64: crc64.to_string(),
                source_mtime: String::new(),
            }],
        }
    }

    /// 与 Go 的 `httptest.NewServer(...)` 同形的**传输桩**：只实现 HEAD 补齐这一面，
    /// 并把每次请求记下来（对应 Go 桩在 handler 里断言 `r.Method`）。
    struct Stub {
        crc: Option<&'static str>,
        head_calls: Cell<usize>,
        get_calls: Cell<usize>,
        head_urls: RefCell<Vec<String>>,
    }

    impl Stub {
        /// 桩服务回一个 crc64（Go 里 `w.Header().Set("X-Tos-Hash-Crc64ecma", ...)`）。
        fn head(crc: &'static str) -> Self {
            Self {
                crc: Some(crc),
                head_calls: Cell::new(0),
                get_calls: Cell::new(0),
                head_urls: RefCell::new(Vec::new()),
            }
        }

        /// 桩服务拿不到 crc64（Go 里 `w.WriteHeader(500)`）。
        fn head_fails() -> Self {
            Self {
                crc: None,
                head_calls: Cell::new(0),
                get_calls: Cell::new(0),
                head_urls: RefCell::new(Vec::new()),
            }
        }

        fn head_calls(&self) -> usize {
            self.head_calls.get()
        }

        fn head_urls(&self) -> Vec<String> {
            self.head_urls.borrow().clone()
        }

        fn get_calls(&self) -> usize {
            self.get_calls.get()
        }
    }

    impl Transport for Stub {
        fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
            self.get_calls.set(self.get_calls.get() + 1);
            Err("planner 不应发 GET".to_string())
        }

        fn head_crc64(&self, url: &str) -> Option<String> {
            self.head_calls.set(self.head_calls.get() + 1);
            self.head_urls.borrow_mut().push(url.to_string());
            self.crc.map(str::to_string)
        }
    }

    /// 对应 Go `TestClassify`。
    #[test]
    fn classify() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/present.txt", b"abc");
        write_file(dir.path(), "t/wrongsize.txt", b"short");
        write_file(dir.path(), "t/nocrc.txt", b"z");

        let mut st = State::load(dir.path());
        st.bind("CODE");
        // 记录一个 mtime 正确的条目，模拟「上次下完的」
        st.put(
            "t/present.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/present.txt"),
                crc64: "1".to_string(),
            },
        );

        // 桩服务：t/nocrc.txt 的 crc64 为空，plan 会来 HEAD 补齐
        let stub = Stub::head("999");
        let m = manifest(STUB_BASE);
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });

        // 先钉住条目总数：否则「少了一条」会被下面的查找掩盖——
        // 每一条清单文件都必须出现（含被跳过的）。
        assert_eq!(
            todo.items.len(),
            4,
            "应有 4 项（每一条清单文件都要出现，含被跳过的），得到 {:?}",
            todo.items
        );
        let mut got: std::collections::BTreeMap<&str, Kind> = std::collections::BTreeMap::new();
        for it in &todo.items {
            got.insert(it.file.path.as_str(), it.kind);
        }
        for (path, want) in [
            ("t/present.txt", Kind::Skip),   // 大小 + mtime + crc64 全部命中
            ("t/missing.txt", Kind::Download), // 本地不存在
            ("t/wrongsize.txt", Kind::Download), // 大小不符
            ("t/nocrc.txt", Kind::Download), // 无 crc64 也必须下载（不得因无法校验就跳过）
        ] {
            match got.get(path) {
                None => panic!("{path} 没出现在待办清单里"),
                Some(k) if *k != want => panic!("{path} 分类错：得到 {k:?}，应为 {want:?}"),
                Some(_) => {}
            }
        }
        // 桩服务给得出 crc64，就不该有任何条目落进「无法校验」报告里。
        assert!(
            todo.unverifiable.is_empty(),
            "HEAD 成功时不应有 unverifiable，得到 {:?}",
            todo.unverifiable
        );
        // 补齐只发生在清单里 crc64 为空的条目上，且请求的是**该文件的**下载 URL。
        //
        // ⚠️ 期望值写成**字面量**，不得用 `m.file_url(...)`（那是被测代码）来给自己当预期：
        // 那样写的话，`file_url` 一旦被改坏（例如丢掉随机码前缀），期望值与实际值**一起变**，
        // 断言照样绿——与 `mtime_of` 复用实现函数是同一类盲区（见任务 6 报告 M7）。
        // 这里逐段写出：基址 `STUB_BASE` + 交付码 `CODE` + 清单路径 `t/nocrc.txt`。
        assert_eq!(stub.head_calls(), 1, "只应为 t/nocrc.txt 发一次 HEAD");
        assert_eq!(
            stub.head_urls(),
            vec!["http://127.0.0.1:18081/CODE/t/nocrc.txt".to_string()],
            "HEAD 请求的必须是 manifest.path 对应的下载 URL"
        );
    }

    /// 对应 Go `TestStaleStateEntryDoesNotSkip`。
    ///
    /// 状态文件只是跳过决策的**加速器**，不是真相。它记的 mtime 与磁盘现状不符时，
    /// 说明本地那份可能已被改动，必须退化为重新下载 + 校验。
    #[test]
    fn stale_state_entry_does_not_skip() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/present.txt", b"abc");

        let mut st = State::load(dir.path());
        st.bind("CODE");
        // 大小相同，但 mtime 记的是旧的（文件在记录之后被改动过）
        st.put(
            "t/present.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/present.txt") - 1.0,
                crc64: "1".to_string(),
            },
        );

        let m = one_file_manifest("CODE", "http://x", "t/present.txt", 3, "1");
        let stub = Stub::head_fails();
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });
        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Download,
            "状态条目的 mtime 不符时应下载，得到 {:?}",
            todo.items
        );
    }

    /// 对应 Go `TestStateCRCMismatchDoesNotSkip`。
    ///
    /// 大小与 mtime 都对上，但状态文件里记的 crc64 与清单给的不是同一批数据——
    /// 这时跳过就是静默少交：本地那份不是客户要的。
    #[test]
    fn state_crc_mismatch_does_not_skip() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/present.txt", b"abc");

        let mut st = State::load(dir.path());
        st.bind("CODE");
        st.put(
            "t/present.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/present.txt"),
                crc64: "OLD".to_string(),
            },
        );

        let m = one_file_manifest("CODE", "http://x", "t/present.txt", 3, "NEW");
        let stub = Stub::head_fails();
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });
        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Download,
            "状态里的 crc64 与清单不符时应下载，得到 {:?}",
            todo.items
        );
    }

    /// 对应 Go `TestNilStateDownloads`。
    ///
    /// 没有状态文件（首次运行）时必须判为下载——不能因为「无从判断」就跳过，
    /// 也不能因为 State 为 None 而崩溃。
    #[test]
    fn nil_state_downloads() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/present.txt", b"abc");
        let m = one_file_manifest("CODE", "http://x", "t/present.txt", 3, "1");

        let stub = Stub::head_fails();
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: None, // 无 State
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });
        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Download,
            "无状态文件时应下载，得到 {:?}",
            todo.items
        );
    }

    /// 对应 Go `TestStrictModeIgnoresState`。
    #[test]
    fn strict_mode_ignores_state() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/present.txt", b"abc");
        let mut st = State::load(dir.path());
        st.bind("CODE");
        st.put(
            "t/present.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/present.txt"),
                crc64: "1".to_string(),
            },
        );

        let m = one_file_manifest("CODE", "http://x", "t/present.txt", 3, "1");
        let stub = Stub::head_fails();
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: true });
        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Download,
            "严格模式下应重新下载，得到 {:?}",
            todo.items
        );
    }

    /// 对应 Go `TestMissingCRCFilledByHEAD`。
    #[test]
    fn missing_crc_filled_by_head() {
        // 服务端回读失败时清单里 crc64 为空；客户端应自己 HEAD 补齐
        let stub = Stub::head("5432380796884633278");
        let dir = TempDir::new();
        let m = one_file_manifest("C", STUB_BASE, "t/a.txt", 6, "");

        let mut st = State::load(dir.path());
        st.bind("C");
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });

        assert_eq!(todo.items.len(), 1, "应有一项待下");
        assert_eq!(
            todo.items[0].file.crc64, "5432380796884633278",
            "HEAD 未补齐 crc64: {:?}",
            todo.items[0].file.crc64
        );
        assert!(
            todo.unverifiable.is_empty(),
            "HEAD 成功，不该记入 unverifiable: {:?}",
            todo.unverifiable
        );
        // 补齐走的是 HEAD，不是 GET（Go 侧桩里断言 `r.Method != http.MethodHead` 即报错）
        //
        // ⚠️ 期望值同样是**字面量**（理由见 `classify` 里的同一处注释）：
        // 基址 `STUB_BASE` + 交付码 `C` + 清单路径 `t/a.txt`。
        assert_eq!(stub.head_calls(), 1, "应发一次 HEAD");
        assert_eq!(
            stub.head_urls(),
            vec!["http://127.0.0.1:18081/C/t/a.txt".to_string()],
            "HEAD 的 URL 不对"
        );
        assert_eq!(stub.get_calls(), 0, "补齐 crc64 只应发 HEAD，不得发 GET");
    }

    /// 对应 Go `TestEmptyCRCAfterHEADFailureStillDownloads`。
    ///
    /// 本地已有一份大小相符的文件，但清单没给 crc64、HEAD 也拿不到：
    /// 没有任何东西能证明本地这份是对的，所以**必须下载**——
    /// 不能因为「无法校验」就走跳过。
    #[test]
    fn empty_crc_after_head_failure_still_downloads() {
        let stub = Stub::head_fails();
        let dir = TempDir::new();
        write_file(dir.path(), "t/a.txt", b"abcdef");
        let mut st = State::load(dir.path());
        st.bind("C"); // 没有任何记录

        let m = one_file_manifest("C", STUB_BASE, "t/a.txt", 6, "");
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });

        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Download,
            "拿不到 crc64 且无状态记录时必须下载，得到 {:?}",
            todo.items
        );
        assert_eq!(
            todo.unverifiable,
            vec!["t/a.txt".to_string()],
            "拿不到 crc64 必须在报告里标注"
        );
    }

    /// 对应 Go `TestStateHitWithEmptyManifestCRCIsReported`。
    ///
    /// 清单没给 crc64、HEAD 也失败，但状态文件里有一条 size+mtime 完全命中的记录。
    ///
    /// 这条记录里的 crc64 是**上一次校验通过时**写下的（verify 只在校验通过时才记状态，
    /// 且只记非空 crc64），换句话说本地这份内容已经被那个 crc64 认过一次，
    /// 且此后大小与 mtime 都没变——跳过是安全的，也正是状态文件存在的意义。
    ///
    /// 但它这一轮无法被独立校验，所以必须出现在 `unverifiable` 里：跳过可以，
    /// 静默不行。这条测试同时钉住这两半，改坏任一半都会红。
    #[test]
    fn state_hit_with_empty_manifest_crc_is_reported() {
        let stub = Stub::head_fails();
        let dir = TempDir::new();
        write_file(dir.path(), "t/a.txt", b"abcdef");
        let mut st = State::load(dir.path());
        st.bind("C");
        st.put(
            "t/a.txt",
            Entry {
                size: 6,
                mtime: mtime_of(dir.path(), "t/a.txt"),
                crc64: "5432380796884633278".to_string(),
            },
        );

        let m = one_file_manifest("C", STUB_BASE, "t/a.txt", 6, "");
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });

        assert!(
            todo.items.len() == 1 && todo.items[0].kind == Kind::Skip,
            "状态记录命中（含已校验过的 crc64）时应跳过，得到 {:?}",
            todo.items
        );
        assert_eq!(
            todo.unverifiable,
            vec!["t/a.txt".to_string()],
            "本轮无法独立校验的文件必须标注出来"
        );
    }

    /// 对应 Go `TestSkipRequiresEvidenceInState`。
    ///
    /// D14 允许「清单 crc64 为空 + HEAD 失败 + 状态命中 → 跳过」，
    /// 其依据是「记录里的 crc64 是上次校验通过的证据」。
    /// 所以记录自己必须带非空 crc64——否则那是无证据的跳过。
    #[test]
    fn skip_requires_evidence_in_state() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/a.txt", b"abc");

        let mut st = State::load(dir.path());
        st.bind("CODE");
        // 记录命中 size+mtime，但自己没带 crc64（被篡改 / 异版本写入 / JSON 缺字段）
        st.put(
            "t/a.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/a.txt"),
                crc64: String::new(),
            },
        );

        let stub = Stub::head_fails();
        let m = one_file_manifest("CODE", STUB_BASE, "t/a.txt", 3, "");
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });
        assert_eq!(todo.items.len(), 1, "应有一项，得到 {:?}", todo.items);
        assert_eq!(
            todo.items[0].kind,
            Kind::Download,
            "记录里没有 crc64 时不得跳过——那是无证据的跳过"
        );
        assert_eq!(todo.unverifiable.len(), 1, "仍须标注为无法校验");
    }

    /// 对应 Go `TestPlanBindsStateToManifestCode`。
    ///
    /// `plan` 应自己把状态绑定到清单的码：调用方漏调 `bind` 时，
    /// 不能拿上一批交付的记录去跳过这一批的文件。
    #[test]
    fn plan_binds_state_to_manifest_code() {
        let dir = TempDir::new();
        write_file(dir.path(), "t/a.txt", b"abc");

        let mut st = State::load(dir.path());
        st.bind("OLD_CODE"); // 绑的是上一批
        st.put(
            "t/a.txt",
            Entry {
                size: 3,
                mtime: mtime_of(dir.path(), "t/a.txt"),
                crc64: "999".to_string(),
            },
        );

        let m = one_file_manifest("NEW_CODE", "http://x", "t/a.txt", 3, "999");
        let stub = Stub::head_fails();
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });
        assert_eq!(todo.items.len(), 1, "应有一项，得到 {:?}", todo.items);
        assert_eq!(
            todo.items[0].kind,
            Kind::Download,
            "换码后上一批的记录必须作废——不得跳过"
        );

        // 直接钉住「plan 自己调了 bind」：上面的分类断言是间接证据，
        // 这一条让「漏调 bind」在编译期借用的那层（`&mut State`）之外也留下可读的证据。
        drop(p); // 结束对 st 的可变借用
        assert_eq!(st.code, "NEW_CODE", "plan 应把状态绑定到清单的码");
        assert!(
            st.get("t/a.txt").is_none(),
            "换码后上一批的条目必须被清掉"
        );
    }

    /// 对应 Go `TestHEADFailureDegradesWithWarning`。
    #[test]
    fn head_failure_degrades_with_warning() {
        let stub = Stub::head_fails();
        let dir = TempDir::new();
        let m = one_file_manifest("C", STUB_BASE, "t/a.txt", 6, "");
        let mut st = State::load(dir.path());
        st.bind("C");
        let mut p = Planner {
            dir: dir.path().to_path_buf(),
            state: Some(&mut st),
            transport: &stub,
        };
        let todo = p.plan(&m, Options { strict: false });

        assert_eq!(
            todo.unverifiable,
            vec!["t/a.txt".to_string()],
            "HEAD 失败时应记入 unverifiable"
        );
        assert_eq!(todo.items.len(), 1, "仍应下载该文件");
        // 「仍应下载」必须真的是下载：降级为仅比对大小 ≠ 可以跳过。
        assert_eq!(
            todo.items[0].kind,
            Kind::Download,
            "HEAD 失败时仍应下载，得到 {:?}",
            todo.items[0].kind
        );
    }

    // ------------------------------------------------------------------------
    // 步骤 6b：`UreqTransport` 的真 HTTP 测试（**不属于 11 条移植**，按全局约束 6
    // 的例外显式记账）。它落在 `planner.rs` 而不是 `delivery.rs`，因为交付约束里
    // `delivery.rs` **只许**改步骤 6c 那一处。
    // ------------------------------------------------------------------------

    /// `UreqTransport` 的真 HTTP 测试。
    ///
    /// 三处行为逐条断言，**每一处都有能打红它的变异体**（见任务 6 报告的变异检查表）：
    ///   1. `head_crc64` 读的是 `X-Tos-Hash-Crc64ecma`，且头名大小写不敏感
    ///   2. 拿不到头 / 请求失败时返回 `None`（不是 panic、不是空串）
    ///   3. `get` 对真 4xx 的错误能带出状态码、且**不可重试**（5xx 作正对照）
    #[test]
    fn ureq_transport_hits_real_http() {
        let t = UreqTransport::new();

        // ---- 1. 头名（含大小写不敏感）-------------------------------------------------
        // 这是「清单 crc64 为空时靠 HEAD 补齐」整条路径的地基。
        // 桩故意用**全小写**的头名发：HTTP 头名本就大小写不敏感，Go 的 `resp.Header.Get`
        // 与 ureq 的查询都不区分大小写。
        // 变异体：把头名改一个字母（例如去掉结尾的 `ecma`）→ 本段必须红。
        let srv = StubHttp::start(vec![
            RawResponse::new(200).header("x-tos-hash-crc64ecma", "5432380796884633278"),
        ]);
        let url = format!("{}/AbCdEfGhIjKlMnOpQrSt/t/a.txt", srv.base());
        assert_eq!(
            t.head_crc64(&url),
            Some("5432380796884633278".to_string()),
            "head_crc64 没读到 X-Tos-Hash-Crc64ecma（头名写错了？）"
        );
        assert_eq!(
            srv.seen(),
            vec![(
                "HEAD".to_string(),
                "/AbCdEfGhIjKlMnOpQrSt/t/a.txt".to_string()
            )],
            "head_crc64 必须发 HEAD，且打的是清单文件的 URL"
        );

        // ---- 2. 头缺失 / 请求失败 → None ---------------------------------------------
        // 调用方据此记 `unverifiable` 并**仍然下载**：返回空串会让它误以为补齐成功。
        let srv_no_header = StubHttp::start(vec![RawResponse::new(200)]);
        assert_eq!(
            t.head_crc64(&format!("{}/AbCdEfGhIjKlMnOpQrSt/t/a.txt", srv_no_header.base())),
            None,
            "响应里没有该头时必须是 None（返回空串会被当成补齐成功）"
        );
        let srv_500 = StubHttp::start(vec![RawResponse::new(500).header("X-Tos-Hash-Crc64ecma", "1")]);
        assert_eq!(
            t.head_crc64(&format!("{}/AbCdEfGhIjKlMnOpQrSt/t/a.txt", srv_500.base())),
            None,
            "非 200（Go 侧 `resp.StatusCode != http.StatusOK`）也必须是 None"
        );
        assert_eq!(
            t.head_crc64("http://127.0.0.1:1/AbCdEfGhIjKlMnOpQrSt/t/a.txt"),
            None,
            "连不上时必须是 None，不得 panic"
        );

        // ---- 3. 真 4xx：错误串带得出状态码、且不可重试 --------------------------------
        // ⚠️ 这一段**必须**走真实的 `UreqTransport` 打真实的 4xx 响应：
        // `http_status_error`（生产端）与 `http_status`（解析端）共用同一个 helper，
        // 测试桩也用它造错误串——于是「把 "HTTP " 前缀两边同时改掉」这类协调性改动
        // 会让所有桩测试全绿，而真实的 4xx 分类已经坏了（任务 2 审查 D21）。
        for status in [403u16, 404, 410] {
            let srv = StubHttp::start(vec![RawResponse::new(status)]);
            let err = t
                .get(&format!(
                    "{}/AbCdEfGhIjKlMnOpQrSt/manifest.json",
                    srv.base()
                ))
                .expect_err("4xx 必须报错");
            assert_eq!(
                http_status(&err),
                Some(status),
                "真实的 {status} 响应，错误串里必须带得出状态码: {err:?}"
            );
            assert!(
                !is_retryable(&err),
                "4xx 是确定性失败，不得被判为可重试: {err:?}"
            );
            assert!(
                err.starts_with(&format!("HTTP {status}")),
                "`Transport` 的错误串契约（delivery.rs 的 trait 文档）：确定性失败必须以 \
                 `HTTP <状态码>` 开头，否则 `fetch` 会去重试它: {err:?}"
            );
        }
        // 404 的专用文案（任务 2 裁定 C 点 1）：交付码打错是最常见的失败，
        // 值得一句能照做的提示。这段文案在别处没有任何守护。
        let srv_404 = StubHttp::start(vec![RawResponse::new(404)]);
        let err_404 = t
            .get(&format!(
                "{}/AbCdEfGhIjKlMnOpQrSt/manifest.json",
                srv_404.base()
            ))
            .expect_err("404 必须报错");
        assert!(
            err_404.contains("清单不存在"),
            "404 应当给一句能照做的提示: {err_404:?}"
        );

        // 正对照：5xx 必须**可重试**。没有这一条，「4xx 不可重试」的断言分不清
        // 「4xx 判对了」与「一切都不可重试」。
        let srv_5xx = StubHttp::start(vec![RawResponse::new(500)]);
        let err_500 = t
            .get(&format!(
                "{}/AbCdEfGhIjKlMnOpQrSt/manifest.json",
                srv_5xx.base()
            ))
            .expect_err("500 必须报错");
        assert_eq!(
            http_status(&err_500),
            Some(500),
            "5xx 的状态码也要带得出来: {err_500:?}"
        );
        assert!(
            is_retryable(&err_500),
            "5xx 是暂时性失败，必须可重试: {err_500:?}"
        );
    }

    /// **阶段 D 任务 A**：`recheck_complete` 只碰磁盘，且判据与 `classify` 是**同一套**。
    ///
    /// 四条路径各守一面（诊断报告 §7 的第 1 / 第 4 / 第 6 条，以及"无证据不得跳过"）：
    ///   - 文件**完好** → 留着（把完好的误判成待下载只是白花带宽，但没必要）；
    ///   - 文件**被删** → 摘掉（§7 第 1 条，人类伙伴实测的那条症状）；
    ///   - 文件**原地改写**（**同大小**、mtime 变了）→ 摘掉（§7 第 4 条：这一条界面上
    ///     连报错都没有，只能靠 mtime 拦）；
    ///   - 状态里**没有条目** → 摘掉（"无证据不得跳过"）。
    ///
    /// ⚠️ 这个函数**签名里就没有 transport**——"只 stat 磁盘、一个请求都不发"因此是
    /// **结构性**的（D-1），不是靠调用方纪律。
    #[test]
    fn recheck_complete_drops_paths_the_disk_no_longer_backs() {
        let dir = TempDir::new();
        let data = b"0123456789";
        for p in ["t/ok.txt", "t/gone.txt", "t/edited.txt", "t/no-entry.txt"] {
            write_file(dir.path(), p, data);
        }

        let f = |p: &str| File {
            path: p.to_string(),
            size: data.len() as i64,
            crc64: "42".to_string(),
            source_mtime: String::new(),
        };
        let m = Manifest {
            code: "CODE".to_string(),
            base_url: STUB_BASE.to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 4,
            total_bytes: 0,
            files: vec![
                f("t/ok.txt"),
                f("t/gone.txt"),
                f("t/edited.txt"),
                f("t/no-entry.txt"),
            ],
        };

        // 状态记录：三个文件各一条（`no-entry.txt` 故意没有）
        let mut st = State::load(dir.path());
        st.bind("CODE");
        for p in ["t/ok.txt", "t/gone.txt", "t/edited.txt"] {
            st.put(
                p,
                Entry {
                    size: data.len() as i64,
                    mtime: mtime_of(dir.path(), p),
                    crc64: "42".to_string(),
                },
            );
        }

        // 磁盘被内核之外的力量改动：删一个、原地改写一个（**同大小**）。
        //
        // ⚠️ mtime 是**显式设死**的，不靠"改写完之后时间戳自然会变"：后者在时间戳粒度
        // 1 秒的文件系统上会**假绿**（改写与记录落在同一秒 ⇒ mtime 没变 ⇒ 这一条悄悄
        // 什么都没测到），而在别的实现下又可能假红。设成 2020 年，与记录里那个"刚刚"
        // 的值必然不同 —— 这一条要测的是**判据认 mtime**，不是文件系统的时间精度。
        std::fs::remove_file(dir.path().join("t/gone.txt")).expect("删文件失败");
        let edited = dir.path().join("t/edited.txt");
        std::fs::write(&edited, data).expect("原地改写失败");
        let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(&edited)
            .expect("打开待改 mtime 的文件失败")
            .set_modified(long_ago)
            .expect("显式设置 mtime 失败");
        assert_eq!(
            std::fs::metadata(&edited).expect("stat 失败").len(),
            data.len() as u64,
            "这一条要的是**同大小**改写（大小不符会走另一条分支，测不到 mtime）"
        );

        let mut complete: BTreeSet<String> = [
            "t/ok.txt",
            "t/gone.txt",
            "t/edited.txt",
            "t/no-entry.txt",
            // 清单里没有的路径：`complete_from` 理论上产不出来，走的是防御性分支
            "t/not-in-manifest.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        recheck_complete(dir.path(), Some(&st), &m, &mut complete);
        assert_eq!(
            complete.iter().cloned().collect::<Vec<_>>(),
            vec!["t/ok.txt".to_string()],
            "只有「盘上确实完好、且有状态记录作证」的那一个才该留下: {complete:?}"
        );
    }
}
