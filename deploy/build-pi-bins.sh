#!/usr/bin/env bash
# Cross-compile midi-engine + jambox-engine + pidi-native for Raspberry Pi
# and stage them under dist/<arch>/ so they can be committed.
#
# Architectures:
#   armv7   → armv7-unknown-linux-gnueabihf → dist/armv7   (Pi 2 / lab fallback)
#   aarch64 → aarch64-unknown-linux-gnu     → dist/aarch64 (Pi 4B 64-bit)
#
# SET→UPDATE never cargo-builds on the Pi. It copies the matching staged
# directory into ~/pi-midi-toolkit/bin/ and restarts the systemd units.
#
# Local (Windows) — preferred entry point (Ubuntu 22.04 WSL or Docker):
#   .\deploy\build-pi-bins.ps1
#   .\deploy\build-pi-bins.ps1 -Arch aarch64
#   .\deploy\build-pi-bins.ps1 -Arch both
#
# Local (already inside Ubuntu 22.04 / CI):
#   ./deploy/build-pi-bins.sh
#   ARCHES=aarch64 ./deploy/build-pi-bins.sh
#   ARCHES=armv7,aarch64 ./deploy/build-pi-bins.sh
#   TARGET=aarch64-unknown-linux-gnu ./deploy/build-pi-bins.sh
#
# On master, GitHub Actions (.github/workflows/build-pi-bins.yml) runs this
# script and commits dist/armv7 + dist/aarch64 so cloud-agent merges get OTA bins.
#
# Env:
#   ARCHES            comma list: armv7, aarch64 (default: armv7 if TARGET unset)
#   TARGET            single rustc target (overrides ARCHES when set alone)
#   SKIP_APT=1        do not apt-install the cross toolchain
#   PACKAGES=midi-engine,jambox-engine,pidi-native
#   ALLOW_PARTIAL=1   still stage whichever crates built if one fails
#   FORCE_NEW_GLIBC=1 allow host glibc > 2.36 (bins likely won't run on Pi)
#   PI_BINS_OK_GLIBC=1 set by wrappers that already verified the host
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Refuse hosts newer than Pi Bookworm unless a wrapper already vouched for them.
# shellcheck disable=SC1091
source "$ROOT/deploy/check-pi-glibc.sh"

PACKAGES="${PACKAGES:-midi-engine,jambox-engine,pidi-native}"
SKIP_APT="${SKIP_APT:-}"
ALLOW_PARTIAL="${ALLOW_PARTIAL:-}"

cd "$ROOT"

need_cmd() {
  command -v "$1" >/dev/null 2>&1
}

log() { echo "build-pi-bins: $*"; }

# Map short arch name → rustc target + stage dir + toolchain triple.
arch_meta() {
  case "$1" in
    armv7)
      echo "armv7-unknown-linux-gnueabihf armv7 armhf arm-linux-gnueabihf"
      ;;
    aarch64|arm64)
      echo "aarch64-unknown-linux-gnu aarch64 arm64 aarch64-linux-gnu"
      ;;
    *)
      return 1
      ;;
  esac
}

target_to_arch() {
  case "$1" in
    armv7-unknown-linux-gnueabihf) echo armv7 ;;
    aarch64-unknown-linux-gnu) echo aarch64 ;;
    *) return 1 ;;
  esac
}

resolve_arch_list() {
  local -a out=()
  if [[ -n "${ARCHES:-}" ]]; then
    local IFS=','
    local a
    for a in $ARCHES; do
      a="$(echo "$a" | tr -d '[:space:]')"
      [[ -z "$a" ]] && continue
      case "$a" in
        arm64) a=aarch64 ;;
      esac
      if ! arch_meta "$a" >/dev/null; then
        echo "error: unknown ARCHES entry '$a' (want armv7 or aarch64)" >&2
        exit 1
      fi
      out+=("$a")
    done
  elif [[ -n "${TARGET:-}" ]]; then
    local a
    a="$(target_to_arch "$TARGET")" || {
      echo "error: unsupported TARGET='$TARGET' (want armv7-unknown-linux-gnueabihf or aarch64-unknown-linux-gnu)" >&2
      exit 1
    }
    out+=("$a")
  else
    out+=(armv7)
  fi
  if [[ ${#out[@]} -eq 0 ]]; then
    echo "error: no architectures to build" >&2
    exit 1
  fi
  printf '%s\n' "${out[@]}"
}

ensure_ubuntu_ports() {
  local multiarch="$1"
  if [[ "$multiarch" == "armhf" ]]; then
    log "pinning Ubuntu archive to amd64 and adding ports.ubuntu.com for armhf"
    bash "$ROOT/deploy/ubuntu-armhf-ports.sh"
  elif [[ "$multiarch" == "arm64" ]]; then
    log "pinning Ubuntu archive to amd64 and adding ports.ubuntu.com for arm64"
    bash "$ROOT/deploy/ubuntu-arm64-ports.sh"
  fi
}

ensure_apt_toolchain() {
  local multiarch="$1" gcc_prefix="$2"
  if [[ -n "$SKIP_APT" ]]; then
    return 0
  fi
  if ! need_cmd sudo; then
    return 0
  fi
  local missing=0
  need_cmd "${gcc_prefix}-gcc" || missing=1
  local pc_alsa="/usr/lib/${gcc_prefix}/pkgconfig/alsa.pc"
  local pc_sdl="/usr/lib/${gcc_prefix}/pkgconfig/sdl2.pc"
  if [[ ! -f "$pc_alsa" && ! -f /usr/lib/pkgconfig/alsa.pc ]]; then
    missing=1
  fi
  if [[ ! -f "$pc_sdl" ]]; then
    missing=1
  fi
  if [[ "$missing" -eq 0 ]]; then
    return 0
  fi
  log "installing ${multiarch} cross toolchain (gcc + alsa + SDL2/GLES)…"
  if ! sudo -n true 2>/dev/null; then
    log "sudo is not passwordless; install manually:"
    log "  sudo dpkg --add-architecture ${multiarch}"
    log "  # Ubuntu: pin archive.ubuntu.com to amd64 and add ports.ubuntu.com for ${multiarch}"
    log "  sudo apt-get update"
    if [[ "$multiarch" == "armhf" ]]; then
      log "  sudo apt-get install -y gcc-arm-linux-gnueabihf g++-arm-linux-gnueabihf pkg-config \\"
      log "    libasound2-dev:armhf libsdl2-dev:armhf libgles2-mesa-dev:armhf \\"
      log "    libegl1-mesa-dev:armhf libgbm-dev:armhf libdrm-dev:armhf"
    else
      log "  sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu pkg-config \\"
      log "    libasound2-dev:arm64 libsdl2-dev:arm64 libgles2-mesa-dev:arm64 \\"
      log "    libegl1-mesa-dev:arm64 libgbm-dev:arm64 libdrm-dev:arm64"
    fi
    return 0
  fi
  sudo dpkg --add-architecture "$multiarch"
  ensure_ubuntu_ports "$multiarch"
  sudo apt-get update -qq
  if [[ "$multiarch" == "armhf" ]]; then
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
  else
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      gcc-aarch64-linux-gnu \
      g++-aarch64-linux-gnu \
      pkg-config \
      libasound2-dev:arm64 \
      libsdl2-dev:arm64 \
      libgles2-mesa-dev:arm64 \
      libegl1-mesa-dev:arm64 \
      libgbm-dev:arm64 \
      libdrm-dev:arm64
  fi
}

ensure_rust_target() {
  local target="$1"
  if ! need_cmd rustup; then
    log "rustup not found; assuming $target is already available"
    return 0
  fi
  rustup target add "$target"
}

export_cross_env() {
  local target="$1" gcc_prefix="$2"
  if ! need_cmd "${gcc_prefix}-gcc"; then
    echo "error: ${gcc_prefix}-gcc not on PATH." >&2
    echo "Install gcc-${gcc_prefix} (Debian/Ubuntu/WSL) or set SKIP_APT=1 after installing a linker." >&2
    exit 1
  fi
  # Cargo env var form: ARMV7_UNKNOWN_LINUX_GNUEABIHF / AARCH64_UNKNOWN_LINUX_GNU
  local cargo_key
  cargo_key="$(echo "$target" | tr 'a-z-' 'A-Z_')"
  export "CARGO_TARGET_${cargo_key}_LINKER=${gcc_prefix}-gcc"
  # cc-rs style
  local cc_key
  cc_key="$(echo "$target" | tr '-' '_')"
  export "CC_${cc_key}=${gcc_prefix}-gcc"
  export "CXX_${cc_key}=${gcc_prefix}-g++"
  export "AR_${cc_key}=${gcc_prefix}-ar"
  export PKG_CONFIG_ALLOW_CROSS="${PKG_CONFIG_ALLOW_CROSS:-1}"
  local pc_dir=""
  for candidate in \
      "/usr/lib/${gcc_prefix}/pkgconfig" \
      "/usr/lib/pkgconfig"; do
    if [[ -d "$candidate" ]]; then
      pc_dir="$candidate"
      break
    fi
  done
  if [[ -n "$pc_dir" ]]; then
    export PKG_CONFIG_LIBDIR="${pc_dir}"
    export PKG_CONFIG_PATH="${pc_dir}"
  fi
  unset PKG_CONFIG_SYSROOT_DIR || true
}

build_one_arch() {
  local arch="$1"
  local meta target stage multiarch gcc_prefix
  meta="$(arch_meta "$arch")"
  read -r target stage multiarch gcc_prefix <<<"$meta"
  local stage_dir="$ROOT/dist/$stage"

  log "=== arch=$arch target=$target stage=dist/$stage ==="
  ensure_apt_toolchain "$multiarch" "$gcc_prefix"
  ensure_rust_target "$target"
  export_cross_env "$target" "$gcc_prefix"
  mkdir -p "$stage_dir"

  local -a built=() failed=()
  local pkg
  for pkg in "${PKG_LIST[@]}"; do
    pkg="$(echo "$pkg" | tr -d '[:space:]')"
    [[ -z "$pkg" ]] && continue
    log "cargo build --release -p $pkg --target $target"
    if cargo build --release -p "$pkg" --target "$target"; then
      local target_dir src
      target_dir="${CARGO_TARGET_DIR:-$ROOT/target}"
      src="$target_dir/$target/release/$pkg"
      if [[ ! -f "$src" ]]; then
        echo "error: cargo reported success but $src is missing" >&2
        failed+=("$pkg")
        continue
      fi
      cp "$src" "$stage_dir/$pkg"
      chmod +x "$stage_dir/$pkg"
      if need_cmd "${gcc_prefix}-strip"; then
        "${gcc_prefix}-strip" "$stage_dir/$pkg" || true
      elif need_cmd strip; then
        strip "$stage_dir/$pkg" || true
      fi
      built+=("$pkg")
      log "staged $stage_dir/$pkg ($(wc -c < "$stage_dir/$pkg") bytes)"
    else
      failed+=("$pkg")
    fi
  done

  {
    echo "target=$target"
    echo "arch=$arch"
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "host=$(uname -s) $(uname -m)"
    local host_glibc
    host_glibc="$(ldd --version 2>&1 | awk 'NR==1 {
      if (match($0, /[0-9]+\.[0-9]+/)) { print substr($0, RSTART, RLENGTH); exit }
    }')"
    echo "host_glibc=${host_glibc:-unknown}"
    echo "git_sha=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
    echo "crates=$(IFS=','; echo "${built[*]}")"
  } > "$stage_dir/VERSION"

  if [[ ${#failed[@]} -gt 0 ]]; then
    echo "error: failed to build for $arch: ${failed[*]}" >&2
    if [[ -z "$ALLOW_PARTIAL" || ${#built[@]} -eq 0 ]]; then
      return 1
    fi
    log "ALLOW_PARTIAL=1 — staged ${built[*]} for $arch and left the rest alone"
  fi

  if [[ ${#built[@]} -eq 0 ]]; then
    echo "error: no binaries staged for $arch" >&2
    return 1
  fi

  log "done dist/$stage — commit so SET→UPDATE can install matching engines"
  return 0
}

IFS=',' read -r -a PKG_LIST <<< "$PACKAGES"

mapfile -t ARCH_LIST < <(resolve_arch_list)

overall_fail=0
for arch in "${ARCH_LIST[@]}"; do
  if ! build_one_arch "$arch"; then
    overall_fail=1
  fi
done

if [[ "$overall_fail" -ne 0 ]]; then
  exit 1
fi

log "all requested arches ok: ${ARCH_LIST[*]}"
