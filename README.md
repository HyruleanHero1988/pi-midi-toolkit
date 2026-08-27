# pi-midi-toolkit

Raspberry Pi **MIDI appliance**: one kiosk UI for local soft-synth play **and**
low-latency MIDI thru/remap to a hardware synth. **Not** related to play-my-synth.

**North star:** power on → kiosk → modes (Synth / Seq / Pads / Kaoss / Log / Map). See [PLAN.md](PLAN.md).

- **Kiosk UI (active):** [`crates/pidi-native`](crates/pidi-native) — SDL/KMSDRM + GLES2 over `jambox-engine`. See [NATIVE_KIOSK.md](NATIVE_KIOSK.md).
- **Python Tk kiosk (archived):** [`apps/pidi`](apps/pidi) on `cursor/python-kiosk-archive-dfc2`
- **Thru engine:** Rust `midi-engine` — channel/CC/velocity remap via CLI + JSON presets (Map mode in the native kiosk)
- **Target hardware:** Pi 2 + MPK mini mk3 (+ USB-MIDI-DIN → synth when available)

## Crates

| Crate | Role |
|-------|------|
| `midi-core` | Event types, presets, transform chain, stuck-note tracking (no I/O) |
| `midi-engine` | `midir` CLI: list / run / learn / test / latency |
| `jambox-core` | Soft-synth DSP + **sample-accurate** sequencer (no I/O, no alloc in render) |
| `jambox-engine` | Realtime audio + sequencer daemon; kiosk UI is a thin client over a socket |
| `jambox-protocol` | JSON control protocol shared by the engine and native UI |
| `pidi-native` | Native kiosk UI (SDL + GLES2) |

## Jambox engine (audio + sequencing)

The jambox half of the box runs as its own realtime process so a busy UI cannot
move the beat. See [PLAN.md](PLAN.md) "Rust jambox engine".

```bash
cargo test -p jambox-core                   # timing + DSP tests, no hardware
cargo run -p jambox-engine -- devices       # audio outputs + MIDI ports
cargo run -p jambox-engine --release -- bench   # CPU headroom, no device needed
cargo run -p jambox-engine -- run --midi-in MPK --control /tmp/jambox.sock --rt
```

Control protocol is line-delimited JSON on a Unix socket (`--tcp` for host testing):

```bash
printf '{"cmd":"note_on","channel":0,"note":60,"velocity":100}\n' | nc -U /tmp/jambox.sock
printf '{"cmd":"clip_launch","slot":0,"quantize":"bar"}\n'        | nc -U /tmp/jambox.sock
printf '{"cmd":"status"}\n'                                        | nc -U /tmp/jambox.sock
```

systemd units: [`deploy/jambox-engine.service`](deploy/jambox-engine.service) and [`deploy/pidi-native.service`](deploy/pidi-native.service).

## Kiosk tests (no Pi, no audio device)

```bash
cargo test -p pidi-native -p jambox-protocol -p jambox-core
cargo run -p pidi-native -- --display dummy --frames 30 --dump /tmp/pidi.ppm
```

The archived Python Tk tests still live on `cursor/python-kiosk-archive-dfc2` under `apps/pidi/tests/`.

## Build & test (Windows / host)

Ensure `%USERPROFILE%\.cargo\bin` is on `PATH`, then:

```bash
cargo test
cargo build -p midi-engine
cargo run -p midi-engine -- latency
```

Linux hosts need ALSA headers for the audio engine: `sudo apt install libasound2-dev`.

## CLI

```bash
# List MIDI ports
cargo run -p midi-engine -- list

# Commissioning: send a few notes to an output
cargo run -p midi-engine -- test --output "MIDI"

# Learn CCs: prints JSON fragments for preset cc_map
cargo run -p midi-engine -- learn --input "MPK" --count 1 --out-channel 2

# CPU-only transform timing (not USB hop)
cargo run -p midi-engine -- latency

# Run thru (watches preset file; reloads + flushes stuck notes on change)
cargo run -p midi-engine -- run --preset presets/mpk-mini-ch3.json
cargo run -p midi-engine -- run --preset presets/mpk-mini-ch3.json --input "MPK" --output "U2MIDI"

# On the Pi, enable RT hints (needs limits from setup-pi.sh / systemd):
midi-engine run --preset presets/active.json --rt
```

Ctrl-C (and preset reload) flush active note-offs + All Notes Off.

## Preset JSON

See [`presets/example.json`](presets/example.json) and [`presets/mpk-mini-ch3.json`](presets/mpk-mini-ch3.json):

- `ports.input` / `ports.output` — name substrings (e.g. `MPK`, `MIDI`)
- `channel_map` — `identity` | `all_to` | `remap`
- `cc_map` — `(in_channel, in_cc) → (out_channel, out_cc)`
- `velocity` — `pass_through` | `always_full` | `clamp` | `curve`

Channels are **0–15** (MIDI channels 1–16). Example forces everything to channel **3** (`channel: 2`).

## Deploy to Pi

### Once on the Pi

```bash
# After scp'ing this repo's deploy/ folder, or cloning:
sudo bash deploy/setup-pi.sh
```

### From the PC (daily)

Cross-compile is preferred for Pi 2. **Standard procedure:** build the Pi
ELFs on the PC (or a Cursor cloud-agent VM) *before* committing crate
changes, so SET→UPDATE can install them:

```bash
./deploy/build-pi-bins.sh          # stages dist/armv7/{midi-engine,jambox-engine}
git add dist/armv7 && git commit   # required for SET→UPDATE
```

SSH deploy still works the same way. Easiest linker path is often Debian/WSL
`gcc-arm-linux-gnueabihf` (the script installs it when sudo is available) or
[cross](https://github.com/cross-rs/cross) — see [`.cargo/config.toml.example`](.cargo/config.toml.example).

```bash
# Bash (Git Bash / WSL / Linux) — also stages dist/armv7 when TARGET is armv7
TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh pi@<pi-ip>

# Or scp already-committed engines (no cargo):
USE_STAGED=1 WITH_JAMBOX=1 ./deploy/deploy.sh pi@<pi-ip>

# PowerShell
.\deploy\deploy.ps1 -PiHost pi@<pi-ip> -Target armv7-unknown-linux-gnueabihf
```

If you cannot cross-compile yet, build **on the Pi** (slow) or copy a host-built binary only for same-arch testing.

systemd unit: [`deploy/midi-engine.service`](deploy/midi-engine.service) (`--watch --rt`).
Edit `presets/active.json` on the device for the live map (port name changes need a restart).

## Phase 1 checklist

- [x] Channel / CC / velocity remap + JSON presets
- [x] CC Learn CLI, test notes, list ports
- [x] Stuck-note flush on exit + preset reload
- [x] Lock-free preset publish (`arc-swap`) for the thru callback
- [x] Transform latency bench (`latency`)
- [x] Linux `SCHED_FIFO` + `mlockall` hints (`--rt`)
- [x] systemd + setup/deploy scripts
- [ ] Real hardware smoke (MPK → Pi → DIN synth) — needs your devices
- [ ] Working armv7 cross linker on the dev machine

## Roadmap

See [PLAN.md](PLAN.md). After Phase 1 feels solid on hardware: drum retrigger → phrase loop → optional arp. Touch UI stays off the MIDI hot path.
