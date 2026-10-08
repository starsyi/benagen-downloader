#!/usr/bin/env bash
#
# windows/scripts/check_big_screen.sh —— **「文件多了就卡死」的复现/回归夹具**。
#
#    bash windows/scripts/check_big_screen.sh            # 默认 3000 行
#    bash windows/scripts/check_big_screen.sh 8000       # 换一个规模
#
# 它起本地静态服务 → 用无头浏览器加载 `frontend-stub/big-harness.html`（**真 `index.html`
# + 真模块**，喂 N 行）→ 页面把每一步的**耗时**POST 回来 → 这里打印。
#
# ⚠️ 浏览器探测、结果通道（POST `/__result`）与 `check_files_screen.sh` 同一套手法与同样的
#    理由（本机 Edge 在 `--dump-dom` 下会挂住 ⇒ 数字必须从浏览器里寄出来）。
#    两份脚本**故意不共用一个库**：这一份是调查/回归用的旁路，改动它不该碰到验收那条主路。
set -euo pipefail

N="${1:-3000}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; ROOT="$(cd "$ROOT/.." && pwd)"
STUB="$ROOT/windows/scripts/frontend-stub"
PORT="${BENAGEN_BIG_PORT:-8394}"
RESULT="$(mktemp -t bigscreen.XXXXXX)"
rm -f "$RESULT"

die() { echo "错误：$*" >&2; exit 1; }

[ -f "$STUB/wire-fixtures.json" ] || die "找不到 $STUB/wire-fixtures.json"
[ -f "$STUB/big-harness.html" ] || die "找不到 $STUB/big-harness.html"

browser=""
candidates=()
[ -n "${BENAGEN_BROWSER:-}" ] && candidates+=("$BENAGEN_BROWSER")
case "$(uname -s 2>/dev/null || echo unknown)" in
  Darwin)
    candidates+=("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge")
    candidates+=("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
    candidates+=("/Applications/Chromium.app/Contents/MacOS/Chromium")
    ;;
  *) candidates+=("microsoft-edge" "google-chrome" "chromium") ;;
esac
for c in "${candidates[@]}"; do
  if [ -x "$c" ]; then browser="$c"; break; fi
  if command -v "$c" >/dev/null 2>&1; then browser="$(command -v "$c")"; break; fi
done
[ -n "$browser" ] || die "找不到无头浏览器（试过：${candidates[*]}）"

python3 "$STUB/serve.py" "$ROOT" "$PORT" "$RESULT" &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$PORT/windows/web/index.html" -o /dev/null 2>/dev/null && break
  sleep 0.2
done

profile="$(mktemp -d -t bigprofile.XXXXXX)"
log="$(mktemp -t bigbrowser.XXXXXX)"
echo "==> 行数 ${N}，浏览器 $(basename "$browser")，端口 ${PORT}"
"$browser" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
  --user-data-dir="$profile" --window-size=1120,720 \
  "http://127.0.0.1:$PORT/windows/scripts/frontend-stub/big-harness.html?n=$N" >"$log" 2>&1 &
browser_pid=$!

waited=0
while [ ! -s "$RESULT" ]; do
  [ "$waited" -ge 150 ] && break
  sleep 1
  waited=$((waited + 1))
done

kill "$browser_pid" 2>/dev/null || true
wait "$browser_pid" 2>/dev/null || true
pkill -f "$profile" 2>/dev/null || true
rm -rf "$profile" "$log"

if [ ! -s "$RESULT" ]; then
  echo "超时：${waited}s 内浏览器没有回结果 —— 这本身就是「卡死」的一个读数。" >&2
  echo "（页面里有 90 s 看门狗；连它都没送出东西，说明主线程真的被堵住了。）" >&2
  exit 1
fi

python3 - "$RESULT" <<'PY'
import json, sys
raw = open(sys.argv[1], encoding="utf-8").read()
try:
    d = json.loads(raw)
except Exception:
    print("结果不是 JSON：", raw[:400]); sys.exit(1)
print(f"---- 夹具回报（n={d.get('n')}，tag={d.get('tag')}）----")
for line in d.get("lines", []):
    print("  " + line)
ok, fail = d.get("ok", 0), d.get("fail", 0)
print(f"CHECK big:n={d.get('n')} ok={ok} fail={fail}")
# ⚠️ 空跑也算不过（同 `test.sh` 与 `check_files_screen.sh` 的口径）：
#    一条都没跑 ⇒ 这里不是"通过"，是"没跑"。
sys.exit(1 if (fail or ok == 0) else 0)
PY
rc=$?
rm -f "$RESULT"
exit "$rc"
