"""字体覆盖判据的核心 —— check_design_fonts.sh 的唯一实现。

⚠️ 本文件与 check_design_fonts.sh 是**一对**，头部的取舍说明在那份 bash 里，别在这里重写一遍。
"""
import hashlib
import os
import re
import sys

# 与字符集抽取**同一份**的注释剥离规则（见 check_design_fonts.sh 头部"判据"一节）。
# ⚠️ 宁可少剥、不可多剥：多剥会把"其实会渲染的字"从要求集里拿掉，而那正是静默失守。
_HTML_COMMENT = re.compile(r"<!--.*?-->", re.S)
_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)
# `//` 前面是 `:` 时**不是**注释（`https://…`、`data:` 那类 URL 的形状）。
# ⚠️ 这仍是一个**启发式**，不是 JS 词法：`"a // 中文"` 这种**串里的** `//` 还是会挨剥。
#    要彻底消掉得写词法分析器 —— **A0 不做**，阶段 A 把 JS 接进判据时再回来定。
#    方向是**宁可少剥**：少剥只是多抽（有人工复核兜底），多剥是静默漏报。
_LINE_COMMENT = re.compile(r"(?<!:)//[^\n]*")


def blank_comments(text, pattern):
    r"""把匹配到的注释换成**等量换行**：字符消失，但**行号不偏**。

    ⚠️ 开头的 `r` 是**必需的**，不是风格：本 docstring 里写了 `/* … */` 这个字形，
    普通字符串会把它当转义序列 ⇒ `SyntaxWarning: invalid escape sequence '\*'`。
    本仓库是**零告警**口径，一条 `SyntaxWarning` 就该红。

    ⚠️ 为什么不直接 `sub("", …)`：行号是给人改字用的（`find_missing` 要报
    `文件:行号`）。直接删会把后面所有行往上挪，报出来的行号就指错地方了。

    ⚠️ **为什么不"按行剥"**（这是本文件第一版栽过的坑，别再栽一次）：
    `/* … */` 常常**跨行**，而逐行做正则时起止符不在同一行 ⇒ 块注释**内部那些行
    一个字都没剥掉**。样张是单文件、CSS 内联，那些 CSS 注释**全是多行中文散文** ——
    按行剥的后果是判据对着它自己要守的文件开几十处假火，然后**被脱敏**
    （本仓库最怕的形态，同 `make_font_subset.sh` 头部那段）。⇒ **整份源码上剥一次。**
    """
    return pattern.sub(lambda m: "\n" * m.group(0).count("\n"), text)


def rendered_text(path, source):
    """剥掉注释之后剩下的文本（**行号与原文逐行对齐**）。

    按扩展名选规则；**认不出的扩展名一律不剥**（多抽，安全的那一侧）。
    """
    ext = os.path.splitext(path)[1].lower()
    if ext == ".html":
        # 剥两种：HTML 注释 + 内联 CSS 的块注释（理由见 check_design_fonts.sh 头部）。
        #
        # ⚠️ **顺序不是"哪一版承重"——两版各有一个会吃掉正文的输入，是对称的**：
        #   · 先 HTML 后块：输入 `/* x <!-- y */ 正文` ⇒ `<!-- … -->` 先匹配到
        #     `<!-- y */ 正文`，**把正文一起吞掉**（少抽 ⇒ 判据漏报豆腐块）；
        #   · 先块后 HTML：输入 `<!-- a /* b --> 正文 */` ⇒ `/* … */` 先匹配到
        #     `/* b --> 正文 */`，同样吞掉正文。
        #   ⇒ 谁都不比谁安全。本计划选**先块注释**，两条理由：
        #     ① 样张里两种注释体**互不嵌套**，两版输出**逐字节相同**（实测）；
        #     ② "HTML 注释里写 `/*`"比"CSS 注释里写 `<!--`"更常见，先剥块注释
        #        对这一侧的输入更稳。
        # ⚠️ 要彻底消掉这个不确定性，得改成"从左往右扫、碰到哪种注释起点就跳到它的终点"
        #    —— **A0 不做**（样张里不存在嵌套）。**阶段 A 若引入嵌套注释，回来改这里。**
        #    `--selftest` 里有一条夹具钉住了这个选择（见 `cases` 里那条「两种注释标记嵌套」的夹具）。
        return blank_comments(blank_comments(source, _BLOCK_COMMENT), _HTML_COMMENT)
    if ext == ".css":
        return blank_comments(source, _BLOCK_COMMENT)
    if ext == ".js":
        return blank_comments(blank_comments(source, _BLOCK_COMMENT), _LINE_COMMENT)
    return source   # 认不出的类型：整份都算要求集（多抽，安全的那一侧）


def cmap_of(path):
    from fontTools.ttLib import TTFont   # 延迟 import：让缺 fontTools 时的报错由 bash 那层给
    return set(TTFont(path, lazy=True).getBestCmap().keys())


# `@font-face { … src: url(…) … }` 里的那串 URL（引号可有可无）。
_FONT_FACE_URL = re.compile(r"@font-face\s*\{[^}]*?url\(\s*[\"']?([^\"')]+)", re.S)

# 交付面：`shell-win/tauri.conf.json` 的 `build.frontendDist = "../web"` ⇒ 运行时
# **只有 `windows/web/` 下的文件**是 webview 取得到的（它就是 webview 的根）。
# 这个常量是本文件自己的位置推出来的（`<repo>/windows/scripts/check_design_fonts.py`），
# 不依赖调用者的 cwd。`--selftest` 会把它换成夹具目录（见 `font_disagreements` 的入参）。
WEB_ROOT = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "web"))


def sha256_file(path):
    """文件的 sha256（十六进制）。字体就 190 KB 上下，整份读进来算没有负担。"""
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest()


def _inside(path, root):
    """`path` 是否在 `root` 之内（两者都已 normpath + abspath）。

    ⚠️ 不能写成 `path.startswith(root)`：`/a/bc` 会以 `/a/b` 开头却不是它的子路径
    —— 那种误判会**放行**一条逃出交付目录的声明（判据要朝"多报"的方向犯）。
    """
    return path == root or path.startswith(root + os.sep)


def font_disagreements(paths, font_path, web_root=None):
    """页面声明的 `@font-face` 里，**有没有一条**就是本判据正在比对的那一份字体？

    ⚠️ **为什么需要这条**：这份字体在**两个地方各有一份** ——
    本脚本从 `$FONT`（`windows/assets/ui-subset.otf`）读它，页面用
    `url("./fonts/ui-subset.otf")` 加载**交付副本**。两者必须逐字节相同，
    而在此之前**没有任何东西校验它们一致**：阶段 A 换成 `ui-subset-v2.otf`
    而只改了一处 ⇒ 判据对着**另一份字体**开火、**照样全绿**，而页面上会出现豆腐块 ——
    那正是这份判据存在的唯一理由被整个绕过，失败形态与"没有判据"完全一样。

    🔴 **判据的锚点 2026-09-20 从"路径"换成了"sha256"（任务 18）——理由是一次实测**：
      · **旧判据在交付布局下是哑的**，两条读数（审查者与实现者各跑一遍，逐字相同）：
        `windows/web/**`（24 个文件）⇒ **退出 3**；`test.sh` 的口径（25 个文件，
        多一个 `windows/design/a0-proof.html`）⇒ **退出 0**。
        ⇒ 25 个文件里唯一满足旧判据的是**样张**那条 `../assets/ui-subset.otf`
        （`windows/design/` 下只有它一个文件被扫到），而**交付页 `app.css` 自己那条
        `./fonts/ui-subset.otf` 从来没有满足过它** —— 也就是说：这条守卫认可的
        是样张的声明，不是交付页的声明。它**既不管交付面，又会因样张改动假红**
        （样张是被明确预期会改的：`windows/design/README.md` 第 ① 与第 ⑤ 条）。
      · ⇒ 锚点改成"**页面声明解析到的那个文件，与判据这份的 sha256 相同**"。
        一次同时补两个洞：(a) 交付页那条声明**从此真的被判**（`web/fonts/` 与
        `assets/` 同 sha ⇒ 一致）；(b) 那份交付副本若停在上一个子集（"忘了重拷"，
        见 `make_font_subset.sh --check` 里的同一条判据）则 sha 不同 ⇒ **红**。

    ⚠️ **判的是"至少一条"，不是"每一条"**（本函数第三版仍然保留这条）：
    规格 §9.2 要求阶段 A 把 Inter 子集也打包进来，那天页面上会**合法地**出现
    多条 `@font-face`。若按"每一条都必须指向判据那份字体"判，判据会在阶段 A
    第一天开始常态性误报，然后**被人关掉** —— 假红的下场就是脱敏。
    这条守卫真正要挡的是"页面加载的字体与判据比对的字体**不是同一份**"。

    🔴 **两条收紧**（都是任务 18 加的，理由各是一条"绿而坏"的路）：
      ① **交付面内**（`windows/web/` 下的文件）：**当被扫的文件（`paths`）里有交付面
         文件时**，算数的那一条声明**必须来自交付面**。少了这一条，样张（一个不随交付物
         发货的文件）的那条声明可以在 `test.sh` 的口径下**替**交付页那条背书 ——
         交付页的字体坏了也照样全绿（那是旧判据的失败形态的另一种写法）。
         ⚠️ **判据是"交付面文件在不在被扫的集合里"，不是"交付面里有没有 `@font-face`"**
         —— 后者会让"交付页把声明整个删掉"变成一条绿路（复审 I-1 实测）。
         实现见下面 `scanned_web` 那一行，夹具⑧钉着它。
         ⚠️ 只扫 `design/`（设计期单独看样张）时**退回**"至少一条"的旧口径：
         样张那句 `../assets/…` 是**对的**（样张不走 webview 的根），不该因此变红。
      ② **交付面内不许逃出交付面**：`windows/web/` 下的声明若解析到 `web/` 之外
         （典型：`../assets/ui-subset.otf`），**无条件红**。理由：webview 的根就是
         `web/`，`../` 会被 URL 标准的 remove dot segments 夹回根 ⇒ 请求
         `/assets/ui-subset.otf` ⇒ **404 ⇒ 静默回退系统字体**，而页面"看着也能看"。
         ⚠️ 这正是旧判据下最省事的那条**骗绿**路（把 url 改成 `../assets/…`
         就与判据"一致"了，而屏幕上加载的根本不是那一份）。
         同类：交付面内的声明解析到一个**不存在**的文件，一样无条件红。

    ⚠️ **已知残留（写在这里免得它再悄悄消失）**：
    调用方一个文件都没给 `@font-face` 时，本函数返回"一致"（绿）。
    对阶段 A 的多文件集合，`@font-face` 可能只在其中一个文件里，
    所以"必须有至少一条"这条不变量**不能**在这里强加 —— 那要等阶段 A
    把被扫描的集合定下来之后再定。今天它是对的（交付页与样张各自声明了
    `@font-face`），但别以为这里已经被守住了。

    `data:` URI 跳过（内联字体没有路径可对；真要用它，这条要另写）。

    ⚠️ **返回值是 `(rows, 一致吗)` 两元组**，不是"空列表即一致"：调用方要能区分
    "一致（绿）"与"不一致（红）"，而**红**有两种形态（"一条算数的都没有"与
    "有声明在交付面里取不到"）——两者的补救不同，报错话术也要分开说，
    所以"为什么红"必须能传出去，不能只传一个空的诊断列表。

    `rows` = `[(声明的文件, url, 解析到的绝对路径, 这一条怎么样)]`，
    **把全部声明都列出来**当诊断材料（第 4 段里带 ✓ 的就是算数的那一条）。
    """
    root = os.path.normpath(os.path.abspath(web_root or WEB_ROOT))
    want_sha = sha256_file(font_path)
    declared = []   # [(声明的文件, url, 解析到的绝对路径)]
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            source = fh.read()
        for url in _FONT_FACE_URL.findall(source):
            if url.startswith("data:"):
                continue
            resolved = os.path.normpath(
                os.path.join(os.path.dirname(os.path.abspath(path)), url))
            declared.append((os.path.abspath(path), url, resolved))
    if not declared:
        return [], True   # 已知残留：一条声明都没有 ⇒ 一致（见 docstring 末段）
    # ⚠️ **"被扫的集合里有交付面文件吗"看的是 `paths`（被扫的文件），不是 `declared`
    #    （有 `@font-face` 的那些文件）** —— 这是任务 18 的复审抓出来的一处
    #    "实现比自述的窄"：写成 `declared` 的话，**交付页把 `@font-face` 整个删掉**时
    #    `scanned_web` 会是 False ⇒ 样张那条声明**照样算数 ⇒ 绿**，而后果是
    #    "页面回退系统字体、所有判据全绿"——正是这条收紧要堵的形态。
    #    夹具⑧（`_font_agreement_fixtures` 里那条「交付页丢了 @font-face」）钉的就是它：
    #    把这一行改回 `declared`，`--selftest` 立刻红。
    scanned_web = any(_inside(os.path.normpath(os.path.abspath(p)), root) for p in paths)

    rows, qualified, hard = [], [], []
    for src, url, resolved in declared:
        src_in_web = _inside(src, root)
        if src_in_web and not _inside(resolved, root):
            # 收紧②：交付面内的声明逃出了交付面 ⇒ 运行时取不到（无条件红）
            why = ("它落在交付目录之外 —— webview 的根就是 `windows/web/`，"
                   "`../` 会被 URL 规则夹回根 ⇒ 运行时 **404 ⇒ 静默回退系统字体**")
            hard.append((src, url, resolved, why))
        elif not os.path.isfile(resolved):
            why = "这个文件不存在（运行时 404 ⇒ 静默回退系统字体）"
            hard.append((src, url, resolved, why))
        elif sha256_file(resolved) != want_sha:
            why = "它的 sha256 与判据这份不同（**内容不是同一份字体**）"
        elif (not scanned_web) or src_in_web:
            why = "与判据这份逐字节相同 ✓"
            qualified.append((src, url, resolved, why))
        else:
            # ⚠️ 这一条是**算数的那一条的候选**（sha 相同），却被收紧① 挡在门外：
            #    它不是交付面里的文件，而这次被扫的集合里**有**交付面文件。
            #    这里必须**说清它为什么不算数** —— 旧写法给它一个 `✓` 却仍然判红，
            #    读者会以为判据自相矛盾（本项目对"报出来的东西读得通"有要求）。
            why = ("它**与判据这份逐字节相同**，但**不在交付面内**：这次被扫的集合里有 "
                   "`windows/web/` 下的文件 ⇒ 算数的那一条**必须来自交付面**（收紧①），"
                   "否则一个不随交付物发货的文件（样张）会替交付页背书")
        rows.append((src, url, resolved, why))

    if hard or not qualified:
        # 没有算数的一条 / 有硬缺陷 ⇒ 红，并把**全部**声明列出来（诊断材料）。
        return rows, False
    return rows, True


def _font_agreement_fixtures(font_path):
    """`font_disagreements()` 的夹具（`--selftest` 的一部分）。

    ⚠️ 它们钉的是**锚点与两条收紧**（见 `font_disagreements` 的 docstring）——
       夹具自己造一棵**假的**交付树（`<tmp>/web`、`<tmp>/design`、`<tmp>/assets`），
       所以不需要碰真的 `windows/web/`（`web_root` 由入参注入）。
    ⚠️ "判据本身也要能被证伪"：每一条都写清"这一条在验什么"，以及期望红/绿。
    """
    import tempfile
    out = []
    with tempfile.TemporaryDirectory() as d:
        web, design, assets = (os.path.join(d, n) for n in ("web", "design", "assets"))
        for p in (web, design, assets):
            os.makedirs(p)
        # 判据那份（`$FONT`）与它的交付副本：夹具用**任意字节**当字体 ——
        # 本判据只算 sha256、不解析 cmap（那正是"锚点换成 sha256"的意思）。
        good = os.path.join(assets, "ui-subset.otf")
        with open(good, "wb") as fh:
            fh.write(b"FONT-A")
        stale = os.path.join(web, "fonts", "ui-subset.otf")

        def write(path, text):
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
            return path

        def copy_font(src, dst):
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(src, "rb") as a, open(dst, "wb") as b:
                b.write(a.read())

        def run(targets):
            """夹具只关心"一致吗" ⇒ 取两元组的第二个（`rows` 是诊断材料）。"""
            return font_disagreements(targets, good, web_root=web)[1]

        # ① 交付页声明交付副本，两份逐字节相同 ⇒ 一致（**这一条就是今天树上的形态**）
        copy_font(good, stale)
        page = write(os.path.join(web, "app.css"),
                     '@font-face { src: url("./fonts/ui-subset.otf"); }')
        out.append((run([page]), "交付页 + 同 sha 的交付副本 ⇒ 必须一致（绿）"))
        # ② 交付副本是**上一个子集**（"忘了重拷"）⇒ 必须红（旧判据在这里是绿的）
        with open(stale, "wb") as fh:
            fh.write(b"FONT-OLD")
        out.append((not run([page]), "交付副本与判据那份 sha 不同 ⇒ 必须红（忘了重拷）"))
        copy_font(good, stale)
        # ③ 交付页声明 `../assets/…`：路径解析到判据那份、sha 也相同，
        #    但**逃出了交付面** ⇒ 必须红（这是旧判据下最省事的骗绿路）
        escape = write(os.path.join(web, "escape.css"),
                       '@font-face { src: url("../assets/ui-subset.otf"); }')
        out.append((not run([escape]), "交付面内的声明逃出 `web/` ⇒ 必须红（运行时 404）"))
        # ④ 交付面内的声明指向一个不存在的文件 ⇒ 必须红
        dead = write(os.path.join(web, "dead.css"),
                     '@font-face { src: url("./fonts/nope.otf"); }')
        out.append((not run([dead]), "交付面内的声明指向不存在的文件 ⇒ 必须红"))
        # ⑤ 样张（不在交付面内）声明 `../assets/…`，只扫它一个 ⇒ 必须一致（绿）：
        #    样张不走 webview 的根，那句 url 是对的 —— 两条收紧都不该管它
        proof = write(os.path.join(design, "a0-proof.html"),
                      '@font-face { src: url("../assets/ui-subset.otf"); }')
        out.append((run([proof]), "只扫样张（不在交付面内）⇒ 必须一致（绿）"))
        # ⑥ **样张不能替交付页背书**：交付页那条坏了（指向不存在的文件），
        #    而集合里还有一条样张的合法声明 ⇒ 仍然必须红（收紧①的全部理由所在）
        out.append((not run([dead, proof]),
                    "交付页的声明坏了、集合里有样张的合法声明 ⇒ 必须红（样张不许替交付面背书）"))
        # ⑦ **交付页把 `@font-face` 整个删掉**（它还在被扫的集合里）⇒ 必须红。
        #    ⚠️ 这条是**唯一钉住收紧①**的夹具（复审 I-1 抓的洞）：⑥ 的红其实来自
        #    "指向不存在的文件"那条硬缺陷，把收紧① 单独回退（`scanned_web` 写成
        #    "有 `@font-face` 的文件在不在 web 里"）它照样红 —— 而**本条的形态**
        #    （交付面里一条声明都没有）在那种写法下是**绿**的，后果正是
        #    "页面回退系统字体、所有判据全绿"。⚠️ 它与末条"一个声明都没有 ⇒ 绿"
        #    的**已知残留不是同一条**：这里有一条声明（样张那条），只是它不算数。
        nodecl = write(os.path.join(web, "nodecl.css"), "body { margin: 0 }")
        out.append((not run([nodecl, proof]),
                    "交付页丢了 @font-face、集合里只剩样张的合法声明 ⇒ 必须红"
                    "（钉住收紧①：算数的那条必须来自交付面）"))
        # ⑧ 上一条的**正对照**：交付面那条声明在（同 sha）时，集合里**带着**样张
        #    也必须一致 —— 这条路径就是 `test.sh` 第 0.6 步的真实输入形状
        #    （24 个 web 文件 + 样张）。少了它，将来有人把收紧① 做成"集合里只要有
        #    design 文件就红"这种**过紧**的写法时，没有任何东西会响。
        out.append((run([page, proof]),
                    "交付页那条在 + 集合里带着样张 ⇒ 必须一致（绿）（`test.sh` 的输入形状）"))
        # ⑨ 集合里**一份声明都没有** ⇒ 一致（**已知残留**，见 docstring 末段；
        #    这条钉的是"它还是老样子"，不是"它是对的"）
        plain = write(os.path.join(web, "plain.css"), "body { margin: 0 }")
        out.append((run([plain]), "一个 @font-face 都没有 ⇒ 一致（**已知残留**，如实钉住）"))
    return out


# ⚠️ **只排除这两个码位**：它们是变体选择符（VS15 `U+FE0E` / VS16 `U+FE0F`）——
#    **没有字形、零宽度**，在任何字体里都渲染不出一个可见的字，所以**永远不会变成方块**；
#    缺了它们只是"退回文本呈现"（`⚠️` 仍然显示成 `⚠`）。
#    ⚠️ **理由的唯一一份写在 `make_font_subset.sh` 的 `EXEMPT_CODEPOINTS` 里** ——
#    这里**引用，不复述**（同一件事只有一处定义，抄一份就会漂移）。
#    ⚠️ **别顺手排别的**：那份豁免清单里还有一条（`U+1F642` `🙂`），它的理由是
#    「**永不进界面**」（只出现在一条 `#[test]` 字符串里），而**本脚本没有资格替它判** ——
#    "会不会进界面"只有那份清单说得了；本脚本判的是"进界面的字有没有字形"。
#    ⇒ `U+1F642` 若真出现在被扫文件里，**本判据照旧要红**（那是真的越界了）。
_VARIATION_SELECTORS = frozenset({0xFE0E, 0xFE0F})


def required_codepoints(paths):
    """这些文件的**要求集码位**（剥掉注释之后剩下的非 ASCII 字符，去掉变体选择符）。

    ⚠️ **它是"哪些字会渲染"这条规则的唯一一份**，两个调用方都用它：
      · 本文件的 `find_missing`（判"子集覆盖得了吗"）；
      · `make_font_subset.sh` 的生成模式（**经 `--chars` 模式**）—— 它要把这些字
        收进子集。在那之前生成模式**只扫三棵 `.rs` 树**，于是 `web/` 里的字
        **永远进不了子集、而判据也永远够不着那个文件**（一个已交付的页面会显示方块，
        而 `test.sh` 与 `build_windows.sh` 两条验收都是绿的）。
    """
    chars = set()
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            source = fh.read()
        for ch in rendered_text(path, source):
            if ord(ch) > 127:
                chars.add(ord(ch))
    return chars - _VARIATION_SELECTORS


def find_missing(paths, covered):
    """返回 [(文件, 行号, 字符)]。

    ⚠️ 顺序是**命令行给的顺序**（每个文件内部按行号升序），**不是**全局按文件名排序
    —— 别把这条 docstring 写成"按文件与位置排序"，那会让人以为输出是排过序的。
    观感上无所谓，但报告里逐条贴出来时，顺序与命令行一致才不会让人以为漏了文件。
    """
    out = []
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            source = fh.read()
        # ⚠️ **在整份源码上剥一次，然后逐行数** —— 不要"在原文上逐行剥"。
        #    后者对跨行块注释是漏的（`blank_comments` 的说明里有完整的坑），
        #    而那正是本文件第一版犯的错：`✗` 放在多行 `/* … */` 里时判据照样开火。
        #    剥完的文本与原文**逐行一一对应**（注释被换成等量换行），
        #    所以这里数出来的行号**就是原文的行号**，可以直接给人看。
        for lineno, line in enumerate(rendered_text(path, source).splitlines(), start=1):
            for ch in line:
                # ⚠️ 变体选择符与 `required_codepoints` 用**同一个**集合排除 ——
                #    两处若各写一份，"哪些字算数"就会在两个方向上分叉。
                if ord(ch) > 127 and ord(ch) not in covered and ord(ch) not in _VARIATION_SELECTORS:
                    out.append((path, lineno, ch))
    return out


def main():
    argv = sys.argv[1:]
    # ⚠️ **`--chars` 是一个"没有字体"的模式**，所以它在**读字体之前**就返回
    #    （`cmap_of` 要 fontTools，而这个模式不需要 —— `make_font_subset.sh --check`
    #    那条路是在**没有 fontTools 的机器上**也要能跑的）。
    #    用法：`check_design_fonts.py --chars <文件…>` ⇒ 每行一个 `U+XXXX`。
    #    它是生成模式与判据**共用同一份抽取规则**的那条通道（见 `required_codepoints`）。
    if argv and argv[0] == "--chars":
        targets = argv[1:]
        if not targets:
            print("check_design_fonts.py --chars: 没有给任何文件。", file=sys.stderr)
            return 2
        for cp in sorted(required_codepoints(targets)):
            print(f"U+{cp:04X}")
        return 0
    if not argv:
        print("check_design_fonts.py: 缺字体路径。", file=sys.stderr)
        return 2
    font, rest = argv[0], argv[1:]
    # ⚠️ **`mode` 是可选的，而且认不出的位置参数一律当"目标文件"** ——
    #    别写成 `font, mode = argv[1], argv[2]`：那样 `py <字体> a.html` 会
    #    把 `a.html` 当成 mode、于是 targets 为空、报"没有给任何文件"，
    #    而文件明明给了。这个陷阱是**实际撞到过**的（第 2 轮实现者手敲时撞上），
    #    而"靠人记得传空串占位"是纪律、不是结构。
    mode = ""
    if rest and (rest[0].startswith("--") or rest[0] == ""):
        mode = rest.pop(0)
    # ⚠️ 认不出的 `--xxx` 必须**报错**，不许"吃掉接着往下走"：
    #    否则 `py <字体> --a.html b.html` 会把 `--a.html` 当 mode 悄悄丢掉，
    #    判据只看了 `b.html` 却报"通过" —— 一个给了却没被检查的文件不留任何痕迹。
    #    空串是**刻意**留的占位（见上面那段"别写成 `font, mode = argv[1], argv[2]`"）。
    if mode not in ("", "--selftest"):
        print(f"check_design_fonts.py: 认不出的参数 `{mode}`。", file=sys.stderr)
        return 2
    targets = rest
    covered = cmap_of(font)
    if mode == "--selftest":
        return selftest(font, covered)
    # ⚠️ **一个文件都没给 ⇒ 拒绝返回绿。**
    #    `find_missing([])` 恒返回 `[]`，于是"没给文件"会被报成"判据通过"——
    #    那是**假绿**：一个没跑过的判据说自己通过了，而且没有任何东西会提醒。
    #    `.sh` 那层虽然挡着这种情况（它的空参数分支 `exit 2`），但**本函数不依赖调用方守规矩**
    #    ——阶段 A 若有人直接调 `.py`、或用一条没匹配到任何文件的 glob 驱动它，
    #    这道守卫就是唯一的拦阻。
    if not targets:
        print("check_design_fonts.py: 没有给任何文件 —— 拒绝返回绿。", file=sys.stderr)
        return 2
    # ⚠️ **先查"判的是不是同一份字体"**：它比字符覆盖更靠上游 ——
    #    判错了字体，覆盖率说得再对也没有意义（页面加载的是另一份）。
    rows, agreed = font_disagreements(targets, font)
    if not agreed:
        sys.stdout.flush()
        print("", file=sys.stderr)
        # ⚠️ 标题**不写成"没有一条指向判据那份"**：红的形态有两种（"一条算数的都没有"
        #    与"有声明在交付面里取不到"），两种都走这条路。写成前者会在后一种情况下
        #    **把根因说错**（本项目对"根因不许说错"有纪律：守卫给出的理由必须是它
        #    真正检查到的东西）—— 所以标题只说"声明有问题"，逐条的理由由下面的
        #    `why` 那几行给（那才是各自真正检查到的东西）。
        print("字体判据没过：目标文件的 @font-face 声明有问题（逐条见下；"
              "判据比的是**字节**，不是路径名）。", file=sys.stderr)
        for path, url, resolved, why in rows:
            print(f"    {os.path.relpath(path)}：声明 `{url}` ⇒ {resolved}", file=sys.stderr)
            print(f"        {why}", file=sys.stderr)
        print(f"    而本判据比对的是：{font}（sha256 {sha256_file(font)}）", file=sys.stderr)
        print("补救（按情况选一条）：", file=sys.stderr)
        print("  · 页面加载的**就是**这一份字体（正常情形）⇒ 把交付副本与其源文件对齐：", file=sys.stderr)
        print("      cp windows/assets/ui-subset.otf windows/web/fonts/ui-subset.otf", file=sys.stderr)
        print("    （重跑 `bash windows/scripts/make_font_subset.sh` 之后必须做这一步 ——", file=sys.stderr)
        print("      同一件事还有一条判据在 `make_font_subset.sh --check` 里守着，两处一起红）", file=sys.stderr)
        print("  · `src` 指向的是**另一份**字体（阶段 A 的 Inter 子集那一类）⇒ 那是对的：", file=sys.stderr)
        print("    本判据只要求**至少一条**算数；同时保留一条指向交付副本的声明即可。", file=sys.stderr)
        print("  · 交付页的 `src` 写到了 `windows/web/` 之外（例如 `../assets/…`）⇒ **改回来**：", file=sys.stderr)
        print("    webview 的根就是 `windows/web/`，`../` 取不到、404 后静默回退系统字体。", file=sys.stderr)
        return 3
    missing = find_missing(targets, covered)
    if not missing:
        print(f"字体覆盖判据通过：{len(targets)} 个文件里每个非 ASCII 字符都在 "
              f"{os.path.basename(font)} 的 cmap 里。")
        return 0
    # ⚠️ 先冲 stdout 再写 stderr：两股流在被管道接走时缓冲方式不同，
    #    不冲的话日志里结论会跑到读数前面（同 make_font_subset.sh 的同一条纪律）。
    sys.stdout.flush()
    print("", file=sys.stderr)
    print(f"覆盖判据没过：{len(missing)} 处字符**不在**子集里，它们会显示成方块：", file=sys.stderr)
    for path, lineno, ch in missing[:30]:
        rel = os.path.relpath(path)
        print(f"    {rel}:{lineno}  U+{ord(ch):04X} {ch}", file=sys.stderr)
    if len(missing) > 30:
        print(f"    …（其余 {len(missing) - 30} 处同类，从略）", file=sys.stderr)
    print("补救（两条，按情况选）：", file=sys.stderr)
    print("  · 这个字**该出现在界面上** ⇒ 让它进要求集：把该文案挪进被扫描的源码字面量，", file=sys.stderr)
    print("    然后重跑 `bash windows/scripts/make_font_subset.sh`（生成模式，需联网）。", file=sys.stderr)
    print("  · 这个字**不该在这里**（打错了/是注释漏剥） ⇒ 改文案。", file=sys.stderr)
    return 3


def selftest(font_path, covered):
    """夹具自检：判据本身也要能被证伪（同 make_font_subset.sh 的两向闭环）。

    ⚠️ **两组夹具**，各自钉一件事，别把它们的结论混起来读：
      · `cases`（下面这一段）钉**字符抽取与注释剥离**（`find_missing`）；
      · `_font_agreement_fixtures` 钉**字体一致那条判据的锚点与两条收紧**
        （`font_disagreements`，见那份 docstring）。后者是任务 18 加的 ——
        那条判据的口径刚改过一次，而"改判据"这件事**自己也要有判据**：
        没有夹具的话，下一个人把锚点改回"路径相等"、或把两条收紧删掉，
        自检照样全绿（那不正是这一整条判据存在的理由被绕过吗）。
    """
    import tempfile
    # ⚠️ 挑一个**确知**不在子集里的字符当"必须红"的样本：`✗` U+2717。
    #    证据在 make_font_subset.sh 头部："(实测试 78/112、87/112、154/192，`✗`U+2717 也缺)"
    #    —— 并且它的 EXTRA_CODEPOINTS 里只收了 `✓`U+2713、**没有**收 U+2717。
    BAD = "✗"
    assert ord(BAD) not in covered, (
        f"自检的前提失效了：U+2717 竟然在子集里。"
        f"换一个确知不在子集里的字符再跑（见 make_font_subset.sh 的 EXEMPT_CODEPOINTS）。")

    # (夹具内容, 扩展名, 期望是否有缺口, 这一步在验什么)
    #
    # ⚠️ 扩展名**显式给出，不做推断**。上一版按正文前两个字符猜
    #    （`//`→.js、`/*`→.css、其余→.html），结果有一条夹具被猜成 `.css`，
    #    而 `.css` 分支根本不剥 HTML 注释 ⇒ 它**分辨不了**自己声称要钉的那个顺序
    #    （实测：把实现换成反序，它照样绿）。夹具的扩展名是**承重的**，
    #    承重的东西不许靠猜。
    cases = [
        # (夹具内容, 扩展名, 期望是否有缺口, 这一步在验什么)
        ("<p>文件 · 已完成</p>", ".html", False, "全是被覆盖的字 ⇒ 必须绿"),
        (f"<p>状态：{BAD}</p>", ".html", True, "有个子集外的字 ⇒ 必须红且指名它"),
        (f"<!-- 备注里的 {BAD} --><p>好</p>", ".html", False,
         "HTML 注释里的字**不**算 ⇒ 必须绿（剥离生效）"),
        (f"<style>/* 内联 CSS 里的 {BAD} */</style><p>好</p>", ".html", False,
         "内联 CSS 块注释里的字**不**算 ⇒ 必须绿（**这条最要紧**：样张是单文件，"
         "CSS 内联，而它的注释全是中文散文）"),
        (f"<style>/* 注释跨行\n     这里有个 {BAD}\n     结束 */</style><p>好</p>", ".html", False,
         "**跨行**块注释里的字也**不**算 ⇒ 必须绿。⚠️ **这条抓的是「按行剥」那个 bug**："
         "起止符不在同一行时，逐行做的正则会**一行都剥不掉**块注释内部的内容，"
         "而样张的 CSS 注释全是多行的中文散文"),
        (f"/* {BAD} */\n<p>好</p>", ".css", False, "独立 .css 的块注释同理"),
        (f"// {BAD}\n<p>好</p>", ".js", False, "独立 .js 的行注释同理"),
        (f'<a href="#{BAD}">好</a>', ".html", True, "属性值里的字**算** ⇒ 必须红"),
        # ⚠️ 「两种注释标记嵌套」这条夹具钉住"先块注释"这个选择（理由见 `rendered_text` 里的那一大段）。
        #    它是**唯一能分辨两个顺序**的输入：`/*` 在前、`<!--` 嵌在它里面、
        #    而正文夹在两者之间。先块 ⇒ 正文留下（红，正确）；先 HTML ⇒ 正文被吞（绿，错误）。
        (f"<div>/* x <!-- y */ 正文 {BAD} --></div>", ".html", True,
         "**两种注释标记嵌套**时不许吞掉正文 ⇒ 必须红（钉住「先块注释」这个顺序）"),
        # ⚠️ 这条抓的是**少抽**（危险的那一侧）：按裸 `//` 剥会把 URL 之后整行吞掉，
        #    于是 `✗` 从要求集里消失、判据**漏报**。`//` 前面是 `:` ⇒ 不是注释。
        (f'const u = "https://e.example/{BAD}";', ".js", True,
         "JS 里 URL 串后面的字**算** ⇒ 必须红（`//` 前是 `:` 时不是注释）"),
    ]
    failures = []
    with tempfile.TemporaryDirectory() as d:
        for i, (body, ext, want_missing, what) in enumerate(cases):
            path = os.path.join(d, f"case{i}{ext}")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(body)
            got = bool(find_missing([path], covered))
            mark = "✓" if got == want_missing else "✗"
            print(f"  {mark} {what}（期望{'有缺口' if want_missing else '无缺口'}，实际"
                  f"{'有' if got else '无'}）")
            if got != want_missing:
                failures.append(what)
    # 第二组：字体一致那条判据自己的夹具（见 `selftest` 的 docstring）。
    agreement = _font_agreement_fixtures(font_path)
    for ok, what in agreement:
        print(f"  {'✓' if ok else '✗'} {what}")
        if not ok:
            failures.append(what)
    if failures:
        sys.stdout.flush()
        print("", file=sys.stderr)
        for f in failures:
            print(f"自检失败：{f}", file=sys.stderr)
        return 3
    # ⚠️ 夹具个数从两组夹具**数出来**，**不写死**：写死的话，加一个夹具就得记得改这句，
    #    而忘了改的表现只是"打印了个旧数字"——没有任何东西会红（同
    #    `presentation/mod.rs` 里 `RowStateStyle::COUNT` 从链表数出来的那条纪律）。
    print(f"check_design_fonts.sh: 自检通过（{len(cases)} + {len(agreement)} 个夹具全中）。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
