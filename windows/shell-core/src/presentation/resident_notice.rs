//! resident_notice —— 常驻提示行的**高度不变量**（散文 48 / 列表 80）。
//!
//! 上游：`macos/Sources/BenagenDownloader/Views/ResidentNotice.swift`。
//!
//! ---------------------------------------------------------------------------
//! 🔴 **为什么它从视图层搬进了 `presentation/`**（这是本模块存在的全部理由）
//! ---------------------------------------------------------------------------
//!
//! 上游那两个常量（`residentNoticeTextMaxHeight = 48`、`residentNoticeListMaxHeight = 80`）
//! **不在 `Presentation/` 里** —— 它们住在**视图层**（`Views/ResidentNotice.swift:90/:98`），
//! 而项目的铁律是"视图不单测"。搬进 `presentation/` 是**刻意的**：
//!
//!   我们这边的"视图"是 **JS**（Tauri 的 webview），而规格 §3.2 那条纪律是
//!   **"显示值全部由 Rust 决定，JS 不拼接任何面向用户的字符串"**。
//!   高度上限**也是显示值**：前端要做的事（"放得下就铺开、放不下就给可滚动视口"）
//!   取决于这两个数。把它们留在 JS 里，同一个数就会有**两个真相源**
//!   —— 样张的设计令牌一份、前端代码一份 —— 而两边一旦漂移，
//!   表现是"某一条提示把窗口底栏顶出去了"，一个**不报错、只能靠人眼发现**的失效。
//!
//!   ⇒ 常量与判决都由 Rust 交出去；JS 只负责按 `height` 摆位置、按 `scrolls` 决定
//!     要不要给滚动视口。
//!
//!   ---------------------------------------------------------------------------
//!   🔴 **而这句话今天只成立了一半，如实记账在这里（R-1，2026-09-20 的整分支审查）**
//!   ---------------------------------------------------------------------------
//!   上面那段说的"JS 只负责摆位置"成立，但**"常量由 Rust 交出去"这一半不成立**：
//!   `windows/web/css/tokens.css:133-134` 里**又写了一份**同样的字面量
//!   （`--notice-text-max-h: 48px` / `--notice-list-max-h: 80px`），而交付物里
//!   真正生效的是**那一份** —— 本模块的 `ResidentNotice::{TEXT,LIST}_MAX_HEIGHT`
//!   与 `bounded()` **在 `api/**` + `shell-win/**` 里零调用者**（正因如此，
//!   那条"两份真相源"的漂移**两边都不会红**：改这里屏幕上一点都不会变）。
//!   ⇒ **单一来源没做成**（那要求把这两个数发进载荷、由 JS 去改样式变量，
//!      而"无构建步骤"的前端改动不在本波次的范围里；如实记在这里，不装作做成了）。
//!   ⇒ 取而代之的是一条**判据**：`windows/scripts/check_presentation_mirrors.sh`
//!      （`test.sh` 第 0.45 步）判两边**数值一致**，并且判那两份令牌真的被
//!      `web/css/**` 用上、`tokens.css` 真的被 `index.html` 那条链加载。
//!   ⚠️ **要改这两个数，两边一起改** —— 判据会红，但红的是"人改了一半"，
//!      不是"这件事现在被自动同步了"。
//!
//! ---------------------------------------------------------------------------
//! 🔴 **不变量**：常驻提示行的高度，**不许由外部文本 / 外部数据的长度决定**。
//! ---------------------------------------------------------------------------
//!
//! 这不是预防性措施，是一次**真实的布局事故**（上游逐字记着现场）：
//! 改完下载目录、内核按新目录重启之后，「最下方的状态栏（全选 / 全不选 / 下载选中）」
//! 与「左侧侧边栏的批次摘要」一起从窗口里消失；**把窗口拉高就恢复**、
//! **把窗口整体上拖没用**、**点「恢复默认」也恢复**。
//!
//! 根因：那条**常驻**回执把**用户选的完整路径**放进了正文，而正文没有行数上限、
//! 还允许折行 ⇒ **路径越长这一行越高** ⇒ 窗口**内容**比窗口高 ⇒
//! 锚在内容底边的两样东西一起被裁掉（实测：同一句话在路径 68 字符时 15pt、
//! 1088 字符时 **195pt**）。
//!
//! ⚠️ **这条不变量的管辖范围就到这里为止：常驻提示行。**
//!    **它不许被读成一条全局规矩**（"只能单行"那种）—— 系统里就有两处**有意**的例外，
//!    它们**都没问题、别去"修"**：工具栏徽标那句「引擎不可用：<原因>」
//!    （它本来就该恒定一行，而且配了悬停看全文），以及底栏本身
//!    （它就是"恒定一行的高度锚点"，不可能反过来去顶别人）。
//!    判据是**位置上**的：**行在窗口内容的上方、且下面锚着别的东西** ⇒ 归这条管。
//!
//! ⚠️ **为什么是"有高度上限的可滚动视口"，而不是"行数截断"**：第 1 轮用的是
//!    `lineLimit(3)`，那**违反"失败原文必须完整可见"** —— 内核对"找不到内核可执行文件"
//!    的原话就带**三行候选路径**（最少 5 行），截成 3 行等于**用"看不见"换了"放得下"**。
//!    所以溢出时给的是**滚**：高度仍然有界，**全文/全部条目都在**，可滚动、可选中复制。
//!
//! ⚠️ **两类内容两种上限**：**散文**（内核 / 系统原文）48pt ≈ 3 行；
//!    **列表**（逐条拒绝理由）80pt ≈ 4–5 条。列表那档更宽是**故意的**：
//!    列表的价值就在"一眼看出是哪些文件没进去"，只露 2 条等于逼用户去滚。

/// 这块常驻内容是哪一类（决定上限）。
///
/// ⚠️ 上游**没有**这个枚举：那边是两个不同的视图类型（`BoundedNoticeText` 与
///    拒因列表各传各的常量）。这里把它显式建模，是为了让"哪一类用哪个上限"
///    成为**一处**判断，而不是让前端在三个调用点各挑一次常量。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoticeKind {
    /// 散文：内核原文 / 系统错误原文 / 壳写的通告。
    Prose,
    /// 列表：逐条拒绝理由那类**一行一条**的内容。
    List,
}

/// **高度有界**的一块内容在给定自然高下的呈现判决。
///
/// ⚠️ 它只有两个字段，而这个**形状本身就是判据**：这里**没有**"截断了多少"之类的
///    通道 —— "截断"这件事在类型上表达不出来。超出的部分只能从 [`Self::scrolls`]
///    那条路走（= 交给可滚动的视口），这是"不许丢证据"的结构化保证。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundedNotice {
    /// 这块内容在界面上占的高度（px）：自然高与上限的**较小者**。
    pub height: u32,
    /// 放不下吗（自然高 **>** 上限）。
    /// `true` ⇒ 调用方**必须**给一个可滚动的视口，全文/全部条目都在里面。
    pub scrolls: bool,
}

/// 常驻提示行的高度上限。
///
/// 上游 `Views/ResidentNotice.swift` 里那两个**全局常量**
/// （`residentNoticeTextMaxHeight` / `residentNoticeListMaxHeight`）—— 见模块头那段
/// "为什么它从视图层搬进来了"。
pub enum ResidentNotice {}

impl ResidentNotice {
    /// 常驻提示行里**散文**（内核 / 系统原文）的高度上限。
    ///
    /// 取 48pt 的算法（上游逐字）：一行约 15–16pt，48pt ≈ **3 行**
    /// （实测 3 行 = 45pt，留 3pt 余量）。常见的内核 / 系统错误是 1–2 行 ⇒
    /// 绝大多数情况下**根本不触发滚动**，只有"找不到内核"那类多行原文才滚 ——
    /// 而那正是"必须完整可见"的场合。
    ///
    /// ⚠️ 与样张的设计令牌是**同一个数**：`windows/design/a0-proof.html` 的
    ///    `--notice-text-max-h:48px`（那一行注释也点名了它对应上游的那个常量）。
    pub const TEXT_MAX_HEIGHT: u32 = 48;

    /// 常驻提示行里**列表**（逐条拒绝理由）的高度上限。
    ///
    /// 取 80pt 的算法（上游逐字）：一条拒绝理由约 16–17pt，80pt ≈ **4–5 条**。
    /// 比散文那一档宽是故意的（理由见模块头）。
    ///
    /// ⚠️ 与样张的设计令牌是**同一个数**：`--notice-list-max-h:80px`。
    pub const LIST_MAX_HEIGHT: u32 = 80;

    /// 这一类内容的上限。
    pub fn limit(kind: NoticeKind) -> u32 {
        match kind {
            NoticeKind::Prose => Self::TEXT_MAX_HEIGHT,
            NoticeKind::List => Self::LIST_MAX_HEIGHT,
        }
    }

    /// **放得下就按自然高铺开，放不下就钉在上限上（由调用方给可滚动的视口）。**
    ///
    /// 上游 `BoundedNoticeArea` / `BoundedNoticeLayout.sizeThatFits` 的那两行
    /// （`min(natural.height, maxHeight)`）—— 它把那个视图层 `Layout` 的**判决**
    /// 抽成一个可断言的值：
    ///   · `height = min(自然高, 上限)` —— "有界"；
    ///   · `scrolls = 自然高 > 上限` —— "不丢"（超出的部分交给滚动，不是截掉）。
    ///
    /// ⚠️ 边界口径是 **`>`**：自然高**正好等于**上限时**不滚**（放得下就铺开 ——
    ///    给一个高度正好、内容一行不多余的视口加滚动条，只会在右侧多出一条灰边）。
    ///
    /// ⚠️ 自然高由**调用方**给（它得量文本），本函数是纯计算。
    pub fn bounded(kind: NoticeKind, natural_height: u32) -> BoundedNotice {
        let limit = Self::limit(kind);
        BoundedNotice {
            height: natural_height.min(limit),
            scrolls: natural_height > limit,
        }
    }
}

// ---------------------------------------------------------------------------
// 测试（先写测试：它们会先红，见任务 6 的步骤 2）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! ⚠️ 上游**没有**这一份单测（那两个常量住在视图层，而视图不单测是本项目的硬约束）
    //! —— 这几条是本波次新增的，判据逐条对着 `ResidentNotice.swift` 里的实测记录
    //! 与 `windows/design/a0-proof.html` 的既有令牌抄。

    use super::{NoticeKind, ResidentNotice};

    /// 新增（控制者点名要的）：两条上限的**数值**。
    #[test]
    fn the_prose_height_limit_is_48_and_the_list_limit_is_80() {
        assert_eq!(ResidentNotice::TEXT_MAX_HEIGHT, 48);
        assert_eq!(ResidentNotice::LIST_MAX_HEIGHT, 80);
        // ⚠️ 上面两条是**常量**的判据；下面两条是**分派**的判据（`limit()` 有没有把
        //    两类接对）—— 两组各管一件事，不是同一句写两遍（审查次要 6 指出旧版把
        //    `TEXT_MAX_HEIGHT == 48` 写了两遍，第二遍还挂在"与样张令牌一致"的注释下）。
        assert_eq!(ResidentNotice::limit(NoticeKind::Prose), 48);
        assert_eq!(ResidentNotice::limit(NoticeKind::List), 80);
        assert_ne!(
            ResidentNotice::limit(NoticeKind::Prose),
            ResidentNotice::limit(NoticeKind::List),
            "两类不许接成同一个上限（那会让列表那一档静默按散文档截）"
        );
        // ⚠️ **与交付那份令牌（`windows/web/css/tokens.css` 的
        //    `--notice-text-max-h: 48px` / `--notice-list-max-h: 80px`）的一致性
        //    不由本模块守**：在这一层读前端文件会把"界面概念"引进纯逻辑 crate
        //    （本 crate 的章程禁止），所以判据落在**仓库级**那一侧 ——
        //    `windows/scripts/check_presentation_mirrors.sh`（`test.sh` 第 0.45 步）
        //    直接比这两个数与 `tokens.css` 里那两份令牌，并判令牌真的被用上。
        //    （这一段从前写的是"靠两边注释互指、**没有自动守卫**"——
        //      R-1 之后那句话已经过期，改掉它，别留一句对不上现状的自诊断。）
    }

    /// 新增：**放得下就按自然高铺开**（这是"有界"的另一半 —— 只有上限、没有自然高，
    /// 一块一行的提示也会被钉在 48pt 上）。
    #[test]
    fn content_that_fits_is_laid_out_at_its_natural_height() {
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::Prose, 0),
            super::BoundedNotice {
                height: 0,
                scrolls: false
            },
            "空内容不占高度（也不该被撑到上限）"
        );
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::Prose, 16),
            super::BoundedNotice {
                height: 16,
                scrolls: false
            }
        );
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::Prose, 47),
            super::BoundedNotice {
                height: 47,
                scrolls: false
            }
        );
    }

    /// 新增：**恰好到上限**不算放不下（边界口径：`>` 而不是 `>=`）。
    #[test]
    fn exactly_at_the_limit_does_not_scroll() {
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::Prose, 48),
            super::BoundedNotice {
                height: 48,
                scrolls: false
            },
            "正好 48 ＝ 三行，铺得下就铺开（实测：3 行 = 45pt，48 留了 3pt 余量）"
        );
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::List, 80),
            super::BoundedNotice {
                height: 80,
                scrolls: false
            }
        );
    }

    /// 新增（简报点名要的那一条）：**超出上限的内容是"滚"，不是"截"**。
    ///
    /// 🔴 这是第 2 轮改动的判据：第 1 轮用的是 `lineLimit(3)`，它**违反约束 4 / C-7**
    ///    —— 内核对"找不到内核可执行文件"的原话就带**三行候选路径**（最少 5 行），
    ///    把它截成 3 行等于**用"看不见"换了"放得下"**，而"失败原文必须完整可见"是硬要求。
    ///    所以溢出时给的是**可滚动视口**：高度仍然有界（不变量成立），
    ///    但**全文/全部条目都在**。
    ///
    /// 判别力：把 `scrolls` 改成恒 `false`（= 那种"截掉算了"的实现）这一条立刻红；
    /// 而这个类型**根本没有** `truncated` / `omitted` 之类的通道 ——
    /// "截断"这件事在**类型上**表达不出来（一条结构化保证，不是一句注释）。
    #[test]
    fn content_beyond_the_limit_scrolls_rather_than_being_truncated() {
        for (kind, limit) in [
            (NoticeKind::Prose, ResidentNotice::TEXT_MAX_HEIGHT),
            (NoticeKind::List, ResidentNotice::LIST_MAX_HEIGHT),
        ] {
            let one_over = ResidentNotice::bounded(kind, limit + 1);
            assert_eq!(one_over.height, limit, "超出一个像素也要钉在上限上");
            assert!(
                one_over.scrolls,
                "放不下 ⇒ 调用方必须给可滚动的视口（{kind:?}）"
            );

            let way_over = ResidentNotice::bounded(kind, 3000);
            assert_eq!(way_over.height, limit);
            assert!(way_over.scrolls);
        }

        // 实测过的那种最极端形态：一句 3000 字的原文。高度**与正文长度无关**。
        assert_eq!(
            ResidentNotice::bounded(NoticeKind::Prose, 3000),
            ResidentNotice::bounded(NoticeKind::Prose, 48 + 1),
            "同一句话在 3000 字时的高度必须与刚过界时**逐位相同**"
        );
    }

    /// 新增：两条上限的**顺序**（列表比散文宽）。
    ///
    /// 理由（上游逐字）：一条拒绝理由约 16–17pt，80pt ≈ **4–5 条**；而散文那档
    /// 48pt ≈ 3 行。比散文那一档宽是**故意的**：列表的价值就在"一眼看出是哪些文件没进去"，
    /// 只露 2 条等于逼用户去滚 —— 而"逐条摆出来"是约束 4 对这条回执的硬要求。
    #[test]
    fn the_list_limit_is_wider_than_the_prose_limit() {
        assert!(
            ResidentNotice::LIST_MAX_HEIGHT > ResidentNotice::TEXT_MAX_HEIGHT,
            "列表那档要更宽（4–5 条 vs 3 行）"
        );
    }
}
