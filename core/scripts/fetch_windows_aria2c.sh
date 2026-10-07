#!/usr/bin/env bash
# 取官方 Windows 版 aria2c 并入 core/assets/，并把来源登记打印出来。
#
# ⚠️ **这是对 macOS 那套纪律的一处有意偏离**（规格 §6.4）：macOS 那份是
#    **从固定 sha256 的源码包自己编**的（downloader/scripts/build_aria2_macos.sh）；
#    Windows 这份是**官方 prebuilt**。
#    理由：自己交叉编译需要 gmp/expat/sqlite/zlib/c-ares/libssh2 六套库的 mingw 版本
#    （官方 README.mingw 自述），而产物性质与官方包没有可见差别——官方包同样
#    **只链系统 DLL**、同样走**原生 TLS（Schannel，对应 macOS 的 AppleTLS）**。
#    代价：可复现性从"配方可重现"降到"二进制 sha256 可核对"。
#    ⇒ 补偿就是本脚本：**来源 URL + 版本 + 包 sha256 + 取出文件自己的 sha256** 全部
#      打印并**当场核对**（下面四个常量就是那份登记），改任何一处都能一条命令复核。
#
# 这里是那四项登记（唯一真相，改版本时连同 README 的引用一起改）：
#
#   来源 URL     https://api.github.com/repos/aria2/aria2/releases/tags/release-1.37.0
#   版本         1.37.0（与 macOS 那份同版本号）
#   资产包       aria2-1.37.0-win-64bit-build1.zip   sha256 67d01530…
#   取出的文件   aria2c.exe（5 649 408 B）             sha256 be2099c2…
#
# ⚠️ **端点必须带版本号**（`…/releases/tags/release-1.37.0`），**不许**用 `releases/latest`：
#    latest 会随上游发新版而漂移，"来源可复核"就成了一句会自己失效的话。
#    包级 sha256 能兜住内容，但记录的来源 URL 本身应当指向**确定的版本**。
# ⚠️ GitHub 的 release 资产**直连 github.com 在本机不可达**（探路实测：curl 连不上），
#    但 api.github.com 可达。所以走**资产端点** `…/releases/assets/<id>` 并带
#    `Accept: application/octet-stream`，由它自己重定向。
#
# 本脚本**自验产物**（W-1：不许相信"下载对了"），四道判据互相独立：
#   ① 包 sha256 与登记值相符（两个独立工具各算一遍，先互相核对）；
#   ② 资产大小与 release API 自报的 `size` 相符（API 是另一条独立通道）；
#   ③ 取出文件自己的 sha256 与登记值相符（同上，两个独立工具）；
#   ④ 产物真是 **x86-64 的 PE**：`file` 的品类 + **从字节里读出 `Machine` 字段 == 0x8664**
#      （python3 解析 PE 头 + objdump 交叉复核），且**只链系统 DLL**（白名单)。
#      第 ④ 条是"文件名说 A、字节是 B"这个失效形态在**产物层**的落点。
#
# 退出码：0 = 资产已就位且登记一致；1 = 环境/下载/自验失败；2 = 下载内容与登记不符；
#         3 = 资产已就位，但**登记闭环没完成**——`daemon.rs` 里的 `ARIA2C_WINDOWS_SHA256`
#             还没跟上（照提示抄一行即可），**或者**根本找不到 `daemon.rs`（核对没做成）。
#             ⚠️ 后一种曾经是 `exit 0`（跳过检查却报成功），已订正：**一个不响的守卫，
#             与没有守卫，在客户机器上的后果完全一样**。
#
# ⚠️ **本脚本只管产物层**。`ARIA2C_WINDOWS_SHA256` 真正被核对是在 **Windows 机器上**
#    （`sha256_known_answer_vectors`），开发机上跑不了那份测试；退出码 3 就是为了
#    把"忘了抄常量"这件事挪到**本机当场**发现——否则它要等到客户机器上才炸。
set -euo pipefail

ARIA2_VERSION="1.37.0"
RELEASE_TAG="release-${ARIA2_VERSION}"
ZIP_NAME="aria2-${ARIA2_VERSION}-win-64bit-build1.zip"
# 登记值（见文件头）。改版本时这两条都要更新，脚本会核对。
ZIP_SHA256="67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288"
EXE_SHA256="be2099c214f63a3cb4954b09a0becd6e2e34660b886d4c898d260febfe9d70c2"
EXE_SIZE="5649408"
# 取出文件在该包里的路径。
ZIP_MEMBER="aria2-${ARIA2_VERSION}-win-64bit-build1/aria2c.exe"

RELEASE_API="https://api.github.com/repos/aria2/aria2/releases/tags/${RELEASE_TAG}"

# ⚠️ 路径全部从**脚本自身**推，与调用者的 cwd 无关（照 macos/scripts/build_dmg_macos.sh 的写法）。
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="$REPO/core/assets/aria2c-windows-x86_64.exe"
DAEMON_RS="$REPO/core/src/engine/daemon.rs"

# PE 的 `Machine` 字段（`IMAGE_FILE_MACHINE_AMD64`）——Windows×x86_64 的那个值。
PE_MACHINE_AMD64="0x8664"
# 唯一的合法架构串。objdump 的两个流派输出不同（llvm 是 `x86_64`，GNU binutils 是
# `i386:x86-64`），所以判据是"命中其中之一"而不是等值比较。
ARCH_OK_RE='(^|[^a-z0-9_])(x86-64|x86_64|i386:x86-64)([^a-z0-9_]|$)'
# 绝不允许出现的架构串——命中即失败（fail-closed，不是"没命中就放行"）。
ARCH_BAD_RE='(aarch64|arm64|ia64|i386([^:]|$)|armv7)'
# 允许的动态库白名单（大小写不敏感，忽略 `.dll` 后缀）。
# 来源：规格 §6.4 的偏离理由——"官方包同样只链系统 DLL"是**可验证的**，这里就验它。
ALLOWED_DLLS='advapi32 bcrypt crypt32 iphlpapi kernel32 msvcrt secur32 shell32 ws2_32 wsock32'

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*"; }

# ---------------------------------------------------------------------------
# 0. 工具预检（W-2：缺工具要**大声说**并给出**可执行的**补救）
# ---------------------------------------------------------------------------
# ⚠️ **补救话术按平台给三行**（macOS / Debian-Ubuntu / Windows）——2026-09-19 的 Windows
#    交接任务改的。本仓库要搬到一台**真 Windows 机器**上继续开发，而那里一句
#    `brew install …` 不但没用、**本身就是错的**；它偏偏又是"缺工具时唯一能看到的东西"，
#    而 W-2 要的是**可执行的**补救。三个平台的补救由 `$3` 带进来（**多行字符串**），
#    下面统一缩进对齐。
#    ⚠️ **判据一个字都没动**：检查的是同一个 `command -v`，失败仍然是这个 `exit 1`。
need() {
  local tool="$1" why="$2" fix="$3"
  if ! command -v "$tool" >/dev/null 2>&1; then
    {
      echo "错误：PATH 里没有 \`$tool\` —— $why"
      echo "      补救（按你的平台挑一行）："
      while IFS= read -r fix_line; do
        echo "        $fix_line"
      done <<< "$fix"
    } >&2
    exit 1
  fi
}
need curl   "下载 release 资产要用它"        "macOS: 系统自带
Debian/Ubuntu: apt-get install -y curl
Windows: Git for Windows 自带（/usr/bin/curl）；MSYS2 里是 \`pacman -S curl\`"
need python3 "解析 release API 的 JSON 与 PE 头" "macOS: 系统自带 python3（没有就 \`brew install python3\`）
Debian/Ubuntu: apt-get install -y python3
Windows: 装 MSYS2，再 \`pacman -S mingw-w64-x86_64-python\`
         ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\`；本脚本按 \`python3\` 找解释器，两个名字都要能被找到"
need unzip  "解开资产包"                    "macOS: 系统自带
Debian/Ubuntu: apt-get install -y unzip
Windows: MSYS2 的 \`pacman -S unzip\`（装完在 C:\\msys64\\usr\\bin\\ 里）"
need file   "自验产物的品类（PE32+）"        "macOS: 系统自带
Debian/Ubuntu: apt-get install -y file
Windows: MSYS2 的 \`pacman -S file\`"
need shasum "算包与产物的 sha256（其一）"    "macOS: 随 Perl 分发（系统自带）
Debian/Ubuntu: apt-get install -y perl
Windows: 随 Git for Windows 里的 Perl 一起来（/usr/bin/shasum）；MSYS2 里是 \`pacman -S perl\`"
need openssl "算包与产物的 sha256（其二，独立工具）" \
             "macOS: 系统自带（LibreSSL）
Debian/Ubuntu: apt-get install -y openssl
Windows: Git for Windows 自带（/usr/bin/openssl）；MSYS2 里是 \`pacman -S openssl\`"
# ⚠️ objdump **是必需的**，不当可选：产物自验里"架构那一半"要有**第二条独立读数**
#    （python3 读 PE 头是第一条），而"只链系统 DLL"这条**只能**由它给出。
#    缺它 ⇒ 只能给出一半证据 ⇒ 宁可大声失败，也不要打印一份看着完整的半证（W-2）。
#
# 两个流派都行，优先取 mingw-w64 自带的那份（它**原生**认 PE）：
#   - `x86_64-w64-mingw32-objdump`（GNU binutils）的 architecture 行是 `i386:x86-64, flags …`；
#   - 系统 `objdump`（macOS 上是 llvm 的）给的是 `x86_64`。
# 下面的白名单正则同时认这两种写法——**别把判据改成等值比较**。
OBJDUMP="$(command -v x86_64-w64-mingw32-objdump || command -v objdump || true)"
if [ -z "$OBJDUMP" ]; then
  {
    echo "错误：PATH 里既没有 x86_64-w64-mingw32-objdump 也没有 objdump ——"
    echo "      产物自验里「架构那一半」的第二条独立读数、以及「只链系统 DLL」这条都出不来。"
    echo "      补救（按你的平台挑一行）："
    echo "        macOS:          xcode-select --install（自带 llvm-objdump）或 brew install mingw-w64"
    echo "        Debian/Ubuntu:  apt-get install -y binutils"
    echo "        Windows:        装 MSYS2，再 \`pacman -S mingw-w64-x86_64-gcc\`（带来 x86_64-w64-mingw32-objdump）"
    echo "                        或 \`pacman -S binutils\`；再把 C:\\msys64\\mingw64\\bin 加进 PATH"
  } >&2
  exit 1
fi

echo "==> 目标：${DEST}"
echo "==> 来源：${RELEASE_API}"
echo "==> 版本：${ARIA2_VERSION}（与 macOS 那份同版本号）"

# ---------------------------------------------------------------------------
# 1. 问 release API 要资产 id 与它自报的大小
# ---------------------------------------------------------------------------
note "查询 release 资产列表"
curl -sS --max-time 60 -o "$WORK/release.json" "$RELEASE_API" \
  || die "取 release 元数据失败：$RELEASE_API"

# ⚠️ 解析与登记**分开**：这里只取出 (id, size)，登记值是否相符由下面的比对负责。
#    解析失败（端点改了形状 / 网络返回了错误 JSON）要**当场**失败，不要静默给空串。
read -r ASSET_ID ASSET_SIZE < <(python3 - "$WORK/release.json" "$ZIP_NAME" <<'PY'
import json, sys
path, want = sys.argv[1], sys.argv[2]
try:
    d = json.load(open(path))
except Exception as e:                       # noqa: BLE001 - 这里就是要把它变成可读的失败
    sys.exit(f"release 元数据不是 JSON（{e}）")
if "assets" not in d:
    sys.exit(f"release 元数据里没有 assets 字段，实际顶层键：{sorted(d)[:8]}")
hit = [a for a in d["assets"] if a.get("name") == want]
if len(hit) != 1:
    sys.exit(f"资产 {want!r} 在 release {d.get('tag_name')!r} 里命中 {len(hit)} 次（应为 1）")
print(hit[0]["id"], hit[0]["size"])
PY
) || die "解析 release 元数据失败（原因见上一行）"

[ -n "${ASSET_ID:-}" ] || die "没解析出资产 id"
note "资产 id=${ASSET_ID}，API 自报大小=${ASSET_SIZE} B"

# ---------------------------------------------------------------------------
# 2. 下载（走资产端点，让它自己重定向到 objects.githubusercontent.com）
# ---------------------------------------------------------------------------
note "下载 ${ZIP_NAME}"
curl -sSL --http1.1 --max-time 240 -H 'Accept: application/octet-stream' \
     -o "$WORK/$ZIP_NAME" \
     "https://api.github.com/repos/aria2/aria2/releases/assets/${ASSET_ID}" \
  || die "下载失败：资产 ${ASSET_ID}（网络不通时先确认 api.github.com 可达）"

# ---------------------------------------------------------------------------
# 3. 自验 ①：包 sha256 —— 两个独立工具各算一遍，**先互相核对**再比登记值
# ---------------------------------------------------------------------------
# ⚠️ 顺序承重：先用两个工具互核，再比登记值。少了互核这一步，"工具本身给错"与
#    "下载内容不对"两种失败会挤在同一句报错里，排障时无从下手。
PKG_A="$(shasum -a 256 "$WORK/$ZIP_NAME" | awk '{print $1}')"
PKG_B="$(openssl dgst -sha256 "$WORK/$ZIP_NAME" | awk '{print $NF}')"
[ "$PKG_A" = "$PKG_B" ] || die "两个工具算出的包 sha256 不一致：shasum=${PKG_A} openssl=${PKG_B}（先怀疑环境，不要改登记值）"
if [ "$PKG_A" != "$ZIP_SHA256" ]; then
  {
    echo "错误：包 sha256 与登记值不符（不符即拒，不安装）。" >&2
    echo "      期望（登记）：$ZIP_SHA256" >&2
    echo "      实际（下载）：$PKG_A" >&2
    echo "      若这是**有意的版本升级**：先确认新包的来源 URL 与版本号，" >&2
    echo "      再把脚本头部的登记、ZIP_SHA256/EXE_SHA256/EXE_SIZE 与 daemon.rs 一起改。" >&2
  } >&2
  exit 2
fi
echo "包 sha256：${PKG_A}（shasum 与 openssl 一致，与登记值相符）"

# ---------------------------------------------------------------------------
# 4. 自验 ②：大小与 API 自报的 size 相符（独立于 sha256 的第二条通道）
# ---------------------------------------------------------------------------
GOT_SIZE="$(wc -c < "$WORK/$ZIP_NAME" | tr -d ' ')"
[ "$GOT_SIZE" = "$ASSET_SIZE" ] || die "包大小与 release API 自报的不符：API=${ASSET_SIZE} 实际=${GOT_SIZE}（下载被截断？）"
echo "包大小：$GOT_SIZE B（与 API 自报一致）"

# ---------------------------------------------------------------------------
# 5. 取出 aria2c.exe
# ---------------------------------------------------------------------------
note "解包并取出 ${ZIP_MEMBER}"
( cd "$WORK" && unzip -q -o "$ZIP_NAME" ) || die "解包失败：$WORK/$ZIP_NAME"
SRC="$WORK/$ZIP_MEMBER"
[ -f "$SRC" ] || die "包里没有 ${ZIP_MEMBER}（包的内部布局变了？用 unzip -l 看一眼）"

# ---------------------------------------------------------------------------
# 6. 自验 ③：取出文件自己的 sha256 与登记值相符
# ---------------------------------------------------------------------------
EXE_A="$(shasum -a 256 "$SRC" | awk '{print $1}')"
EXE_B="$(openssl dgst -sha256 "$SRC" | awk '{print $NF}')"
[ "$EXE_A" = "$EXE_B" ] || die "两个工具算出的 aria2c.exe sha256 不一致：shasum=${EXE_A} openssl=${EXE_B}"
if [ "$EXE_A" != "$EXE_SHA256" ]; then
  {
    echo "错误：取出的 aria2c.exe 与登记值不符（不符即拒，不安装）。" >&2
    echo "      期望（登记）：$EXE_SHA256" >&2
    echo "      实际（取出）：$EXE_A" >&2
  } >&2
  exit 2
fi
EXE_BYTES="$(wc -c < "$SRC" | tr -d ' ')"
[ "$EXE_BYTES" = "$EXE_SIZE" ] || die "aria2c.exe 大小与登记值不符：期望 ${EXE_SIZE} B，实际 ${EXE_BYTES} B"
echo "aria2c.exe sha256：${EXE_A}（shasum 与 openssl 一致，与登记值相符）"
echo "aria2c.exe 大小：${EXE_BYTES} B（与登记值相符）"

# ---------------------------------------------------------------------------
# 7. 自验 ④：产物真是 x86-64 的 PE —— 品类 + Machine 字段 + 只链系统 DLL
# ---------------------------------------------------------------------------
# 7a. 品类。`file` 是**人眼可读**的那一层；它单独不足以判定架构（"PE32+" 同时包含
#     x86-64 与 ARM64），所以下面必须从字节里读 Machine。
FILE_OUT="$(file -b "$SRC")"
echo "file：$FILE_OUT"
case "$FILE_OUT" in
  *PE32+*) : ;;
  *) die "产物不是 PE32+（file 说：${FILE_OUT}）——文件名说它是 Windows 的 PE，字节却不是" ;;
esac
case "$FILE_OUT" in
  *x86-64*|*x86_64*) : ;;
  *) die "file 没把产物认成 x86-64（file 说：${FILE_OUT}）" ;;
esac

# 7b. **从字节里读 `Machine` 字段**（裁定 Q 第 1 件：不相信"下载对了"）。
#     python3 直接解析 PE 头：DOS 头 `e_lfanew`(0x3c) → `PE\0\0` → `Machine`(u16 LE)。
MACHINE="$(python3 - "$SRC" <<'PY'
import struct, sys
b = open(sys.argv[1], "rb").read()
if len(b) < 0x40 or b[:2] != b"MZ":
    sys.exit("不是 MZ 开头（或长度不足 DOS 头）")
e = struct.unpack_from("<I", b, 0x3C)[0]
if len(b) < e + 6:
    sys.exit(f"e_lfanew=0x{e:x} 指向文件外（文件 {len(b)} B）")
if b[e:e+4] != b"PE\0\0":
    sys.exit(f"e_lfanew=0x{e:x} 处不是 PE\\0\\0 签名，实际 {b[e:e+4]!r}")
print(f"0x{struct.unpack_from('<H', b, e+4)[0]:04x}")
PY
)" || die "解析 PE 头失败（产物不是合法 PE）——上面那行是原因"
echo "PE Machine：${MACHINE}（python3 从字节里读出）"
[ "$MACHINE" = "$PE_MACHINE_AMD64" ] || die "产物的 Machine 字段是 ${MACHINE}，不是 x86_64 的 ${PE_MACHINE_AMD64}——架构下错了（ARM64 是 0xaa64、IA64 是 0x0200）"

# 7c. 第二条独立读数：objdump 的 `architecture:` 行。
#     判据是**命中白名单**（llvm 与 GNU 的写法不同）**且不命中黑名单**——fail-closed。
OBJDUMP_ARCH="$("$OBJDUMP" -f "$SRC" 2>/dev/null | sed -n 's/^architecture: *//p' | head -1)"
[ -n "$OBJDUMP_ARCH" ] || die "objdump 没给出 architecture 行（输出格式变了？手工跑一次："$OBJDUMP" -f '$SRC'）"
echo "objdump architecture：$OBJDUMP_ARCH"
printf '%s' "$OBJDUMP_ARCH" | grep -Eq "$ARCH_BAD_RE" \
  && die "objdump 认出的架构是「${OBJDUMP_ARCH}」——这是**别的架构**的产物"
printf '%s' "$OBJDUMP_ARCH" | grep -Eq "$ARCH_OK_RE" \
  || die "objdump 认出的架构「${OBJDUMP_ARCH}」不在 x86-64 的写法白名单里（白名单见脚本里的 ARCH_OK_RE）"

# 7d. 只链系统 DLL —— 这是规格 §6.4 那句偏离理由的**可验证部分**，所以在这里验。
DLLS="$("$OBJDUMP" -p "$SRC" 2>/dev/null | sed -n 's/^[[:space:]]*DLL Name: *//p' | tr 'A-Z' 'a-z' | sed 's/\.dll$//' | sort -u)"
[ -n "$DLLS" ] || die "读不出 DLL 导入表（$OBJDUMP -p 的输出格式变了？）"
echo "依赖的 DLL：$(echo "$DLLS" | tr '\n' ' ')"
BAD_DLLS=""
while IFS= read -r d; do
  [ -n "$d" ] || continue
  case " $ALLOWED_DLLS " in
    *" $d "*) : ;;
    *) BAD_DLLS="$BAD_DLLS $d" ;;
  esac
done <<< "$DLLS"
[ -z "$BAD_DLLS" ] || die "依赖了白名单之外的 DLL：${BAD_DLLS}（官方包的偏离理由之一是"只链系统 DLL"；出现别的 DLL 时那份理由就不成立了）"

# ---------------------------------------------------------------------------
# 8. 安装：先写临时文件再 `mv`（同目录内 rename 是原子的），且**内容相同就不碰**
# ---------------------------------------------------------------------------
if [ -f "$DEST" ] && [ "$(shasum -a 256 "$DEST" | awk '{print $1}')" = "$EXE_SHA256" ]; then
  # 内容相符就不重写，但**模式要对齐**（理由见下面 else 分支里的注释）：内容相同而
  # 只有 mode 不对时，git 仍会记一次变更——修它比留一处无谓的 diff 便宜。
  chmod 755 "$DEST"
  note "core/assets/ 里那份已经就是它（sha256 相符），不重写（已对齐模式）"
else
  TMP="$DEST.tmp.$$"
  cp "$SRC" "$TMP"
  # 与 `core/assets/` 里另外两份对齐（也都是 0755），同时与 `core/build.rs` 的规则一致
  # ——它给非 `.txt` 资产设的就是 0o755。git 会记这一位，所以两边不一致会变成一处无谓的
  # mode 变更；Windows 上这一位没有意义，设它纯粹是为了**同一目录里三份资产口径相同**。
  chmod 755 "$TMP"
  mv -f "$TMP" "$DEST"
  note "已写入 ${DEST}"
fi
[ "$(shasum -a 256 "$DEST" | awk '{print $1}')" = "$EXE_SHA256" ] \
  || die "落位后的文件 sha256 与登记值不符（写入过程中被改动？）"

# ---------------------------------------------------------------------------
# 9. 登记闭环：daemon.rs 里的常量必须与刚落的这份一致
# ---------------------------------------------------------------------------
# ⚠️ 这一步是**本脚本存在的理由之一**：`ARIA2C_WINDOWS_SHA256` 的核对只在
#    **Windows 机器上**发生（`sha256_known_answer_vectors` 是运行时测试），
#    开发机上跑不了。少了这一步，"资产换了、常量忘抄" 就要等到客户机器上才炸。
# ⚠️ **这里是全脚本唯一一处曾经"跳过检查却报成功"的地方**（任务 2 审查裁定的第二件），
#    已改成非零退出：调用者必须能分清"核对过了"与"根本没核对"——
#    **一个不响的守卫，与没有守卫，在客户机器上的后果完全一样**。
#    用退出码 3（与"资产已就位、登记闭环没完成"同一族，见文件头的退出码说明）：
#    资产确实已经落位并自验通过，但**闭环没完成**，所以绝不能以 0 退出。
if [ ! -f "$DAEMON_RS" ]; then
  {
    echo ""
    echo "⚠️ 找不到 ${DAEMON_RS} —— 常量核对**没有做**，不是\"核对通过\"。"
    echo ""
    echo "  这是脚本判断路径的错，不是你的；但**结果不能算成功**："
    echo "  \"资产已落位、而 daemon.rs 里的 ARIA2C_WINDOWS_SHA256 没跟上\" 这件事，"
    echo "  本来就要靠第 9 步在**本机**当场发现，否则要等到客户机器上才炸。"
    echo ""
    echo "  补救：确认脚本所在的是一个**完整的 checkout**（上面 REPO/DEST/DAEMON_RS"
    echo "        三行就是它推路径的全部依据，脚本自己不会去找别的地方）；"
    echo "        core/src/engine/daemon.rs 真的不在时，先把仓库弄完整再重跑本脚本。"
    echo ""
    echo "  资产本身已就位并自验通过：${DEST}"
  } >&2
  exit 3
fi
# 取那条登记的字符串字面量。
#
# ⚠️ **2026-09-18 起判据简化了，理由留在这里**：合并之前 `ARIA2C_EMBED_SHA256` 有
#    三条同名 `#[cfg]` 分支，判据必须"贴着 const 往上找它自己那条 cfg"才能分辨
#    （否则会命中 `ARIA2C_BIN` 的那条 cfg、读到**别的常量**——实测踩过）。
#    现在 Windows 的登记全仓**只有一条、且 cfg-free**（`ARIA2C_WINDOWS_SHA256`），
#    那种歧义不存在了。但"**提到**这个名字的行"仍要排除：文档注释、测试里的别名
#    （`const ARIA2C_EMBED_SHA256: &str = ARIA2C_WINDOWS_SHA256;`）与断言文案里都出现了
#    它，所以判据收紧成"这一行是**定义行**"。
#
# ⚠️ **2026-09-18 第二次收紧：锚定行首**（任务 4 的审查裁定）。上一版判据是
#    `"const ARIA2C_WINDOWS_SHA256" in ln` —— **子串匹配 + 命中即 break**：任何位置的
#    第一条命中即胜。今天全仓只有一条真定义，所以它是**对的**；但它抗不住未来——
#    只要出现一条**排在定义行之前、含这个子串、后面还跟着一对引号**的行，旧判据就会
#    抢先命中并读走那段引号里的东西。实测（构造夹具）：把
#    `//! 见 "const ARIA2C_WINDOWS_SHA256" 与 "const ARIA2C_EMBED_SHA256" 两处登记。`
#    摆在定义行之前，旧判据读出的是 `与`，新判据仍读到真值。
#    ⚠️ **别名那一条命中不了**：`const ARIA2C_EMBED_SHA256: &str = ARIA2C_WINDOWS_SHA256;`
#    里没有 `const ARIA2C_WINDOWS_SHA256` 这个子串——所以它**不是**这次收紧要防的东西
#    （根因写对才算数）。防的是"以后有人把带引号的说明写在定义行前面"。
#    锚定行首之后只有真定义行能赢：允许一个可见性修饰（`pub` / `pub(crate)`），别的都不认。
CONST_IN_RS="$(python3 - "$DAEMON_RS" <<'PY'
import re, sys

lines = open(sys.argv[1], encoding="utf-8").read().split("\n")
found = None
for i, ln in enumerate(lines):
    m_def = re.match(r'^\s*(?:pub(?:\s*\([^)]*\))?\s+)?const\s+ARIA2C_WINDOWS_SHA256\b', ln)
    if not m_def:
        continue
    # 字面量可能与 const 同行，也可能被 rustfmt 折到下一行
    blob = ln[m_def.end():]
    if i + 1 < len(lines):
        blob += "\n" + lines[i + 1]
    m = re.search(r'"([^"]*)"', blob)
    if m:
        found = m.group(1)
        break
sys.stdout.write(found or "")
PY
)"

if [ "$CONST_IN_RS" = "$EXE_SHA256" ]; then
  echo "==> 登记一致：daemon.rs 的 ARIA2C_WINDOWS_SHA256 已经是 $EXE_SHA256"
  exit 0
fi
{
  echo ""
  echo "⚠️ 资产已就位，但 **daemon.rs 的常量还没跟上**——把下面这行抄进去再重跑本脚本："
  echo ""
  echo "    const ARIA2C_WINDOWS_SHA256: &str = \"$EXE_SHA256\";"
  echo ""
  echo "  （${DAEMON_RS}）"
  echo "  当前那里的值是：${CONST_IN_RS:-<解析不出来：那段代码的形状变了>}"
  echo ""
  echo "  为什么必须抄：这条常量只在 **Windows 机器上**被核对，本机跑不到那份测试；"
  echo "  不抄的话，资产与常量的不一致要等到客户机器上才暴露。"
} >&2
exit 3
