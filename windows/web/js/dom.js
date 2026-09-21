// dom.js —— DOM 原语。**本文件里没有一个面向用户的字符。**
//
// 为什么单独一个文件：规格 §3.2 那条纪律（"JS 只负责摆位置"）能成立的前提，
// 是"摆位置"这件事短到一眼能看完。把 `document.createElement` 散在四个屏文件里，
// 每一处都要人重新确认一遍"这里没在拼字"—— 收到一处，就等于把纪律变成了自觉。
//
// ⚠️ 它**不是**一个 UI 框架：没有虚拟 DOM、没有状态、没有 diff。
//    规格 §6.1 明确不引入 npm/vite/React/Svelte，理由就是"JS 里没有业务逻辑、
//    只有渲染 ⇒ 框架的收益很小"。这里多写一层抽象的收益同样是负的。

/**
 * 建一个 HTML 元素。
 *
 * @param {string} tag 标签名
 * @param {object} [attrs] 属性。`text` 是特例（写 `textContent`），
 *   其余按 `setAttribute` 写；值为 `null`/`undefined`/`false` 的属性**跳过**
 *   （这样 `hidden: cond && true` 这种写法读得出来，也不会有 `hidden="false"` 这种坑）。
 * @param {Node[]} [children]
 */
export function h(tag, attrs = {}, children = []) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "text") {
      el.textContent = value;
    } else if (value === true) {
      el.setAttribute(key, "");
    } else {
      el.setAttribute(key, String(value));
    }
  }
  for (const child of children) {
    if (child !== null && child !== undefined) el.append(child);
  }
  return el;
}

/**
 * 写一个元素的文本。`null`/`undefined` 一律变成**空**，不是字符串 `"null"`。
 *
 * ⚠️ 这是本仓库唯一一处"把值放进界面"的写法，所以它必须**只做搬运**：
 *    不 trim、不截断、不加省略号、不在空值上兜一句话。做其中任何一件，
 *    都是在 JS 里生成一个界面字符串（§3.2），而且那种改动**不会有任何东西变红**。
 */
export function setText(el, text) {
  el.textContent = text === null || text === undefined ? "" : String(text);
}

/** 清空一个元素的所有子节点。 */
export function clear(el) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

/**
 * 把一个字符串切成**两段**，让 CSS 去画**中间**的省略号
 * （`truncationMode(.middle)` 的对应物）。
 *
 * ## 为什么要有它、为什么它不算"JS 在加工界面字符串"（§3.2）
 *
 * **纯 CSS 做不到真正的中间省略**：`text-overflow: ellipsis` 只在**行尾**加省略号，
 * 而 `text-overflow: ellipsis middle` 至今没有任何浏览器实现。所以做法是
 * **把字符串切成两段、放进两个紧邻的格子里**：前一段允许被压缩（CSS 在它的**行尾**
 * 画省略号），后一段不压缩 ⇒ 屏幕上就是「前半…后半」。**省略号是浏览器画的**
 * （本文件里没有 `"…"` 这个字面量）。
 *
 * ⚠️ 判据三条（与 `verify.js:pathRow()` 那一段**逐字同源**，那边有完整论证）：
 *   ① **一个字符都没有被改写**：两格拼起来的文本与传进来的**逐字节相同**
 *      （切点是一个下标，不是一次改写）；
 *   ② **切点是结构性的、与数据无关**：所有字符串按同一个规则切，
 *      不存在"这一条该怎么显示"的判断 —— 判断只发生在 CSS 里（放不下就裁）；
 *   ③ **全文两处都在**：`title` 属性给悬停，跨格选择拿到的也是原文。
 *   被 §3.2 明禁的是另一件事：**按像素测量后把字符串切掉一段、再拼一个省略号**
 *   —— 那会把"内核说了什么"改成"我们想显示什么"。这里没有那样做。
 *   ⚠️ 反过来，**"能切就能拼"是错的**：这一层只许**切分**，不许增删或改写字符。
 *
 * ## 两条切法（都不看数据内容，只看形状）
 *
 *   · `separators` 非空（路径那一类）：切在**最后一个分隔符之后** ——
 *     前一段（目录链）可以很长、让它缩；后一段（文件名）**一个字都不许少**。
 *     这正是 `verify.js:pathRow()` 的规则。
 *   · `separators` 为空（不透明字符串：备注、交付码）：切在 60% 处 ——
 *     两端都留一点。**没有任何语义**，纯粹为了让首尾都看得见。
 *
 * ⚠️ 长度不足（`< 2` 个字符）或切点落在两端时：**整条都进前一段、后一段为空**
 *    （退化成末尾省略）—— 那是最坏情况下的兜底，不是常态。
 *
 * @param {string} text 原文（**一个字符都不改**）
 * @param {string} [separators] 分隔符集合（例如 `"/"`）；空串 = 用 60% 那条规则
 * @returns {[string, string]} `[head, tail]` —— 两段拼起来与 `text` 逐字节相同
 */
export function middleSplit(text, separators = "") {
  const source = text === null || text === undefined ? "" : String(text);
  if (source.length < 2) return [source, ""];

  let cut = -1;
  if (separators) {
    for (let i = source.length - 1; i >= 0; i -= 1) {
      if (separators.includes(source[i])) {
        cut = i + 1;
        break;
      }
    }
  } else {
    cut = Math.round(source.length * 0.6);
  }
  // 切点必须落在**两段都非空**的位置上：落在头上或尾上就退回"整条进前一段"。
  if (cut <= 0 || cut >= source.length) return [source, ""];
  return [source.slice(0, cut), source.slice(cut)];
}

/**
 * 克隆 `index.html` 里的一块 `<template>`（**结构文案的家**）。
 *
 * 为什么要有这条通道：常驻提示行那样的东西是**运行时**才出现的，不可能直接写在
 * `index.html` 的正文里；但它的按钮上的字（「重试」/「收起」）属于**结构文案**
 * —— 与数据无关、任何状态下都一样。让 JS 写 `h("button", {text: "重试"})`
 * 会把这一个字变成"JS 生成的界面字符串"（§3.2 明禁），而它其实只是**结构**。
 * ⇒ 结构留在 HTML 的 `<template>` 里，JS 只负责**克隆与摆放**。
 *
 * @param {string} id `<template>` 的 id
 * @returns {DocumentFragment}
 */
export function template(id) {
  const tpl = document.getElementById(id);
  if (!tpl || tpl.tagName !== "TEMPLATE") {
    throw new Error(`index.html 里没有 <template id="${id}">（前端资源不完整？）`);
  }
  return tpl.content.cloneNode(true);
}

/** 按 id 找**壳的**元素（找不到就抛 —— **不返回 null 让它悄悄消失**）。 */
export function byId(id) {
  return requireEl(document.getElementById(id), id, "index.html 里");
}

/**
 * 按 id 找**某一棵子树里**的元素。**屏自己的挂载点必须用这一条，不要用 `byId`。**
 *
 * 🔴 为什么（这不是风格问题）：
 *   ① **换屏是"先挂好新的、再拆旧的"**（`app.js:applyRoute` 里那段注释）——
 *      新屏在挂载那一刻**还在文档外**，而 `document.getElementById` **看不见
 *      文档外的节点** ⇒ 用 `byId` 的屏会在换屏时**直接挂不起来**；
 *   ② 三个屏（任务 10/11/12）从此可以**用同名 id**（`#name` / `#rows` …）而互不打架 ——
 *      全局唯一 id 这条约束是"三屏并行"最容易被无意踩破的一条。
 * @param {Element} root 屏自己的宿主（`mount(el, ctx)` 的 `el`）
 */
export function byIdIn(root, id) {
  // ⚠️ 措辞是**挑过的**，别顺手改回去：这句话**会渲染**（进常驻提示行），而内嵌字体子集
  //    里没有「板」这个字 —— 写成「这一屏的模**板**里」的后果是一个**豆腐块**，
  //    而它出现的那一刻恰恰是"主区空白、用户需要读懂为什么"的那一刻。
  //    （为什么以前没人发现：字体覆盖判据的 glob 只扫 `*.html`，`.js` 从来没被扫过 ——
  //      任务 10b 堵上了那个洞，这条断言现在由 `test.sh` 第 0.6 步守着。）
  //    ⇒ 同一条纪律：**本句里的字必须在 `windows/assets/ui-subset.otf` 的子集里**。
  //      要改这句，先跑 `bash windows/scripts/test.sh`（第 0.6 步会逐字指名缺哪个）。
  return requireEl(root.querySelector(`#${id}`), id, "这一屏的结构里");
}

/** 上面两条共用的"找不到就抛"。 */
function requireEl(el, id, where) {
  if (!el) {
    // 这是一条**诊断**（不是界面文案）：壳自己的 HTML 与 JS 对不上，
    // 属于"程序坏了"，不是"要告诉客户什么"。诊断允许指名道姓（见 invoke.js 顶部那条界线）。
    throw new Error(`${where}没有 #${id} 这个挂载点（前端资源不完整？）`);
  }
  return el;
}

// ---------------------------------------------------------------------------
// 图标
// ---------------------------------------------------------------------------
//
// 全部是**内联 SVG**（约束 3：零外链），造型照 `a0-proof.html` 里那几个。
// （壳的**静态**图形不在这张表里 —— 它们内联在 `index.html`，理由见下面表头。）
//
// 🔴 **名字与图形是两件事**，别把这张表当成"图标名的权威"：
//    · 语义侧的**名字**（`bolt.fill` / `exclamationmark.triangle.fill` …）由 Rust 给
//      （`presentation::engine_status::icon_name`），前端**不判断该用哪个名字**；
//    · "那个名字画成什么样"留在这一层 —— 这正是 `engine_status.rs` 里写的分工
//      （上游把 `tint`（要 `Color`）留在只负责画的那一层，同形）。
//    ⇒ 所以下面这张表的键**只是"画法"的键**，值只是路径数据（纯 ASCII、不是文案）。
//      加一个名字不用改任何判据；改一个图形也不会动到任何语义。
// ⚠️ **画法（`mode`）与名字分开写，别从名字里猜。**
//    这一条是**实测出来的**：第一版让 `icon()` 看名字以 `.fill` 结尾就填色
//    —— 而 `exclamationmark.triangle.fill` 这个**语义名字**是我们从 macOS 的
//    SF Symbol 名逐字抄来的（`Present/` 那 232 条测试钉着它），它的图形在我们这里
//    是**描边**的三角形加叹号。名字里的 `.fill` 描述的是**SF Symbol 的变体**，
//    不是我们这份图形的画法。按名字猜的结果是：三角形被填成一块实心，
//    叹号（一条只有描边的折线）在 `fill` 模式下什么都画不出来 ——
//    于是界面上出现一块**没有叹号的实心三角**，而它看起来"就是个图标"。
// 🔴 **这张表只放"运行时才出现的图形"。**
//    静态结构上的图形（工具栏三项、侧栏三个分区）**内联在 `index.html` 里**，
//    由浏览器直接画 —— 它们不该等到 JS 跑起来才出现，也不该有两个家。
//    ⇒ 这里**没有** `toolbar.*` 一类的键：曾经有过三个，从来没人引用（工具栏的图形
//      在 HTML 里），那是同一段路径的两份抄本。要加**壳的**静态图形，请加在
//      `index.html`；要加**数据驱动**的图形（名字来自 Rust 的 `icon_name`），加在这里。
/// 三角形（描边版）的路径。**两个名字共用同一段绘图**：
/// `exclamationmark.triangle.fill`（引擎横幅那一档）与 `exclamationmark.triangle`
/// （校验总结的 `KernelSaysNotAllGood` 那一档）—— 名字不同（语义、由 Rust 给），
/// 但"画成什么样"这件事在我们这里**就是同一个图形**。
/// ⚠️ 它**不是**"按名字猜画法"（那条坑见下面 `icon()` 的注释）：两个名字各自**显式**
///    指向这里，加第三个名字时仍然要自己决定用哪一段路径。
const TRIANGLE_PATHS = `<path d="M8 1.9 14.6 13.4H1.4z"/><path d="M8 6.1v3.1M8 11.3v.01"/>`;

const ICONS = {
  // ---- 引擎徽标（`EngineStatusPresentation::icon_name` 的四个名字）
  // `bolt.fill` 是这四个里唯一**填色**的（样张那颗工具栏徽标就是实心闪电）。
  "bolt.fill": { mode: "fill", paths: `<path d="M9.4 1.4 4.2 8.9h3.1l-.7 5.7L12 7.1H8.7z"/>` },
  "questionmark.circle": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M6.2 6.3a1.8 1.8 0 1 1 2.4 1.7c-.4.2-.6.5-.6.9v.3M8 11.6v.01"/>` },
  "circle.dashed": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6" stroke-dasharray="2.4 1.9"/>` },
  // 三角形走**描边**：填色版要把叹号挖成背景色，那就得写死一个 `#fff` ——
  // 而"图标颜色一律 currentColor"正是为了不被写死（同 `.tb-ico` 的理由）。
  "exclamationmark.triangle.fill": { mode: "stroke", paths: TRIANGLE_PATHS },

  // ---- 校验结果屏（`VerifyClass::icon_name` 六个 + `VerifyVerdict::icon_name` 四个）
  //
  // ⚠️ **这些名字全部来自 Rust**（`presentation/verify_summary.rs`，六类两两不同、
  //    四个 verdict 也两两不同，各自有单测钉着）。本表只回答"那个名字画成什么样"。
  //
  // ⚠️ **带 `.fill` 的那几个一律走描边** —— 与 `exclamationmark.triangle.fill`
  //    同一条理由（见它上面那段）：SF Symbol 的 `.fill` 变体是**名字**的一部分，
  //    不是我们这份图形的画法；填色版要把对勾/叉号挖成背景色，那就得写死一个 `#fff`，
  //    而"图标颜色一律 currentColor"正是为了不被写死。
  //
  // ⚠️ `checkmark.seal.fill` 与 `index.html` 里侧栏那颗印章**是同一个图形**，
  //    但两处各自内联、不共享一段路径：那一颗是**壳的静态图形**（住 HTML、由浏览器
  //    直接画），这一颗是**数据驱动的图形**（名字从 Rust 的载荷来，要等 JS 跑起来）。
  //    共享会让"换侧栏图标"顺手改掉校验总结那一行的标记 —— 两处的语义并不相同。
  "checkmark.circle.fill": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M5.4 8.2 7.2 10 10.7 6.3"/>` },
  "xmark.circle.fill": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M5.9 5.9l4.2 4.2M10.1 5.9l-4.2 4.2"/>` },
  "questionmark.folder": { mode: "stroke", paths: `<path d="M1.9 4.2h4.2l1.3 1.7h6.7v6.4H1.9z"/><path d="M7 9a1.3 1.3 0 1 1 1.7 1.2c-.3.15-.5.35-.5.65v.25M8.2 12.4v.01"/>` },
  "ruler": { mode: "stroke", paths: `<rect x="1.8" y="5.4" width="12.4" height="5.2" rx="1"/><path d="M4.6 5.4v2.1M7.2 5.4v3.1M9.8 5.4v2.1M12.4 5.4v3.1"/>` },
  "lock.slash": { mode: "stroke", paths: `<rect x="3.4" y="7.2" width="9.2" height="6.6" rx="1.4"/><path d="M5.6 7.2V5.4a2.4 2.4 0 0 1 4.8 0v1.8"/><path d="M2.6 13.6 13.6 2.4"/>` },
  // ⚠️ `clock` 与 `exclamationmark.triangle`（下面）**两个屏都在用**：
  //    校验页那边是 `VerifyClass::icon_name` 的两个名字，传输列表这边是
  //    `TransferRow::icon_name` 的 Waiting / Error 两档（`transfer_row.rs` 的
  //    `icon_name`，五档两两不同有单测钉着）。同一个语义名字画成同一个图形是**对的**
  //    —— 名字相同而图形不同才是分叉（同 `checkmark.seal.fill` 那段记的相反情形）。
  "clock": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M8 4.6V8l2.4 1.5"/>` },
  "checkmark.seal.fill": { mode: "stroke", paths: `<path d="M8 1.6 6.12 3.47 3.47 3.47 3.47 6.12 1.6 8 3.47 9.88 3.47 12.53 6.12 12.53 8 14.4 9.88 12.53 12.53 12.53 12.53 9.88 14.4 8 12.53 6.12 12.53 3.47 9.88 3.47z"/><path d="M5.8 8.05 7.2 9.5 10.1 6.5"/>` },
  "exclamationmark.triangle": { mode: "stroke", paths: TRIANGLE_PATHS },

  // ---- 传输列表屏：**行图标**（名字来自 Rust 的 `TransferRow::icon_name`）--------
  // `transfer_row.rs` 只给出五个名字，且**两两不同**（`every_task_state_has_a_visible_label`
  // 逐档钉着：Waiting=clock / Active=arrow.down.circle / Complete=checkmark.circle /
  // Error=exclamationmark.triangle / Removed=trash）。
  // ⚠️ 后两个名字（`clock` / `exclamationmark.triangle`）**上面已经有了**，这里不再写一遍
  //    —— 同一个名字画成两个图形，等于"改一处、另一处不变"。
  // ⚠️ 与 macOS 的差别在**图形**不在**名字**：那边是 SF Symbol（`Image(systemName:)`），
  //    这边是自绘的描边图形（同 `folder` / `doc` 那一段的分工）。
  "arrow.down.circle": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M8 4.7v6M5.5 8.2 8 10.7l2.5-2.5"/>` },
  "checkmark.circle": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M5.4 8.2 7.2 10 10.7 6.3"/>` },
  "trash": { mode: "stroke", paths: `<path d="M2.6 4.4h10.8M6.4 4.4V3.1h3.2v1.3M4.3 4.4l.7 9h6l.7-9"/><path d="M6.8 6.8v4.2M9.2 6.8v4.2"/>` },

  // ---- 传输列表屏：行菜单那颗「⋯」（`ellipsis.circle`）--------------------------
  // ⚠️ **它不是我 Rust 给的 `icon_name`**（是壳自己的控件，图形由本表给），
  //    但仍必须住在这里：那颗按钮是**运行时**才出现的（每行一个，从
  //    `index.html` 的 `tpl-screen-transfers-row` 克隆），模板里只有按钮与它的
  //    无障碍名字，图形由 JS 补 —— 与 `notice.dismiss` 同一条分工
  //    （"静态图形住 HTML、运行时图形住这里"，见本表文件头那段）。
  //    macOS 侧那一颗是 `Image(systemName: "ellipsis.circle")`（`TransferRowView.swift`
  //    的 `actionsMenu`），这里的三个点就是它。
  "ellipsis.circle": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6"/><path d="M5.3 8v.01M8 8v.01M10.7 8v.01"/>` },

  // ---- 常驻提示行（`exclamationmark.circle`）
  "exclamationmark.circle": { mode: "stroke", paths: `<circle cx="8" cy="8" r="6.2"/><path d="M8 4.6v4M8 10.9v.01"/>` },

  // ---- 提示行那颗「收起」的 ×。
  // ⚠️ 它**不是** Rust 给的 `icon_name`（是壳自己的控件），却仍然必须在这里：
  //    那颗按钮本身是运行时才出现的（从 `index.html` 的 `<template>` 克隆），
  //    `<template>` 里只有按钮与它的无障碍名字，图形由 JS 补 —— 见 `shell.js:noticeRow`。
  //    "静态图形住 HTML、运行时图形住这里"这条分工见上面那段。
  "notice.dismiss": { mode: "stroke", paths: `<path d="M4 4l8 8M12 4l-8 8"/>` },

  // ---- 文件页：**行图标**（名字来自 Rust 的 `BrowserRow::icon_name`）
  // `BrowserRow::of` 只会给出两个名字：目录行是 `"folder"`、文件行是 `"doc"`
  // （`presentation/browser_row.rs:110,121`）—— 本表只是"这两个名字画成什么样"。
  // ⚠️ 与 macOS 的差别：那边是 SF Symbol（`Image(systemName: row.iconName)`），
  //    这边是自绘的描边图形。**差别在图形，不在名字** —— 名字仍然是 Rust 给的，
  //    换名字（或加第三个）时本表要跟着加，而"该用哪个名字"一个字都不在 JS 里判。
  // ⚠️ 这两个走**填色**（`mode: "fill"`），与上面那几个描边图形不同 —— 这是**照 macOS
  //    的观感**做的决定，不是随手挑的：SF Symbol 的 `folder` / `doc` 是**实心**字形，
  //    样张里那两个也是实心（`.ic-dir` / `.ic-file` 用 `background` + `clip-path` 填出来）。
  //    画成描边的话，一行文件名旁边会多出一圈空心轮廓，整列看起来比样张轻一档。
  //    颜色仍然是 `currentColor`（目录 accent、文件次级色，见 `files.css`）。
  folder: { mode: "fill", paths: `<path d="M1.4 3.4h4.4l1.4 1.8h7.4v7.4H1.4z"/>` },
  // `doc`：一页纸，右上角**切掉一个角**（实心图形画不出"折角那一笔"——
  //    那需要第二种颜色，而图标颜色一律 `currentColor`，见上面那段）。
  doc: { mode: "fill", paths: `<path d="M3.4 1.6h6.2l3 3.1v9.7H3.4z"/>` },

  // ---- 文件页：面包屑的**分隔符**（`chevron.compact.right`，macOS `FileBrowser.swift:106`）
  // ⚠️ 它**不是** Rust 给的 `icon_name`（是这一屏自己的静态图形），却仍然必须在这里：
  //    面包屑有几段是**数据决定的**（`Breadcrumb.segments`），所以分隔符只能运行时造。
  //    ⚠️ 为什么不用 `›` 这个字符：那个码位不在内嵌字体子集里，用它就是一个豆腐块
  //       （样张的注释同样钉着这一条）。
  "chevron.compact.right": { mode: "stroke", paths: `<path d="M6 3.5 10.5 8 6 12.5"/>` },
};

/**
 * 造一个图标元素。
 *
 * ⚠️ 画法由表里的 `mode` 决定（**不是由名字猜**，理由见 `ICONS` 上面那段）。
 *    填色型走 `fill="currentColor" stroke="none"`，描边型走
 *    `fill="none" stroke="currentColor"`。**颜色一律是 `currentColor`** ——
 *    于是禁用态、语义色都自动跟着父级走，不需要为每种状态各写一条图标颜色规则
 *    （样张的 `.tb-ico` 就是这么做的，照搬）。
 *
 * @param {string} name `ICONS` 的键；找不到时返回一个**空**的 `<svg>`（不抛、不占位）
 *   —— 少了某个图形的后果是"这一格什么都没有"，而**抛出**会让整屏挂掉。
 *   这条取舍与 `engine_status.rs` 的"图标名是语义标签"同源：图形缺失是绘制问题，
 *   不该升级成渲染失败。
 */
export function icon(name, className = "") {
  const NS = "http://www.w3.org/2000/svg";
  const el = document.createElementNS(NS, "svg");
  el.setAttribute("viewBox", "0 0 16 16");
  el.setAttribute("aria-hidden", "true"); // 装饰性：语义由旁边那句正文承担
  if (className) el.setAttribute("class", className);
  const entry = ICONS[name];
  if (entry) {
    if (entry.mode === "fill") {
      el.setAttribute("fill", "currentColor");
      el.setAttribute("stroke", "none");
    } else {
      el.setAttribute("fill", "none");
      el.setAttribute("stroke", "currentColor");
      el.setAttribute("stroke-width", "1.4");
      el.setAttribute("stroke-linecap", "round");
      el.setAttribute("stroke-linejoin", "round");
    }
    // ⚠️ 上面那些字符串全是**本文件里写死的 SVG 路径数据**（纯 ASCII），
    //    没有任何一处来自数据或用户输入 ⇒ 用 `innerHTML` 是安全的。
    //    一旦将来有人把**变量**拼进来，这里就变成一处注入点（而 Tauri 的 webview
    //    是有 IPC 权限的）—— 要改的话，改用 `createElementNS` 逐个建。
    el.innerHTML = entry.paths;
  }
  return el;
}
