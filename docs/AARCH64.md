# Pi 4B 64-bit bring-up (dual-arch)

PiDI is moving from the **Pi 2 / 32-bit armv7** lab card to a **Pi 4 Model B /
64-bit Raspberry Pi OS (Bookworm)** as the official hardware target.

Until the Pi 4 SD card is the live lab device:

- Lab SSH deploy defaults to **`dist/armv7`**
- CI and local builds publish **both** `dist/armv7` and `dist/aarch64`
- On-device SET→UPDATE installs the stage dir that matches the running binary
  (`aarch64` → `dist/aarch64`, otherwise `dist/armv7`). Override with
  `PIDI_DIST_ARCH`.

## Flash a fresh 64-bit image

1. Use Raspberry Pi Imager → **Raspberry Pi OS (64-bit)** Bookworm for Pi 4.
2. Use a **new** SD card. Do not migrate the Pi 2 card.
3. Enable SSH / set the login user in Imager advanced options.
4. Boot, then on the Pi:

```bash
# From a checkout of this repo on the Pi, or scp the script:
sudo bash deploy/setup-pi.sh
```

`setup-pi.sh` installs ALSA + SDL/KMSDRM runtime packages, realtime limits,
and enables `midi-engine` / `jambox-engine` / `pidi-native` units (rewritten
for `$USER` / `$HOME` when not `pi`). On **aarch64** it also runs
`deploy/enable-hifiberry.sh` (exclusive I2S for the DAC+ADC Pro HAT). Override
with `ENABLE_HIFIBERRY=0` on a headless aarch64 box without the HAT, or
`ENABLE_HIFIBERRY=1` on armv7 if you ever attach one.

After setup (or after SD firstboot), **reboot once** so `aplay -l` shows
`sndrpihifiberry` only (no Headphones). The unit ships with `--output hifiberry`;
the engine opens play+capture duplex on that `hw:CARD=` device.

## Build aarch64 engines (PC)

Bookworm-safe glibc floor: Ubuntu 22.04 WSL or Docker (same as CI).

```powershell
.\deploy\build-pi-bins.ps1 -Arch aarch64
# or both: .\deploy\build-pi-bins.ps1 -Arch both
```

```bash
ARCHES=aarch64 ./deploy/build-pi-bins.sh
```

Stages `dist/aarch64/{midi-engine,jambox-engine,pidi-native}` + `VERSION`.

## Deploy to the Pi 4

```powershell
python deploy_master.py --arch aarch64
# or
python apps/pidi/scripts/deploy/deploy_native.py --arch aarch64
```

```bash
PI_DIST_ARCH=aarch64 USE_STAGED=1 WITH_JAMBOX=1 ./deploy/deploy.sh <user>@<pi-ip>
```

After first deploy, mask LightDM so KMSDRM can own the panel (see
`setup-pi.sh` output), then restart the units.

## HiFiBerry (DAC+ADC Pro)

Canonical script: `deploy/enable-hifiberry.sh` (idempotent).

| Path | What runs it |
|------|----------------|
| Fresh Pi after `setup-pi.sh` | auto on aarch64 |
| Pi 4 SD firstboot | `_pidi_pi4_firstboot.sh` |
| SD image prep | `_prepare_pi4_sd*.sh` patches `config.txt` offline (`SKIP_RUNTIME=1`) |
| Manual / repair | `sudo bash deploy/enable-hifiberry.sh` |

It sets `dtparam=audio=off`, `dtoverlay=vc4-kms-v3d,noaudio`,
`dtoverlay=hifiberry-dacplusadcpro`, forces `--output hifiberry` on the unit,
and unmutes Digital when the card is already loaded.

## Still open (later OS-image tickets)

These are **not** part of the dual-arch / HiFiBerry bring-up:

- Geekworm X728 battery monitoring and `x728off`
- Seamless Plymouth → UI boot (no cursor flash)
- Minimal Buildroot/Yocto image

## Arch cheat sheet

| Short   | Rust target                     | Stage dir       | Multiarch |
|---------|---------------------------------|-----------------|-----------|
| armv7   | `armv7-unknown-linux-gnueabihf` | `dist/armv7`    | armhf     |
| aarch64 | `aarch64-unknown-linux-gnu`     | `dist/aarch64`  | arm64     |
