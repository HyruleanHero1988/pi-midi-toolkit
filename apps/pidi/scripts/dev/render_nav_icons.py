#!/usr/bin/env python3
"""Re-render Font Awesome Free solid icons used by PiDI global nav.

Requires: pip install pillow
Downloads fa-solid-900.ttf into apps/pidi/_icon_build/ if missing.
"""
from __future__ import annotations

import pathlib
import urllib.request

from PIL import Image, ImageDraw, ImageFont

HERE = pathlib.Path(__file__).resolve().parents[2]  # apps/pidi
OUT = HERE / "pidi" / "ui" / "icons"
CACHE = HERE / "_icon_build"
TTF_URL = (
    "https://github.com/FortAwesome/Font-Awesome/raw/6.7.2/webfonts/fa-solid-900.ttf"
)
OUT_SIZE = 64
SCALE = 4
FONT_SIZE = 160
# (glyph, optical_x_nudge, optical_y_nudge) in final pixels
GLYPHS = {
    "arrow-left": ("\uf060", 0, 0),
    "house": ("\uf015", 0, 0),
    "power-off": ("\uf011", 0, 2),
}


def _render(
    char: str,
    fg: tuple[int, int, int],
    bg: tuple[int, int, int],
    path: pathlib.Path,
    font: ImageFont.FreeTypeFont,
    *,
    ox: int = 0,
    oy: int = 0,
) -> None:
    big = OUT_SIZE * SCALE
    img = Image.new("RGB", (big, big), bg)
    draw = ImageDraw.Draw(img)
    bbox = draw.textbbox((0, 0), char, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x = (big - tw) / 2 - bbox[0] + ox * SCALE
    y = (big - th) / 2 - bbox[1] + oy * SCALE
    draw.text((x, y), char, font=font, fill=fg)
    img = img.resize((OUT_SIZE, OUT_SIZE), Image.Resampling.LANCZOS)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path)
    print(f"wrote {path}")


def main() -> None:
    CACHE.mkdir(parents=True, exist_ok=True)
    ttf = CACHE / "fa-solid-900.ttf"
    if not ttf.is_file():
        print(f"download {TTF_URL}")
        urllib.request.urlretrieve(TTF_URL, ttf)
    font = ImageFont.truetype(str(ttf), FONT_SIZE)
    fg = (251, 241, 199)
    fg_dim = (150, 140, 130)
    bg_btn = (60, 56, 54)
    bg_off = (29, 32, 33)
    bg_pwr = (157, 0, 6)
    bg_on = (69, 133, 136)
    ch, ox, oy = GLYPHS["arrow-left"]
    _render(ch, fg, bg_btn, OUT / "nav-back.png", font, ox=ox, oy=oy)
    _render(ch, fg_dim, bg_off, OUT / "nav-back-off.png", font, ox=ox, oy=oy)
    ch, ox, oy = GLYPHS["house"]
    _render(ch, fg, bg_btn, OUT / "nav-home.png", font, ox=ox, oy=oy)
    _render(ch, fg, bg_on, OUT / "nav-home-on.png", font, ox=ox, oy=oy)
    ch, ox, oy = GLYPHS["power-off"]
    _render(ch, fg, bg_pwr, OUT / "nav-power.png", font, ox=ox, oy=oy)


if __name__ == "__main__":
    main()
