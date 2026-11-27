#!/bin/bash
set -euo pipefail
install -m 755 /tmp/jambox-engine /home/ray/pi-midi-toolkit/bin/jambox-engine
chown ray:ray /home/ray/pi-midi-toolkit/bin/jambox-engine
systemctl daemon-reload
systemctl restart jambox-engine
sleep 2
systemctl is-active jambox-engine pidi-native || true
journalctl -u jambox-engine -n 30 --no-pager | grep -iE 'audio:|opening|hifiberry|headphone|error|WARN' || journalctl -u jambox-engine -n 20 --no-pager
echo "rebooting for HiFiBerry exclusive overlay..."
# Give SSH a moment to flush.
sleep 1
systemctl reboot
