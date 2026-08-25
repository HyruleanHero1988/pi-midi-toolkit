# Native jambox kiosk

Status: **greenfield on `cursor/native-kiosk-port`**

Rust-only appliance UI for the Pi TFT70. `jambox-engine` + `pidi-native` are the
runtime — no Python/Tk on this branch.

## Architecture

- `pidi-native` — SDL/KMSDRM + GLES2, mode shell, touch (prefer `--touch evdev`)
- `jambox-engine` — audio, MIDI, transport, clips, KAOSS, repeats
- On-disk phrases/presets/songs stay readable; adapters live in Rust

## Modes

| Mode | Status |
|---|---|
| KAOSS + drums | Scale/key/oct/gate pickers, HOLD, FULL PAD, CELLS/GLOW, trail/ripples, WIPE FX, CH, OUT cycle |
| Nav shell / HOME | Mode tiles + bottom nav |
| PADS | Launch/stop from `phrases/pad-XX.json`; PLAY/EDIT; REC/TRIG/MODE/CLEAR; SEQ→PAD; OUT cycle |
| SYNTH | Morph A/B wave pick, tone/level/atk/rel, vibrato, scope, kit macros, C4–B4 keys |
| SEQ | Backbone REC → engine loop clip; KEEP/DROP/UNDO; len×2/÷2/EXTEND; →PAD; PLAY/STOP/CLEAR/BPM |
| SONGS | List `songs/*.mid`, SMF→clip PLAY/STOP/LOOP, SAVE SEQ, OUT cycle |
| PRESETS | 8 slots save/load synth params to `user-presets/` |
| SETTINGS | Panic, all-notes-off, bus/voice/drum-group FX; WIFI/UPDATE stubs (status/log → `deploy/`) |
| LOG | Engine counters + recent action lines |
| Session | Autosave `settings.json` (synth/kaoss/tempo/OUT prefs) |

## Intentional non-parity (vs Tk)

Not ported on purpose: Map/Thru modes, Kaoss CC→DIN OUT routing,
edge-hold full-pad exit (native uses FULL PAD button), SAVE AS voice bake.
WIFI/UPDATE are UI stubs only (no on-device GitHub/Wi-Fi stack) — use
`deploy/build-pi-bins.sh` / `deploy/deploy.sh` (or host SET→UPDATE path).

OUT: LOCAL / USB / BOTH is a stored preference per pads/songs/kaoss; the engine
may still always play local audio.

## Run (host)

```bash
cargo test -p pidi-native -p jambox-protocol -p jambox-core
cargo run -p jambox-engine -- run --null-audio --tcp --control 127.0.0.1:17890
cargo run -p pidi-native -- --tcp --control 127.0.0.1:17890 --display dummy --frames 30
```

## Pi appliance

Cross-build (WSL Ubuntu 22.04 / Debian, or CI):

```bash
PACKAGES=jambox-engine,pidi-native ./deploy/build-pi-bins.sh
```

Deploy bins to `~/pi-midi-toolkit/bin/`, then:

```bash
sudo systemctl stop lightdm
sudo systemctl mask lightdm   # keep KMSDRM free
sudo systemctl enable --now jambox-engine pidi-native
```

Unit defaults: `--display sdl --touch evdev`. Optional:

- `--evdev /dev/input/event4` if autodetect picks ADS7846
- `--phrases /path/to/phrases`
- `PIDI_PRESETS_DIR`, `PIDI_SONGS_DIR`

## Touch notes

Type-B `ABS_MT_*` only. Legacy `ABS_X`/`ABS_Y` ignored. Prefer FT5x06/Goodix
over ADS7846 when autodetecting.
