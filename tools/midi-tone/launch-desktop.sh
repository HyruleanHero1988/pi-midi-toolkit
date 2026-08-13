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
# Always fullscreen on the panel so we fill 800x480 (not the old 800x420 window)
# and never leave a gray desktop behind a half-drawn / off-to-the-side Tk window.
ARGS=("$@")
have_fs=0
have_input=0
for a in "${ARGS[@]+"${ARGS[@]}"}"; do
  case "$a" in
    --fullscreen) have_fs=1 ;;
    --input) have_input=1 ;;
  esac
done
if [[ $have_fs -eq 0 ]]; then
  ARGS+=(--fullscreen)
fi
if [[ $have_input -eq 0 ]] && aconnect -l 2>/dev/null | grep -qi mpk; then
  ARGS+=(--input MPK)
fi

pkill -f '[m]idi_tone.py' >/dev/null 2>&1 || true
# Give PortAudio / Tk time to release the device and lock file
sleep 1.0

# If the kiosk session restart-loop is already owning the display, let it
# bring midi-tone back — launching a second copy here causes crunchy audio.
if pgrep -f '[k]iosk\.sh|[m]idi-tone-kiosk' >/dev/null 2>&1; then
  echo "kiosk session detected — waiting for its restart loop (not double-launching)"
  for _ in 1 2 3 4 5 6 7 8; do
    sleep 0.5
    if pgrep -f '[m]idi_tone\.py' >/dev/null 2>&1; then
      pgrep -a midi_tone || true
      exit 0
    fi
  done
  echo "kiosk did not restart midi-tone in time; falling through to local launch"
fi

# setsid + nohup so SSH hangup cannot kill the GUI
setsid nohup ./run.sh "${ARGS[@]}" >/tmp/midi-tone.log 2>&1 < /dev/null &
echo "midi-tone launching pid=$! (log: /tmp/midi-tone.log)"
sleep 1
if pgrep -f '[m]idi_tone.py' >/dev/null; then
  # Warn if somehow more than one survived
  count=$(pgrep -c -f '[m]idi_tone\.py' || true)
  pgrep -a midi_tone || true
  if [[ "${count:-0}" -gt 1 ]]; then
    echo "WARNING: ${count} midi_tone processes — killing extras"
    # Keep the newest PID
    newest=$(pgrep -f '[m]idi_tone\.py' | sort -n | tail -1)
    for pid in $(pgrep -f '[m]idi_tone\.py'); do
      if [[ "$pid" != "$newest" ]]; then
        kill "$pid" 2>/dev/null || true
      fi
    done
  fi
else
  echo "Launch may have failed — last log:"
  tail -n 40 /tmp/midi-tone.log || true
  exit 1
fi
