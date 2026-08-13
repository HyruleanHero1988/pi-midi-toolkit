#!/usr/bin/env bash
# Reliable poweroff/reboot for the kiosk POWER button.
# Soft systemctl often hangs on Pi (Plymouth / LightDM / DSI). Escalate from a
# detached helper so SSH/session teardown cannot cancel the force path.
#
# Usage (via sudo):  pi-power.sh reboot|poweroff
set -u
ACTION="${1:-}"
if [[ "$ACTION" != "reboot" && "$ACTION" != "poweroff" ]]; then
  echo "usage: $0 reboot|poweroff" >&2
  exit 2
fi

log() { echo "pi-power: $*" >&2; }

# Tear down the UI fast so session stop isn't what hangs the reboot.
pkill -9 -f '[m]idi_tone.py' >/dev/null 2>&1 || true
pkill -9 -f '[s]plash-x11.py' >/dev/null 2>&1 || true
pkill -9 -f '[k]iosk.sh' >/dev/null 2>&1 || true

# Allow sysrq fallback
echo 1 >/proc/sys/kernel/sysrq 2>/dev/null || true

if [[ "$ACTION" == "reboot" ]]; then
  SYSRQ=b
  FORCE_CMDS='systemctl reboot --force --no-block; sleep 2; systemctl reboot --force --force; sleep 1; printf b >/proc/sysrq-trigger'
else
  SYSRQ=o
  FORCE_CMDS='systemctl poweroff --force --no-block; sleep 2; systemctl poweroff --force --force; sleep 1; printf o >/proc/sysrq-trigger'
fi

# Detached watchdog: if orderly reboot hangs, force after a few seconds.
# Must survive LightDM/SSH dying when reboot.target starts.
log "arm force watchdog (4s)"
setsid /bin/bash -c "
  sleep 4
  echo \"pi-power: force watchdog firing\" >>/tmp/pi-power.log
  $FORCE_CMDS
" </dev/null >>/tmp/pi-power.log 2>&1 &

log "systemctl ${ACTION} --ignore-inhibitors"
if [[ "$ACTION" == "reboot" ]]; then
  systemctl reboot --ignore-inhibitors --no-block >/dev/null 2>&1 \
    || systemctl reboot --no-block >/dev/null 2>&1 \
    || true
else
  systemctl poweroff --ignore-inhibitors --no-block >/dev/null 2>&1 \
    || systemctl poweroff --no-block >/dev/null 2>&1 \
    || true
fi

# Keep this process alive briefly so the soft reboot has a parent; the
# setsid watchdog continues even if we are killed.
sleep 8
log "still alive after 8s — forcing now"
eval "$FORCE_CMDS"
exit 0
