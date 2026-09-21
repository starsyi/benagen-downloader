import Foundation

// ---------------------------------------------------------------------------
// 呈现基础件：**全部是纯函数**，无状态、无 locale 依赖。
//
// ⚠️ 一条也不要用 `ByteCountFormatter`：它的输出**随机器的语言环境变**
//    （同一份代码在中文/英文/任意 region 的 Mac 上显示不同，测试也会跟着红），
//    而且默认口径是**十进制 1000**（1024 与 1000 都会被显示成 "1 KB"）。
//    这里的口径是 1024 进制、一位小数、由本文件逐字钉住。
//
// ⚠️ **本文件还有一份 Rust 孪生**：`core/src/cli/progress.rs`（命令行工具 `benagen-dl`
//    的进度行）。**改一处必须改另一处** —— 同一个数字在图形客户端与命令行上显示成两样
//    （或一边 `0%` 一边 `—`），客户会当成两个不同的事实，而这**不会**报错。
// ---------------------------------------------------------------------------

/// 字节数 → 人话。`0` 特例、"不足 1 KB 不带小数"、负数夹到 `0`。
public enum ByteFormat {
    /// 1024 进制。顺序即换单位的次序。
    private static let units = ["B", "KB", "MB", "GB", "TB", "PB"]

    /// ⚠️ **必须夹到非负**：内核的 `size`/`completed` 是 i64，理论上可为负
    ///（畸形清单、溢出），而 `-1 B` 这种东西不该出现在客户面前。
    public static func text(_ bytes: Int64) -> String {
        let v = max(0, bytes)
        if v < 1024 {
            return "\(v) B"
        }
        var x = Double(v)
        var i = 0
        while x >= 1024 && i < units.count - 1 {
            x /= 1024
            i += 1
        }
        // `String(format:)` 不带 locale：小数点固定是 `.`，不随机器语言环境变。
        return String(format: "%.1f", x) + " " + units[i]
    }
}

/// 速度 → 人话。**`0` 显示破折号，不显示 `0 B/s`**：
/// "速度未知/已停止"与"速度真的是每字节零"在界面上是同一件事，而后者更常见的是前者。
public enum SpeedFormat {
    public static func text(_ bytesPerSecond: Int64) -> String {
        guard bytesPerSecond > 0 else { return "—" }
        return ByteFormat.text(bytesPerSecond) + "/s"
    }
}

/// 进度 → 人话。
public enum PercentFormat {
    /// ⚠️ **`total == 0` 时返回 `"—"`，不是 `"0%"`，更不是 `NaN`**：
    /// 清单还没加载时总量就是 0，显示 `0%` 会让客户以为"一个文件都没下"。
    ///
    /// 口径与内核的 `view::progress` **逐条对齐**（契约 §1.6）：
    /// **整数除法向零截断**（不是四舍五入——四舍五入会让 99.5% 提前显示成 100%，
    /// 客户以为下完了），且夹到 `[0, 100]`（任务超报不得出 200%）。
    ///
    /// ⚠️ 孪生实现在 `core/src/cli/progress.rs` 的 `percent` / `percent_text`
    /// （Rust 那份把这条规则从 `view::progress` 里抄成了公开函数）。**改一处必须改另一处。**
    public static func text(done: Int64, total: Int64) -> String {
        guard total > 0 else { return "—" }
        let d = min(max(0, done), total)
        let (scaled, overflow) = d.multipliedReportingOverflow(by: 100)
        let pct: Int64 = overflow ? Int64((Double(d) / Double(total)) * 100) : scaled / total
        return "\(min(100, max(0, pct)))%"
    }
}
