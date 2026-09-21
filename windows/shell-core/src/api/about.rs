//! 「关于」窗口的载荷（任务 8 新增：`about` 命令）。
//!
//! 上游：`macos/Sources/BenagenDownloader/Views/AboutView.swift`（视图层，三件东西：
//! 横版 logo / 应用名 / 版本号）。**Rust 侧能写出断言的只有版本号那一件** ——
//! 回落规则在 [`AboutInfo`]（R-7 裁决："判据留、来源换"，来源是 `shell-win` 在编译期
//! 取好的两个值），本文件只负责把它装进信封。
//!
//! ## ⚠️ 为什么载荷里**只有 `version` 一格**（这一条是判据，不是漏做）
//!
//!   * **应用名**：它的家是**窗口标题**（`tauri.conf.json` 的 `app.windows[0].title`，
//!     `shell-win/src/main.rs` 的 `WINDOW_TITLE` 与 `build.rs` 的 `PRODUCT_NAME`
//!     都对着它，且有用例钉住三者逐字相同）。在这里再发一份 = 给同一句话造**第三个**
//!     真相源，而这一份**没有任何东西会盯着它**；前端那一侧要显示名字，读的应当是
//!     同一个来源（`windows/web/index.html` 的 `<title>` 已经是它，任务 9 建的）。
//!   * **logo**：R-6 已经把品牌资产判给"前端 + 构建脚本"（`BrandAssets` 有意不移植，
//!     理由写在 `presentation/about_info.rs` 的头注里）—— 它是 webview 的静态文件，
//!     Rust 侧没有可断言的逻辑。
//!
//! ⇒ 多一格不是"更周到"，而是**多一处没人守的复制**。这一条与 `api::state::engine_wire`
//!    那三格（引擎徽标）的取舍方向**不矛盾**：那三格是**算出来的呈现值**（前端算不出、
//!    算出来也必与 macOS 分叉），而名字与 logo 是**静态资产**（本来就不该由代码算）。

use serde_json::{json, Value};

use crate::api::envelope;

/// 关于窗口的载荷：`{"version": "…"}`。
///
/// ⚠️ `version` **必须**是 [`AboutInfo::version`] 的返回值 —— 它**绝不返回空串**
///    （两条回落都落空时是一句点名了"去哪查"的话）。少了那条判据，关于窗口里就是
///    **少一行**：不报错、不是空白块，谁都不会发现（而客户报障时被问到的第一个问题
///    恰恰是"你用的是哪个版本"）。
pub fn about(version: &str) -> Value {
    envelope::ok(json!({ "version": version }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::about_info::AboutInfo;

    /// 版本号原样发出去（**逐字**：它是给人复制去对账的）。
    #[test]
    fn the_version_goes_out_verbatim() {
        let v = about("0.1.0");
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["version"], json!("0.1.0"));

        // 兜底那一句也照发（它本身就是一句完整的话 —— 界面上前面**不加**"版本"两个字，
        // 否则会变成"版本 版本未知（…）"）。
        let fallback = AboutInfo::version(None, None);
        let v = about(&fallback);
        assert_eq!(v["data"]["version"], json!(fallback));
        assert!(
            !v["data"]["version"].as_str().unwrap_or_default().is_empty(),
            "版本号那一格绝不许是空串：{v}"
        );
    }

    /// ⚠️ **载荷里只有 version 一格**（多一格就是第二处真相源，见模块头）。
    ///
    /// 判别力：有人"顺手"加上 `name` / `logo` 时这一条会红 —— 那时要做的是先回答
    /// "那一格的来源与守卫在哪"，而不是把它发出去。
    #[test]
    fn the_payload_has_exactly_one_cell() {
        let v = about("0.1.0");
        let keys: Vec<&String> = v["data"].as_object().expect("载荷是个对象").keys().collect();
        assert_eq!(keys, vec!["version"], "关于窗口的载荷只有版本号：{v}");
    }
}
