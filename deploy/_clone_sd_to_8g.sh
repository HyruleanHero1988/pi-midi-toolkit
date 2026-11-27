#!/bin/bash
# Clone running root disk (mmcblk0) onto USB 8GB card (sda).
# Regenerable build cruft is excluded so the image fits.
set -euo pipefail

SRC_DISK=mmcblk0
DST_DISK=sda
SRC="/dev/${SRC_DISK}"
DST="/dev/${DST_DISK}"
MNT=/mnt/sdclone

echo "=== preflight ==="
lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT,MODEL,TRAN "$SRC" "$DST"
findmnt -n -o SOURCE / | grep -qx "${SRC}p2" || {
  echo "ERROR: / is not ${SRC}p2 — refusing to clone" >&2
  exit 1
}
test -b "$DST" || { echo "ERROR: $DST missing" >&2; exit 1; }
DST_BYTES=$(blockdev --getsize64 "$DST")
# Expect ~7–8GB card, not the 32GB boot disk and not empty 0B readers.
if [[ "$DST_BYTES" -lt 6000000000 || "$DST_BYTES" -gt 9000000000 ]]; then
  echo "ERROR: $DST size ${DST_BYTES}B is not an 8GB-class card" >&2
  exit 1
fi
if findmnt "$DST" >/dev/null 2>&1 || findmnt "${DST}1" >/dev/null 2>&1 || findmnt "${DST}2" >/dev/null 2>&1; then
  echo "ERROR: destination has mounted partitions" >&2
  findmnt "$DST" "${DST}1" "${DST}2" || true
  exit 1
fi

echo "=== free apt/journal on source (safe) ==="
apt-get clean || true
journalctl --vacuum-size=32M || true
df -h /

echo "=== wipe + partition $DST ==="
umount "${DST}1" 2>/dev/null || true
umount "${DST}2" 2>/dev/null || true
wipefs -a "$DST"
parted -s "$DST" mklabel msdos
# Match source layout: FAT boot 8–520 MiB, ext4 rest.
parted -s "$DST" mkpart primary fat32 8MiB 520MiB
parted -s "$DST" mkpart primary ext4 520MiB 100%
parted -s "$DST" set 1 lba on
partprobe "$DST" || true
sleep 2
# Wait for nodes
for i in 1 2 3 4 5 6 7 8 9 10; do
  [[ -b "${DST}1" && -b "${DST}2" ]] && break
  sleep 1
done
[[ -b "${DST}1" && -b "${DST}2" ]] || { echo "ERROR: partitions not ready" >&2; exit 1; }

echo "=== format ==="
mkfs.vfat -F 32 -n bootfs "${DST}1"
# 1% reserved to maximize usable space on a tight card
mkfs.ext4 -F -L rootfs -m 1 "${DST}2"

echo "=== mount ==="
rm -rf "$MNT"
mkdir -p "$MNT/boot" "$MNT/root"
mount "${DST}2" "$MNT/root"
mkdir -p "$MNT/root/boot/firmware"
mount "${DST}1" "$MNT/root/boot/firmware"

echo "=== rsync root (excluding virtfs + build cruft) ==="
rsync -aAXH --info=progress2 --delete \
  --exclude=/dev \
  --exclude=/proc \
  --exclude=/sys \
  --exclude=/tmp \
  --exclude=/run \
  --exclude=/mnt \
  --exclude=/media \
  --exclude=/lost+found \
  --exclude=/boot/firmware \
  --exclude=/home/ray/.rustup \
  --exclude=/home/ray/.cargo \
  --exclude=/home/ray/midi-tone/.venv \
  --exclude=/home/ray/pi-midi-toolkit/target \
  --exclude=/home/ray/pi-midi-toolkit-slice/target \
  / "$MNT/root/"

echo "=== rsync boot firmware ==="
rsync -aAXH --info=progress2 --delete \
  /boot/firmware/ "$MNT/root/boot/firmware/"

echo "=== recreate empty virt dirs ==="
mkdir -p "$MNT/root"/{dev,proc,sys,tmp,run,mnt,media}
chmod 1777 "$MNT/root/tmp"

echo "=== fix root= / fstab to new PARTUUIDs ==="
DISK_ID=$(blkid -o value -s PTUUID "$DST")
BOOT_PARTUUID="${DISK_ID}-01"
ROOT_PARTUUID="${DISK_ID}-02"
echo "DISK_ID=$DISK_ID BOOT=$BOOT_PARTUUID ROOT=$ROOT_PARTUUID"

CMDLINE="$MNT/root/boot/firmware/cmdline.txt"
if [[ -f "$CMDLINE" ]]; then
  # Replace root=PARTUUID=xxxx or root=/dev/... with new PARTUUID
  sed -i -E "s|root=[^ ]+|root=PARTUUID=${ROOT_PARTUUID}|" "$CMDLINE"
  echo "cmdline: $(cat "$CMDLINE")"
else
  echo "WARNING: missing $CMDLINE" >&2
fi

FSTAB="$MNT/root/etc/fstab"
if [[ -f "$FSTAB" ]]; then
  cp -a "$FSTAB" "$FSTAB.bak-clone"
  # Rewrite typical Raspberry Pi PARTUUID fstab lines for p1/p2.
  awk -v boot="$BOOT_PARTUUID" -v root="$ROOT_PARTUUID" '
    BEGIN { done_boot=0; done_root=0 }
    $2=="/boot/firmware" || $2=="/boot" {
      print "PARTUUID=" boot "  " $2 "  vfat  defaults  0  2"
      done_boot=1
      next
    }
    $2=="/" {
      print "PARTUUID=" root "  /  ext4  defaults,noatime  0  1"
      done_root=1
      next
    }
    { print }
    END {
      if (!done_root) print "PARTUUID=" root "  /  ext4  defaults,noatime  0  1"
      if (!done_boot) print "PARTUUID=" boot "  /boot/firmware  vfat  defaults  0  2"
    }
  ' "$FSTAB.bak-clone" > "$FSTAB"
  echo "fstab:"
  cat "$FSTAB"
else
  echo "WARNING: missing $FSTAB" >&2
fi

echo "=== sync + unmount ==="
sync
umount "$MNT/root/boot/firmware"
umount "$MNT/root"
rmdir "$MNT/root" "$MNT/boot" 2>/dev/null || true
rmdir "$MNT" 2>/dev/null || true

echo "=== destination final layout ==="
lsblk -o NAME,SIZE,FSTYPE,LABEL "$DST"
blkid "${DST}1" "${DST}2"
df -h || true

echo "=== DONE: 8GB clone ready. Power off, boot Pi 2 from the 8GB card to verify. ==="
