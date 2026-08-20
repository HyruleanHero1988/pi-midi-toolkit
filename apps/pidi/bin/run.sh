#!/usr/bin/env bash
# Run PiDI on the Pi desktop (needs DISPLAY / local session).
set -euo pipefail
BIN="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$BIN/.." && pwd)"
cd "$ROOT"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "No DISPLAY. From SSH use:"
  echo "  export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/\$(id -u)"
  echo "or run from a terminal on the Pi desktop."
  exit 1
fi

export PULSE_LATENCY_MSEC="${PULSE_LATENCY_MSEC:-100}"
export PIPEWIRE_LATENCY="${PIPEWIRE_LATENCY:-1536/44100}"
export GDK_BACKEND="${GDK_BACKEND:-x11}"

amixer -c 1 set PCM 100% unmute >/dev/null 2>&1 || true

if [[ ! -d .venv ]]; then
  echo "Creating venv…"
  python3 -m venv .venv
  .venv/bin/pip install --upgrade pip
  .venv/bin/pip install -r requirements.txt
fi

export PYTHONPATH="$ROOT${PYTHONPATH:+:$PYTHONPATH}"
exec .venv/bin/python -u -m pidi "$@"
