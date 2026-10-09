//! 「有新版本」提示的**纯逻辑**：解析、比较、拼链接。
//!
//! 网络、线程、落盘都不在这个文件里 —— 那些在 `kernel.rs` 那一侧。
//! 这样切的理由与 `planner.rs` / `view.rs` 同源：**能写出断言的代码一律搬出重的依赖面**。
//!
//! ⚠️ 唯一的例外是 [`UreqFetcher`]（任务 2）：它把"出去问一句最新版是几"收在这里，
//!    但只经 [`Fetcher`] 这个最小接口露面，于是本文件的判据里**只有一条**真发 HTTP。

use std::io::Read;

/// 去掉前导 `v`/`V`，按 `.` 切成**三段纯数字**。任何一条不满足就 `None`。
///
/// ⚠️ **三段，不是四段**：Windows 的**版本资源**是四段式的（`0.2.4.0`），
///    那是 exe 的资源、不是 tag。拿它去比会永远不等。
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let body = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
    let mut it = body.split('.');
    let a = it.next()?.parse::<u64>().ok()?;
    let b = it.next()?.parse::<u64>().ok()?;
    let c = it.next()?.parse::<u64>().ok()?;
    if it.next().is_some() {
        return None; // 多于三段
    }
    Some((a, b, c))
}

/// 逐段**数字**比较。任一侧解析不出来 ⇒ `false`（保守：绝不因为解析失败去骚扰客户）。
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform { MacOs, Windows }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch { Aarch64, X86_64 }

/// 本机平台。
///
/// 用 `cfg!(target_os)`：它按**本产物是为哪个 target 编译的**求值，与它跑在哪台机器上无关。
/// ⚠️ `std::env::consts::OS` 是**同一个东西**（也是编译期常量、不是"此刻在哪台机器上"），
///    换成它**不构成变异体**——真正的平台分化只有"交叉编译的产物在真机上跑"才看得见，
///    而那不是一个单元判据照得到的。
pub fn platform() -> Platform {
    if cfg!(target_os = "macos") { Platform::MacOs } else { Platform::Windows }
}

/// 本机架构。理由（以及 `std::env::consts::ARCH` 与 `cfg!` 等价那件事）同 [`platform`]。
pub fn arch() -> Arch {
    if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 }
}

/// 该平台该架构要下哪个文件。
pub fn asset_name(platform: Platform, arch: Arch) -> &'static str {
    match (platform, arch) {
        (Platform::MacOs, Arch::Aarch64) => "BenagenDownloader-AppleSilicon.dmg",
        (Platform::MacOs, Arch::X86_64) => "BenagenDownloader-Intel.dmg",
        (Platform::Windows, Arch::X86_64) => "BenagenDownloader-Windows-x86_64.exe",
        // 未列举的组合**按平台兜底**，而不是一律退回 macOS 的 dmg。
        // `(Windows, Aarch64)` 是唯一会走到这里的一格（Windows on ARM，真实存在）——
        // 平台这个轴是确定的，架构这个轴在 Windows 上才不确定，所以按确定的那个轴兜：
        //   · Windows 未知架构 ⇒ x86_64 exe（Windows-on-ARM 走模拟，能跑）
        // 反过来（退回 Intel dmg）会给一个 Windows 客户一个根本打不开的磁盘映像。
        (Platform::Windows, _) => {
            eprintln!(
                "benagen-core: 未列举的平台/架构组合 {platform:?}/{arch:?}，退回 x86_64 那份"
            );
            "BenagenDownloader-Windows-x86_64.exe"
        }
    }
}

/// 拼下载链接。**域名与路径是常量**，只有版本段与文件名是变量。
pub fn download_url(version: (u64, u64, u64), file: &str) -> String {
    format!(
        "https://gitee.com/starsyi/benagen-downloader/releases/download/v{}.{}.{}/{}",
        version.0, version.1, version.2, file
    )
}

// ---------------------------------------------------------------------------
// 抓取：本文件里**唯一**发 HTTP 的地方（任务 2）
// ---------------------------------------------------------------------------

/// 一次抓取的超时。规格 §1：**5 秒**。
///
/// ⚠️ 两个超时都要显式写，理由同 `rpc.rs` 那两处：ureq 的 `timeout_connect` 默认
///    **30 秒**，且在连接阶段**优先于**整体超时 —— 只写整体的话，连接阶段实际最多等
///    30 秒，那句"5 秒"就成了一句不成立的话（A4 那一类"声明一个数、实际另一个数"）。
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 响应体上限。这个接口实测约 11 KB，1 MiB 已经很宽松 ——
/// 防的是一个**失控的响应体**，不是正常回复（与 `delivery::MAX_MANIFEST_BYTES` 同一条用意）。
const MAX_BYTES: u64 = 1 << 20;

/// **唯一**的真相源地址。常量，不来自任何输入。
///
/// ⚠️ 它必须走 HTTPS 且不带查询串（规格 §1）：前者让"校验证书"那条成立，
///    后者让请求行里没有任何我们拼上去的参数。
pub const RELEASE_URL: &str =
    "https://gitee.com/api/v5/repos/starsyi/benagen-downloader/releases/latest";

/// 抓取用的最小接口。**为了可测**：判据里绝大多数用例不该真发 HTTP
/// （与 `delivery::Transport` 同一条路子）。
pub trait Fetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// 生产用：`ureq`。**本仓唯一真发这条请求的地方。**
///
/// ⚠️ `agent` 是**结构体字段**、不是每次调用新建：`ureq::Agent` 自带连接池
///    （与 `delivery::UreqTransport` 同一条理由）。
/// ⚠️ 证书校验**保持 ureq 的默认**（开）—— 本文件**没有**、也不该有任何"跳过证书"
///    的开关（规格 §1）。
pub struct UreqFetcher {
    agent: ureq::Agent,
}

impl UreqFetcher {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(FETCH_TIMEOUT)
                .timeout(FETCH_TIMEOUT)
                .build(),
        }
    }
}

// 与 `new()` 成对（clippy 的 `new_without_default`），同 `delivery::UreqTransport`。
impl Default for UreqFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for UreqFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        // 非 2xx 由 ureq 归成 `Error::Status` ⇒ 这里一并变成 `Err`，由 `fetch_latest` 静默掉。
        let resp = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        resp.into_reader()
            .take(MAX_BYTES)
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

/// 取最新版本的 `tag_name`。**任何失败都回 `None`**（规格 §5：失败一律静默）——
/// 网络错、非 200、不是 JSON、解析不出 `tag_name`，全都不是一个"错误"，只是"这次没有新版本可说"。
///
/// 只读 `tag_name` 与 `prerelease` 两个字段；`body` / `assets` / `author` 等
/// **读进来就丢**，不进界面、不落盘。返回的是 `tag_name` 原文，是否"更新"由
/// [`is_newer`] 用**解析后**的三段数字判。
pub fn fetch_latest(fetch: &dyn Fetcher) -> Option<String> {
    let raw = fetch.get(RELEASE_URL).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    if v.get("prerelease").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    Some(v.get("tag_name")?.as_str()?.to_string())
}

// ---------------------------------------------------------------------------
// 状态文件与节流（任务 3）
// ---------------------------------------------------------------------------

// ⚠️ `use` 放在这里、不放在文件顶：文件顶那一行会让任务 1/2 的**行号整体下移**，
//    而 `unreachable_patterns` 那条既有告警（任务 1 留的）在别处是按 `update.rs:68`
//    被引用的。放在本段开头，行号不动。
use std::path::Path;

const DAY_SECS: i64 = 24 * 60 * 60;

/// 内核自己存的运行状态。**与 `last_code` 同层**：那是运行状态，不是下载参数。
///
/// ⚠️ **为什么不塞进 `settings.json`**：那七项是**下载参数**，面板上一行一项、
///    每项都有 1–16 / 1–64 的区间校验。塞进去会让那个面板多出一个语义完全不同的项。
///
/// ⚠️ `#[serde(default)]` 是**承重的**：将来加字段时老文件读得进来，而不是整份作废
///    （没有它，客户那份旧 `update.json` 会让整份状态作废、回不到默认值以外）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    pub enabled: bool,
    /// 上次成功检查的时刻，**Unix 秒**（`0` = 从没查过）。
    pub last_checked_at: i64,
    pub last_seen: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        // 开关**默认开**（规格 §4）
        Self { enabled: true, last_checked_at: 0, last_seen: None }
    }
}

pub fn load_state(path: &Path) -> State {
    let Ok(raw) = std::fs::read(path) else { return State::default() };
    serde_json::from_slice::<State>(&raw).unwrap_or_default()
}

pub fn save_state(path: &Path, s: &State) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, serde_json::to_vec(s).unwrap_or_default())
}

/// 该不该自动查一次（纯函数，把节拍判据从线程里择出来，好在宿主上真测）。
///
/// ⚠️ `last_checked_at` 与 `now` 都是**Unix 秒**（同一个单位，相减才成立；消费方别传毫秒）。
pub fn should_check(enabled: bool, last_checked_at: i64, now: i64) -> bool {
    if !enabled {
        return false;
    }
    let since = now - last_checked_at;
    // ⚠️ `last_checked_at == 0` 是"从没查过"，`since` 会是个巨大的数 ⇒ 天然落进 `>= DAY`
    since >= DAY_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析：三段纯数字才认，别的一律 `None`。
    #[test]
    fn parse_version_is_a_table() {
        assert_eq!(parse_version("v0.2.4"), Some((0, 2, 4)));
        assert_eq!(parse_version("V0.2.4"), Some((0, 2, 4)));
        assert_eq!(parse_version("0.2.4"), Some((0, 2, 4)));
        assert_eq!(parse_version("v10.20.30"), Some((10, 20, 30)));
        // 下面这些**必须**是 None —— 每一条都对应一种真会出现的畸形
        assert_eq!(parse_version("0.2"), None, "两段");
        assert_eq!(parse_version("0.2.4.0"), None, "四段（那是 Windows 的版本资源，不是 tag）");
        assert_eq!(parse_version("v0.2.4-beta"), None, "带预发布后缀");
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("v"), None);
        assert_eq!(parse_version("v1.2.x"), None);
    }

    /// 比较是**数字**比，不是字符串比。
    #[test]
    fn is_newer_compares_numerically() {
        assert!(is_newer("v0.2.10", "v0.2.9"), "字符串比会判反，这一条专抓它");
        assert!(is_newer("v0.3.0", "v0.2.99"));
        assert!(!is_newer("v0.2.4", "v0.2.4"), "相等不是更新");
        assert!(!is_newer("v0.2.3", "v0.2.4"), "数据比装着的旧");
        // 解析不出来 ⇒ 一律 false
        assert!(!is_newer("garbage", "v0.2.4"));
        assert!(!is_newer("v0.2.5", "garbage"), "客户版本畸形时也不提示");
    }

    /// 链接的域名与路径是常量，版本段是**拼回**的三个数。
    #[test]
    fn the_url_is_built_not_forwarded() {
        assert_eq!(
            download_url((0, 2, 5), "BenagenDownloader-Intel.dmg"),
            "https://gitee.com/starsyi/benagen-downloader/releases/download/v0.2.5/BenagenDownloader-Intel.dmg"
        );
        // ⚠️ 这一条是安全判据：原文里带路径分隔符时，**解析那一关就该把它挡掉**，
        //    于是根本走不到拼链接这一步。这里直接验"解析挡得住"。
        assert_eq!(parse_version("v1.2.3/../../evil"), None, "路径注入必须被解析挡下");
    }

    /// 文件名按平台与架构选。
    #[test]
    fn asset_names_are_a_table() {
        assert_eq!(asset_name(Platform::MacOs, Arch::Aarch64), "BenagenDownloader-AppleSilicon.dmg");
        assert_eq!(asset_name(Platform::MacOs, Arch::X86_64), "BenagenDownloader-Intel.dmg");
        assert_eq!(asset_name(Platform::Windows, Arch::X86_64), "BenagenDownloader-Windows-x86_64.exe");
        // 未列举的 `(Windows, Aarch64)`（Windows on ARM）：按**平台**兜，要给 Windows 那份，
        // 不能退回 macOS 的 dmg —— 后者在 Windows 上根本打不开。
        assert_eq!(asset_name(Platform::Windows, Arch::Aarch64), "BenagenDownloader-Windows-x86_64.exe");
    }

    /// `platform()` 的取值必须与**编译期**的 `cfg!(target_os)` 一致。
    ///
    /// **它抓得到**：① 把 `if/else` 两支对调；② 把来源换成**真·运行期**的东西
    /// （`std::env::var("OS")`、外出跑 `uname`）—— 那种实现在宿主上就会红。
    /// **它抓不到**：把 `cfg!` 换成 `std::env::consts::OS` —— 两者**完全等价**
    /// （都是编译期常量，含义是"本产物为哪个 target 编译"，与跑在哪台机器上无关），
    /// 换过去这条依旧绿。真正的平台分化只有"交叉编译的产物在真机上跑"才看得见，
    /// 那不是单元判据照得到的。
    #[test]
    fn platform_follows_cfg_target_os() {
        let expected = if cfg!(target_os = "macos") { Platform::MacOs } else { Platform::Windows };
        assert_eq!(platform(), expected);
    }

    /// `arch()` 的取值必须与**编译期**的 `cfg!(target_arch)` 一致。
    ///
    /// **它抓得到 / 抓不到**同 [`platform_follows_cfg_target_os`]：抓到"两支对调"与
    /// "换成真·运行期来源"；抓不到"换成 `std::env::consts::ARCH`"（与 `cfg!` 等价）。
    #[test]
    fn arch_follows_cfg_target_arch() {
        let expected = if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 };
        assert_eq!(arch(), expected);
    }

    // -----------------------------------------------------------------------
    // 任务 2：抓取（本文唯一的网络判据都在这里）
    // -----------------------------------------------------------------------

    /// 测试用假传输：回一个固定结果，并**记下被问的 URL**。
    ///
    /// ⚠️ 它是**测试脚手架**、不进生产代码 —— 生产侧 `Fetcher` 的实现只有 [`UreqFetcher`]
    ///    一个。顺手记下 URL，把"`fetch_latest` 只可能问 [`RELEASE_URL`]"变成一条**可观测量**
    ///    （比比较字面量强：它证明的是**这条函数问出去的是哪一条地址**）。
    struct StubFetcher {
        result: Result<Vec<u8>, String>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl StubFetcher {
        fn ok(body: &[u8]) -> Self {
            Self {
                result: Ok(body.to_vec()),
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn err(msg: &str) -> Self {
            Self {
                result: Err(msg.to_string()),
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        /// 被问过的 URL，按顺序。
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Fetcher for StubFetcher {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.asked.lock().unwrap().push(url.to_string());
            self.result.clone()
        }
    }

    /// 从真实形状的响应里只取 `tag_name`；`prerelease == true` ⇒ `None`。
    ///
    /// ⚠️ 这里的响应体用 `r#"…"#`（**普通**原始串）+ `.as_bytes()`，不用 `br#"…"#`：
    ///    原始**字节**串字面量只认 ASCII，而真实响应里的 `body`（发行说明）就是中文 ——
    ///    保留一段多字节内容正是在验"`body` 读进来就丢"这件事（乱码式截断会在这里现形）。
    #[test]
    fn fetch_latest_reads_tag_name_and_honours_prerelease() {
        let body = r#"{"tag_name":"v0.2.5","prerelease":false,
                        "body":"很长很长的发行说明","assets":[{"name":"x"}],
                        "author":{"login":"starsyi"}}"#;
        assert_eq!(fetch_latest(&StubFetcher::ok(body.as_bytes())), Some("v0.2.5".to_string()));

        let pre = br#"{"tag_name":"v0.3.0-rc1","prerelease":true}"#;
        assert_eq!(fetch_latest(&StubFetcher::ok(pre)), None, "预发布版不提示");

        // 解析不出 tag_name ⇒ None（**不是**错误）
        assert_eq!(fetch_latest(&StubFetcher::ok(br#"{"name":"x"}"#)), None);
    }

    /// 失败一律静默：网络错、非 200、不是 JSON、解析不出 —— 全都回 `None`。
    ///
    /// ⚠️ 中间那条 `b"<html>502</html>"` **不是真的 502 响应**，是一个**状态码 200** 的假 502 体
    ///    （`StubFetcher::ok` 走"取体成功"那一支）⇒ 它落在"**不是 JSON**"这一支，不是状态码那一支。
    #[test]
    fn every_failure_is_silent() {
        assert_eq!(fetch_latest(&StubFetcher::err("连接失败")), None);
        assert_eq!(fetch_latest(&StubFetcher::ok(b"<html>502</html>")), None);
        assert_eq!(fetch_latest(&StubFetcher::ok(b"")), None);
    }

    /// 🔴 `fetch_latest` 问的**恰好是** [`RELEASE_URL`] —— 地址是常量，不来自任何输入。
    ///
    /// 这比"比较字面量"强：它证明的是**这条函数问出去的地址**就是那一个。
    #[test]
    fn the_release_url_is_a_constant() {
        let f = StubFetcher::ok(br#"{"tag_name":"v0.2.5","prerelease":false}"#);
        let _ = fetch_latest(&f);
        assert_eq!(f.asked(), vec![RELEASE_URL.to_string()], "抓取只能问这一个地址");
        // 协议也在常量里钉住：一个 `http://` 会让"必须校验 HTTPS 证书"那条落空。
        assert!(RELEASE_URL.starts_with("https://"), "必须走 HTTPS：{RELEASE_URL}");
        assert!(!RELEASE_URL.contains('?'), "请求行不许带参数串：{RELEASE_URL}");
    }

    /// 🔴 **客户端什么都不发**：请求行没有 `?`、没有请求体、没有自定义头。
    ///
    /// ⚠️ 这一条打的是**真的 HTTP 桩**（`crate::testutil::StubHttp`），不是 `StubFetcher` ——
    ///    要验的正是"线上那条请求长什么样"，用假传输等于没验。
    ///    这里直接调 `UreqFetcher::new().get(&url)`：`Fetcher::get` 本来就收 URL，
    ///    **不需要为测试在生产代码里开洞**（`new_for_test` 那一类构造函数不存在）。
    #[test]
    fn the_request_sends_nothing() {
        let srv = crate::testutil::StubHttp::start_with(|_req| {
            crate::testutil::RawResponse::new(200)
                .body(r#"{"tag_name":"v0.2.5","prerelease":false}"#.to_string())
        });
        let url = format!("{}/repos/x/y/releases/latest", srv.base());
        let got = UreqFetcher::new().get(&url);
        assert!(got.is_ok(), "桩回了 200，取体不该失败：{got:?}");

        let reqs = srv.requests();
        assert_eq!(reqs.len(), 1, "只该有一次请求");
        let r = &reqs[0];
        assert_eq!(r.method, "GET", "请求行的方法就是 GET");
        assert!(!r.path.contains('?'), "不许带参数串：{}", r.path);
        assert!(r.body.is_empty(), "不许带请求体：{}", r.body);
        let lower = r.path.to_ascii_lowercase();
        assert!(!lower.contains("token"), "不许带凭据");

        // 🔴 头是这一条的重点（规格 §6.3 的判据就是"请求头里没有 Authorization"）。
        //    先证"头**确实被记下来了**"—— 一个头都没记到的话，下面那个循环会空转，
        //    这条判据就等于没写（`SeenRequest::headers` 是它唯一的载体）。
        let names: Vec<String> =
            r.headers.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
        assert!(
            names.iter().any(|n| n == "host"),
            "该记到 Host 头；一个头都没有说明断言在空转：{names:?}"
        );
        // `host` / `user-agent` / `accept*` 是 HTTP 库自己的默认头，**放行**：
        // 规格 §1 说"唯一出站的头是 HTTP 库自己的默认 UA —— 那是库的行为，不是我们加的身份"。
        for name in &names {
            assert_ne!(name, "authorization", "不许带 Authorization");
            assert_ne!(name, "cookie", "不许带 Cookie");
            assert!(!name.starts_with("x-"), "不许带任何 X- 自定义头：{name}");
        }
    }

    // -----------------------------------------------------------------------
    // 任务 3：状态文件与 24 小时节流
    // -----------------------------------------------------------------------

    /// 节流判据（纯函数）。**这一条守的是"一天开 30 次不会查 30 次"。**
    #[test]
    fn should_check_is_a_table() {
        const DAY: i64 = 24 * 60 * 60;
        // 开关关掉：永远不查
        assert!(!should_check(false, 0, 1_000_000));
        // 首次运行（从没查过）：立刻查
        assert!(should_check(true, 0, 1_000_000));
        // 刚查过：不查
        assert!(!should_check(true, 1_000_000, 1_000_000));
        // 差 23 小时：不查
        assert!(!should_check(true, 1_000_000, 1_000_000 + DAY - 1));
        // 差满 24 小时：查
        assert!(should_check(true, 1_000_000, 1_000_000 + DAY));
        // ⚠️ **时钟回拨**：差值为负 ⇒ 不查（宁可不查，也不要因为时钟跳了就狂查）
        assert!(!should_check(true, 2_000_000, 1_000_000));
    }

    /// 状态文件：没写过 ⇒ 默认（开关开、从没查过）；写坏了 ⇒ 同样回默认，**不当错误**。
    #[test]
    fn state_file_degrades_instead_of_failing() {
        let dir = crate::testutil::TempDir::new();
        let p = dir.join("update.json");
        // 文件不存在
        let s = load_state(&p);
        assert!(s.enabled && s.last_checked_at == 0 && s.last_seen.is_none());
        // 写坏了
        std::fs::write(&p, "{不是 json").unwrap();
        let s = load_state(&p);
        assert!(s.enabled, "坏文件不该把开关关掉");
        // 往返
        save_state(&p, &State { enabled: false, last_checked_at: 42, last_seen: Some("v0.2.5".into()) }).unwrap();
        let s = load_state(&p);
        assert!(!s.enabled && s.last_checked_at == 42 && s.last_seen.as_deref() == Some("v0.2.5"));
    }

    /// 🔴 **`#[serde(default)]` 是承重的**：老文件缺了新加的字段，剩下那些字段**照样读得进来**，
    /// 而不是整份作废、悄悄回默认。
    ///
    /// ⚠️ 这一条**不能**用"缺 `last_seen`"来写：`Option<T>` 字段缺了本来就默认 `None`，
    ///    去掉 `#[serde(default)]` 它照样绿 —— 那样等于没测到那个属性。
    ///    所以这里缺的是一个**非 `Option`** 字段（`last_checked_at: i64`）：
    ///    没有 `#[serde(default)]` 时反序列化会整体失败，`load_state` 落回 `State::default()`
    ///    ⇒ `enabled` 会是 `true`。断言 `!s.enabled` 正好把两支分开。
    #[test]
    fn old_state_files_survive_a_missing_field() {
        let dir = crate::testutil::TempDir::new();
        let p = dir.join("update.json");
        std::fs::write(&p, br#"{"enabled":false}"#).unwrap();
        let s = load_state(&p);
        assert!(!s.enabled, "缺字段不该让整份状态作废（`#[serde(default)]` 的承重点）");
        assert_eq!(s.last_checked_at, 0, "缺的字段取 `Default` 的值");
        assert_eq!(s.last_seen, None);
    }
}
