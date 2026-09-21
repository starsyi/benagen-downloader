//! about_info —— 「关于」窗口里那个**版本号**。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/AboutInfo.swift`。
//!
//! 为什么这段回落**不在调用方**：关于窗口里能写出断言的东西只有这一件 ——
//! 版本号从哪来、空了怎么办。调用方那一层能写的断言只有"画了个文本"，而版本号读空的
//! 后果是**那一行直接消失**：不报错、不是空白块，就是少一行，谁都不会发现
//! （而客户报障时被问到的第一个问题恰恰是"你用的是哪个版本"）。
//!
//! ---------------------------------------------------------------------------
//! ⚠️ **与上游的两处差异（控制者裁决，必须写下来）**
//! ---------------------------------------------------------------------------
//!
//! **① 判据留、来源换。** 上游读的是 `Info.plist` 的
//! `CFBundleShortVersionString` / `CFBundleVersion`，而 **Windows 上没有 plist**。
//! 保留的判据一字不改：短版本优先 → 回落构建号 → 兜底文案；空串 / 纯空白 / 缺失
//! **一律回落**；**绝不返回空串**。换掉的是来源 —— Windows 侧那一对值由
//! `shell-win/build.rs` 给（那个文件已经在生成版本资源：`BENAGEN_VERSION` 环境变量优先，
//! 否则用 `CARGO_PKG_VERSION`；见 `windows/shell-win/build.rs` 的
//! `link_manifest_and_version`）。
//!
//! ⚠️ 它们是**参数**而不是本模块的常量：build script 设的环境变量只对**它自己那个 crate**
//!    可见（`shell-core` 没有、也不该有 build script —— 它的定位是纯逻辑），
//!    所以那一对值由 `shell-win` 在编译期取好、调用 `about()` 时传进来。
//!    **不许伪造一个 plist**（那会在 Windows 上永远读空，然后"静默回落"到兜底文案）。
//!
//! **② 兜底文案改成 Windows 口径**：不再出现 plist 的键名（照着它去查只会查到一个
//! 不存在的地方），改说"构建时取不到版本号"并点名 `shell-win/build.rs` ——
//! 与上游同一条理由：**要把缺的是哪一处说出来**，看到这句话的人才知道该去查什么。
//!
//! ---------------------------------------------------------------------------
//! 🔴 **`BrandAssets` 有意不移植**（免得后来者以为漏做了）
//! ---------------------------------------------------------------------------
//!
//! 上游另有一个 `macos/Sources/BenagenCoreKit/Presentation/BrandAssets.swift`：它做的是
//! "从 **app bundle** 里按名字找资源 + 用 **ImageIO** 解码校验"。我们这边**没有 bundle、
//! 没有 ImageIO**：品牌资产是给 webview 的**静态文件**，归属是**前端 + 构建脚本**
//! （计划任务 14）。⇒ **Rust 侧不需要任何逻辑，`presentation/` 下不建
//! `brand_assets.rs`**。上游那三条名字与它们的扩展名仍然是**契约**，抄在这里供任务 14
//! 对齐构建脚本（与上游逐字一致）：
//!
//!   | 用途 | 名字 | 扩展名 | 上游常量的口径 |
//!   |---|---|---|---|
//!   | 应用图标 | `AppIcon` | `icns` | 图标名**不带扩展名**（那是 macOS 那个键的约定） |
//!   | 图形标（空态页） | `benagen-mark` | `png` | 透明底原件 |
//!   | 横版 logo（关于窗口） | `benagen-full-logo` | `png` | 含中英文全称 |
//!
//! ⚠️ 上游为"名字 → 扩展名"单独立了一份显式映射，理由是**实测**：
//!    `Bundle.url(forResource:withExtension: nil)` 在 macOS 上**匹配不到带扩展名的文件**。
//!    我们这边由前端用完整文件名引用，所以那份映射没有对应物 —— 但**"名字配错扩展名 =
//!    图不见了，且不报任何错"** 这条后果一字不变：任务 14 的构建脚本要**逐个做存在性自查**
//!    （上游的打包脚本就是这么办的：缺任何一个就让构建失败。判据的严重度要放在它能生效
//!    的地方 —— 打包时，不是用户机器上）。

// ---------------------------------------------------------------------------
// 版本号
// ---------------------------------------------------------------------------

/// 版本号的来源与回落。
///
/// 上游 `AboutInfo.swift` 的 `AboutInfo`（空 `enum` + `static` 命名空间；
/// 这里对位成空 `enum` + 固有 `impl`，同 `format.rs` 的 `ByteFormat`）。
pub enum AboutInfo {}

impl AboutInfo {
    /// 构建脚本**优先**读的那个环境变量名（`BENAGEN_VERSION=0.2.0 bash build_windows.sh`）。
    ///
    /// ⚠️ **改这里必须同时改 `windows/shell-win/build.rs`**（它 `rerun-if-env-changed`
    ///    的就是这个名字）：对不上的表现是版本号**静默**变成兜底文案。
    ///    对位的是上游的 `AboutInfo.shortVersionKey`（`CFBundleShortVersionString`）。
    pub const VERSION_ENV: &'static str = "BENAGEN_VERSION";

    /// 没有覆盖时用的那个（cargo 给 build script 的 `CARGO_PKG_VERSION`，即
    /// `windows/shell-win/Cargo.toml` 里的 `version`）。
    ///
    /// 对位的是上游的 `AboutInfo.buildVersionKey`（`CFBundleVersion`）。
    pub const PACKAGE_VERSION_ENV: &'static str = "CARGO_PKG_VERSION";

    /// 两条回落都落空时的那句话。**绝不返回空串**。
    ///
    /// 不写成"未知"两个字：要把**缺的是哪一处**说出来 —— 看到这句话的人
    /// （客户 / 分发方）才知道该去查什么。这一条与上游逐字同源，
    /// 只是把 plist 的键名换成了 Windows 侧那一处来源。
    ///
    /// 上游 `AboutInfo.unknownVersionText`。
    pub const UNKNOWN_VERSION_TEXT: &'static str =
        "版本未知（构建时取不到版本号：shell-win/build.rs 没交来 BENAGEN_VERSION 或 CARGO_PKG_VERSION）";

    /// 版本号：短版本 → 构建号 → [`Self::UNKNOWN_VERSION_TEXT`]。
    ///
    /// 上游 `AboutInfo.version(from:)`。
    ///
    /// ⚠️ 空白值（`""`、`" "`）与"没有这个来源"**同解**：它们在界面上渲染出来都是
    ///    一行空白，而这个兜底存在的理由正是"绝不出现一行看不见的东西"。
    ///
    /// ⚠️ **值不是字符串时也走回落**（上游那一条的判据）：本 crate 的这一对参数是
    ///    `Option<&str>`，"不是字符串"在类型上构造不出来，所以那一支整个消失 ——
    ///    见测试 `a_missing_value_falls_back_instead_of_being_invented` 上的形态偏离记账。
    ///    **不替打包脚本格式化**这条判据仍在：这里只 trim，绝不把 `123` 变成 `"123"`。
    pub fn version(short_version: Option<&str>, build_version: Option<&str>) -> String {
        // `str::trim()` 去掉的是 Unicode 的 `White_Space`，与上游
        // `whitespacesAndNewlines` 是同一个集合（同 `batch_history.rs` 那条注释）。
        text(short_version)
            .or_else(|| text(build_version))
            .unwrap_or_else(|| Self::UNKNOWN_VERSION_TEXT.to_string())
    }
}

/// 一个来源 → 可显示的版本号；不是非空字符串时回 `None`。
///
/// 上游 `AboutInfo.text(_:)`。
fn text(value: Option<&str>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ---------------------------------------------------------------------------
// 测试（先写测试：它们会先红，见任务 6 的步骤 2）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游 `macos/Tests/BenagenCoreKitTests/AboutInfoTests.swift`（逐条对位）。
    //!
    //! ⚠️ 这里**不碰真实的应用包 / 构建产物**：喂的是**手工造的**两个来源字符串，
    //!    所以三条回落规则（短版本 → 构建号 → 明确文案）都能被驱动。

    use super::AboutInfo;

    /// 上游 `thePlistKeysAreTheOnesThePackagerWrites`。
    #[test]
    fn the_version_sources_are_the_ones_the_build_script_reads() {
        // ⚠️ 形态换掉了（上游是 plist 的两个键名，Windows 上没有 plist）：
        //    这里钉的是 `shell-win/build.rs` 真正读的那两个环境变量名。
        //    写错的后果与上游逐字相同：版本号**静默**变成兜底文案（不会报错，只是显示"未知"）。
        assert_eq!(AboutInfo::VERSION_ENV, "BENAGEN_VERSION");
        assert_eq!(AboutInfo::PACKAGE_VERSION_ENV, "CARGO_PKG_VERSION");
    }

    /// 上游 `theShortVersionIsUsedWhenPresent`。
    #[test]
    fn the_short_version_is_used_when_present() {
        // 短版本是给人看的版本号（"1.2.3"），优先用它。
        assert_eq!(
            AboutInfo::version(Some("1.2.3"), Some("45")),
            "1.2.3"
        );
    }

    /// 上游 `anEmptyShortVersionFallsBackToTheBuildNumber`。
    #[test]
    fn an_empty_short_version_falls_back_to_the_build_number() {
        // 有来源、值是空串：与"没有这个来源"同解 —— 都是一行空白。
        assert_eq!(AboutInfo::version(Some(""), Some("45")), "45");
        // 只有空白字符同理（" " 渲染出来同样是什么都看不到）。
        assert_eq!(AboutInfo::version(Some(" \n "), Some("45")), "45");
        assert_eq!(AboutInfo::version(None, Some("45")), "45");
    }

    /// 上游 `aNonStringValueFallsBackInsteadOfBeingFormatted`（**形态偏离**）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游那一条喂的是"plist 里放了个数字"，断言它**回落**、
    ///    而不是被顺手格式化。Rust 这一侧的两个来源是 `Option<&str>`
    ///    —— "值不是字符串"在类型上**构造不出来**，所以那一支整个消失了
    ///    （同 `error_text.rs` 里"任意 `Error` 那条兜底路径消失"的记账）。
    ///    留下来的判据是它的另一半：**空/缺一律回落，绝不自己造一个值**。
    #[test]
    fn a_missing_value_falls_back_instead_of_being_invented() {
        assert_eq!(
            AboutInfo::version(None, None),
            AboutInfo::UNKNOWN_VERSION_TEXT,
            "两个来源都没有 ⇒ 兜底文案（**不是**空串、也不是 `0.0.0` 这种编出来的号）"
        );
        assert_eq!(
            AboutInfo::version(Some("   "), None),
            AboutInfo::UNKNOWN_VERSION_TEXT,
            "只有空白 = 没有"
        );
    }

    /// 上游 `theVersionIsNeverAnEmptyString`。
    #[test]
    fn the_version_is_never_an_empty_string() {
        // 两条回落都落空时**必须**是一句明确的话，绝不返回空串：
        // 空的版本行 = 关于窗口里少一行，谁都不会发现。
        for (short, build) in [
            (None, None),
            (Some(""), Some("   ")),
            (Some("  "), Some("")),
        ] {
            let text = AboutInfo::version(short, build);
            assert!(!text.is_empty(), "版本号不得为空串");
            assert!(
                text.contains("版本号"),
                "兜底文案要说清缺的是什么，用户/分发方才知道去查什么：{text}"
            );
            assert!(
                text.contains("build.rs"),
                "兜底文案要点名**去哪查**（Windows 侧那一处是构建脚本）：{text}"
            );
        }
    }

    /// 增量（上游没有）：兜底文案**不许**再出现 plist 的键名 —— 那是 macOS 的东西，
    /// Windows 上根本没有 plist，照着它去查只会查到一个不存在的地方。
    #[test]
    fn the_fallback_text_does_not_name_plist_keys() {
        assert!(!AboutInfo::UNKNOWN_VERSION_TEXT.contains("CFBundle"));
        assert!(!AboutInfo::UNKNOWN_VERSION_TEXT.contains("plist"));
    }

    /// 增量（上游没有）：值两边的空白要去掉 —— 否则界面上会出现一行
    /// " 1.2.3 "（首尾空格在等宽字体里看不太出来，但它会进标题栏与报障邮件）。
    #[test]
    fn the_version_is_trimmed() {
        assert_eq!(AboutInfo::version(Some("  1.2.3  "), None), "1.2.3");
        assert_eq!(AboutInfo::version(None, Some(" 45\n")), "45");
    }
}
