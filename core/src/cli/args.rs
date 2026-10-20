//! argv → [`Action`] 的纯函数解析。
//!
//! **不引 clap**（全局约束 7：不新增依赖）——argv 是一串已按空白切好的字符串，
//! 手写一遍循环比引一棵依赖树划算得多。

use std::path::PathBuf;

use crate::i18n::{text, Lang};
use crate::settings;

/// 帮助文本。默认值是 `settings::default_settings()` 的**实数**（8/16/16），
/// 不是"客户端认为合理的默认值"——两处各写一份就会漂移。
///
/// 文案本体在 `i18n::text`（`MsgKey::CliHelp`）：中文是 0.2.9 的原文，英文是 T8 新增。
pub fn help(lang: Lang) -> &'static str {
    text::t(lang, text::MsgKey::CliHelp)
}

/// 一次调用要做的事。三个"打印点"不解交付码、不碰网络。
///
/// ⚠️ `Help`/`Version` 带**语言**：它们可能出现在 `--lang` 之后
/// （`benagen-dl --help --lang en`），打印时用的就是客户选的那门语言。
#[derive(Debug)]
pub enum Action {
    Run(Options),
    License,
    Version(Lang),
    Help(Lang),
}

/// 一次下载所需的全部输入。
#[derive(Debug)]
pub struct Options {
    pub code: String,
    pub download_dir: PathBuf,
    pub overrides: Overrides,
    pub quiet: bool,
    pub base_url: Option<String>,
    /// 已经解析好的界面语言（`--lang` > `BENAGEN_DL_LANG` > 系统 locale）。
    pub lang: Lang,
}

/// 客户在命令行上显式给出的旋钮。`None` = 没给，交给内核的既有设置。
#[derive(Debug, Default, PartialEq)]
pub struct Overrides {
    pub parallel: Option<i32>,
    pub connections: Option<i32>,
    pub splits: Option<i32>,
}

/// 用法错误。`message` 直接面向客户，**不加工**。
#[derive(Debug)]
pub struct UsageError {
    pub message: String,
}

/// **语言来源的优先级**：命令行 `--lang` > 环境变量 `BENAGEN_DL_LANG` > 系统 locale。
///
/// **纯函数**：三个来源都由壳取好传进来（这样这条优先级能直接单测，不碰进程环境）。
/// 前两个都没给时才落到 `Lang::System.resolve(system_locale)`——"跟随系统"与
/// "谁都没选"在产出上是同一件事，**兜底是英文**（规格 §2.1）。
pub fn pick_lang(cli: Option<Lang>, env: Option<Lang>, system_locale: &str) -> Lang {
    cli.or(env).unwrap_or(Lang::System).resolve(system_locale)
}

/// 预扫 `--lang` 时可能出的两种错（主循环会把它们变成用法错误）。
#[derive(Debug, PartialEq, Eq)]
enum LangScanError {
    /// `--lang` 是最后一个词。
    MissingValue,
    /// `--lang` 的值不认识（不是 `Lang::parse` 认的那几个词）。
    Unknown(String),
}

/// **预扫** argv 里的 `--lang`。
///
/// ⚠️ 为什么帮助/版本/用法错误需要它：`benagen-dl --help --lang en` 里 `--help`
/// 排在 `--lang` **前面**，主循环一看到 `--help` 就 `return` 了，那时还不知道语言。
/// 所以语言要在进主循环**之前**先扫一遍。
///
/// 它必须与主循环吃同一份规则：`--` 之后不再有选项；`-o/-j/-x/-s` 的下一个词是
/// 它们的值（那里可能恰好写成 `--lang`，不能被当成选项）。重复给 `--lang` 时
/// **最后一个赢**。主循环不再自己读 `--lang` 的值（语言只在这里定一次），它只负责
/// 把那个值吃掉、别让它掉进位置参数。
fn scan_cli_lang(argv: &[String]) -> Result<Option<Lang>, LangScanError> {
    let mut found = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--" => break,
            // 值参：连它的值一起跳过去，值长什么样都不看（与主循环同一条宽松规则）。
            "-o" | "-j" | "-x" | "-s" => i += 2,
            "--lang" => {
                let Some(value) = argv.get(i + 1) else {
                    return Err(LangScanError::MissingValue);
                };
                match Lang::parse(value) {
                    Some(lang) => found = Some(lang),
                    None => return Err(LangScanError::Unknown(value.clone())),
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    Ok(found)
}

/// 把已按空白切好的 argv 解析成一个 [`Action`]。
///
/// **纯函数**：`home`、`env_base_url`、`env_lang`、`system_locale` 都由调用方（壳）
/// 从环境里取好传进来。这样这里不读 `$HOME`、不读 `$BENAGEN_BASE_URL`、
/// 不读 `$BENAGEN_DL_LANG`、不读 `$LANG`，每条分支都能直接单测。
///
/// `-o` 的值**原样收下，不检查目录是否存在**：目标目录可不可用是内核 `preflight`
/// 的判据，只有那一处；这里提前判一遍就会出现两份会漂移的判据。
pub fn parse(
    argv: &[String],
    home: Option<PathBuf>,
    env_base_url: Option<String>,
    env_lang: Option<&str>,
    system_locale: &str,
) -> Result<Action, UsageError> {
    // 语言**先定下来**：帮助、版本、以及下面每一条用法错误都要用它。
    // 环境变量里不认识的值当"没设"（与 `hello` 的 `requested_lang` 同一条：
    // 那不是一个客户能改的约定，为它报错只会让人莫名其妙）。
    let env_choice = env_lang.and_then(Lang::parse);
    let cli_choice = match scan_cli_lang(argv) {
        Ok(v) => v,
        Err(e) => {
            // 预扫失败时命令行那一路已经坏了：用**环境/系统**定语言，再报这一条错。
            let lang = pick_lang(None, env_choice, system_locale);
            let usage = text::t(lang, text::MsgKey::CliUsageSnippet);
            let message = match e {
                LangScanError::MissingValue => text::tf(
                    lang,
                    text::MsgKey::CliErrLangNoValue,
                    &[("usage", usage)],
                ),
                LangScanError::Unknown(value) => text::tf(
                    lang,
                    text::MsgKey::CliErrBadLang,
                    &[("value", &value), ("usage", usage)],
                ),
            };
            return Err(UsageError { message });
        }
    };
    let lang = pick_lang(cli_choice, env_choice, system_locale);
    let usage = text::t(lang, text::MsgKey::CliUsageSnippet);

    let mut code: Option<String> = None;
    let mut download_dir: Option<PathBuf> = None;
    let mut overrides = Overrides::default();
    let mut quiet = false;
    // `--` 之后全是位置参数：交付码若以 `-` 开头也能这样传进来。
    let mut positional_only = false;

    let usage_error = |message: String| Err(UsageError { message });

    let take_positional = |value: &str, code: &mut Option<String>| -> Result<(), UsageError> {
        if code.is_some() {
            return Err(UsageError {
                message: text::tf(
                    lang,
                    text::MsgKey::CliErrOnlyOneCode,
                    &[("usage", usage)],
                ),
            });
        }
        *code = Some(value.to_string());
        Ok(())
    };

    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        if positional_only {
            take_positional(arg, &mut code)?;
            i += 1;
            continue;
        }
        match arg {
            "--" => positional_only = true,
            "--quiet" => quiet = true,
            "--help" => return Ok(Action::Help(lang)),
            "--license" => return Ok(Action::License),
            "--version" => return Ok(Action::Version(lang)),
            // 语言已经由上面的预扫定下来了（`lang`），这里只把它吃掉，
            // 别让它的值掉进位置参数（否则 `--lang en` 里的 `en` 会被当成交付码）。
            "--lang" => i += 1,
            "-o" | "-j" | "-x" | "-s" => {
                i += 1;
                let Some(value) = argv.get(i) else {
                    let key = if arg == "-o" {
                        text::MsgKey::CliErrMissingFolder
                    } else {
                        text::MsgKey::CliErrMissingInteger
                    };
                    return usage_error(text::tf(lang, key, &[("arg", arg), ("usage", usage)]));
                };
                if arg == "-o" {
                    download_dir = Some(PathBuf::from(value));
                } else {
                    // 非整数在**解析阶段**就报错，不走内核的 validate：
                    // 内核的判据管的是范围，不是"这是不是一个数"。
                    let Ok(n) = value.parse::<i32>() else {
                        return usage_error(text::tf(
                            lang,
                            text::MsgKey::CliErrNotAnInteger,
                            &[("arg", arg), ("value", &format!("{value:?}"))],
                        ));
                    };
                    match arg {
                        "-j" => overrides.parallel = Some(n),
                        "-x" => overrides.connections = Some(n),
                        _ => overrides.splits = Some(n),
                    }
                }
            }
            other if other.starts_with('-') => {
                return usage_error(text::tf(
                    lang,
                    text::MsgKey::CliErrUnknownOption,
                    &[("other", &format!("{other:?}")), ("usage", usage)],
                ));
            }
            other => take_positional(other, &mut code)?,
        }
        i += 1;
    }

    let Some(code) = code else {
        return usage_error(text::tf(lang, text::MsgKey::CliErrNoCode, &[("usage", usage)]));
    };

    // 范围判据**只有一处**（约束 8：内核文案逐字）——把用户给的值填进一份默认设置里，
    // 让内核的 `validate()` 说话，`Err(message)` 原样透出，不另写一份错误消息。
    let mut probe = settings::default_settings();
    if let Some(v) = overrides.parallel {
        probe.parallel = v;
    }
    if let Some(v) = overrides.connections {
        probe.connections = v;
    }
    if let Some(v) = overrides.splits {
        probe.splits = v;
    }
    probe
        .validate_in(lang)
        .map_err(|message| UsageError { message })?;

    Ok(Action::Run(Options {
        code,
        download_dir: download_dir.unwrap_or_else(|| {
            home.map(|h| h.join("Downloads").join("Benagen"))
                .unwrap_or_else(|| PathBuf::from("Downloads/Benagen"))
        }),
        overrides,
        quiet,
        base_url: env_base_url,
        lang,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_of(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|s| s.to_string()).collect()
    }

    /// 以**中文系统**解析（原有用例都断言中文文案，所以这里固定一个 zh locale）。
    fn parse_with(
        argv: &[&str],
        env_lang: Option<&str>,
        system_locale: &str,
    ) -> Result<Action, UsageError> {
        parse(&argv_of(argv), Some(PathBuf::from("/home/u")), None, env_lang, system_locale)
    }

    fn parse_ok(argv: &[&str]) -> Options {
        match parse_with(argv, None, "zh_CN.UTF-8") {
            Ok(Action::Run(o)) => o,
            other => panic!("期望 Run，得到 {other:?}"),
        }
    }

    fn err_of(argv: &[&str]) -> UsageError {
        parse_with(argv, None, "zh_CN.UTF-8")
            .err()
            .unwrap_or_else(|| panic!("{argv:?} 应该被判死"))
    }

    fn has_cjk(s: &str) -> bool {
        s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    }

    #[test]
    fn a_code_alone_lands_in_the_default_dir() {
        let o = parse_ok(&["ABCdef1234567890abcd"]);
        assert_eq!(o.code, "ABCdef1234567890abcd");
        assert_eq!(o.download_dir, PathBuf::from("/home/u/Downloads/Benagen"));
        assert_eq!(o.overrides, Overrides::default(), "没给的不许有值");
        assert!(!o.quiet);
        // 系统 locale 是中文 ⇒ 中文（谁都没选的时候跟着系统走）
        assert_eq!(o.lang, Lang::Zh);
    }

    #[test]
    fn a_full_link_is_accepted_too() {
        let o = parse_ok(&["http://download.benagen.com/ABCdef1234567890abcd/index.html"]);
        assert!(o.code.starts_with("http://"));
    }

    #[test]
    fn without_home_it_falls_back_to_a_relative_dir() {
        match parse(&argv_of(&["ABCdef1234567890abcd"]), None, None, None, "zh_CN.UTF-8") {
            Ok(Action::Run(o)) => assert_eq!(o.download_dir, PathBuf::from("Downloads/Benagen")),
            other => panic!("期望 Run，得到 {other:?}"),
        }
    }

    #[test]
    fn o_and_the_three_knobs_are_all_taken() {
        let o = parse_ok(&[
            "ABCdef1234567890abcd",
            "-o",
            "/data/x",
            "-j",
            "8",
            "-x",
            "4",
            "-s",
            "5",
            "--quiet",
        ]);
        assert_eq!(o.download_dir, PathBuf::from("/data/x"));
        assert_eq!(o.overrides.parallel, Some(8));
        assert_eq!(o.overrides.connections, Some(4));
        assert_eq!(o.overrides.splits, Some(5));
        assert!(o.quiet);
    }

    #[test]
    fn the_three_switches_take_their_own_branch() {
        for (arg, want) in [
            ("--help", "help"),
            ("--license", "license"),
            ("--version", "version"),
        ] {
            let got = parse_with(&[arg], None, "zh_CN.UTF-8").unwrap();
            let got = match got {
                Action::Help(_) => "help",
                Action::License => "license",
                Action::Version(_) => "version",
                _ => "run",
            };
            assert_eq!(got, want, "{arg}");
        }
    }

    #[test]
    fn every_way_of_being_wrong_has_its_own_words() {
        let cases: &[(&[&str], &str)] = &[
            (&[], "交付码"),
            (&["a", "b"], "只能给一个"),
            (&["x", "--nope"], "不认识的选项"),
            (&["x", "-o"], "-o 后面要跟一个目录"),
            (&["x", "-j", "abc"], "-j"),
            (&["x", "-j", "0"], "并行文件数必须在 1–64 之间"),
            (&["x", "-j", "999"], "并行文件数必须在 1–64 之间"),
            (&["x", "-x", "0"], "单文件连接数必须在 1–16 之间"),
            (&["x", "-s", "0"], "分片数必须在 1–16 之间"),
        ];
        for (argv, want) in cases {
            let msg = err_of(argv).message;
            assert!(msg.contains(want), "{argv:?} 的消息 {msg:?} 里应当含 {want:?}");
        }
    }

    // -----------------------------------------------------------------------
    // 语言（T8）：--lang > BENAGEN_DL_LANG > 系统 locale
    // -----------------------------------------------------------------------

    /// 判据：`--lang` 认三个词，并且**打印点**（help/version）带的就是它。
    #[test]
    fn the_lang_option_sets_the_language() {
        assert!(matches!(
            parse_with(&["--help", "--lang", "en"], None, "zh_CN.UTF-8"),
            Ok(Action::Help(Lang::En))
        ));
        assert!(matches!(
            parse_with(&["--version", "--lang", "zh"], None, "en_US.UTF-8"),
            Ok(Action::Version(Lang::Zh))
        ));
        // `--lang` 的值不许掉进位置参数（否则 `en` 会被当成交付码）
        let o = parse_ok(&["ABCdef1234567890abcd", "--lang", "en"]);
        assert_eq!(o.code, "ABCdef1234567890abcd");
        assert_eq!(o.lang, Lang::En);
    }

    /// 判据：**命令行压过环境变量、环境变量压过系统**（逐层单独钉）。
    ///
    /// 这是规格里那条优先级本身；`pick_lang` 是它唯一的实现点。
    #[test]
    fn the_language_priority_is_cli_then_env_then_system() {
        // ① 命令行 > 环境 > 系统
        assert_eq!(
            pick_lang(Some(Lang::En), Some(Lang::Zh), "zh_CN.UTF-8"),
            Lang::En
        );
        // ② 环境 > 系统
        assert_eq!(
            pick_lang(None, Some(Lang::Zh), "en_US.UTF-8"),
            Lang::Zh
        );
        // ③ 只有系统
        assert_eq!(pick_lang(None, None, "zh_CN.UTF-8"), Lang::Zh);
        assert_eq!(pick_lang(None, None, "en_US.UTF-8"), Lang::En);
        // ④ `--lang system` 是"看系统"这个显式选择：它压过环境变量
        assert_eq!(
            pick_lang(Some(Lang::System), Some(Lang::En), "zh_CN.UTF-8"),
            Lang::Zh
        );

        // 同一套判据走完整解析（命令行那一路）
        let env = parse_with(&["ABCdef1234567890abcd"], Some("zh"), "en_US.UTF-8")
            .expect("应当解析成功");
        let Action::Run(o) = env else { panic!("期望 Run") };
        assert_eq!(o.lang, Lang::Zh, "环境变量应当压过系统");

        let cli_beats_env =
            parse_with(&["ABCdef1234567890abcd", "--lang", "en"], Some("zh"), "zh_CN.UTF-8")
                .expect("应当解析成功");
        let Action::Run(o) = cli_beats_env else { panic!("期望 Run") };
        assert_eq!(o.lang, Lang::En, "命令行应当压过环境变量");

        // 环境里不认识的值当"没设"（不报错），落回系统
        let Action::Run(o) = parse_with(&["ABCdef1234567890abcd"], Some("klingon"), "zh_CN.UTF-8")
            .expect("不认识的环境值不该让解析失败")
        else {
            panic!("期望 Run")
        };
        assert_eq!(o.lang, Lang::Zh);
    }

    /// 判据：`--lang` 的非法取值 ⇒ **用法错误**（`run` 会把它变成退出码 1）。
    #[test]
    fn an_unknown_lang_is_a_usage_error() {
        let e = err_of(&["ABCdef1234567890abcd", "--lang", "klingon"]);
        assert!(e.message.contains("--lang"), "{}", e.message);
        assert!(e.message.contains("klingon"), "{}", e.message);
        // 缺值也是用法错误，而且即使放在 `--help` 前面也照样报（预扫先看它）
        let e = err_of(&["--help", "--lang"]);
        assert!(e.message.contains("--lang"), "{}", e.message);
        // `--` 之后就没有选项了 ⇒ `--lang` 是普通位置参数，不报错
        let o = parse_ok(&["--", "--lang"]);
        assert_eq!(o.code, "--lang");
    }

    /// 判据：英文模式下，帮助与用法错误里**一个汉字都没有**。
    #[test]
    fn the_english_help_and_usage_errors_have_no_cjk() {
        let help = help(Lang::En);
        assert!(!has_cjk(help), "英文帮助里混进了中文：{help}");
        assert!(help.contains("--lang"), "英文帮助里要有 --lang：{help}");
        assert!(help.contains("BENAGEN_DL_LANG"), "英文帮助里要有环境变量：{help}");

        let cases: &[&[&str]] = &[
            &[],
            &["a", "b"],
            &["x", "--nope"],
            &["x", "-o"],
            &["x", "-j", "abc"],
            &["x", "--lang", "klingon"],
        ];
        for argv in cases {
            let msg = parse_with(argv, None, "en_US.UTF-8")
                .expect_err("应当被判死")
                .message;
            assert!(!has_cjk(&msg), "{argv:?} 的英文消息里混进了中文：{msg}");
        }
    }

    /// **R13**：帮助里写的默认值必须与内核的 `settings::default_settings()` 是同一个数。
    ///
    /// 为什么单列一条：这是"**同一件事两个真相源**"——帮助文本里手写的一串数，与内核实数。
    /// 两者漂移的形态是"客户照着帮助去调参，调出来的不是默认"，**而且不报错**。
    ///
    /// 抽取方式刻意不写死位置：按 `-j` / `-x` / `-s` 找到那一行，再从 `（默认 N）`
    /// （英文表是 `(default N)`）里取值。这样"默认值被挪到别的行"、"帮助被改写得
    /// 看不出默认值"都会红，而不是静默通过。**两种语言各验一遍**——英文表是新写的，
    /// 更可能把默认值抄错。
    #[test]
    fn the_defaults_in_the_help_text_are_the_kernels_real_ones() {
        let d = settings::default_settings();
        for lang in [Lang::Zh, Lang::En] {
            let (open, close) = match lang {
                Lang::Zh => ("（默认 ", "）"),
                _ => ("(default ", ")"),
            };
            for (flag, want) in [("-j", d.parallel), ("-x", d.connections), ("-s", d.splits)] {
                let line = help(lang)
                    .lines()
                    .find(|l| l.trim_start().starts_with(&format!("{flag} ")))
                    .unwrap_or_else(|| panic!("{lang:?} 帮助里没有 {flag} 那一行"));
                let got = line
                    .rsplit_once(open)
                    .and_then(|(_, rest)| rest.strip_suffix(close))
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or_else(|| {
                        panic!("{lang:?} 的 {flag} 那一行里没有可解析的默认值：{line:?}")
                    });
                assert_eq!(
                    got, want,
                    "{lang:?}/{flag}：帮助里写的默认值与内核实数不一致"
                );
            }
        }
    }

    /// **R14-1**：三条值参（`-j` / `-x` / `-s`）缺值时都必须报错，且消息里点出是哪一条。
    ///
    /// 这里原先**只有 `-o` 缺值**被钉住（`every_way_of_being_wrong_has_its_own_words`），
    /// 而三条值参走的是另一个分支——少了这条，把那条 `else` 改成"没有值就沉默地当成没给"
    /// 不会红：客户敲错一个 `-j`，工具会**照默认值跑起来**，而他以为设过了。
    #[test]
    fn every_knob_without_its_value_is_a_usage_error() {
        for flag in ["-j", "-x", "-s"] {
            let e = err_of(&["x", flag]);
            assert!(
                e.message.contains(&format!("{flag} 后面要跟一个整数")),
                "{flag} 缺值的消息是 {:?}",
                e.message
            );
        }
    }

    /// **R14-2**：`--` 之后**全是位置参数**（POSIX 惯例）——交付码以 `-` 开头也能这样传进来。
    ///
    /// 这条此前**零覆盖**，而它会静默回归：`--quiet` 在 `--` 后面只是一个普通的词。
    /// ⚠️ 交付码**格式**的判据不在这里（`args` 只看形态、不认内容），
    /// 由 `delivery::extract_code` 承重——所以这里收到的就是那串字面量本身。
    #[test]
    fn after_a_double_dash_everything_is_a_positional() {
        let o = parse_ok(&["--", "--quiet"]);
        assert_eq!(o.code, "--quiet");
        assert!(!o.quiet, "`--` 之后的东西不许被当成选项");
    }

    /// **R14-3**：`-o` 后面跟一个**看起来像选项**的值 ⇒ 宽松地吃下它、原样存、其余不变。
    ///
    /// ⚠️ 这是**决定，不是意外**（控制者裁定：保持宽松，用一条测试把它钉成决定）：
    /// 与"`--` 之后全是位置参数"那套不同，`-o` 是值参，它右边的第一个词就是它的值——
    /// 不做"这看起来像不像选项"的猜测。猜的代价是**正当的目录名传不进来**
    /// （一个叫 `-weird` 的目录永远没法指定），而那没有绕法；
    /// 真给错了目录也不会静默出错：路径可不可用是内核 `preflight` 的判据（只有那一处）。
    #[test]
    fn the_o_knob_takes_the_next_word_even_when_it_looks_like_a_flag() {
        let o = parse_ok(&["-o", "--quiet", "ABCdef1234567890abcd"]);
        assert_eq!(o.download_dir, PathBuf::from("--quiet"));
        assert_eq!(o.code, "ABCdef1234567890abcd");
        assert!(!o.quiet, "`--quiet` 被 -o 当成目录吃掉了，就不该同时生效");
    }

    #[test]
    fn the_base_url_from_the_environment_is_taken() {
        let o = match parse(
            &argv_of(&["ABCdef1234567890abcd"]),
            None,
            Some("http://127.0.0.1:8000".to_string()),
            None,
            "",
        ) {
            Ok(Action::Run(o)) => o,
            other => panic!("{other:?}"),
        };
        assert_eq!(o.base_url.as_deref(), Some("http://127.0.0.1:8000"));
    }
}
