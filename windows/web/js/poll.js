// poll.js —— 轮询节拍（规格 §3.5）。
//
// 内核**没有推送**（协议是纯请求/响应，全仓无事件通道）⇒ 前端只能拉。
// 这张表就是"拉什么、多快拉"的**唯一一份**：
//
//   | 数据        | 节拍  | 依据                                             |
//   | `state()`   | 1 s   | 引擎/加载态的变化远慢于传输                       |
//   | `transfers()`| 200 ms| `TransferListPoll::INTERVAL_NANOSECONDS`（既有常量）|
//   | `verify()`  | 1 s   | **仅在校验页可见时**；否则一拍都不发               |
//   | `tree()`    | 只在用户操作时 | ——（不在本文件里，它没有节拍）                |
//
// ⚠️ 200 ms 那个数**不是**在这里定的，它抄自 `shell-core`：
//    `presentation::transfer_row::TransferListPoll::INTERVAL_NANOSECONDS = 200_000_000`。
//    ⚠️ **JS 读不到那个常量**（它在 Rust 里，而它没有随任何命令下来）⇒
//       下面是它的一份**抄本**。这是本文件唯一一处"值有两个家"，所以它带一条自己的
//       守护：`windows/scripts/` 里目前**没有**东西比对这两个数 ——
//       **本任务如实记账，不假称它被守住了**。真要收口，要么让 `state()` 带上它、
//       要么在 `test.sh` 里加一条 `grep`（那是任务 7/11 的事）。

/** 节拍表（毫秒）。键与 `CMD` 的命令名一致，免得两处对不上。 */
export const INTERVALS_MS = Object.freeze({
  state: 1000,
  transfers: 200,
  verify: 1000,
});

/**
 * 一个按节拍重复跑的调度器。
 *
 * 三条性质，缺一条都会在真机上出问题：
 *
 * ① **重入保护**：上一拍还没回来时**跳过**这一拍，不排队。
 *    没有它的话，内核卡住（或响应慢于节拍）时请求会**越堆越多**，而
 *    `CoreClient` 是一条 FIFO 串行队列 —— 堆到一定程度就是"界面越来越晚"，
 *    用户看到的是"点什么都没反应"（约束 4 的静默失效）。
 *    跳过的代价是"这一拍没更新"，那个代价是**有界**的。
 *
 * ② **停机是同步的**：`stop` 立刻清掉定时器，不等在飞的那一次返回。
 *    切分区/关窗时它必须马上停 —— 规格 §3.5 的闸门要求"仅在校验页可见时 1 s"，
 *    而"等它自己跑完"会让那个"仅"字失效。
 *    在飞的那一次**照常落地**（结果不会被丢掉，只是不再有下一拍）。
 *
 * ③ **闸门**：`allows_requests == false` 时**不发**除 `state` / `retry` 以外的任何命令
 *    （`EngineGate` 的既有语义，规格 §3.5）。判据本身在 Rust
 *    （`state().allows_requests`），这里只**执行**，不重算 ——
 *    让 JS 自己判就等于把那道判据抄进前端（§3.2）。
 */
export class Poller {
  constructor() {
    /** @type {Map<string, {timer: number, inFlight: boolean}>} */
    this._jobs = new Map();
    /** 闸门：`false` 时只有 `ungated` 的任务照跑。 */
    this._gate = false;
  }

  /**
   * 起一个任务（同名的会被替换 —— **不叠加**，否则每切一次分区就多一条定时器）。
   *
   * @param {string} name 任务名（用 `CMD` 里的名字）
   * @param {number} intervalMs
   * @param {() => Promise<void>} run 一次拉取。⚠️ **它必须自己接住失败**
   *   （本文件不重试、不转成文案 —— 重连是 `retry()` 那条命令的事）。
   *   它没接住的会落到 `_fire` 的控制台日志里（那是缺陷，不是正常路径）。
   * @param {boolean} [ungated] `true` = 不看闸门。只有 `state` 与 `retry` 是这类：
   *   闸门本身就是 `state` 报出来的，`state` 自己再被闸门挡住就永远解不开。
   */
  start(name, intervalMs, run, ungated = false) {
    this.stop(name);
    const job = { name, timer: 0, inFlight: false, run, ungated };
    this._jobs.set(name, job);
    job.timer = setInterval(() => this._fire(name, job), intervalMs);
    // 立刻跑第一拍：不然"启动后第一秒"里界面是空的，而那正是最需要说点什么的一刻。
    this._fire(name, job);
  }

  /**
   * 跑一拍。**三条闸门合起来决定发不发**：重入保护 → 闸门 → 真的跑。
   *
   * ⚠️ `run` **抛出来的东西在本文件里就地终止**：不重试、不转成文案、不往上冒。
   *    不往上冒是刻意的 —— `setInterval` 的回调里冒出来的异常会变成一次
   *    **未处理的 Promise 拒绝**（在 webview 里只会进控制台），
   *    而"某一拍失败了"这件事的**正确落点**是那个任务的 `run` 自己
   *    （`app.js` 的那一条会把它变成常驻提示行上的一句原文）。
   *    这里只留一条控制台日志：**它是诊断，不是界面文案**（见 `invoke.js` 的界线）。
   */
  _fire(name, job) {
    if (job.inFlight) return; // ① 重入保护
    if (!job.ungated && !this._gate) return; // ③ 闸门
    job.inFlight = true;
    Promise.resolve()
      .then(job.run)
      .catch((error) => {
        // 走到这里说明**任务的 run 自己没接住**（正常路径下它接得住）。
        // 那是程序缺陷 ⇒ 进控制台指名道姓，而不是悄悄消失。
        // ⚠️ 措辞是**挑过的**，别顺手改回去：内嵌字体子集里没有「询」与「抛」这两个字，
        //    而这句话也会被字体覆盖判据扫（`test.sh` 第 0.6 步）—— 拼不成词的字在
        //    控制台里就是两个问号，而那一刻正是你要读它的时候。
        //    「节拍任务」与「冒出来」在本文件里与「轮询节拍」「冒出来的异常」同源
        //    （见 `_fire` 的头注），不是另外起的一套词。
        console.error(`节拍任务 ${name} 冒出了一个没被接住的错误：`, error);
      })
      .finally(() => {
        job.inFlight = false;
      });
  }

  /** 停一个任务（幂等；没有这个任务时什么都不做）。 */
  stop(name) {
    const job = this._jobs.get(name);
    if (!job) return;
    clearInterval(job.timer);
    this._jobs.delete(name);
  }

  /** 停掉全部（关窗时用）。 */
  stopAll() {
    for (const name of [...this._jobs.keys()]) this.stop(name);
  }

  /**
   * 设置闸门。**只影响"下一拍发不发"**，不取消在飞的那一次
   * （取消在飞请求需要内核侧的取消语义，本代没有）。
   */
  setGate(allowsRequests) {
    this._gate = allowsRequests === true;
  }

  /**
   * 立刻补一拍（不改变节拍）。
   *
   * 给"用户刚做了一个动作"用：点「加载」之后最多要等一整秒的 `state` 轮询才能看到
   * 反应，而那一秒里用户看到的是**按了没反应**（约束 4 最恨的形态之一）。
   * ⇒ 命令一返回就补一拍，把那一秒压到一帧。
   *
   * ⚠️ 它**不绕过重入保护**（在飞的那一次还在跑就跳过）：这条通道是给"快一点"用的，
   *    不是给"多一条并发请求"用的 —— 内核侧是 FIFO 串行队列（约束 15），
   *    在这里绕过去就等于把队列堵死的风险从定时器搬到了点击上。
   */
  pollNow() {
    for (const job of this._jobs.values()) this._fire(job.name, job);
  }

  /** 当前闸门状态（给屏自己判断用，不参与调度）。 */
  get gateOpen() {
    return this._gate;
  }
}
