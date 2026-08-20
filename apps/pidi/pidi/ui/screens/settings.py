"""settings UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido



class SettingsScreenMixin:
    def _build_settings_mode(self) -> None:
        shell = self._settings_shell
        for w in shell.winfo_children():
            w.destroy()

        header, body, footer = self._pack_screen_regions(
            shell,
            header_padx=8,
            header_pady=(10, 4),
            body_padx=10,
            body_pady=4,
            footer_padx=8,
            footer_pady=8,
        )
        tk.Label(
            header, text="Settings", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="software update",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        tk.Label(
            body,
            textvariable=self._settings_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="nw",
        ).pack(fill=tk.BOTH, expand=True, pady=(4, 8))

        tk.Label(
            body,
            text=(
                "CHECK looks at GitHub master. UPDATE deploys new code like SSH, "
                "then restarts. Phrases, songs, presets, and settings.json stay "
                "on this box. Rust binaries are not rebuilt on the Pi."
            ),
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, pady=(0, 4))

        row = tk.Frame(footer, bg="#111111")
        row.pack(fill=tk.X, pady=(0, 6))
        self._settings_check_btn = self._mk_touch_btn(
            row, "CHECK", self._settings_check, bg="#458588"
        )
        self._settings_check_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
        )
        self._settings_update_btn = self._mk_touch_btn(
            row, "UPDATE", self._settings_update, bg="#689d6a"
        )
        self._settings_update_btn.pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=14
        )
        self._mk_touch_btn(footer, "TOKEN", self._open_update_token, bg="#504945").pack(
            fill=tk.X, ipady=10
        )
        self._paint_settings_buttons()


    def _paint_settings_buttons(self) -> None:
        check_btn = self._settings_check_btn
        update_btn = self._settings_update_btn
        if check_btn is None or update_btn is None:
            return
        if self._update_busy:
            check_btn.configure(text="WORKING…", bg="#3c3836", activebackground="#3c3836")
            update_btn.configure(text="WORKING…", bg="#3c3836", activebackground="#3c3836")
            return
        check_btn.configure(text="CHECK", bg="#458588", activebackground="#458588")
        if self._update_confirming:
            update_btn.configure(
                text="INSTALL NOW", bg="#9d0006", activebackground="#9d0006"
            )
            check_btn.configure(text="CANCEL", bg="#504945", activebackground="#504945")
            return
        available = bool(self._update_check and self._update_check.available)
        color = "#689d6a" if available or self._update_check is None else "#3c3836"
        update_btn.configure(text="UPDATE", bg=color, activebackground=color)


    def _refresh_settings_status(self) -> None:
        self._settings_status_var.set(
            updater.format_status_lines(self._update_check)
        )
        self._paint_settings_buttons()


    def _settings_check(self) -> None:
        if self._token_ui_open:
            return
        if self._update_confirming:
            self._update_confirming = False
            self._refresh_settings_status()
            return
        if self._update_busy:
            return
        self._update_busy = True
        self._settings_status_var.set("Checking GitHub for the latest master…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_check_worker, daemon=True).start()


    def _settings_check_worker(self) -> None:
        try:
            result = updater.check_for_update()
            self._q_put(("update", result, None))
        except Exception as exc:
            self._q_put(("update", None, str(exc)))


    def _settings_update(self) -> None:
        if self._token_ui_open or self._update_busy:
            return
        if not self._update_confirming:
            # CHECK first if we have not looked yet, then ask for a second tap.
            if self._update_check is None:
                self._settings_status_var.set("Checking before install…")
                self._update_busy = True
                self._paint_settings_buttons()

                def _check_then_confirm() -> None:
                    try:
                        result = updater.check_for_update()
                        self._q_put(("update", result, "confirm" if result.available else None))
                    except Exception as exc:
                        self._q_put(("update", None, str(exc)))

                threading.Thread(target=_check_then_confirm, daemon=True).start()
                return
            if self._update_check.error:
                self._settings_status_var.set(self._update_check.error)
                return
            if not self._update_check.available:
                self._settings_status_var.set(self._update_check.message or "Already on latest.")
                return
            self._update_confirming = True
            self._settings_status_var.set(
                "This deploys new code from GitHub, then restarts.\n"
                "Phrases, songs, presets, and settings.json are not touched "
                "(same as SSH deploy).\n"
                "Rust engines come from committed dist/armv7 — not built on this Pi.\n"
                "Tap INSTALL NOW to continue, or CANCEL."
            )
            self._paint_settings_buttons()
            return
        self._update_confirming = False
        self._update_busy = True
        self._settings_status_var.set("Installing update…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_apply_worker, daemon=True).start()


    def _settings_apply_worker(self) -> None:
        expected = ""
        if self._update_check and self._update_check.remote.sha:
            expected = self._update_check.remote.sha

        def progress(msg: str) -> None:
            self._q_put(("update_progress", msg))

        try:
            info = updater.apply_update(progress=progress, expected_sha=expected)
            self._q_put(("update_done", info, None))
        except Exception as exc:
            self._q_put(("update_done", None, str(exc)))


    def _restart_after_update(self) -> None:
        """Stop audio, keep the singleton lock, exec the new midi_tone.py."""
        self._append_log("Update installed — restarting…")
        try:
            self._panic()
        except Exception:
            pass
        try:
            self._seq.stop()
        except Exception:
            pass
        try:
            self._phrases.stop_all()
        except Exception:
            pass
        try:
            self._songs.stop()
            self._songs.close_outport()
        except Exception:
            pass
        try:
            self._save_settings_file(SETTINGS_PATH, quiet=True)
        except Exception:
            pass
        if self._inport is not None:
            try:
                self._inport.close()
            except Exception:
                pass
        try:
            self.engine.stop()
        except Exception:
            pass
        try:
            self.root.update_idletasks()
        except Exception:
            pass
        try:
            self.root.destroy()
        except Exception:
            pass
        try:
            updater.restart_current_process()
        except Exception as exc:
            print(f"re-exec failed ({exc}); exiting for kiosk restart", flush=True)
            sys.exit(0)


    def _open_update_token(self) -> None:
        if self._token_ui_open or self._update_busy:
            return
        if self._mode != "settings":
            self._switch_mode("settings")
        self._token_ui_open = True
        self._token_keys_digits = False
        self._settings_shell.pack_forget()
        self._token_frame = tk.Frame(self._mode_host, bg="#111111")
        self._token_frame.pack(fill=tk.BOTH, expand=True)
        header, body, footer = self._pack_screen_regions(
            self._token_frame,
            header_padx=8,
            header_pady=(8, 2),
            body_padx=8,
            body_pady=4,
            footer_padx=8,
            footer_pady=8,
        )
        tk.Label(
            header,
            text="GITHUB TOKEN",
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header,
            text="private repo access",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
        ).pack(side=tk.RIGHT)
        tk.Label(
            body,
            text="Fine-grained PAT with Contents: Read on this repo. Stored only on this box.",
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, pady=(0, 4))
        self._token_entry = tk.Entry(
            body,
            font=("DejaVu Sans Mono", 16),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
            show="•",
        )
        self._token_entry.pack(fill=tk.X, ipady=10, pady=(0, 6))
        self._token_entry.focus_set()
        self._token_keys = tk.Frame(body, bg="#111111")
        self._token_keys.pack(fill=tk.BOTH, expand=True)
        self._paint_token_keyboard()
        self._mk_touch_btn(footer, "SAVE", self._save_update_token, bg="#689d6a").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )
        self._mk_touch_btn(footer, "CANCEL", self._close_update_token, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )


    def _paint_token_keyboard(self) -> None:
        keys = self._token_keys
        if keys is None:
            return
        for w in keys.winfo_children():
            w.destroy()
        if self._token_keys_digits:
            rows = ("1234567890", "-_./")
            toggle_label = "ABC"
        else:
            rows = ("qwertyuiop", "asdfghjkl", "zxcvbnm")
            toggle_label = "123"
        for row in rows:
            fr = tk.Frame(keys, bg="#111111")
            fr.pack(fill=tk.BOTH, expand=True, pady=1)
            for ch in row:
                self._mk_touch_btn(
                    fr,
                    ch.upper() if ch.isalpha() else ch,
                    lambda c=ch: self._token_type(c),
                    bg="#3c3836",
                ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4)
        fr = tk.Frame(keys, bg="#111111")
        fr.pack(fill=tk.BOTH, expand=True, pady=1)
        self._mk_touch_btn(fr, toggle_label, self._toggle_token_keys, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4
        )
        self._mk_touch_btn(fr, "⌫", lambda: self._token_type("\b"), bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=1, ipady=4
        )


    def _toggle_token_keys(self) -> None:
        self._token_keys_digits = not self._token_keys_digits
        self._paint_token_keyboard()


    def _token_type(self, ch: str) -> None:
        entry = self._token_entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
        else:
            entry.insert(tk.END, ch)


    def _save_update_token(self) -> None:
        entry = self._token_entry
        token = (entry.get() if entry is not None else "").strip()
        if not token:
            self._settings_status_var.set("Token was empty — cancelled.")
            self._close_update_token()
            return
        try:
            updater.save_token(token)
        except Exception as exc:
            self._settings_status_var.set(f"Could not save token: {exc}")
            self._close_update_token()
            return
        self._close_update_token()
        self._append_log("GitHub token saved for SET → UPDATE")
        self._settings_status_var.set("Token saved. Tap CHECK to look at GitHub.")
        self._update_check = None
        self._paint_settings_buttons()


    def _close_update_token(self, restore_main: bool = True) -> None:
        if not self._token_ui_open:
            return
        if self._token_frame is not None:
            self._token_frame.destroy()
            self._token_frame = None
        self._token_entry = None
        self._token_keys = None
        self._token_ui_open = False
        if restore_main:
            self._switch_mode("settings")
