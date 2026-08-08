#!/usr/bin/env bash
# Install midi-tone as an X11 Openbox kiosk session (no Raspberry Pi desktop shell).
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"

if [[ ! -f "$DIR/midi_tone.py" ]]; then
  echo "Run this from the midi-tone folder (found no midi_tone.py)."
  exit 1
fi

chmod +x "$DIR/kiosk.sh" "$DIR/run.sh" "$DIR/launch-desktop.sh" 2>/dev/null || true
sed -i 's/\r$//' "$DIR/kiosk.sh" "$DIR/install-kiosk.sh" \
  "$DIR/kiosk/openbox/autostart" 2>/dev/null || true

echo "==> Installing packages (openbox + X11 bits)…"
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  openbox xserver-xorg xinit x11-xserver-utils

if [[ ! -x "$DIR/.venv/bin/python" ]]; then
  echo "==> Creating Python venv…"
  "$DIR/setup-venv.sh"
fi

echo "==> Installing X session: MIDI Tone Kiosk"
SESSION_SRC="$DIR/kiosk/midi-tone-kiosk.desktop"
SESSION_DST="/usr/share/xsessions/midi-tone-kiosk.desktop"
TMP="$(mktemp)"
sed "s|REPLACE_KIOSK_SH|$DIR/kiosk.sh|g" "$SESSION_SRC" >"$TMP"
sudo install -m 644 "$TMP" "$SESSION_DST"
rm -f "$TMP"
echo "    $SESSION_DST"

echo "==> Linking Openbox config into ~/.config/openbox"
mkdir -p "$HOME/.config/openbox"
ln -sfn "$DIR/kiosk/openbox/rc.xml" "$HOME/.config/openbox/rc.xml"
ln -sfn "$DIR/kiosk/openbox/autostart" "$HOME/.config/openbox/autostart"

# LightDM / lightdm-ish: prefer this session for the user
if [[ -d /etc/lightdm ]]; then
  echo "==> Preferring MIDI Tone Kiosk session for user $USER (LightDM accountsservice)"
  mkdir -p "$HOME/.config"
  cat >"$HOME/.dmrc" <<EOF
[Desktop]
Session=midi-tone-kiosk
EOF
fi

# Raspberry Pi OS Bookworm often uses wayland/labwc — force X11 + our session hints
if command -v raspi-config >/dev/null 2>&1; then
  echo
  echo "Raspberry Pi OS notes:"
  echo "  1) Switch to X11 (not Wayland/labwc):"
  echo "       sudo raspi-config"
  echo "       Advanced Options → Wayland → X11"
  echo "  2) Autologin to desktop:"
  echo "       System Options → Boot / Auto Login → Desktop Autologin"
  echo "  3) At the login/session menu (or via .dmrc), choose: MIDI Tone Kiosk"
  echo "  4) Reboot"
fi

echo
echo "Manual test from an existing graphical login (or startx):"
echo "  $DIR/kiosk.sh"
echo
echo "Logs: /tmp/midi-tone-kiosk.log  and  /tmp/midi-tone.log (from run.sh)"
echo "Done."
