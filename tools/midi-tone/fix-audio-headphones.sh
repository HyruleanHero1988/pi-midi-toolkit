#!/usr/bin/env bash
# Route Pi audio to the 3.5mm headphone jack and unmute.
set -euo pipefail
# 0=auto 1=headphones 2=hdmi  (raspi-config nonint)
if command -v raspi-config >/dev/null 2>&1; then
  sudo raspi-config nonint do_audio 1 || true
fi
# Card 1 is usually bcm2835 Headphones on Bookworm + HDMI display
amixer -c 1 set PCM 100% unmute 2>/dev/null || true
amixer -c 1 set Headphone 100% unmute 2>/dev/null || true
amixer set PCM 100% unmute 2>/dev/null || true
amixer set Master 100% unmute 2>/dev/null || true
# Older numid routing (ignore errors)
amixer cset numid=3 1 2>/dev/null || true
echo "Audio forced toward headphones (best-effort)."
aplay -l || true
