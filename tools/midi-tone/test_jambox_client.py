#!/usr/bin/env python3
"""
Protocol tests for the jambox thin client — no audio device or Rust build needed.

A fake Unix-socket server stands in for `jambox-engine` so we can prove the wire
shape and, more importantly, that the UI thread never blocks on the engine.
"""

from __future__ import annotations

import json
import os
import socket
import tempfile
import threading
import time
import unittest

from jambox_client import JamboxClient, PPQ, seconds_to_ticks


class FakeEngine:
    """Accepts one client, records every line, replies `ok`.

    `fragment=True` splits replies across packets, which is what a real TCP/Unix
    stream does and what broke an earlier one-read-per-send client.
    """

    def __init__(self, path: str, fragment: bool = False) -> None:
        self.path = path
        self.fragment = fragment
        self.lines: list[dict] = []
        self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._server.bind(path)
        self._server.listen(1)
        self._server.settimeout(0.5)
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                conn, _ = self._server.accept()
            except (socket.timeout, OSError):
                continue
            with conn:
                conn.settimeout(0.3)
                buf = b""
                while not self._stop.is_set():
                    try:
                        chunk = conn.recv(4096)
                    except socket.timeout:
                        continue
                    except OSError:
                        break
                    if not chunk:
                        break
                    buf += chunk
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        text = line.decode("utf-8").strip()
                        if not text:
                            continue
                        message = json.loads(text)
                        self.lines.append(message)
                        reply = (
                            {"status": {"bpm": 120.0, "position": 4242}}
                            if message.get("cmd") == "status"
                            else {"ok": None}
                        )
                        payload = (json.dumps(reply) + "\n").encode("utf-8")
                        try:
                            if self.fragment:
                                # Trailing newline arrives separately, later.
                                conn.sendall(payload[:-1])
                                time.sleep(0.02)
                                conn.sendall(payload[-1:])
                            else:
                                conn.sendall(payload)
                        except OSError:
                            break

    def wait_for(self, count: int, timeout: float = 2.0) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if len(self.lines) >= count:
                return True
            time.sleep(0.01)
        return False

    def close(self) -> None:
        self._stop.set()
        try:
            self._server.close()
        except OSError:
            pass


class TicksTest(unittest.TestCase):
    def test_a_beat_at_120bpm_is_one_quarter(self) -> None:
        self.assertEqual(seconds_to_ticks(0.5, 120.0), PPQ)

    def test_tempo_scales_the_conversion(self) -> None:
        self.assertEqual(seconds_to_ticks(0.5, 240.0), PPQ * 2)

    def test_negative_time_clamps_to_zero(self) -> None:
        self.assertEqual(seconds_to_ticks(-3.0, 120.0), 0)


class ClientProtocolTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.mkdtemp()
        self.path = os.path.join(self.dir, "jambox.sock")
        self.engine = FakeEngine(self.path)
        self.client = JamboxClient(self.path)

    def tearDown(self) -> None:
        self.client.close()
        self.engine.close()

    def test_note_on_arrives_with_masked_midi_values(self) -> None:
        self.client.note_on(99, 200, 250)
        self.assertTrue(self.engine.wait_for(1))
        message = self.engine.lines[0]
        self.assertEqual(message["cmd"], "note_on")
        self.assertLessEqual(message["channel"], 15)
        self.assertLessEqual(message["note"], 127)
        self.assertLessEqual(message["velocity"], 127)

    def test_fx_target_shape_matches_the_engine(self) -> None:
        self.client.fx(JamboxClient.drum_target(3), "delay_mix", 0.5)
        self.assertTrue(self.engine.wait_for(1))
        message = self.engine.lines[0]
        self.assertEqual(message["target"], {"kind": "drum", "index": 3})
        self.assertEqual(message["param"], "delay_mix")

    def test_clip_load_sends_tick_events(self) -> None:
        self.client.clip_load(
            2,
            [(0, True, 9, 36, 110), (PPQ, False, 9, 36, 0)],
            length_ticks=PPQ * 4,
        )
        self.assertTrue(self.engine.wait_for(1))
        message = self.engine.lines[0]
        self.assertEqual(message["slot"], 2)
        self.assertEqual(message["length_ticks"], PPQ * 4)
        self.assertEqual(len(message["events"]), 2)
        self.assertTrue(message["events"][0]["on"])

    def test_launch_defaults_to_bar_quantize(self) -> None:
        self.client.clip_launch(0)
        self.assertTrue(self.engine.wait_for(1))
        self.assertEqual(self.engine.lines[0]["quantize"], "bar")

    def test_status_round_trips(self) -> None:
        status = self.client.status(timeout=1.0)
        self.assertIsNotNone(status)
        self.assertEqual(status["position"], 4242)


class FragmentedReplyTest(unittest.TestCase):
    """Status must survive replies that arrive split across reads."""

    def setUp(self) -> None:
        self.dir = tempfile.mkdtemp()
        self.path = os.path.join(self.dir, "jambox.sock")
        self.engine = FakeEngine(self.path, fragment=True)
        self.client = JamboxClient(self.path)

    def tearDown(self) -> None:
        self.client.close()
        self.engine.close()

    def test_status_arrives_despite_split_packets(self) -> None:
        self.client.note_on(0, 60, 110)
        self.client.fx(JamboxClient.bus_target(), "reverb_mix", 0.3)
        status = self.client.status(timeout=2.0)
        self.assertIsNotNone(status)
        self.assertEqual(status["position"], 4242)


class ClientResilienceTest(unittest.TestCase):
    def test_calls_never_block_when_the_engine_is_missing(self) -> None:
        client = JamboxClient("/tmp/jambox-does-not-exist.sock")
        try:
            start = time.time()
            for note in range(200):
                client.note_on(0, note % 128, 100)
            elapsed = time.time() - start
            # The UI thread must stay responsive even with nothing listening.
            self.assertLess(elapsed, 0.5)
            self.assertFalse(client.connected)
        finally:
            client.close()

    def test_a_full_queue_drops_instead_of_blocking(self) -> None:
        client = JamboxClient("/tmp/jambox-does-not-exist.sock", auto_connect=False)
        try:
            for _ in range(4096):
                client.note_on(0, 60, 100)
            self.assertGreater(client.dropped, 0)
        finally:
            client.close()


if __name__ == "__main__":
    unittest.main()
