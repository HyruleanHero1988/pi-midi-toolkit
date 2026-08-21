#!/bin/sh
# LightDM display-setup-script: cover the panel with PiDI as soon as X exists,
# before the session script runs (prefer-tft70 / audio / etc.).
# Runs as root with DISPLAY set by LightDM.
set -eu

xsetroot -solid "#000000" 2>/dev/null || true
xset s off 2>/dev/null || true
xset -dpms 2>/dev/null || true
xset s noblank 2>/dev/null || true

SETUP_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
APP_DIR=$(CDPATH= cd -- "$SETUP_DIR/.." && pwd)
SPLASH_PY="$APP_DIR/splash-x11.py"
PIDFILE=/tmp/pidi-splash.pid

AUTH_USER=""
for conf in /etc/lightdm/lightdm.conf.d/*.conf /etc/lightdm/lightdm.conf; do
  [ -f "$conf" ] || continue
  u=$(grep -E '^autologin-user=' "$conf" 2>/dev/null | tail -1 | cut -d= -f2- || true)
  if [ -n "$u" ]; then
    AUTH_USER=$u
  fi
done
AUTH_USER=${AUTH_USER:-ray}

if [ ! -f "$SPLASH_PY" ]; then
  exit 0
fi

# Drop any stale splash from a previous session
pkill -f '[s]plash-x11.py' 2>/dev/null || true
rm -f "$PIDFILE"

# Prefer the same XAUTHORITY LightDM gave this script so the user process can paint.
run_splash() {
  if command -v runuser >/dev/null 2>&1; then
    runuser -u "$AUTH_USER" -- env \
      DISPLAY="${DISPLAY:-:0}" \
      XAUTHORITY="${XAUTHORITY:-}" \
      HOME="$(getent passwd "$AUTH_USER" | cut -d: -f6)" \
      python3 "$SPLASH_PY"
  else
    sudo -u "$AUTH_USER" env \
      DISPLAY="${DISPLAY:-:0}" \
      XAUTHORITY="${XAUTHORITY:-}" \
      HOME="$(getent passwd "$AUTH_USER" | cut -d: -f6)" \
      python3 "$SPLASH_PY"
  fi
}

# Background: keep covering until kiosk/app takes over
run_splash >/tmp/pidi-splash-x11.log 2>&1 &
echo $! >"$PIDFILE"
# Brief yield so the first frame can map before LightDM continues
sleep 0.2
exit 0
