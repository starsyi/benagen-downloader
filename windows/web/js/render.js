// render.js —— **载荷 → 壳的数据槽位**（纯映射）。
//
// 本文件**不建 DOM、不碰事件、不碰载荷以外的东西**。它只回答一个问题：
// "命令回的那份 JSON 里，哪一格进壳的哪个槽位"。
//
// 🔴 它是 §3.2 那条纪律的**检查点**：本文件里不应出现任何字符串**字面量**
//    （命令名与形状键除外）。一旦有人在这里写出一句中文，那就是"JS 开始生成
//    界面文案了"的第一个信号 —— 而那正是本代放弃框架、坚持原生 JS 的代价所在：
//    这条纪律**没有编译器兜着**，只能靠"它一眼看得完"来守。
//
// ⚠️ 本文件里出现的键名（`banner` / `kind` / `text` / `shows_retry` …）**是线上契约**，
//    它们的权威在 `shell-core/src/api/`（`state.rs` 等）与 `presentation/` 的
//    `Serialize` 派生。改这里任何一个键名之前，先改那边 ——
//    契约变了而前端没改，表现是"某一格永远空着"，**不会有任何东西变红**。

/** `EngineBannerKind` 的两个变体（`presentation/transfer_row.rs`）。 */
const BANNER_ENGINE_UNAVAILABLE = "EngineUnavailable";

/**
 * `state()` 的 `banner` + `fallback_notice` → 常驻提示行的描述。
 *
 * 图标与颜色**按 kind 取**：`engineUnavailable` 是红三角、`transientError` 是橙圆圈
 * —— 逐字照 `RootView.swift:370-376` 的 `engineBannerBar`（同一个判据、同一对图形）。
 * ⚠️ 那一步在 macOS 是"视图里的一句三元表达式"，在 web 里就是这里；
 *    它属于**绘制层**（把语义标签画成什么），不是**语义层**（该用哪个标签）——
 *    语义（哪个 kind）是 Rust 给的，这里不重判。
 *
 * ⚠️ 顺序即**显示顺序**，而且它是**承重的**：引擎横幅在最上面 ——
 *    它是"现在有东西坏了"，而且那颗「重试」是握手超时那句文案的落点，必须常驻可见。
 *    ⇒ 往这个数组里加东西时，**加在末尾**（新的一律是"你刚才那个动作的结果"那一类）。
 *
 * ⚠️ 今天只接上**前两条**（① 引擎横幅 / ② 退回内核的披露）。剩下三条槽位
 *    （换码结果 / 历史写盘失败 / 改下载目录回执）的生产者在任务 13 —— 它们的值
 *    现在**根本不在 `state()` 的载荷里**，所以这里无从渲染（也**不许**编一句出来）。
 */
export function noticesFromState(state) {
  const out = [];
  const banner = state && state.banner;
  if (banner) {
    const unavailable = banner.kind === BANNER_ENGINE_UNAVAILABLE;
    out.push({
      kind: unavailable ? "error" : "warning",
      icon: unavailable ? "exclamationmark.triangle.fill" : "exclamationmark.circle",
      // 正文是**内核原文逐字**（约束 3）：不 trim、不折行、不加前缀。
      text: banner.text,
      // 补充说明（`banner.hint`）只有握手超时那一支有 —— 它是**壳写的**那句话。
      hint: banner.hint === null || banner.hint === undefined ? null : banner.hint,
      // ⚠️ `shows_retry` 是**判据**（Rust 算的），不是"有错误就给"：
      //    瞬时错误那一支**不带**「重试」—— 拿重启内核去处理一次可自愈的抖动，
      //    等于杀掉一个健康的引擎。
      retry: banner.shows_retry === true,
    });
  }
  // ② W-2 披露：这一次用的**不是**内嵌的那一份内核（退回了同目录那份）。
  //    ⚠️ 它 macOS 侧**没有对应物**（是 Windows 壳自己的诚实披露），所以它的
  //    图标/配色没有"照搬"的对象 —— 这里按 warning 处理（"能用，但不是我们
  //    打算给你的那一份"）。这是本文件里唯一一处**没有出处可照**的取舍。
  if (typeof state?.fallback_notice === "string" && state.fallback_notice.length > 0) {
    out.push({
      kind: "warning",
      icon: "exclamationmark.circle",
      text: state.fallback_notice,
      hint: null,
      retry: false,
    });
  }
  return out;
}

/**
 * `state()` → 工具栏末尾的引擎徽标描述；**没话说时返回 `null`**（整颗不渲染）。
 *
 * 🔴 **本函数今天恒返回 `null`，而那是一个真实的缺口，不是"还没写"。**
 *
 *    它读的三格 —— `engine.text` / `engine.tooltip` / `engine.icon` —— **目前的
 *    `state()` 线上契约里没有**：`api/state.rs` 的 `engine_wire` 只发
 *    `{kind, reason}`（`kind` ∈ connecting / not_started / running / unavailable）。
 *    而徽标上那句话是 `EngineStatusPresentation::text(engine)` 算的
 *    （"运行中" / "正在连接内核…" / "引擎不可用：<原文>"），
 *    **它是 Rust 的判据**（`presentation/engine_status.rs`，13 条测试钉着）。
 *
 *    ⇒ 两条路都走不得：
 *       · 在 JS 里照抄一遍那个 `match` ⇒ **§3.2 明禁**，而且它与 macOS 的文案
 *         从那一刻起就各走各的（这条纪律存在的全部理由就是"文案自动与 macOS 一致"）；
 *       · 拿 `banner.text` 顶上 ⇒ 值**不一样**：横幅给的是内核原文本身，
 *         徽标给的是"引擎不可用：<原文>"（那一支还有握手超时的特例）。
 *         显示一个**看起来对、其实不是那一句**的字符串，比空着更坏。
 *
 *    ⇒ 本任务的选择是**空着**（那颗徽标不渲染），并把这一格如实记在报告里：
 *      **任务 7 要往 `engine_wire` 补三格**（`text` / `icon` / `tooltip`，
 *      全部来自 `EngineStatusPresentation`）。补上之后本函数**一行都不用改**。
 *
 *    ⚠️ 为什么不用 `engine.kind` 自己画一颗"只有图标没有字"的徽标：那个形态正是
 *       本仓库在 macOS 上栽过三次的那个（工具栏只渲染图标、文字掉进 `.help` 浮层，
 *       `RootView.swift:155-158`）。一颗没字的徽标在客户眼里与"没画"无法区分。
 */
export function engineBadgeFromState(state) {
  const engine = state && state.engine;
  if (!engine) return null;
  const text = engine.text; // ← 这一格 wire 里**还没有**（见上面那段）
  if (typeof text !== "string" || text.length === 0) return null;
  let tone = null;
  if (engine.kind === "running") tone = "running";
  else if (engine.kind === "unavailable") tone = "unavailable";
  const tooltip = typeof engine.tooltip === "string" ? engine.tooltip : null;
  // 🔴 **图标名也必须从这一格来**（规格 §3.2 把"图标名"点名列进"必须由 Rust 决定的显示值"）。
  //    它的值是 `EngineStatusPresentation::icon_name`（Rust 的判据，四个名字有单测钉着）。
  //    ⚠️ 这里**不要**拿 `kind` 自己挑一个图形：那等于把 `icon_name` 那张表抄进前端 ——
  //       Rust 改了名字（比如把 `circle.dashed` 换成别的）而界面**不会有任何东西变红**。
  //    取不到名字时给 `null`：徽标会**没有图形**但**保留正文**（正文是更重要的那一半）。
  const icon = typeof engine.icon === "string" && engine.icon.length > 0 ? engine.icon : null;
  return { text, tone, tooltip, icon };
}

/** `state().load` → 批次摘要（未加载 / 加载中 / 失败时返回 `null` ⇒ 整块不渲染）。 */
export function summaryFromState(state) {
  const load = state && state.load;
  if (!load || load.kind !== "loaded") return null;
  return load.summary || null;
}

/** `state().load` → 工具栏那颗交付码按钮的标签（没有生效批次时 `null` ⇒ 静态那一格）。 */
export function codeLabelFromState(state) {
  const summary = summaryFromState(state);
  return summary && typeof summary.code === "string" ? summary.code : null;
}

/**
 * 主区该显示哪一屏 —— **渲染分派**，不是值计算。
 *
 * ⚠️ 这条判据与 macOS 的 `RootView.mainArea` 逐字同形（`RootView.swift`）：
 *    **只有 `.loaded` 进主界面**，其余三态（未加载 / 加载中 / 失败）**都进空态页** ——
 *    加载中那一刻还没有清单可看，失败要原地给原文与「重试」，
 *    而"没有生效的批次"正是空态页要说的事。
 *
 * ⚠️ 所以分区的选择**不影响**这一支：没加载批次时，侧栏点哪个分区主区都是空态页
 *    （与 macOS 一致 —— 侧栏永远可点，但内容由加载态决定）。
 */
export function routeOfState(state) {
  const load = state && state.load;
  if (load && load.kind === "loaded") return { screen: "section" };
  // ⚠️ 缺载荷时给的是 `idle`（不是 `loading`）：macOS 的 `loadState` 初值就是 `.idle`，
  //    而这一支要给的是"还没有结论"，不是"正在加载"。给错的后果是窗口刚开的那几帧
  //    先转圈、再跳成表单 —— 而那几帧正是用户第一眼看到的东西。
  //    ⚠️ 这个默认值必须与 `screens/empty.js` 首帧那个**逐字一致**。
  return { screen: "empty", load: load || { kind: "idle" } };
}

/**
 * `state()` → 轮询闸门。
 *
 * ⚠️ `allows_requests` 缺席时**闸门关着**（`false`）而不是开着：
 *    载荷读不到的时刻（首拍还没回来、命令层还没实现）恰恰是"最不该再发请求"的时刻，
 *    默认开着会让闸门在最需要它的那一段里失效。
 */
export function gateFromState(state) {
  return state?.allows_requests === true;
}
