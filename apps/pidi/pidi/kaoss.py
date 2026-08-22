"""Kaoss-style XY pad — Kaossilator play + original Kaoss Pad MIDI.

Headless on purpose (no Tk, no audio) so the scale / hold / gate / CC rules
can be unit-tested on any machine. The kiosk applies ``KaossEvent`` lists to
the onboard soft-synth and/or a USB MIDI out port.

Behaviour follows the simplest Korg units:

* **Kaossilator KO-1** — X = scale-quantized pitch, Y = tone; lift = note-off
  unless HOLD; SCALE + KEY; GATE ARP retriggers while the pad is down.
* **Kaoss Pad (factory MIDI)** — X = CC#12, Y = CC#13, pad touch = CC#92
  (127 down / 0 up). HOLD freezes the last XY after the finger lifts.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Sequence, Tuple


# Factory MIDI from the original Kaoss Pad owner's manual.
KAOSS_CC_X = 12          # Effect Control 1
KAOSS_CC_Y = 13          # Effect Control 2
KAOSS_CC_TOUCH = 92      # Effect 2 Depth — pad on/off
KAOSS_OUT_MODES = ("local", "usb", "both")

# C3 is the Kaossilator-ish default root (playable without going sub-bass).
DEFAULT_ROOT_MIDI = 48
DEFAULT_BPM = 120.0
NOTE_NAMES = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B")
# Left-edge C of the pad (Kaossilator OCT +/−). C1 .. C5.
ROOT_OCTAVE_MIDI: Tuple[int, ...] = (24, 36, 48, 60, 72)


@dataclass(frozen=True)
class KaossScale:
    """One factory Kaossilator scale. ``short`` is the 3-letter Korg display code."""

    id: str
    label: str
    short: str
    degrees: Tuple[int, ...]
    curated: bool = False


# Official Kaossilator PRO SCALE LIST (p.99) plus PRO+ / KO-2 extras
# (harmonic minor, melodic minor, Chinese, bass line). OFF = no diatonic lock
# (chromatic — MIDI notes can't go microtonal).
_SCALE_DEFS: Tuple[KaossScale, ...] = (
    KaossScale("off", "OFF", "OFF", (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11)),
    KaossScale("chromatic", "CHROMATIC", "CHR", (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11), True),
    KaossScale("ionian", "MAJOR", "ION", (0, 2, 4, 5, 7, 9, 11), True),
    KaossScale("dorian", "DORIAN", "DOR", (0, 2, 3, 5, 7, 9, 10), True),
    KaossScale("phrygian", "PHRYGIAN", "PHR", (0, 1, 3, 5, 7, 8, 10)),
    KaossScale("lydian", "LYDIAN", "LYD", (0, 2, 4, 6, 7, 9, 11)),
    KaossScale("mixolydian", "MIXOLYDIAN", "MXL", (0, 2, 4, 5, 7, 9, 10), True),
    KaossScale("aeolian", "MINOR", "AEO", (0, 2, 3, 5, 7, 8, 10), True),
    KaossScale("locrian", "LOCRIAN", "LOC", (0, 1, 3, 5, 6, 8, 10)),
    KaossScale("harmonic", "HARM MINOR", "HMI", (0, 2, 3, 5, 7, 8, 11), True),
    KaossScale("melodic", "MEL MINOR", "MMI", (0, 2, 3, 5, 7, 9, 11)),
    KaossScale("major_blues", "MAJ BLUES", "MAB", (0, 3, 4, 7, 9, 10)),
    KaossScale("blues", "BLUES", "MIB", (0, 3, 5, 6, 7, 10), True),
    KaossScale("diminish", "DIMINISH", "DIM", (0, 2, 3, 5, 6, 8, 9, 11)),
    KaossScale("combo_dim", "COMBO DIM", "CDM", (0, 1, 3, 4, 6, 7, 9, 10)),
    KaossScale("major_pent", "MAJ PENT", "MAP", (0, 2, 4, 7, 9), True),
    KaossScale("minor_pent", "MIN PENT", "MIP", (0, 3, 5, 7, 10), True),
    KaossScale("raga_bhairav", "BHAIRAV", "RG1", (0, 1, 4, 5, 7, 8, 11)),
    KaossScale("raga_gamanasrama", "GAMANASRAMA", "RG2", (0, 1, 4, 6, 7, 9, 11)),
    KaossScale("raga_todi", "TODI", "RG3", (0, 1, 3, 6, 7, 8, 11)),
    KaossScale("spanish", "SPANISH", "SPN", (0, 1, 3, 4, 5, 7, 8, 10), True),
    KaossScale("gypsy", "GYPSY", "GYP", (0, 2, 3, 6, 7, 8, 11)),
    KaossScale("arabian", "ARABIAN", "ARB", (0, 2, 4, 5, 6, 8, 10)),
    KaossScale("egyptian", "EGYPTIAN", "EGY", (0, 2, 5, 7, 10)),
    KaossScale("hawaiian", "HAWAIIAN", "HWI", (0, 2, 3, 7, 9)),
    KaossScale("pelog", "PELOG", "PLG", (0, 1, 3, 7, 8)),
    KaossScale("miyakobushi", "MIYAKOBUSHI", "JPN", (0, 1, 5, 7, 8)),
    KaossScale("ryukyu", "RYUKYU", "RKY", (0, 4, 5, 7, 11), True),
    KaossScale("chinese", "CHINESE", "CHN", (0, 4, 6, 7, 11)),
    KaossScale("bassline", "BASS LINE", "BAS", (0, 7, 10), True),
    KaossScale("whole", "WHOLE TONE", "WHL", (0, 2, 4, 6, 8, 10), True),
    KaossScale("min3", "MIN 3RDS", "MI3", (0, 3, 6, 9)),
    KaossScale("maj3", "MAJ 3RDS", "3RD", (0, 4, 8)),
    KaossScale("fourth", "4THS", "4TH", (0, 5, 10)),
    KaossScale("fifth", "5THS", "5TH", (0, 7)),
    KaossScale("octave", "OCTAVE", "OCT", (0,)),
)

SCALES: Dict[str, Tuple[int, ...]] = {s.id: s.degrees for s in _SCALE_DEFS}
SCALE_BY_ID: Dict[str, KaossScale] = {s.id: s for s in _SCALE_DEFS}
SCALE_ORDER: Tuple[str, ...] = tuple(s.id for s in _SCALE_DEFS if s.curated)
SCALE_ORDER_ALL: Tuple[str, ...] = tuple(s.id for s in _SCALE_DEFS)
SCALE_LABELS: Dict[str, str] = {s.id: s.label for s in _SCALE_DEFS}


@dataclass(frozen=True)
class KaossProgram:
    """One pad mapping. ``kind`` decides whether X plays notes or only FX."""

    id: str
    label: str
    kind: str                          # "note" | "fx"
    x_param: Optional[str] = None      # local 0..1 param (FX programs)
    y_param: Optional[str] = None      # local 0..1 param (Y always)
    x_axis: str = "PITCH"
    y_axis: str = "TONE"
    curated: bool = False


# Curated = the obvious starter set. The rest are every XY mapping the
# onboard engine can actually drive (Kaossilator-style Y targets).
PROGRAMS: Tuple[KaossProgram, ...] = (
    KaossProgram("lead", "LEAD", "note", y_param="tone", x_axis="PITCH", y_axis="TONE", curated=True),
    KaossProgram("morph", "MORPH", "note", y_param="morph", x_axis="PITCH", y_axis="MORPH", curated=True),
    KaossProgram("vib", "VIB", "note", y_param="vib", x_axis="PITCH", y_axis="VIB", curated=True),
    KaossProgram("level", "LEVEL", "note", y_param="level", x_axis="PITCH", y_axis="LEVEL"),
    KaossProgram("decay", "DECAY", "note", y_param="release", x_axis="PITCH", y_axis="DECAY"),
    KaossProgram("attack", "ATTACK", "note", y_param="attack", x_axis="PITCH", y_axis="ATTACK"),
    KaossProgram("octave", "OCTAVE", "note", y_param="octave", x_axis="PITCH", y_axis="OCTAVE"),
    KaossProgram("grit", "GRIT", "note", y_param="drive", x_axis="PITCH", y_axis="DRIVE"),
    KaossProgram("delay", "DELAY", "note", y_param="delay_mix", x_axis="PITCH", y_axis="DLY MIX"),
    KaossProgram("filter", "FILTER", "fx", x_param="tone", y_param="morph", x_axis="TONE", y_axis="MORPH", curated=True),
    KaossProgram("echo", "ECHO", "fx", x_param="delay_time", y_param="delay_mix", x_axis="DLY T", y_axis="DLY MIX", curated=True),
    KaossProgram("swell", "SWELL", "fx", x_param="attack", y_param="delay_mix", x_axis="ATTACK", y_axis="DLY MIX", curated=True),
    KaossProgram("env", "ENV", "fx", x_param="attack", y_param="release", x_axis="ATTACK", y_axis="DECAY", curated=True),
    KaossProgram("drive", "DRIVE", "fx", x_param="drive", y_param="reverb_mix", x_axis="DRIVE", y_axis="REVERB", curated=True),
    KaossProgram("space", "SPACE", "fx", x_param="delay_mix", y_param="reverb_mix", x_axis="ECHO", y_axis="REVERB", curated=True),
    KaossProgram("reso", "RESO", "fx", x_param="tone", y_param="delay_fb", x_axis="TONE", y_axis="FB"),
    KaossProgram("wash", "WASH", "fx", x_param="reverb_size", y_param="reverb_mix", x_axis="SIZE", y_axis="REVERB"),
    KaossProgram("crush", "CRUSH", "fx", x_param="drive", y_param="tone", x_axis="DRIVE", y_axis="TONE"),
    KaossProgram("sweep", "SWEEP", "fx", x_param="tone", y_param="delay_time", x_axis="TONE", y_axis="DLY T"),
)
PROGRAM_IDS: Tuple[str, ...] = tuple(p.id for p in PROGRAMS if p.curated)
PROGRAM_IDS_ALL: Tuple[str, ...] = tuple(p.id for p in PROGRAMS)
PROGRAM_BY_ID: Dict[str, KaossProgram] = {p.id: p for p in PROGRAMS}

# LED field — Kaoss Quad / KP3 vibe. Wide TFT gets more columns than rows.
# This grid is independent of the play/touch cells; matching them made the Pi
# crawl, so CELLS stays the original 12×7 light field.
LED_COLS = 12
LED_ROWS = 7
VIZ_STYLES: Tuple[str, ...] = ("glow", "cells")
VIZ_STYLE_LABELS: Dict[str, str] = {
    "glow": "GLOW",
    "cells": "CELLS",
}
DEFAULT_VIZ_STYLE = "glow"
GRID_WIDTH_MIN = 1
GRID_WIDTH_MAX = 5
DEFAULT_GRID_WIDTH = 2
# Program tint so FILTER doesn't light up the same magenta as LEAD.
PROGRAM_HUE: Dict[str, float] = {
    "lead": 0.93,
    "morph": 0.80,
    "vib": 0.55,
    "level": 0.12,
    "decay": 0.08,
    "attack": 0.18,
    "octave": 0.45,
    "grit": 0.02,
    "delay": 0.62,
    "filter": 0.72,
    "echo": 0.58,
    "env": 0.22,
    "drive": 0.04,
    "space": 0.66,
    "reso": 0.85,
    "wash": 0.70,
    "crush": 0.98,
    "sweep": 0.50,
}


@dataclass(frozen=True)
class GatePattern:
    id: str
    label: str
    beats: float = 0.0     # 0 = off
    duty: float = 0.55


GATE_PATTERNS: Tuple[GatePattern, ...] = (
    GatePattern("off", "GATE OFF", 0.0, 0.0),
    GatePattern("8th", "GATE 1/8", 0.5, 0.55),
    GatePattern("16th", "GATE 1/16", 0.25, 0.50),
    GatePattern("trip", "GATE TRIP", 1.0 / 3.0, 0.50),
)
GATE_IDS: Tuple[str, ...] = tuple(g.id for g in GATE_PATTERNS)
GATE_BY_ID: Dict[str, GatePattern] = {g.id: g for g in GATE_PATTERNS}


@dataclass(frozen=True)
class KaossEvent:
    """One thing the UI should do. ``kind`` is the discriminator."""

    kind: str
    note: int = 0
    velocity: int = 0
    control: int = 0
    value: int = 0
    param: str = ""
    param_value: float = 0.0


def clamp01(value: float) -> float:
    return 0.0 if value < 0.0 else 1.0 if value > 1.0 else float(value)


def midi_cc(value: float) -> int:
    return max(0, min(127, int(round(clamp01(value) * 127.0))))


def glow_step(current: float, target: float, dt: float) -> float:
    """Ease the GLOW blob in/out. Attack is quicker than release."""
    cur = clamp01(current)
    tgt = clamp01(target)
    step = max(0.0, min(0.08, float(dt)))
    if tgt >= cur:
        if tgt == cur:
            return cur
        return clamp01(cur + step / 0.16)
    return clamp01(cur - step / 0.32)


def glow_radii(span: float, amp: float) -> Tuple[float, float, float]:
    """Three concentric blooms. Size follows the fade envelope only, not XY."""
    span = max(1.0, float(span))
    amp = clamp01(amp)
    scale = 0.82
    outer = span * 0.52 * scale * (amp ** 1.35)
    mid = span * 0.28 * scale * (amp ** 1.12)
    core = span * 0.11 * scale * amp
    return outer, mid, core


def clamp_grid_width(value: Any) -> int:
    try:
        n = int(round(float(value)))
    except (TypeError, ValueError):
        n = DEFAULT_GRID_WIDTH
    return max(GRID_WIDTH_MIN, min(GRID_WIDTH_MAX, n))


def grid_line_widths(weight: int) -> Tuple[int, int]:
    """(regular, octave-root) canvas stroke widths for the play-grid overlay."""
    regular = clamp_grid_width(weight)
    return regular, regular + 1


def note_name(note: int) -> str:
    n = int(note) & 0x7F
    return f"{NOTE_NAMES[n % 12]}{(n // 12) - 1}"


def scale_notes(
    scale_id: str,
    key: int,
    *,
    root_midi: int = DEFAULT_ROOT_MIDI,
    octaves: int = 2,
) -> List[int]:
    """MIDI notes in ``[root_midi, root_midi + octaves*12]`` that sit in the scale."""
    degrees = SCALES.get(scale_id, SCALES["ionian"])
    key = int(key) % 12
    root = max(0, min(127, int(root_midi)))
    span = max(1, min(4, int(octaves)))
    top = min(127, root + span * 12)
    notes = [n for n in range(root, top + 1) if ((n - key) % 12) in degrees]
    return notes or [root]


def hsv_to_rgb(h: float, s: float, v: float) -> Tuple[int, int, int]:
    """HSV in 0..1 → 8-bit RGB. Cheap enough to run per LED on a Pi 2."""
    h = h % 1.0
    s = 0.0 if s < 0.0 else 1.0 if s > 1.0 else float(s)
    v = 0.0 if v < 0.0 else 1.0 if v > 1.0 else float(v)
    if s <= 0.0:
        c = int(round(v * 255.0))
        return (c, c, c)
    sector = h * 6.0
    i = int(sector)
    f = sector - i
    p = v * (1.0 - s)
    q = v * (1.0 - f * s)
    t = v * (1.0 - (1.0 - f) * s)
    i = i % 6
    if i == 0:
        r, g, b = v, t, p
    elif i == 1:
        r, g, b = q, v, p
    elif i == 2:
        r, g, b = p, v, t
    elif i == 3:
        r, g, b = p, q, v
    elif i == 4:
        r, g, b = t, p, v
    else:
        r, g, b = v, p, q
    return (int(round(r * 255.0)), int(round(g * 255.0)), int(round(b * 255.0)))


def rgb_hex(rgb: Tuple[int, int, int]) -> str:
    r, g, b = rgb
    return f"#{max(0, min(255, r)):02x}{max(0, min(255, g)):02x}{max(0, min(255, b)):02x}"


def program_hue(program_id: str) -> float:
    return float(PROGRAM_HUE.get(program_id, 0.93))


def normalize_viz_style(value: Any) -> str:
    style = str(value or "").strip().lower()
    # Retired: play-grid-matched LEDs ("live") and the near-black "static" fill.
    if style in ("static", "live"):
        style = "cells"
    return style if style in VIZ_STYLES else DEFAULT_VIZ_STYLE


def pad_led_hex(
    col: int,
    row: int,
    *,
    cols: int = LED_COLS,
    rows: int = LED_ROWS,
    t: float = 0.0,
    finger: Optional[Tuple[float, float]] = None,
    trail: Sequence[Tuple[float, float, float]] = (),
    ripples: Sequence[Tuple[float, float, float]] = (),
    hold: bool = False,
    gate_flash: float = 0.0,
    hue_shift: float = 0.0,
) -> str:
    """One LED in the pad field. ``row`` 0 is the bottom (Kaoss Y = 0).

    ``trail`` / ``ripples`` entries are ``(x, y, age)`` with age 1 = fresh, 0 = gone.
    """
    cols = max(2, int(cols))
    rows = max(2, int(rows))
    lx = float(col) / float(cols - 1)
    ly = float(row) / float(rows - 1)
    wave = 0.5 + 0.5 * math.sin(float(t) * 1.6 + col * 0.45 + row * 0.38)
    hue = (lx * 0.70 + float(hue_shift) + float(t) * 0.035) % 1.0
    sat = 0.82
    val = 0.045 + 0.09 * wave
    if hold:
        val += 0.05
    if finger is not None:
        fx, fy = finger
        dist = math.hypot(lx - fx, ly - fy)
        glow = max(0.0, 1.0 - dist / 0.40) ** 1.45
        val = min(1.0, val + glow * 0.92)
        sat = min(1.0, 0.55 + glow * 0.45)
        hue = (hue * (1.0 - glow * 0.55) + (fx * 0.70 + float(hue_shift)) * glow) % 1.0
    for tx, ty, age in trail:
        dist = math.hypot(lx - tx, ly - ty)
        spark = max(0.0, 1.0 - dist / 0.22) ** 1.8 * clamp01(age) * 0.55
        val = min(1.0, val + spark)
    for rx, ry, age in ripples:
        radius = 0.08 + clamp01(age) * 0.72
        dist = math.hypot(lx - rx, ly - ry)
        ring = max(0.0, 1.0 - abs(dist - radius) / 0.10)
        val = min(1.0, val + ring * (1.0 - clamp01(age)) * 0.65)
    val = min(1.0, val + clamp01(gate_flash) * 0.20)
    return rgb_hex(hsv_to_rgb(hue, sat, val))


def note_index_at_x(x: float, n_notes: int) -> int:
    """Equal-width cell index for pad X in 0..1. Last cell includes x=1."""
    n = max(1, int(n_notes))
    idx = int(clamp01(x) * n)
    return n - 1 if idx >= n else idx


def note_cell_edges(n_notes: int) -> List[float]:
    """Inclusive 0..1 edges of equal-width note cells (N notes → N+1 edges)."""
    n = max(1, int(n_notes))
    return [i / n for i in range(n + 1)]


def note_grid_xs(n_notes: int, width: int) -> List[int]:
    """Pixel X of each ``note_cell_edges`` line, clamped onto the canvas."""
    w = max(1, int(width))
    xs: List[int] = []
    for frac in note_cell_edges(n_notes):
        x = int(round(frac * w))
        xs.append(w - 1 if x >= w else x)
    return xs


def note_at_x(x: float, notes: Sequence[int]) -> int:
    if not notes:
        return 60
    return int(notes[note_index_at_x(x, len(notes))])


def velocity_at_y(y: float) -> int:
    """Softer at the bottom of the pad, full at the top — still always audible."""
    return max(72, min(127, 72 + int(round(clamp01(y) * 55.0))))


class KaossPad:
    """XY pad state machine. Thread-unsafe; the UI owns it on the Tk thread."""

    def __init__(self) -> None:
        self.program_id: str = "lead"
        self.scale_id: str = "ionian"
        self.key: int = 0
        self.octaves: int = 2
        self.root_midi: int = DEFAULT_ROOT_MIDI
        self.gate_id: str = "off"
        self.bpm: float = DEFAULT_BPM
        self.hold: bool = False
        self.show_all: bool = False
        self.show_axis_labels: bool = True
        self.show_grid_lines: bool = True
        self.viz_style: str = DEFAULT_VIZ_STYLE
        self.grid_width: int = DEFAULT_GRID_WIDTH
        self.out_mode: str = "local"
        self.channel: int = 0
        self.cc_x: int = KAOSS_CC_X
        self.cc_y: int = KAOSS_CC_Y
        self.cc_touch: int = KAOSS_CC_TOUCH
        self.x: float = 0.5
        self.y: float = 0.5
        self.touching: bool = False
        self._note: Optional[int] = None
        self._latched_note: Optional[int] = None
        self._latched_velocity: int = 100
        self._cc_x_sent: Optional[int] = None
        self._cc_y_sent: Optional[int] = None
        self._touch_sent: Optional[int] = None
        self._gate_on: bool = False
        self._gate_t0: float = 0.0
        self._gate_period: float = 0.0

    # --- lookups ----------------------------------------------------------

    def program(self) -> KaossProgram:
        return PROGRAM_BY_ID.get(self.program_id, PROGRAMS[0])

    def scale(self) -> KaossScale:
        return SCALE_BY_ID.get(self.scale_id, SCALE_BY_ID["ionian"])

    def scale_ids(self) -> Tuple[str, ...]:
        return SCALE_ORDER_ALL if self.show_all else SCALE_ORDER

    def program_ids(self) -> Tuple[str, ...]:
        return PROGRAM_IDS_ALL if self.show_all else PROGRAM_IDS

    def scale_label(self) -> str:
        """Full name for the SCALE button / picker. Never the 3-letter Korg code."""
        return self.scale().label

    def gate(self) -> GatePattern:
        return GATE_BY_ID.get(self.gate_id, GATE_PATTERNS[0])

    def notes(self) -> List[int]:
        return scale_notes(
            self.scale_id, self.key, root_midi=self.root_midi, octaves=self.octaves
        )

    def is_active(self) -> bool:
        return self.touching or (self.hold and self._is_latched())

    def _is_latched(self) -> bool:
        """HOLD latches the last pad press, including gate-off gaps with no note."""
        return self._touch_sent == 127

    def sounding_note(self) -> Optional[int]:
        return self._note

    def gate_flash(self) -> float:
        """1 while GATE ARP is in the on phase — drives the LED field pulse."""
        if self.program().kind != "note":
            return 0.0
        if self.gate().beats <= 0.0 or not self.is_active():
            return 0.0
        return 1.0 if self._gate_on else 0.0

    def viz_pulse(self, now: float) -> float:
        """Beat-synced 0..1 pulse for the pad rim and idle field."""
        period = 60.0 / max(40.0, min(240.0, float(self.bpm)))
        phase = (float(now) % period) / period
        beat = max(0.0, 1.0 - phase * 5.0)
        return max(beat * 0.45, self.gate_flash() * 0.9)

    # --- settings ---------------------------------------------------------

    def snapshot(self) -> Dict[str, Any]:
        return {
            "program": self.program_id,
            "scale": self.scale_id,
            "key": int(self.key),
            "octaves": int(self.octaves),
            "root_midi": int(self.root_midi),
            "gate": self.gate_id,
            "bpm": float(self.bpm),
            "hold": bool(self.hold),
            "show_all": bool(self.show_all),
            "show_axis_labels": bool(self.show_axis_labels),
            "show_grid_lines": bool(self.show_grid_lines),
            "viz_style": self.viz_style,
            "grid_width": int(self.grid_width),
            "out_mode": self.out_mode,
            "channel": int(self.channel),
            "cc_x": int(self.cc_x),
            "cc_y": int(self.cc_y),
            "cc_touch": int(self.cc_touch),
        }

    def apply(self, data: Dict[str, Any]) -> None:
        if not isinstance(data, dict):
            return
        prog = str(data.get("program", self.program_id))
        self.program_id = prog if prog in PROGRAM_BY_ID else "lead"
        scale = str(data.get("scale", self.scale_id))
        self.scale_id = scale if scale in SCALES else "ionian"
        if "show_all" in data:
            self.show_all = bool(data.get("show_all"))
        if "show_axis_labels" in data:
            self.show_axis_labels = bool(data.get("show_axis_labels"))
        if "show_grid_lines" in data:
            self.show_grid_lines = bool(data.get("show_grid_lines"))
        if "viz_style" in data:
            self.viz_style = normalize_viz_style(data.get("viz_style"))
        if "grid_width" in data:
            self.grid_width = clamp_grid_width(data.get("grid_width"))
        self.key = int(data.get("key", self.key)) % 12
        self.octaves = max(1, min(4, int(data.get("octaves", self.octaves))))
        self.root_midi = max(0, min(96, int(data.get("root_midi", self.root_midi))))
        gate = str(data.get("gate", self.gate_id))
        self.gate_id = gate if gate in GATE_BY_ID else "off"
        try:
            self.bpm = max(40.0, min(240.0, float(data.get("bpm", self.bpm))))
        except (TypeError, ValueError):
            pass
        self.hold = bool(data.get("hold", self.hold))
        out = str(data.get("out_mode", self.out_mode))
        self.out_mode = out if out in KAOSS_OUT_MODES else "local"
        self.channel = max(0, min(15, int(data.get("channel", self.channel))))
        self.cc_x = max(0, min(127, int(data.get("cc_x", self.cc_x))))
        self.cc_y = max(0, min(127, int(data.get("cc_y", self.cc_y))))
        self.cc_touch = max(0, min(127, int(data.get("cc_touch", self.cc_touch))))

    def set_program(self, program_id: str) -> KaossProgram:
        pid = str(program_id)
        if pid in PROGRAM_BY_ID:
            self.program_id = pid
        return self.program()

    def cycle_program(self, step: int = 1) -> KaossProgram:
        ids = self.program_ids()
        current = self.program().id
        try:
            idx = ids.index(current)
        except ValueError:
            idx = 0
        self.program_id = ids[(idx + int(step)) % len(ids)]
        return self.program()

    def cycle_scale(self, step: int = 1) -> str:
        ids = self.scale_ids()
        try:
            idx = ids.index(self.scale_id)
        except ValueError:
            idx = 0
        self.scale_id = ids[(idx + int(step)) % len(ids)]
        return self.scale_id

    def set_scale(self, scale_id: str) -> str:
        sid = str(scale_id)
        if sid in SCALES:
            self.scale_id = sid
        return self.scale_id

    def set_show_all(self, enabled: bool) -> bool:
        self.show_all = bool(enabled)
        return self.show_all

    def toggle_show_all(self) -> bool:
        return self.set_show_all(not self.show_all)

    def set_show_axis_labels(self, enabled: bool) -> bool:
        self.show_axis_labels = bool(enabled)
        return self.show_axis_labels

    def toggle_show_axis_labels(self) -> bool:
        return self.set_show_axis_labels(not self.show_axis_labels)

    def set_show_grid_lines(self, enabled: bool) -> bool:
        self.show_grid_lines = bool(enabled)
        return self.show_grid_lines

    def toggle_show_grid_lines(self) -> bool:
        return self.set_show_grid_lines(not self.show_grid_lines)

    def set_viz_style(self, style: str) -> str:
        self.viz_style = normalize_viz_style(style)
        return self.viz_style

    def cycle_viz_style(self) -> str:
        idx = VIZ_STYLES.index(self.viz_style) if self.viz_style in VIZ_STYLES else 0
        self.viz_style = VIZ_STYLES[(idx + 1) % len(VIZ_STYLES)]
        return self.viz_style

    def set_grid_width(self, width: int) -> int:
        self.grid_width = clamp_grid_width(width)
        return self.grid_width

    def nudge_grid_width(self, delta: int) -> int:
        return self.set_grid_width(int(self.grid_width) + int(delta))

    def led_grid_size(self) -> Tuple[int, int]:
        """CELLS visualizer — fixed 12×7 field, not the play/touch grid."""
        return LED_COLS, LED_ROWS

    def set_key(self, key: int) -> int:
        self.key = int(key) % 12
        return self.key

    def cycle_key(self, step: int = 1) -> int:
        self.key = (self.key + int(step)) % 12
        return self.key

    def set_octaves(self, octaves: int) -> int:
        self.octaves = max(1, min(4, int(octaves)))
        return self.octaves

    def cycle_octaves(self) -> int:
        self.octaves = 1 if self.octaves >= 4 else self.octaves + 1
        return self.octaves

    def set_root_midi(self, note: int) -> int:
        self.root_midi = max(24, min(72, int(note)))
        return self.root_midi

    def nudge_root_octave(self, step: int) -> int:
        """Shift the pad window up or down by whole octaves."""
        return self.set_root_midi(int(self.root_midi) + int(step) * 12)

    def root_octave_midi(self) -> int:
        """Nearest C-start on the OCT picker (C1..C5)."""
        c = (int(self.root_midi) // 12) * 12
        if c < ROOT_OCTAVE_MIDI[0]:
            return ROOT_OCTAVE_MIDI[0]
        if c > ROOT_OCTAVE_MIDI[-1]:
            return ROOT_OCTAVE_MIDI[-1]
        return c

    def set_gate(self, gate_id: str, *, now: Optional[float] = None) -> GatePattern:
        gid = str(gate_id)
        if gid in GATE_BY_ID:
            self.gate_id = gid
        # HOLD drone → GATE: start the clock so repeats begin immediately.
        if self.gate().beats > 0.0 and self.is_active():
            self._gate_t0 = 0.0 if now is None else float(now)
            self._gate_on = self._note is not None
        return self.gate()

    def cycle_gate(self, step: int = 1) -> GatePattern:
        idx = GATE_IDS.index(self.gate().id)
        self.gate_id = GATE_IDS[(idx + int(step)) % len(GATE_IDS)]
        return self.gate()

    def set_out_mode(self, mode: str) -> str:
        if mode in KAOSS_OUT_MODES:
            self.out_mode = mode
        return self.out_mode

    def cycle_out_mode(self) -> str:
        idx = KAOSS_OUT_MODES.index(self.out_mode)
        self.out_mode = KAOSS_OUT_MODES[(idx + 1) % len(KAOSS_OUT_MODES)]
        return self.out_mode

    def set_channel(self, channel: int) -> int:
        self.channel = max(0, min(15, int(channel)))
        return self.channel

    def cycle_channel(self) -> int:
        self.channel = (self.channel + 1) % 16
        return self.channel

    def nudge_bpm(self, delta: float) -> float:
        self.bpm = max(40.0, min(240.0, float(self.bpm) + float(delta)))
        return self.bpm

    # --- pad gestures -----------------------------------------------------

    def touch(self, x: float, y: float, *, now: float = 0.0) -> List[KaossEvent]:
        """Finger down (or first sample). Restarts gate phase."""
        self.x = clamp01(x)
        self.y = clamp01(y)
        self.touching = True
        self._gate_t0 = float(now)
        self._gate_on = False
        events: List[KaossEvent] = []
        events.extend(self._emit_touch(127))
        events.extend(self._emit_xy())
        if self.program().kind == "note":
            self._latched_note = self._current_note()
            self._latched_velocity = self._current_velocity()
            if self.gate().beats <= 0.0:
                events.extend(self._ensure_note(self._current_note(), self._current_velocity()))
            # Gated: first tick() will fire the attack
        return events

    def move(self, x: float, y: float) -> List[KaossEvent]:
        if not self.touching and not self.hold:
            return []
        self.x = clamp01(x)
        self.y = clamp01(y)
        events = self._emit_xy()
        if self.program().kind == "note" and self.gate().beats <= 0.0:
            if self.touching or self.hold:
                events.extend(self._ensure_note(self._current_note(), self._current_velocity()))
        return events

    def release(self, *, force: bool = False) -> List[KaossEvent]:
        """Finger up. HOLD keeps the last XY sounding unless ``force``."""
        self.touching = False
        if self.hold and not force:
            return []
        return self._silence()

    def set_hold(self, enabled: bool) -> List[KaossEvent]:
        was = self.hold
        self.hold = bool(enabled)
        if was and not self.hold and not self.touching:
            return self._silence()
        return []

    def toggle_hold(self) -> Tuple[bool, List[KaossEvent]]:
        events = self.set_hold(not self.hold)
        return self.hold, events

    def reassert(self, *, now: float = 0.0) -> List[KaossEvent]:
        """Replay this program's XY mapping without dropping a HOLD latch."""
        if not self.is_active():
            return []
        self._cc_x_sent = None
        self._cc_y_sent = None
        events: List[KaossEvent] = []
        events.extend(self._emit_touch(127))
        events.extend(self._emit_xy())
        if self.program().kind == "note" and self.gate().beats <= 0.0:
            events.extend(
                self._ensure_note(self._current_note(), self._current_velocity())
            )
        return events

    def panic(self) -> List[KaossEvent]:
        self.hold = False
        self.touching = False
        return self._silence()

    def retune(self) -> List[KaossEvent]:
        """Re-quantize the current note after SCALE / KEY / RANGE changes."""
        if self.program().kind != "note":
            return []
        if not self.is_active():
            return []
        if self.gate().beats > 0.0 and not self._gate_on:
            return []
        return self._ensure_note(self._current_note(), self._current_velocity())

    def tick(self, now: float) -> List[KaossEvent]:
        """Advance the gate arpeggiator. Call ~60 Hz while ``is_active()``."""
        gate = self.gate()
        if gate.beats <= 0.0:
            return []
        if not self.is_active():
            return []
        pitch = self._gate_pitch()
        if pitch is None:
            return []
        period = self._period_sec(gate)
        if period <= 0.0:
            return []
        elapsed = max(0.0, float(now) - self._gate_t0)
        phase = (elapsed % period) / period
        want_on = phase < gate.duty
        vel = self._gate_velocity()
        events: List[KaossEvent] = []
        if want_on and not self._gate_on:
            events.extend(self._ensure_note(pitch, vel, retrigger=True))
            self._gate_on = True
        elif not want_on and self._gate_on:
            events.extend(self._note_off())
            self._gate_on = False
        elif want_on and self._note is not None:
            # Slide to a new scale degree mid-gate without waiting for the next step
            if pitch != self._note:
                events.extend(self._ensure_note(pitch, vel))
        return events

    # --- internals --------------------------------------------------------

    def _current_note(self) -> int:
        note = note_at_x(self.x, self.notes())
        if self.program().y_param == "octave":
            note = max(0, min(127, note + int(round(self.y * 24.0))))
        return note

    def _current_velocity(self) -> int:
        return velocity_at_y(self.y)

    def _gate_pitch(self) -> Optional[int]:
        """Pitch the GATE arp retriggers. Frozen on FX programs so FILTER X isn't a keyboard."""
        if self.program().kind == "note":
            pitch = self._current_note()
            self._latched_note = pitch
            return pitch
        return self._latched_note

    def _gate_velocity(self) -> int:
        if self.program().kind == "note":
            vel = self._current_velocity()
            self._latched_velocity = vel
            return vel
        return max(1, min(127, int(self._latched_velocity)))

    def _period_sec(self, gate: GatePattern) -> float:
        bpm = max(40.0, min(240.0, float(self.bpm)))
        return (60.0 / bpm) * float(gate.beats)

    def _emit_touch(self, value: int) -> List[KaossEvent]:
        value = 127 if value else 0
        if self._touch_sent == value:
            return []
        self._touch_sent = value
        return [KaossEvent(kind="touch", control=self.cc_touch, value=value)]

    def _emit_xy(self) -> List[KaossEvent]:
        events: List[KaossEvent] = []
        xv = midi_cc(self.x)
        yv = midi_cc(self.y)
        if xv != self._cc_x_sent:
            self._cc_x_sent = xv
            events.append(KaossEvent(kind="cc", control=self.cc_x, value=xv))
        if yv != self._cc_y_sent:
            self._cc_y_sent = yv
            events.append(KaossEvent(kind="cc", control=self.cc_y, value=yv))
        prog = self.program()
        if prog.kind == "fx" and prog.x_param:
            events.append(
                KaossEvent(kind="param", param=prog.x_param, param_value=self.x)
            )
        if prog.y_param and prog.y_param != "octave":
            events.append(
                KaossEvent(kind="param", param=prog.y_param, param_value=self.y)
            )
        return events

    def _ensure_note(
        self, note: int, velocity: int, *, retrigger: bool = False
    ) -> List[KaossEvent]:
        note = max(0, min(127, int(note)))
        velocity = max(1, min(127, int(velocity)))
        events: List[KaossEvent] = []
        if self._note is not None and (self._note != note or retrigger):
            events.append(KaossEvent(kind="note_off", note=self._note, velocity=0))
            self._note = None
        if self._note != note:
            events.append(KaossEvent(kind="note_on", note=note, velocity=velocity))
            self._note = note
            self._latched_note = note
            self._latched_velocity = velocity
        return events

    def _note_off(self) -> List[KaossEvent]:
        if self._note is None:
            return []
        note = self._note
        self._note = None
        return [KaossEvent(kind="note_off", note=note, velocity=0)]

    def _silence(self) -> List[KaossEvent]:
        events: List[KaossEvent] = []
        events.extend(self._note_off())
        events.extend(self._emit_touch(0))
        self._gate_on = False
        self._latched_note = None
        self._cc_x_sent = None
        self._cc_y_sent = None
        return events

    def header_line(
        self,
        *,
        morph: Optional[Tuple[str, str, float]] = None,
        tone: Optional[float] = None,
    ) -> str:
        """Short enough for the 800px KAOSS title row."""
        prog = self.program()
        if prog.id == "morph" and morph is not None:
            _a, _b, frac = morph
            pct = int(round(clamp01(frac) * 100.0))
            note = note_name(self._note) if self._note is not None else "—"
            return f"{prog.label}  {pct}%  {note}"
        if prog.id == "lead" and tone is not None:
            pct = int(round(clamp01(tone) * 100.0))
            note = note_name(self._note) if self._note is not None else "—"
            return f"{prog.label}  {pct}%  {note}"
        if prog.kind == "note":
            note = note_name(self._note) if self._note is not None else "—"
            return f"{prog.label}  {note}"
        return f"{prog.label}  X={prog.x_axis}  Y={prog.y_axis}"

    def status_line(self) -> str:
        return self.header_line()
