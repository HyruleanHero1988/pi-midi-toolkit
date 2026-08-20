#!/usr/bin/env bash
# PiDI kiosk session entrypoint (Openbox + app only — no Pi desktop shell).
#
# Prefer from deploy root: ./kiosk.sh
# Or: ./bin/kiosk.sh --input MPK
set -euo pipefail
BIN="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$BIN/.." && pwd)"
cd "$ROOT"
DIR="$ROOT"

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [[ -f "$HOME/.Xauthority" ]]; then
  export XAUTHORITY="$HOME/.Xauthority"
fi
export GDK_BACKEND=x11
unset WAYLAND_DISPLAY || true
export PULSE_LATENCY_MSEC="${PULSE_LATENCY_MSEC:-100}"
export PIPEWIRE_LATENCY="${PIPEWIRE_LATENCY:-1536/44100}"
export MIDI_TONE_SPAWN="${MIDI_TONE_SPAWN:-1}"
export MIDI_TONE_JAMBOX_RT="${MIDI_TONE_JAMBOX_RT:-1}"
if [[ -x "$HOME/pi-midi-toolkit/bin/jambox-engine" ]]; then
  export MIDI_TONE_JAMBOX_BIN="${MIDI_TONE_JAMBOX_BIN:-$HOME/pi-midi-toolkit/bin/jambox-engine}"
fi

LOG=/tmp/midi-tone-kiosk.log
echo "==== pidi kiosk $(date -Is) pid=$$ display=$DISPLAY ====" >>"$LOG"

SESSION="$ROOT/scripts/session"
SPLASH_PID=""
_start_pidi_splash() {
  if pgrep -f '[s]plash-x11.py' >/dev/null 2>&1; then
    SPLASH_PID="$(pgrep -f '[s]plash-x11.py' | head -n1 || true)"
    echo "pidi splash already running pid=$SPLASH_PID" >>"$LOG"
    return 0
  fi
  if [[ -f "$ROOT/branding/pidi-splash.png" && -f "$ROOT/pidi/splash-x11.py" ]]; then
    if command -v python3 >/dev/null 2>&1; then
      python3 "$ROOT/pidi/splash-x11.py" >>"$LOG" 2>&1 &
      SPLASH_PID=$!
      echo "pidi splash pid=$SPLASH_PID" >>"$LOG"
      sleep 0.25
    fi
  fi
}
_stop_pidi_splash() {
  if [[ -n "${SPLASH_PID}" ]] && kill -0 "$SPLASH_PID" 2>/dev/null; then
    kill "$SPLASH_PID" 2>/dev/null || true
  fi
  SPLASH_PID=""
  pkill -f '[s]plash-x11.py' >/dev/null 2>&1 || true
  rm -f /tmp/pidi-splash.pid
}
_start_pidi_splash

export OPENBOX_CONFIG_DIR="${OPENBOX_CONFIG_DIR:-$ROOT/kiosk/openbox}"
mkdir -p "$HOME/.config/openbox"
for f in rc.xml autostart; do
  src="$OPENBOX_CONFIG_DIR/$f"
  dst="$HOME/.config/openbox/$f"
  if [[ -f "$src" ]]; then
    ln -sfn "$src" "$dst"
  fi
done

if command -v xset >/dev/null 2>&1; then
  xset s off >/dev/null 2>&1 || true
  xset -dpms >/dev/null 2>&1 || true
  xset s noblank >/dev/null 2>&1 || true
fi

if [[ -x "$SESSION/prefer-tft70-display.sh" ]]; then
  bash "$SESSION/prefer-tft70-display.sh" >>"$LOG" 2>&1 || true
  if ! pgrep -f '[s]plash-x11.py' >/dev/null 2>&1; then
    _start_pidi_splash
  fi
fi

if [[ -x "$SESSION/hide-touch-cursor.sh" ]]; then
  bash "$SESSION/hide-touch-cursor.sh" >>"$LOG" 2>&1 || true
fi

if command -v xinput >/dev/null 2>&1; then
  if xinput list --name-only 2>/dev/null | grep -qx "ADS7846 Touchscreen"; then
    xinput set-button-map "ADS7846 Touchscreen" 1 0 0 0 0 0 0 >/dev/null 2>&1 || true
    xinput enable "ADS7846 Touchscreen" >/dev/null 2>&1 || true
    echo "ADS7846 button map -> 1 (left)" >>"$LOG"
  else
    echo "ADS7846 not in xinput yet" >>"$LOG"
  fi
fi

amixer -c 1 set PCM 100% unmute >/dev/null 2>&1 || true
amixer set Master 100% unmute >/dev/null 2>&1 || true

ARGS=("$@")
if [[ ${#ARGS[@]} -eq 0 ]]; then
  ARGS=(--fullscreen)
  if aconnect -l 2>/dev/null | grep -qi mpk; then
    ARGS+=(--input MPK)
  fi
elif [[ " ${ARGS[*]} " != *" --fullscreen "* ]]; then
  ARGS=(--fullscreen "${ARGS[@]}")
fi

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
  _stop_pidi_splash
  pkill -f '[m]idi_tone.py' >/dev/null 2>&1 || true
  pkill -f 'python -m pidi' >/dev/null 2>&1 || true
  if [[ -n "${WM_PID}" ]] && kill -0 "$WM_PID" 2>/dev/null; then
    kill "$WM_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

while true; do
  echo "starting pidi ${ARGS[*]} at $(date -Is)" >>"$LOG"
  if [[ ! -x .venv/bin/python ]]; then
    echo "No venv — run ./setup-venv.sh" | tee -a "$LOG"
    sleep 5
    continue
  fi
  if ! pgrep -f '[s]plash-x11.py' >/dev/null 2>&1; then
    _start_pidi_splash
  fi
  MARK="kiosk-launch-$$-$(date +%s%N)"
  echo "MARK $MARK" >>"$LOG"
  (
    for _ in $(seq 1 80); do
      if awk -v m="$MARK" '
          index($0, m) { seen = 1 }
          seen && /ui: construction complete/ { found = 1; exit }
          END { exit found ? 0 : 1 }
        ' "$LOG" 2>/dev/null; then
        sleep 0.3
        break
      fi
      sleep 0.25
    done
    pkill -f '[s]plash-x11.py' >/dev/null 2>&1 || true
    rm -f /tmp/pidi-splash.pid
  ) &
  SPLASH_WATCH_PID=$!
  "$BIN/run.sh" "${ARGS[@]}" >>"$LOG" 2>&1 || true
  kill "$SPLASH_WATCH_PID" 2>/dev/null || true
  wait "$SPLASH_WATCH_PID" 2>/dev/null || true
  echo "pidi exited; restarting in 3s" >>"$LOG"
  _start_pidi_splash
  sleep 3
done
