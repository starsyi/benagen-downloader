//! `shell-win/build.rs` —— **构建期装配**（任务 22 + Tauri 那一代，规格 §7.2 / §11.4 / W-7 / §4）。
//!
//! 0. **`tauri_build::build()`**（Tauri 那一代新增）：Tauri 的代码生成 ——
//!    `src/main.rs` 的 `tauri::generate_context!` 读的就是它的产出。**在所有靶上都跑**
//!    （宿主构建跑的是同一个壳，规格 §9.0）。
//! 0b. **单文件交付的三条守卫**（Tauri 那一代新增，规格 §4.2；第三条是任务 15 补的）：
//!     vendor 那份 `webview2-com-sys` 的补丁**还在不在**、**`[patch.crates-io]` 真的
//!     生效了没有**（`Cargo.lock` 里那条不许带 `source`/`checksum`）、内嵌的
//!     `WebView2Loader.dll` 是不是**登记过的那一份**；核过之后把它拷进 `OUT_DIR`
//!     （`src/wv2.rs` 内嵌它）。三条守卫也**在所有靶上跑**，理由写在 `main` 里。
//! 1. **把已经编好的内核 exe 拷进 `OUT_DIR`**，好让 `src/embed.rs` 用
//!    `include_bytes!(concat!(env!("OUT_DIR"), "/benagen-core.exe"))` 内嵌它；
//!    顺手在**这里**（而不是运行期）核对它的 PE 头与架构（W-1）。
//! 2. **生成 `.rc` 并链进去**：application manifest（`supportedOS` = Windows 10，W-7）
//!    与版本资源（`FileVersion` / `ProductName`）。
//!
//! ⚠️ 上面 1 / 2 两件事**只在 Windows 靶上做**（那两段自第一版起就是一个整体，别拆）；
//!    0 / 0b 是 Tauri 那一代加的、**所有靶都做**。
//!
//! ## ⚠️⚠️ 构建顺序是**硬约束**，而这里**故意不去猜它**（本文件最重要的一段）
//!
//! cargo **不知道**"壳依赖内核 exe"这层依赖（`include_bytes!` 的路径是 `OUT_DIR` 里那份
//! 副本，cargo 只看见副本）。所以本文件做的是：
//!
//!   * 内核 exe **不在场就大声失败**（不是"编一个空壳出来"——那会让交付物缺内核而没人知道）；
//!   * 对它做 **W-1 的字节断言**（PE 头 / 架构 / 子系统）：**编不出来就在构建期炸**，
//!     不要等到运行期才发现；
//!   * 把它当成 `rerun-if-changed` 的**依赖**登记：内核 exe 一变，本 crate 重编
//!     （这是 cargo 唯一能听懂的那半句"顺序"）；
//!   * 把它的 **sha256** 当成编译期常量交给运行期（`BENAGEN_CORE_SHA256`，见 `embed.rs`）
//!     —— 那是"内嵌链路没接错、没嵌到旧货"在构建期能被核出来的那一半。
//!
//! **排顺序是 `windows/scripts/build_windows.sh` 的活**（它先编内核、再编壳），
//! 本文件**不做**"新鲜度启发式"——理由是实测出来的两条（任务 5 的审查，2026-09-18）：
//!
//!   * **内核 exe 的 sha256 不可复现**：PE COFF 头每次写**链接时刻**；
//!   * **大小也不是指纹**：同一份源码在两处编出差 98 字节（差值全在尾部 COFF 符号表）。
//!
//! ⇒ 任何"重新编一遍、比哈希/比大小"的判据都会**永远红**。这里**只**比对
//!    **同一份产物文件**（`OUT_DIR` 里那份副本 vs `core/target/…` 里那份原件），
//!    那是唯一有判别力的比较。**下一个改这个文件的人：不要加"重编一次看看"的检查。**
//!
//! ## ⚠️ 本文件在宿主（macOS）上也**会**被跑
//!
//! `cargo test -p shell-win` 要编本 crate（宿主靶），build script 一样会执行。
//! 而宿主上没有内核 exe、也不该要求有 `windres` —— 所以下面第一件事就是判靶：
//! **只有 `CARGO_CFG_TARGET_OS == "windows"` 时才做这两件事**，其余靶直接返回。
//! 这不是"静默降级"（W-2）：宿主的产物**本来就不是交付形态**，它不进任何一条分发路径；
//! 而交付形态那一条（Windows 靶）**一律**做这两个检查，没有开关。

use std::path::{Path, PathBuf};

// ⚠️ **sha256 只有一份实现**：它就是 `shell-core` 那个模块的**原文**
//    （`#[path]` 是把同一份源码编进本 build script，不是复制一份代码）。
//    build script 是独立编译的、没有依赖图，所以不能 `use shell_core::sha256`。
//    理由与"分叉会怎样"写在 `shell-core/src/sha256.rs` 的文件头上。
#[path = "../shell-core/src/sha256.rs"]
mod sha256;

/// 内核 exe 的产物路径（相对本 crate 的清单目录）：**由 `core/scripts/build_core_windows.sh`
/// 钉住的**那一个显式路径（那个脚本显式设 `CARGO_TARGET_DIR`，产物不随调用者漂移）。
const CORE_EXE_REL: &str = "../../core/target/x86_64-pc-windows-gnu/release/benagen-core.exe";

// ---------------------------------------------------------------------------
// 0) WebView2Loader 补丁的两条守卫（规格 §4.2；本任务新增）
// ---------------------------------------------------------------------------

/// vendor 进仓库的那份 `webview2-com-sys`（单文件交付全靠它，见 `windows/Cargo.toml` 的
/// `[patch.crates-io]` 那一段与 `vendor/webview2-com-sys/src/lib.rs` 的文件头）。
const VENDOR_DIR_REL: &str = "../vendor/webview2-com-sys";

/// 补丁**所在的那个文件**：宏在里面，形状变了就是补丁失效了。
const VENDOR_LIB_RS_REL: &str = "../vendor/webview2-com-sys/src/lib.rs";

/// 内嵌进壳的 `WebView2Loader.dll`（x64 一份；本代只支持 x86_64，规格 §4.2）。
const VENDOR_DLL_REL: &str = "../vendor/webview2-com-sys/x64/WebView2Loader.dll";

/// `vendor/…/x64/WebView2Loader.dll` 的 sha256 —— **登记值**（64 位小写十六进制）。
///
/// ⚠️ **它是"这份 DLL 还是不是我们审过的那一份"的判据**，不是校验和装饰：
///    升级 `tauri` ⇒ `webview2-com-sys` 升版 ⇒ 必须重新 vendor ⇒ 这个值**会变**
///    ⇒ 构建当场红，并把"去核对那份 DLL 是不是你要的"这句话给人看（见
///    [`read_and_check_loader_dll`]）。**改这个值是一次能被审查的决定**，
///    与 `licenses.rs` 里那两个 sha256 登记值同一条纪律。
///    实测取值来源：`shasum -a 256 vendor/webview2-com-sys/x64/WebView2Loader.dll`
///    （crates.io 上 `webview2-com-sys 0.38.2` 包内那份，157 KB）。
const LOADER_DLL_SHA256: &str = "8427b1fc58ec707813e5c0a51eb5d69397bb333250a7b891be4d3b123f1e0f1c";

/// 补丁**必须在这里面**的那一串（MSVC 分支，原样保留的那一半）。
const VENDOR_MSVC_LINK: &str = r#"link(name = "WebView2LoaderStatic", kind = "static")"#;
/// 补丁**必须不在**源码里出现的那一串（原版给 gnu 目标的导入声明）。
///
/// ⚠️ **判据是"去掉注释之后再找"**（见 [`read_and_check_vendored_patch`]）：那串字面量
///    在**注释里**本来就要出现（补丁那一处注释正是拿它当反例讲的），
///    直接 `contains` 会**恒真** —— 那样这条守卫就是一句看着在检查、其实什么都没检查的话。
const VENDOR_GNU_LINK: &str = r#"link(name = "WebView2Loader.dll")"#;

/// PE 的 `Machine` 值：`IMAGE_FILE_MACHINE_AMD64`（x86-64）。
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
/// PE 可选头 magic：`PE32+`（64 位）。
const PE32_PLUS_MAGIC: u16 = 0x020b;
/// 子系统：`IMAGE_SUBSYSTEM_WINDOWS_CUI`（控制台）——内核是**控制台程序**，
/// 它的控制台窗口由 shell 侧的 `CREATE_NO_WINDOW` 消掉（`spawn.rs` 的落点②）。
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

fn main() {
    // build.rs 自身变了要重跑（`rerun-if-changed` 一旦被发出，cargo 就只认这些依赖，
    // 所以"自己"这一条不能漏）。
    println!("cargo:rerun-if-changed=build.rs");

    // ⚠️ **Tauri 的代码生成，必须第一个跑**（`src/main.rs` 的 `tauri::generate_context!`
    //    读的就是它产出的东西）：它要在**每一个靶**上跑，宿主构建也不例外
    //    （规格 §9.0：这台 Mac 跑的就是同一个壳）。
    tauri_build::build();

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("cargo 必须给 build script 设 CARGO_MANIFEST_DIR"),
    );
    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo 必须给 build script 设 OUT_DIR"));

    // ---- 三条与靶无关的守卫（本任务）----------------------------------------
    //
    // ⚠️ **它们为什么在所有靶上跑，而不是跟着下面那两件事一起被关在 Windows 靶里**：
    //    这两条守的都是"**单文件交付**"这件事（W-4），而它的失效方式恰恰是
    //    "**在 macOS 上构建照样成功**"（补丁没了 / 重新 vendor 了别的 DLL）——
    //    只在 Windows 靶上跑它们，就等于把最早的反馈推到了最慢的那条构建路径上。
    //    代价是每次构建读两个小文件（1.8 KB + 157 KB）并哈希其中一个，可以忽略。
    read_and_check_vendored_patch(&manifest_dir);
    read_and_check_patch_is_in_effect(&manifest_dir);
    let loader_dll = read_and_check_loader_dll(&manifest_dir);

    // 宿主（macOS）靶：不打包。理由见文件头"本文件在宿主上也会被跑"那一段。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // ⚠️ **内嵌的 DLL 必须先落到 `OUT_DIR`**：`src/wv2.rs` 的 `include_bytes!` 读的是
    //    **那里**（与内核 exe 同形）—— 直接 `include_bytes!("../vendor/…")` 会绕过
    //    上面那条 sha256 断言，而那条断言正是"这份 DLL 还是不是我们审过的那一份"的判据。
    //    ⚠️ 与内核 exe 不同，**这里不需要构建脚本排顺序**：写入 `OUT_DIR` 的正是本脚本自己，
    //       而它一定在 rustc 编 `wv2.rs` 之前跑完（cargo 的契约）。内核 exe 那一条要排顺序，
    //       是因为它来自**另一个 crate 的构建**。
    stage_loader_dll(&loader_dll, &out_dir);

    embed_core_exe(&manifest_dir, &out_dir);
    pin_link_timestamp();
    link_manifest_and_version(&out_dir);
}

// ---------------------------------------------------------------------------
// 0) 单文件交付的两条守卫 + DLL 内嵌（本任务）
// ---------------------------------------------------------------------------

/// **补丁还在不在**：读 vendor 那份 `src/lib.rs`，确认宏长成补丁的形状。
///
/// 判据两条，缺一不可（`VENDOR_GNU_LINK` 那条要**去掉注释之后**再找，理由见常量自己的文档）：
///   · 原版给 gnu 目标的那条 `#[link]` **必须已经不在**；
///   · MSVC 那一支的 `#[link]` **必须还在**（补丁只许删 gnu 那一条，不许动另一半）。
///
/// ⚠️ **它拦的是哪一幕**：`tauri` 升版 ⇒ 重新 vendor ⇒ **忘了把补丁重新打上**
///    （或者打了一半）。那一幕的表现是导入表里多回一行 `WebView2Loader.dll`，
///    而**在 macOS 上构建照样成功** —— 本函数是全仓**最早**会因此变红的地方
///    （`build_windows.sh` 第 4 步那条导入表判据是最后一道，两者都要在）。
///
/// ⚠️ **它拦不住的一幕（如实记账）**：`[patch.crates-io]` 因为版本不相容而**静默不生效**
///    ——那时 vendor 这份**根本没被编进去**，本函数照样绿。那一幕由
///    [`read_and_check_patch_is_in_effect`]（宿主上也跑，秒级）与
///    `build_windows.sh` 的导入表判据（交付靶，最后一道）一起接住（规格 §8.2 第 2 条
///    点名的就是后一条）。
fn read_and_check_vendored_patch(manifest_dir: &Path) {
    let path = manifest_dir.join(VENDOR_LIB_RS_REL);
    let source = std::fs::read_to_string(&path).unwrap_or_else(|cause| {
        panic!(
            "单文件交付的守卫跑不了：读不到补丁版 crate 的源码。\n\
             路径：{}\n\
             系统原话：{cause}\n\
             补救：确认 {VENDOR_DIR_REL}/ 还在（它是仓库的一部分，不是构建产物），\n\
               以及 shell-win/build.rs 的 VENDOR_LIB_RS_REL 与它一致。",
            path.display()
        )
    });
    // ⚠️ cargo 只在"这个文件变了"时才重跑本脚本 —— 少了这一行，补丁被改回去之后
    //    本脚本**不会重跑**，那条守卫就变成了"上一次构建时它还好的"。
    println!("cargo:rerun-if-changed={}", path.display());

    // 去掉纯注释行之后再看：那些字面量在注释里本来就要出现（补丁那一处注释拿它当反例讲），
    // 不去注释的话 `contains` 恒真 —— 那是"看着在检查、其实什么都没检查"。
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !code.contains(VENDOR_GNU_LINK),
        "单文件交付被破坏：补丁版 crate 的 {} 里又有给 gnu 目标的导入声明了。\n\
         找的是这一段：{VENDOR_GNU_LINK}\n\
         ⚠ 后果：链接出来的 exe 会依赖 WebView2Loader.dll（客户机器上没这个文件 ⇒ 起不来）。\n\
         根因（按可能性排序）：① 重新 vendor 时把打了补丁的 src/lib.rs 覆盖回了 crates.io 原版；\n\
           ② 有人「整理」过那个宏；③ 补丁只打了一半。\n\
         补救：按 {VENDOR_DIR_REL}/src/lib.rs 文件头那一段把补丁重新打上。\n\
         完整根因见 docs/superpowers/2026-09-20-tauri2-windows-crosscompile.md §3。",
        path.display()
    );
    assert!(
        code.contains(VENDOR_MSVC_LINK),
        "补丁打过头了：{} 里找不到 MSVC 目标的静态链接声明。\n\
         找的是这一段：{VENDOR_MSVC_LINK}\n\
         ⚠ 补丁只许删**给 gnu 目标**的那一条 #[link]，MSVC 那一支必须原样保留\n\
           （动了它会坏掉 MSVC 目标的构建，而本仓库根本没有 MSVC 的构建路径 ⇒ 坏了当场发现不了）。\n\
         补救：从 crates.io 上 0.38.2 的原版重新 vendor，只删 gnu 那一条。",
        path.display()
    );
}

/// **`[patch.crates-io]` 真的生效了吗**：`shell-win/../Cargo.lock` 里那条
/// `webview2-com-sys` **不许带 `source` / `checksum` 行**。
///
/// ## 为什么需要这一条（它补的是 [`read_and_check_vendored_patch`] 结构上照不到的那一幕）
///
/// `[patch.crates-io]` 是**按版本**匹配的：`windows/vendor/webview2-com-sys/Cargo.toml`
/// 的版本号与 `tauri` 实际拉到的那一版对不上时，cargo **不报错**，只是**这次 patch 不适用**
/// —— 于是编进去的是 **crates.io 上那份原版**，而 vendor 里那份（打过补丁的）**根本没被编**。
///
/// ⚠️ 那一幕，本文件里另外三条守卫**一条都拦不住**：
///   * [`read_and_check_vendored_patch`] 读的是**磁盘上的 vendor 源码** ——
///     它证明的是"仓库里那份是打过补丁的"，**不是**"被编进去的是那一份"；
///   * [`read_and_check_loader_dll`] 同理（它读的也是磁盘）；
///   * 真正拦得住的是 `windows/scripts/build_windows.sh` 第 4 步那条**导入表**判据
///     —— 而它是**最慢**的那条路径（要交叉编译 + `objdump`），在宿主上永远走不到。
///
/// ⇒ 这一条把同一件事搬到了**宿主构建**上：`Cargo.lock` 是 **cargo 自己解析出来的图**
/// （不是我们读的一堆文件），而"用 path 覆盖掉 registry 那一份"这件事在 lock 里有一个
/// 确定的、可判的形状：
///
/// ```text
/// [[package]]
/// name = "webview2-com-sys"          ← 这一条
/// version = "0.38.2"
/// dependencies = [ … ]               ← 没有 source、也没有 checksum
/// ```
///
/// 带 `source = "registry+https://…"`（以及紧随其后的 `checksum = "…"`）就说明吃的是
/// **registry 那份** ⇒ patch 没生效 ⇒ 交付物会重新依赖 `WebView2Loader.dll`。
///
/// ⚠️ **判据的效力边界（如实记账）**：它判的是 **lock 里的解析结果**，不是"链接出来的 exe"。
///    `Cargo.lock` 与 `windows/target/` 不同步（比如有人手工改锁文件、或用了别的 target 目录）
///    时它仍可能绿。所以它**不能**取代 `build_windows.sh` 的导入表判据 —— 两者都在，
///    一个早（宿主，秒级）一个晚（交付靶，最后一道）。
fn read_and_check_patch_is_in_effect(manifest_dir: &Path) {
    let path = manifest_dir.join("../Cargo.lock");
    let lock = std::fs::read_to_string(&path).unwrap_or_else(|cause| {
        panic!(
            "单文件交付的守卫跑不了：读不到工作区的 Cargo.lock。\n\
             路径：{}\n\
             系统原话：{cause}\n\
             补救：确认 windows/Cargo.lock 还在（它是仓库的一部分，不是构建产物）。",
            path.display()
        )
    });
    // ⚠️ 少了这一行，`Cargo.lock` 变了而本脚本不重跑 ⇒ 这条守卫是"上一次构建时它还好的"。
    println!("cargo:rerun-if-changed={}", path.display());

    let mut inside = false;
    let mut found = false;
    for line in lock.lines() {
        let line = line.trim_end();
        if line.starts_with("[[") || (line.starts_with('[') && line.ends_with(']')) {
            // 新的 `[[package]]` 或新的表头 ⇒ 上面那条 `webview2-com-sys` 的块结束了。
            inside = false;
            continue;
        }
        if line.starts_with("name = ") {
            inside = line == r#"name = "webview2-com-sys""#;
            if inside {
                found = true;
            }
            continue;
        }
        if inside && (line.starts_with("source = ") || line.starts_with("checksum = ")) {
            panic!(
                "`[patch.crates-io]` 没有生效：Cargo.lock 里那条 webview2-com-sys 带着 `source`/`checksum`。\n\
                 文件：{}\n\
                 那一行：{line}\n\
                 ⚠ 后果：编进去的是 **crates.io 上的原版**，不是 windows/vendor/ 里那份打过补丁的\n\
                   ⇒ 链接出来的 exe 会依赖 WebView2Loader.dll（客户机器上没这个文件 ⇒ 起不来）。\n\
                 根因（按可能性排序）：① windows/vendor/webview2-com-sys/Cargo.toml 的版本号与\n\
                   `tauri` 实际拉到的那一版**对不上**（patch 是按版本匹配的，对不上就**静默**不适用）；\n\
                   ② windows/Cargo.toml 的 [patch.crates-io] 那一段被删了/写错了；\n\
                   ③ 有人手工编辑过 Cargo.lock。\n\
                 补救：跑 `cargo tree -p webview2-com-sys` 看实际拉的是哪一版，把\n\
                   windows/vendor/webview2-com-sys/Cargo.toml 的 version 改成它，再重编。\n\
                 ⚠ 这一条**在 macOS 上就会红**（本函数在所有靶上都跑）—— 它是这件事**最早**\n\
                   会变红的地方；`build_windows.sh` 第 4 步那条导入表判据是最后一道，两者都要在。",
                path.display()
            );
        }
    }
    assert!(
        found,
        "单文件交付的守卫跑不了：{} 里找不到 [[package]] name = \"webview2-com-sys\"。\n\
         ⚠ 两种可能：① 依赖被整个去掉了（那 patched crate 就不需要了，把本守卫一起删掉并写明理由）；\n\
           ② 锁文件的形状变了（换 cargo 版本？）—— 那就先确认这里读法还对，再改本函数。",
        path.display()
    );
}

/// **交付 exe 的可复现性：不许把链接时刻写进 PE 头**（任务 15 步骤 1b）。
///
/// ## 为什么这是一条判据的载体，而不是"锦上添花"
///
/// 实测（2026-09-20，同一棵树连跑两次 `build_windows.sh`）：**体积一模一样
/// （35,407,071 B）、sha256 不同**（`b1f177c5…` vs `3bd634a8…`）。⇒ 交付形态**不可复现**
/// ⇒ "**发出去的是哪一个**"这个问题答不出来，而真机验收清单上写着 sha256（清单本身是要给的）。
///
/// 根因不是内容：`x86_64-w64-mingw32-objdump -p` 的 `Time/Date` 行**每次都是链接那一刻**
/// —— mingw 的 `ld` 默认往 PE 的 COFF 头写 `TimeDateStamp`。binutils 有一个既有开关
/// `--no-insert-timestamp` 就是关掉它（写 0），于是**同样的输入 ⇒ 同样的字节**。
///
/// ⚠️ **为什么这里发的是 `-Wl,` 前缀的那一份**：rustc 在这个靶上把链接交给
/// `x86_64-w64-mingw32-gcc`（**链接器驱动**，不是 `ld` 本身），`--no-insert-timestamp`
/// 是 `ld` 的选项 ⇒ 必须经 `-Wl,` 转交，写成裸选项 gcc 会以
/// "unrecognized command line option" 失败。
///
/// ⚠️ **只在 Windows 靶上发**（本函数在 `main` 里位于那道靶判断**之后**）：`-Wl,…` 是
///    GNU 工具的语法，宿主（macOS，Mach-O 的 `ld64`）收到它会直接链接失败 —— 而宿主构建
///    是开发期的主验证载体（规格 §9.0），不该为了一个只对交付形态成立的属性把它弄红。
///
/// ⚠️ **换链接器时的表现**：若有人把 `-C linker-flavor` 换成 `lld` / `rust-lld`，
///    `-Wl,` 这一层中转就没了意义（那些驱动不认 `-Wl,`）。那时**构建会大声失败**
///    （链接器说它不认识的选项），不是静默降级 —— 这正是我们要的形态：可复现性
///    要么成立、要么有一条能照着修的错，不许"悄悄又不复现了"。
///
/// ⚠️ **实测取值（2026-09-20）**：加这一条之前，同树两次构建 sha256 不同；加上之后
///    **连跑两次体积与 sha256 都相同**（数字记在 `windows/scripts/build_windows.sh`
///    第 5d 步那段注释与真机验收清单里）。**这条断言的意义是"它现在被什么守着"** ——
///    `build_windows.sh` 第 5d 步**每跑一次就当场比一次**（两次构建 → 两次读数 → 一比），
///    R-61 那条"修好了要答得出被什么守着"在这里的答案就是那一步。
fn pin_link_timestamp() {
    println!("cargo:rustc-link-arg-bins=-Wl,--no-insert-timestamp");
}

/// **这份 DLL 还是不是我们审过的那一份**：读它、哈希它、和登记值比。返回它的字节。
///
/// ⚠️ 返回值是**已经核过的**那批字节，调用方直接拿去写 `OUT_DIR` —— 不要在这里之外
/// 再 `std::fs::read` 一次（那会造出第二条读路径，而"哪一份进了 exe"就说不清了）。
///
/// ⚠️ 哈希**所有靶都算**（理由见 `main`）。判据是**逐字节**的：DLL 换了、被截断了、
/// 被别的东西覆盖了，都拦得住；而"换成了另一个**同长度**的 DLL"也拦得住（哈希会变）。
fn read_and_check_loader_dll(manifest_dir: &Path) -> Vec<u8> {
    let path = manifest_dir.join(VENDOR_DLL_REL);
    let bytes = std::fs::read(&path).unwrap_or_else(|cause| {
        panic!(
            "单文件交付的守卫跑不了：读不到内嵌的 WebView2Loader.dll。\n\
             路径：{}\n\
             系统原话：{cause}\n\
             补救：确认 windows/vendor/webview2-com-sys/x64/WebView2Loader.dll 还在\n\
               （它是仓库的一部分；`git status` 看它是不是被误删了）。",
            path.display()
        )
    });
    // ⚠️ **这一行是承重的**：DLL 换了而本脚本不重跑的话，`wv2.rs` 编出来的还是旧字节，
    //    而那条 sha256 断言根本不会被问到（见文件头"构建顺序"那一段同源的道理）。
    println!("cargo:rerun-if-changed={}", path.display());

    let digest = sha256::sha256_hex(&bytes);
    assert!(
        digest == LOADER_DLL_SHA256,
        "内嵌的 WebView2Loader.dll 不是登记的那一份。\n\
         文件：{}\n\
         登记（shell-win/build.rs 的 LOADER_DLL_SHA256）：{LOADER_DLL_SHA256}\n\
         实算：{digest}\n\
         ⇒ 只有两种可能：① 这份 DLL 被改过/被截断过（那是单文件交付的输入，把它换回来）；\n\
           ② 这是一次**有意**的升级（重新 vendor 了 webview2-com-sys 的另一个版本）——\n\
              那就把 LOADER_DLL_SHA256 与 vendor/ 的版本号一起更新，让这次放宽能被审查。\n\
         ⚠ 不要「顺手把断言删掉」：升级 tauri 忘重新 vendor 的那一幕，全靠这条与\n\
           windows/scripts/build_windows.sh 的导入表判据接住。",
        path.display()
    );
    // 把**已经核过**的这个值交给编译期（`wv2.rs` 用 `env!()` 取，当释放文件名的派生源）。
    // ⚠️ 与内核那条 `BENAGEN_CORE_SHA256` 是同一条口径：**核对过的值与编进代码的值是同一个**
    //    —— 在运行期现算一次会让"哪一份被核过"多出一条分叉。
    // ⚠️ 这一行在**所有靶**上发（本函数就是所有靶都跑的），而读它的 `env!()` 只在
    //    `not(msvc)` 的 Windows 靶上存在 —— 别的靶上它就是一个没人读的环境变量，无害。
    println!("cargo:rustc-env=BENAGEN_WV2_LOADER_SHA256={digest}");
    eprintln!(
        "shell-win/build.rs: 已核对内嵌的 WebView2Loader.dll（{} 字节，sha256 {}）",
        bytes.len(),
        digest
    );
    bytes
}

/// 把**已经核过**的 DLL 放进 `OUT_DIR`，好让 `src/wv2.rs` 用
/// `include_bytes!(concat!(env!("OUT_DIR"), "/WebView2Loader.dll"))` 内嵌它。
///
/// ⚠️ **只在 Windows 靶上被调用**（`wv2.rs` 里那个常量本身就只在 Windows 靶上存在）。
///    本函数**不**自己读盘：字节是 [`read_and_check_loader_dll`] 核过之后传进来的
///    —— 读两次会造出第二条真相（"哪一份被核过"与"哪一份被内嵌"可能不是同一份）。
fn stage_loader_dll(bytes: &[u8], out_dir: &Path) {
    // 只在**字节不同**时才写：相同就别碰（碰了会改 mtime，可能触发无谓的重链）。
    // 与内核 exe 那一处同形。
    let staged = out_dir.join("WebView2Loader.dll");
    if std::fs::read(&staged).ok().as_deref() == Some(bytes) {
        return;
    }
    std::fs::write(&staged, bytes)
        .unwrap_or_else(|e| panic!("把 WebView2Loader.dll 拷进 OUT_DIR 失败（{}）：{e}", staged.display()));
}

// ---------------------------------------------------------------------------
// 1) 内嵌内核 exe
// ---------------------------------------------------------------------------

/// 把内核 exe 拷进 `OUT_DIR`，并把它的事实（sha256 / 字节数）交给编译期。
fn embed_core_exe(manifest_dir: &Path, out_dir: &Path) {
    let core_exe = manifest_dir.join(CORE_EXE_REL);
    let bytes = std::fs::read(&core_exe).unwrap_or_else(|cause| {
        panic!(
            "内嵌内核失败：读不到内核 exe。\n\
             路径：{}\n\
             系统原话：{cause}\n\
             ⚠ 这不是本 crate 的问题：内核必须先被编出来（构建顺序是硬约束，见本文件头）。\n\
             补救：先跑 `bash core/scripts/build_core_windows.sh`（它会自验产物），\n\
               或者用 `bash windows/scripts/build_windows.sh` —— 那条路径本来就先编内核。",
            core_exe.display()
        )
    });

    // ---- W-1：只看字节，不信参数 ------------------------------------------
    let facts = pe_facts(&bytes).unwrap_or_else(|why| {
        panic!(
            "内嵌内核失败：{} 不是一个 PE 文件 —— {why}\n\
             补救：把 core/target/x86_64-pc-windows-gnu/ 清掉重编（`bash core/scripts/build_core_windows.sh`）。",
            core_exe.display()
        )
    });
    assert!(
        facts.machine == IMAGE_FILE_MACHINE_AMD64,
        "内嵌内核失败：{} 的 Machine = 0x{:04x}，不是 x86-64（0x{:04x}）。\n\
         补救：核对构建脚本用的 --target（必须是 x86_64-pc-windows-gnu）。",
        core_exe.display(),
        facts.machine,
        IMAGE_FILE_MACHINE_AMD64
    );
    assert!(
        facts.magic == PE32_PLUS_MAGIC,
        "内嵌内核失败：{} 的可选头 magic = 0x{:04x}，不是 PE32+（0x{:04x}）。",
        core_exe.display(),
        facts.magic,
        PE32_PLUS_MAGIC
    );
    assert!(
        facts.subsystem == IMAGE_SUBSYSTEM_WINDOWS_CUI,
        "内嵌内核失败：{} 的子系统 = {}，不是控制台（{}）。\n\
         ⚠ 内核靠 stdin/stdout 上的 JSON Lines 与壳说话；子系统换了意味着那份产物不是本内核。",
        core_exe.display(),
        facts.subsystem,
        IMAGE_SUBSYSTEM_WINDOWS_CUI
    );

    // ⚠️ **新鲜度的那半句 cargo 听得懂的话**：内核 exe 变了 ⇒ 本 crate 重编。
    //    （顺序那一半它听不懂 —— 由构建脚本负责，见文件头。）
    println!("cargo:rerun-if-changed={}", core_exe.display());

    // 只在**字节不同**时才写 `OUT_DIR`：相同就别碰（碰了会改 mtime，可能触发无谓的重链）。
    let staged = out_dir.join("benagen-core.exe");
    if std::fs::read(&staged).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(&staged, &bytes)
            .unwrap_or_else(|e| panic!("把内核 exe 拷进 OUT_DIR 失败（{}）：{e}", staged.display()));
    }

    let digest = sha256::sha256_hex(&bytes);
    // 这一条是**编译期常量**，运行期用 `env!()` 取（`embed.rs::CORE_SHA256`）：
    //   · 运行时靠它判断"磁盘上那份是不是同一份"（哈希相符才复用）；
    //   · 构建脚本靠它自验"包里那份内核确实是这一份"（离线也能核）。
    // ⚠️ 这里**不**再传字节数：字节数不是判据（见文件头），而一个没人读的常量
    //    会在 Windows 靶上多一条 `dead_code` 告警（本 workspace 是零告警口径）。
    println!("cargo:rustc-env=BENAGEN_CORE_SHA256={digest}");
    // 给人看的一行：构建日志里能直接读出"这次嵌的是哪一份内核"。
    eprintln!(
        "shell-win/build.rs: 内嵌内核 {}（{} 字节，sha256 {}）",
        core_exe.display(),
        bytes.len(),
        digest
    );
}

/// 从 PE 字节里读出这几个字段（`Machine` / 可选头 magic / 子系统）。
///
/// 手写而不引依赖：要读的只有"文件头 + 可选头开头那几个 u16"，为它拉一个 PE 解析
/// crate 不成比例（本项目对"不新增依赖"有纪律，见 `shell-core` 的依赖守卫）。
/// ⚠️ 这里**只做范围检查**（每一处读之前先判长度），越界一律返回 `Err`——
///    坏文件必须是"大声失败"，不是 panic 在切片越界里（那样的话根因就说不清了）。
fn pe_facts(bytes: &[u8]) -> Result<PeFacts, String> {
    let u16_at = |off: usize| -> Result<u16, String> {
        let end = off.checked_add(2).ok_or("偏移溢出")?;
        let slice = bytes
            .get(off..end)
            .ok_or_else(|| format!("文件太短：读 0x{off:x} 处的 u16 时越界（共 {} 字节）", bytes.len()))?;
        Ok(u16::from_le_bytes([slice[0], slice[1]]))
    };
    let u32_at = |off: usize| -> Result<u32, String> {
        let end = off.checked_add(4).ok_or("偏移溢出")?;
        let slice = bytes
            .get(off..end)
            .ok_or_else(|| format!("文件太短：读 0x{off:x} 处的 u32 时越界（共 {} 字节）", bytes.len()))?;
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    };

    if bytes.get(..2) != Some(b"MZ") {
        return Err(format!(
            "开头不是 MZ（前两字节：{:02x?}）",
            bytes.get(..2).unwrap_or(&[])
        ));
    }
    // DOS 头 0x3C 处是 `e_lfanew`（PE 头的文件偏移）。
    let pe_off = u32_at(0x3c)? as usize;
    if bytes.get(pe_off..pe_off + 4) != Some(b"PE\0\0") {
        return Err(format!("0x{pe_off:x} 处不是 PE 签名"));
    }
    let coff = pe_off + 4;
    let machine = u16_at(coff)?;
    // COFF 头 16 字节 + 可选头：可选项就在 COFF 头之后。
    let optional = coff + 20;
    let magic = u16_at(optional)?;
    // 子系统在可选头里的偏移：PE32 与 PE32+ 都是 68（前面的字段宽度不同但总长一致）。
    let subsystem = u16_at(optional + 68)?;
    Ok(PeFacts {
        machine,
        magic,
        subsystem,
    })
}

/// 从 PE 字节里读出来的事实（本文件只用到这三个）。
struct PeFacts {
    machine: u16,
    magic: u16,
    subsystem: u16,
}

// ---------------------------------------------------------------------------
// 2) application manifest + 版本资源（W-7）
// ---------------------------------------------------------------------------

/// 产品名：与窗口标题 / macOS 的 `CFBundleDisplayName` **逐字相同**（规格 §9）。
const PRODUCT_NAME: &str = "Benagen 数据下载工具";
/// 公司名（版本资源里的一栏）。
const COMPANY_NAME: &str = "Benagen";

/// 生成 `.rc` → 用 `windres` 编成 COFF → 让链接器把它链进本 crate 的 exe。
///
/// ⚠️ **为什么在构建脚本里做、而不是在 `build_windows.sh` 里**：这一段的产物是
///    **exe 的一个节**（`.rsrc`），它只在链接那一刻才有意义；写在 shell 脚本里就变成
///    "先编出 exe、再拿 windres 拼进去"——那需要一个额外的改壳工具，而链路里
///    本来就有 windres（mingw 自带）。放在这里，`cargo build` 与脚本走的是**同一条**路。
fn link_manifest_and_version(out_dir: &Path) {
    // 版本号只有一处来源：构建脚本可以覆盖（`VERSION=0.2.0 bash build_windows.sh`，
    // 与 `macos/scripts/build_app_macos.sh` 的口径一致），否则回落 Cargo.toml 的版本。
    println!("cargo:rerun-if-env-changed=BENAGEN_VERSION");
    let version = std::env::var("BENAGEN_VERSION").unwrap_or_else(|_| {
        std::env::var("CARGO_PKG_VERSION")
            .expect("cargo 必须给 build script 设 CARGO_PKG_VERSION")
    });
    let quad = version_quad(&version).unwrap_or_else(|why| {
        panic!(
            "版本号 `{version}` 用不了：{why}\n\
             ⚠ 版本资源里的 FILEVERSION 是**四个 16 位整数**，所以版本号必须能拆成 a.b.c.d。\n\
             补救：用 `VERSION=0.1.0 bash windows/scripts/build_windows.sh` 这样的形式给一个合规的号。"
        )
    });

    let manifest = manifest_xml(&quad);
    let rc = rc_source(&quad);

    let manifest_path = out_dir.join("app.manifest");
    let rc_path = out_dir.join("app.rc");
    write_if_changed(&manifest_path, manifest.as_bytes());
    write_if_changed(&rc_path, rc.as_bytes());

    let res_path = out_dir.join("app.res");
    run_windres(&rc_path, &res_path);

    // ⚠️ **链进 bin**（不是 `rustc-link-arg`）：这个资源只该进交付的那个可执行文件。
    println!("cargo:rustc-link-arg-bins={}", res_path.display());
    eprintln!(
        "shell-win/build.rs: 已生成 application manifest + 版本资源（{version} → {}，{} 字节）",
        quad_string(&quad),
        std::fs::metadata(&res_path).map(|m| m.len()).unwrap_or(0)
    );
}

/// `"0.1.0"` / `"0.2"` / `"1.2.3.4"` → `[a, b, c, d]`（缺的补 0）。
///
/// ⚠️ 拒绝 `-` 后缀（`0.1.0-beta`）：那是**给人看的**语义化版本后缀，
///    FILEVERSION 只能装数字；悄悄截断会让"属性页里的版本号"与"我们说的版本号"对不上。
fn version_quad(version: &str) -> Result<[u16; 4], String> {
    let mut out = [0u16; 4];
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() > 4 {
        return Err(format!("有 {} 段，最多 4 段", parts.len()));
    }
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return Err(format!("第 {} 段是空的", i + 1));
        }
        let n: u16 = part
            .parse()
            .map_err(|_| format!("第 {} 段 `{part}` 不是 0..=65535 的十进制数", i + 1))?;
        out[i] = n;
    }
    Ok(out)
}

/// `[a, b, c, d]` → `"a.b.c.d"`（版本资源里的那个字符串形式）。
fn quad_string(q: &[u16; 4]) -> String {
    format!("{}.{}.{}.{}", q[0], q[1], q[2], q[3])
}

/// application manifest 的原文。
///
/// **两件事都在这里**：
///   * `supportedOS` = Windows 10 的 GUID（W-7）。⚠️ 这**不只是好看**：没有 manifest 的
///     exe 在部分 Windows 上会走**兼容性垫片**（`GetVersionEx` 之类的谎报），
///     行为与真实目标系统不一致（规格 §11.4）；
///   * `requestedExecutionLevel level="asInvoker"`：**不申请提权**。它是默认值，
///     写出来是为了让"我们没有要管理员权限"这件事**在产物里可读**
///     （自解压型 exe 被 SmartScreen 拦的概率本来就高，少一个"要提权"的信号是好事）。
///
/// ⚠️ **有意不写 `dpiAware` / `dpiAwareness`**（W-6 偏离，理由如下）：
///    那两项会改变窗口的 DPI 缩放行为，而**本机（macOS）无法验证**它 —— 探路包没有
///    manifest 也能跑，而 winit 在启动时会自己调 `SetProcessDpiAwarenessContext`
///    （进程已声明 DPI 感知时该调用是无害的 no-op）。⇒ 在真机验收之前**不加**
///    一个没人验过的行为改变。
fn manifest_xml(quad: &[u16; 4]) -> String {
    // 程序集版本也用四段式（manifest 的 assemblyIdentity@version 就要求 a.b.c.d）。
    let identity_version = quad_string(quad);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="BenagenDownloader" version="{identity_version}" processorArchitecture="amd64"/>
  <description>Benagen Downloader</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <!-- Windows 10（本产品的最低支持版本，W-7）。⚠ 这个 GUID 是 Win10 的
           （Win8.1 是 1f676c76-…、Win8 是 4a2f28e3-…）；写错的表现是
           "属性页里看不到、而兼容性垫片照旧生效" —— 没有东西会因此变红。 -->
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
</assembly>
"#
    )
}

/// `.rc` 的原文。
///
/// ⚠️ **`--codepage=65001` 是这个文件能被正确编译的前提**（`run_windres` 里带了它）：
///    本文件是 UTF-8，而 windres 的默认代码页（在中文 Windows 上是 936）会把
///    `数据下载工具` 编成乱码 —— 而**那种乱码不会让任何东西变红**（资源是二进制的，
///    编译期不报错、链接也不报错，只有人打开属性页才看得出来）。
///    ⇒ `build_windows.sh` 里那条"中文产品名确实在产物里"的字节检查就是为它准备的。
///
/// ⚠️ **`1 24`**：资源类型 24 是 `RT_MANIFEST`、ID 1 是"进程的 manifest"。
///    写成别的数字=这个 manifest 不会被 Windows 读，而产物**看起来完全正常**。
fn rc_source(quad: &[u16; 4]) -> String {
    let q = quad_string(quad);
    let comma = format!("{},{},{},{}", quad[0], quad[1], quad[2], quad[3]);
    format!(
        r#"// 由 shell-win/build.rs 生成，**不要手改**（改这里等于改构建脚本）。
1 24 "app.manifest"
VS_VERSION_INFO VERSIONINFO
 FILEVERSION {comma}
 PRODUCTVERSION {comma}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "{COMPANY_NAME}\0"
            VALUE "FileDescription", "{PRODUCT_NAME}\0"
            VALUE "FileVersion", "{q}\0"
            VALUE "InternalName", "BenagenDownloader\0"
            VALUE "OriginalFilename", "BenagenDownloader-Windows-x86_64.exe\0"
            VALUE "ProductName", "{PRODUCT_NAME}\0"
            VALUE "ProductVersion", "{q}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 0x4b0
    END
END
"#
    )
}

/// 跑 `windres`（mingw 自带）把 `.rc` 编成 COFF。
///
/// ⚠️ `-I <OUT_DIR>`：`.rc` 里的 `"app.manifest"` 是**相对**引用，windres 按它自己的
///    搜索路径找 —— 不显式给 `-I` 就会依赖调用者的 cwd（`cargo` 的 cwd 是哪，
///    不是本文件能决定的）。这一条在探路里踩过同形的坑：相对路径在 `cd` 之后解析到别处。
fn run_windres(rc: &Path, res: &Path) {
    // 工具名可覆盖（`BENAGEN_WINDRES`）：交叉编译器的前缀不一定长这样
    // （nixpkgs / MSYS2 各有各的命名），而"找不到 windres"必须是**一句能照着修的话**。
    let program = std::env::var("BENAGEN_WINDRES")
        .unwrap_or_else(|_| "x86_64-w64-mingw32-windres".to_string());
    println!("cargo:rerun-if-env-changed=BENAGEN_WINDRES");

    let out_dir = rc.parent().expect(".rc 一定在 OUT_DIR 里");
    let output = std::process::Command::new(&program)
        .arg("-I")
        .arg(out_dir)
        .arg("--codepage=65001")
        .arg("-O")
        .arg("coff")
        .arg(rc)
        .arg("-o")
        .arg(res)
        .output();
    match output {
        Err(cause) => panic!(
            "跑不了 windres（{program}）：{cause}\n\
             ⚠ 它是 mingw-w64 自带的资源编译器，用来把 application manifest 与版本资源\n\
               链进 exe（规格 W-7）。没有它就没有版本资源 —— 而那是**交付形态的一部分**。\n\
             ⚠ 本函数在 **Windows 宿主机上也会被跑到**（`cargo test -p shell-win` 的宿主靶就是
               windows ⇒ 本函数照样执行），所以那句 `brew install mingw-w64` 在那里**本身就是错的**。
             补救（按你的平台挑一行）：
             macOS: `brew install mingw-w64`
             Debian/Ubuntu: `apt-get install -y mingw-w64`
             Windows: 装 MSYS2（https://www.msys2.org），`pacman -S mingw-w64-x86_64-gcc`
               （它带来 x86_64-w64-mingw32-windres），再把 C:\\msys64\\mingw64\\bin 加进 PATH；
               ⚠ 要在 **Git Bash** 里跑（MSYS2 自带的 shell 会让 $HOME 指错地方）。
             或者设 BENAGEN_WINDRES 指向你那份 windres（名字不一样时用这一条）。"
        ),
        Ok(out) if !out.status.success() => panic!(
            "windres 失败（{program}，退出码 {:?}）：\n\
             stderr 原样：\n{}\n\
             补救：上面那段是 windres 自己的话；`--codepage=65001` 与 `-I` 是本脚本给的参数，\n\
               若它抱怨不认识某个参数，说明你那份 windres 太老。",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ),
        Ok(_) => {}
    }
}

/// 内容不同才写盘（幂等）：相同就别碰 mtime —— 那会触发无谓的重链。
fn write_if_changed(path: &Path, bytes: &[u8]) {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return;
    }
    std::fs::write(path, bytes)
        .unwrap_or_else(|e| panic!("写 {} 失败：{e}", path.display()));
}
