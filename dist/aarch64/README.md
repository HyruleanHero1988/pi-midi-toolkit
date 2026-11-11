# Pi aarch64 engines (committed)

These binaries are the **Raspberry Pi 4B 64-bit**
(`aarch64-unknown-linux-gnu`) builds of `midi-engine`, `jambox-engine`, and
`pidi-native`.

The Pi never `cargo build`s. **SET → UPDATE** copies this directory into
`~/pi-midi-toolkit/bin/` when the running kiosk is aarch64 (or when
`PIDI_DIST_ARCH=aarch64`). Live `bin/` is not overwritten by the generic
overlay so a half-applied update cannot clobber a running engine with a
host-arch file.

The lab Pi remains on [`dist/armv7`](../armv7/) until the Pi 4 card is the
live device. See [docs/AARCH64.md](../../docs/AARCH64.md).

## How these files get onto `master`

A green GitHub Actions run on `master` (crates / lockfile / build scripts)
cross-builds **both** `dist/armv7` and `dist/aarch64` and commits the ELFs
back. Artifact uploads on the workflow run are a backup download, not the
OTA path.

## Manual rebuild

```bash
ARCHES=aarch64 ./deploy/build-pi-bins.sh
# Windows:
#   .\deploy\build-pi-bins.ps1 -Arch aarch64
git add dist/aarch64
git commit -m "Rebuild Pi aarch64 engines"
```

Needs `gcc-aarch64-linux-gnu`, `libasound2-dev:arm64`, `libsdl2-dev:arm64`
plus GLES/EGL/GBM/DRM headers, `rustup target add aarch64-unknown-linux-gnu`,
and rustc **1.83+**. On Ubuntu the script pins `archive.ubuntu.com` to amd64
and adds `ports.ubuntu.com` for arm64 (`deploy/ubuntu-arm64-ports.sh`).

Do **not** put these files in Git LFS.
