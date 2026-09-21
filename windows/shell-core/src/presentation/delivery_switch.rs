//! delivery_switch —— 一次「换交付码」的结果，以及那一行常驻提示要显示什么。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/DeliverySwitch.swift`。
//!
//! ⚠️ 为什么要有这个类型：换码面板是**模态**的，而它的「取消」按钮**不被禁用**
//!    （面板关不关得掉，不该由一次网络请求决定）。于是那次换码的结果会在面板销毁时
//!    **一起消失**，两个方向都会变成静默失效：
//!      - **失败方向**：内核那句原文是用户唯一能照着做点什么的话，面板一关就没了；
//!      - **成功方向**：请求照跑，成功时会把整批换掉并静默复位浏览位置与勾选面 ——
//!        用户看到的是"我按了取消，界面自己换了一批"，比他想象的更糟。
//!    所以结果落在模型上，由主区那一行提示**常驻**呈现。

use serde::Serialize;

/// 一次「换交付码」的结果 —— 界面只需要知道这两件事。
///
/// 上游 `DeliverySwitch.swift` 的 `DeliverySwitch`。
///
/// ⚠️ 文案**不是**壳编的：失败那支就是 [`error_text`](crate::presentation::error_text::error_text)
///    （`ClientError` → 用户可见文案的**唯一**实现）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub enum DeliverySwitch {
    /// 换成功了，这是新的交付码。
    Switched { code: String },
    /// 没换成，这是**可直接显示给用户的那句话**（内核原文，或壳在"内核一个字都没说"时的说明）。
    Failed { message: String },
}

impl DeliverySwitch {
    /// 主区那一行提示要显示的文字。
    ///
    /// 上游 `DeliverySwitch.noticeText`。
    ///
    /// - 失败：**内核原文逐字**，这里不加工、不加前缀。
    /// - 成功：`已换到批次 <code>`。
    ///   🔴 **这一句是壳自己写的用户可见文案**（理由写在这里）：内核**不会为一次成功的
    ///      换码主动说话** —— 它回的是 JSON 字段（`code` 等），没有一句可以照登的人话。
    ///      而"界面自己换了一批"必须有个落点，否则取消之后的那次成功换码在用户眼里
    ///      就是一次**没有来源的界面变化**。这句话只陈述一个**内核给的事实**
    ///      （新批次的码就是它回的 `code`），不添加任何判断或建议。
    pub fn notice_text(&self) -> String {
        match self {
            DeliverySwitch::Switched { code } => format!("已换到批次 {code}"),
            DeliverySwitch::Failed { message } => message.clone(),
        }
    }

    /// 这一行说的是"没成"吗（调用方据此选图标与颜色 —— 渲染分派）。**有单测**：
    /// 它不是顺手加的，而是"两种结果在界面上分得开"这条要求的落点。
    ///
    /// 上游 `DeliverySwitch.isFailure`。
    pub fn is_failure(&self) -> bool {
        matches!(self, DeliverySwitch::Failed { .. })
    }
}

// ---------------------------------------------------------------------------
// 测试（先写测试：它们会先红，见任务 6 的步骤 2）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游 `macos/Tests/BenagenCoreKitTests/DeliverySwitchTests.swift`（逐条对位）。

    use super::DeliverySwitch;
    use crate::client::ClientError;
    use crate::presentation::error_text::error_text;

    /// 上游 `theTwoOutcomesAreDistinguishable`。
    #[test]
    fn the_two_outcomes_are_distinguishable() {
        assert_eq!(
            DeliverySwitch::Switched {
                code: "BBBB".to_string()
            },
            DeliverySwitch::Switched {
                code: "BBBB".to_string()
            }
        );
        assert_ne!(
            DeliverySwitch::Switched {
                code: "BBBB".to_string()
            },
            DeliverySwitch::Switched {
                code: "AAAA".to_string()
            }
        );
        assert_eq!(
            DeliverySwitch::Failed {
                message: "x".to_string()
            },
            DeliverySwitch::Failed {
                message: "x".to_string()
            }
        );
        assert_ne!(
            DeliverySwitch::Failed {
                message: "x".to_string()
            },
            DeliverySwitch::Failed {
                message: "y".to_string()
            }
        );
        // 同一个字符串在两支里**不得**互相相等 —— 那句 `if case .failed` 就靠这一条。
        assert_ne!(
            DeliverySwitch::Switched {
                code: "同一句话".to_string()
            },
            DeliverySwitch::Failed {
                message: "同一句话".to_string()
            }
        );
    }

    /// 上游 `theFailureMessageComesFromTheSingleMapping`。
    ///
    /// ⚠️ 上游那一条比的是 `AppModel.message(of:)`；本 crate 里那条"唯一映射"是
    ///    [`error_text`]（`error_text.rs` 的模块头就是它存在的理由）。这条钉的是
    ///    "两支之间没有第二份映射"：失败那支拿到的就是**它**给的那一句。
    #[test]
    fn the_failure_message_comes_from_the_single_mapping() {
        let e = ClientError::Kernel {
            code: "delivery_fetch_failed".to_string(),
            message: "拉取交付清单失败：HTTP 404".to_string(),
        };
        // ⚠️ **判别力有多大，如实说**（修复轮 2 / 复审）：它**不是**恒真，但只比原来
        //    强一点点 —— 夹具的载荷与期望串是**同一句**，所以它只能杀掉"identity 被破坏"
        //    这一类改法（`DeliverySwitch` 自己给那句话加前缀 / 改写 / 截断），
        //    **测不到**"唯一映射给错了内容"（那要 `error_text` 的用例去管：
        //    `error_text.rs` 的 `the_kernel_message_is_verbatim_without_the_code_prefix`）。
        //    换句话说：这条守的是"**上屏那一句不加工**"，不是"映射对不对"。
        //    （上游那条同形；本波次没有把它删掉，是因为"上屏逐字"确实是这一支的判据。）
        assert_eq!(
            DeliverySwitch::Failed {
                message: error_text(&e)
            }
            .notice_text(),
            "拉取交付清单失败：HTTP 404"
        );
    }

    /// 上游 `theNoticeOfAFailureIsTheKernelTextVerbatim`。
    ///
    /// ⚠️ 这一条**不是**恒真的：它断言的是 `notice_text` 这个**实现**的行为
    ///    （把那句话加工一下、加个「换码失败：」前缀，这里立刻红）。
    ///    内核原文的形状是内核自己的事：前后空白、换行、中文标点都可能是它的一部分。
    #[test]
    fn the_notice_of_a_failure_is_the_kernel_text_verbatim() {
        let kernel_said = "  拉取交付清单失败：HTTP 404（第 2 行）\n";
        assert_eq!(
            DeliverySwitch::Failed {
                message: kernel_said.to_string()
            }
            .notice_text(),
            kernel_said,
            "那一行必须逐字是内核原文"
        );
    }

    /// 上游 `theNoticeOfASuccessNamesTheNewBatch`。
    ///
    /// 为什么这条有判别力：用户点「取消」之后那次换码照样会成功，并把整批换掉 ——
    /// 没有这一行，用户看到的是"我按了取消、界面自己换了一批"。
    #[test]
    fn the_notice_of_a_success_names_the_new_batch() {
        let text = DeliverySwitch::Switched {
            code: "C24-8_×_25WS024".to_string(),
        }
        .notice_text();
        assert!(
            text.contains("C24-8_×_25WS024"),
            "必须报出**新批次**的码（内核回的那个 code）"
        );
        // 成功那支不得混进任何失败措辞（反过来也一样，见上一条的逐字相等）。
        for bogus in ["失败", "错误", "重试"] {
            assert!(!text.contains(bogus), "成功那支出现了失败措辞「{bogus}」：{text}");
        }
    }

    /// 上游 `theNoticeTellsSuccessAndFailureApart`。
    #[test]
    fn the_notice_tells_success_and_failure_apart() {
        assert!(DeliverySwitch::Failed {
            message: "x".to_string()
        }
        .is_failure());
        assert!(
            !DeliverySwitch::Switched {
                code: "x".to_string()
            }
            .is_failure(),
            "反向的一半也要断言，否则恒 true 的变异体也能活下来"
        );
    }
}
