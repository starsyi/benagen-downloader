//! 内核错误码 → 进程退出码（纯函数）。
//!
//! ⚠️ **这是一份契约，不是本工具的内部选择**：脚本与调度系统靠它判"到底成没成"，
//! 所以要写进面向客户的文档，**发布之后不好改**。数值与出处见设计规格 §3。
//! 本模块是纯函数——不读环境、不碰文件系统、不启动引擎。
//!
//! **`0` 的分量比别的码都重**：`0` 只能意味着"清单里所有文件都在盘上、且 crc64 都对得上"。
//! 把"下完了"当成成功，比报失败糟得多——客户拿到一份"成功"的回执、数据其实是坏的，
//! 等发现时可能已经删了源。

use crate::kcodes;
use crate::protocol::codes;
use crate::protocol::ErrorBody;

/// 一次运行的最后结论。**每一支都直接对应进程退出码**（[`ExitCode::code`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// 0：清单里所有文件下载完成**且 crc64 校验通过**。
    Ok,
    /// 1：用法错误（参数不合法 / 交付码格式不对）。
    Usage,
    /// 2：拉清单失败（网络 / 码不存在 / 清单不合法）。
    Manifest,
    /// 3：目标目录不可用（不可写 / 磁盘不足）。
    Preflight,
    /// 4：引擎起不来（aria2c 释放或启动失败）。
    Engine,
    /// 5：下载未完成（有文件失败 / 卡住）。
    Incomplete,
    /// 6：校验不通过（下完了但 crc64 对不上）。
    VerifyFailed,
    /// 130：被 Ctrl-C 中断（**可续传**）。
    Interrupted,
}

impl ExitCode {
    /// **全部变体**，顺序与声明一致。
    ///
    /// ⚠️ 为什么它在这儿、而不是写在测试里：`message()` 的性质测试（每个变体的话非空且
    /// 两两不同）必须遍历**全部**变体。若那份清单手写在测试里，加了第 9 个变体时
    /// `code()` / `message()` 的 `match` 会因穷尽性编不过——**但测试里那份手写清单不会**，
    /// 于是新变体**静默地**不被这条性质覆盖（实测过：加一个变体、只补 `code()`/`message()`，
    /// 那条测试照样 `ok`）。放在枚举旁边、并配上下面的 `ExitCode::roll_call`，
    /// 编译器叫醒作者的地方就正好是这份清单的旁边。
    ///
    /// 数组长度是**显式**的：从 8 变成别的数字，本身就是"看这里"的信号。
    pub const ALL: [ExitCode; 8] = [
        ExitCode::roll_call(ExitCode::Ok),
        ExitCode::roll_call(ExitCode::Usage),
        ExitCode::roll_call(ExitCode::Manifest),
        ExitCode::roll_call(ExitCode::Preflight),
        ExitCode::roll_call(ExitCode::Engine),
        ExitCode::roll_call(ExitCode::Incomplete),
        ExitCode::roll_call(ExitCode::VerifyFailed),
        ExitCode::roll_call(ExitCode::Interrupted),
    ];

    /// 点名。**唯一的目的是让"漏了变体"变成编译错误**：这条 `match` 没有 `_` 分支，
    /// 加了第 9 个变体就编不过，作者被迫回到这个 `impl` 块里来——而 [`ExitCode::ALL`]
    /// 就在上面几行。平常它只是把变体原样还回去，`ALL` 逐项调用它。
    ///
    /// （为什么不能自动枚举：stable Rust 没有 `variant_count`/无依赖的 derive，
    /// 纯手写清单配一条穷尽 `match` 是这里能拿到的最强约束——把"忘了改"从静默变成报错。）
    const fn roll_call(code: ExitCode) -> ExitCode {
        match code {
            ExitCode::Ok
            | ExitCode::Usage
            | ExitCode::Manifest
            | ExitCode::Preflight
            | ExitCode::Engine
            | ExitCode::Incomplete
            | ExitCode::VerifyFailed
            | ExitCode::Interrupted => code,
        }
    }

    /// 进程退出码。数值是契约（设计规格 §3），改动等于改接口。
    pub const fn code(self) -> i32 {
        match self {
            ExitCode::Ok => 0,
            ExitCode::Usage => 1,
            ExitCode::Manifest => 2,
            ExitCode::Preflight => 3,
            ExitCode::Engine => 4,
            ExitCode::Incomplete => 5,
            ExitCode::VerifyFailed => 6,
            // 128 + SIGINT(2)：shell 的惯例读法，也便于脚本区分"被中断"与"下不完"。
            ExitCode::Interrupted => 130,
        }
    }

    /// 最后一行给人看的中文说明。**都在说"接下来该干什么"**，不是复述错误。
    ///
    /// 内核给的原文（`ErrorBody.message`）由壳逐字打印在**前一行**（全局约束 8），
    /// 这一句只负责把"这是哪一步坏的、接下来怎么办"收口。
    pub const fn message(self) -> &'static str {
        match self {
            ExitCode::Ok => "全部完成，校验通过",
            ExitCode::Usage => "命令行用法不对——照上面的说明改一下再跑",
            ExitCode::Manifest => "清单没拉到——先确认交付码与网络，再跑一次",
            ExitCode::Preflight => "这个目录用不了——换一个可写、空间够的目录（-o）再跑",
            ExitCode::Engine => "下载引擎没起来——这是本机环境的问题，把上面的报错发给我们",
            ExitCode::Incomplete => "下载没有完成——再跑一次会接着下",
            ExitCode::VerifyFailed => "文件都下完了，但校验没通过与下一行一起看是哪几类",
            ExitCode::Interrupted => "被中断（再跑一次会接着下）",
        }
    }
}

/// 内核给的错误码 → 退出码。
///
/// ⚠️ **没列出来的码一律归 [`ExitCode::Incomplete`]（5），这是承重的保守选择**：
/// 退出码 `0` 是脚本与调度系统眼里"数据全在盘上、校验也对"的唯一凭证，
/// 而内核的错误码集合**将来还会长**（新方法、新失败模式）。若某个今天不存在的码
/// 落进一条"没匹配上就当成成功"的分支，后果是**失败了却报成功**——正是这张表要防的事，
/// 而且发现时客户可能已经按"成功"处理完、甚至删了源。归 `5` 的代价只是"本来能更精确地
/// 说明原因，现在说成下载未完成"，那是可以承受的；归 `0` 的代价不可承受。
/// 所以新增内核错误码时，正确做法是**在这里补一条并补测试**，而不是放宽兜底。
///
/// ⚠️ **兜底支管的是"将来才会有的码"，不是"已知但懒得分类的码"**（控制者裁定 R15）：
/// 一个码只要**已经被认出来**，就必须有自己的去处。把已知的码丢进兜底不是保守，
/// 而是**让那档的文案去说假话**——兜底那档的话是「下载没有完成——再跑一次会接着下」，
/// 而"协议不匹配"根本不是"没下完"，客户照这句话再跑一次也永远不会好。
///
/// （这里**不列"还有哪些码没被分类"**：那种清单会过期，而过期的清单比没有更坏——
/// 它把"漏了"伪装成"查过了"。判据是上一条：已认出来的码必须有自己的去处。）
///
/// ⚠️ **分支写在 `kcodes` / `codes` 的常量上，不写裸字符串**：这几条码在内核里各有一份常量，
/// 在这里再抄一遍字面量就是同一个真相的第二份拷贝——常量值哪天变了，抄来的那份**不会报错**，
/// 只会静默地掉进兜底支。写成常量之后，同样的改动会**编译期**（重复值触发不可达分支告警）
/// 或**测试期**（`exit.rs` 的用例钉的是字面量）暴露出来。
pub fn of_error(body: &ErrorBody) -> ExitCode {
    match body.code.as_str() {
        // 拉清单：网络、404、清单本身不合法；以及"还没有清单/清单已被换码作废"。
        kcodes::DELIVERY_FETCH_FAILED | kcodes::NO_DELIVERY => ExitCode::Manifest,
        // 开工前检查没过：目标目录不可写、磁盘不足。引擎没被启动过。
        kcodes::PREFLIGHT_FAILED => ExitCode::Preflight,
        // 引擎的四种起不来/断掉：端口、二进制、RPC 不可达、重连也没成、还没起。
        kcodes::ENGINE_START_FAILED
        | kcodes::ENGINE_DISCONNECTED
        | kcodes::ENGINE_NOT_STARTED
        | kcodes::ENGINE_RPC_FAILED => ExitCode::Engine,
        // 参数不合法是**用法**问题（契约 §2.3 的 `-k`），不是数据问题。
        codes::INVALID_PARAMS => ExitCode::Usage,
        // 请求本身不合法（畸形 JSON、形状不对、行长超限）= **我们造出去的请求有问题**，
        // 与"参数不合法"同属输入层。它不是"下到一半"，也不该让客户"再跑一次"。
        codes::BAD_REQUEST => ExitCode::Usage,
        // 内核没按期望的方式应答（版本对不上 / 不认这个方法）：这是**引擎层**说岔了。
        // `unknown_method` 对 CLI 不可达（方法名是我们自己写死的），列出来是为了这份表完整。
        codes::PROTOCOL_MISMATCH | kcodes::UNKNOWN_METHOD => ExitCode::Engine,
        // 内核自己出错（响应序列化不出来那类）：**内核自身**的故障，同属引擎层。
        // ⚠️ 它**有真生产者**（`kernel.rs` 里两处在构造 `codes::INTERNAL`），不是假想的分支——
        // 漏掉它就会去打印「再跑一次会接着下」，而那对内核自身故障是**假话**。
        // （`protocol.rs` 里"这一支实践中不可达、不要为它编假测试"说的是"别去伪造一个畸形
        // 请求来触发它"；这里是**映射断言**，不是伪造协议现场。）
        codes::INTERNAL => ExitCode::Engine,
        // 路径不在清单里：这批数据下不全，但已经下的那些还在，重跑能接着下。
        kcodes::PATH_NOT_FOUND => ExitCode::Incomplete,
        // 兜底：**只给"将来才会有的码"**（见函数头）。**不许改成 `Ok`**。
        _ => ExitCode::Incomplete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 显式写全路径：即使将来上面那条 `use` 被去掉，这几条测试也不受影响。
    use crate::protocol::ErrorBody;

    #[test]
    fn the_numbers_are_the_ones_on_the_contract() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Usage.code(), 1);
        assert_eq!(ExitCode::Manifest.code(), 2);
        assert_eq!(ExitCode::Preflight.code(), 3);
        assert_eq!(ExitCode::Engine.code(), 4);
        assert_eq!(ExitCode::Incomplete.code(), 5);
        assert_eq!(ExitCode::VerifyFailed.code(), 6);
        assert_eq!(ExitCode::Interrupted.code(), 130);
    }

    #[test]
    fn each_kernel_error_code_maps_to_its_own_exit_code() {
        let cases = [
            ("delivery_fetch_failed", ExitCode::Manifest),
            ("no_delivery", ExitCode::Manifest),
            ("preflight_failed", ExitCode::Preflight),
            ("engine_start_failed", ExitCode::Engine),
            ("engine_disconnected", ExitCode::Engine),
            ("engine_not_started", ExitCode::Engine),
            ("engine_rpc_failed", ExitCode::Engine),
            ("invalid_params", ExitCode::Usage),
            ("path_not_found", ExitCode::Incomplete),
        ];
        for (code, want) in cases {
            assert_eq!(of_error(&ErrorBody::new(code, "x")), want, "{code}");
        }
    }

    #[test]
    fn an_unknown_kernel_error_code_falls_back_to_5_never_0() {
        assert_eq!(
            of_error(&ErrorBody::new("将来才有的码", "x")),
            ExitCode::Incomplete
        );
    }

    /// 协议层的三条码**各有各的去处，不许掉进兜底**（控制者裁定 R15）。
    ///
    /// 为什么这不是洁癖：兜底那一档的文案是「下载没有完成——再跑一次会接着下」，
    /// 而"协议不匹配"根本不是"没下完"——判成 5，那句话就成了一句**假话**。
    /// 兜底管的是"**将来才会有的**码"，不是"**已知但懒得分类的**码"。
    #[test]
    fn the_protocol_level_codes_do_not_fall_into_the_catch_all() {
        assert_eq!(of_error(&ErrorBody::new("bad_request", "x")), ExitCode::Usage);
        assert_eq!(
            of_error(&ErrorBody::new("protocol_mismatch", "x")),
            ExitCode::Engine
        );
        assert_eq!(
            of_error(&ErrorBody::new("unknown_method", "x")),
            ExitCode::Engine
        );
        // `internal` 有**真生产者**（`kernel.rs` 里两处在构造 `codes::INTERNAL`），
        // 不是假想的：漏掉它就会去打印「再跑一次会接着下」——对内核自身故障是假话。
        // 这条是**映射断言**，不是伪造协议现场（`protocol.rs` 那条"不要为它编假测试"
        // 说的是后者：不许去构造一个畸形请求来触发 `internal`）。
        assert_eq!(of_error(&ErrorBody::new("internal", "x")), ExitCode::Engine);
    }

    /// 每个变体都要有一句**自己的**话，且非空。
    ///
    /// 两条不同的失败给出同一句话，等于其中一条**永远不会以正确的样子**出现在界面上——
    /// 与 Swift 侧 `RowStateStyle` 钉"四个状态两两不同"是同一条理由。
    /// 这里**不逐字钉文案**（那是措辞，不是契约；脚本读的是数字），只钉这个性质。
    #[test]
    fn every_exit_code_has_its_own_non_empty_message() {
        let mut seen = std::collections::HashSet::new();
        for code in ExitCode::ALL {
            let message = code.message();
            assert!(!message.trim().is_empty(), "{code:?} 的那句话是空的");
            assert!(
                seen.insert(message),
                "{code:?} 的那句话与别的变体重了：{message:?}"
            );
        }
        assert_eq!(
            seen.len(),
            ExitCode::ALL.len(),
            "不同文案的条数必须等于变体数"
        );
    }
}
