// settings.js —— **设置窗口**（规格 §2.1 第 9 项）：下载目录段 + 七项参数 + 许可入口。
//
// 对位 `macos/Sources/BenagenDownloader/Settings/SettingsView.swift`。
// 那是客户**唯一**能改内核参数的地方（工具栏那颗「设置」打开的它）。
//
// ---------------------------------------------------------------------------
// 🔴 本文件里没有一句面向用户的字符串（规格 §3.2）
// ---------------------------------------------------------------------------
//   · **七项参数的标签与区间** —— `settings_get` 的 `parameters[]`（`Parameter{label,min,max}`）；
//   · **两项有语义的说明** —— `notes.limit_mbps` / `notes.apply`；
//   · **`-k` 的取值面与那句"不在集合里"的说明** —— `min_split_size_options` /
//     `min_split_size_note`；
//   · **保存条那句横幅与两条 help** —— `banner.{unsaved,clean}` /
//     `save_help.{unsaved,clean,engine_unavailable}`；
//   · **下载目录那一整段** —— `dir` / `display` / `section_note`，以及
//     `preferences_check` 的 `check`（"这个文件夹不能用"那一句）与 `confirmation`
//     （三条后果），`preferences_set` 的四格回执。
//   ⇒ 本文件只做**挑选**（两种状态里挑一句）与**摆位置**，不做造句。
//     这正是 `api/settings.rs` 头部那句"两格各发一格，前端只做挑选、不做造句"。
//
// ---------------------------------------------------------------------------
// ⚠️ 两处形态偏离（如实记账）
// ---------------------------------------------------------------------------
// **① 原生目录选择器 —— 2026-09-20 已兑现（真机反馈）。**
//    原先记的是"本代**没有**引入 `tauri-plugin-dialog`，所以这一格只是一个可粘贴
//    路径的输入框；代价是用户要自己找到路径（macOS 上是点两下）。这是缺口，不是取舍。"
//    那条缺口已经补上：「选择…」那颗走 `pick_directory`（原生对话框在
//    `shell-win/src/pickdir.rs`：Windows 是 `SHBrowseForFolderW`，**零新依赖** ——
//    那个 API 就在已经开着的 `Win32_UI_Shell` 里，另加的一条 feature 只为释放 PIDL）。
//    选完走的仍然是**同一条**既有的流程（检查 → 确认 → 写偏好 → 重启内核），
//    与 macOS 的 `choose()` → `ask()` 对位。
//    ⚠️ **留着一处平台能力的差别**（不是漏做）：`NSOpenPanel` 有 `message` 与
//       `prompt` 两格文案，而 `SHBrowseForFolderW` 只有 `lpszTitle` 一格 ——
//       取 macOS 那句 `message`（`api::preferences::picker_title`），按钮名由系统给。
//    手工粘路径那条路**一个字都没改**（它仍然在，「选择…」只是多出来的一条）。
//
// **② `notes.limit_mbps` 贴在参数表的下面，不是那一行下面。** macOS 把它贴在那一个
//    控件正下方。载荷里那七项只有 `{label,min,max}`（**没有键名**），
//    "哪一行是限速"在前端**认不出来**；按 `max == 100000` 去猜就是拿一个数字当标识符
//    （Rust 改一次上界，这句话就会贴到别的行下面而**没有任何东西会变红**）。
//    ⇒ 三句说明（限速那句 / `-k` 那句 / 生效时机那句）集中在参数表下面。
//    只有 `-k` 那一句仍然贴在它的控件下面 —— 因为"哪一行是枚举"**有一个真的判据**
//    （`Parameter` 的文档：`min`/`max` 是 `None` 就是它）。
//
// ---------------------------------------------------------------------------
// ⚠️ 本窗口**不轮询**（一个节拍都不起）
// ---------------------------------------------------------------------------
// 它要的两样（参数、偏好）都是**打开时取一次**就够的静态值，改了才重取。
// 唯一的"每一拍"是**引擎闸门**（`state().allows_requests`）—— 那个由壳在
// `render(state)` 里灌进来（`app.js` 本来就每秒问一次 `state()`），
// 本文件**不自己发请求**，也就不会多出一条没有界面的负载（屏契约第 3 条的同一条口径）。

import { byIdIn, clear, h, icon, middleSplit, setText } from "./dom.js";
import { failureText } from "./invoke.js";
import { dismissButton, openModal, openAbout, openLicense } from "./dialogs.js";

/**
 * 七项参数在**线上**的键名，顺序即面板上从上到下的顺序。
 *
 * ⚠️ 它们是**线上契约**（`shell-core/src/protocol.rs` 的 `Settings` 字段名），
 *    由 `api::settings` 的 `set_settings_always_sends_all_seven_keys` 逐字钉着。
 *    写成 camelCase（`minSplitSize`）同样是 7 个键，只是内核把它们当成缺失键、
 *    回一条 `invalid_params` —— 而**不会有任何东西变红**。
 *    **权威在那边**：改这里任何一个名字之前，先改 `protocol.rs`。
 */
const FIELDS = Object.freeze([
  "parallel",
  "connections",
  "splits",
  "min_split_size",
  "limit_mbps",
  "max_tries",
  "retry_wait",
]);

/** 界面语言那一格的线上键名（`protocol::Settings.language`）。它是**字符串**，
 *  不是那六项数值之一，所以不在 `FIELDS` 里（`save()` 会把它编成数字就糟了）。 */
const LANGUAGE_FIELD = "language";

/** `-k` 那一个字段名。它与其余六项的区别（枚举面来自内核的 `hello`）**由 Rust 说**：
 *  `parameters[i]` 的 `min`/`max` 同时为 `null` 就是它（见 `Parameter` 的文档）。 */
const ENUM_FIELD = "min_split_size";

/**
 * 打开设置窗口。
 *
 * @param {object} ctx `{ call, CMD, poller, shell }`
 * @param {object} hooks
 * @param {(desc: object) => void} hooks.onReceipt 要落到**常驻提示行**上的回执。
 *   改下载目录那条回执**必须**有两个落点（macOS 的最终审查重要 2）：设置窗口是
 *   随手就会被关掉的东西，而那条回执里有一格是"**没改成**"（内核重启失败）——
 *   窗口一关它不能跟着消失。
 * @param {() => void} hooks.onClose 窗口**已经关掉**之后回调一次（`app.js` 用它把
 *   "现在开着的是谁"那一格清掉）。⚠️ 挂在模态的关闭路径上（理由同 `switchcode.js`：
 *   取消 / Esc / 点背影三条路都会走到，各写一遍必然漏掉一条）。
 * @returns {{render: Function, close: Function}}
 */
export function openSettings(ctx, hooks) {
  const modal = openModal("tpl-window-settings");
  const root = modal.root;
  modal.onClose = () => {
    if (hooks.onClose) hooks.onClose();
  };

  const dirDisplayEl = byIdIn(root, "set-dir-display");
  const dirDisplayHeadEl = byIdIn(root, "set-dir-display").querySelector('[data-part="head"]');
  const dirDisplayTailEl = byIdIn(root, "set-dir-display").querySelector('[data-part="tail"]');
  const dirInputEl = byIdIn(root, "set-dir-input");
  const dirChooseEl = byIdIn(root, "set-dir-choose");
  const dirApplyEl = byIdIn(root, "set-dir-apply");
  const dirResetEl = byIdIn(root, "set-dir-reset");
  const dirNoteEl = byIdIn(root, "set-dir-note");
  const dirReceiptEl = byIdIn(root, "set-dir-receipt");
  const dirReceiptIconEl = byIdIn(root, "set-dir-receipt-icon");
  const dirReceiptTextEl = byIdIn(root, "set-dir-receipt-text");
  const dirReceiptDismissEl = byIdIn(root, "set-dir-receipt-dismiss");
  const verboseEl = byIdIn(root, "set-verbose");
  const verboseLabelEl = byIdIn(root, "set-verbose-label");
  const verboseNoteEl = byIdIn(root, "set-verbose-note");
  // 「启动时检查更新」（规格 §4）。标签是**结构文案**（住在 `index.html`，本文件不碰）；
  // 这里只管那个勾 ——它的值来自**内核**（`update_status.enabled`）。
  const updateEl = byIdIn(root, "set-update");
  // 「立即检测」（规格 §2）那颗按钮 + **它自己的**回执行（住在同一节里，同导出那一条）。
  // ⚠️ 它的回执那几格全部来自载荷（`api::update::checked`），本文件只决定画成什么样。
  const updateNowEl = byIdIn(root, "set-update-now");
  const updateReceiptEl = byIdIn(root, "set-update-receipt");
  const updateReceiptIconEl = byIdIn(root, "set-update-receipt-icon");
  const updateReceiptTextEl = byIdIn(root, "set-update-receipt-text");
  const updateReceiptDismissEl = byIdIn(root, "set-update-receipt-dismiss");
  const exportEl = byIdIn(root, "set-export");
  const exportReceiptEl = byIdIn(root, "set-export-receipt");
  const exportReceiptIconEl = byIdIn(root, "set-export-receipt-icon");
  const exportReceiptTextEl = byIdIn(root, "set-export-receipt-text");
  const exportReceiptDismissEl = byIdIn(root, "set-export-receipt-dismiss");
  const paramsRowsEl = byIdIn(root, "set-params-rows");
  const languageSectionEl = byIdIn(root, "set-language-section");
  const languageTitleEl = byIdIn(root, "set-language-title");
  const languageRowEl = byIdIn(root, "set-language-row");
  const paramsNotesEl = byIdIn(root, "set-params-notes");
  const unavailableEl = byIdIn(root, "set-unavailable");
  const unavailableTextEl = byIdIn(root, "set-unavailable-text");
  const failureEl = byIdIn(root, "set-failure");
  const failureTextEl = byIdIn(root, "set-failure-text");
  const failureDismissEl = byIdIn(root, "set-failure-dismiss");
  const bannerEl = byIdIn(root, "set-banner");
  const saveEl = byIdIn(root, "set-save");
  const aboutEl = byIdIn(root, "set-about");
  const licenseEl = byIdIn(root, "set-license");
  const closeEl = byIdIn(root, "set-close");

  // 「收起」那颗 ×：走 `dialogs.js:dismissButton`（克隆模板 + **补上那个 ×**）。
  // ⚠️ 只克隆、不补图形的话，那是一颗**没有内容的小方块** —— 看得见（悬停才有一点底色）、
  //    但在客户眼里与"这里坏了"分不开。这一条是审查抓出来的（五个地方同一个错）。
  for (const host of [dirReceiptDismissEl, exportReceiptDismissEl, updateReceiptDismissEl, failureDismissEl]) {
    host.append(dismissButton(() => host.__dismiss()));
  }

  // ---------------------------------------------------------------------------
  // 状态
  // ---------------------------------------------------------------------------
  /** 七项参数**内核手里那一份**（`settings_get` / `settings_set` 的 `data`）。 */
  let payload = null;
  /** 表里**正在编辑**的那一份（从 `payload.settings` 复制出来的一层）。 */
  let edited = null;
  /** 偏好那一格（`preferences_get` 的 `data`）。 */
  let preferences = null;
  /** 一次改动/一次保存在飞。 */
  let busy = false;
  /**
   * 一次「选择…」在飞（**只挡开第二个系统对话框**，不挡别的）。
   *
   * ⚠️ 它与 `busy` 是两件事：`busy` 说的是"正在改下载目录"（那期间不许再发起一次），
   *    而这一格说的是"系统对话框还开着"。两者**时间上重叠但不等** ——
   *    对话框关掉之后才轮到 `askDirectory`（它自己去抢 `busy`）。
   *    ⇒ 合成一个的话，`chooseDirectory` 拿到结果再调 `askDirectory` 时会被自己挡住。
   */
  let picking = false;
  /**
   * 一次「导出诊断日志…」在飞（**只挡开第二个系统对话框**，理由同 [`picking`]）。
   *
   * ⚠️ 它与 `picking` 是**两件事**（两个按钮、两条命令），各自挡自己那一颗：
   *    真机上两个对话框都是**模态**的，同时点不出两个来；分成两格只是为了让
   *    "哪一颗正在等系统"这件事在代码里读得出来。
   */
  let exporting = false;
  /**
   * 一次「立即检测」在飞（规格 §2）。
   *
   * ⚠️ 它挡的**不是系统对话框**（这一条不问系统要东西），而是**连点**：
   *    内核那一条要发一次网络请求（最坏几秒），期间再点一次会排到同一条内核通道后面
   *    （`client.rs` 的单飞闸门）—— 用户看到的是"点了没反应"，而真相是排着队。
   *    挡住它、并在按钮上按在飞状态说明（`aria-busy`），比让客户连点五下诚实。
   */
  let checkingUpdate = false;
  /** 引擎闸门（`state().allows_requests`，壳每一拍灌进来）。 */
  let gateOpen = false;
  /**
   * 七项里**哪一项是枚举**（`-k`）。`paintParams` 现算（判据是 `Parameter` 的
   * `min`/`max` 同时为 `None`，见那个类型的文档），`save` 用它决定要不要转成数字。
   *
   * ⚠️ 它**不是**"第几项"那种写死的下标：Rust 调一次 `parameters` 的顺序，
   *    写死的实现会把值发到**另一个字段**上，而两边都是"一个数"⇒ 不会有东西变红。
   */
  let enumField = null;

  function close() {
    modal.close();
  }
  dirReceiptDismissEl.__dismiss = () => {
    dirReceiptEl.hidden = true;
    // ⚠️ 主区那条常驻回执也一起收掉：两个落点读的是**同一件事**，
    //    在窗口里按了 × 而主区那条还挂着，用户会以为"没收掉"。
    if (hooks.onReceiptDismiss) hooks.onReceiptDismiss();
  };
  failureDismissEl.__dismiss = () => {
    failureEl.hidden = true;
  };
  exportReceiptDismissEl.__dismiss = () => {
    exportReceiptEl.hidden = true;
  };
  updateReceiptDismissEl.__dismiss = () => {
    updateReceiptEl.hidden = true;
  };

  // ---------------------------------------------------------------------------
  // 参数表
  // ---------------------------------------------------------------------------

  /**
   * 把七行画出来。
   *
   * ⚠️ 行是**运行时造的**（七项的标签来自载荷，不是结构文案），所以这里用
   *    `h()` 建节点；但**格子里写进去的每一个字都来自载荷** —— 本函数只摆位置。
   */
  function paintParams() {
    clear(paramsRowsEl);
    clear(paramsNotesEl);
    enumField = null;
    const parameters = payload && Array.isArray(payload.parameters) ? payload.parameters : [];
    if (!edited || parameters.length === 0) return;

    // ⚠️ **键与行的配对**：枚举那一项由 `Parameter` 的 `min`/`max` 同时为 `None` 认出
    //    （Rust 的判据，见 `Parameter` 的文档），其余六项按 `FIELDS` 的顺序**跳过**
    //    枚举那一格往下取。
    //    ⚠️ **不按"第 4 项就是 -k"写死**：那样 Rust 调一次顺序，`limit_mbps` 那一格的
    //    编辑就会落到另一个字段上 —— 而**不会有任何东西变红**（两边都是"一个数字"）。
    let cursor = 0;
    for (const parameter of parameters) {
      const isEnum = parameter.min === null || parameter.min === undefined;
      let field;
      if (isEnum) {
        field = ENUM_FIELD;
        enumField = ENUM_FIELD;
      } else {
        while (cursor < FIELDS.length && FIELDS[cursor] === ENUM_FIELD) cursor += 1;
        field = FIELDS[cursor];
        cursor += 1;
      }
      const row = h("div", { class: "set__row" });
      row.append(h("span", { class: "set__label", text: parameter.label }));

      let control;
      if (isEnum) {
        // `-k`：**枚举控件**，取值集合由内核的 `hello` 给（壳不生成它）。
        control = h("select", { class: "set__control" });
        const options =
          payload && Array.isArray(payload.min_split_size_options)
            ? payload.min_split_size_options
            : [];
        for (const option of options) {
          const optEl = h("option", { value: option, text: option });
          control.append(optEl);
        }
        // ⚠️ **当前值不在候选集合里时**：`min_split_size_options` 已经把当前值
        //    追加在末尾（`SettingsForm::min_split_size_options` 的判据），
        //    所以这一格**一定**选得中 —— 它不会回落到第一项。
        control.value = edited[field];
        control.addEventListener("change", () => {
          edited[field] = control.value;
          paintFooter();
        });
        row.append(control);
        // 那一句"当前值不在内核给出的集合里"（`Option`：`null` = 没什么可说的）。
        const note = payload.min_split_size_note;
        if (typeof note === "string" && note.length > 0) {
          row.append(h("span", { class: "set__inline-note", text: note }));
        }
      } else {
        control = h("input", { class: "set__control", type: "number", inputmode: "numeric" });
        // ⚠️ 上下界是**内核的区间**（`SettingsForm::LIMITS`）：让客户**输不出**越界值。
        //    真正的闸门**始终**是内核的 `validate()`，越界时界面显示的是**内核原文**。
        if (typeof parameter.min === "number") control.setAttribute("min", String(parameter.min));
        if (typeof parameter.max === "number") control.setAttribute("max", String(parameter.max));
        control.value = String(edited[field]);
        control.addEventListener("input", () => {
          // ⚠️ 只记下用户敲的**原文**，不在这里 `parseInt` 出一个"我们以为他想要的值"：
          //    空串、`-`、`1e` 都是打字过程中的合法中间态，就地规范化会让输入框
          //    在用户打字时自己跳（而那是一种"我按的键没生效"的体感）。
          edited[field] = control.value;
          paintFooter();
        });
        row.append(control);
      }
      paramsRowsEl.append(row);
    }

    // 三句说明集中在表下面（理由见文件头那处偏离 ②）。
    for (const text of [payload.notes && payload.notes.limit_mbps, payload.notes && payload.notes.apply]) {
      if (typeof text === "string" && text.length > 0) {
        paramsNotesEl.append(h("p", { class: "set__note", text }));
      }
    }
  }

  /** 界面语言那一行：标题与三个选项名**全部来自载荷**（Rust），这里只摆位置。 */
  function paintLanguage() {
    const spec = payload && payload.language ? payload.language : null;
    if (!spec || !edited) {
      languageSectionEl.hidden = true;
      clear(languageRowEl);
      return;
    }
    languageSectionEl.hidden = false;
    setText(languageTitleEl, spec.title);
    clear(languageRowEl);
    for (const choice of spec.choices) {
      const checked = String(edited[LANGUAGE_FIELD]) === String(choice.value);
      const button = h("button", {
        class: "set__lang",
        type: "button",
        text: choice.label,
        role: "radio",
        "aria-checked": checked ? "true" : "false",
      });
      button.addEventListener("click", () => {
        if (!edited || edited[LANGUAGE_FIELD] === choice.value) return;
        edited[LANGUAGE_FIELD] = choice.value;
        paintLanguage();
        paintFooter();
      });
      languageRowEl.append(button);
    }
  }

  // ---------------------------------------------------------------------------
  // 保存条
  // ---------------------------------------------------------------------------

  /** 表里那七项与**内核手里那一份**是否一致（"没有要保存的改动"）。 */
  function hasUnsavedChanges() {
    if (!edited || !payload || !payload.settings) return false;
    return FIELDS.some(
      (field) => String(edited[field]) !== String(payload.settings[field])
    ) || String(edited[LANGUAGE_FIELD]) !== String(payload.settings[LANGUAGE_FIELD]);
  }

  /**
   * 保存条那一行：横幅 + 保存按钮的可用性与 help。
   *
   * ⚠️ 句子是**挑选**出来的（`banner.unsaved` / `banner.clean`），不是拼出来的：
   *    "有没有未保存的改动"是前端手里那个编辑中的表单与内核那份的比较结果，
   *    而**句子本身**必须由壳给（`api/settings.rs` 的成对发送就是为这个）。
   * ⚠️ help 是**三态**（引擎不可用 / 有改动 / 一致），三句都在载荷里
   *    （`save_help.*`），这里只按闸门与比较结果挑一句。
   */
  function paintFooter() {
    const banner = payload && payload.banner ? payload.banner : null;
    const help = payload && payload.save_help ? payload.save_help : null;
    const dirty = hasUnsavedChanges();
    if (banner) setText(bannerEl, dirty ? banner.unsaved : banner.clean);

    const wants = gateOpen && dirty && !busy && edited !== null;
    if (saveEl.disabled !== !wants) saveEl.disabled = !wants;
    if (help) {
      const text = !gateOpen ? help.engine_unavailable : dirty ? help.unsaved : help.clean;
      if (saveEl.getAttribute("title") !== text) saveEl.setAttribute("title", text);
    }
  }

  // ---------------------------------------------------------------------------
  // 失败原文
  // ---------------------------------------------------------------------------
  /** 一次保存失败的**内核原文**（`SettingsSaveFailure::message` = `error_text`，逐字）。 */
  function showFailure(message) {
    setText(failureTextEl, message);
    failureEl.hidden = false;
  }

  // ---------------------------------------------------------------------------
  // 下载目录那一段
  // ---------------------------------------------------------------------------

  /** 一个"知道了"的提示框（`DownloadDirectory::check` 给出的那句话）。 */
  function askProblem(message) {
    const handle = openModal("tpl-dialog-problem");
    setText(byIdIn(handle.root, "dlg-problem-message"), message);
    byIdIn(handle.root, "dlg-problem-ok").addEventListener("click", () => handle.close());
    return handle;
  }

  /**
   * 确认框：**三条后果**（`DownloadDirectory::confirmation`，逐字来自 Rust，有单测）。
   *
   * ⚠️ 确认那颗按钮的标题把**后果**再说一遍（「更改并重启内核」），不是一句空泛的
   *    「确定」—— 用户点它之前要能看出自己答应的是什么（macOS 同款）。
   *
   * @returns {Promise<boolean>} 用户点了确认 ⇒ `true`（点取消或按 Esc ⇒ `false`）
   */
  function askConfirm(message) {
    const handle = openModal("tpl-dialog-confirm");
    setText(byIdIn(handle.root, "dlg-confirm-message"), message);
    return new Promise((resolve) => {
      let accepted = false;
      byIdIn(handle.root, "dlg-confirm-cancel").addEventListener("click", () => handle.close());
      byIdIn(handle.root, "dlg-confirm-accept").addEventListener("click", () => {
        accepted = true;
        handle.close();
      });
      // ⚠️ 结论在**关闭时**给（`onClose` 是 `openModal` 保证会跑的那一次回调）——
      //    给在按钮上会漏掉"用户按 Esc / 点背影"那两条路，而那两条约等于"取消"。
      handle.onClose = () => resolve(accepted);
    });
  }

  /** 把当前偏好那一份落位（显示值 + 输入框的初值 + 那段脚注）。 */
  function paintPreferences() {
    if (!preferences) return;
    // ⚠️ **中间截断**（macOS `SettingsView` 那一格是 `.lineLimit(1).truncationMode(.middle)`）：
    //    切成两段交给 CSS（规则与理由见 `dom.js:middleSplit` —— 一个字符都不改，
    //    省略号由浏览器画）。
    // 🔴 **两个分隔符都要认**（`/` 与 `\`）：真机是 Windows，那边的路径里**一个 `/`
    //    都没有** ⇒ 只传 `"/"` 的话 `middleSplit` 回 `[整条, ""]`（`dom.js:107` 的兜底）
    //    ⇒ **尾段空着**，而缩的是 `.mid__head`、`.mid__tail` 是 `flex: none` ⇒
    //    "这一格一个字都不许少"的那个名字**恰好落进会被省略号吃掉的那一段**。
    //    （覆盖：`panels-harness.html` 的 ③目录w1 / ③目录w3 用一份 Windows 形状的
    //      载荷各钉一处。
    //    ⚠️ **POSIX 形状那一档的账要分开算**（2026-10-06，Task 4 第三轮重审测量后订正）：
    //       **回执**那一档真的有判据（③zb2 —— 把 `:558` 改成只认 `\` 会让它红）；
    //       而**显示行**那一档**没有**（把这两行改成只认 `\`，整屏 **140/0、一条都不红**）。
    //       原因是 ③b2 只断 `title`、③b 只断 `textContent`，**两者对"这条串从哪儿切开"
    //       都不敏感**。⇒ 别拿 ③b2 的绿当作这两行 POSIX 分支的判据；那是本批之前就有的
    //       缺口（这一行原先两种形状都没有判据），不是本批引入的。）
    setText(dirDisplayHeadEl, middleSplit(preferences.display, "/\\")[0]);
    setText(dirDisplayTailEl, middleSplit(preferences.display, "/\\")[1]);
    // 悬停给**全文**（原文那一份，不是切过的）—— 这是"截断之后还看得到全部"的出口。
    dirDisplayEl.setAttribute("title", preferences.display);
    dirInputEl.value = typeof preferences.dir === "string" ? preferences.dir : "";
    setText(dirNoteEl, preferences.section_note);

    // ⚠️ **勾选框那一格（规格 §2.4）**：标签与说明**跟着载荷下来**（`verbose_label` /
    //    `verbose_note`）—— 本文件一个字都不造（§3.2）。判"是不是开着"的是
    //    `verbose_logging` 这一格，而它是**盘上那一份**：这条命令每次落盘之后都会
    //    重读一次（见 `setVerboseLogging`），于是"存失败了"时勾会**跟着回去**，
    //    不会出现"勾着、而盘上没记住"那种下次启动才发现的分叉。
    setText(
      verboseLabelEl,
      typeof preferences.verbose_label === "string" ? preferences.verbose_label : ""
    );
    setText(
      verboseNoteEl,
      typeof preferences.verbose_note === "string" ? preferences.verbose_note : ""
    );
    verboseEl.checked = preferences.verbose_logging === true;
  }

  /**
   * 发起一次改动（`target` 空串 = **恢复默认**）：**先检查、再确认**。
   *
   * ⚠️ 顺序是**承重的**（`DownloadDirectory::check` 的头注记着）：目录不可用要
   *    **在确认对话框之前**就报出来 —— 等用户看完"正在跑的任务会停"、点了确认、
   *    内核重启完才发现"这个文件夹不可写"，他已经为它**停掉了一次正在跑的下载**。
   * ⚠️ 这两句话都是 `preferences_check` 一次给回来的（`{check, confirmation}`），
   *    所以"检查"与"确认"用的是**同一次调用**的结论 —— 中间不会插进一次别的改动。
   */
  async function askDirectory(target) {
    if (busy) return;
    busy = true;
    try {
      let plan;
      try {
        plan = await ctx.call(ctx.CMD.preferencesCheck, { path: target });
      } catch (error) {
        // 连"能不能用"都问不出来（宿主里没有这条命令）：**原地**说出原文，不改任何东西。
        showFailure(failureText(error));
        return;
      }
      if (plan && typeof plan.check === "string" && plan.check.length > 0) {
        askProblem(plan.check); // §2.3：确认**之前**就中止
        return;
      }
      const confirmed = await askConfirm(plan && plan.confirmation ? plan.confirmation : "");
      if (!confirmed) return;
      await applyDirectory(target);
    } finally {
      busy = false;
    }
  }

  /**
   * 「选择…」→ 弹一次**系统的**「选择文件夹」，把选中的那一个交给**同一条**流程。
   *
   * 对位 macOS `SettingsView.swift:236-247` 的 `choose()`：那边也是
   * "选完 ⇒ `ask(to: url.path)`"（检查 → 确认 → 写偏好 → 重启内核），
   * **不是**"只把路径填进格子、让用户自己去点「应用」"—— 两处说的本来是同一件事。
   *
   * ⚠️ **判据在 Rust**（本文件只读那一格）：`pick_directory` 回 `null` = 用户取消，
   *    而**取消什么都不该发生** ⇒ 输入框一个字都不动、也不进那条流程。
   *    （"空串也算取消"那条判据在 `shell-core` 的 `pickdir::pick_outcome`，
   *      本文件**不重写一遍** —— 壳已经保证它回不到空串。）
   *
   * ⚠️ 失败（宿主里没有这条命令 / 系统对话框起不来）**原地**说原文、不改任何东西：
   *    静默吞掉的话，用户点了「选择…」之后界面一动不动，与"卡住了"分不开（约束 4）。
   */
  async function chooseDirectory() {
    if (picking) return;
    picking = true;
    let picked;
    try {
      picked = await ctx.call(ctx.CMD.pickDirectory);
    } catch (error) {
      showFailure(failureText(error));
      return;
    } finally {
      picking = false;
    }
    if (typeof picked !== "string" || picked === "") return; // 取消
    // ⚠️ 先落进输入框**再**走那条流程：确认框被取消时（用户改主意了），
    //    输入框里留着他刚选的那个路径，一眼就能看出"我刚才选的是这儿"。
    dirInputEl.value = picked;
    await askDirectory(picked);
  }

  /**
   * 用户点了「更改并重启内核」之后才走到这里。
   *
   * ⚠️ 回执那四格**一个字都不加工**（`api::preferences::change` 已经是"算好的呈现值"）：
   *    · `headline` **任何路径下都一样长**（那是一次真实布局事故的修法：把用户选的
   *      完整路径塞进那条常驻回执，长路径会把底栏与侧栏摘要顶出窗口）；
   *    · `path_detail` 是**单独一行**的路径（`null` = 这一条没有路径行）；
   *    · `is_failure` 决定图标与颜色（**绘制**，不是文案）。
   *    ⇒ 这一屏与主区那条常驻行渲染的都是**这两行**，读的是同一次回执。
   */
  async function applyDirectory(target) {
    let change;
    try {
      change = await ctx.call(ctx.CMD.preferencesSet, { dir: target });
    } catch (error) {
      showFailure(failureText(error));
      return;
    }
    paintChangeReceipt(change);
    // ⚠️ 目录改成了 ⇒ **内核按新目录重启**（`preferences_set` 那四步里的第 ③ 步）。
    //    重启会重放当前批次（`restart_kernel` 的注释：用户回到文件页**还是原来那批**），
    //    所以这里补一拍，让引擎徽标与加载态**当场**跟着动 ——
    //    不然那几秒里界面显示的是"什么都没发生"，而内核其实正在换进程。
    ctx.poller.pollNow();
    // 显示值要跟着新目录走（落盘那一份可能被归一化过 —— 只去首尾空白）。
    await loadPreferences();
  }

  /**
   * 打开 / 关掉**详细诊断日志**（`verbose_logging_set`）。**改了会重启内核**。
   *
   * ⚠️ **没有确认框**（与改下载目录那条不同）：这一格不选路径、不会把文件导到别处去，
   *    macOS 那一侧也是一个直接的 `Toggle`。代价（正在跑的任务会停）已经写在
   *    那句说明里（`verbose_note` 的最后半句）—— 用户**按之前**就看得见。
   *
   * ⚠️ **它抢的是同一条 `busy`**（与改目录共用）：两条命令都会重启内核，
   *    同时来一发等于把刚起来的内核再换掉一次。
   *
   * ⚠️ **无论成败都重读一次偏好**：回执那一份可能被归一化过，而**失败时那一格更关键** ——
   *    写盘失败 / 内核起不来的话，界面上的勾必须回到**盘上那一份**，
   *    否则用户看到"勾上了"而根本没记下来（E-2 明禁的那种"以为存上了"）。
   */
  async function setVerboseLogging(on) {
    if (busy) return;
    busy = true;
    try {
      let change;
      try {
        change = await ctx.call(ctx.CMD.verboseLoggingSet, { on });
      } catch (error) {
        showFailure(failureText(error));
        await loadPreferences();
        return;
      }
      // 回执与改目录那条**同一片区域、同一条常驻行**（两件事都是"改了一项要重启内核
      // 的设置"，回执的形状也逐格相同 —— 见 `api::preferences::verbose_change`）。
      paintChangeReceipt(change);
      // 内核按新档位重启了 ⇒ 补一拍，让引擎徽标当场跟着动（同 `applyDirectory`）。
      ctx.poller.pollNow();
      await loadPreferences();
    } finally {
      busy = false;
    }
  }

  // ---------------------------------------------------------------------------
  // 「启动时检查更新」（规格 §4）
  // ---------------------------------------------------------------------------
  //
  // ⚠️ 与上面那个详细日志开关**不是一回事**：这一格**不重启内核**（开关状态在内核自己的
  //    `update.json` 里，内核下一次检查时自己读），所以它**不抢 `busy`**、也没有确认框。
  //    ⚠️ 它也不在 `settings_get` 的七项里（那七项是引擎参数），所以它不走 `payload`。

  /**
   * 读一次内核手里那个开关，把它落到勾选框上（打开窗口时调一次）。
   *
   * ⚠️ **失败静默**（规格 §5）：这一格是"启动时检查更新"，取不到时**保持默认**（勾着）——
   *    不弹任何东西（那条提示条与它的开关都不许给界面冒错误）。**默认就是开**。
   */
  async function loadUpdate() {
    let data;
    try {
      data = await ctx.call(ctx.CMD.updateStatus);
    } catch (error) {
      // 取不到 ⇒ 保持 `index.html` 里的默认（`checked`）。不上屏。
      return;
    }
    // 只有内核**明说** `enabled === false` 时才取消勾选；缺格 / 别的形状一律当"开"
    // （与规格 §5「开关默认开」同一条：拿不准就开）。
    updateEl.checked = !(data && data.enabled === false);
  }

  /**
   * 改那个开关（`update_set_enabled`）。**用返回值刷新界面**（内核回的是写完之后那一份）。
   *
   * ⚠️ **失败**：把勾选框退回**内核手里那一份**（重读一次）——否则用户看到"勾上了"、
   *    而内核根本没记住（E-2 明禁的那种"以为存上了"）。失败原文照登（约束 3）。
   */
  async function setUpdateEnabled(on) {
    let data;
    try {
      data = await ctx.call(ctx.CMD.updateSetEnabled, { enabled: on });
    } catch (error) {
      showFailure(failureText(error));
      await loadUpdate();
      return;
    }
    // 回执是"写完之后"那一份 ⇒ 用**它**刷新（不是用我们发出去的那个 `on`：
    // 万一内核归一化过，界面要跟内核走）。
    updateEl.checked = !(data && data.enabled === false);
  }

  /**
   * 把一条回执落到**某一节**那一行上（图标是**绘制**，文字全部来自载荷）。
   *
   * ⚠️ 抽出来是因为到 2026-10-15 这里已经有**三条**回执读**同一组键**
   *    （下载目录 / 详细日志 / 立即检测 —— 还有导出那一条同形但它多一格 `privacy_note`）：
   *    `api::preferences::receipt` 那一侧的注释说得很清楚，"同一组键只许有一份实现"，
   *    前端这边同一条纪律 —— 三份复制粘贴的结局是某一格哪天在一边改了、另两边没改，
   *    而**不会有什么东西变红**（少的那一行只是安静地不出现）。
   *
   * ⚠️ 它只画**标题 + 可选的一行路径**（那条路径的契约：单行 + 中间截断 + 悬停看全文）。
   *    多出来的行（导出的 `privacy_note`）由调用方在它之后再往 `els.text` 上追加 ——
   *    本函数不猜"你还有没有别的东西要画"。
   *
   * 返回那三格（`headline` / `detail` / `failure`）：调用方要用它们做别的事
   * （改目录那条要把同一件事推到主区那条常驻行上）。
   */
  function paintReceipt(els, change) {
    els.receipt.hidden = false;
    clear(els.icon);
    const failure = change && change.is_failure === true;
    els.icon.className = failure ? "dlg__receipt-icon is-failure" : "dlg__receipt-icon";
    // ⚠️ 这两个名字是**绘制**层的选择（`dom.js:ICONS` 是图形的家）：载荷给的是
    //    `is_failure` 这个**语义档**，本文件只决定它画成什么 ——
    //    与 `render.js:noticesFromState` 里"banner 的 kind → 哪个图标"逐字同一分工。
    els.icon.append(
      icon(
        failure ? "exclamationmark.triangle.fill" : "checkmark.circle.fill",
        "dlg__receipt-svg"
      )
    );
    const headline = change && typeof change.headline === "string" ? change.headline : "";
    const detail = change && typeof change.path_detail === "string" ? change.path_detail : "";
    clear(els.text);
    els.text.append(h("span", { class: "dlg__receipt-headline selectable", text: headline }));
    if (detail) {
      // 路径单独一行、可悬停看全文（`headline` 里**不含**路径 —— 那正是两格分开的理由）。
      // ⚠️ **中间截断**（`app_preferences` 的 `path_detail` 逐字："单行 + 中间截断 +
      //    悬停看全文"）：切在**最后一个分隔符**之后 ⇒ **文件名那一格一个字都不少**。
      //    🔴 两个分隔符都认（理由与显示行那条逐字相同：真机是 Windows，路径里没有 `/`）。
      //    两格之间不许有空白文本节点（否则跨格复制会多一个空格）。
      const [pathHead, pathTail] = middleSplit(detail, "/\\");
      const pathEl = h("span", { class: "dlg__receipt-path selectable", title: detail });
      pathEl.append(h("span", { class: "mid__head", text: pathHead }));
      pathEl.append(h("span", { class: "mid__tail", text: pathTail }));
      els.text.append(pathEl);
    }
    return { headline, detail, failure };
  }

  /** 把一条改目录的回执落到窗口里那一行上。 */
  function paintChangeReceipt(change) {
    const { headline, detail, failure } = paintReceipt(
      { receipt: dirReceiptEl, icon: dirReceiptIconEl, text: dirReceiptTextEl },
      change
    );
    if (hooks.onReceipt) {
      hooks.onReceipt({
        id: "download-dir-change",
        kind: failure ? "error" : "info",
        icon: failure ? "exclamationmark.triangle.fill" : "checkmark.circle.fill",
        text: headline,
        // ⚠️ 主区那条常驻行渲染的也是**这两行**：正文是固定长的标题、路径单独一格
        //    （带高度上限）—— 绝不让那条行的高度由路径长度决定（`resident_notice` 的不变量）。
        hint: detail || null,
        dismiss: true,
      });
    }
  }

  // ---------------------------------------------------------------------------
  // 导出诊断日志（规格 §2.5）
  // ---------------------------------------------------------------------------

  /**
   * 「导出诊断日志…」：**一次系统对话框 + 一次拷贝**，回执就落在**本节里**（按钮下面）。
   *
   * ⚠️ **回执画在本节**（不是下载目录那一节的那一行）：用户点的是这一节里的按钮，
   *    确认就该出现在**同一节**里（上一轮那个开关的教训：勾选框在自己那一节、
   *    回执却画在头顶那一节 —— 不是假话，是误导）。
   *
   * ⚠️ **三档都不是错误**（命令层的那条口径）：成功 / 取消 / 失败都走**成功信封**，
   *    由 `is_failure` 决定图标与颜色。取消那一档也**必须说一句** ——
   *    用户点了「导出」而界面一动不动，与"卡住了"分不开（约束 4）。
   *    只有**命令本身发不出去**（宿主里没有这条命令）才走 `showFailure` 那条既有路径。
   */
  async function exportDiagnostics() {
    if (exporting) return;
    exporting = true;
    try {
      const change = await ctx.call(ctx.CMD.diagnosticsExport);
      paintExportReceipt(change);
    } catch (error) {
      showFailure(failureText(error));
    } finally {
      exporting = false;
    }
  }

  /**
   * 把一次导出的回执落到本节那一行上（图标是**绘制**，文字全部来自载荷）。
   *
   * ⚠️ 前三格与改设置那两条**同形**（`headline` / `path_detail` / `is_failure`）——
   *    2026-10-15 起画法是**同一份** `paintReceipt`（那三条各写一遍结构的时代结束了）。
   *    多出来的第五格 `privacy_note` 是规格 §2.7 要求的**第二处明说** ——
   *    它**只在成功那一档有值**（没导出东西的时候没有什么可提醒的），所以是"有才画"，
   *    而且**由本函数追加**（`paintReceipt` 不管"你还有没有别的东西要画"）。
   *
   * ⚠️ **中间截断那两个分隔符的判据在本机看得见**：夹具里那一条（`diagnosticsExportDone`）
   *    **就是 Windows 形状的**（`D:\导出\诊断日志-…`，故意的）⇒
   *    `panels-harness.html` 的 ③导出6 会在有人把共用的那一份退回 `"/"` 时红
   *    （实测：`["D:\\导出\\诊断日志-…",""]`）。
   */
  function paintExportReceipt(change) {
    paintReceipt(
      { receipt: exportReceiptEl, icon: exportReceiptIconEl, text: exportReceiptTextEl },
      change
    );
    const note = change && typeof change.privacy_note === "string" ? change.privacy_note : "";
    if (note) {
      exportReceiptTextEl.append(h("span", { class: "set__note selectable", text: note }));
    }
  }

  // ---------------------------------------------------------------------------
  // 立即检测更新（规格 §2）
  // ---------------------------------------------------------------------------

  /**
   * 「立即检测」：**不受 24 小时节流**的一次手动检查（内核早就备好了 `update_check_now`，
   * 壳一直到 2026-10-15 才把这条命令与这颗按钮接上）。
   *
   * 🔴 **它存在的理由是那句可能的假话**：内核的 `update_status` 在"这次没查成"与
   *    "查成了、确实没有新版"两种情形下**长得一样**（`has_newer:false`）。后台那条提示条
   *    对此无所谓（没人做动作就不冒话，规格 §5），但**客户点出来的**这一下只按
   *    `has_newer` 判就只剩"已经是最新的" —— 断网时那是**界面在撒谎**。
   *    判决与那五句话住在 `shell_core::presentation::update_check`（内核为此多了一格
   *    `last_check_ok`），**本文件一个字的文案都不写**（R-24），只把载荷原样摆上去。
   *
   * ⚠️ **回执落在本节里**（同导出那一条的理由）：用户点的是这一节里的按钮。
   * ⚠️ **点完还要补一拍**（`pollNow`）：这一条会**改掉内核手里那一份更新状态**
   *    （`last_checked_at` / `last_seen` / `last_check_ok`），而壳层那条提示条读的正是
   *    同一份（`update_status`，每 60 秒一拍）—— 不补这一拍，用户点完"有新版本"
   *    还得等下一次轮询才看见那条提示条（甚至更久：桌面版在设置窗口开着时那一条
   *    也是每 60 秒才动一次）。**"我按了按钮，界面却不跟着动"是这一节最忌的形状。**
   *
   * ⚠️ **命令本身发不出去**（宿主里没有这条命令）走 `showFailure` 那条既有路径；
   *    而"没查成""开关关着"都是**回执里那一档**（它们是成功信封 + `is_failure`），
   *    与导出那条的分工逐字相同。
   */
  async function checkUpdateNow() {
    if (checkingUpdate) return;
    checkingUpdate = true;
    updateNowEl.setAttribute("aria-busy", "true");
    try {
      const change = await ctx.call(ctx.CMD.updateCheckNow);
      paintUpdateReceipt(change);
      ctx.poller.pollNow();
    } catch (error) {
      // 🔴 **命令本身都发不出去 ⇒ 这一下没有任何结论**：把上一次那份回执也清掉。
      //    留着它会与下面那条失败行并排挂着，读起来像"刚才那一次的结论仍然成立"——
      //    而它说的是**上一次**（可能几天前）那一份，两句话并排就是一次误导。
      updateReceiptEl.hidden = true;
      showFailure(failureText(error));
    } finally {
      checkingUpdate = false;
      updateNowEl.removeAttribute("aria-busy");
    }
  }

  /** 把一次「立即检测」的回执落到本节那一行上（同一份画法，文字全部来自载荷）。 */
  function paintUpdateReceipt(change) {
    paintReceipt(
      { receipt: updateReceiptEl, icon: updateReceiptIconEl, text: updateReceiptTextEl },
      change
    );
  }

  // ---------------------------------------------------------------------------
  // 取数
  // ---------------------------------------------------------------------------

  /**
   * 读偏好那一份（`preferences_get`）。
   *
   * ⚠️ 它**与参数表分开**（macOS 的一个判断）：内核参数面板要等握过手才画得出来，
   *    而"改下载目录"恰恰是**内核起不来时**最可能要去改的一件事（状态文件就在下载目录里）。
   *    两者写在一个 `try` 里，内核一挂下载目录那一段就跟着消失 ——
   *    而那正是用户最需要它的时候。
   */
  async function loadPreferences() {
    try {
      preferences = await ctx.call(ctx.CMD.preferencesGet);
      paintPreferences();
    } catch (error) {
      showFailure(failureText(error));
    }
  }

  /**
   * 读七项参数（`settings_get`）并播种表单。
   *
   * ⚠️ **壳里没有"客户端认为合理的一组参数"**：取不到就画"内核还没交出参数面板"
   *    + **原文**，绝不编一份默认值出来 —— 编出来的表现是"客户看到一组他从没设过的参数"。
   * ⚠️ **重播种会把表里没保存的改动丢掉**。所以只在三处调：打开、保存成功（用**回执那一份**）、
   *    以及用户主动重读。`render(state)` 那一拍**不调它**（引擎态一秒一变，
   *    跟着重播种会让用户敲到一半的数字每秒被抹回去）。
   */
  async function loadSettings() {
    try {
      payload = await ctx.call(ctx.CMD.settingsGet);
    } catch (error) {
      payload = null;
      edited = null;
      paintParams();
      setText(unavailableTextEl, failureText(error));
      unavailableEl.hidden = false;
      paintFooter();
      return;
    }
    unavailableEl.hidden = true;
    seed();
  }

  /** 用**内核手里那一份**重新填表（保存成功后的回执也走这一条路）。 */
  function seed() {
    const settings = payload && payload.settings ? payload.settings : null;
    if (!settings) {
      edited = null;
      paintParams();
      paintLanguage();
      paintFooter();
      return;
    }
    edited = {};
    for (const field of FIELDS) edited[field] = settings[field];
    edited[LANGUAGE_FIELD] = settings[LANGUAGE_FIELD];
    paintParams();
    paintLanguage();
    paintFooter();
  }

  // ---------------------------------------------------------------------------
  // 事件
  // ---------------------------------------------------------------------------
  closeEl.addEventListener("click", () => close());
  dirChooseEl.addEventListener("click", () => {
    void chooseDirectory();
  });
  dirApplyEl.addEventListener("click", () => {
    void askDirectory(dirInputEl.value);
  });
  dirInputEl.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    void askDirectory(dirInputEl.value);
  });
  // 「恢复默认」：**空串**是"回到未配置"（`AppPreferences::setting_download_dir` 的既有语义），
  // 不是"删掉这个设置"。它同样要过检查（`check("")` 直接放行 —— 那不是路径）与确认。
  dirResetEl.addEventListener("click", () => {
    void askDirectory("");
  });
  aboutEl.addEventListener("click", () => {
    openAbout(ctx);
  });
  licenseEl.addEventListener("click", () => {
    openLicense(ctx);
  });
  saveEl.addEventListener("click", () => {
    void save();
  });
  // 详细日志那个勾选框：**只有真的变了才发命令**（`change` 事件本身就是这个语义，
  // 而命令那一侧还有一道"同值 ⇒ 什么都不做"的兜底 —— 一条迟到的重复不许把
  // 用户正在跑的任务重启掉）。
  verboseEl.addEventListener("change", () => {
    void setVerboseLogging(verboseEl.checked);
  });
  // 「启动时检查更新」：**变了才发命令**（`change` 事件本身就是这个语义）。它**不重启内核**
  // （与上面那个不同），所以没有 `busy` 那道闸。
  updateEl.addEventListener("change", () => {
    void setUpdateEnabled(updateEl.checked);
  });
  // 「立即检测」：一次手动检查（与上面那个勾选框是两件事 —— 那颗说"以后"，这颗说"现在"）。
  updateNowEl.addEventListener("click", () => {
    void checkUpdateNow();
  });
  // 「导出诊断日志…」：**一次系统对话框 + 一次拷贝**，回执落在本节里那颗按钮下面
  // （与上面那个勾选框同一节 —— 见 `exportDiagnostics` 的文档）。
  exportEl.addEventListener("click", () => {
    void exportDiagnostics();
  });

  /**
   * 保存：`set_settings`。**失败就把内核原文摆出来**（约束 3/4），不重写、不吞掉。
   *
   * ⚠️ 成功之后**用回执那一份重填**（`settings_set` 回的是内核归一化之后那一份，
   *    例如 `-k` 的 `"21m"` 会变成 `"21M"`）—— 客户看得见它变了。
   *    这正是"保存按钮会自己变灰"那条可见证据的来源（`SettingsForm::matches`）。
   */
  async function save() {
    if (busy || !edited) return;
    busy = true;
    paintFooter();
    // 🔴 **数字项必须发数字**：`Settings` 的六个数值成员是 `i32`/`i64`，而输入框里
    //    那一格永远是**字符串**（`input.value`）。发 `"4"` 过去，Tauri 的反序列化
    //    会当场拒掉这条命令 —— 而那句拒绝是宿主给的英文形状的话，不是内核说的。
    //    ⇒ 在**唯一**的发点上转一次。
    // ⚠️ 空输入框 `Number("")` 是 `0`：**不在这里替他编一个合法值**，也不把那一格
    //    变成非法形状（`NaN` → JSON 的 `null`，那会连命令都进不去）。发 `0` 出去，
    //    由内核的 `validate()` 拒掉并说出它那句原文（"并行文件数必须在 1–64 之间，
    //    当前 0"）—— 判据在内核，界面只登原文（§3.2 的同一条分工）。
    const body = {};
    for (const field of FIELDS) {
      body[field] = field === enumField ? edited[field] : Number(edited[field]);
    }
    // 语言是**字符串**（`system`/`zh`/`en`），必须原样带上：
    // 内核的 `set_settings` 是整份替换，漏掉它 = 改一次并行数就把语言重置回跟随系统。
    body[LANGUAGE_FIELD] = edited[LANGUAGE_FIELD];
    try {
      payload = await ctx.call(ctx.CMD.settingsSet, { settings: body });
      seed();
    } catch (error) {
      // ⚠️ 失败**不重新播种**：用户敲进去的那一份要留在表里（他会照着内核那句话改）。
      showFailure(failureText(error));
    } finally {
      busy = false;
      paintFooter();
    }
  }

  // 打开时各读一次（三条并发：它们互不依赖，串起来只会让窗口慢一拍）。
  void loadPreferences();
  void loadSettings();
  void loadUpdate();
  paintFooter();

  return {
    /**
     * 壳每一拍把整个 `state()` 交进来。
     *
     * 本窗口只从它取**一样东西**：**引擎闸门**（`allows_requests`）——
     * 它是"保存"那颗按钮的可用性判据之一，而闸门是 Rust 算的
     * （`state().allows_requests` ← `EngineGate::allows_requests`），
     * 本文件**只读不判**（`poll.js` 的同一条口径）。
     *
     * ⚠️ **不在这里重取参数**：`state()` 每秒来一次，跟着重播种 = 用户敲到一半的
     *    数字每秒被抹回去（而那种缺陷表现为"输入框不好用"，不像 bug）。
     */
    render(state) {
      const next = state && state.allows_requests === true;
      if (next === gateOpen) return;
      gateOpen = next;
      paintFooter();
    },
    close,
  };
}
