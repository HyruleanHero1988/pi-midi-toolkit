#!/usr/bin/env python3
"""
midi-tone — Phase 0 diagnostic: MPK (or any MIDI in) → sine soft-synth + event UI.

Proves USB MIDI host path on the Pi without USB-DIN / hardware synth.
Keep lean for Raspberry Pi 2 (wavetable synth, capped polyphony).
"""

from __future__ import annotations

import argparse
import math
import queue
import sys
import threading
import time
import tkinter as tk
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
MAX_VOICES = 4
TABLE_SIZE = 2048
TABLE_MASK = TABLE_SIZE - 1
LOG_MAX = 80
# MIDI channel 10 (1-based) = index 9 — MPK drum pads
DRUM_CHANNEL = 9
NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

_t = np.linspace(0.0, 1.0, TABLE_SIZE, endpoint=False, dtype=np.float64)
SINE_TABLE = np.sin(2.0 * np.pi * _t).astype(np.float32)
# Band-limited-ish cheap shapes (single-cycle); fine for diagnostic tones
SQUARE_TABLE = np.where(_t < 0.5, 0.35, -0.35).astype(np.float32)
SAW_TABLE = (2.0 * (_t - np.floor(_t + 0.5))).astype(np.float32) * 0.35
WAVETABLES = {
    "sine": SINE_TABLE,
    "square": SQUARE_TABLE,
    "saw": SAW_TABLE,
}


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


class SineEngine:
    """Wavetable synth (sine / square / saw) — light enough for Pi 2."""

    ATTACK_SEC = 0.015
    RELEASE_SEC = 0.040

    def __init__(self, sample_rate: int = SAMPLE_RATE) -> None:
        self.sample_rate = sample_rate
        self._lock = threading.Lock()
        self._voices: Dict[Tuple[int, int], Voice] = {}
        self._stream: Optional[sd.OutputStream] = None
        self._bend_semitones = 0.0
        self._bend_range = 2.0
        self._mod = 0.0
        self._vib_hz = 5.0
        self._vib_depth_semis = 0.5
        self._vib_phase = 0.0
        self._waveform = "sine"
        self._table = WAVETABLES["sine"]
        self._scratch = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._phase_buf = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._arange = np.arange(BLOCKSIZE * 2, dtype=np.float32)
        self._ramp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        # Drum pad gate: aftertouch may only change volume while the pad note is held.
        # Never revive a pad after note-off (sticky aftertouch was leaving notes hung).
        self._drum_gate: Dict[int, bool] = {}

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
            f"latency={LATENCY_SEC}s voices<={MAX_VOICES} "
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
            if (channel & 0x0F) == DRUM_CHANNEL:
                self._drum_gate[key[1]] = True
            existing = self._voices.get(key)
            if existing is not None and (channel & 0x0F) == DRUM_CHANNEL:
                # Same pad again: glide volume to new velocity (no hard restart click)
                existing.releasing = False
                existing.velocity = vel
                existing.target_amp = target
                return
            if key not in self._voices and len(self._voices) >= MAX_VOICES:
                drop = next(iter(self._voices))
                del self._voices[drop]
            self._voices[key] = Voice(
                note=note & 0x7F,
                velocity=vel,
                amp=0.0,
                target_amp=target,
                releasing=False,
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
        """Live volume for held drum pads (aftertouch / pressure).

        Only affects pads whose note is still gated on. After note-off, pressure is
        ignored so sticky aftertouch cannot keep a pad singing forever.
        """
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
                # Also include any currently sounding drum voices still gated
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
                # Pressure > 0: volume only while the pad note is still held
                if not self._drum_gate.get(n, False):
                    continue
                if v is None:
                    continue
                if v.releasing:
                    continue
                v.velocity = vel
                v.target_amp = target

    def set_waveform(self, name: str) -> None:
        name = name.lower().strip()
        table = WAVETABLES.get(name)
        if table is None:
            return
        with self._lock:
            self._waveform = name
            self._table = table

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
            self._arange = np.arange(frames, dtype=np.float32)
            self._ramp = np.zeros(frames, dtype=np.float32)
        buf = self._scratch[:frames]
        buf.fill(0.0)
        ph = self._phase_buf[:frames]
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
            indices = np.bitwise_and(ph.astype(np.int32), TABLE_MASK)
            wave = table[indices]

            start_amp = v.amp
            if v.releasing:
                end_amp = max(0.0, start_amp - release_per_samp * frames * max(start_amp, 1e-4))
            elif v.target_amp > start_amp:
                end_amp = min(
                    v.target_amp,
                    start_amp + attack_per_samp * frames * max(v.target_amp, 0.05),
                )
            elif v.target_amp < start_amp:
                # Glide down when pad pressure / velocity softens
                end_amp = max(
                    v.target_amp,
                    start_amp - release_per_samp * frames * max(start_amp, 0.05),
                )
            else:
                end_amp = start_amp
            # Per-sample linear envelope — avoids boundary pops
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
    def __init__(self, port_filter: str, list_only: bool) -> None:
        self.port_filter = port_filter.strip().lower()
        self.event_q: queue.Queue = queue.Queue()
        self.engine = SineEngine()
        self._inport: Optional[mido.ports.BaseInput] = None
        self._poll_thread: Optional[threading.Thread] = None
        self._stop = threading.Event()

        if list_only:
            self._print_ports()
            return

        port_name = self._pick_port()
        if port_name is None:
            sys.exit("No MIDI input ports found. Is the MPK plugged in?")

        # Must exist before MIDI thread starts
        self._full_vel = True

        self.engine.start()
        self._inport = mido.open_input(port_name)
        self._poll_thread = threading.Thread(target=self._midi_loop, daemon=True)
        self._poll_thread.start()


        self.root = tk.Tk()
        self.root.title("midi-tone")
        self.root.geometry("800x480")
        self.root.configure(bg="#111111")
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)

        self._waveform = "sine"
        self._wave_btns: Dict[str, tk.Button] = {}
        self._full_vel_btn: Optional[tk.Button] = None

        # Bottom touch bar is packed first (side=BOTTOM) so it never gets crushed
        self._touch = tk.Frame(self.root, bg="#111111")
        self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)

        row1 = tk.Frame(self._touch, bg="#111111")
        row1.pack(fill=tk.X, pady=(0, 6))
        self._touch_btn(row1, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )
        self._touch_btn(row1, "CLEAR LOG", self._clear_log, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3
        )

        row2 = tk.Frame(self._touch, bg="#111111")
        row2.pack(fill=tk.X, pady=(0, 6))
        for name in ("sine", "square", "saw"):
            btn = self._touch_btn(
                row2, name.upper(), lambda n=name: self._select_wave(n), bg="#3c3836"
            )
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10)
            self._wave_btns[name] = btn
        self._paint_wave_btns()

        row3 = tk.Frame(self._touch, bg="#111111")
        row3.pack(fill=tk.X)
        self._full_vel_btn = self._touch_btn(
            row3, "FULL VELOCITY: ON", self._toggle_full_vel, bg="#689d6a"
        )
        self._full_vel_btn.pack(fill=tk.BOTH, ipady=8)

        # Everything above the touch bar lives here
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
            self._main, text="Event log  (double-tap to expand)", font=("DejaVu Sans", 10),
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

        # Explicit order for reliable fullscreen restore (pack order matters)
        self._log_chrome = [
            (header, dict(fill=tk.X, padx=8, pady=(8, 2))),
            (last_lbl, dict(fill=tk.X, padx=8, pady=4)),
            (active_lbl, dict(fill=tk.X, padx=8)),
            (mod_lbl, dict(fill=tk.X, padx=8, pady=(2, 0))),
            (self._log_title, dict(fill=tk.X, padx=8, pady=(6, 2))),
        ]
        self._log_expanded = False
        self._last_log_tap = 0.0
        self._exit_log_btn: Optional[tk.Button] = None
        self.log.bind("<Button-1>", self._on_log_tap)

        self._active_notes: Dict[Tuple[int, int], int] = {}
        self.root.after(50, self._drain_queue)
        self._append_log(f"Listening on: {port_name}")
        self._append_log("Touch UI: large pads at bottom.")
        self._append_log("Ch10 pads: velocity + aftertouch → volume.")
        self._append_log("Double-tap the log to fill the screen.")

    def _touch_btn(self, parent: tk.Misc, text: str, command, bg: str = "#3c3836") -> tk.Button:
        return tk.Button(
            parent, text=text, command=command,
            font=("DejaVu Sans", 14, "bold"), fg="#fbf1c7", bg=bg,
            activeforeground="#fbf1c7", activebackground=bg,
            relief=tk.FLAT, bd=0, padx=8, pady=12, cursor="hand2",
        )

    def _on_log_tap(self, _event: object = None) -> None:
        now = time.monotonic()
        if now - self._last_log_tap <= 0.45:
            self._last_log_tap = 0.0
            self._toggle_log_fullscreen()
        else:
            self._last_log_tap = now

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
            self._exit_log_btn = self._touch_btn(
                self.root, "EXIT FULLSCREEN LOG", self._toggle_log_fullscreen, bg="#9d0006"
            )
            self._exit_log_btn.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6, ipady=14)
            self._log_expanded = True
            self.last_var.set("Log fullscreen — tap EXIT to leave")
        else:
            if self._exit_log_btn is not None:
                self._exit_log_btn.destroy()
                self._exit_log_btn = None
            # Unpack log first so chrome can be packed above it again
            self._log_frame.pack_forget()
            for w, opts in self._log_chrome:
                w.pack(**opts)
            self._log_frame.pack(fill=tk.BOTH, expand=True, padx=8, pady=(0, 4))
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            self.log.configure(font=("DejaVu Sans Mono", 11))
            self._log_expanded = False
            self.log.see(tk.END)

    def _paint_wave_btns(self) -> None:
        for name, btn in self._wave_btns.items():
            on = name == self._waveform
            color = "#458588" if on else "#3c3836"
            btn.configure(bg=color, activebackground=color)

    def _select_wave(self, name: str) -> None:
        name = name.lower().strip()
        if name not in WAVETABLES:
            return
        self._waveform = name
        self.engine.set_waveform(name)
        self._paint_wave_btns()
        self.last_var.set(f"Waveform → {name.upper()}")
        self._append_log(f"Waveform → {name}")

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

    def _pick_port(self) -> Optional[str]:
        names = mido.get_input_names()
        if not names:
            return None
        if self.port_filter:
            for n in names:
                if self.port_filter in n.lower():
                    return n
            print(f"No input matching '{self.port_filter}'. Available:")
            for n in names:
                print(f"  {n}")
            sys.exit(1)
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
                # Don't kill the MIDI thread silently — surface it in the UI/log
                tb = __import__("traceback").format_exc()
                print(tb, flush=True)
                self.event_q.put(("log", f"MIDI ERROR: {exc}", False))
            time.sleep(0.001)

    def _put_continuous_log(self, line: str) -> None:
        """Throttle high-rate messages so the Tk queue can't freeze the UI."""
        now = time.monotonic()
        if now - getattr(self, "_last_cont_put", 0.0) < 0.05:
            self._pending_cont_log = line
            return
        self._last_cont_put = now
        self._pending_cont_log = None
        self.event_q.put(("log", line, True))

    def _handle_midi(self, msg: mido.Message) -> None:
        continuous = msg.type == "pitchwheel" or (
            msg.type == "control_change" and msg.control == 1
        )

        if msg.type == "note_on" and msg.velocity > 0:
            is_drum = msg.channel == DRUM_CHANNEL
            # Pads always use real velocity; keys honor "Always full velocity"
            vel = msg.velocity if is_drum or not self._full_vel else 127
            self.engine.note_on(msg.channel, msg.note, vel)
            self.event_q.put(("on", msg.channel, msg.note, vel))
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
            self.event_q.put(("log", line, False))
        elif msg.type == "note_off" or (msg.type == "note_on" and msg.velocity == 0):
            self.engine.note_off(msg.channel, msg.note)
            self.event_q.put(("off", msg.channel, msg.note))
            self.event_q.put(("log", format_message(msg), False))
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
            self.event_q.put(("mod",))
            self._put_continuous_log(format_message(msg))
        elif msg.type == "control_change":
            if msg.control == 1:
                self.engine.set_mod_wheel(msg.value)
                self.event_q.put(("mod",))
                self._put_continuous_log(format_message(msg))
            elif msg.control == 123:
                self.engine.all_notes_off()
                self.event_q.put(("panic",))
                self.event_q.put(("log", format_message(msg), False))
            else:
                self.event_q.put(("log", format_message(msg), continuous))
        else:
            self.event_q.put(("log", format_message(msg), False))

    def _drain_queue(self) -> None:
        # Cap work per tick so a flood can't freeze the UI for seconds
        processed = 0
        try:
            while processed < 40:
                item = self.event_q.get_nowait()
                processed += 1
                kind = item[0]
                if kind == "log":
                    _, line, continuous = item
                    self.last_var.set(line)
                    if continuous:
                        now = time.monotonic()
                        if now - getattr(self, "_last_cont_log", 0.0) >= 0.08:
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
        if not self._stop.is_set():
            self.root.after(50, self._drain_queue)

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
        end_line = int(float(self.log.index("end-1c").split(".")[0]))
        if end_line > LOG_MAX:
            self.log.delete("1.0", f"{end_line - LOG_MAX}.0")
        self.log.see(tk.END)
        self.log.configure(state=tk.DISABLED)

    def _clear_log(self) -> None:
        self.log.configure(state=tk.NORMAL)
        self.log.delete("1.0", tk.END)
        self.log.configure(state=tk.DISABLED)

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
    parser = argparse.ArgumentParser(description="MIDI → sine diagnostic with event UI")
    parser.add_argument("--input", "-i", default="", help="MIDI input name substring")
    parser.add_argument("--list", "-l", action="store_true", help="List MIDI inputs")
    args = parser.parse_args()

    try:
        mido.set_backend("mido.backends.rtmidi")
    except Exception:
        pass

    app = MidiToneApp(port_filter=args.input, list_only=args.list)
    if not args.list:
        app.run()


if __name__ == "__main__":
    main()
