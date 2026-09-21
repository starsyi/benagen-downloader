#!/usr/bin/env bash
#
# windows/scripts/check_files_screen.sh —— **文件页的无头验收，一条命令跑完**。
#
#    bash windows/scripts/check_files_screen.sh            # 跑当前这一版（sub 35 条 + blocked 9 条 + keep 35 条）
#    bash windows/scripts/check_files_screen.sh --all      # 再跑 fail / switch 两个场景
#    bash windows/scripts/check_files_screen.sh --ab       # A/B：当前版 vs 修前那一版，给两组数
#    bash windows/scripts/check_files_screen.sh --all --ab # 两个都要
#
# ⚠️ **三场常驻，各有各的"做没做长得不一样"的理由**（R-61：少了哪一场，那条判据就只剩读代码）：
#    · `sub`     —— 默认那一档载荷（四列 / 底栏 / 勾选 / 右键 / enqueue 回执）；
#    · `blocked` —— 超预算那一档（`action.blocked_reason` 是个字符串）：只有它在场，
#                   "那个动作按不下去时界面做了什么"才可观测（`sub` 那一档是 `null`）；
#    · `keep`    —— **换屏**那一档（R.a1b… 那一批：选一个文件 → 去传输列表看一眼 →
#                   切回来 → 点「下载选中」⇒ 下的是整批）。`sub` 从头到尾没换过屏 ⇒
#                   那条缺陷在那里与"做对了"一模一样；只有真的切走再切回来它才可观测。
#
# 它做的事（四步，全自动）：起本地静态服务 → 用无头浏览器加载夹具页 →
#   夹具页跑断言、把结果 **POST 回来** → 打印 `ok=N fail=M` 并据此定退出码。
#
# ---------------------------------------------------------------------------
# 它为什么存在（R-61：**"修好了"必须答得出"它现在被什么守着"**）
# ---------------------------------------------------------------------------
#   任务 10 那一屏的验收证据曾经是"**临时搭一次、截几张图**"：
#     · 夹具页没入库（`windows/web/` 下不能放 —— 会被编进 exe）⇒ **下一次没人能重跑**；
#     · "修前 17 PASS / 9 FAIL"那句话只能从报告里抄 ⇒ 它是**一次性的**，不是被守住的。
#   而任务 11/12/13 马上要复用这套东西：每次临时搭一遍，等于每次都要**重新论证它搭得对不对**
#   （本仓库吃过这个亏：搭得对不对没人核）。
#   ⇒ 夹具页（`frontend-stub/files-harness.html`）与**这条命令**一起入库：
#     **一条命令**给出"跑了几条、过了几条、红的是哪几条"，而期望值**全部来自
#     `frontend-stub/wire-fixtures.json`**（那份夹具由 `make_wire_fixtures.sh` 从 Rust 倒出来，
#     另有 `check_wire_fixtures.sh` 守着它不漂移）。
#
# ---------------------------------------------------------------------------
# ⚠️ 结果为什么走 POST（而不是 `--dump-dom`）
# ---------------------------------------------------------------------------
#   本仓库实测：这台机器上的 `Microsoft Edge 149` 在 `--headless=new --dump-dom` 下
#   **会挂住**（页面跑完了也不退出、也拿不到 stdout）。而 `--screenshot` 能退（会写出 PNG），
#   但截图里的数字**脚本读不到** —— 那就又回到"靠人看"。
#   ⇒ 本脚本起的是一个**自己写的**小 python 服务（`GET` 静态文件 + `POST /__result`），
#     夹具页把 `{ok, fail, lines}` POST 回来，服务**写进一个文件**，本脚本轮询它。
#     于是"跑完了没有""过了几条"都是脚本读得到的数。
#
# ---------------------------------------------------------------------------
# 🔴 它**自任务 10b 起接进了 `test.sh` 第 0.9 步**（与另外三份 runner 一起，串行跑）
# ---------------------------------------------------------------------------
#   原先这里写的是"它没有接进 `test.sh`（有意）"，理由是"它要一个无头浏览器"。
#   那条理由本身没错，错的结论：**"要额外工具"不是"不接进来"的理由，而是"接进来
#   并让缺工具大声失败"的理由**（W-2 禁的是静默跳过，不是要求工具链最小）。
#   裁决与代价记账写在 `test.sh` 第 0.9 步的注释里。
#   ⚠️ 单独跑它仍然是**一条命令**（上面那几行）——迭代这一屏时用它最快。
#
# ---------------------------------------------------------------------------
# 退出码
# ---------------------------------------------------------------------------
#   0 = 全过（每一轮都是 `fail=0` 且 `ok>0`）
#   1 = 环境问题（缺 python3 / 找不到无头浏览器 / 服务起不来）—— W-2：大声说 + 给补救
#   2 = 用法错误（不认识的参数）
#   3 = **判据没过**（有 FAIL，或**空跑**：没收到结果、或 `ok=0` —— 空跑绿灯判为失败）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STUB="$REPO/windows/scripts/frontend-stub"
PORT="${BENAGEN_FILES_PORT:-8391}"
# 浏览器最多跑多久（秒）。这台机器上 Edge 冷启动 + 跑完一轮大约 40 s，留足余量。
WAIT_SECONDS="${BENAGEN_FILES_WAIT:-120}"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

run_all=0
run_ab=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --all) run_all=1 ;;
    --ab) run_ab=1 ;;
    *)
      echo "check_files_screen.sh: 不认识的参数：$1" >&2
      echo "check_files_screen.sh: 用法：bash windows/scripts/check_files_screen.sh [--all] [--ab]" >&2
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

[ -f "$STUB/files-harness.html" ] || die "找不到 $STUB/files-harness.html（夹具页不在场）。"
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
[ -n "$browser" ] || die "找不到无头浏览器（试过：${candidates[*]}）。
      ⚠️ 跳过的后果是\"这一步从来没跑过而没人知道\"，所以这里**直接失败**而不是跳过。
      补救（二选一）：
        · 装 Microsoft Edge 或 Google Chrome（本代交付跑在 WebView2 上，Edge 最贴近）；
        · 或用 BENAGEN_BROWSER=/path/to/chrome 显式指定。"

command -v curl >/dev/null 2>&1 || die "PATH 里没有 \`curl\`（本脚本用它等本地服务起来）。"

# ---------------------------------------------------------------------------
# 一次运行：起服务 → 跑浏览器 → 收结果 → 打印
# ---------------------------------------------------------------------------
# 参数：<场景> <临时根目录> <结果文件> <可执行文件里的"哪一版">
run_once() {
  local scenario="$1" root="$2" result="$3" label="$4"
  rm -f "$result"

  # ---- 起服务（自己写的那个：GET 静态 + POST /__result）----------------------
  python3 "$root/serve.py" "$root" "$PORT" "$result" &
  local server_pid=$!
  # 无论怎么退出，都别把服务留下（它会把端口占着，下一次跑就红了）。
  # shellcheck disable=SC2064  # 现在就展开 $server_pid —— 那是故意的
  trap "kill $server_pid 2>/dev/null || true" RETURN

  local up=0
  for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:$PORT/windows/web/index.html" -o /dev/null 2>/dev/null; then up=1; break; fi
    sleep 0.2
  done
  [ "$up" = "1" ] || die "本地服务没起来（127.0.0.1:${PORT}）—— 判据无从判起，这不是\"通过\"。"

  # ---- 跑浏览器 ---------------------------------------------------------------
  # ⚠️ **不带 `--virtual-time-budget`、也不带 `--screenshot`**（都与本夹具的配合不好）：
  #    · 虚拟时间会把页面里的 `setTimeout` 快进，而本页**顶层还有一个 `await fetch`**
  #      （读夹具）—— 两者凑在一起时页面会卡住不往下走（实测：卡了 4 分钟一条都没跑）；
  #    · `--screenshot` 是"让 Chromium 自己退出"的办法，但那样就绑死在
  #      "必须在预算内跑完"上；而本脚本**本来就会在收到结果之后杀掉浏览器**
  #      （结果走 POST，见文件头）⇒ 不需要它退。
  #    ⇒ 页面按**真实时间**跑完（约 15 s），脚本轮询结果、拿到就收工。
  local profile="$root/profile-$scenario"
  local log="$root/browser-$scenario.log"
  "$browser" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
    --user-data-dir="$profile" --window-size=1120,720 \
    "http://127.0.0.1:$PORT/files-harness.html?s=$scenario" >"$log" 2>&1 &
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
  # 服务用完了就关（下一轮还要用同一个端口 —— 留着就是下一轮的红）。
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true

  if [ ! -s "$result" ]; then
    # ⚠️ 超时**必须说得出话**：不然接手的人只有"它挂住了"这一条线索
    #    （那正是"搭得对不对没人核"的形态）。把浏览器那侧的尾巴打出来。
    echo "CHECK files:$scenario:$label TIMEOUT (等了 ${WAIT_SECONDS}s 没收到结果)" >&2
    echo "      地址：http://127.0.0.1:${PORT}/files-harness.html?s=$scenario" >&2
    # ⚠️ `$log` 后面紧跟中文标点时**必须写成 `${log}`**：bash 会把多字节字符的首字节
    #    当成变量名的一部分 ⇒ `unbound variable`（本脚本自己踩过**两次** —— 所以
    #    这里留一条纪律：**凡 `$var` 后面接中文，一律写成 `${var}`**）。
    if [ -s "$log" ]; then
      echo "      浏览器最后 15 行（${log}）：" >&2
      tail -n 15 "$log" >&2
    else
      echo "      浏览器一个字都没输出（${log} 是空的）。" >&2
    fi
    return 3
  fi
  python3 - "$result" "$scenario" "$label" <<'PY'
import json, sys
path, scenario, label = sys.argv[1], sys.argv[2], sys.argv[3]
d = json.load(open(path, encoding="utf-8"))
ok, fail = int(d.get("ok", 0)), int(d.get("fail", 0))
aborted = int(d.get("aborted", 0))
# ⚠️ `abort=` 也要打出来：**"跑了几条"与"有没有跑完"是两件事** ——
#    修前那一版跑到第 26 条就因为"没进得去目录"抛了异常（见夹具里 `abort` 那段），
#    那一行不说出来的话，17 PASS 会被读成"修前只错了 9 条"。
print(f"CHECK files:{scenario}:{label} ok={ok} fail={fail} abort={aborted}")
for line in d.get("lines", []):
    if line.startswith("FAIL ") or line.startswith("ABORT "):
        print("      " + line)
sys.exit(3 if (fail > 0 or ok == 0 or aborted > 0) else 0)
PY
}

# ---------------------------------------------------------------------------
# 造一个**临时服务根**：只放夹具页要的两样东西
#   <root>/windows/web/**                              ← 被测的前端（可整体拷贝：很小）
#   <root>/windows/scripts/frontend-stub/wire-fixtures.json
# 这样 `--ab` 换掉 `files.js` 时**不会碰到真仓库**（只在副本里换）。
# ---------------------------------------------------------------------------
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
root="$tmpdir/root"
mkdir -p "$root/windows/scripts/frontend-stub"
cp -R "$REPO/windows/web" "$root/windows/web"
cp "$STUB/wire-fixtures.json" "$root/windows/scripts/frontend-stub/wire-fixtures.json"
cp "$STUB/files-harness.html" "$root/files-harness.html"
cp "$STUB/serve.py" "$root/serve.py"

status=0
# ⚠️ `$PORT` 后面紧跟中文标点时**必须写成 `${PORT}`**：bash 会把多字节字符的首字节
#    当成变量名的一部分 ⇒ `unbound variable`（本脚本自己踩过一次）。
note "跑当前这一版（$(basename "$browser")，端口 ${PORT}）"
run_once "sub" "$root" "$tmpdir/sub.json" current || status=3
# ⚠️ `blocked`（任务 17）**与 `sub` 一样常驻**（不在 `--all` 里面）：它验的是"勾选面大到
#    发不出去时那个动作**真的按不下去**"（载荷里的 `action.blocked_reason`），而 `sub`
#    那一档的 `blocked_reason` 是 `null` ⇒ **做没做在那里长得一模一样**（R-61）。
#    少跑这一轮，这条判据就只剩"读代码"这一条路。
run_once "blocked" "$root" "$tmpdir/blocked.json" current || status=3
# 🔴 `keep`（真机缺陷：**换屏把勾选面打回默认面**）**同样常驻**，理由与 `blocked` 一字不差：
#    它验的是"**切到「传输列表」再切回来**之后，用户攒的勾选面与所在的那一层还在不在"
#    —— 而那件事**只在真的换过一次屏之后**才可观测（`sub` 那一场从头到尾没换过屏，
#    做没做在那里长得一模一样）。真机上它的表现是：选一个文件、去看一眼传输列表、
#    切回来、点「下载选中」⇒ **下的是整批**（`selection == 全部文件` ⇒ 壳收敛成 `paths: []`）。
#    少跑这一轮，这条判据就只剩"读代码"这一条路（R-61）。
run_once "keep" "$root" "$tmpdir/keep.json" current || status=3
if [ "$run_all" = "1" ]; then
  run_once "fail" "$root" "$tmpdir/fail.json" current || status=3
  run_once "switch" "$root" "$tmpdir/switch.json" current || status=3
fi

# ---- A/B：把**修前那一版**（冻结在 frontend-stub/ 里）换进副本，再跑同一套 -------------
if [ "$run_ab" = "1" ]; then
  prefix="$STUB/files-prefix-ba859793.js"
  [ -f "$prefix" ] || die "找不到 ${prefix}（A/B 的对照基准不在场）—— 没有它就没法复现\"修前\"。"
  note "A/B：换入修前那一版（files-prefix-ba859793.js）再跑一遍同一套断言"
  cp "$root/windows/web/js/screens/files.js" "$tmpdir/files-current.js"
  cp "$prefix" "$root/windows/web/js/screens/files.js"
  run_once "sub" "$root" "$tmpdir/sub-prefix.json" prefix || true   # 红的**是预期的**
  cp "$tmpdir/files-current.js" "$root/windows/web/js/screens/files.js"
  echo "      （A/B 的对照：prefix 那一行**红是预期的** —— 它跑的是带着那两条 Critical 的那一版；" >&2
  echo "        两个数对不上时，先看上面那些 FAIL 行。）" >&2
fi

if [ "$status" = "0" ]; then
  note "判据通过（空跑也算不过 —— 见文件头的退出码那一段）"
else
  note "判据没过：上面有 FAIL（或某一轮没收到结果）。"
fi
exit "$status"
