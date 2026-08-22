"""presets UI mixin for MidiToneApp."""
from __future__ import annotations

import json
import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido

from pidi.constants import PRESETS_DIR, PRESET_SLOTS, SETTINGS_PATH, VOICE_NAME_MAX


class PresetsScreenMixin:
    def _preset_path(self, slot: int) -> pathlib.Path:
        return PRESETS_DIR / f"slot-{slot + 1:02d}.json"


    def _build_presets_mode(self) -> None:
        shell = self._presets_shell
        for w in shell.winfo_children():
            w.destroy()
        PRESETS_DIR.mkdir(parents=True, exist_ok=True)

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Presets", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="full session snapshot",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        status = tk.Label(
            shell, textvariable=self._preset_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(4, 8))

        grid = tk.Frame(shell, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True, padx=8, pady=4)
        self._preset_slot_btns = {}
        cols = 4
        for i in range(PRESET_SLOTS):
            r, c = divmod(i, cols)
            btn = self._mk_touch_btn(
                grid,
                self._preset_slot_label(i),
                lambda idx=i: self._select_preset_slot(idx),
                bg="#3c3836",
            )
            btn.configure(font=("DejaVu Sans", 13, "bold"), pady=18)
            btn.grid(row=r, column=c, sticky="nsew", padx=3, pady=3, ipady=6)
            self._preset_slot_btns[i] = btn
        for c in range(cols):
            grid.grid_columnconfigure(c, weight=1)
        for r in range((PRESET_SLOTS + cols - 1) // cols):
            grid.grid_rowconfigure(r, weight=1)

        footer = tk.Frame(shell, bg="#111111")
        footer.pack(fill=tk.X, padx=8, pady=8)
        # FACTORY first so it stays visible on short panels
        self._mk_touch_btn(
            footer, "FACTORY", self._factory_reset_sound, bg="#d79921"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16)
        self._mk_touch_btn(footer, "LOAD", self._preset_load_selected, bg="#458588").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "SAVE", self._open_save_preset, bg="#689d6a").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "DELETE", self._preset_delete_selected, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._paint_preset_slots()


    def _preset_slot_label(self, slot: int) -> str:
        path = self._preset_path(slot)
        if path.is_file():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                name = str(data.get("name") or path.stem)
                seq = data.get("seq") if isinstance(data, dict) else None
                layers = 0
                if isinstance(seq, dict):
                    sequence = seq.get("sequence")
                    if isinstance(sequence, dict):
                        layers = len(sequence.get("layers") or [])
                phrases = data.get("phrases") if isinstance(data, dict) else None
                pads_n = 0
                if isinstance(phrases, dict):
                    pads = phrases.get("pads")
                    if isinstance(pads, list):
                        pads_n = sum(
                            1
                            for p in pads
                            if isinstance(p, dict) and (p.get("events") or [])
                        )
                bits = []
                if layers:
                    bits.append(f"{layers}L")
                if pads_n:
                    bits.append(f"{pads_n}P")
                tag = " ".join(bits) if bits else "session"
                return f"{slot + 1}\n{name}\n{tag}"
            except Exception:
                return f"{slot + 1}\n{path.stem}\n(saved)"
        return f"{slot + 1}\nEMPTY"


    def _paint_preset_slots(self) -> None:
        for i, btn in self._preset_slot_btns.items():
            exists = self._preset_path(i).is_file()
            selected = i == self._preset_slot
            if selected:
                color = "#b16286"
            elif exists:
                color = "#458588"
            else:
                color = "#3c3836"
            btn.configure(
                text=self._preset_slot_label(i),
                bg=color,
                activebackground=color,
            )


    def _suggested_preset_name(self) -> str:
        path = self._preset_path(self._preset_slot)
        if path.is_file():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                existing = sanitize_voice_name(str(data.get("name") or ""))
                if existing and existing != "voice":
                    return existing
            except Exception:
                pass
        a, b, blend = self.engine.morph_neighbors()
        base = suggest_voice_name(a, b, blend)
        return sanitize_voice_name(f"{base}_p{self._preset_slot + 1:02d}")[:VOICE_NAME_MAX]


    def _open_save_preset(self) -> None:
        """Name pad, then write a full-session snapshot into the selected slot."""
        if self._save_preset_open:
            return
        if self._save_voice_open:
            self._close_save_voice(restore_main=False)
        if self._mode != "presets":
            self._switch_mode("presets")
        self._save_preset_open = True
        self._save_preset_keys_digits = False
        self._presets_shell.pack_forget()

        self._save_preset_frame = tk.Frame(self._mode_host, bg="#111111")
        self._save_preset_frame.pack(fill=tk.BOTH, expand=True)

        header = tk.Frame(self._save_preset_frame, bg="#111111")
        header.pack(fill=tk.X, padx=6, pady=(6, 2))
        tk.Label(
            header,
            text="SAVE PRESET",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text=f"slot {self._preset_slot + 1} · full session",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            self._save_preset_frame,
            text=(
                "Stores synth, FX/drum modes, sequencer layers, phrase pads, "
                "songs selection, and the current screen — everything to restore this moment."
            ),
            font=("DejaVu Sans", 10),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, padx=8, pady=(0, 4))

        name_row = tk.Frame(self._save_preset_frame, bg="#111111")
        name_row.pack(fill=tk.X, padx=8, pady=4)
        suggested = self._suggested_preset_name()
        self._save_preset_entry = tk.Entry(
            name_row,
            font=("DejaVu Sans Mono", 18),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
        )
        self._save_preset_entry.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, ipady=10)
        self._save_preset_entry.insert(0, suggested)
        self._save_preset_entry.focus_set()

        self._save_preset_status = tk.Label(
            self._save_preset_frame,
            text=f"Will write {PRESETS_DIR.name}/slot-{self._preset_slot + 1:02d}.json as '{suggested}'",
            font=("DejaVu Sans Mono", 11),
            fg="#83a598",
            bg="#111111",
            anchor="w",
        )
        self._save_preset_status.pack(fill=tk.X, padx=8, pady=(0, 4))

        opt = tk.Frame(self._save_preset_frame, bg="#111111")
        opt.pack(fill=tk.X, padx=6, pady=2)
        self._mk_touch_btn(
            opt, "SUGGEST", self._reset_save_preset_name, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "⌫", lambda: self._save_preset_type("\b"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)
        self._mk_touch_btn(
            opt, "CLR", lambda: self._save_preset_type("\x15"), bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=8)

        footer = tk.Frame(self._save_preset_frame, bg="#111111")
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=6, pady=6)
        self._mk_touch_btn(
            footer, "SAVE", self._confirm_save_preset, bg="#689d6a"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)

        keys = tk.Frame(self._save_preset_frame, bg="#111111")
        keys.pack(fill=tk.BOTH, expand=True, padx=4, pady=2)
        self._save_preset_keys = keys
        self._paint_save_preset_keyboard()
        self._append_log(
            f"SAVE PRESET — name slot {self._preset_slot + 1} (full session)"
        )
        self._paint_nav_back()


    def _paint_save_preset_keyboard(self) -> None:
        keys = getattr(self, "_save_preset_keys", None)
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._save_preset_keys_digits:
            rows = ("1234567890", "-.")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm_")
            toggle_label = "123"
        for row in rows:
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._save_preset_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(
            fr, toggle_label, self._toggle_save_preset_keys, bg="#504945"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=6)


    def _toggle_save_preset_keys(self) -> None:
        self._save_preset_keys_digits = not self._save_preset_keys_digits
        self._paint_save_preset_keyboard()


    def _reset_save_preset_name(self) -> None:
        if self._save_preset_entry is None:
            return
        suggested = self._suggested_preset_name()
        self._save_preset_entry.delete(0, tk.END)
        self._save_preset_entry.insert(0, suggested)
        self._update_save_preset_status()


    def _save_preset_type(self, ch: str) -> None:
        entry = self._save_preset_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        elif ch == "\x15":
            entry.delete(0, tk.END)
        else:
            entry.insert(tk.END, ch)
        self._update_save_preset_status()


    def _update_save_preset_status(self) -> None:
        if self._save_preset_status is None or self._save_preset_entry is None:
            return
        name = sanitize_voice_name(self._save_preset_entry.get())
        path = self._preset_path(self._preset_slot)
        tag = "overwrite" if path.is_file() else "new"
        self._save_preset_status.configure(
            text=f"{tag}: {PRESETS_DIR.name}/{path.name} as '{name}'",
            fg="#fabd2f" if path.is_file() else "#83a598",
        )


    def _confirm_save_preset(self) -> None:
        if self._save_preset_entry is None:
            return
        name = sanitize_voice_name(self._save_preset_entry.get())
        if not name or name == "voice":
            if self._save_preset_status is not None:
                self._save_preset_status.configure(text="Need a name", fg="#fb4934")
            return
        path = self._preset_path(self._preset_slot)
        payload = self._session_dict()
        payload["name"] = name
        try:
            PRESETS_DIR.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            tmp.replace(path)
            self._active_preset_name = path.stem
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._preset_status_var.set(f"Saved '{name}' → {path.name}")
            self._append_log(f"Preset saved: {path.name} ({name})")
            self._paint_preset_slots()
            self._close_save_preset()
        except Exception as exc:
            if self._save_preset_status is not None:
                self._save_preset_status.configure(text=f"Save failed: {exc}", fg="#fb4934")
            self._append_log(f"Preset SAVE error: {exc}")


    def _close_save_preset(self, restore_main: bool = True) -> None:
        if not self._save_preset_open:
            return
        if self._save_preset_frame is not None:
            self._save_preset_frame.destroy()
            self._save_preset_frame = None
        self._save_preset_entry = None
        self._save_preset_status = None
        self._save_preset_keys = None
        self._save_preset_open = False
        if restore_main and self._mode == "presets":
            self._presets_shell.pack(fill=tk.BOTH, expand=True)
            self._paint_preset_slots()
        self._paint_nav_back()


    def _preset_load_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        if not path.is_file():
            self._preset_status_var.set(f"Slot {self._preset_slot + 1} is empty.")
            return
        if self._load_settings_file(path):
            self._active_preset_name = path.stem
            self._refresh_ui_after_session()
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            label = path.name
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                label = str(data.get("name") or path.name)
            except Exception:
                pass
            self._preset_status_var.set(f"Loaded '{label}'")
            self._append_log(f"Preset loaded: {path.name} ({label})")
            self._paint_preset_slots()
        else:
            self._preset_status_var.set(f"Load failed: {path.name}")


    def _preset_delete_selected(self) -> None:
        path = self._preset_path(self._preset_slot)
        if not path.is_file():
            self._preset_status_var.set(f"Slot {self._preset_slot + 1} already empty.")
            return
        try:
            path.unlink()
            if self._active_preset_name == path.stem:
                self._active_preset_name = None
                self._mark_settings_dirty()
            self._preset_status_var.set(f"Deleted {path.name}")
            self._append_log(f"Preset deleted: {path.name}")
            self._paint_preset_slots()
        except Exception as exc:
            self._preset_status_var.set(f"Delete failed: {exc}")


    def _factory_reset_sound(self) -> None:
        """Hard reset to baked-in defaults (drums/FX/morph) — not a saved preset."""
        self._panic()
        if self._fx_ui_open:
            self._close_fx_panel(restore_main=False)
        if self._kit_ui_open:
            self._close_kit_explorer(restore_main=False)
        if self._grid_open:
            self._close_voice_grid(restore_main=False)
        if self._morph_ui_open:
            self._close_morph_menu(restore_main=False)

        self.engine.reset_to_factory_defaults()
        self._full_vel = True
        self._active_preset_name = None
        self._paint_full_vel_btn()
        self._paint_drum_lock_btn()
        self._paint_fx_mode_btn()
        self._paint_bus_fx_mode_btn()
        self._sync_voice_index_from_morph()
        if self._voice_lbl is not None:
            self._voice_lbl.configure(text=self._voice_label_text())
        self.mod_var.set(self._format_mod_line())
        if not self._overlay_busy() and self._mode == "synth":
            self._paint_synth_waveform(force=True)
        self._mark_settings_dirty()
        self._save_settings_file(SETTINGS_PATH, quiet=True)
        self._preset_status_var.set(
            "FACTORY DEFAULTS — morph/tone/drums/FX reset (saved as session)"
        )
        self._append_log(
            "FACTORY DEFAULTS — drums macros, levels, tone, morph A/B, and all FX cleared"
        )
        self.last_var.set("Factory defaults restored")
        self._paint_preset_slots()
