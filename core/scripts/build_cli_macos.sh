#!/usr/bin/env bash
# 构建 `benagen-dl` 的 macOS 分发产物：dist_cli/benagen-dl-macos-{arm64,x86_64}
#
# 用法：
#   bash core/scripts/build_cli_macos.sh              # 两个架构都出（默认）
#   bash core/scripts/build_cli_macos.sh arm64        # 只出 Apple Silicon 那份
#   bash core/scripts/build_cli_macos.sh x86_64       # 只出 Intel 那份
#   BENAGEN_ARCH=x86_64 bash core/scripts/build_cli_macos.sh
#
# ⚠️ **x86_64 那条路要求 `cargo` 解析到 rustup 那份**（`$CARGO_HOME/bin/cargo`）：
#       export PATH="$HOME/.cargo/bin:$PATH"
#    漏了这句的表现是预检**在编译之前**就失败并打印配方（不会编到一半才炸）。
#    本脚本**不替调用者改 PATH**（那正是本项目最忌讳的"静默换个东西来跑"），
#    只让 `core/scripts/preflight_x86_64_toolchain.sh` 大声说话。arm64 那条路不经过预检，
#    本机默认的 cargo（Homebrew 那份）照样能出 Apple Silicon 产物 ——
#    ⚠️ **但字节不同**（2026-09-21 实测）：同一棵树、同一个脚本，
#       rustup 那份   ⇒ 7871968 字节 / sha256 `c40130f7815d4b18…`
#       Homebrew 那份 ⇒ 7960944 字节 / sha256 `dd91cdcf…`
#    两次**都退出码 0、都不告警**。所以"跑两遍 sha256 逐字相同 = 可重现"
#    **只在同一份 cargo 内成立**，不是跨工具链的性质。
#    ⇒ 要复现交付的那份字节（rustup 出的），**必须先 `export PATH="$HOME/.cargo/bin:$PATH"`**；
#      上面那句"照样能出"指的是"**能出**"，不是"出同一份字节"。
#
# 产物与用法（`dist_cli/` 不入库，见仓库根 .gitignore —— **认 sha256 不认文件名**）
#     dist_cli/benagen-dl-macos-arm64    ← Apple Silicon（M 系列）
#     dist_cli/benagen-dl-macos-x86_64   ← Intel
#     $ dist_cli/benagen-dl-macos-arm64 <交付码> -o /path/to/dir
#
# ---------------------------------------------------------------------------
# 为什么与 `macos/scripts/build_app_macos.sh` 是**两个**脚本，而不是一个
# ---------------------------------------------------------------------------
#   那个出的是 `.app`（SwiftUI 壳 + 包内内核 + 包内 aria2c + plist + 品牌资产），
#   这个出的是**单个可执行文件**（aria2c 内嵌在字节里，`include_bytes!`）。
#   两者的输入、产物形态、自验判据（包内三个二进制 vs 独立一个）都不同，
#   合成一个只会让两条路的纪律互相稀释。**共用的是纪律**，不是代码：
#     · 架构自验**不听参数、只看字节**（`lipo -archs`）；
#     · 失败就大声说、并给出补救方向，绝不"降级继续"；
#     · 产物先在暂存目录里验完，最后才 `mv` 进 dist（不留半成品）。
#
# ---------------------------------------------------------------------------
# 架构自验（为什么必须有，以及为什么它比"我传了 --target"强）
# ---------------------------------------------------------------------------
#   探路实测：`arch -x86_64 clang` **仍然编出 arm64** —— 也就是说
#   "参数传对了"与"产出一个名叫 x86_64 的 arm64 二进制"完全可以同时成立。
#   所以判据是**产物字节**：`lipo -archs` 与目标架构**恰好相等**。它同时挡住两种缺陷：
#     · 编成了别的架构（x86_64 目标、arm64 产物）；
#     · 编成了通用二进制（报 `x86_64 arm64`）—— 本脚本出的是**单架构**产物，
#       混进另一个架构同样是缺陷。
#
# ⚠️ arm64 那一支刻意用**原生构建**（`cargo build --release`，**不带 --target**），
#    与 `macos/scripts/build_app_macos.sh` 的既有形态一致。代价是：在非 Apple Silicon
#    机器上跑默认调用时，它编出来的**永远是宿主架构**，而产物名写的是 arm64 ——
#    这正是下面那道架构自验要抓的形态（它会说出"宿主与目标一致"那半话术）。
#
# ---------------------------------------------------------------------------
# 自验的四条（每条都真判，不是打印）
# ---------------------------------------------------------------------------
#   1. `lipo -archs` 恰好等于目标架构；
#   2. `--version` 打印的**内嵌 aria2c 摘要前 12 位**，与 `core/assets/aria2c-macos-<架构>`
#      的 sha256 前 12 位**逐字相同** —— 这一条把"编进去的是哪一份资产"钉死
#      （`--version` 那一行与内核释放临时文件的命名同源，见 `cli/mod.rs::version_line`）；
#   3. 资产摘要用 `shasum -a 256` 与 `openssl dgst -sha256` **各算一遍**，两个结果必须一致
#      —— 与 `daemon.rs` 里 `ARIA2C_EMBED_SHA256` 的填法同源（那一条也是这么核的）；
#   4. `--help` 出得来、且是中性的中文用法说明（打不开的产物 = 客户拿到就卡住）。
set -euo pipefail

ARCH_ARG="${1:-${BENAGEN_ARCH:-}}"
BIN_NAME="benagen-dl"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"      # core/
REPO="$(cd "$ROOT/.." && pwd)"                                # 仓库根
DIST="$REPO/dist_cli"
PREFLIGHT="$ROOT/scripts/preflight_x86_64_toolchain.sh"

# 摘要前 12 位：与内核的 `EMBED_HASH_PREFIX_LEN` 同值。**在这里再写一遍是有意的**——
# 本脚本是"验收"侧，它的判据不该是"内核说它印了 12 位"；两处不一致时会在这里红。
HASH_PREFIX_LEN=12

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '    \033[32m✓\033[0m %s\n' "$*"; }
die()  { printf '\n!! %s\n' "$*" >&2; exit "${2:-1}"; }

# ---------------------------------------------------------------------------
# 新鲜度自验：**产物必须比它的全部输入都新**（具名函数，可单独行使）
# ---------------------------------------------------------------------------
# 为什么现有的四条判据都替代不了它：那四条钉的是"**内嵌的 aria2c 是哪一份**"
# （架构 / 静态 / 摘要 / --help），**钉不住"这份二进制是不是刚编出来的"**。
# 一份陈旧、但内嵌资产恰好没换过的产物，能把那四条全部过掉 ——
# 而它承载的是**旧源码**。
#
# 输入清单 = **cargo build --release 真正的输入**：`src/`、`assets/`（include_bytes!
# 与 build.rs 都读它）、`build.rs`、`Cargo.toml`、`Cargo.lock`。
# ⚠️ **`tests/` 刻意不在清单里**：release 二进制不依赖它，把它算进来会让
#    "改一行测试 ⇒ 必须重编一份字节完全相同的二进制才算新鲜"，那是一条假红。
#    测试那侧由 `cargo test` 单独把关（Linux 脚本的第 4 步）。
#
# 判据交给 `find -newer` 逐文件判，不自己解析 mtime。
# 返回：0 = 新鲜；1 = 陈旧或判据不成立（比它新的输入已打到 stderr）。
verify_fresh() {  # $1 = 二进制，$2 = 源码根（core/）
  local bin="$1" root="$2" newer
  # **判据的适用前提先立住**：少任何一条，就**判死**而不是返回"新鲜"
  # —— "判据跑不起来"与"判据通过"是两件事。
  [ -f "$bin" ] \
    || { printf '新鲜度判据不成立：找不到产物 %s\n' "$bin" >&2; return 1; }
  [ -f "$root/Cargo.toml" ] \
    || { printf '新鲜度判据不成立：%s 不像源码根（没有 Cargo.toml）\n' "$root" >&2; return 1; }
  newer="$(find "$root/src" "$root/assets" "$root/build.rs" \
                 "$root/Cargo.toml" "$root/Cargo.lock" \
                 -type f -newer "$bin" 2>/dev/null | head -20)"
  if [ -n "$newer" ]; then
    printf '比这份产物更新的输入（最多列 20 条）：\n%s\n' "$newer" >&2
    return 1
  fi
  return 0
}

# 暂存目录：产物在里面**验完之后**才 `mv` 进 dist_cli。
# 用 EXIT 陷阱而不是 RETURN：本脚本的失败路径里有直接 `exit` 的分支（如架构不符），
# 只挂 RETURN 的话那些分支会把一个 `.stage.*` 目录留在 dist_cli 里 —— 那正是
# "不留半成品"这条纪律要避免的形态（名字里带 benagen-dl 的残件最容易被误当成产物）。
mkdir -p "$DIST"
STAGE="$(mktemp -d "$DIST/.stage.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

# **本次**产出（收尾要逐条列，并说"这些各自过了自验"）。
# ⚠️ 不能拿 `find "$DIST" -name 'benagen-dl-macos-*'` 代替它：那会把 dist_cli/ 里
#    **上一次留下的**、本次根本没重出的产物一并列出来，然后替它们背书说"过了自验"
#    —— 本脚本的自验只对本次编出来的那一份负责。
PRODUCED=""

# ---------------------------------------------------------------------------
# 架构 → 三件事：cargo 怎么编、产物在哪、内嵌哪一份 aria2c
# ---------------------------------------------------------------------------
case "$(uname -m)" in
  arm64)  HOST_ARCH="arm64"  ;;
  x86_64) HOST_ARCH="x86_64" ;;
  *)      HOST_ARCH="$(uname -m)" ;;
esac

build_one() {  # $1 = arm64 / x86_64
  local arch="$1" target="" bin="" aria2c_name dist_name
  case "$arch" in
    arm64)
      target=""                                       # 空 = 原生构建（与 build_app_macos.sh 同形）
      bin="$ROOT/target/release/$BIN_NAME"
      ;;
    x86_64)
      target="x86_64-apple-darwin"
      bin="$ROOT/target/x86_64-apple-darwin/release/$BIN_NAME"
      ;;
    *) die "不认识的架构 '$arch'（只支持 arm64 / x86_64）" 2 ;;
  esac
  aria2c_name="aria2c-macos-$arch"
  dist_name="benagen-dl-macos-$arch"
  local aria2c_src="$ROOT/assets/$aria2c_name"

  # 入库资产的存在性检查放在**编译之前**：缺件的正确处置是"立刻说清楚"，
  # 而不是等几分钟的编译跑完再让 `--version` 对不上摘要。
  [ -s "$aria2c_src" ] \
    || die "找不到内嵌资产 ${aria2c_src}（入库文件；重新产出的命令见 downloader/scripts/build_aria2_macos.sh）" 2

  if [ -n "$target" ]; then
    log "[$arch] x86_64 工具链预检（${PREFLIGHT}）"
    [ -f "$PREFLIGHT" ] || die "找不到 $PREFLIGHT —— 它是 x86_64 构建路径的前置" 2
    bash "$PREFLIGHT" || die "x86_64 工具链预检没过（配方见上）。**不会**退回去用 arm64 充数（不许静默降级）。" 2
  fi

  log "[$arch] 编译（cargo build --release${target:+ --target $target}）"
  ok "cargo = $(command -v cargo) ($(cargo --version | head -1))"
  if [ -n "$target" ]; then
    (cd "$ROOT" && cargo build --release --target "$target")
  else
    (cd "$ROOT" && cargo build --release)
  fi
  [ -x "$bin" ] || die "cargo build 成功，却找不到可执行文件 $bin" 2

  # ---- 自验 0：新鲜度（**验的是 cargo 的产物，不是下面那份拷贝**）----------
  # ⚠️ 必须在 `cp` **之前**验：`cp` 会把副本的 mtime 刷成"现在"，
  #    拿副本去比就成了一条**永远为真**的判据。判据要问的是
  #    "这份二进制是不是从当前这棵树编出来的"。
  log "[$arch] 新鲜度自验（产物 vs ${ROOT} 的输入）"
  verify_fresh "$bin" "$ROOT" \
    || die "产物比它的输入还旧 —— 这份二进制不是从当前这棵树编出来的（上面列了更新的输入）。
     一份陈旧但内嵌摘要恰好对得上的产物，能把架构/静态/摘要/--help 四条全部过掉，
     所以这一条是必须的。先查：cargo build 是不是被跳过了（比如 target/ 是别人拷来的）。" 1
  ok "产物比 src/ assets/ build.rs Cargo.toml Cargo.lock 都新"

  # ---- 产物先在暂存目录里验完，最后才换位（不留半成品）----------------------
  local out="$STAGE/$dist_name"
  cp "$bin" "$out"
  chmod +x "$out"

  # ---- 自验 1：架构（**只看字节**）----------------------------------------
  local got
  got="$(lipo -archs "$out" 2>/dev/null || true)"
  if [ "$got" != "$arch" ]; then
    printf '\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
    printf '!! 产物架构不对 —— %s 没有做出来\n' "$dist_name" >&2
    printf '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n' >&2
    printf '  文件：%s\n  期望：%s\n  实际：%s\n  宿主：%s\n' "$out" "$arch" "${got:-（lipo 读不出）}" "$HOST_ARCH" >&2
    if [ "$HOST_ARCH" = "$arch" ]; then
      printf '\n  宿主与目标**一致** ⇒ 问题不在交叉编译，而在「产物没被重新生成」：\n' >&2
      printf '    rm -f %s && bash %s %s\n' "$bin" "${BASH_SOURCE[0]}" "$arch" >&2
    else
      printf '\n  宿主与目标**不一致**（%s → %s）⇒ 这正是本检查存在的前提：\n' "$HOST_ARCH" "$arch" >&2
      printf '    x86_64 要交叉编译（--target x86_64-apple-darwin），且 cargo 必须是 rustup 那份；\n' >&2
      printf '    arm64  是本脚本的「原生构建」那一支，在非 Apple Silicon 机器上会编出宿主架构。\n' >&2
    fi
    printf '\n  **不许**跳过这一步继续（那是「静默产出一个坏产物」）。\n' >&2
    exit 1
  fi
  ok "lipo -archs = $got"
  printf '      %s\n' "$(file -b "$out")"

  # ---- 自验 2/3：内嵌 aria2c 的摘要 ---------------------------------------
  # 资产摘要**两条路各算一遍**（见文件头第 3 条）：两个工具实现的都是同一件事，
  # 各算一遍才有互相印证的意义；只算一遍等于把"某个工具的输出"当成了真相。
  local want_shasum want_openssl want
  want_shasum="$(shasum -a 256 "$aria2c_src" | awk '{print $1}')"
  want_openssl="$(openssl dgst -sha256 "$aria2c_src" | awk '{print $NF}')"
  [ "$want_shasum" = "$want_openssl" ] \
    || die "资产 $aria2c_name 的 sha256 两个工具给出了不同的值（shasum=${want_shasum} openssl=${want_openssl}）" 1
  want="${want_shasum:0:$HASH_PREFIX_LEN}"
  ok "内嵌资产 $aria2c_name 的 sha256 前 ${HASH_PREFIX_LEN} 位 = ${want}（两条工具一致）"

  local ver
  ver="$("$out" --version)" || die "产物跑不起来：$out --version 退出码非 0" 1
  printf '      $ %s --version\n      %s\n' "$dist_name" "$ver"
  # `--version` 的一行形如 `benagen-dl 0.1.0（内嵌 aria2c e167cf7d1234）`。
  # 判据是**逐字**：把括号里那串抠出来与资产摘要比。抠不出来（格式变了）= 判死，
  # 因为"格式变了"会让这条判据静默失效 —— 而它正是本任务 @6 要的那一条。
  local got_hash
  got_hash="$(printf '%s' "$ver" | sed -n 's/.*内嵌 aria2c \([0-9a-f]\{12\}\).*/\1/p')"
  [ -n "$got_hash" ] \
    || die "--version 的输出里抠不出内嵌 aria2c 的 ${HASH_PREFIX_LEN} 位摘要（输出：${ver}）——判据失效，按失败处理" 1
  [ "$got_hash" = "$want" ] \
    || die "编进去的 aria2c 与 core/assets/$aria2c_name 不是同一份：--version 说 ${got_hash}，资产是 ${want}" 1
  ok "--version 的摘要与资产逐字一致（${got_hash}）"

  # ---- 自验 4：--help 出得来、是中文用法说明 ------------------------------
  local help_out
  help_out="$("$out" --help)" || die "产物跑不起来：$out --help 退出码非 0" 1
  printf '%s' "$help_out" | grep -q '^用法：' \
    || die "--help 的输出里没有「用法：」—— 用法说明没印出来" 1
  printf '%s' "$help_out" | grep -q '退出码：' \
    || die "--help 的输出里没有「退出码：」—— 退出码表没印出来（它是给客户的对外契约）" 1
  ok "--help 可读（$(printf '%s\n' "$help_out" | head -1)）"

  # ---- 换位 ----------------------------------------------------------------
  mv -f "$out" "$DIST/$dist_name"
  local size hash
  size="$(stat -f '%z' "$DIST/$dist_name")"
  hash="$(shasum -a 256 "$DIST/$dist_name" | awk '{print $1}')"
  PRODUCED="${PRODUCED}${DIST}/${dist_name}"$'\n'
  log "[$arch] 产物"
  printf '    路径   ：%s\n    字节数 ：%s\n    sha256 ：%s\n' "$DIST/$dist_name" "$size" "$hash"
}

# ---------------------------------------------------------------------------
# 默认（不带参数）= 两个架构都出。**x86_64 的预检提到最前面**：
# 它的前置是"可提前知道的"，让人在等几分钟编译之前就知道这一趟出不了 Intel 产物。
# 只要 Apple Silicon 那份的人请显式传 `arm64` —— 那条路完全不经过预检。
# ---------------------------------------------------------------------------
case "$ARCH_ARG" in
  "")
    [ -f "$PREFLIGHT" ] || die "找不到 $PREFLIGHT —— 默认调用（两个架构都出）以它为前置" 2
    log "默认调用：先做 x86_64 工具链预检（两个架构都要出；只要 Apple Silicon 那份请传 arm64）"
    bash "$PREFLIGHT" \
      || die "x86_64 工具链预检没过（配方见上）。**不会**只出一半就当成功。
     只要 Apple Silicon 那份：bash ${BASH_SOURCE[0]} arm64" 2
    build_one arm64
    build_one x86_64
    ;;
  arm64)  build_one arm64  ;;
  x86_64) build_one x86_64 ;;
  -h|--help)
    awk 'NR == 1 { next } /^set / { exit } { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
    ;;
  *) die "不认识的参数 '$ARCH_ARG'（只支持 arm64 / x86_64；不带参数 = 两个都出）" 2 ;;
esac

log "完成"
# 只列**本次**产出的（见 `PRODUCED` 旁边那段）；`dist_cli/` 里别的文件是上一次留下的，
# 本次没有重出、本脚本也没有验过它们。
printf '%s' "$PRODUCED" | sed '/^$/d; s|^|    |'
printf '\n    上面列出的**本次产物**各自过了：新鲜度（比 src/assets/build.rs/Cargo.* 新）、\n'
printf '    架构（lipo -archs）、内嵌 aria2c 摘要（--version 与 core/assets/ 那一份逐字一致）、\n'
printf '    --help 可读。\n'
printf '    ⚠️ 本脚本**没有**为它们声明最低系统版本（minos）—— 那一条不属于它的判据；\n'
printf '       要那个读数自己看：otool -l dist_cli/benagen-dl-macos-arm64 | grep -A3 LC_BUILD_VERSION\n'
