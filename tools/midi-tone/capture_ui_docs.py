#!/usr/bin/env python3
"""Capture midi-tone UI views at the 800×480 panel size for docs.

The previous ImageGrab(bbox=window) approach failed on HiDPI desktops: Tk
reports logical coordinates while the screenshot API uses physical pixels, so
only the top-left chrome was saved (title bar, POWER, a slice of the body).

This script:
  * drives a dedicated 800×480 Xvfb when the current display is not that size
  * runs the Tk UI fullscreen with no window decorations
  * grabs the X11 window / framebuffer (not a DPI-scaled bbox)
  * does not require a physical MIDI controller or working audio device
"""
from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "docs" / "screens"
PANEL = (800, 480)

sys.path.insert(0, str(HERE))


class _DummyMidiPort:
    def iter_pending(self):
        return []

    def close(self) -> None:
        pass


def _screen_size() -> tuple[int, int] | None:
    try:
        out = subprocess.check_output(
            ["xdpyinfo"], text=True, stderr=subprocess.DEVNULL, timeout=5
        )
    except Exception:
        return None
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("dimensions:"):
            # "dimensions:    800x480 pixels (...)"
            token = line.split()[1]
            w, h = token.split("x")
            return int(w), int(h)
    return None


def _ensure_panel_display() -> subprocess.Popen | None:
    """Return an Xvfb process if we had to start one."""
    size = _screen_size()
    if size == PANEL:
        return None
    display_num = 99
    sock = pathlib.Path(f"/tmp/.X11-unix/X{display_num}")
    proc = subprocess.Popen(
        [
            "Xvfb",
            f":{display_num}",
            "-screen",
            "0",
            f"{PANEL[0]}x{PANEL[1]}x24",
            "-nolisten",
            "tcp",
            "-ac",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    os.environ["DISPLAY"] = f":{display_num}"
    os.environ.pop("WAYLAND_DISPLAY", None)
    for _ in range(80):
        if sock.exists() and proc.poll() is None:
            time.sleep(0.1)
            return proc
        time.sleep(0.05)
    proc.kill()
    raise RuntimeError("Xvfb failed to start an 800x480 display")


def _patch_midi_and_audio(mt) -> None:
    """Docs capture must run without an MPK or PortAudio device."""
    mt.MidiToneApp._pick_port = lambda self: "Midi Through Port-0"  # type: ignore[method-assign]
    mt.mido.open_input = lambda name: _DummyMidiPort()  # type: ignore[assignment]
    orig_start = mt.SineEngine.start

    def _start(self) -> None:
        try:
            orig_start(self)
        except Exception as exc:
            print(f"audio skipped for docs capture: {exc}", flush=True)
            self._stream = None

    mt.SineEngine.start = _start  # type: ignore[method-assign]


def _set_demo_patch(app) -> None:
    """Use a distinctive wavetable pair so the CRT scope is readable."""
    names = list(app.engine.voice_names)
    if "vgsaw" in names and "organ" in names:
        app.engine.set_morph_pair(names.index("vgsaw"), names.index("organ"), morph=0.0)
        app._sync_voice_index_from_morph()
        if app._voice_lbl is not None:
            app._voice_lbl.configure(text=app._voice_label_text())
        app.mod_var.set(app._format_mod_line())
        app._paint_synth_waveform(force=True)


def _pump(root, seconds: float = 0.25) -> None:
    deadline = time.time() + seconds
    while time.time() < deadline:
        root.update_idletasks()
        root.update()
        time.sleep(0.03)


def grab(root, path: pathlib.Path) -> None:
    _pump(root, 0.45)
    try:
        root.config(cursor="none")
    except Exception:
        pass
    _pump(root, 0.12)
    path.parent.mkdir(parents=True, exist_ok=True)

    wid = int(root.winfo_id())
    attempts: list[list[str]] = [
        ["import", "-silent", "-window", str(wid), str(path)],
        ["scrot", "--overwrite", "--silent", str(path)],
        ["import", "-silent", "-window", "root", str(path)],
    ]
    last_err: Exception | None = None
    for cmd in attempts:
        try:
            subprocess.check_call(
                cmd,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=12,
            )
            from PIL import Image

            with Image.open(path) as img:
                w, h = img.size
            if w >= 700 and h >= 400:
                print(f"saved {path.name} ({w}x{h}) via {cmd[0]}", flush=True)
                return
            print(
                f"discarded undersized {path.name} ({w}x{h}) from {cmd[0]}",
                flush=True,
            )
        except Exception as exc:
            last_err = exc
    raise RuntimeError(f"failed to capture {path.name}: {last_err}")


def close_overlays(app) -> None:
    if getattr(app, "_power_ui_open", False):
        app._close_power_menu(restore_main=True)
    if getattr(app, "_token_ui_open", False):
        app._close_update_token(restore_main=True)
    if app._grid_open:
        app._close_voice_grid(restore_main=True)
    if app._morph_ui_open:
        app._close_morph_menu(restore_main=True)
    if app._kit_ui_open:
        app._close_kit_explorer(restore_main=True)
    if getattr(app, "_save_voice_open", False):
        app._close_save_voice(restore_main=True)
    if getattr(app, "_kaoss_scale_open", False):
        app._close_kaoss_scale_grid(restore_main=True)


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    xvfb = _ensure_panel_display()
    print(f"display {os.environ.get('DISPLAY')} size={_screen_size()}", flush=True)

    import midi_tone as mt  # noqa: E402  (DISPLAY must be set first)

    _patch_midi_and_audio(mt)

    print("starting midi-tone for screenshots...", flush=True)
    app = mt.MidiToneApp(
        port_filter="",
        list_only=False,
        max_voices=mt.DEFAULT_MAX_VOICES,
        waves_dir=mt.DEFAULT_WAVETABLE_DIR,
        fullscreen=True,
    )
    root = app.root
    try:
        root.overrideredirect(True)
    except Exception:
        pass
    try:
        root.attributes("-fullscreen", True)
    except Exception:
        pass
    root.geometry(f"{PANEL[0]}x{PANEL[1]}+0+0")
    root.minsize(*PANEL)
    root.maxsize(*PANEL)
    try:
        root.config(cursor="none")
    except Exception:
        pass
    root.lift()
    root.focus_force()
    _pump(root, 0.8)
    _set_demo_patch(app)
    _pump(root, 0.4)

    def shot(name: str, action=None) -> None:
        if action is not None:
            action()
        root.geometry(f"{PANEL[0]}x{PANEL[1]}+0+0")
        _pump(root, 0.55)
        grab(root, OUT / f"{name}.png")

    try:
        shot("00-home", lambda: app._switch_mode("home"))
        shot("01-synth", lambda: app._switch_mode("synth"))
        shot("02-seq", lambda: app._switch_mode("seq"))
        shot(
            "03-pads-edit",
            lambda: (app._switch_mode("pads"), app._phrase_set_view("edit")),
        )
        shot("04-pads-play", lambda: app._phrase_set_view("play"))
        shot("13-kaoss", lambda: app._kaoss_docs_pose())
        shot("14-kaoss-scales", lambda: app._kaoss_docs_scale_grid())
        shot(
            "05-songs",
            lambda: (app._kaoss.set_show_all(False), app._switch_mode("songs")),
        )
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
        _pump(root, 0.25)
        if hasattr(app, "_open_save_voice"):
            shot("12-save-as", app._open_save_voice)
        close_overlays(app)
        shot("13-settings", lambda: app._switch_mode("settings"))
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
        if xvfb is not None:
            xvfb.terminate()
            try:
                xvfb.wait(timeout=3)
            except Exception:
                xvfb.kill()

    print(f"captured views in {OUT}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
