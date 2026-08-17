#!/usr/bin/env python3
"""Kaoss pad rules — no display, no audio device.

Pins the Kaossilator-style scale map, HOLD, gate arp, and the original
Kaoss Pad factory MIDI (CC#12 / CC#13 / CC#92) so the kiosk stays honest.
"""

from __future__ import annotations

import unittest

from kaoss import (
    KAOSS_CC_TOUCH,
    KAOSS_CC_X,
    KAOSS_CC_Y,
    KaossPad,
    midi_cc,
    note_at_x,
    scale_notes,
)


class ScaleMapTest(unittest.TestCase):
    def test_c_major_two_octaves_from_c3(self) -> None:
        notes = scale_notes("ionian", 0, root_midi=48, octaves=2)
        self.assertEqual(notes[0], 48)
        self.assertEqual(notes[-1], 72)
        # C D E F G A B C D E F G A B C — 15 notes
        self.assertEqual(len(notes), 15)
        self.assertNotIn(49, notes)  # C#

    def test_a_minor_pent_starts_on_a(self) -> None:
        notes = scale_notes("minor_pent", 9, root_midi=45, octaves=1)
        self.assertEqual(notes[0], 45)  # A2
        self.assertIn(48, notes)  # C
        self.assertNotIn(47, notes)  # B

    def test_x_maps_endpoints(self) -> None:
        notes = [48, 50, 52, 53]
        self.assertEqual(note_at_x(0.0, notes), 48)
        self.assertEqual(note_at_x(1.0, notes), 53)
        self.assertEqual(note_at_x(0.5, notes), 52)


class PadPlayTest(unittest.TestCase):
    def setUp(self) -> None:
        self.pad = KaossPad()
        self.pad.program_id = "lead"
        self.pad.scale_id = "ionian"
        self.pad.key = 0
        self.pad.octaves = 2
        self.pad.root_midi = 48

    def kinds(self, events):
        return [e.kind for e in events]

    def test_touch_plays_scale_note_and_factory_ccs(self) -> None:
        events = self.pad.touch(0.0, 1.0)
        kinds = self.kinds(events)
        self.assertIn("note_on", kinds)
        self.assertIn("touch", kinds)
        self.assertIn("cc", kinds)
        note = next(e for e in events if e.kind == "note_on")
        self.assertEqual(note.note, 48)
        self.assertGreaterEqual(note.velocity, 120)
        touch = next(e for e in events if e.kind == "touch")
        self.assertEqual(touch.control, KAOSS_CC_TOUCH)
        self.assertEqual(touch.value, 127)
        ccs = {e.control: e.value for e in events if e.kind == "cc"}
        self.assertEqual(ccs[KAOSS_CC_X], 0)
        self.assertEqual(ccs[KAOSS_CC_Y], 127)

    def test_slide_changes_note_legato(self) -> None:
        self.pad.touch(0.0, 0.5)
        events = self.pad.move(1.0, 0.5)
        self.assertEqual(self.kinds(events).count("note_off"), 1)
        self.assertEqual(self.kinds(events).count("note_on"), 1)
        self.assertEqual(self.pad.sounding_note(), 72)

    def test_release_sends_note_off_and_touch_up(self) -> None:
        self.pad.touch(0.2, 0.5)
        events = self.pad.release()
        kinds = self.kinds(events)
        self.assertIn("note_off", kinds)
        touch = next(e for e in events if e.kind == "touch")
        self.assertEqual(touch.value, 0)
        self.assertIsNone(self.pad.sounding_note())

    def test_hold_keeps_note_after_lift(self) -> None:
        self.pad.touch(0.0, 0.8)
        self.pad.set_hold(True)
        events = self.pad.release()
        self.assertEqual(events, [])
        self.assertEqual(self.pad.sounding_note(), 48)
        self.assertTrue(self.pad.is_active())
        off = self.pad.set_hold(False)
        self.assertIn("note_off", self.kinds(off))
        self.assertFalse(self.pad.is_active())

    def test_y_writes_tone_param(self) -> None:
        events = self.pad.touch(0.3, 0.25)
        params = [e for e in events if e.kind == "param"]
        self.assertTrue(params)
        self.assertEqual(params[0].param, "tone")
        self.assertAlmostEqual(params[0].param_value, 0.25, places=3)

    def test_fx_program_has_no_notes(self) -> None:
        self.pad.program_id = "echo"
        events = self.pad.touch(0.2, 0.8)
        self.assertNotIn("note_on", self.kinds(events))
        params = {e.param: e.param_value for e in events if e.kind == "param"}
        self.assertAlmostEqual(params["delay_time"], 0.2, places=3)
        self.assertAlmostEqual(params["delay_mix"], 0.8, places=3)

    def test_retune_after_key_change(self) -> None:
        self.pad.touch(0.0, 0.5)
        self.assertEqual(self.pad.sounding_note(), 48)
        self.pad.cycle_key(2)  # C → D — left edge leaves C major
        events = self.pad.retune()
        self.assertIn("note_off", self.kinds(events))
        self.assertNotEqual(self.pad.sounding_note(), 48)
        self.assertIn(self.pad.sounding_note(), self.pad.notes())

    def test_settings_roundtrip(self) -> None:
        self.pad.program_id = "space"
        self.pad.scale_id = "blues"
        self.pad.key = 4
        self.pad.octaves = 3
        self.pad.gate_id = "16th"
        self.pad.bpm = 140
        self.pad.out_mode = "both"
        self.pad.channel = 2
        snap = self.pad.snapshot()
        other = KaossPad()
        other.apply(snap)
        self.assertEqual(other.snapshot(), snap)

    def test_midi_cc_quantizes(self) -> None:
        self.assertEqual(midi_cc(0.0), 0)
        self.assertEqual(midi_cc(1.0), 127)
        self.assertEqual(midi_cc(0.5), 64)


class GateArpTest(unittest.TestCase):
    def test_gate_retriggers_on_period(self) -> None:
        pad = KaossPad()
        pad.gate_id = "8th"
        pad.bpm = 120.0  # 8th = 0.25s
        # First touch does not fire a note when gated — tick does
        down = pad.touch(0.0, 1.0, now=0.0)
        self.assertNotIn("note_on", [e.kind for e in down])
        attack = pad.tick(0.01)
        self.assertIn("note_on", [e.kind for e in attack])
        # Mid-duty: still on, no extra note
        mid = pad.tick(0.10)
        self.assertEqual(mid, [])
        # Off phase
        rest = pad.tick(0.20)
        self.assertIn("note_off", [e.kind for e in rest])
        # Next cycle
        again = pad.tick(0.26)
        self.assertIn("note_on", [e.kind for e in again])

    def test_gate_off_while_held_keeps_sounding(self) -> None:
        pad = KaossPad()
        pad.gate_id = "off"
        pad.touch(0.0, 1.0, now=0.0)
        self.assertEqual(pad.tick(1.0), [])
        self.assertIsNotNone(pad.sounding_note())


if __name__ == "__main__":
    raise SystemExit(unittest.main())
