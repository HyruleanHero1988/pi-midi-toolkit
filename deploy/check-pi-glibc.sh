#!/usr/bin/env bash
# Ensure a Bookworm-safe host before cross-linking armv7 bins.
# Pi Bookworm = glibc 2.36. Ubuntu 22.04 = 2.35 (OK). Ubuntu 24.04+/Debian 13 = 2.38+ (BAD).
#
# Env:
#   FORCE_NEW_GLIBC=1  allow building anyway (bins likely will not run on the Pi)
#   PI_BINS_OK_GLIBC=1 set by Docker/WSL wrappers that already verified the host

set -euo pipefail

max_ok_major=2
max_ok_minor=36

host_glibc() {
  if command -v ldd >/dev/null 2>&1; then
    ldd --version 2>&1 | head -n1 | grep -oE '[0-9]+\.[0-9]+' | head -n1
    return 0
  fi
  echo "0.0"
}

check_host_glibc_for_pi() {
  if [[ -n "${FORCE_NEW_GLIBC:-}" || -n "${PI_BINS_OK_GLIBC:-}" ]]; then
    return 0
  fi
  local ver major minor
  ver="$(host_glibc)"
  major="${ver%%.*}"
  minor="${ver#*.}"
  minor="${minor%%.*}"
  if [[ -z "$major" || -z "$minor" ]]; then
    echo "build-pi-bins: warning: could not parse host glibc ($ver); continuing" >&2
    return 0
  fi
  if (( major > max_ok_major )) || { (( major == max_ok_major )) && (( minor > max_ok_minor )); }; then
    cat >&2 <<EOF
error: host glibc $ver is newer than Pi Bookworm (2.36).
Cross-linked bins from this host will require a newer GLIBC than Bookworm and fail on the Pi.

Fix (pick one):
  • Windows:  .\\deploy\\build-pi-bins.ps1
              (uses Ubuntu-22.04 WSL or Docker — same floor as CI)
  • WSL:      wsl --install -d Ubuntu-22.04
              then run ./deploy/build-pi-bins.sh inside that distro
  • Docker:   docker build -f deploy/Dockerfile.pi-bins -t pi-bins .
              docker run --rm -v \"\$PWD\":/src -w /src pi-bins

Override only if you know what you are doing: FORCE_NEW_GLIBC=1
EOF
    exit 1
  fi
  echo "build-pi-bins: host glibc $ver OK for Pi Bookworm (≤2.36)"
}

check_host_glibc_for_pi
