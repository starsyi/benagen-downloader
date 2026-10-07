//! 「**导出诊断日志**」那一次动作的回执与那两句壳自己写的话（规格 §2.5 / §2.7）。
//!
//! ## 🔴 为什么它必须住在 `shell-core`（而不是命令层里）能写
//!
//! 三种回执里有两种**没有内核原文可登**：取消（那件事内核根本不知道）与写盘失败
//! （内核没被碰过）。按规格 §3.2 的同一条纪律（R-24：命令层一个字都不许自己写），
//! 它们只能由壳给 —— 落地方式与 `api::preferences` 那几句逐字同款。
//!
//! ## ⚠️ 回执形状**与改设置那两条同形**（前端只有一条渲染路径）
//!
//! `headline` / `path_detail` / `notice_text` / `is_failure` 四格逐字沿用
//! [`crate::api::preferences::change`] 的那一组键（布局事故的修法：**标题里绝不含路径**，
//! 路径单独一行）。多出来的第五格 `privacy_note` 是规格 §2.7 要求的**第二处明说**
//! ——它**只在成功那一档有值**（没导出东西的时候没有什么可提醒的）。

use serde_json::{json, Value};

use crate::api::envelope;

/// 导出成功之后，界面上必须出现的那句话（规格 §2.7 的第 2 处明说）。
///
/// 🔴 **它是一条显式接受的风险，不是一个"已经处理好了"的保证**：日志里可能有下载地址
/// 片段（引擎的原始报错会原样带上它），而**没有**做自动过滤（理由见规格 §2.7：
/// 做一个不可靠的过滤器比不做更糟 —— 它会让人以为已经安全了）。
/// ⇒ 于是把它**说出来**，由用户自己决定要不要先打开看一眼。
///
/// ⚠️ 措辞按字体子集的覆盖判据挑过字（`test.sh` 第 0.5 步：每一个非 ASCII 字都必须在
///    内嵌子集里，否则判据变红）。
pub const PRIVACY_NOTE: &str = "这份日志可能包含下载地址片段，发送前你可以自己打开看一眼。";

/// **导出成功**：回执（标题 + 那个文件夹）+ 上面那句提醒。
///
/// ⚠️ `folder` 只出现在 `path_detail` 那一格（**标题里一个字都不许有路径** ——
///    那是一次真实布局事故的修法，见 `api::preferences::receipt` 的文档）。
pub fn export_done(folder: &str) -> Value {
    envelope::ok(json!({
        "headline": "诊断日志已导出",
        "path_detail": folder,
        "notice_text": format!("诊断日志已导出到 {folder}"),
        "is_failure": false,
        "privacy_note": PRIVACY_NOTE,
    }))
}

/// **用户取消了「选择文件夹」**：什么都没发生，而且**这不是失败**。
///
/// ⚠️ 与 `pick_directory` 那条同一条口径（取消一个对话框什么都不该发生）——
///    差别只在这里**要说一句**：用户点的是「导出」，界面一动不动会与"卡住了"分不开
///    （约束 4 明禁的静默失效）。所以回执在，只是它是"没有导出"，不是错误。
pub fn export_cancelled() -> Value {
    envelope::ok(json!({
        "headline": "没有导出（你取消了选择文件夹）",
        "path_detail": Value::Null,
        "notice_text": "没有导出（你取消了选择文件夹）",
        "is_failure": false,
        "privacy_note": Value::Null,
    }))
}

/// **导出失败**（建不出目录 / 拷不进去 / 写不了说明文件）：**原文照登**。
///
/// ⚠️ 那句话是 [`crate::export::folder_failure_text`] 拼好的（点名了哪个文件夹、系统原话、
///    下一步做什么 —— W-2），本函数**一个字都不加工**（同 `DownloadDirChange::Failed`
///    的"照登"口径）。
pub fn export_failed(why: &str) -> Value {
    envelope::ok(json!({
        "headline": why,
        "path_detail": Value::Null,
        "notice_text": why,
        "is_failure": true,
        "privacy_note": Value::Null,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ **三档回执发的是同一组键**（前端只有一条渲染路径）。
    ///
    /// 判别力：某一档少发一格 ⇒ 前端读到一个 `undefined`，那条行会安静地短一块
    /// （而不会有任何东西变红）。
    #[test]
    fn all_three_receipts_carry_the_same_keys() {
        let keys = |v: &Value| {
            let mut k: Vec<String> = v["data"]
                .as_object()
                .expect("回执的 data 一定是个对象")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        };
        let done = keys(&export_done("D:\\导出\\诊断日志-20260101-000000"));
        assert_eq!(keys(&export_cancelled()), done);
        assert_eq!(keys(&export_failed("导出没有完成")), done);
        assert_eq!(done.len(), 5, "四格沿用改设置那两条的形状 + 一格隐私提醒");
    }

    /// 🔴 **成功那一档必须带上那句提醒**（规格 §2.7 的第 2 处明说）。
    ///
    /// 判别力：把 `privacy_note` 从 `export_done` 里删掉 ⇒ 这一条红 —— 而真机上那是
    /// **用户拿着一个包含下载地址片段的文件夹、却没有任何人告诉过他**（那正是规格
    /// §2.7 拒绝做自动过滤之后，唯一还剩下的那道防线）。
    #[test]
    fn a_successful_export_carries_the_privacy_note() {
        let v = export_done("D:\\导出\\诊断日志-20260101-000000");
        assert_eq!(v["data"]["privacy_note"], json!(PRIVACY_NOTE));
        assert!(PRIVACY_NOTE.contains("下载地址"), "那句话要把风险说清：{PRIVACY_NOTE}");
        assert!(PRIVACY_NOTE.contains("自己打开看一眼"), "要给出用户能做的事：{PRIVACY_NOTE}");
        // ⚠️ 标题里**不许有路径**（布局事故的修法）——路径在 `path_detail` 那一格。
        assert_eq!(v["data"]["headline"], json!("诊断日志已导出"));
        assert!(
            !v["data"]["headline"].as_str().unwrap_or_default().contains('\\'),
            "标题里混进了路径：{v}"
        );
        assert_eq!(v["data"]["path_detail"], json!("D:\\导出\\诊断日志-20260101-000000"));
        assert_eq!(v["data"]["is_failure"], json!(false));
    }

    /// ⚠️ **取消不是失败**（与 `pick_directory` 同一条口径），而且要说得出话。
    ///
    /// 判别力：把取消做成失败信封 ⇒ 常驻提示行会为一次"用户自己按了取消"亮一条红的；
    /// 而把回执整个去掉 ⇒ 用户点了「导出」之后界面一动不动（与"卡住了"分不开）。
    #[test]
    fn cancelling_is_not_a_failure_but_it_still_says_something() {
        let v = export_cancelled();
        assert_eq!(v["ok"], json!(true), "取消走的是**成功**信封（它不是错误）：{v}");
        assert_eq!(v["data"]["is_failure"], json!(false));
        assert_eq!(v["data"]["path_detail"], Value::Null, "没有文件夹可说");
        assert_eq!(v["data"]["privacy_note"], Value::Null, "没导出东西，没有什么可提醒的");
        assert!(
            v["data"]["headline"].as_str().unwrap_or_default().contains("没有导出"),
            "要说清「什么都没发生」，否则与卡住分不开：{v}"
        );
    }

    /// **失败那一档照登原文**（不加工、不换一句更客气的话），并且 `is_failure` 翻面。
    #[test]
    fn a_failed_export_reports_the_reason_verbatim() {
        let why = crate::export::folder_failure_text("D:\\只读\\诊断日志-20260101-000000", "拒绝访问 (os error 5)");
        let v = export_failed(&why);
        assert_eq!(v["data"]["headline"], json!(why), "原文逐字，不加工");
        assert_eq!(v["data"]["notice_text"], json!(why));
        assert_eq!(v["data"]["is_failure"], json!(true), "失败要翻面（图标与颜色靠它）");
        assert_eq!(v["data"]["privacy_note"], Value::Null);
    }
}
