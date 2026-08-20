"""Phrase pad bank (clip-launch grid) — headless."""
from __future__ import annotations

import json
import pathlib
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    DRUM_CHANNEL,
    MAX_PHRASE_PLAYERS,
    PHRASE_GRID_CELLS,
    PHRASE_PAD_BASE,
    PHRASE_PAD_COUNT,
    PHRASES_DIR,
)
from pidi.sequencer import LoopEvent, trim_loop_take


def phrase_pad_label(cell: int) -> str:
    """Human label for phrase cell 0..15 → A1..A8 / B1..B8."""
    c = max(0, min(PHRASE_PAD_COUNT - 1, int(cell)))
    bank = "A" if c < 8 else "B"
    return f"{bank}{(c % 8) + 1}"


# Clip-launcher fills. Green is *only* "this pad is sounding".
PHRASE_TILE_PLAYING = "#689d6a"
PHRASE_TILE_IDLE = "#458588"
PHRASE_TILE_SELECTED = "#076678"
PHRASE_TILE_EMPTY = "#3c3836"
PHRASE_TILE_EMPTY_LOOP = "#504945"
PHRASE_TILE_REC = "#9d0006"
PHRASE_TILE_MODE = "#b16286"
PHRASE_TILE_ASSIGN = "#458588"
PHRASE_TILE_CLEAR = "#cc241d"
PHRASE_TILE_CLEAR_EMPTY = "#504945"


def phrase_pad_tile_color(
    *,
    empty: bool,
    loop: bool = False,
    playing: bool = False,
    selected: bool = False,
    recording: bool = False,
    mode_armed: bool = False,
    assign_armed: bool = False,
    clear_armed: bool = False,
    edit_view: bool = False,
) -> str:
    """Pad square fill. Playing is always green; idle clips are blue whether LOOP or 1SHOT."""
    if mode_armed:
        return PHRASE_TILE_MODE
    if assign_armed:
        return PHRASE_TILE_ASSIGN
    if clear_armed:
        return PHRASE_TILE_CLEAR if not empty else PHRASE_TILE_CLEAR_EMPTY
    if recording:
        return PHRASE_TILE_REC
    if playing:
        return PHRASE_TILE_PLAYING
    if empty:
        if selected and edit_view:
            return PHRASE_TILE_SELECTED
        return PHRASE_TILE_EMPTY_LOOP if loop else PHRASE_TILE_EMPTY
    if selected:
        return PHRASE_TILE_SELECTED
    return PHRASE_TILE_IDLE


def phrase_cell_for_note(note: int) -> Optional[int]:
    """Map factory MPK pad note (36–51) → phrase cell index, else None."""
    n = note & 0x7F
    if PHRASE_PAD_BASE <= n < PHRASE_PAD_BASE + PHRASE_PAD_COUNT:
        return n - PHRASE_PAD_BASE
    return None


PHRASE_TRIG_ONESHOT = "oneshot"
PHRASE_TRIG_LOOP = "loop"
PHRASE_TRIG_MODES = (PHRASE_TRIG_ONESHOT, PHRASE_TRIG_LOOP)
PHRASE_VOICE_FOLLOW = "follow"
PHRASE_VOICE_LOCKED = "locked"
PHRASE_VOICE_MODES = (PHRASE_VOICE_FOLLOW, PHRASE_VOICE_LOCKED)
# out_channel -1 = use each event's recorded channel; 0..15 = force MIDI ch 1..16
PHRASE_OUT_AS_RECORDED = -1

PHRASE_GAIN_MIN = 0.10
PHRASE_GAIN_MAX = 2.00
PHRASE_GAIN_STEP = 0.10


def clamp_phrase_gain(value: float) -> float:
    try:
        gain = float(value)
    except (TypeError, ValueError):
        return 1.0
    if gain != gain:  # NaN
        return 1.0
    return max(PHRASE_GAIN_MIN, min(PHRASE_GAIN_MAX, gain))


def scale_velocity(velocity: int, gain: float) -> int:
    """Apply a pad's trim to one note. Never silences a hit that was recorded."""
    scaled = int(round(max(0, int(velocity)) * clamp_phrase_gain(gain)))
    return max(1, min(127, scaled))


@dataclass
class PhraseCell:
    events: List[LoopEvent]
    length: float = 0.0
    trigger_mode: str = PHRASE_TRIG_ONESHOT  # oneshot | loop
    voice_mode: str = PHRASE_VOICE_FOLLOW  # follow global morph | locked snapshot
    morph_a: str = ""
    morph_b: str = ""
    morph: float = 0.0
    out_channel: int = PHRASE_OUT_AS_RECORDED  # -1 or 0..15
    local_synth: bool = True  # False = MIDI-only (no soft-synth for this pad)
    # Per-pad trim so a locked voice can sit under (or over) the rest of the mix.
    # 1.0 = as recorded; LOCK bakes the master level here, VOL −/+ tunes it.
    gain: float = 1.0
    # Vibrato as it sounded while recording. False = follow the live rig.
    vib_baked: bool = False
    vib_depth: float = 0.0  # semitones
    vib_rate: float = 5.0  # Hz
    vib_amount: float = 0.0  # 0..1 (wheel or screen, whichever was asking)

    def vib_tuple(self) -> Optional[Tuple[float, float, float]]:
        """What to hand the engine for this pad's key notes."""
        if not self.vib_baked:
            return None
        return (float(self.vib_depth), float(self.vib_rate), float(self.vib_amount))

    def vib_label(self) -> str:
        if not self.vib_baked:
            return "live"
        if self.vib_amount <= 0.01 or self.vib_depth <= 0.001:
            return "none"
        return f"{self.vib_depth:.1f}st"

    def is_empty(self) -> bool:
        return not self.events or self.length <= 0.0

    def is_loop(self) -> bool:
        return self.trigger_mode == PHRASE_TRIG_LOOP

    def is_voice_locked(self) -> bool:
        return self.voice_mode == PHRASE_VOICE_LOCKED

    def to_dict(self) -> Dict[str, Any]:
        mode = self.trigger_mode if self.trigger_mode in PHRASE_TRIG_MODES else PHRASE_TRIG_ONESHOT
        vmode = self.voice_mode if self.voice_mode in PHRASE_VOICE_MODES else PHRASE_VOICE_FOLLOW
        och = int(self.out_channel)
        if och < -1 or och > 15:
            och = PHRASE_OUT_AS_RECORDED
        return {
            "version": 4,
            "length": float(self.length),
            "trigger_mode": mode,
            "voice_mode": vmode,
            "morph_a": str(self.morph_a or ""),
            "morph_b": str(self.morph_b or ""),
            "morph": float(self.morph),
            "out_channel": och,
            "local_synth": bool(self.local_synth),
            "gain": float(clamp_phrase_gain(self.gain)),
            "vib_baked": bool(self.vib_baked),
            "vib_depth": float(self.vib_depth),
            "vib_rate": float(self.vib_rate),
            "vib_amount": float(self.vib_amount),
            "events": [
                {
                    "t": float(e.t),
                    "on": bool(e.on),
                    "channel": int(e.channel),
                    "note": int(e.note),
                    "velocity": int(e.velocity),
                }
                for e in self.events
            ],
        }

    @staticmethod
    def from_dict(data: Dict[str, Any]) -> "PhraseCell":
        events: List[LoopEvent] = []
        for raw in data.get("events") or []:
            if not isinstance(raw, dict):
                continue
            try:
                events.append(
                    LoopEvent(
                        t=float(raw.get("t", 0.0)),
                        on=bool(raw.get("on", True)),
                        channel=int(raw.get("channel", 0)) & 0x0F,
                        note=int(raw.get("note", 60)) & 0x7F,
                        velocity=max(0, min(127, int(raw.get("velocity", 100)))),
                    )
                )
            except (TypeError, ValueError):
                continue
        length = float(data.get("length", 0.0) or 0.0)
        if events and length <= 0.0:
            length = max(e.t for e in events) + 0.05
        mode = str(data.get("trigger_mode", PHRASE_TRIG_ONESHOT) or PHRASE_TRIG_ONESHOT)
        if mode not in PHRASE_TRIG_MODES:
            mode = PHRASE_TRIG_ONESHOT
        vmode = str(data.get("voice_mode", PHRASE_VOICE_FOLLOW) or PHRASE_VOICE_FOLLOW)
        if vmode not in PHRASE_VOICE_MODES:
            vmode = PHRASE_VOICE_FOLLOW
        try:
            och = int(data.get("out_channel", PHRASE_OUT_AS_RECORDED))
        except (TypeError, ValueError):
            och = PHRASE_OUT_AS_RECORDED
        if och < -1 or och > 15:
            och = PHRASE_OUT_AS_RECORDED
        try:
            morph = float(data.get("morph", 0.0) or 0.0)
        except (TypeError, ValueError):
            morph = 0.0
        try:
            gain = float(data.get("gain", 1.0))
        except (TypeError, ValueError):
            gain = 1.0

        def _num(key: str, default: float, lo: float, hi: float) -> float:
            try:
                return max(lo, min(hi, float(data.get(key, default))))
            except (TypeError, ValueError):
                return default

        return PhraseCell(
            events=events,
            length=length,
            trigger_mode=mode,
            voice_mode=vmode,
            morph_a=str(data.get("morph_a", "") or ""),
            morph_b=str(data.get("morph_b", "") or ""),
            morph=max(0.0, min(1.0, morph)),
            out_channel=och,
            local_synth=bool(data.get("local_synth", True)),
            gain=clamp_phrase_gain(gain),
            vib_baked=bool(data.get("vib_baked", False)),
            vib_depth=_num("vib_depth", 0.0, 0.0, 4.0),
            vib_rate=_num("vib_rate", 5.0, 0.1, 20.0),
            vib_amount=_num("vib_amount", 0.0, 0.0, 1.0),
        )


class PhrasePadBank:
    """16 clip-launch cells: record keyboard phrases, fire from touch or MPK pads."""

    def __init__(
        self,
        engine: "SineEngine",
        emit,  # callable matching event_q tuples
        directory: pathlib.Path = PHRASES_DIR,
    ) -> None:
        self._engine = engine
        self._emit = emit
        self._dir = directory
        self._lock = threading.Lock()
        self._cells: List[PhraseCell] = [PhraseCell(events=[]) for _ in range(PHRASE_PAD_COUNT)]
        self._recording_cell: Optional[int] = None
        self._rec_t0 = 0.0
        self._selected: Optional[int] = None
        self._playing: Dict[int, threading.Event] = {}
        self._threads: Dict[int, threading.Thread] = {}
        self._held: Dict[int, set[Tuple[int, int]]] = {}
        self._active_timbres: Dict[int, np.ndarray] = {}
        # Output hooks (wired by MidiToneApp — share Songs USB port)
        self._get_out_mode = lambda: "local"
        self._ensure_outport = lambda: None
        self._get_outport = lambda: None
        self.load_all()

    def set_output_hooks(self, *, get_out_mode, ensure_outport, get_outport) -> None:
        self._get_out_mode = get_out_mode
        self._ensure_outport = ensure_outport
        self._get_outport = get_outport

    def selected(self) -> Optional[int]:
        with self._lock:
            return self._selected

    def recording_cell(self) -> Optional[int]:
        with self._lock:
            return self._recording_cell

    def is_recording(self) -> bool:
        with self._lock:
            return self._recording_cell is not None

    def is_playing(self, cell: int) -> bool:
        with self._lock:
            return cell in self._playing

    def playing_cells(self) -> List[int]:
        with self._lock:
            return sorted(self._playing.keys())

    def _copy_cell(self, c: PhraseCell) -> PhraseCell:
        return PhraseCell(
            events=list(c.events),
            length=float(c.length),
            trigger_mode=c.trigger_mode,
            voice_mode=c.voice_mode,
            morph_a=c.morph_a,
            morph_b=c.morph_b,
            morph=float(c.morph),
            out_channel=int(c.out_channel),
            local_synth=bool(c.local_synth),
            gain=float(c.gain),
            vib_baked=bool(c.vib_baked),
            vib_depth=float(c.vib_depth),
            vib_rate=float(c.vib_rate),
            vib_amount=float(c.vib_amount),
        )

    def cell(self, idx: int) -> PhraseCell:
        with self._lock:
            return self._copy_cell(self._cells[idx])

    def trigger_mode(self, idx: int) -> str:
        with self._lock:
            if not (0 <= idx < PHRASE_PAD_COUNT):
                return PHRASE_TRIG_ONESHOT
            return self._cells[idx].trigger_mode

    def set_trigger_mode(self, idx: int, mode: str) -> bool:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        if mode not in PHRASE_TRIG_MODES:
            return False
        with self._lock:
            self._cells[idx].trigger_mode = mode
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(
            (
                "log",
                f"Phrase {phrase_pad_label(idx)} → "
                f"{'LOOP' if mode == PHRASE_TRIG_LOOP else 'ONE-SHOT'}",
                False,
            )
        )
        return True

    def toggle_trigger_mode(self, idx: int) -> Optional[str]:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return None
        with self._lock:
            cur = self._cells[idx].trigger_mode
        nxt = PHRASE_TRIG_LOOP if cur != PHRASE_TRIG_LOOP else PHRASE_TRIG_ONESHOT
        self.set_trigger_mode(idx, nxt)
        return nxt

    def set_voice_follow(self, idx: int) -> bool:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        with self._lock:
            c = self._cells[idx]
            c.voice_mode = PHRASE_VOICE_FOLLOW
            # Back to following the live rig: master level owns this pad again.
            c.gain = 1.0
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} voice FOLLOW", False))
        return True

    def lock_voice_from_engine(self, idx: int) -> bool:
        """Snapshot current global morph *and* level onto this pad (LOCKED)."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        a, b, morph = self._engine.snapshot_morph()
        try:
            level = float(self._engine.level())
        except Exception:
            level = 1.0
        try:
            vib_depth, vib_rate, vib_amount = self._engine.vib_state()
        except Exception:
            vib_depth, vib_rate, vib_amount = (0.0, 5.0, 0.0)
        gain = clamp_phrase_gain(level)
        with self._lock:
            c = self._cells[idx]
            c.voice_mode = PHRASE_VOICE_LOCKED
            c.morph_a = a
            c.morph_b = b
            c.morph = float(morph)
            c.gain = gain
            c.vib_baked = True
            c.vib_depth = float(vib_depth)
            c.vib_rate = float(vib_rate)
            c.vib_amount = float(vib_amount)
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(
            (
                "log",
                f"Phrase {phrase_pad_label(idx)} voice LOCKED "
                f"({a}→{b} {int(morph * 100)}%, vol {int(gain * 100)}%)",
                False,
            )
        )
        return True

    def bake_vib_from_engine(self, idx: int) -> bool:
        """Freeze the live vibrato onto this pad (what REC does automatically)."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        try:
            depth, rate, amount = self._engine.vib_state()
        except Exception:
            return False
        with self._lock:
            c = self._cells[idx]
            c.vib_baked = True
            c.vib_depth = float(depth)
            c.vib_rate = float(rate)
            c.vib_amount = float(amount)
            self._selected = idx
            label = c.vib_label()
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} vibrato baked ({label})", False))
        return True

    def set_vib_live(self, idx: int) -> bool:
        """Hand this pad's vibrato back to the live rig."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        with self._lock:
            self._cells[idx].vib_baked = False
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} vibrato live", False))
        return True

    def toggle_vib_baked(self, idx: int) -> Optional[bool]:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return None
        with self._lock:
            baked = self._cells[idx].vib_baked
        if baked:
            self.set_vib_live(idx)
            return False
        self.bake_vib_from_engine(idx)
        return True

    def gain(self, idx: int) -> float:
        with self._lock:
            if not (0 <= idx < PHRASE_PAD_COUNT):
                return 1.0
            return float(self._cells[idx].gain)

    def set_gain(self, idx: int, gain: float) -> Optional[float]:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return None
        value = clamp_phrase_gain(gain)
        with self._lock:
            if abs(self._cells[idx].gain - value) < 1e-6:
                return float(self._cells[idx].gain)
            self._cells[idx].gain = value
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} vol {int(value * 100)}%", False))
        return value

    def nudge_gain(self, idx: int, delta: float) -> Optional[float]:
        """VOL − / + on the selected pad — audible on the next note, mid-loop."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return None
        with self._lock:
            current = self._cells[idx].gain
        return self.set_gain(idx, current + float(delta))

    def toggle_voice_lock(self, idx: int) -> Optional[str]:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return None
        with self._lock:
            locked = self._cells[idx].voice_mode == PHRASE_VOICE_LOCKED
        if locked:
            self.set_voice_follow(idx)
            return PHRASE_VOICE_FOLLOW
        self.lock_voice_from_engine(idx)
        return PHRASE_VOICE_LOCKED

    def set_out_channel(self, idx: int, channel: int) -> bool:
        """channel -1 = as recorded; 0..15 = force MIDI channel."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        ch = int(channel)
        if ch < -1 or ch > 15:
            return False
        with self._lock:
            self._cells[idx].out_channel = ch
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        label = "as-recorded" if ch < 0 else f"ch{ch + 1}"
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} out {label}", False))
        return True

    def cycle_out_channel(self, idx: int) -> int:
        """Cycle: as-recorded → ch1 → … → ch16 → as-recorded."""
        with self._lock:
            cur = self._cells[idx].out_channel if 0 <= idx < PHRASE_PAD_COUNT else -1
        nxt = -1 if cur >= 15 else cur + 1
        self.set_out_channel(idx, nxt)
        return nxt

    def set_local_synth(self, idx: int, enabled: bool) -> bool:
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        with self._lock:
            self._cells[idx].local_synth = bool(enabled)
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(
            (
                "log",
                f"Phrase {phrase_pad_label(idx)} local synth "
                f"{'ON' if enabled else 'OFF'}",
                False,
            )
        )
        return True

    def toggle_local_synth(self, idx: int) -> bool:
        with self._lock:
            cur = self._cells[idx].local_synth if 0 <= idx < PHRASE_PAD_COUNT else True
        self.set_local_synth(idx, not cur)
        return not cur

    def status_line(
        self,
        *,
        clear_armed: bool = False,
        mode_armed: bool = False,
        assign_armed: bool = False,
        view: str = "edit",
    ) -> str:
        with self._lock:
            filled = sum(1 for c in self._cells if not c.is_empty())
            rec = self._recording_cell
            playing = sorted(self._playing.keys())
            sel = self._selected
        if assign_armed:
            return (
                "SEQ → PAD armed — tap a pad (touch or MPK) to drop the sequence there · "
                "→ PAD again to cancel"
            )
        if mode_armed:
            return "MODE armed — tap a pad to toggle ONE-SHOT ↔ LOOP · MODE again to cancel"
        if clear_armed:
            return "CLEAR armed — tap a pad (touch or MPK) to erase it · CLEAR again to cancel"
        if rec is not None:
            return (
                f"Recording {phrase_pad_label(rec)} — keys + drum pads record; "
                f"STOP REC or tap that square to finish ({filled}/16 filled)"
            )
        if view == "play":
            if playing:
                names = ",".join(phrase_pad_label(i) for i in playing[:6])
                return f"PLAY · {names} · tap pad to launch/stop · {filled}/16"
            return f"PLAY · tap a pad to launch · {filled}/16 filled"
        if sel is not None:
            c = self.cell(sel)
            v = "LOCK" if c.is_voice_locked() else "FOLLOW"
            och = "rec" if c.out_channel < 0 else f"ch{c.out_channel + 1}"
            syn = "SYN" if c.local_synth else "MIDI"
            trig = "LOOP" if c.is_loop() else "1SHOT"
            return (
                f"EDIT {phrase_pad_label(sel)} · {trig} · {v} · vol {int(c.gain * 100)}% · "
                f"vib {c.vib_label()} · {och} · {syn} · {filled}/16"
            )
        if playing:
            names = ",".join(phrase_pad_label(i) for i in playing[:6])
            more = f"+{len(playing) - 6}" if len(playing) > 6 else ""
            return f"EDIT · playing {names}{more} · select a pad to fine-tune"
        if filled == 0:
            return "EDIT · tap empty pad to record · fill pads then use PLAY to perform"
        return f"EDIT · {filled}/16 · tap pad to launch+select · use row below to fine-tune"

    def _cell_path(self, idx: int) -> pathlib.Path:
        return self._dir / f"pad-{idx + 1:02d}.json"

    def load_all(self) -> int:
        """Load pad-01.json … pad-16.json. Returns number of non-empty cells."""
        self._dir.mkdir(parents=True, exist_ok=True)
        loaded = 0
        with self._lock:
            for i in range(PHRASE_PAD_COUNT):
                path = self._cell_path(i)
                if not path.is_file():
                    self._cells[i] = PhraseCell(events=[])
                    continue
                try:
                    data = json.loads(path.read_text(encoding="utf-8"))
                    cell = PhraseCell.from_dict(data if isinstance(data, dict) else {})
                    self._cells[i] = cell
                    if not cell.is_empty():
                        loaded += 1
                except Exception as exc:
                    print(f"phrase load skip {path.name}: {exc}", flush=True)
                    self._cells[i] = PhraseCell(events=[])
        return loaded

    def save_cell(self, idx: int) -> bool:
        idx = int(idx)
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        self._dir.mkdir(parents=True, exist_ok=True)
        with self._lock:
            cell = self._copy_cell(self._cells[idx])
        path = self._cell_path(idx)
        try:
            # Drop file only for empty default pads
            defaultish = (
                cell.is_empty()
                and cell.trigger_mode == PHRASE_TRIG_ONESHOT
                and cell.voice_mode == PHRASE_VOICE_FOLLOW
                and cell.out_channel == PHRASE_OUT_AS_RECORDED
                and cell.local_synth
                and abs(cell.gain - 1.0) < 1e-6
                and not cell.vib_baked
            )
            if defaultish:
                if path.is_file():
                    path.unlink()
                return True
            tmp = path.with_suffix(path.suffix + ".tmp")
            tmp.write_text(
                json.dumps(cell.to_dict(), indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            tmp.replace(path)
            return True
        except Exception as exc:
            print(f"phrase save failed ({path}): {exc}", flush=True)
            return False

    def export_bank(self) -> Dict[str, Any]:
        """Full 16-pad snapshot for presets / session restore."""
        with self._lock:
            return {
                "selected": self._selected,
                "pads": [self._copy_cell(c).to_dict() for c in self._cells],
            }

    def import_bank(self, data: Dict[str, Any], *, persist: bool = True) -> int:
        """Replace all pads from export_bank(). Returns non-empty cell count."""
        if self.is_recording():
            self.stop_record()
        self.stop_all()
        pads_raw = data.get("pads") if isinstance(data, dict) else None
        loaded = 0
        with self._lock:
            self._selected = None
            if isinstance(pads_raw, list):
                for i in range(PHRASE_PAD_COUNT):
                    raw = pads_raw[i] if i < len(pads_raw) else None
                    if isinstance(raw, dict):
                        cell = PhraseCell.from_dict(raw)
                    else:
                        cell = PhraseCell(events=[])
                    self._cells[i] = cell
                    if not cell.is_empty():
                        loaded += 1
            else:
                for i in range(PHRASE_PAD_COUNT):
                    self._cells[i] = PhraseCell(events=[])
            sel = data.get("selected") if isinstance(data, dict) else None
            if sel is not None:
                try:
                    idx = int(sel)
                    if 0 <= idx < PHRASE_PAD_COUNT:
                        self._selected = idx
                except (TypeError, ValueError):
                    pass
        if persist:
            for i in range(PHRASE_PAD_COUNT):
                self.save_cell(i)
        self._emit(("phrase",))
        return loaded

    def select(self, idx: int) -> None:
        if 0 <= idx < PHRASE_PAD_COUNT:
            with self._lock:
                self._selected = idx
            self._emit(("phrase",))

    def arm_record(self, idx: int) -> bool:
        """Start recording into cell (clears prior contents). Stops that cell's playback."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        if self.is_recording():
            self.stop_record()
        self.stop_cell(idx)
        with self._lock:
            prev = self._cells[idx]
            self._cells[idx] = PhraseCell(
                events=[],
                trigger_mode=prev.trigger_mode,
                voice_mode=prev.voice_mode,
                morph_a=prev.morph_a,
                morph_b=prev.morph_b,
                morph=prev.morph,
                out_channel=prev.out_channel,
                local_synth=prev.local_synth,
                gain=prev.gain,
            )
            self._recording_cell = idx
            self._rec_t0 = time.monotonic()
            self._selected = idx
        self._emit(("phrase",))
        self._emit(("log", f"Phrase REC {phrase_pad_label(idx)} armed", False))
        return True

    def stop_record(self) -> Optional[int]:
        """Finish recording. Returns cell index, or None if not recording."""
        # Vibrato is part of how the take sounded, so it travels with the clip.
        try:
            vib_depth, vib_rate, vib_amount = self._engine.vib_state()
        except Exception:
            vib_depth, vib_rate, vib_amount = (0.0, 5.0, 0.0)
        with self._lock:
            idx = self._recording_cell
            if idx is None:
                return None
            cell = self._cells[idx]
            if cell.events:
                trimmed, length = trim_loop_take(list(cell.events))
                cell.events = trimmed
                cell.length = length
                cell.vib_baked = True
                cell.vib_depth = float(vib_depth)
                cell.vib_rate = float(vib_rate)
                cell.vib_amount = float(vib_amount)
            else:
                cell.length = 0.0
            self._recording_cell = None
        self.save_cell(idx)
        self._emit(("phrase",))
        with self._lock:
            empty = self._cells[idx].is_empty()
        if empty:
            self._emit(("log", f"Phrase {phrase_pad_label(idx)} empty (nothing recorded)", False))
        else:
            with self._lock:
                n = len(self._cells[idx].events)
                length = self._cells[idx].length
            self._emit(
                ("log", f"Phrase {phrase_pad_label(idx)} saved ({n} ev, {length:.2f}s)", False)
            )
        return idx

    def record_note(self, on: bool, channel: int, note: int, velocity: int) -> None:
        """Capture keyboard and drum-channel notes into the armed cell."""
        with self._lock:
            idx = self._recording_cell
            if idx is None:
                return
            t = time.monotonic() - self._rec_t0
            self._cells[idx].events.append(
                LoopEvent(
                    t=t,
                    on=on,
                    channel=channel & 0x0F,
                    note=note & 0x7F,
                    velocity=max(1, min(127, int(velocity))) if on else 0,
                )
            )
        self._emit(("phrase",))

    def clear_cell(self, idx: Optional[int] = None) -> bool:
        target = idx if idx is not None else self.selected()
        if target is None or not (0 <= target < PHRASE_PAD_COUNT):
            return False
        if self.recording_cell() == target:
            self.stop_record()
        self.stop_cell(target)
        with self._lock:
            self._cells[target] = PhraseCell(events=[])
            self._selected = target
        self.save_cell(target)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(target)} cleared", False))
        return True

    def load_from_events(
        self,
        idx: int,
        events: List[LoopEvent],
        length: float,
        *,
        trigger_mode: str = PHRASE_TRIG_LOOP,
    ) -> bool:
        """Replace a pad's contents with a free-timing take (e.g. from SEQ)."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        if self.is_recording():
            self.stop_record()
        self.stop_cell(idx)
        copied = [
            LoopEvent(
                t=float(e.t),
                on=bool(e.on),
                channel=int(e.channel) & 0x0F,
                note=int(e.note) & 0x7F,
                velocity=max(0, min(127, int(e.velocity))),
            )
            for e in events
        ]
        length = float(length)
        if copied and length <= 0.0:
            length = max(e.t for e in copied) + 0.05
        if not copied or length <= 0.0:
            return False
        mode = trigger_mode if trigger_mode in PHRASE_TRIG_MODES else PHRASE_TRIG_LOOP
        try:
            vib_depth, vib_rate, vib_amount = self._engine.vib_state()
        except Exception:
            vib_depth, vib_rate, vib_amount = (0.0, 5.0, 0.0)
        with self._lock:
            prev = self._cells[idx]
            self._cells[idx] = PhraseCell(
                events=copied,
                length=length,
                trigger_mode=mode,
                voice_mode=prev.voice_mode,
                morph_a=prev.morph_a,
                morph_b=prev.morph_b,
                morph=prev.morph,
                out_channel=prev.out_channel,
                local_synth=prev.local_synth,
                gain=prev.gain,
                vib_baked=True,
                vib_depth=float(vib_depth),
                vib_rate=float(vib_rate),
                vib_amount=float(vib_amount),
            )
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(
            (
                "log",
                f"Phrase {phrase_pad_label(idx)} ← seq "
                f"({len(copied)} ev, {length:.2f}s, "
                f"{'LOOP' if mode == PHRASE_TRIG_LOOP else 'ONE-SHOT'})",
                False,
            )
        )
        return True

    def stop_cell(self, idx: int) -> None:
        with self._lock:
            stop_ev = self._playing.get(idx)
            thread = self._threads.get(idx)
        if stop_ev is not None:
            stop_ev.set()
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            thread.join(timeout=1.0)
        with self._lock:
            self._playing.pop(idx, None)
            self._threads.pop(idx, None)
        self._release_held(idx)
        self._emit(("phrase",))

    def stop_all(self) -> None:
        if self.is_recording():
            self.stop_record()
        with self._lock:
            ids = list(self._playing.keys())
        for idx in ids:
            self.stop_cell(idx)

    def launch(self, idx: int) -> str:
        """
        Launch a filled cell.
        oneshot: play once (re-trigger restarts).
        loop: toggle — start looping, or stop if already playing.
        Returns action: launch | restart | stop | empty | stop_rec
        """
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return "ignore"
        if self.recording_cell() == idx:
            self.stop_record()
            return "stop_rec"
        with self._lock:
            cell = self._copy_cell(self._cells[idx])
            if cell.is_empty():
                return "empty"
            events = list(cell.events)
            length = float(cell.length)
            loop = cell.trigger_mode == PHRASE_TRIG_LOOP
            already = idx in self._playing
            self._selected = idx
            locked_playing = sum(
                1
                for i, c in enumerate(self._cells)
                if i in self._playing and c.voice_mode == PHRASE_VOICE_LOCKED
            )
            if idx not in self._playing and len(self._playing) >= MAX_PHRASE_PLAYERS:
                oldest = next(iter(self._playing))
            else:
                oldest = None
        # Loop pad while playing → toggle off
        if loop and already:
            self.stop_cell(idx)
            self._emit(("log", f"Phrase ■ {phrase_pad_label(idx)} (loop stop)", False))
            return "stop"
        if oldest is not None:
            self.stop_cell(oldest)

        # Bake locked timbre (cap concurrent locked tables)
        timbre: Optional[np.ndarray] = None
        if cell.is_voice_locked():
            if not already and locked_playing >= self._engine.MAX_LOCKED_TIMBRES:
                self._emit(
                    (
                        "log",
                        f"Phrase {phrase_pad_label(idx)} — too many LOCKED pads "
                        f"(max {self._engine.MAX_LOCKED_TIMBRES}); using FOLLOW",
                        False,
                    )
                )
            else:
                timbre = self._engine.bake_morph_table(
                    cell.morph_a or "sine",
                    cell.morph_b or cell.morph_a or "sine",
                    cell.morph,
                )

        out_mode = str(self._get_out_mode() or "local")
        want_usb = out_mode in ("usb", "both")
        if want_usb:
            self._ensure_outport()

        # Locked pads keep morph_a's FX insert; FOLLOW pads use live nearer endpoint.
        fx_name: Optional[str] = None
        if cell.is_voice_locked():
            fx_name = str(cell.morph_a or "sine")

        was_playing = already
        self.stop_cell(idx)
        stop_ev = threading.Event()
        with self._lock:
            self._playing[idx] = stop_ev
            self._held[idx] = set()
            if timbre is not None:
                self._active_timbres[idx] = timbre
            else:
                self._active_timbres.pop(idx, None)
        thread = threading.Thread(
            target=self._play_cell,
            args=(
                idx,
                events,
                length,
                stop_ev,
                loop,
                timbre,
                fx_name,
                cell.out_channel,
                cell.local_synth,
                out_mode,
            ),
            daemon=True,
        )
        with self._lock:
            self._threads[idx] = thread
        thread.start()
        self._emit(("phrase",))
        if loop:
            self._emit(("log", f"Phrase ↻ {phrase_pad_label(idx)} (loop)", False))
            return "loop"
        tag = "restart" if was_playing else "launch"
        self._emit(("log", f"Phrase ▶ {phrase_pad_label(idx)}", False))
        return tag

    def handle_pad(
        self, idx: int, *, from_touch: bool = False, allow_record: bool = True
    ) -> str:
        """
        Touch square or MPK pad hit.
        Empty → arm record (EDIT); filled → launch/toggle; touch on armed cell → stop record.
        PLAY view passes allow_record=False so empty pads only select.
        While a cell is recording, MPK pads on *that* cell (and other empty
        cells) stay drums for the take — but a filled cell still launches,
        matching the touch grid. Returns a short action tag for the UI/log.
        """
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return "ignore"
        with self._lock:
            rec = self._recording_cell
            empty = self._cells[idx].is_empty()
        # Only the touch square ends record — MPK pads stay free for drum takes
        if rec is not None and rec == idx and from_touch:
            self.stop_record()
            return "stop_rec"
        if rec is not None and rec == idx and not from_touch:
            return "ignore"
        if rec is not None and not from_touch and empty:
            # Other empty pads also record as drums while a take is armed
            return "ignore"
        if empty:
            if not allow_record:
                self.select(idx)
                return "empty"
            self.arm_record(idx)
            return "arm"
        return self.launch(idx)

    def _release_held(self, idx: int, *, send_midi: bool = True) -> None:
        with self._lock:
            held = list(self._held.pop(idx, set()))
        port = self._get_outport() if send_midi else None
        for ch, note in held:
            try:
                self._engine.note_off(ch, note)
            except Exception:
                pass
            try:
                self._emit(("off", ch, note))
            except Exception:
                pass
            if port is not None:
                try:
                    port.send(
                        mido.Message("note_off", channel=ch & 0x0F, note=note & 0x7F, velocity=0)
                    )
                except Exception:
                    pass

    def _emit_phrase_note(
        self,
        *,
        on: bool,
        src_channel: int,
        note: int,
        velocity: int,
        out_channel: int,
        local_synth: bool,
        out_mode: str,
        timbre: Optional[np.ndarray],
        fx_name: Optional[str],
        idx: int,
    ) -> None:
        ch = (out_channel & 0x0F) if out_channel >= 0 else (src_channel & 0x0F)
        n = note & 0x7F
        want_usb = out_mode in ("usb", "both")
        want_local = bool(local_synth) and out_mode in ("local", "both")
        vib: Optional[Tuple[float, float, float]] = None
        if on:
            # Read trim + vibrato live so edits land without relaunching the pad
            with self._lock:
                if 0 <= idx < PHRASE_PAD_COUNT:
                    cell = self._cells[idx]
                    gain = cell.gain
                    vib = cell.vib_tuple()
                else:
                    gain = 1.0
            velocity = scale_velocity(velocity, gain)
        if want_usb:
            port = self._get_outport()
            if port is not None:
                try:
                    if on:
                        port.send(
                            mido.Message(
                                "note_on",
                                channel=ch,
                                note=n,
                                velocity=max(1, min(127, int(velocity))),
                            )
                        )
                    else:
                        port.send(
                            mido.Message("note_off", channel=ch, note=n, velocity=0)
                        )
                except Exception:
                    pass
        if want_local:
            if on:
                # Drums use per-model FX inside the engine; keys use fx_name slot.
                use_fx = fx_name if (ch & 0x0F) != DRUM_CHANNEL else None
                self._engine.note_on(
                    ch, n, velocity, timbre=timbre, fx_name=use_fx, vib=vib
                )
                with self._lock:
                    self._held.setdefault(idx, set()).add((ch, n))
                self._emit(("on", ch, n, velocity))
            else:
                self._engine.note_off(ch, n)
                with self._lock:
                    self._held.setdefault(idx, set()).discard((ch, n))
                self._emit(("off", ch, n))
        elif on:
            # Track for USB-only note-offs even without local synth
            with self._lock:
                self._held.setdefault(idx, set()).add((ch, n))
        else:
            with self._lock:
                self._held.setdefault(idx, set()).discard((ch, n))

    def _play_cell(
        self,
        idx: int,
        events: List[LoopEvent],
        length: float,
        stop_ev: threading.Event,
        loop: bool,
        timbre: Optional[np.ndarray],
        fx_name: Optional[str],
        out_channel: int,
        local_synth: bool,
        out_mode: str,
    ) -> None:
        try:
            if not events or length <= 0.0:
                return
            while not stop_ev.is_set():
                t0 = time.monotonic()
                self._release_held(idx, send_midi=True)
                with self._lock:
                    self._held[idx] = set()
                for ev in events:
                    if stop_ev.is_set():
                        break
                    target = t0 + ev.t
                    while True:
                        remain = target - time.monotonic()
                        if remain <= 0:
                            break
                        if stop_ev.wait(min(0.003, remain)):
                            break
                    if stop_ev.is_set():
                        break
                    self._emit_phrase_note(
                        on=bool(ev.on),
                        src_channel=ev.channel,
                        note=ev.note,
                        velocity=ev.velocity,
                        out_channel=out_channel,
                        local_synth=local_synth,
                        out_mode=out_mode,
                        timbre=timbre if (ev.channel & 0x0F) != DRUM_CHANNEL else None,
                        fx_name=fx_name,
                        idx=idx,
                    )
                end = t0 + length
                while not stop_ev.is_set():
                    remain = end - time.monotonic()
                    if remain <= 0:
                        break
                    if stop_ev.wait(min(0.003, remain)):
                        break
                if not loop:
                    break
                self._release_held(idx, send_midi=True)
        finally:
            self._release_held(idx, send_midi=True)
            with self._lock:
                if self._playing.get(idx) is stop_ev:
                    self._playing.pop(idx, None)
                    self._threads.pop(idx, None)
                self._active_timbres.pop(idx, None)
            try:
                self._emit(("phrase",))
            except Exception:
                pass


