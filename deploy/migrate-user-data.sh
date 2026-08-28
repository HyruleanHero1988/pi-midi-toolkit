#!/usr/bin/env bash
# Copy live user data (songs, phrases, presets, wavetables, settings) into
# PIDI_DATA_ROOT. Sources are merged oldest-first so a later tree wins on
# filename collisions. Never deletes the originals.
#
# Usage (on the Pi):
#   PIDI_DATA_ROOT=~/.local/share/pidi ./deploy/migrate-user-data.sh
set -euo pipefail

HOME_DIR="${HOME:-/home/pi}"
DATA="${PIDI_DATA_ROOT:-$HOME_DIR/.local/share/pidi}"
REPO="${PIDI_REPO_ROOT:-$HOME_DIR/pi-midi-toolkit}"
MT="${MIDI_TONE_DIR:-$HOME_DIR/midi-tone}"
DEMOS="$REPO/apps/pidi/demo-songs"

mkdir -p "$DATA"/{songs,phrases,user-presets,user-wavetables,takes}

copy_tree() {
  local src="$1" dest="$2"
  if [[ -d "$src" ]]; then
    mkdir -p "$dest"
    # -a: keep times; no --delete: merge.
    if command -v rsync >/dev/null 2>&1; then
      rsync -a "$src"/ "$dest"/
    else
      cp -a "$src"/. "$dest"/
    fi
  fi
}

copy_file() {
  local src="$1" dest="$2"
  if [[ -f "$src" ]]; then
    mkdir -p "$(dirname "$dest")"
    cp -a "$src" "$dest"
  fi
}

# Oldest (Tk midi-tone) → apps/pidi (gitignored user tree) → repo cwd
# (native kiosk default before PIDI_DATA_ROOT) → crate-local leftover.
for root in "$MT" "$REPO/apps/pidi" "$REPO"; do
  copy_tree "$root/songs" "$DATA/songs"
  copy_tree "$root/phrases" "$DATA/phrases"
  copy_tree "$root/user-presets" "$DATA/user-presets"
  copy_tree "$root/user-wavetables" "$DATA/user-wavetables"
  copy_file "$root/settings.json" "$DATA/settings.json"
done
copy_tree "$REPO/crates/pidi-native/phrases" "$DATA/phrases"

# Seed bundled demos only when the library is still empty.
if [[ -d "$DEMOS" ]] && [[ -z "$(find "$DATA/songs" -maxdepth 1 \( -name '*.mid' -o -name '*.midi' \) -print -quit 2>/dev/null)" ]]; then
  copy_tree "$DEMOS" "$DATA/songs"
  echo "seeded demo songs into $DATA/songs"
fi

echo "user data root: $DATA"
echo -n "songs: "; find "$DATA/songs" -maxdepth 1 \( -name '*.mid' -o -name '*.midi' \) 2>/dev/null | wc -l
echo -n "phrases: "; find "$DATA/phrases" -maxdepth 1 -name 'pad-*.json' 2>/dev/null | wc -l
echo -n "presets: "; find "$DATA/user-presets" -maxdepth 1 -name 'slot-*.json' 2>/dev/null | wc -l
