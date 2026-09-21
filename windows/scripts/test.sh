#!/usr/bin/env bash
#
# windows/scripts/test.sh —— Windows 壳的**唯一**测试入口。
#
# ⚠️ 为什么不直接 `cargo test`：
#   e2e 用例要拉起**真实的**内核二进制，裸 `cargo test` 会因为它不存在/是旧的
#   而给出假绿或假红。macOS 侧栽过一次（内核改了、端到端零覆盖、套件照样全绿），
#   `macos/scripts/test.sh` 的头部注释记着那次事故（本阶段的账本 Ruling C19）——
#   本脚本照同一条纪律写。
#
# ⚠️ **已知基线：本脚本每次运行都会显示 `core/` 的 4 条 `dead_code` 告警——那不是回归。**
#
#   跑第 1 步（重建内核）时 cargo 会稳定打出这四条（点名列出，免得靠记性对）：
#
#       · `crc64xz::sum`
#       · `ARIA2_LICENSE`
#       · `license_text`
#       · `Daemon{secret, argv}`
#
#   它们是**本阶段开跑之前就存在**的（阶段 C 账本 Ruling C8 记的账）。执行口径是阶段 D 的
#   **D-4**：**本任务不新增**，并且**不许**用 `#[allow(dead_code)]` 或删符号去按掉它们
#   （按掉等于把"基线"这件事从账本上抹掉，下一次就没人分得清哪条是新的了）。
#   ⇒ 本脚本**不判**这四条，也**不替它们消音**：它们照常打印，读者按这份说明减掉。
#
#   反过来说：**任何新增的告警都是回归**。看输出时把上面四条减掉再数——对不上，
#   多出来的那些就是这次改动带进来的，要修的是那一条。
#   ⚠️ 这段说明必须留在这里：后续每一个审查都会看到这四行，不写清，要么被误读成新回归，
#      要么让人学会了对真告警脱敏（本项目最怕的形态）。
#
# 本脚本做的每一步（0.x = 便宜的前端/清单守卫；1–3 = 内核与 cargo）：
#
#   0.9) **前端四套整屏验收**（任务 10b 接进来的，R-63）：常驻 stub + 文件页 +
#        传输列表屏 + 任务 13 那一套，全都是"一条命令一份判据"的 runner，由本脚本
#        **串行**跑完。在此之前它们**一个都没接进来** ⇒ 跑不跑全看人记不记得：
#        实测代价是 `check_frontend_stub.sh` 里的 I-3b 自任务 10 起就是红的，
#        **红了四个任务而没有任何人会看见**（跑它的入口不存在）。
#        ⚠️ 它要一个**无头浏览器**，而本仓库**不做条件跳过**（W-2）：找不到就在
#        这一步大声失败，并给出 macOS / Debian-Ubuntu / Windows 三行补救。
#        ⚠️ 因此 `test.sh` 的硬前置从"cargo + python3"变成了"cargo + python3 + 一个
#        Chromium 内核的浏览器" —— 两个声明本脚本前置工具的文档已同步
#        （`docs/superpowers/2026-09-19-windows-dev-setup.md`、`windows/README.md`）。
#
#   0.4) **壳与内核的路径同源判据**（R-4）：同一套"目录名 / 环境变量名"的知识散在
#        四处（`core/src/paths.rs`、`shell-core/src/storage/mod.rs`、
#        `shell-core/src/embedded_core.rs`、`shell-win/src/{embed,wv2}.rs`），
#        而壳**在编译期看不见内核**（`shell-core` 不许依赖 `core`）⇒ 两边漂移时
#        **没有任何东西会红**，后果是壳的 `preferences.json` / `history.json` 与内核的
#        `settings.json` / aria2c 缓存落到**两个不同的目录**：用户的设置与历史
#        **凭空消失**，而壳与内核各自自洽、各自的用例全绿。
#        判据本体在 `check_shared_paths.sh`（把四处的字面量抽出来逐组比对）。
#
#   0.45) **对位判据**（R-5 / R-1）：`resident_notice` 的 48/80 有**两份真相源**
#        （Rust 常量 + `tokens.css` 的令牌），而 `presentation/` 里那几条**零生产调用者**的判据
#        （`ManifestTracking` / `NoteDrafts` / `SettingsForm::matches` / `dirty_banner`）
#        的行为活在前端 —— 它们是本代最严重那个缺陷的**形状**（判据在 Rust、实现抄进 JS、
#        分叉时谁都不会红）。判据本体在 `check_presentation_mirrors.sh`
#        （两个数一致 + 令牌真的被用上 + 对位账的三处坐标在场且账没过期）。
#        ⚠️ 那几条判据**行为上**的对照在**第 0.9 步**（`panels-harness.html` 回放
#        Rust 倒出来的用例表）—— 两步各守一半，别把这一步读成全部。
#
#   0.5) **字体子集的覆盖判据**（任务 21）：内嵌的界面字体子集必须仍覆盖源码里的字
#        （W-5 的兜底那一半）。缺字的后果是界面上一块**豆腐块**，而编译与测试**全都过得去**
#        —— 没有这一步，没有任何东西会因此变红。判据本体在 `make_font_subset.sh --check`。
#
#   0.7) **§3.2 的自动守卫**（任务 9b）：`windows/web/` 下的每个文件、以及它里面
#        每一个"非 ASCII 指纹"，都必须逐条登记；未登记的 ⇒ 红（登记只有两类合法：
#        **结构文案**与**引导期诊断**，理由与白名单都在那份脚本里）。
#        ⚠️ 白名单每条**绑处数**（"这个文件里恰好出现这么多处"）：同一句话**第二次**
#        出现也是红 —— 否则 `重试`/`设置`/`加载` 这类"单独看很正常"的界面词
#        可以无限复制而不留痕迹（那是第 1 轮修复轮堵掉的洞）。
#        在此之前 **往 `js/` 里写一句硬编码中文不会让任何东西变红**：
#        第 0.5/0.6 步判的是"那个字在不在内嵌子集里"，它们判不出
#        "这个字**该不该由前端写**"——只要那个字在子集里，两条判据照样全绿，
#        而 `presentation/` 与 macOS 的文案从那一刻起就各走各的了。
#        判据本体（含白名单、文件清单与每条理由）在 `windows/scripts/check_frontend_copy.sh`。
#
#   0) **依赖守卫**：`shell-core` 的直接依赖只许是白名单里的那些（见下面的裁决 M 段）。
#      `shell-core` 必须保持纯逻辑、跨平台——"壳能在 macOS 上 cargo test"这条地基全靠它，
#      而在此之前**没有任何东西**会在有人加错依赖时变红。
#
#   1) 先编宿主的内核（`cargo build --release`）。shell-core 的 e2e 拉起的是
#      `core/target/<profile>/benagen-core[.exe]` 这个**预先构建的二进制**；不先重建它，
#      "内核改了、壳的端到端一条都没跑到"可以完全无声地发生。已有产物时是秒级 no-op。
#
#   2) 跑壳的测试（`cargo test -p shell-core -p shell-win`）。
#      ⚠️ **为什么连 `shell-win` 一起跑**（任务 9 加的，一条**显式记账的偏离**）：
#         计划里这一步写的是"跑 shell-core 测试"，理由是 shell-win 是 egui 视图、"不单测"。
#         但规格 §11.3 要求"`CREATE_NO_WINDOW` 确实被传"这条**能在本机断言**，而它只能
#         写在 shell-win 里（那是唯一允许出现平台 API 的 crate，见 `shell-win/src/spawn.rs`）。
#         一个写了却**没有任何入口会去跑**的测试，与没有测试等价——本脚本第 3 步
#         （空跑绿灯判为失败）防的正是这个形态。所以 shell-win 也进本入口。
#         代价如实记账：宿主上会编一遍 shell-win 的依赖树。
#         ⚠️ 这一行**改过一次**（任务 10，2026-09-19）：原文写的是"会编一遍
#            eframe/egui（首次约二十秒，之后走缓存）" —— 阶段 A 把 `eframe` 连同
#            `views/`、`fonts.rs` 一起删了（规格 §9.1、§4.2），那句话**已经不成立**。
#            现在编的是 `tiny_http` + `getrandom` + `serde_json` + `shell-core` ——
#            整棵依赖树（`windows/Cargo.lock` 里 `[[package]]` 的条数）**实测**从 413
#            掉到 34 —— 那 379 个就是 `egui`/`winit`/`wgpu`/`accesskit` 那一整棵。
#         这是**看得见**的代价，比留一条永不执行的断言便宜。
#
#         ⚠️ 任务 11 起，这一步还会跑到 `shell-win/tests/e2e_service.rs`（规格 §11.3
#            那条"整条链路可以在 macOS 上验"）：真 HTTP + **假内核**，所以它不需要
#            Windows、不需要 wine、也不需要真的下载。cargo 默认就会跑 `tests/`，
#            **本步没有为此加任何参数** —— 这一点由输出里那行
#            `Running tests/e2e_service.rs` 作证。
#
#   3) **空跑绿灯判为失败**。这是要点：`cargo test` 在过滤器没匹配到任何用例时
#      打印 `running 0 tests` 并**以退出码 0 结束**。过滤器打错一个字，整轮就变成
#      一次静默的空跑绿灯——TDD 里"运行它、预期 FAIL"这一步会假通过。
#      swift-testing 那边踩过同形的坑（见 macos/scripts/test.sh 的头部注释），
#      本脚本把**所有 `running N tests` 行的总和**算出来，为 0 就 `exit 1`
#      并打印脚本自己的一条说明。
#      ⚠️ 判据必须是"总和为 0"而不是"某一行为 0"：cargo 每跑一个测试目标就打印一行，
#         上面那条单测跑完会同时出现 `running 1 test`（lib 的 unittest）与
#         `running 0 tests`（doc-test）——按"任一行"判会把正常绿灯判成失败。
#
# 参数：**只有过滤器**。
#   位置形式 `bash windows/scripts/test.sh Foo` 与
#   `--filter Foo` / `--filter=Foo` 都接受（`--filter` 不是 cargo 的参数，
#   这里翻译成 cargo 的位置形式）。
#   ⚠️ 不认识的参数**直接报错退出**（exit 2），**绝不透传给 cargo**。
#      理由：透传一个打错的 flag 会让 cargo 以 "unexpected argument" 报错退出，
#      而"必须非零退出"这种验收恰好也能被它满足——那就成了一个因错而过的绿灯，
#      与没有验收等价（本项目最怕的假绿）。确实需要别的 cargo 参数时，
#      请显式加进下面的白名单，而不是放开透传。
set -euo pipefail

# 仓库根 = 本脚本所在目录（`windows/scripts/`）上溯两层。
# ⚠️ 用 `${BASH_SOURCE[0]}`（脚本自身的路径，与被调用的工作目录无关）而不是 `$0`：
#    相对路径的 `$0` 在下面的 `cd` 之后会解析到错的地方。
#    （与 `macos/scripts/test.sh` 的 repo_root 同源；任务 2 的下载脚本沿用本写法。）
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# ---- 参数解析：只认过滤器，未知 flag 一律拒收 -------------------------------
# 数组的展开写成 ${filters[@]+"${filters[@]}"}：macOS 自带的 bash 3.2 在 `set -u`
# 下直接展开空数组会报 unbound variable。
filters=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --filter)
      if [ "$#" -lt 2 ]; then
        echo "test.sh: --filter 后面要跟一个过滤器" >&2
        exit 2
      fi
      filters+=("$2")
      shift 2
      ;;
    --filter=*)
      filters+=("${1#--filter=}")
      shift
      ;;
    -*)
      echo "test.sh: 不认识的参数：$1" >&2
      echo "test.sh: 本脚本只接受过滤器（位置形式 Foo，或 --filter Foo / --filter=Foo）。" >&2
      echo "test.sh: 不把未知参数透传给 cargo：那样只会拿到 cargo 的用法错误，" >&2
      echo "test.sh: 一个因错而过的非零退出和没有验收等价。要用别的 cargo 参数请加白名单。" >&2
      exit 2
      ;;
    *)
      filters+=("$1")
      shift
      ;;
  esac
done

# ---- 0) 依赖守卫：shell-core 只许有白名单里的直接依赖（控制者裁决 M）---------
#
# 为什么需要它：`shell-core` 的定位是**跨平台纯逻辑**（规格 §4.3）——协议镜像、
# `Presentation/` 的移植、本地存储，**不含 OS 调用**。整条"壳的纯逻辑能在 macOS 上
# `cargo test`"的地基就是它。但**在此之前没有任何东西会在有人加错依赖时变红**：
# 往 `shell-core/Cargo.toml` 里塞一个 `eframe`/`egui`/`winapi`，现有测试照样全绿，
# 而"能在 macOS 上测"这句话已经悄悄不成立了。一个不响的守卫与没有守卫，
# 在客户机器上的后果完全一样（W-2：不许静默降级）。
#
# 判据是**直接依赖**：`cargo metadata --no-deps` 只读工作区成员自己的清单，
# 不解析整张依赖图（也就不需要网络、不受下游 crate 影响）。
# ⚠️ **三种 kind 全查**（普通 / dev / build / 以及带 `target` 的平台专属依赖）：
#    这条约束的理由是"跨平台可测"，它对三者同样成立——`[target.'cfg(windows)'.dependencies]`
#    里的 `winapi` 正是最该被逮住的那一类。
# ⚠️ 允许清单是**故意写死**在脚本里的。要合法地加依赖，就**显式**改这一行：
#    那是一次能被审查的决定，而不是一次静默的漂移。别把它做成环境变量——
#    环境变量会让"守卫没过"变成"设个变量就能过"。
guard_allow="serde serde_json"

if ! command -v python3 >/dev/null 2>&1; then
  # W-2：不许静默降级。缺工具就大声说，**绝不**跳过守卫继续跑。
  echo "test.sh: 错误：找不到 python3，无法执行 shell-core 的依赖守卫。" >&2
  echo "test.sh: 这道守卫检查的是 shell-core 的直接依赖白名单（${guard_allow}）；" >&2
  echo "test.sh: 跳过它就等于没有这道网，所以这里直接失败而不是继续。" >&2
  echo "test.sh: 补救：装 python3，或把本段换成等价的非 python 实现（不引入构建依赖）。" >&2
  exit 1
fi

# ⚠️ **cargo 的 stderr 与 stdout 必须分开接**（复审抓的，2026-09-18）。
#
# 从前这里写的是 `cargo metadata … 2>&1`，把两者一起喂给 `json.load`。于是
# cargo 往 stderr 打**任何一行**——warning、进度提示、`Blocking waiting for file lock`
# ——都会让 `json.load` 抛，守卫随即走 `guard_status != 0` 分支报
# **"依赖守卫自己失败了"**。方向是响亮的（exit 1，不静默放行），但**根因说错了**：
# 读者被引向"输出格式不匹配"，而不是那行 warning。本项目对"根因不许说错"有纪律
# ——守卫失败时给出的理由必须是它**真正检查到的东西**。
#
# 分开之后：**stdout 只走 JSON 解析**（那才是 `cargo metadata` 的契约输出），
# stderr 原样留着，只在真失败时打出来给读者看。
#
# ⚠️ 全角字符紧跟变量名的写法（`$var（`）在 macOS 自带的 bash 3.2 下会被解析成
#    `var<0xEF>…` ⇒ `set -u` 报 unbound variable 并**以错误的理由退出**
#    （本段的 `${meta_status}` / `${guard_status}` 都补了花括号，理由同
#    `kernel_bin` 那一处的长注释；同文件里另外两处也一并补了）。
guard_tmp="$(mktemp)"
# 这条 trap 只覆盖"还没走到下面那条 trap 就退出"的窗口（守卫失败会 exit）；
# 走到最后那条 trap 时它会**接管**这两个文件（`trap` 是覆盖语义，见那里的注释）。
trap 'rm -f "$guard_tmp"' EXIT

set +e
meta_out="$(cd "$REPO/windows" && cargo metadata --format-version 1 --no-deps 2>"$guard_tmp")"
meta_status=$?
set -e
# 读出来就清空：这个文件后面还要装守卫 python 自己的 stderr
meta_err="$(cat "$guard_tmp")"
: > "$guard_tmp"

if [ "$meta_status" -ne 0 ]; then
  echo "test.sh: 错误：cargo metadata 失败（退出码 ${meta_status}），依赖守卫无法判定。" >&2
  echo "test.sh: 它读的是 $REPO/windows 这个工作区；失败通常意味着工作区的 Cargo.toml 坏了。" >&2
  if [ -n "$meta_err" ]; then
    echo "test.sh: cargo metadata 的 stderr（原样）：" >&2
    printf '%s\n' "$meta_err" >&2
  fi
  if [ -n "$meta_out" ]; then
    echo "test.sh: cargo metadata 的 stdout（原样；失败时通常只有半截 JSON 或空）：" >&2
    printf '%s\n' "$meta_out" >&2
  fi
  echo "test.sh: 补救：先修好工作区清单（cargo metadata 能跑通），再谈测试。" >&2
  exit 1
fi

set +e
bad_deps="$(printf '%s' "$meta_out" | GUARD_ALLOW="$guard_allow" python3 -c '
import json, os, sys

allow = set(os.environ["GUARD_ALLOW"].split())
md = json.load(sys.stdin)
pkg = next((p for p in md["packages"] if p["name"] == "shell-core"), None)
if pkg is None:
    print("__SHELL_CORE_MISSING__")
    sys.exit(0)
for d in pkg["dependencies"]:
    if d["name"] in allow:
        continue
    kind = d.get("kind") or "normal"
    target = d.get("target") or "所有平台"
    rename = "（重命名为 %s）" % d["rename"] if d.get("rename") else ""
    print("%s%s [%s，%s]" % (d["name"], rename, kind, target))
' 2>"$guard_tmp")"
guard_status=$?
set -e
guard_err="$(cat "$guard_tmp")"

if [ "$guard_status" -ne 0 ]; then
  # ⚠️ 这一段**不再**把根因说成"cargo 的输出格式不匹配"：cargo 的 stderr 根本不进
  #    `json.load`（见上面那段注释），所以这里真正可能的原因只剩两个——python3 起不来，
  #    或者 `cargo metadata` 的 **stdout** 不是 JSON。下面把 python 自己的 stderr
  #    **原样**打出来：那是唯一能直接读出根因的东西（`JSONDecodeError` / `KeyError` /
  #    `FileNotFoundError` 各指一件事），不要用我们的猜测盖过它。
  echo "test.sh: 错误：依赖守卫的解析步骤失败了（退出码 ${guard_status}），无法判定 shell-core 的直接依赖。" >&2
  echo "test.sh: 它只读 cargo metadata 的 **stdout**（JSON）；cargo 的 stderr 不会混进来。" >&2
  if [ -n "$guard_err" ]; then
    echo "test.sh: 守卫 python 自己的 stderr（原样，**根因就在这里**）：" >&2
    printf '%s\n' "$guard_err" >&2
  else
    echo "test.sh: 守卫 python 连 stderr 都没打出来（那通常意味着它根本没起来：解释器不可用、或被信号杀掉）。" >&2
  fi
  if [ -n "$meta_err" ]; then
    echo "test.sh: 顺带一提，cargo metadata 这次在 stderr 上说了这些（原样）：" >&2
    printf '%s\n' "$meta_err" >&2
  fi
  echo "test.sh: 补救：手工跑一次 \`cd \"$REPO/windows\" && cargo metadata --format-version 1 --no-deps\`，" >&2
  echo "test.sh: 把它的 **stdout** 单独喂给上面那段 python，看它到底报哪一行。" >&2
  exit 1
fi

if [ "$bad_deps" = "__SHELL_CORE_MISSING__" ]; then
  echo "test.sh: 错误：工作区里找不到名为 shell-core 的成员包。" >&2
  echo "test.sh: 依赖守卫查的就是它；找不到就意味着 $REPO/windows/Cargo.toml 的" >&2
  echo "test.sh: members 被改名/移除了，而壳的测试目标也随之消失（假绿）。" >&2
  echo "test.sh: 补救：把 shell-core 加回 members，或把本脚本与守卫指向新的包名。" >&2
  exit 1
fi

if [ -n "$bad_deps" ]; then
  echo "test.sh: 错误：shell-core 出现了允许清单之外的直接依赖：" >&2
  printf '%s\n' "$bad_deps" | while IFS= read -r line; do
    echo "test.sh:   - $line" >&2
  done
  echo "test.sh: 允许清单：$guard_allow" >&2
  echo "test.sh: 约束的理由：shell-core 必须保持纯逻辑、跨平台（规格 §4.3）——" >&2
  echo "test.sh: 「壳的纯逻辑能在 macOS 上 cargo test」这条地基全靠它。" >&2
  echo "test.sh: 要加 UI / OS 依赖（eframe / egui / winapi …）请加到 shell-win；" >&2
  echo "test.sh: 若这个依赖确实纯逻辑且跨平台，就把它显式加进本段开头的 guard_allow，" >&2
  echo "test.sh: 让这次放宽成为一次能被审查的决定。" >&2
  exit 1
fi

echo "test.sh: 依赖守卫通过：shell-core 的直接依赖都在 [$guard_allow] 内" >&2

# ---- 0.4) 壳与内核的路径同源判据（R-4）---------------------------------------
#
# 为什么必须有这一步：同一套"目录名 / 环境变量名"的知识**散在四处**，而壳**在编译期
# 看不见内核**——`shell-core` 不许依赖 `core`（上面那条依赖白名单就是这条约束的落点），
# 所以"两边写的是同一个名字"这句话**没有任何编译器兜着**。
# 四处今天都一致，所以今天没有可观察的缺陷；缺陷是"漂移时不会有任何东西变红"：
# 壳的 `preferences.json` / `history.json` 与内核的 `settings.json` / aria2c 缓存
# 会落到**两个不同的目录**，用户的下载目录设置与批次历史**凭空消失**，
# 而壳与内核各自都自洽、各自的用例全绿。
#
# ⚠️ 这条被记了很久，记的却是**症状**（"某条用例判别力弱：常量 == 字面量"）而不是
#    病因（仓库级没有判据把两棵树绑在一起）。判别力弱的用例**还在**，它守的是
#    "壳自己那半写对了"；这一步守的是"两半写的是同一件事"——两件事，两条判据。
#
# ⚠️ 与 0.5/0.6/0.7/0.8 同一条口径：**在这里直接失败、不往下跑**。
echo "test.sh: 校验壳与内核的目录名/环境变量名同源（check_shared_paths.sh）" >&2
if ! bash "$REPO/windows/scripts/check_shared_paths.sh"; then
  echo "test.sh: 路径同源判据没过（原因见上面那个脚本的输出）——" >&2
  echo "test.sh: 退出码 3 = 判据没过（某一组出现了第二种值）；1/2 = 环境或用法问题。" >&2
  echo "test.sh: 补救：对着它的输出，把同一个名字在**每一处**都改成一致的那一个。" >&2
  exit 3
fi

# ---- 0.45) 对位判据（R-5 / R-1）---------------------------------------------
#
# 为什么必须有这一步：本代最严重那个缺陷的形状不是"某个函数写错了"，是
# **判据在 Rust 里（有单测、有文档）、实现被抄进 JS，而两边一分叉谁都不会红**。
# 这一步守两件事：
#   · `resident_notice` 的 48/80 有**两份真相源**（Rust 常量 + `tokens.css` 的令牌）
#     —— 那份模块头逐字写着"常量与判决都由 Rust 交出去"，而 CSS 里又写了一份；
#   · 那几条**零生产调用者**的判据（`ManifestTracking` / `NoteDrafts` /
#     `SettingsForm::matches` / `dirty_banner`）的**对位账**：三处坐标必须在场，
#     而且账上的"仍然零调用者"这句话今天还得成立（有人把它们接上了 ⇒ 账要跟着改）。
# 判据本体（含账本、每条的理由与效力边界）在 `check_presentation_mirrors.sh`。
#
# ⚠️ 那几条判据**行为上**对不对，靠的是另一条路：**同一组输入喂给两边、结论必须一致**
#    —— 用例表由 Rust 倒出来（`mirrorCases`），前端在第 0.9 步的
#    `panels-harness.html` 里回放。这一步只守"坐标与数值"那一半。
echo "test.sh: 校验对位判据（check_presentation_mirrors.sh）" >&2
if ! bash "$REPO/windows/scripts/check_presentation_mirrors.sh"; then
  echo "test.sh: 对位判据没过（原因见上面那个脚本的输出）——" >&2
  echo "test.sh: 退出码 3 = 判据没过（两个数不一致 / 令牌没人用 / 对位账的坐标过期）；" >&2
  echo "test.sh: 1/2 = 环境或用法问题。" >&2
  exit 3
fi

# ---- 0.5) 字体子集的覆盖判据（任务 21，W-5）-----------------------------------
#
# 为什么必须有这一步：`shell-win/src/fonts.rs` 把 `windows/assets/ui-subset.otf`
# **内嵌**进 exe（W-5 的"内嵌子集兜底"那一半）。若子集里少了界面文案用到的某个字，
# 界面上就是一块**豆腐块** —— 而编译、测试、构建**全都过得去**，没有任何东西会因此变红
# （除非有人正好盯着那一个字）。这一步就是那个"东西"。
#
# 判据由 `make_font_subset.sh --check` 给：它与**生成**共用同一份抽取代码（同一件事
# 不写两遍，分叉的那一份会成为第二条真相），并且**不联网、不需要 fontTools**
# —— 只要 python3（上面第 0 步已经把它当作硬前置，缺了会大声失败）。
#
# ⚠️ 这里**直接失败、不往下跑**：让 cargo 的绿灯盖过一个缺字的子集，正是最难发现的那种假绿。
echo "test.sh: 校验内嵌字体子集是否仍覆盖源码里的字（make_font_subset.sh --check）" >&2
if ! bash "$REPO/windows/scripts/make_font_subset.sh" --check; then
  echo "test.sh: 字体子集的覆盖判据没过（原因见上面那个脚本的输出）——" >&2
  echo "test.sh: 它的退出码 3 表示\"判据没过\"，与 1/2（环境/下载问题）不同。" >&2
  echo "test.sh: 补救：改完文案后重跑 \`bash windows/scripts/make_font_subset.sh\`（生成模式）。" >&2
  exit 3
fi

# ---- 0.6) 前端文件的字体覆盖判据（A0，W-5 的另一半）--------------------------
#
# 为什么与 0.5 分开：0.5 守的是**Rust 源码字面量**（`make_font_subset.sh --check`
# 的扫描面写死在三棵 `.rs` 树上），而阶段 A0 起界面文案开始出现在 **HTML/CSS/JS** 里。
# 少了这一步，"`web/` 或 `design/` 里有个字不在子集里"会以**豆腐块**的形式出现，
# 而编译、测试、构建**全都过得去** —— 正是 0.5 那段注释说的同一种静默失守。
#
# ⚠️ 下面的 glob **不匹配任何文件时必须失败**（`nullglob` + 显式判空），
#    不许静默跳过：一个"因为没找到文件所以通过"的判据，与没有判据无法区分。
# ⚠️⚠️ **这份清单的属主是 `make_font_subset.sh`，不是这里**（本计划第八版修订）：
#    两边各写一份常量的后果**实测过一次** —— 判据的 glob 只扫 `design/`，
#    而 `shell-win/src/web/index.html`（一个真会渲染的已交付页面）里有 5 个字
#    不在子集里 ⇒ **它会显示方块，而两条验收都是绿的**。
#    ⇒ **问它，不抄它**：目录清单从 `--frontend-dirs` 读（每行一个仓库相对路径）。
#    A-2 把界面搬进 `windows/web/` 时**只改 `make_font_subset.sh` 里那一行**，
#    这里自动跟着走（交接文件「缝 1」的前端那一半因此收口）。
#
# 🔴 **三种扩展名都要扫（`.html` / `.css` / `.js`）—— 任务 10b 修的第二个洞。**
#    这一段的注释从写下的第一天起就写着"界面文案开始出现在 **HTML/CSS/JS** 里"，
#    而 glob 只写了 `*.html` ⇒ **`.js` 与 `.css` 从来没有被扫过**（"守卫存在，但守不到"）。
#    后果已经在树上：`js/dom.js` 的「这一屏的模**板**里」与 `js/screens/registry.js` 的
#    「前端没有注**册**名为「X」的屏。」—— 两句都是**会渲染的失败提示**，而
#    「板」「册」都不在内嵌子集里 ⇒ **渲染成豆腐块，而所有验收全绿**。
#    同一个形状此前在 `design/` 那一版上发生过一次（上面那段记着）；这次是它换了个位置。
#    ⚠️ 修的时候**没有只修这一个洞**：生成侧的同一条 glob（`make_font_subset.sh` 的
#       `required_chars()`）也只扫 `*.html` —— 两处一起改了，否则"重跑生成模式"这条
#       补救对着 `.js` 里的字**永远不会生效**（补救必须可执行）。
#
# ⚠️⚠️🔴 **而且必须递归**（这是同一个洞的第三层，实测才发现）：
#    原来的 glob 是 `"$REPO/$d"/*.html` —— **深度 1**。界面文件的布局是
#    `windows/web/{index.html, app.js, app.css, css/**, js/**}`，而 CSS 在 `css/` 下、
#    JS 在 `js/` 与 `js/screens/` 下 ⇒ 就算把扩展名补齐成三样，**深度 1 的 glob
#    仍然一个 `js/` 文件都扫不到**（实测：改完扩展名后这一步只匹配到 4 个文件：
#    `.html 2 / .css 1 / .js 1`，`js/dom.js`、`js/screens/registry.js` 全在外面）。
#    ⇒ 这里改用 `find -type f`（递归），**不是** `globstar`：本脚本要能在 macOS 自带的
#      bash 3.2 上跑，而 `shopt -s globstar` 是 bash 4 才有的。
# ⚠️ **每一种扩展名都必须至少匹配到一个文件**（下面逐类判空）：只判"总数 > 0"的话，
#    将来 `js/` 被搬走/改名会**静默地**退出扫描面（`.html` 还在，总数照样 > 0）——
#    那正是这次要堵的那个形状，别再留一个同形的。
#
# ⚠️ `--frontend-dirs` **自己失败 ⇒ 必须红**，不许静默退化成空清单：
#    一个"因为清单没读到所以通过"的判据，与没有判据无法区分。
# ⚠️ 命令替换要**先接住它的退出码**再谈别的：`< <(cmd)` 那种写法**拿不到 `cmd` 的退出码**
#    （进程替换的失败不会传播到 `while` 的判断上）⇒ 脚本坏掉会被读成"清单是空的"。
frontend_list="$(bash "$REPO/windows/scripts/make_font_subset.sh" --frontend-dirs)" || {
  echo "test.sh: 错误：读不到前端文件的目录清单（make_font_subset.sh --frontend-dirs 失败）。" >&2
  echo "test.sh: 这不是\"通过\" —— 判据要扫哪些文件都不知道，就无从判起。" >&2
  exit 1
}
if [ -z "$frontend_list" ]; then
  echo "test.sh: 错误：前端文件的目录清单是空的 —— 拒绝返回绿。" >&2
  echo "test.sh: 一个目录都没给，判据就无对象可判；那不是通过。" >&2
  exit 1
fi
# ⚠️ **一条命令都别塞进管道里再谈退出码**：`set -o pipefail` 在本脚本开头就开着，所以
#    `find … | sort` 的失败会被 `||` 接住（`sort` 不会替 `find` 背这个错）。
design_files=()
n_html=0
n_css=0
n_js=0
while IFS= read -r d; do
  [ -n "$d" ] || continue
  found="$(find "$REPO/$d" -type f \
             \( -name '*.html' -o -name '*.css' -o -name '*.js' \) | LC_ALL=C sort)" || {
    echo "test.sh: 错误：扫描前端目录失败：$REPO/$d（find 非零退出）。" >&2
    echo "test.sh: 这不是\"通过\" —— 连有哪些文件都没数出来，就谈不上覆盖。" >&2
    echo "test.sh: 常见原因：清单里的目录不存在（make_font_subset.sh 的 FRONTEND_DIRS 与树对不上）。" >&2
    exit 1
  }
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    design_files+=("$f")
    case "$f" in
      *.html) n_html=$((n_html + 1)) ;;
      *.css)  n_css=$((n_css + 1)) ;;
      *.js)   n_js=$((n_js + 1)) ;;
    esac
  done <<< "$found"
done <<< "$frontend_list"
if [ "${#design_files[@]}" -eq 0 ]; then
  echo "test.sh: 错误：清单里的目录一个 .html/.css/.js 都没匹配到 —— 判据无对象可判。" >&2
  echo "test.sh: （清单来自 make_font_subset.sh --frontend-dirs：${frontend_list//$'\n'/ }）" >&2
  echo "test.sh: 这不是\"通过\"。要么样张/占位页丢了，要么路径改了；两种情况都要人来看。" >&2
  exit 1
fi
# ⚠️ **每一类各自都要有货**（不只看总数）：`js/` 或 `css/` 整类没匹配到时，扫描面会
#    **静默**少掉一大块，而总数（`.html` 还在）照样 > 0 ⇒ 判据显示"通过"。
#    那正是这次修的洞（glob 少了一个扩展名），别留一个同形的。
for pair in "html:${n_html}" "css:${n_css}" "js:${n_js}"; do
  ext="${pair%%:*}"
  count="${pair##*:}"
  if [ "$count" -eq 0 ]; then
    echo "test.sh: 错误：清单里的目录一个 .${ext} 都没匹配到 —— 这一类会被静默漏扫。" >&2
    echo "test.sh: （清单来自 make_font_subset.sh --frontend-dirs：${frontend_list//$'\n'/ }）" >&2
    echo "test.sh: 这不是\"通过\"：界面文案就在这三种文件里，少扫一类就等于少守一块。" >&2
    echo "test.sh: 常见原因：那一类文件被搬到了清单之外的目录（改 make_font_subset.sh 的" >&2
    echo "test.sh: FRONTEND_DIRS），或它们的扩展名变了。" >&2
    exit 1
  fi
done
echo "test.sh: 校验前端文件的字体覆盖（check_design_fonts.sh，共 ${#design_files[@]} 个文件：.html ${n_html} / .css ${n_css} / .js ${n_js}）" >&2
if ! bash "$REPO/windows/scripts/check_design_fonts.sh" "${design_files[@]}"; then
  echo "test.sh: 前端字体覆盖判据没过（原因见上面那个脚本的输出）。" >&2
  echo "test.sh: 退出码 3 = 判据没过（有字不在子集里）；1/2 = 环境或用法问题。" >&2
  exit 3
fi

# ---- 0.7) §3.2 的自动守卫（前端不许自造面向用户的字符串）---------------------
#
# 为什么必须有这一步：规格 §3.2 说「JS **不拼接**任何面向用户的字符串」（状态文案 /
# 颜色档位 / 图标名 / 百分比 / 大小 / 时间 / 错误原文 / 空态提示 / 按钮标题全部由
# `shell-core::presentation` 算好）—— 这条是「界面文案自动与 macOS 一致」的**唯一保证**
# （`presentation/` 那 232 条测试逐字对齐 macOS 的 `Presentation/`）。
#
# 而在此之前**没有任何东西在守它**：往 `js/` 里写一句硬编码中文，编译过、测试全绿、
# 构建也过 —— 因为第 0.5/0.6 步判的是"那个字**在不在内嵌子集里**"（豆腐块），
# 它们判不出"这个字**该不该由前端写**"。这条纪律又**没有编译器兜着**
# （原生 JS、无构建步骤，§6.1），所以少了这一步，它就只剩"实现者自觉"。
#
# 判据本体在 `windows/scripts/check_frontend_copy.sh`：白名单、文件清单与**每条的理由**
# 都写在那一份里（加一条登记 = 改一行 = 一次能被审查的决定）。
# ⚠️ 它顺带守住**第二件事**：`windows/web/` 下的**文件清单** —— 那个目录下的任何文件
#    都会被 `shell-win/tauri.conf.json` 的 `build.frontendDist` 编进 exe，
#    "测试夹具放错地方"因此等于"把测试代码发给客户"，必须有东西当场拦下来。
#
# ⚠️ 这里**直接失败、不往下跑**（与 0.5/0.6 同一条口径）：让 cargo 的绿灯盖过一次
#    §3.2 的越界，正是最难发现的那种假绿。
# ⚠️ 缺 python3 时它自己会**大声失败**（W-2，不静默跳过判据），不靠本脚本兜。
echo "test.sh: 校验前端文案的 §3.2 判据（check_frontend_copy.sh）" >&2
if ! bash "$REPO/windows/scripts/check_frontend_copy.sh"; then
  echo "test.sh: §3.2 判据没过（原因见上面那个脚本的输出）——" >&2
  echo "test.sh: 退出码 3 = 判据没过（有未登记的文件/文案，或有已失效的登记）；1/2 = 环境或用法问题。" >&2
  echo "test.sh: 补救：按它的输出改 —— 该由 Rust 给的文案搬进 presentation/，" >&2
  echo "test.sh: 结构文案/引导期诊断则逐条登记进那份脚本的白名单（并写清理由）。" >&2
  exit 3
fi

# ---- 0.8) 载荷夹具的漂移判据（任务 10 第 2 轮加的那一条）----------------------
# 前端那套无头验收吃的是 `windows/scripts/frontend-stub/wire-fixtures.json`，
# 而它是**生成物**（`make_wire_fixtures.sh` 从 `api::tree` / `api::enqueue` /
# `api::state` 现倒出来）。**没有这一步时，它过期的样子与"它是对的"一模一样** ——
# 前端那套验收会对着**旧的线上形状**全绿（那正是任务 10 第 2 轮修掉的那个病：
# 手抄的夹具遮住了真实破坏）。判据本体在 `check_wire_fixtures.sh` 里（重新生成一份、
# 与入库那份逐字节比）。⚠️ 它是**便宜**的（cargo 增量，秒级），所以放在这里而不是"按需跑"。
echo "test.sh: 校验载荷夹具没漂移（check_wire_fixtures.sh）" >&2
if ! bash "$REPO/windows/scripts/check_wire_fixtures.sh"; then
  echo "test.sh: 夹具漂移判据没过（原因见上面那个脚本的输出）。" >&2
  echo "test.sh: 退出码 3 = 判据没过（夹具过期 / 不在场）；1/2 = 环境或用法问题。" >&2
  echo "test.sh: 补救：bash windows/scripts/make_wire_fixtures.sh 之后把新夹具一起提交。" >&2

  exit 3
fi

# ---- 0.9) 前端整屏验收（无头浏览器；任务 10b 接进来的）------------------------
#
# 为什么接进来（R-63 / R-61：**"修好了"必须答得出"它现在被什么守着"**）：
#   任务 10/11/13 各自把对应那一屏的验收做成了**一条命令**
#   （`frontend-stub/` 下的四份 runner），而它们**一个都没接进本脚本** ——
#   跑不跑全看人记不记得。实测的代价：`check_frontend_stub.sh` 里的 I-3b
#   自任务 10 起就是红的（`grep -c check_frontend_stub test.sh` = 0），
#   **红了四个任务而没有任何人会看见**，因为跑它的入口根本不存在。
#   ⇒ 前端是交付物的一半，只在有人记得时才验 = 没有任何东西在守它。
#
# ⚠️ **不做条件跳过（W-2）**：这一步要一个**无头浏览器**（Tauri 的 webview 是 WebView2
#    = Chromium 内核，所以拿 Chromium 系的浏览器验才与交付可比）。找不到就**在这里
#    大声失败**并给出**可执行的**三平台补救 —— 跳过等于"这一步从来没跑过而没人知道"。
#    ⚠️ 代价如实记账：本步让 `test.sh` 多了一条**硬前置**（浏览器），
#      四份 runner 在 macOS 上实测合计约 60 秒（整轮 `test.sh` 连 cargo 一起 80 秒，
#      2026-09-20 在本机量的）。这是**看得见**的代价，比"四条验收靠人记得"便宜。
#      两个声明本脚本前置工具的文档也一起改了
#      （`docs/superpowers/2026-09-19-windows-dev-setup.md` §1.6、`windows/README.md` §0b）——
#      只改脚本不改文档，会让那台 Windows 机器上的人拿到一句对不上的补救。
#
# ⚠️ 浏览器**在这里探一次就够了**：探到的路径 `export BENAGEN_BROWSER` 出去，
#    四份 runner 都认那个变量（它们各自也有一套探测，优先级同样是它）。
#    不 export 的话就是两处各探一次 —— 探法一旦分叉，会出现
#    "test.sh 说找得到、runner 说找不到"这种**两个真相源**的失败。
frontend_browser=""
if [ -n "${BENAGEN_BROWSER:-}" ]; then
  if [ -x "$BENAGEN_BROWSER" ]; then
    frontend_browser="$BENAGEN_BROWSER"
  elif command -v "$BENAGEN_BROWSER" >/dev/null 2>&1; then
    frontend_browser="$(command -v "$BENAGEN_BROWSER")"
  fi
fi
if [ -z "$frontend_browser" ]; then
  frontend_candidates=()
  case "$(uname -s 2>/dev/null || echo unknown)" in
    Darwin)
      frontend_candidates+=("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge")
      frontend_candidates+=("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
      frontend_candidates+=("/Applications/Chromium.app/Contents/MacOS/Chromium")
      ;;
  esac
  for t in msedge microsoft-edge microsoft-edge-stable google-chrome google-chrome-stable \
           chromium chromium-browser; do
    if command -v "$t" >/dev/null 2>&1; then
      frontend_candidates+=("$(command -v "$t")")
    fi
  done
  # Windows（Git Bash / MSYS2）上 Edge 的常见落点 —— 本仓库要搬到那台机器上继续开发。
  for p in "/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe" \
           "/c/Program Files/Microsoft/Edge/Application/msedge.exe"; do
    [ -x "$p" ] && frontend_candidates+=("$p")
  done
  for c in ${frontend_candidates[@]+"${frontend_candidates[@]}"}; do
    if [ -x "$c" ]; then frontend_browser="$c"; break; fi
  done
fi
if [ -z "$frontend_browser" ]; then
  # W-2：缺工具就大声说，**绝不**跳过判据继续跑。补救按平台给三行（第 0.5/0.6 步的
  # `need` 是同一个形状）。
  echo "test.sh: 错误：找不到一个可用的无头浏览器（Chromium 内核：Edge / Chrome / Chromium）。" >&2
  echo "test.sh: 第 0.9 步要用它跑前端那四套整屏验收（真 index.html、真模块、真轮询；" >&2
  echo "test.sh: Tauri 的 webview 就是 WebView2 = Chromium 内核，所以只有 Chromium 系与交付可比）。" >&2
  echo "test.sh: **跳过它等于这四套验收从来没有跑过**（W-2），所以这里直接失败而不是跳过。" >&2
  echo "test.sh: 补救（按你的平台挑一行）：" >&2
  echo "test.sh:   macOS: 装 Microsoft Edge 或 Google Chrome（放进 /Applications 即可）" >&2
  echo "test.sh:   Debian/Ubuntu: apt-get install -y chromium   （或 chromium-browser / google-chrome-stable）" >&2
  echo "test.sh:   Windows: 装 Edge（系统自带；Git Bash 里若能跑 \`\"/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe\" --version\` 就够了）" >&2
  echo "test.sh: 或者显式指定：BENAGEN_BROWSER=/path/to/浏览器 bash windows/scripts/test.sh" >&2
  exit 1
fi
export BENAGEN_BROWSER="$frontend_browser"
echo "test.sh: 前端验收用的浏览器：$frontend_browser" >&2

# ⚠️ 四份 runner 各自探端口（8391/8392/8397/8377 互不相同），所以**串行**跑就够了，
#    不必担心撞端口 —— 但也别改成并发：它们各自起一个本地服务 + 一个浏览器进程，
#    并发只会在失败时把三份输出搅在一起。
#
# ⚠️ 退出码沿用它们自己的那一套（与 `test.sh` 同源）：0 = 全过；1 = 环境问题
#    （缺 python3/curl、服务起不来）；3 = **判据没过**（有 FAIL / 有 ABORT / 空跑）。
#    ⇒ 本步原样把它们传出去，别把 3 压成 1：那会让"产品坏了"看起来像"环境不对"。
#
# ⚠️ **过滤器不改变这一步**（有意的）：`--filter` 管的是 **cargo 用例**，而这四份
#    验收不是 cargo 用例。要它"因为带了过滤器就不跑"= 给守卫开一个静默的旁路，
#    而旁路一旦开了，下一次"红了没人看见"就从这里进来。
frontend_runners=(
  "check_frontend_stub.sh|常驻 stub：失败原文逐字透传 + 载荷驱动"
  "check_files_screen.sh|文件页整屏（列/面包屑/勾选/右键/底栏）"
  "check_transfers_screen.sh|传输列表屏整屏（行/行菜单/摘要/空态）"
  "check_panels_screen.sh|任务 13 那一套（工具栏下载按钮 / 换码面板 / 设置 / 关于 / 许可）"
)
for entry in "${frontend_runners[@]}"; do
  runner="${entry%%|*}"
  what="${entry#*|}"
  echo "test.sh: 前端验收：${what}（${runner}）" >&2
  set +e
  bash "$REPO/windows/scripts/$runner"
  frontend_rc=$?
  set -e
  if [ "$frontend_rc" -ne 0 ]; then
    echo "test.sh: 前端验收没过：${runner}（退出码 ${frontend_rc}）—— 它的输出在上面。" >&2
    if [ "$frontend_rc" -eq 3 ]; then
      echo "test.sh: 退出码 3 = **判据没过**（有 FAIL / ABORT，或空跑）。" >&2
      echo "test.sh: 补救：按它逐条打出来的断言改 —— 那几行就是「哪一条、期望什么、实际什么」。" >&2
    else
      echo "test.sh: 退出码 ${frontend_rc} = **环境/用法问题**（不是产品判据）—— 按它给的补救走。" >&2
    fi
    echo "test.sh: ⚠️ 这一条**不许**放宽或跳过（红着就是红着）：它守的是「界面在真浏览器里长什么样」。" >&2
    exit "$frontend_rc"
  fi
done
echo "test.sh: 前端四套整屏验收全过（stub + 文件页 + 传输列表 + 任务 13 那一套）" >&2

# ---- 1) 重建宿主内核 ---------------------------------------------------------
if [ ! -d "$REPO/core" ]; then
  # W-2：不许静默降级。缺内核就大声说，并给出可执行的补救。
  echo "test.sh: 错误：找不到 $REPO/core，无法重建内核。" >&2
  echo "test.sh: 壳的 e2e 拉起的是 core/target/release/ 下的 benagen-core[.exe]；" >&2
  echo "test.sh: 缺了它只会得到假红或假绿，所以这里直接失败而不是跳过。" >&2
  echo "test.sh: 补救：核对仓库布局——本脚本期望自己的位置是 <repo>/windows/scripts/test.sh。" >&2
  exit 1
fi
echo "test.sh: 重建内核（cargo build --release；已有产物时是秒级 no-op）" >&2
(cd "$REPO/core" && cargo build --release)

# W-1：每一条构建路径都必须自己验产物。
#   上面那条 `cargo build` **成功**并不等于产物落在 e2e 消费的那个路径上：
#   `core/Cargo.toml` 的 `[[bin]]` 被改名/移除，或构建被交叉编译目标接管
#   （产物落到 `core/target/<triple>/release/`），两种情况下 cargo 都照样退出 0。
#   不验的话，脚本会带着一个**不存在或过期的**内核往下跑，然后在壳的 e2e 那里
#   表现成"内核是旧的那一个"——正是本脚本头部注释声称要堵死的那个坑（Ruling C19）。
#
# ⚠️ 产物名**按宿主平台取**（控制者裁决 P-1）：Windows 宿主（Git Bash / MSYS2 / Cygwin）上
#    cargo 的产物叫 `benagen-core.exe`。写死无扩展名的那个，守卫会在 Windows 上以
#    "`[[bin]]` 被改名/移除"这条**错误的根因**硬失败——本项目对"根因不许说错"有纪律：
#    守卫失败时给出的理由必须是它**真正检查到的东西**。
host_os="${OS:-$(uname -s 2>/dev/null || echo unknown)}"
case "$host_os" in
  Windows_NT | MINGW* | MSYS* | CYGWIN*) kernel_exe="benagen-core.exe" ;;
  *) kernel_exe="benagen-core" ;;
esac
kernel_bin="$REPO/core/target/release/$kernel_exe"
if [ ! -f "$kernel_bin" ] || [ ! -x "$kernel_bin" ]; then
  # ⚠️ `$kernel_bin` 必须写成 `${kernel_bin}`：本机 `bash` 是 macOS 自带的 3.2.57，
  #    它会把紧跟变量名之后那个**全角字符的第一个字节**当成标识符的一部分，
  #    于是 `$kernel_bin（` 被解析成未定义的 `kernel_bin<0xEF>` ⇒ set -u 报
  #    "unbound variable" 并退出。那样这条守卫会以**错误的理由**退出 1——
  #    正是本任务在防的那种假绿/假红。（实测：加了守卫但没加花括号时，
  #    P1 探针的退出码是 1，却来自 unbound variable，守卫根本没执行到。）
  echo "test.sh: 错误：内核构建结束，但产物不在 ${kernel_bin}（不存在或不可执行）。" >&2
  echo "test.sh: 壳的 e2e 拉起的就是这个路径；它不在，测试只会给出假绿或假红。" >&2
  echo "test.sh: 期望的产物名按宿主平台取（host=$host_os → ${kernel_exe}）。" >&2
  echo "test.sh: 常见原因一：core/Cargo.toml 的 [[bin]] 被改名/移除。" >&2
  echo "test.sh: 常见原因二：构建被交叉编译目标接管，产物落到了 core/target/<triple>/release/。" >&2
  echo "test.sh: 补救：核对 core/Cargo.toml 的 [[bin]] name，或把本脚本与 e2e 的路径一并改成新的。" >&2
  exit 1
fi
echo "test.sh: 内核产物已验证：$kernel_bin" >&2

# ---- 2) 跑壳的测试 -----------------------------------------------------------
if [ "${#filters[@]}" -gt 0 ]; then
  echo "test.sh: 过滤器：${filters[*]}" >&2
fi

log="$(mktemp)"
# ⚠️ **一条 trap 收两个临时文件**：`trap` 是**覆盖**语义，不是追加——这里若只写
#    `rm -f "$log"`，上面那条 `trap 'rm -f "$guard_tmp"' EXIT` 就被顶掉了，
#    而这条 trap 每次**正常跑完**都会执行 ⇒ 每成功跑一次就往 $TMPDIR 里漏一个文件。
#    （上面那条 trap 仍然要留：守卫失败会在到达这一行之前就 exit。）
#
# ⚠️ **注释里的绝对行号天生不可维护**（这一处已经错过三次：126 → 129 → 156 → 160）：
#    所以这一条改成**锚点引用**（引那句 trap 的原文），不写行号。
#    改法：要引用本文件里的另一处，写它**长什么样**（那一行的内容），别写它在第几行。
trap 'rm -f "$log" "$guard_tmp"' EXIT

set +e
(cd "$REPO/windows" && cargo test -p shell-core -p shell-win ${filters[@]+"${filters[@]}"}) 2>&1 | tee "$log"
status="${PIPESTATUS[0]}"
set -e

# ---- 3) 空跑绿灯判为失败 -----------------------------------------------------
# `cargo test` 每个测试目标打印一行 `running N test(s)`（含 doc-test）。
# 求和：为 0 说明**一条用例都没真跑**——过滤器没匹配上，或测试目标根本没编进来。
#
# ⚠️ 正则锚定行首行尾（`^...$`），它挡的是**带前缀/后缀**的同形行：被测代码自己的 stdout
#    （`--nocapture` 下原样打出来）里的 `foo running 5 tests`、`running 5 tests 了`
#    这类行，不锚定就会被一并算进总数，**抬高计数、掩掉同一次运行里真正的
#    `running 0 tests`**——那是假绿。
#
# ⚠️ **它挡不住的，是整行恰好等于 `running N tests` 的诱饵**（实测：那样仍会被计入）。
#    这是**已知的残留风险**，不要把它说成"只有 libtest 的汇总行才算数"——那句话说过头了
#    （本注释的第一版就是这么写的，复审时被点名）。
#    之所以**不**去收紧成"必须夹在 libtest 的汇总块里"：本脚本的输入是**无界的 stdout**，
#    任何基于行的启发式都能被一条同形的整行诱饵绕过——那只是把门槛挪一格，却会给**正常**
#    的运行引入假红的风险，而假红会让人把守卫整个关掉，代价比留一条已知的窄缝更大。
#    真正堵住这条输入路径的是**参数解析**：`--nocapture` 之类的未知 flag 一律 `exit 2`，
#    被测代码的 stdout 混不进来。所以这一段是**纵深防御**，不是活漏洞。
running_lines="$(grep -oE '^running [0-9]+ tests?$' "$log" || true)"
total=0
if [ -n "$running_lines" ]; then
  total="$(printf '%s\n' "$running_lines" | grep -oE '[0-9]+' | awk '{n += $1} END {print n + 0}')"
fi
echo "test.sh: 实际运行用例数（所有 'running N tests' 行之和）：$total" >&2

if [ "$total" -eq 0 ]; then
  echo "" >&2
  echo "test.sh: 一个测试都没跑（所有 'running N tests' 行的总和为 0），判为失败。" >&2
  if [ "$status" -ne 0 ]; then
    # 别把根因说错：cargo 自己就失败了（编不过/链不过），空跑只是它的副作用。
    echo "test.sh: 注意：cargo 自己也以退出码 $status 结束——先看上面的编译错误，那才是根因。" >&2
  else
    echo "test.sh: cargo 在这种情形下的退出码是 0，但空跑出来的绿灯不算证据。" >&2
  fi
  echo "test.sh: 传了过滤器就核对拼写；没传就说明测试目标没编进来——检查 src/ 下" >&2
  echo "test.sh: 新增的 .rs 文件有没有在父模块里写 pub mod X;（没被声明的模块，" >&2
  echo "test.sh: 它的测试一条都不会跑，而 cargo test 照样全绿）。" >&2
  exit 1
fi

exit "$status"
