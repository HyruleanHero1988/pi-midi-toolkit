#!/usr/bin/env bash
# Reliable poweroff/reboot for the kiosk POWER button.
# Soft systemctl often hangs on Pi (Plymouth / LightDM / DSI). Escalate from a
# detached helper so SSH/session teardown cannot cancel the force path.
#
# Usage (via sudoers):  sudo -n pi-power.sh reboot|poweroff
# Do NOT kill the LightDM session first — that drops the greeter if poweroff
# fails or is delayed. Queue poweroff/reboot, then optionally stop the app only.
set -u
ACTION="${1:-}"
if [[ "$ACTION" != "reboot" && "$ACTION" != "poweroff" ]]; then
  echo "usage: $0 reboot|poweroff" >&2
  exit 2
fi

log() { echo "pi-power: $*" >&2; }

run_sys() {
  # Prefer plain systemctl when already root (sudoers entry for this script).
  # As a normal user, use passwordless sudo (install-kiosk.sh sudoers).
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo -n "$@"
  fi
}

# Allow sysrq fallback (root only; ignore if we lack permission)
if [[ "$(id -u)" -eq 0 ]]; then
  echo 1 >/proc/sys/kernel/sysrq 2>/dev/null || true
fi

if [[ "$ACTION" == "reboot" ]]; then
  SYSRQ=b
  FORCE_CMDS='systemctl reboot --force --no-block; sleep 2; systemctl reboot --force --force; sleep 1; printf b >/proc/sysrq-trigger'
else
  SYSRQ=o
  FORCE_CMDS='systemctl poweroff --force --no-block; sleep 2; systemctl poweroff --force --force; sleep 1; printf o >/proc/sysrq-trigger'
fi

# Detached watchdog: if orderly poweroff/reboot hangs, force after a few seconds.
# Must survive LightDM/SSH dying when reboot/poweroff.target starts.
# Only meaningful when this script runs as root (sudo -n pi-power.sh …).
if [[ "$(id -u)" -eq 0 ]]; then
  log "arm force watchdog (4s)"
  setsid /bin/bash -c "
    sleep 4
    echo \"pi-power: force watchdog firing\" >>/tmp/pi-power.log
    $FORCE_CMDS
  " </dev/null >>/tmp/pi-power.log 2>&1 &
fi

# Queue shutdown FIRST — never tear down the kiosk session before this.
# When not root, sudoers only allows plain `systemctl poweroff|reboot` (no flags).
log "systemctl ${ACTION}"
SOFT_OK=0
if [[ "$(id -u)" -eq 0 ]]; then
  if systemctl "$ACTION" --ignore-inhibitors --no-block >/tmp/pi-power-soft.err 2>&1 \
    || systemctl "$ACTION" --no-block >/tmp/pi-power-soft.err 2>&1 \
    || systemctl "$ACTION" >/tmp/pi-power-soft.err 2>&1; then
    SOFT_OK=1
  fi
else
  if run_sys systemctl "$ACTION" >/tmp/pi-power-soft.err 2>&1; then
    SOFT_OK=1
  fi
fi

if [[ "$SOFT_OK" -ne 1 ]]; then
  log "soft systemctl failed: $(tr '\n' ' ' </tmp/pi-power-soft.err 2>/dev/null)"
  # Last-resort binaries (also covered by sudoers)
  if [[ "$ACTION" == "poweroff" ]]; then
    run_sys /sbin/poweroff || run_sys /usr/sbin/poweroff || true
  else
    run_sys /sbin/reboot || run_sys /usr/sbin/reboot || true
  fi
fi

# Stop the app only after poweroff is queued (clean audio). Do not kill
# kiosk.sh / the X session — that is what lands users on the LightDM greeter
# when shutdown is blocked or delayed.
pkill -15 -f '[m]idi_tone.py' >/dev/null 2>&1 || true

if [[ "$(id -u)" -ne 0 ]]; then
  # Without root we cannot force via sysrq; soft path must have worked.
  [[ "$SOFT_OK" -eq 1 ]] && exit 0
  exit 1
fi

# Keep this process alive briefly so the soft reboot has a parent; the
# setsid watchdog continues even if we are killed.
sleep 8
log "still alive after 8s — forcing now"
eval "$FORCE_CMDS"
exit 0
