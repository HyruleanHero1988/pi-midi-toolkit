# PiDI branding

Product name: **PiDI** (Pi + MIDI).

## Splash art

- `pidi-splash.png` — 800×480 black field, cyan logo + wordmark (matches the TFT70 panel).

Used in three places so the same look covers power-on → UI:

1. **Plymouth** (`plymouth/pidi/`) — earliest boot, via `../install-pidi-splash.sh`
2. **X session** (`../splash-x11.py`) — while `kiosk.sh` starts Python
3. **App** (`midi_tone.py` boot splash) — until SYNTH chrome is ready

`install-pidi-splash.sh` also hides the panel text login (`getty@tty1` masked,
`console=tty1` removed) and quits Plymouth with `--retain-splash` so the logo
stays until X paints. Use SSH for a shell on the device.

## Install boot splash on the Pi

```bash
cd ~/midi-tone
sed -i 's/\r$//' install-pidi-splash.sh
chmod +x install-pidi-splash.sh
./install-pidi-splash.sh
sudo reboot
```
