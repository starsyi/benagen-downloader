//! settings_form —— 参数面板的**呈现模型**：七项参数的标签与区间、`-k` 的枚举面、
//! 发给内核的载荷、以及保存条上那几句话。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/SettingsForm.swift`。
//! 其中**三项**（七项标签、保存条横幅、保存按钮的 help）在上游是**视图层**的
//! （`macos/Sources/BenagenDownloader/Settings/SettingsView.swift:285-291`、
//! `:425`、`:376-380`），上游**没有单测** —— 本模块把它们搬了进来，理由见下面那段。

//! ---------------------------------------------------------------------------
//! 🔴 **从视图层搬进来的三样**（控制者裁决，理由必须写下来）
//! ---------------------------------------------------------------------------
//!
//! 上游的七项**标签**（`SettingsView.swift:285-291`）、保存条那句**横幅**（`:425`）、
//! 保存按钮的两份 **help**（`:376-380`）都住在**视图层**，上游**一条单测都没有**。
//! 本模块仍然把它们搬进来，理由是规格 §3.2 那条纪律的推论：
//!
//!   > JS **不拼接**任何面向用户的字符串 …… 凡是要显示的东西都由 `shell-core::presentation`
//!   > 算好，经命令层序列化交给 JS。**JS 只负责摆位置。**
//!
//! 我们的"视图"是 JS，于是"这句话长什么样"必须有 **Rust 侧的来源**；
//! 留在 JS 里就是同一条文案的**第二个真相源**，而它**没有单测压着**。
//! （`resident_notice` 那两条高度常量是同一条裁决的另一个落点。）
//!
//! ⚠️ 搬进来的只是**字符串与判据**，不是布局：标签配哪个控件、横幅摆哪一行，
//!    仍然是前端的事。
//!
//! ⚠️ **`last_code` 不在这里**：它是运行状态，不是用户参数。参数面板上多一个
//!    客户改不了、也不该改的"字段"，只会让人以为它能改。
//!
//! ⚠️ 本模块**不 import 任何界面类型**：这里没有颜色、没有控件，整份都能被单测。

use crate::presentation::transfer_row::EngineGate;
use crate::protocol::Settings;
use serde::Serialize;
use serde_json::Value;
use std::ops::RangeInclusive;

// ---------------------------------------------------------------------------
// 七项参数的**标签与区间**
// ---------------------------------------------------------------------------

/// 参数面板上的一项：**给前端用的**标签 + 数值区间。
///
/// ⚠️ 它是 `Serialize` 的：前端拿到的是"这一项叫什么、能填多少"，
///    而**不是**一份写死在 JS 里的中文标签表（规格 §3.2）。
///
/// ⚠️ `min`/`max` 是 `Option`，因为**有一项没有数值区间**：`-k`
///    （最小分片大小）是一个**枚举面**，取值集合由内核的 `hello` 给
///    （见 [`SettingsForm::min_split_size_options`]）。给它编一个区间就等于
///    壳自己生成内核的枚举面 —— 那正是"枚举面来自内核"这条判据禁的。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Parameter {
    /// 界面标签（逐字来自上游 `SettingsView.swift`，含全角括号）。
    pub label: &'static str,
    /// 下界；`None` = 这一项不是数值输入（`-k`）。
    pub min: Option<i64>,
    /// 上界；`None` 的含义同上。
    pub max: Option<i64>,
}

/// 六个区间，逐条对着内核 `validate()` 抄下来。
///
/// ⚠️ 它们的用处只有两个，**都不是"壳自己校验"**：
///    ① 让客户**按不到**越界值（输入控件的边界）；
///    ② 让读代码的人看见"这个数为什么只能到这儿"。
///    真正的闸门**始终**是内核的 `validate()`，越界时界面显示的是**内核原文**
///    —— 壳不自己判非法、也不自己编校验文案。
///
/// ⚠️ 上游 `SettingsForm.limits`（`SettingsForm.swift:188-193`）。这两条都有人按别的
///    软件的习惯写错过，逐字对着内核的 `validate()` 读：
///    `parallel` 的上界是 **64**（不是 16），`retry_wait` 是 **0…60**（不是 1…600）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Limits {
    pub parallel: RangeInclusive<i32>,
    pub connections: RangeInclusive<i32>,
    pub splits: RangeInclusive<i32>,
    pub max_tries: RangeInclusive<i32>,
    pub retry_wait: RangeInclusive<i32>,
    /// `limit_mbps` 是 **`i64`**（上限 100 000 MB/s，换算成字节/秒时还要乘 1024×1024）。
    pub limit_mbps: RangeInclusive<i64>,
}

// ---------------------------------------------------------------------------
// 参数面板的可编辑表单
// ---------------------------------------------------------------------------

/// 参数面板的**可编辑表单**：七个字段 + `-k` 的枚举面。
///
/// 上游 `SettingsForm.swift` 的 `SettingsForm`。
///
/// 它是一个**值类型**：调用方持一份，用户怎么改都不碰模型
/// （"编辑到一半"与"内核手里那一份"是两件事）；按「保存」才走
/// `apply_settings`，而落状态的是**内核回执**那一份。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettingsForm {
    /// `-j`，1–64。
    pub parallel: i32,
    /// `-x`，1–16。
    pub connections: i32,
    /// `-s`，1–16。
    pub splits: i32,
    /// `-k`，`1M`–`100M`。**原值下发**（内核自己 trim + 转大写并归一成规范串）。
    pub min_split_size: String,
    /// 限速（MB/s），`0` = 不限速。
    pub limit_mbps: i64,
    /// `--max-tries`，1–100。
    pub max_tries: i32,
    /// `--retry-wait`，0–60 秒。
    pub retry_wait: i32,
    /// 内核 `hello` 给的 `-k` 取值集合（**壳不生成它**）。
    pub choices: Vec<String>,
}

impl SettingsForm {
    /// 用内核手里那一份填表，并接上内核给的枚举面。
    ///
    /// 上游 `SettingsForm.init(settings:choices:)`。
    pub fn new(settings: &Settings, choices: Vec<String>) -> SettingsForm {
        SettingsForm {
            parallel: settings.parallel,
            connections: settings.connections,
            splits: settings.splits,
            min_split_size: settings.min_split_size.clone(),
            limit_mbps: settings.limit_mbps,
            max_tries: settings.max_tries,
            retry_wait: settings.retry_wait,
            choices,
        }
    }

    /// 表里这七项。
    ///
    /// 上游 `SettingsForm.settings`。
    ///
    /// ⚠️ 它是**唯一**把七个字段重新组装起来的地方（`request_body` 与"有没有改动"
    ///    都读它）—— 少了它就会出现第二份"七项分别是哪七个"的清单。
    pub fn settings(&self) -> Settings {
        Settings {
            parallel: self.parallel,
            connections: self.connections,
            splits: self.splits,
            min_split_size: self.min_split_size.clone(),
            limit_mbps: self.limit_mbps,
            max_tries: self.max_tries,
            retry_wait: self.retry_wait,
        }
    }

    // MARK: - `-k` 的枚举面

    /// 枚举控件里要有哪些项：内核给的集合 **+（必要时）当前值**。
    ///
    /// 上游 `SettingsForm.minSplitSizeOptions`。
    ///
    /// ⚠️ **追加那一步不是装饰**：落盘的 `settings.json` 是可以被手改的
    ///    （内核允许 1M–100M 里的任何一个整数 MB），而 `hello` 给的是整整 100 项 ——
    ///    两者对不上时（例如客户自己改成 `21M` 而枚举面被截短），
    ///    控件的选中项落在一个**不在候选集合里**的值上，界面会把它显示成
    ///    **空白/第一项**：客户看到一个他从没设过的值，而"改回去"这个动作
    ///    反而会真的把它改掉。追加进列表 = 那个值**看得见**。
    ///    顺序保持内核给的顺序（不排序：那份顺序是内核的枚举面，不是壳的排版决定），
    ///    当前值追加在**末尾**（不打断内核那段 1M…100M 的序列）。
    pub fn min_split_size_options(&self) -> Vec<String> {
        if self.choices.iter().any(|c| c == &self.min_split_size) {
            self.choices.clone()
        } else {
            let mut options = self.choices.clone();
            options.push(self.min_split_size.clone());
            options
        }
    }

    /// 当前值不在内核给的集合里时的那句说明；在集合里就是 `None`。
    ///
    /// 上游 `SettingsForm.minSplitSizeNote`。
    ///
    /// ⚠️ 只说**事实**，不承诺内核会接受它：`21M` 在 1–100 之内会被接受，
    ///    而一个手改进 `settings.json` 的 `999M` 会被内核回 `invalid_params`
    ///    —— 谁是合法值由内核判（壳不比内核更松、也不更严），这句话只负责
    ///    让客户知道"这个值不在内核给的清单里"。
    pub fn min_split_size_note(&self) -> Option<String> {
        if self.choices.iter().any(|c| c == &self.min_split_size) {
            None
        } else {
            Some(format!(
                "当前值「{}」不在内核给出的取值集合里。保存会按这个原值下发，由内核校验。",
                self.min_split_size
            ))
        }
    }

    // MARK: - 发给内核的载荷

    /// `set_settings` 的 **`settings` 值**（七个线上键）。
    ///
    /// 上游 `SettingsForm.requestBody`。
    ///
    /// ⚠️ **必须由 [`Settings`] 编码而来，不得手拼七个键。**
    ///
    /// [`Settings`] 的字段名就是线上键名（`min_split_size` / `limit_mbps` /
    /// `max_tries` / `retry_wait`，见 `protocol.rs` 那个类型的注释），所以这里
    /// **直接用 `serde_json` 把类型编出去**。手拼一份
    /// `.object(["min_split_size": …])` 等于把"字段名"和"线格式"各写一遍，
    /// 两处一漂移就是一次 `invalid_params`，而**它看起来完全正常（也是七个键）**；
    /// [`Settings`] 存在的理由就是当这个的单一事实来源。
    ///
    /// ⚠️ **失败不回落、也不 panic：回 `Err`，由调用方大声报出去。**
    ///    回落成 `.null` / 空对象的后果是**静默**发出一声少键的请求，内核回
    ///    `invalid_params`，而壳这边看起来一切正常（本项目最恨的形状）。
    ///    而 **panic 更不行**：它会挂住前端那条 `invoke`（永远等不到回话）——
    ///    见 [`Self::encode_failure_text`] 上面那段"为什么不许 `unreachable!`"。
    ///
    /// ---------------------------------------------------------------------
    /// 🔴 **本波次的修订（R-38 的口径）：它从方法改成了关联函数。**
    /// ---------------------------------------------------------------------
    /// 上游那两样（`requestBody` / `setSettingsParams`）是**表单的**成员，因为
    /// 那边的"表单"是视图里真有的一个对象。**我们这边没有**：表单活在 JS 里
    /// （`web/js/settings.js` 的 `edited` 一格），壳这一侧真正的发货路径
    /// （`shell-win/src/kernel.rs::set_settings`）手里**只有内核给的那一份 `Settings`**。
    /// 于是这两个方法**零生产调用者**，而 `kernel.rs` 自己 `json!` 拼了一次同一个形状
    /// —— 正是账本 R-38 裁掉 `History::put` 的那个形状（"两个都活着、各测各的"）。
    /// 收 `&Settings` 之后，那条路径调的就是**这一处**信封，不再是第二份实现。
    pub fn request_body(settings: &Settings) -> Result<Value, serde_json::Error> {
        serde_json::to_value(settings)
    }

    /// `set_settings` 的**完整 params**：`{"settings": <七个键>}`。
    ///
    /// 上游 `SettingsForm.setSettingsParams`。
    ///
    /// 它把"外面还套一层"这件事写在**同一个地方**，而不是让调用方各自拼一次
    /// （同一个形状拼两遍 = 迟早有一份漏了这一层，而那种请求的形状是
    /// `params.get("settings") == None` ⇒ `invalid_params`）。
    ///
    /// ⚠️ 调用方是 `shell-win/src/kernel.rs::set_settings`（**生产路径**）——
    ///    这一条是有调用者的，见上面 [`Self::request_body`] 那段修订说明。
    ///    ⚠️ 它回的是 `Result`：**调用方必须把 `Err` 报出去**（`kernel.rs` 用
    ///    [`Self::encode_failure_text`] 换成人话），**不许**回落成 `.null`。
    pub fn set_settings_params(settings: &Settings) -> Result<Value, serde_json::Error> {
        Self::set_settings_params_with(settings, Self::request_body)
    }

    /// [`Self::set_settings_params`] 的**本体** —— **编码那一步是参数**。
    ///
    /// ---------------------------------------------------------------------
    /// 🔴 **为什么把编码做成一步可替换的东西**（这不是为了好看）
    /// ---------------------------------------------------------------------
    /// `Settings` 的七个成员全是 `i32`/`i64`/`String` ⇒ **它今天编不出来是不可能的**：
    /// 那条 `Err` 在真实的 `Settings` 上**走不到**。
    ///
    /// 而 **"这一支不可达"恰恰是错的时候最贵的那一类断言**：它不适用时的后果不是
    /// 一句错话，是**前端那条 `invoke` 永远不 resolve**（界面静静地挂在那里）。
    /// 本仓库从头到尾在防的正是这个形状（`test.sh` 第 3 步"空跑绿灯判为失败"、
    /// 各处"一个不被任何东西守着的断言等于没有"都是同一条）。
    ///
    /// ⇒ 判据不能停在"我看它是不可达的"，也**不许**把它写成 `unreachable!` 一了百了：
    ///   那样即使测试压住了"这件事会在测试里红"，**代码仍然带着一个"万一到了这里
    ///   就挂住"的分支进交付物**。所以这里把编码做成**一步可替换的东西**，
    ///   用例拿一个注定失败的编码器走**同一条链**（这个 `?`、下面那层 `json!` 都在），
    ///   断言回的是**一句可读的话**、不是一个 panic。
    ///
    /// ⚠️ 生产路径传进来的**永远**是 [`Self::request_body`]（`serde_json::to_value`）；
    ///    这个参数不是为了"将来换实现"，是为了**让那条路可测**。
    fn set_settings_params_with<E>(settings: &Settings, encode: E) -> Result<Value, serde_json::Error>
    where
        E: FnOnce(&Settings) -> Result<Value, serde_json::Error>,
    {
        Ok(serde_json::json!({ "settings": encode(settings)? }))
    }

    /// 七项参数**编码失败**时，调用方要回给用户的那句话。
    ///
    /// ⚠️ **它是"编码失败"这件事唯一的落点**：`shell-win/src/kernel.rs::set_settings`
    ///    用它把 `Err` 变成 `CallFailure::shell(...)`（与 `task_action` 那条**同款措辞**：
    ///    同样点名"壳自己的序列化失败"、同样给出"把这条原样发给我们"的补救）。
    ///    ⚠️ 它单列成一个**具名函数**而不是就地 `format!` 在那条路径上，
    ///    就是为了让"回的是哪句话"有一条**能走到的**判据 —— 见下面那条用例。
    ///
    /// ⚠️ **原文逐字**：这句话从 `kernel.rs` 搬过来时**一个字都没改**
    ///    （搬的理由正是上面那段"让那条路可测"）。
    pub fn encode_failure_text(error: &serde_json::Error) -> String {
        format!(
            "这七项参数没能编码成内核认识的形状（{error}）：这是壳自己的序列化失败。\
             补救：把这条原样发给我们。"
        )
    }

    // MARK: - 与内核手里那一份的关系

    /// 表里的七项与**内核当前生效的那一份**是否一致（"没有要保存的改动"）。
    ///
    /// 上游 `SettingsForm.matches(_:)`。
    ///
    /// ---------------------------------------------------------------------
    /// 🔴 **对位记账（R-5）：这个判决在**生产路径上零调用者**，判决活在前端**
    /// ---------------------------------------------------------------------
    /// ⚠️ 这一格与 `NoteDrafts` 那几格**不同**：它那两句**文案**确实发出去了
    ///    （`api/settings.rs` 的 `banner.{unsaved,clean}` / `save_help.*`），
    ///    **没发出去的是这句判决** —— "此刻算不算有未保存的改动"由前端自己比
    ///    （`web/js/settings.js` 的 `hasUnsavedChanges()`），而 `matches` **没人调**。
    ///    今天的两个实现在语义上等价（都是逐字段比），所以**没有**可观察的差异；
    ///    危险的是**下一次**：有人改了一边的口径，另一边**照旧全绿**。
    /// ⇒ 对照判据：用例表由 Rust 倒出来（`dump_wire_fixtures.rs` 的
    ///    `mirrorCases.settingsMatches`，`dirty` **现算**），前端把七格填成表里那一份、
    ///    再读横幅是哪一句，两边必须一致；坐标与"仍然零调用者"那句由
    ///    `windows/scripts/check_presentation_mirrors.sh` 守。
    ///
    /// 界面上「保存」那颗按钮的禁用判据读它；它也是"保存成功了没有"的**可见证据**：
    /// 存下去之后按钮会自己变灰，不需要另一个转瞬即逝的"已保存"标记。
    /// 恒 `true` 的实现会让按钮永远点不动、恒 `false` 的会让它永远可点，
    /// 而这两种失效都不会有任何报错 —— 所以那条测试逐字段验它。
    pub fn matches(&self, current: Option<&Settings>) -> bool {
        // 内核还没给出那一份时**不算一致**：那时候"保存"该是能点的，
        // 而不是被悄悄禁用。
        match current {
            Some(current) => *current == self.settings(),
            None => false,
        }
    }

    // MARK: - 区间与标签

    /// 六项的区间（唯一一份真相；`-k` 不在其中，它的面来自内核的 `hello`）。
    ///
    /// 上游 `SettingsForm.limits`。
    pub const LIMITS: Limits = Limits {
        parallel: 1..=64,
        connections: 1..=16,
        splits: 1..=16,
        max_tries: 1..=100,
        retry_wait: 0..=60,
        limit_mbps: 0..=100_000,
    };

    /// 七项的**标签**（顺序与上游 `SettingsView.swift` 的七行控件一致）。
    ///
    /// 🔴 这一份来自**视图层**（上游没有单测），本波次把它搬进来的理由见模块头。
    ///    ⚠️ 它与 [`Self::LIMITS`] 是**两份独立的事实**（一个是名字、一个是数），
    ///    下面的 [`Self::parameters`] 把它们配起来 —— 那一步是"配对"，不是抄数。
    pub const PARAMETER_LABELS: [&'static str; 7] = [
        "并行文件数（-j）",
        "单文件连接数（-x）",
        "分片数（-s）",
        "最小分片大小（-k）",
        "限速（MB/s）",
        "重试次数",
        "重试间隔（秒）",
    ];

    /// 七项 → **给前端用的**（标签 + 区间），顺序即面板上从上到下的顺序。
    ///
    /// 🔴 新增（上游的标签在视图层、区间在 `SettingsForm.limits`，没有任何一处把
    ///    它们配起来；而前端需要的就是这个配对）。判据一条都不多：
    ///    标签来自 [`Self::PARAMETER_LABELS`]，区间来自 [`Self::LIMITS`]。
    pub fn parameters() -> Vec<Parameter> {
        let limits = Self::LIMITS;
        let numeric = |range: &RangeInclusive<i64>| {
            (Some(*range.start()), Some(*range.end()))
        };
        let bounds: [(Option<i64>, Option<i64>); 7] = [
            numeric(&(*limits.parallel.start() as i64..=*limits.parallel.end() as i64)),
            numeric(&(*limits.connections.start() as i64..=*limits.connections.end() as i64)),
            numeric(&(*limits.splits.start() as i64..=*limits.splits.end() as i64)),
            // `-k`：**没有数值区间**（取值面来自内核的 hello）——见 [`Parameter`] 那条。
            (None, None),
            numeric(&limits.limit_mbps),
            numeric(&(*limits.max_tries.start() as i64..=*limits.max_tries.end() as i64)),
            numeric(&(*limits.retry_wait.start() as i64..=*limits.retry_wait.end() as i64)),
        ];
        Self::PARAMETER_LABELS
            .iter()
            .zip(bounds)
            .map(|(label, (min, max))| Parameter { label, min, max })
            .collect()
    }

    // MARK: - 界面文案里**有语义**的那几句

    /// 限速输入框下面那句。
    ///
    /// 上游 `SettingsForm.limitMbpsNote`。
    ///
    /// ⚠️ 必须写出来：`0` 在这里是一个**有专门含义的取值**（不限速），而客户看到
    ///    "限速 0"很自然会读成"限速到 0 = 不让下载"，正好反了。内核侧同一个语义
    ///    （不限速时**显式下发 `"0"`**）也说明 `0` 不是"缺省/未设"。
    pub const LIMIT_MBPS_NOTE: &'static str = "0 = 不限速（不限制总下载速度）";

    /// 参数面板底下那句"改动什么时候生效"。
    ///
    /// 上游 `SettingsForm.applyNote`。
    ///
    /// ⚠️ 七个字段的**生效时机不是同一个**：引擎在跑时 `set_settings` 会把
    ///    `global_options`（并行数 `-j` 与限速）经 `changeGlobalOption` **立即**下发；
    ///    其余各项是逐任务选项，在**添加任务**时随 `addUri` 交出去。
    ///    不说清楚，客户改完 `-s` 会以为正在传的那些任务变了（它们没变）。
    pub const APPLY_NOTE: &'static str =
        "改动会写盘保存（重开应用仍是这一份）。并行数与限速对已启动的引擎即时生效；\
其余各项在添加新任务时生效，已在传输的任务不受影响。";

    /// 保存条上那句横幅：有改动 / 与内核一致。
    ///
    /// 🔴 这一句来自**视图层**（上游 `SettingsView.swift:425`，没有单测），
    ///    本波次把它搬进来的理由见模块头。判据在**调用方**手里
    ///    （`!form.matches(model.settings)`），这里只回答"两种状态各说什么"。
    ///
    /// ⚠️ **对位记账（R-5）**：两句**文案**走的是 `api/settings.rs` 的 `banner.*`
    ///    （发得出去、前端只按判决挑一句），零生产调用者的是
    ///    [`Self::dirty_banner`] 这个**挑选函数**（挑选现在活在前端）。它的落点由
    ///    [`Self::matches`] 那条差分覆盖：判决一致 ⇒ 挑出来的句子自然一致。
    ///    `check_presentation_mirrors.sh` 里那一格账把它记成"承接点是载荷"。
    pub const UNSAVED_BANNER: &'static str = "有未保存的改动";
    /// 与内核一致那一句。见 [`Self::UNSAVED_BANNER`]。
    pub const CLEAN_BANNER: &'static str = "与内核当前参数一致";

    /// 横幅文字（`has_unsaved_changes` 由 [`Self::matches`] 算出来）。
    pub fn dirty_banner(has_unsaved_changes: bool) -> &'static str {
        if has_unsaved_changes {
            Self::UNSAVED_BANNER
        } else {
            Self::CLEAN_BANNER
        }
    }

    /// 保存按钮的 help 文字（三态）。
    ///
    /// 🔴 来自**视图层**（上游 `SettingsView.swift:376-380` 的 `saveHelp`）。
    ///
    /// ⚠️ "引擎不可用"那一支**不重抄**：它直接返回既有的
    ///    [`EngineGate::UNAVAILABLE_HELP`]（那条提示指向那处真的存在的动作：
    ///    点顶部的「重试」）。抄一份的结局是"同一个状态在两处说不同的话"。
    pub fn save_help(engine_allows_actions: bool, has_unsaved_changes: bool) -> &'static str {
        if !engine_allows_actions {
            return EngineGate::UNAVAILABLE_HELP;
        }
        if has_unsaved_changes {
            "把七项参数下发给内核（set_settings）"
        } else {
            "与内核当前参数一致：没有要保存的改动"
        }
    }
}

// ---------------------------------------------------------------------------
// 保存失败 → 界面上那句话：**这一格本波次删掉了**（R-38 的口径）
// ---------------------------------------------------------------------------
//
// 上游 `SettingsForm.swift` 末尾有一个 `SettingsSaveFailure.message(of:)`：它把
// `CoreError` 转成设置窗口上那句话。**我们这边删掉了它**，理由是一条**形状**：
//
//   · 它的实现**逐字**就是 `error_text(error)` —— 而那已经是 `ClientError` →
//     用户可见文案的**唯一实现**（`presentation/error_text.rs`）；
//   · 而生产路径上"设置保存失败"那句话**根本不经过 `ClientError`**：命令层拿到的是
//     `CallFailure`，走 `commands.rs` 的 `note_failure` + `api::failure`
//     （最终落到 `CallFailure::text()`）。**换一个类型转发一次，接不上。**
//   · ⇒ 它**零生产调用者**，却带着三条用例全绿 —— 正是账本 R-38 裁掉 `History::put`
//     的那个形状："一个有测试、没有生产调用者的函数，测试全绿而线上一次都没跑过"。
//     **不许留成"两个都活着、各测各的"。**
//
// ⚠️ **删掉它没有丢掉任何判据**：那三条断言的对象是 `error_text` 的输出
//    （内核原文逐字 / 内核给空串时不编占位 / 非 `Kernel` 变体走 `client.rs` 的 `Display`，
//    含「内核进程已退出（管道结束）」那句字面量）—— 它们的落点在 `error_text.rs`
//    与 `transfer_row.rs` 的用例里，**判据一条都没少**，少的是一个"转发一次"的死入口。
//
// ⚠️ **约束不变**：设置窗口那句失败文案必须由 `error_text` 给，命令层不许自己写一句
//    （规格 §10 的"唯一映射"）。它的落点今天是 `commands.rs::settings_set`
//    （`Err` ⇒ `note_failure` + `api::failure`），**那是唯一实现** —— 删掉这一格之后
//    反而更清楚：那条路径上**没有第二个名字**可以走偏。

// ---------------------------------------------------------------------------
// 测试（先写测试：它们会先红，见任务 6 的步骤 2）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! 上游 `macos/Tests/BenagenCoreKitTests/SettingsFormTests.swift`（逐条对位，
    //! 每条测试上方点名它对应上游的哪一条）+ 三条本波次新增的（上游在视图层、没有单测，
    //! 见模块头那段裁决）。

    use super::SettingsForm;
    use crate::client::ClientError;
    use crate::presentation::error_text::error_text;
    use crate::protocol::Settings;

    /// 上游 `SettingsFormTests.swift` 末尾的 `Settings.fixture()`：
    /// 默认值取内核的 `default_settings()`（`core/src/settings.rs:51-61`）。
    ///
    /// ⚠️ 有判别力的断言**不要**拿这些默认值当期望值。
    fn fixture(
        parallel: i32,
        connections: i32,
        splits: i32,
        min_split_size: &str,
        limit_mbps: i64,
        max_tries: i32,
        retry_wait: i32,
    ) -> Settings {
        Settings {
            parallel,
            connections,
            splits,
            min_split_size: min_split_size.to_string(),
            limit_mbps,
            max_tries,
            retry_wait,
        }
    }

    fn default_settings() -> Settings {
        fixture(8, 16, 16, "20M", 0, 3, 1)
    }

    // -----------------------------------------------------------------------
    // `-k` 的枚举面
    // -----------------------------------------------------------------------

    /// 上游 `minSplitSizeUsesTheChoiceListFromHello`。
    #[test]
    fn min_split_size_uses_the_choice_list_from_hello() {
        // `-k` 必须是枚举控件，取值集合由内核的 hello 给出（100 项，1M…100M）。
        let f = SettingsForm::new(&default_settings(), vec!["1M".into(), "20M".into(), "100M".into()]);
        assert_eq!(f.min_split_size_options(), vec!["1M", "20M", "100M"]);
    }

    /// 上游 `aValueOutsideTheChoiceListIsFlaggedNotSilentlyDropped`。
    #[test]
    fn a_value_outside_the_choice_list_is_flagged_not_silently_dropped() {
        // 落盘的 settings.json 可能被手改成 21M（内核允许 1…100），而 hello 给了 100 项时
        // 不会发生；但若两者对不上，界面必须显示当前值而不是悄悄回落到第一项。
        let f = SettingsForm::new(
            &fixture(8, 16, 16, "21M", 0, 3, 1),
            vec!["1M".into(), "20M".into()],
        );
        assert!(f.min_split_size_options().iter().any(|o| o == "21M"));
    }

    /// 上游 `theChoiceListIsTheKernelsOwnOrderNotARebuiltOne`。
    ///
    /// ⚠️ 这条盯的是"枚举面来自内核"这件事**本身**：上面第一条只证明"传进去什么就得到什么"
    ///    的**一个实例**，一个把 1M…100M 写死的实现也可能恰好通过它。这里换一份
    ///    **乱序、且不含默认值**的 choices，任何排序/去重/自行生成的实现都会当场露馅。
    #[test]
    fn the_choice_list_is_the_kernels_own_order_not_a_rebuilt_one() {
        let f = SettingsForm::new(
            &fixture(8, 16, 16, "3M", 0, 3, 1),
            vec!["3M".into(), "1M".into(), "2M".into()],
        );
        assert_eq!(
            f.min_split_size_options(),
            vec!["3M", "1M", "2M"],
            "顺序照内核给的，不排序、不去重"
        );
        assert_eq!(f.min_split_size_note(), None, "在集合里就没什么可说的");
    }

    /// 上游 `anOffListValueIsAppendedExactlyOnceAndExplained`。
    #[test]
    fn an_off_list_value_is_appended_exactly_once_and_explained() {
        // "Flagged" 的另一半：**界面要能说出这件事**（只是"列表里有它"还不够 ——
        // 客户会以为 21M 是内核给的选项之一）。说明里必须带上这个值本身。
        let f = SettingsForm::new(
            &fixture(8, 16, 16, "21M", 0, 3, 1),
            vec!["1M".into(), "20M".into()],
        );
        assert_eq!(
            f.min_split_size_options(),
            vec!["1M", "20M", "21M"],
            "不在集合里就追加在末尾，只追加一次"
        );
        let note = f.min_split_size_note();
        assert!(
            note.as_deref().unwrap_or("").contains("21M"),
            "说明里要点名这个值：{}",
            note.unwrap_or_default()
        );
        assert!(!note.as_deref().unwrap_or("").is_empty(), "不得是一句空话");
    }

    // -----------------------------------------------------------------------
    // 七个字段：可编辑、一个不少、按线上键名出去
    // -----------------------------------------------------------------------

    /// 上游 `everyFieldIsEditableAndSerializedBack`。
    #[test]
    fn every_field_is_editable_and_serialized_back() {
        // 七个字段一个不少。⚠️ 七个新值**两两不同、且都不同于夹具的默认值**
        //    （3 / 4 / 5 / "100M" / 100_000 / 7 / 60 vs 8 / 16 / 16 / "20M" / 0 / 3 / 1）。
        //    `limit_mbps` 特意给 100_000（`i64` 的量级），任何半路被塞进 i32 的实现
        //    都会溢出成另一个数。
        let mut f = SettingsForm::new(&default_settings(), vec![]);
        f.parallel = 3;
        f.connections = 4;
        f.splits = 5;
        f.min_split_size = "100M".to_string();
        f.limit_mbps = 100_000;
        f.max_tries = 7;
        f.retry_wait = 60;

        assert_eq!(f.settings(), fixture(3, 4, 5, "100M", 100_000, 7, 60));

        // 值也要真的落到 request_body 上（`settings()` 对了但 body 走的是另一份拷贝，
        // 是另一个 bug）。
        let body = SettingsForm::request_body(&f.settings()).expect("Settings 必须编得出来");
        let obj = body.as_object().expect("request_body 必须是一个 JSON 对象");
        assert_eq!(obj["parallel"], serde_json::json!(3));
        assert_eq!(obj["connections"], serde_json::json!(4));
        assert_eq!(obj["splits"], serde_json::json!(5));
        assert_eq!(obj["min_split_size"], serde_json::json!("100M"));
        assert_eq!(obj["limit_mbps"], serde_json::json!(100_000));
        assert_eq!(obj["max_tries"], serde_json::json!(7));
        assert_eq!(obj["retry_wait"], serde_json::json!(60));
    }

    /// 上游 `eachFieldLandsInItsOwnCellOnTheWayIn`。
    ///
    /// ⚠️ **进站方向**（内核手里那一份 → 表单）。`every_field_is_editable_and_serialized_back`
    ///    守的只是**出站**方向，两者不会互相掩护：把 `connections` 与 `splits` 填串，
    ///    出站那条照样全绿（出站用的是同一份填错的值）。
    #[test]
    fn each_field_lands_in_its_own_cell_on_the_way_in() {
        let f = SettingsForm::new(&fixture(8, 4, 5, "20M", 0, 3, 1), vec![]);
        assert_eq!(f.connections, 4, "-x 这一格拿的必须是内核的 connections");
        assert_eq!(f.splits, 5, "-s 这一格拿的必须是内核的 splits");
    }

    /// 上游 `setSettingsAlwaysSendsAllSevenKeys`。
    ///
    /// ⚠️ 内核的 `set_settings` 没有 serde default：少一个键整个请求就 invalid_params。
    /// ⚠️ 断的是**键名**，不是键的个数 —— 字段名逐字是 snake_case，而写成 camelCase
    ///    同样是 7 个键，只是 `min_split_size`/`limit_mbps` 变成了
    ///    `minSplitSize`/`limitMbps`，内核把它们当成缺失键、回 invalid_params。
    ///    所以"个数 == 7"这条对"名字对不对"零判别力，这里逐个点名。
    #[test]
    fn set_settings_always_sends_all_seven_keys() {
        let body = SettingsForm::request_body(&default_settings()).expect("Settings 必须编得出来");
        let obj = body.as_object().expect("request_body 必须是一个 JSON 对象");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "parallel",
                "connections",
                "splits",
                "min_split_size",
                "limit_mbps",
                "max_tries",
                "retry_wait",
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<&str>>()
        );
    }

    /// 上游 `setSettingsParamsWrapsTheBodyUnderTheSettingsKey`。
    #[test]
    fn set_settings_params_wraps_the_body_under_the_settings_key() {
        // `set_settings` 的 params 形状是 `{"settings": <七项>}`。
        let f = SettingsForm::new(&fixture(8, 16, 16, "21M", 0, 3, 1), vec![]);
        // ⚠️ 两处 `expect` **不是放宽断言**：`Settings` 的七个成员全是 `i32`/`i64`/`String`，
        //    这里编不出来只可能是类型被改坏了 —— 那时这条用例**当场红**（红在 expect 上，
        //    而那正是我们要的：**响亮**）。`Err` 那条路本身另有一条用例（见
        //    `an_encoding_failure_comes_back_as_a_readable_sentence_not_a_panic`）。
        let params = SettingsForm::set_settings_params(&f.settings()).expect("Settings 必须编得出来");
        let top = params.as_object().expect("params 必须是对象");
        assert_eq!(top.keys().collect::<Vec<_>>(), vec!["settings"]);
        let inner = top["settings"].as_object().expect("内层必须是对象");
        assert_eq!(
            inner,
            SettingsForm::request_body(&f.settings())
                .expect("Settings 必须编得出来")
                .as_object()
                .expect("request_body 必须是对象"),
            "内层那一份就是 request_body 本身（不是各编一次的两份）"
        );
        assert_eq!(inner.len(), 7);
    }

    /// 🔴 **新增（R-38 复审）：编码失败回的是那句可读的话，不是一个 panic。**
    ///
    /// 为什么必须有这一条：这条 `Err` 在**真实的 `Settings` 上走不到**
    /// （七个成员全是 `i32`/`i64`/`String`）。而"这一支不可达"这种断言
    /// **恰恰是错的时候最贵的那一类** —— 它不适用时的后果不是一句错话，
    /// 是**前端那条 `invoke` 永远不 resolve**（界面静静地挂住）。
    /// 本仓库从头到尾在防这个形状。
    ///
    /// ⇒ 判据不能停在"我看它是不可达的"，也**不许**把它写成 `unreachable!`
    ///    一了百了（那样即使测试压住了，代码仍然带着一个"万一到了这里就挂住"的
    ///    分支进交付物）。这里用一个**注定失败**的编码器走
    ///    [`SettingsForm::set_settings_params`] 的**同一条链**（那个 `?`、那层
    ///    `json!` 都在），断言两件事：
    ///      ① 它回 **`Err`**（不是 panic、也不是回落成 `.null` / 空对象 ——
    ///         后者会静默发一条少键请求，内核回 `invalid_params` 而壳看起来正常）；
    ///      ② 那句 `Err` 换成人话之后**点名了是哪一步失败、责任在谁、怎么补救**，
    ///         并且**带上底层那句话**（客户报障时我们要看到根因）。
    ///
    /// 判别力：把 `set_settings_params` 换回 `unreachable!` / `.unwrap()` ⇒ 这一条
    /// **当场 panic**（红）；把 `encode_failure_text` 改写成"保存失败，请重试"那种
    /// 空话 ⇒ 后三条断言红。
    #[test]
    fn an_encoding_failure_comes_back_as_a_readable_sentence_not_a_panic() {
        /// 一个**注定失败**的编码器：真实的 `Settings` 编不出来是不可能的，
        /// 所以那条路只能这样走到（理由见 `set_settings_params_with` 那段）。
        fn always_fails(_: &Settings) -> Result<serde_json::Value, serde_json::Error> {
            serde_json::from_str::<serde_json::Value>("这不是一个 JSON 对象")
        }
        let error = SettingsForm::set_settings_params_with(
            &fixture(8, 16, 16, "21M", 0, 3, 1),
            always_fails,
        )
        .expect_err("编码失败必须回 Err —— 既不许 panic，也不许回落成一份少键的载荷");

        // 调用方（`kernel.rs::set_settings`）拿它换回来的那句话：
        let text = SettingsForm::encode_failure_text(&error);
        assert!(!text.trim().is_empty(), "失败原文不许是空的（静默降级）");
        assert!(
            text.contains("没能编码成内核认识的形状"),
            "要点名失败在哪一步：{text}"
        );
        assert!(
            text.contains("壳自己的序列化失败"),
            "要说清这是壳自己的问题，客户报障才不会找错方向：{text}"
        );
        assert!(
            text.contains("补救：把这条原样发给我们。"),
            "补救必须真的走得通（W-2 的口径）：{text}"
        );
        assert!(
            text.contains(&error.to_string()),
            "底层那句话要**原样**带上（少了它，客户报回来的东西定位不到根因）：{text}"
        );
    }

    /// 上游 `theBodyIsDerivedFromTheSettingsTypeNotASecondCopyOfTheWireFormat`。
    ///
    /// ⚠️ 手拼七个键的实现在上面几条上**也可能通过**（今天两者恰好一致），所以这条盯的是
    ///    **单一事实来源**：`request_body` 必须与"拿 `Settings` 现编一遍"逐字相同。
    ///    右边是从**类型**重新编出来的，左边若是手抄的副本，任何一次字段增删/改名都会让
    ///    两者分叉 —— 而那时内核只会回一条 invalid_params。
    #[test]
    fn the_body_is_derived_from_the_settings_type_not_a_second_copy_of_the_wire_format() {
        // ⚠️ **判据的对象是字面量**（修复轮 1 改的；右边**不再**是"从类型现编一份"）：
        //    `request_body` 的实现**就是** `serde_json::to_value(self.settings())`，
        //    拿它当断言对象等于 `f(x) == f(x)` —— 恒真（审查 I-1 的同一条口径）。
        //    ⇒ 函数名里那句"derived from the settings type"**现在只描述实现意图，
        //    不再是这条断言的内容**（名字保留是为了对位上游判据名）；这条断言实际钉的是
        //    **线上形状逐字正确**：七个键名、七个值、不多不少。
        //    它能杀掉：手拼的键里任何一个**名字写错**、**值串了格**、多键少键。
        //    ⚠️ 它**证明不了**"由类型派生"（那件事今天只能靠代码审查；上游自己也承认
        //    "手抄的副本今天也相等，挡手拼的是约束本身"）。
        //    ⚠️ 夹具取**两两不同**的值：16/16 与 0/0 那种撞车会让"两个字段串了"活下来。
        let f = SettingsForm::new(&fixture(9, 11, 13, "21M", 12_345, 7, 3), vec![]);
        assert_eq!(
            SettingsForm::request_body(&f.settings()).expect("Settings 必须编得出来"),
            serde_json::json!({
                "parallel": 9,
                "connections": 11,
                "splits": 13,
                "min_split_size": "21M",
                "limit_mbps": 12_345,
                "max_tries": 7,
                "retry_wait": 3,
            })
        );
    }

    /// 上游 `rangesMatchTheCoreLimits`。
    #[test]
    fn ranges_match_the_core_limits() {
        let limits = SettingsForm::LIMITS;
        assert_eq!(limits.parallel, 1..=64); // ⚠️ 不是 1...16
        assert_eq!(limits.connections, 1..=16);
        assert_eq!(limits.splits, 1..=16);
        assert_eq!(limits.max_tries, 1..=100);
        assert_eq!(limits.retry_wait, 0..=60); // ⚠️ 不是 1...600，且下界是 0
        assert_eq!(limits.limit_mbps, 0..=100_000); // LIMIT_MBPS_MAX = 100_000
    }

    // -----------------------------------------------------------------------
    // 表单与内核手里那一份的关系（"有没有要保存的改动"）
    // -----------------------------------------------------------------------

    /// 上游 `theFormKnowsWhetherItDiffersFromWhatTheKernelHolds`。
    #[test]
    fn the_form_knows_whether_it_differs_from_what_the_kernel_holds() {
        // 界面上"保存"那颗按钮的禁用判据读它。⚠️ 判据必须**逐字段**：
        //    一个恒返回 true（或恒 false）的实现会让按钮永远可点 / 永远点不动，
        //    而这两种失效都不会有任何报错。
        let f = SettingsForm::new(&default_settings(), vec![]);
        assert!(f.matches(Some(&default_settings())));
        // 只改一项（不是整份都换）也要看得出来
        assert!(!f.matches(Some(&fixture(8, 16, 16, "20M", 0, 3, 60))));
        assert!(!f.matches(Some(&fixture(8, 16, 16, "100M", 0, 3, 1))));
        assert!(!f.matches(Some(&fixture(8, 16, 16, "20M", 5, 3, 1))));
        // 内核还没给出那一份时也**不算一致**（`None` ≠ 一致）：界面据此才肯让客户保存。
        assert!(!f.matches(None));
    }

    // -----------------------------------------------------------------------
    // 界面文案里**有语义**的那两句（内核语义，不是排版）
    // -----------------------------------------------------------------------

    /// 上游 `zeroMeansUnlimitedAndTheScreenSaysSo`。
    #[test]
    fn zero_means_unlimited_and_the_screen_says_so() {
        // `0` = 不限速，**界面要写明这一条**（客户看到"限速 0"很自然会读成
        // "限速到 0 = 不让下载"，正好反了）。
        assert!(SettingsForm::LIMIT_MBPS_NOTE.contains('0'));
        assert!(SettingsForm::LIMIT_MBPS_NOTE.contains("不限速"));
    }

    /// 上游 `theScreenSaysWhenEachSettingApplies`。
    #[test]
    fn the_screen_says_when_each_setting_applies() {
        // 内核的两条生效路径不同：并行数与限速走 `changeGlobalOption`
        // **对已在跑的引擎即时生效**；其余各项是"添加任务时"下发的逐任务选项。
        // 不说清楚，客户改完 `-s` 会以为正在传的任务变了。
        assert!(SettingsForm::APPLY_NOTE.contains("即时生效"));
        assert!(SettingsForm::APPLY_NOTE.contains("新任务"));
        assert!(!SettingsForm::APPLY_NOTE.is_empty());
    }

    /// 上游 `aSaveFailureShowsTheKernelTextVerbatim`。
    ///
    /// ⚠️ **本波次的修订（R-38）**：这一条从前断言的是 `SettingsSaveFailure::message`，
    ///    而那个类型**零生产调用者**（生产路径上"设置保存失败"那句话走 `CallFailure::text()`，
    ///    见模块里那段说明）⇒ 它连同类型一起删掉了。**判据本身留在这里**：
    ///    它盯的是 `error_text` —— 那句话的**唯一实现**，也是设置窗口今天真正走的那一条。
    ///    删掉之后这条用例仍然逐字钉着同三件事（内核原文逐字、不借道 `Display`、
    ///    非 `Kernel` 变体照登 `client.rs` 里那句字面量）。
    #[test]
    fn a_save_failure_shows_the_kernel_text_verbatim() {
        // 判别力：改写措辞（例如"保存失败，请重试"）会让下面几句全红 ——
        // 而客户最需要看到的恰恰是内核那句"当前 99"和那半句"设置已保存，但…"。
        assert_eq!(
            error_text(&ClientError::Kernel {
                code: "invalid_params".to_string(),
                message: "并行文件数必须在 1–64 之间，当前 99".to_string(),
            }),
            "并行文件数必须在 1–64 之间，当前 99"
        );
        // "已落盘、但下发引擎失败"那一支：这句话的**前半句**（设置已保存）是关键信息，
        // 任何统一改写成"保存失败"的实现在这里就丢掉了它。
        let partial = "设置已保存，但下发到下载引擎失败：connection refused";
        assert_eq!(
            error_text(&ClientError::Kernel {
                code: "engine_rpc_failed".to_string(),
                message: partial.to_string(),
            }),
            partial
        );
        // 闸门那条"壳自己造的传输错误"也照登（它是 `.unavailable` 时唯一能看到的原文）。
        //
        // ⚠️ 右边是**字面量**（`client.rs` 里 `KernelGone` 那句 `Display` 原文），
        //    **不是** `ClientError::KernelGone.to_string()`：后者与左边**是同一个表达式**
        //    （`error_text` 对非 `Kernel` 变体就是 `other => other.to_string()`），
        //    那样的断言恒真、任何"老实走 `error_text`"的实现都恒过 ——
        //    判别力为零，而它夹在一个"看起来覆盖了三条"的用例里（假覆盖比不覆盖危险）。
        //    上游那一条钉的正是**字面量**原文。
        assert_eq!(
            error_text(&ClientError::KernelGone),
            "内核进程已退出（管道结束）"
        );
    }

    // -----------------------------------------------------------------------
    // 🔴 从**视图层**搬进来的三样（上游没有单测 —— 本波次裁决要搬）
    //
    // 理由：我们这边的"视图"是 JS，而 JS **不许自造任何面向用户的字符串**（规格 §3.2）。
    // 所以这些文案**必须**有 Rust 侧的来源，否则前端只能自己写死一份 ——
    // 那正是"同一个数会有两个真相源"（`resident_notice` 那两条高度常量是同一条裁决）。
    // -----------------------------------------------------------------------

    /// 新增（上游在 `SettingsView.swift:285-291` 的视图层，没有单测）：
    /// 七项参数的**标签与区间**。
    ///
    /// ⚠️ 简报里那串「1–64 / 1–64 / 1–64 / 0–… / 0–… / 1–…」是**错的**
    ///    （控制者已订正）；这里逐字对着 `SettingsForm.swift:188-193` 的
    ///    `static let limits` 与 `SettingsView.swift` 的七行控件读。
    ///    ⚠️ 区间本身只有一份真相（[`SettingsForm::LIMITS`]），标签只有一份真相
    ///    （下面这张表）—— 本函数把它们**配**在一起，不是又抄一遍数。
    #[test]
    fn the_seven_parameters_have_their_labels_and_ranges() {
        let parameters = SettingsForm::parameters();
        assert_eq!(parameters.len(), 7, "七项，一个不多一个不少");

        let labels: Vec<&str> = parameters.iter().map(|p| p.label).collect();
        assert_eq!(
            labels,
            vec![
                "并行文件数（-j）",
                "单文件连接数（-x）",
                "分片数（-s）",
                "最小分片大小（-k）",
                "限速（MB/s）",
                "重试次数",
                "重试间隔（秒）",
            ],
            "标签逐字来自 SettingsView.swift（含全角括号）"
        );

        // 六项有数值区间，`-k` 那一项的取值面来自内核的 `hello`（⇒ 没有区间）。
        let ranges: Vec<(Option<i64>, Option<i64>)> =
            parameters.iter().map(|p| (p.min, p.max)).collect();
        assert_eq!(
            ranges,
            vec![
                (Some(1), Some(64)),
                (Some(1), Some(16)),
                (Some(1), Some(16)),
                (None, None),
                (Some(0), Some(100_000)),
                (Some(1), Some(100)),
                (Some(0), Some(60)),
            ]
        );

        // 而且它们**就是从 LIMITS 派生的**（不是手抄的第二份数）：改一处，两处一起动。
        assert_eq!(
            parameters[0].min,
            Some(*SettingsForm::LIMITS.parallel.start() as i64)
        );
        assert_eq!(
            parameters[4].max,
            Some(*SettingsForm::LIMITS.limit_mbps.end())
        );
    }

    /// 新增（上游在 `SettingsView.swift:425` 的视图层，没有单测）：保存条上那句横幅。
    #[test]
    fn the_dirty_banner_says_unsaved_when_it_differs() {
        assert_eq!(SettingsForm::dirty_banner(true), "有未保存的改动");
        assert_ne!(
            SettingsForm::dirty_banner(true),
            SettingsForm::dirty_banner(false),
            "两种状态必须在界面上分得开（判据是 `matches`）"
        );
        // 判据的接线：表里的七项与内核那份不一致 ⇒ 就是"有未保存的改动"。
        let dirty = SettingsForm::new(&default_settings(), vec![]);
        assert_eq!(
            SettingsForm::dirty_banner(!dirty.matches(Some(&fixture(8, 16, 16, "20M", 0, 3, 60)))),
            "有未保存的改动"
        );
    }

    /// 新增（上游在 `SettingsView.swift:425` 的视图层，没有单测）：与内核一致那一句。
    #[test]
    fn the_clean_banner_says_it_matches_the_kernel() {
        assert_eq!(SettingsForm::dirty_banner(false), "与内核当前参数一致");
        let clean = SettingsForm::new(&default_settings(), vec![]);
        assert_eq!(
            SettingsForm::dirty_banner(!clean.matches(Some(&default_settings()))),
            "与内核当前参数一致"
        );
    }

    /// 新增（上游在 `SettingsView.swift:376-380` 的视图层，没有单测）：
    /// 保存按钮的两份 help，外加"引擎不可用"那一份（那一句来自既有的
    /// `EngineGate::UNAVAILABLE_HELP`，本模块**不重抄**它）。
    #[test]
    fn the_save_button_help_follows_the_same_three_states() {
        assert_eq!(
            SettingsForm::save_help(true, true),
            "把七项参数下发给内核（set_settings）"
        );
        assert_eq!(
            SettingsForm::save_help(true, false),
            "与内核当前参数一致：没有要保存的改动"
        );
        // 引擎不可用时**压过**上面两支：那句提示指向那处真的存在的动作（重试），
        // 而不是让用户对着一个点不动的按钮猜。
        assert_eq!(
            SettingsForm::save_help(false, true),
            crate::presentation::transfer_row::EngineGate::UNAVAILABLE_HELP
        );
        assert_eq!(
            SettingsForm::save_help(false, false),
            crate::presentation::transfer_row::EngineGate::UNAVAILABLE_HELP
        );
        assert!(SettingsForm::save_help(false, true).contains("重试"));
    }
}
