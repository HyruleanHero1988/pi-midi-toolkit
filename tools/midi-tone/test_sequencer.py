#!/usr/bin/env python3
"""
Timing rules for the overdub sequencer — no audio device, no Tk, no Rust build.

The transport is driven by a fake clock everywhere except one short real-time
smoke test, so these run in well under a second on any machine.
"""

from __future__ import annotations

import threading
import time
import unittest

from sequencer import (
    MAX_CYCLES,
    SEQ_EMPTY,
    SEQ_OVERDUB,
    SEQ_PLAYING,
    SEQ_REVIEW,
    SEQ_STOPPED,
    LoopEvent,
    OverdubSequencer,
    SeqLayer,
    Sequence,
    close_open_notes,
    cycles_for_take,
    tile_layer,
    trim_loop_take,
)


class FakeEngine:
    """Records what the sequencer asked the synth to play."""

    def __init__(self, vib=(0.0, 5.0, 0.0)) -> None:
        self.calls: list[tuple] = []
        self.vibs: list = []
        self._vib = tuple(float(v) for v in vib)
        self._lock = threading.Lock()

    def note_on(self, channel: int, note: int, velocity: int, **kwargs) -> None:
        with self._lock:
            self.calls.append(("on", channel, note, velocity))
            self.vibs.append(kwargs.get("vib"))

    def note_off(self, channel: int, note: int) -> None:
        with self._lock:
            self.calls.append(("off", channel, note))

    def vib_state(self):
        return self._vib

    def ons(self) -> list[int]:
        with self._lock:
            return [c[2] for c in self.calls if c[0] == "on"]


class FakeClock:
    """Monotonic time the test advances by hand."""

    def __init__(self) -> None:
        self.t = 1000.0

    def __call__(self) -> float:
        return self.t

    def advance(self, dt: float) -> float:
        self.t += dt
        return self.t


def hit(seq: OverdubSequencer, clock: FakeClock, note: int, *, channel: int = 9,
        dt: float = 0.0, hold: float = 0.01) -> None:
    """Advance the clock, then play one short note into the recorder."""
    clock.advance(dt)
    seq.record_note(True, channel, note, 100)
    clock.advance(hold)
    seq.record_note(False, channel, note, 0)


def make_seq(vib=(0.0, 5.0, 0.0), **kwargs) -> tuple[OverdubSequencer, FakeClock, FakeEngine]:
    engine = FakeEngine(vib=vib)
    clock = FakeClock()
    seq = OverdubSequencer(engine, lambda _m: None, autoplay=False, clock=clock, **kwargs)
    return seq, clock, engine


def record_backbone(seq: OverdubSequencer, clock: FakeClock, notes=(36, 38, 36, 38)) -> None:
    seq.start_backbone()
    clock.advance(0.7)  # pre-roll dead air, should be trimmed away
    for note in notes:
        hit(seq, clock, note, dt=0.25)
    clock.advance(1.9)  # slow finger on STOP, should be capped
    seq.stop_record()


class TrimTest(unittest.TestCase):
    def test_drops_pre_roll_and_caps_trail(self) -> None:
        events = [
            LoopEvent(t=2.0, on=True, channel=9, note=36, velocity=100),
            LoopEvent(t=2.05, on=False, channel=9, note=36, velocity=0),
            LoopEvent(t=2.5, on=True, channel=9, note=38, velocity=100),
            LoopEvent(t=2.55, on=False, channel=9, note=38, velocity=0),
            LoopEvent(t=3.0, on=True, channel=9, note=36, velocity=100),
            LoopEvent(t=3.05, on=False, channel=9, note=36, velocity=0),
        ]
        trimmed, length = trim_loop_take(events)
        self.assertAlmostEqual(trimmed[0].t, 0.0, places=6)
        # Largest gap between onsets is 0.5s, so the take ends 0.5s after the last hit
        self.assertAlmostEqual(length, 1.5, places=6)

    def test_single_hit_gets_small_pad_not_stop_lag(self) -> None:
        events = [
            LoopEvent(t=4.0, on=True, channel=9, note=36, velocity=100),
            LoopEvent(t=4.05, on=False, channel=9, note=36, velocity=0),
        ]
        _trimmed, length = trim_loop_take(events)
        self.assertLess(length, 0.5)

    def test_empty_take(self) -> None:
        self.assertEqual(trim_loop_take([]), ([], 0.0))


class ModelTest(unittest.TestCase):
    def test_close_open_notes_adds_missing_off(self) -> None:
        events = [LoopEvent(t=0.1, on=True, channel=0, note=60, velocity=100)]
        closed = close_open_notes(events, 2.0)
        offs = [e for e in closed if not e.on]
        self.assertEqual(len(offs), 1)
        self.assertLessEqual(offs[0].t, 2.0)
        self.assertGreater(offs[0].t, 0.1)

    def test_tile_repeats_short_layer_across_sequence(self) -> None:
        layer = SeqLayer(events=[LoopEvent(t=0.1, on=True, channel=9, note=36, velocity=100)])
        tiled = tile_layer(layer, cycle_len=2.0, cycles=4)
        self.assertEqual([round(e.t, 3) for e in tiled], [0.1, 2.1, 4.1, 6.1])

    def test_tile_long_layer_plays_once(self) -> None:
        layer = SeqLayer(
            events=[LoopEvent(t=5.0, on=True, channel=0, note=60, velocity=100)], span=4
        )
        tiled = tile_layer(layer, cycle_len=2.0, cycles=4)
        self.assertEqual([round(e.t, 3) for e in tiled], [5.0])

    def test_cycles_for_take_rounds_up(self) -> None:
        self.assertEqual(cycles_for_take(1.9, 2.0), 1)
        self.assertEqual(cycles_for_take(2.1, 2.0), 2)
        self.assertEqual(cycles_for_take(6.5, 2.0), 4)
        self.assertEqual(cycles_for_take(99.0, 2.0), MAX_CYCLES)

    def test_cannot_shrink_below_longest_layer(self) -> None:
        seq = Sequence()
        seq.set_backbone([LoopEvent(t=0.0, on=True, channel=9, note=36, velocity=100)], 2.0)
        seq.pending = SeqLayer(
            events=[LoopEvent(t=5.0, on=True, channel=0, note=60, velocity=100)], span=4
        )
        seq.keep_pending()
        self.assertEqual(seq.cycles, 4)
        self.assertFalse(seq.set_cycles(2))
        self.assertEqual(seq.cycles, 4)


class BackboneTest(unittest.TestCase):
    def test_first_take_locks_the_loop_length(self) -> None:
        seq, clock, _engine = make_seq()
        self.assertEqual(seq.state(), SEQ_EMPTY)
        record_backbone(seq, clock)
        st = seq.status()
        self.assertEqual(st["state"], SEQ_STOPPED)
        self.assertEqual(st["layers"], 1)
        self.assertEqual(st["cycles"], 1)
        # 4 onsets 0.26s apart plus one gap of trail — not the 0.7s pre-roll
        # or the 1.9s spent reaching for STOP
        self.assertAlmostEqual(float(st["length"]), 1.04, places=2)

    def test_empty_take_leaves_sequence_empty(self) -> None:
        seq, clock, _engine = make_seq()
        seq.start_backbone()
        clock.advance(2.0)
        seq.stop_record()
        self.assertEqual(seq.state(), SEQ_EMPTY)
        self.assertTrue(seq.status()["events"] == 0)

    def test_re_recording_backbone_replaces_everything(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        first = seq.status()["length"]
        record_backbone(seq, clock, notes=(40, 40))
        self.assertEqual(seq.status()["layers"], 1)
        self.assertNotAlmostEqual(float(seq.status()["length"]), float(first), places=3)


class OverdubTest(unittest.TestCase):
    def start_overdub_at(self, seq: OverdubSequencer, clock: FakeClock, phase: float) -> None:
        """Pretend the transport is mid-pass at `phase` and start an overdub."""
        seq._pass_t0 = clock.t - phase  # transport bookkeeping the thread would own
        seq._state = SEQ_PLAYING
        seq.start_overdub()

    def test_overdub_wraps_into_the_backbone_cycle(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        total = float(seq.status()["length"])
        self.start_overdub_at(seq, clock, phase=0.2)
        self.assertEqual(seq.state(), SEQ_OVERDUB)
        # Play past the loop point: the late hit folds back to the top of the loop
        hit(seq, clock, 42, dt=total - 0.3)
        seq.stop_record()
        self.assertEqual(seq.state(), SEQ_REVIEW)
        pending = seq._seq.pending
        assert pending is not None
        for ev in pending.events:
            self.assertLess(ev.t, total)
        self.assertEqual(pending.span, 1)

    def test_keep_flattens_layer_and_undo_peels_it_back(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        before = int(seq.status()["events"])
        self.start_overdub_at(seq, clock, phase=0.1)
        hit(seq, clock, 42, dt=0.2)
        hit(seq, clock, 42, dt=0.2)
        seq.stop_record()
        self.assertTrue(seq.keep())
        st = seq.status()
        self.assertEqual(st["layers"], 2)
        self.assertEqual(st["pending"], 0)
        self.assertGreater(int(st["events"]), before)
        self.assertTrue(seq.undo())
        self.assertEqual(seq.status()["layers"], 1)
        self.assertEqual(int(seq.status()["events"]), before)

    def test_drop_abandons_layer(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        before = int(seq.status()["events"])
        self.start_overdub_at(seq, clock, phase=0.1)
        hit(seq, clock, 46, dt=0.2)
        seq.stop_record()
        self.assertTrue(seq.drop())
        st = seq.status()
        self.assertEqual(st["layers"], 1)
        self.assertEqual(int(st["events"]), before)

    def test_undo_never_eats_the_backbone(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        self.assertFalse(seq.undo())
        self.assertEqual(seq.status()["layers"], 1)

    def test_pending_take_is_audible_before_you_decide(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        self.start_overdub_at(seq, clock, phase=0.1)
        hit(seq, clock, 42, dt=0.2)
        notes = {e.note for e in seq._seq.schedule()}
        self.assertIn(42, notes)
        seq.stop_record()
        seq.drop()
        self.assertNotIn(42, {e.note for e in seq._seq.schedule()})

    def test_extend_grows_sequence_to_whole_backbone_cycles(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        cycle = float(seq.status()["length"])
        seq.set_extend(True)
        self.start_overdub_at(seq, clock, phase=0.0)
        # Keep playing well past one cycle: the take should stretch the sequence
        hit(seq, clock, 60, channel=0, dt=0.1)
        hit(seq, clock, 62, channel=0, dt=cycle)
        seq.stop_record()
        self.assertTrue(seq.keep())
        st = seq.status()
        self.assertEqual(int(st["cycles"]), 2)
        self.assertAlmostEqual(float(st["length"]), cycle * 2, places=6)
        # Backbone still repeats underneath, once per cycle
        schedule = seq._seq.schedule()
        backbone_ons = [e for e in schedule if e.on and e.channel == 9]
        self.assertEqual(len(backbone_ons), 8)

    def test_undo_of_long_layer_restores_short_sequence(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        cycle = float(seq.status()["length"])
        seq.set_extend(True)
        self.start_overdub_at(seq, clock, phase=0.0)
        hit(seq, clock, 60, channel=0, dt=cycle + 0.1)
        seq.stop_record()
        seq.keep()
        self.assertEqual(int(seq.status()["cycles"]), 2)
        seq.undo()
        self.assertEqual(int(seq.status()["cycles"]), 1)

    def test_length_doubling_tiles_the_backbone(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        one_pass = len(seq._seq.schedule())
        self.assertTrue(seq.double_length())
        self.assertEqual(len(seq._seq.schedule()), one_pass * 2)
        self.assertTrue(seq.halve_length())
        self.assertEqual(len(seq._seq.schedule()), one_pass)

    def test_starting_a_new_take_keeps_an_unresolved_one(self) -> None:
        seq, clock, _engine = make_seq()
        record_backbone(seq, clock)
        self.start_overdub_at(seq, clock, phase=0.1)
        hit(seq, clock, 42, dt=0.2)
        seq.stop_record()
        self.start_overdub_at(seq, clock, phase=0.1)
        self.assertEqual(seq.status()["layers"], 2)
        self.assertEqual(seq.state(), SEQ_OVERDUB)


class LayerVibTest(unittest.TestCase):
    def start_overdub_at(self, seq: OverdubSequencer, clock: FakeClock, phase: float) -> None:
        seq._pass_t0 = clock.t - phase
        seq._state = SEQ_PLAYING
        seq.start_overdub()

    def test_backbone_bakes_the_vibrato_it_was_played_with(self) -> None:
        seq, clock, engine = make_seq(vib=(0.9, 4.0, 1.0))
        record_backbone(seq, clock)
        layer = seq._seq.layers[0]
        self.assertTrue(layer.vib_baked)
        self.assertEqual(layer.vib_tuple(), (0.9, 4.0, 1.0))
        # Changing the live rig afterwards must not reach the take
        engine._vib = (0.0, 5.0, 0.0)
        self.assertEqual(seq._seq.layers[0].vib_tuple(), (0.9, 4.0, 1.0))

    def test_overdub_layer_keeps_its_own_vibrato(self) -> None:
        seq, clock, engine = make_seq(vib=(0.0, 5.0, 0.0))
        record_backbone(seq, clock)
        engine._vib = (1.2, 6.5, 1.0)
        self.start_overdub_at(seq, clock, phase=0.1)
        hit(seq, clock, 60, channel=0, dt=0.2)
        seq.stop_record()
        self.assertTrue(seq.keep())
        self.assertEqual(seq._seq.layers[0].vib_tuple(), (0.0, 5.0, 0.0))
        self.assertEqual(seq._seq.layers[1].vib_tuple(), (1.2, 6.5, 1.0))

    def test_schedule_hands_vibrato_to_key_notes_not_drums(self) -> None:
        seq = Sequence()
        seq.set_backbone(
            [
                LoopEvent(t=0.0, on=True, channel=9, note=36, velocity=100),
                LoopEvent(t=0.05, on=True, channel=0, note=60, velocity=100),
            ],
            1.0,
            vib=(0.8, 5.0, 1.0),
        )
        schedule = seq.schedule()
        drum = next(s for s in schedule if s.channel == 9 and s.on)
        key = next(s for s in schedule if s.channel == 0 and s.on)
        self.assertIsNone(drum.vib)
        self.assertEqual(key.vib, (0.8, 5.0, 1.0))

    def test_playback_passes_baked_vibrato_to_the_synth(self) -> None:
        engine = FakeEngine()
        seq = OverdubSequencer(engine, lambda _m: None, autoplay=False)
        seq._seq.set_backbone(
            [
                LoopEvent(t=0.0, on=True, channel=0, note=60, velocity=100),
                LoopEvent(t=0.05, on=False, channel=0, note=60, velocity=0),
            ],
            0.15,
            vib=(1.0, 5.5, 1.0),
        )
        seq._state = SEQ_STOPPED
        self.assertTrue(seq.start_playback())
        time.sleep(0.25)
        seq.stop_playback()
        self.assertIn((1.0, 5.5, 1.0), engine.vibs)

    def test_dry_take_still_bakes_as_none(self) -> None:
        seq, clock, _engine = make_seq(vib=(0.0, 5.0, 0.0))
        record_backbone(seq, clock)
        self.assertTrue(seq._seq.layers[0].vib_baked)
        self.assertEqual(seq._seq.layers[0].vib_label(), "none")


class TransportTest(unittest.TestCase):
    def test_playback_fires_notes_and_loops(self) -> None:
        engine = FakeEngine()
        seq = OverdubSequencer(engine, lambda _m: None, autoplay=False)
        seq._seq.set_backbone(
            [
                LoopEvent(t=0.00, on=True, channel=9, note=36, velocity=100),
                LoopEvent(t=0.01, on=False, channel=9, note=36, velocity=0),
                LoopEvent(t=0.05, on=True, channel=9, note=38, velocity=100),
                LoopEvent(t=0.06, on=False, channel=9, note=38, velocity=0),
            ],
            0.10,
        )
        seq._state = SEQ_STOPPED
        self.assertTrue(seq.start_playback())
        time.sleep(0.35)
        seq.stop_playback()
        ons = engine.ons()
        self.assertGreaterEqual(ons.count(36), 2, "loop should repeat")
        self.assertGreaterEqual(ons.count(38), 2)
        self.assertEqual(seq.state(), SEQ_STOPPED)

    def test_stop_releases_held_notes(self) -> None:
        engine = FakeEngine()
        seq = OverdubSequencer(engine, lambda _m: None, autoplay=False)
        seq._seq.set_backbone(
            [LoopEvent(t=0.0, on=True, channel=0, note=60, velocity=100)], 1.0
        )
        seq._state = SEQ_STOPPED
        seq.start_playback()
        time.sleep(0.1)
        seq.stop_playback()
        self.assertIn(("off", 0, 60), engine.calls)


class SnapshotTest(unittest.TestCase):
    def test_export_import_roundtrip_keeps_layers(self) -> None:
        engine = FakeEngine()
        seq = OverdubSequencer(engine, lambda _m: None, autoplay=False)
        seq._seq.set_backbone(
            [
                LoopEvent(t=0.0, on=True, channel=9, note=36, velocity=100),
                LoopEvent(t=0.02, on=False, channel=9, note=36, velocity=0),
            ],
            1.0,
        )
        pending = SeqLayer(
            events=[
                LoopEvent(t=0.25, on=True, channel=0, note=60, velocity=90),
                LoopEvent(t=0.40, on=False, channel=0, note=60, velocity=0),
            ],
            span=1,
        )
        pending.set_vib(0.4, 5.0, 0.8)
        seq._seq.pending = pending
        seq._seq.keep_pending()
        seq._extend = True
        seq._state = SEQ_STOPPED

        blob = seq.export_state()
        other = OverdubSequencer(FakeEngine(), lambda _m: None, autoplay=False)
        other.import_state(blob)
        st = other.status()
        self.assertEqual(st["layers"], 2)
        self.assertEqual(st["state"], SEQ_STOPPED)
        self.assertTrue(other._extend)
        self.assertAlmostEqual(float(st["length"]), 1.0, places=5)
        vib = other._seq.layers[1]
        self.assertAlmostEqual(vib.vib_depth, 0.4, places=5)
        self.assertAlmostEqual(vib.vib_amount, 0.8, places=5)


if __name__ == "__main__":
    unittest.main()
