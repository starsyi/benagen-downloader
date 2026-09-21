// verify.js —— **校验结果屏**：总进度 + 一行总结 + **恒六类**分区。
//
// 对位 `macos/Sources/BenagenDownloader/Views/VerifyView.swift`（顶部总进度 + 一行总结
// + 六个分区）与 `Presentation/VerifySummary.swift`（**已移植**到
// `shell-core/src/presentation/verify_summary.rs`，20 条单测逐字钉着）。
//
// ---------------------------------------------------------------------------
// 🔴 这一屏的硬判据：**恒六类，0 也画**
// ---------------------------------------------------------------------------
// `VerifySummary.rows` **恒为 6 条**（校验通过 / 内容不符 / 文件缺失 / 大小不符 /
// 无法校验 / 无法读取），顺序就是 Rust 给的 `ALL_CLASSES` 的顺序。
// ⇒ 本文件**不筛选、不排序、不"跳过计数为 0 的类"**，也不决定"0 就不画"：
//    那三件事里的任何一件，都会让客户眼里的六类少掉一类（约束 4 的静默失效）。
//    「0 项」那句话本身也是 Rust 给的（`VerifyClassRow::count_text`）。
//
// ---------------------------------------------------------------------------
// 本文件里没有一句面向用户的字符串（规格 §3.2）
// ---------------------------------------------------------------------------
// 六类的 `label` / `color` / `icon_name` / `count_text` / `paths` / `note`、总结的
// `headline` / `headline_color` / `headline_icon` / `classified_text`、总进度的
// `percent_text` / `bytes_text` / `speed_text` / `fraction` —— **每一个字、每一个颜色档、
// 每一个图标名都从载荷取**。本文件只做三件事：建 DOM、把值写进去、摆位置。
//
// 结构文案（「总进度」「—」「还没有校验结果」）住在 `index.html` 的
// `<template id="tpl-screen-verify">` 里 —— 它们是**与数据无关、任何状态下都一样**的字，
// 由浏览器直接渲染，本文件只**克隆与摆放**（理由见 `dom.js:template()` 与 index.html 头部）。
// ⇒ 本文件里**一个汉字都没有**：万一写出一个，那就是"JS 自己造了一句界面文案"。
//
// ---------------------------------------------------------------------------
// 节拍（规格 §3.5）：本屏可见时 1 s 一次，挂在 `ctx.poller` 上
// ---------------------------------------------------------------------------
// `verify()` 的 1 s 是**仅在本屏可见时**的 —— 而"可见"这件事在这里就等于"挂载着"
// （`app.js:applyRoute` 换屏时先 `unmount` 旧的）：⇒ 节拍在 `mount` 里起、在
// `unmount` 里停。本文件**没有**任何 `setInterval`（契约第 3 条：那会在切走之后
// 继续打内核，而那种请求没有任何界面会显示它）。
//
// ⚠️ **一次取两样**：校验结果（`verify`）与总进度（`tree` 的 `progress`）。
//    总进度**只有** `tree()` 给得出（`ProgressSummary` 是整棵树那一支的载荷），
//    而 macOS 那边这两样也是**一起**刷的（`VerifyView.refresh()` 逐字：
//    "只刷一半会留下一个'看起来刷新了、其中一格没动'的屏幕 —— 而这一屏上总进度与六类
//    是同一次交付的两个侧面"）。⇒ 两条请求同一个节拍发出去，各记各的失败。
//    ⚠️ **`tree()` 跟着 `verify()` 按 1 s 轮询**（规格 §3.5 的表格原先只写"用户操作时"）：
//    这一屏要显示总进度，且它有一条**自己**的刷新通道（那颗「刷新」）。
//    控制者已裁定**改规格 §3.5**（把"校验页可见时 `tree()` 也 1 s"写进去），本文件不改。
//
// ⚠️ **「刷新」那颗按钮**（macOS `VerifyView.swift:76-93` 那颗，**逐字同名**）：
//    它做的事与 macOS 逐字同款 —— 一次点 = `verify()` + `tree()` **一起**重取；
//    禁用态也是 macOS 的两条（刷新在飞时不点第二下 / 引擎闸门关着时不许发）。
//    文案与图形住在 `index.html`（**结构文案**，JS 只克隆）；点下去走 `ctx.poller.pollNow()`
//    （理由见 `refreshNow` 的注释：那条路自带重入保护与闸门）。
//    ⚠️ 这个字曾经被写成「重新取一次」（那时 `刷` U+5237 不在内嵌字体子集里）——
//    控制者裁定那是**因果倒置**（不许让字体决定文案）：已连同**一次单独的资产刷新**
//    改回上游原文（子集重生成 + `SUBSET_SHA256` 同步更新）。来历写在 `index.html` 那块注释里。

import { byIdIn, clear, h, icon, setText, template } from "../dom.js";
import { failureText } from "../invoke.js";
import { INTERVALS_MS } from "../poll.js";

export const id = "verify";

/**
 * `RowColor`（Rust 的**语义档**）→ CSS 类名。
 *
 * ⚠️ 这一步是**绘制**（"那个语义档画成什么颜色"），不是语义 —— 与 `render.js` 里
 *    "banner 的 kind → 哪个图标"同一条分工：**该用哪个档位是 Rust 判的**，本文件只把
 *    档位翻成一个类名（颜色本身在 `css/screens/verify.css`，值来自 `tokens.css`）。
 * ⚠️ 认不得的档位 ⇒ **不着色**（继承正文色），不编一个默认色：那会让"Rust 加了第六档
 *    而这里没跟上"的表现从"这一行没有语义色"（看得见）退化成"用了别人的颜色"（看不出）。
 */
const COLOR_CLASS = Object.freeze({
  Secondary: "is-secondary",
  Blue: "is-blue",
  Green: "is-green",
  Red: "is-red",
  Orange: "is-orange",
});

const ALL_COLOR_CLASSES = Object.values(COLOR_CLASS);

/**
 * 挂载。
 *
 * @param {HTMLElement} el 壳给的宿主（`#page` 里的一个 `<div>`）
 * @param {{call: Function, CMD: object, poller: object, shell: object}} ctx
 *   见 `registry.js` 的接口契约（**只有那几样**，别往上加东西）。
 */
export function mount(el, ctx) {
  el.append(template("tpl-screen-verify"));

  // ⚠️ 一律 `byIdIn(el, …)`（不是 `byId`）：挂载这一刻 `el` 还在文档外
  //    （换屏是"先挂好新的、再拆旧的"，见 `app.js:applyRoute`）。
  const barEl = byIdIn(el, "verify-bar");
  const barFillEl = byIdIn(el, "verify-bar-fill");
  const percentEl = byIdIn(el, "verify-percent");
  const bytesEl = byIdIn(el, "verify-bytes");
  const speedEl = byIdIn(el, "verify-speed");
  const progressEmptyEl = byIdIn(el, "verify-progress-empty");

  const headlineEl = byIdIn(el, "verify-headline");
  const headlineIconEl = byIdIn(el, "verify-headline-icon");
  const headlineTextEl = byIdIn(el, "verify-headline-text");
  const classifiedEl = byIdIn(el, "verify-classified");
  const headlineEmptyEl = byIdIn(el, "verify-headline-empty");

  const refreshEl = byIdIn(el, "verify-refresh");

  const failureEl = byIdIn(el, "verify-failure");
  const failureIconEl = byIdIn(el, "verify-failure-icon");
  const failureTextEl = byIdIn(el, "verify-failure-text");
  const dismissEl = byIdIn(el, "verify-dismiss");

  const classesEl = byIdIn(el, "verify-classes");

  // 失败条行首那颗标记（造型照 `exclamationmark.triangle`，颜色由 CSS 的红色
  // currentColor 带下去）。**在挂载时建一次**，不在每次渲染时重建。
  failureIconEl.append(icon("exclamationmark.triangle.fill", "verify__failure-svg"));

  // 「收起」那颗按钮：**字住在 `index.html` 的 `<template>` 里**（`tpl-notice-dismiss`
  // 带的是它的无障碍名字），本文件只克隆与摆放 —— 与 `shell.js:noticeRow` 同一手法。
  {
    const frag = template("tpl-notice-dismiss");
    const btn = frag.querySelector("button");
    btn.prepend(icon("notice.dismiss", "verify__dismiss-svg"));
    btn.addEventListener("click", () => {
      // 收起 = "我看到了"。⚠️ **不清 `shownFailure`**：清掉的话，下一拍失败时
      // `setText` 会重写那一段正文 —— 而那一格是可选中复制的（约束 3）。
      failureEl.hidden = true;
    });
    dismissEl.append(frag);
  }

  // -------------------------------------------------------------------------
  // 本屏的数据（**最近一次成功取到的**）
  // -------------------------------------------------------------------------
  // ⚠️ 它们不是"界面的第二份真相"：界面**只**由这两个值画出来，而这两个值**只**来自
  //    命令的载荷。留一份是因为失败时不该把上一拍的内容抹掉（macOS 同款：
  //    `refreshVerify` 失败不动 `model.verify`）。`null` = 还没取到过。
  /** @type {object|null} `VerifySummary`（六行 + 总结）。 */
  let summary = null;
  /** @type {object|null} `ProgressSummary`（`tree().progress`，**本来就可能是 null**）。 */
  let progress = null;
  /** 上一次画出来的**六类 + 总结行**的签名（没变 ⇒ 那一块一个节点都不动，见 `paint`）。 */
  let paintedSummaryKey = undefined;
  /** 上一次画出来的**总进度**的签名（它与上一条**分开**：理由见 `paint`）。 */
  let paintedProgressKey = undefined;
  /** 上一次画出来的总结图标名（`null` = 现在没有图形）—— 名字没变就不重建那枚 `<svg>`。 */
  let shownHeadlineIcon = null;
  /** 当前生效批次的交付码（换批时要把上一批的校验结果清掉，见 `render`）。 */
  let shownCode;
  /**
   * 侧栏「校验结果」那颗未完成计数徽标的数（载荷里的 `badge_count`，**Rust 算的**）。
   *
   * ⚠️ 换批要与 `summary` / `progress` **一起清零**（见 `render`）：上一批那句
   *    「还有 2 项没通过」摆在**这一批**的侧栏上，是一个没有任何提示的错数
   *    （macOS 侧的同一条是 `performLoadDelivery` 里的 `verify = nil`）。
   */
  let badgeCount = 0;

  /** 一条失败原文（`null` = 这一格现在是空的）。 */
  let shownFailure = null;

  /**
   * 本屏自己的"刷新在飞"标记（macOS 的 `@State private var refreshing`）。
   *
   * ⚠️ 它**只是一个防重复点击的闸**，不是界面状态：界面状态（六类、总结、总进度）
   *    一律以载荷为准。有它是因为"一次刷新"= 一次**两条**请求的重取，
   *    而轮询的节拍本身可能正有一拍在飞（`poll.js` 的重入保护会跳过那一拍 ——
   *    那时这个闸由那一拍落地时清掉，见 `tick` 的 `finally`）。
   */
  let refreshing = false;

  /**
   * 「刷新」那颗按钮此刻该不该可按。
   *
   * 🔴 判据是 macOS 的两条（`VerifyView.swift:86` 的 `.disabled(refreshing || !engineAllowsActions)`）：
   *    · **一次刷新在飞**时不许点第二下；
   *    · **引擎闸门关着**时不许发（`EngineGate`）——闸门本身是 Rust 算的
   *      （`state().allows_requests`），本文件**只读不判**，经 `ctx.poller.gateOpen`
   *      拿到 `app.js` 灌进去的那一格（`poll.js` 把它标成"给屏自己判断用"）。
   *      ⚠️ 这也是这一屏**唯一**能拿到闸门的地方：`render()` 只收到 `state().load`。
   */
  function syncRefreshEnabled() {
    const want = refreshing || !ctx.poller.gateOpen;
    // ⚠️ **值没变就不写属性**：这一条每秒被 `render()` 调一次，而无脑写 `disabled`
    //    等于每秒碰一次那个按钮的 DOM —— 同一条纪律（"每拍都在变的东西"不许去动 DOM），
    //    只是这次动的不是子节点而是属性（焦点在它身上时尤其不该碰）。
    if (refreshEl.disabled !== want) refreshEl.disabled = want;
  }

  /**
   * 显示一次失败（原文逐字，约束 3）：不 trim、不折行、不加前缀。
   *
   * 🔴 **同一句话不重写第二遍** —— 与 `empty.js:showFailure` 同一条理由：这一格是
   *    可选中复制的，而它被**每秒一次的节拍**喂；无脑重写 `textContent` 会换掉那个文本
   *    节点 ⇒ **客户正拖着选区准备复制的那段原文，每秒被清一次**。
   */
  function showFailure(message) {
    if (shownFailure !== message) {
      setText(failureTextEl, message);
      shownFailure = message;
    }
    failureEl.hidden = false;
  }

  function hideFailure() {
    if (shownFailure !== null) {
      setText(failureTextEl, "");
      shownFailure = null;
    }
    failureEl.hidden = true;
  }

  /** 把 `RowColor` 的语义档翻成一个类名（见 `COLOR_CLASS`）。 */
  function applyColor(target, color) {
    const wanted = COLOR_CLASS[color] || null;
    for (const name of ALL_COLOR_CLASSES) target.classList.toggle(name, name === wanted);
  }

  // -------------------------------------------------------------------------
  // 总进度（`ProgressSummary` 的四个字段）
  // -------------------------------------------------------------------------
  /**
   * @param {object|null} p `tree().progress`：**`null` 是正常的**（树没到 / 不是这一批）。
   *   ⚠️ 那时**说「—」**（那句住在 `index.html`），不留一格空白、更不编一个 `0%` ——
   *   后者是"替内核宣布一句它没说过的话"（同 `api/tree.rs` 里 `progress` 收 `Option`
   *   的那段论证）。
   */
  function paintProgress(p) {
    const has = p !== null && p !== undefined;
    progressEmptyEl.hidden = has;
    barEl.hidden = !has;
    percentEl.hidden = !has;
    bytesEl.hidden = !has;
    speedEl.hidden = !has;
    if (!has) return;

    setText(percentEl, p.percent_text);
    setText(bytesEl, p.bytes_text);
    setText(speedEl, p.speed_text);

    // 进度条的长度 = `fraction`（Rust 算的、恒在 0…1）。⚠️ 这里**不拿 percent_text
    // 反解**（那一格在总量为 0 时是「—」，反解会得到 NaN）。
    const fraction =
      typeof p.fraction === "number" && Number.isFinite(p.fraction) ? p.fraction : null;
    if (fraction === null) {
      barFillEl.style.removeProperty("--verify-fraction");
      barEl.removeAttribute("aria-valuenow");
      barEl.removeAttribute("aria-valuetext");
      return;
    }
    // 🔴 **JS 一个数都不换算**（规格 §3.2）：条的长度交给 CSS 的 `calc()` 去乘
    //    （与 `transfers.js` 的 `--transfers-fraction` **逐字同款**），
    //    `aria-valuenow` 直接写 Rust 给的那个**原始比例**（模板的
    //    `aria-valuemin/max` 就是 0/1，见 `index.html` 那一段）。
    //
    //    ⚠️ **这里曾经写的是 `Math.round(fraction * 100)`**（2026-09-20 的整分支审查
    //    抓出来的 I-4），两处都错：
    //      · 它正是 §3.2 点名的那一类"**JS 自己算出来的显示值**"—— 一个非 ASCII 字
    //        都没有，所以 §3.2 的指纹守卫**结构上**照不到它，Rust 单测也管不着；
    //      · 而且它与 Rust 的口径**相反**：`PercentFormat` 是**整数除法向零截断**
    //        （"四舍五入会让 99.5% 提前显示成 100%，客户以为下完了"）。
    //        199/200 时屏幕上（Rust 的 `percent_text`）写着「99%」而读屏**读到 100%**；
    //        总量为 0 时屏幕上是「—」而读屏读到 0%。
    //      · 同一件事在 `transfers.js` 那一屏**从来不做换算** —— 两块屏两种做法，
    //        而只有这一块不是 Rust 的口径。现在两处逐字同款。
    barFillEl.style.setProperty("--verify-fraction", String(fraction));
    barEl.setAttribute("aria-valuenow", String(fraction));
    // ⚠️ 读屏**真正念出来**的是 `aria-valuetext`（它在场时优先于 `aria-valuenow`）——
    //    这一格写 Rust 的 `percent_text` ⇒ **读屏读到的就是屏幕上那一格的字**
    //    （含总量为 0 时的「—」：读屏不会再念出一个 0%）。
    barEl.setAttribute(
      "aria-valuetext",
      typeof p.percent_text === "string" ? p.percent_text : ""
    );
  }

  // -------------------------------------------------------------------------
  // 一行总结（`headline` / `headline_color` / `headline_icon` / `classified_text`）
  // -------------------------------------------------------------------------
  function paintHeadline(s) {
    const has = s !== null && s !== undefined;
    headlineEl.hidden = !has;
    // 还没有第一份结果 ⇒ 说「还没有校验结果」（那一句住在 `index.html`）：**不画六个 0**
    // ——那会读成"全部通过"，而这句话谁都没说过（`VerifySummary::of` 的 `None` 那一段）。
    headlineEmptyEl.hidden = has;
    if (!has) return;

    setText(headlineTextEl, s.headline);
    setText(classifiedEl, s.classified_text);
    applyColor(headlineEl, s.headline_color);

    // 图形：名字从载荷来（`headline_icon` ← `VerifyVerdict::icon_name`）。
    // ⚠️ 名字变了才重建 —— 这与"上一句不重写第二遍"是同一条理由（每秒一拍）。
    const name = typeof s.headline_icon === "string" ? s.headline_icon : null;
    if (name !== shownHeadlineIcon) {
      shownHeadlineIcon = name;
      clear(headlineIconEl);
      if (name) headlineIconEl.append(icon(name, "verify__headline-svg"));
    }
  }

  // -------------------------------------------------------------------------
  // 六类（**恒六条**）
  // -------------------------------------------------------------------------
  /**
   * 一类：标记 + 标签 + 计数 +（说明）+ 这一类自己的**全部路径**。
   *
   * ⚠️ **计数为 0 的类照画**：这一块的存在与否**只**取决于 `rows` 里有没有它，
   *    而 `rows` 恒 6 条（口径在 `VerifySummary::of`，有单测钉着）。本函数里
   *    **没有**任何 `if (count === 0) return;` 一类的写法 —— 那正是约束 4 要防的。
   */
  function classBlock(row) {
    const block = h("div", { class: "verify__class" });
    applyColor(block, row.color);

    const head = h("div", { class: "verify__class-head" });
    const name = typeof row.icon_name === "string" ? row.icon_name : null;
    if (name) {
      const iconEl = h("span", { class: "verify__class-icon" });
      iconEl.append(icon(name, "verify__class-svg"));
      head.append(iconEl);
    }
    head.append(h("span", { class: "verify__class-label", text: row.label }));
    head.append(h("span", { class: "verify__class-count", text: row.count_text }));
    block.append(head);

    // 标签下面那句说明：**只有「无法校验」有**（Rust 的 `note` 是 `Option`）。
    // ⚠️ 这里判的是"这一格有没有值"（字符串），**不是**判"是哪一类" ——
    //    "哪一类该有说明"是 Rust 的判据，本文件不重写一份。
    if (typeof row.note === "string") {
      block.append(h("div", { class: "verify__class-note", text: row.note }));
    }

    // 🔴 **这一类**的全部路径，一条不漏、原文照登（约束 3）：不取最后一段、
    //    不规范化、不转义、不去重、不排序。
    for (const path of Array.isArray(row.paths) ? row.paths : []) {
      block.append(pathRow(path));
    }
    return block;
  }

  /**
   * 一条路径（**中间截断**，`.truncationMode(.middle)` 的对应物）。
   *
   * ## 为什么是这样写的（这一段是这一屏唯一一处需要论证的地方）
   *
   * **纯 CSS 做不到真正的中间省略**：`text-overflow: ellipsis` 只会在**行尾**加省略号；
   * 那个"`direction: rtl` + `text-align: left`"的经典手法截的是**开头**（留住文件名），
   * 两端只有一端在 —— 而规划与 macOS 源（`VerifyView.swift:181-182`）要的都是**中间**。
   * CSS 里的 `text-overflow: ellipsis middle` 至今没有任何浏览器实现。
   *
   * ⇒ 这里用**两个紧邻的椭圆格**：把一个字符串按**路径分隔符**切成"首段 + 尾段"，
   *   让 CSS 去决定首段能放下多少（放不下的部分由**浏览器**画成省略号）。
   *
   * ⚠️ 这**不是**"JS 在造/加工一句要显示的字符串"（§3.2），理由三条：
   *    ① **一个字符都没有被改写**：两格拼起来的文本与内核给的路径**逐字节相同**
   *       （切点是一个下标，不是一次改写）。省略号是**浏览器画的**，不是本文件写进去的
   *       —— 本文件里没有 `"…"` 这个字面量；
   *    ② **切点是结构性的、与数据无关**：所有路径都按同一个规则切（最后一个分隔符），
   *       不存在"这条路径该怎么显示"的判断 —— 判断只发生在 CSS 里（放不下就裁）；
   *    ③ **全文两处都在**：`title` 给悬停，**选中复制拿到的也是全文**（两格是紧邻的行内块，
   *       跨格选择得到的就是原来那一整条路径）。
   *    ⚠️ 真正被 §3.2 明禁的是另一件事：**按像素测量后把字符串切掉一段、再拼一个省略号** ——
   *       那会把"内核说了什么"改成"我们想显示什么"。这里没有那样做。
   *    （`truncationMode(.middle)` 在 SwiftUI 里做的也正是这件事：算出首尾两段、中间画省略号。）
   *
   * ⚠️ 没有分隔符（或路径以分隔符结尾）时：整条都进"首段"，尾段为空 ——
   *    退化成**末尾**省略。那是最坏情况下的兜底，不是常态。
   */
  function pathRow(path) {
    const cut = path.lastIndexOf("/");
    const head = cut >= 0 ? path.slice(0, cut + 1) : path;
    const tail = cut >= 0 ? path.slice(cut + 1) : "";
    const row = h("div", { class: "verify__path selectable", title: path });
    // ⚠️ 两格之间**不能有空白文本节点**（那会在路径中间多出一个空隙）
    row.append(h("span", { class: "verify__path-head", text: head }));
    row.append(h("span", { class: "verify__path-tail", text: tail }));
    return row;
  }

  /**
   * 把这一刻的数据画出来。**内容没变就一个节点都不动。**
   *
   * 🔴 这不是性能优化（同 `shell.js:setNotices` 的 `_noticesKey`）：路径是**可选中复制**
   *    的（约束 3），而这一屏每 1 s 被喂一次。每拍重建 DOM 的后果是
   *    **选区每秒被清一次、滚动位置每秒回到顶上** —— 而那两个症状只在"真的用手去复制
   *    一条路径"时才出现，代码里看不出来。
   *
   * 🔴 **两样东西各有各的键**（这一条是**修出来的**，见下）：
   *    · `summary` 决定**六类那整块 + 总结那一行**（重活：那一块要重建 DOM）；
   *    · `progress` 只决定**顶部那四个数 + 那根条**（轻活：那几个节点在挂载时就建好了，
   *      这里只写文本、改宽度）。
   *
   * ⚠️ **曾经两样共用一个键**：`JSON.stringify([summary, progress])`。而 `progress` 的
   *    `percent_text` / `bytes_text` / `speed_text` / `fraction` **每秒都在变**（下载在跑）
   *    ⇒ 那一块**每秒被 `clear()` + 重建一次** ⇒ **客户正在选中复制的路径，选区每秒被清一次、
   *    滚动位置每秒回到顶上**。而这一屏恰恰是为"逐条辨认哪个文件出了问题"设计的，
   *    路径列表就是用户最想复制的东西（macOS 那一屏的同一条理由写在 `VerifyView.swift`）。
   *    ⚠️ 这与任务 9 在 `shell.js:setNotices` 上修的是**同一类缺陷的第二次出现**
   *    （那次清掉的是"客户正在复制的失败原文"）—— 两处的判据一样：
   *    **"每拍都在变的东西"不许进"重建 DOM"那个键**。
   *    ⇒ 钉住它的是 `task-12-harness/paint-check.js`（载荷 F：六类不变、只有 `progress` 在变
   *    ⇒ 六类区块的 `childList` 变更 **0 次**、路径节点**还是同一个**、选区与焦点都还在；
   *    并有一组对照：让 `summary` 真变一次 ⇒ 变更数立刻起来，证明那个 0 不是"观察器没装上"）。
   */
  function paint() {
    // ① 六类 + 总结行：**只有 summary 变了才动**（`progress` 不许把它们拖下水）
    const summaryKey = JSON.stringify(summary);
    if (summaryKey !== paintedSummaryKey) {
      paintedSummaryKey = summaryKey;
      paintHeadline(summary);

      clear(classesEl);
      const rows = summary && Array.isArray(summary.rows) ? summary.rows : null;
      // 还没有结果 ⇒ **一个分区都不画**（不是画六个空的）。有结果时**六条全画**，
      // 顺序与条数都是 Rust 给的。
      if (rows) for (const row of rows) classesEl.append(classBlock(row));
    }

    // ② 总进度：那一格**每拍都可以更新**（数字本来就该跟着下载走），但它**只更新自己**
    //    —— 那几个节点是挂载时建的，这里只改文本与那根条的宽度，一个 `<div>` 都不重建。
    const progressKey = JSON.stringify(progress);
    if (progressKey !== paintedProgressKey) {
      paintedProgressKey = progressKey;
      paintProgress(progress);
    }

    // ③ 那颗按钮的可用性：**值没变就不碰属性**（同一条纪律：别每拍写一次 DOM）
    syncRefreshEnabled();
  }

  // -------------------------------------------------------------------------
  // 拍照（节拍）
  // -------------------------------------------------------------------------
  /** 一条命令的结果：成功给 `data`，失败给**原文逐字**（见 `invoke.js` 的契约）。 */
  function settle(promise) {
    return promise.then(
      (data) => ({ ok: true, data }),
      (error) => ({ ok: false, message: failureText(error) })
    );
  }

  /**
   * 一拍：`verify()` + `tree()` 并发取，各记各的失败。
   *
   * ⚠️ **两条请求的失败落点**：macOS 那边 `refreshVerify` 的失败落顶部常驻横幅、
   *    `getTree` 的失败落本屏的失败条；web 这一代两条都只能落在**本屏**（屏不许调
   *    壳级的 setter，见 `registry.js` 契约第 1 条）。⇒ 同一条失败条，
   *    先到的那条原文优先（两条同时挂时通常是同一句"还没有连上内核"）。
   */
  async function tick() {
    let failure = null;
    try {
      const [v, t] = await Promise.all([
        settle(ctx.call(ctx.CMD.verify)),
        settle(ctx.call(ctx.CMD.tree)),
      ]);

      if (v.ok) {
        summary = v.data;
        // 侧栏「校验结果」那颗徽标的计数：**Rust 算好随这一格载荷下来**
        //（`api::verify::verify` 的 `badge_count` ← `SidebarBadge::unpassed_of_summary`）。
        // ⚠️ **本屏一个数都不自己数**（规格 §3.2，"哪些算未通过"的判据在 `presentation`：
        //    `unverifiable` **不算**）。取不到时留 `0`（"没有数据"不等于"有一堆没通过"）。
        badgeCount = typeof v.data.badge_count === "number" ? v.data.badge_count : 0;
      } else {
        // ⚠️ **不清掉上一次的 `summary`**：一次抖动不该让整屏内容消失（macOS 同款）。
        failure = v.message;
      }
      if (t.ok) {
        // `progress` 本来就可能是 `null`（树不是这一批的）⇒ 原样收下，不当成失败。
        progress = t.data ? (t.data.progress ?? null) : null;
      } else if (failure === null) {
        failure = t.message;
      }
    } finally {
      // 任何一拍落地（不管是节拍还是那颗「刷新」按出来的）都算"刷新完了" ⇒ 放开闸。
      // ⚠️ 放在 `finally`：失败那一支也要放开，否则按钮会永远灰着。
      refreshing = false;
      syncRefreshEnabled();
    }

    if (failure !== null) showFailure(failure);
    else hideFailure();
    paint();
  }

  /**
   * 「刷新」：**用户动作 ⇒ 立刻补一拍**（`poll.js` 的 `pollNow` 就是为这个存在的）。
   *
   * ⚠️ 用它而**不是**自己调一次 `tick()`：`pollNow` 走的是节拍器那条路，天生带
   *    **重入保护**（正有一拍在飞时跳过）与**闸门**（引擎不可用时一拍都不发）。
   *    自己调 `tick()` 会绕过这两条，而绕过闸门正是 macOS 注释里点名的
   *    "结构性防线"（引擎不可用时一个请求都不发）。
   * ⚠️ 若这一下被重入保护跳过（那一拍正飞着），闸由**那一拍**落地时放开 ——
   *    所以按钮最多灰到那一拍结束，不会卡住（`tick` 的 `finally`）。
   */
  function refreshNow() {
    if (refreshing) return;
    refreshing = true;
    syncRefreshEnabled();
    ctx.poller.pollNow();
  }

  refreshEl.addEventListener("click", refreshNow);

  /**
   * 复制路径时，把**浏览器插在块边界上的那个换行**还原掉。
   *
   * 🔴 这是"两格切分"这套做法的**代价**，实测出来的：路径被切成两格**紧邻**的
   *    `<span>`（中间截断，见 `pathRow`），而两格都是**块级盒子**（flex 子项会被
   *    blockify）⇒ **浏览器在序列化跨块边界的那段选区时，会在边界处插一个 `\n`**。
   *    实测（`task-12-harness/copy-check.js`，无头 Chromium）：
   *      · `document.getSelection().toString()` = `"…/25WS028/\nC24-8_…_1.fq.gz"`
   *      · 同一个节点的 `textContent`      = `"…/25WS028/C24-8_…_1.fq.gz"`（原文）
   *    ⇒ 客户**拖选一条路径复制**时，剪贴板里会多一个换行 —— 而"路径原文可整段复制"
   *      正是这一屏存在的理由之一（约束 3；macOS 那边是单个文本节点，没有这件事）。
   *
   * ## 这一段为什么不算"JS 在加工界面字符串"（§3.2）
   *
   *    · 它**不新增、不改写、也不删掉内核给的任何一个字符** —— 它删掉的是
   *      **浏览器自己插进去的那一个 `\n`**，得到的字符串与 `rows[i].paths[j]`
   *      **逐字节相同**（这正是不做这一步时**做不到**的事：那时剪贴板里的是被浏览器
   *      改过的那一份）。
   *    · 它**只在真的命中**"我切的那两格 + 那个换行"时才介入（`clean === text` 就
   *      原样放行，连 `preventDefault` 都不调）—— 用户自己选区里的换行、别的元素
   *      之间的换行，一个都不碰。
   *    · **管辖面 = 本屏里每一对"我切开的相邻两格"**，一处不漏。
   *
   * ## ⚠️ 第二处（修出来的）：每一类那一行的「标签 + 计数」
   *
   * 这一段原先逐字写着"屏里**只有这一处**跨块边界的文本"—— **那句话不成立**，
   * 而它的代价是实测过的：`.verify__class-head` 是 `display: flex`，它的两个子项
   * （`.verify__class-label` 与 `.verify__class-count`）**会被 blockify**，
   * 与路径那两格是**同一个机制** ⇒ 客户跨格选「校验通过 12 项」复制出来的是
   * 「校验通过`\n`12 项」——**剪贴板里的东西与屏幕上呈现的不是同一个**
   * （R-58 那条界线要保护的正是这个：JS 可以还原浏览器插进去的产物，但必须保守、
   * 必须有测试）。同族的两处（这里 + `transfers.js` 的标题）都配了处理器与保守性断言，
   * 只有这一处漏了 —— 因为它落在"路径"这一个类名上，而这一对格子不叫 `verify__path`。
   * ⇒ 判据从"**一个类名**"改回"**这件事**"：凡是本屏切开的相邻两格，都按同一条规则还原。
   *
   * ⚠️ 这里**没有**做"按 CSS 算出来哪些格子会被 blockify"那种聪明事：那要把排版知识
   *    抄进 JS，而且换一次样式就悄悄失效。写的仍然是"**匹配得上才碰**"——
   *    匹配不上（文本里根本没有这个组合）⇒ 一个字符都不动。
   */
  function restoreVerbatimOnCopy(event) {
    const clipboard = event.clipboardData;
    const selection = document.getSelection();
    if (!clipboard || !selection || selection.isCollapsed) return;
    const text = selection.toString();
    let clean = text;

    /** 把"两格之间那个换行"还原掉（两格都在、且文本里真的出现这个组合时才动）。 */
    const joinVerbatim = (a, b) => {
      // 只有"两格都在"的那些才可能出现那个边界（空格子没有边界可言）
      if (a === "" || b === "") return;
      clean = clean.split(`${a}\n${b}`).join(a + b);
    };

    // ① 路径那两格（`pathRow` 的中间截断）。
    for (const row of classesEl.querySelectorAll(".verify__path")) {
      joinVerbatim(
        row.querySelector(".verify__path-head").textContent,
        row.querySelector(".verify__path-tail").textContent
      );
    }
    // ② 每一类那一行的「标签 + 计数」（同一族的第二处，见上）。
    for (const head of classesEl.querySelectorAll(".verify__class-head")) {
      const label = head.querySelector(".verify__class-label");
      const count = head.querySelector(".verify__class-count");
      if (!label || !count) continue;
      joinVerbatim(label.textContent, count.textContent);
    }

    if (clean === text) return; // 没命中 ⇒ 一个字符都不碰
    clipboard.setData("text/plain", clean);
    event.preventDefault();
  }

  classesEl.addEventListener("copy", restoreVerbatimOnCopy);

  /**
   * 壳每一拍把 `state().load` 交进来。
   *
   * ⚠️ 本屏的**内容**不来自这里（校验结果与总进度是这一屏自己按 `verify` 的节拍取的）；
   *    这里只用它回答一个问题："**生效批次换了吗**"。
   *    换了就必须把上一批的校验结果与总进度**清掉** —— 否则在下一拍回来之前（最多 1 s），
   *    上一批的结果会挂在**这一批**的摘要底下。这与 `ProgressSummary::of` 里
   *    `tree_code == code` 那道闸、以及 `BrowserSelection.on_new_manifest` 是**同一条口径**。
   */
  function render(load) {
    const code =
      load && load.kind === "loaded" && load.summary && typeof load.summary.code === "string"
        ? load.summary.code
        : null;
    if (code !== shownCode) {
      shownCode = code;
      summary = null;
      progress = null;
      // ⚠️ 侧栏那颗徽标的数**也是这一批的**（`badge_count` 随校验结果一起下来）⇒
      //    换批要一起清零。不清的后果是"侧栏挂着上一批的 2、这一屏说的是「尚未校验」"
      //    —— 判据与 `ProgressSummary::of` 的 `tree_code == code` 是同一条。
      //    ⚠️ 清零之后下一拍就把这一批的数报回来（这一屏的节拍是 1 s）。
      badgeCount = 0;
      hideFailure();
    }
    // 闸门（`allows_requests`）是**壳每一拍灌进 poller 的** ⇒ 那颗按钮的可用性跟着它走，
    // 而这里正是屏能看见"壳刚灌过"的唯一时机。⚠️ 这一步在 `paint()` 的末尾（值没变就
    // 不碰属性），所以这里不重复调 —— 一处写、一处判。
    paint();
  }

  // 首帧按"什么都没有"画：`render` 的默认值（未加载 ⇒ `code === null`）与这里逐字一致，
  // 而屏刚挂上时**第一拍还没回来** —— 那几帧正是用户切过来第一眼看到的东西。
  render(null);

  // 节拍：名字用命令名（`poll.js` 的约定），只在**本屏挂载着**的这段时间里跑。
  ctx.poller.start(ctx.CMD.verify, INTERVALS_MS.verify, tick);

  return {
    render,
    /**
     * **侧栏「校验结果」那颗徽标的数**（`registry.js` 契约里的可选那一格）。
     *
     * 🔴 屏为什么必须回答它：这个数只有**这一屏的载荷**里有（`badge_count`），
     *    而徽标长在**壳**上、要活过换屏 ⇒ 由 `app.js` 每拍问一次再喂给壳。
     *    ⚠️ 本函数**不数任何东西**（规格 §3.2）：它把载荷里那一格原样交出去。
     *    ⚠️ `0` = 这一格不渲染（macOS 的 `.badge(0)`）。
     */
    navBadge() {
      return { section: "verify", count: badgeCount };
    },
    unmount() {
      // 🔴 停掉本屏的节拍（契约第 3 条）。⚠️ 只停**自己起的**那一条：
      //    `state` 是壳的（它不属于任何一屏、也不该跟着屏停）。
      ctx.poller.stop(ctx.CMD.verify);
      clear(el);
    },
  };
}
