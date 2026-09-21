import Foundation

// ---------------------------------------------------------------------------
// 批次摘要的**呈现模型**：全部是纯函数，无状态、无 SwiftUI 依赖（只 `import Foundation`）。
//
// 为什么这两段映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   判据是"如果一段代码你能写出一个断言，它就不属于视图文件" —— 下面这两条都能写出断言
//   （`expires_at` 那一条还背着简报明文的要求：格式化失败必须回落到原文）。
//   视图侧只剩绑定。
// ---------------------------------------------------------------------------

/// 侧边栏底部常驻的那块摘要（规格 §7.1：批次号 / 文件数 / 总大小 / 有效期）。
///
/// 每一格都是**已经算好的字符串**：视图里不做拼接、不做判断、不做格式化。
public struct DeliverySummary: Equatable, Sendable {
    /// 交付码原文。
    public let code: String
    /// 「7 个文件」。
    public let filesText: String
    /// 「3.0 GB」（口径复用 [`ByteFormat`]：1024 进制、一位小数）。
    public let sizeText: String
    /// 「有效期至 2026-10-14 16:13」。
    public let validityText: String
    /// 「已过期」；没过期是 `nil`（**不是空串** —— 视图靠 nil 决定这一格渲不渲染）。
    public let expiredBadgeText: String?

    /// 值缺失时的占位符。
    ///
    /// ⚠️ 与 `Format.swift` 的 `SpeedFormat`/`PercentFormat` 用同一个字形（`—`）。
    ///    为什么要有占位符而不是渲染空串：空串在界面上就是"什么都没有"，而约束 4 要的是
    ///    「不得静默失效」—— 缺值本身也是一件要说出来的事。
    private static let noValue = "—"

    /// 内核清单 → 摘要。
    public static func of(_ info: DeliveryInfo) -> DeliverySummary {
        DeliverySummary(
            code: info.code.isEmpty ? noValue : info.code,
            filesText: "\(info.totalFiles) 个文件",
            sizeText: ByteFormat.text(info.totalBytes),
            // ⚠️ 空串直接给占位符，**不进** `TimestampPresentation`：那里对空串的行为是
            //    "原样返回"，两者结果相同，但这一层的语义是"这个字段根本没有值"。
            validityText: "有效期至 " + (info.expiresAt.isEmpty
                                         ? noValue
                                         : TimestampPresentation.text(info.expiresAt)),
            // 过期与否**只看内核给的 `expired`**：那是内核按同一份 `expires_at` 算出来的，
            // 壳再自己比一次时间就是重实现内核逻辑（约束 1），两边还会因为时钟差打架。
            expiredBadgeText: info.expired ? "已过期" : nil)
    }

    /// `loadState` → 摘要。**只有 `.loaded` 有摘要**，其余（未加载 / 加载中 / 失败）是 `nil`。
    ///
    /// ⚠️ `.failed` 也是 `nil`：刷新失败时内核清单还留着（`loadDeliveryFailureKeepsTheOldManifest`），
    ///    但界面已经退回空态页（`RootView` 的分支），此时侧边栏再挂一个批次号，
    ///    就是在说"这批还在"而主区正说着"没有生效的批次"。
    public static func of(_ state: AppModel.LoadState) -> DeliverySummary? {
        guard case .loaded(let info) = state else { return nil }
        return of(info)
    }
}

// ---------------------------------------------------------------------------
// 空态页 / 换码面板：这次点「加载」/「重试」该用哪个码，以及它能不能发出去
// ---------------------------------------------------------------------------

/// 「加载一个交付码」这个入口的两个纯计算：
///   ① **这次要加载哪个码**（`resolve`）—— 空态页与换码面板**共用**同一个漏斗；
///   ② **用户手输进这条请求的串能不能发**（`tooLong` / `isSendable` / 两句提示）——
///      与 `code` 同装在一条 `load_delivery` 里的还有 `base_url`，所以这条上界管的是
///      "那几个串"，不只是"交付码"这一个字段（见下面那段注释）。
///
/// 放这里而不是视图里（全局约束 8）：这两类都能写出断言（下面那些测试就是），
/// 所以它们属于 `Presentation/`；放进视图就等于往两个入口各抄一份、迟早分叉。
public enum DeliveryCodeEntry {
    /// 输入框里有东西就用输入框那个；空的就用**内核记着的上次那个**。
    ///
    /// ⚠️ 后半条不是"体贴"，是**必需**：启动时的自动加载失败时（规格 §8.3），
    ///    输入框本来就是空的，而那颗「重试」要重试的正是刚才失败的那一个 ——
    ///    没有这条回落，用户面对的就是一颗按下去什么都不发生的按钮（约束 4 的静默失效）。
    ///    反过来，输入框非空时**必须**以用户敲的为准：错误文案得是他敲的那个码换来的。
    public static func resolve(typed: String, remembered: String) -> String {
        typed.isEmpty ? remembered : typed
    }

    // -----------------------------------------------------------------------
    // 长度上界（全局约束 C-3：「壳侧**绝不**允许发出可能超限的请求」）
    //
    // ⚠️ **为什么这个上界也属于这里**：`resolve` 是"这次要发哪个码"的漏斗，
    //    而"这个码能不能发"是同一个问题的另一半。两个入口（空态页与换码面板）
    //    本来就走同一个 `resolve`，这里再给同一个漏斗加一条判据，两边自动一致 ——
    //    在视图里各写一份长度判断，迟早会分叉成"一个入口挡住了、另一个没挡"。
    //
    // ⚠️ **只管长度、不做形状校验**（有意为之，理由必须写下来，约束 C-10/11）：
    //    内核的 `delivery::extract_code` 同时接受**交付链接**与**裸码**
    //    （它自己会在串里找那个 20 位的码）。壳若在这里要求"必须 20 位"，
    //    就会把客户从交付邮件里整条粘进来的合法输入**误杀** —— 而那恰恰是空态页
    //    那行提示（「整条链接也可以直接粘进来」）鼓励用户去做的事。
    //    所以这里只挡住真正会造成后果的那一类输入：**体量**。
    //
    // ⚠️ **为什么上界取 2 KiB 这么小**：内核的行长上限是 8 MiB（`core/src/main.rs:113`），
    //    而它超限时的处置**不是报错** —— 只回一条 `id == 0` 的协议告警、那条请求
    //    **永远等不到响应**，`CoreClient` 那条 FIFO 串行队列于是被**永久堵死**（约束 15），
    //    界面上一个字都不说（约束 4 明禁的静默失效）。
    //    一个真实的交付码或交付链接只有几十到几百字节；**2 KiB 是"再离谱也不会这么大"的量级**，
    //    同时**远离 8 MiB**，留足了误判余量与信封开销。
    // -----------------------------------------------------------------------

    /// 交付码的长度上界（**字节**，与内核的行长上限同一量纲 —— `String.count` 是字符数，
    /// 一个中文字符占 3 字节，用它判会把"看起来不长"的串放过）。
    public static let maximumBytes = 2 << 10

    /// 上界的**人话**（给用户看的那个数）。⚠️ 它必须与 [`maximumBytes`] 一致 ——
    /// 改一个就改另一个（`deliveryCodeBoundAndItsWordingAgree` 钉着这对关系）。
    public static let maximumText = "2 KiB"

    /// 这一串**太长、发不得**吗。
    ///
    /// ⚠️ 判据是 UTF-8 字节数，不是 `count`：内核量的是行字节。
    public static func tooLong(_ code: String) -> Bool {
        code.utf8.count > maximumBytes
    }

    /// 这一串可以发出去吗（`tooLong` 的反面，写成两个名字是为了让调用点的意图读得出来：
    /// 视图问"能不能发"、模型问"是不是太长"）。
    public static func isSendable(_ code: String) -> Bool {
        !tooLong(code)
    }

    /// `base_url`（空态页「高级：自定义下载地址」）过长时，界面上要说的话。
    ///
    /// ⚠️ **为什么同一个类型管到 `base_url`**：上面这条上界管的不是"交付码"这个**字段**，
    ///    而是"**用户在这次加载里手输进请求的那几个串**"—— 码是其中一个，另一个就是
    ///    `base_url`（它与 `code` 装在**同一条** `load_delivery` 请求里，超了行长上限的
    ///    后果一模一样：内核不报错、只把客户端**静默堵死**）。让两个字段共用同一个上界
    ///    与同一个判据，就不会出现"挡了一个、漏了另一个"。
    ///    `switchDelivery` 那条路传的是 `nil`（面板没有这个字段），所以在那边不起作用。
    ///
    /// 🔴 同 [`tooLongHint`]：**壳自己写的用户可见文案**（内核收不到这条请求，没有原文可登）。
    public static let baseURLTooLongHint =
        "自定义下载地址过长（超过 \(maximumText)），请确认粘贴的内容确实是交付页的地址。"

    /// 输入框里那一串过长时，界面上要说的话。
    ///
    /// 🔴 **这是壳自己写的用户可见文案**（约束 C-10/11 要求写明理由）：内核**一个字都没说**
    ///    —— 它根本收不到这条请求（我们自己挡住不发），没有"原文"可登（约束 3 的例外）。
    ///    而"按了按钮、界面毫无反应"是绝对不能接受的（约束 4）。所以这句话必须**说出
    ///    为什么**（过长）与**该怎么办**（只粘交付码或交付页链接），而不是只把按钮变灰。
    ///
    /// ⚠️ 与 [`AppModel.performLoadDelivery`] 里那条抛出的错误**不是**同一句：那边说的是
    ///    "请求超限的后果"（会一直等下去），是给"绕过视图的调用点"兜底的；
    ///    这里是输入框旁边的一句**即时提示**，在用户还没点之前就该出现。
    ///    两句都从 [`maximumText`] 派生（上界本身只有 [`maximumBytes`] 一份），改一处不会打架。
    public static let tooLongHint = "交付码过长（超过 \(maximumText)），请只粘贴交付码本身或交付页链接。"
}

// ---------------------------------------------------------------------------
// 时间戳的呈现
// ---------------------------------------------------------------------------

/// RFC3339 字符串 → 侧边栏里那行小字。
///
/// `created_at` / `expires_at` 是**内核原样透传**的字符串（`core/src/main.rs` 直接把
/// `manifest.created_at` 塞进 JSON），可能为空串、也可能根本不是时间戳。
public enum TimestampPresentation {

    /// 把 `2026-10-14T16:13:34.805751+08:00` 显示成 `2026-10-14 16:13`。
    ///
    /// **形状**（与内核 `delivery.rs` 的 `parse_rfc3339` 接受的形状一致）：
    /// `YYYY-MM-DDTHH:MM:SS[.小数]±HH:MM` 或 `...Z`。**任何一条不满足就原样返回**——
    /// 不做猜测、不显示 "Invalid Date"（那是壳自己编的文案，约束 3）。
    ///
    /// ⚠️ **有意丢掉时区偏移**（约束 11：有意偏离要写理由）：
    ///   ① 交付服务器只发一个时区（`+08:00`，见 `core/src/delivery.rs` 的测试夹具），
    ///      把 `10:00+08:00` 原样摆进一行 `caption2` 里，客户读到的是噪音；
    ///   ② **换算到本机时区则是错的**：同一份清单在飞往不同时区的机器上会显示不同的有效期，
    ///      而"有效期"是客户与业务方对账的依据；测试也会跟着变成随机器而变。
    ///   丢掉秒同理：侧边栏那行放不下，而"有效期"不需要秒级精度。
    public static func text(_ raw: String) -> String {
        let bytes = Array(raw.utf8)

        // 位置检查与内核逐条对齐：`len >= 20` 且 `-`/`T`/`:` 落在固定位置。
        // 用 UTF-8 字节而不是 Character：多字节字符的续字节都 >= 0x80，
        // 不可能在这些位置冒充 ASCII 分隔符，所以"通过了检查"就等价于"前 19 字节是纯 ASCII"，
        // 后面的切片也就不会切在字符中间。
        guard bytes.count >= 20,
              bytes[4] == UInt8(ascii: "-"), bytes[7] == UInt8(ascii: "-"),
              bytes[10] == UInt8(ascii: "T"),
              bytes[13] == UInt8(ascii: ":"), bytes[16] == UInt8(ascii: ":"),
              digits(bytes, 0, 4), digits(bytes, 5, 2), digits(bytes, 8, 2),
              digits(bytes, 11, 2), digits(bytes, 14, 2), digits(bytes, 17, 2)
        else { return raw }

        // 可选的小数秒（`.` 后至少一位；纳秒与毫秒在这里一视同仁）。
        var i = 19
        if bytes[i] == UInt8(ascii: ".") {
            i += 1
            let start = i
            while i < bytes.count, isDigit(bytes[i]) { i += 1 }
            guard i > start else { return raw }
        }

        guard hasOffset(bytes, from: i) else { return raw }

        return String(decoding: bytes[0..<10], as: UTF8.self)
            + " "
            + String(decoding: bytes[11..<16], as: UTF8.self)
    }

    /// `Z` 或 `±HH:MM`，且必须是**整串的结尾**（多一个字符都不认）。
    private static func hasOffset(_ b: [UInt8], from i: Int) -> Bool {
        if b.count == i + 1, b[i] == UInt8(ascii: "Z") { return true }
        return b.count == i + 6
            && (b[i] == UInt8(ascii: "+") || b[i] == UInt8(ascii: "-"))
            && digits(b, i + 1, 2)
            && b[i + 3] == UInt8(ascii: ":")
            && digits(b, i + 4, 2)
    }

    private static func digits(_ b: [UInt8], _ from: Int, _ count: Int) -> Bool {
        for k in from..<(from + count) where !isDigit(b[k]) { return false }
        return true
    }

    private static func isDigit(_ c: UInt8) -> Bool {
        c >= UInt8(ascii: "0") && c <= UInt8(ascii: "9")
    }
}
