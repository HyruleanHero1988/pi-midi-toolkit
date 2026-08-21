"""Unit tests for diagnostics sampler (no Pi required)."""
from __future__ import annotations

import unittest

from pidi.diagnostics import DiagSample, DiagnosticsSampler


class DiagSampleTest(unittest.TestCase):
    def test_format_line_includes_core_fields(self) -> None:
        s = DiagSample(
            cpu_pct=42.0,
            load1=1.25,
            mem_used_mb=180.0,
            mem_pct=55.0,
            temp_c=51.2,
            rss_mb=90.0,
            swap_used_mb=0.0,
        )
        line = s.format_line()
        self.assertIn("CPU", line)
        self.assertIn("LD", line)
        self.assertIn("RAM", line)
        self.assertIn("APP", line)
        self.assertIn("T", line)
        self.assertNotIn("SWP", line)

    def test_format_line_shows_swap_when_used(self) -> None:
        s = DiagSample(swap_used_mb=12.0, cpu_pct=10.0)
        self.assertIn("SWP", s.format_line())

    def test_sampler_returns_sample(self) -> None:
        sampler = DiagnosticsSampler()
        a = sampler.sample()
        b = sampler.sample()
        self.assertIsInstance(a, DiagSample)
        self.assertIsInstance(b, DiagSample)
        # Second sample may gain CPU% once /proc/stat exists.
        line = b.format_line()
        self.assertTrue(isinstance(line, str) and len(line) > 0)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
