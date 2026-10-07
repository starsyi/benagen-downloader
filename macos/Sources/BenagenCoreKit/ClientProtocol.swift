import Foundation

// ---------------------------------------------------------------------------
// 壳与"驱动内核的那个东西"之间的接缝
// ---------------------------------------------------------------------------
//
// `CoreClient` 是唯一的生产实现，但它**不能**被直接塞进上层：任务 3 的 `AppModel`
// 要能在不起子进程的前提下被测试（"给一条 `list_dir` 的结果，界面该怎么显示"）。
// 所以上层只认这个协议——`AppModel` 持 `any CoreCalling`，测试塞一个按方法名吐
// 夹具值的替身，真机塞 `CoreClient`。
//
// ⚠️ **这个协议里只有"驱动"这一层**：没有任何业务判断（哪个状态算"已暂停"、
//    哪个字段该显示成什么），也没有定时器/轮询——那些在 `AppModel` 里
//    （约束 1：壳不含业务逻辑；约束 15 的轮询跳拍同样是上层的职责）。

/// 上层依赖的最小内核驱动面。
///
/// `Sendable` 是硬要求：这个对象会从界面线程之外的队列被调用。
public protocol CoreCalling: AnyObject, Sendable {
    /// 同步发一条请求。**返回非可选的 `result`**；`ok:false` 时抛 [`CoreError.rpc`]。
    ///
    /// 要强类型结果就自己解：`try client.callSync("hello", …).decoded(HelloResult.self)`。
    @discardableResult
    func callSync(_ method: String, _ params: JSONValue) throws -> JSONValue

    /// 同上，但把活甩到后台队列上——**并发度不变**（同一时刻仍至多一条在飞的请求）。
    /// 界面线程用它，免得一次慢请求把界面冻住。
    @discardableResult
    func callAsync(_ method: String, _ params: JSONValue) async throws -> JSONValue

    /// 内核回的**协议级**错误（`id == 0`：畸形 JSON / 超长行 / 非法 UTF-8）。
    /// 它们不属于任何一个请求，所以既不能当结果、也不能当异常往上抛——只能这样交出来。
    var protocolAlerts: [ErrorBody] { get }

    /// 收尾。**幂等**。
    func shutdown()

    /// 告诉这条连接：**这几个字面串写进诊断日志之前必须抹掉**（交付码、下载目录）。
    ///
    /// 🔴 **它存在的全部理由是隐私，而且这次是"按构造"那一档**（规格 §2.3 B）：
    ///    `kernel_call` 那一行失败时的 `why` 是**内核原文**，而交付码是**我们自己**
    ///    拼进内核文案里的（`core/src/delivery.rs` 那条 404 把 `…/{交付码}/manifest.json`
    ///    整条 URL 送了进来；`core/src/main.rs` 的 `preflight` 带着客户目录名）。
    ///    壳**知道**这两个串（码在它刚发出去的请求里、目录在它自己存的偏好里）
    ///    ⇒ 不必去猜任意第三方文案里像不像码。
    ///
    /// ⚠️ **它在协议上、不在具体类型上**：`AppModel` 持的是 `any CoreCalling`
    ///    （见本文件头注），只有把它放进协议，"推下去"这一步才到得了真的那条连接。
    ///    收的一份**完整清单**（不是追加）：换批次要换码、改设置要换目录。
    func setRedactions(_ secrets: [String])
}

extension JSONValue {
    /// 把一条 `result` 解成强类型。
    ///
    /// 与 [`RawEnvelope.unwrap`] 的差别只有一处：这里解的是**已经取出来的** `result`
    /// （`callSync` 的返回值），而 `unwrap` 解的是整个信封。
    /// 两条路都走 [`CoreJSON`]，所以键策略不会在两处分叉。
    public func decoded<T: Decodable>(_ type: T.Type) throws -> T {
        do {
            let data = try CoreJSON.encoder.encode(self)
            return try CoreJSON.decoder.decode(T.self, from: data)
        } catch {
            throw CoreError.malformedResponse("result 与 \(T.self) 的形状不符: \(error)")
        }
    }
}
