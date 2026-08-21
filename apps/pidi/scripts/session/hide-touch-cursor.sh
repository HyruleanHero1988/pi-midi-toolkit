#!/usr/bin/env bash
# Hide the X cursor for touch use; show it again when a real mouse moves.
#
# Prefers unclutter-xfixes --hide-on-touch (touch keeps cursor gone; mouse
# motion brings it back). Install with: sudo apt-get install -y unclutter-xfixes
set -euo pipefail

export DISPLAY="${DISPLAY:-:0}"
if [[ -f "${HOME}/.Xauthority" ]]; then
  export XAUTHORITY="${XAUTHORITY:-$HOME/.Xauthority}"
fi

# Avoid stacking helpers across kiosk restarts / redeploys.
# Note: "unclutter-xfixes" is >15 chars so pkill -x cannot match it on Linux.
pkill -f '[u]nclutter-xfixes' >/dev/null 2>&1 || true
pkill -f '[u]nclutter ' >/dev/null 2>&1 || true
pkill -x unclutter >/dev/null 2>&1 || true
pkill -x xbanish >/dev/null 2>&1 || true
sleep 0.1

if command -v unclutter-xfixes >/dev/null 2>&1; then
  # start-hidden: no cursor until mouse moves
  # hide-on-touch: touch presses hide again (mouse motion restores)
  # setsid+nohup: survive SSH / parent shell exit (launch-desktop path)
  setsid nohup unclutter-xfixes --hide-on-touch --start-hidden -b \
    >/tmp/unclutter-xfixes.log 2>&1 < /dev/null || true
  sleep 0.2
  if pgrep -f '[u]nclutter-xfixes' >/dev/null 2>&1; then
    echo "hide-touch-cursor: unclutter-xfixes (hide-on-touch, start-hidden)"
  else
    echo "hide-touch-cursor: unclutter-xfixes failed to stay up — see /tmp/unclutter-xfixes.log" >&2
    cat /tmp/unclutter-xfixes.log 2>/dev/null || true
  fi
  exit 0
fi

if command -v unclutter >/dev/null 2>&1; then
  # Classic unclutter: hide after short idle; mouse motion shows again
  setsid nohup unclutter -idle 0.5 -root \
    >/tmp/unclutter.log 2>&1 < /dev/null &
  echo "hide-touch-cursor: unclutter -idle 0.5 (fallback)"
  exit 0
fi

if command -v xbanish >/dev/null 2>&1; then
  setsid nohup xbanish >/tmp/xbanish.log 2>&1 < /dev/null &
  echo "hide-touch-cursor: xbanish (fallback)"
  exit 0
fi

echo "hide-touch-cursor: no unclutter-xfixes/unclutter/xbanish — install unclutter-xfixes" >&2
exit 0
