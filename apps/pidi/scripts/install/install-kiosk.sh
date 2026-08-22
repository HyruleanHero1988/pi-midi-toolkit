#!/usr/bin/env bash
# Install midi-tone as the graphical session: X11 + Openbox + app only.
# Power on → autologin → MIDI Tone Kiosk (no Pi desktop / labwc / panel).
#
# Usage:
#   ./install-kiosk.sh              # packages + session + enable boot
#   ./install-kiosk.sh --no-boot    # packages + session only (manual pick later)
#   ./install-kiosk.sh --boot-only  # assume already installed; just enable boot
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"  # deploy root (apps/pidi or ~/midi-tone)
cd "$DIR"
USER_NAME="${SUDO_USER:-$USER}"
USER_HOME="$(getent passwd "$USER_NAME" | cut -d: -f6)"
if [[ -z "$USER_HOME" || ! -d "$USER_HOME" ]]; then
  USER_HOME="$HOME"
  USER_NAME="$USER"
fi

DO_PACKAGES=1
DO_BOOT=1
for arg in "$@"; do
  case "$arg" in
    --no-boot) DO_BOOT=0 ;;
    --boot-only) DO_PACKAGES=0 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg"
      exit 1
      ;;
  esac
done

if [[ ! -f "$DIR/midi_tone.py" ]]; then
  echo "Run this from the PiDI deploy root (found no midi_tone.py)."
  exit 1
fi

sudo_run() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

as_user() {
  if [[ "$(id -u)" -eq 0 && "$USER_NAME" != "root" ]]; then
    sudo -u "$USER_NAME" -H "$@"
  else
    "$@"
  fi
}

chmod +x "$DIR/kiosk.sh" "$DIR/run.sh" "$DIR/launch-desktop.sh" \
  "$DIR/bin/"*.sh "$DIR/scripts/install/"*.sh "$DIR/scripts/session/"*.sh \
  "$DIR/scripts/hw/"*.sh 2>/dev/null || true
sed -i 's/\r$//' "$DIR/kiosk.sh" "$DIR/bin/"*.sh "$DIR/scripts/install/"*.sh \
  "$DIR/scripts/session/"*.sh "$DIR/kiosk/openbox/autostart" "$DIR/kiosk/openbox/rc.xml" \
  "$DIR/kiosk/midi-tone-kiosk.desktop" \
  "$DIR/kiosk/lightdm/"*.conf 2>/dev/null || true

if [[ "$DO_PACKAGES" -eq 1 ]]; then
  echo "==> Installing packages (openbox + X11 bits)…"
  sudo_run apt-get update
  sudo_run apt-get install -y --no-install-recommends \
    openbox xserver-xorg xinit x11-xserver-utils x11-utils unclutter-xfixes

  if [[ ! -x "$DIR/.venv/bin/python" ]]; then
    echo "==> Creating Python venv…"
    as_user bash "$DIR/scripts/install/setup-venv.sh"
  fi

  echo "==> Installing X session: MIDI Tone Kiosk"
  SESSION_SRC="$DIR/kiosk/midi-tone-kiosk.desktop"
  SESSION_DST="/usr/share/xsessions/midi-tone-kiosk.desktop"
  TMP="$(mktemp)"
  sed "s|REPLACE_KIOSK_SH|$DIR/bin/kiosk.sh|g" "$SESSION_SRC" >"$TMP"
  sudo_run install -m 644 "$TMP" "$SESSION_DST"
  rm -f "$TMP"
  echo "    $SESSION_DST"

  echo "==> Linking Openbox config into $USER_HOME/.config/openbox"
  as_user mkdir -p "$USER_HOME/.config/openbox"
  as_user ln -sfn "$DIR/kiosk/openbox/rc.xml" "$USER_HOME/.config/openbox/rc.xml"
  as_user ln -sfn "$DIR/kiosk/openbox/autostart" "$USER_HOME/.config/openbox/autostart"
fi

if [[ "$DO_BOOT" -eq 0 ]]; then
  echo
  echo "Session installed. Enable boot later with:"
  echo "  $DIR/install-kiosk.sh --boot-only"
  echo "Or pick **MIDI Tone Kiosk** at the login session menu."
  exit 0
fi

echo "==> Enabling boot into MIDI Tone Kiosk (skip desktop shell)"

STATE_DIR="$USER_HOME/.config/midi-tone"
as_user mkdir -p "$STATE_DIR"
STATE="$STATE_DIR/kiosk-boot.state"
# Remember previous session once so disable-kiosk.sh can restore it
if [[ ! -f "$STATE" ]]; then
  PREV_SESSION=""
  if [[ -f "$USER_HOME/.dmrc" ]]; then
    PREV_SESSION="$(awk -F= '/^Session=/{print $2; exit}' "$USER_HOME/.dmrc" || true)"
  fi
  AS_FILE="/var/lib/AccountsService/users/$USER_NAME"
  if [[ -z "$PREV_SESSION" && -f "$AS_FILE" ]]; then
    PREV_SESSION="$(awk -F= '/^XSession=/{print $2; exit}' "$AS_FILE" || true)"
  fi
  {
    echo "previous_session=${PREV_SESSION:-rpd-x}"
    echo "enabled_at=$(date -Is)"
  } | as_user tee "$STATE" >/dev/null
fi

# 1) Prefer X11 over Wayland/labwc (Tk kiosk path)
if command -v raspi-config >/dev/null 2>&1; then
  echo "    raspi-config: switch to X11"
  sudo_run raspi-config nonint do_wayland W1 || true
  echo "    raspi-config: Desktop Autologin"
  # NOTE: B4 writes autologin-session=LXDE-pi-x into /etc/lightdm/lightdm.conf
  # (main file loads AFTER conf.d drop-ins and would override them).
  sudo_run raspi-config nonint do_boot_behaviour B4 || true
fi

# 2) User session preference (LightDM / gdm-ish)
as_user mkdir -p "$USER_HOME/.config"
as_user tee "$USER_HOME/.dmrc" >/dev/null <<EOF
[Desktop]
Session=midi-tone-kiosk
EOF
as_user chmod 644 "$USER_HOME/.dmrc" || true

# 3) AccountsService (used by many greeters on Bookworm)
AS_DIR="/var/lib/AccountsService/users"
AS_FILE="$AS_DIR/$USER_NAME"
if [[ -d "$AS_DIR" ]] || sudo_run mkdir -p "$AS_DIR"; then
  echo "    AccountsService XSession=midi-tone-kiosk"
  TMP_AS="$(mktemp)"
  if [[ -f "$AS_FILE" ]]; then
    # Preserve other keys; force session fields
    sudo_run cp "$AS_FILE" "$TMP_AS"
    sudo_run sed -i '/^Session=/d;/^XSession=/d;/^SystemAccount=/d' "$TMP_AS"
    if ! grep -q '^\[User\]' "$TMP_AS" 2>/dev/null; then
      printf '%s\n' '[User]' | sudo_run tee "$TMP_AS" >/dev/null
    fi
  else
    printf '%s\n' '[User]' >"$TMP_AS"
  fi
  {
    echo "Session=midi-tone-kiosk"
    echo "XSession=midi-tone-kiosk"
    echo "SystemAccount=false"
  } >>"$TMP_AS"
  sudo_run install -m 644 "$TMP_AS" "$AS_FILE"
  rm -f "$TMP_AS"
fi

# 4) LightDM seat defaults — MUST edit main lightdm.conf
# Drop-ins under conf.d/ load BEFORE /etc/lightdm/lightdm.conf, so raspi-config's
# autologin-session=LXDE-pi-x in the main file wins unless we patch it too.
if [[ -d /etc/lightdm ]]; then
  echo "    LightDM: force midi-tone-kiosk in main lightdm.conf + drop-in"
  CONF_SRC="$DIR/kiosk/lightdm/99-midi-tone-kiosk.conf"
  CONF_DST="/etc/lightdm/lightdm.conf.d/99-midi-tone-kiosk.conf"
  DISPLAY_SETUP="$DIR/kiosk/display-setup.sh"
  if [[ -f "$DISPLAY_SETUP" ]]; then
    sed -i 's/\r$//' "$DISPLAY_SETUP" 2>/dev/null || true
    chmod +x "$DISPLAY_SETUP" 2>/dev/null || true
  fi
  if [[ -f /etc/lightdm/lightdm.conf ]]; then
    sudo_run sed -i -E \
      -e 's/^#?user-session=.*/user-session=midi-tone-kiosk/' \
      -e 's/^#?autologin-session=.*/autologin-session=midi-tone-kiosk/' \
      -e "s/^#?autologin-user=.*/autologin-user=$USER_NAME/" \
      -e "s|^#?display-setup-script=.*|display-setup-script=$DISPLAY_SETUP|" \
      /etc/lightdm/lightdm.conf
    # Ensure keys exist if raspi-config left them commented/absent
    if ! grep -qE '^autologin-session=' /etc/lightdm/lightdm.conf; then
      sudo_run sed -i '/^\[Seat:\*\]/a autologin-session=midi-tone-kiosk' /etc/lightdm/lightdm.conf
    fi
    if ! grep -qE '^user-session=' /etc/lightdm/lightdm.conf; then
      sudo_run sed -i '/^\[Seat:\*\]/a user-session=midi-tone-kiosk' /etc/lightdm/lightdm.conf
    fi
    if ! grep -qE '^autologin-user=' /etc/lightdm/lightdm.conf; then
      sudo_run sed -i "/^\[Seat:\*\]/a autologin-user=$USER_NAME" /etc/lightdm/lightdm.conf
    fi
    if ! grep -qE '^display-setup-script=' /etc/lightdm/lightdm.conf; then
      sudo_run sed -i "/^\[Seat:\*\]/a display-setup-script=$DISPLAY_SETUP" /etc/lightdm/lightdm.conf
    fi
  fi
  TMP_L="$(mktemp)"
  sed -e "s|REPLACE_USER|$USER_NAME|g" \
      -e "s|REPLACE_DISPLAY_SETUP|$DISPLAY_SETUP|g" \
      "$CONF_SRC" >"$TMP_L"
  sudo_run mkdir -p /etc/lightdm/lightdm.conf.d
  sudo_run install -m 644 "$TMP_L" "$CONF_DST"
  rm -f "$TMP_L"
fi

# 5) Fallback: ~/.xsession so startx / some DMs still land in kiosk
as_user tee "$USER_HOME/.xsession" >/dev/null <<EOF
#!/bin/sh
exec "$DIR/kiosk.sh"
EOF
as_user chmod +x "$USER_HOME/.xsession"

# 6) Passwordless shutdown/reboot from the kiosk POWER button
echo "    sudoers: allow $USER_NAME pi-power.sh + systemctl poweroff/reboot"
POWER_SH="$DIR/scripts/session/pi-power.sh"
sed -i 's/\r$//' "$POWER_SH" 2>/dev/null || true
chmod +x "$POWER_SH" 2>/dev/null || true
SUDOERS_DST="/etc/sudoers.d/midi-tone-power"
TMP_S="$(mktemp)"
# Keep this file simple — complex systemctl flag lines fail visudo and the
# installer would delete the whole drop-in. Plain poweroff/reboot only.
# Resolve real paths so sudo -n matches exactly.
SYSTEMCTL_BIN="$(command -v systemctl || true)"
POWEROFF_BIN="$(command -v poweroff || true)"
REBOOT_BIN="$(command -v reboot || true)"
ENSURE_LD="$DIR/scripts/session/ensure-lightdm.sh"
sed -i 's/\r$//' "$ENSURE_LD" 2>/dev/null || true
chmod +x "$ENSURE_LD" 2>/dev/null || true
{
  echo "# midi-tone kiosk POWER button — installed by install-kiosk.sh"
  echo "$USER_NAME ALL=(root) NOPASSWD: $POWER_SH reboot, $POWER_SH poweroff"
  echo "$USER_NAME ALL=(root) NOPASSWD: $ENSURE_LD, $ENSURE_LD --force-start"
  if [[ -n "$SYSTEMCTL_BIN" ]]; then
    echo "$USER_NAME ALL=(root) NOPASSWD: $SYSTEMCTL_BIN poweroff, $SYSTEMCTL_BIN reboot"
    # Watchdog / recovery — start + reset-failed only (never restart; that hung this Pi)
    echo "$USER_NAME ALL=(root) NOPASSWD: $SYSTEMCTL_BIN start lightdm, $SYSTEMCTL_BIN start --no-block lightdm, $SYSTEMCTL_BIN reset-failed lightdm, $SYSTEMCTL_BIN is-active lightdm"
  fi
  if [[ -n "$POWEROFF_BIN" && -n "$REBOOT_BIN" ]]; then
    echo "$USER_NAME ALL=(root) NOPASSWD: $POWEROFF_BIN, $REBOOT_BIN"
  elif [[ -n "$POWEROFF_BIN" ]]; then
    echo "$USER_NAME ALL=(root) NOPASSWD: $POWEROFF_BIN"
  elif [[ -n "$REBOOT_BIN" ]]; then
    echo "$USER_NAME ALL=(root) NOPASSWD: $REBOOT_BIN"
  fi
} >"$TMP_S"
sudo_run install -m 440 "$TMP_S" "$SUDOERS_DST"
rm -f "$TMP_S"
if command -v visudo >/dev/null 2>&1; then
  sudo_run visudo -cf "$SUDOERS_DST" >/dev/null || {
    echo "WARNING: sudoers check failed — removing $SUDOERS_DST"
    sudo_run rm -f "$SUDOERS_DST"
  }
fi

# 7) LightDM watchdog — recovers a blank panel if the DM was left stopped
echo "    systemd: midi-tone-lightdm-watchdog.timer"
WATCH_SVC_SRC="$DIR/kiosk/midi-tone-lightdm-watchdog.service"
WATCH_TMR_SRC="$DIR/kiosk/midi-tone-lightdm-watchdog.timer"
WATCH_SVC_DST="/etc/systemd/system/midi-tone-lightdm-watchdog.service"
WATCH_TMR_DST="/etc/systemd/system/midi-tone-lightdm-watchdog.timer"
TMP_W="$(mktemp)"
sed "s|/home/ray/midi-tone|$DIR|g" "$WATCH_SVC_SRC" >"$TMP_W"
sudo_run install -m 644 "$TMP_W" "$WATCH_SVC_DST"
rm -f "$TMP_W"
sudo_run install -m 644 "$WATCH_TMR_SRC" "$WATCH_TMR_DST"
sudo_run systemctl daemon-reload >/dev/null 2>&1 || true
sudo_run systemctl enable --now midi-tone-lightdm-watchdog.timer >/dev/null 2>&1 || true

echo
echo "Kiosk boot enabled for user: $USER_NAME"
echo "  Session: MIDI Tone Kiosk (Openbox + midi-tone --fullscreen)"
echo "  LightDM watchdog timer: midi-tone-lightdm-watchdog.timer (every 30s)"
echo "  Optional PiDI boot splash (Plymouth, from power-on):"
echo "    $DIR/scripts/install/install-pidi-splash.sh && sudo reboot"
echo "  Reboot to apply:  sudo reboot"
echo
echo "Manual test now (from a graphical login / SSH with DISPLAY):"
echo "  $DIR/kiosk.sh"
echo "  In the app: POWER (top bar or LOG) → SHUT DOWN / REBOOT"
echo
echo "Restore the normal desktop later:"
echo "  $DIR/disable-kiosk.sh"
echo
echo "Logs: /tmp/midi-tone-kiosk.log  and  /tmp/midi-tone.log"
echo "Done."
