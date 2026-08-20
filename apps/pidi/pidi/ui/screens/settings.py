"""settings UI mixin for MidiToneApp — hub + nested Update / Wi‑Fi panels."""
from __future__ import annotations

import sys
import threading
import tkinter as tk
from typing import Optional

from pidi import updater
from pidi import wifi as wifi_mod
from pidi.constants import APP_VERSION, SETTINGS_PATH


class SettingsScreenMixin:
    def _build_settings_mode(self) -> None:
        panel = getattr(self, "_settings_panel", "hub") or "hub"
        if panel == "update":
            self._build_settings_update_panel()
        elif panel == "wifi":
            self._build_settings_wifi_panel()
        else:
            self._build_settings_hub()

    def _settings_open_panel(self, panel: str) -> None:
        self._settings_panel = panel
        self._build_settings_mode()

    def _build_settings_hub(self) -> None:
        shell = self._settings_shell
        for w in shell.winfo_children():
            w.destroy()
        self._settings_check_btn = None
        self._settings_update_btn = None
        self._settings_wifi_btn = None

        header, body, _footer = self._pack_screen_regions(
            shell,
            header_padx=8,
            header_pady=(10, 4),
            body_padx=10,
            body_pady=4,
            footer_padx=8,
            footer_pady=8,
        )
        tk.Label(
            header,
            text="Settings",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)

        ver = tk.Frame(body, bg="#1d2021")
        ver.pack(fill=tk.X, pady=(0, 10), ipady=6)
        self._settings_version_lbl = tk.Label(
            ver,
            text=updater.format_running_version_line(),
            font=("DejaVu Sans", 15, "bold"),
            fg="#fabd2f",
            bg="#1d2021",
            anchor="w",
        )
        self._settings_version_lbl.pack(fill=tk.X, padx=10, pady=(6, 2))
        tk.Label(
            ver,
            textvariable=self._settings_hub_detail_var,
            font=("DejaVu Sans", 11),
            fg="#a89984",
            bg="#1d2021",
            wraplength=740,
            justify=tk.LEFT,
            anchor="w",
        ).pack(fill=tk.X, padx=10, pady=(0, 6))

        grid = tk.Frame(body, bg="#111111")
        grid.pack(fill=tk.BOTH, expand=True)
        grid.rowconfigure(0, weight=1, uniform="set_row")
        for c in range(2):
            grid.columnconfigure(c, weight=1, uniform="set_col")

        tiles = (
            ("UPDATE", "#689d6a", lambda: self._settings_open_panel("update")),
            ("WIFI", "#d79921", lambda: self._settings_open_panel("wifi")),
        )
        for i, (title, color, cmd) in enumerate(tiles):
            btn = self._mk_touch_btn(grid, title, cmd, bg=color)
            btn.configure(font=("DejaVu Sans", 18, "bold"), pady=8, justify=tk.CENTER)
            btn.grid(row=0, column=i, sticky="nsew", padx=4, pady=4)

        self._refresh_settings_hub_detail()

    def _refresh_settings_hub_detail(self) -> None:
        # Paint instantly; Wi-Fi / build detail fills in off the UI thread.
        cached = getattr(self, "_settings_hub_wifi_cache", "Wi-Fi: …")
        self._settings_hub_detail_var.set(cached)
        lbl = getattr(self, "_settings_version_lbl", None)
        if lbl is not None:
            try:
                lbl.configure(text=updater.format_running_version_line())
            except Exception:
                pass
        threading.Thread(target=self._settings_hub_status_worker, daemon=True).start()

    def _settings_hub_status_worker(self) -> None:
        try:
            wifi_line = wifi_mod.format_wifi_line(quick=True)
        except Exception as exc:
            wifi_line = f"Wi-Fi: status error ({exc})"
        try:
            stamped = updater.read_version_file(updater.HERE)
            local = stamped if stamped.sha else updater.local_version()
            build = f"Build {local.short}"
            if local.branch:
                build += f" · {local.branch}"
            if local.source and local.source not in ("unknown", ""):
                build += f" · {local.source}"
        except Exception:
            build = "Build unknown"
        self._q_put(("settings_hub", wifi_line, build))

    def _build_settings_update_panel(self) -> None:
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
            header,
            text="Update",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._mk_touch_btn(
            header, "BACK", lambda: self._settings_open_panel("hub"), bg="#504945"
        ).pack(side=tk.RIGHT, padx=2, ipady=4)

        tk.Label(
            body,
            text=f"PiDI {APP_VERSION}",
            font=("DejaVu Sans", 12, "bold"),
            fg="#83a598",
            bg="#111111",
            anchor="w",
        ).pack(fill=tk.X, pady=(0, 2))
        tk.Label(
            body,
            textvariable=self._settings_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="nw",
        ).pack(fill=tk.BOTH, expand=True, pady=(4, 8))

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
        self._refresh_settings_status()

    def _build_settings_wifi_panel(self) -> None:
        shell = self._settings_shell
        for w in shell.winfo_children():
            w.destroy()
        self._settings_check_btn = None
        self._settings_update_btn = None

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
            header,
            text="Wi‑Fi",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._mk_touch_btn(
            header, "BACK", lambda: self._settings_open_panel("hub"), bg="#504945"
        ).pack(side=tk.RIGHT, padx=2, ipady=4)

        tk.Label(
            body,
            textvariable=self._settings_status_var,
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="nw",
        ).pack(fill=tk.BOTH, expand=True, pady=(4, 8))

        self._settings_wifi_btn = self._mk_touch_btn(
            footer, "CONNECT", self._settings_wifi, bg="#d79921"
        )
        self._settings_wifi_btn.pack(fill=tk.BOTH, expand=True, ipady=16)
        self._refresh_settings_status()

    def _paint_settings_buttons(self) -> None:
        check_btn = self._settings_check_btn
        update_btn = self._settings_update_btn
        wifi_btn = getattr(self, "_settings_wifi_btn", None)
        if self._update_busy:
            if check_btn is not None:
                check_btn.configure(
                    text="WORKING…", bg="#3c3836", activebackground="#3c3836"
                )
            if update_btn is not None:
                update_btn.configure(
                    text="WORKING…", bg="#3c3836", activebackground="#3c3836"
                )
            if wifi_btn is not None:
                wifi_btn.configure(
                    text="WORKING…", bg="#3c3836", activebackground="#3c3836"
                )
            return
        if wifi_btn is not None and getattr(self, "_settings_panel", "") == "wifi":
            wifi_btn.configure(text="CONNECT", bg="#d79921", activebackground="#d79921")
        if check_btn is None or update_btn is None:
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
        panel = getattr(self, "_settings_panel", "hub")
        if panel == "hub":
            self._refresh_settings_hub_detail()
            return
        lines = updater.format_status_lines(self._update_check)
        try:
            wifi_line = wifi_mod.format_wifi_line(quick=True)
        except Exception as exc:
            wifi_line = f"Wi-Fi: status error ({exc})"
        if panel == "wifi":
            self._settings_status_var.set(wifi_line)
        else:
            self._settings_status_var.set(f"{lines}\n{wifi_line}")
        self._paint_settings_buttons()

    def _settings_wifi(self) -> None:
        if self._update_busy or self._update_confirming:
            return
        self._update_busy = True
        self._settings_status_var.set("Connecting Wi-Fi…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_wifi_worker, daemon=True).start()

    def _settings_wifi_worker(self) -> None:
        try:
            ok, detail = wifi_mod.ensure_wifi_up()
            self._q_put(("wifi", ok, detail))
        except Exception as exc:
            self._q_put(("wifi", False, str(exc)))

    def _settings_check(self) -> None:
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
        if self._update_busy:
            return
        if not self._update_confirming:
            if self._update_check is None:
                self._settings_status_var.set("Checking before install…")
                self._update_busy = True
                self._paint_settings_buttons()

                def _check_then_confirm() -> None:
                    try:
                        result = updater.check_for_update()
                        self._q_put(
                            ("update", result, "confirm" if result.available else None)
                        )
                    except Exception as exc:
                        self._q_put(("update", None, str(exc)))

                threading.Thread(target=_check_then_confirm, daemon=True).start()
                return
            if self._update_check.error:
                self._settings_status_var.set(self._update_check.error)
                return
            if not self._update_check.available:
                self._settings_status_var.set(
                    self._update_check.message or "Already on latest."
                )
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
        """Stop audio, keep the singleton lock, exec the new midi_tone.py.

        Does not touch LightDM. A separate systemd timer
        (midi-tone-lightdm-watchdog) recovers the DM if it was left dead.
        """
        self._append_log("Update installed — restarting…")
        try:
            import subprocess
            from pidi.constants import HERE

            ensure = HERE / "scripts" / "session" / "ensure-lightdm.sh"
            if ensure.is_file():
                subprocess.run(
                    ["sudo", "-n", str(ensure)],
                    check=False,
                    timeout=25,
                    capture_output=True,
                )
        except Exception:
            pass
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
