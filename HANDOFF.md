# Handoff — pi-midi-toolkit

Updated: 2026-08-10

Open this folder as the Cursor workspace.

## Plan

[PLAN.md](PLAN.md) — **read “Product north star (honest, dual purpose)” first.**

## Product direction

**Dual-purpose music box** (~50/50):

| Pillar | What |
|--------|------|
| **Jambox / soft-synth** | Create and perform: morph synth, drum kit, looper, phrase pads, songs, presets — mental models you already use, not another tracker to learn |
| **MIDI remap appliance** | USB MIDI in → transform → USB/DIN out when hardware synths matter |

One kiosk UI. Soft-synth started as a Phase 0 hear-test; that framing is obsolete — it is already used for real performances (beat + melody + voice). Remap remains the other half, not a demotion of the jambox.

Modes today in `tools/midi-tone`:

- **Synth / Looper / Pads / Songs / Presets / Log**
- **Map / Thru** — not in the kiosk yet; Rust `midi-engine` already does remap via CLI/JSON

**Design law (jambox):** stay obvious under the hands. Winning vs gear you already own (picotracker, EP-class boxes) is *learning cost*, not feature count.

**Jambox FX:** layered in Python `midi-tone` — **FX MODE** (per-wavetable / per-drum inserts + shared **ALL DRUMS** kit bus) and **BUS FX** (optional master mix wet).

**Save voice:** VOICES / MORPH → **SAVE AS…** bakes morph + drive + tone into `.wav`, and keeps delay/reverb in a tiny `.fx.json` beside it.

**Next (architecture):** Rust jambox engine for audio + sample-accurate sequencing; Tk becomes a thin client. Jam on FX in Python first so the rewrite targets a known sound.

## Hardware

- **Pi 2 Model B** (today) — upgrade path to Pi 4/5 if DSI/audio ceiling hits
- **Display (ordered):** BigTreeTech **Pi TFT70 V2.1** — 7″, 800×480, DSI, capacitive GT911 (replaces resistive ADS7846 HDMI panel). See PLAN “Display / touch bring-up”
- **In:** MPK mini mk3 USB
- **Out (mapper / song / pad emit):** USB-MIDI-to-DIN → hardware synth (when you have it)
- Powered hub if USB devices brown out

## What’s active now

`tools/midi-tone` — Tk kiosk UI, wavetables, morph, drums, looper, pads, songs, presets/session JSON, scopes, `kiosk.sh` / `install-kiosk.sh`.

See `tools/midi-tone/README.md`. Session: `settings.json`; slots: `user-presets/`; phrases: `phrases/`.

Branch stack (recent): drum voices → phrase pads → pad enhance → wave viz → plan north-star.

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
