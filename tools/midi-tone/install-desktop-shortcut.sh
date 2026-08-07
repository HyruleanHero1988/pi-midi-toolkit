#!/usr/bin/env bash
# Install midi-tone into the app menu + Desktop (Raspberry Pi OS).
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"

APP_DIR="$DIR"
if [[ ! -f "$APP_DIR/midi_tone.py" ]]; then
  APP_DIR="${HOME}/midi-tone"
fi
if [[ ! -f "$APP_DIR/midi_tone.py" ]]; then
  echo "midi_tone.py not found. Put this folder at ~/midi-tone first."
  exit 1
fi

if [[ ! -x "$APP_DIR/run.sh" ]]; then
  chmod +x "$APP_DIR/run.sh" 2>/dev/null || true
fi
if [[ ! -x "$APP_DIR/.venv/bin/python" ]]; then
  echo "No venv yet. Run: $APP_DIR/setup-venv.sh"
  exit 1
fi

# Prefer launch-desktop.sh so missing MPK doesn't prevent the UI from opening,
# and so SSH/desktop launches detach cleanly.
EXEC="$APP_DIR/launch-desktop.sh"

write_desktop() {
  local target="$1"
  cat > "$target" << EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=MIDI Tone
Comment=Hear/see MPK MIDI - sine diagnostic
Exec=$EXEC
Path=$APP_DIR
Icon=audio-headphones
Terminal=false
Categories=AudioVideo;Audio;Midi;
StartupNotify=true
EOF
  chmod +x "$target"
  # Raspberry Pi OS / PCManFM refuse to launch until marked trusted.
  # Clear stale checksum after rewriting the file, then re-trust.
  if command -v gio >/dev/null 2>&1; then
    gio set -d "$target" metadata::xfce-exe-checksum 2>/dev/null || true
    gio set "$target" metadata::trusted true 2>/dev/null || true
  fi
}

mkdir -p "${HOME}/.local/share/applications"
mkdir -p "${HOME}/Desktop"

write_desktop "${HOME}/.local/share/applications/midi-tone.desktop"
write_desktop "${HOME}/Desktop/midi-tone.desktop"
# Keep a copy next to the app too
write_desktop "$APP_DIR/midi-tone.desktop"

echo "Installed desktop/menu launchers:"
echo "  Exec=$EXEC"
echo "If the desktop icon still will not open: right-click -> Allow Launching"
