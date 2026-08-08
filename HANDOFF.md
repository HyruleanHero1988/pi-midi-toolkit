# Handoff — pi-midi-toolkit

Updated: 2026-08-08

Open this folder as the Cursor workspace.

## Plan

[PLAN.md](PLAN.md) — **read the “Product north star” section first.**

## Product direction

**One kiosk MIDI appliance.** Boot → Openbox kiosk → shared UI with modes:

- **Synth / Looper / Log** — live in `tools/midi-tone` (playable soft-synth)
- **Map / Thru** — not in the kiosk yet; Rust `midi-engine` already does remap via CLI/JSON

Playing notes is intentional, not a throwaway diagnostic. Mapper stays required for when USB-DIN → hardware synth is available; same box / same UI shell.

## Hardware

- **Pi 2 Model B** + touchscreen
- **In:** MPK mini mk3 USB
- **Out (mapper path):** USB-MIDI-to-DIN → hardware synth (when you have it)
- Powered hub if USB devices brown out

## What’s active now

`tools/midi-tone` — Tk kiosk UI, wavetables, morph, looper, modes, `kiosk.sh` / `install-kiosk.sh`.

See `tools/midi-tone/README.md`.

## Phase 1 engine (mapper) status

Done (CLI / headless):

- `midi-core` + `midi-engine` remap thru, learn, presets, stuck-note flush, `--rt`, deploy scripts

Still needs hardware / machine:

1. MPK + USB-DIN → `list` / `test` / `run`
2. armv7 cross build + deploy to Pi
3. Later: **Map mode in the kiosk** talking to `midi-engine` (don’t launch a second app)

## Host setup

Rust toolchain on PATH for engine work. midi-tone is Python venv on the Pi.

## Not related to play-my-synth

Standalone project; no shared code with play-my-synth.
