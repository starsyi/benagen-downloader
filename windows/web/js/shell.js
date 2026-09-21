// shell.js —— **壳的挂载点**：骨架、侧栏、工具栏、常驻提示行。
//
// 这个文件与 `render.js` 的分工是**一条线**，别混：
//   · `render.js` 回答"**载荷里的哪一格填进哪个槽位**"（纯映射，输入是命令的 data；
//     它不建 DOM、不碰事件）；
//   · 本文件回答"**那些槽位长在哪儿、点了会怎样**"（DOM + 事件，输入是 render 的结果；
//     它不看载荷、不知道 `state()` 长什么样）。
// ⇒ 载荷形状变了只改 `render.js`；布局/交互变了只改这里。两边都能被单独读完。
//
// 🔴 **屏（任务 10/11/12）不许碰本文件。** 它们只拿到 `#page` 这个宿主与一个
//    `ctx`（见 `screens/registry.js` 的接口契约）。需要新的**壳级**元素时
//    （比如工具栏上多一颗按钮），改这里并在报告里点名 —— 落在屏文件里的话，
//    切到别的屏那颗按钮就没了，而那种缺陷**只在切屏时**才看得见。
//
// ⚠️ 本文件**不许出现任何面向用户的字面量**（规格 §3.2）：所有会显示的字，
//    要么住在 `index.html`（**结构文案**），要么是参数传进来的（**数据文案**）。

import { byId, h, icon, setText, clear, template } from "./dom.js";

/** 三个分区（`SidebarSection`，`macos/.../Views/Sidebar.swift:17`）。**成员不许改。** */
export const SECTIONS = Object.freeze(["files", "transfers", "verify"]);

/**
 * 装配壳。调用一次，在 `app.js` 的最开头。
 *
 * @param {object} handlers
 * @param {(section: string) => void} handlers.onSectionChange 点了侧栏某一项
 * @param {() => void} handlers.onRetry 点了常驻提示行那颗「重试」（**只有引擎横幅有**）
 * @param {() => void} handlers.onCodeClick 点了工具栏那颗交付码按钮
 * @param {() => void} handlers.onSettingsClick 点了工具栏那颗「设置」
 * @param {() => void} handlers.onDownloadClick 点了工具栏那颗下载按钮
 * @param {(id: string) => void} handlers.onNoticeDismiss 点了某条**带 `id` 的**常驻
 *   提示行上那颗「收起」。⚠️ 只有"壳自己记着的回执"才带 `id`（任务 13 的换码结果 /
 *   历史写盘失败 / 改下载目录回执）—— 它们的正文**不在 `state()` 里**，
 *   所以"收起来"这件事必须回到 `app.js` 那一份数组上去做（就地 `row.remove()` 的话，
 *   下一拍那一行会因为键又变了而**复活**）。`state()` 带回来的那些没有 `id`，
 *   仍然是就地移除（它们的权威在载荷里，下一拍本来就由载荷说了算）。
 */
export function mountShell(handlers = {}) {
  const shell = {
    /** 屏宿主：任务 10/11/12 的东西挂在这里面。 */
    page: byId("page"),
    _section: SECTIONS[0],
    _sectionButtons: new Map(),
    _summaryEl: byId("batch-summary"),
    // 静态「交付码」那一格（住在 index.html）：有生效批次时它让位给真码。
    _codeStatic: byId("tb-code-static"),
    _codeValue: byId("tb-code-value"),
    _downloadBtn: byId("tb-download"),
    _downloadTitle: byId("tb-download-title"),
    _engineBadge: byId("engine-badge"),
    _engineIcon: byId("engine-icon"),
    _engineText: byId("engine-text"),
    /** 上一次画出来的图标名（`null` = 现在没有图形）——用来避免每拍重建那枚 `<svg>`。 */
    _engineIconName: null,
    _notices: byId("notices"),
    /** 上一次 `setNotices` 的内容签名（`undefined` = 还一次都没设过）。 */
    _noticesKey: undefined,
    _batchFields: {
      code: byId("summary-code"),
      expired: byId("summary-expired"),
      files: byId("summary-files"),
      size: byId("summary-size"),
      validity: byId("summary-validity"),
    },
    _badges: {
      files: byId("badge-files"),
      transfers: byId("badge-transfers"),
      verify: byId("badge-verify"),
    },
    _retryHandler: handlers.onRetry || null,
    _dismissHandler: handlers.onNoticeDismiss || null,
  };

  // ---- 侧栏：三项 + 选中态 --------------------------------------------------
  for (const section of SECTIONS) {
    const el = byId(`nav-${section}`);
    shell._sectionButtons.set(section, el);
    el.addEventListener("click", () => {
      shell.setSection(section);
      if (handlers.onSectionChange) handlers.onSectionChange(section);
    });
  }

  // ---- 工具栏 --------------------------------------------------------------
  shell._downloadBtn.addEventListener("click", () => {
    if (handlers.onDownloadClick) handlers.onDownloadClick();
  });
  byId("tb-code").addEventListener("click", () => {
    if (handlers.onCodeClick) handlers.onCodeClick();
  });
  byId("tb-settings").addEventListener("click", () => {
    if (handlers.onSettingsClick) handlers.onSettingsClick();
  });

  // -------------------------------------------------------------------------
  // 数据槽位
  // -------------------------------------------------------------------------

  /** 当前分区（读的是壳自己的状态；**不查载荷** —— 那是 `render.js` 的事）。 */
  shell.getSection = () => shell._section;

  /** 换分区。**只改高亮**：主区显示什么由 `app.js` 的路由决定（与 macOS 的
   *  `NavigationSplitView` 同形 —— 侧栏永远可点，但没加载批次时主区仍是空态页）。 */
  shell.setSection = (section) => {
    shell._section = section;
    for (const [key, el] of shell._sectionButtons) {
      el.classList.toggle("is-selected", key === section);
      // `aria-current` 是给读屏的：视觉上的选中态对辅助技术**完全不可见**
      //（这与"颜色不能是唯一线索"是同一条无障碍纪律）。
      el.setAttribute("aria-current", key === section ? "page" : "false");
    }
  };

  /**
   * **值没变就不碰 DOM** —— 本文件里所有"往一格文本里写东西"的地方都走它。
   *
   * 🔴 这不是性能优化，是一条**功能**要求（本仓库记过三次的那条）：`setText` 干的是
   *    `el.textContent = …`，而那一笔**换掉文本节点** ⇒ 客户正拖着的那段选区被清掉、
   *    读屏正在念的那一句被打断。而本文件那三个 setter（批次摘要 / 交付码 / 引擎徽标）
   *    **每秒被喂一次**（`state` 的节拍），它们装的那几格一秒里几乎从不变化
   *    —— 于是"每拍重写"的全部后果就是**每秒清一次选区**。
   *
   * ⚠️ **判据是"这一格该显示什么"，不是"上一次调用传了什么"**：直接读 DOM 现在的值来比，
   *    于是没有第二份状态要维护（`setDownloadAction` 早就是这么写的，这里只是把它
   *    收成一条所有格子共用的写法 —— 只改 `setDownloadAction` 一处是不够的：
   *    下一格新加的 setter 会照样重犯）。
   *
   * ⚠️ 实测过的后果（2026-09-20 的整分支审查）：`#summary-code` 里那个交付码
   *    **选不动**（选区 1 拍之后变成空串），`#engine-text` 那句引擎原文同理 ——
   *    而那句话正是约束 3 / C-7 要客户"原样发给业务方"的东西。
   */
  function writeText(el, text) {
    const want = text === null || text === undefined ? "" : String(text);
    if (el.textContent !== want) setText(el, want);
  }

  /** 侧栏未完成徽标。`count` 为 0 / 非数 ⇒ **这一格不渲染**（macOS 的 `.badge(0)`）。 */
  shell.setNavBadge = (section, count) => {
    const el = shell._badges[section];
    if (!el) return;
    const n = typeof count === "number" && Number.isFinite(count) ? count : 0;
    if (n > 0) {
      writeText(el, String(n));
      el.hidden = false;
    } else {
      writeText(el, "");
      el.hidden = true;
    }
  };

  /**
   * 底部批次摘要。`summary` 为 `null` ⇒ **整块不渲染**。
   *
   * ⚠️ 四格的值**逐字**来自 `DeliverySummary`（`state().load.summary`），
   *    本函数**不判断过期**（`expired_badge_text` 是不是 `None` 由 Rust 决定）。
   *
   * 🔴 **五格都走 `writeText`**（值没变就不碰 DOM）：这一格里的交付码**是可选中复制**的
   *    （约束 3：客户要把这一批的码发给业务方），而本函数每秒被调一次 ——
   *    无脑重写的后果是**选区每秒被清一次**，`#summary-code` 选不动。
   */
  shell.setBatchSummary = (summary) => {
    if (!summary) {
      shell._summaryEl.hidden = true;
      return;
    }
    shell._summaryEl.hidden = false;
    writeText(shell._batchFields.code, summary.code);
    writeText(shell._batchFields.files, summary.files_text);
    writeText(shell._batchFields.size, summary.size_text);
    writeText(shell._batchFields.validity, summary.validity_text);
    // `expired_badge_text` 是 `Option`：`null` = 没过期（**不是空串**）。
    // `None` 与 `Some("")` 的区别就是"这一格有没有值"，所以这里判的是 `null`。
    const expired = summary.expired_badge_text;
    if (expired === null || expired === undefined) {
      writeText(shell._batchFields.expired, "");
      shell._batchFields.expired.hidden = true;
    } else {
      writeText(shell._batchFields.expired, expired);
      shell._batchFields.expired.hidden = false;
    }
  };

  /**
   * 工具栏那颗交付码按钮的标签。`code` 为 `null` ⇒ 退回 `index.html` 里的静态那一格。
   *
   * 🔴 **值没变就不写**（同 `setBatchSummary` 的理由）：这一格也是可选中复制的原文，
   *    而它每秒被喂一次。切"静态那一格 ⇄ 真码"那两条 `hidden` 也只在真的换态时才动。
   */
  shell.setCodeLabel = (code) => {
    const hasCode = typeof code === "string" && code.length > 0;
    if (shell._codeStatic.hidden !== hasCode) shell._codeStatic.hidden = hasCode;
    if (shell._codeValue.hidden !== !hasCode) shell._codeValue.hidden = !hasCode;
    writeText(shell._codeValue, hasCode ? code : "");
  };

  /**
   * 工具栏那颗下载按钮。
   *
   * ⚠️ `title` **必须**是 `DownloadTargets::button_title` 算出来的那一句 ——
   *    本函数不兜底、不猜（空串时那一格就是不显示，而不是变成"下载"两个字）。
   *    底栏那颗按钮（任务 10）要显示**逐字相同**的一句：同一个算式、同一个选择集，
   *    两处不一样就是错（`RootView.swift` 的注释逐字钉着这一条）。
   *
   * ⚠️ **`title` / `enabled` 的来源是"当前那一屏"**（任务 13）：`app.js` 每一拍问一次
   *    屏的 `toolbarDownload()`（契约在 `js/screens/registry.js` 的文件头），
   *    把它的返回值原样喂进来。**本函数不判断"这一屏该不该有下载动作"** ——
   *    屏返回 `null` 时 `app.js` 交进来的就是 `{title: "", enabled: false}`。
   *    在壳里写"如果是某一屏就……"会让同一条判据有两个真相源。
   *
   * ⚠️ **值没变就不碰 DOM**（与 `setEngineBadge` 的图标名比对、`setNotices` 的内容签名
   *    同一条纪律）：本函数从任务 13 起**每秒被调一次**（它挂在 `state` 那一拍上），
   *    而无脑写 `disabled` / `textContent` 等于每秒碰一次那两个节点 ——
   *    `textContent` 那一笔尤其要命：它**换掉文本节点**，正好落在"鼠标停在按钮上、
   *    或读屏正在念它"的那一刻。判据是"这一格该显示什么"，不是"上一次调用是什么"。
   */
  shell.setDownloadAction = ({ title, enabled }) => {
    const wantDisabled = enabled !== true;
    if (shell._downloadBtn.disabled !== wantDisabled) {
      shell._downloadBtn.disabled = wantDisabled;
    }
    const wantTitle = title === null || title === undefined ? "" : String(title);
    if (shell._downloadTitle.textContent !== wantTitle) {
      setText(shell._downloadTitle, wantTitle);
    }
  };

  /**
   * 工具栏末尾那颗引擎徽标。
   *
   * @param {{text: string, icon: string|null, tone: string|null, tooltip: string|null}|null} desc
   *   ⚠️ **`icon` 是从载荷来的**（`engine.icon` ← `EngineStatusPresentation::icon_name`），
   *      本函数只负责"把那个名字画成图形"，**不判断该用哪个名字**（§3.2）。
   *      `icon` 为 `null` 时徽标**没有图形但保留正文** —— 正文是更重要的那一半。
   *
   * ⚠️ `desc` 为 `null`（**没有正文可说**）时**整颗不渲染** —— 而不是"只留图标"。
   *    理由是本仓库记了三次的那条：工具栏上"只渲染图标、文字掉进 `.help` 浮层"
   *    栽过三次（`RootView.swift:155-158`）。一颗没有字的徽标在客户眼里与"没画"
   *    无法区分，却占着位置。
   */
  shell.setEngineBadge = (desc) => {
    if (!desc || !desc.text) {
      shell._engineBadge.hidden = true;
      // 🔴 **正文走 `writeText`**：这一格装的是"引擎不可用：<内核原文>"那一整段
      //    —— 它是约束 3 / C-7 要客户原样发给业务方的东西，而本函数每秒被调一次。
      //    无脑重写的后果是**客户正拖着选中的那段原文每秒被清一次**。
      writeText(shell._engineText, "");
      // ⚠️ 图形那一格跟着**名字的备忘值**走（同「名字变了才重建」那一条）：
      //    没有正文时它也该回到空，但不必每秒重来一遍。
      if (shell._engineIconName !== null) {
        shell._engineIconName = null;
        setText(shell._engineIcon, "");
      }
      shell._engineBadge.removeAttribute("title");
      shell._engineBadge.classList.remove("is-running", "is-unavailable");
      return;
    }
    shell._engineBadge.hidden = false;
    writeText(shell._engineText, desc.text);
    // ⚠️ 图形只在**名字变了**的时候重建：这一格每秒被喂一次，重建会把正在悬停的
    //    tooltip 抖掉（与 `setNotices` 的内容签名比对同一条理由）。
    if (desc.icon !== shell._engineIconName) {
      shell._engineIconName = desc.icon || null;
      setText(shell._engineIcon, "");
      if (desc.icon) shell._engineIcon.append(icon(desc.icon, "tb-ico"));
    }
    if (desc.tooltip) shell._engineBadge.setAttribute("title", desc.tooltip);
    else shell._engineBadge.removeAttribute("title");
    shell._engineBadge.classList.toggle("is-running", desc.tone === "running");
    shell._engineBadge.classList.toggle("is-unavailable", desc.tone === "unavailable");
  };

  /**
   * 常驻提示行：**整批替换**。
   *
   * 为什么要"整批"而不是逐条增删：这一组行的顺序是承重的（谁能救场永远在最上面），
   * 而顺序只能由**载荷**决定 —— 逐条增删的写法会让顺序变成"哪条先到"的副作用，
   * 那是一种只在并发时才看得见的缺陷。
   *
   * @param {Array<{kind: string, icon: string, text: string, hint?: string,
   *                retry?: boolean, dismiss?: boolean}>} notices
   *   `kind` ∈ `info` / `warning` / `error`（只影响图标颜色）。
   *   ⚠️ 每一条的 `text` 都是**原文逐字**（约束 3）—— 本函数不 trim、不截断、不折叠。
   *
   * 🔴 **内容没变就一个节点都不动**（下面那个 `key` 的比较）。这不是性能优化，
   *    是一条**功能**要求：这些行的正文是**可选中复制**的（约束 3 / C-7，
   *    客户要能把内核原文原样发给业务方），而这一组行**每秒被喂一次**（`state` 的节拍）。
   *    每拍重建 DOM 的后果是：**选区每秒被清一次**、滚动位置每秒回到顶上、
   *    点在那颗「重试」上的焦点每秒丢一次 ——
   *    而这三个症状都只在"真的用人手去复制"时才出现，代码里看不出来。
   */
  shell.setNotices = (notices) => {
    const list = notices || [];
    const key = JSON.stringify(list);
    if (key === shell._noticesKey) return;
    shell._noticesKey = key;
    clear(shell._notices);
    for (const desc of list) {
      shell._notices.append(noticeRow(desc, shell._retryHandler, shell._dismissHandler));
    }
  };

  shell.setSection(SECTIONS[0]);
  return shell;
}

/**
 * 造一条常驻提示行。
 *
 * 🔴 正文的高度**有界可滚**（`max-height: var(--notice-text-max-h)`）——
 *    这是那条高度不变量（`ResidentNotice.swift` 头部的事故）在 web 上的落点。
 *    **不是**截断：溢出时给的是可滚动的全文，一个字符都不少。
 */
function noticeRow(desc, onRetry, onDismiss) {
  const row = h("div", { class: `notice is-${desc.kind || "info"}` });
  row.append(icon(desc.icon, "notice__icon"));
  // `selectable`：客户要能把这段原文原样复制给业务方（约束 3 / C-7）。
  row.append(h("div", { class: "notice__text selectable", text: desc.text }));
  if (desc.hint) {
    row.append(h("span", { class: "notice__hint selectable", text: desc.hint }));
  }
  if (desc.retry) {
    // 按钮上的字（「重试」）是**结构文案** ⇒ 住在 `index.html` 的 `<template>` 里，
    // 这里只克隆（理由见 `dom.js` 的 `template()`）。
    const frag = template("tpl-notice-retry");
    frag.querySelector("button").addEventListener("click", (event) => {
      event.preventDefault();
      if (onRetry) onRetry();
    });
    row.append(frag);
  }
  if (desc.dismiss) {
    const frag = template("tpl-notice-dismiss");
    const btn = frag.querySelector("button");
    btn.prepend(icon("notice.dismiss", "notice__icon"));
    // ⚠️ **带 `id` 的那些**（壳自己记着的回执）走 `app.js` 的出口：它们的正文
    //    **不在 `state()` 里**，就地 `remove()` 只删掉这一个节点 ——
    //    而下一拍只要那份列表的键变过一次（另一条提示来了又走），这一行就会**复活**。
    //    不带 `id` 的（`state()` 带回来的）仍然是就地移除：它们的权威在载荷里，
    //    下一拍本来就由载荷说了算（`setNotices` 的内容签名挡着，不会无谓重建）。
    btn.addEventListener("click", () => {
      if (typeof desc.id === "string" && onDismiss) onDismiss(desc.id);
      else row.remove();
    });
    row.append(frag);
  }
  return row;
}
