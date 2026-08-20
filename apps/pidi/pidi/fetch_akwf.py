#!/usr/bin/env python3
"""Fetch extra Adventure Kid (AKWF) single-cycles into ./wavetables (CC0)."""

from __future__ import annotations

import argparse
import pathlib
import sys
import urllib.request
import wave

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "wavetables"
BASE = (
    "https://raw.githubusercontent.com/"
    "KristofferKarlAxelEkstrand/AKWF-FREE/master/AKWF"
)
TABLE_SIZE = 2048

# Friendly alias → (repo subdir, filename)
CATALOG: dict[str, tuple[str, str]] = {
    "epiano": ("AKWF_epiano", "AKWF_epiano_0001.wav"),
    "organ": ("AKWF_eorgan", "AKWF_eorgan_0001.wav"),
    "flute": ("AKWF_flute", "AKWF_flute_0001.wav"),
    "violin": ("AKWF_violin", "AKWF_violin_0001.wav"),
    "aguitar": ("AKWF_aguitar", "AKWF_aguitar_0001.wav"),
    "ebass": ("AKWF_ebass", "AKWF_ebass_0001.wav"),
    "dbass": ("AKWF_dbass", "AKWF_dbass_0001.wav"),
    "fm": ("AKWF_fmsynth", "AKWF_fmsynth_0001.wav"),
    "piano": ("AKWF_piano", "AKWF_piano_0001.wav"),
    "pluck": ("AKWF_pluckalgo", "AKWF_pluckalgo_0001.wav"),
    "theremin": ("AKWF_theremin", "AKWF_theremin_0001.wav"),
    "voice": ("AKWF_hvoice", "AKWF_hvoice_0001.wav"),
    "clav": ("AKWF_clavinet", "AKWF_clavinet_0001.wav"),
    "chip": ("AKWF_oscchip", "AKWF_oscchip_0001.wav"),
    "vgsaw": ("AKWF_vgamebasic", "AKWF_vgsaw_0001.wav"),
    "sax": ("AKWF_altosax", "AKWF_altosax_0001.wav"),
    "symetric": ("AKWF_symetric", "AKWF_symetric_0001.wav"),
    "akwf_saw": ("AKWF_bw_perfectwaves", "AKWF_saw.wav"),
    "akwf_square": ("AKWF_bw_perfectwaves", "AKWF_squ.wav"),
    "akwf_triangle": ("AKWF_bw_perfectwaves", "AKWF_tri.wav"),
}


def load_wav_bytes(raw: bytes) -> np.ndarray:
    import io

    with wave.open(io.BytesIO(raw), "rb") as w:
        ch = w.getnchannels()
        sw = w.getsampwidth()
        n = w.getnframes()
        data = w.readframes(n)
    if sw == 2:
        x = np.frombuffer(data, dtype="<i2").astype(np.float32) / 32768.0
    elif sw == 4:
        x = np.frombuffer(data, dtype="<i4").astype(np.float32) / 2147483648.0
    else:
        raise ValueError(f"unsupported sample width {sw}")
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x


def resample_cycle(x: np.ndarray, n: int = TABLE_SIZE) -> np.ndarray:
    phase = np.linspace(0.0, 1.0, n, endpoint=False, dtype=np.float64)
    idx = phase * len(x)
    i0 = np.floor(idx).astype(np.int64) % len(x)
    i1 = (i0 + 1) % len(x)
    frac = (idx - np.floor(idx)).astype(np.float32)
    y = x[i0] * (1.0 - frac) + x[i1] * frac
    peak = float(np.max(np.abs(y))) or 1.0
    return (y / peak).astype(np.float32)


def write_wav(path: pathlib.Path, samples: np.ndarray, sr: int = 44100) -> None:
    ints = np.clip(samples * 32767.0, -32768, 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(ints.tobytes())


def fetch_one(alias: str) -> pathlib.Path:
    if alias not in CATALOG:
        raise SystemExit(f"Unknown alias '{alias}'. Try --list")
    sub, fname = CATALOG[alias]
    url = f"{BASE}/{sub}/{fname}"
    print(f"GET {url}")
    with urllib.request.urlopen(url, timeout=30) as resp:
        raw = resp.read()
    y = resample_cycle(load_wav_bytes(raw))
    OUT.mkdir(parents=True, exist_ok=True)
    out = OUT / f"{alias}.wav"
    write_wav(out, y)
    print(f"  -> {out} ({TABLE_SIZE} samples)")
    return out


def main() -> None:
    p = argparse.ArgumentParser(description="Fetch AKWF single-cycles (CC0) for midi-tone")
    p.add_argument("aliases", nargs="*", help="Voice aliases to fetch")
    p.add_argument("--list", action="store_true", help="List known aliases")
    p.add_argument("--all", action="store_true", help="Fetch the whole curated catalog")
    args = p.parse_args()
    if args.list:
        for k in sorted(CATALOG):
            print(k)
        return
    names = list(CATALOG) if args.all else args.aliases
    if not names:
        p.print_help()
        sys.exit(1)
    for name in names:
        fetch_one(name.lower().strip())


if __name__ == "__main__":
    main()
