#!/bin/bash
# Finish a partial clone: mounts already at /mnt/sdclone/root (+ boot/firmware).
set -euo pipefail

DST_DISK=sda
DST="/dev/${DST_DISK}"
MNT=/mnt/sdclone

echo "=== preflight ==="
findmnt -n "${DST}2" | grep -q "$MNT/root" || {
  echo "ERROR: ${DST}2 not mounted at $MNT/root" >&2
  exit 1
}
findmnt -n "${DST}1" | grep -q "boot/firmware" || {
  echo "ERROR: ${DST}1 not mounted under boot/firmware" >&2
  exit 1
}
df -h "$MNT/root" "$MNT/root/boot/firmware"

echo "=== rsync root (resume / complete) ==="
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
  sed -i -E "s|root=[^ ]+|root=PARTUUID=${ROOT_PARTUUID}|" "$CMDLINE"
  echo "cmdline: $(cat "$CMDLINE")"
else
  echo "ERROR: missing $CMDLINE" >&2
  exit 1
fi

FSTAB="$MNT/root/etc/fstab"
if [[ -f "$FSTAB" ]]; then
  cp -a "$FSTAB" "$FSTAB.bak-clone"
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
  echo "ERROR: missing $FSTAB" >&2
  exit 1
fi

echo "=== sync + unmount ==="
sync
umount "$MNT/root/boot/firmware"
umount "$MNT/root"
rmdir "$MNT/root" 2>/dev/null || true
rmdir "$MNT/boot" 2>/dev/null || true
rmdir "$MNT" 2>/dev/null || true

echo "=== destination final layout ==="
lsblk -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$DST"
blkid "${DST}1" "${DST}2"

echo "=== DONE: 8GB clone ready. Power off, boot Pi 2 from the 8GB card to verify. ==="
