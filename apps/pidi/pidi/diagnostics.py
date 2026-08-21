"""Lightweight system diagnostics for the PiDI kiosk toolbar.

Reads /proc (and thermal sysfs) so sampling stays cheap enough for a 1 Hz UI tick.
Falls back gracefully on non-Linux hosts.
"""
from __future__ import annotations

import os
import pathlib
import time
from dataclasses import dataclass
from typing import Optional, Tuple


@dataclass
class DiagSample:
    cpu_pct: Optional[float] = None
    load1: Optional[float] = None
    mem_used_mb: Optional[float] = None
    mem_total_mb: Optional[float] = None
    mem_pct: Optional[float] = None
    temp_c: Optional[float] = None
    rss_mb: Optional[float] = None
    swap_used_mb: Optional[float] = None

    def format_line(self) -> str:
        parts = []
        if self.cpu_pct is not None:
            parts.append(f"CPU {self.cpu_pct:4.0f}%")
        if self.load1 is not None:
            parts.append(f"LD {self.load1:4.2f}")
        if self.mem_pct is not None and self.mem_used_mb is not None:
            parts.append(f"RAM {self.mem_used_mb:4.0f}M {self.mem_pct:3.0f}%")
        elif self.mem_used_mb is not None:
            parts.append(f"RAM {self.mem_used_mb:4.0f}M")
        if self.swap_used_mb is not None and self.swap_used_mb >= 1.0:
            parts.append(f"SWP {self.swap_used_mb:3.0f}M")
        if self.rss_mb is not None:
            parts.append(f"APP {self.rss_mb:4.0f}M")
        if self.temp_c is not None:
            parts.append(f"T {self.temp_c:4.1f}C")
        return "  ".join(parts) if parts else "diagnostics unavailable"


class DiagnosticsSampler:
    """Stateful CPU% calculator (needs two /proc/stat snapshots)."""

    def __init__(self) -> None:
        self._prev_idle: Optional[int] = None
        self._prev_total: Optional[int] = None
        self._prev_ts: float = 0.0

    def sample(self) -> DiagSample:
        out = DiagSample()
        out.cpu_pct = self._cpu_pct()
        out.load1 = self._load1()
        mem = self._mem()
        if mem is not None:
            used, total, pct, swap = mem
            out.mem_used_mb = used
            out.mem_total_mb = total
            out.mem_pct = pct
            out.swap_used_mb = swap
        out.temp_c = self._temp_c()
        out.rss_mb = self._rss_mb()
        return out

    def _read_cpu_times(self) -> Optional[Tuple[int, int]]:
        try:
            line = pathlib.Path("/proc/stat").read_text(encoding="ascii").splitlines()[0]
        except OSError:
            return None
        parts = line.split()
        if not parts or parts[0] != "cpu" or len(parts) < 5:
            return None
        vals = [int(x) for x in parts[1:]]
        idle = vals[3] + (vals[4] if len(vals) > 4 else 0)  # idle + iowait
        total = sum(vals)
        return idle, total

    def _cpu_pct(self) -> Optional[float]:
        cur = self._read_cpu_times()
        now = time.monotonic()
        if cur is None:
            return None
        idle, total = cur
        pct: Optional[float] = None
        if self._prev_idle is not None and self._prev_total is not None:
            didle = idle - self._prev_idle
            dtotal = total - self._prev_total
            if dtotal > 0:
                pct = max(0.0, min(100.0, 100.0 * (1.0 - (didle / dtotal))))
        self._prev_idle = idle
        self._prev_total = total
        self._prev_ts = now
        return pct

    def _load1(self) -> Optional[float]:
        try:
            return float(os.getloadavg()[0])
        except (AttributeError, OSError):
            return None

    def _mem(self) -> Optional[Tuple[float, float, float, float]]:
        try:
            text = pathlib.Path("/proc/meminfo").read_text(encoding="ascii")
        except OSError:
            return None
        kv = {}
        for line in text.splitlines():
            if ":" not in line:
                continue
            k, rest = line.split(":", 1)
            num = rest.strip().split()[0]
            try:
                kv[k] = int(num)  # kB
            except ValueError:
                continue
        total = kv.get("MemTotal")
        avail = kv.get("MemAvailable")
        if total is None:
            return None
        if avail is None:
            free = kv.get("MemFree", 0)
            buff = kv.get("Buffers", 0)
            cached = kv.get("Cached", 0)
            avail = free + buff + cached
        used = max(0, total - avail)
        pct = 100.0 * used / total if total else 0.0
        swap_total = kv.get("SwapTotal", 0)
        swap_free = kv.get("SwapFree", 0)
        swap_used = max(0, swap_total - swap_free) if swap_total else 0
        return used / 1024.0, total / 1024.0, pct, swap_used / 1024.0

    def _temp_c(self) -> Optional[float]:
        # Raspberry Pi thermal zone; try common paths.
        candidates = (
            pathlib.Path("/sys/class/thermal/thermal_zone0/temp"),
            pathlib.Path("/sys/devices/virtual/thermal/thermal_zone0/temp"),
        )
        for path in candidates:
            try:
                raw = int(path.read_text(encoding="ascii").strip())
            except (OSError, ValueError):
                continue
            # Millidegrees on Pi; some boards use whole degrees.
            return raw / 1000.0 if raw > 200 else float(raw)
        return None

    def _rss_mb(self) -> Optional[float]:
        try:
            # VmRSS in kB
            for line in pathlib.Path("/proc/self/status").read_text(encoding="ascii").splitlines():
                if line.startswith("VmRSS:"):
                    kb = int(line.split()[1])
                    return kb / 1024.0
        except (OSError, ValueError, IndexError):
            pass
        return None
