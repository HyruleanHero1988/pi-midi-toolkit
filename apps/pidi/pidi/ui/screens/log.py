"""log UI mixin for MidiToneApp."""
from __future__ import annotations

import math
import pathlib
import time
import tkinter as tk
from tkinter import ttk
from typing import Any, Dict, List, Optional, Tuple

import mido


class LogScreenMixin:
    def _build_log_mode(self) -> None:
        shell = self._log_shell
        for w in shell.winfo_children():
            w.destroy()

        header = tk.Frame(shell, bg="#111111")
        header.pack(fill=tk.X, padx=8, pady=(10, 4))
        tk.Label(
            header, text="Event log", font=("DejaVu Sans", 18, "bold"),
            fg="#fbf1c7", bg="#111111",
        ).pack(side=tk.LEFT)
        tk.Label(
            header, text="MIDI + UI actions",
            font=("DejaVu Sans", 11), fg="#a89984", bg="#111111",
        ).pack(side=tk.RIGHT)

        self._log_last_lbl = tk.Label(
            shell, textvariable=self.last_var,
            font=("DejaVu Sans Mono", 14, "bold"), fg="#fabd2f", bg="#111111",
            wraplength=760, justify=tk.LEFT, anchor="w",
        )
        self._log_last_lbl.pack(fill=tk.X, padx=10, pady=(4, 6))

        body = tk.Frame(shell, bg="#111111")
        body.pack(fill=tk.BOTH, expand=True, padx=8, pady=2)
        self.log = tk.Text(
            body, font=("DejaVu Sans Mono", 13),
            bg="#1d2021", fg="#ebdbb2", insertbackground="#ebdbb2",
            relief=tk.FLAT, state=tk.DISABLED,
        )
        scroll = ttk.Scrollbar(body, command=self.log.yview)
        self.log.configure(yscrollcommand=scroll.set)
        self.log.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        scroll.pack(side=tk.RIGHT, fill=tk.Y)

        footer = tk.Frame(shell, bg="#111111")
        footer.pack(fill=tk.X, padx=8, pady=8)
        self._mk_touch_btn(footer, "CLEAR LOG", self._clear_log, bg="#504945").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
        self._mk_touch_btn(footer, "ALL NOTES OFF", self._panic, bg="#9d0006").pack(
            side=tk.LEFT, expand=True, fill=tk.BOTH, padx=3, ipady=16
        )
