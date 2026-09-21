// invoke.js —— **前端唯一的后端接触面**（规格 §3.1 的依赖方向：webview ──invoke──▶ 命令层）。
//
// 全仓**只有这一个文件**碰 `window.__TAURI__`。这条不是洁癖：命令层是唯一一处
// 能把 `{ok,data|error}` 信封拆开的地方，第二个碰 Tauri 的文件迟早会自己拆一次
// （然后"失败原文逐字透传"就在两条路上各自演化）。要加命令**只在 `call` 的调用点加**。
//
// ---------------------------------------------------------------------------
// 🔴 两条界线（本文件是它们唯一被写下来的地方，改之前先读）
// ---------------------------------------------------------------------------
//
// 1. **界面文案 vs 诊断**（规格 §3.2 的边界）
//    · **界面文案**（要告诉客户"这个世界怎么样了"）：状态、大小、时间、百分比、
//      错误原文、空态提示、按钮标题 —— **一个字都不许在本文件里出现**，
//      它们一律由 Rust 的 `shell_core::presentation::*` 算好、随信封下来。
//    · **诊断**（要告诉**开发者**"程序自己坏了"）：本文件里那几句指名道姓的话
//      （"没有 Tauri"、"命令名不存在"）属于这一类。它们描述的是**管线**坏了，
//      不是**业务**怎么了 —— 而管线坏了的时候，Rust 那边一个字都给不出来
//      （请求根本发不出去），所以这句话没有别的地方可放。
//      ⚠️ 简报明文要求"没有 Tauri 时要抛一句指名道姓的话，而不是白屏"——
//         这条要求本身就说明诊断文案允许存在。但**只允许这一类**。
//
// 2. **失败原文逐字**（约束 3）
//    `call` 失败时抛出的 `Error.message` 就是**内核/宿主说的那句话本身**：
//    不 trim、不截断、不按行拆、不加前缀、不换一句更客气的话。
//    它会被主区或常驻提示行**原样**显示，客户要能把它整段复制给业务方。

/**
 * **任意一种拒绝 → 要显示的那句话**（逐字，不加工）。
 *
 * 🔴 **为什么必须有这一个函数**（这是修一个真实的显示缺陷，不是防御性编程）：
 *    Tauri 的 `#[tauri::command]` 在**参数反序列化失败**、**命令不存在**、**权限被 ACL 挡下**
 *    这几种情形里，reject 出来的是**一个裸字符串**，不是 `Error` ——
 *    例如 `"state not found"` / `"invalid args \`code\` ..."`。
 *    （Tauri 侧是 `InvokeError::from_anyhow` / `Deserialize` 那几条路，
 *     它们最终 `reject(Value::String(msg))`。）
 *
 *    修之前这里写的是 `error.message`：裸字符串取 `.message` 得 `undefined`
 *    ⇒ 交给界面的是**空串** ⇒ **每一条失败提示都渲染成一个没有文字的红三角**。
 *    那正好打在本代最要紧的那条纪律上（约束 3：错误原文逐字透传）——
 *    **客户看不到内核/宿主说的话**，而且它看起来"就是界面丑"，不像一个缺陷。
 *
 * ⚠️ 这条路径**曾经被"没有 Tauri"那条分支结构性地盖住**：无头验证走的是
 *    `resolveInvoke()` 抛出的 `Error`（有 `message`），三种拒绝形状里只覆盖到一种。
 *    ⇒ 现在三种形状各有一条**受控 stub** 的实测（`windows/web/_stub.html` 的验证脚本，
 *      输出贴在本轮的报告里）。
 *
 * 取值顺序（**逐字优先**）：
 *   1. **字符串** —— 它本身就是宿主/内核说的那句话，直接用；
 *   2. 有 `message` 字符串的对象（`Error` 及大多数 `{message}` 形状）—— 用 `message`；
 *   3. 其余对象 —— `String(error)`；
 *   4. `null` / `undefined` —— **空串**，**不**写成 `"undefined"`：
 *      那是"宿主什么都没说"，与 `error_text.rs` 那条"内核给的是空串时不编一个占位出来"
 *      同一条口径（编一个占位等于把"没说"和"没听懂"合并成同一种呈现）。
 */
export function failureText(error) {
  if (typeof error === "string") return error;
  if (error && typeof error.message === "string") return error.message;
  if (error === null || error === undefined) return "";
  return String(error);
}

/** 当前宿主上可用的 `invoke`。每次调用现取（不用模块级常量缓存）。 */
function resolveInvoke() {
  // Tauri 2 的全局注入面。⚠️ 它**不是默认存在的**：要 `tauri.conf.json` 的
  // `app.withGlobalTauri` 为 `true`（默认 `false`）。本仓库已经把它打开
  // （`shell-win/tauri.conf.json`，那个开关的 `deny_unknown_fields` 意味着打错字会当场构建失败）。
  // 为什么不用 `window.__TAURI_INTERNALS__`（那个恒在）：它是**内部** IPC 桥，
  // 官方 API 包怎么变它就怎么变 —— 拿它当契约等于把"能不能调命令"押在别人不保证的东西上。
  const tauri = globalThis.__TAURI__;
  const invoke = tauri && tauri.core && tauri.core.invoke;
  if (typeof invoke === "function") return invoke;
  // 诊断（不是界面文案，见文件头）：指名道姓地说出**缺的是哪一层**，
  // 因为这两种"白屏"的补救完全不同 —— 一个是打开了 `index.html`，
  // 一个是宿主配置丢了那个开关。含糊的一句话会让人往错的方向查。
  throw new Error(
    "没有找到 Tauri 的 invoke：window.__TAURI__ 不存在。"
      + "若这是浏览器里直接打开的页面，那是预期的（前端要在宿主里跑）；"
      + "若是在宿主里，检查 tauri.conf.json 的 app.withGlobalTauri 是否为 true（缺了它 Tauri 不注入全局对象）。"
  );
}

/**
 * 调一条命令，返回信封里的 `data`；失败时抛出**原文**。
 *
 * @param {string} cmd 命令名（`state` / `load` / `retry` / `tree` / `enqueue` /
 *   `transfers` / `verify` / `task_action` / `reveal` / `settings_get` /
 *   `settings_set` / `preferences_get` / `preferences_set` / `history_get` /
 *   `history_put` / `license` / `about` —— 规格 §3.4 的表）。
 *   ⚠️ 这几个名字是本文件里**唯一**出现的命令名，别在屏文件里写字符串字面量。
 * @param {object} [args] 参数。Tauri 默认按 camelCase 映射到 Rust 的 snake_case 形参
 *   （`base_url` → `baseUrl`），**这里传的键名以命令层的形参为准**（任务 7 定，
 *   规格 §3.4 的表格写的是 `load(code, base_url)`）。
 * @returns {Promise<*>} 信封的 `data`
 * @throws {Error} `message` 是失败原文逐字（可能是空串 —— 那是"内核说了空话"，
 *   `error_text.rs` 有一条测试钉着"空串不许补占位"）
 */
export async function call(cmd, args = {}) {
  // ⚠️ **这一层 try/catch 是承重的**：`invoke` 的拒绝**不是** `Error`（见 `failureText`）。
  //    不接住的话，裸字符串会一路原样冒到调用点的 `error.message` 上，变成一句空话。
  let envelope;
  try {
    envelope = await resolveInvoke()(cmd, args);
  } catch (error) {
    throw new Error(failureText(error));
  }

  // ⚠️ 判定顺序是**先判成功**：`ok` 必须**显式**是 `true`。
  //    写成 `if (!envelope.ok)` 的话，一个 `{}`（命令层忘了套信封）会被当成失败，
  //    于是"命令层坏了"与"内核报错了"合并成同一种呈现 —— 两条路要查的东西完全不同。
  if (envelope && envelope.ok === true) return envelope.data;

  // 失败：把原文交出去。⚠️ **不在这里判断 `code`** —— 错误码是给壳分支用的，
  //    不该出现在给客户看的这句话里（`error_text.rs` 的测试逐字钉着这一条）。
  const message =
    envelope && envelope.error && typeof envelope.error.message === "string"
      ? envelope.error.message
      : "";
  throw new Error(message);
}

/** Tauri 与 invoke 此刻可用吗（给"要不要起轮询"这类判断用，不抛）。 */
export function isHostAvailable() {
  try {
    resolveInvoke();
    return true;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// 命令名
// ---------------------------------------------------------------------------
// 集中在这里的唯一理由：屏文件里出现 `call("transfers")` 这样的字面量时，
// 命令改名会**静默**变成一次运行时 404（Tauri 的报错是宿主给的、不指向调用点）。
// 有了这张表，改名是**一处**改动 + 一次 `grep`。
//
// ⚠️ 名字逐字来自规格 §3.4 的表，**不许自己加**（任务 7/8 才是它们的实现者）。
export const CMD = Object.freeze({
  state: "state",
  load: "load",
  retry: "retry",
  tree: "tree",
  enqueue: "enqueue",
  transfers: "transfers",
  verify: "verify",
  taskAction: "task_action",
  reveal: "reveal",
  settingsGet: "settings_get",
  settingsSet: "settings_set",
  preferencesGet: "preferences_get",
  // ⚠️ **这一条规格 §3.4 的表里没有**（那张表只列了 `preferences_get` / `preferences_set`）。
  //    它是任务 8 补的第二个缺口（第一个是 `enqueue`，见 R-13）：前端要拿到
  //    `DownloadDirectory::check` 的结论与那三条后果的确认文案，而两者**都只能由 Rust 给**
  //    （§3.2：JS 不许自己拼那句中文，也问不了文件系统）—— 所以必须有这一条命令。
  //    实现与理由写在 `shell-win/src/commands.rs:preferences_check` 的头注里。
  preferencesCheck: "preferences_check",
  preferencesSet: "preferences_set",
  // ⚠️ **这一条规格 §3.4 的表里也没有**（同 `preferencesCheck`）：它是一条**系统对话框**
  //    的路，第二代与 macOS 都没有对应的"端点"（macOS 在视图里直接调 `NSOpenPanel`，
  //    不过 wire）。它补的缺口是：改下载目录原先只能把路径**粘**进输入框
  //    （2026-09-20 真机反馈）—— 弹一个系统的「选择文件夹」是**平台 API**，
  //    只能在壳里做。实现与理由写在 `shell-win/src/commands.rs:pick_directory` 的头注里。
  pickDirectory: "pick_directory",
  historyGet: "history_get",
  historyPut: "history_put",
  license: "license",
  about: "about",
});
