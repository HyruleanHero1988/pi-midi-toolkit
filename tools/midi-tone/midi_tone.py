#!/usr/bin/env python3
"""
midi-tone — Phase 0 diagnostic turned tiny DIY soft-synth.

MPK (or any MIDI in) → wavetable soft-synth + event UI.
Keep lean for Raspberry Pi 2 (wavetable synth, capped polyphony).
"""

from __future__ import annotations

import argparse
import json
import os
import math
import pathlib
import queue
import re
import shutil
import sys
import subprocess
import threading
import time
import tkinter as tk
import wave
from dataclasses import dataclass
from tkinter import ttk
from typing import Any, Dict, List, Optional, Tuple

try:
    import numpy as np
except ImportError as e:
    sys.exit("numpy required: pip install numpy (or apt install python3-numpy)\n" + str(e))

try:
    import sounddevice as sd
except ImportError as e:
    sys.exit(
        "sounddevice required: pip install sounddevice\n"
        "On Pi you may also need: sudo apt install libportaudio2\n" + str(e)
    )

try:
    import mido
except ImportError as e:
    sys.exit("mido required: pip install mido python-rtmidi\n" + str(e))

from sequencer import (  # noqa: E402  (local module, imported after the hard deps)
    SEQ_EMPTY,
    SEQ_OVERDUB,
    SEQ_REC_BACKBONE,
    LoopEvent,
    OverdubSequencer,
    trim_loop_take,
)
from screensaver import (  # noqa: E402
    PIXEL_SHIFT_AMPLITUDE,
    IdleWatch,
    PanelBacklight,
    next_timeout_preset,
    orbit_xy,
    pixel_shift_xy,
    timeout_from_env,
    timeout_label,
)
import updater  # noqa: E402  (local module; SET screen software update)


SAMPLE_RATE = 44100
# Larger default block / latency = fewer xruns on Pi (crunchy / robotic audio).
# Override for lower latency if the machine can take it:
#   MIDI_TONE_BLOCKSIZE=512 MIDI_TONE_LATENCY=0.03
# Or bump further on a loaded Pi:
#   MIDI_TONE_BLOCKSIZE=1024 MIDI_TONE_LATENCY=0.08
BLOCKSIZE = max(64, int(os.environ.get("MIDI_TONE_BLOCKSIZE", "1536")))
LATENCY_SEC = max(0.005, float(os.environ.get("MIDI_TONE_LATENCY", "0.10")))
DEFAULT_MAX_VOICES = 12
TABLE_SIZE = 2048
TABLE_MASK = TABLE_SIZE - 1
LOG_MAX = 60
EVENT_Q_MAX = 200
# MIDI channel 10 (1-based) = index 9 — MPK drum pads
DRUM_CHANNEL = 9
NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_WAVETABLE_DIR = HERE / "wavetables"
USER_WAVETABLES_DIR = HERE / "user-wavetables"
SETTINGS_PATH = HERE / "settings.json"
PRESETS_DIR = HERE / "user-presets"
PRESET_SLOTS = 8
SONGS_DIR = HERE / "songs"
DEMO_SONGS_DIR = HERE / "demo-songs"
SONG_SEED_MARKER = SONGS_DIR / ".seeded-from-demo"
SONG_LIST_VISIBLE = 4  # chunky rows on screen at once
DEFAULT_SONG_BPM = 120
SONG_OUT_MODES = ("local", "usb", "both")
# Prefer these substrings when auto-picking USB→DIN output
SONG_OUT_PREFER = ("u2midi", "uxmidi", "hxmidi", "din", "usb midi", "midi out", "mio", "um-")
PHRASES_DIR = HERE / "phrases"
PHRASE_PAD_COUNT = 16  # MPK Bank A + Bank B
PHRASE_PAD_BASE = 36   # factory MPC program: pads 36–51
# Visual 4×4 (top→bottom): A top, A bottom, B top, B bottom — matches MPK pad layout
PHRASE_GRID_CELLS = (
    4, 5, 6, 7,      # Bank A pads 5–8 (top row)
    0, 1, 2, 3,      # Bank A pads 1–4 (bottom row)
    12, 13, 14, 15,  # Bank B pads 5–8
    8, 9, 10, 11,    # Bank B pads 1–4
)
MAX_PHRASE_PLAYERS = 8
SETTINGS_VERSION = 2
BUILTIN_VOICE_NAMES = ("sine", "square", "saw", "triangle")
VOICE_NAME_MAX = 24
TOUCH_SCROLL_THRESH_PX = 10  # press→drag before a grid tap counts as scroll (capacitive)
UI_MODES = ("home", "synth", "seq", "pads", "songs", "presets", "log", "settings")
JAM_NAV_MODES = ("synth", "seq", "pads")
HOME_TILES = (
    ("synth", "SYNTH", "soft-synth", "#458588"),
    ("seq", "SEQ", "overdub", "#b16286"),
    ("pads", "PADS", "phrases", "#d79921"),
    ("songs", "SONGS", ".mid files", "#689d6a"),
    ("presets", "PRESETS", "8 slots", "#83a598"),
    ("log", "LOG", "history", "#504945"),
    ("settings", "SET", "update", "#665c54"),
)


def list_song_files(directory: pathlib.Path = SONGS_DIR) -> List[pathlib.Path]:
    """All Standard MIDI Files in songs/ (sorted, case-insensitive)."""
    if not directory.is_dir():
        return []
    files = [
        p
        for p in directory.iterdir()
        if p.is_file() and p.suffix.lower() in (".mid", ".midi")
    ]
    files.sort(key=lambda p: p.name.lower())
    return files


def seed_demo_songs() -> int:
    """Copy any missing bundled demos into ./songs/ (offline-friendly).

    Never overwrites existing files — if you DELETE a demo it stays gone.
    New demos added in a later deploy still appear on the next launch.
    """
    if not DEMO_SONGS_DIR.is_dir():
        return 0
    SONGS_DIR.mkdir(parents=True, exist_ok=True)
    copied = 0
    for src in sorted(DEMO_SONGS_DIR.glob("*.mid")):
        dest = SONGS_DIR / src.name
        if dest.exists():
            continue
        try:
            shutil.copy2(src, dest)
            copied += 1
        except Exception as exc:
            print(f"demo song seed failed ({src.name}): {exc}", flush=True)
    if copied:
        try:
            SONG_SEED_MARKER.write_text(
                "Demo songs copied from demo-songs/ (Mutopia pack).\n"
                "Missing demos are filled on launch; existing/deleted files are left alone.\n",
                encoding="utf-8",
            )
        except Exception as exc:
            print(f"demo song marker write failed: {exc}", flush=True)
    return copied

# Akai MPK mini mk3 factory knobs (Prog Select → Pad 1 / MPC program): CC70–77
CC_MORPH = 70          # Knob 1 — scan / blend through wavetable stack
CC_TONE = 71           # Knob 2 — brightness (low-pass)
CC_ATTACK = 72         # Knob 3 — amp attack
CC_RELEASE = 73        # Knob 4 — amp release
CC_VIB_DEPTH = 74      # Knob 5 — vibrato depth
CC_VIB_RATE = 75       # Knob 6 — vibrato rate
# Touch steps for the VOICES-screen vibrato pair
VIB_DEPTH_STEP = 0.10  # semitones
VIB_RATE_STEP = 0.5    # Hz
CC_LEVEL = 77          # Knob 8 — output level
# Knob 7 (CC76) reserved / free for later
KNOB_CCS = {CC_MORPH, CC_TONE, CC_ATTACK, CC_RELEASE, CC_VIB_DEPTH, CC_VIB_RATE, CC_LEVEL}


# Peak scale for wavetable cycles (was 0.35 — far too quiet into the Pi jack)
TABLE_PEAK = 0.90
# Per-voice amp at velocity 127 (was 0.12 → ~-28 dBFS with old table scale)
VOICE_AMP = 0.48
# Extra mix bus makeup before soft-limit (Pi headphone/line outs are timid)
OUTPUT_MAKEUP = 1.65
DRUM_BUS_GAIN = 1.55
# Mix-bus FX delay line (~1s) and short reverb tank
FX_DELAY_MAX_SEC = 1.0
FX_REVERB_MAX_SEC = 0.55


class MixBusFx:
    """
    Per-instrument FX insert (one instance per wavetable voice or drum model).
    drive → echo → short multi-tap tank. Params 0..1; process() mutates buf.
    Not a global master-bus effect — melody and drums keep separate slots.
    """

    def __init__(self, sample_rate: int) -> None:
        self.sample_rate = max(8000, int(sample_rate))
        self.drive = 0.0
        self.delay_time = 0.28  # mapped 50ms..750ms
        self.delay_fb = 0.35
        self.delay_mix = 0.0
        self.reverb_size = 0.45
        self.reverb_mix = 0.0
        dlen = max(64, int(self.sample_rate * FX_DELAY_MAX_SEC))
        rlen = max(64, int(self.sample_rate * FX_REVERB_MAX_SEC))
        self._delay = np.zeros(dlen, dtype=np.float32)
        self._dpos = 0
        self._reverb = np.zeros(rlen, dtype=np.float32)
        self._rpos = 0
        self._rev_lp = 0.0
        self._tmp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._wet = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._idx = np.zeros(BLOCKSIZE * 2, dtype=np.int32)
        self._write_idx = np.zeros(BLOCKSIZE * 2, dtype=np.int32)
        self._arange_i = np.arange(BLOCKSIZE * 2, dtype=np.int32)

    def _ensure_work_bufs(self, n: int) -> None:
        if n > self._tmp.shape[0]:
            self._tmp = np.zeros(n, dtype=np.float32)
            self._wet = np.zeros(n, dtype=np.float32)
            self._idx = np.zeros(n, dtype=np.int32)
            self._write_idx = np.zeros(n, dtype=np.int32)
        if n > self._arange_i.shape[0]:
            self._arange_i = np.arange(n, dtype=np.int32)

    def snapshot(self) -> Dict[str, float]:
        return {
            "fx_drive": float(self.drive),
            "fx_delay_time": float(self.delay_time),
            "fx_delay_fb": float(self.delay_fb),
            "fx_delay_mix": float(self.delay_mix),
            "fx_reverb_size": float(self.reverb_size),
            "fx_reverb_mix": float(self.reverb_mix),
        }

    def apply_snapshot(self, data: Dict[str, Any]) -> None:
        def _f(key: str, cur: float) -> float:
            if key not in data:
                return cur
            try:
                return max(0.0, min(1.0, float(data[key])))
            except (TypeError, ValueError):
                return cur

        self.drive = _f("fx_drive", self.drive)
        self.delay_time = _f("fx_delay_time", self.delay_time)
        self.delay_fb = _f("fx_delay_fb", self.delay_fb)
        self.delay_mix = _f("fx_delay_mix", self.delay_mix)
        self.reverb_size = _f("fx_reverb_size", self.reverb_size)
        self.reverb_mix = _f("fx_reverb_mix", self.reverb_mix)

    def is_dry(self) -> bool:
        """True when process() would be a no-op (skip in the audio callback)."""
        return (
            float(self.drive) <= 0.001
            and float(self.delay_mix) <= 0.001
            and float(self.delay_fb) <= 0.001
            and float(self.reverb_mix) <= 0.001
        )

    def reset_to_defaults(self) -> None:
        """Hard reset params + clear delay/reverb memory (kills leftover echo)."""
        self.drive = 0.0
        self.delay_time = 0.28
        self.delay_fb = 0.35
        self.delay_mix = 0.0
        self.reverb_size = 0.45
        self.reverb_mix = 0.0
        self._delay.fill(0.0)
        self._reverb.fill(0.0)
        self._dpos = 0
        self._rpos = 0
        self._rev_lp = 0.0

    def process(self, buf: np.ndarray) -> None:
        n = len(buf)
        if n == 0:
            return
        self._ensure_work_bufs(n)
        arange_i = self._arange_i[:n]
        # --- Drive (waveshape) ---
        drive = float(self.drive)
        if drive > 0.001:
            # 0 → gentle; 1 → hot tanh
            amount = 1.0 + drive * 12.0
            np.multiply(buf, np.float32(amount), out=self._tmp[:n])
            np.tanh(self._tmp[:n], out=self._tmp[:n])
            # Compensate so drive=0-ish loudness stays similar at low drive
            norm = math.tanh(amount) if amount > 1e-6 else 1.0
            np.multiply(self._tmp[:n], np.float32(1.0 / max(0.25, norm)), out=buf)

        # --- Echo / delay ---
        dmix = float(self.delay_mix)
        if dmix > 0.001 or float(self.delay_fb) > 0.001:
            dbuf = self._delay
            dlen = len(dbuf)
            # 50ms .. 750ms
            delay_sec = 0.05 + max(0.0, min(1.0, float(self.delay_time))) * 0.70
            ds = max(1, min(dlen - 1, int(delay_sec * self.sample_rate)))
            fb = max(0.0, min(0.92, float(self.delay_fb)))
            pos = self._dpos
            wet = self._wet[:n]
            idx = self._idx[:n]
            write_idx = self._write_idx[:n]
            np.add(np.int32(pos - ds), arange_i, out=idx)
            np.remainder(idx, dlen, out=idx)
            np.take(dbuf, idx, out=wet)
            np.add(np.int32(pos), arange_i, out=write_idx)
            np.remainder(write_idx, dlen, out=write_idx)
            dbuf[write_idx] = buf + wet * np.float32(fb)
            self._dpos = (pos + n) % dlen
            if dmix > 0.001:
                dry = 1.0 - dmix
                buf *= np.float32(dry)
                buf += wet * np.float32(dmix)

        # --- Short recirculating multi-tap tank (reverb-ish) ---
        rmix = float(self.reverb_mix)
        if rmix > 0.001:
            rbuf = self._reverb
            rlen = len(rbuf)
            size = max(0.0, min(1.0, float(self.reverb_size)))
            # Tap spacings scale with size
            base = int((0.018 + 0.040 * size) * self.sample_rate)
            taps = (
                max(1, base),
                max(1, int(base * 1.7)),
                max(1, int(base * 2.5)),
                max(1, int(base * 3.4)),
            )
            gains = (0.55, 0.40, 0.30, 0.22)
            pos = self._rpos
            wet = self._wet[:n]
            idx = self._idx[:n]
            wet.fill(0.0)
            for tap, g in zip(taps, gains):
                tap = min(tap, rlen - 1)
                np.add(np.int32(pos - tap), arange_i, out=idx)
                np.remainder(idx, rlen, out=idx)
                wet += np.take(rbuf, idx) * np.float32(g)
            # Soften highs with a cheap 2-tap blur (size → darker)
            if size > 0.05:
                soft = self._tmp[:n]
                soft[0] = wet[0]
                soft[1:] = 0.5 * (wet[1:] + wet[:-1])
                blend = np.float32(max(0.0, min(1.0, size)))
                wet *= np.float32(1.0 - blend)
                wet += soft * blend
            fb = 0.25 + 0.45 * size
            write_idx = self._write_idx[:n]
            np.add(np.int32(pos), arange_i, out=write_idx)
            np.remainder(write_idx, rlen, out=write_idx)
            rbuf[write_idx] = buf * np.float32(0.7) + wet * np.float32(fb)
            self._rpos = (pos + n) % rlen
            dry = 1.0 - rmix
            buf *= np.float32(dry)
            buf += wet * np.float32(rmix)


def _builtin_tables() -> Dict[str, np.ndarray]:
    t = np.linspace(0.0, 1.0, TABLE_SIZE, endpoint=False, dtype=np.float64)
    sine = (np.sin(2.0 * np.pi * t) * TABLE_PEAK).astype(np.float32)
    square = np.where(t < 0.5, TABLE_PEAK, -TABLE_PEAK).astype(np.float32)
    saw = (2.0 * (t - np.floor(t + 0.5))).astype(np.float32) * np.float32(TABLE_PEAK)
    triangle = (
        (2.0 * np.abs(2.0 * (t - np.floor(t + 0.5))) - 1.0).astype(np.float32)
        * np.float32(TABLE_PEAK)
    )
    return {
        "sine": sine,
        "square": square,
        "saw": saw,
        "triangle": triangle,
    }


def _load_wav_mono(path: pathlib.Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        ch = w.getnchannels()
        sw = w.getsampwidth()
        n = w.getnframes()
        raw = w.readframes(n)
    if sw == 2:
        x = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    elif sw == 4:
        # int32 PCM or float32 — detect by range later; treat as int32 first
        as_i = np.frombuffer(raw, dtype="<i4")
        if np.max(np.abs(as_i)) > 8:
            x = as_i.astype(np.float32) / 2147483648.0
        else:
            x = np.frombuffer(raw, dtype="<f4").copy()
    else:
        raise ValueError(f"{path}: unsupported sample width {sw}")
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x


def _resample_cycle(x: np.ndarray, n: int = TABLE_SIZE) -> np.ndarray:
    if len(x) == n:
        y = x.astype(np.float32, copy=True)
    else:
        phase = np.linspace(0.0, 1.0, n, endpoint=False, dtype=np.float64)
        idx = phase * len(x)
        i0 = np.floor(idx).astype(np.int64) % len(x)
        i1 = (i0 + 1) % len(x)
        frac = (idx - np.floor(idx)).astype(np.float32)
        y = (x[i0] * (1.0 - frac) + x[i1] * frac).astype(np.float32)
    peak = float(np.max(np.abs(y))) or 1.0
    return (y / peak) * np.float32(TABLE_PEAK)


def load_wavetables(*directories: pathlib.Path) -> Dict[str, np.ndarray]:
    """Built-ins first, then *.wav from each directory (later dirs override)."""
    tables = _builtin_tables()
    for directory in directories:
        if not directory or not pathlib.Path(directory).is_dir():
            continue
        for path in sorted(pathlib.Path(directory).glob("*.wav")):
            name = path.stem.lower().strip()
            if not name:
                continue
            # Keep core procedural oscillators; files can add/replace everything else
            if name in BUILTIN_VOICE_NAMES:
                continue
            try:
                tables[name] = _resample_cycle(_load_wav_mono(path))
            except Exception as exc:
                print(f"wavetable skip {path.name}: {exc}", flush=True)
    return tables


def sanitize_voice_name(raw: str) -> str:
    """Lowercase [a-z0-9_], max VOICE_NAME_MAX — safe for filenames + UI."""
    s = re.sub(r"[^a-z0-9_]+", "_", str(raw or "").lower().strip())
    s = re.sub(r"_+", "_", s).strip("_")
    if not s:
        s = "voice"
    return s[:VOICE_NAME_MAX]


def suggest_voice_name(name_a: str, name_b: str, morph: float) -> str:
    a = sanitize_voice_name(name_a) or "a"
    b = sanitize_voice_name(name_b) or "b"
    pct = int(round(max(0.0, min(1.0, float(morph))) * 100.0))
    if a == b:
        return sanitize_voice_name(f"{a}_saved")
    return sanitize_voice_name(f"{a}_{b}_{pct}")


def unique_voice_name(base: str, existing: List[str]) -> str:
    key = sanitize_voice_name(base)
    if key not in BUILTIN_VOICE_NAMES and key not in existing:
        return key
    for n in range(2, 1000):
        suffix = f"_{n}"
        stem = key[: max(1, VOICE_NAME_MAX - len(suffix))]
        cand = f"{stem}{suffix}"
        if cand not in BUILTIN_VOICE_NAMES and cand not in existing:
            return cand
    return sanitize_voice_name(f"voice_{int(time.time()) % 100000}")


def write_wavetable_wav(
    path: pathlib.Path, table: np.ndarray, *, sample_rate: int = 44100
) -> None:
    """Write a mono 16-bit single-cycle WAV (loadable by load_wavetables)."""
    y = np.asarray(table, dtype=np.float32).reshape(-1)
    if y.shape[0] != TABLE_SIZE:
        y = _resample_cycle(y)
    peak = float(np.max(np.abs(y))) or 1.0
    pcm = np.clip(y / peak * 32767.0, -32768, 32767).astype("<i2")
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(sample_rate))
        w.writeframes(pcm.tobytes())


# Time-domain FX that cannot bake into a single cycle — stored beside the .wav
VOICE_FX_SIDECAR_KEYS = (
    "fx_delay_time",
    "fx_delay_fb",
    "fx_delay_mix",
    "fx_reverb_size",
    "fx_reverb_mix",
)


def voice_fx_sidecar_path(waves_dir: pathlib.Path, name: str) -> pathlib.Path:
    return pathlib.Path(waves_dir) / f"{sanitize_voice_name(name)}.fx.json"


def write_voice_fx_sidecar(path: pathlib.Path, fx: Dict[str, float]) -> None:
    """Tiny JSON next to a user wavetable: delay/reverb mixes (+ time/fb/size)."""
    out = {"version": 1}
    for key in VOICE_FX_SIDECAR_KEYS:
        try:
            out[key] = max(0.0, min(1.0, float(fx.get(key, 0.0))))
        except (TypeError, ValueError):
            out[key] = 0.0
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def load_voice_fx_sidecar(path: pathlib.Path) -> Optional[Dict[str, float]]:
    path = pathlib.Path(path)
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None
    if not isinstance(data, dict):
        return None
    out: Dict[str, float] = {}
    for key in VOICE_FX_SIDECAR_KEYS:
        if key not in data:
            continue
        try:
            out[key] = max(0.0, min(1.0, float(data[key])))
        except (TypeError, ValueError):
            continue
    return out or None


def load_user_voice_fx_map(directory: pathlib.Path) -> Dict[str, Dict[str, float]]:
    """voice name → delay/reverb sidecar for user-wavetables/."""
    result: Dict[str, Dict[str, float]] = {}
    directory = pathlib.Path(directory)
    if not directory.is_dir():
        return result
    for path in sorted(directory.glob("*.fx.json")):
        name = sanitize_voice_name(path.name[: -len(".fx.json")])
        if not name or name in BUILTIN_VOICE_NAMES:
            continue
        snap = load_voice_fx_sidecar(path)
        if snap:
            result[name] = snap
    return result


def circular_moving_average(x: np.ndarray, win: int) -> np.ndarray:
    """Tone-bake helper: low-pass a periodic single cycle."""
    n = int(x.shape[0])
    win = max(1, int(win))
    if win <= 1 or n == 0:
        return np.asarray(x, dtype=np.float32)
    k = np.ones(win, dtype=np.float32) / np.float32(win)
    tiled = np.tile(np.asarray(x, dtype=np.float32), 3)
    y = np.convolve(tiled, k, mode="same")
    return y[n : 2 * n].astype(np.float32, copy=False)


def midi_note_name(note: int) -> str:
    return f"{NOTE_NAMES[note % 12]}{note // 12 - 1}"


def midi_to_hz(note: int) -> float:
    return 440.0 * (2.0 ** ((note - 69) / 12.0))


# MPK mini mk3 factory pad notes (Prog Select → Pad 1 / MPC): Bank A = 36–43, Bank B = 44–51.
# Layout per bank: pads 1–4 bottom L→R, pads 5–8 top L→R.
MPK_PAD_KIT: Dict[int, str] = {
    # Bank A
    36: "kick",
    37: "snare",
    38: "clap",
    39: "hat_closed",
    40: "hat_open",
    41: "tom_lo",
    42: "tom_mid",
    43: "rim",
    # Bank B
    44: "kick_tight",
    45: "rimshot",
    46: "shaker",
    47: "hat_pedal",
    48: "tom_hi",
    49: "cowbell",
    50: "clave",
    51: "ride",
}

# Extra GM-ish aliases so other pad programs still land somewhere useful
_DRUM_ALIASES: Dict[int, str] = {
    35: "kick",
    52: "ride",
    55: "hat_open",
    57: "ride",
    59: "ride",
}


def drum_model_for_note(note: int) -> str:
    """Map MIDI note (ch10) → one of 16 procedural drum / one-shot models."""
    n = note & 0x7F
    if n in MPK_PAD_KIT:
        return MPK_PAD_KIT[n]
    if n in _DRUM_ALIASES:
        return _DRUM_ALIASES[n]
    # Unknown note: cycle through the kit so every pad still sounds distinct
    models = list(dict.fromkeys(MPK_PAD_KIT.values()))
    return models[n % len(models)]


def mpk_note_for_phrase_cell(cell: int) -> int:
    """Phrase/pad cell index 0..15 → factory MPK note 36..51."""
    return PHRASE_PAD_BASE + (int(cell) & 0x0F)


SCOPE_CRT_BG = "#031a08"
SCOPE_CRT_WAVE = "#39ff14"  # phosphor green
SCOPE_REDRAW_DEBOUNCE_S = 0.04  # wait after last change before heavy paint
SCOPE_REDRAW_MAX_WAIT_S = 0.10  # never leave the scope blank longer than this
SCOPE_CRT_GRID = "#14532d"
SCOPE_CRT_AXIS = "#4ade80"
SCOPE_MORPH_CYCLES = 3  # several periods — one cycle looks sparse on a wide panel
DRUM_SCOPE_SEC = 0.40  # fixed time window so stretch reads as envelope length


def downsample_waveform(samples: np.ndarray, points: int) -> np.ndarray:
    """Reduce a sample buffer to `points` peaks for canvas drawing."""
    if samples is None or len(samples) == 0 or points <= 0:
        return np.zeros(max(1, points), dtype=np.float32)
    x = np.asarray(samples, dtype=np.float32)
    if len(x) <= points:
        return x.copy()
    # Min/max buckets keep spikes visible (better than plain stride for drums)
    bucket = len(x) / float(points)
    out = np.empty(points, dtype=np.float32)
    for i in range(points):
        a = int(i * bucket)
        b = int((i + 1) * bucket)
        if b <= a:
            b = a + 1
        chunk = x[a:b]
        # Alternate extrema so the polyline fills the envelope
        out[i] = float(np.max(chunk) if (i & 1) == 0 else np.min(chunk))
    return out


def render_drum_preview(
    model: str,
    *,
    pitch: float,
    decay: float,
    noise_amt: float,
    tone: float,
    sample_rate: int = SAMPLE_RATE,
    duration_sec: float = 0.32,
    velocity: float = 0.95,
    seed: int = 7,
) -> np.ndarray:
    """
    Offline one-shot for the drum scope (seeded noise so redraws stay stable).

    Uses a fixed time window so stretch/decay reads as how much of the scope
    stays alive (short = early silence; long = fills the frame).
    """
    sr = max(8000, int(sample_rate))
    n = max(32, int(duration_sec * sr))
    t = np.arange(n, dtype=np.float32) * np.float32(1.0 / sr)
    arange = np.arange(n, dtype=np.float32)
    rng = np.random.RandomState(int(seed) & 0xFFFFFFFF)
    white = (rng.random(n).astype(np.float32) * 2.0 - 1.0)
    win = max(1, int(round((1.0 - max(0.0, min(1.0, tone))) * 16.0)))
    if win <= 1:
        noise = white
    else:
        kernel = np.ones(win, dtype=np.float32) / np.float32(win)
        noise = np.convolve(white, kernel, mode="same").astype(np.float32)
    audio, _dur, _phase = synthesize_drum(
        model,
        t=t,
        arange=arange,
        white=white,
        noise=noise,
        pitch=max(0.0, min(1.0, float(pitch))),
        decay=max(0.0, min(1.0, float(decay))),
        noise_amt=max(0.0, min(1.0, float(noise_amt))),
        tone=max(0.0, min(1.0, float(tone))),
        vel=max(0.05, min(1.0, float(velocity))),
        phase=0.0,
        two_pi=2.0 * math.pi,
        inv_sr=1.0 / sr,
    )
    return audio.astype(np.float32, copy=False)


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


def synthesize_drum(
    model: str,
    *,
    t: np.ndarray,
    arange: np.ndarray,
    white: np.ndarray,
    noise: np.ndarray,
    pitch: float,
    decay: float,
    noise_amt: float,
    tone: float,
    vel: float,
    phase: float,
    two_pi: float,
    inv_sr: float,
) -> Tuple[np.ndarray, float, float]:
    """Render one block of a procedural drum. Returns (audio, dur_sec, new_phase)."""
    # Pitch-envelope body (kick / toms / tight kick)
    if model in ("kick", "kick_tight", "tom_lo", "tom_mid", "tom_hi"):
        if model == "kick":
            f0 = 50.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 28.0 + 16.0 * pitch
            drop_tau = 0.016 + 0.05 * (1.0 - decay)
            body_tau = 0.07 + 0.40 * decay
            amp = 0.38
        elif model == "kick_tight":
            f0 = 68.0 * (2.0 ** ((pitch - 0.5) * 1.6))
            f_end = 40.0 + 20.0 * pitch
            drop_tau = 0.010 + 0.03 * (1.0 - decay)
            body_tau = 0.035 + 0.18 * decay
            amp = 0.34
        elif model == "tom_lo":
            f0 = 85.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 55.0 + 25.0 * pitch
            drop_tau = 0.025 + 0.07 * (1.0 - decay)
            body_tau = 0.08 + 0.38 * decay
            amp = 0.32
        elif model == "tom_mid":
            f0 = 120.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 75.0 + 30.0 * pitch
            drop_tau = 0.022 + 0.06 * (1.0 - decay)
            body_tau = 0.06 + 0.30 * decay
            amp = 0.30
        else:  # tom_hi
            f0 = 170.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 100.0 + 40.0 * pitch
            drop_tau = 0.018 + 0.05 * (1.0 - decay)
            body_tau = 0.045 + 0.22 * decay
            amp = 0.28
        freq = f_end + (f0 - f_end) * np.exp(-t / np.float32(drop_tau))
        phase_inc = two_pi * freq * inv_sr
        phases = phase + np.cumsum(phase_inc)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        click = np.exp(-t / np.float32(0.0035)) * np.float32(0.18 * vel)
        body = np.sin(phases) * env * np.float32(amp * vel)
        sig = body + click * white + noise * np.float32(0.05 * noise_amt * vel) * env
        return sig.astype(np.float32, copy=False), body_tau * 4.5, new_phase

    if model in ("snare", "rimshot"):
        f0 = (200.0 if model == "rimshot" else 175.0) * (2.0 ** ((pitch - 0.5) * 1.4))
        body_tau = 0.018 + 0.10 * decay if model == "rimshot" else 0.03 + 0.18 * decay
        noise_tau = 0.025 + 0.14 * decay if model == "rimshot" else 0.04 + 0.28 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        tone_env = np.exp(-t / np.float32(body_tau))
        noise_env = np.exp(-t / np.float32(noise_tau))
        body_amp = 0.10 if model == "rimshot" else 0.16
        noise_amp = (0.28 + 0.45 * noise_amt) if model == "rimshot" else (0.18 + 0.40 * noise_amt)
        click = np.exp(-t / np.float32(0.002)) * np.float32(0.25 * vel if model == "rimshot" else 0.12 * vel)
        sig = np.sin(phases) * tone_env * np.float32(body_amp * vel)
        sig += noise * noise_env * np.float32(noise_amp * vel)
        sig += click * white
        return sig.astype(np.float32, copy=False), max(body_tau, noise_tau) * 5.0, new_phase

    if model == "clap":
        noise_tau = 0.03 + 0.22 * decay
        noise_env = np.exp(-t / np.float32(noise_tau))
        bursts = (
            np.exp(-((t - 0.000) ** 2) / 0.0000008)
            + np.exp(-((t - 0.012) ** 2) / 0.0000010)
            + np.exp(-((t - 0.024) ** 2) / 0.0000012)
        )
        sig = noise * (bursts * np.float32(0.45) + noise_env * np.float32(0.28))
        sig *= np.float32((0.28 + 0.4 * noise_amt) * vel)
        return sig.astype(np.float32, copy=False), noise_tau * 5.0, phase

    if model in ("hat_closed", "hat_open", "hat_pedal", "ride", "shaker"):
        if model == "hat_open":
            noise_tau = 0.05 + 0.40 * decay
            amp = 0.14 + 0.30 * noise_amt
        elif model == "hat_pedal":
            noise_tau = 0.008 + 0.04 * decay
            amp = 0.12 + 0.22 * noise_amt
        elif model == "ride":
            noise_tau = 0.12 + 0.55 * decay
            amp = 0.10 + 0.22 * noise_amt
        elif model == "shaker":
            noise_tau = 0.02 + 0.10 * decay
            amp = 0.12 + 0.28 * noise_amt
        else:  # hat_closed
            noise_tau = 0.015 + 0.08 * decay
            amp = 0.14 + 0.30 * noise_amt
        bright = white - noise * np.float32(0.85)
        noise_env = np.exp(-t / np.float32(noise_tau))
        if model == "shaker":
            # Grainy amplitude modulation
            grain = 0.55 + 0.45 * np.sin(two_pi * (40.0 + 80.0 * pitch) * t)
            sig = bright * noise_env * grain * np.float32(amp * vel)
        else:
            sig = bright * noise_env * np.float32(amp * vel)
        sig = sig * np.float32(0.35 + 0.65 * tone) + noise * noise_env * np.float32(
            0.10 * (1.0 - tone) * vel
        )
        return sig.astype(np.float32, copy=False), noise_tau * 5.5, phase

    if model == "rim":
        # Short woodblock / stick click
        f0 = 520.0 * (2.0 ** ((pitch - 0.5) * 1.2))
        body_tau = 0.012 + 0.05 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        sig = np.sin(phases) * env * np.float32(0.22 * vel)
        sig += white * np.exp(-t / np.float32(0.004)) * np.float32(0.18 * vel)
        return sig.astype(np.float32, copy=False), body_tau * 5.0, new_phase

    if model == "cowbell":
        # Two inharmonic partials (classic analog cowbell trick)
        f1 = 540.0 * (2.0 ** ((pitch - 0.5) * 1.0))
        f2 = 800.0 * (2.0 ** ((pitch - 0.5) * 1.0))
        body_tau = 0.05 + 0.28 * decay
        env = np.exp(-t / np.float32(body_tau))
        phase_inc1 = two_pi * f1 * inv_sr
        phase_inc2 = two_pi * f2 * inv_sr
        p1 = phase + phase_inc1 * (arange + 1.0)
        p2 = phase * 1.37 + phase_inc2 * (arange + 1.0)
        new_phase = float(p1[-1] % two_pi)
        sig = (np.sin(p1) + 0.7 * np.sin(p2)) * env * np.float32(0.18 * vel)
        sig += noise * env * np.float32(0.04 * noise_amt * vel)
        return sig.astype(np.float32, copy=False), body_tau * 5.0, new_phase

    if model == "clave":
        f0 = 1800.0 * (2.0 ** ((pitch - 0.5) * 0.8))
        body_tau = 0.008 + 0.035 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        sig = np.sin(phases) * env * np.float32(0.20 * vel)
        return sig.astype(np.float32, copy=False), body_tau * 6.0, new_phase

    # Fallback: short closed hat
    noise_tau = 0.02 + 0.08 * decay
    noise_env = np.exp(-t / np.float32(noise_tau))
    sig = (white - noise * 0.8) * noise_env * np.float32(0.15 * vel)
    return sig.astype(np.float32, copy=False), noise_tau * 5.0, phase


@dataclass
class Voice:
    note: int
    velocity: float
    phase: float = 0.0  # 0 .. TABLE_SIZE
    releasing: bool = False
    amp: float = 0.0
    target_amp: float = 0.0
    age: int = 0  # bump on each note_on for steal ordering
    # None → live global morph table; ndarray → locked pad timbre (multi-timbre)
    timbre: Optional[np.ndarray] = None
    # FX slot key: wavetable name for live morph endpoint, or locked pad morph_a
    fx_name: Optional[str] = None
    # None → live global vibrato; (depth semitones, Hz, amount) → phrase-pad bake
    vib: Optional[Tuple[float, float, float]] = None
    vib_phase: float = 0.0


@dataclass
class DrumHit:
    """One-shot analog-style drum voice (Synsonics / TR-ish, not a pitched key)."""

    note: int
    model: str
    velocity: float
    age: int
    # Params frozen at trigger so mid-hit knob twists don't glitch the tail
    pitch: float  # 0..1 tune
    decay: float  # 0..1 stretch / body length
    noise: float  # 0..1 noise amount
    tone: float  # 0..1 noise brightness
    phase: float = 0.0
    pos: int = 0  # samples since trigger
    noise_state: float = 0.0  # cheap LP noise filter memory
    amp_scale: float = 1.0  # live aftertouch trim


class SineEngine:
    """Wavetable keys + procedural ch10 drum voices — light enough for Pi 2."""

    ATTACK_SEC_MIN = 0.002
    ATTACK_SEC_MAX = 0.400
    RELEASE_SEC_MIN = 0.010
    RELEASE_SEC_MAX = 0.800
    MAX_DRUM_HITS = 8  # enough for grooves; 16 + keys blew the Pi audio budget
    MAX_LOCKED_TIMBRES = 4  # concurrent locked pad tables (Pi 2 budget)

    def __init__(
        self,
        tables: Dict[str, np.ndarray],
        sample_rate: int = SAMPLE_RATE,
        max_voices: int = DEFAULT_MAX_VOICES,
    ) -> None:
        if not tables:
            raise ValueError("no wavetables loaded")
        self.sample_rate = sample_rate
        self.max_voices = max(1, int(max_voices))
        self._lock = threading.Lock()
        self._voices: Dict[Tuple[int, int], Voice] = {}
        self._drums: Dict[int, DrumHit] = {}  # note → active one-shot
        self._stream: Optional[sd.OutputStream] = None
        self._bend_semitones = 0.0
        self._bend_range = 2.0
        self._mod = 0.0
        # Screen-set vibrato amount. The mod wheel still works; whichever is
        # higher wins, so touch control doesn't need a wheel to be audible.
        self._vib_always = 0.0
        self._vib_hz = 5.0
        self._vib_depth_semis = 0.5
        self._vib_phase = 0.0
        self._tables = tables
        self._voice_names = list(tables.keys())
        self._table_list = [tables[n] for n in self._voice_names]
        self._waveform = self._voice_names[0]
        # Morph is always between a chosen pair (A ↔ B), not the whole stack.
        self._morph_a = 0
        self._morph_b = 1 if len(self._voice_names) > 1 else 0
        self._morph = 0.0  # 0 = pure A, 1 = pure B
        self._morph_table = self._table_list[0].copy()
        self._morph_dirty = False
        self._tone = 1.0  # 0=dark .. 1=bright (open)
        self._synth_level = 1.0
        self._drum_level = 1.0
        self._attack_sec = 0.012
        self._release_sec = 0.030
        self._filter_state = 0.0
        self._scratch = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._phase_buf = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._frac_buf = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._arange = np.arange(BLOCKSIZE * 2, dtype=np.float32)
        self._ramp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._note_serial = 0
        # Drum pad gate: aftertouch may only change volume while the pad note is held.
        self._drum_gate: Dict[int, bool] = {}
        # Synsonics-ish drum macros (0..1)
        self._drum_pitch = 0.45
        self._drum_decay = 0.40  # "stretch"
        self._drum_noise = 0.55
        self._drum_tone = 0.60
        self._drum_mode = False  # if True, knobs 1–4 edit drums instead of morph/tone
        self._fx_mode = False  # if True, knobs edit per-voice / per-drum insert FX
        self._bus_fx_mode = False  # if True, knobs edit the master mix-bus FX
        # Per wavetable name / per drum model — not a global master bus
        self._voice_fx: Dict[str, MixBusFx] = {}
        self._drum_fx: Dict[str, MixBusFx] = {}
        # Shared wet on the whole kit (after per-model inserts, before keys sum)
        self._drum_group_fx = MixBusFx(self.sample_rate)
        # Optional global wet after keys+drums are summed (separate from inserts)
        self._bus_fx = MixBusFx(self.sample_rate)
        self._fx_edit_kind = "voice"  # voice | drum | drums | bus
        self._fx_edit_drum = "kick"
        self._key_bus = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._drum_bus = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._fx_tmp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        # Pre-baked noise for drums (np.random every block was a major xrun source)
        self._noise_ring = (
            np.random.RandomState(0xC0FFEE).rand(65536).astype(np.float32) * np.float32(2.0)
            - np.float32(1.0)
        )
        self._noise_pos = 0
        self._noise_wrap = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._noise_soft = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        # Reused per-callback accumulators keyed by wavetable / drum model name
        self._voice_fx_buckets: Dict[str, np.ndarray] = {}
        self._rebuild_morph_table_unlocked()

    @property
    def voice_names(self) -> List[str]:
        return list(self._voice_names)

    def _rebuild_morph_table_unlocked(self) -> None:
        n = len(self._table_list)
        ia = max(0, min(n - 1, self._morph_a))
        ib = max(0, min(n - 1, self._morph_b))
        self._morph_a, self._morph_b = ia, ib
        frac = max(0.0, min(1.0, self._morph))
        a = self._table_list[ia]
        b = self._table_list[ib]
        # (1-frac)*A + frac*B — one blended oscillator table for the whole block
        np.multiply(a, np.float32(1.0 - frac), out=self._morph_table)
        self._morph_table += b * np.float32(frac)
        self._waveform = self._voice_names[ia if frac < 0.5 else ib]
        self._morph_dirty = False

    def morph_neighbors(self) -> Tuple[str, str, float]:
        """Return (voice_a, voice_b, blend_frac 0..1)."""
        with self._lock:
            a = self._voice_names[self._morph_a]
            b = self._voice_names[self._morph_b]
            return a, b, max(0.0, min(1.0, self._morph))

    def morph_pair_indices(self) -> Tuple[int, int]:
        with self._lock:
            return self._morph_a, self._morph_b

    def start(self) -> None:
        if self._stream is not None:
            return
        try:
            sd.default.latency = LATENCY_SEC
        except Exception:
            pass

        device: Optional[int] = None
        try:
            for i, d in enumerate(sd.query_devices()):
                if int(d.get("max_output_channels", 0)) <= 0:
                    continue
                name = str(d.get("name", "")).lower()
                if "hdmi" in name or "vc4" in name or "mai pcm" in name:
                    continue
                if "headphone" in name or "bcm2835" in name or "analog" in name:
                    device = i
                    break
        except Exception:
            device = None

        try:
            qdev = device if device is not None else sd.default.device[1]
            if qdev is not None and int(qdev) >= 0:
                info = sd.query_devices(int(qdev), "output")
                native = int(info.get("default_samplerate") or SAMPLE_RATE)
                if native in (44100, 48000):
                    self.sample_rate = native
        except Exception:
            pass

        # Keep FX buffers matched to the actual device rate
        with self._lock:
            need = (
                self._bus_fx.sample_rate != self.sample_rate
                or self._drum_group_fx.sample_rate != self.sample_rate
                or any(
                    fx.sample_rate != self.sample_rate
                    for fx in list(self._voice_fx.values()) + list(self._drum_fx.values())
                )
            )
            if need:
                self._bus_fx = self._clone_fx(self._bus_fx, self.sample_rate)
                self._drum_group_fx = self._clone_fx(self._drum_group_fx, self.sample_rate)
                self._voice_fx = {
                    k: self._clone_fx(v, self.sample_rate) for k, v in self._voice_fx.items()
                }
                self._drum_fx = {
                    k: self._clone_fx(v, self.sample_rate) for k, v in self._drum_fx.items()
                }

        kwargs = dict(
            samplerate=self.sample_rate,
            channels=1,
            dtype="float32",
            blocksize=BLOCKSIZE,
            latency=LATENCY_SEC,
            callback=self._callback,
        )
        if device is not None:
            kwargs["device"] = device
        try:
            self._stream = sd.OutputStream(**kwargs)
        except Exception as exc:
            print(f"audio open failed ({exc}); retrying default device", flush=True)
            kwargs.pop("device", None)
            self._stream = sd.OutputStream(**kwargs)
        self._stream.start()
        print(
            f"audio: wavetable sr={self.sample_rate} block={BLOCKSIZE} "
            f"latency={LATENCY_SEC}s voices<={self.max_voices} "
            f"tables={len(self._tables)} "
            f"device={getattr(self._stream, 'device', None)}",
            flush=True,
        )

    def stop(self) -> None:
        if self._stream is not None:
            self._stream.stop()
            self._stream.close()
            self._stream = None
        with self._lock:
            self._voices.clear()

    def _steal_key(self) -> Optional[Tuple[int, int]]:
        """Pick a voice to drop: releasing/quiet/oldest first. Never None if non-empty."""
        best: Optional[Tuple[int, int]] = None
        best_score: Optional[Tuple[int, float, int]] = None
        for key, v in self._voices.items():
            # Lower tuple wins. Prefer releasing, then quieter, then older.
            score = (0 if v.releasing else 1, v.amp, v.age)
            if best_score is None or score < best_score:
                best_score = score
                best = key
        return best

    def note_on(
        self,
        channel: int,
        note: int,
        velocity: int,
        *,
        timbre: Optional[np.ndarray] = None,
        fx_name: Optional[str] = None,
        vib: Optional[Tuple[float, float, float]] = None,
    ) -> None:
        if velocity <= 0:
            self.note_off(channel, note)
            return
        ch = channel & 0x0F
        n = note & 0x7F
        vel = velocity / 127.0
        if ch == DRUM_CHANNEL:
            self._drum_note_on(n, vel)
            return
        key = (ch, n)
        target = vel * VOICE_AMP
        with self._lock:
            if fx_name is None:
                # Live keys: FX follows the nearer morph endpoint voice name
                if self._morph_dirty:
                    self._rebuild_morph_table_unlocked()
                ia, ib = self._morph_a, self._morph_b
                near = ia if self._morph < 0.5 else ib
                fx_name = self._voice_names[near]
            self._note_serial += 1
            serial = self._note_serial
            existing = self._voices.get(key)
            if existing is not None:
                # Same key re-trigger: reuse slot, restart envelope/phase
                existing.note = n
                existing.velocity = vel
                existing.phase = 0.0
                existing.releasing = False
                existing.amp = 0.0
                existing.target_amp = target
                existing.age = serial
                existing.timbre = timbre
                existing.fx_name = fx_name
                existing.vib = vib
                existing.vib_phase = 0.0
                return
            if len(self._voices) >= self.max_voices:
                drop = self._steal_key()
                if drop is not None:
                    del self._voices[drop]
            self._voices[key] = Voice(
                note=n,
                velocity=vel,
                phase=0.0,
                releasing=False,
                amp=0.0,
                target_amp=target,
                age=serial,
                timbre=timbre,
                fx_name=fx_name,
                vib=vib,
            )

    def _drum_note_on(self, note: int, velocity: float) -> None:
        with self._lock:
            self._note_serial += 1
            serial = self._note_serial
            self._drum_gate[note] = True
            if len(self._drums) >= self.MAX_DRUM_HITS and note not in self._drums:
                oldest = min(self._drums.values(), key=lambda h: h.age)
                self._drums.pop(oldest.note, None)
            self._drums[note] = DrumHit(
                note=note,
                model=drum_model_for_note(note),
                velocity=max(0.05, min(1.0, velocity)),
                age=serial,
                pitch=self._drum_pitch,
                decay=self._drum_decay,
                noise=self._drum_noise,
                tone=self._drum_tone,
                phase=0.0,
                pos=0,
                noise_state=0.0,
                amp_scale=1.0,
            )

    def note_off(self, channel: int, note: int) -> None:
        key = (channel & 0x0F, note & 0x7F)
        with self._lock:
            if (channel & 0x0F) == DRUM_CHANNEL:
                self._drum_gate[key[1]] = False
                # One-shots keep decaying; open hat shortens if pad released early
                hit = self._drums.get(key[1])
                if hit is not None and hit.model == "hat_open":
                    hit.decay *= 0.35
                return
            v = self._voices.get(key)
            if v is None:
                return
            v.releasing = True
            v.target_amp = 0.0

    def all_notes_off(self) -> None:
        with self._lock:
            self._drum_gate.clear()
            self._drums.clear()
            for v in self._voices.values():
                v.releasing = True
                v.target_amp = 0.0

    def set_pitch_bend(self, pitch: int) -> None:
        with self._lock:
            self._bend_semitones = (pitch / 8192.0) * self._bend_range

    def set_mod_wheel(self, value: int) -> None:
        with self._lock:
            self._mod = max(0.0, min(1.0, value / 127.0))

    def set_morph(self, value: float) -> None:
        """Blend A→B: 0..1 (or MIDI 0..127 if > 1)."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._morph = max(0.0, min(1.0, float(value)))
            self._morph_dirty = True

    def set_morph_pair(self, index_a: int, index_b: int, *, morph: Optional[float] = None) -> None:
        """Choose the two voices Knob 1 morphs between."""
        with self._lock:
            n = len(self._voice_names)
            self._morph_a = max(0, min(n - 1, int(index_a)))
            self._morph_b = max(0, min(n - 1, int(index_b)))
            if morph is not None:
                self._morph = max(0.0, min(1.0, float(morph)))
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()

    def set_morph_endpoint(self, which: str, index: int) -> None:
        """Set A or B without changing the other side."""
        which = which.lower().strip()
        with self._lock:
            n = len(self._voice_names)
            idx = max(0, min(n - 1, int(index)))
            if which == "b":
                self._morph_b = idx
            else:
                self._morph_a = idx
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()

    def set_morph_index(self, index: int) -> None:
        """PREV/NEXT / VOICES: set A to this voice and park morph at pure A."""
        with self._lock:
            n = len(self._voice_names)
            idx = max(0, min(n - 1, int(index)))
            self._morph_a = idx
            self._morph = 0.0
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()

    def set_tone(self, value: float) -> None:
        """Brightness 0..1 (MIDI 0..127 accepted)."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._tone = max(0.0, min(1.0, float(value)))

    @staticmethod
    def _knob_to_level(value: float) -> float:
        """Map MIDI 0..127 (or 0..1) to a usable bus gain.

        Near-linear so mid-knob cuts are obvious (old x**0.65 stayed too loud).
        """
        if value > 1.0:
            x = max(0.0, min(1.0, float(value) / 127.0))
        else:
            x = max(0.0, min(1.0, float(value)))
        return x ** 1.15

    def set_level(self, value: float) -> None:
        """Back-compat alias → synth bus (keys / morph)."""
        self.set_synth_level(value)

    def set_synth_level(self, value: float) -> None:
        """Keys / morph soft-synth bus level (Knob 8 when not in DRUM MODE)."""
        with self._lock:
            self._synth_level = self._knob_to_level(value)

    def set_drum_level(self, value: float) -> None:
        """Channel-10 drum bus level (Knob 8 in DRUM MODE)."""
        with self._lock:
            self._drum_level = self._knob_to_level(value)

    def level(self) -> float:
        """Synth bus level 0..1 — phrase pads bake this in when a voice is LOCKED."""
        return self.synth_level()

    def synth_level(self) -> float:
        with self._lock:
            return float(self._synth_level)

    def drum_level(self) -> float:
        with self._lock:
            return float(self._drum_level)

    def set_attack(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        t = max(0.0, min(1.0, float(value)))
        # Exponential-ish feel: low knob = snappy
        sec = self.ATTACK_SEC_MIN * ((self.ATTACK_SEC_MAX / self.ATTACK_SEC_MIN) ** t)
        with self._lock:
            self._attack_sec = sec

    def set_release(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        t = max(0.0, min(1.0, float(value)))
        sec = self.RELEASE_SEC_MIN * ((self.RELEASE_SEC_MAX / self.RELEASE_SEC_MIN) ** t)
        with self._lock:
            self._release_sec = sec

    def set_vib_depth(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            # 0..2 semitones
            self._vib_depth_semis = max(0.0, min(1.0, float(value))) * 2.0

    VIB_DEPTH_MAX = 2.0  # semitones — matches the knob's top end
    VIB_HZ_MIN = 1.0
    VIB_HZ_MAX = 9.0

    def vib_state(self) -> Tuple[float, float, float]:
        """(depth semitones, rate Hz, always-on amount 0..1) for the touch UI."""
        with self._lock:
            return (
                float(self._vib_depth_semis),
                float(self._vib_hz),
                float(self._vib_always),
            )

    def set_vib_always(self, amount: float) -> float:
        """0 = mod wheel gates vibrato (as before); 1 = always on at set depth."""
        with self._lock:
            self._vib_always = max(0.0, min(1.0, float(amount)))
            return float(self._vib_always)

    def nudge_vib_depth(self, delta_semis: float) -> float:
        with self._lock:
            depth = self._vib_depth_semis + float(delta_semis)
            self._vib_depth_semis = max(0.0, min(self.VIB_DEPTH_MAX, depth))
            return float(self._vib_depth_semis)

    def nudge_vib_rate(self, delta_hz: float) -> float:
        with self._lock:
            hz = self._vib_hz + float(delta_hz)
            self._vib_hz = max(self.VIB_HZ_MIN, min(self.VIB_HZ_MAX, hz))
            return float(self._vib_hz)

    def set_vib_rate(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        t = max(0.0, min(1.0, float(value)))
        # ~1 Hz .. ~9 Hz
        with self._lock:
            self._vib_hz = 1.0 + t * 8.0

    def set_pad_pressure(self, channel: int, note: Optional[int], value: int) -> None:
        """Live volume trim for held drum pads (aftertouch / pressure)."""
        if (channel & 0x0F) != DRUM_CHANNEL:
            return
        scale = max(0.0, min(1.0, value / 127.0))
        with self._lock:
            if note is None:
                notes = [n for n, held in self._drum_gate.items() if held]
            else:
                notes = [note & 0x7F]
            for n in notes:
                hit = self._drums.get(n)
                if value <= 0:
                    self._drum_gate[n] = False
                    if hit is not None:
                        hit.amp_scale = 0.0
                    continue
                if hit is not None:
                    hit.amp_scale = 0.35 + 0.65 * scale

    def drum_knob_focus(self) -> bool:
        """True only in explicit drum mode (DRUM MODE button)."""
        with self._lock:
            return self._drum_mode and not self._fx_mode and not self._bus_fx_mode

    def set_drum_mode(self, enabled: bool) -> None:
        with self._lock:
            self._drum_mode = bool(enabled)
            if self._drum_mode:
                self._fx_mode = False
                self._bus_fx_mode = False

    def drum_mode(self) -> bool:
        with self._lock:
            return self._drum_mode

    def fx_knob_focus(self) -> bool:
        """True when knobs edit insert FX or master bus FX."""
        with self._lock:
            return self._fx_mode or self._bus_fx_mode

    def set_fx_mode(self, enabled: bool) -> None:
        """Per-voice / per-drum insert FX edit mode."""
        with self._lock:
            self._fx_mode = bool(enabled)
            if self._fx_mode:
                self._drum_mode = False
                self._bus_fx_mode = False
                if self._fx_edit_kind == "bus":
                    self._fx_edit_kind = "voice"

    def fx_mode(self) -> bool:
        with self._lock:
            return self._fx_mode

    def toggle_fx_mode(self) -> bool:
        with self._lock:
            nxt = not self._fx_mode
        self.set_fx_mode(nxt)
        return nxt

    def set_bus_fx_mode(self, enabled: bool) -> None:
        """Master mix-bus FX edit mode (whole keys+drums sum)."""
        with self._lock:
            self._bus_fx_mode = bool(enabled)
            if self._bus_fx_mode:
                self._drum_mode = False
                self._fx_mode = False
                self._fx_edit_kind = "bus"

    def bus_fx_mode(self) -> bool:
        with self._lock:
            return self._bus_fx_mode

    def toggle_bus_fx_mode(self) -> bool:
        with self._lock:
            nxt = not self._bus_fx_mode
        self.set_bus_fx_mode(nxt)
        return nxt

    # Back-compat aliases used by older call sites / UI helpers
    def set_drum_lock(self, locked: bool) -> None:
        self.set_drum_mode(locked)

    def drum_lock(self) -> bool:
        return self.drum_mode()

    @staticmethod
    def _clone_fx(src: MixBusFx, sample_rate: int) -> MixBusFx:
        out = MixBusFx(sample_rate)
        out.apply_snapshot(src.snapshot())
        return out

    def _ensure_voice_fx_unlocked(self, name: str) -> MixBusFx:
        key = str(name or "sine").lower().strip() or "sine"
        fx = self._voice_fx.get(key)
        if fx is None:
            fx = MixBusFx(self.sample_rate)
            self._voice_fx[key] = fx
        return fx

    def _ensure_drum_fx_unlocked(self, model: str) -> MixBusFx:
        key = str(model or "kick").lower().strip() or "kick"
        fx = self._drum_fx.get(key)
        if fx is None:
            fx = MixBusFx(self.sample_rate)
            self._drum_fx[key] = fx
        return fx

    def set_fx_edit_voice(self, name: Optional[str] = None) -> None:
        """Point insert-FX knobs at a wavetable slot (default: nearer morph endpoint)."""
        with self._lock:
            self._fx_edit_kind = "voice"
            if name:
                self._ensure_voice_fx_unlocked(str(name))
            else:
                if self._morph_dirty:
                    self._rebuild_morph_table_unlocked()
                near = self._morph_a if self._morph < 0.5 else self._morph_b
                self._ensure_voice_fx_unlocked(self._voice_names[near])

    def set_fx_edit_drum(self, model: str) -> None:
        """Point insert-FX knobs at a drum model insert (kick, snare, …)."""
        with self._lock:
            self._fx_edit_kind = "drum"
            self._fx_edit_drum = str(model or "kick")
            self._ensure_drum_fx_unlocked(self._fx_edit_drum)

    def set_fx_edit_drums(self) -> None:
        """Point insert-FX knobs at the shared all-drums group bus."""
        with self._lock:
            self._fx_edit_kind = "drums"

    def set_fx_edit_bus(self) -> None:
        """Point knobs at the master mix-bus FX."""
        with self._lock:
            self._fx_edit_kind = "bus"

    def fx_edit_kind(self) -> str:
        with self._lock:
            if self._bus_fx_mode:
                return "bus"
            return str(self._fx_edit_kind)

    def fx_edit_label(self) -> str:
        with self._lock:
            if self._fx_edit_kind == "bus" or self._bus_fx_mode:
                return "bus"
            if self._fx_edit_kind == "drums":
                return "drums"
            if self._fx_edit_kind == "drum":
                return f"drum:{self._fx_edit_drum}"
            # Prefer nearer morph endpoint as the voice being sculpted
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            near = self._morph_a if self._morph < 0.5 else self._morph_b
            return f"voice:{self._voice_names[near]}"

    def _fx_edit_slot_unlocked(self) -> MixBusFx:
        if self._fx_edit_kind == "bus" or self._bus_fx_mode:
            return self._bus_fx
        if self._fx_edit_kind == "drums":
            return self._drum_group_fx
        if self._fx_edit_kind == "drum":
            return self._ensure_drum_fx_unlocked(self._fx_edit_drum)
        if self._morph_dirty:
            self._rebuild_morph_table_unlocked()
        near = self._morph_a if self._morph < 0.5 else self._morph_b
        return self._ensure_voice_fx_unlocked(self._voice_names[near])

    def _set_fx_param(self, attr: str, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        value = max(0.0, min(1.0, float(value)))
        with self._lock:
            slot = self._fx_edit_slot_unlocked()
            setattr(slot, attr, value)

    def set_fx_drive(self, value: float) -> None:
        self._set_fx_param("drive", value)

    def set_fx_delay_time(self, value: float) -> None:
        self._set_fx_param("delay_time", value)

    def set_fx_delay_fb(self, value: float) -> None:
        self._set_fx_param("delay_fb", value)

    def set_fx_delay_mix(self, value: float) -> None:
        self._set_fx_param("delay_mix", value)

    def set_fx_reverb_size(self, value: float) -> None:
        self._set_fx_param("reverb_size", value)

    def set_fx_reverb_mix(self, value: float) -> None:
        self._set_fx_param("reverb_mix", value)

    def fx_edit_snapshot(self) -> Dict[str, float]:
        with self._lock:
            return self._fx_edit_slot_unlocked().snapshot()

    def set_drum_pitch(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_pitch = max(0.0, min(1.0, float(value)))

    def set_drum_decay(self, value: float) -> None:
        """Stretch / body length."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_decay = max(0.0, min(1.0, float(value)))

    def set_drum_noise(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_noise = max(0.0, min(1.0, float(value)))

    def set_drum_tone(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_tone = max(0.0, min(1.0, float(value)))

    def set_waveform(self, name: str) -> bool:
        name = name.lower().strip()
        if name not in self._tables:
            return False
        try:
            idx = self._voice_names.index(name)
        except ValueError:
            return False
        self.set_morph_index(idx)
        return True

    def waveform(self) -> str:
        with self._lock:
            return self._waveform

    def morph(self) -> float:
        with self._lock:
            return self._morph

    def snapshot_morph(self) -> Tuple[str, str, float]:
        """Current morph pair names + blend — for locking onto a phrase pad."""
        with self._lock:
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            a = self._voice_names[self._morph_a]
            b = self._voice_names[self._morph_b]
            return a, b, float(self._morph)

    def bake_morph_table(
        self, name_a: str, name_b: str, morph: float
    ) -> Optional[np.ndarray]:
        """Build a frozen wavetable blend for a locked pad timbre."""
        names = {n: i for i, n in enumerate(self._voice_names)}
        ia = names.get(str(name_a).lower().strip())
        ib = names.get(str(name_b).lower().strip())
        if ia is None and ib is None:
            return None
        if ia is None:
            ia = ib if ib is not None else 0
        if ib is None:
            ib = ia
        frac = max(0.0, min(1.0, float(morph)))
        a = self._table_list[ia]
        b = self._table_list[ib]
        out = (a * np.float32(1.0 - frac) + b * np.float32(frac)).astype(np.float32)
        return out

    def morph_cycle_copy(self) -> np.ndarray:
        """Snapshot of the live morph wavetable (one cycle) for the scope."""
        with self._lock:
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            return np.copy(self._morph_table)

    def bake_voice_cycle(
        self, *, apply_drive: bool = True, apply_tone: bool = True
    ) -> np.ndarray:
        """
        Freeze the live morph into a new single-cycle wavetable shape.

        Bakes what can become wave shape:
          - morph blend
          - nearer voice's drive (waveshape)
          - tone / brightness (static spectral shape on the cycle)

        Delay and reverb are time-domain — they cannot live in one cycle and are
        left as live FX (not written into the saved wave).
        """
        with self._lock:
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            out = np.copy(self._morph_table)
            near = self._morph_a if self._morph < 0.5 else self._morph_b
            src_name = self._voice_names[near]

            if apply_drive:
                fx = self._voice_fx.get(src_name)
                drive = float(fx.drive) if fx is not None else 0.0
                if drive > 0.001:
                    amount = 1.0 + drive * 12.0
                    tmp = np.tanh(out * np.float32(amount))
                    norm = math.tanh(amount) if amount > 1e-6 else 1.0
                    out = (tmp * np.float32(1.0 / max(0.25, norm))).astype(np.float32)

            if apply_tone:
                tone = float(self._tone)
                if tone < 0.999:
                    win = max(1, int(round((1.0 - tone) * 48.0)))
                    if win > 1:
                        out = circular_moving_average(out, win)

            peak = float(np.max(np.abs(out))) or 1.0
            out = (out / np.float32(peak)) * np.float32(TABLE_PEAK)
            return out

    def current_voice_fx_source(self) -> str:
        """Wavetable name whose insert FX is 'on' the current morph sound."""
        with self._lock:
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            near = self._morph_a if self._morph < 0.5 else self._morph_b
            return self._voice_names[near]

    def save_current_voice(self, name: str) -> Tuple[str, np.ndarray, Dict[str, float]]:
        """
        Bake morph + drive + tone into a new wavetable and select it (A=B).

        Returns (key, cycle, delay/reverb sidecar). Drive stays 0 on the new
        insert (already in the wave); delay/reverb ride along as numbers.
        """
        source = self.current_voice_fx_source()
        with self._lock:
            src_fx = self._ensure_voice_fx_unlocked(source).snapshot()
        sidecar = {k: float(src_fx.get(k, 0.0)) for k in VOICE_FX_SIDECAR_KEYS}
        cycle = self.bake_voice_cycle(apply_drive=True, apply_tone=True)
        key = self.add_wavetable(name, cycle)
        with self._lock:
            fx = self._ensure_voice_fx_unlocked(key)
            fx.drive = 0.0  # baked into wave
            fx.apply_snapshot(sidecar)
            self._fx_edit_kind = "voice"
        return key, cycle, sidecar

    def apply_voice_fx_sidecar(self, name: str, fx: Dict[str, float]) -> None:
        """Restore delay/reverb for a user voice; keep drive at 0 (in the wave)."""
        key = sanitize_voice_name(name)
        if not key or key in BUILTIN_VOICE_NAMES:
            return
        with self._lock:
            slot = self._ensure_voice_fx_unlocked(key)
            slot.drive = 0.0
            slot.apply_snapshot(fx)

    def add_wavetable(self, name: str, table: np.ndarray) -> str:
        """
        Hot-register a single-cycle table under `name` and select it as the
        current pure morph voice (A=B, morph=0).
        """
        key = sanitize_voice_name(name)
        if key in BUILTIN_VOICE_NAMES:
            raise ValueError(f"cannot replace built-in voice '{key}'")
        arr = np.asarray(table, dtype=np.float32).reshape(-1)
        if arr.shape[0] != TABLE_SIZE:
            arr = _resample_cycle(arr)
        else:
            peak = float(np.max(np.abs(arr))) or 1.0
            arr = (arr / np.float32(peak)) * np.float32(TABLE_PEAK)
        with self._lock:
            if key in self._tables:
                idx = self._voice_names.index(key)
                self._tables[key] = arr
                self._table_list[idx] = arr
            else:
                self._tables[key] = arr
                self._voice_names.append(key)
                self._table_list.append(arr)
            idx = self._voice_names.index(key)
            self._morph_a = idx
            self._morph_b = idx
            self._morph = 0.0
            self._waveform = key
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()
        return key

    def suggested_save_voice_name(self) -> str:
        a, b, blend = self.morph_neighbors()
        return unique_voice_name(
            suggest_voice_name(a, b, blend), self.voice_names
        )

    def drum_macros(self) -> Tuple[float, float, float, float]:
        """pitch, decay(stretch), noise, tone — current drum-edit macros."""
        with self._lock:
            return (
                float(self._drum_pitch),
                float(self._drum_decay),
                float(self._drum_noise),
                float(self._drum_tone),
            )

    def preview_drum_waveform(self, model: str) -> np.ndarray:
        """Render a stable offline preview of a kit voice with live macros."""
        pitch, decay, noise, tone = self.drum_macros()
        return render_drum_preview(
            model,
            pitch=pitch,
            decay=decay,
            noise_amt=noise,
            tone=tone,
            sample_rate=self.sample_rate,
        )

    def modulation_state(self) -> Dict[str, float]:
        with self._lock:
            fx = self._fx_edit_slot_unlocked().snapshot()
            return {
                "bend": self._bend_semitones,
                "mod": self._mod,
                "morph": self._morph,
                "tone": self._tone,
                "level": self._synth_level,  # alias for older UI / logs
                "synth_level": self._synth_level,
                "drum_level": self._drum_level,
                "attack": self._attack_sec,
                "release": self._release_sec,
                "vib_hz": self._vib_hz,
                "vib_depth": self._vib_depth_semis,
                "vib_always": self._vib_always,
                "drum_pitch": self._drum_pitch,
                "drum_decay": self._drum_decay,
                "drum_noise": self._drum_noise,
                "drum_tone": self._drum_tone,
                "drum_mode": 1.0 if self._drum_mode else 0.0,
                "fx_mode": 1.0 if self._fx_mode else 0.0,
                "bus_fx_mode": 1.0 if self._bus_fx_mode else 0.0,
                **fx,
            }

    def snapshot_settings(self) -> Dict[str, Any]:
        """Serialize synth sound settings for JSON presets / session restore."""
        with self._lock:
            out: Dict[str, Any] = {
                "morph_a": self._voice_names[self._morph_a],
                "morph_b": self._voice_names[self._morph_b],
                "morph": float(self._morph),
                "tone": float(self._tone),
                "synth_level": float(self._synth_level),
                "drum_level": float(self._drum_level),
                # legacy key — same as synth_level
                "level": float(self._synth_level),
                "attack_sec": float(self._attack_sec),
                "release_sec": float(self._release_sec),
                "vib_hz": float(self._vib_hz),
                "vib_depth": float(self._vib_depth_semis),
                "vib_always": float(self._vib_always),
                "drum_pitch": float(self._drum_pitch),
                "drum_decay": float(self._drum_decay),
                "drum_noise": float(self._drum_noise),
                "drum_tone": float(self._drum_tone),
                # Per-instrument inserts + kit group + optional master bus
                "voice_fx": {k: v.snapshot() for k, v in self._voice_fx.items()},
                "drum_fx": {k: v.snapshot() for k, v in self._drum_fx.items()},
                "drum_group_fx": self._drum_group_fx.snapshot(),
                "bus_fx": self._bus_fx.snapshot(),
                # drum_mode / fx_mode / bus_fx_mode are session UI toggles only
            }
            return out

    def apply_settings(self, data: Dict[str, Any]) -> None:
        """Restore synth sound settings from snapshot_settings() / preset JSON."""
        names = {n: i for i, n in enumerate(self._voice_names)}
        with self._lock:
            a_name = str(data.get("morph_a", self._voice_names[self._morph_a]))
            b_name = str(data.get("morph_b", self._voice_names[self._morph_b]))
            self._morph_a = names.get(a_name, self._morph_a)
            self._morph_b = names.get(b_name, self._morph_b)
            if "morph" in data:
                self._morph = max(0.0, min(1.0, float(data["morph"])))
            if "tone" in data:
                self._tone = max(0.0, min(1.0, float(data["tone"])))
            if "synth_level" in data:
                self._synth_level = max(0.0, min(1.0, float(data["synth_level"])))
            elif "level" in data:
                self._synth_level = max(0.0, min(1.0, float(data["level"])))
            if "drum_level" in data:
                self._drum_level = max(0.0, min(1.0, float(data["drum_level"])))
            if "attack_sec" in data:
                self._attack_sec = max(self.ATTACK_SEC_MIN, min(self.ATTACK_SEC_MAX, float(data["attack_sec"])))
            if "release_sec" in data:
                self._release_sec = max(
                    self.RELEASE_SEC_MIN, min(self.RELEASE_SEC_MAX, float(data["release_sec"]))
                )
            if "vib_hz" in data:
                self._vib_hz = max(0.1, min(20.0, float(data["vib_hz"])))
            if "vib_depth" in data:
                self._vib_depth_semis = max(0.0, min(4.0, float(data["vib_depth"])))
            if "vib_always" in data:
                self._vib_always = max(0.0, min(1.0, float(data["vib_always"])))
            if "drum_pitch" in data:
                self._drum_pitch = max(0.0, min(1.0, float(data["drum_pitch"])))
            if "drum_decay" in data:
                self._drum_decay = max(0.0, min(1.0, float(data["drum_decay"])))
            if "drum_noise" in data:
                self._drum_noise = max(0.0, min(1.0, float(data["drum_noise"])))
            if "drum_tone" in data:
                self._drum_tone = max(0.0, min(1.0, float(data["drum_tone"])))
            # Per-instrument inserts
            vfx = data.get("voice_fx")
            if isinstance(vfx, dict):
                for name, snap in vfx.items():
                    if isinstance(snap, dict):
                        self._ensure_voice_fx_unlocked(str(name)).apply_snapshot(snap)
            dfx = data.get("drum_fx")
            if isinstance(dfx, dict):
                for model, snap in dfx.items():
                    if isinstance(snap, dict):
                        self._ensure_drum_fx_unlocked(str(model)).apply_snapshot(snap)
            dgfx = data.get("drum_group_fx")
            if isinstance(dgfx, dict):
                self._drum_group_fx.apply_snapshot(dgfx)
            # Master bus FX (explicit map, or legacy flat fx_* when no maps existed)
            bfx = data.get("bus_fx")
            if isinstance(bfx, dict):
                self._bus_fx.apply_snapshot(bfx)
            elif any(k.startswith("fx_") for k in data.keys()) and not isinstance(vfx, dict):
                # Early mix-bus experiment stored flat fx_* on the master
                self._bus_fx.apply_snapshot(data)
            # Modes are session-only; always restore knobs to morph
            self._drum_mode = False
            self._fx_mode = False
            self._bus_fx_mode = False
            self._fx_edit_kind = "voice"
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()

    def reset_to_factory_defaults(self) -> None:
        """Hardcoded init sound: morph/tone/env/drums/FX — ignores settings.json / presets."""
        with self._lock:
            self._voices.clear()
            self._drums.clear()
            self._drum_gate.clear()
            self._bend_semitones = 0.0
            self._mod = 0.0
            self._vib_hz = 5.0
            self._vib_depth_semis = 0.5
            self._vib_phase = 0.0
            self._vib_always = 0.0
            self._morph_a = 0
            self._morph_b = 1 if len(self._voice_names) > 1 else 0
            self._morph = 0.0
            self._waveform = self._voice_names[self._morph_a]
            self._tone = 1.0
            self._synth_level = 1.0
            self._drum_level = 1.0
            self._attack_sec = 0.012
            self._release_sec = 0.030
            self._filter_state = 0.0
            self._drum_pitch = 0.45
            self._drum_decay = 0.40
            self._drum_noise = 0.55
            self._drum_tone = 0.60
            self._drum_mode = False
            self._fx_mode = False
            self._bus_fx_mode = False
            self._fx_edit_kind = "voice"
            self._fx_edit_drum = "kick"
            self._voice_fx.clear()
            self._drum_fx.clear()
            self._drum_group_fx.reset_to_defaults()
            self._bus_fx.reset_to_defaults()
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()

    def _callback(self, outdata, frames, time_info, status) -> None:  # noqa: ANN001
        del time_info
        if status:
            # PortAudio underrun/overflow — log sparsely (audio thread)
            now = time.monotonic()
            last = getattr(self, "_last_xrun_log", 0.0)
            if now - last >= 1.0:
                self._last_xrun_log = now
                print(f"audio: xrun {status}", flush=True)
        if frames > self._scratch.shape[0]:
            self._scratch = np.zeros(frames, dtype=np.float32)
            self._phase_buf = np.zeros(frames, dtype=np.float32)
            self._frac_buf = np.zeros(frames, dtype=np.float32)
            self._arange = np.arange(frames, dtype=np.float32)
            self._ramp = np.zeros(frames, dtype=np.float32)
        buf = self._scratch[:frames]
        buf.fill(0.0)
        ph = self._phase_buf[:frames]
        frac = self._frac_buf[:frames]
        arange = self._arange[:frames]
        ramp = self._ramp[:frames]
        sr = float(self.sample_rate)

        with self._lock:
            if self._morph_dirty:
                self._rebuild_morph_table_unlocked()
            items = list(self._voices.items())
            drum_hits = list(self._drums.values())
            bend = self._bend_semitones
            # Wheel or screen — whichever asks for more vibrato
            mod = max(self._mod, self._vib_always)
            vib_hz = self._vib_hz
            vib_depth = self._vib_depth_semis
            table = self._morph_table
            tone = self._tone
            synth_level = self._synth_level
            drum_level = self._drum_level
            attack_sec = self._attack_sec
            release_sec = self._release_sec
            # Live drum macros (knobs must affect ringing hits, not only the next one)
            drum_pitch = self._drum_pitch
            drum_decay = self._drum_decay
            drum_noise = self._drum_noise
            drum_tone = self._drum_tone

        attack_per_samp = 1.0 / max(1.0, attack_sec * sr)
        release_per_samp = 1.0 / max(1.0, release_sec * sr)

        vib_semis = 0.0
        if mod > 0.01 and vib_depth > 0.001:
            self._vib_phase += 2.0 * math.pi * vib_hz * (frames / sr)
            if self._vib_phase > 2.0 * math.pi:
                self._vib_phase %= 2.0 * math.pi
            vib_semis = vib_depth * mod * math.sin(self._vib_phase)
        block_turns = frames / sr

        if frames > self._key_bus.shape[0]:
            self._key_bus = np.zeros(frames, dtype=np.float32)
            self._drum_bus = np.zeros(frames, dtype=np.float32)
            self._fx_tmp = np.zeros(frames, dtype=np.float32)
        key_bus = self._key_bus[:frames]
        drum_bus = self._drum_bus[:frames]
        tmp = self._fx_tmp[:frames]
        key_bus.fill(0.0)
        drum_bus.fill(0.0)

        # Group key voices by FX slot (wavetable / locked morph_a name)
        groups: Dict[str, np.ndarray] = {}
        dead: List[Tuple[int, int]] = []
        denom = np.float32(max(frames - 1, 1))
        for key, v in items:
            if v.vib is None:
                semis = vib_semis
            else:
                # Phrase pad with its own vibrato baked in at record time
                v_depth, v_hz, v_amount = v.vib
                if v_amount > 0.01 and v_depth > 0.001:
                    v.vib_phase += 2.0 * math.pi * v_hz * block_turns
                    if v.vib_phase > 2.0 * math.pi:
                        v.vib_phase %= 2.0 * math.pi
                    semis = v_depth * v_amount * math.sin(v.vib_phase)
                else:
                    semis = 0.0
            hz = midi_to_hz(v.note) * (2.0 ** ((bend + semis) / 12.0))
            phase_inc = (hz * TABLE_SIZE) / sr
            np.add(v.phase, arange * np.float32(phase_inc), out=ph)
            # Linear interpolation (nicer for sampled AKWF cycles)
            np.subtract(ph, np.floor(ph), out=frac)
            i0 = np.bitwise_and(ph.astype(np.int32), TABLE_MASK)
            i1 = np.bitwise_and(i0 + 1, TABLE_MASK)
            src = v.timbre if v.timbre is not None else table
            wave = src[i0] * (1.0 - frac) + src[i1] * frac

            start_amp = v.amp
            if v.releasing:
                end_amp = max(0.0, start_amp - release_per_samp * frames * max(start_amp, 1e-4))
            elif v.target_amp > start_amp:
                end_amp = min(
                    v.target_amp,
                    start_amp + attack_per_samp * frames * max(v.target_amp, 0.05),
                )
            elif v.target_amp < start_amp:
                end_amp = max(
                    v.target_amp,
                    start_amp - release_per_samp * frames * max(start_amp, 0.05),
                )
            else:
                end_amp = start_amp
            np.multiply(arange, np.float32((end_amp - start_amp) / float(denom)), out=ramp)
            np.add(ramp, np.float32(start_amp), out=ramp)
            np.multiply(wave, ramp, out=tmp)
            fx_key = (v.fx_name or "sine").lower().strip() or "sine"
            bucket = groups.get(fx_key)
            if bucket is None:
                pool = self._voice_fx_buckets.get(fx_key)
                if pool is None or pool.shape[0] < frames:
                    pool = np.zeros(frames, dtype=np.float32)
                    self._voice_fx_buckets[fx_key] = pool
                bucket = pool[:frames]
                bucket.fill(0.0)
                groups[fx_key] = bucket
            bucket += tmp
            v.amp = float(end_amp)
            v.phase = float((v.phase + phase_inc * frames) % TABLE_SIZE)
            if v.releasing and v.amp < 0.0005:
                dead.append(key)

        # Apply per-wavetable FX, then sum onto key bus
        with self._lock:
            for fx_key, bucket in groups.items():
                fx = self._ensure_voice_fx_unlocked(fx_key)
                if not fx.is_dry():
                    fx.process(bucket)
                key_bus += bucket

        # Procedural ch10 drums — per-model FX only when a slot is actually wet
        drum_fx_wet = False
        if drum_hits:
            models = {str(h.model) for h in drum_hits}
            with self._lock:
                drum_fx_wet = any(
                    not self._ensure_drum_fx_unlocked(m).is_dry() for m in models
                )
        dead_drums = self._render_drums(
            drum_bus,
            frames,
            sr,
            drum_hits,
            arange,
            pitch=drum_pitch,
            decay=drum_decay,
            noise_amt=drum_noise,
            tone=drum_tone,
            apply_model_fx=drum_fx_wet,
        )
        if frames > 0:
            with self._lock:
                drum_group_fx = self._drum_group_fx
            if not drum_group_fx.is_dry():
                drum_group_fx.process(drum_bus)

        # Tone filter on keys only (drums keep their own tone macro).
        # Cheap O(n) brightness: blend dry with a 2-tap blur (no convolve).
        if tone < 0.999 and frames > 0:
            blend = float(tone * tone)
            soft = np.empty_like(key_bus)
            soft[0] = 0.5 * (key_bus[0] + np.float32(self._filter_state))
            soft[1:] = 0.5 * (key_bus[1:] + key_bus[:-1])
            if blend <= 0.001:
                key_bus[:] = soft
            else:
                key_bus *= np.float32(blend)
                key_bus += soft * np.float32(1.0 - blend)
            self._filter_state = float(key_bus[-1])
        elif frames > 0:
            self._filter_state = float(key_bus[-1])

        if synth_level < 0.999:
            key_bus *= np.float32(synth_level)
        if drum_level < 0.999:
            drum_bus *= np.float32(drum_level)

        buf[:] = key_bus
        buf += drum_bus

        # Master mix-bus FX (optional global wet — separate from per-voice/per-drum inserts)
        if frames > 0:
            with self._lock:
                bus_fx = self._bus_fx
            if not bus_fx.is_dry():
                bus_fx.process(buf)

        # Makeup + soft limit: loud enough for powered speakers, tame chord pile-ups
        if frames > 0:
            buf *= np.float32(OUTPUT_MAKEUP)
            np.tanh(buf, out=buf)
            buf *= np.float32(0.97)
        outdata[:, 0] = buf
        if dead or dead_drums:
            with self._lock:
                for k in dead:
                    self._voices.pop(k, None)
                for n in dead_drums:
                    self._drums.pop(n, None)

    def _next_noise(self, n: int) -> np.ndarray:
        """Slice a pre-baked noise ring (no per-block RNG allocation)."""
        ring = self._noise_ring
        pos = self._noise_pos
        rlen = len(ring)
        if pos + n <= rlen:
            out = ring[pos : pos + n]
            self._noise_pos = pos + n
            return out
        if n > self._noise_wrap.shape[0]:
            self._noise_wrap = np.zeros(n, dtype=np.float32)
        out = self._noise_wrap[:n]
        first = rlen - pos
        out[:first] = ring[pos:]
        out[first:] = ring[: n - first]
        self._noise_pos = (pos + n) % rlen
        return out

    def _render_drums(
        self,
        buf: np.ndarray,
        frames: int,
        sr: float,
        hits: List[DrumHit],
        arange: np.ndarray,
        *,
        pitch: float,
        decay: float,
        noise_amt: float,
        tone: float,
        apply_model_fx: bool = False,
    ) -> List[int]:
        """Add analog-style drum hits into buf. Returns note keys that finished.

        When apply_model_fx is True, each drum *model* (kick, snare, …) runs
        through its own MixBusFx insert before summing onto buf.
        """
        dead: List[int] = []
        if not hits:
            return dead
        two_pi = 2.0 * math.pi
        inv_sr = 1.0 / sr
        # One shared noise block for all hits this callback
        white = self._next_noise(frames)
        if tone >= 0.92:
            noise = white
        else:
            # Cheap brightness: blend dry noise with a 2-tap blur
            if frames > self._noise_soft.shape[0]:
                self._noise_soft = np.zeros(frames, dtype=np.float32)
            soft = self._noise_soft[:frames]
            soft[0] = 0.5 * (white[0] + np.float32(hits[0].noise_state))
            soft[1:] = 0.5 * (white[1:] + white[:-1])
            blend = float(tone * tone)
            if blend <= 0.001:
                noise = soft
            else:
                noise = white * np.float32(blend) + soft * np.float32(1.0 - blend)
            for hit in hits:
                hit.noise_state = float(noise[-1] if hasattr(noise, "__len__") else soft[-1])

        def _synth_hit(hit: DrumHit) -> Tuple[np.ndarray, float]:
            # Keep hit snapshot in sync so UI/debug stay honest; audio uses live macros
            hit.pitch = pitch
            hit.decay = decay
            hit.noise = noise_amt
            hit.tone = tone
            t = (hit.pos + arange) * np.float32(inv_sr)

            audio, dur, new_phase = synthesize_drum(
                hit.model,
                t=t,
                arange=arange,
                white=white,
                noise=noise,
                pitch=pitch,
                decay=decay,
                noise_amt=noise_amt,
                tone=tone,
                vel=hit.velocity * hit.amp_scale,
                phase=hit.phase,
                two_pi=two_pi,
                inv_sr=inv_sr,
            )
            hit.phase = new_phase
            hit.pos += frames
            if hit.pos > int(dur * sr) or float(np.max(np.abs(audio))) < 0.0002:
                dead.append(hit.note)
            return audio, dur

        if not apply_model_fx:
            for hit in hits:
                audio, _dur = _synth_hit(hit)
                buf += audio * np.float32(DRUM_BUS_GAIN)
            return dead

        # Per drum *model* insert FX (kick ≠ snare ≠ hat, …).
        by_model: Dict[str, List[DrumHit]] = {}
        for hit in hits:
            by_model.setdefault(str(hit.model), []).append(hit)

        scratch = self._fx_tmp[:frames]
        for model, model_hits in by_model.items():
            scratch.fill(0.0)
            for hit in model_hits:
                audio, _dur = _synth_hit(hit)
                scratch += audio * np.float32(DRUM_BUS_GAIN)
            with self._lock:
                fx = self._ensure_drum_fx_unlocked(model)
            if not fx.is_dry():
                fx.process(scratch)
            buf += scratch
        return dead


def phrase_pad_label(cell: int) -> str:
    """Human label for phrase cell 0..15 → A1..A8 / B1..B8."""
    c = max(0, min(PHRASE_PAD_COUNT - 1, int(cell)))
    bank = "A" if c < 8 else "B"
    return f"{bank}{(c % 8) + 1}"


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


def _sec_to_ticks(seconds: float, bpm: float, ticks_per_beat: int) -> int:
    return max(0, int(round(float(seconds) * (float(bpm) / 60.0) * ticks_per_beat)))


def take_events_to_midifile(
    events: List[LoopEvent],
    loop_len: float,
    bpm: float = DEFAULT_SONG_BPM,
    ticks_per_beat: int = 480,
) -> mido.MidiFile:
    """Build a Type 0 SMF from a free-timing take (sequencer or pad)."""
    bpm = max(20.0, min(400.0, float(bpm)))
    mid = mido.MidiFile(type=0, ticks_per_beat=ticks_per_beat)
    track = mido.MidiTrack()
    mid.tracks.append(track)
    track.append(mido.MetaMessage("set_tempo", tempo=mido.bpm2tempo(bpm), time=0))
    # At equal times, note-offs before note-ons (cleaner for chords / legato)
    ordered = sorted(events, key=lambda e: (e.t, 0 if not e.on else 1))
    last_tick = 0
    for ev in ordered:
        tick = _sec_to_ticks(ev.t, bpm, ticks_per_beat)
        delta = max(0, tick - last_tick)
        last_tick = tick
        if ev.on:
            track.append(
                mido.Message(
                    "note_on",
                    channel=ev.channel & 0x0F,
                    note=ev.note & 0x7F,
                    velocity=max(1, min(127, int(ev.velocity))),
                    time=delta,
                )
            )
        else:
            track.append(
                mido.Message(
                    "note_off",
                    channel=ev.channel & 0x0F,
                    note=ev.note & 0x7F,
                    velocity=0,
                    time=delta,
                )
            )
    # Pad to loop length so re-import keeps the gap at the end
    end_tick = _sec_to_ticks(max(loop_len, 0.0), bpm, ticks_per_beat)
    pad = max(0, end_tick - last_tick)
    track.append(mido.MetaMessage("end_of_track", time=pad))
    return mid


def _midifile_native_bpm(mid: mido.MidiFile) -> float:
    for track in mid.tracks:
        for msg in track:
            if msg.type == "set_tempo":
                return float(mido.tempo2bpm(msg.tempo))
    return float(DEFAULT_SONG_BPM)


def pick_song_output_name(prefer_substr: str = "") -> Optional[str]:
    """Choose a USB MIDI out port for DIN playback (never a obvious virtual loopback)."""
    try:
        names = list(mido.get_output_names())
    except Exception:
        return None
    if not names:
        return None
    prefer = prefer_substr.strip().lower()
    lowered = [(n, n.lower()) for n in names]

    def score(item: Tuple[str, str]) -> Tuple[int, str]:
        name, low = item
        s = 0
        if prefer and prefer in low:
            s += 100
        for i, needle in enumerate(SONG_OUT_PREFER):
            if needle in low:
                s += 50 - i
        if "through" in low or "midi through" in low:
            s -= 40
        if "mpk" in low:
            s -= 20  # controller ports are usually inputs; still de-prioritize
        return (-s, name)

    lowered.sort(key=score)
    return lowered[0][0]


class SongPlayer:
    """Play a Standard MIDI File into the soft-synth and/or a USB MIDI out port."""

    def __init__(self, engine: "SineEngine", emit) -> None:
        self._engine = engine
        self._emit = emit
        self._lock = threading.Lock()
        self._events: List[Tuple[float, mido.Message]] = []
        self._file_bpm = float(DEFAULT_SONG_BPM)
        self._bpm = float(DEFAULT_SONG_BPM)
        self._path: Optional[pathlib.Path] = None
        self._playing = False
        self._loop = False
        self._out_mode = "local"  # local | usb | both
        self._outport: Optional[Any] = None
        self._out_name: Optional[str] = None
        self._stop_play = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._held: set[Tuple[int, int]] = set()

    def is_playing(self) -> bool:
        with self._lock:
            return self._playing

    def path(self) -> Optional[pathlib.Path]:
        with self._lock:
            return self._path

    def bpm(self) -> float:
        with self._lock:
            return self._bpm

    def file_bpm(self) -> float:
        with self._lock:
            return self._file_bpm

    def loop_enabled(self) -> bool:
        with self._lock:
            return self._loop

    def set_loop(self, enabled: bool) -> None:
        with self._lock:
            self._loop = bool(enabled)

    def out_mode(self) -> str:
        with self._lock:
            return self._out_mode

    def set_out_mode(self, mode: str) -> None:
        mode = mode if mode in SONG_OUT_MODES else "local"
        with self._lock:
            self._out_mode = mode

    def out_port_name(self) -> Optional[str]:
        with self._lock:
            return self._out_name

    def outport(self) -> Optional[Any]:
        with self._lock:
            return self._outport

    def event_count(self) -> int:
        with self._lock:
            return len(self._events)

    def duration(self) -> float:
        with self._lock:
            if not self._events:
                return 0.0
            return float(self._events[-1][0])

    def status(self) -> Dict[str, object]:
        with self._lock:
            return {
                "playing": self._playing,
                "bpm": self._bpm,
                "file_bpm": self._file_bpm,
                "loop": self._loop,
                "out_mode": self._out_mode,
                "out_name": self._out_name,
                "events": len(self._events),
                "duration": self._events[-1][0] if self._events else 0.0,
                "path": str(self._path) if self._path else None,
            }

    def set_bpm(self, bpm: float) -> None:
        with self._lock:
            self._bpm = max(20.0, min(400.0, float(bpm)))

    def nudge_bpm(self, delta: float) -> float:
        with self._lock:
            self._bpm = max(20.0, min(400.0, self._bpm + float(delta)))
            return self._bpm

    def clear(self) -> None:
        self.stop()
        with self._lock:
            self._events = []
            self._path = None
            self._file_bpm = float(DEFAULT_SONG_BPM)

    def load(self, path: pathlib.Path) -> bool:
        self.stop()
        try:
            mid = mido.MidiFile(str(path))
        except Exception as exc:
            print(f"song load failed ({path}): {exc}", flush=True)
            return False
        file_bpm = _midifile_native_bpm(mid)
        timed: List[Tuple[float, mido.Message]] = []
        t = 0.0
        try:
            for msg in mid:
                t += float(msg.time)
                if msg.is_meta:
                    continue
                if msg.type in (
                    "note_on",
                    "note_off",
                    "control_change",
                    "program_change",
                    "pitchwheel",
                    "aftertouch",
                    "polytouch",
                ):
                    timed.append((t, msg.copy(time=0)))
        except Exception as exc:
            print(f"song parse failed ({path}): {exc}", flush=True)
            return False
        with self._lock:
            self._events = timed
            self._path = path
            self._file_bpm = file_bpm
            # Keep user tempo unless this is the first load this session
            if self._bpm <= 0:
                self._bpm = file_bpm
        return True

    def ensure_outport(self, prefer_substr: str = "") -> Optional[str]:
        """Open (or keep) a MIDI output port. Returns port name or None."""
        with self._lock:
            if self._outport is not None and self._out_name:
                return self._out_name
        name = pick_song_output_name(prefer_substr)
        if not name:
            return None
        try:
            port = mido.open_output(name)
        except Exception as exc:
            print(f"song MIDI out open failed ({name}): {exc}", flush=True)
            return None
        with self._lock:
            self._outport = port
            self._out_name = name
        return name

    def close_outport(self) -> None:
        with self._lock:
            port = self._outport
            self._outport = None
            self._out_name = None
        if port is not None:
            try:
                port.close()
            except Exception:
                pass

    def start(self) -> bool:
        with self._lock:
            if not self._events:
                return False
            if self._playing:
                return True
            mode = self._out_mode
        if mode in ("usb", "both"):
            if not self.ensure_outport():
                if mode == "usb":
                    return False
        with self._lock:
            self._playing = True
            self._stop_play.clear()
        self._thread = threading.Thread(target=self._play_loop, daemon=True)
        self._thread.start()
        return True

    def stop(self) -> None:
        self._stop_play.set()
        thread = self._thread
        if thread is not None and thread.is_alive():
            thread.join(timeout=1.5)
        self._thread = None
        with self._lock:
            self._playing = False
        self._release_held(send_midi=True)

    def toggle(self) -> bool:
        if self.is_playing():
            self.stop()
            return False
        return self.start()

    def _want_local(self) -> bool:
        with self._lock:
            return self._out_mode in ("local", "both")

    def _want_usb(self) -> bool:
        with self._lock:
            return self._out_mode in ("usb", "both")

    def _release_held(self, *, send_midi: bool) -> None:
        held = list(self._held)
        self._held.clear()
        for ch, note in held:
            if self._want_local():
                try:
                    self._engine.note_off(ch, note)
                except Exception:
                    pass
                try:
                    self._emit(("off", ch, note))
                except Exception:
                    pass
            if send_midi and self._want_usb():
                port = self._outport
                if port is not None:
                    try:
                        port.send(mido.Message("note_off", channel=ch, note=note, velocity=0))
                    except Exception:
                        pass
        if send_midi and self._want_usb():
            port = self._outport
            if port is not None:
                for ch in range(16):
                    try:
                        port.send(mido.Message("control_change", channel=ch, control=123, value=0))
                    except Exception:
                        pass

    def _dispatch(self, msg: mido.Message) -> None:
        local = self._want_local()
        usb = self._want_usb()
        if usb:
            port = self._outport
            if port is not None:
                try:
                    port.send(msg)
                except Exception:
                    pass
        if not local:
            return
        if msg.type == "note_on":
            if msg.velocity <= 0:
                self._engine.note_off(msg.channel, msg.note)
                self._held.discard((msg.channel, msg.note))
                self._emit(("off", msg.channel, msg.note))
            else:
                self._engine.note_on(msg.channel, msg.note, msg.velocity)
                self._held.add((msg.channel, msg.note))
                self._emit(("on", msg.channel, msg.note, msg.velocity))
        elif msg.type == "note_off":
            self._engine.note_off(msg.channel, msg.note)
            self._held.discard((msg.channel, msg.note))
            self._emit(("off", msg.channel, msg.note))
        elif msg.type == "control_change":
            # Soft-synth only understands a few CCs via the live path; ignore here.
            pass
        elif msg.type == "pitchwheel":
            try:
                self._engine.set_pitch_bend(msg.pitch)
            except Exception:
                pass

    def _play_loop(self) -> None:
        while not self._stop_play.is_set():
            with self._lock:
                events = list(self._events)
                bpm = self._bpm
                file_bpm = self._file_bpm
                loop = self._loop
            if not events:
                break
            # Scale file-native seconds so user BPM matches musical intent
            scale = (file_bpm / bpm) if bpm > 1e-6 else 1.0
            t0 = time.monotonic()
            self._release_held(send_midi=True)
            for abs_t, msg in events:
                if self._stop_play.is_set():
                    self._release_held(send_midi=True)
                    with self._lock:
                        self._playing = False
                    return
                target = t0 + abs_t * scale
                while True:
                    remain = target - time.monotonic()
                    if remain <= 0:
                        break
                    if self._stop_play.wait(min(0.003, remain)):
                        self._release_held(send_midi=True)
                        with self._lock:
                            self._playing = False
                        return
                self._dispatch(msg)
            if not loop or self._stop_play.is_set():
                break
        self._release_held(send_midi=True)
        with self._lock:
            self._playing = False


def format_message(msg: mido.Message) -> str:
    if msg.type == "note_on":
        if msg.velocity == 0:
            return f"Note Off  ch{msg.channel + 1}  {midi_note_name(msg.note)} ({msg.note})  vel 0"
        return (
            f"Note On   ch{msg.channel + 1}  {midi_note_name(msg.note)} ({msg.note})  "
            f"vel {msg.velocity}"
        )
    if msg.type == "note_off":
        return f"Note Off  ch{msg.channel + 1}  {midi_note_name(msg.note)} ({msg.note})  vel {msg.velocity}"
    if msg.type == "control_change":
        return f"CC        ch{msg.channel + 1}  cc{msg.control}  val {msg.value}"
    if msg.type == "pitchwheel":
        return f"PitchBend ch{msg.channel + 1}  {msg.pitch}"
    if msg.type == "program_change":
        return f"Program   ch{msg.channel + 1}  prog {msg.program}"
    if msg.type == "aftertouch":
        return f"AT        ch{msg.channel + 1}  {msg.value}"
    if msg.type == "polytouch":
        return f"PolyAT    ch{msg.channel + 1}  n{msg.note}  {msg.value}"
    return str(msg)


class MidiToneApp:
    def __init__(
        self,
        port_filter: str,
        list_only: bool,
        max_voices: int,
        waves_dir: pathlib.Path,
        fullscreen: bool = False,
    ) -> None:
        self.port_filter = port_filter.strip().lower()
        self.event_q: queue.Queue = queue.Queue(maxsize=EVENT_Q_MAX)
        self._waves_dir = pathlib.Path(waves_dir)
        self._user_waves_dir = USER_WAVETABLES_DIR
        try:
            self._user_waves_dir.mkdir(parents=True, exist_ok=True)
        except Exception:
            pass
        self._tables = load_wavetables(self._waves_dir, self._user_waves_dir)
        self.engine = SineEngine(self._tables, max_voices=max_voices)
        # Delay/reverb numbers beside user wavetables (drive/tone already in the wave)
        self._voice_fx_sidecars: Dict[str, Dict[str, float]] = load_user_voice_fx_map(
            self._user_waves_dir
        )
        for vname, snap in self._voice_fx_sidecars.items():
            self.engine.apply_voice_fx_sidecar(vname, snap)
        self._inport: Optional[mido.ports.BaseInput] = None
        self._poll_thread: Optional[threading.Thread] = None
        self._stop = threading.Event()
        self._voice_names = self.engine.voice_names
        self._voice_index = 0
        self._fullscreen = bool(fullscreen)

        if list_only:
            self._print_ports()
            return

        port_name = (
            self._pick_port(retries=20, delay_s=0.5, allow_fallback=False)
            or self._pick_port(retries=1, delay_s=0.0, allow_fallback=True)
        )
        if port_name is None:
            sys.exit("No MIDI input ports found. Is the MPK plugged in?")

        print(f"midi: will use input '{port_name}' (open after UI build)", flush=True)
        print(f"voices: {', '.join(self._voice_names)}", flush=True)

        self._full_vel = True

        # Create the Tk root BEFORE opening PortAudio — on Pi + labwc/Xwayland,
        # starting audio first then Tk can abort during tk.Tk() with no traceback.
        print("ui: creating Tk root", flush=True)
        self.root = tk.Tk()
        print("ui: Tk root ok", flush=True)
        self.root.title("PiDI")
        # Idle watch before the MIDI thread so notes can poke it safely.
        self._idle = IdleWatch(timeout_from_env())
        self._backlight = PanelBacklight()
        self._saver_canvas: Optional[tk.Canvas] = None
        self._saver_hint: Optional[int] = None
        self._saver_clock: Optional[int] = None
        self._saver_started = 0.0
        self._saver_timeout_btn: Optional[tk.Button] = None
        self._saver_tick_after: Optional[str] = None
        self._shift_started = time.monotonic()
        self._shift_xy = (None, None)
        self.root.bind("<Destroy>", self._on_root_destroy, add="+")
        # TFT70 / Pi panel target is 800×480 (older builds used 800×420 and left a gap)
        self.root.geometry("800x480")
        self.root.configure(bg="#111111")
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)
        if self._fullscreen:
            try:
                self.root.attributes("-fullscreen", True)
            except Exception:
                try:
                    self.root.attributes("-zoomed", True)
                except Exception:
                    self.root.state("zoomed")
            print("ui: fullscreen", flush=True)
        # PiDI branded splash while audio + UI chrome build (covers full window
        # until construction finishes — avoids a blank gap after destroy).
        self._boot_splash_photo = None
        splash_path = pathlib.Path(__file__).resolve().parent / "branding" / "pidi-splash.png"
        splash = tk.Frame(self.root, bg="#000000", highlightthickness=0, borderwidth=0)
        if splash_path.is_file():
            try:
                self._boot_splash_photo = tk.PhotoImage(file=str(splash_path))
            except Exception:
                try:
                    from PIL import Image, ImageTk  # type: ignore

                    im = Image.open(splash_path)
                    self._boot_splash_photo = ImageTk.PhotoImage(im)
                except Exception:
                    self._boot_splash_photo = None
        if self._boot_splash_photo is not None:
            tk.Label(
                splash,
                image=self._boot_splash_photo,
                bg="#000000",
                borderwidth=0,
                highlightthickness=0,
            ).place(relx=0.5, rely=0.5, anchor="center")
        else:
            tk.Label(
                splash,
                text="PiDI",
                font=("DejaVu Sans", 42, "bold"),
                fg="#00d4ff",
                bg="#000000",
            ).place(relx=0.5, rely=0.44, anchor="center")
            tk.Label(
                splash,
                text="Raspberry Pi MIDI Toolkit",
                font=("DejaVu Sans", 14),
                fg="#00d4ff",
                bg="#000000",
            ).place(relx=0.5, rely=0.56, anchor="center")
        splash.place(x=0, y=0, relwidth=1, relheight=1)
        splash.lift()
        self._boot_splash = splash
        try:
            self.root.deiconify()
            self.root.lift()
            self.root.update()
        except Exception:
            self.root.update_idletasks()
        self.root.update_idletasks()
        self._apply_display_geometry()
        self.root.update_idletasks()

        # Defer PortAudio + MIDI until after the heavy Tk build — otherwise the
        # audio callback xruns under GIL while widgets/scopes are constructed.
        self._boot_port_name = port_name

        self._full_vel_btn: Optional[tk.Button] = None
        self._drum_lock_btn: Optional[tk.Button] = None
        self._fx_mode_btn: Optional[tk.Button] = None
        self._bus_fx_mode_btn: Optional[tk.Button] = None
        self._voice_lbl: Optional[tk.Label] = None
        self._wave_canvas: Optional[tk.Canvas] = None
        self._wave_caption: Optional[tk.Label] = None
        self._grid_open = False
        self._grid_frame: Optional[tk.Frame] = None
        self._grid_btns: Dict[str, tk.Button] = {}
        self._morph_ui_open = False
        self._morph_frame: Optional[tk.Frame] = None
        self._morph_pick_side = "a"  # which endpoint the next grid tap sets
        self._morph_side_btns: Dict[str, tk.Button] = {}
        self._morph_grid_btns: Dict[str, tk.Button] = {}
        self._morph_status_lbl: Optional[tk.Label] = None
        self._kit_ui_open = False
        self._kit_frame: Optional[tk.Frame] = None
        self._kit_btns: Dict[int, tk.Button] = {}
        self._kit_all_btn: Optional[tk.Button] = None
        self._kit_wave_canvas: Optional[tk.Canvas] = None
        self._kit_status_var = tk.StringVar(value="")
        self._kit_selected_note = 36  # factory kick
        self._kit_all_drums = False  # FX edit target = shared kit group bus
        self._kit_view = "grid"  # grid | wave (scope is a drill-down)
        self._fx_ui_open = False
        self._fx_frame: Optional[tk.Frame] = None
        self._fx_title_var: Optional[tk.StringVar] = None
        self._fx_target_var: Optional[tk.StringVar] = None
        self._fx_value_vars: Dict[str, tk.StringVar] = {}
        self._fx_prev_mode = "synth"
        self._scope_blanked = False
        self._scope_blanked_synth = False
        self._scope_blanked_drum = False
        self._scope_dirty_synth = False
        self._scope_dirty_drum = False
        self._scope_needs_paint = False
        self._scope_paint_at = 0.0
        self._scope_first_dirty = 0.0
        self._fx_dirty_ui = False
        self._save_voice_open = False
        self._save_voice_frame: Optional[tk.Frame] = None
        self._save_voice_entry: Optional[tk.Entry] = None
        self._save_voice_status: Optional[tk.Label] = None
        self._save_voice_drive_btn: Optional[tk.Button] = None
        self._save_voice_keys: Optional[tk.Frame] = None
        self._save_voice_keys_digits = False
        self._power_ui_open = False
        self._power_frame: Optional[tk.Frame] = None
        self._mode = "synth"  # home | synth | seq | pads | songs | log | presets | settings
        self._mode_btns: Dict[str, tk.Button] = {}
        self._seq = OverdubSequencer(self.engine, self._q_put)
        self._phrases = PhrasePadBank(self.engine, self._q_put, PHRASES_DIR)
        self._songs = SongPlayer(self.engine, self._q_put)
        self._pads_view = "edit"  # play | edit
        self._phrase_out_mode = "local"  # local | usb | both (shares Songs USB port)
        self._phrases.set_output_hooks(
            get_out_mode=lambda: self._phrase_out_mode,
            ensure_outport=lambda: self._songs.ensure_outport(),
            get_outport=lambda: self._songs.outport(),
        )
        self._phrase_status_var = tk.StringVar(
            value=self._phrases.status_line(view=self._pads_view)
        )
        self._phrase_pad_btns: Dict[int, tk.Button] = {}
        self._phrase_clear_btn: Optional[tk.Button] = None
        self._phrase_mode_btn: Optional[tk.Button] = None
        self._phrase_out_btn: Optional[tk.Button] = None
        self._phrase_view_btns: Dict[str, tk.Button] = {}
        self._phrase_trig_btn: Optional[tk.Button] = None
        self._phrase_voice_btn: Optional[tk.Button] = None
        self._phrase_ch_btn: Optional[tk.Button] = None
        self._phrase_synth_btn: Optional[tk.Button] = None
        self._phrase_vib_btn: Optional[tk.Button] = None
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._seq_to_pad_armed = False
        self._seq_to_pad_btn: Optional[tk.Button] = None
        self._phrase_shell: Optional[tk.Frame] = None
        self._seq_status_var = tk.StringVar(value=self._seq.status_line())
        self._seq_layer_var = tk.StringVar(value="no layers yet")
        self._seq_rec_btn: Optional[tk.Button] = None
        self._seq_play_btn: Optional[tk.Button] = None
        self._seq_keep_btn: Optional[tk.Button] = None
        self._seq_drop_btn: Optional[tk.Button] = None
        self._seq_undo_btn: Optional[tk.Button] = None
        self._seq_extend_btn: Optional[tk.Button] = None
        self._seq_len_var = tk.StringVar(value="LEN 1×")
        self._vib_depth_var = tk.StringVar(value="0.50 st")
        self._vib_rate_var = tk.StringVar(value="5.0 Hz")
        self._vib_toggle_btn: Optional[tk.Button] = None
        self._preset_status_var = tk.StringVar(value="Tap a slot, then LOAD or SAVE.")
        self._preset_slot = 0
        self._preset_slot_btns: Dict[int, tk.Button] = {}
        self._active_preset_name: Optional[str] = None
        self._pending_restore_mode: Optional[str] = None
        self._save_preset_open = False
        self._save_preset_frame: Optional[tk.Frame] = None
        self._save_preset_entry: Optional[tk.Entry] = None
        self._save_preset_status: Optional[tk.Label] = None
        self._save_preset_keys: Optional[tk.Frame] = None
        self._save_preset_keys_digits = False
        self._song_status_var = tk.StringVar(
            value="Songs: tap a file to load, set BPM, then PLAY (LOCAL or USB→DIN)."
        )
        self._song_files: List[pathlib.Path] = []
        self._song_selected: Optional[str] = None  # filename in songs/
        self._song_scroll = 0
        self._song_row_btns: List[tk.Button] = []
        self._song_title_cache: Dict[str, str] = {}
        self._song_play_btn: Optional[tk.Button] = None
        self._song_out_btn: Optional[tk.Button] = None
        self._song_loop_btn: Optional[tk.Button] = None
        self._song_bpm_lbl: Optional[tk.Label] = None
        self._song_up_btn: Optional[tk.Button] = None
        self._song_down_btn: Optional[tk.Button] = None
        self._settings_dirty = False
        self._suppress_autosave = False
        self._update_check: Optional[updater.UpdateCheck] = None
        self._update_busy = False
        self._update_confirming = False
        self._settings_status_var = tk.StringVar(value=updater.format_status_lines())
        self._settings_check_btn: Optional[tk.Button] = None
        self._settings_update_btn: Optional[tk.Button] = None
        self._token_ui_open = False
        self._token_frame: Optional[tk.Frame] = None
        self._token_entry: Optional[tk.Entry] = None
        self._token_keys: Optional[tk.Frame] = None
        self._token_keys_digits = False

        # Keep PiDI splash on top while chrome is packed underneath
        splash = getattr(self, "_boot_splash", None)
        if splash is not None:
            try:
                splash.lift()
            except Exception:
                pass

        # Persistent chrome: title, HOME, POWER. Jam modes keep SYNTH/SEQ/PADS
        # on the right; every other screen is reached from HOME tiles.
        self._nav = tk.Frame(self.root, bg="#1d2021")
        self._nav.pack(side=tk.TOP, fill=tk.X, padx=0, pady=0)
        tk.Label(
            self._nav, text="PiDI", font=("DejaVu Sans", 14, "bold"),
            fg="#00d4ff", bg="#1d2021", padx=10, pady=8,
        ).pack(side=tk.LEFT)
        home_btn = self._mk_touch_btn(
            self._nav, "HOME", lambda: self._switch_mode("home"), bg="#3c3836"
        )
        home_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        home_btn.pack(side=tk.LEFT, padx=(0, 4))
        self._mode_btns["home"] = home_btn
        self._home_btn = home_btn
        power_btn = self._mk_touch_btn(
            self._nav, "POWER", self._open_power_menu, bg="#9d0006"
        )
        power_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        power_btn.pack(side=tk.LEFT, padx=(0, 4))
        nav_modes = tk.Frame(self._nav, bg="#1d2021")
        nav_modes.pack(side=tk.RIGHT, padx=4, pady=4)
        self._jam_btns: Dict[str, tk.Button] = {}
        for key, label in (
            ("synth", "SYNTH"),
            ("seq", "SEQ"),
            ("pads", "PADS"),
        ):
            btn = self._mk_touch_btn(
                nav_modes, label, lambda m=key: self._switch_mode(m), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
            self._jam_btns[key] = btn
            self._mode_btns[key] = btn

        # Mode content host
        self._mode_host = tk.Frame(self.root, bg="#111111")
        self._mode_host.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        self._synth_shell = tk.Frame(self._mode_host, bg="#111111")
        self._seq_shell = tk.Frame(self._mode_host, bg="#111111")
        self._pads_shell = tk.Frame(self._mode_host, bg="#111111")
        self._phrase_shell = self._pads_shell
        self._songs_shell = tk.Frame(self._mode_host, bg="#111111")
        self._presets_shell = tk.Frame(self._mode_host, bg="#111111")
        self._log_shell = tk.Frame(self._mode_host, bg="#111111")
        self._settings_shell = tk.Frame(self._mode_host, bg="#111111")
        self._home_shell = tk.Frame(self._mode_host, bg="#111111")

        # Bottom touch bar packed first so it never gets crushed / lost
        self._touch = tk.Frame(self._synth_shell, bg="#111111")
        self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)

        row1 = tk.Frame(self._touch, bg="#111111")
        row1.pack(fill=tk.X, pady=(0, 6))
        self._mk_touch_btn(row1, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )
        self._full_vel_btn = self._mk_touch_btn(
            row1, "FULL VEL: ON", self._toggle_full_vel, bg="#689d6a"
        )
        self._full_vel_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        self._fx_mode_btn = self._mk_touch_btn(
            row1, "FX MODE", self._toggle_fx_mode, bg="#3c3836"
        )
        self._fx_mode_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        self._bus_fx_mode_btn = self._mk_touch_btn(
            row1, "BUS FX", self._toggle_bus_fx_mode, bg="#3c3836"
        )
        self._bus_fx_mode_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)

        # Voice picker — prev / name / next + full grid
        row2 = tk.Frame(self._touch, bg="#111111")
        row2.pack(fill=tk.X, pady=(0, 6))
        self._mk_touch_btn(row2, "◀ PREV", self._prev_voice, bg="#3c3836").pack(
            side=tk.LEFT, fill=tk.BOTH, padx=3, ipady=10
        )
        self._voice_lbl = tk.Label(
            row2,
            text=self._voice_label_text(),
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#458588",
            padx=8,
            pady=12,
        )
        self._voice_lbl.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
        # Tap the name → open the voice grid (easier than blind PREV/NEXT)
        self._voice_lbl.bind("<ButtonPress-1>", lambda _e: self._open_voice_grid())
        self._mk_touch_btn(row2, "NEXT ▶", self._next_voice, bg="#3c3836").pack(
            side=tk.LEFT, fill=tk.BOTH, padx=3, ipady=10
        )

        row3 = tk.Frame(self._touch, bg="#111111")
        row3.pack(fill=tk.X)
        self._mk_touch_btn(row3, "VOICES", self._open_voice_grid, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._mk_touch_btn(row3, "MORPH", self._open_morph_menu, bg="#b16286").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._mk_touch_btn(row3, "KIT", self._open_kit_explorer, bg="#d79921").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._drum_lock_btn = self._mk_touch_btn(
            row3, "DRUM MODE", self._toggle_drum_lock, bg="#3c3836"
        )
        self._drum_lock_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

        self._main = tk.Frame(self._synth_shell, bg="#111111")
        self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        header = tk.Frame(self._main, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(6, 2))
        tk.Label(
            header, text="Synth", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text=port_name, font=("DejaVu Sans", 11),
            fg="#8ec07c", bg="#111111",
        ).pack(side=tk.RIGHT)

        # CRT morph-cycle scope — pack early with a reserved height so the
        # chunky touch bar never collapses it on 480p kiosk screens.
        self._wave_caption = tk.Label(
            self._main,
            text="Morph",
            font=("DejaVu Sans", 11),
            fg="#4ade80",
            bg="#111111",
            anchor="w",
        )
        self._wave_caption.pack(fill=tk.X, padx=8, pady=(2, 0))
        self._wave_canvas = tk.Canvas(
            self._main,
            height=150,
            bg=SCOPE_CRT_BG,
            highlightthickness=1,
            highlightbackground="#14532d",
            bd=0,
        )
        self._wave_canvas.pack(fill=tk.X, padx=8, pady=(2, 4))
        self._wave_canvas.pack_propagate(False)
        self._wave_canvas.bind("<Configure>", self._on_synth_scope_configure)

        self.last_var = tk.StringVar(value="Waiting for MIDI…")
        last_lbl = tk.Label(
            self._main, textvariable=self.last_var,
            font=("DejaVu Sans Mono", 13, "bold"), fg="#fabd2f", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        last_lbl.pack(fill=tk.X, padx=8, pady=(2, 0))

        self.active_var = tk.StringVar(value="Active notes: —")
        active_lbl = tk.Label(
            self._main, textvariable=self.active_var,
            font=("DejaVu Sans", 11), fg="#83a598", bg="#111111", anchor="w",
        )
        active_lbl.pack(fill=tk.X, padx=8)

        self.mod_var = tk.StringVar(value=self._format_mod_line())
        mod_lbl = tk.Label(
            self._main, textvariable=self.mod_var,
            font=("DejaVu Sans Mono", 10), fg="#d3869b", bg="#111111", anchor="w",
        )
        mod_lbl.pack(fill=tk.X, padx=8, pady=(0, 2))

        self._active_notes: Dict[Tuple[int, int], int] = {}
        # Select first voice explicitly
        self.engine.set_waveform(self._voice_names[self._voice_index])
        self.root.after(40, self._drain_queue)
        self.root.after(80, lambda: self._paint_synth_waveform(force=True))
        # Default morph pair: first voice ↔ second (or same if only one)
        if len(self._voice_names) > 1:
            self.engine.set_morph_pair(0, 1, morph=0.0)

        PRESETS_DIR.mkdir(parents=True, exist_ok=True)
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        PHRASES_DIR.mkdir(parents=True, exist_ok=True)
        seeded = seed_demo_songs()

        self._build_seq_mode()
        self._build_pads_mode()
        self._build_songs_mode()
        self._build_presets_mode()
        self._build_log_mode()
        self._build_settings_mode()
        self._build_home_mode()
        self._switch_mode("synth")

        # Restore last session (full vel, morph, seq, phrases, UI mode, …)
        restored = self._load_settings_file(SETTINGS_PATH)
        self._refresh_ui_after_session()

        self._append_log(f"Listening on: {port_name}")
        self._append_log(f"Loaded {len(self._voice_names)} voices — VOICES grid / MORPH pair.")
        self._append_log(
            "MPK knobs (keys): morph / tone / attack / release / vib / — / synth lvl"
        )
        self._append_log(
            "Pads = analog drum voices. After a pad (or DRUM LOCK): knobs → "
            "pitch / stretch / noise / drum-tone / — / — / — / drum lvl"
        )
        self._append_log("HOME opens every mode. Jam cluster: SYNTH / SEQ / PADS stay in the top bar.")
        if seeded:
            self._append_log(
                f"Added {seeded} demo song(s) from demo-songs/ (offline classical pack)."
            )
        if restored:
            self._append_log(f"Restored session from {SETTINGS_PATH.name}")
        else:
            self._append_log("No settings.json yet — changes will autosave.")
        self._append_log("If knobs do nothing: Prog Select + Pad 1 (MPC program).")

        # Start audio after Tk chrome exists so construction can't starve the callback.
        # Re-resolve MIDI in case the MPK finished enumerating after our earlier pick.
        port_name = (
            self._pick_port(retries=12, delay_s=0.4, allow_fallback=False)
            or self._pick_port(retries=1, delay_s=0.0, allow_fallback=True)
            or getattr(self, "_boot_port_name", None)
            or port_name
        )
        self.engine.start()
        print("midi: audio engine started", flush=True)
        self._inport = mido.open_input(port_name)
        print(f"midi: input port open ({port_name})", flush=True)
        self._poll_thread = threading.Thread(target=self._midi_loop, daemon=True)
        self._poll_thread.start()
        print("midi: poll thread started", flush=True)
        if self.port_filter and self.port_filter not in port_name.lower():
            self.root.after(1500, self._maybe_reopen_midi)

        print("ui: construction complete", flush=True)
        # Reveal chrome only after first layout pass
        splash = getattr(self, "_boot_splash", None)
        if splash is not None:
            try:
                self.root.update_idletasks()
                splash.destroy()
            except Exception:
                pass
            self._boot_splash = None
            self._boot_splash_photo = None
            try:
                self.root.update_idletasks()
            except Exception:
                pass
        self.root.after(2000, self._autosave_tick)
        self.root.bind_all("<ButtonPress>", self._on_pointer_activity, add="+")
        self._saver_tick_after = self.root.after(1000, self._screensaver_tick)
        self._apply_pixel_shift()
        self._append_log(
            f"TFT burn-in guard: {timeout_label(self._idle.timeout_sec)} "
            "— tap the panel to wake; MIDI does not."
        )

    def _voice_label_text(self) -> str:
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            return f"{self._voice_index + 1}/{len(self._voice_names)}  {left.upper()}"
        pct = int(round(blend * 100))
        return f"{left.upper()} → {right.upper()}  {pct}%"

    def _format_mod_line(self) -> str:
        st = self.engine.modulation_state()
        if self.engine.fx_knob_focus():
            delay_ms = int((0.05 + st.get("fx_delay_time", 0.0) * 0.70) * 1000)
            target = self.engine.fx_edit_label()
            prefix = "BUS FX" if self.engine.bus_fx_mode() else f"FX {target}"
            return (
                f"{prefix}  "
                f"Drive:{int(st.get('fx_drive', 0.0) * 127):3d}  "
                f"Dly:{delay_ms:3d}ms  "
                f"Fb:{int(st.get('fx_delay_fb', 0.0) * 127):3d}  "
                f"Dmix:{int(st.get('fx_delay_mix', 0.0) * 127):3d}  "
                f"Rvb:{int(st.get('fx_reverb_mix', 0.0) * 127):3d}  "
                f"Syn:{int(st.get('synth_level', st['level']) * 127):3d}  "
                f"Drm:{int(st.get('drum_level', 1.0) * 127):3d}"
            )
        if self.engine.drum_knob_focus():
            return (
                "DRUM MODE  "
                f"Pitch:{int(st['drum_pitch'] * 127):3d}  "
                f"Stretch:{int(st['drum_decay'] * 127):3d}  "
                f"Noise:{int(st['drum_noise'] * 127):3d}  "
                f"Tone:{int(st['drum_tone'] * 127):3d}  "
                f"DrmLvl:{int(st.get('drum_level', 1.0) * 127):3d}"
            )
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            morph_txt = left
        else:
            morph_txt = f"{left}→{right}"
        depth, rate, always = self.engine.vib_state()
        amount = max(float(st["mod"]), always)
        vib_txt = f"{depth:.1f}st@{rate:.1f}Hz" if amount > 0.01 else "off"
        return (
            f"Morph:{int(blend * 100):3d}% ({morph_txt})  "
            f"Tone:{int(st['tone'] * 127):3d}  "
            f"Syn:{int(st.get('synth_level', st['level']) * 127):3d}  "
            f"Drm:{int(st.get('drum_level', 1.0) * 127):3d}  "
            f"Bend:{st['bend']:+.2f}  "
            f"Vib:{vib_txt}"
        )

    def _overlay_busy(self) -> bool:
        return (
            self._power_ui_open
            or self._grid_open
            or self._morph_ui_open
            or self._kit_ui_open
            or self._save_voice_open
            or self._save_preset_open
            or self._fx_ui_open
            or self._token_ui_open
            or bool(getattr(self, "_idle", None) and self._idle.active)
        )

    def _refresh_ui_after_session(self) -> None:
        """Repaint chrome after settings.json / preset LOAD."""
        self._paint_full_vel_btn()
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._sync_voice_index_from_morph()
        self._build_pads_mode()
        self._paint_song_slots()
        self._refresh_song_status()
        self._refresh_seq_status()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        pending = self._pending_restore_mode
        self._pending_restore_mode = None
        if isinstance(pending, str) and pending in self._mode_btns:
            self._switch_mode(pending)
        try:
            self._paint_synth_waveform(force=True)
        except Exception:
            pass

    def _session_dict(self) -> Dict[str, Any]:
        return {
            "version": SETTINGS_VERSION,
            "full_velocity": bool(self._full_vel),
            "active_preset": self._active_preset_name,
            "ui_mode": self._mode,
            "pads_view": self._pads_view,
            "synth": self.engine.snapshot_settings(),
            # Mode toggles — restore the editing context, not just the sound.
            "drum_mode": bool(self.engine.drum_mode()),
            "fx_mode": bool(self.engine.fx_mode()),
            "bus_fx_mode": bool(self.engine.bus_fx_mode()),
            "fx_edit_kind": str(self.engine.fx_edit_kind()),
            "seq": self._seq.export_state(),
            "phrases": self._phrases.export_bank(),
            "pads": {
                "view": self._pads_view,
                "out_mode": self._phrase_out_mode,
            },
            "songs": {
                "selected": self._song_selected,
                "bpm": float(self._songs.bpm()),
                "loop": bool(self._songs.loop_enabled()),
                "out_mode": self._songs.out_mode(),
            },
            "screensaver_sec": float(self._idle.timeout_sec),
        }

    def _apply_session_dict(self, data: Dict[str, Any]) -> None:
        self._suppress_autosave = True
        try:
            if "full_velocity" in data:
                self._full_vel = bool(data["full_velocity"])
            if "screensaver_sec" in data and "MIDI_TONE_SCREENSAVER_SEC" not in os.environ:
                try:
                    self._idle.timeout_sec = max(0.0, float(data["screensaver_sec"]))
                except (TypeError, ValueError):
                    pass
            if "active_preset" in data:
                name = data["active_preset"]
                self._active_preset_name = str(name) if name else None
            synth = data.get("synth")
            if isinstance(synth, dict):
                self.engine.apply_settings(synth)
            # Restore mode toggles after apply_settings (which clears them)
            if "drum_mode" in data:
                self.engine.set_drum_mode(bool(data["drum_mode"]))
            if bool(data.get("bus_fx_mode")):
                self.engine.set_bus_fx_mode(True)
            elif bool(data.get("fx_mode")):
                self.engine.set_fx_mode(True)
                kind = str(data.get("fx_edit_kind", "voice") or "voice")
                if kind == "drums":
                    self.engine.set_fx_edit_drums()
                elif kind == "drum":
                    # Keep last kit selection if present; else nearer voice is fine
                    pass
                elif kind == "bus":
                    self.engine.set_fx_edit_bus()
                else:
                    self.engine.set_fx_edit_voice(None)
            seq = data.get("seq")
            if isinstance(seq, dict):
                try:
                    self._seq.import_state(seq)
                except Exception as exc:
                    print(f"seq restore failed: {exc}", flush=True)
            phrases = data.get("phrases")
            if isinstance(phrases, dict):
                try:
                    self._phrases.import_bank(phrases, persist=True)
                except Exception as exc:
                    print(f"phrases restore failed: {exc}", flush=True)
            pads = data.get("pads")
            if isinstance(pads, dict):
                view = str(pads.get("view", self._pads_view) or "edit")
                self._pads_view = "play" if view == "play" else "edit"
                out = str(pads.get("out_mode", self._phrase_out_mode) or "local")
                self._phrase_out_mode = out if out in SONG_OUT_MODES else "local"
            elif "pads_view" in data:
                view = str(data.get("pads_view") or "edit")
                self._pads_view = "play" if view == "play" else "edit"
            songs = data.get("songs")
            if isinstance(songs, dict):
                if "bpm" in songs:
                    self._songs.set_bpm(float(songs["bpm"]))
                if "loop" in songs:
                    self._songs.set_loop(bool(songs["loop"]))
                if "out_mode" in songs:
                    self._songs.set_out_mode(str(songs["out_mode"]))
                selected = songs.get("selected")
                if not selected and "slot" in songs:
                    # Back-compat with older slot-based settings
                    try:
                        selected = f"song-{int(songs['slot']) + 1:02d}.mid"
                    except Exception:
                        selected = None
                self._refresh_song_file_list(prefer=str(selected) if selected else None)
                path = self._selected_song_path()
                if path is not None and path.is_file():
                    self._songs.load(path)
                    if "bpm" in songs:
                        self._songs.set_bpm(float(songs["bpm"]))
            ui_mode = data.get("ui_mode")
            if isinstance(ui_mode, str) and ui_mode in (
                "synth", "seq", "pads", "songs", "log", "presets", "settings"
            ):
                # Defer switch until chrome exists; store for caller
                self._pending_restore_mode = ui_mode
            self._settings_dirty = False
        finally:
            self._suppress_autosave = False

    def _load_settings_file(self, path: pathlib.Path) -> bool:
        if not path.is_file():
            return False
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:
            print(f"settings load failed ({path}): {exc}", flush=True)
            return False
        if not isinstance(data, dict):
            return False
        self._apply_session_dict(data)
        return True

    def _save_settings_file(self, path: pathlib.Path, *, quiet: bool = False) -> bool:
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(path.suffix + ".tmp")
            tmp.write_text(
                json.dumps(self._session_dict(), indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            tmp.replace(path)
            self._settings_dirty = False
            if not quiet:
                print(f"settings saved → {path}", flush=True)
            return True
        except Exception as exc:
            print(f"settings save failed ({path}): {exc}", flush=True)
            return False

    def _mark_settings_dirty(self) -> None:
        if self._suppress_autosave:
            return
        self._settings_dirty = True

    def _autosave_tick(self) -> None:
        if self._stop.is_set():
            return
        if self._settings_dirty:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        if not self._stop.is_set():
            self.root.after(2000, self._autosave_tick)

    def _preset_path(self, slot: int) -> pathlib.Path:
        return PRESETS_DIR / f"slot-{slot + 1:02d}.json"

    def _selected_song_path(self) -> Optional[pathlib.Path]:
        if not self._song_selected:
            return None
        path = SONGS_DIR / self._song_selected
        return path if path.is_file() else None

    def _refresh_song_file_list(self, prefer: Optional[str] = None) -> None:
        """Rescan songs/ and keep selection/scroll coherent."""
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        self._song_files = list_song_files(SONGS_DIR)
        names = {p.name for p in self._song_files}
        chosen = prefer if prefer in names else None
        if chosen is None and self._song_selected in names:
            chosen = self._song_selected
        if chosen is None and self._song_files:
            chosen = self._song_files[0].name
        self._song_selected = chosen
        if not self._song_files:
            self._song_scroll = 0
            return
        idx = 0
        if chosen:
            for i, p in enumerate(self._song_files):
                if p.name == chosen:
                    idx = i
                    break
        max_scroll = max(0, len(self._song_files) - SONG_LIST_VISIBLE)
        # Keep selection visible
        if idx < self._song_scroll:
            self._song_scroll = idx
        elif idx >= self._song_scroll + SONG_LIST_VISIBLE:
            self._song_scroll = idx - SONG_LIST_VISIBLE + 1
        self._song_scroll = max(0, min(max_scroll, self._song_scroll))

    def _next_take_path(self) -> pathlib.Path:
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        n = 1
        while True:
            path = SONGS_DIR / f"take-{n:03d}.mid"
            if not path.exists():
                return path
            n += 1
            if n > 9999:
                return SONGS_DIR / f"take-{int(time.time())}.mid"

    def _build_songs_mode(self) -> None:
        shell = self._songs_shell
        for w in shell.winfo_children():
            w.destroy()
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        self._refresh_song_file_list(prefer=self._song_selected)

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="Songs", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        bpm_row = tk.Frame(header, bg="#111111")
        bpm_row.pack(side=tk.RIGHT)
        self._mk_touch_btn(bpm_row, "BPM −", lambda: self._song_nudge_bpm(-1), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._song_bpm_lbl = tk.Label(
            bpm_row,
            text=self._song_bpm_label(),
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            padx=6,
        )
        self._song_bpm_lbl.pack(side=tk.LEFT)
        self._mk_touch_btn(bpm_row, "BPM +", lambda: self._song_nudge_bpm(1), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._mk_touch_btn(bpm_row, "−5", lambda: self._song_nudge_bpm(-5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._mk_touch_btn(bpm_row, "+5", lambda: self._song_nudge_bpm(5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )

        status = tk.Label(
            shell, textvariable=self._song_status_var,
            font=("DejaVu Sans", 11, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(2, 4))

        # Transport is packed from the bottom *before* the list, so a short panel
        # shrinks the song rows instead of pushing PLAY/STOP off the screen.
        row_b = tk.Frame(shell, bg="#111111")
        row_b.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(3, 8))
        self._mk_touch_btn(
            row_b, "SAVE SEQ", self._song_save_from_seq, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)
        self._mk_touch_btn(row_b, "DELETE", self._song_delete_selected, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._song_out_btn = self._mk_touch_btn(
            row_b, "OUT: LOCAL", self._song_cycle_out_mode, bg="#3c3836"
        )
        self._song_out_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)
        self._song_loop_btn = self._mk_touch_btn(
            row_b, "SONG LOOP: OFF", self._song_toggle_loop, bg="#3c3836"
        )
        self._song_loop_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

        row_a = tk.Frame(shell, bg="#111111")
        row_a.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(4, 3))
        self._song_play_btn = self._mk_touch_btn(
            row_a, "PLAY", self._song_toggle_play, bg="#689d6a"
        )
        self._song_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10)
        self._mk_touch_btn(row_a, "STOP", self._song_stop, bg="#d79921").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )

        # Chunky list with dedicated scroll targets (no tiny scrollbar). They sit
        # in a side column so paging costs width, which is plentiful, not height.
        list_wrap = tk.Frame(shell, bg="#111111")
        list_wrap.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)

        pager = tk.Frame(list_wrap, bg="#111111")
        pager.pack(side=tk.RIGHT, fill=tk.Y, padx=(6, 0))
        self._song_up_btn = self._mk_touch_btn(
            pager, "▲", lambda: self._song_scroll_by(-SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_up_btn.configure(font=("DejaVu Sans", 18, "bold"), padx=14)
        self._song_up_btn.pack(side=tk.TOP, expand=True, fill=tk.BOTH, pady=(0, 2))
        self._song_down_btn = self._mk_touch_btn(
            pager, "▼", lambda: self._song_scroll_by(SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_down_btn.configure(font=("DejaVu Sans", 18, "bold"), padx=14)
        self._song_down_btn.pack(side=tk.BOTTOM, expand=True, fill=tk.BOTH, pady=(2, 0))

        rows = tk.Frame(list_wrap, bg="#111111")
        rows.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        self._song_row_btns = []
        for i in range(SONG_LIST_VISIBLE):
            btn = self._mk_touch_btn(
                rows,
                "",
                lambda idx=i: self._select_song_row(idx),
                bg="#3c3836",
            )
            btn.configure(
                font=("DejaVu Sans", 13, "bold"),
                anchor="w",
                justify=tk.LEFT,
                pady=4,
            )
            btn.pack(fill=tk.BOTH, expand=True, pady=2, ipady=2)
            self._song_row_btns.append(btn)

        self._paint_song_list()
        self._paint_song_controls()
        self._refresh_song_status()

    def _song_bpm_label(self) -> str:
        return f"{int(round(self._songs.bpm()))} BPM"

    def _song_title_from_file(self, path: pathlib.Path) -> str:
        key = path.name
        cached = self._song_title_cache.get(key)
        if cached is not None:
            return cached
        title = path.stem
        try:
            mid = mido.MidiFile(str(path))
            for tr in mid.tracks:
                for msg in tr:
                    if msg.is_meta and msg.type in ("track_name", "sequence_name"):
                        name = (msg.name or "").strip()
                        if name:
                            title = name
                            break
                    if msg.is_meta and msg.type == "text":
                        text = (msg.text or "").strip()
                        if text:
                            title = text
                            break
                else:
                    continue
                break
        except Exception:
            pass
        title = title[:40]
        self._song_title_cache[key] = title
        return title

    def _song_row_label(self, path: pathlib.Path) -> str:
        """One line per row — four fat targets beat two tall ones on a 480px panel."""
        title = self._song_title_from_file(path)
        if title.lower() == path.stem.lower() or title == path.stem:
            return f"  {path.name}"
        label = f"  {title} · {path.name}"
        return label if len(label) <= 58 else label[:57] + "…"

    def _song_scroll_by(self, delta: int) -> None:
        if not self._song_files:
            return
        max_scroll = max(0, len(self._song_files) - SONG_LIST_VISIBLE)
        self._song_scroll = max(0, min(max_scroll, self._song_scroll + int(delta)))
        self._paint_song_list()

    def _select_song_row(self, row: int) -> None:
        idx = self._song_scroll + row
        if idx < 0 or idx >= len(self._song_files):
            return
        path = self._song_files[idx]
        self._song_selected = path.name
        if self._songs.is_playing():
            self._songs.stop()
        if self._songs.load(path):
            self._append_log(f"Song loaded: {path.name}")
        else:
            self._song_status_var.set(f"Failed to load {path.name}")
        self._mark_settings_dirty()
        self._paint_song_list()
        self._paint_song_controls()
        self._refresh_song_status()

    def _paint_song_list(self) -> None:
        total = len(self._song_files)
        max_scroll = max(0, total - SONG_LIST_VISIBLE)
        self._song_scroll = max(0, min(max_scroll, self._song_scroll))
        for row, btn in enumerate(self._song_row_btns):
            idx = self._song_scroll + row
            if idx >= total:
                btn.configure(
                    text="",
                    state=tk.DISABLED,
                    bg="#1d2021",
                    activebackground="#1d2021",
                    disabledforeground="#665c54",
                )
                continue
            path = self._song_files[idx]
            selected = path.name == self._song_selected
            color = "#b16286" if selected else "#458588"
            btn.configure(
                text=self._song_row_label(path),
                state=tk.NORMAL,
                bg=color,
                activebackground=color,
                fg="#fbf1c7",
            )
        if self._song_up_btn is not None:
            can_up = self._song_scroll > 0
            self._song_up_btn.configure(
                state=tk.NORMAL if can_up else tk.DISABLED,
                bg="#504945" if can_up else "#1d2021",
                activebackground="#504945" if can_up else "#1d2021",
                disabledforeground="#665c54",
            )
        if self._song_down_btn is not None:
            can_down = self._song_scroll < max_scroll
            self._song_down_btn.configure(
                state=tk.NORMAL if can_down else tk.DISABLED,
                bg="#504945" if can_down else "#1d2021",
                activebackground="#504945" if can_down else "#1d2021",
                disabledforeground="#665c54",
            )

    def _paint_song_slots(self) -> None:
        """Compat name used by mode switch / seed — refresh list from disk."""
        self._refresh_song_file_list(prefer=self._song_selected)
        self._paint_song_list()

    def _paint_song_controls(self) -> None:
        if self._song_bpm_lbl is not None:
            self._song_bpm_lbl.configure(text=self._song_bpm_label())
        if self._song_play_btn is not None:
            if self._songs.is_playing():
                self._song_play_btn.configure(
                    text="■ STOP", bg="#d79921", activebackground="#d79921"
                )
            else:
                self._song_play_btn.configure(
                    text="PLAY", bg="#689d6a", activebackground="#689d6a"
                )
        if self._song_out_btn is not None:
            mode = self._songs.out_mode().upper()
            colors = {"LOCAL": "#3c3836", "USB": "#458588", "BOTH": "#689d6a"}
            color = colors.get(mode, "#3c3836")
            self._song_out_btn.configure(
                text=f"OUT: {mode}", bg=color, activebackground=color
            )
        if self._song_loop_btn is not None:
            if self._songs.loop_enabled():
                self._song_loop_btn.configure(
                    text="SONG LOOP: ON", bg="#689d6a", activebackground="#689d6a"
                )
            else:
                self._song_loop_btn.configure(
                    text="SONG LOOP: OFF", bg="#3c3836", activebackground="#3c3836"
                )

    def _refresh_song_status(self) -> None:
        st = self._songs.status()
        path = st.get("path")
        name = pathlib.Path(str(path)).name if path else (self._song_selected or "(none)")
        nfiles = len(self._song_files)
        out = str(st["out_mode"]).upper()
        out_name = st.get("out_name") or "—"
        if nfiles == 0:
            msg = "songs/ is empty — SAVE SEQ, or drop .mid files in. Demos seed on first launch."
        elif st["playing"]:
            msg = (
                f"▶ PLAYING {name}  @ {int(round(float(st['bpm'])))} BPM  "
                f"(file {int(round(float(st['file_bpm'])))})  out={out} ({out_name})"
            )
        elif int(st["events"]) == 0:
            msg = (
                f"{nfiles} file(s) — tap one to load. "
                f"Tempo {int(round(float(st['bpm'])))} BPM · out={out}"
            )
        else:
            msg = (
                f"Ready {name}  {float(st['duration']):.1f}s · {st['events']} ev  "
                f"@ {int(round(float(st['bpm'])))} BPM (file {int(round(float(st['file_bpm'])))})  "
                f"out={out}  [{self._song_scroll + 1}-{min(nfiles, self._song_scroll + SONG_LIST_VISIBLE)}/{nfiles}]"
            )
        self._song_status_var.set(msg)
        self._paint_song_controls()

    def _song_nudge_bpm(self, delta: float) -> None:
        self._songs.nudge_bpm(delta)
        self._mark_settings_dirty()
        self._refresh_song_status()

    def _song_toggle_loop(self) -> None:
        self._songs.set_loop(not self._songs.loop_enabled())
        self._mark_settings_dirty()
        self._refresh_song_status()

    def _song_cycle_out_mode(self) -> None:
        cur = self._songs.out_mode()
        try:
            idx = SONG_OUT_MODES.index(cur)
        except ValueError:
            idx = 0
        nxt = SONG_OUT_MODES[(idx + 1) % len(SONG_OUT_MODES)]
        self._songs.set_out_mode(nxt)
        if nxt in ("usb", "both"):
            name = self._songs.ensure_outport()
            if name:
                self._append_log(f"Song MIDI out: {name}")
            else:
                self._append_log("Song MIDI out: no output port found")
                if nxt == "usb":
                    self._song_status_var.set(
                        "No USB MIDI out found — plug USB→DIN (or set OUT to LOCAL)."
                    )
                    self._paint_song_controls()
                    self._mark_settings_dirty()
                    return
        self._mark_settings_dirty()
        self._refresh_song_status()

    def _song_toggle_play(self) -> None:
        if self._songs.is_playing():
            self._songs.stop()
            self._q_put(("log", "Song PLAY stop", False))
        else:
            path = self._selected_song_path()
            if self._songs.event_count() == 0 and path is not None:
                self._songs.load(path)
            if not self._songs.start():
                mode = self._songs.out_mode()
                if mode == "usb":
                    self._q_put(("log", "Song PLAY failed — no USB MIDI out", False))
                else:
                    self._q_put(("log", "Song empty — tap a file or SAVE SEQ", False))
            else:
                self._q_put(("log", "Song PLAY start", False))
        self._q_put(("song",))
        self._refresh_song_status()

    def _song_stop(self) -> None:
        if self._songs.is_playing():
            self._songs.stop()
            self._q_put(("log", "Song STOP", False))
            self._q_put(("song",))
        self._refresh_song_status()

    def _song_save_from_seq(self) -> None:
        events, loop_len = self._seq.snapshot()
        if not events or loop_len <= 0.0:
            self._song_status_var.set("Sequence is empty — record something in SEQ first.")
            return
        if self._songs.is_playing():
            self._songs.stop()
        path = self._next_take_path()
        bpm = self._songs.bpm()
        try:
            SONGS_DIR.mkdir(parents=True, exist_ok=True)
            mid = take_events_to_midifile(events, loop_len, bpm=bpm)
            # Title for list label
            if mid.tracks:
                mid.tracks[0].insert(
                    0, mido.MetaMessage("track_name", name=path.stem, time=0)
                )
            tmp = path.with_suffix(".mid.tmp")
            mid.save(str(tmp))
            tmp.replace(path)
            self._song_title_cache.pop(path.name, None)
            self._refresh_song_file_list(prefer=path.name)
            self._songs.load(path)
            self._songs.set_bpm(bpm)
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._paint_song_list()
            self._refresh_song_status()
            self._append_log(f"Song saved: {path.name} ({len(events)} events @ {int(bpm)} BPM)")
            self._song_status_var.set(f"Saved sequence → {path.name}")
        except Exception as exc:
            self._song_status_var.set(f"Save failed: {exc}")
            self._append_log(f"Song SAVE error: {exc}")

    def _song_delete_selected(self) -> None:
        path = self._selected_song_path()
        if self._songs.is_playing():
            self._songs.stop()
        if path is None:
            self._song_status_var.set("Nothing selected to delete.")
            return
        try:
            name = path.name
            path.unlink()
            self._song_title_cache.pop(name, None)
            self._songs.clear()
            self._song_selected = None
            self._refresh_song_file_list()
            # Autoload neighbor if any remain
            nxt = self._selected_song_path()
            if nxt is not None:
                self._songs.load(nxt)
            self._mark_settings_dirty()
            self._paint_song_list()
            self._refresh_song_status()
            self._append_log(f"Song deleted: {name}")
            self._song_status_var.set(f"Deleted {name}")
        except Exception as exc:
            self._song_status_var.set(f"Delete failed: {exc}")

    def _build_presets_mode(self) -> None:
        shell = self._presets_shell
        for w in shell.winfo_children():
            w.destroy()
        PRESETS_DIR.mkdir(parents=True, exist_ok=True)

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Presets", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="full session snapshot",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            shell, textvariable=self._preset_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(4, 8))

        grid = tk.Frame(shell, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=8, pady=4)
        self._preset_slot_btns = {}
        cols = 4
        for i in range(PRESET_SLOTS):
            r, c = divmod(i, cols)
            btn = self._mk_touch_btn(
                grid,
                self._preset_slot_label(i),
                lambda idx=i: self._select_preset_slot(idx),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=18)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipady=6)
            self._preset_slot_btns[i] = btn
        for c in range(cols):
            grid.grid_columnconfigure(c, weight=1)
        for r in range((PRESET_SLOTS + cols - 1) // cols):
            grid.grid_rowconfigure(r, weight=1)

        footer = tk.Frame(shell, bg="#111111")
        footer.pack(fill=tk.X, padx=8, pady=8)
        # FACTORY first so it stays visible on short panels
        self._mk_touch_btn(
            footer, "FACTORY", self._factory_reset_sound, bg="#d79921"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16)
        self._mk_touch_btn(footer, "LOAD", self._preset_load_selected, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "SAVE", self._open_save_preset, bg="#689d6a").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "DELETE", self._preset_delete_selected, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._paint_preset_slots()

    def _preset_slot_label(self, slot: int) -> str:
        path = self._preset_path(slot)
        if path.is_file():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                name = str(data.get("name") or path.stem)
                seq = data.get("seq") if isinstance(data, dict) else None
                layers = 0
                if isinstance(seq, dict):
                    sequence = seq.get("sequence")
                    if isinstance(sequence, dict):
                        layers = len(sequence.get("layers") or [])
                phrases = data.get("phrases") if isinstance(data, dict) else None
                pads_n = 0
                if isinstance(phrases, dict):
                    pads = phrases.get("pads")
                    if isinstance(pads, list):
                        pads_n = sum(
                            1
                            for p in pads
                            if isinstance(p, dict) and (p.get("events") or [])
                        )
                bits = []
                if layers:
                    bits.append(f"{layers}L")
                if pads_n:
                    bits.append(f"{pads_n}P")
                tag = " ".join(bits) if bits else "session"
                return f"{slot + 1}\n{name}\n{tag}"
            except Exception:
                return f"{slot + 1}\n{path.stem}\n(saved)"
        return f"{slot + 1}\nEMPTY"

    def _select_preset_slot(self, slot: int) -> None:
        self._preset_slot = max(0, min(PRESET_SLOTS - 1, slot))
        path = self._preset_path(self._preset_slot)
        if path.is_file():
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} selected — LOAD restores full session; SAVE overwrites."
            )
        else:
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} empty — SAVE stores the full session (name it)."
            )
        self._paint_preset_slots()

    def _paint_preset_slots(self) -> None:
        for i, btn in self._preset_slot_btns.items():
            exists = self._preset_path(i).is_file()
            selected = i == self._preset_slot
            if selected:
                color = "#b16286"
            elif exists:
                color = "#458588"
            else:
                color = "#3c3836"
            btn.configure(
                text=self._preset_slot_label(i),
                bg=color,
                activebackground=color,
            )

    def _suggested_preset_name(self) -> str:
        path = self._preset_path(self._preset_slot)
        if path.is_file():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                existing = sanitize_voice_name(str(data.get("name") or ""))
                if existing and existing != "voice":
                    return existing
            except Exception:
                pass
        a, b, blend = self.engine.morph_neighbors()
        base = suggest_voice_name(a, b, blend)
        return sanitize_voice_name(f"{base}_p{self._preset_slot + 1:02d}")[:VOICE_NAME_MAX]

    def _open_save_preset(self) -> None:
        """Name pad, then write a full-session snapshot into the selected slot."""
        if self._save_preset_open:
            return
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._mode != "presets":
            self._switch_mode("presets")
        self._save_preset_open = True
        self._save_preset_keys_digits = False
        self._presets_shell.pack_forget()

        self._save_preset_frame = tk.Frame(self._mode_host, bg="#111111")
        self._save_preset_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._save_preset_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="SAVE PRESET",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text=f"slot {self._preset_slot + 1} · full session",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            self._save_preset_frame,
            text=(
                "Stores synth, FX/drum modes, sequencer layers, phrase pads, "
                "songs selection, and the current screen — everything to restore this moment."
            ),
            font=("DejaVu Sans", 10),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, padx=8, pady=(0, 4))

        name_row = tk.Frame(self._save_preset_frame, bg="#111111")
        name_row.pack(fill=tk.X, padx=8, pady=4)
        suggested = self._suggested_preset_name()
        self._save_preset_entry = tk.Entry(
            name_row,
            font=("DejaVu Sans Mono", 18),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
        )
        self._save_preset_entry.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, ipady=10)
        self._save_preset_entry.insert(0, suggested)
        self._save_preset_entry.focus_set()

        self._save_preset_status = tk.Label(
            self._save_preset_frame,
            text=f"Will write {PRESETS_DIR.name}/slot-{self._preset_slot + 1:02d}.json as '{suggested}'",
            font=("DejaVu Sans Mono", 11),
            fg="#83a598",
            bg="#111111",
            anchor="w",
        )
        self._save_preset_status.pack(fill=tk.X, padx=8, pady=(0, 4))

        opt = tk.Frame(self._save_preset_frame, bg="#111111")
        opt.pack(fill=tk.X, padx=6, pady=2)
        self._mk_touch_btn(
            opt, "SUGGEST", self._reset_save_preset_name, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "⌫", lambda: self._save_preset_type("\b"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "CLR", lambda: self._save_preset_type("\x15"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)

        footer = tk.Frame(self._save_preset_frame, bg="#111111")
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(
            footer, "SAVE", self._confirm_save_preset, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)
        self._mk_touch_btn(
            footer, "CANCEL", self._close_save_preset, bg="#9d0006"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)

        keys = tk.Frame(self._save_preset_frame, bg="#111111")
        keys.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        self._save_preset_keys = keys
        self._paint_save_preset_keyboard()
        self._append_log(
            f"SAVE PRESET — name slot {self._preset_slot + 1} (full session)"
        )

    def _paint_save_preset_keyboard(self) -> None:
        keys = getattr(self, "_save_preset_keys", None)
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._save_preset_keys_digits:
            rows = ("1234567890", "-.")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm_")
            toggle_label = "123"
        for row in rows:
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._save_preset_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(
            fr, toggle_label, self._toggle_save_preset_keys, bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)

    def _toggle_save_preset_keys(self) -> None:
        self._save_preset_keys_digits = not self._save_preset_keys_digits
        self._paint_save_preset_keyboard()

    def _reset_save_preset_name(self) -> None:
        if self._save_preset_entry is None:
            return
        suggested = self._suggested_preset_name()
        self._save_preset_entry.delete(0, tk.END)
        self._save_preset_entry.insert(0, suggested)
        self._update_save_preset_status()

    def _save_preset_type(self, ch: str) -> None:
        entry = self._save_preset_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        elif ch == "\x15":
            entry.delete(0, tk.END)
        else:
            entry.insert(tk.END, ch)
        self._update_save_preset_status()

    def _update_save_preset_status(self) -> None:
        if self._save_preset_status is None or self._save_preset_entry is None:
            return
        name = sanitize_voice_name(self._save_preset_entry.get())
        path = self._preset_path(self._preset_slot)
        tag = "overwrite" if path.is_file() else "new"
        self._save_preset_status.configure(
            text=f"{tag}: {PRESETS_DIR.name}/{path.name} as '{name}'",
            fg="#fabd2f" if path.is_file() else "#83a598",
        )

    def _confirm_save_preset(self) -> None:
        if self._save_preset_entry is None:
            return
        name = sanitize_voice_name(self._save_preset_entry.get())
        if not name or name == "voice":
            if self._save_preset_status is not None:
                self._save_preset_status.configure(text="Need a name", fg="#fb4934")
            return
        path = self._preset_path(self._preset_slot)
        payload = self._session_dict()
        payload["name"] = name
        try:
            PRESETS_DIR.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            tmp.replace(path)
            self._active_preset_name = path.stem
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._preset_status_var.set(f"Saved '{name}' → {path.name}")
            self._append_log(f"Preset saved: {path.name} ({name})")
            self._paint_preset_slots()
            self._close_save_preset()
        except Exception as exc:
            if self._save_preset_status is not None:
                self._save_preset_status.configure(text=f"Save failed: {exc}", fg="#fb4934")
            self._append_log(f"Preset SAVE error: {exc}")

    def _close_save_preset(self, restore_main: bool = True) -> None:
        if not self._save_preset_open:
            return
        if self._save_preset_frame is not None:
            self._save_preset_frame.destroy()
            self._save_preset_frame = None
        self._save_preset_entry = None
        self._save_preset_status = None
        self._save_preset_keys = None
        self._save_preset_open = False
        if restore_main and self._mode == "presets":
            self._presets_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_preset_slots()

    def _preset_load_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        if not path.is_file():
            self._preset_status_var.set(f"Slot {self._preset_slot + 1} is empty.")
            return
        if self._load_settings_file(path):
            self._active_preset_name = path.stem
            self._refresh_ui_after_session()
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            label = path.name
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                label = str(data.get("name") or path.name)
            except Exception:
                pass
            self._preset_status_var.set(f"Loaded '{label}'")
            self._append_log(f"Preset loaded: {path.name} ({label})")
            self._paint_preset_slots()
        else:
            self._preset_status_var.set(f"Load failed: {path.name}")

    def _preset_delete_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        if not path.is_file():
            self._preset_status_var.set(f"Slot {self._preset_slot + 1} already empty.")
            return
        try:
            path.unlink()
            if self._active_preset_name == path.stem:
                self._active_preset_name = None
                self._mark_settings_dirty()
            self._preset_status_var.set(f"Deleted {path.name}")
            self._append_log(f"Preset deleted: {path.name}")
            self._paint_preset_slots()
        except Exception as exc:
            self._preset_status_var.set(f"Delete failed: {exc}")

    def _factory_reset_sound(self) -> None:
        """Hard reset to baked-in defaults (drums/FX/morph) — not a saved preset."""
        self._panic()
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)

        self.engine.reset_to_factory_defaults()
        self._full_vel = True
        self._active_preset_name = None
        self._paint_full_vel_btn()
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._sync_voice_index_from_morph()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        if not self._overlay_busy() and self._mode == "synth":
            self._paint_synth_waveform(force=True)
        self._mark_settings_dirty()
        self._save_settings_file(SETTINGS_PATH, quiet=True)
        self._preset_status_var.set(
            "FACTORY DEFAULTS — morph/tone/drums/FX reset (saved as session)"
        )
        self._append_log(
            "FACTORY DEFAULTS — drums macros, levels, tone, morph A/B, and all FX cleared"
        )
        self.last_var.set("Factory defaults restored")
        self._paint_preset_slots()

    def _paint_full_vel_btn(self) -> None:
        if self._full_vel_btn is None:
            return
        if self._full_vel:
            self._full_vel_btn.configure(
                text="FULL VEL: ON", bg="#689d6a", activebackground="#689d6a"
            )
        else:
            self._full_vel_btn.configure(
                text="FULL VEL: OFF", bg="#3c3836", activebackground="#3c3836"
            )

    def _paint_drum_lock_btn(self) -> None:
        if self._drum_lock_btn is None:
            return
        if self.engine.drum_mode():
            self._drum_lock_btn.configure(
                text="DRUM MODE: ON", bg="#d79921", activebackground="#d79921"
            )
        else:
            self._drum_lock_btn.configure(
                text="DRUM MODE", bg="#3c3836", activebackground="#3c3836"
            )

    def _paint_fx_mode_btn(self) -> None:
        if self._fx_mode_btn is None:
            return
        if self.engine.fx_mode():
            self._fx_mode_btn.configure(
                text="FX MODE: ON", bg="#b16286", activebackground="#b16286"
            )
        else:
            self._fx_mode_btn.configure(
                text="FX MODE", bg="#3c3836", activebackground="#3c3836"
            )

    def _paint_bus_fx_mode_btn(self) -> None:
        if self._bus_fx_mode_btn is None:
            return
        if self.engine.bus_fx_mode():
            self._bus_fx_mode_btn.configure(
                text="BUS FX: ON", bg="#8f3f71", activebackground="#8f3f71"
            )
        else:
            self._bus_fx_mode_btn.configure(
                text="BUS FX", bg="#3c3836", activebackground="#3c3836"
            )

    def _toggle_drum_lock(self) -> None:
        self.engine.set_drum_mode(not self.engine.drum_mode())
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log(
            "DRUM MODE ON — Knob 1–4 edit drums"
            if self.engine.drum_mode()
            else "DRUM MODE OFF — Knob 1 is morph again"
        )
        if self._kit_ui_open:
            if getattr(self, "_kit_view", "grid") == "wave":
                self._paint_kit_waveform(force=True)
            else:
                self._refresh_kit_status()
        else:
            self._paint_synth_waveform(force=True)

    def _toggle_fx_mode(self) -> None:
        # Already editing inserts but left the FX screen (e.g. opened KIT) → reopen
        if self.engine.fx_mode() and not self._fx_ui_open:
            if self._kit_ui_open:
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            else:
                self.engine.set_fx_edit_voice(None)
            self.mod_var.set(self._format_mod_line())
            self._open_fx_panel()
            return
        on = self.engine.toggle_fx_mode()
        if on:
            # KIT open → edit that drum's insert; else nearer morph wavetable.
            if self._kit_ui_open:
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            else:
                self.engine.set_fx_edit_voice(None)
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        target = self.engine.fx_edit_label() if on else ""
        self._append_log(
            f"FX MODE ON — insert FX on {target} "
            "(not the whole mix). KIT → ALL DRUMS for kit echo; tap a pad for one drum; "
            "close KIT for nearer morph voice. Use BUS FX for global wet."
            if on
            else "FX MODE OFF — knobs back to morph / tone / …"
        )
        if on:
            self._open_fx_panel()
        else:
            self._close_fx_panel()

    def _toggle_bus_fx_mode(self) -> None:
        if self.engine.bus_fx_mode() and not self._fx_ui_open:
            self.engine.set_fx_edit_bus()
            self.mod_var.set(self._format_mod_line())
            self._open_fx_panel()
            return
        on = self.engine.toggle_bus_fx_mode()
        if on:
            self.engine.set_fx_edit_bus()
        self._paint_bus_fx_mode_btn()
        self._paint_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log(
            "BUS FX ON — knobs wet the whole soft-synth mix (keys + drums + phrases). "
            "Per-voice/per-drum inserts still run underneath; use FX MODE to edit those."
            if on
            else "BUS FX OFF — knobs back to morph / tone / …"
        )
        if on:
            self._open_fx_panel()
        else:
            self._close_fx_panel()

    def _fx_param_snapshot_lines(self) -> List[str]:
        """Human-readable FX param dump for log / panel."""
        st = self.engine.modulation_state()
        delay_ms = int((0.05 + float(st.get("fx_delay_time", 0.0)) * 0.70) * 1000)
        drive = float(st.get("fx_drive", 0.0))
        fb = float(st.get("fx_delay_fb", 0.0))
        dmix = float(st.get("fx_delay_mix", 0.0))
        rsize = float(st.get("fx_reverb_size", 0.0))
        rmix = float(st.get("fx_reverb_mix", 0.0))
        return [
            f"K1 Drive       {int(drive * 127):3d}  ({drive:.2f})",
            f"K2 Delay       {delay_ms:3d} ms  ({float(st.get('fx_delay_time', 0.0)):.2f})",
            f"K3 Feedback    {int(fb * 127):3d}  ({fb:.2f})",
            f"K4 Delay mix   {int(dmix * 127):3d}  ({dmix:.2f})",
            f"K5 Reverb size {int(rsize * 127):3d}  ({rsize:.2f})",
            f"K6 Reverb mix  {int(rmix * 127):3d}  ({rmix:.2f})",
            f"K8 Synth lvl   {int(float(st.get('synth_level', st.get('level', 1.0))) * 127):3d}",
        ]

    def _open_fx_panel(self) -> None:
        """Dedicated FX knob readout — live values for insert or bus FX."""
        if not self.engine.fx_knob_focus():
            return
        if self._fx_ui_open:
            self._refresh_fx_panel()
            return

        # Close competing overlays so the FX values stay readable
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._power_ui_open:
            self._close_power_menu(restore_main=False)

        self._fx_ui_open = True
        self._fx_prev_mode = self._mode
        for shell in (
            self._synth_shell,
            self._seq_shell,
            self._pads_shell,
            self._songs_shell,
            self._presets_shell,
            self._log_shell,
        ):
            try:
                shell.pack_forget()
            except Exception:
                pass

        self._fx_frame = tk.Frame(self._mode_host, bg="#111111")
        self._fx_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._fx_frame,
            header_padx=10,
            header_pady=(12, 4),
            body_padx=10,
            body_pady=4,
            footer_padx=10,
            footer_pady=10,
        )

        self._fx_title_var = tk.StringVar(value="")
        self._fx_target_var = tk.StringVar(value="")
        tk.Label(
            header,
            textvariable=self._fx_title_var,
            font=("DejaVu Sans", 20, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            textvariable=self._fx_target_var,
            font=("DejaVu Sans", 13),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        self._mk_touch_btn(
            footer, "CLOSE", self._exit_fx_panel, bg="#504945"
        ).pack(fill=tk.X, ipady=16)

        self._fx_value_vars = {}
        rows = (
            ("drive", "K1", "Drive"),
            ("delay", "K2", "Delay"),
            ("fb", "K3", "Feedback"),
            ("dmix", "K4", "Delay mix"),
            ("rsize", "K5", "Reverb size"),
            ("rmix", "K6", "Reverb mix"),
            ("syn", "K8", "Synth lvl"),
        )
        grid = tk.Frame(body, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True)
        for i, (key, knob, name) in enumerate(rows):
            grid.rowconfigure(i, weight=1, uniform="fx")
            tk.Label(
                grid,
                text=knob,
                font=("DejaVu Sans", 14, "bold"),
                fg="#83a598",
                bg="#111111",
                width=4,
                anchor="w",
            ).grid(row=i, column=0, sticky="nsw", padx=(2, 8))
            tk.Label(
                grid,
                text=name,
                font=("DejaVu Sans", 15),
                fg="#ebdbb2",
                bg="#111111",
                anchor="w",
            ).grid(row=i, column=1, sticky="nsw", padx=(0, 12))
            var = tk.StringVar(value="—")
            self._fx_value_vars[key] = var
            tk.Label(
                grid,
                textvariable=var,
                font=("DejaVu Sans", 16, "bold"),
                fg="#fabd2f",
                bg="#111111",
                anchor="e",
            ).grid(row=i, column=2, sticky="nse", padx=(0, 4))
        grid.columnconfigure(1, weight=1)
        grid.columnconfigure(2, weight=1)

        self._refresh_fx_panel()
        for line in self._fx_param_snapshot_lines():
            self._append_log(f"FX  {line}")

    def _refresh_fx_panel(self) -> None:
        if not self._fx_ui_open:
            return
        st = self.engine.modulation_state()
        bus = self.engine.bus_fx_mode()
        if self._fx_title_var is not None:
            self._fx_title_var.set("BUS FX" if bus else "FX MODE")
        if self._fx_target_var is not None:
            if bus:
                self._fx_target_var.set("whole mix")
            else:
                self._fx_target_var.set(self.engine.fx_edit_label())

        delay_ms = int((0.05 + float(st.get("fx_delay_time", 0.0)) * 0.70) * 1000)
        vals = {
            "drive": (
                f"{int(float(st.get('fx_drive', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_drive', 0.0)):.2f})"
            ),
            "delay": f"{delay_ms:3d} ms",
            "fb": (
                f"{int(float(st.get('fx_delay_fb', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_delay_fb', 0.0)):.2f})"
            ),
            "dmix": (
                f"{int(float(st.get('fx_delay_mix', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_delay_mix', 0.0)):.2f})"
            ),
            "rsize": (
                f"{int(float(st.get('fx_reverb_size', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_reverb_size', 0.0)):.2f})"
            ),
            "rmix": (
                f"{int(float(st.get('fx_reverb_mix', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_reverb_mix', 0.0)):.2f})"
            ),
            "syn": f"{int(float(st.get('synth_level', st.get('level', 1.0))) * 127):3d}",
        }
        for key, text in vals.items():
            var = self._fx_value_vars.get(key)
            if var is not None:
                var.set(text)

    def _exit_fx_panel(self) -> None:
        """CLOSE on FX screen — leave FX edit modes and restore previous view."""
        if self.engine.fx_mode():
            self.engine.set_fx_mode(False)
        if self.engine.bus_fx_mode():
            self.engine.set_bus_fx_mode(False)
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log("FX panel closed — knobs back to morph / tone / …")
        self._close_fx_panel()

    def _close_fx_panel(self, restore_main: bool = True) -> None:
        if not self._fx_ui_open:
            return
        prev = getattr(self, "_fx_prev_mode", "synth")
        if self._fx_frame is not None:
            try:
                self._fx_frame.destroy()
            except Exception:
                pass
            self._fx_frame = None
        self._fx_ui_open = False
        self._fx_title_var = None
        self._fx_target_var = None
        self._fx_value_vars = {}
        if restore_main:
            self._switch_mode(
                prev
                if prev in ("synth", "seq", "pads", "songs", "log", "presets")
                else "synth"
            )

    def _on_synth_scope_configure(self, _event: object = None) -> None:
        """Debounce Tk Configure storms (fullscreen/layout) so we don't paint every frame."""
        canvas = self._wave_canvas
        if canvas is None:
            return
        try:
            size = (int(canvas.winfo_width()), int(canvas.winfo_height()))
        except Exception:
            return
        if size[0] < 8 or size[1] < 8:
            return
        if size == getattr(self, "_synth_scope_size", None):
            return
        self._synth_scope_size = size
        self._schedule_scope_paint("synth", blank=False)

    def _on_kit_scope_configure(self, _event: object = None) -> None:
        canvas = self._kit_wave_canvas
        if canvas is None:
            return
        try:
            size = (int(canvas.winfo_width()), int(canvas.winfo_height()))
        except Exception:
            return
        if size[0] < 8 or size[1] < 8:
            return
        if size == getattr(self, "_kit_scope_size", None):
            return
        self._kit_scope_size = size
        self._schedule_scope_paint("drum", blank=False)

    def _active_scope_canvas(self) -> Optional[tk.Canvas]:
        if self._kit_ui_open and self._kit_wave_canvas is not None:
            return self._kit_wave_canvas
        if self._mode == "synth" and not self._overlay_busy():
            return self._wave_canvas
        return None

    def _schedule_scope_paint(self, which: str = "synth", *, blank: bool = True) -> None:
        """Mark only the synth or drum scope dirty (shape changes only)."""
        if which == "drum":
            self._scope_dirty_drum = True
        else:
            self._scope_dirty_synth = True
        self._arm_scope_paint(blank=blank)

    def _arm_scope_paint(self, *, blank: bool = True) -> None:
        """Blank dirty CRT(s) immediately; coalesce the expensive redraw."""
        now = time.monotonic()
        if blank:
            if (
                self._scope_dirty_synth
                and self._wave_canvas is not None
                and self._mode == "synth"
                and not self._overlay_busy()
                and not self._scope_blanked_synth
            ):
                blank_waveform_on_canvas(self._wave_canvas)
                self._scope_blanked_synth = True
                self._scope_blanked = True
            if (
                self._scope_dirty_drum
                and self._kit_ui_open
                and self._kit_wave_canvas is not None
                and getattr(self, "_kit_view", "grid") == "wave"
                and not self._scope_blanked_drum
            ):
                blank_waveform_on_canvas(self._kit_wave_canvas)
                self._scope_blanked_drum = True
                self._scope_blanked = True
        if not self._scope_needs_paint:
            self._scope_first_dirty = now
        self._scope_needs_paint = True
        self._scope_paint_at = min(
            now + SCOPE_REDRAW_DEBOUNCE_S,
            getattr(self, "_scope_first_dirty", now) + SCOPE_REDRAW_MAX_WAIT_S,
        )

    def _flush_scope_paint(self, *, force: bool = False) -> None:
        self._scope_needs_paint = False
        paint_synth = self._scope_dirty_synth
        paint_drum = self._scope_dirty_drum
        self._scope_dirty_synth = False
        self._scope_dirty_drum = False
        if paint_drum:
            self._scope_blanked_drum = False
            self._paint_kit_waveform(force=force)
        if paint_synth:
            self._scope_blanked_synth = False
            if self._mode == "synth" and not self._overlay_busy():
                self._paint_synth_waveform(force=force)
        if not self._scope_blanked_synth and not self._scope_blanked_drum:
            self._scope_blanked = False

    def _paint_synth_waveform(self, *, force: bool = False) -> None:
        canvas = self._wave_canvas
        if canvas is None:
            return
        if self._mode != "synth" or self._overlay_busy():
            return
        try:
            samples = self.engine.morph_cycle_copy()
            if SCOPE_MORPH_CYCLES > 1 and samples is not None and len(samples) > 0:
                samples = np.tile(samples, SCOPE_MORPH_CYCLES)
            draw_waveform_on_canvas(
                canvas,
                samples,
                color=SCOPE_CRT_WAVE,
                redraw_grid=force,
            )
            self._scope_blanked_synth = False
            self._scope_blanked = self._scope_blanked_drum
            self._scope_dirty_synth = False
            if self._wave_caption is not None:
                a, b, blend = self.engine.morph_neighbors()
                if a == b:
                    cap = f"Morph · {a}"
                else:
                    cap = f"Morph · {a} → {b}  {int(blend * 100)}%"
                self._wave_caption.configure(text=cap)
        except Exception:
            if force:
                pass

    def _kit_model_selected(self) -> str:
        return drum_model_for_note(self._kit_selected_note)

    def _kit_pad_caption(self, cell: int, note: int) -> str:
        """Short readable pad label for the full-height kit grid."""
        model = drum_model_for_note(note).replace("_", " ")
        return f"{phrase_pad_label(cell)}\n{model}"

    def _refresh_kit_status(self) -> None:
        pitch, decay, noise, tone = self.engine.drum_macros()
        label = phrase_pad_label(
            max(0, min(15, self._kit_selected_note - PHRASE_PAD_BASE))
        )
        model = self._kit_model_selected().replace("_", " ")
        macros = (
            f"pitch {int(pitch * 127)} · stretch {int(decay * 127)} · "
            f"noise {int(noise * 127)} · tone {int(tone * 127)}"
        )
        if self._kit_all_drums:
            self._kit_status_var.set(
                f"ALL DRUMS · FX shared kit bus · knobs reshape body · {macros}"
            )
        elif getattr(self, "_kit_view", "grid") == "wave":
            self._kit_status_var.set(
                f"{label} · {model} · {macros} · scope {int(DRUM_SCOPE_SEC * 1000)} ms"
            )
        else:
            self._kit_status_var.set(
                f"{label} · {model} · {macros} · tap pad to play · WAVE for scope"
            )

    def _paint_kit_waveform(self, *, force: bool = False) -> None:
        canvas = self._kit_wave_canvas
        if canvas is None or not self._kit_ui_open:
            self._refresh_kit_status()
            return
        if getattr(self, "_kit_view", "grid") != "wave":
            self._refresh_kit_status()
            return
        try:
            samples = self.engine.preview_drum_waveform(self._kit_model_selected())
            draw_waveform_on_canvas(
                canvas,
                samples,
                color=SCOPE_CRT_WAVE,
                duration_sec=DRUM_SCOPE_SEC,
                redraw_grid=force,
            )
            self._scope_blanked_drum = False
            self._scope_blanked = self._scope_blanked_synth
            self._scope_dirty_drum = False
            self._refresh_kit_status()
        except Exception:
            if force:
                pass

    def _paint_kit_pad_btns(self) -> None:
        for note, btn in self._kit_btns.items():
            on = (not self._kit_all_drums) and note == self._kit_selected_note
            color = "#d79921" if on else "#3c3836"
            try:
                btn.configure(bg=color, activebackground=color)
            except Exception:
                pass
        if self._kit_all_btn is not None:
            color = "#b16286" if self._kit_all_drums else "#504945"
            try:
                self._kit_all_btn.configure(bg=color, activebackground=color)
            except Exception:
                pass

    def _select_kit_all_drums(self) -> None:
        """Point FX MODE at the shared kit-group bus (echo on all drums)."""
        self._kit_all_drums = True
        if not self.engine.fx_mode():
            self.engine.set_fx_mode(True)
            self._paint_fx_mode_btn()
            self._paint_bus_fx_mode_btn()
            self._paint_drum_lock_btn()
        self.engine.set_fx_edit_drums()
        self._paint_kit_pad_btns()
        self.mod_var.set(self._format_mod_line())
        self._refresh_kit_status()
        self._append_log("KIT — ALL DRUMS FX (shared kit bus)")

    def _select_kit_note(self, note: int, *, audition: bool = False) -> None:
        note = int(note) & 0x7F
        if note < PHRASE_PAD_BASE or note >= PHRASE_PAD_BASE + 16:
            return
        self._kit_selected_note = note
        self._kit_all_drums = False
        self._paint_kit_pad_btns()
        if self.engine.fx_mode():
            self.engine.set_fx_edit_drum(drum_model_for_note(note))
            self.mod_var.set(self._format_mod_line())
        if getattr(self, "_kit_view", "grid") == "wave":
            self._paint_kit_waveform(force=True)
        else:
            self._refresh_kit_status()
        if audition:
            self.engine.note_on(DRUM_CHANNEL, note, 110)
            self._q_put(("log", f"Kit play {drum_model_for_note(note)}", False))

    def _kit_audition_selected(self) -> None:
        self._select_kit_note(self._kit_selected_note, audition=True)

    def _set_kit_view(self, view: str) -> None:
        nxt = "wave" if view == "wave" else "grid"
        if nxt == getattr(self, "_kit_view", "grid"):
            if nxt == "grid" or self._kit_wave_canvas is not None:
                return
        self._kit_view = nxt
        if self._kit_ui_open:
            self._rebuild_kit_ui()

    def _open_kit_explorer(self) -> None:
        """Kit grid picker; WAVE drills into a CRT scope for the selected drum."""
        if self._kit_ui_open:
            return
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)

        # If insert FX MODE is on, keep drum-group target or point at selected drum.
        # If BUS FX is on, keep global edit (kit is audition/preview only).
        # Otherwise turn on DRUM MODE so knobs reshape the one-shot body.
        if self.engine.fx_mode():
            if self.engine.fx_edit_kind() == "drums":
                self._kit_all_drums = True
            else:
                self._kit_all_drums = False
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            self.mod_var.set(self._format_mod_line())
        elif self.engine.bus_fx_mode():
            self.mod_var.set(self._format_mod_line())
        elif not self.engine.drum_mode():
            self.engine.set_drum_mode(True)
            self._paint_drum_lock_btn()
            self.mod_var.set(self._format_mod_line())

        self._kit_ui_open = True
        self._kit_view = "grid"
        self._synth_shell.pack_forget()
        self._kit_frame = tk.Frame(self._mode_host, bg="#111111")
        self._kit_frame.pack(fill=tk.BOTH, expand=True)
        self._rebuild_kit_ui()
        if self.engine.fx_mode():
            self._append_log(
                "KIT — ALL DRUMS = shared kit FX; tap a pad · WAVE for scope"
            )
        else:
            self._append_log("KIT — tap a drum to play; knobs reshape it · WAVE for scope")

    def _rebuild_kit_ui(self) -> None:
        """Build grid or wave drill-down inside the kit frame (footer-first)."""
        frame = self._kit_frame
        if frame is None:
            return
        for w in frame.winfo_children():
            w.destroy()
        self._kit_btns = {}
        self._kit_all_btn = None
        self._kit_wave_canvas = None

        wave_view = getattr(self, "_kit_view", "grid") == "wave"
        header, body, footer = self._pack_screen_regions(
            frame,
            header_padx=6,
            header_pady=(6, 2),
            body_padx=4,
            body_pady=2,
            footer_padx=6,
            footer_pady=6,
        )

        title = "DRUM WAVE" if wave_view else "DRUM KIT"
        hint = (
            "knobs reshape · PLAY to hear"
            if wave_view
            else "tap pad to play · knobs reshape"
        )
        tk.Label(
            header, text=title, font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text=hint, font=("DejaVu Sans", 11),
            fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        if wave_view:
            self._mk_touch_btn(
                footer, "PLAY", self._kit_audition_selected, bg="#458588"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
            self._mk_touch_btn(
                footer, "BACK", lambda: self._set_kit_view("grid"), bg="#504945"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
            self._mk_touch_btn(
                footer, "CLOSE", self._close_kit_explorer, bg="#9d0006"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        else:
            self._kit_all_btn = self._mk_touch_btn(
                footer, "ALL DRUMS", self._select_kit_all_drums, bg="#504945"
            )
            self._kit_all_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12
            )
            self._mk_touch_btn(
                footer, "WAVE", lambda: self._set_kit_view("wave"), bg="#689d6a"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
            self._mk_touch_btn(
                footer, "CLOSE", self._close_kit_explorer, bg="#9d0006"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)

        status = tk.Label(
            body,
            textvariable=self._kit_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        )
        status.pack(side=tk.TOP, fill=tk.X, padx=4, pady=(0, 4))

        if wave_view:
            self._kit_wave_canvas = tk.Canvas(
                body,
                bg=SCOPE_CRT_BG,
                highlightthickness=1,
                highlightbackground="#14532d",
                bd=0,
            )
            self._kit_wave_canvas.pack(
                side=tk.TOP, fill=tk.BOTH, expand=True, padx=4, pady=2
            )
            self._kit_wave_canvas.bind("<Configure>", self._on_kit_scope_configure)
            self._paint_kit_waveform(force=True)
        else:
            grid = tk.Frame(body, bg="#111111")
            grid.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            for r in range(4):
                grid.rowconfigure(r, weight=1)
            for c in range(4):
                grid.columnconfigure(c, weight=1)
            for i, cell in enumerate(PHRASE_GRID_CELLS):
                note = mpk_note_for_phrase_cell(cell)
                r, c = divmod(i, 4)
                btn = self._mk_touch_btn(
                    grid,
                    self._kit_pad_caption(cell, note),
                    lambda n=note: self._select_kit_note(n, audition=True),
                    bg="#3c3836",
                )
                btn.configure(font=("DejaVu Sans", 14, "bold"), pady=4)
                btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3)
                self._kit_btns[note] = btn
            self._paint_kit_pad_btns()
            self._refresh_kit_status()

    def _close_kit_explorer(self, restore_main: bool = True) -> None:
        if not self._kit_ui_open:
            return
        if self._kit_frame is not None:
            self._kit_frame.destroy()
            self._kit_frame = None
        self._kit_btns = {}
        self._kit_all_btn = None
        self._kit_wave_canvas = None
        self._kit_view = "grid"
        self._kit_ui_open = False
        # Leaving KIT: keep shared-kit FX edit; single-drum insert → nearer morph voice.
        if self.engine.fx_mode():
            if self.engine.fx_edit_kind() == "drum":
                self._kit_all_drums = False
                self.engine.set_fx_edit_voice(None)
            # kind == "drums" stays so you can keep twisting kit echo after CLOSE
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)

    def _build_log_mode(self) -> None:
        shell = self._log_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Event log", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="MIDI + UI actions",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        self._log_last_lbl = tk.Label(
            shell, textvariable=self.last_var,
            font=("DejaVu Sans Mono", 14, "bold"), fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        self._log_last_lbl.pack(fill=tk.X, padx=10, pady=(4, 6))

        body = tk.Frame(shell, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)
        self.log = tk.Text(
            body, font=("DejaVu Sans Mono", 13),
            bg="#1d2021", fg="#ebdbb2", insertbackground="#ebdbb2",
            relief=tk.FLAT, state=tk.DISABLED,
        )
        scroll = ttk.Scrollbar(body, command=self.log.yview)
        self.log.configure(yscrollcommand=scroll.set)
        self.log.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        scroll.pack(side=tk.RIGHT, fill=tk.Y)

        footer = tk.Frame(shell, bg="#111111")
        footer.pack(fill=tk.X, padx=8, pady=8)
        self._mk_touch_btn(footer, "CLEAR LOG", self._clear_log, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )

    def _build_home_mode(self) -> None:
        shell = self._home_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Home", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="tap a mode",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        body = tk.Frame(shell, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=8, pady=8)
        rows = (HOME_TILES[:4], HOME_TILES[4:])
        for spec in rows:
            row = tk.Frame(body, bg="#111111")
            row.pack(fill=tk.BOTH, expand=True, pady=4)
            for key, title, subtitle, color in spec:
                btn = self._mk_touch_btn(
                    row,
                    f"{title}\n{subtitle}",
                    lambda m=key: self._switch_mode(m),
                    bg=color,
                )
                btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22, justify=tk.CENTER)
                btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

    def _build_settings_mode(self) -> None:
        shell = self._settings_shell
        for w in shell.winfo_children():
            w.destroy()

        header, body, footer = self._pack_screen_regions(
            shell,
            header_padx=8,
            header_pady=(10, 4),
            body_padx=10,
            body_pady=4,
            footer_padx=8,
            footer_pady=8,
        )
        tk.Label(
            header, text="Settings", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="software update",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            body,
            textvariable=self._settings_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="nw",
        ).pack(fill=tk.BOTH, expand=True, pady=(4, 8))

        tk.Label(
            body,
            text=(
                "CHECK looks at GitHub master. UPDATE deploys the whole repo "
                "(kiosk, crates, presets) like SSH, then restarts. "
                "Songs, phrases, and settings.json stay. Rust binaries are not rebuilt on the Pi."
            ),
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, pady=(0, 4))

        row = tk.Frame(footer, bg="#111111")
        row.pack(fill=tk.X, pady=(0, 6))
        self._settings_check_btn = self._mk_touch_btn(
            row, "CHECK", self._settings_check, bg="#458588"
        )
        self._settings_check_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
        )
        self._settings_update_btn = self._mk_touch_btn(
            row, "UPDATE", self._settings_update, bg="#689d6a"
        )
        self._settings_update_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
        )
        self._mk_touch_btn(footer, "TOKEN", self._open_update_token, bg="#504945").pack(
            fill=tk.X, ipady=10
        )
        self._paint_settings_buttons()

    def _paint_settings_buttons(self) -> None:
        check_btn = self._settings_check_btn
        update_btn = self._settings_update_btn
        if check_btn is None or update_btn is None:
            return
        if self._update_busy:
            check_btn.configure(text="WORKING…", bg="#3c3836", activebackground="#3c3836")
            update_btn.configure(text="WORKING…", bg="#3c3836", activebackground="#3c3836")
            return
        check_btn.configure(text="CHECK", bg="#458588", activebackground="#458588")
        if self._update_confirming:
            update_btn.configure(
                text="INSTALL NOW", bg="#9d0006", activebackground="#9d0006"
            )
            check_btn.configure(text="CANCEL", bg="#504945", activebackground="#504945")
            return
        available = bool(self._update_check and self._update_check.available)
        color = "#689d6a" if available or self._update_check is None else "#3c3836"
        update_btn.configure(text="UPDATE", bg=color, activebackground=color)

    def _refresh_settings_status(self) -> None:
        self._settings_status_var.set(
            updater.format_status_lines(self._update_check)
        )
        self._paint_settings_buttons()

    def _settings_check(self) -> None:
        if self._token_ui_open:
            return
        if self._update_confirming:
            self._update_confirming = False
            self._refresh_settings_status()
            return
        if self._update_busy:
            return
        self._update_busy = True
        self._settings_status_var.set("Checking GitHub for the latest master…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_check_worker, daemon=True).start()

    def _settings_check_worker(self) -> None:
        try:
            result = updater.check_for_update()
            self._q_put(("update", result, None))
        except Exception as exc:
            self._q_put(("update", None, str(exc)))

    def _settings_update(self) -> None:
        if self._token_ui_open or self._update_busy:
            return
        if not self._update_confirming:
            # CHECK first if we have not looked yet, then ask for a second tap.
            if self._update_check is None:
                self._settings_status_var.set("Checking before install…")
                self._update_busy = True
                self._paint_settings_buttons()

                def _check_then_confirm() -> None:
                    try:
                        result = updater.check_for_update()
                        self._q_put(("update", result, "confirm" if result.available else None))
                    except Exception as exc:
                        self._q_put(("update", None, str(exc)))

                threading.Thread(target=_check_then_confirm, daemon=True).start()
                return
            if self._update_check.error:
                self._settings_status_var.set(self._update_check.error)
                return
            if not self._update_check.available:
                self._settings_status_var.set(self._update_check.message or "Already on latest.")
                return
            self._update_confirming = True
            self._settings_status_var.set(
                "This deploys the whole repo from GitHub, then restarts.\n"
                "Songs, presets, phrases, and settings.json stay.\n"
                "Tap INSTALL NOW to continue, or CANCEL."
            )
            self._paint_settings_buttons()
            return
        self._update_confirming = False
        self._update_busy = True
        self._settings_status_var.set("Installing update…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_apply_worker, daemon=True).start()

    def _settings_apply_worker(self) -> None:
        expected = ""
        if self._update_check and self._update_check.remote.sha:
            expected = self._update_check.remote.sha

        def progress(msg: str) -> None:
            self._q_put(("update_progress", msg))

        try:
            info = updater.apply_update(progress=progress, expected_sha=expected)
            self._q_put(("update_done", info, None))
        except Exception as exc:
            self._q_put(("update_done", None, str(exc)))

    def _restart_after_update(self) -> None:
        """Stop audio, keep the singleton lock, exec the new midi_tone.py."""
        self._append_log("Update installed — restarting…")
        try:
            self._panic()
        except Exception:
            pass
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        try:
            self._songs.stop()
            self._songs.close_outport()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        except Exception:
            pass
        if self._inport is not None:
            try:
                self._inport.close()
            except Exception:
                pass
        try:
            self.engine.stop()
        except Exception:
            pass
        try:
            self.root.update_idletasks()
        except Exception:
            pass
        try:
            self.root.destroy()
        except Exception:
            pass
        try:
            updater.restart_current_process()
        except Exception as exc:
            print(f"re-exec failed ({exc}); exiting for kiosk restart", flush=True)
            sys.exit(0)

    def _open_update_token(self) -> None:
        if self._token_ui_open or self._update_busy:
            return
        if self._mode != "settings":
            self._switch_mode("settings")
        self._token_ui_open = True
        self._token_keys_digits = False
        self._settings_shell.pack_forget()
        self._token_frame = tk.Frame(self._mode_host, bg="#111111")
        self._token_frame.pack(fill=tk.BOTH, expand=True)
        header, body, footer = self._pack_screen_regions(
            self._token_frame,
            header_padx=8,
            header_pady=(8, 2),
            body_padx=8,
            body_pady=4,
            footer_padx=8,
            footer_pady=8,
        )
        tk.Label(
            header,
            text="GITHUB TOKEN",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text="private repo access",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)
        tk.Label(
            body,
            text="Fine-grained PAT with Contents: Read on this repo. Stored only on this box.",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, pady=(0, 4))
        self._token_entry = tk.Entry(
            body,
            font=("DejaVu Sans Mono", 16),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
            show="•",
        )
        self._token_entry.pack(fill=tk.X, ipady=10, pady=(0, 6))
        self._token_entry.focus_set()
        self._token_keys = tk.Frame(body, bg="#111111")
        self._token_keys.pack(fill=tk.BOTH, expand=True)
        self._paint_token_keyboard()
        self._mk_touch_btn(footer, "SAVE", self._save_update_token, bg="#689d6a").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )
        self._mk_touch_btn(footer, "CANCEL", self._close_update_token, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )

    def _paint_token_keyboard(self) -> None:
        keys = self._token_keys
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._token_keys_digits:
            rows = ("1234567890", "-_./")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm")
            toggle_label = "123"
        for row in rows:
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._token_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(fr, toggle_label, self._toggle_token_keys, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4
        )
        self._mk_touch_btn(fr, "⌫", lambda: self._token_type("\b"), bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4
        )

    def _toggle_token_keys(self) -> None:
        self._token_keys_digits = not self._token_keys_digits
        self._paint_token_keyboard()

    def _token_type(self, ch: str) -> None:
        entry = self._token_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        else:
            entry.insert(tk.END, ch)

    def _save_update_token(self) -> None:
        entry = self._token_entry
        token = (entry.get() if entry is not None else "").strip()
        if not token:
            self._settings_status_var.set("Token was empty — cancelled.")
            self._close_update_token()
            return
        try:
            updater.save_token(token)
        except Exception as exc:
            self._settings_status_var.set(f"Could not save token: {exc}")
            self._close_update_token()
            return
        self._close_update_token()
        self._append_log("GitHub token saved for SET → UPDATE")
        self._settings_status_var.set("Token saved. Tap CHECK to look at GitHub.")
        self._update_check = None
        self._paint_settings_buttons()

    def _close_update_token(self, restore_main: bool = True) -> None:
        if not self._token_ui_open:
            return
        if self._token_frame is not None:
            self._token_frame.destroy()
            self._token_frame = None
        self._token_entry = None
        self._token_keys = None
        self._token_ui_open = False
        if restore_main:
            self._switch_mode("settings")

    def _build_seq_mode(self) -> None:
        shell = self._seq_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="Sequencer", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="drums + keys · free timing · overdub layers",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            shell, textvariable=self._seq_status_var,
            font=("DejaVu Sans", 14, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(6, 2))

        layers = tk.Label(
            shell, textvariable=self._seq_layer_var,
            font=("DejaVu Sans Mono", 11),
            fg="#83a598", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        layers.pack(fill=tk.X, padx=10, pady=(0, 6))

        # Button rows claim their strips first, so REC/PLAY keep full height and
        # the how-to line at the very end is the only thing a short panel drops.
        row4 = tk.Frame(shell, bg="#111111")
        row4.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(4, 2))
        self._mk_touch_btn(row4, "STOP ALL", self._seq_stop, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._seq_to_pad_btn = self._mk_touch_btn(
            row4, "→ PAD", self._seq_assign_to_pad, bg="#458588"
        )
        self._seq_to_pad_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10)
        self._mk_touch_btn(row4, "CLEAR", self._seq_clear, bg="#3c3836").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._mk_touch_btn(row4, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )

        row3 = tk.Frame(shell, bg="#111111")
        row3.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=4)
        self._mk_touch_btn(row3, "LEN ×2", self._seq_double, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._mk_touch_btn(row3, "LEN ÷2", self._seq_halve, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._seq_extend_btn = self._mk_touch_btn(
            row3, "OVERDUB: WRAP", self._seq_toggle_extend, bg="#3c3836"
        )
        self._seq_extend_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10)

        row2 = tk.Frame(shell, bg="#111111")
        row2.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=4)
        self._seq_keep_btn = self._mk_touch_btn(row2, "KEEP", self._seq_keep, bg="#458588")
        self._seq_keep_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)
        self._seq_drop_btn = self._mk_touch_btn(row2, "DROP", self._seq_drop, bg="#3c3836")
        self._seq_drop_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)
        self._seq_undo_btn = self._mk_touch_btn(row2, "UNDO", self._seq_undo, bg="#3c3836")
        self._seq_undo_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)

        # Transport — the two buttons you hit while playing
        row1 = tk.Frame(shell, bg="#111111")
        row1.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)
        self._seq_rec_btn = self._mk_touch_btn(
            row1, "REC BACKBONE", self._seq_toggle_record, bg="#9d0006"
        )
        self._seq_rec_btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22)
        self._seq_rec_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        self._seq_play_btn = self._mk_touch_btn(
            row1, "PLAY", self._seq_toggle_play, bg="#689d6a"
        )
        self._seq_play_btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22)
        self._seq_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        tip = tk.Label(
            shell,
            text=(
                "1) REC + play the groove → REC again locks the loop length  "
                "2) it loops; REC again to overdub drums or keys  "
                "3) KEEP flattens the layer, DROP throws it away, UNDO peels the last one off  "
                "4) → PAD then tap a square or MPK pad to drop the sequence onto a phrase clip"
            ),
            font=("DejaVu Sans", 10), fg="#83a598", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        tip.pack(side=tk.BOTTOM, fill=tk.X, padx=10, pady=(2, 6))
        self._paint_seq_buttons()

    def _build_pads_mode(self) -> None:
        shell = self._pads_shell
        for w in shell.winfo_children():
            w.destroy()
        self._phrase_pad_btns.clear()
        self._phrase_view_btns.clear()
        self._phrase_clear_btn = None
        self._phrase_mode_btn = None
        self._phrase_out_btn = None
        self._phrase_trig_btn = None
        self._phrase_voice_btn = None
        self._phrase_ch_btn = None
        self._phrase_synth_btn = None
        self._phrase_vib_btn = None

        play_view = self._pads_view == "play"

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(6, 2))
        tk.Label(
            header, text="Phrase Pads", font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        view_row = tk.Frame(header, bg="#111111")
        view_row.pack(side=tk.RIGHT)
        for key, label in (("play", "PLAY"), ("edit", "EDIT")):
            btn = self._mk_touch_btn(
                view_row, label, lambda v=key: self._phrase_set_view(v), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 11, "bold"), padx=10, pady=4)
            btn.pack(side=tk.LEFT, padx=2)
            self._phrase_view_btns[key] = btn

        status = tk.Label(
            shell, textvariable=self._phrase_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(2, 4))

        # Control rows are packed from the bottom *before* the grid, so a short
        # screen shrinks the pad squares instead of pushing the row off-screen.
        if play_view:
            row = tk.Frame(shell, bg="#111111")
            row.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(4, 6))
            self._mk_touch_btn(row, "STOP ALL", self._phrase_stop_all, bg="#3c3836").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10
            )
            self._phrase_out_btn = self._mk_touch_btn(
                row, "OUT: LOCAL", self._phrase_cycle_out_mode, bg="#504945"
            )
            self._phrase_out_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10
            )
        else:
            # Bottom-up: detail row lands under the transport row
            detail = tk.Frame(shell, bg="#111111")
            detail.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(2, 6))
            row = tk.Frame(shell, bg="#111111")
            row.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(4, 2))
            self._mk_touch_btn(row, "STOP REC", self._phrase_stop_rec, bg="#504945").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_mode_btn = self._mk_touch_btn(
                row, "MODE", self._phrase_toggle_mode_arm, bg="#458588"
            )
            self._phrase_mode_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_clear_btn = self._mk_touch_btn(
                row, "CLEAR", self._phrase_toggle_clear, bg="#9d0006"
            )
            self._phrase_clear_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._mk_touch_btn(row, "STOP ALL", self._phrase_stop_all, bg="#3c3836").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            # Session routing lives with the transport; the row below is per pad
            self._phrase_out_btn = self._mk_touch_btn(
                row, "OUT: LOCAL", self._phrase_cycle_out_mode, bg="#504945"
            )
            self._phrase_out_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )

            self._phrase_trig_btn = self._mk_touch_btn(
                detail, "TRIG", self._phrase_edit_trig, bg="#458588"
            )
            self._phrase_trig_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_voice_btn = self._mk_touch_btn(
                detail, "FOLLOW", self._phrase_edit_voice, bg="#689d6a"
            )
            self._phrase_voice_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_ch_btn = self._mk_touch_btn(
                detail, "CH:rec", self._phrase_edit_channel, bg="#b16286"
            )
            self._phrase_ch_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_synth_btn = self._mk_touch_btn(
                detail, "SYNTH", self._phrase_edit_synth, bg="#d65d0e"
            )
            self._phrase_synth_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_vib_btn = self._mk_touch_btn(
                detail, "VIB live", self._phrase_edit_vib, bg="#458588"
            )
            self._phrase_vib_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            # Narrow trim pair — balance one pad against the rest of the mix
            for label, delta in (("VOL−", -PHRASE_GAIN_STEP), ("VOL+", PHRASE_GAIN_STEP)):
                btn = self._mk_touch_btn(
                    detail, label, lambda d=delta: self._phrase_edit_gain(d), bg="#98971a"
                )
                btn.configure(padx=4)
                btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=2, ipady=8)

        grid = tk.Frame(shell, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=6, pady=2)
        for row_idx in range(4):
            grid.rowconfigure(row_idx, weight=1)
        for col in range(4):
            grid.columnconfigure(col, weight=1)
        for i, cell in enumerate(PHRASE_GRID_CELLS):
            r, c = divmod(i, 4)
            btn = self._mk_touch_btn(
                grid,
                phrase_pad_label(cell),
                lambda idx=cell: self._phrase_pad_tap(idx),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=10)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3)
            self._phrase_pad_btns[cell] = btn

        self._paint_phrase_pads()

    def _phrase_set_view(self, view: str) -> None:
        nxt = "play" if view == "play" else "edit"
        if nxt == self._pads_view and self._phrase_pad_btns:
            return
        if nxt == "play":
            self._phrase_clear_armed = False
            self._phrase_mode_armed = False
            if self._phrases.is_recording():
                self._phrases.stop_record()
        self._pads_view = nxt
        self._mark_settings_dirty()
        self._build_pads_mode()

    def _phrase_pad_tap(self, idx: int) -> None:
        if self._seq_to_pad_armed:
            self._finish_seq_to_pad(idx)
            return
        if self._pads_view == "edit" and self._phrase_mode_armed:
            self._phrases.toggle_trigger_mode(idx)
            self._phrase_mode_armed = False
            self._paint_phrase_pads()
            return
        if self._pads_view == "edit" and self._phrase_clear_armed:
            self._phrases.clear_cell(idx)
            self._phrase_clear_armed = False
            self._paint_phrase_pads()
            return
        self._phrases.handle_pad(
            idx, from_touch=True, allow_record=(self._pads_view == "edit")
        )
        self._paint_phrase_pads()

    def _drum_pad_is_phrase_control(self, cell: int) -> bool:
        """True when a ch10 pad should launch/arm a phrase instead of playing a drum.

        EDIT used to swallow *every* MPK pad while any cell was recording, so the
        only way to fire a filled clip was the touch square. Filled cells still
        launch from hardware; the recording cell (and other empties) stay drums.
        """
        if not (0 <= cell < PHRASE_PAD_COUNT):
            return False
        rec = self._phrases.recording_cell()
        if rec is not None and rec == cell:
            return False
        if rec is not None and self._phrases.cell(cell).is_empty():
            return False
        return True

    def _on_pad_midi(self, cell: int, note: int, velocity: int) -> None:
        """UI-thread handler for an MPK pad that is acting as a phrase trigger."""
        edit_view = self._pads_view == "edit"
        if edit_view and self._phrase_mode_armed:
            mode = self._phrases.toggle_trigger_mode(cell)
            self._phrase_mode_armed = False
            self._paint_phrase_pads()
            self._append_log(
                f"Pad→MODE {phrase_pad_label(cell)} → "
                f"{'LOOP' if mode == PHRASE_TRIG_LOOP else 'ONE-SHOT'}"
            )
            return
        if edit_view and self._phrase_clear_armed:
            self._phrases.clear_cell(cell)
            self._phrase_clear_armed = False
            self._paint_phrase_pads()
            self._append_log(f"Pad→CLEAR {phrase_pad_label(cell)}  note {note}")
            return
        action = self._phrases.handle_pad(
            cell, from_touch=False, allow_record=edit_view
        )
        self._paint_phrase_pads()
        self._append_log(
            f"Pad→Phrase {phrase_pad_label(cell)} ({action})  "
            f"note {note}  vel {velocity}"
        )

    def _phrase_stop_rec(self) -> None:
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        if self._phrases.is_recording():
            self._phrases.stop_record()
        self._paint_phrase_pads()

    def _phrase_stop_all(self) -> None:
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._phrases.stop_all()
        self._paint_phrase_pads()

    def _phrase_toggle_clear(self) -> None:
        """Arm CLEAR: next pad tap erases that cell (cancel by tapping CLEAR again)."""
        if self._pads_view != "edit":
            return
        if self._phrases.is_recording():
            self._phrase_status_var.set("Stop recording before CLEAR")
            return
        self._phrase_mode_armed = False
        self._phrase_clear_armed = not self._phrase_clear_armed
        self._paint_phrase_pads()

    def _phrase_toggle_mode_arm(self) -> None:
        """Arm MODE: next pad tap toggles ONE-SHOT ↔ LOOP."""
        if self._pads_view != "edit":
            return
        if self._phrases.is_recording():
            self._phrase_status_var.set("Stop recording before MODE")
            return
        self._phrase_clear_armed = False
        self._phrase_mode_armed = not self._phrase_mode_armed
        self._paint_phrase_pads()

    def _phrase_selected_or_status(self) -> Optional[int]:
        sel = self._phrases.selected()
        if sel is None:
            self._phrase_status_var.set("Select a pad first (tap a square)")
            return None
        return sel

    def _phrase_edit_trig(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_trigger_mode(sel)
        self._paint_phrase_pads()

    def _phrase_edit_voice(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_voice_lock(sel)
        self._paint_phrase_pads()

    def _phrase_edit_channel(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.cycle_out_channel(sel)
        self._paint_phrase_pads()

    def _phrase_edit_synth(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_local_synth(sel)
        self._paint_phrase_pads()

    def _phrase_edit_vib(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_vib_baked(sel)
        self._paint_phrase_pads()

    def _phrase_edit_gain(self, delta: float) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.nudge_gain(sel, delta)
        self._paint_phrase_pads()

    def _phrase_cycle_out_mode(self) -> None:
        cur = self._phrase_out_mode if self._phrase_out_mode in SONG_OUT_MODES else "local"
        try:
            idx = SONG_OUT_MODES.index(cur)
        except ValueError:
            idx = 0
        nxt = SONG_OUT_MODES[(idx + 1) % len(SONG_OUT_MODES)]
        self._phrase_out_mode = nxt
        if nxt in ("usb", "both"):
            name = self._songs.ensure_outport()
            if name:
                self._append_log(f"Pads MIDI out → {name}")
            else:
                self._append_log("Pads MIDI out: no USB port found")
        self._mark_settings_dirty()
        self._paint_phrase_pads()

    def _paint_phrase_pads(self) -> None:
        clear_armed = bool(self._phrase_clear_armed) and self._pads_view == "edit"
        mode_armed = bool(self._phrase_mode_armed) and self._pads_view == "edit"
        assign_armed = bool(self._seq_to_pad_armed)
        self._phrase_status_var.set(
            self._phrases.status_line(
                clear_armed=clear_armed,
                mode_armed=mode_armed,
                assign_armed=assign_armed,
                view=self._pads_view,
            )
        )
        rec = self._phrases.recording_cell()
        selected = self._phrases.selected()
        playing = set(self._phrases.playing_cells())
        for idx, btn in self._phrase_pad_btns.items():
            cell = self._phrases.cell(idx)
            label = phrase_pad_label(idx)
            loop = cell.is_loop()
            mode_mark = "↻" if loop else "▶"
            lock_mark = "·" if cell.is_voice_locked() else ""
            if mode_armed:
                text = f"{label}\n{mode_mark}?"
                color = "#b16286"
            elif assign_armed:
                text = f"{label}\nDROP?"
                color = "#458588"
            elif clear_armed:
                text = f"{label}\nCLR?" if not cell.is_empty() else f"{label}\n—"
                color = "#cc241d" if not cell.is_empty() else "#504945"
            elif rec == idx:
                text = f"{label}\nREC"
                color = "#9d0006"
            elif idx in playing:
                secs = cell.length
                text = f"{label}\n{mode_mark}{lock_mark} {secs:.1f}s"
                color = "#689d6a"
            elif cell.is_empty():
                text = f"{label}\n{mode_mark} —" if loop else f"{label}\n—"
                color = "#504945" if loop else "#3c3836"
            else:
                text = f"{label}\n{mode_mark}{lock_mark} {cell.length:.1f}s"
                color = "#689d6a" if loop else "#458588"
            if (
                not clear_armed
                and not mode_armed
                and not assign_armed
                and selected == idx
                and rec != idx
                and idx not in playing
            ):
                if not cell.is_empty() or self._pads_view == "edit":
                    color = "#076678"
            try:
                btn.configure(text=text, bg=color, activebackground=color)
            except Exception:
                pass

        for key, btn in self._phrase_view_btns.items():
            on = key == self._pads_view
            bg = "#458588" if on else "#3c3836"
            try:
                btn.configure(bg=bg, activebackground=bg)
            except Exception:
                pass

        if self._phrase_out_btn is not None:
            mode = str(self._phrase_out_mode or "local").upper()
            obg = "#689d6a" if mode != "LOCAL" else "#504945"
            try:
                self._phrase_out_btn.configure(
                    text=f"OUT: {mode}",
                    bg=obg,
                    activebackground=obg,
                )
            except Exception:
                pass

        if self._phrase_clear_btn is not None:
            cbg = "#fb4934" if clear_armed else "#9d0006"
            try:
                self._phrase_clear_btn.configure(
                    text="CLEAR…" if clear_armed else "CLEAR",
                    bg=cbg,
                    activebackground=cbg,
                )
            except Exception:
                pass
        if self._phrase_mode_btn is not None:
            mbg = "#d3869b" if mode_armed else "#458588"
            try:
                self._phrase_mode_btn.configure(
                    text="MODE…" if mode_armed else "MODE",
                    bg=mbg,
                    activebackground=mbg,
                )
            except Exception:
                pass

        if selected is not None:
            cell = self._phrases.cell(selected)
            if self._phrase_trig_btn is not None:
                t = "LOOP" if cell.is_loop() else "1SHOT"
                try:
                    self._phrase_trig_btn.configure(text=t)
                except Exception:
                    pass
            if self._phrase_voice_btn is not None:
                trim = int(round(cell.gain * 100))
                v = f"LOCK {trim}%" if cell.is_voice_locked() else "FOLLOW"
                if not cell.is_voice_locked() and trim != 100:
                    v = f"FOLLOW {trim}%"
                vbg = "#b16286" if cell.is_voice_locked() else "#689d6a"
                try:
                    self._phrase_voice_btn.configure(
                        text=v, bg=vbg, activebackground=vbg
                    )
                except Exception:
                    pass
            if self._phrase_ch_btn is not None:
                ch = "CH:rec" if cell.out_channel < 0 else f"CH:{cell.out_channel + 1}"
                try:
                    self._phrase_ch_btn.configure(text=ch)
                except Exception:
                    pass
            if self._phrase_synth_btn is not None:
                s = "SYNTH" if cell.local_synth else "MIDI"
                sbg = "#d65d0e" if cell.local_synth else "#504945"
                try:
                    self._phrase_synth_btn.configure(
                        text=s, bg=sbg, activebackground=sbg
                    )
                except Exception:
                    pass
            if self._phrase_vib_btn is not None:
                vbg2 = "#458588" if cell.vib_baked else "#3c3836"
                try:
                    self._phrase_vib_btn.configure(
                        text=f"VIB {cell.vib_label()}", bg=vbg2, activebackground=vbg2
                    )
                except Exception:
                    pass

    def _refresh_phrase_status(self) -> None:
        if self._mode == "pads":
            self._paint_phrase_pads()
        else:
            self._phrase_status_var.set(
                self._phrases.status_line(
                    clear_armed=self._phrase_clear_armed,
                    mode_armed=self._phrase_mode_armed,
                    assign_armed=self._seq_to_pad_armed,
                    view=self._pads_view,
                )
            )

    def _switch_mode(self, mode: str) -> None:
        mode = mode if mode in UI_MODES else "synth"
        # Close synth-only overlays before swapping shells
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._token_ui_open:
            self._close_update_token(restore_main=False)

        # Leaving pads while recording: keep the take
        if self._mode == "pads" and mode != "pads":
            if self._phrases.is_recording():
                self._phrases.stop_record()
            self._phrase_clear_armed = False
            self._phrase_mode_armed = False
            if mode != "seq":
                self._seq_to_pad_armed = False
        if mode not in ("pads", "seq"):
            self._seq_to_pad_armed = False

        self._mode = mode
        self._home_shell.pack_forget()
        self._synth_shell.pack_forget()
        self._seq_shell.pack_forget()
        self._pads_shell.pack_forget()
        self._songs_shell.pack_forget()
        self._presets_shell.pack_forget()
        self._log_shell.pack_forget()
        self._settings_shell.pack_forget()
        if self._grid_frame is not None:
            self._grid_frame.pack_forget()
        if self._morph_frame is not None:
            self._morph_frame.pack_forget()
        if self._kit_frame is not None:
            self._kit_frame.pack_forget()
        if self._fx_frame is not None:
            try:
                self._fx_frame.pack_forget()
            except Exception:
                pass
        if self._save_preset_frame is not None:
            try:
                self._save_preset_frame.pack_forget()
            except Exception:
                pass
        if self._save_voice_frame is not None:
            try:
                self._save_voice_frame.pack_forget()
            except Exception:
                pass

        if mode == "home":
            self._home_shell.pack(fill=tk.BOTH, expand=True)
        elif mode == "seq":
            self._seq_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_seq_status()
        elif mode == "pads":
            self._pads_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_phrase_pads()
        elif mode == "songs":
            self._songs_shell.pack(fill=tk.BOTH, expand=True)
            # Rescan directory each visit so dropped-in .mid files appear
            self._paint_song_slots()
            self._refresh_song_status()
        elif mode == "presets":
            self._presets_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_preset_slots()
        elif mode == "log":
            self._log_shell.pack(fill=tk.BOTH, expand=True)
            try:
                self.log.see(tk.END)
            except Exception:
                pass
        elif mode == "settings":
            self._settings_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_settings_status()
        else:
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            # Ensure synth children are packed (overlays may have forgotten them)
            try:
                self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            except Exception:
                pass
            try:
                self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            except Exception:
                pass
            self._paint_synth_waveform(force=True)
        self._paint_mode_btns()

    def _paint_mode_btns(self) -> None:
        jam = self._mode in JAM_NAV_MODES
        for key, btn in self._jam_btns.items():
            if jam:
                if not btn.winfo_ismapped():
                    btn.pack(side=tk.LEFT, padx=1)
            else:
                btn.pack_forget()
            on = jam and key == self._mode
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)
        home = self._mode_btns.get("home")
        if home is not None:
            color = "#458588" if self._mode == "home" else "#3c3836"
            home.configure(bg=color, activebackground=color)

    def _refresh_seq_status(self) -> None:
        self._seq_status_var.set(self._seq.status_line())
        self._seq_layer_var.set(self._seq.layer_line())
        self._paint_seq_buttons()

    def _paint_seq_buttons(self) -> None:
        st = self._seq.status()
        state = str(st["state"])
        pending = int(st["pending"]) > 0 or state == SEQ_OVERDUB
        if self._seq_rec_btn is not None:
            if state == SEQ_REC_BACKBONE:
                text, color = "● STOP REC", "#cc241d"
            elif state == SEQ_OVERDUB:
                text, color = "● STOP OVERDUB", "#cc241d"
            elif state == SEQ_EMPTY:
                text, color = "REC BACKBONE", "#9d0006"
            else:
                text, color = "REC OVERDUB", "#9d0006"
            self._seq_rec_btn.configure(text=text, bg=color, activebackground=color)
        if self._seq_play_btn is not None:
            if self._seq.is_playing():
                self._seq_play_btn.configure(
                    text="■ STOP", bg="#d79921", activebackground="#d79921"
                )
            elif state == SEQ_EMPTY:
                # Nothing to run yet — REC should be the only lit way forward
                self._seq_play_btn.configure(
                    text="PLAY", bg="#3c3836", activebackground="#3c3836"
                )
            else:
                self._seq_play_btn.configure(
                    text="PLAY", bg="#689d6a", activebackground="#689d6a"
                )
        for btn, live in (
            (self._seq_keep_btn, pending),
            (self._seq_drop_btn, pending),
            (self._seq_undo_btn, int(st["layers"]) > 1),
        ):
            if btn is None:
                continue
            base = "#458588" if btn is self._seq_keep_btn else "#665c54"
            color = base if live else "#3c3836"
            btn.configure(bg=color, activebackground=color)
        if self._seq_extend_btn is not None:
            on = bool(st["extend"])
            color = "#b16286" if on else "#3c3836"
            self._seq_extend_btn.configure(
                text="OVERDUB: EXTEND" if on else "OVERDUB: WRAP",
                bg=color,
                activebackground=color,
            )
        if self._seq_to_pad_btn is not None:
            armed = bool(self._seq_to_pad_armed)
            color = "#83a598" if armed else "#458588"
            self._seq_to_pad_btn.configure(
                text="→ PAD…" if armed else "→ PAD",
                bg=color,
                activebackground=color,
            )

    def _seq_toggle_record(self) -> None:
        action = self._seq.toggle_record()
        if action == "backbone" and self._mode != "seq":
            self._switch_mode("seq")
        self._q_put(("seq",))
        self._refresh_seq_status()

    def _seq_toggle_play(self) -> None:
        if self._seq.is_playing():
            self._seq.stop_playback()
        elif not self._seq.start_playback():
            self._q_put(("log", "SEQ empty — record a backbone first", False))
        self._q_put(("seq",))
        self._refresh_seq_status()

    def _seq_keep(self) -> None:
        self._seq.keep()
        self._refresh_seq_status()

    def _seq_drop(self) -> None:
        self._seq.drop()
        self._refresh_seq_status()

    def _seq_undo(self) -> None:
        self._seq.undo()
        self._refresh_seq_status()

    def _seq_double(self) -> None:
        if not self._seq.double_length():
            self._q_put(("log", "SEQ length unchanged (max 8 cycles)", False))
        self._refresh_seq_status()

    def _seq_halve(self) -> None:
        if not self._seq.halve_length():
            self._q_put(("log", "SEQ length unchanged — a layer is that long", False))
        self._refresh_seq_status()

    def _seq_toggle_extend(self) -> None:
        self._seq.toggle_extend()
        self._refresh_seq_status()

    def _seq_stop(self) -> None:
        self._seq_to_pad_armed = False
        self._seq.stop()
        self._q_put(("seq",))
        self._refresh_seq_status()

    def _seq_clear(self) -> None:
        self._seq_to_pad_armed = False
        self._seq.clear()
        self._refresh_seq_status()

    def _seq_assign_to_pad(self) -> None:
        """Arm SEQ → PAD: next phrase-pad tap (touch or MPK) receives this take as a LOOP."""
        if self._seq.is_recording():
            self._seq.toggle_record()
        events, length = self._seq.snapshot()
        if not events or length <= 0.0:
            self._seq_status_var.set("Nothing to assign — record a backbone first.")
            self._seq_to_pad_armed = False
            self._paint_seq_buttons()
            return
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._seq_to_pad_armed = not self._seq_to_pad_armed
        self._refresh_seq_status()
        if self._seq_to_pad_armed:
            self._pads_view = "edit"
            self._switch_mode("pads")
            self._build_pads_mode()
            self._paint_phrase_pads()
            self._phrase_status_var.set(
                "SEQ → PAD — tap a pad (touch or MPK) to drop the sequence (overwrites that pad)"
            )
            self._append_log("SEQ → PAD armed — tap a phrase pad")

    def _finish_seq_to_pad(self, idx: int) -> None:
        events, length = self._seq.snapshot()
        self._seq_to_pad_armed = False
        if not events or length <= 0.0:
            self._phrase_status_var.set("Sequence was empty — assignment cancelled")
            self._paint_phrase_pads()
            self._paint_seq_buttons()
            return
        ok = self._phrases.load_from_events(
            idx, events, length, trigger_mode=PHRASE_TRIG_LOOP
        )
        if ok:
            self._append_log(
                f"SEQ → {phrase_pad_label(idx)} ({len(events)} ev, {length:.2f}s LOOP)"
            )
            # PLAY so the same drum pad now launches the clip
            self._pads_view = "play"
            self._build_pads_mode()
            self._phrase_status_var.set(
                f"Loaded SEQ → {phrase_pad_label(idx)} as LOOP — hit that pad to trigger it"
            )
        else:
            self._phrase_status_var.set("Could not assign sequence to that pad")
            self._paint_phrase_pads()
        self._paint_seq_buttons()

    def _pack_screen_regions(
        self,
        parent: tk.Misc,
        *,
        bg: str = "#111111",
        header_padx: int = 8,
        header_pady: Tuple[int, int] = (8, 2),
        body_padx: int = 6,
        body_pady: int = 2,
        footer_padx: int = 8,
        footer_pady: int = 8,
    ) -> Tuple[tk.Frame, tk.Frame, tk.Frame]:
        """Return (header, body, footer) packed so chrome never falls off-screen.

        Pack order matters on short displays:
          1) footer (BOTTOM) — reserved first, always visible
          2) header (TOP)
          3) body (TOP, expand) — absorbs leftover height only

        Put all action buttons in ``footer`` (or nested frames inside it).
        Put scrollable / expanding content only in ``body``.
        """
        footer = tk.Frame(parent, bg=bg)
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=footer_padx, pady=footer_pady)

        header = tk.Frame(parent, bg=bg)
        header.pack(side=tk.TOP, fill=tk.X, padx=header_padx, pady=header_pady)

        body = tk.Frame(parent, bg=bg)
        body.pack(side=tk.TOP, fill=tk.BOTH, expand=True, padx=body_padx, pady=body_pady)
        return header, body, footer

    def _build_touch_scroll_area(
        self,
        parent: tk.Misc,
        *,
        show_rail: bool = False,
    ) -> Tuple[tk.Frame, tk.Canvas, tk.Frame, Dict[str, object]]:
        """Scroll canvas with finger-drag (TFT70 capacitive). Optional ▲/▼ rail."""
        wrap = tk.Frame(parent, bg="#111111")
        wrap.pack(fill=tk.BOTH, expand=True)

        drag: Dict[str, object] = {
            "start_x": 0,
            "start_y": 0,
            "dragging": False,
            "scanning": False,
            "grabber": None,
        }

        if show_rail:
            # Legacy resistive / accessibility: fat page buttons on the right
            rail = tk.Frame(wrap, bg="#111111", width=88)
            rail.pack(side=tk.RIGHT, fill=tk.Y, padx=(4, 0))
            rail.pack_propagate(False)

            def _scroll_step(direction: int) -> None:
                canvas.update_idletasks()
                top, bot = canvas.yview()
                visible = max(0.12, bot - top)
                step = visible * 0.9
                canvas.yview_moveto(max(0.0, min(1.0, top + direction * step)))

            up = self._mk_touch_btn(rail, "▲\nUP", lambda: _scroll_step(-1), bg="#504945")
            up.configure(font=("DejaVu Sans", 14, "bold"), pady=6)
            up.pack(side=tk.TOP, fill=tk.BOTH, expand=True, pady=(0, 3), ipady=10)

            down = self._mk_touch_btn(
                rail, "▼\nDOWN", lambda: _scroll_step(1), bg="#504945"
            )
            down.configure(font=("DejaVu Sans", 14, "bold"), pady=6)
            down.pack(side=tk.BOTTOM, fill=tk.BOTH, expand=True, pady=(3, 0), ipady=10)

            mid = tk.Frame(wrap, bg="#111111")
            mid.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        else:
            mid = wrap

        canvas = tk.Canvas(mid, bg="#111111", highlightthickness=0, bd=0)
        canvas.pack(fill=tk.BOTH, expand=True)
        drag["canvas"] = canvas

        inner = tk.Frame(canvas, bg="#111111")
        window_id = canvas.create_window((0, 0), window=inner, anchor="nw")

        def _on_inner_configure(_event: object = None) -> None:
            canvas.configure(scrollregion=canvas.bbox("all"))

        def _on_canvas_configure(event: tk.Event) -> None:  # type: ignore[name-defined]
            canvas.itemconfigure(window_id, width=event.width)

        inner.bind("<Configure>", _on_inner_configure)
        canvas.bind("<Configure>", _on_canvas_configure)

        def _canvas_xy(event: tk.Event) -> Tuple[int, int]:  # type: ignore[name-defined]
            return (
                int(event.x_root) - int(canvas.winfo_rootx()),
                int(event.y_root) - int(canvas.winfo_rooty()),
            )

        def _release_grab() -> None:
            grabber = drag.get("grabber")
            drag["grabber"] = None
            if grabber is None:
                return
            try:
                grabber.grab_release()  # type: ignore[union-attr]
            except tk.TclError:
                pass

        def _drag_start(event: tk.Event) -> str:  # type: ignore[name-defined]
            # Grab so B1-Motion keeps arriving after the finger leaves the
            # pressed voice button (Tk otherwise drops Motion on Buttons).
            _release_grab()
            drag["start_x"] = int(event.x_root)
            drag["start_y"] = int(event.y_root)
            drag["dragging"] = False
            drag["scanning"] = False
            try:
                event.widget.grab_set()
                drag["grabber"] = event.widget
            except tk.TclError:
                drag["grabber"] = None
            return "break"

        def _drag_move(event: tk.Event) -> str:  # type: ignore[name-defined]
            y = int(event.y_root)
            start_y = int(drag["start_y"])  # type: ignore[arg-type]
            if abs(y - start_y) >= TOUCH_SCROLL_THRESH_PX:
                drag["dragging"] = True
            if not drag["dragging"]:
                return "break"
            bbox = canvas.bbox("all")
            view_h = max(1, int(canvas.winfo_height()))
            content_h = (bbox[3] - bbox[1]) if bbox else view_h
            if content_h <= view_h:
                return "break"
            cx, cy = _canvas_xy(event)
            if not drag["scanning"]:
                # Anchor at the press point so the first dragto doesn't jump.
                sx = int(drag["start_x"]) - int(canvas.winfo_rootx())  # type: ignore[arg-type]
                sy = int(drag["start_y"]) - int(canvas.winfo_rooty())  # type: ignore[arg-type]
                canvas.scan_mark(sx, sy)
                drag["scanning"] = True
            canvas.scan_dragto(cx, cy, gain=1)
            return "break"

        def _drag_end(event: tk.Event) -> str:  # type: ignore[name-defined]
            del event
            _release_grab()
            drag["scanning"] = False
            return "break"

        def _bind_empty_drag(widget: tk.Misc) -> None:
            widget.bind("<ButtonPress-1>", _drag_start)
            widget.bind("<B1-Motion>", _drag_move)
            widget.bind("<ButtonRelease-1>", _drag_end)

        _bind_empty_drag(canvas)
        _bind_empty_drag(inner)
        drag["_move"] = _drag_move
        drag["_start"] = _drag_start
        drag["_end"] = _drag_end
        drag["_release_grab"] = _release_grab
        return wrap, canvas, inner, drag

    def _mk_scroll_select_btn(
        self,
        parent: tk.Misc,
        text: str,
        command,
        drag: Dict[str, object],
        bg: str = "#3c3836",
    ) -> tk.Button:
        """Grid button: short tap selects; finger drag scrolls the parent canvas."""
        btn = tk.Button(
            parent, text=text,
            font=("DejaVu Sans", 14, "bold"), fg="#fbf1c7", bg=bg,
            activeforeground="#fbf1c7", activebackground=bg,
            relief=tk.FLAT, bd=0, padx=8, pady=12, cursor="hand2",
            takefocus=0,
        )

        def _press(event: tk.Event) -> str:  # type: ignore[name-defined]
            starter = drag.get("_start")
            if callable(starter):
                return starter(event)  # type: ignore[misc]
            return "break"

        def _move(event: tk.Event) -> str:  # type: ignore[name-defined]
            mover = drag.get("_move")
            if callable(mover):
                return mover(event)  # type: ignore[misc]
            return "break"

        def _release(event: tk.Event) -> str:  # type: ignore[name-defined]
            was_drag = bool(drag.get("dragging"))
            ender = drag.get("_end")
            if callable(ender):
                ender(event)  # type: ignore[misc]
            else:
                releaser = drag.get("_release_grab")
                if callable(releaser):
                    releaser()  # type: ignore[misc]
            if was_drag:
                return "break"
            now = time.monotonic()
            last = getattr(btn, "_last_fire", 0.0)
            if now - last < 0.18:
                return "break"
            btn._last_fire = now  # type: ignore[attr-defined]
            command()
            return "break"

        btn.bind("<ButtonPress-1>", _press)
        btn.bind("<B1-Motion>", _move)
        btn.bind("<ButtonRelease-1>", _release)
        return btn

    def _mk_touch_btn(self, parent: tk.Misc, text: str, command, bg: str = "#3c3836") -> tk.Button:
        """Touch-friendly button: fire on press (resistive panels often miss click)."""
        btn = tk.Button(
            parent, text=text,
            font=("DejaVu Sans", 14, "bold"), fg="#fbf1c7", bg=bg,
            activeforeground="#fbf1c7", activebackground=bg,
            relief=tk.FLAT, bd=0, padx=8, pady=12, cursor="hand2",
            takefocus=0,
        )

        def _fire(_event: object = None) -> str:
            # Debounce bounce from ADS7846. Fire on press only — pairing
            # ButtonPress + Button.command double-triggers on release.
            now = time.monotonic()
            last = getattr(btn, "_last_fire", 0.0)
            if now - last < 0.18:
                return "break"
            btn._last_fire = now  # type: ignore[attr-defined]
            command()
            return "break"

        # No command= callback: resistive panels often never complete a click.
        btn.bind("<ButtonPress-1>", _fire)
        return btn

    def _select_voice_index(self, idx: int, *, close_grid: bool = False) -> None:
        if not self._voice_names:
            return
        self._voice_index = idx % len(self._voice_names)
        name = self._voice_names[self._voice_index]
        # VOICES / PREV / NEXT set morph-A and park at pure A (B stays as morph target)
        self.engine.set_morph_index(self._voice_index)
        snap = self._voice_fx_sidecars.get(name)
        if snap is not None:
            self.engine.apply_voice_fx_sidecar(name, snap)
        self._mark_settings_dirty()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        self.last_var.set(f"Voice → {name.upper()}")
        self._q_put(("log", f"Voice → {name}", False))
        if self._grid_open:
            self._paint_voice_grid()
            if close_grid:
                self._close_voice_grid()
        if self._morph_ui_open:
            self._paint_morph_menu()
        if not self._overlay_busy():
            self._paint_synth_waveform(force=True)

    def _sync_voice_index_from_morph(self) -> None:
        """Keep UI index on the nearer morph endpoint while Knob1 moves."""
        a_idx, b_idx = self.engine.morph_pair_indices()
        blend = self.engine.morph()
        self._voice_index = a_idx if blend < 0.5 else b_idx
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        if self._grid_open:
            self._paint_voice_grid()
        if self._morph_ui_open:
            self._paint_morph_menu()

    def _open_save_voice(self) -> None:
        """Bake morph + drive + tone into a new dry wavetable (shape only)."""
        if self._save_voice_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        # Leave other overlays so the name pad is full-screen
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)

        self._save_voice_open = True
        self._save_voice_keys_digits = False
        self._synth_shell.pack_forget()

        self._save_voice_frame = tk.Frame(self._mode_host, bg="#111111")
        self._save_voice_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._save_voice_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="SAVE VOICE",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        a, b, blend = self.engine.morph_neighbors()
        hint = a if a == b else f"{a}→{b} {int(blend * 100)}%"
        tk.Label(
            header,
            text=f"bake shape · {hint}",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            self._save_voice_frame,
            text=(
                "Wave shape: morph + drive + tone → .wav. "
                "Alongside: delay/reverb amounts in a tiny .fx.json "
                "(drive stays in the wave, not double-applied)."
            ),
            font=("DejaVu Sans", 10),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, padx=8, pady=(0, 4))

        name_row = tk.Frame(self._save_voice_frame, bg="#111111")
        name_row.pack(fill=tk.X, padx=8, pady=4)
        suggested = self.engine.suggested_save_voice_name()
        self._save_voice_entry = tk.Entry(
            name_row,
            font=("DejaVu Sans Mono", 18),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
        )
        self._save_voice_entry.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, ipady=10)
        self._save_voice_entry.insert(0, suggested)
        self._save_voice_entry.focus_set()

        self._save_voice_status = tk.Label(
            self._save_voice_frame,
            text=(
                f"Will write {self._user_waves_dir.name}/{suggested}.wav "
                f"+ {suggested}.fx.json"
            ),
            font=("DejaVu Sans Mono", 11),
            fg="#83a598",
            bg="#111111",
            anchor="w",
        )
        self._save_voice_status.pack(fill=tk.X, padx=8, pady=(0, 4))

        opt = tk.Frame(self._save_voice_frame, bg="#111111")
        opt.pack(fill=tk.X, padx=6, pady=2)
        self._mk_touch_btn(
            opt, "SUGGEST", self._reset_save_voice_name, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "⌫", lambda: self._save_voice_type("\b"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "CLR", lambda: self._save_voice_type("\x15"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)

        # SAVE/CANCEL claim their strip first — the keyboard shrinks, never the
        # buttons that end the job.
        footer = tk.Frame(self._save_voice_frame, bg="#111111")
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(
            footer, "SAVE", self._confirm_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)
        self._mk_touch_btn(
            footer, "CANCEL", self._close_save_voice, bg="#9d0006"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)

        keys = tk.Frame(self._save_voice_frame, bg="#111111")
        keys.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        self._save_voice_keys = keys
        self._paint_save_voice_keyboard()
        self._append_log("SAVE VOICE — bake wave shape + keep delay/reverb alongside")

    def _paint_save_voice_keyboard(self) -> None:
        keys = getattr(self, "_save_voice_keys", None)
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._save_voice_keys_digits:
            rows = ("1234567890", "-.")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm_")
            toggle_label = "123"
        for r, row in enumerate(rows):
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._save_voice_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(
            fr, toggle_label, self._toggle_save_voice_keys, bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)

    def _toggle_save_voice_keys(self) -> None:
        self._save_voice_keys_digits = not self._save_voice_keys_digits
        self._paint_save_voice_keyboard()

    def _reset_save_voice_name(self) -> None:
        if self._save_voice_entry is None:
            return
        suggested = self.engine.suggested_save_voice_name()
        self._save_voice_entry.delete(0, tk.END)
        self._save_voice_entry.insert(0, suggested)
        self._update_save_voice_status()

    def _save_voice_type(self, ch: str) -> None:
        entry = self._save_voice_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        elif ch == "\x15":
            entry.delete(0, tk.END)
        else:
            entry.insert(tk.END, ch)
        self._update_save_voice_status()

    def _update_save_voice_status(self) -> None:
        if self._save_voice_status is None or self._save_voice_entry is None:
            return
        name = sanitize_voice_name(self._save_voice_entry.get())
        if name in BUILTIN_VOICE_NAMES:
            self._save_voice_status.configure(
                text=f"'{name}' is a built-in — pick another name", fg="#fb4934"
            )
            return
        path = self._user_waves_dir / f"{name}.wav"
        exists = name in self.engine.voice_names or path.is_file()
        tag = "overwrite" if exists else "new"
        self._save_voice_status.configure(
            text=(
                f"{tag}: {self._user_waves_dir.name}/{name}.wav "
                f"+ {name}.fx.json"
            ),
            fg="#fabd2f" if exists else "#83a598",
        )

    def _confirm_save_voice(self) -> None:
        if self._save_voice_entry is None:
            return
        raw = self._save_voice_entry.get()
        name = sanitize_voice_name(raw)
        if not name:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(text="Need a name", fg="#fb4934")
            return
        if name in BUILTIN_VOICE_NAMES:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(
                    text=f"Cannot replace built-in '{name}'", fg="#fb4934"
                )
            return
        try:
            key, cycle, sidecar = self.engine.save_current_voice(name)
            wav_path = self._user_waves_dir / f"{key}.wav"
            fx_path = voice_fx_sidecar_path(self._user_waves_dir, key)
            write_wavetable_wav(wav_path, cycle, sample_rate=44100)
            write_voice_fx_sidecar(fx_path, sidecar)
            self._voice_fx_sidecars[key] = dict(sidecar)
        except Exception as exc:
            if self._save_voice_status is not None:
                self._save_voice_status.configure(text=f"Save failed: {exc}", fg="#fb4934")
            self._append_log(f"SAVE VOICE failed: {exc}")
            return

        self._voice_names = self.engine.voice_names
        try:
            self._voice_index = self._voice_names.index(key)
        except ValueError:
            self._voice_index = 0
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        self._mark_settings_dirty()
        self._append_log(
            f"Saved voice '{key}' → {wav_path.name} + {fx_path.name} "
            f"(dly={int(sidecar.get('fx_delay_mix', 0) * 127)} "
            f"rvb={int(sidecar.get('fx_reverb_mix', 0) * 127)})"
        )
        self._close_save_voice()
        self._paint_synth_waveform(force=True)

    def _close_save_voice(self, restore_main: bool = True) -> None:
        if not self._save_voice_open:
            return
        if self._save_voice_frame is not None:
            self._save_voice_frame.destroy()
            self._save_voice_frame = None
        self._save_voice_entry = None
        self._save_voice_status = None
        self._save_voice_drive_btn = None
        self._save_voice_keys = None
        self._save_voice_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)

    def _open_voice_grid(self) -> None:
        if self._grid_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        self._grid_open = True
        self._synth_shell.pack_forget()

        self._grid_frame = tk.Frame(self._mode_host, bg="#111111")
        self._grid_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._grid_frame, header_pady=(6, 2), body_padx=4, footer_padx=6, footer_pady=6
        )
        tk.Label(
            header,
            text="VOICES — tap · drag to scroll",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text=f"{len(self._voice_names)} loaded",
            font=("DejaVu Sans", 12),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        _wrap, _canvas, inner, drag = self._build_touch_scroll_area(body)

        cols = 4 if len(self._voice_names) > 8 else 3
        self._grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_scroll_select_btn(
                inner,
                name.upper(),
                lambda idx=i: self._select_voice_index(idx, close_grid=True),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=4, ipady=6)
            self._grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        vib = tk.Frame(footer, bg="#111111")
        vib.pack(fill=tk.X, pady=(0, 4))
        tk.Label(
            vib, text="VIB", font=("DejaVu Sans", 12, "bold"),
            fg="#a89984", bg="#111111", padx=4,
        ).pack(side=tk.LEFT)
        self._vib_toggle_btn = self._mk_touch_btn(
            vib, "WHEEL", self._toggle_vib_always, bg="#3c3836"
        )
        self._vib_toggle_btn.configure(font=("DejaVu Sans", 12, "bold"), padx=6)
        self._vib_toggle_btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=(0, 8), ipady=6)

        def _vib_btn(text: str, command) -> None:
            btn = self._mk_touch_btn(vib, text, command, bg="#504945")
            btn.configure(font=("DejaVu Sans", 12, "bold"), padx=6)
            btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=2, ipady=6)

        def _vib_value(var: tk.StringVar) -> None:
            tk.Label(
                vib, textvariable=var, font=("DejaVu Sans Mono", 12, "bold"),
                fg="#fabd2f", bg="#111111", width=8,
            ).pack(side=tk.LEFT)

        _vib_btn("DEPTH −", lambda: self._nudge_vib_depth(-VIB_DEPTH_STEP))
        _vib_value(self._vib_depth_var)
        _vib_btn("DEPTH +", lambda: self._nudge_vib_depth(VIB_DEPTH_STEP))
        _vib_btn("RATE −", lambda: self._nudge_vib_rate(-VIB_RATE_STEP))
        _vib_value(self._vib_rate_var)
        _vib_btn("RATE +", lambda: self._nudge_vib_rate(VIB_RATE_STEP))

        actions = tk.Frame(footer, bg="#111111")
        actions.pack(fill=tk.X)
        self._mk_touch_btn(
            actions, "SAVE AS…", self._open_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        self._mk_touch_btn(actions, "CLOSE", self._close_voice_grid, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12
        )
        self._paint_vib_controls()
        self._paint_voice_grid()

    def _paint_vib_controls(self) -> None:
        depth, rate, always = self.engine.vib_state()
        self._vib_depth_var.set(f"{depth:.2f} st")
        self._vib_rate_var.set(f"{rate:.1f} Hz")
        if self._vib_toggle_btn is not None:
            on = always > 0.01
            color = "#b16286" if on else "#3c3836"
            try:
                self._vib_toggle_btn.configure(
                    text="ON" if on else "WHEEL", bg=color, activebackground=color
                )
            except Exception:
                pass

    def _toggle_vib_always(self) -> None:
        _depth, _rate, always = self.engine.vib_state()
        value = self.engine.set_vib_always(0.0 if always > 0.01 else 1.0)
        self._mark_settings_dirty()
        self._paint_vib_controls()
        self.mod_var.set(self._format_mod_line())
        self._append_log(f"Vibrato {'always on' if value > 0.01 else 'follows mod wheel'}")

    def _nudge_vib_depth(self, delta: float) -> None:
        depth = self.engine.nudge_vib_depth(delta)
        st = self.engine.modulation_state()
        # Turning depth up with the wheel down would be silent — engage it so
        # the control you just touched is the one you hear.
        if depth > 0.001 and float(st.get("mod", 0.0)) < 0.01:
            _d, _r, always = self.engine.vib_state()
            if always < 0.01:
                self.engine.set_vib_always(1.0)
                self._append_log("Vibrato ON (screen control)")
        self._mark_settings_dirty()
        self._paint_vib_controls()
        self.mod_var.set(self._format_mod_line())

    def _nudge_vib_rate(self, delta: float) -> None:
        self.engine.nudge_vib_rate(delta)
        self._mark_settings_dirty()
        self._paint_vib_controls()

    def _paint_voice_grid(self) -> None:
        if not self._grid_btns:
            return
        current = self._voice_names[self._voice_index] if self._voice_names else ""
        for name, btn in self._grid_btns.items():
            on = name == current
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)

    def _close_voice_grid(self, restore_main: bool = True) -> None:
        if not self._grid_open:
            return
        if self._grid_frame is not None:
            self._grid_frame.destroy()
            self._grid_frame = None
        self._grid_btns = {}
        self._vib_toggle_btn = None
        self._grid_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)

    def _open_morph_menu(self) -> None:
        """Pick morph endpoints A and B; Knob 1 blends A→B."""
        if self._morph_ui_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        # Hand knobs back to morph while editing the pair
        if self.engine.drum_mode() or self.engine.fx_mode() or self.engine.bus_fx_mode():
            self.engine.set_drum_mode(False)
            self.engine.set_fx_mode(False)
            self.engine.set_bus_fx_mode(False)
            self._paint_drum_lock_btn()
            self._paint_fx_mode_btn()
            self._paint_bus_fx_mode_btn()
            self.mod_var.set(self._format_mod_line())

        self._morph_ui_open = True
        self._morph_pick_side = "a"
        # Remember the pair we came in with so CANCEL can put it back
        a_idx, b_idx = self.engine.morph_pair_indices()
        self._morph_undo = (a_idx, b_idx, self.engine.morph(), self._voice_index)
        self._synth_shell.pack_forget()

        self._morph_frame = tk.Frame(self._mode_host, bg="#111111")
        self._morph_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._morph_frame, header_pady=(6, 2), body_padx=4, footer_padx=6, footer_pady=6
        )
        title = tk.Frame(header, bg="#111111")
        title.pack(fill=tk.X)
        tk.Label(
            title,
            text="MORPH PAIR",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._morph_status_lbl = tk.Label(
            title,
            text="tap A/B · drag to scroll",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        )
        self._morph_status_lbl.pack(side=tk.RIGHT)

        # A / B selector row
        pair_row = tk.Frame(header, bg="#111111")
        pair_row.pack(fill=tk.X, pady=(4, 2))
        self._morph_side_btns = {}
        for side, label in (("a", "A"), ("b", "B")):
            btn = self._mk_touch_btn(
                pair_row,
                f"{label}: …",
                lambda s=side: self._set_morph_pick_side(s),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 14, "bold"), pady=10)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
            self._morph_side_btns[side] = btn

        swap_btn = self._mk_touch_btn(pair_row, "SWAP", self._swap_morph_pair, bg="#504945")
        swap_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=10)
        swap_btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=3)

        _wrap, _canvas, inner, drag = self._build_touch_scroll_area(body)

        cols = 4 if len(self._voice_names) > 8 else 3
        self._morph_grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_scroll_select_btn(
                inner,
                name.upper(),
                lambda idx=i: self._assign_morph_endpoint(idx),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=10)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=2, ipady=4)
            self._morph_grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        self._mk_touch_btn(
            footer, "SAVE AS…", self._open_save_voice, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12)
        self._mk_touch_btn(footer, "DONE", self._close_morph_menu, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12
        )
        self._mk_touch_btn(footer, "CANCEL", self._cancel_morph_menu, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12
        )
        self._paint_morph_menu()

    def _set_morph_pick_side(self, side: str) -> None:
        self._morph_pick_side = "b" if side == "b" else "a"
        self._paint_morph_menu()

    def _assign_morph_endpoint(self, idx: int) -> None:
        side = self._morph_pick_side
        self.engine.set_morph_endpoint(side, idx)
        self._mark_settings_dirty()
        name = self._voice_names[idx]
        # After setting A, auto-arm B so picking a pair is two taps
        if side == "a":
            self._morph_pick_side = "b"
            self._voice_index = idx
        else:
            self._morph_pick_side = "a"
        self.last_var.set(f"Morph {side.upper()} → {name.upper()}")
        self._q_put(("log", f"Morph {side.upper()} → {name}", False))
        self._paint_morph_menu()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())

    def _swap_morph_pair(self) -> None:
        a, b = self.engine.morph_pair_indices()
        blend = self.engine.morph()
        # Swap endpoints and invert blend so the sound stays put
        self.engine.set_morph_pair(b, a, morph=1.0 - blend)
        self._mark_settings_dirty()
        self._q_put(("log", "Morph pair swapped", False))
        self._paint_morph_menu()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())

    def _paint_morph_menu(self) -> None:
        if not self._morph_ui_open:
            return
        a_name, b_name, blend = self.engine.morph_neighbors()
        for side, btn in self._morph_side_btns.items():
            name = a_name if side == "a" else b_name
            armed = side == self._morph_pick_side
            label = f"{'●' if armed else '○'} {side.upper()}: {name.upper()}"
            color = "#b16286" if armed else "#3c3836"
            btn.configure(text=label, bg=color, activebackground=color)
        if self._morph_status_lbl is not None:
            self._morph_status_lbl.configure(
                text=f"Knob1 blends  {a_name} → {b_name}  ({int(blend * 100)}%)"
            )
        for name, btn in self._morph_grid_btns.items():
            if name == a_name and name == b_name:
                color = "#689d6a"
            elif name == a_name:
                color = "#458588"
            elif name == b_name:
                color = "#d3869b"
            else:
                color = "#3c3836"
            btn.configure(bg=color, activebackground=color)

    def _cancel_morph_menu(self) -> None:
        """CANCEL means the pair you walked in with, not the one you auditioned."""
        undo = getattr(self, "_morph_undo", None)
        if undo is not None:
            a_idx, b_idx, blend, voice_idx = undo
            self.engine.set_morph_pair(a_idx, b_idx, morph=blend)
            self._voice_index = voice_idx
            self._mark_settings_dirty()
            self._q_put(("log", "Morph pair restored (CANCEL)", False))
        self._close_morph_menu()

    def _close_morph_menu(self, restore_main: bool = True) -> None:
        if not self._morph_ui_open:
            return
        if self._morph_frame is not None:
            self._morph_frame.destroy()
            self._morph_frame = None
        self._morph_side_btns = {}
        self._morph_grid_btns = {}
        self._morph_status_lbl = None
        self._morph_ui_open = False
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)

    def _prev_voice(self) -> None:
        self._select_voice_index(self._voice_index - 1)

    def _next_voice(self) -> None:
        self._select_voice_index(self._voice_index + 1)

    def _toggle_full_vel(self) -> None:
        self._full_vel = not self._full_vel
        self._paint_full_vel_btn()
        self._mark_settings_dirty()
        self._append_log(f"Full velocity → {'ON' if self._full_vel else 'OFF'}")

    def _print_ports(self) -> None:
        names = mido.get_input_names()
        if not names:
            print("No MIDI inputs.")
            return
        print("MIDI inputs:")
        for i, n in enumerate(names):
            print(f"  [{i}] {n}")
        print(f"Wavetables ({len(self._voice_names)}): {', '.join(self._voice_names)}")

    def _maybe_reopen_midi(self) -> None:
        """If we started without the filtered device, adopt it when it appears."""
        if self._stop.is_set() or not self.port_filter:
            return
        try:
            current = ""
            if self._inport is not None:
                current = str(getattr(self._inport, "name", "") or "")
            if self.port_filter in current.lower():
                return
            wanted = self._pick_port(retries=1, delay_s=0.0, allow_fallback=False)
            if not wanted:
                self.root.after(2000, self._maybe_reopen_midi)
                return
            old = self._inport
            self._inport = mido.open_input(wanted)
            if old is not None:
                try:
                    old.close()
                except Exception:
                    pass
            self._append_log(f"MIDI reconnected: {wanted}")
            print(f"midi: reopened input ({wanted})", flush=True)
            self.last_var.set(f"MIDI: {wanted}")
        except Exception as exc:
            print(f"midi: reopen failed: {exc}", flush=True)
            self.root.after(2500, self._maybe_reopen_midi)

    def _pick_port(
        self,
        *,
        retries: int = 1,
        delay_s: float = 0.4,
        allow_fallback: bool = True,
    ) -> Optional[str]:
        """Resolve MIDI in. Retries help after kiosk restarts (MPK port briefly busy)."""
        retries = max(1, int(retries))
        for attempt in range(retries):
            names = mido.get_input_names()
            if not names:
                if attempt + 1 < retries:
                    time.sleep(delay_s)
                    continue
                return None
            if self.port_filter:
                for n in names:
                    if self.port_filter in n.lower():
                        return n
                if attempt + 1 < retries:
                    time.sleep(delay_s)
                    continue
                if not allow_fallback:
                    return None
                for n in names:
                    if "through" not in n.lower():
                        print(f"No input matching '{self.port_filter}'; using {n}", flush=True)
                        return n
                print(f"No input matching '{self.port_filter}'. Available:", flush=True)
                for n in names:
                    print(f"  {n}", flush=True)
                print(f"Falling back to: {names[0]}", flush=True)
                return names[0]
            for n in names:
                if "mpk" in n.lower():
                    return n
            for n in names:
                if "through" not in n.lower():
                    return n
            return names[0]
        return None

    def _midi_loop(self) -> None:
        assert self._inport is not None
        while not self._stop.is_set():
            try:
                for msg in self._inport.iter_pending():
                    self._handle_midi(msg)
            except Exception as exc:
                tb = __import__("traceback").format_exc()
                print(tb, flush=True)
                self._q_put(("log", f"MIDI ERROR: {exc}", False))
            time.sleep(0.001)

    def _q_put(self, item: tuple) -> None:
        """Never block the MIDI thread on a full UI queue — drop oldest junk."""
        try:
            self.event_q.put_nowait(item)
            return
        except queue.Full:
            pass
        try:
            self.event_q.get_nowait()
        except queue.Empty:
            pass
        try:
            self.event_q.put_nowait(item)
        except queue.Full:
            pass

    def _put_continuous_log(self, line: str) -> None:
        """Throttle high-rate messages so the Tk queue can't freeze the UI."""
        now = time.monotonic()
        if now - getattr(self, "_last_cont_put", 0.0) < 0.08:
            self._pending_cont_log = line
            return
        self._last_cont_put = now
        self._pending_cont_log = None
        self._q_put(("log", line, True))

    def _knob_ui_feedback(self, label: Optional[str], *, morph: bool = False) -> None:
        """Coalesce high-rate knob UI updates — don't flood the event queue."""
        self._mod_dirty = True
        if morph:
            self._morph_dirty_ui = True
            self._schedule_scope_paint("synth", blank=True)
        if self.engine.drum_knob_focus() and self._kit_ui_open:
            self._schedule_scope_paint("drum", blank=True)
        if self._fx_ui_open and self.engine.fx_knob_focus():
            self._fx_dirty_ui = True
        if label:
            # Status line only; skip log spam (was making knobs feel laggy on Pi)
            self._pending_cont_log = label

    def _handle_knob_cc(self, control: int, value: int) -> Optional[str]:
        """Map MPK factory knobs. Returns a short UI label or None if unmapped."""
        if self.engine.fx_knob_focus():
            if control == CC_MORPH:
                self.engine.set_fx_drive(value)
                self._mark_settings_dirty()
                return f"FxDrive {value}"
            if control == CC_TONE:
                self.engine.set_fx_delay_time(value)
                self._mark_settings_dirty()
                ms = int((0.05 + (value / 127.0) * 0.70) * 1000)
                return f"FxDelay {ms}ms"
            if control == CC_ATTACK:
                self.engine.set_fx_delay_fb(value)
                self._mark_settings_dirty()
                return f"FxDlyFb {value}"
            if control == CC_RELEASE:
                self.engine.set_fx_delay_mix(value)
                self._mark_settings_dirty()
                return f"FxDlyMix {value}"
            if control == CC_VIB_DEPTH:
                self.engine.set_fx_reverb_size(value)
                self._mark_settings_dirty()
                return f"FxRvbSz {value}"
            if control == CC_VIB_RATE:
                self.engine.set_fx_reverb_mix(value)
                self._mark_settings_dirty()
                return f"FxRvbMix {value}"
            if control == CC_LEVEL:
                self.engine.set_synth_level(value)
                self._mark_settings_dirty()
                return f"SynLvl {value}"
            return None

        # Only in explicit DRUM MODE do knobs edit drum macros
        if self.engine.drum_knob_focus():
            if control == CC_MORPH:
                self.engine.set_drum_pitch(value)
                self._mark_settings_dirty()
                return f"DrumPitch {value}"
            if control == CC_TONE:
                self.engine.set_drum_tone(value)
                self._mark_settings_dirty()
                return f"DrumTone {value}"
            if control == CC_ATTACK:
                self.engine.set_drum_decay(value)
                self._mark_settings_dirty()
                return f"DrumStretch {value}"
            if control == CC_RELEASE:
                self.engine.set_drum_noise(value)
                self._mark_settings_dirty()
                return f"DrumNoise {value}"
            if control == CC_LEVEL:
                self.engine.set_drum_level(value)
                self._mark_settings_dirty()
                return f"DrmLvl {value}"
            # Other knobs ignored while drum-focused (keep level usable)
            if control in (CC_VIB_DEPTH, CC_VIB_RATE):
                return None

        if control == CC_MORPH:
            self.engine.set_morph(value)
            self._mark_settings_dirty()
            left, right, blend = self.engine.morph_neighbors()
            if left == right:
                return f"Morph  {value}  ({left})"
            return f"Morph  {value}  ({left}→{right} {int(blend * 100)}%)"
        if control == CC_TONE:
            self.engine.set_tone(value)
            self._mark_settings_dirty()
            return f"Tone   {value}"
        if control == CC_ATTACK:
            self.engine.set_attack(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"Attack {value}  ({st['attack'] * 1000:.0f} ms)"
        if control == CC_RELEASE:
            self.engine.set_release(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"Release {value}  ({st['release'] * 1000:.0f} ms)"
        if control == CC_VIB_DEPTH:
            self.engine.set_vib_depth(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"VibDepth {value}  ({st['vib_depth']:.2f} st)"
        if control == CC_VIB_RATE:
            self.engine.set_vib_rate(value)
            self._mark_settings_dirty()
            st = self.engine.modulation_state()
            return f"VibRate {value}  ({st['vib_hz']:.1f} Hz)"
        if control == CC_LEVEL:
            self.engine.set_synth_level(value)
            self._mark_settings_dirty()
            return f"SynLvl {value}"
        return None

    def _handle_midi(self, msg: mido.Message) -> None:
        continuous = msg.type == "pitchwheel" or (
            msg.type == "control_change"
            and (msg.control == 1 or msg.control in KNOB_CCS)
        )
        pads_mode = self._mode == "pads"

        if msg.type == "note_on" and msg.velocity > 0:
            is_drum = msg.channel == DRUM_CHANNEL
            phrase_recording = self._phrases.is_recording()
            # SEQ → PAD: MPK pad picks the destination clip (works from SEQ or PADS).
            if is_drum and self._seq_to_pad_armed:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None:
                    self._seq_to_pad_armed = False
                    self._q_put(("seq_to_pad", cell))
                    self._q_put(
                        (
                            "log",
                            f"Pad→SEQ {phrase_pad_label(cell)}  note {msg.note}",
                            False,
                        )
                    )
                    return
            # PADS mode: MPK pads launch/arm phrases. While a take is recording,
            # the armed cell (and other empty pads) stay drums; filled pads still
            # launch — same as tapping the grid. Run on the UI thread.
            if pads_mode and is_drum:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    self._q_put(("pad_midi", cell, msg.note, msg.velocity))
                    return
            # KIT explorer: hitting an MPK pad selects that voice for the scope
            if self._kit_ui_open and is_drum and phrase_cell_for_note(msg.note) is not None:
                self._q_put(("kit_sel", msg.note))
            vel = msg.velocity if is_drum or not self._full_vel else 127
            self.engine.note_on(msg.channel, msg.note, vel)
            self._seq.record_note(True, msg.channel, msg.note, vel)
            if pads_mode or phrase_recording:
                self._phrases.record_note(True, msg.channel, msg.note, vel)
            self._q_put(("on", msg.channel, msg.note, vel))
            if self._seq.is_recording():
                self._q_put(("seq",))
            if is_drum:
                model = drum_model_for_note(msg.note)
                rec_tag = " +rec" if phrase_recording else ""
                line = (
                    f"Pad/{model:<10} ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}{rec_tag}"
                )
                if self.engine.drum_mode():
                    self._q_put(("mod",))
                if phrase_recording:
                    self._q_put(("phrase",))
            elif self._full_vel and msg.velocity != 127:
                line = (
                    f"Note On   ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}→127"
                )
            else:
                line = format_message(msg)
            self._q_put(("log", line, False))
        elif msg.type == "note_off" or (msg.type == "note_on" and msg.velocity == 0):
            # Phrase-launch pads have no held note — but drum takes while recording do
            if pads_mode and msg.channel == DRUM_CHANNEL:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    return
            self.engine.note_off(msg.channel, msg.note)
            self._seq.record_note(False, msg.channel, msg.note, 0)
            if pads_mode or self._phrases.is_recording():
                self._phrases.record_note(False, msg.channel, msg.note, 0)
            self._q_put(("off", msg.channel, msg.note))
            if self._seq.is_recording():
                self._q_put(("seq",))
            self._put_continuous_log(format_message(msg))
        elif msg.type == "polytouch":
            if pads_mode and msg.channel == DRUM_CHANNEL:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None and self._drum_pad_is_phrase_control(cell):
                    return
            self.engine.set_pad_pressure(msg.channel, msg.note, msg.value)
            if msg.channel == DRUM_CHANNEL:
                self._put_continuous_log(
                    f"PadPress ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  press {msg.value}"
                )
            else:
                self._put_continuous_log(format_message(msg))
        elif msg.type == "aftertouch":
            if (
                pads_mode
                and msg.channel == DRUM_CHANNEL
                and not self._phrases.is_recording()
            ):
                return
            self.engine.set_pad_pressure(msg.channel, None, msg.value)
            if msg.channel == DRUM_CHANNEL:
                self._put_continuous_log(
                    f"PadPress ch{msg.channel + 1}  (all)  press {msg.value}"
                )
            else:
                self._put_continuous_log(format_message(msg))
        elif msg.type == "pitchwheel":
            self.engine.set_pitch_bend(msg.pitch)
            self._q_put(("mod",))
            self._put_continuous_log(format_message(msg))
        elif msg.type == "control_change":
            if msg.control == 1:
                self.engine.set_mod_wheel(msg.value)
                self._q_put(("mod",))
                self._put_continuous_log(format_message(msg))
            elif msg.control in KNOB_CCS:
                drum_focus = self.engine.drum_knob_focus()
                fx_focus = self.engine.fx_knob_focus()
                label = self._handle_knob_cc(msg.control, msg.value)
                self._knob_ui_feedback(
                    label,
                    morph=(msg.control == CC_MORPH and not drum_focus and not fx_focus),
                )
            elif msg.control == 123:
                self.engine.all_notes_off()
                self._q_put(("panic",))
                self._q_put(("log", format_message(msg), False))
            else:
                self._q_put(("log", format_message(msg), continuous))
        else:
            self._q_put(("log", format_message(msg), False))

    def _drain_queue(self) -> None:
        # Cap work per tick so a flood can't freeze touch for seconds
        processed = 0
        backlog = self.event_q.qsize()
        limit = 12 if backlog > 80 else 24
        try:
            while processed < limit:
                item = self.event_q.get_nowait()
                processed += 1
                kind = item[0]
                if kind == "log":
                    _, line, continuous = item
                    self.last_var.set(line)
                    if continuous:
                        now = time.monotonic()
                        if now - getattr(self, "_last_cont_log", 0.0) >= 0.12:
                            self._last_cont_log = now
                            self._append_log(line)
                    else:
                        self._append_log(line)
                elif kind == "on":
                    _, ch, note, vel = item
                    self._active_notes[(ch, note)] = vel
                    self._refresh_active()
                elif kind == "off":
                    _, ch, note = item
                    self._active_notes.pop((ch, note), None)
                    self._refresh_active()
                elif kind == "mod":
                    self._mod_dirty = True
                elif kind == "morph":
                    self._morph_dirty_ui = True
                    self._mod_dirty = True
                elif kind == "panic":
                    self._active_notes.clear()
                    self._refresh_active()
                elif kind == "seq":
                    self._refresh_seq_status()
                elif kind == "phrase":
                    self._refresh_phrase_status()
                elif kind == "seq_to_pad":
                    self._finish_seq_to_pad(int(item[1]))
                elif kind == "pad_midi":
                    self._on_pad_midi(int(item[1]), int(item[2]), int(item[3]))
                elif kind == "song":
                    self._refresh_song_status()
                elif kind == "update":
                    self._update_busy = False
                    result = item[1]
                    extra = item[2]
                    if result is not None:
                        self._update_check = result
                    if extra == "confirm" and result is not None and result.available:
                        self._update_confirming = True
                        self._settings_status_var.set(
                            "This deploys the whole repo from GitHub, then restarts.\n"
                            "Songs, presets, phrases, and settings.json stay.\n"
                            "Tap INSTALL NOW to continue, or CANCEL."
                        )
                        self._paint_settings_buttons()
                    elif extra and extra != "confirm":
                        self._settings_status_var.set(str(extra))
                        self._paint_settings_buttons()
                    else:
                        self._refresh_settings_status()
                    if result is not None and result.message:
                        self._append_log(result.message)
                elif kind == "update_progress":
                    self._settings_status_var.set(str(item[1]))
                    self.last_var.set(str(item[1]))
                elif kind == "update_done":
                    self._update_busy = False
                    info, err = item[1], item[2]
                    if err:
                        self._update_confirming = False
                        self._settings_status_var.set(f"Update failed: {err}")
                        self._paint_settings_buttons()
                        self._append_log(f"Update failed: {err}")
                    else:
                        short = getattr(info, "short", "latest")
                        self._settings_status_var.set(f"Installed {short} — restarting…")
                        self._append_log(f"Update installed {short}")
                        self.root.after(250, self._restart_after_update)
                elif kind == "kit_sel":
                    self._select_kit_note(int(item[1]), audition=False)
        except queue.Empty:
            pass
        pending = getattr(self, "_pending_cont_log", None)
        if pending is not None:
            self.last_var.set(pending)
            self._pending_cont_log = None
        # Apply coalesced knob/mod UI once per tick (keeps drum knobs snappy)
        if getattr(self, "_morph_dirty_ui", False):
            self._morph_dirty_ui = False
            self._sync_voice_index_from_morph()
            self._mod_dirty = True
        if getattr(self, "_mod_dirty", False):
            self._mod_dirty = False
            self.mod_var.set(self._format_mod_line())
            if self._fx_ui_open:
                self._refresh_fx_panel()
            if self._grid_open:
                self._paint_vib_controls()
            if self._kit_ui_open and getattr(self, "_kit_view", "grid") != "wave":
                self._refresh_kit_status()
        if getattr(self, "_fx_dirty_ui", False):
            self._fx_dirty_ui = False
            if self._fx_ui_open:
                self._refresh_fx_panel()
        # Coalesced CRT redraw — blanked immediately, painted after debounce
        if getattr(self, "_scope_needs_paint", False):
            if time.monotonic() >= float(getattr(self, "_scope_paint_at", 0.0)):
                self._flush_scope_paint()
        # Keep touch bar stacked above log chrome if packing ever races
        if self._mode == "synth" and not self._overlay_busy():
            try:
                self._touch.lift()
            except Exception:
                pass
        if not self._stop.is_set():
            self.root.after(40, self._drain_queue)

    def _refresh_active(self) -> None:
        if not self._active_notes:
            self.active_var.set("Active notes: —")
            return
        parts = [
            f"{midi_note_name(n)}(ch{ch + 1})"
            for (ch, n), _ in sorted(self._active_notes.items(), key=lambda x: x[0][1])
        ]
        self.active_var.set("Active notes: " + ", ".join(parts))

    def _append_log(self, line: str) -> None:
        ts = time.strftime("%H:%M:%S")
        self.log.configure(state=tk.NORMAL)
        self.log.insert(tk.END, f"{ts}  {line}\n")
        # Trim less often — Text ops are expensive on the Pi and starve touch
        if not hasattr(self, "_log_lines"):
            self._log_lines = 0
        self._log_lines += 1
        if self._log_lines > LOG_MAX + 20:
            end_line = int(float(self.log.index("end-1c").split(".")[0]))
            if end_line > LOG_MAX:
                self.log.delete("1.0", f"{end_line - LOG_MAX}.0")
            self._log_lines = LOG_MAX
        self.log.see(tk.END)
        self.log.configure(state=tk.DISABLED)

    def _clear_log(self) -> None:
        self.log.configure(state=tk.NORMAL)
        self.log.delete("1.0", tk.END)
        self.log.configure(state=tk.DISABLED)
        self._log_lines = 0

    def _panic(self) -> None:
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        if self._songs.is_playing():
            self._songs.stop()
        self.engine.all_notes_off()
        self._active_notes.clear()
        self._refresh_active()
        self._refresh_seq_status()
        self._refresh_phrase_status()
        self._refresh_song_status()
        self._append_log("All Notes Off")

    def _apply_display_geometry(self) -> None:
        """Fill the active X screen (TFT70 is 800×480; older default was 800×420)."""
        try:
            sw = int(self.root.winfo_screenwidth())
            sh = int(self.root.winfo_screenheight())
        except Exception:
            sw, sh = 800, 480
        if sw < 320 or sh < 240:
            sw, sh = 800, 480

        if self._fullscreen:
            # Kiosk: true fullscreen when the WM supports it; always size to screen too
            try:
                self.root.attributes("-fullscreen", True)
            except Exception:
                try:
                    self.root.attributes("-zoomed", True)
                except Exception:
                    try:
                        self.root.state("zoomed")
                    except Exception:
                        pass
            self.root.geometry(f"{sw}x{sh}+0+0")
            print(f"ui: fullscreen {sw}x{sh}", flush=True)
            return

        self.root.geometry(f"{sw}x{sh}+0+0")
        print(f"ui: geometry {sw}x{sh}", flush=True)

    def _on_pointer_activity(self, _event: object = None) -> None:
        idle = getattr(self, "_idle", None)
        if idle is None:
            return
        # While blanked, ANY pointer event must wake — the overlay canvas can
        # miss capacitive events (dual ADS7846+ft5x06, grab failures, etc.).
        if idle.active or self._saver_canvas is not None:
            self._hide_screensaver()
            return
        idle.poke()

    def _on_root_destroy(self, event: tk.Event) -> None:  # type: ignore[name-defined]
        if event.widget is not self.root:
            return
        self._stop.set()
        self._cancel_screensaver_tick()

    def _cancel_screensaver_tick(self) -> None:
        aid = self._saver_tick_after
        self._saver_tick_after = None
        if aid is None:
            return
        try:
            self.root.after_cancel(aid)
        except Exception:
            pass

    def _arm_screensaver_tick(self) -> None:
        self._cancel_screensaver_tick()
        if self._stop.is_set():
            return
        try:
            if not self.root.winfo_exists():
                return
            self._saver_tick_after = self.root.after(1000, self._screensaver_tick)
        except tk.TclError:
            self._saver_tick_after = None

    def _screensaver_tick(self) -> None:
        if self._stop.is_set():
            return
        try:
            if not self.root.winfo_exists():
                return
        except tk.TclError:
            return
        if self._idle.due():
            self._show_screensaver()
        elif self._idle.active:
            self._nudge_screensaver_orbit()
        else:
            self._apply_pixel_shift()
        self._arm_screensaver_tick()

    def _show_screensaver(self, *, force: bool = False) -> None:
        if self._saver_canvas is not None:
            self._nudge_screensaver_orbit()
            return
        if not force and not self._idle.due():
            return
        self._idle.activate()
        self._saver_started = time.monotonic()
        canvas = tk.Canvas(
            self.root,
            bg="#000000",
            highlightthickness=0,
            bd=0,
            cursor="none",
        )
        canvas.place(x=0, y=0, relwidth=1, relheight=1)
        try:
            canvas.lift()
            canvas.focus_set()
        except Exception:
            pass
        try:
            # Prefer global grab so a stray ADS7846/ft5x06 mapping still
            # delivers the wake tap to this overlay.
            canvas.grab_set_global()
        except Exception:
            try:
                canvas.grab_set()
            except Exception:
                pass
        # Capacitive panels sometimes emit release-only or motion-only bursts.
        canvas.bind("<ButtonPress>", self._on_screensaver_tap)
        canvas.bind("<ButtonRelease>", self._on_screensaver_tap)
        canvas.bind("<Motion>", self._on_screensaver_motion)
        canvas.bind("<B1-Motion>", self._on_screensaver_tap)
        self._saver_canvas = canvas
        self._saver_hint = canvas.create_text(
            40,
            40,
            text="tap to wake",
            fill="#4a4a4a",
            font=("DejaVu Sans", 18, "bold"),
            anchor="nw",
        )
        self._saver_clock = canvas.create_text(
            40,
            72,
            text=time.strftime("%H:%M"),
            fill="#3a3a3a",
            font=("DejaVu Sans", 14),
            anchor="nw",
        )
        self._nudge_screensaver_orbit()
        self._backlight.dim()
        print("ui: screensaver on", flush=True)

    def _nudge_screensaver_orbit(self) -> None:
        canvas = self._saver_canvas
        if canvas is None or self._saver_hint is None:
            return
        try:
            w = int(canvas.winfo_width())
            h = int(canvas.winfo_height())
        except Exception:
            w, h = 800, 480
        if w <= 1 or h <= 1:
            w, h = 800, 480
        elapsed = time.monotonic() - self._saver_started
        x, y = orbit_xy(elapsed, w, h, 220, 56)
        canvas.coords(self._saver_hint, x, y)
        if self._saver_clock is not None:
            canvas.itemconfigure(self._saver_clock, text=time.strftime("%H:%M"))
            canvas.coords(self._saver_clock, x, y + 28)

    def _apply_pixel_shift(self) -> None:
        """Nudge chrome a couple of pixels so bold boxes don't sit still."""
        if getattr(self, "_idle", None) is not None and self._idle.active:
            return
        elapsed = time.monotonic() - getattr(self, "_shift_started", time.monotonic())
        dx, dy = pixel_shift_xy(elapsed)
        if (dx, dy) == getattr(self, "_shift_xy", (None, None)):
            return
        self._shift_xy = (dx, dy)
        gutter = PIXEL_SHIFT_AMPLITUDE
        try:
            self._nav.pack_configure(
                padx=(gutter + dx, gutter - dx),
                pady=(gutter + dy, 0),
            )
            self._mode_host.pack_configure(
                padx=(gutter + dx, gutter - dx),
                pady=(0, gutter - dy),
            )
        except Exception:
            pass

    def _on_screensaver_tap(self, _event: object = None) -> str:
        self._hide_screensaver()
        return "break"

    def _on_screensaver_motion(self, event: tk.Event) -> Optional[str]:  # type: ignore[name-defined]
        # Ignore pure hover; only wake when a button is held (touch drag).
        state = int(getattr(event, "state", 0) or 0)
        if state & 0x0100:  # Button1Mask
            self._hide_screensaver()
            return "break"
        return None

    def _hide_screensaver(self) -> None:
        idle = getattr(self, "_idle", None)
        if idle is not None:
            idle.poke()
        canvas = self._saver_canvas
        self._saver_canvas = None
        self._saver_hint = None
        self._saver_clock = None
        if canvas is not None:
            try:
                canvas.grab_release()
            except Exception:
                pass
            try:
                canvas.destroy()
            except Exception:
                pass
            print("ui: screensaver off", flush=True)
        restored = self._backlight.restore()
        if not restored:
            # Last-resort unblank if PanelBacklight had no saved state
            try:
                path = pathlib.Path("/sys/class/backlight/10-0045/brightness")
                if path.is_file():
                    path.write_text("128\n", encoding="ascii")
                power = pathlib.Path("/sys/class/backlight/10-0045/bl_power")
                if power.is_file():
                    power.write_text("0\n", encoding="ascii")
            except OSError:
                pass
        self._apply_pixel_shift()

    def _blank_screen_now(self) -> None:
        self._close_power_menu(restore_main=True)
        self._show_screensaver(force=True)

    def _cycle_screensaver_timeout(self) -> None:
        self._idle.timeout_sec = next_timeout_preset(self._idle.timeout_sec)
        self._idle.poke()
        self._mark_settings_dirty()
        if self._saver_timeout_btn is not None:
            self._saver_timeout_btn.configure(text=timeout_label(self._idle.timeout_sec))
        self._append_log(f"TFT burn-in guard → {timeout_label(self._idle.timeout_sec)}")

    def _open_power_menu(self) -> None:
        """Confirm screen for safe Pi shutdown / reboot (kiosk has no desktop power UI)."""
        if self._power_ui_open:
            return
        # Close other overlays so POWER is always reachable
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._token_ui_open:
            self._close_update_token(restore_main=False)

        self._power_ui_open = True
        prev = self._mode
        for shell in (
            self._synth_shell,
            self._seq_shell,
            self._pads_shell,
            self._songs_shell,
            self._presets_shell,
            self._log_shell,
            self._settings_shell,
            self._home_shell,
        ):
            try:
                shell.pack_forget()
            except Exception:
                pass

        self._power_frame = tk.Frame(self._mode_host, bg="#111111")
        self._power_frame.pack(fill=tk.BOTH, expand=True)
        self._power_frame._prev_mode = prev  # type: ignore[attr-defined]

        header, body, footer = self._pack_screen_regions(
            self._power_frame,
            header_padx=10,
            header_pady=(16, 8),
            body_padx=10,
            body_pady=6,
            footer_padx=10,
            footer_pady=12,
        )
        tk.Label(
            header,
            text="POWER",
            font=("DejaVu Sans", 22, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._saver_timeout_btn = self._mk_touch_btn(
            header,
            timeout_label(self._idle.timeout_sec),
            self._cycle_screensaver_timeout,
            bg="#3c3836",
        )
        self._saver_timeout_btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=8)
        self._saver_timeout_btn.pack(side=tk.RIGHT)

        footer.columnconfigure(0, weight=1)
        footer.columnconfigure(1, weight=1)
        self._mk_touch_btn(
            footer, "CANCEL", self._close_power_menu, bg="#504945"
        ).grid(row=0, column=0, sticky="ew", padx=(0, 4), ipady=18)
        self._mk_touch_btn(
            footer, "SCREEN OFF", self._blank_screen_now, bg="#1d2021"
        ).grid(row=0, column=1, sticky="ew", padx=(4, 0), ipady=18)

        tk.Label(
            body,
            text="Shut down cleanly before unplugging. Reboot restarts into kiosk. "
            "SCREEN OFF blanks the TFT (tap to wake; playing MIDI will not). "
            "While the UI is up it also pixel-shifts so bold chrome cannot ghost.",
            font=("DejaVu Sans", 13),
            fg="#ebdbb2",
            bg="#111111",
            wraplength=740,
            justify=tk.LEFT,
            anchor="w",
        ).pack(side=tk.TOP, fill=tk.X, padx=2, pady=(4, 16))

        # Equal-height actions — pack(expand) alone gives the first button the leftover
        actions = tk.Frame(body, bg="#111111")
        actions.pack(fill=tk.BOTH, expand=True)
        actions.rowconfigure(0, weight=1, uniform="power")
        actions.rowconfigure(1, weight=1, uniform="power")
        actions.columnconfigure(0, weight=1)
        shut = self._mk_touch_btn(
            actions, "SHUT DOWN", lambda: self._pi_power("poweroff"), bg="#9d0006"
        )
        shut.configure(font=("DejaVu Sans", 18, "bold"))
        shut.grid(row=0, column=0, sticky="nsew", pady=(0, 6), ipady=12)
        reboot = self._mk_touch_btn(
            actions, "REBOOT", lambda: self._pi_power("reboot"), bg="#d79921"
        )
        reboot.configure(font=("DejaVu Sans", 18, "bold"))
        reboot.grid(row=1, column=0, sticky="nsew", pady=(6, 0), ipady=12)

    def _close_power_menu(self, restore_main: bool = True) -> None:
        if not self._power_ui_open:
            return
        prev = "synth"
        if self._power_frame is not None:
            prev = getattr(self._power_frame, "_prev_mode", "synth")
            self._power_frame.destroy()
            self._power_frame = None
        self._power_ui_open = False
        if restore_main:
            self._switch_mode(prev if prev in UI_MODES else "synth")

    def _pi_power(self, action: str) -> None:
        """Reboot/poweroff via pi-power.sh / systemctl — never just quit the app."""
        action = "reboot" if action == "reboot" else "poweroff"
        self._append_log(f"Power → {action}…")
        self.last_var.set(f"Powering {action}…")
        try:
            self._panic()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        except Exception:
            pass
        # Give the UI a moment to flush logs / audio stop
        self.root.update_idletasks()

        power_sh = str(pathlib.Path(__file__).resolve().parent / "pi-power.sh")

        def _run() -> None:
            # Only commands covered by /etc/sudoers.d/midi-tone-power (plain
            # poweroff/reboot — flag variants need a password and must not be
            # used here). Never treat app exit as shutdown.
            cmds = [
                ["sudo", "-n", power_sh, action],
                ["sudo", "-n", "systemctl", action],
                (
                    ["sudo", "-n", "poweroff"]
                    if action == "poweroff"
                    else ["sudo", "-n", "reboot"]
                ),
            ]
            last_err = ""
            for cmd in cmds:
                try:
                    r = subprocess.run(
                        cmd,
                        capture_output=True,
                        text=True,
                        timeout=25,
                        check=False,
                    )
                    if r.returncode == 0:
                        return
                    last_err = (r.stderr or r.stdout or f"exit {r.returncode}").strip()
                except Exception as exc:
                    last_err = str(exc)
            self._q_put(
                (
                    "log",
                    f"Power {action} failed: {last_err or 'no permission'} — "
                    f"run ./install-kiosk.sh (adds sudoers) or: sudo systemctl {action}",
                    False,
                )
            )

        threading.Thread(target=_run, daemon=True).start()
        # Also show immediate feedback on the confirm screen
        if self._power_frame is not None:
            tk.Label(
                self._power_frame,
                text=f"Sending {action}… screen will go dark.",
                font=("DejaVu Sans", 14, "bold"),
                fg="#fabd2f",
                bg="#111111",
            ).pack(fill=tk.X, padx=12, pady=8)

    def _on_close(self) -> None:
        self._stop.set()
        self._cancel_screensaver_tick()
        try:
            self._hide_screensaver()
        except Exception:
            pass
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        try:
            self._songs.stop()
            self._songs.close_outport()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=False)
        except Exception:
            pass
        if self._inport is not None:
            try:
                self._inport.close()
            except Exception:
                pass
        self.engine.stop()
        self.root.destroy()

    def run(self) -> None:
        self.root.mainloop()


def _acquire_singleton_lock() -> Optional[Any]:
    """Prevent two midi-tone GUIs (kiosk restart + deploy launch) fighting for CPU/audio."""
    try:
        import fcntl  # Unix / Pi only
    except ImportError:
        return None
    path = pathlib.Path(os.environ.get("MIDI_TONE_LOCK", "/tmp/midi-tone.lock"))
    try:
        fp = open(path, "a+", encoding="utf-8")
        fcntl.flock(fp.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print(
            "midi-tone: already running (singleton lock) — exiting this instance",
            flush=True,
        )
        sys.exit(0)
    try:
        fp.seek(0)
        fp.truncate()
        fp.write(f"{os.getpid()}\n")
        fp.flush()
    except Exception:
        pass
    return fp

def main() -> None:
    import faulthandler

    faulthandler.enable()
    parser = argparse.ArgumentParser(description="MIDI → wavetable soft-synth with event UI")
    parser.add_argument("--input", "-i", default="", help="MIDI input name substring")
    parser.add_argument("--list", "-l", action="store_true", help="List MIDI inputs")
    parser.add_argument(
        "--voices",
        type=int,
        default=DEFAULT_MAX_VOICES,
        help=f"Max polyphony (default {DEFAULT_MAX_VOICES})",
    )
    parser.add_argument(
        "--waves-dir",
        type=pathlib.Path,
        default=DEFAULT_WAVETABLE_DIR,
        help="Directory of single-cycle WAV voices",
    )
    parser.add_argument(
        "--fullscreen",
        action="store_true",
        help="Fill the screen (used by kiosk.sh)",
    )
    args = parser.parse_args()

    lock_fp = None
    if not args.list:
        lock_fp = _acquire_singleton_lock()

    try:
        mido.set_backend("mido.backends.rtmidi")
    except Exception:
        pass

    print("midi-tone: starting", flush=True)
    app = MidiToneApp(
        port_filter=args.input,
        list_only=args.list,
        max_voices=args.voices,
        waves_dir=args.waves_dir,
        fullscreen=args.fullscreen,
    )
    if not args.list:
        print("midi-tone: entering mainloop", flush=True)
        try:
            app.run()
        finally:
            if lock_fp is not None:
                try:
                    lock_fp.close()
                except Exception:
                    pass
        print("midi-tone: mainloop exited", flush=True)


if __name__ == "__main__":
    main()
