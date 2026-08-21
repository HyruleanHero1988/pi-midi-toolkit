"""seq UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.domain.phrases import PHRASE_TRIG_LOOP, phrase_pad_label
from pidi.sequencer import SEQ_EMPTY, SEQ_OVERDUB, SEQ_REC_BACKBONE


class SeqScreenMixin:
    def _build_seq_mode(self) -> None:
        shell = self._seq_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="Sequencer", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="drums + keys · free timing · overdub layers",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            shell, textvariable=self._seq_status_var,
            font=("DejaVu Sans", 14, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(6, 2))

        layers = tk.Label(
            shell, textvariable=self._seq_layer_var,
            font=("DejaVu Sans Mono", 11),
            fg="#83a598", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        layers.pack(fill=tk.X, padx=10, pady=(0, 6))

        # Button rows claim their strips first, so REC/PLAY keep full height and
        # the how-to line at the very end is the only thing a short panel drops.
        row4 = tk.Frame(shell, bg="#111111")
        row4.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(4, 2))
        self._mk_touch_btn(row4, "STOP ALL", self._seq_stop, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._seq_to_pad_btn = self._mk_touch_btn(
            row4, "→ PAD", self._seq_assign_to_pad, bg="#458588"
        )
        self._seq_to_pad_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10)
        self._mk_touch_btn(row4, "CLEAR", self._seq_clear, bg="#3c3836").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._mk_touch_btn(row4, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )

        row3 = tk.Frame(shell, bg="#111111")
        row3.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=4)
        self._mk_touch_btn(row3, "LEN ×2", self._seq_double, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._mk_touch_btn(row3, "LEN ÷2", self._seq_halve, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10
        )
        self._seq_extend_btn = self._mk_touch_btn(
            row3, "OVERDUB: WRAP", self._seq_toggle_extend, bg="#3c3836"
        )
        self._seq_extend_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=10)

        row2 = tk.Frame(shell, bg="#111111")
        row2.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=4)
        self._seq_keep_btn = self._mk_touch_btn(row2, "KEEP", self._seq_keep, bg="#458588")
        self._seq_keep_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)
        self._seq_drop_btn = self._mk_touch_btn(row2, "DROP", self._seq_drop, bg="#3c3836")
        self._seq_drop_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)
        self._seq_undo_btn = self._mk_touch_btn(row2, "UNDO", self._seq_undo, bg="#3c3836")
        self._seq_undo_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4, ipady=12)

        # Transport — the two buttons you hit while playing
        row1 = tk.Frame(shell, bg="#111111")
        row1.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)
        self._seq_rec_btn = self._mk_touch_btn(
            row1, "REC BACKBONE", self._seq_toggle_record, bg="#9d0006"
        )
        self._seq_rec_btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22)
        self._seq_rec_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        self._seq_play_btn = self._mk_touch_btn(
            row1, "PLAY", self._seq_toggle_play, bg="#689d6a"
        )
        self._seq_play_btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22)
        self._seq_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)

        tip = tk.Label(
            shell,
            text=(
                "1) REC + play the groove → REC again locks the loop length  "
                "2) it loops; REC again to overdub drums or keys  "
                "3) KEEP flattens the layer, DROP throws it away, UNDO peels the last one off  "
                "4) → PAD then tap a square or MPK pad to drop the sequence onto a phrase clip"
            ),
            font=("DejaVu Sans", 10), fg="#83a598", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        tip.pack(side=tk.BOTTOM, fill=tk.X, padx=10, pady=(2, 6))
        self._paint_seq_buttons()


    def _refresh_seq_status(self) -> None:
        self._seq_status_var.set(self._seq.status_line())
        self._seq_layer_var.set(self._seq.layer_line())
        self._paint_seq_buttons()


    def _paint_seq_buttons(self) -> None:
        st = self._seq.status()
        state = str(st["state"])
        pending = int(st["pending"]) > 0 or state == SEQ_OVERDUB
        if self._seq_rec_btn is not None:
            if state == SEQ_REC_BACKBONE:
                text, color = "● STOP REC", "#cc241d"
            elif state == SEQ_OVERDUB:
                text, color = "● STOP OVERDUB", "#cc241d"
            elif state == SEQ_EMPTY:
                text, color = "REC BACKBONE", "#9d0006"
            else:
                text, color = "REC OVERDUB", "#9d0006"
            self._seq_rec_btn.configure(text=text, bg=color, activebackground=color)
        if self._seq_play_btn is not None:
            if self._seq.is_playing():
                self._seq_play_btn.configure(
                    text="■ STOP", bg="#d79921", activebackground="#d79921"
                )
            elif state == SEQ_EMPTY:
                # Nothing to run yet — REC should be the only lit way forward
                self._seq_play_btn.configure(
                    text="PLAY", bg="#3c3836", activebackground="#3c3836"
                )
            else:
                self._seq_play_btn.configure(
                    text="PLAY", bg="#689d6a", activebackground="#689d6a"
                )
        for btn, live in (
            (self._seq_keep_btn, pending),
            (self._seq_drop_btn, pending),
            (self._seq_undo_btn, int(st["layers"]) > 1),
        ):
            if btn is None:
                continue
            base = "#458588" if btn is self._seq_keep_btn else "#665c54"
            color = base if live else "#3c3836"
            btn.configure(bg=color, activebackground=color)
        if self._seq_extend_btn is not None:
            on = bool(st["extend"])
            color = "#b16286" if on else "#3c3836"
            self._seq_extend_btn.configure(
                text="OVERDUB: EXTEND" if on else "OVERDUB: WRAP",
                bg=color,
                activebackground=color,
            )
        if self._seq_to_pad_btn is not None:
            armed = bool(self._seq_to_pad_armed)
            color = "#83a598" if armed else "#458588"
            self._seq_to_pad_btn.configure(
                text="→ PAD…" if armed else "→ PAD",
                bg=color,
                activebackground=color,
            )


    def _seq_toggle_record(self) -> None:
        action = self._seq.toggle_record()
        if action == "backbone" and self._mode != "seq":
            self._switch_mode("seq")
        self._q_put(("seq",))
        self._refresh_seq_status()


    def _seq_toggle_play(self) -> None:
        if self._seq.is_playing():
            self._seq.stop_playback()
        elif not self._seq.start_playback():
            self._q_put(("log", "SEQ empty — record a backbone first", False))
        self._q_put(("seq",))
        self._refresh_seq_status()


    def _seq_keep(self) -> None:
        self._seq.keep()
        self._refresh_seq_status()


    def _seq_drop(self) -> None:
        self._seq.drop()
        self._refresh_seq_status()


    def _seq_undo(self) -> None:
        self._seq.undo()
        self._refresh_seq_status()


    def _seq_double(self) -> None:
        if not self._seq.double_length():
            self._q_put(("log", "SEQ length unchanged (max 8 cycles)", False))
        self._refresh_seq_status()


    def _seq_halve(self) -> None:
        if not self._seq.halve_length():
            self._q_put(("log", "SEQ length unchanged — a layer is that long", False))
        self._refresh_seq_status()


    def _seq_toggle_extend(self) -> None:
        self._seq.toggle_extend()
        self._refresh_seq_status()


    def _seq_stop(self) -> None:
        self._seq_to_pad_armed = False
        self._seq.stop()
        self._q_put(("seq",))
        self._refresh_seq_status()


    def _seq_clear(self) -> None:
        self._seq_to_pad_armed = False
        self._seq.clear()
        self._refresh_seq_status()


    def _seq_assign_to_pad(self) -> None:
        """Arm SEQ → PAD: next phrase-pad tap (touch or MPK) receives this take as a LOOP."""
        if self._seq.is_recording():
            self._seq.toggle_record()
        events, length = self._seq.snapshot()
        if not events or length <= 0.0:
            self._seq_status_var.set("Nothing to assign — record a backbone first.")
            self._seq_to_pad_armed = False
            self._paint_seq_buttons()
            return
        self._phrase_clear_armed = False
        self._phrase_mode_armed = False
        self._seq_to_pad_armed = not self._seq_to_pad_armed
        self._refresh_seq_status()
        if self._seq_to_pad_armed:
            self._pads_view = "edit"
            self._switch_mode("pads")
            self._build_pads_mode()
            self._paint_phrase_pads()
            self._phrase_status_var.set(
                "SEQ → PAD — tap a pad (touch or MPK) to drop the sequence (overwrites that pad)"
            )
            self._append_log("SEQ → PAD armed — tap a phrase pad")


    def _finish_seq_to_pad(self, idx: int) -> None:
        events, length = self._seq.snapshot()
        self._seq_to_pad_armed = False
        if not events or length <= 0.0:
            self._phrase_status_var.set("Sequence was empty — assignment cancelled")
            self._paint_phrase_pads()
            self._paint_seq_buttons()
            return
        ok = self._phrases.load_from_events(
            idx, events, length, trigger_mode=PHRASE_TRIG_LOOP
        )
        if ok:
            self._append_log(
                f"SEQ → {phrase_pad_label(idx)} ({len(events)} ev, {length:.2f}s LOOP)"
            )
            # PLAY so the same drum pad now launches the clip
            self._pads_view = "play"
            self._build_pads_mode()
            self._phrase_status_var.set(
                f"Loaded SEQ → {phrase_pad_label(idx)} as LOOP — hit that pad to trigger it"
            )
        else:
            self._phrase_status_var.set("Could not assign sequence to that pad")
            self._paint_phrase_pads()
        self._paint_seq_buttons()
