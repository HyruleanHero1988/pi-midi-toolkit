"""SMF song library + player — headless transport."""
from __future__ import annotations

import pathlib
import shutil
import threading
import time
from typing import Any, Callable, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    DEMO_SONGS_DIR,
    DEFAULT_SONG_BPM,
    SONG_LIST_VISIBLE,
    SONG_OUT_MODES,
    SONG_OUT_PREFER,
    SONG_SEED_MARKER,
    SONGS_DIR,
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


