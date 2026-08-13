#!/usr/bin/env bash
# midi-tone kiosk session entrypoint (Openbox + app only — no Pi desktop shell).
#
# Used as an X session (install-kiosk.sh) or manually under X:
#   ./kiosk.sh
#   ./kiosk.sh --input MPK
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
export PULSE_LATENCY_MSEC="${PULSE_LATENCY_MSEC:-100}"
export PIPEWIRE_LATENCY="${PIPEWIRE_LATENCY:-1536/44100}"

# Point Openbox at our minimal config (no panel / desktop icons)
export OPENBOX_CONFIG_DIR="${OPENBOX_CONFIG_DIR:-$DIR/kiosk/openbox}"
mkdir -p "$HOME/.config/openbox"
for f in rc.xml autostart; do
  src="$OPENBOX_CONFIG_DIR/$f"
  dst="$HOME/.config/openbox/$f"
  if [[ -f "$src" ]]; then
    ln -sfn "$src" "$dst"
  fi
done

LOG=/tmp/midi-tone-kiosk.log
echo "==== midi-tone kiosk $(date -Is) pid=$$ display=$DISPLAY ====" >>"$LOG"

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

# ADS7846 (legacy resistive): make sure pen-down is Button1 for Tk <ButtonPress-1>
if command -v xinput >/dev/null 2>&1; then
  if xinput list --name-only 2>/dev/null | grep -qx "ADS7846 Touchscreen"; then
    xinput set-button-map "ADS7846 Touchscreen" 1 0 0 0 0 0 0 >/dev/null 2>&1 || true
    xinput enable "ADS7846 Touchscreen" >/dev/null 2>&1 || true
    echo "ADS7846 button map -> 1 (left)" >>"$LOG"
  else
    echo "ADS7846 not in xinput yet" >>"$LOG"
  fi
fi

# Keep analog jack unmuted (HDMI panels often mute PCM)
amixer -c 1 set PCM 100% unmute >/dev/null 2>&1 || true
amixer set Master 100% unmute >/dev/null 2>&1 || true

# Prefer MPK when present; always fullscreen in kiosk
ARGS=("$@")
if [[ ${#ARGS[@]} -eq 0 ]]; then
  ARGS=(--fullscreen)
  if aconnect -l 2>/dev/null | grep -qi mpk; then
    ARGS+=(--input MPK)
  fi
elif [[ " ${ARGS[*]} " != *" --fullscreen "* ]]; then
  ARGS=(--fullscreen "${ARGS[@]}")
fi

# Tk --fullscreen fills the panel. Openbox is optional: enabling it *after*
# a live Tk window has blanked the display before. Default off; set
# MIDI_TONE_OPENBOX=1 only for a clean session start if you need EWMH.
need_wm=0
if [[ "${MIDI_TONE_OPENBOX:-0}" == "1" ]]; then
  need_wm=1
fi
if [[ "$need_wm" -eq 1 ]]; then
  if ! command -v openbox >/dev/null 2>&1; then
    echo "openbox not installed — run ./install-kiosk.sh" | tee -a "$LOG"
    need_wm=0
  elif pgrep -x openbox >/dev/null 2>&1; then
    need_wm=0
    echo "openbox already running; not starting another" >>"$LOG"
  fi
else
  echo "skipping openbox (Tk fullscreen); set MIDI_TONE_OPENBOX=1 to enable" >>"$LOG"
  pkill -x openbox >/dev/null 2>&1 || true
fi

WM_PID=""
if [[ "$need_wm" -eq 1 ]]; then
  openbox --config-file "$HOME/.config/openbox/rc.xml" >>"$LOG" 2>&1 &
  WM_PID=$!
  sleep 0.5
  echo "openbox pid=$WM_PID" >>"$LOG"
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
  echo "midi-tone exited; restarting in 3s" >>"$LOG"
  sleep 3
done
