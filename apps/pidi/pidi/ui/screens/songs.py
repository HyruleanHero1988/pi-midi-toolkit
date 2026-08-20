"""songs UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido



class SongsScreenMixin:
    def _selected_song_path(self) -> Optional[pathlib.Path]:
        if not self._song_selected:
            return None
        path = SONGS_DIR / self._song_selected
        return path if path.is_file() else None


    def _refresh_song_file_list(self, prefer: Optional[str] = None) -> None:
        """Rescan songs/ and keep selection/scroll coherent."""
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        self._song_files = list_song_files(SONGS_DIR)
        names = {p.name for p in self._song_files}
        chosen = prefer if prefer in names else None
        if chosen is None and self._song_selected in names:
            chosen = self._song_selected
        if chosen is None and self._song_files:
            chosen = self._song_files[0].name
        self._song_selected = chosen
        if not self._song_files:
            self._song_scroll = 0
            return
        idx = 0
        if chosen:
            for i, p in enumerate(self._song_files):
                if p.name == chosen:
                    idx = i
                    break
        max_scroll = max(0, len(self._song_files) - SONG_LIST_VISIBLE)
        # Keep selection visible
        if idx < self._song_scroll:
            self._song_scroll = idx
        elif idx >= self._song_scroll + SONG_LIST_VISIBLE:
            self._song_scroll = idx - SONG_LIST_VISIBLE + 1
        self._song_scroll = max(0, min(max_scroll, self._song_scroll))


    def _next_take_path(self) -> pathlib.Path:
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        n = 1
        while True:
            path = SONGS_DIR / f"take-{n:03d}.mid"
            if not path.exists():
                return path
            n += 1
            if n > 9999:
                return SONGS_DIR / f"take-{int(time.time())}.mid"


    def _build_songs_mode(self) -> None:
        shell = self._songs_shell
        for w in shell.winfo_children():
            w.destroy()
        SONGS_DIR.mkdir(parents=True, exist_ok=True)
        self._refresh_song_file_list(prefer=self._song_selected)

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(8, 2))
        tk.Label(
            header, text="Songs", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        bpm_row = tk.Frame(header, bg="#111111")
        bpm_row.pack(side=tk.RIGHT)
        self._mk_touch_btn(bpm_row, "BPM −", lambda: self._song_nudge_bpm(-1), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._song_bpm_lbl = tk.Label(
            bpm_row,
            text=self._song_bpm_label(),
            font=("DejaVu Sans", 13, "bold"),
            fg="#fabd2f",
            bg="#111111",
            padx=6,
        )
        self._song_bpm_lbl.pack(side=tk.LEFT)
        self._mk_touch_btn(bpm_row, "BPM +", lambda: self._song_nudge_bpm(1), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._mk_touch_btn(bpm_row, "−5", lambda: self._song_nudge_bpm(-5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )
        self._mk_touch_btn(bpm_row, "+5", lambda: self._song_nudge_bpm(5), bg="#3c3836").pack(
            side=tk.LEFT, padx=2
        )

        status = tk.Label(
            shell, textvariable=self._song_status_var,
            font=("DejaVu Sans", 11, "bold"),
            fg="#fabd2f", bg="#111111",
            wraplength=780, justify=tk.LEFT, anchor="w",
        )
        status.pack(fill=tk.X, padx=10, pady=(2, 4))

        # Transport is packed from the bottom *before* the list, so a short panel
        # shrinks the song rows instead of pushing PLAY/STOP off the screen.
        row_b = tk.Frame(shell, bg="#111111")
        row_b.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(3, 8))
        self._mk_touch_btn(
            row_b, "SAVE SEQ", self._song_save_from_seq, bg="#458588"
        ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)
        self._mk_touch_btn(row_b, "DELETE", self._song_delete_selected, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8
        )
        self._song_out_btn = self._mk_touch_btn(
            row_b, "OUT: LOCAL", self._song_cycle_out_mode, bg="#3c3836"
        )
        self._song_out_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)
        self._song_loop_btn = self._mk_touch_btn(
            row_b, "SONG LOOP: OFF", self._song_toggle_loop, bg="#3c3836"
        )
        self._song_loop_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=8)

        row_a = tk.Frame(shell, bg="#111111")
        row_a.pack(side=tk.BOTTOM, fill=tk.X, padx=8, pady=(4, 3))
        self._song_play_btn = self._mk_touch_btn(
            row_a, "PLAY", self._song_toggle_play, bg="#689d6a"
        )
        self._song_play_btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10)
        self._mk_touch_btn(row_a, "STOP", self._song_stop, bg="#d79921").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=10
        )

        # Chunky list with dedicated scroll targets (no tiny scrollbar). They sit
        # in a side column so paging costs width, which is plentiful, not height.
        list_wrap = tk.Frame(shell, bg="#111111")
        list_wrap.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)

        pager = tk.Frame(list_wrap, bg="#111111")
        pager.pack(side=tk.RIGHT, fill=tk.Y, padx=(6, 0))
        self._song_up_btn = self._mk_touch_btn(
            pager, "▲", lambda: self._song_scroll_by(-SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_up_btn.configure(font=("DejaVu Sans", 18, "bold"), padx=14)
        self._song_up_btn.pack(side=tk.TOP, expand=True, fill=tk.BOTH, pady=(0, 2))
        self._song_down_btn = self._mk_touch_btn(
            pager, "▼", lambda: self._song_scroll_by(SONG_LIST_VISIBLE), bg="#504945"
        )
        self._song_down_btn.configure(font=("DejaVu Sans", 18, "bold"), padx=14)
        self._song_down_btn.pack(side=tk.BOTTOM, expand=True, fill=tk.BOTH, pady=(2, 0))

        rows = tk.Frame(list_wrap, bg="#111111")
        rows.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        self._song_row_btns = []
        for i in range(SONG_LIST_VISIBLE):
            btn = self._mk_touch_btn(
                rows,
                "",
                lambda idx=i: self._select_song_row(idx),
                bg="#3c3836",
            )
            btn.configure(
                font=("DejaVu Sans", 13, "bold"),
                anchor="w",
                justify=tk.LEFT,
                pady=4,
            )
            btn.pack(fill=tk.BOTH, expand=True, pady=2, ipady=2)
            self._song_row_btns.append(btn)

        self._paint_song_list()
        self._paint_song_controls()
        self._refresh_song_status()


    def _song_bpm_label(self) -> str:
        return f"{int(round(self._songs.bpm()))} BPM"


    def _song_title_from_file(self, path: pathlib.Path) -> str:
        key = path.name
        cached = self._song_title_cache.get(key)
        if cached is not None:
            return cached
        title = path.stem
        try:
            mid = mido.MidiFile(str(path))
            for tr in mid.tracks:
                for msg in tr:
                    if msg.is_meta and msg.type in ("track_name", "sequence_name"):
                        name = (msg.name or "").strip()
                        if name:
                            title = name
                            break
                    if msg.is_meta and msg.type == "text":
                        text = (msg.text or "").strip()
                        if text:
                            title = text
                            break
                else:
                    continue
                break
        except Exception:
            pass
        title = title[:40]
        self._song_title_cache[key] = title
        return title


    def _song_row_label(self, path: pathlib.Path) -> str:
        """One line per row — four fat targets beat two tall ones on a 480px panel."""
        title = self._song_title_from_file(path)
        if title.lower() == path.stem.lower() or title == path.stem:
            return f"  {path.name}"
        label = f"  {title} · {path.name}"
        return label if len(label) <= 58 else label[:57] + "…"


    def _song_scroll_by(self, delta: int) -> None:
        if not self._song_files:
            return
        max_scroll = max(0, len(self._song_files) - SONG_LIST_VISIBLE)
        self._song_scroll = max(0, min(max_scroll, self._song_scroll + int(delta)))
        self._paint_song_list()


    def _select_song_row(self, row: int) -> None:
        idx = self._song_scroll + row
        if idx < 0 or idx >= len(self._song_files):
            return
        path = self._song_files[idx]
        self._song_selected = path.name
        if self._songs.is_playing():
            self._songs.stop()
        if self._songs.load(path):
            self._append_log(f"Song loaded: {path.name}")
        else:
            self._song_status_var.set(f"Failed to load {path.name}")
        self._mark_settings_dirty()
        self._paint_song_list()
        self._paint_song_controls()
        self._refresh_song_status()


    def _paint_song_list(self) -> None:
        total = len(self._song_files)
        max_scroll = max(0, total - SONG_LIST_VISIBLE)
        self._song_scroll = max(0, min(max_scroll, self._song_scroll))
        for row, btn in enumerate(self._song_row_btns):
            idx = self._song_scroll + row
            if idx >= total:
                btn.configure(
                    text="",
                    state=tk.DISABLED,
                    bg="#1d2021",
                    activebackground="#1d2021",
                    disabledforeground="#665c54",
                )
                continue
            path = self._song_files[idx]
            selected = path.name == self._song_selected
            color = "#b16286" if selected else "#458588"
            btn.configure(
                text=self._song_row_label(path),
                state=tk.NORMAL,
                bg=color,
                activebackground=color,
                fg="#fbf1c7",
            )
        if self._song_up_btn is not None:
            can_up = self._song_scroll > 0
            self._song_up_btn.configure(
                state=tk.NORMAL if can_up else tk.DISABLED,
                bg="#504945" if can_up else "#1d2021",
                activebackground="#504945" if can_up else "#1d2021",
                disabledforeground="#665c54",
            )
        if self._song_down_btn is not None:
            can_down = self._song_scroll < max_scroll
            self._song_down_btn.configure(
                state=tk.NORMAL if can_down else tk.DISABLED,
                bg="#504945" if can_down else "#1d2021",
                activebackground="#504945" if can_down else "#1d2021",
                disabledforeground="#665c54",
            )


    def _paint_song_slots(self) -> None:
        """Compat name used by mode switch / seed — refresh list from disk."""
        self._refresh_song_file_list(prefer=self._song_selected)
        self._paint_song_list()


    def _paint_song_controls(self) -> None:
        if self._song_bpm_lbl is not None:
            self._song_bpm_lbl.configure(text=self._song_bpm_label())
        if self._song_play_btn is not None:
            if self._songs.is_playing():
                self._song_play_btn.configure(
                    text="■ STOP", bg="#d79921", activebackground="#d79921"
                )
            else:
                self._song_play_btn.configure(
                    text="PLAY", bg="#689d6a", activebackground="#689d6a"
                )
        if self._song_out_btn is not None:
            mode = self._songs.out_mode().upper()
            colors = {"LOCAL": "#3c3836", "USB": "#458588", "BOTH": "#689d6a"}
            color = colors.get(mode, "#3c3836")
            self._song_out_btn.configure(
                text=f"OUT: {mode}", bg=color, activebackground=color
            )
        if self._song_loop_btn is not None:
            if self._songs.loop_enabled():
                self._song_loop_btn.configure(
                    text="SONG LOOP: ON", bg="#689d6a", activebackground="#689d6a"
                )
            else:
                self._song_loop_btn.configure(
                    text="SONG LOOP: OFF", bg="#3c3836", activebackground="#3c3836"
                )


    def _refresh_song_status(self) -> None:
        st = self._songs.status()
        path = st.get("path")
        name = pathlib.Path(str(path)).name if path else (self._song_selected or "(none)")
        nfiles = len(self._song_files)
        out = str(st["out_mode"]).upper()
        out_name = st.get("out_name") or "—"
        if nfiles == 0:
            msg = "songs/ is empty — SAVE SEQ, or drop .mid files in. Demos seed on first launch."
        elif st["playing"]:
            msg = (
                f"▶ PLAYING {name}  @ {int(round(float(st['bpm'])))} BPM  "
                f"(file {int(round(float(st['file_bpm'])))})  out={out} ({out_name})"
            )
        elif int(st["events"]) == 0:
            msg = (
                f"{nfiles} file(s) — tap one to load. "
                f"Tempo {int(round(float(st['bpm'])))} BPM · out={out}"
            )
        else:
            msg = (
                f"Ready {name}  {float(st['duration']):.1f}s · {st['events']} ev  "
                f"@ {int(round(float(st['bpm'])))} BPM (file {int(round(float(st['file_bpm'])))})  "
                f"out={out}  [{self._song_scroll + 1}-{min(nfiles, self._song_scroll + SONG_LIST_VISIBLE)}/{nfiles}]"
            )
        self._song_status_var.set(msg)
        self._paint_song_controls()


    def _song_nudge_bpm(self, delta: float) -> None:
        self._songs.nudge_bpm(delta)
        self._mark_settings_dirty()
        self._refresh_song_status()


    def _song_toggle_loop(self) -> None:
        self._songs.set_loop(not self._songs.loop_enabled())
        self._mark_settings_dirty()
        self._refresh_song_status()


    def _song_cycle_out_mode(self) -> None:
        cur = self._songs.out_mode()
        try:
            idx = SONG_OUT_MODES.index(cur)
        except ValueError:
            idx = 0
        nxt = SONG_OUT_MODES[(idx + 1) % len(SONG_OUT_MODES)]
        self._songs.set_out_mode(nxt)
        if nxt in ("usb", "both"):
            name = self._songs.ensure_outport()
            if name:
                self._append_log(f"Song MIDI out: {name}")
            else:
                self._append_log("Song MIDI out: no output port found")
                if nxt == "usb":
                    self._song_status_var.set(
                        "No USB MIDI out found — plug USB→DIN (or set OUT to LOCAL)."
                    )
                    self._paint_song_controls()
                    self._mark_settings_dirty()
                    return
        self._mark_settings_dirty()
        self._refresh_song_status()


    def _song_toggle_play(self) -> None:
        if self._songs.is_playing():
            self._songs.stop()
            self._q_put(("log", "Song PLAY stop", False))
        else:
            path = self._selected_song_path()
            if self._songs.event_count() == 0 and path is not None:
                self._songs.load(path)
            if not self._songs.start():
                mode = self._songs.out_mode()
                if mode == "usb":
                    self._q_put(("log", "Song PLAY failed — no USB MIDI out", False))
                else:
                    self._q_put(("log", "Song empty — tap a file or SAVE SEQ", False))
            else:
                self._q_put(("log", "Song PLAY start", False))
        self._q_put(("song",))
        self._refresh_song_status()


    def _song_stop(self) -> None:
        if self._songs.is_playing():
            self._songs.stop()
            self._q_put(("log", "Song STOP", False))
            self._q_put(("song",))
        self._refresh_song_status()


    def _song_save_from_seq(self) -> None:
        events, loop_len = self._seq.snapshot()
        if not events or loop_len <= 0.0:
            self._song_status_var.set("Sequence is empty — record something in SEQ first.")
            return
        if self._songs.is_playing():
            self._songs.stop()
        path = self._next_take_path()
        bpm = self._songs.bpm()
        try:
            SONGS_DIR.mkdir(parents=True, exist_ok=True)
            mid = take_events_to_midifile(events, loop_len, bpm=bpm)
            # Title for list label
            if mid.tracks:
                mid.tracks[0].insert(
                    0, mido.MetaMessage("track_name", name=path.stem, time=0)
                )
            tmp = path.with_suffix(".mid.tmp")
            mid.save(str(tmp))
            tmp.replace(path)
            self._song_title_cache.pop(path.name, None)
            self._refresh_song_file_list(prefer=path.name)
            self._songs.load(path)
            self._songs.set_bpm(bpm)
            self._mark_settings_dirty()
            self._save_settings_file(SETTINGS_PATH, quiet=True)
            self._paint_song_list()
            self._refresh_song_status()
            self._append_log(f"Song saved: {path.name} ({len(events)} events @ {int(bpm)} BPM)")
            self._song_status_var.set(f"Saved sequence → {path.name}")
        except Exception as exc:
            self._song_status_var.set(f"Save failed: {exc}")
            self._append_log(f"Song SAVE error: {exc}")


    def _song_delete_selected(self) -> None:
        path = self._selected_song_path()
        if self._songs.is_playing():
            self._songs.stop()
        if path is None:
            self._song_status_var.set("Nothing selected to delete.")
            return
        try:
            name = path.name
            path.unlink()
            self._song_title_cache.pop(name, None)
            self._songs.clear()
            self._song_selected = None
            self._refresh_song_file_list()
            # Autoload neighbor if any remain
            nxt = self._selected_song_path()
            if nxt is not None:
                self._songs.load(nxt)
            self._mark_settings_dirty()
            self._paint_song_list()
            self._refresh_song_status()
            self._append_log(f"Song deleted: {name}")
            self._song_status_var.set(f"Deleted {name}")
        except Exception as exc:
            self._song_status_var.set(f"Delete failed: {exc}")
