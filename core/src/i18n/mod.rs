//! 语言：谁跟谁说话用哪一套词。
//!
//! 规格 §2：**三端**（Windows 壳、macOS 壳、CLI）与内核共用同一份"语言"概念，
//! 只有两档产出——中文、英文。`Lang::System` 是**输入值**（"跟随系统"），
//! 不是产出值：任何要拿它去查文案的地方都必须先 [`Lang::resolve`]，拿到的
//! 永远是 `Zh` 或 `En`。这条规矩让"忘了解析"在类型上就写不出来（`resolve`
//! 的返回值仍含 `System`，但它不会返回它；`resolved()` 则是编译期的护栏）。
//!
//! 文案表**不在这里**：`core/src/i18n/` 随 T2 落地，本文件只放"选哪门语言"。

pub mod text;

use serde::{Deserialize, Serialize};

/// 界面语言。
///
/// `#[serde(rename_all = "lowercase")]` ⇒ 线上就是 `"system"` / `"zh"` / `"en"`，
/// 三端照抄这三个字符串，不另造枚举名（规格 §2.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// 跟随系统。**只有设置里能存这个值**。
    #[default]
    System,
    Zh,
    En,
}

impl Lang {
    /// 把设置值解析成真正要用的语言。
    ///
    /// * `Zh`/`En` ⇒ 原样返回（客户显式选过，压过系统）；
    /// * `System` ⇒ 看 `system_locale`：`zh` 开头给中文，**其余一律英文**
    ///   （规格 §2.1：英语是兜底，因为"看不懂的英文"强于"看不懂的中文"）。
    pub fn resolve(self, system_locale: &str) -> Lang {
        match self {
            Lang::Zh | Lang::En => self,
            Lang::System => detect(system_locale),
        }
    }

    /// 线上取值（与 `Serialize` 逐字一致）。给日志和夹具用。
    pub fn tag(self) -> &'static str {
        match self {
            Lang::System => "system",
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// 从一个语言标签解析（`hello` 的 `language` 参数、CLI 的 `--lang`）。
    ///
    /// 认 `system`/`zh`/`en` 三个词，**大小写不敏感**；也认 `zh-CN`、`zh_CN.UTF-8`
    /// 这类系统 locale 写法（CLI 直接吃 `LANG` 时用得上）。不认识 ⇒ `None`：
    /// 调用方决定是"当成没传"还是报参数错（`hello` 选前者，`--lang` 选后者）。
    pub fn parse(s: &str) -> Option<Lang> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "system" | "auto" => Some(Lang::System),
            "zh" => Some(Lang::Zh),
            "en" => Some(Lang::En),
            _ => {
                if s.is_empty() {
                    None
                } else if s.starts_with("zh") && s[2..].starts_with(['-', '_', '.']) {
                    Some(Lang::Zh)
                } else if s.starts_with("en") && s[2..].starts_with(['-', '_', '.']) {
                    Some(Lang::En)
                } else {
                    None
                }
            }
        }
    }
}

/// 系统 locale → 语言。判据只有一条：**语言部分是 `zh` 就算中文**，否则英文。
///
/// 认的写法：`zh`、`zh-Hans`、`zh_CN.UTF-8`、`EN-us`。空串 ⇒ 英文
/// （"什么都不知道"时跟着最广的默认走，规格 §2.1）。
pub fn detect(system_locale: &str) -> Lang {
    let s = system_locale.trim().to_ascii_lowercase();
    let primary = s.split(['-', '_', '.', '@']).next().unwrap_or("");
    if primary == "zh" {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// 本进程看到的系统 locale（CLI 用；壳会通过 `hello.system_locale` 自己交一份）。
///
/// 顺序：`LC_ALL` → `LC_MESSAGES` → `LANG` →（Windows）问系统 → 空串（[`Lang::En`]）。
///
/// ⚠️ 为什么最后要问系统：Windows **没有** `LANG`/`LC_ALL` 这套变量，只读环境变量时
///    中文 Windows 上的命令行工具会一路落到英文 —— 而 CLI 也一样要"默认跟系统"。
///    手法与 `procstat`/`free_bytes` 相同：`#[link]` + 裸 `extern "system"`，不引 crate、
///    不加 `windows-sys` feature（`kernel32` 本来就在导入表里）。
pub fn system_locale() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: 无参数、无所有权转移，返回 LANGID。
        let langid = unsafe { GetUserDefaultUILanguage() };
        return primary_locale(langid).to_string();
    }
    #[cfg(not(windows))]
    String::new()
}

/// 把 LANGID 归成 [`detect`] 认得的形态（主语言中文 ⇒ `zh-CN`，其余 ⇒ `en-US`）。
///
/// 抽成纯函数是为了**在非 Windows 机器上也能测**：这条链路上唯一有分支的就是归类规则。
pub fn primary_locale(langid: u16) -> &'static str {
    if langid & 0x03FF == 0x04 {
        "zh-CN"
    } else {
        "en-US"
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetUserDefaultUILanguage() -> u16;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判据：`zh` 开头给中文，其他给英文，空串给英文。
    /// LANGID 归类：中文三种变体都进中文界面，别的语言进英文界面。
    ///
    /// 这条钉的是 Windows CLI 的"默认跟系统"——`LANG` 在那台机器上根本不存在。
    #[test]
    fn the_system_language_id_maps_to_one_of_the_two_languages() {
        assert_eq!(primary_locale(0x0804), "zh-CN"); // 简体
        assert_eq!(primary_locale(0x0404), "zh-CN"); // 繁体
        assert_eq!(primary_locale(0x0C04), "zh-CN"); // 香港
        assert_eq!(primary_locale(0x0409), "en-US"); // 英文
        assert_eq!(primary_locale(0x0411), "en-US"); // 日文（界面只有两门语言）
        assert_eq!(detect(primary_locale(0x0804)), Lang::Zh);
        assert_eq!(detect(primary_locale(0x0411)), Lang::En);
    }

    #[test]
    fn only_zh_opens_the_chinese_door() {
        let cases = [
            ("zh-Hans", Lang::Zh),
            ("zh_CN.UTF-8", Lang::Zh),
            ("zh", Lang::Zh),
            ("ZH-hant", Lang::Zh),
            ("en-US", Lang::En),
            ("en", Lang::En),
            ("fr", Lang::En),
            ("", Lang::En),
            ("zhx", Lang::En), // `zhx` 不是中文，别被前缀骗了
        ];
        for (locale, want) in cases {
            assert_eq!(detect(locale), want, "locale = {locale}");
        }
    }

    /// 判据：客户显式选过就压过系统；`system` 才去看系统。
    #[test]
    fn an_explicit_choice_beats_the_system_locale() {
        assert_eq!(Lang::Zh.resolve("en-US"), Lang::Zh);
        assert_eq!(Lang::En.resolve("zh-Hans"), Lang::En);
        assert_eq!(Lang::System.resolve("zh-Hans"), Lang::Zh);
        assert_eq!(Lang::System.resolve("fr"), Lang::En);
    }

    /// 判据：`resolve` 的结果**永远不是** `System`（否则后面查文案会落空）。
    #[test]
    fn resolved_is_never_system() {
        for l in [Lang::System, Lang::Zh, Lang::En] {
            for locale in ["", "zh-Hans", "en-US", "fr"] {
                assert_ne!(l.resolve(locale), Lang::System, "{l:?} / {locale}");
            }
        }
    }

    #[test]
    fn parse_accepts_the_three_wire_words_and_locales() {
        assert_eq!(Lang::parse("system"), Some(Lang::System));
        assert_eq!(Lang::parse("auto"), Some(Lang::System));
        assert_eq!(Lang::parse("zh"), Some(Lang::Zh));
        assert_eq!(Lang::parse("EN"), Some(Lang::En));
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse("  en_US.UTF-8 "), Some(Lang::En));
        assert_eq!(Lang::parse("fr"), None);
        assert_eq!(Lang::parse(""), None);
    }

    /// 线上形状：三个词，不多不少。
    #[test]
    fn the_wire_shape_is_three_lowercase_words() {
        for (l, wire) in [(Lang::System, "\"system\""), (Lang::Zh, "\"zh\""), (Lang::En, "\"en\"")] {
            assert_eq!(serde_json::to_string(&l).unwrap(), wire);
            assert_eq!(serde_json::from_str::<Lang>(wire).unwrap(), l);
            assert_eq!(l.tag(), wire.trim_matches('"'));
        }
        // 默认值是"跟随系统"：老 settings.json 里没有这一格时要落到这里。
        assert_eq!(Lang::default(), Lang::System);
    }
}
