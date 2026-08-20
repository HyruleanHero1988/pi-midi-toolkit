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

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


class FakeEngine:
    """Enough of SineEngine for PhrasePadBank to record, launch and stop."""

    MAX_LOCKED_TIMBRES = 2

    def __init__(self, level: float = 1.0, vib=(0.0, 5.0, 0.0)) -> None:
        self._level = float(level)
        self._vib = tuple(float(v) for v in vib)
        self.ons: list[tuple[int, int, int]] = []
        self.vibs: list = []

    def note_on(self, channel, note, velocity, **k) -> None:
        self.ons.append((channel, note, velocity))
        self.vibs.append(k.get("vib"))

    def note_off(self, *a, **k) -> None:
        pass

    def level(self) -> float:
        return self._level

    def vib_state(self):
        return self._vib

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

    def make_bank(self, engine: FakeEngine | None = None):
        return self.midi_tone.PhrasePadBank(
            engine or FakeEngine(), lambda _m: None, self.dir
        )

    def test_pad_take_is_trimmed_like_a_sequencer_take(self) -> None:
        bank = self.make_bank()
        clock = [1000.0]
        with mock.patch("pidi.domain.phrases.time.monotonic", lambda: clock[0]):
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
        from pidi import sequencer

        self.assertIs(self.midi_tone.trim_loop_take, sequencer.trim_loop_take)
        self.assertIs(self.midi_tone.LoopEvent, sequencer.LoopEvent)

    def test_lock_bakes_the_master_level_as_the_pad_trim(self) -> None:
        engine = FakeEngine(level=0.6)
        bank = self.make_bank(engine)
        bank.arm_record(0)
        bank.record_note(True, 0, 60, 100)
        bank.record_note(False, 0, 60, 0)
        bank.stop_record()

        bank.lock_voice_from_engine(0)
        self.assertAlmostEqual(bank.cell(0).gain, 0.6, places=6)
        # Master level moving later must not drag the locked pad with it
        engine._level = 1.0
        self.assertAlmostEqual(bank.cell(0).gain, 0.6, places=6)

        bank.set_voice_follow(0)
        self.assertAlmostEqual(bank.cell(0).gain, 1.0, places=6)

    def test_trim_scales_played_velocity(self) -> None:
        engine = FakeEngine()
        bank = self.make_bank(engine)
        bank.arm_record(0)
        bank.record_note(True, 0, 60, 100)
        bank.record_note(False, 0, 60, 0)
        bank.stop_record()
        bank.set_gain(0, 0.5)

        bank._emit_phrase_note(
            on=True,
            src_channel=0,
            note=60,
            velocity=100,
            out_channel=self.midi_tone.PHRASE_OUT_AS_RECORDED,
            local_synth=True,
            out_mode="local",
            timbre=None,
            fx_name=None,
            idx=0,
        )
        self.assertEqual(engine.ons[-1], (0, 60, 50))

    def test_trim_survives_save_and_reload(self) -> None:
        bank = self.make_bank()
        bank.arm_record(2)
        bank.record_note(True, 9, 36, 120)
        bank.record_note(False, 9, 36, 0)
        bank.stop_record()
        bank.nudge_gain(2, -self.midi_tone.PHRASE_GAIN_STEP)
        expected = bank.cell(2).gain
        self.assertLess(expected, 1.0)

        reloaded = self.make_bank()
        self.assertAlmostEqual(reloaded.cell(2).gain, expected, places=6)

    def test_trim_is_clamped_and_never_silences_a_hit(self) -> None:
        bank = self.make_bank()
        self.assertAlmostEqual(bank.set_gain(0, 99.0), self.midi_tone.PHRASE_GAIN_MAX, places=6)
        self.assertAlmostEqual(bank.set_gain(0, -3.0), self.midi_tone.PHRASE_GAIN_MIN, places=6)
        self.assertEqual(self.midi_tone.scale_velocity(1, 0.1), 1)
        self.assertEqual(self.midi_tone.scale_velocity(127, 2.0), 127)

    def test_recording_bakes_the_vibrato_it_was_played_with(self) -> None:
        engine = FakeEngine(vib=(0.8, 4.5, 1.0))
        bank = self.make_bank(engine)
        bank.arm_record(0)
        bank.record_note(True, 0, 60, 100)
        bank.record_note(False, 0, 60, 0)
        bank.stop_record()

        cell = bank.cell(0)
        self.assertTrue(cell.vib_baked)
        self.assertEqual(cell.vib_tuple(), (0.8, 4.5, 1.0))

        # Changing the live rig afterwards must not reach the recorded phrase
        engine._vib = (0.0, 5.0, 0.0)
        self.assertEqual(bank.cell(0).vib_tuple(), (0.8, 4.5, 1.0))

    def test_baked_vibrato_is_handed_to_the_synth_on_playback(self) -> None:
        engine = FakeEngine(vib=(0.6, 6.0, 1.0))
        bank = self.make_bank(engine)
        bank.arm_record(0)
        bank.record_note(True, 0, 60, 100)
        bank.record_note(False, 0, 60, 0)
        bank.stop_record()

        def emit_one() -> None:
            bank._emit_phrase_note(
                on=True,
                src_channel=0,
                note=60,
                velocity=100,
                out_channel=self.midi_tone.PHRASE_OUT_AS_RECORDED,
                local_synth=True,
                out_mode="local",
                timbre=None,
                fx_name=None,
                idx=0,
            )

        emit_one()
        self.assertEqual(engine.vibs[-1], (0.6, 6.0, 1.0))

        # VIB live hands the pad back to whatever the rig is doing
        bank.set_vib_live(0)
        emit_one()
        self.assertIsNone(engine.vibs[-1])

        bank.toggle_vib_baked(0)
        emit_one()
        self.assertEqual(engine.vibs[-1], (0.6, 6.0, 1.0))

    def test_baked_vibrato_survives_save_and_reload(self) -> None:
        engine = FakeEngine(vib=(1.2, 3.5, 1.0))
        bank = self.make_bank(engine)
        bank.arm_record(3)
        bank.record_note(True, 0, 64, 100)
        bank.record_note(False, 0, 64, 0)
        bank.stop_record()

        reloaded = self.make_bank()
        self.assertEqual(reloaded.cell(3).vib_tuple(), (1.2, 3.5, 1.0))

    def test_lock_recaptures_vibrato_with_the_voice(self) -> None:
        engine = FakeEngine(vib=(0.0, 5.0, 0.0))
        bank = self.make_bank(engine)
        bank.arm_record(1)
        bank.record_note(True, 0, 60, 100)
        bank.record_note(False, 0, 60, 0)
        bank.stop_record()
        self.assertEqual(bank.cell(1).vib_tuple(), (0.0, 5.0, 0.0))

        engine._vib = (1.5, 7.0, 1.0)
        bank.lock_voice_from_engine(1)
        self.assertEqual(bank.cell(1).vib_tuple(), (1.5, 7.0, 1.0))

    def test_empty_pad_take_stays_empty(self) -> None:
        bank = self.make_bank()
        bank.arm_record(1)
        time.sleep(0.01)
        bank.stop_record()
        self.assertTrue(bank.cell(1).is_empty())

    def test_load_from_events_drops_a_sequence_onto_a_pad(self) -> None:
        mt = self.midi_tone
        bank = self.make_bank()
        events = [
            mt.LoopEvent(t=0.00, on=True, channel=9, note=36, velocity=100),
            mt.LoopEvent(t=0.05, on=False, channel=9, note=36, velocity=0),
            mt.LoopEvent(t=0.25, on=True, channel=0, note=60, velocity=90),
            mt.LoopEvent(t=0.40, on=False, channel=0, note=60, velocity=0),
        ]
        self.assertTrue(bank.load_from_events(3, events, 0.50))
        cell = bank.cell(3)
        self.assertFalse(cell.is_empty())
        self.assertTrue(cell.is_loop())
        self.assertAlmostEqual(cell.length, 0.50, places=3)
        self.assertEqual(len(cell.events), 4)
        self.assertFalse(bank.load_from_events(0, [], 1.0))
        self.assertTrue(bank.cell(0).is_empty())

    def test_drum_pad_handle_launches_a_filled_cell(self) -> None:
        mt = self.midi_tone
        engine = FakeEngine()
        bank = self.make_bank(engine)
        events = [
            mt.LoopEvent(t=0.00, on=True, channel=0, note=64, velocity=100),
            mt.LoopEvent(t=0.05, on=False, channel=0, note=64, velocity=0),
        ]
        bank.load_from_events(0, events, 0.10, trigger_mode=mt.PHRASE_TRIG_ONESHOT)
        action = bank.handle_pad(0, from_touch=False, allow_record=False)
        self.assertEqual(action, "launch")
        deadline = time.monotonic() + 0.5
        while time.monotonic() < deadline and not engine.ons:
            time.sleep(0.01)
        self.assertEqual(engine.ons[0], (0, 64, 100))
        bank.stop_all()

    def test_drum_pad_still_launches_while_another_cell_records(self) -> None:
        """Touch could fire a filled clip during REC; MPK pads used to be ignored."""
        mt = self.midi_tone
        engine = FakeEngine()
        bank = self.make_bank(engine)
        events = [
            mt.LoopEvent(t=0.00, on=True, channel=0, note=64, velocity=100),
            mt.LoopEvent(t=0.05, on=False, channel=0, note=64, velocity=0),
        ]
        bank.load_from_events(0, events, 0.10, trigger_mode=mt.PHRASE_TRIG_ONESHOT)
        bank.arm_record(1)
        self.assertTrue(bank.is_recording())
        self.assertEqual(bank.handle_pad(0, from_touch=False), "launch")
        self.assertEqual(bank.handle_pad(1, from_touch=False), "ignore")
        self.assertEqual(bank.handle_pad(2, from_touch=False), "ignore")
        deadline = time.monotonic() + 0.5
        while time.monotonic() < deadline and not engine.ons:
            time.sleep(0.01)
        self.assertEqual(engine.ons[0], (0, 64, 100))
        bank.stop_all()


class PhrasePadMappingTest(unittest.TestCase):
    def test_screen_top_left_a1_is_mpk_hardware_pad_5(self) -> None:
        from pidi.domain.phrases import (
            mpk_note_for_phrase_cell,
            phrase_cell_for_note,
            phrase_pad_label,
        )

        self.assertEqual(phrase_pad_label(0), "A1")
        self.assertEqual(phrase_cell_for_note(40), 0)  # MPK pad 5 (top-left)
        self.assertEqual(phrase_cell_for_note(36), 4)  # MPK pad 1 (bottom-left) → A5
        self.assertEqual(mpk_note_for_phrase_cell(0), 40)
        self.assertEqual(mpk_note_for_phrase_cell(4), 36)
        self.assertEqual(phrase_cell_for_note(48), 8)  # Bank B pad 5 → B1
        self.assertEqual(phrase_cell_for_note(44), 12)  # Bank B pad 1 → B5
        self.assertEqual(mpk_note_for_phrase_cell(8), 48)


class PhrasePadTileColorTest(unittest.TestCase):
    def test_green_means_playing_not_loop_mode(self) -> None:
        mt = load_midi_tone()
        idle_loop = mt.phrase_pad_tile_color(empty=False, loop=True)
        idle_shot = mt.phrase_pad_tile_color(empty=False, loop=False)
        playing_loop = mt.phrase_pad_tile_color(empty=False, loop=True, playing=True)
        playing_shot = mt.phrase_pad_tile_color(empty=False, loop=False, playing=True)
        self.assertEqual(idle_loop, idle_shot)
        self.assertEqual(idle_loop, mt.PHRASE_TILE_IDLE)
        self.assertEqual(playing_loop, playing_shot)
        self.assertEqual(playing_loop, mt.PHRASE_TILE_PLAYING)
        self.assertNotEqual(idle_loop, playing_loop)

    def test_selected_idle_is_teal_playing_stays_green(self) -> None:
        mt = load_midi_tone()
        selected = mt.phrase_pad_tile_color(empty=False, selected=True)
        playing_sel = mt.phrase_pad_tile_color(
            empty=False, selected=True, playing=True
        )
        self.assertEqual(selected, mt.PHRASE_TILE_SELECTED)
        self.assertEqual(playing_sel, mt.PHRASE_TILE_PLAYING)


if __name__ == "__main__":
    sys.exit(unittest.main())
