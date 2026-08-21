#!/usr/bin/env bash
# Thin wrapper — real script lives in bin/
exec "$(cd "$(dirname "$0")" && pwd)/bin/kiosk.sh" "$@"
