#!/usr/bin/env bash
# Switch ADS7846 penirq / SPI params. Requires reboot.
# Usage: ./set-touch-overlay.sh [25|17|22|27]
set -euo pipefail
PENIRQ="${1:-17}"
CFG=/boot/firmware/config.txt
[[ -f "$CFG" ]] || CFG=/boot/config.txt

BEGIN='# --- midi-toolkit touch (ads7846) begin ---'
END='# --- midi-toolkit touch (ads7846) end ---'

sudo cp "$CFG" "${CFG}.bak-touch-$(date +%Y%m%d%H%M%S)"

sudo python3 - "$CFG" "$PENIRQ" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1])
penirq = sys.argv[2]
begin = "# --- midi-toolkit touch (ads7846) begin ---"
end = "# --- midi-toolkit touch (ads7846) end ---"
block = f"""{begin}
# Generic 5\" HDMI LCD on GPIO: video via mini-HDMI, touch via SPI (ADS7846).
dtparam=spi=on
dtoverlay=ads7846,cs=1,penirq={penirq},penirq_pull=2,speed=1000000,keep_vref_on=1,swapxy=0,pmax=255,xohms=150,xmin=200,xmax=3900,ymin=200,ymax=3900
{end}
"""
text = p.read_text(encoding="utf-8", errors="replace")
if begin in text and end in text:
    pre, rest = text.split(begin, 1)
    _, post = rest.split(end, 1)
    text = pre.rstrip() + "\n" + block + post.lstrip("\n")
else:
    text = text.rstrip() + "\n\n" + block + "\n"
p.write_text(text, encoding="utf-8")
print("updated", p, "penirq=", penirq)
PY

grep -A3 "midi-toolkit touch" "$CFG" | head -10
echo
echo "Reboot required: sudo reboot"
