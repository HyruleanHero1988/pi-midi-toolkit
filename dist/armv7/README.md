# Pi armv7 engines (committed)

These binaries are the **Raspberry Pi 2 / 32-bit lab** 
(`armv7-unknown-linux-gnueabihf`) builds of `midi-engine`, `jambox-engine`,
and `pidi-native`.

Official new hardware is the **Pi 4B 64-bit** path under
[`dist/aarch64`](../aarch64/) — see [docs/AARCH64.md](../../docs/AARCH64.md).
Lab SSH deploy still defaults here until that card is the live device.

The Pi never `cargo build`s. **SET → UPDATE** copies this directory into
`~/pi-midi-toolkit/bin/` when the running kiosk is not aarch64 (or when
`PIDI_DIST_ARCH=armv7`). Live `bin/` is not overwritten by the generic
overlay so a half-applied update cannot clobber a running engine with a
host-arch file.

## How these files get onto `master`

A green GitHub Actions run on `master` (crates / lockfile / build scripts)
cross-builds **both** armv7 and aarch64 and **commits the ELFs back** to
`master`. SET→UPDATE then has matching binaries for that tree. Artifact
uploads on the workflow run are a backup download, not the OTA path.

If `github-actions[bot]` cannot push (branch protection), either allow that
bot to update `master` or rebuild by hand as below.

## Manual rebuild

On a Linux PC, WSL, or a Cursor cloud-agent VM:

```bash
./deploy/build-pi-bins.sh
git add dist/armv7
git commit -m "Rebuild Pi armv7 engines"
```

Needs `gcc-arm-linux-gnueabihf`, `libasound2-dev:armhf` (both engines link
ALSA via midir/cpal), `libsdl2-dev:armhf` plus GLES/EGL/GBM/DRM headers
(`pidi-native` links SDL2), `rustup target add armv7-unknown-linux-gnueabihf`,
and rustc **1.83+**. The script installs the armhf toolchain with passwordless
sudo when it can. On Ubuntu it also pins `archive.ubuntu.com` to amd64 and
adds `ports.ubuntu.com` for armhf.

Do **not** put these files in Git LFS. GitHub archive downloads used by
UPDATE would then contain pointer files instead of real ELFs.
