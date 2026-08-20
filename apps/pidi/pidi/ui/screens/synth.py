"""synth UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido
import numpy as np

from pidi.constants import (
    SCOPE_CRT_WAVE,
    SCOPE_MORPH_CYCLES,
    SCOPE_REDRAW_DEBOUNCE_S,
    SCOPE_REDRAW_MAX_WAIT_S,
)
from pidi.ui.scope import blank_waveform_on_canvas, draw_waveform_on_canvas


class SynthScreenMixin:
    def _on_synth_scope_configure(self, _event: object = None) -> None:
        """Debounce Tk Configure storms (fullscreen/layout) so we don't paint every frame."""
        canvas = self._wave_canvas
        if canvas is None:
            return
        try:
            size = (int(canvas.winfo_width()), int(canvas.winfo_height()))
        except Exception:
            return
        if size[0] < 8 or size[1] < 8:
            return
        if size == getattr(self, "_synth_scope_size", None):
            return
        self._synth_scope_size = size
        self._schedule_scope_paint("synth", blank=False)


    def _on_kit_scope_configure(self, _event: object = None) -> None:
        canvas = self._kit_wave_canvas
        if canvas is None:
            return
        try:
            size = (int(canvas.winfo_width()), int(canvas.winfo_height()))
        except Exception:
            return
        if size[0] < 8 or size[1] < 8:
            return
        if size == getattr(self, "_kit_scope_size", None):
            return
        self._kit_scope_size = size
        self._schedule_scope_paint("drum", blank=False)


    def _active_scope_canvas(self) -> Optional[tk.Canvas]:
        if self._kit_ui_open and self._kit_wave_canvas is not None:
            return self._kit_wave_canvas
        if self._mode == "synth" and not self._overlay_busy():
            return self._wave_canvas
        return None


    def _schedule_scope_paint(self, which: str = "synth", *, blank: bool = True) -> None:
        """Mark only the synth or drum scope dirty (shape changes only)."""
        if which == "drum":
            self._scope_dirty_drum = True
        else:
            self._scope_dirty_synth = True
        self._arm_scope_paint(blank=blank)


    def _arm_scope_paint(self, *, blank: bool = True) -> None:
        """Blank dirty CRT(s) immediately; coalesce the expensive redraw."""
        now = time.monotonic()
        if blank:
            if (
                self._scope_dirty_synth
                and self._wave_canvas is not None
                and self._mode == "synth"
                and not self._overlay_busy()
                and not self._scope_blanked_synth
            ):
                blank_waveform_on_canvas(self._wave_canvas)
                self._scope_blanked_synth = True
                self._scope_blanked = True
            if (
                self._scope_dirty_drum
                and self._kit_ui_open
                and self._kit_wave_canvas is not None
                and getattr(self, "_kit_view", "grid") == "wave"
                and not self._scope_blanked_drum
            ):
                blank_waveform_on_canvas(self._kit_wave_canvas)
                self._scope_blanked_drum = True
                self._scope_blanked = True
        if not self._scope_needs_paint:
            self._scope_first_dirty = now
        self._scope_needs_paint = True
        self._scope_paint_at = min(
            now + SCOPE_REDRAW_DEBOUNCE_S,
            getattr(self, "_scope_first_dirty", now) + SCOPE_REDRAW_MAX_WAIT_S,
        )


    def _flush_scope_paint(self, *, force: bool = False) -> None:
        self._scope_needs_paint = False
        paint_synth = self._scope_dirty_synth
        paint_drum = self._scope_dirty_drum
        self._scope_dirty_synth = False
        self._scope_dirty_drum = False
        if paint_drum:
            self._scope_blanked_drum = False
            self._paint_kit_waveform(force=force)
        if paint_synth:
            self._scope_blanked_synth = False
            if self._mode == "synth" and not self._overlay_busy():
                self._paint_synth_waveform(force=force)
        if not self._scope_blanked_synth and not self._scope_blanked_drum:
            self._scope_blanked = False


    def _paint_synth_waveform(self, *, force: bool = False) -> None:
        canvas = self._wave_canvas
        if canvas is None:
            return
        if self._mode != "synth" or self._overlay_busy():
            return
        try:
            samples = self.engine.morph_cycle_copy()
            if SCOPE_MORPH_CYCLES > 1 and samples is not None and len(samples) > 0:
                samples = np.tile(samples, SCOPE_MORPH_CYCLES)
            draw_waveform_on_canvas(
                canvas,
                samples,
                color=SCOPE_CRT_WAVE,
                redraw_grid=force,
            )
            self._scope_blanked_synth = False
            self._scope_blanked = self._scope_blanked_drum
            self._scope_dirty_synth = False
            if self._wave_caption is not None:
                a, b, blend = self.engine.morph_neighbors()
                if a == b:
                    cap = f"Morph · {a}"
                else:
                    cap = f"Morph · {a} → {b}  {int(blend * 100)}%"
                self._wave_caption.configure(text=cap)
        except Exception:
            if force:
                pass
