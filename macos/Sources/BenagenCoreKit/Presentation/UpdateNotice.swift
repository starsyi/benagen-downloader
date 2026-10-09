import Foundation

// ---------------------------------------------------------------------------
// 「有新版本」那条提示的呈现模型（规格 §3/§4）。
//
// 全局约束 8：把内核数据变成界面值的**纯计算**都放 `Presentation/`，每条都要有单测；
// 视图里只留绑定与渲染分派。本文件**不 `import SwiftUI`、也不 `import AppKit`** ——
// 「去下载」那句 `NSWorkspace.shared.open` 因此不在本文件里（裁定 R15：它落在视图层，
// 见下面的 `url` 与 `UpdateNotice.of`）。
// ---------------------------------------------------------------------------

/// 内核 `update_status` / `update_set_enabled` 回的那一份状态
/// （`core/src/kernel.rs` 的 `op_update_status`）。
///
/// ⚠️ **为什么这个类型要显式写出来（裁定 R22）**：内核回的是 **snake_case**
///    （`has_newer`），而 Swift 属性名是 camelCase。壳的解码走
///    `CoreJSON.decoder`（`.convertFromSnakeCase`）—— 它**先把 JSON 键转成 camelCase**，
///    再拿它去比对 `CodingKeys` 的 `stringValue`（`JSONValue.swift:104-107` 记着这条交互，
///    `HelloResult` 的 `case protocolVersion = "protocol"` 是现成的例子）。
///    ⇒ 下面的 `CodingKeys` 取值必须是**转换后**的 camelCase；写成 `"has_newer"`
///    是**双重转换**、一定解不开。**没有映射**（或写错）的症状是"**永远不提示**"，
///    而它**没有任何东西会红**（`refreshUpdateStatus` 那侧静默）——
///    `UpdateNoticeTests.解码内核的蛇形键…` 就是为这条立的桩。
///
/// ⚠️ **`enabled` 只在内核的 `update_status` 里**（不在 `settings.json`，规格 §3）：
///    设置页那个勾选框要显示**当前值**，所以它必须随状态一起解进来。
public struct UpdateStatus: Equatable, Decodable, Sendable {
    /// 内核**现算**的"有没有新版"（线上 `has_newer`）。**这是唯一的判据**（裁定 R20）——
    /// **不是**"`url` 非空"：内核在没有新版时**照样可能**给一条指向**已装版本**的链接，
    /// 按 `url` 渲染会把客户引去下载他自己已经装着的那个版本。
    public let hasNewer: Bool
    /// 数据源里那个版本号的**原文**（线上 `latest`，如 `"v0.2.5"`）；
    /// 内核没有可报的版本时是 `null`（`op_update_status` 的 `last_seen` 是 `Option<String>`）。
    public let latest: String?
    /// 内核**自己的**版本（线上 `current`，来自 `env!("CARGO_PKG_VERSION")`）。
    /// 比较与显示同源 —— 内核保证它一定在。
    public let current: String
    /// 内核**拼好**的下载链接（线上 `url`）；没有可解析的版本时是 `null`。
    /// ⚠️ 壳**一个字符都不拼**（约束 3）—— 这里只把它解进来。
    public let url: String?
    /// 「启动时检查更新」那个开关的**当前值**（规格 §4）。**默认开**。
    public let enabled: Bool

    /// 显式构造器。`enabled` 放在**最后且带默认值 `true`**，于是那句"开关默认开"
    /// 在调用点上是看得见的（简报里那三处 4 参调用点也原样可用）。
    public init(hasNewer: Bool, latest: String?, current: String, url: String?,
                enabled: Bool = true) {
        self.hasNewer = hasNewer
        self.latest = latest
        self.current = current
        self.url = url
        self.enabled = enabled
    }

    private enum CodingKeys: String, CodingKey {
        // ⚠️ `"hasNewer"` 是**转换后**的键名（`.convertFromSnakeCase` 已经把线上的
        //    `has_newer` 变成了 `hasNewer`）。**别**改成 `"has_newer"` —— 那是双重转换。
        case hasNewer = "hasNewer"
        case latest
        case current
        case url
        case enabled
    }

    /// 手写解码（不是合成的），只为一条：**`enabled` 缺失时按 `true` 降级**
    /// （开关默认开）—— 少这一格**不许**让整份解码失败，否则一个不认得 `enabled`
    /// 的旧内核会让壳连"有没有新版"都读不出来（那是"永远不提示"的另一种长相）。
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        hasNewer = try c.decode(Bool.self, forKey: .hasNewer)
        latest = try c.decodeIfPresent(String.self, forKey: .latest)
        current = try c.decode(String.self, forKey: .current)
        url = try c.decodeIfPresent(String.self, forKey: .url)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
    }
}

/// 「有新版本」那条提示的**文案与链接**。纯值类型、纯函数，好测。
///
/// ⚠️ **链接与版本号都来自内核**（约束 3：壳不改写内核的话）—— 本文件一个字符都不拼。
///    macOS 的常驻提示行有一条不变量：**高度不许由外部数据长度决定**
///    （`Views/ResidentNotice.swift` 的文件头，那是一次真实布局事故）。
///    这里的保护有两层：
///      ① 视图用 `BoundedNoticeText`（它自带高度上限，溢出进可滚的全文）；
///      ② [`of`] 自己**只让"三段纯数字"的版本号进得来**（见下面 R21 那一段），
///         于是进到这条文案里的版本号在正常情形下很短。
public struct UpdateNotice: Equatable {
    public let text: String
    /// 「去下载」要打开的链接。**`nil` = 不画那颗按钮**（链接缺失，或不在官方发布路径下）。
    public let url: URL?

    /// 官方发布页前缀（裁定 R15）。**写死在代码里**，不接受任何参数 ——
    /// 规格 §1 的安全属性是"**写死之后，即使接口被篡改，客户被带到的仍然是那个官方仓库页面**"。
    ///
    /// ⚠️ 末尾那个 `/` 是**承重的**（与 Windows 那侧 `openurl.rs::ALLOWED_PREFIX` 同源）：
    ///    少了它，`…/releases.evil.example/x` 也会以 `…/releases` 开头。
    static let allowedReleasePrefix = "https://gitee.com/starsyi/benagen-downloader/releases/"

    /// 内核状态 → 提示条要画的东西。**不满足就回 `nil`（整条提示不出现）**。
    public static func of(_ st: UpdateStatus?) -> UpdateNotice? {
        // 🔴 判据只有 `has_newer`（R20）——不是"`url` 非空"。
        guard let st, st.hasNewer, let latest = st.latest else { return nil }

        // 🔴 **裁定 R21：`latest` 必须是"三段纯数字"（可选前导 `v` / `V`），否则整条不出现。**
        //
        // 这是**有意的逻辑重复**（内核对 `tag_name` 做过同一件事，规格 §2）—— 接受它，理由：
        //   ① 壳是 Swift、内核是 Rust，**调不到内核的 `parse_version`**；
        //   ② 这道校验守的是**壳自己的布局不变量**（`ResidentNotice.swift`：常驻行高度
        //      不许由外部数据长度决定），而不是内核的业务规则 —— 版本号是**网络来的**，
        //      内核哪天放宽了判定，壳仍然必须自己挡住，否则一段超长的 `latest` 会顺着
        //      `text` 进到那条常驻行里。**别把它当冗余删掉。**
        //
        // 语义**精确到可判定**：去掉前导 `v`/`V` ⇒ 按 `.` 切 ⇒ **恰好三段**、
        // 每段**非空且全是 ASCII 数字**；任何一条不满足 ⇒ 回 `nil`。
        // （**不是**"只截断"，也**不是**"只做长度限制"——那些都还能把畸形版本号画上屏。）
        guard isThreeSegmentVersion(latest) else { return nil }

        return UpdateNotice(
            text: "有新版本 \(latest)（当前 \(st.current)）",
            // 🔴 链接要过**前缀白名单**（R15）：不在官方发布路径下就当作**没有链接**
            //    （提示照常出现，只是不画「去下载」）。见 `allowedReleaseURL`。
            url: st.url.flatMap(allowedReleaseURL(_:)))
    }

    /// `latest` 是不是"三段纯数字"（可选前导 `v` / `V`）。见 [`of`] 里 R21 那一段。
    static func isThreeSegmentVersion(_ s: String) -> Bool {
        var body = Substring(s)
        if body.first == "v" || body.first == "V" { body = body.dropFirst() }
        // `omittingEmptySubsequences: false`：`"0.2."` ⇒ `["0","2",""]`（最后一段空 ⇒ 拒），
        // `"0..2"` ⇒ 中间一段空 ⇒ 拒。少了它，空段会被静默吃掉、三段变两段而判错。
        let parts = body.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3 else { return false }
        return parts.allSatisfy { part in
            !part.isEmpty && part.allSatisfy { ("0"..."9").contains($0) }
        }
    }

    /// 内核给的链接 → 可打开的 `URL?`。**不在官方发布路径下就回 `nil`**（裁定 R15）。
    ///
    /// 🔴 三道闸，**fail-closed**（照抄 Windows 那侧 `openurl.rs::is_allowed` 已经收紧过的口径）：
    ///   ① **前缀**（钉死 scheme + host + owner + repo + `releases/`）；
    ///   ② **余下部分非空**（光秃秃的 `…/releases/` 不是一个下载页）；
    ///   ③ **余下部分只允许 `[A-Za-z0-9._/-]`，且不含 `..` 段**。
    ///
    /// ⚠️ **为什么第 ③ 道必须有**：只判前缀挡不住 `…/releases/../../evil` ——
    ///    它以前缀开头，却能把客户带离原路径（Windows 那侧第一版只做了前缀判定，
    ///    被审查者用 `.../releases/../../evil` 绕过，是个 Critical）。
    ///    第 ③ 道是**白名单**（只放行我们真实会产生的那一类字符），不是逐个拦 `..` /
    ///    `%2e` / `\` 的黑名单 —— 黑名单永远会漏下一个。`%` 不在字符集里 ⇒
    ///    `%2e%2e` / `%2f` 这类编码**整条被拒**（连解都懒得解）。
    ///    现有资产名（`BenagenDownloader-AppleSilicon.dmg` / `-Intel.dmg`）与版本段
    ///    （`download/v{major}.{minor}.{patch}/`）全在这个字符集内 ⇒ 当下不误伤。
    ///
    /// ⚠️ **与 Windows 那侧的差别（R15 要求写清）**：Windows 那条命令是暴露给
    ///    **webview** 的（注入的 JS 能拿任意字符串调它，白名单**必需**，且是在
    ///    Rust 命令那一侧判）；macOS **没有 webview**（SwiftUI 原生），这里的白名单是
    ///    **廉价的纵深防御** —— 它挡的是"内核/接口被篡改后给出一个指向别处的链接"，
    ///    而不是"被注入的前端"。两条判据同源，价值不同。
    static func allowedReleaseURL(_ raw: String) -> URL? {
        guard raw.hasPrefix(allowedReleasePrefix) else { return nil }
        let rest = raw.dropFirst(allowedReleasePrefix.count)
        guard !rest.isEmpty else { return nil }
        let allowed = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._/-")
        guard rest.allSatisfy({ allowed.contains($0) }) else { return nil }
        // 拆段判 `..`，不是 `contains("..")` —— 后者会把合法的 `x..y` 一起拦掉。
        guard !rest.split(separator: "/").contains(where: { $0 == ".." }) else { return nil }
        return URL(string: String(raw))
    }
}
