"""Shared paths, modes, and audio defaults for the PiDI kiosk."""
from __future__ import annotations

import os
import pathlib

# Deploy root (apps/pidi or ~/midi-tone), not the inner package directory.
HERE = pathlib.Path(__file__).resolve().parents[1]


def _read_app_version() -> str:
    path = HERE / "VERSION"
    try:
        text = path.read_text(encoding="utf-8").strip().splitlines()[0].strip()
        if text:
            return text
    except (OSError, IndexError):
        pass
    return "0.0.0"


APP_VERSION = _read_app_version()

SAMPLE_RATE = 44100
# Larger default block / latency = fewer xruns on Pi (crunchy / robotic audio).
# Override for lower latency if the machine can take it:
#   MIDI_TONE_BLOCKSIZE=512 MIDI_TONE_LATENCY=0.03
# Or bump further on a loaded Pi:
#   MIDI_TONE_BLOCKSIZE=1024 MIDI_TONE_LATENCY=0.08
BLOCKSIZE = max(64, int(os.environ.get("MIDI_TONE_BLOCKSIZE", "1536")))
LATENCY_SEC = max(0.005, float(os.environ.get("MIDI_TONE_LATENCY", "0.10")))
DEFAULT_MAX_VOICES = 12
TABLE_SIZE = 2048
TABLE_MASK = TABLE_SIZE - 1
LOG_MAX = 60
# Full-pad: shove into the bottom edge and stay still this long to peek controls.
KAOSS_PLAY_EXIT_MS = 700
KAOSS_PLAY_BORDER_PX = 40
KAOSS_PLAY_HOLD_SLOP_PX = 18
EVENT_Q_MAX = 200
# MIDI channel 10 (1-based) = index 9 — MPK drum pads
DRUM_CHANNEL = 9
NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
DEFAULT_WAVETABLE_DIR = HERE / "wavetables"
USER_WAVETABLES_DIR = HERE / "user-wavetables"
SETTINGS_PATH = HERE / "settings.json"
PRESETS_DIR = HERE / "user-presets"
PRESET_SLOTS = 8
SONGS_DIR = HERE / "songs"
DEMO_SONGS_DIR = HERE / "demo-songs"
SONG_SEED_MARKER = SONGS_DIR / ".seeded-from-demo"
SONG_LIST_VISIBLE = 4  # chunky rows on screen at once
DEFAULT_SONG_BPM = 120
SONG_OUT_MODES = ("local", "usb", "both")
# Prefer these substrings when auto-picking USB→DIN output
SONG_OUT_PREFER = ("u2midi", "uxmidi", "hxmidi", "din", "usb midi", "midi out", "mio", "um-")
PHRASES_DIR = HERE / "phrases"
PHRASE_PAD_COUNT = 16  # MPK Bank A + Bank B
PHRASE_PAD_BASE = 36   # factory MPC program: pads 36–51
# Visual 4×4 (top→bottom): A1–A4, A5–A8, B1–B4, B5–B8
# MPK hardware has pads 5–8 on top and 1–4 on bottom; note↔cell swaps those rows
# so the top-left screen pad (A1) matches the top-left MPK pad (note 40).
PHRASE_GRID_CELLS = (
    0, 1, 2, 3,      # Bank A 1–4 (top row)
    4, 5, 6, 7,      # Bank A 5–8
    8, 9, 10, 11,    # Bank B 1–4
    12, 13, 14, 15,  # Bank B 5–8
)
MAX_PHRASE_PLAYERS = 8
SETTINGS_VERSION = 2
BUILTIN_VOICE_NAMES = ("sine", "square", "saw", "triangle")
VOICE_NAME_MAX = 24
TOUCH_SCROLL_THRESH_PX = 10  # press→drag before a grid tap counts as scroll (capacitive)
UI_MODES = ("home", "synth", "seq", "pads", "kaoss", "songs", "presets", "log", "settings")
JAM_NAV_MODES = ("synth", "seq", "pads", "kaoss")
HOME_TILES = (
    ("synth", "SYNTHESIZER", "#458588"),
    ("seq", "SEQUENCER", "#b16286"),
    ("pads", "PADS", "#d79921"),
    ("kaoss", "KAOSS", "#fe8019"),
    ("songs", "SONGS", "#689d6a"),
    ("presets", "PRESETS", "#83a598"),
    ("log", "LOG", "#504945"),
    ("settings", "SETTINGS", "#665c54"),
)


CC_MORPH = 70          # Knob 1 — scan / blend through wavetable stack
CC_TONE = 71           # Knob 2 — brightness (low-pass)
CC_ATTACK = 72         # Knob 3 — amp attack
CC_RELEASE = 73        # Knob 4 — amp release
CC_VIB_DEPTH = 74      # Knob 5 — vibrato depth
CC_VIB_RATE = 75       # Knob 6 — vibrato rate
# Touch steps for the VOICES-screen vibrato pair
VIB_DEPTH_STEP = 0.10  # semitones
VIB_RATE_STEP = 0.5    # Hz
CC_LEVEL = 77          # Knob 8 — output level
# Knob 7 (CC76) reserved / free for later
KNOB_CCS = {CC_MORPH, CC_TONE, CC_ATTACK, CC_RELEASE, CC_VIB_DEPTH, CC_VIB_RATE, CC_LEVEL}


# Peak scale for wavetable cycles (was 0.35 — far too quiet into the Pi jack)
TABLE_PEAK = 0.90
# Per-voice amp at velocity 127 (was 0.12 → ~-28 dBFS with old table scale)
VOICE_AMP = 0.48
# Extra mix bus makeup before soft-limit (Pi headphone/line outs are timid)
OUTPUT_MAKEUP = 1.65
DRUM_BUS_GAIN = 1.55
# Mix-bus FX delay line (~1s) and short reverb tank
FX_DELAY_MAX_SEC = 1.0
FX_REVERB_MAX_SEC = 0.55

# CRT-style waveform scopes (SYNTH / KIT)
SCOPE_CRT_BG = "#031a08"
SCOPE_CRT_WAVE = "#39ff14"
SCOPE_CRT_GRID = "#14532d"
SCOPE_CRT_AXIS = "#4ade80"
SCOPE_REDRAW_DEBOUNCE_S = 0.04
SCOPE_REDRAW_MAX_WAIT_S = 0.10
SCOPE_MORPH_CYCLES = 3
DRUM_SCOPE_SEC = 0.40

