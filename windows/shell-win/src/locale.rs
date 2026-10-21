//! 「操作系统界面语言」—— 握手时交给内核，供「跟随系统语言」解析。
//!
//! 为什么要它（`core/src/protocol.rs::HelloParams::system_locale` 的同一段话）：
//! 内核只看 `LC_ALL/LC_MESSAGES/LANG`，而**图形壳里这些变量通常是空的**，空串按英文
//! 兜底 —— 中文 Windows 上首启就成了英文界面。
//!
//! ⚠️ **不引新依赖、不加 windows-sys feature**（`shell-win/Cargo.toml` 的纪律：其余任何
//!    新 feature 一律不许）。`GetUserDefaultUILanguage` 住在 **kernel32**，导入表里本来
//!    就有它，所以这里直接声明 FFI 调它 —— 只多一个「按语言 ID 归类」的纯函数。
//!
//! 归类口径与内核 `i18n::detect` 一致：**主语言是中文 ⇒ `zh-CN`，其余 ⇒ `en-US`**。
//! （内核只认中文/英文两档，把 `ja-JP` 报成 `en-US` 是有意的：界面只有这两门语言。）

/// 主语言 ID（LANGID 的低 10 位）。
const LANG_CHINESE: u16 = 0x04;

/// 把 LANGID 归成内核认得的 BCP-47 形态。
///
/// 单独抽成纯函数是为了**能在非 Windows 机器上测**：真机那一句 FFI 调用在测试机上
/// 走不到（`cfg(windows)`），而归类规则是这条链路上唯一有分支的地方。
pub fn primary_locale(langid: u16) -> &'static str {
    if langid & 0x03FF == LANG_CHINESE {
        "zh-CN"
    } else {
        "en-US"
    }
}

/// 系统界面语言。Windows 上问系统；其它平台（测试机 / 交叉编译检查）读环境变量。
pub fn system_locale() -> String {
    #[cfg(windows)]
    {
        // SAFETY: `GetUserDefaultUILanguage` 无参数、无所有权转移，返回 LANGID。
        let langid = unsafe { GetUserDefaultUILanguage() };
        return primary_locale(langid).to_string();
    }
    #[cfg(not(windows))]
    {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(key) {
                if !v.trim().is_empty() {
                    return v;
                }
            }
        }
        "en-US".to_string()
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    /// `kernel32!GetUserDefaultUILanguage`（系统默认界面语言的 LANGID）。
    fn GetUserDefaultUILanguage() -> u16;
}

/// **启动期**的界面语言（握手落定之前）—— 消息框、托盘、窗口标题都用它。
///
/// ⚠️ 这是唯一一处把「系统 locale」翻成 `Lang` 的地方：`main` / `bootstrap` /
///    `single_instance` 只调它，不各自判字符串（各处自己判 = 同一个真相多个来源）。
pub fn startup_lang() -> shell_core::i18n::Lang {
    lang_for_locale(&system_locale())
}

/// locale 串 → 界面语言。**主语言是中文 ⇒ `Zh`，其余 ⇒ `En`**（与
/// [`primary_locale`] 及内核 `i18n::detect` 同一条口径）。
pub fn lang_for_locale(locale: &str) -> shell_core::i18n::Lang {
    let lower = locale.trim().to_ascii_lowercase();
    if lower.starts_with("zh") {
        shell_core::i18n::Lang::Zh
    } else {
        shell_core::i18n::Lang::En
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_primary_language_maps_to_zh() {
        // 简体中文 0x0804、繁体 0x0404、香港 0x0C04 —— 主语言都是 0x04。
        assert_eq!(primary_locale(0x0804), "zh-CN");
        assert_eq!(primary_locale(0x0404), "zh-CN");
        assert_eq!(primary_locale(0x0C04), "zh-CN");
    }

    #[test]
    fn everything_else_falls_back_to_english() {
        // 英文 0x0409、日文 0x0411、法文 0x040C —— 界面只有两门语言，一律英文。
        assert_eq!(primary_locale(0x0409), "en-US");
        assert_eq!(primary_locale(0x0411), "en-US");
        assert_eq!(primary_locale(0x040C), "en-US");
    }

    #[test]
    fn startup_language_follows_the_system_locale() {
        use shell_core::i18n::Lang;
        assert_eq!(lang_for_locale("zh-CN"), Lang::Zh);
        assert_eq!(lang_for_locale("zh_CN.UTF-8"), Lang::Zh);
        assert_eq!(lang_for_locale("  zh-Hans  "), Lang::Zh);
        assert_eq!(lang_for_locale("en-US"), Lang::En);
        assert_eq!(lang_for_locale("ja-JP"), Lang::En, "界面只有两门语言：其余一律英文");
        assert_eq!(lang_for_locale(""), Lang::En, "空串按英文兜底（绝不 panic）");
    }

    #[test]
    fn a_locale_is_always_produced() {
        // 非 Windows 上读环境变量；这台测试机可能什么都没有 —— 那就必须是 `en-US`
        // （**绝不允许空串**：空串交给内核会走"环境变量兜底"那条路，等于白交）。
        assert!(!system_locale().trim().is_empty());
    }
}
