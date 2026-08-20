"""Keys-bus tone (Chamberlin SVF brightness)."""
from __future__ import annotations

import math
from typing import Tuple

import numpy as np

def apply_tone_lowpass(
    buf: np.ndarray,
    tone: float,
    low: float,
    band: float = 0.0,
    sample_rate: float = 44100.0,
) -> Tuple[float, float]:
    """Darken a live audio block in place. ``tone`` 1 = open, 0 = dark.

    Resonant lowpass (Chamberlin SVF), exponential cutoff. A one-pole with a
    linear/squared knob sat near-open across most of the pad, so Kaoss Y and
    the MPK tone knob sounded dead on typical wavetables.
    """
    n = int(buf.size)
    tone = max(0.0, min(1.0, float(tone)))
    if n == 0:
        return float(low), float(band)
    if tone >= 0.985:
        return float(buf[-1]), 0.0
    sr = max(8000.0, float(sample_rate))
    # 90 Hz .. 8 kHz. Bottom of the pad is a closed filter; top is almost open
    # (full bypass is the >= 0.985 branch).
    fc = 90.0 * ((8000.0 / 90.0) ** tone)
    f = 2.0 * math.sin(math.pi * min(fc, sr * 0.14) / sr)
    damp = 0.38 + 0.62 * tone
    lp = float(low)
    bp = float(band)
    samples = buf
    for i in range(n):
        lp += f * bp
        hp = float(samples[i]) - lp - damp * bp
        bp += f * hp
        samples[i] = lp
    return lp, bp


