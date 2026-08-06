#!/usr/bin/env bash
# Enable SPI + ADS7846 touch for generic 5" HDMI LCDs that mount on the Pi GPIO
# (video via mini-HDMI, power+touch via the 40-pin header).
set -euo pipefail

CFG=/boot/firmware/config.txt
if [[ ! -f "$CFG" ]]; then CFG=/boot/config.txt; fi

BEGIN='# --- midi-toolkit touch (ads7846) begin ---'
END='# --- midi-toolkit touch (ads7846) end ---'

sudo cp "$CFG" "${CFG}.bak-before-touch"

sudo python3 - "$CFG" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1])
begin = "# --- midi-toolkit touch (ads7846) begin ---"
end = "# --- midi-toolkit touch (ads7846) end ---"
block = f"""{begin}
# Generic 5\" HDMI LCD on GPIO: video via mini-HDMI, touch via SPI (ADS7846).
dtparam=spi=on
dtoverlay=ads7846,cs=1,penirq=25,penirq_pull=2,speed=50000,keep_vref_on=0,swapxy=0,pmax=255,xohms=150,xmin=200,xmax=3900,ymin=200,ymax=3900
{end}
"""
text = p.read_text(encoding="utf-8", errors="replace")
if begin in text and end in text:
    pre, rest = text.split(begin, 1)
    _, post = rest.split(end, 1)
    text = pre.rstrip() + "\n" + block + post.lstrip("\n")
elif "[all]" in text:
    a, b = text.split("[all]", 1)
    text = a + "[all]\n" + block + b.lstrip("\n")
else:
    text = text.rstrip() + "\n\n" + block + "\n"
p.write_text(text, encoding="utf-8")
print("updated", p)
PY

echo "Touch overlay installed. Reboot with: sudo reboot"
echo "After reboot you should see 'ADS7846 Touchscreen' in: cat /proc/bus/input/devices"
echo "If axes are swapped/inverted, edit the dtoverlay line (swapxy=1) and reboot again."
