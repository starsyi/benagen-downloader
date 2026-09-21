#!/usr/bin/env bash
# portcheck.sh —— 「Swift 源 ↔ Rust 移植」的**差分对拍**（可复跑，控制者裁决 HH）。
#
# 为什么存在：本阶段移植的 163 条测试里，测试覆盖的是**源测试写死的那些值**。
# 差分对拍补的是另一半证据：拿同一批**边界输入**喂两侧、逐行比对输出。
# 这个手法在本波次被用过三次（任务 10、12、16），而每次的驱动都是临时的、结论不可复现
# —— 后面还有一批要移植（写这段时是 98 条，T15 落地后只剩 T14 那 20 条），所以在这里固化：
#
#   * **两侧的驱动源码就在本脚本里**（heredoc），不落在临时目录里；
#   * **被测的是源文件本身**：Swift 侧直接进 `swiftc`，Rust 侧用 `#[path]` 指到工作树的
#     `shell-core/src/`（**没有复制、没有改写**——`//!` 也不用动，因为它是以模块文件
#     的身份被加载的，文档注释本来就合法）；
#   * 唯一"生成"的东西是从 `AppModel.swift` 里**按内容锚点抽出来的** `message(of:)`
#     那 7 行（源文件太大、依赖太多，整体编不进来），抽取结果**自带守卫**：抽歪了就大声失败。
#
# 用法：
#   bash windows/scripts/portcheck.sh              # 跑全部 case
#   bash windows/scripts/portcheck.sh --list       # 列出 case
#   bash windows/scripts/portcheck.sh error_text   # 只跑某个 case
#   bash windows/scripts/portcheck.sh --keep       # 留下工作目录（排障用）
#
# 退出码：0 = 每个 case 的 diff 都为空；1 = 有差异或环境不满足（**不许静默跳过**，W-2）。
#
# ⚠️ 它**不在** `test.sh` 里：`test.sh` 是"一条命令跑完壳的单测"的入口，而它跑在
#    Windows 的 Git Bash 上（那里没有 `swiftc`）。把对拍挂进去会让那个入口在真开发机上
#    变成失败。所以它是一个**独立的手工/CI 可选**入口，缺 `swiftc` 时**大声失败**而不是跳过。
#
# ⚠️ **怎么加一个新 case**（后来的人看这里）：
#    1. 写一个 `case_<名字>()`，里面用**行内 heredoc** 写两份驱动
#       （`cat > "$w/main.swift" <<'SWIFT' … SWIFT` 与 `cat > "$w/driver.rs" <<'RUST' … RUST`。
#       ⚠️ 脚本里**没有** `swift_driver` / `rust_driver` 这样的助手函数，别去找——
#       对照 `case_error_text` 那样写就行），两份驱动各自把同一批输入打印成
#       **同样的行格式**（一行一个结论）；
#    2. 在 `CASES` 里登记它；
#    3. 驱动里 `print` 的每一行都会参与 diff —— 所以**别打印时间戳/路径/指针**这类噪声。
#    ⚠️ **Rust 驱动要不要改本脚本，取决于新模块住在哪一层**：
#       · `presentation/` 下的**新增模块免改**：`new_work` 把整个 `presentation/` **目录**
#         软链过去，`pub mod X;` 一加就跟着进来了；
#       · `shell-core/src/` **顶层**的新模块**要改两处**：`new_work` 里加一条
#         `ln -s "$WIN_ROOT/shell-core/src/X.rs" "$WORK/src/X.rs"`，驱动里再加
#         `#[path = "src/X.rs"] mod X;`。漏了不会静默跳过——驱动会以
#         `error[E0583]: file not found for module` **响亮失败**——但失败的是脚本脚手架，
#         别把它当成对拍结论（下面 `build_rust_driver` 的退出码会即时暴露它）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
WIN_ROOT="$REPO_ROOT/windows"
MACOS_SRC="$REPO_ROOT/macos/Sources/BenagenCoreKit"

KEEP=0
ONLY=()
CASES=(breadcrumb_summary browser_row download_targets error_text transfer_row verify_summary)

# ⚠️ 行号范围**跟着头部注释走**：改完头部记得让它停在本文件第一条非注释行之前
#    （现在就是下面那行 `set -euo pipefail`）——范围错到会把代码当说明打出来。
usage() { sed -n '2,44p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

# ---------------------------------------------------------------------------
# 环境守卫（W-2：不许静默跳过）
# ---------------------------------------------------------------------------

have() { command -v "$1" >/dev/null 2>&1; }

require_env() {
  have swiftc || {
    echo "portcheck: 找不到 swiftc —— 这一条对拍需要 macOS 上的 Swift 工具链。" >&2
    echo "           补救：在装了 Xcode/Command Line Tools 的机器上跑；" >&2
    echo "           或者**别跑它**（它不在 test.sh 里，不是单测的必经之路）。" >&2
    exit 1
  }
  have rustc || { echo "portcheck: 找不到 rustc（见 windows/README.md 的工具链说明）" >&2; exit 1; }
  [[ -f "$MACOS_SRC/Presentation/DownloadTargets.swift" ]] || {
    echo "portcheck: 找不到上游源码目录 $MACOS_SRC" >&2; exit 1; }
}

WORK=""
cleanup() { [[ $KEEP -eq 1 || -z "$WORK" ]] || rm -rf "$WORK"; }
trap cleanup EXIT

new_work() {
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/portcheck.XXXXXX")"
  mkdir -p "$WORK/src"
  # Rust 侧：把整棵 `shell-core/src` 按原样暴露给驱动（`#[path]` 加载，**不改一个字节**）。
  ln -s "$WIN_ROOT/shell-core/src/presentation" "$WORK/src/presentation"
  ln -s "$WIN_ROOT/shell-core/src/protocol.rs" "$WORK/src/protocol.rs"
  ln -s "$WIN_ROOT/shell-core/src/client.rs" "$WORK/src/client.rs"
  # ⚠️ `platform.rs` 也是**必须**的（任务 15 的裁定 JJ 之后）：`presentation::transfer_row`
  #    引用了 `crate::platform`，而驱动自己就是这个 crate 的根 —— 根上不声明它，
  #    `#[path]` 加载过来的那个模块会以 `could not find \`platform\` in the crate root` 报错，
  #    而**三个 case 都会挂**（它们都 `#[path]` 加载了 `presentation/mod.rs`）。
  ln -s "$WIN_ROOT/shell-core/src/platform.rs" "$WORK/src/platform.rs"
}

# ---------------------------------------------------------------------------
# Rust 驱动的编译：`--extern` 从 cargo 的 deps 目录自动解析（**不写死哈希**）
# ---------------------------------------------------------------------------

find_extern() { # $1 = 形如 libserde-*.rlib 的 glob
  local hit
  hit="$(ls -t $WIN_ROOT/target/debug/deps/$1 2>/dev/null | head -1 || true)"
  [[ -n "$hit" ]] && printf '%s' "$hit"
}

build_rust_driver() { # $1 = 驱动源码文件，$2 = 产物**路径**
  local driver="$1" out="$2" serde serde_json serde_derive
  serde="$(find_extern 'libserde-*.rlib')"
  serde_json="$(find_extern 'libserde_json-*.rlib')"
  serde_derive="$(find_extern 'libserde_derive-*.dylib')"
  [[ -n "$serde_derive" ]] || serde_derive="$(find_extern 'libserde_derive-*.so')"
  if [[ -z "$serde" || -z "$serde_json" || -z "$serde_derive" ]]; then
    echo "portcheck: 找不到 serde/serde_json 的编译产物 —— 先跑一次 cargo build -p shell-core。" >&2
    echo "           （为什么必须用**同一批**依赖：驱动要 include 的是这个工作树的源码。）" >&2
    exit 1
  fi
  # `-A warnings`：这是**对拍的脚手架**（一个 bin crate），源文件是以模块身份被整体加载的，
  # 那些 `never used` 与本 crate 的零告警纪律无关（那条由 `cargo test` 守）。不静音的话
  # 49 条噪声会把真正的 diff 淹掉。
  rustc --edition 2021 -A warnings -o "$out" "$driver" \
    --extern "serde=$serde" \
    --extern "serde_derive=$serde_derive" \
    --extern "serde_json=$serde_json" \
    -L "$WIN_ROOT/target/debug/deps"
}

# ---------------------------------------------------------------------------
# 从 `AppModel.swift` 里**按内容锚点**抽出 `message(of:)`（不是抄一份桩）
# ---------------------------------------------------------------------------
#
# ⚠️ 这是本脚本唯一的"生成物"，也是它比"手写桩"强的地方：抽的是**源文件里那几行**，
#    源一改，对拍立刻跟着变（抽不到就大声失败，绝不退回一个桩）。
#    为什么不能整份 `AppModel.swift` 编进来：它有 1700+ 行、依赖一大片（视图、存储、
#    定时器），为了 7 行函数把整棵树拖进来不划算。
extract_appmodel_message() { # $1 = 输出文件
  local out="$1" body
  body="$(awk '
    /nonisolated static func message\(of error: Error\) -> String \{/ { capture = 1 }
    capture { print }
    capture && /^    \}$/ { exit }
  ' "$MACOS_SRC/AppModel.swift")"

  # 守卫：抽歪了（函数被改名/挪走/缩进变了）就**响**，不要把一个空壳编进去当"通过"。
  for needle in 'as? CoreError' 'case .transport(let m), .malformedResponse(let m): return m' \
                'case .rpc(_, let m): return m'; do
    grep -qF "$needle" <<<"$body" || {
      # ⚠️ 反引号必须转义：不转义的话 bash 会把 `$needle` 当**命令替换**跑掉
      #    （实测 stderr 多出一行 `<本脚本>: line NNN: as?: command not found`），于是这条
      #    提示退化成"失败（缺 ）"——唯一说明**哪个锚点丢了**的那段信息被吞掉。
      #    退出码仍是 1（不是假绿），但诊断质量塌了。**行号会随本文件增删而变，
      #    别把它当成判据**；判据是"缺「」里有没有那个锚点"。
      echo "portcheck: 从 AppModel.swift 里抽 message(of:) 失败（缺 \`$needle\`）。" >&2
      echo "           补救：核对 AppModel.swift 里那个函数是否还在、签名是否变了。" >&2
      exit 1
    }
  done
  # ⚠️ 外壳写成 `public enum`：源里的 `AppModel` 是 public，而 `DeliverySummary.swift`
  #    的 `public static func of(_ state: AppModel.LoadState)` 在 macOS 源里已经声明为 public
  #    —— 外壳若取默认的 internal，那一行就会以
  #    "method cannot be declared public because its parameter uses an internal type"
  #    编不过（实测）。这是**脚手架的语言需求**，不是把桩抬成源的一部分：
  #    `message(of:)` 那几行仍然逐字来自源文件。
  { echo '// 以下 7 行由 portcheck.sh 从 AppModel.swift **原样抽出**（不是抄写的桩）：'
    echo 'public enum AppModel {'
    printf '%s\n' "$body"
    echo '}'
  } > "$out"
}

# ---------------------------------------------------------------------------
# 任务 15 要的那三块 `AppModel` 零件（**必须合成一个 `AppModel`**）
# ---------------------------------------------------------------------------
#
# 同一处纪律（同 `extract_appmodel_message`）：抽的是**源文件里的那几行本身**，
# 源一改，对拍立刻跟着变（抽不到就大声失败，绝不退回一个手写的桩）。
# 桩会让"源改了、壳没跟上"这件事在 diff 里凭空消失 —— 那正是对拍的全部价值所在。
#
# ⚠️ **三块必须写在同一个 `enum AppModel { … }` 里**：分开两个文件会得到两个同名类型，
#    Swift 侧报 `'AppModel' is ambiguous for type lookup`（第一次跑就是这么红的）。
#    三块是：`EngineState`（`EngineBanner`/`TransferListEmpty`/`EngineGate` 的入参）、
#    `handshakeTimeoutMessage`（`EngineStatusPresentation` 与两条横幅用例都引用它）、
#    `message(of:)`（`TransferActionFailure` 的落点）。
extract_appmodel_for_transfer_row() { # $1 = 输出文件
  local out="$1" state body timeout
  state="$(awk '
    /^    public enum EngineState: Equatable \{$/ { capture = 1 }
    capture { print }
    capture && /^    \}$/ { exit }
  ' "$MACOS_SRC/AppModel.swift")"
  body="$(awk '
    /nonisolated static func message\(of error: Error\) -> String \{/ { capture = 1 }
    capture { print }
    capture && /^    \}$/ { exit }
  ' "$MACOS_SRC/AppModel.swift")"
  # 这一行**承重**：它对位 Rust 侧 `engine_status::HANDSHAKE_TIMEOUT_MESSAGE`
  # （控制者裁决 EE 定的家）。两边逐字不同的话，横幅那条 diff 会立刻红。
  timeout="$(grep -E '^    public nonisolated static let handshakeTimeoutMessage = ' \
             "$MACOS_SRC/AppModel.swift" || true)"

  # 守卫：抽歪了（改名/挪走/缩进变了）就**响**。
  for needle in 'case unknown' 'case notStarted' 'case running' 'case unavailable(String)'; do
    grep -qF "$needle" <<<"$state" || {
      echo "portcheck: 从 AppModel.swift 里抽 EngineState 失败（缺 \`$needle\`）。" >&2
      echo "           补救：核对 AppModel.swift 里那个 enum 是否还在、缩进是否变了。" >&2
      exit 1
    }
  done
  for needle in 'as? CoreError' 'case .transport(let m), .malformedResponse(let m): return m' \
                'case .rpc(_, let m): return m'; do
    grep -qF "$needle" <<<"$body" || {
      echo "portcheck: 从 AppModel.swift 里抽 message(of:) 失败（缺 \`$needle\`）。" >&2
      exit 1
    }
  done
  grep -qF 'handshakeTimeoutMessage' <<<"$timeout" || {
    echo "portcheck: 从 AppModel.swift 里抽 handshakeTimeoutMessage 失败。" >&2
    echo "           补救：核对那条 static let 是否还在、缩进与修饰符是否变了。" >&2
    exit 1
  }

  # ⚠️ 包装类型必须是 `public`：`EngineStatusPresentation` 的那几个方法声明成 `public` 且**入参
  #    就是 `AppModel.EngineState`**，而"public 方法的参数用了 internal 类型"是编译错误
  #    （第二次跑就是这么红的）。抽出来的**那几行本身**一个字符都没改，改的只是外面这层壳。
  { echo '// 以下若干行由 portcheck.sh 从 AppModel.swift **原样抽出**（不是抄写的桩）：'
    echo 'public enum AppModel {'
    printf '%s\n' "$state" "$timeout" "$body"
    echo '}'
  } > "$out"
}

# ---------------------------------------------------------------------------
# 从 `BrowserRow.swift` 里**按内容锚点**抽出 `RowColor`（任务 15 的呈现色枚举）
# ---------------------------------------------------------------------------
#
# ⚠️ 为什么**不**整份编 `BrowserRow.swift`（它只有 Foundation 一个 import，看起来能编）：
#    它引用了 `Breadcrumb`（`Presentation/Breadcrumb.swift`）与 `TimestampPresentation`，
#    把这两份也拖进来只为了一个三行的枚举 —— 编译面越大，对拍失败时的根因越说不清。
#    抽出来的仍然**是源里那三行**（不是抄写的桩），抽不到就大声失败。
extract_row_color() { # $1 = 输出文件
  local out="$1" body
  body="$(awk '
    /^public enum RowColor: Equatable, Sendable \{$/ { capture = 1 }
    capture { print }
    capture && /^\}$/ { exit }
  ' "$MACOS_SRC/Presentation/BrowserRow.swift")"

  # 守卫：抽歪了就**响**（顺带钉住"是 5 个成员、没有 Gray"这条记账，见 `presentation/mod.rs`）。
  for needle in 'case secondary, blue, green, red, orange'; do
    grep -qF "$needle" <<<"$body" || {
      echo "portcheck: 从 BrowserRow.swift 里抽 RowColor 失败（缺 \`$needle\`）。" >&2
      echo "           补救：核对那个 enum 是否还在、成员是否被改过（本阶段按源取 5 个）。" >&2
      exit 1
    }
  done
  { echo '// 以下若干行由 portcheck.sh 从 BrowserRow.swift **原样抽出**（不是抄写的桩）：'
    printf '%s\n' "$body"
  } > "$out"
}

# ---------------------------------------------------------------------------
# 从 `AppModel.swift` 里**按内容锚点**抽出 `LoadState`（第二个 `of` 重载的入参类型）
# ---------------------------------------------------------------------------
#
# 同 `extract_appmodel_message`：抽的是**源文件里那几行**、自带守卫，不是抄一份桩。
# 为什么非抽不可：`DeliverySummary.swift` 的第二个 `of` 收 `AppModel.LoadState`，
# 少了它，那份文件整个编不进来 —— 而 `breadcrumb_summary` 这个 case 要对正是
# **同一个文件里**的 `DeliverySummary` / `DeliveryCodeEntry` / `TimestampPresentation`。
extract_appmodel_load_state() { # $1 = 输出文件
  local out="$1" body
  body="$(awk '
    /^    public enum LoadState: Equatable \{/ { capture = 1 }
    capture { print }
    capture && /^    \}$/ { exit }
  ' "$MACOS_SRC/AppModel.swift")"

  # 守卫：四个变体少一个就**响**（少了 `loaded` 会让第二个 `of` 编不过，少了 `failed`
  # 会让"失败态没有摘要"这条失去夹具）——绝不退回一个手写的桩。
  for needle in 'case idle' 'case loading' 'case loaded(DeliveryInfo)' 'case failed(String)'; do
    grep -qF "$needle" <<<"$body" || {
      echo "portcheck: 从 AppModel.swift 里抽 LoadState 失败（缺 \`$needle\`）。" >&2
      echo "           补救：核对 AppModel.swift 里那个嵌套枚举是否还在、四个变体是否改了名。" >&2
      exit 1
    }
  done
  { echo '// 以下几行由 portcheck.sh 从 AppModel.swift **原样抽出**（不是抄写的桩）：'
    echo 'extension AppModel {'
    printf '%s\n' "$body"
    echo '}'
  } > "$out"
}

# ---------------------------------------------------------------------------
# case：breadcrumb_summary
# ---------------------------------------------------------------------------
#
# 覆盖任务 11 移植的三组纯计算（**同一个 Swift 源文件里的三个类型**）：
#   `Breadcrumb`（含 `DirLoadFailure`）、`DeliverySummary`、`DeliveryCodeEntry`、
#   `TimestampPresentation`。
#
# ⚠️ **可比范围**（与 `case_error_text` 同一条纪律）：`DirLoadFailure.of` 在 Swift 侧
#    收 `CoreError`，这里只喂 `.rpc(code:message:)` ⇒ Rust 侧 `ClientError::Kernel`。
#    `.transport` / `.malformedResponse` 两侧不是同一份文本（Rust 的那几句归 `client.rs`），
#    **不参与对拍**，由 `breadcrumb.rs` 的单测钉住。
#
# ⚠️ 夹具里那些畸形时间戳（全角数字、字母 O、缺冒号的偏移、带尾巴的串）是**故意**的：
#    这条映射的判据是"逐字节的位置检查"，只喂合法样例等于没对拍。
case_breadcrumb_summary() {
  local w="$WORK/breadcrumb_summary"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"

  extract_appmodel_message "$w/msg_extract.swift"
  extract_appmodel_load_state "$w/load_state_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

/// 换行会破坏"一行一个结论"的 diff：两侧都做同一处两字符替换（见 Rust 侧同名的助手）。
func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}

// ① 层级链：**路径一律是清单原文**（约束 3）——空段、前导斜杠、非 ASCII、空格都在里面。
let paths = ["", "a", "a/b/c", "a//b", "a//", "/a", "a/", "//",
             "client-test//C24-8_×_25WS024//Figure", "C24-8_×_25WS024/Figure/QC 图.png",
             " 两边有空白 /x", "dir/./sub//x.bin"]
for (i, p) in paths.enumerated() {
    let b = Breadcrumb(path: p)
    print("BC \(i) \(b.segments.count) \(flat(b.path)) \(flat(b.segments.map(\.name).joined(separator: "|"))) \(flat(b.segments.map(\.path).joined(separator: "|")))")
    print("PP \(i) \(flat(Breadcrumb.parentPath(ofCurrent: p)))")
}
let joins: [(String, String)] = [("", "top"), ("a/b", "子目录"), ("a//b", ""), ("", ""),
                                 ("client-test", "C24-8_×_25WS024"), ("a/", "b")]
for (i, (parent, name)) in joins.enumerated() {
    print("JN \(i) \(flat(Breadcrumb.join(parent: parent, name: name)))")
}

// ② 时间戳：合法形状、变体、以及**每一条判据各自的反例**。
let stamps = ["2026-10-14T16:13:34+08:00", "2026-09-17T09:37:12.805751+08:00",
              "2026-10-14T16:13:34Z", "2026-10-14T23:13:34-05:00",
              "2026-10-14T16:13:34.1Z", "2026-10-14T16:13:34.Z", "2026-10-14T16:13:34.",
              "", "待定", "2026-10-14", "2026-10-14 16:13:34",
              "2026-10-14T16:13:34+08:00 尾巴", "2026-10-14T16:13:34+0800",
              "2026-10-14T16:13:34+08:0", "2026-10-14T16:13:34+080:00",
              "2026-1O-14T16:13:34+08:00", "2026-10-14T16:13:34+08:00\n",
              "2026-10-14T16:13:34+08:00\r", "２026-10-14T16:13:34+08:00",
              "2026-10-14T16:13:34+08:0000", "2026-10-14T16:13:34z",
              "0000-00-00T00:00:00Z", "2026-10-14t16:13:34+08:00"]
for (i, s) in stamps.enumerated() { print("TS \(i) \(flat(TimestampPresentation.text(s)))") }

// ③ 交付码入口：长度上界（闭区间、按字节）、漏斗、两句提示。
let codes = ["", "C24-8", "  两边有空白  ", "C24-8_×_25WS024",
             "https://download.benagen.com/C24-8_×_25WS024/C24-8_×_25WS024.html",
             String(repeating: "A", count: DeliveryCodeEntry.maximumBytes),
             String(repeating: "A", count: DeliveryCodeEntry.maximumBytes + 1),
             String(repeating: "码", count: 1024),
             String(repeating: "A", count: 9 << 20)]
for (i, c) in codes.enumerated() {
    print("DC \(i) \(DeliveryCodeEntry.tooLong(c)) \(DeliveryCodeEntry.isSendable(c))")
}
let typed: [(String, String)] = [("AbCdEfGhIjKlMnOpQrSt", "C24-8"), ("", "C24-8"), ("", ""), (" x ", "y")]
for (i, (t, r)) in typed.enumerated() {
    print("RES \(i) \(flat(DeliveryCodeEntry.resolve(typed: t, remembered: r)))")
}
print("BOUND \(DeliveryCodeEntry.maximumBytes) \(DeliveryCodeEntry.maximumText)")
print("HINT \(flat(DeliveryCodeEntry.tooLongHint))")
print("URLHINT \(flat(DeliveryCodeEntry.baseURLTooLongHint))")

// ④ 摘要：值两两不同、且非零（见上游夹具那条说明）。
func wire(code: String, createdAt: String, expiresAt: String, expired: Bool,
          totalFiles: Int64, totalBytes: Int64) -> String {
    """
    {"code":"\(code)","page_url":"http://dl.example/C24-8/index.html","base_url":"http://dl.example",
     "created_at":"\(createdAt)","expires_at":"\(expiresAt)","expired":\(expired),
     "total_files":\(totalFiles),"total_bytes":\(totalBytes),
     "tree":{"type":"dir","name":"","children":{}}}
    """
}
let fixtures: [(String, String, String, Bool, Int64, Int64)] = [
    ("C24-8_×_25WS024", "2026-09-01T10:00:00+08:00", "2026-10-14T16:13:34+08:00", false, 7, 3_221_225_472),
    ("C24-8_×_25WS024", "2026-09-01T10:00:00+08:00", "2026-10-14T16:13:34+08:00", true, 7, 3_221_225_472),
    ("", "", "", false, 0, 0),
    ("C24-8", "2026-09-01T10:00:00+08:00", "待定", false, 1, 1023),
    ("C24-8", "", "2026-10-14T16:13:34", false, 1_000_000, 1_099_511_627_776),
]
func info(_ json: String) -> DeliveryInfo {
    try! CoreJSON.decoder.decode(DeliveryInfo.self, from: Data(json.utf8))
}
var infos: [DeliveryInfo] = []
for f in fixtures {
    infos.append(info(wire(code: f.0, createdAt: f.1, expiresAt: f.2, expired: f.3,
                           totalFiles: f.4, totalBytes: f.5)))
}
for (i, d) in infos.enumerated() {
    let s = DeliverySummary.of(d)
    print("DS \(i) \(flat(s.code))|\(flat(s.filesText))|\(flat(s.sizeText))|\(flat(s.validityText))|\(s.expiredBadgeText.map(flat) ?? "nil")")
}

// ⑤ 加载状态 → 摘要：只有 `.loaded` 有摘要。
let states: [AppModel.LoadState] = [.idle, .loading, .failed("拉取交付清单失败：HTTP 404"),
                                    .failed(""), .failed("待定"), .loaded(infos[0]), .loaded(infos[2])]
for (i, st) in states.enumerated() {
    if let s = DeliverySummary.of(st) {
        print("LS \(i) \(flat(s.code))|\(flat(s.filesText))|\(flat(s.sizeText))|\(flat(s.validityText))|\(s.expiredBadgeText.map(flat) ?? "nil")")
    } else {
        print("LS \(i) nil")
    }
}

// ⑥ 目录加载失败：停在哪儿、说什么（**只比 `.rpc`**，理由见本 case 头部）。
let rpcCases: [(String, String)] = [("path_not_found", "清单里没有目录 \"a/b/c\""),
                                    ("path_not_found", ""),
                                    ("engine_disconnected", "下载引擎已断开"),
                                    ("invalid_params", "参数不合法"),
                                    ("bad_request", "[invalid_params] 正文里本来就有码字样")]
for (i, (code, message)) in rpcCases.enumerated() {
    for (j, current) in ["a/b/c", "a", "", "a//b", "/x"].enumerated() {
        let f = DirLoadFailure.of(CoreError.rpc(code: code, message: message), currentPath: current)
        print("DLF \(i) \(j) \(flat(f.path))|\(f.didFallBack)|\(f.notice.map(flat) ?? "nil")|\(flat(f.message))")
    }
}
SWIFT

  cat > "$w/driver.rs" <<'RUST'
// Rust 侧驱动。`#[path]` 直接把工作树的源码按模块加载（**一个字节都没改**）。
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use client::ClientError;
use presentation::breadcrumb::{Breadcrumb, DirLoadFailure};
use presentation::delivery_summary::{DeliveryCodeEntry, DeliverySummary, TimestampPresentation};
use protocol::{DeliveryInfo, LoadState};

/// 与 Swift 侧 `flat` 同形（换行会破坏一行一个结论的 diff）。
fn flat(s: &str) -> String { s.replace('\n', "\\n").replace('\r', "\\r") }
fn some_or_nil(v: Option<String>) -> String { v.unwrap_or_else(|| "nil".to_string()) }

fn main() {
    // ① 层级链：**路径一律是清单原文**（约束 3）。
    let paths = ["", "a", "a/b/c", "a//b", "a//", "/a", "a/", "//",
                 "client-test//C24-8_×_25WS024//Figure", "C24-8_×_25WS024/Figure/QC 图.png",
                 " 两边有空白 /x", "dir/./sub//x.bin"];
    for (i, p) in paths.iter().enumerate() {
        let b = Breadcrumb::new(p);
        let names: Vec<&str> = b.segments.iter().map(|s| s.name.as_str()).collect();
        let seg_paths: Vec<&str> = b.segments.iter().map(|s| s.path.as_str()).collect();
        println!("BC {i} {} {} {} {}",
                 b.segments.len(), flat(&b.path), flat(&names.join("|")), flat(&seg_paths.join("|")));
        println!("PP {i} {}", flat(&Breadcrumb::parent_path_of_current(p)));
    }
    let joins: [(&str, &str); 6] = [("", "top"), ("a/b", "子目录"), ("a//b", ""), ("", ""),
                                    ("client-test", "C24-8_×_25WS024"), ("a/", "b")];
    for (i, (parent, name)) in joins.iter().enumerate() {
        println!("JN {i} {}", flat(&Breadcrumb::join(parent, name)));
    }

    // ② 时间戳：合法形状、变体、以及**每一条判据各自的反例**。
    let stamps = ["2026-10-14T16:13:34+08:00", "2026-09-17T09:37:12.805751+08:00",
                  "2026-10-14T16:13:34Z", "2026-10-14T23:13:34-05:00",
                  "2026-10-14T16:13:34.1Z", "2026-10-14T16:13:34.Z", "2026-10-14T16:13:34.",
                  "", "待定", "2026-10-14", "2026-10-14 16:13:34",
                  "2026-10-14T16:13:34+08:00 尾巴", "2026-10-14T16:13:34+0800",
                  "2026-10-14T16:13:34+08:0", "2026-10-14T16:13:34+080:00",
                  "2026-1O-14T16:13:34+08:00", "2026-10-14T16:13:34+08:00\n",
                  "2026-10-14T16:13:34+08:00\r", "２026-10-14T16:13:34+08:00",
                  "2026-10-14T16:13:34+08:0000", "2026-10-14T16:13:34z",
                  "0000-00-00T00:00:00Z", "2026-10-14t16:13:34+08:00"];
    for (i, s) in stamps.iter().enumerate() {
        println!("TS {i} {}", flat(&TimestampPresentation::text(s)));
    }

    // ③ 交付码入口。
    let codes = ["".to_string(), "C24-8".to_string(), "  两边有空白  ".to_string(),
                 "C24-8_×_25WS024".to_string(),
                 "https://download.benagen.com/C24-8_×_25WS024/C24-8_×_25WS024.html".to_string(),
                 "A".repeat(DeliveryCodeEntry::MAXIMUM_BYTES),
                 "A".repeat(DeliveryCodeEntry::MAXIMUM_BYTES + 1),
                 "码".repeat(1024),
                 "A".repeat(9 << 20)];
    for (i, c) in codes.iter().enumerate() {
        println!("DC {i} {} {}", DeliveryCodeEntry::too_long(c), DeliveryCodeEntry::is_sendable(c));
    }
    let typed: [(&str, &str); 4] = [("AbCdEfGhIjKlMnOpQrSt", "C24-8"), ("", "C24-8"), ("", ""), (" x ", "y")];
    for (i, (t, r)) in typed.iter().enumerate() {
        println!("RES {i} {}", flat(&DeliveryCodeEntry::resolve(t, r)));
    }
    println!("BOUND {} {}", DeliveryCodeEntry::MAXIMUM_BYTES, DeliveryCodeEntry::MAXIMUM_TEXT);
    println!("HINT {}", flat(&DeliveryCodeEntry::too_long_hint()));
    println!("URLHINT {}", flat(&DeliveryCodeEntry::base_url_too_long_hint()));

    // ④ 摘要。
    fn wire(code: &str, created_at: &str, expires_at: &str, expired: bool,
            total_files: i64, total_bytes: i64) -> String {
        format!(
            r#"{{"code":"{code}","page_url":"http://dl.example/C24-8/index.html","base_url":"http://dl.example",
 "created_at":"{created_at}","expires_at":"{expires_at}","expired":{expired},
 "total_files":{total_files},"total_bytes":{total_bytes},
 "tree":{{"type":"dir","name":"","children":{{}}}}}}"#
        )
    }
    let fixtures: [(&str, &str, &str, bool, i64, i64); 5] = [
        ("C24-8_×_25WS024", "2026-09-01T10:00:00+08:00", "2026-10-14T16:13:34+08:00", false, 7, 3_221_225_472),
        ("C24-8_×_25WS024", "2026-09-01T10:00:00+08:00", "2026-10-14T16:13:34+08:00", true, 7, 3_221_225_472),
        ("", "", "", false, 0, 0),
        ("C24-8", "2026-09-01T10:00:00+08:00", "待定", false, 1, 1023),
        ("C24-8", "", "2026-10-14T16:13:34", false, 1_000_000, 1_099_511_627_776),
    ];
    let infos: Vec<DeliveryInfo> = fixtures
        .iter()
        .map(|f| {
            let json = wire(f.0, f.1, f.2, f.3, f.4, f.5);
            serde_json::from_str(&json).expect("夹具必须能解")
        })
        .collect();
    let summary_line = |s: &DeliverySummary| {
        format!("{}|{}|{}|{}|{}",
                flat(&s.code), flat(&s.files_text), flat(&s.size_text), flat(&s.validity_text),
                some_or_nil(s.expired_badge_text.clone()))
    };
    for (i, d) in infos.iter().enumerate() {
        println!("DS {i} {}", summary_line(&DeliverySummary::of(d)));
    }

    // ⑤ 加载状态 → 摘要：只有 `.loaded` 有摘要。
    let states = [LoadState::Idle, LoadState::Loading,
                  LoadState::Failed("拉取交付清单失败：HTTP 404".to_string()),
                  LoadState::Failed(String::new()),
                  LoadState::Failed("待定".to_string()),
                  LoadState::Loaded(infos[0].clone()),
                  LoadState::Loaded(infos[2].clone())];
    for (i, st) in states.iter().enumerate() {
        match DeliverySummary::of_load_state(st) {
            Some(s) => println!("LS {i} {}", summary_line(&s)),
            None => println!("LS {i} nil"),
        }
    }

    // ⑥ 目录加载失败（**只比 `.rpc`**）。
    let rpc_cases: [(&str, &str); 5] = [("path_not_found", "清单里没有目录 \"a/b/c\""),
                                        ("path_not_found", ""),
                                        ("engine_disconnected", "下载引擎已断开"),
                                        ("invalid_params", "参数不合法"),
                                        ("bad_request", "[invalid_params] 正文里本来就有码字样")];
    for (i, (code, message)) in rpc_cases.iter().enumerate() {
        for (j, current) in ["a/b/c", "a", "", "a//b", "/x"].iter().enumerate() {
            let e = ClientError::Kernel { code: (*code).to_string(), message: (*message).to_string() };
            let f = DirLoadFailure::of(&e, current);
            println!("DLF {i} {j} {}|{}|{}|{}",
                     flat(&f.path), f.did_fall_back(), some_or_nil(f.notice.clone()), flat(&f.message));
        }
    }
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$MACOS_SRC/Presentation/Format.swift" \
         "$MACOS_SRC/Presentation/Breadcrumb.swift" \
         "$MACOS_SRC/Presentation/DeliverySummary.swift" \
         "$w/msg_extract.swift" "$w/load_state_extract.swift" "$w/main.swift" -o "$w/bs_swift"
  build_rust_driver "$w/driver.rs" "$w/bs_rust"

  "$w/bs_swift" > "$w/swift.out"
  "$w/bs_rust" > "$w/rust.out"
  diff_outputs "breadcrumb_summary" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# case：download_targets
# ---------------------------------------------------------------------------

case_download_targets() {
  local w="$WORK/download_targets"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"   # `#[path]` 相对驱动文件所在目录解析

  extract_appmodel_message "$w/msg_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

// 夹具与驱动 —— 与 Rust 侧逐行同形（两侧输出直接 diff）。
let partialWire = #"""
{"added":[{"gid":"g1","path":"a.bin"},{"gid":"g2","path":"b.bin"},{"gid":"g3","path":"c.bin"}],
 "rejected":[{"path":"z.bin","reason":"路径不安全（越界/控制字符/空段）"},
             {"path":"C24-8_×_25WS024/QC 图.png","reason":"清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"}]}
"""#
let nothingAtAllWire = #"{"added":[],"rejected":[]}"#
let allRejectedError = #"没有任何文件被加入下载：[Object {"path": "z.bin", "reason": "路径不安全（越界/控制字符/空段）"}]"#

func enqueued(_ j: String) -> EnqueueResult {
    try! CoreJSON.decoder.decode(EnqueueResult.self, from: Data(j.utf8))
}
/// 换行会破坏"一行一个结论"的 diff：两侧都做同一处两字符替换（见 Rust 侧同名的助手）。
func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}
func show(_ label: String, _ s: Set<String>, _ a: Set<String>) {
    print("P \(label) \(flat(DownloadTargets.paths(for: s, allPaths: a).joined(separator: "|")))")
}

// ① paths：勾选面 → 请求
show("dir-as-is", ["a/b"], ["a/b/one.bin"])
show("sorted", ["c.bin", "a/b"], ["c.bin", "a/b/one.bin"])
show("no-selection", [], ["a.bin"])
show("six", ["b.bin", " C24-8_×_25WS024/Figure", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"],
             ["b.bin", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"])
show("all", ["a", "b", "c"], ["a", "b", "c"])
show("missing-one", ["a", "b"], ["a", "b", "c"])
show("empty-batch-both", [], [])
show("empty-batch-selected", ["x.bin"], [])

// ② requestBytes：逐字节比（含引号/反斜杠/换行/制表/非 ASCII/两种规范化形态）
let byteCases: [[String]] = [
    [""], ["a.txt", "dir/b.bin"], ["a/b"], ["x"],
    [" C24-8_×_25WS024/Figure"], ["a\"b"], ["a\\b"], ["a\nb"], ["a\tb"],
    ["清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"],
    ["dir/./sub//x.bin"], ["é", "e\u{301}"],
    Array(repeating: "dir0/file-with-a-fairly-long-name-0.bin", count: 20),
    (0..<300).map { "dir\($0)/file-\($0).bin" },
]
for (i, c) in byteCases.enumerated() {
    print("B \(i) \(DownloadTargets.requestBytes(c)) \(DownloadTargets.exceedsRequestBudget(c))")
}
let many = (0..<200_000).map { "dir\($0)/file-with-a-fairly-long-name-\($0).bin" }
print("B oversized \(DownloadTargets.requestBytes(many)) \(DownloadTargets.exceedsRequestBudget(many))")

// ③ 预算边界（信封**自量**，不硬编码 14）
let envelope = DownloadTargets.requestBytes([""])
let exact = DownloadTargets.requestBudgetBytes - envelope
print("BDY envelope=\(envelope) budget=\(DownloadTargets.requestBudgetBytes) exact=\(exact)")
let onTheLine = [String(repeating: "x", count: exact)]
let oneByteOver = [String(repeating: "x", count: exact + 1)]
print("BDY on=\(DownloadTargets.requestBytes(onTheLine)) \(DownloadTargets.exceedsRequestBudget(onTheLine)) over=\(DownloadTargets.requestBytes(oneByteOver)) \(DownloadTargets.exceedsRequestBudget(oneByteOver))")

// ④ 固定文案
print("HINT \(flat(DownloadTargets.emptySelectionHint))")
print("BTN0 \(flat(DownloadTargets.buttonTitle(for: [])))")
print("BTN1 \(flat(DownloadTargets.buttonTitle(for: ["a.bin"])))")
print("BTN2 \(flat(DownloadTargets.buttonTitle(for: ["a.bin", "b/c"])))")
print("HELP \(flat(DownloadTargets.helpText))")

// ⑤ 回执：**失败那几行走的是上面从 AppModel.swift 抽出来的真 `message(of:)`**
let f = EnqueueFeedback.of(enqueued(partialWire))
print("FB \(flat(f.summary)) | \(f.switchesToTransfers) | \(f.rejections.count) | \(flat(f.rejections.map(\.path).joined(separator: "|")))")
print("FB2 \(flat(f.rejections.map { "\($0.path) => \($0.reason)" }.joined(separator: " || ")))")
let nothing = EnqueueFeedback.of(enqueued(nothingAtAllWire))
print("FB3 \(flat(nothing.summary)) | \(nothing.switchesToTransfers) | \(nothing.rejections.count)")
print("FB4 \(flat(EnqueueFeedback.failure(of: CoreError.rpc(code: "invalid_params", message: allRejectedError)).summary))")
print("FB5 \(flat(EnqueueFeedback.failure(of: CoreError.rpc(code: "engine_start_failed", message: "启动下载引擎失败：端口 6800 被占用")).summary))")

// ⑥ DownloadNotice
let n = DownloadNotice.of(enqueued(partialWire), code: "AAA-1")
print("N1 \(n.belongsTo(to: "AAA-1")) \(flat(n.summary(currentCode: "AAA-1"))) \(n.belongsTo(to: "BBB-2")) \(flat(n.summary(currentCode: "BBB-2"))) \(n.feedback.rejections.count)")
let nf = DownloadNotice.failure(of: CoreError.rpc(code: "preflight_failed", message: "磁盘空间不足：需要 10 GB，可用 2 GB"), code: "AAA-1")
print("N2 \(flat(nf.summary(currentCode: "AAA-1"))) || \(flat(nf.summary(currentCode: "BBB-2"))) || \(nf.feedback.switchesToTransfers)")
let noCode = DownloadNotice.of(enqueued(nothingAtAllWire), code: nil)
print("N3 \(noCode.belongsTo(to: nil)) \(flat(noCode.summary(currentCode: nil))) || \(flat(noCode.summary(currentCode: "AAA-1")))")
SWIFT

  cat > "$w/driver.rs" <<'RUST'
// Rust 侧驱动。`#[path]` 直接把工作树的源码按模块加载（**一个字节都没改**）。
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use client::ClientError;
use presentation::download_targets::{DownloadNotice, DownloadTargets, EnqueueFeedback};
use std::collections::BTreeSet;

const PARTIAL_WIRE: &str = r#"{"added":[{"gid":"g1","path":"a.bin"},{"gid":"g2","path":"b.bin"},{"gid":"g3","path":"c.bin"}],
 "rejected":[{"path":"z.bin","reason":"路径不安全（越界/控制字符/空段）"},
             {"path":"C24-8_×_25WS024/QC 图.png","reason":"清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"}]}"#;
const NOTHING_AT_ALL_WIRE: &str = r#"{"added":[],"rejected":[]}"#;
const ALL_REJECTED_ERROR: &str =
    r#"没有任何文件被加入下载：[Object {"path": "z.bin", "reason": "路径不安全（越界/控制字符/空段）"}]"#;

fn set(items: &[&str]) -> BTreeSet<String> { items.iter().map(|s| (*s).to_string()).collect() }
fn v(items: &[&str]) -> Vec<String> { items.iter().map(|s| (*s).to_string()).collect() }
fn enqueued(j: &str) -> protocol::EnqueueResult { serde_json::from_str(j).expect("夹具必须能解") }
/// 与 Swift 侧 `flat` 同形（换行会破坏一行一个结论的 diff）。
fn flat(s: &str) -> String { s.replace('\n', "\\n").replace('\r', "\\r") }

fn show(label: &str, s: BTreeSet<String>, a: BTreeSet<String>) {
    println!("P {} {}", label, flat(&DownloadTargets::paths(&s, &a).join("|")));
}

fn main() {
    show("dir-as-is", set(&["a/b"]), set(&["a/b/one.bin"]));
    show("sorted", set(&["c.bin", "a/b"]), set(&["c.bin", "a/b/one.bin"]));
    show("no-selection", set(&[]), set(&["a.bin"]));
    show("six",
         set(&["b.bin", " C24-8_×_25WS024/Figure", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"]),
         set(&["b.bin", "a/./c.bin", "a//d", "a/b.bin", "Z.bin"]));
    show("all", set(&["a", "b", "c"]), set(&["a", "b", "c"]));
    show("missing-one", set(&["a", "b"]), set(&["a", "b", "c"]));
    show("empty-batch-both", set(&[]), set(&[]));
    show("empty-batch-selected", set(&["x.bin"]), set(&[]));

    let byte_cases: Vec<Vec<String>> = vec![
        v(&[""]), v(&["a.txt", "dir/b.bin"]), v(&["a/b"]), v(&["x"]),
        v(&[" C24-8_×_25WS024/Figure"]), v(&["a\"b"]), v(&["a\\b"]), v(&["a\nb"]), v(&["a\tb"]),
        v(&["清单里没有 \"C24-8_×_25WS024/QC 图.png\"（既不是文件，也不是任何文件的目录前缀）"]),
        v(&["dir/./sub//x.bin"]), v(&["é", "e\u{301}"]),
        (0..20).map(|_| "dir0/file-with-a-fairly-long-name-0.bin".to_string()).collect(),
        (0..300).map(|i| format!("dir{i}/file-{i}.bin")).collect(),
    ];
    for (i, c) in byte_cases.iter().enumerate() {
        println!("B {i} {} {}", DownloadTargets::request_bytes(c), DownloadTargets::exceeds_request_budget(c));
    }
    let many: Vec<String> = (0..200_000)
        .map(|i| format!("dir{i}/file-with-a-fairly-long-name-{i}.bin")).collect();
    println!("B oversized {} {}", DownloadTargets::request_bytes(&many), DownloadTargets::exceeds_request_budget(&many));

    let envelope = DownloadTargets::request_bytes(&v(&[""]));
    let exact = DownloadTargets::REQUEST_BUDGET_BYTES - envelope;
    println!("BDY envelope={envelope} budget={} exact={exact}", DownloadTargets::REQUEST_BUDGET_BYTES);
    let on_the_line = v(&[&"x".repeat(exact)]);
    let one_byte_over = v(&[&"x".repeat(exact + 1)]);
    println!("BDY on={} {} over={} {}",
             DownloadTargets::request_bytes(&on_the_line), DownloadTargets::exceeds_request_budget(&on_the_line),
             DownloadTargets::request_bytes(&one_byte_over), DownloadTargets::exceeds_request_budget(&one_byte_over));

    println!("HINT {}", flat(DownloadTargets::empty_selection_hint()));
    println!("BTN0 {}", flat(&DownloadTargets::button_title(&set(&[]))));
    println!("BTN1 {}", flat(&DownloadTargets::button_title(&set(&["a.bin"]))));
    println!("BTN2 {}", flat(&DownloadTargets::button_title(&set(&["a.bin", "b/c"]))));
    println!("HELP {}", flat(DownloadTargets::help_text()));

    let f = EnqueueFeedback::of(&enqueued(PARTIAL_WIRE));
    println!("FB {} | {} | {} | {}",
             flat(&f.summary), f.switches_to_transfers, f.rejections.len(),
             flat(&f.rejections.iter().map(|r| r.path.as_str()).collect::<Vec<_>>().join("|")));
    println!("FB2 {}", flat(&f.rejections.iter()
        .map(|r| format!("{} => {}", r.path, r.reason)).collect::<Vec<_>>().join(" || ")));
    let nothing = EnqueueFeedback::of(&enqueued(NOTHING_AT_ALL_WIRE));
    println!("FB3 {} | {} | {}", flat(&nothing.summary), nothing.switches_to_transfers, nothing.rejections.len());
    let all_rej = EnqueueFeedback::failure(&ClientError::Kernel {
        code: "invalid_params".to_string(), message: ALL_REJECTED_ERROR.to_string() });
    println!("FB4 {}", flat(&all_rej.summary));
    let failed = EnqueueFeedback::failure(&ClientError::Kernel {
        code: "engine_start_failed".to_string(), message: "启动下载引擎失败：端口 6800 被占用".to_string() });
    println!("FB5 {}", flat(&failed.summary));

    let n = DownloadNotice::of(&enqueued(PARTIAL_WIRE), Some("AAA-1"));
    println!("N1 {} {} {} {} {}",
             n.belongs_to(Some("AAA-1")), flat(&n.summary(Some("AAA-1"))),
             n.belongs_to(Some("BBB-2")), flat(&n.summary(Some("BBB-2"))), n.feedback.rejections.len());
    let nf = DownloadNotice::failure(&ClientError::Kernel {
        code: "preflight_failed".to_string(), message: "磁盘空间不足：需要 10 GB，可用 2 GB".to_string() }, Some("AAA-1"));
    println!("N2 {} || {} || {}", flat(&nf.summary(Some("AAA-1"))), flat(&nf.summary(Some("BBB-2"))), nf.feedback.switches_to_transfers);
    let no_code = DownloadNotice::of(&enqueued(NOTHING_AT_ALL_WIRE), None);
    println!("N3 {} {} || {}", no_code.belongs_to(None), flat(&no_code.summary(None)), flat(&no_code.summary(Some("AAA-1"))));
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$MACOS_SRC/Presentation/DownloadTargets.swift" \
         "$w/msg_extract.swift" "$w/main.swift" -o "$w/dt_swift"
  build_rust_driver "$w/driver.rs" "$w/dt_rust"

  "$w/dt_swift" > "$w/swift.out"
  "$w/dt_rust" > "$w/rust.out"
  diff_outputs "download_targets" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# case：error_text（对齐 `AppModel.message(of:)` 的 `.rpc` 那一行）
# ---------------------------------------------------------------------------
#
# ⚠️ **可比范围**：Swift 侧只能比 `.rpc(code:message:)` ⇒ Rust 侧的 `ClientError::Kernel`。
#    `transport`/`malformedResponse` 在 Rust 侧对应的是 `client.rs` 自己写的人话文案，
#    Swift 侧那些话出自 `CoreClient.swift`，两边**不是同一份文本**，所以**不可比**
#    （那部分由 `presentation/error_text.rs` 的单测钉住，不靠对拍）。
case_error_text() {
  local w="$WORK/error_text"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"

  extract_appmodel_message "$w/msg_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}
// 与 Rust 侧同一批输入（含空串、多行、方括号、非 ASCII、看起来像 `[code]` 的正文）。
let cases: [(String, String)] = [
    ("invalid_params", "没有任何文件被加入下载：[Object {\"path\": \"z.bin\", \"reason\": \"路径不安全（越界/控制字符/空段）\"}]"),
    ("engine_start_failed", "启动下载引擎失败：端口 6800 被占用"),
    ("preflight_failed", "磁盘空间不足：需要 10 GB，可用 2 GB"),
    ("no_delivery", ""),
    ("delivery_fetch_failed", "第一行\n第二行\n第三行"),
    ("path_not_found", "[invalid_params] 这句正文里本来就有方括号与码字样"),
    ("bad_request", "C24-8_×_25WS024：非 ASCII 的交付码不得被改写"),
]
for (i, (code, message)) in cases.enumerated() {
    print("E \(i) \(flat(AppModel.message(of: CoreError.rpc(code: code, message: message))))")
}
SWIFT

  cat > "$w/driver.rs" <<'RUST'
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use client::ClientError;
use presentation::error_text::error_text;

fn flat(s: &str) -> String { s.replace('\n', "\\n").replace('\r', "\\r") }

fn main() {
    // 与 Swift 侧同一批输入，逐项同形。
    let cases: [(&str, &str); 7] = [
        ("invalid_params", "没有任何文件被加入下载：[Object {\"path\": \"z.bin\", \"reason\": \"路径不安全（越界/控制字符/空段）\"}]"),
        ("engine_start_failed", "启动下载引擎失败：端口 6800 被占用"),
        ("preflight_failed", "磁盘空间不足：需要 10 GB，可用 2 GB"),
        ("no_delivery", ""),
        ("delivery_fetch_failed", "第一行\n第二行\n第三行"),
        ("path_not_found", "[invalid_params] 这句正文里本来就有方括号与码字样"),
        ("bad_request", "C24-8_×_25WS024：非 ASCII 的交付码不得被改写"),
    ];
    for (i, (code, message)) in cases.iter().enumerate() {
        let e = ClientError::Kernel { code: (*code).to_string(), message: (*message).to_string() };
        println!("E {i} {}", flat(&error_text(&e)));
    }
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$w/msg_extract.swift" "$w/main.swift" -o "$w/et_swift"
  build_rust_driver "$w/driver.rs" "$w/et_rust"

  "$w/et_swift" > "$w/swift.out"
  "$w/et_rust" > "$w/rust.out"
  diff_outputs "error_text" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# case：transfer_row（任务 15）
# ---------------------------------------------------------------------------
#
# ⚠️ **可比范围**：本 case 覆盖任务 15 移植的全部纯计算 —— 行（标题/进度/状态/动作）、
#    全局汇总、顶部横幅、空态、准入闸门、轮询节拍、清单路径 → 落盘路径。
#    `TransferActionFailure` 也在内：Swift 侧走的是同一个 case 里**抽出来的** `message(of:)`
#    （不是桩），Rust 侧走 `error_text` —— 于是两侧的"内核原文"是同一份来源。
#
# ⚠️ 需要把 `AppModel.EngineState` 也抽出来（`EngineBanner`/`TransferListEmpty`/`EngineGate`
#    的入参就是它）。与 `message(of:)` 的抽法同一条纪律：**按内容锚点抽、抽歪了大声失败**，
#    绝不退回一个手写的桩 —— 桩会让"源改了、壳没跟上"这件事在 diff 里消失。
case_transfer_row() {
  local w="$WORK/transfer_row"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"   # `#[path]` 相对驱动文件所在目录解析

  extract_appmodel_for_transfer_row "$w/appmodel_extract.swift"
  extract_row_color "$w/color_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}
func show(_ s: String?) -> String { s.map(flat) ?? "-" }

func colorName(_ c: RowColor) -> String {
    switch c {
    case .secondary: return "secondary"
    case .blue: return "blue"
    case .green: return "green"
    case .red: return "red"
    case .orange: return "orange"
    }
}
/// 按**线上字面量**排序：`Set` 的遍历顺序每次进程都不同，不排序就没法逐行 diff。
func actionNames(_ s: Set<TaskAction>) -> String {
    s.map(\.rawValue).sorted().joined(separator: ",")
}
func stateName(_ s: AppModel.EngineState) -> String {
    switch s {
    case .unknown: return "unknown"
    case .notStarted: return "notStarted"
    case .running: return "running"
    case .unavailable: return "unavailable"
    }
}
// 夹具：**经解码构造**（与 `TransferRowTests.swift` 的 `TransferItem.fixture` 同形）。
func fixture(gid: String = "g-1", total: Int64 = 1000, completed: Int64 = 250,
             speed: Int64 = 512, state: TaskState = .active, rawStatus: String = "active",
             errorMessage: String = "", path: String? = "a.bin") -> TransferItem {
    var obj: [String: Any] = ["gid": gid, "total": total, "completed": completed,
                              "speed": speed, "conns": 2, "state": state.rawValue,
                              "raw_status": rawStatus, "error_message": errorMessage]
    obj["path"] = path ?? NSNull()
    let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    return try! CoreJSON.decoder.decode(TransferItem.self, from: data)
}
func row(_ label: String, _ item: TransferItem) {
    let r = TransferRow(item)
    print("R \(label) | \(flat(r.title)) | \(show(r.manifestPath)) | \(flat(r.progressText))"
        + " | \(r.percentText) | \(String(format: "%.6f", r.progressFraction))"
        + " | \(flat(r.speedText)) | \(flat(r.stateLabel)) | \(colorName(r.stateColor))"
        + " | \(r.stateIconName) | \(r.showsPausedBadge ? 1 : 0) | \(show(r.errorText))"
        + " | \(actionNames(r.availableActions)) | \(r.canRevealInFinder ? 1 : 0)")
}
func banner(_ label: String, _ e: AppModel.EngineState, _ lastError: String?) {
    guard let b = EngineBanner.of(engine: e, lastError: lastError) else {
        print("B \(label) none"); return
    }
    let kind: String
    switch b.kind {
    case .engineUnavailable: kind = "engineUnavailable"
    case .transientError: kind = "transientError"
    }
    print("B \(label) \(kind) | \(flat(b.text)) | \(show(b.hint)) | \(b.showsRetry ? 1 : 0)")
}
func empty(_ label: String, _ e: AppModel.EngineState) {
    print("E \(label) \(show(TransferListEmpty.of(engine: e)))")
}
func gate(_ label: String, _ e: AppModel.EngineState) {
    print("A \(label) \(EngineGate.allowsRequests(e) ? 1 : 0)")
}
func reveal(_ label: String, _ manifest: String?, _ home: String, _ dir: String) {
    print("V \(label) \(show(TransferReveal.localPath(manifestPath: manifest, home: home, downloadDir: dir)))")
}
func root(_ label: String, _ home: String, _ dir: String) {
    print("D \(label) \(TransferReveal.downloadRoot(home: home, downloadDir: dir))")
}
func homeOf(_ label: String, _ environment: [String: String], _ fallback: String) {
    print("H \(label) \(TransferReveal.home(environment: environment, fallback: fallback))")
}

// -------- 行 --------
row("default", fixture())
row("path-nil", fixture(path: nil))
row("path-empty", fixture(path: ""))
row("path-verbatim", fixture(path: "sub dir/C24-8_×_25WS024/QC 图.png"))
row("error-multiline", fixture(state: .error, rawStatus: "error",
                               errorMessage: "连接超时（第 3 次重试）\n原因：Connection reset by peer"))
row("no-error", fixture(state: .active, errorMessage: ""))
row("total-zero", fixture(total: 0, completed: 0))
row("over-report", fixture(total: 100, completed: 250))
row("negative", fixture(total: 100, completed: -5))
row("quarter", fixture(total: 400, completed: 100))
row("progress-line", fixture(total: 2048, completed: 1024))
row("speed-zero", fixture(speed: 0))
row("paused", fixture(state: .waiting, rawStatus: "paused"))
row("queued", fixture(state: .waiting, rawStatus: "waiting"))
row("complete", fixture(state: .complete, rawStatus: "complete"))
row("removed", fixture(state: .removed, rawStatus: "removed"))
row("no-gid", fixture(gid: "", state: .active))
for s in TaskState.allCases {
    row("state-\(s.rawValue)", fixture(state: s))
}
// 溢出那一支（`PercentFormat` 的 `checked_mul` 对位）：i64 的上下边界。
row("huge-half", fixture(total: Int64.max, completed: Int64.max / 2))
row("huge-full", fixture(total: Int64.max, completed: Int64.max))
row("huge-over", fixture(total: 1_000_000_000_000_000_000, completed: Int64.max))

// -------- 全局汇总 --------
let gs = GlobalStat(downloadSpeed: 900, numActive: 2, numWaiting: 3, numStopped: 4)
let s = TransferGlobalSummary.of(gs)
print("G all \(flat(s.speedText)) | \(flat(s.activityText)) | \(s.canClearFinished ? 1 : 0)")
let g0 = TransferGlobalSummary.of(GlobalStat(downloadSpeed: 0, numActive: 0, numWaiting: 0, numStopped: 0))
print("G zero \(flat(g0.speedText)) | \(flat(g0.activityText)) | \(g0.canClearFinished ? 1 : 0)")
let g1 = TransferGlobalSummary.of(GlobalStat(downloadSpeed: 0, numActive: 1, numWaiting: 0, numStopped: 1))
print("G one-stopped \(flat(g1.speedText)) | \(flat(g1.activityText)) | \(g1.canClearFinished ? 1 : 0)")

// -------- 横幅 --------
let reasons = ["下载引擎已断开：内核进程已退出（管道结束）",
               "引擎启动失败：aria2c 没有在 30 秒内就绪",
               "内核重启失败：找不到内核可执行文件 benagen-core",
               "内核崩了\n第二行",
               AppModel.handshakeTimeoutMessage]
for (i, why) in reasons.enumerated() {
    banner("unavailable-\(i)", .unavailable(why), nil)
}
banner("unavailable-stale", .unavailable(reasons[0]), "上一拍的瞬时错误")
banner("transient", .running, "下载引擎 RPC 失败：connection reset")
banner("transient-empty", .running, "")
banner("running-none", .running, nil)
banner("notStarted-none", .notStarted, nil)
banner("unknown-none", .unknown, nil)
banner("timeout-with-stale", .unavailable(AppModel.handshakeTimeoutMessage), "上一拍的瞬时错误")

// -------- 空态 --------
empty("notStarted", .notStarted)
empty("running", .running)
empty("unknown", .unknown)
empty("unavailable", .unavailable(reasons[0]))

// -------- 准入闸门 --------
gate("running", .running)
gate("notStarted", .notStarted)
gate("unknown", .unknown)
gate("unavailable", .unavailable(reasons[0]))
gate("timeout", .unavailable(AppModel.handshakeTimeoutMessage))
print("A help \(flat(EngineGate.unavailableHelp))")

// -------- 轮询节拍 --------
print("P \(TransferListPoll.intervalNanoseconds)")
print("P-home \(TransferReveal.downloadRootRelativeToHome)")

// -------- 落盘路径 --------
reveal("nil", nil, "/Users/x", "")
reveal("empty", "", "/Users/x", "")
reveal("plain", "a.bin", "/Users/x", "")
reveal("subdir", "sub dir/QC 图.png", "/Users/x", "")
reveal("home-slash", "a.bin", "/Users/x/", "")
reveal("home-root", "a.bin", "/", "")
reveal("home-empty", "a.bin", "", "")
reveal("configured", "a.bin", "/Users/x", "/Volumes/Data/交付")
reveal("configured-slash", "a.bin", "/Users/x", "/Volumes/Data/交付/")
reveal("configured-root", "a.bin", "/Users/x", "/")
reveal("configured-double", "a.bin", "/Users/x", "C:/下载//")
reveal("configured-verbatim", "a/../b.bin", "/Users/x", "/Volumes/Data/交付")
root("default", "/Users/x", "")
root("home-slash", "/Users/x/", "")
root("home-root", "/", "")
root("home-empty", "", "")
root("configured", "/Users/x", "/Volumes/Data/交付")
root("configured-slash", "/Users/x", "/Volumes/Data/交付/")
// ⚠️ 可比范围见 Rust 侧同段的注释（`H` 行在**任何宿主上**都逐字可比：
//    每侧夹具按**各自实现读的那一级**建，而打印出来的只有结论、不含键名）。
homeOf("hit", ["HOME": "/tmp/t8home"], "/Users/x")
homeOf("miss", [:], "/Users/x")
homeOf("empty", ["HOME": ""], "/Users/x")
homeOf("other-key", ["USERPROFILE": "/other-home"], "/Users/x")

// -------- 动作失败：内核原文逐字（走抽出来的真 message(of:)）--------
print("F0 \(flat(TransferActionFailure.message(of: CoreError.rpc(code: "invalid_params", message: "GID g1 没有路径映射（可能已被移除）"))))")
print("F1 \(flat(TransferActionFailure.message(of: CoreError.transport("内核进程已退出（管道结束）"))))")
print("F2 \(flat(TransferActionFailure.message(of: CoreError.rpc(code: "no_delivery", message: ""))))")
SWIFT

  cat > "$w/driver.rs" <<'RUST'
// Rust 侧驱动。`#[path]` 直接把工作树的源码按模块加载（**一个字节都没改**）。
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use client::ClientError;
use presentation::transfer_row::{
    EngineBanner, EngineGate, TransferActionFailure, TransferGlobalSummary, TransferListEmpty,
    TransferListPoll, TransferReveal, TransferRow,
};
use presentation::RowColor;
use protocol::{EngineState, GlobalStat, TaskAction, TaskState, TransferItem};
use std::collections::BTreeMap;

/// ⚠️ 这里的常量与 Swift 侧**同一个来源**：Swift 驱动用的是从 `AppModel.swift` 抽出来的
/// `AppModel.handshakeTimeoutMessage`，这边用 `engine_status` 里那一条（**不重写第二份字面量**）。
const HANDSHAKE_TIMEOUT_MESSAGE: &str = presentation::engine_status::HANDSHAKE_TIMEOUT_MESSAGE;

/// **另一平台**（不是本靶这一族）—— 驱动里**不写第二份平台判定**：
/// `cfg!` 在本工作树里只许出现在 `platform.rs` 的 `PLATFORM` 一处（裁定 JJ），
/// 这里若自己写一个 `cfg!(target_os = "windows")` 就是那份判定的第二份副本，
/// 而两份副本会分叉。取的是同一条判据的**另一支**。
fn other_platform() -> platform::Platform {
    match platform::PLATFORM {
        platform::Platform::Windows => platform::Platform::Other,
        platform::Platform::Other => platform::Platform::Windows,
    }
}

fn flat(s: &str) -> String { s.replace('\n', "\\n").replace('\r', "\\r") }
fn show(s: Option<&str>) -> String { s.map(flat).unwrap_or_else(|| "-".to_string()) }

fn color_name(c: RowColor) -> &'static str {
    match c {
        RowColor::Secondary => "secondary",
        RowColor::Blue => "blue",
        RowColor::Green => "green",
        RowColor::Red => "red",
        RowColor::Orange => "orange",
    }
}
/// 按**线上字面量**排序（`BTreeSet` 的原生次序是变体声明序，与 Swift 侧排出来的不一样）；
/// 判据"集合相等"不受顺序影响，排序只为让两侧能逐行 diff。
fn action_names(s: &std::collections::BTreeSet<TaskAction>) -> String {
    let mut names: Vec<String> = s
        .iter()
        .map(|a| serde_json::to_value(a).expect("TaskAction 一定序列化得出来").as_str().expect("线上是字符串").to_string())
        .collect();
    names.sort();
    names.join(",")
}

/// 与 Swift 侧 `fixture(...)` 逐行同形（默认值一致、`path: None` 发 `"path":null`）。
#[derive(Clone)]
struct Fx {
    gid: String,
    total: i64,
    completed: i64,
    speed: i64,
    state: TaskState,
    raw_status: String,
    error_message: String,
    path: Option<String>,
}
impl Default for Fx {
    fn default() -> Self {
        Self {
            gid: "g-1".to_string(), total: 1000, completed: 250, speed: 512,
            state: TaskState::Active, raw_status: "active".to_string(),
            error_message: String::new(), path: Some("a.bin".to_string()),
        }
    }
}
impl Fx {
    fn build(&self) -> TransferItem {
        let mut obj = serde_json::json!({
            "gid": &self.gid, "total": self.total, "completed": self.completed,
            "speed": self.speed, "conns": 2,
            "state": serde_json::to_value(self.state).unwrap(),
            "raw_status": &self.raw_status, "error_message": &self.error_message,
        });
        obj["path"] = match &self.path {
            Some(p) => serde_json::Value::String(p.clone()),
            None => serde_json::Value::Null,
        };
        serde_json::from_value(obj).expect("夹具必须能解")
    }
}

fn row(label: &str, f: Fx) {
    let r = TransferRow::new(&f.build());
    println!(
        "R {} | {} | {} | {} | {} | {:.6} | {} | {} | {} | {} | {} | {} | {} | {}",
        label, flat(&r.title), show(r.manifest_path.as_deref()), flat(&r.progress_text),
        r.percent_text, r.progress_fraction, flat(&r.speed_text), flat(&r.state_label),
        color_name(r.state_color), r.state_icon_name,
        if r.shows_paused_badge { 1 } else { 0 },
        show(r.error_text.as_deref()), action_names(&r.available_actions),
        if r.can_reveal_in_finder() { 1 } else { 0 },
    );
}
fn banner(label: &str, e: &EngineState, last_error: Option<&str>) {
    match EngineBanner::of(e, last_error) {
        None => println!("B {label} none"),
        Some(b) => {
            let kind = match b.kind {
                presentation::transfer_row::EngineBannerKind::EngineUnavailable => "engineUnavailable",
                presentation::transfer_row::EngineBannerKind::TransientError => "transientError",
            };
            println!("B {} {} | {} | {} | {}", label, kind, flat(&b.text), show(b.hint.as_deref()),
                     if b.shows_retry { 1 } else { 0 });
        }
    }
}
fn empty(label: &str, e: &EngineState) { println!("E {} {}", label, show(TransferListEmpty::of(e))); }
fn gate(label: &str, e: &EngineState) {
    println!("A {} {}", label, if EngineGate::allows_requests(e) { 1 } else { 0 });
}
fn reveal(label: &str, manifest: Option<&str>, home: &str, dir: &str) {
    println!("V {} {}", label, show(TransferReveal::local_path(manifest, home, dir).as_deref()));
}
fn root(label: &str, home: &str, dir: &str) {
    println!("D {} {}", label, TransferReveal::download_root(home, dir));
}
fn home_of(label: &str, environment: &[(&str, &str)], fallback: &str) {
    let m: BTreeMap<String, String> = environment.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    println!("H {} {}", label, TransferReveal::home(&m, fallback));
}

fn main() {
    row("default", Fx::default());
    row("path-nil", Fx { path: None, ..Default::default() });
    row("path-empty", Fx { path: Some(String::new()), ..Default::default() });
    row("path-verbatim", Fx { path: Some("sub dir/C24-8_×_25WS024/QC 图.png".to_string()), ..Default::default() });
    row("error-multiline", Fx {
        state: TaskState::Error, raw_status: "error".to_string(),
        error_message: "连接超时（第 3 次重试）\n原因：Connection reset by peer".to_string(),
        ..Default::default() });
    row("no-error", Fx { state: TaskState::Active, error_message: String::new(), ..Default::default() });
    row("total-zero", Fx { total: 0, completed: 0, ..Default::default() });
    row("over-report", Fx { total: 100, completed: 250, ..Default::default() });
    row("negative", Fx { total: 100, completed: -5, ..Default::default() });
    row("quarter", Fx { total: 400, completed: 100, ..Default::default() });
    row("progress-line", Fx { total: 2048, completed: 1024, ..Default::default() });
    row("speed-zero", Fx { speed: 0, ..Default::default() });
    row("paused", Fx { state: TaskState::Waiting, raw_status: "paused".to_string(), ..Default::default() });
    row("queued", Fx { state: TaskState::Waiting, raw_status: "waiting".to_string(), ..Default::default() });
    row("complete", Fx { state: TaskState::Complete, raw_status: "complete".to_string(), ..Default::default() });
    row("removed", Fx { state: TaskState::Removed, raw_status: "removed".to_string(), ..Default::default() });
    row("no-gid", Fx { gid: String::new(), state: TaskState::Active, ..Default::default() });
    for s in [TaskState::Waiting, TaskState::Active, TaskState::Complete, TaskState::Error, TaskState::Removed] {
        let label = format!("state-{}", serde_json::to_value(s).unwrap().as_str().unwrap());
        row(&label, Fx { state: s, ..Default::default() });
    }
    row("huge-half", Fx { total: i64::MAX, completed: i64::MAX / 2, ..Default::default() });
    row("huge-full", Fx { total: i64::MAX, completed: i64::MAX, ..Default::default() });
    row("huge-over", Fx { total: 1_000_000_000_000_000_000, completed: i64::MAX, ..Default::default() });

    let gs = GlobalStat { download_speed: 900, num_active: 2, num_waiting: 3, num_stopped: 4 };
    let s = TransferGlobalSummary::of(&gs);
    println!("G all {} | {} | {}", flat(&s.speed_text), flat(&s.activity_text), if s.can_clear_finished { 1 } else { 0 });
    let g0 = TransferGlobalSummary::of(&GlobalStat { download_speed: 0, num_active: 0, num_waiting: 0, num_stopped: 0 });
    println!("G zero {} | {} | {}", flat(&g0.speed_text), flat(&g0.activity_text), if g0.can_clear_finished { 1 } else { 0 });
    let g1 = TransferGlobalSummary::of(&GlobalStat { download_speed: 0, num_active: 1, num_waiting: 0, num_stopped: 1 });
    println!("G one-stopped {} | {} | {}", flat(&g1.speed_text), flat(&g1.activity_text), if g1.can_clear_finished { 1 } else { 0 });

    let reasons = [
        "下载引擎已断开：内核进程已退出（管道结束）",
        "引擎启动失败：aria2c 没有在 30 秒内就绪",
        "内核重启失败：找不到内核可执行文件 benagen-core",
        "内核崩了\n第二行",
        HANDSHAKE_TIMEOUT_MESSAGE,
    ];
    for (i, why) in reasons.iter().enumerate() {
        banner(&format!("unavailable-{i}"), &EngineState::Unavailable((*why).to_string()), None);
    }
    banner("unavailable-stale", &EngineState::Unavailable(reasons[0].to_string()), Some("上一拍的瞬时错误"));
    banner("transient", &EngineState::Running, Some("下载引擎 RPC 失败：connection reset"));
    banner("transient-empty", &EngineState::Running, Some(""));
    banner("running-none", &EngineState::Running, None);
    banner("notStarted-none", &EngineState::NotStarted, None);
    banner("unknown-none", &EngineState::Connecting, None);
    banner("timeout-with-stale", &EngineState::Unavailable(HANDSHAKE_TIMEOUT_MESSAGE.to_string()), Some("上一拍的瞬时错误"));

    empty("notStarted", &EngineState::NotStarted);
    empty("running", &EngineState::Running);
    empty("unknown", &EngineState::Connecting);
    empty("unavailable", &EngineState::Unavailable(reasons[0].to_string()));

    gate("running", &EngineState::Running);
    gate("notStarted", &EngineState::NotStarted);
    gate("unknown", &EngineState::Connecting);
    gate("unavailable", &EngineState::Unavailable(reasons[0].to_string()));
    gate("timeout", &EngineState::Unavailable(HANDSHAKE_TIMEOUT_MESSAGE.to_string()));
    println!("A help {}", flat(EngineGate::UNAVAILABLE_HELP));

    println!("P {}", TransferListPoll::INTERVAL_NANOSECONDS);
    println!("P-home {}", TransferReveal::DOWNLOAD_ROOT_RELATIVE_TO_HOME);

    reveal("nil", None, "/Users/x", "");
    reveal("empty", Some(""), "/Users/x", "");
    reveal("plain", Some("a.bin"), "/Users/x", "");
    reveal("subdir", Some("sub dir/QC 图.png"), "/Users/x", "");
    reveal("home-slash", Some("a.bin"), "/Users/x/", "");
    reveal("home-root", Some("a.bin"), "/", "");
    reveal("home-empty", Some("a.bin"), "", "");
    reveal("configured", Some("a.bin"), "/Users/x", "/Volumes/Data/交付");
    reveal("configured-slash", Some("a.bin"), "/Users/x", "/Volumes/Data/交付/");
    reveal("configured-root", Some("a.bin"), "/Users/x", "/");
    reveal("configured-double", Some("a.bin"), "/Users/x", "C:/下载//");
    reveal("configured-verbatim", Some("a/../b.bin"), "/Users/x", "/Volumes/Data/交付");
    root("default", "/Users/x", "");
    root("home-slash", "/Users/x/", "");
    root("home-root", "/", "");
    root("home-empty", "", "");
    root("configured", "/Users/x", "/Volumes/Data/交付");
    root("configured-slash", "/Users/x", "/Volumes/Data/交付/");
    // ⚠️ **可比范围（只说全，别只点名一行）**：
    //    · `hit`/`empty`/`miss` 三行：**每侧的环境字典都按各自实现读的那一级来建**
    //      （Rust 侧 `TransferReveal::home_variable()`，Swift 侧源里写死的 `"HOME"`），
    //      而驱动打印的只有结论、**不含键名** ⇒ 这几行在**任何宿主上**都逐字可比。
    //      ⚠️ 这条曾经不是这样：早先 Rust 侧也写死 `"HOME"`，于是 Windows 宿主上
    //      实现读 `%USERPROFILE%`、夹具却给 `HOME` ⇒ 三行会红。夹具改成"按各自实现读的键建"
    //      之后，那个分叉在**任何宿主上**都不成立 —— 所以这里**不再是**一条"在 Windows
    //      上会红的、红得有理由"的 case，它就是一条普通的对拍。
    //    · `other-key` 一行是"两级里取哪一级"的**探针**：只放**另一平台**那一级
    //      （`other_platform()`，**不是**驱动里自己写的平台判定 —— 那会是第二份副本），
    //      两侧都必须回落 ⇒ 同样在任何宿主上都可比（两侧都出 `/Users/x`）。
    //    **真正钉住"两级里取哪一级"的是单测**（`platform.rs` 的两支断言 + `transfer_row.rs` 的
    //    `the_download_root_uses_the_same_home_source_as_the_kernel`）；本对拍这一段的职责只有一条：
    //    **两侧在同一批输入上逐字算得一样**。
    home_of("hit", &[(TransferReveal::home_variable(), "/tmp/t8home")], "/Users/x");
    home_of("miss", &[], "/Users/x");
    home_of("empty", &[(TransferReveal::home_variable(), "")], "/Users/x");
    home_of(
        "other-key",
        &[(platform::env_var_name(other_platform()), "/other-home")],
        "/Users/x",
    );

    println!("F0 {}", flat(&TransferActionFailure::message_of(&ClientError::Kernel {
        code: "invalid_params".to_string(), message: "GID g1 没有路径映射（可能已被移除）".to_string() })));
    println!("F1 {}", flat(&TransferActionFailure::message_of(&ClientError::KernelGone)));
    println!("F2 {}", flat(&TransferActionFailure::message_of(&ClientError::Kernel {
        code: "no_delivery".to_string(), message: String::new() })));
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$MACOS_SRC/Presentation/Format.swift" \
         "$MACOS_SRC/Presentation/EngineStatusPresentation.swift" \
         "$MACOS_SRC/Presentation/TransferRow.swift" \
         "$w/color_extract.swift" "$w/appmodel_extract.swift" \
         "$w/main.swift" -o "$w/tr_swift"
  build_rust_driver "$w/driver.rs" "$w/tr_rust"

  "$w/tr_swift" > "$w/swift.out"
  "$w/tr_rust" > "$w/rust.out"
  diff_outputs "transfer_row" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# case：browser_row（任务 13）
# ---------------------------------------------------------------------------
#
# ⚠️ **可比范围**：本 case 覆盖任务 13 移植的全部纯计算 —— 行（判别键 / 路径 / 三列的值）、
#    状态样式（五格 × 标签 × 语义色）、排序、默认选中面、整批全选、底部汇总、
#    `.task` 触发键、以及**`ManifestTracking` 的整套状态转移**（复位 + 播种两半）。
#
#    ⚠️ **`SourceTimeText` 不直接调**：它在 Swift 侧是 internal（同模块的驱动够得着），
#       而 Rust 侧的对应物是 `browser_row` 的**私有**项（同一个模块之外够不着）。
#       所以时间列一律**经线上输入到输出的整条路**对拍（那本来也是更强的形态）：
#       一列 `source_mtime` 取值各起一行，外加 `list_dir` 老载荷那条"键根本不在"。
#
#    ⚠️ **已知的形状差异（不参与本 case 的判据）**：`task_key` 的长度前缀，上游数**字素簇**
#       （`String.count`）、Rust 数 **Unicode 标量**（`chars().count()`）。两者只在组合序列
#       （基字 + 组合记号、ZWJ 表情）上不同，而那时两侧的 key 会不同 —— 有意不喂那种输入，
#       因为"这把 key 只在壳自己这一侧用、不跨语言"（见 `browser_row.rs` 的 W-6 记账）。
case_browser_row() {
  local w="$WORK/browser_row"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"   # `#[path]` 相对驱动文件所在目录解析

  # ⚠️ 两份抽出来的零件都是**必须**的，与 `breadcrumb_summary` 同一个抽法、同一条纪律
  #    （按内容锚点抽、抽歪了大声失败）：
  #      · `AppModel.LoadState` —— `DeliverySummary.swift`（`TimestampPresentation` 的出处）
  #        的第二个 `of` 收它；
  #      · `AppModel.message(of:)` —— `Breadcrumb.swift` 的 `DirLoadFailure.of` 调它。
  extract_appmodel_message "$w/msg_extract.swift"
  extract_appmodel_load_state "$w/load_state_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

/// 与 Rust 侧 `flat` 同形（换行会破坏一行一个结论的 diff）。
func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}
func show(_ s: String?) -> String { s.map(flat) ?? "-" }
func n(_ v: Int?) -> String { v.map(String.init) ?? "-" }
func n64(_ v: Int64?) -> String { v.map(String.init) ?? "-" }
func joined(_ s: Set<String>) -> String { s.sorted().joined(separator: "|") }

func colorName(_ c: RowColor) -> String {
    switch c {
    case .secondary: return "secondary"
    case .blue: return "blue"
    case .green: return "green"
    case .red: return "red"
    case .orange: return "orange"
    }
}

// -------- 夹具：一律是**内核会发的那种线上 JSON 文本** --------
// 路径是清单原文：`×` 与空格逐字，不得转义。
let listDirWire = #"""
{"path":"client-test/C24-8_×_25WS024",
 "entries":[{"type":"file","name":"QC 图.png",
             "path":"client-test/C24-8_×_25WS024/QC 图.png","crc64":"",
             "size":7000,"completed":0,"total":7000,"speed":0,"state":"pending","err":""},
            {"type":"dir","name":"Zeta","children_count":1},
            {"type":"file","name":"reads.fq.gz",
             "path":"client-test/C24-8_×_25WS024/reads.fq.gz","crc64":"9988776655443322110",
             "size":2048,"completed":2048,"total":2048,"speed":0,"state":"complete","err":""},
            {"type":"dir","name":"Figure","children_count":3}]}
"""#
/// 根那一层：只有一个顶层目录（钉"根下不得出现前导 /"）。
let rootWire = #"{"path":"","entries":[{"type":"dir","name":"client-test","children_count":2}]}"#
let parent = "client-test/C24-8_×_25WS024"

/// 时间列：一列 `source_mtime` 各起一行（形状合法 / 空串 / 认不出来 / 各种反例）。
/// ⚠️ 里面**不带引号与反斜杠**，所以驱动可以直接拼串（两侧拼法一致，只为能解出来）。
let timeRaws = ["2026-09-14T12:00:00+08:00", "", "待定",
                "2026-09-17T09:37:12.805751+08:00", "2026-10-14T16:13:34Z",
                "2026-10-14T23:13:34-05:00", "2026-10-14T16:13:34.1Z",
                "2026-09-14", "2026-09-14T12:00", "2026-10-14T16:13:34+0800",
                "2026-10-14T16:13:34+08:00 尾巴", "2026-10-14t16:13:34+08:00",
                "0000-00-00T00:00:00Z", "2026-10-14T16:13:34.Z"]
func buildTimeWire(_ raws: [String]) -> String {
    let es = raws.enumerated().map { (i, raw) in
        "{\"type\":\"file\",\"name\":\"f\(i).bin\",\"path\":\"t/f\(i).bin\",\"crc64\":\"\","
        + "\"size\":1,\"completed\":0,\"total\":1,\"speed\":0,\"state\":\"pending\",\"err\":\"\","
        + "\"source_mtime\":\"\(raw)\"}"
    }
    return "{\"path\":\"t\",\"entries\":[\(es.joined(separator: ","))]}"
}
let timeWire = buildTimeWire(timeRaws)

func listDir(_ j: String) -> ListDirResult {
    try! CoreJSON.decoder.decode(ListDirResult.self, from: Data(j.utf8))
}
func tree(_ j: String) -> TreeResult {
    try! CoreJSON.decoder.decode(TreeResult.self, from: Data(j.utf8))
}
let treeWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"a.bin","name":"a.bin","size":2048,"state":"pending"},
         {"path":"b.bin","name":"b.bin","size":3072,"state":"complete"}],
 "default_selected":["a.bin"],
 "progress":{"total_bytes":5120,"done_bytes":3072,"speed":0,"percent":60}}
"""#
/// **上一批**的树（`default_selected` 与上面**不同** ⇒ "播错了"才可观测）。
let treeOldWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"z.bin","name":"z.bin","size":1024,"state":"pending"}],
 "default_selected":["z.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}
"""#
/// **另一批**的树（三份的 `default_selected` 两两不同）。
let treeBWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"b.bin","name":"b.bin","size":1024,"state":"pending"}],
 "default_selected":["b.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}
"""#
/// `flat` 里 `path` 与 `name` **不同**（用来钉"取的是 path"）。
let treePrefixWire = #"""
{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"C24-8_×_25WS024/Figure/QC 图.png","name":"QC 图.png","size":1,"state":"pending"}],
 "default_selected":[],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}
"""#

let treeNew = tree(treeWire), treeOld = tree(treeOldWire), treeB = tree(treeBWire)
let treePrefix = tree(treePrefixWire)

// -------- ① 行：逐列 --------
func emitRows(_ label: String, _ j: String, _ p: String) {
    let rs = BrowserRow.rows(listDir(j).entries, parent: p)
    for (i, r) in rs.enumerated() {
        print("R \(label) \(i) \(r.kind.rawValue)|\(flat(r.name))|\(flat(r.path))"
            + "|\(n(r.childrenCount))|\(n64(r.size))|\(flat(r.detailText))"
            + "|\(flat(r.sourceTimeText))|\(r.state.label)|\(colorName(r.state.color))|\(r.iconName)")
    }
    print("ORDER \(label) \(flat(rs.map(\.name).joined(separator: "|")))")
}
emitRows("level", listDirWire, parent)
emitRows("root", rootWire, "")
emitRows("times", timeWire, "t")
// 老载荷：**没有** `source_mtime` 这个键 ⇒ 时间列必须是 `—`，不是空白。
emitRows("old", listDirWire, parent)

// -------- ② 状态样式：五格 × 标签 × 语义色；四态各一格 --------
for (i, s) in RowStateStyle.allCases.enumerated() {
    print("ST \(i) \(s.label) \(colorName(s.color))")
}
for f in FileState.allCases {
    print("SF \(f) \(RowStateStyle.of(f).label) \(colorName(RowStateStyle.of(f).color))")
}
print("SF nil \(RowStateStyle.of(nil).label) \(colorName(RowStateStyle.of(nil).color))")

// -------- ③ 换批复位的判据（onLoad）--------
func showLoad(_ label: String, _ l: ManifestLoad) {
    print("L \(label) \(l.isANewManifest ? 1 : 0) \(show(l.lastDisplayedCode))")
}
showLoad("cold-nil", BrowserSelection.onLoad(code: nil, lastDisplayedCode: nil))
showLoad("nil-after-A", BrowserSelection.onLoad(code: nil, lastDisplayedCode: "AAA-1"))
showLoad("A-cold", BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: nil))
showLoad("A-after-A", BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: "AAA-1"))
showLoad("B-after-A", BrowserSelection.onLoad(code: "BBB-2", lastDisplayedCode: "AAA-1"))
showLoad("A-after-B", BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: "BBB-2"))
// A→B→A 三步（承重事项 A 的守卫）
let l1 = BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: nil)
showLoad("step1", l1)
let l2 = BrowserSelection.onLoad(code: "BBB-2", lastDisplayedCode: l1.lastDisplayedCode)
showLoad("step2", l2)
let l3 = BrowserSelection.onLoad(code: "AAA-1", lastDisplayedCode: l2.lastDisplayedCode)
showLoad("step3", l3)

// -------- ④ 默认选中面：树必须属于这一批 --------
func showNM(_ label: String, _ s: Set<String>?) {
    print("NM \(label) \(s.map(joined) ?? "-")")
}
showNM("new-no-tree", BrowserSelection.onNewManifest(code: "NEW", previousCode: nil, treeCode: nil, tree: nil))
showNM("new-claim-no-tree", BrowserSelection.onNewManifest(code: "NEW", previousCode: nil, treeCode: "NEW", tree: nil))
showNM("new-other-tree", BrowserSelection.onNewManifest(code: "NEW", previousCode: nil, treeCode: "OLD", tree: treeOld))
showNM("new-own-tree", BrowserSelection.onNewManifest(code: "NEW", previousCode: nil, treeCode: "NEW", tree: treeNew))
showNM("same-batch", BrowserSelection.onNewManifest(code: "C24-8", previousCode: "C24-8", treeCode: "C24-8", tree: treeNew))
showNM("other-batch", BrowserSelection.onNewManifest(code: "BBB-2", previousCode: "C24-8", treeCode: "BBB-2", tree: treeNew))

// -------- ⑤ 全选当前层 / 整批全部文件 --------
let level = BrowserRow.rows(listDir(listDirWire).entries, parent: parent)
print("ALL \(flat(joined(BrowserSelection.all(in: level))))")
print("AF \(flat(joined(BrowserSelection.allFiles(in: treeNew.flat))))")
print("AF-empty \(flat(joined(BrowserSelection.allFiles(in: []))))")
print("AF-prefix \(flat(joined(BrowserSelection.allFiles(in: treePrefix.flat))))")

// -------- ⑥ 底部汇总 --------
let index = SelectionSummary.sizeIndex(treeNew.flat)
let sums: [(String, [String])] = [("both", ["a.bin", "b.bin"]), ("empty", []),
                                  ("dir-only", ["client-test/C24-8_×_25WS024/Figure"]),
                                  ("mixed", ["client-test/C24-8_×_25WS024/Figure", "a.bin"]),
                                  ("unknown", ["nope"]), ("prefix", ["C24-8_×_25WS024/Figure/QC 图.png"])]
for (label, sel) in sums {
    let s = SelectionSummary.of(selected: Set(sel), sizes: index)
    print("SUM \(label) \(flat(s.countText)) | \(flat(s.sizeText))")
}

// -------- ⑦ `.task` 触发键 --------
let keys: [(String, String, String?, Int)] = [
    ("AAA-1", "", nil, 1), ("AAA-1", "", "AAA-1", 1), ("AAA-1", "", "AAA-1", 2),
    ("A|B", "", nil, 1), ("A", "B|", nil, 1), ("A", "", "B|", 1), ("A", "B|", "", 1),
    ("A", "", "B|1", 0), ("A", "", "B", 1),
    ("C24-8_×_25WS024", "a/b c", nil, 42), ("", "", nil, 0), ("", "a", "b", -1),
]
for (i, k) in keys.enumerated() {
    print("K \(i) \(flat(BrowserSelection.taskKey(code: k.0, path: k.1, treeCode: k.2, generation: k.3)))")
}

// -------- ⑧ `ManifestTracking` 的整套状态转移 --------
func showD(_ label: String, _ b: Bool) { print("D \(label) \(b ? 1 : 0)") }
func showS(_ label: String, _ r: ManifestSeeding?) {
    if let r {
        print("S \(label) \(flat(joined(r.selection))) \(r.isANewBatch ? 1 : 0)")
    } else {
        print("S \(label) - -")
    }
}
var t = ManifestTracking()
showD("d1", t.display(code: "AAA-1"))                                   // 第一次显示 ⇒ 换批
showS("s1", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeNew, generation: 1))   // 播 a.bin / 换批
showS("s2", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeNew, generation: 1))   // 同一代 ⇒ 不播
showD("d2", t.display(code: "AAA-1"))                                   // 同一批 ⇒ 不复位
showS("s3", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeNew, generation: 1))   // 仍不播
showS("s4", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeB, generation: 2))     // 同码重载 ⇒ 播 b.bin / **不算换批**
showD("d3", t.display(code: "BBB-2"))                                   // 换批
showS("s5", t.seed(code: "BBB-2", treeCode: nil, tree: nil, generation: 3))           // 树没到 ⇒ 推迟
showS("s6", t.seed(code: "BBB-2", treeCode: "AAA-1", tree: treeNew, generation: 3))   // 旧树 ⇒ 推迟
showD("d4", t.display(code: "AAA-1"))                                   // A→B→A ⇒ 换批
showS("s7", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeNew, generation: 3))   // **再播一次**
showD("d5", t.display(code: nil))                                       // 抖动 ⇒ 不复位
showD("d6", t.display(code: "AAA-1"))                                   // 同一批 ⇒ 不复位
showS("s8", t.seed(code: "AAA-1", treeCode: "AAA-1", tree: treeNew, generation: 3))   // 没有新加载 ⇒ 不重播

// 那个已知竞态：换批复位落在播种**后面**时，必须还能补播。
var u = ManifestTracking()
showD("r1", u.display(code: "AAA-1"))
showS("r2", u.seed(code: "BBB-2", treeCode: "BBB-2", tree: treeB, generation: 2))
showD("r3", u.display(code: "BBB-2"))
showS("r4", u.seed(code: "BBB-2", treeCode: "BBB-2", tree: treeB, generation: 2))
SWIFT

  cat > "$w/driver.rs" <<'RUST'
// Rust 侧驱动。`#[path]` 直接把工作树的源码按模块加载（**一个字节都没改**）。
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use presentation::browser_row::{
    BrowserRow, BrowserRowKind, BrowserSelection, ManifestSeeding, ManifestTracking, RowStateStyle,
    SelectionSummary,
};
use presentation::RowColor;
use protocol::{FileState, ListDirResult, TreeResult};
use std::collections::BTreeSet;

/// 与 Swift 侧 `flat` 同形（换行会破坏一行一个结论的 diff）。
fn flat(s: &str) -> String { s.replace('\n', "\\n").replace('\r', "\\r") }
fn show(s: Option<&str>) -> String { s.map(flat).unwrap_or_else(|| "-".to_string()) }
fn n(v: Option<i64>) -> String { v.map(|x| x.to_string()).unwrap_or_else(|| "-".to_string()) }
fn joined(s: &BTreeSet<String>) -> String { s.iter().cloned().collect::<Vec<_>>().join("|") }

fn color_name(c: RowColor) -> &'static str {
    match c {
        RowColor::Secondary => "secondary",
        RowColor::Blue => "blue",
        RowColor::Green => "green",
        RowColor::Red => "red",
        RowColor::Orange => "orange",
    }
}
fn kind_name(k: BrowserRowKind) -> &'static str {
    match k {
        BrowserRowKind::Dir => "dir",
        BrowserRowKind::File => "file",
    }
}
/// `FileState` 的**线上字面量**（与 Swift 侧 `"\(f)"` 同一个口径：都是那个小写串）。
fn state_name(f: FileState) -> String {
    serde_json::to_value(f).expect("FileState 一定序列化得出来").as_str().expect("线上是字符串").to_string()
}

fn list_dir(j: &str) -> ListDirResult {
    serde_json::from_str(j).expect("夹具是内核会发的线上原文，必须能解")
}
fn tree(j: &str) -> TreeResult {
    serde_json::from_str(j).expect("夹具是内核会发的线上原文，必须能解")
}

const LIST_DIR_WIRE: &str = r#"{"path":"client-test/C24-8_×_25WS024",
 "entries":[{"type":"file","name":"QC 图.png",
             "path":"client-test/C24-8_×_25WS024/QC 图.png","crc64":"",
             "size":7000,"completed":0,"total":7000,"speed":0,"state":"pending","err":""},
            {"type":"dir","name":"Zeta","children_count":1},
            {"type":"file","name":"reads.fq.gz",
             "path":"client-test/C24-8_×_25WS024/reads.fq.gz","crc64":"9988776655443322110",
             "size":2048,"completed":2048,"total":2048,"speed":0,"state":"complete","err":""},
            {"type":"dir","name":"Figure","children_count":3}]}"#;
const ROOT_WIRE: &str = r#"{"path":"","entries":[{"type":"dir","name":"client-test","children_count":2}]}"#;
const PARENT: &str = "client-test/C24-8_×_25WS024";

const TREE_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"a.bin","name":"a.bin","size":2048,"state":"pending"},
         {"path":"b.bin","name":"b.bin","size":3072,"state":"complete"}],
 "default_selected":["a.bin"],
 "progress":{"total_bytes":5120,"done_bytes":3072,"speed":0,"percent":60}}"#;
const TREE_OLD_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"z.bin","name":"z.bin","size":1024,"state":"pending"}],
 "default_selected":["z.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}"#;
const TREE_B_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"b.bin","name":"b.bin","size":1024,"state":"pending"}],
 "default_selected":["b.bin"],
 "progress":{"total_bytes":1024,"done_bytes":0,"speed":0,"percent":0}}"#;
const TREE_PREFIX_WIRE: &str = r#"{"tree":{"type":"dir","name":"","children":{}},
 "flat":[{"path":"C24-8_×_25WS024/Figure/QC 图.png","name":"QC 图.png","size":1,"state":"pending"}],
 "default_selected":[],
 "progress":{"total_bytes":1,"done_bytes":0,"speed":0,"percent":0}}"#;

/// 与 Swift 侧 `timeRaws` **逐字同一批**（顺序也一样）。
const TIME_RAWS: [&str; 14] = ["2026-09-14T12:00:00+08:00", "", "待定",
    "2026-09-17T09:37:12.805751+08:00", "2026-10-14T16:13:34Z",
    "2026-10-14T23:13:34-05:00", "2026-10-14T16:13:34.1Z",
    "2026-09-14", "2026-09-14T12:00", "2026-10-14T16:13:34+0800",
    "2026-10-14T16:13:34+08:00 尾巴", "2026-10-14t16:13:34+08:00",
    "0000-00-00T00:00:00Z", "2026-10-14T16:13:34.Z"];

fn build_time_wire(raws: &[&str]) -> String {
    let es: Vec<String> = raws.iter().enumerate().map(|(i, raw)| format!(
        "{{\"type\":\"file\",\"name\":\"f{i}.bin\",\"path\":\"t/f{i}.bin\",\"crc64\":\"\",\
         \"size\":1,\"completed\":0,\"total\":1,\"speed\":0,\"state\":\"pending\",\"err\":\"\",\
         \"source_mtime\":\"{raw}\"}}")).collect();
    format!("{{\"path\":\"t\",\"entries\":[{}]}}", es.join(","))
}

fn emit_rows(label: &str, j: &str, p: &str) {
    let rs = BrowserRow::rows(&list_dir(j).entries, p);
    for (i, r) in rs.iter().enumerate() {
        println!("R {} {} {}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                 label, i, kind_name(r.kind), flat(&r.name), flat(&r.path),
                 n(r.children_count), n(r.size), flat(&r.detail_text),
                 flat(&r.source_time_text), r.state.label(), color_name(r.state.color()),
                 r.icon_name);
    }
    let names: Vec<String> = rs.iter().map(|r| r.name.clone()).collect();
    println!("ORDER {} {}", label, flat(&names.join("|")));
}

fn set(items: &[&str]) -> BTreeSet<String> { items.iter().map(|s| (*s).to_string()).collect() }

fn show_load(label: &str, l: &presentation::browser_row::ManifestLoad) {
    println!("L {} {} {}", label, if l.is_a_new_manifest { 1 } else { 0 },
             show(l.last_displayed_code.as_deref()));
}
fn show_nm(label: &str, s: Option<BTreeSet<String>>) {
    match s {
        Some(s) => println!("NM {} {}", label, flat(&joined(&s))),
        None => println!("NM {} -", label),
    }
}
fn show_d(label: &str, b: bool) { println!("D {} {}", label, if b { 1 } else { 0 }); }
fn show_s(label: &str, r: Option<ManifestSeeding>) {
    match r {
        Some(r) => println!("S {} {} {}", label, flat(&joined(&r.selection)), if r.is_a_new_batch { 1 } else { 0 }),
        None => println!("S {} - -", label),
    }
}

fn main() {
    let tree_new = tree(TREE_WIRE);
    let tree_old = tree(TREE_OLD_WIRE);
    let tree_b = tree(TREE_B_WIRE);
    let tree_prefix = tree(TREE_PREFIX_WIRE);

    // -------- ① 行：逐列 --------
    emit_rows("level", LIST_DIR_WIRE, PARENT);
    emit_rows("root", ROOT_WIRE, "");
    emit_rows("times", &build_time_wire(&TIME_RAWS), "t");
    emit_rows("old", LIST_DIR_WIRE, PARENT);

    // -------- ② 状态样式 --------
    for (i, s) in RowStateStyle::ALL.iter().enumerate() {
        println!("ST {} {} {}", i, s.label(), color_name(s.color()));
    }
    for f in [FileState::Pending, FileState::Downloading, FileState::Complete, FileState::Failed] {
        println!("SF {} {} {}", state_name(f), RowStateStyle::of(Some(f)).label(),
                 color_name(RowStateStyle::of(Some(f)).color()));
    }
    println!("SF nil {} {}", RowStateStyle::of(None).label(), color_name(RowStateStyle::of(None).color()));

    // -------- ③ 换批复位的判据（on_load）--------
    show_load("cold-nil", &BrowserSelection::on_load(None, None));
    show_load("nil-after-A", &BrowserSelection::on_load(None, Some("AAA-1")));
    show_load("A-cold", &BrowserSelection::on_load(Some("AAA-1"), None));
    show_load("A-after-A", &BrowserSelection::on_load(Some("AAA-1"), Some("AAA-1")));
    show_load("B-after-A", &BrowserSelection::on_load(Some("BBB-2"), Some("AAA-1")));
    show_load("A-after-B", &BrowserSelection::on_load(Some("AAA-1"), Some("BBB-2")));
    let l1 = BrowserSelection::on_load(Some("AAA-1"), None);
    show_load("step1", &l1);
    let l2 = BrowserSelection::on_load(Some("BBB-2"), l1.last_displayed_code.as_deref());
    show_load("step2", &l2);
    let l3 = BrowserSelection::on_load(Some("AAA-1"), l2.last_displayed_code.as_deref());
    show_load("step3", &l3);

    // -------- ④ 默认选中面 --------
    show_nm("new-no-tree", BrowserSelection::on_new_manifest("NEW", None, None, None));
    show_nm("new-claim-no-tree", BrowserSelection::on_new_manifest("NEW", None, Some("NEW"), None));
    show_nm("new-other-tree", BrowserSelection::on_new_manifest("NEW", None, Some("OLD"), Some(&tree_old)));
    show_nm("new-own-tree", BrowserSelection::on_new_manifest("NEW", None, Some("NEW"), Some(&tree_new)));
    show_nm("same-batch", BrowserSelection::on_new_manifest("C24-8", Some("C24-8"), Some("C24-8"), Some(&tree_new)));
    show_nm("other-batch", BrowserSelection::on_new_manifest("BBB-2", Some("C24-8"), Some("BBB-2"), Some(&tree_new)));

    // -------- ⑤ 全选当前层 / 整批全部文件 --------
    let level = BrowserRow::rows(&list_dir(LIST_DIR_WIRE).entries, PARENT);
    println!("ALL {}", flat(&joined(&BrowserSelection::all(&level))));
    println!("AF {}", flat(&joined(&BrowserSelection::all_files(&tree_new.flat))));
    println!("AF-empty {}", flat(&joined(&BrowserSelection::all_files(&[]))));
    println!("AF-prefix {}", flat(&joined(&BrowserSelection::all_files(&tree_prefix.flat))));

    // -------- ⑥ 底部汇总 --------
    let index = SelectionSummary::size_index(&tree_new.flat);
    let sums: [(&str, &[&str]); 6] = [("both", &["a.bin", "b.bin"]), ("empty", &[]),
        ("dir-only", &["client-test/C24-8_×_25WS024/Figure"]),
        ("mixed", &["client-test/C24-8_×_25WS024/Figure", "a.bin"]),
        ("unknown", &["nope"]), ("prefix", &["C24-8_×_25WS024/Figure/QC 图.png"])];
    for (label, sel) in sums.iter() {
        let s = SelectionSummary::of(&set(sel), &index);
        println!("SUM {} {} | {}", *label, flat(&s.count_text), flat(&s.size_text));
    }

    // -------- ⑦ `.task` 触发键 --------
    let keys: [(&str, &str, Option<&str>, i64); 12] = [
        ("AAA-1", "", None, 1), ("AAA-1", "", Some("AAA-1"), 1), ("AAA-1", "", Some("AAA-1"), 2),
        ("A|B", "", None, 1), ("A", "B|", None, 1), ("A", "", Some("B|"), 1), ("A", "B|", Some(""), 1),
        ("A", "", Some("B|1"), 0), ("A", "", Some("B"), 1),
        ("C24-8_×_25WS024", "a/b c", None, 42), ("", "", None, 0), ("", "a", Some("b"), -1)];
    for (i, k) in keys.iter().enumerate() {
        println!("K {} {}", i, flat(&BrowserSelection::task_key(k.0, k.1, k.2, k.3)));
    }

    // -------- ⑧ `ManifestTracking` 的整套状态转移 --------
    let mut t = ManifestTracking::new();
    show_d("d1", t.display(Some("AAA-1")));
    show_s("s1", t.seed("AAA-1", Some("AAA-1"), Some(&tree_new), 1));
    show_s("s2", t.seed("AAA-1", Some("AAA-1"), Some(&tree_new), 1));
    show_d("d2", t.display(Some("AAA-1")));
    show_s("s3", t.seed("AAA-1", Some("AAA-1"), Some(&tree_new), 1));
    show_s("s4", t.seed("AAA-1", Some("AAA-1"), Some(&tree_b), 2));
    show_d("d3", t.display(Some("BBB-2")));
    show_s("s5", t.seed("BBB-2", None, None, 3));
    show_s("s6", t.seed("BBB-2", Some("AAA-1"), Some(&tree_new), 3));
    show_d("d4", t.display(Some("AAA-1")));
    show_s("s7", t.seed("AAA-1", Some("AAA-1"), Some(&tree_new), 3));
    show_d("d5", t.display(None));
    show_d("d6", t.display(Some("AAA-1")));
    show_s("s8", t.seed("AAA-1", Some("AAA-1"), Some(&tree_new), 3));

    let mut u = ManifestTracking::new();
    show_d("r1", u.display(Some("AAA-1")));
    show_s("r2", u.seed("BBB-2", Some("BBB-2"), Some(&tree_b), 2));
    show_d("r3", u.display(Some("BBB-2")));
    show_s("r4", u.seed("BBB-2", Some("BBB-2"), Some(&tree_b), 2));
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$MACOS_SRC/Presentation/Format.swift" \
         "$MACOS_SRC/Presentation/Breadcrumb.swift" \
         "$MACOS_SRC/Presentation/DeliverySummary.swift" \
         "$MACOS_SRC/Presentation/BrowserRow.swift" \
         "$w/msg_extract.swift" "$w/load_state_extract.swift" \
         "$w/main.swift" -o "$w/br_swift"
  build_rust_driver "$w/driver.rs" "$w/br_rust"

  "$w/br_swift" > "$w/swift.out"
  "$w/br_rust" > "$w/rust.out"
  diff_outputs "browser_row" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# case：verify_summary（任务 14）
# ---------------------------------------------------------------------------
#
# ⚠️ **可比范围**：本 case 覆盖任务 14 移植的全部纯计算 —— 六类各行（标签/颜色/图标/
#    说明/计数/路径）、顶部总结四档、分区徽标两个计数、总进度五项、刷新失败那句话。
#    `VerifyRefreshFailure` 比的是 `.rpc` 那一路（Swift 的 `CoreError.rpc` ⇒ Rust 的
#    `ClientError::Kernel`）：两边交出来的都是**内核原文**，所以逐字可比；
#    `.transport`/`.malformedResponse` 在 Rust 侧是 `client.rs` 自己写的人话文案，
#    **不可比**（由 `verify_summary.rs` 的单测钉住）。
#
# ⚠️ 两侧的夹具**都走解码**（同一批 JSON 字面量的键 = 内核线上的 snake_case）。
#    手搓结构体会让"壳解不解得动内核发来的那一行"这件事在 diff 里凭空消失 ——
#    而那正是裁决 V 那个坑（按变体名解会得到六个空桶）要处理的形状。
case_verify_summary() {
  local w="$WORK/verify_summary"; mkdir -p "$w"
  ln -s "$WORK/src" "$w/src"   # `#[path]` 相对驱动文件所在目录解析

  # ⚠️ 这里用 `extract_appmodel_for_transfer_row`（不是 `extract_appmodel_message`）：
  #    `VerifySummary.swift` 的 `ProgressSummary` 走 `TransferRow.fraction`，所以
  #    `TransferRow.swift` 要一起编进来，而它需要 `AppModel.EngineState`
  #    （`EngineBanner`/`EngineGate` 的入参）与 `EngineStatusPresentation`。
  extract_appmodel_for_transfer_row "$w/appmodel_extract.swift"
  extract_row_color "$w/color_extract.swift"

  cat > "$w/main.swift" <<'SWIFT'
import Foundation

func flat(_ s: String) -> String {
    s.replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\r", with: "\\r")
}
func show(_ s: String?) -> String { s.map(flat) ?? "-" }
func colorName(_ c: RowColor) -> String {
    switch c {
    case .secondary: return "secondary"
    case .blue: return "blue"
    case .green: return "green"
    case .red: return "red"
    case .orange: return "orange"
    }
}
/// 路径之间用控制字符分隔（清单原文里可能出现 `|`：分隔符不能与数据撞）。
let sep = "\u{1}"

/// 一次 `verify_status` 的结果，**经解码构造**（键是内核线上的 snake_case）。
func status(ok: [String] = [], bad: [String] = [], missing: [String] = [],
            sizeMismatch: [String] = [], unverifiable: [String] = [], unreadable: [String] = [],
            allGood: Bool? = nil) -> VerifyStatus {
    let obj: [String: Any] = [
        "ok": ok, "bad": bad, "missing": missing,
        "size_mismatch": sizeMismatch,
        "unverifiable": unverifiable, "unreadable": unreadable,
        "all_good": allGood ?? (bad.isEmpty && missing.isEmpty
                                && sizeMismatch.isEmpty && unreadable.isEmpty),
    ]
    let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    return try! CoreJSON.decoder.decode(VerifyStatus.self, from: data)
}

/// 一次 `get_tree` 的结果，**经解码构造**（本 case 只用到 `progress`）。
func tree(total: Int64 = 4096, done: Int64 = 1024, speed: Int64 = 512,
          percent: Int32 = 25) -> TreeResult {
    let obj: [String: Any] = [
        "tree": [:], "flat": [], "default_selected": [],
        "progress": ["total_bytes": total, "done_bytes": done,
                     "speed": speed, "percent": Int(percent)],
    ]
    let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    return try! CoreJSON.decoder.decode(TreeResult.self, from: data)
}

/// `transfer_list` 的一项，**经解码构造**。
func item(_ gid: String, _ state: TaskState, _ rawStatus: String) -> TransferItem {
    let obj: [String: Any] = ["gid": gid, "total": 1000, "completed": 250, "speed": 0,
                              "conns": 1, "state": state.rawValue, "raw_status": rawStatus,
                              "error_message": "", "path": "\(gid).bin"]
    let data = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    return try! CoreJSON.decoder.decode(TransferItem.self, from: data)
}

/// 一份结果的全部呈现值：总结那一行 + 六行（**恒六行，含计数为 0 的**）。
func dump(_ label: String, _ s: VerifyStatus?) {
    guard let s = s, let sum = VerifySummary.of(s) else {
        print("S \(label) nil"); return
    }
    print("S \(label) \(sum.allGood ? 1 : 0) \(sum.failedCount) \(sum.classifiedCount)"
        + " | \(flat(sum.headline)) | \(colorName(sum.headlineColor)) | \(sum.headlineIcon)"
        + " | \(flat(sum.classifiedText))")
    for r in sum.rows {
        print("R \(label) \(r.id) \(flat(r.label)) \(colorName(r.color)) \(r.iconName)"
            + " \(r.count) \(flat(r.countText)) \(r.isFailure ? 1 : 0) \(show(r.note))"
            + " | \(flat(r.paths.joined(separator: sep)))")
    }
}

// -------- 六类 / 总结 --------
let weird = "client-test/C24-8_×_25WS024/reads 1.fq.gz"
let cases: [(String, VerifyStatus)] = [
    ("empty", status()),
    ("only-ok", status(ok: ["a"])),
    ("one-bad", status(bad: ["b"])),
    ("all-six", status(ok: ["p1"], bad: ["p2"], missing: ["p3"], sizeMismatch: ["p4"],
                       unverifiable: ["p5"], unreadable: ["p6"])),
    ("unverifiable-only", status(ok: ["a"], unverifiable: ["u", "v"])),
    ("verbatim", status(ok: [weird], bad: ["a/./b.txt", "a//b", "dir/", ""])),
    ("kernel-says-clear-but-failed", status(bad: ["b"], allGood: true)),
    ("kernel-contradicts", status(ok: ["a"], allGood: false)),
    ("unreadable-only", status(unreadable: ["u"])),
    ("size-and-missing", status(missing: ["m"], sizeMismatch: ["s"])),
]
for (label, s) in cases { dump(label, s) }
dump("nil", nil)

// -------- 分区徽标 --------
let items = [item("w", .waiting, "waiting"), item("a", .active, "active"),
             item("e", .error, "error"), item("c", .complete, "complete"),
             item("r", .removed, "removed"), item("p", .waiting, "paused")]
let list = TransferListResult(
    items: items,
    global: GlobalStat(downloadSpeed: 0, numActive: 1, numWaiting: 2, numStopped: 3))
print("B transfers \(SidebarBadge.unfinishedTransfers(list))"
    + " \(SidebarBadge.unfinishedTransfers(nil))"
    + " \(SidebarBadge.unfinishedTransfers(TransferListResult(items: [], global: GlobalStat(downloadSpeed: 0, numActive: 0, numWaiting: 0, numStopped: 0))))")
print("B unpassed \(SidebarBadge.unpassed(status(ok: ["a"], unverifiable: ["u", "v"])))"
    + " \(SidebarBadge.unpassed(status(bad: ["b"], unreadable: ["u"])))"
    + " \(SidebarBadge.unpassed(status()))"
    + " \(SidebarBadge.unpassed(nil))")

// -------- 总进度 --------
let pcases: [(String, TreeResult?, String?, String?)] = [
    ("in-batch", tree(), "C1", "C1"),
    ("other-batch", tree(), "A", "B"),
    ("no-tree-code", tree(), nil, "B"),
    ("no-code", tree(), "B", nil),
    ("no-tree", nil, "B", "B"),
    ("all-nil", nil, nil, nil),
    ("zero-total", tree(total: 0, done: 0, speed: 0, percent: 0), "C", "C"),
    ("over-report", tree(total: 100, done: 250), "C", "C"),
    ("negative-done", tree(total: 100, done: -5), "C", "C"),
    ("huge", tree(total: Int64.max, done: Int64.max / 2, speed: Int64.max), "C", "C"),
    ("verbatim-code", tree(), "C24-8_×_25WS024", "C24-8_×_25WS024"),
    ("empty-code", tree(), "", ""),
]
for (label, t, treeCode, code) in pcases {
    if let p = ProgressSummary.of(tree: t, treeCode: treeCode, code: code) {
        print("P \(label) \(flat(p.percentText)) | \(flat(p.bytesText)) | \(flat(p.speedText))"
            + " | \(String(format: "%.9f", p.fraction))")
    } else {
        print("P \(label) nil")
    }
}

// -------- 刷新失败：内核原文逐字（只有 `.rpc` 那一路可比）--------
let failures: [(String, String)] = [
    ("invalid_params", "内核原文"),
    ("no_delivery", ""),
    ("delivery_fetch_failed", "第一行\n第二行"),
    ("path_not_found", "[invalid_params] 这句正文里本来就有码字样"),
]
for (i, (code, message)) in failures.enumerated() {
    print("F \(i) \(flat(VerifyRefreshFailure.message(of: CoreError.rpc(code: code, message: message))))")
}
SWIFT

  cat > "$w/driver.rs" <<'RUST'
// Rust 侧驱动。`#[path]` 直接把工作树的源码按模块加载（**一个字节都没改**）。
#[path = "src/protocol.rs"]
mod protocol;
#[path = "src/client.rs"]
mod client;
// ⚠️ `platform` 必须在**根**上声明：`presentation/transfer_row.rs` 用的是 `crate::platform`
//    （裁定 JJ 把平台判据搬进这个模块之后），而驱动文件就是这个 crate 的根。
#[path = "src/platform.rs"]
mod platform;
#[path = "src/presentation/mod.rs"]
mod presentation;

use client::ClientError;
use presentation::verify_summary::{ProgressSummary, SidebarBadge, VerifyRefreshFailure, VerifySummary};
use presentation::RowColor;
use protocol::{GlobalStat, TaskState, TransferItem, TransferListResult, TreeResult, VerifyStatus};

/// 与 Swift 侧 `flat` 同形（换行会破坏一行一个结论的 diff）。
fn flat(s: &str) -> String {
    s.replace('\n', "\\n").replace('\r', "\\r")
}
fn show(v: Option<&str>) -> String {
    v.map(flat).unwrap_or_else(|| "-".to_string())
}
fn color_name(c: RowColor) -> &'static str {
    match c {
        RowColor::Secondary => "secondary",
        RowColor::Blue => "blue",
        RowColor::Green => "green",
        RowColor::Red => "red",
        RowColor::Orange => "orange",
    }
}

/// 一次 `verify_status` 的结果，**经解码构造**（键是内核线上的 snake_case）。
struct StatusFx {
    ok: Vec<String>,
    bad: Vec<String>,
    missing: Vec<String>,
    size_mismatch: Vec<String>,
    unverifiable: Vec<String>,
    unreadable: Vec<String>,
    all_good: Option<bool>,
}
impl Default for StatusFx {
    fn default() -> Self {
        Self {
            ok: Vec::new(),
            bad: Vec::new(),
            missing: Vec::new(),
            size_mismatch: Vec::new(),
            unverifiable: Vec::new(),
            unreadable: Vec::new(),
            all_good: None,
        }
    }
}
impl StatusFx {
    fn build(&self) -> VerifyStatus {
        let all_good = self.all_good.unwrap_or(
            self.bad.is_empty()
                && self.missing.is_empty()
                && self.size_mismatch.is_empty()
                && self.unreadable.is_empty(),
        );
        let obj = serde_json::json!({
            "ok": self.ok, "bad": self.bad, "missing": self.missing,
            "size_mismatch": self.size_mismatch,
            "unverifiable": self.unverifiable, "unreadable": self.unreadable,
            "all_good": all_good,
        });
        serde_json::from_value(obj).expect("夹具必须能解")
    }
}
fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}
fn status(ok: &[&str], bad: &[&str], missing: &[&str], size_mismatch: &[&str],
          unverifiable: &[&str], unreadable: &[&str], all_good: Option<bool>) -> VerifyStatus {
    StatusFx {
        ok: v(ok), bad: v(bad), missing: v(missing), size_mismatch: v(size_mismatch),
        unverifiable: v(unverifiable), unreadable: v(unreadable), all_good,
    }
    .build()
}

/// 一次 `get_tree` 的结果，**经解码构造**（本 case 只用到 `progress`）。
fn tree(total: i64, done: i64, speed: i64, percent: i32) -> TreeResult {
    let obj = serde_json::json!({
        "tree": {}, "flat": [], "default_selected": [],
        "progress": {"total_bytes": total, "done_bytes": done, "speed": speed, "percent": percent},
    });
    serde_json::from_value(obj).expect("夹具必须能解")
}

/// `transfer_list` 的一项，**经解码构造**。
fn item(gid: &str, state: TaskState, raw_status: &str) -> TransferItem {
    let obj = serde_json::json!({
        "gid": gid, "total": 1000, "completed": 250, "speed": 0, "conns": 1,
        "state": state, "raw_status": raw_status, "error_message": "",
        "path": format!("{gid}.bin"),
    });
    serde_json::from_value(obj).expect("夹具必须能解")
}
fn list(items: Vec<TransferItem>) -> TransferListResult {
    TransferListResult {
        items,
        global: GlobalStat { download_speed: 0, num_active: 1, num_waiting: 2, num_stopped: 3 },
    }
}

/// 一份结果的全部呈现值：总结那一行 + 六行（**恒六行，含计数为 0 的**）。
fn dump(label: &str, s: Option<&VerifyStatus>) {
    let Some(sum) = VerifySummary::of(s) else {
        println!("S {label} nil");
        return;
    };
    println!(
        "S {} {} {} {} | {} | {} | {} | {}",
        label,
        if sum.all_good { 1 } else { 0 },
        sum.failed_count,
        sum.classified_count,
        flat(&sum.headline()),
        color_name(sum.headline_color()),
        sum.headline_icon(),
        flat(&sum.classified_text()),
    );
    for r in &sum.rows {
        let paths: Vec<&str> = r.paths.iter().map(String::as_str).collect();
        println!(
            "R {} {} {} {} {} {} {} {} {} | {}",
            label, r.id(), flat(&r.label), color_name(r.color), r.icon_name,
            r.count(), flat(&r.count_text()), if r.is_failure() { 1 } else { 0 },
            show(r.note.as_deref()), flat(&paths.join("\u{1}")),
        );
    }
}

fn main() {
    // -------- 六类 / 总结 --------
    let weird = "client-test/C24-8_×_25WS024/reads 1.fq.gz";
    let cases: Vec<(&str, VerifyStatus)> = vec![
        ("empty", status(&[], &[], &[], &[], &[], &[], None)),
        ("only-ok", status(&["a"], &[], &[], &[], &[], &[], None)),
        ("one-bad", status(&[], &["b"], &[], &[], &[], &[], None)),
        ("all-six", status(&["p1"], &["p2"], &["p3"], &["p4"], &["p5"], &["p6"], None)),
        ("unverifiable-only", status(&["a"], &[], &[], &[], &["u", "v"], &[], None)),
        ("verbatim", status(&[weird], &["a/./b.txt", "a//b", "dir/", ""], &[], &[], &[], &[], None)),
        ("kernel-says-clear-but-failed", status(&[], &["b"], &[], &[], &[], &[], Some(true))),
        ("kernel-contradicts", status(&["a"], &[], &[], &[], &[], &[], Some(false))),
        ("unreadable-only", status(&[], &[], &[], &[], &[], &["u"], None)),
        ("size-and-missing", status(&[], &[], &["m"], &["s"], &[], &[], None)),
    ];
    for (label, s) in &cases {
        dump(label, Some(s));
    }
    dump("nil", None);

    // -------- 分区徽标 --------
    let items = vec![
        item("w", TaskState::Waiting, "waiting"),
        item("a", TaskState::Active, "active"),
        item("e", TaskState::Error, "error"),
        item("c", TaskState::Complete, "complete"),
        item("r", TaskState::Removed, "removed"),
        item("p", TaskState::Waiting, "paused"),
    ];
    println!(
        "B transfers {} {} {}",
        SidebarBadge::unfinished_transfers(Some(&list(items))),
        SidebarBadge::unfinished_transfers(None),
        SidebarBadge::unfinished_transfers(Some(&TransferListResult {
            items: Vec::new(),
            global: GlobalStat { download_speed: 0, num_active: 0, num_waiting: 0, num_stopped: 0 },
        })),
    );
    println!(
        "B unpassed {} {} {} {}",
        SidebarBadge::unpassed(Some(&status(&["a"], &[], &[], &[], &["u", "v"], &[], None))),
        SidebarBadge::unpassed(Some(&status(&[], &["b"], &[], &[], &[], &["u"], None))),
        SidebarBadge::unpassed(Some(&status(&[], &[], &[], &[], &[], &[], None))),
        SidebarBadge::unpassed(None),
    );

    // -------- 总进度 --------
    let pcases: Vec<(&str, Option<TreeResult>, Option<&str>, Option<&str>)> = vec![
        ("in-batch", Some(tree(4096, 1024, 512, 25)), Some("C1"), Some("C1")),
        ("other-batch", Some(tree(4096, 1024, 512, 25)), Some("A"), Some("B")),
        ("no-tree-code", Some(tree(4096, 1024, 512, 25)), None, Some("B")),
        ("no-code", Some(tree(4096, 1024, 512, 25)), Some("B"), None),
        ("no-tree", None, Some("B"), Some("B")),
        ("all-nil", None, None, None),
        ("zero-total", Some(tree(0, 0, 0, 0)), Some("C"), Some("C")),
        ("over-report", Some(tree(100, 250, 512, 25)), Some("C"), Some("C")),
        ("negative-done", Some(tree(100, -5, 512, 25)), Some("C"), Some("C")),
        ("huge", Some(tree(i64::MAX, i64::MAX / 2, i64::MAX, 25)), Some("C"), Some("C")),
        ("verbatim-code", Some(tree(4096, 1024, 512, 25)), Some("C24-8_×_25WS024"), Some("C24-8_×_25WS024")),
        ("empty-code", Some(tree(4096, 1024, 512, 25)), Some(""), Some("")),
    ];
    for (label, t, tree_code, code) in &pcases {
        match ProgressSummary::of(t.as_ref(), *tree_code, *code) {
            Some(p) => println!(
                "P {} {} | {} | {} | {:.9}",
                label, flat(&p.percent_text), flat(&p.bytes_text), flat(&p.speed_text), p.fraction
            ),
            None => println!("P {label} nil"),
        }
    }

    // -------- 刷新失败：内核原文逐字（只有 `.rpc` 那一路可比）--------
    let failures: [(&str, &str); 4] = [
        ("invalid_params", "内核原文"),
        ("no_delivery", ""),
        ("delivery_fetch_failed", "第一行\n第二行"),
        ("path_not_found", "[invalid_params] 这句正文里本来就有码字样"),
    ];
    for (i, (code, message)) in failures.iter().enumerate() {
        let e = ClientError::Kernel { code: (*code).to_string(), message: (*message).to_string() };
        println!("F {i} {}", flat(&VerifyRefreshFailure::message_of(&e)));
    }
}
RUST

  swiftc "$MACOS_SRC/Protocol.swift" "$MACOS_SRC/JSONValue.swift" \
         "$MACOS_SRC/Presentation/Format.swift" \
         "$MACOS_SRC/Presentation/EngineStatusPresentation.swift" \
         "$MACOS_SRC/Presentation/TransferRow.swift" \
         "$MACOS_SRC/Presentation/VerifySummary.swift" \
         "$w/color_extract.swift" "$w/appmodel_extract.swift" \
         "$w/main.swift" -o "$w/vs_swift"
  build_rust_driver "$w/driver.rs" "$w/vs_rust"

  "$w/vs_swift" > "$w/swift.out"
  "$w/vs_rust" > "$w/rust.out"
  diff_outputs "verify_summary" "$w/swift.out" "$w/rust.out"
}

# ---------------------------------------------------------------------------
# 比对：**空 diff 才算过**（不空就退出码 1，并把人能读的差异打出来）
# ---------------------------------------------------------------------------

diff_outputs() { # $1 = case 名，$2 = swift 输出，$3 = rust 输出
  local name="$1" a="$2" b="$3" lines lines_b
  lines="$(wc -l < "$a" | tr -d ' ')"
  lines_b="$(wc -l < "$b" | tr -d ' ')"

  # ---- 空跑判为失败（同 `test.sh` 第 3 步的纪律；控制者裁定 JJ 的次要 #7 补上）------
  #
  # 为什么必须有：**两个驱动都一行不打印时，`diff` 得到的是空 diff ⇒ 判为通过**。
  # 那正是本项目最怕的假绿 —— 驱动里一个 `exit(0)`、一段被注释掉的 `print`、
  # 或者两侧的 `main` 同时提前返回，都会让这个 case 变成"看起来绿了、其实什么都没比"。
  # `test.sh` 已经用"所有 `running N tests` 行之和为 0 就 exit 1"堵同一件事，
  # 这里按同一条口径补：**一行都没有 ⇒ 这个 case 不算过**。
  #
  # ⚠️ 判据是"任一驱动为空"而不是"两个都为空"：只空一个的时候 `diff` 本来就会红，
  #    但那时报出来的是"两侧有差异"，读者会去找差异 —— 而根因是**其中一侧根本没跑**。
  #    分开判能把根因说准（本项目对"根因不许说错"有纪律）。
  if [[ "$lines" -eq 0 || "$lines_b" -eq 0 ]]; then
    echo "PORTCHECK FAILED: $name —— 驱动输出为空（swift=$lines 行，rust=$lines_b 行）。" >&2
    echo "           一个什么都没打印的驱动与没有驱动等价，空 diff 不算证据。" >&2
    echo "           补救：核对 case_$name 里两份驱动的 main/顶层代码是否真的在打印。" >&2
    echo "工作目录：${WORK}（--keep 可保留下来复看）" >&2
    exit 1
  fi

  if diff -u "$a" "$b" > "$WORK/$name.diff"; then
    echo "PORTCHECK OK: $name —— $lines 行，两侧逐字相同"
  else
    echo "PORTCHECK FAILED: $name —— 两侧输出有差异（- 是 Swift，+ 是 Rust）：" >&2
    cat "$WORK/$name.diff" >&2
    echo "工作目录：${WORK}（--keep 可保留下来复看）" >&2
    exit 1
  fi
  if [[ $KEEP -eq 1 ]]; then echo "  工作目录：$WORK"; fi
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

for arg in "$@"; do
  case "$arg" in
    --keep) KEEP=1 ;;
    --list) printf '%s\n' "${CASES[@]}"; exit 0 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "portcheck: 不认识的参数 $arg" >&2; usage >&2; exit 2 ;;
    *) ONLY+=("$arg") ;;
  esac
done

require_env
new_work
if have cargo && [[ ! -d "$WIN_ROOT/target/debug/deps" ]]; then
  (cd "$WIN_ROOT" && cargo build -p shell-core >/dev/null)
fi

if [[ ${#ONLY[@]} -gt 0 ]]; then
  for want in "${ONLY[@]}"; do
    found=0
    for c in "${CASES[@]}"; do [[ "$c" == "$want" ]] && found=1; done
    [[ $found -eq 1 ]] || { echo "portcheck: 没有这个 case：${want}（--list 看全部）" >&2; exit 2; }
    "case_$want"
  done
  # ⚠️ **指定 case 时不许说"全部通过"**：那会让"跑了 1 个、另外 2 个没跑"读起来
  #    像"3 个都过了"——本项目最恨的就是这种不会变红的谎报。两种情形分开说。
  echo "PORTCHECK: 指定的 case 全部通过（${#ONLY[@]} 个；本次**未跑**其余 $(( ${#CASES[@]} - ${#ONLY[@]} )) 个）"
else
  for c in "${CASES[@]}"; do "case_$c"; done
  echo "PORTCHECK: 全部通过（${#CASES[@]} 个 case）"
fi
