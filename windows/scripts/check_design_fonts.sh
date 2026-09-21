#!/usr/bin/env bash
#
# windows/scripts/check_design_fonts.sh —— **前端文件的字体覆盖判据**。
#
#    bash windows/scripts/check_design_fonts.sh --selftest        # 自检（造夹具，必须全绿）
#    bash windows/scripts/check_design_fonts.sh <文件…>            # 判据（有缺字就红）
#    bash windows/scripts/check_design_fonts.sh --chars <文件…>    # 只抽字（每行一个 U+XXXX）
#                                                                # —— 给生成模式用，见下面
#
# ---------------------------------------------------------------------------
# 它补的是哪一块（**别与 make_font_subset.sh 混起来读**）
# ---------------------------------------------------------------------------
#   `make_font_subset.sh --check` 判的是："**Rust 源码字面量**里会被渲染的字，
#   内嵌子集都覆盖吗"。它的扫描面写死在 `SOURCE_ROOTS`（三棵 `.rs` 树）。
#
#   阶段 A0 起，界面文案开始出现在 **HTML/CSS/JS** 里 —— 那不在它的扫描面内。
#   于是"样张/前端里有个字不在子集里"会以**豆腐块**的形式出现，而
#   编译、测试、构建全都过得去。本脚本就是那道缺失的网。
#
#   ⚠️ 两个脚本**各读一次 cmap 是可以接受的**，不是重复实现：那个脚本手写了一份
#      sfnt/cmap 解析器，唯一的理由是"`--check` 要能在没有 fontTools 的机器上跑"。
#      本脚本是**设计期/开发期工具**，直接用 fontTools（一行 `getBestCmap()`）——
#      而 fontTools 本来就是生成子集的硬前置（`make_font_subset.sh` 第 0 步 check 它）。
#      **不要**为了"统一"而把这里改成抄一份手写解析器。
#
# ---------------------------------------------------------------------------
# 判据（fail-closed）
# ---------------------------------------------------------------------------
#   要求集 = 目标文件里**会被渲染的**非 ASCII 字符，**剥掉注释之后**剩下的那些。
#   覆盖集 = `windows/assets/ui-subset.otf` 的 cmap。
#   缺一个就**硬失败**（退出码 3）并**逐字指名**是哪一个字、在哪个文件。
#
#   ⚠️ 抽不到字比多抽字危险得多：多抽 = 子集要多带几个字（没有风险），
#      少抽 = 界面上出现一块豆腐而**没有任何东西会变红**。所以下面的注释剥离
#      一律**按文件类型**做，宁可少剥、不可多剥：
#        · `.html` 剥 `<!-- -->` **与** `/* */`。
#          ⚠️ 第二条是承重的，不是顺手：样张是**单文件**（规格 §7.4），CSS 内联在
#             `<style>` 里，而那些 CSS 注释**全是中文散文**。不剥它们的话，
#             每加一段说明性注释就会往要求集里塞进几十个新字 ⇒ 判据**天天误报**，
#             而"让守卫被脱敏"正是本仓库最怕的形态（`make_font_subset.sh` 头部
#             记着同一条：实测每个提交约 +4 个新字，几乎全是注释里的散文）。
#          ⚠️ 但 `.html` **不剥 `//`**：HTML 正文/属性里的 `http://` 会被它吃掉半行，
#             而那是"少抽"（不安全的方向）。本样张**零 JS**，内联 `<script>` 不存在；
#             阶段 A 的脚本一律是独立的 `.js`（规格 §6.2：原生 ES modules）。
#        · `.css`  剥 `/* */`。
#        · `.js`   剥 `/* */` 与 `//`。
#      属性值（`href`、`title`）里的字符串**一样算要求集**（它们会显示出来）。
#
#   ⚠️ **本脚本没有 `EXEMPT_CODEPOINTS` 那样的豁免清单，这是刻意的。**
#      `make_font_subset.sh` 需要它，因为内核的**运行期原文**里有 `⚠️`（U+26A0 + U+FE0F），
#      而 U+FE0F 是**变体选择符**：它在任何字体里都没有字形、零宽度，渲染不出豆腐块
#      （那一条的理由逐字写在该脚本的 EXEMPT_CODEPOINTS 里）。
#
#      本脚本守的是**源码字面量**（样张的界面文案），而那条路是**壳自己写的字**——
#      写的时候**不要带变体选择符**就行：`⚠` 与 `⚠️` 在屏幕上完全一样。
#      ⇒ 不设豁免清单，判据保持"要什么就有什么"这一条最简单的形状。
#
#      ⚠️ 反过来说：**内核运行期回传的原文不受本脚本管辖**（它是数据，会随 API 进场，
#      不在这份文件的字面量里）。它与文件名是同一类东西 —— 见
#      `make_font_subset.sh` 头部"子集**不**负责文件名里的生僻字"那一节的分工。
#
#   ⚠️ **页面声明的 `@font-face` 必须真的加载到本脚本 `FONT` 那一份字体**，
#      由 `check_design_fonts.py` 的 `font_disagreements()` 守。
#      理由：这份字体在**两个地方各有一份** —— 本脚本从上面的 `FONT`
#      （`windows/assets/ui-subset.otf`）读它，页面用 `url("./fonts/ui-subset.otf")`
#      加载**交付副本**。**没有任何别的东西校验它们一致**：
#      阶段 A 换字体（比如 `ui-subset-v2.otf`）而只改了一处 ⇒ 判据对着**另一份字体**
#      开火、**照样全绿**，而页面上会出现豆腐块 —— 判据存在的唯一理由被整个绕过，
#      且失败形态与"根本没有判据"一模一样。
#
#   🔴 **判据的锚点 2026-09-20 从"路径相等"换成了"sha256 相等"（任务 18）**，
#      因为旧口径在**交付布局**下是哑的。两条实测读数：
#        · `windows/web/**`（24 个文件）⇒ **退出 3**（交付页自己那条 `./fonts/…`
#          被判成"不一致"——它是对的）；
#        · `test.sh` 的口径（25 个文件，多一个 `windows/design/a0-proof.html`）⇒ **退出 0**，
#          而那唯一的绿灯**来自样张**（`design/` 下只有它一个文件被扫到）。
#      ⇒ 旧判据认可的**不是**交付页的声明，是一个**不随交付物发货**的文件；
#        既不管交付面，又会因样张改动**假红**（样张被明确预期会改）。两个方向都坏。
#      换成 sha256 之后，判交付面的是**交付页自己那条**（交付副本必须与判据那份
#      逐字节相同），并且一次补两个洞 ——"忘了重拷"（交付副本停在上一版子集）
#      从此会红。另加两条收紧：交付面内的声明**不许逃出 `windows/web/`**
#      （webview 的根就是它，`../assets/…` 会 404 ⇒ 静默回退系统字体）、
#      **不许指向不存在的文件**。完整理由与夹具见 `check_design_fonts.py`。
#      ⇒ 判据会**先**查这一条（它比字符覆盖更靠上游：判错了字体，覆盖率说得再对
#        也没有意义），不一致就退出码 3，并逐条打印"声明了什么 ⇒ 解析到哪 ⇒ 那一条
#        算不算数 / 为什么不算数"。
#
# ---------------------------------------------------------------------------
# 退出码
# ---------------------------------------------------------------------------
#   0 = 判据通过（或 --selftest 全过）
#   1 = 环境/工具缺失（W-2：缺工具大声说，并给出可执行的补救）
#   2 = 用法错误（没有给文件、或给了不认识的参数）
#   3 = **判据没过**（有字不在子集里）——与 1/2 的"修环境"不同，补救是"改字或重生成子集"
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FONT="$REPO/windows/assets/ui-subset.otf"

# ⚠️ **补救话术按平台给三行**（macOS / Debian-Ubuntu / Windows）——2026-09-19 的 Windows
#    交接任务改的。本仓库要搬到一台**真 Windows 机器**上继续开发，而那里一句
#    `brew install …` 不但没用、**本身就是错的**；它偏偏又是"缺工具时唯一能看到的东西"，
#    而 W-2 要的是**可执行的**补救。三个平台的补救由 `$3` 带进来（**多行字符串**），
#    下面统一缩进对齐。
#    ⚠️ **判据一个字都没动**：检查的是同一个 `command -v`，失败仍然是这个 `exit 1`。
need() {  # need <tool> <why> <fix>
  command -v "$1" >/dev/null 2>&1 || {
    echo "错误：PATH 里没有 \`$1\` —— $2" >&2
    echo "      补救（按你的平台挑一行）：" >&2
    while IFS= read -r fix_line; do
      echo "        $fix_line" >&2
    done <<< "$3"
    exit 1
  }
}
need python3 "抽取字符与解析字体 cmap" \
             "macOS: 系统自带 python3
Debian/Ubuntu: apt-get install -y python3
Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`（或 python.org 的安装包）
         ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\`；本脚本按 \`python3\` 找解释器，两个名字都要能被找到"
# ⚠️ **`--chars` 模式既不需要字体、也不需要 fontTools** —— 它在**读字体之前**就返回
#    （见下面 `case` 里的说明）。⇒ 这两个前置检查必须排在**认出模式之后**：
#    `make_font_subset.sh` 会拿 `--chars` 去抽前端文件的字，而那一步要在
#    **没有 fontTools 的机器上**也能跑（`make_font_subset.sh --check` 的性质，
#    见那个脚本头部）。写成无条件检查的话，生成模式会被这里挡住。
if [ "${1:-}" != "--chars" ]; then
  if ! python3 -c 'import fontTools' >/dev/null 2>&1; then
    {
      echo "错误：python3 里没有 fontTools —— 本脚本要读字体的 cmap。"
      echo "      当前解释器：$(command -v python3)"
      echo "      补救（按你的平台挑一行）："
      echo "        macOS:          python3 -m pip install --user fonttools"
      echo "                        （或 \`pipx install fonttools\` / \`brew install fonttools\`）"
      echo "        Debian/Ubuntu:  python3 -m pip install --user fonttools"
      echo "                        （或 \`apt-get install -y python3-fonttools\`）"
      echo "        Windows:        先装 MSYS2 与 \`pacman -S mingw-w64-x86_64-python\`，再"
      echo "                        \`python -m pip install --user fonttools\`"
      echo "                        ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\`"
      echo "      （它本来也是 \`make_font_subset.sh\` 生成子集时的硬前置。）"
    } >&2
    exit 1
  fi
  [ -f "$FONT" ] || {
    echo "错误：找不到内嵌子集 ${FONT}。" >&2
    echo "      补救：bash windows/scripts/make_font_subset.sh（生成模式，需联网）" >&2
    exit 1
  }
fi

mode="${1:-}"
case "$mode" in
  --selftest) shift ;;
  # `--chars`：**只抽字、不判**（每行一个 `U+XXXX`）。它是**给生成模式用的**
  # （`make_font_subset.sh` 的要求集要并进前端文件的字，见那里）——
  # 于是"哪些字会渲染"这条规则只有一份实现，两个方向不会分叉。
  # ⚠️ 它**不需要字体**，也不需要 fontTools（`check_design_fonts.py` 在那种模式下
  # 连 `cmap_of` 都不会走到）—— `make_font_subset.sh --check` 是在没有 fontTools
  # 的机器上也要能跑的，这条性质别弄丢。
  --chars) shift ;;
  --*) echo "check_design_fonts.sh: 不认识的参数：${mode}" >&2; exit 2 ;;
  "") echo "check_design_fonts.sh: 没有给文件。" >&2
      echo "用法：bash windows/scripts/check_design_fonts.sh <文件…>" >&2
      echo "      bash windows/scripts/check_design_fonts.sh --selftest" >&2
      exit 2 ;;
esac

# 判据核心：一个 python 程序，两种模式共用（**同一份抽取代码**——生成/校验若各写一份，
# 两者会在某次改动后悄悄分叉，而分叉的表现恰好是"永远绿灯"）。
#
# ⚠️ **只有 `--selftest` 才把 flag 传下去；文件模式一个 flag 都不传。**
#    文件模式下 `$mode` 就是**第一个文件路径**（`mode="${1:-}"`，而且上面那个
#    `case` **只对 `--selftest` 做 `shift`**）—— 再把它传一遍，那个文件会在 targets 里
#    **出现两次**：缺字清单翻倍、文件计数错（"2 个文件"其实只有 1 个）。
#    **退出码仍然是对的，所以不会有假绿** —— 但本项目对"报出来的数就是真的那个数"
#    有纪律（同 `make_font_subset.sh` 头部"报的是并集的大小，不是…的和"那句）。
if [ "$mode" = "--selftest" ]; then
  python3 "$REPO/windows/scripts/check_design_fonts.py" "$FONT" --selftest
elif [ "$mode" = "--chars" ]; then
  # ⚠️ 这个模式**没有字体参数**（见上面 `case` 里的说明）—— 别顺手把 `$FONT` 传进去，
  #    那会被当成**第一个目标文件**去打开。
  python3 "$REPO/windows/scripts/check_design_fonts.py" --chars "$@"
else
  python3 "$REPO/windows/scripts/check_design_fonts.py" "$FONT" "$@"
fi
