#!/usr/bin/env python3
"""
midi-tone — Phase 0 diagnostic turned tiny DIY soft-synth.

MPK (or any MIDI in) → wavetable soft-synth + event UI.
Keep lean for Raspberry Pi 2 (wavetable synth, capped polyphony).
"""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import queue
import shutil
import sys
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


SAMPLE_RATE = 44100
BLOCKSIZE = 1024
LATENCY_SEC = 0.08
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
SETTINGS_VERSION = 1


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

    def process(self, buf: np.ndarray) -> None:
        n = len(buf)
        if n == 0:
            return
        if n > self._tmp.shape[0]:
            self._tmp = np.zeros(n, dtype=np.float32)
            self._wet = np.zeros(n, dtype=np.float32)
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
            # Vectorized circular read
            idx = (pos - ds + np.arange(n, dtype=np.int32)) % dlen
            np.take(dbuf, idx, out=wet)
            # Write input + feedback * delayed
            write_idx = (pos + np.arange(n, dtype=np.int32)) % dlen
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
            wet.fill(0.0)
            for tap, g in zip(taps, gains):
                tap = min(tap, rlen - 1)
                idx = (pos - tap + np.arange(n, dtype=np.int32)) % rlen
                wet += np.take(rbuf, idx) * np.float32(g)
            # Soften highs with a short moving average (size → darker)
            win = max(1, int(1 + size * 12))
            if win > 1:
                kernel = np.ones(win, dtype=np.float32) / np.float32(win)
                wet[:] = np.convolve(wet, kernel, mode="same")
            fb = 0.25 + 0.45 * size
            write_idx = (pos + np.arange(n, dtype=np.int32)) % rlen
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


def load_wavetables(directory: pathlib.Path) -> Dict[str, np.ndarray]:
    """Built-ins first, then any *.wav in directory (stem = voice name)."""
    tables = _builtin_tables()
    if directory.is_dir():
        for path in sorted(directory.glob("*.wav")):
            name = path.stem.lower().strip()
            if not name:
                continue
            # Keep core procedural oscillators; files can add/replace everything else
            if name in ("sine", "square", "saw", "triangle"):
                continue
            try:
                tables[name] = _resample_cycle(_load_wav_mono(path))
            except Exception as exc:
                print(f"wavetable skip {path.name}: {exc}", flush=True)
    return tables


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


def draw_waveform_on_canvas(
    canvas: "tk.Canvas",
    samples: np.ndarray,
    *,
    color: str = "#83a598",
    grid_color: str = "#3c3836",
) -> None:
    """Paint a normalized polyline waveform into a Tk canvas."""
    try:
        canvas.delete("wave")
        w = int(canvas.winfo_width())
        h = int(canvas.winfo_height())
    except Exception:
        return
    if w < 8 or h < 8:
        return
    # Midline
    mid = h * 0.5
    canvas.create_line(0, mid, w, mid, fill=grid_color, tags="wave")
    if samples is None or len(samples) < 2:
        return
    pts = downsample_waveform(samples, max(32, w // 2))
    peak = float(np.max(np.abs(pts))) or 1.0
    y_scale = (h * 0.42) / peak
    coords: List[float] = []
    n = len(pts)
    for i, v in enumerate(pts):
        x = (i / max(1, n - 1)) * (w - 1)
        y = mid - float(v) * y_scale
        coords.extend((x, y))
    canvas.create_line(*coords, fill=color, width=2, smooth=True, tags="wave")


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
    MAX_DRUM_HITS = 16  # full MPK A+B pad bank polyphony
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
        self._level = 1.0
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
        # Optional global wet after keys+drums are summed (separate from inserts)
        self._bus_fx = MixBusFx(self.sample_rate)
        self._fx_edit_kind = "voice"  # voice | drum | bus
        self._fx_edit_drum = "kick"
        self._key_bus = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._drum_bus = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._fx_tmp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
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
                or any(
                    fx.sample_rate != self.sample_rate
                    for fx in list(self._voice_fx.values()) + list(self._drum_fx.values())
                )
            )
            if need:
                self._bus_fx = self._clone_fx(self._bus_fx, self.sample_rate)
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

    def set_level(self, value: float) -> None:
        """Master level. MIDI CC is 0–127; ease the bottom so mid-knob isn't tiny."""
        if value > 1.0:
            # Slightly loud-biased curve: mid CC still usable on a powered speaker
            x = max(0.0, min(1.0, float(value) / 127.0))
            value = x ** 0.65
        with self._lock:
            self._level = max(0.0, min(1.0, float(value)))

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

    def set_fx_edit_bus(self) -> None:
        """Point knobs at the master mix-bus FX."""
        with self._lock:
            self._fx_edit_kind = "bus"

    def fx_edit_label(self) -> str:
        with self._lock:
            if self._fx_edit_kind == "bus" or self._bus_fx_mode:
                return "bus"
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
                "level": self._level,
                "attack": self._attack_sec,
                "release": self._release_sec,
                "vib_hz": self._vib_hz,
                "vib_depth": self._vib_depth_semis,
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
                "level": float(self._level),
                "attack_sec": float(self._attack_sec),
                "release_sec": float(self._release_sec),
                "vib_hz": float(self._vib_hz),
                "vib_depth": float(self._vib_depth_semis),
                "drum_pitch": float(self._drum_pitch),
                "drum_decay": float(self._drum_decay),
                "drum_noise": float(self._drum_noise),
                "drum_tone": float(self._drum_tone),
                # Per-instrument inserts + optional master bus
                "voice_fx": {k: v.snapshot() for k, v in self._voice_fx.items()},
                "drum_fx": {k: v.snapshot() for k, v in self._drum_fx.items()},
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
            if "level" in data:
                self._level = max(0.0, min(1.0, float(data["level"])))
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

    def _callback(self, outdata, frames, time_info, status) -> None:  # noqa: ANN001
        del time_info, status
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
            mod = self._mod
            vib_hz = self._vib_hz
            vib_depth = self._vib_depth_semis
            table = self._morph_table
            tone = self._tone
            level = self._level
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
            hz = midi_to_hz(v.note) * (2.0 ** ((bend + vib_semis) / 12.0))
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
                fx.process(bucket)
                key_bus += bucket

        # Procedural ch10 drums — per-model FX, then drum bus
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
            apply_model_fx=True,
        )

        # Tone filter on keys only (drums keep their own tone macro)
        if tone < 0.999 and frames > 0:
            win = max(1, int(round((1.0 - tone) * 48.0)))
            if win > 1:
                kernel = np.ones(win, dtype=np.float32) / np.float32(win)
                pad_left = np.full(win - 1, self._filter_state, dtype=np.float32)
                padded = np.concatenate([pad_left, key_bus])
                filtered = np.convolve(padded, kernel, mode="valid")
                key_bus[:] = filtered[:frames]
                self._filter_state = float(key_bus[-1])
            else:
                self._filter_state = float(key_bus[-1]) if frames else self._filter_state
        elif frames > 0:
            self._filter_state = float(key_bus[-1])

        buf[:] = key_bus
        buf += drum_bus

        # Master mix-bus FX (optional global wet — separate from per-voice/per-drum inserts)
        if frames > 0:
            with self._lock:
                bus_fx = self._bus_fx
            bus_fx.process(buf)

        if level < 0.999:
            buf *= np.float32(level)
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

        def _synth_hit(hit: DrumHit) -> Tuple[np.ndarray, float]:
            # Keep hit snapshot in sync so UI/debug stay honest; audio uses live macros
            hit.pitch = pitch
            hit.decay = decay
            hit.noise = noise_amt
            hit.tone = tone
            t = (hit.pos + arange) * np.float32(inv_sr)
            white = (np.random.random(frames).astype(np.float32) * 2.0 - 1.0)
            win = max(1, int(round((1.0 - tone) * 16.0)))
            if win <= 1:
                noise = white
                hit.noise_state = float(white[-1])
            else:
                kernel = np.ones(win, dtype=np.float32) / np.float32(win)
                pad = np.full(win - 1, hit.noise_state, dtype=np.float32)
                noise = np.convolve(np.concatenate([pad, white]), kernel, mode="valid")[:frames]
                hit.noise_state = float(noise[-1])

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
            fx.process(scratch)
            buf += scratch
        return dead


@dataclass
class LoopEvent:
    t: float  # seconds from record start
    on: bool
    channel: int
    note: int
    velocity: int


def trim_loop_take(
    events: List[LoopEvent],
    *,
    default_gap: float = 0.35,
    min_gap: float = 0.05,
    max_gap: float = 2.0,
) -> Tuple[List[LoopEvent], float]:
    """
    Trim leading/trailing dead space from a free-timing take.

    - Shift so the first note-on starts at t=0 (drop pre-roll before the groove).
    - Trailing silence after the last note-on is capped to the largest gap
      between consecutive note-ons (so STOP lag doesn't inflate the loop).
    - Note-offs after the last hit are still kept; trail is measured from ons.
    """
    if not events:
        return [], 0.0
    ons = sorted(e.t for e in events if e.on)
    if not ons:
        # Degenerate: only note-offs — keep relative timing, short length
        t0 = min(e.t for e in events)
        shifted = [
            LoopEvent(
                t=max(0.0, e.t - t0),
                on=e.on,
                channel=e.channel,
                note=e.note,
                velocity=e.velocity,
            )
            for e in events
        ]
        length = max(e.t for e in shifted) + min_gap
        return shifted, max(min_gap, length)

    t0 = ons[0]
    gaps = [ons[i + 1] - ons[i] for i in range(len(ons) - 1) if ons[i + 1] > ons[i]]
    if gaps:
        trail = max(min_gap, min(max_gap, max(gaps)))
    else:
        # Single hit: small default pad (not the whole time spent hitting STOP)
        trail = max(min_gap, min(max_gap, default_gap))

    last_on = ons[-1]
    last_ev = max(e.t for e in events)
    # Loop end from first hit: last onset + trail, but never cut off a later note-off
    end_abs = max(last_on + trail, last_ev + 0.01)
    length = max(min_gap, end_abs - t0)

    shifted = [
        LoopEvent(
            t=max(0.0, e.t - t0),
            on=e.on,
            channel=e.channel,
            note=e.note,
            velocity=e.velocity,
        )
        for e in events
        if e.t >= t0 - 1e-6
    ]
    # Drop events that fall past the trimmed end (shouldn't happen often)
    shifted = [e for e in shifted if e.t <= length + 1e-6]
    if not shifted:
        return [], 0.0
    return shifted, float(length)


class MidiLooper:
    """Simple free-timing MIDI note looper (record → play on repeat)."""

    def __init__(
        self,
        engine: "SineEngine",
        emit,  # callable matching event_q tuples: ("on",...) / ("off",...)
    ) -> None:
        self._engine = engine
        self._emit = emit
        self._lock = threading.Lock()
        self._events: List[LoopEvent] = []
        self._recording = False
        self._playing = False
        self._rec_t0 = 0.0
        self._loop_len = 0.0
        self._stop_play = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._held: set[Tuple[int, int]] = set()

    def is_recording(self) -> bool:
        with self._lock:
            return self._recording

    def is_playing(self) -> bool:
        with self._lock:
            return self._playing

    def event_count(self) -> int:
        with self._lock:
            return len(self._events)

    def loop_length(self) -> float:
        with self._lock:
            return self._loop_len

    def status(self) -> Dict[str, object]:
        with self._lock:
            return {
                "recording": self._recording,
                "playing": self._playing,
                "events": len(self._events),
                "length": self._loop_len,
            }

    def start_record(self) -> None:
        self.stop_playback()
        with self._lock:
            self._events = []
            self._recording = True
            self._rec_t0 = time.monotonic()
            self._loop_len = 0.0

    def stop_record(self) -> None:
        with self._lock:
            if not self._recording:
                return
            self._recording = False
            if self._events:
                trimmed, length = trim_loop_take(list(self._events))
                self._events = trimmed
                self._loop_len = length
            else:
                self._loop_len = 0.0

    def toggle_record(self) -> bool:
        if self.is_recording():
            self.stop_record()
            return False
        self.start_record()
        return True

    def record_note(self, on: bool, channel: int, note: int, velocity: int) -> None:
        with self._lock:
            if not self._recording:
                return
            t = time.monotonic() - self._rec_t0
            self._events.append(
                LoopEvent(
                    t=t,
                    on=on,
                    channel=channel & 0x0F,
                    note=note & 0x7F,
                    velocity=max(1, min(127, int(velocity))) if on else 0,
                )
            )

    def clear(self) -> None:
        self.stop_playback()
        with self._lock:
            self._events = []
            self._loop_len = 0.0
            self._recording = False

    def snapshot(self) -> Tuple[List[LoopEvent], float]:
        with self._lock:
            return list(self._events), float(self._loop_len)

    def start_playback(self) -> bool:
        if self.is_recording():
            self.stop_record()
        with self._lock:
            if not self._events or self._loop_len <= 0.0:
                return False
            if self._playing:
                return True
            self._playing = True
            self._stop_play.clear()
        self._thread = threading.Thread(target=self._play_loop, daemon=True)
        self._thread.start()
        return True

    def stop_playback(self) -> None:
        self._stop_play.set()
        thread = self._thread
        if thread is not None and thread.is_alive():
            thread.join(timeout=1.5)
        self._thread = None
        with self._lock:
            self._playing = False
        self._release_held()

    def toggle_playback(self) -> bool:
        if self.is_playing():
            self.stop_playback()
            return False
        return self.start_playback()

    def _release_held(self) -> None:
        held = list(self._held)
        self._held.clear()
        for ch, note in held:
            try:
                self._engine.note_off(ch, note)
            except Exception:
                pass
            try:
                self._emit(("off", ch, note))
            except Exception:
                pass

    def _play_loop(self) -> None:
        while not self._stop_play.is_set():
            with self._lock:
                events = list(self._events)
                loop_len = self._loop_len
            if not events or loop_len <= 0.0:
                break
            cycle_t0 = time.monotonic()
            self._release_held()
            for ev in events:
                if self._stop_play.is_set():
                    self._release_held()
                    with self._lock:
                        self._playing = False
                    return
                target = cycle_t0 + ev.t
                while True:
                    remain = target - time.monotonic()
                    if remain <= 0:
                        break
                    if self._stop_play.wait(min(0.003, remain)):
                        self._release_held()
                        with self._lock:
                            self._playing = False
                        return
                if ev.on:
                    self._engine.note_on(ev.channel, ev.note, ev.velocity)
                    self._held.add((ev.channel, ev.note))
                    self._emit(("on", ev.channel, ev.note, ev.velocity))
                else:
                    self._engine.note_off(ev.channel, ev.note)
                    self._held.discard((ev.channel, ev.note))
                    self._emit(("off", ev.channel, ev.note))
            end = cycle_t0 + loop_len
            while True:
                remain = end - time.monotonic()
                if remain <= 0:
                    break
                if self._stop_play.wait(min(0.003, remain)):
                    self._release_held()
                    with self._lock:
                        self._playing = False
                    return
            self._release_held()
        with self._lock:
            self._playing = False


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
            "version": 2,
            "length": float(self.length),
            "trigger_mode": mode,
            "voice_mode": vmode,
            "morph_a": str(self.morph_a or ""),
            "morph_b": str(self.morph_b or ""),
            "morph": float(self.morph),
            "out_channel": och,
            "local_synth": bool(self.local_synth),
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
            self._cells[idx].voice_mode = PHRASE_VOICE_FOLLOW
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(("log", f"Phrase {phrase_pad_label(idx)} voice FOLLOW", False))
        return True

    def lock_voice_from_engine(self, idx: int) -> bool:
        """Snapshot current global morph onto this pad (LOCKED)."""
        if not (0 <= idx < PHRASE_PAD_COUNT):
            return False
        a, b, morph = self._engine.snapshot_morph()
        with self._lock:
            c = self._cells[idx]
            c.voice_mode = PHRASE_VOICE_LOCKED
            c.morph_a = a
            c.morph_b = b
            c.morph = float(morph)
            self._selected = idx
        self.save_cell(idx)
        self._emit(("phrase",))
        self._emit(
            ("log", f"Phrase {phrase_pad_label(idx)} voice LOCKED ({a}→{b} {int(morph*100)}%)", False)
        )
        return True

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
        view: str = "edit",
    ) -> str:
        with self._lock:
            filled = sum(1 for c in self._cells if not c.is_empty())
            rec = self._recording_cell
            playing = sorted(self._playing.keys())
            sel = self._selected
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
                f"EDIT {phrase_pad_label(sel)} · {trig} · {v} · {och} · {syn} · "
                f"{filled}/16"
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
            )
            self._recording_cell = idx
            self._rec_t0 = time.monotonic()
            self._selected = idx
        self._emit(("phrase",))
        self._emit(("log", f"Phrase REC {phrase_pad_label(idx)} armed", False))
        return True

    def stop_record(self) -> Optional[int]:
        """Finish recording. Returns cell index, or None if not recording."""
        with self._lock:
            idx = self._recording_cell
            if idx is None:
                return None
            cell = self._cells[idx]
            if cell.events:
                trimmed, length = trim_loop_take(list(cell.events))
                cell.events = trimmed
                cell.length = length
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
        Touch square or MPK pad hit (when not recording drums).
        Empty → arm record (EDIT); filled → launch/toggle; touch on armed cell → stop record.
        PLAY view passes allow_record=False so empty pads only select.
        Returns a short action tag for the UI/log.
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
        if rec is not None and not from_touch:
            # Hardware pad while recording is handled by MIDI path as drums
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
                    ch, n, velocity, timbre=timbre, fx_name=use_fx
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


def looper_events_to_midifile(
    events: List[LoopEvent],
    loop_len: float,
    bpm: float = DEFAULT_SONG_BPM,
    ticks_per_beat: int = 480,
) -> mido.MidiFile:
    """Build a Type 0 SMF from free-timing looper note events."""
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
        self._tables = load_wavetables(waves_dir)
        self.engine = SineEngine(self._tables, max_voices=max_voices)
        self._inport: Optional[mido.ports.BaseInput] = None
        self._poll_thread: Optional[threading.Thread] = None
        self._stop = threading.Event()
        self._voice_names = self.engine.voice_names
        self._voice_index = 0
        self._fullscreen = bool(fullscreen)

        if list_only:
            self._print_ports()
            return

        port_name = self._pick_port()
        if port_name is None:
            sys.exit("No MIDI input ports found. Is the MPK plugged in?")

        print(f"midi: opening input '{port_name}'", flush=True)
        print(f"voices: {', '.join(self._voice_names)}", flush=True)

        self._full_vel = True

        # Create the Tk root BEFORE opening PortAudio — on Pi + labwc/Xwayland,
        # starting audio first then Tk can abort during tk.Tk() with no traceback.
        print("ui: creating Tk root", flush=True)
        self.root = tk.Tk()
        print("ui: Tk root ok", flush=True)
        self.root.title("midi-tone")
        self.root.geometry("800x420")
        self.root.configure(bg="#111111")
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)
        if self._fullscreen:
            # Kiosk: fill the screen (Openbox also forces maximize/fullscreen)
            try:
                self.root.attributes("-fullscreen", True)
            except Exception:
                try:
                    self.root.attributes("-zoomed", True)
                except Exception:
                    self.root.state("zoomed")
            print("ui: fullscreen", flush=True)
        self.root.update_idletasks()

        self.engine.start()
        print("midi: audio engine started", flush=True)
        self._inport = mido.open_input(port_name)
        print("midi: input port open", flush=True)
        self._poll_thread = threading.Thread(target=self._midi_loop, daemon=True)
        self._poll_thread.start()
        print("midi: poll thread started", flush=True)

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
        self._kit_wave_canvas: Optional[tk.Canvas] = None
        self._kit_status_var = tk.StringVar(value="")
        self._kit_selected_note = 36  # factory kick
        self._mode = "synth"  # synth | looper | pads | songs | log | presets
        self._mode_btns: Dict[str, tk.Button] = {}
        self._looper = MidiLooper(self.engine, self._q_put)
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
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._phrase_shell: Optional[tk.Frame] = None
        self._loop_status_var = tk.StringVar(value="Loop empty — tap RECORD, play notes, STOP, then PLAY.")
        self._loop_rec_btn: Optional[tk.Button] = None
        self._loop_play_btn: Optional[tk.Button] = None
        self._preset_status_var = tk.StringVar(value="Tap a slot, then LOAD or SAVE.")
        self._preset_slot = 0
        self._preset_slot_btns: Dict[int, tk.Button] = {}
        self._active_preset_name: Optional[str] = None
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

        # Persistent mode navigation (always visible)
        self._nav = tk.Frame(self.root, bg="#1d2021")
        self._nav.pack(side=tk.TOP, fill=tk.X, padx=0, pady=0)
        tk.Label(
            self._nav, text="midi-tone", font=("DejaVu Sans", 14, "bold"),
            fg="#fbf1c7", bg="#1d2021", padx=10, pady=8,
        ).pack(side=tk.LEFT)
        nav_modes = tk.Frame(self._nav, bg="#1d2021")
        nav_modes.pack(side=tk.RIGHT, padx=4, pady=4)
        for key, label in (
            ("synth", "SYNTH"),
            ("looper", "LOOPER"),
            ("pads", "PADS"),
            ("songs", "SONGS"),
            ("presets", "PRESETS"),
            ("log", "LOG"),
        ):
            btn = self._mk_touch_btn(
                nav_modes, label, lambda m=key: self._switch_mode(m), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 10, "bold"), pady=8, padx=4)
            btn.pack(side=tk.LEFT, padx=1)
            self._mode_btns[key] = btn

        # Mode content host
        self._mode_host = tk.Frame(self.root, bg="#111111")
        self._mode_host.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        self._synth_shell = tk.Frame(self._mode_host, bg="#111111")
        self._looper_shell = tk.Frame(self._mode_host, bg="#111111")
        self._pads_shell = tk.Frame(self._mode_host, bg="#111111")
        self._phrase_shell = self._pads_shell
        self._songs_shell = tk.Frame(self._mode_host, bg="#111111")
        self._presets_shell = tk.Frame(self._mode_host, bg="#111111")
        self._log_shell = tk.Frame(self._mode_host, bg="#111111")

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
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="Synth", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text=port_name, font=("DejaVu Sans", 11),
            fg="#8ec07c", bg="#111111",
        ).pack(side=tk.RIGHT)

        self.last_var = tk.StringVar(value="Waiting for MIDI…")
        last_lbl = tk.Label(
            self._main, textvariable=self.last_var,
            font=("DejaVu Sans Mono", 15, "bold"), fg="#fabd2f", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        last_lbl.pack(fill=tk.X, padx=8, pady=4)

        self.active_var = tk.StringVar(value="Active notes: —")
        active_lbl = tk.Label(
            self._main, textvariable=self.active_var,
            font=("DejaVu Sans", 12), fg="#83a598", bg="#111111", anchor="w",
        )
        active_lbl.pack(fill=tk.X, padx=8)

        self.mod_var = tk.StringVar(value=self._format_mod_line())
        mod_lbl = tk.Label(
            self._main, textvariable=self.mod_var,
            font=("DejaVu Sans Mono", 11), fg="#d3869b", bg="#111111", anchor="w",
        )
        mod_lbl.pack(fill=tk.X, padx=8, pady=(2, 4))

        self._wave_caption = tk.Label(
            self._main,
            text="Morph cycle",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
            anchor="w",
        )
        self._wave_caption.pack(fill=tk.X, padx=8, pady=(4, 0))
        self._wave_canvas = tk.Canvas(
            self._main,
            height=110,
            bg="#1d2021",
            highlightthickness=1,
            highlightbackground="#3c3836",
            bd=0,
        )
        self._wave_canvas.pack(fill=tk.BOTH, expand=True, padx=8, pady=(2, 8))
        self._wave_canvas.bind(
            "<Configure>", lambda _e: self._paint_synth_waveform(force=True)
        )

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

        self._build_looper_mode()
        self._build_pads_mode()
        self._build_songs_mode()
        self._build_presets_mode()
        self._build_log_mode()
        self._switch_mode("synth")

        # Restore last session (full vel, morph pair, knob-shaped tone, etc.)
        restored = self._load_settings_file(SETTINGS_PATH)
        self._paint_full_vel_btn()
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._sync_voice_index_from_morph()
        # Rebuild pads chrome so restored PLAY/EDIT + OUT mode paint correctly
        self._build_pads_mode()
        self._paint_song_slots()
        self._refresh_song_status()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())

        self._append_log(f"Listening on: {port_name}")
        self._append_log(f"Loaded {len(self._voice_names)} voices — VOICES grid / MORPH pair.")
        self._append_log(
            "MPK knobs (keys): morph / tone / attack / release / vib / — / level"
        )
        self._append_log(
            "Pads = analog drum voices. After a pad (or DRUM LOCK): knobs → "
            "pitch / stretch / noise / drum-tone / — / — / — / level"
        )
        self._append_log("Modes: SYNTH / LOOPER / PADS / SONGS / PRESETS / LOG (top right).")
        if seeded:
            self._append_log(
                f"Added {seeded} demo song(s) from demo-songs/ (offline classical pack)."
            )
        if restored:
            self._append_log(f"Restored session from {SETTINGS_PATH.name}")
        else:
            self._append_log("No settings.json yet — changes will autosave.")
        self._append_log("If knobs do nothing: Prog Select + Pad 1 (MPC program).")
        print("ui: construction complete", flush=True)
        self.root.after(2000, self._autosave_tick)

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
                f"Lvl:{int(st['level'] * 127):3d}"
            )
        if self.engine.drum_knob_focus():
            return (
                "DRUM MODE  "
                f"Pitch:{int(st['drum_pitch'] * 127):3d}  "
                f"Stretch:{int(st['drum_decay'] * 127):3d}  "
                f"Noise:{int(st['drum_noise'] * 127):3d}  "
                f"Tone:{int(st['drum_tone'] * 127):3d}  "
                f"Lvl:{int(st['level'] * 127):3d}"
            )
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            morph_txt = left
        else:
            morph_txt = f"{left}→{right}"
        return (
            f"Morph:{int(blend * 100):3d}% ({morph_txt})  "
            f"Tone:{int(st['tone'] * 127):3d}  "
            f"Lvl:{int(st['level'] * 127):3d}  "
            f"Bend:{st['bend']:+.2f}  "
            f"Vib:{int(st['mod'] * 127)}"
        )

    def _overlay_busy(self) -> bool:
        return self._grid_open or self._morph_ui_open or self._kit_ui_open

    def _session_dict(self) -> Dict[str, Any]:
        return {
            "version": SETTINGS_VERSION,
            "full_velocity": bool(self._full_vel),
            "active_preset": self._active_preset_name,
            "synth": self.engine.snapshot_settings(),
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
        }

    def _apply_session_dict(self, data: Dict[str, Any]) -> None:
        self._suppress_autosave = True
        try:
            if "full_velocity" in data:
                self._full_vel = bool(data["full_velocity"])
            if "active_preset" in data:
                name = data["active_preset"]
                self._active_preset_name = str(name) if name else None
            synth = data.get("synth")
            if isinstance(synth, dict):
                self.engine.apply_settings(synth)
            pads = data.get("pads")
            if isinstance(pads, dict):
                view = str(pads.get("view", self._pads_view) or "edit")
                self._pads_view = "play" if view == "play" else "edit"
                out = str(pads.get("out_mode", self._phrase_out_mode) or "local")
                self._phrase_out_mode = out if out in SONG_OUT_MODES else "local"
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
            side=tk.LEFT, padx=2, ipady=4
        )
        self._song_bpm_lbl = tk.Label(
            bpm_row,
            text=self._song_bpm_label(),
            font=("DejaVu Sans", 14, "bold"),
            fg="#fabd2f",
            bg="#111111",
            padx=8,
        )
        self._song_bpm_lbl.pack(side=tk.LEFT)
        self._mk_touch_btn(bpm_row, "BPM +", lambda: self._song_nudge_bpm(1), bg="#3c3836").pack(
            side=tk.LEFT, padx=2, ipady=4
        )
        self._mk_touch_btn(bpm_row, "−5", lambda: self._song_nudge_bpm(-5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2, ipady=4
        )
        self._mk_touch_btn(bpm_row, "+5", lambda: self._song_nudge_bpm(5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2, ipady=4
        )

        status = tk.Label(
            shell, textvariable=self._song_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(2, 4))

        # Chunky list with dedicated scroll targets (no tiny scrollbar)
        list_wrap = tk.Frame(shell, bg="#111111")
        list_wrap.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)

        self._song_up_btn = self._mk_touch_btn(
            list_wrap, "▲  UP", lambda: self._song_scroll_by(-SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_up_btn.configure(font=("DejaVu Sans", 16, "bold"), pady=10)
        self._song_up_btn.pack(fill=tk.X, pady=(0, 4), ipady=6)

        rows = tk.Frame(list_wrap, bg="#111111")
        rows.pack(fill=tk.BOTH, expand=True)
        self._song_row_btns = []
        for i in range(SONG_LIST_VISIBLE):
            btn = self._mk_touch_btn(
                rows,
                "",
                lambda idx=i: self._select_song_row(idx),
                bg="#3c3836",
            )
            btn.configure(
                font=("DejaVu Sans", 14, "bold"),
                anchor="w",
                justify=tk.LEFT,
                pady=12,
            )
            btn.pack(fill=tk.BOTH, expand=True, pady=2, ipady=8)
            self._song_row_btns.append(btn)

        self._song_down_btn = self._mk_touch_btn(
            list_wrap, "▼  DOWN", lambda: self._song_scroll_by(SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_down_btn.configure(font=("DejaVu Sans", 16, "bold"), pady=10)
        self._song_down_btn.pack(fill=tk.X, pady=(4, 0), ipady=6)

        row_a = tk.Frame(shell, bg="#111111")
        row_a.pack(fill=tk.X, padx=8, pady=(6, 3))
        self._song_play_btn = self._mk_touch_btn(
            row_a, "PLAY", self._song_toggle_play, bg="#689d6a"
        )
        self._song_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12)
        self._mk_touch_btn(row_a, "STOP", self._song_stop, bg="#d79921").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=12
        )

        row_b = tk.Frame(shell, bg="#111111")
        row_b.pack(fill=tk.X, padx=8, pady=3)
        self._mk_touch_btn(
            row_b, "SAVE LOOP", self._song_save_from_looper, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10)
        self._mk_touch_btn(row_b, "DELETE", self._song_delete_selected, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )

        row_c = tk.Frame(shell, bg="#111111")
        row_c.pack(fill=tk.X, padx=8, pady=(3, 8))
        self._song_out_btn = self._mk_touch_btn(
            row_c, "OUT: LOCAL", self._song_cycle_out_mode, bg="#3c3836"
        )
        self._song_out_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)
        self._song_loop_btn = self._mk_touch_btn(
            row_c, "SONG LOOP: OFF", self._song_toggle_loop, bg="#3c3836"
        )
        self._song_loop_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

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
        title = self._song_title_from_file(path)
        if title.lower() == path.stem.lower() or title == path.stem:
            return f"  {path.name}"
        return f"  {title}\n  {path.name}"

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
            msg = "songs/ is empty — SAVE LOOP, or drop .mid files in. Demos seed on first launch."
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
                    self._q_put(("log", "Song empty — tap a file or SAVE LOOP", False))
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

    def _song_save_from_looper(self) -> None:
        events, loop_len = self._looper.snapshot()
        if not events or loop_len <= 0.0:
            self._song_status_var.set("Looper is empty — record something in LOOPER first.")
            return
        if self._songs.is_playing():
            self._songs.stop()
        path = self._next_take_path()
        bpm = self._songs.bpm()
        try:
            SONGS_DIR.mkdir(parents=True, exist_ok=True)
            mid = looper_events_to_midifile(events, loop_len, bpm=bpm)
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
            self._song_status_var.set(f"Saved looper → {path.name}")
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
            header, text="synth sound + full-vel",
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
        self._mk_touch_btn(footer, "LOAD", self._preset_load_selected, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "SAVE", self._preset_save_selected, bg="#689d6a").pack(
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
                name = data.get("name") or path.stem
                a = (data.get("synth") or {}).get("morph_a", "?")
                b = (data.get("synth") or {}).get("morph_b", "?")
                return f"{slot + 1}\n{name}\n{a}→{b}"
            except Exception:
                return f"{slot + 1}\n{path.stem}\n(saved)"
        return f"{slot + 1}\nEMPTY"

    def _select_preset_slot(self, slot: int) -> None:
        self._preset_slot = max(0, min(PRESET_SLOTS - 1, slot))
        path = self._preset_path(self._preset_slot)
        if path.is_file():
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} selected — LOAD to use, SAVE to overwrite."
            )
        else:
            self._preset_status_var.set(
                f"Slot {self._preset_slot + 1} empty — SAVE stores the current sound here."
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

    def _preset_save_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        payload = self._session_dict()
        payload["name"] = f"slot-{self._preset_slot + 1:02d}"
        try:
            PRESETS_DIR.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            tmp.replace(path)
            self._active_preset_name = path.stem
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._preset_status_var.set(f"Saved → {path.name}")
            self._append_log(f"Preset saved: {path.name}")
            self._paint_preset_slots()
        except Exception as exc:
            self._preset_status_var.set(f"Save failed: {exc}")
            self._append_log(f"Preset SAVE error: {exc}")

    def _preset_load_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        if not path.is_file():
            self._preset_status_var.set(f"Slot {self._preset_slot + 1} is empty.")
            return
        if self._load_settings_file(path):
            self._active_preset_name = path.stem
            self._paint_full_vel_btn()
            self._sync_voice_index_from_morph()
            if self._voice_lbl is not None:
                self._voice_lbl.configure(text=self._voice_label_text())
            self.mod_var.set(self._format_mod_line())
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._preset_status_var.set(f"Loaded {path.name}")
            self._append_log(f"Preset loaded: {path.name}")
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
            self._paint_kit_waveform(force=True)
        else:
            self._paint_synth_waveform(force=True)

    def _toggle_fx_mode(self) -> None:
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
            "(not the whole mix). Open KIT + tap a drum for that drum; "
            "close KIT for nearer morph voice. Use BUS FX for global wet."
            if on
            else "FX MODE OFF — knobs back to morph / tone / …"
        )

    def _toggle_bus_fx_mode(self) -> None:
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

    def _paint_synth_waveform(self, *, force: bool = False) -> None:
        canvas = self._wave_canvas
        if canvas is None:
            return
        if self._mode != "synth" or self._overlay_busy():
            return
        try:
            samples = self.engine.morph_cycle_copy()
            draw_waveform_on_canvas(canvas, samples, color="#83a598")
            if self._wave_caption is not None:
                a, b, blend = self.engine.morph_neighbors()
                if a == b:
                    cap = f"Morph cycle · {a}"
                else:
                    cap = f"Morph cycle · {a} → {b}  {int(blend * 100)}%"
                self._wave_caption.configure(text=cap)
        except Exception:
            if force:
                pass

    def _kit_model_selected(self) -> str:
        return drum_model_for_note(self._kit_selected_note)

    def _paint_kit_waveform(self, *, force: bool = False) -> None:
        canvas = self._kit_wave_canvas
        if canvas is None or not self._kit_ui_open:
            return
        try:
            model = self._kit_model_selected()
            samples = self.engine.preview_drum_waveform(model)
            draw_waveform_on_canvas(canvas, samples, color="#fabd2f")
            pitch, decay, noise, tone = self.engine.drum_macros()
            label = phrase_pad_label(
                max(0, min(15, self._kit_selected_note - PHRASE_PAD_BASE))
            )
            self._kit_status_var.set(
                f"{label} · {model} · pitch {int(pitch * 127)} · "
                f"stretch {int(decay * 127)} · noise {int(noise * 127)} · "
                f"tone {int(tone * 127)}"
            )
        except Exception:
            if force:
                pass

    def _paint_kit_pad_btns(self) -> None:
        for note, btn in self._kit_btns.items():
            on = note == self._kit_selected_note
            color = "#d79921" if on else "#3c3836"
            try:
                btn.configure(bg=color, activebackground=color)
            except Exception:
                pass

    def _select_kit_note(self, note: int, *, audition: bool = False) -> None:
        note = int(note) & 0x7F
        if note < PHRASE_PAD_BASE or note >= PHRASE_PAD_BASE + 16:
            return
        self._kit_selected_note = note
        self._paint_kit_pad_btns()
        self._paint_kit_waveform(force=True)
        if self.engine.fx_mode():
            self.engine.set_fx_edit_drum(drum_model_for_note(note))
            self.mod_var.set(self._format_mod_line())
        if audition:
            self.engine.note_on(DRUM_CHANNEL, note, 110)
            self._q_put(("log", f"Kit audition {drum_model_for_note(note)}", False))

    def _open_kit_explorer(self) -> None:
        """Drill-down: pick a kit pad and watch its one-shot while knobs move."""
        if self._kit_ui_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)

        # If insert FX MODE is on, point knobs at the selected drum.
        # If BUS FX is on, keep global edit (kit is audition/preview only).
        # Otherwise turn on DRUM MODE so knobs reshape the one-shot body.
        if self.engine.fx_mode():
            self.engine.set_fx_edit_drum(self._kit_model_selected())
            self.mod_var.set(self._format_mod_line())
        elif self.engine.bus_fx_mode():
            self.mod_var.set(self._format_mod_line())
        elif not self.engine.drum_mode():
            self.engine.set_drum_mode(True)
            self._paint_drum_lock_btn()
            self.mod_var.set(self._format_mod_line())

        self._kit_ui_open = True
        self._synth_shell.pack_forget()

        self._kit_frame = tk.Frame(self._mode_host, bg="#111111")
        self._kit_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._kit_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="DRUM KIT",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text="tap a pad · knobs reshape the wave",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            self._kit_frame,
            textvariable=self._kit_status_var,
            font=("DejaVu Sans Mono", 11),
            fg="#fabd2f",
            bg="#111111",
            anchor="w",
        )
        status.pack(fill=tk.X, padx=8, pady=(0, 2))

        self._kit_wave_canvas = tk.Canvas(
            self._kit_frame,
            height=100,
            bg="#1d2021",
            highlightthickness=1,
            highlightbackground="#3c3836",
            bd=0,
        )
        self._kit_wave_canvas.pack(fill=tk.X, padx=8, pady=(0, 4))
        self._kit_wave_canvas.bind(
            "<Configure>", lambda _e: self._paint_kit_waveform(force=True)
        )

        grid = tk.Frame(self._kit_frame, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        for r in range(4):
            grid.rowconfigure(r, weight=1)
        for c in range(4):
            grid.columnconfigure(c, weight=1)
        self._kit_btns = {}
        for i, cell in enumerate(PHRASE_GRID_CELLS):
            note = mpk_note_for_phrase_cell(cell)
            model = drum_model_for_note(note)
            label = f"{phrase_pad_label(cell)}\n{model}"
            r, c = divmod(i, 4)
            btn = self._mk_touch_btn(
                grid,
                label,
                lambda n=note: self._select_kit_note(n, audition=True),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 11, "bold"), pady=6)
            btn.grid(row=r, column=c, sticky="nsew", padx=2, pady=2)
            self._kit_btns[note] = btn

        footer = tk.Frame(self._kit_frame, bg="#111111")
        footer.pack(fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(
            footer, "AUDITION", lambda: self._select_kit_note(self._kit_selected_note, audition=True),
            bg="#458588",
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        self._mk_touch_btn(footer, "CLOSE", self._close_kit_explorer, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12
        )
        self._paint_kit_pad_btns()
        self._paint_kit_waveform(force=True)
        if self.engine.fx_mode():
            self._append_log(
                "KIT — pick a drum; FX MODE knobs edit that drum's insert"
            )
        else:
            self._append_log("KIT — pick a drum; DRUM MODE knobs reshape its wave")

    def _close_kit_explorer(self, restore_main: bool = True) -> None:
        if not self._kit_ui_open:
            return
        if self._kit_frame is not None:
            self._kit_frame.destroy()
            self._kit_frame = None
        self._kit_btns = {}
        self._kit_wave_canvas = None
        self._kit_ui_open = False
        # Leaving KIT while FX MODE is on → return knobs to nearer morph voice.
        if self.engine.fx_mode():
            self.engine.set_fx_edit_voice(None)
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

    def _build_looper_mode(self) -> None:
        shell = self._looper_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Looper", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="MIDI notes only · free timing",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            shell, textvariable=self._loop_status_var,
            font=("DejaVu Sans", 14, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(8, 12))

        # Giant transport buttons
        row1 = tk.Frame(shell, bg="#111111")
        row1.pack(fill=tk.BOTH, expand=True, padx=8, pady=4)
        self._loop_rec_btn = self._mk_touch_btn(
            row1, "RECORD", self._loop_toggle_record, bg="#9d0006"
        )
        self._loop_rec_btn.configure(font=("DejaVu Sans", 20, "bold"), pady=28)
        self._loop_rec_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        self._loop_play_btn = self._mk_touch_btn(
            row1, "PLAY", self._loop_toggle_play, bg="#689d6a"
        )
        self._loop_play_btn.configure(font=("DejaVu Sans", 20, "bold"), pady=28)
        self._loop_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        row2 = tk.Frame(shell, bg="#111111")
        row2.pack(fill=tk.X, padx=8, pady=(4, 10))
        self._mk_touch_btn(row2, "STOP", self._loop_stop, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=18
        )
        self._mk_touch_btn(row2, "CLEAR", self._loop_clear, bg="#3c3836").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=18
        )
        self._mk_touch_btn(row2, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=18
        )

        tip = tk.Label(
            shell,
            text="1) RECORD  2) play notes on the MPK  3) RECORD again to stop  4) PLAY loops it",
            font=("DejaVu Sans", 11), fg="#83a598", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        tip.pack(fill=tk.X, padx=10, pady=(0, 10))
        self._paint_looper_buttons()

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

        grid = tk.Frame(shell, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=6, pady=2)
        for row in range(4):
            grid.rowconfigure(row, weight=1)
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

        if play_view:
            row = tk.Frame(shell, bg="#111111")
            row.pack(fill=tk.X, padx=6, pady=(4, 6))
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
            row = tk.Frame(shell, bg="#111111")
            row.pack(fill=tk.X, padx=6, pady=(4, 2))
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

            detail = tk.Frame(shell, bg="#111111")
            detail.pack(fill=tk.X, padx=6, pady=(2, 6))
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
            self._phrase_out_btn = self._mk_touch_btn(
                detail, "OUT: LOCAL", self._phrase_cycle_out_mode, bg="#504945"
            )
            self._phrase_out_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
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
        self._phrase_status_var.set(
            self._phrases.status_line(
                clear_armed=clear_armed,
                mode_armed=mode_armed,
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
                v = "LOCK" if cell.is_voice_locked() else "FOLLOW"
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

    def _refresh_phrase_status(self) -> None:
        if self._mode == "pads":
            self._paint_phrase_pads()
        else:
            self._phrase_status_var.set(
                self._phrases.status_line(
                    clear_armed=self._phrase_clear_armed,
                    mode_armed=self._phrase_mode_armed,
                    view=self._pads_view,
                )
            )

    def _switch_mode(self, mode: str) -> None:
        mode = mode if mode in ("synth", "looper", "pads", "songs", "log", "presets") else "synth"
        # Close synth-only overlays before swapping shells
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)

        # Leaving pads while recording: keep the take
        if self._mode == "pads" and mode != "pads":
            if self._phrases.is_recording():
                self._phrases.stop_record()
            self._phrase_clear_armed = False
            self._phrase_mode_armed = False

        self._mode = mode
        self._synth_shell.pack_forget()
        self._looper_shell.pack_forget()
        self._pads_shell.pack_forget()
        self._songs_shell.pack_forget()
        self._presets_shell.pack_forget()
        self._log_shell.pack_forget()
        if self._grid_frame is not None:
            self._grid_frame.pack_forget()
        if self._morph_frame is not None:
            self._morph_frame.pack_forget()
        if self._kit_frame is not None:
            self._kit_frame.pack_forget()

        if mode == "looper":
            self._looper_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_loop_status()
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
        for key, btn in self._mode_btns.items():
            on = key == self._mode
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)

    def _refresh_loop_status(self) -> None:
        st = self._looper.status()
        n = int(st["events"])
        length = float(st["length"])
        if st["recording"]:
            msg = f"● RECORDING…  {n} events  (tap RECORD to stop)"
        elif st["playing"]:
            msg = f"▶ PLAYING loop  {length:.2f}s · {n} events  (tap PLAY to stop)"
        elif n == 0:
            msg = "Loop empty — tap RECORD, play notes, RECORD again, then PLAY."
        else:
            msg = f"Ready  {length:.2f}s · {n} events — tap PLAY to loop."
        self._loop_status_var.set(msg)
        self._paint_looper_buttons()

    def _paint_looper_buttons(self) -> None:
        if self._loop_rec_btn is not None:
            if self._looper.is_recording():
                self._loop_rec_btn.configure(
                    text="● STOP REC", bg="#cc241d", activebackground="#cc241d"
                )
            else:
                self._loop_rec_btn.configure(
                    text="RECORD", bg="#9d0006", activebackground="#9d0006"
                )
        if self._loop_play_btn is not None:
            if self._looper.is_playing():
                self._loop_play_btn.configure(
                    text="■ STOP", bg="#d79921", activebackground="#d79921"
                )
            else:
                self._loop_play_btn.configure(
                    text="PLAY", bg="#689d6a", activebackground="#689d6a"
                )

    def _loop_toggle_record(self) -> None:
        recording = self._looper.toggle_record()
        if recording:
            self._q_put(("log", "Loop RECORD start", False))
        else:
            st = self._looper.status()
            self._q_put(
                (
                    "log",
                    f"Loop RECORD stop — trimmed to {float(st['length']):.2f}s "
                    f"({int(st['events'])} events)",
                    False,
                )
            )
        self._q_put(("loop",))
        self._refresh_loop_status()

    def _loop_toggle_play(self) -> None:
        if self._looper.is_playing():
            self._looper.stop_playback()
            self._q_put(("log", "Loop PLAY stop", False))
        else:
            if not self._looper.start_playback():
                self._q_put(("log", "Loop empty — record something first", False))
            else:
                self._q_put(("log", "Loop PLAY start", False))
        self._q_put(("loop",))
        self._refresh_loop_status()

    def _loop_stop(self) -> None:
        if self._looper.is_recording():
            self._looper.stop_record()
        if self._looper.is_playing():
            self._looper.stop_playback()
        self._q_put(("log", "Loop STOP", False))
        self._q_put(("loop",))
        self._refresh_loop_status()

    def _loop_clear(self) -> None:
        self._looper.clear()
        self._q_put(("log", "Loop CLEAR", False))
        self._q_put(("loop",))
        self._refresh_loop_status()

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

    def _open_voice_grid(self) -> None:
        if self._grid_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        self._grid_open = True
        self._synth_shell.pack_forget()

        self._grid_frame = tk.Frame(self._mode_host, bg="#111111")
        self._grid_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._grid_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 4))
        tk.Label(
            header,
            text="VOICES — tap one",
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

        # Scrollable canvas so many wavetables still fit on a 5" panel
        body = tk.Frame(self._grid_frame, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        canvas = tk.Canvas(body, bg="#111111", highlightthickness=0, bd=0)
        scroll = ttk.Scrollbar(body, orient="vertical", command=canvas.yview)
        canvas.configure(yscrollcommand=scroll.set)
        scroll.pack(side=tk.RIGHT, fill=tk.Y)
        canvas.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)

        inner = tk.Frame(canvas, bg="#111111")
        window_id = canvas.create_window((0, 0), window=inner, anchor="nw")

        def _on_inner_configure(_event: object = None) -> None:
            canvas.configure(scrollregion=canvas.bbox("all"))

        def _on_canvas_configure(event: tk.Event) -> None:  # type: ignore[name-defined]
            canvas.itemconfigure(window_id, width=event.width)

        inner.bind("<Configure>", _on_inner_configure)
        canvas.bind("<Configure>", _on_canvas_configure)

        # Drag-to-scroll helps when the resistive panel has no wheel
        self._grid_drag_y = 0

        def _drag_start(event: tk.Event) -> None:  # type: ignore[name-defined]
            self._grid_drag_y = event.y
            canvas.scan_mark(event.x, event.y)

        def _drag_move(event: tk.Event) -> None:  # type: ignore[name-defined]
            canvas.scan_dragto(event.x, event.y, gain=1)

        canvas.bind("<ButtonPress-1>", _drag_start)
        canvas.bind("<B1-Motion>", _drag_move)

        cols = 4 if len(self._voice_names) > 8 else 3
        self._grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_touch_btn(
                inner,
                name.upper(),
                lambda idx=i: self._select_voice_index(idx, close_grid=True),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=18)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=4, ipady=8)
            self._grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        footer = tk.Frame(self._grid_frame, bg="#111111")
        footer.pack(fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(footer, "CLOSE", self._close_voice_grid, bg="#9d0006").pack(
            fill=tk.BOTH, ipady=14
        )
        self._paint_voice_grid()

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
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
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
        self._synth_shell.pack_forget()

        self._morph_frame = tk.Frame(self._mode_host, bg="#111111")
        self._morph_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._morph_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="MORPH PAIR",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._morph_status_lbl = tk.Label(
            header,
            text="tap A or B, then a voice",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        )
        self._morph_status_lbl.pack(side=tk.RIGHT)

        # A / B selector row
        pair_row = tk.Frame(self._morph_frame, bg="#111111")
        pair_row.pack(fill=tk.X, padx=6, pady=(4, 6))
        self._morph_side_btns = {}
        for side, label in (("a", "A"), ("b", "B")):
            btn = self._mk_touch_btn(
                pair_row,
                f"{label}: …",
                lambda s=side: self._set_morph_pick_side(s),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 14, "bold"), pady=14)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3)
            self._morph_side_btns[side] = btn

        swap_btn = self._mk_touch_btn(pair_row, "SWAP", self._swap_morph_pair, bg="#504945")
        swap_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
        swap_btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=3)

        # Voice grid for assigning the armed side
        body = tk.Frame(self._morph_frame, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        canvas = tk.Canvas(body, bg="#111111", highlightthickness=0, bd=0)
        scroll = ttk.Scrollbar(body, orient="vertical", command=canvas.yview)
        canvas.configure(yscrollcommand=scroll.set)
        scroll.pack(side=tk.RIGHT, fill=tk.Y)
        canvas.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)

        inner = tk.Frame(canvas, bg="#111111")
        window_id = canvas.create_window((0, 0), window=inner, anchor="nw")

        def _on_inner_configure(_event: object = None) -> None:
            canvas.configure(scrollregion=canvas.bbox("all"))

        def _on_canvas_configure(event: tk.Event) -> None:  # type: ignore[name-defined]
            canvas.itemconfigure(window_id, width=event.width)

        inner.bind("<Configure>", _on_inner_configure)
        canvas.bind("<Configure>", _on_canvas_configure)
        canvas.bind("<ButtonPress-1>", lambda e: canvas.scan_mark(e.x, e.y))
        canvas.bind("<B1-Motion>", lambda e: canvas.scan_dragto(e.x, e.y, gain=1))

        cols = 4 if len(self._voice_names) > 8 else 3
        self._morph_grid_btns = {}
        for i, name in enumerate(self._voice_names):
            r, c = divmod(i, cols)
            btn = self._mk_touch_btn(
                inner,
                name.upper(),
                lambda idx=i: self._assign_morph_endpoint(idx),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=14)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipadx=2, ipady=6)
            self._morph_grid_btns[name] = btn
        for c in range(cols):
            inner.grid_columnconfigure(c, weight=1)

        footer = tk.Frame(self._morph_frame, bg="#111111")
        footer.pack(fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(footer, "DONE", self._close_morph_menu, bg="#689d6a").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
        )
        self._mk_touch_btn(footer, "CANCEL", self._close_morph_menu, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
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

    def _pick_port(self) -> Optional[str]:
        names = mido.get_input_names()
        if not names:
            return None
        if self.port_filter:
            for n in names:
                if self.port_filter in n.lower():
                    return n
            print(f"No input matching '{self.port_filter}'. Available:", flush=True)
            for n in names:
                print(f"  {n}", flush=True)
            print(f"Falling back to: {names[0]}", flush=True)
            return names[0]
        for n in names:
            if "mpk" in n.lower():
                return n
        return names[0]

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
                self.engine.set_level(value)
                self._mark_settings_dirty()
                return f"Level  {value}"
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
                self.engine.set_level(value)
                self._mark_settings_dirty()
                return f"Level  {value}"
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
            self.engine.set_level(value)
            self._mark_settings_dirty()
            return f"Level  {value}"
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
            # PADS mode: MPK pads launch/arm phrases — unless recording (then drums)
            # or CLEAR/MODE is armed.
            if pads_mode and is_drum and not phrase_recording:
                cell = phrase_cell_for_note(msg.note)
                if cell is not None:
                    edit_view = self._pads_view == "edit"
                    if edit_view and self._phrase_mode_armed:
                        mode = self._phrases.toggle_trigger_mode(cell)
                        self._phrase_mode_armed = False
                        self._q_put(("phrase",))
                        self._q_put(
                            (
                                "log",
                                f"Pad→MODE {phrase_pad_label(cell)} → "
                                f"{'LOOP' if mode == PHRASE_TRIG_LOOP else 'ONE-SHOT'}",
                                False,
                            )
                        )
                        return
                    if edit_view and self._phrase_clear_armed:
                        self._phrases.clear_cell(cell)
                        self._phrase_clear_armed = False
                        self._q_put(("phrase",))
                        self._q_put(
                            (
                                "log",
                                f"Pad→CLEAR {phrase_pad_label(cell)}  note {msg.note}",
                                False,
                            )
                        )
                        return
                    action = self._phrases.handle_pad(
                        cell, from_touch=False, allow_record=edit_view
                    )
                    self._q_put(("phrase",))
                    self._q_put(
                        (
                            "log",
                            f"Pad→Phrase {phrase_pad_label(cell)} ({action})  "
                            f"note {msg.note}  vel {msg.velocity}",
                            False,
                        )
                    )
                    return
            # KIT explorer: hitting an MPK pad selects that voice for the scope
            if self._kit_ui_open and is_drum and phrase_cell_for_note(msg.note) is not None:
                self._q_put(("kit_sel", msg.note))
            vel = msg.velocity if is_drum or not self._full_vel else 127
            self.engine.note_on(msg.channel, msg.note, vel)
            self._looper.record_note(True, msg.channel, msg.note, vel)
            if pads_mode or phrase_recording:
                self._phrases.record_note(True, msg.channel, msg.note, vel)
            self._q_put(("on", msg.channel, msg.note, vel))
            if self._looper.is_recording():
                self._q_put(("loop",))
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
            if (
                pads_mode
                and msg.channel == DRUM_CHANNEL
                and not self._phrases.is_recording()
            ):
                if phrase_cell_for_note(msg.note) is not None:
                    return
            self.engine.note_off(msg.channel, msg.note)
            self._looper.record_note(False, msg.channel, msg.note, 0)
            if pads_mode or self._phrases.is_recording():
                self._phrases.record_note(False, msg.channel, msg.note, 0)
            self._q_put(("off", msg.channel, msg.note))
            if self._looper.is_recording():
                self._q_put(("loop",))
            self._q_put(("log", format_message(msg), False))
        elif msg.type == "polytouch":
            if (
                pads_mode
                and msg.channel == DRUM_CHANNEL
                and not self._phrases.is_recording()
            ):
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
                elif kind == "loop":
                    self._refresh_loop_status()
                elif kind == "phrase":
                    self._refresh_phrase_status()
                elif kind == "song":
                    self._refresh_song_status()
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
            if self._kit_ui_open:
                self._paint_kit_waveform()
            elif self._mode == "synth" and not self._overlay_busy():
                self._paint_synth_waveform()
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
        if self._looper.is_playing():
            self._looper.stop_playback()
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        if self._songs.is_playing():
            self._songs.stop()
        self.engine.all_notes_off()
        self._active_notes.clear()
        self._refresh_active()
        self._refresh_loop_status()
        self._refresh_phrase_status()
        self._refresh_song_status()
        self._append_log("All Notes Off")

    def _on_close(self) -> None:
        self._stop.set()
        try:
            self._looper.stop_playback()
            self._looper.stop_record()
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
        app.run()
        print("midi-tone: mainloop exited", flush=True)


if __name__ == "__main__":
    main()
