import Foundation

// ---------------------------------------------------------------------------
// 面包屑与目录导航的**呈现模型**：全部是纯函数，无状态、无 SwiftUI 依赖
// （只 `import Foundation`）。
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件" —— 下面每一段都能写出断言，
//   而且有三条背着**约束 3**（路径原文逐字保真）与**简报点名的 `path_not_found` 回退**。
//
// ⚠️ **路径是清单原文，不得规范化**（约束 3）：不解码 `×`、不折叠 `//`、不改空白、
//    不 `standardizingPath`、不做百分号转义。本文件里没有任何一步碰过 `URL` /
//    `NSString.standardizingPath` / `removingPercentEncoding` —— 那是**故意**的。
// ---------------------------------------------------------------------------

/// 当前目录的层级链（面包屑）。根是空串，**不是** `"/"`。
public struct Breadcrumb: Equatable, Sendable {
    /// 一段面包屑：显示用的名字 + 点了要跳过去的那条完整路径。
    public struct Segment: Equatable, Sendable {
        /// 这一段的显示名（路径最后那一段的原文）。
        public let name: String
        /// 从根到这一段的完整路径（原文拼接，逐字）。
        public let path: String
    }

    /// 当前目录的路径原文。
    public let path: String
    /// 从根到当前目录的每一段。根（空路径）时是**空数组**。
    public let segments: [Segment]

    public init(path: String) {
        self.path = path

        // ⚠️ 空路径 = 根 ⇒ 没有任何一段。少了这条特判，`components(separatedBy:)`
        //    会回 `[""]`，于是图上面包屑会多出一个空白的、点了不知道去哪的一段。
        guard !path.isEmpty else {
            self.segments = []
            return
        }

        // ⚠️ 用 `components(separatedBy:)`（**保留空段**）而不是
        //    `split(separator: "/", omittingEmptySubsequences: true)`：
        //    后者会把 `a//b` 折成两段 —— 折叠就是"壳替内核改了路径"（约束 3）。
        var built = ""
        var out: [Segment] = []
        for (i, name) in path.components(separatedBy: "/").enumerated() {
            built = i == 0 ? name : built + "/" + name
            out.append(Segment(name: name, path: built))
        }
        self.segments = out
    }

    /// 上一层的路径。**顶层条目的上一层是根**（空串），根自己没有上一层。
    ///
    /// 它是"返回上一层"与 `path_not_found` 回退共用的那一步。
    public static func parentPath(ofCurrent path: String) -> String {
        // 没有 "/" ⇒ 自己就是顶层条目 ⇒ 上一层是根。空串走的也是这一支（返回自己）。
        guard let cut = path.lastIndex(of: "/") else { return "" }
        return String(path[path.startIndex..<cut])
    }

    /// 把一条目录项拼成它的完整路径：`父路径 + "/" + name`。
    ///
    /// ⚠️ **必需**，不是顺手：`list_dir` 的目录项**没有 `path` 键**（内核的 `entries_json`
    ///    只给目录发 `type`/`name`/`children_count`），文件项才有 `path`。
    ///    所以目录路径只能由壳拼 —— 不拼就没有路径可点进去。
    ///    根下的条目**不得出现前导 `/`**（那是另一个路径，内核会回 `path_not_found`）。
    public static func join(parent: String, name: String) -> String {
        parent.isEmpty ? name : parent + "/" + name
    }
}

// ---------------------------------------------------------------------------
// `list_dir` 失败之后：停在哪儿、说什么
// ---------------------------------------------------------------------------

/// 一次目录加载失败之后，界面该停在哪儿、该显示什么。
///
/// ⚠️ **这是简报点名的要求，不是顺手处理**：内核在路径**指向文件**时也报
///    `path_not_found`（`op_list_dir` 只在子树里找 `NodeType::Dir`），所以这个错误码
///    的含义是"这一层不是一个目录"，而不是"出了大事"。把它渲染成错误页会让用户
///    卡在一个死胡同里，而正确处置是**退回上一层并说明**。
public struct DirLoadFailure: Equatable, Sendable {
    /// 给用户看的那句话：**内核原文逐字**（约束 3：壳不加工、不编文案）。
    public let message: String
    /// 失败之后该停在哪一层。`path_not_found` 退到上一层，其余错误**原地不动**。
    public let path: String
    /// 非 nil 表示"这是从别处退回来的"，界面上要额外说一句。
    ///
    /// ⚠️ 这句话是**壳自己写的**（不是内核原文）：它描述的是**界面动作**（"我把你挪回上一层了"），
    ///    内核不知道这回事、也没有对应的话可说。约束 3 管的是"内核说了什么"，
    ///    不是"壳对自己的界面动作的说明"（同 `AppModel.busyReason` 的「正在加载交付清单…」）。
    public let notice: String?

    /// 是否真的退了一层。
    public var didFallBack: Bool { notice != nil }

    /// 退回上一层时那句提示。
    private static let fallbackNotice = "已返回上一层"

    /// 直接构造。给"不是 `CoreError` 的失败"用（视图侧那条通用 `catch`）——
    /// `listDir` 正常只抛 `CoreError`，但那条出口不能因此被编译器吞掉。
    public init(message: String, path: String, notice: String? = nil) {
        self.message = message
        self.path = path
        self.notice = notice
    }

    public static func of(_ error: CoreError, currentPath: String) -> DirLoadFailure {
        let message = AppModel.message(of: error)

        // 只认**结构化错误码** `path_not_found`，不看 `message` 的措辞（契约 §5.1）。
        if case .rpc(let code, _) = error, ErrorCode(rawValue: code) == .pathNotFound {
            let parent = Breadcrumb.parentPath(ofCurrent: currentPath)
            // 根上无路可退：停在根，且**不说**「已返回上一层」（没退成就不该这么说）。
            // 少了这条守卫，这里会拼出一个 "/" 或空路径，让下一次 `list_dir` 换一个新错误回来，
            // 用户看到的是"错误在变、位置不变"。
            if parent != currentPath {
                return DirLoadFailure(message: message, path: parent, notice: fallbackNotice)
            }
        }

        // 其余错误码（`engine_disconnected` / `invalid_params` / transport / 形状不符…）
        // 一律**原地不动**：它们跟"这一层存不存在"无关，把用户弹到上一层是壳在替内核
        // 解释错误（约束 1）。
        return DirLoadFailure(message: message, path: currentPath, notice: nil)
    }
}
