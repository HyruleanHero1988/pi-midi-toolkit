# Pi armv7 engines (committed)

These binaries are the **Raspberry Pi 2** (`armv7-unknown-linux-gnueabihf`)
builds of `midi-engine` and `jambox-engine`.

The Pi never `cargo build`s. **SET → UPDATE** copies this directory into
`~/pi-midi-toolkit/bin/` (the live systemd paths) after overlaying the rest
of the repo. Live `bin/` is not overwritten by the generic overlay so a
half-applied update cannot clobber a running engine with a host-arch file.

## Rebuild before you commit crate changes

On a Linux PC, WSL, or a Cursor cloud-agent VM:

```bash
./deploy/build-pi-bins.sh
git add dist/armv7
git commit -m "Rebuild Pi armv7 engines"
```

Needs `gcc-arm-linux-gnueabihf`, `libasound2-dev:armhf` (both engines link
ALSA via midir/cpal), and `rustup target add armv7-unknown-linux-gnueabihf`.
The script installs those with passwordless sudo when it can.

Do **not** put these files in Git LFS. GitHub archive downloads used by
UPDATE would then contain pointer files instead of real ELFs.
