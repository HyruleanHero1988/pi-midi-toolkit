#!/bin/bash
# Copy lab Pi WiFi NetworkManager profiles onto the USB Pi 4 SD rootfs.
set -euo pipefail

DST=/dev/sda2
MNT=/mnt/pi4root

findmnt -n -o SOURCE / | grep -q mmcblk0 || {
  echo "ERROR: unexpected root disk" >&2
  exit 1
}
test -b "$DST" || { echo "ERROR: $DST missing" >&2; exit 1; }

umount "$MNT" 2>/dev/null || true
umount "$DST" 2>/dev/null || true
mkdir -p "$MNT"
mount "$DST" "$MNT"

DEST_DIR="$MNT/etc/NetworkManager/system-connections"
mkdir -p "$DEST_DIR"
cp -a /etc/NetworkManager/system-connections/preconfigured.nmconnection "$DEST_DIR/"
cp -a /etc/NetworkManager/system-connections/AlphaMenorahJavascript2.4Ghz.nmconnection "$DEST_DIR/"
chmod 600 "$DEST_DIR"/*.nmconnection
chown root:root "$DEST_DIR"/*.nmconnection

echo "=== staged wifi on Pi4 card ==="
ls -la "$DEST_DIR"
echo "=== connection ids / ssids (no secrets) ==="
grep -hE '^(id|ssid)=' "$DEST_DIR"/*.nmconnection || true

sync
umount "$MNT"
rmdir "$MNT" 2>/dev/null || true
echo "DONE"
