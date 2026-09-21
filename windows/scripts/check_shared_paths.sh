#!/usr/bin/env bash
#
# windows/scripts/check_shared_paths.sh —— **壳与内核必须落在同一个目录、读同一个环境变量**。
#
#    bash windows/scripts/check_shared_paths.sh
#
# 它由 `windows/scripts/test.sh` 调用（**按需单跑也可以**）。
#
# ---------------------------------------------------------------------------
# 它为什么存在（R-4：**"某两处必须一致"这种话，要有东西守着**）
# ---------------------------------------------------------------------------
#   同一套"目录名 / 环境变量名"的知识**散在四处**，而它们各自都是常量、各自都没错：
#
#     ① `core/src/paths.rs`                 —— 内核读 `%APPDATA%` / `%LOCALAPPDATA%`，
#                                              目录名 `BenagenDownloader`
#     ② `windows/shell-core/src/storage/mod.rs`  —— 壳读 `%APPDATA%`，
#                                              目录名 `APP_DIR_NAME`
#     ③ `windows/shell-core/src/embedded_core.rs` —— 释放缓存到
#                                              `<…>\BenagenDownloader\cache`
#     ④ `windows/shell-win/src/{embed,wv2}.rs`  —— 取 `%LOCALAPPDATA%` 的字面量
#
#   四处**今天都一致**，所以今天没有可观察的缺陷。但**漂移时不会有任何东西变红**，
#   而后果是具体的：壳的 `preferences.json` / `history.json`（②）与内核的
#   `settings.json` / aria2c 缓存（①）会落到**两个不同的目录** ——
#   用户的下载目录设置与批次历史"凭空消失"（换了目录之后读不到旧文件），
#   而壳与内核**各自都自洽、各自的用例全绿**。
#
#   ⚠️ 这条被记了很久，但记错了形状：账本里它写作"某条用例判别力弱
#      （常量 == 字面量，没有任何机制把它与 `core/src/paths.rs` 绑住）"——
#      而它实际是"**没有任何东西**保证壳与内核同源"。用例判别力弱是**症状**，
#      缺一条仓库级判据才是**病因**（R-40 那条教训：一句留着不纠正的自诊断，
#      会让下一个人去"修"一条本来就好的判据）。
#
#   ⚠️ 为什么不能写成一条 Rust 用例：`shell-core` **不许依赖 `core`**（那是计划强制的
#      形态，也是"壳能在 macOS 上 cargo test"这条地基）。所以两边**在编译期互相看不见**
#      —— 唯一能看全两棵树的地方，就是本脚本这种**仓库级**的检查。
#
# ---------------------------------------------------------------------------
# 判据（fail-closed）
# ---------------------------------------------------------------------------
#   把上面四处里的**字面量**抽出来，按"同一件事"分组比对，**任何一组出现第二种值 ⇒ 红**。
#   抽取失败（正则没命中 / 文件不在）**也红**：一个"因为没抽到所以通过"的判据，
#   与没有判据无法区分。
#
#   ⚠️ **效力边界（如实记账）：它判的是这几处的字面量，不是运行期行为。**
#      "壳与内核在真机上真的落到同一个目录"仍然只有第 4 条真机验收能证
#      （`windows/README.md` §1）。这条判据守的是**源码里那几句话不许分叉**。
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_frontend_copy.sh` / `check_wire_fixtures.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 判据通过（每一组都只有一种值）
#   1 = 环境问题（缺 python3，或四个来源文件不在场 —— W-2：大声说 + 可执行的补救）
#   2 = 用法错误（给了参数）
#   3 = **判据没过**（某一组出现了第二种值）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# 参数：**一个都不认**（同 `check_wire_fixtures.sh` 的纪律：透传一个打错的 flag 会让
# 脚本以一个"看起来像失败"的退出码收场，而根因是打错了字）
while [ "$#" -gt 0 ]; do
  echo "check_shared_paths.sh: 不认识的参数：$1" >&2
  echo "check_shared_paths.sh: 本脚本不接受任何参数（四个来源与四组判据都写死在本文件里）。" >&2
  exit 2
done

command -v python3 >/dev/null 2>&1 || die "PATH 里没有 \`python3\` —— 本判据的全部实现是它。
      补救（按你的平台挑一行）：
        macOS: 系统自带 python3
        Debian/Ubuntu: apt-get install -y python3
        Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`
      ⚠️ 跳过它等于没有这条判据（W-2 禁的是静默跳过，不是要求工具链最小）。"

note "比对壳与内核的目录名 / 环境变量名（四处）"
# ⚠️ python 的退出码**原样传出去**（0/3 是它给的，别在这里压成 1）：
#    "产品判据没过"与"环境不对"是两件事，压平之后读完数的人分不出来。
python3 - "$REPO" <<'PY'
import re
import sys
from pathlib import Path

repo = Path(sys.argv[1])

# ---- 四个来源（缺一个就红：那不是"跳过"，是判据无对象可判） -------------------
SOURCES = {
    "核心：内核的目录与变量名": "core/src/paths.rs",
    "壳：本地存储的目录名与环境变量名": "windows/shell-core/src/storage/mod.rs",
    "壳：内嵌件的缓存目录": "windows/shell-core/src/embedded_core.rs",
    "壳-win：把 LOCALAPPDATA 交出去的两处": "windows/shell-win/src/embed.rs",
    "壳-win：WebView2Loader 那一处": "windows/shell-win/src/wv2.rs",
}

missing = [rel for rel in SOURCES.values() if not (repo / rel).is_file()]
if missing:
    print("判据判不了：下面这些来源文件不在场 ——", file=sys.stderr)
    for rel in missing:
        print(f"    ✗ {rel}", file=sys.stderr)
    print("  它们就是本判据的两侧（内核 / 壳）。少一侧，\"两边一致\"这句话无从判起。", file=sys.stderr)
    print("  补救：核对仓库布局——本脚本期望自己在 <repo>/windows/scripts/check_shared_paths.sh。", file=sys.stderr)
    sys.exit(1)

text = {rel: (repo / rel).read_text(encoding="utf-8") for rel in SOURCES.values()}


def find_all(rel, pattern, what, *, scope=None):
    """在某个来源（或它的某个函数体内）抽出**全部**命中；一个都没有 ⇒ 抛（判据无对象）。

    ⚠️ **为什么要按函数体圈定**：`paths.rs` 里"两个 `.join` 叠在一起"的形状在好几个
       地方都有（`settings.json` 那一串、macOS 那串 `Library/Application Support/…`、
       `cache` 那一串）。不圈定的话，"缓存那一级叫什么"会把 `settings.json` 也抓进来，
       判据就会**假红** —— 而假红会让人把守卫整个关掉（代价比留一条窄缝更大，
       同 `test.sh` 第 3 步那段注释）。圈定用的是"函数签名 → 第一个顶格 `}`"，
       对这几个文件的形状成立（嵌套块的 `}` 都有缩进）。
    """
    body = text[rel]
    if scope is not None:
        start = body.find(scope)
        if start < 0:
            raise LookupError(
                f"{rel}：找不到函数 `{scope}` —— 判据依赖它还在不在那里。\n"
                f"    这不是\"通过\"：函数被改名/搬走了，本判据两侧的对照就少了一半。"
            )
        end = body.find("\n}\n", start)
        if end < 0:
            raise LookupError(f"{rel}：`{scope}` 之后找不到顶格的 `}}`（函数体切不出来）。")
        body = body[start:end]
    hits = re.findall(pattern, body)
    if not hits:
        raise LookupError(
            f"{rel}：抽不到「{what}」（模式 {pattern!r} 一处都没命中）。\n"
            f"    这不是\"通过\"——要么这句话被改写成了判据认不出的形状，\n"
            f"    要么它被删了。两种都要人来判：对着本脚本头部那张表改判据或改代码。"
        )
    return hits


PATHS = "core/src/paths.rs"
STORAGE = "windows/shell-core/src/storage/mod.rs"
EMBEDDED = "windows/shell-core/src/embedded_core.rs"
EMBED = "windows/shell-win/src/embed.rs"
WV2 = "windows/shell-win/src/wv2.rs"

# ---- 四组判据：每一组 = 一件"必须两边同名"的事 ---------------------------------
#
# 每组是一串 `(出处, 值)`。**同一个值不许出现第二种写法**。
groups = []


def group(name, why, pairs):
    groups.append({"name": name, "why": why, "pairs": pairs})


def sites(label, rel, pattern, what, *, scope=None):
    """`find_all` 的带出处版本：重名的一处也要能分辨（同一句写出两遍是常态）。

    ⚠️ **出处要能落到"是哪一处"**：`settings_file_for` 里 Windows 与 macOS 两支各写
       了一遍目录名（它们**本来就该**一致，所以两次命中是判据想要的东西）——
       但红的时候两行同名的话，读者分不出"两处都写了"还是"脚本打重了"。
    """
    hits = find_all(rel, pattern, what, scope=scope)
    if len(hits) == 1:
        return [(label, hits[0])]
    return [(f"{label}（第 {i} 处）", value) for i, value in enumerate(hits, start=1)]


try:
    # ① 应用目录名：内核两处（settings.json 的父目录、aria2c 缓存的父目录 ——
    #    各自还有 Windows / macOS 两支）与壳两处（`APP_DIR_NAME`、释放缓存时拼的那一段）
    #    必须是同一个名字。
    group(
        "应用目录名（`BenagenDownloader`）",
        "壳的 preferences.json / history.json 与内核的 settings.json / aria2c 缓存要落在同一个目录；"
        "分叉的后果是用户的设置与历史\"凭空消失\"",
        sites("core/src/paths.rs 的 settings_file_for（settings.json 的父目录）", PATHS,
              r'\.join\("([^"]+)"\)\s*\.join\("settings\.json"\)',
              "settings.json 的父目录", scope="fn settings_file_for(")
        + sites("core/src/paths.rs 的 cache_dir_for（cache 的父目录）", PATHS,
                r'\.join\("([^"]+)"\)\s*\.join\("cache"\)',
                "cache 的父目录", scope="fn cache_dir_for(")
        + sites("shell-core/src/storage/mod.rs 的 APP_DIR_NAME", STORAGE,
                r'const\s+APP_DIR_NAME\s*:\s*&str\s*=\s*"([^"]+)"', "APP_DIR_NAME")
        + sites("shell-core/src/embedded_core.rs 的 cache_dir_from（释放缓存时拼的目录）", EMBEDDED,
                r'\.join\("([^"]+)"\)\s*\.join\("cache"\)',
                "释放缓存时拼的目录", scope="pub fn cache_dir_from("),
    )

    # ② `settings.json` 读哪个环境变量：内核 `env_var_name` 的 Windows 那支
    #    与壳 `env_var_name` 的 Windows 那支。
    group(
        "设置文件所在的标准目录（环境变量名）",
        "两边读的不是同一个变量 ⇒ 壳改的设置在另一份文件里，而界面看起来一切正常",
        sites("core/src/paths.rs 的 Purpose::Settings", PATHS,
              r'Purpose::Settings\s*=>\s*"([A-Z_]+)"', "Purpose::Settings 的环境变量名")
        + sites("shell-core/src/storage/mod.rs 的 Platform::Windows", STORAGE,
                r'Platform::Windows\s*=>\s*"([A-Z_]+)"', "Platform::Windows 的环境变量名"),
    )

    # ③ 缓存目录读哪个环境变量：内核 `Purpose::Cache` 与壳-win 那两处**字面量**。
    group(
        "缓存目录所在的标准目录（环境变量名）",
        "壳把内嵌件释放到 A、内核去 B 找 aria2c ⇒ 首次运行就会自己重下一份，"
        "而两边的日志各说各话",
        sites("core/src/paths.rs 的 Purpose::Cache", PATHS,
              r'Purpose::Cache\s*=>\s*"([A-Z_]+)"', "Purpose::Cache 的环境变量名")
        + sites("shell-win/src/embed.rs", EMBED, r'var_os\("([A-Z_]+)"\)', "var_os 的环境变量名")
        + sites("shell-win/src/wv2.rs", WV2, r'var_os\("([A-Z_]+)"\)', "var_os 的环境变量名"),
    )

    # ④ 缓存目录那一级的名字（`cache`）：内核与壳的释放路径末尾那一节。
    group(
        "缓存目录那一级的名字（`cache`）",
        "壳释放到 `…\\BenagenDownloader\\cache`、内核去别处找 ⇒ 每次启动都白释放一遍",
        sites("core/src/paths.rs 的 cache_dir_for", PATHS,
              r'\.join\("[^"]+"\)\s*\.join\("([^"]+)"\)',
              "缓存那一级", scope="fn cache_dir_for(")
        + sites("shell-core/src/embedded_core.rs 的 cache_dir_from", EMBEDDED,
                r'\.join\("[^"]+"\)\s*\.join\("([^"]+)"\)',
                "缓存那一级", scope="pub fn cache_dir_from("),
    )
except LookupError as error:
    print("判据判不了：抽字面量这一步失败了。", file=sys.stderr)
    print(f"    {error}", file=sys.stderr)
    sys.exit(1)

# ---- 比对 ---------------------------------------------------------------------
failures = []
for entry in groups:
    values = {}
    for where, value in entry["pairs"]:
        values.setdefault(value, []).append(where)
    if len(values) > 1:
        failures.append((entry, values))

if failures:
    print("", file=sys.stderr)
    print("壳与内核的目录/变量名判据没过：下面这几件事**出现了第二种值**。", file=sys.stderr)
    print("", file=sys.stderr)
    for entry, values in failures:
        print(f"  ✗ {entry['name']}", file=sys.stderr)
        for value, wheres in values.items():
            print(f"      「{value}」", file=sys.stderr)
            for where in wheres:
                print(f"          ← {where}", file=sys.stderr)
        print(f"      为什么它必须一致：{entry['why']}", file=sys.stderr)
        print("", file=sys.stderr)
    print("  补救：挑一个值，把上面列出的**每一处**都改成它。", file=sys.stderr)
    print("  ⚠️ 别只改一处就重跑：这条判据判的就是\"改了一半\"——两边各自自洽、", file=sys.stderr)
    print("     各自的用例全绿，而用户的设置与历史落在两个目录里。", file=sys.stderr)
    print("  ⚠️ 也别把本判据放宽成\"两处都在场\"：那正是它要挡的形状。", file=sys.stderr)
    sys.exit(3)

# ---- 通过：把每一组**现算**的值打出来（不写死 —— 写死了就在判据改了之后不会露馅）--
for entry in groups:
    values = sorted({value for _where, value in entry["pairs"]})
    sites = len(entry["pairs"])
    print(f"{entry['name']}：{sites} 处一致 → {'、'.join(values)}")
print("路径同源判据通过：壳与内核读同一个环境变量、落在同一个目录（四组，逐处比对）。")
sys.exit(0)
PY
