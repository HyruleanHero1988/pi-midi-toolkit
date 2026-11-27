#!/bin/bash
# Runs once on the Pi 4B after first boot (pidi-firstboot.service).
set -euo pipefail

LOG=/var/log/pidi-firstboot.log
exec > >(tee -a "$LOG") 2>&1

echo "=== pidi firstboot $(date -Is) ==="

BOOTSTRAP=/boot/firmware/pidi-bootstrap
# Fallback if firmware path differs
if [[ ! -d "$BOOTSTRAP" && -d /boot/pidi-bootstrap ]]; then
  BOOTSTRAP=/boot/pidi-bootstrap
fi

USERNAME=ray
# Wait for userconf to create the login user (up to ~2 minutes).
for i in $(seq 1 60); do
  if id "$USERNAME" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
id "$USERNAME" >/dev/null 2>&1 || {
  echo "ERROR: user $USERNAME not found after wait" >&2
  exit 1
}

HOME_DIR="$(getent passwd "$USERNAME" | cut -d: -f6)"
APP_DIR="$HOME_DIR/pi-midi-toolkit"
export SUDO_USER="$USERNAME"
export USER="$USERNAME"
export APP_DIR

echo "=== install runtime packages ==="
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  alsa-utils \
  libasound2 \
  libsdl2-2.0-0 \
  libgles2 \
  libegl1 \
  libgbm1 \
  libdrm2 \
  libgl1-mesa-dri

echo "=== install tree under $APP_DIR ==="
mkdir -p "$APP_DIR/bin" "$APP_DIR/presets" "$APP_DIR/deploy"
if [[ -d "$BOOTSTRAP/deploy" ]]; then
  cp -a "$BOOTSTRAP/deploy/." "$APP_DIR/deploy/"
fi
if [[ -d "$BOOTSTRAP/bin" ]]; then
  cp -a "$BOOTSTRAP/bin/." "$APP_DIR/bin/"
  chmod +x "$APP_DIR/bin/"* || true
fi
# POWER button needs this even on thin bin-only installs.
if [[ -f "$BOOTSTRAP/session/pi-power.sh" ]]; then
  mkdir -p "$APP_DIR/apps/pidi/scripts/session"
  cp -a "$BOOTSTRAP/session/pi-power.sh" "$APP_DIR/apps/pidi/scripts/session/pi-power.sh"
  chmod +x "$APP_DIR/apps/pidi/scripts/session/pi-power.sh"
fi
chown -R "$USERNAME:$USERNAME" "$APP_DIR"

echo "=== realtime limits ==="
cat >/etc/security/limits.d/99-pi-midi-toolkit.conf <<EOF
$USERNAME    -    rtprio     95
$USERNAME    -    memlock    unlimited
EOF

echo "=== systemd units ==="
for unit in midi-engine jambox-engine pidi-native; do
  UNIT_SRC="$APP_DIR/deploy/${unit}.service"
  if [[ -f "$UNIT_SRC" ]]; then
    tmp="$(mktemp)"
    sed \
      -e "s|^User=pi$|User=$USERNAME|" \
      -e "s|/home/pi/pi-midi-toolkit|$APP_DIR|g" \
      "$UNIT_SRC" > "$tmp"
    cp "$tmp" "/etc/systemd/system/${unit}.service"
    rm -f "$tmp"
  fi
done
systemctl daemon-reload
systemctl enable midi-engine.service jambox-engine.service pidi-native.service || true

echo "=== HiFiBerry DAC+ADC Pro ==="
HB_SCRIPT=""
if [[ -f "$APP_DIR/deploy/enable-hifiberry.sh" ]]; then
  HB_SCRIPT="$APP_DIR/deploy/enable-hifiberry.sh"
elif [[ -f "$BOOTSTRAP/deploy/enable-hifiberry.sh" ]]; then
  HB_SCRIPT="$BOOTSTRAP/deploy/enable-hifiberry.sh"
fi
if [[ -n "$HB_SCRIPT" ]]; then
  bash "$HB_SCRIPT" || echo "WARNING: enable-hifiberry.sh failed" >&2
else
  echo "WARNING: enable-hifiberry.sh missing from bootstrap/deploy" >&2
fi

# Lite has no LightDM; mask if a desktop image was used instead.
systemctl stop lightdm 2>/dev/null || true
systemctl disable lightdm 2>/dev/null || true
systemctl mask lightdm 2>/dev/null || true

if [[ -x "$APP_DIR/bin/pidi-native" && -x "$APP_DIR/bin/jambox-engine" ]]; then
  echo "=== starting services ==="
  systemctl restart midi-engine.service jambox-engine.service pidi-native.service || true
  systemctl --no-pager --full status jambox-engine.service pidi-native.service || true
else
  echo "WARNING: staged bins missing; deploy from PC with:"
  echo "  python deploy_master.py --arch aarch64"
fi

touch /var/lib/pidi-firstboot.done
# Disable further runs (also gated by ConditionPathExists=!done file)
systemctl disable pidi-firstboot.service 2>/dev/null || true

echo "=== pidi firstboot complete $(date -Is) ==="
echo "Reboot once if this was the first HiFiBerry overlay apply so aplay -l shows sndrpihifiberry."
