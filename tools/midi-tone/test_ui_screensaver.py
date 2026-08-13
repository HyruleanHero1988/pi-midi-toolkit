#!/usr/bin/env python3
"""Headless smoke test for TFT burn-in / idle blanking.

Builds the real Tk app with stub audio/MIDI (same harness as test_ui_seq),
then blanks the panel after touch-idle, wakes it from a tap, and checks that
MIDI notes do not wake it (a long jam with no settings tweaks still sleeps).
"""

from __future__ import annotations

import pathlib
import sys
import tempfile
import time
import unittest


class FakeStream:
    def __init__(self, **kwargs) -> None:
        self.kwargs = kwargs
        self.callback = kwargs.get("callback")
        self.samplerate = kwargs.get("samplerate", 44100)
        self.channels = kwargs.get("channels", 1)
        self.blocksize = kwargs.get("blocksize", 1024)
        self.latency = 0.05
        self.active = False

    def start(self) -> None:
        self.active = True

    def stop(self) -> None:
        self.active = False

    def close(self) -> None:
        self.active = False


class FakePort:
    def __init__(self, name: str = "fake MPK") -> None:
        self.name = name
        self.sent: list = []

    def iter_pending(self):
        return iter(())

    def send(self, msg) -> None:
        self.sent.append(msg)

    def close(self) -> None:
        pass


def display_available() -> bool:
    import os

    if not os.environ.get("DISPLAY"):
        return False
    try:
        import tkinter

        root = tkinter.Tk()
        root.destroy()
        return True
    except Exception:
        return False


@unittest.skipUnless(display_available(), "no X display (try xvfb-run)")
class ScreensaverUiTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        import sounddevice as sd
        import mido

        cls._tmp = tempfile.TemporaryDirectory()
        tmp = pathlib.Path(cls._tmp.name)

        sd.OutputStream = FakeStream  # type: ignore[assignment]
        sd.query_devices = lambda *a, **k: (  # type: ignore[assignment]
            {"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}
            if a
            else [{"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}]
        )
        mido.get_input_names = lambda: ["fake MPK"]  # type: ignore[assignment]
        mido.get_output_names = lambda: []  # type: ignore[assignment]
        mido.open_input = lambda name: FakePort(name)  # type: ignore[assignment]
        mido.open_output = lambda name: FakePort(name)  # type: ignore[assignment]

        import midi_tone

        midi_tone.SETTINGS_PATH = tmp / "settings.json"
        midi_tone.PRESETS_DIR = tmp / "presets"
        midi_tone.SONGS_DIR = tmp / "songs"
        midi_tone.PHRASES_DIR = tmp / "phrases"
        midi_tone.DEMO_SONGS_DIR = tmp / "demo-songs"
        midi_tone.SONG_SEED_MARKER = tmp / "songs" / ".seeded"
        midi_tone.USER_WAVETABLES_DIR = tmp / "user-wavetables"
        cls.midi_tone = midi_tone
        cls.app = midi_tone.MidiToneApp(
            port_filter="",
            list_only=False,
            max_voices=4,
            waves_dir=midi_tone.DEFAULT_WAVETABLE_DIR,
        )

    @classmethod
    def tearDownClass(cls) -> None:
        try:
            cls.app._stop.set()
            cls.app._hide_screensaver()
            cls.app._seq.stop()
            cls.app.engine.stop()
            cls.app.root.destroy()
        except Exception:
            pass
        cls._tmp.cleanup()

    def pump(self, seconds: float = 0.05) -> None:
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.app.root.update()
            time.sleep(0.005)

    def setUp(self) -> None:
        self.app._hide_screensaver()
        self.app._idle.timeout_sec = 0.0
        self.app._idle.poke()

    def tearDown(self) -> None:
        try:
            self.app._hide_screensaver()
        except Exception:
            pass
        self.app._idle.timeout_sec = 0.0
        self.app._idle.poke()

    def test_idle_tick_covers_the_panel(self) -> None:
        app = self.app
        app._idle.timeout_sec = 1.0
        app._idle._last = time.monotonic() - 5.0
        app._screensaver_tick()
        self.pump()
        self.assertTrue(app._idle.active)
        self.assertIsNotNone(app._saver_canvas)

    def test_tap_wakes_but_midi_does_not(self) -> None:
        import mido

        app = self.app
        app._show_screensaver(force=True)
        self.pump()
        self.assertIsNotNone(app._saver_canvas)

        app._handle_midi(mido.Message("note_on", channel=0, note=60, velocity=100))
        app._handle_midi(mido.Message("control_change", channel=0, control=70, value=64))
        self.pump()
        self.assertIsNotNone(app._saver_canvas, "MIDI must not wake the TFT")
        self.assertTrue(app._idle.active)
        self.assertIn((0, 60), app._active_notes, "engine still plays while blanked")

        app._on_screensaver_tap()
        self.pump()
        self.assertIsNone(app._saver_canvas)
        self.assertFalse(app._idle.active)

    def test_global_pointer_also_wakes(self) -> None:
        app = self.app
        app._show_screensaver(force=True)
        self.pump()
        self.assertIsNotNone(app._saver_canvas)
        app._on_pointer_activity()
        self.pump()
        self.assertIsNone(app._saver_canvas)
        self.assertFalse(app._idle.active)

    def test_midi_does_not_postpone_blanking(self) -> None:
        import mido

        app = self.app
        app._idle.timeout_sec = 1.0
        app._idle._last = time.monotonic() - 5.0
        app._handle_midi(mido.Message("note_on", channel=0, note=64, velocity=100))
        self.assertTrue(app._idle.due(), "a jam without touch must still time out")
        app._screensaver_tick()
        self.pump()
        self.assertTrue(app._idle.active)
        self.assertIsNotNone(app._saver_canvas)

    def test_pixel_shift_moves_the_nav_chrome(self) -> None:
        app = self.app
        app._shift_started = time.monotonic() - 50.0
        app._shift_xy = (None, None)
        app._apply_pixel_shift()
        info = app._nav.pack_info()
        padx = str(info.get("padx", "0"))
        self.assertNotEqual(padx, "0", "awake chrome should inset/shift off dead-center")
        self.assertEqual(app._shift_xy, (2, 0))

    def test_timeout_cycle_persists_in_session(self) -> None:
        app = self.app
        app._idle.timeout_sec = 180.0
        app._cycle_screensaver_timeout()
        self.assertEqual(app._idle.timeout_sec, 600.0)
        snap = app._session_dict()
        self.assertEqual(snap["screensaver_sec"], 600.0)
        app._idle.timeout_sec = 180.0


if __name__ == "__main__":
    sys.exit(unittest.main())
