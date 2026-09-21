import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 夹具：**真实抓取**，不是手编的
// ---------------------------------------------------------------------------
//
// 抓取方式（真内核，`core/target/release/benagen-core`）：
//
//   printf '%s\n' '{"id":1,"method":"hello","params":{"protocol":1}}' \
//     | ./core/target/release/benagen-core
//
// ⚠️ 下面第一条里的 `min_split_size_choices` 是**截短过的**：真内核回的是
//    `["1M","2M",…,"100M"]` 整整 100 项，这里为可读性留了前 3 项。
//    **其余字段逐字来自真实抓取**（`protocol` 是 `1`，它是 result 里排序后的最后一个键）。
//    100 项那条的形状由 `helloChoiceListIsTheWholeContractRange` 单独钉住。

private let HELLO_OK = #"{"id":1,"ok":true,"result":{"min_split_size_choices":["1M","2M","3M"],"protocol":1}}"#

// 真实抓取：`{"id":2,"method":"get_settings","params":{}}`（逐字，未截断）
private let SETTINGS_OK = #"{"id":2,"ok":true,"result":{"last_code":"","settings":{"connections":16,"limit_mbps":0,"max_tries":3,"min_split_size":"20M","parallel":8,"retry_wait":1,"splits":16}}}"#

// 真实抓取：`{"id":1,"method":"hello","params":{"protocol":99}}`（逐字，未截断）
private let MISMATCH = #"{"id":1,"ok":false,"error":{"code":"protocol_mismatch","message":"协议版本不匹配：内核是 1，请求方是 99"}}"#

// 真实抓取：`{"id":3,"method":"verify_status"}`（逐字，未截断）
private let VERIFY_STATUS_OK = #"{"id":3,"ok":true,"result":{"all_good":true,"bad":[],"missing":[],"ok":[],"size_mismatch":[],"unreadable":[],"unverifiable":[]}}"#

// 真实抓取：`{"id":3,"method":"transfer_list"}` 里 `items[]` 的一条（`enqueue` 之后立刻取快照）。
private let TRANSFER_ITEM_ACTIVE = #"{"completed":0,"conns":1,"error_message":"","gid":"d2e08fde0dbd0be4","path":"Readme.txt","raw_status":"active","speed":0,"state":"active","total":0}"#

// 真实抓取：同一批任务下载完之后再取一次，`items[]` 里的一条。
private let TRANSFER_ITEM_COMPLETE = #"{"completed":6,"conns":0,"error_message":"","gid":"a7c84c73a72736de","path":"Readme.txt","raw_status":"complete","speed":0,"state":"complete","total":6}"#

// ⚠️ **这一条不是抓取**：形状逐字照上面两条（同一份 `TransferItem` 序列化），
//    但取值是人为拉开的。理由：`paused` 这条状态我在真内核上没能复现出来
//    （先 `enqueue` 拿到的 GID、下一条 `task_action pause` 就报 `GID … is not found`
//    ——`enqueue` 与 `task_action` 跑在两个进程里，GID 在不同批次间不通用）。
//    取值故意都取非零/非空，否则"字段搬运"改坏了也看不出来（阶段 A 的 I-1 就是这么栽的）。
private let TRANSFER_ITEM_PAUSED = #"{"completed":32768,"conns":1,"error_message":"连接超时","gid":"g-paused","path":"a/b.txt","raw_status":"paused","speed":1024,"state":"waiting","total":65536}"#

// 真实抓取：`load_delivery` → `{"id":2,...}` 的 result。
// 夹具是**一条真实 manifest**（`delivery_manifest.build_manifest`）产出的清单跑出来的原文，
// 含非 ASCII 与带空格的路径（`sub dir/QC 图.png`）。
private let LOAD_DELIVERY_OK = #"{"id":2,"ok":true,"result":{"base_url":"http://127.0.0.1:8791","code":"R.pnhIQhki1xxKWIMErd","created_at":"2026-09-17T09:37:12.805751+08:00","expired":false,"expires_at":"2026-10-17T09:37:12.805751+08:00","page_url":"http://127.0.0.1:8791/R.pnhIQhki1xxKWIMErd/index.html","total_bytes":65547,"total_files":3,"tree":{"children":{"Readme.txt":{"completed":0,"crc64":"","err":"","name":"Readme.txt","path":"Readme.txt","size":6,"speed":0,"state":"pending","total":6,"type":"file"},"sub dir":{"children":{"QC 图.png":{"completed":0,"crc64":"","err":"","name":"QC 图.png","path":"sub dir/QC 图.png","size":65536,"speed":0,"state":"pending","total":65536,"type":"file"},"deep":{"children":{"read1.fq.gz":{"completed":0,"crc64":"","err":"","name":"read1.fq.gz","path":"sub dir/deep/read1.fq.gz","size":5,"speed":0,"state":"pending","total":5,"type":"file"}},"name":"deep","type":"dir"}},"name":"sub dir","type":"dir"}},"name":"","type":"dir"}}}"#

// 真实抓取：**零文件清单**的 `load_delivery`——`tree` 是空对象 `{}`（附录 A 的 ⚠️）。
private let LOAD_DELIVERY_EMPTY_TREE = #"{"id":1,"ok":true,"result":{"base_url":"http://127.0.0.1:8791","code":"R.zzzzzzzzzzzzzzzzzz","created_at":"2026-09-17T09:37:35.943293+08:00","expired":false,"expires_at":"2026-10-17T09:37:35.943293+08:00","page_url":"http://127.0.0.1:8791/R.zzzzzzzzzzzzzzzzzz/index.html","total_bytes":0,"total_files":0,"tree":{}}}"#

// 真实抓取：`list_dir` 根目录（目录项是 `{"children_count":2,"name":"sub dir","type":"dir"}`，**没有 `path` 键**）
private let LIST_DIR_ROOT = #"{"id":4,"ok":true,"result":{"entries":[{"completed":0,"crc64":"","err":"","name":"Readme.txt","path":"Readme.txt","size":6,"speed":0,"state":"pending","total":6,"type":"file"},{"children_count":2,"name":"sub dir","type":"dir"}],"path":""}}"#

// 真实抓取：`get_tree`
private let GET_TREE_OK = #"{"id":3,"ok":true,"result":{"default_selected":["Readme.txt","sub dir/QC 图.png","sub dir/deep/read1.fq.gz"],"flat":[{"name":"read1.fq.gz","path":"sub dir/deep/read1.fq.gz","size":5,"state":"pending"},{"name":"QC 图.png","path":"sub dir/QC 图.png","size":65536,"state":"pending"},{"name":"Readme.txt","path":"Readme.txt","size":6,"state":"pending"}],"progress":{"done_bytes":0,"percent":0,"speed":0,"total_bytes":65547},"tree":{}}}"#

// 真实抓取：`plan` `{"strict":true}`
private let PLAN_OK = #"{"id":6,"ok":true,"result":{"complete":[],"items":[{"crc64":"","kind":"download","path":"Readme.txt","size":6}],"strict":true,"unverifiable":["Readme.txt"]}}"#

// 真实抓取：`get_state`
private let GET_STATE_OK = #"{"id":7,"ok":true,"result":{"code":"R.pnhIQhki1xxKWIMErd","files":{},"version":1}}"#

// ---------------------------------------------------------------------------
// 信封与握手
// ---------------------------------------------------------------------------

@Test func decodesHelloResult() throws {
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(HELLO_OK.utf8))
    #expect(env.id == 1)
    #expect(env.ok == true)
    let hello = try env.unwrap(HelloResult.self)
    #expect(hello.protocolVersion == 1)          // JSON 键是 "protocol"，见 CodingKeys
    #expect(hello.minSplitSizeChoices == ["1M", "2M", "3M"])
}

@Test func decodesErrorEnvelope() throws {
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(MISMATCH.utf8))
    #expect(env.ok == false)
    #expect(env.error?.code == ErrorCode.protocolMismatch.rawValue)
    #expect(env.error?.message == "协议版本不匹配：内核是 1，请求方是 99")
    #expect(throws: CoreError.self) { try env.unwrap(HelloResult.self) }
}

@Test func decodesSettingsResult() throws {
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(SETTINGS_OK.utf8))
    let r = try env.unwrap(SettingsResult.self)
    #expect(r.lastCode == "")
    #expect(r.settings.parallel == 8)
    #expect(r.settings.connections == 16)
    #expect(r.settings.splits == 16)
    #expect(r.settings.minSplitSize == "20M")
    #expect(r.settings.limitMbps == 0)
    #expect(r.settings.maxTries == 3)
    #expect(r.settings.retryWait == 1)
}

@Test func envelopeCarriesExactlyOneOfResultOrError() throws {
    // ok:true 不得带 error；ok:false 不得带 result（内核的 skip_serializing_if 语义）
    let a = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(HELLO_OK.utf8))
    #expect(a.result != nil && a.error == nil)
    let b = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(MISMATCH.utf8))
    #expect(b.result == nil && b.error != nil)
}

// ---------------------------------------------------------------------------
// 裁定 1：`.convertFromSnakeCase` 与显式 CodingKeys 的交互——**被验证，不是被假设**
// ---------------------------------------------------------------------------

@Test func snakeCaseStrategyReachesExplicitAndImplicitKeys() throws {
    // ① `raw_status` → `rawStatus`，`error_message` → `errorMessage`
    //    （用内核真实的 `transfer_list` 条目；策略不生效时这两条直接解码失败）
    let real = try CoreJSON.decoder.decode(TransferItem.self, from: Data(TRANSFER_ITEM_ACTIVE.utf8))
    #expect(real.rawStatus == "active")
    #expect(real.errorMessage == "")
    #expect(real.state == .active)
    #expect(real.gid == "d2e08fde0dbd0be4")
    #expect(real.conns == 1)
    #expect(real.path == "Readme.txt")

    let done = try CoreJSON.decoder.decode(TransferItem.self, from: Data(TRANSFER_ITEM_COMPLETE.utf8))
    #expect(done.rawStatus == "complete")
    #expect(done.state == .complete)
    #expect(done.completed == 6)
    #expect(done.total == 6)

    // ② `min_split_size_choices` → `minSplitSizeChoices`（真实抓取的那一行，见 decodesHelloResult）

    // ③ `size_mismatch` → `sizeMismatch`，`all_good` → `allGood`
    let verifyEnv = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(VERIFY_STATUS_OK.utf8))
    let v = try verifyEnv.unwrap(VerifyStatus.self)
    #expect(v.sizeMismatch.isEmpty)
    #expect(v.unverifiable.isEmpty)
    #expect(v.allGood == true)

    // ④ `{"protocol": 1}` 配 `case protocolVersion = "protocol"`：
    //    这里比对的是**转换后的键名**（`protocol` 无下划线，转换后仍是 `protocol`）
    //    与 CodingKey 的 `stringValue`。`decodesHelloResult` 那条走的就是它。
    let hello = try CoreJSON.decoder.decode(HelloResult.self, from: Data(#"{"protocol":1,"min_split_size_choices":["1M"]}"#.utf8))
    #expect(hello.protocolVersion == 1)
    #expect(hello.minSplitSizeChoices == ["1M"])
}

@Test func snakeCaseStrategyAlsoCoversMultiwordKeysInEachResultType() throws {
    // `page_url` / `base_url` / `created_at` / `expires_at` / `total_files` / `total_bytes`
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(LOAD_DELIVERY_OK.utf8))
    let info = try env.unwrap(DeliveryInfo.self)
    #expect(info.pageUrl == "http://127.0.0.1:8791/R.pnhIQhki1xxKWIMErd/index.html")
    #expect(info.baseUrl == "http://127.0.0.1:8791")
    #expect(info.createdAt == "2026-09-17T09:37:12.805751+08:00")
    #expect(info.expiresAt == "2026-10-17T09:37:12.805751+08:00")
    #expect(info.expired == false)
    #expect(info.totalFiles == 3)
    #expect(info.totalBytes == 65547)

    // `children_count` → `childrenCount`（list_dir 的目录项，**没有 `path` 键**）
    let dirEnv = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(LIST_DIR_ROOT.utf8))
    let listing = try dirEnv.unwrap(ListDirResult.self)
    #expect(listing.path == "")
    #expect(listing.entries.count == 2)
    guard case .dir(let dirName, let count) = listing.entries[1] else {
        Issue.record("第二个条目应是目录，实际 \(listing.entries[1])")
        return
    }
    #expect(dirName == "sub dir")
    #expect(count == 2)

    // `default_selected` → `defaultSelected`，`total_bytes` → `totalBytes`，`done_bytes` → `doneBytes`
    let treeEnv = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(GET_TREE_OK.utf8))
    let tree = try treeEnv.unwrap(TreeResult.self)
    #expect(tree.defaultSelected == ["Readme.txt", "sub dir/QC 图.png", "sub dir/deep/read1.fq.gz"])
    #expect(tree.progress.totalBytes == 65547)
    #expect(tree.progress.doneBytes == 0)
    #expect(tree.flat.count == 3)

    // `download_speed` → `downloadSpeed` 等四条
    let g = try CoreJSON.decoder.decode(GlobalStat.self, from: Data(#"{"download_speed":6000,"num_active":2,"num_stopped":1,"num_waiting":0}"#.utf8))
    #expect(g.downloadSpeed == 6000)
    #expect(g.numActive == 2)
    #expect(g.numStopped == 1)
    #expect(g.numWaiting == 0)
}

// ---------------------------------------------------------------------------
// 树的形状
// ---------------------------------------------------------------------------

@Test func decodesTheWholeDeliveryTree() throws {
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(LOAD_DELIVERY_OK.utf8))
    let info = try env.unwrap(DeliveryInfo.self)

    guard case .dir(let rootName, let root) = info.tree else {
        Issue.record("根必须是 dir 节点（真实抓取是 {\"children\":…,\"name\":\"\",\"type\":\"dir\"}）")
        return
    }
    #expect(rootName == "")
    #expect(Set(root.keys) == ["Readme.txt", "sub dir"])

    guard case .dir(let subName, let sub) = root["sub dir"] else {
        Issue.record("`sub dir` 必须是 dir 节点")
        return
    }
    #expect(subName == "sub dir")
    #expect(Set(sub.keys) == ["QC 图.png", "deep"])

    guard case .file(let leaf) = sub["QC 图.png"] else {
        Issue.record("`QC 图.png` 必须是 file 节点")
        return
    }
    #expect(leaf.name == "QC 图.png")
    #expect(leaf.path == "sub dir/QC 图.png")     // 路径原样，含空格与 ×(U+00D7)
    #expect(leaf.size == 65536)
    #expect(leaf.state == .pending)
    #expect(leaf.err == "")
    #expect(leaf.crc64 == "")                     // 清单里 crc64 可以是空串

    guard case .dir(_, let deep) = sub["deep"], case .file(let f) = deep["read1.fq.gz"] else {
        Issue.record("`sub dir/deep/read1.fq.gz` 这条两级嵌套没解出来")
        return
    }
    #expect(f.path == "sub dir/deep/read1.fq.gz")
}

@Test func emptyTreeDecodesToTheEmptyCase() throws {
    // 内核在清单零文件时发 `{}`（真实抓取，见 LOAD_DELIVERY_EMPTY_TREE）——不是 dir 节点。
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(LOAD_DELIVERY_EMPTY_TREE.utf8))
    let info = try env.unwrap(DeliveryInfo.self)
    #expect(info.tree == .empty)
    #expect(info.totalFiles == 0)
}

@Test func decodesPlanAndStateResults() throws {
    let planEnv = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(PLAN_OK.utf8))
    let plan = try planEnv.unwrap(PlanResult.self)
    #expect(plan.strict == true)
    #expect(plan.items.count == 1)
    #expect(plan.items[0].kind == .download)
    #expect(plan.items[0].size == 6)
    #expect(plan.unverifiable == ["Readme.txt"])
    #expect(plan.complete.isEmpty)

    let stateEnv = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(GET_STATE_OK.utf8))
    let st = try stateEnv.unwrap(StateFile.self)
    #expect(st.version == 1)
    #expect(st.code == "R.pnhIQhki1xxKWIMErd")
    #expect(st.files.isEmpty)
}

// ---------------------------------------------------------------------------
// 请求编码
// ---------------------------------------------------------------------------

@Test func requestsEncodeToOneLineOfJSON() throws {
    let line = try RequestLine.encode(id: 1, method: "hello",
                                      params: .object(["protocol": .number(1)]))
    #expect(line == #"{"id":1,"method":"hello","params":{"protocol":1}}"#)
    #expect(!line.contains("\n"))

    // params 里的键是**调用方给的线上键名**，逐字带出去——嵌套一层也一样。
    // 这就是任务 10 调 `set_settings` 时的形状（`settings` 里面那七个键由
    // `CoreJSON.requestEncoder` 负责，见 `settingsEncodeToTheKernelWireKeys`）。
    let nested = try RequestLine.encode(
        id: 2, method: "set_settings",
        params: .object(["settings": .object(["min_split_size": .string("20M"),
                                              "max_tries": .integer(3)])]))
    #expect(nested == #"{"id":2,"method":"set_settings","params":{"settings":{"max_tries":3,"min_split_size":"20M"}}}"#)
    #expect(!nested.contains("\n"))
}

@Test func requestEncodingEscapesAndKeepsNonASCII() throws {
    // 交付码里会出现 `×`（U+00D7）这类字符，method 与 params 都必须原样带出去
    let line = try RequestLine.encode(id: 7, method: "list_dir",
                                      params: .object(["path": .string("C24-8_×_25WS024")]))
    #expect(line == #"{"id":7,"method":"list_dir","params":{"path":"C24-8_×_25WS024"}}"#)
    #expect(!line.contains("\n"))

    // 真需要转义的字符不得把这一行切碎
    let tricky = try RequestLine.encode(id: 8, method: "x", params: .string("a\"b\\c\nd"))
    #expect(!tricky.contains("\n"))
    #expect(tricky == #"{"id":8,"method":"x","params":"a\"b\\c\nd"}"#)
}

// ---------------------------------------------------------------------------
// 编码口径：`JSONValue` 走 `CoreJSON.encoder`，**类型化载荷**走 `CoreJSON.requestEncoder`
// ---------------------------------------------------------------------------

@Test func settingsEncodeToTheKernelWireKeys() throws {
    // `set_settings` 的载荷是**类型化**的 `Settings`：内核走
    // `serde_json::from_value::<Settings>`（`core/src/main.rs:1546`），而 `Settings`
    // 没有任何 `rename`（`core/src/settings.rs:32-47`），所以线上键必须是 snake_case 的
    // 那七个。Swift 侧属性名是 camelCase —— **不转换就是一份线上非法的 payload**，
    // 而它看起来完全正常（同样是七个键）。
    let s = Settings(parallel: 8, connections: 16, splits: 16, minSplitSize: "20M",
                     limitMbps: 0, maxTries: 3, retryWait: 1)
    let body = try CoreJSON.requestEncoder.encode(s)

    // ① 键名**逐字**：不是数个数——camelCase 编出来同样是 7 个键，数个数零判别力。
    let obj = try #require(try JSONSerialization.jsonObject(with: body) as? [String: Any])
    #expect(Set(obj.keys) == ["parallel", "connections", "splits",
                              "min_split_size", "limit_mbps", "max_tries", "retry_wait"])

    // ② 更强的：与真内核 `{"id":2,"method":"get_settings"}` 抓下来的 `settings` 子对象**逐字相同**
    #expect(String(decoding: body, as: UTF8.self)
            == #"{"connections":16,"limit_mbps":0,"max_tries":3,"min_split_size":"20M","parallel":8,"retry_wait":1,"splits":16}"#)

    // ③ 编解码两个口径互相对称（转出去能转回来）
    #expect(try CoreJSON.decoder.decode(Settings.self, from: body) == s)
}

@Test func theTwoEncodersAreNotInterchangeable() throws {
    // 这一条钉的是**两个编码器没有被合并**——它是"串了口径"这个改动的守卫，
    // 两个方向都能红：
    //   把 `requestEncoder` 的策略去掉 → `typed` 里不再有 `min_split_size`，红；
    //   给 `encoder` 也加上 `.convertToSnakeCase`（"统一成一个不就完了"）→ `plain`
    //   里不再有 `minSplitSize`，红。
    //
    // ⚠️ 我原本想写的是「`JSONValue` 的键在 `encoder` 下原样保留」，**那条是恒真的、已删**：
    //    `JSONValue.encode(to:)` 走 `singleValueContainer`，而 `.keyEncodingStrategy`
    //    根本到不了那条路径。实测（同一台机器、同一版工具链）：
    //        singleValue+dict, none     : {"files":{"sampleReadLength.txt":{"size":6}}}
    //        singleValue+dict, snakeCase: {"files":{"sampleReadLength.txt":{"size":6}}}
    //        keyed struct,     none     : {"limitMbps":0,"minSplitSize":"20M"}
    //        keyed struct,     snakeCase: {"limit_mbps":0,"min_split_size":"20M"}
    //    也就是说 `JSONValue` 对键策略的免疫是 `singleValueContainer` 的**副产品**，
    //    不是契约——`JSONValue.encode(to:)` 哪天改成 `container(keyedBy:)`（很自然的重构），
    //    免疫当场消失。所以两个编码器必须分开：**分开是设计意图，不是靠这条测试兜的。**
    let s = Settings(parallel: 8, connections: 16, splits: 16, minSplitSize: "20M",
                     limitMbps: 0, maxTries: 3, retryWait: 1)
    let typed = String(decoding: try CoreJSON.requestEncoder.encode(s), as: UTF8.self)
    let plain = String(decoding: try CoreJSON.encoder.encode(s), as: UTF8.self)

    #expect(typed.contains(#""min_split_size":"20M""#), "类型化载荷必须出去 snake_case：\(typed)")
    #expect(plain.contains(#""minSplitSize":"20M""#), "`CoreJSON.encoder` 不得带键策略：\(plain)")
    #expect(typed != plain)
}

// ---------------------------------------------------------------------------
// 裁定 2：整数精度
// ---------------------------------------------------------------------------

@Test func jsonValueKeepsNineteenDigitIntegersExact() throws {
    // 19 位的 CRC 值：Double 的 53 位尾数装不下它（走 Double 会变成 …633e18），
    // 所以 `integer(Int64)` 与 `number(Double)` 必须分开、且整数在解码时**先于** Double 被探测。
    // 这一行是 `get_state` 真实响应的片段（`crc64` 在协议里是字符串，这里测的是它的整数值形态）。
    let line = #"{"crc64":5432380796884633278,"mtime":1789516921.2171671,"size":65536}"#
    let v = try CoreJSON.decoder.decode(JSONValue.self, from: Data(line.utf8))
    #expect(v == .object([
        "crc64": .integer(5432380796884633278),
        "mtime": .number(1789516921.2171671),
        "size": .integer(65536),
    ]))

    let back = try CoreJSON.encoder.encode(v)
    #expect(String(decoding: back, as: UTF8.self) == line, "整数被当成 Double 编回去时这里会变成 543238079688463.3 量级的科学计数法")
}

@Test func jsonValueCoversEveryJSONShape() throws {
    let line = #"{"a":null,"b":true,"c":1,"d":1.5,"e":"s","f":[1,"x"],"g":{}}"#
    let v = try CoreJSON.decoder.decode(JSONValue.self, from: Data(line.utf8))
    #expect(v == .object([
        "a": .null,
        "b": .bool(true),
        "c": .integer(1),
        "d": .number(1.5),
        "e": .string("s"),
        "f": .array([.integer(1), .string("x")]),
        "g": .object([:]),
    ]))
}

// ---------------------------------------------------------------------------
// 枚举字面量：与内核逐字对齐
// ---------------------------------------------------------------------------

@Test func allThirteenErrorCodesAreKnown() {
    let wire = ["bad_request","invalid_params","protocol_mismatch","internal",
                "unknown_method","no_delivery","delivery_fetch_failed","preflight_failed",
                "engine_not_started","engine_start_failed","engine_disconnected",
                "engine_rpc_failed","path_not_found"]
    for w in wire { #expect(ErrorCode(rawValue: w) != nil, "缺少错误码 \(w)") }
    #expect(ErrorCode.allCases.count == 13)
}

@Test func enumLiteralsMatchTheCore() {
    #expect(TaskState.allCases.map(\.rawValue) == ["waiting","active","complete","error","removed"])
    #expect(FileState.allCases.map(\.rawValue) == ["pending","downloading","complete","failed"])
    #expect(VerifyClass.allCases.map(\.rawValue)
            == ["ok","bad","missing","size_mismatch","unverifiable","unreadable"])
    #expect(PlanKind.allCases.map(\.rawValue) == ["skip","download"])
    #expect(TaskAction.allCases.map(\.rawValue)
            == ["pause","unpause","retry","remove","clear_finished"])
}

@Test func verifyClassReadsItsOwnBucket() throws {
    let line = #"{"ok":["a"],"bad":["b"],"missing":["c"],"size_mismatch":["d"],"unverifiable":["e"],"unreadable":["f"],"all_good":false}"#
    let v = try CoreJSON.decoder.decode(VerifyStatus.self, from: Data(line.utf8))
    #expect(v.paths(.ok) == ["a"])
    #expect(v.paths(.bad) == ["b"])
    #expect(v.paths(.missing) == ["c"])
    #expect(v.paths(.sizeMismatch) == ["d"])
    #expect(v.paths(.unverifiable) == ["e"])
    #expect(v.paths(.unreadable) == ["f"])
    #expect(v.allGood == false)
}

@Test func helloChoiceListIsTheWholeContractRange() throws {
    // 真内核回的 `min_split_size_choices` 是 1M–100M 整整 100 项（协议 §2.3 的枚举面）。
    // 壳的 `-k` 控件就是照它建的，所以条数与两端必须钉住。
    // 下面这行**是真实抓取**（`hello` 响应的原文，未截断）。
    let line = #"{"id":1,"ok":true,"result":{"min_split_size_choices":["1M","2M","3M","4M","5M","6M","7M","8M","9M","10M","11M","12M","13M","14M","15M","16M","17M","18M","19M","20M","21M","22M","23M","24M","25M","26M","27M","28M","29M","30M","31M","32M","33M","34M","35M","36M","37M","38M","39M","40M","41M","42M","43M","44M","45M","46M","47M","48M","49M","50M","51M","52M","53M","54M","55M","56M","57M","58M","59M","60M","61M","62M","63M","64M","65M","66M","67M","68M","69M","70M","71M","72M","73M","74M","75M","76M","77M","78M","79M","80M","81M","82M","83M","84M","85M","86M","87M","88M","89M","90M","91M","92M","93M","94M","95M","96M","97M","98M","99M","100M"],"protocol":1}}"#
    let env = try CoreJSON.decoder.decode(RawEnvelope.self, from: Data(line.utf8))
    let r = try env.unwrap(HelloResult.self)
    #expect(r.minSplitSizeChoices.count == 100)
    #expect(r.minSplitSizeChoices.first == "1M")
    #expect(r.minSplitSizeChoices.last == "100M")
    #expect(r.protocolVersion == 1)
}

@Test func pausedAndWaitingDifferOnlyByRawStatus() throws {
    // 契约 §8 的 #14：aria2 的 `paused` 与 `waiting` 映到**同一个** `TaskState`
    //（都是 `waiting`）。设计规格 §8.4 要界面把暂停的任务标注「已暂停」——
    // 那个区分**只能**从 `raw_status` 取，壳拿不到它就渲染不出标注。
    let paused = try CoreJSON.decoder.decode(TransferItem.self, from: Data(TRANSFER_ITEM_PAUSED.utf8))
    #expect(paused.state == .waiting, "领域状态按契约就是 waiting")
    #expect(paused.rawStatus == "paused", "raw_status 必须是逐字透传，不是从 state 反推")
    #expect(paused.errorMessage == "连接超时")
    #expect(paused.speed == 1024)
    #expect(paused.gid == "g-paused")
    #expect(paused.path == "a/b.txt")
}

@Test func transferItemKeepsUnknownPathAsNil() throws {
    // 内核的 `path: Option<String>` 没有 `skip_serializing_if`，没有 GID 映射时发的是 **null**，
    // 不是省略键（`core/src/protocol.rs` 的 `TransferItem::path`）。壳必须解得出来。
    let line = #"{"completed":0,"conns":0,"error_message":"","gid":"g","path":null,"raw_status":"waiting","speed":0,"state":"waiting","total":0}"#
    let i = try CoreJSON.decoder.decode(TransferItem.self, from: Data(line.utf8))
    #expect(i.path == nil)
    #expect(i.rawStatus == "waiting")
}
