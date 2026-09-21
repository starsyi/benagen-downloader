#!/usr/bin/env bash
# x86_64 构建的**工具链预检**：拿不到 x86_64 的 std 就大声失败，并给出确切配方。
#
# 这个脚本是阶段 F「第 5 步设计决定」的落地物。决定与理由：
#
#   **在"构建时按需装工具链"与"预检失败并给出确切配方"之间，选后者。**
#
#   理由一（为什么不做按需装）：构建脚本不该在背后下载并安装一整套工具链。
#   那是个 3–4 分钟的网络动作 + 一次机器级安装，藏在 `build_app_macos.sh` 里执行，
#   失败时的表现是"编到一半炸"，而且为了让 rustup 的 cargo 赢过 Homebrew 的 cargo，
#   脚本还得自己改 PATH —— 那正是本项目最忌讳的"静默换个东西来跑"。
#
#   理由二（预检为什么够）：判据是"干净检出 + 按文档做 ⇒ 能得到 x86_64 构建"。
#   配方就在下面 `recipe()` 里（**与报错信息是同一条字符串**，不会漂移），
#   执行一次、约 3–4 分钟、之后每次构建只需一句 `export PATH`。
#   一次性动作交给开发者显式做，比构建脚本替所有人猜要诚实。
#
#   理由三（F-2 的字面要求）："拿不到 x86_64 的 std ⇒ 大声失败并给出可执行的补救"。
#   没有 rustup 时，cargo 自己那句 `rustup target add x86_64-apple-darwin`
#   **照抄不了**（本机没有 rustup）—— 响但不**可执行**。补上这一条就是本脚本的全部价值。
#
# 用法（x86_64 的构建路径都该先跑它）：
#   bash core/scripts/preflight_x86_64_toolchain.sh && \
#     cargo build --release --target x86_64-apple-darwin
#
#   ⚠️ 本脚本**只管"编"**。除了它，x86_64 还有一条**运行**前置：Apple Silicon 上的
#   Rosetta 2（本机所有 x86_64 执行都经它翻译）。构建**不需要** Rosetta（交叉编译靠
#   std + 链接器），所以这一条在这里只**警告**、不拦：
#     - 想"出一份 Intel 包发出去"的人，在没装 Rosetta 的机器上照样出得来，而且那份包
#       是完全正确的（架构自验走 `lipo`，不经过 Rosetta）；
#     - 但**要在这台机器上跑它**（`macos/scripts/e2e_shell_macos.sh x86_64` 真起内核）
#       就必须有 Rosetta —— 那条脚本会**硬拦**在这里，并给出同一条安装命令。
#   这个分工是刻意的：拦在构建上会让"只想出包"的人无谓地卡住，拦在运行前则恰好在
#   需要它的那一步、且不会把失败点推到"启动取证"那种看不懂的报错上。
#
# 退出码：0 = 可以编（Rosetta 缺失时仍是 0，但会打印警告）；1 = 工具链缺失（配方已打印）；
#         2 = 用法错误。
set -uo pipefail

TARGET="x86_64-apple-darwin"

# ⚠️ CARGO_HOME 只解析一次，**检查与配方共用同一个值**。
#    曾经这里踩过一次"响但不可补救"：检查用的是 `${CARGO_HOME:-$HOME/.cargo}`，
#    而打印出来的配方写死 `$HOME/.cargo/bin` —— 在显式设了 CARGO_HOME 的环境
#    （CI 常见）里，照抄那句 `export PATH=…` **不生效**，人照着做完还是编不过，
#    于是这条补救等于没给。F-2 要的"可执行的补救"就是这么丢的。
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
CARGO_BIN_DIR="$CARGO_HOME_DIR/bin"

# 配方只写一遍：报错信息与文档是同一段文本，不会各自漂移。
# 里面用 `__CARGO_BIN_DIR__` 占位，由下面的 sed 换成**上面那个变量**的值
# （heredoc 用引号形式，避免配方里的反引号被当成命令替换）。
recipe() {
  cat <<'RECIPE' | sed "s|__CARGO_BIN_DIR__|$CARGO_BIN_DIR|g" >&2

补救（一次性，约 3–4 分钟，视网络）：

  curl -fsSL -o /tmp/rustup-init.sh https://sh.rustup.rs
  sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain 1.89.0
  export PATH="__CARGO_BIN_DIR__:$PATH"          # ← 每次构建 x86_64 之前都要先执行这句
  rustup target add x86_64-apple-darwin

  # 之后：
  export PATH="__CARGO_BIN_DIR__:$PATH"
  cargo build --release --target x86_64-apple-darwin

⚠️ 上面那个路径是本脚本按 CARGO_HOME 解析出来的**确切值**，照抄即可；
   **不要**把它换成 `$HOME/.cargo/bin` 之类想当然的写法 —— 那在设过 CARGO_HOME
   的环境里是错的（而 CI 上通常都设了）。
⚠️ `--no-modify-path` 是**故意**的：rustup 因此不碰 shell 配置，本机默认的 `cargo`
   （Homebrew 那份）**仍然是默认**，arm64 那条路一个字节都不变（F-4）。
   代价是 x86_64 构建前**必须**自己把 __CARGO_BIN_DIR__ 放到 PATH 最前面 ——
   漏了这句的表现是 `cargo` 仍解析到 Homebrew 那份、照样报 E0463。
   下面有一项专门检查这一条。
⚠️ `--default-toolchain 1.89.0` 与本机 Homebrew 的 rustc 同版本：两个架构**应当由同一个
   编译器**编出来。Homebrew 升版后记得同步，否则两边编出来的东西口径不同。
⚠️ 在已装 Homebrew Rust 的机器上，安装器会打印
   "error: cannot install while Rust is installed" —— 那是**可忽略的**（带 `-y` 会继续）。
RECIPE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  # ⚠️ **不要用硬编码行号**（这里原来是 `sed -n '2,30p'`）：文件头一长就静默截断。
  #    本轮就是这么被咬的 —— 给头部加了 Rosetta 那段之后，`-h` 停在句子中间，
  #    连**退出码约定**与整段 Rosetta 说明都看不到了（而它们正是这一轮刚改过的内容）。
  #    判据改成**跟着文件头走**：从第 2 行打印到第一行以 `set ` 开头的正文为止（不含它）。
  awk 'NR == 1 { next } /^set / { exit } { sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  exit 0
fi

echo "==> 预检 x86_64 工具链（目标 ${TARGET}）"

if ! command -v rustup >/dev/null 2>&1; then
  echo "错误：PATH 里没有 rustup —— 无从确认 $TARGET 的 std 是否可用。" >&2
  echo "      当前 cargo: $(command -v cargo 2>/dev/null || echo '（PATH 里没有 cargo）')" >&2
  recipe
  exit 1
fi

# ⚠️ 这一条最容易被漏掉：rustup 装好了、target 也加了，但 PATH 顺序不对时
#    `cargo` 仍然解析到 Homebrew 那份，编 x86_64 照样 E0463 —— 而报错指向的
#    却是"目标没装"，把人往错的方向引。
CARGO_BIN="$(command -v cargo || true)"
if [ "$CARGO_BIN" != "$CARGO_BIN_DIR/cargo" ]; then
  echo "错误：cargo 解析到了 ${CARGO_BIN}，不是 rustup 管的那份（$CARGO_BIN_DIR/cargo）。" >&2
  echo "      这多半是 PATH 顺序问题：rustup 的 bin 目录没排在其它 Rust 安装之前。" >&2
  recipe
  exit 1
fi

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "错误：rustup 没装 $TARGET 的 std（已装：$(rustup target list --installed | tr '\n' ' ')）" >&2
  echo "      直接编会在**编依赖**阶段报 E0463（can't find crate for \`core\`）。" >&2
  recipe
  exit 1
fi

# ---------------------------------------------------------------------------
# 运行前置：Rosetta 2（**只警告，不拦** —— 理由见文件头）
# ---------------------------------------------------------------------------
# 探针用 `arch -x86_64 /usr/bin/true`：这是"能不能执行一个 x86_64 二进制"的最小可执行判据
# （不是"看 /Library/Apple/... 在不在"那种间接证据）。仅在 Apple Silicon 上才需要。
ROSETTA_OK=1
if [ "$(uname -m)" = "arm64" ]; then
  if ! /usr/bin/arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
    ROSETTA_OK=0
  fi
fi

echo "==> 通过：cargo = ${CARGO_BIN}，$TARGET 的 std 已装"

if [ "$ROSETTA_OK" = 0 ]; then
  echo "" >&2
  echo "⚠️ 警告：本机（Apple Silicon）没装 Rosetta 2 —— x86_64 的产物**编得出来、跑不起来**。" >&2
  echo "         构建不受影响（交叉编译不需要 Rosetta），所以这里**不拦**；" >&2
  echo "         但要在本机**跑**它（macos/scripts/e2e_shell_macos.sh x86_64）会**硬失败**。" >&2
  echo "" >&2
  echo "         装它（一次性，需要联网，约 1 分钟）：" >&2
  echo "           softwareupdate --install-rosetta --agree-to-license" >&2
  echo "" >&2
fi
