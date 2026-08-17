#!/usr/bin/env python3
"""
Headless smoke test for the KAOSS screen.

Builds the real Tk app with stub audio/MIDI, then drives the XY pad the way
a finger does: press, slide, hold, lift. Catches missing widgets and wiring
mistakes the model tests can't see.

Needs an X display; skipped automatically when there isn't one
(`xvfb-run -a python3 -m unittest test_ui_kaoss`).
"""

from __future__ import annotations

import pathlib
import sys
import tempfile
import time
import unittest


class FakeStream:
    def __init__(self, **kwargs) -> None:
        self.kwargs = kwargs
        self.callback = kwargs.get("callback")
        self.samplerate = kwargs.get("samplerate", 44100)
        self.channels = kwargs.get("channels", 1)
        self.blocksize = kwargs.get("blocksize", 1024)
        self.latency = 0.05
        self.active = False

    def start(self) -> None:
        self.active = True

    def stop(self) -> None:
        self.active = False

    def close(self) -> None:
        self.active = False


class FakePort:
    def __init__(self, name: str = "fake MPK") -> None:
        self.name = name
        self.sent: list = []

    def iter_pending(self):
        return iter(())

    def send(self, msg) -> None:
        self.sent.append(msg)

    def close(self) -> None:
        pass


def display_available() -> bool:
    import os

    if not os.environ.get("DISPLAY"):
        return False
    try:
        import tkinter

        root = tkinter.Tk()
        root.destroy()
        return True
    except Exception:
        return False


@unittest.skipUnless(display_available(), "no X display (try xvfb-run)")
class KaossScreenTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        import sounddevice as sd
        import mido

        cls._tmp = tempfile.TemporaryDirectory()
        tmp = pathlib.Path(cls._tmp.name)

        sd.OutputStream = FakeStream  # type: ignore[assignment]
        sd.query_devices = lambda *a, **k: (  # type: ignore[assignment]
            {"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}
            if a
            else [{"name": "fake", "max_output_channels": 2, "default_samplerate": 44100.0}]
        )
        mido.get_input_names = lambda: ["fake MPK"]  # type: ignore[assignment]
        mido.get_output_names = lambda: ["fake DIN"]  # type: ignore[assignment]
        mido.open_input = lambda name: FakePort(name)  # type: ignore[assignment]
        mido.open_output = lambda name: FakePort(name)  # type: ignore[assignment]

        import midi_tone

        midi_tone.SETTINGS_PATH = tmp / "settings.json"
        midi_tone.PRESETS_DIR = tmp / "presets"
        midi_tone.SONGS_DIR = tmp / "songs"
        midi_tone.PHRASES_DIR = tmp / "phrases"
        midi_tone.DEMO_SONGS_DIR = tmp / "demo-songs"
        midi_tone.SONG_SEED_MARKER = tmp / "songs" / ".seeded"
        midi_tone.USER_WAVETABLES_DIR = tmp / "user-wavetables"
        cls.midi_tone = midi_tone
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
            cls.app._kaoss_cancel_tick()
            cls.app._kaoss_cancel_viz()
            cls.app._close_kaoss_scale_grid(restore_main=False)
            cls.app._seq.stop()
            cls.app.engine.stop()
            cls.app.root.destroy()
        except Exception:
            pass
        cls._tmp.cleanup()

    def setUp(self) -> None:
        app = self.app
        app._kaoss.panic()
        app._kaoss.out_mode = "local"
        app._kaoss.hold = False
        app._kaoss.program_id = "lead"
        app._kaoss.gate_id = "off"
        app._kaoss.scale_id = "ionian"
        if app._kaoss_scale_open:
            app._close_kaoss_scale_grid(restore_main=False)
        app.engine.all_notes_off()

    def pump(self, seconds: float = 0.05) -> None:
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.app.root.update()
            time.sleep(0.005)

    def test_kaoss_tab_and_pad_plays_local_note(self) -> None:
        app = self.app
        self.assertIn("kaoss", app._mode_btns)
        app._switch_mode("kaoss")
        self.pump()
        self.assertEqual(app._mode, "kaoss")
        self.assertIsNotNone(app._kaoss_canvas)
        self.assertIn("LEAD", app._kaoss_status_var.get())

        class Ev:
            def __init__(self, x: int, y: int, state: int = 0x0100) -> None:
                self.x = x
                self.y = y
                self.state = state

        canvas = app._kaoss_canvas
        canvas.update_idletasks()
        w = max(40, int(canvas.winfo_width()))
        h = max(40, int(canvas.winfo_height()))
        # Bottom-left of the pad = root of the scale (C3 in factory settings)
        app._kaoss_on_press(Ev(2, h - 2))
        app._drain_queue()
        self.pump()
        note = app._kaoss.sounding_note()
        self.assertIsNotNone(note)
        self.assertIn((app._kaoss.channel, note), app.engine._voices)

        app._kaoss_on_move(Ev(w - 2, 2))
        self.pump()
        self.assertGreater(app._kaoss.sounding_note() or 0, 48)

        app._kaoss_on_release()
        self.pump()
        self.assertIsNone(app._kaoss.sounding_note())

    def test_hold_and_usb_out_send_factory_ccs(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._kaoss.out_mode = "usb"
        app._songs.ensure_outport()
        port = app._songs.outport()
        self.assertIsNotNone(port)
        port.sent.clear()

        class Ev:
            def __init__(self, x: int, y: int) -> None:
                self.x = x
                self.y = y
                self.state = 0x0100

        canvas = app._kaoss_canvas
        canvas.update_idletasks()
        app._kaoss_on_press(Ev(4, 4))
        app._kaoss.set_hold(True)
        app._kaoss_on_release()
        self.pump()
        self.assertTrue(app._kaoss.is_active())
        kinds = [m.type for m in port.sent]
        self.assertIn("note_on", kinds)
        self.assertIn("control_change", kinds)
        ccs = {m.control: m.value for m in port.sent if m.type == "control_change"}
        self.assertIn(12, ccs)
        self.assertIn(13, ccs)
        self.assertEqual(ccs.get(92), 127)

        app._panic()
        self.pump()
        self.assertFalse(app._kaoss.is_active())

    def test_show_all_unlocks_factory_scales(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        curated = len(app._kaoss.scale_ids())
        app._kaoss_toggle_show_all()
        self.pump()
        self.assertTrue(app._kaoss.show_all)
        self.assertGreater(len(app._kaoss.scale_ids()), curated)
        self.assertIn("ALL: ON", app._kaoss_all_btn.cget("text"))
        app._switch_mode("presets")
        self.pump()
        self.assertIn("KAOSS: ALL", app._preset_kaoss_all_btn.cget("text"))
        app._kaoss_toggle_show_all()
        self.assertFalse(app._kaoss.show_all)

    def test_axis_labels_sit_on_bottom_and_left(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        canvas = app._kaoss_canvas
        canvas.update_idletasks()
        app._kaoss_draw_grid()
        w = max(40, int(canvas.winfo_width()))
        h = max(40, int(canvas.winfo_height()))
        texts = []
        for item in canvas.find_withtag("axis"):
            if canvas.type(item) != "text":
                continue
            texts.append((canvas.itemcget(item, "text"), canvas.coords(item)))
        names = " ".join(t for t, _ in texts)
        self.assertIn("X", names)
        self.assertIn("PITCH", names)
        self.assertIn("Y", names)
        self.assertIn("TONE", names)
        x_item = next(coords for text, coords in texts if "PITCH" in text)
        y_item = next(coords for text, coords in texts if "TONE" in text and "PITCH" not in text)
        self.assertGreater(x_item[1], h * 0.65)
        self.assertLess(y_item[0], w * 0.35)

    def test_scale_button_opens_named_grid(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._open_kaoss_scale_grid()
        self.pump()
        self.assertTrue(app._kaoss_scale_open)
        names = [btn.cget("text") for btn in app._kaoss_scale_btns.values()]
        self.assertIn("MAJOR", names)
        self.assertIn("DORIAN", names)
        self.assertNotIn("DOR", names)
        self.assertNotIn("ION", names)
        app._kaoss_pick_scale("aeolian")
        self.pump()
        self.assertFalse(app._kaoss_scale_open)
        self.assertEqual(app._kaoss.scale_id, "aeolian")
        self.assertEqual(app._kaoss_scale_btn.cget("text"), "MINOR")

        app._kaoss_toggle_show_all()
        app._open_kaoss_scale_grid()
        self.pump()
        all_names = [btn.cget("text") for btn in app._kaoss_scale_btns.values()]
        self.assertIn("MIYAKOBUSHI", all_names)
        self.assertIn("GAMANASRAMA", all_names)
        self.assertNotIn("JPN", all_names)
        app._close_kaoss_scale_grid()
        app._kaoss_toggle_show_all()

    def test_led_field_follows_finger(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        canvas = app._kaoss_canvas
        canvas.update_idletasks()
        self.assertEqual(len(app._kaoss_leds), 12 * 7)

        class Ev:
            def __init__(self, x: int, y: int) -> None:
                self.x = x
                self.y = y
                self.state = 0x0100

        w = max(40, int(canvas.winfo_width()))
        h = max(40, int(canvas.winfo_height()))
        app._kaoss_on_press(Ev(int(w * 0.7), int(h * 0.25)))
        app._kaoss_paint_leds(0.0)
        fills = [canvas.itemcget(item, "fill") for item in app._kaoss_leds]
        self.assertTrue(any(self._luma(c) > 80 for c in fills))
        app._kaoss_on_release()

    @staticmethod
    def _luma(color: str) -> int:
        raw = (color or "#000000").lstrip("#")
        if len(raw) < 6:
            return 0
        return int(raw[0:2], 16) + int(raw[2:4], 16) + int(raw[4:6], 16)


if __name__ == "__main__":
    sys.exit(unittest.main())
