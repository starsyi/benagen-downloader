#!/usr/bin/env bash
# 为 Linux（x86_64-musl）构建**真静态**的 aria2c，供 benagen-dl 内嵌。
#
# 构建机：root@<构建机>（Oracle Linux Server 9.5，x86_64，8 核 / 74G 空闲 / 外网通）
# 在**那台机器上**以 root 运行本脚本（它只写 $BUILD_ROOT，默认 /root/benagen-cli-build）。
#
#     scp core/scripts/build_aria2_linux.sh root@<构建机>:/tmp/
#     ssh root@<构建机> 'bash /tmp/build_aria2_linux.sh'
#
# 产物：$BUILD_ROOT/out/aria2c-linux-x86_64（静态、能跑，末尾打印 sha256 与 file 输出）
#
# ⚠️ 本脚本是 2026-09-21 在**真实构建机上从零跑通**的配方，不是推演出来的。
#    下面每一条"⚠️ 与计划不同"都对应一次实测失败，原始输出留在 $BUILD_ROOT/logs/。
#    改动这些地方之前请先读该条注释。
#
# ---------------------------------------------------------------------------
# ⚠️ 与任务书 Step 3–8 的**五处差异**（每一处都是实测撞出来的，不是自由发挥）
# ---------------------------------------------------------------------------
# ① Step 5 少了一条 `make install`。
#    musl-cross-make 的 README 原文："Nothing is installed until running `make install`"。
#    只跑 `make -j8` 得到的 `$BUILD/musl/bin/` 是**空的**，后面所有步骤都会找不到交叉编译器。
#    → 本脚本在 `make -j8` 之后补 `make install`。
#
# ② Step 5 会失败：musl-cross-make 的 cowpatch.sh 与 OL9 自带的 GNU patch 2.7.6 不兼容。
#    cowpatch.sh 用「符号链接农场」省一次拷贝；而 patch 2.7.6 **拒绝**穿过符号链接。实测两种形态：
#      路径中间某段是符号链接   -> can't find file to patch（16 个 hunk 全被 skip）
#      要打的文件本身是符号链接 -> File ... is not a regular file -- refusing to patch
#    于是 linux-headers 目标必然 Error 1（原始输出见上面那段"can't find file to patch"）。
#    `patch --follow-symlinks` 在该版本上**不起作用**（实测无效）。
#    → 本脚本用 `COWPATCH=` 覆盖成一个**真拷贝**版本（$BUILD_ROOT/bin/cowpatch_realcopy.sh）：
#      `-I` 做 cp -a，`-p1` 直接交给 patch。打出来的源码树与上游语义完全一致，
#      只是不再靠符号链接省空间。
#
# ③ Step 6 的 `AR="x86_64-linux-musl-ar r"` 会**编不过**。
#    OpenSSL 生成的 Makefile 是 `ARFLAGS= qc`，规则是 `$(AR) $(ARFLAGS) $@ ...`；
#    AR 里再带一个 `r` 就变成 `ar r qc apps/libapps.a ...`，GNU ar 把 `qc` 当成了**归档文件名**：
#        x86_64-linux-musl-ar: creating qc
#        x86_64-linux-musl-ar: apps/libapps.a: No such file or directory
#        make[1]: *** [Makefile:3159: apps/libapps.a] Error 1
#    → AR 只能是 `x86_64-linux-musl-ar`（`r` 由 OpenSSL 自己的 ARFLAGS 提供）。
#
# ④ Step 6 需要 `no-module`，否则编出来的 aria2c **一跑就崩**。
#    aria2 1.37 在 OpenSSL >= 3.0 下**无条件**调用 `OSSL_PROVIDER_load(nullptr, "legacy")`，
#    失败就抛异常（src/Platform.cc:125）。默认构建里 legacy 只是个 `legacy.so` 模块，
#    而 musl 静态二进制根本没有动态加载器，`dlopen` 必然失败 —— 实测：
#        $ ./aria2c --version
#        Exception caught
#        Exception: [Platform.cc:125] errorCode=1 OSSL_PROVIDER_load 'legacy' failed.
#        (退出码 1，无任何版本输出)
#    OpenSSL 自己的 providers/build.info 写得很明白：`$disabled{module}` 为真时
#    legacy 被编进 libcrypto（`SOURCE[../libcrypto]=$LIBLEGACY`）。
#    → Configure 加 `no-module`。
#
# ⑤ Step 7 的 `--with-openssl=<路径>` 是**静默无效**的。
#    aria2 的 configure.ac 里那条判断是 `if test "x$with_openssl" = "xyes" && ...`，
#    传路径进去时该值不等于 `xyes`，整段 OpenSSL 探测**直接被跳过**，于是：
#        SSL Support:    no
#        OpenSSL:        no (CFLAGS='' LIBS='')
#    接着 Platform.cc 编不过（`'setClientTLSContext' is not a member of 'aria2::SocketApi'`）。
#    也就是说：**"编出来了"的假象 + 一个没有 TLS 的产物**，正是 Step 8 那条判据要拦的东西。
#    → 改用 pkg-config 通路：`--with-openssl=yes` + `PKG_CONFIG_PATH=$SSL/lib64/pkgconfig`
#      （OpenSSL 的 install_sw 会装 openssl.pc / libssl.pc / libcrypto.pc）。
#
# ---------------------------------------------------------------------------
# ⚠️ 构建机上额外装的东西（都是构建期依赖，不在产物里）
# ---------------------------------------------------------------------------
# Step 3 的 dnf 清单之外，还装了 9 个 perl 模块：OpenSSL 的 Configure/构建要用，
# 而 OL9 的最小化 perl 把它们拆包了 —— 少一个就停在 Configure。实测撞到的是前两个：
#     Can't locate FindBin.pm in @INC ... at ./Configure line 15.
#     Can't locate IPC/Cmd.pm in @INC ... at util/perl/OpenSSL/config.pm line 19.
# 其余是照 OpenSSL 源码里 `use` 到的模块清单一次补齐的（省得一个一个撞）。
#   perl-FindBin perl-File-Copy perl-Math-BigInt perl-Digest-SHA
#   perl-IPC-Cmd perl-File-Compare perl-Pod-Html perl-Test-Simple perl-Text-Diff
#
# ---------------------------------------------------------------------------
# ⚠️ 源码包 sha256 是**固定校验**的（与 downloader/scripts/build_aria2_macos.sh 同款纪律）
# ---------------------------------------------------------------------------
# aria2 与 openssl 两个是上游官方 release 包，sha256 稳定。
# musl-cross-make 那个是 **GitHub 自动生成的 tarball**（codeload），它的字节
# **不保证跨时间稳定**（GitHub 换过压缩实现）。这里仍然钉住实测值 —— 好处是
# "这次构建用了哪份字节"有据可查；代价是将来某天它会 fail 而不是静默换一份源码。
# 真到那天：核对 tarball 内容后更新常量，并**在提交信息里写清为什么**。
# 它自己下载的 binutils/gcc/musl/gmp/mpfr/mpc/linux-headers 由 musl-cross-make
# 用仓库内 `hashes/*.sha1` 自行校验（本次实测 linux-headers 的 sha1 与钉住值一致）。
# ⚠️ 注意 LINUX_HEADERS_SITE 是 **http**（ftp.barfooze.de），完整性依赖上面那条 sha1。
#
# ---------------------------------------------------------------------------
# ⚠️ 产物**不是逐字节可复现**的（与 macOS 那份同样的结论）
# ---------------------------------------------------------------------------
# aria2 把编译时刻编进了二进制（`aria2c --version` 里的 `on Sep 21 2026 01:51:52`），
# 所以重跑得到的 sha256 **必然**与上一次不同。可复现的是**配方**，不是字节。
# 每次跑完都把 sha256 打出来；要入库时以**当次**的值为准。
#
# ---------------------------------------------------------------------------
# ⚠️ 本脚本**做 strip**（第 6 步里，判据自验之前）—— 与 macOS 那份**不 strip** 并不矛盾
# ---------------------------------------------------------------------------
# 两边不是"一个 strip 一个没 strip"的不一致，是**各自的约束不同**：
#
#   macOS（downloader/scripts/build_aria2_macos.sh:56-59，那边写明了理由）**不能** strip：
#     ① 它没有可去的东西 —— 那份脚本显式传了 `CFLAGS="-O2 -arch ..."`，这会**顶掉**
#        autoconf 的默认 `-g -O2`，产物本来就没有调试信息（"既不需要")；
#     ② strip 会**让 arm64 的 ad-hoc 签名失效**，而那边实测"arm64 上签名无效等于不能执行"。
#
#   Linux（本脚本）**要** strip：
#     ① 有真东西可去 —— 本脚本照任务书**没有显式设 CFLAGS**，于是 autoconf 补上了
#        默认的 `-g -O2`（这是有意的，见第 4 步那段：显式设 CFLAGS 会连 `-O2` 一起丢掉）。
#        实测调试信息占了 79 699 424 B 里的 73 MB 左右，代码+数据只有 6 466 380 B；
#     ② Linux 没有签名这道约束 —— ELF 上 strip 只是删符号表，**不影响可执行性**
#        （下面判据自验就是对着 strip 之后的字节跑的）。
#
# 为什么值得做：这个产物要经 `include_bytes!` 内嵌进 benagen-dl，
# 不 strip 的话 Linux 的 CLI 会变成 ~81 MB，而 strip 后 ~6.4 MB ——
# 同一个交付里 macOS 那份才 5.5 MB。"单文件工具"不该为了调试符号胖 12 倍。
# 实测（2026-09-21）：79 699 424 B → 6 432 984 B，三条判据依然全过。
set -euo pipefail

# ---------------------------------------------------------------------------
# 版本与校验和（改这里 = 换依赖版本，务必同时更新 sha256）
# ---------------------------------------------------------------------------
ARIA2_VERSION="1.37.0"
OPENSSL_VERSION="3.0.15"
MCM_VERSION="0.9.9"
RUST_TOOLCHAIN="1.89.0"

MUSL_TARGET="x86_64-unknown-linux-musl"
CROSS="x86_64-linux-musl"

ARIA2_TARBALL_SHA256="8e7021c6d5e8f8240c9cc19482e0c8589540836747744724d86bf8af5a21f0e8"
OPENSSL_TARBALL_SHA256="23c666d0edf20f14249b3d8f0368acaee9ab585b09e1de82107c66e1f3ec9533"
# GitHub codeload 自动生成的 tarball —— 见上面那段说明
MCM_TARBALL_SHA256="ff3e2188626e4e55eddcefef4ee0aa5a8ffb490e3124850589bcaf4dd60f5f04"

BUILD_ROOT="${BUILD_ROOT:-/root/benagen-cli-build}"
SRC="$BUILD_ROOT/src"
BUILD="$BUILD_ROOT/build"
OUT="$BUILD_ROOT/out"
LOGS="$BUILD_ROOT/logs"
BIN="$BUILD_ROOT/bin"
TOOLCHAIN="$BUILD/musl"
SSL="$BUILD/ssl"
COWPATCH_SHIM="$BIN/cowpatch_realcopy.sh"

# ---------------------------------------------------------------------------
# 步骤记账：任何一步失败，非零退出并**打印当前是哪一步**
# ---------------------------------------------------------------------------
STEP_NAME="（尚未开始）"
DIED=0
step() {
  STEP_NAME="$1"
  echo ""
  echo "================================================================"
  echo "== [STEP] $STEP_NAME"
  echo "================================================================"
}
# 显式失败：说清是哪一步、为什么，然后非零退出
die() {
  DIED=1
  echo "" >&2
  echo "!!!!! 构建失败于步骤：$STEP_NAME" >&2
  echo "!!!!! 原因：$*" >&2
  exit 1
}
# 兜底：任何**没有**走 die 的失败（set -e 直接掀桌的那类）也要说清是哪一步
on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ] && [ "$DIED" -eq 0 ]; then
    echo "" >&2
    echo "!!!!! 构建失败于步骤：${STEP_NAME}（退出码 $rc，未走 die 路径）" >&2
  fi
}
trap on_exit EXIT

# fetch <url> <目标文件> <期望 sha256>
# 已存在且校验通过就跳过下载（重跑友好）；下载走与 macOS 脚本同款的抗断流参数。
fetch() {
  local url="$1" dest="$2" want="$3" got
  if [ -f "$dest" ]; then
    got="$(sha256sum "$dest" | awk '{print $1}')"
    if [ "$got" = "$want" ]; then
      echo "  [fetch] 已存在且校验通过，跳过下载：$dest"
      return 0
    fi
    echo "  [fetch] 已存在但 sha256 不符，重新下载：$dest"
    rm -f "$dest"
  fi
  echo "  [fetch] 下载 $url"
  # --retry-all-errors：实测到 GitHub 的链路会在**传输中途**断（curl 56），
  #   默认的 --retry 只管连接阶段，不带这个开关会直接判死。
  # --speed-limit/--speed-time：实测撞过**半开连接**（连上了、收了几百 KB、然后彻底不动），
  #   那种卡法不报错，curl 会无限期挂着。
  # 两者都救不了"完全不通"——那时 curl 重试完照样失败退出，这是对的（大声失败）。
  curl -fsSL --retry 10 --retry-all-errors --retry-delay 3 \
    --connect-timeout 20 --speed-limit 1024 --speed-time 30 \
    -o "$dest.part" "$url" || { rm -f "$dest.part"; die "下载失败：$url"; }
  mv -f "$dest.part" "$dest"
  got="$(sha256sum "$dest" | awk '{print $1}')"
  if [ "$got" != "$want" ]; then
    echo "错误：sha256 不符，拒绝继续。" >&2
    echo "  文件: $dest" >&2
    echo "  期望: $want" >&2
    echo "  实际: $got" >&2
    die "源码包 sha256 校验失败（下载被劫持或上游换了字节？）"
  fi
  echo "  [fetch] sha256 校验通过：$got"
}

mkdir -p "$SRC" "$BUILD" "$OUT" "$LOGS" "$BIN"

# ===========================================================================
step "0/7 预检：这台机器还是那个样子吗"
# ===========================================================================
# 后面的数（并行度、磁盘）都是照这台机器算的；对不上就停下，别硬编。
ARCH="$(uname -m)"
[ "$ARCH" = "x86_64" ] || die "构建机架构是 $ARCH，本脚本只处理 x86_64"
NPROC="$(nproc)"
AVAIL_K="$(df -Pk / | tail -1 | awk '{print $4}')"
echo "  架构:     $ARCH"
echo "  内核/系统: $(. /etc/os-release && echo "$PRETTY_NAME")"
echo "  核数:     $NPROC"
echo "  根分区可用: $((AVAIL_K / 1024 / 1024)) GiB"
# 整套东西（源码 + 各套构建树）实测 5.8 GiB，留 20 GiB 底 —— 余量给的是
# 「构建中途解压/链接的瞬时峰值」，不是稳态占用。
[ "$AVAIL_K" -gt $((20 * 1024 * 1024)) ] || die "根分区可用空间不足 20 GiB，构建会中途写满"

# ===========================================================================
step "1/7 装构建工具链（dnf）"
# ===========================================================================
# 这里装的是**构建期**依赖，产物里不含它们。
# ⚠️ `file` 必须在这个清单里：它撑着三处判据（交叉链冒烟、判据①、末尾指纹），
#    却**没有**传递依赖 —— gcc 会带进 binutils（=> readelf），但不会带进 file。
#    它缺席时的失效形态很坏：`command -v file` 失败会让 `file ... | grep -q "statically linked"`
#    恒假，报出来的是"交叉链冒烟产物不是静态的"，把人往交叉链上引。
#    所以下面既装它、又显式断言它在（见本步末尾）。
TOOLCHAIN_PKGS="gcc gcc-c++ make cmake patch bzip2 xz tar curl git file"
MISSING_PKGS=""
for c in gcc make file readelf; do
  command -v "$c" >/dev/null 2>&1 || MISSING_PKGS="$MISSING_PKGS $c"
done
if [ -n "$MISSING_PKGS" ]; then
  echo "  缺:$MISSING_PKGS —— 装"
  dnf install -y $TOOLCHAIN_PKGS || die "dnf 装基础工具链失败"
else
  echo "  gcc/make/file/readelf 已在，跳过"
fi
# 装完再断言一次：dnf 说成功不等于命令真在（别把"报错被吞掉"当成"装好了"）。
for c in gcc make file readelf; do
  command -v "$c" >/dev/null 2>&1 \
    || die "构建依赖 '$c' 不在 PATH 里 —— 它会撑起后面的判据，缺席时判据会假过"
done
# OpenSSL 的 Configure/构建要用的 perl 模块；OL9 的最小化 perl 把它们拆包了。
# 少一个就停在 Configure（见文件顶部那段）。
dnf install -y perl-FindBin perl-File-Copy perl-Math-BigInt perl-Digest-SHA \
  perl-IPC-Cmd perl-File-Compare perl-Pod-Html perl-Test-Simple perl-Text-Diff \
  || die "dnf 装 perl 构建依赖失败"
echo "  gcc:  $(gcc --version | head -1)"
echo "  make: $(make --version | head -1)"

# ===========================================================================
step "2/7 装 Rust 与 musl target"
# ===========================================================================
# ⚠️ 这一步**不是本脚本产物的依赖**，对本脚本的产物没有任何影响：
#    aria2 是 C++，走的是第 3 步那条 C 交叉链。装 Rust + musl target 是为
#    Task 9（benagen-dl 本体）预备的 —— 主工程要用
#    `--target x86_64-unknown-linux-musl` 出单文件。
#    `rustup target add` 只是**下载 rust-std**，不链接、不需要交叉链，
#    所以它与第 3 步**没有先后依赖**（实测：本次运行里它排在第 3 步之前，正常通过）。
#    排在前面只是让 rustup 的下载与后面漫长的编译重叠掉一部分。
if [ -x /root/.cargo/bin/rustup ]; then
  echo "  rustup 已在，跳过安装"
else
  curl -fsSL --retry 5 --retry-all-errors -o /tmp/rustup-init.sh https://sh.rustup.rs \
    || die "下载 rustup-init.sh 失败"
  sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN" \
    || die "rustup 安装失败"
fi
/root/.cargo/bin/rustup target add "$MUSL_TARGET" || die "rustup target add $MUSL_TARGET 失败"
echo "  已安装 target："
/root/.cargo/bin/rustup target list --installed | sed 's/^/    /'

# ===========================================================================
step "3/7 编 musl 交叉链（musl-cross-make $MCM_VERSION）"
# ===========================================================================
# ---- 3a. OL9 的 patch 2.7.6 不兼容 cowpatch.sh：写一个真拷贝版替换品 ----
# 详见文件顶部 ② 条。这里把替换品落盘（内容固定，重跑覆盖即可）。
cat > "$COWPATCH_SHIM" <<'COWPATCH_EOF'
#!/bin/sh
# musl-cross-make 的 cowpatch.sh 用「符号链接农场」省一次拷贝。
# OL9 自带的 GNU patch 2.7.6 **拒绝**穿过符号链接：
#   路径中间某段是符号链接   -> "can't find file to patch"
#   要打的文件本身是符号链接 -> "is not a regular file -- refusing to patch"
# 于是 cowpatch.sh 的 -I/-p1 组合在 OL9 上必然失败。
# 本脚本是等价替换：-I 做**真拷贝**，-p1 直接交给 patch。产出的源码树与上游语义一致。
set -e
case "${1:-}" in
  -I)
    dir="${2:?cowpatch_realcopy: -I needs a directory}"
    [ -d "$dir" ] || { echo "cowpatch_realcopy: no such dir: $dir" >&2; exit 1; }
    cp -a "$dir"/. .
    ;;
  -p1)
    exec patch -p1
    ;;
  *)
    echo "cowpatch_realcopy: unrecognized args: $*" >&2
    exit 2
    ;;
esac
COWPATCH_EOF
chmod +x "$COWPATCH_SHIM"

# ---- 3b. 工具链已装好就跳过（重跑友好）----
if [ -x "$TOOLCHAIN/bin/$CROSS-gcc" ]; then
  echo "  交叉链已在 $TOOLCHAIN，跳过构建"
else
  fetch "https://github.com/richfelker/musl-cross-make/archive/refs/tags/v${MCM_VERSION}.tar.gz" \
    "$SRC/musl-cross-make.tar.gz" "$MCM_TARBALL_SHA256"
  rm -rf "$SRC/musl-cross-make-${MCM_VERSION}"
  tar -xzf "$SRC/musl-cross-make.tar.gz" -C "$SRC" || die "解压 musl-cross-make 失败"
  cd "$SRC/musl-cross-make-${MCM_VERSION}"
  printf "TARGET = %s\nOUTPUT = %s\n" "$CROSS" "$TOOLCHAIN" > config.mak
  cat config.mak | sed 's/^/    /'
  # ⚠️ 这一步是本脚本最慢也最可能卡住的一段（实测 ~10 分钟，负载只有 8 核）。
  #    它**没有内置超时**：跑它的人要自己盯（别挂在无人看管的前台）。
  #    真卡住就停下来问人 —— 不要自己换方案：换方案 = 换整个交付形态，是产品决定。
  make -j"$NPROC" COWPATCH="$COWPATCH_SHIM" 2>&1 | tee "$LOGS/musl-cross-make.log" \
    || die "musl-cross-make 编译失败（原始输出见 $LOGS/musl-cross-make.log）"
  # ⚠️ `make` 只是**编**，不装。少这条，$TOOLCHAIN/bin 会是空的（见文件顶部 ① 条）。
  make install COWPATCH="$COWPATCH_SHIM" 2>&1 | tee -a "$LOGS/musl-cross-make.log" \
    || die "musl-cross-make install 失败（原始输出见 $LOGS/musl-cross-make.log）"
  [ -x "$TOOLCHAIN/bin/$CROSS-gcc" ] || die "musl 交叉链没产出 $TOOLCHAIN/bin/$CROSS-gcc"
fi
echo "  交叉链：$($TOOLCHAIN/bin/$CROSS-gcc --version | head -1)"

# 冒烟自验：交叉链真的能出**静态**可执行文件吗（这一步不做，后面失败会难查得多）
printf '#include <stdio.h>\nint main(void){ printf("musl-static-ok\\n"); return 0; }\n' > "$BUILD/smoke.c"
"$TOOLCHAIN/bin/$CROSS-gcc" -static -o "$BUILD/smoke" "$BUILD/smoke.c" || die "交叉链冒烟编译失败"
"$BUILD/smoke" | grep -q musl-static-ok || die "交叉链冒烟产物跑不起来"
file "$BUILD/smoke" | grep -q "statically linked" || die "交叉链冒烟产物不是静态的"
echo "  冒烟自验通过：$(file -b "$BUILD/smoke")"

# ===========================================================================
step "4/7 编静态 OpenSSL $OPENSSL_VERSION"
# ===========================================================================
fetch "https://www.openssl.org/source/openssl-${OPENSSL_VERSION}.tar.gz" \
  "$SRC/openssl.tar.gz" "$OPENSSL_TARBALL_SHA256"
rm -rf "$SRC/openssl-${OPENSSL_VERSION}"
tar -xzf "$SRC/openssl.tar.gz" -C "$SRC" || die "解压 openssl 失败"
cd "$SRC/openssl-${OPENSSL_VERSION}"
# ⚠️ 三处与任务书不同，每一处都对应一次实测失败，详见文件顶部 ③④ 条：
#   · AR 只能是不带 `r` 的 `$CROSS-ar`（`r` 由 OpenSSL 自己的 ARFLAGS=qc 提供）
#   · 必须加 `no-module`：否则 legacy provider 只是 legacy.so，静态二进制 dlopen 不了，
#     aria2c 一跑就 "OSSL_PROVIDER_load 'legacy' failed."（退出码 1，无输出）
#   · CFLAGS 不显式设 —— autoconf 会补默认的 `-g -O2`。**这里是有意不设的**：
#     一旦显式设了 CFLAGS，autoconf 的 `-O2` 会被整条丢掉（macOS 脚本踩过这个坑）。
export PATH="$TOOLCHAIN/bin:$PATH"
./Configure linux-x86_64 no-shared no-module no-tests \
  --prefix="$SSL" --openssldir="$SSL" \
  CC="$CROSS-gcc" AR="$CROSS-ar" RANLIB="$CROSS-ranlib" || die "OpenSSL Configure 失败"
make -j"$NPROC" 2>&1 | tee "$LOGS/openssl-build.log" || die "OpenSSL make 失败"
make install_sw 2>&1 | tee -a "$LOGS/openssl-build.log" || die "OpenSSL install_sw 失败"
# 两个 .a 在不在 —— 少了任何一个都说明没编成静态
ls "$SSL"/lib64/libssl.a "$SSL"/lib64/libcrypto.a >/dev/null 2>&1 \
  || die "OpenSSL 没产出静态库（$SSL/lib64/lib{ssl,crypto}.a）"
echo "  $(file -b "$SSL/lib64/libssl.a")"
echo "  $(file -b "$SSL/lib64/libcrypto.a")"

# ===========================================================================
step "5/7 编静态 aria2 $ARIA2_VERSION"
# ===========================================================================
fetch "https://github.com/aria2/aria2/releases/download/release-${ARIA2_VERSION}/aria2-${ARIA2_VERSION}.tar.gz" \
  "$SRC/aria2.tar.gz" "$ARIA2_TARBALL_SHA256"
rm -rf "$SRC/aria2-${ARIA2_VERSION}"
tar -xzf "$SRC/aria2.tar.gz" -C "$SRC" || die "解压 aria2 失败"
cd "$SRC/aria2-${ARIA2_VERSION}"
export PATH="$TOOLCHAIN/bin:$PATH"
# ⚠️ `--with-openssl=yes` + PKG_CONFIG_PATH，**不是**任务书里的 `--with-openssl=<路径>`：
#    后者在 aria2 1.37 上是**静默无效**的（configure.ac 判的是 `= "xyes"`），
#    结果是一个没有 TLS 的产物 —— 详见文件顶部 ⑤ 条。
# ⚠️ 开关清单照抄 macOS 那份（downloader/scripts/build_aria2_macos.sh:274-280），只换工具链。
export PKG_CONFIG_PATH="$SSL/lib64/pkgconfig"
./configure --host="$CROSS" --prefix="$OUT" \
  --disable-nls --disable-bittorrent --disable-metalink \
  --without-libxml2 --without-libssh2 --without-libcares --without-sqlite3 \
  --with-openssl=yes \
  ARIA2_STATIC=yes \
  CC="$CROSS-gcc" CXX="$CROSS-g++" \
  LDFLAGS="-static" 2>&1 | tee "$LOGS/aria2-build.log" || die "aria2 Configure 失败"
# 防"静默没链上 TLS"：Configure 摘要里这两行必须是 yes，否则后面全是白工。
# （用 `[[:space:]]+` 而不是空格字面量：这两行是靠列对齐的，别把对齐当契约。）
grep -qE '^SSL Support:[[:space:]]+yes' "$LOGS/aria2-build.log" \
  || die "configure 摘要里 SSL Support 不是 yes —— TLS 没链上，见文件顶部 ⑤ 条"
grep -qE '^OpenSSL:[[:space:]]+yes' "$LOGS/aria2-build.log" \
  || die "configure 摘要里 OpenSSL 不是 yes —— TLS 没链上，见文件顶部 ⑤ 条"
make -j"$NPROC" 2>&1 | tee -a "$LOGS/aria2-build.log" || die "aria2 make 失败"

# ===========================================================================
step "6/7 strip + 三条判据自验（缺一条都不算通过）"
# ===========================================================================
cd "$SRC/aria2-${ARIA2_VERSION}/src"
[ -x aria2c ] || die "没产出 $SRC/aria2-${ARIA2_VERSION}/src/aria2c"

# ---- 6a. strip：**排在判据自验之前**，让下面的三条判据跑在"真正要发布的那份字节"上 ----
# ⚠️ 顺序是有意的：先 strip 再验，才是"验的就是要发的"；先验再 strip 等于验了个中间产物
#    （strip 动的是二进制本身，签名也能被它弄坏——macOS 那份正是栽在这上面）。
# ⚠️ 为什么 Linux 要 strip 而 macOS 不要，见文件顶部那一整段（不是不一致，是约束不同）。
STRIP_SIZE_BEFORE="$(stat -c %s aria2c)"
"$TOOLCHAIN/bin/$CROSS-strip" aria2c || die "strip 失败"
STRIP_SIZE_AFTER="$(stat -c %s aria2c)"
echo "  strip: $STRIP_SIZE_BEFORE B -> $STRIP_SIZE_AFTER B"
# strip 之后必须还是个能跑的 ELF —— 万一 strip 工具链不对，这里就要响
[ "$STRIP_SIZE_AFTER" -lt "$STRIP_SIZE_BEFORE" ] \
  || die "strip 之后字节数没变小（$STRIP_SIZE_BEFORE -> $STRIP_SIZE_AFTER），工具链可疑"

FILE_OUT="$(file -b aria2c)"
echo "  file: $FILE_OUT"
echo "$FILE_OUT" | grep -q "statically linked" \
  || die "判据①：file 没说 statically linked —— 它不是真静态"

# ⚠️ 判据②**必须先把 readelf 的输出落到文件**，再对**文件** grep。
#    曾经的写法是 `NEEDED="$(readelf -d aria2c | grep NEEDED || true)"`，
#    它有一个致命塌缩：**"readelf 跑不起来"和"没有 NEEDED 行"变成同一个空串**，
#    而下一行还无条件打印"（无，静态）"—— 于是"判据②通过"这句话可以是脚本自己 echo 的，
#    不是 readelf 说的。这正是"判据可以在没真跑的情况下报通过"。
READELF_OUT="$LOGS/readelf-d.txt"
if ! readelf -d aria2c > "$READELF_OUT" 2>&1; then
  echo "---- readelf -d 的输出（$READELF_OUT）----" >&2
  sed 's/^/    /' "$READELF_OUT" >&2
  die "判据②：readelf -d 本身跑失败了 —— 此时\"没有 NEEDED\"这个结论**不成立**"
fi
echo "  readelf -d 的输出（存在 $READELF_OUT，共 $(wc -l < "$READELF_OUT") 行）："
sed 's/^/    /' "$READELF_OUT"
NEEDED="$(grep NEEDED "$READELF_OUT" || true)"
if [ -n "$NEEDED" ]; then
  echo "$NEEDED" | sed 's/^/    /' >&2
  die "判据②：readelf -d 里有 NEEDED —— 动态链接了，客户机器上会缺库"
fi
# 这句话只有在 readelf **真的跑成功**之后才允许打印（上面那个 if 已经保证了）
echo "    （readelf 跑通了，且其输出里没有 NEEDED ⇒ 静态）"

# ⚠️ --version 只跑**一次**并把输出收进变量：跑两次既慢，又会在 `| head` 那侧
#    撞上 SIGPIPE（配合 pipefail 会误判成"跑不起来"）。
VER_OUT="$(./aria2c --version)" \
  || die "判据③：aria2c --version 跑不起来（静态 TLS 没链上？见文件顶部 ④ 条）"
# ⚠️ 判据③的"含 OpenSSL"必须**只对 Configuration 段**判，不能对整段 --version 输出判：
#    判据说的是"Configuration 段含 OpenSSL"，而整段输出里别处也可能出现这个词
#    （GPL 正文、`Report bugs to` 那一带）。所以先把段切出来、断言段非空，再对**段变量** grep。
CONFIG_SECTION="$(printf '%s\n' "$VER_OUT" \
  | awk '/^\*\* Configuration \*\*/{f=1} f&&/^$/{exit} f')"
[ -n "$CONFIG_SECTION" ] \
  || die "判据③：--version 里切不出 Configuration 段（输出格式变了？）—— 判据无从谈起"
echo "  Configuration 段："
printf '%s\n' "$CONFIG_SECTION" | sed 's/^/    /'
# ⚠️ 任务书示例里的 `grep -A2 Configuration` 只看 Configuration 后面 2 行，**够不到**
#    `Libraries:` 那行 —— 别照那个 grep 判，会得到假阴性。
# ⚠️ 反过来也**不要**去校验那行的版本串：见下面那段（aria2 的版本解码对 OpenSSL 3.x 是坏的）。
printf '%s\n' "$CONFIG_SECTION" | grep -q "OpenSSL" \
  || die "判据③：Configuration 段里没有 OpenSSL —— TLS 没链上，客户用 https 镜像就会坏"

# ---------------------------------------------------------------------------
# ⚠️⚠️ 别把那行的**版本串**当版本用 —— `Libraries: OpenSSL/3.0.0o` 里的 `3.0.0o` 是**假的**
# ---------------------------------------------------------------------------
# 本脚本钉的是 OpenSSL **3.0.15**（见文件顶部 OPENSSL_TARBALL_SHA256 与第 4 步），
# 而 --version 会打印 `Libraries: OpenSSL/3.0.0o` —— 一个**根本不存在的版本号**。
# 2026-09-21 查清了，根因在 aria2 自己身上，不在依赖上（两边都是实测）：
#
#   · 真正的版本：同源码树 include/openssl/opensslv.h 里
#         # define OPENSSL_VERSION_STR  "3.0.15"
#         # define OPENSSL_VERSION_TEXT "OpenSSL 3.0.15 3 Sep 2024"
#     二进制里也找得到字面量 `3.0.15`。⇒ **链的就是 3.0.15，依赖没错。**
#
#   · 那串是怎么来的：aria2 1.37 的 src/FeatureConfig.cc:216-222
#         res += fmt("OpenSSL/%ld.%ld.%ld", OPENSSL_VERSION_NUMBER >> 28,
#                    (OPENSSL_VERSION_NUMBER >> 20) & 0xff,
#                    (OPENSSL_VERSION_NUMBER >> 12) & 0xff);
#         if ((OPENSSL_VERSION_NUMBER >> 4) & 0xff)
#           res += 'a' + ((OPENSSL_VERSION_NUMBER >> 4) & 0xff) - 1;
#     它按的是 **OpenSSL 1.1.1 及更早**的位布局（低位 nibble = 发行字母）。
#     OpenSSL 3.x 换了布局：opensslv.h 里
#         # define OPENSSL_VERSION_NUMBER  \
#             ((OPENSSL_VERSION_MAJOR<<28) | (OPENSSL_VERSION_MINOR<<20) \
#              | (OPENSSL_VERSION_PATCH<<4) | _OPENSSL_VERSION_PRE_RELEASE)
#     3.0.15 ⇒ 3<<28 | 0<<20 | 15<<4 | 0xf = **0x300000ff**。
#     套进上面那段解码：major=3、minor=0、`>>12 & 0xff`=**0**（patch 不在那一段了）、
#     低位 0xf 非零 ⇒ 追加 'a'+15-1 = **'o'** ⇒ 拼出 **"OpenSSL/3.0.0o"**。
#     （本机复算过：0x300000ff 按那段代码得 `OpenSSL/3.0.0o`，与实测输出逐字符相同。）
#
#   ⇒ 这是 aria2 的**显示缺陷**，不是我们的依赖出了问题；两条判据（含 OpenSSL）照样成立。
#     **但绝不要**把版本串纳入判据 —— 它会随着 OpenSSL 版本变化给出噪声，
#     而且对 3.x 永远是错的。要判版本就读 opensslv.h / 查上面那条 sha256。

# ===========================================================================
step "7/7 就位并打印产物指纹"
# ===========================================================================
# 发布到任务书约定/接口路径（先写临时文件再 mv：中途失败不留半截二进制）
install -m 0755 "$SRC/aria2-${ARIA2_VERSION}/src/aria2c" "$OUT/aria2c-linux-x86_64.tmp"
mv -f "$OUT/aria2c-linux-x86_64.tmp" "$OUT/aria2c-linux-x86_64"

ARTIFACT="$OUT/aria2c-linux-x86_64"
echo ""
echo "==================================================================="
echo "产物: $ARTIFACT"
echo "字节数: $(stat -c %s "$ARTIFACT")"
echo "sha256: $(sha256sum "$ARTIFACT" | awk '{print $1}')"
echo "file:   $(file -b "$ARTIFACT")"
echo "==================================================================="
echo ""
echo "⚠️ 这个 sha256 **只对本次构建有效**：aria2 把编译时刻编进了二进制"
echo "   （--version 里那行 'on <日期>'），重跑必然得到不同的 sha256。"
echo "   要入库时以当次的值为准，并同步更新核心侧的 ARIA2C_EMBED_SHA256。"
echo ""
echo "⚠️ 上面那份是 **strip 之后**的字节（第 6 步做的，${CROSS}-strip）。
   不 strip 的话约 79 699 424 B —— 差的 73 MB 全是 autoconf 默认 -g 带来的调试信息。
   与 macOS 那份"不 strip"不矛盾：那边是签名 + 无可去之物，见文件顶部那一整段。"
echo "完成。"
