// empty.js —— **空态屏**：交付码入口。
//
// 三种加载态（未加载 / 加载中 / 失败）**都落在这一屏**（`RootView.mainArea` 的分派）：
// 加载中那一刻还没有清单可看，失败要原地给原文与「重试」，
// 而"没有生效的批次"正是这一屏要说的事。
//
// 结构：标题 + 输入卡片 + 状态区（照 `EmptyState.swift` 的三段式）。
// 结构文案全部住在 `index.html` 的 `<template id="tpl-screen-empty">` 里 ——
// 本文件**一个字面量文案都没有**（§3.2）。

import { byIdIn, clear, icon, setText, template } from "../dom.js";
import { failureText } from "../invoke.js";

export const id = "empty";

/**
 * 挂载。
 *
 * @param {HTMLElement} el 壳给的宿主（`#page` 里的一个 `<div>`）
 * @param {{call: Function, CMD: object, poller: object, shell: object}} ctx
 *   见 `registry.js` 的接口契约（**只有那几样**，别往上加东西）。
 */
export function mount(el, ctx) {
  el.append(template("tpl-screen-empty"));

  // ⚠️ 一律 `byIdIn(el, …)`（**不是** `byId`）：挂载这一刻 `el` 还在文档外
  //    （换屏是"先挂好新的、再拆旧的"，见 `app.js:applyRoute`），
  //    而 `document.getElementById` 看不见文档外的节点。理由写在 `dom.js` 里。
  const codeEl = byIdIn(el, "empty-code");
  const baseUrlEl = byIdIn(el, "empty-base-url");
  const submitEl = byIdIn(el, "empty-submit");
  const retryEl = byIdIn(el, "empty-retry");
  const busyEl = byIdIn(el, "empty-busy");
  const failureEl = byIdIn(el, "empty-failure");
  const failureTextEl = byIdIn(el, "empty-failure-text");
  const failureIconEl = byIdIn(el, "empty-failure-icon");

  // 失败图标（造型照 `exclamationmark.triangle.fill`，颜色由 `.empty__failure` 的
  // 红色 currentColor 带下去）。**在挂载时建一次**，不在每次渲染时重建。
  failureIconEl.append(icon("exclamationmark.triangle.fill", "empty__failure-icon-svg"));

  // 本屏自己的"在飞"标记。⚠️ 它**只是一个防重复点击的闸**，不是界面状态 ——
  // 界面状态一律以 `state().load` 为准（macOS 的 `isBusy` 也是 `loadState == .loading`
  // 算出来的，不是视图自己记的）。有它是因为"一次只发一条"这条约束（约束 15 的单飞
  // 语义在内核侧），而两次点击之间隔着最多 1 秒的轮询窗口。
  let inFlight = false;

  /** 这次要发的码。⚠️ **不 trim** —— macOS 的 `codeToLoad.isEmpty` 也不 trim，
   *  前端多做一次加工就是"界面上看到的与发出去的不是同一个串"。 */
  const currentCode = () => codeEl.value;
  const currentBaseUrl = () => baseUrlEl.value;

  /**
   * 「加载」那颗按钮此刻该不该可按。
   *
   * 🔴 **判据只有"码是空的"这一条**（`EmptyState.swift` 的
   *    `isBusy || codeToLoad.isEmpty || codeTooLong || baseURLTooLong` 里，
   *    本任务只保留前两条里的第二条）。为什么**不复刻**那两条长度判据：
   *      它们的提示句是 `DeliveryCodeEntry::too_long_hint()` /
   *      `base_url_too_long_hint()` —— **Rust 算的**，而那两个值**没有任何命令
   *      把它们送到前端**（`state()` 的载荷里没有，也不该有）。
   *      照抄一份字面量到这里 = JS 造了一句界面文案，而且是**第二份真相**：
   *      哪天 `MAXIMUM_TEXT` 或句式改了，前端这一份**不会有任何东西变红**。
   *      只把按钮变灰、什么都不说 = 约束 4 明禁的"按钮灰着、界面不说什么"。
   *    ⇒ 本任务的取舍：**让它点得动**，把拒绝留给命令层（任务 7 会在那里调
   *      `is_sendable`），拒绝的那句话走信封回来、**原文**显示在下面那一格。
   *      这样"为什么没发出去"永远说得出来，而且说的正是 Rust 的那一句。
   */
  function syncSubmitEnabled() {
    submitEl.disabled = inFlight || currentCode() === "";
    retryEl.disabled = inFlight || currentCode() === "";
  }

  /** 上一次画出来的失败原文（`null` = 那格现在是空的）。 */
  let shownFailure = null;

  /**
   * 显示一次失败（原文逐字）。⚠️ 只写 `textContent`，不加工。
   *
   * 🔴 **同一句话不重写第二遍**：这一格里的正文是**可选中复制**的（约束 3），
   *    而它是被**每秒一次的轮询**喂的。无脑重写 `textContent` 会换掉那个文本节点
   *    ⇒ **客户正拖着选区准备复制的原文，每秒被清一次**；
   *    而那正是"失败原文要能整段发给业务方"这条要求的落点。
   */
  function showFailure(message) {
    if (shownFailure !== message) {
      setText(failureTextEl, message);
      shownFailure = message;
    }
    failureEl.hidden = false;
    retryEl.hidden = false;
  }

  function hideFailure() {
    if (shownFailure !== null) {
      setText(failureTextEl, "");
      shownFailure = null;
    }
    failureEl.hidden = true;
    retryEl.hidden = true;
  }

  /**
   * 发一条 `load`。
   *
   * ⚠️ 结果**不看这条命令的返回值**：按规格 §3.4，`load` 是"异步起 `load_delivery`，
   *    结果落 `session.load`" —— 也就是说**权威在 `state()` 那一格里**，
   *    本屏只负责把命令发出去，然后由 `render()` 被下一次轮询喂。
   *    这里只处理**发都发不出去**的那一类失败（命令层的拒绝、宿主不可用）——
   *    它们在 `session.load` 里没有落点，所以必须在这一侧显示出来，否则就是静默。
   */
  async function send() {
    const code = currentCode();
    if (code === "" || inFlight) return;
    inFlight = true;
    hideFailure();
    syncSubmitEnabled();
    try {
      // ⚠️ 形参名按 Tauri 2 的默认约定：JS 侧 camelCase（`baseUrl`）→ Rust 侧
      //    snake_case（`base_url`）。**权威在命令层**（任务 7 的 `commands.rs`），
      //    这里与规格 §3.4 表格里的 `load(code, base_url)` 是同一个函数。
      await ctx.call(ctx.CMD.load, { code, baseUrl: currentBaseUrl() });
      // 立刻补一拍：不然"点了加载"到"界面上有反应"之间要等一整秒的 state 轮询，
      // 而那一秒里用户看到的是**按了没反应**（约束 4 最恨的形态之一）。
      ctx.poller.pollNow();
    } catch (error) {
      // 失败原文**逐字**（`invoke.js` 的契约）。⚠️ 经 `failureText` 取，**不要**写
      // `error.message` —— `invoke` 的拒绝可能是一个**裸字符串**（Tauri 参数反序列化
      // 失败/命令不存在时就是这样），取 `.message` 会得到 `undefined` ⇒ 空串 ⇒
      // 界面上只有一颗没有文字的红三角（约束 3 要的原文就丢了）。
      showFailure(failureText(error));
    } finally {
      inFlight = false;
      syncSubmitEnabled();
    }
  }

  submitEl.addEventListener("click", send);
  retryEl.addEventListener("click", send);
  codeEl.addEventListener("input", syncSubmitEnabled);
  codeEl.addEventListener("keydown", (event) => {
    // 回车 = 按那颗「加载」（macOS 的 `.onSubmit(load)` 与 `.keyboardShortcut(.defaultAction)`）。
    if (event.key === "Enter") {
      event.preventDefault();
      send();
    }
  });

  function render(load) {
    // 缺载荷时按 `idle`（= 显示表单）：这是 macOS 的初值（`loadState` 从 `.idle` 起），
    // 也让"还没拿到第一拍"这一段不至于把用户挡在一颗转圈后面。见下面 mount 末尾那段。
    const kind = load && typeof load.kind === "string" ? load.kind : "idle";
    const busy = kind === "loading";

    // 进行中：输入与按钮全禁用（macOS：一次只发一条）。
    codeEl.disabled = busy;
    baseUrlEl.disabled = busy;
    // ⚠️ **不碰 `advancedEl.open`**：展开/收起是**用户**的状态（macOS 那边是
    //    `@State showAdvanced`，加载中也不会自己合上）。这里顺手把它收起来，
    //    等于"我替你做了一个你没做的动作"，而那个动作**不会有任何东西变红**。
    busyEl.hidden = !busy;

    if (kind === "failed") {
      // ⚠️ 正文是**内核原文逐字**（约束 3）：不 trim、不折行、不加前缀、
      //    不换一句更客气的话。空串也照登（`error_text.rs` 有一条测试钉着
      //    "内核给的是空串时不编一个占位出来"）。
      showFailure(typeof load.message === "string" ? load.message : "");
    } else {
      hideFailure();
    }
    syncSubmitEnabled();
  }

  // 首帧按 `idle` 画（= 表单）。⚠️ 与 `render.js:routeOfState` 的默认值**必须一致** ——
  // 不一致的话，"还没拿到第一拍"的那几帧会先画成 A、再跳成 B，
  // 而那几帧正好是用户打开窗口第一眼看到的东西。
  render({ kind: "idle" });

  return {
    render,
    unmount() {
      clear(el);
    },
  };
}
