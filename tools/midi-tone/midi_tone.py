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
SETTINGS_VERSION = 1

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


def _builtin_tables() -> Dict[str, np.ndarray]:
    t = np.linspace(0.0, 1.0, TABLE_SIZE, endpoint=False, dtype=np.float64)
    sine = np.sin(2.0 * np.pi * t).astype(np.float32)
    square = np.where(t < 0.5, 0.35, -0.35).astype(np.float32)
    saw = (2.0 * (t - np.floor(t + 0.5))).astype(np.float32) * 0.35
    triangle = (2.0 * np.abs(2.0 * (t - np.floor(t + 0.5))) - 1.0).astype(np.float32) * 0.35
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
    return (y / peak) * np.float32(0.35)


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


@dataclass
class Voice:
    note: int
    velocity: float
    phase: float = 0.0  # 0 .. TABLE_SIZE
    releasing: bool = False
    amp: float = 0.0
    target_amp: float = 0.0
    age: int = 0  # bump on each note_on for steal ordering


class SineEngine:
    """Wavetable synth — light enough for Pi 2."""

    ATTACK_SEC_MIN = 0.002
    ATTACK_SEC_MAX = 0.400
    RELEASE_SEC_MIN = 0.010
    RELEASE_SEC_MAX = 0.800

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

    def note_on(self, channel: int, note: int, velocity: int) -> None:
        if velocity <= 0:
            self.note_off(channel, note)
            return
        key = (channel & 0x0F, note & 0x7F)
        vel = velocity / 127.0
        # Drums a bit hotter so soft hits still speak; keys stay as before
        scale = 0.16 if (channel & 0x0F) == DRUM_CHANNEL else 0.12
        target = vel * scale
        with self._lock:
            self._note_serial += 1
            serial = self._note_serial
            if (channel & 0x0F) == DRUM_CHANNEL:
                self._drum_gate[key[1]] = True
            existing = self._voices.get(key)
            if existing is not None and (channel & 0x0F) == DRUM_CHANNEL:
                # Same pad again: glide volume to new velocity (no hard restart click)
                existing.releasing = False
                existing.velocity = vel
                existing.target_amp = target
                existing.age = serial
                return
            if existing is not None:
                # Same key re-trigger: reuse slot, restart envelope/phase
                existing.note = note & 0x7F
                existing.velocity = vel
                existing.phase = 0.0
                existing.releasing = False
                existing.amp = 0.0
                existing.target_amp = target
                existing.age = serial
                return
            if len(self._voices) >= self.max_voices:
                drop = self._steal_key()
                if drop is not None:
                    del self._voices[drop]
            self._voices[key] = Voice(
                note=note & 0x7F,
                velocity=vel,
                amp=0.0,
                target_amp=target,
                releasing=False,
                age=serial,
            )

    def note_off(self, channel: int, note: int) -> None:
        key = (channel & 0x0F, note & 0x7F)
        with self._lock:
            if (channel & 0x0F) == DRUM_CHANNEL:
                self._drum_gate[key[1]] = False
            v = self._voices.get(key)
            if v is None:
                return
            v.releasing = True
            v.target_amp = 0.0

    def all_notes_off(self) -> None:
        with self._lock:
            self._drum_gate.clear()
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
        if value > 1.0:
            value = value / 127.0
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
        """Live volume for held drum pads (aftertouch / pressure)."""
        if (channel & 0x0F) != DRUM_CHANNEL:
            return
        vel = max(0, min(127, value)) / 127.0
        target = vel * 0.16
        with self._lock:
            if note is None:
                keys = [
                    (DRUM_CHANNEL, n)
                    for n, held in self._drum_gate.items()
                    if held
                ]
                for (ch, n), v in self._voices.items():
                    if ch == DRUM_CHANNEL and self._drum_gate.get(n, False):
                        if (ch, n) not in keys:
                            keys.append((ch, n))
            else:
                keys = [(DRUM_CHANNEL, note & 0x7F)]

            for key in keys:
                n = key[1]
                v = self._voices.get(key)
                if value <= 0:
                    self._drum_gate[n] = False
                    if v is not None:
                        v.releasing = True
                        v.target_amp = 0.0
                    continue
                if not self._drum_gate.get(n, False):
                    continue
                if v is None or v.releasing:
                    continue
                v.velocity = vel
                v.target_amp = target

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

    def modulation_state(self) -> Dict[str, float]:
        with self._lock:
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
            }

    def snapshot_settings(self) -> Dict[str, Any]:
        """Serialize synth sound settings for JSON presets / session restore."""
        with self._lock:
            return {
                "morph_a": self._voice_names[self._morph_a],
                "morph_b": self._voice_names[self._morph_b],
                "morph": float(self._morph),
                "tone": float(self._tone),
                "level": float(self._level),
                "attack_sec": float(self._attack_sec),
                "release_sec": float(self._release_sec),
                "vib_hz": float(self._vib_hz),
                "vib_depth": float(self._vib_depth_semis),
            }

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
            bend = self._bend_semitones
            mod = self._mod
            vib_hz = self._vib_hz
            vib_depth = self._vib_depth_semis
            table = self._morph_table
            tone = self._tone
            level = self._level
            attack_sec = self._attack_sec
            release_sec = self._release_sec

        attack_per_samp = 1.0 / max(1.0, attack_sec * sr)
        release_per_samp = 1.0 / max(1.0, release_sec * sr)

        vib_semis = 0.0
        if mod > 0.01 and vib_depth > 0.001:
            self._vib_phase += 2.0 * math.pi * vib_hz * (frames / sr)
            if self._vib_phase > 2.0 * math.pi:
                self._vib_phase %= 2.0 * math.pi
            vib_semis = vib_depth * mod * math.sin(self._vib_phase)

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
            wave = table[i0] * (1.0 - frac) + table[i1] * frac

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
            buf += wave * ramp
            v.amp = float(end_amp)
            v.phase = float((v.phase + phase_inc * frames) % TABLE_SIZE)
            if v.releasing and v.amp < 0.0005:
                dead.append(key)

        # Cheap bus low-pass for "tone" knob (moving average). tone=1 → open.
        if tone < 0.999 and frames > 0:
            win = max(1, int(round((1.0 - tone) * 48.0)))
            if win > 1:
                kernel = np.ones(win, dtype=np.float32) / np.float32(win)
                # Continue from last filter state so block edges don't click
                pad_left = np.full(win - 1, self._filter_state, dtype=np.float32)
                padded = np.concatenate([pad_left, buf])
                filtered = np.convolve(padded, kernel, mode="valid")
                buf[:] = filtered[:frames]
                self._filter_state = float(buf[-1])
            else:
                self._filter_state = float(buf[-1])
        elif frames > 0:
            self._filter_state = float(buf[-1])

        if level < 0.999:
            buf *= np.float32(level)
        np.clip(buf, -0.95, 0.95, out=buf)
        outdata[:, 0] = buf
        if dead:
            with self._lock:
                for k in dead:
                    self._voices.pop(k, None)


@dataclass
class LoopEvent:
    t: float  # seconds from record start
    on: bool
    channel: int
    note: int
    velocity: int


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
            elapsed = time.monotonic() - self._rec_t0
            if self._events:
                last = max(e.t for e in self._events)
                self._loop_len = max(elapsed, last + 0.05)
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
        self._voice_lbl: Optional[tk.Label] = None
        self._grid_open = False
        self._grid_frame: Optional[tk.Frame] = None
        self._grid_btns: Dict[str, tk.Button] = {}
        self._morph_ui_open = False
        self._morph_frame: Optional[tk.Frame] = None
        self._morph_pick_side = "a"  # which endpoint the next grid tap sets
        self._morph_side_btns: Dict[str, tk.Button] = {}
        self._morph_grid_btns: Dict[str, tk.Button] = {}
        self._morph_status_lbl: Optional[tk.Label] = None
        self._mode = "synth"  # synth | looper | log | presets
        self._mode_btns: Dict[str, tk.Button] = {}
        self._looper = MidiLooper(self.engine, self._q_put)
        self._loop_status_var = tk.StringVar(value="Loop empty — tap RECORD, play notes, STOP, then PLAY.")
        self._loop_rec_btn: Optional[tk.Button] = None
        self._loop_play_btn: Optional[tk.Button] = None
        self._preset_status_var = tk.StringVar(value="Tap a slot, then LOAD or SAVE.")
        self._preset_slot = 0
        self._preset_slot_btns: Dict[int, tk.Button] = {}
        self._active_preset_name: Optional[str] = None
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
            ("presets", "PRESETS"),
            ("log", "LOG"),
        ):
            btn = self._mk_touch_btn(
                nav_modes, label, lambda m=key: self._switch_mode(m), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=8, padx=10)
            btn.pack(side=tk.LEFT, padx=2)
            self._mode_btns[key] = btn

        # Mode content host
        self._mode_host = tk.Frame(self.root, bg="#111111")
        self._mode_host.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        self._synth_shell = tk.Frame(self._mode_host, bg="#111111")
        self._looper_shell = tk.Frame(self._mode_host, bg="#111111")
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
        self._full_vel_btn = self._mk_touch_btn(
            row3, "FULL VEL: ON", self._toggle_full_vel, bg="#689d6a"
        )
        self._full_vel_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

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
        mod_lbl.pack(fill=tk.X, padx=8, pady=(2, 8))

        # Spacer so synth isn't empty under the status lines
        tk.Label(
            self._main,
            text="Use VOICES / MORPH below · open LOG mode for the full event history",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, padx=8, pady=(12, 0))

        self._active_notes: Dict[Tuple[int, int], int] = {}
        # Select first voice explicitly
        self.engine.set_waveform(self._voice_names[self._voice_index])
        self.root.after(40, self._drain_queue)
        # Default morph pair: first voice ↔ second (or same if only one)
        if len(self._voice_names) > 1:
            self.engine.set_morph_pair(0, 1, morph=0.0)

        self._build_looper_mode()
        self._build_presets_mode()
        self._build_log_mode()
        self._switch_mode("synth")

        # Restore last session (full vel, morph pair, knob-shaped tone, etc.)
        restored = self._load_settings_file(SETTINGS_PATH)
        self._paint_full_vel_btn()
        self._sync_voice_index_from_morph()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())

        self._append_log(f"Listening on: {port_name}")
        self._append_log(f"Loaded {len(self._voice_names)} voices — VOICES grid / MORPH pair.")
        self._append_log(
            "MPK knobs (CC70–77): morph / tone / attack / release / "
            "vib depth / vib rate / — / level"
        )
        self._append_log("Modes: SYNTH / LOOPER / PRESETS / LOG (top right).")
        if restored:
            self._append_log(f"Restored session from {SETTINGS_PATH.name}")
        else:
            self._append_log("No settings.json yet — changes will autosave.")
        self._append_log("If knobs do nothing: Prog Select + Pad 1 (MPC program).")
        print("ui: construction complete", flush=True)
        PRESETS_DIR.mkdir(parents=True, exist_ok=True)
        self.root.after(2000, self._autosave_tick)

    def _voice_label_text(self) -> str:
        left, right, blend = self.engine.morph_neighbors()
        if left == right:
            return f"{self._voice_index + 1}/{len(self._voice_names)}  {left.upper()}"
        pct = int(round(blend * 100))
        return f"{left.upper()} → {right.upper()}  {pct}%"

    def _format_mod_line(self) -> str:
        st = self.engine.modulation_state()
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
        return self._grid_open or self._morph_ui_open

    def _session_dict(self) -> Dict[str, Any]:
        return {
            "version": SETTINGS_VERSION,
            "full_velocity": bool(self._full_vel),
            "active_preset": self._active_preset_name,
            "synth": self.engine.snapshot_settings(),
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

    def _switch_mode(self, mode: str) -> None:
        mode = mode if mode in ("synth", "looper", "log", "presets") else "synth"
        # Close synth-only overlays before swapping shells
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)

        self._mode = mode
        self._synth_shell.pack_forget()
        self._looper_shell.pack_forget()
        self._presets_shell.pack_forget()
        self._log_shell.pack_forget()
        if self._grid_frame is not None:
            self._grid_frame.pack_forget()
        if self._morph_frame is not None:
            self._morph_frame.pack_forget()

        if mode == "looper":
            self._looper_shell.pack(fill=tk.BOTH, expand=True)
            self._refresh_loop_status()
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
        self._q_put(("log", "Loop RECORD start" if recording else "Loop RECORD stop", False))
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

    def _open_morph_menu(self) -> None:
        """Pick morph endpoints A and B; Knob 1 blends A→B."""
        if self._morph_ui_open:
            return
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)

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

    def _handle_knob_cc(self, control: int, value: int) -> Optional[str]:
        """Map MPK factory knobs. Returns a short UI label or None if unmapped."""
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

        if msg.type == "note_on" and msg.velocity > 0:
            is_drum = msg.channel == DRUM_CHANNEL
            vel = msg.velocity if is_drum or not self._full_vel else 127
            self.engine.note_on(msg.channel, msg.note, vel)
            self._looper.record_note(True, msg.channel, msg.note, vel)
            self._q_put(("on", msg.channel, msg.note, vel))
            if self._looper.is_recording():
                self._q_put(("loop",))
            if is_drum:
                line = (
                    f"Pad      ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}"
                )
            elif self._full_vel and msg.velocity != 127:
                line = (
                    f"Note On   ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  vel {msg.velocity}→127"
                )
            else:
                line = format_message(msg)
            self._q_put(("log", line, False))
        elif msg.type == "note_off" or (msg.type == "note_on" and msg.velocity == 0):
            self.engine.note_off(msg.channel, msg.note)
            self._looper.record_note(False, msg.channel, msg.note, 0)
            self._q_put(("off", msg.channel, msg.note))
            if self._looper.is_recording():
                self._q_put(("loop",))
            self._q_put(("log", format_message(msg), False))
        elif msg.type == "polytouch":
            self.engine.set_pad_pressure(msg.channel, msg.note, msg.value)
            if msg.channel == DRUM_CHANNEL:
                self._put_continuous_log(
                    f"PadPress ch{msg.channel + 1}  {midi_note_name(msg.note)} "
                    f"({msg.note})  press {msg.value}"
                )
            else:
                self._put_continuous_log(format_message(msg))
        elif msg.type == "aftertouch":
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
                label = self._handle_knob_cc(msg.control, msg.value)
                self._q_put(("mod",))
                if msg.control == CC_MORPH:
                    self._q_put(("morph",))
                if label:
                    self._put_continuous_log(label)
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
                    self.mod_var.set(self._format_mod_line())
                elif kind == "morph":
                    self._sync_voice_index_from_morph()
                    self.mod_var.set(self._format_mod_line())
                elif kind == "panic":
                    self._active_notes.clear()
                    self._refresh_active()
                elif kind == "loop":
                    self._refresh_loop_status()
        except queue.Empty:
            pass
        pending = getattr(self, "_pending_cont_log", None)
        if pending is not None:
            self.last_var.set(pending)
            self._pending_cont_log = None
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
        self.engine.all_notes_off()
        self._active_notes.clear()
        self._refresh_active()
        self._refresh_loop_status()
        self._append_log("All Notes Off")

    def _on_close(self) -> None:
        self._stop.set()
        try:
            self._looper.stop_playback()
            self._looper.stop_record()
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
