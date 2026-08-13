#!/usr/bin/env bash
# Launch midi-tone on the Pi desktop, detached from SSH/parent shells.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [[ -f "$HOME/.Xauthority" ]]; then
  export XAUTHORITY="$HOME/.Xauthority"
fi
export GDK_BACKEND="${GDK_BACKEND:-x11}"
# Tk on labwc is happier on Xwayland without inheriting Wayland as primary
unset WAYLAND_DISPLAY || true

# Prefer TFT70 DSI as the kiosk surface when present
if [[ -x "$DIR/prefer-tft70-display.sh" ]]; then
  bash "$DIR/prefer-tft70-display.sh" >/tmp/prefer-tft70.log 2>&1 || true
fi

# Hide cursor for touch; mouse motion brings it back
if [[ -x "$DIR/hide-touch-cursor.sh" ]]; then
  bash "$DIR/hide-touch-cursor.sh" >/tmp/hide-touch-cursor.log 2>&1 || true
fi

# Prefer MPK when present; still start if it isn't (UI + Midi Through fallback).
# Always fullscreen on the panel so we fill 800×480 (not the old 800×420 window).
ARGS=("$@")
if [[ ${#ARGS[@]} -eq 0 ]]; then
  ARGS=(--fullscreen)
  if aconnect -l 2>/dev/null | grep -qi mpk; then
    ARGS+=(--input MPK)
  fi
elif [[ " ${ARGS[*]} " != *" --fullscreen "* ]]; then
  ARGS=(--fullscreen "${ARGS[@]}")
fi

pkill -f '[m]idi_tone.py' >/dev/null 2>&1 || true
sleep 0.2

# setsid + nohup so SSH hangup cannot kill the GUI
setsid nohup ./run.sh "${ARGS[@]}" >/tmp/midi-tone.log 2>&1 < /dev/null &
echo "midi-tone launching pid=$! (log: /tmp/midi-tone.log)"
sleep 1
if pgrep -f '[m]idi_tone.py' >/dev/null; then
  pgrep -a midi_tone || true
else
  echo "Launch may have failed — last log:"
  tail -n 40 /tmp/midi-tone.log || true
  exit 1
fi
