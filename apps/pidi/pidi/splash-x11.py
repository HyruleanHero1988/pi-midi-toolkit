#!/usr/bin/env python3
"""Full-screen PiDI splash for the gap between X start and midi-tone paint."""
from __future__ import annotations

import pathlib
import sys
import tkinter as tk

HERE = pathlib.Path(__file__).resolve().parent
# Deploy root is apps/pidi (or ~/midi-tone); package lives one level under it.
ROOT = HERE.parent
SPLASH = ROOT / "branding" / "pidi-splash.png"
BG = "#000000"


def main() -> int:
    root = tk.Tk()
    root.title("PiDI")
    root.configure(bg=BG)
    # Overrideredirect maps faster and avoids WM decorate flash
    try:
        root.overrideredirect(True)
    except Exception:
        pass
    root.attributes("-fullscreen", True)
    root.configure(cursor="none")
    root.lift()
    try:
        root.attributes("-topmost", True)
    except Exception:
        pass
    try:
        root.update_idletasks()
        root.update()
    except Exception:
        pass

    frame = tk.Frame(root, bg=BG)
    frame.pack(fill=tk.BOTH, expand=True)

    photo = None
    if SPLASH.is_file():
        try:
            # Tk 8.6+ supports PNG natively on most Pi / desktop builds
            photo = tk.PhotoImage(file=str(SPLASH))
        except Exception:
            try:
                from PIL import Image, ImageTk  # type: ignore

                im = Image.open(SPLASH)
                photo = ImageTk.PhotoImage(im)
            except Exception as exc:
                print(f"splash-x11: image load failed: {exc}", file=sys.stderr)

    if photo is not None:
        lbl = tk.Label(frame, image=photo, bg=BG, borderwidth=0, highlightthickness=0)
        lbl.image = photo  # keep ref
        lbl.place(relx=0.5, rely=0.5, anchor="center")
    else:
        tk.Label(
            frame,
            text="PiDI",
            font=("DejaVu Sans", 48, "bold"),
            fg="#00d4ff",
            bg=BG,
        ).place(relx=0.5, rely=0.45, anchor="center")
        tk.Label(
            frame,
            text="Raspberry Pi MIDI Toolkit",
            font=("DejaVu Sans", 14),
            fg="#00d4ff",
            bg=BG,
        ).place(relx=0.5, rely=0.55, anchor="center")

    root.update_idletasks()
    root.update()
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
