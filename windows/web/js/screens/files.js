// files.js —— **文件页**：面包屑 + 四列表格 + 勾选 + 右键菜单 + 底栏。
//
// 上游：`macos/Sources/BenagenDownloader/Views/FileBrowser.swift` 与 `SelectionBar.swift`
// （逐条对位：四列的宽度与对齐、四态的呈现、底栏那句话、右键菜单那三项）。
//
// ⚠️ **本文件里一个面向用户的字都没有**（规格 §3.2）。这一屏会显示的字只有两个来源：
//    ① **数据** —— `BrowserRow` 的 `name` / `detail_text` / `source_time_text` /
//       `state.label` / `state.color` / `icon_name`、`Breadcrumb.segment.name`、
//       `SelectionSummary` 的两句、`DownloadAction` 的两句、`DirLoadFailure` 的三格、
//       `EnqueueFeedback` 的 `summary` 与 `rejections`。**逐字落位，不 trim、
//       不折叠、不在空值上兜一句话**；
//    ② **结构文案** —— 与数据无关、任何状态下都一样的那些字（列名、按钮名、状态句）。
//       它们住在 `index.html` 的 `tpl-screen-files` 里，本文件只**克隆与摆放**
//       （`dom.js:template()`），一个字都不拼。
//    ⇒ 于是"界面文案自动与 macOS 一致"这件事在前端不需要任何人工维护：
//      文案全部来自 `presentation/` 那 232 条单测钉着的地方。
//
// ⚠️ 三条不许越过的线（`js/screens/registry.js` 的文件头）：
//    ① 不碰壳（`#nav` / `.toolbar` / `#notices` / `#batch-summary`）—— 本文件只往
//       自己的宿主 `el` 里放东西；
//    ② 不自己 `invoke` —— 一律走 `ctx.call`（唯一的后端接触面）；
//    ③ 不起自己的定时器 —— 本屏那一拍挂在 `ctx.poller` 上（见 `batchWatch`）。
//
// ⚠️ 它是**屏**，不是壳的一部分：`.files` 这一棵子树在换屏时随宿主一起消失，
//    所以这里不许出现任何"切到别的屏还该在"的 **DOM**。
//
// 🔴 **例外只有三格，而且它们是承重的**（`kept`，见下面那段）：**当前层、勾选面、
//    "手上的数据属于哪一批"**。它们**必须**活过一次换屏 —— 切分区**不是**换批次
//    （macOS 侧把这三格放在 `RootView` 而不是 `FileBrowser` 的 `@State` 里，
//    理由逐字写在 `FileBrowser.swift` 顶上那段：留在视图里的话，
//    "用户跨目录攒的选择与当前所在的目录被**无声地**打回 `default_selected` + 根目录"）。
//    ⚠️ 在真机上那条"无声地打回"就是：**选一个文件 → 去传输列表看一眼 → 切回来 →
//    点「下载选中」⇒ 下的是整批** —— 因为打回去的那个面是内核给的 `default_selected`
//    （"所有还没下完的文件"，一批全新的交付里**就是整批**），而它与整批**相等**时
//    `DownloadTargets::paths` 会收敛成 `paths: []`（= 内核语义的"下全部待下载"）。

import { byIdIn, clear, h, icon, setText, template } from "../dom.js";
import { INTERVALS_MS } from "../poll.js";
import { failureText } from "../invoke.js";

export const id = "files";

/** 判别键的**线上形态**：`BrowserRowKind::Dir` 经 `Serialize` 派生出来就是 `"Dir"`。
 *
 *  🔴 **它不是 `"dir"`** —— 这一格的名字曾经写错过一次，而后果是**双击目录变成
 *     「下载整个目录」**（判不出目录 ⇒ 落到入队那一支），界面上没有任何东西变红。
 *     `"Dir"` / `"File"` 这两个字符串由 shell-core 的
 *     `api::tree::tests::the_wire_forms_of_kind_and_color_are_pinned` **逐字钉着**
 *     （内核自己的 `"type"` 是小写的 `dir`/`file`，那是**内核那一侧**的形态，
 *     与壳发出去的这一格不是同一个字符串 —— 别把两者混起来看）。 */
const KIND_DIR = "Dir";

/** `RowColor`（五个成员，`presentation/mod.rs`）→ 本屏状态列的类名后缀。
 *
 * ⚠️ **这张表是"语义 → 画法"的那一步**（同 `dom.js` 的 `ICONS`）：名字是 Rust 给的
 *    （载荷里的 `state_color` ← `RowStateStyle::color()`，有单测钉着"四态两两不同"），
 *    这里只决定它画成哪个色。
 *    ⚠️ **五个成员一个都不能少**（`RowColor` 不是 6 个成员那笔账记在
 *    `presentation/mod.rs` 里）；取不到值时**不给类名** ⇒ 那一格退回继承色 ——
 *    而不是"随便挑一个颜色"（挑错就是"两种状态画成一样"，约束 4 明禁）。
 *    ⚠️ 反过来，这里**不许**按 `state_label` 去猜颜色：那是把 `RowStateStyle`
 *    的两条判据（label 与 color）在前端重新配一次对，Rust 改了映射不会有东西变红。
 *    ⚠️ 键是 `RowColor` 的**变体名**（`Serialize` 派生出来的那五个字符串），
 *    由 `api::tree::tests::the_wire_forms_of_kind_and_color_are_pinned` 钉着。 */
const ROW_COLOR_CLASS = Object.freeze({
  Secondary: "is-secondary",
  Blue: "is-blue",
  Green: "is-green",
  Red: "is-red",
  Orange: "is-orange",
});

/** 目录行的 `icon_name`（`BrowserRow::of` 只给两个名字之一）。
 *  它只用来决定**名称列那个图标要不要用 accent**（macOS `.foregroundStyle(.accentColor)`）——
 *  "该用哪个图形"仍然只由 `icon_name` 说了算（`dom.js:icon`）。 */
const ICON_DIR = "folder";

/** 本屏那一拍的任务名（`poller.start` / `poller.stop` 用的是同一个键）。
 *  用命令名当键：`poll.js` 的节拍表就是这么索引的，两处对不上时 `stop` 会静默不生效。 */
const TICK = "tree";

// ---------------------------------------------------------------------------
// 🔴 **活过一次挂载的那三格**（真机上"点「下载选中」却下了整批"那个缺陷的落点）
// ---------------------------------------------------------------------------
//
// 它们**不属于一次挂载**：`mount` 的闭包在换屏时会随宿主一起丢掉，而**换屏不是换批**。
//
// ## 为什么必须在这里（而不是 `mount` 里那几个 `let`）
//
// 本屏只在**一个分区**里可见。切到「传输列表」再切回来 ⇒ `app.js:applyRoute` 会
// `unmount()` + `mount()` 一次 ⇒ 挂载闭包里的东西**全部重建**。修之前这三格就在那里，
// 于是"选一个文件 → 去传输列表看一眼 → 切回来"的后果是：
//   · **勾选面被重播成内核给的 `default_selected`**（"所有还没下完的文件" ——
//     一批全新的交付里**就是整批**）；
//   · **当前目录被退回根**；
//   · 而它与整批**相等**时，`DownloadTargets::paths` 会收敛成 `paths: []`
//     （内核语义 = 下全部待下载）⇒ **用户只选了一个文件，下的是整批**。
//
// ## 上游是怎么做的（这不是"我们的一种设计选择"，是**漏了一次移植**）
//
// macOS 侧的这三格（`currentPath` / `selection` / `ManifestTracking`）
// **由 `RootView` 持有、以 `@Binding` 传给 `FileBrowser`**
// （`macos/Sources/BenagenDownloader/Views/RootView.swift` 那一段 `@State` 的注释）。
// 那里的理由逐字是：
//
//   > `mainArea` 是按 `section` 分派的 `switch` —— 切到「传输列表」再切回来，
//   > `FileBrowser` 会被重新构造、它的 `@State` 跟着重建。而**切分区不是换批次**：
//   > 简报只允许"换交付码时清空选择"，可"选完文件去看一眼传输列表再回来"恰恰是这个
//   > 应用的主流程。留在视图的 `@State` 里的后果是：用户跨目录攒的选择与当前所在的
//   > 目录被**无声地**打回 `default_selected` + 根目录。
//
// web 这一代没有"上一层视图"可以挂（`ctx` 是**只读**的四样东西，屏不许调壳级 setter
// —— `registry.js` 的契约），而 ES 模块**只求值一次** ⇒ 模块级的这几格就是
// "活得比一次挂载久"的那个落点，与 `RootView` 在那里扮演的角色**逐字同义**。
//
// ## ⚠️ 复位仍然只发生在一处：**换批**
//
// `reseed`（`batchWatch` 在 `observedCode !== kept.loaded` 时调它）把这三格一起复位
// —— 那正是"换交付码时清空选择"这一条。**换屏不走它**。
//
// ⚠️ 与 macOS 的一处**已知差异（如实记账）**：macOS 的 `RootView` 一直在世，
// 所以"离开期间换了批"那一刻它就能复位；本代的 `files` 屏在别的分区里**没有挂载**，
// 看不到 `state()` 的节拍 ⇒ 只能等切回来之后再复位（最长一拍，1 s）。
// 窗口里那一小段的表现是"先按旧位置读一次、随后复位"——**结局正确**（换批一定复位），
// 代价是一次可能落空的 `tree(path)`。收口要一条"屏不在场也能收到换批"的通道，那是另一件事。
const kept = {
  /** 当前层（清单**原文**路径；根是空串）。壳不规范化它（约束 3）。 */
  path: "",
  /** 勾选面：**路径**的集合（跨目录多选，进目录时不清空 —— macOS 同款）。 */
  selection: new Set(),
  /** **这一屏手上的数据**属于哪一批（`undefined` = 还一次都没拿到过 `state()` 的结论）。
   *  与 `observedCode`（`render` 记下来的那一批）不同 ⇒ 欠一次复位 + 重读。 */
  loaded: undefined,
};

/**
 * 挂载。
 *
 * @param {HTMLElement} el 壳给的宿主（`#page` 里的一个 `<div class="screen">`）
 * @param {{call: Function, CMD: object, poller: object, shell: object}} ctx
 *   见 `registry.js` 的接口契约（**只有那几样**）。
 */
export function mount(el, ctx) {
  // 结构（含结构文案）全部从 `index.html` 的模板克隆 —— 本文件一个字都不造。
  el.append(template("tpl-screen-files"));
  // ⚠️ 右键菜单挂进 `.files` 里面（不是挂到 `el` 上）：它是 `position: absolute` 的，
  //    要相对**这一屏**定位；挂到外面就会相对更上一层的定位祖先跑掉。
  const root = byIdIn(el, "files-root");
  root.append(template("tpl-screen-files-menu"));

  // ⚠️ 一律 `byIdIn(el, …)`（**不是** `byId`）：挂载这一刻 `el` 还在文档外
  //    （换屏是"先挂好新的、再拆旧的"，见 `app.js:applyRoute`），
  //    而 `document.getElementById` 看不见文档外的节点。理由写在 `dom.js` 里。
  const crumbsEl = byIdIn(el, "files-crumbs");
  const downloadDirEl = byIdIn(el, "files-download-dir");
  const reloadEl = byIdIn(el, "files-reload");
  const failureEl = byIdIn(el, "files-failure");
  const failureNoticeEl = byIdIn(el, "files-failure-notice");
  const failureTextEl = byIdIn(el, "files-failure-text");
  const failureDismissEl = byIdIn(el, "files-failure-dismiss");
  const rowsEl = byIdIn(el, "files-rows");
  const headEl = byIdIn(el, "files-head");
  const statusEl = byIdIn(el, "files-status");
  const statusSpinnerEl = byIdIn(el, "files-status-spinner");
  const statusLoadingEl = byIdIn(el, "files-status-loading");
  const statusFailedEl = byIdIn(el, "files-status-failed");
  const statusEmptyEl = byIdIn(el, "files-status-empty");
  const statusRetryEl = byIdIn(el, "files-status-retry");
  const noticeEl = byIdIn(el, "files-notice");
  const noticeTextEl = byIdIn(el, "files-notice-text");
  const noticeListEl = byIdIn(el, "files-notice-list");
  const hintEl = byIdIn(el, "files-bar-hint");
  const hintSepEl = byIdIn(el, "files-bar-hint-sep");
  const countEl = byIdIn(el, "files-bar-count");
  const sizeEl = byIdIn(el, "files-bar-size");
  const selectAllEl = byIdIn(el, "files-bar-select-all");
  const selectNoneEl = byIdIn(el, "files-bar-select-none");
  const downloadEl = byIdIn(el, "files-bar-download");
  const menuEl = byIdIn(el, "files-menu");
  const menuDownloadEl = byIdIn(el, "files-menu-download");
  const menuSelectLevelEl = byIdIn(el, "files-menu-select-level");
  const menuSelectNoneEl = byIdIn(el, "files-menu-select-none");

  // ---------------------------------------------------------------------------
  // 状态（**这一屏自己的**，不往壳上放）
  // ---------------------------------------------------------------------------

  /** 当前层的行（`BrowserRow` 原样，一格都不改）。 */
  let rows = [];
  /** 上一次成功载荷里的层级链（`Breadcrumb.segments`，**载荷原样**）。
   *  ⚠️ 段落不是本文件按 "/" 切的：切分规则在 `Breadcrumb::new` 里（空段不折叠、根是空串），
   *    抄一份到前端就等于把同一条判据写出两个会分叉的答案。 */
  let segments = [];
  /** 一次 `tree(path)` 的生命周期：`loading` / `loaded` / `failed`。 */
  let phase = "loading";
  /** 上一次读这一层失败的说明（`DirLoadFailure` 原样，含"退回哪一层"）。 */
  let failure = null;
  /** 整棵树的勾选摘要与底栏那个动作（`tree()` 无 `path` 那一支的载荷，两格原样）。 */
  let summary = null;
  let action = null;
  /** 上一次 `enqueue` 的回执（`EnqueueFeedback` 原样；`null` = 那一块不显示）。 */
  let feedback = null;

  /** `render()` 记下来的那一批（`undefined` = 还一次都没拿到过 `state()` 的结论）。
   *  ⚠️ 它只是一个**账**：`render` 把载荷里那个码记在这里，**不发任何请求**
   *     （契约：`render` 只做"载荷 → DOM"）。 */
  let observedCode;
  /** 上一次**画进面包屑根那一格**的批次号。
   *
   *  🔴 它是**必需的**，不是缓存优化：`render` 每一拍（1 s）都会被喂一次，
   *     而无脑重画面包屑的后果是**每秒把那一行 DOM 重建一遍** ——
   *     正在悬停的那一段会闪、键盘焦点会丢、拖着选的文字会被清掉。
   *     同 `shell.setNotices` 的内容签名（那里也是这个理由）。
   *     ⇒ 只有**码真的变了**才重画。 */
  let paintedCode;
  /** 一次只发一条（约束 15 的单飞语义在内核侧，这里只是防重复点击/重复那一拍）。 */
  let inFlight = false;
  /**
   * 🔴 **首挂那一次「读整棵树 + 读根」正在飞** —— `batchWatch` 靠它认出
   * "这一批**正在被读**"，从而**不在同一时刻再读一遍**。
   *
   * 为什么必须有它（这是一条**确定性**的缺陷，不是时序边界）：`mount` 末尾那个 IIFE
   * 与 `batchWatch` 的第一拍**天生同时开工** ——
   *   · `poll.js:start` 的"立刻跑第一拍"是 `Promise.resolve().then(job.run)`（**微任务**）；
   *   · 而 `app.js:applyRoute` 在 `mount()` 返回后**同步**跑 `render(loadState)` ⇒
   *     那个微任务真正跑起来时 `observedCode` **已经有值**，而 `kept.loaded` 还是
   *     `undefined`（IIFE 刚发出请求、还没回来）⇒ `batchWatch` 无条件 `reseed()`。
   *   · 后果实测过（无头 Chromium，2026-09-20 的整分支审查）：挂上后 55 ms 内发出的是
   *     `["state", "tree", "tree"]`，**两条 `tree` 都没有 `path`** —— 整棵树读了两遍
   *     （最重的那一条命令），而且 `reseed` 把 `action` 置空的那一段里，底栏那颗主按钮
   *     是**空文案 + 可点**（点了没有任何反馈）。
   * ⇒ 首挂交给那个 IIFE，`batchWatch` 在它落地之前**一拍都不许插**。
   * ⚠️ 它**不是** `kept.loaded !== undefined` 那种判据：那个会连带改掉
   *   "`render` 还没跑过时留 `undefined`、下一拍补复位"那条既有语义（见 IIFE 末尾那段）。
   */
  let mountingRead = true;

  // ---------------------------------------------------------------------------
  // 画（**只读状态、只写 DOM**；不请求、不判断业务）
  // ---------------------------------------------------------------------------

  /** 面包屑：根那一格是**批次号**（macOS `FileBrowser.swift:103`），后面跟着每一段。 */
  function paintCrumbs() {
    paintedCode = observedCode;
    clear(crumbsEl);
    // 根那一格。
    crumbsEl.append(crumb(codeText(), "", kept.path === ""));
    // ⚠️ 段落来自**载荷**（`tree()` 的 `breadcrumb.segments`），不是本文件按 "/" 切的：
    //    切分规则在 `Breadcrumb::new` 里（空段不折叠、根是空串），抄一份到这里
    //    就等于把同一条判据写出两个会分叉的答案。
    const segments = crumbSegments();
    for (const segment of segments) {
      crumbsEl.append(icon("chevron.compact.right", "files__crumb-sep"));
      crumbsEl.append(crumb(segment.name, segment.path, segment.path === kept.path));
    }
    // 「下载本目录」只在**不在根**时出现（根上它与「全部下载」同义 —— macOS `:115`）。
    downloadDirEl.hidden = kept.path === "";
  }

  /** 一段面包屑。⚠️ 文案是**路径原文**（`×` 与空格由 `textContent` 原样渲染，约束 3）。 */
  function crumb(name, path, isCurrent) {
    const el = h("button", {
      class: isCurrent ? "files__crumb is-current" : "files__crumb",
      type: "button",
      // 悬停提示 = 那一段的完整路径（**数据**，不是文案）。
      title: path === "" ? null : path,
      text: name,
    });
    // ⚠️ 用 `data-path` 而不是闭包：面包屑是**整段重建**的（换一层就重画），
    //    闭包会跟着旧的节点一起被丢掉，而 `data-path` 让事件委托读得出点了哪一段。
    el.dataset.path = path;
    return el;
  }

  /** 当前这一批的码（工具栏那一格用的是同一个值，`render.js:codeLabelFromState`）。
   *  ⚠️ 取不到时**空着**（不编一个占位符出来）—— macOS 侧那一句 `?? "—"` 是**视图里写的**，
   *    而本代"一个面向用户的字都不许由前端造"（§3.2）：`—` 也是一个字。
   *    真机上取不到只有一种可能：这一屏被挂起来了而 `state().load` 还不是 `loaded`，
   *    而 `render.js:routeOfState` 不会让那种情况发生。 */
  function codeText() {
    return typeof observedCode === "string" ? observedCode : "";
  }

  function crumbSegments() {
    return Array.isArray(segments) ? segments : [];
  }

  /** 表格那一块的四态（读取中 / 读失败 / 空目录 / 有内容）。 */
  function paintPhase() {
    const hasRows = rows.length > 0;
    const showList = phase === "loaded" && hasRows;
    headEl.hidden = !showList;
    rowsEl.hidden = !showList;
    // ⚠️ 四态**都要有可见的呈现**（约束 4）。三块状态文案常驻在 DOM 里、由 `hidden` 切换
    //    （照 `empty.js` 的理由：重建会让"用户正拖着选区准备复制的原文"被清掉）。
    statusEl.hidden = showList;
    statusSpinnerEl.hidden = phase !== "loading";
    statusLoadingEl.hidden = phase !== "loading";
    statusFailedEl.hidden = phase !== "failed";
    statusEmptyEl.hidden = !(phase === "loaded" && !hasRows);
    // 「重试」只在**读失败**时给（读取中给它没有意义；空目录不是错误）——
    // 否则用户面对的是一个没有出口的页面（macOS `:201-210`）。
    statusRetryEl.hidden = phase !== "failed";
  }

  /** 目录读失败那条说明（壳写的那句 + 内核原文逐字）。 */
  function paintFailure() {
    if (!failure) {
      failureEl.hidden = true;
      return;
    }
    failureEl.hidden = false;
    // `notice` 是 `Option`：`null` = 这一次没有"退回上一层"这回事（**不是空串**）。
    const notice = failure.notice === null || failure.notice === undefined ? "" : failure.notice;
    setText(failureNoticeEl, notice);
    failureNoticeEl.hidden = notice === "";
    // ⚠️ 正文是**内核原文逐字**（约束 3）：不 trim、不折行、不加前缀。
    setText(failureTextEl, failure.message);
  }

  /** 勾选面变化之后要重画的那几格（**不重建行**：重建会丢掉"正在拖着复制的选区"）。 */
  function paintSelection() {
    for (const rowEl of rowsEl.children) {
      const path = rowEl.dataset.path;
      const checked = kept.selection.has(path);
      rowEl.classList.toggle("is-selected", checked);
      const cb = rowEl.querySelector(".files__cb");
      if (cb) cb.classList.toggle("is-checked", checked);
    }
  }

  /** 底栏。⚠️ 几格说的必须是**同一件事**（同一个勾选面）—— 见 `loadWhole`。 */
  function paintBar() {
    // 摘要与动作的**文案**都从载荷来（`SelectionSummary` / `DownloadAction`），
    // 这里只摆位置。载荷还没到时三格是**空的**（不是编一句"0 项"出来）。
    setText(countEl, summary ? summary.count_text : "");
    setText(sizeEl, summary ? summary.size_text : "");
    setText(downloadEl, action ? action.button_title : "");
    // ⚠️ 「未勾选任何项，将下载全部待下载文件」—— `null` = 这一格此刻不该出现
    // （有勾选时它就不出现；macOS `SelectionBar.swift:33` 的 `hint`）。
    const hint = action && typeof action.empty_selection_hint === "string" ? action.empty_selection_hint : "";
    // 🔴 「这个动作此刻**按不下去**，因为……」（`DownloadAction.blocked_reason`，**Rust 给的**）。
    //    勾选面大到一次发不出去时（C-3 的另一半），按下去只会撞上一条发不出去的请求 ——
    //    所以这个动作**此刻不可用**（工具栏那颗按钮禁用），而这句话就摆在底栏这里。
    //    措辞、门槛、判据全在 Rust 里，
    //    本文件**一个字都不造**（§3.2），这里只做"是不是一个字符串"这一步取值。
    //    ⚠️ 与 `hint` **共用同一个坑位**（两者互斥）：空勾选面发的是一个空数组
    //    （请求体恒定）⇒ 它不可能超预算，所以这两句话永远不会同时出现。
    const blocked = blockedReason();
    const note = blocked === "" ? hint : blocked;
    setText(hintEl, note);
    hintEl.hidden = note === "";
    hintSepEl.hidden = note === "";
    // 🔴 **底栏这颗与右键菜单那一项：真的禁用**（`files.css` 的 `:disabled` 段给了它们
    //    看得出来的禁用态 —— `panels.css` 那套写法的同一份：变灰 + 光标归一 +
    //    悬停**不再**点亮）。少了"看得出"这三个字，禁用就退化成"看着能用、点下去没反应"。
    //    ⚠️ 这一格是**异步**来的（勾选面变了要等载荷回来），所以壳那边还有一道兜底闸
    //    （`commands::enqueue_with`：不发内核、把**同一句话**当这一发的回执）——
    //    窗口里点得动的那一下由它接住。
    //
    // 🔴 **`action === null`（载荷还没到）也禁用** —— 判据与工具栏那一颗**逐字相同**
    //    （`toolbarDownload()` 在 `action` 为 `null` 时返回 `null` ⇒ 壳 `setDownloadAction`
    //    拿到 `enabled: false` ⇒ 禁用）。两个入口、同一件事，**不许分叉**：
    //    `RootView.swift:542-556` 那条注释钉着的就是它。
    //    少了这半条，首挂（或 `reseed`）那一段里底栏这颗是"**空文案 + 可点**"，
    //    而工具栏那颗是禁用的 —— 客户看到一颗没有字的蓝色主按钮，点下去**一条消息都没有**。
    //    （实测：那一刻点击 ⇒ `enqueue` 被 `inFlight` 挡在门口、`invoke` 零调用。）
    const unusable = action === null || blocked !== "";
    downloadEl.disabled = unusable;
    menuDownloadEl.disabled = unusable;
  }

  /**
   * 当前载荷说"这个动作按不下去"的理由（**空串 = 可以按**）。
   *
   * ⚠️ 一律走这一个取值口：两处入口（底栏 / 工具栏）各写一遍 `typeof`，
   *    迟早会漂成"一个禁用一个不禁用"—— 而那种不一致在界面上看起来只是"那颗还能点"。
   */
  function blockedReason() {
    return action && typeof action.blocked_reason === "string" ? action.blocked_reason : "";
  }

  /** `enqueue` 的回执：顶上一句 + **逐条**拒绝理由（约束 4：两边都要有落点）。 */
  function paintFeedback() {
    if (!feedback) {
      noticeEl.hidden = true;
      clear(noticeListEl);
      return;
    }
    noticeEl.hidden = false;
    setText(noticeTextEl, feedback.summary);
    clear(noticeListEl);
    const rejected = Array.isArray(feedback.rejections) ? feedback.rejections : [];
    for (const r of rejected) {
      noticeListEl.append(
        h("li", {}, [
          h("span", { class: "files__notice-path selectable", text: r.path }),
          h("span", { class: "files__notice-reason selectable", text: r.reason }),
        ])
      );
    }
  }

  /** 重画某一层的行。**只在"这一层的行变了"时调**（勾选变化走 `paintSelection`）。 */
  function paintRows() {
    clear(rowsEl);
    for (const row of rows) {
      // ⚠️ **格名逐字来自载荷**：`state_color` / `state_label` 是 `api::tree::row_wire`
      //    调 `RowStateStyle::{color,label}()` 补上的两格 ——
      //    `row.state` 那一格（变体名）**前端不看**（见下面那一格）。
      const colorClass = ROW_COLOR_CLASS[row.state_color];
      const iconEl = h("span", {
        class: row.icon_name === ICON_DIR ? "files__icon is-folder" : "files__icon",
      });
      iconEl.append(icon(row.icon_name));
      // 复选框：**控件**（不是行级手势）—— macOS `:306-315` 记着"行级手势会与选中竞争"。
      // ⚠️ 它的无障碍名字取**这一行自己的名字**（数据，来自 Rust），本文件不造字。
      const cb = h("button", {
        class: kept.selection.has(row.path) ? "files__cb is-checked" : "files__cb",
        type: "button",
        "aria-label": row.name,
      });
      const rowEl = h(
        "div",
        { class: kept.selection.has(row.path) ? "files__row is-selected" : "files__row" },
        [
          h("span", { class: "files__col-name" }, [
            cb,
            iconEl,
            // ⚠️ 文件名原文直出：`×` 与空格照原样显示（约束 3）。
            h("span", { class: "files__name", title: row.name, text: row.name }),
          ]),
          // 三格**逐字**取载荷里的字符串，本文件一格都不格式化。
          h("span", { class: "files__size", text: row.detail_text }),
          h("span", { class: "files__time", text: row.source_time_text }),
          // ⚠️ 这一格的正文是 `state_label`（**不是** `row.state.label` ——
          //    线上 `state` 只是一个变体名 `"Complete"`，没有 `.label` 这个字段；
          //    写成 `.label` 的后果是**整列空白**，而不会有任何东西变红）。
          h("span", {
            class: colorClass ? `files__state ${colorClass}` : "files__state",
            text: row.state_label,
          }),
        ]
      );
      rowEl.dataset.path = row.path;
      rowsEl.append(rowEl);
    }
  }

  // ---------------------------------------------------------------------------
  // 取数（**只有这几个入口**：动作进来、不是渲染进来）
  // ---------------------------------------------------------------------------

  /**
   * 读一层目录。**本屏唯一的 `tree(path)` 调用点之一**。
   *
   * ⚠️ `path` 是**清单原文**（约束 3）：从这一层往下走时它来自 `BrowserRow.path`
   *    （或目录项的 `Breadcrumb::join` 结果），本文件**不拼路径、不规范化**。
   *
   * @param {string} path 要读的那一层
   * @param {boolean} [allowFallback] 读到 `path_not_found` 时要不要退回上一层再读一次
   *   （默认允许；递归进来时传 `false`，见下面那段）。
   */
  async function load(path, allowFallback = true) {
    phase = "loading";
    // ⚠️ **这里不许清 `failure`**（macOS `FileBrowser.swift:466-489` 的 `load` 也不清）：
    //    回退之后要紧接着读**上一层**，而清掉的话那条说明会当场消失 ——
    //    用户看到的是"我双击了一个目录，界面自己跳回上一层，一句话都没说"（约束 4 的静默失效）。
    //    清它的地方只有两处：用户主动导航（`navigate`）与那颗「收起」。
    paintPhase();
    paintFailure();
    let data;
    try {
      // ⚠️ 形参名按 Tauri 2 的默认约定：JS 侧 camelCase、Rust 侧 snake_case。
      //    `tree(path: Option<String>)` —— 给了 `path` 就是"读这一层"那一支。
      data = await ctx.call(ctx.CMD.tree, { path });
    } catch (error) {
      // 整条命令失败（没有内核 / 内核回了错误信封）：**原地显示原文**（约束 3）。
      // ⚠️ 走 `failureText` 取，**不要**写 `error.message`（`invoke` 的拒绝可能是裸字符串）。
      faceFailure({ message: failureText(error), path, notice: null });
      return;
    }
    // ⚠️ **读不到某一层仍然是 `ok` 信封**（`api::tree::level_failure` 的判据）：
    //    它是界面上的一个状态（内核原文 + 该停在哪一层 + 有没有退过），不是整屏错误。
    if (data && data.failure) {
      const f = data.failure;
      // 面包屑跟着**退到的那一层**走（载荷里那一格就是 `Breadcrumb::new(&failure.path)`）。
      applyBreadcrumb(data);
      faceFailure(f);
      // ⚠️ `path_not_found`（内核在路径**指向文件**时也报这个）：退回上一层并**再读一次**
      //    （macOS 靠 `taskKey` 变了自己重进一次）。退不了（根上）就停在原地。
      //    递归只可能发生一次：`DirLoadFailure::of` 只在"上一层与当前层不同"时给 notice。
      if (allowFallback && f.notice !== null && f.notice !== undefined && f.path !== path) {
        await load(f.path, false);
      }
      return;
    }
    rows = data && Array.isArray(data.rows) ? data.rows : [];
    applyBreadcrumb(data);
    phase = "loaded";
    paintRows();
    paintPhase();
    paintFailure();
  }

  /** 把载荷里的面包屑落位（**路径与段落都从载荷来**，本文件不重算）。 */
  function applyBreadcrumb(data) {
    const bc = data && data.breadcrumb ? data.breadcrumb : null;
    if (bc) {
      if (typeof bc.path === "string") kept.path = bc.path;
      segments = Array.isArray(bc.segments) ? bc.segments : [];
    } else {
      segments = [];
    }
    paintCrumbs();
  }

  /** 一次失败：落位 + 画。 */
  function faceFailure(f) {
    failure = f;
    phase = "failed";
    rows = [];
    paintRows();
    paintPhase();
    paintFailure();
  }

  /**
   * 读**整棵树**那一支（勾选摘要 + 底栏那个动作 + 内核给的默认勾选面）。
   *
   * @param {boolean} seedSelection 要不要把**内核给的默认勾选面**播下去。
   *
   * 🔴 **这两个用途必须分开，别合成一个函数**（`ManifestTracking` 那笔账在 web 上的落点）：
   *    · **播种**（`seedSelection = true`）只在"**这一批的树到位那一刻**"发生
   *      —— 首帧与换批各一次。播下去的是 `default_selected`（内核算的"所有非 complete
   *      的文件"），它**不是**"当前层的全部条目"：这正是"点开就能直接点下载"的来源
   *      （macOS `BrowserRow.swift:569-571`）。
   *    · **刷新底栏**（`seedSelection = false`）发生在**用户每动一次勾选**之后 ——
   *      它只要那两句话（`SelectionSummary` / `DownloadAction`）。
   *    ⚠️ 把后者也写成播种，后果是**每勾一项就把用户攒的选择抹回默认面**
   *      —— 而界面上看起来只是"勾了没反应"，不像缺陷。
   */
  async function loadWhole(seedSelection) {
    let data;
    try {
      // 无 `path` 参数的那一支（Tauri 把缺席的 `Option` 收成 `None`）。
      // ⚠️ **刷新那一拍要连勾选面一起交上去**（任务 17）：摘要 / 按钮文案 /
      //    "此刻能不能按"这三格**都是"当前这个勾选面"的函数** —— 不交的话壳只能拿
      //    内核的默认面算，而用户早就把它改过了（底栏与工具栏会说一句**过期的话**）。
      // ⚠️ **播种那一拍不交**（首帧 / 换批复位之后）：那一刻前端手上还没有自己的面
      //    （它正要被这一批的 `default_selected` 播种）—— 交一个空面上去，
      //    壳会如实回答"一项都没勾"，而标题栏里那句"未勾选任何项，将下载全部待下载文件"
      //    就会跟刚发生的事（勾上默认面）对不上。
      const args = seedSelection ? {} : { selection: [...kept.selection] };
      data = await ctx.call(ctx.CMD.tree, args);
    } catch (error) {
      faceFailure({ message: failureText(error), path: kept.path, notice: null });
      return;
    }
    summary = data && data.selection ? data.selection : null;
    action = data && data.action ? data.action : null;
    if (seedSelection) {
      const selected = data && Array.isArray(data.selected) ? data.selected : [];
      kept.selection = new Set(selected);
    }
    paintBar();
    paintSelection();
  }

  /**
   * 换了一批（`observedCode` 与当前数据所属的那一批不同）：**复位 + 重播 + 读根**。
   *
   * ⚠️ 复位发生在**码变的那一刻**、不等这一批的树到（macOS `RootView.onChange(of:)` →
   *    `ManifestTracking::display`）：不复位的话，浏览器会带着**上一批**的路径与勾选面
   *    去面对新批次 —— 那些路径在新批次里可能指向别的文件，而用户以为在下 B 批的东西。
   */
  async function reseed(code) {
    kept.path = "";
    segments = [];
    kept.selection = new Set();
    rows = [];
    failure = null;
    feedback = null;
    summary = null;
    action = null;
    phase = "loading";
    paintCrumbs();
    paintRows();
    paintPhase();
    paintFailure();
    paintBar();
    paintFeedback();
    await loadWhole(true);
    await load("");
    kept.loaded = code;
  }

  // ---------------------------------------------------------------------------
  // 动作（全部由用户动作触发；`render` 一个都不调）
  // ---------------------------------------------------------------------------

  /** 导航到某一层。⚠️ 与 macOS 的 `navigate(to:)` 一样：**上一次的回退说明到这里就过期了**。 */
  function navigate(path) {
    // 🔴 **要去的那一层不许留在勾选面里**（真机缺陷，2026-09-20）。
    //
    // 为什么它会在里面：真浏览器的双击**先发两次 `click`**（`click` → `click` →
    // `dblclick`），而上面那个 `click` 处理器是 `kept.selection = new Set([path])`
    // ⇒ 点着目录的那两下**已经把目录自己选进去了**。
    // 为什么留着是缺陷：它**不在当前这一层**（进了它之后就没有它的行了）⇒ 用户既
    // 看不见、也取消不掉；而内核把每个路径当**前缀**展开
    // （`resolve_targets`：`f.path == p || f.path.starts_with("{p}/")`）⇒
    // 在目录里勾一个文件、点「下载选中」，下的是**整个目录** —— 与"我只勾了这一个"
    // 正好相反。
    //
    // ⚠️ **只去掉"要去的那一层"这一个**，别的勾选面一律不动：跨目录攒下来的选择是
    //    **有意的**（夹具 `keep` 那一场 ⑤ 钉着它）。去掉的那一个也不需要"还回去"——
    //    它现在有行了（就是当前这一层的内容），想要它可以用「全选本层」。
    if (kept.selection.delete(path)) {
      paintSelection();
      void refreshSummary();
    }
    // ⚠️ 上一次的回退说明到这里就过期了（macOS `:493-494` 的 `failure = nil`）：
    //    用户已经在主动换地方，"我把你挪回上一层了"那句话不再说明任何事。
    //    ⚠️ 它是**唯一**一处清说明的地方（外加那颗「收起」）—— 见 `load` 里那段。
    failure = null;
    paintFailure();
    if (path === kept.path) {
      // 点当前那一段 = 重新读一次这一层（macOS `:496`）。
      void load(path);
      return;
    }
    kept.path = path;
    paintCrumbs();
    void load(path);
  }

  /**
   * 双击一行 / 右键菜单里的「下载选中项」：**按行的判别键分派**。
   *
   * ⚠️ **形态偏离（如实记账）**：macOS 的分派判据是 `BrowserPrimaryAction.of(paths:…)`
   *    （纯函数、四条单测），它按"**当前这一层里哪些路径是目录**"判 ——
   *    因为双击的 `items` 可能是**一整个跨目录的勾选面**。
   *    本代那一格**不在任何命令的载荷里**（它是 `presentation` 的纯函数，
   *    但没有一条 wire 把它送出来），而本屏能拿到的只有"被点的这一行的 `kind`"。
   *    ⇒ 这里的判据是 `row.kind === KIND_DIR`（线上是 `"Dir"`，见那个常量），
   *      **只说"被点的这一行"**：
   *      双击一个目录就进它、双击一个文件就入队它自己。多选时双击**不**入队整个勾选面
   *      （与 macOS 的行为有一处可观测的差别）。这是缺口，不是取舍 —— 见报告。
   */
  function primaryAction(row) {
    if (row.kind === KIND_DIR) {
      navigate(row.path);
      return;
    }
    void enqueue([row.path]);
  }

  /** 当前层的全部行（含目录）。⚠️ **本层的**，不是整批 —— 见 `enqueue` 的注释。 */
  function currentLevelPaths() {
    return rows.map((r) => r.path);
  }

  /**
   * 加入下载。
   *
   * ⚠️ `paths` **原样**交给命令层（约束 1：目录由内核按前缀展开，壳不展开）。
   *    **空集合 = 空数组**：内核的语义是「`paths` 为空 = 下全部待下载」，
   *    所以「全部下载」那颗按钮按下去发的是一个空数组（`DownloadTargets::paths` 的既有语义）。
   *
   * ⚠️ **C-3 那道保护的两半都在壳里**（前端一个字都不判）：
   *    · "勾选面覆盖整批 ⇒ 交给内核的是空数组" —— 任务 16 接在 `commands::enqueue` 里；
   *    · "部分勾选但仍然超预算 ⇒ 这个动作**按不下去**" —— 任务 17 接在载荷里
   *      （`DownloadAction.blocked_reason`）。**本文件只读那一格**：是字符串 ⇒
   *      工具栏那颗禁用、并把 Rust 给的那句话摆在底栏（`paintBar` / `blockedReason`）。
   *      ⚠️ 所以本屏**不再**有"最大的那条路走不通"这个缺口 —— 但判据仍然只有一份实现。
   *    ⚠️ 还有一道**兜底闸**在 `commands::enqueue_with` 里：前端这一格是异步取回来的，
   *      勾完之后到载荷回来之间那一下点得下去 —— 那一下由壳挡（不发内核、把同一句话当回执）。
   */
  async function enqueue(paths) {
    if (inFlight) return;
    inFlight = true;
    try {
      const data = await ctx.call(ctx.CMD.enqueue, { paths });
      feedback = data && typeof data === "object" ? data : null;
    } catch (error) {
      // ⚠️ 失败要**原地**显示内核原文、不切走（`EnqueueFeedback::failure` 那条判据的同一句话）：
      //    切走了，那句话就没人看见了。
      feedback = { summary: failureText(error), rejections: [], switches_to_transfers: false };
    } finally {
      inFlight = false;
      paintFeedback();
      // ⚠️ 回执可能改变了"哪些文件还没下完" ⇒ 立刻补一拍 `state()`，
      //    免得要等一整秒才看得见反应（`poll.js:pollNow` 的既有用途）。
      ctx.poller.pollNow();
    }
  }

  // ---------------------------------------------------------------------------
  // 事件
  // ---------------------------------------------------------------------------

  // 面包屑：**事件委托**（面包屑每换一层就整段重建）。
  crumbsEl.addEventListener("click", (event) => {
    const target = event.target.closest(".files__crumb");
    if (!target || !crumbsEl.contains(target)) return;
    navigate(target.dataset.path || "");
  });

  // 「下载本目录」：把**当前这一层**（含子目录）加进下载（macOS `:116-124`）。
  downloadDirEl.addEventListener("click", () => {
    void enqueue([kept.path]);
  });

  // 「重新读取这一层」：重读当前层。
  reloadEl.addEventListener("click", () => {
    void load(kept.path);
  });

  // 回退说明右上角那颗 ×（只收起这条说明，不改位置 —— 位置已经是回退之后那一层了）。
  failureDismissEl.addEventListener("click", () => {
    failure = null;
    paintFailure();
  });

  // 加载失败那颗「重试」。
  statusRetryEl.addEventListener("click", () => {
    void load(kept.path);
  });

  // ---- 行：单击选中 / 复选框勾选 / 双击 / 右键 ---------------------------------
  //
  // ⚠️ 事件挂在**容器**上（委托），不是每建一行挂一次：行是整层重建的，
  //    逐行挂监听会在每次重建时留下一批指向已丢弃节点的闭包。
  rowsEl.addEventListener("click", (event) => {
    const rowEl = event.target.closest(".files__row");
    if (!rowEl) return;
    const path = rowEl.dataset.path;
    if (event.target.closest(".files__cb")) {
      // 复选框：**只动这一项**（macOS `:306-315` 的 insert/remove）。
      toggleChecked(path);
      return;
    }
    // 行上的单击 = 选中**这一行**（`List(selection:)` 的默认行为）。
    // ⚠️ 行高亮与勾选框在 macOS 侧是**同一个集合**（`FileBrowser.swift:229`，
    //    样张的 README 也钉着"不许分到两行上"）⇒ 这里换的是同一个 `selection`。
    //    ⌘/Ctrl + 单击 = 切换这一项（与 `List` 的多选同义），不碰别的行。
    if (event.metaKey || event.ctrlKey) {
      toggleChecked(path);
      return;
    }
    kept.selection = new Set([path]);
    paintSelection();
    void refreshSummary();
  });

  rowsEl.addEventListener("dblclick", (event) => {
    const rowEl = event.target.closest(".files__row");
    if (!rowEl) return;
    const row = rows.find((r) => r.path === rowEl.dataset.path);
    if (row) primaryAction(row);
  });

  rowsEl.addEventListener("contextmenu", (event) => {
    const rowEl = event.target.closest(".files__row");
    // 右键落在**那一行**上，而它不在勾选面里 ⇒ 先把勾选面换成它
    //（macOS 的 `contextMenu(forSelectionType:)`：菜单作用于被点的那几行）。
    // 落在空白处 ⇒ 菜单按**当前勾选面**来（不让用户对着一个空集发请求：
    // 那会变成 `paths: []` = 下全部，与他的意图正好相反 —— macOS `:245-247`）。
    if (rowEl && !kept.selection.has(rowEl.dataset.path)) {
      kept.selection = new Set([rowEl.dataset.path]);
      paintSelection();
      void refreshSummary();
    }
    event.preventDefault();
    openMenu(event.clientX, event.clientY);
  });

  /** 勾选面变了 ⇒ 底栏那几格要跟着变。 */
  function toggleChecked(path) {
    if (kept.selection.has(path)) kept.selection.delete(path);
    else kept.selection.add(path);
    paintSelection();
    void refreshSummary();
  }

  /**
   * 勾选面变了之后刷新底栏那几格。
   *
   * ⚠️ **必须问一次后端**：`SelectionSummary`（「已选 N 项 · 合计 X」）与
   *    `DownloadAction`（按钮文案 / 那句"明说"）都是 `presentation` 的纯函数算的，
   *    而**前端算不出合计大小**（那要 `get_tree` 的 `flat`，`api::tree::whole` 有意不发它）。
   *    ⇒ 抄一份算式到 JS 就是第二份真相（§3.2），所以这里重问一次"整棵树"那一支。
   *    代价：勾一次 = 一次 `get_tree`（**只在用户动作时发生**，不是节拍，规格 §3.5 允许）。
   */
  async function refreshSummary() {
    await loadWhole(false);
    // ⚠️ **补一拍**（`pollNow` 的既有用途：某个用户动作改了会话之后补一拍）：
    //    工具栏那颗下载按钮的文案由 `app.js` **每一拍**问这一屏要
    //    （`registry.js` 的 `toolbarDownload()` 契约），而那一拍最多要等一整秒 ——
    //    用户勾完一项会**盯着那颗按钮**看它变不变（macOS 是即时的）。
    //    补这一拍把那一秒压到一帧，代价是每勾一次多一条 `state()`（它本来就一秒一条）。
    //    ⚠️ 它**不绕过**重入保护与闸门（`poll.js` 的 `_fire`）—— 那正是走这条通道
    //    而不是自己调 `tickState()` 的理由。
    ctx.poller.pollNow();
  }

  // ---- 右键菜单 ---------------------------------------------------------------
  function openMenu(x, y) {
    // ⚠️ 先显示再量尺寸：`hidden` 的元素量出来是 0（`display:none`）。
    menuEl.hidden = false;
    const rect = menuEl.getBoundingClientRect();
    const rootRect = root.getBoundingClientRect();
    // 贴着光标，但**不许超出这一屏**（超出去的部分点不到，等于菜单项少了一半）。
    const left = Math.max(0, Math.min(x - rootRect.left, rootRect.width - rect.width));
    const top = Math.max(0, Math.min(y - rootRect.top, rootRect.height - rect.height));
    menuEl.style.left = `${left}px`;
    menuEl.style.top = `${top}px`;
  }

  function closeMenu() {
    menuEl.hidden = true;
  }
  menuDownloadEl.addEventListener("click", () => {
    closeMenu();
    // 空勾选 ⇒ 空数组 = 全部待下载（与底栏那颗按钮发的是同一件事）。
    void enqueue([...kept.selection]);
  });
  menuSelectLevelEl.addEventListener("click", () => {
    closeMenu();
    kept.selection = new Set(currentLevelPaths());
    paintSelection();
    void refreshSummary();
  });
  menuSelectNoneEl.addEventListener("click", () => {
    closeMenu();
    kept.selection = new Set();
    paintSelection();
    void refreshSummary();
  });

  // ---- 底栏 -------------------------------------------------------------------
  //
  // ⚠️ **「全选」勾的是"这一层看得见的行"，不是"整批的全部文件"**（macOS `:88`）。
  //    有意偏离，如实记账：macOS 那一支要 `get_tree` 的 `flat`（整批的文件路径全集），
  //    而**前端拿不到它**（`api::tree::whole` 有意不发 `flat`）⇒ 本代只能选当前这一层。
  //    后果：**跨层勾满整批**在本代是做得到的（进各层分别全选），但**不是**一次点击；
  //    判据不依赖"用户怎么勾到的" —— 勾选面 == 全集就发 `[]`（任务 16 接上的那一半）。
  selectAllEl.addEventListener("click", () => {
    kept.selection = new Set(currentLevelPaths());
    paintSelection();
    void refreshSummary();
  });
  selectNoneEl.addEventListener("click", () => {
    kept.selection = new Set();
    paintSelection();
    void refreshSummary();
  });
  downloadEl.addEventListener("click", () => {
    void enqueue([...kept.selection]);
  });

  // 点别处 / 按 Esc 关掉右键菜单（**全局监听**，`unmount` 时要解绑）。
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
  // 这一屏的那一拍
  // ---------------------------------------------------------------------------

  /**
   * **本屏唯一的一拍**，挂在 `ctx.poller` 上（第三条线：不许自己起定时器）。
   *
   * ⚠️ 它平时**什么都不做**（连一条请求都不发）：`tree()` 没有节拍（规格 §3.5 的表：
   *    "仅用户操作时"）。它存在的唯一理由是**换批**：
   *    `render()` 把 `state()` 载荷里的码记成 `observedCode`，而"码变了 ⇒ 复位 + 重读"
   *    这件事必须发请求 —— 而 `render` **不许发请求**（契约逐字）。
   *    ⇒ 请求挪到这一拍里发（它一秒钟跑一次，而 `observedCode` 与 `kept.loaded`
   *      不一样只可能发生在**刚刚换了一批**之后）。
   *
   * ⚠️ 用 `INTERVALS_MS.state` 的节拍：本屏要跟的是**会话**那一格的变化
   *    （换批发生在 `state().load` 上），而不是给 `tree()` 定一个自己的节拍
   *    —— 后者会变成一条规格没有授权的轮询。节拍表只有 `poll.js` 那一份。
   */
  function batchWatch() {
    if (observedCode === undefined) return;
    if (observedCode === kept.loaded) return;
    // 🔴 **首挂那一次读还在飞 ⇒ 让它读**（见 `mountingRead` 的注释）：
    //    它是**同一次读**，不是"漏了一次复位"。少了这一句，首挂会确定性地
    //    把整棵树读两遍、并在那一段里露出一颗空文案还可点的主按钮。
    if (mountingRead) return;
    if (inFlight) return;
    inFlight = true;
    void reseed(observedCode).finally(() => {
      inFlight = false;
    });
  }
  ctx.poller.start(TICK, INTERVALS_MS.state, batchWatch);

  // 🔴 **首帧先把"载荷还没到"这件事画出来**（F-2 的另一半）：这一屏刚挂上来的那一段里
  //    底栏那三格**什么都没有**（`summary` / `action` 都还是 `null`），而模板里那颗
  //    `#files-bar-download` **既没有文字、也没有 `disabled`** ⇒ 那一刻它是
  //    "**空文案 + 可点**"，而工具栏上同功能的那颗此刻是**禁用**的
  //    （`toolbarDownload()` 返回 `null` ⇒ 壳禁用并清空）。
  //    点它：`enqueue` 进得去、`action` 是 `null` 所以底栏那颗**没被挡**，
  //    而队列里的语义是"下全部待下载" —— 实测里它落在"点了完全没有反馈"那一支
  //    （约束 4 最恨的形态），而**没有任何东西会红**。
  //    ⇒ 统一到**与工具栏那一条判据**（`action === null` ⇒ 禁用）：两处永远相同
  //    （`RootView.swift:542-556`：两个入口、一份文案、不会分叉）。
  paintBar();

  // 首帧：这一屏刚挂上来，先把"这一批"的默认勾选面与根那一层读回来。
  // ⚠️ **请求发在 `mount` 与用户动作里，不在 `render` 里**（契约：`render` 只做"载荷 → DOM"）。
  void (async () => {
    try {
      // 🔴 **重新挂载 ≠ 换批**（真机上的动作：选完文件去「传输列表」看一眼再切回来）。
      //    这一屏手上**已经有**某一批的数据时，切回来只该把**当前这一层重读一次**
      //    （行是"这一帧的加载结果"，不跨挂载）—— **绝不重播种、也绝不回根**
      //    （`kept` 那段记着这一条的全部理由与真机后果）。
      //    ⚠️ 换批由 `batchWatch` 管（`observedCode` 与 `kept.loaded` 不同 ⇒ `reseed`），
      //    所以"离开期间换了批"这条路由它兜住：它会把位置与勾选面一起复位
      //    ——**那正是对的**（旧批次的路径在新批次里可能指向别的文件）。
      if (kept.loaded !== undefined) {
        await load(kept.path);
        // ⚠️ **底栏那几格也要重新问一次**：`summary` / `action`（已选几项 / 合计多大 /
        //    按钮叫什么 / 此刻能不能按）都是"**当前这个勾选面**"的函数，而勾选面活过了
        //    换屏、这几格没有（它们住在 `mount` 的闭包里）。不问这一拍的后果是
        //    **底栏回来时是空白的**：按钮没有文字、工具栏那颗 `toolbarDownload()` 返回
        //    `null` ⇒ **被禁用** —— 用户看到的是"下载选中"那颗按不下去（真机上
        //    人类伙伴报的"无法进行选中下载"里就有这一半）。
        //    它与 `load` 一样是**取数**（不是渲染）：这里问、`paintBar` 只负责摆。
        await loadWhole(false);
        return;
      }
      await loadWhole(true);
      await load("");
      // ⚠️ 记账：这一屏的数据属于"`render` 记下来的那一批"。`render` 还没跑过时
      //    `observedCode` 是 `undefined` ⇒ `kept.loaded` 也留 `undefined`，
      //    于是 `render` 一到、`batchWatch` 就会补一次复位 + 重读（那正是对的）。
      kept.loaded = observedCode;
    } finally {
      // 🔴 **这一句是"首挂只读一次"的闸**（呼应 `mountingRead` 的注释）：它在**所有**
      //    出口上都要落地 —— 上面任何一步抛出来，`batchWatch` 都必须重新拿回"该不该复位"
      //    的判断权（否则一次抛错会让本屏**再也认不出换批**）。
      mountingRead = false;
    }
  })();

  return {
    /**
     * 载荷 → DOM。
     *
     * ⚠️ 本屏只从这一格取**一样东西**：这一批的码（`state.load.summary.code`）。
     *    行、面包屑、勾选摘要、按钮文案全在 `tree()` 的载荷里 —— 它们不是"每一拍都在变"
     *    的东西，所以**不跟着 `state()` 的节拍走**（规格 §3.5：`tree()` 只在用户操作时）。
     *
     * ⚠️ **本函数不发请求、不建事件、不碰 `kept.selection`**（契约逐字）：
     *    它只把看到的那一格记下来，剩下的交给 `batchWatch` 那一拍。
     */
    render(load) {
      const code =
        load && load.kind === "loaded" && load.summary && typeof load.summary.code === "string"
          ? load.summary.code
          : undefined;
      observedCode = code;
      // ⚠️ 批次号**画在面包屑的根那一格上**（macOS `:103`），所以它变了要重画 ——
      //    但**只在真的变了时**重画（见 `paintedCode` 那段：无脑重画 = 每秒重建那一行）。
      if (code !== undefined && code !== paintedCode) paintCrumbs();
    },

    /**
     * **工具栏那颗下载按钮**（`registry.js` 文件头那份契约的**唯一实现者**今天只有这一屏）。
     *
     * 🔴 为什么必须由**屏**来回答（而不是壳自己算）：那颗按钮的文案与动作**都由勾选面决定**
     *    （`DownloadTargets::button_title(selection)`），而勾选面住在这一屏里
     *    （`kept.selection`，壳看不见）。人类伙伴在真机上点它的时候，
     *    期待的是 macOS 的行为：**空勾时换一句话说、点它 = 下当前勾选面**。
     *
     * ⚠️ **两份值都不是本文件造的**：
     *    · `title` —— `action.button_title`，来自 `tree()` 无 `path` 那一支的载荷
     *      （`api::tree::DownloadAction`，由 `DownloadTargets::button_title` 算出来）。
     *      它与底栏那颗 `#files-bar-download` 显示的是**同一个字符串**（`paintBar` 里
     *      也读它）—— 两处不一样就是错（`RootView.swift` 的注释逐字钉着这一条）。
     *    · `run()` —— 与底栏那颗、右键菜单那颗走**同一个** `enqueue([...kept.selection])`；
     *      空勾选 ⇒ **空数组** ⇒ 内核的语义是「下全部待下载」（`DownloadTargets::paths`
     *      的既有语义）。**不在这里判"空勾要不要发"** —— 那是 Rust 的判据。
     *
     * ⚠️ `action` 为 `null`（还没拿到过 `tree()` 无 `path` 那一支的载荷）时返回 `null`
     *    ⇒ 壳把按钮禁用并清空文字。这与 macOS 的 `.disabled(!hasLoadedManifest)`
     *    （`RootView.swift:554`）是**同一个判据的两种落点**：没加载批次时那颗按钮是灰的。
     *    这一屏被挂起来时 `routeOfState` 已经保证 `load.kind === "loaded"`，
     *    所以 `null` 只出现在"刚挂上、第一拍 `tree()` 还没回来"的那一小段。
     *
     * 🔴 **`enabled` 就是"这一刻按下去发不发得出去"**（任务 17）：载荷说按不下去
     *    （`action.blocked_reason` 是个字符串）时它是 `false` —— 与底栏那颗**同一个判据**
     *    （都走 `blockedReason()`），而那句话由底栏摆出来（工具栏那一格只放得下标题）。
     *    ⚠️ 这不是"暂时没有选中项"那个分支：那种情况 `enabled` 仍然是 `true`
     *    （那时按钮换一句话说、点下去 = 下全部待下载 —— 见 `button_title` 的文档）。
     */
    toolbarDownload() {
      if (!action) return null;
      return {
        title: action.button_title,
        enabled: blockedReason() === "",
        // ⚠️ `run` **不接参数**：壳不知道勾选面（那是这一屏的私有状态）。
        run: () => {
          void enqueue([...kept.selection]);
        },
      };
    },

    unmount() {
      // 本屏挂上的那一拍要**立刻**停（第三条线：屏自己的节拍不许在切走之后继续打内核）。
      ctx.poller.stop(TICK);
      document.removeEventListener("pointerdown", onDocumentPointerDown);
      document.removeEventListener("keydown", onDocumentKeyDown);
      clear(el);
    },
  };
}
