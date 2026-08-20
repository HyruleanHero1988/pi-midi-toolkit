#!/usr/bin/env bash
# Create venv + install deps for PiDI (Bookworm-safe).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$DIR"

sudo apt-get update
sudo apt-get install -y \
  python3-venv python3-full python3-tk \
  libportaudio2 libopenblas0

if [[ -d .venv ]]; then
  echo "Removing old .venv…"
  rm -rf .venv
fi

python3 -m venv .venv
.venv/bin/pip install --upgrade pip
.venv/bin/pip install -r requirements.txt

echo
echo "OK. Run with:"
echo "  export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/\$(id -u)"
echo "  $DIR/run.sh --input MPK"
