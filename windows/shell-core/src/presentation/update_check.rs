//! 手动检测更新（设置里那颗「立即检测」按钮）的**判决与那几句回执**。
//!
//! ## 它为什么存在：规格 §5 留下的一个洞
//!
//! 内核的 `update_status` 在**「这次没查成」**与**「查成了、确实没有新版」**两种情形下
//! 长得**一模一样**（`has_newer:false` + `last_seen` 不变）。对那条**后台**提示条来说这是
//! 对的：客户没做任何动作，界面不该冒话（规格 §5"失败一律静默"）。
//! 但设置里那颗按钮**是客户点出来的**——客户问的是"有没有新版本"，而在断网时只按
//! `has_newer` 判就只剩一句**"已经是最新的"**：那是**界面在撒谎**。
//!
//! ⇒ 内核因此多了一格 `last_check_ok`（见 `core/src/kernel.rs` 的
//! `Kernel::update_last_check_ok` 的注释），本模块把它 + `has_newer` + 版本号判成**五档**，
//! 每档一句人话。**三档不许并成两档**：`Disabled` / `Unreachable` / `NoAnswer` 都不配说
//! "已经是最新的"。
//!
//! ## 判据在 Rust、行为在前端（本仓的既有纪律）
//!
//! 那五句是**给客户看的话**，所以它们住在这里（R-24：命令层与前端都不许自己写界面文案）
//! ——前端只按 `is_failure` 挑图标、把 `headline` 原样摆上去。
//! 与 [`settings_form`](crate::presentation::settings_form) /
//! [`resident_notice`](crate::presentation::resident_notice) 同一条纪律。
//!
//! ## ⚠️ 中文字面量的一个既有约束：**字必须在内嵌子集里**
//!
//! 壳的界面用一份内嵌字体子集渲染（`windows/assets/ui-subset.otf`），
//! 而 `windows/scripts/make_font_subset.sh --check` 会把**被扫源码里所有非注释的非 ASCII
//! 字符**都算进要求集（`check_design_fonts.py` 的 `required_codepoints`）。
//! ⇒ 本文件里新写的每一个字都必须是子集里**已有**的字，否则 `windows/scripts/test.sh`
//! 第 0.5 步会红（exit 3）。改这几句话之前先跑一次 `bash windows/scripts/make_font_subset.sh --check`。

use serde_json::Value;

/// 一次「立即检测」的结局（**五档**）。
///
/// ⚠️ 五档里只有 [`UpdateCheck::UpToDate`] 配说"已经是最新的"：那一档要求
/// `last_check_ok == true` **且** `has_newer != true` —— 两个条件缺一个都是**猜**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// 开关关着：内核**一道请求都不会发**（`run_update_check` 的第一道闸，规格 §5），
    /// 所以这一下**根本没查**。不许说成"没查成"，更不许说成"已经是最新的"。
    Disabled,
    /// 查成了，而且发布页上那个版本比当前这个新。
    Newer { latest: String, current: String },
    /// 查成了，已经是最新的（**只有这一档**配这么说话）。
    UpToDate { current: String },
    /// 没查成：网络/发布页不可达，或发布页上最新那条是预发布
    /// （`core/src/update.rs` 的 `fetch_latest` 口径）——两者都表示**这次没拿到一个
    /// 可比较的版本号**。
    Unreachable,
    /// 内核**没给** `last_check_ok` 这一格（旧内核 / 载荷缺格），或给了一份自相矛盾的
    /// 载荷（说"有新版"却拿不出 `latest`、拿不出 `current`）。
    /// **不许**当成查成了 —— 说不清就说不清。
    NoAnswer,
}

/// 从载荷里取一格**非空字符串**（空串与缺失是同一件事：都说不出口）。
fn text(status: &Value, key: &str) -> Option<String> {
    status
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl UpdateCheck {
    /// 判决：`update_status`（或 `update_check_now`）回的**那一份状态** → 五档之一。
    ///
    /// ⚠️ **判据顺序是承重的**：先看开关（关着 ⇒ 根本没查，说什么都错），再看
    /// "这次查成没有"，**最后**才轮到"有没有新版"。
    /// 把顺序换过来（先看 `has_newer`）会得到这样一份载荷的错判决：
    /// `{enabled:false, last_check_ok:true, has_newer:true, …}` —— 开关关着的时候
    /// 内核一次都没查，`has_newer` 是**上一次**（可能几天前）的旧结论。
    pub fn of(status: &Value) -> Self {
        // ① 开关关着 ⇒ 这一下没有查（内核的 `run_update_check` 第一道闸就返回了）。
        if status.get("enabled").and_then(Value::as_bool) == Some(false) {
            return UpdateCheck::Disabled;
        }

        let current = text(status, "current");

        // ② 这次查成没有 —— 这一格就是整个模块存在的理由。
        match status.get("last_check_ok").and_then(Value::as_bool) {
            Some(true) => {
                // ③ 查成了，才轮到"有没有新版"。
                let Some(current) = current else {
                    // 查成了却报不出"当前是哪个版本" ⇒ 这句话说不全，不许硬凑。
                    return UpdateCheck::NoAnswer;
                };
                if status.get("has_newer").and_then(Value::as_bool) == Some(true) {
                    match text(status, "latest") {
                        Some(latest) => UpdateCheck::Newer { latest, current },
                        // "有新版"却说不出是哪个版本 ⇒ 不猜（宁可不说话）。
                        None => UpdateCheck::NoAnswer,
                    }
                } else {
                    UpdateCheck::UpToDate { current }
                }
            }
            // ④ 没查成：网络/发布页不可达，或最新那条是预发布。
            Some(false) => UpdateCheck::Unreachable,
            // ⑤ 内核没给这一格：**不许**当成查成了（旧内核 / 协议缺格）。
            None => UpdateCheck::NoAnswer,
        }
    }

    /// **这条回执那一句话**（`api::update::checked` 把它装进 `headline`）。
    ///
    /// ⚠️ 全部是**长度固定**的短句（`Newer` / `UpToDate` 里只插版本号，而版本号是
    /// 三段数字、长度可控）—— 理由与 `DownloadDirChange::headline` 那条一样：
    /// 回执那一格的高度不许由外部文本决定。
    ///
    /// ⚠️ 失败那三档都点名了**是哪一步** + **可执行的补救**（W-2 口径）：
    /// 只报一句"没查成"而不说下一步，等于把客户扔在原地。
    pub fn headline(&self) -> String {
        match self {
            UpdateCheck::Disabled => {
                "自动检查更新是关着的，刚才没有去查。补救：打开上面那个开关，再点一次「立即检测」。"
                    .to_string()
            }
            UpdateCheck::Newer { latest, current } => {
                format!("有新版本 {latest}（当前 {current}）。")
            }
            UpdateCheck::UpToDate { current } => format!("已经是最新的（当前 {current}）。"),
            UpdateCheck::Unreachable => {
                "没查成：这次没连上发布页。补救：确认这台机器能上网，或者过一会儿再点一次。"
                    .to_string()
            }
            UpdateCheck::NoAnswer => {
                "没查到结果：内核回的这份检查状态不全。补救：重开一次客户端再试。".to_string()
            }
        }
    }

    /// 前端据此挑图标（`exclamationmark.triangle.fill` / `checkmark.circle.fill`）。
    ///
    /// ⚠️ **前两档也是"失败"**（`Disabled`：你这一下没查；`NoAnswer`：说不清）——
    /// 按"是不是错误"来分会让那句"刚才没有去查"配一个绿勾，读起来像"已经是最新的"。
    pub fn is_failure(&self) -> bool {
        match self {
            UpdateCheck::Disabled | UpdateCheck::Unreachable | UpdateCheck::NoAnswer => true,
            UpdateCheck::Newer { .. } | UpdateCheck::UpToDate { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 一份"查成了、没有新版"的载荷（后面几条测试在它上面改一格）。
    fn checked_ok() -> Value {
        json!({
            "enabled": true, "checking": false, "current": "0.2.7",
            "latest": "0.2.7", "has_newer": false, "url": null,
            "checked_at": 7, "last_check_ok": true
        })
    }

    /// 🔴 **本模块存在的理由**：没查成**不许**被说成"已经是最新的"。
    ///
    /// 判别力：把 `Some(false)` 那一支并进 `UpToDate`（或干脆只看 `has_newer`）⇒ 这一条红，
    /// 客户在断网时会读到一句假话。
    #[test]
    fn a_failed_check_is_never_reported_as_up_to_date() {
        let mut st = checked_ok();
        st["last_check_ok"] = json!(false);
        let check = UpdateCheck::of(&st);
        assert_eq!(check, UpdateCheck::Unreachable);
        assert!(
            !check.headline().contains("已经是最新"),
            "断网时一个字都不许说『已经是最新的』：{}",
            check.headline()
        );
        assert!(check.headline().contains("补救"), "要说下一步：{}", check.headline());
        assert!(check.is_failure());
    }

    /// 🔴 **内核没给这一格**（旧内核 / 协议缺格）⇒ 也不许当成查成了。
    ///
    /// 判别力：把 `None` 那一支并进 `UpToDate` ⇒ 红。`has_newer:false` 单独一格
    /// **不构成**"已经是最新的"的证据 —— 那正是本模块要修的那个洞。
    #[test]
    fn a_missing_cell_is_not_a_success() {
        let mut st = checked_ok();
        st.as_object_mut().unwrap().remove("last_check_ok");
        assert_eq!(UpdateCheck::of(&st), UpdateCheck::NoAnswer);
        assert!(UpdateCheck::of(&st).is_failure());
    }

    /// 拿不出 `latest` 却声称"有新版" ⇒ **不猜**（不许拼一句"有新版本 "）。
    #[test]
    fn a_contradictory_payload_is_not_guessed_at() {
        let mut st = checked_ok();
        st["has_newer"] = json!(true);
        st["latest"] = json!(null);
        assert_eq!(UpdateCheck::of(&st), UpdateCheck::NoAnswer);

        // 空串与缺失是同一件事：都说不出口。
        st["latest"] = json!("");
        assert_eq!(UpdateCheck::of(&st), UpdateCheck::NoAnswer);

        // 反向：说"查成了"却报不出"当前是哪个版本" ⇒ 同样不猜。
        let mut st2 = checked_ok();
        st2["current"] = json!(null);
        assert_eq!(UpdateCheck::of(&st2), UpdateCheck::NoAnswer);
    }

    /// 只有"查成了 + 没有新版"才配说那一句；版本号照登。
    #[test]
    fn only_a_finished_check_may_say_up_to_date() {
        let check = UpdateCheck::of(&checked_ok());
        assert_eq!(
            check,
            UpdateCheck::UpToDate {
                current: "0.2.7".to_string()
            }
        );
        assert!(check.headline().contains("0.2.7"), "{}", check.headline());
        assert!(!check.is_failure());
    }

    /// 有新版本：两个版本号都要在场（客户才知道"从哪个到哪个"）。
    #[test]
    fn a_newer_version_names_both_numbers() {
        let mut st = checked_ok();
        st["has_newer"] = json!(true);
        st["latest"] = json!("0.2.9");
        let check = UpdateCheck::of(&st);
        assert_eq!(
            check,
            UpdateCheck::Newer {
                latest: "0.2.9".to_string(),
                current: "0.2.7".to_string()
            }
        );
        let line = check.headline();
        assert!(line.contains("0.2.9") && line.contains("0.2.7"), "{line}");
        assert!(!check.is_failure());
    }

    /// 🔴 **开关关着的时候，`has_newer` 是旧结论** —— 判据顺序不许换。
    ///
    /// 判别力：先看 `has_newer` 再看 `enabled` ⇒ 这一条会说"有新版本 0.2.9"，
    /// 而内核在开关关着时**一次都没查**（那 0.2.9 是几天前的旧快照）。
    #[test]
    fn the_switch_is_judged_before_the_version_numbers() {
        let mut st = checked_ok();
        st["enabled"] = json!(false);
        st["has_newer"] = json!(true);
        st["latest"] = json!("0.2.9");
        let check = UpdateCheck::of(&st);
        assert_eq!(check, UpdateCheck::Disabled);
        assert!(!check.headline().contains("0.2.9"), "{}", check.headline());
        assert!(check.headline().contains("补救"), "{}", check.headline());
        assert!(check.is_failure(), "点了没查，不该配成功档的图标");
    }

    /// 五档**各有各的话**（并档、或某档留空，都在这里红）。
    #[test]
    fn every_branch_has_its_own_sentence() {
        let all = [
            UpdateCheck::Disabled,
            UpdateCheck::Newer {
                latest: "0.2.9".to_string(),
                current: "0.2.7".to_string(),
            },
            UpdateCheck::UpToDate {
                current: "0.2.7".to_string(),
            },
            UpdateCheck::Unreachable,
            UpdateCheck::NoAnswer,
        ];
        let lines: Vec<String> = all.iter().map(UpdateCheck::headline).collect();
        for line in &lines {
            assert!(!line.trim().is_empty(), "每一档都要有一句话");
        }
        for (i, a) in lines.iter().enumerate() {
            for b in lines.iter().skip(i + 1) {
                assert_ne!(a, b, "两档说了同一句话 ⇒ 并档了：{a}");
            }
        }
        // 失败那三档都要说下一步；两档好消息不该带"补救"（没有要补的东西）。
        assert!(all[0].headline().contains("补救"));
        assert!(all[3].headline().contains("补救"));
        assert!(all[4].headline().contains("补救"));
        assert!(!all[1].headline().contains("补救"));
        assert!(!all[2].headline().contains("补救"));
    }
}
