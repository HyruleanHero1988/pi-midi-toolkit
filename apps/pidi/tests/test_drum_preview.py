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

    def _preview(self, model: str) -> np.ndarray:
        return render_drum_preview(
            model,
            pitch=0.45,
            decay=0.40,
            noise_amt=0.55,
            tone=0.60,
            sample_rate=SAMPLE_RATE,
            duration_sec=0.30,
            velocity=110.0 / 127.0,
        )

    def test_cowbell_rings_after_clave_dies(self) -> None:
        cow = self._preview("cowbell")
        clav = self._preview("clave")
        sr = SAMPLE_RATE
        cow_early = float(np.sqrt(np.mean(cow[:256] ** 2)))
        clav_early = float(np.sqrt(np.mean(clav[:256] ** 2)))
        late = slice(int(0.09 * sr), int(0.12 * sr))
        cow_late = float(np.sqrt(np.mean(cow[late] ** 2)))
        clav_late = float(np.sqrt(np.mean(clav[late] ** 2)))
        self.assertGreater(cow_early, 0.02)
        self.assertGreater(clav_early, 0.02)
        self.assertGreater(cow_late, clav_late * 4.0)
        self.assertLess(clav_late, 0.002)

    def test_cowbell_is_square_not_sine(self) -> None:
        cow = self._preview("cowbell")[:2048]
        signs = np.sign(cow)
        crossings = int(np.count_nonzero(signs[1:] != signs[:-1]))
        self.assertGreater(crossings, 40)
        self.assertGreater(float(np.max(np.abs(cow))), 0.08)

    def test_clap_stutters_and_is_not_a_snare(self) -> None:
        clap = self._preview("clap")
        snare = self._preview("snare")
        sr = SAMPLE_RATE
        first = float(np.sqrt(np.mean(clap[:90] ** 2)))
        dip = float(np.sqrt(np.mean(clap[200:320] ** 2)))
        second = float(np.sqrt(np.mean(clap[420:520] ** 2)))
        self.assertGreater(first, 0.02)
        self.assertLess(dip, first * 0.55)
        self.assertGreater(second, dip * 1.6)

        n = min(3500, len(clap), len(snare))

        def goertzel(buf: np.ndarray, freq: float) -> float:
            w = 2.0 * np.pi * freq / sr
            coeff = 2.0 * np.cos(w)
            s0 = s1 = s2 = 0.0
            for x in buf[:n]:
                s0 = float(x) + coeff * s1 - s2
                s2, s1 = s1, s0
            return s1 * s1 + s2 * s2 - coeff * s1 * s2

        clap_body = goertzel(clap, 175.0)
        snare_body = goertzel(snare, 175.0)
        clap_mid = goertzel(clap, 1200.0)
        self.assertGreater(snare_body, clap_body * 4.0)
        self.assertGreater(clap_mid, clap_body)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
