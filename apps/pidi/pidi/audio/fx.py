"""Insert / bus FX (drive → delay → short tank)."""
from __future__ import annotations

import math
from typing import Any, Dict

import numpy as np

from pidi.constants import BLOCKSIZE, FX_DELAY_MAX_SEC, FX_REVERB_MAX_SEC

class MixBusFx:
    """
    Per-instrument FX insert (one instance per wavetable voice or drum model).
    drive → echo → short multi-tap tank. Params 0..1; process() mutates buf.
    Not a global master-bus effect — melody and drums keep separate slots.
    """

    def __init__(self, sample_rate: int) -> None:
        self.sample_rate = max(8000, int(sample_rate))
        self.drive = 0.0
        self.delay_time = 0.28  # mapped 50ms..750ms
        self.delay_fb = 0.35
        self.delay_mix = 0.0
        self.reverb_size = 0.45
        self.reverb_mix = 0.0
        dlen = max(64, int(self.sample_rate * FX_DELAY_MAX_SEC))
        rlen = max(64, int(self.sample_rate * FX_REVERB_MAX_SEC))
        self._delay = np.zeros(dlen, dtype=np.float32)
        self._dpos = 0
        self._reverb = np.zeros(rlen, dtype=np.float32)
        self._rpos = 0
        self._rev_lp = 0.0
        self._tmp = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._wet = np.zeros(BLOCKSIZE * 2, dtype=np.float32)
        self._idx = np.zeros(BLOCKSIZE * 2, dtype=np.int32)
        self._write_idx = np.zeros(BLOCKSIZE * 2, dtype=np.int32)
        self._arange_i = np.arange(BLOCKSIZE * 2, dtype=np.int32)

    def _ensure_work_bufs(self, n: int) -> None:
        if n > self._tmp.shape[0]:
            self._tmp = np.zeros(n, dtype=np.float32)
            self._wet = np.zeros(n, dtype=np.float32)
            self._idx = np.zeros(n, dtype=np.int32)
            self._write_idx = np.zeros(n, dtype=np.int32)
        if n > self._arange_i.shape[0]:
            self._arange_i = np.arange(n, dtype=np.int32)

    def snapshot(self) -> Dict[str, float]:
        return {
            "fx_drive": float(self.drive),
            "fx_delay_time": float(self.delay_time),
            "fx_delay_fb": float(self.delay_fb),
            "fx_delay_mix": float(self.delay_mix),
            "fx_reverb_size": float(self.reverb_size),
            "fx_reverb_mix": float(self.reverb_mix),
        }

    def apply_snapshot(self, data: Dict[str, Any]) -> None:
        def _f(key: str, cur: float) -> float:
            if key not in data:
                return cur
            try:
                return max(0.0, min(1.0, float(data[key])))
            except (TypeError, ValueError):
                return cur

        self.drive = _f("fx_drive", self.drive)
        self.delay_time = _f("fx_delay_time", self.delay_time)
        self.delay_fb = _f("fx_delay_fb", self.delay_fb)
        self.delay_mix = _f("fx_delay_mix", self.delay_mix)
        self.reverb_size = _f("fx_reverb_size", self.reverb_size)
        self.reverb_mix = _f("fx_reverb_mix", self.reverb_mix)

    def is_dry(self) -> bool:
        """True when process() would be a no-op (skip in the audio callback)."""
        return (
            float(self.drive) <= 0.001
            and float(self.delay_mix) <= 0.001
            and float(self.delay_fb) <= 0.001
            and float(self.reverb_mix) <= 0.001
        )

    def reset_to_defaults(self) -> None:
        """Hard reset params + clear delay/reverb memory (kills leftover echo)."""
        self.drive = 0.0
        self.delay_time = 0.28
        self.delay_fb = 0.35
        self.delay_mix = 0.0
        self.reverb_size = 0.45
        self.reverb_mix = 0.0
        self._delay.fill(0.0)
        self._reverb.fill(0.0)
        self._dpos = 0
        self._rpos = 0
        self._rev_lp = 0.0

    def process(self, buf: np.ndarray) -> None:
        n = len(buf)
        if n == 0:
            return
        self._ensure_work_bufs(n)
        arange_i = self._arange_i[:n]
        # --- Drive (waveshape) ---
        drive = float(self.drive)
        if drive > 0.001:
            # 0 → gentle; 1 → hot tanh
            amount = 1.0 + drive * 12.0
            np.multiply(buf, np.float32(amount), out=self._tmp[:n])
            np.tanh(self._tmp[:n], out=self._tmp[:n])
            # Compensate so drive=0-ish loudness stays similar at low drive
            norm = math.tanh(amount) if amount > 1e-6 else 1.0
            np.multiply(self._tmp[:n], np.float32(1.0 / max(0.25, norm)), out=buf)

        # --- Echo / delay ---
        dmix = float(self.delay_mix)
        if dmix > 0.001 or float(self.delay_fb) > 0.001:
            dbuf = self._delay
            dlen = len(dbuf)
            # 50ms .. 750ms
            delay_sec = 0.05 + max(0.0, min(1.0, float(self.delay_time))) * 0.70
            ds = max(1, min(dlen - 1, int(delay_sec * self.sample_rate)))
            fb = max(0.0, min(0.92, float(self.delay_fb)))
            pos = self._dpos
            wet = self._wet[:n]
            idx = self._idx[:n]
            write_idx = self._write_idx[:n]
            np.add(np.int32(pos - ds), arange_i, out=idx)
            np.remainder(idx, dlen, out=idx)
            np.take(dbuf, idx, out=wet)
            np.add(np.int32(pos), arange_i, out=write_idx)
            np.remainder(write_idx, dlen, out=write_idx)
            dbuf[write_idx] = buf + wet * np.float32(fb)
            self._dpos = (pos + n) % dlen
            if dmix > 0.001:
                dry = 1.0 - dmix
                buf *= np.float32(dry)
                buf += wet * np.float32(dmix)

        # --- Short recirculating multi-tap tank (reverb-ish) ---
        rmix = float(self.reverb_mix)
        if rmix > 0.001:
            rbuf = self._reverb
            rlen = len(rbuf)
            size = max(0.0, min(1.0, float(self.reverb_size)))
            # Tap spacings scale with size
            base = int((0.018 + 0.040 * size) * self.sample_rate)
            taps = (
                max(1, base),
                max(1, int(base * 1.7)),
                max(1, int(base * 2.5)),
                max(1, int(base * 3.4)),
            )
            gains = (0.55, 0.40, 0.30, 0.22)
            pos = self._rpos
            wet = self._wet[:n]
            idx = self._idx[:n]
            wet.fill(0.0)
            for tap, g in zip(taps, gains):
                tap = min(tap, rlen - 1)
                np.add(np.int32(pos - tap), arange_i, out=idx)
                np.remainder(idx, rlen, out=idx)
                wet += np.take(rbuf, idx) * np.float32(g)
            # Soften highs with a cheap 2-tap blur (size → darker)
            if size > 0.05:
                soft = self._tmp[:n]
                soft[0] = wet[0]
                soft[1:] = 0.5 * (wet[1:] + wet[:-1])
                blend = np.float32(max(0.0, min(1.0, size)))
                wet *= np.float32(1.0 - blend)
                wet += soft * blend
            fb = 0.25 + 0.45 * size
            write_idx = self._write_idx[:n]
            np.add(np.int32(pos), arange_i, out=write_idx)
            np.remainder(write_idx, rlen, out=write_idx)
            rbuf[write_idx] = buf * np.float32(0.7) + wet * np.float32(fb)
            self._rpos = (pos + n) % rlen
            dry = 1.0 - rmix
            buf *= np.float32(dry)
            buf += wet * np.float32(rmix)


