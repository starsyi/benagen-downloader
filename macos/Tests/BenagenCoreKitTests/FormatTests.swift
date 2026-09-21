import Testing
@testable import BenagenCoreKit

// 全部是**纯函数**的边界值。这一层最容易"看起来对就行"，所以每个边界逐个钉住。
// ⚠️ 一条也不许改成 `ByteCountFormatter`：它的输出随 locale 变（测试会随机器语言环境红），
//    且默认是十进制 1000（1024 会被显示成 "1 KB" 而 1000 也是 "1 KB"）。

@Test func formatsBytesWithBinaryUnits() {
    #expect(ByteFormat.text(0) == "0 B")
    #expect(ByteFormat.text(1) == "1 B")
    #expect(ByteFormat.text(1023) == "1023 B")
    #expect(ByteFormat.text(1024) == "1.0 KB")
    #expect(ByteFormat.text(1536) == "1.5 KB")
    #expect(ByteFormat.text(1_048_576) == "1.0 MB")
    #expect(ByteFormat.text(65_536) == "64.0 KB")
    #expect(ByteFormat.text(5_368_709_120) == "5.0 GB")
}

@Test func formatsBytesAcrossEveryUnitBoundary() {
    // 1024 进制：每一级的边界都必须换单位，且**不能用十进制 1000 的口径**
    #expect(ByteFormat.text(1000) == "1000 B")          // 十进制口径会在这里变成 "1.0 KB"
    #expect(ByteFormat.text(1_048_575) == "1024.0 KB")  // 换单位前最后一个值
    #expect(ByteFormat.text(1_073_741_824) == "1.0 GB")
    #expect(ByteFormat.text(1_099_511_627_776) == "1.0 TB")
    #expect(ByteFormat.text(1_125_899_906_842_624) == "1.0 PB")
}

@Test func formatsSpeed() {
    #expect(SpeedFormat.text(0) == "—")          // 速度为 0 不显示 "0 B/s"
    #expect(SpeedFormat.text(1_048_576) == "1.0 MB/s")
    #expect(SpeedFormat.text(-1) == "—")         // 负速度同样不显示数字
    #expect(SpeedFormat.text(1024) == "1.0 KB/s")
}

@Test func formatsPercentAgainstZeroTotal() {
    #expect(PercentFormat.text(done: 0, total: 0) == "—")   // 除零：不得出 NaN
    #expect(PercentFormat.text(done: 5, total: 0) == "—")
    #expect(PercentFormat.text(done: 0, total: 100) == "0%")
    #expect(PercentFormat.text(done: 50, total: 100) == "50%")
    #expect(PercentFormat.text(done: 100, total: 100) == "100%")
    #expect(PercentFormat.text(done: 200, total: 100) == "100%")  // 超额不得出 200%
}

@Test func formatsNonNegativeClamped() {
    #expect(ByteFormat.text(-1) == "0 B")        // 内核的 i64 理论上可为负，不得显示 "-1 B"
    #expect(PercentFormat.text(done: -5, total: 100) == "0%")
    #expect(ByteFormat.text(Int64.min) == "0 B") // 取反溢出：不得崩，也不得显示负数
}

@Test func percentIsTruncatedNotRounded() {
    // 99% 与 100% 之间不能因为四舍五入而提前显示"完了"——客户会以为下完了。
    #expect(PercentFormat.text(done: 99, total: 100) == "99%")
    #expect(PercentFormat.text(done: 199, total: 200) == "99%")
    #expect(PercentFormat.text(done: 1, total: 3) == "33%")
}
