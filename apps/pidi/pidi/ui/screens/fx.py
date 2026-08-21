"""fx UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.constants import UI_MODES


class FxScreenMixin:
    def _paint_full_vel_btn(self) -> None:
        if self._full_vel_btn is None:
            return
        if self._full_vel:
            self._full_vel_btn.configure(
                text="FULL VEL: ON", bg="#689d6a", activebackground="#689d6a"
            )
        else:
            self._full_vel_btn.configure(
                text="FULL VEL: OFF", bg="#3c3836", activebackground="#3c3836"
            )


    def _paint_drum_lock_btn(self) -> None:
        if self._drum_lock_btn is None:
            return
        if self.engine.drum_mode():
            self._drum_lock_btn.configure(
                text="DRUM MODE: ON", bg="#d79921", activebackground="#d79921"
            )
        else:
            self._drum_lock_btn.configure(
                text="DRUM MODE", bg="#3c3836", activebackground="#3c3836"
            )


    def _paint_fx_mode_btn(self) -> None:
        if self._fx_mode_btn is None:
            return
        if self.engine.fx_mode():
            self._fx_mode_btn.configure(
                text="FX MODE: ON", bg="#b16286", activebackground="#b16286"
            )
        else:
            self._fx_mode_btn.configure(
                text="FX MODE", bg="#3c3836", activebackground="#3c3836"
            )


    def _paint_bus_fx_mode_btn(self) -> None:
        if self._bus_fx_mode_btn is None:
            return
        if self.engine.bus_fx_mode():
            self._bus_fx_mode_btn.configure(
                text="BUS FX: ON", bg="#8f3f71", activebackground="#8f3f71"
            )
        else:
            self._bus_fx_mode_btn.configure(
                text="BUS FX", bg="#3c3836", activebackground="#3c3836"
            )


    def _toggle_drum_lock(self) -> None:
        self.engine.set_drum_mode(not self.engine.drum_mode())
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log(
            "DRUM MODE ON — Knob 1–4 edit drums"
            if self.engine.drum_mode()
            else "DRUM MODE OFF — Knob 1 is morph again"
        )
        if self._kit_ui_open:
            if getattr(self, "_kit_view", "grid") == "wave":
                self._paint_kit_waveform(force=True)
            else:
                self._refresh_kit_status()
        else:
            self._paint_synth_waveform(force=True)


    def _toggle_fx_mode(self) -> None:
        # Already editing inserts but left the FX screen (e.g. opened KIT) → reopen
        if self.engine.fx_mode() and not self._fx_ui_open:
            if self._kit_ui_open:
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            else:
                self.engine.set_fx_edit_voice(None)
            self.mod_var.set(self._format_mod_line())
            self._open_fx_panel()
            return
        on = self.engine.toggle_fx_mode()
        if on:
            # KIT open → edit that drum's insert; else nearer morph wavetable.
            if self._kit_ui_open:
                self.engine.set_fx_edit_drum(self._kit_model_selected())
            else:
                self.engine.set_fx_edit_voice(None)
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        target = self.engine.fx_edit_label() if on else ""
        self._append_log(
            f"FX MODE ON — insert FX on {target} "
            "(not the whole mix). KIT → ALL DRUMS for kit echo; tap a pad for one drum; "
            "close KIT for nearer morph voice. Use BUS FX for global wet."
            if on
            else "FX MODE OFF — knobs back to morph / tone / …"
        )
        if on:
            self._open_fx_panel()
        else:
            self._close_fx_panel()


    def _toggle_bus_fx_mode(self) -> None:
        if self.engine.bus_fx_mode() and not self._fx_ui_open:
            self.engine.set_fx_edit_bus()
            self.mod_var.set(self._format_mod_line())
            self._open_fx_panel()
            return
        on = self.engine.toggle_bus_fx_mode()
        if on:
            self.engine.set_fx_edit_bus()
        self._paint_bus_fx_mode_btn()
        self._paint_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log(
            "BUS FX ON — knobs wet the whole soft-synth mix (keys + drums + phrases). "
            "Per-voice/per-drum inserts still run underneath; use FX MODE to edit those."
            if on
            else "BUS FX OFF — knobs back to morph / tone / …"
        )
        if on:
            self._open_fx_panel()
        else:
            self._close_fx_panel()


    def _fx_param_snapshot_lines(self) -> List[str]:
        """Human-readable FX param dump for log / panel."""
        st = self.engine.modulation_state()
        delay_ms = int((0.05 + float(st.get("fx_delay_time", 0.0)) * 0.70) * 1000)
        drive = float(st.get("fx_drive", 0.0))
        fb = float(st.get("fx_delay_fb", 0.0))
        dmix = float(st.get("fx_delay_mix", 0.0))
        rsize = float(st.get("fx_reverb_size", 0.0))
        rmix = float(st.get("fx_reverb_mix", 0.0))
        return [
            f"K1 Drive       {int(drive * 127):3d}  ({drive:.2f})",
            f"K2 Delay       {delay_ms:3d} ms  ({float(st.get('fx_delay_time', 0.0)):.2f})",
            f"K3 Feedback    {int(fb * 127):3d}  ({fb:.2f})",
            f"K4 Delay mix   {int(dmix * 127):3d}  ({dmix:.2f})",
            f"K5 Reverb size {int(rsize * 127):3d}  ({rsize:.2f})",
            f"K6 Reverb mix  {int(rmix * 127):3d}  ({rmix:.2f})",
            f"K8 Synth lvl   {int(float(st.get('synth_level', st.get('level', 1.0))) * 127):3d}",
        ]


    def _open_fx_panel(self) -> None:
        """Dedicated FX knob readout — live values for insert or bus FX."""
        if not self.engine.fx_knob_focus():
            return
        if self._fx_ui_open:
            self._refresh_fx_panel()
            return

        # Close competing overlays so the FX values stay readable
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._save_preset_open:
            self._close_save_preset(restore_main=False)
        if self._power_ui_open:
            self._close_power_menu(restore_main=False)

        self._fx_ui_open = True
        self._fx_prev_mode = self._mode
        for shell in (
            self._synth_shell,
            self._seq_shell,
            self._pads_shell,
            self._songs_shell,
            self._presets_shell,
            self._log_shell,
        ):
            try:
                shell.pack_forget()
            except Exception:
                pass

        self._fx_frame = tk.Frame(self._mode_host, bg="#111111")
        self._fx_frame.pack(fill=tk.BOTH, expand=True)

        header, body, footer = self._pack_screen_regions(
            self._fx_frame,
            header_padx=10,
            header_pady=(12, 4),
            body_padx=10,
            body_pady=4,
            footer_padx=10,
            footer_pady=10,
        )

        self._fx_title_var = tk.StringVar(value="")
        self._fx_target_var = tk.StringVar(value="")
        tk.Label(
            header,
            textvariable=self._fx_title_var,
            font=("DejaVu Sans", 20, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            textvariable=self._fx_target_var,
            font=("DejaVu Sans", 13),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        # Leave via global ← (footer reserved then forgotten for more body space).
        footer.pack_forget()

        self._fx_value_vars = {}
        rows = (
            ("drive", "K1", "Drive"),
            ("delay", "K2", "Delay"),
            ("fb", "K3", "Feedback"),
            ("dmix", "K4", "Delay mix"),
            ("rsize", "K5", "Reverb size"),
            ("rmix", "K6", "Reverb mix"),
            ("syn", "K8", "Synth lvl"),
        )
        grid = tk.Frame(body, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True)
        for i, (key, knob, name) in enumerate(rows):
            grid.rowconfigure(i, weight=1, uniform="fx")
            tk.Label(
                grid,
                text=knob,
                font=("DejaVu Sans", 14, "bold"),
                fg="#83a598",
                bg="#111111",
                width=4,
                anchor="w",
            ).grid(row=i, column=0, sticky="nsw", padx=(2, 8))
            tk.Label(
                grid,
                text=name,
                font=("DejaVu Sans", 15),
                fg="#ebdbb2",
                bg="#111111",
                anchor="w",
            ).grid(row=i, column=1, sticky="nsw", padx=(0, 12))
            var = tk.StringVar(value="—")
            self._fx_value_vars[key] = var
            tk.Label(
                grid,
                textvariable=var,
                font=("DejaVu Sans", 16, "bold"),
                fg="#fabd2f",
                bg="#111111",
                anchor="e",
            ).grid(row=i, column=2, sticky="nse", padx=(0, 4))
        grid.columnconfigure(1, weight=1)
        grid.columnconfigure(2, weight=1)

        self._refresh_fx_panel()
        for line in self._fx_param_snapshot_lines():
            self._append_log(f"FX  {line}")
        self._paint_nav_back()


    def _refresh_fx_panel(self) -> None:
        if not self._fx_ui_open:
            return
        st = self.engine.modulation_state()
        bus = self.engine.bus_fx_mode()
        if self._fx_title_var is not None:
            self._fx_title_var.set("BUS FX" if bus else "FX MODE")
        if self._fx_target_var is not None:
            if bus:
                self._fx_target_var.set("whole mix")
            else:
                self._fx_target_var.set(self.engine.fx_edit_label())

        delay_ms = int((0.05 + float(st.get("fx_delay_time", 0.0)) * 0.70) * 1000)
        vals = {
            "drive": (
                f"{int(float(st.get('fx_drive', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_drive', 0.0)):.2f})"
            ),
            "delay": f"{delay_ms:3d} ms",
            "fb": (
                f"{int(float(st.get('fx_delay_fb', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_delay_fb', 0.0)):.2f})"
            ),
            "dmix": (
                f"{int(float(st.get('fx_delay_mix', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_delay_mix', 0.0)):.2f})"
            ),
            "rsize": (
                f"{int(float(st.get('fx_reverb_size', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_reverb_size', 0.0)):.2f})"
            ),
            "rmix": (
                f"{int(float(st.get('fx_reverb_mix', 0.0)) * 127):3d}   "
                f"({float(st.get('fx_reverb_mix', 0.0)):.2f})"
            ),
            "syn": f"{int(float(st.get('synth_level', st.get('level', 1.0))) * 127):3d}",
        }
        for key, text in vals.items():
            var = self._fx_value_vars.get(key)
            if var is not None:
                var.set(text)


    def _exit_fx_panel(self) -> None:
        """CLOSE on FX screen — leave FX edit modes and restore previous view."""
        if self.engine.fx_mode():
            self.engine.set_fx_mode(False)
        if self.engine.bus_fx_mode():
            self.engine.set_bus_fx_mode(False)
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._paint_drum_lock_btn()
        self.mod_var.set(self._format_mod_line())
        self._append_log("FX panel closed — knobs back to morph / tone / …")
        self._close_fx_panel()


    def _close_fx_panel(self, restore_main: bool = True) -> None:
        if not self._fx_ui_open:
            return
        prev = getattr(self, "_fx_prev_mode", "synth")
        if self._fx_frame is not None:
            try:
                self._fx_frame.destroy()
            except Exception:
                pass
            self._fx_frame = None
        self._fx_ui_open = False
        self._fx_title_var = None
        self._fx_target_var = None
        self._fx_value_vars = {}
        if restore_main:
            self._switch_mode(
                prev
                if prev in UI_MODES
                else "synth"
            )
        self._paint_nav_back()
