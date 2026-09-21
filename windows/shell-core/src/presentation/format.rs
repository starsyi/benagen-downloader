//! format —— 字节数 / 速度 / 进度 → 人话。**全部是纯函数**，无状态、无 locale 依赖。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/Format.swift`（逐字对位）。
//!
//! ⚠️ 一条也不要用现成的"人类可读字节数"工具（上游点名的是 Apple 的 `ByteCountFormatter`）：
//!    它的输出**随机器的语言环境变**（同一份代码在中文/英文/任意 region 的机器上显示不同，
//!    测试也会跟着红），而且默认口径是**十进制 1000**（1024 与 1000 都会被显示成 "1 KB"）。
//!    这里的口径是 **1024 进制、一位小数**，由本文件逐字钉住。
//!    Rust 侧的同类诱惑是引 `bytesize`/`human_bytes` 这类 crate：同样不允许——
//!    除了口径会漂，`shell-core` 的直接依赖还锁在 `test.sh` 的白名单里（只有 serde 两个），
//!    加一个就是在改那条守卫，而不是"顺手用一下"。

// ---------------------------------------------------------------------------
// 字节数
// ---------------------------------------------------------------------------

/// 字节数 → 人话。`0` 特例、"不足 1 KB 不带小数"、负数夹到 `0`。
///
/// 上游 `Format.swift:13-33`。Swift 那边是 `enum` + `static func` 的命名空间用法；
/// 这里用**空 `enum` + 固有 `impl`** 对位（空 `enum` 同样不可构造，语义一致）。
pub enum ByteFormat {}

impl ByteFormat {
    /// 1024 进制。顺序即换单位的次序（上游的 `units`）。
    const UNITS: [&'static str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

    /// ⚠️ **必须夹到非负**：内核的 `size`/`completed` 是 i64，理论上可为负
    /// （畸形清单、溢出），而 `-1 B` 这种东西不该出现在客户面前。
    pub fn text(bytes: i64) -> String {
        let v = bytes.max(0);
        if v < 1024 {
            return format!("{v} B");
        }
        let mut x = v as f64;
        let mut i = 0usize;
        while x >= 1024.0 && i < Self::UNITS.len() - 1 {
            x /= 1024.0;
            i += 1;
        }
        // 小数点固定是 `.`：Rust 的 `{:.1}` 走的是同一个 C 口径的十进制小数点，
        // 不随机器语言环境变（上游对应的是不带 locale 的 `String(format:)`）。
        format!("{:.1} {}", x, Self::UNITS[i])
    }
}

// ---------------------------------------------------------------------------
// 速度
// ---------------------------------------------------------------------------

/// 速度 → 人话。**`0` 显示破折号，不显示 `0 B/s`**：
/// "速度未知/已停止"与"速度真的是每字节零"在客户眼里是同一件事，
/// 而后者更常见的是前者（上游 `Format.swift:37-42`）。
pub enum SpeedFormat {}

impl SpeedFormat {
    pub fn text(bytes_per_second: i64) -> String {
        if bytes_per_second > 0 {
            return format!("{}/s", ByteFormat::text(bytes_per_second));
        }
        "—".to_string()
    }
}

// ---------------------------------------------------------------------------
// 进度
// ---------------------------------------------------------------------------

/// 进度 → 人话（上游 `Format.swift:45-59`）。
pub enum PercentFormat {}

impl PercentFormat {
    /// ⚠️ **`total == 0` 时返回 `"—"`，不是 `"0%"`，更不是 `NaN`**：
    /// 清单还没加载时总量就是 0，显示 `0%` 会让客户以为"一个文件都没下"。
    ///
    /// 口径与内核的 `view::progress` **逐条对齐**（契约 §1.6）：
    /// **整数除法向零截断**（不是四舍五入——四舍五入会让 99.5% 提前显示成 100%，
    /// 客户以为下完了），且夹到 `[0, 100]`（任务超报不得出 200%）。
    pub fn text(done: i64, total: i64) -> String {
        if total <= 0 {
            return "—".to_string();
        }
        // `total > 0` 由上一行保证 ⇒ `clamp` 的 `min <= max` 成立（它不会 panic）。
        let d = done.clamp(0, total);
        // 上游用 `multipliedReportingOverflow(by: 100)` 判溢出，溢出时退回浮点算。
        // 这里 `checked_mul` 对位同一个判据；溢出只可能发生在 `total` 极大时，
        // 而那条路上 `d <= total` ⇒ 结果仍在 `[0, 100]`，`as i64` 的饱和转换
        // （Rust 的 `as` 不会 panic）与上游的 `Int64(...)` 得到同一个值。
        let pct: i64 = match d.checked_mul(100) {
            Some(scaled) => scaled / total,
            None => ((d as f64 / total as f64) * 100.0) as i64,
        };
        format!("{}%", pct.clamp(0, 100))
    }
}

#[cfg(test)]
mod tests {
    //! 全部是**纯函数**的边界值。这一层最容易"看起来对就行"，所以每个边界逐个钉住。
    //!
    //! 上游：`macos/Tests/BenagenCoreKitTests/FormatTests.swift`（6 条，逐条对位）。
    //!
    //! ⚠️ 一条也不许改成"人类可读字节数"的现成实现：它的输出随语言环境变（测试会随机器红），
    //!    且默认是十进制 1000（1024 会被显示成 "1 KB" 而 1000 也是 "1 KB"）。

    use super::{ByteFormat, PercentFormat, SpeedFormat};

    /// 上游 `formatsBytesWithBinaryUnits`。
    #[test]
    fn formats_bytes_with_binary_units() {
        assert_eq!(ByteFormat::text(0), "0 B");
        assert_eq!(ByteFormat::text(1), "1 B");
        assert_eq!(ByteFormat::text(1023), "1023 B");
        assert_eq!(ByteFormat::text(1024), "1.0 KB");
        assert_eq!(ByteFormat::text(1536), "1.5 KB");
        assert_eq!(ByteFormat::text(1_048_576), "1.0 MB");
        assert_eq!(ByteFormat::text(65_536), "64.0 KB");
        assert_eq!(ByteFormat::text(5_368_709_120), "5.0 GB");
    }

    /// 上游 `formatsBytesAcrossEveryUnitBoundary`。
    ///
    /// 1024 进制：每一级的边界都必须换单位，且**不能用十进制 1000 的口径**。
    #[test]
    fn formats_bytes_across_every_unit_boundary() {
        // 十进制口径会在这里变成 "1.0 KB"
        assert_eq!(ByteFormat::text(1000), "1000 B");
        // 换单位前最后一个值
        assert_eq!(ByteFormat::text(1_048_575), "1024.0 KB");
        assert_eq!(ByteFormat::text(1_073_741_824), "1.0 GB");
        assert_eq!(ByteFormat::text(1_099_511_627_776), "1.0 TB");
        assert_eq!(ByteFormat::text(1_125_899_906_842_624), "1.0 PB");
    }

    /// 上游 `formatsSpeed`。
    #[test]
    fn formats_speed() {
        // 速度为 0 不显示 "0 B/s"
        assert_eq!(SpeedFormat::text(0), "—");
        assert_eq!(SpeedFormat::text(1_048_576), "1.0 MB/s");
        // 负速度同样不显示数字
        assert_eq!(SpeedFormat::text(-1), "—");
        assert_eq!(SpeedFormat::text(1024), "1.0 KB/s");
    }

    /// 上游 `formatsPercentAgainstZeroTotal`。
    #[test]
    fn formats_percent_against_zero_total() {
        // 除零：不得出 NaN
        assert_eq!(PercentFormat::text(0, 0), "—");
        assert_eq!(PercentFormat::text(5, 0), "—");
        assert_eq!(PercentFormat::text(0, 100), "0%");
        assert_eq!(PercentFormat::text(50, 100), "50%");
        assert_eq!(PercentFormat::text(100, 100), "100%");
        // 超额不得出 200%
        assert_eq!(PercentFormat::text(200, 100), "100%");
    }

    /// 上游 `formatsNonNegativeClamped`。
    #[test]
    fn formats_non_negative_clamped() {
        // 内核的 i64 理论上可为负，不得显示 "-1 B"
        assert_eq!(ByteFormat::text(-1), "0 B");
        assert_eq!(PercentFormat::text(-5, 100), "0%");
        // 取反溢出：不得崩，也不得显示负数
        assert_eq!(ByteFormat::text(i64::MIN), "0 B");
    }

    /// 上游 `percentIsTruncatedNotRounded`。
    ///
    /// 99% 与 100% 之间不能因为四舍五入而提前显示"完了"——客户会以为下完了。
    #[test]
    fn percent_is_truncated_not_rounded() {
        assert_eq!(PercentFormat::text(99, 100), "99%");
        assert_eq!(PercentFormat::text(199, 200), "99%");
        assert_eq!(PercentFormat::text(1, 3), "33%");
    }
}
