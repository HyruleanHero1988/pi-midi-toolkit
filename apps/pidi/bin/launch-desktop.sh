#!/usr/bin/env bash
# Launch PiDI on the Pi desktop, detached from SSH/parent shells.
set -euo pipefail
BIN="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$BIN/.." && pwd)"
SESSION="$ROOT/scripts/session"
cd "$ROOT"

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [[ -f "$HOME/.Xauthority" ]]; then
  export XAUTHORITY="$HOME/.Xauthority"
fi
export GDK_BACKEND="${GDK_BACKEND:-x11}"
unset WAYLAND_DISPLAY || true

if [[ -x "$SESSION/prefer-tft70-display.sh" ]]; then
  bash "$SESSION/prefer-tft70-display.sh" >/tmp/prefer-tft70.log 2>&1 || true
fi
if [[ -x "$SESSION/hide-touch-cursor.sh" ]]; then
  bash "$SESSION/hide-touch-cursor.sh" >/tmp/hide-touch-cursor.log 2>&1 || true
fi

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
pkill -f 'python -m pidi' >/dev/null 2>&1 || true
sleep 1.0

if pgrep -f '[k]iosk\.sh|[m]idi-tone-kiosk' >/dev/null 2>&1; then
  echo "kiosk session detected — waiting for its restart loop (not double-launching)"
  for _ in 1 2 3 4 5 6 7 8; do
    sleep 0.5
    if pgrep -f '[m]idi_tone\.py|python -m pidi' >/dev/null 2>&1; then
      pgrep -af 'midi_tone|python -m pidi' || true
      exit 0
    fi
  done
  echo "kiosk did not restart in time; falling through to local launch"
fi

setsid nohup "$BIN/run.sh" "${ARGS[@]}" >/tmp/midi-tone.log 2>&1 < /dev/null &
echo "pidi launching pid=$! (log: /tmp/midi-tone.log)"
sleep 1
if pgrep -f '[m]idi_tone\.py|python -m pidi' >/dev/null; then
  pgrep -af 'midi_tone|python -m pidi' || true
else
  echo "Launch may have failed — last log:"
  tail -n 40 /tmp/midi-tone.log || true
  exit 1
fi
