// files-prefix-ba859793.js —— **冻结的"修前"副本**（只为复现 A/B 而入库，永不参与运行）。
//
// 它是什么：`windows/web/js/screens/files.js` 在提交
//   ba8597930cc2717ae64b0a91d825fe4ad9959587 那一版的**逐字节副本**
//   —— 也就是带着那两条 Critical 的那一版：
//     · 状态列读 `row.state.label` / `row.state.color`，而线上 `state` 是一个**变体名字符串**
//       ⇒ **整列空白**、也没有颜色；
//     · `KIND_DIR = "dir"`，而线上 `kind` 是 `"Dir"` ⇒ **双击目录变成「下载整个目录」**。
//
// 🔴 它为什么在仓库里：**"修好了"必须能复现"修之前长什么样"**。
//   `windows/scripts/check_files_screen.sh --ab` 会把这份文件换进一份临时副本里跑同一套断言，
//   于是"修前 17 PASS / 9 FAIL"这句话是**跑出来的**，不是从报告里抄的（R-61：
//   每次判定"修好了"，都要答得出"它现在被什么守着、以及它改的是什么"）。
//
// ⚠️ 三条纪律：
//   ① **它不在 `windows/web/` 下** —— 那个目录里的任何文件都会被 `frontendDist` 编进 exe
//      （测试/证据代码进 exe = 把测试发给客户）；
//   ② **它不会被任何东西 import**：只有 `--ab` 那条路把它拷进一个临时目录；
//   ③ **不要"顺手更新"它**：它是**冻结的证据**，改了就不再是"修前那一版"了。
//      （唯一该动它的时候：`--ab` 的对照基准要换成另一个提交 —— 那要连文件名一起改。）
//
// 下面是原件（一字未动）：
// ---------------------------------------------------------------------------
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
//    所以这里不许出现任何"切到别的屏还该在"的东西。

import { byIdIn, clear, h, icon, setText, template } from "../dom.js";
import { INTERVALS_MS } from "../poll.js";
import { failureText } from "../invoke.js";

export const id = "files";

/** `BrowserRowKind` 的线上原文（`presentation/browser_row.rs` 的判别键来自内核的 `"type"`）。 */
const KIND_DIR = "dir";

/** `RowColor`（五个成员，`presentation/mod.rs`）→ 本屏状态列的类名后缀。
 *
 * ⚠️ **这张表是"语义 → 画法"的那一步**（同 `dom.js` 的 `ICONS`）：名字是 Rust 给的
 *    （`RowStateStyle::color()`，有单测钉着"四态两两不同"），这里只决定它画成哪个色。
 *    ⚠️ **五个成员一个都不能少**（`RowColor` 不是 6 个成员那笔账记在
 *    `presentation/mod.rs` 里）；取不到值时**不给类名** ⇒ 那一格退回继承色 ——
 *    而不是"随便挑一个颜色"（挑错就是"两种状态画成一样"，约束 4 明禁）。
 *    ⚠️ 反过来，这里**不许**按 `state.label` 去猜颜色：那是把 `RowStateStyle`
 *    的两条判据（label 与 color）在前端重新配一次对，Rust 改了映射不会有东西变红。 */
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

  /** 当前层（清单**原文**路径；根是空串）。壳不规范化它（约束 3）。 */
  let currentPath = "";
  /** 勾选面：**路径**的集合（跨目录多选，进目录时不清空 —— macOS 同款）。 */
  let selection = new Set();
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
  /** 这一屏**当前这些数据**属于哪一批。与 `observedCode` 不同 ⇒ 欠一次重读。 */
  let loadedCode;
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

  // ---------------------------------------------------------------------------
  // 画（**只读状态、只写 DOM**；不请求、不判断业务）
  // ---------------------------------------------------------------------------

  /** 面包屑：根那一格是**批次号**（macOS `FileBrowser.swift:103`），后面跟着每一段。 */
  function paintCrumbs() {
    paintedCode = observedCode;
    clear(crumbsEl);
    // 根那一格。
    crumbsEl.append(crumb(codeText(), "", currentPath === ""));
    // ⚠️ 段落来自**载荷**（`tree()` 的 `breadcrumb.segments`），不是本文件按 "/" 切的：
    //    切分规则在 `Breadcrumb::new` 里（空段不折叠、根是空串），抄一份到这里
    //    就等于把同一条判据写出两个会分叉的答案。
    const segments = crumbSegments();
    for (const segment of segments) {
      crumbsEl.append(icon("chevron.compact.right", "files__crumb-sep"));
      crumbsEl.append(crumb(segment.name, segment.path, segment.path === currentPath));
    }
    // 「下载本目录」只在**不在根**时出现（根上它与「全部下载」同义 —— macOS `:115`）。
    downloadDirEl.hidden = currentPath === "";
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
      const checked = selection.has(path);
      rowEl.classList.toggle("is-selected", checked);
      const cb = rowEl.querySelector(".files__cb");
      if (cb) cb.classList.toggle("is-checked", checked);
    }
  }

  /** 底栏。⚠️ 三格说的必须是**同一件事**（同一个勾选面）—— 见 `applyWhole`。 */
  function paintBar() {
    // 摘要与动作的**文案**都从载荷来（`SelectionSummary` / `DownloadAction`），
    // 这里只摆位置。载荷还没到时三格是**空的**（不是编一句"0 项"出来）。
    setText(countEl, summary ? summary.count_text : "");
    setText(sizeEl, summary ? summary.size_text : "");
    setText(downloadEl, action ? action.button_title : "");
    // 「未勾选任何项，将下载全部待下载文件」—— `null` = 这一格此刻不该出现
    // （有勾选时它就不出现；macOS `SelectionBar.swift:33` 的 `hint`）。
    const hint = action && typeof action.empty_selection_hint === "string" ? action.empty_selection_hint : "";
    setText(hintEl, hint);
    hintEl.hidden = hint === "";
    hintSepEl.hidden = hint === "";
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
      const colorClass = ROW_COLOR_CLASS[row.state && row.state.color];
      const iconEl = h("span", {
        class: row.icon_name === ICON_DIR ? "files__icon is-folder" : "files__icon",
      });
      iconEl.append(icon(row.icon_name));
      // 复选框：**控件**（不是行级手势）—— macOS `:306-315` 记着"行级手势会与选中竞争"。
      // ⚠️ 它的无障碍名字取**这一行自己的名字**（数据，来自 Rust），本文件不造字。
      const cb = h("button", {
        class: selection.has(row.path) ? "files__cb is-checked" : "files__cb",
        type: "button",
        "aria-label": row.name,
      });
      const rowEl = h(
        "div",
        { class: selection.has(row.path) ? "files__row is-selected" : "files__row" },
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
          h("span", {
            class: colorClass ? `files__state ${colorClass}` : "files__state",
            text: row.state ? row.state.label : "",
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
      if (typeof bc.path === "string") currentPath = bc.path;
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
      data = await ctx.call(ctx.CMD.tree, {});
    } catch (error) {
      faceFailure({ message: failureText(error), path: currentPath, notice: null });
      return;
    }
    summary = data && data.selection ? data.selection : null;
    action = data && data.action ? data.action : null;
    if (seedSelection) {
      const selected = data && Array.isArray(data.selected) ? data.selected : [];
      selection = new Set(selected);
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
    currentPath = "";
    segments = [];
    selection = new Set();
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
    loadedCode = code;
  }

  // ---------------------------------------------------------------------------
  // 动作（全部由用户动作触发；`render` 一个都不调）
  // ---------------------------------------------------------------------------

  /** 导航到某一层。⚠️ 与 macOS 的 `navigate(to:)` 一样：**上一次的回退说明到这里就过期了**。 */
  function navigate(path) {
    // ⚠️ 上一次的回退说明到这里就过期了（macOS `:493-494` 的 `failure = nil`）：
    //    用户已经在主动换地方，"我把你挪回上一层了"那句话不再说明任何事。
    //    ⚠️ 它是**唯一**一处清说明的地方（外加那颗「收起」）—— 见 `load` 里那段。
    failure = null;
    paintFailure();
    if (path === currentPath) {
      // 点当前那一段 = 重新读一次这一层（macOS `:496`）。
      void load(path);
      return;
    }
    currentPath = path;
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
   *    ⇒ 这里的判据是 `row.kind === "dir"`，**只说"被点的这一行"**：
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
   * ⚠️ **缺口（如实记账）**：`DownloadTargets::paths` 那条"勾选面覆盖了整批 ⇒ 发 `[]`"
   *    的规则（约束 C-3：超限的请求不会报错，只会把客户端**永久堵死**）
   *    在 `shell-win` 里**没有调用点**（`commands::enqueue` 直接收 `Vec<String>`），
   *    而前端拿不到 `flat`（`api::tree::whole` 有意不发它）⇒ 这条保护今天**没有任何一端在守**。
   *    本屏能做的只有"发显式列表"，它在大批次上是真的可能超限的。见报告。
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
    void enqueue([currentPath]);
  });

  // 「重新读取这一层」：重读当前层。
  reloadEl.addEventListener("click", () => {
    void load(currentPath);
  });

  // 回退说明右上角那颗 ×（只收起这条说明，不改位置 —— 位置已经是回退之后那一层了）。
  failureDismissEl.addEventListener("click", () => {
    failure = null;
    paintFailure();
  });

  // 加载失败那颗「重试」。
  statusRetryEl.addEventListener("click", () => {
    void load(currentPath);
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
    selection = new Set([path]);
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
    if (rowEl && !selection.has(rowEl.dataset.path)) {
      selection = new Set([rowEl.dataset.path]);
      paintSelection();
      void refreshSummary();
    }
    event.preventDefault();
    openMenu(event.clientX, event.clientY);
  });

  /** 勾选面变了 ⇒ 底栏那几格要跟着变。 */
  function toggleChecked(path) {
    if (selection.has(path)) selection.delete(path);
    else selection.add(path);
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
    void enqueue([...selection]);
  });
  menuSelectLevelEl.addEventListener("click", () => {
    closeMenu();
    selection = new Set(currentLevelPaths());
    paintSelection();
    void refreshSummary();
  });
  menuSelectNoneEl.addEventListener("click", () => {
    closeMenu();
    selection = new Set();
    paintSelection();
    void refreshSummary();
  });

  // ---- 底栏 -------------------------------------------------------------------
  //
  // ⚠️ **「全选」勾的是"这一层看得见的行"，不是"整批的全部文件"**（macOS `:88`）。
  //    有意偏离，如实记账：macOS 那一支要 `get_tree` 的 `flat`（整批的文件路径全集），
  //    而**前端拿不到它**（`api::tree::whole` 有意不发 `flat`）⇒ 本代只能选当前这一层。
  //    后果：底栏那条"勾选面覆盖整批 ⇒ 发 `[]`"的入口在本代是**够不着**的（见 `enqueue`）。
  selectAllEl.addEventListener("click", () => {
    selection = new Set(currentLevelPaths());
    paintSelection();
    void refreshSummary();
  });
  selectNoneEl.addEventListener("click", () => {
    selection = new Set();
    paintSelection();
    void refreshSummary();
  });
  downloadEl.addEventListener("click", () => {
    void enqueue([...selection]);
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
   *    ⇒ 请求挪到这一拍里发（它一秒钟跑一次，而 `observedCode` 与 `loadedCode`
   *      不一样只可能发生在**刚刚换了一批**之后）。
   *
   * ⚠️ 用 `INTERVALS_MS.state` 的节拍：本屏要跟的是**会话**那一格的变化
   *    （换批发生在 `state().load` 上），而不是给 `tree()` 定一个自己的节拍
   *    —— 后者会变成一条规格没有授权的轮询。节拍表只有 `poll.js` 那一份。
   */
  function batchWatch() {
    if (observedCode === undefined) return;
    if (observedCode === loadedCode) return;
    if (inFlight) return;
    inFlight = true;
    void reseed(observedCode).finally(() => {
      inFlight = false;
    });
  }
  ctx.poller.start(TICK, INTERVALS_MS.state, batchWatch);

  // 首帧：这一屏刚挂上来，先把"这一批"的默认勾选面与根那一层读回来。
  // ⚠️ **请求发在 `mount` 与用户动作里，不在 `render` 里**（契约：`render` 只做"载荷 → DOM"）。
  void (async () => {
    await loadWhole(true);
    await load("");
    // ⚠️ 记账：这一屏的数据属于"`render` 记下来的那一批"。`render` 还没跑过时
    //    `observedCode` 是 `undefined` ⇒ `loadedCode` 也留 `undefined`，
    //    于是 `render` 一到、`batchWatch` 就会补一次复位 + 重读（那正是对的）。
    loadedCode = observedCode;
  })();

  return {
    /**
     * 载荷 → DOM。
     *
     * ⚠️ 本屏只从这一格取**一样东西**：这一批的码（`state.load.summary.code`）。
     *    行、面包屑、勾选摘要、按钮文案全在 `tree()` 的载荷里 —— 它们不是"每一拍都在变"
     *    的东西，所以**不跟着 `state()` 的节拍走**（规格 §3.5：`tree()` 只在用户操作时）。
     *
     * ⚠️ **本函数不发请求、不建事件、不碰 `selection`**（契约逐字）：
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
    unmount() {
      // 本屏挂上的那一拍要**立刻**停（第三条线：屏自己的节拍不许在切走之后继续打内核）。
      ctx.poller.stop(TICK);
      document.removeEventListener("pointerdown", onDocumentPointerDown);
      document.removeEventListener("keydown", onDocumentKeyDown);
      clear(el);
    },
  };
}
