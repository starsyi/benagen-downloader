//! breadcrumb —— 路径层级链（面包屑）与目录导航失败的呈现模型。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/Breadcrumb.swift`（逐字对位）。
//! 上游测试：`macos/Tests/BenagenCoreKitTests/BreadcrumbTests.swift`（9 条，逐条对位）。
//!
//! ⚠️ **路径是清单原文，不得规范化**（约束 3）：不解码 `×`、不折叠 `//`、不改空白、
//!    不做百分号转义。本文件里没有任何一步碰过路径规范化 —— 那是**故意**的，
//!    `repeated_slashes_are_not_collapsed` 钉着它（契约 §3.1 同源）。
//!
//! ⚠️ **纪律（`lib.rs` 的章程）**：本 crate 是纯逻辑层，注释里不出现章程点名的那类词。
//!    上游那些呈现侧的说法在这里一律换成中性表述（"层级链""那一层"…）——
//!    **换掉的只是措辞，判据一个字没动**。

use serde::Serialize;
use crate::client::ClientError;
use crate::presentation::error_text::error_text;

// ---------------------------------------------------------------------------
// 当前目录的层级链
// ---------------------------------------------------------------------------

/// 当前目录的层级链。根是空串，**不是** `"/"`。
///
/// 上游是 `Breadcrumb`（`Equatable, Sendable`）；这里对位成 `Clone, PartialEq, Eq, Debug`
/// ——同 `browser_primary_action.rs` 的记账：`Sendable` 在 Rust 里由类型系统默认保证，
/// `Debug` 是断言失败时要看得见值才加的。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Breadcrumb {
    /// 当前目录的路径原文。
    pub path: String,
    /// 从根到当前目录的每一段。根（空路径）时是**空数组**。
    pub segments: Vec<Segment>,
}

/// 一段层级链：显示用的名字 + 点了要跳过去的那条完整路径。
///
/// 上游是嵌套的 `Breadcrumb.Segment`；Rust 没有嵌套类型，所以它平铺在模块里
/// （同 `download_targets.rs` 的 `Rejection`），**字段名一字不改**。
///
/// ⚠️ **字段名以源为准（W-6 记账）**：任务 11 的简报 `:23` 写的是
///    `pub label: String`，而源里是 `Breadcrumb.Segment.name`（`Breadcrumb.swift:23`）
///    ——按**源是权威**取 `name`（简报与源不符的第 5 处，见任务 11 报告 §4）。
///    改叫 `label` 的代价很具体：本 crate 里凡是 `label` 都指"那句要显示的话"
///    （见 `RowStateStyle::label`），而这一格是**路径最后一段的原文**，不是话。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Segment {
    /// 这一段的显示名（路径最后那一段的原文）。
    pub name: String,
    /// 从根到这一段的完整路径（原文拼接，逐字）。
    pub path: String,
}

impl Breadcrumb {
    pub fn new(path: &str) -> Self {
        // ⚠️ 空路径 = 根 ⇒ 没有任何一段。少了这条特判，按 `'/'` 切分的结果是 `[""]`，
        //    于是层级链上会多出一个空白的、点了不知道去哪的一段。
        if path.is_empty() {
            return Self {
                path: path.to_string(),
                segments: Vec::new(),
            };
        }

        // ⚠️ 用 `split('/')`（**保留空段**）而不是"切分后过滤掉空串"：
        //    后者会把 `a//b` 折成两段 —— 折叠就是"壳替内核改了路径"（约束 3）。
        let mut built = String::new();
        let mut out: Vec<Segment> = Vec::new();
        for (i, name) in path.split('/').enumerate() {
            if i == 0 {
                built = name.to_string();
            } else {
                built = format!("{built}/{name}");
            }
            out.push(Segment {
                name: name.to_string(),
                path: built.clone(),
            });
        }
        Self {
            path: path.to_string(),
            segments: out,
        }
    }

    /// 上一层的路径。**顶层条目的上一层是根**（空串），根自己没有上一层。
    ///
    /// 它是"返回上一层"与 `path_not_found` 回退共用的那一步。
    ///
    /// ⚠️ **名字偏离（W-6 记账）**：上游是 `parentPath(ofCurrent:)`，任务 11 的简报 `:27`
    ///    把它写成 `parent_path(of_current:)`。Rust 侧没有参数标签，那个 `of_current`
    ///    在调用点会消失，于是"算的是谁的上一级"必须留在**函数名**里：
    ///    `parent_path_of_current(p)` 读得出 `p` 是当前路径，`parent_path(p)` 读不出。
    ///    **判据与返回值一字未改**（同批签名的 `TransferRow::new` 对同类偏离有同样的记账）。
    pub fn parent_path_of_current(current: &str) -> String {
        // 没有 "/" ⇒ 自己就是顶层条目 ⇒ 上一层是根。空串走的也是这一支（返回自己）。
        // ⚠️ 按**最后一个** "/" 切（`rfind`），不是第一个：`a/b/c` 的上一层是 `a/b`。
        //    '/' 是单字节，所以字节下标切出来的边界一定落在字符边界上。
        match current.rfind('/') {
            None => String::new(),
            Some(cut) => current[..cut].to_string(),
        }
    }

    /// 把一条目录项拼成它的完整路径：`父路径 + "/" + name`。
    ///
    /// ⚠️ **必需**，不是顺手：`list_dir` 的目录项**没有 `path` 键**（内核的 `entries_json`
    ///    只给目录发 `type`/`name`/`children_count`），文件项才有 `path`。
    ///    所以目录路径只能由壳拼 —— 不拼就没有路径可点进去。
    ///    根下的条目**不得出现前导 `/`**（那是另一个路径，内核会回 `path_not_found`）。
    pub fn join(parent: &str, name: &str) -> String {
        if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        }
    }
}

// ---------------------------------------------------------------------------
// `list_dir` 失败之后：停在哪儿、说什么
// ---------------------------------------------------------------------------

/// 一次目录加载失败之后，该停在哪儿、该显示什么。
///
/// ⚠️ **这是简报点名的要求，不是顺手处理**：内核在路径**指向文件**时也报
///    `path_not_found`（`op_list_dir` 只在子树里找 `NodeType::Dir`），所以这个错误码
///    的含义是"这一层不是一个目录"，而不是"出了大事"。把它当成一个走不出去的错误
///    会让用户卡在死胡同里，而正确处置是**退回上一层并说明**。
///
/// ⚠️ **形态偏离（W-6）**：上游的 `of` 收 `CoreError`（`.rpc` / `.transport` /
///    `.malformedResponse` 三种变体各带一句文本），认的是 `ErrorCode(rawValue: code)`
///    这个**强类型**枚举。Rust 侧对应的是 [`ClientError`]：`.rpc` ⇒
///    [`ClientError::Kernel`]（带结构化 `code`），`.transport(msg)` ⇒
///    [`ClientError::KernelGone`]（`msg` 那几句在 `client.rs` 里各是一个变体，
///    不是任意字符串），`.malformedResponse` ⇒ [`ClientError::MalformedResponse`]
///    （多带一个 `cause`）。**这一层的判据没有变**：只认结构化错误码
///    [`crate::protocol::codes::PATH_NOT_FOUND`]，不看 `message` 的措辞（契约 §5.1）。
///
/// ⚠️ 上游那个 `public init(message:path:notice:)`（给"不是 `CoreError` 的失败"用）
///    在这里**没有对应物**：Rust 侧没有"任意错误"这条路径。字段全是 `pub`，
///    真要手工构造时用结构体字面量，别另加一个只会被绕过的构造函数。
// ⚠️ `Serialize` 是批次 C 的审查修复轮补的（本计划第七版修订）：`DirLoadFailure` **是界面值**
//    —— 它是"这一层没读成 / 该退回哪一层 / 有没有退过"这三件事的成句 —— 但在那之前它是
//    18 个界面值里**唯一没有派生**的一个，于是 `shell-win/src/server/routes.rs` 只能把三个
//    字段**手写摊开**进 JSON（那正是那条"不手写投影"的纪律要禁的形态，而它当时是**代码里
//    没写出来的一个例外**，只在报告里说过）。
//    ⇒ 补上派生，那处摊开就消失了；判据一个字节没动（字段名本来就是给前端看的那三个）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct DirLoadFailure {
    /// 给用户看的那句话：**内核原文逐字**（约束 3：壳不加工、不编文案）。
    pub message: String,
    /// 失败之后该停在哪一层。`path_not_found` 退到上一层，其余错误**原地不动**。
    pub path: String,
    /// `Some` 表示"这是从别处退回来的"，要额外说一句。
    ///
    /// ⚠️ 这句话是**壳自己写的**（不是内核原文）：它描述的是**壳自己的动作**
    ///    （"我把你挪回上一层了"），内核不知道这回事、也没有对应的话可说。约束 3 管的是
    ///    "内核说了什么"，不是"壳对自己动作的说明"。
    pub notice: Option<String>,
}

impl DirLoadFailure {
    /// 是否真的退了一层。
    pub fn did_fall_back(&self) -> bool {
        self.notice.is_some()
    }

    /// 退回上一层时那句提示。
    const FALLBACK_NOTICE: &'static str = "已返回上一层";

    pub fn of(error: &ClientError, current_path: &str) -> Self {
        let message = error_text(error);

        // 只认**结构化错误码** `path_not_found`，不看 `message` 的措辞（契约 §5.1）。
        if let ClientError::Kernel { code, .. } = error {
            if code == crate::protocol::codes::PATH_NOT_FOUND {
                let parent = Breadcrumb::parent_path_of_current(current_path);
                // 根上无路可退：停在根，且**不说**「已返回上一层」（没退成就不该这么说）。
                // 少了这条守卫，这里会拼出一个 "/" 或空路径，让下一次 `list_dir` 换一个新错误回来，
                // 用户看到的是"错误在变、位置不变"。
                if parent != current_path {
                    return Self {
                        message,
                        path: parent,
                        notice: Some(Self::FALLBACK_NOTICE.to_string()),
                    };
                }
            }
        }

        // 其余错误码（`engine_disconnected` / `invalid_params` / transport / 形状不符…）
        // 一律**原地不动**：它们跟"这一层存不存在"无关，把用户弹到上一层是壳在替内核
        // 解释错误（约束 1）。
        Self {
            message,
            path: current_path.to_string(),
            notice: None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! 上游 `BreadcrumbTests.swift`（9 条，逐条对位）。
    //!
    //! ⚠️ 夹具里的 `C24-8_×_25WS024` / `QC 图.png` 是**真清单里的原文**
    //!    （内核夹具：`core/src/delivery.rs` 的 `file_url_escapes_like_server`）。
    //!    带 `×` 与空格的那几条就是约束 3 的现场：路径不得被解码、不得被转义。

    use super::{Breadcrumb, DirLoadFailure};
    use crate::client::ClientError;
    use crate::presentation::error_text::error_text;

    /// 上游 `splitsPathIntoSegmentsForTheCrumbs`。
    #[test]
    fn splits_path_into_segments_for_the_crumbs() {
        let b = Breadcrumb::new("client-test/C24-8_×_25WS024/Figure");

        assert_eq!(
            b.segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["client-test", "C24-8_×_25WS024", "Figure"]
        );
        // 每一段带的是**到它为止**的完整路径（点了就能跳过去）。
        assert_eq!(
            b.segments.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
            [
                "client-test",
                "client-test/C24-8_×_25WS024",
                "client-test/C24-8_×_25WS024/Figure"
            ]
        );
        assert_eq!(
            b.path, "client-test/C24-8_×_25WS024/Figure",
            "crumbs 记着的是它自己那条原文路径"
        );
    }

    /// 上游 `rootIsTheEmptyPath`。
    #[test]
    fn root_is_the_empty_path() {
        // 内核把根目录的 `path` 回成**空串**，不是 `"/"`（`ListDirResult.path` 的注释）。
        assert!(Breadcrumb::new("").segments.is_empty());
    }

    /// 上游 `nonASCIIAndSpacesSurviveUntouched`。
    #[test]
    fn non_ascii_and_spaces_survive_untouched() {
        // 约束 3：× 与空格是清单原文，不得解码、不得转义。
        assert_eq!(
            Breadcrumb::new("C24-8_×_25WS024/Figure/QC 图.png")
                .segments
                .last()
                .map(|s| s.path.as_str()),
            Some("C24-8_×_25WS024/Figure/QC 图.png")
        );
        // 反向的一半：**没有**被拼成 %C3%97 / %20 那一类转义产物（否则上面那条
        // 只要"整串照抄"就能过，`×` 是否被处理过就看不出来了）。
        assert_eq!(
            Breadcrumb::new("C24-8_×_25WS024")
                .segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["C24-8_×_25WS024"]
        );
        assert_eq!(
            Breadcrumb::new("QC 图.png")
                .segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["QC 图.png"]
        );
    }

    /// 上游 `repeatedSlashesAreNotCollapsed`。
    ///
    /// ⚠️ 与契约 §3.1 同源：**不得规范化路径**。`split(separator: "/")` 加
    /// "过滤空串"这类写法会把 `a//b` 折成两段，于是这一段在层级链里凭空消失一格 ——
    /// 折叠就是"壳替内核改了路径"。
    #[test]
    fn repeated_slashes_are_not_collapsed() {
        let b = Breadcrumb::new("a//b");
        assert_eq!(b.segments.len(), 3, "空段也是一个段（不折叠）");
        assert_eq!(
            b.segments.last().map(|s| s.path.as_str()),
            Some("a//b"),
            "逐段拼回去必须**逐字**等于原文"
        );
    }

    /// 上游 `parentOfATopLevelEntryIsRoot`。
    #[test]
    fn parent_of_a_top_level_entry_is_root() {
        assert_eq!(Breadcrumb::parent_path_of_current("a"), "");
        assert_eq!(Breadcrumb::parent_path_of_current("a/b/c"), "a/b");
        // 根自己没有上一级（返回它自己），否则「返回上一层」在根上会造出一个 "/"。
        assert_eq!(Breadcrumb::parent_path_of_current(""), "");
    }

    /// 上游 `joiningADirEntryRebuildsItsPath`。
    #[test]
    fn joining_a_dir_entry_rebuilds_its_path() {
        // ⚠️ `list_dir` 的目录项**没有 `path` 键**（判别键是 `"type"`，没有 `is_dir` 布尔）——
        //    目录的路径只能由壳自己拼：父路径 + "/" + name。
        assert_eq!(Breadcrumb::join("a/b", "子目录"), "a/b/子目录");
        assert_eq!(Breadcrumb::join("", "top"), "top"); // 根下不得出现前导 "/"
        // 名字里带空格与非 ASCII 时逐字保真（不得转义）。
        assert_eq!(
            Breadcrumb::join("client-test", "C24-8_×_25WS024"),
            "client-test/C24-8_×_25WS024"
        );
    }

    /// 上游 `pathNotFoundGoesUpOneLevel`。
    #[test]
    fn path_not_found_goes_up_one_level() {
        // ⚠️ 内核在**路径指向文件**时也报 `path_not_found`（`op_list_dir` 只在子树里找目录）。
        //    所以这个错误不是"停在错误页"的理由 —— 退回上一层并说明，才是简报要的处置。
        let e = ClientError::Kernel {
            code: "path_not_found".to_string(),
            message: "清单里没有目录 \"a/b/c\"".to_string(),
        };
        let f = DirLoadFailure::of(&e, "a/b/c");

        assert_eq!(f.path, "a/b", "退回上一层");
        assert_eq!(f.message, "清单里没有目录 \"a/b/c\"", "内核原文逐字照登（约束 3）");
        assert_eq!(
            f.notice.as_deref(),
            Some("已返回上一层"),
            "退回去了就得说一声，否则用户不知道自己怎么换了地方"
        );
        assert!(f.did_fall_back());
    }

    /// 上游 `pathNotFoundAtTheRootStaysAtTheRoot`。
    #[test]
    fn path_not_found_at_the_root_stays_at_the_root() {
        // 根上无路可退：必须停在根，不得拼出一个前导 "/" 的假路径（那会让下一次
        // `list_dir` 换一个新错误回来，用户看到的是错误在变、位置不变）。
        let f = DirLoadFailure::of(
            &ClientError::Kernel {
                code: "path_not_found".to_string(),
                message: "清单里没有目录 \"\"".to_string(),
            },
            "",
        );
        assert_eq!(f.path, "");
        assert!(!f.did_fall_back(), "没退成就不该说「已返回上一层」");
    }

    /// 上游 `otherErrorsStayPutAndKeepTheKernelWording`。
    #[test]
    fn other_errors_stay_put_and_keep_the_kernel_wording() {
        // 其余错误码一律**原地不动**：传输断了、参数错了，都跟"这一层存不存在"无关，
        // 把用户弹到上一层是壳在替内核解释错误（约束 1）。
        //
        // ⚠️ 与上游四条夹具的对位（**形状收窄**，理由见文件头的 W-6 记账）：
        //    `.rpc(code:message:)` ⇒ [`ClientError::Kernel`]；
        //    上游 `.transport("内核进程已退出（管道结束）")` ⇒ [`ClientError::KernelGone`]
        //    —— 那句话正是它在 `client.rs` 里的 `Display` 正文（逐字相同）；
        //    上游 `.malformedResponse("…")` ⇒ [`ClientError::MalformedResponse`]（多了 `cause`）。
        let bad_shape = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let cases = [
            ClientError::Kernel {
                code: "engine_disconnected".to_string(),
                message: "下载引擎已断开".to_string(),
            },
            ClientError::Kernel {
                code: "invalid_params".to_string(),
                message: "参数不合法".to_string(),
            },
            ClientError::KernelGone,
            ClientError::MalformedResponse {
                line_prefix: "id 7 的 result 与 ListDirResult.self 的形状不符".to_string(),
                cause: bad_shape,
            },
        ];
        for e in &cases {
            let f = DirLoadFailure::of(e, "a/b");
            assert_eq!(f.path, "a/b", "{e} 不该把用户弹走");
            assert_eq!(f.notice, None);
            assert!(!f.did_fall_back());
            // ⚠️ **这条断言不是"怎么改都不会红"的恒真判据（控制者裁决 LL）**：
            //    它钉的是 [`DirLoadFailure::of`] 里**不另加措辞** —— 把实现改成自己拼一句
            //    （哪怕只是加个前缀、折个行），这一条立刻红。它防的正是
            //    "同一句话出现第二份来源"。
            //    形态与上游逐字同形（源写的是 `f.message == AppModel.message(of: e)`），
            //    按裁决 T"不得意译"，这里**不**改写成一个写死的字面量。
            assert_eq!(f.message, error_text(e), "原文逐字（约束 3）");
        }
    }
}
