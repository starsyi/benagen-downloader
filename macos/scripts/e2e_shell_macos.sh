#!/usr/bin/env bash
# 端到端验收：把「真内核 + 真 aria2c + 本地桩服务的 e2e 测试」「打包」「启动取证」
# 串成一条命令。
#
#     bash macos/scripts/e2e_shell_macos.sh                # arm64（默认，与以前逐字相同）
#     bash macos/scripts/e2e_shell_macos.sh x86_64         # Intel（先读下面那段 ⚠️）
#
# 它回答的问题是"**这个客户端真的能交付吗**"，三件事缺一不可：
#   1. **链路通**：真 `delivery_manifest.py` 产清单 → 真 `benagen-core`（内嵌真 aria2c）
#      从 `python3 -m http.server` 桩上下完一批文件 → 落盘逐字一致 → 校验六类 → 状态文件；
#      全程由**壳自己的 `CoreClient`** 驱动（`EndToEndTests.swift`，**无条件执行**）。
#   2. **产物成**：`dist/BenagenDownloader.app` 里壳、内核、aria2c、GPL 全文四件齐。
#   3. **能起窗口**：`open` 之后应用真的接入了 WindowServer（两路取证）。
#
# ⚠️ 为什么内核必须在跑测试**之前**构建：那条 e2e 是**无条件**执行的
#    （不是 `.enabled(if: CoreClient.devCoreBinaryExists())`）。内核不在时它会**红**，
#    而那是**脚本该报的错**——"内核没构建"不该被伪装成一条静默跳过的测试。
#    已有产物时 `cargo build --release` 是秒级 no-op。
#
# ⚠️ 为什么用 `macos/scripts/test.sh` 而不是裸 `swift test`：本机宏插件路径问题会让
#    裸调约一半概率随机红（与代码无关），而且 `--filter` 打错字会以 0 退出、
#    打印 "No matching test cases were run" ——**空跑绿灯**。脚本把这两种情况都判成失败。
#
# ⚠️ **边界（如实标注）**：第 3 步的启动取证会真的 `open` 一个 `.app`，
#    而壳会照内核的默认值把下载目录设为 `$HOME/Downloads/Benagen` ——
#    那是**真实交付数据所在的目录**，要知道它确实碰了那里。
#
#    ⚠️ **而且它不只是"起窗口"**：起真壳会**自动加载内核记住的交付码**。
#       `App.swift` 的 `.task` 里是 `await model.start()` **紧接着**
#       `await model.loadRememberedDelivery()`，而内核把 `last_code` 落盘
#       （`core/src/main.rs` 的 `remember_last_code`，落在
#       `~/Library/Application Support/BenagenDownloader/last_code`）——
#       只要这台机器上**成功加载过一次**交付码，此后**每一次**起真壳都会真的去拉一次
#       清单、并执行一次 `state.save()`（原子写，不会损坏状态文件，但确实发生）。
#       这一步仍然**不点任何按钮**。
#
#    想要一次**无副作用**的启动取证，可以让壳看不到那份状态（可选项）：
#        HOME="$(mktemp -d)" "$APP/Contents/MacOS/BenagenDownloader" &
#        # $APP = macos/dist/BenagenDownloader.app
#    直接起可执行文件能继承环境变量（`settings.json` 与 `last_code` 都在 `$HOME` 下，
#    下载目录同理）；`open` 走 LaunchServices，**传不进** `HOME`。
#    ⚠️ **别**因此把取证手段换成它 —— `open` 才是"双击能不能开"的判据。
#
#    **这不是本脚本引入的**：`verify_app_launch_macos.sh`
#    从任务 4 起就是这么跑的，它是"包能不能起"的唯一取证手段。
#    与本脚本第 1 步的 e2e 测试**无关**：那条测试全程在临时目录里跑
#    （`CoreClient.live(settingsPath:downloadDir:)` 两个目录都指到临时目录），
#    碰不到 `~/Downloads/Benagen`。
#
# ---------------------------------------------------------------------------
# ⚠️ x86_64（Intel）那条路：**本机跑通 ≠ 在真 Intel 上验过**（F-2 的诚实边界）
# ---------------------------------------------------------------------------
# 本机是 Apple Silicon，所有 x86_64 运行都经 **Rosetta 翻译**。所以这一趟能证明的是
# "x86_64 的字节在本机能被加载、能起子进程、能跑完整条链路"，**不能**证明：
#   - 真 Intel 机器上的 HTTPS 下载链路（SecureTransport 握手路径不同）；
#   - Gatekeeper / quarantine 行为；
#   - 下限声明在旧系统上的**实际行为**（2026-09-20 起下限是 **13.0**：
#     声明的下限本机能静态读字节验，见 `check_minos_macos.sh`；但
#     "13 上真的起得来、aria2c 真的执行"只有真机能验——弱链接符号、
#     AppleTLS 路径、TCC 授权都不是同一条实现）。
# 完整清单见 macos/README.md 的「出 Intel 包 → 只能真 Intel 验的四件事」
# 与「只能真 macOS 13 验的」两节。
#
# 两个实现要点（都是为了**不许静默退化**，F-2）：
#   1. 内核用 `cargo build --release --target x86_64-apple-darwin` 编，
#      产物路径是 `core/target/x86_64-apple-darwin/release/benagen-core`
#      （**不是** `core/target/release/`），编完还要过一遍 `lipo -archs`。
#   2. 端到端测试用 `BENAGEN_CORE=<x86_64 内核>` 驱动（见第 1 步末尾那段）——
#      不设它的话，Swift 测试会退回第三条候选（本机架构的内核），
#      于是"Intel 的端到端"变成"arm64 的端到端"：一样全绿，却一个 x86_64 字节都没跑到。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"      # macos/
REPO="$(cd "$ROOT/.." && pwd)"                                # 仓库根

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '    \033[32m✓\033[0m %s\n' "$*"; }
fail() { printf '\n!! %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 架构（阶段 F 任务 2）：默认 arm64，**不带参数时的行为与以前逐字相同**
# ---------------------------------------------------------------------------
# 为什么这条脚本也必须接受架构：它**自己也编内核**（第 1 步）。不同步的话，
# `--arch x86_64` 会得到一个「壳是 Intel、内核是 arm64」的包 —— 而它不会报任何错，
# 直到在真 Intel 机器上点了没反应。所以每一步都按 $ARCH 走，并且**自己验产物架构**。
ARCH="${1:-${BENAGEN_ARCH:-arm64}}"
case "$ARCH" in
  arm64)
    CORE_TARGET=""                                     # 空 = 原生构建，不带 --target
    CORE_BIN="$REPO/core/target/release/benagen-core"
    ;;
  x86_64)
    CORE_TARGET="x86_64-apple-darwin"
    CORE_BIN="$REPO/core/target/x86_64-apple-darwin/release/benagen-core"
    ;;
  *)
    fail "不认识的架构 '$ARCH'（只支持 arm64 / x86_64）"
    ;;
esac
CORE_TARGET_DESC="${CORE_TARGET:+ --target $CORE_TARGET}"
# 包内引擎的文件名跟着架构走（阶段 F 任务 1：两个架构各入库一份）。
ARIA2C_REL="Contents/Resources/aria2c-macos-$ARCH"

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

# ---------------------------------------------------------------------------
# 0) 预检：缺什么就明说，别让它变成后面一句看不懂的报错
# ---------------------------------------------------------------------------
log "预检（目标架构 ${ARCH}）"
for tool in python3 cargo swift; do
  command -v "$tool" >/dev/null 2>&1 || fail "PATH 里没有 $tool"
done
ok "python3 = $(command -v python3)"
ok "cargo   = $(command -v cargo) ($(cargo --version | head -1))"
# e2e 的桩夹具由**仓库根的真实生成器**产出（行为契约 §3.1），少了它整条链就跑不起来
[ -f "$REPO/delivery_manifest.py" ] \
  || fail "找不到 $REPO/delivery_manifest.py —— 夹具必须由真实生成器产出，不能手写清单"
ok "夹具生成器 = $REPO/delivery_manifest.py"

# x86_64 的工具链预检，**在编译之前**（F-2）：拿不到 x86_64 的 std 就不可能产出
# Intel 包。预检脚本会把确切的补救配方整段打出来；本脚本不替它降级
# （不装工具链、不改 PATH、更不会退回去用 arm64 跑完这一趟）。
if [ -n "$CORE_TARGET" ]; then
  log "x86_64 工具链预检（core/scripts/preflight_x86_64_toolchain.sh）"
  [ -f "$REPO/core/scripts/preflight_x86_64_toolchain.sh" ] \
    || fail "找不到 $REPO/core/scripts/preflight_x86_64_toolchain.sh —— 它是 x86_64 构建路径的前置"
  bash "$REPO/core/scripts/preflight_x86_64_toolchain.sh" \
    || fail "x86_64 工具链预检没过（配方见上）。**不会**退回去用 arm64 跑完这一趟（F-2）"

  # ---------------------------------------------------------------------------
  # Rosetta 2：**运行**前置 —— 这条脚本要真跑 x86_64 内核，所以**硬拦**（预检那边只警告）
  # ---------------------------------------------------------------------------
  # 编译 x86_64 不需要 Rosetta，所以 `build_app_macos.sh x86_64` 那条路只警告不拦；
  # 而这里要 posix_spawn 一个 x86_64 内核并让它起 aria2c —— 没有 Rosetta 就是做不到。
  # 不拦的话失败点会推迟到第 4 步的"启动取证"，报错形态与 Rosetta 毫无关系，
  # 人根本猜不到要装什么（F-2 要的"可执行的补救"就是这么丢的）。
  # 判据用最小可执行探针 `arch -x86_64 /usr/bin/true`，不看任何间接证据。
  # ⚠️ 那句"已通过"**必须**留在 `[ 宿主 = arm64 ]` 分支**里面**：在宿主本来就是 x86_64 的
  #    机器上（**人类伙伴在真 Intel 上跑 `e2e_shell_macos.sh x86_64`** —— 最自然的那条路），
  #    探针**根本没跑**，而在分支外无条件打印 `✓ Rosetta 2 可用` 就是一句**没有证据支撑的
  #    "已通过"**：恰恰是本项目最在意的那种形态（无证据的绿灯）。
  if [ "$(uname -m)" = "arm64" ]; then
    if ! /usr/bin/arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
      fail "本机（Apple Silicon）没装 Rosetta 2 —— x86_64 的端到端**跑不起来**（编译可以、执行不行）。
      装它（一次性，需要联网，约 1 分钟）：softwareupdate --install-rosetta --agree-to-license
      （这条不是"跳过某一步"能绕开的：本机执行任何 x86_64 二进制都要经 Rosetta 翻译。）"
    fi
    ok "Rosetta 2 可用（arch -x86_64 /usr/bin/true 通过）—— 本机是 Apple Silicon，x86_64 内核经它翻译执行"
  else
    # 真 Intel 机器走这里：x86_64 是**原生**执行的，Rosetta 不参与、也无需参与。
    # 只说这一句为真的话（不说"探针通过"，因为它没跑，跑了也没意义）。
    ok "宿主就是 $(uname -m)：x86_64 原生执行，不需要 Rosetta（探针未跑，也无需跑）"
  fi
fi

# ---------------------------------------------------------------------------
# 1) 内核（必须在测试之前，见文件头）
# ---------------------------------------------------------------------------
log "构建内核（cargo build --release${CORE_TARGET_DESC}；已有产物时是秒级 no-op）"
if [ -n "$CORE_TARGET" ]; then
  (cd "$REPO/core" && cargo build --release --target "$CORE_TARGET")
else
  (cd "$REPO/core" && cargo build --release)
fi
[ -x "$CORE_BIN" ] || fail "cargo build --release$CORE_TARGET_DESC 之后仍找不到可执行文件 $CORE_BIN"
ok "内核 = $CORE_BIN ($(du -h "$CORE_BIN" | cut -f1))"

# 内核架构自验（F-1）：**本脚本自己编出来的那份也要自己验**，不看参数。
# 理由与 build_app_macos.sh 的 verify_arch 同款：`arch -x86_64 clang` 仍会编出 arm64，
# "传了 --target" 本身证明不了任何事。
kernel_arch="$(lipo -archs "$CORE_BIN" 2>/dev/null || true)"
[ "$kernel_arch" = "$ARCH" ] \
  || fail "内核产物架构不对：期望 ${ARCH}，实际「${kernel_arch:-lipo 读不出}」（${CORE_BIN}）—— 继续下去就是拿错架构的内核跑测试，不许静默继续"
ok "内核架构自验：lipo -archs = $kernel_arch"

# ⚠️ 壳找内核的第一条候选就是环境变量 `BENAGEN_CORE`（`CoreClient.locateCoreBinary()` 顺序①）。
#    这里**必须**把它设上：不设的话 Swift 测试走第三条候选
#    （`core/target/release/benagen-core`，那是**本机架构**的内核）——于是"跑 x86_64 的端到端"
#    会**静默地**变成"跑 arm64 的端到端"：一样全绿，却一个 x86_64 字节都没执行到。
#    那正是 F-2 要根除的形态（"退化"比"失败"危险，因为它看起来成功）。
#    arm64 那支不设：那条路与以前逐字相同（F-4）。
if [ -n "$CORE_TARGET" ]; then
  export BENAGEN_CORE="$CORE_BIN"
  # ⚠️ 同一类"没有证据的话"在这里也要去掉（复审 Minor 1 的姊妹）：**在真 Intel 机器上**
  #    （宿主就是 x86_64）这里是**原生**执行，说"经 Rosetta 执行"是假的。
  if [ "$(uname -m)" = "arm64" ]; then
    log "端到端测试将驱动 x86_64 内核（BENAGEN_CORE=${CORE_BIN}，在本机经 Rosetta 翻译执行）"
  else
    log "端到端测试将驱动 x86_64 内核（BENAGEN_CORE=${CORE_BIN}，本机就是 $(uname -m)，原生执行）"
  fi
fi

# ---------------------------------------------------------------------------
# 2) 端到端测试（真内核 + 真 aria2c + 桩服务）
# ---------------------------------------------------------------------------
log "端到端测试：真内核 + 真 aria2c + 本地桩服务（bash macos/scripts/test.sh --filter EndToEnd）"
set +e
bash "$ROOT/scripts/test.sh" --filter EndToEnd 2>&1 | tee "$LOG"
status="${PIPESTATUS[0]}"
set -e
[ "$status" -eq 0 ] || fail "端到端测试失败（退出码 ${status}），见上面的输出"

# ⚠️ 二次守卫：退出码 0 **还不够**。"过滤器拼错"或"用例被改名"都可能让这一轮
#    一个用例都没跑（前者的空跑绿灯已由 test.sh 拦下，后者没有）——
#    这里正面要求那行 `passed` 出现在输出里。
grep -q "Test endToEndThroughTheShellsOwnClient() passed" "$LOG" \
  || fail "输出里没有 \"Test endToEndThroughTheShellsOwnClient() passed\" —— 端到端测试没有真的跑过（空跑不算证据）"

echo
echo "    端到端测试的观测值（测试自己打印的，逐行）："
grep -F '[e2e]' "$LOG" | sed 's|^|      |' || true
ok "端到端链路通过（清单 → 入队 → aria2 落盘 → 回读路径逐字一致 → 校验 → 状态文件 → 收尾无孤儿）"

# ---------------------------------------------------------------------------
# 3) 打包
# ---------------------------------------------------------------------------
log "打包 .app（bash macos/scripts/build_app_macos.sh ${ARCH}）"
bash "$ROOT/scripts/build_app_macos.sh" "$ARCH"
APP="$ROOT/dist/BenagenDownloader.app"
[ -d "$APP" ] || fail "打包脚本跑完却没有 $APP"
# 四件缺一：壳本身、壳的查找顺序②（包内内核）、内嵌引擎、GPL 分发义务的载体
for f in Contents/MacOS/BenagenDownloader \
         Contents/Resources/benagen-core \
         "$ARIA2C_REL" \
         Contents/Resources/COPYING-GPLv2.txt; do
  [ -f "$APP/$f" ] || fail "包内缺 $f"
  ok "$f ($(du -h "$APP/$f" | cut -f1))"
done
# 包内三个可执行文件的架构自验（F-1：每条构建路径都自己验产物）。
# build_app_macos.sh 里已经验过一次；这里**再验一次**是有意的复制——本脚本是
# "验收"脚本，它的判据不该是"上游脚本说它验过了"。
for f in Contents/MacOS/BenagenDownloader \
         Contents/Resources/benagen-core \
         "$ARIA2C_REL"; do
  got="$(lipo -archs "$APP/$f" 2>/dev/null || true)"
  [ "$got" = "$ARCH" ] || fail "包内 $f 的架构是「${got:-lipo 读不出}」，期望 $ARCH"
  ok "架构自验：$f → $got"
done

# ---------------------------------------------------------------------------
# 4) 启动取证（两路：lsappinfo 的 Foreground + CoreGraphics 的真窗口）
# ---------------------------------------------------------------------------
log "启动取证（bash macos/scripts/verify_app_launch_macos.sh）"
bash "$ROOT/scripts/verify_app_launch_macos.sh" "$APP"
ok "窗口取证通过（收尾在取证脚本的 EXIT 陷阱里：壳 + 内核都收掉了）"

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------
cat <<'SUMMARY'

==> 端到端验收通过

    自动化到这里为止。**界面上的每一条判据都得用眼睛过一遍**——
    清单在 `macos/README.md`（16 条，含 8b / 11b–11e 四条子项），
    每条都写了"怎么看算通过"，以及哪几条是**已知缺口**（第 16 条）
    或**尚未被人眼验过**（第 11、11b、11e 条）。

    完整测试套件是另一条命令：bash macos/scripts/test.sh
SUMMARY
