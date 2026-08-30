# Native jambox vertical slice

Status: **prototype on this branch, for Pi hardware measurement**

This is the focused KAOSS/drum surface described in
[JAMBOX_ARCHITECTURE_NEXT.md](JAMBOX_ARCHITECTURE_NEXT.md). It is not a
replacement for the Tk kiosk yet. Keep PiDI running until this slice beats that
baseline on the real Pi 2 + TFT70.

The slice exists to prove **SDL + DRM/KMS + OpenGL ES 2** on this exact stack.
fbdev is an explicit fallback, not the default presenter.

## What it proves

| Gate | How this slice exercises it |
|---|---|
| Engine owns musical time | Sample-clocked kick repeat; UI has no 250 ms timer |
| Reliable edges vs latest XY | `Touch` down/up on the command ring; moves overwrite a mailbox |
| Disconnect safety | Killing `pidi-native` emergency-releases that session's gestures |
| Explicit ALSA buffering | `jambox-engine run --buffer-frames 512` probes 512 → 1024 → 256 → default |
| Real diagnostics | Status reports callback frames/µs, xruns, command drops, emergency releases |
| Multitouch | SDL `FingerId` events (evdev type-B only if SDL touch is wrong) |
| CELLS at 60 Hz | 12×7 field as one GLES2 color-quad mesh; previous frame kept until swap |
| 16-drum grid | Pads 36–51; hold **KICK** to repeat; other pads fire immediately |

Out of scope: Wi-Fi, updater, songs, SEQ, presets, visual polish.

## Runtime path

```
SDL FingerId (or evdev if --touch evdev)
  → NativeModel hit-test
  → Outbox (reliable edges + coalesced XY)
  → jambox-engine (DSP, repeats, notes)

scene::build(model) → GLES2 batched quads + glyph atlas → SDL swap
```

Musical time stays in the engine. This process may drop frames; it must never
own a note release. SDL audio is forced to the dummy driver so the UI never
opens ALSA.

`--display auto` prefers SDL/GLES, then fbdev, then dummy. `--display sdl`
fails if the context cannot be created (except `--frames`, which falls back to
dummy so host CI can run headless). `--display fb` and `--display dummy` are
explicit. `--touch auto` uses SDL fingers when the presenter is SDL.

On the Pi, LightDM/X owns DRM. KMSDRM needs the kiosk stopped:

```bash
STOP_KIOSK=1 ./deploy/deploy-native-slice.sh pi@<host>
# or:
sudo systemctl stop lightdm
sudo systemctl restart pidi-native
```

The unit sets `SDL_VIDEODRIVER=kmsdrm` and `SDL_AUDIODRIVER=dummy`.

## Binaries

- `jambox-engine` — audio, MIDI, protocol v1 (`hello` / `touch` / `repeat` / `status`)
- `pidi-native` — 800×480 KAOSS + drums client (SDL/GLES presenter)

Host (no Pi, no audio device, no GPU required):

```bash
cargo test -p jambox-core -p jambox-protocol -p jambox-engine -p pidi-native
cargo run -p jambox-engine -- run --null-audio --tcp --control 127.0.0.1:17890
cargo run -p pidi-native -- --tcp --control 127.0.0.1:17890 --display dummy --frames 30 --dump /tmp/slice.ppm
```

Host window (if the machine has a display):

```bash
cargo run -p pidi-native -- --tcp --control 127.0.0.1:17890 --display sdl --windowed
```

## Deploy to the Pi

### Fast host loop (this PC — required for Pi-compatible bins)

Pi Bookworm has **glibc 2.36**. Bins linked on Ubuntu 24.04 / Debian 13 need
**2.38+** and will not start on the device. GitHub Actions is optional backup;
local builds must use the same floor as CI (**Ubuntu 22.04 / glibc 2.35**).

**Windows (recommended):**

```powershell
# One-time: installs Ubuntu-22.04 WSL if needed, then cross-builds
.\deploy\build-pi-bins.ps1 -InstallDistro

# Later rebuilds
.\deploy\build-pi-bins.ps1
.\deploy\build-pi-bins.ps1 -Packages "jambox-engine,pidi-native"
```

Docker Desktop alternative: `.\deploy\build-pi-bins.ps1 -PreferDocker`
(builds `deploy/Dockerfile.pi-bins`).

**Already inside Ubuntu 22.04 WSL:**

```bash
cd /mnt/c/Users/Raymond/Documents/Development/pi-midi-toolkit
# first time: rustup + passwordless sudo helps apt in the script
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
./deploy/build-pi-bins.sh
```

`./deploy/build-pi-bins.sh` **refuses** hosts with glibc > 2.36 unless you set
`FORCE_NEW_GLIBC=1` (bins will not run on the Pi).

CI (`.github/workflows/build-pi-bins.yml` on ubuntu-22.04) cross-builds and,
on `master`, commits `dist/armv7` so SET→UPDATE can install the ELFs.

Cross-build needs `gcc-arm-linux-gnueabihf`, rustc **1.83+**, and the armhf SDL
+ GLES development packages (`libsdl2-dev:armhf`, `libgles2-mesa-dev:armhf`,
`libegl1-mesa-dev:armhf`, `libgbm-dev:armhf`, `libdrm-dev:armhf`). The build
script installs those when `sudo` is passwordless.

```bash
PACKAGES=midi-engine,jambox-engine,pidi-native ./deploy/build-pi-bins.sh
git add dist/armv7
```

The Pi runtime needs `libsdl2-2.0-0`, `libgles2`, `libgbm1`, and `libdrm2`.

Install:

```bash
./deploy/deploy-native-slice.sh pi@<host>
# Optional: STOP_KIOSK=1 ./deploy/deploy-native-slice.sh pi@<host>
```

```bash
~/pi-midi-toolkit/bin/hardware-check.sh
```

## Pi test checklist

1. `evtest` — Goodix/GT911 slots 0–4, tracking IDs, lift = `-1`.
2. Engine log — `audio: opening output buffer=fixed-512` (or 1024/256), not a silent ~4410-frame default.
3. Tap KAOSS — sound starts; lift — note stops within one audio period.
4. Fast pitch scrub — no notes continue after lift.
5. Hold KICK — quarter-note (or 1/8, 1/8T, 1/16) continues if the UI stalls.
6. Hold KICK + tap snare/hat — extra hits, repeat keeps time.
7. `killall pidi-native` — engine stays up; owned notes/repeats release (`emergency_releases` increments).
8. Restart UI — no stuck notes.
9. CELLS — no blank flash; previous frame remains until the next complete swap.
10. Status HUD — callback frames, µs, xruns, drops.
11. Confirm `pidi-native` log says `SDL/GL presenter` / display `sdl-gles`, not fbdev.
12. If SDL finger IDs or coordinates are wrong, keep `--display sdl` and switch `--touch evdev` (lab: `--evdev /dev/input/event4` for FT5x06; avoid ADS7846).

Capture Tk baseline with the same gestures before deciding to migrate the rest
of PiDI.

## Limitations

- KMSDRM cannot share the panel with LightDM/X. Stop the kiosk for this slice.
- If SDL's KMS touch path mishandles the TFT70 window ID or coordinates, keep
  SDL for rendering and use `--touch evdev`. Do not fall back to mouse
  emulation on the appliance.
- Host machines without GLES 2 may get an OpenGL 2.1 compatibility context
  with the same shaders. That is a development fallback, not the Pi proof.
- KAOSS mapping in this slice is LEAD (ionian C, Y = tone) only.
- Kick is the only pad that owns a repeat lane; other drums are one-shots.
- Python PiDI still works against the engine for notes/clips; it does not yet
  send `touch` / `repeat`.
