#!/usr/bin/env bash
# Hardware + runtime checklist for the native KAOSS/drum vertical slice.
# Run on the Raspberry Pi 2 after the TFT70 and MPK are attached.
set -euo pipefail

echo "== kernel / display =="
uname -a
if command -v tvservice >/dev/null 2>&1; then
  tvservice -s || true
fi
if command -v kmsprint >/dev/null 2>&1; then
  kmsprint || true
elif command -v modetest >/dev/null 2>&1; then
  modetest -c | head -n 40 || true
fi
echo
echo "framebuffers:"
ls -l /dev/fb* 2>/dev/null || echo "(none)"
echo
echo "== touch (GT911 / Goodix) =="
echo "Select the Goodix/GT911 node in evtest and confirm slots 0-4,"
echo "independent X/Y, and tracking id -1 on lift."
if command -v evtest >/dev/null 2>&1; then
  echo "devices:"
  evtest --list-devices 2>/dev/null || true
else
  echo "evtest is not installed (sudo apt-get install evtest)"
fi
if command -v libinput >/dev/null 2>&1; then
  echo
  echo "libinput devices:"
  libinput list-devices 2>/dev/null | sed -n '1,80p' || true
fi
echo
echo "== audio =="
if command -v aplay >/dev/null 2>&1; then
  aplay -l || true
  arecord -l 2>/dev/null || true
  echo
  if aplay -l 2>/dev/null | grep -qiE 'hifiberry|DAC\+ADC'; then
    echo "HiFiBerry HAT detected. Expect jambox logs:"
    echo "  audio: running device=hw:CARD=sndrpihifiberry,... "
    echo "  audio: capture running device=hw:CARD=sndrpihifiberry,..."
    echo "Digital mixer should be unmuted (enable-hifiberry.sh / setup-pi.sh)."
  else
    echo "No HiFiBerry card yet — on Pi 4 run: sudo bash deploy/enable-hifiberry.sh && sudo reboot"
    echo "On Pi 2 / headphones, speaker cable swap should log 'output died; reopening'."
  fi
  echo
  echo "Recent engine audio lines:"
  echo "  journalctl -u jambox-engine -n 40 --no-pager | grep -iE 'audio:|hifiberry|Digital'"
fi
echo
echo "== MIDI =="
if command -v amidi >/dev/null 2>&1; then
  amidi -l || true
fi
lsusb 2>/dev/null | grep -i -E 'akai|mpk|midi' || echo "(no obvious MPK USB device)"
echo
echo "== thermal / throttle =="
if command -v vcgencmd >/dev/null 2>&1; then
  vcgencmd measure_temp || true
  vcgencmd get_throttled || true
  vcgencmd measure_clock arm || true
fi
echo
echo "== sockets / services =="
systemctl is-active jambox-engine 2>/dev/null || true
systemctl is-active pidi-native 2>/dev/null || true
ls -l /tmp/jambox.sock 2>/dev/null || echo "(no /tmp/jambox.sock — start jambox-engine)"
echo
echo "== SDL / KMS =="
echo "pidi-native should log 'SDL/GL presenter' with SDL_VIDEODRIVER=kmsdrm."
echo "LightDM/X must be stopped or KMSDRM cannot own the TFT70."
echo "If finger IDs or coordinates are wrong, keep --display sdl and use --touch evdev."
echo
echo "Slice test matrix (after pidi-native is on the TFT):"
echo "  1. Tap KAOSS — note starts; lift — note stops immediately"
echo "  2. Diagonal drag — pitch steps, CELLS follow, no flash"
echo "  3. Hold KICK — quarter-note (or selected division) repeats on the engine clock"
echo "  4. Hold KICK and tap SNARE/HAT — extra hits while the repeat continues"
echo "  5. Five contacts if evtest showed five slots"
echo "  6. killall pidi-native — engine keeps running and owned notes/repeats release"
echo "  7. journalctl -u jambox-engine — callback frames should be 512/1024/256, not ~4410"
