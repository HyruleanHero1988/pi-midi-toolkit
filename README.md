# pi-midi-toolkit

Raspberry Pi **MIDI appliance**: one kiosk UI for local soft-synth play **and**
low-latency MIDI thru/remap to a hardware synth. **Not** related to play-my-synth.

**North star:** power on → kiosk → modes (Synth / Seq / Pads / Kaoss / Chords / Map / Log). See [PLAN.md](PLAN.md).

- **Kiosk UI (active):** [`crates/pidi-native`](crates/pidi-native) — SDL/KMSDRM + GLES2 over `jambox-engine`. See [NATIVE_KIOSK.md](NATIVE_KIOSK.md) and the [native screen reference](docs/index.html).
- **Shared Pi assets:** [`apps/pidi`](apps/pidi) — wavetables, power/HW scripts (not a UI). OTA is [`deploy/updater.py`](deploy/updater.py), invoked by the native kiosk.
- **Thru engine:** Rust `midi-engine` — channel/CC/velocity remap via CLI + JSON presets (Map mode in the native kiosk)
- **Target hardware:** Pi 2 + any class-compliant USB MIDI keyboard or USB-MIDI-DIN interface (MPK mini, U2MIDI PRO, …)

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
cargo run -p jambox-engine -- run --control /tmp/jambox.sock --rt
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

## USB MIDI (plug and play)

Any class-compliant USB MIDI keyboard or interface should just work. The
engine opens **every** hardware input (it skips Midi Through / loopback).
You do not have to pick MPK, U2MIDI, or any other brand.

1. Plug the USB MIDI device. For a DIN keyboard, use a USB-MIDI-DIN
   interface (e.g. U2MIDI PRO): keyboard DIN **out** → interface **in**.
2. Play. SYNTH should sound. SETTINGS → PORTS shows live ports and the last
   incoming note; tap IN only if you want to pin one device.
3. Hardware-synth **out** is the first hardware port by default. Tap OUT
   on PORTS if you have more than one, set a mode's OUT to USB/BOTH, then
   **TEST OUT**.
4. HOME → **MAP** remaps incoming MIDI channels (e.g. ch 1 → ch 6, or 1 → several).
5. **THRU ON** (on PORTS) is the optional remap cable (keyboard → DIN synth). You do
   not need it to play the onboard synth.

## Preset JSON

See [`presets/example.json`](presets/example.json) and [`presets/mpk-mini-ch3.json`](presets/mpk-mini-ch3.json):

- `ports.input` / `ports.output` — name substrings (e.g. `MPK`, `MIDI`)
- `channel_map` — `identity` | `all_to` | `remap` | `fanout` (1:N bitmasks)
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

PowerShell (lab Pi; uses `apps/pidi/.pi-credentials`):

```powershell
python deploy_master.py          # deploy master
python deploy/deploy-branch.py   # deploy the current checkout
```

These overlay the repo, install committed `dist/armv7` engines, and restart
`jambox-engine` + `pidi-native`. Rebuild bins first with
`.\deploy\build-pi-bins.ps1` when crates changed.

Cross-compile is preferred for Pi 2. **SET→UPDATE** installs committed
`dist/armv7/{midi-engine,jambox-engine,pidi-native}` onto `bin/`. A green
push to `master` that touches crates runs
[`.github/workflows/build-pi-bins.yml`](.github/workflows/build-pi-bins.yml),
which rebuilds those ELFs and commits them back so cloud-agent merges are
OTA-ready. Manual rebuild is still useful for LAN SSH deploys:

```bash
./deploy/build-pi-bins.sh          # stages dist/armv7/{midi-engine,jambox-engine,pidi-native}
git add dist/armv7 && git commit   # optional when CI commit-back will run
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
