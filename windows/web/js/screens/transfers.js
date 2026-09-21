// transfers.js —— **传输列表屏**：行（进度/速度/状态/错误原文/暂停角标）+ 行菜单
//                  + 顶部全局摘要 + 空态，**200 ms 一拍**。
//
// 对位 `macos/Sources/BenagenDownloader/Views/TransfersView.swift` 与
// `TransferRowView.swift`（行/菜单/摘要/空态四块逐段对位）；
// 每一个显示值来自已经移植好的 `shell-core/src/presentation/transfer_row.rs`
// （`TransferRow` / `TransferGlobalSummary` / `TransferListEmpty` / `TransferActionFailure`
// / `EngineGate` / `TransferListPoll`，42 条单测逐字钉着）。
//
// ---------------------------------------------------------------------------
// 🔴 这一屏的判据：`TransferRow` 的 **13 个字段**逐一落位（规格 §3.2）
// ---------------------------------------------------------------------------
//   ① `gid`               → 行元素的 `data-gid`（这一行的身份，也是 `task_action` 的实参）
//   ② `title`             → `.transfers__title` 里那**两格**的文本（首段 + 尾段，
//                            **中间省略**；`×` 与空格照原样 —— 见 `paintTitle`）
//   ③ `manifest_path`     → `reveal` 的实参；**它同时就是**菜单里「在资源管理器中显示」
//                            出不出现的判据（见下面 `openMenu` 那段）
//   ④ `progress_text`     → `.transfers__progress` 的文本
//   ⑤ `percent_text`      → `.transfers__percent` 的文本 + 进度条的 `aria-valuetext`
//   ⑥ `progress_fraction` → 进度条的**宽度**（`--transfers-fraction`，恒 0.0–1.0）
//                            与 `aria-valuenow` —— **JS 不做任何换算**（见 `paintRow`）
//   ⑦ `speed_text`        → `.transfers__speed`
//   ⑧ `state_label`       → `.transfers__state`
//   ⑨ `state_color`       → `.transfers__state` 与 `.transfers__icon` 的语义色类
//   ⑩ `state_icon_name`   → `.transfers__icon` 里那枚 `<svg>`（**名字变了才重建**）
//   ⑪ `shows_paused_badge`→ `.transfers__badge` 的 `hidden`（判据只能来自 `raw_status`）
//   ⑫ `error_text`        → `.transfers__error` 的文本（**行下方全文展开、不截断、可选中**）
//   ⑬ `available_actions` → **行菜单**（打开时按它建）+ 「⋯」那颗按钮的可点性；
//                            **空集 = 一个动作都不给**（见 `openMenu`）
//
// ⚠️ 本文件里**没有一句面向用户的字符串**：以上 13 格全部来自载荷，而结构文案
//    （菜单项名 / 角标名 / 那颗「清空已完成」）住在 `index.html` 的
//    `tpl-screen-transfers*` 三个模板里，本文件只**克隆与摆放**（`dom.js:template()`）。
//
// ---------------------------------------------------------------------------
// 🔴 节拍：**200 ms**，而且**只在屏可见时跑**（规格 §3.5）
// ---------------------------------------------------------------------------
//   | 数据 | 节拍 | 依据 |
//   | `transfers()` | 200 ms | `TransferListPoll::INTERVAL_NANOSECONDS` |
//
//   · 节拍**挂在 `ctx.poller` 上**（契约第 3 条：屏不许有定时器），名字用命令名；
//   · "可见"在本代 = "挂载着"（`app.js:applyRoute` 换屏时先 `unmount` 旧的）
//     ⇒ 在 `mount` 里起、在 `unmount` 里停；
//   · 闸门是 `state().allows_requests`（`EngineGate`）：`poll.js` 对**没有标记 ungated**
//     的任务一律拦下 ⇒ 本屏什么都不用判（引擎不可用时一拍都不发）。
//
// ---------------------------------------------------------------------------
// 🔴 200 ms 一拍**只改文本与宽度，一个节点都不建**（这一条是本代的第三次同类缺陷）
// ---------------------------------------------------------------------------
//   任务 9 在 `shell.js:setNotices` 上修过一次、任务 12 又在 `verify.js` 上修过一次，
//   两次的病一样：**"每拍都在变的东西"被放进了"重建 DOM"那个键**，于是整块 DOM
//   每秒被重建一次，把用户正拖着的选区与滚动位置清掉。
//   这一屏的风险更高：**错误原文正是客户要复制给业务方的东西**（约束 3），
//   而它每 200 ms 被喂一次（一秒五次）。
//
//   ⇒ 本文件里的三条具体做法：
//     ① 行 DOM **按 `gid` 复用**：只有 `gid` 序列变了才动结构（`syncRowDom`），
//        已经画出来的行**元素不重建** —— 重组、重排、文本变化都不碰节点；
//     ② 把值写进已经建好的那一格时走 `writeText()`（改**文本节点的 `nodeValue`**），
//        **不是** `dom.js:setText` 的 `el.textContent = …` —— 后者会把那个文本节点
//        换成一个新的（一次 `childList` 变更），200 ms 一次就是"选区每 200 ms 被清一次"；
//     ③ 进度条的宽度写的是**自定义属性**（`--transfers-fraction`），CSS 里
//        `calc(var(...) * 100%)` —— 于是 JS 连"乘 100 拼一个 `%`"都不用做（§3.2）。
//      错误原文那一格**常驻**（没有错误时是 `hidden`）：它不随"有没有错误"而增删节点。
//
//   钉住它的用例：`windows/scripts/frontend-stub/transfers-harness.html` 的
//   `runPaintCheck` —— 载荷每拍只改数值（`transfersTick0`/`transfersTick1` 两份都由
//   Rust 倒出来）⇒ 行容器的 `childList` 变更数 **0**、行元素与错误原文的文本节点
//   **还是同一个**、选区还活着；**并有一组对照**：把载荷换成"多一行"⇒ 变更数立刻起来
//   （证明那个 0 不是"观察器没装上"）。
//
// ---------------------------------------------------------------------------
// ⚠️ 三条不许越过的线（`js/screens/registry.js` 的文件头）
// ---------------------------------------------------------------------------
//   ① 不碰壳（`#nav` / `.toolbar` / `#notices` / `#batch-summary`）—— 只往宿主 `el` 里放东西；
//   ② 不自己 `invoke` —— `transfers` / `task_action` / `reveal` 一律走 `ctx.call`；
//   ③ 不起自己的定时器 —— 那一拍挂在 `ctx.poller` 上（且只在挂载期间跑）。
//
// ⚠️ **引擎横幅（`EngineBanner`）与闸门（`EngineGate`）不在本屏**：横幅是壳的常驻提示行
//    （`app.js` 每一拍灌进 `shell.setNotices`），闸门由 `poll.js` 执行。
//    本屏与它们的关系只有两条，两条都是**读**：
//      · `empty_text` 为 `null` 时**这一屏什么都不说**（引擎不可用那一档，
//        `TransferListEmpty::of` 的判据：横幅已经在说同一句话了）；
//      · 动作面的可用性读 `ctx.poller.gateOpen`（`EngineGate::allows_requests` 的执行结果）。

import { byIdIn, clear, icon, template } from "../dom.js";
import { failureText } from "../invoke.js";
import { INTERVALS_MS } from "../poll.js";

export const id = "transfers";

/**
 * `RowColor`（五个成员，`presentation/mod.rs`）→ 本屏状态列的类名后缀。
 *
 * ⚠️ **这张表是"语义 → 画法"的那一步**（同 `files.js` / `verify.js` 里各自的同名表）：
 *    名字是 Rust 给的（载荷里的 `state_color` ← `TransferRow::color()`，有单测钉着
 *    "五档两两不同"），这里只决定它画成哪个色（值在 `css/screens/transfers.css`）。
 *    ⚠️ 五个成员一个都不能少；取不到值时**不给类名**（退回继承色），而不是随便挑一个
 *    —— 挑错就是"两个状态画成一样"（约束 4）。
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
 * 「清空已完成」那一条的线上字面量（`TaskAction::ClearFinished`）。
 *
 * 🔴 它是一个**单独的常量**，不是一张表：`TaskAction` 的**动作面**（哪一行有哪几个动作）
 *    完全由载荷里的 `available_actions` 决定，本文件里**没有**任何"某状态下有哪几个动作"
 *    的判断（那是 `TransferRow::actions()` 的判据，把它抄一份到前端 = 第二份会分叉的真相）。
 *    这里唯一写死的，是"那颗列表级按钮按下去发哪个动作名"。
 *    字面量由 `protocol.rs` 的 `task_action_pins_the_five_wire_literals` 逐字钉着
 *    （内核按字面量匹配）。行级那五个字面量**不在这里** —— 它们在
 *    `index.html` 的 `tpl-screen-transfers-menu` 里，和 Item 的名字待在一起。
 */
const ACTION_CLEAR_FINISHED = "clear_finished";

/** 本屏那一拍的任务名（`poller.start` / `poller.stop` 用的是同一个键）。 */
const TICK = "transfers";

/**
 * 挂载。
 *
 * @param {HTMLElement} el 壳给的宿主（`#page` 里的一个 div）
 * @param {{call: Function, CMD: object, poller: object, shell: object}} ctx
 *   见 `registry.js` 的接口契约（**只有那几样**）。
 */
export function mount(el, ctx) {
  // 结构（含结构文案）全部从 `index.html` 的模板克隆 —— 本文件一个字都不造。
  el.append(template("tpl-screen-transfers"));

  // ⚠️ 一律 `byIdIn(el, …)`（**不是** `byId`）：挂载这一刻 `el` 还在文档外
  //    （换屏是"先挂好新的、再拆旧的"，见 `app.js:applyRoute`）。
  const root = byIdIn(el, "transfers-root");
  const headerEl = byIdIn(el, "transfers-header");
  const headerDataEl = byIdIn(el, "transfers-header-data");
  const speedEl = byIdIn(el, "transfers-speed");
  const activityEl = byIdIn(el, "transfers-activity");
  const refreshEl = byIdIn(el, "transfers-refresh");
  const clearEl = byIdIn(el, "transfers-clear");
  const failureEl = byIdIn(el, "transfers-failure");
  const failureIconEl = byIdIn(el, "transfers-failure-icon");
  const failureTextEl = byIdIn(el, "transfers-failure-text");
  const dismissEl = byIdIn(el, "transfers-dismiss");
  const rowsEl = byIdIn(el, "transfers-rows");
  const loadingEl = byIdIn(el, "transfers-loading");
  const emptyEl = byIdIn(el, "transfers-empty");
  const emptyTextEl = byIdIn(el, "transfers-empty-text");
  const menuEl = byIdIn(el, "transfers-menu");

  // 失败条行首那颗标记（造型照 `exclamationmark.triangle`，颜色由 CSS 的红色
  // currentColor 带下去）。**在挂载时建一次**，不在每次渲染时重建。
  failureIconEl.append(icon("exclamationmark.triangle.fill", "transfers__failure-svg"));

  // 「收起」那颗按钮：**字住在 `index.html` 的 `<template>` 里**（`tpl-notice-dismiss`
  // 带的是它的无障碍名字），本文件只克隆与摆放 —— 与 `verify.js` 同一手法。
  {
    const frag = template("tpl-notice-dismiss");
    const btn = frag.querySelector("button");
    btn.prepend(icon("notice.dismiss", "transfers__dismiss-svg"));
    btn.addEventListener("click", () => {
      // 收起 = "我看到了"。⚠️ 与 `verify.js` 那条不同：这里**两格都清**
      //（动作失败与取列表失败共用这一条），因为使用者按的是同一颗按钮。
      // ⚠️ 清掉之后**下一次 `transfers()` 成功之前它不会自己回来** —— 那是刻意的：
      //    用户说了"我知道了"，就不该被同一句话再拦一次。
      actionFailure = null;
      pollFailure = null;
      paintFailure();
    });
    dismissEl.append(frag);
  }

  // ---------------------------------------------------------------------------
  // 状态（**这一屏自己的**，不往壳上放）
  // ---------------------------------------------------------------------------

  /** 最近一次成功载荷里的行（`TransferRow` 原样，一格都不改）。 */
  let rows = [];
  /** 最近一次成功载荷里的顶部摘要（`TransferGlobalSummary` 原样）。
   *  ⚠️ `null` 是**正常态**（内核那条回执里根本没有 `global` 这一格，见 `api/transfers.rs`）
   *     ⇒ 整条顶部**不渲染**：不编一个"0 B/s / 活动 0"替内核说话。 */
  let global = null;
  /** 空态那句话（`TransferListEmpty::of` 的原文）；`null` = 这一屏此刻不说话。 */
  let emptyText = null;
  /**
   * 侧栏「传输列表」那颗未完成计数徽标的数（载荷里的 `badge_count`，**Rust 算的**）。
   *
   * ⚠️ 它留在这一屏、由 `app.js` 每一拍问一次（`navBadge()`）之后**记在壳上**：
   *    原因是徽标必须在**切走之后还在**（"去那一屏看看"这个提示的全部意义），
   *    而屏的闭包会随 `unmount` 消失。
   */
  let badgeCount = 0;
  /**
   * 有没有**成功拿到过**一份快照。
   *
   * 🔴 它对应 macOS 的 `model.transfers == nil`（`TransfersView.swift:113`），
   *    而"还没有快照"与"确实没有任务"是**两件事**（前者要转圈 + 说一句话，
   *    后者是空态）—— 折成一个的后果是**窗口刚开的那一帧就在说"没有正在传输的任务"**，
   *    而那一刻我们其实什么都还不知道。
   * ⚠️ **失败不清它**（同 macOS：`refreshTransfers` 失败不动 `model.transfers`）——
   *    一次抖动不该把屏幕退回"正在读取"（那会把上一拍的内容也一起抹掉）。
   */
  let snapshotSeen = false;

  /** 「取列表」这条命令的失败 —— **非粘滞**：下一拍成功就自己消失（同 `verify.js`）。
   *  ⚠️ 它**不与** `actionFailure` 合并成一个槽（下面那格有各自的寿命）。 */
  let pollFailure = null;
  /**
   * 「我刚做的那个动作」的失败 —— **粘滞**，直到用户点「收起」或做了下一个动作。
   *
   * 🔴 为什么必须**分两格**（这是一条会毁掉整个动作反馈的判据）：
   *    动作失败（比如「移除」回了一句 `invalid_params`）如果与取列表失败共用一个槽，
   *    那么**下一次成功的 `transfers()`（最迟 200 ms）就会把它清掉** ——
   *    用户看到的是一条一闪而过的红字，等于没有反馈（约束 4 的静默失效）。
   *    macOS 那边 `failure` 是 `@State`，也只在"下一个动作/用户收起"时变
   *    （`TransfersView.run` / `failureBar`），刷新的节拍不碰它。
   */
  let actionFailure = null;

  /** 上一次画出来的**行序列**（结构键）。它没变 ⇒ 一个节点都不动（见 `syncRowDom`）。 */
  let paintedGids = [];
  /** `gid` → 这一行的 DOM 记录（元素引用 + 上一次画上去的那几个"记忆值"）。 */
  const painted = new Map();

  /** 上一次写进失败条正文的那句话（`null` = 这一格现在是空的）。 */
  let shownFailure = null;

  /** 菜单此刻指着哪一行（`null` = 没开着）。那一行没了就把它收起来。 */
  let menuGid = null;

  /** 一次只发一条动作（防重复点击；内核侧是 FIFO 串行队列，见 `poll.js` 的注释）。 */
  let inFlight = false;

  // ---------------------------------------------------------------------------
  // 写值：**只改文本节点，不建节点**（本屏那 200 ms 一拍的承重墙）
  // ---------------------------------------------------------------------------

  /**
   * 把一个值写进**已经建好的那一格**。
   *
   * 🔴 与 `dom.js:setText` 的分工（这不是"抄了第二份"）：
   *    · `setText(el, text)` 是**建行/建块**时用的（"把一个值放进界面"，语义上可以换节点）；
   *    · 本函数是**每一拍**用的：它写的是那个**已经存在的文本节点**的 `nodeValue`
   *      ⇒ 只产生一次 `characterData` 变更，**不产生 `childList` 变更**。
   *    为什么这件事是承重的：`el.textContent = …` 会把原来的文本节点**换成一个新的**，
   *    而"换节点"就是一次 `childList` 变更 —— 用户正拖着的那段选区（错误原文！）
   *    会在这一下里被清掉，200 ms 一次。
   *
   * ⚠️ 只对**只放一个文本节点**的格子用（本屏所有会显示值的格子都是这种：
   *    标题 / 进度 / 百分比 / 速度 / 状态 / 错误原文 / 全局那两格 / 空态那句）。
   *    元素里有别的子节点（比如那颗「⋯」按钮里的 `<svg>`）时**不要**用。
   * ⚠️ 第一拍会建那一个文本节点（`append` 一次，之后一直复用）——所以建行的
   *    `buildRow` 会先把每一格都写一遍空串，把这一次建节点的开销挪到挂载那一刻。
   * ⚠️ 搬运规则与 `setText` 逐字相同：不 trim、不截断、不加省略号、`null`/`undefined`
   *    一律变成**空**（不是字符串 `"null"`）。
   */
  function writeText(target, value) {
    const next = value === null || value === undefined ? "" : String(value);
    const node = target.firstChild;
    if (node !== null && node.nodeType === 3 /* Text */) {
      if (node.nodeValue !== next) node.nodeValue = next;
      return;
    }
    // 这一格还没写过 ⇒ 建那**一个**文本节点（只可能发生在第一次）。
    target.append(document.createTextNode(next));
  }

  /** 把 `RowColor` 的语义档翻成一个类名（见 `COLOR_CLASS`）。 */
  function applyColor(target, color) {
    const wanted = COLOR_CLASS[color] || null;
    for (const name of ALL_COLOR_CLASSES) target.classList.toggle(name, name === wanted);
  }

  // ---------------------------------------------------------------------------
  // 行：建（**只在结构变了时**）与画（**每一拍**）
  // ---------------------------------------------------------------------------

  /**
   * 建一行。**只在"这一行的 `gid` 头一次出现在载荷里"时调一次。**
   *
   * ⚠️ 结构全在 `index.html` 的 `tpl-screen-transfers-row` 里（含那颗「⋯」的
   *    无障碍名字）—— 本函数只克隆、把七个格子找出来、把空文本节点先建好。
   */
  function buildRow(row) {
    const frag = template("tpl-screen-transfers-row");
    const rowEl = frag.querySelector(".transfers__row");
    const rec = {
      el: rowEl,
      iconEl: frag.querySelector(".transfers__icon"),
      // 标题是**两格**（中间省略，见 `paintRow` 里那段）：容器 + 首段 + 尾段。
      titleEl: frag.querySelector(".transfers__title"),
      titleHeadEl: frag.querySelector(".transfers__title-head"),
      titleTailEl: frag.querySelector(".transfers__title-tail"),
      badgeEl: frag.querySelector(".transfers__badge"),
      barEl: frag.querySelector(".transfers__bar"),
      barFillEl: frag.querySelector(".transfers__bar-fill"),
      progressEl: frag.querySelector(".transfers__progress"),
      percentEl: frag.querySelector(".transfers__percent"),
      errorEl: frag.querySelector(".transfers__error"),
      speedEl: frag.querySelector(".transfers__speed"),
      stateEl: frag.querySelector(".transfers__state"),
      menuBtnEl: frag.querySelector(".transfers__menu-btn"),
      row,
      // 上一次画上去的"记忆值"（用来避免"每拍都碰一次 DOM"）
      iconName: undefined,
      titleText: undefined,
      badgeHidden: null,
      errorHidden: null,
      menuDisabled: null,
      /** 这一行此刻允许的动作（`available_actions` 原样；菜单打开时按它建）。 */
      allowed: new Set(),
    };
    rowEl.dataset.gid = row.gid;
    // 行菜单那颗「⋯」的图形（**运行时图形**，住 `dom.js` 的 `ICONS`；名字不是 Rust 给的，
    // 是壳自己的控件 —— 同 `notice.dismiss` 的分工）。
    rec.menuBtnEl.append(icon("ellipsis.circle", "transfers__menu-svg"));
    // 先把每个格子的文本节点建出来（之后每一拍只改 `nodeValue`，见 `writeText`）。
    // ⚠️ 标题是两格（首段 + 尾段），两格都要先建。
    for (const target of [
      rec.titleHeadEl,
      rec.titleTailEl,
      rec.progressEl,
      rec.percentEl,
      rec.errorEl,
      rec.speedEl,
      rec.stateEl,
    ]) {
      writeText(target, "");
    }
    return rec;
  }

  /**
   * 行的**结构**同步：只有 `gid` 序列变了才走这里。
   *
   * ⚠️ **已经画出来的行按 `gid` 复用**（`painted` 那张表）：重建会让那一行的
   *    错误原文选区、它的滚动位置、以及键盘焦点一起丢掉 —— 而这一屏恰恰是为
   *    "看清那条 aria2 原文、把它复制下来"设计的。
   *    只有真的出现了新的一行、或某一行真的没了，才会增删节点。
   */
  function syncRowDom(gids) {
    paintedGids = gids;
    const want = new Set(gids);
    for (const [gid, rec] of painted) {
      if (!want.has(gid)) {
        rec.el.remove();
        painted.delete(gid);
      }
    }
    for (let i = 0; i < rows.length; i += 1) {
      const row = rows[i];
      let rec = painted.get(row.gid);
      if (!rec) {
        rec = buildRow(row);
        painted.set(row.gid, rec);
      }
      // 按载荷的顺序归位（已经在对的位置上就**一个节点都不动**）。
      if (rowsEl.children[i] !== rec.el) rowsEl.insertBefore(rec.el, rowsEl.children[i] || null);
    }
  }

  /**
   * 标题：**中间省略**（macOS `.lineLimit(1)` + `.truncationMode(.middle)`，
   * `TransferRowView.swift:81-84` 逐字）。
   *
   * ## 为什么必须切、以及这为什么不算 §3.2 的拼串
   *
   * `row.title` 是**清单原文路径**：末尾是文件名、中间是目录层级。末尾省略会把
   * **最能区分两行的那一段**（目录）切掉 —— 同一批里 `L01` / `L02` 两行的差距
   * 正好落在中间。而**纯 CSS 做不到真正的中间省略**（`text-overflow: ellipsis`
   * 只画在行尾；那个 `direction: rtl` 的经典手法截的是开头），CSS 的
   * `text-overflow: ellipsis middle` 至今没有任何浏览器实现。
   * ⇒ 与 `verify.js:pathRow` **同一套做法**（那一处的完整论证写在那边的注释里，
   *    这里逐条照办、不另发明）：把字符串按**最后一个分隔符**切成"首段 + 尾段"
   *    两格**紧邻**的 `<span>`，让 CSS 决定首段能放下多少 —— 放不下的部分由
   *    **浏览器**画成省略号。
   *
   * ⚠️ 三条与 `verify.js:pathRow` **逐字相同**的理由（说明它不是"JS 在造要显示的字符串"）：
   *    ① **一个字符都没有被改写**：两格拼起来的文本与内核给的路径**逐字节相同**
   *       （切点是一个下标，不是一次改写）；省略号是浏览器画的，**本文件里没有
   *       `"…"` 这个字面量**；
   *    ② **切点是结构性的、与数据无关**：所有标题都按同一个规则切（最后一个 `/`），
   *       不存在"这条标题该怎么显示"的判断 —— 判断只发生在 CSS 里（放不下就裁）；
   *    ③ **全文两处都在**：`title` 属性给悬停（macOS 的 `.help(row.manifestPath ?? row.title)`）、
   *       **选中复制拿到的也是全文**（两格紧邻，跨格选择得到的就是原来那一整条路径）——
   *       前提是下面那个 `copy` 处理器把浏览器插在块边界上的 `\n` 还原掉。
   *
   * ⚠️ **只在标题真的变了时才写**（`rec.titleText` 那个备忘值）：这一格是 200 ms 一拍
   *    的热路径，而标题在一行的生命周期里通常一个字节都不变 ⇒ 每一拍一个 DOM 操作都不做。
   */
  function paintTitle(rec, row) {
    const title = typeof row.title === "string" ? row.title : "";
    if (rec.titleText === title) return;
    rec.titleText = title;
    // 切点 = **最后一个** `/`（照 macOS 的 `.truncationMode(.middle)` 与 `verify.js:pathRow`）。
    // ⚠️ 没有分隔符（或标题以分隔符结尾）时整条都进首段、尾段为空 —— 退化成**末尾**省略，
    //    那是最坏情况下的兜底（`（未知路径）` 这种占位符就走这一支），不是常态。
    const cut = title.lastIndexOf("/");
    writeText(rec.titleHeadEl, cut >= 0 ? title.slice(0, cut + 1) : title);
    writeText(rec.titleTailEl, cut >= 0 ? title.slice(cut + 1) : "");
    // 悬停给**完整原文**（数据，不是文案）。同一条备忘值管着它 ⇒ 不是每拍一次属性写。
    rec.titleEl.title = title;
  }

  /** 把一行的那 13 格里**会随每一拍变的那几格**写上去。**不建节点。** */
  function paintRow(rec, row) {
    rec.row = row;
    if (rec.el.dataset.gid !== row.gid) rec.el.dataset.gid = row.gid; // 复用是按键来的，正常不变

    // ② 标题（清单原文逐字：`×` 与空格照原样显示，约束 3）
    paintTitle(rec, row);

    // ⑪ 「已暂停」角标：判据只能来自 `raw_status`（裁决 #88），由 Rust 算好在 `shows_paused_badge`
    //    上。字住在模板里，这里只切 `hidden`。
    const badgeHidden = row.shows_paused_badge !== true;
    if (rec.badgeHidden !== badgeHidden) {
      rec.badgeHidden = badgeHidden;
      rec.badgeEl.hidden = badgeHidden;
    }

    // ⑥ 进度：**直接把 Rust 给的那个 0.0–1.0 写进自定义属性**，宽度由 CSS 的
    //    `calc(var(--transfers-fraction) * 100%)` 算 —— JS 连"乘 100 拼一个 %"都不做。
    //    `aria-valuenow` 也写这个数（条本身的 `aria-valuemin/max` 就是 0/1，
    //    见行模板）：读屏用户拿到的百分比由读屏软件按同一个值域算，前端不换算。
    const fraction =
      typeof row.progress_fraction === "number" && Number.isFinite(row.progress_fraction)
        ? row.progress_fraction
        : 0;
    const now = String(fraction);
    rec.barFillEl.style.setProperty("--transfers-fraction", now);
    if (rec.barEl.getAttribute("aria-valuenow") !== now) rec.barEl.setAttribute("aria-valuenow", now);
    // 条上那句可读的值用 `percent_text`（Rust 给的原文，总量为 0 时是「—」）——
    // 不拿 fraction 现算一个百分比（那会把「—」变成「0%」，替内核说了句它没说的话）。
    const valueText = typeof row.percent_text === "string" ? row.percent_text : "";
    if (rec.barEl.getAttribute("aria-valuetext") !== valueText) {
      rec.barEl.setAttribute("aria-valuetext", valueText);
    }

    // ④⑤⑦⑧ 四格文本
    writeText(rec.progressEl, row.progress_text);
    writeText(rec.percentEl, row.percent_text);
    writeText(rec.speedEl, row.speed_text);
    writeText(rec.stateEl, row.state_label);

    // ⑨ 语义色（状态那一格与行首那颗图标）
    applyColor(rec.stateEl, row.state_color);
    applyColor(rec.iconEl, row.state_color);

    // ⑩ 状态图标：**名字变了才重建那枚 `<svg>`**（同 `verify.js` 的 `shownHeadlineIcon`：
    //    每拍重建一个相同的图形，等于每 200 ms 往那一格里塞一个新节点）。
    const name =
      typeof row.state_icon_name === "string" && row.state_icon_name !== "" ? row.state_icon_name : null;
    if (name !== rec.iconName) {
      rec.iconName = name;
      clear(rec.iconEl);
      if (name) rec.iconEl.append(icon(name, "transfers__state-svg"));
    }

    // ⑫ 错误原文：**行下方全文展开**（不截断、不折行、可选中 —— `selectable` 类在模板上，
    //    换行由 CSS 的 `white-space: pre-wrap` 原样保留）。`error_text` 是 `Option`：
    //    `null` ⇒ 那一格 `hidden`（**不是**写一个空串进去：空行会在行与行之间留一条缝）。
    const hasError = typeof row.error_text === "string";
    const errorHidden = !hasError;
    if (rec.errorHidden !== errorHidden) {
      rec.errorHidden = errorHidden;
      rec.errorEl.hidden = errorHidden;
    }
    // ⚠️ 只在**有原文**时写它：没有错误时那一格的文本节点保持上一次的内容没关系
    //    （它整格 `hidden`），但**不能**写空串 —— 那会在"同一句话"上产生一次
    //    `characterData` 变更，把用户正拖着的那段选区按在空串上。
    if (hasError) writeText(rec.errorEl, row.error_text);

    // ⑬ 动作面：这里只**记下来**（菜单是打开时才建的，见 `openMenu`）。
    //    ⚠️ 不在这一拍建菜单节点：那一块是"用户点开才存在"的东西，摆进行里会让
    //       每一行的 DOM 多一棵常驻子树，而它 99.9% 的时间都不可见。
    rec.allowed = new Set(Array.isArray(row.available_actions) ? row.available_actions : []);
    // 「⋯」那颗按钮在"这一行一个动作都给不出"时**禁用** —— 一个点开之后空无一物的
    // 菜单比一颗灰着的按钮更像坏了（约束 4）。判据 = 动作集为空 **且** 没有路径
    //（有路径时「在资源管理器中显示」还在，见 `openMenu`）。
    const menuDisabled = rec.allowed.size === 0 && typeof row.manifest_path !== "string";
    if (rec.menuDisabled !== menuDisabled) {
      rec.menuDisabled = menuDisabled;
      rec.menuBtnEl.disabled = menuDisabled;
    }
  }

  // ---------------------------------------------------------------------------
  // 画（**只读状态、只写 DOM**；不请求、不判断业务）
  // ---------------------------------------------------------------------------

  /**
   * 顶部全局摘要（`TransferGlobalSummary` 的三格）。
   *
   * ⚠️ **整条栏常驻，只有那两格数据随 `global` 显隐**（macOS 的 `summaryBar` 同款：
   *    没有快照时它那两格的位置放一句「读取中…」，两颗按钮照常在）。
   *    `global` 是 `null` 时**不编**"0 B/s / 活动 0"（`api/transfers.rs` 的判据），
   *    也**不把整条栏藏掉** —— 藏掉的话，引擎不可用/还没有快照时那颗「刷新」
   *    会连入口一起消失，而那正是用户唯一能自救的地方。
   */
  function paintGlobal() {
    const has = global !== null && typeof global === "object";
    headerDataEl.hidden = !has;
    if (has) {
      writeText(speedEl, global.speed_text);
      writeText(activityEl, global.activity_text);
    }
    syncActionGates();
  }

  /**
   * 两颗按钮的可用性（「刷新」与「清空已完成」）。
   *
   * ⚠️ 两个判据**都是别人算的**：`can_clear_finished` 来自 `TransferGlobalSummary`
   *    （Rust，有单测：没有已结束的任务时禁用），闸门来自 `ctx.poller.gateOpen`
   *    （`EngineGate::allows_requests` 的执行结果，由 `app.js` 每一拍灌进 poller）。
   *    「刷新」的禁用条件**只有闸门这一条** —— macOS `TransfersView.swift:86` 逐字是
   *    `.disabled(!engineAllowsActions)`（那一屏没有"刷新在飞"这个状态；`pollNow` 那条路
   *    自带重入保护，正有一拍在飞时它跳过，不会多打内核）。
   *    这里只把判据**与**起来，并且**值没变就不碰属性** —— 本函数每一拍（200 ms）
   *    都会被调一次，而无脑写 `disabled` 等于每拍碰两次按钮的 DOM。
   */
  function syncActionGates() {
    const gateOpen = ctx.poller.gateOpen;
    const wantClear = !(global !== null && global.can_clear_finished === true && gateOpen);
    if (clearEl.disabled !== wantClear) clearEl.disabled = wantClear;
    const wantRefresh = !gateOpen;
    if (refreshEl.disabled !== wantRefresh) refreshEl.disabled = wantRefresh;
  }

  /**
   * 列表区那**四态**（顺序照 macOS `TransfersView.content`，**顺序是承重的**）：
   *
   *   ① 有行 ⇒ 列表；
   *   ② 还没有快照 **且** 引擎能发请求 ⇒ 转圈 + 「正在读取传输列表…」；
   *   ③ 确实没有任务（`TransferListEmpty::of` 有话说）⇒ 空态那句话；
   *   ④ 其余（引擎不可用且还没有快照）⇒ **什么都不画**。
   *
   * ⚠️ ②排在③**之前**（macOS 那一支的注释逐字：*还没有快照、但引擎能发请求：轮询的
   *    第一拍马上就到*）：把两者折成一个的后果是**窗口刚开的那一帧就在说
   *    「没有正在传输的任务」**，而那一刻我们其实什么都还不知道。
   * ⚠️ ②必须**与闸门相与**：闸门关着时一个转圈会永远转下去（没有任何请求会返回它）
   *    —— 那正是约束 4 明禁的静默挂死，也正是 macOS 那一支写着 `&& engineAllowsActions`
   *    的原因（原因与出口都在顶部那条常驻横幅上）。
   * ⚠️ ④那一档**不许兜底编一句**（同 `TransferListEmpty::of` 的判据）。
   */
  function paintPhase() {
    const hasRows = rows.length > 0;
    const showLoading = !hasRows && !snapshotSeen && ctx.poller.gateOpen;
    const showEmpty = !hasRows && !showLoading && typeof emptyText === "string";
    rowsEl.hidden = !hasRows;
    loadingEl.hidden = !showLoading;
    emptyEl.hidden = !showEmpty;
    if (showEmpty) writeText(emptyTextEl, emptyText);
  }

  /** 失败条：两格来源（`actionFailure` 优先），同一处落点。 */
  function paintFailure() {
    const message = actionFailure !== null ? actionFailure : pollFailure;
    if (message === null) {
      if (shownFailure !== null) {
        writeText(failureTextEl, "");
        shownFailure = null;
      }
      failureEl.hidden = true;
      return;
    }
    // 🔴 **同一句话不重写第二遍**：这一格是可选中复制的，而它被 200 ms 的节拍喂 ——
    //    无脑重写会换掉那个文本节点 ⇒ 客户正拖着选区准备复制的那段原文被清掉。
    //    （`writeText` 已经把这一步拦在 `nodeValue` 那一层，这里再拦一道是为了
    //      "值没变就什么都不做"，与 `verify.js:showFailure` 同款。）
    if (shownFailure !== message) {
      writeText(failureTextEl, message);
      shownFailure = message;
    }
    failureEl.hidden = false;
  }

  /** 把这一拍的状态整块画出来。 */
  function paint() {
    paintGlobal();
    // ① 结构：只有 `gid` 序列变了才动 DOM
    const gids = rows.map((r) => r.gid);
    if (gids.length !== paintedGids.length || gids.some((g, i) => g !== paintedGids[i])) {
      syncRowDom(gids);
    }
    // ② 文本与宽度：每一拍都跑，一个节点都不建
    for (const row of rows) {
      const rec = painted.get(row.gid);
      if (rec) paintRow(rec, row);
    }
    // ③ 菜单指着的那一行没了就把它收起来（收起的动作本身不做任何业务）
    if (menuGid !== null && !painted.has(menuGid)) closeMenu();
    paintPhase();
    paintFailure();
  }

  // ---------------------------------------------------------------------------
  // 行菜单（**打开时才建**；建哪几项由载荷决定）
  // ---------------------------------------------------------------------------

  /**
   * 按**载荷**建这一行的菜单。
   *
   * 🔴 **动作面只有一个来源：`available_actions`。**
   *    本函数里**没有**"某状态下该有哪几个动作"的判断：载荷里有的就建、没有的不建
   *    —— **空集 = 一个动作都不给**（`TransferRow::actions` 的注释："给一处注定报错的
   *    入口比没有入口更糟"，`a_removed_row_offers_nothing_at_all` 钉着）。
   *    菜单里那六个名字住在 `index.html` 的 `tpl-screen-transfers-menu` 里，
   *    它们的 `data-action` 就是**线上字面量**（`protocol.rs` 钉着）—— 于是
   *    "名字"与"发出去的那个动作"永远对得上，而前端不知道任何状态与动作的对应关系。
   *
   * ⚠️ **「在资源管理器中显示」是另一个判据**（macOS `TransferRowView` 同款）：
   *    它不是 `TaskAction`（协议里没有 `reveal`，规格 §5.2 —— 那是壳的事），
   *    准入判据是 `TransferRow::can_reveal_in_finder()`，而那条判据**就是**
   *    "`manifest_path` 有没有值"（那一格已经做过归一化：`null` 与空串都是 `None`）。
   *    ⇒ 这一项出不出现看 `manifest_path`，**不看** `available_actions`
   *      （macOS 也是这么分的：`canAct(_:)` 与 `row.canRevealInFinder` 两个判据）。
   */
  function buildMenu(rec) {
    const frag = template("tpl-screen-transfers-menu");
    const gate = ctx.poller.gateOpen;
    const row = rec.row || {};
    const hasPath = typeof row.manifest_path === "string";
    for (const item of [...frag.querySelectorAll(".transfers__menu-item")]) {
      const key = item.dataset.action;
      const isShell = item.dataset.kind === "shell";
      if (isShell ? !hasPath : !rec.allowed.has(key)) {
        item.remove();
        continue;
      }
      if (isShell) {
        item.addEventListener("click", () => {
          closeMenu();
          void sendReveal(row.manifest_path);
        });
      } else {
        // 闸门关着时那一项**禁用而不是消失**（macOS 同款：`.disabled(!canAct(...))`）——
        // 动作面本身仍然完全由载荷说了算。
        item.disabled = !gate;
        item.addEventListener("click", () => {
          closeMenu();
          void sendAction(key, row.gid);
        });
      }
    }
    tidySeparators(frag);
    return frag;
  }

  /** 去掉被删空之后剩下的、孤零零的分隔线（前一项或后一项没了，它就只是一条多余的横线）。 */
  function tidySeparators(frag) {
    for (const sep of [...frag.querySelectorAll(".transfers__menu-sep")]) {
      while (sep.previousElementSibling && sep.previousElementSibling.classList.contains("transfers__menu-sep")) {
        sep.previousElementSibling.remove();
      }
      if (!sep.previousElementSibling || !sep.nextElementSibling) sep.remove();
    }
  }

  /** 开/关这一行的菜单（同一次点击点第二下 = 收起来）。 */
  function toggleMenu(gid, x, y) {
    if (menuGid === gid && !menuEl.hidden) {
      closeMenu();
      return;
    }
    openMenu(gid, x, y);
  }

  function openMenu(gid, x, y) {
    const rec = painted.get(gid);
    if (!rec) return;
    const frag = buildMenu(rec);
    // ⚠️ 一个动作都给不出 ⇒ **菜单根本不弹**（空集 = 一个都不给）。
    //    弹一个空盒子比不弹更像坏了；而"这一行没有动作"这件事已经由那颗「⋯」
    //    的**禁用态**说清楚了。
    if (frag.querySelector(".transfers__menu-item") === null) {
      closeMenu();
      return;
    }
    clear(menuEl);
    menuEl.append(frag);
    menuGid = gid;
    // ⚠️ 先显示再量尺寸：`hidden` 的元素量出来是 0（`display:none`）。
    menuEl.hidden = false;
    const rect = menuEl.getBoundingClientRect();
    const rootRect = root.getBoundingClientRect();
    // 贴着光标/那颗按钮，但**不许超出这一屏**（超出去的部分点不到，等于菜单项少了一半）。
    const left = Math.max(0, Math.min(x - rootRect.left, rootRect.width - rect.width));
    const top = Math.max(0, Math.min(y - rootRect.top, rootRect.height - rect.height));
    menuEl.style.left = `${left}px`;
    menuEl.style.top = `${top}px`;
  }

  function closeMenu() {
    menuEl.hidden = true;
    menuGid = null;
  }

  // ---------------------------------------------------------------------------
  // 动作（全部由用户动作触发；`render` 一个都不调）
  // ---------------------------------------------------------------------------

  /**
   * 发一条 `task_action`。
   *
   * ⚠️ `gid` 是**这一行的 `gid` 原文**（载荷给的）；`clear_finished` 那一支发 `null`
   *    （列表级动作没有 gid，`TransferRow::actions` 的注释里写着为什么它不在行级）。
   *
   * ⚠️ 成功后**立刻补一拍**（`ctx.poller.pollNow()`）：移除/清空之后要能**看见行消失**，
   *    否则用户会以为没生效（约束 4）。这与 macOS 那句 `await model.refreshTransfers()`
   *    是同一件事 —— 用 `pollNow` 而**不是**自己调 `tick()`：那条路自带重入保护与闸门
   *    （`poll.js` 的既有语义），自己调会绕过两道防线。
   */
  async function sendAction(action, gid) {
    if (inFlight) return;
    inFlight = true;
    try {
      await ctx.call(ctx.CMD.taskAction, { action, gid });
      actionFailure = null;
    } catch (error) {
      // 失败**就地**显示内核原文（约束 3），不切走、不加工 —— 走 `failureText` 取，
      // **不要**写 `error.message`（`invoke` 的拒绝可能是裸字符串）。
      actionFailure = failureText(error);
    } finally {
      inFlight = false;
      paintFailure();
      ctx.poller.pollNow();
    }
  }

  /**
   * 「在资源管理器中显示」（`reveal`）。
   *
   * ⚠️ 实参是**清单相对路径**（`manifest_path` 那一格的内核原文），**不是**磁盘路径：
   *    换算成落盘路径只有一处实现（`shell_core::api::reveal::local_path`），壳不拼路径。
   * ⚠️ 它**不补一拍**：这件事没有经过内核（规格 §5.2），内核里的任务一个字节都没变，
   *    补一拍只是白打一次 kernel（200 ms 的节拍马上也会来）。
   */
  async function sendReveal(path) {
    try {
      await ctx.call(ctx.CMD.reveal, { path });
      actionFailure = null;
    } catch (error) {
      actionFailure = failureText(error);
    } finally {
      paintFailure();
    }
  }

  // ---------------------------------------------------------------------------
  // 事件
  // ---------------------------------------------------------------------------

  // ---- 行：事件委托（行是按 `gid` 复用/增删的，逐行挂监听会在每次增删时
  //      留下一批指向已丢弃节点的闭包）------------------------------------------------
  rowsEl.addEventListener("click", (event) => {
    const btn = event.target.closest(".transfers__menu-btn");
    if (!btn || !rowsEl.contains(btn)) return;
    const rowEl = btn.closest(".transfers__row");
    if (!rowEl) return;
    const rect = btn.getBoundingClientRect();
    toggleMenu(rowEl.dataset.gid, rect.left, rect.bottom + 4);
  });

  // 右键落在某一行上 ⇒ 弹**那一行**的菜单（macOS 的 `.contextMenu` 与那颗 Menu 是
  // "同一份菜单挂两份"）。落在空白处 ⇒ 没有菜单可弹（这一屏没有列表级的行菜单）。
  rowsEl.addEventListener("contextmenu", (event) => {
    const rowEl = event.target.closest(".transfers__row");
    if (!rowEl) return; // 空白处：让浏览器的默认菜单出来（这一屏没有"对着空白处的动作"）
    event.preventDefault();
    toggleMenu(rowEl.dataset.gid, event.clientX, event.clientY);
  });

  /**
   * 复制标题时，把**浏览器插在块边界上的那个换行**还原掉。
   *
   * 🔴 这是"两格切分"（中间省略，见 `paintTitle`）的**代价**，与 `verify.js:restoreVerbatimOnCopy`
   *    是**同一件事、同一套做法**：标题被切成两格**紧邻**的 `<span>`，而两格都是
   *    **块级盒子**（flex 子项会被 blockify）⇒ **浏览器在序列化跨块边界的那段选区时，
   *    会在边界处插一个 `\n`**。⇒ 客户拖选一条路径复制时，剪贴板里会多一个换行
   *    （macOS 那边是单个文本节点，没有这件事）。
   *
   * ⚠️ **保守到底**（照 `verify.js` 那条）：只在真的命中"我切的那两格 + 那个换行"时才介入
   *    （`clean === text` 就**原样放行**，连 `preventDefault` 都不调）——
   *    用户自己选区里的换行、别的元素之间的换行，一个都不碰。
   * ⚠️ 它**不新增、不改写、也不删掉内核给的任何一个字符**：删掉的是**浏览器自己插进去的
   *    那一个 `\n`**，得到的字符串与 `row.title` **逐字节相同**。
   * ⚠️ 错误原文那一格**不受影响**（它是单个文本节点，没有块边界可言）——
   *    所以这个处理器的管辖面就是标题那两格。
   */
  function restoreVerbatimOnCopy(event) {
    const clipboard = event.clipboardData;
    const selection = document.getSelection();
    if (!clipboard || !selection || selection.isCollapsed) return;
    const text = selection.toString();
    let clean = text;
    for (const titleEl of rowsEl.querySelectorAll(".transfers__title")) {
      const head = titleEl.querySelector(".transfers__title-head").textContent;
      const tail = titleEl.querySelector(".transfers__title-tail").textContent;
      // 只有"两格都在"的那些标题才可能出现那个边界（空的尾段没有边界可言）
      if (head === "" || tail === "") continue;
      clean = clean.split(`${head}\n${tail}`).join(head + tail);
    }
    if (clean === text) return; // 没命中 ⇒ 一个字符都不碰
    clipboard.setData("text/plain", clean);
    event.preventDefault();
  }

  rowsEl.addEventListener("copy", restoreVerbatimOnCopy);

  // 「刷新」：**立刻取一次传输列表快照**（macOS `TransfersView.swift:79-81` 那颗）。
  //
  // ⚠️ 用 `pollNow` 而**不是**自己调一次 `tick()`：`pollNow` 走的是节拍器那条路，
  //    天生带**重入保护**（正有一拍在飞时跳过）与**闸门**（引擎不可用时一拍都不发）。
  //    自己调 `tick()` 会绕过这两条，而绕过闸门正是 macOS 注释里点名的
  //    "结构性防线"（引擎不可用时一个请求都不发）。
  // ⚠️ 本屏**没有**"刷新在飞"这个状态（macOS 那一屏也没有）：200 ms 一拍的节拍本身
  //    就在不停地取，多一个状态只会多一处会与节拍抢的东西。
  refreshEl.addEventListener("click", () => {
    ctx.poller.pollNow();
  });

  // 「清空已完成」：列表级动作（没有 gid）。
  clearEl.addEventListener("click", () => {
    void sendAction(ACTION_CLEAR_FINISHED, null);
  });

  // 点别处 / 按 Esc 关掉行菜单（**全局监听**，`unmount` 时要解绑）。
  const onDocumentPointerDown = (event) => {
    if (menuEl.hidden) return;
    if (root.contains(event.target)) return;
    closeMenu();
  };
  const onDocumentKeyDown = (event) => {
    if (event.key === "Escape") closeMenu();
  };
  document.addEventListener("pointerdown", onDocumentPointerDown);
  document.addEventListener("keydown", onDocumentKeyDown);

  // ---------------------------------------------------------------------------
  // 这一屏的那一拍（200 ms，只在挂载期间跑）
  // ---------------------------------------------------------------------------

  /**
   * 一拍：取一次 `transfers()`。
   *
   * ⚠️ **失败不清掉上一拍的内容**（同 `verify.js` / macOS）：一次抖动不该让整屏消失。
   *    失败落在**本屏的失败条**上（屏不许调壳级的 setter，见 `registry.js` 契约第 1 条）
   *    —— 与动作失败共用那一格，但**各有各的寿命**（见 `actionFailure` 那段）。
   */
  async function tick() {
    let data;
    try {
      data = await ctx.call(ctx.CMD.transfers);
    } catch (error) {
      pollFailure = failureText(error);
      paint();
      return;
    }
    // 成功 ⇒ 清掉"取列表失败"那一格（**非粘滞**：下一次成功就自己消失）。
    // ⚠️ 但**不碰 `actionFailure`**：那是用户刚做的那个动作的回执，得他自己收起。
    pollFailure = null;
    // 拿到了第一份快照 ⇒ "正在读取"那一档从此不再出现（**它一次都不清回去**：
    // 后面某一拍失败时 `rows` 会保持上一拍的样子，见 `snapshotSeen` 那段）。
    snapshotSeen = true;
    // 侧栏「传输列表」那颗徽标的计数：**Rust 算好随这一格载荷下来**
    //（`api::transfers::transfers` 的 `badge_count` ← `SidebarBadge::unfinished_of_rows`）。
    // ⚠️ **本屏一个数都不自己数**（规格 §3.2，"哪些算未落定"的判据在 `presentation`）。
    //    取不到时留 `0`（"没有数据"不等于"有一堆没落定"）—— 与 `SidebarBadge` 对
    //    `None` 的处置同一条。
    badgeCount = data && typeof data.badge_count === "number" ? data.badge_count : 0;
    rows = data && Array.isArray(data.rows) ? data.rows : [];
    // ⚠️ `global` 与 `rows: []` 是两件事（`api/transfers.rs` 的判据）：空列表是**事实**，
    //    而 `global` 缺是**内核这条回执里根本没有那个字段** ⇒ 原样收下 `null`，
    //    不当成失败、更不编三个 0。
    global = data && data.global ? data.global : null;
    emptyText = data && typeof data.empty_text === "string" ? data.empty_text : null;
    paint();
  }

  ctx.poller.start(TICK, INTERVALS_MS.transfers, tick);

  return {
    /**
     * 壳每一拍把 `state().load` 交进来。
     *
     * ⚠️ **本屏的显示值一个都不来自这里**：行 / 摘要 / 空态句全在 `transfers()` 自己的
     *    载荷里（那是 200 ms 的一拍）。这一格里唯一与本屏有关的东西是**闸门**
     *    （`allows_requests`）—— 而它不是从参数来的，是 `app.js` 每一拍灌进
     *    `ctx.poller` 的；`render` 是屏能看见"壳刚灌过"的**唯一**时机
     *    ⇒ 在这里把动作面的可用性重同步一次（同 `verify.js` 对那颗「刷新」做的事）。
     *
     * ⚠️ **本函数不发请求、不建事件、不动载荷**（契约逐字）：它只做"值 → 属性"的同步。
     */
    render(load) {
      // ⚠️ 参数收下但不用（上面那段说了为什么）—— 留着是为了让下一个读者看得见
      //    "我确实拿到了它，而且是有意不用"。
      void load;
      syncActionGates();
      // ⚠️ **"还没有快照"那一档也要跟着闸门重画**：闸门是**壳每一拍灌进 poller** 的，
      //    而 `render` 是屏能看见"壳刚灌过"的唯一时机 —— 少了这一句，闸门从开到关时
      //    那个转圈会一直转下去（macOS 那一支的 `&& engineAllowsActions` 就是这个判据）。
      paintPhase();
    },
    /**
     * **侧栏「传输列表」那颗徽标的数**（`registry.js` 契约里的可选那一格）。
     *
     * 🔴 屏为什么必须回答它：这个数只有**这一屏的载荷**里有（`badge_count`），
     *    而徽标长在**壳**上、要活过换屏 ⇒ 由 `app.js` 每拍问一次再喂给壳。
     *    ⚠️ 本函数**不数任何东西**（规格 §3.2）：它把载荷里那一格原样交出去。
     *    ⚠️ `0` = 这一格不渲染（macOS 的 `.badge(0)`）—— 与 `SidebarBadge` 给 0 同义。
     */
    navBadge() {
      return { section: "transfers", count: badgeCount };
    },
    unmount() {
      // 🔴 停掉本屏的节拍（契约第 3 条：屏自己的节拍不许在切走之后继续打内核）。
      //    ⚠️ 只停**自己起的**那一条：`state` 是壳的（它不属于任何一屏）。
      ctx.poller.stop(TICK);
      document.removeEventListener("pointerdown", onDocumentPointerDown);
      document.removeEventListener("keydown", onDocumentKeyDown);
      clear(el);
    },
  };
}
