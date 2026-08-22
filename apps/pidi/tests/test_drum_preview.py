#!/usr/bin/env python3
"""Drum scope preview — KIT → WAVE has to render a real one-shot, not throw."""

from __future__ import annotations

import pathlib
import sys
import unittest

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from pidi.audio.drums import render_drum_preview  # noqa: E402
from pidi.constants import DRUM_SCOPE_SEC, SAMPLE_RATE  # noqa: E402


class DrumPreviewTest(unittest.TestCase):
    def test_kick_preview_is_audible_and_matches_scope_window(self) -> None:
        wave = render_drum_preview(
            "kick",
            pitch=0.5,
            decay=0.4,
            noise_amt=0.3,
            tone=0.5,
            sample_rate=SAMPLE_RATE,
            duration_sec=DRUM_SCOPE_SEC,
        )
        self.assertGreater(len(wave), 100)
        expected = int(DRUM_SCOPE_SEC * SAMPLE_RATE)
        self.assertEqual(len(wave), expected)
        self.assertGreater(float(np.max(np.abs(wave))), 0.05)

    def test_engine_preview_drum_waveform(self) -> None:
        import sounddevice as sd
        import mido

        sd.OutputStream = object  # type: ignore[assignment]
        mido.get_input_names = lambda: []  # type: ignore[assignment]
        mido.get_output_names = lambda: []  # type: ignore[assignment]
        import midi_tone

        phase = np.linspace(0.0, 2.0 * np.pi, midi_tone.TABLE_SIZE, endpoint=False)
        tables = {
            "sine": np.sin(phase).astype(np.float32),
            "saw": np.linspace(-1.0, 1.0, midi_tone.TABLE_SIZE, dtype=np.float32),
        }
        engine = midi_tone.SineEngine(tables, max_voices=2)
        wave = engine.preview_drum_waveform("snare")
        self.assertGreater(len(wave), 100)
        self.assertGreater(float(np.max(np.abs(wave))), 0.02)

        engine.note_on(midi_tone.DRUM_CHANNEL, 36, 110)
        self.assertIn(36, engine._drums)
        self.assertEqual(engine._drums[36].model, "kick")


if __name__ == "__main__":
    raise SystemExit(unittest.main())
