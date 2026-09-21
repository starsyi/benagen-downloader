#!/usr/bin/env bash
#
# windows/scripts/make_wire_fixtures.sh —— **一条命令重新生成前端那套无头验收用的载荷夹具**。
#
#    bash windows/scripts/make_wire_fixtures.sh [输出路径]
#
# 默认输出：`windows/scripts/frontend-stub/wire-fixtures.json`
#
# ---------------------------------------------------------------------------
# 它为什么存在（**这是一次审查换来的**）
# ---------------------------------------------------------------------------
#   前端那套无头验收（iframe + 事后注入 `window.__TAURI__`）原本吃的是**手抄的**
#   stub 载荷。手抄的夹具与前端代码会**犯同一个错** —— 两边都信了同一份简报里
#   两个根本不存在的字段名（`state.label` / `state.color`，而线上 `state` 只是一个
#   **变体名**字符串），于是三十几条断言**全绿**，而真载荷下：
#
#     · **状态列整列空白**（前端读 `row.state.label` 得到 `undefined`）；
#     · **双击目录变成「下载整个目录」**（线上 `kind` 是 `"Dir"`，前端按 `"dir"` 比）。
#
#   ⇒ 那套测试验证的是**错误假设的自洽**。修法不是"再多写几条断言"，而是
#     **把夹具的源头换成 Rust**：本脚本调 `shell-core` 的一个 example
#     （`examples/dump_wire_fixtures.rs`），逐个调 `api::tree::*` /
#     `api::enqueue::*` / `api::state::*` 那几个**纯函数**，把它们真的会发出去的
#     字节倒成一份 JSON。前端那边**一个字段名都不再手抄**。
#
#   ⚠️ 任务 11/12/13 复用同一条命令：往那个 example 里加自己那一屏的端点即可
#      （生成器在 `shell-core` 里，不在本脚本里 —— 本脚本只负责"找到 cargo、指对路径"）。
#
#   ⚠️ 夹具的**位置**是承重的：它**不能**住在 `windows/web/` 下 ——
#      `shell-win/tauri.conf.json` 的 `build.frontendDist = "../web"` 意味着
#      那个目录下的**任何文件都会被编进 exe**（那件事由 `check_frontend_copy.sh`
#      的文件清单判据守着）。住这里不会被编进产物。
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_frontend_copy.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 生成成功
#   1 = 环境问题（缺 cargo / 生成器编不过 —— W-2：大声说 + 给可执行的补救）
#   2 = 用法错误（给了不认识的参数）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEFAULT_OUT="$REPO/windows/scripts/frontend-stub/wire-fixtures.json"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# ---------------------------------------------------------------------------
# 参数：至多一个（输出路径）。不认识的参数直接报错，**绝不透传**。
# ---------------------------------------------------------------------------
out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -*)
      echo "make_wire_fixtures.sh: 不认识的参数：$1" >&2
      echo "make_wire_fixtures.sh: 用法：bash windows/scripts/make_wire_fixtures.sh [输出路径]" >&2
      exit 2
      ;;
    *)
      [ -z "$out" ] || { echo "make_wire_fixtures.sh: 只接一个输出路径（多的：$1）" >&2; exit 2; }
      out="$1"
      ;;
  esac
  shift
done
[ -n "$out" ] || out="$DEFAULT_OUT"

# ---------------------------------------------------------------------------
# cargo 在哪（**这一步有一条本仓库特有的纪律**）
# ---------------------------------------------------------------------------
# ⚠️ **优先 `$HOME/.cargo/bin/cargo`，不要用 PATH 上那个**：本机 PATH 上的
#    `cargo` 是 Homebrew 装的那一份（`/opt/homebrew/bin/cargo`），
#    而本项目的工具链是 rustup 装在 `~/.cargo` / `~/.rustup` 下的那一份
#    （Windows 客户端的交叉编译脚本用的是同一个口径）。
#    两套混用会让"编得过/编不过"取决于谁的 PATH 在前面。
#    ⇒ 顺序：`$BENAGEN_CARGO`（显式覆盖）→ `$HOME/.cargo/bin/cargo` → 大声失败。
cargo="${BENAGEN_CARGO:-}"
if [ -z "$cargo" ]; then
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    cargo="$HOME/.cargo/bin/cargo"
  else
    die "找不到 cargo（\$HOME/.cargo/bin/cargo 不存在，且没有设 BENAGEN_CARGO）。
      本脚本要它来跑生成器（\`windows/shell-core/examples/dump_wire_fixtures.rs\`）。
      补救（按你的平台挑一行）：
        mac/linux: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
        Windows:   装 MSYS2（https://www.msys2.org）后 \`pacman -S mingw-w64-x86_64-rust\`
      ⚠️ 也**可以**用 BENAGEN_CARGO=/path/to/cargo 显式指定。
      ⚠️ 不要拿 Homebrew 那份（/opt/homebrew/bin/cargo）：本项目用的是 \$HOME/.cargo 下那套。"
  fi
fi

[ -d "$REPO/windows/shell-core/examples" ] || die "找不到 $REPO/windows/shell-core/examples
      —— 生成器不在场（要么 checkout 不完整，要么它被搬走了）。这不是\"跳过\"。"
[ -f "$REPO/windows/shell-core/examples/dump_wire_fixtures.rs" ] || die "找不到生成器：
      $REPO/windows/shell-core/examples/dump_wire_fixtures.rs"

mkdir -p "$(dirname "$out")" || die "建不出输出目录：$(dirname "$out")"

note "用 $cargo 跑生成器（-p shell-core --example dump_wire_fixtures）"
"$cargo" run -q --manifest-path "$REPO/windows/Cargo.toml" \
  -p shell-core --example dump_wire_fixtures -- "$out" || {
  die "生成器失败了（cargo 的非零退出码见上）。
      ⚠️ 它自己带自检（行数 / state_label 非空 / kind 的形态），失败时**不会写出半份夹具**。"
}

[ -s "$out" ] || die "生成器跑完了，但 $out 是空的（或没写出来）—— 这不是\"成功\"。"
note "完成：$out"
