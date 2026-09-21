//! 「在资源管理器中显示」那一件事的**判据与话术**（任务 8 新增：`reveal` 命令）。
//!
//! ## ⚠️ 路径怎么算，本文件**一个字都不重写**
//!
//! 落盘路径 = **清单相对路径拼到落盘根上**，而"落盘根是哪"有两级（配过下载目录就是它；
//! 没配过才是 `<home>/Downloads/Benagen`）—— 那套判据**只有一份实现**：
//! [`TransferReveal::download_root`] / [`TransferReveal::local_path`]
//! （`presentation/transfer_row.rs`，42 条单测钉着，含"为什么不能用相对路径"的论证）。
//! 本文件**只加一道前置闸**（见 [`local_path`]），不另拼一遍路径 ——
//! 拼第二遍的那天，"壳以为的路径"与"内核实际写的路径"就开始各走各的，
//! 而分歧的代价是**定位到空处**（约束 4 明禁的静默失效）。
//!
//! ## 四种结局（形状见 [`Outcome`] 的文档）
//!
//! 平台那一半（`ShellExecuteW + explorer /select,` / 宿主上的 `open -R`）**不在这里**：
//! 它是 `shell-win` 的 `reveal.rs`（OS 调用不进 `shell-core`，章程见 `lib.rs` 头部）。
//! 本文件只回答"**要显示哪一个文件**"，以及"显示不了时怎么说"。

use serde_json::{json, Value};

use crate::api::envelope;
use crate::presentation::transfer_row::TransferReveal;

/// 一次「在资源管理器中显示」的结局。
///
/// ⚠️ **四档不是"成功/失败"两档**（每一档的落点都不同，合并任何两档都会丢信息）：
///   * [`Outcome::Ready`] —— 算出了绝对路径，交给平台那一半去显示；
///   * [`Outcome::NoManifestPath`] —— **这一行没有路径映射**（内核发的 `path` 是 `null`
///     或空串）。界面上那一项本来就该是**禁用**的（`TransferRow::can_reveal_in_finder`），
///     走到这里是**纵深防御**：渲染与点击之间状态可能刚变，那一下不该变成一个空动作；
///   * [`Outcome::RootUnknown`] —— 落盘根**算不出来**（既没配下载目录、又取不到 home
///     那一级环境变量）。它与上一档**不是同一件事**：上一档是"这一行没有文件"，
///     这一档是"**这台机器上我们不知道文件都放在哪**"，后者要说出来（W-2）；
///   * [`Outcome::Failed`] —— 平台那一半没能把资源管理器拉起来（原样带回来）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 要显示的那个文件的**绝对路径**。
    Ready(String),
    /// 没有可显示的路径（那一项本该是禁用的）。
    NoManifestPath,
    /// 落盘根算不出来（句子里要点名缺的是哪一级环境变量）。
    RootUnknown,
    /// 平台调用失败，附带原文。
    Failed(String),
}

/// **判据**：这一行要不要显示、显示哪一个文件。
///
/// ⚠️ **前置闸（本文件唯一新增的判据）**：落盘根**必须是绝对路径**，否则**不猜**。
///    理由是一条真实的坏路径：`home` 取不到时 `download_root` 会拼出
///    `Downloads/Benagen`（一个相对路径），而相对路径的基准是**进程的当前工作目录**
///    （双击启动时是 exe 所在目录）—— 拿它去开资源管理器，用户看到的是"什么都没发生"
///    或者一个毫不相干的文件夹。那正是本项目最恨的静默失效。
///    ⚠️ 判据用的是 `std::path::Path::is_absolute`（**本靶**的文件系统语义），
///    **不是**"里面有没有 `/`"这种近似——后者会把 `C:\…` 之外的写法判错。
///
/// ⚠️ 闸开在 `local_path` **之外**、而不是改它：那个函数是既有判据（有单测、
///    与 macOS 逐字对位），本层的职责是"要不要用它"，不是"改它"。
pub fn local_path(manifest_path: Option<&str>, home: &str, download_dir: &str) -> Outcome {
    // 先分清"这一行有没有文件"与"落盘根算不算得出来"——两句话不一样（见 `Outcome`）。
    if TransferReveal::manifest_path(manifest_path).is_none() {
        return Outcome::NoManifestPath;
    }
    let root = TransferReveal::download_root(home, download_dir);
    if !std::path::Path::new(&root).is_absolute() {
        return Outcome::RootUnknown;
    }
    match TransferReveal::local_path(manifest_path, home, download_dir) {
        Some(path) => Outcome::Ready(path),
        None => Outcome::NoManifestPath,
    }
}

// ---------------------------------------------------------------------------
// 四种结局各自的载荷
// ---------------------------------------------------------------------------

/// 显示成功了。
///
/// ⚠️ `status` 是**状态令牌**（`revealed` / `no_path` 那两个词是给前端分支用的机器值，
///    不是显示文案 —— 与 `load` 的 `loading` / `no_kernel` 同一个口径）。
pub fn revealed() -> Value {
    envelope::ok(json!({ "status": "revealed" }))
}

/// 这一行没有可显示的路径（**不是失败**，见 [`Outcome::NoManifestPath`]）。
pub fn no_path() -> Value {
    envelope::ok(json!({ "status": "no_path" }))
}

/// 「算不出文件在哪」那句话。
///
/// 🔴 **壳自己写的**：这件事根本没走到内核（"在资源管理器中显示"是**壳**的事，
///    规格 §5.2 —— 协议里没有这个动作），所以没有内核原文可登。
///    而它必须说出来（W-2）：用户点了那一项、什么都没发生，唯一的解释就在这句话里。
///
/// ⚠️ `variable` 由调用方给（`TransferReveal::home_variable()`：Windows 上是
///    `USERPROFILE`、其余平台是 `HOME`）—— **不在这里写死一个平台的名字**。
pub fn root_unknown(variable: &str) -> Value {
    envelope::err(&format!(
        "定位不到这个文件：既没有配置过下载目录，也取不到 {variable} 那一级环境变量，\
         算不出它在哪。补救：在设置里指定一个下载目录。"
    ))
}

/// 平台那一半失败时那句话（`cause` 是系统的原文）。
///
/// 🔴 壳自己写的（同 [`root_unknown`]）：那一刻没有内核原文。
///    句子里要**带上一件用户能自己查的事**（那个文件可能已经不在了）——
///    因为"在资源管理器里显示"最常失败的原因就是文件被移走/删掉了，
///    而系统给的那句话（`ShellExecuteW` 的返回值 / `open` 的 stderr）读起来像天书。
pub fn failure(cause: &str) -> Value {
    envelope::err(&format!(
        "没能在资源管理器里显示这个文件（{cause}）。\
         补救：先确认它还在那儿（也许已被移走或删掉），再试一次。"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/x";

    /// 配过下载目录 ⇒ 落盘根就是它（与 home 无关）；算出来的路径要**逐字**是
    /// 那个目录 + `/` + 清单原文。
    #[test]
    fn a_configured_download_directory_wins_over_the_home_default() {
        assert_eq!(
            local_path(Some("PFX/sub/a.txt"), HOME, "/data/交付"),
            Outcome::Ready("/data/交付/PFX/sub/a.txt".to_string())
        );
        // 配过目录时 home 就算是空的也照样算得出来（那一格不参与）。
        assert_eq!(
            local_path(Some("a.txt"), "", "/data/交付"),
            Outcome::Ready("/data/交付/a.txt".to_string())
        );
    }

    /// 没配过 ⇒ 落盘根是 `<home>/Downloads/Benagen`（内核的默认值，与它同源）。
    #[test]
    fn an_unconfigured_download_directory_falls_back_to_the_kernel_default() {
        assert_eq!(
            local_path(Some("PFX/sub/a.txt"), HOME, ""),
            Outcome::Ready("/Users/x/Downloads/Benagen/PFX/sub/a.txt".to_string())
        );
    }

    /// ⚠️ **清单路径是原文**（约束 3）：不 trim、不折叠 `//`、不取最后一段。
    #[test]
    fn the_manifest_path_goes_through_untouched() {
        assert_eq!(
            local_path(Some("a//b/中文 名.bin"), HOME, "/data"),
            Outcome::Ready("/data/a//b/中文 名.bin".to_string())
        );
    }

    /// 🔴 **没有路径那一档**：`null` 与空串都是"这一行没有文件"（**不是**"算不出来"）。
    ///
    /// 判别力：把这两档合成一档（都报失败/都报成功），这一条立刻红 ——
    /// 而真机上的表现是"每一行没有路径的任务都在点开之后弹一句看不懂的错"。
    #[test]
    fn a_row_without_a_path_is_its_own_outcome_not_a_failure() {
        assert_eq!(local_path(None, HOME, "/data"), Outcome::NoManifestPath);
        assert_eq!(local_path(Some(""), HOME, "/data"), Outcome::NoManifestPath);
    }

    /// 🔴 **落盘根算不出来那一档**：没配下载目录 + 取不到 home ⇒ **不猜**（不许拼相对路径）。
    ///
    /// 判别力（这是本文件最要紧的一条）：把前置闸删掉，`TransferReveal::local_path`
    /// 会高高兴兴地返回 `Downloads/Benagen/a.txt`（一个相对路径）⇒ 这一条立刻红。
    /// 而那个相对路径的基准是**进程的当前工作目录**（双击启动时是 exe 所在目录），
    /// 用户在资源管理器里看到的是一个毫不相干的地方 —— 或者干脆什么都没发生。
    #[test]
    fn an_unknown_root_is_refused_instead_of_building_a_relative_path() {
        assert_eq!(local_path(Some("a.txt"), "", ""), Outcome::RootUnknown);
        assert_eq!(local_path(Some("a.txt"), "relative/home", ""), Outcome::RootUnknown);
        // ⚠️ 反方向：配过下载目录时**照样**算得出来（`RootUnknown` 不是恒真的兜底）。
        assert!(matches!(
            local_path(Some("a.txt"), "", "/data/交付"),
            Outcome::Ready(_)
        ));
    }

    /// 四种结局各自的信封（成功 = `ok:true` + 令牌；两种失败 = `ok:false` + 原文）。
    #[test]
    fn every_outcome_has_its_own_envelope() {
        assert_eq!(revealed()["ok"], json!(true));
        assert_eq!(revealed()["data"]["status"], json!("revealed"));
        assert_eq!(no_path()["ok"], json!(true));
        assert_eq!(no_path()["data"]["status"], json!("no_path"));

        let unknown = root_unknown("USERPROFILE");
        assert_eq!(unknown["ok"], json!(false));
        let message = unknown["error"]["message"].as_str().expect("失败要有一句话");
        assert!(message.contains("USERPROFILE"), "要点名缺的是哪一级变量：{message}");
        assert!(message.contains("下载目录"), "要给出可执行的补救：{message}");

        let failed = failure("ShellExecuteW 返回 2");
        assert_eq!(failed["ok"], json!(false));
        let message = failed["error"]["message"].as_str().expect("失败要有一句话");
        assert!(message.contains("ShellExecuteW 返回 2"), "系统原文必须照登：{message}");
        assert!(
            message.contains("还在"),
            "要说清那个文件可能已经被移走或删掉（最常见的原因）：{message}"
        );
    }
}
