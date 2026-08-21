"""Reusable Switch-style touch keyboard overlay for PiDI.

Opens over ``mode_host`` (or any parent), types into an Entry, and calls
``on_submit(text)`` / ``on_cancel()``. Designed for 800x480 TFT kiosks.

Layout follows the Nintendo Switch OSK (aligned grid, not staggered):

* ``1 2 3 4 5 6 7 8 9 0 - ⌫``
* ``q w e r t y u i o p /``
* ``a s d f g h j k l : '``
* ``z x c v b n m , . ? !``
* ``⇧   #+=   Space   OK``

``#+=`` flips to a symbols page; ``ABC`` returns. Shift uppercases letters
(one-shot). Mode changes lift a prebuilt page (or retarget labels) so the
grid never blanks while Tk recreates widgets.
"""
from __future__ import annotations

import tkinter as tk
from dataclasses import dataclass
from typing import Callable, List, Optional, Sequence, Tuple


# (label, action, column_span) — action is inserted text or a command token.
Key = Tuple[str, str, int]

ACT_SHIFT = "shift"
ACT_SYM = "sym"
ACT_ABC = "abc"
ACT_BACK = "back"
ACT_SPACE = "space"
ACT_OK = "ok"
ACT_PAD = "pad"

COLS = 12


def _abc_rows(*, shift: bool) -> List[List[Key]]:
    def L(ch: str) -> Key:
        shown = ch.upper() if shift and ch.isalpha() else ch
        inserted = ch.upper() if shift and ch.isalpha() else ch
        return (shown, inserted, 1)

    return [
        [L(c) for c in "1234567890-"] + [("⌫", ACT_BACK, 1)],
        [L(c) for c in "qwertyuiop/"] + [("", ACT_PAD, 1)],
        [L(c) for c in "asdfghjkl:'"] + [("", ACT_PAD, 1)],
        [L(c) for c in "zxcvbnm,.?!"] + [("", ACT_PAD, 1)],
        [
            ("⇧", ACT_SHIFT, 2),
            ("#+=", ACT_SYM, 2),
            ("Space", ACT_SPACE, 5),
            ("OK", ACT_OK, 3),
        ],
    ]


def _sym_rows() -> List[List[Key]]:
    """Extra punctuation / symbols page (still Switch-like top number row)."""
    return [
        [("1", "1", 1), ("2", "2", 1), ("3", "3", 1), ("4", "4", 1), ("5", "5", 1),
         ("6", "6", 1), ("7", "7", 1), ("8", "8", 1), ("9", "9", 1), ("0", "0", 1),
         ("_", "_", 1), ("⌫", ACT_BACK, 1)],
        [("!", "!", 1), ("@", "@", 1), ("#", "#", 1), ("$", "$", 1), ("%", "%", 1),
         ("^", "^", 1), ("&", "&", 1), ("*", "*", 1), ("(", "(", 1), (")", ")", 1),
         ("+", "+", 1), ("=", "=", 1)],
        [("~", "~", 1), ("`", "`", 1), ("{", "{", 1), ("}", "}", 1), ("[", "[", 1),
         ("]", "]", 1), ("\\", "\\", 1), ("|", "|", 1), (";", ";", 1), ("\"", "\"", 1),
         ("<", "<", 1), (">", ">", 1)],
        [
            ("ABC", ACT_ABC, 3),
            ("Space", ACT_SPACE, 6),
            ("OK", ACT_OK, 3),
        ],
    ]


PRINTABLE_ASCII = frozenset(chr(c) for c in range(0x20, 0x7F))


def reachable_characters() -> frozenset:
    """Every character this keyboard can insert (for tests / audits)."""
    found: set[str] = {" "}
    skip = {ACT_SHIFT, ACT_SYM, ACT_ABC, ACT_BACK, ACT_SPACE, ACT_OK, ACT_PAD}
    for shift in (False, True):
        for row in _abc_rows(shift=shift):
            for _label, action, _span in row:
                if action not in skip:
                    found.add(action)
    for row in _sym_rows():
        for _label, action, _span in row:
            if action not in skip:
                found.add(action)
    return frozenset(found)


def missing_printable_ascii(chars: Optional[Sequence[str]] = None) -> List[str]:
    have = set(chars) if chars is not None else set(reachable_characters())
    return sorted(PRINTABLE_ASCII - have)


@dataclass
class TouchKeyboardOptions:
    title: str = "Keyboard"
    subtitle: str = ""
    initial: str = ""
    password: bool = False
    submit_label: str = "OK"
    cancel_label: str = ""  # empty → no footer cancel (use global ←)
    max_length: int = 0  # 0 = unlimited


@dataclass
class _KeySlot:
    """Mutable action target so Shift can retarget without rebinding."""

    btn: tk.Button
    action: str
    base: str = ""  # lowercase letter for Shift retarget; empty if N/A


class TouchKeyboardOverlay:
    """Full-screen Switch-style keyboard hosted on a parent frame."""

    def __init__(
        self,
        host: tk.Misc,
        *,
        mk_button: Callable[..., tk.Button],
        on_submit: Callable[[str], None],
        on_cancel: Optional[Callable[[], None]] = None,
        options: Optional[TouchKeyboardOptions] = None,
    ) -> None:
        self._host = host
        self._mk_button = mk_button
        self._on_submit = on_submit
        self._on_cancel = on_cancel or (lambda: None)
        self._opts = options or TouchKeyboardOptions()
        self._shift = False
        self._sym = False
        self._show_password = not self._opts.password
        self.frame: Optional[tk.Frame] = None
        self._entry: Optional[tk.Entry] = None
        self._keys: Optional[tk.Frame] = None
        self._status: Optional[tk.StringVar] = None
        self._show_btn: Optional[tk.Button] = None
        self._page_abc: Optional[tk.Frame] = None
        self._page_sym: Optional[tk.Frame] = None
        self._abc_slots: List[_KeySlot] = []
        self._shift_btn: Optional[tk.Button] = None

    def open(self) -> None:
        self.close()
        opts = self._opts
        frame = tk.Frame(self._host, bg="#111111")
        frame.pack(fill=tk.BOTH, expand=True)
        self.frame = frame

        frame.columnconfigure(0, weight=1)
        frame.rowconfigure(3, weight=1)

        header = tk.Frame(frame, bg="#111111")
        header.grid(row=0, column=0, sticky="ew", padx=8, pady=(6, 2))
        tk.Label(
            header,
            text=opts.title,
            font=("DejaVu Sans", 16, "bold"),
            fg="#fbf1c7",
            bg="#111111",
        ).pack(side=tk.LEFT)
        if opts.subtitle:
            tk.Label(
                header,
                text=opts.subtitle[:32],
                font=("DejaVu Sans", 12),
                fg="#a89984",
                bg="#111111",
            ).pack(side=tk.RIGHT)

        self._status = tk.StringVar(value="")
        tk.Label(
            frame,
            textvariable=self._status,
            font=("DejaVu Sans", 11, "bold"),
            fg="#fabd2f",
            bg="#111111",
            anchor="w",
        ).grid(row=1, column=0, sticky="ew", padx=10, pady=(0, 2))

        entry_row = tk.Frame(frame, bg="#111111")
        entry_row.grid(row=2, column=0, sticky="ew", padx=8, pady=2)
        self._entry = tk.Entry(
            entry_row,
            font=("DejaVu Sans Mono", 16),
            bg="#1d2021",
            fg="#fbf1c7",
            insertbackground="#fbf1c7",
            relief=tk.FLAT,
            show="*" if opts.password and not self._show_password else "",
        )
        self._entry.pack(side=tk.LEFT, expand=True, fill=tk.BOTH, ipady=6)
        if opts.initial:
            self._entry.insert(0, opts.initial)
        self._entry.focus_set()

        if opts.password:
            self._show_btn = self._mk_button(
                entry_row,
                "SHOW",
                self._toggle_show,
                bg="#458588",
            )
            self._show_btn.pack(side=tk.LEFT, padx=(6, 0), ipady=6)

        keys = tk.Frame(frame, bg="#111111")
        keys.grid(row=3, column=0, sticky="nsew", padx=4, pady=2)
        self._keys = keys

        # Prebuild both pages once; mode changes only lift / retarget labels.
        self._page_abc = tk.Frame(keys, bg="#111111")
        self._page_sym = tk.Frame(keys, bg="#111111")
        for page in (self._page_abc, self._page_sym):
            page.place(relx=0, rely=0, relwidth=1, relheight=1)
            for c in range(COLS):
                page.columnconfigure(c, weight=1)

        self._abc_slots = []
        self._shift_btn = None
        self._build_page(self._page_abc, _abc_rows(shift=False), track_abc=True)
        self._build_page(self._page_sym, _sym_rows(), track_abc=False)

        # Optional cancel strip — Wi‑Fi uses the global ← instead.
        if opts.cancel_label:
            footer = tk.Frame(frame, bg="#111111")
            footer.grid(row=4, column=0, sticky="ew", padx=8, pady=(2, 8))
            self._mk_button(
                footer, opts.cancel_label, self._cancel, bg="#9d0006"
            ).pack(side=tk.LEFT, expand=True, fill=tk.BOTH, padx=2, ipady=10)

        self._show_active_page()

    def close(self) -> None:
        if self.frame is not None:
            try:
                self.frame.destroy()
            except tk.TclError:
                pass
        self.frame = None
        self._entry = None
        self._keys = None
        self._show_btn = None
        self._page_abc = None
        self._page_sym = None
        self._abc_slots = []
        self._shift_btn = None
        self._shift = False
        self._sym = False

    def set_status(self, text: str) -> None:
        if self._status is not None:
            self._status.set(text)

    def _toggle_show(self) -> None:
        self._show_password = not self._show_password
        if self._entry is not None:
            self._entry.configure(show="" if self._show_password else "*")
        if self._show_btn is not None:
            self._show_btn.configure(text="HIDE" if self._show_password else "SHOW")

    def _key_bg(self, action: str) -> str:
        if action == ACT_SHIFT and self._shift:
            return "#689d6a"
        if action == ACT_OK:
            return "#689d6a"
        if action in (ACT_BACK, ACT_SYM, ACT_ABC, ACT_SHIFT):
            return "#504945"
        return "#3c3836"

    def _build_page(
        self, page: tk.Frame, rows: List[List[Key]], *, track_abc: bool
    ) -> None:
        submit = self._opts.submit_label
        for r, row in enumerate(rows):
            page.rowconfigure(r, weight=1)
            col = 0
            for label, action, span in row:
                span = max(1, span)
                if action == ACT_PAD:
                    pad = tk.Frame(page, bg="#111111")
                    pad.grid(
                        row=r, column=col, columnspan=span, sticky="nsew", padx=1, pady=1
                    )
                    col += span
                    continue
                shown = submit if action == ACT_OK else label
                base = (
                    action.lower()
                    if track_abc and len(action) == 1 and action.isalpha()
                    else ""
                )
                # Slot filled after the button exists; fire reads slot.action.
                slot_box: List[_KeySlot] = []

                def _fire(box: List[_KeySlot] = slot_box) -> None:
                    if box:
                        self._handle(box[0].action)

                btn = self._mk_button(page, shown, _fire, bg=self._key_bg(action))
                btn.configure(font=("DejaVu Sans", 12, "bold"))
                btn.grid(
                    row=r,
                    column=col,
                    columnspan=span,
                    sticky="nsew",
                    padx=1,
                    pady=1,
                )
                slot = _KeySlot(btn=btn, action=action, base=base)
                slot_box.append(slot)
                if track_abc:
                    self._abc_slots.append(slot)
                    if action == ACT_SHIFT:
                        self._shift_btn = btn
                col += span

    def _show_active_page(self) -> None:
        if self._page_abc is None or self._page_sym is None:
            return
        if self._sym:
            self._page_sym.lift()
        else:
            self._page_abc.lift()

    def _apply_shift_labels(self) -> None:
        """Retarget abc letter keys in place — no destroy/rebuild."""
        for slot in self._abc_slots:
            if not slot.base:
                continue
            ch = slot.base.upper() if self._shift else slot.base
            slot.action = ch
            try:
                slot.btn.configure(text=ch)
            except tk.TclError:
                pass
        if self._shift_btn is not None:
            try:
                self._shift_btn.configure(bg=self._key_bg(ACT_SHIFT), activebackground=self._key_bg(ACT_SHIFT))
            except tk.TclError:
                pass

    def _handle(self, action: str) -> None:
        if action == ACT_PAD:
            return
        if action == ACT_SHIFT:
            self._shift = not self._shift
            self._apply_shift_labels()
            return
        if action == ACT_SYM:
            self._sym = True
            if self._shift:
                self._shift = False
                self._apply_shift_labels()
            self._show_active_page()
            return
        if action == ACT_ABC:
            self._sym = False
            self._shift = False
            self._apply_shift_labels()
            self._show_active_page()
            return
        if action == ACT_BACK:
            self._type("\b")
            return
        if action == ACT_SPACE:
            self._type(" ")
            return
        if action == ACT_OK:
            self._submit()
            return
        self._type(action)
        if self._shift and not self._sym and len(action) == 1 and action.isalpha():
            self._shift = False
            self._apply_shift_labels()

    def _type(self, ch: str) -> None:
        entry = self._entry
        if entry is None:
            return
        if ch == "\b":
            cur = entry.get()
            entry.delete(0, tk.END)
            entry.insert(0, cur[:-1])
            return
        max_len = int(self._opts.max_length or 0)
        if max_len and len(entry.get()) >= max_len:
            return
        entry.insert(tk.END, ch)

    def _submit(self) -> None:
        text = self._entry.get() if self._entry is not None else ""
        self._on_submit(text)

    def _cancel(self) -> None:
        self._on_cancel()
