#!/usr/bin/env bash
# Migrate scattered Pi user libraries into the XDG data root:
#   ${PIDI_DATA_ROOT:-$HOME/.local/share/pidi}
#
# Prefers ~/midi-tone (historical Tk / lab) over blank repo-root copies.
# Factory wavetables are NOT moved — they stay in the git/deploy tree.
#
# Usage (on the Pi, as the kiosk user):
#   ./deploy/migrate-user-data.sh
#   ./deploy/migrate-user-data.sh --dry-run
set -euo pipefail

DRY=0
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY=1
fi

HOME_DIR="${HOME:-/home/pi}"
DATA_ROOT="${PIDI_DATA_ROOT:-$HOME_DIR/.local/share/pidi}"
MIDI_TONE="${MIDI_TONE_DIR:-$HOME_DIR/midi-tone}"
REPO="${PIDI_REPO_ROOT:-$HOME_DIR/pi-midi-toolkit}"

log() { echo "migrate-user-data: $*"; }
run() {
  if [[ "$DRY" -eq 1 ]]; then
    log "DRY $* "
  else
    # shellcheck disable=SC2086
    eval "$@"
  fi
}

log "data root → $DATA_ROOT"
run "mkdir -p '$DATA_ROOT'/{songs,phrases,user-presets,user-wavetables,takes}"

merge_dir() {
  local src="$1" dest="$2" label="$3"
  if [[ ! -d "$src" ]]; then
    return 0
  fi
  local count
  count="$(find "$src" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')"
  if [[ "$count" -eq 0 ]]; then
    log "skip empty $label ($src)"
    return 0
  fi
  log "merge $label: $src → $dest ($count entries)"
  run "mkdir -p '$dest'"
  run "rsync -a '$src'/ '$dest'/"
}

merge_file() {
  local src="$1" dest="$2" label="$3"
  if [[ ! -f "$src" ]]; then
    return 0
  fi
  if [[ -f "$dest" ]] && [[ "$src" -ot "$dest" ]]; then
    log "keep newer $label at $dest"
    return 0
  fi
  log "copy $label: $src → $dest"
  run "mkdir -p \"\$(dirname '$dest')\""
  run "cp -a '$src' '$dest'"
}

# Prefer historical midi-tone library, then any non-empty repo-root leftovers.
for pair in \
  "songs:$MIDI_TONE/songs:$DATA_ROOT/songs" \
  "phrases:$MIDI_TONE/phrases:$DATA_ROOT/phrases" \
  "user-presets:$MIDI_TONE/user-presets:$DATA_ROOT/user-presets" \
  "user-wavetables:$MIDI_TONE/user-wavetables:$DATA_ROOT/user-wavetables" \
  "songs:$REPO/songs:$DATA_ROOT/songs" \
  "phrases:$REPO/phrases:$DATA_ROOT/phrases" \
  "user-presets:$REPO/user-presets:$DATA_ROOT/user-presets" \
  "user-wavetables:$REPO/apps/pidi/user-wavetables:$DATA_ROOT/user-wavetables" \
  "user-presets:$REPO/apps/pidi/user-presets:$DATA_ROOT/user-presets" \
  "songs:$REPO/apps/pidi/songs:$DATA_ROOT/songs" \
  "phrases:$REPO/apps/pidi/phrases:$DATA_ROOT/phrases"
do
  IFS=':' read -r label src dest <<<"$pair"
  merge_dir "$src" "$dest" "$label"
done

merge_file "$MIDI_TONE/settings.json" "$DATA_ROOT/settings.json" "settings"
merge_file "$REPO/settings.json" "$DATA_ROOT/settings.json" "settings"
merge_file "$REPO/apps/pidi/settings.json" "$DATA_ROOT/settings.json" "settings"

for cred in \
  "$DATA_ROOT/.wifi-credentials" \
  "$MIDI_TONE/.wifi-credentials" \
  "$REPO/.wifi-credentials" \
  "$REPO/apps/pidi/.wifi-credentials"
do
  if [[ -f "$cred" ]] && [[ "$cred" != "$DATA_ROOT/.wifi-credentials" ]]; then
    merge_file "$cred" "$DATA_ROOT/.wifi-credentials" "wifi"
    break
  fi
done
if [[ -f "$DATA_ROOT/.wifi-credentials" ]]; then
  run "chmod 600 '$DATA_ROOT/.wifi-credentials'"
fi

# Prefer Tk slot-0N names when only those exist; native also writes slot-N.
if [[ -d "$DATA_ROOT/user-presets" ]]; then
  for n in 1 2 3 4 5 6 7 8; do
    short="$DATA_ROOT/user-presets/slot-$n.json"
    padded="$DATA_ROOT/user-presets/slot-0$n.json"
    if [[ -f "$padded" && ! -f "$short" ]]; then
      log "alias preset slot-0$n → slot-$n (native)"
      run "cp -a '$padded' '$short'"
    fi
  done
fi

log "done."
log "  songs:          $(find "$DATA_ROOT/songs" -type f 2>/dev/null | wc -l | tr -d ' ')"
log "  phrases:        $(find "$DATA_ROOT/phrases" -type f 2>/dev/null | wc -l | tr -d ' ')"
log "  user-presets:   $(find "$DATA_ROOT/user-presets" -type f 2>/dev/null | wc -l | tr -d ' ')"
log "  user-wavetables:$(find "$DATA_ROOT/user-wavetables" -type f 2>/dev/null | wc -l | tr -d ' ')"
log "Point systemd at PIDI_DATA_ROOT=$DATA_ROOT and restart pidi-native / jambox-engine."
