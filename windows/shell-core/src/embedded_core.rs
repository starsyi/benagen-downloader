//! `embedded_core` —— **把内嵌的内核 exe 释放到磁盘上**（规格 §7.2）。W-4 的"客户只拿一个 exe"
//! 全靠这一手：内核 exe 被 `include_bytes!` 编进壳里，第一次运行时落到
//! `%LOCALAPPDATA%\BenagenDownloader\cache\benagen-core-<sha 前 12 位>.exe`。
//!
//! ## ⚠️ 这套机制**照抄** `core/src/engine/daemon.rs:918-949` 释放 aria2c 的那一套
//!
//! 不新发明，逐条理由如下（那套写法是带着**实测事故**的注释留下来的）：
//!
//!   * **名字由内容哈希派生**：同一个版本永远同一个名字 ⇒ 不同版本可以共存、
//!     且"名字"本身就声明了内容是什么；
//!   * **哈希相符则复用**：不然每次启动都写十几 MB（白费 I/O，还平白多一次被
//!     杀毒软件盯上的机会）；
//!   * **哈希不符就重写**：**绝不**使用来路不明的文件（那里可能是一个被替换过的内核）；
//!   * **写 `.tmp` 再 `rename`**：`rename` 在同一文件系统内是原子的 ⇒
//!     同机同时起来两个客户端也不会读到写了一半的二进制；
//!   * **私有临时名**（`<name>.<pid>.<序号>.tmp`）：共用 `<name>.tmp` 会**实测**出
//!     一条缺陷 —— 先完成的那次 `rename` 把后来者正在写的 `.tmp` 搬走，后来者的
//!     `rename` 立刻 `ENOENT`（daemon.rs 的 `private_tmp` 注释记着原始报错）。
//!   * **名字必须带 `.exe`**（规格 §7.2 第 2 条）：Windows 的 `CreateProcess` 对
//!     **无扩展名**的 PE 能不能直接执行**是推断、不是实测**（探路没验过）⇒ 不赌。
//!     内核那边释放 aria2c 用的名字**没有**扩展名，这是壳这侧的**有意偏离**。
//!
//! ## ⚠️ 本模块在 `shell-core` 里（而不是视图层）的两个理由
//!
//!   1. 它是**纯逻辑 + `std::fs`**：没有一处 Windows 专有 API，因此**能在宿主上真跑**
//!      （下面那些用例不是"读代码推出来的"，是本机执行过的）；
//!   2. 规格 §4.3：能写出断言的代码一律搬出平台目标。壳那侧的 `embed.rs` 只剩
//!      "内嵌的字节是什么"与"缓存目录在哪"两件事。
//!
//! ## ⚠️ 本机（macOS）验不到的那一半，如实记账
//!
//! "**杀毒软件拦截**"这条失败分支（规格 §14.3 的待定项）在本机**造不出来**：
//! 它的形态取决于那台机器上的杀软。所以 [`ReleaseStage::Verify`] 那一支是**写出来的
//! 真实分支**（不是假设它不会发生），它的触发条件、话术与补救都在这里，
//! **真机验收时要专门看一眼**（把 `%LOCALAPPDATA%\BenagenDownloader` 整个删掉、
//! 开着实时防护再启动一次）。
//!
//! ## ⚠️ 2026-09-20：这套机制**参数化**了（在此之前它只会释放内核 exe）
//!
//! 本代要释放的**不止一件**：内核 exe（`.exe`）与 WebView2Loader DLL（`.dll`）。
//! 一开始走的是"给 DLL 再抄一份"的路（`shell-win/src/wv2.rs`），而**那一份在几小时
//! 之内就漂了**：抄的时候削掉了 `Some(32) | Some(33)`（文件被占着）与
//! `Verify + NotFound`（落地后被隔离）两档 —— 而那两档恰恰是 `.dll` **最**可能撞上的
//! （杀软实时防护按着不放 / 事后隔离），也是它自己的模块头点名成"唯一能自己现形的失败
//! 形态"的那两档。
//! ⇒ 按本仓库那条"**同一个真相不许有两个来源**"改成参数化：[`ReleaseSpec`] 说清
//! "释放的是什么"，机制、分档与话术只有**这一份**（两份的代价在几小时内就现形了）。
//!
//! ⚠️ 参数化只动了"名字与话术里指名道姓的那几处"：**分档逻辑（错误码 → 补救）一条都
//! 没有放宽**，下面 `mod tests` 里既有断言的判别力也一条都没有变。

use std::path::{Path, PathBuf};

/// **"被释放的是什么"** —— 文件名的形状与失败话术里指名道姓的那几处，全由它派生。
///
/// ⚠️ 为什么要这么多字段而不是一个 `name: &str`：**话术里试过只用一个字段**，
///    结果每一句都别扭一点（"释放内嵌WebView2Loader.dll失败" / "把内嵌内核写进缓存目录
///    失败"）。字段各自对应一处**真正因物而异**的地方，逐条写在字段自己的文档里 ——
///    这比在模板里拼字符串诚实（拼出来的话一旦读着别扭，下一个人就会去改模板，
///    而那会把两件东西的话术重新搅在一起）。
pub struct ReleaseSpec {
    /// 释放出来的文件名的前缀（内容哈希接在它后面）。
    pub prefix: &'static str,
    /// 文件名的后缀，**含那个点**（`.exe` / `.dll`）。
    ///
    /// ⚠️ 内核那份**必须**是 `.exe`（Windows 的 `CreateProcess` 对无扩展名的 PE 能不能
    /// 直接执行是推断、不是实测）；DLL 那份**必须**是 `.dll`（`LoadLibraryW` 同理）。
    pub suffix: &'static str,
    /// 「释放 X 失败」那句里的**完整主语短语**（"内嵌内核" / "内嵌的 WebView2Loader.dll"）。
    pub subject: &'static str,
    /// 各阶段那句里指代它的**简短名词**（"内核" / "WebView2Loader.dll"）。
    pub noun: &'static str,
    /// 磁盘满那一档的补充（说清这件东西大概要多少空闲空间），会连括号一起插进那句话；
    /// `None` = 不提（两件东西的量级差 60 倍：内核十几 MB、DLL 157 KB）。
    pub space_hint: Option<&'static str>,
    /// 失败话术末尾那句「临时绕过」。
    ///
    /// ⚠️ `None` 的语义是**这件东西没有旁路可走**（不是"忘了填"）：WebView2Loader.dll
    ///    只能从缓存目录被装载，没有"放到程序旁边也行"这一条 —— 写一句假的旁路会让用户
    ///    照做之后拿到同一个错（W-2：补救必须真的走得通）。
    pub escape: Option<&'static str>,
}

/// **内核 exe** 的释放规格（`shell-win/src/embed.rs` 用）。
///
/// ⚠️ 字符串里的每一句都是**逐字搬过来的**（参数化之前它们硬编码在下面那几个函数里）：
///    这条规矩是为了让这次重构在话术上**几乎零改动** —— 那些话是复审判过的。
/// ⚠️ **一处例外，如实记账**：[`cache_dir_from`] 的补救句原先写的是
///    "把 benagen-core.exe 放到本程序所在目录，或设环境变量 BENAGEN_CORE 指向它，
///    然后点「重试」。"，参数化之后它**复用下面 `escape` 这一句**（括号形式，
///    语义相同、标点略有不同）—— 为了三个字点再添一个只为标点存在的字段不成比例。
///    那一句上的两条断言（`补救` / `BENAGEN_CORE`）一个字没动。
pub const BENAGEN_CORE: ReleaseSpec = ReleaseSpec {
    prefix: "benagen-core",
    suffix: ".exe",
    subject: "内嵌内核",
    noun: "内核",
    space_hint: Some("本程序需要大约与内嵌内核同样大的空闲空间：二十 MB 上下。"),
    escape: Some(
        "把 benagen-core.exe 放到本程序所在目录（或设环境变量 BENAGEN_CORE 指向它）后点「重试」。",
    ),
};

/// 内容哈希取前多少位当文件名的一部分（与内核那边释放 aria2c 的口径一致：12 位十六进制
/// = 48 位，碰撞概率在"一台机器上共存几个版本的客户端"这个量级上可以忽略）。
const HASH_PREFIX_LEN: usize = 12;

/// 释放出来的文件名：`<前缀>-<sha 前 12 位><后缀>`（例：`benagen-core-0123456789ab.exe`）。
///
/// ⚠️ `sha256_hex` 短于 12 个字符时**不 panic**（它是我们自己写进去的常量，不是外部输入）：
///    退化成"用整串"，名字依然是内容派生的。真正会出问题的是**空串** ⇒
///    那时哈希比较恒不成立、于是每次都重写（功能正确，只是白做功）。
pub fn release_name(spec: &ReleaseSpec, sha256_hex: &str) -> String {
    let take = sha256_hex.len().min(HASH_PREFIX_LEN);
    format!("{}-{}{}", spec.prefix, &sha256_hex[..take], spec.suffix)
}

/// 缓存目录：`<LOCALAPPDATA>\BenagenDownloader\cache`（**两件东西共用同一个**）。
///
/// ⚠️ **吃一个参数而不是直接读环境变量**（控制者裁定 JJ 的形状，与
///    `platform::env_var_name` 同源）：`cfg!(windows)` 在 macOS 上恒为假，
///    "Windows 上该读哪个变量、取不到怎么办"这件事在那条路上**没有任何东西在验**。
///    做成纯函数之后，两支都能在宿主上被真断言（见下面 `mod tests`）。
///
/// ⚠️ **取不到就大声失败，不退到 `%TEMP%`**（W-2）。规格 §7.2 把"释放目标是
///    `%LOCALAPPDATA%` 而不是 `%TEMP%`"写成了一条**针对杀软风险的缓解措施** ——
///    悄悄退到 `%TEMP%` 等于把那句话作废。调用方（`shell-win`）会把这句话原样显示给
///    用户（内核那一档还会另给"手工把内核放到程序旁边"的出路，见 [`ReleaseSpec::escape`]）。
///
/// ⚠️ **它也吃 `spec`**（2026-09-20 参数化）：这句话里点了名"释放的是**什么**"，
///    而**没有旁路**的那件东西（DLL）不能照抄内核那句"把 benagen-core.exe 放到程序旁边"
///    —— 那句对它是假的（W-2：补救必须真的走得通）。
pub fn cache_dir_from(
    spec: &ReleaseSpec,
    local_appdata: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    match local_appdata {
        Some(value) if !value.is_empty() => {
            Ok(PathBuf::from(value).join("BenagenDownloader").join("cache"))
        }
        _ => Err(format!(
            "取不到 %LOCALAPPDATA%，不知道该把{}释放到哪里。\n\
             ⚠ 这里**故意**不退到 %TEMP%：本程序把{}放在 %LOCALAPPDATA% 下是有意的\
             （规格 §7.2 对杀软风险的缓解措施），悄悄换一个地方等于把那句话作废。\n\
             补救：{}",
            spec.subject,
            spec.noun,
            spec.escape.unwrap_or(
                "确认这台机器上的 %LOCALAPPDATA% 是设好的（本程序只能把它放在那里），\
                 然后重新启动；若还不行，把这条原样发给我们。"
            )
        )),
    }
}

/// 释放的**全部机制**：把 `bytes` 落到 `cache_dir` 下，返回落位后的路径。
///
/// `spec` 说清"释放的是什么"（文件名形状 + 话术里指名道姓的那几处，见 [`ReleaseSpec`]）；
/// `expected_sha256` 是"这份字节的 sha256"（生产路径给的是**编译期常量**，
/// 见 `shell-win/src/embed.rs` / `shell-win/src/wv2.rs`；于是运行期不必把十几 MB 的
/// 内嵌字节再哈希一遍）。
///
/// 幂等：**哈希相符就复用**（连 mtime 都不碰）；不符就重写。
pub fn extract_into(
    spec: &ReleaseSpec,
    bytes: &[u8],
    expected_sha256: &str,
    cache_dir: &Path,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| release_error(spec, ReleaseStage::CreateDir, cache_dir, &e))?;

    let path = cache_dir.join(release_name(spec, expected_sha256));

    // ① 已经在场且**逐字节相符** ⇒ 复用（这就是"名字由内容派生"换来的一半好处）。
    if let Ok(existing) = std::fs::read(&path) {
        if crate::sha256::sha256_hex(&existing) == expected_sha256 {
            return Ok(path);
        }
        // 不相符（被替换过 / 上一次写坏过）：**继续往下重写**，绝不使用它。
    }

    // ② 写私有的临时名，再原子换位。
    let tmp = private_tmp(&path);
    if let Err(cause) = write_private(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp); // 收尾：别在缓存目录里留垃圾
        return Err(release_error(spec, ReleaseStage::Write, &tmp, &cause));
    }
    if let Err(cause) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(release_error(spec, ReleaseStage::Rename, &path, &cause));
    }

    // ③ **落地之后再核一遍**。这一条不是多余的健壮性：它是"**被杀软拦了**"唯一能
    //    自己现形的形态 —— 写成功、换位成功，而文件随后被隔离/改动/删除。
    //    （写盘时不 fsync：万一断电留下半截文件，上面的哈希判据下次启动就会重写它。）
    match std::fs::read(&path) {
        Ok(written) if crate::sha256::sha256_hex(&written) == expected_sha256 => Ok(path),
        Ok(_) => Err(format!(
            "释放出来的{}与本程序内嵌的那份**不是一个东西**：{}\n\
             ⚠ 文件已经落盘，内容却被改动了 —— 常见原因是杀毒软件（实时防护）动了它。\n\
             补救：把 {} 整个删掉后重试；若还在，用杀毒软件扫一次这台机器。{}",
            spec.noun,
            path.display(),
            cache_dir.display(),
            escape_suffix(spec)
        )),
        Err(cause) => Err(release_error(spec, ReleaseStage::Verify, &path, &cause)),
    }
}

/// 失败话术末尾那句「临时绕过」（连前面的换行一起给）。
///
/// ⚠️ [`ReleaseSpec::escape`] 是 `None` 时**返回空串**（不是"什么都没说"）：
///    那件东西**没有旁路可走**，硬凑一句假的会比不写更糟（W-2：补救必须真的走得通）。
fn escape_suffix(spec: &ReleaseSpec) -> String {
    match spec.escape {
        Some(text) => format!("\n临时绕过：{text}"),
        None => String::new(),
    }
}

/// 每次写入用自己的**私有**临时名（`<name>.<pid>.<序号>.tmp`）。
///
/// ⚠️ 这是相对内核那套的一处**逐字沿用**（`daemon.rs` 的 `private_tmp`），理由见本文件头：
///    共用 `<name>.tmp` 会让并发写入的后来者以 `ENOENT` 失败。
fn private_tmp(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// 写入一个**可执行**文件。
///
/// ⚠️ unix 上执行位是在**创建时**给定的（不是写完再 `chmod`）：中间那一瞬间不能留下
///    一个"没有执行位、却被另一个进程看见"的文件（同 `daemon.rs` 的 `write_executable`）。
///    Windows 上不需要这一步 —— PE 能不能执行是**文件头**说了算，与文件属性无关。
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

/// 失败发生在哪一步 —— **三档失败的处置不同，所以必须分开报**（W-2 / 规格 §10 的
/// "可执行的补救"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStage {
    /// 建缓存目录。
    CreateDir,
    /// 往临时文件里写字节（磁盘满是这一步）。
    Write,
    /// 把临时文件换到最终名字（被杀软占着/权限不足是这一步）。
    Rename,
    /// 换位之后再读回来核对（**被杀软删掉/改动**是这一步）。
    Verify,
}

impl ReleaseStage {
    /// 这一步在干什么（话术里的前半句）。⚠️ 名词由 `spec` 给（见 [`ReleaseSpec::noun`]）。
    fn what(self, spec: &ReleaseSpec) -> String {
        let noun = spec.noun;
        match self {
            ReleaseStage::CreateDir => "建缓存目录失败".to_string(),
            ReleaseStage::Write => format!("把{noun}写进缓存目录失败"),
            ReleaseStage::Rename => format!("把{noun}换到最终文件名失败"),
            ReleaseStage::Verify => format!("释放完成后再核对{noun}失败"),
        }
    }
}

/// 把一次 IO 失败翻成**一句说得清根因、又给得出下一步**的话（W-2）。
///
/// 三档**必须分开**（规格 §10 要求"可执行的补救"）：磁盘满 / 权限不足 / 被杀软拦截 ——
/// 三者的处置完全不同（清理磁盘 / 换账户或改权限 / 加白名单），合成一句"释放失败"，
/// 用户拿到的是一句谁都对不上号的废话。
///
/// ⚠️ **判据是 Windows 的错误码**（`raw_os_error`），不是 `ErrorKind`：Windows 的
///    `ERROR_DISK_FULL`/`ERROR_VIRUS_INFECTED` 在 Rust 的 `ErrorKind` 里**没有各自的一档**
///    （`ERROR_DISK_FULL` 落到 `Uncategorized`，而 `ErrorKind::StorageFull` 至今是
///     nightly）。拿 `ErrorKind` 去分这三档，结果会是"三档全都落进兜底那一句"。
///    ⇒ 用错误码；`ErrorKind` 只在前者缺席时兜底（非 Windows 宿主、或非 OS 错误）。
///
/// 本机上"磁盘满""被杀软拦"两条**造不出来**（见文件头"验不到的那一半"），
/// 但**分类函数**是纯的 ⇒ 可以用人造的错误码把每一支都断言掉（下面 `mod tests`）。
fn release_error(spec: &ReleaseSpec, stage: ReleaseStage, path: &Path, cause: &std::io::Error) -> String {
    let code = cause.raw_os_error();
    let remedy = match code {
        // ERROR_HANDLE_DISK_FULL (39) / ERROR_DISK_FULL (112)
        // ⚠️ 这一档**带一个由 `spec` 给的括号补充**（两件东西的量级差 60 倍：
        //    内核十几 MB、DLL 157 KB ⇒ 同一句"二十 MB 上下"对后者是假话）。
        Some(39) | Some(112) => match spec.space_hint {
            Some(hint) => format!("缓存目录所在的磁盘满了：清理空间后点「重试」。（{hint}）"),
            None => "缓存目录所在的磁盘满了：清理空间后点「重试」。".to_string(),
        },
        // ERROR_VIRUS_INFECTED (225) / ERROR_VIRUS_DELETED (226)
        Some(225) | Some(226) => "杀毒软件把这次写入拦下来了：把本程序与缓存目录加入白名单后点「重试」。\
             ⚠ 我们**不做任何规避杀软的动作**（那是另一个性质的事，规格 §7.2）：\
             正确做法是让用户把这份程序加进白名单。"
            .to_string(),
        // ERROR_ACCESS_DENIED (5) / ERROR_WRITE_PROTECT (19)
        Some(5) | Some(19) => "权限不足：确认缓存目录可写（不要用受限账户运行本程序）。\
             若杀毒软件开启了「受控文件夹访问」之类，也会表现成这一条。"
            .to_string(),
        // ERROR_SHARING_VIOLATION (32) / ERROR_LOCK_VIOLATION (33)
        Some(32) | Some(33) => "文件被别的进程占着（杀毒软件正在扫描？资源管理器在预览？）：\
             等它松开、或把它关掉，然后点「重试」。"
            .to_string(),
        // 走到这里说明错误码不在上面那张表里，或者压根不是 OS 错误。
        _ => match cause.kind() {
            // 非 Windows 宿主上跑本模块（本仓的测试）时走这一支。
            std::io::ErrorKind::PermissionDenied => "权限不足：确认缓存目录可写。".to_string(),
            // ⚠️ **这一支与下面那一支只差 `stage == Verify`**，而它们的处置完全不同
            //    （"被隔离了"要加白名单，"路径没了"再试一次就行）。**不许把这两支合成
            //    一支** —— 这一条正是 2026-09-20 那次抄写丢掉的两支之一（见文件头）。
            std::io::ErrorKind::NotFound if stage == ReleaseStage::Verify => format!(
                "刚释放出来的{}转眼就不见了：多半是杀毒软件把它隔离了。\
                 把本程序与缓存目录加入白名单后点「重试」。",
                spec.noun
            ),
            std::io::ErrorKind::NotFound => {
                "路径不存在（缓存目录被删了？）：重新点一次「重试」。".to_string()
            }
            _ => "原因见下面那行系统原话。".to_string(),
        },
    };
    format!(
        "释放{}失败：{}。\n路径：{}\n系统原话：{cause}\n补救：{remedy}{}",
        spec.subject,
        stage.what(spec),
        path.display(),
        escape_suffix(spec)
    )
}

#[cfg(test)]
mod tests {
    //! 本模块的每一支都能在**宿主上真跑**（这是它放在 `shell-core` 里的全部理由）。
    //!
    //! ⚠️ 唯独"杀软拦截"那一条**造不出来**（见文件头）：这里能验的是**分类函数**
    //!    （人造错误码 → 那句话长什么样、说的是不是根因），不是"真机上它一定会发生"。

    use super::*;
    use std::io::Error;

    /// 一个用完就删的临时目录（不引 `tempfile`：本 workspace 的依赖纪律）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "benagen-embed-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p).expect("建临时目录");
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 释放名字：**内容派生 + 必须带 `.exe`**（规格 §7.2 第 2 条点名的那一条）。
    ///
    /// 判别力：把 `release_name` 里的 `.exe` 去掉（"照抄内核那套"最可能的写法），
    /// 这一条立刻红 —— 而在真机上它的表现是"内核起不来"，且**本机没有任何东西会红**。
    #[test]
    fn the_release_name_is_content_derived_and_ends_with_exe() {
        let sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let name = release_name(&BENAGEN_CORE, sha);
        assert_eq!(name, "benagen-core-0123456789ab.exe");
        assert!(
            name.ends_with(".exe"),
            "释放出来的内核**必须**带 .exe 后缀（Windows 的 CreateProcess 对无扩展名的 PE \
             是推断不是实测，规格 §7.2 明确'不要赌'）"
        );
        // 不同内容 ⇒ 不同名字（否则两个版本会互相覆盖，而旧的下载引擎可能还在跑）。
        assert_ne!(
            release_name(&BENAGEN_CORE, sha),
            release_name(&BENAGEN_CORE, "ffffffffffffffff")
        );
        // 畸形输入不 panic（它是常量，不是外部数据）；退化成"用整串"。
        assert_eq!(release_name(&BENAGEN_CORE, "abc"), "benagen-core-abc.exe");
        assert_eq!(release_name(&BENAGEN_CORE, ""), "benagen-core-.exe");
    }

    /// 缓存目录：**两支在宿主上都能断言**（控制者裁定 JJ 的形状）。
    #[test]
    fn the_cache_dir_is_local_appdata_and_missing_it_is_loud() {
        let dir = cache_dir_from(
            &BENAGEN_CORE,
            Some(std::ffi::OsString::from(r"C:\Users\x\AppData\Local")),
        )
        .expect("有 LOCALAPPDATA 时必须给出目录");
        assert_eq!(
            dir,
            PathBuf::from(r"C:\Users\x\AppData\Local")
                .join("BenagenDownloader")
                .join("cache"),
            "规格 §8 的表里钉的就是这个位置（与内核释放 aria2c 的目录同一个）"
        );

        for missing in [None, Some(std::ffi::OsString::new())] {
            let why = cache_dir_from(&BENAGEN_CORE, missing)
                .expect_err("取不到就必须失败，不许静默换地方");
            assert!(why.contains("LOCALAPPDATA"), "根因要点名那个变量：{why}");
            assert!(why.contains("%TEMP%"), "要说清为什么**不**退到别处：{why}");
            assert!(why.contains("补救"), "W-2：失败话术必须带可执行的补救：{why}");
        }
    }

    /// **释放一次：文件真的落地、内容逐字节一致、名字由内容派生。**
    #[test]
    fn extraction_writes_the_bytes_under_a_content_derived_name() {
        let dir = TempDir::new("write");
        let bytes = b"\x4d\x5a fake but stable payload".repeat(64);
        let sha = crate::sha256::sha256_hex(&bytes);

        let path = extract_into(&BENAGEN_CORE, &bytes, &sha, dir.path()).expect("释放必须成功");
        assert_eq!(path, dir.path().join(release_name(&BENAGEN_CORE, &sha)));
        assert_eq!(std::fs::read(&path).expect("文件必须在场"), bytes);
        // 临时文件不许留在原地（`rename` 之后它就不存在了）。
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("读目录")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不该留下临时文件：{leftovers:?}");
    }

    /// **哈希相符就复用**（连 mtime 都不碰）—— 这是"名字由内容派生"换来的另一半。
    ///
    /// ⚠️ 判据是 `mtime` 不变，所以两次调用之间要**跨过文件系统的时间粒度**
    ///    （1 秒足够覆盖本仓会跑到的那些文件系统）。
    #[test]
    fn a_second_extraction_reuses_the_file_instead_of_rewriting_it() {
        let dir = TempDir::new("reuse");
        let bytes = b"reuse-me".to_vec();
        let sha = crate::sha256::sha256_hex(&bytes);

        let first = extract_into(&BENAGEN_CORE, &bytes, &sha, dir.path()).expect("第一次释放");
        let mtime_before = std::fs::metadata(&first).expect("stat").modified().ok();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = extract_into(&BENAGEN_CORE, &bytes, &sha, dir.path()).expect("第二次释放");

        assert_eq!(first, second, "名字由内容派生 ⇒ 两次落到同一个路径");
        assert_eq!(
            std::fs::metadata(&second).expect("stat").modified().ok(),
            mtime_before,
            "哈希相符必须**复用**（重写会白费十几 MB 的 I/O，还多一次被杀软盯上的机会）"
        );
    }

    /// **哈希不符就重写**：绝不用来路不明的文件。
    ///
    /// 判别力：把复用判据从"哈希相符"放宽成"文件存在"（一个很自然的简化），这一条立刻红。
    #[test]
    fn a_tampered_file_is_replaced_not_trusted() {
        let dir = TempDir::new("tamper");
        let bytes = b"the real kernel".to_vec();
        let sha = crate::sha256::sha256_hex(&bytes);

        // 先在最终名字上放一个**内容不对**的文件（模拟被替换/上次写坏）。
        let target = dir.path().join(release_name(&BENAGEN_CORE, &sha));
        std::fs::write(&target, b"i am definitely not the kernel").expect("放一个冒牌货");

        let path = extract_into(&BENAGEN_CORE, &bytes, &sha, dir.path()).expect("释放必须成功");
        assert_eq!(path, target);
        assert_eq!(
            std::fs::read(&path).expect("读回"),
            bytes,
            "内容不符的文件必须被**重写**，而不是被当成'已经有了'放行"
        );
    }

    /// **并发释放不会互相踩**（内核那边 `private_tmp` 的原始事故就是踩了这一脚）。
    ///
    /// 这里放真并发（4 条线程同一份字节、同一个目录）：共用 `<name>.tmp` 的写法会让
    /// 后完成者的 `rename` 拿到 `ENOENT`（先完成者把它的 `.tmp` 搬走了）。
    #[test]
    fn concurrent_extractions_all_succeed() {
        let dir = TempDir::new("concurrent");
        let bytes = b"concurrent payload".repeat(1024);
        let sha = crate::sha256::sha256_hex(&bytes);
        let dir_path = dir.path().to_path_buf();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let bytes = bytes.clone();
                let sha = sha.clone();
                let dir = dir_path.clone();
                std::thread::spawn(move || extract_into(&BENAGEN_CORE, &bytes, &sha, &dir))
            })
            .collect();

        for h in handles {
            let path = h
                .join()
                .expect("线程不许 panic")
                .expect("并发释放也必须成功（私有临时名就是为这一条存在的）");
            assert_eq!(std::fs::read(&path).expect("读回"), bytes);
        }
    }

    /// **三档失败各有一句说得清的根因 + 补救，而且彼此不同**（W-2 / 规格 §10）。
    ///
    /// ⚠️ 人造错误码：`Error::from_raw_os_error` 在**任何**宿主上都构造得出这些码，
    ///    所以这条在 macOS 上也是真的在验"分类逻辑"，不是"读代码推出来的"。
    ///    （真机上它们各自会不会发生，只有真 Windows 能判 —— 见文件头。）
    #[test]
    fn the_three_failure_families_say_different_and_actionable_things() {
        let stage = ReleaseStage::Write;
        let path = Path::new(r"C:\Users\x\AppData\Local\BenagenDownloader\cache\benagen-core-x.exe");

        let full = release_error(&BENAGEN_CORE, stage, path, &Error::from_raw_os_error(112));
        let denied = release_error(&BENAGEN_CORE, stage, path, &Error::from_raw_os_error(5));
        let virus = release_error(&BENAGEN_CORE, stage, path, &Error::from_raw_os_error(225));
        let sharing = release_error(&BENAGEN_CORE, stage, path, &Error::from_raw_os_error(32));

        assert!(full.contains("磁盘满"), "112 = ERROR_DISK_FULL：{full}");
        assert!(denied.contains("权限"), "5 = ERROR_ACCESS_DENIED：{denied}");
        assert!(virus.contains("杀毒软件"), "225 = ERROR_VIRUS_INFECTED：{virus}");
        assert!(sharing.contains("占着"), "32 = ERROR_SHARING_VIOLATION：{sharing}");

        // 四句话必须**真的不一样**（合成一句"释放失败"就是这条要防的形态）。
        let all = [&full, &denied, &virus, &sharing];
        for (i, a) in all.iter().enumerate() {
            assert!(a.contains("补救"), "每一档都要给可执行的补救：{a}");
            assert!(
                a.contains("benagen-core.exe") && a.contains("BENAGEN_CORE"),
                "每一档都要给出**临时绕过的那条路**（把内核放到程序旁边）：{a}"
            );
            for (j, b) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "第 {i} 档与第 {j} 档的话不能是同一句");
                }
            }
        }

        // 另一档：换位之后再核对时"文件不见了"（杀软隔离最常见的样子）。
        let vanished = release_error(
            &BENAGEN_CORE,
            ReleaseStage::Verify,
            path,
            &Error::new(std::io::ErrorKind::NotFound, "gone"),
        );
        assert!(vanished.contains("杀毒软件"), "Verify 阶段的不见了多半是隔离：{vanished}");

        // 兜底那一支也要出现在话里（不许吞掉系统原文）。
        let other = release_error(&BENAGEN_CORE, stage, path, &Error::from_raw_os_error(1234));
        assert!(other.contains("1234"), "认不出的错误码也要把原文带出来：{other}");
    }

    /// ⭐ **两支"最容易被抄丢"的分支必须各有一句话、且彼此不同**（2026-09-20 补）。
    ///
    /// 这条用例是**被一次真实的漂移逼出来的**（见文件头「这套机制参数化了」那一段）：
    /// 给 WebView2Loader.dll 抄一份释放实现时，抄件削掉了
    ///   · `Some(32) | Some(33)`（文件被别的进程占着）
    ///   · `NotFound` **且** `stage == Verify`（落地之后被隔离）
    /// 两档，而这两档恰恰是 `.dll` 最可能撞上的（杀软实时防护按着不放 / 事后隔离）。
    /// 而当时**没有任何东西会红**：抄件自带的那条用例只喂了 112/5/225，且 `stage` 恒传
    /// `Write` —— 判别力正好覆盖不到被削掉的那两支。现在重复的那份已经删掉（机制只有
    /// 一份），这条用例补的是"这两支**各自的判别力**"，让它们别再只靠"读起来一样"。
    ///
    /// 判别力（**两个方向都会红**，逐个列明）：
    ///   · 把 `Some(33)` 从那一支里删掉 ⇒ 第 2 条断言红（33 落进兜底）；
    ///   · 把 `stage == Verify` 那个守卫删掉（两支合一）⇒ 第 4 条断言红；
    ///   · 把某一支的话换成另一支的 ⇒ `assert_ne!` 红。
    #[test]
    fn the_sharing_and_the_quarantine_families_are_not_swallowed_by_the_fallback() {
        let path = Path::new(r"C:\Users\x\AppData\Local\BenagenDownloader\cache\benagen-core-x.exe");

        // ① 32 与 33 **是同一档**（表里写的是 `Some(32) | Some(33)`）—— 两个码都要被认出来。
        for code in [32, 33] {
            let why = release_error(
                &BENAGEN_CORE,
                ReleaseStage::Write,
                path,
                &Error::from_raw_os_error(code),
            );
            assert!(
                why.contains("占着"),
                "{code} = ERROR_SHARING/ERROR_LOCK_VIOLATION 必须走「文件被别的进程占着」那一档，\
                 而不是落进兜底的『原因见下面那行系统原话』：{why}"
            );
        }

        // ② **同一句系统原话，落在兜底那一支上就得不到上面那句话** —— 这一半证明①有判别力
        //    （不然"任何码都带占着"这句断言就没有意义）。
        let fallback = release_error(
            &BENAGEN_CORE,
            ReleaseStage::Write,
            path,
            &Error::from_raw_os_error(1234),
        );
        assert!(
            !fallback.contains("占着"),
            "认不出的错误码不该被说成「被占着」：{fallback}"
        );

        // ③ `NotFound` 在 **Verify 阶段**是「刚释放出来就被隔离了」，在别的阶段只是「路径没了」。
        let gone = Error::new(std::io::ErrorKind::NotFound, "gone");
        let quarantined = release_error(&BENAGEN_CORE, ReleaseStage::Verify, path, &gone);
        let missing = release_error(&BENAGEN_CORE, ReleaseStage::Write, path, &gone);
        assert!(
            quarantined.contains("杀毒软件"),
            "Verify + NotFound = 落地之后被隔离（那是这一档的根因）：{quarantined}"
        );
        // ④ 两根支的话**必须不一样**：合起来就是把"该去加白名单"说成"再点一次重试"。
        assert_ne!(
            quarantined, missing,
            "`stage == Verify` 那个判断被去掉了？这两档的处置完全不同\
             （加白名单 vs 再试一次），合成一句就是给用户一句对不上号的话"
        );
        assert!(
            missing.contains("路径不存在"),
            "Write + NotFound 仍然是「路径不存在（缓存目录被删了？）」：{missing}"
        );
    }

    /// `private_tmp`：**私有**（同一目标两次调用不同名）、带 pid、以 `.tmp` 结尾。
    ///
    /// ⚠️ 这验不到"并发就安全"（那是上面那条真并发用例的事），它钉的是形状本身：
    ///    共用 `<name>.tmp` 的写法会在这里红。
    #[test]
    fn the_temporary_name_is_private_and_ends_with_tmp() {
        let target = Path::new("/tmp/cache/benagen-core-0123456789ab.exe");
        let a = private_tmp(target);
        let b = private_tmp(target);
        assert_ne!(a, b, "两次写入必须用不同的临时名（共用一个就会踩出 ENOENT）");
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("benagen-core-0123456789ab.exe."), "{name}");
            assert!(name.ends_with(".tmp"), "{name}");
            assert!(
                name.contains(&format!(".{}.", std::process::id())),
                "临时名里必须有自己的 pid（另一台进程/另一次运行要能区分）：{name}"
            );
            assert_eq!(p.parent(), target.parent(), "临时文件必须与目标同目录");
        }
    }
}
