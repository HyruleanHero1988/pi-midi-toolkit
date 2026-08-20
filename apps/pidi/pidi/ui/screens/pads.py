"""pads UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.constants import (
    PHRASE_GRID_CELLS,
    PHRASE_PAD_COUNT,
    SONG_OUT_MODES,
)
from pidi.domain.phrases import (
    PHRASE_GAIN_STEP,
    PHRASE_TRIG_LOOP,
    phrase_pad_label,
    phrase_pad_tile_color,
)


class PadsScreenMixin:
    def _build_pads_mode(self) -> None:
        shell = self._pads_shell
        for w in shell.winfo_children():
            w.destroy()
        self._phrase_pad_btns.clear()
        self._phrase_view_btns.clear()
        self._phrase_clear_btn = None
        self._phrase_mode_btn = None
        self._phrase_out_btn = None
        self._phrase_trig_btn = None
        self._phrase_voice_btn = None
        self._phrase_ch_btn = None
        self._phrase_synth_btn = None
        self._phrase_vib_btn = None

        play_view = self._pads_view == "play"

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(6, 2))
        tk.Label(
            header, text="Phrase Pads", font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        view_row = tk.Frame(header, bg="#111111")
        view_row.pack(side=tk.RIGHT)
        for key, label in (("play", "PLAY"), ("edit", "EDIT")):
            btn = self._mk_touch_btn(
                view_row, label, lambda v=key: self._phrase_set_view(v), bg="#3c3836"
            )
            btn.configure(font=("DejaVu Sans", 11, "bold"), padx=10, pady=4)
            btn.pack(side=tk.LEFT, padx=2)
            self._phrase_view_btns[key] = btn

        status = tk.Label(
            shell, textvariable=self._phrase_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(2, 4))

        # Control rows are packed from the bottom *before* the grid, so a short
        # screen shrinks the pad squares instead of pushing the row off-screen.
        if play_view:
            row = tk.Frame(shell, bg="#111111")
            row.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(4, 6))
            self._mk_touch_btn(row, "STOP ALL", self._phrase_stop_all, bg="#3c3836").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10
            )
            self._phrase_out_btn = self._mk_touch_btn(
                row, "OUT: LOCAL", self._phrase_cycle_out_mode, bg="#504945"
            )
            self._phrase_out_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10
            )
        else:
            # Bottom-up: detail row lands under the transport row
            detail = tk.Frame(shell, bg="#111111")
            detail.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(2, 6))
            row = tk.Frame(shell, bg="#111111")
            row.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=(4, 2))
            self._mk_touch_btn(row, "STOP REC", self._phrase_stop_rec, bg="#504945").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_mode_btn = self._mk_touch_btn(
                row, "MODE", self._phrase_toggle_mode_arm, bg="#458588"
            )
            self._phrase_mode_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_clear_btn = self._mk_touch_btn(
                row, "CLEAR", self._phrase_toggle_clear, bg="#9d0006"
            )
            self._phrase_clear_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._mk_touch_btn(row, "STOP ALL", self._phrase_stop_all, bg="#3c3836").pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            # Session routing lives with the transport; the row below is per pad
            self._phrase_out_btn = self._mk_touch_btn(
                row, "OUT: LOCAL", self._phrase_cycle_out_mode, bg="#504945"
            )
            self._phrase_out_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )

            self._phrase_trig_btn = self._mk_touch_btn(
                detail, "TRIG", self._phrase_edit_trig, bg="#458588"
            )
            self._phrase_trig_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_voice_btn = self._mk_touch_btn(
                detail, "FOLLOW", self._phrase_edit_voice, bg="#689d6a"
            )
            self._phrase_voice_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_ch_btn = self._mk_touch_btn(
                detail, "CH:rec", self._phrase_edit_channel, bg="#b16286"
            )
            self._phrase_ch_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_synth_btn = self._mk_touch_btn(
                detail, "SYNTH", self._phrase_edit_synth, bg="#d65d0e"
            )
            self._phrase_synth_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            self._phrase_vib_btn = self._mk_touch_btn(
                detail, "VIB live", self._phrase_edit_vib, bg="#458588"
            )
            self._phrase_vib_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8
            )
            # Narrow trim pair — balance one pad against the rest of the mix
            for label, delta in (("VOL−", -PHRASE_GAIN_STEP), ("VOL+", PHRASE_GAIN_STEP)):
                btn = self._mk_touch_btn(
                    detail, label, lambda d=delta: self._phrase_edit_gain(d), bg="#98971a"
                )
                btn.configure(padx=4)
                btn.pack(side=tk.LEFT, fill=tk.BOTH, padx=2, ipady=8)

        grid = tk.Frame(shell, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=6, pady=2)
        for row_idx in range(4):
            grid.rowconfigure(row_idx, weight=1)
        for col in range(4):
            grid.columnconfigure(col, weight=1)
        for i, cell in enumerate(PHRASE_GRID_CELLS):
            r, c = divmod(i, 4)
            btn = self._mk_touch_btn(
                grid,
                phrase_pad_label(cell),
                lambda idx=cell: self._phrase_pad_tap(idx),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=10)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3)
            self._phrase_pad_btns[cell] = btn

        self._paint_phrase_pads()


    def _phrase_set_view(self, view: str) -> None:
        nxt = "play" if view == "play" else "edit"
        if nxt == self._pads_view and self._phrase_pad_btns:
            return
        if nxt == "play":
            self._phrase_clear_armed = False
            self._phrase_mode_armed = False
            if self._phrases.is_recording():
                self._phrases.stop_record()
        self._pads_view = nxt
        self._mark_settings_dirty()
        self._build_pads_mode()


    def _phrase_pad_tap(self, idx: int) -> None:
        if self._seq_to_pad_armed:
            self._finish_seq_to_pad(idx)
            return
        if self._pads_view == "edit" and self._phrase_mode_armed:
            self._phrases.toggle_trigger_mode(idx)
            self._phrase_mode_armed = False
            self._paint_phrase_pads()
            return
        if self._pads_view == "edit" and self._phrase_clear_armed:
            self._phrases.clear_cell(idx)
            self._phrase_clear_armed = False
            self._paint_phrase_pads()
            return
        self._phrases.handle_pad(
            idx, from_touch=True, allow_record=(self._pads_view == "edit")
        )
        self._paint_phrase_pads()


    def _drum_pad_is_phrase_control(self, cell: int) -> bool:
        """True when a ch10 pad should launch/arm a phrase instead of playing a drum.

        EDIT used to swallow *every* MPK pad while any cell was recording, so the
        only way to fire a filled clip was the touch square. Filled cells still
        launch from hardware; the recording cell (and other empties) stay drums.
        """
        if not (0 <= cell < PHRASE_PAD_COUNT):
            return False
        rec = self._phrases.recording_cell()
        if rec is not None and rec == cell:
            return False
        if rec is not None and self._phrases.cell(cell).is_empty():
            return False
        return True


    def _on_pad_midi(self, cell: int, note: int, velocity: int) -> None:
        """UI-thread handler for an MPK pad that is acting as a phrase trigger."""
        edit_view = self._pads_view == "edit"
        if edit_view and self._phrase_mode_armed:
            mode = self._phrases.toggle_trigger_mode(cell)
            self._phrase_mode_armed = False
            self._paint_phrase_pads()
            self._append_log(
                f"Pad→MODE {phrase_pad_label(cell)} → "
                f"{'LOOP' if mode == PHRASE_TRIG_LOOP else 'ONE-SHOT'}"
            )
            return
        if edit_view and self._phrase_clear_armed:
            self._phrases.clear_cell(cell)
            self._phrase_clear_armed = False
            self._paint_phrase_pads()
            self._append_log(f"Pad→CLEAR {phrase_pad_label(cell)}  note {note}")
            return
        action = self._phrases.handle_pad(
            cell, from_touch=False, allow_record=edit_view
        )
        self._paint_phrase_pads()
        self._append_log(
            f"Pad→Phrase {phrase_pad_label(cell)} ({action})  "
            f"note {note}  vel {velocity}"
        )


    def _phrase_stop_rec(self) -> None:
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        if self._phrases.is_recording():
            self._phrases.stop_record()
        self._paint_phrase_pads()


    def _phrase_stop_all(self) -> None:
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._phrases.stop_all()
        self._paint_phrase_pads()


    def _phrase_toggle_clear(self) -> None:
        """Arm CLEAR: next pad tap erases that cell (cancel by tapping CLEAR again)."""
        if self._pads_view != "edit":
            return
        if self._phrases.is_recording():
            self._phrase_status_var.set("Stop recording before CLEAR")
            return
        self._phrase_mode_armed = False
        self._phrase_clear_armed = not self._phrase_clear_armed
        self._paint_phrase_pads()


    def _phrase_toggle_mode_arm(self) -> None:
        """Arm MODE: next pad tap toggles ONE-SHOT ↔ LOOP."""
        if self._pads_view != "edit":
            return
        if self._phrases.is_recording():
            self._phrase_status_var.set("Stop recording before MODE")
            return
        self._phrase_clear_armed = False
        self._phrase_mode_armed = not self._phrase_mode_armed
        self._paint_phrase_pads()


    def _phrase_selected_or_status(self) -> Optional[int]:
        sel = self._phrases.selected()
        if sel is None:
            self._phrase_status_var.set("Select a pad first (tap a square)")
            return None
        return sel


    def _phrase_edit_trig(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_trigger_mode(sel)
        self._paint_phrase_pads()


    def _phrase_edit_voice(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_voice_lock(sel)
        self._paint_phrase_pads()


    def _phrase_edit_channel(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.cycle_out_channel(sel)
        self._paint_phrase_pads()


    def _phrase_edit_synth(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_local_synth(sel)
        self._paint_phrase_pads()


    def _phrase_edit_vib(self) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.toggle_vib_baked(sel)
        self._paint_phrase_pads()


    def _phrase_edit_gain(self, delta: float) -> None:
        sel = self._phrase_selected_or_status()
        if sel is None:
            return
        self._phrases.nudge_gain(sel, delta)
        self._paint_phrase_pads()


    def _phrase_cycle_out_mode(self) -> None:
        cur = self._phrase_out_mode if self._phrase_out_mode in SONG_OUT_MODES else "local"
        try:
            idx = SONG_OUT_MODES.index(cur)
        except ValueError:
            idx = 0
        nxt = SONG_OUT_MODES[(idx + 1) % len(SONG_OUT_MODES)]
        self._phrase_out_mode = nxt
        if nxt in ("usb", "both"):
            name = self._songs.ensure_outport()
            if name:
                self._append_log(f"Pads MIDI out → {name}")
            else:
                self._append_log("Pads MIDI out: no USB port found")
        self._mark_settings_dirty()
        self._paint_phrase_pads()


    def _paint_phrase_pads(self) -> None:
        clear_armed = bool(self._phrase_clear_armed) and self._pads_view == "edit"
        mode_armed = bool(self._phrase_mode_armed) and self._pads_view == "edit"
        assign_armed = bool(self._seq_to_pad_armed)
        self._phrase_status_var.set(
            self._phrases.status_line(
                clear_armed=clear_armed,
                mode_armed=mode_armed,
                assign_armed=assign_armed,
                view=self._pads_view,
            )
        )
        rec = self._phrases.recording_cell()
        selected = self._phrases.selected()
        playing = set(self._phrases.playing_cells())
        for idx, btn in self._phrase_pad_btns.items():
            cell = self._phrases.cell(idx)
            label = phrase_pad_label(idx)
            loop = cell.is_loop()
            mode_mark = "↻" if loop else "▶"
            lock_mark = "·" if cell.is_voice_locked() else ""
            if mode_armed:
                text = f"{label}\n{mode_mark}?"
                color = phrase_pad_tile_color(empty=cell.is_empty(), mode_armed=True)
            elif assign_armed:
                text = f"{label}\nDROP?"
                color = phrase_pad_tile_color(empty=cell.is_empty(), assign_armed=True)
            elif clear_armed:
                text = f"{label}\nCLR?" if not cell.is_empty() else f"{label}\n—"
                color = phrase_pad_tile_color(
                    empty=cell.is_empty(), clear_armed=True
                )
            elif rec == idx:
                text = f"{label}\nREC"
                color = phrase_pad_tile_color(empty=False, recording=True)
            elif idx in playing:
                secs = cell.length
                text = f"{label}\n{mode_mark}{lock_mark} {secs:.1f}s"
                color = phrase_pad_tile_color(empty=False, playing=True)
            elif cell.is_empty():
                text = f"{label}\n{mode_mark} —" if loop else f"{label}\n—"
                color = phrase_pad_tile_color(
                    empty=True,
                    loop=loop,
                    selected=selected == idx,
                    edit_view=self._pads_view == "edit",
                )
            else:
                text = f"{label}\n{mode_mark}{lock_mark} {cell.length:.1f}s"
                color = phrase_pad_tile_color(
                    empty=False,
                    loop=loop,
                    selected=selected == idx,
                    edit_view=self._pads_view == "edit",
                )
            try:
                btn.configure(text=text, bg=color, activebackground=color)
            except Exception:
                pass

        for key, btn in self._phrase_view_btns.items():
            on = key == self._pads_view
            bg = "#458588" if on else "#3c3836"
            try:
                btn.configure(bg=bg, activebackground=bg)
            except Exception:
                pass

        if self._phrase_out_btn is not None:
            mode = str(self._phrase_out_mode or "local").upper()
            obg = "#689d6a" if mode != "LOCAL" else "#504945"
            try:
                self._phrase_out_btn.configure(
                    text=f"OUT: {mode}",
                    bg=obg,
                    activebackground=obg,
                )
            except Exception:
                pass

        if self._phrase_clear_btn is not None:
            cbg = "#fb4934" if clear_armed else "#9d0006"
            try:
                self._phrase_clear_btn.configure(
                    text="CLEAR…" if clear_armed else "CLEAR",
                    bg=cbg,
                    activebackground=cbg,
                )
            except Exception:
                pass
        if self._phrase_mode_btn is not None:
            mbg = "#d3869b" if mode_armed else "#458588"
            try:
                self._phrase_mode_btn.configure(
                    text="MODE…" if mode_armed else "MODE",
                    bg=mbg,
                    activebackground=mbg,
                )
            except Exception:
                pass

        if selected is not None:
            cell = self._phrases.cell(selected)
            if self._phrase_trig_btn is not None:
                t = "LOOP" if cell.is_loop() else "1SHOT"
                try:
                    self._phrase_trig_btn.configure(text=t)
                except Exception:
                    pass
            if self._phrase_voice_btn is not None:
                trim = int(round(cell.gain * 100))
                v = f"LOCK {trim}%" if cell.is_voice_locked() else "FOLLOW"
                if not cell.is_voice_locked() and trim != 100:
                    v = f"FOLLOW {trim}%"
                vbg = "#b16286" if cell.is_voice_locked() else "#689d6a"
                try:
                    self._phrase_voice_btn.configure(
                        text=v, bg=vbg, activebackground=vbg
                    )
                except Exception:
                    pass
            if self._phrase_ch_btn is not None:
                ch = "CH:rec" if cell.out_channel < 0 else f"CH:{cell.out_channel + 1}"
                try:
                    self._phrase_ch_btn.configure(text=ch)
                except Exception:
                    pass
            if self._phrase_synth_btn is not None:
                s = "SYNTH" if cell.local_synth else "MIDI"
                sbg = "#d65d0e" if cell.local_synth else "#504945"
                try:
                    self._phrase_synth_btn.configure(
                        text=s, bg=sbg, activebackground=sbg
                    )
                except Exception:
                    pass
            if self._phrase_vib_btn is not None:
                vbg2 = "#458588" if cell.vib_baked else "#3c3836"
                try:
                    self._phrase_vib_btn.configure(
                        text=f"VIB {cell.vib_label()}", bg=vbg2, activebackground=vbg2
                    )
                except Exception:
                    pass


    def _refresh_phrase_status(self) -> None:
        if self._mode == "pads":
            self._paint_phrase_pads()
        else:
            self._phrase_status_var.set(
                self._phrases.status_line(
                    clear_armed=self._phrase_clear_armed,
                    mode_armed=self._phrase_mode_armed,
                    assign_armed=self._seq_to_pad_armed,
                    view=self._pads_view,
                )
            )
