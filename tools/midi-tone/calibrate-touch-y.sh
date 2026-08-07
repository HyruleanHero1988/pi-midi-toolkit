#!/usr/bin/env bash
# Correct ADS7846 Y offset for generic 5" HDMI GPIO panels on Raspberry Pi OS (labwc).
set -euo pipefail

MATRIX="${1:-1 0 0 0 1 -0.06}"

sudo tee /etc/udev/rules.d/99-ads7846-touch.rules >/dev/null <<EOF
ACTION=="add|change", KERNEL=="event*", ATTRS{name}=="ADS7846 Touchscreen", ENV{ID_INPUT}="1", ENV{ID_INPUT_TOUCHSCREEN}="1", ENV{ID_INPUT_WIDTH_MM}="150", ENV{ID_INPUT_HEIGHT_MM}="100", ENV{LIBINPUT_CALIBRATION_MATRIX}="${MATRIX}"
EOF

sudo tee /etc/udev/hwdb.d/61-ads7846-calibration.hwdb >/dev/null <<EOF
libinput:name:*ADS7846 Touchscreen*:
 LIBINPUT_CALIBRATION_MATRIX=${MATRIX}
EOF

sudo systemd-hwdb update
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=input --action=remove || true
sleep 1
sudo udevadm trigger --subsystem-match=input --action=add || true

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
labwc --reconfigure 2>/dev/null || killall -SIGHUP labwc 2>/dev/null || true

echo "Applied LIBINPUT_CALIBRATION_MATRIX='${MATRIX}'"
libinput list-devices 2>/dev/null | awk 'BEGIN{RS=""; FS="\n"} /ADS7846/{print}' || true
echo
echo "Use a small Y shift only (no big scale) or taps vanish off-screen."
echo "Still low on panel?  $0 '1 0 0 0 1 -0.10'"
echo "Too high?            $0 '1 0 0 0 1 -0.03'"
echo "Identity (no adj):   $0 '1 0 0 0 1 0'"
