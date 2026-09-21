//! engine_status —— 引擎状态 → 正文 / 图标名 / tooltip 的纯映射。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/EngineStatusPresentation.swift`（逐字对位）。
//!
//! 为什么这些映射在这里而不在 `shell-win`（全局约束 8）：判据是"如果一段代码你能写出一个
//! 断言，它就不属于只负责画的那一层"——下面这几条映射每一条都能写出断言（`single_line`
//! 还背着约束 3 的一条有语义的边界），所以它们属于这里。`shell-win` 只剩绑定。
//!
//! ⚠️ 本模块**不引入任何绘制依赖**：需要颜色的那一步（上游的 `tint`）与"图标名怎么画成图形"
//!    都留在 `shell-win`。本模块只做纯字符串计算。

use crate::protocol::EngineState;

/// 握手超时后装进 [`EngineState::Unavailable`] 的那句话。
///
/// ⚠️ **W-6（落点改写；控制者裁定 EE）**：上游这一条常量挂在 `AppModel` 上
///    （`AppModel.swift:240` 的 `AppModel.handshakeTimeoutMessage`），而**本波次不移植
///    `AppModel`**（它是 macOS 侧的观察/编排对象）。
///    **裁定：家就定在这里，不搬。** 需要它的地方**一律 `use` 这条常量**、
///    **不要重写第二份字面量** —— 搬一次家就要把每一处 `use` 路径跟着改一遍，
///    那正是漂移的温床；两处各写一份字面量更糟（本文件与 `protocol.rs` 是同一处纪律的两个例子）。
///    路径**按消费者所在的 crate 取**（两处写法不同，是 crate 边界，不是两种规则）：
///      · `shell-core` **内部**（任务 15 的 `TransferRow`）：
///        `use crate::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;`
///      · **`shell-win`**（任务 17-20：那边没有 `crate::` 这条路，要经 crate 名进来）：
///        `use shell_core::presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;`
///
/// 内容是**壳自己写的**（理由同上游：超时是壳探测到的状况，内核一个 message 都没给，
/// 没有"原文"可登）。它同时是一个**判别式**：见 [`EngineStatusPresentation::is_handshake_timeout`]，
/// 所以它必须**逐字固定**，不要往里插值。
///
/// ⚠️ **这句话里的「5 秒」必须与壳实际的握手超时值一致。** 两个数字对不上，那句文案就是假话，
///    而**本模块自己没有任何东西会红**——它断言不了 `shell-win` 的时钟（断言在哪，见下）。
///    上游把这条耦合钉在**三处**：`AppModel.swift:215-216`（"改一个就改另一个"）、
///    `AppModelTimeoutTests.swift:329`（`defaultHandshakeTimeout == 5`）、
///    `:337`（正文含「5 秒」且与 `defaultHandshakeTimeout` 一致）。
///    `defaultHandshakeTimeout` 是 `AppModel` 的成员，**本波次没有移植 `AppModel`**
///    ⇒ **任务 17 已经在 Rust 侧重建了这条配对**（控制者裁定 FF）：
///      · **值的家**：`shell-win/src/main.rs` 的 `DEFAULT_HANDSHAKE_TIMEOUT`（5 秒）；
///      · 握手**只经** `shell-win` 的 `handshake()` 走那个值（那个函数刻意**不吃**
///        "超时值"参数：一参数化，"握手用了哪个值"就有了第二个、且测不出来的答案）；
///      · **钉住这条配对的是 `shell-win` 里的两条断言**：
///        `the_handshake_timeout_and_its_wording_agree`（改本常量的秒数**或**改那边的
///        超时值都会红）与 `the_handshake_itself_uses_the_shared_timeout_constant`
///        （握手真的走了那个值，不是"机制在、没人用"）。
///    ⚠️ **为什么断言在 `shell-win` 而不在这里**：crate 依赖是单向的（壳看得见本 crate，
///    本 crate 看不见壳），而这条配对的两个量**一边一个** —— 断言只能写在看得见两边的那一侧。
///    ⚠️ 这一处曾经写着"Windows 侧的握手**根本没有超时**、这句文案无处兑现"（任务 12 的
///    原话，当时是真的）。**那句话现在已经不成立**：任务 17 给握手加上了上界。
///    订正它的是任务 17（裁定 FF 里"顺带复核本注释"那一条）—— 别再照旧文改回去。
pub const HANDSHAKE_TIMEOUT_MESSAGE: &str = "内核无响应（等待超过 5 秒）";

/// 补充提示里那句"该干什么"（**两条**：是什么 + 怎么办；只有"是什么"的话，客户能做的
/// 就只剩"再等等"）。这句提示**只有客户去问才看得到**，所以它只补充。
///
/// ⚠️ 上游是 `EngineStatusPresentation.handshakeTimeoutTooltip` 这个 `static let`；
///    这里写成模块私有常量再经 [`EngineStatusPresentation::handshake_timeout_tooltip`] 导出
///    ——命名空间用法与上游一致（空 `enum` + 固有 `impl`，见 `presentation/mod.rs` 头部）。
const HANDSHAKE_TIMEOUT_TOOLTIP: &str = concat!(
    "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。",
    "请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
);

/// Swift `Character.isNewline` 的**逐字对位**：U+000A–U+000D、U+0085、U+2028、U+2029。
///
/// ⚠️ W-6：本函数在 Swift 源里没有对应物 —— 那里直接用 `\.isNewline`。写成显式集合是因为
///    **Rust 的 `char::is_whitespace` 不是同一个谓词**（它多含空格、制表符、U+00A0 等）：
///    若拿它当分隔符，`single_line("tab\tinside")` 会把制表符也折成空格，而上游明确不动它。
fn is_newline(c: char) -> bool {
    matches!(c, '\u{000A}'..='\u{000D}' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// 引擎状态的**呈现模型**（上游同名的空 `enum` + 固有 `impl`）。
///
/// ⚠️ `EngineState` **不在这里定义**：它在 [`crate::protocol::EngineState`]（**唯一一份**，
///    控制者裁决 C）。本模块只 `use` 它 —— 两处各定义一份是必须避免的。
///
/// 约束 4「不得静默失效」在正文上的**第一个落点**：内核没起来时，[`Self::text`] 里那句原因
/// 就是客户唯一的线索 —— 所以它必须**看得见**，不能只塞进 [`Self::tooltip`]。
pub enum EngineStatusPresentation {}

impl EngineStatusPresentation {
    /// 握手超时的补充提示（**只有客户去问才可见**，所以它只补充、不承担"失败出现在正文里"）。
    ///
    /// ⚠️ 这一处的文案是**壳自己写的**（不是"唯一一处"——上游 `AppModel` 的
    ///    `.protocolMismatch` 分支、`onKernelDeath`、`restartKernel` 里也有壳写的话；
    ///    其余一律"内核原文照登"，约束 3）。理由见 [`HANDSHAKE_TIMEOUT_MESSAGE`]。
    pub fn handshake_timeout_tooltip() -> &'static str {
        HANDSHAKE_TIMEOUT_TOOLTIP
    }

    /// 这一条 `reason` 是不是"握手超时"（而不是内核给的失败原因）？
    ///
    /// 判别式是**逐字相等**，不是包含 —— 内核原文（约束 3 照登）里万一出现同样的字样，
    /// 也不该被壳抢过来改写成自己的文案。钉住这条的是
    /// `engine_status_timeout_discriminator_is_exact_not_fuzzy`。
    ///
    /// 为什么需要它：[`EngineState`] 只有 `Unavailable` 这一个"不可用"分支，
    /// 而超时与"内核说它自己不可用"要给出完全不同的两句话 —— 前者是壳写的、可操作的；
    /// 后者是内核原文、一个字不改。判别式就是这两者的分水岭。
    pub fn is_handshake_timeout(reason: &str) -> bool {
        reason == HANDSHAKE_TIMEOUT_MESSAGE
    }

    /// 状态正文。
    ///
    /// `Unavailable` 的括号里是**内核原文照登**（约束 3：壳不加工），只做折行处理 ——
    /// 见 [`Self::single_line`]。
    ///
    /// ⚠️ 唯一的例外是握手超时：那时正文里必须有**那句话本身**（[`HANDSHAKE_TIMEOUT_MESSAGE`]），
    ///    不再套「引擎不可用：」的前缀 —— 它本身就是一句完整的话，而正文只有一行、
    ///    还要留给「内核无响应（等待超过 5 秒）」这个长度（约束 4：失败要**出现在正文里**，
    ///    不能只活在补充提示里）。
    pub fn text(engine: &EngineState) -> String {
        match engine {
            EngineState::Connecting => "正在连接内核…".to_string(),
            EngineState::NotStarted => "引擎未启动".to_string(),
            EngineState::Running => "运行中".to_string(),
            EngineState::Unavailable(reason) => {
                if Self::is_handshake_timeout(reason) {
                    return HANDSHAKE_TIMEOUT_MESSAGE.to_string();
                }
                format!("引擎不可用：{}", Self::single_line(reason))
            }
        }
    }

    /// 图标名。
    ///
    /// ⚠️ **W-6**：上游叫 `systemImage`、返回的是 **SF Symbol 名**（由 macOS 侧的
    ///    `EngineStatusBadge` 消费）。本 crate 的惯例名是 `icon_name`（见 `protocol.rs` 对
    ///    `VerifyClass` 的同一条惯例），任务简报的 API 块也这么写，故此处随简报名；
    ///    **返回的字符串逐字照抄上游**，一个字符都没改。
    ///
    /// ⚠️ Windows **没有 SF Symbols**：这些名字在 Windows 上是**语义标签**，
    ///    "标签 → 实际画成什么（文字/自绘图形）"那一步留在 `shell-win`——
    ///    与上游把 `tint`（要 `Color`）留在只负责画的那一层是同一条分工。
    ///    **本函数照样是纯函数、照样有断言**：
    ///    它钉住的是"每个状态有自己的名字"，而不是"名字长什么样"。
    pub fn icon_name(engine: &EngineState) -> &'static str {
        match engine {
            EngineState::Connecting => "questionmark.circle",
            EngineState::NotStarted => "circle.dashed",
            EngineState::Running => "bolt.fill",
            EngineState::Unavailable(_) => "exclamationmark.triangle.fill",
        }
    }

    /// 补充提示。
    ///
    /// ⚠️ `Unavailable` 这里给的是**完整原文**（含换行），而 [`Self::text`] 给的是折行版：
    ///    正文只有一行，折行只为排版；提示放得下换行，所以一个字符都不动。
    ///
    /// ⚠️ 唯一的例外仍是握手超时：内核一个字都没回，那句"原文"不存在 ——
    ///    这里给的是壳写的 [`Self::handshake_timeout_tooltip`]。
    pub fn tooltip(engine: &EngineState) -> String {
        match engine {
            EngineState::Connecting => "内核尚未握手（壳正在启动它）".to_string(),
            EngineState::NotStarted => {
                "下载引擎尚未启动 —— 添加下载任务后内核会启动它".to_string()
            }
            EngineState::Running => "下载引擎正在运行".to_string(),
            EngineState::Unavailable(reason) => {
                if Self::is_handshake_timeout(reason) {
                    return Self::handshake_timeout_tooltip().to_string();
                }
                reason.clone()
            }
        }
    }

    /// 把多行原文折成**一行**。
    ///
    /// 内核对 `coreNotFoundError` 的原话就带换行（「找不到内核可执行文件 benagen-core（找过：\n<三条路径>\n）」）。
    ///
    /// 契约（约束 3「壳不加工」在排版上的唯一让步）：
    ///   - 换行符折成**一个空格**；
    ///   - **除换行以外，一个字符都不增删改** —— 非换行的字符原样保留、顺序不变。
    ///   - 上游用 `split`（默认 `omittingEmptySubsequences: true`），所以**连续/首尾的换行会被合并**：
    ///     `"\n\na"` → `"a"`（不是 `"  a"`）。这是有意的：折行的目的是排版。
    ///
    /// ⚠️ W-6：这里显式 `.filter(|s| !s.is_empty())` 就是上面那个 `omittingEmptySubsequences`
    ///    的逐字对位，**少了它会多出一串空格**。另外 Swift 把 `\r\n` 当作**一个** `Character`
    ///    （一个分隔符），Rust 里它是两个 `char`（两个分隔符）——靠这个 filter 归一，
    ///    `"a\r\nb"` 两边都得到 `"a b"`（多余的那个空片段被丢掉，不是折成两个空格）。
    pub fn single_line(text: &str) -> String {
        text.split(is_newline)
            .filter(|piece| !piece.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/EngineStatusPresentationTests.swift`（13 条，逐条对位）。

    use super::{is_newline, EngineState, EngineStatusPresentation, HANDSHAKE_TIMEOUT_MESSAGE};
    use std::collections::BTreeSet;

    /// 上游 `hasNewline(_:)`：`String.contains(where: \.isNewline)`。
    fn has_newline(s: &str) -> bool {
        s.chars().any(is_newline)
    }

    // MARK: - text(for:)

    #[test]
    fn engine_status_text_pins_every_case() {
        assert_eq!(
            EngineStatusPresentation::text(&EngineState::Connecting),
            "正在连接内核…"
        );
        assert_eq!(
            EngineStatusPresentation::text(&EngineState::NotStarted),
            "引擎未启动"
        );
        assert_eq!(EngineStatusPresentation::text(&EngineState::Running), "运行中");
        assert_eq!(
            EngineStatusPresentation::text(&EngineState::Unavailable("内核崩了".to_string())),
            "引擎不可用：内核崩了"
        );
    }

    #[test]
    fn engine_status_text_distinguishes_all_four_cases() {
        let all: BTreeSet<String> = [
            EngineStatusPresentation::text(&EngineState::Connecting),
            EngineStatusPresentation::text(&EngineState::NotStarted),
            EngineStatusPresentation::text(&EngineState::Running),
            EngineStatusPresentation::text(&EngineState::Unavailable("x".to_string())),
        ]
        .into_iter()
        .collect();
        assert_eq!(all.len(), 4, "四个分支的正文必须两两不同：{all:?}");
    }

    #[test]
    fn engine_status_text_folds_newlines_but_keeps_the_reason_verbatim() {
        let reason = "找不到内核可执行文件 benagen-core（找过：\n/a\n/b\n/c）";
        let text = EngineStatusPresentation::text(&EngineState::Unavailable(reason.to_string()));

        assert_eq!(
            text,
            "引擎不可用：找不到内核可执行文件 benagen-core（找过： /a /b /c）"
        );
        assert!(!has_newline(&text), "正文里不许再有换行：{text:?}");
        assert!(text.starts_with("引擎不可用："));
    }

    // MARK: - 握手超时

    #[test]
    fn engine_status_text_and_tooltip_pin_the_handshake_timeout_copy() {
        let engine = EngineState::Unavailable(HANDSHAKE_TIMEOUT_MESSAGE.to_string());

        assert_eq!(HANDSHAKE_TIMEOUT_MESSAGE, "内核无响应（等待超过 5 秒）");
        assert_eq!(
            EngineStatusPresentation::text(&engine),
            "内核无响应（等待超过 5 秒）"
        );
        assert_eq!(
            EngineStatusPresentation::tooltip(&engine),
            concat!(
                "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。",
                "请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
            )
        );
        assert_eq!(
            EngineStatusPresentation::handshake_timeout_tooltip(),
            concat!(
                "内核可能正卡在系统授权对话框上（例如\"想访问下载文件夹\"）。",
                "请查看屏幕上是否有该对话框并点「允许」，然后点「重试」。"
            )
        );
        assert_eq!(
            EngineStatusPresentation::icon_name(&engine),
            "exclamationmark.triangle.fill"
        );
    }

    #[test]
    fn engine_status_timeout_discriminator_is_exact_not_fuzzy() {
        let near_miss = format!("{HANDSHAKE_TIMEOUT_MESSAGE}。");
        assert_eq!(
            EngineStatusPresentation::text(&EngineState::Unavailable(near_miss.clone())),
            format!("引擎不可用：{near_miss}")
        );
        assert_eq!(
            EngineStatusPresentation::tooltip(&EngineState::Unavailable(near_miss.clone())),
            near_miss
        );
        assert!(!EngineStatusPresentation::is_handshake_timeout(&near_miss));
        assert!(EngineStatusPresentation::is_handshake_timeout(
            HANDSHAKE_TIMEOUT_MESSAGE
        ));
    }

    // MARK: - icon_name(engine)

    #[test]
    fn engine_status_icon_name_pins_every_case() {
        assert_eq!(
            EngineStatusPresentation::icon_name(&EngineState::Connecting),
            "questionmark.circle"
        );
        assert_eq!(
            EngineStatusPresentation::icon_name(&EngineState::NotStarted),
            "circle.dashed"
        );
        assert_eq!(
            EngineStatusPresentation::icon_name(&EngineState::Running),
            "bolt.fill"
        );
        assert_eq!(
            EngineStatusPresentation::icon_name(&EngineState::Unavailable("x".to_string())),
            "exclamationmark.triangle.fill"
        );
    }

    #[test]
    fn engine_status_icon_name_distinguishes_all_four_cases() {
        let all: BTreeSet<&str> = [
            EngineStatusPresentation::icon_name(&EngineState::Connecting),
            EngineStatusPresentation::icon_name(&EngineState::NotStarted),
            EngineStatusPresentation::icon_name(&EngineState::Running),
            EngineStatusPresentation::icon_name(&EngineState::Unavailable("x".to_string())),
        ]
        .into_iter()
        .collect();
        assert_eq!(all.len(), 4, "四个图标名必须两两不同：{all:?}");
    }

    // MARK: - tooltip(engine)

    #[test]
    fn engine_status_tooltip_pins_every_case() {
        assert_eq!(
            EngineStatusPresentation::tooltip(&EngineState::Connecting),
            "内核尚未握手（壳正在启动它）"
        );
        assert_eq!(
            EngineStatusPresentation::tooltip(&EngineState::NotStarted),
            "下载引擎尚未启动 —— 添加下载任务后内核会启动它"
        );
        assert_eq!(
            EngineStatusPresentation::tooltip(&EngineState::Running),
            "下载引擎正在运行"
        );
    }

    #[test]
    fn engine_status_tooltip_keeps_reason_untouched_while_text_folds_it() {
        let reason = "找不到内核可执行文件（找过：\n/a\n/b）";
        assert_eq!(
            EngineStatusPresentation::tooltip(&EngineState::Unavailable(reason.to_string())),
            reason
        );
        assert_ne!(
            EngineStatusPresentation::tooltip(&EngineState::Unavailable(reason.to_string())),
            EngineStatusPresentation::text(&EngineState::Unavailable(reason.to_string()))
        );
        assert!(!has_newline(&EngineStatusPresentation::text(
            &EngineState::Unavailable(reason.to_string())
        )));
        assert!(has_newline(&EngineStatusPresentation::tooltip(
            &EngineState::Unavailable(reason.to_string())
        )));
    }

    // MARK: - single_line(_:)

    #[test]
    fn single_line_folds_newlines_into_exactly_one_space() {
        assert_eq!(EngineStatusPresentation::single_line("a\nb"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\nb\nc"), "a b c");
        assert_eq!(EngineStatusPresentation::single_line("a\r\nb"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\rb"), "a b");
        // ⚠️ 下面**五个**码位是**本侧补钉的**（W-6）：上游的这条测试只覆盖了 `\n`/`\r`/`\r\n`，
        //    而 `Character.isNewline` 的集合比这大 —— U+0085（NEL）、U+2028（行分隔符）、
        //    U+2029（段分隔符）以及 U+000B/U+000C 也在内。
        //    补钉之前，`is_newline` 若把这几个漏掉或写错，**没有任何测试会红**；
        //    证据是一次性的（`swiftc` 对 16 个码位、37 用例的差分对拍 ⇒ 零差异），
        //    只有把断言留在测试里它才留得下来。
        //    （`\u{b}`/`\u{c}` **不是** `is_newline` 里 `..=` 那一截的两端 —— 那一截是
        //     `U+000A..=U+000D`，两端由上面 `"a\nb"`（U+000A）与 `"a\rb"`（U+000D）两条钉住；
        //     这两个是区间**内部**的码位，补钉它们是为了让"中间那几个也在集合里"这句话
        //     有断言，不是为了让端点有人守。）
        assert_eq!(EngineStatusPresentation::single_line("a\u{85}b"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\u{2028}b"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\u{2029}b"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\u{b}b"), "a b");
        assert_eq!(EngineStatusPresentation::single_line("a\u{c}b"), "a b");
        // 反向：**不是**换行的空白不许折（U+00A0 是不换行空格，`is_whitespace` 里有它、
        // `isNewline` 里没有 —— 这正是本侧不能用 `char::is_whitespace` 的第二个理由）。
        assert_eq!(EngineStatusPresentation::single_line("a\u{a0}b"), "a\u{a0}b");
    }

    #[test]
    fn single_line_leaves_newline_free_text_completely_untouched() {
        let samples = [
            "",
            "abc",
            "引擎不可用：内核崩了",
            "a  b",
            "  前导与尾随空格  ",
            "mixed 中英文 ASCII 123 ！@#",
            "emoji 🙂 也要原样",
            "tab\tinside",
        ];
        for s in samples {
            assert_eq!(EngineStatusPresentation::single_line(s), s, "输入 {s:?} 被改动了");
        }
    }

    #[test]
    fn single_line_preserves_every_non_newline_character_in_order() {
        let input = "找过：\n/path/a\n/path/b\n";
        let out = EngineStatusPresentation::single_line(input);

        assert!(!has_newline(&out), "换行一个不剩：{out:?}");
        let strip = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
        assert_eq!(strip(&out), strip(input), "非换行字符一个不少、顺序不变");
        assert_eq!(out, "找过： /path/a /path/b");
    }

    #[test]
    fn single_line_keeps_the_answer_one_line() {
        assert_eq!(EngineStatusPresentation::single_line("\n\na"), "a");
        assert_eq!(EngineStatusPresentation::single_line("a\n\n"), "a");
        assert_eq!(EngineStatusPresentation::single_line("\n"), "");
        for s in ["\n\na", "a\n\n", "\n", "a\n\nb"] {
            let out = EngineStatusPresentation::single_line(s);
            assert!(!has_newline(&out));
            assert_eq!(out.trim(), out, "{s:?} 的结果有前导/尾随空白：{out:?}");
        }
    }
}
