"""生成 SK-vitals 的 exe 图标（assets/sk-vitals.ico）。

风格与托盘图标 / 悬浮窗保持一致：深色圆角卡片 + 三根递增的绿色柱子。
每个尺寸都单独按矢量比例重绘再降采样，避免从大图缩到 16px 时发糊。

用法：
    python tools/make_icon.py
"""
import os
import struct
import io

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "assets", "sk-vitals.ico")

CARD_TOP = (32, 39, 53)
CARD_BOTTOM = (20, 25, 34)
BAR = (61, 214, 140)
BAR_TOP = (126, 236, 178)

SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 8  # 超采样倍数


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def rounded_mask(size, radius):
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return mask


def render(size):
    """按目标尺寸绘制一张 RGBA 图标（内部超采样后缩小）。"""
    S = size * SS
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # 卡片背景：竖向渐变
    grad = Image.new("RGB", (1, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        gd.point((0, y), fill=lerp(CARD_TOP, CARD_BOTTOM, y / max(1, S - 1)))
    grad = grad.resize((S, S))
    card = grad.convert("RGBA")
    card.putalpha(rounded_mask(S, int(S * 0.2)))
    img = Image.alpha_composite(img, card)

    # 顶部高光描边
    edge = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ed = ImageDraw.Draw(edge)
    r = int(S * 0.2)
    ed.rounded_rectangle(
        [1, 1, S - 2, S - 2], radius=r, outline=(255, 255, 255, 26), width=max(1, int(S * 0.012))
    )
    img = Image.alpha_composite(img, edge)

    # 三根递增的柱子
    bars = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    bd = ImageDraw.Draw(bars)
    w = S * 0.155
    gap = S * 0.075
    total = 3 * w + 2 * gap
    left = (S - total) / 2
    bottom = S * 0.735
    heights = [S * 0.28, S * 0.44, S * 0.60]
    radius = w / 2
    for i, h in enumerate(heights):
        x0 = left + i * (w + gap)
        bd.rounded_rectangle(
            [x0, bottom - h, x0 + w, bottom], radius=radius, fill=BAR + (255,)
        )
    img = Image.alpha_composite(img, bars)

    # 最高的那根再亮一点，做出层次
    top_bar = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    td = ImageDraw.Draw(top_bar)
    x0 = left + 2 * (w + gap)
    td.rounded_rectangle(
        [x0, bottom - heights[2], x0 + w, bottom], radius=radius, fill=BAR_TOP + (255,)
    )
    img = Image.alpha_composite(img, top_bar)

    return img.resize((size, size), Image.LANCZOS)


def bmp_entry(img):
    """32bpp BGRA 的 ICO 分量（含 AND 掩码）。"""
    w, h = img.size
    px = img.load()
    data = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            data += bytes((b, g, r, a))
    row = ((w + 31) // 32) * 4
    mask = bytearray()
    for y in range(h - 1, -1, -1):
        m = bytearray(row)
        for x in range(w):
            if px[x, y][3] < 128:
                m[x // 8] |= 0x80 >> (x % 8)
        mask += m
    header = struct.pack(
        "<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(data) + len(mask), 0, 0, 0, 0
    )
    return bytes(header + data + mask)


def png_entry(img):
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def main():
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    images = {s: render(s) for s in SIZES}

    entries = []
    for s in SIZES:
        if s >= 256:
            entries.append((s, png_entry(images[s])))
        else:
            entries.append((s, bmp_entry(images[s])))

    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_bytes = bytearray()
    for s, data in entries:
        dim = 0 if s >= 256 else s
        dir_bytes += struct.pack(
            "<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset
        )
        offset += len(data)

    with open(OUT, "wb") as f:
        f.write(header)
        f.write(dir_bytes)
        for _, data in entries:
            f.write(data)

    # 顺手留一张 PNG 预览，方便 README / 提交记录里看
    images[256].save(os.path.join(ROOT, "docs", "icon.png"))
    print(f"SAVED {OUT} ({os.path.getsize(OUT)} bytes), sizes={SIZES}")


if __name__ == "__main__":
    main()
