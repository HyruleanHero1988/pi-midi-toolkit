#!/usr/bin/env bash
# One-time Raspberry Pi OS setup for pi-midi-toolkit.
# Run on the Pi:  curl ... | bash   or   scp + sudo bash setup-pi.sh
set -euo pipefail

APP_DIR="${APP_DIR:-/home/pi/pi-midi-toolkit}"
USER_NAME="${SUDO_USER:-pi}"

echo "==> packages"
sudo apt-get update
sudo apt-get install -y alsa-utils libasound2

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

echo "==> systemd unit (install binary first via deploy.sh)"
UNIT_SRC="$(cd "$(dirname "$0")" && pwd)/midi-engine.service"
if [[ -f "$UNIT_SRC" ]]; then
  sudo cp "$UNIT_SRC" /etc/systemd/system/midi-engine.service
  sudo systemctl daemon-reload
  sudo systemctl enable midi-engine.service || true
fi

echo "==> done"
echo "Next:"
echo "  1. From your PC: TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh $USER_NAME@<pi-ip>"
echo "  2. Edit $APP_DIR/presets/active.json (start from mpk-mini-ch3.json)"
echo "  3. midi-engine list   # confirm MPK + USB-DIN port names"
echo "  4. sudo systemctl restart midi-engine"
echo "Plug MPK + USB-MIDI→DIN on separate USB ports; use a powered hub if devices drop."
