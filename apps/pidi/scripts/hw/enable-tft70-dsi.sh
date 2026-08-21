#!/usr/bin/env bash
# Switch midi-pi from legacy HDMI+ADS7846 panel to BigTreeTech Pi TFT70 (DSI + GT911).
#
# TFT70 V2.1: 7" 800x480 DSI capacitive. Official Pi 7" KMS overlay works for
# most BTT Pi TFT panels; we remove the old ADS7846 SPI overlay which fights DSI.
#
# Usage (on the Pi, from ~/midi-tone):
#   ./enable-tft70-dsi.sh
#   sudo reboot
set -euo pipefail

CFG=/boot/firmware/config.txt
if [[ ! -f "$CFG" ]]; then
  CFG=/boot/config.txt
fi
if [[ ! -f "$CFG" ]]; then
  echo "No config.txt found under /boot/firmware or /boot" >&2
  exit 1
fi

if [[ "$(id -u)" -ne 0 ]]; then
  exec sudo -E bash "$0" "$@"
fi

TS="$(date +%Y%m%d-%H%M%S)"
cp -a "$CFG" "${CFG}.bak-pre-tft70-${TS}"
echo "Backed up → ${CFG}.bak-pre-tft70-${TS}"

# Drop legacy resistive HDMI touch overlay lines
tmp="$(mktemp)"
grep -vE \
  '^[[:space:]]*dtoverlay=ads7846|^\s*#.*ADS7846|^\s*#.*Generic 5' \
  "$CFG" >"$tmp" || true
# Also strip prior midi-tone TFT70 block so re-runs are idempotent
awk '
  BEGIN {skip=0}
  /^# --- midi-tone TFT70/ {skip=1; next}
  /^# --- end midi-tone TFT70/ {skip=0; next}
  skip==0 {print}
' "$tmp" >"${tmp}.2"
mv "${tmp}.2" "$tmp"

# Ensure vc4 KMS is present (Bookworm default, but be explicit)
if ! grep -qE '^[[:space:]]*dtoverlay=vc4-kms-v3d' "$tmp"; then
  echo "dtoverlay=vc4-kms-v3d" >>"$tmp"
fi

cat >>"$tmp" <<'EOF'

# --- midi-tone TFT70 (BigTreeTech Pi TFT70 V2.1 DSI 800x480 + GT911) ---
# Remove HDMI-force lines from the old panel if display stays blank after reboot.
hdmi_force_hotplug=0
# Official 7" DSI KMS panel driver — BTT Pi TFT70 is pin-compatible / GT911 path
dtoverlay=vc4-kms-dsi-7inch
# Touch often rides the DSI bridge; if GT911 is missing after reboot, see KIOSK.md
dtparam=i2c_arm=on
# --- end midi-tone TFT70 ---
EOF

install -m 644 "$tmp" "$CFG"
rm -f "$tmp"

echo
echo "Updated $CFG for TFT70 DSI."
echo "Next:"
echo "  1) Confirm DSI ribbon is seated (contacts correct way; not reversed)."
echo "  2) sudo reboot"
echo "  3) After boot: libinput list-devices | grep -iE 'goodix|gt911|ft5|touch'"
echo "  4) DISPLAY=:0 xrandr  (expect 800x480)"
echo
echo "If still blank: try commenting hdmi_* lines in $CFG, keep only vc4-kms-v3d + vc4-kms-dsi-7inch."
