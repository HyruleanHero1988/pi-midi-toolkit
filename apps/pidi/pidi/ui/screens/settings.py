"""settings UI mixin for MidiToneApp — hub + nested Update / Wi‑Fi panels."""
from __future__ import annotations

import sys
import threading
import tkinter as tk
from typing import List, Optional

from pidi import updater
from pidi import wifi as wifi_mod
from pidi.constants import APP_VERSION, HERE, SETTINGS_PATH
from pidi.ui.touch_keyboard import TouchKeyboardOptions

WIFI_LIST_VISIBLE = 4


def _tk_label(text: str) -> str:
    """Tk treats ``&`` as a keyboard mnemonic — show it literally on touch buttons."""
    return text.replace("&", "&&")


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
        # Leaving Update/Wi‑Fi mid-flight does not cancel the worker — and an
        # install that finishes will still restart. Keep the user on this screen.
        if self._update_busy and panel != getattr(self, "_settings_panel", None):
            return
        self._close_wifi_password_overlay(restore=False)
        self._settings_panel = panel
        self._build_settings_mode()
        self._paint_nav_back()

    def _settings_back_to_hub(self) -> None:
        if self._update_busy:
            return
        if getattr(self, "_touch_keyboard", None) is not None:
            self._close_wifi_password_overlay(restore=True)
            self._paint_nav_back()
            return
        self._settings_open_panel("hub")

    def _build_settings_hub(self) -> None:
        shell = self._settings_shell
        for w in shell.winfo_children():
            w.destroy()
        self._settings_check_btn = None
        self._settings_update_btn = None
        self._settings_wifi_btn = None
        self._settings_diag_btn = None
        self._settings_back_btn = None

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
        for c in range(3):
            grid.columnconfigure(c, weight=1, uniform="set_col")

        tiles = (
            ("UPDATE", "#689d6a", lambda: self._settings_open_panel("update")),
            ("WIFI", "#d79921", lambda: self._settings_open_panel("wifi")),
            ("DIAG", "#504945", self._toggle_diagnostics),
        )
        for i, (title, color, cmd) in enumerate(tiles):
            btn = self._mk_touch_btn(grid, title, cmd, bg=color)
            btn.configure(font=("DejaVu Sans", 18, "bold"), pady=8, justify=tk.CENTER)
            btn.grid(row=0, column=i, sticky="nsew", padx=4, pady=4)
            if title == "DIAG":
                self._settings_diag_btn = btn
        self._paint_diag_btn()

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
        self._settings_back_btn = None

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
        self._settings_wifi_btn = None
        self._wifi_row_btns = []

        header, body, footer = self._pack_screen_regions(
            shell,
            header_padx=8,
            header_pady=(8, 2),
            body_padx=8,
            body_pady=2,
            footer_padx=8,
            footer_pady=6,
        )
        tk.Label(
            header,
            text="Wi‑Fi",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        self._settings_back_btn = None

        tk.Label(
            body,
            textvariable=self._settings_status_var,
            font=("DejaVu Sans", 12, "bold"),
            fg="#fabd2f",
            bg="#111111",
            wraplength=760,
            justify=tk.LEFT,
            anchor="nw",
        ).pack(fill=tk.X, pady=(0, 4))

        list_wrap = tk.Frame(body, bg="#111111")
        list_wrap.pack(fill=tk.BOTH, expand=True)

        pager = tk.Frame(list_wrap, bg="#111111")
        pager.pack(side=tk.RIGHT, fill=tk.Y, padx=(6, 0))
        self._mk_touch_btn(
            pager, "▲", lambda: self._wifi_scroll_by(-WIFI_LIST_VISIBLE), bg="#504945"
        ).pack(side=tk.TOP, expand=True, fill=tk.BOTH, pady=(0, 2), ipady=8)
        self._mk_touch_btn(
            pager, "▼", lambda: self._wifi_scroll_by(WIFI_LIST_VISIBLE), bg="#504945"
        ).pack(side=tk.BOTTOM, expand=True, fill=tk.BOTH, pady=(2, 0), ipady=8)

        rows = tk.Frame(list_wrap, bg="#111111")
        rows.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        self._wifi_row_btns = []
        for i in range(WIFI_LIST_VISIBLE):
            btn = self._mk_touch_btn(
                rows,
                "",
                lambda idx=i: self._wifi_select_row(idx),
                bg="#3c3836",
            )
            btn.configure(
                font=("DejaVu Sans", 13, "bold"),
                anchor="w",
                padx=10,
            )
            btn.pack(fill=tk.BOTH, expand=True, pady=2, ipady=6)
            self._wifi_row_btns.append(btn)

        self._mk_touch_btn(
            footer, "SCAN", self._settings_wifi_scan, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)
        self._settings_wifi_btn = self._mk_touch_btn(
            footer, "REJOIN", self._settings_wifi, bg="#d79921"
        )
        self._settings_wifi_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=12)

        self._refresh_settings_status()
        self._paint_wifi_network_rows()
        if not getattr(self, "_wifi_networks", None):
            self._settings_wifi_scan()

    def _open_wifi_password_overlay(self) -> None:
        """Switch-style password keyboard (reusable TouchKeyboardOverlay)."""
        self._close_wifi_password_overlay(restore=False)
        ssid = getattr(self, "_wifi_selected_ssid", "") or "network"
        try:
            self._settings_shell.pack_forget()
        except Exception:
            pass

        def _on_submit(password: str) -> None:
            self._settings_wifi_join_with_password(password)

        def _on_cancel() -> None:
            self._close_wifi_password_overlay(restore=True)

        kb = self._open_touch_keyboard(
            TouchKeyboardOptions(
                title="JOIN WI‑FI",
                subtitle=ssid,
                password=True,
                submit_label="JOIN",
                cancel_label="",
            ),
            on_submit=_on_submit,
            on_cancel=_on_cancel,
        )
        kb.set_status(f"Enter password for {ssid}")

    def _close_wifi_password_overlay(self, restore: bool = True) -> None:
        self._close_touch_keyboard()
        # Legacy cleanup if an older frame is still around.
        frame = getattr(self, "_wifi_password_frame", None)
        if frame is not None:
            try:
                frame.destroy()
            except Exception:
                pass
        self._wifi_password_frame = None
        self._wifi_password_entry = None
        if restore:
            self._wifi_selected_ssid = ""
        if not restore:
            return
        if getattr(self, "_mode", "") != "settings":
            return
        try:
            self._settings_shell.pack(fill=tk.BOTH, expand=True)
        except Exception:
            pass
        self._refresh_settings_status()
        self._paint_settings_buttons()
        self._paint_nav_back()

    def _wifi_scroll_by(self, delta: int) -> None:
        networks = getattr(self, "_wifi_networks", []) or []
        if not networks:
            return
        max_scroll = max(0, len(networks) - WIFI_LIST_VISIBLE)
        self._wifi_scroll = max(0, min(max_scroll, self._wifi_scroll + delta))
        self._paint_wifi_network_rows()

    def _paint_wifi_network_rows(self) -> None:
        networks: List = getattr(self, "_wifi_networks", []) or []
        btns = getattr(self, "_wifi_row_btns", []) or []
        scroll = int(getattr(self, "_wifi_scroll", 0) or 0)
        for i, btn in enumerate(btns):
            idx = scroll + i
            if idx >= len(networks):
                btn.configure(text="", state=tk.DISABLED, bg="#1d2021")
                continue
            net = networks[idx]
            btn.configure(
                text=net.label(),
                state=tk.NORMAL,
                bg="#689d6a" if net.in_use else "#3c3836",
                activebackground="#689d6a" if net.in_use else "#504945",
            )

    def _wifi_select_row(self, row_idx: int) -> None:
        if self._update_busy:
            return
        networks = getattr(self, "_wifi_networks", []) or []
        idx = int(getattr(self, "_wifi_scroll", 0) or 0) + row_idx
        if idx < 0 or idx >= len(networks):
            return
        net = networks[idx]
        self._wifi_selected_ssid = net.ssid
        self._wifi_selected_open = bool(net.is_open)
        if net.is_open:
            self._settings_status_var.set(f"Joining open network {net.ssid}…")
            self._update_busy = True
            self._paint_settings_buttons()
            threading.Thread(
                target=self._settings_wifi_join_worker,
                args=(net.ssid, ""),
                daemon=True,
            ).start()
            return
        self._open_wifi_password_overlay()

    def _paint_settings_buttons(self) -> None:
        check_btn = self._settings_check_btn
        update_btn = self._settings_update_btn
        wifi_btn = getattr(self, "_settings_wifi_btn", None)
        busy = bool(self._update_busy)

        def _dim(btn: Optional[tk.Button], label: str, idle_bg: str) -> None:
            if btn is None:
                return
            show = _tk_label(label)
            if busy:
                btn.configure(
                    text=show,
                    bg="#3c3836",
                    activebackground="#3c3836",
                    state=tk.DISABLED,
                )
            else:
                btn.configure(
                    text=show,
                    bg=idle_bg,
                    activebackground=idle_bg,
                    state=tk.NORMAL,
                )

        if busy:
            _dim(check_btn, "CHECK", "#458588")
            _dim(update_btn, "UPDATE", "#689d6a")
            _dim(wifi_btn, "REJOIN", "#d79921")
            return

        if wifi_btn is not None and getattr(self, "_settings_panel", "") == "wifi":
            _dim(wifi_btn, "REJOIN", "#d79921")
        if check_btn is None or update_btn is None:
            return
        if self._update_confirming:
            check_btn.configure(
                text=_tk_label("CANCEL"),
                bg="#504945",
                activebackground="#504945",
                state=tk.NORMAL,
            )
            update_btn.configure(
                text=_tk_label("INSTALL NOW"),
                bg="#9d0006",
                activebackground="#9d0006",
                state=tk.NORMAL,
            )
            return
        available = bool(self._update_check and self._update_check.available)
        color = "#689d6a" if available or self._update_check is None else "#3c3836"
        check_btn.configure(
            text=_tk_label("CHECK"), bg="#458588", activebackground="#458588", state=tk.NORMAL
        )
        update_btn.configure(
            text=_tk_label("UPDATE"), bg=color, activebackground=color, state=tk.NORMAL
        )

    def _refresh_settings_status(self) -> None:
        panel = getattr(self, "_settings_panel", "hub")
        if panel == "hub":
            self._refresh_settings_hub_detail()
            return
        if getattr(self, "_touch_keyboard", None) is not None:
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
        self._settings_status_var.set("Rejoining saved / preconfigured Wi‑Fi…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_wifi_worker, daemon=True).start()

    def _settings_wifi_worker(self) -> None:
        try:
            ok, detail = wifi_mod.ensure_wifi_up()
            self._q_put(("wifi", ok, detail))
        except Exception as exc:
            self._q_put(("wifi", False, str(exc)))

    def _settings_wifi_scan(self) -> None:
        if self._update_busy or self._update_confirming:
            return
        self._update_busy = True
        self._settings_status_var.set("Scanning for networks…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_wifi_scan_worker, daemon=True).start()

    def _settings_wifi_scan_worker(self) -> None:
        try:
            networks, err = wifi_mod.scan_wifi_networks(rescan=True)
            self._q_put(("wifi_scan", networks, err))
        except Exception as exc:
            self._q_put(("wifi_scan", [], str(exc)))

    def _settings_wifi_join_with_password(self, password: str) -> None:
        if self._update_busy:
            return
        ssid = getattr(self, "_wifi_selected_ssid", "") or ""
        kb = getattr(self, "_touch_keyboard", None)
        if not ssid:
            if kb is not None:
                kb.set_status("No network selected")
            return
        self._update_busy = True
        if kb is not None:
            kb.set_status(f"Joining {ssid}…")
        threading.Thread(
            target=self._settings_wifi_join_worker,
            args=(ssid, password),
            daemon=True,
        ).start()

    def _settings_wifi_join_worker(self, ssid: str, password: str) -> None:
        try:
            ok, detail = wifi_mod.connect_wifi(
                ssid, password, install=HERE, remember=True
            )
            self._q_put(("wifi_join", ok, detail, ssid))
        except Exception as exc:
            self._q_put(("wifi_join", False, str(exc), ssid))

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
        self._settings_status_var.set("[0% · 0:00] Starting install…")
        self._paint_settings_buttons()
        threading.Thread(target=self._settings_apply_worker, daemon=True).start()

    def _settings_apply_worker(self) -> None:
        expected = ""
        if self._update_check and self._update_check.remote.sha:
            expected = self._update_check.remote.sha

        tracker = updater.ProgressTracker(
            lambda msg: self._q_put(("update_progress", msg))
        )
        self._update_progress = tracker
        self._q_put(("update_progress_start",))

        try:
            info = updater.apply_update(progress=tracker, expected_sha=expected)
            self._q_put(("update_done", info, None))
        except Exception as exc:
            self._q_put(("update_done", None, str(exc)))

    def _start_update_progress_tick(self) -> None:
        self._stop_update_progress_tick()
        self._tick_update_progress()

    def _stop_update_progress_tick(self) -> None:
        aid = getattr(self, "_update_progress_tick_after", None)
        self._update_progress_tick_after = None
        if aid is not None:
            try:
                self.root.after_cancel(aid)
            except Exception:
                pass

    def _tick_update_progress(self) -> None:
        tracker = getattr(self, "_update_progress", None)
        if not getattr(self, "_update_busy", False) or tracker is None:
            self._update_progress_tick_after = None
            return
        try:
            tracker.tick()
        except Exception:
            pass
        try:
            self._update_progress_tick_after = self.root.after(
                1000, self._tick_update_progress
            )
        except Exception:
            self._update_progress_tick_after = None

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
