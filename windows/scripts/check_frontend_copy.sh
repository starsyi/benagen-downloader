#!/usr/bin/env bash
#
# windows/scripts/check_frontend_copy.sh —— **§3.2 的自动守卫**：
# 前端（`windows/web/**`）里不许出现"自造的面向用户的字符串"。
#
#    bash windows/scripts/check_frontend_copy.sh
#
# ---------------------------------------------------------------------------
# 为什么需要这个脚本（**这是本代最要紧的一条纪律，而此前没有任何东西在守它**）
# ---------------------------------------------------------------------------
#   规格 §3.2（原文）：
#
#     > JS **不拼接**任何面向用户的字符串。凡是要显示的东西——状态文案、颜色档位、
#     > 图标名、百分比、大小、时间、错误原文、空态提示、按钮标题——都由
#     > `shell-core::presentation` 算好，经命令层序列化成 JSON 交给 JS。
#     > **JS 只负责摆位置。**
#
#   这条是「界面文案自动与 macOS 一致」的**唯一保证**（`presentation/` 那 232 条测试
#   逐字对齐 macOS 的 `Presentation/`，前端不自己造字，两边就永远不会分叉）。
#
#   ⚠️ 但在此之前，**往 `js/` 里写一句硬编码中文不会有任何东西变红**：
#      · `test.sh` 第 0.5/0.6 步只判"**字体覆盖**"（那个字在不在内嵌子集里），
#        它判不出"这个字**该不该由前端写**"——一句硬编码中文只要那个字在子集里，
#        两条判据都是绿的，而 §3.2 已经被踩了；
#      · 这条纪律**没有编译器兜着**（原生 JS、无构建步骤，见 §6.1）——
#        它此前完全靠"实现者自觉"。
#   一个不被任何东西守着的纪律，在客户机器上的后果与"没有这条纪律"完全一样（W-2）。
#   本脚本就是那个"东西"。**任务 9 的实现者自己复核后主动提了这件事**（它做了检查、
#   给了数字、说明那条一次性脚本没进仓库），这就是它被单列成任务 9b 的原因。
#
# ---------------------------------------------------------------------------
# 判据（三条，都 fail-closed）
# ---------------------------------------------------------------------------
#   ③ **交付副本的字节**（任务 18 加的）：`windows/web/` 下有几份**入库的副本**
#      （两张品牌图），它们必须与源文件**逐字节相同**（`TWIN_FILES`）。
#      ⚠️ 字体那两份的同一条判据住在 `make_font_subset.sh --check`（不在这里重复，
#      理由见 `TWIN_FILES` 上面那段）。为什么必须有：这份副本是**进了 exe 的**，
#      而"换了源忘了重拷"的表现是"界面上悄悄换回旧的 logo"，**所有判据都绿** ——
#      与 ① 只判"在不在"正好差一层（① 判不出"在的是不是那一份"）。
#
#   ① **文件清单**：`windows/web/` 下的**每一个文件**都必须在 `EXPECTED_FILES` 里
#      （清单里登记的、而现场没有的，也要红）。
#      为什么：`shell-win/tauri.conf.json` 的 `build.frontendDist = "../web"`
#      ⇒ **这个目录下的任何文件都会被编进 exe**。一个"只是测试用的""顺手放的"文件
#      会**悄悄发给客户**——任务 9 的受控 stub 就是因此做完就删的（它的落点现在在
#      `windows/scripts/frontend-stub/`，那个位置不会被编进产物）。
#      这条判据把"放错地方"从"没人会发现"变成"当场红"。
#
#   ② **非 ASCII 文案**：`windows/web/**`（`.html`/`.css`/`.js`）里，**剥掉注释之后**
#      出现的每一个"非 ASCII 指纹"都必须在 `WHITELIST` 里逐条登记。
#
#      指纹 = **一行里、剥掉注释后的文本中，把所有非 ASCII 字符按出现顺序拼起来的那串**
#      （ASCII 与空白不参与、也不切断它）。
#      ⚠️ 为什么是"指纹"而不是"连续的片段"：一行里的那句话常常夹着英文标识符
#      （`tauri.conf.json` / `window.__TAURI__`），按连续片段切会把一句话切成
#      `"的"`、`"："` 这种碎片。指纹按**行**取，一个诊断站点的一句就是一条，
#      读得出来、也对得上源码。
#      ⚠️ 指纹**不含 ASCII** ⇒ 改代码里的英文名（配置项、命令名）**不需要**动白名单：
#      本判据判的是**会被渲染的字**，不是"源码里的所有字节"（同
#      `make_font_subset.sh` 头部"为什么是字面量而不是全部源码字符"那一节的口径）。
#
#   ⚠️⚠️ **抽取规则不在这里重写一份**：剥注释走 `check_design_fonts.py` 的
#      `rendered_text()`（**import 它**）——那条路径已经在用（第 0.6 步）、已经被审查过，
#      而且它对 `.html`/`.css`/`.js` 各有一套"宁可少剥、不可多剥"的规则（那一节的长注释
#      在 `check_design_fonts.sh` 头部）。**别在这里造第二套解析**：两套解析会分叉，
#      而分叉的表现恰好是"判据永远绿灯"。
#
# ---------------------------------------------------------------------------
# 白名单（`WHITELIST`）——**短、显式、逐条带理由、且绑处数**
# ---------------------------------------------------------------------------
#   形状：`("仓库相对路径", "指纹"): (期望处数, "为什么它是合法的")`。
#
#   ⚠️⚠️ **登记 = 处数**（2026-09-20 第 1 轮修复轮加的）：每条记的是
#      "这个文件里**恰好**出现这么多处"，两向都判：
#        · **多于**期望 ⇒ 红 —— 出现了**新的一处**（这是守卫真正要抓的那件事）；
#        · **少于**期望 ⇒ 红 —— 登记烂了（与下面那条反向闭环同源）。
#      ⚠️ **为什么不用行号**：行号对"在文件上方插一行"极其敏感，那种假红会把人训练成
#         "红了就改行号"——而**改行号这个动作不经过任何判断**。
#         处数对**新增一处**敏感、对**行位移**不敏感 —— 那正是我们要的：
#         该红的是"这个界面词被用到了第三处"，不是"它挪到了第几行"。
#      ⚠️ 初版这里是**一个 `set`**、只按 `(文件, 指纹)` 索引 ⇒ **同一个词第二次出现时
#         守卫失明**（审查实测：往 `index.html` 末尾追加一行 `<p>重试</p>` ⇒ 退出 0，
#         还报"26 条登记全部命中"）。那正是"这份守卫存在的全部理由"被绕过：
#         `index.html` 那 15 条里有 **9 条单独看就是正常界面词**（`文件` `传输列表`
#         `校验结果` `批次摘要` `交付码` `设置` `重试` `收起` `加载`），
#         任务 10/11/12/13 往 `<template>` 里加一颗叫「重试」的按钮时，本该
#         **必须显式登记一次**。⇒ 计数就是那条"必须"的落点。
#      ⚠️ 计数变大时，**理由也必须跟着写清"新增的是哪一处、为什么它合法"** ——
#         把数字改大**不是**理由（那只是把同一句话再说一遍）。
#
#   ⚠️ **今天合法的只有两类**，两类**性质完全不同**，别把它们的理由混起来读：
#     · **类 1：结构文案（只可能出现在 `.html` 里）** —— 导航项名、按钮名、分组名、
#       标题句那一类**与数据无关、任何状态下都一样**的字。它们住在 HTML 里、由浏览器
#       直接渲染，**JS 一个字都不碰**（JS 只克隆与摆放，见 `dom.js:template()`），
#       所以它们不落在 §3.2 的管辖面里（§3.2 管的是 **JS** 拼串）。
#       `index.html` 头部有一整段论证这件事，包括"为什么不由 Rust 给"
#       （`presentation/` 里没有这些字的模块；macOS 侧它们同样住在视图文件里）
#       —— **本清单就是那份账的机器可读版本**：哪一句还没进 `presentation/`，
#       这里逐条看得见。
#     · **类 2：引导期诊断（只允许出现在 `.js` 里）** —— "**程序自己坏了**"那一类
#       （"没有 Tauri"、"找不到挂载点"、"这个屏没注册"）。它们说的是**管线**坏了，
#       不是"这个世界怎么样了"；而管线坏了的时候 **Rust 那边一个字都给不出来**
#       （请求根本发不出去），所以这些话没有别的地方可放。`invoke.js` 头部那条
#       "界面文案 vs 诊断"的界线是它们的出处。
#
#   ⚠️ **"诊断"这一类必须逐条登记，不许放宽成"诊断都行"**：放宽的那一刻，任何人
#      都可以把一句**面向用户的文案**写成 `throw` 或者 `console.error` 的形状绕过判据
#      —— 而"绕过判据的写法"一旦存在，判据剩下的就只有心理安慰作用。
#      加一条 = 加一行 `WHITELIST` + 写一句理由：**那是一次能被审查的决定**。
#
#   ⚠️ 反向闭环（同 `make_font_subset.sh` 的 `EXEMPT_CODEPOINTS` 与 `test.sh` 的
#      `guard_allow`）：**登记了、而源码里已经找不到的指纹 ⇒ 红**。
#      不清掉的话，清单会烂在那里；而烂掉的清单里，下一条登记的"理由"就没人读了。
#      （"少于期望处数"是这条闭环的加强版：不只是"找不到"，还包括"少了一处"。）
#
# ---------------------------------------------------------------------------
# ⚠️ 已知盲区（**如实记账 —— 别让下一个读者以为"扫过就是全干净"**）
# ---------------------------------------------------------------------------
#   本判据只看**非 ASCII 字符** ⇒ **纯 ASCII 的面向用户字符串它扫不到**。
#   例：`throw new Error("Failed to load")`、`el.textContent = "Retry"`、
#   `h("button", { text: "Download" })` —— 那同样是"JS 自造了一句面向用户的话"
#   （§3.2 明禁），但**不会红**。
#
#   今天这条盲区的影响有限：本代的界面文案是中文（逐字对齐的对象 macOS 也是中文），
#   一句英文文案会立刻与 macOS 不一致 ⇒ 属于**肉眼可见**的那一类缺陷，
#   而不是"藏起来的那种"。
#   但它**不是"被守住了"**：真要堵，得再加一条"字符串字面量里不许出现英文句子"的判据
#   （那需要一份 JS 词法，本任务不做）。**本任务如实记下它，不假装它被守住了。**
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_design_fonts.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 判据通过
#   1 = 环境问题（W-2：缺 python3、或 `windows/web/` 整个不见了 ——
#       **大声失败并给出可执行的补救**，绝不跳过判据）
#   2 = 用法错误（给了不认识的参数）
#   3 = **判据没过**（有未登记的文件 / 有未登记的指纹 / 有失效的登记）——
#       补救是"改文案或登记一处"，与 1/2 的"修环境"不同
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WEB="$REPO/windows/web"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# ---------------------------------------------------------------------------
# 参数：**一个都不认**（不认识的直接报错退出，绝不透传）
# ---------------------------------------------------------------------------
# 与 `test.sh`/`make_font_subset.sh` 同一条纪律：透传一个打错的 flag 会让脚本以一个
# "看起来像失败"的退出码收场，而根因是打错了字 —— 那样的红与没有判据等价。
# 本判据今天没有参数（扫描面与白名单都写死在这里，刻意如此：它们是**数据**，
# 而数据要能被审查，就必须住在同一个文件里、改一次是一次能被看见的改动）。
while [ "$#" -gt 0 ]; do
  echo "check_frontend_copy.sh: 不认识的参数：$1" >&2
  echo "check_frontend_copy.sh: 本脚本不接受任何参数（扫描面 windows/web/ 与白名单都写在本文件里）。" >&2
  exit 2
done

# ---------------------------------------------------------------------------
# 工具预检（W-2：缺工具**大声说**并给出**可执行的补救**）
# ---------------------------------------------------------------------------
# ⚠️ **只需要 python3**：本判据不读字体、不需要 fontTools、不联网
#    （它要能进 `test.sh` 的每一步）。`test.sh` 第 0 步已经把 python3 当硬前置了。
need() {  # need <tool> <why> <fix>
  if ! command -v "$1" >/dev/null 2>&1; then
    {
      echo "错误：PATH 里没有 \`$1\` —— $2"
      echo "      补救（按你的平台挑一行）："
      while IFS= read -r fix_line; do
        echo "        $fix_line"
      done <<< "$3"
    } >&2
    exit 1
  fi
}
need python3 "要它剥注释、扫非 ASCII（本判据的全部实现）" \
             "macOS: 系统自带 python3
Debian/Ubuntu: apt-get install -y python3
Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`（或 python.org 的安装包）
         ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\`；本脚本按 \`python3\` 找解释器，两个名字都要能被找到"
[ -d "$WEB" ] || die "找不到 $WEB —— 判据无对象可判（这不是\"通过\"）。
      `windows/web/` 是这一代界面的家（`shell-win/tauri.conf.json` 的 frontendDist）。
      它整个不见了 ⇒ 要么 checkout 不完整，要么界面被搬走而本脚本没跟上：两种情况都要人来看。"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

helper="$WORK/check_frontend_copy.py"
cat > "$helper" <<'PY'
# -*- coding: utf-8 -*-
"""§3.2 的自动守卫 —— `windows/web/**` 里不许出现自造的面向用户的字符串。

⚠️ 剥注释**不在这里重写一份**：用 `check_design_fonts.rendered_text()`（import 它）。
   那条路径是"哪些字会被渲染"的唯一一份实现，第 0.6 步的字体覆盖判据用的也是它。
"""
import collections
import os
import sys

REPO = sys.argv[1]
WEB = os.path.join(REPO, "windows", "web")
# 抽取规则的家：`windows/scripts/check_design_fonts.py`（同目录）。
sys.path.insert(0, os.path.join(REPO, "windows", "scripts"))
import check_design_fonts as cdf   # noqa: E402 —— 只 import（它没有副作用、不需要 fontTools）

# 会被扫的扩展名。⚠️ `rendered_text()` 对**认不出**的扩展名一律不剥注释
#（"多抽，安全的那一侧"）—— 那条规则在这里用不上：下面的文件清单判据已经把
# "多出来的文件"整个拦掉了，所以这里只需列出**允许存在**的三种。
SCAN_EXTENSIONS = (".html", ".css", ".js")

# ---------------------------------------------------------------------------
# 判据①：`windows/web/` 下的文件清单（**唯一属主是本文件**）
# ---------------------------------------------------------------------------
# ⚠️ 全部**仓库相对路径**，逐条登记。加文件 = 加一行：那是一次能被审查的决定。
# ⚠️ 这份清单**同时也是"哪些文件会进 exe"的账**（frontendDist 指的就是这个目录）。
#    `windows/scripts/frontend-stub/`（常驻夹具）**不在**这里，也**不该**在这里。
EXPECTED_FILES = [
    # ---- 结构 / 样式 ------------------------------------------------------
    ("windows/web/index.html", "窗口骨架 + 两块 <template>（结构文案的家）"),
    ("windows/web/app.js", "引导与路由（装配壳、起轮询、每一拍灌载荷）"),
    ("windows/web/app.css", "入口样式表（@font-face + 各屏汇总）"),
    # ---- 内嵌字体（任务 14）-----------------------------------------------
    # ⚠️ 这两份是 `windows/assets/` 下同名文件的**逐字节副本**，放进来只有一个理由：
    #    `build.frontendDist = "../web"` ⇒ **只有这个目录下的文件才进 exe**，
    #    而 webview 取不到 `web/` 之外的东西（`../assets/…` 会被 URL 规则夹回根、
    #    静默 404 ⇒ 回退系统字体）。理由与"重新生成子集后必须重拷"那一步写在
    #    `windows/web/app.css` 末尾那一块上（**唯一一份**，别在这里复述成一处分叉）。
    # ⚠️ 唯一真相仍是 `windows/assets/` 那两份（字体由 `make_font_subset.sh` 生成、
    #    sha256 被 `SUBSET_SHA256` 钉着；许可全文与官方 `LICENSE` 逐字节相同）。
    ("windows/web/fonts/ui-subset.otf", "内嵌界面字体子集（Noto Sans SC Regular，OFL-1.1；`windows/assets/ui-subset.otf` 的交付副本）"),
    ("windows/web/fonts/OFL-1.1.txt", "该字体的许可全文（OFL 条件 2 的随附义务；跟字体同目录走）"),
    # ---- 品牌资产（任务 18）------------------------------------------------
    # ⚠️ 这两张图的出处是 `macos/Resources/`（那两份才是入库的源，`macos/scripts/
    #    make_brand_assets.py` 生成它们）。放一份到这里只有一个理由，与字体那两份
    #    逐字同款：`build.frontendDist = "../web"` ⇒ **只有这个目录下的文件进 exe**，
    #    而 webview 取不到 `web/` 之外的东西（`../Resources/…` 会被 URL 规则夹回根、
    #    静默 404 ⇒ 界面上是一张破图）。
    # ⚠️ 它们**没有字形** ⇒ 不进字体覆盖判据的扫描面（那一份按扩展名只取
    #    `.html`/`.css`/`.js`）；守着它们的是下面这两条：本清单，以及 `TWIN_FILES`。
    ("windows/web/img/benagen-mark.png", "空态页顶部的品牌图形标（`macos/Resources/benagen-mark.png` 的交付副本，`EmptyState.swift` 的 `mark`）"),
    ("windows/web/img/benagen-full-logo.png", "关于窗口的全称 logo（`macos/Resources/benagen-full-logo.png` 的交付副本，`AboutView.swift` 的 `logoCard`）"),
    ("windows/web/css/tokens.css", "设计令牌（尺寸/配色/字号，逐字来自 a0-proof.html）"),
    ("windows/web/css/base.css", "基础重置与排版"),
    ("windows/web/css/shell.css", "壳：侧栏 / 工具栏 / 常驻提示行"),
    ("windows/web/css/screens/empty.css", "空态屏"),
    # ---- 逻辑（js/）-------------------------------------------------------
    ("windows/web/js/shell.js", "壳的挂载与数据槽位"),
    ("windows/web/js/render.js", "载荷 → 槽位的纯映射"),
    ("windows/web/js/dom.js", "DOM 原语 + 图标表"),
    ("windows/web/js/invoke.js", "唯一碰 window.__TAURI__ 的文件"),
    ("windows/web/js/poll.js", "轮询节拍"),
    ("windows/web/js/screens/registry.js", "屏的注册表与接口契约"),
    ("windows/web/js/screens/empty.js", "空态屏（本任务唯一已实现的屏）"),
    ("windows/web/js/screens/files.js", "文件页（任务 10：四列/面包屑/勾选/右键/底栏）"),
    ("windows/web/css/screens/files.css", "文件页的样式（任务 10）"),
    ("windows/web/js/screens/verify.js", "校验结果屏（任务 12：总进度 + 一行总结 + 恒六类）"),
    ("windows/web/css/screens/verify.css", "校验结果屏的样式（尺寸照 VerifyView.swift）"),
    ("windows/web/js/screens/transfers.js", "传输列表屏（任务 11：行/行菜单/全局摘要/空态，200 ms 一拍）"),
    ("windows/web/css/screens/transfers.css", "传输列表屏的样式（尺寸照 TransfersView.swift / TransferRowView.swift）"),
    ("windows/web/js/switchcode.js", "换码面板（任务 13：历史列表 + 备注就地编辑 + 点一行直接切换）"),
    ("windows/web/js/settings.js", "设置窗口（任务 13：下载目录段 + 七项参数 + 许可入口）"),
    ("windows/web/js/dialogs.js", "模态层（任务 13）+ 关于窗口 + 许可全文窗口"),
    ("windows/web/css/panels.css", "模态层的样式（任务 13）"),
]

# ---------------------------------------------------------------------------
# 判据③：`windows/web/` 下的**交付副本**必须与它的源文件**逐字节相同**（任务 18）
# ---------------------------------------------------------------------------
# ⚠️ **为什么它在这里**：判据①（文件清单）只判"文件在不在"，判不了"**在的是不是
#    那一份**"。而这一代 `windows/web/` 下有几份**入库的副本**（字体、许可、两张
#    品牌图），它们的源在别处 —— 换了源而忘了重拷的后果是"界面上悄悄换回旧的"
#    （旧字形 / 旧 logo），**而所有判据都绿**。
#    ⚠️ 字体那两份的**同一条判据**住在 `make_font_subset.sh --check`（那里是
#       `assets/` 那份的属主，资产的身份闭环也在那）—— 这里**不重复登记**它们：
#       同一件事两处判会分叉，而分叉的那一份迟早变成第二条真相。
#       品牌图这两份的源头在 `macos/`（本脚本够得着、而字体脚本够不着），
#       所以它们的家在这里。
# ⚠️ 形状与 `EXPECTED_FILES` 一样是**一张数据表**：加一份副本 = 加一行 = 一次能被
#    审查的决定。表里的路径全部是**仓库相对路径**。
# ⚠️ 比的是 **sha256**（不引 `cmp`）：本脚本的硬前置只有 python3（见头部
#    "工具预检"），而"抽取/解析"的那一份实现已经 import 进来了（`cdf`）。
TWIN_FILES = [
    # (交付副本, 唯一的源, 为什么需要这份副本)
    ("windows/web/img/benagen-mark.png",
     "macos/Resources/benagen-mark.png",
     "空态页顶部那颗图形标。源在 macOS 那侧（`make_brand_assets.py` 生成），"
     "而 webview 只能取到 `web/` 里的东西"),
    ("windows/web/img/benagen-full-logo.png",
     "macos/Resources/benagen-full-logo.png",
     "关于窗口那颗全称 logo，同上"),
]


def twin_problems():
    """返回 [(副本, 源, 为什么)]；空列表 = 全部逐字节相同。

    ⚠️ **源文件不在场 ⇒ 判据无对象可判 ⇒ 红**（不写成"找不到就跳过"：
       那正是 `make_font_subset.sh` 第 10 步记着的那次回归的形状 ——
       判据在"文件不在"时**安静地不执行**，而读者看到的仍然是一句"完成"）。
    ⚠️ 副本不在场**不在这里报**：判据①已经报过（`EXPECTED_FILES` 两向都判），
       同一件事报两遍会把读者引向"到底哪条才算"。
    """
    out = []
    for dst, src, _why in TWIN_FILES:
        d, s = os.path.join(REPO, dst), os.path.join(REPO, src)
        if not os.path.isfile(s):
            out.append((dst, src, f"源文件不在场（{src}）—— 判据无对象可判，这不是「通过」"))
            continue
        if not os.path.isfile(d):
            continue
        if cdf.sha256_file(d) != cdf.sha256_file(s):
            out.append((dst, src, "两份不是逐字节相同（副本停在旧版本 / 被手工换过）"))
    return out


# ---------------------------------------------------------------------------
# 判据②：非 ASCII 指纹的白名单（**逐条带理由**；理由的两类见本脚本头部）
# ---------------------------------------------------------------------------
WHITELIST = {
    # =====================================================================
    # 类 1：结构文案（只出现在 `.html` 里；浏览器直接渲染，JS 不碰）
    # =====================================================================
    # 文件：windows/web/index.html
    # 理由（对下面每一条都成立）：它是**结构**——与数据无关、任何状态下都一样的字
    #   （导航项名 / 按钮名 / 分组名 / 标题句 / 输入框占位符）。JS 只**克隆与摆放**
    #   它们（`dom.js:template()`），不拼串 ⇒ 不在 §3.2 的管辖面里。
    #   `index.html` 头部有完整论证，包括"为什么不由 Rust 给"：`presentation/` 里
    #   没有这些字的模块，而 macOS 侧它们同样住在视图文件里（`Sidebar.swift` 的
    #   `SidebarSection.title`、`EmptyState.swift`、`RootView.swift` 的「设置」）。
    #   ⚠️ **本表就是那份"哪些字还没有 Rust 的家"的账**：任务 9 的报告里逐条列过它们。
    ("windows/web/index.html", "数据下载工具"): (2, "窗口标题（`<title>`；`tauri.conf.json` 的 windows[0].title 是同一句）＋ 任务 13 关于窗口里的应用名（`AboutView.swift` 那一行）"),
    ("windows/web/index.html", "文件"): (1, "侧栏分区名（`SidebarSection.title`，macOS 逐字）"),
    ("windows/web/index.html", "传输列表"): (1, "侧栏分区名（同上）"),
    ("windows/web/index.html", "校验结果"): (1, "侧栏分区名（同上）"),
    ("windows/web/index.html", "批次摘要"): (1, "底栏摘要的分组名（`DeliverySummary` 的那一块的标题）"),
    ("windows/web/index.html", "·"): (3, "摘要里的分隔符（`<span>·</span>`，夹在文件数与大小之间）"
                                        " ＋ 文件页底栏的两个（`SelectionBar.swift:51,56` 那两处 Text(\"·\")）"),
    ("windows/web/index.html", "交付码"): (3, "工具栏那一格的静态标签（有生效批次时让位给真码 —— 真码来自 `summary.code`）"
                                                "＋ 空态屏输入框的占位符（`placeholder`）＋ 任务 13 换码面板输入框的占位符（`SwitchDeliverySheet.swift` 的 `TextField(\"交付码\")`）"),
    ("windows/web/index.html", "设置"): (2, "工具栏那颗按钮的名字（`RootView.swift` 的「设置」）＋ 任务 13 设置窗口的标题（`SettingsView.swift` 那一扇窗）"),
    ("windows/web/index.html", "重试"): (4, "常驻提示行 `tpl-notice-retry` 里那颗按钮的字"
                                            "＋ 空态屏失败态那颗「重试」（`empty__retry`）"
                                            "＋ 文件页读失败那格的「重试」（`FileBrowser.swift:208`）"
                                            "＋ **传输列表屏行菜单里的「重试」**（任务 11：`TransferRowView.actionItems` 的 `Button(\"重试\")`，对应 `TaskAction::Retry`）"),
    ("windows/web/index.html", "收起"): (1, "常驻提示行 `tpl-notice-dismiss` 那颗按钮的无障碍名字（`aria-label`）"),
    ("windows/web/index.html", "输入交付码开始下载"): (1, "空态屏的标题（`EmptyState.swift` 逐句）"),
    ("windows/web/index.html", "交付码在交付邮件或交付页链接里（整条链接也可以直接粘进来）"): (1, "空态屏的副标题（同上）"),
    ("windows/web/index.html", "加载"): (2, "空态屏那颗提交按钮的字（同上）＋ 任务 13 换码面板那颗提交按钮（`SwitchDeliverySheet.swift` 的 `Button(\"加载\")`）"),
    ("windows/web/index.html", "高级：自定义下载地址"): (1, "空态屏 `<summary>` 的展开文案（同上）"),
    ("windows/web/index.html", "留空即用默认交付服务器"): (1, "空态屏「高级」里的说明句（同上）"),

    # =====================================================================
    # 类 2：引导期诊断（只允许出现在 `.js` 里；**逐条登记，不许放宽**）
    # =====================================================================
    # 文件：windows/web/js/dom.js —— 「找不到挂载点」
    # 理由：壳自己的 HTML 与 JS 对不上（`<template id>` 丢了 / 屏的模板里没有那个
    #   挂载点）= **程序自己坏了**。这句话在**抛出**与**控制台**之间，客户看不到一句
    #   面向他的文案；而它说的正是"为什么主区是空的"。前端资源不完整时 Rust 够不着。
    ("windows/web/js/dom.js", "里没有（前端资源不完整？）"): (1, "诊断①：`template()` 找不到 `<template id>`（抛出）"),
    ("windows/web/js/dom.js", "里"): (1, "诊断①的调用点：`byId()` 说的是「**index.html 里**没有 #…」"),
    ("windows/web/js/dom.js", "这一屏的结构里"): (1, "诊断①的调用点：`byIdIn()` 说的是「**这一屏的结构里**没有 #…」（换屏时新屏还在文档外，必须用 root 查）。⚠️ 原文是「这一屏的模**板**里」—— 任务 10b 改的：那个「板」不在内嵌字体子集里，而这句话**会渲染**（进常驻提示行）⇒ 改词而不是放宽判据；理由逐条写在 `dom.js:byIdIn()` 上面那段注释里"),
    ("windows/web/js/dom.js", "没有这个挂载点（前端资源不完整？）"): (1, "诊断①的正文：`requireEl()` 抛出那句「…没有 #id 这个挂载点」"),
    # 文件：windows/web/js/invoke.js —— 「没有 Tauri」
    # 理由：**这是引导期诊断的标准形态** —— `window.__TAURI__` 不在时请求根本发不出去，
    #   Rust 那边一个字都给不出来，而客户看到的是一个白屏。这句话必须由前端说，
    #   它指名道姓地说出缺的是哪一层（浏览器里打开 vs 宿主漏了 withGlobalTauri），
    #   因为两种"白屏"的补救完全不同。`invoke.js` 头部那条界线是它的出处。
    ("windows/web/js/invoke.js", "没有找到的：不存在。"): (1, "诊断②：`resolveInvoke()` 抛出的「没有找到 Tauri 的 invoke：window.__TAURI__ 不存在。」（会显示在常驻提示行上）"),
    ("windows/web/js/invoke.js", "若这是浏览器里直接打开的页面，那是预期的（前端要在宿主里跑）；"): (1, "诊断②的第二句（同上，同一个字符串）"),
    ("windows/web/js/invoke.js", "若是在宿主里，检查的是否为（缺了它不注入全局对象）。"): (1, "诊断②的第三句：指出 `tauri.conf.json` 的 `app.withGlobalTauri`（同一个字符串）"),
    # 文件：windows/web/js/poll.js —— 「某一拍的 run 自己没接住」
    # 理由：那是**程序缺陷**（正常路径下每个任务的 run 都自己接住失败），
    #   所以它只进控制台、**不进界面**（`poll.js` 的 `_fire` 里写着这条取舍）。
    ("windows/web/js/poll.js", "节拍任务冒出了一个没被接住的错误："): (1, "诊断③：`console.error`（只进控制台，客户看不到）。⚠️ 原文是「轮**询**任务**抛**出了一个没被接住的错误：」—— 任务 10b 改的：那两个不在内嵌字体子集里的字让它在控制台里变成两个方块，而任务 10b 之后 `.js` 也在字体覆盖判据的扫描面内（`test.sh` 第 0.6 步）；理由写在 `poll.js:_fire()` 的注释里"),
    # 文件：windows/web/js/screens/registry.js —— 「这个屏没注册」
    # 理由：注册表少一行（任务 10/11/12 的屏还没落地）⇒ 路由算出来的屏 id 找不到实现。
    #   这时主区会保持旧屏（`app.js:applyRoute`），而**必须有一句话说得出来**为什么，
    #   不然用户看到的是"加载成功之后主区一片空白"。这句话会挂到常驻提示行上。
    ("windows/web/js/screens/registry.js", "前端没有名为「」的屏。"): (1, "诊断④：`screenFor()` 抛出的「前端没有名为「<id>」的屏。」（会显示在常驻提示行上）。⚠️ 原文是「前端没有注**册**名为…」—— 任务 10b 改的：那个「册」不在内嵌字体子集里，而这句话**会渲染**；理由写在 `registry.js:screenFor()` 的注释里"),
    ("windows/web/js/screens/registry.js", "若这是任务的屏，检查的里有没有加它那一行；"): (1, "诊断④的第二句（同一个字符串，指到 SCREENS 注册表）"),
    ("windows/web/js/screens/registry.js", "若这是路由算出来的，检查与是否一致。"): (1, "诊断④的第三句（同一个字符串，指到 routeOfState 与 SECTIONS 是否一致）"),
    ("windows/web/index.html", "下载本目录"): (1, "文件页：面包屑右侧那颗「下载本目录」（`FileBrowser.swift:119` 的 `Label`，图标 + 文字，不许只留图标）"),
    ("windows/web/index.html", "重新读取这一层"): (2, "文件页：重读那颗纯图标按钮的 tooltip 与无障碍名字（同一个字符串占两格，`FileBrowser.swift:132` 的 `.help`）"),
    ("windows/web/index.html", "收起这条提示"): (2, "文件页：目录读失败那条说明右上角那颗 ×（tooltip + 无障碍名字，`FileBrowser.swift:182` 的 `.help`）"),
    ("windows/web/index.html", "名称"): (1, "文件页：四列的第一个列名（`FileBrowser.swift:284`）"),
    ("windows/web/index.html", "大小"): (1, "文件页：第二个列名（`FileBrowser.swift:286`）"),
    ("windows/web/index.html", "时间"): (1, "文件页：第三个列名（`FileBrowser.swift:288`）"),
    ("windows/web/index.html", "状态"): (1, "文件页：第四个列名（`FileBrowser.swift:290`）"),
    ("windows/web/index.html", "正在读取目录…"): (1, "文件页：读取中那一态（`FileBrowser.swift:196`）"),
    ("windows/web/index.html", "这一层没能读出来"): (1, "文件页：读失败那一态（`FileBrowser.swift:205`）"),
    ("windows/web/index.html", "这个目录是空的"): (1, "文件页：空目录那一态（`FileBrowser.swift:216`）"),
    ("windows/web/index.html", "全选"): (1, "文件页：底栏那颗「全选」（`SelectionBar.swift:66`）"),
    ("windows/web/index.html", "全不选"): (2, "文件页：底栏那颗「全不选」（`SelectionBar.swift:69`）＋ 右键菜单里同名的那一项（`FileBrowser.swift:250`）"),
    ("windows/web/index.html", "下载选中项"): (1, "文件页：右键菜单第一项（`FileBrowser.swift:247`）"),
    ("windows/web/index.html", "全选本层"): (1, "文件页：右键菜单第二项（`FileBrowser.swift:249`）"),
    ("windows/web/index.html", "总进度"): (1, "校验页顶部那一行的**分组名**（`VerifyView.swift:47` 的 `Text(\"总进度\")`）：它标注的是下面那根进度条，本身不含任何数据"),
    ("windows/web/index.html", "—"): (1, "总进度**没有归属**时（树还没到 / 不是这一批）的占位符（`VerifyView.swift:69`）。⚠️ 它是**结构**不是数据：真正的数字全部由 `ProgressSummary` 给，连它自己的「—」也是 Rust 给的（`percent_text` 在总量为 0 时）；这一格只在\"连 ProgressSummary 都没有\"时出现"),
    ("windows/web/index.html", "刷新"): (2, "按钮上的字（`VerifyView.swift:79` 那颗，**逐字**对上游）。它与数据无关、任何状态下都一样。⚠️ 它曾经被写成「重新取一次」（那时 `刷` U+5237 不在子集里）—— 控制者裁定那是因果倒置，已连同一次单独的资产刷新改回来（§12）"
                                           "＋ **传输列表屏顶部那条栏上的「刷新」**（任务 11：`TransfersView.swift:79-81` 的 `Label(\"刷新\", systemImage: \"arrow.clockwise\")`，与校验页那颗**逐字同名**）"),
    ("windows/web/index.html", "还没有校验结果"): (1, "还没拿到第一份校验结果时的标题句（`VerifyView.swift:109`）：任何状态下措辞都一样，不随数据变"),
    ("windows/web/index.html", "加载另一个交付码"): (1, "换码面板的标题（`SwitchDeliverySheet.swift` 的 `Text(\"加载另一个交付码\")`）"),
    ("windows/web/index.html", "换码会把当前的下载任务从引擎里摘掉（含等待中的）；已经下载到磁盘的文件不会被删除。"): (1, "换码面板里那句**后果说明**（同上）。⚠️ 它是**壳自己写的**：内核不会为一次还没发生的操作主动说话，而这句话必须在用户点「加载」之前就摆在屏幕上"),
    ("windows/web/index.html", "历史批次"): (1, "历史列表那一组的分组名（`SwitchDeliverySheet.swift` 的 `Text(\"历史批次\")`）"),
    ("windows/web/index.html", "点一行即可切换到那一批；备注写在这里，重启应用后仍在。"): (1, "历史列表底下那句用法说明（同上）"),
    ("windows/web/index.html", "取消"): (2, "换码面板那颗「取消」（同上，**有意不禁用**）＋ 改下载目录确认框里那颗「取消」（`SettingsView.swift` 的 `.alert`）"),
    ("windows/web/index.html", "备注"): (1, "历史行里那个备注输入框的**占位符**（`SwitchDeliverySheet.swift` 的 `TextField(\"备注\")`）"),
    ("windows/web/index.html", "关闭"): (3, "设置窗口、关于窗口、许可窗口各一颗 × 的**无障碍名字**（`LicenseView.swift` 的 `Button(\"关闭\")` 与两扇窗的同一颗）"),
    ("windows/web/index.html", "下载到"): (1, "下载目录那一段的标签（`SettingsView.swift` 的 `LabeledContent(\"下载到\")`）"),
    ("windows/web/index.html", "文件夹路径"): (1, "下载目录那个输入框的占位符。⚠️ macOS 那边这一格是 `NSOpenPanel`（选文件夹），本代**没有**原生目录选择器（见任务 13 报告的偏离 ①）—— 提示词逐字说明要填的是什么"),
    ("windows/web/index.html", "选择…"): (1, "下载目录那颗「选择…」（对位 `SettingsView.swift:230` 的 `choose()`）。⚠️ 任务 13 报告里那条「本代没有原生目录选择器」的偏离**已经兑现**（2026-09-20 真机反馈）：原生对话框由 `shell-win/src/pickdir.rs` 弹，它自己顶上那句话在 `api::preferences::picker_title`（**这里扫不到**，那是系统对话框）"),
    ("windows/web/index.html", "应用"): (1, "下载目录那颗提交按钮（同上）"),
    ("windows/web/index.html", "恢复默认"): (1, "下载目录那颗「恢复默认」（同上：空串 = 回到未配置，同时会重启内核）"),
    ("windows/web/index.html", "内核还没交出参数"): (1, "参数表还没拿到时那句标题（`SettingsView.swift` 的 `Text(\"内核还没有交出参数面板\")`）。⚠️ **原文照登**：下面那一格是内核/宿主给的失败原文，本文件不加工"),
    ("windows/web/index.html", "关于"): (2, "设置窗口底栏那颗入口（webview 里**没有应用菜单**，macOS 那一边它住在菜单栏）＋ 关于窗口自己的标题"),
    ("windows/web/index.html", "开源许可…"): (1, "设置窗口底栏那颗许可入口（`SettingsView.swift` 的 `Button(\"开源许可…\")`，含那个省略号）"),
    ("windows/web/index.html", "保存"): (1, "参数面板那颗「保存」（同上，主按钮）"),
    ("windows/web/index.html", "这个文件夹不能用"): (1, "目录不可用那个提示框的标题（`SettingsView.swift` 的 `alertTitle` 第一支）"),
    ("windows/web/index.html", "知道了"): (1, "那个提示框唯一那颗按钮（同上，`Button(\"知道了\")`）"),
    ("windows/web/index.html", "更改下载目录？"): (1, "确认框的标题（同上，`alertTitle` 第二支）"),
    ("windows/web/index.html", "更改并重启内核"): (1, "确认框那颗确认按钮（同上）。⚠️ 它的标题把**后果**再说一遍，而不是一句空泛的「确定」"),
    ("windows/web/index.html", "开源许可"): (1, "许可全文窗口的标题（`LicenseView.swift` 的 `Text(\"开源许可\")`）。两处条目的名字来自载荷（`License.name`），这一格不是"),
    ("windows/web/index.html", "全文"): (1, "sha256 那一行的小标签（`licenses.rs` 把那个值**发出来**就是为了让它真的出现在界面上：用户能拿它去跟官方发布件核对）"),
    ("windows/web/index.html", "清空已完成"): (1, "传输列表屏：顶部那颗**列表级**动作（`TransfersView.summaryBar` 的 `Label(\"清空已完成\")`）。它没有 gid、不属于任何一行 —— `TransferRow::actions` 的注释里写着为什么它不许出现在行级菜单里"),
    ("windows/web/index.html", "已暂停"): (1, "传输列表屏：行首那颗角标（`TransferRowView.titleLine` 的 `Text(\"已暂停\")`）。⚠️ 它与数据有关的是**显隐**（`shows_paused_badge`，判据只能来自 `raw_status`，裁决 #88），而这两个字本身与数据无关 ⇒ 字住这里、显隐由 JS 切"),
    ("windows/web/index.html", "这一行的操作"): (1, "传输列表屏：行尾那颗「⋯」的**无障碍名字**（`TransferRowView.actionsMenu` 的 `.help`）。那颗按钮本身是运行时才出现的（从行模板克隆），所以字必须跟着模板走"),
    ("windows/web/index.html", "暂停"): (1, "传输列表屏：行菜单第一项（`TransferRowView.actionItems` 的 `Button(\"暂停\")`）"),
    ("windows/web/index.html", "继续"): (1, "传输列表屏：行菜单第二项（同上，`Button(\"继续\")`，对应 `TaskAction::Unpause`）"),
    ("windows/web/index.html", "在资源管理器中显示"): (1, "传输列表屏：行菜单里的「在资源管理器中显示」（macOS 是 `Button(\"在访达中显示\")`；Windows 的对位写法见设计规格 §9）。⚠️ 这一项**不是** `TaskAction`（协议里没有 `reveal`，规格 §5.2：那是壳的事）"),
    ("windows/web/index.html", "移除"): (1, "传输列表屏：行菜单最后一项（`TransferRowView.actionItems` 的 `Button(\"移除\", role: .destructive)`）"),
    ("windows/web/index.html", "正在读取传输列表…"): (1, "传输列表屏：**还没有快照**那一档那一句话（`TransfersView.swift:113-121` 的 `Text(\"正在读取传输列表…\")`，逐字）。⚠️ 它与数据无关、任何状态下都一样（转圈那一档永远是这一句），转圈那一颗是纯 CSS ⇒ 按 §3.2 它是**结构文案**，住 HTML、JS 只切 `hidden`"),
}

# 缺口清单在报错里最多逐条打印这么多处（再多就把真信息淹了；总数仍写在标题里）。
SHOW_LIMIT = 30

# 给"未登记指纹"的读者一条**可执行**的出路：`shell-core/src/presentation/` 下现有的家。
# ⚠️ 这张表**不是**判据（判据是"登记 / 不登记"），它只是一句话的地图 ——
#    但缺了它，"这句该搬到哪"要靠人翻目录，而那正是"算了，先这样吧"的开始。
PRESENTATION_HOMES = """\
      status 文案 / 引擎 / 图标名   presentation/engine_status.rs（EngineStatusPresentation）
      错误原文（逐字透传）          presentation/error_text.rs
      大小 / 时间 / 百分比 / 速度    presentation/format.rs
      批次摘要                      presentation/delivery_summary.rs（DeliverySummary）
      常驻提示行                    presentation/resident_notice.rs
      传输行（进度/状态/动作面）      presentation/transfer_row.rs
      校验结果（恒六类）            presentation/verify_summary.rs
      文件页的行 / 面包屑 / 主按钮    presentation/browser_row.rs / breadcrumb.rs / browser_primary_action.rs
      下载按钮标题                  presentation/download_targets.rs
      设置 / 偏好 / 历史 / 关于       presentation/{settings_form,app_preferences,batch_history,about_info}.rs
      ⚠️ 上面**没有**对应物时（比如空态页那几句）：§3.2 的推论是"先把那个模块补齐，
         再让 JS 摆位置"（`presentation/` 缺的模块必须先补齐，前端才能开工）——
         **不是**"那就写在 JS 里吧"。"""


def collect_files():
    """`windows/web/` 下的**全部**文件（仓库相对路径，排序）。"""
    out = []
    for dirpath, dirnames, filenames in os.walk(WEB):
        dirnames.sort()
        for fn in sorted(filenames):
            full = os.path.join(dirpath, fn)
            out.append(os.path.relpath(full, REPO).replace(os.sep, "/"))
    return sorted(out)


def scan_offenders(expected_files):
    """返回 (逐处的未登记项, 每个指纹的**出现处数**, 每个指纹出现的行号, 扫描的文件数)。

    ⚠️ "指纹"= 一行里剥掉注释后的非 ASCII 字符按顺序拼起来的那串（见本脚本头部）。
    ⚠️ **按"处"数**（一行 = 一处），不按"见过没有" —— 同一个指纹出现两次就是两处，
       而白名单登记的是**处数**（见头部"登记 = 处数"那一段）。
    """
    unknown, counts, sites, scanned = [], collections.Counter(), {}, 0
    for rel in sorted(expected_files):
        if os.path.splitext(rel)[1].lower() not in SCAN_EXTENSIONS:
            continue
        path = os.path.join(REPO, rel)
        if not os.path.isfile(path):
            continue          # 缺席已由判据①报过，这里不重复报
        scanned += 1
        with open(path, encoding="utf-8") as fh:
            source = fh.read()
        original = source.splitlines()
        # ⚠️ **在整份源码上剥一次，再逐行取指纹** —— 剥完的文本与原文**逐行一一对应**
        #    （注释被换成等量换行），所以下面数出来的行号就是原文的行号。
        #    按行剥会漏掉跨行块注释（`check_design_fonts.py:blank_comments` 里那个坑）。
        for lineno, line in enumerate(cdf.rendered_text(path, source).splitlines(), start=1):
            fp = "".join(ch for ch in line if ord(ch) > 127)
            if not fp:
                continue
            raw = original[lineno - 1].strip() if lineno <= len(original) else ""
            # ⚠️ 行号只用来**给人看**（报错里指出"几处在哪"），**不用来判**（见头部）。
            sites.setdefault((rel, fp), []).append((lineno, raw))
            counts[(rel, fp)] += 1
            if (rel, fp) not in WHITELIST:
                unknown.append((rel, lineno, fp, raw))
    return unknown, counts, sites, scanned


def main():
    actual = collect_files()
    expected = sorted(rel for rel, _ in EXPECTED_FILES)
    # 判据①：两个方向都判（多出来的、以及登记了却不在场的）
    unexpected = [p for p in actual if p not in expected]
    missing = [p for p in expected if p not in actual]
    unknown, counts, sites, scanned = scan_offenders(expected)
    twins = twin_problems()
    # 判据②的两向闭环（**按处数**，见头部"登记 = 处数"）：
    #   · 多于期望 ⇒ 出现了新的一处（守卫真正要抓的那件事）；
    #   · 少于期望 ⇒ 登记烂了（这句话少了一处，或计数过时了）。
    surplus, shortfall = [], []
    for key in WHITELIST:
        want = WHITELIST[key][0]
        got = counts.get(key, 0)
        if got > want:
            surplus.append((key[0], key[1], want, got))
        elif got < want:
            shortfall.append((key[0], key[1], want, got))
    n_sites = sum(want for want, _why in WHITELIST.values())

    if unexpected or missing:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print("文件清单判据没过：`windows/web/` 下的文件与登记表不一致。", file=sys.stderr)
        for p in unexpected:
            print(f"    ✗ 未登记：{p}", file=sys.stderr)
        for p in missing:
            print(f"    ✗ 登记了、但现场没有：{p}", file=sys.stderr)
        print("  为什么这条判据存在：`shell-win/tauri.conf.json` 的", file=sys.stderr)
        print("  `build.frontendDist = \"../web\"` ⇒ **这个目录下的任何文件都会被编进 exe**。", file=sys.stderr)
        print("  一个\"只是测试用的\"\"顺手放的\"文件会**悄悄发给客户**——", file=sys.stderr)
        print("  任务 9 的受控 stub 就是因此做完就删的（它的常驻落点在", file=sys.stderr)
        print("  `windows/scripts/frontend-stub/`，那个位置不进产物）。", file=sys.stderr)
        print("  补救（二选一）：", file=sys.stderr)
        print("    · 它本来就该进交付物 ⇒ 在 `check_frontend_copy.sh` 的 EXPECTED_FILES 里", file=sys.stderr)
        print("      登记它（**一次能被审查的决定**）；", file=sys.stderr)
        print("    · 它是测试/临时文件 ⇒ 搬出 `windows/web/`。", file=sys.stderr)

    if unknown:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print(f"§3.2 判据没过：{len(unknown)} 处**白名单之外**的非 ASCII 文案。", file=sys.stderr)
        for rel, lineno, fp, raw in unknown[:SHOW_LIMIT]:
            ext = os.path.splitext(rel)[1].lower()
            print(f"", file=sys.stderr)
            print(f"    {rel}:{lineno}", file=sys.stderr)
            print(f"        指纹：「{fp}」", file=sys.stderr)
            print(f"        源码：{raw[:110]}", file=sys.stderr)
            # ⚠️ 话术**按文件类型分叉**：`.html` 的越界与 `.js` 的越界不是同一件事，
            #    用同一句话说会把读者引向错的补救（"把它搬进 presentation"对一份
            #    **结构文案**来说是错的方向 —— 那些字 macOS 侧同样住在视图文件里）。
            if ext == ".html":
                print(f"        它出现在 HTML 里。两份可能，**性质完全不同**：", file=sys.stderr)
                print(f"          ① 它是**结构文案**（与数据无关、任何状态下都一样，浏览器直接渲染、", file=sys.stderr)
                print(f"             JS 只克隆与摆放）⇒ 它**可以**住在 HTML 里，但必须**逐条登记**；", file=sys.stderr)
                print(f"             登记时你要回答的是\"**为什么它不该由 Rust 给**\"", file=sys.stderr)
                print(f"             （`index.html` 头部那段论证就是范例：`presentation/` 里没有这些字的", file=sys.stderr)
                print(f"             模块，而 macOS 侧它们同样住在 `Sidebar.swift` / `EmptyState.swift` 里）。", file=sys.stderr)
                print(f"          ② 它是**数据文案**（状态 / 大小 / 时间 / 百分比 / 错误原文 / 空态提示 /", file=sys.stderr)
                print(f"             按钮标题）⇒ §3.2 明写这一格**必须**由 presentation 给，", file=sys.stderr)
                print(f"             **它不该出现在这里** —— 哪怕它看起来只是\"一句静态的字\"。", file=sys.stderr)
            elif ext == ".css":
                print(f"        它出现在 CSS 里：CSS 的非 ASCII 只在 `content:` 这类**会渲染出字**的", file=sys.stderr)
                print(f"        地方才出现 —— 那与 JS 拼串是同一性质的东西（会走到客户眼前的一句话），", file=sys.stderr)
                print(f"        所以归 §3.2 管。", file=sys.stderr)
            else:
                print(f"        它属于什么：**面向用户的字符串**（不是\"程序自己坏了\"那一类诊断）。", file=sys.stderr)
            print(f"        为什么违反 §3.2：JS **不拼接**任何面向用户的字符串——状态文案、颜色档位、", file=sys.stderr)
            print(f"            图标名、百分比、大小、时间、错误原文、空态提示、按钮标题，全部由", file=sys.stderr)
            print(f"            `shell-core::presentation` 算好、随命令的信封下来；**JS 只负责摆位置**。", file=sys.stderr)
            print(f"            这句话写在这里，就从这一刻起与 macOS 的 `presentation/` 各走各的了", file=sys.stderr)
            print(f"            （而\"文案自动与 macOS 一致\"正是这条纪律存在的全部理由）。", file=sys.stderr)
            print(f"        这个字符串应该由哪个 presentation 类型提供：", file=sys.stderr)
            print(PRESENTATION_HOMES, file=sys.stderr)
        if len(unknown) > SHOW_LIMIT:
            print(f"    …（其余 {len(unknown) - SHOW_LIMIT} 处同类，从略）", file=sys.stderr)
        print("", file=sys.stderr)
        print("  ⚠️ **登记进白名单不是默认出路**：只有两类是合法的（见本脚本头部）——", file=sys.stderr)
        print("     「结构文案」（只可能出现在 `.html` 里，浏览器直接渲染）与", file=sys.stderr)
        print("     「引导期诊断」（只允许出现在 `.js` 里，且**逐条**登记、**不许**放宽成\"诊断都行\"）。", file=sys.stderr)
        print("     一句面向用户的文案写成 `throw` / `console.error` 的形状**不改变它的性质**。", file=sys.stderr)

    if surplus:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print(f"处数判据没过：有 {len(surplus)} 条登记**比源码里的处数少** ——"
              " 这个文件里出现了**新的一处**同一句话。", file=sys.stderr)
        for rel, fp, want, got in surplus:
            print(f"", file=sys.stderr)
            print(f"    ✗ {rel}：「{fp}」—— 登记 {want} 处，源码里有 **{got} 处**", file=sys.stderr)
            print(f"        这一句在源码里的 {got} 处（按出现顺序，**哪一处是新的由你来判**）：", file=sys.stderr)
            for i, (lineno, raw) in enumerate(sites.get((rel, fp), []), start=1):
                print(f"          {i}) :{lineno}  {raw[:100]}", file=sys.stderr)
        print("", file=sys.stderr)
        print("  补救（二选一）：", file=sys.stderr)
        print("    · 新增的那一处**合法**（`结构文案` / `引导期诊断`，见本脚本头部）", file=sys.stderr)
        print("      ⇒ 把这条的期望处数改成实际处数，**并把新增那一处的理由补进这条的理由里**", file=sys.stderr)
        print("      ——把数字改大**不是**理由，那只是把同一句话再说一遍；", file=sys.stderr)
        print("    · 它其实是**数据文案**（状态/大小/时间/百分比/错误原文/空态提示/按钮标题）", file=sys.stderr)
        print("      ⇒ 它该由 `presentation` 给：按上面 §3.2 那段改文案，别再登记。", file=sys.stderr)
        print("  ⚠️ 这条判据存在的理由：`文件` `重试` `设置` `加载` 这类词**单独看都是正常界面词**", file=sys.stderr)
        print("     —— 它们第二次、第三次出现时，**必须**是一次显式登记（见本脚本头部\"登记 = 处数\"）。", file=sys.stderr)

    if shortfall:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print(f"处数判据没过：有 {len(shortfall)} 条登记**比源码里的处数多** ——"
              " 登记的那一处（或几处）已经不在源码里了。", file=sys.stderr)
        for rel, fp, want, got in shortfall:
            print(f"    ✗ {rel}：「{fp}」—— 登记 {want} 处，源码里只有 **{got} 处**", file=sys.stderr)
            for i, (lineno, raw) in enumerate(sites.get((rel, fp), []), start=1):
                print(f"        {i}) :{lineno}  {raw[:100]}", file=sys.stderr)
        print("  补救：把这条的期望处数改成实际处数（并核对理由里写的那几处还成不成立）；", file=sys.stderr)
        print("  或者把被删掉的那句话改回来。**别让它烂在清单里**：清单里有一条对不上的登记，", file=sys.stderr)
        print("  下一条登记的\"理由\"就没人读了。", file=sys.stderr)

    if twins:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print(f"交付副本判据没过：有 {len(twins)} 份副本与它的源**不是逐字节相同**。",
              file=sys.stderr)
        for dst, src, why in twins:
            print(f"    ✗ {dst}", file=sys.stderr)
            print(f"        源：{src}", file=sys.stderr)
            print(f"        {why}", file=sys.stderr)
        print("  为什么必须有这条：`windows/web/` 下的文件**都会进 exe**，而这几份是", file=sys.stderr)
        print("  **入库的副本** —— 换了源而忘了重拷的后果是「界面上悄悄换回旧的」，", file=sys.stderr)
        print("  而文件清单、字体覆盖、文案三批判据**全都不会响**。", file=sys.stderr)
        print("  补救：重跑生成源的那一步，然后把源文件拷成副本（**两份必须逐字节相同**）：", file=sys.stderr)
        print("      python3 macos/scripts/make_brand_assets.py     # 只在换品牌资产时跑", file=sys.stderr)
        print("      cp macos/Resources/benagen-mark.png      windows/web/img/benagen-mark.png", file=sys.stderr)
        print("      cp macos/Resources/benagen-full-logo.png windows/web/img/benagen-full-logo.png", file=sys.stderr)

    if unexpected or missing or unknown or surplus or shortfall or twins:
        return 3

    print(f"文件清单：{len(expected)} 个文件，与登记表一致"
          f"（`windows/web/` 下没有未登记的文件 —— 这一条守的是\"别把测试代码发给客户\"）")
    # ⚠️ 同理：这个数**现算**，不写死（见下面那段"报出来的数是算出来的"）。
    print(f"交付副本：{len(TWIN_FILES)} 份与各自的源逐字节相同"
          f"（{', '.join(dst for dst, _src, _why in TWIN_FILES)}）")
    # ⚠️ 报出来的数**是算出来的**、不是写死的：走到这里 `unknown` 与两向的处数差都必然为空
    #    （非空已经在上面 `return 3` 了），但把 `len(unknown)` 现算一遍，
    #    是为了让这条输出在"判据改了而这里忘了改"时**自己会露馅**（同 `test.sh` 第 3 步
    #    从日志里数 `running N tests` 而不写死那个数）。
    print(f"非 ASCII 文案：扫了 {scanned} 个文件（{'/'.join(SCAN_EXTENSIONS)}），"
          f"{len(WHITELIST)} 条登记、**{n_sites} 处**全部命中（处数逐条相符）、"
          f"{len(unknown)} 处未登记")
    print("§3.2 判据通过：`windows/web/` 里没有一个白名单之外的非 ASCII 字符 ——"
          " 界面文案仍然全部由 Rust 决定。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
PY

note "扫描 windows/web/（文件清单 + §3.2 的非 ASCII 文案）"
python3 "$helper" "$REPO"
