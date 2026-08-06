#!/usr/bin/env bash
# Deploy midi-engine binary + presets to a Raspberry Pi over SSH.
# Usage:
#   ./deploy/deploy.sh pi@192.168.1.50
#   TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh pi@192.168.1.50
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${1:?usage: deploy.sh user@host}"
TARGET="${TARGET:-}"
REMOTE_DIR="${REMOTE_DIR:-/home/pi/pi-midi-toolkit}"

cd "$ROOT"

if [[ -n "$TARGET" ]]; then
  echo "cross-building for $TARGET ..."
  cargo build --release -p midi-engine --target "$TARGET"
  BIN="$ROOT/target/$TARGET/release/midi-engine"
else
  echo "building host release (use TARGET=... for Pi cross-compile) ..."
  cargo build --release -p midi-engine
  BIN="$ROOT/target/release/midi-engine"
fi

ssh "$HOST" "mkdir -p '$REMOTE_DIR/bin' '$REMOTE_DIR/presets'"
scp "$BIN" "$HOST:$REMOTE_DIR/bin/midi-engine"
scp -r "$ROOT/presets/." "$HOST:$REMOTE_DIR/presets/"
scp "$ROOT/deploy/midi-engine.service" "$HOST:/tmp/midi-engine.service"

ssh "$HOST" bash -s <<EOF
set -euo pipefail
sudo mv /tmp/midi-engine.service /etc/systemd/system/midi-engine.service
sudo systemctl daemon-reload
if [[ ! -f '$REMOTE_DIR/presets/active.json' ]]; then
  if [[ -f '$REMOTE_DIR/presets/mpk-mini-ch3.json' ]]; then
    cp '$REMOTE_DIR/presets/mpk-mini-ch3.json' '$REMOTE_DIR/presets/active.json'
  else
    cp '$REMOTE_DIR/presets/example.json' '$REMOTE_DIR/presets/active.json'
  fi
fi
chmod +x '$REMOTE_DIR/bin/midi-engine'
sudo systemctl restart midi-engine || true
sudo systemctl status midi-engine --no-pager || true
EOF

echo "deployed to $HOST:$REMOTE_DIR"
