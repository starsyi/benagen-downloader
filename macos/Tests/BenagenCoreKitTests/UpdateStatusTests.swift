import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 更新状态在 `AppModel` 这一侧的接线（R27 / ③）
//
// 这里盯的是**壳自己**那两处新契约：
//   · `setUpdateEnabled` —— 成功用**内核回的**那一份刷新界面；**失败返回原文**（界面不许撒谎）；
//   · `pollUpdateStatus` —— 后台那一拍**有待显示的错误时先不打扰它**（R27 + 我报告里那条 `lastError` 观察）。
// 纯值 / 白名单 / 解码在 `UpdateNoticeTests.swift`。
//
// 夹具是**内核 `op_update_status` 逐字的形状**（snake_case，含 `checking` / `checked_at`
// 这两格壳不用的），走 `FakeCore` 的线上 JSON 打桩（与真内核同一条解码口径）。
// ---------------------------------------------------------------------------

private let enabledWire = #"""
{"enabled": true, "checking": false, "current": "0.2.4",
 "latest": null, "has_newer": false, "url": null, "checked_at": 0}
"""#

private let disabledWire = #"""
{"enabled": false, "checking": false, "current": "0.2.4",
 "latest": null, "has_newer": false, "url": null, "checked_at": 0}
"""#

/// 🔴 **③**：成功 ⇒ 回 `nil`，并用**内核回的那一份**刷新（不是我们发出去的那个 `on`），
///    入参是**顶层的布尔**（内核读 `params.get("enabled")`）。
@Test @MainActor func 改开关成功后用返回值刷新界面() async {
    let fake = FakeCore()
    fake.stub("update_status", enabledWire)
    fake.stub("update_set_enabled", disabledWire)
    let model = AppModel(client: fake)
    await model.refreshUpdateStatus()
    #expect(model.updateStatus?.enabled == true, "前置现场没搭起来")

    let failure = await model.setUpdateEnabled(false)

    #expect(failure == nil, "成功了就没有原文要说")
    #expect(model.updateStatus?.enabled == false, "跟着内核回的那一份走")
    #expect(fake.params("update_set_enabled") == .object(["enabled": .bool(false)]),
            "入参是顶层的布尔（内核读 params.get(\"enabled\")）")
}

/// 🔴 **③**：失败 ⇒ 回**内核原文**（约束 3，视图直接摆到屏幕上），并且**一个字都不改**
///    `updateStatus` —— 勾选框据此退回内核手里那一份，**不假装存上了**。
///
/// 判别力：把失败那一支也去改 `updateStatus`（或回 `nil`）⇒ 这一条立刻红。
@Test @MainActor func 改开关失败时回原文且不假装存上() async {
    let fake = FakeCore()
    fake.stub("update_status", enabledWire)
    let model = AppModel(client: fake)
    await model.refreshUpdateStatus()
    #expect(model.updateStatus?.enabled == true)

    fake.stub("update_set_enabled",
              throwing: .rpc(code: "invalid_params", message: "缺少 enabled（布尔）"))
    let failure = await model.setUpdateEnabled(false)

    #expect(failure == "缺少 enabled（布尔）", "内核原文照登（约束 3）")
    #expect(model.updateStatus?.enabled == true, "没存上就不许假装存上了")
}

/// 🔴 **R27 + `lastError`**：**已经有过一次状态**（`updateStatus != nil`）之后，屏上挂着一句失败原文时，
///    后台那一拍**不拉**（`call` 成功会清 `lastError`，而 `update_status` 几乎总成功 ⇒ 会抹掉那句原文）；
///    那句原文被一次成功请求清掉之后，那一拍**自动恢复**。
///
/// 判别力：把 `pollUpdateStatus` 里那道 `lastError == nil` 那一头去掉（任何时刻都拉）⇒
///    第一条 `callCount("update_status") == 1` 之后的断言立刻红。
@Test @MainActor func 有待显示的错误时后台那一拍先不打扰它() async {
    let fake = FakeCore()
    fake.stub("update_status", enabledWire)
    // 造一条"待显示的失败原文"：`refreshVerify` 走 `surfacing`，失败落 `lastError`。
    fake.stub("verify_status", throwing: .malformedResponse("一行垃圾"))
    let model = AppModel(client: fake)

    // ⚠️ 这道闸只在**已经有过状态**之后才生效（见下一条判据：还没过状态时必须放行）。
    await model.refreshUpdateStatus()
    #expect(model.updateStatus != nil, "前置现场没搭起来（还没有过状态）")

    await model.refreshVerify()
    #expect(model.lastError == "一行垃圾", "前置现场没搭起来（lastError 没写上）")

    await model.pollUpdateStatus()
    #expect(fake.callCount("update_status") == 1, "已有过状态、又有待显示的错误时，后台那一拍先不打扰它")

    // 一次**显式**拉（设置页那条路，不带这道闸）成功 ⇒ `lastError` 被清 ⇒ 后台那一拍恢复。
    await model.refreshUpdateStatus()
    #expect(model.lastError == nil)
    await model.pollUpdateStatus()
    #expect(fake.callCount("update_status") == 3, "错误清掉之后才拉（2 次显式 + 1 次后台）")
}

/// 🔴 **R27 的另一头（"开机必然拉一次"）**：**还没拿到过任何状态**（`updateStatus == nil`）时，
///    哪怕屏上已经挂着一句失败原文，后台那一拍也**必须放行** —— 否则开机瞬间 `lastError` 非 nil
///    （握手期的协议级告警 / `load_delivery` 失败留下的原文）就会把文件页这**唯一**一条周期性
///    拉取永久挡死，本次会话的提示条**可能一直不出现**（"开机就该知道有新版本"正因此存在）。
///    一旦**有过状态**（`updateStatus != nil`），失败原文立刻重新受保护、这一拍不再拉。
///
/// 判别力：把闸退回 `guard lastError == nil` ⇒ 第一段 `callCount("update_status") == 1`
///    立刻红（开机那一拍被一句原文挡掉了）。
@Test @MainActor func 还没拿到过状态时开机那一拍必然拉一次() async {
    let fake = FakeCore()
    fake.stub("update_status", enabledWire)
    // 开机瞬间就有一条失败原文：`refreshVerify` 走 `surfacing`，失败落 `lastError`。
    fake.stub("verify_status", throwing: .malformedResponse("一行垃圾"))
    let model = AppModel(client: fake)

    await model.refreshVerify()
    #expect(model.lastError == "一行垃圾", "前置现场没搭起来（lastError 没写上）")
    #expect(model.updateStatus == nil, "前置现场没搭起来（本不该已经有状态）")

    // 还没过状态 ⇒ 放行：开机那一拍必然拉一次（宁可冒"抹掉那句开机原文"的风险）。
    await model.pollUpdateStatus()
    #expect(fake.callCount("update_status") == 1, "没有过状态时，开机那一拍必然拉一次")
    #expect(model.updateStatus != nil, "这一拉把状态落下了")

    // 已经有过状态：把失败原文重新摆上屏之后，这一拍不再拉（原文照旧受保护）。
    await model.refreshVerify()
    #expect(model.lastError == "一行垃圾", "前置现场没搭起来（原文又摆回去了）")
    await model.pollUpdateStatus()
    #expect(fake.callCount("update_status") == 1, "已经有过状态时，失败原文照旧受保护")
}

