# Archived: Python Tk PiDI kiosk

**This branch is a frozen snapshot.** Do not land new kiosk features here.

It captures the last complete **Python / Tk** PiDI kiosk as of
`master` at `683de10` (2026-08-22) — PR #24, `refactor/pidi-ui-architecture`.

The live UI is moving to a **native** kiosk. Keep this branch as the recovery
point if you need the Tk surface, the Python PortAudio fallback, or the
`python -m pidi` session.

## What is frozen

| Path | Role |
|------|------|
| [`apps/pidi`](apps/pidi) | Python Tk kiosk (product version **0.2.0**) |
| `apps/pidi/pidi/` | Package: screens, chrome, MIDI I/O, Kaoss, sequencer, updater |
| `apps/pidi/pidi/audio/` | In-process PortAudio soft-synth fallback |
| `crates/jambox-engine` | Rust audio + sequencer daemon the kiosk already talks to |
| `crates/midi-engine` | MIDI thru / remap CLI |

`tools/midi-tone` is only a pointer to `apps/pidi`.

## Run the archived kiosk

```bash
cd apps/pidi
./setup-venv.sh
export PYTHONPATH="$PWD"
python -m pidi --fullscreen
# or:
./run.sh --input MPK
```

Headless tests (no Pi, no audio device):

```bash
cd apps/pidi
python3 -m unittest tests.test_sequencer tests.test_kaoss tests.test_phrase_pads \
    tests.test_synth_vibrato tests.test_jambox_client tests.test_screensaver \
    tests.test_updater
```

Tk UI tests need Xvfb:

```bash
cd apps/pidi
xvfb-run -a python3 -m unittest tests.test_ui_seq tests.test_ui_kaoss \
    tests.test_ui_screensaver tests.test_ui_settings
```

## Checkout later

```bash
git fetch origin
git checkout cursor/python-kiosk-archive-dfc2
```
