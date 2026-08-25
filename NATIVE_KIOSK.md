# Native jambox kiosk

Status: **in progress on `cursor/native-kiosk-port`**

Greenfield Rust UI for the Pi TFT70. This branch ignores the Python/Tk kiosk
entirely: `jambox-engine` + `pidi-native` are the appliance.

## Architecture

- `pidi-native` — SDL/KMSDRM + GLES2, mode shell, touch (prefer `--touch evdev`)
- `jambox-engine` — audio, MIDI, transport, clips, KAOSS, repeats
- On-disk phrases/presets/wavetables stay readable; adapters live in Rust

## Modes

| Mode | Status |
|---|---|
| KAOSS + drums | Working — scale/key cycle, FULL PAD, CELLS |
| Nav shell / HOME | Working |
| PADS | Launch/stop from `phrases/pad-XX.json` |
| SYNTH | Morph/tone/level/atk/rel sliders + C4–B4 keys |
| SEQ / SONGS / PRESETS / LOG / SET | Placeholders |

## Run (host)

```bash
cargo test -p pidi-native -p jambox-protocol -p jambox-core
cargo run -p jambox-engine -- run --null-audio --tcp --control 127.0.0.1:17890
cargo run -p pidi-native -- --tcp --control 127.0.0.1:17890 --display dummy --frames 30
```

## Run (Pi)

```bash
# LightDM must not own the panel
sudo systemctl stop lightdm   # or keep it masked
# unit uses --display sdl --touch evdev
sudo systemctl restart jambox-engine pidi-native
```

Phrase bank: `PIDI_PHRASES_DIR` or `--phrases /path` (defaults to `./phrases`).

## Touch notes

Use type-B `ABS_MT_*` only. Legacy `ABS_X`/`ABS_Y` are ignored so a second
contact cannot corrupt slot 0. Prefer FT5x06/Goodix over ADS7846 when
autodetecting.
