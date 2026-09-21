#!/usr/bin/env bash
# 构建 macOS 客户端的**分发形态**：BenagenDownloader.app（SwiftUI 壳 + Rust 内核）
#
# 为什么必须有这个脚本
#   `swift build` 只产出 `.build/<triple>/release/BenagenDownloader` 这一个可执行文件。
#   而壳启动时要找的三条内核路径里有一条是**包内**（`CoreClient.locateCoreBinary()` 的
#   查找顺序②：`Contents/Resources/benagen-core`）—— 没有这个脚本，那条路径永远不存在，
#   客户拿到的是一个起不来引擎的壳。把"壳 + 内核 + aria2c"装进一个 .app 就是它存在的全部理由。
#
# 分发形态
#     dist/BenagenDownloader.app/Contents/MacOS/BenagenDownloader       ← SwiftUI 壳
#     dist/BenagenDownloader.app/Contents/Resources/benagen-core        ← Rust 内核（查找顺序②）
#     dist/BenagenDownloader.app/Contents/Resources/aria2c-macos-<架构> ← 下载引擎
#     dist/BenagenDownloader.app/Contents/Resources/COPYING-GPLv2.txt  ← GPL 分发义务
#     dist/BenagenDownloader.app/Contents/Resources/AppIcon.icns       ← 应用图标（CFBundleIconFile）
#     dist/BenagenDownloader.app/Contents/Resources/benagen-mark.png   ← 空态页顶部的图形标
#     dist/BenagenDownloader.app/Contents/Resources/benagen-full-logo.png ← 关于窗口的全称 logo
#     dist/BenagenDownloader.app/Contents/Info.plist                   ← bundle 元数据
#
# ⚠️ 上面三个品牌资产是**入库的静态产物**（来源 `macos/Resources/`），本脚本**不生成**它们：
#    生成那条链要 Pillow，而构建机不该依赖 Python 图像库（换品牌资产时手动跑一次
#    `python3 macos/scripts/make_brand_assets.py`，把产物连同源图一起提交）。
#    它们的**文件名与壳里的常量逐字对应**（`BrandAssets`），改名字要两边一起改 ——
#    对不上的表现是"应用没有图标、空态页没有标"，而这两件**都不报错**。
#   dist/ 是可再生产物，已加进根 .gitignore，不入库。
#
# ⚠️ `Contents/Resources/benagen-core` 这个**位置与文件名**是壳的查找顺序②，
#    改名字或挪地方等于让包内查找永远落空 —— 而因为顺序③（仓库内 core/target/release/）
#    在开发机上恰好存在，`swift run` 下照样能跑，只有打包后的真机启动才会暴露。
#    启动取证脚本的 `pgrep -fl benagen-core` 那一步就是专门抓这个的，别跳过。
#
# 已知边界（如实标注，别当成没这回事）
#   1. **本阶段不做代码签名、不做公证**（与 Go 版同一决定）。cargo/swift 在 darwin/arm64 上
#      会自动打 ad-hoc（linker-signed）签名，脚本末尾用 codesign 打印实际状态。
#   2. **未带 com.apple.quarantine 的产物可直接运行**（本机构建出来的产物就是这种，
#      脚本末尾会打印实际的 xattr）；带该属性的（例如经浏览器下载后解压出来的）会被
#      Gatekeeper 拦一次 ⇒ 放行流程**按系统版本分**，详见 `build_dmg_macos.sh` 文件头
#      与 `macos/README.md` §0b 第 3 条。
#      ⚠️ **旧文档那句"右键 → 打开"从 macOS 15 Sequoia 起已失效**（Apple 移除了那条路）：
#      15 及以上要先双击一次 → 系统设置 →「隐私与安全性」→「仍要打开」（该按钮只在
#      尝试打开后约 1 小时内可用）。两个架构这一步**完全一样**（ad-hoc 不是 Developer ID，
#      Gatekeeper 只认 Developer ID + 公证）。
#   3. **完整的"客户从浏览器下载 → 首次打开点穿弹窗"链路未实测**（取证环境无点击权限）。
#      脚本只验证到"产物能被 open 起窗口"。
#   4. **两个架构各出一份包，不做通用二进制**（人类伙伴的决定，见阶段 F 计划）。
#      `arm64`（默认）与 `x86_64`（Intel）各出各的：内核按 `#[cfg(target_arch)]`
#      内嵌**自己架构**的 aria2c（阶段 F 任务 1），所以 Intel 包里三个二进制都是 x86_64。
#   5. **本机跑 x86_64 ≠ 在真 Intel 上验过**：所有 x86_64 运行都经 Rosetta 翻译。
#      真实 HTTPS 下载链路、Gatekeeper、SecureTransport 握手，**只能真 Intel 验**
#      （见 macos/README.md）。
#   6. **下限声明（`minos`）本脚本会自己验**：组装完调 `scripts/check_minos_macos.sh`，
#      包内任何一个二进制超了下限就**不出包**。当前下限是 **13.0**（见 `MIN_MACOS`）。
#      ⚠️ 静态读字节只能证明"声明的下限"，**证明不了 13 上真的跑得起来** ——
#      那一轮只有真机能做，清单见 macos/README.md 的《只能真 macOS 13 验的》。
#
# 产物架构自验（F-1）
#   本脚本**不听参数，只看字节**：组装好之后把包里那三个二进制逐个 `lipo -archs`，
#   任何一个不等于本次目标架构就**大声失败、包不做出来**。理由（探路实测）：
#   `arch -x86_64 clang` **仍然编出 arm64** —— "参数传对了"与"产出一个名叫 x86_64 的
#   arm64 二进制"完全可以同时成立。详情见下面的 `verify_arch`。
#
# 用法：
#   bash macos/scripts/build_app_macos.sh                 # arm64（默认，调用形态与以前逐字相同）
#   bash macos/scripts/build_app_macos.sh x86_64           # Intel
#   BENAGEN_ARCH=x86_64 bash macos/scripts/build_app_macos.sh
#   VERSION=0.2.0 bash macos/scripts/build_app_macos.sh   # 覆盖版本号
#
# ⚠️ x86_64 那条路要求 `cargo` 解析到 **rustup 那份**（`$HOME/.cargo/bin/cargo`）：
#       export PATH="$HOME/.cargo/bin:$PATH"
#    漏了这句的表现是预检**在编译之前**就失败并打印配方（不会编到一半才炸）。
#    arm64 那条路不受影响：本机默认 cargo 仍是 Homebrew 那份（F-4）。
set -euo pipefail

ARCH="${1:-${BENAGEN_ARCH:-arm64}}"
VERSION="${VERSION:-0.1.0}"
MIN_MACOS="${MIN_MACOS:-13.0}"
BUNDLE_ID="com.benagen.downloader"
BIN_NAME="BenagenDownloader"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"      # macos/
REPO="$(cd "$ROOT/.." && pwd)"                                # 仓库根
DIST="$ROOT/dist"
APP="$DIST/${BIN_NAME}.app"

# ---- 部署目标的**两个来源必须同值**（2026-09-20 补的判据）--------------------
#
# 同一个"最低系统版本"这件事在这里有**两个发出点**，它们管的不是同一件事：
#   · `Package.swift` 的 `platforms: [.macOS(.vN)]` —— 决定**二进制**的 minos，
#     也就是 loader 手里那道**真正的硬闸门**；
#   · 上面那个 `MIN_MACOS` —— **只**决定 `Info.plist` 的 `LSMinimumSystemVersion`，
#     那是 LaunchServices 决定"要不要拒绝启动"的那一格。
#
# ⚠️ 这两处**曾经只靠一句口头约定**（阶段 B 计划里的"必须一致"），没有任何判据。
#    2026-09-20 的事故正是从这个缝里出去的：文档给的补救命令是
#    `MIN_MACOS=13.0 bash build_app_macos.sh`，而它**只改 plist、不改二进制** ——
#    照它做出来的包，plist 说 13.0、壳说 14.0，**整条链没有一步会报错**。
#    ⇒ 当场比一次：不一致就停在这里，别等它装到旧机器上才说话。
PKG_PLATFORM="$(grep -oE '\.macOS\(\.v[0-9]+\)' "$ROOT/Package.swift" | head -1 | grep -oE '[0-9]+' || true)"
if [ -z "$PKG_PLATFORM" ]; then
  echo "错误：读不出 Package.swift 里的 platforms 声明（.macOS(.vN)）—— 部署目标没有第二个来源可比。" >&2
  echo "      它不是可以省略的：二进制的 minos 就来自那里，而 MIN_MACOS 只管 plist。" >&2
  exit 2
fi
if [ "${MIN_MACOS%%.*}" != "$PKG_PLATFORM" ]; then
  echo "错误：部署目标的两个来源不一致。" >&2
  echo "      Package.swift 的 platforms = .macOS(.v${PKG_PLATFORM})（决定二进制的 minos）" >&2
  echo "      MIN_MACOS                     = ${MIN_MACOS}（只决定 Info.plist 那一格）" >&2
  echo "      两者必须同值。⚠️ 只改 MIN_MACOS **不会**改二进制 —— 那正是 2026-09-20 事故的形态。" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# 架构 → 三件事：内核编到哪、内嵌哪一份 aria2c、SwiftPM 要不要带 --arch
# ---------------------------------------------------------------------------
# ⚠️ **arm64 那一支刻意保持"什么都不加"**：`cargo build --release`（不带 --target）、
#    `swift build -c release`（不带 --arch）—— 与阶段 F 之前的调用形态**逐字相同**。
#    本阶段对这条路的判据是"一个字不许坏"（F-4），所以宁可不统一写法，也不动它。
case "$ARCH" in
  arm64)
    CORE_TARGET=""                                    # 空 = 原生构建，不带 --target
    CORE_BIN="$REPO/core/target/release/benagen-core"
    SWIFT_ARCH_ARGS=()
    ;;
  x86_64)
    CORE_TARGET="x86_64-apple-darwin"
    CORE_BIN="$REPO/core/target/x86_64-apple-darwin/release/benagen-core"
    SWIFT_ARCH_ARGS=(--arch x86_64)
    ;;
  *)
    echo "错误：不认识的架构 '$ARCH'（只支持 arm64 / x86_64）。" >&2
    echo "  arm64  = Apple Silicon（默认，不带参数即是它）" >&2
    echo "  x86_64 = Intel" >&2
    echo "本阶段是「两个独立包」、不做通用二进制；要出 Intel 包见 macos/README.md 的「出 Intel 包」一节。" >&2
    exit 2
    ;;
esac

# 内嵌的 aria2c 资产名**跟着架构走**（阶段 F 任务 1：两个架构各入库一份）。
# arm64 那支算出来仍是 `aria2c-macos-arm64` —— 包内文件名与以前逐字相同。
ARIA2C_NAME="aria2c-macos-$ARCH"
# 宿主架构：只用于**把失败话术说准**（见 loud_arch_failure 里那段双向说明），
# 不参与任何判定 —— 判定永远是"产物字节 vs 目标架构"。
HOST_ARCH="$(uname -m)"
CORE_TARGET_DESC="${CORE_TARGET:+ --target $CORE_TARGET}"
SWIFT_ARCH_DESC="${SWIFT_ARCH_ARGS[@]+ ${SWIFT_ARCH_ARGS[*]}}"

ARIA2C_SRC="$REPO/core/assets/$ARIA2C_NAME"
GPL_SRC="$REPO/core/assets/COPYING-GPLv2.txt"
# 品牌资产：入库的静态产物（见文件顶部那段）。少了它们**构不成一个像样的包**，
# 但壳不会因此报任何错 —— 所以它们在下面有一道专门的存在性自查。
BRAND_DIR="$ROOT/Resources"

# ---------------------------------------------------------------------------
# 架构自验（F-1）：**不听参数，只看字节**
# ---------------------------------------------------------------------------
# 为什么必须有：探路实测 `arch -x86_64 clang` **仍然编出 arm64**。也就是说
# "参数传对了"与"产出一个名叫 x86_64 的 arm64 二进制"完全可以同时成立 ——
# 只检查"我传了 --target/--arch"是**验不出任何东西**的。
#
# 判据是 `lipo -archs` 与目标架构**恰好相等**，它同时挡住两种缺陷：
#   - 编成了别的架构（如 x86_64 目标、arm64 产物）；
#   - 编成了通用二进制（报 `x86_64 arm64`，与单架构不相等）。本阶段的决定是
#     "两个独立包、不做通用二进制"，所以包里混进另一个架构同样是缺陷，一并拒。
#
# `loud_arch_failure` 只负责"大声失败 + 说出补救方向"（F-2）：
# **没有**"降级继续"这条分支——继续下去得到的是一个在目标机器上跑不起来的包，
# 而它看起来完全正常。
loud_arch_failure() {  # $1 = 路径，$2 = 期望架构，$3 = lipo 读到的实际值
  echo "" >&2
  echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" >&2
  echo "!! 产物架构不对 —— 包没有做出来（dist/ 里没有留下半成品）" >&2
  echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" >&2
  echo "  文件：$1" >&2
  echo "  期望：$2" >&2
  echo "  实际：$3" >&2
  echo "  本机：${HOST_ARCH}（宿主）    目标：$2" >&2
  echo "" >&2
  echo "  这条检查是**故意不信参数**的：实测 arch -x86_64 clang 仍会编出 arm64，" >&2
  echo "  于是「参数传对了」可以和「拿到一个名叫 x86_64 的 arm64 二进制」同时成立。" >&2
  echo "" >&2
  # ⚠️ 话术**必须双向**（复审两轮各指出一半）：原来只假设"宿主 arm64、目标 x86_64"，
  #    在真 Intel 上**不带参数**跑（默认 arm64）时方向是反的；补了那一半之后，
  #    **宿主 == 目标**那半又空了 —— 而"宿主 arm64、目标 arm64、无参数"（默认调用）
  #    真出架构不符时，读者既拿不到"宿主≠目标"的提示，也看不到最可能的那条成因。
  if [ "$HOST_ARCH" != "$ARCH" ]; then
    echo "  ⚠️ 宿主与目标**不一致**（${HOST_ARCH} → ${ARCH}）—— 这正是本检查存在的前提：" >&2
    echo "     两者不同时，「编出来了」与「编对了」是两件事。" >&2
  else
    echo "  ⚠️ 宿主与目标**一致**（都是 ${ARCH}）：所以问题**不在交叉编译**，" >&2
    echo "     而在「有一个产物没被重新生成」—— 见下面第一条。" >&2
  fi
  echo "" >&2
  # ⚠️ 这一条**两边都要能看到**（复审指出它只留在 x86_64 那半是不对的）：
  #    "共用产物目录 + SwiftPM 没重建"与宿主是不是 Intel **无关**，
  #    而且在默认调用（宿主 arm64 / 目标 arm64）下它恰恰是**最可能**的那一条。
  echo "  **先查这一条（不分架构，两条路都可能撞上）**：" >&2
  echo "    - 壳（Contents/MacOS/${BIN_NAME}）：macos/.build/out/Products/Release 是" >&2
  echo "      **两个架构共用**的路径（实测：带与不带 --arch 报的是同一个目录），切换架构时" >&2
  echo "      SwiftPM 会重建；**万一它没重建，拿到的就是上一次那个架构的产物**。" >&2
  echo "      修：把那个产物删掉再重跑（文件没了 SwiftPM 必然重新链接）：" >&2
  echo "        rm -f macos/.build/out/Products/Release/${BIN_NAME}" >&2
  echo "        bash macos/scripts/build_app_macos.sh $ARCH" >&2
  echo "      还不行就退一步：rm -rf macos/.build（代价是一次完整的壳重建）。" >&2
  echo "" >&2
  echo "  其余的排查方向按**目标**架构分，别照抄不对的那半：" >&2
  if [ "$2" = "x86_64" ]; then
    echo "    - 内核（Resources/benagen-core）：x86_64 必须走交叉编译，先过" >&2
    echo "      core/scripts/preflight_x86_64_toolchain.sh；cargo 必须是 \$HOME/.cargo/bin/cargo" >&2
    echo "      （rustup 那份），否则 cargo 会拿本机架构编，而这一步不报任何错。" >&2
    echo "      命令形如：cargo build --release --target x86_64-apple-darwin" >&2
    echo "    - aria2c（Resources/${ARIA2C_NAME}）：入库资产，重新产出的命令是" >&2
    echo "      bash downloader/scripts/build_aria2_macos.sh x86_64 （它自己也会验架构）。" >&2
  else
    echo "    - **arm64 是本脚本的「原生构建」那一支**：不带参数时用的是" >&2
    echo "      cargo build --release（**不带 --target**）与 swift build -c release（**不带 --arch**），" >&2
    echo "      所以它编出来的**永远是宿主架构**。" >&2
    if [ "$HOST_ARCH" != "arm64" ]; then
      echo "      而本机宿主是 ${HOST_ARCH} ⇒ 三个二进制会全是 ${HOST_ARCH}、目标是 arm64。" >&2
      echo "      ⇒ 在非 Apple Silicon 机器上出 arm64 包需要交叉编译（--target aarch64-apple-darwin" >&2
      echo "        ＋ swift build --arch arm64），**本脚本没有验过那条路、故意不提供**；" >&2
      echo "        请在 Apple Silicon 机器上出这个包，别在这里绕。" >&2
      echo "    - 若你本来就想出**本机架构**的包：在非 arm64 宿主上请显式传 x86_64" >&2
      echo "      （bash macos/scripts/build_app_macos.sh x86_64）。" >&2
    else
      echo "      本机宿主就是 arm64 ⇒ **不是**交叉编译的问题，回到上面那条" >&2
      echo "      「先查这一条」（多半是壳的产物没重建）；内核那边也可以直接看：" >&2
      echo "        lipo -archs core/target/release/benagen-core" >&2
    fi
  fi
  echo "" >&2
  echo "  **不许**跳过这一步继续用别的架构（F-2）：那是「静默产出一个坏包」。" >&2
  exit 1
}

verify_arch() {  # $1 = 可执行文件路径，$2 = 期望架构（arm64 / x86_64）
  local path="$1" want="$2" got
  if [ ! -f "$path" ]; then
    loud_arch_failure "$path" "$want" "（文件不存在）"
  fi
  got="$(lipo -archs "$path" 2>/dev/null || true)"
  if [ "$got" != "$want" ]; then
    loud_arch_failure "$path" "$want" "${got:-（lipo 读不出架构）}"
  fi
  printf '    \033[32m✓\033[0m lipo -archs = %s\n' "$got"
  printf '      %s\n' "$(file -b "$path")"
}

# ⚠️ 这里**只预检入库文件**（core/assets/ 两个 + 品牌资产三个），不预检 $CORE_BIN。
#    $CORE_BIN 由本脚本**自己**在下面 `cargo build --release` 现编出来 —— 把它放进预检循环
#    会让"可随时重建"这个前提失效：core/target/ 被 core/.gitignore 忽略（`git ls-files core/ |
#    grep -c target` == 0），所以任何一次干净检出或 `cargo clean` 之后，预检必然失败、
#    脚本以退出码 2 死在编译之前，**连内核都不编**。它的存在性检查放在 `cargo build` 之后。
#
# ⚠️ 品牌资产（`macos/Resources/` 三件）也在这里：它们同样是**入库的、本脚本不生成的**
#    文件，缺件的正确处置是"立刻说清楚、别白编一趟"，而不是等几分钟的编译跑完
#    再让 `cp` 报一句同样的事。**空文件在这里不算缺**（`-f`）—— 那属于"包组装得对不对"，
#    由下面组装之后那道自查管（用 `-s`）。
for f in "$ARIA2C_SRC" "$GPL_SRC" \
         "$BRAND_DIR/AppIcon.icns" "$BRAND_DIR/benagen-mark.png" "$BRAND_DIR/benagen-full-logo.png"; do
  if [ ! -f "$f" ]; then
    echo "错误：找不到 $f" >&2
    echo "（这些是入库文件：core/assets/ 两个 + macos/Resources/ 三个品牌资产，" >&2
    echo "  缺失说明检出或工作区有问题；品牌资产可用 python3 macos/scripts/make_brand_assets.py 重新生成）" >&2
    exit 2
  fi
done

echo "==> 目标架构：$ARCH"

# ---------------------------------------------------------------------------
# 0) x86_64 的工具链预检（**在编译之前**，见 F-2）
# ---------------------------------------------------------------------------
# 拿不到 x86_64 的 std 就不可能产出 Intel 包。预检脚本（阶段 F 任务 1 的落地物）
# 会把"确切的、可执行的补救配方"整段打出来；本脚本**不替它降级**：
# 不装工具链、不改 PATH、更不会退回去编一个 arm64 包再叫它 Intel 包。
# arm64 那条路完全不经过这里（F-4）。
if [ -n "$CORE_TARGET" ]; then
  PREFLIGHT="$REPO/core/scripts/preflight_x86_64_toolchain.sh"
  echo "==> x86_64 工具链预检（${PREFLIGHT}）"
  if [ ! -f "$PREFLIGHT" ]; then
    echo "错误：找不到 $PREFLIGHT —— 它是 x86_64 构建路径的前置。" >&2
    exit 2
  fi
  if ! bash "$PREFLIGHT"; then
    echo "" >&2
    echo "错误：x86_64 工具链预检没过（配方见上）。包没有做出来。" >&2
    echo "      **不会**退回去编一个 arm64 包充数（F-2：不许静默降级）。" >&2
    exit 2
  fi
fi

# 先在 dist/ 下的临时目录里组装，最后整体换位：
# 中途失败不会在 dist/ 留下一个"看着像包、其实缺东西"的半成品。
mkdir -p "$DIST"
STAGE="$(mktemp -d "$DIST/.stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

CONTENTS="$STAGE/${BIN_NAME}.app/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

echo "==> 编译内核（cargo build --release${CORE_TARGET_DESC}）"
if [ -n "$CORE_TARGET" ]; then
  (cd "$REPO/core" && cargo build --release --target "$CORE_TARGET")
else
  (cd "$REPO/core" && cargo build --release)
fi

# 内核产物存在性检查放在它自己的编译**之后**（见文件上方预检处的说明）：
# cargo 成功却拿不到可执行文件，说明 target 目录或 package 配置有问题，这是真错误。
if [ ! -f "$CORE_BIN" ]; then
  echo "错误：cargo build --release 成功，但找不到内核产物 $CORE_BIN" >&2
  echo "（检查 core/Cargo.toml 的 bin 名称是否仍为 benagen-core）" >&2
  exit 2
fi

echo "==> 编译壳（swift build -c release${SWIFT_ARCH_DESC}）"
# ${arr[@]+"${arr[@]}"} 的写法是为了兼容 macOS 自带的 bash 3.2：空数组在 `set -u` 下
# 直接展开会报 unbound variable（与 macos/scripts/test.sh 里的同款处理）。
(cd "$ROOT" && swift build -c release ${SWIFT_ARCH_ARGS[@]+"${SWIFT_ARCH_ARGS[@]}"})

# ⚠️ 产物路径不写死：Swift 6.4 是 .build/<triple>/release/（规格 §11 写的 .build/release/ 已过时），
#    而且 SwiftPM 有权改这个布局 —— 由 SwiftPM 自己报出来才是唯一可靠的口径。
# ⚠️ 已实测：单架构（不带 --arch 的 arm64 与 `--arch x86_64`）**两条路报同一个目录**
#    （.build/out/Products/Release），切换架构时 SwiftPM 会重建该产物。
#    万一它没重建，下面那道 verify_arch 会拿到上一次的架构并**大声失败**——
#    这正是"不许静默"的落点。
BIN_DIR="$(cd "$ROOT" && swift build -c release ${SWIFT_ARCH_ARGS[@]+"${SWIFT_ARCH_ARGS[@]}"} --show-bin-path)"
echo "    产物目录：$BIN_DIR"

echo "==> 组装 bundle"
cp "$CORE_BIN"   "$CONTENTS/Resources/benagen-core"
cp "$ARIA2C_SRC" "$CONTENTS/Resources/$ARIA2C_NAME"
cp "$GPL_SRC"    "$CONTENTS/Resources/COPYING-GPLv2.txt"
# 三个品牌资产：**目标文件名**与壳里的 `BrandAssets` 常量逐字对应（改名要两边一起改）。
# `AppIcon.icns` 这个名字同时被下面 Info.plist 的 `CFBundleIconFile` 引用（不带扩展名）。
cp "$BRAND_DIR/AppIcon.icns"          "$CONTENTS/Resources/AppIcon.icns"
cp "$BRAND_DIR/benagen-mark.png"      "$CONTENTS/Resources/benagen-mark.png"
cp "$BRAND_DIR/benagen-full-logo.png" "$CONTENTS/Resources/benagen-full-logo.png"
cp "$BIN_DIR/$BIN_NAME" "$CONTENTS/MacOS/$BIN_NAME"
chmod +x "$CONTENTS/MacOS/$BIN_NAME" \
         "$CONTENTS/Resources/benagen-core" \
         "$CONTENTS/Resources/$ARIA2C_NAME"

echo "==> 组装 Info.plist"
cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleExecutable</key>
    <string>${BIN_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${BIN_NAME}</string>
    <!-- 界面标题里给客户看的名字（可与可执行文件名不同）。
         启动取证脚本按这个名字找窗口，改这里要同步改 verify_app_launch_macos.sh 的 WINDOW_OWNER。 -->
    <key>CFBundleDisplayName</key>
    <string>Benagen 数据下载工具</string>
    <!-- 应用图标。⚠️ **不带扩展名**是这个键的约定（指向 Contents/Resources/AppIcon.icns）。
         写错（或不来这一键）的表现是 Dock / 访达里一个**空白默认图标**，而它不报任何错。
         Finder 有 LaunchServices 的图标缓存：换图标后还看见旧的属于缓存，不是失败。 -->
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>${MIN_MACOS}</string>
    <!-- 不声明 LSBackgroundOnly/ LSUIElement：这是带窗口的前台应用 -->
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# plutil 校验：写坏的 plist 会让整个包"点了没反应"，而那是客户最难自查的故障。
echo "==> 校验 Info.plist"
plutil -lint "$CONTENTS/Info.plist"

# 包内内核缺了或没有可执行位，症状与"没打包"完全一样（点了没反应）——在换位之前先自查。
for f in "$CONTENTS/Resources/benagen-core" "$CONTENTS/Resources/$ARIA2C_NAME" \
         "$CONTENTS/MacOS/$BIN_NAME"; do
  if [ ! -x "$f" ]; then
    echo "错误：$f 不存在或没有可执行位 —— 包组装失败" >&2
    exit 1
  fi
done

# ---- 产物架构自验（F-1）-----------------------------------------------------
# 验的是**组装好的那三个二进制**（而不是三个来源文件）：来源对了而 cp 拿错文件、
# 或者某个环节静默产出了别的架构，都只有这一步能发现。判据与失败形态见文件头部
# `verify_arch` / `loud_arch_failure`。
#
# 位置在 `mv` **之前**：失败时 dist/ 里不会留下一个"看着像包、架构却是错的"半成品
# —— 与上面"先在临时目录组装"的纪律同源。
#
# ⚠️ 三个名字与壳/Aria2 的分发约定逐字对应：
#    Contents/MacOS/$BIN_NAME                     ← 壳（swift build 的产物）
#    Contents/Resources/benagen-core              ← 内核（壳的查找顺序②，名字写死在 CoreClient）
#    Contents/Resources/$ARIA2C_NAME              ← 内嵌引擎（GPL 分发义务的载体；名字跟着架构走）
echo "==> 产物架构自验（目标 ${ARCH}，判据 lipo -archs 恰好等于 ${ARCH}）"
verify_arch "$CONTENTS/MacOS/$BIN_NAME"        "$ARCH"
verify_arch "$CONTENTS/Resources/benagen-core" "$ARCH"
verify_arch "$CONTENTS/Resources/$ARIA2C_NAME" "$ARCH"
echo "    包里三个二进制都是 $ARCH ✓"

# ---- 产物最低系统版本自验（2026-09-20 补，**它不是可选的**）------------------
#
# 🔴 为什么必须在这里判、而不是"应该有人会看一眼"：
#    **`lipo -archs` 对部署目标是瞎的。** 三个二进制的架构都对、产物名也对、
#    上面每一条判据全绿 —— 而壳的 minos 是 14.0、内嵌 aria2c 是 26.0，
#    客户那台 macOS 13.7.8 的 Intel 机上**直接被拒绝启动**。
#    2026-09-20 交付出去的正是这样一份包。
#
# ⚠️ 三个二进制的 minOS **各有各的来源**，所以只能逐个判、不能"改一个地方就以为全好了"：
#      · 壳          ← `Package.swift` 的 `platforms`（真正的闸门）
#      · benagen-core ← rustc 对该 target 的默认值（11.0 / 10.12，本来就合格）
#      · aria2c      ← 它自己的编译标志（`downloader/scripts/build_aria2_macos.sh`）
#    判据的实现、两种 Mach-O 形态、以及"读不出必须判死"的理由都在那个脚本的文件头。
echo "==> 产物最低系统版本自验（判据：每个二进制的 minOS ≤ ${MIN_MACOS}）"
if ! bash "$ROOT/scripts/check_minos_macos.sh" "$(dirname "$CONTENTS")" "$MIN_MACOS"; then
  echo "" >&2
  echo "本次不产出 .app：上面那条判据没过 —— 这一份装到 ${MIN_MACOS} 那一档系统上会被拒绝。" >&2
  exit 1
fi

# 品牌资产在包里**真的落地了**没有 —— 只看存在性（图片没有可执行位，
# 不适用上面那个 `-x` 循环）。
#
# ⚠️ 为什么必须拦下：壳那侧读不到 logo 是**不报错**的 —— 视图整块不渲染它、
#    也不弹任何提示（装饰性资产，见 `BrandAssets.swift` 顶部关于"判据要放在它能生效的
#    地方"的那段）。于是"缺件"这个缺陷唯一的发现地点就是**打包时**。
#    与开头那次预检的分工：那次看的是**输入**（入库文件在不在，快速失败），
#    这一次看的是**组装结果** —— 目的文件名写错时 `cp` 会静默造出一个别的名字，
#    只有这一道能发现，而"名字对不上"正是壳那边不报错的失效方式。
#    用 `-s` 而不是 `-f`：0 字节的 logo 与没有 logo 在界面上是同一件事。
for f in "$CONTENTS/Resources/AppIcon.icns" "$CONTENTS/Resources/benagen-mark.png" \
         "$CONTENTS/Resources/benagen-full-logo.png"; do
  if [ ! -s "$f" ]; then
    echo "错误：$f 不存在或为空 —— 品牌资产缺件，包不做出来" >&2
    echo "（这三个是 macos/Resources/ 下的入库产物；重新生成：python3 macos/scripts/make_brand_assets.py）" >&2
    exit 1
  fi
done

rm -rf "$APP"
mv "$STAGE/${BIN_NAME}.app" "$APP"

echo "==> 产物"
find "$APP" -print | sed 's|^|    |'
du -sh "$APP"

echo "==> 签名状态（本阶段预期为 ad-hoc / 未签名）"
if codesign -dv "$APP" 2>&1 | sed 's|^|    |'; then
  :
else
  echo "    （codesign 未给出签名信息——即未签名）"
fi

echo "==> 隔离属性（空 = 未带 quarantine，可直接运行）"
xattr -l "$APP" 2>/dev/null | sed 's|^|    |' || true

echo "==> 完成：${APP}（${ARCH}，三个二进制已逐个自验）"
echo "    启动取证见同目录 verify_app_launch_macos.sh（或手工 open + lsappinfo + pgrep -fl benagen-core）"
echo "    装盘见同目录 build_dmg_macos.sh（产物名跟着包的架构走，见 F-3）"
