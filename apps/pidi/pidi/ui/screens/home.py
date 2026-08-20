"""home UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from typing import Any, Dict, List, Optional, Tuple

import mido



class HomeScreenMixin:
    def _build_home_mode(self) -> None:
        shell = self._home_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Home", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="tap a mode",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        body = tk.Frame(shell, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=8, pady=8)
        rows = (HOME_TILES[:4], HOME_TILES[4:])
        for spec in rows:
            row = tk.Frame(body, bg="#111111")
            row.pack(fill=tk.BOTH, expand=True, pady=4)
            for key, title, subtitle, color in spec:
                btn = self._mk_touch_btn(
                    row,
                    f"{title}\n{subtitle}",
                    lambda m=key: self._switch_mode(m),
                    bg=color,
                )
                btn.configure(font=("DejaVu Sans", 18, "bold"), pady=22, justify=tk.CENTER)
                btn.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=4)
