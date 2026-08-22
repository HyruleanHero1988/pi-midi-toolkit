#!/usr/bin/env bash
# Install the native vertical-slice UI next to jambox-engine.
# Does not replace the Tk kiosk unless STOP_KIOSK=1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${1:-}"
REMOTE_DIR="${REMOTE_DIR:-/home/pi/pi-midi-toolkit}"
STAGE="$ROOT/dist/armv7"

if [[ -z "$HOST" ]]; then
  echo "usage: $0 user@host" >&2
  echo "  STOP_KIOSK=1 $0 user@host   # also disable LightDM/kiosk" >&2
  exit 1
fi

need() {
  if [[ ! -f "$1" ]]; then
    echo "error: missing $1 — run ./deploy/build-pi-bins.sh first" >&2
    exit 1
  fi
}

need "$STAGE/jambox-engine"
need "$STAGE/pidi-native"

ssh "$HOST" "mkdir -p '$REMOTE_DIR/bin'"
scp "$STAGE/jambox-engine" "$HOST:$REMOTE_DIR/bin/jambox-engine"
scp "$STAGE/pidi-native" "$HOST:$REMOTE_DIR/bin/pidi-native"
scp "$ROOT/deploy/jambox-engine.service" "$HOST:/tmp/jambox-engine.service"
scp "$ROOT/deploy/pidi-native.service" "$HOST:/tmp/pidi-native.service"
scp "$ROOT/deploy/hardware-check.sh" "$HOST:$REMOTE_DIR/bin/hardware-check.sh"

ssh "$HOST" bash -s <<EOF
set -euo pipefail
chmod +x '$REMOTE_DIR/bin/jambox-engine' '$REMOTE_DIR/bin/pidi-native' '$REMOTE_DIR/bin/hardware-check.sh'
sudo mv /tmp/jambox-engine.service /etc/systemd/system/jambox-engine.service
sudo mv /tmp/pidi-native.service /etc/systemd/system/pidi-native.service
sudo systemctl daemon-reload
sudo systemctl enable --now jambox-engine
if [[ "${STOP_KIOSK:-}" == "1" ]]; then
  sudo systemctl stop lightdm || true
  sudo systemctl disable lightdm || true
fi
sudo systemctl enable --now pidi-native || true
sudo systemctl restart pidi-native || true
sudo systemctl status jambox-engine --no-pager || true
sudo systemctl status pidi-native --no-pager || true
EOF

echo "native slice deployed to $HOST"
echo "On the Pi: install libsdl2-2.0-0 libgles2 libgbm1 libdrm2 if missing,"
echo "stop LightDM so KMSDRM can own the panel, then:"
echo "  $REMOTE_DIR/bin/hardware-check.sh"
