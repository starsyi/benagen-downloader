//! 响应信封 —— **整条链路上唯一**拼装它两个键的地方（约束 3 / 规格 §3.2）。
//!
//! ```json
//! {"ok": true,  "data":  { … }}                  // 成功：`data` 键**恒在**
//! {"ok": false, "error": { "message": "…" }}     // 失败：`error.message` **恒在**
//! ```
//!
//! ## ⚠️ 两条硬口径
//!
//!   * **成功恒带 `data` 键、失败恒带 `error.message`**，且**两个键不共存**
//!     （成功不带 `error`、失败不带 `data`）。前端只有**一条**解包路径
//!     （`windows/web/js/invoke.js` 逐字："把 `{ok,data|error}` 信封拆开"）——
//!     形状多一种，前端就多一条只有真机上才会走到的分支。
//!   * **`message` 原文逐字、不加工、不截断**（约束 3）：内核怎么说就怎么写。
//!     折行、加"出错啦："前缀、超长截断**全都是呈现层的事** ——
//!     在这里洗一遍，"壳没有改内核的话"这条契约就没了，而且**不会有东西变红**。
//!
//! ## ⚠️ 为什么不写成 `data: Option<…>` 那种一个函数两种形状
//!
//! 那会让"成功到底带不带 `data`"变成一个运行期的偶然，而它是一条**静态契约**：
//! 两个函数各自把结论写死，调用方在编译期就只能二选一。
//!
//! ## ⚠️ 对齐 macOS
//!
//! macOS 侧没有这个名字的文件：那一代的"信封"是 SwiftUI 直接读 `AppModel` 的字段
//! （没有一条给前端消费的 JSON 边界）。所以本文件**不是移植**，它是第三代
//! 由 Tauri `invoke` 带来的新边界 —— 对齐的是**上游同一条纪律**（约束 3：
//! 原文逐字）在 web 前端的同形落地（规格 §3.2）。
use serde_json::Value;

/// 成功：`{"ok": true, "data": <data>}`。
///
/// ⚠️ 收 `impl Serialize` 而不是 `Value`：调用方直接把手上的界面值递进来就行，
///    不必先自己转一次 —— 少一次转换就少一次"顺手改成 `to_string()`"的机会。
pub fn ok(data: impl serde::Serialize) -> Value {
    serde_json::json!({ "ok": true, "data": data })
}

/// 失败：`{"ok": false, "error": {"message": <message>}}`。
///
/// ⚠️ **`message` 原样进 `json!`**：不 `trim`、不按行拆、不截断（约束 3）。
///    这条注释不是装饰 —— 它就是这个函数存在的理由，改这一行之前先读上面那段。
pub fn err(message: &str) -> Value {
    serde_json::json!({ "ok": false, "error": { "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_always_carries_a_data_key() {
        let v = ok(serde_json::json!({"a": 1}));
        assert_eq!(v["ok"], serde_json::json!(true));
        assert!(v.get("data").is_some(), "data 键恒在");
        assert!(v.get("error").is_none(), "成功不带 error");
    }

    #[test]
    fn err_carries_the_message_verbatim() {
        let raw = "内核无响应（等待超过 5 秒）\n第二行";
        let v = err(raw);
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["error"]["message"], serde_json::json!(raw),
                   "原文逐字、不加工、不截断（约束 3）");
        assert!(v.get("data").is_none());
    }

    /// ⚠️ **`data` 是 `null` 与"没有 `data` 键"是两件事**（成功的契约是"键恒在"）。
    ///
    /// 判别力：把 `ok` 写成 `if data.is_null() { … }` 之类的"顺手省一格"，
    /// 这一条立刻红 —— 而前端的 `v.data` 与 `'data' in v` 在那之后就会分叉。
    #[test]
    fn an_ok_with_a_null_payload_still_has_the_key() {
        let v = ok(serde_json::Value::Null);
        assert!(v.get("data").is_some(), "载荷是 null 也要带上 data 键：{v}");
        assert_eq!(v["data"], serde_json::Value::Null);
    }

    /// 信封**不多不少**就这两个键（成功那两格）。
    ///
    /// ⚠️ 判别力：往成功里塞第三个键（`"status"` / `"code"` 这类"顺手给前端用的"
    /// 东西）会被这一条逮住 —— 那些格子是**内核协议**的形状，不是界面值
    /// （约束 1：前端不知道内核协议的形状）。
    #[test]
    fn the_success_envelope_has_exactly_two_keys() {
        let v = ok(serde_json::json!({"a": 1}));
        let keys: Vec<&String> = v.as_object().expect("信封是个对象").keys().collect();
        assert_eq!(keys, vec!["data", "ok"], "成功信封的键只有 data 与 ok：{v}");
    }
}
