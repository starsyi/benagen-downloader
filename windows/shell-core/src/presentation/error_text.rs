//! error_text —— `ClientError` → 要说的话。**规格 §10 那条"唯一映射"的 Rust 侧实现。**
//!
//! 上游：`macos/Sources/BenagenCoreKit/AppModel.swift:1701-1707` 的 `AppModel.message(of:)`。
//! 上游那一侧的三种情形（`rpc` / `transport` / `malformedResponse`）**都返回它自己的载荷
//! 文本**，一个字的加工都没有 —— 而这正是规格 §10 的口径："壳不改写内核的话"。
//!
//! ⚠️ **为什么它单独成一个模块**（控制者裁决 GG）：这条纯计算在源里住在**未移植的**
//!    `AppModel.swift` 里，而本波次就必须用到它。裁决 GG 的口径是：这类"源在
//!    `AppModel.swift` 里、本波次又必须存在的纯计算"一律落 `presentation/` 下
//!    **按主题命名的 `pub` 模块**、**自带单测**，**不寄生在"第一个用到它的模块"里**。
//!    不这么做的代价是具体的：它会长在"下载目标"那个模块里，于是那个模块变成全应用
//!    错误文案的家（名字与职责对不上），而下一个需要它的任务只有两条路 ——
//!    把它从别人家里 `pub` 出来，或者抄一份（**直接违反 §10 的"只许有一份"**）。
//!
//! ⚠️ **形态偏离（W-6）**：源是 `message(of error: Error) -> String`，收的是**任意** `Error`，
//!    并带一条兜底：`guard let e = error as? CoreError else { return "\(error)" }`
//!    （`AppModel.swift:1702`）。Rust 侧的对应物**只能收 [`ClientError`]**（本 crate 是强类型的，
//!    没有"任意错误"这个类型），所以**那条兜底路径整个消失了**。
//!    影响面：一个**不是** `ClientError` 的错误在当前结构里根本到不了这里（调用方拿到的就是
//!    `ClientError`），所以消失的兜底**没有对应的可达输入**；但日后壳若引入第二种错误类型，
//!    "任意错误 → 文本"这一步必须在**调用点**补（或给本函数加一层枚举包装），
//!    **不许在这里悄悄吞掉**——那会把"某个错误没人翻译"变成一句空话或一行 `{:?}`。
//!
//! ⚠️ 本模块**不是**"错误码 → 中文文案"的查表（规格 §10 明确说壳不做这个，macOS 版也没有：
//!    `ErrorCode` 只用于分支与状态处置）。它只是把**已经有**的文本交出来。

use crate::client::ClientError;

/// `ClientError` → 要说的话。
///
/// 对位关系（Rust 的 `ClientError` 比 Swift 的 `CoreError` 形状更细，所以判据要写清）：
///
///   - [`ClientError::Kernel`] ⇒ 取 `message` 字段**逐字**。这里**不能**走 `Display`：
///     后者会加上 `内核报错 [code]：` 前缀，而约束 3 要的是内核原文一字不加
///     （上游同样是直接返回 `message`，不拼 `code`）。
///   - 其余变体 ⇒ 用 `Display`。那些人话文案归 `client.rs` 所有（它们与内核无关，
///     是壳自己探测到的传输层故障），这里**不重抄一份**——抄了就是 §10 禁止的第二份。
pub fn error_text(error: &ClientError) -> String {
    match error {
        ClientError::Kernel { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! 本模块的测试**不是**从上游搬来的（上游那一条在 `AppModelTests` 里，属另一个文件的
    //! 移植范围）：它钉的是裁决 GG 要求"自带单测"的这条纯计算，
    //! 以及 W-6 记下的那处形态偏离的可观测后果。
    //!
    //! 三条各钉一个**不同的失效形态**，缺一条就有一种改法不会变红。

    use super::error_text;
    use crate::client::ClientError;
    use std::path::PathBuf;

    /// **内核原文逐字，一个字都不许加** —— 尤其不许加 `[code]` 前缀。
    ///
    /// 判别力：把实现换成 `error.to_string()`（即 `Display`），这一条立刻红 ——
    /// `Display` 给的是 `内核报错 [invalid_params]：…`。而夹具里的 `message` **本身**
    /// 就带方括号与码字样的文本，所以"前缀"与"正文"在断言上分得开。
    ///
    /// 相反方向的改法（把 message 里的某一段"清理"掉）同样会红：断言是**相等**，
    /// 不是"包含"。
    #[test]
    fn the_kernel_message_is_verbatim_without_the_code_prefix() {
        let message = "没有任何文件被加入下载：[Object {\"path\": \"z.bin\", \"reason\": \"路径不安全（越界/控制字符/空段）\"}]";
        let e = ClientError::Kernel {
            code: "invalid_params".to_string(),
            message: message.to_string(),
        };

        assert_eq!(error_text(&e), message, "内核原文必须逐字交出去（约束 3）");
        assert!(
            !error_text(&e).contains("内核报错"),
            "不得借道 `Display`：它会拼上「内核报错 [code]：」前缀"
        );
        assert!(
            !error_text(&e).contains("invalid_params"),
            "`code` 是给壳分支用的（契约 §5.1），不该出现在给用户看的这句话里"
        );
    }

    /// **非 `Kernel` 的变体走 `client.rs` 那份人话文案**，这里不另写一份、也不吞掉。
    ///
    /// 判别力：把 `other => other.to_string()` 换成返回空串/占位符（"出错了"之类），
    /// 这一条立刻红 —— 而那个改动会把"内核进程没了""管道断了"这些**能救的**信息
    /// 换成一句没法行动的空话（本项目最恨的静默降级）。
    #[test]
    fn transport_failures_keep_the_clients_own_text() {
        let cases = [
            ClientError::KernelGone,
            ClientError::LineTooLong { limit: 64 << 20 },
            ClientError::LineNotUtf8,
            ClientError::RequestTooLong {
                bytes: 9 << 20,
                limit: 8 << 20,
            },
            ClientError::Spawn {
                path: PathBuf::from("/tmp/no-such-core"),
                cause: std::io::Error::new(std::io::ErrorKind::NotFound, "没有这个文件"),
            },
        ];
        for e in &cases {
            let text = error_text(e);
            assert!(!text.trim().is_empty(), "传输层失败也必须说得出话（{e:?}）");
            assert_eq!(
                text,
                e.to_string(),
                "非 Kernel 变体交给 `client.rs` 的 `Display`（唯一一份），不在这里另写"
            );
        }
    }

    /// **内核给的是空串时不编一个占位出来**（契约 §10 的"只陈述事实"）。
    ///
    /// 判别力：给空 `message` 补一句"未知错误"看起来无害，但它把"内核说了空话"
    /// 与"壳没听懂"合并成了同一种呈现 —— 那是本项目反复记账的"静默少交"。
    #[test]
    fn an_empty_kernel_message_stays_empty_instead_of_gaining_a_placeholder() {
        let e = ClientError::Kernel {
            code: String::new(),
            message: String::new(),
        };
        assert_eq!(error_text(&e), "");
    }
}
