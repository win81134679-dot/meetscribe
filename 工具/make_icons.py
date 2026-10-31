"""語音轉錄的圖示：電腦與手機同一份幾何（2026-10-31 使用者：「為什麼電腦程式沒有圖示 幫我重新設計圖示電腦與手機要一致」）。

圖案是「聲音變成文字」：左邊四條長短不一的聲波、右邊三行逐字稿，白色疊在主色 #7F77DD（theme.rs 的 accent）上。
座標用 Android 自適應圖示的 108 單位畫布；手機桌面看得到的是中間 72 單位（18～90），電腦版的圓角方塊就取同一塊，兩邊比例一樣。

  <python> 工具/make_icons.py          重產下面全部（要 Pillow）
產出：packaging/icon.ico（16～256，exe／安裝程式／捷徑）、packaging/icon-256.png（視窗左上角）、packaging/icon-preview.png（看圖用）、
android/app/src/main/res/drawable/ic_launcher_foreground.xml（啟動圖示，背景色是 values/colors.xml 的 accent）、ic_notification.xml（通知列）。
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ACCENT = (0x7F, 0x77, 0xDD)
WHITE = (255, 255, 255)

# (x, y, 寬, 高)；圓角＝短邊的一半（膠囊形）。全部落在安全區 21～87 內
BARS = [(27, 47, 6, 14), (36, 37, 6, 34), (45, 42, 6, 24), (54, 49, 6, 10)]
LINES = [(65, 40, 18, 6), (65, 51, 18, 6), (65, 62, 11, 6)]
SHAPES = BARS + LINES
# 24 像素以下 7 個形狀擠成一團：三條聲波＋兩行字、筆畫粗一點（同樣的構圖）
SMALL = [(27, 44, 8, 20), (39, 36, 8, 36), (51, 46, 8, 16), (64, 43, 20, 8), (64, 57, 13, 8)]
VISIBLE = (18, 90)  # 自適應圖示被遮罩後看得到的範圍
TILE_RADIUS = 0.225  # 電腦版圓角方塊的圓角（相對邊長）


def pill_path(x, y, w, h, scale=1.0, dx=0.0, dy=0.0):
    """膠囊形的 Android pathData（直的或橫的都可）。"""
    x, y, w, h = x * scale + dx, y * scale + dy, w * scale, h * scale
    r = min(w, h) / 2
    f = lambda v: f"{v:.3f}".rstrip("0").rstrip(".")  # noqa: E731
    if h >= w:  # 直的：上下半圓
        return (f"M{f(x)},{f(y + r)}a{f(r)},{f(r)} 0,0 1,{f(w)},0v{f(h - 2 * r)}"
                f"a{f(r)},{f(r)} 0,0 1,{f(-w)},0z")
    return (f"M{f(x + r)},{f(y)}h{f(w - 2 * r)}a{f(r)},{f(r)} 0,0 1,0,{f(h)}"
            f"h{f(-(w - 2 * r))}a{f(r)},{f(r)} 0,0 1,0,{f(-h)}z")


def vector(size_dp, viewport, paths, comment):
    body = "\n".join(f'    <path\n        android:fillColor="#FFFFFF"\n        android:pathData="{p}" />' for p in paths)
    return (f'<?xml version="1.0" encoding="utf-8"?>\n<!-- {comment}；由 工具/make_icons.py 產生，改圖示改那支 -->\n'
            f'<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
            f'    android:width="{size_dp}dp"\n    android:height="{size_dp}dp"\n'
            f'    android:viewportWidth="{viewport}"\n    android:viewportHeight="{viewport}">\n{body}\n</vector>\n')


def tile(px: int) -> Image.Image:
    """電腦版：圓角方塊＋圖案，先畫 8 倍再縮（邊緣平滑）。小尺寸留 1 像素邊，跟 Windows 圖示一樣不頂到邊。"""
    ss = 8
    big = px * ss
    pad = (1 if px <= 32 else round(px * 0.04)) * ss
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    side = big - 2 * pad
    d.rounded_rectangle((pad, pad, pad + side - 1, pad + side - 1), radius=round(side * TILE_RADIUS), fill=ACCENT)
    v0, v1 = VISIBLE
    k = side / (v1 - v0)
    for x, y, w, h in (SMALL if px <= 24 else SHAPES):
        grow = 0.3 if 24 < px <= 48 else 0.0
        x0, y0 = pad + (x - v0 - grow / 2) * k, pad + (y - v0 - grow / 2) * k
        x1, y1 = x0 + (w + grow) * k, y0 + (h + grow) * k
        d.rounded_rectangle((x0, y0, x1, y1), radius=min(x1 - x0, y1 - y0) / 2, fill=WHITE)
    return img.resize((px, px), Image.LANCZOS)


def main():
    pk = ROOT / "packaging"
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    tiles = {s: tile(s) for s in sizes}
    tiles[256].save(pk / "icon.ico", sizes=[(s, s) for s in sizes], append_images=[tiles[s] for s in sizes[:-1]])
    tiles[256].save(pk / "icon-256.png")

    res = ROOT / "android" / "app" / "src" / "main" / "res" / "drawable"
    (res / "ic_launcher_foreground.xml").write_bytes(
        vector(108, 108, [pill_path(*s) for s in SHAPES], "聲波＋三行逐字稿（電腦版 icon.ico 同一份幾何）").encode("utf-8"))
    # 通知列 24dp：安全區 21～87 對到 1～23
    k = 22 / 66
    (res / "ic_notification.xml").write_bytes(
        vector(24, 24, [pill_path(*s, scale=k, dx=1 - 21 * k, dy=1 - 21 * k) for s in SHAPES], "通知列：同一個圖案、單色").encode("utf-8"))

    # 看圖：電腦各尺寸＋手機圓形／方形遮罩
    sheet = Image.new("RGBA", (1000, 540), (246, 245, 241, 255))
    x = 20
    for s in [256, 128, 64, 48, 32, 24, 16]:
        sheet.alpha_composite(tiles[s], (x, 20 + (256 - s) // 2))
        x += s + 24
    # 下排：小尺寸放大 6 倍（看像素）、手機圓形與圓角方形遮罩
    x = 20
    for s in [16, 24, 32]:
        sheet.alpha_composite(tiles[s].resize((s * 6, s * 6), Image.NEAREST), (x, 300))
        x += s * 6 + 20
    sheet.alpha_composite(tile_masked(200, circle=True), (x, 300))
    sheet.alpha_composite(tile_masked(200, circle=False), (x + 220, 300))
    sheet.save(pk / "icon-preview.png")
    print("ok")


def tile_masked(px, circle):
    """手機桌面的樣子：108 畫布的前景＋accent 背景，用圓形或圓角方形遮罩剪出中間 72。"""
    ss = 4
    big = px * ss
    k = big / (VISIBLE[1] - VISIBLE[0])
    img = Image.new("RGBA", (big, big), ACCENT + (255,))
    d = ImageDraw.Draw(img)
    for x, y, w, h in SHAPES:
        x0, y0 = (x - VISIBLE[0]) * k, (y - VISIBLE[0]) * k
        d.rounded_rectangle((x0, y0, x0 + w * k, y0 + h * k), radius=min(w, h) * k / 2, fill=WHITE)
    mask = Image.new("L", (big, big), 0)
    md = ImageDraw.Draw(mask)
    if circle:
        md.ellipse((0, 0, big - 1, big - 1), fill=255)
    else:
        md.rounded_rectangle((0, 0, big - 1, big - 1), radius=big * 0.22, fill=255)
    img.putalpha(mask)
    return img.resize((px, px), Image.LANCZOS)


if __name__ == "__main__":
    main()
