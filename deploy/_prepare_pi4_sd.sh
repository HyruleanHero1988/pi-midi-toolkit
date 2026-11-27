#!/bin/bash
# Flash Raspberry Pi OS Bookworm Lite 64-bit onto USB 32GB (/dev/sda)
# while this Pi 2 is booted from the 8GB card (/dev/mmcblk0).
# Then enable SSH, create user ray, and stage PiDI first-boot bootstrap.
set -euo pipefail

DST_DISK=sda
DST="/dev/${DST_DISK}"
SRC_ROOT_DISK=mmcblk0
IMG_XZ="${IMG_XZ:-/home/ray/raspios-bookworm-arm64-lite.img.xz}"
IMG_URL="${IMG_URL:-https://downloads.raspberrypi.com/raspios_oldstable_lite_arm64/images/raspios_oldstable_lite_arm64-2026-09-15/2026-09-15-raspios-bookworm-arm64-lite.img.xz}"
MNT_BOOT=/mnt/pi4boot
MNT_ROOT=/mnt/pi4root
BOOTSTRAP_SRC="${BOOTSTRAP_SRC:-/home/ray/pidi-pi4-bootstrap}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
USERNAME=ray
PASSWORD=midi

echo "=== preflight ==="
findmnt -n -o SOURCE / | grep -qx "/dev/${SRC_ROOT_DISK}p2" || {
  echo "ERROR: / is not /dev/${SRC_ROOT_DISK}p2" >&2
  exit 1
}
test -b "$DST" || { echo "ERROR: $DST missing" >&2; exit 1; }
DST_BYTES=$(blockdev --getsize64 "$DST")
# 32GB-class card (accept ~28–34 GB)
if [[ "$DST_BYTES" -lt 28000000000 || "$DST_BYTES" -gt 34000000000 ]]; then
  echo "ERROR: $DST size ${DST_BYTES}B is not a 32GB-class card" >&2
  exit 1
fi
lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT "$DST" "/dev/${SRC_ROOT_DISK}"

# Unmount destination if somehow mounted
for p in "${DST}1" "${DST}2" "$DST"; do
  umount "$p" 2>/dev/null || true
done

if [[ ! -f "$IMG_XZ" ]]; then
  echo "=== download image ==="
  echo "URL: $IMG_URL"
  wget --progress=dot:giga -O "$IMG_XZ.partial" "$IMG_URL"
  mv "$IMG_XZ.partial" "$IMG_XZ"
else
  echo "=== using existing image $IMG_XZ ==="
  ls -lh "$IMG_XZ"
fi

echo "=== flash $IMG_XZ -> $DST (this takes a while) ==="
# Stream decompress straight to the card; do not write a full .img (won't fit on 8GB root).
xzcat "$IMG_XZ" | dd of="$DST" bs=4M status=progress conv=fsync
sync
partprobe "$DST" || true
sleep 2
for i in $(seq 1 20); do
  [[ -b "${DST}1" && -b "${DST}2" ]] && break
  sleep 1
done
[[ -b "${DST}1" && -b "${DST}2" ]] || { echo "ERROR: partitions missing after flash" >&2; exit 1; }
lsblk -o NAME,SIZE,FSTYPE,LABEL "$DST"

echo "=== mount boot + root ==="
rm -rf "$MNT_BOOT" "$MNT_ROOT"
mkdir -p "$MNT_BOOT" "$MNT_ROOT"
# Bookworm: boot is vfat p1; root is ext4 p2
mount "${DST}1" "$MNT_BOOT"
mount "${DST}2" "$MNT_ROOT"

# Newer images use /boot/firmware as the mount point inside rootfs;
# the FAT partition contents are what Imager edits (ssh, userconf).
BOOT_EDIT="$MNT_BOOT"

echo "=== enable SSH + user $USERNAME ==="
HASH="$(openssl passwd -6 "$PASSWORD")"
printf '%s\n' "${USERNAME}:${HASH}" > "$BOOT_EDIT/userconf.txt"
: > "$BOOT_EDIT/ssh"
# Hostname for easier discovery (optional)
if [[ -f "$MNT_ROOT/etc/hostname" ]]; then
  echo "midi-pi4" > "$MNT_ROOT/etc/hostname"
fi
if [[ -f "$MNT_ROOT/etc/hosts" ]]; then
  sed -i 's/10\.0\.0\.1.*raspberrypi/10.0.0.1\tmidi-pi4/; s/\traspberrypi$/\tmidi-pi4/' "$MNT_ROOT/etc/hosts" || true
  # Common image hosts line: 127.0.1.1       raspberrypi
  sed -i 's/127\.0\.1\.1.*/127.0.1.1\tmidi-pi4/' "$MNT_ROOT/etc/hosts" || true
fi

echo "=== stage PiDI bootstrap on boot partition ==="
mkdir -p "$BOOT_EDIT/pidi-bootstrap/deploy" "$BOOT_EDIT/pidi-bootstrap/bin"
if [[ -d "$BOOTSTRAP_SRC" ]]; then
  cp -a "$BOOTSTRAP_SRC/deploy/." "$BOOT_EDIT/pidi-bootstrap/deploy/" 2>/dev/null || true
  cp -a "$BOOTSTRAP_SRC/bin/." "$BOOT_EDIT/pidi-bootstrap/bin/" 2>/dev/null || true
  if [[ -f "$BOOTSTRAP_SRC/firstboot.sh" ]]; then
    cp -a "$BOOTSTRAP_SRC/firstboot.sh" "$BOOT_EDIT/pidi-bootstrap/firstboot.sh"
  fi
fi
# Prefer repo copies so SD prep stays current with enable-hifiberry / firstboot.
if [[ -f "$SCRIPT_DIR/enable-hifiberry.sh" ]]; then
  cp -a "$SCRIPT_DIR/enable-hifiberry.sh" "$BOOT_EDIT/pidi-bootstrap/deploy/enable-hifiberry.sh"
fi
for unit in midi-engine jambox-engine pidi-native; do
  if [[ -f "$SCRIPT_DIR/${unit}.service" ]]; then
    cp -a "$SCRIPT_DIR/${unit}.service" "$BOOT_EDIT/pidi-bootstrap/deploy/${unit}.service"
  fi
done
if [[ -f "$SCRIPT_DIR/_pidi_pi4_firstboot.sh" ]]; then
  cp -a "$SCRIPT_DIR/_pidi_pi4_firstboot.sh" "$BOOT_EDIT/pidi-bootstrap/firstboot.sh"
fi
chmod +x "$BOOT_EDIT/pidi-bootstrap/firstboot.sh" 2>/dev/null || true
chmod +x "$BOOT_EDIT/pidi-bootstrap/bin/"* 2>/dev/null || true
chmod +x "$BOOT_EDIT/pidi-bootstrap/deploy/enable-hifiberry.sh" 2>/dev/null || true

echo "=== HiFiBerry overlays in boot config.txt (active on first boot) ==="
if [[ -f "$BOOT_EDIT/pidi-bootstrap/deploy/enable-hifiberry.sh" ]]; then
  CFG="$BOOT_EDIT/config.txt" SKIP_RUNTIME=1 bash "$BOOT_EDIT/pidi-bootstrap/deploy/enable-hifiberry.sh"
else
  echo "WARNING: enable-hifiberry.sh missing; firstboot will try again on device" >&2
fi

# Install first-boot oneshot into the rootfs (runs after userconf creates $USERNAME).
if [[ -f "$BOOT_EDIT/pidi-bootstrap/firstboot.sh" ]]; then
  mkdir -p "$MNT_ROOT/usr/local/lib/pidi"
  cp -a "$BOOT_EDIT/pidi-bootstrap/firstboot.sh" "$MNT_ROOT/usr/local/lib/pidi/firstboot.sh"
  chmod +x "$MNT_ROOT/usr/local/lib/pidi/firstboot.sh"
  cat > "$MNT_ROOT/etc/systemd/system/pidi-firstboot.service" <<'EOF'
[Unit]
Description=PiDI first-boot setup (packages, units, staged aarch64 bins)
After=local-fs.target network-online.target userconfig.service
Wants=network-online.target
ConditionPathExists=/boot/firmware/pidi-bootstrap/firstboot.sh
ConditionPathExists=!/var/lib/pidi-firstboot.done

[Service]
Type=oneshot
ExecStart=/usr/local/lib/pidi/firstboot.sh
RemainAfterExit=yes
TimeoutStartSec=0

[Install]
WantedBy=multi-user.target
EOF
  # Enable without systemctl (chroot not needed): create wants symlink
  mkdir -p "$MNT_ROOT/etc/systemd/system/multi-user.target.wants"
  ln -sf /etc/systemd/system/pidi-firstboot.service \
    "$MNT_ROOT/etc/systemd/system/multi-user.target.wants/pidi-firstboot.service"
fi

cat > "$BOOT_EDIT/PIDI-README.txt" <<EOF
PiDI Pi 4B card (Raspberry Pi OS Bookworm Lite 64-bit)

Login: ${USERNAME} / (password from lab credentials)
SSH enabled.

Boot config already has HiFiBerry DAC+ADC Pro overlays (audio=off + dacplusadcpro).

On first boot the pidi-firstboot service will:
  - install ALSA/SDL runtime packages
  - install systemd units for midi-engine / jambox-engine / pidi-native
  - re-apply HiFiBerry (service --output hifiberry + mixer unmute)
  - copy staged aarch64 binaries into ~/pi-midi-toolkit/bin

If firstboot fails (e.g. no network yet), after network is up:
  sudo bash /boot/firmware/pidi-bootstrap/firstboot.sh

Then from the PC (if bins were missing or you rebuilt):
  python deploy_master.py --arch aarch64
EOF

echo "=== sync + unmount ==="
sync
umount "$MNT_ROOT"
umount "$MNT_BOOT"
rmdir "$MNT_ROOT" "$MNT_BOOT" 2>/dev/null || true

echo "=== DONE ==="
lsblk -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$DST"
echo "Remove the 32GB card, insert into the Pi 4B, boot, wait for firstboot, then SSH as ${USERNAME}."
