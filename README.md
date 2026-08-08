# pi-midi-toolkit

Raspberry Pi **MIDI appliance**: one kiosk UI for local soft-synth play **and**
low-latency MIDI thru/remap to a hardware synth. **Not** related to play-my-synth.

**North star:** power on → kiosk → modes (Synth / Looper / Log / Map). See [PLAN.md](PLAN.md).

- **Kiosk UI (active):** [`tools/midi-tone`](tools/midi-tone) — wavetable synth, morph, looper, Openbox kiosk
- **Thru engine:** Rust `midi-engine` — channel/CC/velocity remap via CLI + JSON presets (Map mode UI next)
- **Target hardware:** Pi 2 + MPK mini mk3 (+ USB-MIDI-DIN → synth when available)

## Crates

| Crate | Role |
|-------|------|
| `midi-core` | Event types, presets, transform chain, stuck-note tracking (no I/O) |
| `midi-engine` | `midir` CLI: list / run / learn / test / latency |

## Build & test (Windows / host)

Ensure `%USERPROFILE%\.cargo\bin` is on `PATH`, then:

```bash
cargo test
cargo build -p midi-engine
cargo run -p midi-engine -- latency
```

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

Cross-compile is preferred for Pi 2. Easiest path is often [cross](https://github.com/cross-rs/cross) or a Linux/WSL linker — see [`.cargo/config.toml.example`](.cargo/config.toml.example).

```bash
# Bash (Git Bash / WSL / Linux)
TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh pi@<pi-ip>

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
