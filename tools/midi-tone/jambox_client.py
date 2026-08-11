#!/usr/bin/env python3
"""
Thin client for the Rust `jambox-engine`.

The kiosk UI does not render audio or schedule notes when this client is active —
it only describes intent (see PLAN.md "UI is never on the audio / sequencer hot
path"). Every call here is non-blocking: messages go onto a bounded queue that a
daemon thread drains, so a stalled socket can never freeze Tk, and a busy Tk can
never stall the engine.

If the engine is not running, the client degrades to a no-op and `connected` stays
False, which lets the existing in-process Python synth keep working.
"""

from __future__ import annotations

import json
import queue
import socket
import threading
import time
from typing import Any, Dict, Iterable, List, Optional, Tuple

DEFAULT_SOCKET = "/tmp/jambox.sock"
# Ticks per quarter note — must match jambox_core::PPQ.
PPQ = 960
# Bounded so a dead engine cannot grow memory without limit.
SEND_QUEUE_MAX = 512
RECONNECT_MIN = 0.5
RECONNECT_MAX = 5.0


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

    def send(self, message: Dict[str, Any]) -> bool:
        """Queue a message. Never blocks; returns False if it was dropped."""
        try:
            self._q.put_nowait(message)
            return True
        except queue.Full:
            self._dropped += 1
            return False

    # ---- performance ---------------------------------------------------

    def note_on(self, channel: int, note: int, velocity: int) -> bool:
        return self.send(
            {
                "cmd": "note_on",
                "channel": int(channel) & 0x0F,
                "note": int(note) & 0x7F,
                "velocity": max(0, min(127, int(velocity))),
            }
        )

    def note_off(self, channel: int, note: int) -> bool:
        return self.send(
            {"cmd": "note_off", "channel": int(channel) & 0x0F, "note": int(note) & 0x7F}
        )

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
