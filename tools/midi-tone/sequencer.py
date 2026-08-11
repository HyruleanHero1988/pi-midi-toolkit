#!/usr/bin/env python3
"""
Overdub sequencer — the 808-style "keep adding to the beat" recorder.

Two halves live here:

* A pure model (`LoopEvent`, `SeqLayer`, `Sequence`) with no threads, no audio
  and no Tk, so the timing rules can be unit-tested on any machine.
* `OverdubSequencer`, which wraps the model in a playback thread and talks to
  the soft-synth through the same `note_on` / `note_off` calls the rest of the
  kiosk uses.

Model in one paragraph: the first take is the **backbone**. Trimmed of dead
air, its length becomes the cycle every later take is measured against. After
that the sequence loops forever and each new take lands in a **pending layer**
you can KEEP (flattened onto the stack, undoable) or DROP. Layers are tiled:
a one-cycle layer repeats inside a four-cycle sequence, which is what lets a
long take stretch the sequence without re-recording the groove underneath.
"""

from __future__ import annotations

import math
import threading
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Sequence as Seq, Tuple

MAX_CYCLES = 8
# Guard against a stuck note when a recorded note-off went missing.
HELD_NOTE_GRACE = 0.05


@dataclass
class LoopEvent:
    t: float  # seconds from take/sequence start
    on: bool
    channel: int
    note: int
    velocity: int


def trim_loop_take(
    events: List[LoopEvent],
    *,
    default_gap: float = 0.35,
    min_gap: float = 0.05,
    max_gap: float = 2.0,
) -> Tuple[List[LoopEvent], float]:
    """
    Trim leading/trailing dead space from a free-timing take.

    - Shift so the first note-on starts at t=0 (drop pre-roll before the groove).
    - Trailing silence after the last note-on is capped to the largest gap
      between consecutive note-ons (so STOP lag doesn't inflate the loop).
    - Note-offs after the last hit are still kept; trail is measured from ons.

    Used by every free-timing take in the kiosk: sequencer backbone and phrase
    pads both go through here so a slow finger on STOP never costs a dead bar.
    """
    if not events:
        return [], 0.0
    ons = sorted(e.t for e in events if e.on)
    if not ons:
        # Degenerate: only note-offs — keep relative timing, short length
        t0 = min(e.t for e in events)
        shifted = [
            LoopEvent(
                t=max(0.0, e.t - t0),
                on=e.on,
                channel=e.channel,
                note=e.note,
                velocity=e.velocity,
            )
            for e in events
        ]
        length = max(e.t for e in shifted) + min_gap
        return shifted, max(min_gap, length)

    t0 = ons[0]
    gaps = [ons[i + 1] - ons[i] for i in range(len(ons) - 1) if ons[i + 1] > ons[i]]
    if gaps:
        trail = max(min_gap, min(max_gap, max(gaps)))
    else:
        # Single hit: small default pad (not the whole time spent hitting STOP)
        trail = max(min_gap, min(max_gap, default_gap))

    last_on = ons[-1]
    last_ev = max(e.t for e in events)
    # Loop end from first hit: last onset + trail, but never cut off a later note-off
    end_abs = max(last_on + trail, last_ev + 0.01)
    length = max(min_gap, end_abs - t0)

    shifted = [
        LoopEvent(
            t=max(0.0, e.t - t0),
            on=e.on,
            channel=e.channel,
            note=e.note,
            velocity=e.velocity,
        )
        for e in events
        if e.t >= t0 - 1e-6
    ]
    # Drop events that fall past the trimmed end (shouldn't happen often)
    shifted = [e for e in shifted if e.t <= length + 1e-6]
    if not shifted:
        return [], 0.0
    return shifted, float(length)


def close_open_notes(events: List[LoopEvent], span: float) -> List[LoopEvent]:
    """Give every note-on a note-off inside `span` (a take can end mid-note)."""
    if span <= 0.0:
        return list(events)
    held: Dict[Tuple[int, int], LoopEvent] = {}
    for ev in sorted(events, key=_order_key):
        key = (ev.channel, ev.note)
        if ev.on:
            held[key] = ev
        else:
            held.pop(key, None)
    if not held:
        return list(events)
    end = max(0.0, span - 1e-3)
    out = list(events)
    for (channel, note), on_ev in held.items():
        out.append(
            LoopEvent(
                t=max(on_ev.t + 1e-3, end),
                on=False,
                channel=channel,
                note=note,
                velocity=0,
            )
        )
    return out


def _order_key(ev: LoopEvent) -> Tuple[float, int, int, int]:
    # Note-offs first at the same instant so a re-hit of the same note retriggers.
    return (ev.t, 1 if ev.on else 0, ev.channel, ev.note)


def tile_layer(layer: "SeqLayer", cycle_len: float, cycles: int) -> List[LoopEvent]:
    """Repeat a layer across the sequence (a 1-cycle groove under a 4-cycle take)."""
    if cycle_len <= 0.0 or cycles <= 0 or not layer.events:
        return []
    span = max(1, min(int(layer.span), cycles))
    repeats = max(1, cycles // span)
    period = span * cycle_len
    out: List[LoopEvent] = []
    for rep in range(repeats):
        offset = rep * period
        for ev in layer.events:
            if ev.t >= period - 1e-9 and rep < repeats - 1:
                continue
            out.append(
                LoopEvent(
                    t=ev.t + offset,
                    on=ev.on,
                    channel=ev.channel,
                    note=ev.note,
                    velocity=ev.velocity,
                )
            )
    return out


@dataclass
class SeqLayer:
    """One take. `span` is its length in backbone cycles."""

    events: List[LoopEvent] = field(default_factory=list)
    span: int = 1
    label: str = ""

    def copy(self) -> "SeqLayer":
        return SeqLayer(
            events=[
                LoopEvent(t=e.t, on=e.on, channel=e.channel, note=e.note, velocity=e.velocity)
                for e in self.events
            ],
            span=int(self.span),
            label=str(self.label),
        )

    def is_empty(self) -> bool:
        return not self.events


class Sequence:
    """Backbone + flattened layers + the pending take, with no notion of time."""

    def __init__(self) -> None:
        self.cycle_len: float = 0.0
        self.cycles: int = 1
        self.layers: List[SeqLayer] = []
        self.pending: Optional[SeqLayer] = None

    def is_empty(self) -> bool:
        return self.cycle_len <= 0.0 or not self.layers

    def total_len(self) -> float:
        if self.cycle_len <= 0.0:
            return 0.0
        return self.cycle_len * max(1, self.cycles)

    def max_span(self) -> int:
        spans = [max(1, int(layer.span)) for layer in self.layers]
        if self.pending is not None:
            spans.append(max(1, int(self.pending.span)))
        return max(spans) if spans else 1

    def event_count(self) -> int:
        n = sum(len(layer.events) for layer in self.layers)
        if self.pending is not None:
            n += len(self.pending.events)
        return n

    def set_backbone(self, events: List[LoopEvent], length: float) -> None:
        self.cycle_len = max(0.0, float(length))
        self.cycles = 1
        self.layers = [SeqLayer(events=list(events), span=1, label="backbone")]
        self.pending = None

    def keep_pending(self) -> bool:
        """Flatten the pending take onto the stack; it stays undoable."""
        layer = self.pending
        self.pending = None
        if layer is None or layer.is_empty():
            return False
        span = max(1, min(MAX_CYCLES, int(layer.span)))
        if span > self.cycles:
            self.cycles = span
        layer.span = span
        layer.label = f"layer {len(self.layers)}"
        self.layers.append(layer)
        return True

    def drop_pending(self) -> bool:
        had = self.pending is not None and not self.pending.is_empty()
        self.pending = None
        return had

    def undo_layer(self) -> Optional[SeqLayer]:
        """Remove the newest flattened layer. The backbone is only cleared by CLEAR."""
        if len(self.layers) <= 1:
            return None
        layer = self.layers.pop()
        self.cycles = max(1, min(self.cycles, max(self.max_span(), 1)))
        return layer

    def set_cycles(self, cycles: int) -> bool:
        want = max(1, min(MAX_CYCLES, int(cycles)))
        if self.cycle_len <= 0.0:
            return False
        if want < self.max_span():
            return False
        if want == self.cycles:
            return False
        self.cycles = want
        return True

    def clear(self) -> None:
        self.cycle_len = 0.0
        self.cycles = 1
        self.layers = []
        self.pending = None

    def schedule(self, *, include_pending: bool = True) -> List[LoopEvent]:
        """Every layer tiled onto one pass of the sequence, in play order."""
        if self.cycle_len <= 0.0:
            return []
        out: List[LoopEvent] = []
        for layer in self.layers:
            out.extend(tile_layer(layer, self.cycle_len, self.cycles))
        if include_pending and self.pending is not None:
            out.extend(tile_layer(self.pending, self.cycle_len, self.cycles))
        out.sort(key=_order_key)
        return out


def cycles_for_take(take_len: float, cycle_len: float, *, minimum: int = 1) -> int:
    """How many backbone cycles a take of `take_len` needs (rounded up)."""
    if cycle_len <= 0.0:
        return max(1, minimum)
    needed = int(math.ceil((take_len - 1e-3) / cycle_len))
    return max(1, minimum, min(MAX_CYCLES, needed))


# Transport states
SEQ_EMPTY = "empty"
SEQ_REC_BACKBONE = "rec_backbone"
SEQ_STOPPED = "stopped"
SEQ_PLAYING = "playing"
SEQ_OVERDUB = "overdub"
SEQ_REVIEW = "review"


class OverdubSequencer:
    """
    Record a backbone, then keep layering over it while it plays.

    Buttons map to: `toggle_record` (backbone → overdub), `keep`, `drop`,
    `undo`, `clear`, `toggle_play`, `set_extend`, `set_cycles`.
    """

    def __init__(
        self,
        engine,
        emit: Callable[[tuple], None],
        *,
        autoplay: bool = True,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self._engine = engine
        self._emit = emit
        self._autoplay = bool(autoplay)
        self._now = clock
        self._lock = threading.RLock()
        self._seq = Sequence()
        self._state = SEQ_EMPTY
        self._extend = False
        # Recording bookkeeping
        self._rec_t0 = 0.0
        self._rec_events: List[LoopEvent] = []
        self._overdub_start_phase = 0.0
        # Playback bookkeeping
        self._pass_t0 = 0.0
        self._stop_play = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._held: Dict[Tuple[int, int], float] = {}

    # ---------------------------------------------------------------- status

    def state(self) -> str:
        with self._lock:
            return self._state

    def is_recording(self) -> bool:
        with self._lock:
            return self._state in (SEQ_REC_BACKBONE, SEQ_OVERDUB)

    def is_playing(self) -> bool:
        with self._lock:
            return self._state in (SEQ_PLAYING, SEQ_OVERDUB, SEQ_REVIEW)

    def _transport_alive(self) -> bool:
        thread = self._thread
        return thread is not None and thread.is_alive() and not self._stop_play.is_set()

    def has_pending(self) -> bool:
        with self._lock:
            return self._seq.pending is not None and not self._seq.pending.is_empty()

    def extend_mode(self) -> bool:
        with self._lock:
            return self._extend

    def status(self) -> Dict[str, object]:
        with self._lock:
            seq = self._seq
            pending = seq.pending
            return {
                "state": self._state,
                "recording": self._state in (SEQ_REC_BACKBONE, SEQ_OVERDUB),
                "playing": self._state in (SEQ_PLAYING, SEQ_OVERDUB, SEQ_REVIEW),
                "cycle_len": float(seq.cycle_len),
                "cycles": int(seq.cycles),
                "length": float(seq.total_len()),
                "layers": len(seq.layers),
                "events": seq.event_count(),
                "pending": len(pending.events) if pending is not None else 0,
                "take": len(self._rec_events),
                "extend": self._extend,
            }

    def status_line(self) -> str:
        st = self.status()
        state = st["state"]
        cycles = int(st["cycles"])
        length = float(st["length"])
        layers = int(st["layers"])
        span = f"{length:.2f}s" + (f" · {cycles} cycles" if cycles > 1 else "")
        if state == SEQ_EMPTY:
            return "Empty — tap REC and play the backbone; it sets the loop length."
        if state == SEQ_REC_BACKBONE:
            return f"● RECORDING BACKBONE — {int(st['take'])} events · tap REC to close the loop"
        if state == SEQ_OVERDUB:
            return (
                f"● OVERDUB over {span} — {int(st['pending'])} new · "
                f"KEEP to flatten, DROP to abandon"
            )
        if state == SEQ_REVIEW:
            return (
                f"◉ REVIEW layer — {int(st['pending'])} new events over {span} · "
                f"KEEP or DROP"
            )
        if state == SEQ_PLAYING:
            return f"▶ PLAYING {span} · {layers} layer(s) · tap REC to overdub"
        return f"■ STOPPED {span} · {layers} layer(s) · PLAY to run, REC to overdub"

    def layer_line(self) -> str:
        with self._lock:
            seq = self._seq
            if not seq.layers:
                return "no layers yet"
            parts = [f"backbone {len(seq.layers[0].events)}ev"]
            for i, layer in enumerate(seq.layers[1:], start=1):
                tail = f"×{layer.span}" if layer.span > 1 else ""
                parts.append(f"L{i} {len(layer.events)}ev{tail}")
            if seq.pending is not None and seq.pending.events:
                parts.append(f"[pending {len(seq.pending.events)}ev]")
            return " · ".join(parts)

    def snapshot(self) -> Tuple[List[LoopEvent], float]:
        """Flat event list for exporting (Songs `SAVE SEQ`)."""
        with self._lock:
            return self._seq.schedule(), self._seq.total_len()

    # ------------------------------------------------------------- recording

    def toggle_record(self) -> str:
        """One button, context-sensitive. Returns the action taken."""
        with self._lock:
            state = self._state
        if state == SEQ_REC_BACKBONE:
            self.stop_record()
            return "backbone_done"
        if state == SEQ_OVERDUB:
            self.stop_record()
            return "overdub_done"
        if state == SEQ_EMPTY:
            self.start_backbone()
            return "backbone"
        self.start_overdub()
        return "overdub"

    def start_backbone(self) -> bool:
        self.stop_playback()
        with self._lock:
            self._seq.clear()
            self._rec_events = []
            self._rec_t0 = self._now()
            self._state = SEQ_REC_BACKBONE
        self._emit(("log", "SEQ backbone REC", False))
        return True

    def start_overdub(self) -> bool:
        with self._lock:
            if self._seq.is_empty():
                return False
            if self._state == SEQ_OVERDUB:
                return True
            # A pending take that was never resolved is kept — you asked for
            # another pass, not for the previous one to vanish.
            if self._seq.pending is not None and self._seq.pending.events:
                self._seq.keep_pending()
        if not self.is_playing():
            self.start_playback()
        with self._lock:
            now = self._now()
            self._rec_events = []
            self._rec_t0 = now
            total = self._seq.total_len()
            phase = (now - self._pass_t0) if total > 0.0 else 0.0
            if total > 0.0:
                phase = max(0.0, min(total - 1e-4, phase))
            self._overdub_start_phase = phase
            self._state = SEQ_OVERDUB
            self._seq.pending = SeqLayer(events=[], span=self._seq.cycles, label="pending")
        self._emit(("log", "SEQ overdub REC", False))
        return True

    def stop_record(self) -> bool:
        with self._lock:
            state = self._state
        if state == SEQ_REC_BACKBONE:
            return self._finish_backbone()
        if state == SEQ_OVERDUB:
            return self._finish_overdub()
        return False

    def _finish_backbone(self) -> bool:
        with self._lock:
            events = list(self._rec_events)
            self._rec_events = []
            if not events:
                self._state = SEQ_EMPTY
                self._seq.clear()
                self._emit(("log", "SEQ backbone empty — nothing recorded", False))
                return False
            trimmed, length = trim_loop_take(events)
            trimmed = close_open_notes(trimmed, length)
            trimmed.sort(key=_order_key)
            self._seq.set_backbone(trimmed, length)
            self._state = SEQ_STOPPED
        self._emit(
            (
                "log",
                f"SEQ backbone {length:.2f}s ({len(trimmed)} events) — loop length locked",
                False,
            )
        )
        if self._autoplay:
            self.start_playback()
        return True

    def _finish_overdub(self) -> bool:
        with self._lock:
            events = list(self._rec_events)
            self._rec_events = []
            layer = self._seq.pending
            if layer is None or not events:
                self._seq.pending = None
                self._state = SEQ_PLAYING if self._transport_alive() else SEQ_STOPPED
                self._emit(("log", "SEQ overdub empty", False))
                return False
            span_len = layer.span * self._seq.cycle_len
            layer.events = close_open_notes(list(events), span_len)
            layer.events.sort(key=_order_key)
            self._state = SEQ_REVIEW
            n = len(layer.events)
        self._emit(("log", f"SEQ overdub take: {n} events — KEEP or DROP", False))
        self._emit(("seq",))
        return True

    def record_note(self, on: bool, channel: int, note: int, velocity: int) -> None:
        """Called from the MIDI/touch path for every note the player triggers."""
        with self._lock:
            state = self._state
            if state not in (SEQ_REC_BACKBONE, SEQ_OVERDUB):
                return
            now = self._now()
            if state == SEQ_REC_BACKBONE:
                t = now - self._rec_t0
            else:
                t = self._overdub_position(now)
            ev = LoopEvent(
                t=max(0.0, t),
                on=bool(on),
                channel=channel & 0x0F,
                note=note & 0x7F,
                velocity=max(1, min(127, int(velocity))) if on else 0,
            )
            self._rec_events.append(ev)
            layer = self._seq.pending
            if state == SEQ_OVERDUB and layer is not None:
                # Live pending layer: heard from the next pass, 808-style.
                layer.events.append(ev)
                layer.events.sort(key=_order_key)
                if self._extend:
                    layer.span = cycles_for_take(
                        max(e.t for e in layer.events) + 1e-3,
                        self._seq.cycle_len,
                        minimum=self._seq.cycles,
                    )

    def _overdub_position(self, now: float) -> float:
        """Where in the sequence this hit lands (locked while `self._lock` is held)."""
        seq = self._seq
        total = seq.total_len()
        if total <= 0.0:
            return 0.0
        if self._extend:
            # Straight line from where the take started: a long take grows the
            # sequence to whole backbone cycles instead of folding back on itself.
            t = self._overdub_start_phase + (now - self._rec_t0)
            limit = MAX_CYCLES * seq.cycle_len
            return max(0.0, min(limit - 1e-3, t))
        phase = now - self._pass_t0
        if phase < 0.0 or phase >= total:
            phase = phase % total
        return max(0.0, min(total - 1e-4, phase))

    # ------------------------------------------------------------ layer ops

    def keep(self) -> bool:
        """Flatten the pending take onto the backbone stack (undoable)."""
        if self.state() == SEQ_OVERDUB:
            self._finish_overdub()
        with self._lock:
            grew_from = self._seq.cycles
            ok = self._seq.keep_pending()
            cycles = self._seq.cycles
            layers = len(self._seq.layers)
            if ok and self._state == SEQ_REVIEW:
                self._state = SEQ_PLAYING if self._transport_alive() else SEQ_STOPPED
        if not ok:
            self._emit(("log", "SEQ nothing to keep", False))
            return False
        grew = f" · sequence now {cycles} cycles" if cycles != grew_from else ""
        self._emit(("log", f"SEQ layer flattened ({layers} layers){grew}", False))
        self._emit(("seq",))
        return True

    def drop(self) -> bool:
        """Abandon the pending take; the backbone is untouched."""
        if self.state() == SEQ_OVERDUB:
            self._finish_overdub()
        with self._lock:
            had = self._seq.drop_pending()
            self._rec_events = []
            if self._state in (SEQ_REVIEW, SEQ_OVERDUB):
                self._state = SEQ_PLAYING if self._transport_alive() else SEQ_STOPPED
        self._emit(("log", "SEQ layer dropped" if had else "SEQ nothing to drop", False))
        self._emit(("seq",))
        return had

    def undo(self) -> bool:
        """Peel the newest flattened layer back off."""
        with self._lock:
            layer = self._seq.undo_layer()
            layers = len(self._seq.layers)
        if layer is None:
            self._emit(("log", "SEQ nothing to undo (backbone stays — use CLEAR)", False))
            return False
        self._emit(("log", f"SEQ layer undone ({layers} left)", False))
        self._emit(("seq",))
        return True

    def set_cycles(self, cycles: int) -> bool:
        with self._lock:
            if self._state in (SEQ_REC_BACKBONE, SEQ_OVERDUB):
                return False
            ok = self._seq.set_cycles(cycles)
            now = self._seq.cycles
        if ok:
            self._emit(("log", f"SEQ length {now} × backbone", False))
            self._emit(("seq",))
        return ok

    def double_length(self) -> bool:
        with self._lock:
            want = self._seq.cycles * 2
        return self.set_cycles(want)

    def halve_length(self) -> bool:
        with self._lock:
            want = max(1, self._seq.cycles // 2)
        return self.set_cycles(want)

    def set_extend(self, enabled: bool) -> bool:
        with self._lock:
            self._extend = bool(enabled)
            value = self._extend
        self._emit(("log", f"SEQ overdub {'EXTEND' if value else 'WRAP'}", False))
        return value

    def toggle_extend(self) -> bool:
        return self.set_extend(not self.extend_mode())

    def clear(self) -> None:
        self.stop_playback()
        with self._lock:
            self._seq.clear()
            self._rec_events = []
            self._state = SEQ_EMPTY
        self._emit(("log", "SEQ cleared", False))
        self._emit(("seq",))

    # ------------------------------------------------------------- playback

    def start_playback(self) -> bool:
        with self._lock:
            if self._seq.is_empty():
                return False
            if self._state in (SEQ_PLAYING, SEQ_OVERDUB, SEQ_REVIEW):
                return True
            self._state = SEQ_PLAYING
            self._pass_t0 = self._now()
            self._stop_play.clear()
        thread = threading.Thread(target=self._play_loop, daemon=True, name="seq-play")
        self._thread = thread
        thread.start()
        return True

    def stop_playback(self) -> None:
        self._stop_play.set()
        thread = self._thread
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            thread.join(timeout=1.5)
        self._thread = None
        with self._lock:
            if self._state in (SEQ_PLAYING, SEQ_OVERDUB, SEQ_REVIEW):
                self._state = SEQ_STOPPED if not self._seq.is_empty() else SEQ_EMPTY
        self._release_all()

    def toggle_play(self) -> bool:
        if self.is_playing():
            self.stop_playback()
            return False
        return self.start_playback()

    def stop(self) -> None:
        """Panic / mode-exit: end any take, keep the material, stop the transport."""
        if self.is_recording():
            self.stop_record()
        self.stop_playback()

    def _sleep_until(self, target: float) -> bool:
        """Wait for `target`; True if we were asked to stop."""
        while True:
            remain = target - self._now()
            if remain <= 0:
                return False
            if self._stop_play.wait(min(0.003, remain)):
                return True

    def _play_loop(self) -> None:
        try:
            while not self._stop_play.is_set():
                with self._lock:
                    schedule = self._seq.schedule()
                    total = self._seq.total_len()
                if total <= 0.0:
                    break
                t0 = self._now()
                with self._lock:
                    self._pass_t0 = t0
                for ev in schedule:
                    if self._stop_play.is_set():
                        break
                    if self._sleep_until(t0 + ev.t):
                        break
                    self._fire(ev)
                if self._stop_play.is_set():
                    break
                if self._sleep_until(t0 + total):
                    break
                # Notes that wrap the loop point are fine; a note held longer
                # than a whole pass means its note-off never arrived.
                self._release_stale(total)
        finally:
            self._release_all()
            with self._lock:
                if self._state in (SEQ_PLAYING, SEQ_OVERDUB, SEQ_REVIEW):
                    self._state = SEQ_STOPPED if not self._seq.is_empty() else SEQ_EMPTY

    def _fire(self, ev: LoopEvent) -> None:
        key = (ev.channel, ev.note)
        if ev.on:
            try:
                self._engine.note_on(ev.channel, ev.note, ev.velocity)
            except Exception:
                return
            self._held[key] = self._now()
            self._safe_emit(("on", ev.channel, ev.note, ev.velocity))
        else:
            try:
                self._engine.note_off(ev.channel, ev.note)
            except Exception:
                pass
            self._held.pop(key, None)
            self._safe_emit(("off", ev.channel, ev.note))

    def _safe_emit(self, msg: tuple) -> None:
        try:
            self._emit(msg)
        except Exception:
            pass

    def _release_stale(self, total: float) -> None:
        now = self._now()
        cutoff = total + HELD_NOTE_GRACE
        for (channel, note), started in list(self._held.items()):
            if now - started >= cutoff:
                self._release_one(channel, note)

    def _release_all(self) -> None:
        for channel, note in list(self._held.keys()):
            self._release_one(channel, note)

    def _release_one(self, channel: int, note: int) -> None:
        self._held.pop((channel, note), None)
        try:
            self._engine.note_off(channel, note)
        except Exception:
            pass
        self._safe_emit(("off", channel, note))


def events_to_dicts(events: Seq[LoopEvent]) -> List[Dict[str, object]]:
    return [
        {
            "t": float(e.t),
            "on": bool(e.on),
            "channel": int(e.channel),
            "note": int(e.note),
            "velocity": int(e.velocity),
        }
        for e in events
    ]
