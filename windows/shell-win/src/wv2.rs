//! `wv2` —— **Windows GNU 靶的 WebView2Loader 补丁面**（规格 §4）。
//!
//! ## 为什么需要它（一句话版；完整根因见探路报告 §3）
//!
//! 交付形态是**单个 exe**（W-4）。`webview2-com-sys` 只对 MSVC 目标静态链接那 5 个
//! WebView2 loader 函数，对**非 MSVC**（我们的 `x86_64-pc-windows-gnu`）发的是
//! `#[link(name = "WebView2Loader.dll")]` ⇒ 链成**动态导入** ⇒ exe 必须和
//! `WebView2Loader.dll`（157 KB）一起发，单文件交付当场不成立。
//!
//! ⇒ `windows/vendor/webview2-com-sys` 里那份补丁版把那条 `#[link]` **删掉了**
//! （于是那 5 个符号变成"未定义的普通符号"），由**本模块**给出它们的定义：
//! DLL 内嵌在 exe 里 → 启动时释放到缓存目录 → `LoadLibraryW` 装载 → `GetProcAddress`
//! 把 5 个符号解析成函数指针 → 5 个 `#[no_mangle]` 的转发函数把调用转过去。
//!
//! ⚠️ **这一整套只在 `not(msvc)` 的 Windows 靶上存在**；MSVC 靶走 crate 原版的静态链接
//! （那时本模块**一个符号都不许定义**，否则会与静态库里的定义撞名）。
//! 宿主（macOS）靶更没有这一套 —— 那里是 WKWebView，压根没有 WebView2 这回事
//! （规格 §9.0：宿主跑的是同一个壳，但平台专属落点走各自的 `#[cfg]` 分支）。
//!
//! ## ⚠️⚠️ 这是本代**唯一从未运行过**的代码（规格 §9.2）
//!
//! 本机没有 Wine、没有虚拟机 ⇒ 这 5 个转发函数的**名字与 ABI 只有真 Windows 能验**
//! （签名是从 `vendor/webview2-com-sys/src/bindings.rs` 的 `link!` 声明**逐字**抄的）。
//! ⇒ **第一次在真 Windows 上跑必然发现新东西，那不是回归。**
//! 也正因为如此，那 5 个函数**逐个手写、不用宏**：宏里的 `concat!(stringify!())`
//! 一旦在 ABI 上对不齐（`windows_core::PCWSTR` 是个 `#[repr(transparent)]` 的新类型），
//! 根因会被埋进宏展开里 —— 而这份代码的全部价值就是"出了问题能一眼读出来"。
//!
//! ## 释放那一套：**用 `shell_core::embedded_core` 的那一份，本模块不再自带**（2026-09-20 修）
//!
//! ⚠️ 本模块**一度自带过一份**释放实现（哈希派生名字 + 复用 + `.tmp` + `rename` +
//!    落地后再核对），理由是当时那套机制把前缀/后缀写死成 `benagen-core` / `.exe`
//!    （而并行的工作流占着 `shell-core`，不许改）。
//!    那份抄件**在几小时之内就漂了**：抄的时候削掉了 `Some(32) | Some(33)`（文件被
//!    占着）与 `Verify + NotFound`（落地后被杀软隔离）两档失败 —— 而那两档恰恰是
//!    `.dll` **最**可能撞上的（实时防护按着不放 / 事后隔离），也正是本模块的文件头把
//!    "被杀软拦了"点名成"唯一能自己现形的失败形态"的那一支。
//! ⇒ 那套机制已经**参数化**（`shell_core::embedded_core::ReleaseSpec`），本模块只提供
//!    "**释放的是什么**"（[`LOADER`]），机制、分档与话术全在 `shell-core` 那一份里
//!    （那里有一整套宿主单测，含专门为这两支补的用例）。**别在这里再抄一份。**

// ⚠️ `all(windows, not(target_env = "msvc"))` **是承重的**（不是"顺手加个 windows 就行"）：
//    MSVC 靶上这 5 个符号由 crate 原版的静态库提供，我们**再定义一份就是撞名**
//    （链接期 duplicate symbol，而本仓库根本没有 MSVC 的构建路径 ⇒ 那种坏法当场发现不了）。
#[cfg(all(windows, not(target_env = "msvc")))]
use std::ffi::c_void;

// ⚠️ 只在 `LOADER` 存在的地方需要（它自己的 `#[cfg]` 就在下面几行）。
#[cfg(any(all(windows, not(target_env = "msvc")), test))]
use shell_core::embedded_core::ReleaseSpec;

/// 内嵌的 `WebView2Loader.dll`。x64 一份 —— 本代只支持 x86_64（规格 §4.2）。
///
/// ⚠️ 路径是 **`OUT_DIR`**，由本 crate 的 `build.rs` 从
/// `vendor/webview2-com-sys/x64/WebView2Loader.dll` 拷进来（**并断言它的 sha256**）
/// —— 与内核 exe 同形。
/// 直接用 `include_bytes!("../vendor/…")` 会**绕过那条 sha256 断言**，
/// 而那条断言正是"这份 DLL 还是不是我们审过的那一份"的唯一判据。
#[cfg(all(windows, not(target_env = "msvc")))]
pub const LOADER_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/WebView2Loader.dll"));

/// 上面那份字节的 sha256 —— **由 `build.rs` 在核对那一刻交给编译期**（`cargo:rustc-env`）。
///
/// ⚠️ 运行期要它来拼释放文件名（`WebView2Loader-<前 12 位>.dll`）。
///    不在这里现算的理由与内核那一份相同：**编译期常量与构建期核对过的是同一个值**，
///    现算一次就多出一条"哪一份被核过"的分叉。
#[cfg(all(windows, not(target_env = "msvc")))]
pub const LOADER_SHA256: &str = env!("BENAGEN_WV2_LOADER_SHA256");

/// 释放并装载 DLL。**必须在 Tauri 起来之前调用**，失败要给**人看得见**的话（规格 §4.2）。
///
/// 做三件事，任何一步失败都返回一句**能直接显示给人看**的话：
///   1. 把内嵌的字节释放到 `%LOCALAPPDATA%\BenagenDownloader\cache\`（与内核 exe 同一个目录）；
///   2. `LoadLibraryW` 装载它（**绝对路径**）；
///   3. 把 5 个符号逐个 `GetProcAddress` 出来存进各自的 `OnceLock` —— **取不到就指名道姓**。
///
/// ⚠️ **为什么要在 Tauri 起来之前**：这 5 个符号是 Tauri 的 webview 后端（wry）在
///    `tao` 建窗口的过程中调的。等它调的时候才发现 DLL 没装载上，那就是一次
///    **没有任何界面可说的崩溃**（交付形态没有控制台，见 `main.rs` 的落点①）。
///    ⇒ 先做、先报，把失败变成一句人话（规格 §4.2："不许静默 assert 崩掉"）。
///
/// ⚠️ **本函数不缓存失败**：它只在进程启动时被调一次（`main.rs` 第一步），
///    成功之后 5 个 `OnceLock` 就永远是满的。**没有重试语义**（重试需要重启进程）。
#[cfg(all(windows, not(target_env = "msvc")))]
pub fn install() -> Result<(), String> {
    gnu::install()
}

/// 宿主（macOS）靶与 MSVC 靶：**没有这一套**（模块头那两段说了为什么）。
///
/// ⚠️ 返回 `Ok(())` **不是"静默降级"**（W-2 的那条纪律针对的是**交付形态**）：
///   · 宿主靶压根没有 WebView2（宿主是 WKWebView），这里的 `Ok` 是**事实**；
///   · MSVC 靶由 crate 原版的静态链接兜着，同样没有事要办。
///   而**交付形态那一条（`not(msvc)` 的 Windows 靶）没有开关**：上面那个 `install`
///   一定做那三件事，任何一步失败都返回 `Err`。
///
/// ⚠️ 它**不**用 `#[cfg]` 把调用点也关掉：`main.rs` 里那一步必须是**无条件**的一行
///   （"仅 Windows 靶"这件事由一个函数的两支实现表达，而不是散在调用点）。
#[cfg(not(all(windows, not(target_env = "msvc"))))]
pub fn install() -> Result<(), String> {
    Ok(())
}

/// **WebView2Loader.dll 的释放规格** —— `shell_core::embedded_core` 的那套机制吃它。
///
/// ⚠️ 它住在**本模块**、不在 `shell-core`：那一层是**跨平台纯逻辑**，"WebView2" 是
///    Windows 专有的概念，它不该知道有这个 DLL（内核那份 `BENAGEN_CORE` 留在
///    `shell-core`，是因为它描述的就是那边释放的东西）。
///
/// ⚠️ `escape: None` 是**语义**，不是"忘了填"：这个 DLL **只能**从缓存目录被装载
///    （`LoadLibraryW` 拿的就是那条绝对路径），没有"把它放到程序旁边也行"这条旁路
///    —— 照抄内核那句会让用户白试一次（W-2：补救必须真的走得通）。
///    同理 `space_hint: None`：157 KB 的东西不该在"磁盘满"那句里报一个 MB 级的数。
///
/// **为什么带 `test`**：宿主（非 Windows）靶上没有东西读它（`install` 那一支被
/// `#[cfg]` 掉了），而下面那条钉住"释放文件名长什么样"的用例**必须在宿主上跑得到**
/// —— 那正是本代最容易"看着对"的一处（照抄内核那套就会得到 `.exe`）。
/// 加上 `test` 之后它在**宿主测试构建**里存在（由用例真的读到），而在宿主 / MSVC 的
/// **非测试**构建里根本不存在（不留一条没人读的常量，也不需要靠 `pub` 把告警按平）。
#[cfg(any(all(windows, not(target_env = "msvc")), test))]
const LOADER: ReleaseSpec = ReleaseSpec {
    prefix: "WebView2Loader",
    suffix: ".dll",
    subject: "内嵌的 WebView2Loader.dll",
    noun: "WebView2Loader.dll",
    space_hint: None,
    escape: None,
};

#[cfg(test)]
mod tests {
    //! 宿主上**真跑**的那两条（`LOADER` 为什么带 `test`，见它自己的文档）。
    //!
    //! ⚠️ 机制本身的判据**不在这里**：释放的幂等、复用/重写、并发、失败分档全都在
    //!    `shell_core::embedded_core` 的 `mod tests` 里（机制只有那一份）。
    //!    这里只剩两件**只属于这个 DLL** 的事：名字里的 `.dll`，以及
    //!    "失败话术说的是 DLL 而不是内核、而且不编一条假的旁路"。

    use super::LOADER;
    use std::path::{Path, PathBuf};

    /// 一个用完就删的临时目录（不引 `tempfile`：本 workspace 的依赖纪律）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "benagen-wv2-{tag}-{}-{:?}",
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

    /// 释放名字：**内容派生 + 必须带 `.dll`**。
    ///
    /// 判别力：把 [`LOADER`] 的 `suffix` 改成 `.exe`（"照抄内核那套"最可能的写法），
    /// 这一条立刻红 —— 而在真机上它的表现是"WebView2 起不来"，
    /// 且**本机没有任何东西会红**（`LoadLibraryW` 那条路只有真 Windows 跑得到）。
    #[test]
    fn the_release_name_is_content_derived_and_ends_with_dll() {
        let sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let name = shell_core::embedded_core::release_name(&LOADER, sha);
        assert_eq!(name, "WebView2Loader-0123456789ab.dll");
        assert!(
            name.ends_with(".dll"),
            "释放出来的模块**必须**带 .dll 后缀（LoadLibraryW 对无扩展名的推断\
             是推断不是实测，与内核那条「不要赌」同源）"
        );
        // 不同内容 ⇒ 不同名字（否则两个版本会互相覆盖）。
        assert_ne!(
            name,
            shell_core::embedded_core::release_name(&LOADER, "ffffffffffffffff")
        );
        // 畸形输入不 panic（它是常量，不是外部数据）；退化成"用整串"。
        assert_eq!(
            shell_core::embedded_core::release_name(&LOADER, "abc"),
            "WebView2Loader-abc.dll"
        );
        assert_eq!(
            shell_core::embedded_core::release_name(&LOADER, ""),
            "WebView2Loader-.dll"
        );
    }

    /// ⭐ **这个 DLL 的失败话术说的是 DLL、而且不编一条假的旁路。**
    ///
    /// 判别力（**如实记账，别把话说满**）：它钉的是 **[`LOADER`] 这份规格本身** ——
    /// 名字是 DLL、话术说的是 DLL、且**没有**那条"放到程序旁边"的假旁路。
    /// 把 `LOADER` 的任何一个字段照抄成内核那份（`noun: "内核"`、`escape: Some(…)`、
    /// `suffix: ".exe"`），这一条立刻红。
    ///
    /// ⚠️ 它**钉不住**"`install()` 里传的是哪一份规格"：那一句在
    ///    `#[cfg(all(windows, not(target_env = "msvc")))]` 里，宿主上根本编不到
    ///    —— 那一层只有真 Windows 能判（与规格 §9.2 那张"没被证明的事"表同性质）。
    ///
    /// ⚠️ 失败是**造出来的**：让缓存目录的祖先是一个**普通文件** ⇒ `create_dir_all`
    ///    必然失败（`ENOTDIR`）。这条在宿主上真跑，不是"读代码推出来的"。
    #[test]
    fn the_dll_release_failure_names_the_dll_and_offers_no_fake_escape() {
        let dir = TempDir::new("fail");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a directory").expect("造一个挡住路径的文件");

        let why = shell_core::embedded_core::extract_into(
            &LOADER,
            b"whatever",
            &"0".repeat(64),
            &blocker.join("cache"),
        )
        .expect_err("缓存目录建不出来 ⇒ 释放必须失败");

        assert!(
            why.contains("WebView2Loader.dll"),
            "话术要点名被释放的那件东西：{why}"
        );
        assert!(
            !why.contains("内核") && !why.contains("benagen-core"),
            "把内核那件东西的话术套到 DLL 上了（规格传错了？）：{why}"
        );
        assert!(
            !why.contains("临时绕过"),
            "这个 DLL 没有旁路可走 —— 不许给一句用户照做也没用的补救：{why}"
        );
        assert!(why.contains("补救"), "W-2：失败话术必须带可执行的补救：{why}");
    }
}

/// **Windows `not(msvc)` 靶**那一层：装载 DLL + 解析 5 个符号 + 那 5 个转发函数。
///
/// ⚠️ 整个模块只在那个靶上存在（理由见文件头：MSVC 由 crate 原版的静态库兜着，
/// 宿主压根没有 WebView2）。
#[cfg(all(windows, not(target_env = "msvc")))]
mod gnu {
    use super::{c_void, LOADER_DLL, LOADER_SHA256};
    use std::sync::OnceLock;

    /// `HRESULT`。⚠️ 与 `windows_core::HRESULT`（`#[repr(transparent)]` 包着 `i32`）
    /// **ABI 逐位相同** —— 这里不引 `windows-core` 是因为它只是那个 crate 的实现细节，
    /// 而本 crate 的直接依赖清单里没有它（加一个传递依赖当直接依赖要单独论证）。
    type Hresult = i32;

    /// `PCWSTR` / `PWSTR` 同理：前者是 `#[repr(transparent)]` 包着 `*const u16`、
    /// 后者包着 `*mut u16`。⇒ 下面一律写裸指针，**ABI 与 crate 里那份声明逐位相同**
    /// （出处：`vendor/webview2-com-sys/src/bindings.rs` 的 5 条 `link!` 声明）。
    type Pcwstr = *const u16;
    /// `*mut PWSTR` ⇒ `*mut *mut u16`（`versioninfo` 那个出参）。
    type VersionInfoOut = *mut *mut u16;

    // -----------------------------------------------------------------------
    // 5 个符号的签名 —— **逐个手写**，一个宏都不用（理由见文件头的 §9.2 那一段）
    // -----------------------------------------------------------------------

    /// `CompareBrowserVersions(version1, version2, result) -> HRESULT`
    type FnCompareBrowserVersions = unsafe extern "system" fn(Pcwstr, Pcwstr, *mut i32) -> Hresult;
    /// `CreateCoreWebView2Environment(environmentcreatedhandler) -> HRESULT`
    type FnCreateCoreWebView2Environment = unsafe extern "system" fn(*mut c_void) -> Hresult;
    /// `CreateCoreWebView2EnvironmentWithOptions(folder, userdata, options, handler) -> HRESULT`
    type FnCreateCoreWebView2EnvironmentWithOptions =
        unsafe extern "system" fn(Pcwstr, Pcwstr, *mut c_void, *mut c_void) -> Hresult;
    /// `GetAvailableCoreWebView2BrowserVersionString(folder, versioninfo) -> HRESULT`
    type FnGetAvailableCoreWebView2BrowserVersionString =
        unsafe extern "system" fn(Pcwstr, VersionInfoOut) -> Hresult;
    /// `GetAvailableCoreWebView2BrowserVersionStringWithOptions(folder, options, versioninfo) -> HRESULT`
    type FnGetAvailableCoreWebView2BrowserVersionStringWithOptions =
        unsafe extern "system" fn(Pcwstr, *mut c_void, VersionInfoOut) -> Hresult;

    // ⚠️ **5 个 `OnceLock` 分开、每个装自己那个类型**（不合成一个 `HashMap<String, usize>`）：
    //    合成之后每个转发函数都要在运行期 downcast，而那正是"ABI 对不对"最容易被埋掉的地方
    //    （本项目对"根因不许说错"有纪律）。
    //    函数指针本身是 `Send + Sync` ⇒ 这 5 个 `static` 不需要任何 `unsafe impl`。
    static COMPARE_BROWSER_VERSIONS: OnceLock<FnCompareBrowserVersions> = OnceLock::new();
    static CREATE_CORE_WEBVIEW2_ENVIRONMENT: OnceLock<FnCreateCoreWebView2Environment> =
        OnceLock::new();
    static CREATE_CORE_WEBVIEW2_ENVIRONMENT_WITH_OPTIONS: OnceLock<
        FnCreateCoreWebView2EnvironmentWithOptions,
    > = OnceLock::new();
    static GET_AVAILABLE_BROWSER_VERSION_STRING: OnceLock<
        FnGetAvailableCoreWebView2BrowserVersionString,
    > = OnceLock::new();
    static GET_AVAILABLE_BROWSER_VERSION_STRING_WITH_OPTIONS: OnceLock<
        FnGetAvailableCoreWebView2BrowserVersionStringWithOptions,
    > = OnceLock::new();

    /// **`install` 的本体**：释放 → 装载 → 解析 5 个符号。三步任何一步失败都指名道姓。
    pub fn install() -> Result<(), String> {
        // ---- 1) 释放到 %LOCALAPPDATA%\BenagenDownloader\cache\ ----
        //
        // ⚠️ **这一步用的是 `shell-core` 那套机制**（本模块一度自带过一份，删掉了：
        //    见文件头）。`cache_dir_from` / `extract_into` 都是那边的纯函数，
        //    参数化 LOCALAPPDATA 与"释放的是什么"就是为了它们能在宿主上被单测 ——
        //    同一个缓存目录，用户要清就一处清干净。
        // ⚠️ 那件"释放的是什么"是本模块的 [`super::LOADER`]（**不是内核那份**：
        //    传错的话，用户会读到一句"把 benagen-core.exe 放到程序旁边"的假补救）。
        let cache_dir = shell_core::embedded_core::cache_dir_from(
            &super::LOADER,
            std::env::var_os("LOCALAPPDATA"),
        )?;
        let path = shell_core::embedded_core::extract_into(
            &super::LOADER,
            LOADER_DLL,
            LOADER_SHA256,
            &cache_dir,
        )?;

        // ---- 2) LoadLibraryW（**绝对路径**）----
        //
        // ⚠️ **绝对路径是承重的**：相对路径会让 Windows 按它自己的搜索顺序去找
        //    （当前目录 → PATH → …），那正是"加载了来路不明的 DLL"的经典形态。
        //    `extract_into` 返回的是 `%LOCALAPPDATA%\…` 下的路径，本来就是绝对路径。
        let module = load_library(&path)?;
        // ⚠️ **句柄故意不留在任何地方**（不存进 static、也不调用 `FreeLibrary`）——
        //    模块的存活**不靠这个句柄变量**：`LoadLibraryW` 成功那一刻，这个映像就装进了
        //    本进程、一直活到进程结束（它不是一个"持有期"对象，也没有 `Drop`）。
        //    下面 5 个 `GetProcAddress` 用完它之后，它的全部价值就只剩那条引用计数，
        //    而我们是**故意**不卸载的（卸载一个正在被 Tauri/wry 用的映像必然崩）。

        // ---- 3) 5 个符号逐个解析（**取不到就指名道姓**）----
        let f = unsafe { resolve::<FnCompareBrowserVersions>(module, b"CompareBrowserVersions\0")? };
        let _ = COMPARE_BROWSER_VERSIONS.set(f);
        let f = unsafe {
            resolve::<FnCreateCoreWebView2Environment>(module, b"CreateCoreWebView2Environment\0")?
        };
        let _ = CREATE_CORE_WEBVIEW2_ENVIRONMENT.set(f);
        let f = unsafe {
            resolve::<FnCreateCoreWebView2EnvironmentWithOptions>(
                module,
                b"CreateCoreWebView2EnvironmentWithOptions\0",
            )?
        };
        let _ = CREATE_CORE_WEBVIEW2_ENVIRONMENT_WITH_OPTIONS.set(f);
        let f = unsafe {
            resolve::<FnGetAvailableCoreWebView2BrowserVersionString>(
                module,
                b"GetAvailableCoreWebView2BrowserVersionString\0",
            )?
        };
        let _ = GET_AVAILABLE_BROWSER_VERSION_STRING.set(f);
        let f = unsafe {
            resolve::<FnGetAvailableCoreWebView2BrowserVersionStringWithOptions>(
                module,
                b"GetAvailableCoreWebView2BrowserVersionStringWithOptions\0",
            )?
        };
        let _ = GET_AVAILABLE_BROWSER_VERSION_STRING_WITH_OPTIONS.set(f);
        Ok(())
    }

    /// `LoadLibraryW`（绝对路径）。失败时把系统原话带出来。
    fn load_library(path: &std::path::Path) -> Result<*mut c_void, String> {
        // ⚠️ `OsStrExt::encode_wide` 是 **Windows 专有**的（`std::os::windows::ffi`）——
        //    Win32 的 W 系列 API 要的是 UTF-16，而 `OsStr` 的编码在别的平台不是这一套。
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY：缓冲区以 NUL 结尾、在本调用期间一直存活；这是 LoadLibraryW 的契约。
        let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "装载 WebView2Loader.dll 失败：LoadLibraryW 返回了 NULL。\n\
                 文件：{}\n\
                 系统原话：{}\n\
                 补救：确认这个文件真的在（杀毒软件可能刚把它隔离了）；\
                 把本程序与 %LOCALAPPDATA%\\BenagenDownloader 加入杀软白名单后重新启动。\n\
                 ⚠ 本程序**没有控制台**可以说话，所以这条消息本身就是要显示给用户的那句话。",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(handle)
    }

    /// `GetProcAddress` 一次，返回函数地址（**不转型**）。
    ///
    /// ⚠️ **转型（`transmute`）留在调用点**，不在这里做：那一步是"ABI 对不对"唯一能被
    ///    人眼核对的地方（`usize` → 一个具名的函数指针类型），藏进一个泛型 helper 里
    ///    就等于把它埋掉了（那正是本文件拒绝用宏的理由）。
    ///
    /// ⚠️ `name` 必须**以 `\0` 结尾**：`GetProcAddress` 要的是 NUL 结尾的 ANSI 串，
    ///    少一个字节它会往后一直读到别的内存（找不到符号，或者更糟 —— 撞上一个同名的
    ///    无关导出）。⇒ 5 个调用点写的都是 `b"名字\0"` 字面量。
    unsafe fn proc_address(module: *mut c_void, name: &[u8]) -> Result<usize, String> {
        use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        // SAFETY：`module` 是 install 刚装载出来的句柄；`name` 以 NUL 结尾（见上）。
        let raw = unsafe { GetProcAddress(module, name.as_ptr()) };
        // `FARPROC` 在 windows-sys 里是 `Option<unsafe extern "system" fn() -> isize>`：
        // `None` 就是"没这个导出"（也可能是名字拼错了），两种都要响。
        raw.map(|f| f as usize).ok_or_else(|| {
            let name = String::from_utf8_lossy(&name[..name.len().saturating_sub(1)]);
            format!(
                "WebView2Loader.dll 里找不到符号 `{name}` —— 单文件交付的转发面接不上。\n\
                 补救：这多半意味着内嵌的那份 DLL 与 vendored 的 crate 版本对不上\
                 （换了 tauri 的版本之后忘了重新 vendor？）。\n\
                 把这条原样发给我们（`windows/vendor/webview2-com-sys` 的版本号是关键信息）。"
            )
        })
    }

    /// 把一个符号解析成**具名的函数指针类型** `T`。
    ///
    /// ⚠️ `unsafe` 的边界在这里一次性说清：`GetProcAddress` 返回的地址**是不是** `T`
    ///    那个签名，**没有任何运行期检查**（C 的导出表里只有名字）。对不对只有真 Windows
    ///    能验（规格 §9.2），这也是本文件把 5 个签名逐个手写、并把 `T` 显式写在调用点上的原因。
    unsafe fn resolve<T: Copy>(module: *mut c_void, name: &[u8]) -> Result<T, String> {
        let raw = unsafe { proc_address(module, name)? };
        debug_assert!(std::mem::size_of::<T>() == std::mem::size_of::<usize>());
        // SAFETY：函数指针与 usize 同宽（上面那个 debug_assert）；地址来自
        // WebView2Loader.dll 的导出表，签名由调用点给的 `T` 声明 —— 那就是"对不对"的全部依据。
        Ok(unsafe { std::mem::transmute_copy::<usize, T>(&raw) })
    }

    // -----------------------------------------------------------------------
    // 5 个转发函数 —— 这就是 crate 里那 5 条 `link!` 声明要的符号
    // -----------------------------------------------------------------------
    //
    // ⚠️ `#[no_mangle]` + `extern "system"`：**符号名与 ABI 都要与
    //    `vendor/webview2-com-sys/src/bindings.rs` 里那 5 条 `link!` 逐字对齐**。
    //    对不上的表现是链接期 `undefined reference`（好，响亮），
    //    或者更坏的：名字对上了、ABI 不对（那只有真机上才炸）。
    //
    // ⚠️ 每个函数第一行的 `.expect(...)` 都**不可达**：`install()` 要么把 5 个符号全解析好、
    //    要么返回 `Err`（调用方弹消息框然后退出）⇒ 能走到这里就说明那 5 个槽都满了。

    /// `CompareBrowserVersions`（见 crate 里的同名导出）。
    #[no_mangle]
    pub unsafe extern "system" fn CompareBrowserVersions(
        version1: Pcwstr,
        version2: Pcwstr,
        result: *mut i32,
    ) -> Hresult {
        let f = *COMPARE_BROWSER_VERSIONS
            .get()
            .expect("install() 没有先跑成功：那 5 个符号必须由它全部解析好");
        f(version1, version2, result)
    }

    /// `CreateCoreWebView2Environment`。
    #[no_mangle]
    pub unsafe extern "system" fn CreateCoreWebView2Environment(
        environmentcreatedhandler: *mut c_void,
    ) -> Hresult {
        let f = *CREATE_CORE_WEBVIEW2_ENVIRONMENT
            .get()
            .expect("install() 没有先跑成功：那 5 个符号必须由它全部解析好");
        f(environmentcreatedhandler)
    }

    /// `CreateCoreWebView2EnvironmentWithOptions`。
    #[no_mangle]
    pub unsafe extern "system" fn CreateCoreWebView2EnvironmentWithOptions(
        browserexecutablefolder: Pcwstr,
        userdatafolder: Pcwstr,
        environmentoptions: *mut c_void,
        environmentcreatedhandler: *mut c_void,
    ) -> Hresult {
        let f = *CREATE_CORE_WEBVIEW2_ENVIRONMENT_WITH_OPTIONS
            .get()
            .expect("install() 没有先跑成功：那 5 个符号必须由它全部解析好");
        f(
            browserexecutablefolder,
            userdatafolder,
            environmentoptions,
            environmentcreatedhandler,
        )
    }

    /// `GetAvailableCoreWebView2BrowserVersionString`。
    #[no_mangle]
    pub unsafe extern "system" fn GetAvailableCoreWebView2BrowserVersionString(
        browserexecutablefolder: Pcwstr,
        versioninfo: VersionInfoOut,
    ) -> Hresult {
        let f = *GET_AVAILABLE_BROWSER_VERSION_STRING
            .get()
            .expect("install() 没有先跑成功：那 5 个符号必须由它全部解析好");
        f(browserexecutablefolder, versioninfo)
    }

    /// `GetAvailableCoreWebView2BrowserVersionStringWithOptions`。
    #[no_mangle]
    pub unsafe extern "system" fn GetAvailableCoreWebView2BrowserVersionStringWithOptions(
        browserexecutablefolder: Pcwstr,
        environmentoptions: *mut c_void,
        versioninfo: VersionInfoOut,
    ) -> Hresult {
        let f = *GET_AVAILABLE_BROWSER_VERSION_STRING_WITH_OPTIONS
            .get()
            .expect("install() 没有先跑成功：那 5 个符号必须由它全部解析好");
        f(browserexecutablefolder, environmentoptions, versioninfo)
    }
}
