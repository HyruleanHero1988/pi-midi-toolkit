"""chrome UI mixin for MidiToneApp."""
from __future__ import annotations


class ChromeMixin:
    def _pack_screen_regions(
        self,
        parent: tk.Misc,
        *,
        bg: str = "#111111",
        header_padx: int = 8,
        header_pady: Tuple[int, int] = (8, 2),
        body_padx: int = 6,
        body_pady: int = 2,
        footer_padx: int = 8,
        footer_pady: int = 8,
    ) -> Tuple[tk.Frame, tk.Frame, tk.Frame]:
        """Return (header, body, footer) packed so chrome never falls off-screen.

        Pack order matters on short displays:
          1) footer (BOTTOM) — reserved first, always visible
          2) header (TOP)
          3) body (TOP, expand) — absorbs leftover height only

        Put all action buttons in ``footer`` (or nested frames inside it).
        Put scrollable / expanding content only in ``body``.
        """
        footer = tk.Frame(parent, bg=bg)
        footer.pack(side=tk.BOTTOM, fill=tk.X, padx=footer_padx, pady=footer_pady)

        header = tk.Frame(parent, bg=bg)
        header.pack(side=tk.TOP, fill=tk.X, padx=header_padx, pady=header_pady)

        body = tk.Frame(parent, bg=bg)
        body.pack(side=tk.TOP, fill=tk.BOTH, expand=True, padx=body_padx, pady=body_pady)
        return header, body, footer


    def _build_touch_scroll_area(
        self,
        parent: tk.Misc,
        *,
        show_rail: bool = False,
    ) -> Tuple[tk.Frame, tk.Canvas, tk.Frame, Dict[str, object]]:
        """Scroll canvas with finger-drag (TFT70 capacitive). Optional ▲/▼ rail."""
        wrap = tk.Frame(parent, bg="#111111")
        wrap.pack(fill=tk.BOTH, expand=True)

        drag: Dict[str, object] = {
            "start_x": 0,
            "start_y": 0,
            "dragging": False,
            "scanning": False,
            "grabber": None,
        }

        if show_rail:
            # Legacy resistive / accessibility: fat page buttons on the right
            rail = tk.Frame(wrap, bg="#111111", width=88)
            rail.pack(side=tk.RIGHT, fill=tk.Y, padx=(4, 0))
            rail.pack_propagate(False)

            def _scroll_step(direction: int) -> None:
                canvas.update_idletasks()
                top, bot = canvas.yview()
                visible = max(0.12, bot - top)
                step = visible * 0.9
                canvas.yview_moveto(max(0.0, min(1.0, top + direction * step)))

            up = self._mk_touch_btn(rail, "▲\nUP", lambda: _scroll_step(-1), bg="#504945")
            up.configure(font=("DejaVu Sans", 14, "bold"), pady=6)
            up.pack(side=tk.TOP, fill=tk.BOTH, expand=True, pady=(0, 3), ipady=10)

            down = self._mk_touch_btn(
                rail, "▼\nDOWN", lambda: _scroll_step(1), bg="#504945"
            )
            down.configure(font=("DejaVu Sans", 14, "bold"), pady=6)
            down.pack(side=tk.BOTTOM, fill=tk.BOTH, expand=True, pady=(3, 0), ipady=10)

            mid = tk.Frame(wrap, bg="#111111")
            mid.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        else:
            mid = wrap

        canvas = tk.Canvas(mid, bg="#111111", highlightthickness=0, bd=0)
        canvas.pack(fill=tk.BOTH, expand=True)
        drag["canvas"] = canvas

        inner = tk.Frame(canvas, bg="#111111")
        window_id = canvas.create_window((0, 0), window=inner, anchor="nw")

        def _on_inner_configure(_event: object = None) -> None:
            canvas.update_idletasks()
            req_h = max(1, int(inner.winfo_reqheight()))
            req_w = max(int(canvas.winfo_width()), int(inner.winfo_reqwidth()), 1)
            canvas.configure(scrollregion=(0, 0, req_w, req_h))

        def _on_canvas_configure(event: tk.Event) -> None:  # type: ignore[name-defined]
            canvas.itemconfigure(window_id, width=event.width)
            _on_inner_configure()

        inner.bind("<Configure>", _on_inner_configure)
        canvas.bind("<Configure>", _on_canvas_configure)

        def _canvas_xy(event: tk.Event) -> Tuple[int, int]:  # type: ignore[name-defined]
            return (
                int(event.x_root) - int(canvas.winfo_rootx()),
                int(event.y_root) - int(canvas.winfo_rooty()),
            )

        def _release_grab() -> None:
            grabber = drag.get("grabber")
            drag["grabber"] = None
            if grabber is None:
                return
            try:
                grabber.grab_release()  # type: ignore[union-attr]
            except tk.TclError:
                pass

        def _drag_start(event: tk.Event) -> str:  # type: ignore[name-defined]
            # Grab so B1-Motion keeps arriving after the finger leaves the
            # pressed voice button (Tk otherwise drops Motion on Buttons).
            _release_grab()
            drag["start_x"] = int(event.x_root)
            drag["start_y"] = int(event.y_root)
            drag["dragging"] = False
            drag["scanning"] = False
            try:
                event.widget.grab_set()
                drag["grabber"] = event.widget
            except tk.TclError:
                drag["grabber"] = None
            return "break"

        def _drag_move(event: tk.Event) -> str:  # type: ignore[name-defined]
            y = int(event.y_root)
            start_y = int(drag["start_y"])  # type: ignore[arg-type]
            if abs(y - start_y) >= TOUCH_SCROLL_THRESH_PX:
                drag["dragging"] = True
            if not drag["dragging"]:
                return "break"
            bbox = canvas.bbox("all")
            view_h = max(1, int(canvas.winfo_height()))
            content_h = (bbox[3] - bbox[1]) if bbox else view_h
            if content_h <= view_h:
                return "break"
            cx, cy = _canvas_xy(event)
            if not drag["scanning"]:
                # Anchor at the press point so the first dragto doesn't jump.
                sx = int(drag["start_x"]) - int(canvas.winfo_rootx())  # type: ignore[arg-type]
                sy = int(drag["start_y"]) - int(canvas.winfo_rooty())  # type: ignore[arg-type]
                canvas.scan_mark(sx, sy)
                drag["scanning"] = True
            canvas.scan_dragto(cx, cy, gain=1)
            return "break"

        def _drag_end(event: tk.Event) -> str:  # type: ignore[name-defined]
            del event
            _release_grab()
            drag["scanning"] = False
            return "break"

        def _bind_empty_drag(widget: tk.Misc) -> None:
            widget.bind("<ButtonPress-1>", _drag_start)
            widget.bind("<B1-Motion>", _drag_move)
            widget.bind("<ButtonRelease-1>", _drag_end)

        _bind_empty_drag(canvas)
        _bind_empty_drag(inner)

        def _on_wheel(event: tk.Event) -> str:  # type: ignore[name-defined]
            delta = int(getattr(event, "delta", 0) or 0)
            if delta == 0:
                num = int(getattr(event, "num", 0) or 0)
                steps = -1 if num == 4 else 1 if num == 5 else 0
            else:
                steps = -1 if delta > 0 else 1
            if steps:
                canvas.yview_scroll(steps, "units")
            return "break"

        canvas.bind("<MouseWheel>", _on_wheel)
        canvas.bind("<Button-4>", _on_wheel)
        canvas.bind("<Button-5>", _on_wheel)
        inner.bind("<MouseWheel>", _on_wheel)
        inner.bind("<Button-4>", _on_wheel)
        inner.bind("<Button-5>", _on_wheel)
        drag["_move"] = _drag_move
        drag["_start"] = _drag_start
        drag["_end"] = _drag_end
        drag["_release_grab"] = _release_grab
        drag["_bind_tree"] = lambda widget: self._bind_touch_scroll_tree(widget, drag)
        return wrap, canvas, inner, drag


    def _bind_touch_scroll_tree(self, widget: tk.Misc, drag: Dict[str, object]) -> None:
        """Let nested labels/frames drag-scroll; buttons already have their own bind."""
        starter = drag.get("_start")
        mover = drag.get("_move")
        ender = drag.get("_end")
        if not isinstance(widget, tk.Button) and callable(starter) and callable(mover) and callable(ender):
            widget.bind("<ButtonPress-1>", starter)  # type: ignore[arg-type]
            widget.bind("<B1-Motion>", mover)  # type: ignore[arg-type]
            widget.bind("<ButtonRelease-1>", ender)  # type: ignore[arg-type]
        for child in widget.winfo_children():
            self._bind_touch_scroll_tree(child, drag)


    def _arm_overlay_guard(self, sec: float = 0.40) -> None:
        """Ignore the finger-up that opened a grid so it cannot pick a tile."""
        self._picker_ignore_until = time.monotonic() + float(sec)


    def _mk_scroll_select_btn(
        self,
        parent: tk.Misc,
        text: str,
        command,
        drag: Dict[str, object],
        bg: str = "#3c3836",
    ) -> tk.Button:
        """Grid button: short tap selects; finger drag scrolls the parent canvas."""
        btn = tk.Button(
            parent, text=text,
            font=("DejaVu Sans", 14, "bold"), fg="#fbf1c7", bg=bg,
            activeforeground="#fbf1c7", activebackground=bg,
            relief=tk.FLAT, bd=0, padx=8, pady=12, cursor="hand2",
            takefocus=0,
        )

        def _press(event: tk.Event) -> str:  # type: ignore[name-defined]
            starter = drag.get("_start")
            if callable(starter):
                return starter(event)  # type: ignore[misc]
            return "break"

        def _move(event: tk.Event) -> str:  # type: ignore[name-defined]
            mover = drag.get("_move")
            if callable(mover):
                return mover(event)  # type: ignore[misc]
            return "break"

        def _release(event: tk.Event) -> str:  # type: ignore[name-defined]
            was_drag = bool(drag.get("dragging"))
            ender = drag.get("_end")
            if callable(ender):
                ender(event)  # type: ignore[misc]
            else:
                releaser = drag.get("_release_grab")
                if callable(releaser):
                    releaser()  # type: ignore[misc]
            if was_drag:
                return "break"
            now = time.monotonic()
            if now < float(getattr(self, "_picker_ignore_until", 0.0) or 0.0):
                return "break"
            last = getattr(btn, "_last_fire", 0.0)
            if now - last < 0.18:
                return "break"
            btn._last_fire = now  # type: ignore[attr-defined]
            command()
            return "break"

        btn.bind("<ButtonPress-1>", _press)
        btn.bind("<B1-Motion>", _move)
        btn.bind("<ButtonRelease-1>", _release)
        return btn
