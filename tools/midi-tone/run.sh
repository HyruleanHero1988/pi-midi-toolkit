#!/usr/bin/env bash
# Run midi-tone on the Pi desktop (needs DISPLAY / local session).
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "No DISPLAY. From SSH use:"
  echo "  export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/\$(id -u)"
  echo "or run from a terminal on the Pi desktop."
  exit 1
fi

# Common Debian/Pi stutter fix: give Pulse/PipeWire more buffering
export PULSE_LATENCY_MSEC="${PULSE_LATENCY_MSEC:-80}"
export PIPEWIRE_LATENCY="${PIPEWIRE_LATENCY:-1024/44100}"

# Keep analog jack unmuted (HDMI screens often leave PCM at -inf)
amixer -c 1 set PCM 100% unmute >/dev/null 2>&1 || true

if [[ ! -d .venv ]]; then
  echo "Creating venv…"
  python3 -m venv .venv
  .venv/bin/pip install --upgrade pip
  .venv/bin/pip install -r requirements.txt
fi

exec .venv/bin/python -u midi_tone.py "$@"
