// app.js —— **引导与路由**。整个前端从这里开始，也只有这里知道"全都怎么接起来"。
//
// 它做四件事，顺序是承重的：
//   ① 装配壳（`shell.js`）—— 之后 `#page` 才有宿主、工具栏才有事件；
//   ② 建路由：把 `state()` 的结果翻译成"哪一屏该在 `#page` 里"（`render.js` 的
//      `routeOfState` 给判据，这里只执行分派）；
//   ③ 起轮询（`poll.js`）—— **一拍 `state()`**，因为规格 §3.5 里 `tree()`/`transfers()`/
//      `verify()` 的节拍全部由"屏幕可见性"决定，而那些节拍**归各自那一屏**（任务 10/11/12）；
//   ④ 把每一拍的结果灌进壳与当前那一屏。
//
// ⚠️ 本文件**不许出现任何面向用户的字面量**（§3.2）。它出现的字符串只有命令名
//    （经 `invoke.js` 的 `CMD`）与内部标识符。

import { mountShell, SECTIONS } from "./js/shell.js";
import { applyCopy, applyCopyToTemplates } from "./js/dom.js";
import { call, CMD, failureText } from "./js/invoke.js";
import { Poller, INTERVALS_MS } from "./js/poll.js";
import { screenFor } from "./js/screens/registry.js";
import { openSwitchcode } from "./js/switchcode.js";
import { openSettings } from "./js/settings.js";
import { openModal } from "./js/dialogs.js";
import {
  noticesFromState,
  engineBadgeFromState,
  summaryFromState,
  codeLabelFromState,
  routeOfState,
  gateFromState,
  updateNoticeFromStatus,
} from "./js/render.js";


// ---------------------------------------------------------------------------
// 首启语言询问（规格 W1）
// ---------------------------------------------------------------------------
//
// ⚠️ 判据**不在前端**：`state().language_chosen` 是内核看设置文件说的
//    （"客户/壳有没有明确选过这一格"）——前端若自己按"语言是不是 system"去猜，
//    主动选了「跟随系统」的客户每次启动都会被再问一次。
// ⚠️ 文案**全部来自 Rust**（`settings_get` 的 `language` 那一格）：标题、说明、
//    三个选项名。前端只摆位置（§3.2）。
let languageAsked = false;

async function maybeAskLanguage(state) {
  if (languageAsked) return;
  if (!state || state.language_chosen !== false) return;
  languageAsked = true;
  let payload;
  try {
    payload = await call(CMD.settingsGet);
  } catch {
    // 问不到就**不打扰**：语言这一格晚一点再选也不影响下载（首启弹窗不是阻塞路径）。
    return;
  }
  const spec = payload && payload.language ? payload.language : null;
  if (!spec || !Array.isArray(spec.choices) || spec.choices.length === 0) return;

  const modal = openModal("tpl-window-language");
  const title = modal.root.querySelector("#lang-title");
  const note = modal.root.querySelector("#lang-note");
  const row = modal.root.querySelector("#lang-row");
  if (title) title.textContent = spec.title;
  if (note) note.textContent = spec.note;
  if (!row) return;
  let busy = false;
  for (const choice of spec.choices) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "set__lang";
    button.setAttribute("role", "radio");
    button.setAttribute(
      "aria-checked",
      String(choice.value) === String(spec.value) ? "true" : "false"
    );
    button.textContent = choice.label;
    button.addEventListener("click", async () => {
      if (busy) return;
      busy = true;
      for (const b of row.querySelectorAll("button")) b.disabled = true;
      try {
        // 整份设置回传（内核是整份替换），只改语言那一格。
        await call(CMD.settingsSet, {
          settings: { ...payload.settings, language: choice.value },
        });
        modal.close();
        // 立刻重取一拍：`language_chosen` 变成 true，界面文案跟着内核那一档换。
        await tickState();
      } catch (error) {
        busy = false;
        for (const b of row.querySelectorAll("button")) b.disabled = false;
        showTransientFailure(failureText(error));
      }
    });
    row.append(button);
  }
}

// ---------------------------------------------------------------------------
// 装配
// ---------------------------------------------------------------------------

const shell = mountShell({
  onSectionChange: () => {
    // ⚠️ 换分区**不重挂**：当前那一屏的 `unmount()` 由路由在"该换屏了"的时候调
    //    （见下面 `applyRoute`）。这里只是把选择记在壳上 —— 换分区之后该显示什么，
    //    由下一拍 `state()` 的路由结果说了算（与 macOS 的 `NavigationSplitView` 同形：
    //    侧栏永远可点，但没加载批次时主区仍是空态页）。
    // TODO(任务 10/11/12)：这三个屏的节拍在这里启停 —— 见 `applyRoute` 的注释。
  },
  onRetry: () => {
    // 常驻提示行那颗「重试」= 重连内核（`EngineBanner.shows_retry` 为真的那一支）。
    // ⚠️ 它**不看闸门**：闸门（`allows_requests`）本身就是因为引擎不可用才关上的，
    //    而这一颗正是"把它救回来"的唯一入口 —— 被闸门挡住就永远解不开。
    void call(CMD.retry)
      // ⚠️ 取原文一律经 `failureText`（`invoke.js`）：`invoke` 的拒绝**不一定是 `Error`**
      //    —— 裸字符串取 `.message` 得 `undefined` ⇒ 界面上是一个没有字的红三角。
      .catch((error) => showTransientFailure(failureText(error)))
      .finally(() => poller.pollNow());
  },
  /**
   * 工具栏那颗**下载**按钮（任务 13 接上的）。
   *
   * 🔴 它做的事只有一件：**问当前那一屏要 `run()`，然后调它**。
   *    那颗按钮长什么样（文案 / 启用态）也一样由屏回答（`toolbarDownload()`，
   *    契约在 `js/screens/registry.js` 的文件头），壳只负责摆位置。
   *    ⚠️ 本函数**不是**第二个真相源：它用的是**上一拍**算出来的那个描述符
   *    （`toolbarDownload`），所以屏幕上显示什么、点下去就执行什么 ——
   *    不会出现"按钮写着 A、点下去做 B"。
   */
  onDownloadClick: () => {
    const desc = toolbarDownload;
    if (!desc || desc.enabled !== true) return;
    // ⚠️ `run()` 由**屏**实现（只有它知道勾选面在哪）。它自己接住自己的失败
    //    （`files.js:enqueue` 把失败落成一条回执），所以这里不套 `try`。
    desc.run();
  },
  /**
   * 工具栏那颗**交付码**按钮 = 打开换码面板（规格 §2.1 第 8 项）。
   * ⚠️ **同一时刻只开一扇**：面板是模态的，第二扇盖在第一扇上没有任何意义，
   *    而它会让"关掉一扇之后 `app.js` 那一格指着谁"变成一个要现场推的问题。
   */
  onCodeClick: () => {
    if (codePanel) return;
    codePanel = openSwitchcode(
      { call, CMD, poller },
      {
        // 输入框的初值 = **当前生效批次的码**（macOS 的 `onAppear` 那一条）。
        // ⚠️ 从**最近一次成功的 `state()`** 取，而不是当场再发一条请求：
        //    打开一扇窗不该多出一条命令（那一条的答案一秒前刚拿到）。
        initialCode: loadedCode(),
        onReceipt: pushReceipt,
        // ⚠️ 清"现在开着的是谁"这一格**必须**挂在这条回调上（不是自己记一份
        //    "我调过 close 没有"）：面板在**四条路**上会关掉 —— 取消、Esc、
        //    点背影、以及**换码成功之后自己关**。漏掉最后一条的表现是
        //    "面板没了、但再点工具栏那颗交付码按钮**打不开**"。
        onClose: () => {
          codePanel = null;
        },
      }
    );
  },
  /** 工具栏那颗**设置**按钮 = 打开设置窗口（规格 §2.1 第 9 项）。同样只开一扇。 */
  onSettingsClick: () => {
    if (settingsWindow) return;
    settingsWindow = openSettings(
      { call, CMD, poller, shell },
      {
        onReceipt: pushReceipt,
        // ⚠️ 窗口里那颗 × 收的是**同一件事**（两个落点读同一个值）——
        //    主区那条常驻回执也要跟着收，否则用户会以为"没收掉"。
        onReceiptDismiss: () => dismissReceipt(DIR_CHANGE_RECEIPT_ID),
        onClose: () => {
          settingsWindow = null;
        },
      }
    );
  },
  /**
   * 更新提示条那颗「去下载」（规格 §4）：把**内核给的那条链接**交给系统默认浏览器。
   *
   * 🔴 **白名单在 Rust 那条命令里判**（`open_update_url`），前端**一个字都不判** ——
   *    前端是可被注入的那一侧，承诺它自己会判等于没有承诺（R15）。
   * ⚠️ 地址用的是**上一次摆进那一格的**那一条（`shell.setUpdateNotice` 写下的
   *    `dataset.url`），不是当场重算：屏幕上写着哪个版本、点下去就去下哪个版本。
   * ⚠️ 失败**上屏**（不留静默）：它只有两种来源 —— 那条地址不在白名单里（Rust 拒绝）、
   *    或者系统没能把浏览器拉起来。两者对客户都是"你点了、什么都没发生"，
   *    而那是本项目最忌的那一类（约束 4）。走**回执**（不是瞬时错误）：它是
   *    "你刚才那个动作的结果"那类，由客户自己收起。
   */
  onUpdateDownload: (url) => {
    if (typeof url !== "string" || url === "") return;
    void call(CMD.openUpdateUrl, { url }).catch((error) => {
      pushReceipt({
        id: UPDATE_OPEN_RECEIPT_ID,
        kind: "error",
        icon: "exclamationmark.triangle.fill",
        text: failureText(error),
        hint: null,
        retry: false,
        dismiss: true,
      });
    });
  },
  /**
   * 更新提示条那颗「收起」：**本次运行内**不再出现（规格 §4）。
   *
   * ⚠️ 这一格记在**本文件**（不是把那个节点就地删掉）：提示条每一拍都会重画，
   *    删节点只能躲过"下一拍之前"的那一瞬间 —— 与本文件记 `localReceipts` 同一条理由。
   * ⚠️ 不落盘、不进 localStorage、不记"被忽略的版本"（规格 §4 明确否掉了后者）：
   *    下次启动会再出现一次，那是**有意**的。
   */
  onUpdateDismiss: () => {
    dismissedUpdate = true;
    paintUpdateNotice();
  },
  onNoticeDismiss: (id) => dismissReceipt(id),
});

const poller = new Poller();

/** 当前挂着的那一屏（`{ id, controller }`）。**同一时刻至多一个**。 */
let activeScreen = null;

/**
 * **上一拍**从当前那一屏问来的"工具栏那颗下载按钮该长什么样"。
 * `null` = 这一屏没有下载这个动作（壳把按钮禁用并清空文字）。
 *
 * 🔴 它是"屏 → 壳"那条链上**唯一**的中转站，而且**只由 `syncToolbarDownload` 写**。
 *    点击处理器读它（而不是当场再问一次屏）是为了让"屏幕上显示什么"与
 *    "点下去执行什么"来自**同一次**问询 —— 分开问两次就可能出现
 *    "按钮写着 A、点下去做 B"（中间隔着一拍，勾选面已经变了）。
 *
 * @type {{title: string, enabled: boolean, run: Function}|null}
 */
let toolbarDownload = null;

/** 现在开着的换码面板（`null` = 没开）。**同一时刻至多一扇**。 */
let codePanel = null;
/** 现在开着的设置窗口（`null` = 没开）。 */
let settingsWindow = null;

/**
 * **壳自己记着的回执**（任务 13）：历史写盘失败 / 改下载目录回执。
 *
 * ⚠️ 为什么它们不在 `state()` 里：这两件事都是"**你刚才那个动作的结果**"，
 *    而它们的值只存在于**那一次命令的返回值**里（`history_put` 的失败原文、
 *    `preferences_set` 的四格回执）—— 会话总览里根本没有这两格。
 *    这也是 `render.js:noticesFromState` 头部那段"③④⑤ 的生产者在任务 13"的落点。
 *
 * ⚠️ **常驻提示行那一排今天只接上了 ③⑤ 里的两条，第 ③ 条（换码结果）仍然空着**：
 *    macOS 的「已换到批次 X」是 `DeliverySwitch::notice_text` 算的，而那个类型
 *    **不在任何载荷里**（`load()` 命令回的是 `{status}`，没有那句话）。
 *    前端不许自己拼（§3.2）⇒ 本任务**不编**，如实记在报告里。
 *    今天用户看到的是"新批次自己出现了"（面板关掉、工具栏的码换了、文件页换了那一批）——
 *    与 macOS 相比少一句来源说明，而那句话的缺口在**命令层**（要 `load()` 带上
 *    `DeliverySwitch`，或让 `state()` 多一格），不是在前端。
 *
 * ⚠️ **每条必须带 `id`**：带 `id` 的那些走 `shell.js:noticeRow` 的 `onNoticeDismiss`
 *    出口（收起来时从这一份数组里删掉）；不带 `id` 的只能就地 `remove()`，
 *    而那种删法会在列表键下一次变化时**复活**。
 *
 * ⚠️ **顺序 = 显示顺序，加在末尾**：`render.js` 那一段逐字钉着"新的一律是
 *    '你刚才那个动作的结果'那一类 ⇒ 加在末尾"（引擎横幅永远在最上面）。
 *
 * @type {Array<object>}
 */
let localReceipts = [];

/** 改下载目录那条回执的 id（`settings.js` 用它，`app.js` 的"收起"出口也用它）。 */
const DIR_CHANGE_RECEIPT_ID = "download-dir-change";

/** 「去下载」没打开浏览器那条回执的 id（同一个 `id` 只留一条，后到的顶掉先前的）。 */
const UPDATE_OPEN_RECEIPT_ID = "update-open-failed";

/** 当前生效批次的码（`state().load.summary.code`；没有生效批次时是空串）。
 *  ⚠️ **不编占位符**：取不到就是空串（同 `files.js:codeText` 与
 *  `shell.js:setCodeLabel` 那条口径 —— `—` 也是一个字）。 */
function loadedCode() {
  const summary = summaryFromState(lastState);
  return summary && typeof summary.code === "string" ? summary.code : "";
}

/** 记下一条壳自己产生的回执（同一个 `id` 只留一条，后到的顶掉先前的）。 */
function pushReceipt(desc) {
  if (!desc || typeof desc.id !== "string") return;
  localReceipts = localReceipts.filter((row) => row.id !== desc.id).concat([desc]);
  shell.setNotices(currentNotices());
}

/** 收起一条壳自己产生的回执（`shell.js` 那颗 × 与设置窗口里那颗 × 都走这里）。 */
function dismissReceipt(id) {
  const before = localReceipts.length;
  localReceipts = localReceipts.filter((row) => row.id !== id);
  if (localReceipts.length === before) return;
  shell.setNotices(currentNotices());
}

// ---------------------------------------------------------------------------
// 更新提示（规格 §4）：**壳级的那一条**
// ---------------------------------------------------------------------------

/** 最近一次成功拉回来的 `update_status` 载荷（`null` = 还一次都没成功过）。 */
let lastUpdateStatus = null;

/**
 * 🔴 **本次运行内客户点过「收起」**（规格 §4：「关掉之后，本次运行内不再出现 ——
 * 后续的每日检查也不会把它叫回来，直到下次启动」）。
 *
 * ⚠️ **模块级、住在本文件**（不是 `shell.js`、也不是哪一屏）：提示条现在挂在壳上，
 *    换屏（去传输列表看一眼再切回来）不是"重启应用"，收起要跨过换屏活着。
 * ⚠️ **不落盘、不进 localStorage、不记"被忽略的版本"**（规格 §4 明确否掉了后者）：
 *    下次启动会再出现一次，那是**有意**的。
 */
let dismissedUpdate = false;

/**
 * 把"这一拍知道的更新状态"画到壳上。
 *
 * ⚠️ **判据在 `render.js:updateNoticeFromStatus`**（只有 `has_newer === true` 才算"有新版本"，
 *    R20），这里只把**收起过没有**叠上去 —— 收起是前端这一侧的事
 *    （规格 §4 说它是"本次运行内"的，内核对它一无所知）。
 */
function paintUpdateNotice() {
  shell.setUpdateNotice(dismissedUpdate ? null : updateNoticeFromStatus(lastUpdateStatus));
}

/**
 * 更新提示那一拍（`update_status`，宽节拍 60 s）。
 *
 * 🔴 **它挂在壳上、不挂在任何一屏上**（0.2.8 修的一场）：macOS 那条提示挂在
 *    `RootView` 的主区里，**任何分区都看得见**；而这一条原来长在文件页模板里、
 *    它那一拍也归文件页 ⇒ 启动落在空态页、或者客户正站在传输列表上时，
 *    它**永远不出现** —— "Windows 客户端没有更新提示"正是这么来的。
 *
 * ⚠️ **失败一律静默**（规格 §5）：内核在"取不到"时本就回成功（`has_newer:false`），
 *    所以这一支的失败只可能是"没有内核 / IPC 断了"——那是**别处**（`state()` 的节拍）
 *    已经在说的一件事。这一条一个字都不上屏，也**不动上一次的结论**：
 *    把已有的结论擦掉比留着它更不诚实 —— 我们没有新消息，不等于"没有新版"。
 */
async function tickUpdate() {
  let status;
  try {
    status = await call(CMD.updateStatus);
  } catch (error) {
    return;
  }
  lastUpdateStatus = status;
  paintUpdateNotice();
}

/**
 * 问**当前那一屏**要一次"工具栏那颗下载按钮该长什么样"，并把它喂给壳。
 *
 * 🔴 这一条是任务 13 接上那颗按钮的地方。修之前 `setDownloadEnabled` /
 *    `setDownloadTitle` 的调用点**各为 0** ⇒ 它永远停在 HTML 里的 `disabled`、
 *    标题那一格永远是空的 —— 即"**一个禁用且没有文字的按钮**"。
 *
 * ⚠️ **判据在屏身上，本函数一条都不判**（契约逐字）：
 *    · 屏没写 `toolbarDownload`（`empty` / `verify` 就没有）与返回 `null` **同义**；
 *    · 屏抛出来的东西**不许静默吞掉**：那是一次"程序自己坏了"，
 *      按 `applyRoute` 的同一条处置挂到常驻提示行上（下一拍成功时自动清掉）。
 */
function syncToolbarDownload() {
  let desc = null;
  const controller = activeScreen ? activeScreen.controller : null;
  if (controller && typeof controller.toolbarDownload === "function") {
    try {
      desc = controller.toolbarDownload();
    } catch (error) {
      showTransientFailure(failureText(error));
      desc = null;
    }
  }
  toolbarDownload =
    desc && typeof desc.title === "string" && typeof desc.run === "function"
      ? { title: desc.title, enabled: desc.enabled === true, run: desc.run }
      : null;
  // ⚠️ 交进壳的那两格：**空串标题 = 那一格不渲染**（`shell.setDownloadAction`
  //    的注释：本函数不兜底、不猜），`enabled` 只有真的是 `true` 才算可用。
  shell.setDownloadAction({
    title: toolbarDownload ? toolbarDownload.title : "",
    enabled: toolbarDownload !== null && toolbarDownload.enabled === true,
  });
}

// ---------------------------------------------------------------------------
// 侧栏那三个未完成计数徽标（`SidebarBadge`）
// ---------------------------------------------------------------------------

/**
 * 三个分区各自的未完成计数。`0` = 这一格不渲染（macOS 的 `.badge(0)` 什么都不画）。
 *
 * 🔴 **它为什么住在壳这一层、而且必须活过一次换屏**：这三个数是**导航级**的提示
 *    （"传输列表还有 3 个没落定的" —— macOS 上这是"去那一屏看看"的唯一入口），
 *    而每一个数只有**那一屏自己的载荷**才有（`tree()`/`transfers()`/`verify()`
 *    三条命令各答各的）。屏换掉时它的闭包跟着消失 ⇒ 数只能由壳记着。
 *
 * ⚠️ **它与 macOS 的口径逐字同源（包括"什么时候会旧"）**：那边读的是
 *    `AppModel.transfers` / `AppModel.verify`，而那两份快照**只在各自那一屏可见时**
 *    被刷新（`RootView.pollLoop` 的闸是 `section == .transfers`；校验那一份由
 *    `VerifyView` 自己刷）⇒ 徽标反映的本来就是"最后一次看到它时的样子"。
 *    本代照搬这条口径：屏把**载荷里 Rust 算好的那一格**交上来，壳只把它摆到徽标上。
 *
 * ⚠️ **计数一个都不许在 JS 里算**（规格 §3.2）：`counts_as_unfinished` 与
 *    `failed_count` 两条判据都在 `presentation::verify_summary::SidebarBadge`，
 *    命令层把它们随各自那一屏的载荷发下来。
 *
 * ⚠️ **换批要清零**（见 `syncNavBadges` 里那段）：上一批的"还有 3 个没落定的"
 *    摆在**这一批**的侧栏上，是一个没有任何提示的错数。
 *
 * @type {{files: number, transfers: number, verify: number}}
 */
const navCounts = { files: 0, transfers: 0, verify: 0 };

/** 上面那三个数是**哪一批**的（`null` = 还没有生效批次）。换批 ⇒ 全部清零。 */
let navCountsCode = null;

/**
 * 把当前那一屏报上来的未完成计数收下，并喂给壳的三个徽标。
 *
 * ⚠️ **与 `syncToolbarDownload` 同一个形状、同一个位置**：屏回答、壳摆位置，
 *    `app.js` 一个判据都不重写（契约见 `screens/registry.js` 的 `navBadge()` 那一条）。
 *    ⚠️ 屏抛出来的东西**不许静默吞掉**（同那条的处置）：那是一次"程序自己坏了"。
 */
function syncNavBadges() {
  const controller = activeScreen ? activeScreen.controller : null;
  if (controller && typeof controller.navBadge === "function") {
    try {
      const desc = controller.navBadge();
      if (desc && typeof desc.count === "number" && SECTIONS.includes(desc.section)) {
        navCounts[desc.section] = desc.count;
      }
    } catch (error) {
      showTransientFailure(failureText(error));
    }
  }
  // ⚠️ **三个都喂**（不是只喂刚才那一屏）：另外两个分区的数是**壳记着的**，
  //    换屏之后必须原样留在侧栏上（"离开传输列表之后那颗徽标就没了"是错的）。
  for (const section of SECTIONS) shell.setNavBadge(section, navCounts[section]);
}

/**
 * 「这一拍之前那条瞬时错误」——它没有别的落点。
 *
 * ⚠️ 为什么这一格**在前端**而 `state().last_error` 在两处不同：那一格是**内核侧**
 *    （`SessionView.last_error`）的非粘滞错误；而这一格装的是"**连 `state()` 都没调成**"
 *    的那一类（宿主里没有这条命令、IPC 断了）。后者**根本进不了 `state()` 的载荷**
 *    （请求发不出去），所以它只能由发出请求的这一侧记着。
 * ⚠️ 它会在**下一拍成功时自动清掉**（非粘滞）：一次可自愈的抖动不该在界面上留一辈子。
 */
let localFailure = null;

/**
 * 最近一次**成功**的 `state()` 载荷（`null` = 还一次都没成功过）。
 * 它只用于"这一拍要显示哪些常驻提示行"（`currentNotices`）——
 * ⚠️ **不是**一个缓存了所有东西的状态中心：壳与屏的每一格都仍然只从**这一拍**的载荷来。
 *    留着它，是因为"连 `state()` 都没调成"的那一拍里，**别的来源全都已经是旧的了**，
 *    而提示行不能因此整排消失（那会让一次抖动看起来像"故障自己好了"）。
 */
let lastState = null;

function showTransientFailure(message) {
  localFailure = typeof message === "string" ? message : "";
}

// ---------------------------------------------------------------------------
// 路由
// ---------------------------------------------------------------------------

/**
 * 按 `state()` 的路由结果决定 `#page` 里挂哪一屏。
 *
 * ⚠️ **换屏只在"屏 id 真的变了"时发生**：不然每一拍都会 `unmount()` + `mount()` 一次，
 *    而那会把用户正在敲的半句话、滚动位置、焦点全部丢掉 —— 每秒一次。
 *    这是一种**只看代码看不出来、只在真机上用手感发现**的缺陷。
 */
function applyRoute(route, loadState) {
  const wanted = route.screen === "empty" ? "empty" : shell.getSection();
  if (!activeScreen || activeScreen.id !== wanted) {
    let mod;
    try {
      mod = screenFor(wanted);
    } catch (error) {
      // 屏没注册（任务 10/11/12 还没落地，或有人的注册表少了一行）。
      // ⚠️ **不许静默**：这一支如果只是 `return`，用户看到的是"加载成功之后主区一片空白"
      //    ——而没有一句话说为什么。挂到常驻提示行上，它会在修好之前一直挂着。
      showTransientFailure(failureText(error));
      shell.setNotices(currentNotices());
      return;
    }

    // -----------------------------------------------------------------------
    // 🔴 **先把新屏挂好，再拆旧屏** —— 顺序是承重的（这是一处修出来的缺陷）。
    //
    // 修之前是"先 `unmount()` → 再 `replaceChildren()` → 再 `mount()`"：
    // 于是 `mount()` 一旦抛（或上面 `screenFor` 抛），`#page` 里已经是一个**空的**
    // host、而 `activeScreen` 还指着那个已经被拆掉的旧屏 ⇒ **主区永久空白**，
    // 顶上只有一条提示行。任务 7→10 之间这条路径**是可达的**（`state()` 一旦能答
    // `load.kind === "loaded"`，路由就会去找 `files` 那一屏，而它在任务 10 之前没注册）。
    //
    // ⇒ 拆旧屏之前先确认新屏**真的挂起来了**。挂不上就**原样留着旧屏**（用户至少还有
    //   上一次那个界面），并且 `activeScreen` 保持旧值 ⇒ 下一拍会**自动重试**。
    //   这一条与"不许静默降级"不冲突：提示行上同时挂着那句诊断，不是悄悄不动。
    // -----------------------------------------------------------------------
    const host = document.createElement("div");
    host.className = "screen";
    let controller;
    try {
      // ⚠️ 挂进一个**还在文档外**的 host：屏自己的挂载点必须用 `byIdIn(root, …)` 查
      //    （不是 `document.getElementById`）—— 见 `dom.js` 里那一条的注释。
      controller = mod.mount(host, { call, CMD, poller, shell });
    } catch (error) {
      showTransientFailure(failureText(error));
      shell.setNotices(currentNotices());
      return; // 旧屏原样还在（`unmount()` 还没调过）
    }
    // 新屏已经挂好了 ⇒ **现在**才拆旧屏、才换 DOM、才改 `activeScreen`。
    if (activeScreen) {
      // 换屏前先停掉**本屏**挂上的节拍。屏不许自带 `setInterval`（`registry.js`
      // 的契约第 3 条），所以这里只需要把 `state` 之外的任务拢一遍：
      // `state` 是壳自己的（它不属于任何一屏、也不该跟着屏停）。
      // TODO(任务 10/11/12)：屏名与节拍名的对应关系在各自那一屏里定，
      // 在这里加一行 `poller.stop(<节拍名>)` 即可。
      activeScreen.controller.unmount();
    }
    shell.page.replaceChildren(host);
    activeScreen = { id: wanted, controller };
  }
  activeScreen.controller.render(loadState);
}

// ---------------------------------------------------------------------------
// 每一拍
// ---------------------------------------------------------------------------

/**
 * 一拍 `state()`。
 *
 * ⚠️ 它**不看闸门**（`ungated = true`，见 `poll.js` 的 `start` 第 4 个参数）：
 *    闸门本身就是这一格报出来的（`state().allows_requests`），
 *    它自己再被闸门挡住就永远解不开了（规格 §3.5 的闸门明文把 `state`/`retry` 排除在外）。
 */
let paintedLang = null;

async function tickState() {
  let state;
  try {
    state = await call(CMD.state);
  } catch (error) {
    // 失败**不许静默**（W-2）：挂到常驻提示行，下一次成功自动清掉。
    showTransientFailure(failureText(error));
    // ⚠️ **不动路由**：这一拍没拿到数据，不等于"现在没有生效批次"。
    //    把用户正看着的文件页拆掉换成一个空态页，是拿"我们没收到更新"去冒充
    //    "这个世界变了" —— 而那正是本项目反复记账的那类错误。
    //    （首帧的路由在文件末尾那一次 `applyRoute` 里已经画过了，这里不会出现白屏。）
    shell.setNotices(currentNotices());
    return;
  }
  // 成功了 ⇒ 清掉"连不上"那一格（非粘滞），但**不清** `state()` 自己带回来的
  // `last_error` —— 那一格由 Rust 决定什么时候消失（`SessionView` 的语义）。
  localFailure = null;
  lastState = state;
  // 静态文案（侧栏/工具栏/表头/模板里的按钮）全部由 Rust 按 `state.lang` 成句，
  // 这里只把 `data-copy*` 标记的位置填上。模板先填，之后克隆出来的屏才不是空的。
  applyCopy(document, state.copy);
  applyCopyToTemplates(state.copy);
  if (typeof state.lang === "string") {
    // 语言变了 ⇒ 整页重载一次。为什么不是"就地重画"：译文装在**上一次载荷**里
    // （文件树的底栏按钮、换码历史的行、几处一次性的回执），重画本地缓存只会把
    // 旧语言再贴一遍 —— 那份"英文界面里夹着中文"的现场就是这么来的。
    // 重载后 `paintedLang` 从同一个值重新起算，不会自己转圈。
    if (paintedLang !== null && paintedLang !== state.lang) {
      location.reload();
      return;
    }
    paintedLang = state.lang;
    document.documentElement.lang = state.lang === "en" ? "en" : "zh-CN";
  }
  // 首启语言询问（规格 W1）：判据是 `language_chosen === false`（**设置文件里有没有
  // 那一格**，由内核在握手时说的）。只问一次；问过之后由壳自己记住本次会话不再问。
  void maybeAskLanguage(state);

  poller.setGate(gateFromState(state));

  const route = routeOfState(state);
  applyRoute(route, route.screen === "empty" ? route.load : state.load);

  shell.setBatchSummary(summaryFromState(state));
  shell.setCodeLabel(codeLabelFromState(state));
  // 工具栏那颗下载按钮：**问当前那一屏**（任务 13 接上的那一条链）。
  //   标题是 `DownloadTargets::button_title`（Rust）算的、动作是屏自己实现的 ——
  //   `app.js` 与 `shell.js` 都**不判**"这一屏该不该有下载动作"，只负责问与摆。
  //   ⚠️ 必须在 `applyRoute` **之后**调：换屏那一刻 `activeScreen` 才换成新的那一屏，
  //      先问就会拿到上一屏的答案（"按钮写着文件的文案、屏幕上却是校验页"）。
  syncToolbarDownload();
  // 🔴 **换批 ⇒ 侧栏那三个计数一起清零**（它们说的都是"上一批"的事）：
  //    上一批那句「还有 3 个没落定的」摆在**这一批**的侧栏上，是一个没有任何提示的
  //    错数 —— 判据与 `ProgressSummary::of` 的 `tree_code == code`、
  //    `verify.js:render` 的 `shownCode` 是**同一条**（macOS 侧是
  //    `performLoadDelivery` 里的 `verify = nil`）。
  //    ⚠️ 清零之后**当前那一屏**在同一拍就把自己那一格报回来（只要它手上已经是
  //    这一批的载荷）；报不回来就说明它手上还是上一批的 —— 那时**不显示**才是对的。
  const batchCode = loadedCode();
  const batchKey = batchCode === "" ? null : batchCode;
  if (batchKey !== navCountsCode) {
    navCountsCode = batchKey;
    for (const section of SECTIONS) navCounts[section] = 0;
  }
  syncNavBadges();
  shell.setEngineBadge(engineBadgeFromState(state));
  shell.setNotices(currentNotices());

  // 任务 13 的三扇窗：**每一拍把载荷喂进去**（与屏的 `render(payload)` 同一个位置）。
  // ⚠️ 只喂，不替它们做判断 —— 它们各自只从载荷里取自己要的那一格
  //    （换码面板取 `load`，设置窗口取 `allows_requests`）。
  if (codePanel) codePanel.render(state.load);
  if (settingsWindow) settingsWindow.render(state);
}

/**
 * 这一拍该显示的常驻提示行。
 *
 * ⚠️ **三处**来源合并在这里（顺序即显示顺序）：
 *    · `state()` 自己的（引擎横幅 / W-2 披露）；
 *    · **本文件记着的那一格**（`localFailure`，连 `state()` 都没调成的那种）——
 *      它排在最前：那一类失败意味着"连会话总览都拿不到"，
 *      而它下面的每一条**此刻都已经是旧的了**；
 *    · **壳自己记着的回执**（`localReceipts`，任务 13）—— 换码结果 / 历史写盘失败 /
 *      改下载目录回执。⚠️ 它们排在**最后**（`render.js` 那段逐字钉着的顺序：
 *      "新的一律是'你刚才那个动作的结果'那一类 ⇒ 加在末尾"）。
 *      ⚠️ 这三条的**正文不在载荷里**（它们只存在于那一次命令的返回值中），
 *      所以"它们该在不在"这件事由本文件而不是由 `render.js` 决定 ——
 *      而 `render.js` 那三条槽位仍然空着（它只映射载荷）。
 */
function currentNotices() {
  const out = [];
  if (localFailure !== null) {
    out.push({
      kind: "error",
      icon: "exclamationmark.triangle.fill",
      text: localFailure,
      hint: null,
      retry: false,
      dismiss: false,
    });
  }
  return out.concat(lastState ? noticesFromState(lastState) : [], localReceipts);
}

// ---------------------------------------------------------------------------
// 起步
// ---------------------------------------------------------------------------

// ⚠️ **一拍都不发之前先画一遍**：`state()` 要一跳 IPC 才回来，而窗口是先显示的
//    ——不先画的话，用户看到的第一帧是一整片白，而"白屏"与"崩了"在客户眼里一样。
//    这一帧的载荷是**空**的：壳画成"什么都没有"，屏画成 `idle`（表单），
//    两侧的默认值在 `render.js:routeOfState` 与 `screens/empty.js` 里逐字对齐。
applyRoute({ screen: "empty" }, { kind: "idle" });
// 工具栏那颗下载按钮也要有首帧：空态屏没有 `toolbarDownload()`（这一屏没有下载动作）
// ⇒ 那颗按钮**禁用且清空文字**。⚠️ 不写这一句的话，它会停在 `index.html` 里的
// 初始形态 —— 而那正好也是"禁用 + 空文字"，所以这一句的作用不是改观感，
// 而是**让那一格从第一帧起就由显式的判据说了算**（而不是由 HTML 的初始属性兜着）。
syncToolbarDownload();
// 侧栏那三个徽标也要有首帧：此刻一个数都没有（`navCounts` 全 0）⇒ **三个都藏着**。
// ⚠️ 与 macOS 逐字同源：那边首帧读的是 `AppModel.transfers/verify`，两者都还是 `nil`
//    ⇒ `SidebarBadge` 给 0 ⇒ `.badge(0)` 什么都不画。这一句的作用不是改观感，
//    而是让那三格从第一帧起就由**显式的判据**说了算（而不是由 HTML 的 `hidden` 兜着）。
syncNavBadges();
shell.setNotices(currentNotices());
// 更新提示条的首帧：此刻还没有任何结论（`lastUpdateStatus === null` ⇒ 判据给 `null`）
// ⇒ **藏起来**。这一句的作用与前两句相同：让那一格从第一帧起就由**显式的判据**说了算，
// 而不是由 `index.html` 上的那个 `hidden` 兜着（它已经从文件页搬到了壳上）。
paintUpdateNotice();

// ④ 起轮询。**只有这一条** —— `transfers()` 的 200 ms 与 `verify()` 的 1 s 归
//    各自那一屏（任务 11/12），它们**只在那一屏可见时**才该跑（规格 §3.5）。
//    ⚠️ 第 4 个参数 `true` = **不看闸门**（`state` 与 `retry` 是仅有的两个例外）。
poller.start(CMD.state, INTERVALS_MS.state, tickState, true);

// ⑤ 更新提示那一拍：**壳自己的**（不归任何一屏，理由见 `tickUpdate`）。
//    ⚠️ 宽节拍（60 s，`INTERVALS_MS.update_status`）：内核自己把它节流到 24 小时，
//       这一拍只是把结果拿回来画。首拍立刻跑（`poll.js:start` 的既有语义）
//       ⇒ 开窗一秒钟内就知道有没有新版，不用等一整分钟。
//    ⚠️ **受闸门管**（不传 `ungated`）：引擎不可用的时候不去打内核 ——
//       与它原先挂在文件页上时的口径一致（`poll.js:_fire` 第二条）。
poller.start(CMD.updateStatus, INTERVALS_MS.update_status, tickUpdate);

// ⑥ 自动重启那一拍（规格 W3）：**壳自己的**，与上一条同理由（任何分区都看得见）。
//    它是只读的装饰命令：拿不到（没有内核 / 老内核不认这个方法）⇒ 什么都不说，
//    绝不把一次读失败摆成红条 —— 那时 `state()` 那条节拍本来就在说更准的话。
poller.start(CMD.restartProgress, INTERVALS_MS.restart_progress, tickRestart);

/**
 * 自动重启那一拍（`restart_progress`，3 s）。
 *
 * 句子在 Rust（`presentation::restart_notice`），这里只把拿到的 `text` 摆上壳级条。
 * `text` 为 `null`（内核说 `idle`）⇒ 收起那条 —— "没有在重启"就不该有横幅。
 */
async function tickRestart() {
  let payload;
  try {
    payload = await call(CMD.restartProgress);
  } catch (error) {
    return;
  }
  shell.setRestartNotice(payload && typeof payload.text === "string" ? payload : null);
}

// 关窗/刷新时停掉全部节拍。⚠️ 这一步**不是**为了省电：Tauri 里 webview 一销毁，
// 在飞的 `invoke` 会以一次拒绝收场，而那一次拒绝会走 `showTransientFailure` ⇒
// 往一个已经不存在的 DOM 里写东西。停在这里最省事，也最诚实。
window.addEventListener("pagehide", () => poller.stopAll());
