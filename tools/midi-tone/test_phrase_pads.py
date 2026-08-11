#!/usr/bin/env python3
"""
Phrase-pad recording rules — no display, no audio device.

Mostly here to pin the dead-air trim: a pad take and a sequencer take must be
cut the same way, so a slow finger on the pad square never bakes silence into
a looping clip.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile
import time
import unittest
from unittest import mock


class FakeEngine:
    """Enough of SineEngine for PhrasePadBank to record and stop."""

    MAX_LOCKED_TIMBRES = 2

    def note_on(self, *a, **k) -> None:
        pass

    def note_off(self, *a, **k) -> None:
        pass

    def snapshot_morph(self):
        return ("sine", "saw", 0.0)

    def bake_morph_table(self, *a, **k):
        return None


def load_midi_tone():
    """Import midi_tone with the audio + MIDI backends stubbed out."""
    import sounddevice as sd
    import mido

    sd.OutputStream = object  # type: ignore[assignment]
    mido.get_input_names = lambda: []  # type: ignore[assignment]
    mido.get_output_names = lambda: []  # type: ignore[assignment]
    import midi_tone

    return midi_tone


class PhrasePadTrimTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.midi_tone = load_midi_tone()

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = pathlib.Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def make_bank(self):
        return self.midi_tone.PhrasePadBank(FakeEngine(), lambda _m: None, self.dir)

    def test_pad_take_is_trimmed_like_a_sequencer_take(self) -> None:
        bank = self.make_bank()
        clock = [1000.0]
        with mock.patch("midi_tone.time.monotonic", lambda: clock[0]):
            bank.arm_record(0)
            clock[0] += 1.2  # hunting for the first hit
            for _ in range(3):
                bank.record_note(True, 9, 36, 100)
                clock[0] += 0.02
                bank.record_note(False, 9, 36, 0)
                clock[0] += 0.28
            clock[0] += 2.5  # reaching for STOP
            bank.stop_record()
        cell = bank.cell(0)
        self.assertAlmostEqual(min(e.t for e in cell.events), 0.0, places=6)
        # Onsets are 0.30s apart, so the clip ends one gap after the last hit —
        # not 1.2s of pre-roll or 2.5s of stop lag later.
        self.assertAlmostEqual(cell.length, 0.90, places=2)

    def test_trim_helper_is_shared_with_the_sequencer(self) -> None:
        import sequencer

        self.assertIs(self.midi_tone.trim_loop_take, sequencer.trim_loop_take)
        self.assertIs(self.midi_tone.LoopEvent, sequencer.LoopEvent)

    def test_empty_pad_take_stays_empty(self) -> None:
        bank = self.make_bank()
        bank.arm_record(1)
        time.sleep(0.01)
        bank.stop_record()
        self.assertTrue(bank.cell(1).is_empty())


if __name__ == "__main__":
    sys.exit(unittest.main())
