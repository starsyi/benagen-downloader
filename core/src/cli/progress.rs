//! 进度行的格式化（**纯函数**）。
//!
//! 下载时那一行一直在变的字。本模块不读环境、不碰文件系统、不启动引擎，也不打印——
//! 它只把 [`View`] 变成一串字符，**带不带换行、用 `\r` 还是 `\n`，全由调用方决定**
//!（[`line`] 的返回串里没有换行符）。节奏（什么时候发）由 [`should_emit`] 决定。
//!
//! # 三份口径的来源（跨语言契约）
//!
//! 字节 / 速度 / 百分比这三样，**必须与 Swift 侧那份逐条一致**：
//! `macos/Sources/BenagenCoreKit/Presentation/Format.swift` 的
//! `ByteFormat` / `SpeedFormat` / `PercentFormat`。
//!
//! ⚠️ **改一处必须改另一处。** 同一个数字在图形客户端与命令行工具上显示成两样
//!（或一边显示 `0%` 一边显示 `—`），客户会当成两个不同的事实——而这**不会**报错，
//! 只会让人以为"这批数据到底下没下完"取决于看哪个窗口。
//! 分量最重的是百分比那条：四舍五入会让 `199/200` 提前显示成 `100%`，
//! 客户据此以为下完了、把源删了（见 [`percent`]）。
//!
//! ⚠️ **一条也不要用 locale 相关的格式化**：`format!("{:.1}")` 的小数点固定是 `.`，
//! 不随机器语言环境变（与 Swift 那份刻意绕开 `ByteCountFormatter` 是同一条理由）。

/// 未知/不适用时打的那个字。**破折号，不是 `0`**（见 [`speed`] / [`percent_text`]）。
pub const DASH: &str = "—";

/// 1024 进制。**顺序即换单位的次序**，与 Swift `ByteFormat.units` 一字不差。
const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// 进度行要显示的全部事实。
///
/// 字段名与类型是**契约**：Task 6 的 `poll_once` 构造它、`run()` 读它（`done_bytes` /
/// `total_bytes`）。改名或改类型等于改接口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    /// 已完成的文件数（不是"已开始的"）。
    pub done_files: usize,
    /// 清单里的文件总数。
    pub total_files: usize,
    /// 已完成字节。i64 与内核的 `size`/`completed` 同型。
    pub done_bytes: i64,
    /// 清单总字节。
    pub total_bytes: i64,
    /// 瞬时速度（字节/秒）。`<= 0` 表示"未知/停了"，显示成破折号。
    pub speed: i64,
    /// 失败的文件数。`> 0` 时**必须出现在行里**——失败了却不显示，客户会一直等。
    pub failed: usize,
}

/// 字节数 → 人话。1024 进制。**负数夹到 `0`**。
///
/// 口径（与 Swift `ByteFormat.text` 逐条对齐）：
/// * `< 1024` 出 `"N B"`，**不带小数**（`1023 B`，不是 `1023.0 B`）；
/// * 否则一路除到 `[1, 1024)` 再 `%.1f` + 一个空格 + 单位（`3.4 GB`）；
/// * 单位到 `PB` 封顶（i64 全量程也到不了 EB）；
/// * 小数点固定 `.`，不随语言环境。
///
/// ⚠️ **必须夹到非负**：内核的 `size`/`completed` 是 i64，理论上可为负（畸形清单、溢出），
/// 而 `-1 B` 这种东西不该出现在客户面前。
pub fn bytes(n: i64) -> String {
    let v = n.max(0);
    if v < 1024 {
        return format!("{v} B");
    }
    let mut x = v as f64;
    let mut i = 0usize;
    while x >= 1024.0 && i < UNITS.len() - 1 {
        x /= 1024.0;
        i += 1;
    }
    format!("{x:.1} {}", UNITS[i])
}

/// 速度 → 人话。**`<= 0` 出破折号，不出 `0 B/s`**。
///
/// "速度未知/已停止"与"速度真的是每秒零字节"在界面上是同一件事，
/// 而前者比后者常见得多（刚起步、卡在握手、任务排空）。写 `0 B/s` 会让客户以为
/// 网络断了，写 `—` 才是"这会儿没数"。
///
/// 口径与 Swift `SpeedFormat.text` 一致：非正数出 `"—"`，否则字节串 + `"/s"`。
pub fn speed(bytes_per_second: i64) -> String {
    if bytes_per_second <= 0 {
        return DASH.to_string();
    }
    format!("{}/s", bytes(bytes_per_second))
}

/// 已完成 / 总量 → `0`–`100` 的整数百分比（**数值形态**，配 [`should_emit`] 用）。
///
/// 口径（与内核 `view::progress` 的 `percent` 同一条，也是 Swift `PercentFormat.text` 那条）：
/// * `total <= 0` 出 `0`（**不除零**；清单还没加载时总量就是 0）；
/// * **整数除法向零截断**——不是四舍五入。`199/200` 是 `99.5%`，四舍五入成 `100%` 会让
///   客户以为下完了、把源删了，而实际还有一个文件没落盘。宁可让进度条"卡在 99%"一会儿。
/// * 夹到 `[0, 100]`：`done` 先夹到 `[0, total]`（任务超报不得出 200%），结果再夹一次。
pub fn percent(done: i64, total: i64) -> i64 {
    if total <= 0 {
        return 0;
    }
    // `total > 0` ⇒ `0 <= total`，`clamp` 的前提（min <= max）成立。
    let d = done.clamp(0, total);
    // ⚠️ 用 i128 算中间量：`done` 是 i64 全量程，`done * 100` 在 i64 里会溢出
    // （`i64::MAX * 100` 出界），而溢出在 release 下是**静默回绕**——会得到一个
    // 看似正常、其实毫无关系的百分比。Swift 那份用 `multipliedReportingOverflow`
    // 探测溢出后退回浮点；这里换一种等价、且不必分支的写法，结果同为向零截断。
    let pct = (i128::from(d) * 100 / i128::from(total)) as i64;
    pct.clamp(0, 100)
}

/// 百分比 → 人话（**显示形态**）。`total <= 0` 出 `"—"`。
///
/// ⚠️ **不是 `"0%"`、更不是 `"NaN"`**：清单还没加载时总量就是 0，
/// 显示 `0%` 会让客户以为"一个文件都没下"——那是错的，是"还不知道"。
/// 与 Swift `PercentFormat.text` 的 `guard total > 0 else { return "—" }` 同一条。
///
/// 数值一律走 [`percent`]，**不在这儿重算一遍**：同一个规则两份实现就会漂移，
/// 而这两处漂移的表现正好是"进度条 99% 而文字 100%"这种最难查的样子。
pub fn percent_text(done: i64, total: i64) -> String {
    if total <= 0 {
        return DASH.to_string();
    }
    format!("{}%", percent(done, total))
}

/// 进度行。**不带换行符**——调用方决定 `\r`（TTY 原地刷新）还是 `\n`（非 TTY 逐行）。
///
/// 形状（设计规格 §2 那一行，加上控制者裁定 R18 的百分比那一格）：
///
/// ```text
/// 已下 12/48 · 3.4 GB / 12.1 GB · 28% · 8.2 MB/s
/// ```
///
/// **百分比接在字节那一段后面**（R18）：`3.4 GB / 12.1 GB` 要客户自己心算，
/// `28%` 一眼就有。更要紧的是，非 TTY 的节流判据本来就是"整数百分比变了才打一行"
///（[`should_emit`]）——行里带上百分比，那条规则才自洽：每次打一行，行里告诉你现在几成。
/// 它的值走 [`percent_text`]，所以 `total_bytes <= 0` 时那一格自然是 `—`，不是 `0%`。
///
/// `failed > 0` 时在末尾补 ` · 失败 N`。
pub fn line(v: &View) -> String {
    let mut s = format!(
        "已下 {}/{} · {} / {} · {} · {}",
        v.done_files,
        v.total_files,
        bytes(v.done_bytes),
        bytes(v.total_bytes),
        percent_text(v.done_bytes, v.total_bytes),
        speed(v.speed),
    );
    if v.failed > 0 {
        s.push_str(&format!(" · 失败 {}", v.failed));
    }
    s
}

/// 这一拍要不要真的发出去。
///
/// * **TTY**：每次都发——原地刷新（`\r`）不掉历史，客户看到的是平滑的进度条。
/// * **非 TTY**（重定向、管道、CI）：**只在整数百分比变化时发**。否则日志里会是
///   每秒几十行一模一样的字，真正有用的一行被冲得找不着。
///
/// `now_percent` 用 [`percent`] 算。非 TTY 下是"与上一次发出去的那个值比"，
/// 所以调用方要把**发出去过的**值存起来，而不是上一拍算出来的值——
/// 两者在非 TTY 下恰好相同，但把"上次算的"当基准，一旦将来改成别处也调它就会漏发。
pub fn should_emit(prev_percent: i64, now_percent: i64, tty: bool) -> bool {
    tty || prev_percent != now_percent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_division_by_zero_on_zero_total() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(0, 10), 0);
        assert_eq!(percent(5, 10), 50);
        assert_eq!(percent(10, 10), 100);
    }

    #[test]
    fn the_line_carries_all_four_pieces() {
        let v = View {
            done_files: 12,
            total_files: 48,
            done_bytes: 3_650_722_201,
            total_bytes: 12_994_918_400,
            speed: 8_598_323,
            failed: 0,
        };
        let s = line(&v);
        assert!(s.contains("12/48"), "{s}");
        assert!(s.contains("3.4 GB"), "{s}");
        assert!(s.contains("12.1 GB"), "{s}");
        assert!(s.contains("8.2 MB/s"), "{s}");
        assert!(!s.contains('\n'), "不带换行符 —— 换行由调用方决定（TTY 是 \\r）");
    }

    /// 行里也要有百分比（控制者裁定 R18）。
    ///
    /// 单独一条、而不是塞进上面那条：上面那条的名字是"四样都在"，它钉的就是那四样；
    /// 往里面再塞一样，名字就成了假话。四条断言一字未动，百分比另起一条钉。
    ///
    /// 28% 是**按实现算出来的**，不是拍脑袋：`3_650_722_201 * 100 / 12_994_918_400`
    /// = 28.09…，向零截断 = 28。
    #[test]
    fn the_line_carries_the_percent_too() {
        let v = View {
            done_files: 12,
            total_files: 48,
            done_bytes: 3_650_722_201,
            total_bytes: 12_994_918_400,
            speed: 8_598_323,
            failed: 0,
        };
        assert!(line(&v).contains("28%"), "{}", line(&v));
    }

    /// 总量还没加载时，行里那一格出 `—` 而**不是 `0%`**——R17 的那条规则要真的
    /// 走在输出路径上，光在 `percent_text` 里钉住不算数（R18 让我把百分比放进行里，
    /// 这条路才第一次变得可达）。
    #[test]
    fn the_line_shows_a_dash_before_the_total_is_known() {
        let v = View {
            done_files: 0,
            total_files: 0,
            done_bytes: 0,
            total_bytes: 0,
            speed: 0,
            failed: 0,
        };
        let s = line(&v);
        // 百分比那一格与速度那一格都是 `—`，所以两个破折号挨着
        assert!(s.contains("— · —"), "{s}");
        assert!(!s.contains("0%"), "清单还没加载 ⇒ 不是 0%：{s}");
    }

    /// 失败项**必须**在行里。失败了却不显示，客户会一直等一个永远不会来的完成。
    /// `failed == 0` 时**不**显示 `失败 0`：那一行是每秒刷新的，多三个字就是噪声，
    /// 而"没有失败"正是默认的、无需汇报的状态。
    #[test]
    fn failures_must_show_up_in_the_line() {
        let v = View {
            done_files: 1,
            total_files: 4,
            done_bytes: 1,
            total_bytes: 4,
            speed: 0,
            failed: 2,
        };
        assert!(line(&v).contains("失败 2"), "{}", line(&v));

        let clean = View { failed: 0, ..v };
        assert!(!line(&clean).contains("失败"), "{}", line(&clean));
    }

    #[test]
    fn non_tty_emits_only_when_the_integer_percent_changes() {
        assert!(should_emit(10, 11, false), "跨过一个整数百分比 ⇒ 发");
        assert!(!should_emit(10, 10, false), "没变 ⇒ 不发");
        assert!(should_emit(10, 10, true), "TTY 每拍都刷新");
    }

    // -----------------------------------------------------------------------
    // 边界（控制者裁定 R17）：与 Swift `Format.swift` 逐条对齐的那几处
    // -----------------------------------------------------------------------

    /// `-1 B` 这种东西不该出现在客户面前（Swift 注释点名的边界）。
    #[test]
    fn bytes_never_shows_a_negative() {
        assert_eq!(bytes(-1), "0 B");
        assert_eq!(bytes(i64::MIN), "0 B");
    }

    /// `< 1024` 不带小数；`1024` 恰好进位。
    #[test]
    fn bytes_below_one_kib_have_no_decimal() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1), "1 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
    }

    /// "速度未知"与"速度真的是零"在界面上是同一件事 ⇒ 都出 `—`，不出 `0 B/s`。
    #[test]
    fn speed_is_a_dash_when_not_positive() {
        assert_eq!(speed(0), "—");
        assert_eq!(speed(-5), "—");
        assert_eq!(speed(1024), "1.0 KB/s");
    }

    /// 清单还没加载（总量 0）时出 `—`，不出 `0%`、更不出 `NaN`。
    #[test]
    fn percent_text_of_a_zero_total_is_a_dash() {
        assert_eq!(percent_text(0, 0), "—");
        assert_eq!(percent_text(5, 0), "—");
        assert_eq!(percent_text(5, -1), "—");
    }

    /// ⚠️ **这一条最贵的**：四舍五入会让 `199/200`（= 99.5%）显示成 `100%`，
    /// 客户以为下完了、把源删了，而实际还有一个文件没落盘。
    #[test]
    fn percent_truncates_towards_zero_and_never_rounds_up() {
        assert_eq!(percent(199, 200), 99);
        assert_eq!(percent_text(199, 200), "99%");
        assert_eq!(percent_text(1, 3), "33%");
    }

    /// 夹到 `[0, 100]`：任务超报不得出 `200%`，负数不得出负百分比。
    #[test]
    fn percent_is_clamped_to_0_100() {
        assert_eq!(percent(-5, 10), 0);
        assert_eq!(percent(100, 10), 100);
        assert_eq!(percent_text(100, 10), "100%");
        // `i64::MAX * 100` 在 i64 里会溢出静默回绕 —— 这条钉住它没在 i64 里算。
        assert_eq!(percent(i64::MAX, i64::MAX), 100);
    }
}
