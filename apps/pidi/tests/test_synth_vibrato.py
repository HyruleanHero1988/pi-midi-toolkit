#!/usr/bin/env python3
"""
Vibrato rules for the soft-synth — renders blocks directly, no audio device.

The point of these: the screen control has to be audible on its own. Vibrato
used to be gated by the mod wheel, so depth set from the touch UI would have
done nothing on a controller with the joystick centred.
"""

from __future__ import annotations

import pathlib
import sys
import unittest

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


def load_midi_tone():
    import sounddevice as sd
    import mido

    sd.OutputStream = object  # type: ignore[assignment]
    mido.get_input_names = lambda: []  # type: ignore[assignment]
    mido.get_output_names = lambda: []  # type: ignore[assignment]
    import midi_tone

    return midi_tone


class VibratoTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.midi_tone = load_midi_tone()

    def make_engine(self):
        mt = self.midi_tone
        phase = np.linspace(0.0, 2.0 * np.pi, mt.TABLE_SIZE, endpoint=False)
        tables = {
            "sine": np.sin(phase).astype(np.float32),
            "saw": np.linspace(-1.0, 1.0, mt.TABLE_SIZE, dtype=np.float32),
        }
        return mt.SineEngine(tables, max_voices=4)

    def render(self, engine, frames: int = 256) -> np.ndarray:
        out = np.zeros((frames, 1), dtype=np.float32)
        engine._callback(out, frames, None, None)
        return np.copy(out[:, 0])

    def test_screen_vibrato_sounds_without_the_mod_wheel(self) -> None:
        dry = self.make_engine()
        dry.note_on(0, 60, 100)
        self.render(dry, 256)  # let the envelope settle identically in both
        flat = self.render(dry, 256)

        wobble = self.make_engine()
        wobble.set_vib_always(1.0)
        wobble.nudge_vib_depth(1.0)
        wobble.note_on(0, 60, 100)
        self.render(wobble, 256)
        bent = self.render(wobble, 256)

        self.assertFalse(
            np.allclose(flat, bent, atol=1e-4), "vibrato should change the rendered tone"
        )

    def test_wheel_down_and_screen_off_is_still_dry(self) -> None:
        engine = self.make_engine()
        engine.nudge_vib_depth(1.0)  # depth set, but nothing asking for vibrato
        engine.note_on(0, 60, 100)
        self.render(engine, 256)
        gated = self.render(engine, 256)

        dry = self.make_engine()
        dry.note_on(0, 60, 100)
        self.render(dry, 256)
        flat = self.render(dry, 256)

        np.testing.assert_allclose(gated, flat, atol=1e-6)

    def test_mod_wheel_still_works_when_screen_control_is_off(self) -> None:
        engine = self.make_engine()
        engine.nudge_vib_depth(1.0)
        engine.set_mod_wheel(127)
        engine.note_on(0, 60, 100)
        self.render(engine, 256)
        wheeled = self.render(engine, 256)

        dry = self.make_engine()
        dry.note_on(0, 60, 100)
        self.render(dry, 256)
        flat = self.render(dry, 256)

        self.assertFalse(np.allclose(wheeled, flat, atol=1e-4))

    def test_nudges_are_clamped_to_the_knob_range(self) -> None:
        engine = self.make_engine()
        for _ in range(60):
            engine.nudge_vib_depth(self.midi_tone.VIB_DEPTH_STEP)
        depth, _rate, _always = engine.vib_state()
        self.assertAlmostEqual(depth, engine.VIB_DEPTH_MAX, places=6)
        for _ in range(60):
            engine.nudge_vib_depth(-self.midi_tone.VIB_DEPTH_STEP)
        self.assertAlmostEqual(engine.vib_state()[0], 0.0, places=6)

        for _ in range(60):
            engine.nudge_vib_rate(self.midi_tone.VIB_RATE_STEP)
        self.assertAlmostEqual(engine.vib_state()[1], engine.VIB_HZ_MAX, places=6)
        for _ in range(60):
            engine.nudge_vib_rate(-self.midi_tone.VIB_RATE_STEP)
        self.assertAlmostEqual(engine.vib_state()[1], engine.VIB_HZ_MIN, places=6)

    def test_per_voice_vibrato_is_independent_of_the_live_rig(self) -> None:
        """A phrase pad's baked wobble plays even with the rig's vibrato off."""
        baked = self.make_engine()
        baked.note_on(0, 60, 100, vib=(2.0, 6.0, 1.0))
        self.render(baked, 256)
        wobbled = self.render(baked, 256)

        dry = self.make_engine()
        dry.note_on(0, 60, 100)
        self.render(dry, 256)
        flat = self.render(dry, 256)

        self.assertFalse(np.allclose(wobbled, flat, atol=1e-4))

    def test_baked_vibrato_ignores_the_global_setting(self) -> None:
        """Two engines, opposite rig settings, same baked pad note → same sound."""
        rig_off = self.make_engine()
        rig_off.note_on(0, 60, 100, vib=(1.0, 5.0, 1.0))
        self.render(rig_off, 256)
        a = self.render(rig_off, 256)

        rig_on = self.make_engine()
        rig_on.set_vib_always(1.0)
        rig_on.nudge_vib_depth(1.0)
        rig_on.set_mod_wheel(127)
        rig_on.note_on(0, 60, 100, vib=(1.0, 5.0, 1.0))
        self.render(rig_on, 256)
        b = self.render(rig_on, 256)

        np.testing.assert_allclose(a, b, atol=1e-6)

    def test_always_on_survives_a_preset_round_trip(self) -> None:
        engine = self.make_engine()
        engine.set_vib_always(1.0)
        engine.nudge_vib_depth(0.5)
        engine.nudge_vib_rate(-2.0)
        snap = engine.snapshot_settings()

        restored = self.make_engine()
        restored.apply_settings(snap)
        self.assertEqual(restored.vib_state(), engine.vib_state())

    def test_tone_zero_is_darker_than_tone_open(self) -> None:
        """KAOSS Y=TONE / the tone knob must change the sound, not just a 2-tap mix."""
        mt = self.midi_tone
        saw = np.linspace(-1.0, 1.0, mt.TABLE_SIZE, dtype=np.float32)

        def centroid(tone: float) -> float:
            engine = mt.SineEngine({"saw": saw}, max_voices=4)
            engine.set_tone(tone)
            engine.note_on(0, 60, 127)
            self.render(engine, 512)
            wave = self.render(engine, 2048)
            window = wave * np.hanning(len(wave))
            mag = np.abs(np.fft.rfft(window)) + 1e-12
            freqs = np.fft.rfftfreq(len(wave), 1.0 / engine.sample_rate)
            return float(np.sum(freqs * mag) / np.sum(mag))

        open_c = centroid(1.0)
        self.assertLess(centroid(0.0), open_c * 0.45)
        # Mid-pad used to sit near-open. 35% must already be a closed filter.
        self.assertLess(centroid(0.35), open_c * 0.75)

    def test_tone_closes_enough_to_quiet_a_high_sine(self) -> None:
        """Bottom of the pad must pull cutoff below a high note, not just dull a saw."""
        mt = self.midi_tone
        phase = np.linspace(0.0, 2.0 * np.pi, mt.TABLE_SIZE, endpoint=False)
        sine = np.sin(phase).astype(np.float32)

        def rms(tone: float) -> float:
            engine = mt.SineEngine({"sine": sine}, max_voices=4)
            engine.set_tone(tone)
            engine.note_on(0, 84, 127)  # C6 ~ 1047 Hz
            self.render(engine, 512)
            wave = self.render(engine, 2048)
            return float(np.sqrt(np.mean(wave * wave)))

        self.assertLess(rms(0.0), rms(1.0) * 0.35)

    def test_leaving_vib_clears_always_on_vibrato(self) -> None:
        """VIB Y arms always-on vibrato; switching to LEAD must not leave it stuck."""
        from types import SimpleNamespace

        from pidi.kaoss import KaossPad

        mt = self.midi_tone
        engine = self.make_engine()
        pad = KaossPad()
        pad.program_id = "vib"
        engine.set_tone(0.35)

        class Harness:
            def __init__(self) -> None:
                self.engine = engine
                self._kaoss = pad
                self._kaoss_fx_snap = None
                self._mode = "kaoss"
                self._seq = SimpleNamespace(record_note=lambda *a, **k: None)

            def _q_put(self, *a, **k):
                pass

            def _paint_kaoss_status(self):
                pass

            def _paint_kaoss(self):
                pass

            def _kaoss_draw_grid(self):
                pass

            def _mark_settings_dirty(self):
                pass

            def _kaoss_arm_tick(self):
                pass

            def _kaoss_refresh_axis_labels(self):
                pass

            def _paint_kaoss_status(self):
                pass

            def _kaoss_midi_send(self, msg):
                pass

            def _kaoss_capture_fx(self):
                return mt.MidiToneApp._kaoss_capture_fx(self)

            def _kaoss_overlay_names(self, prog):
                return mt.MidiToneApp._kaoss_overlay_names(self, prog)

            def _kaoss_restore_fx(self):
                return mt.MidiToneApp._kaoss_restore_fx(self)

            def _kaoss_apply(self, events, *, began=False, ended=False, restore=None):
                return mt.MidiToneApp._kaoss_apply(
                    self, events, began=began, ended=ended, restore=restore
                )

            def _kaoss_apply_program(self, program_id):
                return mt.MidiToneApp._kaoss_apply_program(self, program_id)

        app = Harness()
        app._kaoss_apply(pad.touch(0.5, 0.85), began=True)
        _depth, _hz, always = engine.vib_state()
        self.assertGreater(always, 0.5)

        app._kaoss_apply(pad.release(), ended=True)
        _depth, _hz, always = engine.vib_state()
        self.assertEqual(always, 0.0)
        self.assertAlmostEqual(engine.modulation_state()["tone"], 0.35, places=3)

        app._kaoss_apply(pad.touch(0.4, 0.9), began=True)
        self.assertGreater(engine.vib_state()[2], 0.5)
        pad.hold = True
        app._kaoss_apply_program("lead")
        self.assertEqual(engine.vib_state()[2], 0.0)

    def test_hold_keeps_voice_when_switching_to_filter(self) -> None:
        """HOLD + LEAD, then FILTER, must keep the note so XY can audition FX."""
        from types import SimpleNamespace

        from pidi.kaoss import KaossPad

        mt = self.midi_tone
        engine = self.make_engine()
        pad = KaossPad()
        pad.program_id = "lead"

        class Harness:
            def __init__(self) -> None:
                self.engine = engine
                self._kaoss = pad
                self._kaoss_fx_snap = None
                self._mode = "kaoss"
                self._seq = SimpleNamespace(record_note=lambda *a, **k: None)

            def _q_put(self, *a, **k):
                pass

            def _paint_kaoss_status(self):
                pass

            def _paint_kaoss(self):
                pass

            def _kaoss_draw_grid(self):
                pass

            def _mark_settings_dirty(self):
                pass

            def _kaoss_arm_tick(self):
                pass

            def _kaoss_refresh_axis_labels(self):
                pass

            def _paint_kaoss_status(self):
                pass

            def _kaoss_midi_send(self, msg):
                pass

            def _kaoss_capture_fx(self):
                return mt.MidiToneApp._kaoss_capture_fx(self)

            def _kaoss_overlay_names(self, prog):
                return mt.MidiToneApp._kaoss_overlay_names(self, prog)

            def _kaoss_restore_fx(self):
                return mt.MidiToneApp._kaoss_restore_fx(self)

            def _kaoss_apply(self, events, *, began=False, ended=False, restore=None):
                return mt.MidiToneApp._kaoss_apply(
                    self, events, began=began, ended=ended, restore=restore
                )

            def _kaoss_apply_program(self, program_id):
                return mt.MidiToneApp._kaoss_apply_program(self, program_id)

        app = Harness()
        app._kaoss_apply(pad.touch(0.0, 0.8), began=True)
        pad.set_hold(True)
        app._kaoss_apply(pad.release(), ended=True)
        self.assertTrue(engine._voices)
        note = pad.sounding_note()
        app._kaoss_apply_program("filter")
        self.assertEqual(pad.program_id, "filter")
        self.assertTrue(pad.is_active())
        self.assertEqual(pad.sounding_note(), note)
        self.assertTrue(engine._voices)

    def test_morph_program_keeps_voice_blend(self) -> None:
        """MORPH Y is a knob: leaving the pad must not snap back to the old mix."""
        from types import SimpleNamespace

        from pidi.kaoss import KaossPad

        mt = self.midi_tone
        engine = self.make_engine()
        engine.set_morph(0.15)
        pad = KaossPad()
        pad.program_id = "morph"

        class Harness:
            def __init__(self) -> None:
                self.engine = engine
                self._kaoss = pad
                self._kaoss_fx_snap = None
                self._mode = "kaoss"
                self._seq = SimpleNamespace(record_note=lambda *a, **k: None)

            def _q_put(self, *a, **k):
                pass

            def _paint_kaoss_status(self):
                pass

            def _paint_kaoss(self):
                pass

            def _kaoss_draw_grid(self):
                pass

            def _mark_settings_dirty(self):
                pass

            def _kaoss_arm_tick(self):
                pass

            def _kaoss_refresh_axis_labels(self):
                pass

            def _paint_kaoss_status(self):
                pass

            def _kaoss_midi_send(self, msg):
                pass

            def _kaoss_capture_fx(self):
                return mt.MidiToneApp._kaoss_capture_fx(self)

            def _kaoss_overlay_names(self, prog):
                return mt.MidiToneApp._kaoss_overlay_names(self, prog)

            def _kaoss_restore_fx(self):
                return mt.MidiToneApp._kaoss_restore_fx(self)

            def _kaoss_apply(self, events, *, began=False, ended=False, restore=None):
                return mt.MidiToneApp._kaoss_apply(
                    self, events, began=began, ended=ended, restore=restore
                )

            def _kaoss_apply_program(self, program_id):
                return mt.MidiToneApp._kaoss_apply_program(self, program_id)

        app = Harness()
        app._kaoss_apply(pad.touch(0.5, 0.8), began=True)
        self.assertAlmostEqual(engine.morph(), 0.8, places=3)
        app._kaoss_apply(pad.release(), ended=True)
        self.assertAlmostEqual(engine.morph(), 0.8, places=3)
        app._kaoss_apply_program("lead")
        self.assertAlmostEqual(engine.morph(), 0.8, places=3)

    def test_tone_lfo_wobbles_the_filter(self) -> None:
        dry = self.make_engine()
        dry.set_tone(0.5)
        dry.note_on(0, 60, 100)
        self.render(dry, 256)
        flat = self.render(dry, 2048)

        wah = self.make_engine()
        wah.set_tone(0.5)
        wah.set_tone_lfo_rate(1.0)
        wah.set_tone_lfo_amount(1.0)
        wah.note_on(0, 60, 100)
        self.render(wah, 256)
        wobble = self.render(wah, 2048)

        self.assertFalse(
            np.allclose(flat, wobble, atol=1e-4),
            "tone LFO should wah the filter vs sticky tone",
        )

    def test_kaoss_wah_param_does_not_rewrite_tone(self) -> None:
        engine = self.make_engine()
        engine.set_tone(0.4)
        engine.set_kaoss_param("tone_lfo", 0.85)
        hz, amt = engine.tone_lfo_state()
        self.assertGreater(amt, 0.5)
        self.assertGreater(hz, 4.0)
        self.assertAlmostEqual(engine.modulation_state()["tone"], 0.4, places=3)
        engine.set_tone_lfo_amount(0.0)
        self.assertEqual(engine.tone_lfo_state()[1], 0.0)

    def test_leaving_wah_clears_tone_lfo(self) -> None:
        from types import SimpleNamespace

        from pidi.kaoss import KaossPad

        mt = self.midi_tone
        engine = self.make_engine()
        engine.set_tone(0.35)
        pad = KaossPad()
        pad.program_id = "wah"

        class Harness:
            def __init__(self) -> None:
                self.engine = engine
                self._kaoss = pad
                self._kaoss_fx_snap = None
                self._mode = "kaoss"
                self._seq = SimpleNamespace(record_note=lambda *a, **k: None)

            def _q_put(self, *a, **k):
                pass

            def _paint_kaoss_status(self):
                pass

            def _paint_kaoss(self):
                pass

            def _kaoss_draw_grid(self):
                pass

            def _mark_settings_dirty(self):
                pass

            def _kaoss_arm_tick(self):
                pass

            def _kaoss_refresh_axis_labels(self):
                pass

            def _kaoss_midi_send(self, msg):
                pass

            def _kaoss_capture_fx(self):
                return mt.MidiToneApp._kaoss_capture_fx(self)

            def _kaoss_overlay_names(self, prog):
                return mt.MidiToneApp._kaoss_overlay_names(self, prog)

            def _kaoss_restore_fx(self):
                return mt.MidiToneApp._kaoss_restore_fx(self)

            def _kaoss_apply(self, events, *, began=False, ended=False, restore=None):
                return mt.MidiToneApp._kaoss_apply(
                    self, events, began=began, ended=ended, restore=restore
                )

            def _kaoss_apply_program(self, program_id):
                return mt.MidiToneApp._kaoss_apply_program(self, program_id)

        app = Harness()
        app._kaoss_apply(pad.touch(0.5, 0.9), began=True)
        self.assertGreater(engine.tone_lfo_state()[1], 0.5)

        app._kaoss_apply(pad.release(), ended=True)
        self.assertEqual(engine.tone_lfo_state()[1], 0.0)
        self.assertAlmostEqual(engine.modulation_state()["tone"], 0.35, places=3)

        app._kaoss_apply(pad.touch(0.4, 0.8), began=True)
        self.assertGreater(engine.tone_lfo_state()[1], 0.5)
        pad.hold = True
        app._kaoss_apply_program("lead")
        self.assertEqual(engine.tone_lfo_state()[1], 0.0)


if __name__ == "__main__":
    sys.exit(unittest.main())
