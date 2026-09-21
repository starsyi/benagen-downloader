#!/usr/bin/env bash
#
# 稳定地跑 swift test。参数原样透传给 swift test（例如 --filter FooTests）。
#
# 为什么需要这个脚本：
#   本机只有 Command Line Tools，没有完整 Xcode。CLT 把测试用的宏插件放在一个
#   **子目录** 里：
#       $(xcode-select -p)/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib
#   而 libObservationMacros.dylib / libSwiftMacros.dylib 就在 plugins/ 本级。
#   SwiftPM 生成的 -load-resolved-plugin 参数只覆盖 plugins/ 本级，够不到 testing/。
#   于是裸 `swift test` 约一半的概率在编译测试目标时报：
#       error: external macro implementation type
#              'TestingMacros.TestDeclarationMacro' could not be found
#              for macro 'Test'; plugin for module 'TestingMacros' not found
#   这与被测代码无关——/tmp 里一个 4 行的最小包同样复现。
#
#   实测（同一棵树、同一份代码）：
#       裸 swift test        6 次里红 3 次
#       本脚本               6 次全绿
#
#   `swift build` 不受影响（连跑 4 次 0 error），所以约束 12 的判据命令照常裸调。
#
# 路径从 xcode-select 推导，不写死机器绝对路径——否则换台机器必炸。
# 装了完整 Xcode 并 xcode-select -s 过去之后，testing/ 子目录不存在，脚本自动退回裸调用。
#
# 第二件事：swift-testing 在 --filter 没匹配到任何用例时会打印
#   warning: No matching test cases were run
# 并 **以退出码 0 结束**。过滤器打错一个字，整轮就变成一次静默的空跑绿灯——
# TDD 里"运行它、预期 FAIL"这一步会假通过。本脚本把这种情况转成退出码 1。
#
# 第三件事（**产物新鲜度**）：先 (cd core && cargo build --release)。
#
#   为什么必须有：`EndToEndTests` 拉起的是 `core/target/release/benagen-core` 这个
#   **预先构建的二进制**，而本脚本原先只跑 `swift test`。于是"内核改了、Swift 端到端
#   一条都没跑到"可以**完全无声地**发生 —— 本阶段的账本 Ruling C19 就是一次这样的
#   事故（整个阶段的内核改动在 Swift 端到端上零覆盖，而套件全绿）。
#   不修的话，这个套件的绿灯本身就没有可信度。
#
#   形态与 `macos/scripts/e2e_shell_macos.sh:77-78` 逐字同款（那边先做了，这里是
#   把它补到**所有** Swift 测试的入口上）。已有产物时是秒级 no-op。
#
#   为什么选"构建"而不是"在测试里比 mtime"：后者只能**发现**不新鲜并报错，
#   还要人再手工跑一次 `cargo build`（多一步、多一个会说谎的中间状态）；
#   而且"core/src 下最新源文件"这个判据本身会漏掉 `build.rs` / `Cargo.toml` /
#   依赖图变化（那些同样能让产物过期）。构建是唯一一个**把状态修对**的动作。

set -euo pipefail

# 仓库根 = 本脚本所在目录（`macos/scripts/`）上溯两层。与 `e2e_shell_macos.sh` 的
# `ROOT` / `REPO` 同源，不另写一份"仓库在哪"的判断。
# ⚠️ 必须在下面那次 `cd` **之前**算，而且要用 `${BASH_SOURCE[0]}`（脚本自身的路径，
#    与被调用的工作目录无关）—— 用 `$0` 的相对路径在 `cd` 之后会解析到错的地方
#    （实测：`bash macos/scripts/test.sh` 从仓库根调进来会变成 `macos/macos/...`）。
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [ -d "$repo_root/core" ]; then
  echo "test.sh: 重建内核（cargo build --release；已有产物时是秒级 no-op）" >&2
  (cd "$repo_root/core" && cargo build --release)
else
  # 仓库布局变了（core/ 不在预期位置）——**说清楚**，不要静默跳过这道新鲜度保证。
  echo "test.sh: 警告：找不到 $repo_root/core，跳过内核重建；若 EndToEndTests 会跑到，它的产物可能不新鲜" >&2
fi

cd "$(dirname "$0")/.."

extra=()
plugin_dir="$(xcode-select -p 2>/dev/null || true)/usr/lib/swift/host/plugins/testing"
if [ -d "$plugin_dir" ]; then
  extra=(-Xswiftc -plugin-path -Xswiftc "$plugin_dir")
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

set +e
# ${extra[@]+...} 的写法是为了兼容 macOS 自带的 bash 3.2：
# 空数组在 `set -u` 下直接展开会报 unbound variable。
swift test ${extra[@]+"${extra[@]}"} "$@" 2>&1 | tee "$log"
status="${PIPESTATUS[0]}"
set -e

if grep -q "No matching test cases were run" "$log"; then
  echo "" >&2
  echo "test.sh: --filter 没有匹配到任何用例，一个测试都没跑。" >&2
  echo "test.sh: swift-testing 对这种情形的退出码是 0，这里把它判为失败——" >&2
  echo "test.sh: 空跑出来的绿灯不算证据。请核对过滤器拼写。" >&2
  exit 1
fi

exit "$status"
