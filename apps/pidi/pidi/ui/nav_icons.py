"""Global nav icons — Font Awesome Free solid glyphs, pre-rendered to PNG.

Source: Font Awesome Free 6 (SIL OFL) via ``fa-solid-900.ttf``.
Re-render with ``python scripts/dev/render_nav_icons.py``.
"""
from __future__ import annotations

import pathlib
import time
import tkinter as tk
from typing import Callable, Dict

_ICON_DIR = pathlib.Path(__file__).resolve().parent / "icons"

# Icon PNG edge length (pixels). Buttons match the image exactly.
NAV_BTN_SIZE = 64


def _load_png(name: str) -> tk.PhotoImage:
    path = _ICON_DIR / name
    if not path.is_file():
        raise FileNotFoundError(f"missing nav icon: {path}")
    return tk.PhotoImage(file=str(path))


def load_nav_icons() -> Dict[str, tk.PhotoImage]:
    """Load PhotoImages; keep the returned dict alive (Tk drops unreferenced images)."""
    return {
        "back": _load_png("nav-back.png"),
        "back_off": _load_png("nav-back-off.png"),
        "home": _load_png("nav-home.png"),
        "home_on": _load_png("nav-home-on.png"),
        "power": _load_png("nav-power.png"),
    }


def style_nav_icon_button(widget: tk.Misc, image: tk.PhotoImage, *, bg: str) -> None:
    """Apply image + bg to a nav Label (preferred) or Button."""
    widget.configure(image=image, bg=bg)  # type: ignore[call-arg]
    if isinstance(widget, tk.Button):
        widget.configure(
            activebackground=bg,
            text="",
            compound=tk.CENTER,
            font=("DejaVu Sans", 1),
            highlightthickness=0,
            bd=0,
            borderwidth=0,
            relief=tk.FLAT,
            padx=0,
            pady=0,
            width=image.width(),
            height=image.height(),
        )
    else:
        widget.configure(
            highlightthickness=0,
            bd=0,
            borderwidth=0,
            padx=0,
            pady=0,
            width=image.width(),
            height=image.height(),
        )
    widget.image = image  # type: ignore[attr-defined]


def make_nav_icon_label(
    parent: tk.Misc,
    image: tk.PhotoImage,
    command: Callable[[], None],
    *,
    bg: str,
) -> tk.Label:
    """Touch Label that shows a centered icon (avoids Tk Button text-metric skew)."""
    lbl = tk.Label(
        parent,
        image=image,
        bg=bg,
        bd=0,
        borderwidth=0,
        highlightthickness=0,
        padx=0,
        pady=0,
        cursor="hand2",
        takefocus=0,
    )
    lbl.image = image  # type: ignore[attr-defined]

    def _fire(_event: object = None) -> str:
        now = time.monotonic()
        last = getattr(lbl, "_last_fire", 0.0)
        if now - last < 0.18:
            return "break"
        lbl._last_fire = now  # type: ignore[attr-defined]
        command()
        return "break"

    lbl.bind("<ButtonPress-1>", _fire)
    return lbl
