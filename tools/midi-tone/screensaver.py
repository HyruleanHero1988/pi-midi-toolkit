#!/usr/bin/env python3
"""Idle blanking for the TFT kiosk — image-retention / burn-in protection.

The panel is an IPS LCD (BigTreeTech Pi TFT70). The kiosk chrome is bold and
mostly static, so a long jam with no touch still ghosts the same pixels.
midi-tone already turns X DPMS off so the kiosk never blanks on its own; this
module is the replacement:

- After a stretch of **no touch**, cover the UI and dim the DSI backlight.
- While the UI is up, slowly shift it by a couple of pixels so labels/boxes
  do not sit on the same cells for the whole session.

Audio / sequencer / songs keep running. Only a tap wakes the panel — MIDI
never counts as activity.
"""

from __future__ import annotations

import math
import os
import pathlib
from typing import Mapping, Optional, Sequence, Tuple


DEFAULT_TIMEOUT_SEC = 180.0  # 3 minutes
TIMEOUT_PRESETS: Tuple[float, ...] = (60.0, 180.0, 600.0, 0.0)
PIXEL_SHIFT_AMPLITUDE = 2
PIXEL_SHIFT_DWELL_SEC = 40.0


def timeout_from_env(env: Optional[Mapping[str, str]] = None) -> float:
    src = os.environ if env is None else env
    raw = src.get("MIDI_TONE_SCREENSAVER_SEC")
    if raw is None or raw.strip() == "":
        return DEFAULT_TIMEOUT_SEC
    try:
        return max(0.0, float(raw))
    except ValueError:
        return DEFAULT_TIMEOUT_SEC


def next_timeout_preset(current: float, presets: Sequence[float] = TIMEOUT_PRESETS) -> float:
    values = [float(p) for p in presets]
    cur = float(current)
    for i, value in enumerate(values):
        if abs(value - cur) < 0.5:
            return values[(i + 1) % len(values)]
    return values[0] if values else DEFAULT_TIMEOUT_SEC


def timeout_label(seconds: float) -> str:
    sec = float(seconds)
    if sec <= 0:
        return "BLANK OFF"
    minutes = sec / 60.0
    if abs(minutes - round(minutes)) < 0.05 and round(minutes) >= 1:
        return f"BLANK {int(round(minutes))} MIN"
    return f"BLANK {int(round(sec))}s"


def orbit_xy(
    elapsed: float,
    width: int,
    height: int,
    label_w: int,
    label_h: int,
    margin: int = 16,
) -> Tuple[int, int]:
    """Slow incommensurate Lissajous path so the wake hint never sits still."""
    span_x = max(0, int(width) - int(label_w) - 2 * margin)
    span_y = max(0, int(height) - int(label_h) - 2 * margin)
    # Periods 47s / 31s don't close quickly — classic pixel-shift screensaver.
    x = margin + int((math.sin(elapsed * 2.0 * math.pi / 47.0) * 0.5 + 0.5) * span_x)
    y = margin + int(
        (math.sin(elapsed * 2.0 * math.pi / 31.0 + 1.2) * 0.5 + 0.5) * span_y
    )
    return x, y


def pixel_shift_xy(
    elapsed: float,
    amplitude: int = PIXEL_SHIFT_AMPLITUDE,
    dwell_sec: float = PIXEL_SHIFT_DWELL_SEC,
) -> Tuple[int, int]:
    """1–2px square orbit. Dwells so the chrome is not constantly jittering."""
    amp = max(1, int(amplitude))
    dwell = dwell_sec if dwell_sec > 0 else PIXEL_SHIFT_DWELL_SEC
    idx = int(max(0.0, float(elapsed)) // dwell) % 8
    pattern = (
        (0, 0),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
    )
    dx, dy = pattern[idx]
    return dx * amp, dy * amp


class IdleWatch:
    """Monotonic idle timer. Tk must not be called from the MIDI thread."""

    def __init__(self, timeout_sec: float = DEFAULT_TIMEOUT_SEC, *, now: Optional[float] = None) -> None:
        self.timeout_sec = max(0.0, float(timeout_sec))
        self._last = 0.0 if now is None else float(now)
        if now is None:
            import time

            self._last = time.monotonic()
        self.active = False

    def poke(self, now: Optional[float] = None) -> bool:
        """Record activity. Returns True if this dismissed an active saver."""
        import time

        self._last = time.monotonic() if now is None else float(now)
        if self.active:
            self.active = False
            return True
        return False

    def due(self, now: Optional[float] = None) -> bool:
        if self.active or self.timeout_sec <= 0:
            return False
        import time

        t = time.monotonic() if now is None else float(now)
        return (t - self._last) >= self.timeout_sec

    def activate(self) -> bool:
        """Mark saver on. Returns False if it was already showing."""
        if self.active:
            return False
        self.active = True
        return True


class PanelBacklight:
    """Best-effort DSI backlight via /sys/class/backlight (needs video group)."""

    def __init__(self, root: Optional[pathlib.Path] = None) -> None:
        self.root = pathlib.Path("/sys/class/backlight") if root is None else pathlib.Path(root)
        self._path: Optional[pathlib.Path] = None
        self._saved: Optional[int] = None
        self._power_path: Optional[pathlib.Path] = None
        self._saved_power: Optional[int] = None

    def _find(self) -> Optional[pathlib.Path]:
        if self._path is not None and self._path.is_file():
            return self._path
        if not self.root.is_dir():
            return None
        candidates = sorted(p for p in self.root.glob("*/brightness") if p.is_file())
        if not candidates:
            return None
        self._path = candidates[0]
        power = self._path.parent / "bl_power"
        self._power_path = power if power.is_file() else None
        return self._path

    def dim(self) -> bool:
        path = self._find()
        if path is None:
            return False
        try:
            current = int(path.read_text(encoding="ascii").strip() or "0")
        except (OSError, ValueError):
            return False
        if self._saved is None:
            self._saved = current
        if self._power_path is not None and self._saved_power is None:
            try:
                self._saved_power = int(
                    self._power_path.read_text(encoding="ascii").strip() or "0"
                )
            except (OSError, ValueError):
                self._saved_power = None
        ok = False
        try:
            path.write_text("0\n", encoding="ascii")
            ok = True
        except OSError:
            pass
        if self._power_path is not None:
            try:
                # 4 = FB_BLANK_POWERDOWN
                self._power_path.write_text("4\n", encoding="ascii")
                ok = True
            except OSError:
                pass
        return ok

    def restore(self) -> bool:
        path = self._path
        if path is None:
            return False
        ok = False
        if self._power_path is not None:
            value = 0 if self._saved_power is None else int(self._saved_power)
            try:
                self._power_path.write_text(f"{value}\n", encoding="ascii")
                ok = True
            except OSError:
                pass
        if self._saved is not None:
            try:
                path.write_text(f"{int(self._saved)}\n", encoding="ascii")
                ok = True
            except OSError:
                pass
        self._saved = None
        self._saved_power = None
        return ok
