import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「关于」窗口里的版本号（任务 12）
//
// 为什么这件事要有一条单测：版本号是**打包脚本现写进 Info.plist** 的
// （`build_app_macos.sh` 的 heredoc，`VERSION` 默认 `0.1.0`），壳这边只是照着读。
// 读空的后果是一个**没人会发现的空白**——关于窗口里应用名下面那一行直接消失，
// 而"这一版到底是哪个版本"是客户报障时唯一能被问的东西。
//
// ⚠️ 这里**不碰 `Bundle.main`**：喂的是**手工造的 Info.plist 字典**，
//    所以三条回落规则（短版本 → 构建号 → 明确文案）都能被驱动。
// ---------------------------------------------------------------------------

@Test func thePlistKeysAreTheOnesThePackagerWrites() {
    // 与 `build_app_macos.sh` 的 Info.plist heredoc 里那两个键逐字一致。
    // 写错的后果是版本号静默变成兜底文案（不会报错，只是显示"未知"）。
    #expect(AboutInfo.shortVersionKey == "CFBundleShortVersionString")
    #expect(AboutInfo.buildVersionKey == "CFBundleVersion")
}

@Test func theShortVersionIsUsedWhenPresent() {
    // `CFBundleShortVersionString` 是给人看的版本号（"1.2.3"），优先用它。
    #expect(AboutInfo.version(from: ["CFBundleShortVersionString": "1.2.3",
                                     "CFBundleVersion": "45"]) == "1.2.3")
}

@Test func anEmptyShortVersionFallsBackToTheBuildNumber() {
    // 键在、值是空串：与"没有这个键"同解 —— 界面上都是一行空白。
    #expect(AboutInfo.version(from: ["CFBundleShortVersionString": "",
                                     "CFBundleVersion": "45"]) == "45")
    // 只有空白字符同理（" " 渲染出来同样是什么都看不到）。
    #expect(AboutInfo.version(from: ["CFBundleShortVersionString": " \n ",
                                     "CFBundleVersion": "45"]) == "45")
    #expect(AboutInfo.version(from: ["CFBundleVersion": "45"]) == "45")
}

@Test func aNonStringValueFallsBackInsteadOfBeingFormatted() {
    // plist 里放了个数字：回落，而不是"顺手格式化一下" —— 版本号是字符串，
    // 壳没有资格替打包脚本决定它长什么样。
    #expect(AboutInfo.version(from: ["CFBundleShortVersionString": 123,
                                     "CFBundleVersion": "45"]) == "45")
}

@Test func theVersionIsNeverAnEmptyString() {
    // 两条回落都落空时**必须**是一句明确的话，绝不返回空串：
    // 空的版本行 = 关于窗口里少一行，谁都不会发现。
    for info: [String: Any]? in [nil, [:], ["CFBundleShortVersionString": "",
                                            "CFBundleVersion": "   "]] {
        let text = AboutInfo.version(from: info)
        #expect(!text.isEmpty, "版本号不得为空串")
        #expect(text.contains("CFBundleShortVersionString"),
                "兜底文案要说清缺的是哪个键，用户/分发方才知道去查什么")
    }
}
