#!/usr/bin/env python3
"""Idle blanking / burn-in helpers — no Tk, no audio."""

from __future__ import annotations

import pathlib
import sys
import unittest


HERE = pathlib.Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import screensaver  # noqa: E402


class TimeoutHelpersTest(unittest.TestCase):
    def test_env_override_and_default(self) -> None:
        self.assertEqual(screensaver.timeout_from_env({}), screensaver.DEFAULT_TIMEOUT_SEC)
        self.assertEqual(screensaver.timeout_from_env({"MIDI_TONE_SCREENSAVER_SEC": "45"}), 45.0)
        self.assertEqual(screensaver.timeout_from_env({"MIDI_TONE_SCREENSAVER_SEC": "0"}), 0.0)
        self.assertEqual(
            screensaver.timeout_from_env({"MIDI_TONE_SCREENSAVER_SEC": "nope"}),
            screensaver.DEFAULT_TIMEOUT_SEC,
        )

    def test_preset_cycle_and_label(self) -> None:
        self.assertEqual(screensaver.next_timeout_preset(180.0), 600.0)
        self.assertEqual(screensaver.next_timeout_preset(600.0), 0.0)
        self.assertEqual(screensaver.next_timeout_preset(0.0), 60.0)
        self.assertEqual(screensaver.next_timeout_preset(99.0), 60.0)
        self.assertEqual(screensaver.timeout_label(0), "BLANK OFF")
        self.assertEqual(screensaver.timeout_label(60), "BLANK 1 MIN")
        self.assertEqual(screensaver.timeout_label(180), "BLANK 3 MIN")
        self.assertEqual(screensaver.timeout_label(45), "BLANK 45s")


class IdleWatchTest(unittest.TestCase):
    def test_due_after_timeout_then_poke_resets(self) -> None:
        watch = screensaver.IdleWatch(timeout_sec=10.0, now=100.0)
        self.assertFalse(watch.due(now=109.0))
        self.assertTrue(watch.due(now=110.0))
        self.assertFalse(watch.poke(now=111.0))
        self.assertFalse(watch.due(now=120.0))
        self.assertTrue(watch.due(now=121.0))

    def test_disabled_timeout_never_fires(self) -> None:
        watch = screensaver.IdleWatch(timeout_sec=0.0, now=0.0)
        self.assertFalse(watch.due(now=9999.0))

    def test_activate_and_poke_dismisses(self) -> None:
        watch = screensaver.IdleWatch(timeout_sec=5.0, now=0.0)
        self.assertTrue(watch.activate())
        self.assertFalse(watch.activate())
        self.assertFalse(watch.due(now=100.0), "already showing is not 'due'")
        self.assertTrue(watch.poke(now=101.0), "poke while active dismisses")
        self.assertFalse(watch.active)
        self.assertFalse(watch.due(now=105.0))


class OrbitTest(unittest.TestCase):
    def test_stays_inside_the_panel(self) -> None:
        seen = set()
        for t in range(0, 200, 3):
            x, y = screensaver.orbit_xy(float(t), 800, 480, 220, 40, margin=16)
            self.assertGreaterEqual(x, 16)
            self.assertGreaterEqual(y, 16)
            self.assertLessEqual(x, 800 - 220 - 16)
            self.assertLessEqual(y, 480 - 40 - 16)
            seen.add((x, y))
        self.assertGreater(len(seen), 20, "hint should wander, not park")

    def test_tiny_panel_does_not_go_negative(self) -> None:
        x, y = screensaver.orbit_xy(12.0, 100, 50, 200, 80, margin=16)
        self.assertGreaterEqual(x, 16)
        self.assertGreaterEqual(y, 16)


class PixelShiftTest(unittest.TestCase):
    def test_dwells_then_moves_by_amplitude(self) -> None:
        self.assertEqual(screensaver.pixel_shift_xy(0.0, amplitude=2, dwell_sec=10.0), (0, 0))
        self.assertEqual(screensaver.pixel_shift_xy(9.9, amplitude=2, dwell_sec=10.0), (0, 0))
        self.assertEqual(screensaver.pixel_shift_xy(10.0, amplitude=2, dwell_sec=10.0), (2, 0))
        self.assertEqual(screensaver.pixel_shift_xy(20.0, amplitude=2, dwell_sec=10.0), (2, 2))

    def test_visits_every_corner(self) -> None:
        seen = {
            screensaver.pixel_shift_xy(float(t), amplitude=1, dwell_sec=1.0)
            for t in range(8)
        }
        self.assertEqual(len(seen), 8)
        self.assertIn((0, 0), seen)
        self.assertIn((1, 1), seen)
        self.assertIn((-1, -1), seen)


class PanelBacklightTest(unittest.TestCase):
    def test_dim_and_restore_sysfs(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            device = root / "10-0045"
            device.mkdir()
            brightness = device / "brightness"
            power = device / "bl_power"
            brightness.write_text("128\n", encoding="ascii")
            power.write_text("0\n", encoding="ascii")
            (device / "max_brightness").write_text("255\n", encoding="ascii")

            panel = screensaver.PanelBacklight(root)
            self.assertTrue(panel.dim())
            self.assertEqual(brightness.read_text(encoding="ascii").strip(), "0")
            # Must NOT power the panel down — that kills capacitive wake taps.
            self.assertEqual(power.read_text(encoding="ascii").strip(), "0")
            self.assertTrue(panel.restore())
            self.assertEqual(brightness.read_text(encoding="ascii").strip(), "128")
            self.assertEqual(power.read_text(encoding="ascii").strip(), "0")

    def test_missing_sysfs_is_a_no_op(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as raw:
            panel = screensaver.PanelBacklight(pathlib.Path(raw))
            self.assertFalse(panel.dim())
            self.assertFalse(panel.restore())


if __name__ == "__main__":
    sys.exit(unittest.main())
