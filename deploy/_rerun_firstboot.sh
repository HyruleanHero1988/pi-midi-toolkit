#!/bin/bash
set -euo pipefail
# Ensure WiFi country stays set, then finish PiDI bootstrap now that DNS works.
raspi-config nonint do_wifi_country US || true
rfkill unblock wifi || true
nmcli radio wifi on || true
rm -f /var/lib/pidi-firstboot.done
bash /usr/local/lib/pidi/firstboot.sh
echo '=== services ==='
systemctl is-active midi-engine jambox-engine pidi-native || true
systemctl --no-pager --full status jambox-engine pidi-native | sed -n '1,40p' || true
