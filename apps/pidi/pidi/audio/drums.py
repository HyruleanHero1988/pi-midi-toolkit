"""Procedural drum models and kit helpers."""
from __future__ import annotations

import math
from typing import Dict, Optional, Tuple

import numpy as np

from pidi.constants import DRUM_SCOPE_SEC, NOTE_NAMES, SAMPLE_RATE, SCOPE_MORPH_CYCLES
from pidi.domain.phrases import mpk_note_for_phrase_cell  # noqa: F401 — kit / UI re-export

def midi_note_name(note: int) -> str:
    return f"{NOTE_NAMES[note % 12]}{note // 12 - 1}"


def midi_to_hz(note: int) -> float:
    return 440.0 * (2.0 ** ((note - 69) / 12.0))


# MPK mini mk3 factory pad notes (Prog Select → Pad 1 / MPC): Bank A = 36–43, Bank B = 44–51.
# Layout per bank: pads 1–4 bottom L→R, pads 5–8 top L→R.
MPK_PAD_KIT: Dict[int, str] = {
    # Bank A
    36: "kick",
    37: "snare",
    38: "clap",
    39: "hat_closed",
    40: "hat_open",
    41: "tom_lo",
    42: "tom_mid",
    43: "rim",
    # Bank B
    44: "kick_tight",
    45: "rimshot",
    46: "shaker",
    47: "hat_pedal",
    48: "tom_hi",
    49: "cowbell",
    50: "clave",
    51: "ride",
}

# Extra GM-ish aliases so other pad programs still land somewhere useful
_DRUM_ALIASES: Dict[int, str] = {
    35: "kick",
    52: "ride",
    55: "hat_open",
    57: "ride",
    59: "ride",
}


def drum_model_for_note(note: int) -> str:
    """Map MIDI note (ch10) → one of 16 procedural drum / one-shot models."""
    n = note & 0x7F
    if n in MPK_PAD_KIT:
        return MPK_PAD_KIT[n]
    if n in _DRUM_ALIASES:
        return _DRUM_ALIASES[n]
    # Unknown note: cycle through the kit so every pad still sounds distinct
    models = list(dict.fromkeys(MPK_PAD_KIT.values()))
    return models[n % len(models)]


def _cheap_square(phases: np.ndarray, two_pi: float) -> np.ndarray:
    """Naive bipolar square; matches jambox-core `cheap_square`."""
    return np.where(np.mod(phases, two_pi) < math.pi, 1.0, -1.0).astype(np.float32)


def _one_pole_lp(x: np.ndarray, coef: float) -> np.ndarray:
    y = np.empty_like(x, dtype=np.float32)
    acc = 0.0
    c = float(coef)
    for i, s in enumerate(x):
        acc += (float(s) - acc) * c
        y[i] = acc
    return y


def downsample_waveform(samples: np.ndarray, points: int) -> np.ndarray:
    """Reduce a sample buffer to `points` peaks for canvas drawing."""
    if samples is None or len(samples) == 0 or points <= 0:
        return np.zeros(max(1, points), dtype=np.float32)
    x = np.asarray(samples, dtype=np.float32)
    if len(x) <= points:
        return x.copy()
    # Min/max buckets keep spikes visible (better than plain stride for drums)
    bucket = len(x) / float(points)
    out = np.empty(points, dtype=np.float32)
    for i in range(points):
        a = int(i * bucket)
        b = int((i + 1) * bucket)
        if b <= a:
            b = a + 1
        chunk = x[a:b]
        # Alternate extrema so the polyline fills the envelope
        out[i] = float(np.max(chunk) if (i & 1) == 0 else np.min(chunk))
    return out


def render_drum_preview(
    model: str,
    *,
    pitch: float,
    decay: float,
    noise_amt: float,
    tone: float,
    sample_rate: int = SAMPLE_RATE,
    duration_sec: float = 0.32,
    velocity: float = 0.95,
    seed: int = 7,
) -> np.ndarray:
    """
    Offline one-shot for the drum scope (seeded noise so redraws stay stable).

    Uses a fixed time window so stretch/decay reads as how much of the scope
    stays alive (short = early silence; long = fills the frame).
    """
    sr = max(8000, int(sample_rate))
    n = max(32, int(duration_sec * sr))
    t = np.arange(n, dtype=np.float32) * np.float32(1.0 / sr)
    arange = np.arange(n, dtype=np.float32)
    rng = np.random.RandomState(int(seed) & 0xFFFFFFFF)
    white = (rng.random(n).astype(np.float32) * 2.0 - 1.0)
    win = max(1, int(round((1.0 - max(0.0, min(1.0, tone))) * 16.0)))
    if win <= 1:
        noise = white
    else:
        kernel = np.ones(win, dtype=np.float32) / np.float32(win)
        noise = np.convolve(white, kernel, mode="same").astype(np.float32)
    audio, _dur, _phase = synthesize_drum(
        model,
        t=t,
        arange=arange,
        white=white,
        noise=noise,
        pitch=max(0.0, min(1.0, float(pitch))),
        decay=max(0.0, min(1.0, float(decay))),
        noise_amt=max(0.0, min(1.0, float(noise_amt))),
        tone=max(0.0, min(1.0, float(tone))),
        vel=max(0.05, min(1.0, float(velocity))),
        phase=0.0,
        two_pi=2.0 * math.pi,
        inv_sr=1.0 / sr,
    )
    return audio.astype(np.float32, copy=False)


def synthesize_drum(
    model: str,
    *,
    t: np.ndarray,
    arange: np.ndarray,
    white: np.ndarray,
    noise: np.ndarray,
    pitch: float,
    decay: float,
    noise_amt: float,
    tone: float,
    vel: float,
    phase: float,
    two_pi: float,
    inv_sr: float,
) -> Tuple[np.ndarray, float, float]:
    """Render one block of a procedural drum. Returns (audio, dur_sec, new_phase)."""
    # Pitch-envelope body (kick / toms / tight kick)
    if model in ("kick", "kick_tight", "tom_lo", "tom_mid", "tom_hi"):
        if model == "kick":
            f0 = 50.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 28.0 + 16.0 * pitch
            drop_tau = 0.016 + 0.05 * (1.0 - decay)
            body_tau = 0.07 + 0.40 * decay
            amp = 0.38
        elif model == "kick_tight":
            f0 = 68.0 * (2.0 ** ((pitch - 0.5) * 1.6))
            f_end = 40.0 + 20.0 * pitch
            drop_tau = 0.010 + 0.03 * (1.0 - decay)
            body_tau = 0.035 + 0.18 * decay
            amp = 0.34
        elif model == "tom_lo":
            f0 = 85.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 55.0 + 25.0 * pitch
            drop_tau = 0.025 + 0.07 * (1.0 - decay)
            body_tau = 0.08 + 0.38 * decay
            amp = 0.32
        elif model == "tom_mid":
            f0 = 120.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 75.0 + 30.0 * pitch
            drop_tau = 0.022 + 0.06 * (1.0 - decay)
            body_tau = 0.06 + 0.30 * decay
            amp = 0.30
        else:  # tom_hi
            f0 = 170.0 * (2.0 ** ((pitch - 0.5) * 1.8))
            f_end = 100.0 + 40.0 * pitch
            drop_tau = 0.018 + 0.05 * (1.0 - decay)
            body_tau = 0.045 + 0.22 * decay
            amp = 0.28
        freq = f_end + (f0 - f_end) * np.exp(-t / np.float32(drop_tau))
        phase_inc = two_pi * freq * inv_sr
        phases = phase + np.cumsum(phase_inc)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        click = np.exp(-t / np.float32(0.0035)) * np.float32(0.18 * vel)
        body = np.sin(phases) * env * np.float32(amp * vel)
        sig = body + click * white + noise * np.float32(0.05 * noise_amt * vel) * env
        return sig.astype(np.float32, copy=False), body_tau * 4.5, new_phase

    if model in ("snare", "rimshot"):
        f0 = (200.0 if model == "rimshot" else 175.0) * (2.0 ** ((pitch - 0.5) * 1.4))
        body_tau = 0.018 + 0.10 * decay if model == "rimshot" else 0.03 + 0.18 * decay
        noise_tau = 0.025 + 0.14 * decay if model == "rimshot" else 0.04 + 0.28 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        tone_env = np.exp(-t / np.float32(body_tau))
        noise_env = np.exp(-t / np.float32(noise_tau))
        body_amp = 0.10 if model == "rimshot" else 0.16
        noise_amp = (0.28 + 0.45 * noise_amt) if model == "rimshot" else (0.18 + 0.40 * noise_amt)
        click = np.exp(-t / np.float32(0.002)) * np.float32(0.25 * vel if model == "rimshot" else 0.12 * vel)
        sig = np.sin(phases) * tone_env * np.float32(body_amp * vel)
        sig += noise * noise_env * np.float32(noise_amp * vel)
        sig += click * white
        return sig.astype(np.float32, copy=False), max(body_tau, noise_tau) * 5.0, new_phase

    if model == "clap":
        noise_tau = 0.03 + 0.22 * decay
        noise_env = np.exp(-t / np.float32(noise_tau))
        bursts = (
            np.exp(-((t - 0.000) ** 2) / 0.0000008)
            + np.exp(-((t - 0.012) ** 2) / 0.0000010)
            + np.exp(-((t - 0.024) ** 2) / 0.0000012)
        )
        sig = noise * (bursts * np.float32(0.45) + noise_env * np.float32(0.28))
        sig *= np.float32((0.28 + 0.4 * noise_amt) * vel)
        return sig.astype(np.float32, copy=False), noise_tau * 5.0, phase

    if model in ("hat_closed", "hat_open", "hat_pedal", "shaker"):
        if model == "hat_open":
            noise_tau = 0.05 + 0.40 * decay
            amp = 0.14 + 0.30 * noise_amt
        elif model == "hat_pedal":
            noise_tau = 0.008 + 0.04 * decay
            amp = 0.12 + 0.22 * noise_amt
        elif model == "shaker":
            noise_tau = 0.02 + 0.10 * decay
            amp = 0.12 + 0.28 * noise_amt
        else:  # hat_closed
            noise_tau = 0.015 + 0.08 * decay
            amp = 0.14 + 0.30 * noise_amt
        bright = white - noise * np.float32(0.85)
        noise_env = np.exp(-t / np.float32(noise_tau))
        if model == "shaker":
            # Grainy amplitude modulation
            grain = 0.55 + 0.45 * np.sin(two_pi * (40.0 + 80.0 * pitch) * t)
            sig = bright * noise_env * grain * np.float32(amp * vel)
        else:
            sig = bright * noise_env * np.float32(amp * vel)
        sig = sig * np.float32(0.35 + 0.65 * tone) + noise * noise_env * np.float32(
            0.10 * (1.0 - tone) * vel
        )
        return sig.astype(np.float32, copy=False), noise_tau * 5.5, phase

    if model == "ride":
        # Metallic ding + wash, not a lower open hat.
        f0 = 3200.0 * (2.0 ** ((pitch - 0.5) * 0.45))
        body_tau = 0.08 + 0.32 * decay
        noise_tau = 0.12 + 0.55 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        body_env = np.exp(-t / np.float32(body_tau))
        noise_env = np.exp(-t / np.float32(noise_tau))
        ding = np.sin(phases) * body_env * np.float32(0.09 * vel)
        bright = white - noise * np.float32(0.85)
        wash_amp = 0.10 + 0.22 * noise_amt
        wash = bright * noise_env * np.float32(wash_amp * vel)
        wash = wash * np.float32(0.35 + 0.65 * tone) + noise * noise_env * np.float32(
            0.10 * (1.0 - tone) * vel
        )
        sig = ding + wash
        return sig.astype(np.float32, copy=False), max(body_tau, noise_tau) * 5.5, new_phase

    if model == "rim":
        # Short woodblock / stick click
        f0 = 520.0 * (2.0 ** ((pitch - 0.5) * 1.2))
        body_tau = 0.012 + 0.05 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        sig = np.sin(phases) * env * np.float32(0.22 * vel)
        sig += white * np.exp(-t / np.float32(0.004)) * np.float32(0.18 * vel)
        return sig.astype(np.float32, copy=False), body_tau * 5.0, new_phase

    if model == "cowbell":
        # TR-808: two inharmonic square partials (~540 Hz + ~800 Hz), not a sine clave.
        f1 = 540.0 * (2.0 ** ((pitch - 0.5) * 1.0))
        f2 = 800.0 * (2.0 ** ((pitch - 0.5) * 1.0))
        body_tau = 0.08 + 0.32 * decay
        env = np.exp(-t / np.float32(body_tau))
        p1 = two_pi * f1 * t
        p2 = two_pi * f2 * t
        mix = _cheap_square(p1, two_pi) + np.float32(0.72) * _cheap_square(p2, two_pi)
        color_lp = _one_pole_lp(mix, 0.16)
        colored = mix - np.float32(0.55) * color_lp
        sig = colored * env * np.float32(0.13 * vel)
        sig += noise * env * np.float32(0.03 * noise_amt * vel)
        return sig.astype(np.float32, copy=False), body_tau * 5.0, float(p1[-1] % two_pi)

    if model == "clave":
        # Short woody tick — distinct from cowbell (higher, much shorter).
        f0 = 2450.0 * (2.0 ** ((pitch - 0.5) * 0.55))
        body_tau = 0.006 + 0.016 * decay
        phase_inc = two_pi * f0 * inv_sr
        phases = phase + phase_inc * (arange + 1.0)
        new_phase = float(phases[-1] % two_pi)
        env = np.exp(-t / np.float32(body_tau))
        sig = np.sin(phases) * env * np.float32(0.22 * vel)
        sig += white * np.exp(-t / np.float32(0.0035)) * np.float32(0.22 * vel)
        return sig.astype(np.float32, copy=False), body_tau * 6.0, new_phase

    # Fallback: short closed hat
    noise_tau = 0.02 + 0.08 * decay
    noise_env = np.exp(-t / np.float32(noise_tau))
    sig = (white - noise * 0.8) * noise_env * np.float32(0.15 * vel)
    return sig.astype(np.float32, copy=False), noise_tau * 5.0, phase
