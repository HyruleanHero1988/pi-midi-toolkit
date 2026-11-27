#!/usr/bin/env bash
# Compatibility wrapper — use deploy/enable-hifiberry.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
exec bash "$HERE/enable-hifiberry.sh" "$@"
