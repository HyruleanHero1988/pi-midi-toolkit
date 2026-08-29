#!/usr/bin/env python3
"""Kaoss pad rules — no display, no audio device.

Pins the Kaossilator-style scale map, HOLD, gate arp, and the original
Kaoss Pad factory MIDI (CC#12 / CC#13 / CC#92) so the kiosk stays honest.
"""

from __future__ import annotations

import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from pidi.kaoss import (
    KAOSS_CC_TOUCH,
    KAOSS_CC_X,
    KAOSS_CC_Y,
    PROGRAM_IDS,
    PROGRAM_IDS_ALL,
    ROOT_OCTAVE_MIDI,
    SCALE_ORDER,
    SCALE_ORDER_ALL,
    VIZ_STYLES,
    KaossPad,
    glow_radii,
    glow_step,
    grid_line_widths,
    hsv_to_rgb,
    midi_cc,
    note_at_x,
    note_cell_edges,
    note_grid_xs,
    note_index_at_x,
    note_name,
    pad_led_hex,
    program_hue,
    rgb_hex,
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

    def test_x_uses_equal_width_cells(self) -> None:
        notes = [48, 50, 52, 53]
        self.assertEqual(note_at_x(0.24, notes), 48)
        self.assertEqual(note_at_x(0.25, notes), 50)
        self.assertEqual(note_at_x(0.49, notes), 50)
        self.assertEqual(note_at_x(0.50, notes), 52)
        self.assertEqual(note_cell_edges(4), [0.0, 0.25, 0.5, 0.75, 1.0])

    def test_dorian_grid_lines_match_note_boundaries(self) -> None:
        notes = scale_notes("dorian", 0, root_midi=48, octaves=2)
        n = len(notes)
        self.assertEqual(n, 15)
        width = 800
        xs = note_grid_xs(n, width)
        self.assertEqual(len(xs), n + 1)
        self.assertEqual(xs[0], 0)
        self.assertEqual(xs[-1], width - 1)
        for i, midi in enumerate(notes):
            left = i / n
            right = (i + 1) / n
            self.assertEqual(note_at_x(left + 1e-6, notes), midi)
            self.assertEqual(note_at_x((left + right) / 2, notes), midi)
            if i + 1 < n:
                self.assertNotEqual(note_at_x(right + 1e-6, notes), midi)
            pixel = max(0, min(width - 1, xs[i]))
            x = pixel / width
            self.assertEqual(note_index_at_x(x + 1.0 / width, n), i)


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

    def test_hold_keeps_gate_arp_after_lift(self) -> None:
        self.pad.gate_id = "8th"
        self.pad.bpm = 120.0
        self.pad.touch(0.0, 1.0, now=0.0)
        self.assertIn("note_on", self.kinds(self.pad.tick(0.01)))
        self.pad.set_hold(True)
        self.assertEqual(self.pad.release(), [])
        self.assertTrue(self.pad.is_active())
        rest = self.pad.tick(0.20)
        self.assertIn("note_off", self.kinds(rest))
        self.assertTrue(self.pad.is_active())
        self.assertIn("note_on", self.kinds(self.pad.tick(0.26)))
        off = self.pad.set_hold(False)
        self.assertIn("note_off", self.kinds(off))
        self.assertFalse(self.pad.is_active())
        self.assertEqual(self.pad.tick(0.51), [])

    def test_hold_keeps_note_when_switching_to_filter(self) -> None:
        self.pad.touch(0.0, 0.8)
        self.pad.set_hold(True)
        self.pad.release()
        note = self.pad.sounding_note()
        self.assertIsNotNone(note)
        self.pad.set_program("filter")
        events = self.pad.reassert(now=1.0)
        self.assertEqual(self.pad.sounding_note(), note)
        self.assertTrue(self.pad.is_active())
        self.assertEqual(self.pad.program().id, "filter")
        self.assertIn("param", self.kinds(events))
        self.assertNotIn("note_off", self.kinds(events))

    def test_hold_and_gate_before_first_tick(self) -> None:
        self.pad.gate_id = "8th"
        self.pad.bpm = 120.0
        self.pad.touch(0.0, 1.0, now=0.0)
        self.pad.set_hold(True)
        self.pad.release()
        self.assertTrue(self.pad.is_active())
        self.assertIsNone(self.pad.sounding_note())
        self.assertIn("note_on", self.kinds(self.pad.tick(0.01)))

    def test_hold_and_gate_keep_repeating_on_filter(self) -> None:
        """FILTER must not kill a HOLD+GATE arp — X is tone, pitch stays latched."""
        self.pad.gate_id = "8th"
        self.pad.bpm = 120.0
        self.pad.touch(0.0, 1.0, now=0.0)
        self.assertIn("note_on", self.kinds(self.pad.tick(0.01)))
        note = self.pad.sounding_note()
        self.pad.set_hold(True)
        self.pad.release()
        self.pad.set_program("filter")
        self.pad.reassert(now=0.01)
        self.assertTrue(self.pad.is_active())
        rest = self.pad.tick(0.20)
        self.assertIn("note_off", self.kinds(rest))
        self.assertTrue(self.pad.is_active())
        again = self.pad.tick(0.26)
        self.assertIn("note_on", self.kinds(again))
        on = next(e for e in again if e.kind == "note_on")
        self.assertEqual(on.note, note)

    def test_turning_gate_on_while_held_starts_the_arp(self) -> None:
        self.pad.touch(0.0, 1.0, now=0.0)
        self.pad.set_hold(True)
        self.pad.release()
        self.assertIsNotNone(self.pad.sounding_note())
        self.pad.set_gate("8th", now=0.0)
        self.assertTrue(self.pad.is_active())
        rest = self.pad.tick(0.20)
        self.assertIn("note_off", self.kinds(rest))
        self.assertIn("note_on", self.kinds(self.pad.tick(0.26)))

    def test_y_writes_tone_param(self) -> None:
        events = self.pad.touch(0.3, 0.25)
        params = [e for e in events if e.kind == "param"]
        self.assertTrue(params)
        self.assertEqual(params[0].param, "tone")
        self.assertAlmostEqual(params[0].param_value, 0.25, places=3)

    def test_wah_plays_note_and_writes_tone_lfo_rate(self) -> None:
        self.pad.program_id = "wah"
        events = self.pad.touch(0.0, 0.75)
        self.assertIn("note_on", self.kinds(events))
        note = next(e for e in events if e.kind == "note_on")
        self.assertEqual(note.note, 48)
        params = [e for e in events if e.kind == "param"]
        self.assertTrue(params)
        self.assertEqual(params[0].param, "tone_lfo")
        self.assertAlmostEqual(params[0].param_value, 0.75, places=3)
        self.assertIn("wah", PROGRAM_IDS)

    def test_fx_program_has_no_notes(self) -> None:
        self.pad.program_id = "echo"
        events = self.pad.touch(0.2, 0.8)
        self.assertNotIn("note_on", self.kinds(events))
        params = {e.param: e.param_value for e in events if e.kind == "param"}
        self.assertAlmostEqual(params["delay_time"], 0.2, places=3)
        self.assertAlmostEqual(params["delay_mix"], 0.8, places=3)

    def test_swell_maps_attack_and_delay_mix(self) -> None:
        self.pad.program_id = "swell"
        events = self.pad.touch(0.15, 0.9)
        self.assertNotIn("note_on", self.kinds(events))
        params = {e.param: e.param_value for e in events if e.kind == "param"}
        self.assertAlmostEqual(params["attack"], 0.15, places=3)
        self.assertAlmostEqual(params["delay_mix"], 0.9, places=3)
        self.assertIn("swell", PROGRAM_IDS)

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
        self.pad.show_all = True
        self.pad.show_axis_labels = False
        self.pad.viz_style = "cells"
        snap = self.pad.snapshot()
        other = KaossPad()
        other.apply(snap)
        self.assertEqual(other.snapshot(), snap)
        self.assertEqual(other.viz_style, "cells")

    def test_octave_start_and_width(self) -> None:
        self.assertEqual(ROOT_OCTAVE_MIDI, (24, 36, 48, 60, 72))
        self.assertEqual(note_name(24), "C1")
        self.assertEqual(note_name(48), "C3")
        self.pad.set_root_midi(36)
        self.assertEqual(self.pad.root_midi, 36)
        self.assertEqual(self.pad.root_octave_midi(), 36)
        self.pad.set_octaves(4)
        self.assertEqual(self.pad.octaves, 4)
        self.pad.nudge_root_octave(1)
        self.assertEqual(self.pad.root_midi, 48)
        self.assertEqual(self.pad.root_octave_midi(), 48)


class ShowAllCatalogTest(unittest.TestCase):
    def test_factory_lists_are_larger_than_curated(self) -> None:
        self.assertGreater(len(SCALE_ORDER_ALL), len(SCALE_ORDER))
        self.assertGreater(len(PROGRAM_IDS_ALL), len(PROGRAM_IDS))
        self.assertIn("raga_bhairav", SCALE_ORDER_ALL)
        self.assertIn("pelog", SCALE_ORDER_ALL)
        self.assertIn("miyakobushi", SCALE_ORDER_ALL)
        self.assertNotIn("raga_bhairav", SCALE_ORDER)
        self.assertIn("bassline", SCALE_ORDER)
        self.assertIn("octave", PROGRAM_IDS_ALL)
        self.assertNotIn("octave", PROGRAM_IDS)

    def test_curated_cycle_skips_factory_only_scales(self) -> None:
        pad = KaossPad()
        pad.show_all = False
        seen = {pad.scale_id}
        for _ in range(len(SCALE_ORDER) + 2):
            seen.add(pad.cycle_scale())
        self.assertEqual(seen, set(SCALE_ORDER))
        self.assertNotIn("egyptian", seen)

    def test_show_all_cycle_reaches_every_factory_scale(self) -> None:
        pad = KaossPad()
        pad.toggle_show_all()
        seen = {pad.scale_id}
        for _ in range(len(SCALE_ORDER_ALL) + 2):
            seen.add(pad.cycle_scale())
        self.assertEqual(seen, set(SCALE_ORDER_ALL))

    def test_show_all_cycle_reaches_every_program(self) -> None:
        pad = KaossPad()
        pad.set_show_all(True)
        seen = {pad.program_id}
        for _ in range(len(PROGRAM_IDS_ALL) + 2):
            seen.add(pad.cycle_program().id)
        self.assertEqual(seen, set(PROGRAM_IDS_ALL))

    def test_scale_label_is_full_name_even_when_show_all(self) -> None:
        pad = KaossPad()
        pad.set_scale("mixolydian")
        pad.set_show_all(True)
        self.assertEqual(pad.scale_label(), "MIXOLYDIAN")
        self.assertNotEqual(pad.scale_label(), pad.scale().short)

    def test_set_scale_ignores_unknown_ids(self) -> None:
        pad = KaossPad()
        pad.set_scale("blues")
        self.assertEqual(pad.set_scale("not-a-scale"), "blues")
        self.assertEqual(pad.scale_label(), "BLUES")

    def test_set_program_and_hide_axis_labels(self) -> None:
        pad = KaossPad()
        self.assertEqual(pad.set_program("filter").id, "filter")
        self.assertEqual(pad.set_program("nope").id, "filter")
        pad.set_show_axis_labels(False)
        self.assertFalse(pad.show_axis_labels)
        self.assertFalse(pad.snapshot()["show_axis_labels"])
        pad.set_show_grid_lines(False)
        self.assertFalse(pad.show_grid_lines)
        self.assertFalse(pad.snapshot()["show_grid_lines"])

    def test_exotic_scale_still_quantizes(self) -> None:
        notes = scale_notes("egyptian", 0, root_midi=48, octaves=1)
        self.assertEqual(notes[0], 48)
        self.assertIn(50, notes)  # D
        self.assertNotIn(49, notes)  # C#

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

    def test_gate_flash_tracks_on_phase(self) -> None:
        pad = KaossPad()
        pad.gate_id = "8th"
        pad.bpm = 120.0
        pad.touch(0.0, 1.0, now=0.0)
        self.assertEqual(pad.gate_flash(), 0.0)
        pad.tick(0.01)
        self.assertEqual(pad.gate_flash(), 1.0)
        pad.tick(0.20)
        self.assertEqual(pad.gate_flash(), 0.0)


class LedFieldTest(unittest.TestCase):
    def test_hsv_primaries(self) -> None:
        self.assertEqual(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0))
        self.assertEqual(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), (0, 255, 0))
        self.assertEqual(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), (0, 0, 255))
        self.assertEqual(hsv_to_rgb(0.0, 0.0, 0.0), (0, 0, 0))
        self.assertEqual(rgb_hex((15, 32, 255)), "#0f20ff")

    def test_finger_lights_nearest_led(self) -> None:
        hot = pad_led_hex(6, 3, t=0.0, finger=(0.55, 0.5), hue_shift=0.93)
        cold = pad_led_hex(0, 0, t=0.0, finger=(0.55, 0.5), hue_shift=0.93)
        self.assertGreater(self._luma(hot), self._luma(cold) + 40)

    def test_ripple_and_hold_add_light(self) -> None:
        idle = pad_led_hex(4, 3, t=0.0, hue_shift=0.2)
        held = pad_led_hex(4, 3, t=0.0, hold=True, hue_shift=0.2)
        ring = pad_led_hex(
            4, 3, t=0.0, ripples=((0.0, 0.5, 0.39),), hue_shift=0.2
        )
        self.assertGreater(self._luma(held), self._luma(idle))
        self.assertGreater(self._luma(ring), self._luma(idle))

    def test_glow_eases_in_and_out(self) -> None:
        self.assertLess(glow_step(0.0, 1.0, 0.05), 0.45)
        self.assertGreater(glow_step(0.0, 1.0, 0.05), 0.2)
        self.assertGreater(glow_step(1.0, 0.0, 0.05), 0.7)
        self.assertEqual(glow_step(1.0, 1.0, 0.05), 1.0)

    def test_glow_size_follows_fade_not_position(self) -> None:
        span = 400.0
        full = glow_radii(span, 1.0)
        faded = glow_radii(span, 0.2)
        self.assertGreater(full[0], faded[0] * 2.5)
        self.assertGreater(full[0], 140.0)

    def test_programs_use_different_hues(self) -> None:
        self.assertNotEqual(program_hue("lead"), program_hue("filter"))
        self.assertGreater(KaossPad().viz_pulse(0.0), 0.3)

    def test_viz_style_cycles_and_rejects_junk(self) -> None:
        pad = KaossPad()
        self.assertEqual(pad.viz_style, "glow")
        self.assertEqual(pad.cycle_viz_style(), "cells")
        self.assertEqual(pad.cycle_viz_style(), "glow")
        pad.set_viz_style("nope")
        self.assertEqual(pad.viz_style, "glow")
        pad.set_viz_style("static")
        self.assertEqual(pad.viz_style, "cells")
        self.assertEqual(set(VIZ_STYLES), {"glow", "cells"})

    def test_grid_width_defaults_thicker_and_clamps(self) -> None:
        pad = KaossPad()
        self.assertEqual(pad.grid_width, 2)
        self.assertEqual(grid_line_widths(2), (2, 3))
        self.assertEqual(pad.nudge_grid_width(1), 3)
        self.assertEqual(pad.nudge_grid_width(9), 5)
        self.assertEqual(pad.nudge_grid_width(-20), 1)
        pad.apply({"grid_width": "nope"})
        self.assertEqual(pad.grid_width, 2)
        pad.apply({"grid_width": 4})
        self.assertEqual(pad.snapshot()["grid_width"], 4)

    def test_cell_grid_is_fixed_12x7(self) -> None:
        pad = KaossPad()
        pad.program_id = "lead"
        pad.scale_id = "ionian"
        pad.octaves = 2
        self.assertEqual(pad.led_grid_size(), (12, 7))
        pad.program_id = "filter"
        self.assertEqual(pad.led_grid_size(), (12, 7))

    def test_morph_header_shows_percent(self) -> None:
        pad = KaossPad()
        pad.program_id = "morph"
        line = pad.header_line(morph=("saw", "organ", 0.42))
        self.assertIn("42%", line)
        self.assertIn("MORPH", line)

    def test_lead_header_shows_tone_percent(self) -> None:
        pad = KaossPad()
        pad.program_id = "lead"
        line = pad.header_line(tone=0.12)
        self.assertIn("12%", line)
        self.assertIn("LEAD", line)

    @staticmethod
    def _luma(color: str) -> int:
        raw = color.lstrip("#")
        return int(raw[0:2], 16) + int(raw[2:4], 16) + int(raw[4:6], 16)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
