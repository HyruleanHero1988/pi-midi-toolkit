#!/usr/bin/env bash
# One-time bootstrap for Ubuntu-22.04 WSL Pi builds (run as root).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PI_BINS_OK_GLIBC=1

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
fi
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
rustup target add armv7-unknown-linux-gnueabihf

# C toolchain for the host (rustc needs cc for build scripts)
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends build-essential curl ca-certificates pkg-config python3

dpkg --add-architecture armhf
bash "$ROOT/deploy/ubuntu-armhf-ports.sh"
apt-get update -qq
apt-get install -y --no-install-recommends \
  gcc-arm-linux-gnueabihf g++-arm-linux-gnueabihf pkg-config \
  libasound2-dev:armhf libsdl2-dev:armhf libgles2-mesa-dev:armhf \
  libegl1-mesa-dev:armhf libgbm-dev:armhf libdrm-dev:armhf

arm-linux-gnueabihf-gcc --version | head -1
ldd --version | head -1
echo "bootstrap-pi-bins: OK"
