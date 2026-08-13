#!/usr/bin/env bash
# Install PiDI splash as early as possible on Raspberry Pi OS:
#   1) Plymouth theme (kernel boot)
#   2) silence rainbow/firmware splash
#   3) quiet splash on cmdline
#
# Usage (on the Pi, from ~/midi-tone):
#   sed -i 's/\r$//' install-pidi-splash.sh
#   chmod +x install-pidi-splash.sh
#   ./install-pidi-splash.sh
#   sudo reboot
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
THEME_SRC="$DIR/branding/plymouth/pidi"
SPLASH_PNG="$DIR/branding/pidi-splash.png"

if [[ ! -f "$SPLASH_PNG" ]]; then
  echo "Missing $SPLASH_PNG" >&2
  exit 1
fi
if [[ ! -d "$THEME_SRC" ]]; then
  echo "Missing $THEME_SRC" >&2
  exit 1
fi

sudo_run() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

echo "==> Installing packages (plymouth)"
sudo_run apt-get update
sudo_run apt-get install -y --no-install-recommends plymouth plymouth-themes

echo "==> Installing PiDI Plymouth theme"
# Windows deploys can leave CRLF; Plymouth treats that as part of ModuleName.
sudo_run mkdir -p /usr/share/plymouth/themes/pidi
sudo_run cp -f "$THEME_SRC/pidi.plymouth" /usr/share/plymouth/themes/pidi/
sudo_run cp -f "$THEME_SRC/pidi.script" /usr/share/plymouth/themes/pidi/
sudo_run cp -f "$SPLASH_PNG" /usr/share/plymouth/themes/pidi/splash.png
sudo_run sed -i 's/\r$//' \
  /usr/share/plymouth/themes/pidi/pidi.plymouth \
  /usr/share/plymouth/themes/pidi/pidi.script

if command -v plymouth-set-default-theme >/dev/null 2>&1; then
  sudo_run plymouth-set-default-theme -R pidi
else
  echo "plymouth-set-default-theme missing — writing default.plymouth manually"
  echo -e "[DEFAULT]\nTheme=pidi" | sudo_run tee /etc/plymouth/plymouthd.conf >/dev/null
  if command -v update-initramfs >/dev/null 2>&1; then
    sudo_run update-initramfs -u
  fi
fi

# Firmware rainbow / GPU splash off
CFG=/boot/firmware/config.txt
[[ -f "$CFG" ]] || CFG=/boot/config.txt
if [[ -f "$CFG" ]]; then
  echo "==> config.txt: disable_splash=1"
  if grep -qE '^\s*disable_splash=' "$CFG"; then
    sudo_run sed -i 's/^\s*disable_splash=.*/disable_splash=1/' "$CFG"
  else
    echo "disable_splash=1" | sudo_run tee -a "$CFG" >/dev/null
  fi
fi

# Kernel cmdline: quiet splash (keep existing tokens)
CMDLINE=/boot/firmware/cmdline.txt
[[ -f "$CMDLINE" ]] || CMDLINE=/boot/cmdline.txt
if [[ -f "$CMDLINE" ]]; then
  echo "==> cmdline.txt: quiet splash, no panel console"
  CUR="$(tr -d '\n' <"$CMDLINE")"
  for tok in quiet splash logo.nologo plymouth.ignore-serial-consoles vt.global_cursor_default=0; do
    if [[ " $CUR " != *" $tok "* ]]; then
      CUR="$CUR $tok"
    fi
  done
  # Drop the on-panel Linux console (login prompt / getty). Keep serial if present.
  CUR="$(echo "$CUR" | sed -E 's/\bconsole=tty1\b//g; s/\bloglevel=[0-9]+\b//g; s/  +/ /g; s/^ //; s/ $//')"
  if [[ " $CUR " != *" console="* ]]; then
    CUR="console=serial0,115200 $CUR"
  fi
  echo "$CUR" | sudo_run tee "$CMDLINE" >/dev/null
fi

echo "==> Hide getty on tty1 (panel); keep SSH / serial for recovery"
sudo_run systemctl mask getty@tty1.service >/dev/null 2>&1 || true

# Early X splash (display-setup.sh) covers the Plymouth→session gap. The old
# plymouth-quit --retain-splash drop-in can hang systemctl reboot on the Pi.
echo "==> Remove plymouth-quit retain-splash override (avoids stuck reboot)"
PLY_DROPIN=/etc/systemd/system/plymouth-quit.service.d/50-pidi-retain-splash.conf
if [[ -f "$PLY_DROPIN" ]] || [[ -d /etc/systemd/system/plymouth-quit.service.d ]]; then
  sudo_run rm -f "$PLY_DROPIN" >/dev/null 2>&1 || true
  sudo_run rmdir /etc/systemd/system/plymouth-quit.service.d >/dev/null 2>&1 || true
  sudo_run systemctl daemon-reload >/dev/null 2>&1 || true
fi

echo
echo "PiDI boot splash installed."
echo "  Reboot to see it:  sudo reboot"
echo "  X/session splash is handled by kiosk.sh + midi-tone app branding."
echo "  Panel text login is masked (getty@tty1); use SSH if you need a shell."
