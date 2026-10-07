#!/usr/bin/env bash
#
# core/scripts/build_core_windows.sh —— 把 Windows 内核**交叉编译出来并自验产物**（W-1）。
#
# 为什么必须有这个脚本，而不是手敲 cargo 命令
#   1) **交叉编译的配方有坑，而且踩过**：本机有两个 cargo —— `/opt/homebrew/bin/cargo`
#      只有 aarch64 的 std，拿它编 Windows 会报 `E0463`，而那句错误的措辞**不指向真正的
#      根因**（看起来像"目标没装"，实际是"用错了 cargo"）。必须用 rustup 管的那份，并把
#      RUSTUP_HOME / CARGO_HOME / PATH 摆对（探路实测：
#      `docs/superpowers/2026-09-18-windows-spike.md` §2）。写死在脚本里，比让每个人
#      各自记得强。
#
#      ⚠️ **上面这一段是"在 macOS 开发机上"的实测**（2026-09-19 起补记）：在 Windows 上
#         **只有一个 cargo**（rustup 装的那份），所以"两个 cargo 抢 PATH"那段症状**不会
#        出现**。**但"用绝对路径调 rustup 的 cargo"这条纪律照旧** —— 它防的是"PATH 上恰好
#         有另一份 cargo"，而不是"这台机器上一定有两份"。⚠️ 在 Windows 上这条纪律**更要紧**：
#         `${CARGO_HOME:-$HOME/.cargo}` 在 **MSYS2 自带的 shell** 里会落到 `/home/<你>/.cargo`
#         —— 见下面失败分支里那段说明。
#   2) **W-1：每一条构建路径都必须自己验产物。**"cargo 退出 0" ≠ "产物是给 Windows 的
#      x86-64 控制台程序"——参数传对与产出一个错的东西完全可以同时成立（macOS 侧栽过
#      同形的坑：`arch -x86_64 clang` 仍然编出 arm64）。所以下面只看**字节**。
#   3) **W-2：工具链缺什么就大声说什么，并给出可执行的补救。**
#
# ⚠️ 本脚本**只管"编内核"**：不打壳、不打包。壳侧的交叉编译与打包在
#    `windows/scripts/build_windows.sh`（那是另一条构建路径，它自己也要验一遍产物）。
#    ⚠️ **构建顺序是硬约束**：壳把内核 exe 内嵌进自己的资源里（`shell-win/build.rs`），
#       所以**内核必须先编出来**，否则会内嵌**上一次**的旧内核，而 cargo 完全不知情。
#
# ⚠️ 本机能给的证据**只到产物层**：这是交叉编译，本机（macOS）跑不了这个 exe，
#    也**没有**任何东西在这里声称"Windows 上跑过了"。见本文件末尾那段说明。
#
# 用法：
#   bash core/scripts/build_core_windows.sh      # 交叉编译 + 自验本阶段的完整产物
#   bash core/scripts/build_core_windows.sh -h   # 只看这段说明
#
# 退出码：0 = 编好且自验通过；1 = 产物/工具链不合格（原因与补救已打印到 stderr）；
#         2 = 用法错误；**101 = cargo/rustc 自己失败**。
#
# ⚠️ 那个 101 **不是笔误、也不是本脚本定义的码**：cargo 编译失败时的退出码就是 101，
#    而本脚本按 `set -e` **原样把它传出去**（不转译、不吞掉）。所以调用方**不能**把非零
#    一律当成"产物不合格"去重试——101 意味着"代码/依赖没编过"，重试一百次也一样。
#    同理，cargo 的其它码（如 104）也会原样透出。判据是"本脚本自己报的错一定配了补救文案"：
#    输出来自 cargo 的，就是 cargo 的问题。
#
# ⚠️ **所有输出都走 stderr**（借 `windows/scripts/build_windows.sh` 的同一条纪律）：
#    这样调用者可以放心把 stdout 当成"给机器的通道"，不必担心构建噪音混进去。
set -euo pipefail

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  # ⚠️ **不要用硬编码行号**（`preflight_x86_64_toolchain.sh` 被这个咬过：文件头一长就
  #    静默截断，连退出码约定都看不到了）。判据跟着文件头走：从第 2 行打印到第一行以
  #    `set ` 开头的正文为止（不含它）。
  awk 'NR == 1 { next } /^set / { exit } { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}" >&2
  exit 0
fi
if [ "$#" -gt 0 ]; then
  # 与 `test.sh` / `build_windows.sh` 同一条纪律：未知参数**不透传**。
  # 透传会让脚本以一个"看起来像失败"的退出码收场，而根因只是打错了字——
  # 那样的红与没有验收等价。
  echo "build_core_windows.sh: 不认识的参数：$1（本脚本不接受参数，-h 看用法）" >&2
  exit 2
fi

# ⚠️ 仓库根**从脚本自身位置反推**，不用 `$PWD`：本仓库在这个坑上栽过两次。
#    脚本在 `core/scripts/` 下 ⇒ `../..` 是仓库根。`${BASH_SOURCE[0]}` 而不是 `$0`：
#    相对路径的 `$0` 在下面的 `cd` 之后会解析到错的地方。
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE="${REPO}/core"

TARGET="x86_64-pc-windows-gnu"
# `[[bin]] name`（`core/Cargo.toml`）——cargo 的产物名就是它 + `.exe`。
BIN_NAME="benagen-core"
# 显式钉住 target 目录（而不是吃默认值）：产物路径要**只由脚本位置决定**——
# 既不看调用者的 cwd，也不受别处设的 CARGO_TARGET_DIR 影响。自验那一步的路径
# 因此是可推导的，不是猜的。默认值本来就是 `${CORE}/target`，热缓存照旧。
TARGET_DIR="${CORE}/target"
BIN="${TARGET_DIR}/${TARGET}/release/${BIN_NAME}.exe"

# ---- 1) 工具链 preflight（W-2）---------------------------------------------
#
# ⚠️ CARGO_HOME 只解析一次，**检查与补救共用同一个值**。`preflight_x86_64_toolchain.sh`
#    在这里踩过：检查用 `${CARGO_HOME:-$HOME/.cargo}`、补救却写死 `$HOME/.cargo/bin`，
#    于是在显式设了 CARGO_HOME 的环境（CI 常见）里，照抄那句 `export PATH=…` 不生效，
#    "可执行的补救"等于没给。
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
CARGO_BIN_DIR="${CARGO_HOME_DIR}/bin"
CARGO="${CARGO_BIN_DIR}/cargo"
RUSTUP="${CARGO_BIN_DIR}/rustup"

# 用**绝对路径**调 rustup 的 cargo，绝不用 PATH 上解析到的那份（理由见文件头第 1 条）。
if [ ! -x "${CARGO}" ]; then
  echo "build_core_windows.sh: 错误：找不到 rustup 的 cargo：${CARGO}" >&2
  echo "build_core_windows.sh: 本脚本故意用绝对路径调它，不用 PATH 上的 cargo ——" >&2
  echo "build_core_windows.sh:   在 macOS 上，PATH 上那份很可能是 /opt/homebrew/bin/cargo，它只有" >&2
  echo "build_core_windows.sh:   aarch64 的 std，编 ${TARGET} 会报 E0463，而那句错误的" >&2
  echo "build_core_windows.sh:   根因看起来完全不是'用错了 cargo'。" >&2
  echo "build_core_windows.sh: ⚠️ 在 Windows 上，这个路径查不到**先看你在哪个 shell 里**：" >&2
  echo "build_core_windows.sh:   · **MSYS2 自带的 shell**：它的 \$HOME 是 /home/<你>（MSYS2 自己的家目录），" >&2
  echo "build_core_windows.sh:     而 rustup 装的东西在 C:\\Users\\<你>\\.cargo ⇒ 上面那个路径会解析成" >&2
  echo "build_core_windows.sh:     /home/<你>/.cargo/bin/cargo，**那里没有东西**。" >&2
  echo "build_core_windows.sh:   · **Git Bash**：它的 \$HOME 就是 %USERPROFILE%（= C:\\Users\\<你>）⇒ 这个" >&2
  echo "build_core_windows.sh:     路径**是对的**；还找不到就只剩'rustup 没装'或'装到了别处'。" >&2
  echo "build_core_windows.sh: 补救（按平台挑一行）：" >&2
  echo "build_core_windows.sh:   macOS:         装 rustup（https://rustup.rs），或用 CARGO_HOME=<你的 rustup" >&2
  echo "build_core_windows.sh:                  目录> 重跑本脚本。" >&2
  echo "build_core_windows.sh:   Debian/Ubuntu: 装 rustup（https://rustup.rs）；发行版自带的 cargo 不经 rustup，" >&2
  echo "build_core_windows.sh:                  本脚本要的是 rustup 管的那份。" >&2
  echo "build_core_windows.sh:   Windows:       装 rustup（https://rustup.rs），然后**在 Git Bash 里**跑本脚本" >&2
  echo "build_core_windows.sh:                  （MSYS2 自带的 shell 会让 \$HOME 指错地方，见上）；" >&2
  echo "build_core_windows.sh:                  或用 CARGO_HOME=C:\\Users\\<你>\\.cargo 显式指过去。" >&2
  exit 1
fi
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME_DIR}"
# PATH 里也把 rustup 的 bin 排到最前：cargo 拉起的子进程（build script 等）可能再找 `cargo`，
# 让它们也拿到同一份。（`${CARGO_HOME_DIR}` 此刻已确定存在，前面的检查刚证过。）
export PATH="${CARGO_BIN_DIR}:${PATH}"

if ! "${CARGO}" --version >/dev/null 2>&1; then
  echo "build_core_windows.sh: 错误：${CARGO} --version 跑不起来（rustup shim 坏了？）。" >&2
  echo "build_core_windows.sh: 补救：手工跑一次 \`${CARGO} --version\` 看它的原话；必要时重装 rustup。" >&2
  exit 1
fi

if [ ! -x "${RUSTUP}" ]; then
  echo "build_core_windows.sh: 错误：找不到 rustup：${RUSTUP}" >&2
  echo "build_core_windows.sh: 没有它就没法确认 ${TARGET} 的 std 装没装，而" >&2
  echo "build_core_windows.sh:   '没装 std' 的表现是编到一半的 E0463（根因说错的那一条）。" >&2
  echo "build_core_windows.sh: 补救：装 rustup（https://rustup.rs）。" >&2
  exit 1
fi

# 目标 std 必须真的装了 —— 否则下面那次 cargo build 会以 `E0463` 失败。
#
# ⚠️ **rustup 自己失败与"没装 std"是两件事，必须分开报**：原写法是
#    `rustup target list --installed 2>/dev/null | grep -qx …`，它把 rustup 的**错误文本
#    连同退出码一起吞掉**（`2>/dev/null` + `grep` 只看有没有那一行），于是 rustup 自身坏掉时
#    报的是"没装 std（已装：<空>）"+ 一句 `rustup target add …` —— 而那条补救**打不中**，
#    因为真正的根因在 rustup 本身（PATH 装了个壳？toolchain 目录被裁剪？）。W-2 要的不是
#    "响"，是**指向真根因**。所以下面先看 rustup 自己的退出码，再看它的输出里有没有那一行。
rustup_out=""
if ! rustup_out="$("${RUSTUP}" target list --installed 2>&1)"; then
  echo "build_core_windows.sh: 错误：\`${RUSTUP} target list --installed\` 自己失败了（不是'没装 std'）。" >&2
  echo "build_core_windows.sh: rustup 的原话：${rustup_out}" >&2
  echo "build_core_windows.sh: 补救：手工跑一次 \`${RUSTUP} target list --installed\` 看它的原话；" >&2
  echo "build_core_windows.sh:   常见是 rustup shim 与工具链目录对不上（被裁剪/被换过），重装 rustup 可解。" >&2
  exit 1
fi
if ! grep -qxF "${TARGET}" <<<"${rustup_out}"; then
  echo "build_core_windows.sh: 错误：rustup 没装 ${TARGET} 的 std（已装：$(printf '%s' "${rustup_out}" | tr '\n' ' ')）" >&2
  echo "build_core_windows.sh: 补救：\`${RUSTUP} target add ${TARGET}\`" >&2
  exit 1
fi

# mingw-w64：链接器（cargo 用得到）与自验工具（`objdump`）。
# `file` 通常是系统自带的，但**仍然显式检查**——它缺席时自验那几步会以一句
# "command not found" 收场，而那看起来像脚本坏了，不像工具缺失。
missing_tools=""
for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-objdump file; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    missing_tools="${missing_tools} ${tool}"
  fi
done
if [ -n "${missing_tools}" ]; then
  echo "build_core_windows.sh: 错误：缺工具：${missing_tools}" >&2
  echo "build_core_windows.sh: 交叉编译到 ${TARGET} 要 mingw-w64 的链接器，自验要 objdump 与 file。" >&2
  echo "build_core_windows.sh: 补救（按平台挑一行）：" >&2
  echo "build_core_windows.sh:   macOS:         \`brew install mingw-w64\`（file 是系统自带的；" >&2
  echo "build_core_windows.sh:                  file 缺失说明这台机器不寻常，先查 PATH）。" >&2
  echo "build_core_windows.sh:   Debian/Ubuntu: \`apt-get install -y mingw-w64 file\`。" >&2
  echo "build_core_windows.sh:   Windows:       装 MSYS2（https://www.msys2.org），然后" >&2
  echo "build_core_windows.sh:                  \`pacman -S mingw-w64-x86_64-gcc\`（带来 x86_64-w64-mingw32-gcc /" >&2
  echo "build_core_windows.sh:                  -objdump / -windres 三个）与 \`pacman -S file\`；" >&2
  echo "build_core_windows.sh:                  再把 C:\\msys64\\mingw64\\bin 加进 PATH。" >&2
  echo "build_core_windows.sh:                  ⚠️ **本脚本要在 Git Bash 里跑**，不是 MSYS2 自带的 shell。" >&2
  echo "build_core_windows.sh: ⚠️ 这里**不降级**：没有 objdump 就没法验产物是不是 PE、依赖了哪些 DLL（W-1），" >&2
  echo "build_core_windows.sh:    而'编出来了但没验'正是本脚本要防的那件事。" >&2
  exit 1
fi
echo "build_core_windows.sh: 工具链就绪（$("${CARGO}" --version)，目标 ${TARGET}）" >&2

# ---- 2) 交叉编译 ------------------------------------------------------------
#
# ⚠️ **strip 是这一轮的修复（审查裁定 II 的"免费那一刀"）**：产物原本在**尾部**多带
#    1.64 MB 的死重 —— COFF 符号表 377,658 B + 字符串表 1,262,447 B（= 1,640,105 B，
#    实测 17,878,087 → 16,240,128 字节）。这些东西：
#      · 对"跑起来"毫无用处（PE 的导入表在 `.idata`，不在符号表里）；
#      · 会被壳侧（任务 22）**再内嵌一次** ⇒ 死重按倍数传进交付包。
#    ⇒ 用 rustc 自己的 `-C strip=symbols`（经 `--config` 传 profile），**不是**事后拿
#      `strip` 工具改 cargo 的产物：产物"生下来就是最终的"，壳侧那条"内嵌的是哪一份"
#      的顺序约束不必再考虑"谁在 cargo 之后动过这个文件"。
#    ⚠️ **只从本脚本传，不改 `core/Cargo.toml` 的 `[profile.release]`**：那会连 macOS 的
#      release 产物一起改，超出这条构建路径的范围（本任务只管 Windows 内核）。
#    ⚠️ 这一刀**不解决**下面这条（**已转给任务 22**）：`.rdata` 里 `include_bytes!` 的
#      aria2c 有**两份逐字节相同的复制**（5,649,408 B × 2 = 11,298,816 B = 全文件的 63%）。
#      根因是 rustc 按 codegen unit 复制 `include_bytes!` 常量，`-C codegen-units=1` 可去重
#      （实测 17,878,185 → 11,937,653）。去重会改变 codegen 配置（全量重编 + 与体积预算
#      一起决策），**不在本任务里做**。
STRIP_CONFIG='profile.release.strip="symbols"'
echo "build_core_windows.sh: 交叉编译 ${BIN_NAME}（--release，目标 ${TARGET}，strip=symbols）" >&2
(cd "${CORE}" && "${CARGO}" build --release --bin "${BIN_NAME}" --target "${TARGET}" --target-dir "${TARGET_DIR}" --config "${STRIP_CONFIG}")

# ⚠️ **`#[cfg(windows)]` 的那些代码，只有交叉编译能在本机把它们编到。**
#    本机是 macOS：宿主 `cargo test` 编不到那些分支，于是"Windows 上编不过"会一直躲到
#    真机上才现形。`--tests` 这一条把**测试代码里**的 `#[cfg(windows)]` 也编一遍
#    （`core/src/engine/daemon.rs` 里有若干条），宿主测试是全绿的、只有这条会红。
#    ⚠️ 它只**编**不**跑**：本机跑不了 Windows 的测试二进制。
echo "build_core_windows.sh: 编译检查（--tests）——保证 #[cfg(windows)] 的那些分支真的编得过" >&2
#     ⚠️ 与上面那次用**同一个** `--config`：profile 不同会让 cargo 认为指纹变了，
#        两条命令会各自全量重编一遍（白烧几分钟）。
(cd "${CORE}" && "${CARGO}" check --quiet --tests --bin "${BIN_NAME}" --target "${TARGET}" --target-dir "${TARGET_DIR}" --config "${STRIP_CONFIG}")

# ---- 3) W-1：只看字节，不信参数 --------------------------------------------
if [ ! -f "${BIN}" ]; then
  echo "build_core_windows.sh: 错误：构建结束，但产物不在 ${BIN}。" >&2
  echo "build_core_windows.sh: 路径由脚本位置 + 目标三元组 + [[bin]] name 推出（上面那次 cargo 用的是" >&2
  echo "build_core_windows.sh: --target-dir ${TARGET_DIR}）。" >&2
  echo "build_core_windows.sh: 常见原因一：core/Cargo.toml 的 [[bin]] name 被改名。" >&2
  echo "build_core_windows.sh: 常见原因二：cargo 额外吃了别处的配置（.cargo/config.toml 里改 target-dir？）。" >&2
  echo "build_core_windows.sh: 补救：核对上面两处，或把本脚本的路径一并改掉。" >&2
  exit 1
fi

file_line="$(file -b "${BIN}")"
echo "build_core_windows.sh: file → ${file_line}" >&2

# 3a) 架构与位数：必须是 PE32+ / x86-64。
#     ⚠️ 判据是**固定字符串**（`grep -F`），不是正则：`PE32+` 里的 `+` 在 BRE 里是字面量、
#     在 ERE 里是量词，而 `\+` 是 **GNU BRE 的扩展**（= "一个或多个"）——**三家实现语义不同**：
#     GNU grep 当量词（在本行上恰好也能命中），BSD grep 与 ugrep 则另外解释（恒假）。
#     差一个字符就从"恒真"变成"恒假"，而它一变，**一份完全正确的产物就被判成"不是 PE32+"**。
#     第一版写的就是 `'PE32\+ executable'`，在本机**恒假** —— 实测踩到。
#     ⚠️ **根因订正（2026-09-18 审查带出，别再把这条记成 ugrep）**：脚本运行时拿到的是
#     `/usr/bin/grep` = **BSD grep 2.6.0**。交互 shell 里的 `grep` 可能是某个 shell **函数**
#     （Claude Code 就装过一个），但**函数不会被 `bash script.sh` 继承**，`env` 里也没有
#     `BASH_FUNC_grep%%`；`/opt/homebrew/bin/grep` 根本不存在。所以"本机是 ugrep"这句**是错的**
#     —— 当时是从**交互 shell 里的行为**反推脚本的行为，推错了。
#     结论不变且更强：`-F` 是 POSIX 固定字符串、三家实现语义相同 ⇒ 这类差别从根上消失。
#     ⚠️ 用 here-string 而不是 `file … | grep -q …`：`grep -q` 命中即退出，会给上游
#     `file` 一个 SIGPIPE，而 `set -o pipefail` 会把那条管道判成失败——又一个**假的**红。
#     （本行的 `file` 输出只有一行、撞不上，但这条纪律写死在这里更省事。）
if ! grep -qF 'PE32+ executable' <<<"${file_line}"; then
  echo "build_core_windows.sh: 错误：产物不是 PE32+（不是 64 位 Windows 可执行文件）。file 的原话：${file_line}" >&2
  echo "build_core_windows.sh: 补救：确认第 2 步的 --target 是 ${TARGET}（产物应落在 ${TARGET_DIR}/${TARGET}/ 下）；" >&2
  echo "build_core_windows.sh:   若 file 认不出来（报成 data 之类），检查产物是不是被别的东西覆盖了。" >&2
  exit 1
fi
if ! grep -qF 'x86-64' <<<"${file_line}"; then
  echo "build_core_windows.sh: 错误：产物不是 x86-64。file 的原话：${file_line}" >&2
  echo "build_core_windows.sh: 补救：核对该目标三元组是不是被改成了 aarch64-pc-windows-*。" >&2
  exit 1
fi
echo "build_core_windows.sh: ✓ 架构（file）：PE32+ x86-64" >&2

# 3a') 同一条判断，换**第二个工具**再读一遍。
#      `file` 报的是 PE 头里的机器类型，`objdump` 报的是 binutils 自己解析出来的
#      `file format pei-x86-64` —— 两次读的**不是同一段代码**，所以两个都对上才算数。
#      W-1 的原话是"不许相信参数传对了"；同一条纪律也适用于"不许只相信一个工具的转述"。
#      （`objdump -f` 跑不起来时**不降级**：读不出头部就是自验没跑成，不是"大致没问题"。）
objdump_f=""
if ! objdump_f="$(x86_64-w64-mingw32-objdump -f "${BIN}" 2>&1)"; then
  echo "build_core_windows.sh: 错误：objdump -f 读不了 ${BIN} —— 自验没跑成，不能放行。" >&2
  echo "build_core_windows.sh: objdump 的原话：${objdump_f}" >&2
  echo "build_core_windows.sh: 补救：手工跑 \`x86_64-w64-mingw32-objdump -f ${BIN}\` 看它的原话。" >&2
  exit 1
fi
if ! grep -qF 'pei-x86-64' <<<"${objdump_f}"; then
  echo "build_core_windows.sh: 错误：objdump 读出的目标格式不是 pei-x86-64。它的原话：" >&2
  echo "${objdump_f}" >&2
  echo "build_core_windows.sh: 这与上面 `file` 的结论冲突时**以失败论**：两个工具都读对才会打印成功。" >&2
  exit 1
fi
# ⚠️ 逐行打印（并跳过空行）：本机这份 objdump 的输出**开头就有一个空行**，
#    直接 `head -1` 会打出一片空白（第一版就这么干了，看上去像"读到了空的东西"）。
echo "build_core_windows.sh: ✓ 头部（objdump -f 原样）：" >&2
while IFS= read -r objdump_line; do
  if [ -n "${objdump_line}" ]; then
    echo "build_core_windows.sh:   ${objdump_line}" >&2
  fi
done <<<"${objdump_f}"

# 3b) **子系统必须是 console** —— 内核的 stdout 是**协议专用通道**（每行一条 JSON）。
#     这不是风格问题：内核用 GUI 子系统就**没有** stdout（GUI 进程不接控制台），
#     壳按行读 JSON 的那条管道会当场空掉。所以这一条与壳侧**恰好相反**
#     （`windows/scripts/build_windows.sh` 要求 (GUI)，且理由同样是硬的：壳不该弹黑窗）。
if ! grep -qF '(console)' <<<"${file_line}"; then
  echo "build_core_windows.sh: 错误：产物不是 console 子系统。file 的原话：${file_line}" >&2
  echo "build_core_windows.sh: 内核的 stdout 是协议通道（每行一条 JSON），GUI 子系统没有 stdout，" >&2
  echo "build_core_windows.sh: 壳按行读 JSON 会当场空掉。" >&2
  echo "build_core_windows.sh: 补救：查 core/src/main.rs 顶上的 #![windows_subsystem] / .cargo/config.toml" >&2
  echo "build_core_windows.sh:   里有没有给这个 bin 设 GUI 子系统（**不该有**）。" >&2
  exit 1
fi
echo "build_core_windows.sh: ✓ 子系统：console（落点：内核的 stdout 协议通道还在）" >&2

# 3c) 依赖 DLL：**不许可 mingw 运行时、不许可任何非系统 DLL**。
#
#     两道网，缺一不可：
#       (1) **黑名单**：mingw 的三个运行时 DLL，出现即硬失败。内核 exe 会被壳**内嵌**进
#           交付包，它必须自包含——客户机器上没有 mingw。
#       (2) **白名单**：不认识的 DLL 也硬失败。理由不是"系统里没有"（客户机器都是 Windows），
#           而是：一个新引入的非系统依赖（比如某个 crate 拖来的 zlib1.dll）如果只是被
#           "打印出来"，没有人会去看；而它到了客户机器上就是"缺 DLL 起不来"。
#           要放行就**显式**把名字加进白名单——那是一次能被审查的决定，而不是一次静默的漂移。
dlls="$(x86_64-w64-mingw32-objdump -p "${BIN}" | awk '/^[[:space:]]*DLL Name:/ {print $3}')"
if [ -z "${dlls}" ]; then
  echo "build_core_windows.sh: 错误：objdump 没能列出任何依赖 DLL —— 自验没跑成，不能放行。" >&2
  echo "build_core_windows.sh: 补救：手工跑 \`x86_64-w64-mingw32-objdump -p ${BIN}\` 看它的原话。" >&2
  exit 1
fi
echo "build_core_windows.sh: 依赖 DLL（objdump -p 原样）：" >&2
printf 'build_core_windows.sh:   - %s\n' ${dlls} >&2

# ⚠️ 两张表里写的是**不带 `.dll` 后缀**的名字，而 objdump 打出来的**带**后缀——
#    所以比较之前统一把后缀剥掉（下面的 `dll_base`）。这条注释是**踩过之后**才写的
#    （壳侧同形的坑）：白名单写成带后缀的名字、比较时又不剥，于是**每一个** DLL 都被判成
#    "不认识"，报错信息却长得像"新引入了一堆依赖"——**根因说错了**。
#    ⚠️ `msvcrt` 是 mingw 给 Windows 自带 CRT 起的**导入名**，不是要随包发的那个 DLL，
#       所以在白名单里（它在任何 Windows 上都有）。
DENY_DLLS="libgcc_s_seh-1 libgcc_s_dw2-1 libwinpthread-1 libstdc++-6"
ALLOW_DLLS="kernel32 user32 advapi32 bcrypt bcryptprimitives ntdll userenv ws2_32 wsock32 \
msvcrt ucrtbase shell32 ole32 oleaut32 shlwapi iphlpapi secur32 crypt32 \
psapi version setupapi sechost comdlg32 comctl32 winmm"
#   ⚠️ 收进表里的判据是"**Windows 自带的系统组件**"（客户机器上一定有），不是"本次观测到的
#      这几个"。后者会让白名单随时漂移，而漂移过的白名单等于没有。

# `DLL Name:` 打出来的是 `KERNEL32.dll` 这样的形状：转小写、剥掉 `.dll`。
dll_base() { printf '%s' "$1" | tr 'A-Z' 'a-z' | sed 's/\.dll$//'; }

# 判"这一条在系统白名单里吗"——**只写一处**，守卫与正式检查都走它（守卫才有判别力）。
is_allowed_dll() {
  local low
  low="$(dll_base "$1")"
  # 系统 API set 与 CRT 的转发 DLL：名字带这个前缀的一律是 Windows 自带的转发层。
  case "${low}" in
    api-ms-win-*|ext-ms-win-*) return 0 ;;
  esac
  local ok
  for ok in ${ALLOW_DLLS}; do
    if [ "${low}" = "${ok}" ]; then return 0; fi
  done
  return 1
}

# 「守卫的守卫」：拿**一定在**的依赖走一遍**同一个**匹配函数。匹配逻辑自己坏掉时
# （比如 `.dll` 后缀没剥），下面那条正式检查会以"出现了不认识的依赖 DLL"报错——那是
# **错误的根因**，会把人送去查依赖，而该改的是本脚本。一个自己写错的守卫比没有守卫更糟：
# 它给出的是**虚假的安全感**。
#
# ⚠️ **必须探两条，各覆盖一条判定路径**（2026-09-18 审查带出）：
#   · `KERNEL32.dll`      → 覆盖 `ALLOW_DLLS` **精确名单**那一条；
#   · `api-ms-win-core-…` → 覆盖 `api-ms-win-*|ext-ms-win-*` **前缀规则**那一条。
#   只探第一条的话，**前缀规则坏掉时守卫照样放行**，而正式检查会以"出现了白名单以外的
#   依赖 DLL"报错、并把 8 条 api-ms 全列成"不认识"——那正是这条守卫存在的理由。
#   （`api-ms-win-core-synch-l1-2-0` 也是**本产物真的依赖**的一条，不是凭空挑的名字。）
# ⚠️ 这段的自证方式（把探针换成不在白名单里的名字 ⇒ 必须报"本脚本写错了"；把
#    `is_allowed_dll` 里的前缀分支删掉 ⇒ **第二条探针必须红**）写在 task-5 的报告里。
GUARD_PROBE_DLLS="KERNEL32.dll api-ms-win-core-synch-l1-2-0.dll"
for guard_probe in ${GUARD_PROBE_DLLS}; do
  if ! is_allowed_dll "${guard_probe}"; then
    echo "build_core_windows.sh: 错误：**本脚本写错了** —— 自验白名单的匹配逻辑本身坏了" >&2
    echo "build_core_windows.sh:   （拿 ${guard_probe} 走 is_allowed_dll，没匹配上任何一项）。" >&2
    echo "build_core_windows.sh: 这**不是**产物的依赖有问题。常见原因：白名单里写的是带 .dll 后缀的名字，" >&2
    echo "build_core_windows.sh:   而 dll_base 剥掉后缀后就对不上了；或者白名单/前缀规则被改坏了。" >&2
    echo "build_core_windows.sh: 若不先自证，下面会以'出现了不认识的依赖 DLL'报错——那是错误的根因。" >&2
    exit 1
  fi
done

unknown_dlls=""
for dll in ${dlls}; do
  low="$(dll_base "${dll}")"
  # (1) 黑名单：mingw 运行时 —— 出现即硬失败。
  for bad in ${DENY_DLLS}; do
    if [ "${low}" = "${bad}" ]; then
      echo "build_core_windows.sh: 错误：产物依赖了 mingw 运行时 DLL：${dll}" >&2
      echo "build_core_windows.sh: 这个 exe 会被壳内嵌进交付包，客户机器上没有 mingw —— 会缺 DLL 起不来。" >&2
      echo "build_core_windows.sh: 补救：静态链接（RUSTFLAGS 里加 -C target-feature=+crt-static，或" >&2
      echo "build_core_windows.sh:   给该 crate 关掉动态运行时的特性）。**不要**改成把 DLL 一起发。" >&2
      exit 1
    fi
  done
  # (2) 白名单：系统 API set（前缀）+ 名单内的系统 DLL。
  if ! is_allowed_dll "${dll}"; then
    unknown_dlls="${unknown_dlls} ${dll}"
  fi
done

if [ -n "${unknown_dlls}" ]; then
  echo "build_core_windows.sh: 错误：出现了白名单以外的依赖 DLL：${unknown_dlls}" >&2
  echo "build_core_windows.sh: 两种可能，都要人来判：" >&2
  echo "build_core_windows.sh:   ① 它是这次改动**新引入的非系统依赖** —— 那正是这道网要抓的东西" >&2
  echo "build_core_windows.sh:      （客户机器上没有它 ⇒ '缺 DLL 起不来'）；" >&2
  echo "build_core_windows.sh:   ② 它是合法的系统 DLL，只是白名单还没有它 —— 那就把名字**显式**加进" >&2
  echo "build_core_windows.sh:      本脚本的 ALLOW_DLLS（不带 .dll 后缀），让这次放宽成为一次能被审查的决定。" >&2
  exit 1
fi
#     ⚠️ "自包含"这句话**以 W-7（最低支持 Windows 10）为准**：`api-ms-win-crt-*` 是
#        **UCRT 的转发层**，Windows 10 起随系统提供；Win7/8 上要有这些名字，得先装
#        Universal CRT 更新。本脚本的判据（"是不是系统 DLL"）在几代上都成立，但
#        "客户机不用装任何东西"这句**只在 W-7 范围内**成立——别读成"任意 Windows 上都跑"。
echo "build_core_windows.sh: ✓ 依赖：只有系统 DLL，没有 mingw 运行时（自包含**以 W-7 为准**：Windows 10 起）" >&2

# 3d) **strip 真生效了吗**（这一层是"审查裁定 II 那一刀"的自验）。
#     判据是**COFF 符号表不存在**：`objdump -x` 会打 `SYMBOL TABLE:` 后紧跟 `no symbols`。
#     不验这一条的话，谁把 `--config` 那一行删掉/改错，尺寸就静默涨回 1.64 MB ——
#     而"静默涨回去"正是这次修复要防的形态（它会被壳侧再内嵌一次）。
#     ⚠️ 用 `grep -A1`（不带 -q/-m）：它会读完整段输入，不会给上游 objdump 一个 SIGPIPE
#        ——`grep -q` 命中即退，在 `set -o pipefail` 下会制造一个**假的红**（本脚本已踩过）。
symtab="$(x86_64-w64-mingw32-objdump -x "${BIN}" | grep -A1 '^SYMBOL TABLE:')"
if ! grep -qF 'no symbols' <<<"${symtab}"; then
  echo "build_core_windows.sh: 错误：产物里还带着 COFF 符号表 —— strip 没生效。" >&2
  echo "build_core_windows.sh: 现在的样子：${symtab}" >&2
  echo "build_core_windows.sh: 补救：确认上面那次 cargo 带了 --config '${STRIP_CONFIG}'（约 1.64 MB 的死重）。" >&2
  echo "build_core_windows.sh:   ⚠️ 这份死重会被壳侧**再内嵌一次**，所以它必须在这里当场发现。" >&2
  exit 1
fi
echo "build_core_windows.sh: ✓ 符号表：已 strip（尾部没有 COFF 符号表/字符串表）" >&2

# ---- 4) 产物摘要（给报告/给壳侧内嵌时核对用）--------------------------------
# ⚠️ 只打印**证据**，不下"能在 Windows 上跑"的结论：本机是 macOS，这是交叉编译。
#    任何"跑过了"的断言都只能来自 Windows 机器（W-1 只谈产物自验）。
#
# ⚠️ 下面那个 sha256 是**本次构建**的指纹，**不是**可复现性判据——本机实测（2026-09-18）：
#    **同一份源码**（一个字节都没改）连着编两次，产物的 sha256 就不一样（大小相同）。
#    根因看得见：PE 的 COFF 头里有 `TimeDateStamp`，每次链接都写当时的时刻
#    （`x86_64-w64-mingw32-objdump -p` 的 "Time/Date" 行，实测两次构建相差 11 秒）。
#    ⇒ 谁要拿它判"壳内嵌的是不是**这一次**编出来的内核"，只能拿**同一份产物文件**比
#      （`windows/scripts/build_windows.sh` 里那条"内核必须先编好"的顺序约束就是为此），
#      **不能**靠"重新编一遍看看哈希对不对"——那永远不会相等。
#    ⇒ 还有一条**没查清机理**的实测（照实记下，别当结论）：在 `paths.rs` 文件头加**一行
#      注释**后重编，产物少了 61 字节（17878087 → 17878026）。注释不参与代码生成，
#      所以这 61 字节的来源本机没查明（PE 里既没有源文件路径字符串，也没有调试目录）。
#      结论只要这一条：**别把产物 sha256 当成源码状态的指纹。**
size_bytes="$(wc -c <"${BIN}" | tr -d ' ')"
if command -v sha256sum >/dev/null 2>&1; then
  sha="$(sha256sum "${BIN}" | cut -d' ' -f1)"
else
  sha="$(shasum -a 256 "${BIN}" | cut -d' ' -f1)"
fi
echo "build_core_windows.sh: 产物：${BIN}" >&2
echo "build_core_windows.sh: 大小：${size_bytes} 字节（$(du -h "${BIN}" | cut -f1)）" >&2
echo "build_core_windows.sh: sha256：${sha}" >&2
echo "build_core_windows.sh: ✓ 交叉编译 + 产物自验通过（**本机未运行**过这个 exe）" >&2
