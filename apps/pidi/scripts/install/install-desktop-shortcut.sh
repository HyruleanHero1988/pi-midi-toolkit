#!/usr/bin/env bash
# Install PiDI into the app menu + Desktop (Raspberry Pi OS).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

APP_DIR="$DIR"
if [[ ! -f "$APP_DIR/midi_tone.py" ]]; then
  APP_DIR="${HOME}/midi-tone"
fi
if [[ ! -f "$APP_DIR/midi_tone.py" ]]; then
  echo "midi_tone.py not found. Put this folder at ~/midi-tone first."
  exit 1
fi

chmod +x "$APP_DIR/run.sh" "$APP_DIR/launch-desktop.sh" 2>/dev/null || true
if [[ ! -x "$APP_DIR/.venv/bin/python" ]]; then
  echo "No venv yet. Run: $APP_DIR/setup-venv.sh"
  exit 1
fi

EXEC="$APP_DIR/launch-desktop.sh"

write_desktop() {
  local target="$1"
  cat > "$target" << EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=PiDI
Comment=Raspberry Pi MIDI toolkit kiosk
Exec=$EXEC
Path=$APP_DIR
Icon=audio-headphones
Terminal=false
Categories=AudioVideo;Audio;Midi;
StartupNotify=true
EOF
}

mkdir -p "$HOME/.local/share/applications"
write_desktop "$HOME/.local/share/applications/pidi.desktop"
# Keep legacy name so old menu entries still work
write_desktop "$HOME/.local/share/applications/midi-tone.desktop"

if [[ -d "$HOME/Desktop" ]]; then
  write_desktop "$HOME/Desktop/pidi.desktop"
  chmod +x "$HOME/Desktop/pidi.desktop" 2>/dev/null || true
fi

echo "Installed desktop launchers (Path=$APP_DIR)."
