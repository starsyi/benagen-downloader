#!/usr/bin/env bash
#
# windows/scripts/check_presentation_mirrors.sh —— **对位判据**：
# `presentation/` 里那几条"判据在 Rust、行为在前端"的东西，**不许两边各说各话**。
#
#    bash windows/scripts/check_presentation_mirrors.sh
#
# 它由 `windows/scripts/test.sh` 第 0.45 步调用（**按需单跑也可以**）。
#
# ---------------------------------------------------------------------------
# 它为什么存在（R-5：本代最严重那个缺陷的**形状**）
# ---------------------------------------------------------------------------
#   本代最严重那个缺陷（"切一下分区回来，攒的选择就被抹掉"）的形状不是"某个函数写错了"，
#   而是：**判据在 Rust 里（有单测、有文档，文档里逐字写着那个坑的名字），
#   实现被抄进了 JS（`switchcode.js` 的文件头自己写着"本文件抄了 NoteDrafts 一份"、
#   `files.js` 把它当作 `ManifestTracking` 的对位），而两边一分叉——谁都不会红。**
#   ⇒ 判据早就写在那儿了，那个坑还是又长了一遍，最后在真机被用户撞到。
#
#   所以本脚本要做两件事，各治一半：
#     ① **两份真相源**（`resident_notice` 的 48/80）：Rust 交出去的那两个数
#        与 CSS 里那两个令牌**必须一致**，而且那份令牌**必须真的被用上**
#        （否则"Rust 那份是唯一来源"这句话是空头 —— 它根本没到界面上）；
#     ② **对位账**：每一条"有 Rust 判据、零生产调用者、行为由前端承接"的判据，
#        它的**三处坐标**（Rust 判据 / 承接点 / 守着它的判据）都必须还在场 ——
#        并且在账上那一条**仍然零调用者**。
#        ⇒ 账本自己会过期：有人把某一条**接上**了（好事），这一条也会红，
#          逼着账跟着改（"接上了"与"还挂着"是两件事，账上必须说得出是哪一件）。
#
# ⚠️ **效力边界（如实记账，别把这条读成它做不到的事）**：
#    · ①判的是**数值**（两个数一不一样）与"令牌有没有被用"，**不是**"运行时真的按它排版"
#      —— 后者只有真机（`windows/README.md` §1 的第 2 条）能给；
#    · ②判的是**账上那几条**的坐标在场 + 仍然零调用者。**将来新出现的**第三条
#      "零调用者判据"它**看不见**（那需要一个全量扫描，而全量扫描在 `presentation/`
#      这个体量上噪声很大：`pub` 项里有一大批只在测试里被调）。这条残留记在
#      `fixwave-rust-report.md` 与本脚本头部，别当成"全量守卫"。
#
#   ⚠️ 至于那几条判据**行为上**对不对，靠的是另一条路：**同一组输入喂给两边**、
#      结论必须一致。用例表由 Rust 倒出来（`dump_wire_fixtures.rs` 的 `mirrorCases`），
#      前端那一侧在 `frontend-stub/panels-harness.html` 里回放（`check_panels_screen.sh` 跑）。
#      本脚本不重复那件事 —— 它守的是**坐标与数值**那一半。
#
# ---------------------------------------------------------------------------
# 退出码（与 `check_frontend_copy.sh` / `check_wire_fixtures.sh` 同一套）
# ---------------------------------------------------------------------------
#   0 = 判据通过
#   1 = 环境问题（缺 python3，或判据要读的文件不在场 —— W-2：大声说 + 可执行的补救）
#   2 = 用法错误（给了参数）
#   3 = **判据没过**（两个数不一致 / 令牌没人用 / 账上某一条的坐标不见了 / 账过期了）
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "==> $*" >&2; }

# 参数：**一个都不认**（同 `check_wire_fixtures.sh` 的纪律）
while [ "$#" -gt 0 ]; do
  echo "check_presentation_mirrors.sh: 不认识的参数：$1" >&2
  echo "check_presentation_mirrors.sh: 本脚本不接受任何参数（判据与账本都写死在本文件里）。" >&2
  exit 2
done

command -v python3 >/dev/null 2>&1 || die "PATH 里没有 \`python3\` —— 本判据的全部实现是它。
      补救（按你的平台挑一行）：
        macOS: 系统自带 python3
        Debian/Ubuntu: apt-get install -y python3
        Windows: 装 MSYS2（https://www.msys2.org），再 \`pacman -S mingw-w64-x86_64-python\`
      ⚠️ 跳过它等于没有这条判据（W-2 禁的是静默跳过，不是要求工具链最小）。"

note "校验常驻提示行的两个上限（Rust ↔ CSS）与对位账的坐标"
python3 - "$REPO" <<'PY'
import re
import sys
from pathlib import Path

repo = Path(sys.argv[1])

failures = []


def read(rel):
    path = repo / rel
    if not path.is_file():
        failures.append(f"判据要读的文件不在场：{rel}")
        return None
    return path.read_text(encoding="utf-8")


def grab(text, pattern, what, rel):
    """抽第一个捕获组；抽不到 ⇒ 记一条失败（判据无对象，**不是**通过）。"""
    if text is None:
        return None
    hit = re.search(pattern, text)
    if hit is None:
        failures.append(f"{rel}：抽不到「{what}」（模式 {pattern!r} 一处都没命中）")
        return None
    return hit.group(1)


# ===========================================================================
# ① 常驻提示行的两个高度上限：**两份真相源**必须一致
# ===========================================================================
#
# Rust：`windows/shell-core/src/presentation/resident_notice.rs` 的
#       `TEXT_MAX_HEIGHT` / `LIST_MAX_HEIGHT`（模块头逐字写着"常量与判决都由 Rust
#       交出去；JS 只负责按 height 摆位置"）。
# CSS ：`windows/web/css/tokens.css` 的 `--notice-text-max-h` / `--notice-list-max-h`
#       —— **而它今天就是那第二份**（那个模块头说这件事不该发生，而它发生了）。
NOTICE_RS = "windows/shell-core/src/presentation/resident_notice.rs"
TOKENS = "windows/web/css/tokens.css"
INDEX = "windows/web/index.html"

notice_src = read(NOTICE_RS)
tokens_src = read(TOKENS)
index_src = read(INDEX)

rust_text = grab(notice_src, r"TEXT_MAX_HEIGHT:\s*u32\s*=\s*(\d+)", "TEXT_MAX_HEIGHT", NOTICE_RS)
rust_list = grab(notice_src, r"LIST_MAX_HEIGHT:\s*u32\s*=\s*(\d+)", "LIST_MAX_HEIGHT", NOTICE_RS)
css_text = grab(tokens_src, r"--notice-text-max-h:\s*(\d+)px", "--notice-text-max-h", TOKENS)
css_list = grab(tokens_src, r"--notice-list-max-h:\s*(\d+)px", "--notice-list-max-h", TOKENS)

if None not in (rust_text, css_text) and rust_text != css_text:
    failures.append(
        f"散文那一档的上限**两份真相源不一致**：\n"
        f"      Rust  {NOTICE_RS}  TEXT_MAX_HEIGHT = {rust_text}\n"
        f"      CSS   {TOKENS}  --notice-text-max-h: {css_text}px\n"
        f"      后果：改了一边，屏幕上**一点都不会变**，而两边都不会红。"
    )
if None not in (rust_list, css_list) and rust_list != css_list:
    failures.append(
        f"列表那一档的上限**两份真相源不一致**：\n"
        f"      Rust  {NOTICE_RS}  LIST_MAX_HEIGHT = {rust_list}\n"
        f"      CSS   {TOKENS}  --notice-list-max-h: {css_list}px"
    )

# 那份令牌**必须真的被用上**：定义了两个数却没人 `var(...)` 它们的话，
# "Rust 交出去的那一份才是唯一来源"这件事在界面上根本不成立（而两个数照样一致 ⇒ 上面判不出来）。
CSS_DIR = repo / "windows/web/css"
if CSS_DIR.is_dir():
    css_files = sorted(CSS_DIR.rglob("*.css"))
    used = {rel: (repo / rel).read_text(encoding="utf-8") for rel in
            [str(p.relative_to(repo)) for p in css_files]}
    for name in ("--notice-text-max-h", "--notice-list-max-h"):
        sites = [rel for rel, body in used.items() if f"var({name})" in body]
        if not sites:
            failures.append(
                f"{TOKENS} 里定义了 `{name}`，但 `windows/web/css/**` 里**没有一处** "
                f"`var({name})` —— 也就是说这个令牌没到界面上。\n"
                f"      这条判据判的是「Rust 交出去的那两个数就是界面上生效的那两个数」；"
                f"令牌没人用的时候它判不到（两个数照样一致）。\n"
                f"      补救：核对用到它的屏样式（散文那一档：`shell.css` / `panels.css`；"
                f"列表那一档：`shell.css` / `screens/files.css`）是不是被改名/删掉了。"
            )
else:
    failures.append(f"找不到 {CSS_DIR}（前端样式目录不在场）")

# 那份样式表**必须真的被交付页面加载**（没人加载的话，上面那两个数在**交付物里**
# 根本不存在 —— 而它们在源码里是对得上的，所以上一条判据判不出来）。
#
# ⚠️ 链接形状是 `index.html` → `app.css` → `@import "./css/tokens.css"`（无构建步骤，
#    见 `app.css` 头部）。**判的是"这条链通不通"**，不是"哪一行长什么样"：
#    写成"index.html 里必须有 css/tokens.css"会在今天这个已经正确的树上直接假红
#    （实测过，第一版就是这么写的）。
APP_CSS = "windows/web/app.css"
appcss_src = read(APP_CSS)
if index_src is not None and "app.css" not in index_src:
    failures.append(
        f"{INDEX} 里没有加载 `app.css` —— 那两个令牌所在的样式表链在第一跳就断了。"
    )
if appcss_src is not None and "tokens.css" not in appcss_src:
    failures.append(
        f"{APP_CSS} 里没有 `@import` `tokens.css` —— 那两个令牌在交付形态里根本不存在。\n"
        f"      补救：核对那份 `@import` 清单（`tokens.css` 必须**第一个**，见该文件头部的理由）。"
    )

# ===========================================================================
# ② 对位账：每一条的**三处坐标**都必须在场，且账要没过期
# ===========================================================================
#
# 每条账 = （Rust 判据, 承接点, 守着它的判据, 有没有差分对照）。
# `rust_callers` 那一段是**让账自己会过期**：账上写的是"这一条**零生产调用者**、
# 行为由前端承接"——有人把某一条真的**接上**了（好事），这一条就会红，
# 逼着账跟着改（"接上了"与"还挂着"必须说得出是哪一件）。
MIRRORS = [
    {
        "name": "ManifestTracking（同一次加载内不重播 / 换批才复位与重播种）",
        # ⚠️ 这一条**没有**差分对照，理由是**输入字母表对不上**（见 why）——
        #    而它不是"漏了"：它的行为回归网是 `check_files_screen.sh` 的 `keep` 档，
        #    那一档跑的正是真机上撞到的那个动作序列。
        "rust": ("windows/shell-core/src/presentation/browser_row.rs", "pub struct ManifestTracking"),
        "caller_probe": "ManifestTracking",
        "mirrored_at": ("windows/web/js/screens/files.js", "kept.loaded"),
        "guarded_by": ("windows/scripts/check_files_screen.sh", "\"keep\""),
        "differential": False,
        "why": "JS 那一侧**不是抄件，是另一套模型**：它用「这一屏还挂着没有」"
               "（`kept.loaded === undefined`）代替了 Rust 的「代数」（`seeded_generation`），"
               "两边的输入字母表都不一样 ⇒「同一组输入」这句话在那里没有定义，"
               "硬做出来的差分只会钉住一种**编码选择**，不是行为。"
               "所以这一格的回归网在**行为**那一侧：`check_files_screen.sh` 的 `keep` 档"
               "（选一个文件 → 去传输列表看一眼 → 切回来 → 点下载 ⇒ 下的必须是那一个）。",
    },
    {
        "name": "NoteDrafts（备注草稿的五条判决）",
        "rust": ("windows/shell-core/src/presentation/batch_history.rs", "pub struct NoteDrafts"),
        "caller_probe": "NoteDrafts",
        "mirrored_at": ("windows/web/js/switchcode.js", "function commitNote"),
        "guarded_by": ("windows/scripts/frontend-stub/panels-harness.html", "noteDraftsDifferential"),
        "differential": True,
        "why": "差分对照：用例表由 Rust 倒出来（`dump_wire_fixtures.rs` 的 `mirrorCases.noteDrafts`），"
               "前端在无头浏览器里回放同一组操作、把观察到的 `history_put` 与 Rust 的判决逐条比。",
    },
    {
        "name": "SettingsForm::matches（此刻算不算有未保存的改动）",
        "rust": ("windows/shell-core/src/presentation/settings_form.rs", "pub fn matches"),
        "caller_probe": "::matches(",
        "mirrored_at": ("windows/web/js/settings.js", "function hasUnsavedChanges"),
        "guarded_by": ("windows/scripts/frontend-stub/panels-harness.html", "settingsMatchesDifferential"),
        "differential": True,
        "why": "差分对照：用例表由 Rust 倒出来（`mirrorCases.settingsMatches`），"
               "前端把七格填成表里那一份、再读横幅是哪一句。",
    },
    {
        "name": "SettingsForm::dirty_banner（保存条那两句横幅）",
        "rust": ("windows/shell-core/src/presentation/settings_form.rs", "pub const UNSAVED_BANNER"),
        "caller_probe": "dirty_banner",
        # 两句**文案**确实发出去了（`api/settings.rs` 的 `banner.{unsaved,clean}`），
        # 前端只负责挑一句。所以这一格的"承接点"是**载荷**，不是某一行 JS。
        "mirrored_at": ("windows/shell-core/src/api/settings.rs", "banner"),
        "guarded_by": ("windows/scripts/frontend-stub/panels-harness.html", "③m"),
        "differential": False,
        "why": "这一格与上面两条不同：**两句文案是发出去了的**（`api::settings::payload` 的 "
               "`banner` 那一格），前端只是按判决挑一句。真正零调用者的是"
               "`dirty_banner(has_unsaved_changes)` 这个**挑选函数**（挑选现在活在前端）——"
               "它的落点由上一格那条差分覆盖：判决一致，挑出来的句子自然一致。",
    },
]

# 生产路径 = 命令层与壳的命令实现（`presentation` 自己的定义与用例不算）。
PRODUCTION_DIRS = ["windows/shell-core/src/api", "windows/shell-win/src"]


def production_caller(probe):
    """这一条**今天还有没有生产调用者**（有 ⇒ 账过期了，见下面的处置）。"""
    for rel_dir in PRODUCTION_DIRS:
        base = repo / rel_dir
        if not base.is_dir():
            continue
        for path in base.rglob("*.rs"):
            if probe in path.read_text(encoding="utf-8"):
                return str(path.relative_to(repo))
    return None


for entry in MIRRORS:
    name = entry["name"]
    for label, (rel, marker) in (
        ("Rust 判据", entry["rust"]),
        ("承接点", entry["mirrored_at"]),
        ("守着它的判据", entry["guarded_by"]),
    ):
        body = read(rel)
        if body is not None and marker not in body:
            failures.append(
                f"对位账里「{name}」的**{label}**不在它写着的地方了：\n"
                f"      {rel} 里找不到 `{marker}`\n"
                f"      这条账记的是\"这一条判据的行为由前端承接，而承接点/守卫在那里\"——\n"
                f"      坐标过期之后，那句话就成了一句**没人能核**的断言（R-40 那条教训）。\n"
                f"      补救：核对它被搬去了哪里，把账改对；若它被删了，把这一条从账上删掉。"
            )
    # 账要没过期：这一条**今天仍然是零生产调用者**。
    caller = production_caller(entry["caller_probe"])
    if caller is not None:
        failures.append(
            f"对位账过期了：「{name}」**已经有生产调用者**了（{caller}）。\n"
            f"      账上写的仍然是「零生产调用者、行为由前端承接」——那句**已经不成立**。\n"
            f"      补救（二选一）：把这一条从账上删掉（它已经被接上了，这是好事），\n"
            f"      或把账改成「已接上，承接点是 X」并说明前端那一份还算不算数。"
        )

if failures:
    print("", file=sys.stderr)
    print("对位判据没过：", file=sys.stderr)
    print("", file=sys.stderr)
    for item in failures:
        print(f"  ✗ {item}", file=sys.stderr)
        print("", file=sys.stderr)
    sys.exit(3)

# ---- 通过：把**算出来**的数打出来（不写死 —— 写死了就在判据改了之后不会露馅）----
print(f"常驻提示行的两个上限：Rust({rust_text}/{rust_list}) == CSS({css_text}px/{css_list}px)，"
      f"两份令牌都被 `windows/web/css/**` 用上，且样式表链 `{INDEX}` → `{APP_CSS}` → `tokens.css` 通")
print(f"对位账：{len(MIRRORS)} 条，三处坐标全部在场、且**都仍然是零生产调用者**"
      f"（其中差分对照 {sum(1 for m in MIRRORS if m['differential'])} 条，"
      f"登记为\"承接点不是抄件\"的 {sum(1 for m in MIRRORS if not m['differential'])} 条）")
print("对位判据通过：两份真相源一致；每条对位账的坐标在场、账没过期。")
sys.exit(0)
PY
