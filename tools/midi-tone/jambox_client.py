#!/usr/bin/env python3
"""
Thin client for the Rust `jambox-engine`.

The kiosk UI does not render audio or schedule notes when this client is active —
it only describes intent and displays engine MIDI/state. Touch/KAOSS notes are
injected as MIDI ingest (same path as the MPK). Every call here is non-blocking.

If the engine is not running, the client degrades to a no-op and `connected` stays
False, which lets the existing in-process Python synth keep working.
"""

from __future__ import annotations

import json
import os
import pathlib
import queue
import shutil
import socket
import subprocess
import sys
import threading
import time
from typing import Any, Dict, Iterable, List, Optional, Tuple

DEFAULT_SOCKET = "/tmp/jambox.sock"
DEFAULT_TCP = "127.0.0.1:17890"
# Ticks per quarter note — must match jambox_core::PPQ.
PPQ = 960
# Bounded so a dead engine cannot grow memory without limit.
SEND_QUEUE_MAX = 512
MIDI_QUEUE_MAX = 512
RECONNECT_MIN = 0.5
RECONNECT_MAX = 5.0

# Kit order must match jambox_core::DrumModel.
DRUM_MODEL_NAMES = (
    "kick",
    "snare",
    "clap",
    "hat_closed",
    "hat_open",
    "tom_lo",
    "tom_mid",
    "rim",
    "kick_tight",
    "rimshot",
    "shaker",
    "hat_pedal",
    "tom_hi",
    "cowbell",
    "clave",
    "ride",
)


def seconds_to_ticks(seconds: float, bpm: float) -> int:
    """Convert free-timing take seconds into musical ticks."""
    beats = max(0.0, float(seconds)) * (float(bpm) / 60.0)
    return int(round(beats * PPQ))


class JamboxClient:
    """Line-delimited JSON client with a background sender."""

    def __init__(
        self,
        address: str = DEFAULT_SOCKET,
        *,
        tcp: bool = False,
        auto_connect: bool = True,
    ) -> None:
        self.address = address
        self.tcp = bool(tcp)
        self._sock: Optional[socket.socket] = None
        self._lock = threading.Lock()
        self._q: "queue.Queue[Dict[str, Any]]" = queue.Queue(maxsize=SEND_QUEUE_MAX)
        self._stop = threading.Event()
        self._connected = threading.Event()
        self._dropped = 0
        self._thread: Optional[threading.Thread] = None
        # Status arrives on the same connection; the engine serves one client.
        self._status_lock = threading.Lock()
        self._last_status: Optional[Dict[str, Any]] = None
        self._status_seq = 0
        self._midi_q: "queue.Queue[Dict[str, Any]]" = queue.Queue(maxsize=MIDI_QUEUE_MAX)
        if auto_connect:
            self.start()

    # ---- lifecycle -----------------------------------------------------

    def start(self) -> None:
        if self._thread is not None:
            return
        self._stop.clear()
        self._thread = threading.Thread(
            target=self._run, name="jambox-client", daemon=True
        )
        self._thread.start()

    def close(self) -> None:
        self._stop.set()
        with self._lock:
            if self._sock is not None:
                try:
                    self._sock.close()
                except OSError:
                    pass
                self._sock = None
        self._connected.clear()

    @property
    def connected(self) -> bool:
        return self._connected.is_set()

    @property
    def dropped(self) -> int:
        """Messages discarded because the queue was full (engine down/slow)."""
        return self._dropped

    # ---- transport -----------------------------------------------------

    def _connect(self) -> Optional[socket.socket]:
        try:
            if self.tcp:
                host, _, port = self.address.rpartition(":")
                sock = socket.create_connection((host or "127.0.0.1", int(port)), 1.0)
            else:
                if not hasattr(socket, "AF_UNIX"):
                    return None
                sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                sock.settimeout(1.0)
                sock.connect(self.address)
        except (OSError, ValueError):
            return None
        sock.settimeout(1.0)
        return sock

    def _run(self) -> None:
        backoff = RECONNECT_MIN
        while not self._stop.is_set():
            sock = self._connect()
            if sock is None:
                self._connected.clear()
                time.sleep(backoff)
                backoff = min(RECONNECT_MAX, backoff * 2.0)
                continue

            with self._lock:
                self._sock = sock
            self._connected.set()
            backoff = RECONNECT_MIN

            # Replies are a byte stream, not messages: a dedicated reader keeps
            # partial lines from stalling status until the next send.
            reader = threading.Thread(
                target=self._read_loop, args=(sock,), name="jambox-reader", daemon=True
            )
            reader.start()

            try:
                while not self._stop.is_set():
                    try:
                        message = self._q.get(timeout=0.2)
                    except queue.Empty:
                        if not self._connected.is_set():
                            break
                        continue
                    payload = (json.dumps(message) + "\n").encode("utf-8")
                    sock.sendall(payload)
            except OSError:
                pass
            finally:
                self._connected.clear()
                with self._lock:
                    self._sock = None
                try:
                    sock.close()
                except OSError:
                    pass
                reader.join(timeout=0.5)

    def _read_loop(self, sock: socket.socket) -> None:
        """Consume replies until the socket closes."""
        pending = b""
        while not self._stop.is_set():
            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            if not chunk:
                break
            pending += chunk
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                self._handle_reply(line)
        self._connected.clear()

    def _handle_reply(self, raw: bytes) -> None:
        text = raw.decode("utf-8", "replace").strip()
        if not text:
            return
        try:
            reply = json.loads(text)
        except ValueError:
            return
        if isinstance(reply, dict) and isinstance(reply.get("status"), dict):
            with self._status_lock:
                self._last_status = reply["status"]
                self._status_seq += 1
            return
        if isinstance(reply, dict) and isinstance(reply.get("midi"), dict):
            try:
                self._midi_q.put_nowait(reply["midi"])
            except queue.Full:
                try:
                    self._midi_q.get_nowait()
                except queue.Empty:
                    pass
                try:
                    self._midi_q.put_nowait(reply["midi"])
                except queue.Full:
                    pass

    def send(self, message: Dict[str, Any]) -> bool:
        """Queue a message. Never blocks; returns False if it was dropped."""
        try:
            self._q.put_nowait(message)
            return True
        except queue.Full:
            self._dropped += 1
            return False

    # ---- performance ---------------------------------------------------

    def midi(
        self,
        kind: str,
        *,
        channel: int = 0,
        note: Optional[int] = None,
        velocity: Optional[int] = None,
        control: Optional[int] = None,
        value: Optional[int] = None,
    ) -> bool:
        """Inject MIDI on the same ingest path as a hardware port."""
        message: Dict[str, Any] = {
            "cmd": "midi",
            "kind": str(kind),
            "channel": int(channel) & 0x0F,
        }
        if note is not None:
            message["note"] = int(note) & 0x7F
        if velocity is not None:
            message["velocity"] = max(0, min(127, int(velocity)))
        if control is not None:
            message["control"] = int(control) & 0x7F
        if value is not None:
            message["value"] = int(value)
        return self.send(message)

    def note_on(self, channel: int, note: int, velocity: int) -> bool:
        return self.midi("note_on", channel=channel, note=note, velocity=velocity)

    def note_off(self, channel: int, note: int) -> bool:
        return self.midi("note_off", channel=channel, note=note)

    def knob_map(
        self,
        mode: str,
        *,
        fx_kind: Optional[str] = None,
        fx_index: int = 0,
    ) -> bool:
        message: Dict[str, Any] = {"cmd": "knob_map", "mode": str(mode), "fx_index": int(fx_index)}
        if fx_kind:
            message["fx_kind"] = str(fx_kind)
        return self.send(message)

    def all_notes_off(self) -> bool:
        return self.send({"cmd": "all_notes_off"})

    def panic(self) -> bool:
        return self.send({"cmd": "panic"})

    # ---- sound ---------------------------------------------------------

    def synth(self, param: str, value: float) -> bool:
        return self.send({"cmd": "synth", "param": str(param), "value": float(value)})

    def fx(self, target: Dict[str, Any], param: str, value: float) -> bool:
        return self.send(
            {"cmd": "fx", "target": target, "param": str(param), "value": float(value)}
        )

    @staticmethod
    def voice_target(index: int) -> Dict[str, Any]:
        return {"kind": "voice", "index": int(index)}

    @staticmethod
    def drum_target(index: int) -> Dict[str, Any]:
        return {"kind": "drum", "index": int(index)}

    @staticmethod
    def drum_group_target() -> Dict[str, Any]:
        return {"kind": "drum_group"}

    @staticmethod
    def bus_target() -> Dict[str, Any]:
        return {"kind": "bus"}

    def morph_pair(self, a: int, b: int) -> bool:
        return self.send({"cmd": "morph_pair", "a": int(a), "b": int(b)})

    # ---- transport / clips ---------------------------------------------

    def tempo(self, bpm: float) -> bool:
        return self.send({"cmd": "tempo", "bpm": float(bpm)})

    def beats_per_bar(self, beats: int) -> bool:
        return self.send({"cmd": "beats_per_bar", "beats": int(beats)})

    def clip_load(
        self,
        slot: int,
        events: Iterable[Tuple[int, bool, int, int, int]],
        length_ticks: int,
        mode: str = "loop",
    ) -> bool:
        """Upload a phrase. `events` is (tick, on, channel, note, velocity)."""
        wire: List[Dict[str, Any]] = [
            {
                "tick": int(tick),
                "on": bool(on),
                "channel": int(channel) & 0x0F,
                "note": int(note) & 0x7F,
                "velocity": max(0, min(127, int(velocity))),
            }
            for tick, on, channel, note, velocity in events
        ]
        return self.send(
            {
                "cmd": "clip_load",
                "slot": int(slot),
                "length_ticks": int(length_ticks),
                "mode": str(mode),
                "events": wire,
            }
        )

    def clip_clear(self, slot: int) -> bool:
        return self.send({"cmd": "clip_clear", "slot": int(slot)})

    def clip_mode(self, slot: int, mode: str) -> bool:
        return self.send({"cmd": "clip_mode", "slot": int(slot), "mode": str(mode)})

    def clip_launch(self, slot: int, quantize: str = "bar") -> bool:
        return self.send(
            {"cmd": "clip_launch", "slot": int(slot), "quantize": str(quantize)}
        )

    def clip_stop(self, slot: int, quantize: str = "bar") -> bool:
        return self.send(
            {"cmd": "clip_stop", "slot": int(slot), "quantize": str(quantize)}
        )

    def stop_all_clips(self) -> bool:
        return self.send({"cmd": "stop_all_clips"})

    def status(self, timeout: float = 0.5) -> Optional[Dict[str, Any]]:
        """Ask for a fresh status, waiting briefly for the reply.

        Never blocks longer than `timeout`; returns the last known value (or None)
        if the engine is slow or absent, so a UI tick can call this safely.
        """
        with self._status_lock:
            seen = self._status_seq
        self.send({"cmd": "status"})
        deadline = time.time() + max(0.0, timeout)
        while time.time() < deadline:
            with self._status_lock:
                if self._status_seq != seen:
                    return self._last_status
            time.sleep(0.005)
        with self._status_lock:
            return self._last_status

    def drain_midi(self) -> List[Dict[str, Any]]:
        """Non-blocking: every MIDI notice received since the last drain."""
        out: List[Dict[str, Any]] = []
        while True:
            try:
                out.append(self._midi_q.get_nowait())
            except queue.Empty:
                return out


def prefer_python_engine() -> bool:
    return os.environ.get("MIDI_TONE_ENGINE", "").strip().lower() in (
        "python",
        "py",
        "sine",
    )


def control_address() -> Tuple[str, bool]:
    """(address, tcp). Unix socket on POSIX unless MIDI_TONE_JAMBOX_TCP is set."""
    env_addr = os.environ.get("MIDI_TONE_JAMBOX_SOCK", "").strip()
    env_tcp = os.environ.get("MIDI_TONE_JAMBOX_TCP", "").strip().lower()
    if env_tcp in ("1", "true", "yes") or (env_addr and ":" in env_addr and not env_addr.startswith("/")):
        return env_addr or DEFAULT_TCP, True
    if sys.platform == "win32" and not env_addr:
        return DEFAULT_TCP, True
    return env_addr or DEFAULT_SOCKET, False


def engine_binary() -> Optional[pathlib.Path]:
    env = os.environ.get("MIDI_TONE_JAMBOX_BIN", "").strip()
    if env:
        path = pathlib.Path(env)
        return path if path.is_file() else None
    here = pathlib.Path(__file__).resolve().parent
    roots: List[pathlib.Path] = []
    # Full clone: tools/midi-tone → repo root.
    if here.name == "midi-tone" and here.parent.name == "tools":
        roots.append(here.parents[1])
    # Split kiosk install (~/midi-tone) plus the usual engine tree.
    roots.append(here)
    roots.append(pathlib.Path.home() / "pi-midi-toolkit")
    roots.append(pathlib.Path("/home/pi/pi-midi-toolkit"))
    names = ("jambox-engine.exe", "jambox-engine") if sys.platform == "win32" else ("jambox-engine",)
    candidates: List[pathlib.Path] = []
    for root in roots:
        for name in names:
            candidates.extend(
                (
                    root / "bin" / name,
                    root / "dist" / "armv7" / name,
                    root / "target" / "release" / name,
                    root / "target" / "debug" / name,
                )
            )
    which = shutil.which("jambox-engine")
    if which:
        candidates.append(pathlib.Path(which))
    seen = set()
    for path in candidates:
        resolved = path
        if resolved in seen:
            continue
        seen.add(resolved)
        if path.is_file():
            return path
    return None


def wait_connected(client: JamboxClient, timeout: float = 2.5) -> bool:
    deadline = time.time() + max(0.0, timeout)
    while time.time() < deadline:
        if client.connected:
            return True
        time.sleep(0.05)
    return client.connected


def spawn_engine(
    *,
    address: str,
    tcp: bool,
    waves: Optional[pathlib.Path] = None,
    user_waves: Optional[pathlib.Path] = None,
    midi_in: str = "MPK",
    null_audio: bool = False,
    rt: bool = False,
) -> Optional[subprocess.Popen]:
    binary = engine_binary()
    if binary is None:
        return None
    cmd: List[str] = [str(binary), "run", "--midi-in", midi_in or "MPK", "--output", "headphone"]
    if tcp:
        cmd.extend(["--tcp", "--control", address])
    else:
        cmd.extend(["--control", address])
    if waves is not None and pathlib.Path(waves).is_dir():
        cmd.extend(["--waves", str(pathlib.Path(waves))])
    if user_waves is not None and pathlib.Path(user_waves).is_dir():
        cmd.extend(["--user-waves", str(pathlib.Path(user_waves))])
    if null_audio or os.environ.get("MIDI_TONE_JAMBOX_NULL", "").strip().lower() in (
        "1",
        "true",
        "yes",
    ):
        cmd.append("--null-audio")
    if rt or os.environ.get("MIDI_TONE_JAMBOX_RT", "").strip().lower() in (
        "1",
        "true",
        "yes",
    ):
        cmd.append("--rt")
    try:
        return subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError:
        return None


def connect_or_spawn(
    *,
    waves: Optional[pathlib.Path] = None,
    user_waves: Optional[pathlib.Path] = None,
    midi_in: str = "MPK",
    spawn: Optional[bool] = None,
) -> Tuple[Optional[JamboxClient], Optional[subprocess.Popen]]:
    """Connect to a running engine, optionally spawning one.

    Spawn defaults on when MIDI_TONE_SPAWN=1 (kiosk.sh). Unit tests leave it off
    so a leftover binary in target/debug cannot steal MIDI from FakePort.
    """
    if prefer_python_engine():
        return None, None
    address, tcp = control_address()
    client = JamboxClient(address, tcp=tcp)
    if wait_connected(client, timeout=0.6):
        return client, None
    do_spawn = spawn
    if do_spawn is None:
        flag = os.environ.get("MIDI_TONE_SPAWN", "").strip().lower()
        do_spawn = flag in ("1", "true", "yes")
    if not do_spawn:
        client.close()
        return None, None
    proc = spawn_engine(
        address=address,
        tcp=tcp,
        waves=waves,
        user_waves=user_waves,
        midi_in=midi_in,
    )
    if proc is None:
        client.close()
        return None, None
    if wait_connected(client, timeout=4.0):
        return client, proc
    try:
        proc.terminate()
    except OSError:
        pass
    client.close()
    return None, None


def midi_notice_to_message(notice: Dict[str, Any]) -> Any:
    """Turn an engine MIDI notice into a mido.Message (or None)."""
    try:
        import mido
    except ImportError:
        return None
    kind = str(notice.get("kind") or "")
    ch = int(notice.get("channel") or 0) & 0x0F
    if kind == "note_on":
        return mido.Message(
            "note_on",
            channel=ch,
            note=int(notice.get("note") or 0) & 0x7F,
            velocity=int(notice.get("velocity") or 0) & 0x7F,
        )
    if kind == "note_off":
        return mido.Message(
            "note_off",
            channel=ch,
            note=int(notice.get("note") or 0) & 0x7F,
            velocity=int(notice.get("velocity") or 0) & 0x7F,
        )
    if kind == "control_change":
        return mido.Message(
            "control_change",
            channel=ch,
            control=int(notice.get("control") or 0) & 0x7F,
            value=int(notice.get("value") or 0) & 0x7F,
        )
    if kind in ("pitch_bend", "pitchwheel"):
        value = int(notice.get("value") or 8192)
        return mido.Message("pitchwheel", channel=ch, pitch=value - 8192)
    if kind in ("channel_pressure", "aftertouch"):
        return mido.Message(
            "aftertouch",
            channel=ch,
            value=int(notice.get("value") or 0) & 0x7F,
        )
    if kind in ("poly_pressure", "polytouch"):
        return mido.Message(
            "polytouch",
            channel=ch,
            note=int(notice.get("note") or 0) & 0x7F,
            value=int(notice.get("value") or 0) & 0x7F,
        )
    if kind == "program_change":
        return mido.Message(
            "program_change",
            channel=ch,
            program=int(notice.get("value") or 0) & 0x7F,
        )
    return None


def main() -> None:
    """Tiny CLI so you can poke a running engine from the Pi shell."""
    import argparse

    parser = argparse.ArgumentParser(description="Talk to jambox-engine")
    parser.add_argument("--address", default=DEFAULT_SOCKET)
    parser.add_argument("--tcp", action="store_true")
    parser.add_argument("--status", action="store_true", help="print engine status")
    parser.add_argument("--note", type=int, help="play a note number briefly")
    args = parser.parse_args()

    client = JamboxClient(args.address, tcp=args.tcp)
    # Give the sender thread a moment to connect before the first request.
    time.sleep(0.3)
    if args.status:
        print(json.dumps(client.status(timeout=1.0), indent=2))
    if args.note is not None:
        client.note_on(0, args.note, 110)
        time.sleep(0.6)
        client.note_off(0, args.note)
        time.sleep(0.2)
    client.close()


if __name__ == "__main__":
    main()
