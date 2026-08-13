#!/usr/bin/env bash
# midi-tone kiosk session entrypoint (Openbox + app only — no Pi desktop shell).
#
# Installed as an X session via install-kiosk.sh, or run manually under X:
#   ./kiosk.sh
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [[ -f "$HOME/.Xauthority" ]]; then
  export XAUTHORITY="$HOME/.Xauthority"
fi
export GDK_BACKEND=x11
unset WAYLAND_DISPLAY || true
export PULSE_LATENCY_MSEC="${PULSE_LATENCY_MSEC:-80}"
export PIPEWIRE_LATENCY="${PIPEWIRE_LATENCY:-1024/44100}"

# Point Openbox at our minimal config (no panel / desktop icons)
export OPENBOX_CONFIG_DIR="${OPENBOX_CONFIG_DIR:-$DIR/kiosk/openbox}"
mkdir -p "$HOME/.config/openbox"
# Symlink once so openbox finds rc.xml/autostart under ~/.config/openbox
for f in rc.xml autostart; do
  src="$OPENBOX_CONFIG_DIR/$f"
  dst="$HOME/.config/openbox/$f"
  if [[ -f "$src" ]]; then
    ln -sfn "$src" "$dst"
  fi
done

LOG=/tmp/midi-tone-kiosk.log
echo "==== midi-tone kiosk $(date -Is) ====" >>"$LOG"

# Disable screen blanking / DPMS if xset exists
if command -v xset >/dev/null 2>&1; then
  xset s off >/dev/null 2>&1 || true
  xset -dpms >/dev/null 2>&1 || true
  xset s noblank >/dev/null 2>&1 || true
fi

# Prefer TFT70 DSI as the only 800x480 kiosk surface (HDMI off if also plugged)
if [[ -x "$DIR/prefer-tft70-display.sh" ]]; then
  bash "$DIR/prefer-tft70-display.sh" >>"$LOG" 2>&1 || true
fi

# Hide cursor for touch; mouse motion brings it back
if [[ -x "$DIR/hide-touch-cursor.sh" ]]; then
  bash "$DIR/hide-touch-cursor.sh" >>"$LOG" 2>&1 || true
fi

# Keep analog jack unmuted (HDMI panels often mute PCM)
amixer -c 1 set PCM 100% unmute >/dev/null 2>&1 || true

# Prefer MPK when present
ARGS=("$@")
if [[ ${#ARGS[@]} -eq 0 ]]; then
  ARGS=(--fullscreen)
  if aconnect -l 2>/dev/null | grep -qi mpk; then
    ARGS+=(--input MPK)
  fi
elif [[ " ${ARGS[*]} " != *" --fullscreen "* ]]; then
  ARGS=(--fullscreen "${ARGS[@]}")
fi

# Start Openbox if we are the session leader and no WM is running yet.
# When used as an xsessions Exec, this script *is* the session.
need_wm=1
if ! command -v openbox >/dev/null 2>&1; then
  echo "openbox not installed — run ./install-kiosk.sh" | tee -a "$LOG"
  need_wm=0
elif command -v xprop >/dev/null 2>&1 && xprop -root _NET_SUPPORTING_WM_CHECK >/dev/null 2>&1; then
  # A window manager is already managing this display
  need_wm=0
fi

if [[ "$need_wm" -eq 1 ]]; then
  openbox --config-file "$HOME/.config/openbox/rc.xml" >>"$LOG" 2>&1 &
  WM_PID=$!
  sleep 0.4
else
  WM_PID=""
fi

cleanup() {
  pkill -f '[m]idi_tone.py' >/dev/null 2>&1 || true
  if [[ -n "${WM_PID}" ]] && kill -0 "$WM_PID" 2>/dev/null; then
    kill "$WM_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

# Restart loop — if the UI crashes, come back without a desktop
while true; do
  echo "starting midi-tone ${ARGS[*]} at $(date -Is)" >>"$LOG"
  if [[ ! -x .venv/bin/python ]]; then
    echo "No venv — run ./setup-venv.sh" | tee -a "$LOG"
    sleep 5
    continue
  fi
  ./run.sh "${ARGS[@]}" >>"$LOG" 2>&1 || true
  echo "midi-tone exited; restarting in 2s" >>"$LOG"
  sleep 2
done
