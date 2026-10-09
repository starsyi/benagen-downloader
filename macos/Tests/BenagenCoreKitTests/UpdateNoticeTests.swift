import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「有新版本」提示（规格 §3/§4）
//
// 本文件盯的都是**壳自己**的那几条判据，它们各自对应计划里的一条裁定：
//   · R20 —— 判据只有 `has_newer`，不是"`url` 非空"；
//   · R21 —— `latest` 必须是"三段纯数字"，否则整条不出现（守住常驻行的高度不变量）；
//   · R22 —— snake_case 解得开、`enabled` 缺失按 `true` 降级（漏了是"永远不提示"的静默失效）；
//   · R15 —— 「去下载」的链接要过官方发布路径白名单（含路径穿越）。
// ---------------------------------------------------------------------------

/// 官方发布页上一条真会产生的链接（Windows 那侧 `openurl.rs` 的用例里逐字出现过）。
private let officialURL =
    "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/BenagenDownloader-AppleSilicon.dmg"

// ---------------------------------------------------------------------------
// R20：判据只有 has_newer
// ---------------------------------------------------------------------------

@Test func 没有新版就没有提示() {
    #expect(UpdateNotice.of(nil) == nil)
    #expect(UpdateNotice.of(.init(hasNewer: false, latest: "v0.2.5", current: "0.2.4", url: nil)) == nil)
}

@Test func 文案与链接都来自内核那一份() {
    let st = UpdateStatus(hasNewer: true, latest: "v0.2.5", current: "0.2.4",
                          url: "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/x.dmg")
    let n = UpdateNotice.of(st)
    #expect(n?.text == "有新版本 v0.2.5（当前 0.2.4）")
    #expect(n?.url?.host == "gitee.com", "链接必须是内核给的那一个，不自己拼")
}

/// 🔴 **R20**：内核在"没有新版"时**照样可能**给一条指向**已装版本**的链接。
///    按"`url` 非空"渲染会把客户引去下载他自己已经装着的那个版本。
///
/// 判别力：把 `of` 里的 `st.hasNewer` 换成"只看 `st.url != nil`" ⇒ 这一条立刻红。
@Test func 没有新版时即使有链接也不提示() {
    let st = UpdateStatus(hasNewer: false, latest: "v0.2.4", current: "0.2.4",
                          url: "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.4/x.dmg")
    #expect(UpdateNotice.of(st) == nil, "判据是 has_newer，不是 url 非空")
}

// ---------------------------------------------------------------------------
// R21：latest 必须是"三段纯数字"
// ---------------------------------------------------------------------------

/// 🔴 **R21**：一个超长的畸形 `latest`（5000 个 `9`、只有一段）**不许**撑开这条常驻行。
///
/// 判别力：把 `of` 里那道 `isThreeSegmentVersion` 校验丢掉 ⇒ `text` ≈ 5000+ 字符、
/// 这一条立刻红（这正是简报第三条测试与它自己的实现互斥的那个现场）。
@Test func 版本号畸形时长度仍然有界() {
    let st = UpdateStatus(hasNewer: true, latest: String(repeating: "9", count: 5000),
                          current: "0.2.4", url: nil)
    #expect(UpdateNotice.of(st) == nil, "不是三段纯数字 ⇒ 整条提示不出现（长度自然有界）")
}

/// R21 的正反两面表：合法的照常显示（且**原文照登**，壳不改写版本号）；
/// 任何一条不满足的**整条不出现**。
@Test func 非三段纯数字的版本一律不提示() {
    let bad = [
        "",                 // 空串
        "v",                // 只有前导 v
        "0.2",              // 两段
        "0.2.4.0",          // 四段
        "0.2.4-beta",       // 段里带非数字
        "abc",              // 不是数字
        "0..2",             // 中间空段
        "0.2.",             // 末尾空段
        "v 0.2.4",          // 段里有空格
        "0.2.4\n",          // 段里有换行
        " 0.2.4",           // 整串前导空格（trim 之后才合法，这里必须不认）
        "0.2.4 ",           // 整串尾随空格（同上）
        "０.２.４",          // 全角数字（不是 ASCII 数字）
    ]
    for latest in bad {
        let st = UpdateStatus(hasNewer: true, latest: latest, current: "0.2.4", url: nil)
        #expect(UpdateNotice.of(st) == nil, "这个 latest 不是三段纯数字，整条提示都不该出现：\(latest.debugDescription)")
    }

    for latest in ["0.2.4", "v0.2.4", "V1.2.3", "10.20.30"] {
        let st = UpdateStatus(hasNewer: true, latest: latest, current: "0.0.1", url: nil)
        #expect(UpdateNotice.of(st)?.text == "有新版本 \(latest)（当前 0.0.1）",
                "合法版本号原文照登（壳不改写内核的话）：\(latest)")
    }
}

// ---------------------------------------------------------------------------
// R15：「去下载」的链接要过官方发布路径白名单
// ---------------------------------------------------------------------------

/// 🔴 **R15**：不在官方发布路径下的链接**当作没有链接**（提示照常出现、只是不画按钮）。
///
/// 判别力：把 `of` 里的白名单那一关去掉（`st.url.flatMap(URL.init(string:))`）⇒
/// 下面所有"该拒"的用例立刻红。
@Test func 链接只放行官方发布路径() {
    func notice(_ url: String?) -> UpdateNotice? {
        UpdateNotice.of(UpdateStatus(hasNewer: true, latest: "v0.2.5", current: "0.2.4", url: url))
    }

    // 官方发布页下真会产生的链接：放行。
    #expect(notice(officialURL)?.url?.absoluteString == officialURL)
    #expect(notice("https://gitee.com/starsyi/benagen-downloader/releases/tag/v0.2.5")?.url != nil)

    let refused = [
        // 路径穿越（Windows 那侧第一版只判前缀被这个绕过）：
        "https://gitee.com/starsyi/benagen-downloader/releases/../../evil",
        "https://gitee.com/starsyi/benagen-downloader/releases/../../../starsyi/fake",
        "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/..%2f..%2fevil",
        // 相邻主机名（末尾那个 / 承重的那条）：
        "https://gitee.com/starsyi/benagen-downloader/releases.evil.example/x",
        // 别的 scheme / 主机 / 空：不
        "http://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/x.dmg",
        "https://evil.example/",
        // 前缀本身、查询串、片段、反斜杠：拒
        "https://gitee.com/starsyi/benagen-downloader/releases/",
        "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/x.exe?a=b",
        "https://gitee.com/starsyi/benagen-downloader/releases/download/v1/x.exe#frag",
        "https://gitee.com/starsyi/benagen-downloader/releases/evil\\..\\x",
    ]
    for url in refused {
        let n = notice(url)
        #expect(n != nil, "提示本身仍然出现（只是没有链接）：\(url)")
        #expect(n?.url == nil, "这条不该被当作可打开的链接：\(url)")
    }
}

/// 链接缺失（内核回 `null`）⇒ 不画「去下载」，但提示照常出现。
@Test func 内核没给链接时只是不画按钮() {
    let n = UpdateNotice.of(UpdateStatus(hasNewer: true, latest: "v0.2.5", current: "0.2.4", url: nil))
    #expect(n?.url == nil)
    #expect(n?.text == "有新版本 v0.2.5（当前 0.2.4）")
}

// ---------------------------------------------------------------------------
// R22：snake_case 解得开、enabled 缺失按 true
// ---------------------------------------------------------------------------

/// 🔴 **R22**：走**与线上同一条解码口径**（`CoreJSON.decoder`，`.convertFromSnakeCase`）
///    解内核那一份 snake_case。
///
/// 判别力：把 `UpdateStatus` 的 `CodingKeys` 写成线上原文（`case hasNewer = "has_newer"`，
/// 双重转换）⇒ `has_newer` 解不开（`keyNotFound`）；`latest` / `enabled` 一并对不上 ⇒
/// 这一条红。（这也是那条"永远不提示且没有东西会红"的桩。）
@Test func 解码内核的蛇形键与缺省开关() throws {
    // 逐字是内核 `op_update_status` 回的那一份（含 `checking` / `checked_at` 这两格壳不用的）。
    let wire = #"""
    { "enabled": true, "checking": false, "current": "0.2.4",
      "latest": "v0.2.5", "has_newer": true,
      "url": "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/BenagenDownloader-AppleSilicon.dmg",
      "checked_at": 1791600000 }
    """#
    let st = try CoreJSON.decoder.decode(UpdateStatus.self, from: Data(wire.utf8))
    #expect(st.hasNewer == true, "`has_newer` 必须映射到 `hasNewer`")
    #expect(st.latest == "v0.2.5")
    #expect(st.current == "0.2.4")
    #expect(st.url?.contains("releases/download/v0.2.5/") == true)
    #expect(st.enabled == true)

    // 没有新版时内核回 `latest: null` / `url: null`（`last_seen` 是 `Option<String>`）。
    let noUpdate = #"{"has_newer": false, "latest": null, "current": "0.2.4", "url": null, "enabled": false}"#
    let st2 = try CoreJSON.decoder.decode(UpdateStatus.self, from: Data(noUpdate.utf8))
    #expect(st2.hasNewer == false)
    #expect(st2.latest == nil)
    #expect(st2.url == nil)
    #expect(st2.enabled == false)

    // 🔴 `enabled` 缺失 ⇒ 按 `true` 降级（开关默认开），**不许**整份解码失败。
    let withoutEnabled = #"{"has_newer": false, "latest": null, "current": "0.2.4", "url": null}"#
    let st3 = try CoreJSON.decoder.decode(UpdateStatus.self, from: Data(withoutEnabled.utf8))
    #expect(st3.enabled == true, "缺这一格要降级成默认开，而不是让整份解码失败")
    #expect(st3.hasNewer == false)
}
