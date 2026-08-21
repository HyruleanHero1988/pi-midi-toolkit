#!/usr/bin/env python3
"""Fetch Mutopia Project MIDI demos into ./songs (Public Domain / CC).

A classical pack already ships in ./demo-songs/ and is copied into ./songs/
on launch (no network). Use this script only when online to refresh or
pull individual aliases.

Sources: https://www.mutopiaproject.org/

Usage:
  ./venv/bin/python fetch_songs.py --list
  ./venv/bin/python fetch_songs.py --all              # full catalog → alias.mid
  ./venv/bin/python fetch_songs.py fur-elise maple-leaf
  ./venv/bin/python fetch_songs.py --all --force      # overwrite
"""

from __future__ import annotations

import argparse
import io
import pathlib
import sys
import urllib.request

try:
    import mido
except ImportError:
    sys.exit("mido required: pip install mido\n")

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "songs"
BASE = "https://www.mutopiaproject.org"

# alias -> (ftp-relative path, title, license note)
CATALOG: dict[str, tuple[str, str, str]] = {
    "bach-prelude": (
        "ftp/BachJS/BWV846/wtk1-prelude1/wtk1-prelude1.mid",
        "Bach WTC I Prelude 1",
        "Public Domain (Mutopia)",
    ),
    "fur-elise": (
        "ftp/BeethovenLv/WoO59/fur_Elise_WoO59/fur_Elise_WoO59.mid",
        "Beethoven Für Elise",
        "Public Domain (Mutopia)",
    ),
    "ode-to-joy": (
        "ftp/BeethovenLv/ode/ode.mid",
        "Beethoven Ode to Joy",
        "Public Domain (Mutopia)",
    ),
    "greensleeves": (
        "ftp/Traditional/greensleeves/greensleeves.mid",
        "Greensleeves",
        "Public Domain (Mutopia)",
    ),
    "maple-leaf": (
        "ftp/JoplinS/maple/maple.mid",
        "Joplin Maple Leaf Rag",
        "Public Domain (Mutopia)",
    ),
    "clair-de-lune": (
        "ftp/DebussyC/L75/debussy_Ste_Bergamesq_Clair/debussy_Ste_Bergamesq_Clair.mid",
        "Debussy Clair de Lune",
        "Public Domain (Mutopia)",
    ),
    "mozart-facile": (
        "ftp/MozartWA/KV545/K545-1/K545-1.mid",
        "Mozart Sonata Facile I",
        "Public Domain (Mutopia)",
    ),
    "pachelbel": (
        "ftp/PachelbelJ/CanonInD/CanonInD.mid",
        "Pachelbel Canon in D",
        "Public Domain (Mutopia)",
    ),
    "bach-air": (
        "ftp/BachJS/BWV1068/bach_air_bmv_1068/bach_air_bmv_1068.mid",
        "Bach Air on the G String",
        "Public Domain (Mutopia)",
    ),
    "arbeau-belle": (
        "ftp/ArbeauT/Orch/belle/belle.mid",
        "Arbeau Belle qui tiens ma Vie",
        "Public Domain (Mutopia)",
    ),
    "blue-danube": (
        "ftp/StraussJJ/O314/blue_danube/blue_danube.mid",
        "Strauss Blue Danube (theme)",
        "CC-BY-SA 4.0 (Mutopia)",
    ),
    "satie-gym1": (
        "ftp/SatieE/gymnopedie_1/gymnopedie_1.mid",
        "Satie Gymnopédie 1",
        "Public Domain (Mutopia)",
    ),
}

# Bundled offline in demo-songs/ (same set)
BUNDLED = tuple(CATALOG.keys())


def download(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "midi-tone-fetch_songs/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        data = resp.read()
    if len(data) < 32:
        raise ValueError(f"too small ({len(data)} bytes)")
    if data[:4] != b"MThd":
        raise ValueError("not a Standard MIDI File (missing MThd)")
    return data


def stamp_title(raw: bytes, title: str) -> bytes:
    """Ensure the SMF carries a track/sequence name for the Songs UI label."""
    mid = mido.MidiFile(file=io.BytesIO(raw))
    if not mid.tracks:
        mid.tracks.append(mido.MidiTrack())
    track = mid.tracks[0]
    cleaned = [
        m
        for m in track
        if not (m.is_meta and m.type in ("track_name", "sequence_name"))
    ]
    track.clear()
    track.append(mido.MetaMessage("track_name", name=title[:48], time=0))
    track.extend(cleaned)
    buf = io.BytesIO()
    mid.save(file=buf)
    return buf.getvalue()


def write_song(path: pathlib.Path, raw: bytes, title: str, *, force: bool) -> bool:
    if path.exists() and not force:
        print(f"skip {path.name} (exists; use --force)")
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    stamped = stamp_title(raw, title)
    tmp = path.with_suffix(".mid.tmp")
    tmp.write_bytes(stamped)
    tmp.replace(path)
    print(f"wrote {path.name}  ← {title}  ({len(stamped)} bytes)")
    return True


def fetch_alias(alias: str, dest: pathlib.Path, *, force: bool) -> bool:
    if alias not in CATALOG:
        print(f"unknown alias: {alias}", file=sys.stderr)
        return False
    rel, title, license_note = CATALOG[alias]
    url = f"{BASE}/{rel}"
    print(f"fetch {alias}: {url}")
    print(f"  license: {license_note}")
    try:
        raw = download(url)
    except Exception as exc:
        print(f"  FAILED: {exc}", file=sys.stderr)
        return False
    return write_song(dest, raw, title, force=force)


def main() -> None:
    parser = argparse.ArgumentParser(description="Fetch Mutopia MIDI demos into ./songs")
    parser.add_argument("aliases", nargs="*", help="Catalog aliases to fetch")
    parser.add_argument("--list", action="store_true", help="List catalog and exit")
    parser.add_argument(
        "--all",
        action="store_true",
        help=f"Download the full {len(BUNDLED)}-song classical pack (alias.mid names)",
    )
    parser.add_argument(
        "--starter",
        action="store_true",
        help="Same as --all (kept for older docs/scripts)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Overwrite existing files",
    )
    parser.add_argument(
        "--out-dir",
        type=pathlib.Path,
        default=OUT,
        help="Output directory (default: ./songs)",
    )
    args = parser.parse_args()
    out_dir = args.out_dir

    if args.list:
        print("alias                  license                       title")
        print("-----                  -------                       -----")
        for alias, (_rel, title, lic) in CATALOG.items():
            mark = " *" if alias in BUNDLED else "  "
            print(f"{alias:20s}{mark} {lic:28s}  {title}")
        print()
        print(f"* = bundled offline in demo-songs/ ({len(BUNDLED)} files)")
        print(f"Source: {BASE}")
        return

    want_all = args.all or args.starter
    if not want_all and not args.aliases:
        parser.print_help()
        print("\nTip: ./venv/bin/python fetch_songs.py --all")
        print("Offline pack already ships in demo-songs/ (no network needed).")
        return

    aliases = list(BUNDLED) if want_all else []
    for a in args.aliases:
        if a not in aliases:
            aliases.append(a)

    ok = 0
    for alias in aliases:
        dest = out_dir / f"{alias}.mid"
        if fetch_alias(alias, dest, force=args.force):
            ok += 1

    print(f"done — {ok} file(s) written under {out_dir}")
    print("In midi-tone: open SONGS, scroll, tap a file, then PLAY.")


if __name__ == "__main__":
    main()
