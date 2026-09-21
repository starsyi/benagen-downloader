// probe_macos_appearance.swift —— **设计期的量尺**，把 macOS 的系统色与语义字号量成具体值。
//
//     swift windows/scripts/probe_macos_appearance.swift
//
// ---------------------------------------------------------------------------
// 为什么需要它
// ---------------------------------------------------------------------------
//   `docs/superpowers/2026-09-19-macos-visual-inventory.md` 要回答"macOS 客户端长什么样"，
//   而 macOS 侧**一个 hex 都没有**（全仓零 `.colorset`、零 `Color(red:…)`，只有
//   `.blue` / `.secondary` 这类**语义色**）——那些名字在不同外观下解析成不同的值。
//   ⇒ 读源码**永远得不到**具体颜色，"照搬 macOS 观感"这件事就缺一个可核对的起点。
//
//   所以这份清点的颜色与字号**一律从本脚本量出来**，不是从记忆里抄的。
//   `2026-09-19-macos-visual-inventory.md` §1 那张表就是本脚本某一次的读数。
//
// ⚠️ **这不是构建链的一部分，也没有任何东西依赖它。**
//    它只在"设计令牌需要重新对齐"时由人手动跑一次（比如 macOS 大版本升级后重取）。
//    别把它接进 `test.sh`：它是一个**量尺**，不是判据 —— 把它当判据的话，
//    苹果在任一次系统更新里微调一个色值都会让本仓库变红，而那与我们的对错无关。
//    判据要判**性质**（见 `make_font_subset.sh` 文件头对"判产物字节"的同一条论述）。
//
// ⚠️ **读数依赖当前外观**：本脚本在无 GUI 的 CLI 里跑，外观取 Aqua（浅色）。
//    要量深色外观得先让进程处在深色 appearance 下 —— **今天不需要**（规格 §13：
//    不做主题切换，Windows 版只有浅色一档）。
//
// ⚠️ **`quaternaryLabelColor` 出现两次是刻意的**（下面 `items` 里）：
//    常驻提示条的底色是 `.quaternary.opacity(0.4)`，而 `.quaternary` 本身**已经带 0.098
//    的 alpha**（不是不透明的灰）⇒ 最终色是"黑 @ 0.098 × 0.4 ≈ 0.039"，**不是** 0.098。
//    两行并排是给读的人看清这一步乘法从哪来，**不要**把第二行当成重复项删掉。

import AppKit

/// 把一个 `NSColor` 落成 `#RRGGBB  a=…`。
///
/// ⚠️ 必须先 `usingColorSpace(.sRGB)`：系统语义色是**动态色**（同一个 `NSColor` 在浅色/
///    深色下是两套值），不指定色彩空间就读不到分量。转换后的 alpha 要**照实打印** ——
///    它是"这个颜色其实半透明"这件事的唯一线索（见文件头对 `.quaternary` 的说明）。
func hex(_ c: NSColor) -> String {
    guard let s = c.usingColorSpace(.sRGB) else { return "n/a（转换不到 sRGB）" }
    return String(
        format: "#%02X%02X%02X  a=%.3f",
        Int((s.redComponent * 255).rounded()),
        Int((s.greenComponent * 255).rounded()),
        Int((s.blueComponent * 255).rounded()),
        s.alphaComponent)
}

// ---- 语义色（清点 §1.1 / §1.2 那一组）--------------------------------------
// 顺序即清点文档里的顺序。「谁配哪个语义」的指派在 Swift 侧（4 处 `tint(_:)`），
// 本脚本只负责"这些名字各自等于什么值"。
let items: [(String, NSColor)] = [
    ("systemRed", .systemRed),
    ("systemOrange", .systemOrange),
    ("systemGreen", .systemGreen),
    ("systemBlue", .systemBlue),
    ("systemGray", .systemGray),
    ("labelColor", .labelColor),
    ("secondaryLabelColor", .secondaryLabelColor),
    ("tertiaryLabelColor", .tertiaryLabelColor),
    ("quaternaryLabelColor", .quaternaryLabelColor),
    ("windowBackgroundColor", .windowBackgroundColor),
    ("controlBackgroundColor", .controlBackgroundColor),
    ("separatorColor", .separatorColor),
    ("selectedContentBackgroundColor", .selectedContentBackgroundColor),
    ("textColor", .textColor),
]

print("=== 语义色（sRGB，当前外观）===")
for (name, color) in items {
    print(String(format: "%-30s %@", (name as NSString).utf8String!, hex(color)))
}

// ---- 语义字号（清点 §1.4）--------------------------------------------------
// ⚠️ 打的是 `pointSize` 与**实际解析出的字体名**：后者是"全仓没有指定字体"这件事的
//    证据（`.SFNS-*` = 系统字体 SF Pro 的私有族名）。哪一天这里冒出一个非 `.SFNS-`
//    的名字，就说明有人开始指定字体了 —— 那会让规格 §7.1 的字体栈推理失效。
print("\n=== 语义字号（NSFont.preferredFont）===")
let styles: [(String, NSFont.TextStyle)] = [
    ("largeTitle", .largeTitle),
    ("title", .title),
    ("title2", .title2),
    ("title3", .title3),
    ("headline", .headline),
    ("body", .body),
    ("callout", .callout),
    ("caption", .caption1),
    ("caption2", .caption2),
]
for (name, style) in styles {
    let f = NSFont.preferredFont(forTextStyle: style)
    print("\(name): \(f.pointSize)pt  \(f.fontName)")
}
