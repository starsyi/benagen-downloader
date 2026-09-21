import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// DeliverySummary 的测试（全局约束 8：`Presentation/` 里的每条纯计算都要有单测）
//
// ⚠️ 夹具一律是**内核会发的那种线上 JSON 文本**（键是 snake_case，时间戳形态照抄
//    `ProtocolTests` 里那条真实抓取：`"2026-09-17T09:37:12.805751+08:00"`），
//    经 `CoreJSON.decoder` 解成 `DeliveryInfo`。手搓结构体会让"壳解不解得动内核的输出"
//    在测试里凭空消失。
//
// ⚠️ 夹具的数值**两两不同、且非零**（7 个文件 / 3 GiB / 两个不同的时间戳）——
//    这是阶段 A 第五类错误（字段没被搬运的变异体活下来）的防线。
// ---------------------------------------------------------------------------

/// 真形态的 `DeliveryInfo`。默认值全部非空、数值全部非零。
private func deliveryInfo(code: String = "C24-8_×_25WS024",
                          createdAt: String = "2026-09-01T10:00:00+08:00",
                          expiresAt: String = "2026-10-14T16:13:34+08:00",
                          expired: Bool = false,
                          totalFiles: Int64 = 7,
                          totalBytes: Int64 = 3_221_225_472) throws -> DeliveryInfo {
    let json = """
    {"code":"\(code)","page_url":"http://dl.example/C24-8/index.html","base_url":"http://dl.example",
     "created_at":"\(createdAt)","expires_at":"\(expiresAt)","expired":\(expired),
     "total_files":\(totalFiles),"total_bytes":\(totalBytes),
     "tree":{"type":"dir","name":"","children":{}}}
    """
    return try CoreJSON.decoder.decode(DeliveryInfo.self, from: Data(json.utf8))
}

// ---------------------------------------------------------------------------
// 侧边栏底部那块摘要（规格 §7.1：批次号 / 文件数 / 总大小 / 有效期）
// ---------------------------------------------------------------------------

@Test func summaryShowsBatchNumberFilesAndTotalSize() throws {
    let s = DeliverySummary.of(try deliveryInfo())

    #expect(s.code == "C24-8_×_25WS024", "批次号（交付码）原文照登")
    #expect(s.filesText == "7 个文件")
    #expect(s.sizeText == "3.0 GB", "口径复用 ByteFormat（1024 进制、一位小数）")
    #expect(s.validityText == "有效期至 2026-10-14 16:13")
    #expect(s.expiredBadgeText == nil, "没过期的批次不该挂过期标记")
}

@Test func summaryMarksExpiredBatches() throws {
    let expired = DeliverySummary.of(try deliveryInfo(expired: true))

    #expect(expired.expiredBadgeText == "已过期", "过期必须让客户看得见（规格 §7.1）")

    // ⚠️ 反向的一半也断言：否则"恒返回已过期"的变异体也能活下来。
    #expect(DeliverySummary.of(try deliveryInfo(expired: false)).expiredBadgeText == nil)
}

@Test func summaryHandlesUnparseableTimestamps() throws {
    // `created_at` / `expires_at` 是内核**原样透传**的字符串，可能为空串、也可能不是
    // RFC3339。格式化失败必须**回落到原文** —— 绝不显示 "Invalid Date"（简报明文）。
    #expect(DeliverySummary.of(try deliveryInfo(expiresAt: "")).validityText == "有效期至 —",
            "空串给占位符，不给 'nil'、不给空白")
    #expect(DeliverySummary.of(try deliveryInfo(expiresAt: "待定")).validityText == "有效期至 待定",
            "不是时间戳就原样显示")
    #expect(DeliverySummary.of(try deliveryInfo(expiresAt: "2026-10-14T16:13:34")).validityText
            == "有效期至 2026-10-14T16:13:34",
            "缺时区偏移不是 RFC3339（内核的 parse_rfc3339 同样拒绝它）→ 回落到原文")
}

@Test func summaryDoesNotInventTextForEmptyFields() throws {
    let s = DeliverySummary.of(try deliveryInfo(code: "", createdAt: "", expiresAt: "",
                                                totalFiles: 0, totalBytes: 0))

    #expect(s.code == "—", "空批次号不得渲染成空串（更不得渲染成字面量 nil）")
    #expect(s.filesText == "0 个文件", "一个文件都没有也要说 0，不是空白")
    #expect(s.sizeText == "0 B")
    #expect(s.validityText == "有效期至 —")
    #expect(s.expiredBadgeText == nil)

    // 逐格扫一遍：任何一格都不许出现 nil / null / Invalid（约束 3：壳不编文案）。
    for field in [s.code, s.filesText, s.sizeText, s.validityText] {
        let lower = field.lowercased()
        #expect(!lower.contains("nil"), "「\(field)」里出现了 nil")
        #expect(!lower.contains("null"), "「\(field)」里出现了 null")
        #expect(!lower.contains("invalid"), "「\(field)」里出现了 Invalid")
    }
}

@Test func summaryIsAbsentUntilAManifestIsLoaded() throws {
    let info = try deliveryInfo()

    // 没加载（含加载中、失败）时摘要为 nil —— 界面上那格显示占位，**不显示上一批的批次号**。
    #expect(DeliverySummary.of(.idle) == nil)
    #expect(DeliverySummary.of(.loading) == nil)
    #expect(DeliverySummary.of(.failed("拉取交付清单失败：HTTP 404")) == nil)

    #expect(DeliverySummary.of(.loaded(info))?.code == "C24-8_×_25WS024")
    #expect(DeliverySummary.of(.loaded(info))?.sizeText == "3.0 GB")
}

// ---------------------------------------------------------------------------
// 空态页：这次点按钮该用哪个码
// ---------------------------------------------------------------------------

@Test func typedCodeWinsOverTheRememberedOne() {
    // 用户在输入框里敲了东西 —— 那就是他要加载的（哪怕框里那个是错的，
    // 界面显示的错误也必须是**他敲的那个码**换来的原文）。
    #expect(DeliveryCodeEntry.resolve(typed: "AbCdEfGhIjKlMnOpQrSt",
                                      remembered: "C24-8") == "AbCdEfGhIjKlMnOpQrSt")
}

@Test func anEmptyFieldFallsBackToTheRememberedCode() {
    // ⚠️ 空输入框 + 「重试」不是"什么都不做"：启动时的自动加载失败时输入框本来就是空的，
    //    而那颗按钮要重试的正是**刚才失败的那一个**（内核记着的那个码）。
    #expect(DeliveryCodeEntry.resolve(typed: "", remembered: "C24-8") == "C24-8")
    #expect(DeliveryCodeEntry.resolve(typed: "", remembered: "") == "",
            "两边都空就还是空（调用方据此禁用按钮，不要发一条空码的请求）")
}

// ---------------------------------------------------------------------------
// 交付码的长度上界（全局约束 C-3）
//
// 背景：内核的行长上限是 8 MiB（`core/src/main.rs:113`），而它超限时的处置**不是报错** ——
// 只回一条 `id == 0` 的协议告警、那条请求**永远等不到响应**，`CoreClient` 那条 FIFO
// 串行队列于是被**永久堵死**，界面上一个字都不说。所以壳侧**绝不**允许发出可能超限的请求。
// 这一节钉住"判据本身"，`AppModelTests.anOversizedDeliveryCodeNeverReachesTheKernel` 钉住
// "它真的挡住了那条请求"。
// ---------------------------------------------------------------------------

@Test func deliveryCodeBoundAndItsWordingAgree() {
    // ⚠️ 上界与那句人话是**两个**常量（一处给代码、一处给用户），改一个忘了另一个
    //    就会出现"界面说 2 KiB、代码挡在别处"的分叉。这条把它们钉在一起。
    #expect(DeliveryCodeEntry.maximumBytes == 2 << 10, "上界就是 2 KiB —— 改它请连同这一条")
    #expect(DeliveryCodeEntry.maximumText == "2 KiB")
    #expect(DeliveryCodeEntry.tooLongHint.contains(DeliveryCodeEntry.maximumText),
            "提示里说的数必须与判据用的数一致：\(DeliveryCodeEntry.tooLongHint)")
    // 提示必须**说出为什么**（否则就只剩一颗灰按钮，约束 4）与**该怎么做**。
    #expect(DeliveryCodeEntry.tooLongHint.contains("过长"), "要说清楚是因为太长")
    #expect(DeliveryCodeEntry.tooLongHint.contains("链接"), "要给出出路（只粘贴交付码或交付页链接）")

    // `base_url`（空态页「高级」）与 `code` 是**同一条请求**上的两个用户可输入字段，
    // 所以共用同一个上界与同一套要求 —— 下面两条钉住"那一半也没有被落下"。
    #expect(DeliveryCodeEntry.baseURLTooLongHint.contains(DeliveryCodeEntry.maximumText),
            "两个字段说的数必须是同一个：\(DeliveryCodeEntry.baseURLTooLongHint)")
    #expect(DeliveryCodeEntry.baseURLTooLongHint.contains("自定义下载地址"),
            "要说清楚过长的是哪一个字段（否则用户不知道该改哪一格）")
}

@Test func theCodeBoundIsNotAShapeCheck() {
    // ⚠️ **本组的核心纪律**：这里只设**长度**上界，**不校验形状**。
    //    内核的 `delivery::extract_code` 同时接受交付链接与裸码，壳若要求"必须 20 位"
    //    就会把客户从交付邮件里整条粘进来的合法输入**误杀** —— 那正是空态页那行提示
    //    （「整条链接也可以直接粘进来」）鼓励用户去做的事。
    //
    // 下面每一条都是**会被形状校验误杀**的合法输入（长度都在上界之内 ⇒ 都必须放行）：
    let legit = [
        "https://download.benagen.com/C24-8_×_25WS024/C24-8_×_25WS024.html",  // 整条交付页链接
        "http://download.benagen.com/AbCdEfGhIjKlMnOpQrSt/index.html?from=mail&batch=3",
        "C24-8",                      // 比 20 位短（不是码的形状，但内核自己会找）
        "C24-8_×_25WS024",            // 含非 ASCII 的批次号
        "  两边有空白  ",
        "交付码在邮件里：AbCdEfGhIjKlMnOpQrSt",   // 混着中文说明
    ]
    for code in legit {
        #expect(DeliveryCodeEntry.isSendable(code), "合法粘贴被误杀：\(code)")
        #expect(DeliveryCodeEntry.tooLong(code) == false, "同上：\(code)")
    }
}

@Test func onlyTheLengthBoundRejects() {
    // 边界是**闭区间**：恰好 2 KiB 放行、多 1 字节拒绝。
    let atBound = String(repeating: "A", count: DeliveryCodeEntry.maximumBytes)
    let overBound = atBound + "A"

    #expect(DeliveryCodeEntry.isSendable(atBound), "恰好到上界仍然可以发")
    #expect(DeliveryCodeEntry.tooLong(atBound) == false)
    #expect(DeliveryCodeEntry.tooLong(overBound), "超 1 字节就必须拒发（边界是闭区间）")

    // ⚠️ 判据是 **UTF-8 字节数**，不是 `count`：一个中文字符占 3 字节。
    //    用 `count` 判的话，下面这个串（其 `count` 只有 1024，看起来"没超"）会被放过，
    //    而它其实是 3072 字节 —— 与内核量的不是同一个东西。
    let multibyte = String(repeating: "码", count: 1024)
    #expect(multibyte.count < DeliveryCodeEntry.maximumBytes, "前提：这个串的**字符数**在上界之内")
    #expect(multibyte.utf8.count > DeliveryCodeEntry.maximumBytes, "前提：它的**字节数**超了")
    #expect(DeliveryCodeEntry.tooLong(multibyte), "必须按字节判（与内核的行长同一量纲）")

    // 9 MiB（复审给的那个量级）：远超上界，也远超误判余量。
    #expect(DeliveryCodeEntry.tooLong(String(repeating: "A", count: 9 << 20)))
}

// ---------------------------------------------------------------------------
// 时间戳的呈现
// ---------------------------------------------------------------------------

@Test func timestampsKeepTheSourceWallClockAndDropSubMinuteDetail() {
    // ⚠️ **不换算到本机时区**：显示的是原文里那个墙上时间。
    //    换算的后果是同一份清单在两台时区不同的机器上显示不同的有效期，而"有效期"
    //    是客户与业务方对账的依据；测试也会跟着变成随机器而变。
    #expect(TimestampPresentation.text("2026-10-14T16:13:34+08:00") == "2026-10-14 16:13")
    #expect(TimestampPresentation.text("2026-09-17T09:37:12.805751+08:00") == "2026-09-17 09:37",
            "内核真实抓取带 6 位小数秒")
    #expect(TimestampPresentation.text("2026-10-14T16:13:34Z") == "2026-10-14 16:13")
    #expect(TimestampPresentation.text("2026-10-14T23:13:34-05:00") == "2026-10-14 23:13")
}

@Test func timestampsFallBackToTheRawText() {
    #expect(TimestampPresentation.text("") == "")
    #expect(TimestampPresentation.text("待定") == "待定")
    #expect(TimestampPresentation.text("2026-10-14") == "2026-10-14", "只有日期没有时刻：不猜时刻")
    #expect(TimestampPresentation.text("2026-10-14 16:13:34") == "2026-10-14 16:13:34",
            "空格分隔不是 RFC3339（且长度不够）")
    #expect(TimestampPresentation.text("2026-10-14T16:13:34+08:00 尾巴") == "2026-10-14T16:13:34+08:00 尾巴",
            "时区偏移后面还有别的东西 → 整串回落到原文")
    #expect(TimestampPresentation.text("2026-10-14T16:13:34+0800") == "2026-10-14T16:13:34+0800",
            "偏移必须带冒号（RFC3339）；不带就回落原文，不猜")
}
