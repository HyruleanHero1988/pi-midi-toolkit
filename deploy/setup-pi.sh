#!/usr/bin/env bash
# One-time Raspberry Pi OS setup for pi-midi-toolkit (armv7 or aarch64).
# Run on the Pi:  scp + sudo bash setup-pi.sh
#
# Works on 32-bit Bookworm (lab Pi 2) and 64-bit Bookworm (Pi 4B).
# Does not configure HiFiBerry / X728 / splash — those are later OS-image tickets.
set -euo pipefail

USER_NAME="${SUDO_USER:-${USER:-pi}}"
HOME_DIR="$(getent passwd "$USER_NAME" | cut -d: -f6)"
HOME_DIR="${HOME_DIR:-/home/$USER_NAME}"
APP_DIR="${APP_DIR:-$HOME_DIR/pi-midi-toolkit}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "==> packages (ALSA + SDL/KMSDRM runtime)"
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  alsa-utils \
  libasound2 \
  libsdl2-2.0-0 \
  libgles2 \
  libegl1 \
  libgbm1 \
  libdrm2 \
  libgl1-mesa-dri

echo "==> directories"
sudo mkdir -p "$APP_DIR/bin" "$APP_DIR/presets"
sudo chown -R "$USER_NAME:$USER_NAME" "$APP_DIR"

echo "==> realtime limits for $USER_NAME"
# Allow SCHED_FIFO up to 95 and unlimited mlock for the engine user.
LIMITS=/etc/security/limits.d/99-pi-midi-toolkit.conf
sudo tee "$LIMITS" >/dev/null <<EOF
$USER_NAME    -    rtprio     95
$USER_NAME    -    memlock    unlimited
EOF

echo "==> systemd units (binaries come from deploy / dist/<arch>)"
for unit in midi-engine jambox-engine pidi-native; do
  UNIT_SRC="$SCRIPT_DIR/${unit}.service"
  if [[ -f "$UNIT_SRC" ]]; then
    # Rewrite User=/home paths when the login user is not the stock "pi".
    tmp="$(mktemp)"
    sed \
      -e "s|^User=pi$|User=$USER_NAME|" \
      -e "s|/home/pi/pi-midi-toolkit|$APP_DIR|g" \
      "$UNIT_SRC" > "$tmp"
    sudo cp "$tmp" "/etc/systemd/system/${unit}.service"
    rm -f "$tmp"
  fi
done
sudo systemctl daemon-reload
sudo systemctl enable midi-engine.service jambox-engine.service pidi-native.service || true

echo "==> display note (KMSDRM kiosk)"
echo "    pidi-native needs the DRM master. After first deploy of bins:"
echo "      sudo systemctl stop lightdm || true"
echo "      sudo systemctl disable lightdm || true"
echo "      sudo systemctl mask lightdm || true"
echo "    Then: sudo systemctl restart jambox-engine pidi-native"

ARCH="$(uname -m)"
case "$ARCH" in
  aarch64|arm64) DIST_HINT=aarch64; TARGET_HINT=aarch64-unknown-linux-gnu ;;
  *) DIST_HINT=armv7; TARGET_HINT=armv7-unknown-linux-gnueabihf ;;
esac

echo "==> done (detected uname -m=$ARCH → dist/$DIST_HINT)"
echo "Next:"
echo "  1. From your PC (Windows): .\\deploy\\build-pi-bins.ps1 -Arch $DIST_HINT"
echo "     or Linux: ARCHES=$DIST_HINT ./deploy/build-pi-bins.sh"
echo "  2. Deploy staged bins:"
echo "       python deploy_master.py --arch $DIST_HINT"
echo "       # or: PI_DIST_ARCH=$DIST_HINT USE_STAGED=1 ./deploy/deploy.sh $USER_NAME@<pi-ip>"
echo "  3. Mask LightDM if the panel stays black under KMSDRM (see above)."
echo "  4. See docs/AARCH64.md for Pi 4B 64-bit bring-up."
echo "TARGET hint for cargo: $TARGET_HINT"
