# Handoff — pi-midi-toolkit

Saved: 2026-07-12 (Phase 1 MVP software largely complete)

Open this folder as the Cursor workspace: `c:\Users\Raymond\Documents\Development\pi-midi-toolkit`

## Plan

[PLAN.md](PLAN.md) and [`.cursor/plans/pi_midi_toolkit_7f45b204.plan.md`](.cursor/plans/pi_midi_toolkit_7f45b204.plan.md).

## Hardware

- **Pi 2 Model B** + touchscreen (UI later)
- First I/O: **MPK mini mk3 USB** → Pi → **USB-MIDI-to-DIN** → synth
- Use a powered hub if USB devices brown out

## Phase 1 status (software)

Done:

- `midi-core`: event / preset (validated) / process (fixed CC table + inline velocity curve) / stuck
- `midi-engine`: `list` | `run` (--watch, --rt) | `learn` | `test` | `latency`
- Atomic preset publish via `arc-swap`; stuck flush on reload/exit
- Presets: `example.json`, `mpk-mini-ch3.json`
- `deploy/setup-pi.sh`, `deploy.sh`, `deploy.ps1`, systemd unit with `--rt`
- `.cargo/config.toml.example` for armv7 cross notes
- `cargo test` / `latency` on Windows host

Still needs your hardware / machine:

1. Plug MPK + USB-DIN → `list` / `test` / `run` (PC or Pi)
2. Install armv7 linker or `cross`, then first `deploy` to the Pi
3. On Pi: `setup-pi.sh`, confirm ALSA names, `systemctl status midi-engine`

## Phase 0 status

`tools/midi-tone` — Python sine + Tk event UI. Run on Pi desktop; see `tools/midi-tone/README.md`.

## Host setup

Rust **1.97.0** at `%USERPROFILE%\.cargo\bin` — add to PATH in new shells.

## Not related to play-my-synth

Standalone project; no shared code with play-my-synth.
