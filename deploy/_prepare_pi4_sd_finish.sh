#!/bin/bash
# Finish Pi 4 SD staging after image flash (FAT can't preserve Unix ownership).
set -euo pipefail

DST=/dev/sda
MNT_BOOT=/mnt/pi4boot
MNT_ROOT=/mnt/pi4root
BOOTSTRAP_SRC=/home/ray/pidi-pi4-bootstrap
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
USERNAME=ray
PASSWORD=midi

echo "=== preflight ==="
test -b "${DST}1" && test -b "${DST}2"
findmnt -n -o SOURCE / | grep -q mmcblk0 || {
  echo "ERROR: unexpected root disk" >&2
  exit 1
}
umount "$MNT_BOOT" 2>/dev/null || true
umount "$MNT_ROOT" 2>/dev/null || true
umount "${DST}1" 2>/dev/null || true
umount "${DST}2" 2>/dev/null || true

mkdir -p "$MNT_BOOT" "$MNT_ROOT"
mount "${DST}1" "$MNT_BOOT"
mount "${DST}2" "$MNT_ROOT"

echo "=== ensure SSH + userconf ==="
HASH="$(openssl passwd -6 "$PASSWORD")"
printf '%s\n' "${USERNAME}:${HASH}" > "$MNT_BOOT/userconf.txt"
: > "$MNT_BOOT/ssh"
echo "midi-pi4" > "$MNT_ROOT/etc/hostname"
sed -i 's/127\.0\.1\.1.*/127.0.1.1\tmidi-pi4/' "$MNT_ROOT/etc/hosts" || true

echo "=== stage bootstrap onto FAT (no -a) ==="
rm -rf "$MNT_BOOT/pidi-bootstrap"
mkdir -p "$MNT_BOOT/pidi-bootstrap/deploy" "$MNT_BOOT/pidi-bootstrap/bin"
cp -r "$BOOTSTRAP_SRC/deploy/." "$MNT_BOOT/pidi-bootstrap/deploy/"
cp -r "$BOOTSTRAP_SRC/bin/." "$MNT_BOOT/pidi-bootstrap/bin/"
cp "$BOOTSTRAP_SRC/firstboot.sh" "$MNT_BOOT/pidi-bootstrap/firstboot.sh"
# Prefer repo copies so SD prep stays current with enable-hifiberry / firstboot.
if [[ -f "$SCRIPT_DIR/enable-hifiberry.sh" ]]; then
  cp "$SCRIPT_DIR/enable-hifiberry.sh" "$MNT_BOOT/pidi-bootstrap/deploy/enable-hifiberry.sh"
fi
for unit in midi-engine jambox-engine pidi-native; do
  if [[ -f "$SCRIPT_DIR/${unit}.service" ]]; then
    cp "$SCRIPT_DIR/${unit}.service" "$MNT_BOOT/pidi-bootstrap/deploy/${unit}.service"
  fi
done
if [[ -f "$SCRIPT_DIR/_pidi_pi4_firstboot.sh" ]]; then
  cp "$SCRIPT_DIR/_pidi_pi4_firstboot.sh" "$MNT_BOOT/pidi-bootstrap/firstboot.sh"
fi
chmod +x "$MNT_BOOT/pidi-bootstrap/firstboot.sh" || true
chmod +x "$MNT_BOOT/pidi-bootstrap/deploy/enable-hifiberry.sh" 2>/dev/null || true

echo "=== HiFiBerry overlays in boot config.txt (active on first boot) ==="
if [[ -f "$MNT_BOOT/pidi-bootstrap/deploy/enable-hifiberry.sh" ]]; then
  CFG="$MNT_BOOT/config.txt" SKIP_RUNTIME=1 bash "$MNT_BOOT/pidi-bootstrap/deploy/enable-hifiberry.sh"
else
  echo "WARNING: enable-hifiberry.sh missing; firstboot will try again on device" >&2
fi

echo "=== also stage into rootfs (ext4) ==="
mkdir -p "$MNT_ROOT/usr/local/lib/pidi"
cp "$MNT_BOOT/pidi-bootstrap/firstboot.sh" "$MNT_ROOT/usr/local/lib/pidi/firstboot.sh"
chmod +x "$MNT_ROOT/usr/local/lib/pidi/firstboot.sh"

# Prefill app tree so firstboot has less to copy if boot mount differs
mkdir -p "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/bin" \
         "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/deploy" \
         "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/presets"
cp -r "$MNT_BOOT/pidi-bootstrap/deploy/." "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/deploy/"
cp -r "$BOOTSTRAP_SRC/bin/." "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/bin/"
chmod +x "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/bin/"* || true
chmod +x "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/deploy/enable-hifiberry.sh" 2>/dev/null || true
# uid/gid 1000 is the default first user on Raspberry Pi OS
chown -R 1000:1000 "$MNT_ROOT/home/$USERNAME" || true

cat > "$MNT_ROOT/etc/systemd/system/pidi-firstboot.service" <<'EOF'
[Unit]
Description=PiDI first-boot setup (packages, units, staged aarch64 bins)
After=local-fs.target network-online.target
Wants=network-online.target
ConditionPathExists=!/var/lib/pidi-firstboot.done

[Service]
Type=oneshot
ExecStart=/usr/local/lib/pidi/firstboot.sh
RemainAfterExit=yes
TimeoutStartSec=0

[Install]
WantedBy=multi-user.target
EOF

mkdir -p "$MNT_ROOT/etc/systemd/system/multi-user.target.wants"
ln -sf /etc/systemd/system/pidi-firstboot.service \
  "$MNT_ROOT/etc/systemd/system/multi-user.target.wants/pidi-firstboot.service"

cat > "$MNT_BOOT/PIDI-README.txt" <<EOF
PiDI Pi 4B card — Raspberry Pi OS Bookworm Lite 64-bit

Login: ${USERNAME} / (lab password)
SSH enabled on first boot.
Hostname: midi-pi4

Boot config already has HiFiBerry DAC+ADC Pro overlays (audio=off + dacplusadcpro).
First boot runs pidi-firstboot.service (packages, units, HiFiBerry service/mixer, aarch64 bins).
If that fails (no network yet):
  sudo bash /boot/firmware/pidi-bootstrap/firstboot.sh

From the PC afterwards (rebuild/redeploy):
  python deploy_master.py --arch aarch64
EOF

echo "=== verify ==="
ls -la "$MNT_BOOT/ssh" "$MNT_BOOT/userconf.txt" "$MNT_BOOT/PIDI-README.txt"
ls -la "$MNT_BOOT/pidi-bootstrap/bin"
ls -la "$MNT_ROOT/home/$USERNAME/pi-midi-toolkit/bin"
test -L "$MNT_ROOT/etc/systemd/system/multi-user.target.wants/pidi-firstboot.service"

echo "=== sync + unmount ==="
sync
umount "$MNT_ROOT"
umount "$MNT_BOOT"
rmdir "$MNT_ROOT" "$MNT_BOOT" 2>/dev/null || true

echo "=== DONE: 32GB Pi 4B card is ready ==="
lsblk -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$DST"
