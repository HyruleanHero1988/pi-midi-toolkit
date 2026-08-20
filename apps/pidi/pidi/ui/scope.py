"""Waveform scope drawing helpers for SYNTH / KIT canvases."""
from __future__ import annotations

from typing import TYPE_CHECKING, Optional

import numpy as np

from pidi.constants import SCOPE_CRT_AXIS, SCOPE_CRT_GRID, SCOPE_CRT_WAVE

if TYPE_CHECKING:
    import tkinter as tk

def draw_scope_grid(
    canvas: "tk.Canvas",
    *,
    grid_color: str = SCOPE_CRT_GRID,
    axis_color: str = SCOPE_CRT_AXIS,
    duration_sec: Optional[float] = None,
    x_label: Optional[str] = None,
) -> None:
    """Static CRT grid + axis (drawn once; wave polyline updates separately)."""
    try:
        canvas.delete("grid")
        w = int(canvas.winfo_width())
        h = int(canvas.winfo_height())
    except Exception:
        return
    if w < 8 or h < 8:
        return
    axis_h = 16 if duration_sec is not None or x_label else 0
    plot_h = max(8, h - axis_h)
    mid = plot_h * 0.5
    for frac in (0.25, 0.5, 0.75):
        y = plot_h * frac
        canvas.create_line(0, y, w, y, fill=grid_color, tags="grid")
        x = (w - 1) * frac
        canvas.create_line(x, 0, x, plot_h, fill=grid_color, tags="grid")
    canvas.create_line(0, mid, w, mid, fill=axis_color, tags="grid")
    if duration_sec is not None and duration_sec > 0:
        ticks_ms = [0]
        step = 100 if duration_sec >= 0.35 else 50
        t = step
        while t < duration_sec * 1000 - 1:
            ticks_ms.append(t)
            t += step
        end_ms = int(round(duration_sec * 1000))
        if ticks_ms[-1] != end_ms:
            ticks_ms.append(end_ms)
        for ms in ticks_ms:
            frac = ms / (duration_sec * 1000.0)
            x = frac * (w - 1)
            canvas.create_line(x, plot_h - 3, x, plot_h, fill=axis_color, tags="grid")
            canvas.create_text(
                x, h - 2, text=f"{ms}", anchor="s",
                fill=axis_color, font=("DejaVu Sans Mono", 8), tags="grid",
            )
        canvas.create_text(
            w - 2, h - 2, text="ms", anchor="se",
            fill=axis_color, font=("DejaVu Sans Mono", 8), tags="grid",
        )
    elif x_label:
        canvas.create_text(
            w // 2, h - 2, text=x_label, anchor="s",
            fill=axis_color, font=("DejaVu Sans Mono", 9), tags="grid",
        )


def blank_waveform_on_canvas(canvas: "tk.Canvas") -> None:
    """Clear the trace immediately (leave the CRT grid) so stale waves don't linger."""
    try:
        canvas.delete("wave")
    except Exception:
        pass


def draw_waveform_on_canvas(
    canvas: "tk.Canvas",
    samples: np.ndarray,
    *,
    color: str = SCOPE_CRT_WAVE,
    grid_color: str = SCOPE_CRT_GRID,
    axis_color: str = SCOPE_CRT_AXIS,
    duration_sec: Optional[float] = None,
    x_label: Optional[str] = None,
    redraw_grid: bool = False,
) -> None:
    """Paint a CRT-green scope. Grid is cached; only the trace is rewritten."""
    try:
        canvas.delete("wave")
        w = int(canvas.winfo_width())
        h = int(canvas.winfo_height())
    except Exception:
        return
    if w < 8 or h < 8:
        return

    if redraw_grid or not canvas.find_withtag("grid"):
        draw_scope_grid(
            canvas,
            grid_color=grid_color,
            axis_color=axis_color,
            duration_sec=duration_sec,
            x_label=x_label,
        )

    axis_h = 16 if duration_sec is not None or x_label else 0
    plot_h = max(8, h - axis_h)
    mid = plot_h * 0.5

    if samples is None or len(samples) < 2:
        return
    # Fewer points = much cheaper Tk polyline on Pi 2
    pts = downsample_waveform(samples, max(48, w // 3))
    peak = float(np.max(np.abs(pts))) or 1.0
    y_scale = (plot_h * 0.40) / peak
    coords: List[float] = []
    n = len(pts)
    for i, v in enumerate(pts):
        x = (i / max(1, n - 1)) * (w - 1)
        y = mid - float(v) * y_scale
        coords.extend((x, y))
    canvas.create_line(*coords, fill=color, width=2, smooth=False, tags="wave")


