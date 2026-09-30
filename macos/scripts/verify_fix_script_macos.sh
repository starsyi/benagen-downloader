#!/usr/bin/env bash
# 修复脚本取证：证明 `已损坏修复.command` **真的把隔离属性清掉了**。
#
# 为什么需要它：这个脚本的全部价值就在「客户双击之后，那台机器上的属性真的没了」。
# 这件事没有任何编译期或静态检查覆盖得了 —— 改坏了不会当场红，要等到下一个客户
# 在盘里双击它。所以按本仓库一贯的纪律：**跑完看结果**，并把读数留成可复跑的脚本。
#
# ⚠️ **本脚本验不了的那一跳**（别把它当成「全验过了」）：
#    「属性清掉之后，Gatekeeper 真的放行」—— 那要求取证机上 Gatekeeper 是**开着**的，
#    而开发机/取证机上它多半是关的（实测 `spctl --status` 报 `assessments disabled`；
#    参考 DMG 里那个第三方修复脚本带着 `spctl --master-disable`，正是这个后果）。
#    本脚本能证明的是三件**必要条件**：
#      · 属性确实从目标上清干净了（**复查**，不是「命令没报错」）
#      · 应用确实落到了目标目录，且结构完整（Info.plist 与可执行位都在）
#      · 启动命令确实成功返回，且应用真的跑起来了
#    最后一跳只能在客户的机器上算数。这句话同样写在修复脚本自己的文件头里。
#
# ⚠️ **全程不碰真的 `/Applications`**：用 `BENAGEN_APP_DEST` 把目标指到临时目录，
#    跑完连临时目录一起删。修复脚本留那个环境变量，主要就是为了这件事。
#
# ⚠️ 路径二/三依赖本机状态：它们要走到 `/Volumes` 兜底搜索，而**本机挂着别的
#    Benagen 盘时那条路会先命中别的盘**。脚本检测到就**如实跳过**，不假装跑过。
#
# 用法：bash macos/scripts/verify_fix_script_macos.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXER="$REPO/macos/Resources/已损坏修复.command"
APP_NAME="BenagenDownloader.app"

if [ ! -x "$FIXER" ]; then
  echo "错误：找不到可执行的 $FIXER" >&2
  exit 2
fi

PASS=0; FAIL=0; SKIP=0
ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip() { printf '  \033[33m–\033[0m %s（跳过：%s）\n' "$1" "$2"; SKIP=$((SKIP + 1)); }

# `xattr -r` 只列属性名、不列值 —— 够用了：我们只关心这个属性还在不在。
has_quarantine() { xattr -r "$1" 2>/dev/null | grep -q 'com\.apple\.quarantine'; }

# 本机有没有别的 Benagen 盘挂着 —— 路径二/三能不能跑，全看这一条。
other_volumes() { ls -d /Volumes/*/"$APP_NAME" 2>/dev/null || true; }

T="$(mktemp -d)"
MNT="$T/mnt"
# 私有挂载点**故意不放在 /Volumes 下**：被测脚本优先看「自己旁边」，
# 所以路径一照样走得通，同时与用户真挂着的盘互不打扰。
cleanup() {
  hdiutil detach "$MNT" >/dev/null 2>&1 || true
  rm -rf "$T"
}
trap cleanup EXIT

echo '==> 造一个「从浏览器下载来的」应用（自带 com.apple.quarantine）'
STAGE="$T/stage"
mkdir -p "$STAGE/$APP_NAME/Contents/MacOS"
cat > "$STAGE/$APP_NAME/Contents/MacOS/BenagenDownloader" <<EOF
#!/bin/bash
echo launched > "$T/launched.txt"
EOF
chmod +x "$STAGE/$APP_NAME/Contents/MacOS/BenagenDownloader"
cat > "$STAGE/$APP_NAME/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>BenagenDownloader</string>
<key>CFBundleIdentifier</key><string>verify.fake.benagen</string>
<key>CFBundleName</key><string>BenagenDownloader</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
# 隔离属性是**烘进镜像**的（挂在只读卷上改不了它 —— 这正是修复脚本必须先拷出来的原因）。
# 所以先给 stage 里的 app 打属性、再打包，形态与参考 DMG 里那份一致。
xattr -w com.apple.quarantine "0181;$(printf '%x' "$(date +%s)");Chrome;" "$STAGE/$APP_NAME"
cp "$FIXER" "$STAGE/"
mkdir -p "$MNT" "$T/dest-a" "$T/dest-b" "$T/dest-c" "$T/elsewhere"

hdiutil create -volname BenagenFixVerify -srcfolder "$STAGE" -ov -format UDZO "$T/test.dmg" >/dev/null
hdiutil attach "$T/test.dmg" -nobrowse -readonly -mountpoint "$MNT" >/dev/null

if ! has_quarantine "$MNT/$APP_NAME"; then
  echo '错误：造出来的盘里那份 app 没有隔离属性 —— 取证装置本身坏了。' >&2
  exit 2
fi

printf '\n== 路径一：脚本与 app 同在安装盘里（正常用法）\n'
BENAGEN_APP_DEST="$T/dest-a" /bin/bash "$MNT/已损坏修复.command" < /dev/null > "$T/log-a.txt" 2>&1 || true

if [ -d "$T/dest-a/$APP_NAME" ]; then
  ok '应用被装到了目标目录'
else
  bad '目标目录里没有应用 —— 脚本没走到安装那一步'
fi
if [ -f "$T/dest-a/$APP_NAME/Contents/Info.plist" ] \
   && [ -x "$T/dest-a/$APP_NAME/Contents/MacOS/BenagenDownloader" ]; then
  ok '包结构完整（Info.plist 与可执行位都在 —— 这条抓的是「用 cp -R 拷坏了 bundle」）'
else
  bad '包结构不完整 —— 拷贝方式有问题（ditto 才保得住 app bundle）'
fi
if has_quarantine "$T/dest-a/$APP_NAME"; then
  bad '目标上**仍有**隔离属性 —— 这正是客户会再被拦一次的原因'
else
  ok '目标上的隔离属性已清干净（**复查**，不是「命令没报错」）'
fi
# ⚠️ 这里**必须等**：`open` 把启动交给 LaunchServices 就返回了，假 app 写标记文件
#    在它之后。早先直接读会稳定地读空 —— 那不是"没启动"，是取证脚本自己的竞态。
LAUNCHED=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [ -f "$T/launched.txt" ] && { LAUNCHED=yes; break; }
  sleep 0.5
done
if [ -n "$LAUNCHED" ]; then
  ok '应用真跑起来了（假 app 写的标记文件在）'
else
  bad '应用没有被启动（等了 5 秒仍没有标记文件）—— 见上面脚本的输出'
fi

printf '\n== 路径二：安装盘不在，只修已经装好的那一份\n'
if [ -n "$(other_volumes)" ]; then
  skip '就地修复已安装的那份' '本机挂着别的 Benagen 盘，兜底搜索会先命中它'
else
  cp -R "$T/dest-a/$APP_NAME" "$T/dest-b/$APP_NAME"
  xattr -w com.apple.quarantine "0181;$(printf '%x' "$(date +%s)");Chrome;" "$T/dest-b/$APP_NAME"
  cp "$FIXER" "$T/elsewhere/"          # 脚本孤零零放在一个没有 app 的目录里
  BENAGEN_APP_DEST="$T/dest-b" /bin/bash "$T/elsewhere/已损坏修复.command" < /dev/null > "$T/log-b.txt" 2>&1 || true
  if has_quarantine "$T/dest-b/$APP_NAME"; then
    bad '盘不在时没能就地修好已经装好的那一份'
  else
    ok '盘不在时，就地把已装那份的隔离属性清掉了'
  fi
fi

printf '\n== 路径三：哪儿都找不到 → 必须**明确报错退出**，不是静默成功\n'
if [ -n "$(other_volumes)" ]; then
  skip '找不到就报错' '同上，本机挂着别的 Benagen 盘'
else
  cp "$FIXER" "$T/elsewhere/" 2>/dev/null || true
  if BENAGEN_APP_DEST="$T/dest-c" /bin/bash "$T/elsewhere/已损坏修复.command" < /dev/null > "$T/log-c.txt" 2>&1; then
    bad '什么都找不到时**退出码仍是 0** —— 客户会以为修好了'
  else
    ok '找不到时报了错并以非 0 退出'
  fi
  if grep -q '找不到' "$T/log-c.txt"; then
    ok '报错信息里说清了「找不到」，不是一句无头无尾的失败'
  else
    bad '报错信息看不出是「找不到应用」'
  fi
fi

# `spctl --status` 在 Gatekeeper 关着时**退出码非 0**，所以不能直接 `|| echo 读不出`
# ——那会把两个值都打出来（实测就是这么错的）。先取输出，再看它空不空。
SPCTL="$(spctl --status 2>&1 || true)"
[ -n "$SPCTL" ] || SPCTL='读不出'
printf '\n== 边界（本脚本**证明不了**的）\n'
printf '  · Gatekeeper 放行本身：要求取证机上 Gatekeeper 开着，而本机报的是「%s」\n' "$SPCTL"
printf '  · 客户真机上的那一下双击：本脚本用 `bash <路径>` 调用，不是访达里的双击\n'

printf '\n== 读数：通过 %d / 失败 %d / 跳过 %d\n' "$PASS" "$FAIL" "$SKIP"
[ "$FAIL" -eq 0 ] || exit 1
