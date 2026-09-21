#!/usr/bin/env bash
#
# windows/scripts/check_frontend_stub.sh —— 跑**常驻夹具** `frontend-stub/stub.html`。
#
#    bash windows/scripts/check_frontend_stub.sh
#
# ---------------------------------------------------------------------------
# 它是什么（**一条命令**，不再是"每次临时搭一遍"）
# ---------------------------------------------------------------------------
#   任务 9 的修复轮临时搭过一个**受控 stub**（iframe + 事后注入 `window.__TAURI__`、
#   跑真 `index.html`、同一套模块、同一条轮询），三种拒绝形状各验了一遍 —— 然后
#   **用完就删了**（`windows/web/` 下任何文件都会被 `frontendDist` 编进 exe）。
#   任务 10/11/12/13 要验的**正是同一件事**（"失败原文真的透出去了吗"、
#   "载荷变了界面跟着变吗"）⇒ 每次临时搭一遍是重复劳动，而且**搭得对不对没人核**。
#   ⇒ 夹具收成常驻的（`frontend-stub/stub.html`），跑法就是上面那一行。
#
#   ⚠️ 夹具的**位置**是承重的：本目录（`windows/scripts/`）**不在** frontendDist 里，
#      所以它不会被编进用户拿到的 exe。那件事由 `check_frontend_copy.sh` 的
#      文件清单判据守着（`windows/web/` 下出现未登记的文件 ⇒ 红）。
#
#   🔴 **它自任务 10b 起接进了 `test.sh` 第 0.9 步**（连同 `frontend-stub/` 下的三份
#      整屏 runner）。在此之前那一段写的是"它不在 test.sh 里，这是刻意的" ——
#      而那条"刻意"的实测代价是：本文件里的 I-3b **自任务 10 起就是红的**，
#      红了四个任务**没有一个人看见**，因为跑它的入口根本不存在（R-61/R-63）。
#      ⇒ 裁决改为：**接进去 + 缺浏览器就大声失败**，不做条件跳过（W-2）。
#      `test.sh` 因此多了一条硬前置（一个 Chromium 内核的浏览器），两个声明前置
#      工具的文档也一起改了（`docs/superpowers/2026-09-19-windows-dev-setup.md`、
#      `windows/README.md`）。
#   ⚠️ 单独跑它仍然是**一条命令**（上面那行），改动 `js/` 下与错误原文/载荷渲染
#      有关的代码时先跑它最快。
#
# ---------------------------------------------------------------------------
# 退出码
# ---------------------------------------------------------------------------
#   0 = 夹具全过（`DONE ok=N fail=0`，且 N > 0）
#   1 = 环境问题（缺 python3 / 找不到无头浏览器 / 本地服务起不来）—— W-2：大声说 + 给补救
#   3 = **判据没过**（夹具报了 FAIL，或**空跑**：没有 DONE 行、或 ok=0）
#       ⚠️ "空跑绿灯判为失败" 与 `test.sh` 第 3 步同一条纪律：
#          夹具没跑起来时不会有人报错，只会静悄悄地少验几件事。
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STUB_REL="windows/scripts/frontend-stub/stub.html"
PORT="${BENAGEN_STUB_PORT:-8377}"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# ---------------------------------------------------------------------------
# 工具预检（缺什么就大声说，并给出**可执行的**补救）
# ---------------------------------------------------------------------------
command -v python3 >/dev/null 2>&1 || die "PATH 里没有 \`python3\` —— 本脚本要用它起本地服务。
      补救（按你的平台挑一行）：
        macOS: 系统自带 python3
        Debian/Ubuntu: apt-get install -y python3
        Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`"

# **无头浏览器**：按平台探测（顺序即优先级），也认环境变量覆盖。
# ⚠️ 找不到时**不许静默跳过**：跳过的后果是"这一步从来没跑过，而没人知道"。
browser=""
candidates=()
if [ -n "${BENAGEN_BROWSER:-}" ]; then
  candidates+=("$BENAGEN_BROWSER")
fi
case "$(uname -s 2>/dev/null || echo unknown)" in
  Darwin)
    candidates+=("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge")
    candidates+=("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
    candidates+=("/Applications/Chromium.app/Contents/MacOS/Chromium")
    ;;
esac
for t in msedge google-chrome google-chrome-stable chromium chromium-browser; do
  if command -v "$t" >/dev/null 2>&1; then
    candidates+=("$(command -v "$t")")
  fi
done
# Windows 上（Git Bash / MSYS2）Edge 的常见落点 —— 本仓库要搬到那台机器上继续开发。
for p in "/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe" \
         "/c/Program Files/Microsoft/Edge/Application/msedge.exe"; do
  [ -x "$p" ] && candidates+=("$p")
done
for c in ${candidates[@]+"${candidates[@]}"}; do
  if [ -x "$c" ]; then browser="$c"; break; fi
done
if [ -z "$browser" ]; then
  {
    echo "错误：找不到一个可用的无头浏览器（Chromium 内核：Edge / Chrome / Chromium）。"
    echo "      本夹具要一个真浏览器来跑真 \`index.html\`（Tauri 的 webview 是 WebView2，"
    echo "      也就是 Chromium 内核）——**没有它就等于这条判据从来没有跑过**，"
    echo "      所以这里直接失败，不跳过（W-2）。"
    echo "      补救（挑一条）："
    echo "        macOS: 装 Microsoft Edge 或 Google Chrome（放到 /Applications 下即可）"
    echo "        Linux: apt-get install -y chromium   （或 chromium-browser）"
    echo "        Windows: 装 Edge（系统自带）"
    echo "      或显式指定：BENAGEN_BROWSER=/path/to/浏览器 bash windows/scripts/check_frontend_stub.sh"
  } >&2
  exit 1
fi
note "浏览器：$browser"

[ -f "$REPO/$STUB_REL" ] || die "找不到夹具本体：$REPO/$STUB_REL"

WORK="$(mktemp -d)"
server_pid=""
browser_pid=""
cleanup() {
  # ⚠️ **浏览器也要收**：它在 `--dump-dom` 输出完之后**不保证自己退出**（见下面那段），
  #    而且它跑在自己的 `--user-data-dir` 下 ⇒ 不收的话每跑一次就留一棵进程树。
  if [ -n "$browser_pid" ]; then
    kill "$browser_pid" 2>/dev/null || true
  fi
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 本地服务（**必需**：`file://` 下 ES 模块与字体都会被 CORS 拦掉）
# ---------------------------------------------------------------------------
# ⚠️ 服务根是**仓库根**：夹具按 `/windows/web/index.html` 找真页面（同任务 9 的手法）。
# ⚠️ 端口默认 8377（不常用，免得撞上开发时随手起的 8000/8123）；被占就换
#    `BENAGEN_STUB_PORT`，**不许**静默换一个端口 —— 那会让"连错服务"变成一个看不见的失败。
( cd "$REPO" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 ) \
  > "$WORK/http.log" 2>&1 &
server_pid=$!

url="http://127.0.0.1:${PORT}/${STUB_REL}"
if ! python3 - "$url" <<'PY'
import sys, time, urllib.request
url = sys.argv[1]
for _ in range(60):
    try:
        urllib.request.urlopen(url, timeout=1).read(1)
        sys.exit(0)
    except Exception:
        time.sleep(0.25)
sys.exit(1)
PY
then
  echo "错误：本地服务没起来（${url}）。" >&2
  echo "      可能是端口 ${PORT} 被别的进程占着。补救：换一个端口重跑，" >&2
  echo "      BENAGEN_STUB_PORT=8391 bash windows/scripts/check_frontend_stub.sh" >&2
  sed 's/^/      http.log: /' "$WORK/http.log" >&2 || true
  exit 1
fi
note "本地服务：$url"

# ---------------------------------------------------------------------------
# 跑夹具：无头浏览器 + `--dump-dom`（结论在 `<pre id="out">` 里）
# ---------------------------------------------------------------------------
# ⚠️ `--virtual-time-budget` 让定时器**立刻**推进：夹具里那些 `sleep(1400)` 因此
#    不用真的等 1.4 秒（任务 9 的同一手法）。预算给足（它只管上限）。
# ⚠️ `--user-data-dir` 指到临时目录：不碰任何人的真实浏览器配置，跑起来也互相独立。
# 🔴 **预算从 40000 提到 150000**（2026-09-20 的修复轮）：F-1/F-3/F-4/F-5 那一段又加了
#    几十拍（换分区要等下一拍 `state()`、报数再等一拍），夹具自己排的虚拟时间已经
#    超过 40 s ⇒ 预算不够时页面**在写到一半就被停住**，runner 只看到"没有 DONE 行"
#    （那正是"空跑"那条判据要抓的形态 —— 它会红，且红得对）。虚拟时间是**立刻推进**的，
#    所以调大它只多花**真实**时间里的一点点。
note "无头渲染（virtual-time-budget=150000）"
# ⚠️⚠️ **必须放后台跑 + 自己收尾**（实测，2026-09-20）：Edge 149 在这种调用下
#    **`--dump-dom` 写完之后不会自行退出**，前台跑会把这个夹具**挂死** ——
#    而挂死比失败更坏：它什么都不说，跑的人只能猜（本仓库最恨的形态之一）。
#    ⇒ 等的是"**dump 写完了吗**"（以 `</html>` 收尾为准），不是"进程退了吗"。
"$browser" --headless=new --disable-gpu --no-sandbox \
  --no-first-run --no-default-browser-check \
  --user-data-dir="$WORK/profile" \
  --virtual-time-budget=150000 \
  --dump-dom "$url" > "$WORK/dom.html" 2> "$WORK/browser.err" &
browser_pid=$!

dom_ready=0
for _ in $(seq 1 180); do          # 最多等 90 秒
  if grep -q "</html>" "$WORK/dom.html" 2>/dev/null; then
    dom_ready=1
    break
  fi
  if ! kill -0 "$browser_pid" 2>/dev/null; then
    break                          # 它自己退了（也行，下面按文件内容判）
  fi
  sleep 0.5
done
if kill -0 "$browser_pid" 2>/dev/null; then
  kill "$browser_pid" 2>/dev/null || true
  wait "$browser_pid" 2>/dev/null || true
fi
if [ "$dom_ready" -eq 1 ]; then
  note "dump 完成（浏览器不会自己退出，已收掉）"
else
  note "⚠️ 没等到完整的 dump（浏览器提前退出，或 90 秒超时）—— 下面按已有的内容判，别把这一段读成\"通过\""
fi

if [ ! -s "$WORK/dom.html" ]; then
  {
    echo "错误：无头浏览器没有输出任何 DOM（${browser}）。"
    echo "      补救：手工跑一次看它的报错 ——"
    echo "        \"$browser\" --headless=new --dump-dom \"$url\""
  } >&2
  sed 's/^/      browser.err: /' "$WORK/browser.err" >&2 || true
  exit 1
fi

# 结论：把 `<pre id="out">` 的文本取出来（**原样打印**，再去判 DONE 那一行）。
set +e
python3 - "$WORK/dom.html" <<'PY'
import html
import re
import sys

dom = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<pre id="out">(.*?)</pre>', dom, re.S)
if m is None:
    print("错误：dump 出来的 DOM 里没有 <pre id=\"out\"> —— 夹具根本没跑到写结论那一步。", file=sys.stderr)
    sys.exit(1)
# `--dump-dom` 会给 `<`/`>`/`&` 转义；还原成原文再打印（读的人要看到真的那句话）。
report = html.unescape(m.group(1))
print(report.rstrip("\n"))
m2 = re.search(r"^DONE ok=(\d+) fail=(\d+)$", report, re.M)
if m2 is None:
    # ⚠️ 这里的反引号**不加反斜杠**：普通字符串里的 `\`` 是一次无效转义
    #    ⇒ `SyntaxWarning`（本仓库是**零告警**口径，一条告警就该红；
    #    同 `check_design_fonts.py:blank_comments` 那段说明）。
    print("错误：结论里没有 `DONE ok=N fail=M` 那一行 —— 夹具没跑完（超时？脚本语法错？）。", file=sys.stderr)
    print("      这不是\"通过\"：空跑出来的绿灯与没有判据等价（同 test.sh 第 3 步）。", file=sys.stderr)
    sys.exit(1)
ok, fail = int(m2.group(1)), int(m2.group(2))
if ok == 0:
    print("错误：夹具一条都没验到（ok=0）—— 判为失败（空跑绿灯）。", file=sys.stderr)
    sys.exit(1)
if fail != 0:
    print(f"错误：夹具报了 {fail} 条 FAIL（见上面逐条的输出）。", file=sys.stderr)
    sys.exit(1)
print(f"==> 受控 stub 全过：ok={ok} fail=0", file=sys.stderr)
PY
rc=$?
set -e
exit "$rc"
