//! aria2 的原始状态 → 客户端领域模型的映射。**全是纯函数。**

use serde::{Deserialize, Serialize};

/// 客户端视角的任务状态。
///
/// `rename_all = "lowercase"` 与 Go 侧的字面量一致（`TaskActive = "active"`）——
/// 这些字符串会进状态文件，是与 Go 版共用的持久化格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Waiting,  // 还没开始传
    Active,   // 正在传
    Complete, // 传完了
    Error,    // 失败
    Removed,  // 被移除
}

/// 一个下载任务的领域模型。
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub gid: String,
    pub total: i64,
    pub completed: i64,
    pub speed: i64,
    pub conns: i32,
    pub state: TaskState,
    pub err: String,
}

/// aria2 的原始任务条目。
///
/// 字段全部宽容（缺字段即空串），因为这是**网络来的**结构：
/// 一次响应里少一个键不应该让整次轮询失败。
///
/// ⚠️ **`gid` 也在"全部"里面**（它曾是七个字段里唯一没有 `#[serde(default)]` 的那个）：
/// 少了它，`from_value` 会报 `missing field gid`，`RpcClient::list()` **整趟失败**
/// （`list()` 是 `active ++ waiting ++ stopped` 三段拼接，一段坏掉整趟就没了），
/// 界面于是把一次"字段缺失"升级成一次「下载引擎调用失败」的重判定——**代价不对称**。
/// 宽容成空串之后，这一条在消费者那里自然被丢掉（`path_map` 里没有 `""` 这个键，
/// `view::compose`/`progress` 都按路径匹配），**不会**影响其余任务的显示。
/// 钉住它的是 `raw_task_tolerates_missing_gid`。
///
/// ⚠️ **`rename_all = "camelCase"` 是承重的，不是装饰**：aria2 发的键名是
/// `totalLength` / `completedLength` / `downloadSpeed` / `errorMessage`
/// （Go 的 `RawTask` 有显式 `json:"totalLength"` 等七个 tag，逐字对应）。
/// 少了它，四个**多词**键全部落空、被上面的 `#[serde(default)]` 静默吃成空串，
/// 再经 `to_task()` 变成 `0`/`""`——**不报错**，只是进度恒为 0、失败原因恒为空
/// （`gid`/`status`/`connections` 恰好同名，所以看着"大部分是对的"）。
/// `raw_task_reads_aria2_wire_form` 是钉住它的那一条（任务 9 补，不计入 4 条移植）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTask {
    // 七个字段**无一例外**都要宽容，`gid` 也不例外——理由见结构体的文档注释。
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub total_length: String,
    #[serde(default)]
    pub completed_length: String,
    #[serde(default)]
    pub download_speed: String,
    #[serde(default)]
    pub connections: String,
    #[serde(default)]
    pub error_message: String,
}

impl RawTask {
    /// 把线上形态转成领域模型。字段缺失或**完全**无法解析时退化为 0，不 panic。
    pub fn to_task(&self) -> Task {
        Task {
            gid: self.gid.clone(),
            total: parse_num(&self.total_length),
            completed: parse_num(&self.completed_length),
            speed: parse_num(&self.download_speed),
            // Go 的 `Conns` 是 64 位 `int`，这里按契约收窄成 `i32`；
            // aria2 的 connections 是两位数，`as` 截断只影响现实中不存在的巨值。
            conns: parse_num(&self.connections) as i32,
            state: raw_status_to_state(&self.status),
            err: self.error_message.clone(),
        }
    }
}

/// 把 aria2 传数字的字符串解析成 `i64`。
///
/// 对应 Go 的 `atoi64`（`fmt.Sscanf(s, "%d", &n)`），语义逐条对齐：
///   - 前导空白跳过；随后可选正负号
///   - 取**连续十进制数字前缀**，遇到第一个非数字字符即停（`"12abc"` → 12）
///   - 一个数字都没有（`""`、`"不是数字"`、`"-"`）→ 0
///   - 溢出 `i64` → 0（Go 的 Sscanf 在溢出时报错，`n` 保持 0）
///
/// ⚠️ 退化为 0 的条件是**完全**无法解析，不是"半合法值也退化"：`"12abc"` 会得到 12。
/// 别把这条读成"非数字一律为 0"——aria2 不会发这种半合法值，
/// 但注释若承诺了它做不到的事，下一个改代码的人就会照着错的前提做判断。
pub fn parse_num(s: &str) -> i64 {
    // `trim_start` 对应 Go `fmt` 扫描前的 `SkipSpace()`：`" 12"` 两边都是 12。
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return 0; // 一个数字都没有：`""`、`"不是数字"`、`"-"`
    }
    // 溢出时 `parse` 报错 → 0，与 Go 的 `Sscanf` 溢出行为一致。
    // `i` 落在 ASCII 字节上，必是字符边界。
    t[..i].parse::<i64>().unwrap_or(0)
}

/// 把 aria2 的状态字面量映射成枚举。
///
/// 认不出的值（含空串）一律按"还没开始传"处理——这是**安全方向**：
/// 把它当成"完成"会让客户以为下好了，当成"失败"会误报。
///
/// `paused` 与 `waiting` 同义：aria2 的 paused 对我们等价于"还没在传"。
/// **这个别名在本内核里事实上不可测**（`default` 分支恰好也返回 `Waiting`，
/// 契约 §8 表的 `#14` 记录过这条）——**保留它是为了显式表达意图**：
/// 将来若按 aria2 的语义给 `default` 换上别的值（例如新增 `unknown` 变体），
/// 这行 `paused` 仍然声明"暂停就是没在传"，而不会被顺手改掉。
pub fn raw_status_to_state(s: &str) -> TaskState {
    match s {
        "active" => TaskState::Active,
        // `paused` 是显式声明的别名（见上文：事实上不可测，保留以表达意图）。
        "waiting" | "paused" => TaskState::Waiting,
        "complete" => TaskState::Complete,
        "error" => TaskState::Error,
        "removed" => TaskState::Removed,
        _ => TaskState::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对应 Go 的结构体字面量只填部分字段：其余字段取零值（空串）。
    fn raw(gid: &str, status: &str) -> RawTask {
        RawTask {
            gid: gid.to_string(),
            status: status.to_string(),
            total_length: String::new(),
            completed_length: String::new(),
            download_speed: String::new(),
            connections: String::new(),
            error_message: String::new(),
        }
    }

    /// 对应 Go `TestRawTaskToTaskParsesStringNumbers`。
    #[test]
    fn raw_task_to_task_parses_string_numbers() {
        // aria2 把数字都当字符串传
        let r = RawTask {
            gid: "abc".to_string(),
            status: "active".to_string(),
            total_length: "65536".to_string(),
            completed_length: "32768".to_string(),
            download_speed: "1024".to_string(),
            connections: "4".to_string(),
            error_message: String::new(),
        };
        let tk = r.to_task();
        assert!(
            tk.total == 65536 && tk.completed == 32768 && tk.speed == 1024,
            "字符串→整数解析错误: {tk:?}"
        );
        // gid 与 conns 也要断言：只看前三个数会让"漏搬 conns / 漏搬 gid"的变异体活下来
        assert!(
            tk.conns == 4 && tk.gid == "abc",
            "conns/gid 搬运错误: {tk:?}"
        );
        assert_eq!(tk.state, TaskState::Active, "状态映射错误: {:?}", tk.state);
    }

    /// 对应 Go `TestStatusLiteralMapping`。
    #[test]
    fn status_literal_mapping() {
        let cases: [(&str, TaskState); 8] = [
            ("active", TaskState::Active),
            ("waiting", TaskState::Waiting),
            ("paused", TaskState::Waiting), // aria2 的 paused 对我们等价于"还没在传"
            ("complete", TaskState::Complete),
            ("error", TaskState::Error),
            ("removed", TaskState::Removed),
            ("", TaskState::Waiting),        // 空串按未开始处理，不 panic
            ("未知值", TaskState::Waiting),
        ];
        for (input, want) in cases {
            assert_eq!(
                raw_status_to_state(input),
                want,
                "raw_status_to_state({input:?})"
            );
        }
    }

    /// 对应 Go `TestToTaskCarriesErrorMessage`。
    #[test]
    fn to_task_carries_error_message() {
        let r = RawTask {
            error_message: "连接超时".to_string(),
            ..raw("x", "error")
        };
        assert_eq!(r.to_task().err, "连接超时", "失败原因应带出来");
    }

    /// **不计入 4 条移植**（全局约束 6 的例外，显式记账）：钉住 `RawTask` 的**线上键名**。
    ///
    /// 为什么必须有这一条：上面四条测试**全部直接构造结构体**，于是
    /// 「从 aria2 的 JSON 读进来」这条路一次都没被走过——而 `RawTask` 曾经**没有**
    /// `camelCase` 重命名，四个多词键（`totalLength`/`completedLength`/`downloadSpeed`/
    /// `errorMessage`）全部落空、被 `#[serde(default)]` 静默吃成空串，再经 `to_task()` 变成
    /// `0`/`""`。`gid`/`status`/`connections` 恰好同名，所以只有它们是对的，掩盖了缺陷。
    /// 任务 9 落地 `rpc::list()`（第一个真正的反序列化消费者）时才暴露。
    ///
    /// **判别力**：去掉 `#[serde(rename_all = "camelCase")]` → 本测试必红
    /// （四个数值与 `errorMessage` 全部对不上）。断言用的是**字段原文**而不是只看
    /// `to_task()` 的整数结果：只断言整数值会让「`errorMessage` 漏搬」的变异体活下来。
    #[test]
    fn raw_task_reads_aria2_wire_form() {
        // 形状照 aria2 的真实响应写（`tellActive` 的条目）：额外带上 aria2 一定会发的
        // `dir`/`files`/`errorCode` 等键，顺带确认「多出来的键不会让反序列化失败」。
        let wire = r#"{
            "gid": "2089b05ecca3d829",
            "status": "active",
            "totalLength": "65536",
            "completedLength": "32768",
            "downloadSpeed": "1024",
            "connections": "4",
            "errorMessage": "连接超时",
            "dir": "/Users/you/Downloads",
            "files": [{"path": "/Users/you/Downloads/a.txt", "length": "65536"}],
            "errorCode": "1"
        }"#;
        let r: RawTask =
            serde_json::from_str(wire).expect("aria2 的真实形态必须能读进来（键名对不上？）");
        // 七个个字段逐个断（与 Go 的七个 json tag 一一对应）
        assert_eq!(r.gid, "2089b05ecca3d829", "gid");
        assert_eq!(r.status, "active", "status");
        assert_eq!(r.total_length, "65536", "totalLength 没读出来（缺 camelCase 重命名？）");
        assert_eq!(r.completed_length, "32768", "completedLength 没读出来");
        assert_eq!(r.download_speed, "1024", "downloadSpeed 没读出来");
        assert_eq!(r.connections, "4", "connections");
        assert_eq!(r.error_message, "连接超时", "errorMessage 没读出来");
        // 再走一遍 to_task()：线上形态读对了还不够，领域模型也必须拿到值
        let t = r.to_task();
        assert!(
            t.total == 65536 && t.completed == 32768 && t.speed == 1024 && t.conns == 4,
            "to_task 之后的数值错了: {t:?}"
        );
        assert_eq!(t.err, "连接超时", "失败原因必须带出来");
        assert_eq!(t.state, TaskState::Active);
    }

    /// **不计入移植条数**（全局约束 6 的例外，显式记账）：
    /// 钉住「`RawTask` 的字段**全部**宽容」这句话——**包括 `gid`**。
    ///
    /// 为什么必须有这一条：`RawTask` 的文档注释写着"字段全部宽容（缺字段即空串），
    /// 因为这是**网络来的**结构：一次响应里少一个键不应该让整次轮询失败"，
    /// 而 `gid` 曾经是**七个字段里唯一没有 `#[serde(default)]` 的**——
    /// 少一个键会让 `from_value` 报 `missing field gid`，`RpcClient::list()`
    /// **整趟失败**（`list()` 是 `active ++ waiting ++ stopped` 三段拼接，
    /// 一段坏掉整趟就没了），于是界面把一次"字段缺失"升级成一次
    /// 「下载引擎调用失败 / 已断开」的重判定。**用一条残缺的条目换一次引擎级重判定，
    /// 代价不对称。** 注释说一套、代码做另一套本身就是缺陷，这里让代码符合注释。
    ///
    /// 判别力：去掉 `gid` 上的 `#[serde(default)]` → 本测试在读那一条 JSON 时就红。
    #[test]
    fn raw_task_tolerates_missing_gid() {
        // 一条**缺 `gid`** 的条目：宽容成空串，而不是让整趟 list 失败
        let wire = r#"{"status":"active","totalLength":"1024","completedLength":"512"}"#;
        let r: RawTask = serde_json::from_str(wire)
            .expect("缺 gid 不该让反序列化失败（文档注释承诺的是「字段全部宽容」）");
        assert_eq!(r.gid, "", "缺字段应宽容成空串");
        assert_eq!(r.status, "active", "同一条里其余字段照常读出");
        assert_eq!(r.to_task().gid, "", "to_task 同样不该 panic");

        // 一个**完全没有 gid** 的三段拼接：整趟 list 不得失败。
        // 这里直接钉 `Vec<RawTask>` 的反序列化形状——`RpcClient::list()` 对每段做的
        // 就是这件事（`serde_json::from_value::<Vec<RawTask>>`）。
        let several: Vec<RawTask> = serde_json::from_str(
            r#"[{"status":"active"},{"gid":"g2","status":"waiting"}]"#,
        )
        .expect("整段里有一条缺 gid，不得让整段失败");
        assert_eq!(several.len(), 2, "两条都要留下: {several:?}");
        assert_eq!(several[1].gid, "g2", "正常那一条不受影响");
    }

    /// 对应 Go `TestToTaskToleratesBadNumbers`。
    #[test]
    fn to_task_tolerates_bad_numbers() {
        // 缺字段或非数字不得 panic
        let r = RawTask {
            total_length: String::new(),
            completed_length: "不是数字".to_string(),
            ..raw("x", "active")
        };
        let tk = r.to_task();
        assert!(
            tk.total == 0 && tk.completed == 0,
            "非法数字应退化为 0，实际: {tk:?}"
        );
    }
}
