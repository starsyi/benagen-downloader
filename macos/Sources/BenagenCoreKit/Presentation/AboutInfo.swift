import Foundation

// ---------------------------------------------------------------------------
// 「关于」窗口里的版本号（任务 12）
//
// 为什么这段回落不在视图里（全局约束 8）：
//   关于窗口里能写出断言的东西只有这一件 —— 版本号从哪来、空了怎么办。
//   视图那一层能写的断言只有"画了个 `Text`"，而版本号读空的后果是**那一行直接消失**：
//   不报错、不是空白块，就是少一行，谁都不会发现（而客户报障时被问到的第一个问题
//   恰恰是"你用的是哪个版本"）。
//
// ⚠️ 本文件**不要** `import SwiftUI`：这里没有任何视图类型，整份都能进 `Tests/` 被单测。
// ---------------------------------------------------------------------------

/// `Info.plist` → 给人看的版本号。
public enum AboutInfo {

    /// 打包脚本写进 `Info.plist` 的两个版本键（`macos/scripts/build_app_macos.sh` 的 heredoc）。
    /// **改这里必须同时改那一边**：对不上的表现是版本号静默变成兜底文案。
    public static let shortVersionKey = "CFBundleShortVersionString"
    public static let buildVersionKey = "CFBundleVersion"

    /// 两条回落都落空时的那句话。**绝不返回空串**。
    ///
    /// 不写成"未知"两个字：要把**缺的是哪个键**说出来 ——
    /// 看到这句话的人（客户 / 分发方）才知道该去查什么。
    public static let unknownVersionText =
        "版本未知（应用包缺少 CFBundleShortVersionString / CFBundleVersion）"

    /// 版本号：`CFBundleShortVersionString` → `CFBundleVersion` → `unknownVersionText`。
    ///
    /// ⚠️ 空白值（`""`、`" "`）与"没有这个键"**同解**：它们在界面上渲染出来都是一行空白，
    ///    而这个兜底存在的理由正是"绝不出现一行看不见的东西"。
    ///
    /// ⚠️ 值不是字符串时（plist 里放了个数字之类）也走回落，**不替打包脚本格式化**：
    ///    版本号长什么样是打包那边的事，壳没有资格猜。
    public static func version(from info: [String: Any]?) -> String {
        text(info?[shortVersionKey]) ?? text(info?[buildVersionKey]) ?? unknownVersionText
    }

    /// 便利入口：应用包 `Info.plist` 那一份。
    public static func version(in bundle: Bundle = .main) -> String {
        version(from: bundle.infoDictionary)
    }

    /// plist 里的一项 → 可显示的版本号；不是非空字符串时回 `nil`。
    private static func text(_ value: Any?) -> String? {
        guard let s = value as? String else { return nil }
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
