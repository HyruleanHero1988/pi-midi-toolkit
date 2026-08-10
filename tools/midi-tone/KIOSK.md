# midi-tone kiosk — new Pi bring-up guide

Goal: a Raspberry Pi that **powers on into fullscreen midi-tone** (no desktop
wallpaper, panel, or file manager). Keyboard/mouse are fine; resistive touch is
optional and covered only briefly at the end.

This matches the setup used on the lab unit (`midi-pi`) after
`install-kiosk.sh` + LightDM autologin into the **MIDI Tone Kiosk** session.

---

## What you end up with

| Piece | Role |
| --- | --- |
| Raspberry Pi OS (Bookworm) + **X11** | Display server (not Wayland/labwc) |
| LightDM + Desktop Autologin | Boots straight into a user session |
| Session `midi-tone-kiosk` | Runs `kiosk.sh` instead of LXDE |
| Openbox (optional) | Minimal WM; kiosk can also run Tk fullscreen alone |
| `kiosk.sh` | Starts `midi_tone.py --fullscreen`, restarts if it exits |
| Audio + MIDI | Local soft-synth; USB controller when present |

**Display:** UI is laid out for **800×480**. Lab hardware is a 5″ HDMI GPIO
panel; the planned panel is BigTreeTech Pi TFT70 (DSI, same resolution).

---

## 0. Hardware / image checklist

1. Flash **Raspberry Pi OS** (Desktop image is fine — kiosk replaces the shell).
2. Enable SSH (Imager advanced options, or `raspi-config` once).
3. Create a normal user (lab: `ray`) with sudo.
4. Connect: Ethernet or Wi‑Fi, HDMI (or DSI) panel, USB keyboard/mouse for setup.
5. Optional: USB MIDI keyboard/pads (e.g. Akai MPK).

First boot: finish the Pi OS wizard, set locale/timezone, then continue below.

---

## 1. Put midi-tone on the Pi

### Option A — from a Windows/dev machine (lab path)

On the PC, in `tools/midi-tone/`:

1. Create `.pi-credentials` (gitignored):

   ```
   PI_HOST=192.168.1.225
   PI_USER=ray
   PI_PASSWORD=...
   PI_DIR=~/midi-tone
   ```

2. Deploy:

   ```powershell
   python deploy_pi.py
   ```

   Use `--restart` only if a desktop session is already up and you want a
   one-shot app relaunch. For a **new** Pi, prefer install + reboot (below).

### Option B — clone on the Pi

```bash
sudo apt-get update
sudo apt-get install -y git python3-venv python3-pip
git clone <YOUR_REPO_URL> ~/pi-midi-toolkit
cd ~/pi-midi-toolkit/tools/midi-tone
```

If you copy files from Windows, strip CR:

```bash
sed -i 's/\r$//' *.sh kiosk/openbox/* kiosk/*.desktop kiosk/lightdm/* 2>/dev/null
chmod +x *.sh
```

---

## 2. Python venv + smoke test (optional but recommended)

```bash
cd ~/midi-tone   # or …/tools/midi-tone
./setup-venv.sh
```

With a normal desktop still active, you can test once:

```bash
export DISPLAY=:0
./launch-desktop.sh
```

You should see fullscreen midi-tone. Quit with the window close / POWER flow,
or `pkill -f midi_tone.py` over SSH.

---

## 3. Install kiosk boot (the important step)

```bash
cd ~/midi-tone
chmod +x install-kiosk.sh disable-kiosk.sh kiosk.sh run.sh launch-desktop.sh
./install-kiosk.sh
sudo reboot
```

`install-kiosk.sh` will:

1. Install Openbox + X11 packages (and create the venv if missing).
2. Install `/usr/share/xsessions/midi-tone-kiosk.desktop` pointing at `kiosk.sh`.
3. Switch to **X11** and **Desktop Autologin** via `raspi-config` (noninteractive).
4. Set `~/.dmrc`, AccountsService, and — critically — **both**:
   - `/etc/lightdm/lightdm.conf.d/99-midi-tone-kiosk.conf`
   - **`/etc/lightdm/lightdm.conf`** (`user-session` / `autologin-session` =
     `midi-tone-kiosk`)

Why both? LightDM loads the **main** `lightdm.conf` *after* `conf.d/`.
`raspi-config` B4 writes `autologin-session=LXDE-pi-x` into the main file, which
would otherwise override a drop-in-only install and land you on the gray desktop.

Also installs a small sudoers snippet so POWER → shut down / reboot works from
the UI without a password.

**Flags:**

```bash
./install-kiosk.sh --no-boot     # packages + session only
./install-kiosk.sh --boot-only   # enable autologin/session only
```

---

## 4. Verify after reboot

On the panel you should see **midi-tone** fullscreen (SYNTH / LOOPER / …), not
the Pi desktop.

Over SSH:

```bash
# Session should be kiosk, not lxsession
pgrep -af 'kiosk.sh|midi_tone|openbox|lxsession'

# LightDM must name the kiosk session
grep -E '^(user-session|autologin-session)=' /etc/lightdm/lightdm.conf

# App log
tail -n 40 /tmp/midi-tone-kiosk.log
```

Expect something like:

- `kiosk.sh` running
- `python … midi_tone.py --fullscreen`
- `user-session=midi-tone-kiosk` and `autologin-session=midi-tone-kiosk`
- Log lines: `ui: construction complete` → `entering mainloop`

Manual relaunch without reboot (from SSH, while X is up):

```bash
export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/$(id -u)
cd ~/midi-tone && ./kiosk.sh
```

---

## 5. Audio / MIDI notes

- Prefer the analog headphone jack when testing speakers/headphones; HDMI panels
  often leave PCM muted. `fix-audio-headphones.sh` can help.
- With no USB MIDI device, the app falls back to **Midi Through** and still
  shows the UI.
- When an MPK (or similar) is connected, `launch-desktop.sh` / `kiosk.sh` prefer
  `--input MPK` automatically.

---

## 6. Undo kiosk (back to normal desktop)

```bash
cd ~/midi-tone
./disable-kiosk.sh
sudo reboot
```

Restores the previous desktop session preference and removes the LightDM kiosk
drop-in / main-conf kiosk session keys. Does **not** uninstall Openbox packages.

---

## 7. File map

| Path | Purpose |
| --- | --- |
| `install-kiosk.sh` | Packages + X session + enable boot |
| `disable-kiosk.sh` | Restore desktop boot |
| `kiosk.sh` | Session entry: Openbox (optional) + app restart loop |
| `kiosk/midi-tone-kiosk.desktop` | xsessions definition |
| `kiosk/lightdm/99-midi-tone-kiosk.conf` | LightDM drop-in template |
| `kiosk/openbox/rc.xml` | Undecorated / fullscreen-friendly WM config |
| `kiosk/openbox/autostart` | Empty stub (app started by `kiosk.sh`) |
| `launch-desktop.sh` | One-shot fullscreen launch from a desktop/SSH |
| `run.sh` | venv + `midi_tone.py` |
| `setup-venv.sh` | Create `.venv` + pip install |

Logs: `/tmp/midi-tone-kiosk.log`, `/tmp/midi-tone.log`.

---

## 8. Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Boots to **gray Pi desktop** | LightDM still on `LXDE-pi-x` | Re-run `./install-kiosk.sh --boot-only` and confirm **main** `lightdm.conf` sessions; reboot |
| Blank / solid dark screen, process running | Duplicate app instances or WM fight | `pkill -f midi_tone.py`; check a single `kiosk.sh`; reboot once |
| `couldn't connect to display ":0"` | X/LightDM not up yet or crashed | `systemctl status lightdm`; `sudo systemctl start lightdm` |
| POWER shut down fails | Missing sudoers | Re-run `install-kiosk.sh` (writes `/etc/sudoers.d/midi-tone-power`) |
| No sound | HDMI vs jack / mute | `./fix-audio-headphones.sh`; check `alsamixer` |

If LightDM hangs on stop/restart over SSH:

```bash
sudo systemctl kill -s SIGKILL lightdm
sudo systemctl reset-failed lightdm
sudo systemctl start lightdm
```

---

## 9. Touch (optional — pinned)

Resistive ADS7846 (SPI on many 5″ HDMI GPIO panels) is **legacy**. Keyboard and
mouse are enough for development.

If you revisit touch later:

```bash
./enable-gpio-touch.sh    # dtoverlay in /boot/firmware/config.txt — needs reboot
./fix-touch-x11.sh       # X11 evdev InputClass + udev tags
./set-touch-overlay.sh 25 # or 17 — penirq experiments; needs reboot
./calibrate-touch-y.sh    # libinput Y matrix tweak
```

Capacitive TFT70 bring-up is tracked in `PLAN.md` and will not use these ADS7846
scripts.

---

## Quick copy-paste (experienced)

```bash
cd ~/midi-tone
sed -i 's/\r$//' *.sh kiosk/openbox/* kiosk/*.desktop kiosk/lightdm/* 2>/dev/null
chmod +x *.sh
./setup-venv.sh
./install-kiosk.sh
sudo reboot
```
