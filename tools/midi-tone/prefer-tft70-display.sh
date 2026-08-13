#!/usr/bin/env bash
# Prefer BigTreeTech Pi TFT70 (DSI-1) as the only kiosk screen.
# Safe to run at session start; no-ops if DSI is absent.
set -euo pipefail

export DISPLAY="${DISPLAY:-:0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [[ -f "$HOME/.Xauthority" ]]; then
  export XAUTHORITY="$HOME/.Xauthority"
fi

if ! command -v xrandr >/dev/null 2>&1; then
  exit 0
fi

# Wait briefly for DRM connectors after X starts
for _ in 1 2 3 4 5 6 7 8; do
  if xrandr --query 2>/dev/null | grep -q 'DSI-1 connected'; then
    break
  fi
  sleep 0.4
done

if ! xrandr --query 2>/dev/null | grep -q 'DSI-1 connected'; then
  echo "prefer-tft70: DSI-1 not connected — leaving HDMI as-is" >&2
  exit 0
fi

# TFT70 = primary 800x480 at origin; disable HDMI so the desktop isn't 1600px wide
xrandr --output DSI-1 --primary --mode 800x480 --pos 0x0 --rotate normal
if xrandr --query 2>/dev/null | grep -q 'HDMI-1 connected'; then
  xrandr --output HDMI-1 --off || true
fi

echo "prefer-tft70: DSI-1 primary 800x480 (HDMI off if present)"
xrandr --query | head -20 || true
