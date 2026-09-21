#!/usr/bin/env bash
#
# check_minos_macos.sh —— **包内每一个二进制的最低系统版本，都必须在声明的下限之内**。
#
#    bash macos/scripts/check_minos_macos.sh <BenagenDownloader.app 路径> <下界，如 13.0>
#
# 退出码（与 `build_app_macos.sh` 的 `verify_arch()` 同一套口径）：
#   0 = 全过
#   1 = **判据没过**（某个二进制超了下限，或 plist 与下限不一致）
#   2 = 用法/环境问题（参数不对、.app 不在、包里缺件、工具读不出）
#
# ---------------------------------------------------------------------------
# 🔴 它为什么存在（这不是"补一条判据"，是补一个**已经出过事故**的洞）
# ---------------------------------------------------------------------------
# 2026-09-20：客户在 **macOS 13.7.8 Intel** 上双击交付的 `.app`，得到
# 「应用程序的这个版本不能与此版本的 macOS 配合使用」。查下来包里三个二进制
# **各带各的下限**，而且**没有一个地方看得见这件事**：
#
#   Contents/MacOS/BenagenDownloader      minos 14.0   ← Package.swift 的 .macOS(.v14)
#   Contents/Resources/benagen-core       minos 11.0   ← rustc 对该 target 的默认值（唯一合格的）
#   Contents/Resources/aria2c-macos-*     minos 26.0   ← 构建机是 macOS 26、而编译时没给版本标志
#
# ⚠️ **`lipo -archs` 对这件事是瞎的**：三个架构都对（x86_64）、名字也对，所以
#    既有那五条判据一条都不红。而"读得出来的那个数"（minos）**没有任何人看**。
#
# ⚠️ **文档里那条补救命令当时也是错的**（`macos/README.md` 的"只能真 Intel 验"那张表）：
#    `MIN_MACOS=13.0 bash build_app_macos.sh x86_64` 只改 Info.plist 里那一格，
#    **不改二进制** —— 壳的 minos 仍然来自 `Package.swift`。⇒ 照它做出来的包，
#    plist 说 13.0、壳说 14.0，而**整条链没有一步会报错**。本脚本就是那一步。
#
# ---------------------------------------------------------------------------
# 判据的形状（三条，各自挡一种）
# ---------------------------------------------------------------------------
#   ① `Info.plist` 的 `LSMinimumSystemVersion` **必须等于**传进来的下限
#      —— 它是 LaunchServices 决定"要不要拒绝启动"的那一格；
#   ② 包内**每一个** Mach-O 的 minOS **必须 ≤ 下限**
#      —— 它是 loader 的硬闸门，比 plist 更底层（plist 放行、它照样拒绝）；
#   ③ **读不出 = 失败**（不是"跳过"）：读不出说明这份字节不是我们以为的那个东西。
#      ⚠️ 这条判据的口径与 `verify_arch()` / `build_dmg_macos.sh` 那段
#         "读不出必须当场判死"**逐字对齐** —— 同一份字节不许出现"这里说行、那里说不行"。
#
# ---------------------------------------------------------------------------
# ⚠️ 两种 Mach-O 形态都要认（少认一种就会对某些二进制"读到空")
# ---------------------------------------------------------------------------
#   · 新形态：`LC_BUILD_VERSION` + `minos 14.0`
#   · 旧形态：`LC_VERSION_MIN_MACOSX` + `version 10.12`
#   实测：`core/target/x86_64-apple-darwin/release/benagen-core` 走的是**旧形态**；
#   同一个 crate 的 arm64 产物走新形态。只 grep `minos` 的实现会在 x86_64 那一行
#   **读到空字符串**，而"空"如果被当成"没有约束"就正好放走一个坏包。
#
# ⚠️ 版本比较必须**数值比较**：`10.12` 用字符串比会 > `13`（"1" > "1" 之后 "0" < "3"，
#    看起来对；但 `9.0` vs `13.0` 字符串比会判 `9.0` 大，方向就错了）。这里拆成
#    major/minor 按 1000 进制比。
set -euo pipefail

die() { echo "check_minos_macos.sh: $*" >&2; exit 2; }

# 版本比较：$1 <= $2 ⇒ 0
version_le() {
  awk -v a="$1" -v b="$2" 'BEGIN{
    n=split(a,x,"."); m=split(b,y,".");
    av=(x[1]+0)*1000+((n>1)?(x[2]+0):0);
    bv=(y[1]+0)*1000+((m>1)?(y[2]+0):0);
    exit (av<=bv)?0:1
  }'
}
# 版本比较：$1 == $2 ⇒ 0
version_eq() { version_le "$1" "$2" && version_le "$2" "$1"; }

# 一个 Mach-O 的 minOS。**读不出就打印空**（调用方必须判死，不许当成"没有约束"）。
minos_of() {
  otool -l "$1" 2>/dev/null | awk '
    /LC_BUILD_VERSION/      { new = 1 }
    new && /^[ \t]*minos /   { print $2; exit }
    /LC_VERSION_MIN_MACOSX/ { old = 1 }
    old && /^[ \t]*version / { print $2; exit }
  ' | tr -d ' '
}

[ "$#" -eq 2 ] || die "用法：bash macos/scripts/check_minos_macos.sh <BenagenDownloader.app> <下界，如 13.0>"
APP="$1"
BOUND="$2"

[ -d "$APP" ] || die "找不到 .app：${APP}"
command -v otool  >/dev/null || die "找不到 otool（装了 Xcode 或 Command Line Tools 才有）"
command -v plutil >/dev/null || die "找不到 plutil"

PLIST="$APP/Contents/Info.plist"
[ -f "$PLIST" ] || die "找不到 ${PLIST}"

echo "==> 判据：包内每个二进制的最低系统版本都必须 ≤ ${BOUND}" >&2
echo "    包：${APP}" >&2

fail=0

# ---- ① Info.plist 那一格 ---------------------------------------------------
declared="$(plutil -extract LSMinimumSystemVersion raw -o - "$PLIST" 2>/dev/null || true)"
if [ -z "$declared" ]; then
  echo "错误：Info.plist 里没有 LSMinimumSystemVersion —— 那一格是 LaunchServices 判「能不能启动」的依据。" >&2
  exit 2
fi
if version_eq "$declared" "$BOUND"; then
  printf '    \033[32m✓\033[0m %-24s minOS %s（= 下限）\n' "Info.plist" "$declared"
else
  printf '    \033[31m✗\033[0m %-24s minOS %s（**不等于**下限 %s）\n' "Info.plist" "$declared" "$BOUND"
  echo "      ⇒ plist 与下限不一致。它的来源是 build_app_macos.sh 的 MIN_MACOS。" >&2
  fail=1
fi

# ---- ② 包内三个二进制 ------------------------------------------------------
# 壳的名字从 plist 读（`CFBundleExecutable`）—— 写死一个名字的话，改名那天这条判据会
# 静默地判一个不存在的东西（下面 [ -f ] 会红，但报出来的是"缺文件"而不是"名字改了"）。
exe_name="$(plutil -extract CFBundleExecutable raw -o - "$PLIST" 2>/dev/null || true)"
[ -n "$exe_name" ] || die "Info.plist 里没有 CFBundleExecutable —— 不知道该验哪一个壳"

# aria2c 用 glob：名字带架构后缀，**命中数必须恰好 1**（0 个 = 包不完整；
# 多个 = 这个包混了架构，那不归本脚本判，但**不许在这里静默挑一个**）。
# 形态照抄 `build_dmg_macos.sh` 那一段。
set +e
aria2c_hits="$(ls "$APP/Contents/Resources/"aria2c-macos-* 2>/dev/null)"
set -e
aria2c_count="$(printf '%s' "$aria2c_hits" | grep -c . || true)"
[ "$aria2c_count" = "1" ] || {
  echo "错误：包里的 aria2c-macos-* 命中 ${aria2c_count} 个（必须恰好 1 个）。" >&2
  echo "      0 个 = 包不完整；多个 = 混架构。先重建：bash macos/scripts/build_app_macos.sh [arm64|x86_64]" >&2
  exit 2
}

for f in "$APP/Contents/MacOS/$exe_name" "$APP/Contents/Resources/benagen-core" "$aria2c_hits"; do
  name="$(basename "$f")"
  [ -f "$f" ] || {
    echo "错误：包内缺 ${name}（${f}）—— 这个 .app 不完整。" >&2
    echo "      先重建：bash macos/scripts/build_app_macos.sh [arm64|x86_64]" >&2
    exit 2
  }
  got="$(minos_of "$f")"
  # ⚠️ **读不出必须当场判死**：空值若被当成"没有约束"，这条判据就正好放走
  #    那个它本该拦住的包（同 `verify_arch` 与 `build_dmg_macos.sh` 的口径）。
  if [ -z "$got" ]; then
    echo "错误：${name} 的 minOS 读不出（既没有 LC_BUILD_VERSION/minos，也没有 LC_VERSION_MIN_MACOSX/version）。" >&2
    echo "      它不是我们以为的那种 Mach-O，或者这份字节被换过 —— 不能用它下'可以在旧系统上跑'的结论。" >&2
    exit 2
  fi
  if version_le "$got" "$BOUND"; then
    printf '    \033[32m✓\033[0m %-24s minOS %s\n' "$name" "$got"
  else
    printf '    \033[31m✗\033[0m %-24s minOS %s（**超了**下限 %s）\n' "$name" "$got" "$BOUND"
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "" >&2
  echo "判据没过：上面打了 ✗ 的那些，装到「下限」那一档系统上会**直接被拒绝**——" >&2
  echo "   · 壳超了   ⇒ 改 macos/Package.swift 的 platforms（部署目标唯一的来源）；" >&2
  echo "   · aria2c 超 ⇒ 用 -mmacosx-version-min 重编：bash downloader/scripts/build_aria2_macos.sh <arch>；" >&2
  echo "   · plist    ⇒ build_app_macos.sh 的 MIN_MACOS 与 Package.swift 必须同值。" >&2
  echo "  ⚠️ 只改 MIN_MACOS **没用**：它只写 plist，不改二进制（2026-09-20 的事故就是这么来的）。" >&2
  exit 1
fi

echo "==> 判据通过：包内每一个二进制与 Info.plist 都在 ${BOUND} 之内。" >&2
