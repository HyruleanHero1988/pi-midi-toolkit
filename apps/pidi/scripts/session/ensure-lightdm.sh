#!/usr/bin/env bash
# Keep the graphical seat alive without a hanging `systemctl restart lightdm`.
#
# `restart` on this Pi has timed out mid-stop (SIGKILL) and left LightDM dead —
# blank panel, no :0, kiosk looping on TclError. Prefer:
#   - do nothing if already active
#   - reset-failed + start if inactive/failed
# Never block forever: use --no-block where possible and short timeouts.
#
# Usage:  ensure-lightdm.sh [--force-start]
# Prefer from sudoers:  sudo -n ensure-lightdm.sh
set -u

LOG=/tmp/midi-tone-lightdm-watchdog.log
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --force-start) FORCE=1 ;;
  esac
done

log() { echo "$(date -Is) ensure-lightdm: $*" | tee -a "$LOG" >&2; }

run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo -n "$@"
  fi
}

# Already good — OTA / app re-exec must not touch the DM.
state="$(systemctl is-active lightdm 2>/dev/null || true)"
if [[ "$state" == "active" && "$FORCE" -eq 0 ]]; then
  if [[ -S /tmp/.X11-unix/X0 ]] || [[ -e /tmp/.X11-unix/X0 ]]; then
    exit 0
  fi
  log "lightdm active but no /tmp/.X11-unix/X0 — will try start/reset"
fi

if [[ "$state" == "failed" || "$state" == "inactive" || "$state" == "deactivating" || -z "$state" ]]; then
  log "lightdm state=$state — reset-failed + start"
  run_root systemctl reset-failed lightdm >/dev/null 2>&1 || true
  # --no-block so a wedged unit cannot hang the watchdog forever
  if run_root systemctl start --no-block lightdm >/dev/null 2>&1 \
    || run_root systemctl start lightdm >/dev/null 2>&1; then
    log "start issued"
  else
    log "start failed"
    exit 1
  fi
elif [[ "$FORCE" -eq 1 ]]; then
  log "force-start requested while state=$state"
  run_root systemctl reset-failed lightdm >/dev/null 2>&1 || true
  run_root systemctl start --no-block lightdm >/dev/null 2>&1 || true
fi

# Brief wait for the socket; do not spin forever
for _ in $(seq 1 20); do
  if [[ -S /tmp/.X11-unix/X0 ]] || [[ -e /tmp/.X11-unix/X0 ]]; then
    if systemctl is-active --quiet lightdm 2>/dev/null; then
      exit 0
    fi
  fi
  sleep 0.5
done

if systemctl is-active --quiet lightdm 2>/dev/null; then
  exit 0
fi
log "lightdm still not active after start attempt (state=$(systemctl is-active lightdm 2>/dev/null || true))"
exit 1
