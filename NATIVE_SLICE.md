# Native jambox vertical slice

Status: **prototype on this branch, for Pi hardware measurement**

This is the focused KAOSS/drum surface described in
[JAMBOX_ARCHITECTURE_NEXT.md](JAMBOX_ARCHITECTURE_NEXT.md). It is not a
replacement for the Tk kiosk yet. Keep PiDI running until this slice beats that
baseline on the real Pi 2 + TFT70.

## What it proves

| Gate | How this slice exercises it |
|---|---|
| Engine owns musical time | Sample-clocked kick repeat; UI has no 250 ms timer |
| Reliable edges vs latest XY | `Touch` down/up on the command ring; moves overwrite a mailbox |
| Disconnect safety | Killing `pidi-native` emergency-releases that session's gestures |
| Explicit ALSA buffering | `jambox-engine run --buffer-frames 512` probes 512 → 1024 → 256 → default |
| Real diagnostics | Status reports callback frames/µs, xruns, command drops, emergency releases |
| Multitouch | Up to five contacts; GT911 still needs `evtest` proof on the TFT70 |
| CELLS at 60 Hz | 12×7 field drawn as one CPU frame, previous frame kept until present |
| 16-drum grid | Pads 36–51; hold **KICK** to repeat; other pads fire immediately |

Out of scope: Wi-Fi, updater, songs, SEQ, presets, visual polish, SDL/GLES (the
presenter is fbdev or dummy so the crate cross-compiles without SDL2).

## Binaries

- `jambox-engine` — audio, MIDI, protocol v1 (`hello` / `touch` / `repeat` / `status`)
- `pidi-native` — 800×480 KAOSS + drums client

Host (no Pi, no audio device):

```bash
cargo test -p jambox-core -p jambox-protocol -p jambox-engine -p pidi-native
cargo run -p jambox-engine -- run --null-audio --tcp --control 127.0.0.1:17890
cargo run -p pidi-native -- --tcp --control 127.0.0.1:17890 --display dummy --frames 30 --dump /tmp/slice.ppm
```

## Deploy to the Pi

Cross-build (needs `gcc-arm-linux-gnueabihf` and rustc **1.85+**):

```bash
PACKAGES=midi-engine,jambox-engine,pidi-native ./deploy/build-pi-bins.sh
git add dist/armv7
```

Install:

```bash
./deploy/deploy-native-slice.sh pi@<host>
# Optional: STOP_KIOSK=1 ./deploy/deploy-native-slice.sh pi@<host>
```

On the Pi, LightDM/X will own DRM. For the framebuffer presenter to show on the
TFT70, either stop the kiosk (`STOP_KIOSK=1`) or run `pidi-native` on a free VT:

```bash
sudo systemctl stop lightdm
sudo systemctl restart pidi-native
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
9. CELLS — no blank flash; previous frame remains until the next complete frame.
10. Status HUD — callback frames, µs, xruns, drops.

Capture Tk baseline with the same gestures before deciding to migrate the rest
of PiDI.

## Limitations

- Presenter is **fbdev**, not DRM/KMS + GLES. That is the next hardware probe if
  `/dev/fb0` is missing or X owns the display.
- Touch is **evdev type-B**, not SDL. If coordinates are swapped or inverted on
  this TFT70, fix the absinfo map in `pidi-native` after `evtest`.
- KAOSS mapping in this slice is LEAD (ionian C, Y = tone) only.
- Kick is the only pad that owns a repeat lane; other drums are one-shots.
- Python PiDI still works against the engine for notes/clips; it does not yet
  send `touch` / `repeat`.
