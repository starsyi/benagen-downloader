//! argv → [`Action`] 的纯函数解析。
//!
//! **不引 clap**（全局约束 7：不新增依赖）——argv 是一串已按空白切好的字符串，
//! 手写一遍循环比引一棵依赖树划算得多。

use std::path::PathBuf;

use crate::settings;

/// 帮助文本。默认值是 `settings::default_settings()` 的**实数**（8/16/16），
/// 不是"客户端认为合理的默认值"——两处各写一份就会漂移。
pub const HELP: &str = r#"benagen-dl —— 把一批交付数据完整下载到本地

用法：
  benagen-dl <交付码或完整链接> [选项]

选项：
  -o <目录>     下载到哪（默认 $HOME/Downloads/Benagen）
  -j <N>        并行文件数，1–64（默认 8）
  -x <N>        单文件连接数，1–16（默认 16）
  -s <N>        分片数，1–16（默认 16）
  --quiet       只打印错误与最终结论
  --license     打印内嵌组件（aria2）的 GPLv2 全文
  --version     打印版本与内嵌 aria2c 的摘要
  --help        打印这段说明

环境变量：
  BENAGEN_BASE_URL   清单与文件的基址（默认 http://download.benagen.com）

退出码：
  0 全部完成且校验通过   1 用法错误   2 拉清单失败   3 目标目录不可用
  4 引擎起不来           5 下载未完成 6 校验不通过   130 被 Ctrl-C 中断（可续传）

中断或断网后，**再跑一次同一条命令**就会接着下。
"#;

/// 一次调用要做的事。三个"打印点"不解交付码、不碰网络。
#[derive(Debug)]
pub enum Action {
    Run(Options),
    License,
    Version,
    Help,
}

/// 一次下载所需的全部输入。
#[derive(Debug)]
pub struct Options {
    pub code: String,
    pub download_dir: PathBuf,
    pub overrides: Overrides,
    pub quiet: bool,
    pub base_url: Option<String>,
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

/// 把已按空白切好的 argv 解析成一个 [`Action`]。
///
/// **纯函数**：`home` 与 `env_base_url` 由调用方（壳）从环境里取好传进来。
/// 这样这里不读 `$HOME`、不读 `$BENAGEN_BASE_URL`，每条分支都能直接单测。
///
/// `-o` 的值**原样收下，不检查目录是否存在**：目标目录可不可用是内核 `preflight`
/// 的判据，只有那一处；这里提前判一遍就会出现两份会漂移的判据。
pub fn parse(
    argv: &[String],
    home: Option<PathBuf>,
    env_base_url: Option<String>,
) -> Result<Action, UsageError> {
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
                message:
                    "只能给一个交付码。用法：benagen-dl <交付码或完整链接> [选项]".to_string(),
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
            "--help" => return Ok(Action::Help),
            "--license" => return Ok(Action::License),
            "--version" => return Ok(Action::Version),
            "-o" | "-j" | "-x" | "-s" => {
                i += 1;
                let Some(value) = argv.get(i) else {
                    let what = if arg == "-o" {
                        "-o 后面要跟一个目录".to_string()
                    } else {
                        format!("{arg} 后面要跟一个整数")
                    };
                    return usage_error(format!(
                        "{what}。用法：benagen-dl <交付码或完整链接> [选项]"
                    ));
                };
                if arg == "-o" {
                    download_dir = Some(PathBuf::from(value));
                } else {
                    // 非整数在**解析阶段**就报错，不走内核的 validate：
                    // 内核的判据管的是范围，不是"这是不是一个数"。
                    let Ok(n) = value.parse::<i32>() else {
                        return usage_error(format!("{arg} 需要一个整数，收到 {value:?}"));
                    };
                    match arg {
                        "-j" => overrides.parallel = Some(n),
                        "-x" => overrides.connections = Some(n),
                        _ => overrides.splits = Some(n),
                    }
                }
            }
            other if other.starts_with('-') => {
                return usage_error(format!(
                    "不认识的选项 {other:?}。用法：benagen-dl <交付码或完整链接> [选项]"
                ));
            }
            other => take_positional(other, &mut code)?,
        }
        i += 1;
    }

    let Some(code) = code else {
        return usage_error(
            "缺交付码。用法：benagen-dl <交付码或完整链接> [选项]，例如 benagen-dl ABCdef1234567890abcd"
                .to_string(),
        );
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
    probe.validate().map_err(|message| UsageError { message })?;

    Ok(Action::Run(Options {
        code,
        download_dir: download_dir.unwrap_or_else(|| {
            home.map(|h| h.join("Downloads").join("Benagen"))
                .unwrap_or_else(|| PathBuf::from("Downloads/Benagen"))
        }),
        overrides,
        quiet,
        base_url: env_base_url,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(argv: &[&str]) -> Options {
        match parse(
            &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            Some(PathBuf::from("/home/u")),
            None,
        ) {
            Ok(Action::Run(o)) => o,
            other => panic!("期望 Run，得到 {other:?}"),
        }
    }

    #[test]
    fn a_code_alone_lands_in_the_default_dir() {
        let o = parse_ok(&["ABCdef1234567890abcd"]);
        assert_eq!(o.code, "ABCdef1234567890abcd");
        assert_eq!(o.download_dir, PathBuf::from("/home/u/Downloads/Benagen"));
        assert_eq!(o.overrides, Overrides::default(), "没给的不许有值");
        assert!(!o.quiet);
    }

    #[test]
    fn a_full_link_is_accepted_too() {
        let o = parse_ok(&["http://download.benagen.com/ABCdef1234567890abcd/index.html"]);
        assert!(o.code.starts_with("http://"));
    }

    #[test]
    fn without_home_it_falls_back_to_a_relative_dir() {
        match parse(&["ABCdef1234567890abcd".to_string()], None, None) {
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
            let got = parse(&[arg.to_string()], None, None).unwrap();
            let got = match got {
                Action::Help => "help",
                Action::License => "license",
                Action::Version => "version",
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
            let got = parse(
                &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                None,
                None,
            );
            let msg = got.expect_err(&format!("{argv:?} 应该被判死")).message;
            assert!(msg.contains(want), "{argv:?} 的消息 {msg:?} 里应当含 {want:?}");
        }
    }

    /// **R13**：`HELP` 里写的默认值必须与内核的 `settings::default_settings()` 是同一个数。
    ///
    /// 为什么单列一条：这是"**同一件事两个真相源**"——帮助文本里手写的一串数，与内核实数。
    /// 两者漂移的形态是"客户照着帮助去调参，调出来的不是默认"，**而且不报错**。
    ///
    /// 抽取方式刻意不写死位置：按 `-j` / `-x` / `-s` 找到那一行，再从 `（默认 N）` 里取值。
    /// 这样"默认值被挪到别的行"、"帮助被改写得看不出默认值"都会红，而不是静默通过。
    /// 判据只认 `（默认 N）` 这个形状——它是 HELP 与客户之间的约定，不是本测试发明的。
    #[test]
    fn the_defaults_in_the_help_text_are_the_kernels_real_ones() {
        let d = settings::default_settings();
        for (flag, want) in [("-j", d.parallel), ("-x", d.connections), ("-s", d.splits)] {
            let line = HELP
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("{flag} ")))
                .unwrap_or_else(|| panic!("帮助里没有 {flag} 那一行"));
            let got = line
                .rsplit_once("（默认 ")
                .and_then(|(_, rest)| rest.strip_suffix("）"))
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or_else(|| {
                    panic!("{flag} 那一行里没有可解析的「（默认 N）」：{line:?}")
                });
            assert_eq!(got, want, "{flag}：帮助里写的默认值与内核实数不一致");
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
            let e = parse(&["x".to_string(), flag.to_string()], None, None)
                .expect_err(&format!("{flag} 缺值应当被判死"));
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
            &["ABCdef1234567890abcd".to_string()],
            None,
            Some("http://127.0.0.1:8000".to_string()),
        ) {
            Ok(Action::Run(o)) => o,
            other => panic!("{other:?}"),
        };
        assert_eq!(o.base_url.as_deref(), Some("http://127.0.0.1:8000"));
    }
}
