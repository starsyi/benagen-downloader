// switchcode.js —— **换码面板**（规格 §2.1 第 8 项）：历史批次列表 + 备注就地编辑 +
// 点一行直接切换。
//
// 对位 `macos/Sources/BenagenDownloader/Views/SwitchDeliverySheet.swift`。
//
// ---------------------------------------------------------------------------
// 🔴 本文件里没有一句面向用户的字符串（规格 §3.2）
// ---------------------------------------------------------------------------
// 会显示的字只有两个来源，与三个屏**逐字同一条分工**：
//   ① **数据** —— `BatchHistoryRow` 的 `title` / `code` / `note` / `time_text` /
//      `base_url`、以及失败时的 `state().load.message`（内核/壳原文逐字）。
//      **逐字落位，不 trim、不折叠、不在空值上兜一句话**；
//   ② **结构文案** —— 与数据无关、任何状态下都一样的那些字（面板标题、那句后果说明、
//      分组名、按钮名、输入框占位符、行尾那句提示）。它们住在 `index.html` 的
//      `tpl-panel-switchcode` / `tpl-switchcode-row` 里，本文件只**克隆与摆放**。
//
// ⚠️ **注释里的中文不算**：§3.2 的守卫（`check_frontend_copy.sh`）扫的是**剥掉注释后**
//    的字面量。本文件里那些解释"为什么"的长注释是写给人看的，不是要渲染的东西。
//
// ---------------------------------------------------------------------------
// 它为什么不是一个"屏"（`js/screens/` 下那些）
// ---------------------------------------------------------------------------
// 三个屏挂在 `#page` 里、**互斥**（换屏 = 拆掉旧的），而这一扇是**压在窗口上**的
// 模态面板：打开它的时候文件页还在底下、还在跑自己那一拍。它没有节拍
// （`ctx.poller` 一次都不用），所以屏契约第 3 条在这里是**天然成立**的 ——
// 它连 `poller` 都不需要。
//
// ---------------------------------------------------------------------------
// 三个提交点（回车 / 这一行失焦 / 面板关闭）—— 缺一个就会静默丢用户的字
// ---------------------------------------------------------------------------
// 备注是**就地编辑**的，而写盘要走 `history_put`（一次 IPC + 一次写文件）。
// 逐字符绑上去等于每敲一个字发一条命令、写一次盘。⇒ 只在三个点交出去：
//   ① 在这一行的输入框里按回车（`.` 那条路径）；
//   ② 焦点离开这一行；
//   ③ 面板关闭 —— 挂在**模态自己的 `close()`** 上（`dialogs.js` 的四条关闭路径唯一的
//      汇合点：取消 / Esc / 点背影 / 成功换码自己关），**不是**挂在本文件的 `close()` 上。
// ⚠️ ② **不保证**会跑：它的触发条件是"关窗那一刻这个框正好有焦点"。
//    面板一打开焦点就在**码输入框**上（`dialogs.js` 把焦点移给第一个输入格），
//    用户敲完备注去看别处、再按 Esc —— 那时浏览器**不会**补 `blur`
//    ⇒ 少了 ③，"敲完备注直接关窗"那一句就**无声地丢了**，而"重启应用后仍在"
//    正是这一段功能的验收项（静默丢用户敲进去的字是约束 4 明禁的那类失效）。
// ⚠️ **③ 挂在模态上而不是挂在本文件那个 `close()` 上，是判据逼出来的**：
//    挂在后者上时，Esc 与点背影**绕过去**，全靠在"有焦点"时浏览器补一次 `blur` 接住 ——
//    而那正是 ② 自己声明"不保证"的一格。`panels-harness.html` 里那两条
//    "关窗时备注框**没有**焦点"的用例钉的就是这条路。
//
// ---------------------------------------------------------------------------
// ⚠️ 备注草稿的判决：**本文件抄了 `presentation::batch_history::NoteDrafts` 一份**
// ---------------------------------------------------------------------------
// 上游那五条规则（没草稿不写 / 没变不写 / 交完删草稿 / 草稿优先于现值 / 关闭时全部交出去）
// 是 Rust 里一个有单测的值类型，而**它没有任何命令把它送到前端**
// （`history_put` 收的是**已经判决完**的一次写入，见 `api::history::HistoryWrite`）。
// ⇒ 前端要么自己判一遍，要么把"用户敲了一半的字"整个丢掉。这里选前者，
//    并把**每一条规则点名**写在 `drafts` 那几行旁边 —— 抄的是**判决**，不是文案
//    （§3.2 管的是字符串，不是这点状态机）。
// ⚠️ 这是**形态偏离（W-6 那一类）**，如实记在报告里：本文件里那四条规则**没有单测压着**
//    （它在 `.js` 里），而 Rust 那一份有。两处判据的分叉**不会让任何东西变红**。
//
// ⚠️ 本文件**不碰壳**（`registry.js` 契约第 1 条同理）：它只往自己的模态宿主里放东西，
//    往壳上报告"历史写盘失败"走的是 `onReceipt` 回调（由 `app.js` 落到常驻提示行）——
//    面板自己不调任何壳级 setter。

import { byIdIn, clear, middleSplit, setText, template } from "./dom.js";
import { failureText } from "./invoke.js";
import { dismissButton, openModal } from "./dialogs.js";

/**
 * 「历史写盘失败」那条**常驻回执**的 id。
 *
 * ⚠️ 它与 `app.js` 的 `DIR_CHANGE_RECEIPT_ID` 一样是**跨文件的一个键**，不是本地常量：
 *    `app.js` 用它去重与收起。两处写同一个字面量的代价是"改名时漏掉一处"，
 *    而漏掉的后果是那条回执**再也收不起来**（`dismissReceipt` 找不到它）。
 *    今天只有两个 id，所以没有建一个共享模块 —— 两处各自的注释互相点名。
 */
const HISTORY_WRITE_FAILURE_ID = "history-write-failure";

/**
 * 打开换码面板。
 *
 * @param {object} ctx `{ call, CMD, poller }`（与屏的 `ctx` 同形，**少了 `shell`**：
 *   面板不是屏，不该读分区）。
 * @param {object} hooks
 * @param {string} hooks.initialCode 打开时输入框的初值（= 当前生效批次的码；
 *   没有生效批次时是空串 —— 那种情况下工具栏那颗按钮本来就是灰的）。
 * @param {(desc: object) => void} hooks.onReceipt 要落到**常驻提示行**上的回执
 *   （今天只有一条：历史写盘失败）。⚠️ 形状与 `shell.setNotices` 收的一条一致。
 *   面板**就地也显示一份**（理由同 macOS：用户正对着这个面板，而主区那行在它背后）。
 * @param {() => void} hooks.onClose 面板**已经关掉**之后回调一次（`app.js` 用它把
 *   "现在开着的是谁"那一格清掉）。⚠️ 它挂在模态的关闭路径上，所以**取消 / Esc /
 *   点背影 / 成功换码自己关**四条路都会走到 —— 而四条各写一遍必然会漏掉一条。
 * @returns {{render: Function, close: Function}}
 */
export function openSwitchcode(ctx, hooks) {
  const modal = openModal("tpl-panel-switchcode");
  const root = modal.root;

  // 🔴 第三个提交点挂在**模态自己的 `close()`** 上，而不是挂在下面那个本文件的 `close()` 里。
  //
  // 四条关闭路径（取消 / Esc / 点背影 / 成功换码自己关）**都汇到** `dialogs.js` 的
  // 模态栈那一个 `handle.close()`，而本文件的 `close()` 只是其中两条的入口 ——
  // 挂在后者上，Esc 与点背影就**绕过去了**（`panels-harness.html` 里那两条"关窗时
  // 备注框没有焦点"的用例，钉的就是这件事：那时浏览器**不会**补 `blur`，
  // 用户敲了一半的备注会**无声丢掉**）。
  //
  // ⚠️ **包在真正的 `close()` 之前**（`dialogs.js:109` 会先摘 DOM）：顺序是承重的两条 ——
  //   · 摘 DOM 那一刻备注框若有焦点，浏览器补一次失焦提交（模块头说它"不保证会跑"）；
  //     先交草稿 ⇒ 那次补发看到的草稿**已经被删了**（`commitNote` 的"交完必删"），
  //     是个空转，不会写两遍；
  //   · 于是"写了哪几条、按什么顺序"在四条路径上是**同一个答案**
  //     （`commitAllNotes` 那个按码排序），而不是"看浏览器补了什么"。
  const modalClose = modal.close;
  modal.close = () => {
    commitAllNotes();
    modalClose();
  };

  modal.onClose = () => {
    if (hooks.onClose) hooks.onClose();
  };

  const codeEl = byIdIn(root, "sc-code");
  const loadEl = byIdIn(root, "sc-load");
  const cancelEl = byIdIn(root, "sc-cancel");
  const failureEl = byIdIn(root, "sc-failure");
  const failureTextEl = byIdIn(root, "sc-failure-text");
  const historyEl = byIdIn(root, "sc-history");
  const rowsEl = byIdIn(root, "sc-history-rows");
  const historyFailureEl = byIdIn(root, "sc-history-failure");
  const historyFailureTextEl = byIdIn(root, "sc-history-failure-text");
  const historyFailureDismissEl = byIdIn(root, "sc-history-failure-dismiss");

  // 「收起」那颗 ×（历史写盘失败那一行）：走 `dialogs.js:dismissButton` ——
  // 克隆常驻提示行那份模板（`tpl-notice-dismiss` 带着它的无障碍名字）**并把 × 补上**，
  // 本文件一个字都不造。与 `verify.js` 收起失败条、`shell.js:noticeRow` 是同一手法。
  historyFailureDismissEl.append(
    dismissButton(() => {
      historyFailureEl.hidden = true;
    })
  );

  // ---------------------------------------------------------------------------
  // 状态
  // ---------------------------------------------------------------------------
  /** 屏上那些行（`BatchHistoryRow` **原样**，一格都不改）。 */
  let rows = [];
  /** 一次"换码"在飞（防连点；面板上两颗入口共用这一个闸）。 */
  let inFlight = false;
  /** 面板**这一次**发过 `load` 没有（`null` = 还没有）。见 `render` 那段。 */
  let pendingCode = null;
  /** 打开那一刻的批次码。`load` 成功**换了一批**时才关面板（见 `render`）。 */
  let codeAtOpen = typeof hooks.initialCode === "string" ? hooks.initialCode : "";
  /** 上一次画出来的历史那一块的签名（内容没变就不重建 DOM —— 见 `paintHistory`）。 */
  let paintedHistoryKey;
  /** 历史写盘失败的原文（`null` = 那一行不显示）。 */
  let historyFailure = null;
  /** 上一次画出来的失败原文（`null` = 那一格现在是空的）。 */
  let shownFailure = null;

  /**
   * 还没交出去的备注草稿（**码 → 用户敲进去的原文**）。
   *
   * 对位 `presentation::batch_history::NoteDrafts` 的 `by_code`（`BTreeMap<String, String>`）。
   * ⚠️ **`has` 与"值是空串"是两件事**：`Some("")` 是"用户把它清空了"（一个真的草稿），
   *    `None` 是"用户没碰过"。混为一谈的后果是**清空备注之后旧的那句会自己弹回来**。
   */
  let drafts = new Map();

  /** 这一行现在该显示什么：**草稿优先于现值**（`NoteDrafts::text`）。 */
  function draftText(code, current) {
    return drafts.has(code) ? drafts.get(code) : current;
  }

  /** 某个码在**当前载荷里**的备注；`undefined` = 这个码已经不在列表里了。 */
  function currentNote(code) {
    const row = rows.find((r) => r.code === code);
    return row ? row.note : undefined;
  }

  // ---------------------------------------------------------------------------
  // 历史列表
  // ---------------------------------------------------------------------------

  /**
   * 把历史那一块画出来。**内容没变就一个节点都不动。**
   *
   * 🔴 这不是性能优化：行里的 `code` 是**可选中复制**的（约束 3：用户要能把码发给业务方），
   *    而这个面板**每写一次备注就会收到一整份新载荷**（`history_put` 回的是写完之后
   *    那一份）。每回重建 DOM 的后果是**用户正在拖的选区被清掉**——
   *    而那正好发生在他刚敲完一句备注、想去复制那个码的时候。
   *    同 `shell.setNotices` 的内容签名、`verify.js:paint` 的两把键。
   *
   * ⚠️ 键里**不含草稿**（草稿是输入框里的东西，不是这一块的内容）——
   *    含进去的话每敲一个字就重建一次列表，输入框会当场失去焦点。
   */
  function paintHistory() {
    const key = JSON.stringify(rows);
    if (key === paintedHistoryKey) return;
    paintedHistoryKey = key;

    // ⚠️ **空列表时整段不渲染**（`BatchHistoryRow::rows` 的文档逐字：
    //    "返回空 Vec 时调用方整段不渲染 —— 不要给空盒子"）。
    historyEl.hidden = rows.length === 0;
    clear(rowsEl);
    for (const row of rows) rowsEl.append(historyRow(row));
  }

  /**
   * 一行：**点标题即切换** + 就地编辑备注。
   *
   * ⚠️ 上屏的字**一个都不在这里拼**：`title` / `code` / `note` / `time_text` 全是
   *    `BatchHistoryRow` 算好的（模块头第 ① 条）。
   *
   * ⚠️ **与 macOS 的一处形态偏离（如实记账）**：macOS 那一行是**整块可点**
   *    （`.contentShape(Rectangle())`），而我们这边"点一下"的落点是**标题那一格**
   *    —— 码与时间那两格留在按钮外面，因为它们要**可选中复制**，
   *    而文字套进 `<button>` 之后在部分浏览器里选不动（那会毁掉"把码复制走"这个用法）。
   *    ⇒ 行上面积最大的那块（标题）仍然是切换，但右边那两格不是。
   */
  function historyRow(row) {
    const frag = template("tpl-switchcode-row");
    const switchEl = frag.querySelector(".sc__switch");
    const titleHeadEl = frag.querySelector('[data-part="title-head"]');
    const titleTailEl = frag.querySelector('[data-part="title-tail"]');
    const codeEl2 = frag.querySelector(".sc__code");
    const codeHeadEl = frag.querySelector('[data-part="code-head"]');
    const codeTailEl = frag.querySelector('[data-part="code-tail"]');
    const timeEl = frag.querySelector('[data-part="time"]');
    const noteEl = frag.querySelector(".sc__note");

    // ⚠️ **中间截断**（`truncationMode(.middle)`，macOS `SwitchDeliverySheet` 那两格
    //    都是它）：切成两段、由 CSS 在前一段的行尾画省略号 —— 规则与理由见
    //    `dom.js:middleSplit`（**一个字符都不改**，省略号是浏览器画的）。
    //    两格之间**没有空白文本节点**，所以 `textContent` 与跨格选择拿到的都是原文。
    const [titleHead, titleTail] = middleSplit(row.title);
    setText(titleHeadEl, titleHead);
    setText(titleTailEl, titleTail);
    const [codeHead, codeTail] = middleSplit(row.code);
    setText(codeHeadEl, codeHead);
    setText(codeTailEl, codeTail);
    setText(timeEl, row.time_text);
    // 🔴 **两格都要给悬停的全文**（`dom.js:middleSplit` 的判据 ③："全文两处都在"）：
    //    中间截断意味着**中间那一段屏幕上看不到**，没有 `title` 就没有任何出口
    //    （而"这条历史是哪一批"正是靠标题认的）。
    //    ⚠️ **标题那一格曾经漏了这一句**（第 2 轮修复轮补的）：同批四处（这两格 +
    //    设置里的目录显示值 + 回执里的路径行）与参照实现 `verify.js:pathRow` 都设了，
    //    只有它没有 —— 而报告当时**声称四处都做了**。那种"声明说了件不成立的事"
    //    正是本仓库反复栽的坑，所以本条**有断言压着**（②e3）。
    //    两格的 `title` 都是**原文**（`row.title` / `row.code`）—— 逐字不截断（约束 3），
    //    截断只发生在 CSS 那一层。
    switchEl.setAttribute("title", row.title);
    codeEl2.setAttribute("title", row.code);
    noteEl.value = draftText(row.code, row.note);

    switchEl.addEventListener("click", () => {
      // ⚠️ **`base_url` 必须跟着一起发**（这一条最容易漏）：历史里记着这一条是从哪台
      //    交付服务器加载的，不带它，从**自定义**服务器加载过的批次再点一次就会去问
      //    **默认**服务器 —— 拿到的是"码不存在"之类的失败，而用户刚刚明明看见它列在历史里。
      //    ⚠️ 空串 ⇒ **请求里不出现 `base_url` 这个键**（`BatchHistoryRow::base_url_or_nil`
      //    的判据）：显式传一个"和默认一样"的值会在内核默认值变化时静默分叉（E-5）。
      const args = { code: row.code };
      if (row.base_url) args.baseUrl = row.base_url;
      void send(args);
    });

    noteEl.addEventListener("input", () => {
      // 敲一个字只记一笔草稿，**不发命令**（见模块头"三个提交点"）。
      drafts.set(row.code, noteEl.value);
    });
    noteEl.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      commitNote(row.code);
    });
    noteEl.addEventListener("blur", () => commitNote(row.code));

    return frag;
  }

  // ---------------------------------------------------------------------------
  // 备注的提交（三个点，见模块头）
  // ---------------------------------------------------------------------------

  /**
   * 把**一个**码的草稿交出去。对位 `NoteDrafts::committing`。
   *
   * 三道闸（每一条都在挡一种具体的伤害）：
   *   ① **没有草稿** ⇒ 一个字节都不写（用户只是点了一下那一行，不是来改备注的）；
   *   ② **这一行已经不在屏上了**（`current === undefined`）⇒ 丢掉草稿，不写
   *      （凭空写一条没有时间戳的记录只会变成"排在最末、点了没反应"的假条目）；
   *   ③ **与现值相同** ⇒ 不写（免得每次失焦都写一次盘）。
   * ⚠️ **有草稿就一定把它清掉**（连 ①②③ 三支也是）：交完之后编辑框读回**模型那份** ——
   *    它在 `BatchHistory::setting_note` 里被归一化过（首尾空白去掉、换行折成空格），
   *    编辑框要如实显示**存下去的那一份**，而不是用户敲的原文。
   */
  function commitNote(code) {
    if (!drafts.has(code)) return; // ①
    const draft = drafts.get(code);
    drafts.delete(code); // 交完必删（见上）
    const current = currentNote(code);
    if (current === undefined) return; // ②
    if (current === draft) return; // ③
    void writeNote(code, draft);
  }

  /**
   * 把**还挂着的所有**草稿交出去（面板关闭时那一次兜底）。对位 `NoteDrafts::committing_all`。
   *
   * ⚠️ 判据是**这里还挂着哪些键**，不是"屏上有哪些行"：这时面板正在销毁，
   *    走一遍屏上的行等于把"用户敲过字"这件事重新押在视图树还在不在上。
   * ⚠️ 顺序**按码排序**（Rust 那一份是 `BTreeMap`，也是有序的）：写盘本身幂等，
   *    顺序只影响可观察性 —— 而"到底写了几条、写了哪几条"必须是一个确定的答案。
   */
  function commitAllNotes() {
    for (const code of [...drafts.keys()].sort()) commitNote(code);
  }

  /** 真的写一条备注（`history_put` 的第二个变体：**不动时间戳**，见 `HistoryWrite`）。 */
  async function writeNote(code, note) {
    try {
      const data = await ctx.call(ctx.CMD.historyPut, { entry: { kind: "note", code, note } });
      // ⚠️ 写成功 ⇒ **用回执那一份重新画**（`history_put` 回的就是写完之后那一份历史）：
      //    归一化过的备注（首尾空白、换行）要在界面上如实出现 —— 那正是
      //    "交完之后编辑框读回模型那份"那条判据在 web 上的落点。
      applyPayload(data);
      hideHistoryFailure();
    } catch (error) {
      // 失败**不许静默**（约束 4）：就地一行 + 主区一条常驻回执（两个落点、同一句话）。
      showHistoryFailure(failureText(error));
    }
  }

  /** 历史写盘失败：就地显示 + 交给 `app.js` 挂到常驻提示行上。 */
  function showHistoryFailure(message) {
    historyFailure = message;
    setText(historyFailureTextEl, message);
    historyFailureEl.hidden = false;
    if (hooks.onReceipt) {
      hooks.onReceipt({
        // ⚠️ **`id` 是必需的**（不是装饰）：`app.js:pushReceipt` 用它去重、
        //    `dismissReceipt` 用它收起 —— 少了它这一条会被**静默丢掉**
        //    （`pushReceipt` 的第一个判断就是"有没有 id"）。
        //    这个 id 同时是"同一个失败只留一条"的键（连点两次不会排出两条）。
        id: HISTORY_WRITE_FAILURE_ID,
        // ⚠️ `kind` 只决定图标与配色（绘制层），不是文案：正文是**原文逐字**。
        kind: "warning",
        icon: "exclamationmark.triangle.fill",
        text: message,
        hint: null,
        dismiss: true,
      });
    }
  }

  function hideHistoryFailure() {
    if (historyFailure === null) return;
    historyFailure = null;
    setText(historyFailureTextEl, "");
    historyFailureEl.hidden = true;
  }

  /** 把一份 `history_get` / `history_put` 的载荷落到行上（两处共用一条渲染路径）。 */
  function applyPayload(data) {
    rows = data && Array.isArray(data.rows) ? data.rows : [];
    paintHistory();
  }

  // ---------------------------------------------------------------------------
  // 换码
  // ---------------------------------------------------------------------------

  /** 这次要发的码：用户敲的。⚠️ **不 trim**（同 `empty.js`：前端多加工一次，
   *  界面上看到的与发出去的就成了两个串）。 */
  function currentCode() {
    return codeEl.value;
  }

  /**
   * 发一条 `load`。
   *
   * ⚠️ **结果不看这条命令的返回值**：按规格 §3.4，`load` 是"异步起 `load_delivery`，
   *    结果落 `session.load`" —— 权威在 `state()` 那一格里。所以这里只把命令发出去，
   *    失败**由壳每一拍喂进来的 `state().load`** 显示（见 `render`）。
   *    这一条与 `empty.js:send` 是**同一条分工**（那边也只有"发都发不出去"那一类
   *    落在本侧）。
   *
   * ⚠️ 过长的码**不在这里挡**：那两条长度判据（`DeliveryCodeEntry::too_long` /
   *     `base_url_too_long`）以及它们各自那句提示**都是 Rust 的**，而那两个值
   *    **不在任何载荷里**。照抄一份字面量到这里 = JS 造界面文案 + 第二份真相
   *    （`empty.js:syncSubmitEnabled` 那段记着同一条推理）。⇒ 让它发出去，
   *    由命令层的闸拒绝，拒绝的那句话经 `session.load` 回来、**原文**显示在下面那一格。
   *
   * @param {object} args `{code}` 或 `{code, baseUrl}`（后者来自历史里那一行）
   */
  async function send(args) {
    if (inFlight) return; // 一次只发一条（防连点）
    if (!args.code) return;
    inFlight = true;
    syncEnabled();
    hideFailure();
    pendingCode = args.code;
    try {
      await ctx.call(ctx.CMD.load, args);
      // 立刻补一拍：不然"点了加载"到"界面上有反应"之间要等一整秒的 `state` 轮询，
      // 而那一秒里用户看到的是**按了没反应**（约束 4 最恨的形态之一）。
      ctx.poller.pollNow();
    } catch (error) {
      // 发都发不出去（宿主里没有这条命令 / IPC 断了）：**就地**显示原文。
      // ⚠️ 这一支与"内核拒了"不是同一件事：后者落在 `state().load` 上（见 `render`）。
      showFailure(failureText(error));
      pendingCode = null;
    } finally {
      inFlight = false;
      syncEnabled();
    }
  }

  /** 面板上那两颗按钮此刻该不该可按（一次只发一条 + 码非空）。 */
  function syncEnabled() {
    const want = inFlight || currentCode() === "";
    // ⚠️ 值没变就不写属性（同 `verify.js:syncRefreshEnabled` 的纪律）。
    if (loadEl.disabled !== want) loadEl.disabled = want;
  }

  // ---------------------------------------------------------------------------
  // 失败原文（**内核原文逐字**，约束 3）
  // ---------------------------------------------------------------------------

  /**
   * 🔴 **同一句话不重写第二遍** —— 与 `empty.js:showFailure` / `verify.js:showFailure`
   *    同一条理由：这一格是**可选中复制**的，而它被**每一拍的 `state()`** 喂；
   *    无脑重写 `textContent` 会换掉那个文本节点 ⇒ **用户正拖着选区准备复制的那段原文，
   *    被清掉**。而"把这句原话发给业务方"正是这一格存在的理由。
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

  // ---------------------------------------------------------------------------
  // 事件
  // ---------------------------------------------------------------------------

  loadEl.addEventListener("click", () => {
    void send({ code: currentCode() });
  });
  codeEl.addEventListener("input", syncEnabled);
  codeEl.addEventListener("keydown", (event) => {
    // 回车 = 按那颗「加载」（macOS 的 `.onSubmit(load)` + `.keyboardShortcut(.defaultAction)`）。
    if (event.key !== "Enter") return;
    event.preventDefault();
    void send({ code: currentCode() });
  });
  cancelEl.addEventListener("click", () => close());

  /**
   * 收起面板。
   *
   * ⚠️ **这里不交草稿**：第三个提交点挂在 `modal.close` 上（见 `openSwitchcode` 开头那段）
   *    —— 那是四条关闭路径唯一的汇合点，本函数只是其中两条的入口。
   *    写在这里会让 Esc / 点背影绕过它（那正是它原先的形态）。
   */
  function close() {
    modal.close();
  }

  // 首帧：输入框的初值 = 当前批次码（macOS `onAppear` 那一条）。
  // ⚠️ 只在空的时候写（同 macOS 的判据）：写第二遍会把用户已经敲进去的码抹掉。
  if (codeEl.value === "" && codeAtOpen) codeEl.value = codeAtOpen;
  syncEnabled();

  // 读历史。**没有节拍**：它只在打开时读一次，之后每次写入用回执那一份更新
  // （`history_put` 回的就是写完之后那一份 —— 少一次往返，也少一个"界面与盘上不一致"的窗口）。
  ctx.call(ctx.CMD.historyGet).then(applyPayload, (error) => {
    // 读失败 ⇒ 这一块**不画**（`rows` 保持空数组 ⇒ `historyEl.hidden`）。
    // ⚠️ 不把"读失败"塞进上面那格失败原文：那一格说的是**换码**的结果，
    //    两件事混在一格会让用户以为"我还没点就失败了"。
    showHistoryFailure(failureText(error));
  });

  return {
    /**
     * 壳每一拍把 `state().load` 交进来（与屏的 `render(payload)` 同一个位置）。
     *
     * 这一扇窗只从它取**一件事**：**这次换码成没成**。
     *   · `failed` ⇒ 把 `load.message` **原文**摆在面板里（macOS：失败要留在原地显示内核原文，
     *     面板不关）；
     *   · `loaded` ⇒ 说明新批次已经生效。⚠️ 判据是"**码真的换了**"
     *     （`load.summary.code !== codeAtOpen`），不是"看见 loaded 就关"：
     *     发出去到 `state` 反映之间有一整拍的窗口，那时 `load` 还是**上一批**的
     *     `loaded` —— 看见它就关，面板会在请求发出去的**同一瞬间**消失。
     *     用户会以为"我按了取消"（macOS 的 `DeliverySwitch` 头注记着的正是这个形态）。
     *
     * ⚠️ **`pendingCode` 那道闸**：面板刚打开时 `load` 可能已经是一批 `failed`
     *    （上一次加载失败留在会话里）—— 那时显示它，就是"一进来就显示上一轮的结论"，
     *    而用户还没点过任何东西。⇒ 只有**本面板发过 load 之后**才把失败摆出来。
     */
    render(load) {
      if (pendingCode === null) return;
      const kind = load && typeof load.kind === "string" ? load.kind : "";
      if (kind === "failed") {
        showFailure(typeof load.message === "string" ? load.message : "");
        return;
      }
      if (kind === "loaded") {
        const code =
          load.summary && typeof load.summary.code === "string" ? load.summary.code : null;
        if (code !== null && code !== codeAtOpen) {
          // ⚠️ 新批次的码记下来再关：`close()` 的草稿提交里若有人回头看这一格，
          //    它得是**新的**那一批（否则那些码是按旧批次在历史里找的）。
          codeAtOpen = code;
          close();
        }
      }
    },
    close,
  };
}
