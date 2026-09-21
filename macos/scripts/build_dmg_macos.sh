#!/usr/bin/env bash
# 把已构建的 `.app` 打成可安装的 DMG。
#
# ⚠️ **本脚本不构建 `.app`**（那是 `build_app_macos.sh` 的事）。它只做"装盘"：
#    预检产物存在 → 摆好安装布局 → `hdiutil` 压成 UDZO → **自己挂载一次验证内容**。
#    这样跑之前必须先跑 `bash macos/scripts/build_app_macos.sh`。
#
# 产物名**自解释**（F-3）：本阶段出**两个独立包**（不做通用二进制，见阶段 F 计划），
# 客户必须一眼看出下哪个：
#     BenagenDownloader-AppleSilicon.dmg   ← arm64
#     BenagenDownloader-Intel.dmg          ← x86_64
#
# ⚠️ **架构是"读"出来的，不是"传"进来的**：脚本对包内那三个二进制逐个 `lipo -archs`，
#    三者必须是**同一个单架构**，再由它决定文件名。于是"名字说 Intel、里面其实是
#    arm64"这种形态**在结构上不可能发生**——名字是从字节推出来的，不是从参数抄来的。
#    包是通用二进制（两个架构）时**拒绝**：它没法被命名成其中一个，本阶段也不做通用包。
#    这一条与 `build_app_macos.sh` 的 F-1 自验是**同一条纪律的两个落点**：
#    那边验"编出来的对不对"，这边验"装进去的与名字对不对得上"。
#
# ⚠️ **本脚本不删别人的文件**：`dist/` 里可能还躺着早期那个看不出架构的
#    `BenagenDownloader.dmg`，脚本**不碰它**（只在自己的产物名上 `rm -f`）。
#    那个含糊名字的处置留给验收者。
#
# ⚠️ **安装布局**：卷里放 `.app` **和一条指向 `/Applications` 的符号链接** ——
#    这是 macOS 用户认得的"拖进去"形态。没有那条链接，用户得自己想"拖到哪儿"。
#
# ⚠️ **不做签名、不做公证**（与 `build_app_macos.sh` 同一决定：本阶段交付给自己人测）。
#    后果要说清楚，别让人以为"双击就能装到任何人机器上"：
#    - **本机**产的盘不带 `com.apple.quarantine`，双击直接开（实测 `xattr -l` 为空）。
#    - **经网络传到别人机器上**时，系统会给它打上 quarantine，Gatekeeper 会拦一次。
#      放行方式**按系统版本分**——旧文档那句"右键 →「打开」"**从 macOS 15 Sequoia 起已失效**
#      （Apple 移除了那条路），照着念会让客户卡在"我右键了但没这个选项"：
#        macOS 15 及以上：**先双击一次**（会被拦下）→ 系统设置 →「隐私与安全性」→
#                          在"安全性"里找到那条拦截记录 → 点「仍要打开」→ 再确认一次。
#                          ⚠️ 那个按钮**只在尝试打开之后的约 1 小时内可用**；
#                          隔太久就再双击一次，回到系统设置里点。
#        macOS 14 及以下：右键（或按住 Control 点击）→「打开」→ 再确认一次。
#    - **两个架构这一步完全一样**：ad-hoc **不是** Developer ID，而 Gatekeeper 只认
#      Developer ID + 公证 ⇒ 带 quarantine 时，"ad-hoc 签名的 arm64"与"完全不签的 x86_64"
#      是同一个结局（都被拦一次、都走上面那条放行流程）。
#      两者唯一真正的差别是：**arm64 上"有 ad-hoc 签名"是内核的执行要求**（不签根本起不来，
#      链接器会自动补上），x86_64 **不要求**（`codesign -dv` 报 `not signed at all` 也照跑）。
#      ⇒ 准确的说法是"**Intel 那份执行不受影响、安全姿态并不更强**"，
#      **不是**"Intel 那份更坏"。
#
# 用法（架构从**已构建的 .app** 里读出来，所以没有架构参数）：
#   bash macos/scripts/build_app_macos.sh [arm64|x86_64]
#   bash macos/scripts/build_dmg_macos.sh
#
#   两个架构各跑一遍这两条，就得到 BenagenDownloader-AppleSilicon.dmg 与
#   BenagenDownloader-Intel.dmg —— 见 macos/README.md「出 Intel 包」。
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST="$REPO/macos/dist"
APP="$DIST/BenagenDownloader.app"
BIN_NAME="BenagenDownloader"

if [ ! -d "$APP" ]; then
  echo "错误：找不到 $APP" >&2
  echo "先跑：bash macos/scripts/build_app_macos.sh（或 ... build_app_macos.sh x86_64）" >&2
  exit 2
fi

# 产物必须有个能执行的主可执行文件 —— 否则打出来的是一张"看起来像应用"的空壳。
# 这条来自 build_app_macos.sh 的同一纪律：宁可在这里失败，也不要交付一个装得上、打不开的包。
MAIN="$APP/Contents/MacOS/$BIN_NAME"
if [ ! -x "$MAIN" ]; then
  echo "错误：$MAIN 不存在或不可执行 —— 这个 .app 是坏的，先重建。" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# 架构探测 + 自验（名字从字节推出来，不从参数抄过来）
# ---------------------------------------------------------------------------
# 包内三个二进制：壳、内核（壳的查找顺序②）、内嵌引擎（GPL 载体）。
CORE="$APP/Contents/Resources/benagen-core"
# 引擎名带架构（build_app_macos.sh 按架构落地），所以这里用 glob 找而不是写死。
# 0 个 = 包不完整；2 个以上 = 包里同时有两种架构的引擎，同样不该出现。
ARIA2C=""
ARIA2C_COUNT=0
for f in "$APP/Contents/Resources/"aria2c-macos-*; do
  [ -f "$f" ] || continue
  ARIA2C_COUNT=$((ARIA2C_COUNT + 1))
  ARIA2C="$f"
done
if [ "$ARIA2C_COUNT" -ne 1 ]; then
  echo "错误：包内 Contents/Resources/aria2c-macos-* 匹配到 $ARIA2C_COUNT 个（应恰好 1 个）。" >&2
  echo "      这个 .app 不是 build_app_macos.sh 产出的形态，先重建。" >&2
  exit 2
fi

echo "==> 探测包内架构（lipo -archs）"
ONLY_ARCH=""
for f in "$MAIN" "$CORE" "$ARIA2C"; do
  if [ ! -f "$f" ]; then
    echo "错误：包内缺 $(basename "$f")（${f}）—— 这个 .app 不完整，先重建。" >&2
    exit 2
  fi
  got="$(lipo -archs "$f" 2>/dev/null || true)"
  printf '    %-22s %s\n' "$(basename "$f")" "${got:-（lipo 读不出架构）}"
  # ⚠️ **"读不出"必须当场判死**，不能留给下一轮去比。写成
  #    `[ -z "$ONLY_ARCH" ] && ONLY_ARCH="$got"` 时，`got` 为空会把 ONLY_ARCH 留在空值上，
  #    于是"三个必须同架构"**静默退化成**"三个都等于第一个**非空**值"——
  #    第一个读不出的二进制被跳过、它的架构**从来没被验过**，而产物名照样照后两个定下来
  #    （复现过：把壳换成非 Mach-O 可执行文件，脚本照样打印"包是单架构 arm64 ⇒ 产物名
  #    BenagenDownloader-AppleSilicon.dmg"，失败点被推迟到盘内自验、退出码才非 0，
  #    而且 dist/ 里留下一张**名字与内容不符**的半成品 —— 验收动作恰好就是在这个目录里
  #    逐个 lipo 复查）。判据与 `build_app_macos.sh` 的 `verify_arch()` **逐字对齐**：
  #    **读不出 = 失败**。两处标准必须一致：否则同一份字节会出现"打包脚本说行、装盘脚本说不行"，
  #    而这条链上的每个判据都会被人在验收时独立复查一遍。
  if [ -z "$got" ]; then
    echo "错误：$(basename "$f") 的架构读不出（lipo -archs 没有输出）—— 它多半不是 Mach-O 可执行文件。" >&2
    echo "      这个 .app 不完整或被替换过，不能用它推产物名（名字必须从**三个**字节推出来）。" >&2
    echo "      先重建：bash macos/scripts/build_app_macos.sh [arm64|x86_64]" >&2
    exit 2
  fi
  if [ -z "$ONLY_ARCH" ]; then ONLY_ARCH="$got"; fi
  if [ "$got" != "$ONLY_ARCH" ]; then
    echo "错误：包内三个二进制的架构不一致（上面那份是 ${ONLY_ARCH:-读不出}，$(basename "$f") 是 ${got:-读不出}）。" >&2
    echo "      混架构的包不能交付；用 build_app_macos.sh 重新出一份单架构包。" >&2
    exit 2
  fi
done

# 架构 → 产物名。这里是 F-3 的全部：**名字必须让人看出该下哪个**。
case "$ONLY_ARCH" in
  arm64)
    STEM="BenagenDownloader-AppleSilicon"
    ;;
  x86_64)
    STEM="BenagenDownloader-Intel"
    ;;
  *)
    echo "错误：包内架构是「${ONLY_ARCH:-读不出}」—— 给不出自解释的产物名。" >&2
    echo "      本阶段的决定是**两个独立包**、不做通用二进制（通用包在这里无法命名），" >&2
    echo "      而且客户也需要一眼看出下哪个。请用单架构重建：" >&2
    echo "        bash macos/scripts/build_app_macos.sh arm64    → AppleSilicon" >&2
    echo "        bash macos/scripts/build_app_macos.sh x86_64   → Intel" >&2
    exit 2
    ;;
esac
echo "    包是单架构 $ONLY_ARCH ⇒ 产物名 $STEM.dmg"

VOL_NAME="$STEM"
OUT="$DIST/${STEM}.dmg"

# 挂载点自查：同名的卷已经挂着时，`hdiutil attach` 会把它挂成「$VOL_NAME 1」，
# 而下面那句"取第一个 /Volumes/ 路径"就可能抓到**旧的那个卷**，于是自验看的是别人的内容。
# 与其在那种歧义里继续，不如现在就说清楚（不替谁卸载别人的卷）。
if [ -d "/Volumes/$VOL_NAME" ]; then
  echo "错误：/Volumes/$VOL_NAME 已经挂着了 —— 先卸载它再跑本脚本：" >&2
  echo "      hdiutil detach /Volumes/$VOL_NAME" >&2
  exit 2
fi

STAGE="$(mktemp -d)"
# 成功失败都清理：失败路径也不该在 /tmp 里留下一个 13MB 的应用副本
trap 'rm -rf "$STAGE"' EXIT

echo "==> 摆放安装布局"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

echo "==> 压缩为 DMG（UDZO）"
rm -f "$OUT"
hdiutil create \
  -volname "$VOL_NAME" \
  -srcfolder "$STAGE" \
  -ov -format UDZO \
  "$OUT" >/dev/null

# ⚠️ **自己挂载一次**再报成功：`hdiutil create` 退出码为 0 只说明"写文件成功"，
#    不说明"这张盘挂得起来、里面有东西"。本项目一贯的纪律是"跑完看结果"，不是"命令没报错"。
echo "==> 挂载验证"
MOUNT_POINT="$(hdiutil attach "$OUT" -nobrowse -readonly | grep -o '/Volumes/.*' | head -1)"
if [ -z "$MOUNT_POINT" ]; then
  echo "错误：DMG 做出来了但挂载不上。" >&2
  exit 1
fi
trap 'hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true; rm -rf "$STAGE"' EXIT

echo "--- 卷内容 ---"
ls -la "$MOUNT_POINT"
if [ ! -x "$MOUNT_POINT/$BIN_NAME.app/Contents/MacOS/$BIN_NAME" ]; then
  echo "错误：卷里那个 .app 不可执行。" >&2
  exit 1
fi
if [ ! -L "$MOUNT_POINT/Applications" ]; then
  echo "错误：卷里没有指向 /Applications 的符号链接（用户会不知道该拖到哪儿）。" >&2
  exit 1
fi

# 挂载自验的第二步：**从盘里读回来的**那三个二进制必须与产物名说的架构一致。
# （上面那次探测读的是 dist/ 里的 .app，这一次读的是**真装进 DMG 的字节**——
#  hdiutil 复制/压缩出问题时，只有这一步能发现。）
echo "--- 盘内架构自验（应为 ${ONLY_ARCH}，与产物名 $STEM 一致）---"
for f in "$MOUNT_POINT/$BIN_NAME.app/Contents/MacOS/$BIN_NAME" \
         "$MOUNT_POINT/$BIN_NAME.app/Contents/Resources/benagen-core" \
         "$MOUNT_POINT/$BIN_NAME.app/Contents/Resources/$(basename "$ARIA2C")"; do
  got="$(lipo -archs "$f" 2>/dev/null || true)"
  if [ "$got" != "$ONLY_ARCH" ]; then
    echo "错误：盘内 $(basename "$f") 的架构是「${got:-读不出}」，与产物名 ${STEM}（${ONLY_ARCH}）不符。" >&2
    echo "      这张盘不能交付：名字说的架构与实际内容对不上。" >&2
    exit 1
  fi
  printf '    \033[32m✓\033[0m %-22s %s\n' "$(basename "$f")" "$got"
done

# ---- 盘内最低系统版本自验（2026-09-20 补）-----------------------------------
#
# 为什么 `build_app_macos.sh` 已经判过、这里还要**再判一次**：
#   本脚本**不重建 .app**（见文件头）—— 它把 `dist/` 里现成的那一份装盘。
#   ⇒ "改完代码没重跑 build_app、直接跑这个"会把**旧包**重新装盘，
#     而产物名一样、架构一样、盘内架构自验照样全绿 —— 这一条是那种情况下唯一的红灯。
#   （2026-09-20 交付出去的那份包，就是"架构全对、最低系统版本全错"的形态。）
#
# 期望下限取自 **`Package.swift`**，不是本脚本里再抄一个常量：
# 部署目标的唯一来源就是它（`build_app_macos.sh` 的 `MIN_MACOS` 是第二处、
# 那里有一条"两处必须同值"的断言；这里读同一个来源，免得出现第三份）。
EXPECTED_MINOS="$(grep -oE '\.macOS\(\.v[0-9]+\)' "$REPO/macos/Package.swift" | head -1 | grep -oE '[0-9]+' || true)"
[ -n "$EXPECTED_MINOS" ] || {
  echo "错误：读不出 Package.swift 的 platforms 声明 —— 盘内最低系统版本没有可比的期望值。" >&2
  exit 2
}
echo "--- 盘内最低系统版本自验（期望下限 ${EXPECTED_MINOS}.0，读的是真装进 DMG 的字节）---"
if ! bash "$REPO/macos/scripts/check_minos_macos.sh" "$MOUNT_POINT/$BIN_NAME.app" "${EXPECTED_MINOS}.0"; then
  echo "错误：盘内那一份的某个二进制超过了 ${EXPECTED_MINOS}.0 —— 这张盘不能交付。" >&2
  echo "      先重建 .app 再装盘：bash macos/scripts/build_app_macos.sh [arm64|x86_64]" >&2
  exit 1
fi

echo "==> 卸载"
hdiutil detach "$MOUNT_POINT" >/dev/null
trap 'rm -rf "$STAGE"' EXIT

echo "==> 完成：${OUT}（${ONLY_ARCH}）"
echo "    大小：$(du -h "$OUT" | awk '{print $1}')"
echo "    校验和：$(shasum -a 256 "$OUT" | awk '{print $1}')"

# 含糊名字的旧产物由**验收者**处置，本脚本不删别人的文件（见文件头）。
if [ -f "$DIST/BenagenDownloader.dmg" ] && [ "$OUT" != "$DIST/BenagenDownloader.dmg" ]; then
  echo "    注：dist/ 里还有早期那个看不出架构的 BenagenDownloader.dmg，本脚本不碰它。"
fi
