#!/usr/bin/env bash
# Enable HiFiBerry DAC+ADC Pro as the exclusive I2S audio path (Pi 4 / Bookworm).
#
# Idempotent. Safe to re-run.
#
# Live Pi (default):
#   sudo bash deploy/enable-hifiberry.sh
#
# Offline SD / mounted boot partition (no mixer / no systemd):
#   CFG=/mnt/pi4boot/config.txt SKIP_RUNTIME=1 sudo bash deploy/enable-hifiberry.sh
#
# What it does:
#   - dtparam=audio=off (disable bcm2835 Headphones)
#   - dtoverlay=vc4-kms-v3d,noaudio
#   - dtoverlay=hifiberry-dacplusadcpro
#   - jambox-engine.service --output hifiberry (if unit present)
#   - unmute Digital / Analogue when the card is already loaded
#
# Overlay changes need a reboot to take exclusive effect.
set -euo pipefail

CFG="${CFG:-}"
if [[ -z "$CFG" ]]; then
  if [[ -f /boot/firmware/config.txt ]]; then
    CFG=/boot/firmware/config.txt
  elif [[ -f /boot/config.txt ]]; then
    CFG=/boot/config.txt
  else
    echo "ERROR: no config.txt found (set CFG=...)" >&2
    exit 1
  fi
fi
SKIP_RUNTIME="${SKIP_RUNTIME:-0}"
UNIT="${UNIT:-/etc/systemd/system/jambox-engine.service}"

if [[ ! -f "$CFG" ]]; then
  echo "ERROR: missing $CFG" >&2
  exit 1
fi

BACKUP="${CFG}.bak-hifiberry-$(date +%Y%m%d%H%M%S)"
cp -a "$CFG" "$BACKUP"
echo "backed up $CFG -> $BACKUP"

python3 - "$CFG" <<'PY'
import sys
from pathlib import Path

cfg = Path(sys.argv[1])
text = cfg.read_text(encoding="utf-8", errors="replace")
lines = text.splitlines(keepends=True)

def strip_eol(s: str) -> str:
    return s.rstrip("\r\n")

out = []
seen_hb = False
seen_audio_off = False
i = 0
while i < len(lines):
    raw = lines[i]
    s = strip_eol(raw).strip()
    if s.startswith("dtparam=audio="):
        if not seen_audio_off:
            out.append("dtparam=audio=off\n")
            seen_audio_off = True
        i += 1
        continue
    if s.startswith("dtoverlay=vc4-kms-v3d") and "noaudio" not in s:
        rest = s[len("dtoverlay=vc4-kms-v3d"):]
        if rest.startswith(","):
            out.append(f"dtoverlay=vc4-kms-v3d,noaudio{rest}\n")
        else:
            out.append("dtoverlay=vc4-kms-v3d,noaudio\n")
        i += 1
        continue
    if s.startswith("dtoverlay=hifiberry-dacplusadcpro"):
        if not seen_hb:
            out.append("dtoverlay=hifiberry-dacplusadcpro\n")
            seen_hb = True
        i += 1
        continue
    out.append(raw if raw.endswith("\n") else raw + "\n")
    i += 1

if not seen_audio_off:
    out.append("\n# PiDI: disable onboard analog so HiFiBerry owns I2S\n")
    out.append("dtparam=audio=off\n")
if not seen_hb:
    out.append("\n# PiDI: HiFiBerry DAC+ADC Pro\n")
    out.append("dtoverlay=hifiberry-dacplusadcpro\n")

new = "".join(out)
if new != text:
    cfg.write_text(new, encoding="utf-8")
    print("config.txt updated")
else:
    print("config.txt already correct")
print("--- relevant lines ---")
for line in cfg.read_text(encoding="utf-8", errors="replace").splitlines():
    t = line.strip()
    if (
        t.startswith("dtparam=audio")
        or t.startswith("dtoverlay=vc4")
        or "hifiberry" in t
        or t.startswith("force_eeprom")
    ):
        print(t)
PY

if [[ "$SKIP_RUNTIME" != "1" ]]; then
  if [[ -f "$UNIT" ]]; then
    # Prefer HiFiBerry card name substring over bcm2835 Headphones.
    sed -i 's|--output headphone|--output hifiberry|g' "$UNIT"
    if command -v systemctl >/dev/null 2>&1; then
      systemctl daemon-reload || true
    fi
    echo "jambox-engine.service --output -> hifiberry"
    grep -E 'ExecStart|--output' "$UNIT" || true
  else
    echo "NOTE: $UNIT missing (ok during image prep; setup-pi installs it)"
  fi

  if command -v amixer >/dev/null 2>&1 && command -v aplay >/dev/null 2>&1; then
    CARD=$(aplay -l 2>/dev/null | awk -F: '/hifiberry|DAC\+ADC/ {gsub(/card /, "", $1); print $1; exit}')
    if [[ -n "${CARD:-}" ]]; then
      echo "HiFiBerry card=$CARD"
      amixer -c "$CARD" sset Digital unmute 2>/dev/null || true
      amixer -c "$CARD" sset Digital 0dB 2>/dev/null || amixer -c "$CARD" sset Digital 90% 2>/dev/null || true
      amixer -c "$CARD" sset Analogue unmute 2>/dev/null || true
      amixer -c "$CARD" sset 'ADC' unmute 2>/dev/null || true
      amixer -c "$CARD" sget Digital 2>/dev/null | head -20 || true
    else
      echo "HiFiBerry not listed in aplay -l yet (overlay needs reboot)"
    fi
  fi

  echo
  echo "DONE. Reboot required for dtoverlay/audio=off to take exclusive effect:"
  echo "  sudo reboot"
else
  echo "DONE (config only; SKIP_RUNTIME=1). Reboot the target after first boot."
fi
