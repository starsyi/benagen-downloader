#!/usr/bin/env bash
#
# windows/scripts/check_panels_screen.sh —— **任务 13 的无头验收，一条命令跑完**：
# 工具栏那颗下载按钮 + 换码面板 + 设置窗口 + 关于 + 许可全文
# + 🔴 **R-5 的对位差分**（夹具页里 ⑦ 那一组：把 Rust 倒出来的用例表回放一遍，
#   `NoteDrafts` / `SettingsForm::matches` / 开关面板的两条关闭路径，两边结论必须一致
#   —— 这是本代最严重那个缺陷（"判据在 Rust、实现被抄进 JS"）的守卫，
#   见 `windows/scripts/check_presentation_mirrors.sh` 与夹具页里那一段）。
#
#    bash windows/scripts/check_panels_screen.sh
#
# 它做的事（四步，全自动，与 `check_files_screen.sh` 同一条形状）：
#   起本地静态服务 → 用无头浏览器加载夹具页 → 夹具页跑断言、把结果 **POST 回来**
#   → 打印 `ok=N fail=M` 并据此定退出码。
#
# ---------------------------------------------------------------------------
# ⚠️ **无头 Chromium 才与交付可比**（规格 §9.0 第 3 条）
# ---------------------------------------------------------------------------
#   宿主（开发机）是 WKWebView，**交付**是 WebView2（Chromium 内核）。两者的排版与
#   DOM 行为并不完全一样 ⇒ 拿宿主上的 Safari/WebKit 去看这一套**不能**当作交付证据。
#   本脚本按平台探测 Edge（WebView2 同一个内核）→ Chrome → Chromium，正是这个理由。
#
# ---------------------------------------------------------------------------
# 🔴 它**自任务 10b 起接进了 `test.sh` 第 0.9 步**（同 `check_files_screen.sh` 的裁决）
# ---------------------------------------------------------------------------
#   原先这里写的是"它没接进 `test.sh`（有意）"。任务 10b 把这条裁决改了：
#   **要无头浏览器不是"不接进来"的理由** —— 缺工具就大声失败并给三平台补救（W-2）。
#   逐条理由写在 `test.sh` 第 0.9 步的注释里。
#   ⚠️ 单独跑它仍然是**一条命令**（上面那几行）。
#   ⚠️ **接进来的第一天它是红的**（任务 10b 实测）：`windows/web/index.html` 里
#      `tpl-screen-transfers-menu` 少了一个 `</template>` ⇒ 任务 13 那四块模板全都
#      嵌在它里面 ⇒ `document.getElementById` 看不见它们 ⇒ **四扇窗都打不开**
#      （换码面板 / 设置 / 关于 / 许可）。这条 runner 一直没人跑，所以没人知道。
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_files_screen.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 全过（`fail=0` 且 `ok>0` 且 `abort=0`）
#   1 = 环境问题（缺 python3 / 找不到无头浏览器 / 服务起不来）
#   2 = 用法错误（给了参数）
#   3 = **判据没过**（有 FAIL / 有 ABORT / **空跑**：没收到结果、或 `ok=0`）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
STUB="$REPO/windows/scripts/frontend-stub"
PORT="${BENAGEN_PANELS_PORT:-8397}"
WAIT_SECONDS="${BENAGEN_PANELS_WAIT:-180}"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# ---------------------------------------------------------------------------
# 参数：**一个都不认**（同 `check_files_screen.sh` 的纪律：透传一个打错的 flag 会让
# 脚本以一个"看起来像失败"的退出码收场，而根因是打错了字）
# ---------------------------------------------------------------------------
while [ "$#" -gt 0 ]; do
  echo "check_panels_screen.sh: 不认识的参数：$1" >&2
  echo "check_panels_screen.sh: 本脚本不接受任何参数（端口用 BENAGEN_PANELS_PORT 覆盖）。" >&2
  exit 2
done

command -v python3 >/dev/null 2>&1 || die "PATH 里没有 \`python3\` —— 本脚本要用它起本地服务。
      补救（按你的平台挑一行）：
        macOS: 系统自带 python3
        Debian/Ubuntu: apt-get install -y python3
        Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`"

[ -f "$STUB/panels-harness.html" ] || die "找不到 $STUB/panels-harness.html（夹具页不在场）。"
[ -f "$STUB/wire-fixtures.json" ] || die "找不到 $STUB/wire-fixtures.json（载荷夹具不在场）。
      补救：bash windows/scripts/make_wire_fixtures.sh"
[ -f "$STUB/serve.py" ] || die "找不到 $STUB/serve.py（本地服务不在场）。"

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
  if [ -x "$c" ]; then browser="$c"; break; fi
  if command -v "$c" >/dev/null 2>&1; then browser="$(command -v "$c")"; break; fi
done
[ -n "$browser" ] || die "找不到无头浏览器（试过：${candidates[*]}）。
      ⚠️ 跳过的后果是\"这一步从来没跑过而没人知道\"，所以这里**直接失败**而不是跳过。
      ⚠️ 宿主是 WKWebView、交付是 WebView2 ⇒ **无头 Chromium 才与交付可比**（优先 Edge）。
      补救（二选一）：
        · 装 Microsoft Edge 或 Google Chrome；
        · 或用 BENAGEN_BROWSER=/path/to/chrome 显式指定。"

command -v curl >/dev/null 2>&1 || die "PATH 里没有 \`curl\`（本脚本用它等本地服务起来）。"

# ---------------------------------------------------------------------------
# 造一个**临时服务根**：只放夹具页要的那几样
#   <root>/windows/web/**                              ← 被测的前端（可整体拷贝：很小）
#   <root>/windows/scripts/frontend-stub/wire-fixtures.json
#   <root>/core/assets/COPYING-GPLv2.txt               ← 许可全文（夹具**按真文件**倒载荷）
#   <root>/windows/assets/OFL-1.1.txt                  ← 同上
#   <root>/windows/assets/WebView2-SDK-LICENSE.txt     ← 同上（任务 15 加的第三份）
#   <root>/panels-harness.html  <root>/serve.py
# ---------------------------------------------------------------------------
# ⚠️ 三处许可全文是**承重的**：`panels-harness.html` 的 `licensePayload()` 用
#    `fetch` 把它们取回来当载荷的 `text`，并用 `crypto.subtle` 现算 sha256 ——
#    于是"全文逐字、不被截断"这条断言判的是**真的那三份文件**，而不是手抄的字符串。
#    （`name` / `why` 三条仍然写死在夹具里，见那份文件头那段记账。）
# ⚠️ **漏拷一份的表现是"断言拿着一个 404 的 HTML 当正文"** —— 实测（任务 15）：
#    第三份没拷进来，`fetch` 拿回目录列表页，而"'正文 == 那个文件' 逐字相同"那条断言
#    **照样绿**（两边都是同一份 HTML）。⇒ 承重的是**下面那条 `[ -f ] || die`**，
#    不是那句 `cp`（前者会让"少拷了一份"当场停住，后者只会安静地少一个文件）。
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
root="$tmpdir/root"
mkdir -p "$root/windows/scripts/frontend-stub" "$root/core/assets" "$root/windows/assets"
cp -R "$REPO/windows/web" "$root/windows/web"
cp "$STUB/wire-fixtures.json" "$root/windows/scripts/frontend-stub/wire-fixtures.json"
cp "$STUB/panels-harness.html" "$root/panels-harness.html"
cp "$STUB/serve.py" "$root/serve.py"
[ -f "$REPO/core/assets/COPYING-GPLv2.txt" ] || die "找不到 core/assets/COPYING-GPLv2.txt
      —— 夹具要按它现倒许可全文（那一份是分发给客户的那一份）。"
[ -f "$REPO/windows/assets/OFL-1.1.txt" ] || die "找不到 windows/assets/OFL-1.1.txt（同上）。"
[ -f "$REPO/windows/assets/WebView2-SDK-LICENSE.txt" ] || die "找不到 windows/assets/WebView2-SDK-LICENSE.txt
      —— 夹具要按它现倒许可全文（第三份：随交付物再分发的微软 loader 的 BSD-3）。
      ⚠️ 别把这句删掉换成'没有就少一份'：漏拷的表现是**断言拿 404 页当正文而照样绿**。"
cp "$REPO/core/assets/COPYING-GPLv2.txt" "$root/core/assets/COPYING-GPLv2.txt"
cp "$REPO/windows/assets/OFL-1.1.txt" "$root/windows/assets/OFL-1.1.txt"
cp "$REPO/windows/assets/WebView2-SDK-LICENSE.txt" "$root/windows/assets/WebView2-SDK-LICENSE.txt"

result="$tmpdir/result.json"
rm -f "$result"

python3 "$root/serve.py" "$root" "$PORT" "$result" &
server_pid=$!
# shellcheck disable=SC2064  # 现在就展开 $server_pid —— 那是故意的
trap "kill $server_pid 2>/dev/null || true; rm -rf '$tmpdir'" EXIT

up=0
for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:${PORT}/windows/web/index.html" -o /dev/null 2>/dev/null; then up=1; break; fi
  sleep 0.2
done
[ "$up" = "1" ] || die "本地服务没起来（127.0.0.1:${PORT}）—— 判据无从判起，这不是\"通过\"。"

# ---------------------------------------------------------------------------
# 🔴 **端口上回答的必须是我们刚起的那个服务**
# ---------------------------------------------------------------------------
# ⚠️ 这一段是一次**实测**逼出来的：默认端口上如果已经坐着**别人**的服务
#    （本机实测过一次：另一个进程占着端口），上面那个探测会**照样通过** ——
#    于是浏览器加载的是别人的页面，夹具一个字都不会回报，脚本最后以
#    "TIMEOUT" 收场。而 TIMEOUT 把读者的注意力引向"夹具卡住了"，
#    真正的根因（端口被占）一个字都不在输出里。
#    ⇒ 用**只有本任务的夹具才有**的那几格当指纹；对不上就当场指名道姓。
fixture_probe="$(curl -fsS "http://127.0.0.1:${PORT}/windows/scripts/frontend-stub/wire-fixtures.json" 2>/dev/null || true)"
case "$fixture_probe" in
  *preferencesSetFailed*) : ;;
  *) die "127.0.0.1:${PORT} 上回答的**不是**本脚本刚起的那个服务（夹具指纹对不上）。
      最可能的原因：这个端口上已经坐着别人的东西。
      补救：用 BENAGEN_PANELS_PORT=<一个空闲端口> bash windows/scripts/check_panels_screen.sh
      服务进程（pid ${server_pid}）还活着吗：$(kill -0 "$server_pid" 2>/dev/null && echo 是 || echo 不是)
      ⚠️ 这不是\"夹具卡住了\" —— 别去改夹具，先换端口。" ;;
esac

note "跑任务 13 那一套（$(basename "$browser")，端口 ${PORT}）"
profile="$tmpdir/profile"
log="$tmpdir/browser.log"
# ⚠️ 不带 `--virtual-time-budget`：本页顶层有 `await fetch`，两者凑在一起会卡住
#    （`check_files_screen.sh` 头部记着实测）。页面按**真实时间**跑完，脚本轮询结果。
"$browser" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
  --user-data-dir="$profile" --window-size=1120,720 \
  "http://127.0.0.1:${PORT}/panels-harness.html" >"$log" 2>&1 &
browser_pid=$!

waited=0
while [ ! -s "$result" ]; do
  if [ "$waited" -ge "$WAIT_SECONDS" ]; then break; fi
  sleep 1
  waited=$((waited + 1))
done
kill "$browser_pid" 2>/dev/null || true
wait "$browser_pid" 2>/dev/null || true
pkill -f "$profile" 2>/dev/null || true
kill "$server_pid" 2>/dev/null || true
wait "$server_pid" 2>/dev/null || true

if [ ! -s "$result" ]; then
  # ⚠️ 超时**必须说得出话**（同 `check_files_screen.sh`）：不然接手的人只有"它挂住了"。
  # ⚠️ `$log` 后面紧跟中文标点时**必须写成 `${log}`**：bash 会把多字节字符的首字节
  #    当成变量名的一部分 ⇒ `unbound variable`（本仓库踩过两次，记在这里）。
  echo "CHECK panels TIMEOUT (等了 ${WAIT_SECONDS}s 没收到结果)" >&2
  echo "      地址：http://127.0.0.1:${PORT}/panels-harness.html" >&2
  if [ -s "$log" ]; then
    echo "      浏览器最后 15 行（${log}）：" >&2
    tail -n 15 "$log" >&2
  else
    echo "      浏览器一个字都没输出（${log} 是空的）。" >&2
  fi
  exit 3
fi

python3 - "$result" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
ok, fail = int(d.get("ok", 0)), int(d.get("fail", 0))
aborted = int(d.get("aborted", 0))
print(f"CHECK panels current ok={ok} fail={fail} abort={aborted}")
for line in d.get("lines", []):
    if line.startswith("FAIL ") or line.startswith("ABORT "):
        print("      " + line)
sys.exit(3 if (fail > 0 or ok == 0 or aborted > 0) else 0)
PY
status=$?

if [ "$status" = "0" ]; then
  note "判据通过（空跑也算不过 —— 见文件头的退出码那一段）"
else
  note "判据没过：上面有 FAIL（或某一轮没收到结果）。"
fi
exit "$status"
