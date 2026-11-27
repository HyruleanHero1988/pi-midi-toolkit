#!/bin/bash
set -euo pipefail
USERNAME=ray
APP_DIR=/home/ray/pi-midi-toolkit
DATA_ROOT=/home/ray/.local/share/pidi

mkdir -p "$DATA_ROOT/user-wavetables" "$DATA_ROOT/phrases" "$DATA_ROOT/songs" "$DATA_ROOT/user-presets"
chown -R "$USERNAME:$USERNAME" "$APP_DIR" "$DATA_ROOT" || true

for unit in midi-engine jambox-engine pidi-native; do
  src="$APP_DIR/deploy/${unit}.service"
  # Strip CRLF if present, then rewrite pi -> ray everywhere needed.
  tmp="$(mktemp)"
  tr -d '\r' < "$src" | sed \
    -e "s|^User=pi$|User=$USERNAME|" \
    -e "s|^Group=pi$|Group=$USERNAME|" \
    -e "s|/home/pi/pi-midi-toolkit|$APP_DIR|g" \
    -e "s|/home/pi/.local/share/pidi|$DATA_ROOT|g" \
    > "$tmp"
  cp "$tmp" "/etc/systemd/system/${unit}.service"
  rm -f "$tmp"
  echo "=== installed ${unit}.service ==="
  grep -E '^(User|Group|WorkingDirectory|Environment=PIDI_|ExecStart)' "/etc/systemd/system/${unit}.service" || true
done

systemctl daemon-reload
systemctl restart midi-engine.service jambox-engine.service pidi-native.service
sleep 2
systemctl is-active midi-engine jambox-engine pidi-native
systemctl --no-pager --full status jambox-engine pidi-native | sed -n '1,50p'
