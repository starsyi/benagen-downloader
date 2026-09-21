#!/usr/bin/env python3
"""从源图生成品牌资产：AppIcon.icns + 两张透明底 PNG。

⚠️ **这个脚本不是构建的一部分**：`build_app_macos.sh` 不调用它。
   它产出的是**入库的静态产物**，理由见下。

为什么不做成构建的一步：它要 **Pillow**，而构建机不该依赖 Python 图像库
（本项目至今只用系统框架与 cargo/swift 两条链）。要换品牌资产时才手动跑一次，
把产物连同源图一起提交。

⚠️ 为什么**不裁边**：图形本就**紧贴画布边**，裁了等于没裁。实测两路判据
（按 alpha 找 bbox、合成到白底后取非白像素）**都等于整张画布**，原因是那些边缘像素
是**真图形的抗锯齿边缘**，不是瑕疵：第 0 行有 20 个 `(16,161,215, α≤128)` 的**蓝色**像素
（最上方那颗蓝圆的顶点），第 0 列有 `(188,201,58, α≤192)` 的**绿色**像素（绿块左侧）。
对最上方那个圆做最小二乘拟合得圆心 y≈90.5、半径 r≈90.7 ⇒ 顶点在 y≈-0.2，
即**恰好相切**；连通域量出的各图形 bbox 也都等于各自的对称直径（触顶/触右/触左/触底），
**没有一个是"被切平"的**。所以原样使用。

用法：python3 macos/scripts/make_brand_assets.py
"""
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]          # …/macos
SRC = ROOT / "Resources" / "source"
OUT = ROOT / "Resources"

# macOS Big Sur 起的自定义图标惯例：1024 画布 + 824 白底圆角矩形，圆角半径 185。
# 系统**不会**帮你套这个壳（只有 App Store 的图标才自动切），必须自己烘进去。
CANVAS, PLATE, RADIUS = 1024, 824, 185
MARK_FRACTION = 0.60        # 图形标占白底的边长比，留出呼吸空间


def main() -> int:
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        print("需要 Pillow：pip install Pillow（本脚本只在改品牌资产时手动跑）", file=sys.stderr)
        return 2

    mark_src = SRC / "benagen.logo.png"
    full_src = SRC / "benagen_full-logo.png"
    for p in (mark_src, full_src):
        if not p.is_file():
            print(f"错误：找不到源图 {p}", file=sys.stderr)
            return 2

    OUT.mkdir(parents=True, exist_ok=True)

    # 透明底原件原样入库：界面上的白底由 SwiftUI 画（见 BrandAssets.swift 的说明），
    # 不烘进图里 —— 烘进去的话深色模式下就是一块脏白方块。
    shutil.copyfile(mark_src, OUT / "benagen-mark.png")
    shutil.copyfile(full_src, OUT / "benagen-full-logo.png")

    icon = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    d = ImageDraw.Draw(icon)
    o = (CANVAS - PLATE) // 2
    d.rounded_rectangle([o, o, o + PLATE - 1, o + PLATE - 1], RADIUS, fill=(255, 255, 255, 255))

    mark = Image.open(mark_src).convert("RGBA")
    box = int(PLATE * MARK_FRACTION)
    scale = min(box / mark.width, box / mark.height)
    sized = mark.resize((max(1, round(mark.width * scale)),
                         max(1, round(mark.height * scale))), Image.LANCZOS)
    icon.alpha_composite(sized, ((CANVAS - sized.width) // 2, (CANVAS - sized.height) // 2))

    iconset = OUT / "AppIcon.iconset"
    iconset.mkdir(exist_ok=True)
    for px in (16, 32, 64, 128, 256, 512):
        icon.resize((px, px), Image.LANCZOS).save(iconset / f"icon_{px}x{px}.png")
        icon.resize((px * 2, px * 2), Image.LANCZOS).save(iconset / f"icon_{px}x{px}@2x.png")
    icon.resize((1024, 1024), Image.LANCZOS).save(iconset / "icon_512x512@2x.png")

    # iconutil 是 macOS 自带的，不需要额外依赖。
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(OUT / "AppIcon.icns")],
                   check=True)
    shutil.rmtree(iconset)

    for name in ("AppIcon.icns", "benagen-mark.png", "benagen-full-logo.png"):
        f = OUT / name
        print(f"  {name:24s} {f.stat().st_size:>9,d} 字节")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
