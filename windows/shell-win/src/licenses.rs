//! `licenses` —— **随附的三份许可全文**（规格 **W-5** / 控制者**裁定 RR**）。
//!
//! ## 为什么这是一个独立模块，而不是写在视图里
//!
//! W-5 逐字要求"许可全文要随附**并能在界面里看到**"。在此之前，那些全文都**只在仓库里**
//! （`core/assets/COPYING-GPLv2.txt`、`windows/assets/OFL-1.1.txt`）—— exe 里没有、
//! 界面上也看不到。那是"**看起来合规**"：分发时缺了法律要求的随附文本，而**没有任何东西
//! 会因此变红**。
//!
//! 所以这里做两件事：
//!   1. **把全文编进 exe**（`include_str!` ⇒ 编译期资产，不可能"忘了带"）；
//!   2. 给它们**唯一的登记处**（[`LICENSES`]）：界面那一侧只做绑定，不在这里拼文案。
//!
//! ⚠️ **三份许可是三份独立的义务，别把它们看成一类**：
//!   * **GPLv2** 随附的是内嵌的 **aria2**（下载引擎）；
//!   * **OFL-1.1** 随附的是内嵌的**界面字体**（Noto Sans SC 子集，`windows/assets/ui-subset.otf`）；
//!   * **3-Clause BSD**（任务 15 加的第三条）随附的是**随交付物再分发的微软代码**
//!     —— `WebView2Loader.dll`（`windows/vendor/webview2-com-sys/x64/`，内嵌进 exe）。
//! 各自的全文、各自的标题、各自"为什么会在这里"的一句话都在 [`LICENSES`] 里。
//!
//! ## ⚠️ 第三条的来源与「可审计」的那三样（任务 15）
//!
//! * **确切 URL**（官方渠道，api.nuget.org；那份许可条款本身就要求"必须直接从微软获取"）：
//!   `https://api.nuget.org/v3-flatcontainer/microsoft.web.webview2/1.0.3650.58/microsoft.web.webview2.1.0.3650.58.nupkg`
//! * **版本 = 1.0.3650.58**，**不是猜的**：`webview2-com-sys 0.38.2` 的 CHANGELOG 逐字写着
//!   "update WebView2 SDK to 1.0.3650.58"；更强的证据是**包里的那一份 DLL 与仓库里
//!   `vendor/…/x64/WebView2Loader.dll` 的 sha256 完全相同**
//!   （`8427b1fc58ec707813e5c0a51eb5d69397bb333250a7b891be4d3b123f1e0f1c`，
//!   也就是 `shell-win/build.rs` 的 `LOADER_DLL_SHA256`）⇒ **版本选对了，不是"取最新的"**。
//! * **包本体** `microsoft.web.webview2.1.0.3650.58.nupkg`：8,965,153 B，
//!   sha256 `911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60`；
//!   我们所取的那一份 `LICENSE.txt`：**1,487 B**，
//!   sha256 `0af8f1b807512aae39c2ac1aa4d0cae65cabecb6fd554b8439a5162a0d6eca55`
//!   （**入库的 `windows/assets/WebView2-SDK-LICENSE.txt` 与它逐字节相同**，有用例钉着）。
//!
//! ⚠️ **只取了 `LICENSE.txt`，没有取 `NOTICE.txt`** —— 这是一次**能被审查的决定**，理由：
//!    包里的 `NOTICE.txt` 列的是**该 SDK 的托管程序集**（`.NET` 的 ANTLR3 / StringTemplate4）
//!    所携带的第三方材料，而我们**只再分发 `WebView2Loader.dll`**（原生 C++ 装载器，
//!    157 KB；`strings` 实测**不含**那两个组件名）。⇒ 与"我们分发出去的那些字节"无关。
//!    **若将来把 SDK 里的托管程序集也随包发出去，这一条必须重新审。**
//!
//! ⚠️ **别把 WebView2 *Runtime* 的许可混进来**（规格 §10 待定项 D-1 的原始警告）：
//!    那份是 **Edge WebView2 软件许可条款**（用户机器上由系统/Edge 提供），
//!    **不随我们分发**；本条登记的是 **SDK**（= 我们内嵌的那个 DLL）的许可。
//!    两者混了**比没有更坏** —— 那会让这个登记处记着一份管错了东西的文本。
//!    （这条是从"取回来的到底是什么"读出来的：全文以 `Copyright (C) Microsoft Corporation`
//!    开头、三条 `Redistribution…` 条款、无任何 Edge/Runtime/EULA 字样。）
//!
//! ## ⚠️ "读不到全文时不许显示空白页"在**这一侧**是怎么被守住的（与 macOS 的差别）
//!
//! macOS 侧（`LicenseView.swift` + `LicenseText.swift`）的全文来自 **app 包里的文件**，
//! 所以它必须处理"文件不在/是空的/读不出来"三种运行期失败 —— 那三支各有各的话术。
//!
//! 本壳的全文是**编译期常量**（`include_str!`）：文件不在 ⇒ **编不过**；
//! 文件是空的 ⇒ 下面那三条 `const` 断言在**编译期**就炸。⇒ 运行期**没有**那条失败路径，
//! 也就没有"空白页"这个形态（这是**形态偏离**，W-6 记账：偏离的是实现，不是义务）。
//! 落地成"能看见"的那一半**排在任务 7**：读这些全文的**命令**（`license()`，规格 §3.4
//! 那张表）与页面那一屏都还没做（修订史：原文说的是 egui 的 `views/license.rs`，
//! 那个视图随阶段 A 删除；接着是第二代本地 HTTP 服务的 `/api/license` 端点，
//! 那个服务随 Tauri 那一代整体删除 —— **两处都只是文档，代码一字未改**）。
//! ⇒ **命令与页面已在 Tauri 那一代落地**（`commands.rs` 的 `license()` + 设置里的
//!   「开源许可」面板），本模块**不再**是"被内嵌、但无人读"的状态。
//!
//! ⚠️ 但**编译期嵌入也有一处会静默出错的地方**：`include_str!` 的路径指错了文件
//! （比如指到 `core/assets/COPYING-OFL-1.1.txt` 或指到一份摘要）—— 那一样编得过、
//! 断言也可能过。所以下面的用例**各自回到磁盘上**去读权威原件、逐字节比对
//! （`CARGO_MANIFEST_DIR` 是编译期给的，测试进程自己的 cwd 不参与）。
//!
//! ⚠️⚠️ **而"真的编进去了一份全文"这件事，靠的是三个**登记值**（[`GPLV2_SHA256`] /
//!    [`OFL_1_1_SHA256`] / [`WEBVIEW2_SDK_SHA256`]）—— 审查 2026-09-19 重要 #1 的教训：
//!    只 grep 标题行的判据对"换成一段 stub、保留标题"是**绿**的。理由写在下面那段登记说明里。

/// 内嵌的 **GPLv2** 全文（内嵌组件 = aria2 下载引擎）。
///
/// ⚠️ **只有一份 GPLv2 全文入库**：`core/assets/COPYING-GPLv2.txt`（它自己又是
/// `build.rs` 从 `downloader/internal/engine/assets/` 拷来的副本，见那边的文件头）。
/// 这里直接引它，**不**在 `windows/` 下再存一份 —— 三份拷贝就是三个会漂移的真相。
pub const GPLV2: &str = include_str!("../../../core/assets/COPYING-GPLv2.txt");

/// 内嵌的 **SIL OFL 1.1** 全文（内嵌组件 = 界面字体子集）。
///
/// ⚠️ 它与 `core/assets/COPYING-OFL-1.1.txt` **逐字节相同**（两条路径都指向官方包里的
/// `LICENSE` 原文）。**这一条由下面的用例逐字节比对守住** —— 简报里写了它是事实，
/// 但事实需要一条网，不然下一次换字体时两边会静默分叉。
pub const OFL_1_1: &str = include_str!("../../assets/OFL-1.1.txt");

/// 内嵌的 **3-Clause BSD** 全文（再分发的组件 = 微软的 `WebView2Loader.dll`）。
///
/// ⚠️ 它与 NuGet 包里那份 `LICENSE.txt` **逐字节相同**（来源与三样可审计的凭据写在
/// 文件头"第三条的来源"那一节）。**这一条由下面的用例逐字节比对守住**。
pub const WEBVIEW2_SDK: &str = include_str!("../../assets/WebView2-SDK-LICENSE.txt");

// ---------------------------------------------------------------------------
// 三份全文的**身份登记**（sha256 + 长度下限）
// ---------------------------------------------------------------------------
//
// ⚠️⚠️ **为什么需要登记值，而不是"从文件现算"**（审查 2026-09-19 的**重要 #1**）：
//     `build_windows.sh` 原来只 grep 一行**标题**（`GNU GENERAL PUBLIC LICENSE`），
//     于是"把全文换成一段 stub、只保留标题行"能**一路绿到出海**——判据没有判别力，
//     而它看起来像在检查。**从文件现算哈希也救不了**：文件换成 stub，现算出来的哈希
//     会跟着变，判据恒真。⇒ 唯一的办法是**人工登记**一个**独立于构建输入**的期望值：
//     登记值与文件不符 ⇒ 红（而不是"自动跟上"）。这与 `make_font_subset.sh` 的
//     `SUBSET_SHA256`、`fetch_windows_aria2c.sh` 的资产 sha256 是同一条纪律。
//
// ⚠️ **改了全文就必须改这里的登记值**（三处一起：那个 `*_SHA256` 常量、
//    `LICENSES` 里那一格、以及长度下限）。改错了不会有任何东西"自动变绿"：
//    下面的用例与 `build_windows.sh` 第 5a 步都会红，而且红的时候会告诉你**期望值是多少**。
//
// ⚠️ 这些常量**不是摆设**：设置里的「开源许可」那一屏要把它们显示出来（"全文 sha256：…"，
//    可选中复制），所以它们**在 exe 里**（构建脚本第 5 步能 grep 到）—— 用户也能拿它去跟
//    官方发布件核对。

/// **GPLv2 全文的 sha256**（源头：`core/assets/COPYING-GPLv2.txt`，18,092 B）。
///
/// 它是"内嵌的确实是那一份 GPLv2"的**登记值**（理由见上）。
pub const GPLV2_SHA256: &str = "8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643";

/// **OFL-1.1 全文的 sha256**（源头：`windows/assets/OFL-1.1.txt`，4,301 B）。
///
/// ⚠️ 它与 `core/assets/COPYING-OFL-1.1.txt` 逐字节相同（下面有用例钉住）。
pub const OFL_1_1_SHA256: &str = "6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2";

/// **WebView2 SDK 许可（3-Clause BSD）全文的 sha256**
/// （源头：`windows/assets/WebView2-SDK-LICENSE.txt`，1,487 B）。
///
/// ⚠️ **它是"我们随交付物再分发的微软代码附上了正确的许可全文"的登记值**：
///    这台机器上**没有任何别的东西**能证明那份 DLL 的再分发义务被履行了
///    （`build.rs` 的 `LOADER_DLL_SHA256` 证明的是"发的是哪一份 DLL"，不是"许可跟上了"）。
/// ⚠️ **这一行必须写成一整行**（`pub const …: &str = "<64 位>";`）：
///    `build_windows.sh` 的 `lic_sha()` 按**同一行**的 `^pub const <名字>: &str = "`
///    去读它（另两条也是那个形状）。拆成两行 ⇒ 那条读法读不到 ⇒ 构建当场红，
///    而红的话会告诉你"读不到登记值"（根因说对了，但那是一次本可以避免的返工）。
pub const WEBVIEW2_SDK_SHA256: &str = "0af8f1b807512aae39c2ac1aa4d0cae65cabecb6fd554b8439a5162a0d6eca55";

// ⚠️ **编译期的"绝不空白 + 不许是摘要"**：三份全文的长度下限。
//    `include_str!` 只能保证"文件在场且能当 UTF-8 读"，**不能**保证它不是空的
//    （一个 0 字节的 COPYING 一样编得过）。空白的许可页在界面上与"没做"完全一样，
//    而且谁都不会去查一个空白页 —— 所以这一条必须在**编不过**那一档拦住。
//    ⚠️ 门槛与 `build_windows.sh` 里那三条**是同一组数** —— **但不是各写一遍**：
//       那个脚本用 `lic_min()` 从**本文件**读（与它读 sha256 的 `lic_sha()` 同一形状），
//       所以这里是**唯一**的真相（2026-09-20 任务 15 收口：在此之前是三对字面量分别在
//       两个文件里，改一处不会有任何东西变红 —— 账本 §6.6 ⑥）。
//       取值口径：**明显小于真全文**（GPLv2 真长 18,092 B、OFL 真长 4,301 B、
//       WebView2 许可真长 1,487 B）—— 留出足够余量（将来官方文本的极小改动不该让判据
//       假红），但仍然能拦住"整份换成一段 stub"（审查里那个变异用的是 5.5 KB 的 stub）。
const GPLV2_MIN_BYTES: usize = 15_000;
const OFL_1_1_MIN_BYTES: usize = 4_000;
/// ⚠️ 这一条比另两条小得多，因为**这份文本本来就短**（BSD-3 全文 1,487 B）。
///    1,200 B 把它与"只留版权行"（约 55 B）或"留版权行 + 三条条款但删掉免责声明"
///    （约 900 B）的 stub 区分开，同时给官方的极小改版留了 19% 的余量。
const WEBVIEW2_SDK_MIN_BYTES: usize = 1_200;

const _: () = assert!(
    GPLV2.len() >= GPLV2_MIN_BYTES,
    "内嵌的 GPLv2 全文短得不像全文 —— 空白的许可页等于没履行义务，所以这里在编译期就拦住"
);
const _: () = assert!(
    OFL_1_1.len() >= OFL_1_1_MIN_BYTES,
    "内嵌的 OFL-1.1 全文短得不像全文 —— 理由同上"
);
const _: () = assert!(
    WEBVIEW2_SDK.len() >= WEBVIEW2_SDK_MIN_BYTES,
    "内嵌的 WebView2 SDK 许可全文短得不像全文 —— 理由同上（这份本来就短，见下限那一段）"
);

/// 一条许可：标题 + "它为什么会在这个程序里" + 全文。
///
/// ⚠️ 字段名与 macOS 侧 `LicenseView` 的三块内容一一对应（标题 / caption / 可滚动全文），
///    那边 caption 说的是 GPLv2 那一件事（"本应用内嵌 aria2（GPLv2 许可）…"）。
///    本壳有**三份**许可，所以那句话必须**跟着条目走**，不能写成一条全局的常量 ——
///    否则 OFL 那一页会写着 aria2 的话（一句**看起来对、实际错**的说明）。
pub struct License {
    /// 界面上的条目名（选择器里那一行）。
    pub name: &'static str,
    /// 一句话说明：这份许可是**随哪个内嵌组件**来的。
    pub why: &'static str,
    /// 全文（**逐字**，只做排版，不截断、不重排）。
    pub text: &'static str,
    /// 全文的 sha256（**登记值**，见上面那一段）。
    ///
    /// ⚠️ 它**不是装饰**：界面把它显示出来（"全文 sha256：…"，可选中复制），
    ///    于是它**真的被编进 exe** —— 构建脚本第 5a 步靠它证明"编进去的是登记的
    ///    那一份全文"，而不是一段 stub（审查重要 #1 的修法②）。
    pub sha256: &'static str,
}

/// 随本程序一起分发的三份许可。**这是唯一的登记处**（界面那一侧照它渲染）。
///
/// ⚠️ 顺序即界面上的顺序（GPLv2 → OFL → WebView2 SDK），**不是**"重要性"的排序。
///    第三条（WebView2 SDK）是**任务 15 补的**：在此之前，我们**已经在随交付物再分发**
///    微软的那个 DLL，而这里只有两条 —— 那正是本仓库自己的 W-5 纪律不允许的形状
///    （"内嵌的可再分发资产必须随附许可全文"）。
pub const LICENSES: [License; 3] = [
    License {
        name: "GNU 通用公共许可协议 第 2 版（GPLv2）",
        why: "本程序内嵌 aria2（GPLv2 许可）作为下载引擎，因此随附下列许可全文。",
        text: GPLV2,
        sha256: GPLV2_SHA256,
    },
    License {
        name: "SIL 开放字体许可 1.1（OFL-1.1）",
        why: "本程序内嵌 Noto Sans SC 的一个子集（SIL OFL 1.1 许可）作为界面字体，\
              因此随附下列许可全文。",
        text: OFL_1_1,
        sha256: OFL_1_1_SHA256,
    },
    License {
        name: "WebView2 SDK 许可（3-Clause BSD）",
        why: "本程序随交付物再分发 Microsoft 的 WebView2Loader.dll（WebView2 SDK，\
              3-Clause BSD 许可），因此随附下列许可全文。",
        text: WEBVIEW2_SDK,
        sha256: WEBVIEW2_SDK_SHA256,
    },
];

/// 三份许可 → **给前端的载荷**（`license` 命令的唯一映射点）。
///
/// ⚠️ **为什么映射在本文件而不是命令层**：四格里有**两格是面向用户的文案**
///    （`name` 的标题与 `why` 的"这份许可是随哪个内嵌组件来的"），而命令层
///    **一个字都不许自己写**（R-24）。它们本来就住在本文件（[`LICENSES`]）
///    —— 映射跟着数据走，命令层只写 `api::envelope::ok(licenses::wire())`。
///
/// ⚠️ `text` 是**全文**（18 KB + 4 KB + 1.5 KB）：不截断、不重排、不做摘要。许可页要能
///    整份看到、整份选中复制（W-5："随附**并能**看到"）。这一格**必须**发出去 ——
///    少了它，界面上就只剩标题与 sha256，而"全文"又回到了"只在仓库里"。
///
/// ⚠️ `sha256` 也发出去：界面把它显示出来（可选中复制），用户能拿它去跟官方发布件核对
///    （见那三个登记值的文档）。
pub fn wire() -> serde_json::Value {
    let entries: Vec<serde_json::Value> = LICENSES
        .iter()
        .map(|license| {
            serde_json::json!({
                "name": license.name,
                "why": license.why,
                "text": license.text,
                "sha256": license.sha256,
            })
        })
        .collect();
    serde_json::json!({ "licenses": entries })
}

#[cfg(test)]
mod tests {
    //! ⚠️ 这里的每一条都**回到磁盘**去读权威原件：`include_str!` 编进来的串与
    //!    "路径指对了没有"是两件事，而后者是这一层唯一会静默出错的地方。

    use super::*;
    use std::path::PathBuf;

    /// 三份全文里**最短**的那一条的下限。
    ///
    /// ⚠️ 这是**测试里**的下限口径（"它至少得像一份全文"）。逐条与自己那个下限比对
    /// 在 [`all_three_texts_are_longer_than_the_registered_floors`] 里做 ——
    /// 两条判据问的不是同一件事，别合并。
    const FLOOR_OF_THE_SHORTEST: usize = WEBVIEW2_SDK_MIN_BYTES;

    fn repo_file(rel: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {}（{}）：{e}", path.display(), rel))
    }

    /// ⭐ **登记值必须与内嵌的全文逐字节相符**（审查重要 #1 的回归网）。
    ///
    /// 判别力：**把任意一条 `include_str!` 换成一段 stub（哪怕保留标题行）⇒ 这一条立刻红**。
    /// 而在修这条之前，"只 grep 标题"的判据对那个变异是**绿的**（它一路出海）。
    ///
    /// ⚠️ 这条与下面那条"逐字节等于磁盘原件"是**两件事**：
    ///    * 这一条钉的是"编进来的字节 == **登记**的那一份"（登记值在源码里，是人工维护的）；
    ///    * 那一条钉的是"编进来的字节 == 磁盘上那份文件"（防 `include_str!` 的**路径**写错）。
    ///    两条都过，才能说"客户拿到的就是那份 GPLv2 / OFL / BSD"。
    #[test]
    fn the_registered_digests_match_the_embedded_texts() {
        assert_eq!(
            shell_core::sha256::sha256_hex(GPLV2.as_bytes()),
            GPLV2_SHA256,
            "内嵌的 GPLv2 与**登记值**不符 —— 全文被换过（或改动过）却没改登记？\
             若是**有意**换的，请同时更新 GPLV2_SHA256 与长度下限；若不是，把全文换回来。"
        );
        assert_eq!(
            shell_core::sha256::sha256_hex(OFL_1_1.as_bytes()),
            OFL_1_1_SHA256,
            "内嵌的 OFL-1.1 与登记值不符 —— 同上"
        );
        assert_eq!(
            shell_core::sha256::sha256_hex(WEBVIEW2_SDK.as_bytes()),
            WEBVIEW2_SDK_SHA256,
            "内嵌的 WebView2 SDK 许可与登记值不符 —— 同上。\
             ⚠️ 换这一份之前先把来源与三样凭据（URL / 包版本 / 包 sha256）写进文件头：\
             这份文本是**再分发的 Microsoft 代码**的许可，不是随手可换的文案。"
        );
        // 登记值本身的形状（少一位、带大写都会让构建脚本那条 grep 变成"永远查不到"）。
        for digest in [GPLV2_SHA256, OFL_1_1_SHA256, WEBVIEW2_SDK_SHA256] {
            assert_eq!(digest.len(), 64, "sha256 的十六进制形式恒为 64 个字符：{digest}");
            assert!(
                digest.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "必须是小写十六进制：{digest}"
            );
        }
        assert_ne!(GPLV2_SHA256, OFL_1_1_SHA256, "两份登记值不该相同");
        assert_ne!(GPLV2_SHA256, WEBVIEW2_SDK_SHA256, "两份登记值不该相同");
        assert_ne!(OFL_1_1_SHA256, WEBVIEW2_SDK_SHA256, "两份登记值不该相同");
    }

    /// 长度下限：拦住"整份换成一段 stub"（审查里那个变异用的是 5.5 KB 的 stub）。
    ///
    /// 判别力：真全文 18,092 B / 4,301 B / 1,487 B；把任意一条 `include_str!`
    /// 换成 5.5 KB 的 stub ⇒ 对**前两条**红（第三条的下限本来就是 1,200 B，
    /// 见下面那条断言里的说明）。
    #[test]
    fn all_three_texts_are_longer_than_the_registered_floors() {
        assert!(
            GPLV2.len() >= GPLV2_MIN_BYTES,
            "GPLv2 只有 {} 字节（下限 {}）—— 那不是全文",
            GPLV2.len(),
            GPLV2_MIN_BYTES
        );
        assert!(
            OFL_1_1.len() >= OFL_1_1_MIN_BYTES,
            "OFL-1.1 只有 {} 字节（下限 {}）—— 那不是全文",
            OFL_1_1.len(),
            OFL_1_1_MIN_BYTES
        );
        assert!(
            WEBVIEW2_SDK.len() >= WEBVIEW2_SDK_MIN_BYTES,
            "WebView2 SDK 许可只有 {} 字节（下限 {}）—— 那不是全文（BSD-3 全文约 1.5 KB）",
            WEBVIEW2_SDK.len(),
            WEBVIEW2_SDK_MIN_BYTES
        );
        // ⚠️ **"最短那份的下限最小"这件事要有判据**：`build_windows.sh` 用同一组数
        //    （它从本文件读，见 `lic_min()`），而下面那条"像不像全文"的通用断言用的是
        //    `FLOOR_OF_THE_SHORTEST`。若有人把某条下限写成大于另一条真全文的值，
        //    那些断言会以"某份许可短得不像全文"的形式红 —— 根因却是**下限写错了**。
        assert!(
            GPLV2_MIN_BYTES > WEBVIEW2_SDK_MIN_BYTES,
            "下限之间的大小关系反了：GPLv2 的全文比 WebView2 许可长得多，下限也该更大"
        );
    }

    /// 三份全文都在场、都是**全文**（不是摘要、不是空文件）。
    #[test]
    fn all_three_licenses_are_present_and_are_full_texts() {
        assert_eq!(
            LICENSES.len(),
            3,
            "随附义务有三份：aria2 的 GPLv2、字体的 OFL-1.1、Microsoft 的 WebView2Loader.dll 之 BSD-3"
        );
        for license in LICENSES {
            assert!(
                license.text.len() >= FLOOR_OF_THE_SHORTEST,
                "{} 的全文只有 {} 字节 —— 那不像一份全文（连三份里较短那份的下限都没到）",
                license.name,
                license.text.len()
            );
            assert!(
                !license.sha256.is_empty(),
                "{} 没有登记 sha256 —— 构建脚本第 5a 步靠它判'编进去的是不是全文'",
                license.name
            );
            assert!(
                license.text.contains('\n'),
                "{} 的全文连换行都没有？",
                license.name
            );
            assert!(
                !license.why.is_empty() && license.why.contains('许'),
                "{} 缺一句'为什么会在这里'（OFL 那一页写 aria2 的话就是这种错的样子）",
                license.name
            );
        }
    }

    /// **标题真的在全文里**（编进来的确实是那三份文档）。
    ///
    /// 判别力：把任意两条 `include_str!` 的路径**对调**，这一条立刻红
    /// （三份长度都 > 1000，光看长度是发现不了的）。
    #[test]
    fn each_text_is_the_document_its_name_claims() {
        assert!(
            GPLV2.contains("GNU GENERAL PUBLIC LICENSE"),
            "GPLv2 的全文里必须有它的标题行"
        );
        assert!(
            GPLV2.contains("Version 2, June 1991"),
            "GPLv2 的全文里必须有版本行（第 2 版）"
        );
        assert!(
            OFL_1_1.contains("SIL OPEN FONT LICENSE"),
            "OFL-1.1 的全文里必须有它的标题行"
        );
        assert!(
            OFL_1_1.contains("Version 1.1"),
            "OFL-1.1 的全文里必须有版本行"
        );
        // ⚠️ 第三条**没有标题行**（BSD-3 的原文是从版权行直接开始的）⇒ 这里的判据只能
        //    取它**结构上一定有**的那几段：版权行 + 三条条款各自的标志句。
        //    ⚠️ 别把判据写成"含 BSD"——那份文本里**根本没有 "BSD" 这三个字母**（实测）。
        assert!(
            WEBVIEW2_SDK.contains("Copyright (C) Microsoft Corporation"),
            "WebView2 SDK 许可的全文里必须有版权行（BSD-3 的全文就是以它开头的，没有标题行）"
        );
        assert!(
            WEBVIEW2_SDK.contains("Redistribution and use in source and binary forms"),
            "必须有第一条：源码再分发要保留版权声明"
        );
        assert!(
            WEBVIEW2_SDK.contains("Redistributions in binary form must reproduce"),
            "必须有第二条 —— **正是我们触发的那一条**（二进制再分发要复现版权声明、条件与那段不保证的话）"
        );
        assert!(
            WEBVIEW2_SDK.contains("THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS"),
            "必须有那段不保证的声明（第二条要求连它也一起复现）"
        );
        assert_ne!(GPLV2, OFL_1_1, "两份许可不是同一份文档（对调路径会从这一条露出来）");
        assert_ne!(GPLV2, WEBVIEW2_SDK, "同上");
        assert_ne!(OFL_1_1, WEBVIEW2_SDK, "同上");
    }

    /// ⚠️ **这一条钉的是"我们触发的是哪一条义务"** —— 不是文案对不对。
    ///
    /// 我们把那个 DLL **以二进制形式**内嵌进 exe 再分发 ⇒ 触发的是 BSD-3 的**第二条**
    /// （"Redistributions in binary form must reproduce …"）。⇒ 全文里必须有它，
    /// 否则这份登记**看着合规、实际履行的不是那一条**。
    ///
    /// 判别力：把全文换成"只留版权行 + 第一条"的 stub ⇒ 红。
    #[test]
    fn the_bsd_text_contains_the_clause_our_distribution_triggers() {
        assert!(
            WEBVIEW2_SDK.contains("in the documentation and/or other materials provided with the"),
            "BSD-3 第二条要求把版权声明、条件与那段不保证的声明放进**文档和/或随附文档**里 —— \
             那正是「开源许可」那一屏在做的事；这一段不在全文里就说明登记的不是那一份"
        );
        // ⚠️ **反方向也要判**：这份文本**不该**提到 Runtime / Edge / EULA。
        //    那三者属于**另一份**许可（Edge WebView2 软件许可条款，**不随我们分发**），
        //    混进来会让这个登记处记着一份管错了东西的文本（比没有更坏）。
        for forbidden in ["WebView2 Runtime", "Microsoft Edge", "End-User License", "EULA"] {
            assert!(
                !WEBVIEW2_SDK.contains(forbidden),
                "WebView2 SDK 的许可全文里不该出现 `{forbidden}` —— \
                 那是 **Runtime**（Edge 那一份）的措辞，而我们只再分发 loader"
            );
        }
    }

    /// **逐字节等于磁盘上的权威原件**（这一条替"来源可审计"当网）。
    #[test]
    fn the_embedded_bytes_equal_the_canonical_files_on_disk() {
        assert_eq!(
            GPLV2,
            repo_file("../../core/assets/COPYING-GPLv2.txt"),
            "内嵌的 GPLv2 必须**逐字**是 core/assets 里那一份（那是入库的唯一真相）"
        );
        assert_eq!(
            OFL_1_1,
            repo_file("../assets/OFL-1.1.txt"),
            "内嵌的 OFL-1.1 必须逐字是 windows/assets 里那一份（字体子集随附的那份）"
        );
        assert_eq!(
            WEBVIEW2_SDK,
            repo_file("../assets/WebView2-SDK-LICENSE.txt"),
            "内嵌的 WebView2 SDK 许可必须逐字是 windows/assets 里那一份\
             （它又是从 Microsoft 的 NuGet 包里取出来的那一份，见文件头）"
        );
        // ⚠️ 简报（裁定 RR）里把这条写成了事实：`windows/assets/OFL-1.1.txt` 与
        //    `core/assets/COPYING-OFL-1.1.txt` **逐字节相同**。事实要有网 ——
        //    不然下一次换字体时两边会静默分叉，而"客户拿到的到底是哪一份"就说不清了。
        assert_eq!(
            repo_file("../assets/OFL-1.1.txt"),
            repo_file("../../core/assets/COPYING-OFL-1.1.txt"),
            "windows/assets/OFL-1.1.txt 与 core/assets/COPYING-OFL-1.1.txt 必须逐字节相同"
        );
    }

    /// 🔴 **`license` 命令的载荷：三份许可、四格、全文都在**（任务 8 接上、任务 15 扩到三份）。
    ///
    /// 判别力：
    ///   * 少发 `text`（只发标题与 sha256）⇒ 第二条断言红 —— 而那时"全文"又回到了
    ///     "只在仓库里"（W-5 的另一半失效，且**没有任何东西会变红**）；
    ///   * 少发 `why` ⇒ 界面那一页就没有"它为什么会在这里"（用户看不出这与自己何干）；
    ///   * 只发两份 ⇒ 第一条断言红。
    #[test]
    fn the_wire_payload_carries_all_three_full_texts() {
        let v = wire();
        let entries = v["licenses"].as_array().expect("licenses 必须是数组");
        assert_eq!(entries.len(), LICENSES.len(), "三份许可一份都不能少");
        assert_eq!(
            entries.len(),
            3,
            "随附义务有三份（aria2 的 GPLv2、字体的 OFL-1.1、Microsoft loader 的 BSD-3）——\
             ⚠️ 第三条是**任务 15** 补的：少了它，界面那一屏就看不到我们正在再分发的那份 Microsoft 代码的许可"
        );

        for (entry, license) in entries.iter().zip(LICENSES.iter()) {
            let keys: std::collections::BTreeSet<&str> = entry
                .as_object()
                .expect("每一条都是对象")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                keys,
                ["name", "why", "text", "sha256"].into_iter().collect(),
                "四格一个不多一个不少：{entry}"
            );
            assert_eq!(entry["name"], serde_json::json!(license.name));
            assert_eq!(entry["why"], serde_json::json!(license.why));
            assert_eq!(entry["sha256"], serde_json::json!(license.sha256));
            // ⚠️ **全文**（不是摘要、不是空串）：长度与内嵌的那一份逐字相同。
            assert_eq!(entry["text"], serde_json::json!(license.text));
            assert!(
                entry["text"].as_str().unwrap_or_default().len() >= FLOOR_OF_THE_SHORTEST,
                "{} 的全文短得不像全文：{}",
                license.name,
                entry["text"].as_str().unwrap_or_default().len()
            );
        }
    }

    /// 名称与"为什么"都**非空且互不相同**（界面上是个选择器，重名就分不清点的是哪一条）。
    #[test]
    fn the_three_entries_are_distinguishable_in_the_ui() {
        for (i, a) in LICENSES.iter().enumerate() {
            for b in LICENSES.iter().skip(i + 1) {
                assert_ne!(a.name, b.name, "两条许可的标题重了");
                assert_ne!(a.why, b.why, "两条许可的'为什么会在这里'重了");
            }
        }
        for license in LICENSES {
            assert!(!license.name.trim().is_empty());
            assert!(!license.why.trim().is_empty());
        }
    }
}
