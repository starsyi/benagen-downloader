//! view —— 界面的**数据模型**：把「清单 + 本地扫描 + RPC 状态」三路数据
//! 合成每个节点的四态，并算出进度。
//!
//! 逐条移植自 `downloader/internal/view/{model.go,model_test.go}`（Go 侧已冻结）。
//! 29 条测试与 Go 一一对应，另加 3 条由变异检查补上（见下文与
//! `.superpowers/sdd/2026-09-16-rust-core-phase-a/task-8-report.md`：
//! 第 30 条补契约 §1.5 的「失败 > 完成」，第 31 条补 §1.2 排名表的末档「移除 0」，
//! 第 32 条补 `progress` 侧同一档的「移除不是活跃任务」）。
//!
//! ⚠️ **全是纯函数**——没有界面概念（约束 2：这里是数据状态，不是界面状态）。
//! 界面/协议层因此可以保持"只接线"，而这些语义被完整单测覆盖。
//!
//! ⚠️ **这一节是全项目风险最集中的地方**（契约 §1）：Go 版在这里栽过三次。
//! 移植时四处**不得凭直觉写**，必须照契约：
//!   1. 活跃任务（active/waiting）优先于本地扫描；已结束的（complete/error/removed）不优先（§1.1）
//!   2. 同路径多任务**先按 `task_rank` 挑出唯一胜出者，再应用一次 switch**（§1.2）——
//!      `Engine::list` 的拼接顺序（active→waiting→stopped）会让 last-write-wins
//!      **确定性地**选到有害的一侧（不是偶发，是必然，且触发路径正是"失败 → 重下"的常规重试流）
//!   3. 已完成量**按文件二选一**（§1.3）——取和会得到 `9 > 6` 的重复计数，客户看到进度条卡在 100%
//!   4. 同路径多任务取 `max` 不取和（§1.4）——取和会把两个各 3/6 的任务报成 100% 假完成

use std::collections::{BTreeMap, BTreeSet};

use crate::delivery::File;
use crate::engine::status::{Task, TaskState};

/// 区分目录与文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Dir,
    File,
}

/// 树上的一个节点。
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub node_type: NodeType,
    pub name: String,
    /// 仅文件有：manifest 原文路径（约束 3：原样使用，不做任何规范化）
    pub path: String,
    pub size: i64,
    pub crc64: String,
    /// 同 `delivery::File::source_mtime`，逐字带过来。
    pub source_mtime: String,
    /// 仅目录有
    pub children: BTreeMap<String, Node>,
}

/// 每个文件的四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 待下载
    Pending,
    /// 下载中
    Downloading,
    /// 已完整
    Complete,
    /// 失败
    Failed,
}

/// 合成后的每文件状态。
#[derive(Debug, Clone, PartialEq)]
pub struct FileState {
    pub state: State,
    /// 下载中时的已完成字节
    pub completed: i64,
    /// 下载中时的总字节
    pub total: i64,
    /// 下载中时的实时速度
    pub speed: i64,
    /// 失败原因
    pub err: String,
}

impl Node {
    /// 目录节点：只有 `name` 与空的 `children`，其余字段是文件专有的（零值）。
    fn dir(name: &str) -> Node {
        Node {
            node_type: NodeType::Dir,
            name: name.to_string(),
            path: String::new(),
            size: 0,
            crc64: String::new(),
            // 时间只属于文件，目录没有——空串与 `path`/`crc64` 同一种做法
            source_mtime: String::new(),
            children: BTreeMap::new(),
        }
    }

    /// 文件叶节点：`path` 是 manifest **原文**（约束 3），`name` 是它的最后一段。
    fn leaf(f: &File, name: &str) -> Node {
        Node {
            node_type: NodeType::File,
            name: name.to_string(),
            path: f.path.clone(),
            size: f.size,
            crc64: f.crc64.clone(),
            // 逐字搬运，不解析不校验（见 `delivery::File::source_mtime`）
            source_mtime: f.source_mtime.clone(),
            children: BTreeMap::new(),
        }
    }
}

/// 把清单里的扁平路径列表还原成嵌套树。
///
/// 路径**原样**使用（含 `×`、空格、中文）——不转码、不重命名。
pub fn build_tree(files: &[File]) -> BTreeMap<String, Node> {
    let mut root: BTreeMap<String, Node> = BTreeMap::new();
    for f in files {
        // Go 的 `strings.Split(f.Path, "/")`：**空串也切成一个空段**，
        // 所以 `""` 会在根下留下一个名为 `""` 的文件节点。Rust 的 `split('/')` 同语义。
        let parts: Vec<&str> = f.path.split('/').collect();
        insert_path(&mut root, &parts, f);
    }
    root
}

/// 把一个文件按路径逐段插入某一层，对应 Go `BuildTree` 内层的那个 `for i, part := range parts`。
///
/// ⚠️ 中途的段若已存在但**不是目录**（清单里同时有 `a` 与 `a/b` 这种自相矛盾的形态），
/// Go 是**整个替换**掉那个节点（`if !ok || n.Type != NodeDir`）——文件节点的
/// size/crc64 就此丢失。这里逐字保留该行为：清单是服务端来的，自相矛盾的输入
/// 该被原样照做，而不是在客户端"修好"它。
fn insert_path(level: &mut BTreeMap<String, Node>, parts: &[&str], f: &File) {
    if parts.is_empty() {
        return; // `split` 至少给一段，这里只是防御
    }
    let part = parts[0];
    if parts.len() == 1 {
        level.insert(part.to_string(), Node::leaf(f, part));
        return;
    }
    let entry = level
        .entry(part.to_string())
        .or_insert_with(|| Node::dir(part));
    if entry.node_type != NodeType::Dir {
        *entry = Node::dir(part);
    }
    insert_path(&mut entry.children, &parts[1..], f);
}

/// 四态合成。见契约 §1.1/§1.2 —— **顺序无关**是硬要求。
///
/// 状态合成的规则——两句话，落点在「哪一路数据更新」：
///
///  1. **活跃任务（active / waiting）优先于本地扫描结果。** 本地扫描是加载时的一张快照，
///     任务是活的。客户双击一个已完整的文件要求重下时，界面必须给出进度，
///     否则点了没反应（用户反馈③「没有下载进度」）。
///  2. **已结束的任务（complete / error / removed）不得覆盖「本地已校验完整」。**
///     残留的旧失败任务把磁盘上完好的文件谎报成「失败」，与谎报成功同样有害；
///     残留的旧 complete 任务也不该把已校验完整的文件一直挂在「下载中 100%」。
///  3. 其余：本地已完整 → complete；有失败任务 → failed；否则 pending。
///
/// **同一路径有多个任务时，任务之间也按这个优先级排名，而不是按切片顺序覆盖**：
/// 客户重试后，旧的已结束任务仍留在 `gid_to_path` 里，而 aria2 的列表方法
/// （`Engine::list`：tellActive → tellWaiting → tellStopped）把活跃任务排在前面、
/// 已结束的排在后面。若按顺序覆盖，一个陈旧的失败任务会把**正在下载**的文件改判成
/// 「失败」——不是偶发，是必然。胜出任务的挑法见 [`task_rank`]。
///
/// 已移除的任务（removed）不进任何 case，保持基底状态不变——它已经没有下文的了。
///
/// `gid_to_path` 由 engine 的 daemon 维护——aria2 只回 GID，**路径映射必须我们自己记**，
/// 不能从 aria2 的 `files[].path` 反推（那是落盘绝对路径，还带着下载根）。
/// 映射的值就是 manifest 相对路径。
pub fn compose(
    files: &[File],
    complete: &BTreeSet<String>,
    gid_to_path: &BTreeMap<String, String>,
    tasks: &[Task],
) -> BTreeMap<String, FileState> {
    let mut out: BTreeMap<String, FileState> = BTreeMap::new();

    // 先按清单铺开，再让任务覆盖——避免"有任务但清单里没有"凭空造出节点
    for f in files {
        let state = if complete.contains(&f.path) {
            State::Complete
        } else {
            State::Pending
        };
        out.insert(
            f.path.clone(),
            FileState {
                state,
                completed: 0,
                total: f.size,
                speed: 0,
                err: String::new(),
            },
        );
    }

    // 同一路径可能有多个任务（客户重试后，旧的已结束任务仍留在映射里），
    // 而 aria2 的列表方法把活跃任务排在前面、已结束的排在后面。
    // 若按切片顺序覆盖，一个陈旧的失败任务会把正在下载的文件改判成「失败」。
    // 所以先按**语义**而非顺序挑出每个路径的胜出任务——这样结果与切片顺序无关。
    let mut best: BTreeMap<&str, &Task> = BTreeMap::new();
    for t in tasks {
        let Some(path) = gid_to_path.get(&t.gid).map(|p| p.as_str()) else {
            continue; // 映射里没有的 GID 不是我们的任务（或已清理）
        };
        if !out.contains_key(path) {
            continue; // 清单里没有的路径不得造节点
        }
        match best.get(path) {
            // 严格大于才替换：同 rank 时保留**先遇到的**（如两条失败，报哪条原因都可以）。
            // 注意这与"按顺序覆盖"是两回事——覆盖是**无条件**的，这里是排名相等才不动。
            Some(cur) if task_rank(cur.state) >= task_rank(t.state) => {}
            _ => {
                best.insert(path, t);
            }
        }
    }

    // 每个路径只在这里被应用一次、且只写 `out[path]`，路径之间互不影响，
    // 所以结果与 `best` 的迭代顺序无关。**别以为这里漏了排序。**
    for (path, t) in &best {
        let st = &out[*path];
        match t.state {
            // 活任务优先：客户刚点了重下，界面必须给出进度
            TaskState::Active | TaskState::Waiting => {
                out.insert(
                    (*path).to_string(),
                    FileState {
                        state: State::Downloading,
                        completed: t.completed,
                        total: t.total,
                        speed: t.speed,
                        err: String::new(),
                    },
                );
            }
            // 已结束的任务不得覆盖"磁盘上已校验完整"这个更强的事实
            TaskState::Error => {
                if st.state != State::Complete {
                    // ⚠️ 逐字对应 Go 的 `FileState{State: StateFailed, Err: t.Err}`：
                    // `Completed`/`Total`/`Speed` 取零值，**`Total` 也是 0**（不是清单里的
                    // `f.size`）——Go 的复合字面量没写这两个字段。别"顺手补上"。
                    out.insert(
                        (*path).to_string(),
                        FileState {
                            state: State::Failed,
                            completed: 0,
                            total: 0,
                            speed: 0,
                            err: t.err.clone(),
                        },
                    );
                }
            }
            // 任务传完但校验还没跑：仍标 downloading，等校验结果来定
            TaskState::Complete => {
                if st.state != State::Complete {
                    out.insert(
                        (*path).to_string(),
                        FileState {
                            state: State::Downloading,
                            completed: t.total,
                            total: t.total,
                            speed: 0,
                            err: String::new(),
                        },
                    );
                }
            }
            // 已移除的任务保持基底状态不变
            TaskState::Removed => {}
        }
    }
    out
}

/// 进度条的几个数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressInfo {
    pub total_bytes: i64,
    pub done_bytes: i64,
    pub speed: i64,
    pub percent: i32,
}

/// 见契约 §1.3/§1.4——已完成量**按文件二选一**，同路径多任务取 `max`。
///
/// 已完成量**按文件二选一**，与 [`compose`] 的四态规则对称（§1.3）——本地扫描是旧快照，
/// 任务是活的：
///
///   - 该文件有活跃任务（active / waiting）→ 取这些任务里**进度最远**的那个的已完成字节，
///     并按文件大小夹取。
///     客户双击重下一个已完整的文件时，进度条反映的是**这次重传**的进度，
///     而不是"已完整"的旧事实——否则就是"点了重下、进度条一直 100% 不动"。
///   - 否则本地已完整 → 取文件大小。
///   - 否则 0。
///
/// ⚠️ **绝不能把两者相加**：同一份字节会被算两遍（6 字节的文件报出 9 字节），
/// 界面上会出现"已完成 > 总量"。二选一才对得上 `compose` 里"活跃任务压过本地已完整"
/// （§1.3：原注释里"已完成量 = 已完整字节 + 下载中任务的已完成字节之和"那句话**已不成立**）。
pub fn progress(
    files: &[File],
    complete: &BTreeSet<String>,
    gid_to_path: &BTreeMap<String, String>,
    tasks: &[Task],
) -> ProgressInfo {
    let mut p = ProgressInfo {
        total_bytes: 0,
        done_bytes: 0,
        speed: 0,
        percent: 0,
    };

    // 清单内的路径集合——映射里指向清单外路径的任务，连速度也不该混进来，
    // 否则界面上的速度会虚高（Compose 有同样的守卫）。
    let in_manifest: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();

    // 每个路径的活跃任务已完成字节。同一文件有多个任务（例如客户点了两次）时取 **max**
    // 而不是取和：它们传的是同一份数据，取和会把多路进度叠加成"假完成"
    // （两个各 3/6 的任务会被报成 6/6 = 100%），"最远的那个"才是诚实的已完成量（§1.4）。
    //
    // ⚠️ 与 `compose` 的口径差别只在这一处：**进度取最远、状态取最新**。
    // `compose` 对同一路径只应用胜出的那一个任务；这里则要在多个任务里挑进度最大的那个。
    let mut active: BTreeMap<&str, i64> = BTreeMap::new();
    for t in tasks {
        let Some(path) = gid_to_path.get(&t.gid).map(|p| p.as_str()) else {
            continue; // 映射里没有的 GID 不是我们的任务（或已清理）
        };
        if !in_manifest.contains(path) {
            continue; // 清单里没有的路径：不计字节，也不计速度
        }
        if matches!(t.state, TaskState::Active | TaskState::Waiting) {
            let cur = active.entry(path).or_insert(0);
            if t.completed > *cur {
                *cur = t.completed;
            }
            p.speed += t.speed;
        }
    }

    for f in files {
        p.total_bytes += f.size;
        if let Some(&done) = active.get(f.path.as_str()) {
            // 有活跃任务时以任务为准：客户双击重下一个已完整的文件，
            // 进度条必须反映这次重传，而不是"已完整"的旧事实。
            // 单个文件也要夹取——任务超报会掩盖别的文件还没下完这件事。
            p.done_bytes += done.min(f.size);
            continue;
        }
        if complete.contains(&f.path) {
            p.done_bytes += f.size;
        }
    }

    if p.total_bytes > 0 {
        // 整数除法向零截断（§1.6 的注记要求"截断"而非四舍五入），与 Go 的 `int(...)` 一致。
        p.percent = (p.done_bytes * 100 / p.total_bytes) as i32;
        // **纵深防御**（§1.6）：按文件夹取之后 `done_bytes <= total_bytes` 恒成立，
        // 所以这个夹取**结构性不可达**、没有测试能判别它。保留它是防**将来**有人改坏
        // 上面那两处夹取（把 `min` 去掉、或改回求和）。这与"什么都不断言的假测试"不同：
        // 这是真代码配一句诚实的说明。
        if p.percent > 100 {
            p.percent = 100;
        }
    }
    p
}

/// 按树序返回所有文件节点，供界面按行渲染、以及「全选 / 反选」构造勾选集合使用。
///
/// ⚠️ 默认勾选（`default_selected`）**不经过**这里：它吃的是 `compose` 的 map，与树序无关。
///
/// 遍历顺序**必须排序**：Go 那边 map 的迭代顺序是随机的，不排序会让界面每次刷新时
/// 行的次序都变。Rust 这边 `BTreeMap` 本来就按 key 升序迭代（与 Go `sort.Strings`
/// 同为字节序，中文/空格的相对次序也一致），**但排序语义仍然是本函数的契约**——
/// 将来若把 `children` 换成别的容器，这里必须自己排。`flatten_is_deterministic_and_files_ordered`
/// 钉的就是这条。
pub fn flatten(root: &BTreeMap<String, Node>) -> Vec<&Node> {
    fn walk<'a>(level: &'a BTreeMap<String, Node>, out: &mut Vec<&'a Node>) {
        // 目录先于文件，便于界面上"目录在前"
        for n in level.values() {
            if n.node_type == NodeType::Dir {
                walk(&n.children, out);
            }
        }
        for n in level.values() {
            if n.node_type == NodeType::File {
                out.push(n);
            }
        }
    }
    let mut out: Vec<&Node> = Vec::new();
    walk(root, &mut out);
    out
}

/// 待下载的**唯一**判据（契约 §1.7）。任何需要它的地方都调它，不得就地重写。
///
/// 返回**仍需要下载**的清单路径，按传入的 `files` 顺序。
///
/// 界面有 6 处需要这个判断（按钮可用性、全选、反选、下载全部、下载选中、勾选计数），
/// 散在各处已经漂移过一次：`downloadAll` 只看了 planner 的分类、漏了四态那一半，
/// 于是「已完整但分类仍是待下载」的文件会被**再下一遍**（50GB 上是几十分钟磁盘 I/O）。
/// 所以收敛成一个纯函数——**四态是"本地已完整"的唯一真相来源**，planner 的分类
/// 是加载那一刻的旧结论，任务跑完之后就不再作数了。
///
/// ⚠️ `states` 里**没有**的路径按"待下载"处理：那是尚未扫描（或扫描尚未回填）的初始态，
/// 此时宁可认为它还需要下——多下一个文件只是浪费带宽，少下一个是少交。
pub fn pending_paths(files: &[File], states: &BTreeMap<String, FileState>) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        if let Some(st) = states.get(&f.path) {
            if st.state == State::Complete {
                continue;
            }
        }
        out.push(f.path.clone());
    }
    out
}

/// 已完整的默认不勾选。
///
/// 理由：已完整的行默认勾上，客户一点「下载选中」就会把已下好的再传一遍（白传）。
pub fn default_selected(states: &BTreeMap<String, FileState>) -> BTreeSet<String> {
    states
        .iter()
        .filter(|(_, st)| st.state != State::Complete)
        .map(|(path, _)| path.clone())
        .collect()
}

/// 同一路径上多个任务谁说了算（契约 §1.2/§1.5）：活跃 3 > 失败 2 > 完成 1 > 移除 0。
///
/// 活跃最高：客户刚点了重下，界面必须给出这次重传的进度。
/// 失败次之：**失败是比「已传完」更值得暴露的事实**（传完了但校验还没跑）。
/// 移除最低：它不是一次真正的下载结果。
///
/// ⚠️ **不要改成「完成优先于失败」**（§1.5）。场景：某文件本地未判完整、同一路径同时挂着
/// 陈旧的 `Error` 与刚传完待校验的 `Complete` 时，现在会**确定地**显示「失败」。
/// 两个方向都是"错报"，但代价不对称——错报「失败」会促使客户重试
/// （`-c` 恒开，重试廉价且幂等），错报「下载中 100%」则是一个**不会自愈的停滞**，
/// 客户会以为下好了。方向与「**不得静默少交**」同向。由
/// `compose_stale_failed_task_beats_fresh_complete_task` 钉住。
///
/// Go 侧这里有个 `default: return 0`（兜住 `TaskRemoved` 与任何未知字面量）；
/// Rust 的枚举是封闭的，`Removed` 就是那个 default，无需通配分支。
fn task_rank(s: TaskState) -> i32 {
    match s {
        TaskState::Active | TaskState::Waiting => 3,
        TaskState::Error => 2,
        TaskState::Complete => 1,
        TaskState::Removed => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应 Go 的 `manifest()`——三条路径，含深层目录、空格与非 ASCII。
    fn manifest() -> Vec<File> {
        vec![
            file("PFX/Readme.txt", 6, "1"),
            file("PFX/a/b/deep.txt", 4, "2"),
            file("PFX/中文 名.bin", 100, "3"),
        ]
    }

    fn file(path: &str, size: i64, crc64: &str) -> File {
        File {
            path: path.to_string(),
            size,
            crc64: crc64.to_string(),
            source_mtime: String::new(),
        }
    }

    /// 对应 Go 里 `engine.Task{GID: ...}` 的**部分字段字面量**：其余字段取零值。
    fn task(gid: &str, state: TaskState) -> Task {
        Task {
            gid: gid.to_string(),
            total: 0,
            completed: 0,
            speed: 0,
            conns: 0,
            state,
            err: String::new(),
        }
    }

    /// 对应 Go 的 `map[string]bool{...}` / `nil`。
    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 对应 Go 的 `map[string]string{...}` / `nil`。
    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// 对应 Go `TestBuildTreeNestsByPath`。
    #[test]
    fn build_tree_nests_by_path() {
        let root = build_tree(&manifest());
        let pfx = match root.get("PFX") {
            Some(n) => n,
            None => panic!("顶层应有 PFX 目录: {root:?}"),
        };
        assert_eq!(pfx.node_type, NodeType::Dir, "顶层应有 PFX 目录: {root:?}");
        assert!(pfx.children.contains_key("Readme.txt"), "PFX 下应有 Readme.txt");
        assert_eq!(
            pfx.children["a"].children["b"].children["deep.txt"].node_type,
            NodeType::File,
            "深层文件应被正确嵌套"
        );
        assert!(
            pfx.children.contains_key("中文 名.bin"),
            "含空格与非 ASCII 的文件名应原样成为节点"
        );
    }

    /// 对应 Go `TestTreeNodeCarriesPathSizeAndCRC`。
    #[test]
    fn tree_node_carries_path_size_and_crc() {
        let root = build_tree(&manifest());
        let f = &root["PFX"].children["Readme.txt"];
        assert!(
            f.path == "PFX/Readme.txt" && f.size == 6 && f.crc64 == "1",
            "叶节点应带原文路径/大小/crc64: {f:?}"
        );
    }

    /// 源文件时间必须从 `delivery::File` 逐字带到 `view::Node` 上。
    ///
    /// ⚠️ 这一条守的是本任务最容易漏的那一处：协议 JSON 的两个出口读的是 `view::Node`，
    /// **不是** `delivery::File`。只给 `File` 加字段、忘了 `Node::leaf` 这一行的话，
    /// 整棵树上所有节点的 `source_mtime` 都还是空串——**没有任何错误**，字段就是到不了壳。
    /// 值本身也有判别力：一旦被"顺手规范化"成 UTC，`+08:00` 会变成 `Z`，逐字比较变红。
    #[test]
    fn tree_node_carries_source_mtime_verbatim() {
        let files = vec![File {
            path: "PFX/a.txt".to_string(),
            size: 1,
            crc64: "1".to_string(),
            source_mtime: "2026-09-14T12:00:00+08:00".to_string(),
        }];
        let root = build_tree(&files);
        assert_eq!(
            root["PFX"].children["a.txt"].source_mtime, "2026-09-14T12:00:00+08:00",
            "叶节点必须逐字带上源文件时间（不是解析后的时刻）"
        );
        // 时间只属于文件：目录节点没有它，是空串（与 path/crc64 同一种做法）
        assert_eq!(
            root["PFX"].source_mtime, "",
            "目录节点不该有源文件时间"
        );
    }

    /// 对应 Go `TestComposeFourStates`。
    #[test]
    fn compose_four_states() {
        // 三路数据：清单 + 本地已完整集合 + RPC 任务
        let files = manifest();
        let complete = set(&["PFX/Readme.txt"]);
        let tasks = vec![Task {
            completed: 50,
            total: 100,
            ..task("g1", TaskState::Active)
        }];
        // gid → manifest 相对路径的映射由 Daemon 维护
        let gid_to_path = map(&[("g1", "PFX/中文 名.bin")]);

        let got = compose(&files, &complete, &gid_to_path, &tasks);
        assert_eq!(
            got["PFX/Readme.txt"].state,
            State::Complete,
            "本地已完整的应为 complete，实际 {:?}",
            got["PFX/Readme.txt"].state
        );
        assert_eq!(
            got["PFX/中文 名.bin"].state,
            State::Downloading,
            "有活动任务的应为 downloading，实际 {:?}",
            got["PFX/中文 名.bin"].state
        );
        assert_eq!(
            got["PFX/中文 名.bin"].completed, 50,
            "应带上已完成字节，实际 {}",
            got["PFX/中文 名.bin"].completed
        );
        assert_eq!(
            got["PFX/a/b/deep.txt"].state,
            State::Pending,
            "既不在本地也没任务的应为 pending，实际 {:?}",
            got["PFX/a/b/deep.txt"].state
        );
    }

    /// 对应 Go `TestComposeLocalCompleteWinsOverNoTask`。
    #[test]
    fn compose_local_complete_wins_over_no_task() {
        let files = manifest();
        let got = compose(&files, &set(&["PFX/Readme.txt"]), &BTreeMap::new(), &[]);
        assert_eq!(got["PFX/Readme.txt"].state, State::Complete, "无任务但本地完整 → complete");
    }

    /// 对应 Go `TestComposeFailedTask`。
    #[test]
    fn compose_failed_task() {
        let files = manifest();
        let tasks = vec![Task {
            err: "连接超时".to_string(),
            ..task("g", TaskState::Error)
        }];
        let got = compose(&files, &set(&[]), &map(&[("g", "PFX/Readme.txt")]), &tasks);
        assert_eq!(
            got["PFX/Readme.txt"].state,
            State::Failed,
            "失败任务应标 failed，实际 {:?}",
            got["PFX/Readme.txt"].state
        );
        assert_eq!(
            got["PFX/Readme.txt"].err, "连接超时",
            "失败原因应带出来，实际 {:?}",
            got["PFX/Readme.txt"].err
        );
    }

    /// 对应 Go `TestComposeIgnoresTasksWithoutKnownPath`。
    #[test]
    fn compose_ignores_tasks_without_known_path() {
        // 映射里没有的 GID 不得凭空造出一个节点。
        //
        // ⚠️ Go 侧的 `engine.Task{GID: "陌生"}` 没写 State，取零值 `TaskState("")`——
        // 一个**匹配不到任何 case** 的未知字面量。Rust 的枚举是封闭的，没有"未知值"；
        // 这里取 `Active`：它是**严格更强**的选择（若守卫被删，活跃状态会真的造出一个
        // 幽灵节点），而断言与 Go 逐字相同。
        let got = compose(
            &manifest(),
            &set(&[]),
            &BTreeMap::new(),
            &[task("陌生", TaskState::Active)],
        );
        assert_eq!(got.len(), 3, "应仍只有 3 个文件，实际 {}", got.len());
    }

    /// 对应 Go `TestProgressCounts`。
    #[test]
    fn progress_counts() {
        let files = manifest(); // 6 + 4 + 100 = 110
        let complete = set(&["PFX/Readme.txt"]);
        let tasks = vec![Task {
            completed: 25,
            total: 100,
            ..task("g1", TaskState::Active)
        }];
        let gid_to_path = map(&[("g1", "PFX/中文 名.bin")]);

        let p = progress(&files, &complete, &gid_to_path, &tasks);
        assert_eq!(p.total_bytes, 110, "总量应取自清单 = 110，实际 {}", p.total_bytes);
        // 已完成 = 已完整字节(6) + 下载中任务的已完成字节(25)
        assert_eq!(p.done_bytes, 31, "已完成字节应为 6+25=31，实际 {}", p.done_bytes);
        assert_eq!(p.percent, 28, "百分比应约 28，实际 {}", p.percent); // 31/110 ≈ 28%
    }

    /// 对应 Go `TestProgressClampsTo100`。
    #[test]
    fn progress_clamps_to_100() {
        // 任务报的已完成字节可能因预分配等原因超过清单值——不得显示 >100%
        let files = manifest();
        let tasks = vec![Task {
            completed: 999,
            total: 999,
            ..task("g1", TaskState::Active)
        }];
        let p = progress(&files, &set(&[]), &map(&[("g1", "PFX/中文 名.bin")]), &tasks);
        assert!(p.percent <= 100, "百分比不得超 100，实际 {}", p.percent);
    }

    /// 对应 Go `TestProgressZeroTotalDoesNotDivideByZero`。
    #[test]
    fn progress_zero_total_does_not_divide_by_zero() {
        let p = progress(&[], &set(&[]), &BTreeMap::new(), &[]);
        assert!(
            p.percent == 0 && p.total_bytes == 0,
            "空清单应得零值，实际 {p:?}"
        );
    }

    /// 对应 Go `TestFlattenIsDeterministicAndFilesOrdered`。
    #[test]
    fn flatten_is_deterministic_and_files_ordered() {
        // map 迭代顺序是随机的——不排序会让界面每次刷新时行的次序都变
        let root = build_tree(&manifest());
        let first = flatten(&root);
        for i in 0..50 {
            let again = flatten(&root);
            assert_eq!(
                again.len(),
                first.len(),
                "第 {i} 次调用长度不同: {} vs {}",
                again.len(),
                first.len()
            );
            for j in 0..first.len() {
                assert_eq!(
                    again[j].path, first[j].path,
                    "第 {i} 次调用次序变了（位置 {j}）: {:?} vs {:?}",
                    again[j].path, first[j].path
                );
            }
        }
        // 目录先于文件：PFX/a/b/deep.txt 排在 PFX 下的两个文件之前；
        // 同层的文件按名字升序 → "Readme.txt" < "中文 名.bin"
        let want = ["PFX/a/b/deep.txt", "PFX/Readme.txt", "PFX/中文 名.bin"];
        assert_eq!(first.len(), want.len(), "应返回 {} 个文件节点，实际 {}", want.len(), first.len());
        for (i, w) in want.iter().enumerate() {
            assert_eq!(first[i].path, *w, "第 {i} 个应为 {w:?}，实际 {:?}", first[i].path);
        }
    }

    /// 对应 Go `TestPendingPathsExcludesComplete`。
    ///
    /// `pending_paths` 是"谁还需要下载"的**唯一判据**（界面 6 处都调它）。
    /// 原来的界面里这条规则抄了 5 遍，抄出了偏差：downloadAll 只看 planner 的分类、
    /// 漏了四态那一半，于是"刚下完但分类仍是待下载"的文件会被再下一遍。
    #[test]
    fn pending_paths_excludes_complete() {
        let files = manifest();
        let states = compose(&files, &set(&["PFX/Readme.txt"]), &BTreeMap::new(), &[]);

        let got = pending_paths(&files, &states);
        assert_eq!(got.len(), 2, "应返回 2 个待下载路径，实际 {}: {got:?}", got.len());
        for p in &got {
            assert_ne!(
                p, "PFX/Readme.txt",
                "本地已完整（四态为 complete）的文件不该出现在待下载里——再下它一遍就是白传，\
                 50GB 的交付上还要重算几十分钟 CRC64"
            );
        }
        assert!(
            got[0] == "PFX/a/b/deep.txt" && got[1] == "PFX/中文 名.bin",
            "顺序应与传入的 files 一致（也不该漏掉谁）: {got:?}"
        );

        // 失败的行**必须**仍在待下载里：它是需要重试的那一批
        let tasks = vec![Task {
            err: "x".to_string(),
            ..task("g", TaskState::Error)
        }];
        let states2 = compose(&files, &set(&[]), &map(&[("g", "PFX/Readme.txt")]), &tasks);
        assert_eq!(
            pending_paths(&files, &states2).len(),
            3,
            "失败的文件必须仍在待下载里，实际 {:?}",
            pending_paths(&files, &states2)
        );
    }

    /// 对应 Go `TestPendingPathsTreatsUnknownAsPending`。
    ///
    /// states 里没有的路径按"待下载"处理：那是尚未扫描的初始态。
    /// 方向是刻意选的——多下一个文件只是浪费带宽，少下一个是少交。
    #[test]
    fn pending_paths_treats_unknown_as_pending() {
        let files = manifest();
        // 空 states：一个都没扫过
        let got = pending_paths(&files, &BTreeMap::new());
        assert_eq!(got.len(), files.len(), "空 states 时应全部视为待下载，实际 {} 个: {got:?}", got.len());
        // 只有部分路径有状态：其余仍视为待下载
        let partial = BTreeMap::from([(
            "PFX/Readme.txt".to_string(),
            FileState {
                state: State::Complete,
                completed: 0,
                total: 0,
                speed: 0,
                err: String::new(),
            },
        )]);
        let got = pending_paths(&files, &partial);
        assert_eq!(
            got.len(),
            files.len() - 1,
            "已知的已完整项应被排除、未知的应保留，实际 {got:?}"
        );
        for p in &got {
            assert_ne!(p, "PFX/Readme.txt", "已完整的项没有被排除");
        }
    }

    /// 对应 Go `TestDefaultSelectedSkipsComplete`。
    #[test]
    fn default_selected_skips_complete() {
        let states = compose(&manifest(), &set(&["PFX/Readme.txt"]), &BTreeMap::new(), &[]);
        let sel = default_selected(&states);
        assert!(
            !sel.contains("PFX/Readme.txt"),
            "已完整的文件默认不应勾选——否则客户一点「下载选中」就把下好的又传一遍"
        );
        assert!(
            sel.contains("PFX/a/b/deep.txt") && sel.contains("PFX/中文 名.bin"),
            "未完整的文件默认应勾选"
        );
        assert_eq!(sel.len(), 2, "默认勾选集合应恰好是 2 个，实际 {}", sel.len());
    }

    /// 对应 Go `TestDefaultSelectedIncludesFailedAndDownloading`。
    #[test]
    fn default_selected_includes_failed_and_downloading() {
        // 失败与下载中的行都必须默认勾选：漏掉失败的那一行，客户重试时就把它忘了
        let tasks = vec![Task {
            err: "x".to_string(),
            ..task("g", TaskState::Error)
        }];
        let states = compose(&manifest(), &set(&[]), &map(&[("g", "PFX/Readme.txt")]), &tasks);
        let sel = default_selected(&states);
        assert!(sel.contains("PFX/Readme.txt"), "失败的文件必须默认勾选");
        assert_eq!(sel.len(), 3, "应勾选全部 3 个，实际 {}", sel.len());
        // 名字里的「下载中」也必须真的构造到：只喂 error 任务的话，
        // 这条测试从没执行过 State::Downloading 那一支。
        let tasks2 = vec![Task {
            completed: 1,
            total: 4,
            ..task("g2", TaskState::Active)
        }];
        let sel2 = default_selected(&compose(
            &manifest(),
            &set(&[]),
            &map(&[("g2", "PFX/a/b/deep.txt")]),
            &tasks2,
        ));
        assert!(sel2.contains("PFX/a/b/deep.txt"), "下载中的文件必须默认勾选");
        assert_eq!(
            sel2.len(),
            3,
            "下载中/失败/待下载三种都应勾选（3 个），实际 {}",
            sel2.len()
        );
    }

    /// 对应 Go `TestComposeActiveTaskBeatsLocalComplete`。
    #[test]
    fn compose_active_task_beats_local_complete() {
        // 客户双击一个已完整的文件要求重下：必须看到进度，不能被"完整"压住
        let states = compose(
            &manifest(),
            &set(&["PFX/Readme.txt"]),
            &map(&[("g", "PFX/Readme.txt")]),
            &[Task {
                completed: 3,
                total: 6,
                ..task("g", TaskState::Active)
            }],
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Downloading,
            "活跃任务应压过本地已完整，实际 {:?}",
            states["PFX/Readme.txt"].state
        );
    }

    /// 对应 Go `TestComposeFinishedTaskDoesNotOverwriteLocalComplete`。
    #[test]
    fn compose_finished_task_does_not_overwrite_local_complete() {
        // 残留的旧任务不得把磁盘上已校验完整的文件谎报成失败/下载中
        let states = compose(
            &manifest(),
            &set(&["PFX/Readme.txt"]),
            &map(&[("g", "PFX/Readme.txt")]),
            &[Task {
                err: "旧的失败".to_string(),
                ..task("g", TaskState::Error)
            }],
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Complete,
            "已结束的任务不应覆盖本地已完整，实际 {:?}",
            states["PFX/Readme.txt"].state
        );
        assert_eq!(
            states["PFX/Readme.txt"].err, "",
            "既然判定为完整，就不该残留失败原因，实际 {:?}",
            states["PFX/Readme.txt"].err
        );
    }

    /// 对应 Go `TestComposeFinishedCompleteTaskDoesNotOverwriteLocalComplete`。
    #[test]
    fn compose_finished_complete_task_does_not_overwrite_local_complete() {
        // 裁定 3 说「已结束的任务（complete / error / removed）不优先」，上面那条只钉了 error。
        // TaskComplete 是另一个分支：残留的旧 complete 任务同样不该把已校验完整的文件
        // 一直挂在「下载中 100%」。
        let files = manifest();
        let comp = set(&["PFX/Readme.txt"]);
        let path = map(&[("g", "PFX/Readme.txt")]);

        let got = compose(
            &files,
            &comp,
            &path,
            &[Task {
                completed: 6,
                total: 6,
                ..task("g", TaskState::Complete)
            }],
        );
        assert_eq!(
            got["PFX/Readme.txt"].state,
            State::Complete,
            "已完整的文件不该被旧 complete 任务改判成下载中，实际 {:?}",
            got["PFX/Readme.txt"].state
        );

        // 但守卫必须只挡"已完整"这一种：文件没完整时，这条分支还得照常工作，
        // 否则把整个 case 删掉也能让上面那句通过。
        let got3 = compose(
            &files,
            &set(&[]),
            &map(&[("g", "PFX/中文 名.bin")]),
            &[Task {
                completed: 100,
                total: 100,
                ..task("g", TaskState::Complete)
            }],
        );
        assert_eq!(
            got3["PFX/中文 名.bin"].state,
            State::Downloading,
            "任务传完但未校验的文件应仍标下载中，实际 {:?}",
            got3["PFX/中文 名.bin"].state
        );
    }

    /// 对应 Go `TestComposeWaitingTaskBeatsLocalComplete`。
    #[test]
    fn compose_waiting_task_beats_local_complete() {
        // 裁定 3 的规则写的是「活跃任务（active / waiting）」——waiting 也算。
        // 排队中的文件若被"本地已完整"压住，客户会看到一行"完整"却什么都没在下，
        // 与双击重下那条路径是同一个毛病。
        let states = compose(
            &manifest(),
            &set(&["PFX/Readme.txt"]),
            &map(&[("g", "PFX/Readme.txt")]),
            &[Task {
                total: 6,
                ..task("g", TaskState::Waiting)
            }],
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Downloading,
            "排队中的任务也属活跃任务，应压过本地已完整，实际 {:?}",
            states["PFX/Readme.txt"].state
        );
    }

    /// 对应 Go `TestComposeIgnoresKnownGIDForUnknownPath`。
    ///
    /// 上一条只喂了空映射，走的是「GID 查不到」那条分支；这里补上另一条：
    /// GID 查得到、但它指向的路径不在清单里——同样不得凭空造节点，
    /// 否则界面上会冒出一行既没有大小也没有 crc64 的幽灵文件。
    #[test]
    fn compose_ignores_known_gid_for_unknown_path() {
        let tasks = vec![Task {
            completed: 1,
            total: 2,
            ..task("g", TaskState::Active)
        }];
        let got = compose(
            &manifest(),
            &set(&[]),
            &map(&[("g", "PFX/不存在的.bin")]),
            &tasks,
        );
        assert_eq!(got.len(), 3, "应仍只有 3 个文件，实际 {}", got.len());
        assert!(
            !got.contains_key("PFX/不存在的.bin"),
            "清单里没有的路径不得出现在结果里"
        );
    }

    /// 对应 Go `TestProgressTruncatesRatherThanRounds`。
    #[test]
    fn progress_truncates_rather_than_rounds() {
        // 简报步骤 4 的注记要求「截断」而不是四舍五入，但 `progress_counts` 那组
        // 数（31/110 = 28.18）两种取整都得 28，判别不了。这里挑一组两法结果不同的：
        // 2/3 = 66.67 → 截断 66、四舍五入 67。
        let files = vec![file("PFX/a.bin", 3, "")];
        let tasks = vec![Task {
            completed: 2,
            total: 3,
            ..task("g", TaskState::Active)
        }];
        let p = progress(&files, &set(&[]), &map(&[("g", "PFX/a.bin")]), &tasks);
        assert_eq!(p.percent, 66, "取整方式应为截断（2/3 = 66.67 → 66），实际 {}", p.percent);
    }

    /// 对应 Go `TestComposeStaleFailedTaskDoesNotOverrideActiveOne`。
    #[test]
    fn compose_stale_failed_task_does_not_override_active_one() {
        // 同一个文件上：一个正在重下的活跃任务 + 一个更早的失败任务。
        // engine.List 把活跃的排在前面、已结束的排在后面——若按顺序覆盖，
        // 这个正在下载的文件会被那个陈旧失败改判成「失败」。
        let tasks = vec![
            Task {
                completed: 3,
                total: 6,
                speed: 1024,
                ..task("live", TaskState::Active)
            },
            Task {
                err: "旧的失败".to_string(),
                ..task("stale", TaskState::Error)
            },
        ];
        let states = compose(
            &manifest(),
            &set(&[]),
            &map(&[("live", "PFX/Readme.txt"), ("stale", "PFX/Readme.txt")]),
            &tasks,
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Downloading,
            "活跃任务必须压过陈旧的失败任务，实际 {:?}",
            states["PFX/Readme.txt"].state
        );
        assert_eq!(
            states["PFX/Readme.txt"].err, "",
            "既然判定为下载中，就不该带失败原因，实际 {:?}",
            states["PFX/Readme.txt"].err
        );
    }

    /// 对应 Go `TestComposeStaleCompleteTaskDoesNotOverrideActiveOne`。
    #[test]
    fn compose_stale_complete_task_does_not_override_active_one() {
        let tasks = vec![
            Task {
                completed: 1,
                total: 6,
                ..task("live", TaskState::Active)
            },
            Task {
                total: 6,
                ..task("old", TaskState::Complete)
            },
        ];
        let states = compose(
            &manifest(),
            &set(&[]),
            &map(&[("live", "PFX/Readme.txt"), ("old", "PFX/Readme.txt")]),
            &tasks,
        );
        let s = &states["PFX/Readme.txt"];
        assert!(
            s.state == State::Downloading && s.completed == 1,
            "应以活跃任务的进度为准（1/6），实际 {s:?}"
        );
    }

    /// 对应 Go `TestComposeTaskRankingIsOrderIndependent`。
    #[test]
    fn compose_task_ranking_is_order_independent() {
        // 上面两条依赖 aria2 的返回顺序（活跃在前）。但实现承诺的是「**与切片顺序无关**」：
        // 挑胜出任务靠的是语义排名，不是谁先来。把陈旧任务放在前面，
        // 结果必须一样——否则这段代码就只是碰巧对了。
        let tasks = vec![
            Task {
                err: "旧的失败".to_string(),
                ..task("stale", TaskState::Error)
            },
            Task {
                completed: 3,
                total: 6,
                ..task("live", TaskState::Active)
            },
        ];
        let states = compose(
            &manifest(),
            &set(&[]),
            &map(&[("live", "PFX/Readme.txt"), ("stale", "PFX/Readme.txt")]),
            &tasks,
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Downloading,
            "顺序颠倒后仍应判下载中（排名说了算），实际 {:?}",
            states["PFX/Readme.txt"].state
        );
        assert_eq!(
            states["PFX/Readme.txt"].completed, 3,
            "应以活跃任务的进度为准（3），实际 {}",
            states["PFX/Readme.txt"].completed
        );
    }

    /// 对应 Go `TestComposeActiveTaskCarriesSpeedAndTotal`。
    #[test]
    fn compose_active_task_carries_speed_and_total() {
        // 速度是用户反馈③「看得见进度」要显示的字段——没有断言就等于没有保障
        let states = compose(
            &manifest(),
            &set(&[]),
            &map(&[("g", "PFX/Readme.txt")]),
            &[Task {
                completed: 3,
                total: 6,
                speed: 1024,
                ..task("g", TaskState::Active)
            }],
        );
        let s = &states["PFX/Readme.txt"];
        assert_eq!(s.speed, 1024, "活跃任务的速度应带出来，实际 {}", s.speed);
        assert_eq!(s.total, 6, "活跃任务的总字节应带出来，实际 {}", s.total);
        assert_eq!(s.completed, 3, "活跃任务的已完成字节应带出来，实际 {}", s.completed);
    }

    /// 对应 Go `TestProgressDoesNotDoubleCountRedownloadedCompleteFile`。
    #[test]
    fn progress_does_not_double_count_redownloaded_complete_file() {
        // 本地已完整 + 有活跃任务（双击重下）时，不能把同一份字节算两遍
        let files = manifest(); // 6 + 4 + 100 = 110
        let complete = set(&["PFX/Readme.txt"]);
        let tasks = vec![Task {
            completed: 3,
            total: 6,
            ..task("g", TaskState::Active)
        }];
        let p = progress(&files, &complete, &map(&[("g", "PFX/Readme.txt")]), &tasks);
        assert_eq!(
            p.done_bytes, 3,
            "重下中的文件应以任务进度为准（3），而不是 6+3=9，实际 {}",
            p.done_bytes
        );
        assert!(
            p.done_bytes <= p.total_bytes,
            "已完成量不得超总量，实际 {} > {}",
            p.done_bytes,
            p.total_bytes
        );
    }

    /// 对应 Go `TestProgressClampsSingleTaskOvershoot`。
    #[test]
    fn progress_clamps_single_task_overshoot() {
        // 任务报的已完成字节可能因预分配超过文件大小——按文件夹取，
        // 否则一个文件超报会把别的文件还没下完这件事掩盖过去
        let files = manifest();
        let tasks = vec![Task {
            completed: 999,
            total: 999,
            ..task("g1", TaskState::Active)
        }];
        let p = progress(&files, &set(&[]), &map(&[("g1", "PFX/中文 名.bin")]), &tasks);
        assert_eq!(p.done_bytes, 100, "应夹到该文件大小 100，实际 {}", p.done_bytes);
    }

    /// 对应 Go `TestProgressCountsWaitingTaskAndSpeed`。
    #[test]
    fn progress_counts_waiting_task_and_speed() {
        // 裁定 4 的规则写的是「活跃任务（active / waiting）」——排队中的也要计入，
        // 少了它，一个刚排上队的文件会在进度条上白占 0 字节（与 Compose 那边不一致）。
        // 同时钉住速度：用户反馈③要的就是"看得见进度"，速度是要显示出去的字段。
        let files = manifest(); // 6 + 4 + 100 = 110
        let tasks = vec![
            Task {
                completed: 2,
                total: 6,
                ..task("g1", TaskState::Waiting)
            },
            Task {
                completed: 10,
                total: 100,
                speed: 2048,
                ..task("g2", TaskState::Active)
            },
        ];
        let p = progress(
            &files,
            &set(&[]),
            &map(&[("g1", "PFX/Readme.txt"), ("g2", "PFX/中文 名.bin")]),
            &tasks,
        );
        assert_eq!(
            p.done_bytes, 12,
            "排队中与下载中的任务都应计入（2+10=12），实际 {}",
            p.done_bytes
        );
        assert_eq!(p.speed, 2048, "实时速度应带出来给界面显示，实际 {}", p.speed);
        assert_eq!(p.percent, 10, "百分比应为 10，实际 {}", p.percent); // 12/110 = 10.9 → 截断 10
    }

    /// 对应 Go `TestProgressTakesFarthestTaskNotSum`。
    #[test]
    fn progress_takes_farthest_task_not_sum() {
        // 同一文件上两个任务各完成 3/6（例如客户点了两次）：它们传的是同一份数据，
        // 取和会报成 6/6 = 100%「假完成」，诚实的是"最远的那个" = 3。
        let files = vec![file("PFX/a.bin", 6, "")];
        let tasks = vec![
            Task {
                completed: 3,
                total: 6,
                ..task("g1", TaskState::Active)
            },
            Task {
                completed: 3,
                total: 6,
                ..task("g2", TaskState::Active)
            },
        ];
        let p = progress(
            &files,
            &set(&[]),
            &map(&[("g1", "PFX/a.bin"), ("g2", "PFX/a.bin")]),
            &tasks,
        );
        assert_eq!(
            p.done_bytes, 3,
            "同一文件的多个任务应取最远（3）而不是取和（6），实际 {}",
            p.done_bytes
        );
        assert_eq!(p.percent, 50, "百分比应为 50（3/6），实际 {}", p.percent);
    }

    /// 对应 Go `TestProgressIgnoresTasksOutsideManifest`。
    #[test]
    fn progress_ignores_tasks_outside_manifest() {
        // 映射指向清单外路径的任务：字节不该计（Compose 已有守卫），
        // 速度同样不该计——否则界面上的速度会虚高，而清单里根本没这个文件。
        let files = manifest();
        let tasks = vec![Task {
            completed: 500,
            total: 500,
            speed: 4096,
            ..task("ghost", TaskState::Active)
        }];
        let p = progress(
            &files,
            &set(&[]),
            &map(&[("ghost", "PFX/不存在的.bin")]),
            &tasks,
        );
        assert_eq!(p.done_bytes, 0, "清单外路径的字节不得计入，实际 {}", p.done_bytes);
        assert_eq!(p.speed, 0, "清单外路径的速度不得计入（否则速度虚高），实际 {}", p.speed);
        assert_eq!(p.total_bytes, 110, "总量仍应取自清单 = 110，实际 {}", p.total_bytes);
    }

    // ---- 以下 3 条**不在 Go 的 29 条里**，由本任务的变异检查补上（简报步骤 6 授权：
    // ---- 「task_rank 把完成排在失败之前 → 必须有测试红；若没有，补一条，契约 §1.5」；
    // ---- 修复轮 1 的「§1.2 排名表末档『移除 0』零守护」；
    // ---- 修复轮 2 的「`progress` 侧同一档：移除不是活跃任务」——
    // ---- 三条同一类根因：没有任何测试构造过 `TaskState::Removed`（§0：不许静默跳过））----

    /// 契约 §1.5：同一路径上「陈旧的失败」与「刚传完待校验的完成」并存、且文件本地未判完整时，
    /// 必须**确定地**判为失败（`失败 2 > 完成 1`）。
    ///
    /// ⚠️ 这条是 Rust 侧新增，Go 的 29 条里没有对应项：既有测试里没有任何一条让
    /// `TaskError` 与 `TaskComplete` 同时挂在同一路径上，所以把 `task_rank` 的两档对调
    /// （完成 2 > 失败 1）**一条都不会红**。方向不可颠倒的理由见契约 §1.5：
    /// 错报「失败」会促使客户重试（`-c` 恒开，重试廉价且幂等），
    /// 错报「下载中 100%」则是一个不会自愈的停滞。
    #[test]
    fn compose_stale_failed_task_beats_fresh_complete_task() {
        let tasks = vec![
            Task {
                err: "旧的失败".to_string(),
                ..task("stale", TaskState::Error)
            },
            Task {
                total: 6,
                completed: 6,
                ..task("fresh", TaskState::Complete)
            },
        ];
        let states = compose(
            &manifest(),
            &set(&[]),
            &map(&[("stale", "PFX/Readme.txt"), ("fresh", "PFX/Readme.txt")]),
            &tasks,
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Failed,
            "失败必须排在完成之前（§1.5）：错报「下载中 100%」是不会自愈的停滞，实际 {:?}",
            states["PFX/Readme.txt"].state
        );
        assert_eq!(
            states["PFX/Readme.txt"].err, "旧的失败",
            "既然判失败，失败原因也要带出来，实际 {:?}",
            states["PFX/Readme.txt"].err
        );
    }

    /// 契约 §1.2 排名表的**末档「移除 0」**：活跃 3 > 失败 2 > 完成 1 > **移除 0**。
    ///
    /// ⚠️ 这条是 Rust 侧新增（第 31 条），补的是与上一条**同一类**的输入形态缺口：
    /// Go 的 29 条里**没有任何一条构造过 `TaskState::Removed`**，所以末档既没被钉住、
    /// 也没在契约里注明（§0 要求二者必居其一）。审查者据此造的三个变异体
    /// （`Removed => 3`、`Removed => 1`、以及把 `Removed` 分支改写失败态）
    /// 在此之前**全部编译通过、全套 114 条全绿**。
    ///
    /// 这一档在生产输入下可达且后果确定：`engine/status.rs` 把 aria2 的 `"removed"`
    /// 映射进来；`Engine::list` 按 active→waiting→stopped 拼接（**已移除的排在活任务后面**）、
    /// `by_gid` 又只增不删，所以"同一路径上同时挂着 `Removed` 与 `Active`"是**常规输入**。
    /// 若 `Removed` 被排到与 `Active` 并列的最高档，先遇到者胜——**正在重下的文件会回落到基态**；
    /// 基态若是"已完整"，客户看到的就是「点了重下、进度条不动」，
    /// 正是契约 §1.1 之所以存在要防的那条反馈。
    ///
    /// 三个侧面写在一个测试里，沿用 Go `TestComposeFinishedCompleteTaskDoesNotOverwriteLocalComplete`
    /// 那种"守卫要挡哪种 + 不挡哪种"的同函数多场景写法：它们本就是**同一个排名档的三条出路**，
    /// 每条断言都带自己的说明，红了能直接定位是哪一侧。
    #[test]
    fn compose_removed_task_is_lowest_rank_and_changes_nothing() {
        // ---- 侧面 1：与最高档（Active 3）并列就会漏掉"正在重下"这个更强的事实 ----
        // 客户双击一个已完整的文件要求重下，同时那条旧任务已被移除：
        // 活跃任务必须赢（§1.1），界面必须给出这次重传的进度。
        let tasks = vec![
            // 已移除的排在**前面**：stopped 列表内部先后不可控，排名必须说了算
            task("gone", TaskState::Removed),
            Task {
                completed: 3,
                total: 6,
                ..task("live", TaskState::Active)
            },
        ];
        let states = compose(
            &manifest(),
            &set(&["PFX/Readme.txt"]),
            &map(&[("gone", "PFX/Readme.txt"), ("live", "PFX/Readme.txt")]),
            &tasks,
        );
        assert_eq!(
            states["PFX/Readme.txt"].state,
            State::Downloading,
            "移除档（0）必须低于活跃档（3）：与活跃并列会让正在重下的文件回落到基态「已完整」，\
             客户看到的就是「点了重下、进度条不动」（§1.2 末档），实际 {:?}",
            states["PFX/Readme.txt"].state
        );
        assert_eq!(
            states["PFX/Readme.txt"].completed, 3,
            "赢家必须是活跃任务，进度取它的 3，实际 {}",
            states["PFX/Readme.txt"].completed
        );

        // ---- 侧面 2：与「完成 1」并列就会漏掉"传完待校验"这个更强的事实 ----
        // 文件本地未判完整、刚传完待校验的任务与一条已移除的任务同时挂着：
        // 完成档必须赢，界面仍标「下载中 100%」等校验结果。
        let tasks2 = vec![
            task("gone", TaskState::Removed),
            Task {
                total: 6,
                ..task("done", TaskState::Complete)
            },
        ];
        let states2 = compose(
            &manifest(),
            &set(&[]),
            &map(&[("gone", "PFX/Readme.txt"), ("done", "PFX/Readme.txt")]),
            &tasks2,
        );
        assert_eq!(
            states2["PFX/Readme.txt"].state,
            State::Downloading,
            "移除档（0）必须低于完成档（1）：与完成并列会让刚传完待校验的文件回落到「待下载」，\
             客户看到进度又退回起点，实际 {:?}",
            states2["PFX/Readme.txt"].state
        );
        assert_eq!(
            states2["PFX/Readme.txt"].completed, 6,
            "赢家必须是完成任务，进度取它的 total（6/6），实际 {}",
            states2["PFX/Readme.txt"].completed
        );

        // ---- 侧面 3：它**不是一次真正的下载结果**，被应用了也必须什么都不改 ----
        // （Go 的 switch 里没有 `removed` 这一支，保持基底状态不变。）
        let only_gone = vec![task("gone", TaskState::Removed)];
        let path = map(&[("gone", "PFX/Readme.txt")]);

        // 3a. 本地未完整 → 必须保持 pending，不得被改判成 failed
        let s3a = compose(&manifest(), &set(&[]), &path, &only_gone);
        assert_eq!(
            s3a["PFX/Readme.txt"].state,
            State::Pending,
            "被移除的任务不进任何 case，基态应原样保留（pending），实际 {:?}",
            s3a["PFX/Readme.txt"].state
        );
        assert_eq!(
            s3a["PFX/Readme.txt"].err, "",
            "移除不是失败，不得带出失败原因，实际 {:?}",
            s3a["PFX/Readme.txt"].err
        );

        // 3b. 本地已完整 → 必须保持 complete（谎报失败与谎报成功同样有害，§1.1）
        let s3b = compose(&manifest(), &set(&["PFX/Readme.txt"]), &path, &only_gone);
        assert_eq!(
            s3b["PFX/Readme.txt"].state,
            State::Complete,
            "已校验完整的文件不得被一条残留的移除任务改判，实际 {:?}",
            s3b["PFX/Readme.txt"].state
        );
        assert_eq!(
            s3b["PFX/Readme.txt"].err, "",
            "既然判定为完整，就不该残留任何原因，实际 {:?}",
            s3b["PFX/Readme.txt"].err
        );
    }

    /// 契约 §1.1/§1.3 在 `progress` 侧的同一档：**被移除的任务不是活跃任务**，
    /// 它不得贡献已完成字节、不得贡献实时速度、也不得压过活跃任务。
    ///
    /// ⚠️ 这条是 Rust 侧新增（第 32 条，修复轮 2），补的是与第 31 条**同源**的缺口：
    /// `progress` 的活跃判据是 `matches!(t.state, Active | Waiting)`，而 Go 的 29 条里
    /// **没有任何一条把 `TaskState::Removed` 喂给 `Progress`**，所以这一支同样零守护。
    /// 实测：把判据改成 `Active | Waiting | Removed`，**编译通过、全套 115 条全绿**。
    ///
    /// 后果是**方向不确定的错报**：`progress` 里那一支在累加 `p.speed += t.speed` 的同时
    /// 还会让 `active[path]` 取到移除任务的 `completed`，而 §1.3 的规则是"有活跃任务就以
    /// 任务为准、不再计本地已完整"——于是
    ///   - 一条残留的移除任务会把**已完整**的文件从进度里**抹掉**（客户看到进度倒退）；
    ///   - 它的 `completed` 若比真活跃任务大，会把**正在重下**的进度**虚高**（同 §1.4 取 max 的害处）；
    ///   - 它的 `speed` 会混进来，界面上的速度虚高（与第 29 条要防的"清单外任务让速度虚高"同类）。
    ///
    /// 断言一律**精确到值**（不是"不等于某值"）：字节、速度、百分比三件都钉死，
    /// 这样"少算一部分"与"多算一部分"两种改坏都能红。
    #[test]
    fn progress_removed_task_contributes_nothing() {
        // ---- 侧面 1：单独一条移除任务：字节与速度都必须是 0 ----
        // 清单 6 + 4 + 100 = 110；移除任务报 500/500、速度 4096——一个数都不许进来。
        let tasks = vec![Task {
            completed: 500,
            total: 500,
            speed: 4096,
            ..task("gone", TaskState::Removed)
        }];
        let p = progress(
            &manifest(),
            &set(&[]),
            &map(&[("gone", "PFX/中文 名.bin")]),
            &tasks,
        );
        assert_eq!(p.total_bytes, 110, "总量仍应取自清单 = 110，实际 {}", p.total_bytes);
        assert_eq!(p.done_bytes, 0, "已移除的任务不得贡献已完成字节，实际 {}", p.done_bytes);
        assert_eq!(p.speed, 0, "已移除的任务不得贡献实时速度，实际 {}", p.speed);
        assert_eq!(p.percent, 0, "没有任何字节完成时应为 0%，实际 {}", p.percent);

        // ---- 侧面 2：与活跃任务同路径：不得压过活跃任务（§1.4 的取 max 只在活跃任务之间）----
        // 活跃任务 30/100、速度 1024；移除任务报得更大（100/100、4096）也必须输。
        let tasks2 = vec![
            Task {
                completed: 100,
                total: 100,
                speed: 4096,
                ..task("gone", TaskState::Removed)
            },
            Task {
                completed: 30,
                total: 100,
                speed: 1024,
                ..task("live", TaskState::Active)
            },
        ];
        let p2 = progress(
            &manifest(),
            &set(&[]),
            &map(&[("gone", "PFX/中文 名.bin"), ("live", "PFX/中文 名.bin")]),
            &tasks2,
        );
        assert_eq!(p2.done_bytes, 30, "取活跃任务的 30，不得取移除任务的 100，实际 {}", p2.done_bytes);
        assert_eq!(p2.speed, 1024, "只累计活跃任务的速度 1024，不得混入 4096，实际 {}", p2.speed);
        assert_eq!(p2.percent, 27, "30/110 = 27.27 → 截断 27，实际 {}", p2.percent);

        // ---- 侧面 3：不得把「本地已完整」从进度里抹掉（§1.3 的二选一方向）----
        // 文件本地已完整贡献 6；一条 `completed = 0` 的残留移除任务若被当成活跃任务，
        // §1.3 的"二选一"就会取它的 0，进度凭空从 6 掉到 0。
        let tasks3 = vec![task("gone", TaskState::Removed)];
        let p3 = progress(
            &manifest(),
            &set(&["PFX/Readme.txt"]),
            &map(&[("gone", "PFX/Readme.txt")]),
            &tasks3,
        );
        assert_eq!(
            p3.done_bytes, 6,
            "已完整的 6 字节不得被一条残留的移除任务抹掉，实际 {}",
            p3.done_bytes
        );
        assert_eq!(p3.percent, 5, "6/110 = 5.45 → 截断 5，实际 {}", p3.percent);
    }
}
