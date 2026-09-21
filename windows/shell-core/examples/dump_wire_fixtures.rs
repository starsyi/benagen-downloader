//! dump_wire_fixtures —— 把**真载荷**倒成一份 JSON 夹具（给前端那套无头验收用）。
//!
//! ```text
//! bash windows/scripts/make_wire_fixtures.sh                  # 写到默认位置
//! cargo run -q -p shell-core --example dump_wire_fixtures -- <输出路径>
//! ```
//!
//! ## 🔴 它为什么存在（这是一次审查换来的）
//!
//! 前端那套无头验收原本用的是**手抄的** stub 载荷。手抄的夹具与前端代码会
//! **犯同一个错** —— 两边都信了同一份简报里两个根本不存在的字段名
//! （`state.label` / `state.color`，而线上 `state` 只是一个**变体名**字符串），
//! 于是三十几条断言**全绿**，而真载荷下**状态列整列空白**、**双击目录变成下载整个目录**。
//! 也就是说：那套测试验证的是**错误假设的自洽**，不是"前端读得懂 Rust 发的东西"。
//!
//! ⇒ 夹具的源头必须是 Rust：**每一个字节都从 `api::*` 那几个纯函数出来**
//!   （今天：`api::state` / `api::tree` / `api::enqueue` / `api::transfers`）。
//!   本文件里手写的只有**内核那一侧**的输入（`list_dir` / `get_tree` / `enqueue` 的
//!   线上原文）—— 那是内核协议，不是壳的输出，而且那是本仓库既有测试的通行做法
//!   （"夹具一律是内核会发的那种线上 JSON 文本"）。
//!
//! ## ⚠️ 它不进交付物
//!
//! `examples/` 不参与 `cargo build` 的产物，也不在 `windows/web/` 下
//! （那个目录里的任何文件都会被 `frontendDist` 编进 exe）。
//!
//! ## ⚠️ 与命令层的关系（一处**已知的**重复，如实记账）
//!
//! `tree()` 无 `path` 那一支的**入参组装**（进度 / 勾选摘要 / 底栏那个动作）在
//! `shell-win/src/commands.rs:whole_tree` 里，而这里照它的三行又写了一遍
//! （`ProgressSummary::of` / `SelectionSummary::{of,size_index}` / `DownloadTargets::*`）。
//! 重复的是**组装那三行**，不是任何一条判据 —— 而那三行各自的计算全在
//! `presentation` 里（本文件只是把它们串起来）。
//! ⚠️ 要收口就得把 `whole_tree` 的组装搬进 `api::tree`（那会改命令层的一段既有论证：
//!    "它要读 `SessionView`"）—— 那是另一件事，**本任务不做**，记在任务 10 报告里。

use serde_json::json;
use std::collections::BTreeSet;

use shell_core::api;
use shell_core::presentation::about_info::AboutInfo;
use shell_core::presentation::app_preferences::{DownloadDirChange, DownloadDirectory};
use shell_core::presentation::batch_history::NoteDrafts;
use shell_core::presentation::breadcrumb::Breadcrumb;
use shell_core::presentation::browser_row::{BrowserRow, BrowserSelection, SelectionSummary};
use shell_core::presentation::download_targets::{DownloadTargets, EnqueueFeedback};
use shell_core::presentation::settings_form::SettingsForm;
use shell_core::presentation::transfer_row::{
    TransferGlobalSummary, TransferListEmpty, TransferRow,
};
use shell_core::presentation::verify_summary::ProgressSummary;
use shell_core::protocol::{
    DeliveryInfo, EngineState, EnqueueResult, FileState, FlatEntry, GlobalStat, ListDirResult,
    LoadState, Settings, SettingsResult, TransferItem, TreeResult, TreeNode,
};
use shell_core::session_view::SessionView;
use shell_core::storage::history::{History, HistoryEntry};
use shell_core::storage::preferences::Preferences;

/// 交付码（与样张 `a0-proof.html` 上那一个逐字相同）。
const CODE: &str = "R.a1b2C3d4E5f6G7h8I9j";

/// **根那一层**的内核原文（`list_dir` 的 result）。
///
/// ⚠️ 内容逐行对齐 `windows/design/a0-proof.html` 的样张（两个目录 + 五个文件、
///    四个状态各一个、名字里带 `×` 与空格）—— 这样"前端渲染 vs 样张"那次并排比
///    才是在比同一批数据。
/// ⚠️ **没有 `path` 键的就是目录项**（内核的 `entries_json` 只给文件发 `path`）——
///    这正是"目录的路径要壳自己拼"那条判据的现场。
const ROOT_WIRE: &str = r#"{"path":"","entries":[
  {"type":"dir","name":"01.RawData","children_count":10},
  {"type":"dir","name":"02.CleanData","children_count":6},
  {"type":"file","name":"C24-8_×_25WS024_L01_R1.fastq.gz",
   "path":"C24-8_×_25WS024_L01_R1.fastq.gz","crc64":"","size":4509715660,
   "completed":4509715660,"total":4509715660,"speed":0,"state":"complete","err":"",
   "source_mtime":"2025-09-03T14:22:07+08:00"},
  {"type":"file","name":"C24-8_×_25WS024_L01_R2.fastq.gz",
   "path":"C24-8_×_25WS024_L01_R2.fastq.gz","crc64":"","size":4402341478,
   "completed":1200000000,"total":4402341478,"speed":1048576,"state":"downloading","err":"",
   "source_mtime":"2025-09-03T14:22:07+08:00"},
  {"type":"file","name":"C24-8_×_25WS024_L02_R1.fastq.gz",
   "path":"C24-8_×_25WS024_L02_R1.fastq.gz","crc64":"","size":4187593113,
   "completed":0,"total":4187593113,"speed":0,"state":"pending","err":"",
   "source_mtime":"2025-09-03T14:31:44+08:00"},
  {"type":"file","name":"C24-8_×_25WS024_L02_R2.fastq.gz",
   "path":"C24-8_×_25WS024_L02_R2.fastq.gz","crc64":"","size":4080218931,
   "completed":0,"total":4080218931,"speed":0,"state":"failed","err":"磁盘空间不足",
   "source_mtime":"2025-09-03T14:31:44+08:00"},
  {"type":"file","name":"交付说明.pdf","path":"交付说明.pdf","crc64":"","size":1258291,
   "completed":0,"total":1258291,"speed":0,"state":"pending","err":"",
   "source_mtime":"2025-09-03T15:02:11+08:00"}]}"#;

/// **子目录那一层**（双击 `01.RawData` 之后看到的东西）。
const SUB_WIRE: &str = r#"{"path":"01.RawData","entries":[
  {"type":"file","name":"L01_R1.fastq.gz","path":"01.RawData/L01_R1.fastq.gz","crc64":"",
   "size":4509715660,"completed":0,"total":4509715660,"speed":0,"state":"pending","err":"",
   "source_mtime":"2025-09-03T14:22:07+08:00"},
  {"type":"file","name":"QC 图.png","path":"01.RawData/QC 图.png","crc64":"","size":716800,
   "completed":0,"total":716800,"speed":0,"state":"failed","err":"清单里没有这个文件",
   "source_mtime":"2025-09-03T14:25:03+08:00"}]}"#;

/// 整棵树（`get_tree` 的 result）：两个文件待下载、一个已完成。
const TREE_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[
   {"path":"C24-8_×_25WS024_L02_R1.fastq.gz","name":"C24-8_×_25WS024_L02_R1.fastq.gz","size":4187593113,"state":"pending"},
   {"path":"C24-8_×_25WS024_L02_R2.fastq.gz","name":"C24-8_×_25WS024_L02_R2.fastq.gz","size":4080218931,"state":"failed"},
   {"path":"交付说明.pdf","name":"交付说明.pdf","size":1258291,"state":"pending"}],
 "default_selected":["C24-8_×_25WS024_L02_R1.fastq.gz","C24-8_×_25WS024_L02_R2.fastq.gz"],
 "progress":{"total_bytes":8406100336,"done_bytes":4203050168,"speed":1048576,"percent":50}}"#;

/// `enqueue` 的回执：**部分成功**（3 个加进去、1 个被拒）——
/// 全成功那条会让"逐条拒绝理由"变成真空断言（`download_targets.rs` 的夹具头同一条理由）。
const ENQUEUE_PARTIAL_WIRE: &str = r#"{"added":[
   {"gid":"g1","path":"C24-8_×_25WS024_L02_R1.fastq.gz"},
   {"gid":"g2","path":"C24-8_×_25WS024_L02_R2.fastq.gz"},
   {"gid":"g3","path":"交付说明.pdf"}],
 "rejected":[{"path":"z.bin","reason":"清单里没有 \"z.bin\"（既不是文件，也不是任何文件的目录前缀）"}]}"#;

/// `enqueue` 的回执：`added` 与 `rejected` **都空**（D-2 那条真实回执）。
const ENQUEUE_NOTHING_WIRE: &str = r#"{"added":[],"rejected":[]}"#;

/// 内核原文里的那句 `path_not_found`（读某一层失败时显示的就是它）。
const PATH_NOT_FOUND_MESSAGE: &str = "清单里没有目录 \"01.RawData\"";

/// 传输列表那六行的**内核原文**（`transfer_list` 的 `items`）。
///
/// ⚠️ 六行是**挑出来覆盖判据**的，不是随手凑的（每一行守着一格）：
///    | gid | 覆盖 |
///    |---|---|
///    | `g-active`  | `Active` 档：能暂停、能移除，**不能重试** |
///    | `g-paused`  | `Waiting` + `raw_status == "paused"` ⇒ **「已暂停」角标**（裁决 #88） |
///    | `g-queued`  | `Waiting` + `"waiting"` ⇒ **同档但角标必须是关的**（对照） |
///    | `g-error`   | `Error` 档 + **多行** `errorMessage`（换行是刻意的：原文要**全文**展开）|
///    | `g-done`   | `Complete` 档：`completed == total` ⇒ 进度条满格、百分比 100% |
///    | `g-removed` | `Removed` 档 + `path: null` ⇒ **一个动作都不给**、标题是「（未知路径）」|
///
/// ⚠️ `path` 那一格**只有 `g-removed` 是 `null`**：它在呈现层就是"没有可用的路径"
///    （`TransferReveal::manifest_path` 把 `null` 与空串一起收成 `None`），
///    于是那一行既没有 `reveal` 可点、标题也落到 `TransferRow::UNKNOWN_PATH` 上。
const TRANSFER_ITEMS_WIRE: &str = r#"[
  {"gid":"g-active","total":4509715660,"completed":1200000000,"speed":1048576,"conns":4,
   "state":"active","raw_status":"active","error_message":"",
   "path":"01.RawData/C24-8_×_25WS024_L01_R1.fastq.gz"},
  {"gid":"g-paused","total":4402341478,"completed":900000000,"speed":0,"conns":2,
   "state":"waiting","raw_status":"paused","error_message":"",
   "path":"01.RawData/C24-8_×_25WS024_L01_R2.fastq.gz"},
  {"gid":"g-queued","total":4187593113,"completed":0,"speed":0,"conns":2,
   "state":"waiting","raw_status":"waiting","error_message":"",
   "path":"01.RawData/C24-8_×_25WS024_L02_R1.fastq.gz"},
  {"gid":"g-error","total":4080218931,"completed":168000000,"speed":0,"conns":2,
   "state":"error","raw_status":"error",
   "error_message":"连接超时（第 3 次重试）\n原因：Connection reset by peer",
   "path":"01.RawData/QC 图.png"},
  {"gid":"g-done","total":1258291,"completed":1258291,"speed":0,"conns":1,
   "state":"complete","raw_status":"complete","error_message":"","path":"交付说明.pdf"},
  {"gid":"g-removed","total":1048576,"completed":0,"speed":0,"conns":1,
   "state":"removed","raw_status":"removed","error_message":"","path":null}
]"#;

/// 快照那一刻的 `global`（四个数**刻意互不相同**：串位必须被夹具里的断言抓住）。
const TRANSFER_GLOBAL: GlobalStat = GlobalStat {
    download_speed: 1572864,
    num_active: 1,
    num_waiting: 2,
    num_stopped: 3,
};

/// 下一拍的 `global`：**只有速度在走**（活动数一个都没变）——
/// 给"200 ms 一拍只改文本、不建节点"那条用例用。
///
/// ⚠️ 三个计数**逐字重复写了一遍**（没有用 `..TRANSFER_GLOBAL`）：结构体更新语法要求
///    基值可 `Copy`，而 `GlobalStat` 是 `protocol.rs` 里的协议镜像 ——
///    为了省三行字去动它（加 `Copy`）会把这次改动推到协议层上。
const TRANSFER_GLOBAL_NEXT: GlobalStat = GlobalStat {
    download_speed: 2097152,
    num_active: 1,
    num_waiting: 2,
    num_stopped: 3,
};

fn transfer_items(json: &str) -> Vec<TransferItem> {
    serde_json::from_str(json).expect("这是内核会发的线上 JSON，必须解得出")
}

/// 下一拍的那六行：**只有 `g-active` 的进度与速度在走**，其余五格一个字节都不变。
///
/// 🔴 这是"200 ms 一拍不许重建 DOM"那条用例的**载荷**：它必须与第一拍**结构完全相同**
///    （同一批 gid、同一批状态、同一句错误原文），只有数字不同 —— 否则那条用例
///    （"行容器的 `childList` 变更 = 0"）要么假红、要么变成一句没有判别力的话。
fn transfer_items_next(items: &[TransferItem]) -> Vec<TransferItem> {
    items
        .iter()
        .cloned()
        .map(|mut it| {
            if it.gid == "g-active" {
                it.completed = 2_100_000_000;
                it.speed = 2_097_152;
            }
            it
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 任务 13（换码面板 / 设置窗口）的夹具
// ---------------------------------------------------------------------------

/// **换码面板**要吃的那份历史：三条，**故意两种都有** ——
/// 一条带备注（标题显示备注、码**照样**显示）、一条没备注（标题回落到码）、
/// 一条是从**自定义**交付服务器加载的（点它要把 `base_url` 一起发出去）。
///
/// ⚠️ 时间戳是**手写的字面量**（不是 `BatchHistory::timestamp` 现算的）：
///    拿被测代码生成夹具 = 让夹具替变异体打掩护（`batch_history.rs` 的测试头同一条口径）。
///    排序（倒序）由 `History::new` 自己做 —— 这里乱序传入，顺带证明"顺序是历史的不变量"。
const HISTORY_WIRE: &str = r#"[
  {"code":"R.a1b2C3d4E5f6G7h8I9j","note":"客户张三 / 9月肿瘤数据","base_url":"",
   "last_used_at":"2026-09-18T09:12:00+08:00"},
  {"code":"R.9z8Y7x6W5v4U3t2S1r0","note":"","base_url":"http://delivery.example.com:8010",
   "last_used_at":"2026-09-17T16:30:00+08:00"},
  {"code":"R.ZZZZZZZZZZZZZZZZZZZZ","note":"   ","base_url":"",
   "last_used_at":"2026-09-16T08:00:00+08:00"}]"#;

/// 七项参数那一格的内核回执。
///
/// ⚠️ `min_split_size` **故意取一个不在 `hello` 候选集合里的值**（`21M`）：那是
///    `SettingsForm::min_split_size_options` 与 `min_split_size_note` **唯一有内容的**
///    那一支（手改过的 `settings.json` 就是这个形态）—— 只用"在集合里"那一种，
///    前端把当前值丢掉、回落到第一项的缺陷**不会有任何东西变红**。
///    其余六项取内核的默认值（`core/src/settings.rs` 的 `default_settings()`）。
fn settings_result() -> SettingsResult {
    SettingsResult {
        settings: Settings {
            parallel: 8,
            connections: 16,
            splits: 16,
            min_split_size: "21M".to_string(),
            limit_mbps: 0,
            max_tries: 3,
            retry_wait: 1,
        },
        last_code: CODE.to_string(),
    }
}

// ---------------------------------------------------------------------------
// R-5：**对位对照的用例表**（`presentation/` 里那几条"有判据、零生产调用者"的判据，
//       行为由前端（JS / CSS）承接 —— 见 `windows/scripts/check_presentation_mirrors.sh`）
// ---------------------------------------------------------------------------
//
// 🔴 **它们为什么在这里**：本代最严重那个缺陷（"切一下分区回来，攒的选择就被抹掉"）
//    的形状是"**判据在 Rust 里、实现被抄进 JS**，两边一分叉，谁都不会红"。
//    判据是：**同一组输入喂给两边，结论必须一致** —— 而"同一组输入"这句话只有把
//    用例表**从 Rust 倒出来**才成立（手抄一份 case 表就是同一个病的第二次）。
//
// ⚠️ 于是这份表与 `wire-fixtures.json` 的其余部分**同一条纪律**：
//    它是**生成物**，`check_wire_fixtures.sh` 会重新生成一份逐字节比 ——
//    想要改用例，改的是本文件，不是那份 JSON。
// ⚠️ 前端那一侧的回放在 `windows/scripts/frontend-stub/panels-harness.html`
//    （"R-5 对位差分"那一段），由 `check_panels_screen.sh` 跑。

/// 换码面板备注草稿的一个操作（三个提交点各一个 + 编辑本身）。
enum NoteOp {
    Edit(&'static str, &'static str),
    /// 在那一行的输入框里按回车。
    Enter(&'static str),
    /// 焦点离开那一行。
    Blur(&'static str),
    /// 关掉面板（`#sc-cancel` 那条路 —— 它走 `close()`，会先交草稿再拆 DOM）。
    Close,
}

/// 把一例跑一遍：**判决全部来自 [`NoteDrafts`]**，这里只是把三个提交点串起来。
///
/// ⚠️ 写成功之后把 `current` 更新成**刚写下去那一份**：真机上 `history_put` 回的是
///    写完之后那一份历史（面板拿它整块重画），stub 与真内核在这件事上同形。
///    少了这一步，"同一个草稿交两次"那一例会得到两份写入，而线上只会有一份 ——
///    对照就会**假红**。
fn run_note_case(rows: &[(&str, &str)], ops: &[NoteOp]) -> Vec<serde_json::Value> {
    let mut current: std::collections::BTreeMap<String, String> = rows
        .iter()
        .map(|(code, note)| ((*code).to_string(), (*note).to_string()))
        .collect();
    let mut drafts = NoteDrafts::new();
    let mut writes: Vec<serde_json::Value> = Vec::new();

    for op in ops {
        let outcome = match op {
            NoteOp::Edit(code, text) => {
                drafts = drafts.editing(text, code);
                continue;
            }
            NoteOp::Enter(code) | NoteOp::Blur(code) => {
                drafts.committing(code, current.get(*code).map(String::as_str))
            }
            NoteOp::Close => drafts.committing_all(|code| current.get(code).cloned()),
        };
        drafts = outcome.drafts;
        for write in outcome.writes {
            current.insert(write.code.clone(), write.note.clone());
            writes.push(json!({ "code": write.code, "note": write.note }));
        }
    }
    writes
}

/// 换码面板的备注草稿：`presentation::batch_history::NoteDrafts` ↔ `web/js/switchcode.js`。
///
/// ⚠️ **行只是面板的脚手架**（`title` / `time_text` 不参与判决）：判决那一半
///    （要不要写、写哪几条、按什么顺序）**全部**由 `NoteDrafts` 现算出来。
///    `note` 是真输入 —— 它正是 `committing(code, current)` 的 `current`。
fn note_draft_cases() -> serde_json::Value {
    const A: &str = "R.aaaaaaaaaaaaaaaaaaaa";
    const B: &str = "R.bbbbbbbbbbbbbbbbbbbb";
    const C: &str = "R.cccccccccccccccccccc";

    // (规则编号, 一句话，行（码 → 现值），操作)
    let table: &[(&str, &str, &[(&str, &str)], &[NoteOp])] = &[
        (
            "no-draft",
            "① 没有草稿 ⇒ 一个字节都不写（用户只是点了一下那一行）",
            &[(A, "旧备注")],
            &[NoteOp::Enter(A)],
        ),
        (
            "unchanged",
            "③ 与现值相同 ⇒ 不写（免得每次失焦都写一次盘）",
            &[(A, "旧备注")],
            &[NoteOp::Edit(A, "旧备注"), NoteOp::Enter(A)],
        ),
        (
            "write",
            "④ 有改动 ⇒ 把**用户敲的原文**交出去（回车那一个提交点）",
            &[(A, "旧备注")],
            &[NoteOp::Edit(A, "客户李四"), NoteOp::Enter(A)],
        ),
        (
            "after-write",
            "交完之后再把同一句交一次 ⇒ 不写（现值已经变成它了）",
            &[(A, "旧备注")],
            &[
                NoteOp::Edit(A, "客户李四"),
                NoteOp::Enter(A),
                NoteOp::Edit(A, "客户李四"),
                NoteOp::Enter(A),
            ],
        ),
        (
            "empty-is-a-draft",
            "用户把备注清空 ⇒ 那是**一个真的草稿**（`Some(\"\")`，不是「没碰过」）",
            &[(A, "旧备注")],
            &[NoteOp::Edit(A, ""), NoteOp::Enter(A)],
        ),
        (
            "blur",
            "失焦是**同一个判决**（第二个提交点，与回车不分叉）",
            &[(A, "旧备注")],
            &[NoteOp::Edit(A, "失焦那一句"), NoteOp::Blur(A)],
        ),
        (
            "close-order",
            "⑤ 关面板 ⇒ 把还挂着的**全部**交出去，**按码排序**（第三个提交点）",
            &[(B, "b 的旧备注"), (A, "a 的旧备注")],
            &[
                NoteOp::Edit(B, "b 的新备注"),
                NoteOp::Edit(A, "a 的新备注"),
                NoteOp::Close,
            ],
        ),
        (
            "close-partial",
            "⑤ 关面板 ⇒ 没草稿的、以及没改过的**都不写**（只有那一条交出去）",
            &[(A, "a 的旧备注"), (B, "b 的旧备注"), (C, "c 的旧备注")],
            &[
                NoteOp::Edit(A, "a 的新备注"),
                NoteOp::Edit(B, "b 的旧备注"),
                NoteOp::Close,
            ],
        ),
    ];

    let cases: Vec<serde_json::Value> = table
        .iter()
        .map(|(rule, name, rows, ops)| {
            // 面板要的三格（`title` / `time_text` / `base_url`）在这里是**脚手架**：
            // 标题回落到码（`BatchHistoryRow` 在没备注时就是这么做的），时间是常量。
            let wire_rows: Vec<serde_json::Value> = rows
                .iter()
                .map(|(code, note)| {
                    json!({
                        "code": code,
                        "title": if note.is_empty() { *code } else { *note },
                        "note": note,
                        "time_text": "09-18 09:12",
                        "base_url": "",
                    })
                })
                .collect();
            json!({
                "rule": rule,
                "name": name,
                "rows": wire_rows,
                "ops": ops.iter().map(|op| match op {
                    NoteOp::Edit(code, text) => json!({"op": "edit", "code": code, "text": text}),
                    NoteOp::Enter(code) => json!({"op": "enter", "code": code}),
                    NoteOp::Blur(code) => json!({"op": "blur", "code": code}),
                    NoteOp::Close => json!({"op": "close"}),
                }).collect::<Vec<_>>(),
                "writes": run_note_case(rows, ops),
            })
        })
        .collect();

    json!({
        // ⚠️ 判据**没覆盖到**的那一条，如实写在这里（不是"忘了"，是"到这个界面里到不了"）：
        "notCoveredThroughTheDom": [
            "② 这一行已经不在屏上了 ⇒ 丢掉草稿、不写：面板的行**只从它自己的载荷来**，\
             而载荷只在这几处变（打开时读一次、写成功后用回执那一份重画）——\
             \"草稿还挂着、那一行却没了\"在这个界面上**构造不出来**。\
             Rust 那一侧的用例仍然钉着它（`batch_history.rs` 的用例）。"
        ],
        "cases": cases,
    })
}

/// 参数面板的"有没有未保存的改动"：`presentation::settings_form::SettingsForm::matches`
/// ↔ `web/js/settings.js` 的 `hasUnsavedChanges()`。
///
/// 🔴 **这一格与上面那一格不同**：两句**文案**（`banner.{unsaved,clean}`）确实发出去了，
///    没发出去的是那句**判决** —— "此刻算不算有未保存的改动"由前端自己比，
///    而 `matches` 的判据在 Rust 里、没人调。
fn settings_match_cases() -> serde_json::Value {
    let base = settings_result().settings;

    let tweak = |f: &dyn Fn(&mut Settings)| {
        let mut s = base.clone();
        f(&mut s);
        s
    };

    // (规则编号, 一句话, **内核手里那一份**（= 载荷里的 `settings`）, 表里那一份)
    let table: &[(&str, &str, Settings, Settings)] = &[
        (
            "same",
            "表里与内核那一份逐字段相同 ⇒ 「与内核当前参数一致」",
            base.clone(),
            base.clone(),
        ),
        (
            "one-number-changed",
            "-j 从 8 改成 4 ⇒ 有未保存的改动",
            base.clone(),
            tweak(&|s| s.parallel = 4),
        ),
        (
            "number-touched-but-same",
            "把 -j 敲成同一个值 ⇒ **不算**改动（判据是\"值一不一样\"，不是\"碰过就算\"）",
            base.clone(),
            tweak(&|s| s.parallel = 8),
        ),
        (
            "enum-changed",
            "-k 换成集合里的另一项（`20M`）⇒ 有未保存的改动",
            base.clone(),
            tweak(&|s| s.min_split_size = "20M".to_string()),
        ),
        (
            "all-seven-differ",
            "七项全不一样 ⇒ 有未保存的改动",
            base.clone(),
            Settings {
                parallel: 1,
                connections: 1,
                splits: 1,
                min_split_size: "1M".to_string(),
                limit_mbps: 100,
                max_tries: 100,
                retry_wait: 60,
            },
        ),
        (
            "current-is-not-the-fixture-one",
            "载荷里那一份**不是**夹具初始的那一份（内核重启之后就是这样）\
             ⇒ 表里与它相同也算「一致」",
            Settings {
                parallel: 16,
                connections: 4,
                splits: 2,
                min_split_size: "5M".to_string(),
                limit_mbps: 250,
                max_tries: 7,
                retry_wait: 30,
            },
            Settings {
                parallel: 16,
                connections: 4,
                splits: 2,
                min_split_size: "5M".to_string(),
                limit_mbps: 250,
                max_tries: 7,
                retry_wait: 30,
            },
        ),
    ];

    let cases: Vec<serde_json::Value> = table
        .iter()
        .map(|(rule, name, current, form)| {
            // ⚠️ 判决**现算**（`matches` 是那条判据本身），不是手写的字面量 ——
            //    手写的话，这条对照就变成了"把 Rust 的结论再抄一遍"。
            let dirty = !SettingsForm::new(form, Vec::new()).matches(Some(current));
            json!({
                "rule": rule,
                "name": name,
                "current": current,
                "form": form,
                "dirty": dirty,
            })
        })
        .collect();

    json!({
        // ⚠️ 面板上七格与七个字段的**对应顺序**（前端那一侧要按它去填格子）。
        //    来源：`SettingsForm::PARAMETER_LABELS` 的顺序 = `Parameters` 的顺序
        //    = `protocol::Settings` 的字段声明顺序（三处今天一致）。
        //    ⚠️ 这份对照**判不到**"顺序被重排了"（重排之后多数用例的判决仍然相同）——
        //       `check_mirror_cases` 的自检与前端那几条标签断言只钉得住"标签逐字来自载荷"。
        //       如实记账在这里，别把它读成"顺序也被守着了"。
        "fieldOrder": ["parallel", "connections", "splits", "min_split_size",
                       "limit_mbps", "max_tries", "retry_wait"],
        "cases": cases,
    })
}

/// 握手的 `hello` 回执原文（`-k` 的候选集合**只在它里面**，见 `api::settings` 的模块头）。
///
/// ⚠️ 这里是**内核会发的那种线上 JSON 文本**（与 `ROOT_WIRE` 一类同源）：
///    真实的内核给 100 项（1M…100M），这里给了 7 项 —— 夹具要的是"集合来自内核"
///    这条**性质**（顺序照内核给的、不排序），不是那 100 项本身。
///    ⚠️ 故意**不含 `21M`**：上面那份 `settings` 的当前值就在集合外。
const HELLO_WIRE: &str =
    r#"{"protocol":1,"min_split_size_choices":["1M","2M","5M","10M","20M","50M","100M"]}"#;

/// **设置窗口**要吃的偏好：**已配置**（未配置那一档由 `preferencesGetDefault` 给）。
fn preferences() -> Preferences {
    Preferences::new("/Volumes/Data/交付")
}

/// 上面那份线上原文 → 一份**排好序的**历史（排序是 `History::new` 的不变量）。
fn a_history() -> History {
    let entries: Vec<HistoryEntry> =
        serde_json::from_str(HISTORY_WIRE).expect("这是历史文件的线上形状，必须解得出");
    History::new(entries)
}

fn rows(json: &str, parent: &str) -> Vec<BrowserRow> {
    let result: ListDirResult =
        serde_json::from_str(json).expect("这是内核会发的线上 JSON，必须解得出");
    BrowserRow::rows(&result.entries, parent)
}

fn tree() -> TreeResult {
    serde_json::from_str(TREE_WIRE).expect("这是内核会发的线上 JSON，必须解得出")
}

fn enqueued(json: &str) -> EnqueueResult {
    serde_json::from_str(json).expect("这是内核会发的线上 JSON，必须解得出")
}

/// `state()` 的载荷：引擎在跑 + 一批已加载。
fn a_view() -> SessionView {
    SessionView {
        engine: EngineState::Running,
        load: LoadState::Loaded(DeliveryInfo {
            code: CODE.to_string(),
            page_url: format!("https://download.benagen.com/{CODE}/index.html"),
            base_url: "https://download.benagen.com".to_string(),
            created_at: "2026-03-01T09:00:00+08:00".to_string(),
            expires_at: "2026-03-18T23:59:00+08:00".to_string(),
            expired: false,
            total_files: 128,
            total_bytes: 443_033_255_936,
            tree: TreeNode::Empty,
        }),
        last_error: None,
        fallback_notice: None,
        handshake_reply: None,
    }
}

/// `state()` 的载荷：**引擎不可用**（那条带「重试」的常驻提示行）——
/// 前端那套验收里"红三角横幅 + 重试"那一块就靠它，而它的文案是
/// `EngineBanner::of` 算的（不是手写的）。
fn a_down_view() -> SessionView {
    SessionView {
        engine: EngineState::Unavailable("内核进程已退出".to_string()),
        last_error: Some("上一次请求失败了".to_string()),
        ..a_view()
    }
}

/// `state()` 的载荷：**引擎正在连接内核**（`EngineState::Connecting`），
/// 而**当前那一批仍然生效**（`load` 还是 `Loaded`）。
///
/// 🔴 这一档是**任务 13 点名要验的那一条**："改下载目录会重启内核，而内核重启会
///    **重放当前批次**"（`commands.rs:restart_kernel` 的四步里第 ③ 步）。
///    界面上它的表现正是这一份载荷：
///      · `engine.text` = 「正在连接内核…」（Rust 算的，不是前端编的）；
///      · `load` **不掉**（还是 `Loaded` 同一批）⇒ 主区**仍然是文件页**，
///        而不是"换完目录批次没了"。
///    ⚠️ 少了这一档，前端那套验收就只能验"回执画出来了"，验不了
///       "重启期间用户的批次还在不在" —— 而那一条才是这一屏**不慌**的理由。
fn a_connecting_view() -> SessionView {
    SessionView {
        engine: EngineState::Connecting,
        ..a_view()
    }
}

/// `state()` 的载荷：**握手超时**那一档。
///
/// ⚠️ 它是**唯一**会给横幅补一句壳写的 `hint` 的分支（`EngineStatusPresentation::
///    is_handshake_timeout` 认的是"逐字等于 `HANDSHAKE_TIMEOUT_MESSAGE`"）——
///    前端那套验收里"横幅正文 + 补充说明 + 重试"三块同时在场就靠它。
///    那句常量**从 `presentation` 取**（不在这份夹具里抄一遍）：抄一遍的话，
///    改了那句话而这里不动，夹具就与真载荷对不上了。
fn a_timeout_view() -> SessionView {
    SessionView {
        engine: EngineState::Unavailable(
            shell_core::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE.to_string(),
        ),
        last_error: None,
        ..a_view()
    }
}

fn main() {
    // 输出路径：命令行第一个参数；不给就打到 stdout。
    let out_path = std::env::args().nth(1);

    // ---- 文件页那两屏 --------------------------------------------------------
    let root_bc = Breadcrumb::new("");
    let root_rows = rows(ROOT_WIRE, "");
    let sub_bc = Breadcrumb::new("01.RawData");
    let sub_rows = rows(SUB_WIRE, "01.RawData");

    // 整棵树那一支：**照 `commands.rs:whole_tree` 的三行组装**（见文件头那条记账）。
    let tree = tree();
    let code = Some(CODE);
    let progress = ProgressSummary::of(Some(&tree), code, code);
    let selected: BTreeSet<String> = tree.default_selected.iter().cloned().collect();
    let sizes = SelectionSummary::size_index(&tree.flat);
    // ⚠️ 判据要的"这一批的全部文件"取自 `flat` —— 与 `whole_tree` 同源同算法
    //    （那边也是 `BrowserSelection::all_files(&tree.flat)`；两条来源的等价性由
    //    `the_two_receipts_of_one_manifest_name_the_same_files` 钉着）。
    let all_paths = BrowserSelection::all_files(&tree.flat);
    let action = api::tree::DownloadAction {
        button_title: DownloadTargets::button_title(&selected),
        empty_selection_hint: if selected.is_empty() {
            Some(DownloadTargets::empty_selection_hint().to_string())
        } else {
            None
        },
        // ⚠️ 与 `commands.rs:whole_tree` **逐字同一处判据**（任务 17）：入参是"真正要发出去的
        //    那一串"（勾选面过一遍 `DownloadTargets::paths`）—— 这批只有两个文件，
        //    所以这里就是 `None`。**不手写 `None`**：手写的话，"载荷里那一格是不是判据给的"
        //    在这份夹具上不可观测（判据改了夹具照样绿）。
        blocked_reason: DownloadTargets::blocked_reason(&DownloadTargets::paths(&selected, &all_paths))
            .map(str::to_string),
    };

    // ---- 任务 17：**按不下去**那一档（超预算的勾选面）----------------------------
    //
    // ⚠️ **为什么要自己造一份 20 万条的勾选面**：真机上"按不下去"只在**勾选面大到发不出去**
    //    时出现，而上面那份夹具树只有两个文件 —— 拿它算出来的 `blocked_reason` 永远是 `None`
    //    ⇒ 这一档**永远没有对象**，前端那条"禁用 + 把那句话摆出来"的验收也就没有载荷可吃
    //    （做没做都验不出来，R-61 那个形态）。所以照真机的量级造一份
    //    （与 `download_targets.rs` 的 `an_over_budget_set` 同一个数、同一串形状）。
    // ⚠️ **它只有两格会进 JSON**（`selection.count_text` / `size_text`）：那 20 万条路径
    //    本身**不进载荷**（那正是这条判据存在的理由），夹具的体积不受它影响。
    // ⚠️ `blocked_reason` **不手写**：走与 `commands.rs:whole_tree` 逐字同一处判据。
    let blocked_paths: BTreeSet<String> = (0..200_000)
        .map(|i| format!("dir{i}/file-with-a-fairly-long-name-{i}.bin"))
        .collect();
    let blocked_flat: Vec<FlatEntry> = blocked_paths
        .iter()
        .map(|path| FlatEntry {
            path: path.clone(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            // 每项 1 MiB：只是让这份样张的「合计」读起来像真的（判据不看它，看的是**项数**）。
            size: 1 << 20,
            state: FileState::Pending,
        })
        .collect();
    let blocked_sizes = SelectionSummary::size_index(&blocked_flat);
    let blocked_action = api::tree::DownloadAction {
        button_title: DownloadTargets::button_title(&blocked_paths),
        // 勾选面非空 ⇒ 这句「明说」不在场（与真机上那一档逐字相同）。
        empty_selection_hint: None,
        blocked_reason: DownloadTargets::blocked_reason(&DownloadTargets::paths(
            &blocked_paths,
            &all_paths,
        ))
        .map(str::to_string),
    };

    // ---- 读不到某一层 --------------------------------------------------------
    // `DirLoadFailure::of` 只认**结构化错误码**，所以这里从一个真的 `ClientError` 造。
    let failure = shell_core::presentation::breadcrumb::DirLoadFailure::of(
        &shell_core::client::ClientError::Kernel {
            code: shell_core::protocol::codes::PATH_NOT_FOUND.to_string(),
            message: PATH_NOT_FOUND_MESSAGE.to_string(),
        },
        "01.RawData",
    );

    // ---- 传输列表（任务 11）--------------------------------------------------
    // ⚠️ 这几份的组装**照 `commands.rs:transfers` 的三行**（同上面 `whole_tree` 那条记账）：
    //    `TransferRow::new` 逐条过 → `TransferGlobalSummary::of(&global)` →
    //    `TransferListEmpty::of(&engine)`。
    let transfer_items = transfer_items(TRANSFER_ITEMS_WIRE);
    let transfer_rows: Vec<TransferRow> = transfer_items.iter().map(TransferRow::new).collect();
    let transfer_rows_next: Vec<TransferRow> = transfer_items_next(&transfer_items)
        .iter()
        .map(TransferRow::new)
        .collect();
    // 「移除了一行之后的下一拍」：**结构真的变了一次** —— 给那条用例的**对照组**
    //（"行容器的 childList 变更 = 0" 必须能被证伪）。
    let transfer_rows_after: Vec<TransferRow> = transfer_items
        .iter()
        .filter(|it| it.gid != "g-done")
        .map(TransferRow::new)
        .collect();
    let transfer_global = TransferGlobalSummary::of(&TRANSFER_GLOBAL);
    let transfer_global_next = TransferGlobalSummary::of(&TRANSFER_GLOBAL_NEXT);
    // 空态那句话由 `TransferListEmpty::of` 给：引擎在跑 ⇒ 「没有正在传输的任务」；
    // 引擎不可用 ⇒ **`None`**（"这一屏不说话"，横幅已在说同一句）。
    let empty_running = TransferListEmpty::of(&EngineState::Running);
    let empty_engine_down =
        TransferListEmpty::of(&EngineState::Unavailable("引擎不可用".to_string()));

    let fixtures = json!({
        // `state()`：引擎在跑、一批已加载（`load.summary.code` 就是面包屑根那一格的字）。
        "stateLoaded": api::state::state(&a_view(), None),
        // `state()`：引擎不可用（常驻提示行那条 + 那颗「重试」）。
        "stateEngineDown": api::state::state(&a_down_view(), None),
        // `state()`：握手超时（横幅正文 + 壳写的补充说明 + 那颗「重试」都在场）。
        "stateHandshakeTimeout": api::state::state(&a_timeout_view(), None),
        // `state()`：**引擎正在连接内核**，而当前那一批仍然生效 —— 改下载目录
        // 重启内核那一小段就是它（"重放当前批次"在界面上的全部表现）。
        "stateConnecting": api::state::state(&a_connecting_view(), None),
        // `tree()` 无 path：进度 + 勾选摘要 + **内核给的默认勾选面** + 底栏那个动作。
        "treeWhole": api::tree::whole(
            &Breadcrumb::new(""),
            progress.as_ref(),
            &SelectionSummary::of(&selected, &sizes),
            &tree.default_selected,
            &action,
        ),
        // `tree()` 无 path：**同一支命令的第二档回执** —— 勾选面大到一次发不出去，
        // 于是 `action.blocked_reason` 是一个字符串（任务 17；前端的 `blocked` 场景吃它）。
        "treeWholeBlocked": api::tree::whole(
            &Breadcrumb::new(""),
            progress.as_ref(),
            &SelectionSummary::of(&blocked_paths, &blocked_sizes),
            // ⚠️ `selected` 是**内核给的默认面**（"勾选面该被播种成什么"），与"这一刻按哪个面
            //    渲染"是两件事 —— 这一档沿用同一批的那一份。
            &tree.default_selected,
            &blocked_action,
        ),
        // `tree("")`：根那一层。
        "treeLevelRoot": api::tree::level(&root_bc, &root_rows),
        // `tree("01.RawData")`：子目录那一层。
        "treeLevelSub": api::tree::level(&sub_bc, &sub_rows),
        // `tree("01.RawData")` 的失败支：面包屑跟着**退到的那一层**走。
        "treeLevelFailure": api::tree::level_failure(&Breadcrumb::new(&failure.path), &failure),
        // `enqueue`：部分成功（`added` 与 `rejected` 两边都有落点）。
        "enqueuePartial": api::enqueue::enqueue(&EnqueueFeedback::of(&enqueued(ENQUEUE_PARTIAL_WIRE))),
        // `enqueue`：一件都没加（`{"added":[],"rejected":[]}` 那条真实回执）。
        "enqueueNothing": api::enqueue::enqueue(&EnqueueFeedback::of(&enqueued(ENQUEUE_NOTHING_WIRE))),

        // ---- 传输列表（任务 11）---------------------------------------------
        // 快照：六行 + 全局摘要。⚠️ `empty_text` **照样在场**（命令层恒发它，
        // `api/transfers.rs` 的形状固定）—— 而"有行时它不该显示"正是那条判据。
        "transfersSnapshot": api::transfers::transfers(
            &transfer_rows,
            Some(&transfer_global),
            empty_running,
        ),
        // 下一拍：**同一批行、只有数字在走**（200 ms 那条用例的载荷）。
        "transfersTick": api::transfers::transfers(
            &transfer_rows_next,
            Some(&transfer_global_next),
            empty_running,
        ),
        // 「移除了一行之后」：结构真的变了一次（那条用例的**对照组**）。
        "transfersAfterRemove": api::transfers::transfers(
            &transfer_rows_after,
            Some(&transfer_global),
            empty_running,
        ),
        // 空态：`rows: []` + **`global: null`** + 「没有正在传输的任务」。
        "transfersEmpty": api::transfers::transfers(&[], None, empty_running),
        // 引擎不可用：`empty_text` 是 **`null`** —— 这一屏**什么都不说**。
        "transfersEngineDown": api::transfers::transfers(&[], None, empty_engine_down),

        // ---- 任务 13：换码面板 / 设置窗口 ------------------------------------
        // `history_get`：三条（带备注 / 没备注 / 从自定义服务器加载的）。
        "historyGet": api::history::payload(&a_history()),
        // `history_get`：**空历史 ⇒ 空数组**（不是 null、也不是一句文案）——
        // 判据是"调用方据这个空数组整段不渲染"（`BatchHistoryRow::rows` 的文档）。
        "historyEmpty": api::history::payload(&History::empty()),
        // `settings_get`：七项 + `-k` 的候选集合（`hello`）+ 两句说明 + 两种横幅 + 三份 help。
        "settingsGet": api::settings::payload(&settings_result(), Some(HELLO_WIRE)),
        // `preferences_get`：已配置 / 未配置（后者那一行是 Rust 给的"默认（…）"，不是空行）。
        "preferencesGet": api::preferences::current(&preferences()),
        "preferencesGetDefault": api::preferences::current(&Preferences::empty()),
        // `preferences_check`：**能用的**那一档（`check` 为 null = 可以用 ⇒ 前端弹确认）。
        // ⚠️ 确认文案取**真的那一段**（三条后果全在里面，`DownloadDirectory::confirmation`）。
        "preferencesCheckOk": api::preferences::plan(
            None,
            DownloadDirectory::confirmation(
                &preferences().download_dir,
                "/Volumes/Data/交付_新盘",
            ),
        ),
        // `preferences_check`：**不能用的**那一档（`check` 那句话由真判据算出来）。
        // ⚠️ `/nope` 是一个**保证不存在**的绝对路径（下面有断言守着：
        //    它一旦在这台机器上存在，生成器**当场红**，而不是悄悄写出一份会漂的夹具）。
        "preferencesCheckBad": api::preferences::plan(
            DownloadDirectory::check("/nope"),
            DownloadDirectory::confirmation(&preferences().download_dir, "/nope"),
        ),
        // `preferences_set` 的三支回执（成功 / 没动 / 失败）。
        // ⚠️ 失败那一句用 `api::preferences::restart_timed_out()`（**壳自己的那一句**，
        //    不是在这里编的）：编一句到夹具里 = 夹具与真载荷各说各话，
        //    而夹具正是前端那套验收的**唯一**期望值来源。
        "preferencesSetChanged": api::preferences::change(&DownloadDirChange::Changed {
            dir: "/Volumes/Data/交付_新盘".to_string(),
        }),
        "preferencesSetUnchanged": api::preferences::change(&DownloadDirChange::Unchanged {
            dir: preferences().download_dir,
        }),
        "preferencesSetFailed": api::preferences::change(&DownloadDirChange::Failed {
            message: api::preferences::restart_timed_out().to_string(),
        }),
        // `about`：正常那一档（短版本优先）+ 两个来源都空的那一档（兜底文案必须非空）。
        "about": api::about::about(&AboutInfo::version(Some("0.1.0"), Some("1"))),
        "aboutFallback": api::about::about(&AboutInfo::version(None, None)),
        // R-5：**对位对照的用例表**（判据由 Rust 现算，前端那一侧回放同一份用例）。
        "mirrorCases": {
            "noteDrafts": note_draft_cases(),
            "settingsMatches": settings_match_cases(),
        },
    });

    // ---- 倒之前先自检（夹具**退化**时当场红，而不是让前端那边少验几件事）--------
    let root = fixtures["treeLevelRoot"]["data"]["rows"]
        .as_array()
        .expect("treeLevelRoot.data.rows 必须是数组");
    assert_eq!(root.len(), 7, "根那一层应当是 7 行（两个目录 + 五个文件）：{fixtures}");
    for row in root {
        let label = row["state_label"].as_str().unwrap_or("");
        let color = row["state_color"].as_str().unwrap_or("");
        assert!(!label.is_empty(), "有一行的 state_label 是空的（状态列会整列空白）：{row}");
        assert!(!color.is_empty(), "有一行的 state_color 是空的（状态列会没有颜色）：{row}");
        assert!(
            ["Dir", "File"].contains(&row["kind"].as_str().unwrap_or("")),
            "行的 kind 不是前端认得的那两个形态之一：{row}"
        );
    }
    assert_eq!(
        fixtures["treeWhole"]["data"]["action"]["button_title"],
        json!("下载选中"),
        "底栏那颗按钮的文案不在夹具里：{fixtures}"
    );

    // ---- 传输列表那几份也要先自检（同上：夹具**退化**时当场红）---------------
    let trows = fixtures["transfersSnapshot"]["data"]["rows"]
        .as_array()
        .expect("transfersSnapshot.data.rows 必须是数组");
    assert_eq!(trows.len(), 6, "传输列表快照应当是 6 行：{fixtures}");
    // 五档状态**一档不漏**地出现在夹具里（少一档 = 那一档的呈现没人验）。
    let labels: std::collections::BTreeSet<&str> = trows
        .iter()
        .map(|r| r["state_label"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(labels.len(), 5, "五档状态必须各出现一次（否则有一档没被验到）：{labels:?}");
    // 「已暂停」角标：夹具里必须**同时**有一行开着、一行关着（否则那条断言没有对照）。
    let badges: Vec<bool> = trows
        .iter()
        .map(|r| r["shows_paused_badge"].as_bool().unwrap_or(false))
        .collect();
    assert!(badges.contains(&true) && badges.contains(&false), "角标要有两档：{badges:?}");
    // 多行错误原文：换行必须在夹具里（那是"全文展开"那条判据的载荷）。
    assert!(
        trows.iter().any(|r| r["error_text"].as_str().is_some_and(|t| t.contains('\n'))),
        "夹具里必须有一行是**多行**错误原文：{trows:?}"
    );
    // 一个动作都不给的那一行（`Removed`）：它是"空集 = 一个都不给"的载荷。
    assert!(
        trows.iter().any(|r| r["available_actions"].as_array().is_some_and(|a| a.is_empty())),
        "夹具里必须有一行**没有任何动作**：{trows:?}"
    );
    // 没有路径的那一行（`path: null` ⇒ 标题落到占位符、`reveal` 不可点）。
    assert!(
        trows.iter().any(|r| r["manifest_path"].is_null()),
        "夹具里必须有一行**没有清单路径**：{trows:?}"
    );
    // 🔴 两份"拍"的载荷**结构必须完全一样**（只有数字不同）——否则那条
    //    "childList 变更 = 0"的用例要么假红、要么没有判别力。
    let next = fixtures["transfersTick"]["data"]["rows"]
        .as_array()
        .expect("transfersTick.data.rows 必须是数组");
    assert_eq!(next.len(), trows.len(), "两拍的条数必须一样：{fixtures}");
    for (a, b) in trows.iter().zip(next.iter()) {
        for key in ["gid", "title", "manifest_path", "state_label", "state_icon_name"] {
            assert_eq!(a[key], b[key], "两拍的 {key} 必须逐字相同（这正是那条用例的前提）");
        }
        assert_eq!(a["error_text"], b["error_text"], "两拍的错误原文必须逐字相同");
        assert_eq!(
            a["available_actions"], b["available_actions"],
            "两拍的动作面必须逐字相同"
        );
    }
    // 而数字**必须真的在走**（否则那条用例证明不了"文本确实更新了"）。
    assert_ne!(
        trows[0]["progress_text"], next[0]["progress_text"],
        "第一行（g-active）的进度文本两拍之间必须不同：{fixtures}"
    );
    assert_ne!(
        trows[0]["progress_fraction"], next[0]["progress_fraction"],
        "第一行的 progress_fraction 两拍之间必须不同"
    );
    assert_ne!(
        fixtures["transfersSnapshot"]["data"]["global"]["speed_text"],
        fixtures["transfersTick"]["data"]["global"]["speed_text"],
        "两拍的全局速度必须不同"
    );
    // 对照组那一份：**少一行**（结构真的变了）。
    assert_eq!(
        fixtures["transfersAfterRemove"]["data"]["rows"]
            .as_array()
            .map(|r| r.len()),
        Some(5),
        "「移除之后」那一份应当少一行：{fixtures}"
    );
    // 空态两份：一句话在、一句话**不在**（`null`）。
    assert_eq!(
        fixtures["transfersEmpty"]["data"]["empty_text"],
        json!(TransferListEmpty::NOTHING_IN_FLIGHT),
        "空态那句话不在夹具里：{fixtures}"
    );
    assert_eq!(
        fixtures["transfersEmpty"]["data"]["global"],
        serde_json::Value::Null,
        "空态那一份的 global 必须是 null（不是 0/0/0）：{fixtures}"
    );
    assert_eq!(
        fixtures["transfersEngineDown"]["data"]["empty_text"],
        serde_json::Value::Null,
        "引擎不可用那一份**不许说话**（横幅已在说同一句）：{fixtures}"
    );

    // ---- 任务 13 的自检 --------------------------------------------------------
    // ⚠️ `/nope` 那条判据**会碰文件系统**（`DownloadDirectory::check` 要 stat 它）——
    //    它在这台机器上**存在**的话，"不可用目录"那一档就变成了另一个分支，
    //    而夹具会**悄悄**换一个形状（前端那边继续全绿）。⇒ 在这里当场拦住。
    assert!(
        fixtures["preferencesCheckBad"]["data"]["check"].is_string(),
        "夹具的「不可用目录」那一档取不到了（/nope 在这台机器上居然存在？）：{}",
        fixtures["preferencesCheckBad"]
    );
    // `preferencesCheckOk` 的另一半：`check` 为 `null` = **可以用**（前端据此才弹确认框）。
    assert!(
        fixtures["preferencesCheckOk"]["data"]["check"].is_null(),
        "「能用的目录」那一档的 check 必须是 null：{}",
        fixtures["preferencesCheckOk"]
    );
    // 历史：**倒序**（最近的在最前）+ 备注为空那条的标题**回落到码**。
    // ⚠️ 这两条是"换码面板那一屏读得懂 Rust 发的东西"的判据里最要紧的两条：
    //    顺序错了用户会点错批次；标题回落没了，一行没写备注的历史就是**一行空白**。
    let history_rows = fixtures["historyGet"]["data"]["rows"]
        .as_array()
        .expect("historyGet.data.rows 必须是数组");
    assert_eq!(history_rows.len(), 3, "历史夹具是三条：{fixtures}");
    assert_eq!(
        history_rows[0]["code"], json!(CODE),
        "最近用的那条必须在最前（`History::new` 的不变量）：{fixtures}"
    );
    assert_eq!(
        history_rows[0]["title"],
        json!("客户张三 / 9月肿瘤数据"),
        "有备注 ⇒ 标题是备注：{fixtures}"
    );
    assert_eq!(
        history_rows[2]["title"], history_rows[2]["code"],
        "没备注（只有空白也算没写）⇒ 标题回落到码：{fixtures}"
    );
    assert!(
        history_rows[1]["base_url"]
            .as_str()
            .map(|u| !u.is_empty())
            .unwrap_or(false),
        "中间那条是从自定义交付服务器加载的（点它要把 base_url 一起发出去）：{fixtures}"
    );
    // `-k`：当前值**不在**候选集合里 ⇒ 它被追加到末尾，且给出一句说明。
    let options = fixtures["settingsGet"]["data"]["min_split_size_options"]
        .as_array()
        .expect("min_split_size_options 必须是数组");
    assert_eq!(
        options.last().and_then(|v| v.as_str()),
        Some("21M"),
        "不在集合里的当前值必须被追加到**末尾**（否则控件会画成空白或第一项）：{fixtures}"
    );
    assert!(
        fixtures["settingsGet"]["data"]["min_split_size_note"].is_string(),
        "不在集合里 ⇒ 必须有一句说明（不能只让控件落在集合外而不说）：{fixtures}"
    );
    // 兜底的版本号**绝不返回空串**（空串 = 关于窗口里少一行，谁都不会发现）。
    assert!(
        fixtures["aboutFallback"]["data"]["version"]
            .as_str()
            .map(|v| !v.is_empty())
            .unwrap_or(false),
        "版本号的兜底文案不得为空串：{fixtures}"
    );

    // ---- R-5 的自检：对位用例表**不许退化** --------------------------------------
    //
    // ⚠️ 为什么这几条断言在这里：那张表是**前端的期望值来源**，它少一条用例，
    //    前端那边就**少对一条规则** —— 而少掉的那一条**不会让任何东西变红**
    //    （"没验"与"验过了"在输出里长得一样）。这正是本仓库反复栽的形状，
    //    所以在**倒之前**把"这张表还是完整的那一张"当场判掉。
    let note_meta = &fixtures["mirrorCases"]["noteDrafts"];
    let note_cases = note_meta["cases"]
        .as_array()
        .expect("mirrorCases.noteDrafts.cases 必须是数组");
    // 逐条规则**点名**（少一条就红：按规则名而不是条数判，条数对了但换了内容也看得出来）。
    let want_rules = [
        "no-draft",
        "unchanged",
        "write",
        "after-write",
        "empty-is-a-draft",
        "blur",
        "close-order",
        "close-partial",
    ];
    for rule in want_rules {
        assert!(
            note_cases.iter().any(|c| c["rule"] == json!(rule)),
            "对位用例里少了规则 `{rule}`：{note_meta}"
        );
    }
    // 🔴 表里**至少有几条真的写出去**：整张表可以是"什么都不写"的恒真表 ——
    //    而那会让前端那一段回放**全绿**（不做的那一半永远不会红）。
    let written: usize = note_cases
        .iter()
        .map(|c| c["writes"].as_array().map_or(0, |w| w.len()))
        .sum();
    assert!(written >= 4, "对位用例表里写出去的条目太少（{written} 条）：{note_meta}");
    // 「关面板 ⇒ 全部交出去、按码排序」那一例：**两条、按码升序**（顺序是判据的一部分）。
    let close_order = note_cases
        .iter()
        .find(|c| c["rule"] == json!("close-order"))
        .expect("close-order 在表里");
    let codes: Vec<&str> = close_order["writes"]
        .as_array()
        .expect("writes 是数组")
        .iter()
        .map(|w| w["code"].as_str().expect("code 是字符串"))
        .collect();
    assert_eq!(codes.len(), 2, "close-order 应当写出两条：{close_order}");
    assert!(
        codes[0] < codes[1],
        "关面板那一次必须**按码升序**交出去（顺序是可观察的判决之一）：{close_order}"
    );
    // 「清空」那一例：草稿是空串**也要交出去**（`Some(\"\")` 与"没碰过"是两件事）。
    let empty_case = note_cases
        .iter()
        .find(|c| c["rule"] == json!("empty-is-a-draft"))
        .expect("empty-is-a-draft 在表里");
    assert_eq!(
        empty_case["writes"][0]["note"],
        json!(""),
        "清空备注是一次**真的**写入（note 是空串，不是「不写」）：{empty_case}"
    );

    let settings_meta = &fixtures["mirrorCases"]["settingsMatches"];
    let settings_cases = settings_meta["cases"]
        .as_array()
        .expect("mirrorCases.settingsMatches.cases 必须是数组");
    // 两向都要有（只有"脏"或只有"干净"的话，一句恒 `true` / 恒 `false` 的实现也能全绿）。
    assert!(
        settings_cases.iter().any(|c| c["dirty"] == json!(true)),
        "对位用例里必须有一条是**有改动**：{settings_meta}"
    );
    assert!(
        settings_cases.iter().any(|c| c["dirty"] == json!(false)),
        "对位用例里必须有一条是**没改动**：{settings_meta}"
    );
    // 🔴 面板上七格与七个字段的顺序：**第 4 项是枚举**（前端按这份顺序填格子，
    //    而它成立的前提是 `PARAMETER_LABELS` 的顺序 = `Settings` 的字段顺序）。
    assert!(
        SettingsForm::parameters()[3].min.is_none(),
        "第 4 项应当是没有数值区间的枚举（`-k`）——`fieldOrder` 那份顺序就建立在它上面：{}",
        settings_meta["fieldOrder"]
    );
    assert_eq!(
        settings_meta["fieldOrder"].as_array().map(Vec::len),
        Some(7),
        "fieldOrder 必须是七项：{settings_meta}"
    );

    // ⚠️ 这两个数**数出来**，不写死：写死过一次（"5 个端点"），而它当时已经是 9 个 ——
    //    那种过时的话术与"夹具过期"是同一个病（读者的第一手信息是错的）。
    let endpoints = fixtures.as_object().map(|m| m.len()).unwrap_or(0);

    let text = serde_json::to_string_pretty(&fixtures).expect("夹具是个 JSON 对象，序列化不会失败");
    match out_path {
        Some(path) => {
            std::fs::write(&path, format!("{text}\n"))
                .unwrap_or_else(|e| panic!("写不出 {path}：{e}"));
            eprintln!(
                "==> 夹具已写出：{path}（{} 字节；{endpoints} 个端点 / 根那一层 {} 行）",
                text.len() + 1,
                root.len()
            );
        }
        None => println!("{text}"),
    }
}
