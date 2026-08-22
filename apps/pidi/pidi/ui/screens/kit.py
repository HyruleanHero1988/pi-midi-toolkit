"""kit UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.audio.drums import drum_model_for_note, mpk_note_for_phrase_cell
from pidi.constants import (
    DRUM_CHANNEL,
    DRUM_SCOPE_SEC,
    PHRASE_GRID_CELLS,
    PHRASE_PAD_BASE,
    SCOPE_CRT_BG,
    SCOPE_CRT_WAVE,
)
from pidi.domain.phrases import phrase_pad_label
from pidi.ui.scope import draw_waveform_on_canvas


class KitScreenMixin:
    def _kit_model_selected(self) -> str:
        return drum_model_for_note(self._kit_selected_note)


    def _kit_pad_caption(self, cell: int, note: int) -> str:
        """Short readable pad label for the full-height kit grid."""
        model = drum_model_for_note(note).replace("_", " ")
        return f"{phrase_pad_label(cell)}\n{model}"


    def _paint_kit_waveform(self, *, force: bool = False) -> None:
        canvas = self._kit_wave_canvas
        if canvas is None or not self._kit_ui_open:
            self._refresh_kit_status()
            return
        if getattr(self, "_kit_view", "grid") != "wave":
            self._refresh_kit_status()
            return
        try:
            samples = self.engine.preview_drum_waveform(self._kit_model_selected())
            draw_waveform_on_canvas(
                canvas,
                samples,
                color=SCOPE_CRT_WAVE,
                duration_sec=DRUM_SCOPE_SEC,
                redraw_grid=force,
            )
            self._scope_blanked_drum = False
            self._scope_blanked = self._scope_blanked_synth
            self._scope_dirty_drum = False
            self._refresh_kit_status()
        except Exception as exc:
            print(f"kit wave paint failed: {exc}", flush=True)


    def _paint_kit_pad_btns(self) -> None:
        for note, btn in self._kit_btns.items():
            on = (not self._kit_all_drums) and note == self._kit_selected_note
            color = "#d79921" if on else "#3c3836"
            try:
                btn.configure(bg=color, activebackground=color)
            except Exception:
                pass
        if self._kit_all_btn is not None:
            color = "#b16286" if self._kit_all_drums else "#504945"
            try:
                self._kit_all_btn.configure(bg=color, activebackground=color)
            except Exception:
                pass


    def _select_kit_all_drums(self) -> None:
        """Point FX MODE at the shared kit-group bus (echo on all drums)."""
        self._kit_all_drums = True
        if not self.engine.fx_mode():
            self.engine.set_fx_mode(True)
            self._paint_fx_mode_btn()
            self._paint_bus_fx_mode_btn()
            self._paint_drum_lock_btn()
        self.engine.set_fx_edit_drums()
        self._paint_kit_pad_btns()
        self.mod_var.set(self._format_mod_line())
        self._refresh_kit_status()
        self._append_log("KIT — ALL DRUMS FX (shared kit bus)")


    def _select_kit_note(self, note: int, *, audition: bool = False) -> None:
        note = int(note) & 0x7F
        if note < PHRASE_PAD_BASE or note >= PHRASE_PAD_BASE + 16:
            return
        self._kit_selected_note = note
        self._kit_all_drums = False
        self._paint_kit_pad_btns()
        if self.engine.fx_mode():
            self.engine.set_fx_edit_drum(drum_model_for_note(note))
            self.mod_var.set(self._format_mod_line())
        if getattr(self, "_kit_view", "grid") == "wave":
            self._paint_kit_waveform(force=True)
        else:
            self._refresh_kit_status()
        if audition:
            self.engine.note_on(DRUM_CHANNEL, note, 110)
            self._q_put(("log", f"Kit play {drum_model_for_note(note)}", False))


    def _kit_audition_selected(self) -> None:
        self._select_kit_note(self._kit_selected_note, audition=True)


    def _set_kit_view(self, view: str) -> None:
        nxt = "wave" if view == "wave" else "grid"
        if nxt == getattr(self, "_kit_view", "grid"):
            if nxt == "grid" or self._kit_wave_canvas is not None:
                return
        self._kit_view = nxt
        if self._kit_ui_open:
            self._rebuild_kit_ui()


    def _open_kit_explorer(self) -> None:
        """Kit grid picker; WAVE drills into a CRT scope for the selected drum."""
        if self._kit_ui_open:
            return
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._mode != "synth":
            self._switch_mode("synth")
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)

        # If insert FX MODE is on, keep drum-group target or point at selected drum.
        # If BUS FX is on, keep global edit (kit is audition/preview only).
        # Otherwise turn on DRUM MODE so knobs reshape the one-shot body.
        if self.engine.fx_mode():
            if self.engine.fx_edit_kind() == "drums":
                self._kit_all_drums = True
            else:
                self._kit_all_drums = False
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            self.mod_var.set(self._format_mod_line())
        elif self.engine.bus_fx_mode():
            self.mod_var.set(self._format_mod_line())
        elif not self.engine.drum_mode():
            self.engine.set_drum_mode(True)
            self._paint_drum_lock_btn()
            self.mod_var.set(self._format_mod_line())

        self._kit_ui_open = True
        self._kit_view = "grid"
        self._synth_shell.pack_forget()
        self._kit_frame = tk.Frame(self._mode_host, bg="#111111")
        self._kit_frame.pack(fill=tk.BOTH, expand=True)
        self._rebuild_kit_ui()
        if self.engine.fx_mode():
            self._append_log(
                "KIT — ALL DRUMS = shared kit FX; tap a pad · WAVE for scope"
            )
        else:
            self._append_log("KIT — tap a drum to play; knobs reshape it · WAVE for scope")


    def _rebuild_kit_ui(self) -> None:
        """Build grid or wave drill-down inside the kit frame (footer-first)."""
        frame = self._kit_frame
        if frame is None:
            return
        for w in frame.winfo_children():
            w.destroy()
        self._kit_btns = {}
        self._kit_all_btn = None
        self._kit_wave_canvas = None

        wave_view = getattr(self, "_kit_view", "grid") == "wave"
        header, body, footer = self._pack_screen_regions(
            frame,
            header_padx=6,
            header_pady=(6, 2),
            body_padx=4,
            body_pady=2,
            footer_padx=6,
            footer_pady=6,
        )

        title = "DRUM WAVE" if wave_view else "DRUM KIT"
        hint = (
            "knobs reshape · PLAY to hear"
            if wave_view
            else "tap pad to play · knobs reshape"
        )
        tk.Label(
            header, text=title, font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text=hint, font=("DejaVu Sans", 11),
            fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        if wave_view:
            self._mk_touch_btn(
                footer, "PLAY", self._kit_audition_selected, bg="#458588"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        else:
            self._kit_all_btn = self._mk_touch_btn(
                footer, "ALL DRUMS", self._select_kit_all_drums, bg="#504945"
            )
            self._kit_all_btn.pack(
                side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12
            )
            self._mk_touch_btn(
                footer, "WAVE", lambda: self._set_kit_view("wave"), bg="#689d6a"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)

        status = tk.Label(
            body,
            textvariable=self._kit_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        )
        status.pack(side=tk.TOP, fill=tk.X, padx=4, pady=(0, 4))

        if wave_view:
            self._kit_wave_canvas = tk.Canvas(
                body,
                bg=SCOPE_CRT_BG,
                highlightthickness=1,
                highlightbackground="#14532d",
                bd=0,
            )
            self._kit_wave_canvas.pack(
                side=tk.TOP, fill=tk.BOTH, expand=True, padx=4, pady=2
            )
            self._kit_scope_size = None
            self._kit_wave_canvas.bind("<Configure>", self._on_kit_scope_configure)
            self._paint_kit_waveform(force=True)
            try:
                self.root.after_idle(lambda: self._paint_kit_waveform(force=True))
            except Exception:
                pass
        else:
            grid = tk.Frame(body, bg="#111111")
            grid.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            for r in range(4):
                grid.rowconfigure(r, weight=1)
            for c in range(4):
                grid.columnconfigure(c, weight=1)
            for i, cell in enumerate(PHRASE_GRID_CELLS):
                note = mpk_note_for_phrase_cell(cell)
                r, c = divmod(i, 4)
                btn = self._mk_touch_btn(
                    grid,
                    self._kit_pad_caption(cell, note),
                    lambda n=note: self._select_kit_note(n, audition=True),
                    bg="#3c3836",
                )
                btn.configure(font=("DejaVu Sans", 14, "bold"), pady=4)
                btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3)
                self._kit_btns[note] = btn
            self._paint_kit_pad_btns()
            self._refresh_kit_status()
        self._paint_nav_back()


    def _close_kit_explorer(self, restore_main: bool = True) -> None:
        if not self._kit_ui_open:
            return
        if self._kit_frame is not None:
            self._kit_frame.destroy()
            self._kit_frame = None
        self._kit_btns = {}
        self._kit_all_btn = None
        self._kit_wave_canvas = None
        self._kit_view = "grid"
        self._kit_ui_open = False
        # Leaving KIT: keep shared-kit FX edit; single-drum insert → nearer morph voice.
        if self.engine.fx_mode():
            if self.engine.fx_edit_kind() == "drum":
                self._kit_all_drums = False
                self.engine.set_fx_edit_voice(None)
            # kind == "drums" stays so you can keep twisting kit echo after CLOSE
        if restore_main and self._mode == "synth":
            self._synth_shell.pack(fill=tk.BOTH, expand=True)
            self._main.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
            self._touch.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
            self.mod_var.set(self._format_mod_line())
            self._paint_synth_waveform(force=True)
        self._paint_nav_back()
