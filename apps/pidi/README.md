# apps/pidi — shared Pi appliance assets

The **UI is not here.** The live kiosk is [`crates/pidi-native`](../../crates/pidi-native)
(SDL/KMSDRM + GLES2). See [NATIVE_KIOSK.md](../../NATIVE_KIOSK.md).

This directory holds what the native stack still needs next to the crates:

| Path | Role |
|------|------|
| `wavetables/` | Factory AKWF single-cycles for `jambox-engine` |
| `pidi/updater.py` | OTA check/apply helper (`pidi-native` SET → UPDATE shells `--check`) |
| `scripts/session/pi-power.sh` | POWER reboot/poweroff |
| `scripts/hw/` | TFT70 / GPIO touch bring-up |
| `scripts/session/fix-audio-headphones.sh` | ALSA headphone routing |
| `docs/` | Screen captures mirrored for [raygarrison.us](https://raygarrison.us) |
| `.wifi-credentials.example` | Optional Wi‑Fi creds format for native |

User data (`songs/`, `phrases/`, `user-presets/`, `settings.json`) lives on the
device under `PIDI_DATA_ROOT` (default `~/.local/share/pidi` on the appliance)
— not in this tree for the native boot.

Historical Python/Tk sources (if you need them) are on the archive branch
`cursor/python-kiosk-archive-dfc2`.
