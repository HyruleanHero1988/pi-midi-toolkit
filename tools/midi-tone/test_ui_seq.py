#!/usr/bin/env python3
"""
Headless smoke test for the SEQ screen.

Builds the real Tk app with a stub audio device and a stub MIDI port, then
drives the sequencer the way a player does: record a backbone, overdub, keep,
undo. Catches the wiring mistakes unit tests can't see (missing widgets,
stale attribute names, handlers that raise).

Needs an X display; skipped automatically when there isn't one (`xvfb-run -a
python3 -m unittest test_ui_seq` gives you one).
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
class SeqScreenTest(unittest.TestCase):
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

        # Keep the test out of the repo working tree
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

    def play(self, note: int, *, channel: int = 9, velocity: int = 100) -> None:
        import mido

        self.app._handle_midi(
            mido.Message("note_on", channel=channel, note=note, velocity=velocity)
        )
        self.app._handle_midi(
            mido.Message("note_off", channel=channel, note=note, velocity=0)
        )

    def test_seq_screen_records_overdubs_and_undoes(self) -> None:
        app = self.app
        app._switch_mode("seq")
        self.pump()
        self.assertEqual(app._mode, "seq")

        # Backbone take: pads on channel 10
        app._seq_toggle_record()
        self.assertEqual(app._seq.state(), self.midi_tone.SEQ_REC_BACKBONE)
        for note in (36, 38, 36, 38):
            self.play(note)
            self.pump(0.06)
        app._seq_toggle_record()
        self.pump(0.1)
        status = app._seq.status()
        self.assertEqual(int(status["layers"]), 1)
        self.assertGreater(float(status["length"]), 0.0)
        self.assertTrue(app._seq.is_playing(), "backbone should start looping")

        # Overdub a melody line on the keys
        app._seq_toggle_record()
        self.assertEqual(app._seq.state(), self.midi_tone.SEQ_OVERDUB)
        for note in (60, 64):
            self.play(note, channel=0)
            self.pump(0.05)
        app._seq_toggle_record()
        self.pump(0.05)
        self.assertGreater(int(app._seq.status()["pending"]), 0)

        app._seq_keep()
        self.pump(0.05)
        self.assertEqual(int(app._seq.status()["layers"]), 2)
        self.assertIn("L1", app._seq_layer_var.get())

        app._seq_undo()
        self.pump(0.05)
        self.assertEqual(int(app._seq.status()["layers"]), 1)

        # Sequence shape controls
        app._seq_double()
        self.assertEqual(int(app._seq.status()["cycles"]), 2)
        app._seq_halve()
        self.assertEqual(int(app._seq.status()["cycles"]), 1)
        app._seq_toggle_extend()
        self.assertTrue(app._seq.extend_mode())
        app._seq_toggle_extend()

        # Drop path: a take you decide against leaves the backbone alone
        app._seq_toggle_record()
        self.play(46)
        self.pump(0.05)
        app._seq_toggle_record()
        app._seq_drop()
        self.pump(0.05)
        self.assertEqual(int(app._seq.status()["layers"]), 1)

        app._seq_stop()
        self.pump(0.05)
        self.assertFalse(app._seq.is_playing())
        app._seq_clear()
        self.pump(0.05)
        self.assertEqual(app._seq.state(), self.midi_tone.SEQ_EMPTY)

    def test_panic_and_song_export_use_the_sequence(self) -> None:
        app = self.app
        app._switch_mode("seq")
        app._seq_toggle_record()
        for note in (36, 38):
            self.play(note)
            self.pump(0.06)
        app._seq_toggle_record()
        self.pump(0.05)

        app._song_save_from_seq()
        self.pump(0.05)
        songs = sorted(p.name for p in self.midi_tone.SONGS_DIR.glob("*.mid"))
        self.assertTrue(songs, "SAVE SEQ should write a .mid")

        app._panic()
        self.pump(0.05)
        self.assertFalse(app._seq.is_playing())
        app._seq_clear()

    def test_seq_to_pad_then_drum_pad_launches_the_clip(self) -> None:
        """SEQ → PAD (lost when LOOPER became SEQ) plus MPK pad launch."""
        app = self.app
        mt = self.midi_tone
        app._switch_mode("seq")
        self.pump()
        app._seq_toggle_record()
        for note in (36, 38):
            self.play(note)
            self.pump(0.06)
        app._seq_toggle_record()
        self.pump(0.05)
        self.assertGreater(float(app._seq.status()["length"]), 0.0)

        app._seq_assign_to_pad()
        self.pump()
        self.assertTrue(app._seq_to_pad_armed)
        self.assertEqual(app._mode, "pads")

        # MPK pad A1 (note 36, ch10) drops the sequence onto that cell
        self.play(36)
        self.pump(0.15)
        cell = app._phrases.cell(0)
        self.assertFalse(cell.is_empty())
        self.assertTrue(cell.is_loop())
        self.assertFalse(app._seq_to_pad_armed)
        self.assertEqual(app._pads_view, "play")

        # Same drum pad now launches the clip instead of playing a one-shot
        self.play(36)
        self.pump(0.12)
        self.assertTrue(app._phrases.is_playing(0))
        app._phrase_stop_all()
        app._phrases.clear_cell(0)
        app._seq_clear()
        self.pump()
        self.assertEqual(mt.phrase_cell_for_note(36), 0)


if __name__ == "__main__":
    sys.exit(unittest.main())
