//! delivery_summary —— 批次摘要、交付码入口、时间戳的呈现模型。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/DeliverySummary.swift`（逐字对位）。
//! 上游测试：`macos/Tests/BenagenCoreKitTests/DeliverySummaryTests.swift`（12 条，逐条对位）。
//!
//! ⚠️ **纪律（`lib.rs` 的章程）**：本 crate 是纯逻辑层，注释里不出现章程点名的那类词。
//!    上游那些呈现侧的说法在这里一律换成中性表述（"那一格""调用方""本次要加载的串"…）——
//!    **换掉的只是措辞，判据一个字没动**。

use serde::Serialize;
use crate::presentation::format::ByteFormat;
use crate::protocol::{DeliveryInfo, LoadState};

// ---------------------------------------------------------------------------
// 批次摘要
// ---------------------------------------------------------------------------

/// 常驻的那块摘要（规格 §7.1：批次号 / 文件数 / 总大小 / 有效期）。
///
/// 每一格都是**已经算好的字符串**：呈现层不做拼接、不做判断、不做格式化。
///
/// 上游写的是 `Equatable, Sendable`；这里对位成 `Clone, PartialEq, Eq, Debug`
/// （理由同 `browser_primary_action.rs` 的记账）。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct DeliverySummary {
    /// 交付码原文。
    pub code: String,
    /// 「7 个文件」。
    pub files_text: String,
    /// 「3.0 GB」（口径复用 [`ByteFormat`]：1024 进制、一位小数）。
    pub size_text: String,
    /// 「有效期至 2026-10-14 16:13」。
    pub validity_text: String,
    /// 「已过期」；没过期是 `None`（**不是空串** —— `None` 与 `Some("")` 的区别
    /// 就是"这一格有没有值"）。
    pub expired_badge_text: Option<String>,
}

impl DeliverySummary {
    /// 值缺失时的占位符。
    ///
    /// ⚠️ 与 `format.rs` 的 `SpeedFormat`/`PercentFormat` 用同一个字形（`—`）。
    ///    为什么要有占位符而不是空串：空串读起来就是"什么都没有"，而约束 4 要的是
    ///    「不得静默失效」—— 缺值本身也是一件要说出来的事。
    const NO_VALUE: &'static str = "—";

    /// 内核清单 → 摘要。
    pub fn of(info: &DeliveryInfo) -> DeliverySummary {
        DeliverySummary {
            code: if info.code.is_empty() {
                Self::NO_VALUE.to_string()
            } else {
                info.code.clone()
            },
            files_text: format!("{} 个文件", info.total_files),
            size_text: ByteFormat::text(info.total_bytes),
            // ⚠️ 空串直接给占位符，**不进** [`TimestampPresentation`]：那里对空串的行为是
            //    "原样返回"，两者结果相同，但这一层的语义是"这个字段根本没有值"。
            validity_text: format!(
                "有效期至 {}",
                if info.expires_at.is_empty() {
                    Self::NO_VALUE.to_string()
                } else {
                    TimestampPresentation::text(&info.expires_at)
                }
            ),
            // 过期与否**只看内核给的 `expired`**：那是内核按同一份 `expires_at` 算出来的，
            // 壳再自己比一次时间就是重实现内核逻辑（约束 1），两边还会因为时钟差打架。
            expired_badge_text: if info.expired {
                Some("已过期".to_string())
            } else {
                None
            },
        }
    }

    /// 加载状态 → 摘要。**只有 [`LoadState::Loaded`] 有摘要**，其余是 `None`。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游是两个 `of` 重载（一个收 `DeliveryInfo`、一个收
    ///    `AppModel.LoadState`）。Rust 没有重载，所以收状态的那一支改名为
    ///    `of_load_state` —— 语义、返回值一个字节都没动。
    ///
    /// ⚠️ **与上游的第二处形态差别（W-6）**：上游写的是 `guard case .loaded(...)`，
    ///    即"不是 `.loaded` 的都落 `nil`"（一条通配）。这里的 `match` **逐变体写全**：
    ///    日后 `LoadState` 多一个变体时，这一处会**编译不过**，逼着人明确决定它算不算
    ///    "有摘要"——而通配写法会让新状态**静默**地变成"没有摘要"（本项目最怕的那种形态）。
    ///
    /// ⚠️ [`LoadState::Failed`] 也是 `None`：刷新失败时内核清单还留着
    ///    （`loadDeliveryFailureKeepsTheOldManifest`），但呈现层已经退回"没有生效的批次"
    ///    那一支，此时再挂一个批次号，就是在说"这批还在"而主区正说着"没有生效的批次"。
    pub fn of_load_state(state: &LoadState) -> Option<DeliverySummary> {
        match state {
            LoadState::Idle | LoadState::Loading | LoadState::Failed(_) => None,
            LoadState::Loaded(info) => Some(Self::of(info)),
        }
    }
}

// ---------------------------------------------------------------------------
// 交付码入口：这次该用哪个码，以及它能不能发出去
// ---------------------------------------------------------------------------

/// 「加载一个交付码」这个入口的两个纯计算：
///   ① **这次要加载哪个码**（[`DeliveryCodeEntry::resolve`]）—— 两个入口**共用**同一个漏斗；
///   ② **用户手输进这条请求的串能不能发**（[`DeliveryCodeEntry::too_long`] /
///      [`DeliveryCodeEntry::is_sendable`] / 两句提示）——
///      与 `code` 同装在一条 `load_delivery` 里的还有 `base_url`，所以这条上界管的是
///      "那几个串"，不只是"交付码"这一个字段（见下面那段注释）。
///
/// 上游是 `enum DeliveryCodeEntry` + `static func`/`static let` 的命名空间用法；
/// 这里用**空 `enum` + 固有 `impl`** 对位（同 `format.rs` 的 `ByteFormat`）。
pub enum DeliveryCodeEntry {}

impl DeliveryCodeEntry {
    /// `typed` 非空就用 `typed`；空的就用**内核记着的上次那个**。
    ///
    /// ⚠️ 后半条不是"体贴"，是**必需**：启动时的自动加载失败时（规格 §8.3），
    ///    `typed` 本来就是空的，而那次「重试」要重试的正是刚才失败的那一个 ——
    ///    没有这条回落，调用方就会做出一个什么都不发生的动作（约束 4 的静默失效）。
    ///    反过来，`typed` 非空时**必须**以它为准：错误文案得是那个码换来的。
    pub fn resolve(typed: &str, remembered: &str) -> String {
        if typed.is_empty() {
            remembered.to_string()
        } else {
            typed.to_string()
        }
    }

    // -----------------------------------------------------------------------
    // 长度上界（全局约束 C-3：「壳侧**绝不**允许发出可能超限的请求」）
    //
    // ⚠️ **为什么这个上界也属于这里**：`resolve` 是"这次要发哪个码"的漏斗，
    //    而"这个码能不能发"是同一个问题的另一半。两个入口本来就走同一个 `resolve`，
    //    这里再给同一个漏斗加一条判据，两边自动一致 —— 在呈现层各写一份长度判断，
    //    迟早会分叉成"一个入口挡住了、另一个没挡"。
    //
    // ⚠️ **只管长度、不做形状校验**（有意为之，理由必须写下来，约束 C-10/11）：
    //    内核的 `delivery::extract_code` 同时接受**交付链接**与**裸码**
    //    （它自己会在串里找那个 20 位的码）。壳若在这里要求"必须 20 位"，
    //    就会把客户从交付邮件里整条粘进来的合法输入**误杀** —— 而那恰恰是
    //    「整条链接也可以直接粘进来」那句提示鼓励用户去做的事。
    //    所以这里只挡住真正会造成后果的那一类输入：**体量**。
    //
    // ⚠️ **为什么上界取 2 KiB 这么小**：内核的行长上限是 8 MiB（`core/src/main.rs:113`），
    //    而它超限时的处置**不是报错** —— 只回一条 `id == 0` 的协议告警、那条请求
    //    **永远等不到响应**，`CoreClient` 那条 FIFO 串行队列于是被**永久堵死**（约束 15），
    //    一个字都不说（约束 4 明禁的静默失效）。
    //    一个真实的交付码或交付链接只有几十到几百字节；**2 KiB 是"再离谱也不会这么大"的量级**，
    //    同时**远离 8 MiB**，留足了误判余量与信封开销。
    // -----------------------------------------------------------------------

    /// 交付码的长度上界（**字节**，与内核的行长上限同一量纲 —— 字符数是另一回事，
    /// 一个中文字符占 3 字节，用字符数判会把"看起来不长"的串放过）。
    pub const MAXIMUM_BYTES: usize = 2 << 10;

    /// 上界的**人话**（给用户看的那个数）。⚠️ 它必须与 [`Self::MAXIMUM_BYTES`] 一致 ——
    /// 改一个就改另一个（`delivery_code_bound_and_its_wording_agree` 钉着这对关系）。
    pub const MAXIMUM_TEXT: &'static str = "2 KiB";

    /// 这一串**太长、发不得**吗。
    ///
    /// ⚠️ 判据是 UTF-8 **字节数**（`str::len`），不是字符数：内核量的是行字节。
    pub fn too_long(code: &str) -> bool {
        code.len() > Self::MAXIMUM_BYTES
    }

    /// 这一串可以发出去吗（[`Self::too_long`] 的反面，写成两个名字是为了让调用点的意图
    /// 读得出来：呈现层问"能不能发"、模型问"是不是太长"）。
    pub fn is_sendable(code: &str) -> bool {
        !Self::too_long(code)
    }

    /// `base_url`（「高级：自定义下载地址」）过长时，要说的话。
    ///
    /// ⚠️ **为什么同一个类型管到 `base_url`**：上面这条上界管的不是"交付码"这个**字段**，
    ///    而是"**用户在这次加载里手输进请求的那几个串**"—— 码是其中一个，另一个就是
    ///    `base_url`（它与 `code` 装在**同一条** `load_delivery` 请求里，超了行长上限的
    ///    后果一模一样：内核不报错、只把客户端**静默堵死**）。让两个字段共用同一个上界
    ///    与同一个判据，就不会出现"挡了一个、漏了另一个"。
    ///
    /// 🔴 同 [`Self::too_long_hint`]：**壳自己写的用户可见文案**（内核收不到这条请求，
    ///    没有原文可登）。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游这两句提示是 `static let`，句中的 `\(maximumText)`
    ///    在静态初始化时就插好了。Rust 的 `concat!` 只吃字面量、拼不进另一个常量，
    ///    所以这里写成函数、用 `format!` 从唯一的 [`Self::MAXIMUM_TEXT`] 派生 ——
    ///    **保住的正是上游那句"改一处不会打架"**（两个数只有一份）。
    ///    任务 11 的简报把这两条写成 `too_long_hint()`，与这里的形态一致。
    pub fn base_url_too_long_hint() -> String {
        format!(
            "自定义下载地址过长（超过 {}），请确认粘贴的内容确实是交付页的地址。",
            Self::MAXIMUM_TEXT
        )
    }

    /// 手输的那一串过长时，要说的话。
    ///
    /// 🔴 **这是壳自己写的用户可见文案**（约束 C-10/11 要求写明理由）：内核**一个字都没说**
    ///    —— 它根本收不到这条请求（我们自己挡住不发），没有"原文"可登（约束 3 的例外）。
    ///    而"做了动作、毫无反应"是绝对不能接受的（约束 4）。所以这句话必须**说出
    ///    为什么**（过长）与**该怎么办**（只粘交付码或交付页链接），而不是只把它标成不可发。
    ///
    /// ⚠️ 与那次加载失败时抛出的错误**不是**同一句：那边说的是"请求超限的后果"
    ///    （会一直等下去），是给"绕过呈现层的调用点"兜底的；这里是 `typed` 旁边的一句
    ///    **即时提示**，在用户还没点之前就该出现。两句都从 [`Self::MAXIMUM_TEXT`]
    ///    派生（上界本身只有 [`Self::MAXIMUM_BYTES`] 一份），改一处不会打架。
    pub fn too_long_hint() -> String {
        format!(
            "交付码过长（超过 {}），请只粘贴交付码本身或交付页链接。",
            Self::MAXIMUM_TEXT
        )
    }
}

// ---------------------------------------------------------------------------
// 时间戳的呈现
// ---------------------------------------------------------------------------

/// RFC3339 字符串 → 摘要里那行小字。
///
/// `created_at` / `expires_at` 是**内核原样透传**的字符串（`core/src/main.rs` 直接把
/// `manifest.created_at` 塞进 JSON），可能为空串、也可能根本不是时间戳。
pub enum TimestampPresentation {}

impl TimestampPresentation {
    /// 把 `2026-10-14T16:13:34.805751+08:00` 显示成 `2026-10-14 16:13`。
    ///
    /// **形状**（与内核 `delivery.rs` 的 `parse_rfc3339` 接受的形状一致）：
    /// `YYYY-MM-DDTHH:MM:SS[.小数]±HH:MM` 或 `...Z`。**任何一条不满足就原样返回**——
    /// 不做猜测、不显示 "Invalid Date"（那是壳自己编的文案，约束 3）。
    ///
    /// ⚠️ **有意丢掉时区偏移**（约束 11：有意偏离要写理由）：
    ///   ① 交付服务器只发一个时区（`+08:00`，见 `core/src/delivery.rs` 的测试夹具），
    ///      把 `10:00+08:00` 原样摆进一行小字里，客户读到的是噪音；
    ///   ② **换算到本机时区则是错的**：同一份清单在飞往不同时区的机器上会显示不同的有效期，
    ///      而"有效期"是客户与业务方对账的依据；测试也会跟着变成随机器而变。
    ///    丢掉秒同理：那行放不下，而"有效期"不需要秒级精度。
    pub fn text(raw: &str) -> String {
        let bytes = raw.as_bytes();

        // 位置检查与内核逐条对齐：`len >= 20` 且 `-`/`T`/`:` 落在固定位置。
        // 用 UTF-8 字节而不是字符：多字节字符的续字节都 >= 0x80，
        // 不可能在这些位置冒充 ASCII 分隔符，所以"通过了检查"就等价于"前 19 字节是纯 ASCII"，
        // 后面的切片也就不会切在字符中间。
        if !(bytes.len() >= 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && Self::digits(bytes, 0, 4)
            && Self::digits(bytes, 5, 2)
            && Self::digits(bytes, 8, 2)
            && Self::digits(bytes, 11, 2)
            && Self::digits(bytes, 14, 2)
            && Self::digits(bytes, 17, 2))
        {
            return raw.to_string();
        }

        // 可选的小数秒（`.` 后至少一位；纳秒与毫秒在这里一视同仁）。
        let mut i = 19;
        if bytes[i] == b'.' {
            i += 1;
            let start = i;
            while i < bytes.len() && Self::is_digit(bytes[i]) {
                i += 1;
            }
            if i == start {
                return raw.to_string();
            }
        }

        if !Self::has_offset(bytes, i) {
            return raw.to_string();
        }

        // 前 10 字节（`YYYY-MM-DD`）与 `11..16`（`HH:MM`）已由上面的检查钉成 ASCII，
        // 所以逐字节转字符既不会切错字符、也不会丢信息（`u8 as char` 对 ASCII 是恒等）。
        let mut out = String::with_capacity(16);
        for &c in &bytes[0..10] {
            out.push(c as char);
        }
        out.push(' ');
        for &c in &bytes[11..16] {
            out.push(c as char);
        }
        out
    }

    /// `Z` 或 `±HH:MM`，且必须是**整串的结尾**（多一个字符都不认）。
    fn has_offset(b: &[u8], from: usize) -> bool {
        if b.len() == from + 1 && b[from] == b'Z' {
            return true;
        }
        b.len() == from + 6
            && (b[from] == b'+' || b[from] == b'-')
            && Self::digits(b, from + 1, 2)
            && b[from + 3] == b':'
            && Self::digits(b, from + 4, 2)
    }

    /// `count` 个连续数字。
    ///
    /// ⚠️ **形态偏离（W-6）**：上游直接下标取值（越界会 trap，靠调用点的守卫保证不越界）。
    ///    这里多一条 `from + count <= b.len()`：判据在可达输入上**逐字等价**，
    ///    但这条判据自己**不会 panic**（本 crate 的纪律：一条"理论上不会越界"的下标
    ///    不该把 panic 面留给读代码的人）。
    fn digits(b: &[u8], from: usize, count: usize) -> bool {
        from + count <= b.len() && (from..from + count).all(|k| Self::is_digit(b[k]))
    }

    fn is_digit(c: u8) -> bool {
        c.is_ascii_digit()
    }
}

#[cfg(test)]
mod tests {
    //! 上游 `DeliverySummaryTests.swift`（12 条，逐条对位）。
    //!
    //! ⚠️ 夹具一律是**内核会发的那种线上 JSON 文本**（键是 snake_case，时间戳形态照抄
    //!    `ProtocolTests` 里那条真实抓取：`"2026-09-17T09:37:12.805751+08:00"`），
    //!    经 `serde_json` 解成 [`DeliveryInfo`]。手搓结构体会让"壳解不解得动内核的输出"
    //!    在测试里凭空消失。
    //!
    //! ⚠️ 夹具的数值**两两不同、且非零**（7 个文件 / 3 GiB / 两个不同的时间戳）——
    //!    这是阶段 A 第五类错误（字段没被搬运的变异体活下来）的防线。

    use super::{DeliveryCodeEntry, DeliverySummary, TimestampPresentation};
    use crate::protocol::{DeliveryInfo, LoadState};

    /// 真形态的 `DeliveryInfo` 夹具。
    ///
    /// 上游那个 `deliveryInfo(...)` 有一组默认参数；Rust 没有默认参数，所以这里用
    /// **结构体更新**对位同一个用法：只写要改的那一格，
    /// `DeliveryFixture { expires_at: "".into(), ..DeliveryFixture::kernel_defaults() }`。
    /// 默认值全部非空、数值全部非零（上游那句说明在这里同样成立）。
    struct DeliveryFixture {
        code: String,
        created_at: String,
        expires_at: String,
        expired: bool,
        total_files: i64,
        total_bytes: i64,
    }

    impl DeliveryFixture {
        fn kernel_defaults() -> Self {
            Self {
                code: "C24-8_×_25WS024".to_string(),
                created_at: "2026-09-01T10:00:00+08:00".to_string(),
                expires_at: "2026-10-14T16:13:34+08:00".to_string(),
                expired: false,
                total_files: 7,
                total_bytes: 3_221_225_472,
            }
        }

        fn info(&self) -> DeliveryInfo {
            let json = format!(
                r#"{{"code":"{}","page_url":"http://dl.example/C24-8/index.html","base_url":"http://dl.example",
 "created_at":"{}","expires_at":"{}","expired":{},
 "total_files":{},"total_bytes":{},
 "tree":{{"type":"dir","name":"","children":{{}}}}}}"#,
                self.code, self.created_at, self.expires_at, self.expired, self.total_files,
                self.total_bytes
            );
            serde_json::from_str(&json).expect("这是内核会发的线上 JSON，必须解得出")
        }
    }

    /// 上游那条不带参数的调用（全默认值）。
    fn delivery_info() -> DeliveryInfo {
        DeliveryFixture::kernel_defaults().info()
    }

    /// 上游 `summaryShowsBatchNumberFilesAndTotalSize`。
    #[test]
    fn summary_shows_batch_number_files_and_total_size() {
        let s = DeliverySummary::of(&delivery_info());

        assert_eq!(s.code, "C24-8_×_25WS024", "批次号（交付码）原文照登");
        assert_eq!(s.files_text, "7 个文件");
        assert_eq!(s.size_text, "3.0 GB", "口径复用 ByteFormat（1024 进制、一位小数）");
        assert_eq!(s.validity_text, "有效期至 2026-10-14 16:13");
        assert_eq!(s.expired_badge_text, None, "没过期的批次不该挂过期标记");
    }

    /// 上游 `summaryMarksExpiredBatches`。
    #[test]
    fn summary_marks_expired_batches() {
        let expired = DeliverySummary::of(&DeliveryFixture {
            expired: true,
            ..DeliveryFixture::kernel_defaults()
        }
        .info());

        assert_eq!(
            expired.expired_badge_text.as_deref(),
            Some("已过期"),
            "过期必须让客户看得见（规格 §7.1）"
        );

        // ⚠️ 反向的一半也断言：否则"恒返回已过期"的变异体也能活下来。
        assert_eq!(
            DeliverySummary::of(&delivery_info()).expired_badge_text,
            None
        );
    }

    /// 上游 `summaryHandlesUnparseableTimestamps`。
    #[test]
    fn summary_handles_unparseable_timestamps() {
        // `created_at` / `expires_at` 是内核**原样透传**的字符串，可能为空串、也可能不是
        // RFC3339。格式化失败必须**回落到原文** —— 绝不显示 "Invalid Date"（简报明文）。
        assert_eq!(
            DeliverySummary::of(
                &DeliveryFixture {
                    expires_at: String::new(),
                    ..DeliveryFixture::kernel_defaults()
                }
                .info()
            )
            .validity_text,
            "有效期至 —",
            "空串给占位符，不给 'nil'、不给空白"
        );
        assert_eq!(
            DeliverySummary::of(
                &DeliveryFixture {
                    expires_at: "待定".to_string(),
                    ..DeliveryFixture::kernel_defaults()
                }
                .info()
            )
            .validity_text,
            "有效期至 待定",
            "不是时间戳就原样显示"
        );
        assert_eq!(
            DeliverySummary::of(
                &DeliveryFixture {
                    expires_at: "2026-10-14T16:13:34".to_string(),
                    ..DeliveryFixture::kernel_defaults()
                }
                .info()
            )
            .validity_text,
            "有效期至 2026-10-14T16:13:34",
            "缺时区偏移不是 RFC3339（内核的 parse_rfc3339 同样拒绝它）→ 回落到原文"
        );
    }

    /// 上游 `summaryDoesNotInventTextForEmptyFields`。
    #[test]
    fn summary_does_not_invent_text_for_empty_fields() {
        let s = DeliverySummary::of(
            &DeliveryFixture {
                code: String::new(),
                created_at: String::new(),
                expires_at: String::new(),
                total_files: 0,
                total_bytes: 0,
                ..DeliveryFixture::kernel_defaults()
            }
            .info(),
        );

        // ⚠️ 上游那句断言的措辞里带一个呈现侧动词，这里换成中性的"输出"——
        //    **判据与断言值一字未改**（换的只是这句人话）。
        assert_eq!(s.code, "—", "空批次号不得输出成空串（更不得出现字面量 nil）");
        assert_eq!(s.files_text, "0 个文件", "一个文件都没有也要说 0，不是空白");
        assert_eq!(s.size_text, "0 B");
        assert_eq!(s.validity_text, "有效期至 —");
        assert_eq!(s.expired_badge_text, None);

        // 逐格扫一遍：任何一格都不许出现 nil / null / Invalid（约束 3：壳不编文案）。
        for field in [&s.code, &s.files_text, &s.size_text, &s.validity_text] {
            let lower = field.to_lowercase();
            assert!(!lower.contains("nil"), "「{field}」里出现了 nil");
            assert!(!lower.contains("null"), "「{field}」里出现了 null");
            assert!(!lower.contains("invalid"), "「{field}」里出现了 Invalid");
        }
    }

    /// 上游 `summaryIsAbsentUntilAManifestIsLoaded`。
    #[test]
    fn summary_is_absent_until_a_manifest_is_loaded() {
        let info = delivery_info();

        // 没加载（含加载中、失败）时摘要为 None —— 那一格显示占位，**不显示上一批的批次号**。
        assert!(DeliverySummary::of_load_state(&LoadState::Idle).is_none());
        assert!(DeliverySummary::of_load_state(&LoadState::Loading).is_none());
        assert!(DeliverySummary::of_load_state(&LoadState::Failed(
            "拉取交付清单失败：HTTP 404".to_string()
        ))
        .is_none());

        assert_eq!(
            DeliverySummary::of_load_state(&LoadState::Loaded(info.clone()))
                .map(|s| s.code)
                .as_deref(),
            Some("C24-8_×_25WS024")
        );
        assert_eq!(
            DeliverySummary::of_load_state(&LoadState::Loaded(info))
                .map(|s| s.size_text)
                .as_deref(),
            Some("3.0 GB")
        );
    }

    /// 上游 `typedCodeWinsOverTheRememberedOne`。
    #[test]
    fn typed_code_wins_over_the_remembered_one() {
        // `typed` 非空 —— 那就是要加载的（哪怕它是错的，
        // 显示出来的错误也必须是**那个码**换来的原文）。
        assert_eq!(
            DeliveryCodeEntry::resolve("AbCdEfGhIjKlMnOpQrSt", "C24-8"),
            "AbCdEfGhIjKlMnOpQrSt"
        );
    }

    /// 上游 `anEmptyFieldFallsBackToTheRememberedCode`。
    #[test]
    fn an_empty_field_falls_back_to_the_remembered_code() {
        // ⚠️ `typed` 为空 + 「重试」不是"什么都不做"：启动时的自动加载失败时 `typed`
        //    本来就是空的，而那次重试要重试的正是**刚才失败的那一个**（内核记着的那个码）。
        assert_eq!(DeliveryCodeEntry::resolve("", "C24-8"), "C24-8");
        // ⚠️ 与 `summary_does_not_invent_text_for_empty_fields` 那一条同一种处置：
        //    上游这句失败消息里带一个**界面动词**（源的那个词点出了"点不动"这件事），
        //    这里换成中性的"不要发一条空码的请求" —— **判据与断言值一字未改**
        //    （换的只是这句人话；本 crate 不含界面概念，章程见 `lib.rs` 头部）。
        assert_eq!(
            DeliveryCodeEntry::resolve("", ""),
            "",
            "两边都空就还是空（调用方据此不要发一条空码的请求）"
        );
    }

    /// 上游 `deliveryCodeBoundAndItsWordingAgree`。
    #[test]
    fn delivery_code_bound_and_its_wording_agree() {
        // ⚠️ 上界与那句人话是**两个**常量（一处给代码、一处给用户），改一个忘了另一个
        //    就会出现"提示里说 2 KiB、判据挡在别处"的分叉。这条把它们钉在一起。
        assert_eq!(
            DeliveryCodeEntry::MAXIMUM_BYTES,
            2 << 10,
            "上界就是 2 KiB —— 改它请连同这一条"
        );
        assert_eq!(DeliveryCodeEntry::MAXIMUM_TEXT, "2 KiB");
        // ⚠️ **下面这两条 `contains` 不是"怎么改都不会红"的恒真判据（控制者裁决 LL）**：
        //    `hint` 里那个数是从**唯一一份** [`DeliveryCodeEntry::MAXIMUM_TEXT`] 派生出来的，
        //    谁把它换成第二份写死的字面量（哪怕值一样），这一条立刻红。它钉的正是
        //    本阶段最想要的那条性质：**同一件事不出现第二份来源**。
        //    形态与上游逐字同形（源的 `static let` 也是插值、`#expect` 也是自指），
        //    按裁决 T"不得意译"，这里**不**改写成写死的字面量。
        let hint = DeliveryCodeEntry::too_long_hint();
        assert!(
            hint.contains(DeliveryCodeEntry::MAXIMUM_TEXT),
            "提示里说的数必须与判据用的数一致：{hint}"
        );
        // 提示必须**说出为什么**（否则调用方就只剩一个不会响的动作，约束 4）与**该怎么做**。
        assert!(hint.contains("过长"), "要说清楚是因为太长");
        assert!(hint.contains("链接"), "要给出出路（只粘贴交付码或交付页链接）");

        // `base_url`（「高级：自定义下载地址」那一项）与 `code` 是**同一条请求**上的两格用户可填的字段，
        // 所以共用同一个上界与同一套要求 —— 下面两条钉住"那一半也没有被落下"。
        // 同上（裁决 LL）：这一条钉的是"两个字段说的数是同一个"，
        // 而 `base_url` 那一句同样从 [`DeliveryCodeEntry::MAXIMUM_TEXT`] 派生。
        let url_hint = DeliveryCodeEntry::base_url_too_long_hint();
        assert!(
            url_hint.contains(DeliveryCodeEntry::MAXIMUM_TEXT),
            "两个字段说的数必须是同一个：{url_hint}"
        );
        assert!(
            url_hint.contains("自定义下载地址"),
            "要说清楚过长的是哪一个字段（否则用户不知道该改哪一格）"
        );
    }

    /// 上游 `theCodeBoundIsNotAShapeCheck`。
    #[test]
    fn the_code_bound_is_not_a_shape_check() {
        // ⚠️ **本组的核心纪律**：这里只设**长度**上界，**不校验形状**。
        //    内核的 `delivery::extract_code` 同时接受交付链接与裸码，壳若要求"必须 20 位"
        //    就会把客户从交付邮件里整条粘进来的合法输入**误杀** —— 那正是那句提示
        //    （「整条链接也可以直接粘进来」）鼓励用户去做的事。
        //
        // 下面每一条都是**会被形状校验误杀**的合法输入（长度都在上界之内 ⇒ 都必须放行）：
        let legit = [
            "https://download.benagen.com/C24-8_×_25WS024/C24-8_×_25WS024.html", // 整条交付页链接
            "http://download.benagen.com/AbCdEfGhIjKlMnOpQrSt/index.html?from=mail&batch=3",
            "C24-8",           // 比 20 位短（不是码的形状，但内核自己会找）
            "C24-8_×_25WS024", // 含非 ASCII 的批次号
            "  两边有空白  ",
            "交付码在邮件里：AbCdEfGhIjKlMnOpQrSt", // 混着中文说明
        ];
        for code in legit {
            assert!(
                DeliveryCodeEntry::is_sendable(code),
                "合法粘贴被误杀：{code}"
            );
            assert!(
                !DeliveryCodeEntry::too_long(code),
                "同上：{code}"
            );
        }
    }

    /// 上游 `onlyTheLengthBoundRejects`。
    #[test]
    fn only_the_length_bound_rejects() {
        // 边界是**闭区间**：恰好 2 KiB 放行、多 1 字节拒绝。
        let at_bound = "A".repeat(DeliveryCodeEntry::MAXIMUM_BYTES);
        let over_bound = format!("{at_bound}A");

        assert!(DeliveryCodeEntry::is_sendable(&at_bound), "恰好到上界仍然可以发");
        assert!(!DeliveryCodeEntry::too_long(&at_bound));
        assert!(
            DeliveryCodeEntry::too_long(&over_bound),
            "超 1 字节就必须拒发（边界是闭区间）"
        );

        // ⚠️ 判据是 **UTF-8 字节数**，不是 `chars().count()`：一个中文字符占 3 字节。
        //    用字符数判的话，下面这个串（其字符数只有 1024，看起来"没超"）会被放过，
        //    而它其实是 3072 字节 —— 与内核量的不是同一个东西。
        let multibyte = "码".repeat(1024);
        assert!(
            multibyte.chars().count() < DeliveryCodeEntry::MAXIMUM_BYTES,
            "前提：这个串的**字符数**在上界之内"
        );
        assert!(
            multibyte.len() > DeliveryCodeEntry::MAXIMUM_BYTES,
            "前提：它的**字节数**超了"
        );
        assert!(
            DeliveryCodeEntry::too_long(&multibyte),
            "必须按字节判（与内核的行长同一量纲）"
        );

        // 9 MiB（复审给的那个量级）：远超上界，也远超误判余量。
        assert!(DeliveryCodeEntry::too_long(&"A".repeat(9 << 20)));
    }

    /// 上游 `timestampsKeepTheSourceWallClockAndDropSubMinuteDetail`。
    #[test]
    fn timestamps_keep_the_source_wall_clock_and_drop_sub_minute_detail() {
        // ⚠️ **不换算到本机时区**：显示的是原文里那个墙上时间。
        //    换算的后果是同一份清单在两台时区不同的机器上显示不同的有效期，而"有效期"
        //    是客户与业务方对账的依据；测试也会跟着变成随机器而变。
        assert_eq!(TimestampPresentation::text("2026-10-14T16:13:34+08:00"), "2026-10-14 16:13");
        assert_eq!(
            TimestampPresentation::text("2026-09-17T09:37:12.805751+08:00"),
            "2026-09-17 09:37",
            "内核真实抓取带 6 位小数秒"
        );
        assert_eq!(TimestampPresentation::text("2026-10-14T16:13:34Z"), "2026-10-14 16:13");
        assert_eq!(
            TimestampPresentation::text("2026-10-14T23:13:34-05:00"),
            "2026-10-14 23:13"
        );
    }

    /// 上游 `timestampsFallBackToTheRawText`。
    #[test]
    fn timestamps_fall_back_to_the_raw_text() {
        assert_eq!(TimestampPresentation::text(""), "");
        assert_eq!(TimestampPresentation::text("待定"), "待定");
        assert_eq!(
            TimestampPresentation::text("2026-10-14"),
            "2026-10-14",
            "只有日期没有时刻：不猜时刻"
        );
        assert_eq!(
            TimestampPresentation::text("2026-10-14 16:13:34"),
            "2026-10-14 16:13:34",
            "空格分隔不是 RFC3339（且长度不够）"
        );
        assert_eq!(
            TimestampPresentation::text("2026-10-14T16:13:34+08:00 尾巴"),
            "2026-10-14T16:13:34+08:00 尾巴",
            "时区偏移后面还有别的东西 → 整串回落到原文"
        );
        assert_eq!(
            TimestampPresentation::text("2026-10-14T16:13:34+0800"),
            "2026-10-14T16:13:34+0800",
            "偏移必须带冒号（RFC3339）；不带就回落原文，不猜"
        );
    }
}
