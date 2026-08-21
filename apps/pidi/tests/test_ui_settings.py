#!/usr/bin/env python3
"""Headless smoke test for the SET (settings / update) screen."""

from __future__ import annotations

import pathlib
import sys
import tempfile
import time
import unittest
from unittest import mock

import test_ui_seq as seq_harness


@unittest.skipUnless(seq_harness.display_available(), "no X display (try xvfb-run)")
class SettingsScreenTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        import sounddevice as sd
        import mido

        cls._tmp = tempfile.TemporaryDirectory()
        tmp = pathlib.Path(cls._tmp.name)

        sd.OutputStream = seq_harness.FakeStream  # type: ignore[assignment]
        sd.query_devices = lambda *a, **k: (  # type: ignore[assignment]
            {"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}
            if a
            else [{"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}]
        )
        mido.get_input_names = lambda: ["fake MPK"]  # type: ignore[assignment]
        mido.get_output_names = lambda: []  # type: ignore[assignment]
        mido.open_input = lambda name: seq_harness.FakePort(name)  # type: ignore[assignment]
        mido.open_output = lambda name: seq_harness.FakePort(name)  # type: ignore[assignment]

        import midi_tone
        from pidi import updater

        midi_tone.SETTINGS_PATH = tmp / "settings.json"
        midi_tone.PRESETS_DIR = tmp / "presets"
        midi_tone.SONGS_DIR = tmp / "songs"
        midi_tone.PHRASES_DIR = tmp / "phrases"
        midi_tone.DEMO_SONGS_DIR = tmp / "demo-songs"
        midi_tone.SONG_SEED_MARKER = tmp / "songs" / ".seeded"
        midi_tone.USER_WAVETABLES_DIR = tmp / "user-wavetables"
        cls.midi_tone = midi_tone
        cls.updater = updater
        cls.app = midi_tone.MidiToneApp(
            port_filter="",
            list_only=False,
            max_voices=4,
            waves_dir=midi_tone.DEFAULT_WAVETABLE_DIR,
        )

    @classmethod
    def tearDownClass(cls) -> None:
        try:
            cls.app._stop.set()
            cls.app._seq.stop()
            cls.app.engine.stop()
            cls.app.root.destroy()
        except Exception:
            pass
        cls._tmp.cleanup()

    def pump(self, seconds: float = 0.08) -> None:
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.app.root.update()
            time.sleep(0.005)

    def test_settings_mode_has_check_and_update(self) -> None:
        app = self.app
        self.assertIn("home", app._mode_btns)
        app._switch_mode("home")
        self.pump()
        self.assertEqual(app._mode, "home")
        self.assertTrue(app._home_shell.winfo_ismapped())
        self.assertFalse(app._jam_btns["synth"].winfo_ismapped())

        app._switch_mode("settings")
        self.pump()
        self.assertEqual(app._mode, "settings")
        self.assertIsNotNone(app._settings_check_btn)
        self.assertIsNotNone(app._settings_update_btn)
        self.assertIn("Running:", app._settings_status_var.get())
        self.assertTrue(app._settings_shell.winfo_ismapped())
        self.assertNotIn("songs", app._mode_btns)
        self.assertNotIn("settings", app._mode_btns)

    def test_jam_shortcuts_only_on_synth_seq_pads_kaoss(self) -> None:
        app = self.app
        app._switch_mode("synth")
        self.pump()
        for key in ("synth", "seq", "pads", "kaoss"):
            self.assertTrue(app._jam_btns[key].winfo_ismapped(), key)
        app._switch_mode("songs")
        self.pump()
        for key in ("synth", "seq", "pads", "kaoss"):
            self.assertFalse(app._jam_btns[key].winfo_ismapped(), key)
        app._switch_mode("pads")
        self.pump()
        self.assertTrue(app._jam_btns["pads"].winfo_ismapped())
        self.assertEqual(app._jam_btns["pads"].cget("bg"), "#458588")

    def test_check_posts_update_available_without_installing(self) -> None:
        app = self.app
        app._switch_mode("settings")
        fake = self.updater.UpdateCheck(
            local=self.updater.VersionInfo(sha="aaa1111", branch="master", source="file"),
            remote=self.updater.VersionInfo(sha="bbb2222", branch="master", source="remote"),
            available=True,
            message="Update available: aaa1111 → bbb2222 (master)",
        )
        with mock.patch.object(self.updater, "check_for_update", return_value=fake), mock.patch.object(
            self.updater, "apply_update", side_effect=AssertionError("must not install on CHECK")
        ):
            app._settings_check()
            # Worker thread + Tk queue
            deadline = time.monotonic() + 2.0
            while app._update_busy and time.monotonic() < deadline:
                self.pump(0.05)
            self.pump(0.1)
        self.assertFalse(app._update_busy)
        self.assertIsNotNone(app._update_check)
        self.assertTrue(app._update_check.available)
        self.assertIn("bbb2222", app._settings_status_var.get())
        self.assertEqual(app._settings_update_btn.cget("text"), "UPDATE")

        # First UPDATE tap arms confirm; does not apply yet.
        with mock.patch.object(
            self.updater, "apply_update", side_effect=AssertionError("must not install on confirm tap")
        ):
            app._settings_update()
            self.pump(0.1)
        self.assertTrue(app._update_confirming)
        self.assertEqual(app._settings_update_btn.cget("text"), "INSTALL NOW")
        self.assertEqual(app._settings_check_btn.cget("text"), "CANCEL")

        app._settings_check()  # CANCEL
        self.pump(0.05)
        self.assertFalse(app._update_confirming)
        self.assertEqual(app._settings_update_btn.cget("text"), "UPDATE")

    def test_diagnostics_strip_stays_visible_across_modes(self) -> None:
        """DIAG is global chrome, not a Home/Settings-only overlay."""
        app = self.app
        app.root.geometry("800x480")
        app.root.update_idletasks()
        app._set_diagnostics(True)
        self.pump()
        self.assertTrue(app._diagnostics_on)
        self.assertTrue(app._diag_bar.winfo_ismapped())
        modes = (
            "home",
            "synth",
            "seq",
            "pads",
            "kaoss",
            "songs",
            "presets",
            "log",
            "settings",
        )
        for mode in modes:
            app._switch_mode(mode)
            self.pump()
            app.root.update_idletasks()
            self.assertTrue(app._diag_bar.winfo_ismapped(), mode)
            self.assertGreaterEqual(int(app._diag_bar.winfo_height()), 20, mode)
            bar_top = int(app._diag_bar.winfo_rooty())
            host_bottom = int(app._mode_host.winfo_rooty()) + int(
                app._mode_host.winfo_height()
            )
            self.assertLessEqual(host_bottom, bar_top + 2, mode)
        app._set_diagnostics(False)
        self.pump()
        self.assertFalse(app._diag_bar.winfo_ismapped())


if __name__ == "__main__":
    sys.exit(unittest.main())
