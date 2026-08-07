#!/usr/bin/env python3
"""
midi-tone — Phase 0 diagnostic turned tiny DIY soft-synth.

MPK (or any MIDI in) → wavetable soft-synth + event UI.
Keep lean for Raspberry Pi 2 (wavetable synth, capped polyphony).
"""

from __future__ import annotations

import argparse
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
from typing import Dict, List, Optional, Tuple

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

    ATTACK_SEC = 0.012
    RELEASE_SEC = 0.030

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
        self._waveform = self._voice_names[0]
        self._table = tables[self._waveform]
        self._scratch = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._phase_buf = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._frac_buf = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._arange = np.arange(BLOCKSIZE * 2, dtype=np.float32)
        self._ramp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._note_serial = 0
        # Drum pad gate: aftertouch may only change volume while the pad note is held.
        self._drum_gate: Dict[int, bool] = {}

    @property
    def voice_names(self) -> List[str]:
        return list(self._voice_names)

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
        table = self._tables.get(name)
        if table is None:
            return False
        with self._lock:
            self._waveform = name
            self._table = table
        return True

    def waveform(self) -> str:
        with self._lock:
            return self._waveform

    def modulation_state(self) -> Tuple[float, float]:
        with self._lock:
            return self._bend_semitones, self._mod

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
        attack_per_samp = 1.0 / max(1.0, self.ATTACK_SEC * sr)
        release_per_samp = 1.0 / max(1.0, self.RELEASE_SEC * sr)

        with self._lock:
            items = list(self._voices.items())
            bend = self._bend_semitones
            mod = self._mod
            table = self._table

        vib_semis = 0.0
        if mod > 0.01:
            self._vib_phase += 2.0 * math.pi * self._vib_hz * (frames / sr)
            if self._vib_phase > 2.0 * math.pi:
                self._vib_phase %= 2.0 * math.pi
            vib_semis = self._vib_depth_semis * mod * math.sin(self._vib_phase)

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

        np.clip(buf, -0.95, 0.95, out=buf)
        outdata[:, 0] = buf
        if dead:
            with self._lock:
                for k in dead:
                    self._voices.pop(k, None)


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
        self._log_expanded = False

        # Bottom touch bar packed first so it never gets crushed / lost
        self._touch = tk.Frame(self.root, bg="#111111")
        self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)

        row1 = tk.Frame(self._touch, bg="#111111")
        row1.pack(fill=tk.X, pady=(0, 6))
        self._mk_touch_btn(row1, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )
        self._mk_touch_btn(row1, "CLEAR LOG", self._clear_log, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )
        self._mk_touch_btn(row1, "EXPAND LOG", self._toggle_log_fullscreen, bg="#3c3836").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )

        # Voice picker — prev / name / next (fits many wavetables on a small panel)
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
        # Tap the name to jump forward too (big hit target)
        self._voice_lbl.bind("<ButtonPress-1>", lambda _e: self._next_voice())
        self._mk_touch_btn(row2, "NEXT ▶", self._next_voice, bg="#3c3836").pack(
            side=tk.LEFT, fill=tk.BOTH, padx=3, ipady=10
        )

        row3 = tk.Frame(self._touch, bg="#111111")
        row3.pack(fill=tk.X)
        self._full_vel_btn = self._mk_touch_btn(
            row3, "FULL VELOCITY: ON", self._toggle_full_vel, bg="#689d6a"
        )
        self._full_vel_btn.pack(fill=tk.BOTH, ipady=8)

        self._main = tk.Frame(self.root, bg="#111111")
        self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)

        header = tk.Frame(self._main, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="midi-tone", font=("DejaVu Sans", 18, "bold"),
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

        self.mod_var = tk.StringVar(value="Bend: 0.00 st   CC1 (vib): 0")
        mod_lbl = tk.Label(
            self._main, textvariable=self.mod_var,
            font=("DejaVu Sans Mono", 12), fg="#d3869b", bg="#111111", anchor="w",
        )
        mod_lbl.pack(fill=tk.X, padx=8, pady=(2, 0))

        self._log_title = tk.Label(
            self._main, text="Event log", font=("DejaVu Sans", 10),
            fg="#a89984", bg="#111111", anchor="w",
        )
        self._log_title.pack(fill=tk.X, padx=8, pady=(6, 2))

        self._log_frame = tk.Frame(self._main, bg="#111111")
        self._log_frame.pack(fill=tk.BOTH, expand=True, padx=8, pady=(0, 4))
        self.log = tk.Text(
            self._log_frame, height=6, font=("DejaVu Sans Mono", 11),
            bg="#1d2021", fg="#ebdbb2", insertbackground="#ebdbb2",
            relief=tk.FLAT, state=tk.DISABLED,
        )
        scroll = ttk.Scrollbar(self._log_frame, command=self.log.yview)
        self.log.configure(yscrollcommand=scroll.set)
        self.log.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        scroll.pack(side=tk.RIGHT, fill=tk.Y)

        self._log_chrome = [
            (header, dict(fill=tk.X, padx=8, pady=(8, 2))),
            (last_lbl, dict(fill=tk.X, padx=8, pady=4)),
            (active_lbl, dict(fill=tk.X, padx=8)),
            (mod_lbl, dict(fill=tk.X, padx=8, pady=(2, 0))),
            (self._log_title, dict(fill=tk.X, padx=8, pady=(6, 2))),
        ]
        self._exit_log_btn: Optional[tk.Button] = None

        self._active_notes: Dict[Tuple[int, int], int] = {}
        # Select first voice explicitly
        self.engine.set_waveform(self._voice_names[self._voice_index])
        self.root.after(40, self._drain_queue)
        self._append_log(f"Listening on: {port_name}")
        self._append_log(f"Loaded {len(self._voice_names)} voices (PREV/NEXT).")
        self._append_log("Ch10 pads: velocity + aftertouch → volume.")
        self._append_log("Use EXPAND LOG — double-tap disabled (touch bounce).")
        print("ui: construction complete", flush=True)

    def _voice_label_text(self) -> str:
        name = self._voice_names[self._voice_index]
        return f"{self._voice_index + 1}/{len(self._voice_names)}  {name.upper()}"

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

    def _select_voice_index(self, idx: int) -> None:
        if not self._voice_names:
            return
        self._voice_index = idx % len(self._voice_names)
        name = self._voice_names[self._voice_index]
        self.engine.set_waveform(name)
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.last_var.set(f"Voice → {name.upper()}")
        self._q_put(("log", f"Voice → {name}", False))

    def _prev_voice(self) -> None:
        self._select_voice_index(self._voice_index - 1)

    def _next_voice(self) -> None:
        self._select_voice_index(self._voice_index + 1)

    def _toggle_log_fullscreen(self) -> None:
        now = time.monotonic()
        if now - getattr(self, "_last_log_toggle", 0.0) < 0.35:
            return
        self._last_log_toggle = now

        if not self._log_expanded:
            for w, _opts in self._log_chrome:
                w.pack_forget()
            self._touch.pack_forget()
            self._log_frame.pack_configure(padx=4, pady=(4, 0))
            self.log.configure(font=("DejaVu Sans Mono", 14))
            self._exit_log_btn = self._mk_touch_btn(
                self.root, "EXIT FULLSCREEN LOG", self._toggle_log_fullscreen, bg="#9d0006"
            )
            self._exit_log_btn.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6, ipady=14)
            self._log_expanded = True
            self.last_var.set("Log fullscreen — tap EXIT to leave")
        else:
            if self._exit_log_btn is not None:
                self._exit_log_btn.destroy()
                self._exit_log_btn = None
            self._log_frame.pack_forget()
            for w, opts in self._log_chrome:
                w.pack(**opts)
            self._log_frame.pack(fill=tk.BOTH, expand=True, padx=8, pady=(0, 4))
            # Always restore the touch bar last so it stays visible
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            self.log.configure(font=("DejaVu Sans Mono", 11))
            self._log_expanded = False
            self.log.see(tk.END)

    def _toggle_full_vel(self) -> None:
        self._full_vel = not self._full_vel
        if self._full_vel_btn is not None:
            if self._full_vel:
                self._full_vel_btn.configure(
                    text="FULL VELOCITY: ON", bg="#689d6a", activebackground="#689d6a"
                )
            else:
                self._full_vel_btn.configure(
                    text="FULL VELOCITY: OFF", bg="#3c3836", activebackground="#3c3836"
                )
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

    def _handle_midi(self, msg: mido.Message) -> None:
        continuous = msg.type == "pitchwheel" or (
            msg.type == "control_change" and msg.control == 1
        )

        if msg.type == "note_on" and msg.velocity > 0:
            is_drum = msg.channel == DRUM_CHANNEL
            vel = msg.velocity if is_drum or not self._full_vel else 127
            self.engine.note_on(msg.channel, msg.note, vel)
            self._q_put(("on", msg.channel, msg.note, vel))
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
            self._q_put(("off", msg.channel, msg.note))
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
                    bend, mod = self.engine.modulation_state()
                    self.mod_var.set(
                        f"Bend: {bend:+.2f} st   CC1 (vib): {int(mod * 127)}"
                    )
                elif kind == "panic":
                    self._active_notes.clear()
                    self._refresh_active()
        except queue.Empty:
            pass
        pending = getattr(self, "_pending_cont_log", None)
        if pending is not None:
            self.last_var.set(pending)
            self._pending_cont_log = None
        # Keep touch bar stacked above log chrome if packing ever races
        if not self._log_expanded:
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
        self.engine.all_notes_off()
        self._active_notes.clear()
        self._refresh_active()
        self._append_log("All Notes Off")

    def _on_close(self) -> None:
        self._stop.set()
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
    )
    if not args.list:
        print("midi-tone: entering mainloop", flush=True)
        app.run()
        print("midi-tone: mainloop exited", flush=True)


if __name__ == "__main__":
    main()
