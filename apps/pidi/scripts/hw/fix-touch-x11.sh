#!/usr/bin/env bash
# Install X11/udev config so ADS7846 resistive touch reaches the kiosk UI.
# Safe to re-run. Needs a session restart (LightDM) afterward.
set -euo pipefail

echo "Installing xserver-xorg-input-evdev (if needed)…"
sudo apt-get install -y xserver-xorg-input-evdev xinput evtest >/tmp/apt-touch.log 2>&1 || {
  echo "apt install had issues — see /tmp/apt-touch.log (continuing with config)"
}

sudo tee /etc/X11/xorg.conf.d/99-ads7846.conf >/dev/null <<'EOF'
# Resistive ADS7846 (generic 5" HDMI+GPIO panels). Prefer evdev over libinput —
# libinput often sees the device but never delivers ButtonPress to Tk.
Section "InputClass"
    Identifier "ADS7846 evdev touchscreen"
    MatchProduct "ADS7846 Touchscreen"
    MatchDevicePath "/dev/input/event*"
    MatchIsTouchscreen "on"
    Driver "evdev"
    Option "Calibration" "200 3900 200 3900"
    Option "SwapAxes" "0"
    Option "InvertX" "0"
    Option "InvertY" "0"
    Option "EmulateThirdButton" "0"
    # Ensure pen-down maps to Button1 (Tk listens for ButtonPress-1)
    Option "ButtonMapping" "1 0 0 0 0 0 0"
EndSection
EOF

sudo tee /etc/udev/rules.d/99-ads7846-touch.rules >/dev/null <<'EOF'
ACTION=="add|change", KERNEL=="event*", ATTRS{name}=="ADS7846 Touchscreen", \
  ENV{ID_INPUT}="1", ENV{ID_INPUT_TOUCHSCREEN}="1", ENV{ID_INPUT_TOUCHPAD}="0", \
  ENV{ID_INPUT_MOUSE}="0"
EOF

sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input --action=change || true

echo "Touch X11 config installed:"
echo "  /etc/X11/xorg.conf.d/99-ads7846.conf"
echo "Restart the graphical session to load it:"
echo "  sudo systemctl restart lightdm"
