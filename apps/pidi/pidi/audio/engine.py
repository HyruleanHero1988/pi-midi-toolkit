"""Python PortAudio soft-synth (fallback when jambox-engine is unavailable)."""
from __future__ import annotations

import math
import pathlib
import threading
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

try:
    import sounddevice as sd
except ImportError as e:  # pragma: no cover
    raise SystemExit(
        "sounddevice required: pip install sounddevice\n"
        "On Pi you may also need: sudo apt install libportaudio2\n" + str(e)
    ) from e

from pidi.audio.drums import midi_to_hz, synthesize_drum
from pidi.audio.fx import MixBusFx
from pidi.audio.tone import apply_tone_lowpass
from pidi.audio.wavetable import (
    load_user_voice_fx_map,
    load_wavetables,
    sanitize_voice_name,
    suggest_voice_name,
    unique_voice_name,
    voice_fx_sidecar_path,
    write_voice_fx_sidecar,
    write_wavetable_wav,
)
from pidi.constants import (
    BLOCKSIZE,
    DEFAULT_MAX_VOICES,
    DRUM_BUS_GAIN,
    DRUM_CHANNEL,
    LATENCY_SEC,
    OUTPUT_MAKEUP,
    SAMPLE_RATE,
    TABLE_MASK,
    TABLE_SIZE,
    USER_WAVETABLES_DIR,
    VOICE_AMP,
)
from pidi.jambox_client import JamboxClient


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
        self._svf_band = 0.0
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
        self._remote: Optional[JamboxClient] = None
        self._echoing = False

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

    def attach_remote(self, client: Optional[JamboxClient]) -> None:
        """Mirror notes/params to jambox-engine. None detaches."""
        self._remote = client
        if client is not None and client.connected:
            self.sync_remote()
            self._push_knob_map()

    def using_remote(self) -> bool:
        return self._remote is not None and self._remote.connected

    def _r(self) -> Optional[JamboxClient]:
        client = self._remote
        if client is None or not client.connected:
            return None
        return client

    def _push_knob_map(self) -> None:
        """UI mode buttons tell ingest which bank hardware knobs address."""
        client = self._r()
        if client is None:
            return
        from jambox_client import DRUM_MODEL_NAMES

        with self._lock:
            if self._fx_mode or self._bus_fx_mode:
                mode = "fx"
                if self._fx_edit_kind == "bus" or self._bus_fx_mode:
                    kind, index = "bus", 0
                elif self._fx_edit_kind == "drums":
                    kind, index = "drums", 0
                elif self._fx_edit_kind == "drum":
                    kind = "drum"
                    try:
                        index = DRUM_MODEL_NAMES.index(str(self._fx_edit_drum or "kick"))
                    except ValueError:
                        index = 0
                else:
                    kind = "voice"
                    if self._morph_dirty:
                        self._rebuild_morph_table_unlocked()
                    index = self._morph_a if self._morph < 0.5 else self._morph_b
            elif self._drum_mode:
                mode, kind, index = "drums", None, 0
            else:
                mode, kind, index = "keys", None, 0
        client.knob_map(mode, fx_kind=kind, fx_index=index)

    @staticmethod
    def _unit01(value: float) -> float:
        v = float(value)
        if v > 1.0:
            v = v / 127.0
        return max(0.0, min(1.0, v))

    def _remote_synth(self, param: str, value: float) -> None:
        if self._echoing:
            return
        client = self._r()
        if client is not None:
            client.synth(param, float(value))

    def _remote_fx_target_unlocked(self) -> Dict[str, Any]:
        if self._fx_edit_kind == "bus" or self._bus_fx_mode:
            return JamboxClient.bus_target()
        if self._fx_edit_kind == "drums":
            return JamboxClient.drum_group_target()
        if self._fx_edit_kind == "drum":
            from jambox_client import DRUM_MODEL_NAMES

            name = str(self._fx_edit_drum or "kick")
            try:
                index = DRUM_MODEL_NAMES.index(name)
            except ValueError:
                index = 0
            return JamboxClient.drum_target(index)
        if self._morph_dirty:
            self._rebuild_morph_table_unlocked()
        near = self._morph_a if self._morph < 0.5 else self._morph_b
        return JamboxClient.voice_target(near)

    def _remote_fx(self, target: Dict[str, Any], param: str, value: float) -> None:
        if self._echoing:
            return
        client = self._r()
        if client is not None:
            client.fx(target, param, float(value))

    def sync_remote(self) -> None:
        """Push current Python synth/FX state to the engine (session restore)."""
        client = self._r()
        if client is None:
            return
        with self._lock:
            morph_a, morph_b = self._morph_a, self._morph_b
            morph = float(self._morph)
            tone = float(self._tone)
            level = float(self._synth_level)
            attack = float(self._attack_sec)
            release = float(self._release_sec)
            vib_depth = float(self._vib_depth_semis)
            vib_hz = float(self._vib_hz)
            vib_always = float(self._vib_always)
            vib_mod = float(self._mod)
            bend = float(self._bend_semitones)
            drum_pitch = float(self._drum_pitch)
            drum_decay = float(self._drum_decay)
            drum_noise = float(self._drum_noise)
            drum_tone = float(self._drum_tone)
            bus_snap = self._bus_fx.snapshot()
            group_snap = self._drum_group_fx.snapshot()
            voice_fx = {k: v.snapshot() for k, v in self._voice_fx.items()}
            drum_fx = {k: v.snapshot() for k, v in self._drum_fx.items()}
            names = list(self._voice_names)
        client.morph_pair(morph_a, morph_b)
        client.synth("morph", morph)
        client.synth("tone", tone)
        client.synth("level", level)
        amin, amax = self.ATTACK_SEC_MIN, self.ATTACK_SEC_MAX
        rmin, rmax = self.RELEASE_SEC_MIN, self.RELEASE_SEC_MAX
        attack_u = 0.0
        if attack > amin and amax > amin:
            attack_u = math.log(max(amin, attack) / amin) / math.log(amax / amin)
        release_u = 0.0
        if release > rmin and rmax > rmin:
            release_u = math.log(max(rmin, release) / rmin) / math.log(rmax / rmin)
        client.synth("attack", max(0.0, min(1.0, attack_u)))
        client.synth("release", max(0.0, min(1.0, release_u)))
        client.synth("vibrato_depth", max(0.0, min(1.0, vib_depth / 2.0)))
        client.synth("vibrato_rate", max(0.0, min(1.0, (vib_hz - 1.0) / 8.0)))
        client.synth("vibrato_always", vib_always)
        client.synth("vibrato_mod", vib_mod)
        client.synth("pitch_bend", bend)
        client.synth("drum_pitch", drum_pitch)
        client.synth("drum_decay", drum_decay)
        client.synth("drum_noise", drum_noise)
        client.synth("drum_tone", drum_tone)
        snap_to_param = {
            "fx_drive": "drive",
            "fx_delay_time": "delay_time",
            "fx_delay_fb": "delay_fb",
            "fx_delay_mix": "delay_mix",
            "fx_reverb_size": "reverb_size",
            "fx_reverb_mix": "reverb_mix",
        }
        for src, param in snap_to_param.items():
            if src in bus_snap:
                client.fx(JamboxClient.bus_target(), param, float(bus_snap[src]))
            if src in group_snap:
                client.fx(JamboxClient.drum_group_target(), param, float(group_snap[src]))
        from jambox_client import DRUM_MODEL_NAMES

        for i, name in enumerate(names):
            snap = voice_fx.get(name)
            if not snap:
                continue
            for src, param in snap_to_param.items():
                if src in snap:
                    client.fx(JamboxClient.voice_target(i), param, float(snap[src]))
        for model, snap in drum_fx.items():
            try:
                index = DRUM_MODEL_NAMES.index(model)
            except ValueError:
                continue
            for src, param in snap_to_param.items():
                if src in snap:
                    client.fx(JamboxClient.drum_target(index), param, float(snap[src]))
        self._push_knob_map()

    def morph_pair_indices(self) -> Tuple[int, int]:
        with self._lock:
            return self._morph_a, self._morph_b

    def start(self) -> None:
        if self.using_remote():
            print("audio: jambox-engine (Python PortAudio skipped)", flush=True)
            return
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
        client = self._r()
        if client is not None:
            client.midi("note_on", channel=ch, note=n, velocity=velocity)
            return
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
            else:
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
        client = self._r()
        if client is not None:
            client.midi("note_off", channel=channel & 0x0F, note=note & 0x7F)
            return
        key = (channel & 0x0F, note & 0x7F)
        with self._lock:
            if (channel & 0x0F) == DRUM_CHANNEL:
                self._drum_gate[key[1]] = False
                # One-shots keep decaying; open hat shortens if pad released early
                hit = self._drums.get(key[1])
                if hit is not None and hit.model == "hat_open":
                    hit.decay *= 0.35
            else:
                v = self._voices.get(key)
                if v is not None:
                    v.releasing = True
                    v.target_amp = 0.0

    def all_notes_off(self) -> None:
        with self._lock:
            self._drum_gate.clear()
            self._drums.clear()
            for v in self._voices.values():
                v.releasing = True
                v.target_amp = 0.0
        if not self._echoing:
            client = self._r()
            if client is not None:
                client.all_notes_off()

    def set_pitch_bend(self, pitch: int) -> None:
        with self._lock:
            self._bend_semitones = (pitch / 8192.0) * self._bend_range
            semis = float(self._bend_semitones)
        self._remote_synth("pitch_bend", semis)

    def set_mod_wheel(self, value: int) -> None:
        with self._lock:
            self._mod = max(0.0, min(1.0, value / 127.0))
            mod = float(self._mod)
        self._remote_synth("vibrato_mod", mod)

    def set_morph(self, value: float) -> None:
        """Blend A→B: 0..1 (or MIDI 0..127 if > 1)."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._morph = max(0.0, min(1.0, float(value)))
            self._morph_dirty = True
        self._remote_synth("morph", self._morph)

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
        self._push_morph_pair()

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
        self._push_morph_pair()

    def set_morph_index(self, index: int) -> None:
        """PREV/NEXT / VOICES: set A to this voice and park morph at pure A."""
        with self._lock:
            n = len(self._voice_names)
            idx = max(0, min(n - 1, int(index)))
            self._morph_a = idx
            self._morph = 0.0
            self._morph_dirty = True
            self._rebuild_morph_table_unlocked()
        self._push_morph_pair()

    def _push_morph_pair(self) -> None:
        client = self._r()
        if client is None:
            return
        with self._lock:
            a, b, m = self._morph_a, self._morph_b, float(self._morph)
        client.morph_pair(a, b)
        client.synth("morph", m)

    def set_tone(self, value: float) -> None:
        """Brightness 0..1 (MIDI 0..127 accepted)."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._tone = max(0.0, min(1.0, float(value)))
        self._remote_synth("tone", self._tone)

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
        self._remote_synth("level", self._synth_level)

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
        self._remote_synth("attack", t)

    def set_release(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        t = max(0.0, min(1.0, float(value)))
        sec = self.RELEASE_SEC_MIN * ((self.RELEASE_SEC_MAX / self.RELEASE_SEC_MIN) ** t)
        with self._lock:
            self._release_sec = sec
        self._remote_synth("release", t)

    def set_vib_depth(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            # 0..2 semitones
            self._vib_depth_semis = max(0.0, min(1.0, float(value))) * 2.0
        self._remote_synth("vibrato_depth", self._unit01(value))

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
            out = float(self._vib_always)
        self._remote_synth("vibrato_always", out)
        return out

    def nudge_vib_depth(self, delta_semis: float) -> float:
        with self._lock:
            depth = self._vib_depth_semis + float(delta_semis)
            self._vib_depth_semis = max(0.0, min(self.VIB_DEPTH_MAX, depth))
            out = float(self._vib_depth_semis)
        self._remote_synth("vibrato_depth", out / self.VIB_DEPTH_MAX)
        return out

    def nudge_vib_rate(self, delta_hz: float) -> float:
        with self._lock:
            hz = self._vib_hz + float(delta_hz)
            self._vib_hz = max(self.VIB_HZ_MIN, min(self.VIB_HZ_MAX, hz))
            out = float(self._vib_hz)
        span = self.VIB_HZ_MAX - self.VIB_HZ_MIN
        self._remote_synth("vibrato_rate", (out - self.VIB_HZ_MIN) / span if span else 0.0)
        return out

    def set_vib_rate(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        t = max(0.0, min(1.0, float(value)))
        # ~1 Hz .. ~9 Hz
        with self._lock:
            self._vib_hz = 1.0 + t * 8.0
        self._remote_synth("vibrato_rate", t)

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
        self._push_knob_map()

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
        self._push_knob_map()

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
        self._push_knob_map()

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
        self._push_knob_map()

    def set_fx_edit_drum(self, model: str) -> None:
        """Point insert-FX knobs at a drum model insert (kick, snare, …)."""
        with self._lock:
            self._fx_edit_kind = "drum"
            self._fx_edit_drum = str(model or "kick")
            self._ensure_drum_fx_unlocked(self._fx_edit_drum)
        self._push_knob_map()

    def set_fx_edit_drums(self) -> None:
        """Point insert-FX knobs at the shared all-drums group bus."""
        with self._lock:
            self._fx_edit_kind = "drums"
        self._push_knob_map()

    def set_fx_edit_bus(self) -> None:
        """Point knobs at the master mix-bus FX."""
        with self._lock:
            self._fx_edit_kind = "bus"
        self._push_knob_map()

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
            target = self._remote_fx_target_unlocked()
        self._remote_fx(target, attr, value)

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

    KAOSS_BUS_PARAMS = (
        "drive",
        "delay_time",
        "delay_fb",
        "delay_mix",
        "reverb_size",
        "reverb_mix",
    )

    def set_kaoss_param(self, name: str, value: float) -> None:
        """Live XY mapping: voice tone/morph/vib, or mix-bus FX (Kaoss insert)."""
        if name == "tone":
            self.set_tone(value)
            return
        if name == "morph":
            self.set_morph(value)
            return
        if name == "vib":
            self.set_vib_depth(value)
            self.set_vib_always(1.0 if float(value) > 0.02 else 0.0)
            return
        if name == "level":
            self.set_level(value)
            return
        if name == "attack":
            self.set_attack(value)
            return
        if name == "release":
            self.set_release(value)
            return
        if name in self.KAOSS_BUS_PARAMS:
            if value > 1.0:
                value = value / 127.0
            value = max(0.0, min(1.0, float(value)))
            with self._lock:
                setattr(self._bus_fx, name, value)
            self._remote_fx(JamboxClient.bus_target(), name, value)

    def wipe_kaoss_bus_fx(self) -> None:
        """Clear mix-bus pad FX (and delay/reverb memory) and tell the engine."""
        with self._lock:
            self._bus_fx.reset_to_defaults()
        self.set_vib_always(0.0)
        snap = self.bus_fx_snapshot()
        self._remote_fx(JamboxClient.bus_target(), "drive", float(snap["fx_drive"]))
        self._remote_fx(JamboxClient.bus_target(), "delay_time", float(snap["fx_delay_time"]))
        self._remote_fx(JamboxClient.bus_target(), "delay_fb", float(snap["fx_delay_fb"]))
        self._remote_fx(JamboxClient.bus_target(), "delay_mix", float(snap["fx_delay_mix"]))
        self._remote_fx(JamboxClient.bus_target(), "reverb_size", float(snap["fx_reverb_size"]))
        self._remote_fx(JamboxClient.bus_target(), "reverb_mix", float(snap["fx_reverb_mix"]))

    def bus_fx_snapshot(self) -> Dict[str, float]:
        with self._lock:
            return self._bus_fx.snapshot()

    def apply_bus_fx_snapshot(self, data: Dict[str, Any]) -> None:
        with self._lock:
            self._bus_fx.apply_snapshot(data)

    def fx_edit_snapshot(self) -> Dict[str, float]:
        with self._lock:
            return self._fx_edit_slot_unlocked().snapshot()

    def set_drum_pitch(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_pitch = max(0.0, min(1.0, float(value)))
        self._remote_synth("drum_pitch", self._drum_pitch)

    def set_drum_decay(self, value: float) -> None:
        """Stretch / body length."""
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_decay = max(0.0, min(1.0, float(value)))
        self._remote_synth("drum_decay", self._drum_decay)

    def set_drum_noise(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_noise = max(0.0, min(1.0, float(value)))
        self._remote_synth("drum_noise", self._drum_noise)

    def set_drum_tone(self, value: float) -> None:
        if value > 1.0:
            value = value / 127.0
        with self._lock:
            self._drum_tone = max(0.0, min(1.0, float(value)))
        self._remote_synth("drum_tone", self._drum_tone)

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
            self._svf_band = 0.0
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

        # Tone = brightness (Kaoss Y on LEAD, MPK tone knob). Top/open = 1.
        if tone < 0.985 and frames > 0:
            self._filter_state, self._svf_band = apply_tone_lowpass(
                key_bus,
                tone,
                self._filter_state,
                self._svf_band,
                sr,
            )
        elif frames > 0:
            self._filter_state = float(key_bus[-1])
            self._svf_band = 0.0

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


