"""kaoss UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    KAOSS_PLAY_BORDER_PX,
    KAOSS_PLAY_EXIT_MS,
    KAOSS_PLAY_HOLD_SLOP_PX,
    NOTE_NAMES,
)
from pidi.kaoss import (
    GATE_PATTERNS,
    KAOSS_OUT_MODES,
    LED_COLS,
    LED_ROWS,
    PROGRAM_BY_ID,
    ROOT_OCTAVE_MIDI,
    SCALE_BY_ID,
    VIZ_STYLE_LABELS,
    VIZ_STYLES,
    KaossEvent,
    KaossProgram,
    clamp01,
    glow_radii,
    glow_step,
    grid_line_widths,
    hsv_to_rgb,
    note_grid_xs,
    note_name as kaoss_note_name,
    pad_led_hex,
    program_hue,
    rgb_hex,
)


class KaossScreenMixin:
    def _build_kaoss_mode(self) -> None:
        """XY pad: Kaossilator-style notes + original Kaoss Pad MIDI CCs."""
        shell = self._kaoss_shell
        for w in shell.winfo_children():
            w.destroy()
        self._kaoss_status_var.set(self._kaoss.status_line())

        header, body, footer = self._pack_screen_regions(
            shell,
            header_padx=8,
            header_pady=(6, 0),
            body_padx=6,
            body_pady=2,
            footer_padx=6,
            footer_pady=6,
        )
        self._kaoss_header = header
        self._kaoss_footer = footer
        tk.Label(
            header,
            text="Kaoss",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            textvariable=self._kaoss_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            anchor="e",
        ).pack(side=tk.RIGHT, padx=(12, 0))

        pad_wrap = tk.Frame(body, bg="#111111")
        pad_wrap.pack(fill=tk.BOTH, expand=True)
        self._kaoss_canvas = tk.Canvas(
            pad_wrap,
            bg="#08040a",
            highlightthickness=3,
            highlightbackground="#9d2449",
            bd=0,
            cursor="none",
        )
        self._kaoss_canvas.pack(fill=tk.BOTH, expand=True)
        self._kaoss_canvas.bind("<Configure>", lambda _e: self._kaoss_draw_grid())
        self._kaoss_canvas.bind("<ButtonPress-1>", self._kaoss_on_press)
        self._kaoss_canvas.bind("<B1-Motion>", self._kaoss_on_move)
        self._kaoss_canvas.bind("<ButtonRelease-1>", self._kaoss_on_release)
        # Finger leaving chrome still counts as a lift; full-pad uses the screen edge to exit.
        self._kaoss_canvas.bind("<Leave>", self._kaoss_on_leave)

        row_a = tk.Frame(footer, bg="#111111")
        row_a.pack(fill=tk.X, pady=(0, 4))
        self._kaoss_prog_btn = self._mk_touch_btn(
            row_a, "LEAD", lambda: self._open_kaoss_picker("program"), bg="#b16286"
        )
        self._kaoss_scale_btn = self._mk_touch_btn(
            row_a, "MAJOR", lambda: self._open_kaoss_picker("scale"), bg="#458588"
        )
        self._kaoss_scale_btn.configure(font=("DejaVu Sans", 12, "bold"))
        self._kaoss_key_btn = self._mk_touch_btn(
            row_a, "KEY C", lambda: self._open_kaoss_picker("key"), bg="#3c3836"
        )
        self._kaoss_oct_btn = self._mk_touch_btn(
            row_a, "2 OCT", lambda: self._open_kaoss_picker("octave"), bg="#3c3836"
        )
        self._kaoss_hold_btn = self._mk_touch_btn(
            row_a, "HOLD", self._kaoss_toggle_hold, bg="#3c3836"
        )
        for btn in (
            self._kaoss_prog_btn,
            self._kaoss_scale_btn,
            self._kaoss_key_btn,
            self._kaoss_oct_btn,
            self._kaoss_hold_btn,
        ):
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=8)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2)

        row_b = tk.Frame(footer, bg="#111111")
        row_b.pack(fill=tk.X)
        self._kaoss_gate_btn = self._mk_touch_btn(
            row_b, "GATE OFF", lambda: self._open_kaoss_picker("gate"), bg="#3c3836"
        )
        bpm_minus = self._mk_touch_btn(
            row_b, "BPM −", lambda: self._kaoss_nudge_bpm(-5), bg="#3c3836"
        )
        self._kaoss_bpm_lbl = tk.Label(
            row_b,
            text="120",
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            padx=6,
        )
        bpm_plus = self._mk_touch_btn(
            row_b, "BPM +", lambda: self._kaoss_nudge_bpm(5), bg="#3c3836"
        )
        self._kaoss_play_btn = self._mk_touch_btn(
            row_b, "FULL PAD", self._kaoss_on_full_pad_btn, bg="#689d6a"
        )
        self._kaoss_gear_btn = self._mk_touch_btn(
            row_b, "⚙", self._open_kaoss_settings, bg="#504945"
        )
        for btn in (
            self._kaoss_gate_btn,
            bpm_minus,
            bpm_plus,
            self._kaoss_play_btn,
        ):
            btn.configure(font=("DejaVu Sans", 12, "bold"), pady=8)
        self._kaoss_gear_btn.configure(font=("DejaVu Sans", 18, "bold"), pady=4, padx=12)
        # Pack gear first on the right so expand=True siblings cannot steal it.
        self._kaoss_gear_btn.pack(side=tk.RIGHT, fill=tk.Y, padx=2)
        self._kaoss_gate_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2)
        bpm_minus.pack(side=tk.LEFT, fill=tk.BOTH, padx=2)
        self._kaoss_bpm_lbl.pack(side=tk.LEFT)
        bpm_plus.pack(side=tk.LEFT, fill=tk.BOTH, padx=2)
        self._kaoss_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2)
        self._paint_kaoss()


    def _kaoss_enter_play(self) -> None:
        """Hide chrome so the pad owns the 800×480 surface."""
        if self._kaoss_play:
            return
        if self._kaoss_picker_open:
            self._close_kaoss_picker(restore_main=True)
        if self._kaoss_settings_open:
            self._close_kaoss_settings(restore_main=True)
        if self._mode != "kaoss":
            self._switch_mode("kaoss")
        self._kaoss_play = True
        self._kaoss_play_footer = False
        try:
            self._nav.pack_forget()
        except tk.TclError:
            pass
        if self._kaoss_header is not None:
            self._kaoss_header.pack_forget()
        if self._kaoss_footer is not None:
            self._kaoss_footer.pack_forget()
        self._kaoss_draw_grid()
        self._kaoss_arm_viz()
        self._append_log("KAOSS FULL PAD — hold the bottom edge for controls")


    def _kaoss_leave_play(self) -> None:
        if not self._kaoss_play:
            return
        self._kaoss_cancel_exit_hold()
        self._kaoss_play = False
        self._kaoss_play_footer = False
        self._kaoss_play_exit_from_inside = False
        self._kaoss_play_exit_anchor = None
        try:
            self._nav.pack(side=tk.TOP, fill=tk.X, before=self._mode_host)
        except tk.TclError:
            try:
                self._nav.pack(side=tk.TOP, fill=tk.X)
            except tk.TclError:
                pass
        footer = self._kaoss_footer
        header = self._kaoss_header
        if footer is not None:
            footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        if header is not None:
            header.pack(side=tk.TOP, fill=tk.X, padx=8, pady=(6, 0))
        self._paint_kaoss()
        self._kaoss_draw_grid()
        self._kaoss_arm_viz()


    def _kaoss_on_full_pad_btn(self) -> None:
        if self._kaoss_play:
            self._kaoss_leave_play()
            return
        self._kaoss_enter_play()


    def _kaoss_show_play_footer(self) -> None:
        if not self._kaoss_play or self._kaoss_play_footer:
            return
        footer = self._kaoss_footer
        if footer is None:
            return
        self._kaoss_play_footer = True
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        self._paint_kaoss()
        self._kaoss_draw_grid()


    def _kaoss_hide_play_footer(self) -> None:
        if not self._kaoss_play or not self._kaoss_play_footer:
            return
        self._kaoss_play_footer = False
        if self._kaoss_footer is not None:
            self._kaoss_footer.pack_forget()
        self._paint_kaoss()
        self._kaoss_draw_grid()


    def _kaoss_event_in_play_bottom(self, event: object) -> bool:
        canvas = self._kaoss_canvas
        if canvas is None:
            return False
        h = max(1, int(canvas.winfo_height()))
        y = float(getattr(event, "y", 0))
        return y >= (h - KAOSS_PLAY_BORDER_PX)


    def _kaoss_paint_play_exit(self, *, dwelling: bool) -> None:
        canvas = self._kaoss_canvas
        if canvas is None:
            return
        try:
            canvas.delete("play-exit")
            if dwelling:
                w = max(1, int(canvas.winfo_width()))
                h = max(1, int(canvas.winfo_height()))
                canvas.create_rectangle(
                    0,
                    max(0, h - 8),
                    w,
                    h,
                    fill="#fabd2f",
                    outline="",
                    tags="play-exit",
                )
                canvas.tag_raise("play-exit")
        except tk.TclError:
            pass


    def _kaoss_watch_play_exit(self, event: object, *, touching: bool) -> None:
        if not self._kaoss_play:
            return
        if not touching:
            self._kaoss_play_exit_from_inside = False
            self._kaoss_play_exit_anchor = None
            self._kaoss_cancel_exit_hold()
            return
        in_bottom = self._kaoss_event_in_play_bottom(event)
        if in_bottom:
            if not self._kaoss_play_exit_from_inside:
                return
            x = float(getattr(event, "x", 0))
            y = float(getattr(event, "y", 0))
            anchor = self._kaoss_play_exit_anchor
            if self._kaoss_exit_after_id is None or anchor is None:
                self._kaoss_play_exit_anchor = (x, y)
                self._kaoss_arm_play_exit()
                return
            slop = KAOSS_PLAY_HOLD_SLOP_PX
            dx = x - anchor[0]
            dy = y - anchor[1]
            if (dx * dx + dy * dy) > (slop * slop):
                self._kaoss_cancel_exit_hold()
                self._kaoss_play_exit_anchor = (x, y)
                self._kaoss_arm_play_exit()
            return
        self._kaoss_play_exit_from_inside = True
        self._kaoss_play_exit_anchor = None
        self._kaoss_cancel_exit_hold()


    def _kaoss_arm_play_exit(self) -> None:
        if not self._kaoss_play or self._kaoss_exit_after_id is not None:
            return
        self._kaoss_paint_play_exit(dwelling=True)
        self._kaoss_exit_after_id = self.root.after(
            KAOSS_PLAY_EXIT_MS, self._kaoss_exit_hold_done
        )


    def _kaoss_exit_hold_done(self) -> None:
        self._kaoss_exit_after_id = None
        self._kaoss_play_exit_anchor = None
        self._kaoss_paint_play_exit(dwelling=False)
        if self._kaoss.touching:
            self._kaoss_on_release()
        if self._kaoss_play_footer:
            self._kaoss_hide_play_footer()
        else:
            self._kaoss_show_play_footer()


    def _kaoss_cancel_exit_hold(self) -> None:
        aid = self._kaoss_exit_after_id
        self._kaoss_exit_after_id = None
        if aid is not None:
            try:
                self.root.after_cancel(aid)
            except Exception:
                pass
        self._kaoss_paint_play_exit(dwelling=False)


    def _kaoss_xy(self, event: tk.Event) -> Tuple[float, float]:  # type: ignore[name-defined]
        canvas = self._kaoss_canvas
        if canvas is None:
            return 0.5, 0.5
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        x = max(0.0, min(1.0, float(event.x) / w))
        y = max(0.0, min(1.0, 1.0 - (float(event.y) / h)))
        return x, y


    def _kaoss_on_press(self, event: tk.Event) -> str:  # type: ignore[name-defined]
        if self._kaoss_play:
            in_bottom = self._kaoss_event_in_play_bottom(event)
            self._kaoss_play_exit_from_inside = not in_bottom
            self._kaoss_play_exit_anchor = None
            self._kaoss_cancel_exit_hold()
            if self._kaoss_play_footer and not in_bottom:
                self._kaoss_hide_play_footer()
        x, y = self._kaoss_xy(event)
        now = time.monotonic()
        self._kaoss_apply(self._kaoss.touch(x, y, now=now), began=True)
        self._kaoss_push_ripple(x, y, now)
        self._kaoss_push_trail(x, y, now)
        self._kaoss_draw_cursor(x, y, active=True)
        self._kaoss_paint_leds(now)
        self._kaoss_arm_tick()
        self._kaoss_arm_viz()
        return "break"


    def _kaoss_on_move(self, event: tk.Event) -> str:  # type: ignore[name-defined]
        if not self._kaoss.touching:
            return "break"
        x, y = self._kaoss_xy(event)
        self._kaoss_apply(self._kaoss.move(x, y))
        self._kaoss_push_trail(x, y)
        if self._kaoss.viz_style != "glow":
            self._kaoss_draw_cursor(x, y, active=True)
        self._kaoss_watch_play_exit(event, touching=True)
        return "break"


    def _kaoss_on_release(self, _event: object = None) -> str:
        self._kaoss_watch_play_exit(_event or object(), touching=False)
        self._kaoss_apply(self._kaoss.release(), ended=not self._kaoss.is_active())
        if self._kaoss.is_active():
            self._kaoss_arm_tick()
        self._kaoss_draw_cursor(self._kaoss.x, self._kaoss.y, active=self._kaoss.is_active())
        self._kaoss_paint_leds()
        return "break"


    def _kaoss_on_leave(self, event: tk.Event) -> str:  # type: ignore[name-defined]
        # Full-pad: the screen edge is the exit gesture — don't treat Leave as lift.
        if self._kaoss_play:
            return "break"
        # Leave fires when crossing chrome; only treat as lift if button is down
        if getattr(event, "state", 0) & 0x0100 and self._kaoss.touching:
            return self._kaoss_on_release(event)
        return "break"


    def _kaoss_arm_tick(self) -> None:
        if self._kaoss_tick_armed:
            return
        if not self._kaoss.is_active() and self._kaoss.gate().beats <= 0.0:
            return
        self._kaoss_tick_armed = True
        self._kaoss_after_id = self.root.after(16, self._kaoss_tick)


    def _kaoss_cancel_tick(self) -> None:
        self._kaoss_tick_armed = False
        self._kaoss_cancel_exit_hold()
        aid = self._kaoss_after_id
        self._kaoss_after_id = None
        if aid is not None:
            try:
                self.root.after_cancel(aid)
            except Exception:
                pass


    def _kaoss_arm_viz(self) -> None:
        if self._kaoss_viz_after_id is not None:
            return
        if self._mode != "kaoss" or self._stop.is_set():
            return
        self._kaoss_viz_after_id = self.root.after(
            50 if self._kaoss.viz_style == "glow" else 80,
            self._kaoss_viz_tick,
        )


    def _kaoss_cancel_viz(self) -> None:
        aid = self._kaoss_viz_after_id
        self._kaoss_viz_after_id = None
        if aid is not None:
            try:
                self.root.after_cancel(aid)
            except Exception:
                pass


    def _kaoss_viz_tick(self) -> None:
        self._kaoss_viz_after_id = None
        if self._stop.is_set() or self._mode != "kaoss":
            return
        self._kaoss_paint_leds()
        self._kaoss_arm_viz()


    def _kaoss_push_trail(self, x: float, y: float, now: Optional[float] = None) -> None:
        t = time.monotonic() if now is None else float(now)
        if self._kaoss_trail:
            lx, ly, _lt = self._kaoss_trail[-1]
            if abs(lx - x) < 0.018 and abs(ly - y) < 0.018:
                self._kaoss_trail[-1] = (x, y, t)
                return
        self._kaoss_trail.append((x, y, t))
        if len(self._kaoss_trail) > 12:
            del self._kaoss_trail[:-12]


    def _kaoss_push_ripple(self, x: float, y: float, now: Optional[float] = None) -> None:
        t = time.monotonic() if now is None else float(now)
        self._kaoss_ripples.append((x, y, t))
        if len(self._kaoss_ripples) > 4:
            del self._kaoss_ripples[:-4]


    def _kaoss_tick(self) -> None:
        self._kaoss_tick_armed = False
        self._kaoss_after_id = None
        if self._stop.is_set():
            return
        if not self._kaoss.is_active():
            return
        events = self._kaoss.tick(time.monotonic())
        if events:
            self._kaoss_apply(events)
            if self._mode == "kaoss":
                self._paint_kaoss_status()
        if self._kaoss.is_active():
            self._kaoss_arm_tick()


    def _kaoss_capture_fx(self) -> Dict[str, Any]:
        st = self.engine.modulation_state()
        return {
            "tone": float(st.get("tone", 0.5)),
            "morph": float(st.get("morph", 0.0)),
            "level": float(st.get("synth_level", st.get("level", 1.0))),
            "attack": float(st.get("attack", 0.0)),
            "release": float(st.get("release", 0.0)),
            "vib_depth": float(st.get("vib_depth", 0.0)),
            "vib_always": float(st.get("vib_always", 0.0)),
            "bus_fx": self.engine.bus_fx_snapshot(),
        }


    def _kaoss_overlay_names(self, prog: KaossProgram) -> set:
        """XY params this program writes live. LEAD tone stays put like a knob."""
        names = {p for p in (prog.x_param, prog.y_param) if p}
        if prog.id == "lead":
            names.discard("tone")
        if prog.id == "morph":
            names.discard("morph")
        return names


    def _kaoss_restore_fx(self) -> None:
        snap = self._kaoss_fx_snap
        self._kaoss_fx_snap = None
        if not isinstance(snap, dict):
            return
        names = self._kaoss_overlay_names(self._kaoss.program())
        if "tone" in names:
            self.engine.set_tone(float(snap.get("tone", 0.5)))
        if "morph" in names:
            self.engine.set_morph(float(snap.get("morph", 0.0)))
        if "vib" in names:
            depth = float(snap.get("vib_depth", 0.0))
            # set_vib_depth expects 0..1 (or MIDI). Stored value is semitones 0..2.
            self.engine.set_vib_depth(depth / 2.0)
            self.engine.set_vib_always(float(snap.get("vib_always", 0.0)))
        if names.intersection(("level", "attack", "release")):
            with self.engine._lock:
                if "level" in names:
                    self.engine._synth_level = float(snap.get("level", self.engine._synth_level))
                if "attack" in names:
                    self.engine._attack_sec = float(snap.get("attack", self.engine._attack_sec))
                if "release" in names:
                    self.engine._release_sec = float(snap.get("release", self.engine._release_sec))
        bus = snap.get("bus_fx")
        bus_names = names.intersection(self.engine.KAOSS_BUS_PARAMS)
        if isinstance(bus, dict) and bus_names:
            current = self.engine.bus_fx_snapshot()
            for key in bus_names:
                if key in bus:
                    current[key] = bus[key]
            self.engine.apply_bus_fx_snapshot(current)
        self._q_put(("mod",))


    def _kaoss_wipe_fx(self) -> None:
        """Kill leftover pad delay/reverb/drive and drop a held KAOSS note."""
        self._kaoss_apply(
            self._kaoss.release(force=True), ended=True, restore=False
        )
        self._kaoss_fx_snap = None
        self.engine.wipe_kaoss_bus_fx()
        self._q_put(("mod",))
        self._mark_settings_dirty()
        self._append_log("KAOSS FX wiped")
        if self._mode == "kaoss":
            self._paint_kaoss_status()
            self._kaoss_refresh_axis_labels()


    def _kaoss_midi_send(self, msg: mido.Message) -> None:
        if self._kaoss.out_mode not in ("usb", "both"):
            return
        if self._songs.outport() is None:
            self._songs.ensure_outport()
        port = self._songs.outport()
        if port is None:
            return
        try:
            port.send(msg)
        except Exception:
            pass


    def _kaoss_apply(
        self,
        events: List[KaossEvent],
        *,
        began: bool = False,
        ended: bool = False,
        restore: Optional[bool] = None,
    ) -> None:
        prog = self._kaoss.program()
        if began and self._kaoss_fx_snap is None:
            self._kaoss_fx_snap = self._kaoss_capture_fx()
        ch = self._kaoss.channel & 0x0F
        want_local = self._kaoss.out_mode in ("local", "both")
        remote = self.engine.using_remote()
        for ev in events:
            if ev.kind == "note_on":
                vel = max(1, min(127, int(ev.velocity)))
                if remote:
                    self.engine.note_on(ch, ev.note, vel)
                elif want_local:
                    self.engine.note_on(ch, ev.note, vel)
                    self._seq.record_note(True, ch, ev.note, vel)
                    self._q_put(("on", ch, ev.note, vel))
                self._kaoss_midi_send(
                    mido.Message("note_on", channel=ch, note=ev.note & 0x7F, velocity=vel)
                )
            elif ev.kind == "note_off":
                if remote:
                    self.engine.note_off(ch, ev.note)
                elif want_local:
                    self.engine.note_off(ch, ev.note)
                    self._seq.record_note(False, ch, ev.note, 0)
                    self._q_put(("off", ch, ev.note))
                self._kaoss_midi_send(
                    mido.Message("note_off", channel=ch, note=ev.note & 0x7F, velocity=0)
                )
            elif ev.kind in ("cc", "touch"):
                if remote:
                    client = getattr(self, "_jambox", None)
                    if client is not None:
                        client.midi(
                            "control_change",
                            channel=ch,
                            control=ev.control & 0x7F,
                            value=ev.value & 0x7F,
                        )
                self._kaoss_midi_send(
                    mido.Message(
                        "control_change",
                        channel=ch,
                        control=ev.control & 0x7F,
                        value=ev.value & 0x7F,
                    )
                )
            elif ev.kind == "param":
                self.engine.set_kaoss_param(ev.param, ev.param_value)
                self._q_put(("mod",))
        should_restore = ended and not self._kaoss.touching and (
            restore is True or (restore is not False and not self._kaoss.hold)
        )
        if should_restore:
            self._kaoss_restore_fx()
        if self._mode == "kaoss":
            self._paint_kaoss_status()
            self._kaoss_refresh_axis_labels()


    def _kaoss_apply_program(self, program_id: str) -> None:
        keep_hold = bool(self._kaoss.hold and self._kaoss.is_active())
        x, y = self._kaoss.x, self._kaoss.y
        if keep_hold:
            # HOLD is a drone: keep the latched note so FILTER/ECHO can run on it.
            # Still restore the previous program's overlays (VIB must not stick).
            self._kaoss_restore_fx()
            self._kaoss.set_program(program_id)
            self._kaoss_apply(
                self._kaoss.reassert(now=time.monotonic()), began=True
            )
            self._kaoss_arm_tick()
        else:
            was = self._kaoss.touching
            self._kaoss_apply(self._kaoss.release(force=True), ended=True, restore=True)
            self._kaoss.set_program(program_id)
            if was:
                self._kaoss_apply(
                    self._kaoss.touch(x, y, now=time.monotonic()), began=True
                )
                self._kaoss_arm_tick()
        self._paint_kaoss()
        self._kaoss_draw_grid()
        self._mark_settings_dirty()


    def _kaoss_picker_spec(self, kind: str) -> Tuple[str, str, int, List[Tuple[str, str]]]:
        """title, count line, columns, (id, label) tiles."""
        if kind == "program":
            ids = list(self._kaoss.program_ids())
            if self._kaoss.program_id not in ids:
                ids.insert(0, self._kaoss.program_id)
            items = [
                (pid, PROGRAM_BY_ID[pid].label)
                for pid in ids
                if pid in PROGRAM_BY_ID
            ]
            catalog = "all programs" if self._kaoss.show_all else "starter"
            return "PROGRAM — tap one", f"{len(items)} {catalog}", 3, items
        if kind == "scale":
            ids = list(self._kaoss.scale_ids())
            if self._kaoss.scale_id not in ids:
                ids.insert(0, self._kaoss.scale_id)
            items = [
                (sid, SCALE_BY_ID[sid].label)
                for sid in ids
                if sid in SCALE_BY_ID
            ]
            catalog = "all factory" if self._kaoss.show_all else "starter · more in ⚙"
            return "SCALES — tap one", f"{len(items)} {catalog}", 3, items
        if kind == "key":
            items = [(str(i), NOTE_NAMES[i]) for i in range(12)]
            return "KEY — tap one", "root of the scale", 6, items
        if kind == "octave":
            starts = [(f"start:{midi}", kaoss_note_name(midi)) for midi in ROOT_OCTAVE_MIDI]
            wides = [(f"wide:{n}", f"{n} OCT wide") for n in range(1, 5)]
            return (
                "OCTAVE — start + width",
                "left edge of the pad, then how many octaves",
                5,
                starts + wides,
            )
        items = [(g.id, g.label) for g in GATE_PATTERNS]
        return "GATE — tap one", "retrigger while the pad is down", 2, items


    def _open_kaoss_scale_grid(self) -> None:
        self._open_kaoss_picker("scale")


    def _open_kaoss_picker(self, kind: str) -> None:
        """VOICES-style grid instead of cycling a list with one button."""
        if kind not in ("program", "scale", "key", "octave", "gate"):
            return
        if self._kaoss_picker_open and self._kaoss_picker_kind == kind:
            return
        if self._kaoss_play:
            self._kaoss_leave_play()
        if self._kaoss_settings_open:
            self._close_kaoss_settings(restore_main=False)
        if self._kaoss_picker_open:
            self._close_kaoss_picker(restore_main=False)
        if self._mode != "kaoss":
            self._switch_mode("kaoss")
        self._kaoss_picker_open = True
        self._kaoss_picker_kind = kind
        self._kaoss_scale_open = kind == "scale"
        self._kaoss_cancel_viz()
        self._kaoss_shell.pack_forget()

        title, count, _cols, _items = self._kaoss_picker_spec(kind)
        self._kaoss_picker_frame = tk.Frame(self._mode_host, bg="#111111")
        self._kaoss_picker_frame.pack(fill=tk.BOTH, expand=True)
        header, body, footer = self._pack_screen_regions(
            self._kaoss_picker_frame,
            header_padx=8,
            header_pady=(6, 4),
            body_padx=4,
            body_pady=2,
            footer_padx=6,
            footer_pady=6,
        )
        tk.Label(
            header,
            text=title,
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._kaoss_picker_count_var.set(count)
        tk.Label(
            header,
            textvariable=self._kaoss_picker_count_var,
            font=("DejaVu Sans", 12),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        _wrap, _canvas, inner, drag = self._build_touch_scroll_area(body)
        self._kaoss_picker_inner = inner
        self._kaoss_picker_drag = drag
        self._fill_kaoss_picker()
        binder = drag.get("_bind_tree")
        if callable(binder):
            binder(inner)

        self._mk_touch_btn(
            footer, "CLOSE", self._close_kaoss_picker, bg="#9d0006"
        ).pack(fill=tk.X, ipady=14)
        self._arm_overlay_guard()


    def _kaoss_picker_current(self) -> str:
        kind = self._kaoss_picker_kind
        if kind == "program":
            return self._kaoss.program_id
        if kind == "scale":
            return self._kaoss.scale_id
        if kind == "key":
            return str(self._kaoss.key % 12)
        if kind == "octave":
            return ""
        return self._kaoss.gate_id


    def _paint_kaoss_picker(self) -> None:
        kind = self._kaoss_picker_kind
        if kind == "octave":
            selected = {
                f"start:{self._kaoss.root_octave_midi()}",
                f"wide:{self._kaoss.octaves}",
            }
            for item_id, btn in self._kaoss_picker_btns.items():
                on = item_id in selected
                color = "#458588" if on else "#3c3836"
                try:
                    btn.configure(bg=color, activebackground=color)
                except tk.TclError:
                    pass
            return
        current = self._kaoss_picker_current()
        for item_id, btn in self._kaoss_picker_btns.items():
            on = item_id == current
            color = "#458588" if on else "#3c3836"
            try:
                btn.configure(bg=color, activebackground=color)
            except tk.TclError:
                pass


    def _kaoss_picker_choose(self, value: str) -> None:
        kind = self._kaoss_picker_kind
        if kind == "program":
            self._kaoss_apply_program(value)
            self._append_log(f"KAOSS program → {self._kaoss.program().label}")
        elif kind == "scale":
            self._kaoss.set_scale(value)
            self._kaoss_apply(self._kaoss.retune())
            self._append_log(f"KAOSS scale → {self._kaoss.scale_label()}")
        elif kind == "key":
            self._kaoss.set_key(int(value))
            self._kaoss_apply(self._kaoss.retune())
            self._append_log(f"KAOSS key → {NOTE_NAMES[self._kaoss.key % 12]}")
        elif kind == "octave":
            if value.startswith("start:"):
                self._kaoss.set_root_midi(int(value.split(":", 1)[1]))
            elif value.startswith("wide:"):
                self._kaoss.set_octaves(int(value.split(":", 1)[1]))
            else:
                self._kaoss.set_octaves(int(value))
            self._kaoss_apply(self._kaoss.retune())
            self._append_log(
                f"KAOSS octave → {kaoss_note_name(self._kaoss.root_midi)} "
                f"×{self._kaoss.octaves}"
            )
            self._paint_kaoss_picker()
            self._paint_kaoss()
            self._mark_settings_dirty()
            return
        else:
            self._kaoss.set_gate(value, now=time.monotonic())
            if self._kaoss.gate().beats > 0.0 and self._kaoss.is_active():
                self._kaoss_arm_tick()
            self._append_log(f"KAOSS {self._kaoss.gate().label}")
        self._close_kaoss_picker()
        self._paint_kaoss()
        self._kaoss_draw_grid()
        self._mark_settings_dirty()


    def _kaoss_pick_scale(self, scale_id: str) -> None:
        self._kaoss_picker_kind = "scale"
        self._kaoss_picker_choose(scale_id)


    def _close_kaoss_picker(self, restore_main: bool = True) -> None:
        if not self._kaoss_picker_open:
            return
        if self._kaoss_picker_frame is not None:
            self._kaoss_picker_frame.destroy()
            self._kaoss_picker_frame = None
        self._kaoss_picker_btns = {}
        self._kaoss_scale_btns = {}
        self._kaoss_picker_inner = None
        self._kaoss_picker_drag = None
        self._kaoss_picker_open = False
        self._kaoss_picker_kind = ""
        self._kaoss_scale_open = False
        if restore_main and self._mode == "kaoss" and not self._kaoss_settings_open:
            self._kaoss_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_kaoss()
            self._kaoss_draw_grid()
            self._kaoss_arm_tick()
            self._kaoss_arm_viz()


    def _close_kaoss_scale_grid(self, restore_main: bool = True) -> None:
        self._close_kaoss_picker(restore_main=restore_main)


    def _kaoss_docs_scale_grid(self) -> None:
        """Open the full factory list so the docs shot shows readable names."""
        self._kaoss.set_show_all(True)
        self._open_kaoss_picker("scale")


    def _open_kaoss_settings(self) -> None:
        if self._kaoss_settings_open:
            return
        if self._kaoss_play:
            self._kaoss_leave_play()
        if self._kaoss_picker_open:
            self._close_kaoss_picker(restore_main=False)
        if self._mode != "kaoss":
            self._switch_mode("kaoss")
        self._kaoss_settings_open = True
        self._kaoss_cancel_viz()
        self._kaoss_shell.pack_forget()

        self._kaoss_settings_frame = tk.Frame(self._mode_host, bg="#111111")
        self._kaoss_settings_frame.pack(fill=tk.BOTH, expand=True)
        header, body, footer = self._pack_screen_regions(
            self._kaoss_settings_frame,
            header_padx=8,
            header_pady=(6, 4),
            body_padx=8,
            body_pady=4,
            footer_padx=6,
            footer_pady=6,
        )
        tk.Label(
            header,
            text="KAOSS settings",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text="drag to scroll",
            font=("DejaVu Sans", 12),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        _wrap, canvas, inner, drag = self._build_touch_scroll_area(body)

        wipe = self._mk_scroll_select_btn(
            inner, "WIPE FX", self._kaoss_wipe_fx, drag, bg="#9d0006"
        )
        wipe.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
        wipe.pack(fill=tk.X, pady=(0, 8))

        toggles = tk.Frame(inner, bg="#111111")
        toggles.pack(fill=tk.X, pady=(0, 8))
        self._kaoss_settings_all_btn = self._mk_scroll_select_btn(
            toggles, "SHOW ALL", self._kaoss_toggle_show_all, drag, bg="#3c3836"
        )
        self._kaoss_settings_all_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
        self._kaoss_settings_all_btn.pack(fill=tk.X)
        overlay = tk.Frame(inner, bg="#111111")
        overlay.pack(fill=tk.X, pady=(0, 8))
        self._kaoss_settings_axes_btn = self._mk_scroll_select_btn(
            overlay, "AXES: ON", self._kaoss_toggle_axis_labels, drag, bg="#3c3836"
        )
        self._kaoss_settings_axes_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
        self._kaoss_settings_axes_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=(0, 4)
        )
        self._kaoss_settings_grid_btn = self._mk_scroll_select_btn(
            overlay, "GRID: ON", self._kaoss_toggle_grid_lines, drag, bg="#3c3836"
        )
        self._kaoss_settings_grid_btn.configure(font=("DejaVu Sans", 13, "bold"), pady=14)
        self._kaoss_settings_grid_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=(4, 0)
        )

        tk.Label(
            inner,
            text="PAD VIZ",
            font=("DejaVu Sans", 12, "bold"),
            fg="#a89984",
            bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, pady=(4, 2))
        viz_row = tk.Frame(inner, bg="#111111")
        viz_row.pack(fill=tk.X, pady=(0, 8))
        self._kaoss_settings_viz_btns = {}
        for style in VIZ_STYLES:
            btn = self._mk_scroll_select_btn(
                viz_row,
                VIZ_STYLE_LABELS.get(style, style.upper()),
                lambda s=style: self._kaoss_set_viz_style(s),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=12)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2)
            self._kaoss_settings_viz_btns[style] = btn

        tk.Label(
            inner,
            text="GRID LINES",
            font=("DejaVu Sans", 12, "bold"),
            fg="#a89984",
            bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, pady=(4, 2))
        grid_row = tk.Frame(inner, bg="#111111")
        grid_row.pack(fill=tk.X, pady=(0, 8))
        minus = self._mk_scroll_select_btn(
            grid_row, "−", lambda: self._kaoss_nudge_grid_width(-1), drag, bg="#3c3836"
        )
        minus.configure(font=("DejaVu Sans", 18, "bold"), pady=12)
        minus.pack(side=tk.LEFT, fill=tk.Y, padx=(0, 4), ipadx=16)
        self._kaoss_settings_grid_lbl = tk.Label(
            grid_row,
            text="2",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#1d2021",
            pady=12,
        )
        self._kaoss_settings_grid_lbl.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4
        )
        plus = self._mk_scroll_select_btn(
            grid_row, "+", lambda: self._kaoss_nudge_grid_width(1), drag, bg="#3c3836"
        )
        plus.configure(font=("DejaVu Sans", 18, "bold"), pady=12)
        plus.pack(side=tk.RIGHT, fill=tk.Y, padx=(4, 0), ipadx=16)

        tk.Label(
            inner,
            text="OUT",
            font=("DejaVu Sans", 12, "bold"),
            fg="#a89984",
            bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, pady=(4, 2))
        out_row = tk.Frame(inner, bg="#111111")
        out_row.pack(fill=tk.X, pady=(0, 8))
        self._kaoss_settings_out_btns = {}
        for mode in KAOSS_OUT_MODES:
            btn = self._mk_scroll_select_btn(
                out_row,
                mode.upper(),
                lambda m=mode: self._kaoss_set_out(m),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=12)
            btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2)
            self._kaoss_settings_out_btns[mode] = btn

        tk.Label(
            inner,
            text="MIDI channel",
            font=("DejaVu Sans", 12, "bold"),
            fg="#a89984",
            bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, pady=(4, 2))
        ch_grid = tk.Frame(inner, bg="#111111")
        ch_grid.pack(fill=tk.X, pady=(0, 12))
        self._kaoss_settings_ch_btns = {}
        for i in range(16):
            r, c = divmod(i, 4)
            btn = self._mk_scroll_select_btn(
                ch_grid,
                str(i + 1),
                lambda ch=i: self._kaoss_set_channel(ch),
                drag,
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 16, "bold"), pady=18)
            btn.grid(row=r, column=c, sticky="nsew", padx=4, pady=4, ipadx=4, ipady=8)
            self._kaoss_settings_ch_btns[i] = btn
        for c in range(4):
            ch_grid.grid_columnconfigure(c, weight=1)

        binder = drag.get("_bind_tree")
        if callable(binder):
            binder(inner)
        self.root.after_idle(lambda: inner.event_generate("<Configure>"))

        self._mk_touch_btn(
            footer, "CLOSE", self._close_kaoss_settings, bg="#9d0006"
        ).pack(fill=tk.X, ipady=14)
        self._paint_kaoss_settings()
        self._arm_overlay_guard()


    def _close_kaoss_settings(self, restore_main: bool = True) -> None:
        if not self._kaoss_settings_open:
            return
        if self._kaoss_settings_frame is not None:
            self._kaoss_settings_frame.destroy()
            self._kaoss_settings_frame = None
        self._kaoss_settings_all_btn = None
        self._kaoss_settings_axes_btn = None
        self._kaoss_settings_grid_btn = None
        self._kaoss_settings_grid_lbl = None
        self._kaoss_settings_viz_btns = {}
        self._kaoss_settings_out_btns = {}
        self._kaoss_settings_ch_btns = {}
        self._kaoss_settings_open = False
        if restore_main and self._mode == "kaoss" and not self._kaoss_picker_open:
            self._kaoss_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_kaoss()
            self._kaoss_draw_grid()
            self._kaoss_arm_tick()
            self._kaoss_arm_viz()


    def _paint_kaoss_settings(self) -> None:
        on = bool(self._kaoss.show_all)
        color = "#d79921" if on else "#3c3836"
        if self._kaoss_settings_all_btn is not None:
            self._kaoss_settings_all_btn.configure(
                text="SHOW ALL: ON" if on else "SHOW ALL: OFF",
                bg=color,
                activebackground=color,
            )
        axes = bool(self._kaoss.show_axis_labels)
        axes_color = "#458588" if axes else "#3c3836"
        if self._kaoss_settings_axes_btn is not None:
            self._kaoss_settings_axes_btn.configure(
                text="AXES: ON" if axes else "AXES: OFF",
                bg=axes_color,
                activebackground=axes_color,
            )
        grid_on = bool(self._kaoss.show_grid_lines)
        grid_color = "#458588" if grid_on else "#3c3836"
        if self._kaoss_settings_grid_btn is not None:
            self._kaoss_settings_grid_btn.configure(
                text="GRID: ON" if grid_on else "GRID: OFF",
                bg=grid_color,
                activebackground=grid_color,
            )
        if self._kaoss_settings_grid_lbl is not None:
            n = int(self._kaoss.grid_width)
            self._kaoss_settings_grid_lbl.configure(text=f"{n} px")
        for style, btn in self._kaoss_settings_viz_btns.items():
            selected = style == self._kaoss.viz_style
            c = "#b16286" if selected else "#3c3836"
            try:
                btn.configure(bg=c, activebackground=c)
            except tk.TclError:
                pass
        for mode, btn in self._kaoss_settings_out_btns.items():
            selected = mode == self._kaoss.out_mode
            c = "#458588" if selected else "#3c3836"
            try:
                btn.configure(bg=c, activebackground=c)
            except tk.TclError:
                pass
        for ch, btn in self._kaoss_settings_ch_btns.items():
            selected = ch == self._kaoss.channel
            c = "#458588" if selected else "#3c3836"
            try:
                btn.configure(bg=c, activebackground=c)
            except tk.TclError:
                pass


    def _kaoss_toggle_hold(self) -> None:
        _on, events = self._kaoss.toggle_hold()
        self._kaoss_apply(events, ended=not self._kaoss.hold)
        self._paint_kaoss()
        self._kaoss_draw_cursor(
            self._kaoss.x, self._kaoss.y, active=self._kaoss.is_active()
        )
        if self._kaoss.is_active():
            self._kaoss_arm_tick()
        self._mark_settings_dirty()


    def _kaoss_nudge_bpm(self, delta: float) -> None:
        self._kaoss.nudge_bpm(delta)
        self._paint_kaoss()
        self._mark_settings_dirty()


    def _kaoss_set_out(self, mode: str) -> None:
        self._kaoss.set_out_mode(mode)
        if self._kaoss.out_mode in ("usb", "both"):
            name = self._songs.ensure_outport()
            if name:
                self._append_log(f"KAOSS USB out → {name}")
            else:
                self._append_log("KAOSS USB out — no MIDI port found")
        self._paint_kaoss_settings()
        self._mark_settings_dirty()


    def _kaoss_set_channel(self, channel: int) -> None:
        self._kaoss.set_channel(channel)
        self._paint_kaoss_settings()
        self._mark_settings_dirty()
        self._append_log(f"KAOSS MIDI ch → {self._kaoss.channel + 1}")


    def _kaoss_toggle_axis_labels(self) -> None:
        on = self._kaoss.toggle_show_axis_labels()
        self._paint_kaoss_settings()
        self._kaoss_draw_grid()
        self._mark_settings_dirty()
        self._append_log(f"KAOSS axis labels → {'ON' if on else 'OFF'}")


    def _kaoss_toggle_grid_lines(self) -> None:
        on = self._kaoss.toggle_show_grid_lines()
        self._paint_kaoss_settings()
        self._kaoss_draw_grid()
        self._mark_settings_dirty()
        self._append_log(f"KAOSS grid lines → {'ON' if on else 'OFF'}")


    def _kaoss_set_viz_style(self, style: str) -> None:
        self._kaoss.set_viz_style(style)
        self._kaoss_led_geom = None
        self._paint_kaoss_settings()
        self._kaoss_arm_viz()
        if self._kaoss_canvas is not None:
            self._kaoss_draw_grid()
        self._mark_settings_dirty()


    def _kaoss_nudge_grid_width(self, delta: int) -> None:
        self._kaoss.nudge_grid_width(delta)
        self._paint_kaoss_settings()
        if self._kaoss_canvas is not None:
            self._kaoss_draw_grid()
        self._mark_settings_dirty()


    def _kaoss_toggle_show_all(self) -> None:
        on = self._kaoss.toggle_show_all()
        self._paint_kaoss()
        self._paint_kaoss_settings()
        self._kaoss_draw_grid()
        if self._kaoss_picker_open and self._kaoss_picker_kind in ("program", "scale"):
            self._fill_kaoss_picker()
        self._mark_settings_dirty()
        n_scale = len(self._kaoss.scale_ids())
        n_prog = len(self._kaoss.program_ids())
        self._append_log(
            f"KAOSS list → {'ALL' if on else 'CURATED'} "
            f"({n_prog} programs, {n_scale} scales)"
        )


    def _paint_kaoss_status(self) -> None:
        morph = None
        tone = None
        live = bool(self._kaoss.touching or self._kaoss.hold)
        if self._kaoss.program_id == "morph":
            a, b, frac = self.engine.morph_neighbors()
            morph = (a, b, self._kaoss.y if live else frac)
        elif self._kaoss.program_id == "lead":
            if live:
                tone = float(self._kaoss.y)
            else:
                tone = float(self.engine.modulation_state().get("tone", 1.0))
        self._kaoss_status_var.set(self._kaoss.header_line(morph=morph, tone=tone))


    def _paint_kaoss(self) -> None:
        self._paint_kaoss_status()
        prog = self._kaoss.program()
        if self._kaoss_prog_btn is not None:
            color = "#b16286" if prog.kind == "note" else "#d79921"
            self._kaoss_prog_btn.configure(
                text=prog.label, bg=color, activebackground=color
            )
        if self._kaoss_scale_btn is not None:
            muted = prog.kind != "note"
            self._kaoss_scale_btn.configure(
                text=self._kaoss.scale_label(),
                bg="#3c3836" if muted else "#458588",
                activebackground="#3c3836" if muted else "#458588",
            )
        if self._kaoss_key_btn is not None:
            self._kaoss_key_btn.configure(text=f"KEY {NOTE_NAMES[self._kaoss.key % 12]}")
        if self._kaoss_oct_btn is not None:
            self._kaoss_oct_btn.configure(
                text=f"{kaoss_note_name(self._kaoss.root_midi)}·{self._kaoss.octaves}"
            )
        if self._kaoss_hold_btn is not None:
            on = self._kaoss.hold
            color = "#689d6a" if on else "#3c3836"
            self._kaoss_hold_btn.configure(
                text="HOLD: ON" if on else "HOLD",
                bg=color,
                activebackground=color,
            )
        if self._kaoss_gate_btn is not None:
            gate = self._kaoss.gate()
            on = gate.beats > 0.0
            color = "#b16286" if on else "#3c3836"
            self._kaoss_gate_btn.configure(
                text=gate.label, bg=color, activebackground=color
            )
        if self._kaoss_bpm_lbl is not None:
            self._kaoss_bpm_lbl.configure(text=str(int(round(self._kaoss.bpm))))
        if self._kaoss_play_btn is not None:
            if self._kaoss_play:
                self._kaoss_play_btn.configure(
                    text="EXIT", bg="#9d0006", activebackground="#9d0006"
                )
            else:
                self._kaoss_play_btn.configure(
                    text="FULL PAD", bg="#689d6a", activebackground="#689d6a"
                )


    def _kaoss_draw_grid(self) -> None:
        canvas = self._kaoss_canvas
        if canvas is None:
            return
        canvas.delete("grid")
        canvas.delete("axis")
        canvas.delete("axis-label")
        self._kaoss_axis_label_cache = None
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        self._kaoss_ensure_leds(w, h)
        prog = self._kaoss.program()
        if self._kaoss.show_grid_lines:
            regular, octave_w = grid_line_widths(self._kaoss.grid_width)
            # Faint overlay so scale degrees stay readable on top of the LEDs
            for frac, color in ((0.25, "#3a1528"), (0.5, "#5b203c"), (0.75, "#3a1528")):
                y = int(h * (1.0 - frac))
                stroke = octave_w if frac == 0.5 else regular
                canvas.create_line(0, y, w, y, fill=color, width=stroke, tags="grid")
            if prog.kind == "note":
                notes = self._kaoss.notes()
                key = self._kaoss.key % 12
                xs = note_grid_xs(len(notes), w)
                for i, x in enumerate(xs):
                    starts = notes[i] if i < len(notes) else None
                    octave = starts is not None and (starts % 12) == key
                    color = "#fb4934" if octave else "#4a2040"
                    canvas.create_line(
                        x,
                        0,
                        x,
                        h,
                        fill=color,
                        width=octave_w if octave else regular,
                        tags="grid",
                    )
            else:
                for frac in (0.25, 0.5, 0.75):
                    x = int(w * frac)
                    stroke = octave_w if frac == 0.5 else regular
                    canvas.create_line(
                        x, 0, x, h, fill="#3a1528", width=stroke, tags="grid"
                    )
        self._kaoss_draw_axes(canvas, w, h, prog)
        self._kaoss_paint_leds()
        if self._kaoss.is_active() or self._kaoss.hold:
            self._kaoss_draw_cursor(self._kaoss.x, self._kaoss.y, active=True)
        else:
            canvas.delete("cursor")
        canvas.tag_raise("grid")
        canvas.tag_raise("ripple")
        canvas.tag_raise("axis")
        canvas.tag_raise("axis-label")
        canvas.tag_raise("cursor")
        canvas.tag_raise("play-exit")


    def _kaoss_axis_pct(self, param: Optional[str], pad_axis: float) -> Optional[int]:
        """0–100 for a pad-mapped 0..1 param. None = this axis is not a mix amount."""
        if not param or param == "octave":
            return None
        if self._kaoss.touching or self._kaoss.hold:
            return int(round(clamp01(pad_axis) * 100.0))
        if param == "morph":
            return int(round(clamp01(self.engine.morph_neighbors()[2]) * 100.0))
        if param == "tone":
            return int(round(clamp01(self.engine.modulation_state().get("tone", 0.0)) * 100.0))
        if param == "level":
            return int(
                round(clamp01(self.engine.modulation_state().get("synth_level", 0.0)) * 100.0)
            )
        bus = self.engine.bus_fx_snapshot()
        if param in bus:
            return int(round(clamp01(float(bus.get(param, 0.0))) * 100.0))
        return None


    def _kaoss_axis_label_texts(self) -> Tuple[str, str]:
        prog = self._kaoss.program()
        x_label = f"X  {prog.x_axis}"
        y_label = f"Y  {prog.y_axis}"
        xp = self._kaoss_axis_pct(prog.x_param, self._kaoss.x)
        yp = self._kaoss_axis_pct(prog.y_param, self._kaoss.y)
        if xp is not None:
            x_label = f"X  {prog.x_axis} {xp}%"
        if prog.y_param == "morph":
            a, b, frac = self.engine.morph_neighbors()
            if self._kaoss.touching or self._kaoss.hold:
                frac = self._kaoss.y
            pct = int(round(clamp01(frac) * 100.0))
            y_label = f"Y  {a[:6]} {pct}% {b[:6]}"
        elif yp is not None:
            y_label = f"Y  {prog.y_axis} {yp}%"
        return x_label, y_label


    def _kaoss_refresh_axis_labels(self) -> None:
        canvas = self._kaoss_canvas
        if canvas is None or self._mode != "kaoss":
            return
        if not self._kaoss.show_axis_labels:
            if self._kaoss_axis_label_cache is not None:
                canvas.delete("axis-label")
                self._kaoss_axis_label_cache = None
            return
        x_label, y_label = self._kaoss_axis_label_texts()
        if (x_label, y_label) == self._kaoss_axis_label_cache and canvas.find_withtag(
            "axis-label"
        ):
            canvas.tag_raise("axis-label")
            canvas.tag_raise("cursor")
            return
        self._kaoss_axis_label_cache = (x_label, y_label)
        canvas.delete("axis-label")
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        left = 22
        bottom = max(28, h - 18)
        right = max(left + 40, w - 16)
        self._kaoss_axis_caption(
            canvas, (left + right) // 2, bottom - 13, x_label
        )
        try:
            self._kaoss_axis_caption(
                canvas, left + 15, h // 2, y_label, angle=90.0
            )
        except tk.TclError:
            self._kaoss_axis_caption(
                canvas, left + 30, h // 2, f"Y ↑ {y_label[2:].strip()}"
            )
        canvas.tag_raise("axis-label")
        canvas.tag_raise("cursor")


    def _kaoss_draw_axes(self, canvas: tk.Canvas, w: int, h: int, _prog) -> None:
        """L-shaped XY legend — labels sit on the edge they control, not both left."""
        if self._kaoss.show_grid_lines:
            spine = "#d3869b"
            left = 22
            bottom = max(28, h - 18)
            top = 16
            right = max(left + 40, w - 16)
            canvas.create_line(
                left,
                bottom,
                right,
                bottom,
                fill=spine,
                width=2,
                arrow=tk.LAST,
                arrowshape=(10, 12, 5),
                tags="axis",
            )
            canvas.create_line(
                left,
                bottom,
                left,
                top,
                fill=spine,
                width=2,
                arrow=tk.LAST,
                arrowshape=(10, 12, 5),
                tags="axis",
            )
        self._kaoss_axis_label_cache = None
        self._kaoss_refresh_axis_labels()


    def _kaoss_axis_caption(
        self,
        canvas: tk.Canvas,
        x: int,
        y: int,
        text: str,
        *,
        angle: float = 0.0,
    ) -> None:
        item = canvas.create_text(
            x,
            y,
            text=text,
            fill="#fbf1c7",
            font=("DejaVu Sans", 13, "bold"),
            angle=angle,
            tags=("axis", "axis-label"),
        )
        box = canvas.bbox(item)
        if box is None:
            return
        x0, y0, x1, y1 = box
        bg = canvas.create_rectangle(
            x0 - 5,
            y0 - 2,
            x1 + 5,
            y1 + 2,
            fill="#0c060a",
            outline="#5b203c",
            width=1,
            tags=("axis", "axis-label"),
        )
        canvas.tag_raise(item, bg)


    def _kaoss_ensure_leds(self, w: int, h: int) -> None:
        canvas = self._kaoss_canvas
        if canvas is None or w < 40 or h < 40:
            return
        style = self._kaoss.viz_style
        cols, rows = self._kaoss.led_grid_size()
        geom = (w, h, cols, rows, style)
        if style == "glow":
            if self._kaoss_led_geom == geom and self._kaoss_glow:
                return
            canvas.delete("led")
            canvas.delete("glow")
            canvas.delete("ripple")
            self._kaoss_leds = []
            self._kaoss_ripple_items = []
            wash = canvas.create_rectangle(
                0, 0, w, h, fill="#08040a", outline="", tags="glow"
            )
            blooms = [
                canvas.create_oval(0, 0, 0, 0, fill="", outline="", tags="glow")
                for _ in range(3)
            ]
            trail = [
                canvas.create_oval(0, 0, 0, 0, fill="", outline="", tags="glow")
                for _ in range(8)
            ]
            self._kaoss_glow = {"wash": wash, "blooms": blooms, "trail": trail}
            for _ in range(4):
                self._kaoss_ripple_items.append(
                    canvas.create_oval(0, 0, 0, 0, outline="", width=2, tags="ripple")
                )
            canvas.tag_lower("glow")
            self._kaoss_led_geom = geom
            return
        if self._kaoss_led_geom == geom and len(self._kaoss_leds) == LED_COLS * LED_ROWS:
            return
        canvas.delete("led")
        canvas.delete("glow")
        canvas.delete("ripple")
        self._kaoss_leds = []
        self._kaoss_glow = {}
        self._kaoss_ripple_items = []
        gap = 3
        cell_w = max(2.0, (w - gap * (LED_COLS + 1)) / float(LED_COLS))
        cell_h = max(2.0, (h - gap * (LED_ROWS + 1)) / float(LED_ROWS))
        for row in range(LED_ROWS):
            # row 0 is the bottom of the pad (Kaoss Y = 0)
            top = gap + (LED_ROWS - 1 - row) * (cell_h + gap)
            for col in range(LED_COLS):
                left = gap + col * (cell_w + gap)
                item = canvas.create_rectangle(
                    left,
                    top,
                    left + cell_w,
                    top + cell_h,
                    fill="#12060e",
                    outline="#1a0a14",
                    width=1,
                    tags="led",
                )
                self._kaoss_leds.append(item)
        for _ in range(4):
            self._kaoss_ripple_items.append(
                canvas.create_oval(0, 0, 0, 0, outline="", width=2, tags="ripple")
            )
        canvas.tag_lower("led")
        self._kaoss_led_geom = geom


    def _kaoss_viz_motion(self, now: Optional[float] = None):
        t = time.monotonic() if now is None else float(now)
        finger = None
        if self._kaoss.is_active() or self._kaoss.touching:
            finger = (self._kaoss.x, self._kaoss.y)
        trail: List[Tuple[float, float, float]] = []
        live_trail: List[Tuple[float, float, float]] = []
        for x, y, born in self._kaoss_trail:
            age = 1.0 - (t - born) / 0.45
            if age <= 0.0:
                continue
            live_trail.append((x, y, born))
            trail.append((x, y, min(1.0, age)))
        self._kaoss_trail = live_trail
        ripples: List[Tuple[float, float, float]] = []
        live_ripples: List[Tuple[float, float, float]] = []
        for x, y, born in self._kaoss_ripples:
            age = (t - born) / 0.55
            if age >= 1.0:
                continue
            live_ripples.append((x, y, born))
            ripples.append((x, y, max(0.0, age)))
        self._kaoss_ripples = live_ripples
        return t, finger, trail, ripples


    def _kaoss_paint_leds(self, now: Optional[float] = None) -> None:
        canvas = self._kaoss_canvas
        if canvas is None:
            return
        t, finger, trail, ripples = self._kaoss_viz_motion(now)
        hue = program_hue(self._kaoss.program_id)
        flash = self._kaoss.gate_flash()
        pulse = self._kaoss.viz_pulse(t)
        hold = bool(self._kaoss.hold and self._kaoss.is_active())
        if self._kaoss.viz_style == "glow":
            self._kaoss_paint_glow(canvas, t, finger, trail, hue, pulse, hold)
        elif self._kaoss_leds:
            idx = 0
            for row in range(LED_ROWS):
                for col in range(LED_COLS):
                    if idx >= len(self._kaoss_leds):
                        break
                    color = pad_led_hex(
                        col,
                        row,
                        t=t,
                        finger=finger,
                        trail=trail,
                        ripples=ripples,
                        hold=hold,
                        gate_flash=flash,
                        hue_shift=hue,
                    )
                    canvas.itemconfigure(self._kaoss_leds[idx], fill=color)
                    idx += 1
        self._kaoss_paint_ripples(canvas, ripples, hue)
        rim = rgb_hex(hsv_to_rgb(hue, 0.85, 0.28 + 0.55 * pulse))
        try:
            canvas.configure(highlightbackground=rim)
        except tk.TclError:
            pass
        canvas.tag_raise("grid")
        canvas.tag_raise("ripple")
        canvas.tag_raise("axis")
        canvas.tag_raise("axis-label")
        canvas.tag_raise("cursor")


    def _kaoss_paint_glow(
        self,
        canvas: tk.Canvas,
        t: float,
        finger: Optional[Tuple[float, float]],
        trail: List[Tuple[float, float, float]],
        hue: float,
        pulse: float,
        hold: bool,
    ) -> None:
        glow = self._kaoss_glow
        if not glow:
            return
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        dt = 0.0 if self._glow_t <= 0.0 else min(0.08, t - self._glow_t)
        self._glow_t = t
        target = 1.0 if finger is not None else 0.0
        if target > self._glow_amp and dt <= 0.0:
            dt = 0.02
        self._glow_amp = glow_step(self._glow_amp, target, dt)
        if finger is not None:
            self._glow_xy = finger
        amp = self._glow_amp
        wash_v = (0.05 + 0.07 * pulse + (0.04 if hold else 0.0)) * (0.35 + 0.65 * amp)
        canvas.itemconfigure(
            glow["wash"], fill=rgb_hex(hsv_to_rgb(hue, 0.55, wash_v))
        )
        canvas.coords(glow["wash"], 0, 0, w, h)
        blooms = glow.get("blooms") or []
        fx, fy = self._glow_xy
        px = fx * (w - 1)
        py = (1.0 - fy) * (h - 1)
        if amp < 0.02:
            for item in blooms:
                canvas.coords(item, 0, 0, 0, 0)
                canvas.itemconfigure(item, fill="")
        else:
            radii = glow_radii(min(w, h), amp)
            fills = (
                rgb_hex(hsv_to_rgb(hue, 0.85, 0.34 * amp)),
                rgb_hex(hsv_to_rgb(hue, 0.70, 0.68 * amp)),
                rgb_hex(hsv_to_rgb(hue, 0.18, 0.55 + 0.45 * amp)),
            )
            for item, radius, fill in zip(blooms, radii, fills):
                if radius < 1.5:
                    canvas.coords(item, 0, 0, 0, 0)
                    canvas.itemconfigure(item, fill="")
                    continue
                canvas.coords(item, px - radius, py - radius, px + radius, py + radius)
                canvas.itemconfigure(item, fill=fill, outline="")
        trail_items = glow.get("trail") or []
        for i, item in enumerate(trail_items):
            if i >= len(trail):
                canvas.coords(item, 0, 0, 0, 0)
                canvas.itemconfigure(item, fill="")
                continue
            tx, ty, age = trail[i]
            px = tx * (w - 1)
            py = (1.0 - ty) * (h - 1)
            radius = (8 + 18 * age) * max(0.25, amp)
            canvas.coords(item, px - radius, py - radius, px + radius, py + radius)
            canvas.itemconfigure(
                item,
                fill=rgb_hex(hsv_to_rgb(hue, 0.45, 0.50 * age * max(0.2, amp))),
                outline="",
            )


    def _kaoss_paint_ripples(
        self,
        canvas: tk.Canvas,
        ripples: List[Tuple[float, float, float]],
        hue: float,
    ) -> None:
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        for i, item in enumerate(self._kaoss_ripple_items):
            if i >= len(ripples):
                canvas.coords(item, 0, 0, 0, 0)
                canvas.itemconfigure(item, outline="")
                continue
            x, y, age = ripples[i]
            px = x * (w - 1)
            py = (1.0 - y) * (h - 1)
            radius = 10 + age * min(w, h) * 0.42
            color = rgb_hex(hsv_to_rgb(hue, 0.35, 0.95 * (1.0 - age)))
            canvas.coords(item, px - radius, py - radius, px + radius, py + radius)
            canvas.itemconfigure(item, outline=color, width=2)


    def _kaoss_draw_cursor(self, x: float, y: float, *, active: bool) -> None:
        canvas = self._kaoss_canvas
        if canvas is None:
            return
        canvas.delete("cursor")
        w = max(1, int(canvas.winfo_width()))
        h = max(1, int(canvas.winfo_height()))
        px = int(round(max(0.0, min(1.0, x)) * (w - 1)))
        py = int(round((1.0 - max(0.0, min(1.0, y))) * (h - 1)))
        hue = (max(0.0, min(1.0, x)) * 0.70 + program_hue(self._kaoss.program_id)) % 1.0
        # GLOW already draws the finger blob. The hard rings were the "two
        # circles that pop" — skip them so the envelope can fade in.
        if self._kaoss.viz_style == "glow":
            pass
        elif active:
            outer = rgb_hex(hsv_to_rgb(hue, 0.90, 0.55))
            mid = rgb_hex(hsv_to_rgb(hue, 0.85, 1.0))
            core = rgb_hex(hsv_to_rgb(hue, 0.12, 1.0))
            canvas.create_oval(
                px - 34, py - 34, px + 34, py + 34, outline=outer, width=2, tags="cursor"
            )
            canvas.create_oval(
                px - 20, py - 20, px + 20, py + 20, outline=mid, width=3, tags="cursor"
            )
            canvas.create_oval(
                px - 7, py - 7, px + 7, py + 7, fill=core, outline=mid, width=1, tags="cursor"
            )
            canvas.create_line(px - 28, py, px + 28, py, fill=mid, width=2, tags="cursor")
            canvas.create_line(px, py - 28, px, py + 28, fill=mid, width=2, tags="cursor")
        else:
            canvas.create_oval(
                px - 10, py - 10, px + 10, py + 10, outline="#665c54", width=2, tags="cursor"
            )
        if active and self._kaoss.program().kind == "note" and self._kaoss.sounding_note() is not None:
            canvas.create_text(
                px,
                max(18, py - 40),
                text=kaoss_note_name(self._kaoss.sounding_note() or 60),
                fill="#fbf1c7",
                font=("DejaVu Sans", 14, "bold"),
                tags="cursor",
            )
        canvas.tag_raise("cursor")


    def _kaoss_docs_pose(self) -> None:
        """Fake a finger on the pad so docs screenshots show the LED field."""
        self._switch_mode("kaoss")
        try:
            self.root.update_idletasks()
        except tk.TclError:
            pass
        now = time.monotonic()
        self._kaoss_apply(self._kaoss.touch(0.66, 0.70, now=now), began=True)
        self._kaoss_ripples = [(0.42, 0.32, now - 0.22), (0.66, 0.70, now - 0.04)]
        self._kaoss_trail = [
            (0.34, 0.22, now - 0.36),
            (0.44, 0.38, now - 0.24),
            (0.54, 0.52, now - 0.12),
            (0.66, 0.70, now),
        ]
        self._kaoss_draw_grid()
        self._kaoss_draw_cursor(0.66, 0.70, active=True)
        self._kaoss_arm_tick()
        self._kaoss_arm_viz()


    def _kaoss_docs_play(self) -> None:
        """Docs shot: full-pad play with the LED field lit."""
        self._kaoss_docs_pose()
        self._kaoss_enter_play()
