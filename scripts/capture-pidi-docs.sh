#!/usr/bin/env bash
# Recapture native PiDI 800×480 screenshots into docs/screens and the
# raygarrison.us copy under apps/pidi/docs/screens.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

OUT="$ROOT/docs/screens"
mkdir -p "$OUT"

cargo run -p pidi-native --example dump_docs --no-default-features -- "$OUT"
python3 "$ROOT/scripts/ppm_to_png.py" "$OUT"

SITE="$ROOT/apps/pidi/docs"
mkdir -p "$SITE/screens"
cp -f "$ROOT/docs/index.html" "$SITE/index.html"
rm -rf "$SITE/screens"
mkdir -p "$SITE/screens"
cp -f "$OUT"/*.png "$SITE/screens/"

echo "captured $(ls -1 "$OUT"/*.png | wc -l) screens → $OUT and $SITE/screens"
