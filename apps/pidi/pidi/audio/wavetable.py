"""Wavetable load / normalize / FX sidecars."""
from __future__ import annotations

import pathlib
import re
import wave
from typing import Dict, List, Optional

import numpy as np

from pidi.constants import BUILTIN_VOICE_NAMES, TABLE_PEAK, TABLE_SIZE, VOICE_NAME_MAX

def _builtin_tables() -> Dict[str, np.ndarray]:
    t = np.linspace(0.0, 1.0, TABLE_SIZE, endpoint=False, dtype=np.float64)
    sine = (np.sin(2.0 * np.pi * t) * TABLE_PEAK).astype(np.float32)
    square = np.where(t < 0.5, TABLE_PEAK, -TABLE_PEAK).astype(np.float32)
    saw = (2.0 * (t - np.floor(t + 0.5))).astype(np.float32) * np.float32(TABLE_PEAK)
    triangle = (
        (2.0 * np.abs(2.0 * (t - np.floor(t + 0.5))) - 1.0).astype(np.float32)
        * np.float32(TABLE_PEAK)
    )
    return {
        "sine": sine,
        "square": square,
        "saw": saw,
        "triangle": triangle,
    }


def _load_wav_mono(path: pathlib.Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        ch = w.getnchannels()
        sw = w.getsampwidth()
        n = w.getnframes()
        raw = w.readframes(n)
    if sw == 2:
        x = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    elif sw == 4:
        # int32 PCM or float32 — detect by range later; treat as int32 first
        as_i = np.frombuffer(raw, dtype="<i4")
        if np.max(np.abs(as_i)) > 8:
            x = as_i.astype(np.float32) / 2147483648.0
        else:
            x = np.frombuffer(raw, dtype="<f4").copy()
    else:
        raise ValueError(f"{path}: unsupported sample width {sw}")
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x


def _resample_cycle(x: np.ndarray, n: int = TABLE_SIZE) -> np.ndarray:
    if len(x) == n:
        y = x.astype(np.float32, copy=True)
    else:
        phase = np.linspace(0.0, 1.0, n, endpoint=False, dtype=np.float64)
        idx = phase * len(x)
        i0 = np.floor(idx).astype(np.int64) % len(x)
        i1 = (i0 + 1) % len(x)
        frac = (idx - np.floor(idx)).astype(np.float32)
        y = (x[i0] * (1.0 - frac) + x[i1] * frac).astype(np.float32)
    peak = float(np.max(np.abs(y))) or 1.0
    return (y / peak) * np.float32(TABLE_PEAK)


def load_wavetables(*directories: pathlib.Path) -> Dict[str, np.ndarray]:
    """Built-ins first, then *.wav from each directory (later dirs override)."""
    tables = _builtin_tables()
    for directory in directories:
        if not directory or not pathlib.Path(directory).is_dir():
            continue
        for path in sorted(pathlib.Path(directory).glob("*.wav")):
            name = path.stem.lower().strip()
            if not name:
                continue
            # Keep core procedural oscillators; files can add/replace everything else
            if name in BUILTIN_VOICE_NAMES:
                continue
            try:
                tables[name] = _resample_cycle(_load_wav_mono(path))
            except Exception as exc:
                print(f"wavetable skip {path.name}: {exc}", flush=True)
    return tables


def sanitize_voice_name(raw: str) -> str:
    """Lowercase [a-z0-9_], max VOICE_NAME_MAX — safe for filenames + UI."""
    s = re.sub(r"[^a-z0-9_]+", "_", str(raw or "").lower().strip())
    s = re.sub(r"_+", "_", s).strip("_")
    if not s:
        s = "voice"
    return s[:VOICE_NAME_MAX]


def suggest_voice_name(name_a: str, name_b: str, morph: float) -> str:
    a = sanitize_voice_name(name_a) or "a"
    b = sanitize_voice_name(name_b) or "b"
    pct = int(round(max(0.0, min(1.0, float(morph))) * 100.0))
    if a == b:
        return sanitize_voice_name(f"{a}_saved")
    return sanitize_voice_name(f"{a}_{b}_{pct}")


def unique_voice_name(base: str, existing: List[str]) -> str:
    key = sanitize_voice_name(base)
    if key not in BUILTIN_VOICE_NAMES and key not in existing:
        return key
    for n in range(2, 1000):
        suffix = f"_{n}"
        stem = key[: max(1, VOICE_NAME_MAX - len(suffix))]
        cand = f"{stem}{suffix}"
        if cand not in BUILTIN_VOICE_NAMES and cand not in existing:
            return cand
    return sanitize_voice_name(f"voice_{int(time.time()) % 100000}")


def write_wavetable_wav(
    path: pathlib.Path, table: np.ndarray, *, sample_rate: int = 44100
) -> None:
    """Write a mono 16-bit single-cycle WAV (loadable by load_wavetables)."""
    y = np.asarray(table, dtype=np.float32).reshape(-1)
    if y.shape[0] != TABLE_SIZE:
        y = _resample_cycle(y)
    peak = float(np.max(np.abs(y))) or 1.0
    pcm = np.clip(y / peak * 32767.0, -32768, 32767).astype("<i2")
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(sample_rate))
        w.writeframes(pcm.tobytes())


# Time-domain FX that cannot bake into a single cycle — stored beside the .wav
VOICE_FX_SIDECAR_KEYS = (
    "fx_delay_time",
    "fx_delay_fb",
    "fx_delay_mix",
    "fx_reverb_size",
    "fx_reverb_mix",
)


def voice_fx_sidecar_path(waves_dir: pathlib.Path, name: str) -> pathlib.Path:
    return pathlib.Path(waves_dir) / f"{sanitize_voice_name(name)}.fx.json"


def write_voice_fx_sidecar(path: pathlib.Path, fx: Dict[str, float]) -> None:
    """Tiny JSON next to a user wavetable: delay/reverb mixes (+ time/fb/size)."""
    out = {"version": 1}
    for key in VOICE_FX_SIDECAR_KEYS:
        try:
            out[key] = max(0.0, min(1.0, float(fx.get(key, 0.0))))
        except (TypeError, ValueError):
            out[key] = 0.0
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def load_voice_fx_sidecar(path: pathlib.Path) -> Optional[Dict[str, float]]:
    path = pathlib.Path(path)
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None
    if not isinstance(data, dict):
        return None
    out: Dict[str, float] = {}
    for key in VOICE_FX_SIDECAR_KEYS:
        if key not in data:
            continue
        try:
            out[key] = max(0.0, min(1.0, float(data[key])))
        except (TypeError, ValueError):
            continue
    return out or None


def load_user_voice_fx_map(directory: pathlib.Path) -> Dict[str, Dict[str, float]]:
    """voice name → delay/reverb sidecar for user-wavetables/."""
    result: Dict[str, Dict[str, float]] = {}
    directory = pathlib.Path(directory)
    if not directory.is_dir():
        return result
    for path in sorted(directory.glob("*.fx.json")):
        name = sanitize_voice_name(path.name[: -len(".fx.json")])
        if not name or name in BUILTIN_VOICE_NAMES:
            continue
        snap = load_voice_fx_sidecar(path)
        if snap:
            result[name] = snap
    return result


def circular_moving_average(x: np.ndarray, win: int) -> np.ndarray:
    """Tone-bake helper: low-pass a periodic single cycle."""
    n = int(x.shape[0])
    win = max(1, int(win))
    if win <= 1 or n == 0:
        return np.asarray(x, dtype=np.float32)
    k = np.ones(win, dtype=np.float32) / np.float32(win)
    tiled = np.tile(np.asarray(x, dtype=np.float32), 3)
    y = np.convolve(tiled, k, mode="same")
    return y[n : 2 * n].astype(np.float32, copy=False)


