//! `/api/enqueue` 那一格的载荷（搬自 `shell-win/src/server/routes.rs:332` 的 `enqueue`）。
//!
//! ## ⚠️ 本模块是**补搬**的（R-13，计划裁定）
//!
//! 规格 §5.3 的散文点了 `enqueue` 的名（"把 `state_json` / `tree_json` / `enqueue` /
//! `transfers` / `verify` / `note_failure` / `engine_wire` / `load_wire` 搬进
//! `shell-core/src/api/`"），而计划任务 2 的表里**没有它** —— 两者对不上。
//! 任务 2 按表办（"留在第二代路由里，等它自己的那一波任务"），于是它一直留在
//! 第二代的路由文件里；那份文件随第二代传输层一起删掉之后，**它是唯一一处没有家的载荷
//! 拼装**。本任务（任务 7）把它补成第六个纯函数，形状与其余五个**逐字一致**：
//! 入参是数据（一份 `EnqueueFeedback`），出参是套好信封的 `Value`。
//!
//! ⚠️ **`EnqueueFeedback::of` 那一侧一个字都没搬**：`added` / `rejected` 怎么成句
//!    （含"没有需要下载的文件"那句壳自己写的话、约束 4 的"两边都要有落点"）
//!    全在 `presentation/download_targets.rs` 里（那是它的家，有单测）。
//!    本函数只做"把算好的界面值装进信封"——**不在这里重算**，也不在这里加工。
//!
//! ## ⚠️ 第二代那个函数里**没有搬过来**的东西：对内核的调用 + 四档 400
//!
//! 上游那个 `enqueue(session, req)` 一进门就问 `session.client()`、把正文解成
//! `paths`、**逐档判**"这次请求读懂了没有"、再调 `kernel::enqueue`、失败时 `note_failure`。
//! 那些全是**命令层**的事（它才拿得到 `Session`、内核连接与 Tauri 给的强类型实参）。
//!
//! ⚠️ 那四档 400（正文不是 JSON / 没有 `paths` 键 / `paths` 不是数组 / 数组里有非字符串）
//!    **不是被丢掉，是被形状取代了**：本代的 `invoke('enqueue', { paths: [...] })`
//!    的实参由 Tauri 按 `Vec<String>` 反序列化 —— 四种"没读懂"在**进命令层之前**
//!    就被 Tauri 自己挡下（JS 那一侧拿到一个 rejected promise），根本走不到
//!    `kernel::enqueue`。而"内核把空的 `paths` 当成下全部待下载"这条**约定**、
//!    以及"绝不把读不懂的请求静默升级成下全部"那条纪律，住在命令层与
//!    `controller` 的注释里（`shell-win/src/commands.rs`）—— 它在那边仍然是承重的。

use serde_json::Value;

use crate::api::{envelope, to_wire};
use crate::presentation::download_targets::EnqueueFeedback;

/// 加入下载任务的回执：`data` 就是那份 `EnqueueFeedback`（**不是**包一层 `{"feedback": …}`）。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:332` 的 `enqueue`（成功那一支）。
/// **对齐 macOS**：`Presentation/DownloadTargets.swift` 的 `EnqueueFeedback`
/// （**已移植**到 `presentation/download_targets.rs`）。
///
/// ⚠️ **`data` 就是那份反馈本身**（与 [`crate::api::verify::verify`] 同形）：
///    第二代那一条发的是 `envelope::ok(to_wire!(EnqueueFeedback::of(&result)))`，
///    中间**没有**再加一层键。多一层就是前端多一条读法，而形状变了**不会有东西变红**。
///
/// ⚠️ **失败那一档不经过本函数**：它是"这一次动作整个没成"，走
///    [`crate::api::failure`]（`CallFailure` → 信封），与其余五个端点同一条口径。
///    ⚠️ 于是 `EnqueueFeedback::failure` 那条路（原地显示内核原文、不切走）
///    在本代**没有调用点** —— 那是**有意的**，不是漏搬：它服务于"就地把错误摆在
///    下载按钮旁边"那个 egui/第二代视图的形状，而本代前端拿的是
///    `{"ok":false,"error":{"message":…}}`（原文逐字、同一个信封装着）。
///    它的家仍然在 `presentation/download_targets.rs`（那里的单测钉着它），
///    与 `EnqueueFeedback::of` 摆在一起 —— **不要因为"没人用"去删它**。
pub fn enqueue(fb: &EnqueueFeedback) -> Value {
    envelope::ok(to_wire(fb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::download_targets::Rejection;
    use crate::protocol::{AddedTask, EnqueueResult, RejectedTask};

    /// 一条**部分成功**的内核回执（不是全成功：全成功那条会让"逐条拒绝理由"变成真空断言）。
    fn a_result() -> EnqueueResult {
        EnqueueResult {
            added: vec![AddedTask {
                gid: "g1".to_string(),
                path: "01.RawData/a.fq.gz".to_string(),
            }],
            rejected: vec![RejectedTask {
                path: "01.RawData/b.fq.gz".to_string(),
                reason: "已经下载过了".to_string(),
            }],
        }
    }

    /// `data` **就是**那份反馈（不包一层 `feedback` 键：形状多一层，前端多一条读法）。
    #[test]
    fn the_feedback_is_the_data_itself() {
        let fb = EnqueueFeedback::of(&a_result());
        let v = enqueue(&fb);
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert!(v["data"].get("summary").is_some(), "data 该是那份回执：{v}");
        assert!(v["data"].get("feedback").is_none(), "不该再包一层：{v}");
    }

    /// ⚠️ **`added` 与 `rejected` 两边都要有落点**（约束 4：不得静默少交）。
    ///
    /// 判别力：把 `summary` 或 `rejections` 中的任何一个丢掉，这一条立刻红 ——
    /// 而真机上的表现是"已加入 N 个"旁边那几条拒绝理由不见了（用户以为全下上了）。
    #[test]
    fn both_the_added_count_and_every_rejection_reach_the_client() {
        let v = enqueue(&EnqueueFeedback::of(&a_result()));
        assert_eq!(v["data"]["summary"], serde_json::json!("已加入 1 个下载任务"), "{v}");
        let rejections = v["data"]["rejections"].as_array().expect("rejections 是个数组");
        assert_eq!(rejections.len(), 1, "逐条拒绝理由必须都在：{v}");
        assert_eq!(rejections[0]["path"], serde_json::json!("01.RawData/b.fq.gz"));
        // ⚠️ 路径与理由是**内核原文、逐字**（约束 3）：本层不加工、不截断。
        assert_eq!(rejections[0]["reason"], serde_json::json!("已经下载过了"));
        assert_eq!(v["data"]["switches_to_transfers"], serde_json::json!(true));
    }

    /// ⚠️ **"没有需要下载的文件"那句是壳自己写的**（内核只给了空数组，一个字都没说）——
    ///    它必须原样到达前端，不许在本层被换成一个空串或 `null`（那是约束 4 明禁的静默失效）。
    ///
    /// 判别力：把本函数改成"`added` 空就不发 `data`"，这一条立刻红。
    #[test]
    fn an_empty_added_list_still_says_so_in_words() {
        // ⚠️ 夹具是**空回执**：`added` 与 `rejected` 都空（D-2 那条真实回执）。
        let fb = EnqueueFeedback::of(&EnqueueResult { added: vec![], rejected: vec![] });
        let v = enqueue(&fb);
        assert_eq!(v["data"]["summary"], serde_json::json!("没有需要下载的文件"), "{v}");
        assert_eq!(v["data"]["rejections"], serde_json::json!([]));
        // 空 `added` **不切**分区（切过去只会看到一片空，还会把壳写的那句话吞掉）。
        assert_eq!(v["data"]["switches_to_transfers"], serde_json::json!(false));
    }

    /// 逐条拒绝理由那三个字段**恒在**（`path` / `reason` 是内核原文，`Rejection` 没有别的格）。
    ///
    /// ⚠️ 这一条是给**将来**的网：`Rejection` 加字段、或本层"顺手"改键名，
    ///    前端那一条渲染会以"某一栏是 undefined"收场（静默）。
    #[test]
    fn every_rejection_field_is_present() {
        let fb = EnqueueFeedback {
            summary: "已加入 0 个下载任务".to_string(),
            rejections: vec![Rejection {
                path: "p".to_string(),
                reason: "r".to_string(),
            }],
            switches_to_transfers: false,
        };
        let v = enqueue(&fb);
        let keys: Vec<&String> = v["data"]["rejections"][0]
            .as_object()
            .expect("一条拒绝理由是个对象")
            .keys()
            .collect();
        assert_eq!(keys, vec!["path", "reason"], "拒绝理由的格子是固定的：{v}");
    }
}
