#!/usr/bin/env python3
"""Convert binary P6 PPM frames (from dump_docs) into PNG without extra deps."""

from __future__ import annotations

import argparse
import pathlib
import struct
import zlib


def ppm_to_png(ppm_path: pathlib.Path, png_path: pathlib.Path) -> tuple[int, int]:
    data = ppm_path.read_bytes()
    if not data.startswith(b"P6"):
        raise ValueError(f"{ppm_path} is not a P6 PPM")
    header, rest = data.split(b"\n", 1)
    parts = header.split()
    if len(parts) < 4:
        raise ValueError(f"{ppm_path}: bad PPM header {header!r}")
    width, height, maxval = int(parts[1]), int(parts[2]), int(parts[3])
    if maxval != 255:
        raise ValueError(f"{ppm_path}: expected maxval 255, got {maxval}")
    raw = rest
    expected = width * height * 3
    if len(raw) != expected:
        raise ValueError(f"{ppm_path}: expected {expected} RGB bytes, got {len(raw)}")

    def chunk(tag: bytes, payload: bytes) -> bytes:
        crc = zlib.crc32(tag + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", crc)

    rows = bytearray()
    stride = width * 3
    for y in range(height):
        rows.append(0)
        rows.extend(raw[y * stride : (y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 9))
    png += chunk(b"IEND", b"")
    png_path.write_bytes(png)
    return width, height


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=pathlib.Path)
    parser.add_argument("--keep-ppm", action="store_true")
    args = parser.parse_args()
    files = sorted(args.directory.glob("*.ppm"))
    if not files:
        raise SystemExit(f"no PPM files in {args.directory}")
    for ppm in files:
        png = ppm.with_suffix(".png")
        w, h = ppm_to_png(ppm, png)
        print(f"wrote {png.name} ({w}x{h})")
        if not args.keep_ppm:
            ppm.unlink()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
