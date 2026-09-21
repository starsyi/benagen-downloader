#!/usr/bin/env bash
#
# windows/scripts/make_font_subset.sh —— 生成并校验**内嵌界面字体子集**（规格 W-5；
# 资产与体积的登记在现行规格 §6.3，第 10 步会**当场核对**它）。
#
#    bash windows/scripts/make_font_subset.sh            # 生成/刷新资产（要下载，约 50 MB）
#    bash windows/scripts/make_font_subset.sh --check    # 只校验**已入库的那份**是否还覆盖源码（不联网）
#    bash windows/scripts/make_font_subset.sh --frontend-dirs
#                                                        # 只打印前端文件的那两棵目录
#                                                        # （**仓库相对路径**，每行一个）；
#                                                        # 不联网、不要 fontTools、不写文件。
#                                                        # `test.sh` 第 0.6 步从这里读 ——
#                                                        # 那份清单的唯一属主是本脚本。
#
# ---------------------------------------------------------------------------
# 为什么需要这个脚本
# ---------------------------------------------------------------------------
#   ① egui 自带字体**不含 CJK**，Windows 上没有 macOS 那层 SwiftUI 兜底 ⇒ 不处理就是
#      一片豆腐块（`docs/superpowers/2026-09-18-windows-spike.md` §4.2 的探路实测）。
#   ② W-5：**系统字体优先 + 内嵌子集兜底**，而内嵌的那份**必须是可再分发的字体**。
#      ⇒ 本脚本**只**认 Noto Sans SC（SIL OFL 1.1）这一条路，**没有**"拿本机字体凑一份"
#         的开关。那个开关一旦存在，迟早有人在赶时间时按下它，而产物里就多了一份
#         **不可再分发**的字体（微软雅黑）——那是一次法律问题，且**没有任何测试会发现**
#         （那份子集本身渲染得好好的）。缺开关是刻意的：这条纪律靠结构守，不靠自觉。
#   ③ 字符集**从源码里抽**，不许手写：手写的字符集会在此后任何一次改文案时**静默漏字**，
#      而漏字的后果是界面上出现豆腐块 —— 除了"有人正好盯着那个字"，**没有任何东西会变红**。
#      所以取样、生成、校验走**同一条**抽取路径（本脚本里只有一份抽取代码：helper 的 `required_chars()`）。
#
# ---------------------------------------------------------------------------
# ⚠️ 分工：子集**不**负责文件名里的生僻字（**别把这条当成 bug**）
# ---------------------------------------------------------------------------
#   内嵌子集覆盖的是**壳自己的界面文案用字**。文件名会显示在界面上，而文件名里的字
#   **不受控**（生产数据里有 `C24-8_×_25WS024` 这种），把它们也塞进子集就得把整个
#   CJK 基本区打包进去 —— 那正是"内嵌整份字体"的另一种写法，与本规格的体积目标冲突。
#   ⇒ **分工**：界面文案由内嵌子集保证不豆腐；**文件名里的生僻字交给系统字体**
#     （这也是"系统字体优先"的第一条理由）。所以"子集里查不到某个生僻字"是**设计**。
#   ⇒ 推论（写在这里免得后来者找错地方）：若一台机器上**系统字体也没有**，窗口里的
#     界面文案正常、而文件名里的生僻字会是方块 —— 这是已知的、有意接受的边界，
#     不是本脚本漏抽了字。`fonts.rs` 的 `FontStatus::Embedded` 会把这件事**说出来**。
#
# ---------------------------------------------------------------------------
# 来源登记（**唯一真相**；改版本时连同现行规格 §6.3 的体积登记一起改）
# ---------------------------------------------------------------------------
#   上游        Noto Sans CJK（= Adobe Source Han Sans 的同源发行）
#   发布        https://api.github.com/repos/notofonts/noto-cjk/releases/tags/Sans2.004
#   版本        2.004（2022-01-27 发布；字体自身的 name ID 5 就是这条版本串）
#   资产        18_NotoSansSC.zip（release API 自报 50 077 940 B）
#                 sha256 4d107c09ada479d3e48b6e78c83835773cbd9214bf6e12cdb7b60f8e068292ec
#   取出①       NotoSansSC-Regular.otf（8 331 336 B）
#                 sha256 faa6c9df652116dde789d351359f3d7e5d2285a2b2a1f04a2d7244df706d5ea9
#   取出②       LICENSE（4 301 B，OFL 1.1 全文）—— 落成 windows/assets/OFL-1.1.txt
#                 sha256 6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2
#   本机工具    fonttools 4.62.1（`pyftsubset`）；这个版本只影响**产物的字节**，
#                 不影响覆盖率判据 —— 所以版本记在这里、而**不**把产物的 sha256 当判据：
#                 换一个 fontTools 会让产物字节变、而那条判据会假红（判据要判**性质**，
#                 不是判"上次那串字节"）。
#
# ⚠️ **端点必须带版本号**（`…/releases/tags/Sans2.004`），不许用 `releases/latest`：
#    latest 会随上游发新版而漂移，"来源可复核"就成了一句会自己失效的话。
#    与 `core/scripts/fetch_windows_aria2c.sh` 同一条纪律。
# ⚠️ GitHub 的 release 资产**直连 github.com 在本机不可达**（探路实测），但 api.github.com
#    可达 ⇒ 走**资产端点** `…/releases/assets/<id>` 并带 `Accept: application/octet-stream`，
#    由它自己重定向（aria2 那份脚本记着同一条）。**别浪费轮次去试 github.com 直连。**
#
# ---------------------------------------------------------------------------
# 许可（**分发义务**，与 aria2 的 GPLv2 同一处理方式）
# ---------------------------------------------------------------------------
#   OFL 1.1 的条件 2 要求"每一份拷贝都随附版权声明与本许可"。落地方式：
#     · **本许可全文**：`windows/assets/OFL-1.1.txt` 与 `core/assets/COPYING-OFL-1.1.txt`
#       —— 与官方包内的 `LICENSE` **逐字节相同**（sha256 见上），不在这里二次排版：
#         一旦改了字节，"与官方一致"这件事就只剩一句话、没有证据了。
#     · **版权声明**：随**字体本身**走 —— 子集保留了 name 表（`--name-IDs='*'`），
#       里面 nameID 0 = `© 2014-2021 Adobe`、nameID 13 = OFL 1.1 声明、nameID 14 = OFL 网址。
#       脚本第 8 步（校验产物）会**断言这三条还在**（不然"随附版权"就悄悄地没了）。
#     · **展示入口**：阶段 2 的"开源许可"界面（GPLv2 + 本 OFL 两份）。本任务只保证
#       **文本进包**；⚠️ 单文件分发（W-4）下"进包"= 内嵌进 exe，那一步与打包一起做
#       （任务 22 / 阶段 2），本脚本不假装它已经完成。
#
#   ⚠️ 保留字体名（OFL 条件 3）**不适用**：本字体未声明 Reserved Font Name
#      （nameID 0 只有版权、没有 "with Reserved Font Name"），所以子集沿用
#      `Noto Sans SC` 这个名字是允许的。改字体时**必须重查这一条**——它决定要不要改名。
#
# ---------------------------------------------------------------------------
# 字符集：哪些字必须有（判据 fail-closed）
# ---------------------------------------------------------------------------
#   要求集 = 下面 `SOURCE_ROOTS` 三棵树里**字符串/字符字面量**的非 ASCII 字符
#            ∪ 常用标点与全角区（见 helper 里的 `EXTRA_RANGES`）。
#   取样、生成、校验三者用的是**同一份**抽取代码（helper 里的 `lex()`）。
#
#   ⚠️ **为什么是"字面量"而不是"源码里全部非 ASCII 字符"**（这是一次**有意收窄**，
#      不是漏了 —— W-6 的那类决定，理由如下）：
#     · 会进到屏幕上的字**几乎**全部来自字面量，注释永远不会。**另一类**是**数据驱动的
#       文案**（文件名、内核回传的值）—— 这一类**已知、且已被有意接受**由系统字体兜
#       （上面的"分工"一节），所以它**不在**要求集里。收窄针对的正是"注释散文"这一类。
#     · "全部源码字符"的判据在本仓库**会天天误报**：实测最近 12 个提交，三棵树里的
#       不重复非 ASCII 字符从 1156 涨到 1202（**每个提交约 +4 个新字**，几乎全是注释里
#       的散文）。按前者判 ⇒ 几乎每个提交都要红一次、都要重新生成一份二进制资产
#       （今天这份约 160 KB）—— 那正是"让守卫被脱敏"的经典配方（本项目最怕的形态）。
#     · 收窄**不放过任何真风险**：字面量里出现的新字仍然会红（那才会豆腐）；
#       注释里的新字不会。判据判的是**会被渲染的字**，不是"源码里的所有字节"。
#
#   ⚠️ 三棵树里**包括 `core/src`**：内核的话是**原样显示在界面上**的（规格 §10：壳不改写
#      内核的话）—— 内核报错文案里的字一样会豆腐，所以一样要在要求集里。
#
#   ⚠️ 抽取用的是一个**最小的 Rust 词法**（行注释 `//`、可嵌套块注释 `/* */`、
#      `"…"`（含 `\"` 转义与跨行）、`r#"…"#`、`b"…"`、字符字面量 `'c'`），
#      并且 **fail-closed**：注释与字面量**之外**不允许出现任何非 ASCII 字符 ——
#      出现了就**硬失败**（那说明词法过期了，或有人写了非 ASCII 标识符；两种都要人来判，
#      **绝不能**悄悄少抽几个字）。今天这条检查的读数是 0（`outside_nonascii=0`）。
#
#   附加要求集（`EXTRA_RANGES` / `EXTRA_CODEPOINTS`）：界面上**可能被代码拼出来**、
#   且**这份字体确实全部覆盖**的标点（ASCII、CJK 标点、全角形式、Latin-1 补充区，
#   外加少数逐码位收的常用标点）。⚠️ 它**不是**"所有常用标点"的保证：U+2000–206F、
#   箭头区、装饰符号区这份字体**缺大多数**（实测 78/112、87/112、154/192，`✗`U+2717 也缺），
#   按区间收只会让生成模式对着一堆字体本来就没有的码位硬失败。要真覆盖它们得换字体。
#
#   豁免：`EXEMPT_CODEPOINTS` —— **逐码位**列出的"要求集里有、而这份字体提供不了"的码位，
#   每条都带理由，命中的码位会**逐条打印**（不静默）。⚠️ **不用区间**：区间会把"以为有人
#   渲染得出来"的字一起放行 —— 实测（**读 epaint 0.32.3 随附的四份字体取并集**）
#   U+1F000–U+1FAFF 的 2816 个码位里它们只覆盖 793 个、U+FE00–FE0F 一个都没有，
#   而**四份并集里也没有** U+1F642(🙂)。详见 helper 里的 `EXEMPT_CODEPOINTS`。
#
#   ⚠️ **豁免有两条（2026-09-19 起）**，两条的性质**不同**，别把它们的理由混起来读：
#     · `U+1F642`(🙂) —— 只在一条 `#[test]` 字符串里，**永不进界面**；
#     · `U+FE0F`(VS16) —— **确实进界面**（内核错误原文里的 `⚠️`，`core/src/paths.rs`），
#       理由不是"永不渲染"而是"**它没有字形、零宽度、渲染不出豆腐块**"。
#       ⚠️ 这一条是**变体选择符**：U+FE00–FE0F 这一整段 Noto Sans SC **一个都没有**
#       （fontTools 独立复核过），而它少了只退回文本呈现。逐条理由见 helper。
#
# ---------------------------------------------------------------------------
# 退出码
# ---------------------------------------------------------------------------
#   0 = 成功（生成模式：资产已刷新且自验通过；--check 模式：覆盖判据通过）
#   1 = 环境/工具/IO 失败（含"缺工具"，以及"规格文件不在场"——W-2：这一类的纪律是
#       **大声失败并给出可执行的补救**，**绝不**"缺了就跳过那一步"）
#       ⚠️ 规格文件不在场**属于这一类，不属于 3**：那不是"判据判出来不合格"，
#          是"判据根本没有对象可判"（见第 10 步开头那段）。
#   2 = 下载内容与登记不符（不符即拒，不安装）
#   3 = **判据没过**（同码，因为补救都是"改一处/抄一个数，再重跑"，与 1/2 的"修环境"不同）：
#         · 覆盖缺口：会被渲染的（非豁免）非 ASCII 字符没有全部进子集；
#         · 豁免清单失效：清单里的码位已经被覆盖，或已不在要求集里（见 EXEMPT_CODEPOINTS）；
#         · 身份不符：入库子集的 sha256 与 SUBSET_SHA256 不一致，或名字表里缺版权/OFL 声明；
#         · 许可不符：两份许可全文与官方 LICENSE 不再逐字节相同；
#         · 交付副本不符（任务 18 ①）：`windows/web/fonts/` 下的两份与这里的
#           `windows/assets/` 两份不再逐字节相同（改动时忘了重拷，或副本被手工换过）
#           —— 判据与两条调用点见 `check_delivery_twins()`；
#         · 体积超线：产物超过 `MAX_SUBSET_BYTES` 的 1 MB 硬线（口径要求停下来重新权衡，
#           不是自动放行 —— 它出自**已随那一代方案删掉的** `2026-09-18-windows-client-design.md`
#           §14.1；那条硬线本身没变，只是在这份规格里还没有家，见 §6.3 末尾那条遗留）；
#         · 登记闭环没完成：规格 §6.3 里的体积（两处：字节数 + KB）、
#           或脚本头部的 SUBSET_SHA256 与实物不一致
#           （"文档/常量忘改"要在本机当场发现，别等到客户机器上）。
set -euo pipefail

# ---- 登记（见文件头；唯一真相）---------------------------------------------
FONT_RELEASE_TAG="Sans2.004"
FONT_ASSET_ID="60014263"
FONT_ASSET_NAME="18_NotoSansSC.zip"
FONT_ZIP_SHA256="4d107c09ada479d3e48b6e78c83835773cbd9214bf6e12cdb7b60f8e068292ec"
FONT_ZIP_SIZE="50077940"
FONT_OTF_MEMBER="NotoSansSC-Regular.otf"
FONT_OTF_SHA256="faa6c9df652116dde789d351359f3d7e5d2285a2b2a1f04a2d7244df706d5ea9"
FONT_OTF_SIZE="8331336"
FONT_LICENSE_MEMBER="LICENSE"
FONT_LICENSE_SHA256="6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2"
FONT_LICENSE_SIZE="4301"
FONT_RELEASE_API="https://api.github.com/repos/notofonts/noto-cjk/releases/tags/${FONT_RELEASE_TAG}"

# **产物**的 sha256 —— 只入 `--check`，**不进**生成模式的"下载是否可信"判据（见文件头
# "本机工具"那段：换一个 fontTools 会改产物字节，拿它当下载判据会假红）。
# ⚠️ 它守的是**另一件事**：这份入库的资产**是不是我们登记的那一份**。
#    没有它，`--check` 只证"覆盖率"，那么"有人把 ui-subset.otf 换成一份**从微软雅黑抽的、
#    同样覆盖这些字形**的子集"就能**全绿**过去 —— 而"不许打包不可再分发字体"（W-5）
#    就**没有任何自动化在守**。第 8/9 步在生成模式里做同样的闭环（对不上就退出 3 并打印
#    该抄的那一行，与 `fetch_windows_aria2c.sh` 第 9 步同一手法）。
SUBSET_SHA256="8b2c504a2eda0674ed27d8d70d729325fddda48893107b3a552e437a02f70f80"

# 那条硬线：**超过 1 MB 就停下来重新权衡**（口径出自 2026-09-18 规格 §14.1，那份文件
# 已随那一代方案删除；数字没变，见文件头"登记闭环"与规格 §6.3 末尾的遗留记账）。
MAX_SUBSET_BYTES="1048576"

# 路径全部从**脚本自身**推，与调用者的 cwd 无关（同 `fetch_windows_aria2c.sh`）。
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_FONT="$REPO/windows/assets/ui-subset.otf"
OUT_LICENSE="$REPO/windows/assets/OFL-1.1.txt"
CORE_LICENSE="$REPO/core/assets/COPYING-OFL-1.1.txt"
# 交付副本（`windows/web/fonts/` 下那两份，任务 14 接的）。它们的**唯一真相是上面
# 那两份**，判据与理由见下面的 `check_delivery_twins()`。
WEB_FONT="$REPO/windows/web/fonts/ui-subset.otf"
WEB_LICENSE="$REPO/windows/web/fonts/OFL-1.1.txt"
# 下载缓存：生成一次约 50 MB，而**改文案重跑是常态**（见文件头），不该每次都重下。
# ⚠️ 缓存**不是**真相：命中的缓存仍要过第 2 步的 sha256 核对，不符即弃用并重下。
CACHE_DIR="${BENAGEN_FONT_CACHE:-${TMPDIR:-/tmp}/benagen-font-cache}"

# 要抽字的三棵源码树（相对 REPO）。见文件头"字符集"一节的取舍说明。
SOURCE_ROOTS="windows/shell-core/src windows/shell-win/src core/src"

# 前端文件的两棵目录（相对 REPO）：**它们里面的字也是会被渲染的字**（HTML/CSS/JS 不在
# 任何 `.rs` 字面量里 ⇒ 不并进来的话，那些字永远进不了子集，而判据会一直要它们）。
#
# ⚠️⚠️ **这条清单的唯一属主是本文件**（本计划第八版修订）：`test.sh` 第 0.6 步
#    用 `--frontend-dirs` **从这里读**，不再自己写一份常量。
#    两边各写一份的后果**实测过一次**：`shell-win/src/web/index.html`（一个真会渲染的
#    已交付页面）里有 5 个字不在子集里 ⇒ 它会显示方块，而 `test.sh` 与
#    `build_windows.sh` **两条验收都是绿的**（判据的 glob 只扫 `design/`，
#    而生成模式的要求集只扫 `.rs` —— 两处都不够得着那个文件）。
#    ⇒ **A-2 把界面搬进 `windows/web/` 时，只改这一行**（`test.sh` 会自动跟着走）。
#
# ⚠️ **2026-09-20 改过一次，就是上面那句要做的那一次**（Tauri 那一代的任务 1）：
#    界面的家从 `shell-win/src/web/` 搬到了 **`windows/web/`**（规格 §6.1、R-2：
#    前端与 Rust 源码不同层），原来的那一页随第二代传输层一起删掉
#    （它是本地 HTTP 服务发给系统浏览器的那一页）。`test.sh` 一个字没动 ——
#    它从 `--frontend-dirs` 读，这正是把清单收到本文件里的用意。
#
# 🔴 **扫描面（`required_chars()` 里那条 glob）2026-09-20 又改了两次，理由记账在这里**
#    （任务 10b；与 `test.sh` 第 0.6 步是**同一个洞的两侧**，两处必须一起改）：
#      · ① 原来只扫 `*.html` —— 而界面文案从阶段 A0 起就在 **HTML/CSS/JS** 里
#        （`js/dom.js` 的「这一屏的模**板**里」、`js/screens/registry.js` 的
#        「前端没有注**册**名为「X」的屏。」都是**会渲染的失败提示**，而这两个字
#        都不在子集里 ⇒ 豆腐块，且所有验收全绿）。
#      · ② 原来还是**深度 1** 的 —— 而 CSS 住在 `css/`、JS 住在 `js/` 与 `js/screens/`
#        下 ⇒ 就算补齐扩展名，一个 `js/` 文件也扫不到。
#      · ⇒ 现在是**递归**扫这三种扩展名。
#    ⚠️ **两侧一起改是承重的**：只改 `test.sh` 的话，判据红了以后那句补救
#       （"重跑生成模式"）对着 `.js` 里的字**永远不会生效** —— 补救必须可执行。
FRONTEND_DIRS="windows/design windows/web"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# ---- 参数：只认 --check，别的**直接报错退出**（exit 2）------------------------
# 与 `test.sh` / `build_windows.sh` 同一条纪律：未知参数**不透传**——透传会让脚本
# 以一个"看起来像失败"的退出码收场，而根因是打错了字，那样的红与没有验收等价。
check_only=0
frontend_dirs_only=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check) check_only=1; shift ;;
    --frontend-dirs) frontend_dirs_only=1; shift ;;
    *)
      echo "make_font_subset.sh: 不认识的参数：$1" >&2
      echo "make_font_subset.sh: 只接受 --check（不联网，只校验已入库的子集是否还覆盖源码）" >&2
      echo "                    与 --frontend-dirs（只打印前端文件的那两棵目录）。" >&2
      exit 2
      ;;
  esac
done

# ⚠️ **`--frontend-dirs` 在"创建临时目录"与"工具预检"之前就返回** —— 它只打印一行清单，
#    **不联网、不要 fontTools、不写任何文件**（与 `check_design_fonts.py --chars` 同一性质）。
#    它是那份清单的**唯一出口**：`test.sh` 第 0.6 步从这里读，
#    于是"哪些目录算前端文件"只有一处定义（见 `FRONTEND_DIRS` 上面那段）。
#
# ⚠️ 输出是**仓库相对路径**，每行一个（`test.sh` 会拼上 `$REPO/`）。
if [ "$frontend_dirs_only" -eq 1 ]; then
  # shellcheck disable=SC2086  # FRONTEND_DIRS 就是要按空格拆成多行
  for d in $FRONTEND_DIRS; do
    printf '%s\n' "$d"
  done
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ---------------------------------------------------------------------------
# 0. 工具预检（W-2：缺工具**大声说**并给出**可执行的补救**）
# ---------------------------------------------------------------------------
# ⚠️ `--check` 模式**只需要 python3**：它读的是已入库的子集与源码，不下载、不子集化。
#    （判据必须在**没网、没 fontTools** 的机器上也跑得起来 —— 它要进 `test.sh` 的每一步。）
#
# ⚠️ **补救话术按平台给三行**（macOS / Debian-Ubuntu / Windows）——2026-09-19 的 Windows
#    交接任务改的。理由：本仓库要搬到一台**真 Windows 机器**上继续开发，而那里一句
#    `brew install …` 不但没用、**本身就是错的**；偏偏它又是"缺工具时唯一能看到的东西"，
#    而 W-2 要的是**可执行的**补救。⇒ 三个平台的补救由 `$3` 带进来（**多行字符串**），
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
need python3 "抽取字符集与解析字体的 cmap（生成与 --check 都要）" \
             "macOS: 系统自带 python3
Debian/Ubuntu: apt-get install -y python3
Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`（或 python.org 的安装包）
         ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\`；本仓库的脚本按 \`python3\` 找解释器，两个名字都要能被找到"
if [ "$check_only" -eq 0 ]; then
  need curl    "下载发布资产"                    "macOS: 系统自带
Debian/Ubuntu: apt-get install -y curl
Windows: Git for Windows 自带（/usr/bin/curl）；MSYS2 里是 \`pacman -S curl\`"
  need unzip   "解开资产包"                      "macOS: 系统自带
Debian/Ubuntu: apt-get install -y unzip
Windows: MSYS2 的 \`pacman -S unzip\`（装完在 C:\\msys64\\usr\\bin\\ 里）"
  need shasum  "算 sha256（其一，与 openssl 互核）" "macOS: 随 Perl 分发（系统自带）
Debian/Ubuntu: apt-get install -y perl
Windows: 随 Git for Windows 里的 Perl 一起来（/usr/bin/shasum）；MSYS2 里是 \`pacman -S perl\`"
  need openssl "算 sha256（其二，独立工具）"     "macOS: 系统自带（LibreSSL）
Debian/Ubuntu: apt-get install -y openssl
Windows: Git for Windows 自带（/usr/bin/openssl）；MSYS2 里是 \`pacman -S openssl\`"
  # `pyftsubset` 缺了**不静默降级**（W-2）：拿不到的补救要**可执行**，所以下面把
  # "装到用户目录"的确切命令写出来，而不是说一句"请安装 fonttools"。
  if ! command -v pyftsubset >/dev/null 2>&1; then
    {
      echo "错误：PATH 里没有 \`pyftsubset\` —— 生成子集要用它（fontTools）。"
      echo "      当前解释器：$(command -v python3)"
      echo "      补救（按你的平台挑一行）："
      echo "        macOS:          python3 -m pip install --user fonttools"
      echo "                        （或 \`pipx install fonttools\` / \`brew install fonttools\`）"
      echo "        Debian/Ubuntu:  python3 -m pip install --user fonttools"
      echo "                        （或 \`apt-get install -y python3-fonttools\`）"
      echo "        Windows:        先装 MSYS2 与 \`pacman -S mingw-w64-x86_64-python\`，再"
      echo "                        \`python -m pip install --user fonttools\`"
      echo "                        ⚠️ Windows 上有时只有 \`python\` 没有 \`python3\` —— 上面那条 pip 用 \`python\` 打头"
      echo "      装完确认：pyftsubset --version   （或 python3 -m fontTools.subset --help）"
    } >&2
    exit 1
  fi
fi

# ---------------------------------------------------------------------------
# 抽取/解析 helper（写成一个文件，两种模式共用**同一份**代码）
# ---------------------------------------------------------------------------
# ⚠️ 一份代码、两个入口，是刻意的：生成与校验若各写一份抽取逻辑，两者会在某次改动后
#    悄悄分叉，而分叉的表现恰好是"生成时抽了一组字、校验时比另一组字 ⇒ 永远绿灯"。
helper="$WORK/fontchars.py"
cat > "$helper" <<'PY'
"""字符集抽取、字体 cmap 解析、覆盖率比对 —— `make_font_subset.sh` 的唯一实现。"""
import glob
import os
import re
import struct
import subprocess
import sys

# ⚠️ **前端文件的目录清单不在这里定义**：它的唯一属主是 `make_font_subset.sh` 的
#    `FRONTEND_DIRS`（bash 侧那一个），由命令行 `--frontend <目录…>` 传进来。
#    这里再写一份常量就等于又有了两个属主 —— 而"两处常量分叉"正是本脚本这一段的由来。
#
# ⚠️ **抽取规则也不在这里重写一份**：用 `check_design_fonts.py --chars`（那份唯一实现，
#    它连 fontTools 都不需要）。

# 附加要求集（**不是**"所有常用标点"的保证，取舍见文件头"字符集"一节）：
#   · 区间：界面上**可能被代码拼出来**、且**这份字体确实全部覆盖**的那几个区。
#     （`（{} KB）` 这类由代码拼的串，其括号不一定出现在源码字面量里。）
#   · 逐码位：常用标点里**字体确实有**、而它的 Unicode 区**大多数成员字体没有**的那几个。
#     ⚠️ 不收整个区间的理由：U+2000–206F / U+2190–21FF / U+2700–27BF 这份字体分别缺
#     78/112、87/112、154/192（`✗`U+2717 也缺）⇒ 按区间收 = 生成模式对着一堆**字体本来
#     就没有**的码位硬失败。那些红是噪音，改不动字体就消不掉。
# ⚠️ 这些区里的字若字体本身没有，下面的比对会**判失败**并指名是哪一个 —— 那时要么把该区间
#    收窄并写明理由，要么换字体，**不许**把失败调成警告。
EXTRA_RANGES = [
    (0x0020, 0x007E),   # ASCII 可打印区
    (0x00A0, 0x00FF),   # Latin-1 补充区（× · ° ± § —— 界面上很可能拼出来）
    (0x3000, 0x303F),   # CJK 标点（、。《》「」…）
    (0xFF01, 0xFF60),   # 全角形式（！＂＃…￠）
    (0xFFE0, 0xFFE6),   # 全角货币/符号
]
# 逐码位补的那几个（都**实测过这份字体有**；缺一个都会在生成模式的比对里被判失败）：
EXTRA_CODEPOINTS = [
    0x2014,  # — 破折号
    0x2018, 0x2019,  # ‘ ’
    0x201C, 0x201D,  # “ ”
    0x2022,  # • 项目符号
    0x2026,  # … 省略号
    0x2192,  # → 箭头（界面里"从…到…"这类文案很可能拼）
    0x2500,  # ─ 制表横线
    0x25CF,  # ● 圆点（状态点）
    0x2713,  # ✓ 对勾（注意：✗ U+2717 这份字体**没有**，所以只收了一个）
]

# 豁免清单：**逐码位**列出"要求集里有、而这份字体提供不了"的码位，**每条带理由**。
#
# ⚠️ 第一版这里是**区间**（U+FE00–FE0F + U+1F000–1FAFF），复审 2026-09-18 判定那是错的：
#    区间会把"**以为**有人渲染得出来"的字一起放行。复审实测（读 epaint 0.32.3 随附的
#    四份字体 NotoEmoji-Regular / emoji-icon-font / Hack-Regular / Ubuntu-Light 的并集）：
#      · U+1F000–U+1FAFF 共 2816 个码位，四份并集只覆盖 **793** 个（**2023 个不覆盖**）；
#      · U+FE00–FE0F 变体选择符**一个都没有**；
#      · 本条唯一命中的 U+1F642(🙂)：四份并集里**也没有**（U+1F600 😀 有，U+1F642 没有）。
#    ⇒ 按区间豁免 = 把两千多个**真豆腐**当豁免放行，与"漏检且没有东西会变红"**完全同型**。
#      所以改成逐码位 + 理由（与 `test.sh` 的 `guard_allow` 同形：放宽必须是一次能被审查的决定）。
#
# 两向闭环（见 `compare()`）：
#   · 清单里的码位若**已经被覆盖** ⇒ 判失败（字体其实有它，这条豁免是在放行一个能判的东西）；
#   · 清单里的码位若**已不在要求集里** ⇒ 判失败（这条豁免今天用不上了，删掉它，别让它烂在清单里）。
EXEMPT_CODEPOINTS = {
    0x1F642:
        "🙂 只在 core/src/presentation/engine_status.rs 的一条 #[test] 字符串里出现（**永不渲染**）。"
        "Noto Sans SC 是文字字体、不含 emoji，而 egui 随附的四份字体并集里**也没有**这个码位"
        "⇒ 它的理由是「**永不进界面**」，**不是**「有人渲染得出来」。"
        "一旦有人把它写进界面文案，这条豁免必须删掉，并改文案或换字体。",
    0xFE0F:
        "U+FE0F 是**变体选择符 VS16**：它**没有字形、零宽度**（Unicode 就是这么定的），"
        "在任何字体里都渲染不出一个可见的字，所以它**永远不会变成豆腐块** —— "
        "它跟在 U+26A0(⚠) 后面只是请求 emoji 呈现，缺了它只退回**文本呈现**，"
        "而 U+26A0 本身**已经在子集里**（客户看到的是 ⚠，不是方块）。"
        "⚠️ 它与上面那条**性质不同，别混起来读**：U+FE0F **确实出现在界面文案里**"
        "（`core/src/paths.rs` 的 `missing()` 拼出来的内核错误原文「⚠️ 这里**不会**退回相对路径…」，"
        "按规格 §10 壳不改写内核的话 ⇒ 它原样显示给客户）。"
        "⇒ 这条豁免的理由**不是**「永不进界面」，而是「**它没有任何可渲染的字形**」："
        "少掉的是「呈现方式」，不是「一个字」。"
        "证据（2026-09-19，集成树）：`fontTools` 独立读源包里的 `NotoSansSC-Regular.otf`，"
        "U+FE0F 与 U+FE0E **都不在 cmap 里**（不是本脚本自带解析器的读数问题）；"
        "同一个字面量里的 U+26A0 在里面，并且已随本次生成进了子集。"
        "⚠️ 换到一款**含 VS16** 的可再分发字体时，上面那条两向闭环会要求删掉本条目。",
}


# 缺口清单在报错里最多逐条打印这么多个（再多就把真信息淹了；总数仍会写在标题里）。
SHOW_GAP_LIMIT = 30


def in_ranges(cp, ranges):
    return any(lo <= cp <= hi for lo, hi in ranges)


# ---------------------------------------------------------------------------
# 最小的 Rust 词法：把**字面量**（字符串/字符/原始字符串）与注释分开。
# ---------------------------------------------------------------------------
# 只要这三件事做对，本脚本的判据就成立；**任何漏判都必须朝"多抽"的方向犯**
# （多抽 = 子集多带几个字 = 没有风险；少抽 = 界面上一块豆腐 = 静默失守）。
# 所以下面 `lex()` 还会返回"注释与字面量之外的文本"，调用方**要求它不含非 ASCII**
# —— 那是这条词法的 fail-closed 自检：词法若与代码的实际写法脱节，这里会红。
_RAW_RE = re.compile(r'(?:b?r)(?P<h>#{0,255})"(?P<body>.*?)"(?P=h)', re.S)
_CHAR_RE = re.compile(r"'(?:\\.|[^\\'])'")


def lex(src):
    """返回 (字面量文本列表, 注释/字面量之外的文本)。"""
    i, n = 0, len(src)
    lits, outside = [], []
    while i < n:
        if src.startswith("//", i):                     # 行注释
            j = src.find("\n", i)
            i = n if j < 0 else j + 1
            continue
        if src.startswith("/*", i):                     # 块注释（Rust 允许嵌套）
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            continue
        m = _RAW_RE.match(src, i)                       # r"…" / r#"…"# / br#"…"#
        if m and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            lits.append(m.group("body"))
            i = m.end()
            continue
        if src[i] == "'":                               # 字符字面量（不是生命周期）
            m = _CHAR_RE.match(src, i)
            if m:
                lits.append(m.group(0)[1:-1])
                i = m.end()
                continue
            i += 1                                      # 生命周期 `'a`：跳过那个撇号
            continue
        if src[i] == '"':                               # 普通字符串（`\"` 转义、可跨行）
            j, buf = i + 1, []
            while j < n:
                if src[j] == "\\":
                    buf.append(src[j:j + 2])
                    j += 2
                    continue
                if src[j] == '"':
                    break
                buf.append(src[j])
                j += 1
            lits.append("".join(buf))
            i = j + 1
            continue
        outside.append(src[i])
        i += 1
    return lits, "".join(outside)


def required_chars(repo, roots, frontend_dirs):
    """要求集 = 三棵源码树的**字面量**里的非 ASCII 字符 ∪ 前端文件的字 ∪ EXTRA_RANGES。

    见文件头"字符集"一节：为什么是字面量而不是"全部源码字符"（有意收窄，W-6 那类决定）；
    以及为什么**前端文件也必须算进来**（它们不在任何 `.rs` 字面量里，而它们会被渲染）。

    ⚠️ `frontend_dirs` 由**调用方**给（本脚本 bash 侧的 `FRONTEND_DIRS`，
    经命令行 `--frontend` 传进来）—— 本函数**不再自带一份常量**。
    """
    chars = set()
    files = 0
    literals = 0
    outside_nonascii = {}
    for root in roots:
        base = os.path.join(repo, root)
        if not os.path.isdir(base):
            sys.exit(f"错误：源码树不存在：{base}（本脚本期望一个完整 checkout）")
        for dirpath, _dirnames, filenames in os.walk(base):
            for fn in filenames:
                if not fn.endswith(".rs"):
                    continue
                files += 1
                with open(os.path.join(dirpath, fn), encoding="utf-8") as fh:
                    source = fh.read()
                lits, outside = lex(source)
                literals += len(lits)
                for text in lits:
                    for ch in text:
                        if ord(ch) > 127:
                            chars.add(ord(ch))
                for ch in outside:
                    if ord(ch) > 127:
                        outside_nonascii[ch] = outside_nonascii.get(ch, 0) + 1
    # fail-closed 自检：注释与字面量之外**不该**有非 ASCII。有 ⇒ 词法过期或有人在写
    # 非 ASCII 标识符，两种都要人来判，**绝不能**悄悄少抽几个字。
    if outside_nonascii:
        shown = " ".join(f"U+{ord(c):04X}×{n}" for c, n in sorted(outside_nonascii.items(),
                                                                 key=lambda kv: ord(kv[0])))
        sys.exit(f"错误：注释与字面量之外出现了非 ASCII 字符（{shown}）——\n"
                 f"      这说明本脚本的 Rust 词法已经与代码的实际写法脱节（或者有人写了\n"
                 f"      非 ASCII 标识符）。**不要**把这条检查关掉：它一关，抽取就可能\n"
                 f"      悄悄少抽几个字，而那正是界面豆腐块、且没有任何东西会变红的来源。\n"
                 f"      补救：在本脚本的 lex() 里补上那种写法，然后重跑。")
    # 前端文件（`FRONTEND_DIRS` 下**递归**的 `.html` / `.css` / `.js`）：它们的字也是
    # **会被渲染的字**，而它们不在任何 `.rs` 字面量里 ⇒ 必须并进来，否则判据
    # （test.sh 第 0.6 步）会一直要求一批生成模式**从不收进子集**的字。
    # ⚠️ 抽取规则走**那份唯一实现**（`check_design_fonts.py --chars`）。
    # 🔴 扩展名与递归性是任务 10b 订正的（理由逐条记在 `FRONTEND_DIRS` 那一段）：
    #    只扫 `*.html` 时 `.js`/`.css` 里的字永远进不了子集；只扫深度 1 时
    #    `css/` 与 `js/` 下的文件连一个都够不着。**必须与 test.sh 第 0.6 步同一条扫描面**。
    frontend = []
    for rel in frontend_dirs:
        base = os.path.join(repo, rel)
        if os.path.isdir(base):
            for pattern in ("*.html", "*.css", "*.js"):
                frontend.extend(
                    sorted(glob.glob(os.path.join(base, "**", pattern), recursive=True)))
    frontend_only = set()
    if frontend:
        proc = subprocess.run(
            [sys.executable,
             os.path.join(repo, "windows", "scripts", "check_design_fonts.py"),
             "--chars", *frontend],
            capture_output=True, text=True)
        if proc.returncode != 0:
            sys.exit(f"错误：抽取前端文件的字符失败（check_design_fonts.py --chars，"
                     f"退出码 {proc.returncode}）：\n{proc.stderr}")
        for line in proc.stdout.split():
            line = line.strip()
            if line.startswith("U+"):
                frontend_only.add(int(line[2:], 16))
    chars.update(frontend_only)
    literal_chars = len(chars)
    extra = sum(hi - lo + 1 for lo, hi in EXTRA_RANGES) + len(EXTRA_CODEPOINTS)
    for lo, hi in EXTRA_RANGES:
        chars.update(range(lo, hi + 1))
    chars.update(EXTRA_CODEPOINTS)
    # ⚠️ 报的是**并集**的大小，不是"字面量 + 标点"的和：两者有重叠（字面量里本来就有
    #    全角标点），而"合计"写成和数会让后面比对里那个数字对不上账（本项目对
    #    "报出来的数就是真的那个数"有纪律）。
    print(f"源码树：{files} 个 .rs 文件、{literals} 段字面量；"
          f"前端文件 {len(frontend)} 个（{' '.join(frontend_dirs)}）里的非 ASCII 字符 "
          f"{len(frontend_only)} 个；两类并集 {literal_chars} 个非 ASCII 字符，"
          f"∪ 附加标点集（区间 {sum(hi - lo + 1 for lo, hi in EXTRA_RANGES)} 个 + 逐码位 "
          f"{len(EXTRA_CODEPOINTS)} 个，去重后净增 {len(chars) - literal_chars} 个）"
          f"⇒ **要求集共 {len(chars)} 个码位**（附加集共 {extra} 项）", file=sys.stderr)
    print(f"        自检 outside_nonascii={len(outside_nonascii)}"
          f"（注释与字面量之外的非 ASCII 字符数，必须是 0）", file=sys.stderr)
    return chars


def sfnt_tables(blob, path):
    if len(blob) < 12 or blob[:4] not in (b"\x00\x01\x00\x00", b"OTTO", b"true", b"ttcf"):
        sys.exit(f"错误：{path} 不是可识别的 sfnt 字体（前四字节 {blob[:4]!r}）")
    if blob[:4] == b"ttcf":
        sys.exit(f"错误：{path} 是 TTC 集合字体，本脚本只处理单一字体（不猜第几份）")
    if len(blob) < 12:
        sys.exit(f"错误：{path} 太短，连 sfnt 头都不全")
    num_tables = struct.unpack_from(">H", blob, 4)[0]
    tables = {}
    for i in range(num_tables):
        off = 12 + 16 * i
        if off + 16 > len(blob):
            sys.exit(f"错误：{path} 的表目录越界（第 {i} 条）")
        tag = blob[off:off + 4].decode("latin-1")
        toff, tlen = struct.unpack_from(">II", blob, off + 8)
        if toff + tlen > len(blob):
            sys.exit(f"错误：{path} 的表 {tag} 越界（{toff}+{tlen} > {len(blob)}）")
        tables[tag] = (toff, tlen)
    return tables


def cmap_codepoints(blob, path):
    """**不依赖 fontTools** 的 cmap 解析（`--check` 要在没有 fontTools 的机器上跑）。

    只认 Unicode 子表的 format 4 与 format 12 —— 这两者覆盖 BMP 与增补面，
    也是所有现代字体实际使用的那两种；遇到别的格式**大声失败**而不是返回空集
    （返回空集会让"覆盖率"判据以"字体里一个字都没有"的形式炸，根因就说不清了）。
    """
    tables = sfnt_tables(blob, path)
    if "cmap" not in tables:
        sys.exit(f"错误：{path} 里没有 cmap 表")
    off, _ = tables["cmap"]
    n = struct.unpack_from(">H", blob, off + 2)[0]
    subs = []
    for i in range(n):
        p = off + 4 + 8 * i
        pid, eid, so = struct.unpack_from(">HHI", blob, p)
        subs.append((pid, eid, off + so))
    # 选子表的优先级：`(3,10)`=UCS-4 → `(0,*)`=Unicode → `(3,1)`=BMP。
    order = [(3, 10), (0, 6), (0, 4), (0, 3), (0, 2), (0, 1), (0, 0), (3, 1)]
    chosen = None
    for want in order:
        for pid, eid, sub in subs:
            if (pid, eid) == want:
                fmt = struct.unpack_from(">H", blob, sub)[0]
                if fmt in (4, 12):
                    chosen = (sub, fmt)
                    break
        if chosen:
            break
    if chosen is None:
        sys.exit(f"错误：{path} 里找不到可用的 Unicode cmap 子表"
                 f"（实际有 {[(p, e) for p, e, _ in subs]}）")
    sub, fmt = chosen
    cps = set()
    if fmt == 4:
        seg_x2 = struct.unpack_from(">H", blob, sub + 6)[0]
        seg = seg_x2 // 2
        ends = struct.unpack_from(f">{seg}H", blob, sub + 14)
        starts = struct.unpack_from(f">{seg}H", blob, sub + 16 + seg_x2)
        deltas = struct.unpack_from(f">{seg}h", blob, sub + 16 + 2 * seg_x2)
        range_off_pos = sub + 16 + 3 * seg_x2
        for i in range(seg):
            if starts[i] > ends[i]:
                continue
            for cp in range(starts[i], ends[i] + 1):
                if cp == 0xFFFF:
                    continue  # format 4 的哨兵段
                ro = struct.unpack_from(">H", blob, range_off_pos + 2 * i)[0]
                if ro == 0:
                    gid = (cp + deltas[i]) & 0xFFFF
                else:
                    gi = range_off_pos + 2 * i + ro + 2 * (cp - starts[i])
                    if gi + 2 > len(blob):
                        continue
                    gid = struct.unpack_from(">H", blob, gi)[0]
                    if gid != 0:
                        gid = (gid + deltas[i]) & 0xFFFF
                if gid != 0:
                    cps.add(cp)
    else:  # format 12
        n_groups = struct.unpack_from(">I", blob, sub + 12)[0]
        for i in range(n_groups):
            p = sub + 16 + 12 * i
            start, end, gid = struct.unpack_from(">III", blob, p)
            if gid != 0:
                cps.update(range(start, min(end, start + 0x10FFFF) + 1))
    return cps


def read_required(path):
    with open(path, encoding="utf-8") as fh:
        return {ord(ch) for ch in fh.read()}


def compare(required, covered, source_label):
    missing = sorted(cp for cp in required if cp not in covered)
    exempt = [cp for cp in missing if cp in EXEMPT_CODEPOINTS]
    gap = [cp for cp in missing if cp not in EXEMPT_CODEPOINTS]
    # 两向闭环：豁免清单本身也要能被证伪（见 `EXEMPT_CODEPOINTS` 上面那段）。
    # ① 覆盖到了 ⇒ 这条豁免在放行一个**本来能判**的东西；
    # ② 已不在要求集里 ⇒ 这条豁免今天用不上了。
    stale_covered = sorted(cp for cp in EXEMPT_CODEPOINTS if cp in required and cp in covered)
    stale_unused = sorted(cp for cp in EXEMPT_CODEPOINTS if cp not in required)
    print(f"要求集 {len(required)} 个码位；{source_label} 覆盖 {len(covered)} 个；"
          f"缺 {len(missing)} 个（其中豁免 {len(exempt)} / 真缺口 {len(gap)}）")
    for cp in exempt:
        print(f"  豁免：U+{cp:04X} —— {EXEMPT_CODEPOINTS[cp]}")
    if stale_covered or stale_unused:
        sys.stdout.flush()
        print("", file=sys.stderr)
        print("豁免清单失效了（`--check` 与生成模式都会因此失败）：", file=sys.stderr)
        for cp in stale_covered:
            print(f"    U+{cp:04X}：**它其实被覆盖了** —— 这条豁免在放行一个本来能判的码位，"
                  f"删掉它（清单在 EXEMPT_CODEPOINTS）", file=sys.stderr)
        for cp in stale_unused:
            print(f"    U+{cp:04X}：**它已不在要求集里** —— 这条豁免今天用不上了，"
                  f"删掉它（别让清单烂在那里）", file=sys.stderr)
        return 3
    if gap:
        # ⚠️ 先把 stdout 冲掉再往 stderr 写：两股流在**被管道接走**时缓冲方式不同
        #    （stdout 块缓冲、stderr 不缓冲），不冲的话日志里"结论"会跑到"读数"前面，
        #    读者看到的第一行是一句没有上下文的失败（本项目对"报出来的东西读得通"有要求）。
        sys.stdout.flush()
        print("", file=sys.stderr)
        print(f"覆盖判据没过：要求集里有 {len(gap)} 个码位**不在**子集里，它们会显示成方块：",
              file=sys.stderr)
        for cp in gap[:SHOW_GAP_LIMIT]:
            print(f"    U+{cp:04X} {chr(cp)}", file=sys.stderr)
        if len(gap) > SHOW_GAP_LIMIT:
            print(f"    …（其余 {len(gap) - SHOW_GAP_LIMIT} 个同类，从略）", file=sys.stderr)
        print("补救：重跑 `bash windows/scripts/make_font_subset.sh`（生成模式）。", file=sys.stderr)
        print("      · 生成模式里**同一组**字要先过一遍源字体：源字体本身没有这些字时，", file=sys.stderr)
        print("        它会指名是哪几个，并告诉你这一组字**换字体也解决不了**。", file=sys.stderr)
        print("      · **不要**为了让这条变绿而把码位塞进 EXEMPT_CODEPOINTS：豁免的意思是", file=sys.stderr)
        print("        \"这个码位**永不进界面**\"，而且要**逐条写明理由**。会被渲染的字，", file=sys.stderr)
        print("        唯一的解法是让它进子集（生成模式会做），或者改文案。", file=sys.stderr)
        return 3
    print("覆盖判据通过：源码里每个（非豁免）非 ASCII 字符都在子集里。")
    return 0


def name_records(blob, path):
    """取 name 表里 (nameID, 字符串) 的列表 —— 用来断言版权/OFL 声明还在。"""
    tables = sfnt_tables(blob, path)
    if "name" not in tables:
        return []
    off, _ = tables["name"]
    count, string_off = struct.unpack_from(">HH", blob, off + 2)
    out = []
    for i in range(count):
        p = off + 6 + 12 * i
        pid, eid, lid, nid, length, so = struct.unpack_from(">HHHHHH", blob, p)
        raw = blob[off + string_off + so: off + string_off + so + length]
        try:
            if pid == 3:      # Windows：UTF-16BE
                text = raw.decode("utf-16-be")
            elif pid == 1 and eid == 0:  # Mac Roman
                text = raw.decode("mac-roman")
            else:
                continue
        except UnicodeDecodeError:
            continue
        out.append((nid, text))
    return out


def main():
    if len(sys.argv) < 2:
        sys.exit("内部错误：helper 缺子命令")
    cmd = sys.argv[1]
    if cmd == "required":
        repo, out_path = sys.argv[2], sys.argv[3]
        # ⚠️ `--frontend <目录…>` 是本脚本 bash 侧传进来的那份清单（唯一属主在那边）。
        rest = sys.argv[4:]
        frontend_dirs = []
        if "--frontend" in rest:
            i = rest.index("--frontend")
            frontend_dirs = rest[i + 1:]
            rest = rest[:i]
        chars = required_chars(repo, rest, frontend_dirs)
        with open(out_path, "w", encoding="utf-8") as fh:
            fh.write("".join(chr(cp) for cp in sorted(chars)))
        return 0
    if cmd == "cmap":
        path, out_path = sys.argv[2], sys.argv[3]
        with open(path, "rb") as fh:
            cps = cmap_codepoints(fh.read(), path)
        with open(out_path, "w", encoding="ascii") as fh:
            fh.write(" ".join(f"{cp:X}" for cp in sorted(cps)))
        print(f"{os.path.basename(path)}：cmap 覆盖 {len(cps)} 个码位（本脚本自带的解析器）")
        return 0
    if cmd == "compare":
        required = read_required(sys.argv[2])
        with open(sys.argv[3], encoding="ascii") as fh:
            covered = {int(tok, 16) for tok in fh.read().split()}
        return compare(required, covered, sys.argv[4] if len(sys.argv) > 4 else "子集")
    if cmd == "sha256":
        # 用 python 自己算，是为了让 `--check`**不引入新工具**（shasum/openssl 都只在
        # 生成模式需要）—— 这条判据要在"只有 python3"的机器上也能跑。
        import hashlib
        with open(sys.argv[2], "rb") as fh:
            print(hashlib.sha256(fh.read()).hexdigest())
        return 0
    if cmd == "names":
        path = sys.argv[2]
        with open(path, "rb") as fh:
            blob = fh.read()
        bad = 0
        found = {}
        for nid, text in name_records(blob, path):
            found.setdefault(nid, text)
        checks = [
            (0, "Adobe", "版权声明（nameID 0）"),
            (13, "SIL Open Font License", "OFL 声明（nameID 13）"),
            (14, "scripts.sil.org/OFL", "OFL 网址（nameID 14）"),
        ]
        for nid, needle, label in checks:
            have = found.get(nid, "")
            if needle in have:
                print(f"  ✓ {label} 在")
            else:
                print(f"  ✗ {label} **不在**（实际：{have[:80]!r}）", file=sys.stderr)
                bad = 1
        return 1 if bad else 0
    sys.exit(f"内部错误：不认识的子命令 {cmd}")


if __name__ == "__main__":
    sys.exit(main())
PY

# ---------------------------------------------------------------------------
# 交付副本判据：`windows/web/fonts/` 下的两份必须是这里那两份的**逐字节副本**
# ---------------------------------------------------------------------------
# 为什么需要它（任务 18 ①，审查者 I-1 点名的"风险归属真空"）：
#   · **为什么会有这份副本**：`shell-win/tauri.conf.json` 的
#     `build.frontendDist = "../web"` ⇒ 运行时**只有 `windows/web/` 下的文件**是
#     webview 取得到的，而字体必须跟着页面走（`../assets/…` 会被 URL 规则夹回根 ⇒
#     404 ⇒ 静默回退系统字体）。所以交付页那份**必须**在 `web/fonts/` 里。
#   · **风险**：它是**入库的副本** ⇒ 重新生成子集之后要**人肉重拷**一次。
#     忘了重拷的表现是"**界面上悄悄换回旧字形**"（新加的那个字是豆腐块），
#     而 `test.sh` / `build_windows.sh` / 覆盖判据**全绿** —— 在加这条判据之前，
#     这堆判据里**没有一条**会响（它们读的都是 `assets/` 那份）。
#   · 审查者点名的正是这件事：**这个风险当时没有属主**（任务 15 的步骤表里也没有
#     这一步）。⇒ 现在它有属主了，就是这里，两条调用点：
#       `--check`（`test.sh` 第 0.5 步走这条）与生成模式第 9.6 步。
#
# ⚠️ **判的是"逐字节相同"，不是"文件在不在"**：只判存在性的话，
#    一份**上一个版本的子集**照样能让所有判据全绿 —— 那正是这条判据要防的形态。
# ⚠️ **两份都判**：许可全文也是同样的"入库副本"（OFL 条件 2 的随附义务），
#    客户在设置里能读到的就是 web 那一份 ⇒ 它停在旧版本同样是"悄悄失效"。
# ⚠️ 判据用 `python3` 算 sha256（**不引入 `cmp`**）：`--check` 的工具前置只有
#    `python3` 一条（见第 0 步），加一个工具就等于给这条判据加一个"环境不对就跳过"
#    的缺口 —— 而 W-2 明令不许静默降级。
check_delivery_twins() {
  local src dst label sha_src sha_dst bad=0
  while IFS='|' read -r src dst label; do
    [ -n "$src" ] || continue
    for f in "$src" "$dst"; do
      [ -f "$f" ] || {
        echo "错误：交付副本判据的对象不在场：$f（判据无对象可判）。" >&2
        bad=1
      }
    done
    [ -f "$src" ] && [ -f "$dst" ] || continue
    sha_src="$(python3 "$helper" sha256 "$src")"
    sha_dst="$(python3 "$helper" sha256 "$dst")"
    if [ "$sha_src" != "$sha_dst" ]; then
      {
        echo ""
        echo "错误：${label}的交付副本与它的源文件**不是逐字节相同**（不符即拒）。"
        echo "      源文件（唯一真相）：${src#"$REPO"/}"
        echo "          sha256 ${sha_src}"
        echo "      交付副本（页面真正加载的那一份）：${dst#"$REPO"/}"
        echo "          sha256 ${sha_dst}"
        echo "      为什么这条必须一致：webview 的根是 windows/web/（tauri.conf.json 的"
        echo "      build.frontendDist），页面加载的就是**副本**那一份 —— 副本停在旧版本的"
        echo "      后果是「界面上悄悄换回旧字形 / 许可全文是旧的」，而其余判据**全都不会响**"
        echo "      （它们读的是 assets/ 那份）。"
        echo "      补救：重跑生成模式刷出新产物之后，**把这两份一起拷过去**："
        echo "          cp windows/assets/ui-subset.otf windows/web/fonts/ui-subset.otf"
        echo "          cp windows/assets/OFL-1.1.txt   windows/web/fonts/OFL-1.1.txt"
        echo "      （它们由 windows/scripts/build_windows.sh 编进 exe，所以这一步"
        echo "        不是「优化」，是交付物的一部分。）"
      } >&2
      bad=1
    else
      echo "交付副本一致：${dst#"$REPO"/}（sha256 ${sha_dst}，与 ${src#"$REPO"/} 逐字节相同）"
    fi
  done <<EOF
$OUT_FONT|$WEB_FONT|内嵌子集
$OUT_LICENSE|$WEB_LICENSE|OFL 1.1 全文
EOF
  [ "$bad" -eq 0 ] || exit 3
}

# ---------------------------------------------------------------------------
# 1. 取要求集（**两种模式共用**；见文件头"字符集"）
# ---------------------------------------------------------------------------
note "抽取要求集（源码树：${SOURCE_ROOTS}）"
# shellcheck disable=SC2086  # SOURCE_ROOTS 就是要按空格拆成多个参数
python3 "$helper" required "$REPO" "$WORK/required.txt" $SOURCE_ROOTS \
  --frontend $FRONTEND_DIRS \
  || die "抽取字符集失败（原因见上一行）"

# ---------------------------------------------------------------------------
# 2. --check：只校验**已入库的那份**（不联网、不需要 fontTools）
# ---------------------------------------------------------------------------
if [ "$check_only" -eq 1 ]; then
  [ -f "$OUT_FONT" ] || {
    echo "错误：找不到内嵌子集 ${OUT_FONT}。" >&2
    echo "      W-5 要求「系统字体优先 + 内嵌子集兜底」，而 fonts.rs 用 include_bytes! 内嵌它——" >&2
    echo "      文件缺席不是「少了个优化」，是**编译都过不去**（以及许可证义务没履行）。" >&2
    echo "      补救：bash windows/scripts/make_font_subset.sh（生成模式，需联网）" >&2
    exit 1
  }
  [ -s "$OUT_LICENSE" ] && [ -s "$CORE_LICENSE" ] || {
    echo "错误：许可全文不在场（${OUT_LICENSE} / ${CORE_LICENSE}）。" >&2
    echo "      OFL 1.1 的条件 2 要求随附许可；文件空着或缺失 = 分发义务没履行。" >&2
    echo "      补救：bash windows/scripts/make_font_subset.sh（生成模式会把它从官方包里落下来）" >&2
    exit 1
  }
  # ---- ① 身份：这份资产**是不是我们登记的那一份** ------------------------------
  # ⚠️ 这一步与"覆盖率"是**两件事**，缺了它，换成一份从**不可再分发**字体抽的同形子集
  #    也能全绿过去（复审 2026-09-18 的重要 #3）。
  GOT_SUBSET_SHA="$(python3 "$helper" sha256 "$OUT_FONT")"
  if [ "$GOT_SUBSET_SHA" != "$SUBSET_SHA256" ]; then
    {
      echo "错误：内嵌子集的 sha256 与登记值不符（不符即拒）。" >&2
      echo "      期望（登记）：$SUBSET_SHA256" >&2
      echo "      实际（入库）：$GOT_SUBSET_SHA" >&2
      echo "      两种可能，补救不同：" >&2
      echo "        ① 你**有意**用生成模式刷新过它（换了 fontTools 版本也会走到这里）" >&2
      echo "           ⇒ 把上面那行「实际」抄进脚本头部的 SUBSET_SHA256，然后重跑本脚本。" >&2
      echo "        ② 它被人**手工换过**（或者来路不明）" >&2
      echo "           ⇒ 别抄那行。用 \`bash windows/scripts/make_font_subset.sh\` 重新生成，" >&2
      echo "              再核对生成模式打印出来的来源登记。" >&2
      echo "      —— 这条判据守的是 W-5 的硬禁令：内嵌的那份**必须**是可再分发的字体。" >&2
    } >&2
    exit 3
  fi
  echo "内嵌子集 sha256：${GOT_SUBSET_SHA}（与登记值相符）"

  # ---- ② 来路：字体自带的版权与 OFL 声明还在吗（nameID 0/13/14）----------------
  # 这一条独立于 sha256：它读的是**字体的 name 表**。若有人塞进来一份从微软雅黑（或别的
  # 不可再分发字体）抽的子集，sha256 与这里**都**会响 —— 两条判据的失效方式不同，
  # 所以都留。
  python3 "$helper" names "$OUT_FONT" || {
    echo "错误：${OUT_FONT} 的名字表里缺版权/OFL 声明 —— 它**不是**登记的那份 Noto 子集。" >&2
    exit 3
  }

  # ---- ③ 许可：两份全文必须**与官方 LICENSE 逐字节相同** ------------------------
  # 从前的判据只是"文件非空且含标题"——那挡不住"换成另一份（或改写过的）许可文本"。
  for f in "$OUT_LICENSE" "$CORE_LICENSE"; do
    got="$(python3 "$helper" sha256 "$f")"
    if [ "$got" != "$FONT_LICENSE_SHA256" ]; then
      {
        echo "错误：${f} 与官方包内 LICENSE 不再逐字节相同（不符即拒）。" >&2
        echo "      期望（官方 LICENSE 的登记 sha256）：$FONT_LICENSE_SHA256" >&2
        echo "      实际：$got" >&2
        echo "      补救：bash windows/scripts/make_font_subset.sh（生成模式会从官方包重新落下来）" >&2
      } >&2
      exit 3
    fi
  done
  echo "许可全文 sha256：${FONT_LICENSE_SHA256}（两份都与官方 LICENSE 逐字节相同）"

  # ---- ④ 覆盖：源码里会被渲染的字，子集里都得有 --------------------------------
  note "校验已入库子集的 cmap（本脚本自带的解析器，不需要 fontTools）"
  python3 "$helper" cmap "$OUT_FONT" "$WORK/covered.txt" || die "解析 ${OUT_FONT} 失败（它不是可用的字体？）"
  set +e
  python3 "$helper" compare "$WORK/required.txt" "$WORK/covered.txt" "已入库的 ${OUT_FONT##*/}"
  rc=$?
  set -e
  [ "$rc" -eq 0 ] || exit "$rc"

  # ---- ⑤ 交付副本：`windows/web/fonts/` 那两份必须与这里这两份逐字节相同 ----------
  # ⚠️ 它**必须**也在 `--check` 里（不是只在生成模式里）：`test.sh` 第 0.5 步走的
  #    就是这条 —— 而"忘了重拷"这件事正是发生在那一步**之前**（有人手工刷了资产、
  #    或者换了机器/有一次不完整的 checkout）。失败即退出码 3，理由同②③④。
  check_delivery_twins

  note "--check 通过（身份 sha256 + 名字表声明 + 许可逐字节 + 覆盖判据 + 交付副本逐字节）"
  exit 0
fi

# ---------------------------------------------------------------------------
# 3. 下载源字体包（缓存命中也要过 sha256）
# ---------------------------------------------------------------------------
mkdir -p "$CACHE_DIR"
ZIP="$CACHE_DIR/$FONT_ASSET_NAME"
note "源字体包：${FONT_ASSET_NAME}（缓存目录 ${CACHE_DIR}）"

zip_ok() {
  [ -f "$1" ] || return 1
  [ "$(wc -c < "$1" | tr -d ' ')" = "$FONT_ZIP_SIZE" ] || return 1
  [ "$(shasum -a 256 "$1" | awk '{print $1}')" = "$FONT_ZIP_SHA256" ] || return 1
}

if zip_ok "$ZIP"; then
  note "缓存命中且 sha256 相符，不重新下载"
else
  note "下载中（约 50 MB）"
  # 先下到临时文件再原子落位：下到一半的包**绝不能**留在缓存里冒充"已核实"的缓存。
  curl -sSL --http1.1 --max-time 600 -H 'Accept: application/octet-stream' \
       -o "$WORK/$FONT_ASSET_NAME" \
       "https://api.github.com/repos/notofonts/noto-cjk/releases/assets/${FONT_ASSET_ID}" \
    || die "下载失败：资产 ${FONT_ASSET_ID}（先确认 api.github.com 可达；github.com 直连在本机不可达，别去试）"
  mv -f "$WORK/$FONT_ASSET_NAME" "$ZIP"
fi

# ---------------------------------------------------------------------------
# 4. 自验 ①：包 sha256 —— **两个独立工具各算一遍，先互相核对**再比登记值
# ---------------------------------------------------------------------------
# ⚠️ 顺序承重（照抄 aria2 那份脚本的理由）：先用两个工具互核，再比登记值。少了互核，
#    "工具本身给错"与"下载内容不对"两种失败会挤在同一句报错里，排障时无从下手。
PKG_A="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
PKG_B="$(openssl dgst -sha256 "$ZIP" | awk '{print $NF}')"
[ "$PKG_A" = "$PKG_B" ] || die "两个工具算出的包 sha256 不一致：shasum=${PKG_A} openssl=${PKG_B}（先怀疑环境，不要改登记值）"
if [ "$PKG_A" != "$FONT_ZIP_SHA256" ]; then
  {
    echo "错误：包 sha256 与登记值不符（不符即拒，不安装）。" >&2
    echo "      期望（登记）：$FONT_ZIP_SHA256" >&2
    echo "      实际（下载）：$PKG_A" >&2
    echo "      若这是**有意的版本升级**：先确认新的 release tag 与资产 id，" >&2
    echo "      再把脚本头部的登记、下面五个常量与规格 §6.3 的体积登记一起改。" >&2
  } >&2
  exit 2
fi
PKG_SIZE="$(wc -c < "$ZIP" | tr -d ' ')"
echo "包 sha256：${PKG_A}（shasum 与 openssl 一致，与登记值相符）"
echo "包大小：${PKG_SIZE} B（登记值 ${FONT_ZIP_SIZE}）"

# ---------------------------------------------------------------------------
# 5. 取出字体与许可，各自自验 sha256 + 大小
# ---------------------------------------------------------------------------
note "解包并取出 ${FONT_OTF_MEMBER} 与 ${FONT_LICENSE_MEMBER}"
( cd "$WORK" && unzip -q -o "$ZIP" "$FONT_OTF_MEMBER" "$FONT_LICENSE_MEMBER" ) \
  || die "解包失败：${ZIP}（包的内部布局变了？用 unzip -l 看一眼）"
SRC_OTF="$WORK/$FONT_OTF_MEMBER"
SRC_LICENSE="$WORK/$FONT_LICENSE_MEMBER"
[ -f "$SRC_OTF" ] || die "包里没有 ${FONT_OTF_MEMBER}"
[ -f "$SRC_LICENSE" ] || die "包里没有 ${FONT_LICENSE_MEMBER}"

verify_file() {  # verify_file <path> <expect-sha256> <expect-size> <label>
  local path="$1" want_sha="$2" want_size="$3" label="$4"
  local a b size
  a="$(shasum -a 256 "$path" | awk '{print $1}')"
  b="$(openssl dgst -sha256 "$path" | awk '{print $NF}')"
  [ "$a" = "$b" ] || die "两个工具算出的 ${label} sha256 不一致：shasum=${a} openssl=${b}"
  if [ "$a" != "$want_sha" ]; then
    {
      echo "错误：${label} 与登记值不符（不符即拒，不安装）。" >&2
      echo "      期望（登记）：$want_sha" >&2
      echo "      实际（取出）：$a" >&2
    } >&2
    exit 2
  fi
  size="$(wc -c < "$path" | tr -d ' ')"
  [ "$size" = "$want_size" ] || die "${label} 大小与登记值不符：期望 ${want_size} B，实际 ${size} B"
  echo "${label} sha256：${a}（shasum 与 openssl 一致，与登记值相符；${size} B）"
}
verify_file "$SRC_OTF" "$FONT_OTF_SHA256" "$FONT_OTF_SIZE" "$FONT_OTF_MEMBER"
verify_file "$SRC_LICENSE" "$FONT_LICENSE_SHA256" "$FONT_LICENSE_SIZE" "$FONT_LICENSE_MEMBER"

# 版本串也报一遍：sha256 已经能保证字节，但"版本"要能被人一眼看见（报告里要写它）。
# ⚠️ 这里**不把版本串当判据**：它只是给人看的第二读数（判据是 sha256，且它已经过了）。
FONT_VERSION="$(python3 - "$SRC_OTF" <<'PY'
import struct, sys
blob = open(sys.argv[1], "rb").read()
n = struct.unpack_from(">H", blob, 4)[0]
for i in range(n):
    off = 12 + 16 * i
    if blob[off:off + 4] != b"name":
        continue
    toff, _ = struct.unpack_from(">II", blob, off + 8)
    count, str_off = struct.unpack_from(">HH", blob, toff + 2)
    for k in range(count):
        p = toff + 6 + 12 * k
        pid, _eid, _lid, nid, length, so = struct.unpack_from(">HHHHHH", blob, p)
        if nid == 5 and pid == 3:
            sys.stdout.write(blob[toff + str_off + so: toff + str_off + so + length].decode("utf-16-be"))
            break
    break
PY
)"
echo "源字体自报版本（name ID 5）：${FONT_VERSION:-（取不到——名字表形状变了？）}"

# ---------------------------------------------------------------------------
# 6. 先拿**源字体**过一遍要求集（fail-closed：源字体都没有的字，子集里更不会有）
# ---------------------------------------------------------------------------
# 这一步是"换字体也解决不了"那个根因的落点：若有人新增了一个 Noto Sans SC 里没有的
# 汉字，这里会**指名道姓**地失败，而不是等到界面上出现一块豆腐才被发现。
note "先核对源字体的 cmap（同一组要求集）"
python3 "$helper" cmap "$SRC_OTF" "$WORK/src_cmap.txt" || die "解析源字体失败"
set +e
python3 "$helper" compare "$WORK/required.txt" "$WORK/src_cmap.txt" "源字体 ${FONT_OTF_MEMBER}"
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
  {
    echo ""
    echo "⚠️ 上面的缺口出在**源字体本身**：这些码位 Noto Sans SC 里根本没有，"
    echo "   所以它们**不是**本脚本能补的 —— 换子集参数没用，换字体才有用。"
    echo "   两条可走的路（选一条并写明理由）："
    echo "     ① 这些字**不该出现在需要兜底的界面上** ⇒ 改文案/改数据，别让它们进要求集；"
    echo "     ② 这些字**必须**有 ⇒ 换一款含它们的可再分发字体（仍须 SIL OFL 一类），"
    echo "        并把本脚本头部的登记与 EXEMPT_RANGES 一起改。"
  } >&2
  exit "$rc"
fi

# ---------------------------------------------------------------------------
# 7. 生成子集
# ---------------------------------------------------------------------------
# 参数逐条记账（每一条都要能说出"为什么去掉它不影响渲染"）：
#   --no-hinting        egui 用 ab_glyph 自带的光栅化器，**不消费**字体里的 hinting
#                       （TT 的 fpgm/prep、CFF 的 hint 操作符都不读）⇒ 去掉只减体积。
#                       实测：与不去掉相比省约 40 KB（同一组字、同一版本 fontTools）。
#   --layout-features='' 与 --drop-tables+=GSUB,GPOS,GDEF,BASE：egui **不做复杂排版**
#                       （规格里没有连字/上下文替换这类需求），它只做"字符 → 字形 → 光栅化"。
#                       字形替换子表因此是死重量。实测：省约 19 KB。
#   --drop-tables+=VORG,vhea,vmtx：本界面**没有竖排文字**（这三张是竖排的字形/度量表）。
#   --drop-tables+=BASE,DSIG：BASE 是复杂文种的基线表（egui 不做那类排版）；
#                       DSIG 是已废弃的签名表（现代工具链不写、校验也不看）。
#   --name-IDs='*'      **反着来的那一条**：默认的子集化会丢掉 nameID 13/14（许可与声明），
#                       而"随附版权声明与许可"是 OFL 的条件 2 ⇒ 显式保留整张 name 表。
#                       代价约 2 KB，买的是"字体自带许可声明"这件事。第 8 步会断言它还在。
note "生成子集（覆盖要求集里的全部字符；源字体没有的会被它自己丢掉，第 6 步已拦过）"
pyftsubset "$SRC_OTF" \
  --text-file="$WORK/required.txt" \
  --output-file="$WORK/ui-subset.otf" \
  --no-hinting \
  --layout-features='' \
  --drop-tables+=GSUB,GPOS,GDEF,BASE,VORG,vhea,vmtx,DSIG \
  --name-IDs='*' \
  || die "pyftsubset 失败（fonttools 版本：$(python3 -c 'import fontTools; print(fontTools.version)' 2>/dev/null || echo 未知)）"
SUBSET_BYTES="$(wc -c < "$WORK/ui-subset.otf" | tr -d ' ')"
echo "子集大小：${SUBSET_BYTES} B"
if [ "$SUBSET_BYTES" -gt "$MAX_SUBSET_BYTES" ]; then
  {
    echo "错误：子集 ${SUBSET_BYTES} B 超过 1 MB 硬线（MAX_SUBSET_BYTES=${MAX_SUBSET_BYTES}）。" >&2
    echo "      那条口径（出自已随那一代方案删掉的 §14.1，见 MAX_SUBSET_BYTES 上面那段）：" >&2
    echo "      '若超过 1 MB，回来重新权衡' —— 所以这里**停下来**，不是自动放行。" >&2
    echo "      可以先看的几处：要求集是不是被注释里的大段散文撑大了（可改成只抽字符串字面量，"
    echo "      但那样就换来了'词法判决漏字'的风险 —— 要显式记账）、或换成更小的字体。" >&2
  } >&2
  exit 3
fi

# ---------------------------------------------------------------------------
# 8. 自验 ②：产物的覆盖判据 —— **两条独立读数**（本脚本解析器 + fontTools）
# ---------------------------------------------------------------------------
note "校验产物子集"
python3 "$helper" cmap "$WORK/ui-subset.otf" "$WORK/sub_cmap.txt" || die "解析产物子集失败"
set +e
python3 "$helper" compare "$WORK/required.txt" "$WORK/sub_cmap.txt" "产物子集（本脚本解析器）"
rc=$?
set -e
[ "$rc" -eq 0 ] || exit "$rc"

# 第二条独立读数：fontTools 自己的 cmap（与上面那个手写解析器**不同源**）。
# 两条读数不一致 ⇒ 说明其中一条错了，此处**当场失败**（不能挑一个好看的信）。
if python3 - "$WORK/ui-subset.otf" "$WORK/sub_cmap.txt" <<'PY'
import sys
from fontTools.ttLib import TTFont
path, cmap_file = sys.argv[1], sys.argv[2]
mine = {int(tok, 16) for tok in open(cmap_file, encoding="ascii").read().split()}
theirs = set(TTFont(path, lazy=True).getBestCmap().keys())
if mine != theirs:
    only_mine = sorted(mine - theirs)[:10]
    only_theirs = sorted(theirs - mine)[:10]
    sys.exit(f"两条读数不一致：只有本脚本解析器认为有 {[hex(c) for c in only_mine]}；"
             f"只有 fontTools 认为有 {[hex(c) for c in only_theirs]}")
print(f"两条独立读数一致：fontTools 与脚本自带解析器都认为子集覆盖 {len(theirs)} 个码位")
PY
then :; else die "产物覆盖判据的两条读数不一致（原因见上一行）"; fi

# 许可声明必须**随字体走**（OFL 条件 2 的机器可读那一半，见文件头）。
python3 "$helper" names "$WORK/ui-subset.otf" || die "子集里缺版权/OFL 声明（--name-IDs='*' 没生效？）"

# ---------------------------------------------------------------------------
# 9. 安装：先写临时文件再 `mv`（同目录内 rename 是原子的），**内容相同就不碰**
# ---------------------------------------------------------------------------
install_if_changed() {  # install_if_changed <src> <dst> <label>
  local src="$1" dst="$2" label="$3"
  if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
    note "${label} 已是最新（逐字节相同），不重写"
    return 0
  fi
  mkdir -p "$(dirname "$dst")"
  local tmp="$dst.tmp.$$"
  cp "$src" "$tmp"
  mv -f "$tmp" "$dst"
  note "已写入 ${dst}（${label}）"
}
install_if_changed "$WORK/ui-subset.otf" "$OUT_FONT" "内嵌子集"
install_if_changed "$SRC_LICENSE" "$OUT_LICENSE" "OFL 1.1 全文（壳侧）"
install_if_changed "$SRC_LICENSE" "$CORE_LICENSE" "OFL 1.1 全文（内核侧副本）"

# 落位后再核一次（写入过程中被改动 / 落错地方都会在这里现形）。
[ "$(shasum -a 256 "$OUT_LICENSE" | awk '{print $1}')" = "$FONT_LICENSE_SHA256" ] \
  || die "落位后的 ${OUT_LICENSE} 与官方 LICENSE 不再逐字节相同（写入过程被改动？）"
[ "$(shasum -a 256 "$CORE_LICENSE" | awk '{print $1}')" = "$FONT_LICENSE_SHA256" ] \
  || die "落位后的 ${CORE_LICENSE} 与官方 LICENSE 不再逐字节相同（写入过程被改动？）"

# ---------------------------------------------------------------------------
# 9.5 登记闭环（产物）：入库的这份子集必须就是 `SUBSET_SHA256` 那一份
# ---------------------------------------------------------------------------
# ⚠️ 与 `fetch_windows_aria2c.sh` 第 9 步同一件事：**"产物换了、常量忘抄"要挪到本机当场
#    发现**。这条常量是 `--check` 用来判"资产有没有被人手工换过"的（见那里的说明），
#    所以它必须与实物一致 —— 不一致就不是"核对通过"，是"核对没做"。
# ⚠️ 换一个 fontTools 版本、或换了字体版本，都会让产物字节变 ⇒ 走到这里。那时**不是**
#    让你去改判据，是让你确认这次变化是有意的，然后把新值抄进常量。
GOT_SUBSET_SHA="$(shasum -a 256 "$OUT_FONT" | awk '{print $1}')"
if [ "$GOT_SUBSET_SHA" != "$SUBSET_SHA256" ]; then
  {
    echo ""
    echo "⚠️ 子集已刷新，但脚本头部的 SUBSET_SHA256 没跟上（或写法变了）。"
    echo "   把下面这行抄进脚本头部（替换原来那行 SUBSET_SHA256=\"…\"）："
    echo ""
    echo "    SUBSET_SHA256=\"$GOT_SUBSET_SHA\""
    echo ""
    echo "   为什么必须抄：这条常量是 \`--check\` 判\"资产有没有被人手工换过\"的依据；"
    echo "   不抄的话，下一次 \`test.sh\` 会以\"资产来路不明\"为由红在这里（那也算响亮，"
    echo "   但根因会指错方向）。"
  } >&2
  exit 3
fi
echo "==> 登记一致：入库子集的 sha256 就是 ${SUBSET_SHA256}"

# ---------------------------------------------------------------------------
# 9.6 交付副本（`windows/web/fonts/`）：**它不在上面任何一步里**
# ---------------------------------------------------------------------------
# 生成模式只写 `windows/assets/` 那两份，而交付页加载的是 `windows/web/fonts/` 的副本
# （理由与完整判据见 `check_delivery_twins()` 上面那一段）。⇒ 生成模式**必须自己说**
# 这件事：不然"重跑生成模式之后忘了重拷"就是一次**只在客户屏幕上可见**的漂移。
# ⚠️ 位置在 9.5 **之后**：9.5 判的是"这份产物是不是我们登记的那一份"（资产的身份），
#    它是这一条的前提 —— 资产身份没落定之前，谈"副本跟没跟上"没有意义。
#    ⇒ 今天这条调用点**够不到**（本树在 9.5 就红了：已知的"资产不幂等"，
#      按裁决不刷新资产）；它会在**下一次有人合法地刷新资产时**第一次生效。
check_delivery_twins

# ---------------------------------------------------------------------------
# 10. 登记闭环：规格里的体积必须与本脚本刚产出的这份一致
# ---------------------------------------------------------------------------
# ⚠️ 与 aria2 那份脚本第 9 步同一件事：**"资产换了、文档忘改"要挪到本机当场发现**，
#    否则它要等到有人读规格时才发现 —— 而"读规格的人正好在核对体积"这件事不可依赖。
#    ⚠️ **两处**都要判：§6.3 的正文写字节数，§6.3 的资产清单里写 KB 数 ——
#    它们**是同一个数字的两份副本**。上一版只 grep 了字节那一处，却在对读者说"另一处
#    也写了同一个数字，一并核对" —— **那是一句指令，不是一条判据**（复审 2026-09-18
#    次要 #7）：没有任何东西会拦住另一处那个数字烂掉。现在两处各自有判据。
#
# ⚠️⚠️ **规格的路径 2026-09-20（任务 9b）改过一次，而且那次是一条已经发生的回归**：
#    这里原来指的是 `2026-09-18-windows-client-design.md`，那份文件随两代已废文档被删了
#    （`6e71cac`）⇒ 下面那个 `[ -f "$SPEC" ]` **恒假**、整段判据**永远不会响**，
#    而**没有任何东西会说它不响了** —— 静默降级（W-2）的教科书形态。
#    它已经造成后果：合并任务 4/5/6 后子集是 190184 B，而规格里还写着 "179 KB"。
#    ⇒ 现在的口径：**路径指现行规格；文件不在场就 `die`，绝不跳过**（见下面的说明）。
#    **教训**：删除一个被判据/脚本读的文件时，"它是不是文档"不是判据，
#    **"有没有程序在读它"才是** —— 清理时只扫了文档之间的引用，没扫代码对文档的引用。
SPEC="$REPO/docs/superpowers/specs/2026-09-20-windows-tauri-client-design.md"
# 🔴 **不许**把这一段改回"文件不在就跳过"（`if [ -f "$SPEC" ] … else 悄悄放过`）——
#    那正是上面那条回归的形状：判据在"文件不在场"时**安静地不执行**，
#    而读者看到的仍然是一句"完成"。缺文件只有两种可能，**两种都要人来处理**：
#      · checkout 不完整（漏了 docs/）⇒ 补文件，**不是**放过判据；
#      · 规格被改名/搬走 ⇒ 把 SPEC 改到新家（一次能被审查的决定）。
#    ⇒ 用 `die`（退出码 1 = 环境/IO），理由是"判据没有对象可判"，
#      而"没有对象可判"与"判过了、没问题"在输出上**长得一模一样**。
if [ ! -f "$SPEC" ]; then
  die "找不到规格文件 ${SPEC} —— 第 10 步（体积登记闭环）**无法执行**，不会跳过。
      这一步判的是：**规格 §6.3 里写的子集体积，与刚产出的这份实物是否一致**
      （两处：正文里的字节数、资产清单里的 KB 数）。
      它为什么不能没有那个文件：这条判据的**对象就是那份文件里的两行字** ——
      文件不在场，判据无对象可判；而"无对象可判"与"判过了"在输出上分不开，
      于是"资产换了、文档忘改"会**一路静默到客户手里**（本仓库明令禁止的 W-2）。
      补救（二选一）：
        · checkout 不完整 ⇒ 把 docs/ 补齐（这条判据要读的就是它）；
        · 规格被改名/搬走 ⇒ 把本脚本的 SPEC 指到新家，并核一遍那两行的形状
          （见下面两处 grep 的注释）。"
fi

# §6.3 资产清单里那一行：**行首缩进 + `ui-subset.otf`**，写的是 KB（整数除法，向下取整）。
# 用行首锚定是为了把它与同一节那段散文区分开（后者也含这个文件名）。
SUBSET_KB="$((SUBSET_BYTES / 1024))"
ok_bytes=0; ok_kb=0
# 中间用 `.*` 而不是一个空格：那一行的写法（反引号、加粗、括号）以后会变，
# 判据不该因为排版调整而假红 —— 它要判的是"数字对不对"，不是"排版长什么样"。
grep -q "ui-subset\.otf.*${SUBSET_BYTES} B" "$SPEC" && ok_bytes=1
grep -qE "^[[:space:]]+ui-subset\.otf.*${SUBSET_KB} KB" "$SPEC" && ok_kb=1
if [ "$ok_bytes" -eq 1 ] && [ "$ok_kb" -eq 1 ]; then
  echo "==> 登记一致：§6.3 写的 ${SUBSET_BYTES} B（= ${SUBSET_KB} KB）与实物相符"
else
  {
    echo ""
    echo "⚠️ 子集已刷新，但**规格里的体积没跟上**（或写法变了）。两个判据各自的结论："
    if [ "$ok_bytes" -eq 1 ]; then
      echo "    §6.3 正文（字节数 ${SUBSET_BYTES} B）：✓"
    else
      echo "    §6.3 正文（字节数 ${SUBSET_BYTES} B）：✗ 没找到 —— 那一行要含 \`ui-subset.otf\` 与这个字节数"
    fi
    if [ "$ok_kb" -eq 1 ]; then
      echo "    §6.3 资产清单（${SUBSET_KB} KB）：✓"
    else
      echo "    §6.3 资产清单（${SUBSET_KB} KB）：✗ 没找到 —— 那一行要**以缩进 + \`ui-subset.otf\` 开头**，"
      echo "                                            且含 \`${SUBSET_KB} KB\`（整数除法向下取整，**别抄 \`ls\`**："
      echo "                                            \`ls\` 是四舍五入，两者会差 1）"
    fi
    echo "    规格里现行的相关行（§6.3 各处）："
    grep -n "ui-subset.otf" "$SPEC" | sed 's/^/      /' >&2
    echo "    改完重跑本脚本（生成模式会顺带把几处登记都核一遍）。"
  } >&2
  exit 3
fi

# ---------------------------------------------------------------------------
# 11. 打印登记（给人看，也给报告抄）
# ---------------------------------------------------------------------------
cat >&2 <<EOF

==> 完成。登记如下（可直接抄进报告）：
    字体        Noto Sans SC Regular（Noto Sans CJK ${FONT_RELEASE_TAG}，name ID 5 自报 ${FONT_VERSION}）
    来源        ${FONT_RELEASE_API}
    资产        ${FONT_ASSET_NAME}  ${FONT_ZIP_SIZE} B  sha256 ${FONT_ZIP_SHA256}
    取出的字体  ${FONT_OTF_MEMBER}  ${FONT_OTF_SIZE} B  sha256 ${FONT_OTF_SHA256}
    许可        ${FONT_LICENSE_MEMBER}  ${FONT_LICENSE_SIZE} B  sha256 ${FONT_LICENSE_SHA256}
                → windows/assets/OFL-1.1.txt 与 core/assets/COPYING-OFL-1.1.txt（两份逐字节相同）
    子集        windows/assets/ui-subset.otf  ${SUBSET_BYTES} B  sha256 $(shasum -a 256 "$OUT_FONT" | awk '{print $1}')
    工具        fonttools $(python3 -c 'import fontTools; print(fontTools.version)' 2>/dev/null || echo '（版本取不到）')
EOF
