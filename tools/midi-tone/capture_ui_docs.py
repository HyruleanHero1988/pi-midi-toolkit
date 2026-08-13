#!/usr/bin/env python3
"""Capture midi-tone UI views locally (no Pi required) for docs."""
from __future__ import annotations

import pathlib
import sys
import time

from PIL import ImageGrab

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "docs" / "screens"
sys.path.insert(0, str(HERE))

import midi_tone as mt  # noqa: E402


def grab(root, path: pathlib.Path) -> None:
    root.update_idletasks()
    root.update()
    time.sleep(0.4)
    root.update()
    x = int(root.winfo_rootx())
    y = int(root.winfo_rooty())
    w = int(root.winfo_width())
    h = int(root.winfo_height())
    box = (x, y, x + max(w, 1), y + max(h, 1))
    img = ImageGrab.grab(bbox=box)
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path)
    print(f"saved {path.name} ({img.size[0]}x{img.size[1]})", flush=True)


def close_overlays(app: mt.MidiToneApp) -> None:
    if getattr(app, "_power_ui_open", False):
        app._close_power_menu(restore_main=True)
    if app._grid_open:
        app._close_voice_grid(restore_main=True)
    if app._morph_ui_open:
        app._close_morph_menu(restore_main=True)
    if app._kit_ui_open:
        app._close_kit_explorer(restore_main=True)
    if getattr(app, "_save_voice_open", False):
        app._close_save_voice(restore_main=True)


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    print("starting midi-tone for screenshots...", flush=True)
    # Avoid full-monitor geometry during capture
    app = mt.MidiToneApp(
        port_filter="",
        list_only=False,
        max_voices=mt.DEFAULT_MAX_VOICES,
        waves_dir=mt.DEFAULT_WAVETABLE_DIR,
        fullscreen=False,
    )
    root = app.root
    root.geometry("800x480+60+60")
    root.minsize(800, 480)
    root.maxsize(800, 480)
    root.lift()
    root.focus_force()
    for _ in range(8):
        root.update_idletasks()
        root.update()
        time.sleep(0.05)
    time.sleep(0.6)

    def shot(name: str, action=None) -> None:
        if action is not None:
            action()
        root.geometry("800x480+60+60")
        for _ in range(6):
            root.update_idletasks()
            root.update()
            time.sleep(0.05)
        time.sleep(0.35)
        grab(root, OUT / f"{name}.png")

    try:
        shot("01-synth")
        shot("02-seq", lambda: app._switch_mode("seq"))
        shot(
            "03-pads-edit",
            lambda: (app._switch_mode("pads"), app._phrase_set_view("edit")),
        )
        shot("04-pads-play", lambda: app._phrase_set_view("play"))
        shot("05-songs", lambda: app._switch_mode("songs"))
        shot("06-presets", lambda: app._switch_mode("presets"))
        shot("07-log", lambda: app._switch_mode("log"))
        shot(
            "08-voices",
            lambda: (
                close_overlays(app),
                app._switch_mode("synth"),
                app._open_voice_grid(),
            ),
        )
        shot(
            "09-morph",
            lambda: (
                close_overlays(app),
                app._switch_mode("synth"),
                app._open_morph_menu(),
            ),
        )
        shot(
            "10-kit",
            lambda: (
                close_overlays(app),
                app._switch_mode("synth"),
                app._open_kit_explorer(),
            ),
        )
        shot(
            "11-power",
            lambda: (
                close_overlays(app),
                app._switch_mode("synth"),
                app._open_power_menu(),
            ),
        )
        close_overlays(app)
        app._switch_mode("synth")
        app._open_voice_grid()
        root.update()
        if hasattr(app, "_open_save_voice"):
            shot("12-save-as", app._open_save_voice)
        close_overlays(app)
        app._switch_mode("synth")
    finally:
        try:
            app.engine.stop()
        except Exception:
            pass
        try:
            if app._inport is not None:
                app._inport.close()
        except Exception:
            pass
        try:
            root.destroy()
        except Exception:
            pass

    print(f"captured views in {OUT}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
