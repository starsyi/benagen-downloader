#!/usr/bin/env bash
# 为 Linux（x86_64-musl）构建 `benagen-dl` 的**单文件静态**分发产物。
#
# 构建机：root@<构建机>（<构建机主机名>，Oracle Linux 9.5，x86_64）
# **在那一台机器上**以 root 运行本脚本。
#
#   ① 在 Mac 上把源码送过去（**只碰 ${BUILD_ROOT}，绝不碰生产目录**）：
#        rsync -a --delete --exclude 'target/' --exclude '.git/' --exclude 'dist_cli/' \
#              --exclude 'macos/.build/' --exclude 'downloader/dist/' \
#              --exclude '.superpowers/' --exclude '.claude/' --exclude '__pycache__/' \
#              ./ root@<构建机>:/root/benagen-cli-build/src/repo/
#      （实测：这几条排除**不是可选的**。带上 .git / target / .claude 是 15 GB 量级，
#        排除之后源头总共 159 MB —— 这条链路只有 ~400 KB/s。）
#   ② 在那台机器上跑本脚本：
#        scp core/scripts/build_cli_linux.sh root@<构建机>:/tmp/
#        ssh root@<构建机> 'bash /tmp/build_cli_linux.sh'
#
# 产物：$BUILD_ROOT/out/benagen-dl-linux-x86_64（静态、能跑，末尾打印 file/readelf/sha256）
#       取回：scp root@<构建机>:/root/benagen-cli-build/out/benagen-dl-linux-x86_64 \
#                 dist_cli/benagen-dl-linux-x86_64
#
# ---------------------------------------------------------------------------
# 🔴 生产机纪律（本脚本的每一处路径都受它约束）
# ---------------------------------------------------------------------------
#   · 只写 `$BUILD_ROOT`（默认 `/root/benagen-cli-build`），仓库源码在
#     `$BUILD_ROOT/src/repo`。**不碰** `/root/fenghang/fastapi_auto_script/script`（生产主控）
#     与 `/bnzk1/tosup`（工作根）—— 本脚本里没有、也不许出现这两条路径。
#   · **不 dnf 装东西、不升级系统**：musl 交叉链与静态 OpenSSL 都已经在
#     `$BUILD_ROOT/build/musl/bin` 与 `$BUILD_ROOT/src/` 里（Task 1 的产物）。
#   · `aria2c-linux-x86_64` 那份资产**已经入库**在 `core/assets/`（Task 8 入的，
#     就是 Task 1 在 `$BUILD_ROOT/out/` 里编出来的那一份），所以本脚本**不重新编它**、
#     也不调 `build_aria2_linux.sh` —— 它只**对一次**摘要（见下面的前置检查），
#     把本产物内嵌的引擎一路钉回 Task 1 的产出。要重编 aria2c 是另一件事、另一个脚本。
#
# ---------------------------------------------------------------------------
# ⚠️ 必须用 rustup 那份 cargo（`/root/.cargo/bin/cargo`），不能用别的
# ---------------------------------------------------------------------------
#   这台机器上 rustup 装在 `/root/.cargo/bin`，musl 的 std 在
#   `/root/.rustup/toolchains/…` 里。**别的 cargo（例如系统包管理器装的那份）看不到它**，
#   表现是编到**第三方依赖**（`cfg-if` / `litemap` 这些）时报
#       error[E0463]: can't find crate for `core`
#   —— 那个报错**不是本仓的问题**，是 cargo 选错了。所以本脚本显式查这一条并大声失败。
#
# ---------------------------------------------------------------------------
# 本脚本不只"编出来"，还要在那台机器上**真跑**（两件事都做，缺一不可）
# ---------------------------------------------------------------------------
#   1. `cargo test`：**这是全案唯一能执行 Linux 靶测试的地方**。`cli_e2e` 那条要起
#      **真的 aria2c**（内嵌的那份）＋桩 HTTP 服务，本机（macOS）跑不了它。
#      跳过它等于"这三条 Linux 靶的测试从没被执行过"。
#   2. `benagen-dl --version / --help`：跑的是**最终产物那个二进制**，
#      不是 cargo 的中间物。
#
#   ⚠️ **不包含**一次"拿真交付码真下载"——那需要真的交付码，不该硬编码进构建脚本。
#      构建之后请手动跑一次，那是"Linux 产物真能用"的最强证据：
#          cd $BUILD_ROOT && ./out/benagen-dl-linux-x86_64 <真交付码> -o $BUILD_ROOT/dl
#          echo "退出码=$?"
#      到不了 download.benagen.com 时，改用桩服务（`BENAGEN_BASE_URL` 指过去），
#      但**必须有一条真跑通的证据** —— "编出来"不等于"能用"。
#
# 退出码：0 = 产物已出且自验全过；1 = 自验或测试没过；2 = 用法/前置缺失（配方已打印）。
set -euo pipefail

BUILD_ROOT="${BUILD_ROOT:-/root/benagen-cli-build}"
REPO_DIR="${REPO_DIR:-$BUILD_ROOT/src/repo}"
OUT="$BUILD_ROOT/out"
CARGO_BIN_DIR="${CARGO_HOME:-/root/.cargo}/bin"
TARGET="x86_64-unknown-linux-musl"
BIN_NAME="benagen-dl"
ARIA2C_ASSET="aria2c-linux-x86_64"
HASH_PREFIX_LEN=12

log() { printf '\n==> %s\n' "$*"; }
ok()  { printf '    \033[32m✓\033[0m %s\n' "$*"; }
die() { printf '\n!! %s\n' "$*" >&2; exit "${2:-1}"; }

# ---------------------------------------------------------------------------
# 新鲜度自验：**产物必须比它的全部输入都新**（具名函数，可单独行使）
# ---------------------------------------------------------------------------
# 为什么现有的判据都替代不了它：`file` / `readelf` / `--version` 摘要 / `--help`
# 钉的是"**内嵌的 aria2c 是哪一份**"，**钉不住"这份二进制是不是刚编出来的"**。
# 一份陈旧、但内嵌资产恰好没换过的产物，能把那四条全部过掉 ——
# 而它承载的是**旧源码**。
#
# 输入清单 = **cargo build --release 真正的输入**：`src/`、`assets/`（include_bytes!
# 与 build.rs 都读它）、`build.rs`、`Cargo.toml`、`Cargo.lock`。
# ⚠️ **`tests/` 刻意不在清单里**：release 二进制不依赖它，把它算进来会让
#    "改一行测试 ⇒ 必须重编一份字节完全相同的二进制才算新鲜"，那是一条假红。
#    测试那侧由本脚本第 4 步的 `cargo test` 单独把关。
#
# 判据交给 `find -newer` 逐文件判，不自己解析 mtime。
# 返回：0 = 新鲜；1 = 陈旧或判据不成立（比它新的输入已打到 stderr）。
verify_fresh() {  # $1 = 二进制，$2 = 源码根（core/）
  local bin="$1" root="$2" newer
  # **判据的适用前提先立住**：少任何一条，就**判死**而不是返回"新鲜"
  # —— "判据跑不起来"与"判据通过"是两件事。
  [ -f "$bin" ] \
    || { printf '新鲜度判据不成立：找不到产物 %s\n' "$bin" >&2; return 1; }
  [ -f "$root/Cargo.toml" ] \
    || { printf '新鲜度判据不成立：%s 不像源码根（没有 Cargo.toml）\n' "$root" >&2; return 1; }
  # ⚠️ `src/` 也在前提里：它**不存在或是空的**时，下面的 `find` 一个输入都收不到，
  #    于是"没有比产物更新的输入"成立 ⇒ 判据**恒真**。
  #    （修复轮 3 实测：`src/` 缺失与空目录两种形态都返回 0 —— 一条空判据。）
  #    判据跑不起来 ≠ 判据通过，所以这里也判死。
  if [ ! -d "$root/src" ] || [ -z "$(ls -A "$root/src" 2>/dev/null)" ]; then
    printf '新鲜度判据不成立：%s/src 不存在或是空的 —— 没有输入可比，这条判据是空的\n' "$root" >&2
    return 1
  fi
  newer="$(find "$root/src" "$root/assets" "$root/build.rs" \
                 "$root/Cargo.toml" "$root/Cargo.lock" \
                 -type f -newer "$bin" 2>/dev/null | head -20)"
  if [ -n "$newer" ]; then
    printf '比这份产物更新的输入（最多列 20 条）：\n%s\n' "$newer" >&2
    return 1
  fi
  return 0
}

# ---------------------------------------------------------------------------
# 测试闸门的新鲜度：**这一轮 cargo test 跑的测试二进制，必须出自当前的测试源码**
# ---------------------------------------------------------------------------
# 为什么不能只看 cargo：cargo 判"要不要重编"靠 **mtime 指纹**，源码 mtime 一旦
# **倒退**到产物之后（`rsync -a` 保 mtime；从备份 `cp` 回来也常带回旧 mtime），
# cargo 就判"没变"、直接复用上一次编出来的测试二进制。
#
# **本机实测（2026-09-21，命令与读数见任务报告 §10.1）**：
#   ① 改了 `tests/cli_e2e.rs` 的内容、再把它的 mtime `touch -t 202001010000` 倒回
#      ⇒ `cargo test --test cli_e2e --no-run` 只打 `Finished`、**没有 Compiling**，
#        二进制的 md5 与 mtime **一字未动** —— 内容是新的，跑的是旧的；
#   ② 只把 mtime `touch` 到"现在"（内容与 ① 完全相同）
#      ⇒ 打出 `Compiling benagen-core`，二进制被重写。
#   ⇒ 光看 cargo 不够，得有一道**独立的**后置断言。
#
# 判据：从 cargo test 的日志里抠出 `Running tests/<x>.rs (<路径>)` 这些对，
# 逐个断言那个二进制**比它自己的源文件新**。
# ⚠️ 每个二进制**只跟自己那份源码比**：拿"`tests/` 下最新的一份"去比，
#    会在只改了一个测试文件时误伤另一个目标（假红）。
# ⚠️ 只看 `tests/` 那几行（`Running unittests src/lib.rs` 之类不算）：库产物不依赖
#    `tests/`，拿它去比同样是假红 —— 与 `verify_fresh` 里排除 `tests/` 是同一条理由。
#
# 🔴 **这条断言在它被写出来专门要抓的那个场景里是恒真的**（修复轮 3 的 N-1 订正，
#    审查实测 + 我复现）——**不许**把它当成"能发现复用"的判据：
#      cargo 的脏判据是 `源 mtime > 产物 mtime`；"复用"的定义就是这个式子**不成立**，
#      也就是 `产物 mtime >= 源 mtime` —— 而这**正好**是本函数的谓词（"源不比产物新"）。
#      ⇒ 复用一旦发生，本函数必然返回 0。
#      复现（沙箱，产物 mtime 晚于源）：`verify_test_bins_fresh` ⇒ **返回码=0**。
#    它唯一会响的方向是"**产物比源旧**"，而 cargo 自己**不会留下**那种状态
#    （它每次编完都会把产物 mtime 刷新）。所以：
#
#    ⇒ **真正让"这一轮跑的是当前源码"成立的，是下面那次 `cargo clean -p --profile test`
#       （强制现编），不是这条断言。**
#    ⇒ 这条断言是**形状上的兜底**（万一将来 cargo 换成内容哈希、或者有人手工塞了一份
#       旧二进制进来，它才会响）。**不要**把它当成第 5 步的承重判据。
#    ⇒ 也**不要**为了"简化"把上面那次 `cargo clean` 删掉：删掉之后，复用会**静默**
#       发生（读数照样绿，而跑的是旧代码），本断言不会拦。
verify_test_bins_fresh() {  # $1 = cargo test 的日志，$2 = 源码根（core/）
  local log="$1" root="$2" rc=0 src bin pairs
  pairs="$(sed -n 's|^ *Running \(tests/[^ ]*\.rs\) (\(.*\))$|\1\t\2|p' "$log" | sort -u)"
  [ -n "$pairs" ] \
    || { printf '测试新鲜度判据不成立：日志里一条 "Running tests/…" 都没有\n' >&2; return 1; }
  while IFS=$'\t' read -r src bin; do
    [ -n "$src" ] || continue
    # ⚠️ cargo 在日志里写的是**相对路径**（相对它自己的工作目录，也就是 `$root`），
    #    除非显式设了绝对的 `CARGO_TARGET_DIR`。**实测**：不补这一步，
    #    判据会在自己身上红（"找不到产物 target/debug/deps/…"）—— 那不是测试的问题，
    #    是判据没解析路径。这里按"不是绝对路径就挂到源码根上"处理。
    case "$bin" in
      /*) : ;;
      *)  bin="${root}/${bin}" ;;
    esac
    if [ ! -f "$bin" ]; then
      printf '测试新鲜度判据不成立：日志说跑了 %s，但找不到产物 %s\n' "$src" "$bin" >&2
      rc=1; continue
    fi
    if [ -n "$(find "${root}/${src}" -newer "$bin" 2>/dev/null)" ]; then
      printf '测试二进制比它的源码**旧**（cargo 复用了旧产物，这份读数不算数）：\n' >&2
      printf '  源  ：%s\n        %s\n' "${root}/${src}" "$(stat -c '%y' "${root}/${src}")" >&2
      printf '  产物：%s\n        %s\n' "$bin" "$(stat -c '%y' "$bin")" >&2
      rc=1
    fi
  done <<< "$pairs"
  return "$rc"
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  awk 'NR == 1 { next } /^set / { exit } { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  exit 0
fi
[ "$(id -u)" = "0" ] || die "本脚本要在构建机上以 root 跑（它写 ${BUILD_ROOT}）。" 2

# ---------------------------------------------------------------------------
# 0) 前置：cargo 是不是 rustup 那份（见文件头那条 E0463）
# ---------------------------------------------------------------------------
# 判据是**路径相等**，不是"cargo 命令存在"：编依赖时红在 `can't find crate for core`
# 那种报错把人往"目标没装"的方向引，而真正的原因只是 PATH 上另一份 cargo 先被找到。
log "0/5 前置检查"
# ⚠️ 这里**用绝对路径点名** rustup 那份 cargo，而不是让 PATH 去挑一个 ——
#    挑错的那一份在**编第三方依赖**时红在 `can't find crate for \`core\``（见文件头），
#    那个报错指向"目标没装"，与真正的原因（cargo 选错）毫无关系。
#    这不是"静默换个东西来跑"：用的是哪一份**就打印哪一份**，且下面会点出
#    PATH 上若另有一份 cargo、它就会被绕开 —— 免得人手工在同一个目录里敲 `cargo`
#    时撞上 E0463 而不知道原因。
CARGO="${CARGO:-$CARGO_BIN_DIR/cargo}"
[ -x "$CARGO" ] \
  || die "找不到 rustup 的 cargo：${CARGO}（musl 的 std 只装在它管的 toolchain 里）
     补救：curl -fsSL -o /tmp/rustup-init.sh https://sh.rustup.rs && sh /tmp/rustup-init.sh -y \\
             --no-modify-path --profile minimal --default-toolchain 1.89.0
           $CARGO_BIN_DIR/rustup target add $TARGET" 2
PATH_CARGO="$(command -v cargo || true)"
if [ -n "$PATH_CARGO" ] && [ "$PATH_CARGO" != "$CARGO" ]; then
  printf '    ⚠️ PATH 上的 cargo 是 %s；本脚本**故意绕开它**，用 %s。\n' "$PATH_CARGO" "$CARGO"
fi
ok "cargo = ${CARGO}（$("$CARGO" --version | head -1)）"

RUSTUP="$CARGO_BIN_DIR/rustup"
[ -x "$RUSTUP" ] || die "找不到 ${RUSTUP}（rustup 的 shim 不在？）" 2
if ! "$RUSTUP" target list --installed | grep -qx "$TARGET"; then
  die "rustup 没装 $TARGET 的 std（已装：$("$RUSTUP" target list --installed | tr '\n' ' ')）
     补救：$RUSTUP target add $TARGET" 2
fi
ok "$TARGET 的 std 已装"

[ -f "$REPO_DIR/core/Cargo.toml" ] || die "找不到 ${REPO_DIR}/core/Cargo.toml —— 源码没送到位。
     从 Mac 上先跑（只碰 ${BUILD_ROOT}）：
       rsync -a --delete --exclude target --exclude .git --exclude dist_cli \\
             --exclude macos/.build --exclude downloader/dist \\
             ./ root@<构建机>:$REPO_DIR/" 2

ARIA2C_SRC="$REPO_DIR/core/assets/$ARIA2C_ASSET"
[ -s "$ARIA2C_SRC" ] \
  || die "找不到内嵌资产 ${ARIA2C_SRC}（它是**入库**文件 —— 缺了说明源码没同步全，
     而不是要在这里重编 aria2c；重编走 core/scripts/build_aria2_linux.sh）" 2

# ---------------------------------------------------------------------------
# Task 1 参考产物：**硬判据**（参考件不在 = 停，不是跳过）
# ---------------------------------------------------------------------------
# `$OUT/aria2c-linux-x86_64` 是 `core/scripts/build_aria2_linux.sh` 在**那台机器上**
# 编出来的那一份；`core/assets/` 里的是**入库**副本（Task 8 入的，就是它）。
# 两者必须是同一个字节串 —— 这一条把"本产物内嵌的引擎"一路钉回 Task 1 那次受审的构建。
#
# ⚠️ **这一条曾经是可选的**（`if [ -f … ]` 包着，else 只打一句小字、不改退出码）：
#    在新机器或被清过的机器上它会**静默不跑**，而脚本照报"完成"。
#    一条会无声消失的判据正是本仓反复栽的那个坑，所以改成**硬判据**：
#    参考件不在就停在这里、并告诉操作者怎么把它产出来。
#    **没有**为它加任何环境变量开关 —— 要绕过它，就得先解释为什么这份产物的引擎
#    追溯不到 Task 1。
TASK1_ARIA2C="$OUT/$ARIA2C_ASSET"
[ -f "$TASK1_ARIA2C" ] \
  || die "找不到 Task 1 的参考产物：${TASK1_ARIA2C}
     本脚本要求把内嵌引擎**追溯到 Task 1 那次构建**（「入库的这份 == 构建机上那次
     编出来的那份」）。少了这个参考件，那条判据就静默不跑，脚本不该报"完成"。
     补救（在构建机上，一次性；它只写 ${BUILD_ROOT}）：
       scp core/scripts/build_aria2_linux.sh root@<构建机>:/tmp/
       ssh root@<构建机> 'bash /tmp/build_aria2_linux.sh'
     产出 ${OUT}/${ARIA2C_ASSET} 之后再跑本脚本。" 2
TASK1_SHA="$(sha256sum "$TASK1_ARIA2C" | awk '{print $1}')"
REPO_SHA="$(sha256sum "$ARIA2C_SRC" | awk '{print $1}')"
[ "$TASK1_SHA" = "$REPO_SHA" ] \
  || die "入库资产 core/assets/$ARIA2C_ASSET 与 Task 1 的产物 ${TASK1_ARIA2C} 不是同一个字节串：
     入库 ：${REPO_SHA}
     Task1：${TASK1_SHA}
     两者本应逐字相同（Task 8 就是把 Task 1 的产物入库的）。差异意味着某一侧被换过 ——
     请先弄清是哪一份对，再决定重编（core/scripts/build_aria2_linux.sh）还是更新入库副本。" 1
ok "入库资产与 Task 1 的产物逐字相同（sha256 ${TASK1_SHA}）"

# ---------------------------------------------------------------------------
# musl 交叉链：C 编译**必须**有它（这一条是实测撞出来的，不是预防性写的）
# ---------------------------------------------------------------------------
# 只装 rustup 的 musl **std** 是不够的：依赖树里的 `ring`（rustls 的密码学后端）
# 有 C 源码，它的 `cc-rs` 构建脚本要找 `x86_64-linux-musl-gcc`；PATH 上没有就报
#     error occurred in cc-rs: failed to find tool "x86_64-linux-musl-gcc"
# 这个报错发生在**编依赖**阶段，与"目标没装 std"是两回事（后者才是假 E0463）。
# 交叉链是 Task 1 用 `build_aria2_linux.sh` 建的，落在 `$BUILD_ROOT/build/musl/bin`
# （脚本里是 `TOOLCHAIN="$BUILD/musl"`）。本脚本**只把它挂到这一条编译命令的 PATH 上**
# —— 不 export、不影响后面的 `cargo test`（宿主 gnu 靶不该沾 musl 的 binutils）。
MUSL_BIN="${MUSL_BIN:-$BUILD_ROOT/build/musl/bin}"
[ -x "$MUSL_BIN/x86_64-linux-musl-gcc" ] \
  || die "找不到 musl 交叉链：${MUSL_BIN}/x86_64-linux-musl-gcc
     它是 Task 1 用 core/scripts/build_aria2_linux.sh 在这台机器上建的（默认落在
     \$BUILD_ROOT/build/musl/bin）。**不要** dnf 装一个来顶替 —— 那会换掉整条交叉链，
     请先确认 \$BUILD_ROOT 是不是被清过。" 2
ok "musl 交叉链 = ${MUSL_BIN}（$("$MUSL_BIN/x86_64-linux-musl-gcc" --version | head -1)）"

mkdir -p "$OUT"

# ---------------------------------------------------------------------------
# 1) 编译（release / musl）
# ---------------------------------------------------------------------------
# ⚠️ `-C relocation-model=static` **不是**可有可无的调优，它决定产物是不是判据要的那个形态。
#    实测（本机 rustc 1.89.0）：只加 `--target x86_64-unknown-linux-musl` 编出来的是
#    **位置无关的静态可执行文件**，`file` 的原话是
#        ELF 64-bit LSB pie executable, x86-64, …, **static-pie linked**, …
#    ——它确实没有 NEEDED、没有 PT_INTERP、`ldd` 也说 "statically linked"，
#    但 `file` 不会说 `statically linked` 这个字面读数，而判据要的正是后者（计划 Step 4）。
#    加上 `-C relocation-model=static` 之后 `file` 说的是
#        ELF 64-bit LSB executable, x86-64, …, **statically linked**, …
#    且 `readelf -d` 直接是 "There is no dynamic section in this file."。
#    两份都是"单文件、无动态依赖"，但 ET_EXEC 那一份不依赖内核的 static-PIE 加载路径，
#    在旧内核上更稳妥 —— 交付物的形态按后者。
log "1/5 编译（cargo build --release --target ${TARGET}，-C relocation-model=static）"
(cd "$REPO_DIR/core" && env PATH="$MUSL_BIN:$PATH" \
  RUSTFLAGS="-C relocation-model=static" "$CARGO" build --release --target "$TARGET")

BIN="$REPO_DIR/core/target/$TARGET/release/$BIN_NAME"
[ -x "$BIN" ] || die "cargo build 成功，却找不到可执行文件 $BIN" 2

# 新鲜度：产物必须是**从当前这棵树**编出来的（见 `verify_fresh` 旁边那段）。
# 位置在 `cp` 到 `out/` **之前** —— `cp` 会把副本的 mtime 刷成"现在"，
# 拿副本去比就成了一条永远为真的判据。
log "2/5 新鲜度自验（产物 vs ${REPO_DIR}/core 的输入）"
verify_fresh "$BIN" "$REPO_DIR/core" \
  || die "产物比它的输入还旧 —— 这份二进制不是从当前这棵树编出来的（上面列了更新的输入）。
     先查：cargo build 是不是被跳过了（比如 target/ 是从别处拷来的、或源码刚同步过）。" 1
ok "产物比 src/ assets/ build.rs Cargo.toml Cargo.lock 都新"

# ---------------------------------------------------------------------------
# 2) 自验：静态 / 无动态依赖 / 内嵌资产对得上 / 用法说明出得来
# ---------------------------------------------------------------------------
log "3/5 产物自验（${BIN}）"

FILE_OUT="$(file -b "$BIN")"
printf '    file：%s\n' "$FILE_OUT"
# 判据是 `file` 自己那句 `statically linked`。**不是**"没有任何 .so 字样"那种间接判断：
# 动态链接的产物也可能一个 libc 都不列出来（例如全静态的其它库），只有这句是直接的。
printf '%s' "$FILE_OUT" | grep -q 'statically linked' \
  || die "产物不是静态链接的（file 说：${FILE_OUT}）—— 本产物的形态就是「单文件静态」，
     退成动态链接等于换交付形态，那是产品决定，不许在这里静默发生。
     ⚠️ 若上面那句是 'static-pie linked'：那不是动态链接，但也不是判据要的字面读数 ——
        多半是上面那条 `-C relocation-model=static` 被去掉了，见它旁边的注释。" 1
ok "file = statically linked"

# 动态段里**一个 NEEDED 都不许有**。这是上一条的**独立**佐证：`file` 看的是 ELF 头里的
# 那一位，这里看的是**.dynamic 段的实际内容** —— 两者同时成立才算数。
#
# ⚠️ **stderr 必须与 stdout 分开**（原来是 `2>&1`，那是个静默通过的洞）：
#    合流之后，`readelf` **自己失败**时它的错误文本会让"输出非空"这个守卫放行，
#    接着 `grep NEEDED` 一条都找不到 ⇒ 判据"通过"。分开之后：
#      · `readelf` 的成败看**退出码**（失败就把它的 stderr 原样打出来再判死）；
#      · NEEDED 只在 **stdout** 里找（实测：静态产物那句
#        `There is no dynamic section in this file.` 是 **stdout**、rc=0，下面那条守卫
#        因此不会误伤它 —— 这一点在本轮的两台机器上都实测过）。
READELF_ERR="$(mktemp)"
set +e
DYNAMIC_OUT="$(readelf -d "$BIN" 2>"$READELF_ERR")"
READELF_RC=$?
set -e
READELF_ERR_TEXT="$(cat "$READELF_ERR")"
rm -f "$READELF_ERR"
[ "$READELF_RC" -eq 0 ] \
  || die "readelf -d 自己失败了（rc=${READELF_RC}）—— 这条判据没跑起来（不是通过）。
     readelf 的 stderr：${READELF_ERR_TEXT:-（空）}" 1
DYNAMIC_FIRST="$(printf '%s\n' "$DYNAMIC_OUT" | grep -v '^[[:space:]]*$' | head -1)"
[ -n "$DYNAMIC_FIRST" ] \
  || die "readelf -d 成功了但 stdout 是空的 —— 这条判据没跑起来（不是通过）。
     先确认这是个 ELF 文件。readelf 的 stderr：${READELF_ERR_TEXT:-（空）}" 1
if printf '%s' "$DYNAMIC_OUT" | grep -q 'NEEDED'; then
  printf '%s\n' "$DYNAMIC_OUT" | grep 'NEEDED' | sed 's|^|    |' >&2
  die "readelf -d 里出现了 NEEDED —— 产物有动态依赖，到了客户的机器上可能起不来。" 1
fi
ok "readelf -d 无 NEEDED（${DYNAMIC_FIRST}）"

# 内嵌 aria2c 的摘要：资产两条工具各算一遍（互相印证），再与产物 `--version` 对上。
WANT_SHASUM="$(sha256sum "$ARIA2C_SRC" | awk '{print $1}')"
WANT_OPENSSL="$(openssl dgst -sha256 "$ARIA2C_SRC" | awk '{print $NF}')"
[ "$WANT_SHASUM" = "$WANT_OPENSSL" ] \
  || die "资产 sha256 两个工具给出了不同的值（sha256sum=${WANT_SHASUM} openssl=${WANT_OPENSSL}）" 1
WANT="${WANT_SHASUM:0:$HASH_PREFIX_LEN}"
ok "内嵌资产 ${ARIA2C_ASSET} 的 sha256 前 ${HASH_PREFIX_LEN} 位 = ${WANT}（两条工具一致）"

VER="$("$BIN" --version)" || die "产物跑不起来：$BIN --version 退出码非 0" 1
printf '    $ %s --version\n    %s\n' "$BIN_NAME" "$VER"
GOT_HASH="$(printf '%s' "$VER" | sed -n 's/.*内嵌 aria2c \([0-9a-f]\{12\}\).*/\1/p')"
[ -n "$GOT_HASH" ] \
  || die "--version 的输出里抠不出内嵌 aria2c 的 ${HASH_PREFIX_LEN} 位摘要（输出：${VER}）——判据失效，按失败处理" 1
[ "$GOT_HASH" = "$WANT" ] \
  || die "编进去的 aria2c 与 core/assets/$ARIA2C_ASSET 不是同一份：--version 说 ${GOT_HASH}，资产是 ${WANT}" 1
ok "--version 的摘要与资产逐字一致（${GOT_HASH}）"

HELP="$("$BIN" --help)" || die "产物跑不起来：$BIN --help 退出码非 0" 1
printf '%s' "$HELP" | grep -q '^用法：' || die "--help 的输出里没有「用法：」" 1
printf '%s' "$HELP" | grep -q '退出码：' \
  || die "--help 的输出里没有「退出码：」—— 退出码表是给客户的对外契约" 1
ok "--help 可读（$(printf '%s\n' "$HELP" | head -1)）"

# ---------------------------------------------------------------------------
# 3) 产物落位
# ---------------------------------------------------------------------------
# ⚠️ **落位在测试之前**（照任务书的顺序：编译 → 自验 → 取回产物；`cargo test` 是本
#    脚本另加的第四道）。理由：测试红的时候，一份**已经过全部产物自验**的二进制
#    不该跟着消失 —— 否则人得自己去 `target/` 里翻。落位本身不是任何"已通过"的断言，
#    退出码仍然由下面那道测试决定，红了照样非 0，什么都没被静默放过。
log "4/5 产物落位"
cp -f "$BIN" "$OUT/$BIN_NAME-linux-x86_64"
chmod +x "$OUT/$BIN_NAME-linux-x86_64"
DST="$OUT/$BIN_NAME-linux-x86_64"
printf '    路径   ：%s\n    字节数 ：%s\n    sha256 ：%s\n' \
  "$DST" "$(stat -c '%s' "$DST")" "$(sha256sum "$DST" | awk '{print $1}')"

# ---------------------------------------------------------------------------
# 4) cargo test —— **全案唯一能执行 Linux 靶测试的地方**（见文件头）
# ---------------------------------------------------------------------------
log "5/5 cargo test（真 CLI + 真 aria2c + 桩服务；本机是唯一跑得了它的地方）"
TEST_LOG="$(mktemp)"

# ---------------------------------------------------------------------------
# ⚠️ 先清掉本包的 test-profile 产物：**实测撞过，不是预防性写的**
# ---------------------------------------------------------------------------
# cargo 的"要不要重编"靠 **mtime 指纹**。只要源码的 mtime **倒退**到产物之后
# （`rsync -a` 是保 mtime 的、从备份 `cp` 回来的文件也常如此），cargo 就判"没变"、
# 直接复用**上一次**编出来的测试二进制 —— 而它承载的是**旧源码**。
# 本轮的实测现场（构建机上）：
#     tests/cli_e2e.rs  mtime 07:48:42
#     target/debug/deps/cli_e2e-6e6be25603c7211e  mtime 08:08:17   ← 产物比源码**新**
#   ⇒ `cargo test` 复用了它，`cli_e2e` 报 3/4（一条红），
#     而 `touch tests/cli_e2e.rs` 强制重编后**同一条用例立刻变绿**，
#     二进制的 md5 也从 f2b74855… 变成 e46f2147… ⇒ 之前那份**确实是旧的**。
#
# ⚠️ 危险的方向不只是"假红"，还有**假绿**：旧二进制可能是绿的，而新源码是坏的。
#    本脚本第 5 步的全部主张就是"这棵树在 Linux 上跑过测试且是绿的"，
#    所以这里用 `cargo clean -p` 把那条主张的**前提**钉死：产物必须现编。
#
# 🔴 **这一行才是第 5 步的承重判据**（修复轮 3 的 N-1 订正）：
#    后面那条 `verify_test_bins_fresh` 在"复用已经发生"的场景里**恒真**
#    （理由见它的注释）—— 能**防住**复用的只有这里，断言只是形状上的兜底。
#    所以：**不许**以"有那条断言兜着"为由删掉这一行。
#    `--profile test` 只清 test profile（`target/debug`），**不动** release 产物
#    —— 上面刚落位的那份 `$OUT/` 与 `target/.../release/` 都不受影响。
#    代价是这一趟的测试要现编一次（本机实测约 1 分钟）；这是刻意的取舍：
#    一次确定的编译，换掉一条会被 mtime 骗过去的读数。
(cd "$REPO_DIR/core" && "$CARGO" clean -p benagen-core --profile test)

# `cargo test` 用的是**宿主的 gnu target**（不是 musl）：测的是源码语义，
# 与交付形态无关；而它编出来的 `benagen-dl` 内嵌的仍是同一份
# `aria2c-linux-x86_64` 资产（`ARIA2C_ASSET_NAME` 只看 os+arch），所以
# `cli_e2e` 里那条"真起 aria2c"跑的就是交付形态里那个引擎。
set +e
(cd "$REPO_DIR/core" && "$CARGO" test 2>&1 | tee "$TEST_LOG")
TEST_RC="${PIPESTATUS[0]}"
set -e

# 逐靶列出"跑了几个测试目标、各多少条"——这是给报告用的读数，不只是结论。
# ⚠️ 打印**在退出码判定之前**：红了的时候恰恰最需要这份读数（哪几条跑了、哪条红），
#    放在 die 之后就等于"一红就没有摘要"，而那正是要读它的时候。
printf '    各测试目标：\n'
grep -E '^\s+Running |^test result:' "$TEST_LOG" | sed 's|^|      |' || true

# ⚠️ **先判"这份读数算不算数"，再判"绿不绿"**：测试二进制若是旧的，绿红都没有意义。
#    判据与实测现场见 `verify_test_bins_fresh` 上方那段。
verify_test_bins_fresh "$TEST_LOG" "$REPO_DIR/core" \
  || die "跑的不是当前测试源码编出来的测试二进制（上面列了来源与时间）——
     这份测试读数**不算数**，所以本脚本不报成功。
     多数情况是源码 mtime 被倒回到产物之后了（rsync -a / 从备份 cp 都会）：
     本脚本第 5 步开头的 \`cargo clean -p --profile test\` 就是为这个加的。" 1
ok "测试二进制都出自当前测试源码（逐个比过 mtime）"

[ "$TEST_RC" -eq 0 ] || die "cargo test 退出码 ${TEST_RC}（见上面输出）。产物**已落位**（上面那一步），
     但本脚本不认为它"验过" —— Linux 靶的测试是红的。" 1

# ⚠️ 退出码之外还要看**跑了几条**：`cargo test` 在"一个测试都没跑"时也是 0
#    （例如目标被改名、或所有测试都被 `#[cfg]` 掉）。所以正面要求结尾那行
#    `N passed` 里 N > 0，并且**端到端那个靶必须真的跑过** —— 它是全案唯一的
#    Linux 运行时证据，被 `#[cfg]` 掉的话整台机器就白建了。
PASSED="$(grep -oE 'test result: ok\. [0-9]+ passed' "$TEST_LOG" | awk '{s+=$4} END {print s+0}')"
[ "$PASSED" -gt 0 ] \
  || die "cargo test 退出码 0，但一条测试都没跑过（passed=0）—— 空跑绿灯不算证据" 1
grep -q 'Running tests/cli_e2e.rs' "$TEST_LOG" \
  || die "cargo test 的输出里没有 'Running tests/cli_e2e.rs' —— 端到端（真 CLI + 真 aria2c + 桩服务）没有执行。
     它是 Linux 侧唯一的运行时证据，**不许**在没有它的前提下报成功。" 1
ok "cargo test：${PASSED} 条通过，0 条失败；cli_e2e 确实跑过"
rm -f "$TEST_LOG"

log "完成"
printf '    产物已落位：%s\n' "$DST"
printf '\n    取回（在 Mac 上）：\n'
printf '      scp root@%s:%s dist_cli/benagen-dl-linux-x86_64\n' "$(hostname)" "$DST"
printf '\n    还剩一件本脚本**不做**的事（见文件头）：拿真交付码真下载一次 ——\n'
printf '      cd %s && ./out/%s-linux-x86_64 <真交付码> -o %s/dl; echo "退出码=$?"\n' \
  "$BUILD_ROOT" "$BIN_NAME" "$BUILD_ROOT"
