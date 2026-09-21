#!/usr/bin/env bash
# 启动取证：证明 .app **真的起了窗口**，而不只是"进程还活着"。
#
# 为什么需要它：一个 bundle 里的二进制路径写错、Info.plist 的 CFBundleExecutable
# 与文件名对不上、或者链接期的三方库缺失，都会表现成"点了没反应"——进程可能起了又退，
# 也可能起来但从未接入 WindowServer。只看 ps 是分辨不出来的。
#
# 两路取证：
#   1. `lsappinfo`   —— 应用是否以 type="Foreground" 接入 WindowServer
#   2. CoreGraphics  —— 屏幕上是否真有一个属于它的窗口，标题与尺寸是多少
# 两路都拿到才算通过；脚本以退出码表明结论（0=通过，非 0=没起窗口）。
#
# ⚠️ 本脚本**不点击任何按钮**（取证环境没有屏幕录制/辅助功能权限）。
#    它只证明"窗口打开了"。被点击驱动的流程不由它负责，不要在这里假装点过。
#
# ⚠️ 本脚本**不检查内核子进程**（`pgrep -fl benagen-core`）——那一项是手工取证步骤，
#    见任务简报步骤 6：窗口能开只证明 SwiftUI 活着，证明不了壳找到了内核。
#
# 用法：bash macos/scripts/verify_app_launch_macos.sh [BenagenDownloader.app 路径]
set -euo pipefail

APP="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/dist/BenagenDownloader.app}"
BUNDLE_ID="com.benagen.downloader"
WINDOW_OWNER="Benagen 数据下载工具"

if [ ! -d "$APP" ]; then
  echo "错误：找不到 $APP —— 先跑 macos/scripts/build_app_macos.sh" >&2
  exit 2
fi

CG_SRC="$(mktemp -t benagen-cgwin).swift"

# 收尾。挂在 EXIT 上（**唯一的** trap：bash 里对同一信号再 trap 会替换掉前一个），
# 所以它同时管三件事，并且覆盖**每一条**出口 —— 包括下面那几个 `exit 1` 的失败路径。
#
# ⚠️ 为什么必须显式收**内核**：`pkill` 发的是 SIGTERM，而壳里没有任何信号处理器，
#    所以 `applicationWillTerminate` → `model.shutdown()` **根本不会执行**
#    （`App.swift` 只把收尾挂在 willTerminate 上）。健康的核会随管道 EOF 自己退，
#    但**卡在 `open()` 里的核不读 stdin、收不到 EOF** —— 于是留下一个 PPID=1 的孤儿，
#    而它日后若等到用户点了那个系统授权框的「允许」，**会醒过来写同一个状态文件**
#    （即"同时跑着两个内核"）。实测：本轮审查前脚本每跑一次就漏一个。
#
# ⚠️ 为什么用 `pkill -f benagen-core` 而不是 `osascript -e 'quit app id …'`：
#    优雅退出更彻底（走 willTerminate），但它需要"自动化"权限、未授权时会弹框或直接失败，
#    而收尾必须在无权限、无点击的前提下都能跑（与本脚本"不点击任何按钮"的边界一致）。
#    按进程名收是确定性的；`-f` 匹配完整命令行，不会误伤同名的无关进程。
#
# ⚠️ 它**不参与判定**：只清理，不改退出码（每条都有 `|| true`）。
cleanup() {
  rm -f "$CG_SRC" 2>/dev/null || true
  pkill -f "$APP/Contents/MacOS/" 2>/dev/null || true
  pkill -f "benagen-core" 2>/dev/null || true
}
trap cleanup EXIT
cat > "$CG_SRC" <<'SWIFT'
import CoreGraphics
import Foundation

// 只列**屏幕上的**普通窗口层（layer==0），排除菜单/浮层等辅助窗口。
let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] ?? []
var found = 0
for w in list {
    let owner = w[kCGWindowOwnerName as String] as? String ?? ""
    let name  = w[kCGWindowName as String] as? String ?? ""
    guard owner.contains("Benagen") || name.contains("Benagen") else { continue }
    let layer = w[kCGWindowLayer as String] as? Int ?? -1
    let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    found += 1
    print("window owner=\(owner) title=\(name) layer=\(layer) bounds=\(b)")
}
print("matched_windows=\(found)")
SWIFT

echo "==> open $APP"
open "$APP"
sleep 4

echo
echo "==> [1/2] lsappinfo：是否以 Foreground 接入 WindowServer"
LSAPPINFO="$(lsappinfo list 2>/dev/null || true)"
# 取 bundleID 那一行往上 1 行（ASN 行）、往下 6 行（pid/type 等）
LAUNCH_BLOCK="$(printf '%s\n' "$LSAPPINFO" | grep -B1 -A6 "bundleID=\"$BUNDLE_ID\"" || true)"
if [ -z "$LAUNCH_BLOCK" ]; then
  echo "    没找到 bundleID=$BUNDLE_ID 的已注册应用 —— 应用没有起来"
  printf '%s\n' "$LSAPPINFO" | grep -i -A4 "benagen" | head -20 || true
  exit 1                 # 关闭动作在 EXIT 陷阱里（见文件上方 cleanup）
fi
printf '%s\n' "$LAUNCH_BLOCK" | sed 's|^|    |'
if ! printf '%s\n' "$LAUNCH_BLOCK" | grep -q 'type="Foreground"'; then
  echo "    ✗ 应用已注册但不是 Foreground（没有接入 WindowServer）"
  exit 1                 # 关闭动作在 EXIT 陷阱里
fi
echo "    ✓ type=\"Foreground\"（已接入 WindowServer）"

echo
echo "==> [2/2] CoreGraphics：屏幕上是否真有它的窗口"
CG_OUT="$(swift "$CG_SRC" 2>/dev/null || true)"
printf '%s\n' "$CG_OUT" | sed 's|^|    |'
if ! printf '%s\n' "$CG_OUT" | grep -q 'matched_windows=[1-9]'; then
  echo "    ✗ 屏幕上没有属于它的窗口"
  echo "    提示：若 title 为空但 owner 命中，可能是本机未授予屏幕录制权限"
  echo "          （CGWindowName 会被系统隐去）——那种情况不算窗口没开。"
  exit 1                 # 关闭动作在 EXIT 陷阱里
fi
echo "    ✓ 窗口已在屏幕上（title/bounds 见上）"

echo
echo "==> 取证通过，关闭应用（壳 + 内核，在 EXIT 陷阱里；内核那一份见文件上方 cleanup 的说明）"
echo "==> 完成"
