#!/usr/bin/env bash
# Cut VBUS to the USB Wi-Fi dongle except while SET → WIFI / UPDATE.
# Uses uhubctl (LAN9514 per-port power) or sysfs port disable.
# Never powers Ethernet (0424:ec00) or the hub itself.
#
# Usage:  pi-wifi-power.sh on|off|status
# sudoers (NOPASSWD):
#   ray ALL=(root) NOPASSWD: /home/ray/pi-midi-toolkit/apps/pidi/scripts/session/pi-wifi-power.sh
#
# Env:
#   PIDI_WIFI_USB_IDS   comma list (default 148f:5370 — RT5370)
#   PIDI_WIFI_USB_HUB   e.g. 1-1
#   PIDI_WIFI_USB_PORT  e.g. 2
#   PIDI_DATA_ROOT      stamp file wifi-usb.port
set -u
ACTION="${1:-}"
if [[ "$ACTION" != "on" && "$ACTION" != "off" && "$ACTION" != "status" ]]; then
  echo "usage: $0 on|off|status" >&2
  exit 2
fi

log() { echo "pi-wifi-power: $*" >&2; }

run_sys() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo -n "$@"
  fi
}

DATA_ROOT="${PIDI_DATA_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/pidi}"
STATE="${DATA_ROOT}/wifi-usb.port"
IDS="${PIDI_WIFI_USB_IDS:-148f:5370}"
ETH_IDS="0424:ec00,0424:9514,0424:9512,1d6b:0002"

mkdir -p "$DATA_ROOT" 2>/dev/null || true

id_match() {
  local pair="$1" list="$2"
  IFS=',' read -r -a items <<<"$list"
  local item
  for item in "${items[@]}"; do
    [[ "${item,,}" == "${pair,,}" ]] && return 0
  done
  return 1
}

# 1-1.3 → hub=1-1 port=3
parse_loc() {
  local loc="$1"
  if [[ "$loc" != *.* ]]; then
    return 1
  fi
  HUB="${loc%.*}"
  PORT="${loc##*.}"
  [[ -n "$HUB" && "$PORT" =~ ^[0-9]+$ ]] || return 1
  return 0
}

is_ethernet_port() {
  local hub="$1" port="$2"
  local child
  for child in /sys/bus/usb/devices/"${hub}.${port}" /sys/bus/usb/devices/"${hub}.${port}":*; do
    [[ -f "${child}/idVendor" ]] || continue
    local pair
    pair="$(cat "${child}/idVendor" 2>/dev/null):$(cat "${child}/idProduct" 2>/dev/null)"
    if id_match "$pair" "$ETH_IDS"; then
      return 0
    fi
  done
  return 1
}

find_wifi_loc() {
  local d loc vid pid pair
  for d in /sys/bus/usb/devices/*; do
    [[ -f "$d/idVendor" && -f "$d/idProduct" ]] || continue
    loc="$(basename "$d")"
    [[ "$loc" == *:* ]] && continue
    vid="$(cat "$d/idVendor" 2>/dev/null || true)"
    pid="$(cat "$d/idProduct" 2>/dev/null || true)"
    pair="${vid}:${pid}"
    if id_match "$pair" "$ETH_IDS"; then
      continue
    fi
    if id_match "$pair" "$IDS"; then
      if parse_loc "$loc"; then
        echo "$HUB $PORT"
        return 0
      fi
    fi
  done
  return 1
}

load_saved() {
  if [[ -n "${PIDI_WIFI_USB_HUB:-}" && -n "${PIDI_WIFI_USB_PORT:-}" ]]; then
    echo "${PIDI_WIFI_USB_HUB} ${PIDI_WIFI_USB_PORT}"
    return 0
  fi
  if [[ -f "$STATE" ]]; then
    # shellcheck disable=SC2162
    read hub port <"$STATE"
    if [[ -n "${hub:-}" && -n "${port:-}" ]]; then
      echo "$hub $port"
      return 0
    fi
  fi
  return 1
}

resolve_target() {
  local found
  if found="$(find_wifi_loc)"; then
    echo "$found" >"$STATE"
    echo "$found"
    return 0
  fi
  if found="$(load_saved)"; then
    echo "$found"
    return 0
  fi
  return 1
}

uhubctl_bin() {
  command -v uhubctl 2>/dev/null || true
}

sysfs_disable_path() {
  local hub="$1" port="$2"
  local cand
  for cand in \
    "/sys/bus/usb/devices/${hub}:1.0/${hub}-port${port}/disable" \
    "/sys/bus/usb/devices/${hub}:1.0/usb1-port${port}/disable" \
    "/sys/bus/usb/devices/${hub}:1.0/usb${hub}-port${port}/disable"; do
    if [[ -e "$cand" ]]; then
      echo "$cand"
      return 0
    fi
  done
  return 1
}

set_port() {
  local want="$1" hub="$2" port="$3"
  if is_ethernet_port "$hub" "$port"; then
    log "refusing hub $hub port $port — Ethernet lives there"
    return 1
  fi
  if [[ "$port" == "1" && "$hub" == "1-1" ]]; then
    # Pi 2 LAN9514 port 1 is the onboard SMSC Ethernet.
    log "refusing 1-1 port 1 (onboard Ethernet)"
    return 1
  fi

  local uh
  uh="$(uhubctl_bin)"
  if [[ -n "$uh" ]]; then
    if run_sys "$uh" -l "$hub" -p "$port" -a "$want" >/tmp/pi-wifi-power.uhub 2>&1; then
      return 0
    fi
    log "uhubctl failed: $(tr '\n' ' ' </tmp/pi-wifi-power.uhub 2>/dev/null)"
  fi

  local path
  if path="$(sysfs_disable_path "$hub" "$port")"; then
    # disable=1 means port off
    local val=0
    [[ "$want" == "off" ]] && val=1
    if echo "$val" | run_sys tee "$path" >/dev/null; then
      return 0
    fi
  fi

  if [[ "$want" == "off" ]]; then
    nmcli radio wifi off >/dev/null 2>&1 || true
    log "VBUS cut unavailable (install uhubctl); radio off only"
    return 0
  fi
  nmcli radio wifi on >/dev/null 2>&1 || true
  log "VBUS on unavailable (install uhubctl); radio on only"
  return 0
}

wait_up() {
  local i
  for i in $(seq 1 20); do
    if find_wifi_loc >/dev/null; then
      return 0
    fi
    if ip link show wlan0 >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.4
  done
  return 1
}

TARGET="$(resolve_target || true)"
if [[ -z "$TARGET" ]]; then
  if [[ "$ACTION" == "status" ]]; then
    echo "wifi-usb: unknown (dongle not enumerated, no saved port)"
    exit 0
  fi
  if [[ "$ACTION" == "off" ]]; then
    nmcli radio wifi off >/dev/null 2>&1 || true
    echo "wifi-usb: radio off (port unknown)"
    exit 0
  fi
  echo "wifi-usb: no dongle ${IDS} and no saved port" >&2
  exit 1
fi

HUB="${TARGET% *}"
PORT="${TARGET#* }"

case "$ACTION" in
  status)
    if find_wifi_loc >/dev/null; then
      echo "wifi-usb: on hub=${HUB} port=${PORT}"
    else
      echo "wifi-usb: off hub=${HUB} port=${PORT}"
    fi
    ;;
  on)
    if ! set_port on "$HUB" "$PORT"; then
      exit 1
    fi
    nmcli radio wifi on >/dev/null 2>&1 || true
    if wait_up; then
      echo "wifi-usb: on hub=${HUB} port=${PORT}"
    else
      echo "wifi-usb: powered hub=${HUB} port=${PORT} (waiting for wlan)"
    fi
    ;;
  off)
    nmcli radio wifi off >/dev/null 2>&1 || true
    sleep 0.2
    if ! set_port off "$HUB" "$PORT"; then
      exit 1
    fi
    echo "wifi-usb: off hub=${HUB} port=${PORT}"
    ;;
esac
exit 0
