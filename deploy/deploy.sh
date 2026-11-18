#!/usr/bin/env bash
# Deploy midi-engine binary + presets to a Raspberry Pi over SSH.
# Usage:
#   ./deploy/deploy.sh pi@192.168.1.50
#   TARGET=armv7-unknown-linux-gnueabihf ./deploy/deploy.sh pi@192.168.1.50
#   TARGET=aarch64-unknown-linux-gnu ./deploy/deploy.sh pi@192.168.1.50
#   WITH_JAMBOX=1 ./deploy/deploy.sh pi@192.168.1.50   # also ship the audio engine
#   USE_STAGED=1 ./deploy/deploy.sh pi@192.168.1.50    # scp dist/armv7 (no cargo)
#   PI_DIST_ARCH=aarch64 USE_STAGED=1 ./deploy/deploy.sh pi@192.168.1.50
#
# Daily SET→UPDATE path needs those same ELFs committed: ./deploy/build-pi-bins.sh
# Matching TARGET builds are also copied into dist/<arch> for that commit.
#
# jambox-engine links ALSA, so cross-building it needs libasound for the target
# (e.g. via `cross` or `build-pi-bins.sh`). It is opt-in so the mapper deploy
# never breaks on that.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${1:?usage: deploy.sh user@host}"
TARGET="${TARGET:-}"
REMOTE_DIR="${REMOTE_DIR:-/home/pi/pi-midi-toolkit}"

cd "$ROOT"

USE_STAGED="${USE_STAGED:-}"
WITH_JAMBOX="${WITH_JAMBOX:-}"
# Lab default remains armv7 until the Pi 4B is the live device.
PI_DIST_ARCH="${PI_DIST_ARCH:-armv7}"
case "$PI_DIST_ARCH" in
  arm64) PI_DIST_ARCH=aarch64 ;;
esac
case "$TARGET" in
  aarch64-unknown-linux-gnu) PI_DIST_ARCH=aarch64 ;;
  armv7-unknown-linux-gnueabihf) PI_DIST_ARCH=armv7 ;;
esac
STAGE="$ROOT/dist/$PI_DIST_ARCH"

stage_for_target() {
  local src="$1" name="$2"
  case "$TARGET" in
    armv7-unknown-linux-gnueabihf|aarch64-unknown-linux-gnu) ;;
    *) return 0 ;;
  esac
  mkdir -p "$STAGE"
  cp "$src" "$STAGE/$name"
  chmod +x "$STAGE/$name"
  echo "staged $STAGE/$name (commit dist/$PI_DIST_ARCH so SET→UPDATE can install it)"
}

if [[ -n "$USE_STAGED" ]]; then
  BIN="$STAGE/midi-engine"
  if [[ ! -f "$BIN" ]]; then
    echo "error: USE_STAGED=1 but $BIN is missing — run ./deploy/build-pi-bins.sh first" >&2
    exit 1
  fi
  if [[ -n "$WITH_JAMBOX" ]]; then
    JAMBOX_BIN="$STAGE/jambox-engine"
    if [[ ! -f "$JAMBOX_BIN" ]]; then
      echo "error: USE_STAGED=1 WITH_JAMBOX=1 but $JAMBOX_BIN is missing" >&2
      exit 1
    fi
  fi
elif [[ -n "$TARGET" ]]; then
  echo "cross-building for $TARGET ..."
  cargo build --release -p midi-engine --target "$TARGET"
  BIN="$ROOT/target/$TARGET/release/midi-engine"
  stage_for_target "$BIN" midi-engine
else
  echo "building host release (use TARGET=... for Pi cross-compile) ..."
  cargo build --release -p midi-engine
  BIN="$ROOT/target/release/midi-engine"
fi

if [[ -n "$WITH_JAMBOX" && -z "$USE_STAGED" ]]; then
  if [[ -n "$TARGET" ]]; then
    cargo build --release -p jambox-engine --target "$TARGET"
    JAMBOX_BIN="$ROOT/target/$TARGET/release/jambox-engine"
    stage_for_target "$JAMBOX_BIN" jambox-engine
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
