#!/usr/bin/env bash
# Undo install-kiosk.sh boot preference — restore a normal desktop session.
# Does not uninstall Openbox packages or the xsessions .desktop file.
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
USER_NAME="${SUDO_USER:-$USER}"
USER_HOME="$(getent passwd "$USER_NAME" | cut -d: -f6)"
if [[ -z "$USER_HOME" || ! -d "$USER_HOME" ]]; then
  USER_HOME="$HOME"
  USER_NAME="$USER"
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

STATE="$USER_HOME/.config/midi-tone/kiosk-boot.state"
PREV="rpd-x"
if [[ -f "$STATE" ]]; then
  PREV="$(awk -F= '/^previous_session=/{print $2; exit}' "$STATE" || true)"
  PREV="${PREV:-rpd-x}"
fi

# Prefer a real desktop session if the saved one was our kiosk / legacy alias
case "$PREV" in
  midi-tone-kiosk|"") PREV="LXDE-pi-x" ;;
  rpd-x|rpd-wayland) PREV="LXDE-pi-x" ;;
esac

echo "==> Restoring desktop session for $USER_NAME → $PREV"

as_user mkdir -p "$USER_HOME/.config"
as_user tee "$USER_HOME/.dmrc" >/dev/null <<EOF
[Desktop]
Session=$PREV
EOF

AS_FILE="/var/lib/AccountsService/users/$USER_NAME"
if [[ -f "$AS_FILE" ]]; then
  TMP_AS="$(mktemp)"
  sudo_run cp "$AS_FILE" "$TMP_AS"
  sudo_run sed -i '/^Session=/d;/^XSession=/d' "$TMP_AS"
  if ! grep -q '^\[User\]' "$TMP_AS" 2>/dev/null; then
    printf '%s\n' '[User]' | sudo_run tee "$TMP_AS" >/dev/null
  fi
  {
    echo "Session=$PREV"
    echo "XSession=$PREV"
  } >>"$TMP_AS"
  sudo_run install -m 644 "$TMP_AS" "$AS_FILE"
  rm -f "$TMP_AS"
fi

# Main lightdm.conf wins over conf.d — restore desktop session there too
if [[ -f /etc/lightdm/lightdm.conf ]]; then
  echo "    Restoring LightDM main conf session → $PREV"
  sudo_run sed -i -E \
    -e "s/^#?user-session=.*/user-session=$PREV/" \
    -e "s/^#?autologin-session=.*/autologin-session=$PREV/" \
    /etc/lightdm/lightdm.conf
fi

if [[ -f /etc/lightdm/lightdm.conf.d/99-midi-tone-kiosk.conf ]]; then
  echo "    Removing LightDM kiosk drop-in"
  sudo_run rm -f /etc/lightdm/lightdm.conf.d/99-midi-tone-kiosk.conf
fi

if [[ -f /etc/sudoers.d/midi-tone-power ]]; then
  echo "    Removing kiosk power sudoers"
  sudo_run rm -f /etc/sudoers.d/midi-tone-power
fi

if [[ -f "$USER_HOME/.xsession" ]] && grep -q 'kiosk.sh' "$USER_HOME/.xsession" 2>/dev/null; then
  echo "    Removing kiosk ~/.xsession"
  as_user rm -f "$USER_HOME/.xsession"
fi

as_user rm -f "$STATE"

echo
echo "Desktop session restored. Reboot to apply:  sudo reboot"
echo "Optional: return to Wayland/labwc via raspi-config → Advanced → Wayland."
echo "Kiosk session file left installed; run ./kiosk.sh anytime to test."
echo "Done."
