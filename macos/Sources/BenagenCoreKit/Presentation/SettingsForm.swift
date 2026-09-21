import Foundation

// ---------------------------------------------------------------------------
// 参数面板的**呈现模型**（任务 10，设置窗口）
//
// 为什么这些映射在这里而不在视图里（全局约束 8）：
//   「把内核数据变成界面值」的纯计算一律放 `Presentation/`，**每条都要有单测**；
//   `Sources/BenagenDownloader/` 下的视图文件里不许有非渲染逻辑。下面每一条都能写出
//   断言，而且有三条背着硬约束：
//     · **取值集合来自内核**：`-k` 的枚举面是 `hello` 给的（契约 §2.3），壳不自己生成；
//     · **不得悄悄丢值**：`settings.json` 里那个值要是不在那份集合里，界面上必须
//       **显示它**，而不是让 Picker 静默回落到第一项（那会让客户看到一个他从没设过的值）；
//     · **单一事实来源**：`requestBody` 必须由 `Settings` 经
//       [`CoreJSON.requestEncoder`] 编出来，不得手拼七个键（理由见 `requestBody`）。
//
// ⚠️ **`last_code` 不在这里**：它是运行状态，不是用户参数（阶段 A 裁决 #25）。
//    参数面板上多一个客户改不了、也不该改的"字段"，只会让人以为它能改。
//
// ⚠️ 本文件**不要** `import SwiftUI`：这里没有任何 `Color`、没有任何视图类型，
//    所以整份都能进 `Tests/` 被单测。
// ---------------------------------------------------------------------------

/// 参数面板的**可编辑表单**：七个字段 + `-k` 的枚举面。
///
/// 它是一个**值类型**：视图用 `@State` 持一份，用户怎么改都不碰 `AppModel`
/// （"编辑到一半"与"内核手里那一份"是两件事）；按「保存」才走
/// `AppModel.applySettings(_:)`，而落状态的是**内核回执**那一份。
public struct SettingsForm: Equatable, Sendable {

    // MARK: - 七个可编辑字段（顺序与内核 `core/src/settings.rs:32-47` 一致）

    /// `-j`，1–64。
    public var parallel: Int32
    /// `-x`，1–16。
    public var connections: Int32
    /// `-s`，1–16。
    public var splits: Int32
    /// `-k`，`1M`–`100M`。**原值下发**（内核自己 `trim` + 转大写并归一成规范串）。
    public var minSplitSize: String
    /// 限速（MB/s），`0` = 不限速。**注意是 `Int64`**：上限 100 000 MB/s，
    /// 而它换算成字节/秒时还要乘 1024×1024（`core/src/settings.rs` 的 `global_options`）。
    public var limitMbps: Int64
    /// `--max-tries`，1–100。
    public var maxTries: Int32
    /// `--retry-wait`，0–60 秒。
    public var retryWait: Int32

    /// 内核 `hello` 给的 `-k` 取值集合（**壳不生成它**，契约 §2.3）。
    public let choices: [String]

    /// 用内核手里那一份填表，并接上内核给的枚举面。
    public init(settings: Settings, choices: [String]) {
        self.parallel = settings.parallel
        self.connections = settings.connections
        self.splits = settings.splits
        self.minSplitSize = settings.minSplitSize
        self.limitMbps = settings.limitMbps
        self.maxTries = settings.maxTries
        self.retryWait = settings.retryWait
        self.choices = choices
    }

    // MARK: - 回到 `Settings`

    /// 表里这七项。
    ///
    /// ⚠️ 它是**唯一**把七个字段重新组装起来的地方（`requestBody` 与视图的"有没有改动"
    ///    都读它）—— 少了它就会出现第二份"七项分别是哪七个"的清单。
    public var settings: Settings {
        Settings(parallel: parallel,
                 connections: connections,
                 splits: splits,
                 minSplitSize: minSplitSize,
                 limitMbps: limitMbps,
                 maxTries: maxTries,
                 retryWait: retryWait)
    }

    // MARK: - `-k` 的枚举面

    /// `Picker` 里要有哪些项：内核给的集合 **+（必要时）当前值**。
    ///
    /// ⚠️ **追加那一步不是装饰**：落盘的 `settings.json` 是可以被手改的
    ///    （内核允许 1M–100M 里的任何一个整数 MB），而 `hello` 给的是整整 100 项 ——
    ///    两者对不上时（例如客户自己改成 `21M` 而枚举面被截短），
    ///    `Picker` 的 `selection` 落在一个**不在 tag 集合里**的值上，macOS 会把它
    ///    显示成**空白/第一项**：客户看到一个他从没设过的值，而"改回去"这个动作
    ///    反而会真的把它改掉。追加进列表 = 那个值**看得见**。
    ///    顺序保持内核给的顺序（不排序：那份顺序是内核的枚举面，不是壳的排版决定），
    ///    当前值追加在**末尾**（不打断内核那段 1M…100M 的序列）。
    public var minSplitSizeOptions: [String] {
        choices.contains(minSplitSize) ? choices : choices + [minSplitSize]
    }

    /// 当前值不在内核给的集合里时的那句说明；在集合里就是 `nil`。
    ///
    /// ⚠️ 只说**事实**，不承诺内核会接受它：`21M` 在 1–100 之内会被接受，
    ///    而一个手改进 `settings.json` 的 `999M` 会被内核回 `invalid_params`
    ///    —— 谁是合法值由内核判（壳不比内核更松、也不更严），这句话只负责
    ///    让客户知道"这个值不在内核给的清单里"。
    public var minSplitSizeNote: String? {
        choices.contains(minSplitSize)
            ? nil
            : "当前值「\(minSplitSize)」不在内核给出的取值集合里。保存会按这个原值下发，由内核校验。"
    }

    // MARK: - 发给内核的载荷

    /// `set_settings` 的 **`settings` 值**（七个线上键）。
    ///
    /// ⚠️ **必须由 `Settings` 编码而来，不得手拼七个键。**
    ///
    /// `Settings` 的属性名是 camelCase，而内核 `set_settings` 走
    /// `serde_json::from_value::<Settings>`（`core/src/main.rs:1546`）且 `Settings`
    /// 没有任何 `rename`（`core/src/settings.rs:32-47`）—— 键名必须是
    /// `min_split_size` / `limit_mbps` / `max_tries` / `retry_wait`。所以这里走
    /// [`CoreJSON.requestEncoder`]（带 `.convertToSnakeCase`），不是 [`CoreJSON.encoder`]。
    /// 手拼一份 `.object(["min_split_size": …])` 等于把"属性名"和"线格式"各写一遍，
    /// 两处一漂移就是一次 `invalid_params`，而**它看起来完全正常（也是七个键）**；
    /// `Settings` 存在的理由就是当这个的单一事实来源。
    ///
    /// ⚠️ 失败**不回落**：`Settings` 的七个成员全是 `Int32`/`Int64`/`String`，
    ///    `JSONEncoder` 编它们不会失败 —— 真编不出来说明这个类型被改成了编不出来的形状，
    ///    那是编程错误。回落成 `.null` / 空对象的后果是**静默**发出一条少键的请求，
    ///    内核回 `invalid_params`，而壳这边看起来一切正常。
    public var requestBody: JSONValue {
        do {
            let data = try CoreJSON.requestEncoder.encode(settings)
            return try CoreJSON.decoder.decode(JSONValue.self, from: data)
        } catch {
            preconditionFailure("Settings 编码失败（这是编程错误，不是运行时状况）：\(error)")
        }
    }

    /// `set_settings` 的**完整 params**：`{"settings": <七个键>}`
    /// （`core/src/main.rs:1542-1546` 的 `params.get("settings")`）。
    ///
    /// 它把"外面还套一层"这件事写在**同一个地方**，而不是让调用方各自拼一次
    /// （同一个形状拼两遍 = 迟早有一份漏了这一层，而那种请求的形状是
    /// `params.get("settings") == None` ⇒ `invalid_params`）。
    public var setSettingsParams: JSONValue {
        .object(["settings": requestBody])
    }

    // MARK: - 与内核手里那一份的关系

    /// 表里的七项与**内核当前生效的那一份**是否一致（"没有要保存的改动"）。
    ///
    /// 界面上「保存」那颗按钮的禁用判据读它；它也是"保存成功了没有"的**可见证据**：
    /// 存下去之后按钮会自己变灰，不需要另一个转瞬即逝的"已保存"标记。
    /// 恒 `true` 的实现会让按钮永远点不动、恒 `false` 的会让它永远可点，
    /// 而这两种失效都不会有任何报错 —— 所以 `theFormKnowsWhetherItDiffersFromWhatTheKernelHolds`
    /// 逐字段验它。
    public func matches(_ current: Settings?) -> Bool {
        // 内核还没给出那一份（`model.settings == nil`）时**不算一致**：那时候
        // "保存"该是能点的（客户改的就是要发给内核的那份），而不是被悄悄禁用。
        guard let current else { return false }
        return current == settings
    }

    // MARK: - 区间（**不是校验**，是输入边界）

    /// 六个区间，逐条对着 `core/src/settings.rs:68-98` 的 `validate()` 抄下来。
    ///
    /// ⚠️ 它们的用处只有两个，**都不是"壳自己校验"**：
    ///    ① `Stepper` 的输入边界 —— 让客户**按不到**越界值。
    ///       ⚠️ 这**不是**契约要求：契约 §2.3 那句「设置面板的价值是让客户**不可能**
    ///       输错，而不是"输错有提示"」**只针对 `-k`**（它是"写错会让 aria2 直接启动
    ///       失败"的那一项），而 `-k` 是一个 `Picker`（枚举面来自内核的 `hello`，
    ///       见 `minSplitSizeOptions`）。所以 `limit_mbps` 的 `TextField`**故意不夹取值**：
    ///       给它加 clamp 正是 §2.1 明禁的"在下发函数里再抄一遍边界"（第二份边界）。
    ///    ② 让读代码的人看见"这个数为什么只能到这儿"。
    ///    真正的闸门**始终**是内核的 `validate()`（`op_set_settings` 里
    ///    `s.validate().map_err(ErrorBody::invalid_params)`），越界时界面显示的是
    ///    **内核原文**（约束 3）—— 壳不自己判非法、也不自己编校验文案。
    public struct Limits: Equatable, Sendable {
        public let parallel: ClosedRange<Int32>
        public let connections: ClosedRange<Int32>
        public let splits: ClosedRange<Int32>
        public let maxTries: ClosedRange<Int32>
        public let retryWait: ClosedRange<Int32>
        public let limitMbps: ClosedRange<Int64>
    }

    /// ⚠️ `parallel` 的上界是 **64**（不是 16），`retryWait` 是 **0…60**（不是 1…600）：
    ///    这两条都有人按别的软件的习惯写错过，逐字对着内核的 `validate()` 读。
    ///    `limitMbps` 的上界 `100_000` 是 `LIMIT_MBPS_MAX`（为防溢出而设，规格未规定上界）。
    public static let limits = Limits(parallel: 1...64,
                                      connections: 1...16,
                                      splits: 1...16,
                                      maxTries: 1...100,
                                      retryWait: 0...60,
                                      limitMbps: 0...100_000)

    // MARK: - 界面文案里**有语义**的两句

    /// 限速输入框下面那句。
    ///
    /// ⚠️ 必须写出来：`0` 在这里是一个**有专门含义的取值**（不限速），而客户看到
    ///    "限速 0"很自然会读成"限速到 0 = 不让下载"，正好反了。内核侧同一个语义
    ///    （`Settings::global_options` 在不限速时**显式下发 `"0"`**，见
    ///    `core/src/settings.rs` 的那段实测）也说明 `0` 不是"缺省/未设"。
    public static let limitMbpsNote = "0 = 不限速（不限制总下载速度）"

    /// 参数面板底下那句"改动什么时候生效"。
    ///
    /// ⚠️ 七个字段的**生效时机不是同一个**（`core/src/main.rs:1558-1568`）：
    ///    引擎在跑时 `set_settings` 会把 `global_options`（并行数 `-j` 与限速）
    ///    经 `changeGlobalOption` **立即**下发；其余各项是 `per_task_options`，
    ///    在**添加任务**时随 `addUri` 交出去。不说清楚，客户改完 `-s` 会以为
    ///    正在传的那些任务变了（它们没变）。
    public static let applyNote =
        "改动会写盘保存（重开应用仍是这一份）。并行数与限速对已启动的引擎即时生效；"
        + "其余各项在添加新任务时生效，已在传输的任务不受影响。"
}

// ---------------------------------------------------------------------------
// 保存失败 → 界面上那句话
// ---------------------------------------------------------------------------

/// 一次 `set_settings` 失败之后要显示的那句话。
///
/// ⚠️ **这是给视图用的唯一入口，视图自己不做映射**（全局约束 8：视图里不映射 `CoreError`）。
///    映射本身走 `AppModel.message(of:)` —— 那是 `CoreError` → 用户可见文案的**唯一实现**
///    （`DirLoadFailure` / `VerifyRefreshFailure` / `TransferActionFailure` /
///    `EnqueueFeedback` 用的也是它）。本类型**不抄第二份**：抄了就会出现
///    "同一个错误在设置窗口和别处措辞不同"。
///
/// 文案**逐字是内核原文**（约束 3），因为这一路上只有原文说得清：
///   - `invalid_params` 那一支带着内核的**校验消息**（"并行文件数必须在 1–64 之间，当前 99"）
///     —— 客户照着这句话就知道该改成多少；
///   - 还有一支是"设置已保存，但下发到下载引擎失败：…"（`core/src/main.rs:1562-1566`）：
///     **落盘成功了、引擎没吃下去**。这种情况只有内核那句话说得清，壳重写成
///     "保存失败"会把客户引到错误的方向（他会以为参数没存上，再改一遍）。
public enum SettingsSaveFailure {
    public static func message(of error: Error) -> String {
        AppModel.message(of: error)
    }
}
