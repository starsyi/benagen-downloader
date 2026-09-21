#!/usr/bin/env bash
#
# windows/scripts/build_windows.sh —— 把 Windows 客户端**交叉编译出来、打包成单文件、并自验产物**（W-1）。
#
#       bash windows/scripts/build_windows.sh --check-only   → 编内核 + 编壳 + 自验，**不打包**，通过即退出 0
#       bash windows/scripts/build_windows.sh                → 上面那些 + **打包成单文件交付形态**
#
# 交付形态（规格 §7.2 / §11.4 / W-4）：`windows/dist/BenagenDownloader-Windows-x86_64.exe`
# —— **一个** exe，内核在它里面（`shell-win/build.rs` 内嵌 + `src/embed.rs` 释放）。
#
# ⚠️ **本脚本是"构建顺序"这件事的**唯一**属主**（任务 22）：壳把内核 exe 内嵌进自己，
#    而 cargo **不知道**这层依赖 ⇒ 必须先编内核、再编壳（第 2 步 / 第 3 步）。
#    顺序错了的表现是"内嵌了上一次构建的旧内核"，而**套件照样全绿**
#    （macOS 侧栽过同形的坑，见 `windows/scripts/test.sh` 头部那段 Ruling C19）。
#
# 为什么必须有这个脚本（而不是手敲 cargo 命令）
#   1) **交叉编译的配方有坑，而且踩过**：本机有两个 cargo —— `/opt/homebrew/bin/cargo`
#      只有 aarch64 的 std，拿它编 Windows 会报 `E0463`；必须用 `~/.cargo/bin/cargo`
#      （rustup shim），并且把 RUSTUP_HOME / CARGO_HOME / PATH 摆对。
#      这套环境变量是探路**实测**得出的（`docs/superpowers/2026-09-18-windows-spike.md` §2），
#      不是推断。把它写死在脚本里，比让每个人各自记得强。
#
#      ⚠️ **上面这一段是"在 macOS 开发机上"的实测**（2026-09-19 起补记）：在 Windows 上
#         **只有一个 cargo**（rustup 装的那份），所以"两个 cargo 抢 PATH"那段症状**不会出现**。
#         **但"用绝对路径调 rustup 的 cargo"这条纪律照旧** —— 它防的是"PATH 上恰好有另一份
#         cargo"，而不是"这台机器上一定有两份"；把纪律写成"只在这台 Mac 上成立"，下一次
#         换机器就会有人以为可以放开它。
#   2) **W-1：每一条构建路径都要自己验产物**。"cargo 退出 0" ≠ "产物是给 Windows 的 x86-64
#      GUI 程序"。参数传对与产出一个错的东西完全可以同时成立（macOS 侧栽过同形的坑：
#      `arch -x86_64 clang` 仍然编出 arm64）。所以下面第 4 步**只看字节**。
#   3) **打包那一段（任务 22）只能在**这里**做**：产物是"单个 exe"这件事
#      （内嵌内核 + manifest + 版本资源 + 三份许可）不是任何一条 cargo 命令的属性，
#      它是**组装**出来的 —— 组装顺序与组装后的自验都属于本脚本。
#
# ⚠️ **体积是一等公民**（W-4：客户要下的就是这一个文件的字节数）：本脚本第 2 步给内核
#    构建显式带上 `codegen-units=1`（去重内嵌的 aria2c），第 5 步把每一层的体积都打出来。
#    那一条的理由与实测数字见第 2 步的注释。
#
# 用法：
#   bash windows/scripts/build_windows.sh --check-only
#   bash windows/scripts/build_windows.sh
#
# 环境变量（都可选）：
#   VERSION=0.2.0   覆盖版本资源里的版本号（默认 0.1.0，与 `macos/scripts/build_app_macos.sh` 同口径）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
#   ⚠️ 用 `${BASH_SOURCE[0]}` 而不是 `$0`：相对路径的 `$0` 在下面的 `cd` 之后会解析到错的地方
#      （与 `test.sh` / `macos/scripts/*.sh` 同源）。

TARGET="x86_64-pc-windows-gnu"
# `[[bin]] name`（`shell-win/Cargo.toml`）——cargo 的产物名就是它 + `.exe`。
BIN_NAME="BenagenDownloader"
# 交付形态的名字（规格 §11.4：产物名自解释）。
DIST_NAME="BenagenDownloader-Windows-x86_64.exe"
# 交付形态落在哪儿：与 `macos/scripts/build_app_macos.sh` 同一个口径（`<平台>/dist/`，
# 且 `dist/` 是可再生产物、不入库）。
DIST="${REPO}/windows/dist"
DIST_EXE="${DIST}/${DIST_NAME}"

# 内核 exe 的产物路径 —— **由 `core/scripts/build_core_windows.sh` 钉住的那一个**
# （那个脚本显式设 `CARGO_TARGET_DIR`，产物不随调用者的 cwd 漂移）。
# ⚠️ `shell-win/build.rs` 读的是**同一个**路径（`CORE_EXE_REL`），两边必须一致：
#    对不上的表现是"壳内嵌了一个不存在的内核"，而那条会在构建期大声失败（不是静默）。
CORE_EXE="${REPO}/core/target/${TARGET}/release/benagen-core.exe"

# 版本号（规格 W-7：版本资源里要能看到版本号）。默认 0.1.0 与 macOS 侧同口径。
# ⚠️ 它同时喂给 `shell-win/build.rs`（写进 .rc）与第 5 步的自验（grep 交付形态里的那一串），
#    两边用的是**同一个**变量 —— 不然"自验"就会变成"拿脚本自己编的数去核自己"。
BENAGEN_VERSION="${VERSION:-0.1.0}"
export BENAGEN_VERSION

# ---- 参数：只认 --check-only，别的**直接报错退出**（exit 2）-------------------
# 与 `test.sh` 同一条纪律：未知参数**不透传**。透传会让脚本以一个"看起来像失败"的
# 退出码收场，而根因是打错了字——那样的红与没有验收等价。
check_only=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check-only) check_only=1; shift ;;
    *)
      echo "build_windows.sh: 不认识的参数：$1" >&2
      echo "build_windows.sh: 现在只接受 --check-only（编内核 + 编壳 + 自验，不打包）。" >&2
      exit 2
      ;;
  esac
done

# ---- 1) 工具链 preflight（W-2：缺什么就大声说，并给出可执行的补救）-----------
#
# ⚠️ **绝不用 PATH 上的 `cargo`**：本机有两个，`/opt/homebrew/bin/cargo` 只有 aarch64 的 std，
#    用它会得到一句 `E0463: can't find crate for std`——那句错误**不指向真正的根因**
#    （看起来像"目标没装"，实际是"用错了 cargo"）。
#
#    ⚠️ **这条实测同样是 macOS 开发机上的**（2026-09-19 起补记）：Windows 上只有一个 cargo，
#       上面那段症状**不会出现** —— **但"用绝对路径调 rustup 的 cargo"这条纪律照旧**
#       （它防的是"PATH 上恰好有另一份"，不是"这台机器上一定有两份"）。
#       ⚠️ 而且在 Windows 上这条纪律**更要紧**：下面那个 `${HOME}/.cargo/bin/cargo` 在
#       MSYS2 自带的 shell 里会落到 `/home/<你>/.cargo/bin/cargo` —— 见失败分支里的说明。
CARGO="${HOME}/.cargo/bin/cargo"
if [ ! -x "${CARGO}" ]; then
  echo "build_windows.sh: 错误：找不到 rustup 的 cargo：${CARGO}" >&2
  echo "build_windows.sh: 本脚本**故意**用绝对路径调它，不用 PATH 上的 cargo ——" >&2
  echo "build_windows.sh: 在 macOS 上，PATH 上那份很可能是 /opt/homebrew/bin/cargo，它只有 aarch64 的" >&2
  echo "build_windows.sh:   std，编 Windows 会报 E0463，而那句错误的根因看起来完全不是'用错了 cargo'。" >&2
  echo "build_windows.sh: ⚠️ 这个路径在 Windows 上查不到，先看你在哪个 shell 里（两种都解释得通）：" >&2
  echo "build_windows.sh:   · **MSYS2 自带的 shell**：它的 \$HOME 是 /home/<你>（MSYS2 自己的家目录），" >&2
  echo "build_windows.sh:     而 rustup 装的东西在 C:\\Users\\<你>\\.cargo ⇒ 上面那个路径会解析成" >&2
  echo "build_windows.sh:     /home/<你>/.cargo/bin/cargo，**那里没有东西**。" >&2
  echo "build_windows.sh:   · **Git Bash**：它的 \$HOME 就是 %USERPROFILE%（= C:\\Users\\<你>）⇒ 这个路径" >&2
  echo "build_windows.sh:     **是对的**；还找不到就只剩'rustup 没装'或'装到了别处'两种可能。" >&2
  echo "build_windows.sh: 补救（按平台挑一行）：" >&2
  echo "build_windows.sh:   macOS:         装 rustup（https://rustup.rs），或把 CARGO 指到你的 rustup shim 上。" >&2
  echo "build_windows.sh:   Debian/Ubuntu: 装 rustup（https://rustup.rs）—— 发行版自带的 cargo 不经 rustup，" >&2
  echo "build_windows.sh:                  本脚本要的是 rustup 管的 \$HOME/.cargo/bin/cargo。" >&2
  echo "build_windows.sh:   Windows:       装 rustup（https://rustup.rs），然后**在 Git Bash 里**跑本脚本" >&2
  echo "build_windows.sh:                  （MSYS2 自带的 shell 会让 \$HOME 指错地方，见上）。" >&2
  exit 1
fi
export RUSTUP_HOME="${HOME}/.rustup"
export CARGO_HOME="${HOME}/.cargo"
export PATH="${HOME}/.cargo/bin:${PATH}"

# 目标 std 必须真的装了 —— 否则下面那次 cargo build 会以 `E0463` 失败，
# 而那条错误的措辞会把读者引向"代码有问题"。
if ! "${CARGO}" --version >/dev/null 2>&1; then
  echo "build_windows.sh: 错误：${CARGO} --version 跑不起来（rustup shim 坏了？）。" >&2
  echo "build_windows.sh: 补救：跑一次 \`${CARGO} --version\` 看它的原话；必要时重装 rustup。" >&2
  exit 1
fi
if ! "${HOME}/.cargo/bin/rustup" target list --installed 2>/dev/null | grep -qx "${TARGET}"; then
  echo "build_windows.sh: 错误：rustup 里没有装目标 ${TARGET} 的 std。" >&2
  echo "build_windows.sh: 补救：\`${HOME}/.cargo/bin/rustup target add ${TARGET}\`" >&2
  exit 1
fi

# mingw-w64：链接器（cargo 用得到）与自验工具（`objdump`）。
# `file` 是 macOS 自带的，但**仍然显式检查**——它缺席时第 4 步会以一句
# "command not found" 收场，而那看起来像脚本坏了，不像工具缺失。
missing_tools=""
for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-objdump x86_64-w64-mingw32-windres file; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    missing_tools="${missing_tools} ${tool}"
  fi
done
# sha256 工具：两个名字各平台不一样（macOS 自带 shasum，Linux 一般是 sha256sum）。
# ⚠️ **它缺席也算缺工具**：第 2 步与第 5 步都要拿它当判据（"包里那份内核确实是这一份"），
#    悄悄跳过那条判据等于把本脚本最要紧的一次自验变成一句"看着没问题"。
if ! command -v shasum >/dev/null 2>&1 && ! command -v sha256sum >/dev/null 2>&1; then
  missing_tools="${missing_tools} shasum|sha256sum"
fi
if [ -n "${missing_tools}" ]; then
  echo "build_windows.sh: 错误：缺工具：${missing_tools}" >&2
  echo "build_windows.sh: 交叉编译到 ${TARGET} 要 mingw-w64 的链接器与 windres，" >&2
  echo "build_windows.sh: 自验要 objdump、file 与一个 sha256 工具。" >&2
  echo "build_windows.sh: 补救（按平台挑一行）：" >&2
  echo "build_windows.sh:   macOS:         \`brew install mingw-w64\`（file 与 shasum 是系统自带的；" >&2
  echo "build_windows.sh:                  file 缺失说明这台机器不寻常，先查 PATH）。" >&2
  echo "build_windows.sh:   Debian/Ubuntu: \`apt-get install -y mingw-w64 file\`（sha256 用 sha256sum，随 coreutils）。" >&2
  echo "build_windows.sh:   Windows:       装 MSYS2（https://www.msys2.org），然后" >&2
  echo "build_windows.sh:                  \`pacman -S mingw-w64-x86_64-gcc\`（带来 x86_64-w64-mingw32-gcc /" >&2
  echo "build_windows.sh:                  -objdump / -windres 三个）与 \`pacman -S file\`；" >&2
  echo "build_windows.sh:                  再把 C:\\msys64\\mingw64\\bin 加进 PATH。" >&2
  echo "build_windows.sh:                  ⚠️ **本脚本要在 Git Bash 里跑**，不是 MSYS2 自带的 shell。" >&2
  echo "build_windows.sh: ⚠️ 这里**不降级**：没有 objdump 就没法验产物是不是 PE（W-1），" >&2
  echo "build_windows.sh:    而'编出来了但没验'正是本脚本要防的那件事。" >&2
  exit 1
fi
echo "build_windows.sh: 工具链就绪（$("${CARGO}" --version)，目标 ${TARGET}）" >&2

# 算 sha256（两个名字各平台不一样）。
#
# ⚠️ 输出统一成"只有那 64 个十六进制字符、小写"：下面两处判据都是拿它去 `grep -F` 比字面量，
#    两种工具的输出格式（`<sha>  <文件>` vs `<sha> *<文件>`）不一样，直接内插会带上文件名，
#    判据就恒不成立了（而且看起来像"内嵌的不是这份内核"——**根因说错**）。
sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

# ---- 2) **先编内核**（Windows 靶）—— 顺序是硬约束，见文件头 -------------------
#
# ⚠️ 这一步**必须**在编壳之前。壳用 `include_bytes!` 内嵌内核 exe，而 cargo 不知道
#    这层依赖：顺序反了会**安静地**内嵌"上一次构建的旧内核"，套件照样全绿
#    （macOS 侧那次的形态）。所以顺序在这里定，而 `shell-win/build.rs` 负责
#    "内核不在场就大声失败 + 把它的 sha256 记成编译期常量"（第 5 步再拿那个常量核对）。
#
# 内核自己那条构建路径（`core/scripts/build_core_windows.sh`，流 K 的产物）**自带 W-1 自验**
# （PE 头 / 架构 / strip / 体积），本脚本**不**重写它 —— 同一条构建路径有两份实现，
# 迟早分叉。这里只负责"排顺序"与"把体积决策传下去"。
#
# ⚠️ **`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`：本任务（任务 22）的体积决策，去重。**
#    实测（任务 5 的审查，2026-09-18）：rustc 按 **codegen unit** 复制 `include_bytes!`
#    常量 ⇒ 内嵌的 aria2c（5.65 MB）在产物里出现**两份且逐字节相同**，占全文件 63%；
#    `-C codegen-units=1` 之后只剩一份（**实测 17,878,185 → 11,937,653，−5.94 MB**）。
#    而这份内核 exe 会被壳**再内嵌一次**（本任务的第 3 步）⇒ 那 5.94 MB 会**原样传进
#    交付包**（W-4：客户要下的字节数是一等公民）。
#    代价与理由：代价是**构建时间**（codegen 少并行，且换配置会触发一次全量重编），
#    而"编译一次、分发很多次"的流水线里那是**一次性**的；换来的是每个客户少下约 6 MB。
#    ⚠️ 这条**只**在本脚本这条路径上生效（`core/scripts/build_core_windows.sh` 单独跑
#    仍是默认配置）—— 两处口径不同这件事是**有意的**（这条决策属于打包这一步），
#    如实记账在这里。若将来要全局生效，正确的家是 `core/Cargo.toml` 的 `[profile.release]`。
if [ ! -f "${REPO}/core/scripts/build_core_windows.sh" ]; then
  echo "build_windows.sh: 错误：找不到内核的 Windows 构建脚本：core/scripts/build_core_windows.sh" >&2
  echo "build_windows.sh: 本脚本不重写它（同一条构建路径有两份实现迟早分叉），而是调用它。" >&2
  echo "build_windows.sh: ⚠️ 缺它意味着这棵树上**内核的 Windows 维度还没进来**——" >&2
  echo "build_windows.sh:    本任务（T22）依赖流 K 的产出（跨流依赖：K 必须先合入）。" >&2
  echo "build_windows.sh: 补救：确认 phase4-win-kernel（或它已合入的那条集成分支）在本次检出里；" >&2
  echo "build_windows.sh:   若在并行工作树里干活，请把 K 合进来再跑本脚本（不要在本脚本里造第二份构建实现）。" >&2
  exit 1
fi
echo "build_windows.sh: 先编内核（core/scripts/build_core_windows.sh，codegen-units=1 去重内嵌 aria2c）" >&2
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 bash "${REPO}/core/scripts/build_core_windows.sh"

# W-1：**这一步的产物自己验一遍**（不靠上一条命令的退出码说话）。
#   上面那个脚本已经验过一遍；这里再验一次是因为**本脚本**接下来要把它当输入用
#   （内嵌），"输入存在且是 PE" 这件事必须在**消费点**也成立 —— 中间任何一步（清理、
#   换分支、别的脚本删产物）都会让它不成立，而那时下面那次 cargo 会以
#   "内嵌内核失败：读不到内核 exe" 的形态失败（响亮，但根因会指向 build.rs）。
if [ ! -f "${CORE_EXE}" ]; then
  echo "build_windows.sh: 错误：内核构建结束，但产物不在 ${CORE_EXE}。" >&2
  echo "build_windows.sh: 期望的路径由目标三元组 + core/Cargo.toml 的 [[bin]] name 拼出来。" >&2
  echo "build_windows.sh: 补救：手工跑一次 \`bash core/scripts/build_core_windows.sh\` 看它的原话。" >&2
  exit 1
fi
core_file_line="$(file -b "${CORE_EXE}")"
case "${core_file_line}" in
  *PE32+*x86-64*) : ;;
  *)
    echo "build_windows.sh: 错误：内核产物不是 PE32+ x86-64。file 的原话：${core_file_line}" >&2
    echo "build_windows.sh: 补救：核对 core/scripts/build_core_windows.sh 用的 --target。" >&2
    exit 1
    ;;
esac
core_sha="$(sha256_of "${CORE_EXE}")"
core_size="$(wc -c < "${CORE_EXE}" | tr -d ' ')"
echo "build_windows.sh: ✓ 内核：PE32+ x86-64，${core_size} B，sha256 ${core_sha}" >&2

# ---- 3) 交叉编译壳 ----------------------------------------------------------
# ⚠️ 工作区根在 `${REPO}/windows`（不是仓库根）：它的 Cargo.toml 才是那个 [workspace]。
echo "build_windows.sh: 交叉编译 -p shell-win（--release，目标 ${TARGET}）" >&2
(cd "${REPO}/windows" && "${CARGO}" build --release -p shell-win --target "${TARGET}")

# ⚠️ **`#[cfg(windows)]` 的那一半代码，只有这条命令能在本机被编译到。**
#    本机是 macOS：`cargo test -p shell-win`（宿主）编不到那些分支，于是
#    "Windows 上编不过"会一直躲到交叉编译才现形，甚至躲到真机上才现形。
#    实测抓到过一条：`CommandExt::get_creation_flags()` **不存在**（std 只有 setter），
#    宿主测试全绿、这条 check 当场红。
echo "build_windows.sh: 编译检查（--tests）——保证 #[cfg(windows)] 的那些分支真的编得过" >&2
(cd "${REPO}/windows" && "${CARGO}" check --tests -p shell-win --target "${TARGET}")

# ---- 4) W-1：只看字节，不信参数 -------------------------------------------
EXE="${REPO}/windows/target/${TARGET}/release/${BIN_NAME}.exe"
if [ ! -f "${EXE}" ]; then
  echo "build_windows.sh: 错误：构建结束，但产物不在 ${EXE}。" >&2
  echo "build_windows.sh: 期望的路径由目标三元组 + [[bin]] name 拼出来（${BIN_NAME}.exe）。" >&2
  echo "build_windows.sh: 常见原因一：shell-win/Cargo.toml 的 [[bin]] name 被改名。" >&2
  echo "build_windows.sh: 常见原因二：构建被别的目标接管，产物落到 windows/target/<别的三元组>/。" >&2
  echo "build_windows.sh: 补救：核对上面两处，或把本脚本的路径一并改掉。" >&2
  exit 1
fi

file_line="$(file -b "${EXE}")"
echo "build_windows.sh: file → ${file_line}" >&2

# 3a) 架构与位数：必须是 PE32+ / x86-64。
#     `file` 认不出来的情况（沿用脚本的判据）会给出 "data" 之类——一并算失败。
case "${file_line}" in
  *PE32+*x86-64*) : ;;
  *)
    echo "build_windows.sh: 错误：${EXE} 不是 PE32+ x86-64。file 的原话：${file_line}" >&2
    echo "build_windows.sh: 补救：确认第 3 步用的是 --target ${TARGET}（产物应落在 target/${TARGET}/ 下）。" >&2
    exit 1
    ;;
esac
echo "build_windows.sh: ✓ 架构：PE32+ x86-64" >&2

# 3b) **子系统必须是 GUI** —— 这是落点①（壳自己的控制台窗口）在本机**唯一**能被验到的形态。
#
#     `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 的效果，
#     字节层面就看得出：console 子系统的 PE 会被 `file` 报成 "(console)"，
#     GUI 的是 "(GUI)"。**这不是推断，是探路实测的对照**：
#     探路包**有意**没设那个 attribute，`file` 报的正是 "PE32+ executable (console)"
#     （spike §4.1），而它跑起来就有一个黑色控制台窗口（spike §6.1）。
#     ⇒ 这次是 release 构建，必须是 (GUI)；报成 (console) 就是"落点①没生效"。
case "${file_line}" in
  *"(GUI)"*) : ;;
  *"(console)"*)
    echo "build_windows.sh: 错误：产物是 **console 子系统**——双击会带出一个黑色控制台窗口。" >&2
    echo "build_windows.sh: 根因：shell-win/src/main.rs 顶上的" >&2
    echo "build_windows.sh:   #![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]" >&2
    echo "build_windows.sh: 没生效（被删了？被别处覆盖了？或者这次编的是 debug？）。" >&2
    echo "build_windows.sh: ⚠️ debug 构建**有意**是 console（好让 panic 看得见），" >&2
    echo "build_windows.sh:    所以只有 --release 才要求 (GUI)。" >&2
    exit 1
    ;;
  *)
    echo "build_windows.sh: 错误：读不出子系统（file 的原话：${file_line}）。" >&2
    echo "build_windows.sh: 判据是 (GUI) / (console)；读不出就没法确认落点①，不能放行。" >&2
    exit 1
    ;;
esac
echo "build_windows.sh: ✓ 子系统：GUI（落点①：壳自己不带控制台窗口）" >&2

# 3c) 依赖 DLL：**只许可系统 DLL**（那才让"单个 exe、自包含"这句话成立）。
#
#     两道网，缺一不可：
#       (1) **黑名单**：mingw 的三个运行时 DLL + `WebView2Loader.dll`，出现即硬失败。
#           前三个是"漏了静态链接"，最后一个（Tauri 那一代加的）是"**单文件交付被破坏**"
#           —— 根因不同、补救完全不同，所以下面的 `case` 给它们各自一段话；
#           而 `WebView2Loader.dll` 那一条是本代**最容易悄悄坏掉**的一条（规格 §8.2）：
#           补丁没了或升版忘了重新 vendor，**在 macOS 上构建照样成功**；
#       (2) **白名单**：不认识的 DLL 也硬失败。理由：一个新引入的非系统依赖
#           （比如某个 crate 拖来的 zlib1.dll）如果只是被"打印出来"，没有人会去看；
#           而它到了客户机器上就是"缺 DLL 起不来"。要放行就**显式**把名字加进白名单——
#           那是一次能被审查的决定，而不是一次静默的漂移。
#
# ⚠️ 两张表里写的是**不带 `.dll` 后缀**的名字，而 objdump 打出来的**带**后缀——
#    所以比较之前统一把后缀剥掉（下面 `${low%.dll}`）。这条注释是**踩过之后**才写的：
#    第一版白名单写成带后缀的名字、比较时又没剥，于是**每一个** DLL 都被判成"不认识"，
#    报错信息却长得像"新引入了一堆依赖"——**根因说错了**（本项目对这条有纪律）。
#    下面那段"守卫的守卫"就是为此加的。
# ⚠️ `webview2loader` 在这张表里是**单独一档**（Tauri 那一代加的）：它与那三个 mingw
#    运行时**不是同一类东西** —— 后者是"漏了静态链接"，前者是"**单文件交付被破坏**"
#    （规格 §4 / §8.2 第 2 条，本代最容易悄悄坏掉的一条）。下面那个 `case` 给它单独一段话，
#    因为把根因说成"依赖了 mingw 运行时"**是错的**（本项目对"根因不许说错"有纪律）。
#    ⇒ 它进这张表的意义是"**同一条判据只有一处清单**"，措辞由下面的 `case` 分派。
deny_dlls="libgcc_s_seh-1 libgcc_s_dw2-1 libwinpthread-1 libstdc++-6 webview2loader"
allow_dlls="kernel32 user32 gdi32 opengl32 shell32 shlwapi ole32 oleaut32 uiautomationcore \
dwmapi imm32 uxtheme bcryptprimitives advapi32 bcrypt ws2_32 userenv ntdll msvcrt wsock32 \
iphlpapi secur32 crypt32 oleacc comctl32 comdlg32 winmm psapi version setupapi sechost \
msimg32 d3d9 dxgi d3d11 glu32 ucrtbase propsys"

dll_base() { printf '%s' "$1" | tr 'A-Z' 'a-z' | sed 's/\.dll$//'; }

# 依赖 DLL 的判据本体 —— **一处实现、两个调用点**（任务 15 步骤 2 把第二处补上）。
#
# ⚠️ **为什么要两处**（这是任务 15 补第二处的原因，不是"多跑一遍更保险"）：
#    第 4 步那次判的是 **cargo 的构件**（`windows/target/…/BenagenDownloader.exe`），
#    而**客户拿到的是 `windows/dist/` 里那一个**。在此之前，交付形态那一段（5a）
#    只判了 PE/子系统/manifest/版本/许可 —— **导入表不在其中**。
#    "两者今天逐字节相同"是真的，但那是**这一条链路今天的性质**，不是判据：
#    打包那一段将来多一步（压缩、签名、改壳），"构件没有非系统依赖"与
#    "**交付字节**没有非系统依赖"就变成两个不同的命题了。
#    ⇒ 同一个函数在两个点各跑一次，判据只有一份实现（改一处两处一起走）。
#
# ⚠️ **`$1` = 被查的文件、`$2` = 报告里用的名字**（"构件"/"交付形态"）：出错时那句
#    "哪条路径上的哪个文件"必须说对，否则人会去查错的那一个。
check_dependency_dlls() {
  dep_file="$1"; dep_label="$2"
  dlls="$(x86_64-w64-mingw32-objdump -p "${dep_file}" | awk '/^[[:space:]]*DLL Name:/ {print $3}')"
  if [ -z "${dlls}" ]; then
    echo "build_windows.sh: 错误：objdump 没能列出 ${dep_label} 的任何依赖 DLL —— 自验没跑成，不能放行。" >&2
    echo "build_windows.sh: 文件：${dep_file}" >&2
    echo "build_windows.sh: 补救：手工跑 \`x86_64-w64-mingw32-objdump -p ${dep_file}\` 看它的原话。" >&2
    exit 1
  fi
  echo "build_windows.sh: ${dep_label}的依赖 DLL（objdump -p 原样）：" >&2
  printf 'build_windows.sh:   - %s\n' ${dlls} >&2

  # 「守卫的守卫」：拿一个**一定在**的系统 DLL 走一遍匹配逻辑。
  # 匹配逻辑自己坏掉时（比如后缀没剥），下面会以"出现了不认识的依赖 DLL"报错——
  # 那是**错误的根因**。这里先自证一遍，让根因说对。
  guard_probe="$(dll_base KERNEL32.dll)"
  guard_ok=0
  for ok in ${allow_dlls}; do
    if [ "${guard_probe}" = "${ok}" ]; then guard_ok=1; break; fi
  done
  if [ "${guard_ok}" -ne 1 ]; then
    echo "build_windows.sh: 错误：自验白名单的匹配逻辑本身坏了（KERNEL32.dll 没匹配上任何一项）。" >&2
    echo "build_windows.sh: 这**不是**产物的依赖有问题，是本脚本这一段写错了（例如 .dll 后缀没剥）。" >&2
    echo "build_windows.sh: 若不先自证，下面会以'出现了不认识的依赖 DLL'报错——那是错误的根因。" >&2
    exit 1
  fi

  unknown_dlls=""
  for dll in ${dlls}; do
    low="$(dll_base "${dll}")"
    # (1) 黑名单：出现即硬失败。**措辞按名字分派**（清单是一张，根因是两种）。
    for bad in ${deny_dlls}; do
      if [ "${low}" = "${bad}" ]; then
        case "${low}" in
          webview2loader)
            echo "build_windows.sh: 错误：${dep_label}依赖了 WebView2Loader.dll —— 单文件交付被破坏。" >&2
            echo "build_windows.sh: 文件：${dep_file}" >&2
            echo "build_windows.sh: 根因：webview2-com-sys 只对 target_env=msvc 静态链接，gnu 目标链成动态导入。" >&2
            echo "build_windows.sh: 补救：确认 windows/vendor/webview2-com-sys 的补丁在（src/lib.rs 的宏里" >&2
            echo "build_windows.sh:       not(target_env=\"msvc\") 那条 #[link] 必须已被删掉），" >&2
            echo "build_windows.sh:       且 windows/Cargo.toml 的 [patch.crates-io] 生效" >&2
            echo "build_windows.sh:       （后一半宿主构建就会红：shell-win/build.rs 的" >&2
            echo "build_windows.sh:        read_and_check_patch_is_in_effect 会查 Cargo.lock 里那条有没有 source/checksum）。" >&2
            echo "build_windows.sh:       详细根因见 docs/superpowers/2026-09-20-tauri2-windows-crosscompile.md §3。" >&2
            echo "build_windows.sh: ⚠️ 这一条**在 macOS 上构建照样成功**（补丁没了不会报任何错）——" >&2
            echo "build_windows.sh:    所以它是这条交付形态在本机唯一拦得住它的判据，别绕过它。" >&2
            ;;
          *)
            echo "build_windows.sh: 错误：${dep_label}依赖了 mingw 运行时 DLL：${dll}" >&2
            echo "build_windows.sh: 文件：${dep_file}" >&2
            echo "build_windows.sh: 这会让'单个 exe、客户机器上不用装任何东西'这句话不成立。" >&2
            echo "build_windows.sh: 补救：静态链接（RUSTFLAGS 里加 -C target-feature=+crt-static，或" >&2
            echo "build_windows.sh:   给该 crate 关掉动态运行时的特性）。**不要**改成把 DLL 一起发。" >&2
            ;;
        esac
        exit 1
      fi
    done
    # (2) 白名单：前缀（系统 API set 与 CRT）+ 名单内的系统 DLL。
    case "${low}" in
      api-ms-win-*|ext-ms-win-*) continue ;;
    esac
    known=0
    for ok in ${allow_dlls}; do
      if [ "${low}" = "${ok}" ]; then known=1; break; fi
    done
    if [ "${known}" -eq 0 ]; then
      unknown_dlls="${unknown_dlls} ${dll}"
    fi
  done

  if [ -n "${unknown_dlls}" ]; then
    echo "build_windows.sh: 错误：${dep_label}出现了不认识的依赖 DLL：${unknown_dlls}" >&2
    echo "build_windows.sh: 文件：${dep_file}" >&2
    echo "build_windows.sh: 两种可能，都要人来判：" >&2
    echo "build_windows.sh:   ① 它是这次改动**新引入的非系统依赖** —— 那正是这道网要抓的东西" >&2
    echo "build_windows.sh:      （客户机器上没有它 ⇒ '缺 DLL 起不来'）；" >&2
    echo "build_windows.sh:   ② 它是合法的系统 DLL，只是白名单还没有它 —— 那就把名字**显式**加进" >&2
    echo "build_windows.sh:      本脚本的 allow_dlls，让这次放宽成为一次能被审查的决定。" >&2
    exit 1
  fi
  echo "build_windows.sh: ✓ ${dep_label}的依赖：只有系统 DLL，没有 mingw 运行时、没有 WebView2Loader（自包含）" >&2
}

check_dependency_dlls "${EXE}" "构件"

# ---- 4d) **可复现性：同一棵树连跑两次，交付字节必须逐字节相同**（任务 15 步骤 1b）---
#
# ⚠️ **这条判据治的是一个实测出来的缺陷**（2026-09-20，任务 15）：
#    同一棵树连跑两次本脚本 ⇒ **体积一模一样（35,407,071 B）、sha256 不同**
#    （`b1f177c5…` vs `3bd634a8…`）。⇒ 交付形态**不可复现** ⇒
#    "**发出去的是哪一个**"这个问题答不出来 —— 而真机验收清单上是要写 sha256 的。
#
#    根因不是内容：`x86_64-w64-mingw32-objdump -p` 的 `Time/Date` 行**每次都是链接那一刻**
#    —— mingw 的 `ld` 默认往 PE 的 COFF 头写 `TimeDateStamp`。
#    `shell-win/build.rs` 的 `pin_link_timestamp()` 发一条 `-Wl,--no-insert-timestamp`
#    把它关掉（写 0），于是**同样的输入 ⇒ 同样的字节**。
#
# ⚠️ **为什么这条判据要"真的再链一次"而不是只 grep 一下构建脚本**：
#    只查"那一行还在不在"是**语法**判据，它挡不住"开关还在、但它不生效"这一类
#    （比如 `-Wl,` 前缀丢了、换了个不认这个选项的链接器驱动、或 ld 版本变了）。
#    "可复现"是个**性质**，判它的唯一硬证据是**真的产出两份字节去比**。
#    成本实测：删掉产物再 `cargo build` 只重跑**最后一次链接**（约 0.2 秒；
#    若 cargo 认为需要重编，本机实测 13–25 秒 —— 一次性，不是常态）。
#
# ⚠️ **为什么删掉产物能逼出第二次链接**：cargo 的"新鲜"判定里含**产物文件在不在**，
#    文件没了它就重跑 rustc（这里只剩链接阶段是实质工作）。**这不是启发式**：
#    下面第一件事就是确认"重链之后产物确实又出现了"，没出现就大声失败
#    （那说明这条路子失效了，判据会变成"拿一个不存在的文件和它自己比"= 恒真）。
#
# ⚠️ **比较的是 `${EXE}` 而不是最终 dist 那个**：dist 是它的逐字节副本（第 5 步 `cp`），
#    在这里比较能**在打包之前**就红 —— 半成品不会进 `dist/`。
#
# ⚠️⚠️ **本步判的是「同一条构建路径（就是本脚本）重复跑稳不稳」，不是「可复现」这四个字
#    的全部**（审查 2026-09-20 实测）：拿**裸 `cargo build`**（环境与脚本这次不同）去链，
#    会得到一个**同长度、不同字节**的 exe —— 机理**未查清**，但**不是时间戳**
#    （两种产物的 `Time/Date` 都是 0）。⇒ 这一条的措辞在 README §0 里写的是
#    "**同环境可复现**"。**别把它读成"换一种构建方式也得到同一份字节"。**
#
# ⚠️⚠️ **两次链接之间必须隔开一秒以上（`sleep`），否则这条判据没有判别力** ——
#    这不是"保险起见"，是**实测**出来的：本判据的第一版**没有** sleep，而作者拿
#    "把 `pin_link_timestamp()` 摘掉"做变异测试时它**报了绿**（🔑 见下面"第一次"那一段）
#    （`EXIT=0`，两次链接的 `Time/Date` 都落在**同一秒** `12:08:58` ⇒ 字节当然相同）。
#    ⇒ 这条判据检验的性质是"**链接产物不依赖它是在哪一刻跑的**"，
#      而一个"两次都发生在同一刻"的实验**结构上问不出这件事**。
#    ⇒ `sleep 2`（PE 的 `TimeDateStamp` 精度是**秒**，隔 2 秒足以跨过任何舍入）。
#      代价 2 秒；换来的是"这条判据真的会响"。
#
# ⚠️ 本节还单独判一次 **PE 头里的 `TimeDateStamp` 必须为 0**（4d-1）。它与下面那次
#    字节比较**不是一回事**：字节比较是"性质"，但它只在**开关失效**时才红，
#    而红的时候要靠人去读 `objdump` 才知道根因；4d-1 直接把根因说出来，且**不依赖时序**。
#
# 🔑 **这是本代第一条「判据自身被变异验过」的判据**（审查 2026-09-20 的原话）：
#    在此之前本代所有"我们加了条判据"的说法，验的都是"它现在是绿的"；
#    这一条验的是"**把它要抓的那个变异放进去，它真的会红**"
#    （摘掉 `pin_link_timestamp()` ⇒ 4d-1 当场红；而且**第一版没有 `sleep 2` 时它报绿**，
#    所以那条 sleep 是这次变异逼出来的，不是抄来的经验）。
#    ⇒ 判据的判别力**不是靠读代码确认的，是靠一次变异确认的**。
pe_ts_hex() {
  # PE 的 COFF 头 TimeDateStamp（链接时刻）那 4 个字节的**原样十六进制**。
  # 偏移：DOS 头 0x3C 处是 PE 签名的文件偏移（4 字节小端），签名 4 字节之后是 COFF 头，
  #       而 COFF 头的头 4 字节是 Machine、再 2 字节 NumberOfSections ⇒ 时间戳在 pe_off+8。
  # ⚠️ 走**字节**而不是 `od -tu4`：`-tu4` 按本机字节序解释，换台大端机器读数就反了，
  #    而"是不是全 0"这件事在字节层面**与字节序无关**（0 的反序还是 0）。
  local b off
  b="$(od -An -tx1 -j60 -N4 "$1" | tr -d ' \n')"
  if [ "${#b}" -ne 8 ]; then
    echo "build_windows.sh: 错误：读不出 ${1} 的 PE 头偏移（文件太短？不是 PE？）。" >&2
    exit 1
  fi
  off=$(( 0x${b:6:2}${b:4:2}${b:2:2}${b:0:2} ))
  od -An -tx1 -j$(( off + 8 )) -N4 "$1" | tr -d ' \n'
}

relink_ts="$(pe_ts_hex "${EXE}")"
if [ "${relink_ts}" != "00000000" ]; then
  echo "build_windows.sh: 错误：交付 exe 的 PE 头里写进了**链接时刻**（TimeDateStamp=${relink_ts}，不是 0）。" >&2
  echo "build_windows.sh: 后果：这份 exe **不可复现** —— 同一棵树换个时刻再构建一次就是另一个字节，" >&2
  echo "build_windows.sh:   于是「发出去的是哪一个」答不出来（真机验收清单上要写 sha256）。" >&2
  echo "build_windows.sh: 根因：mingw 的 ld 默认往 PE 的 COFF 头写链接时刻；关掉它的是" >&2
  echo "build_windows.sh:   shell-win/build.rs 的 pin_link_timestamp()（\"-Wl,--no-insert-timestamp\"）。" >&2
  echo "build_windows.sh: 补救：把它加回去；若是换了链接器驱动导致 -Wl, 中转失效，见那个函数的注释。" >&2
  exit 1
fi
echo "build_windows.sh: ✓ 可复现（4d-1）：PE 头的链接时刻是 0（ld 的 --no-insert-timestamp 生效）" >&2

relink_exe="${EXE}"
relink_keep="$(mktemp)"
# ⚠️ 这两个临时文件要跟着 5) 那条既有的 trap 一起收（它此刻还没装 —— 所以本段自带一个
#    只活到本段结束的 try/finally 形状：无论走哪条分支都删掉自己的临时文件）。
rm -f "${relink_keep}"
cp "${relink_exe}" "${relink_keep}"
first_sha="$(sha256_of "${relink_keep}")"
first_size="$(wc -c < "${relink_keep}" | tr -d ' ')"
rm -f "${relink_exe}"
echo "build_windows.sh: 可复现性（4d-2）：删掉构件、隔 2 秒再链一次，逐字节比" >&2
# ⚠️ 见上面那段：**没有这个 sleep，这条判据就是个装饰**（实测过）。
sleep 2
(cd "${REPO}/windows" && "${CARGO}" build --release -p shell-win --target "${TARGET}") >/dev/null
if [ ! -f "${relink_exe}" ]; then
  rm -f "${relink_keep}"
  echo "build_windows.sh: 错误：删掉产物之后重跑 cargo，产物没有回来 —— 可复现性判据无法执行。" >&2
  echo "build_windows.sh: 文件：${relink_exe}" >&2
  echo "build_windows.sh: 本条判据的形式是'产出两份字节去比'；少一份，它就会退化成" >&2
  echo "build_windows.sh: '拿一个不存在的文件和自己比'（恒真）—— 那比没有判据更坏。" >&2
  echo "build_windows.sh: 补救：手工跑一次上面的 cargo 命令看它的原话。" >&2
  exit 1
fi
second_sha="$(sha256_of "${relink_exe}")"
second_size="$(wc -c < "${relink_exe}" | tr -d ' ')"
if ! cmp -s "${relink_keep}" "${relink_exe}"; then
  rm -f "${relink_keep}"
  echo "build_windows.sh: 错误：交付 exe **不可复现** —— 同一棵树连链两次，字节不同。" >&2
  echo "build_windows.sh: 第一次：${first_size} B，sha256 ${first_sha}" >&2
  echo "build_windows.sh: 第二次：${second_size} B，sha256 ${second_sha}" >&2
  # ⚠️ **先把根因指向最可能的那一个，但不要说死**：本代实测的根因就是 PE 头里的
  #    链接时刻；不过"体积相同、sha 不同"也可能是别的东西（编译器布局、随机种子）。
  echo "build_windows.sh: 最可能的根因（本代实测过的那一个）：PE 的 COFF 头里写进了**链接时刻**。" >&2
  echo "build_windows.sh: 看一眼：x86_64-w64-mingw32-objdump -p <exe> | grep 'Time/Date'" >&2
  echo "build_windows.sh:   · 两次不一样 ⇒ 就是它。那条开关在 shell-win/build.rs 的 pin_link_timestamp()" >&2
  echo "build_windows.sh:     （\"-Wl,--no-insert-timestamp\"）—— 它掉了？被别处覆盖了？还是换了链接器驱动？" >&2
  echo "build_windows.sh:   · 两次都是 1970-01-01 ⇒ 不是它，另找差异（\`cmp -l\` 看第一个不同的字节在哪个节）。" >&2
  echo "build_windows.sh: ⚠️ **不要**把本条判据删掉、或改成「只查构建脚本里那一行还在不在」：" >&2
  echo "build_windows.sh:    那是语法判据，挡不住'开关还在、但它不生效'——而这一条是" >&2
  echo "build_windows.sh:    '发出去的是哪一个'能不能回答的唯一依据（真机验收清单上要写 sha256）。" >&2
  exit 1
fi
rm -f "${relink_keep}"
echo "build_windows.sh: ✓ 可复现：连链两次逐字节相同（${first_size} B，sha256 ${first_sha}）" >&2
# 让后面那几步也看得到这个数（第 5b 步的"与上一份比"、第 6 步的收尾打印）。
# ⚠️ 用**同一次读数**，不要在后面再算一遍 —— 两次读数会造出"报出来的数与判据用的数不同"的缝。
EXE_SHA="${second_sha}"
EXE_SIZE="${second_size}"
export EXE_SHA EXE_SIZE

# ---- 打包（任务 22）--------------------------------------------------------
if [ "${check_only}" -eq 1 ]; then
  echo "build_windows.sh: --check-only：编内核 + 编壳 + 自验完成，跳过打包。" >&2
  echo "build_windows.sh: 产物（中间件，不是交付形态）：${EXE}" >&2
  exit 0
fi

# ---- 5) 打包成**单文件交付形态** + 自验交付形态（任务 22）-------------------
#
# ⚠️ **这一步不是"改个名"**：交付形态是"**一个** exe，内核在它里面"（W-4 / §7.2），
#    而那件事由**组装**保证：`shell-win/build.rs` 把内核 exe 拷进 `OUT_DIR` 并断言它的
#    PE 头/架构/子系统，`src/embed.rs` 用 `include_bytes!` 内嵌它，运行期释放到
#    `%LOCALAPPDATA%\BenagenDownloader\cache\`。本步只做**最后一跳**（搬到 dist）与
#    **对交付字节的自验**。
#
# 先在 dist 下写临时名、验收通过再换位（与 `macos/scripts/build_app_macos.sh` 同一纪律）：
# 中途失败不会在 dist/ 里留下一个"看着像交付形态、其实缺东西"的半成品。
mkdir -p "${DIST}"
staged="${DIST}/.${DIST_NAME}.staged"
# ⚠️ **trap 必须装在这里，不能等到 5a 那段**（复审 2026-09-19 次要 #2）。
#
#    旧位置在下面第一对 `file` 判据**之后**，而 `cp`（上面这条）与那两条判据都会
#    `exit 1` ⇒ 那几条路径**会把 `.staged` 留在 dist/ 里**，下一次跑就撞上第 5c 步那条
#    "dist/ 里只有一个文件"的判据。那条判据是**对的**，但它给出的根因会指向"目录脏"，
#    而真根因是"上一次构建中途失败了" —— 本项目对"根因不许说错"有纪律。
#
#    ⚠️ `${stripped}` 此刻**还没被赋值**（它在 5a 才 `mktemp`）⇒ 清理函数里必须用 `if` 判空：
#      · 不能写 `rm -f "${stripped}"`：`set -u` 下那会以 unbound variable 报错，
#        而清理函数**崩在 EXIT 上**会把真正的退出码盖掉（那正是本项目最怕的"错误理由"）；
#      · 也不能写 `[ -n … ] && rm …`：`set -e` 下最后一条测试为假会让函数返回 1，同样盖掉退出码。
stripped=""
cleanup_staged() {
  if [ -n "${stripped}" ]; then rm -f "${stripped}"; fi
  if [ -n "${staged}" ]; then rm -f "${staged}"; fi
}
trap cleanup_staged EXIT
rm -f "${staged}"
cp "${EXE}" "${staged}"

if [ ! -f "${staged}" ]; then
  echo "build_windows.sh: 错误：把构件拷到 ${staged} 失败（磁盘满？权限？）。" >&2
  echo "build_windows.sh: 补救：清一下 ${DIST} 所在磁盘，或查 ${DIST} 的写权限。" >&2
  exit 1
fi

# ---- 5a) 交付形态的字节自验（**这是客户真正拿到的那个文件**）----------------
#
# ⚠️ 下面这些判据**只对交付形态做**（不对中间件做）：中间件那三个检查（PE/子系统/DLL）
#    在第 4 步已经做过一遍 —— 那是"cargo 产出了对的构件"；这里要证的是
#    "**组装之后**那个文件仍然具备全部交付属性"。两者是不同的命题：
#    资源（manifest / 版本）与三份许可是**组装**带进来的，第 4 步看不见它们。
#
# ⚠️ **任务 15 起，PE / 子系统 / 依赖 DLL 这三条在**这里**也各跑一遍**（原话来自计划
#    步骤 2："在既有的子系统断言旁边加：导入表里不许有 webview2loader"）。理由不是
#    "多跑一遍更保险"：那一遍判的是**客户拿到的字节**，第 4 步判的是**构件**。
#    两者今天逐字节相同（5d 步那条 `cmp` 就是钉这件事的），但那是链路今天的性质，
#    不是判据。**判据只有一份实现**（`check_dependency_dlls`），所以两处的清单、
#    白名单与措辞不会分叉。
staged_line="$(file -b "${staged}")"
case "${staged_line}" in
  *PE32+*x86-64*) : ;;
  *)
    echo "build_windows.sh: 错误：交付形态不是 PE32+ x86-64。file 的原话：${staged_line}" >&2
    exit 1
    ;;
esac
case "${staged_line}" in
  *"(GUI)"*) : ;;
  *)
    echo "build_windows.sh: 错误：交付形态是 console 子系统（双击会带出黑窗）。file 原话：${staged_line}" >&2
    exit 1
    ;;
esac

# 5a-3) **交付字节的依赖判据**（任务 15 步骤 2 补的第二处调用点）。
#
# ⚠️ 第 4 步那次判的是 **cargo 的构件**；这一次判的是**客户真正拿到的那个文件**。
#    为什么要两处、而不是"反正它是 `cp` 过去的"—— 理由写在 `check_dependency_dlls`
#    上面那段（打包那一段将来多一步，两个命题就不再等价了）。
#    ⚠️ 位置就在**子系统断言旁边**（计划步骤 2 的原话），因为它们属于同一族判据：
#      "这个交付字节是不是我们要发的那一类可执行文件"。
check_dependency_dlls "${staged}" "交付形态"

# 去 NUL 之后落一份副本：UTF-16LE 的**版本资源**字符串去掉 0x00 之后就是可 grep 的 ASCII。
# ⚠️ `LC_ALL=C` **不是可选的**：macOS 自带的 `tr` 在 UTF-8 locale 下遇到非 UTF-8 字节会报
#    `Illegal byte sequence` 并**中途退出**（实测），于是下面所有判据都会以"查不到"的形式红
#    —— 而那句话会把根因指向"产物里没有 X"，完全指错方向。
stripped="$(mktemp)"
# ⚠️ 收拾 `${stripped}` 与 `${staged}` 的那条 trap **已经装在上面 5) 的入口处**
#    （那里才有覆盖到 `cp` 与这两条 `file` 判据的窗口，理由写在那里的注释里）。
#    ⚠️ 别再在这里装第二条：`trap` 是**覆盖**语义不是追加 —— 在这里重装会把上面那条
#    **提前**的 trap 顶掉，正好把这条修复的效果抹平（同 `test.sh` 里那条注释记的坑）。
LC_ALL=C tr -d '\000' < "${staged}" > "${stripped}"

# 判据的公共形状：**查不到就红**，并把"这一条在查什么、去哪儿看"说清楚。
#
# ⚠️ **空的 needle 一律当失败**：`grep -F ""` 匹配一切（恒真），而那正是本项目最恨的
#    "看起来在检查、其实什么都没检查"。针是自己（上面那段 iconv）拼出来的，
#    拼失败时它会变成空串 —— 这条守卫让那种情形响亮，而不是静默通过。
must_contain() {
  what="$1"; needle="$2"; why="$3"
  if [ -z "${needle}" ]; then
    echo "build_windows.sh: 错误：判据『${what}』的查找串是空的 —— 这条判据没有意义（grep 空串恒命中）。" >&2
    echo "build_windows.sh: 多半是上面拼 needle 的那一步失败了（例如缺 iconv）。" >&2
    exit 1
  fi
  if ! LC_ALL=C grep -a -c -F -- "${needle}" "${stripped}" >/dev/null; then
    echo "build_windows.sh: 错误：交付形态里查不到：${what}" >&2
    echo "build_windows.sh: 找的是这段字节：${needle}" >&2
    echo "build_windows.sh: 它为什么必须在里面：${why}" >&2
    echo "build_windows.sh: 补救：先确认这一段是不是这次改动动过（改了就同步改本判据）；" >&2
    echo "build_windows.sh:   若没动过，说明组装那一步（build.rs 的 .rc / 许可内嵌 / 内嵌内核）没生效。" >&2
    exit 1
  fi
}

# W-7：application manifest 的 `supportedOS` = Windows 10。
must_contain "application manifest 的 supportedOS 声明" "supportedOS" \
  "W-7 要求产物带 manifest 声明最低支持 Windows 10（没有它，部分 Windows 上会走兼容性垫片）"
must_contain "Windows 10 的 supportedOS GUID" "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" \
  "上面那条声明必须是 **Win10** 那一个 GUID（写成别的版本号会静默地不生效）"

# W-7：版本资源（右键 → 属性 → 详细信息）。
must_contain "版本资源里的 ProductName 键" "ProductName" \
  "W-7：右键 → 属性 → 详细信息 里要能看到版本号（没有版本资源时客户报障说不清装的是哪一版）"
must_contain "版本资源里的 FileVersion 键" "FileVersion" \
  "同上：版本号那一栏就是它（本脚本的第 5b 步还会核对**值**是 ${BENAGEN_VERSION}）"
# 产品名（中文）那一针：UTF-8 → UTF-16LE → 去掉 0x00 ⇒ 与去 NUL 之后的产物里那串字节同形。
# ⚠️ 缺 `iconv` 时**大声失败**（不许"那就不查这一条"）：这一条查的是"windres 的
#    `--codepage=65001` 有没有生效"，而它失效的表现是版本资源里一片问号 —— **静默**。
product_name_needle=""
if command -v iconv >/dev/null 2>&1; then
  product_name_needle="$(printf 'Benagen 数据下载工具' | iconv -f UTF-8 -t UTF-16LE | LC_ALL=C tr -d '\000')"
else
  echo "build_windows.sh: 错误：PATH 里没有 iconv，无法构造\"中文产品名在不在产物里\"这一条判据。" >&2
  echo "build_windows.sh: 跳过它等于放过一条**静默**的失败形态（windres 代码页不对 ⇒ 版本资源里全是问号）。" >&2
  echo "build_windows.sh: 补救（按平台挑一行）：" >&2
  echo "build_windows.sh:   macOS:         \`iconv\` 是系统自带的（GNU libiconv 的 BSD 版）。" >&2
  echo "build_windows.sh:   Debian/Ubuntu: \`iconv\` 在 libc-bin 里（一般已装；缺了就 \`apt-get install -y libc-bin\`）。" >&2
  echo "build_windows.sh:   Windows:       装 MSYS2，\`pacman -S libiconv\`（它带来 \`iconv\`）；" >&2
  echo "build_windows.sh:                  Git for Windows 那份不一定有 —— 先用 \`command -v iconv\` 看一眼。" >&2
  exit 1
fi
must_contain "版本资源里的产品名（中文，${#product_name_needle} 字节的 UTF-16LE）" "${product_name_needle}" \
  "产品名与窗口标题 / macOS 的 CFBundleDisplayName 逐字相同（规格 §9）。⚠️ 这一段是**非 ASCII**：\
   它能查到就同时证明 windres 的 --codepage=65001 生效了（否则会是一片问号，而那是静默的）"

# 版本号的值：四段式（build.rs 把 0.1.0 补成 0.1.0.0）。
version_quad() {
  IFS='.' read -r -a parts <<< "$1"
  printf '%s.%s.%s.%s' "${parts[0]:-0}" "${parts[1]:-0}" "${parts[2]:-0}" "${parts[3]:-0}"
}
quad="$(version_quad "${BENAGEN_VERSION}")"
must_contain "版本资源里的版本号值（${quad}）" "${quad}" \
  "版本资源里写的就是这个数（它由 VERSION 环境变量给到 build.rs，与第 5b 步核对的是同一个来源）"

# ---- 裁定 RR / W-5：**三份许可全文**（四道判据 × 三条，缺一不可）------------
#
# ⚠️⚠️ **这一段是审查（2026-09-19 重要 #1）重写过的**：原来只有"grep 一行标题"，
#    而它对"把全文换成一段 stub、只保留标题行"是**绿的** —— 判据看着在检查，
#    其实没有判别力（那份 stub 会一路绿到出海，而随附全文是**法律义务**）。
#
# ⚠️ **第三条（WebView2Loader.dll 的 3-Clause BSD）是任务 15 补的**：它**不是**新义务
#    —— 我们**早就**在随交付物再分发那份 DLL 了，缺的一直是它的全文。
#    来源与三样可审计的凭据（URL / 包版本 / 包 sha256）写在 `licenses.rs` 的文件头。
#
# 现在四道网（对**每一条**许可各跑一遍），各自堵一个形态：
#   ① **登记值 vs 磁盘原件**：`licenses.rs` 里登记的 sha256 必须与**磁盘上那份**逐字节相符
#      ⇒ 拦住"把文件改了/截断了/换成 stub"（连翻 1 个字节都拦得住）；
#   ② **长度下限（磁盘原件）**：拦住"整份换成一段 stub **短于下限**、并把登记值也同步改掉"
#      这种"一次能被审查的放宽"。⚠️ 下限定**值**只有一处（`licenses.rs` 的
#      `*_MIN_BYTES`，本脚本用 `lic_min()` 读它）；这里给的只是那个常量的**名字**；
#   ③ **产物里有登记值**：那些 sha256 是**渲染在界面上**的（设置 →「开源许可」面板里
#      「全文 sha256：…」那一行）⇒ 编进去就一定在 exe 里。查不到 = 常量没进产物；
#   ④ **产物里有全文的**首行与末行**（标记**从磁盘原件现取**）：stub 可以带标题行，
#      但带不上**末尾那一段** ⇒ 这一条堵死"标题在、全文不在"。
#
# ⚠️⚠️ **四道网合起来的效力边界（复审 2026-09-19 定向复审实测，别把话说满）**：
#    ② 只在 stub **短于下限**时成立。把磁盘原件换成一段**≥ 下限**的 stub
#    （GPLv2 的下限是 15000 B；复审用的那个反例是 15,930 B）**并把 `GPLV2_SHA256` 一起改掉**
#    ⇒ **①②③④ 一起绿、EXIT=0** —— 那是一次"能被审查的放宽"，但它**不是**被这四道网拦住的，
#    而是靠**人**在 diff 里看见"有个常量跟着换了"才拦得住。
#    （本任务当初自证判别力时用的那段 16 KB stub 正是这个反例 —— 上面那句"拦住整份换 stub"
#    是**说过头了**，这里改准：② 拦的是"**短** stub"，不是"stub"。）
#    ③ 的**唯一可达失败形态**是"两个常量整个从二进制里消失"，而那种情形已先被空登记值那条
#    守卫接住 ⇒ 四个真实变异里它一次都没响过。它**不是循环的**（值取自源码常量，不是从产物
#    反算的），但**判别力弱**这件事本身要记账，不要当成"有网就行"。
licenses_rs="${REPO}/windows/shell-win/src/licenses.rs"
lic_sha() {
  # 从登记处读一个 sha256 常量（**单一真相**在 licenses.rs，不在本脚本里抄一遍）。
  grep -m1 "^pub const $1: &str = \"" "${licenses_rs}" | sed -E 's/.*"([0-9a-f]{64})".*/\1/'
}
# ⚠️⚠️ **本函数必须带 `|| true` 调用**（复审 2026-09-19 次要 #1，实测过）。
#
#   本脚本是 `set -euo pipefail`（见文件头）。`lic_sha` 是一条**管道**：grep 找不到那个常量
#   时它退出 1 ⇒ `pipefail` 把整条管道判成失败 ⇒ **命令替换 `$(…)` 的赋值也失败** ⇒
#   `set -e` **立刻让脚本退出**，一行说明都不打。
#   后果：下面 `check_license` 里那段"读不到登记值…"的守卫（含它那句"等于没有判据"的
#   W-2 说明）**从来没被执行过** —— 它是**死代码**，而真正发生的是**一次没有任何说明的
#   静默红**。那与这段守卫自己的用意（"空白 needle 恒命中，等于没有判据"⇒要响亮）**正好相反**。
#   ⇒ 调用点写 `registered="$(lic_sha "${sha_const}" || true)"`：让 grep 的失败**留在函数里**，
#     由下面那条 `[ -z … ]` 的守卫去解释。**别把 `|| true` 挪进 `lic_sha` 内部** ——
#     那样函数永远返回 0，读不到值的失败就再也没人看得见了（这一条是**判据的载体**，
#     不是"让脚本别退出"的补丁）。
#   （自证：把 `licenses.rs` 里登记的常量改个名，脚本必须打印那段守卫的话并 `exit 1`，
#     而不是无声退出 —— 见第 4 轮合并报告。）
# 从登记处读一个**长度下限**常量（任务 15 收口；账本 §6.6 ⑥）。
#
# ⚠️ **在这一次收口之前，这三个数是"同一组数写在两处"**：`licenses.rs` 的
#    `const *_MIN_BYTES: usize` 与本脚本 `check_license` 的第四个实参**各写一遍**，
#    改一处不会有任何东西变红（对比：sha256 那一侧早就走 `lic_sha` 从 `licenses.rs` 读，
#    是单一来源）。⇒ 现在**真相只有一处**（`licenses.rs`），本函数是它的唯一读法。
#
# ⚠️ 与 `lic_sha` **同一形状、同一纪律**：`grep` 找不到就退出 1，所以调用点**必须**
#    写 `|| true`（理由逐字写在 `lic_sha` 上面那一段），由 `check_license` 里那条
#    `[ -z … ]` 的守卫去**解释**这件事。**别把 `|| true` 挪进来**。
#
# ⚠️⚠️ **这段的写法是踩过两次之后才定下来的，别"简化"它**（任务 15 实测）：
#
#   第一版是 `grep -oE '[0-9][0-9_]*'`（想在整行里抓那个数）。它**错得很安静**：
#   那一行是 `const GPLV2_MIN_BYTES: usize = 15_000;`，而 `-o` 把**常量名里的 `2`**
#   也当成一个匹配 ⇒ 读出来是 `"2\n15000"` ⇒ 下游那句
#   `[ "${actual_bytes}" -lt "${min_bytes}" ]` 打出
#   `integer expression expected` 并**返回 2** ⇒ `if` 判为假 ⇒
#   **那条法律义务的判据一声不响地"通过"了**（正是本仓库最恨的 W-2 形态）。
#   ⇒ 现在**先切到 `=` 右边、再只留数字**（`tr -cd '0-9'` 顺手把 `_` 与结尾的 `;` 去掉），
#     并且下面加了一条**形状守卫**（不是数字就 `exit 1`）—— 光靠"非空"守卫接不住这一例。
lic_min() {
  grep -m1 "^const $1: usize = " "${licenses_rs}" \
    | sed -E 's/^[^=]*= *//' | tr -cd '0-9'
}
lic_file_sha() { sha256_of "$1"; }
# 末行/首行（去掉行首行尾空白，好让 grep 用的串是文件里的**连续字节**）。
first_line() { sed -n '1p' "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }
last_line() { grep -v '^[[:space:]]*$' "$1" | tail -1 | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }

check_license() {
  label="$1"; file="$2"; sha_const="$3"; min_bytes="$4"
  # ⚠️ `|| true` **是承重的**，见 `lic_sha` 上面那段：少了它，读不到常量时脚本会在这里
  #    **静默退出**，而下面这条守卫是死代码。
  registered="$(lic_sha "${sha_const}" || true)"
  if [ -z "${registered}" ]; then
    echo "build_windows.sh: 错误：从 ${licenses_rs} 里读不到登记值 ${sha_const}。" >&2
    echo "build_windows.sh: 那道'编进去的是不是全文'的判据就没了载体（空白 needle 恒命中，等于没有判据）。" >&2
    echo "build_windows.sh: 补救：确认 licenses.rs 里还有 \`pub const ${sha_const}: &str = \"<64 位小写十六进制>\";\`。" >&2
    exit 1
  fi
  if [ ! -f "${file}" ]; then
    echo "build_windows.sh: 错误：找不到 ${label} 的全文原件：${file}" >&2
    exit 1
  fi

  # ① 登记值 == 磁盘原件的 sha256（连 1 个字节都拦得住）
  actual="$(lic_file_sha "${file}")"
  if [ "${actual}" != "${registered}" ]; then
    echo "build_windows.sh: 错误：${label} 的全文与**登记值**不符。" >&2
    echo "build_windows.sh:   文件：${file}" >&2
    echo "build_windows.sh:   登记：${registered}（licenses.rs 的 ${sha_const}）" >&2
    echo "build_windows.sh:   实算：${actual}" >&2
    echo "build_windows.sh: ⇒ 只有两种可能：① 全文被改过/被截断过（那是随附义务出了问题，把它换回来）；" >&2
    echo "build_windows.sh:   ② 这是一次**有意**换全文 —— 那就把 ${sha_const} 与长度下限一起更新，让这次放宽能被审查。" >&2
    exit 1
  fi

  # ② 长度下限（拦住"换了**短于下限**的 stub 又把登记值同步改掉"）。
  # ⚠️ 对"**≥ 下限**的 stub + 同步改登记值"这条**不管用** —— 效力边界写在上面那段注释里。
  # ⚠️ 下限**不是**调用方给的（那是"同一组数写两处"，账本 §6.6 ⑥）：$4 只是那条常量的**名字**，
  #    值现从 `licenses.rs` 读。读不到 ⇒ **大声失败**（与登记值那一条同一条纪律：
  #    没有载体 = 没有判据 = 不许放过）。
  min_name="$4"
  min_bytes="$(lic_min "${min_name}" || true)"
  # ⚠️⚠️ **形状守卫，不是"非空守卫"** —— 这一条的写法是**实测**出来的：
  #    第一版只判了"空不空"，而 `lic_min` 当时读出来的是 `"2\n15000"`（常量名里那个 `2`
  #    被 `grep -o` 当成了一个匹配）⇒ 非空 ⇒ 守卫放行 ⇒ 下游 `[ … -lt … ]` 打一句
  #    `integer expression expected` 并返回 2 ⇒ `if` 判为假 ⇒ **判据安静地"通过"了**。
  #    ⇒ 这里必须判**形状**（全是十进制数字），而不是"有没有值"。
  #    理由与本文件别处的"守卫的守卫"同一条：**匹配/解析逻辑自己坏掉时，
  #    报出来的根因会指错方向（这里更糟 —— 它什么都不报）。**
  case "${min_bytes}" in
    ''|*[!0-9]*)
      echo "build_windows.sh: 错误：从 ${licenses_rs} 里读不到长度下限 ${min_name}（读出来是「${min_bytes}」）。" >&2
      echo "build_windows.sh: 这一条判据问的是'这份全文是不是短得不像全文'，而下限就是它的阈值 ——" >&2
      echo "build_windows.sh: 读不到（或读出来不是纯数字）就等于**没有阈值**，那比没有这条判据更坏：" >&2
      echo "build_windows.sh: 下游那句 \`[ … -lt … ]\` 会打一句 integer expression expected 然后**判为假** ——" >&2
      echo "build_windows.sh: 也就是**这条法律义务的判据一声不响地通过**。所以这里必须当场拦住。" >&2
      echo "build_windows.sh: 补救：确认 licenses.rs 里还有 \`const ${min_name}: usize = <十进制数字>;\`" >&2
      echo "build_windows.sh:   （下划线分隔符可有可无；本脚本只取 \`=\` 右边的数字）。" >&2
      exit 1
      ;;
  esac

  actual_bytes="$(wc -c < "${file}" | tr -d ' ')"
  if [ "${actual_bytes}" -lt "${min_bytes}" ]; then
    echo "build_windows.sh: 错误：${label} 的全文只有 ${actual_bytes} 字节（下限 ${min_bytes}）—— 那不是全文。" >&2
    echo "build_windows.sh: 补救：${file} 应当是官方许可的**逐字**全文，不要「整理」、不要截断。" >&2
    exit 1
  fi

  # ③ 产物里有登记值（⇒ 常量真的进了 exe）
  must_contain "${label} 的 sha256 登记值（${registered}）" "${registered}" \
    "W-5 / 裁定 RR：这份 sha256 与全文一起被渲染在「开源许可」那一屏，所以它必须在 exe 里 —— \
     查不到说明那段常量根本没编进去"

  # ④ 产物里有全文的**首行**与**末行**（标记从磁盘原件现取）
  head_marker="$(first_line "${file}")"
  tail_marker="$(last_line "${file}")"
  must_contain "${label} 全文的首行" "${head_marker}" \
    "全文的第一行 —— 与末行一起，证明产物里的是**从头到尾**那一份，而不是一段 stub"
  must_contain "${label} 全文的末行" "${tail_marker}" \
    "全文的最后一行 —— stub 带得上标题，但带不上结尾（这一条堵的就是那个形态）"

  echo "build_windows.sh: ✓ ${label}：登记值相符、${actual_bytes} B（下限 ${min_bytes}）、产物里有首末行" >&2
}

check_license "GPLv2（aria2 的随附义务）" \
  "${REPO}/core/assets/COPYING-GPLv2.txt" "GPLV2_SHA256" "GPLV2_MIN_BYTES"
check_license "OFL-1.1（字体的随附义务）" \
  "${REPO}/windows/assets/OFL-1.1.txt" "OFL_1_1_SHA256" "OFL_1_1_MIN_BYTES"
# 第三条（**任务 15 补的**）：随交付物再分发的 `WebView2Loader.dll` 是**微软的代码**，
# 而 BSD-3 的**第二条**要求"二进制再分发必须复现版权声明、条件与免责声明" ——
# 在此之前我们**已经在发它了**，而这一段里只有两条（本仓库自己的 W-5 纪律不允许的形状）。
# ⚠️ 全文来自**官方渠道**（api.nuget.org 的 `microsoft.web.webview2` 包，
#    版本与凭据写在 `licenses.rs` 的文件头）；**别**把它换成"另一份看起来一样的 BSD-3"
#    —— 版权行与免责声明的措辞是那份文件的一部分。
check_license "3-Clause BSD（WebView2Loader.dll 的再分发义务）" \
  "${REPO}/windows/assets/WebView2-SDK-LICENSE.txt" "WEBVIEW2_SDK_SHA256" "WEBVIEW2_SDK_MIN_BYTES"

# ⚠️ **内嵌的内核确实是"刚编出来的这一份"** —— 本机（macOS）能拿到的、关于
#    "构建顺序没接错"的**唯一**硬证据：壳把内核的 sha256 当成**编译期常量**编了进去
#    （`build.rs` → `embed.rs::CORE_SHA256`），这里拿它跟磁盘上那份内核 exe 的 sha256 比。
#    嵌了旧内核 / 嵌错了文件 ⇒ 这两个值对不上 ⇒ 当场红。
#    （⚠️ **不要**改成"再编一遍内核、比哈希"：sha256 **不可复现**、大小也不是指纹 ——
#     PE 头里有链接时刻、尾部有符号表。那会**永远红**，理由写在 `shell-win/build.rs` 头部。）
core_sha_now="$(sha256_of "${CORE_EXE}")"
if [ "${core_sha_now}" != "${core_sha}" ]; then
  echo "build_windows.sh: 错误：内核 exe 在编壳的过程中变了（第 2 步算的是 ${core_sha}，现在是 ${core_sha_now}）。" >&2
  echo "build_windows.sh: 这**不是**产物的问题：说明有东西在我们编壳的同时动了内核产物（别的构建？）。" >&2
  echo "build_windows.sh: 补救：重跑本脚本；若还红，看看是不是有并行的构建在同一个 target 目录里跑。" >&2
  exit 1
fi
must_contain "内嵌内核的 sha256（${core_sha_now}）" "${core_sha_now}" \
  "它是**构建顺序没接错**的证据：壳内嵌的那份内核，sha256 必须等于第 2 步编出来的那一份"

# ---- 5a-4) **前端资源（含字体子集）确实被内嵌进产物** ------------------------
#
# 🔴 **这一条在此之前不存在，而 `windows/README.md` 已经在向人担保它了**（R-2，2026-09-20
#    的整分支审查实测）：README 逐字写着"这些判据在交付形态那一个文件上各跑一遍：…
#    **三份**许可全文**与字体子集**都在里面"，而这一段里当时只有许可全文那条判据
#    —— **字体子集与整个前端资源一条断言都没有**。规格 §9.1 把"前端资源是否被正确内嵌"
#    列在"能在这台 macOS 上判"那一栏，也一直没有落点。
#
# 为什么它值得一条真判据（而不是一句文档）：`frontendDist` 指错、`web/fonts/` 被某个
#    ignore 规则漏掉、或将来打包那一步多一次"只挑几个文件"的拷贝 ⇒ **构建全绿、判据全绿、
#    exe 照常产出**，而客户机器上是一屏**豆腐块 / 白屏**。"看起来在守、其实没守"正是
#    本代反复记账的那一类，所以这条要么真加上、要么把 README 改成如实的。
#
# 判据怎么成立（2026-09-20 实测）：Tauri 把 `frontendDist` 下每个文件的**键名**（相对路径）
#    以**明文**写进二进制（它只对**字节**做 brotli，键不压缩）⇒ 拿上面那份 `${stripped}`
#    直接 `grep -F` 路径即可，与许可判据**共用同一份实现**（`must_contain`）。
#    实测（那份已交付的 exe）：`js/shell.js` / `css/tokens.css` / `fonts/ui-subset.otf`
#    各命中 1 次。
#
# ⚠️ **逐文件判，不是"抽查三个"**：抽查漏掉的正是"新增了一个文件而它没进 exe"这个形态
#    —— 而那正是这条判据要挡的事。清单直接取自 `windows/web/`（`frontendDist` 指的就是它），
#    所以"有哪些文件"没有第二个真相源。
# ⚠️ **残留风险（如实记账）**：它判的是**键名在不在**，判不出"同名文件的内容被换掉了"
#    —— 那一层由入库副本的 sha256 判据守着（`check_frontend_copy.sh` 的 `TWIN_FILES`
#    与 `make_font_subset.sh --check` 的交付副本比对）。两条判据各守一层，别把这条读成后者。
web_root="${REPO}/windows/web"
if [ ! -d "${web_root}" ]; then
  echo "build_windows.sh: 错误：找不到 ${web_root}（前端资源目录不在场）。" >&2
  echo "build_windows.sh: 本判据要证明「这些资源被编进了 exe」；目录都不在，那不是\"通过\"。" >&2
  echo "build_windows.sh: 补救：核对 shell-win/tauri.conf.json 的 build.frontendDist（现为 ../web）。" >&2
  exit 1
fi
# ⚠️ `find` 失败**必须红**：一个"因为列不出文件所以通过"的判据，与没有判据无法区分
#    （同 `test.sh` 里那条"空跑绿灯判为失败"的纪律）。
web_files="$(cd "${web_root}" && find . -type f | LC_ALL=C sed 's|^\./||' | LC_ALL=C sort)" || {
  echo "build_windows.sh: 错误：列举 ${web_root} 下的文件失败（find 非零退出）。" >&2
  echo "build_windows.sh: 这不是\"通过\"——连有哪些资源都没数出来，就谈不上内嵌。" >&2
  exit 1
}
if [ -z "${web_files}" ]; then
  echo "build_windows.sh: 错误：${web_root} 下一个文件都没有 —— 判据无对象可判。" >&2
  echo "build_windows.sh: 前端资源整棵不见了（那不是\"通过\"）：客户会拿到一个白屏的 exe。" >&2
  exit 1
fi
web_count=0
while IFS= read -r rel; do
  [ -n "${rel}" ] || continue
  web_count=$((web_count + 1))
  must_contain "前端资源 ${rel}" "${rel}" \
    "前端资源是**编进 exe** 的（tauri.conf.json 的 build.frontendDist = ../web）：\
少了它界面上就是白屏 / 豆腐块，而编译、测试、构建**全都过得去**"
done <<< "${web_files}"
echo "build_windows.sh: ✓ 前端资源（含字体子集）全在产物里（${web_count} 个文件，逐键判定）" >&2

# ---- 5b) 换位：临时名 → 交付名 --------------------------------------------
if [ -f "${DIST_EXE}" ] && cmp -s "${staged}" "${DIST_EXE}"; then
  # 逐字节相同：不换位（改 mtime 会让"这份包是什么时候建的"变成一个假的读数）
  rm -f "${staged}"
  echo "build_windows.sh: 交付形态与上一份逐字节相同，未改动 ${DIST_EXE}" >&2
else
  mv -f "${staged}" "${DIST_EXE}"
fi

# ---- 5c) **W-4 的判据：dist/ 里只有一个文件** -------------------------------
#
# ⚠️ W-4 的原话是"客户只拿**一个** exe"。在此之前这条**没有任何判据**：
#    `dist/` 里多出任何东西（半成品、旧的临时名、调试用的 sidecar）都没人会红。
# ⚠️ 它同时兜住一类具体的坏结局：构建中途失败时，`dist/.<名字>.staged` 会留在原地
#    （上面的 cp 之后、mv 之前的那一段窗口）—— 那条由下面的 trap 收拾，
#    而这一条 grep 是"万一 trap 也没兜住"的第二道网（**空目录也会被它逮住**：
#    产物没做出来时 `ls -A` 是 0 行）。
dist_entries="$(ls -A "${DIST}" | wc -l | tr -d ' ')"
if [ "${dist_entries}" -ne 1 ]; then
  echo "build_windows.sh: 错误：${DIST} 里有 ${dist_entries} 个东西，而 W-4 要求交付形态是**一个** exe。" >&2
  ls -A "${DIST}" | sed 's/^/build_windows.sh:   - /' >&2
  echo "build_windows.sh: 补救：清理 ${DIST}（只留 ${DIST_NAME}），或看看是不是有构建的残留物。" >&2
  exit 1
fi
if [ ! -f "${DIST_EXE}" ]; then
  echo "build_windows.sh: 错误：${DIST} 里唯一的那个东西不是 ${DIST_NAME}。" >&2
  exit 1
fi
echo "build_windows.sh: ✓ W-4：dist/ 里只有一个文件（${DIST_NAME}）" >&2

# ---- 5d) **客户拿到的字节 == 前面验过的那一份字节**（任务 15 补）------------
#
# ⚠️ **这条不是"多此一举"**：上面 5a 的每一条判据（含任务 15 新加的依赖判据与两条许可
#    判据）判的都是 `${staged}`；而 `dist/` 里那个交付名是**另一条路径**（5b 的 `mv` /
#    "逐字节相同就不换位"）。两者今天必然一致 —— 但"必然一致"是**这一条链路今天的性质**，
#    不是判据。将来打包多一步（签名、压缩、改壳），"验过的那个文件"与"发出去的那个文件"
#    就会**静默**变成两个东西，而那时上面每一条判据都还在报绿。
#    ⇒ 一条 `cmp` 就能把这句话钉死，成本是读一遍字节。
if ! cmp -s "${EXE}" "${DIST_EXE}"; then
  echo "build_windows.sh: 错误：dist/ 里的交付形态与前一步自验过的构件**不是同一份字节**。" >&2
  echo "build_windows.sh: 自验过的：${EXE}（sha256 ${EXE_SHA}）" >&2
  echo "build_windows.sh: 要发的：  ${DIST_EXE}（sha256 $(sha256_of "${DIST_EXE}")）" >&2
  echo "build_windows.sh: ⚠ 这意味着上面 5a 那些判据**没有一条**作用在客户拿到的文件上。" >&2
  echo "build_windows.sh: 补救：查第 5b 步的换位逻辑（是不是有人往中间加了一步转换？）。" >&2
  exit 1
fi
echo "build_windows.sh: ✓ 交付形态 == 自验过的那一份（sha256 ${EXE_SHA}）" >&2

# ---- 6) 收尾：把每一层的体积打出来（W-4：客户要下的字节数是一等公民）------
# ⚠️ sha256 用**第 4d 步判据里的那一次读数**（`${EXE_SHA}`），不是在这里再算一遍：
#    两次读数会造出"报出来的数与判据用的数可能不同"的缝 —— 而这一段正是给人抄进
#    真机验收清单的那一份（清单上的 sha256 必须就是被断言过的那一个）。
dist_size="$(wc -c < "${DIST_EXE}" | tr -d ' ')"
core_kb=$(( (core_size + 1023) / 1024 ))
dist_kb=$(( (dist_size + 1023) / 1024 ))
echo "" >&2
echo "build_windows.sh: ===== 交付形态 =====" >&2
echo "build_windows.sh: 路径：${DIST_EXE}" >&2
echo "build_windows.sh: 体积：${dist_size} B（${dist_kb} KB）" >&2
echo "build_windows.sh: 内嵌内核：${core_size} B（${core_kb} KB）⇒ 占交付形态的 $(( core_size * 100 / dist_size ))%" >&2
echo "build_windows.sh: sha256：${EXE_SHA}" >&2
echo "build_windows.sh: 版本资源：${BENAGEN_VERSION}（quad ${quad}）" >&2
echo "build_windows.sh: 可复现：连链两次逐字节相同（判据在第 4d 步）——" >&2
echo "build_windows.sh:   ⇒ 上面这个 sha256 就是**同树重建两次都会得到**的那一个，" >&2
echo "build_windows.sh:     可以抄进真机验收清单当'发出去的是哪一个'的凭据。" >&2
# ⚠️ 这一段**不点名宿主 OS**（2026-09-19 改）：原文写的是"本机（macOS）验到的到此为止"，
#    而本仓库已经搬到过 Windows 上跑 —— 那句话在 Windows 机器上会**直接是假的**，
#    连带"'双击起不起'只有真 Windows 能判"也假（那台机器就是 Windows，跑一下就知道）。
#    判据一个字没动：这只是收尾那三行打印。
echo "build_windows.sh: ⚠️ 本脚本验到的到此为止：上面每一条都是**字节层**的读数。" >&2
echo "build_windows.sh:    '双击起不起、有没有控制台窗口、内核释放成不成功'**不是本脚本的判据**：" >&2
echo "build_windows.sh:    那几件事只有真的把 exe 在 Windows 上跑起来才判得了（规格 §11.1）——" >&2
echo "build_windows.sh:    本脚本不声称它验过那些。" >&2
exit 0
