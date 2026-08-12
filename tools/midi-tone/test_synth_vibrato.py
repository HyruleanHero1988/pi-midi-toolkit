#!/usr/bin/env python3
"""
Vibrato rules for the soft-synth — renders blocks directly, no audio device.

The point of these: the screen control has to be audible on its own. Vibrato
used to be gated by the mod wheel, so depth set from the touch UI would have
done nothing on a controller with the joystick centred.
"""

from __future__ import annotations

import sys
import unittest

import numpy as np


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

    def test_always_on_survives_a_preset_round_trip(self) -> None:
        engine = self.make_engine()
        engine.set_vib_always(1.0)
        engine.nudge_vib_depth(0.5)
        engine.nudge_vib_rate(-2.0)
        snap = engine.snapshot_settings()

        restored = self.make_engine()
        restored.apply_settings(snap)
        self.assertEqual(restored.vib_state(), engine.vib_state())


if __name__ == "__main__":
    sys.exit(unittest.main())
