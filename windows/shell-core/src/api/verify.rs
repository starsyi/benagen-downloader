//! `/api/verify` 那一格的载荷（搬自 `shell-win/src/server/routes.rs:420` 的 `verify`）。
//!
//! ⚠️ 上游那个函数一进门就问 `session.client()`、调 `kernel::verify_status`、
//!    失败时走 `note_failure` —— 那些是**命令层**的事（它才拿得到 `Session` 与内核连接）。
//!    这里只回答"给定这份界面值，前端该收到什么"。

use serde_json::Value;

use crate::api::{envelope, to_wire};
use crate::presentation::verify_summary::{SidebarBadge, VerifySummary};

/// 「内核回了一份 `status`、而壳算不出一份摘要」那句话。**壳自己写的**
/// （这一档没有内核原文可登 —— 内核说的话我们收到了，是壳自己把它算没了）。
///
/// ## ⚠️ 先说清楚：**这条分支今天是不可达的**
///
/// `VerifySummary::of(Some(_))` **恒 `Some`**（它唯一的 `None` 出口是 `let status = status?;`
/// 那一行，也就是"实参本身是 `None`"）。而命令层调它时手上**恒有一条**内核刚回的
/// `status`（`commands.rs` 的 `verify()` 在 `Ok(status)` 那一支里才走到这里）⇒
/// 按今天的签名，这句话**一个字都不会出现在屏幕上**。
///
/// ## ⚠️ 那它存在的意义是什么：**纵深防御，不是历史残留**
///
/// 它挡的是一条**会悄悄发生的**改法：`VerifySummary::of` 哪天多出一个 `None` 出口
/// （比如"这份 status 里一个文件都没有 ⇒ 不算一份结果"—— 那是这个类型完全可能收到的
/// 判断），于是命令层手上就有了一个**真实的** `None`。那一刻只剩三条路：
///
///   1. **`panic`/`expect`** —— ❌ **明禁**。这条命令跑在 Tauri 的 `invoke` 线程上，
///      panic 之后**回执永远不会发出去** ⇒ 前端那条 `invoke` 永远不 resolve，
///      界面停在"正在校验…"上，**一个字都不说**。那是本项目最恨的形态
///      （沉默的挂住），比报一句不准确的话坏得多。
///   2. **自己编一份"六类全 0 + 全部通过"** —— ❌ **明禁**（判据在
///      [`VerifySummary::of`] 的文档里：那是**替内核宣布一句它没说过的话**）。
///      客户会看到"全部通过"，而壳其实什么都没算出来。
///   3. **大声说一句人话** —— ✅ 就是这一条。
///
/// ⇒ 留这句话的代价是**一行常量**；删掉它的代价是上面第 1/2 两种改法**都不会有东西变红**。
///
/// ## ⚠️ 它为什么住在 `shell-core::api` 而不是命令层
///
/// 因为它是一句**面向用户的失败文案**，而规格 §3.2 那条承重墙（"JS 不拼接任何面向用户的
/// 字符串"）在 Rust 侧的同一形态是：**命令层不许自造文案**。它是与 [`super::NO_KERNEL`]
/// 同一条纪律的第二个落点 —— 两边都必须留下同一个痕：
/// **"就这一句"正是墙塌的方式**（命令层有一句之后，第二句就没有任何东西拦得住了）。
/// ⇒ 命令层那一支现在只写 `envelope::err(api::verify::NO_VERIFY_SUMMARY)`，
/// **一个汉字都不自己写**。
///
/// ⚠️ 措辞的两半与 [`super::NO_KERNEL`] 同形：**根因**（壳算不出摘要）+
/// **可执行的下一步**（把这条原样发回来，W-2）。
/// 它刻意**不提**任何内核协议的名词（那属于诊断、不属于给客户的话）。
pub const NO_VERIFY_SUMMARY: &str =
    "内核回的校验结果里一个文件都没有，壳算不出一份摘要。补救：把这条原样发给我们。";

/// 校验状态：`data` 就是那份 `VerifySummary`（**不是**包一层 `{"summary": …}`）。
///
/// ⚠️ **搬自** `shell-win/src/server/routes.rs:420` 的 `verify`。
/// **对齐 macOS**：`Presentation/VerifySummary.swift`（`VerifySummary`，
/// **已移植**到 `presentation/verify_summary.rs`）。
///
/// ⚠️ **为什么收 `&VerifySummary` 而不是 `Option`**：上游的调用点手上**恒有一条**
///    内核刚回的 `status`（`VerifySummary::of(Some(&status))` 恒 `Some`）。
///    "还没取过"（`None`）这一档在**命令层**就分掉了 —— 它根本不会调到本函数
///    （那时该发的是 `err`：这一次动作整个没成）。
///    ⚠️ 但那**不等于** `VerifySummary::of` 的 `None` 可以折成"六类全 0 + 全部通过"：
///    那条判据在 `VerifySummary::of` 里，别在这里重写一份
///    （抄第二份就会出现"同一个状态在两处措辞不同"）。
///
/// ## 🔴 那五格是**补上的**（它们曾经真的缺着）
///
/// `VerifySummary` 的 `headline()` / `headline_color()` / `headline_icon()` /
/// `classified_text()` 与 `VerifyClassRow::count_text()` 是**方法**，不是字段 ——
/// 派生的 `Serialize` 里**没有它们**。于是 `to_wire(summary)` 发出去的只有
/// `rows`（六个键）/ `all_good` / `failed_count` / `classified_count` / `verdict`，
/// 而**校验结果屏要显示的正是那五句**：
///
/// | 界面上那一格 | 该由谁给 | 缺了会怎样 |
/// |---|---|---|
/// | 总结那一行（`尚未校验` / `全部通过` / `N 项未通过` / 内核自相矛盾那一档） | [`VerifySummary::headline`] | 空着 |
/// | 总结的语义色 | [`VerifySummary::headline_color`] | 标记没有颜色 |
/// | 总结的标记 | [`VerifySummary::headline_icon`] | 没有图形 |
/// | 「已校验 N 项」 | [`VerifySummary::classified_text`] | 空着 |
/// | 每类的「N 项」（**包括「0 项」**） | [`VerifyClassRow::count_text`] | 六类只剩标签，**看不出一类是 0 还是缺失** |
///
/// 它们**不会让任何东西变红**（少一个键 ≠ 报错），所以缺口一直没被发现 ——
/// 这正是规格 §3.2 要说的事：**漏发一格呈现值的后果不是"前端少显示一点"，
/// 而是前端只能自己造一个（明禁），或者什么都不显示**。
///
/// ## ⚠️ 五格**一个字都不自己写**
///
/// 它们**一律取自** `presentation::verify_summary`（20 条单测逐字对位 macOS 的
/// `Presentation/VerifySummary.swift`）—— 本函数只负责**搬运**。
/// 这与 [`super::state::state`] 里 `engine_wire` 补那三格（`text` / `icon` / `tooltip`）
/// 是**同一个形态**：`api/mod.rs` 文件头那条"不手写 JSON 投影"禁的是**第二份形状知识**，
/// 而这里没有第二份 —— 形状仍然全部由 `to_wire(summary)` 派生出来，本函数只在它上面
/// **加**五格**算好的呈现值**。
/// （`api/mod.rs` 那张"三处具名例外"表**不需要**改：这不是第四处例外，同 `engine_wire`。）
///
/// ⚠️ `verdict` 那一格**照样发**（派生的那一半）：它是 `Serialize` 的枚举
/// （`"AllGood"` / `{"Failed":3}` 这样的**变体名**），前端**不拿它显示** ——
/// 给客户看的是 `headline` 那一格。留着它是"`presentation` 算好的界面值原样透传"
/// 这条纪律的结果，不是为了让人读它。
///
/// ⚠️ **`count_text` 必须一格一格地补**：它在**每一个 row** 上（方法挂在行上），
/// 而行的结构由 `to_wire` 派生 ⇒ 这里按下标把它与 `summary.rows` 对齐插进去
/// （两个数组同源、同序、同长：`to_wire` 就是 `summary.rows` 的序列化）。
pub fn verify(summary: &VerifySummary) -> Value {
    let mut data = to_wire(summary);

    // ---- 六类每一行：补一格 `count_text` ---------------------------------
    // ⚠️ 0 也要有它的那一句（「0 项」）：约束 4（六类互斥穷尽、计数为 0 的类**显示 0、
    //    不得隐藏**）在这一层的落点就是这个键**恒在** —— 有它，"这一类是 0 项"与
    //    "这一类没画出来"才是两件说得清的事。
    if let Some(rows) = data.get_mut("rows").and_then(Value::as_array_mut) {
        for (wire_row, row) in rows.iter_mut().zip(summary.rows.iter()) {
            if let Some(map) = wire_row.as_object_mut() {
                map.insert("count_text".to_string(), Value::String(row.count_text()));
            }
        }
    }

    // ---- 顶部那一行总结：补五格 ------------------------------------------
    // 形状与 `api/state.rs:engine_wire` 补那三格逐字同形（五格全是**搬运**）。
    if let Some(map) = data.as_object_mut() {
        // 🔴 **侧栏那颗徽标的计数**（第五格，补上的）：与上面那一行总结**共用同一个数**
        //    （`SidebarBadge::unpassed_of_summary` 的正文逐字就是 `summary.failed_count`）
        //    —— 分叉的症状是"侧栏挂着 2、校验屏顶上写着 1"。
        //    ⚠️ 它为什么必须由壳算：徽标要显示的是"还有几项没通过"，而"哪些算未通过"
        //    是 `VerifyClass::is_failure` 那一条判据（`unverifiable` **不算**）——
        //    交给 JS 数就等于把判据抄进前端（规格 §3.2）。
        //    ⚠️ **对齐 macOS**：那边的徽标读 `AppModel.verify`（`Sidebar.swift:84`），
        //    而那一份快照只在**校验分区可见时**被刷新（`VerifyView`）⇒ 徽标反映的
        //    本来就是"最后一次看到它时的样子"。这一格随这一屏的载荷下来，不额外起轮询。
        map.insert(
            "badge_count".to_string(),
            serde_json::json!(SidebarBadge::unpassed_of_summary(summary)),
        );
        map.insert("headline".to_string(), Value::String(summary.headline()));
        map.insert(
            "headline_icon".to_string(),
            Value::String(summary.headline_icon().to_string()),
        );
        map.insert(
            "classified_text".to_string(),
            Value::String(summary.classified_text()),
        );
        map.insert(
            "headline_color".to_string(),
            to_wire(&summary.headline_color()),
        );
    }

    envelope::ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::verify_summary::{VerifySummary, ALL_CLASSES};
    use crate::protocol::VerifyStatus;

    /// 一份**有一类没过**的回执（不是全绿 —— 全绿那一条会让"六行都在"变成真空断言）。
    fn a_status() -> VerifyStatus {
        VerifyStatus {
            ok: vec!["a.fq.gz".to_string()],
            bad: vec!["b.fq.gz".to_string()],
            missing: vec![],
            size_mismatch: vec![],
            unverifiable: vec![],
            unreadable: vec![],
            all_good: false,
        }
    }

    /// ⚠️ **`NO_VERIFY_SUMMARY` 要同时说清根因与下一步**（W-2），而且走的是**失败**那条信封。
    ///
    /// 与 `api/mod.rs` 里那条 `the_no_kernel_message_says_the_cause_and_the_next_step`
    /// 同形 —— 两句话是同一条纪律（命令层不许自造文案）的两个落点，网的形状也该一样。
    ///
    /// 判别力（每一句都会红）：把"壳算不出一份摘要"那半删掉（只剩一句笼统的"出错了"），
    /// 第一条断言红；把补救那半删掉，第二条红；把 `envelope::err` 换成 `ok`，
    /// 第三条红（前端会把它当数据渲染）。
    ///
    /// ⚠️ 这条用例**不**、也**不能**断言"这句话在某条路径上真的被发出去过" ——
    /// 那条路径按今天的签名不可达（理由见常量的文档）。它钉的是**这句话长什么样**：
    /// 那一支真被走到时（`VerifySummary::of` 多了个 `None` 出口），用户看到的是人话，
    /// 而不是一个 panic 或者一句假话。
    #[test]
    fn the_uncomputable_summary_message_says_the_cause_and_the_next_step() {
        assert!(
            NO_VERIFY_SUMMARY.contains("壳") && NO_VERIFY_SUMMARY.contains("摘要"),
            "这句话没说清根因（是壳自己没算出摘要，不是内核报错）：{NO_VERIFY_SUMMARY}"
        );
        assert!(
            NO_VERIFY_SUMMARY.contains("补救"),
            "这句话没给出可执行的下一步：{NO_VERIFY_SUMMARY}"
        );
        let v = envelope::err(NO_VERIFY_SUMMARY);
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(
            v["error"]["message"],
            serde_json::json!(NO_VERIFY_SUMMARY),
            "原文逐字、不加工"
        );
    }

    /// `data` **就是**那份摘要（不包一层 `summary` 键：形状多一层，前端多一条读法）。
    #[test]
    fn the_summary_is_the_data_itself() {
        let summary = VerifySummary::of(Some(&a_status())).expect("有回执就恒有摘要");
        let v = verify(&summary);
        assert_eq!(v["ok"], serde_json::json!(true));
        assert!(v["data"].get("rows").is_some(), "data 该是那份摘要：{v}");
        assert!(v["data"].get("summary").is_none(), "不该再包一层：{v}");
    }

    /// 六类那一列**逐格都在**（空的那几类也在，约束 4）—— 少一格 ⇒ 界面上少一行，
    /// 而不会有东西变红。
    ///
    /// ⚠️ 期望的顺序取自 `ALL_CLASSES`（那里有编译期自检钉着"无重复、无遗漏"），
    ///    不手抄一遍字面量：抄了就是同一条判据的第二个实现。
    #[test]
    fn every_class_row_reaches_the_client() {
        let summary = VerifySummary::of(Some(&a_status())).expect("有回执就恒有摘要");
        let v = verify(&summary);
        let rows = v["data"]["rows"].as_array().expect("rows 是个数组");
        assert_eq!(rows.len(), ALL_CLASSES.len(), "校验六类、一条不少：{v}");
        for (row, class) in rows.iter().zip(ALL_CLASSES) {
            // ⚠️ 线上那一格叫 `kind`（`VerifyClassRow` 的字段名），**不是** `id`：
            //    `id()` 是 Rust 侧的稳定身份方法，它没有自己的字段。
            assert_eq!(row["kind"], serde_json::json!(class.wire_key()), "{v}");
            assert_eq!(row["id"], serde_json::Value::Null, "行上没有 id 这一格：{v}");
        }
        // 内核的 `all_good` 原样透传（壳不二次推导，约束 1/3）。
        assert_eq!(v["data"]["all_good"], serde_json::json!(false));
        // 未通过项数由明细算出来（这里 `bad` 有一项）。
        assert_eq!(v["data"]["failed_count"], serde_json::json!(1));
    }

    /// 🔴 **五格呈现值必须过线**（它们**曾经真的缺着**，见 [`verify`] 的文档）。
    ///
    /// 那一屏要显示的正是这五句：总结那一行（正文 / 颜色 / 标记）、「已校验 N 项」、
    /// 以及每一类的「N 项」。少任何一格，前端**不会报错** —— 它只会把那一格空着，
    /// 或者（明禁地）自己造一句。所以这里逐格钉住。
    ///
    /// 判别力（每一格都会红）：把这五格里的任何一格从 [`verify`] 里删掉，
    /// 对应的那条断言立刻红 —— 而**没有别的东西会红**。
    /// 形状与 `api/state.rs:engine_wire` 补那三格是同一条判据。
    #[test]
    fn the_presentation_cells_reach_the_client() {
        let summary = VerifySummary::of(Some(&a_status())).expect("有回执就恒有摘要");
        let v = verify(&summary);

        // ---- ① 顶部那一行总结：正文 / 标记 / 颜色 / 「已校验 N 项」-------------
        // ⚠️ 期望值写的是**字面量**（不是拿 `summary.headline()` 当期望值 ——
        //    那就成了"同一个方法自己证明自己"）：字面量才钉得住"发的是哪一个方法"。
        //    `a_status()` 是 ok 一条 + bad 一条 ⇒ 1 项未通过 / 已校验 2 项。
        assert_eq!(v["data"]["headline"], serde_json::json!("1 项未通过"), "{v}");
        assert_eq!(
            v["data"]["headline_icon"],
            serde_json::json!("exclamationmark.triangle.fill"),
            "{v}"
        );
        assert_eq!(v["data"]["classified_text"], serde_json::json!("已校验 2 项"), "{v}");
        // ---- ⑤ 侧栏徽标的计数（第五格，补上的）--------------------------------
        // ⚠️ **它必须与 `failed_count` 同值、且与顶上那一行里的 N 同值** ——
        //    分叉的症状是"侧栏挂着 1、校验屏顶上写着 2"。
        //    判别力：把这一格从 `verify()` 里删掉 ⇒ 下一条断言红（`get(...)` 是 `None`）；
        //    自己另求一遍和（把 `unverifiable` 也算进去）⇒ 第二条断言红。
        assert!(
            v["data"]["badge_count"].is_u64(),
            "badge_count 那一格没有发出来（侧栏那一格会永远是 hidden）：{v}"
        );
        assert_eq!(v["data"]["badge_count"], v["data"]["failed_count"], "{v}");
        assert_eq!(v["data"]["badge_count"], serde_json::json!(1), "{v}");
        // 颜色走 `to_wire`（它是 `presentation` 的枚举，前端只把档位翻成类名）。
        assert_eq!(v["data"]["headline_color"], to_wire(&summary.headline_color()), "{v}");

        // ---- ② 每一类的「N 项」：**含计数为 0 的那几类**（约束 4）--------------
        let rows = v["data"]["rows"].as_array().expect("rows 是个数组");
        assert_eq!(rows[0]["count_text"], serde_json::json!("1 项"), "{v}");
        assert_eq!(rows[1]["count_text"], serde_json::json!("1 项"), "{v}");
        // 🔴 「0 项」**必须是一句看得见的话**：靠"这一类没有内容"来表达 0，
        //    在客户眼里就是"这一类不见了" —— 而那正是这一屏的硬判据要防的事。
        assert_eq!(rows[2]["count_text"], serde_json::json!("0 项"), "{v}");
        assert_eq!(rows[5]["count_text"], serde_json::json!("0 项"), "{v}");

        // ---- ③ 五格与 `presentation` 的方法逐格同值（改一处就有一处红）--------
        assert_eq!(v["data"]["headline"], serde_json::json!(summary.headline()), "{v}");
        assert_eq!(
            v["data"]["headline_icon"],
            serde_json::json!(summary.headline_icon()),
            "{v}"
        );
        assert_eq!(
            v["data"]["classified_text"],
            serde_json::json!(summary.classified_text()),
            "{v}"
        );
        for (row, src) in rows.iter().zip(summary.rows.iter()) {
            assert_eq!(row["count_text"], serde_json::json!(src.count_text()), "{v}");
        }
    }
}
