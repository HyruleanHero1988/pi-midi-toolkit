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


# Scale degrees from the root. Names match the KO-1 / Kaossilator PRO lists
# we actually use (a useful subset — not all 31/35 factory scales).
SCALES: Dict[str, Tuple[int, ...]] = {
    "chromatic": (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11),
    "ionian": (0, 2, 4, 5, 7, 9, 11),          # major — KO-1 default
    "aeolian": (0, 2, 3, 5, 7, 8, 10),         # natural minor
    "dorian": (0, 2, 3, 5, 7, 9, 10),
    "mixolydian": (0, 2, 4, 5, 7, 9, 10),
    "harmonic": (0, 2, 3, 5, 7, 8, 11),
    "major_pent": (0, 2, 4, 7, 9),
    "minor_pent": (0, 3, 5, 7, 10),
    "blues": (0, 3, 5, 6, 7, 10),
    "whole": (0, 2, 4, 6, 8, 10),
    "ryukyu": (0, 4, 5, 7, 11),                # Okinawan — Kaossilator special
    "spanish": (0, 1, 4, 5, 7, 8, 10),
}

SCALE_ORDER: Tuple[str, ...] = (
    "ionian",
    "aeolian",
    "dorian",
    "mixolydian",
    "harmonic",
    "major_pent",
    "minor_pent",
    "blues",
    "whole",
    "ryukyu",
    "spanish",
    "chromatic",
)

SCALE_LABELS: Dict[str, str] = {
    "chromatic": "CHROM",
    "ionian": "MAJOR",
    "aeolian": "MINOR",
    "dorian": "DORIAN",
    "mixolydian": "MIXO",
    "harmonic": "H.MIN",
    "major_pent": "PENT+",
    "minor_pent": "PENT−",
    "blues": "BLUES",
    "whole": "WHOLE",
    "ryukyu": "RYUKYU",
    "spanish": "SPANISH",
}


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


# Keep the list short and obvious — not a 100-program dump.
PROGRAMS: Tuple[KaossProgram, ...] = (
    KaossProgram("lead", "LEAD", "note", y_param="tone", x_axis="PITCH", y_axis="TONE"),
    KaossProgram("morph", "MORPH", "note", y_param="morph", x_axis="PITCH", y_axis="MORPH"),
    KaossProgram("vib", "VIB", "note", y_param="vib", x_axis="PITCH", y_axis="VIB"),
    KaossProgram("filter", "FILTER", "fx", x_param="tone", y_param="morph", x_axis="TONE", y_axis="MORPH"),
    KaossProgram("echo", "ECHO", "fx", x_param="delay_time", y_param="delay_mix", x_axis="DLY T", y_axis="DLY MIX"),
    KaossProgram("drive", "DRIVE", "fx", x_param="drive", y_param="reverb_mix", x_axis="DRIVE", y_axis="REVERB"),
    KaossProgram("space", "SPACE", "fx", x_param="delay_mix", y_param="reverb_mix", x_axis="ECHO", y_axis="REVERB"),
)
PROGRAM_IDS: Tuple[str, ...] = tuple(p.id for p in PROGRAMS)
PROGRAM_BY_ID: Dict[str, KaossProgram] = {p.id: p for p in PROGRAMS}


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


def note_at_x(x: float, notes: Sequence[int]) -> int:
    if not notes:
        return 60
    if len(notes) == 1:
        return int(notes[0])
    idx = int(round(clamp01(x) * (len(notes) - 1)))
    return int(notes[idx])


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
        self.out_mode: str = "local"
        self.channel: int = 0
        self.cc_x: int = KAOSS_CC_X
        self.cc_y: int = KAOSS_CC_Y
        self.cc_touch: int = KAOSS_CC_TOUCH
        self.x: float = 0.5
        self.y: float = 0.5
        self.touching: bool = False
        self._note: Optional[int] = None
        self._cc_x_sent: Optional[int] = None
        self._cc_y_sent: Optional[int] = None
        self._touch_sent: Optional[int] = None
        self._gate_on: bool = False
        self._gate_t0: float = 0.0
        self._gate_period: float = 0.0

    # --- lookups ----------------------------------------------------------

    def program(self) -> KaossProgram:
        return PROGRAM_BY_ID.get(self.program_id, PROGRAMS[0])

    def gate(self) -> GatePattern:
        return GATE_BY_ID.get(self.gate_id, GATE_PATTERNS[0])

    def notes(self) -> List[int]:
        return scale_notes(
            self.scale_id, self.key, root_midi=self.root_midi, octaves=self.octaves
        )

    def is_active(self) -> bool:
        return self.touching or (self.hold and self._is_latched())

    def _is_latched(self) -> bool:
        if self.program().kind == "note":
            return self._note is not None
        return self._touch_sent == 127

    def sounding_note(self) -> Optional[int]:
        return self._note

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

    def cycle_program(self, step: int = 1) -> KaossProgram:
        idx = PROGRAM_IDS.index(self.program().id)
        self.program_id = PROGRAM_IDS[(idx + int(step)) % len(PROGRAM_IDS)]
        return self.program()

    def cycle_scale(self, step: int = 1) -> str:
        try:
            idx = SCALE_ORDER.index(self.scale_id)
        except ValueError:
            idx = 0
        self.scale_id = SCALE_ORDER[(idx + int(step)) % len(SCALE_ORDER)]
        return self.scale_id

    def cycle_key(self, step: int = 1) -> int:
        self.key = (self.key + int(step)) % 12
        return self.key

    def cycle_octaves(self) -> int:
        self.octaves = 1 if self.octaves >= 4 else self.octaves + 1
        return self.octaves

    def cycle_gate(self, step: int = 1) -> GatePattern:
        idx = GATE_IDS.index(self.gate().id)
        self.gate_id = GATE_IDS[(idx + int(step)) % len(GATE_IDS)]
        return self.gate()

    def cycle_out_mode(self) -> str:
        idx = KAOSS_OUT_MODES.index(self.out_mode)
        self.out_mode = KAOSS_OUT_MODES[(idx + 1) % len(KAOSS_OUT_MODES)]
        return self.out_mode

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

    def panic(self) -> List[KaossEvent]:
        self.hold = False
        self.touching = False
        return self._silence()

    def retune(self) -> List[KaossEvent]:
        """Re-quantize the current note after SCALE / KEY / RANGE changes."""
        if self.program().kind != "note":
            return []
        if not (self.touching or (self.hold and self._note is not None)):
            return []
        if self.gate().beats > 0.0 and not self._gate_on:
            return []
        return self._ensure_note(self._current_note(), self._current_velocity())

    def tick(self, now: float) -> List[KaossEvent]:
        """Advance the gate arpeggiator. Call ~60 Hz while ``is_active()``."""
        if self.program().kind != "note":
            return []
        gate = self.gate()
        if gate.beats <= 0.0:
            return []
        if not self.touching and not (self.hold and self._note is not None):
            return []
        period = self._period_sec(gate)
        if period <= 0.0:
            return []
        elapsed = max(0.0, float(now) - self._gate_t0)
        phase = (elapsed % period) / period
        want_on = phase < gate.duty
        events: List[KaossEvent] = []
        if want_on and not self._gate_on:
            events.extend(self._ensure_note(self._current_note(), self._current_velocity(), retrigger=True))
            self._gate_on = True
        elif not want_on and self._gate_on:
            events.extend(self._note_off())
            self._gate_on = False
        elif want_on and self._note is not None:
            # Slide to a new scale degree mid-gate without waiting for the next step
            nxt = self._current_note()
            if nxt != self._note:
                events.extend(self._ensure_note(nxt, self._current_velocity()))
        return events

    # --- internals --------------------------------------------------------

    def _current_note(self) -> int:
        return note_at_x(self.x, self.notes())

    def _current_velocity(self) -> int:
        return velocity_at_y(self.y)

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
        if prog.y_param:
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
        self._cc_x_sent = None
        self._cc_y_sent = None
        return events

    def status_line(self) -> str:
        prog = self.program()
        key = NOTE_NAMES[self.key % 12]
        scale = SCALE_LABELS.get(self.scale_id, self.scale_id.upper())
        hold = "HOLD" if self.hold else "lift=off"
        gate = self.gate().label
        out = self.out_mode.upper()
        ch = self.channel + 1
        note = note_name(self._note) if self._note is not None else "—"
        if prog.kind == "note":
            return (
                f"{prog.label}  {key} {scale}  {self.octaves}oct  "
                f"{gate}  {int(round(self.bpm))} BPM  {hold}  "
                f"OUT {out}  CH{ch}  {note}"
            )
        return (
            f"{prog.label}  X={prog.x_axis} Y={prog.y_axis}  "
            f"{hold}  OUT {out}  CH{ch}  "
            f"CC{self.cc_x}/{self.cc_y}"
        )
