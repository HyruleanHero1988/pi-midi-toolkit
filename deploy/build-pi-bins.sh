#!/usr/bin/env bash
# Cross-compile midi-engine + jambox-engine for Raspberry Pi 2 (armv7) and
# stage them in dist/armv7/ so they can be committed.
#
# SET→UPDATE never cargo-builds on the Pi. It copies these staged files into
# ~/pi-midi-toolkit/bin/ and restarts the systemd units.
#
# Procedure (PC Linux/WSL, or a Cursor cloud-agent VM):
#   ./deploy/build-pi-bins.sh
#   git add dist/armv7
#   git commit -m "Rebuild Pi armv7 engines"
#
# Run this whenever crates/midi-engine, crates/jambox-engine, crates/pidi-native, or their
# workspace deps change. Skipping it means SET→UPDATE ships new Rust *source*
# but the box keeps running the old binaries.
#
# Env:
#   TARGET       default armv7-unknown-linux-gnueabihf
#   SKIP_APT=1   do not apt-install the cross toolchain
#   PACKAGES=midi-engine,jambox-engine
#   ALLOW_PARTIAL=1  still stage whichever crates built if one fails
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${TARGET:-armv7-unknown-linux-gnueabihf}"
STAGE="$ROOT/dist/armv7"
PACKAGES="${PACKAGES:-midi-engine,jambox-engine,pidi-native}"
SKIP_APT="${SKIP_APT:-}"
ALLOW_PARTIAL="${ALLOW_PARTIAL:-}"

cd "$ROOT"

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

log() { echo "build-pi-bins: $*"; }

ensure_ubuntu_armhf_ports() {
  # Ubuntu keeps armhf packages on ports.ubuntu.com. Adding armhf without
  # pinning archive.ubuntu.com to amd64 makes apt 404 on noble/jammy.
  log "pinning Ubuntu archive to amd64 and adding ports.ubuntu.com for armhf"
  bash "$ROOT/deploy/ubuntu-armhf-ports.sh"
}

ensure_apt_toolchain() {
  if [[ -n "$SKIP_APT" ]]; then
    return 0
  fi
  if ! need_cmd sudo; then
    return 0
  fi
  local missing=0
  need_cmd arm-linux-gnueabihf-gcc || missing=1
  if [[ ! -f /usr/lib/arm-linux-gnueabihf/pkgconfig/alsa.pc ]] \
     && [[ ! -f /usr/lib/pkgconfig/alsa.pc ]]; then
    missing=1
  fi
  if [[ ! -f /usr/lib/arm-linux-gnueabihf/pkgconfig/sdl2.pc ]]; then
    missing=1
  fi
  if [[ "$missing" -eq 0 ]]; then
    return 0
  fi
  log "installing armhf cross toolchain (gcc + alsa + SDL2/GLES)…"
  if ! sudo -n true 2>/dev/null; then
    log "sudo is not passwordless; install manually:"
    log "  sudo dpkg --add-architecture armhf"
    log "  # Ubuntu: pin archive.ubuntu.com to amd64 and add ports.ubuntu.com for armhf"
    log "  sudo apt-get update"
    log "  sudo apt-get install -y gcc-arm-linux-gnueabihf g++-arm-linux-gnueabihf pkg-config \\"
    log "    libasound2-dev:armhf libsdl2-dev:armhf libgles2-mesa-dev:armhf \\"
    log "    libegl1-mesa-dev:armhf libgbm-dev:armhf libdrm-dev:armhf"
    return 0
  fi
  sudo dpkg --add-architecture armhf
  ensure_ubuntu_armhf_ports
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gcc-arm-linux-gnueabihf \
    g++-arm-linux-gnueabihf \
    pkg-config \
    libasound2-dev:armhf \
    libsdl2-dev:armhf \
    libgles2-mesa-dev:armhf \
    libegl1-mesa-dev:armhf \
    libgbm-dev:armhf \
    libdrm-dev:armhf
}

ensure_rust_target() {
  if ! need_cmd rustup; then
    log "rustup not found; assuming $TARGET is already available"
    return 0
  fi
  rustup target add "$TARGET"
}

export_cross_env() {
  if ! need_cmd arm-linux-gnueabihf-gcc; then
    echo "error: arm-linux-gnueabihf-gcc not on PATH." >&2
    echo "Install gcc-arm-linux-gnueabihf (Debian/Ubuntu/WSL) or set SKIP_APT=1 after installing a linker." >&2
    exit 1
  fi
  export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="${CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER:-arm-linux-gnueabihf-gcc}"
  export CC_armv7_unknown_linux_gnueabihf="${CC_armv7_unknown_linux_gnueabihf:-arm-linux-gnueabihf-gcc}"
  export CXX_armv7_unknown_linux_gnueabihf="${CXX_armv7_unknown_linux_gnueabihf:-arm-linux-gnueabihf-g++}"
  export AR_armv7_unknown_linux_gnueabihf="${AR_armv7_unknown_linux_gnueabihf:-arm-linux-gnueabihf-ar}"
  export PKG_CONFIG_ALLOW_CROSS="${PKG_CONFIG_ALLOW_CROSS:-1}"
  # Prefer Debian multiarch alsa.pc so midir/cpal link against armhf libasound.
  local pc_dir=""
  for candidate in \
      /usr/lib/arm-linux-gnueabihf/pkgconfig \
      /usr/lib/pkgconfig; do
    if [[ -d "$candidate" ]]; then
      pc_dir="$candidate"
      break
    fi
  done
  if [[ -n "$pc_dir" ]]; then
    export PKG_CONFIG_LIBDIR="${PKG_CONFIG_LIBDIR:-$pc_dir}"
    export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-$pc_dir}"
  fi
  unset PKG_CONFIG_SYSROOT_DIR || true
}

IFS=',' read -r -a PKG_LIST <<< "$PACKAGES"

ensure_apt_toolchain
ensure_rust_target
export_cross_env

mkdir -p "$STAGE"

built=()
failed=()
for pkg in "${PKG_LIST[@]}"; do
  pkg="$(echo "$pkg" | tr -d '[:space:]')"
  [[ -z "$pkg" ]] && continue
  log "cargo build --release -p $pkg --target $TARGET"
  if cargo build --release -p "$pkg" --target "$TARGET"; then
    src="$ROOT/target/$TARGET/release/$pkg"
    if [[ ! -f "$src" ]]; then
      echo "error: cargo reported success but $src is missing" >&2
      failed+=("$pkg")
      continue
    fi
    cp "$src" "$STAGE/$pkg"
    chmod +x "$STAGE/$pkg"
    if need_cmd arm-linux-gnueabihf-strip; then
      arm-linux-gnueabihf-strip "$STAGE/$pkg" || true
    elif need_cmd strip; then
      strip "$STAGE/$pkg" || true
    fi
    built+=("$pkg")
    log "staged $STAGE/$pkg ($(wc -c < "$STAGE/$pkg") bytes)"
  else
    failed+=("$pkg")
  fi
done

{
  echo "target=$TARGET"
  echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host=$(uname -s) $(uname -m)"
  echo "git_sha=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "crates=$(IFS=','; echo "${built[*]}")"
} > "$STAGE/VERSION"

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "error: failed to build: ${failed[*]}" >&2
  if [[ -z "$ALLOW_PARTIAL" || ${#built[@]} -eq 0 ]]; then
    exit 1
  fi
  log "ALLOW_PARTIAL=1 — staged ${built[*]} and left the rest alone"
fi

if [[ ${#built[@]} -eq 0 ]]; then
  echo "error: no binaries staged" >&2
  exit 1
fi

log "done. Commit dist/armv7 so SET→UPDATE can install the engines:"
log "  git add dist/armv7 && git commit -m \"Rebuild Pi armv7 engines\""
