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
            cls.app._close_kaoss_picker(restore_main=False)
            cls.app._close_kaoss_settings(restore_main=False)
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
        app._kaoss.show_all = False
        app._kaoss.show_axis_labels = True
        app._kaoss.viz_style = "glow"
        app._kaoss_led_geom = None
        if app._kaoss_picker_open:
            app._close_kaoss_picker(restore_main=False)
        if app._kaoss_settings_open:
            app._close_kaoss_settings(restore_main=False)
        if app._kaoss_play:
            app._kaoss_leave_play()
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
        app._open_kaoss_settings()
        self.pump()
        self.assertTrue(app._kaoss_settings_open)
        app._kaoss_toggle_show_all()
        self.pump()
        self.assertTrue(app._kaoss.show_all)
        self.assertGreater(len(app._kaoss.scale_ids()), curated)
        self.assertIn("ON", app._kaoss_settings_all_btn.cget("text"))
        app._close_kaoss_settings()
        self.pump()
        app._kaoss_toggle_show_all()
        self.assertFalse(app._kaoss.show_all)

    def test_program_button_opens_named_grid(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._open_kaoss_picker("program")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        names = [btn.cget("text") for btn in app._kaoss_picker_btns.values()]
        self.assertIn("LEAD", names)
        self.assertIn("FILTER", names)
        app._kaoss_picker_choose("morph")
        self.pump()
        self.assertFalse(app._kaoss_picker_open)
        self.assertEqual(app._kaoss.program_id, "morph")
        self.assertEqual(app._kaoss_prog_btn.cget("text"), "MORPH")

    def test_key_opens_grid_and_ignores_the_opening_finger_up(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._kaoss.set_key(0)
        app._open_kaoss_picker("key")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        names = [btn.cget("text") for btn in app._kaoss_picker_btns.values()]
        self.assertIn("C", names)
        self.assertIn("F#", names)
        # Same lift that opened KEY must not steal C# (the old "cycling" feel).
        app._kaoss_picker_btns["1"].event_generate("<ButtonRelease-1>")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        self.assertEqual(app._kaoss.key, 0)
        app._picker_ignore_until = 0.0
        app._kaoss_picker_choose("1")
        self.pump()
        self.assertFalse(app._kaoss_picker_open)
        self.assertEqual(app._kaoss.key, 1)
        self.assertEqual(app._kaoss_key_btn.cget("text"), "KEY C#")

    def test_octave_picker_sets_start_and_width_without_closing(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._open_kaoss_picker("octave")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        app._kaoss_picker_choose("start:36")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        self.assertEqual(app._kaoss.root_midi, 36)
        app._kaoss_picker_choose("wide:4")
        self.pump()
        self.assertTrue(app._kaoss_picker_open)
        self.assertEqual(app._kaoss.octaves, 4)
        app._close_kaoss_picker()
        self.pump()
        self.assertEqual(app._kaoss_oct_btn.cget("text"), "C2·4")

    def test_settings_can_hide_axis_labels(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._open_kaoss_settings()
        self.pump()
        app._kaoss_toggle_axis_labels()
        self.pump()
        self.assertFalse(app._kaoss.show_axis_labels)
        self.assertIn("OFF", app._kaoss_settings_axes_btn.cget("text"))
        app._close_kaoss_settings()
        self.pump()
        canvas = app._kaoss_canvas
        app._kaoss_draw_grid()
        texts = []
        for item in canvas.find_withtag("axis"):
            if canvas.type(item) != "text":
                continue
            texts.append(canvas.itemcget(item, "text"))
        names = " ".join(texts)
        self.assertNotIn("PITCH", names)
        self.assertNotIn("TONE", names)

    def test_full_pad_hides_chrome_hold_exit_restores(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        self.assertTrue(bool(app._nav.winfo_ismapped()))
        app._kaoss_enter_play()
        self.pump()
        self.assertTrue(app._kaoss_play)
        self.assertFalse(bool(app._nav.winfo_ismapped()))
        self.assertTrue(bool(app._kaoss_exit_bar.winfo_ismapped()))
        app._kaoss_exit_press()
        app._kaoss_exit_release()
        self.pump()
        self.assertTrue(app._kaoss_play)
        app._kaoss_exit_press()
        self.pump(0.85)
        self.assertFalse(app._kaoss_play)
        self.assertTrue(bool(app._nav.winfo_ismapped()))

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

    def test_tone_percent_updates_while_sliding(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        canvas = app._kaoss_canvas
        canvas.update_idletasks()
        app._kaoss_draw_grid()
        w = max(40, int(canvas.winfo_width()))
        h = max(40, int(canvas.winfo_height()))

        class Ev:
            def __init__(self, x: int, y: int, state: int = 0x0100) -> None:
                self.x = x
                self.y = y
                self.state = state

        def axis_blob() -> str:
            texts = []
            for item in canvas.find_withtag("axis-label"):
                if canvas.type(item) != "text":
                    continue
                texts.append(canvas.itemcget(item, "text"))
            return " ".join(texts)

        app._kaoss_on_press(Ev(4, h))
        self.pump()
        low = axis_blob()
        self.assertRegex(low, r"TONE\s+0%")
        self.assertIn("0%", app._kaoss_status_var.get())

        app._kaoss_on_move(Ev(4, 0))
        self.pump()
        high = axis_blob()
        self.assertRegex(high, r"TONE\s+100%")
        self.assertIn("100%", app._kaoss_status_var.get())
        self.assertNotEqual(low, high)
        app._kaoss_on_release()

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
        app._kaoss_draw_grid()
        self.assertEqual(app._kaoss.viz_style, "glow")
        self.assertEqual(app._kaoss_leds, [])
        self.assertTrue(canvas.find_withtag("glow"))

        app._kaoss_set_viz_style("cells")
        self.pump()
        self.assertEqual(len(app._kaoss_leds), 12 * 7)
        outline = canvas.itemcget(app._kaoss_leds[0], "outline")
        self.assertTrue(outline, "original CELLS tiles have a gap outline")

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

    def test_settings_can_pick_pad_viz(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app._open_kaoss_settings()
        self.pump()
        self.assertIn("glow", app._kaoss_settings_viz_btns)
        self.assertIn("cells", app._kaoss_settings_viz_btns)
        self.assertNotIn("static", app._kaoss_settings_viz_btns)
        self.assertEqual(app._kaoss_settings_viz_btns["cells"].cget("text"), "CELLS")
        self.assertEqual(len(app._kaoss_settings_ch_btns), 16)
        app._kaoss_set_viz_style("cells")
        self.assertEqual(app._kaoss.viz_style, "cells")
        app._close_kaoss_settings()
        self.pump()
        self.assertEqual(app._kaoss.viz_style, "cells")

    def test_settings_can_thicken_grid_lines(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        self.assertEqual(app._kaoss.grid_width, 2)
        app._open_kaoss_settings()
        self.pump()
        self.assertIsNotNone(app._kaoss_settings_grid_lbl)
        self.assertIn("2", app._kaoss_settings_grid_lbl.cget("text"))
        app._kaoss_nudge_grid_width(1)
        self.assertEqual(app._kaoss.grid_width, 3)
        self.assertIn("3", app._kaoss_settings_grid_lbl.cget("text"))
        app._close_kaoss_settings()
        self.pump()
        canvas = app._kaoss_canvas
        app._kaoss_draw_grid()
        widths = [
            int(float(canvas.itemcget(item, "width") or 0))
            for item in canvas.find_withtag("grid")
        ]
        self.assertTrue(widths)
        self.assertGreaterEqual(max(widths), 3)

    def test_wipe_fx_clears_bus_delay_and_drops_hold(self) -> None:
        app = self.app
        app._switch_mode("kaoss")
        self.pump()
        app.engine.set_kaoss_param("delay_mix", 0.8)
        app.engine.set_kaoss_param("reverb_mix", 0.6)
        app.engine.set_kaoss_param("drive", 0.5)
        app._kaoss.set_hold(True)
        canvas = app._kaoss_canvas
        canvas.update_idletasks()

        class Ev:
            def __init__(self, x: int, y: int) -> None:
                self.x = x
                self.y = y
                self.state = 0x0100

        app._kaoss_on_press(Ev(40, 40))
        app._kaoss_on_release()
        self.pump()
        self.assertTrue(app._kaoss.is_active())
        app._kaoss_wipe_fx()
        self.pump()
        self.assertFalse(app._kaoss.is_active())
        bus = app.engine.bus_fx_snapshot()
        self.assertLessEqual(bus["fx_delay_mix"], 0.001)
        self.assertLessEqual(bus["fx_reverb_mix"], 0.001)
        self.assertLessEqual(bus["fx_drive"], 0.001)

    @staticmethod
    def _luma(color: str) -> int:
        raw = (color or "#000000").lstrip("#")
        if len(raw) < 6:
            return 0
        return int(raw[0:2], 16) + int(raw[2:4], 16) + int(raw[4:6], 16)


if __name__ == "__main__":
    sys.exit(unittest.main())
