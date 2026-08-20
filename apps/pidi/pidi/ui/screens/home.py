"""home UI mixin for MidiToneApp."""
from __future__ import annotations

import tkinter as tk

from pidi.constants import HOME_TILES


class HomeScreenMixin:
    def _build_home_mode(self) -> None:
        shell = self._home_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header,
            text="Home",
            font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)

        # Fixed 4×2 grid — equal column/row weights so every tile is the same size.
        body = tk.Frame(shell, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=8, pady=8)
        for r in range(2):
            body.rowconfigure(r, weight=1, uniform="home_row")
        for c in range(4):
            body.columnconfigure(c, weight=1, uniform="home_col")

        tiles = list(HOME_TILES)
        if len(tiles) != 8:
            raise RuntimeError(f"HOME_TILES must be exactly 8 entries (4×2), got {len(tiles)}")
        for i, (key, title, color) in enumerate(tiles):
            r, c = divmod(i, 4)
            btn = self._mk_touch_btn(
                body,
                title,
                lambda m=key: self._switch_mode(m),
                bg=color,
            )
            btn.configure(font=("DejaVu Sans", 16, "bold"), pady=8, justify=tk.CENTER)
            btn.grid(row=r, column=c, sticky="nsew", padx=4, pady=4)
