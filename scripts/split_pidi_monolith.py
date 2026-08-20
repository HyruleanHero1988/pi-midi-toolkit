#!/usr/bin/env python3
"""Split apps/pidi/midi_tone.py into the pidi package.

Run from repo root:
  python scripts/split_pidi_monolith.py
"""
from __future__ import annotations

import ast
import pathlib
import textwrap

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEPLOY = ROOT / "apps" / "pidi"
PKG = DEPLOY / "pidi"
SRC = DEPLOY / "midi_tone.py"

SCREEN_PREFIXES: list[tuple[str, tuple[str, ...]]] = [
    ("kaoss", ("_kaoss_", "_build_kaoss", "_open_kaoss", "_close_kaoss", "_paint_kaoss")),
    (
        "songs",
        (
            "_build_songs",
            "_song_",
            "_paint_song",
            "_refresh_song",
            "_select_song",
            "_selected_song",
            "_next_take_path",
        ),
    ),
    (
        "presets",
        (
            "_build_presets",
            "_preset_",
            "_paint_preset",
            "_open_save_preset",
            "_close_save_preset",
            "_save_preset_",
            "_confirm_save_preset",
            "_paint_save_preset",
            "_toggle_save_preset",
            "_reset_save_preset",
            "_update_save_preset",
            "_suggested_preset",
            "_factory_reset_sound",
        ),
    ),
    (
        "pads",
        (
            "_build_pads",
            "_phrase_",
            "_drum_pad_is_phrase",
            "_on_pad_midi",
            "_paint_phrase",
            "_refresh_phrase",
        ),
    ),
    ("seq", ("_build_seq", "_seq_", "_paint_seq", "_refresh_seq", "_finish_seq")),
    (
        "kit",
        (
            "_kit_",
            "_open_kit",
            "_close_kit",
            "_rebuild_kit",
            "_paint_kit",
            "_select_kit",
            "_set_kit",
        ),
    ),
    (
        "fx",
        (
            "_fx_",
            "_open_fx",
            "_close_fx",
            "_exit_fx",
            "_refresh_fx",
            "_toggle_fx",
            "_toggle_bus_fx",
            "_paint_fx",
            "_paint_bus_fx",
            "_paint_drum_lock",
            "_paint_full_vel",
            "_toggle_drum_lock",
        ),
    ),
    (
        "settings",
        (
            "_build_settings",
            "_settings_",
            "_paint_settings",
            "_refresh_settings",
            "_open_update_token",
            "_close_update_token",
            "_paint_token",
            "_toggle_token",
            "_token_type",
            "_save_update_token",
            "_restart_after_update",
        ),
    ),
    ("log", ("_build_log",)),
    ("home", ("_build_home",)),
    (
        "synth",
        (
            "_on_synth_scope",
            "_on_kit_scope",
            "_active_scope",
            "_schedule_scope",
            "_arm_scope",
            "_flush_scope",
            "_paint_synth_waveform",
        ),
    ),
]

CHROME_PREFIXES = (
    "_pack_screen_regions",
    "_build_touch_scroll_area",
    "_bind_touch_scroll_tree",
    "_arm_overlay_guard",
    "_mk_scroll_select_btn",
)


def method_module(name: str) -> str | None:
    if any(name == p or name.startswith(p) for p in CHROME_PREFIXES):
        return "chrome"
    for mod, prefixes in SCREEN_PREFIXES:
        for p in prefixes:
            if name == p or name.startswith(p):
                return mod
    return None


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if not text.endswith("\n"):
        text += "\n"
    path.write_text(text, encoding="utf-8", newline="\n")
    print(f"  {path.relative_to(ROOT)} ({len(text.splitlines())} lines)")


def span(lines: list[str], start: int, end: int) -> str:
    """1-based start inclusive, end exclusive."""
    return "".join(lines[start - 1 : end - 1])


def slice_node(src: str, node: ast.AST) -> str:
    lines = src.splitlines(keepends=True)
    return "".join(lines[node.lineno - 1 : node.end_lineno])


def main() -> None:
    if not SRC.is_file():
        raise SystemExit(f"missing {SRC}")
    print(f"splitting {SRC.relative_to(ROOT)} -> {PKG.relative_to(ROOT)}/")

    src = SRC.read_text(encoding="utf-8")
    lines = src.splitlines(keepends=True)
    tree = ast.parse(src)

    def find_line(prefix: str) -> int:
        for i, ln in enumerate(lines, 1):
            if ln.startswith(prefix):
                return i
        raise SystemExit(f"marker not found: {prefix!r}")

    # --- non-UI modules by line range ---------------------------------------
    write(
        PKG / "constants.py",
        CONST_HEADER
        + span(lines, find_line("SAMPLE_RATE"), find_line("def list_song_files"))
        + span(lines, find_line("CC_MORPH"), find_line("class MixBusFx")),
    )
    write(PKG / "audio" / "__init__.py", '"""Python soft-synth fallback + wavetable helpers."""\n')
    write(
        PKG / "audio" / "fx.py",
        FX_HEADER + span(lines, find_line("class MixBusFx"), find_line("def _builtin_tables")),
    )
    write(
        PKG / "audio" / "wavetable.py",
        WAVETABLE_HEADER
        + span(lines, find_line("def _builtin_tables"), find_line("def apply_tone_lowpass")),
    )
    write(
        PKG / "audio" / "tone.py",
        TONE_HEADER
        + span(lines, find_line("def apply_tone_lowpass"), find_line("def midi_note_name")),
    )
    write(
        PKG / "audio" / "drums.py",
        DRUMS_HEADER
        + span(lines, find_line("def midi_note_name"), find_line("def draw_scope_grid"))
        + span(lines, find_line("def synthesize_drum"), find_line("class Voice")),
    )
    write(
        PKG / "ui" / "scope.py",
        SCOPE_HEADER
        + span(lines, find_line("def draw_scope_grid"), find_line("def synthesize_drum")),
    )
    write(
        PKG / "audio" / "engine.py",
        ENGINE_HEADER + span(lines, find_line("class Voice"), find_line("def phrase_pad_label")),
    )
    write(PKG / "domain" / "__init__.py", '"""Headless musical domain: phrases, songs."""\n')
    write(
        PKG / "domain" / "phrases.py",
        PHRASES_HEADER
        + span(lines, find_line("def phrase_pad_label"), find_line("def _sec_to_ticks")),
    )
    write(
        PKG / "domain" / "songs.py",
        SONGS_HEADER
        + span(lines, find_line("def list_song_files"), find_line("CC_MORPH"))
        + span(lines, find_line("def _sec_to_ticks"), find_line("class MidiToneApp")),
    )

    # --- MidiToneApp method buckets ----------------------------------------
    app_node = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "MidiToneApp")
    buckets: dict[str, list[str]] = {m: [] for m, _ in SCREEN_PREFIXES}
    buckets["chrome"] = []
    buckets["core"] = []

    for stmt in app_node.body:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            mod = method_module(stmt.name) or "core"
        else:
            mod = "core"
        buckets[mod].append(slice_node(src, stmt))

    write(PKG / "ui" / "__init__.py", '"""Tk kiosk UI — PiDI / jambox surface."""\n')
    write(PKG / "ui" / "screens" / "__init__.py", SCREENS_INIT)

    for mod, parts in buckets.items():
        if mod == "core" or not parts:
            continue
        body = "\n\n".join(textwrap.dedent(p) for p in parts)
        body = textwrap.indent(body, "    ")
        if mod == "chrome":
            class_name = "ChromeMixin"
            path = PKG / "ui" / "chrome.py"
        else:
            class_name = "".join(p.title() for p in mod.split("_")) + "ScreenMixin"
            path = PKG / "ui" / "screens" / f"{mod}.py"
        write(
            path,
            f'"""{mod} UI mixin for MidiToneApp."""\nfrom __future__ import annotations\n\n\n'
            f"class {class_name}:\n{body}",
        )

    core_body = "\n\n".join(textwrap.dedent(p) for p in buckets["core"])
    core_body = textwrap.indent(core_body, "    ")
    write(PKG / "ui" / "app.py", APP_HEADER + f"class MidiToneApp(\n{APP_BASES}):\n{core_body}")

    write(PKG / "__init__.py", '"""PiDI — Raspberry Pi MIDI toolkit kiosk UI."""\n\n__version__ = "0.1.0"\n')
    write(PKG / "__main__.py", "from pidi.main import main\n\nif __name__ == '__main__':\n    main()\n")
    write(PKG / "main.py", MAIN_PY)
    write(SRC, SHIM_ROOT)
    write(PKG / "midi_tone.py", SHIM_PKG)

    print("done")


CONST_HEADER = '''\
"""Shared paths, modes, and audio defaults for the PiDI kiosk."""
from __future__ import annotations

import os
import pathlib

# Deploy root (apps/pidi or ~/midi-tone), not the inner package directory.
HERE = pathlib.Path(__file__).resolve().parents[1]

'''

FX_HEADER = '''\
"""Insert / bus FX (drive → delay → short tank)."""
from __future__ import annotations

import math
from typing import Any, Dict

import numpy as np

from pidi.constants import BLOCKSIZE, FX_DELAY_MAX_SEC, FX_REVERB_MAX_SEC

'''

WAVETABLE_HEADER = '''\
"""Wavetable load / normalize / FX sidecars."""
from __future__ import annotations

import pathlib
import re
import wave
from typing import Dict, List, Optional

import numpy as np

from pidi.constants import BUILTIN_VOICE_NAMES, TABLE_PEAK, TABLE_SIZE, VOICE_NAME_MAX

'''

TONE_HEADER = '''\
"""Keys-bus tone (Chamberlin SVF brightness)."""
from __future__ import annotations

import math
from typing import Tuple

import numpy as np

'''

DRUMS_HEADER = '''\
"""Procedural drum models and kit helpers."""
from __future__ import annotations

from typing import Dict, Optional, Tuple

import numpy as np

'''

SCOPE_HEADER = '''\
"""Waveform scope drawing helpers for SYNTH / KIT canvases."""
from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np

if TYPE_CHECKING:
    import tkinter as tk

'''

ENGINE_HEADER = '''\
"""Python PortAudio soft-synth (fallback when jambox-engine is unavailable)."""
from __future__ import annotations

import math
import pathlib
import threading
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

import numpy as np

try:
    import sounddevice as sd
except ImportError as e:  # pragma: no cover
    raise SystemExit(
        "sounddevice required: pip install sounddevice\\n"
        "On Pi you may also need: sudo apt install libportaudio2\\n" + str(e)
    ) from e

from pidi.audio.drums import synthesize_drum
from pidi.audio.fx import MixBusFx
from pidi.audio.tone import apply_tone_lowpass
from pidi.audio.wavetable import (
    load_user_voice_fx_map,
    load_wavetables,
    sanitize_voice_name,
    suggest_voice_name,
    unique_voice_name,
    voice_fx_sidecar_path,
    write_voice_fx_sidecar,
    write_wavetable_wav,
)
from pidi.constants import (
    BLOCKSIZE,
    DEFAULT_MAX_VOICES,
    DRUM_BUS_GAIN,
    DRUM_CHANNEL,
    LATENCY_SEC,
    OUTPUT_MAKEUP,
    SAMPLE_RATE,
    TABLE_MASK,
    TABLE_SIZE,
    USER_WAVETABLES_DIR,
    VOICE_AMP,
)
from pidi.jambox_client import JamboxClient

'''

PHRASES_HEADER = '''\
"""Phrase pad bank (clip-launch grid) — headless."""
from __future__ import annotations

import json
import pathlib
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    MAX_PHRASE_PLAYERS,
    PHRASE_GRID_CELLS,
    PHRASE_PAD_BASE,
    PHRASE_PAD_COUNT,
    PHRASE_TILE_ASSIGN,
    PHRASE_TILE_CLEAR,
    PHRASE_TILE_CLEAR_EMPTY,
    PHRASE_TILE_EMPTY,
    PHRASE_TILE_EMPTY_LOOP,
    PHRASE_TILE_IDLE,
    PHRASE_TILE_MODE,
    PHRASE_TILE_PLAYING,
    PHRASE_TILE_REC,
    PHRASE_TILE_SELECTED,
)

'''

SONGS_HEADER = '''\
"""SMF song library + player — headless transport."""
from __future__ import annotations

import pathlib
import shutil
import threading
import time
from typing import Any, Callable, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    DEMO_SONGS_DIR,
    DEFAULT_SONG_BPM,
    SONG_LIST_VISIBLE,
    SONG_OUT_MODES,
    SONG_OUT_PREFER,
    SONG_SEED_MARKER,
    SONGS_DIR,
)

'''

SCREENS_INIT = '''\
"""Screen mixins composed into MidiToneApp."""
from pidi.ui.screens.fx import FxScreenMixin
from pidi.ui.screens.home import HomeScreenMixin
from pidi.ui.screens.kaoss import KaossScreenMixin
from pidi.ui.screens.kit import KitScreenMixin
from pidi.ui.screens.log import LogScreenMixin
from pidi.ui.screens.pads import PadsScreenMixin
from pidi.ui.screens.presets import PresetsScreenMixin
from pidi.ui.screens.seq import SeqScreenMixin
from pidi.ui.screens.settings import SettingsScreenMixin
from pidi.ui.screens.songs import SongsScreenMixin
from pidi.ui.screens.synth import SynthScreenMixin

__all__ = [
    "FxScreenMixin",
    "HomeScreenMixin",
    "KaossScreenMixin",
    "KitScreenMixin",
    "LogScreenMixin",
    "PadsScreenMixin",
    "PresetsScreenMixin",
    "SeqScreenMixin",
    "SettingsScreenMixin",
    "SongsScreenMixin",
    "SynthScreenMixin",
]
'''

APP_BASES = """    HomeScreenMixin,
    SynthScreenMixin,
    SeqScreenMixin,
    PadsScreenMixin,
    KaossScreenMixin,
    SongsScreenMixin,
    PresetsScreenMixin,
    LogScreenMixin,
    SettingsScreenMixin,
    KitScreenMixin,
    FxScreenMixin,
    ChromeMixin,
"""

APP_HEADER = '''\
"""PiDI kiosk application shell (Tk)."""
from __future__ import annotations

import json
import math
import os
import pathlib
import queue
import shutil
import subprocess
import sys
import threading
import time
import tkinter as tk
from tkinter import ttk
from typing import Any, Dict, List, Optional, Tuple

try:
    import numpy as np
except ImportError as e:  # pragma: no cover
    sys.exit("numpy required: pip install numpy (or apt install python3-numpy)\\n" + str(e))

try:
    import mido
except ImportError as e:  # pragma: no cover
    sys.exit("mido required: pip install mido python-rtmidi\\n" + str(e))

from pidi import updater
from pidi.audio.drums import (
    downsample_waveform,
    drum_model_for_note,
    mpk_note_for_phrase_cell,
    render_drum_preview,
)
from pidi.audio.engine import SineEngine
from pidi.audio.wavetable import load_wavetables
from pidi.constants import *  # noqa: F403
from pidi.domain.phrases import (
    PhraseCell,
    PhrasePadBank,
    clamp_phrase_gain,
    phrase_cell_for_note,
    phrase_pad_label,
    phrase_pad_tile_color,
    scale_velocity,
)
from pidi.domain.songs import (
    SongPlayer,
    list_song_files,
    pick_song_output_name,
    seed_demo_songs,
)
from pidi.jambox_client import (
    JamboxClient,
    connect_or_spawn,
    midi_notice_to_message,
    prefer_python_engine,
)
from pidi.kaoss import (
    GATE_PATTERNS,
    KAOSS_OUT_MODES,
    LED_COLS,
    LED_ROWS,
    PROGRAM_BY_ID,
    ROOT_OCTAVE_MIDI,
    SCALE_BY_ID,
    VIZ_STYLE_LABELS,
    VIZ_STYLES,
    KaossEvent,
    KaossPad,
    KaossProgram,
    clamp01,
    glow_radii,
    glow_step,
    grid_line_widths,
    hsv_to_rgb,
    note_grid_xs,
    note_name as kaoss_note_name,
    pad_led_hex,
    program_hue,
    rgb_hex,
)
from pidi.screensaver import (
    PIXEL_SHIFT_AMPLITUDE,
    IdleWatch,
    PanelBacklight,
    next_timeout_preset,
    orbit_xy,
    pixel_shift_xy,
    timeout_from_env,
    timeout_label,
)
from pidi.sequencer import (
    SEQ_EMPTY,
    SEQ_OVERDUB,
    SEQ_REC_BACKBONE,
    LoopEvent,
    OverdubSequencer,
    trim_loop_take,
)
from pidi.ui.chrome import ChromeMixin
from pidi.ui.scope import blank_waveform_on_canvas, draw_scope_grid, draw_waveform_on_canvas
from pidi.ui.screens import (
    FxScreenMixin,
    HomeScreenMixin,
    KaossScreenMixin,
    KitScreenMixin,
    LogScreenMixin,
    PadsScreenMixin,
    PresetsScreenMixin,
    SeqScreenMixin,
    SettingsScreenMixin,
    SongsScreenMixin,
    SynthScreenMixin,
)


def format_message(msg: mido.Message) -> str:
    if msg.type == "note_on":
        return f"Note On   ch{msg.channel + 1}  n{msg.note}  vel {msg.velocity}"
    if msg.type == "note_off":
        return f"Note Off  ch{msg.channel + 1}  n{msg.note}"
    if msg.type == "control_change":
        return f"CC        ch{msg.channel + 1}  cc{msg.control}  {msg.value}"
    if msg.type == "pitchwheel":
        return f"PitchBend ch{msg.channel + 1}  {msg.pitch}"
    if msg.type == "program_change":
        return f"Program   ch{msg.channel + 1}  prog {msg.program}"
    if msg.type == "aftertouch":
        return f"AT        ch{msg.channel + 1}  {msg.value}"
    if msg.type == "polytouch":
        return f"PolyAT    ch{msg.channel + 1}  n{msg.note}  {msg.value}"
    return str(msg)


'''

MAIN_PY = '''\
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
        fp.write(f"{os.getpid()}\\n")
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
'''

SHIM_ROOT = '''\
#!/usr/bin/env python3
"""Deploy-root entrypoint (``python midi_tone.py``). Prefer ``python -m pidi``."""
from __future__ import annotations

import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parent
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

from pidi.midi_tone import *  # noqa: F401,F403
from pidi.main import main

if __name__ == "__main__":
    main()
'''

SHIM_PKG = '''\
"""Compatibility re-exports for tests that ``import midi_tone``."""
from __future__ import annotations

from pidi.audio.drums import *  # noqa: F401,F403
from pidi.audio.engine import DrumHit, SineEngine, Voice  # noqa: F401
from pidi.audio.fx import MixBusFx  # noqa: F401
from pidi.audio.tone import apply_tone_lowpass  # noqa: F401
from pidi.audio.wavetable import *  # noqa: F401,F403
from pidi.constants import *  # noqa: F401,F403
from pidi.domain.phrases import *  # noqa: F401,F403
from pidi.domain.songs import *  # noqa: F401,F403
from pidi.main import main  # noqa: F401
from pidi.sequencer import SEQ_EMPTY, SEQ_OVERDUB, SEQ_REC_BACKBONE  # noqa: F401
from pidi.ui.app import MidiToneApp, format_message  # noqa: F401
from pidi.ui.scope import blank_waveform_on_canvas, draw_scope_grid, draw_waveform_on_canvas  # noqa: F401
'''


if __name__ == "__main__":
    main()
