#!/usr/bin/env bash
#
# windows/scripts/check_transfers_screen.sh —— **传输列表屏的无头验收，一条命令跑完**。
#
#    bash windows/scripts/check_transfers_screen.sh            # 跑 still（13 格 + 行菜单 + 动作）
#    bash windows/scripts/check_transfers_screen.sh --all      # 再跑 tick / empty / down / gate / loading 五个场景
#
# 它做的事（四步，全自动）：起本地静态服务 → 用无头浏览器加载夹具页 →
#   夹具页跑断言、把结果 **POST 回来** → 打印 `ok=N fail=M abort=K` 并据此定退出码。
#
# ---------------------------------------------------------------------------
# 它为什么长这样（**照 `check_files_screen.sh` 的形状**）
# ---------------------------------------------------------------------------
#   任务 10 把"文件页的无头验收"从"临时搭一次、截几张图"变成了**一条命令**，
#   并把夹具页与 runner 一起入库（R-61：**"修好了"必须答得出"它现在被什么守着"**）。
#   任务 11 这一屏复用同一条路，一个字都不改那套机制：
#     · 夹具页 `frontend-stub/transfers-harness.html`（**常驻**，不在 `windows/web/` 下）；
#     · 载荷 `frontend-stub/wire-fixtures.json`（由 `make_wire_fixtures.sh` 从 Rust 倒出来，
#       另有 `check_wire_fixtures.sh` 守着它不漂移）；
#     · 这一条命令给出"跑了几条、过了几条、红的是哪几条"。
#
# ---------------------------------------------------------------------------
# 🔴 它**自任务 10b 起接进了 `test.sh` 第 0.9 步**（同 `check_files_screen.sh` 的裁决）
# ---------------------------------------------------------------------------
#   原先这里写的是"它没有接进 `test.sh`（有意）"。任务 10b 把这条裁决改了：
#   **要无头浏览器不是"不接进来"的理由** —— 缺工具就大声失败并给三平台补救（W-2），
#   而不是让它变成"有人记得才跑"。逐条理由写在 `test.sh` 第 0.9 步的注释里。
#   ⚠️ 单独跑它仍然是**一条命令**（上面那几行）。
#
# ---------------------------------------------------------------------------
# 结果为什么走 POST（而不是 `--dump-dom`）
# ---------------------------------------------------------------------------
#   本仓库实测：这台机器上的 `Microsoft Edge 149` 在 `--headless=new --dump-dom` 下
#   **会挂住**（页面跑完了也不退出、也拿不到 stdout）。⇒ 夹具页把 `{ok, fail, lines}`
#   POST 回来，本脚本起的小 python 服务（`frontend-stub/serve.py`）把它写进一个文件，
#   本脚本轮询它。于是"跑完了没有""过了几条"都是脚本读得到的数。
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_files_screen.sh` / `check_frontend_copy.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 全过（每一轮都是 `fail=0` 且 `ok>0`）
#   1 = 环境问题（缺 python3/curl / 找不到无头浏览器 / 服务起不来）—— W-2：大声说 + 给补救
#   2 = 用法错误（不认识的参数）
#   3 = **判据没过**（有 FAIL，或**空跑**：没收到结果、或 `ok=0` —— 空跑绿灯判为失败）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STUB="$REPO/windows/scripts/frontend-stub"
# ⚠️ 端口与文件页那一条**不同**（8391）：两条验收可能同时在跑，撞端口的样子是
#    "服务起不来"——那会把人引到错的方向去查。
PORT="${BENAGEN_TRANSFERS_PORT:-8392}"
# 浏览器最多跑多久（秒）。`--all` 那一轮最慢（六个场景，每个都有 1 s 的 `state` 节拍要等）。
WAIT_SECONDS="${BENAGEN_TRANSFERS_WAIT:-180}"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

run_all=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --all) run_all=1 ;;
    *)
      echo "check_transfers_screen.sh: 不认识的参数：$1" >&2
      echo "check_transfers_screen.sh: 用法：bash windows/scripts/check_transfers_screen.sh [--all]" >&2
      exit 2
      ;;
  esac
  shift
done

# ---------------------------------------------------------------------------
# 工具预检（缺什么就大声说，并给出**可执行的**补救）
# ---------------------------------------------------------------------------
command -v python3 >/dev/null 2>&1 || die "PATH 里没有 \`python3\` —— 本脚本要用它起本地服务。
      补救（按你的平台挑一行）：
        macOS: 系统自带 python3
        Debian/Ubuntu: apt-get install -y python3
        Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`"

[ -f "$STUB/transfers-harness.html" ] || die "找不到 $STUB/transfers-harness.html（夹具页不在场）。"
[ -f "$STUB/wire-fixtures.json" ] || die "找不到 $STUB/wire-fixtures.json（载荷夹具不在场）。
      补救：bash windows/scripts/make_wire_fixtures.sh"

# 无头浏览器：按平台探测（顺序即优先级），也认环境变量覆盖。
browser=""
candidates=()
[ -n "${BENAGEN_BROWSER:-}" ] && candidates+=("$BENAGEN_BROWSER")
case "$(uname -s 2>/dev/null || echo unknown)" in
  Darwin)
    candidates+=("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge")
    candidates+=("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
    candidates+=("/Applications/Chromium.app/Contents/MacOS/Chromium")
    ;;
  *)
    candidates+=("microsoft-edge" "microsoft-edge-stable" "google-chrome" "google-chrome-stable" "chromium" "chromium-browser")
    ;;
esac
for c in "${candidates[@]}"; do
  if [ -x "$c" ]; then browser="$c"; break; fi          # 绝对路径
  if command -v "$c" >/dev/null 2>&1; then browser="$(command -v "$c")"; break; fi
done
# ⚠️ 宿主是 WKWebView、**交付是 WebView2（Chromium 内核）** ⇒ 只有 Chromium 系的
#    无头浏览器才与交付可比（这一条是任务 11 简报点名的）。
[ -n "$browser" ] || die "找不到无头浏览器（试过：${candidates[*]}）。
      ⚠️ 跳过的后果是\"这一步从来没跑过而没人知道\"，所以这里**直接失败**而不是跳过。
      补救（二选一）：
        · 装 Microsoft Edge 或 Google Chrome（本代交付跑在 WebView2 上，Edge 最贴近）；
        · 或用 BENAGEN_BROWSER=/path/to/chrome 显式指定。"

command -v curl >/dev/null 2>&1 || die "PATH 里没有 \`curl\`（本脚本用它等本地服务起来）。"

# ---------------------------------------------------------------------------
# 一次运行：起服务 → 跑浏览器 → 收结果 → 打印
# ---------------------------------------------------------------------------
# 参数：<场景> <临时根目录> <结果文件>
run_once() {
  local scenario="$1" root="$2" result="$3"
  rm -f "$result"

  # ---- 起服务（自己写的那个：GET 静态 + POST /__result）----------------------
  python3 "$root/serve.py" "$root" "$PORT" "$result" &
  local server_pid=$!
  # shellcheck disable=SC2064  # 现在就展开 $server_pid —— 那是故意的
  trap "kill $server_pid 2>/dev/null || true" RETURN

  local up=0
  for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:$PORT/windows/web/index.html" -o /dev/null 2>/dev/null; then up=1; break; fi
    sleep 0.2
  done
  [ "$up" = "1" ] || die "本地服务没起来（127.0.0.1:${PORT}）—— 判据无从判起，这不是\"通过\"。"

  # ---- 跑浏览器 ---------------------------------------------------------------
  # ⚠️ **不带 `--virtual-time-budget`**：虚拟时间会把页面里的定时器快进，而这一屏要验的
  #    恰恰是"**真实** 200 ms 一拍下 DOM 有没有被重建"——快进之后那件事就不是那件事了。
  #    也不带 `--screenshot`：结果走 POST，本脚本收到就杀掉浏览器（见文件头）。
  local profile="$root/profile-$scenario"
  local log="$root/browser-$scenario.log"
  "$browser" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
    --user-data-dir="$profile" --window-size=1120,720 \
    "http://127.0.0.1:$PORT/transfers-harness.html?s=$scenario" >"$log" 2>&1 &
  local browser_pid=$!

  # ---- 等结果（轮询那个文件）--------------------------------------------------
  local waited=0
  while [ ! -s "$result" ]; do
    if [ "$waited" -ge "$WAIT_SECONDS" ]; then break; fi
    sleep 1
    waited=$((waited + 1))
  done
  kill "$browser_pid" 2>/dev/null || true
  wait "$browser_pid" 2>/dev/null || true
  # ⚠️ 浏览器会拉一串子进程；主进程被杀之后它们可能还挂一会儿。
  #    按**本轮独有的 profile 目录**收一遍（这个 pattern 只可能匹配到本轮那几只）。
  pkill -f "$profile" 2>/dev/null || true
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true

  if [ ! -s "$result" ]; then
    # ⚠️ 超时**必须说得出话**：不然接手的人只有"它挂住了"这一条线索。
    echo "CHECK transfers:$scenario TIMEOUT (等了 ${WAIT_SECONDS}s 没收到结果)" >&2
    echo "      地址：http://127.0.0.1:${PORT}/transfers-harness.html?s=$scenario" >&2
    # ⚠️ `$log` 后面紧跟中文标点时**必须写成 `${log}`**：bash 会把多字节字符的首字节
    #    当成变量名的一部分 ⇒ `unbound variable`（`check_files_screen.sh` 踩过两次）。
    if [ -s "$log" ]; then
      echo "      浏览器最后 15 行（${log}）：" >&2
      tail -n 15 "$log" >&2
    else
      echo "      浏览器一个字都没输出（${log} 是空的）。" >&2
    fi
    return 3
  fi
  python3 - "$result" "$scenario" <<'PY'
import json, sys
path, scenario = sys.argv[1], sys.argv[2]
d = json.load(open(path, encoding="utf-8"))
ok, fail = int(d.get("ok", 0)), int(d.get("fail", 0))
aborted = int(d.get("aborted", 0))
# ⚠️ `abort=` 也要打出来：**"跑了几条"与"有没有跑完"是两件事**
#    （中止的那几条不计进 ok/fail，见夹具里 `abort` 那段）。
print(f"CHECK transfers:{scenario} ok={ok} fail={fail} abort={aborted}")
for line in d.get("lines", []):
    if line.startswith("FAIL ") or line.startswith("ABORT "):
        print("      " + line)
sys.exit(3 if (fail > 0 or ok == 0 or aborted > 0) else 0)
PY
}

# ---------------------------------------------------------------------------
# 造一个**临时服务根**：只放夹具页要的那几样东西
#   <root>/windows/web/**                              ← 被测的前端（可整体拷贝：很小）
#   <root>/windows/scripts/frontend-stub/wire-fixtures.json
# 这样跑的是**这一份源码**、而临时根里的东西改坏了也不碰真仓库。
# ---------------------------------------------------------------------------
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
root="$tmpdir/root"
mkdir -p "$root/windows/scripts/frontend-stub"
cp -R "$REPO/windows/web" "$root/windows/web"
cp "$STUB/wire-fixtures.json" "$root/windows/scripts/frontend-stub/wire-fixtures.json"
cp "$STUB/transfers-harness.html" "$root/transfers-harness.html"
cp "$STUB/serve.py" "$root/serve.py"

status=0
# ⚠️ `$PORT` 后面紧跟中文标点时**必须写成 `${PORT}`**（见上面那条纪律）。
note "跑 still（$(basename "$browser")，端口 ${PORT}）"
run_once "still" "$root" "$tmpdir/still.json" || status=3
if [ "$run_all" = "1" ]; then
  note "跑 tick（200 ms 一拍：只改文本与宽度 + 对照组）"
  run_once "tick" "$root" "$tmpdir/tick.json" || status=3
  note "跑 empty（空态）"
  run_once "empty" "$root" "$tmpdir/empty.json" || status=3
  # ⚠️ 这句里**不许出现反引号**：它在双引号里会被 bash 当成命令替换去执行
  #    （本脚本自己踩过一次：屏幕上打出 `empty_text: command not found`）。
  note "跑 down（载荷说不用说话：empty_text 是 null）"
  run_once "down" "$root" "$tmpdir/down.json" || status=3
  note "跑 gate（闸门关着 ⇒ 一拍都不发；刷新在场但禁用）"
  run_once "gate" "$root" "$tmpdir/gate.json" || status=3
  note "跑 loading（还没有快照 ⇒ 转圈 + 那一句话，且排在空态之前）"
  run_once "loading" "$root" "$tmpdir/loading.json" || status=3
fi

if [ "$status" = "0" ]; then
  note "判据通过（空跑也算不过 —— 见文件头的退出码那一段）"
else
  note "判据没过：上面有 FAIL（或某一轮没收到结果）。"
fi
exit "$status"
