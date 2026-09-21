import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 设置窗口的**呈现模型**（任务 10）
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。
//   `SettingsForm` 里有五条背着硬约束的映射：
//     · `-k` 的取值集合**来自内核的 hello**（契约 §2.3），不是壳里的一份硬编码；
//     · 当前值不在那份集合里时**不得被悄悄丢掉**（要显示出来，否则界面会显示成
//       另一个值，而客户改都没改过它）；
//     · `requestBody` 必须**由 `Settings` 编码而来**（`CoreJSON.requestEncoder`，
//       `.convertToSnakeCase`），不得手拼七个键 —— `Settings` 存在的理由就是当
//       "线格式"的单一事实来源，手拼等于把属性名和线格式各写一遍；
//     · 六个区间逐条对着 `core/src/settings.rs:68-98` 的 `validate()`；
//     · `last_code` **不在这里**（它是运行状态，不是用户参数，阶段 A 裁决 #25）。
//
// ⚠️ 夹具用**内核默认值**（`core/src/settings.rs` 的 `default_settings()`），
//    而有判别力的断言一律喂**非默认**取值（见 `everyFieldIsEditableAndSerializedBack`）：
//    拿默认值当期望值等于让夹具替变异体打掩护。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// `-k` 的枚举面
// ---------------------------------------------------------------------------

@Test func minSplitSizeUsesTheChoiceListFromHello() {
    // 规格 §2.3：-k 必须是枚举控件，取值集合由内核的 hello 给出（100 项，1M…100M）
    let f = SettingsForm(settings: .fixture(), choices: ["1M", "20M", "100M"])
    #expect(f.minSplitSizeOptions == ["1M", "20M", "100M"])
}

@Test func aValueOutsideTheChoiceListIsFlaggedNotSilentlyDropped() {
    // 落盘的 settings.json 可能被手改成 21M（内核允许 1…100），而 hello 给了 100 项时不会发生；
    // 但若两者对不上，界面必须显示当前值而不是悄悄回落到第一项
    let f = SettingsForm(settings: .fixture(minSplitSize: "21M"), choices: ["1M", "20M"])
    #expect(f.minSplitSizeOptions.contains("21M"))
}

@Test func theChoiceListIsTheKernelsOwnOrderNotARebuiltOne() {
    // ⚠️ 这条盯的是"枚举面来自内核"这件事**本身**：上面第一条只证明"传进去什么就得到什么"
    //    的**一个实例**，一个把 1M…100M 写死的实现也可能恰好通过它（只要夹具的 choices
    //    与那份硬编码长得一样）。这里换一份**乱序、且不含默认值**的 choices，
    //    任何排序/去重/自行生成的实现都会当场露馅。
    let f = SettingsForm(settings: .fixture(minSplitSize: "3M"), choices: ["3M", "1M", "2M"])
    #expect(f.minSplitSizeOptions == ["3M", "1M", "2M"], "顺序照内核给的，不排序、不去重")
    #expect(f.minSplitSizeNote == nil, "在集合里就没什么可说的")
}

@Test func anOffListValueIsAppendedExactlyOnceAndExplained() {
    // "Flagged" 的另一半：**界面要能说出这件事**（只是"列表里有它"还不够 ——
    // 客户会以为 21M 是内核给的选项之一）。说明里必须带上这个值本身。
    let f = SettingsForm(settings: .fixture(minSplitSize: "21M"), choices: ["1M", "20M"])
    #expect(f.minSplitSizeOptions == ["1M", "20M", "21M"], "不在集合里就追加在末尾，只追加一次")
    let note = f.minSplitSizeNote
    #expect(note?.contains("21M") == true, "说明里要点名这个值：\(note ?? "nil")")
    #expect(note?.isEmpty == false, "不得是一句空话")
}

// ---------------------------------------------------------------------------
// 七个字段：可编辑、一个不少、按线上键名出去
// ---------------------------------------------------------------------------

@Test func everyFieldIsEditableAndSerializedBack() {
    // 七个字段一个不少。⚠️ 七个新值**两两不同、且都不同于夹具的默认值** ——
    //    （3 / 4 / 5 / "100M" / 100_000 / 7 / 60 vs 8 / 16 / 16 / "20M" / 0 / 3 / 1）。
    //    这样"某个字段没被带上"（回落成夹具值）、"两个字段串了"（例如 connections 与
    //    splits 互换）都会当场被抓住；`limitMbps` 特意给 100_000（Int64 的量级），
    //    任何半路被塞进 Int32 的实现都会溢出成另一个数。
    var f = SettingsForm(settings: .fixture(), choices: [])
    f.parallel = 3
    f.connections = 4
    f.splits = 5
    f.minSplitSize = "100M"
    f.limitMbps = 100_000
    f.maxTries = 7
    f.retryWait = 60

    #expect(f.settings == Settings(parallel: 3, connections: 4, splits: 5, minSplitSize: "100M",
                                   limitMbps: 100_000, maxTries: 7, retryWait: 60))

    // 值也要真的落到 requestBody 上（`settings` 对了但 body 走的是另一份拷贝，是另一个 bug）
    guard case .object(let o) = f.requestBody else {
        Issue.record("requestBody 必须是一个 JSON 对象"); return
    }
    #expect(o["parallel"] == .integer(3))
    #expect(o["connections"] == .integer(4))
    #expect(o["splits"] == .integer(5))
    #expect(o["min_split_size"] == .string("100M"))
    #expect(o["limit_mbps"] == .integer(100_000))
    #expect(o["max_tries"] == .integer(7))
    #expect(o["retry_wait"] == .integer(60))
}

@Test func eachFieldLandsInItsOwnCellOnTheWayIn() {
    // ⚠️ **进站方向**（内核手里那一份 → 表单）。`everyFieldIsEditableAndSerializedBack`
    //    守的只是**出站**方向（表单 → `requestBody`），两者不会互相掩护：
    //    把 `SettingsForm.init` 里的 `self.connections = settings.splits` 写错，
    //    出站那条照样全绿 —— 因为 `Settings.fixture()` 的 `connections` 与 `splits`
    //    **都是 16**（内核默认值确实如此，见本文件末尾的夹具说明）。
    //    用户可见后果：打开设置窗口，`-x` 与 `-s` 两格显示对方的数，一个测试都不会红。
    //    所以这里特意让两者**不等**。
    let f = SettingsForm(settings: .fixture(connections: 4, splits: 5), choices: [])
    #expect(f.connections == 4, "-x 这一格拿的必须是内核的 connections")
    #expect(f.splits == 5, "-s 这一格拿的必须是内核的 splits")
}

@Test func setSettingsAlwaysSendsAllSevenKeys() {
    // ⚠️ 内核的 set_settings 没有 serde default：少一个键整个请求就 invalid_params
    // ⚠️ 断的是**键名**，不是键的个数——`core/src/settings.rs:32-47` 的字段名逐字是
    //    snake_case，而 `Settings` 的属性名是 camelCase。按 `Settings` 的 Codable
    //    合成实现编出来同样是 7 个键，只是 `min_split_size`/`limit_mbps` 变成了
    //    `minSplitSize`/`limitMbps`，内核把它们当成缺失键、回 invalid_params。
    //    所以"个数 == 7"这条对"名字对不对"零判别力，这里逐个点名。
    let body = SettingsForm(settings: .fixture(), choices: []).requestBody
    guard case .object(let obj) = body else {
        Issue.record("requestBody 必须是一个 JSON 对象"); return
    }
    #expect(Set(obj.keys) == [
        "parallel", "connections", "splits", "min_split_size",
        "limit_mbps", "max_tries", "retry_wait",
    ])
}

@Test func setSettingsParamsWrapsTheBodyUnderTheSettingsKey() throws {
    // `set_settings` 的 params 形状是 `{"settings": <七项>}`（`core/src/main.rs:1542-1546`
    // 的 `params.get("settings")` + `serde_json::from_value::<Settings>`）。
    // 与 `AppModelTests.applySettingsSendsTheSevenWireKeys` 断的是同一件事，
    // 这里补的是**内层那一份就是 requestBody 本身**（不是各编一次的两份）。
    let f = SettingsForm(settings: .fixture(minSplitSize: "21M"), choices: [])
    guard case .object(let top) = f.setSettingsParams,
          case .object(let inner)? = top["settings"] else {
        Issue.record("set_settings 的 params 形状不对：\(f.setSettingsParams)"); return
    }
    #expect(Set(top.keys) == ["settings"])
    #expect(inner == (try bodyObject(f.requestBody)))
    #expect(Set(inner.keys) == ["parallel", "connections", "splits", "min_split_size",
                               "limit_mbps", "max_tries", "retry_wait"])
}

@Test func theBodyIsDerivedFromTheSettingsTypeNotASecondCopyOfTheWireFormat() throws {
    // ⚠️ 手拼七个键的实现在上面几条上**也可能通过**（今天两者恰好一致），所以这条盯的是
    //    **单一事实来源**：`requestBody` 必须与"拿 `CoreJSON.requestEncoder` 现编同一个
    //    `Settings`"逐字相同。右边是从**类型**重新编出来的，左边若是手抄的副本，
    //    任何一次字段增删/改名都会让两者分叉 —— 而那时内核只会回一条 invalid_params。
    //    （它不是万能的：手抄的副本今天也相等。挡手拼的是约束本身，这条挡的是**漂移**。）
    let f = SettingsForm(settings: .fixture(minSplitSize: "21M", retryWait: 0), choices: [])
    let recomputed = try CoreJSON.decoder.decode(
        JSONValue.self, from: try CoreJSON.requestEncoder.encode(f.settings))
    #expect(f.requestBody == recomputed)
}

@Test func rangesMatchTheCoreLimits() {
    // 逐条核对过 core/src/settings.rs:68-98 的 validate() 与 protocol.rs:318-319
    #expect(SettingsForm.limits.parallel == 1...64)          // ⚠️ 不是 1...16
    #expect(SettingsForm.limits.connections == 1...16)
    #expect(SettingsForm.limits.splits == 1...16)
    #expect(SettingsForm.limits.maxTries == 1...100)
    #expect(SettingsForm.limits.retryWait == 0...60)         // ⚠️ 不是 1...600，且下界是 0
    #expect(SettingsForm.limits.limitMbps == 0...100_000)    // LIMIT_MBPS_MAX = 100_000
}

// ---------------------------------------------------------------------------
// 表单与内核手里那一份的关系（"有没有要保存的改动"）
// ---------------------------------------------------------------------------

@Test func theFormKnowsWhetherItDiffersFromWhatTheKernelHolds() {
    // 界面上"保存"那颗按钮的禁用判据读它。⚠️ 判据必须**逐字段**：
    //    一个恒返回 true（或恒 false）的实现会让按钮永远可点 / 永远点不动，
    //    而这两种失效都不会有任何报错。
    let f = SettingsForm(settings: .fixture(), choices: [])
    #expect(f.matches(.fixture()))
    // 只改一项（不是整份都换）也要看得出来
    #expect(!f.matches(.fixture(retryWait: 60)))
    #expect(!f.matches(.fixture(minSplitSize: "100M")))
    #expect(!f.matches(.fixture(limitMbps: 5)))
    // 内核还没给出那一份时也**不算一致**（`nil` ≠ 一致）：界面据此才肯让客户保存。
    #expect(!f.matches(nil))
}

// ---------------------------------------------------------------------------
// 界面文案里**有语义**的那两句（内核语义，不是排版）
// ---------------------------------------------------------------------------

@Test func zeroMeansUnlimitedAndTheScreenSaysSo() {
    // 简报明文：`0` = 不限速，**界面要写明这一条**（`0` 是一个有专门含义的取值，
    // 客户看到"限速 0"很自然会读成"限速到 0 = 不让下载"，正好反了）。
    #expect(SettingsForm.limitMbpsNote.contains("0"))
    #expect(SettingsForm.limitMbpsNote.contains("不限速"))
}

@Test func theScreenSaysWhenEachSettingApplies() {
    // 内核的两条生效路径不同（`core/src/main.rs:1558-1568` 与 `settings.rs` 的
    // `per_task_options` / `global_options`）：并行数与限速走 `changeGlobalOption`
    // **对已在跑的引擎即时生效**；`-j` 之外的五项是"添加任务时"下发的逐任务选项。
    // 不说清楚，客户改完 `-s` 会以为正在传的任务变了。
    #expect(SettingsForm.applyNote.contains("即时生效"))
    #expect(SettingsForm.applyNote.contains("新任务"))
    #expect(!SettingsForm.applyNote.isEmpty)
}

@Test func aSaveFailureShowsTheKernelTextVerbatim() {
    // 视图不映射 `CoreError`（约束 8）：`CoreError` → 用户可见文案只有
    // `AppModel.message(of:)` 一份实现，`SettingsSaveFailure` 是它在设置窗口的入口。
    // ⚠️ 判别力在这一条上：改写措辞（例如"保存失败，请重试"）会让下面两句全红 ——
    //    而客户最需要看到的恰恰是内核那句"当前 99"和那半句"设置已保存，但…"。
    #expect(SettingsSaveFailure.message(
        of: CoreError.rpc(code: "invalid_params",
                          message: "并行文件数必须在 1–64 之间，当前 99"))
        == "并行文件数必须在 1–64 之间，当前 99")
    // "已落盘、但下发引擎失败"那一支：这句话的**前半句**（设置已保存）是关键信息，
    // 任何统一改写成"保存失败"的实现在这里就丢掉了它。
    let partial = "设置已保存，但下发到下载引擎失败：connection refused"
    #expect(SettingsSaveFailure.message(of: CoreError.rpc(code: "engine_rpc_failed",
                                                          message: partial)) == partial)
    // 闸门那条"壳自己造的 transport"也照登（它是 `.unavailable` 时用户唯一能看到的原文）
    #expect(SettingsSaveFailure.message(of: CoreError.transport("内核没有起来（握手超时）"))
        == "内核没有起来（握手超时）")
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// `Settings` 的夹具：默认值取内核的 `default_settings()`（`core/src/settings.rs:51-61`）。
///
/// ⚠️ 有判别力的断言**不要**拿这些默认值当期望值（见本文件顶部那段注释）。
extension Settings {
    static func fixture(parallel: Int32 = 8,
                        connections: Int32 = 16,
                        splits: Int32 = 16,
                        minSplitSize: String = "20M",
                        limitMbps: Int64 = 0,
                        maxTries: Int32 = 3,
                        retryWait: Int32 = 1) -> Settings {
        Settings(parallel: parallel, connections: connections, splits: splits,
                 minSplitSize: minSplitSize, limitMbps: limitMbps,
                 maxTries: maxTries, retryWait: retryWait)
    }
}

/// 取出一个 JSON 对象（断言里的辅助，失败时给一句人话而不是一个 `try` 崩溃）。
private func bodyObject(_ v: JSONValue) throws -> [String: JSONValue] {
    guard case .object(let o) = v else {
        throw CoreError.malformedResponse("requestBody 不是对象：\(v)")
    }
    return o
}
