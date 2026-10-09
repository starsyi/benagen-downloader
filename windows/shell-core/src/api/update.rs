//! `/api/update`：**更新提示**那条提示条的载荷（规格 §4）。
//!
//! ## ⚠️ 它是一份**纯透传**（与 `enqueue` / `verify` 同口径）
//!
//! 内核的 `update_status` 回的本来就是"给前端看的那一份"（`op_update_status` 的
//! `json!`：`enabled` / `checking` / `current` / `latest` / `has_newer` / `url` /
//! `checked_at`），而**链接与版本号都来自内核**（约束 3：壳不改写内核的话）。
//! ⇒ 本函数**不拼 URL、不拼文案、不改写任何一个字**，只把它套进信封。
//!
//! ## 🔴 判据只有 `has_newer`（R20）
//!
//! 内核的 `url` 在 `has_newer == false` 时**照样可能非空**（`last_seen` 解析得出三段数字
//! 时就会带一条指向**已装版本**的链接）⇒ 前端**不许**按"`url` 非空"渲染，
//! 判据必须落在 `has_newer`。那是 `files.js:paintUpdate` 的事，本层只如实透传这两格
//! （透传的意义就是：判据在哪一层、由哪一层说了算，**不在这里替他裁**）。

use serde_json::Value;

use crate::api::envelope;

/// 更新状态：`data` 就是内核回的那一份，**不包一层**（与 `enqueue` / `verify` 同口径）。
///
/// ⚠️ **收 `&Value` 而不是 `&UpdateStatus`**：内核这一条**没有**在 `protocol.rs` 里
///    的镜像类型（它不在七个既有端点里、也不在任务 8 的十个里 —— 它是本波次随更新检查
///    新增的第三类）。刻意**不新造**一个 `protocol::UpdateStatus`：那会是一份"给内核形状
///    造第二个真相源"的类型，而前端要的键名（`has_newer` / `checked_at` 这些 snake_case）
///    与内核 `json!` 里逐字相同 ⇒ 直接透传即可（少一个会漂移的镜像）。
///
/// ⚠️ **它没有任何失败分支**：内核的 `op_update_status` **任何取不到的情形都回成功**
///    （`has_newer:false`）—— 所以"这一条不会给界面冒出错误"这件事是内核那一侧的判据
///    （见 `op_update_status` 的文档）。本函数连一个 `Err` 都没有。
///
/// ⚠️ **`envelope::ok(v)` 直接吃 `&Value`**：`Value` 实现了 `Serialize`，所以
///    `{"ok":true,"data":<内核那一份>}` 逐字成立，**不必先 `to_wire`**（`to_wire`
///    是给"界面值类型"用的那一步，这里没有界面值类型要转）。
pub fn update_status(kernel: &Value) -> Value {
    envelope::ok(kernel)
}

// ===========================================================================
// 「去下载」那一次动作的回执（R15）—— **壳自己写的**
// ===========================================================================
//
// ⚠️ 与上面那个 `update_status` **不是同一件事**：那一条是内核回什么的透传，而这一段是
//    **壳自己的动作**（打开系统浏览器）回什么。它**没有内核原文可登**，所以那两句人话住在
//    这里（R-24：命令层一个字都不许自己写）。三档各自成一句的理由见 [`opened`] /
//    [`refused`] / [`open_failed`]。

/// **拒绝**那条 URL 时的一句话（不在白名单里 ⇒ 一次系统调用都不发）。
///
/// ⚠️ 它不是"内核报的错"：这是**壳自己**的第二道锁在说话（规格 §1：写死之后，
///    即使接口被篡改，客户被带到的仍然是那个官方仓库页面）。判据在
///    `shell_win::openurl::is_allowed`，本常量只负责"拒绝时说一句人话"。
pub const REFUSED: &str = "这条链接不是官方发布页的地址，已拒绝打开。";

/// 一次「去下载」成功了（已经交给系统浏览器）。
///
/// ⚠️ 与 `load` 的 `{status:"loading"}` 同口径：**状态令牌**，不是显示文案。
///    这一条动作的成功**没有别的话可说**（浏览器已经在开了），前端只拿它当"发出去了"。
pub fn opened() -> Value {
    envelope::ok(serde_json::json!({ "status": "opened" }))
}

/// 白名单放行了、但**系统那一半没打开**（原样带上原因）。
///
/// ⚠️ 那句话点名了**是哪一步**（起不了默认浏览器）+ **可执行的补救**（手动去官方页），
///    与 `NO_KERNEL` / `NO_VERIFY_SUMMARY` 同一条 W-2 口径。
pub fn open_failed(why: &str) -> Value {
    envelope::err(&format!("打开浏览器失败：{why}。补救：手动打开官方发布页。"))
}

/// 一条不在白名单里的 URL ⇒ **拒绝**（走失败信封，把 [`REFUSED`] 交出去）。
pub fn refused() -> Value {
    envelope::err(REFUSED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 载荷形状：`data` 就是内核回的那一份，**不包一层**（与 `enqueue`/`verify` 同口径）。
    #[test]
    fn the_payload_is_the_kernel_value_itself() {
        let v = update_status(&json!({
            "enabled": true, "checking": false, "current": "0.2.4",
            "latest": "0.2.5", "has_newer": true,
            "url": "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/x.exe",
            "checked_at": 42
        }));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["has_newer"], json!(true));
        assert!(v["data"].get("update").is_none(), "不该再包一层");
    }

    /// 🔴 **提示文案与链接都是内核给的，壳不自己拼**（约束 3：壳不改写内核的话）。
    ///
    /// 判别力：把 `url` 那一格丢掉、或让壳自己拼一个 —— 这一条红。
    #[test]
    fn the_shell_does_not_rebuild_the_url() {
        let url = "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/x.exe";
        let v = update_status(&json!({ "has_newer": true, "url": url, "latest": "0.2.5", "current": "0.2.4" }));
        assert_eq!(v["data"]["url"], json!(url));
        assert_eq!(v["data"]["latest"], json!("0.2.5"));
    }

    /// 🔴 **`has_newer:false` 而 `url` 非空那一份照原样透传**（R20 的**内核侧**事实）。
    ///
    /// 内核在"已装的版本被 `last_seen` 解析出来"时**照样**会给一条指向**已装版本**的
    /// `url`。壳**不许**在这里把它抹成 `null`（那是替内核改它的话），也**不许**据此判定
    /// "有新版" —— 判据只有 `has_newer`，而它由前端读（`files.js`）。
    /// 这一条钉的是"透传"这半边：`has_newer` 与 `url` **两格都原样在**，前端才分得清。
    ///
    /// 判别力：让本函数"顺手"在 `has_newer == false` 时把 `url` 清成 `null` ⇒ 这一条红。
    #[test]
    fn a_stale_url_is_passed_through_untouched() {
        let url = "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.4/x.exe";
        let v = update_status(&json!({ "has_newer": false, "url": url, "latest": "0.2.4", "current": "0.2.4" }));
        assert_eq!(v["data"]["has_newer"], json!(false));
        assert_eq!(v["data"]["url"], json!(url), "内核的话逐字透传，壳不替它裁");
    }

    /// 成功信封的键只有那两个（`data` / `ok`），与其余载荷同形。
    #[test]
    fn the_envelope_is_the_usual_two_keys() {
        let v = update_status(&json!({ "has_newer": false }));
        assert_eq!(v["ok"], json!(true));
        assert!(v.get("data").is_some());
        assert!(v.get("error").is_none());
    }

    /// `opened()` 是**成功**信封、载荷是一个状态令牌（不是一句显示文案）。
    #[test]
    fn opened_is_a_success_envelope_with_a_status_token() {
        let v = opened();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["status"], json!("opened"));
    }

    /// 🔴 **拒绝走失败信封**，且那句话点名了"是壳自己的锁在说话"（不是内核报错）。
    ///
    /// 判别力：把 `refused()` 写成 `ok(...)` ⇒ 前端（若哪天接了这条）会把它当数据渲染；
    /// 把 [`REFUSED`] 删成一句空话 ⇒ `contains` 那两条红。
    #[test]
    fn refused_is_a_failure_envelope_with_the_reason() {
        assert!(
            REFUSED.contains("官方") || REFUSED.contains("发布页"),
            "这句拒绝要说清它拦的是什么（官方发布页之外的一律不放行）：{REFUSED}"
        );
        let v = refused();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"]["message"], json!(REFUSED), "原文逐字、不加工");
        assert!(v.get("data").is_none(), "失败正文里不该有 data");
    }

    /// `open_failed(&why)` 把系统原文带进那句话里（点名是哪一步 + 补救）。
    #[test]
    fn open_failed_keeps_the_system_wording_and_names_the_next_step() {
        let v = open_failed("ShellExecuteW 返回 2（不超过 32 都是错误码）");
        assert_eq!(v["ok"], json!(false));
        let msg = v["error"]["message"].as_str().expect("失败信封里有 message");
        assert!(msg.contains("ShellExecuteW 返回 2"), "系统原文要逐字带上：{msg}");
        assert!(msg.contains("补救"), "要给一句可执行的下一步：{msg}");
    }
}
