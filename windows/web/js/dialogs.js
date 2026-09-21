// dialogs.js —— **模态层**：把 `index.html` 里的一块 `<template>` 盖在窗口上，
// 外加「关于」与「开源许可」两扇**纯展示**的窗（它们的入口在设置窗口的底栏）。
//
// 为什么单独一个文件：这一代有**四扇**压在窗口上的东西（换码面板 / 设置窗口 /
// 关于 / 许可全文），它们共用同一套"怎么盖上去、怎么收起来、Esc 归谁"的机制。
// 把那段机制写在四个地方，就会出现四种 Esc 行为 —— 而那种差别**只在同时开了两扇窗时**
// 才看得见（本仓库记过同形的账：确认框与不可用提示"只有一个真的出得来"）。
//
// ---------------------------------------------------------------------------
// 三条纪律
// ---------------------------------------------------------------------------
// 1. **本文件里一个面向用户的字符都没有**（规格 §3.2）：标题、按钮名、说明句全部住在
//    `index.html` 的 `<template>` 里（**结构文案**，JS 只克隆与摆放），
//    会变的那几格（版本号、许可名、许可全文、sha256）**全部来自命令载荷**。
//    ⇒ 本文件里一个中文字符串字面量都没有（注释不算 —— 守卫会剥掉它们）。
//
// 2. **栈**：同一时刻可以开着两扇（设置窗口之上再开许可全文）。Esc 只收**最上面**那一扇，
//    背影点击同理。收掉上面那一扇之后，下面那一扇仍然在，焦点回到它里面。
//
// 3. **焦点进来、出去**：打开时把焦点移进面板（键盘用户才用得了），收起时还给
//    打开它之前那个元素。少了前半条，Tab 会跑到**盖在下面**的界面上（视觉上被遮住、
//    键盘上却够得着）；少了后半条，用户关掉窗之后焦点落在 `<body>`，
//    下一次 Tab 从窗口最开头重来。
//
// ⚠️ 本文件**不碰 `ctx.call` 之外的任何后端面**（`registry.js` 契约第 2 条的同一条口径），
//    也**不建定时器**（第 3 条）：这两扇窗的数据是**打开时取一次**的静态载荷
//    （版本号、两份许可全文），没有节拍可言。

import { byIdIn, clear, h, icon, setText, template } from "./dom.js";
import { failureText } from "./invoke.js";

/**
 * 造一颗「收起」按钮（从 `tpl-notice-dismiss` 克隆，**并把那个 × 补上**）。
 *
 * 🔴 **这一条是审查抓出来的**：我第一版把监听器直接挂在模板里那个**空的
 *    `<span class="set__dismiss">`** 上 —— 它没有内容、没有尺寸、**点不动**。
 *    一条"点不动的关闭控件"比"根本没有"更坏：它看起来就是坏的。
 *    `verify.js` 与 `shell.js:noticeRow` 那两处一直是对的（克隆 + `prepend(icon(...))`），
 *    我这一版漏了后半句 ⇒ 五处（关于 / 许可 / 设置的两条 / 换码面板那条）全是
 *    **一个看不见也点不到的空壳**。⇒ 收成这一个函数，三个文件共用。
 *
 * ⚠️ 那颗 × 的图形名 `notice.dismiss` 住在 `dom.js:ICONS`（图形的家）——
 *    与 `verify.js` 的收起、`shell.js` 的收起是**同一个名字、同一段路径**。
 *    它的颜色走 `currentColor`（`.notice__icon` 给次级色），所以三处长得一样。
 *
 * @param {() => void} onClick 点下去要做的事（收起的**判决**由调用方给）
 * @returns {DocumentFragment}
 */
export function dismissButton(onClick) {
  const frag = template("tpl-notice-dismiss");
  const btn = frag.querySelector("button");
  btn.prepend(icon("notice.dismiss", "notice__icon"));
  btn.addEventListener("click", onClick);
  return frag;
}

/** 现在开着的模态（**栈**：末尾是最上面那一扇）。 */
const STACK = [];

/**
 * 最上面那一扇的 Esc 处理。
 *
 * ⚠️ 用**捕获**阶段（`capture: true`）并在命中时 `stopPropagation`：
 *    屏里也有 Esc 的用处（文件页的右键菜单靠它收起来）。不拦的话，
 *    用户按 Esc 想关窗，结果是背后的右键菜单收起来了 —— 而窗还在。
 */
function onDocumentKeyDown(event) {
  if (event.key !== "Escape") return;
  const top = STACK[STACK.length - 1];
  if (!top) return;
  event.preventDefault();
  event.stopPropagation();
  top.close();
}

/**
 * 打开一块 `<template>` 作为模态面板。
 *
 * @param {string} templateId `index.html` 里那块 `<template>` 的 id
 * @returns {{root: HTMLElement, panel: HTMLElement, close: Function}}
 *   `root` = 背影 + 面板那一整棵（调用方要查自己的挂载点，一律 `byIdIn(root, …)`）；
 *   `panel` = 面板本身（定位/量尺寸用它）。
 */
export function openModal(templateId) {
  const root = h("div", { class: "modal" });
  const panel = h("div", { class: "modal__panel" });
  // ⚠️ 先 `append` 模板、再 `append` 到 `document.body`：模板里的挂载点在**挂上去之前**
  //    就都在 `panel` 里了，所以调用方拿到的 `root` 立刻可用（同 `app.js:applyRoute`
  //    的"先挂好新的、再换上去"）。
  panel.append(template(templateId));
  root.append(panel);

  // 打开之前焦点在哪儿 —— 收起时还给它（见文件头第 3 条）。
  const previouslyFocused = document.activeElement;

  const handle = {
    root,
    panel,
    /** 这一扇还开着没有。收起之后是 `false`（重复 `close()` 不会再动一次栈）。 */
    closed: false,
    close() {
      if (handle.closed) return;
      handle.closed = true;
      const at = STACK.indexOf(handle);
      if (at >= 0) STACK.splice(at, 1);
      if (STACK.length === 0) {
        document.removeEventListener("keydown", onDocumentKeyDown, true);
      }
      root.remove();
      // 焦点还给打开它的人。⚠️ 只在**它还在文档里**时还：那个元素可能已经被
      // 一次换屏/一次重建丢掉了（那时 `focus()` 是个空操作，但要先判掉，
      // 免得把焦点送回一个已经不在的元素上而**什么都没发生**）。
      if (previouslyFocused && document.contains(previouslyFocused)) {
        previouslyFocused.focus();
      }
      // ⚠️ 回调放在**最后**：调用方在回调里做的事（例如"这一扇没了 ⇒ 把 app.js 里
      //    那一格清掉"）不该看见一棵半拆的 DOM。
      if (typeof handle.onClose === "function") handle.onClose();
    },
  };

  // 背影点击 = 关掉（只认**正落在背影上**的那一下：落在面板里的冒泡上来时
  // `event.target` 是面板里的东西，不是 `root` 本身）。
  root.addEventListener("mousedown", (event) => {
    if (event.target === root) handle.close();
  });

  STACK.push(handle);
  if (STACK.length === 1) {
    document.addEventListener("keydown", onDocumentKeyDown, true);
  }
  document.body.append(root);

  // 焦点移进面板（见文件头第 3 条）。
  // ⚠️ **只找输入类**（用户是来填东西的），一格都没有时聚焦**面板本身**。
  //    两条都写错过的形态记在这里：
  //      · 写成 `"input, select, textarea, button, [href]"` 一个选择器：`querySelector`
  //        按**文档顺序**取第一个 —— 而设置窗口里第一个可聚焦的是头上那颗「关闭」，
  //        于是打开窗口时按一下回车就把窗关了；
  //      · 退而聚焦"第一个按钮"（关于 / 许可那两扇就是这样）：屏幕上会出现一个
  //        谁都没按过、却亮着的**焦点环**，看起来像"按钮卡住了"。
  //    ⇒ 没有输入格时聚焦面板本身（`tabindex="-1"`：可编程聚焦、但不进 Tab 序列），
  //      Tab 从它往下走仍然会走到那几颗按钮上。
  const focusable = panel.querySelector("input, select, textarea");
  if (focusable) {
    focusable.focus();
  } else {
    panel.setAttribute("tabindex", "-1");
    panel.focus();
  }
  return handle;
}

// ---------------------------------------------------------------------------
// 「关于」
// ---------------------------------------------------------------------------

/**
 * 打开「关于」窗（规格 §2.1 第 10 项）。
 *
 * 内容与 macOS 的 `AboutView.swift` 对齐：**全称 logo + 应用名 + 版本号**，
 * 版本号可选中复制（报障时被问到的第一个问题就是"你用的是哪个版本"）。
 *
 * ⚠️ **那颗全称 logo 是任务 18 接上的**（`img/benagen-full-logo.png`，静态资源、
 *    不需要 JS 做任何事 —— 所以本文件里没有它的代码，只有这一条注释）。
 *    在此之前这里写的是"品牌资产（`benagen-full-logo.png`）的入库与打包是任务 14
 *    的活（R-6：`BrandAssets` 有意不移植）"—— **那句话把 R-6 读错了**：
 *    R-6 说的是"不建 `brand_assets.rs` 这个 Rust 模块"（资产是给 webview 的
 *    静态文件，归**前端 + 构建脚本**），不是"不显示这颗 logo"。两边一叠加，
 *    这活当时无人认领 —— 任务 18 的报告里记着这次归属真空。
 *
 * ⚠️ **版本号的回落（短版本 → 构建号 → 兜底，绝不返回空串）在 Rust 里**
 *    （`presentation/about_info.rs`，有单测）：本函数只把那**一格**写进界面，
 *    兜底文案也是 Rust 给的那一句（所以取不到版本时，界面上出现的是一句
 *    说明了"去哪儿查"的话，不是一行空白）。
 *
 * @param {{call: Function, CMD: object}} ctx
 */
export function openAbout(ctx) {
  const handle = openModal("tpl-dialog-about");
  const versionEl = byIdIn(handle.root, "about-version");
  const failureEl = byIdIn(handle.root, "about-failure");
  const failureTextEl = byIdIn(handle.root, "about-failure-text");
  byIdIn(handle.root, "about-close").addEventListener("click", () => handle.close());
  // 「收起」= 克隆模板 + 补上那个 ×（见 `dismissButton` 的头注：这一处**曾经是个
  // 挂在空 span 上的监听器**，看得见、点不动）。
  byIdIn(handle.root, "about-failure-dismiss").append(
    dismissButton(() => {
      failureEl.hidden = true;
    })
  );

  // 取一次（没有节拍：版本号在进程的生命周期里不会变）。
  ctx.call(ctx.CMD.about).then(
    (data) => {
      // ⚠️ `version` 是**载荷里那一格**，本文件不 trim、不兜底、不判空 ——
      //    Rust 那边保证过"绝不返回空串"（`the_version_is_never_an_empty_string`）。
      setText(versionEl, data && typeof data.version === "string" ? data.version : "");
    },
    (error) => {
      // 失败原文**逐字**（约束 3）：这一条命令不碰内核，所以它只有一种失败
      // —— 宿主里没有这条命令（前端与命令层不同步）。那也是要说得出来的一件事。
      setText(failureTextEl, failureText(error));
      failureEl.hidden = false;
    }
  );
  return handle;
}

// ---------------------------------------------------------------------------
// 「开源许可」（全文）
// ---------------------------------------------------------------------------

/**
 * 打开「开源许可」窗：**两份许可的全文**（GPLv2 / OFL-1.1）。
 *
 * ⚠️ 这一扇不是装饰，是**法律义务**：应用内嵌 aria2（GPLv2）与 Noto Sans SC 的子集
 *    （OFL-1.1），分发它们就要一并给出许可全文**与展示入口**
 *    （规格 §6.3 的 W-5：「随附**并能看到**」，`licenses.rs` 的文件头记着"第二代内嵌了
 *    但没有任何读取者"那一笔账）。
 *
 * 🔴 **全文逐字，只做排版**（等宽 + 可选中复制，**不截断、不重排、不做摘要**）：
 *    它由 Rust 在编译期编进 exe（`include_str!`），并且构建脚本用 sha256 证明
 *    "编进去的是登记的那一份"。界面把那个 sha256 也显示出来（可选中复制），
 *    用户能拿它去跟官方发布件核对 —— 这正是它被**发**出来的理由（不是装饰）。
 *
 * ⚠️ `name` / `why` 两格同样是载荷给的（`licenses.rs` 的 `LICENSES` 表）：两句话
 *    **跟着条目走**，所以 OFL 那一页不会写着 aria2 的话。
 *
 * @param {{call: Function, CMD: object}} ctx
 */
export function openLicense(ctx) {
  const handle = openModal("tpl-dialog-license");
  const listEl = byIdIn(handle.root, "license-list");
  const bodyEl = byIdIn(handle.root, "license-body");
  const whyEl = byIdIn(handle.root, "license-why");
  const hashEl = byIdIn(handle.root, "license-sha");
  const failureEl = byIdIn(handle.root, "license-failure");
  const failureTextEl = byIdIn(handle.root, "license-failure-text");
  byIdIn(handle.root, "license-close").addEventListener("click", () => handle.close());
  // 同上：「收起」要**真的有一颗粒子可点**。
  byIdIn(handle.root, "license-failure-dismiss").append(
    dismissButton(() => {
      failureEl.hidden = true;
    })
  );

  /** 载荷里的那两份（`null` = 还没取到）。 */
  let licenses = null;
  /** 现在正文里显示的是哪一份（点上面那条时换）。 */
  let shown = -1;

  /** 把某一份的正文摆上去。⚠️ 只写文本：全文那一格是**可选中复制**的。 */
  function show(index) {
    if (shown === index) return; // 同一份不重写第二遍（那一格是给人拖着复制的）
    shown = index;
    const entry = licenses[index];
    clear(bodyEl);
    setText(whyEl, entry.why);
    setText(hashEl, entry.sha256);
    // ⚠️ **逐字**：全文进的是一个单独的 `<pre>`（等宽、可滚动、可选中），
    //    自带换行 —— 许可全文的排版（空行、缩进）是它的一部分，不许折叠。
    bodyEl.append(h("pre", { class: "dlg__license-text selectable", text: entry.text }));
    for (const [i, el] of [...listEl.children].entries()) {
      el.classList.toggle("is-current", i === index);
      el.setAttribute("aria-current", i === index ? "true" : "false");
    }
  }

  ctx.call(ctx.CMD.license).then(
    (data) => {
      licenses = data && Array.isArray(data.licenses) ? data.licenses : [];
      clear(listEl);
      // ⚠️ **两份一个都不许少**：`licenses` 由 Rust 的 `LICENSES` 表决定（今天是 2 条），
      //    本文件不判条数、也不给"一份都没有"编一句话 —— 那会是替内核/壳宣布一件
      //    它没说过的事。
      for (const [index, entry] of licenses.entries()) {
        // 条目的名字来自载荷（`name`）。按钮是**运行时**造的（条数由载荷决定），
        // 所以它只能在这里建；它的**字**仍然全部来自载荷，一个都不在这里拼。
        const btn = h("button", {
          class: "dlg__tab",
          type: "button",
          text: entry.name,
        });
        btn.addEventListener("click", () => show(index));
        listEl.append(btn);
      }
      if (licenses.length > 0) show(0);
    },
    (error) => {
      setText(failureTextEl, failureText(error));
      failureEl.hidden = false;
    }
  );
  return handle;
}
