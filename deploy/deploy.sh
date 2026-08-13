#!/usr/bin/env bash
# Deploy midi-engine binary + presets to a Raspberry Pi over SSH.
# Usage:
#   ./deploy/deploy.sh pi@192.168.1.50
#   TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh pi@192.168.1.50
#   WITH_JAMBOX=1 ./deploy/deploy.sh pi@192.168.1.50   # also ship the audio engine
#
# jambox-engine links ALSA, so cross-building it needs libasound for the target
# (e.g. via `cross`). It is opt-in so the mapper deploy never breaks on that.
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

WITH_JAMBOX="${WITH_JAMBOX:-}"
if [[ -n "$WITH_JAMBOX" ]]; then
  if [[ -n "$TARGET" ]]; then
    cargo build --release -p jambox-engine --target "$TARGET"
    JAMBOX_BIN="$ROOT/target/$TARGET/release/jambox-engine"
  else
    cargo build --release -p jambox-engine
    JAMBOX_BIN="$ROOT/target/release/jambox-engine"
  fi
fi

ssh "$HOST" "mkdir -p '$REMOTE_DIR/bin' '$REMOTE_DIR/presets'"
scp "$BIN" "$HOST:$REMOTE_DIR/bin/midi-engine"
scp -r "$ROOT/presets/." "$HOST:$REMOTE_DIR/presets/"
scp "$ROOT/deploy/midi-engine.service" "$HOST:/tmp/midi-engine.service"

if [[ -n "$WITH_JAMBOX" ]]; then
  scp "$JAMBOX_BIN" "$HOST:$REMOTE_DIR/bin/jambox-engine"
  scp "$ROOT/deploy/jambox-engine.service" "$HOST:/tmp/jambox-engine.service"
  ssh "$HOST" bash -s <<EOF
set -euo pipefail
sudo mv /tmp/jambox-engine.service /etc/systemd/system/jambox-engine.service
sudo systemctl daemon-reload
chmod +x '$REMOTE_DIR/bin/jambox-engine'
sudo systemctl restart jambox-engine || true
sudo systemctl status jambox-engine --no-pager || true
EOF
fi

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
