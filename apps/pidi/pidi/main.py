"""CLI entry: ``python -m pidi``."""
from __future__ import annotations

import argparse
import os
import pathlib
import sys

from pidi.constants import DEFAULT_MAX_VOICES, DEFAULT_WAVETABLE_DIR
from pidi.ui.app import MidiToneApp


def _acquire_singleton_lock():
    import fcntl

    path = pathlib.Path(os.environ.get("MIDI_TONE_LOCK", "/tmp/midi-tone.lock"))
    fp = open(path, "a+", encoding="utf-8")
    try:
        fcntl.flock(fp.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("pidi: already running (singleton lock) — exiting this instance", flush=True)
        sys.exit(0)
    try:
        fp.seek(0)
        fp.truncate()
        fp.write(f"{os.getpid()}\n")
        fp.flush()
    except Exception:
        pass
    return fp


def main() -> None:
    import faulthandler

    faulthandler.enable()
    parser = argparse.ArgumentParser(description="PiDI — MIDI soft-synth kiosk UI")
    parser.add_argument("--input", "-i", default="", help="MIDI input name substring")
    parser.add_argument("--list", "-l", action="store_true", help="List MIDI inputs")
    parser.add_argument("--voices", type=int, default=DEFAULT_MAX_VOICES)
    parser.add_argument("--waves-dir", type=pathlib.Path, default=DEFAULT_WAVETABLE_DIR)
    parser.add_argument("--fullscreen", action="store_true")
    args = parser.parse_args()

    lock_fp = None
    if not args.list and sys.platform.startswith("linux"):
        lock_fp = _acquire_singleton_lock()

    try:
        import mido

        mido.set_backend("mido.backends.rtmidi")
    except Exception:
        pass

    print("pidi: starting", flush=True)
    app = MidiToneApp(
        port_filter=args.input,
        list_only=args.list,
        max_voices=args.voices,
        waves_dir=args.waves_dir,
        fullscreen=args.fullscreen,
    )
    if not args.list:
        print("pidi: entering mainloop", flush=True)
        try:
            app.run()
        finally:
            if lock_fp is not None:
                try:
                    lock_fp.close()
                except Exception:
                    pass
        print("pidi: mainloop exited", flush=True)


if __name__ == "__main__":
    main()
